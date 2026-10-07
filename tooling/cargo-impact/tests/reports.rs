use std::{collections::BTreeMap, fs};

use cargo_impact::{
    BuildResult, Classification, CompilerDiagnostic, DownstreamResult, DownstreamSource,
    ImpactReport, RunStatus,
    config::Config,
    forge::{Forge, Repository},
    runner::BuildRecipe,
};
use serde_json::json;
use tempfile::TempDir;

fn regression(name: &str) -> DownstreamResult {
    let diagnostic: CompilerDiagnostic = serde_json::from_value(json!({
        "package_id":"consumer 1.0.0", "target":null,
        "package":{"name":"consumer","manifest":"Cargo.toml","origin":"downstream"},
        "source_files":{"/worker/src/a b#?.rs":"src/a b#?.rs"},
        "message":"method is private <untrusted>","code":{"code":"E0624","explanation":null},"level":"error",
        "spans":[{"file_name":"/worker/src/a b#?.rs","byte_start":1,"byte_end":20,"line_start":42,"line_end":42,"column_start":9,"column_end":17,"is_primary":true,
            "text":[{"text":"préfixe λ.retiré(); // @foo","highlight_start":9,"highlight_end":17}],"label":"private method","suggested_replacement":null,"suggestion_applicability":null,"expansion":null}],
        "children":[],"rendered":"error[E0624]: method is private\n<script>alert(\"PWN\")</script>\n"
    })).unwrap();
    DownstreamResult {
        name: name.into(),
        revision: Some("a".repeat(40)),
        source: DownstreamSource::Git {
            url: "https://gitlab.com/group/subgroup/consumer".into(),
            revision: "HEAD".into(),
            forge: None,
        },
        manifest: "crates/consumer/Cargo.toml".into(),
        source_fingerprint: Some("source-hash".into()),
        recipe: BuildRecipe::default(),
        classification: Classification::Regression,
        baseline: BuildResult {
            success: true,
            ..Default::default()
        },
        candidate: BuildResult {
            diagnostics: vec![diagnostic.clone(), diagnostic],
            ..Default::default()
        },
        message: None,
        elapsed_ms: 1234,
    }
}

fn report() -> ImpactReport {
    let mut report = ImpactReport::failed("demo-lib", "initial");
    report.error = None;
    report.gate.ran = true;
    report.gate.reason = "An API change requires downstream experiments.".into();
    report.baseline_fingerprint = Some("base-hash".into());
    report.candidate_fingerprint = Some("head-hash".into());
    report.downstreams.push(regression("consumer"));
    report
}

#[test]
fn evidence_is_highlighted_deduplicated_and_commit_pinned_in_every_view() {
    let report = report();
    let markdown = report.markdown();
    assert!(markdown.contains("**` λ.retiré `**"), "{markdown}");
    assert!(markdown.contains("```rust\npréfixe"));
    assert_eq!(markdown.matches("### E0624:").count(), 1);
    let html = report.html();
    assert!(html.contains("<mark>λ.retiré</mark>"));
    assert!(html.contains("// @foo"), "source text must remain exact");
    assert!(!html.contains("<script>alert(\"PWN\")"));
    assert!(html.contains("src/a%20b%23%3F.rs#L42"));
    assert!(html.contains("Runtime and storage measurements were not recorded"));
    let sarif = report.sarif();
    let result = &sarif["runs"][0]["results"][0];
    assert_eq!(sarif["runs"][0]["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["locations"][0]["physicalLocation"]["region"]["startLine"],
        42
    );
    assert!(
        result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .unwrap()
            .contains(&"a".repeat(40))
    );
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["executionSuccessful"],
        true
    );
}

#[test]
fn report_navigation_never_links_to_a_missing_consumer_anchor() {
    let mut report = report();
    for index in 0..45 {
        report.downstreams.push(regression(&format!("app-{index}")));
    }
    let mut compatible = regression("compatible");
    compatible.classification = Classification::Compatible;
    compatible.candidate = compatible.baseline.clone();
    report.downstreams.insert(0, compatible);
    let markdown = report.markdown();
    for tail in markdown.split("](#").skip(1) {
        let id = tail.split(')').next().unwrap();
        assert!(
            markdown.contains(&format!("name=\"{id}\"")),
            "dead anchor {id}"
        );
    }
    assert!(markdown.contains("Recorded in JSON"));
}

#[test]
fn issue_bundle_reproduces_selected_package_manifest_and_pinned_revision() {
    let mut report = report();
    report.downstreams[0].recipe.packages = vec!["consumer".into()];
    let draft = report.issue_drafts().remove(0);
    let config: Config = toml::from_str(&draft.reproduction_config).unwrap();
    assert_eq!(
        config.downstreams[0].manifest.to_str(),
        Some("crates/consumer/Cargo.toml")
    );
    assert_eq!(config.recipe.packages, vec!["consumer"]);
    assert!(!config.discovery.crates_io && !config.discovery.github);
    let DownstreamSource::Git { revision, .. } = &config.downstreams[0].source else {
        panic!()
    };
    assert_eq!(revision, &"a".repeat(40));
    assert!(
        draft
            .composer_url
            .unwrap()
            .path()
            .ends_with("/-/work_items/new")
    );
    assert!(draft.body.contains("base-hash") && draft.body.contains("head-hash"));
}

