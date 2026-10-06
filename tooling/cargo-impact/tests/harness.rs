use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use cargo_impact::{
    Classification, DownstreamSpec, HarnessFailure, ImpactRequest, analyze, analyze_with_discovery,
    discovery::Discovery,
    runner::{BuildRecipe, Runner},
};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    request: ImpactRequest,
}
impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let baseline = dir.path().join("base");
        let candidate = dir.path().join("candidate");
        package(
            &baseline,
            "changed-lib",
            "1.0.0",
            "pub fn removed() {}\npub fn kept() {}",
            "",
        );
        package(&candidate, "changed-lib", "1.0.0", "pub fn kept() {}", "");
        let mut request = ImpactRequest::new("changed-lib", baseline, candidate);
        request.force = true;
        request.recipe.runner = Runner::Local;
        request.work_dir = Some(dir.path().join("work"));
        request.timeout = Duration::from_secs(20);
        Self { dir, request }
    }
    fn consumer(&mut self, name: &str, source: &str, dependencies: &str) -> PathBuf {
        let root = self.dir.path().join(name);
        package(&root, "consumer", "1.0.0", source, dependencies);
        self.request
            .downstreams
            .push(DownstreamSpec::local(name, &root));
        root
    }
}

fn package(root: &Path, name: &str, version: &str, source: &str, extra: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname={name:?}\nversion={version:?}\nedition='2021'\n{extra}\n"),
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), source).unwrap();
}
const DEP: &str = "[dependencies]\nchanged-lib='1'";

#[test]
fn closed_gate_skips_discovery_and_invalid_downstreams() {
    let mut fixture = Fixture::new();
    fixture.request.force = false;
    fs::write(
        fixture.request.candidate.join("src/lib.rs"),
        "pub fn removed() {}\npub fn kept() {}\npub fn added() {}",
    )
    .unwrap();
    fixture.request.downstreams.push(DownstreamSpec::local(
        "does-not-exist",
        "/invalid/no-source",
    ));
    let report =
        analyze_with_discovery(&fixture.request, || panic!("discovery must be lazy")).unwrap();
    assert!(!report.gate.ran);
    assert!(report.downstreams.is_empty());
}

#[test]
fn disabled_optional_dependency_is_not_claimed_as_compatible() {
    let mut fixture = Fixture::new();
    fixture.consumer(
        "optional",
        "pub fn harmless() {}",
        "[dependencies]\nchanged-lib={version='1',optional=true}",
    );
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::NotExercised
    );
}

#[test]
fn explicit_feature_exercises_optional_dependency_and_finds_error() {
    let mut fixture = Fixture::new();
    fixture.consumer("optional", "pub fn api() { changed_lib::removed(); }", "[features]\napi=['dep:changed-lib']\n[dependencies]\nchanged-lib={version='1',optional=true}");
    fixture.request.recipe.features = vec!["api".into()];
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::Regression
    );
}

#[test]
fn workspace_inheritance_renaming_and_target_dependencies_are_preserved() {
    let mut fixture = Fixture::new();
    let root = fixture.dir.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\nmembers=['app']\nresolver='2'\n[workspace.dependencies]\nrenamed={package='changed-lib',version='1',default-features=false}\n").unwrap();
    package(
        &root.join("app"),
        "consumer",
        "1.0.0",
        "pub fn api() { renamed::removed(); }",
        "[target.'cfg(any(unix,windows))'.dependencies]\nrenamed={workspace=true}",
    );
    let mut spec = DownstreamSpec::local("workspace", &root);
    spec.manifest = "app/Cargo.toml".into();
    fixture.request.downstreams.push(spec);
    let original = fs::read(root.join("Cargo.toml")).unwrap();
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::Regression,
        "{:?}",
        report.downstreams[0]
    );
    assert_eq!(fs::read(root.join("Cargo.toml")).unwrap(), original);
}

#[test]
fn preexisting_failure_remains_inconclusive_even_with_a_new_candidate_error() {
    let mut fixture = Fixture::new();
    fixture.consumer(
        "already-broken",
        "pub fn api() { unknown(); changed_lib::removed(); }",
        DEP,
    );
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::PreExistingFailure
    );
    assert!(!report.has_regressions());
}

#[test]
fn build_script_failure_is_a_harness_failure_and_does_not_stop_other_consumers() {
    let mut fixture = Fixture::new();
    let broken = fixture.consumer(
        "missing-system-dependency",
        "pub fn api() { changed_lib::kept(); }",
        DEP,
    );
    fs::write(
        broken.join("build.rs"),
        "fn main() { panic!(\"system library unavailable\"); }",
    )
    .unwrap();
    fixture.consumer("healthy", "pub fn api() { changed_lib::kept(); }", DEP);
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::HarnessFailure
    );
    assert_eq!(
        report.downstreams[0].baseline.failure,
        Some(HarnessFailure::Environment)
    );
    assert_eq!(
        report.downstreams[1].classification,
        Classification::Compatible
    );
}

