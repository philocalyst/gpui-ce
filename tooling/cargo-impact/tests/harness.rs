use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use cargo_impact::{
    Classification, DiagnosticOrigin, DownstreamSource, DownstreamSpec, ExperimentId,
    ExperimentStage, ExperimentStatus, HarnessFailure, ImpactRequest, RunStatus, analyze,
    analyze_with_discovery, analyze_with_progress,
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
        // This is now a budget for the complete pair, not each individual Cargo command.
        request.timeout = Duration::from_secs(60);
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
    assert_eq!(
        report.markdown().matches("error[E0463]").count(),
        1,
        "the human report should not repeat rustc's error in both harness logs"
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
    let graph = result.candidate.dependency_graph.as_ref().unwrap();
    let root = graph
        .packages
        .iter()
        .find(|package| package.name == "app")
        .unwrap();
    let path = graph
        .dependency_path(&root.id, &graph.selected_library)
        .unwrap();
    let names: Vec<_> = path
        .iter()
        .map(|id| {
            graph
                .packages
                .iter()
                .find(|package| &package.id == id)
                .unwrap()
                .name
                .as_str()
        })
        .collect();
    assert_eq!(names, ["app", "middle", "changed-lib"]);
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

#[test]
fn generated_out_dir_error_is_a_regression_without_a_consumer_source_mapping() {
    let mut fixture = Fixture::new();
    let consumer = fixture.consumer(
        "generated-source",
        "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));",
        DEP,
    );
    fs::write(
        consumer.join("build.rs"),
        r#"
fn main() {
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(
        output.join("generated.rs"),
        "pub fn api() { changed_lib::removed(); }\n",
    )
    .unwrap();
}
"#,
    )
    .unwrap();

    let report = analyze(&fixture.request).unwrap();
    let result = &report.downstreams[0];
    assert_eq!(
        result.classification,
        Classification::Regression,
        "{result:?}"
    );
    let diagnostic = result
        .candidate
        .diagnostics
        .iter()
        .find(|d| d.code.as_ref().is_some_and(|c| c.code == "E0425"))
        .expect("generated consumer code should report the removed API");
    assert_eq!(
        diagnostic.package.as_ref().unwrap().origin,
        DiagnosticOrigin::Downstream
    );
    let primary = diagnostic
        .spans
        .iter()
        .find(|span| span.is_primary)
        .unwrap();
    assert!(primary.file_name.ends_with("generated.rs"), "{primary:?}");
    assert!(
        !diagnostic.source_files.contains_key(&primary.file_name),
        "generated OUT_DIR files must not map to consumer checkout sources"
    );
}

#[test]
fn exported_macro_diagnostic_keeps_consumer_callsite_and_library_definition_provenance() {
    let mut fixture = Fixture::new();
    let macro_source = "#[macro_export]\nmacro_rules! call_removed { () => { $crate::removed() }; }\npub fn removed() {}\npub fn kept() {}\n";
    fs::write(fixture.request.baseline.join("src/lib.rs"), macro_source).unwrap();
    fs::write(
        fixture.request.candidate.join("src/lib.rs"),
        "#[macro_export]\nmacro_rules! call_removed { () => { $crate::removed() }; }\npub fn kept() {}\n",
    )
    .unwrap();
    fixture.consumer(
        "macro-consumer",
        "pub fn api() { changed_lib::call_removed!(); }\n",
        DEP,
    );

    let mut report = analyze(&fixture.request).unwrap();
    let (primary_source, primary_line) = {
        let result = &report.downstreams[0];
        assert_eq!(
            result.classification,
            Classification::Regression,
            "{result:?}"
        );
        let diagnostic = result
            .candidate
            .diagnostics
            .iter()
            .find(|d| d.code.as_ref().is_some_and(|c| c.code == "E0425"))
            .expect("the expanded macro should report its removed API");
        assert_eq!(
            diagnostic.package.as_ref().unwrap().origin,
            DiagnosticOrigin::Downstream
        );
        assert_eq!(diagnostic.target.as_ref().unwrap().name, "consumer");
        let expansion = diagnostic
            .spans
            .iter()
            .find_map(|span| span.expansion.as_ref())
            .expect("rustc macro expansion provenance should be retained");
        assert_eq!(
            diagnostic.source_files.get(&expansion.span.file_name),
            Some(&PathBuf::from("src/lib.rs")),
            "the invocation site should map to the original consumer source"
        );
        let definition = expansion
            .def_site_span
            .as_ref()
            .expect("exported macro provenance should include its definition site");
        assert!(
            !diagnostic.source_files.contains_key(&definition.file_name),
            "upstream macro definitions must not link into the consumer repository"
        );
        let primary = diagnostic
            .spans
            .iter()
            .find(|span| span.is_primary)
            .unwrap();
        (
            diagnostic.source_files.get(&primary.file_name).cloned(),
            primary.line_start,
        )
    };

    let sha = "a".repeat(40);
    report.downstreams[0].source = DownstreamSource::Git {
        url: "https://github.com/example/consumer".into(),
        revision: "main".into(),
        forge: None,
    };
    report.downstreams[0].revision = Some(sha.clone());
    let markdown = report.markdown();
    if let Some(path) = primary_source {
        assert!(markdown.contains(&format!("/blob/{sha}/{}#L{}", path.display(), primary_line)));
    } else {
        assert!(markdown.contains("Macro invocation:"));
        assert!(
            markdown.contains(&format!("/blob/{sha}/src/lib.rs#L1")),
            "the verified consumer invocation should be linked"
        );
    }
}

#[cfg(unix)]
#[test]
fn dangling_unrelated_license_link_is_preserved_but_escaping_links_fail_with_context() {
    let mut fixture = Fixture::new();
    let consumer = fixture.consumer(
        "dangling-license",
        "pub fn api() { changed_lib::removed(); }",
        DEP,
    );
    let license = consumer.join("tooling/perf/LICENSE-APACHE");
    fs::create_dir_all(license.parent().unwrap()).unwrap();
    let safe_target = PathBuf::from("../../LICENSE-APACHE");
    std::os::unix::fs::symlink(&safe_target, &license).unwrap();

    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::Regression,
        "an unrelated safe dangling license link must not hide the API break"
    );
    assert_eq!(fs::read_link(&license).unwrap(), safe_target);
    let snapshots = fs::read_dir(
        fixture
            .request
            .work_dir
            .as_ref()
            .unwrap()
            .join("downstreams"),
    )
    .unwrap()
    .map(|entry| entry.unwrap().path())
    .collect::<Vec<_>>();
    assert_eq!(snapshots.len(), 1);
    let snapshot_link = snapshots[0].join("tooling/perf/LICENSE-APACHE");
    assert!(
        fs::symlink_metadata(&snapshot_link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(&snapshot_link).unwrap(), safe_target);

    for (target, reason) in [
        (
            PathBuf::from("../../../../outside-missing-license"),
            "escapes source tree",
        ),
        (
            PathBuf::from("/outside-missing-license"),
            "relative target inside the source tree",
        ),
    ] {
        fs::remove_file(&license).unwrap();
        std::os::unix::fs::symlink(&target, &license).unwrap();
        let report = analyze(&fixture.request).unwrap();
        let result = &report.downstreams[0];
        assert_eq!(
            result.classification,
            Classification::HarnessFailure,
            "unsafe dangling link {target:?} must fail snapshot preparation"
        );
        let message = result.message.as_deref().unwrap_or_default();
        assert!(message.contains("tooling/perf/LICENSE-APACHE"), "{message}");
        assert!(message.contains(reason), "{message}");
        assert_eq!(fs::read_link(&license).unwrap(), target);
    }
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
fn same_name_experiments_keep_distinct_recipes_and_deduplicate_exact_identity() {
    let mut fixture = Fixture::new();
    fixture.consumer("same", "#[cfg(feature=\"api\")] pub fn api() { changed_lib::removed(); }", "[features]\napi=['dep:changed-lib']\nextra=[]\n[dependencies]\nchanged-lib={version='1',optional=true}");
    let original = fixture.request.downstreams[0].clone();
    let mut exercised = original.clone();
    exercised.recipe = Some(BuildRecipe {
        features: vec!["api".into()],
        ..fixture.request.recipe.clone()
    });
    fixture.request.downstreams.extend([exercised, original]);
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(report.downstreams.len(), 2, "{report:?}");
    assert_eq!(
        report.downstreams[0].classification,
        Classification::NotExercised
    );
    assert_eq!(
        report.downstreams[1].classification,
        Classification::Regression
    );
    assert_ne!(
        report.downstreams[0].experiment_id,
        report.downstreams[1].experiment_id
    );
    assert!(
        report
            .downstreams
            .iter()
            .all(|result| result.lifecycle.status == ExperimentStatus::Complete)
    );
    assert!(
        report
            .discovery
            .notes
            .iter()
            .any(|note| note.contains("Duplicate experiment"))
    );
    let mut recipe = fixture.request.recipe.clone();
    recipe.features = vec!["api".into(), "extra".into()];
    let id =
        ExperimentId::for_spec("changed-lib", &fixture.request.downstreams[0], &recipe).unwrap();
    recipe.features = vec!["extra".into(), "api".into(), "api".into()];
    assert_eq!(
        id,
        ExperimentId::for_spec("changed-lib", &fixture.request.downstreams[0], &recipe).unwrap()
    );
    assert!(serde_json::from_str::<ExperimentId>("\"../../outside\"").is_err());
}

#[test]
fn phase_evidence_is_durable_before_candidate_and_observer_failure_stops_the_pair() {
    let mut fixture = Fixture::new();
    let consumer = fixture.consumer("phase-ack", "pub fn api() { changed_lib::kept(); }", DEP);
    let marker = fixture.dir.path().join("candidate-started");
    fs::write(consumer.join("build.rs"),format!("fn main() {{ if std::fs::read_to_string(\"Cargo.toml\").unwrap().contains(\"upstream/candidate\") {{ std::fs::write({:?},\"started\").unwrap(); }} }}",marker)).unwrap();
    let checkpoint = fixture.dir.path().join("baseline-checkpoint.json");
    let error = analyze_with_progress(
        &fixture.request,
        || Ok((Vec::new(), Discovery::default())),
        |report| {
            if report
                .downstreams
                .first()
                .is_some_and(|result| result.baseline.success)
            {
                assert!(
                    !marker.exists(),
                    "candidate ran before baseline evidence was acknowledged"
                );
                fs::write(&checkpoint, serde_json::to_vec(report).unwrap())?;
                return Err(std::io::Error::other("baseline checkpoint refused"));
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("baseline checkpoint refused"),
        "{error}"
    );
    assert!(!marker.exists());
    let saved: cargo_impact::ImpactReport =
        serde_json::from_slice(&fs::read(checkpoint).unwrap()).unwrap();
    let result = &saved.downstreams[0];
    assert_eq!(result.lifecycle.status, ExperimentStatus::Baseline);
    assert!(result.baseline.lockfile.as_ref().unwrap().verify());
    assert_eq!(result.candidate.selected_library, None);
    assert_eq!(saved.run.status, RunStatus::Running);
}

#[test]
fn preparation_failures_keep_their_stage_and_never_invent_candidate_failures() {
    let mut fixture = Fixture::new();
    fixture.request.downstreams.push(DownstreamSpec::local(
        "missing-source",
        fixture.dir.path().join("absent"),
    ));
    fixture.consumer("working", "pub fn api() { changed_lib::kept(); }", DEP);
    let report = analyze(&fixture.request).unwrap();
    let result = &report.downstreams[0];
    assert_eq!(result.lifecycle.status, ExperimentStatus::Complete);
    assert_eq!(
        result.lifecycle.failure.as_ref().unwrap().stage,
        ExperimentStage::SourcePreparation
    );
    assert_eq!(result.baseline.failure, None);
    assert_eq!(result.candidate.failure, None);
    assert_eq!(
        report.downstreams[1].classification,
        Classification::Compatible
    );
}

#[test]
fn retained_locks_and_source_relative_graph_proof_survive_a_new_work_directory() {
    let mut fixture = Fixture::new();
    fixture.consumer("portable", "pub fn api() { changed_lib::removed(); }", DEP);
    let first = analyze(&fixture.request).unwrap();
    fixture.request.work_dir = Some(fixture.dir.path().join("second-work"));
    let second_report = analyze(&fixture.request).unwrap();
    let first = &first.downstreams[0];
    let second = &second_report.downstreams[0];
    assert_eq!(first.experiment_id, second.experiment_id);
    assert_ne!(
        first.baseline.selected_library,
        second.baseline.selected_library
    );
    for (first, second) in [
        (&first.baseline, &second.baseline),
        (&first.candidate, &second.candidate),
    ] {
        let lock = second.lockfile.as_ref().unwrap();
        assert!(lock.verify());
        assert_eq!(
            Some(&lock.sha256),
            second.provenance.lock_fingerprint.as_ref()
        );
        assert_eq!(first.lockfile.as_ref().unwrap().sha256, lock.sha256);
        assert_eq!(
            first
                .dependency_graph
                .as_ref()
                .unwrap()
                .fingerprint()
                .unwrap(),
            second
                .dependency_graph
                .as_ref()
                .unwrap()
                .fingerprint()
                .unwrap()
        );
    }
    assert!(matches!(
        cargo_impact::replay::ReplayIdentity::from_report(&second_report, second),
        Err(cargo_impact::replay::ReplayIneligible::UnsupportedRunner)
    ));
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
    let result = &report.downstreams[0];
    assert!(result.baseline.provenance.lock_fingerprint.is_some());
    let snapshot = fs::read_dir(
        fixture
            .request
            .work_dir
            .as_ref()
            .unwrap()
            .join("downstreams"),
    )
    .unwrap()
    .next()
    .unwrap()
    .unwrap()
    .path();
    assert!(
        !snapshot.join("app/Cargo.lock").exists(),
        "this fixture must exercise a workspace-root lock"
    );
    use sha2::Digest;
    let lock = fs::read(snapshot.join("Cargo.lock")).unwrap();
    assert_eq!(
        result.candidate.provenance.lock_fingerprint.as_deref(),
        Some(format!("{:x}", sha2::Sha256::digest(lock)).as_str())
    );
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
        Classification::Compatible,
        "{report:?}"
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

#[test]
fn bounded_workers_overlap_consumers_keep_pairs_sequential_and_checkpoint_in_order() {
    let mut fixture = Fixture::new();
    fixture.request.timeout = Duration::from_secs(40);
    fixture.request.execution.jobs = 2;
    let barrier = fixture.dir.path().join("barrier");
    fs::create_dir(&barrier).unwrap();
    for (name, other) in [("left", "right"), ("right", "left")] {
        let consumer = fixture.consumer(name, "pub fn api() { changed_lib::kept(); }", DEP);
        fs::write(
            consumer.join("build.rs"),
            format!(
                r#"
fn main() {{
    let barrier = std::path::Path::new({barrier:?});
    let manifest = std::fs::read_to_string("Cargo.toml").unwrap();
    let baseline = manifest.contains("upstream/baseline");
    if baseline {{
        std::fs::write(barrier.join("{name}-started"), "").unwrap();
        let start = std::time::Instant::now();
        while !barrier.join("{other}-started").exists() {{
            assert!(start.elapsed().as_secs() < 10, "consumers were serialized");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }}
        std::fs::write(barrier.join("{name}-baseline-done"), "").unwrap();
    }} else {{
        assert!(barrier.join("{name}-baseline-done").exists(), "candidate overlapped its baseline");
        std::fs::write(barrier.join("{name}-candidate-done"), "").unwrap();
    }}
}}
"#,
                barrier = barrier.to_str().unwrap()
            ),
        )
        .unwrap();
    }
    let mut checkpoints = Vec::new();
    let report = analyze_with_progress(
        &fixture.request,
        || Ok((Vec::new(), Discovery::default())),
        |report| {
            checkpoints.push((
                report.run.status,
                report.run.completed_downstreams,
                report
                    .downstreams
                    .iter()
                    .map(|result| result.name.clone())
                    .collect::<Vec<_>>(),
            ));
            Ok(())
        },
    )
    .unwrap();
    assert!(
        report
            .downstreams
            .iter()
            .all(|result| result.classification == Classification::Compatible),
        "{report:?}"
    );
    assert_eq!(report.run.planned_downstreams, 2);
    assert_eq!(report.run.completed_downstreams, 2);
    assert_eq!(report.run.status, RunStatus::Complete);
    assert!(
        checkpoints
            .iter()
            .any(|(status, completed, _)| *status == RunStatus::Running && *completed == 1)
    );
    assert_eq!(checkpoints.last().unwrap().2, ["left", "right"]);
    assert!(barrier.join("left-candidate-done").exists());
    assert!(barrier.join("right-candidate-done").exists());
    let provenance = &report.downstreams[0].candidate.provenance;
    assert!(provenance.rustc.as_ref().unwrap().contains("rustc "));
    assert!(provenance.cargo.as_ref().unwrap().contains("cargo "));
    assert!(provenance.lock_fingerprint.is_some());
    assert!(report.downstreams[0].source_fingerprint.is_some());
}

#[test]
fn observer_failure_fails_scan_and_storage_pruning_preserves_non_cache_data() {
    let mut fixture = Fixture::new();
    fixture.consumer("consumer", "pub fn api() { changed_lib::kept(); }", DEP);
    fixture.request.timeout = Duration::from_secs(40);
    let error = analyze_with_progress(
        &fixture.request,
        || Ok((Vec::new(), Discovery::default())),
        |_| Err(std::io::Error::other("checkpoint disk failed")),
    )
    .unwrap_err();
    assert!(error.to_string().contains("checkpoint disk failed"));
    analyze(&fixture.request).unwrap();
    let root = fixture.request.work_dir.as_ref().unwrap();
    let sentinel = root.join("targets/obsolete/sentinel");
    fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    fs::write(&sentinel, "old cache").unwrap();
    let note = root.join("user-notes.txt");
    fs::write(&note, "keep this").unwrap();
    fixture.request.execution.prune_before_run = true;
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::Compatible
    );
    assert!(!sentinel.exists());
    assert_eq!(fs::read_to_string(note).unwrap(), "keep this");
}

#[test]
fn growing_build_storage_is_killed_and_inconclusive() {
    let mut fixture = Fixture::new();
    let consumer = fixture.consumer("disk-hog", "pub fn api() { changed_lib::kept(); }", DEP);
    fs::write(consumer.join("build.rs"), r#"
fn main() {
    let file = std::fs::File::create(std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("large" )).unwrap();
    file.set_len(128 * 1024 * 1024).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(30));
}
"#).unwrap();
    fixture.request.execution.max_work_bytes = Some(64 * 1024 * 1024);
    fixture.request.timeout = Duration::from_secs(15);
    let started = std::time::Instant::now();
    let report = analyze(&fixture.request).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(12),
        "storage watchdog did not stop build promptly"
    );
    assert_eq!(
        report.downstreams[0].classification,
        Classification::HarnessFailure
    );
    assert_eq!(
        report.downstreams[0].baseline.failure,
        Some(HarnessFailure::StorageLimit)
    );
    assert!(report.run.storage_bytes > fixture.request.execution.max_work_bytes.unwrap());
}

#[test]
fn host_source_mutation_cannot_be_reported_as_compatible() {
    let mut fixture = Fixture::new();
    let consumer = fixture.consumer(
        "mutates-upstream",
        "pub fn api() { changed_lib::removed(); }",
        DEP,
    );
    let candidate = fixture
        .request
        .work_dir
        .as_ref()
        .unwrap()
        .join("upstream/candidate/src/lib.rs");
    fs::write(consumer.join("build.rs"), format!("fn main() {{ std::fs::write({:?}, \"pub fn removed() {{}}\\npub fn kept() {{}}\").unwrap(); }}", candidate.to_str().unwrap())).unwrap();
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::HarnessFailure
    );
    assert_eq!(
        report.downstreams[0]
            .lifecycle
            .failure
            .as_ref()
            .map(|failure| failure.cause),
        Some(HarnessFailure::InputMutation)
    );
    assert_eq!(
        report.downstreams[0]
            .lifecycle
            .failure
            .as_ref()
            .unwrap()
            .stage,
        ExperimentStage::IntegrityCheck
    );
    assert!(!report.has_regressions());
}

#[test]
fn consumer_source_mutation_is_not_a_controlled_comparison() {
    let mut fixture = Fixture::new();
    let consumer = fixture.consumer(
        "self-mutating",
        "pub fn api() { changed_lib::kept(); }",
        DEP,
    );
    fs::write(
        consumer.join("build.rs"),
        "fn main() { std::fs::write(\"src/lib.rs\", \"pub fn api() {}\").unwrap(); }",
    )
    .unwrap();
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::HarnessFailure
    );
    assert_eq!(
        report.downstreams[0].baseline.failure,
        Some(HarnessFailure::InputMutation)
    );
    assert_eq!(report.run.coverage_sufficient, Some(false));
    assert_eq!(report.run.exercised_downstreams, 0);
}

#[cfg(unix)]
#[test]
fn source_cargo_configuration_cannot_replace_the_recorded_compiler_or_rustdoc() {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    fixture.request.force = false;
    let consumer = fixture.consumer(
        "compiler-config",
        "pub fn api() { changed_lib::removed(); }",
        DEP,
    );
    let replacement = fixture.dir.path().join("replace-compiler");
    let marker = fixture.dir.path().join("replacement-executed");
    fs::write(
        &replacement,
        format!("#!/bin/sh\ntouch {:?}\nexit 1\n", marker),
    )
    .unwrap();
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755)).unwrap();
    for force in [true, false] {
        let env_value = |value: &str| {
            if force {
                format!("{{value={value:?},force=true}}")
            } else {
                format!("{value:?}")
            }
        };
        let config = format!(
            r#"
[build]
rustc={replacement:?}
rustdoc={replacement:?}
rustc-wrapper={replacement:?}
rustc-workspace-wrapper={replacement:?}
[env]
RUSTC={compiler}
RUSTDOC={compiler}
RUSTC_WRAPPER={compiler}
RUSTC_WORKSPACE_WRAPPER={compiler}
PATH={path}
"#,
            replacement = replacement.to_str().unwrap(),
            compiler = env_value(replacement.to_str().unwrap()),
            path = env_value("/nonexistent-command-directory"),
        );
        for source in [
            &fixture.request.baseline,
            &fixture.request.candidate,
            &consumer,
        ] {
            fs::create_dir_all(source.join(".cargo")).unwrap();
            fs::write(source.join(".cargo/config.toml"), &config).unwrap();
        }
        let report = analyze(&fixture.request).unwrap();
        assert!(report.gate.ran, "{report:?}");
        assert_eq!(
            report.downstreams[0].classification,
            Classification::Regression,
            "{report:?}"
        );
        assert!(
            report.downstreams[0]
                .baseline
                .provenance
                .rustc
                .as_ref()
                .unwrap()
                .starts_with("rustc ")
        );
        assert!(!marker.exists(), "project replacement compiler was invoked");
    }
}

