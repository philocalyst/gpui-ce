use std::{fs, path::Path, process::Command};

use assert_cmd::{Command as AssertCommand, cargo::cargo_bin};
use predicates::str::contains;
use serde_json::Value;
use tempfile::TempDir;

fn cli() -> AssertCommand {
    AssertCommand::new(cargo_bin!("cargo-impact"))
}

#[test]
fn init_vendors_a_standalone_scanner_manifest() {
    let directory = TempDir::new().unwrap();
    fs::write(
        directory.path().join(".gitignore"),
        "user-owned-pattern\n.cargo-impact/",
    )
    .unwrap();
    cli()
        .args(["init", "--library", "probe-lib", "--directory"])
        .arg(directory.path())
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(directory.path().join(".gitignore")).unwrap(),
        "user-owned-pattern\n.cargo-impact/\nimpact-report/\n"
    );

    let scanner = directory.path().join(".github/cargo-impact");
    let metadata = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version=1",
            "--manifest-path",
        ])
        .arg(scanner.join("Cargo.toml"))
        .current_dir(directory.path())
        .output()
        .expect("cargo metadata should start");
    assert!(
        metadata.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: Value = serde_json::from_slice(&metadata.stdout).unwrap();
    assert_eq!(
        metadata["workspace_root"].as_str(),
        Some(scanner.to_str().unwrap()),
        "the vendored scanner must be its own nested workspace"
    );
    assert!(
        metadata["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|package| package["name"] == "cargo-impact")
    );
    assert!(scanner.join("src/setup.rs").is_file());
    assert!(scanner.join("examples/impact.yml").is_file());
    assert!(scanner.join("action.yml").is_file());
    for asset in [
        "build.rs",
        "src/engine/scheduler.rs",
        "src/report/bundle/verify.rs",
        "src/cli/check.rs",
    ] {
        assert!(
            scanner.join(asset).is_file(),
            "missing standalone asset {asset}"
        );
    }
    let impact_workflow =
        fs::read_to_string(directory.path().join(".github/workflows/impact.yml")).unwrap();
    assert!(impact_workflow.contains("uses: ./baseline/.github/cargo-impact"));
    let comment_workflow = fs::read_to_string(
        directory
            .path()
            .join(".github/workflows/impact-comment.yml"),
    )
    .unwrap();
    assert!(comment_workflow.contains("uses: ./.github/cargo-impact"));
}

#[test]
fn init_preflights_all_destinations_before_writing_anything() {
    for existing in [
        "impact.toml",
        ".github/cargo-impact/examples/impact-comment.yml",
    ] {
        let directory = TempDir::new().unwrap();
        let existing_path = directory.path().join(existing);
        fs::create_dir_all(existing_path.parent().unwrap()).unwrap();
        fs::write(&existing_path, "user-owned\n").unwrap();

        cli()
            .args(["init", "--library", "probe-lib", "--directory"])
            .arg(directory.path())
            .assert()
            .failure()
            .stderr(contains("already exists"));

        assert_eq!(fs::read(&existing_path).unwrap(), b"user-owned\n");
        assert!(
            !directory
                .path()
                .join(".github/workflows/impact.yml")
                .exists()
        );
        assert!(
            !directory
                .path()
                .join(".github/workflows/impact-comment.yml")
                .exists()
        );
        assert!(
            !directory
                .path()
                .join(".github/cargo-impact/Cargo.toml")
                .exists()
        );
        if existing != "impact.toml" {
            assert!(!directory.path().join("impact.toml").exists());
        }
    }
}

#[test]
fn invalid_library_name_does_not_create_the_destination() {
    let parent = TempDir::new().unwrap();
    let destination = parent.path().join("must-not-exist");
    cli()
        .args(["init", "--library", "../probe-lib", "--directory"])
        .arg(&destination)
        .assert()
        .failure()
        .stderr(contains("invalid library name"));
    assert!(!destination.exists());
}

#[test]
fn cargo_plugin_style_argv_is_accepted() {
    cli()
        .args(["impact", "check", "--help"])
        .assert()
        .success()
        .stdout(contains("Usage: cargo-impact check"));
}

#[cfg(unix)]
#[test]
fn setup_rejects_a_symlink_inside_the_chosen_repository() {
    let directory = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), directory.path().join(".github")).unwrap();
    cli()
        .args(["init", "--library", "probe-lib", "--directory"])
        .arg(directory.path())
        .assert()
        .code(2)
        .stderr(contains("must be a directory without symlinks"));
    assert!(!directory.path().join("impact.toml").exists());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn saved_report_renders_offline_into_a_complete_bundle() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("input.json");
    fs::write(
        &input,
        serde_json::to_vec(&cargo_impact::ImpactReport::failed(
            "demo",
            "missing dependency",
        ))
        .unwrap(),
    )
    .unwrap();
    let output = directory.path().join("rendered");
    cli()
        .args(["report", "--input"])
        .arg(&input)
        .args(["--output-dir"])
        .arg(&output)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .assert()
        .success();
    for name in [
        "index.html",
        "report.json",
        "report.md",
        "summary.md",
        "report.sarif",
        "issues/index.md",
        "issues/index.json",
    ] {
        assert!(output.join(name).is_file(), "missing {name}");
    }
    let sarif: Value =
        serde_json::from_slice(&fs::read(output.join("report.sarif")).unwrap()).unwrap();
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["executionSuccessful"],
        false
    );
}

