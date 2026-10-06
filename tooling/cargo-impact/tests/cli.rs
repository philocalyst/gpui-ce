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
    cli()
        .args(["init", "--library", "probe-lib", "--directory"])
        .arg(directory.path())
        .assert()
        .success();

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

fn package(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe-lib\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn kept() {}\n").unwrap();
}