#[cfg(unix)]
#[test]
fn persisted_snapshot_symlinks_cannot_redirect_host_writes() {
    let fixture = Fixture::new();
    let outside = tempfile::TempDir::new().unwrap();
    let root = fixture.request.work_dir.as_ref().unwrap();
    fs::create_dir_all(root).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("upstream")).unwrap();
    let error = analyze(&fixture.request).unwrap_err().to_string();
    assert!(error.contains("symlinks"), "{error}");
    assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[test]
fn gate_source_mutation_cannot_hide_a_breaking_api_change() {
    let mut fixture = Fixture::new();
    fixture.request.force = false;
    fs::write(
        fixture.request.baseline.join("build.rs"),
        "fn main() { std::fs::write(\"src/lib.rs\", \"pub fn kept() {}\").unwrap(); }",
    )
    .unwrap();
    let error = analyze(&fixture.request).unwrap_err().to_string();
    assert!(
        error.contains("changed its upstream source copy"),
        "{error}"
    );
}

#[test]
fn global_deadline_stops_running_pairs_and_preserves_queued_coverage() {
    let mut fixture = Fixture::new();
    fixture.request.execution.jobs = 1;
    fixture.request.execution.scan_timeout_seconds = 3;
    let consumer = fixture.consumer("slow", "pub fn api() { changed_lib::kept(); }", DEP);
    fs::write(
        consumer.join("build.rs"),
        "fn main() { std::thread::sleep(std::time::Duration::from_secs(30)); }",
    )
    .unwrap();
    fixture.consumer("queued-one", "pub fn api() { changed_lib::kept(); }", DEP);
    fixture.consumer("queued-two", "pub fn api() { changed_lib::kept(); }", DEP);
    let started = std::time::Instant::now();
    let report = analyze(&fixture.request).unwrap();
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(
        report
            .error
            .as_deref()
            .unwrap()
            .contains("wall-clock budget")
    );
    assert_eq!(report.run.status, RunStatus::Complete);
    assert_eq!(report.run.planned_downstreams, 3);
    assert_eq!(report.run.completed_downstreams, 1);
    assert_eq!(report.run.coverage_sufficient, Some(false));
    for result in report.downstreams.iter().skip(1) {
        assert_eq!(result.lifecycle.status, ExperimentStatus::Cancelled);
        assert_eq!(
            result.lifecycle.failure.as_ref().unwrap().cause,
            HarnessFailure::Timeout
        );
        assert_eq!(
            result.lifecycle.failure.as_ref().unwrap().stage,
            ExperimentStage::Scheduling
        );
        assert_eq!(result.baseline.failure, None);
        assert_eq!(result.candidate.failure, None);
        assert!(result.message.as_deref().unwrap().contains("not started"));
    }
}