#[test]
fn bot_refuses_events_outside_github_actions() {
    cli()
        .args([
            "bot",
            "--event",
            "/no/such/event.json",
            "--repository",
            "owner/repository",
        ])
        .env("GITHUB_ACTIONS", "false")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .assert()
        .failure()
        .code(2)
        .stderr(contains("trusted GitHub Actions event file"));
}

#[test]
fn fatal_scan_error_writes_both_reports_before_exiting_two() {
    let directory = TempDir::new().unwrap();
    let candidate = directory.path().join("candidate");
    package(&candidate);
    let missing_baseline = directory.path().join("missing-baseline");
    let work_dir = directory.path().join("work");
    let report_dir = directory.path().join("report");

    cli()
        .args(["check", "--library", "probe-lib", "--baseline"])
        .arg(missing_baseline)
        .args(["--candidate"])
        .arg(candidate)
        .args(["--work-dir"])
        .arg(work_dir)
        .args(["--report-dir"])
        .arg(&report_dir)
        .args(["--no-discovery"])
        .assert()
        .failure()
        .code(2);

    let json: Value =
        serde_json::from_slice(&fs::read(report_dir.join("report.json")).unwrap()).unwrap();
    assert!(json["error"].as_str().unwrap().contains("No such file"));
    let markdown = fs::read_to_string(report_dir.join("report.md")).unwrap();
    assert!(markdown.contains("experiment did not complete"));
    assert!(markdown.contains("Adjust the library build recipe"));
}