#[test]
fn checkpoints_and_new_runs_invalidate_old_issue_drafts_and_success_views() {
    let directory = TempDir::new().unwrap();
    let report = report();
    report.write_artifacts(directory.path()).unwrap();
    let draft = report.issue_drafts().remove(0);
    let path = directory.path().join("issues").join(&draft.filename);
    assert!(path.exists());
    fs::write(directory.path().join("issues/notes.md"), "user notes").unwrap();
    let mut next = ImpactReport::failed("another-lib", "preparing");
    next.run.status = RunStatus::Running;
    next.write_checkpoint(directory.path()).unwrap();
    assert!(!path.exists());
    assert_eq!(
        fs::read_to_string(directory.path().join("issues/notes.md")).unwrap(),
        "user notes"
    );
    assert!(
        fs::read_to_string(directory.path().join("index.html"))
            .unwrap()
            .contains("Partial downstream impact")
    );
    assert_eq!(
        ImpactReport::load(&directory.path().join("report.json"))
            .unwrap()
            .library,
        "another-lib"
    );
    let sarif: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("report.sarif")).unwrap()).unwrap();
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["executionSuccessful"],
        false
    );
    next.write_artifacts(directory.path()).unwrap();
    assert_eq!(
        fs::read_to_string(directory.path().join("issues/index.json")).unwrap(),
        "[]"
    );
}

#[test]
fn completed_evidence_survives_failure_to_write_a_secondary_view() {
    let directory = TempDir::new().unwrap();
    fs::create_dir(directory.path().join("index.html")).unwrap();
    let report = report();
    assert!(report.write_checkpoint(directory.path()).is_err());
    let saved = ImpactReport::load(&directory.path().join("report.json")).unwrap();
    assert_eq!(saved.downstreams.len(), 1);
    assert_eq!(
        saved.downstreams[0].classification,
        Classification::Regression
    );
}

#[test]
fn malformed_spans_and_unverified_paths_cannot_create_links_or_panic() {
    let mut report = report();
    report.downstreams[0].source = DownstreamSource::Local { path: ".".into() };
    let diagnostic = &mut report.downstreams[0].candidate.diagnostics[0];
    diagnostic.source_files =
        BTreeMap::from([("/worker/src/a b#?.rs".into(), "src/a b#?.rs".into())]);
    diagnostic.diagnostic.spans[0].text[0].highlight_start = usize::MAX;
    diagnostic.diagnostic.spans[0].text[0].highlight_end = 0;
    let _ = report.html();
    let sarif = report.sarif();
    assert_eq!(
        sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "src/a%20b%23%3F.rs"
    );
    for diagnostic in &mut report.downstreams[0].candidate.diagnostics {
        diagnostic.source_files.clear();
    }
    assert!(
        report.sarif()["runs"][0]["results"][0]["locations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn forge_issue_forms_encode_text_and_advertise_actual_capabilities() {
    for (url, forge, path, key) in [
        (
            "https://github.com/org/repo",
            None,
            "/org/repo/issues/new",
            "body",
        ),
        (
            "https://forge.example/org/repo",
            Some(Forge::Gitea),
            "/org/repo/issues/new",
            "body",
        ),
        (
            "https://gitlab.example/group/sub/repo",
            Some(Forge::GitLab),
            "/group/sub/repo/-/work_items/new",
            "issue[description]",
        ),
    ] {
        let repo = Repository::parse(url, forge).unwrap();
        let url = repo
            .issue_composer("a & b?", "line one\nline two # end")
            .unwrap();
        assert_eq!(url.path(), path);
        assert!(
            url.query_pairs()
                .any(|(k, v)| k == key && v == "line one\nline two # end")
        );
        assert!(repo.issue_composer("title", &"x".repeat(9_000)).is_none());
    }
    let repo = Repository::parse("https://unknown.example/org/repo", None).unwrap();
    assert!(!repo.capabilities().issue_composer);
    assert!(repo.issue_composer("title", "body").is_none());
}

#[test]
fn changed_input_diagnostics_do_not_claim_verified_source_links() {
    let mut report = report();
    report.downstreams[0].classification = Classification::HarnessFailure;
    report.downstreams[0].candidate.failure = Some(cargo_impact::HarnessFailure::InputMutation);
    assert!(!report.html().contains("src/a%20b%23%3F.rs#L42"));
    assert!(
        report.sarif()["runs"][0]["results"][0]["locations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn same_named_experiments_keep_their_own_issue_and_recipe_links() {
    let mut report = report();
    let mut alternate = report.downstreams[0].clone();
    alternate.recipe.features = vec!["alternate-feature".into()];
    report.downstreams.push(alternate);
    let drafts = report.issue_drafts();
    assert_ne!(drafts[0].id, drafts[1].id);
    assert_ne!(drafts[0].reproduction_config, drafts[1].reproduction_config);
    let html = report.html();
    let markdown = report.markdown();
    for draft in drafts {
        assert_eq!(
            html.matches(&format!("href=\"issues/{}\"", draft.filename))
                .count(),
            1
        );
        assert_eq!(
            html.matches(&format!("href=\"issues/{}\"", draft.reproduction_filename))
                .count(),
            1
        );
        assert_eq!(
            markdown
                .matches(&format!("issues/{}", draft.filename))
                .count(),
            1
        );
    }
}

#[test]
fn offline_report_loader_rejects_future_schema_and_truncated_json() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("report.json");
    let mut report = report();
    report.schema_version = 99;
    fs::write(&input, serde_json::to_vec(&report).unwrap()).unwrap();
    assert!(
        ImpactReport::load(&input)
            .unwrap_err()
            .to_string()
            .contains("schema 99")
    );
    fs::write(&input, "{\"schema_version\":1").unwrap();
    assert!(ImpactReport::load(&input).is_err());
}
