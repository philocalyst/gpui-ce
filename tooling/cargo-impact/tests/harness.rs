use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use cargo_impact::{
    Classification, DiagnosticOrigin, DownstreamSource, DownstreamSpec, HarnessFailure,
    ImpactRequest, analyze, analyze_with_discovery,
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
fn unavailable_standard_library_is_an_environment_failure() {
    let mut fixture = Fixture::new();
    fixture.consumer("embedded", "pub fn api() { changed_lib::removed(); }", DEP);
    // This embedded target has no std even if its core component is installed.
    fixture.request.recipe.target = Some("thumbv7em-none-eabi".into());
    let report = analyze(&fixture.request).unwrap();
    let result = &report.downstreams[0];
    assert_eq!(
        result.classification,
        Classification::HarnessFailure,
        "{result:?}"
    );
    assert_eq!(result.baseline.failure, Some(HarnessFailure::Environment));
    assert!(
        result
            .baseline
            .diagnostics
            .iter()
            .any(|d| d.code.as_ref().is_some_and(|c| c.code == "E0463"))
    );
}

#[test]
fn unrelated_invalid_manifest_fixtures_do_not_break_a_valid_package() {
    let mut fixture = Fixture::new();
    let consumer = fixture.consumer(
        "with-fixtures",
        "pub fn api() { changed_lib::removed(); }",
        DEP,
    );
    fs::create_dir_all(consumer.join("tests/fixtures/invalid")).unwrap();
    fs::write(
        consumer.join("tests/fixtures/invalid/Cargo.toml"),
        "this intentionally isn't TOML!",
    )
    .unwrap();
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::Regression
    );
}

#[test]
fn transitive_local_dependency_is_rewritten_inside_the_workspace_snapshot() {
    let mut fixture = Fixture::new();
    let root = fixture.dir.path().join("transitive");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers=['app','middle']\nresolver='2'",
    )
    .unwrap();
    package(
        &root.join("app"),
        "app",
        "1.0.0",
        "pub fn api() { middle::api(); }",
        "[dependencies]\nmiddle={path='../middle'}",
    );
    package(
        &root.join("middle"),
        "middle",
        "1.0.0",
        "pub fn api() { changed_lib::removed(); }",
        DEP,
    );
    fixture
        .request
        .downstreams
        .push(DownstreamSpec::local("transitive", &root));
    let report = analyze(&fixture.request).unwrap();
    let result = &report.downstreams[0];
    assert_eq!(
        result.classification,
        Classification::Regression,
        "{result:?}"
    );
    assert!(
        result
            .candidate
            .diagnostics
            .iter()
            .any(|d| d.package_id.contains("middle")
                && d.code.as_ref().is_some_and(|c| c.code == "E0425"))
    );
    let diagnostic = result
        .candidate
        .diagnostics
        .iter()
        .find(|d| d.code.as_ref().is_some_and(|c| c.code == "E0425"))
        .unwrap();
    assert_eq!(
        diagnostic.package.as_ref().unwrap().origin,
        DiagnosticOrigin::Downstream
    );
    let primary = diagnostic.spans.iter().find(|s| s.is_primary).unwrap();
    assert_eq!(
        diagnostic
            .source_files
            .get(&primary.file_name)
            .unwrap_or_else(|| panic!("unmapped primary span: {diagnostic:?}")),
        Path::new("middle/src/lib.rs")
    );
    // Render a Git report from the same verified source evidence without fetching a remote.
    let mut report = report;
    report.downstreams[0].source = DownstreamSource::Git {
        url: "https://github.com/example/consumer".into(),
        revision: "main".into(),
        forge: None,
    };
    report.downstreams[0].revision = Some("a".repeat(40));
    assert!(
        report
            .markdown()
            .contains(&format!("/blob/{}/middle/src/lib.rs#L1", "a".repeat(40)))
    );
}

#[test]
fn transitive_git_dependency_is_patched_after_graph_resolution() {
    let mut fixture = Fixture::new();
    let original_library = fixture.dir.path().join("git-library");
    package(
        &original_library,
        "changed-lib",
        "1.0.0",
        "pub fn removed() {}",
        "",
    );
    let library_sha = commit_fixture(&original_library);
    let middle = fixture.dir.path().join("git-middle");
    let library_url = url::Url::from_directory_path(&original_library).unwrap();
    package(
        &middle,
        "middle",
        "1.0.0",
        "pub fn api() { changed_lib::removed(); }",
        &format!("[dependencies]\nchanged-lib={{git='{library_url}',rev='{library_sha}'}}"),
    );
    let middle_sha = commit_fixture(&middle);
    let middle_url = url::Url::from_directory_path(&middle).unwrap();
    fixture.consumer(
        "git-transitive",
        "pub fn api() { middle::api(); }",
        &format!("[dependencies]\nmiddle={{git='{middle_url}',rev='{middle_sha}'}}"),
    );

    let report = analyze(&fixture.request).unwrap();
    let result = &report.downstreams[0];
    assert_eq!(
        result.classification,
        Classification::Regression,
        "{result:?}"
    );
    assert!(
        result
            .baseline
            .selected_library
            .as_ref()
            .unwrap()
            .contains("upstream/baseline")
    );
    assert!(
        result
            .candidate
            .diagnostics
            .iter()
            .any(|d| d.package_id.contains("middle")
                && d.code.as_ref().is_some_and(|c| c.code == "E0425"))
    );
    let diagnostic = result
        .candidate
        .diagnostics
        .iter()
        .find(|d| d.code.as_ref().is_some_and(|c| c.code == "E0425"))
        .unwrap();
    assert_eq!(
        diagnostic.package.as_ref().unwrap().origin,
        DiagnosticOrigin::Dependency
    );
    assert!(
        diagnostic.source_files.is_empty(),
        "dependency sources must not link into the consumer repository"
    );
}

#[test]
fn candidate_library_compilation_error_is_inconclusive_with_provenance() {
    let mut fixture = Fixture::new();
    fixture.consumer("valid", "pub fn api() { changed_lib::kept(); }", DEP);
    fs::write(
        fixture.request.candidate.join("src/lib.rs"),
        "pub fn kept() { missing(); }",
    )
    .unwrap();
    let report = analyze(&fixture.request).unwrap();
    let result = &report.downstreams[0];
    assert!(result.baseline.success);
    assert_eq!(result.classification, Classification::HarnessFailure);
    assert_eq!(
        result.candidate.failure,
        Some(HarnessFailure::LibraryCompilation)
    );
    let error = result
        .candidate
        .diagnostics
        .iter()
        .find(|d| d.code.as_ref().is_some_and(|c| c.code == "E0425"))
        .unwrap();
    assert_eq!(
        error.package.as_ref().unwrap().origin,
        DiagnosticOrigin::Library
    );
    assert!(error.source_files.is_empty());
    assert!(error.target.is_some());
    assert!(
        report
            .markdown()
            .contains("injected library failed to compile")
    );
}

fn commit_fixture(root: &Path) -> String {
    let run = |args: &[&str]| {
        let output = Command::new("git")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    run(&["init", "--quiet"]);
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "fixture"]);
    run(&["rev-parse", "HEAD"])
}

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