#[test]
fn malformed_config_writes_failure_reports_with_config_path_and_library() {
    let directory = TempDir::new().unwrap();
    let config = directory.path().join("broken-impact.toml");
    fs::write(&config, "this is not valid TOML = [\n").unwrap();
    let report_dir = directory.path().join("report");

    let output = cli()
        .args(["check", "--library", "probe-lib", "--config"])
        .arg(&config)
        .args(["--baseline"])
        .arg(directory.path().join("missing-baseline"))
        .args(["--candidate"])
        .arg(directory.path().join("missing-candidate"))
        .args(["--report-dir"])
        .arg(&report_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let report: Value =
        serde_json::from_slice(&fs::read(report_dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["library"], "probe-lib");
    let error = report["error"].as_str().unwrap();
    assert!(error.contains(config.to_str().unwrap()), "{error}");
    assert!(error.contains("configuration file"), "{error}");
    let markdown = fs::read_to_string(report_dir.join("report.md")).unwrap();
    assert!(markdown.contains("experiment did not complete"));
    assert!(markdown.contains("broken-impact.toml"));
}

#[test]
fn invalid_upstream_context_replaces_previous_success_evidence() {
    let directory = TempDir::new().unwrap();
    let report_dir = directory.path().join("report");
    let mut old = cargo_impact::ImpactReport::failed("old-library", "old error");
    old.error = None;
    old.write_artifacts(&report_dir).unwrap();
    cli()
        .args([
            "check",
            "--library",
            "probe-lib",
            "--baseline",
            "missing-baseline",
            "--candidate",
            "missing-candidate",
            "--upstream-repository",
            "https://user:password@github.com/org/repo",
            "--report-dir",
        ])
        .arg(&report_dir)
        .assert()
        .code(2);
    let report = cargo_impact::ImpactReport::load(&report_dir.join("report.json")).unwrap();
    assert_eq!(report.library, "probe-lib");
    assert!(report.error.is_some());
    assert!(!report.error.unwrap().contains("password"));
    assert!(
        fs::read_to_string(report_dir.join("summary.md"))
            .unwrap()
            .contains("experiment did not complete")
    );
}

#[test]
fn local_recipe_without_allow_local_writes_failure_reports_without_building() {
    let directory = TempDir::new().unwrap();
    let config = directory.path().join("impact.toml");
    fs::write(
        &config,
        "library = 'probe-lib'\n\n[recipe.runner]\nkind = 'local'\n",
    )
    .unwrap();
    let report_dir = directory.path().join("report");

    let output = cli()
        .args(["check", "--config"])
        .arg(&config)
        .args(["--baseline"])
        .arg(directory.path().join("missing-baseline"))
        .args(["--candidate"])
        .arg(directory.path().join("missing-candidate"))
        .args(["--report-dir"])
        .arg(&report_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let report: Value =
        serde_json::from_slice(&fs::read(report_dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["library"], "probe-lib");
    let error = report["error"].as_str().unwrap();
    assert!(error.contains("--allow-local"), "{error}");
    let markdown = fs::read_to_string(report_dir.join("report.md")).unwrap();
    assert!(markdown.contains("--allow-local"));
    assert!(markdown.contains("Adjust the library build recipe"));
}

fn package(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe-lib\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn kept() {}\n").unwrap();
}

#[test]
fn verify_checks_published_views_before_reporting_validity() {
    let directory = TempDir::new().unwrap();
    let report = cargo_impact::ImpactReport::failed("demo", "retained failure");
    report.write_artifacts(directory.path()).unwrap();
    let output = cli()
        .args(["verify", "--json", "--report-dir"])
        .arg(directory.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let verified: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(verified["valid"], true);
    assert!(verified["bundle"].as_str().is_some());
    let manifest = cargo_impact::report::bundle::verify(directory.path())
        .unwrap()
        .manifest
        .unwrap();
    fs::write(
        directory
            .path()
            .join(manifest.directory())
            .join("report.md"),
        "changed view",
    )
    .unwrap();
    cli()
        .args(["verify", "--report-dir"])
        .arg(directory.path())
        .assert()
        .code(2)
        .stderr(contains("digest or size differs"));
}

#[test]
fn setup_rejects_blocking_ancestors_before_changing_any_files() {
    for ancestor in [".github/workflows", ".github/cargo-impact/src/report"] {
        let directory = TempDir::new().unwrap();
        let blocker = directory.path().join(ancestor);
        fs::create_dir_all(blocker.parent().unwrap()).unwrap();
        fs::write(&blocker, "user owned").unwrap();
        fs::write(directory.path().join(".gitignore"), "original\n").unwrap();
        cli()
            .args(["init", "--library", "demo", "--directory"])
            .arg(directory.path())
            .assert()
            .code(2);
        assert!(!directory.path().join("impact.toml").exists());
        assert_eq!(
            fs::read_to_string(directory.path().join(".gitignore")).unwrap(),
            "original\n"
        );
        assert_eq!(fs::read_to_string(blocker).unwrap(), "user owned");
    }
}

#[test]
fn replay_cannot_overwrite_its_input_generation_or_work_inside_the_bundle() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("input");
    cargo_impact::ImpactReport::failed("demo", "retained original")
        .write_artifacts(&input)
        .unwrap();
    let manifest = cargo_impact::report::bundle::verify(&input)
        .unwrap()
        .manifest
        .unwrap();
    let generation = input.join(manifest.directory());
    let original = fs::read(generation.join("report.json")).unwrap();
    cli()
        .args(["replay", "--report-dir"])
        .arg(&input)
        .args([
            "--experiment",
            &"a".repeat(64),
            "--baseline",
            "base",
            "--candidate",
            "head",
            "--output-dir",
        ])
        .arg(&generation)
        .assert()
        .code(2)
        .stderr(contains("outside its input report tree"));
    assert_eq!(fs::read(generation.join("report.json")).unwrap(), original);
    cli()
        .args(["replay", "--report-dir"])
        .arg(&input)
        .args([
            "--experiment",
            &"a".repeat(64),
            "--baseline",
            "base",
            "--candidate",
            "head",
            "--output-dir",
        ])
        .arg(directory.path().join("output"))
        .arg("--work-dir")
        .arg(input.join("future/work"))
        .assert()
        .code(2)
        .stderr(contains("outside its input report tree"));
    assert!(!directory.path().join("output").exists());
    cargo_impact::report::bundle::verify(&input).unwrap();
}

#[test]
fn publish_accepts_a_bare_report_filename_and_rejects_damage_before_api_access() {
    let directory = TempDir::new().unwrap();
    cargo_impact::ImpactReport::failed("demo", "original")
        .write_artifacts(directory.path())
        .unwrap();
    let manifest = cargo_impact::report::bundle::verify(directory.path())
        .unwrap()
        .manifest
        .unwrap();
    fs::write(
        directory
            .path()
            .join(manifest.directory())
            .join("index.html"),
        "damaged",
    )
    .unwrap();
    cli()
        .current_dir(directory.path())
        .args([
            "publish",
            "--repository",
            "owner/repo",
            "--pull-request",
            "1",
            "--candidate-sha",
            &"a".repeat(40),
            "--report",
            "report.json",
            "--run-url",
            "https://github.com/owner/repo/actions/runs/1",
        ])
        .assert()
        .code(2)
        .stderr(contains("digest or size differs"));
}