#[test]
#[ignore = "requires a Docker daemon and the Rust worker image; run in isolated Linux CI"]
fn docker_baseline_cannot_mutate_candidate_or_sibling_workers() {
    let mut fixture = Fixture::new();
    fixture.request.recipe.runner = Runner::default();
    fixture.request.timeout = Duration::from_secs(180);
    let consumer = fixture.consumer("malicious", "pub fn api() { changed_lib::removed(); }", DEP);
    let root = fixture.request.work_dir.as_ref().unwrap().to_owned();
    fs::create_dir_all(root.join("workers/sibling")).unwrap();
    fs::write(root.join("workers/sibling/secret"), "another consumer").unwrap();
    fs::write(consumer.join("build.rs"), format!(r#"
fn main() {{
    let root = std::path::Path::new({root:?});
    assert!(std::fs::write(root.join("upstream/candidate/src/lib.rs"), "pub fn removed() {{}}" ).is_err(), "candidate was writable");
    assert!(std::fs::write(root.join("upstream/baseline/src/lib.rs"), "pub fn removed() {{}}" ).is_err(), "baseline was writable");
    assert!(!root.join("checkouts").exists(), "sibling checkouts were exposed");
    assert!(!root.join("workers/sibling").exists(), "sibling workers were exposed");
    assert!(std::fs::write("src/lib.rs", "pub fn api() {{}}" ).is_err(), "consumer sources were writable during compilation");
}}
"#, root = root.to_str().unwrap())).unwrap();
    let report = analyze(&fixture.request).unwrap();
    assert_eq!(
        report.downstreams[0].classification,
        Classification::Regression,
        "{report:?}"
    );
    let proof = cargo_impact::replay::ReplayIdentity::from_report(&report, &report.downstreams[0])
        .expect("real isolated comparison must retain a complete replay identity");
    assert_eq!(proof.baseline.image, proof.candidate.image);
    assert!(proof.baseline.image.starts_with("sha256:"));
    assert_eq!(
        fs::read_to_string(root.join("upstream/candidate/src/lib.rs")).unwrap(),
        "pub fn kept() {}"
    );
}