#[test]
fn retry_recipe_changes_are_selected_and_input_trees_stay_untouched() {
    let mut fixture = Fixture::new();
    let consumer = fixture.consumer("needs-feature", "#[cfg(feature=\"api\")] pub fn api() { changed_lib::removed(); }", "[features]\napi=['dep:changed-lib']\n[dependencies]\nchanged-lib={version='1',optional=true}");
    let manifest = fs::read(consumer.join("Cargo.toml")).unwrap();
    let first = analyze(&fixture.request).unwrap();
    assert_eq!(
        first.downstreams[0].classification,
        Classification::NotExercised
    );
    fixture.request.downstreams[0].recipe = Some(BuildRecipe {
        runner: Runner::Local,
        features: vec!["api".into()],
        ..Default::default()
    });
    let second = analyze(&fixture.request).unwrap();
    assert_eq!(
        second.downstreams[0].classification,
        Classification::Regression
    );
    assert_eq!(fs::read(consumer.join("Cargo.toml")).unwrap(), manifest);
    assert!(!consumer.join("Cargo.lock").exists());
}

#[test]
fn repeated_run_drops_deleted_sources_and_reuses_the_managed_target_location() {
    let mut fixture = Fixture::new();
    fixture.consumer("consumer", "pub fn api() { changed_lib::removed(); }", DEP);
    let first = analyze(&fixture.request).unwrap();
    assert_eq!(
        first.downstreams[0].classification,
        Classification::Regression
    );
    let target = fs::read_dir(fixture.request.work_dir.as_ref().unwrap().join("targets"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(
        fixture.request.candidate.join("src/lib.rs"),
        "pub fn removed() {}\npub fn kept() {}",
    )
    .unwrap();
    let second = analyze(&fixture.request).unwrap();
    assert_eq!(
        second.downstreams[0].classification,
        Classification::Compatible
    );
    assert_ne!(first.candidate_fingerprint, second.candidate_fingerprint);
    assert!(target.is_dir());
    assert_eq!(
        fs::read_dir(fixture.request.work_dir.as_ref().unwrap().join("targets"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn version_bump_does_not_hide_api_breaks_or_prevent_consumer_injection() {
    let mut fixture = Fixture::new();
    fixture.request.force = false;
    package(
        &fixture.request.candidate,
        "changed-lib",
        "2.0.0",
        "pub fn kept() {}",
        "",
    );
    fixture.consumer(
        "old-version-requirement",
        "pub fn api() { changed_lib::removed(); }",
        DEP,
    );
    let original = fs::read(fixture.request.candidate.join("Cargo.toml")).unwrap();
    let report = analyze(&fixture.request).unwrap();
    assert!(report.gate.ran);
    assert_eq!(
        report.downstreams[0].classification,
        Classification::Regression
    );
    assert_eq!(
        fs::read(fixture.request.candidate.join("Cargo.toml")).unwrap(),
        original
    );
    assert!(
        report
            .discovery
            .notes
            .iter()
            .any(|v| v.contains("injected as baseline version"))
    );
}

#[test]
fn discovery_failure_keeps_explicit_consumers_and_coverage_evidence() {
    let mut fixture = Fixture::new();
    fixture.consumer("consumer", "pub fn api() { changed_lib::kept(); }", DEP);
    let report = analyze_with_discovery(&fixture.request, || Err("rate limited".into())).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::Compatible
    );
    assert!(
        report
            .discovery
            .notes
            .iter()
            .any(|n| n.contains("rate limited"))
    );
    assert!(
        serde_json::to_value(&report)
            .unwrap()
            .get("schema_version")
            .is_some()
    );
    let _ = Discovery::default();
}

#[test]
fn timeout_is_classified_and_descendants_do_not_hold_scan_open() {
    let mut fixture = Fixture::new();
    let consumer = fixture.consumer("timeout", "pub fn api() { changed_lib::kept(); }", DEP);
    fs::write(
        consumer.join("build.rs"),
        "fn main() { std::thread::sleep(std::time::Duration::from_secs(20)); }",
    )
    .unwrap();
    fixture.request.timeout = Duration::from_secs(2);
    let started = std::time::Instant::now();
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::HarnessFailure
    );
    assert_eq!(
        report.downstreams[0].baseline.failure,
        Some(HarnessFailure::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(12));
}
