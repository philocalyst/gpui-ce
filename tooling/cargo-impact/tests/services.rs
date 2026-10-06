use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use cargo_impact::{
    bot::{self, BotError, CommentEvent},
    discovery::{CratesIo, Discover, GitHubSearch},
    forge::{Forge, Repository},
    http::{Api, ApiError},
};
use serde_json::json;
use url::Url;

type Response<'a> = (u16, serde_json::Value, Vec<(&'a str, &'a str)>);

struct Server {
    base: Url,
    requests: Arc<Mutex<Vec<String>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn new(responses: Vec<Response<'_>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = requests.clone();
        let replies: Vec<_> = responses
            .into_iter()
            .map(|(s, b, h)| {
                (
                    s,
                    b,
                    h.into_iter()
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let thread = thread::spawn(move || {
            for (status, body, headers) in replies {
                let started = Instant::now();
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                started.elapsed() < Duration::from_secs(5),
                                "expected request did not arrive"
                            );
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let size = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..size]);
                    if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length: ")?.parse::<usize>().ok()
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                    if size == 0 {
                        break;
                    }
                }
                received
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&request).into_owned());
                let body = body.to_string();
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len()).unwrap();
                for (key, value) in headers {
                    write!(stream, "{key}: {value}\r\n").unwrap();
                }
                write!(stream, "\r\n{body}").unwrap();
            }
        });
        Self {
            base,
            requests,
            thread: Some(thread),
        }
    }

    fn api(&self) -> Api {
        Api::new(self.base.clone(), Some("fixture-token".into())).unwrap()
    }
    fn finish(mut self) -> Vec<String> {
        self.thread.take().unwrap().join().unwrap();
        Arc::try_unwrap(self.requests)
            .unwrap()
            .into_inner()
            .unwrap()
    }
}

#[test]
fn registry_paginates_and_preserves_multi_package_repository_evidence() {
    let first = json!({"dependencies": [ {} ], "versions": [
        {"crate":"a", "num":"1.0.0", "repository":"https://gitlab.com/group/consumer.git"},
        {"crate":"unhosted", "num":"1.0.0", "repository":null}
    ], "meta":{"total":2}});
    let second = json!({"dependencies": [ {} ], "versions": [
        {"crate":"b", "num":"1.0.0", "repository":"https://gitlab.com/group/consumer"}
    ], "meta":{"total":2}});
    let server = Server::new(vec![(200, first, vec![]), (200, second, vec![])]);
    let discovery = CratesIo {
        api: server.api(),
        max_pages: 10,
    }
    .discover("demo-lib")
    .unwrap();
    assert_eq!(discovery.candidates.len(), 1);
    assert_eq!(discovery.candidates[0].packages.len(), 2);
    assert_eq!(discovery.candidates[0].repository.forge(), Forge::GitLab);
    assert!(discovery.notes.iter().any(|v| v.contains("no repository")));
    let requests = server.finish();
    assert!(requests[0].starts_with("GET /api/v1/crates/demo-lib/reverse_dependencies?"));
    assert!(requests[1].contains("page=2"));
}

#[test]
fn search_truncation_and_false_positive_candidates_are_explicit() {
    let server = Server::new(vec![(
        200,
        json!({
            "total_count":1001, "incomplete_results":true, "items":[
                {"path":"app/Cargo.toml", "repository":{"html_url":"https://github.com/org/app"}},
                {"path":"../Cargo.toml", "repository":{"html_url":"https://github.com/org/unsafe"}}
            ]
        }),
        vec![],
    )]);
    let discovery = GitHubSearch {
        api: server.api(),
        max_pages: 1,
    }
    .discover("demo-lib")
    .unwrap();
    assert_eq!(discovery.candidates.len(), 1);
    assert!(discovery.notes.iter().any(|v| v.contains("capped")));
    assert!(
        discovery
            .notes
            .iter()
            .any(|v| v.contains("incomplete results"))
    );
    assert!(discovery.notes.iter().any(|v| v.contains("unsafe")));
    server.finish();
}

#[test]
fn rate_limit_is_actionable_and_requests_do_not_retry_indefinitely() {
    let server = Server::new(vec![(429, json!({}), vec![("Retry-After", "42")])]);
    let error = server
        .api()
        .get::<serde_json::Value>("search/code", &[])
        .unwrap_err();
    assert!(matches!(
        error,
        ApiError::RateLimited {
            retry_after_seconds: Some(42)
        }
    ));
    assert_eq!(server.finish().len(), 1);
}

#[test]
fn later_discovery_failure_retains_earlier_candidates() {
    let first = json!({"dependencies":[{}], "versions":[{"crate":"app", "num":"1.0.0", "repository":"https://codeberg.org/org/app"}], "meta":{"total":2}});
    let server = Server::new(vec![
        (200, first, vec![]),
        (429, json!({}), vec![("Retry-After", "60")]),
    ]);
    let discovery = CratesIo {
        api: server.api(),
        max_pages: 3,
    }
    .discover("demo-lib")
    .unwrap();
    assert_eq!(discovery.candidates.len(), 1);
    assert!(
        discovery
            .notes
            .iter()
            .any(|note| note.starts_with("Discovery failed:")
                && note.contains("earlier candidates retained"))
    );
    assert_eq!(server.finish().len(), 2);
}

