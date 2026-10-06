use std::{fs, path::Path, time::Duration};

use cargo_impact::{
    Classification, DownstreamSource, DownstreamSpec, ImpactRequest, analyze,
};
use tempfile::TempDir;

#[test]
fn classifies_real_downstream_builds_against_both_library_versions() {
    let fixture = TempDir::new().unwrap();
    let baseline = fixture.path().join("upstream-baseline");
    let candidate = fixture.path().join("upstream-candidate");
    library(&baseline, "pub fn removed() -> u8 { 1 }\npub fn kept() -> u8 { 2 }\n");
    library(&candidate, "pub fn kept() -> u8 { 2 }\n");

    let regress = downstream(fixture.path(), "regress", "pub fn use_api() { changed_lib::removed(); }\n");
    let compatible = downstream(fixture.path(), "compatible", "pub fn use_api() { let _ = 1; }\n");
    let preexisting = downstream(fixture.path(), "preexisting", "pub fn use_api() { missing_name(); }\n");
    let broken_metadata = fixture.path().join("invalid-metadata");
    fs::create_dir_all(&broken_metadata).unwrap();
    fs::write(broken_metadata.join("Cargo.toml"), "this is not a manifest").unwrap();

    let report = analyze(&ImpactRequest {
        library: "changed-lib".into(),
        baseline,
        candidate,
        downstreams: vec![
            local("regression", regress),
            local("compatible", compatible),
            local("pre-existing", preexisting),
            local("invalid-metadata", broken_metadata),
        ],
        work_dir: Some(fixture.path().join("work")),
        timeout: Duration::from_secs(240),
    })
    .expect("semver gate and fixture runs should complete");
    assert!(report.gate.ran, "removed public API should open the gate: {:?}", report.gate);
    assert_eq!(report.downstreams[0].classification, Classification::Regression);
    assert!(report.downstreams[0].candidate.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("cannot find function `removed`")
            && diagnostic.spans.iter().any(|span| span.is_primary && !span.text.is_empty())
    }));
    assert_eq!(report.downstreams[1].classification, Classification::Compatible);
    assert_eq!(report.downstreams[2].classification, Classification::PreExistingFailure);
    assert_eq!(report.downstreams[3].classification, Classification::HarnessFailure);
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
        work_dir: None,
        timeout: Duration::from_secs(240),
    })
    .unwrap();
    assert!(report.gate.ran);
    assert_eq!(report.downstreams[0].classification, Classification::Compatible);
}

fn local(name: &str, path: impl Into<std::path::PathBuf>) -> DownstreamSpec {
    DownstreamSpec {
        name: name.into(),
        source: DownstreamSource::Local(path.into()),
    }
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
