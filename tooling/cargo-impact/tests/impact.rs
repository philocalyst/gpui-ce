use std::{fs, path::Path, time::Duration};

use cargo_impact::{Classification, DownstreamSpec, ImpactRequest, analyze};
use tempfile::TempDir;

#[test]
fn classifies_real_downstream_builds_against_both_library_versions() {
    let fixture = TempDir::new().unwrap();
    let baseline = fixture.path().join("upstream-baseline");
    let candidate = fixture.path().join("upstream-candidate");
    library(
        &baseline,
        "pub fn removed() -> u8 { 1 }\npub fn kept() -> u8 { 2 }\n",
    );
    library(&candidate, "pub fn kept() -> u8 { 2 }\n");

    let regress = downstream(
        fixture.path(),
        "regress",
        "pub fn use_api() { changed_lib::removed(); }\n",
    );
    let compatible = downstream(
        fixture.path(),
        "compatible",
        "pub fn use_api() { let _ = 1; }\n",
    );
    let preexisting = downstream(
        fixture.path(),
        "preexisting",
        "pub fn use_api() { missing_name(); }\n",
    );
    let broken_metadata = fixture.path().join("invalid-metadata");
    fs::create_dir_all(&broken_metadata).unwrap();
    fs::write(broken_metadata.join("Cargo.toml"), "this is not a manifest").unwrap();

    let mut report = analyze(&ImpactRequest {
        library: "changed-lib".into(),
        baseline,
        candidate,
        downstreams: vec![
            local("regression", regress),
            local("compatible", compatible),
            local("pre-existing", preexisting),
            local("invalid-metadata", broken_metadata),
        ],
        recipe: cargo_impact::runner::BuildRecipe {
            runner: cargo_impact::runner::Runner::Local,
            ..Default::default()
        },
        force: false,
        work_dir: Some(fixture.path().join("work")),
        timeout: Duration::from_secs(240),
        execution: Default::default(),
        upstream: None,
        semver_helper: Some(assert_cmd::cargo::cargo_bin!("cargo-impact").into()),
    })
    .expect("semver gate and fixture runs should complete");
    assert!(
        report.gate.ran,
        "removed public API should open the gate: {:?}",
        report.gate
    );
    assert_eq!(
        report.downstreams[0].classification,
        Classification::Regression
    );
    assert!(
        report.downstreams[0]
            .candidate
            .diagnostics
            .iter()
            .any(|diagnostic| {
                diagnostic
                    .message
                    .contains("cannot find function `removed`")
                    && diagnostic
                        .spans
                        .iter()
                        .any(|span| span.is_primary && !span.text.is_empty())
            })
    );
    assert_eq!(
        report.downstreams[1].classification,
        Classification::Compatible
    );
    assert_eq!(
        report.downstreams[2].classification,
        Classification::PreExistingFailure
    );
    assert_eq!(
        report.downstreams[3].classification,
        Classification::HarnessFailure
    );
    let markdown = report.markdown();
    let error = report.downstreams[0]
        .candidate
        .diagnostics
        .iter()
        .find(|diagnostic| {
            matches!(
                diagnostic.level,
                cargo_metadata::diagnostic::DiagnosticLevel::Error
            )
        })
        .expect("the regression should retain a compiler error");
    let error_code = error
        .code
        .as_ref()
        .expect("the fixture's compiler error should have a code")
        .code
        .clone();
    let duplicate = error.clone();
    report.downstreams[0].candidate.diagnostics.push(duplicate);
    assert_eq!(
        report.markdown(),
        markdown,
        "duplicate target errors should stay out of the human report"
    );
    let regression_section = markdown
        .split("\n## 🔴 regression · Regression\n")
        .nth(1)
        .expect("the regression details should be rendered")
        .split("\n## ")
        .next()
        .unwrap();
    assert_eq!(
        regression_section
            .matches(&format!("{error_code}:"))
            .count(),
        1,
        "the compiler error should appear once in the concise report"
    );

    // The same evidence can generate immutable forge links and survive report persistence.
    let sha = "a".repeat(40);
    report.downstreams[0].source = cargo_impact::DownstreamSource::Git {
        url: "https://gitlab.com/team/app".into(),
        revision: "main".into(),
        forge: Some(cargo_impact::forge::Forge::GitLab),
    };
    report.downstreams[0].revision = Some(sha.clone());
    assert!(
        report
            .markdown()
            .contains(&format!("/-/blob/{sha}/src/lib.rs#L1"))
    );
    let persisted = serde_json::to_vec(&report).unwrap();
    let restored: cargo_impact::ImpactReport = serde_json::from_slice(&persisted).unwrap();
    let error = restored.downstreams[0]
        .candidate
        .diagnostics
        .iter()
        .find(|d| d.code.as_ref().is_some_and(|c| c.code == "E0425"))
        .unwrap();
    let span = error.spans.iter().find(|s| s.is_primary).unwrap();
    assert!(span.byte_end > span.byte_start);
    assert!(!span.text.is_empty());
    assert!(error.rendered.as_ref().unwrap().contains("removed"));
    assert_eq!(restored.markdown(), report.markdown());
}

#[test]
fn semantic_break_without_downstream_impact_does_not_invent_a_regression() {
    let fixture = TempDir::new().unwrap();
    let baseline = fixture.path().join("upstream-baseline");
    let candidate = fixture.path().join("upstream-candidate");
    library(&baseline, "pub fn removed() -> u8 { 1 }\n");
    library(&candidate, "// API was removed\n");
    let consumer = downstream(fixture.path(), "consumer", "pub fn harmless() {}\n");

    let report = analyze(&ImpactRequest {
        library: "changed-lib".into(),
        baseline,
        candidate,
        downstreams: vec![local("consumer", consumer)],
        recipe: cargo_impact::runner::BuildRecipe {
            runner: cargo_impact::runner::Runner::Local,
            ..Default::default()
        },
        force: false,
        work_dir: None,
        timeout: Duration::from_secs(240),
        execution: Default::default(),
        upstream: None,
        semver_helper: None,
    })
    .unwrap();
    assert!(report.gate.ran);
    assert_eq!(
        report.downstreams[0].classification,
        Classification::Compatible
    );
}

fn local(name: &str, path: impl Into<std::path::PathBuf>) -> DownstreamSpec {
    DownstreamSpec::local(name, path)
}

fn library(root: &Path, source: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"changed-lib\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), source).unwrap();
}

fn downstream(root: &Path, name: &str, source: &str) -> std::path::PathBuf {
    let root = root.join("downstream").join(name);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fixture-consumer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nchanged-lib = { package = \"changed-lib\", version = \"1\" }\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), source).unwrap();
    root
}