#[test]
fn stale_bot_results_never_comment_on_a_new_head() {
    let server = Server::new(vec![(
        200,
        json!({"state":"open","base":{"sha":"a".repeat(40),"repo":{"full_name":"org/lib"}},"head":{"sha":"c".repeat(40),"repo":{"full_name":"org/lib"}}}),
        vec![],
    )]);
    let report = cargo_impact::ImpactReport::failed("demo", "missing system dependency");
    assert!(
        !bot::publish(
            &server.api(),
            "org/lib",
            7,
            &"b".repeat(40),
            &report,
            &Url::parse("https://github.com/org/lib/actions/runs/1").unwrap()
        )
        .unwrap()
    );
    assert_eq!(server.finish().len(), 1);
}

#[test]
fn bot_reports_harness_failures_and_neutralizes_untrusted_mentions_and_fences() {
    let sha = "b".repeat(40);
    let server = Server::new(vec![
        (
            200,
            json!({"state":"open","base":{"sha":"a".repeat(40),"repo":{"full_name":"org/lib"}},"head":{"sha":sha,"repo":{"full_name":"org/lib"}}}),
            vec![],
        ),
        (201, json!({"id":1}), vec![]),
    ]);
    let report = cargo_impact::ImpactReport::failed(
        "[demo](bad) @everyone <img>",
        "```\n@everyone failed\n```",
    );
    assert!(report.has_harness_failures());
    assert!(!report.has_regressions());
    assert!(
        bot::publish(
            &server.api(),
            "org/lib",
            7,
            &sha,
            &report,
            &Url::parse("https://github.com/org/lib/actions/runs/1").unwrap()
        )
        .unwrap()
    );
    let requests = server.finish();
    let (_, body) = requests[1].split_once("\r\n\r\n").unwrap();
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    let text = body["body"].as_str().unwrap();
    assert!(text.contains("@\u{200b}everyone"));
    assert!(!text.contains("@everyone"));
    assert!(text.contains("````text"));
    assert!(text.contains("&lt;img&gt;"));
    assert!(text.contains("result is inconclusive"));
}

fn event(body: &str) -> CommentEvent {
    serde_json::from_value(json!({
        "action":"created", "repository":{"full_name":"org/lib"},
        "issue":{"number":7,"pull_request":{}},
        "comment":{"id":123,"body":body,"user":{"login":"maintainer","type":"User"},"author_association":"OWNER"}
    })).unwrap()
}

#[test]
fn bot_rechecks_current_permission_before_reacting() {
    let server = Server::new(vec![(200, json!({"permission":"read"}), vec![])]);
    let error = bot::plan(
        &server.api(),
        &event("@cargo-impact check"),
        "org/lib",
        "cargo-impact",
    )
    .unwrap_err();
    assert!(matches!(error, BotError::Unauthorized));
    let requests = server.finish();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("collaborators/maintainer/permission"));
}

#[test]
fn authorized_bot_pins_both_shas_and_acknowledges() {
    let baseline = "a".repeat(40);
    let candidate = "b".repeat(40);
    let server = Server::new(vec![
        (200, json!({"permission":"maintain"}), vec![]),
        (
            200,
            json!({"state":"open","base":{"sha":baseline,"repo":{"full_name":"org/lib"}},"head":{"sha":candidate,"repo":{"full_name":"contributor/lib"}}}),
            vec![],
        ),
        (201, json!({"id":1}), vec![]),
    ]);
    let api = server.api();
    let plan = bot::plan(
        &api,
        &event("@cargo-impact check"),
        "org/lib",
        "cargo-impact",
    )
    .unwrap()
    .unwrap();
    assert_eq!(plan.baseline_sha, baseline);
    assert_eq!(plan.candidate_sha, candidate);
    bot::acknowledge(&api, &plan).unwrap();
    let requests = server.finish();
    assert!(requests[2].starts_with("POST /repos/org/lib/issues/comments/123/reactions"));
    assert!(requests[2].contains("\"eyes\""));
}

#[test]
fn bot_ignores_quotes_arguments_and_edited_comments() {
    let api = Api::new(Url::parse("http://127.0.0.1:1/").unwrap(), None).unwrap();
    for body in [
        "> @cargo-impact check",
        "@cargo-impact check --shell rm",
        "@cargo-impact check\nmore",
    ] {
        assert!(
            bot::plan(&api, &event(body), "org/lib", "cargo-impact")
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn source_links_handle_forges_and_reject_unsafe_identities() {
    let sha = "a".repeat(40);
    for (input, forge, fragment) in [
        ("https://github.com/org/repo.git", None, "/blob/"),
        ("https://gitlab.com/group/subgroup/repo", None, "/-/blob/"),
        ("https://codeberg.org/org/repo", None, "/blob/"),
        (
            "https://forge.example/org/repo",
            Some(Forge::Gitea),
            "/blob/",
        ),
    ] {
        let repo = Repository::parse(input, forge).unwrap();
        let link = repo.source_link(&sha, "src/a b.rs", 12).unwrap();
        assert!(link.as_str().contains(fragment));
        assert!(link.as_str().contains("a%20b.rs#L12"));
        assert!(repo.source_link(&sha, "../private", 12).is_none());
    }
    for input in [
        "file:///etc/passwd",
        "https://token@github.com/org/repo",
        "https://github.com/org/repo?token=x",
    ] {
        assert!(Repository::parse(input, None).is_err());
    }
}
