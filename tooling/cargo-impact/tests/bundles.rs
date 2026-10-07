use std::fs;

use cargo_impact::{
    BuildProvenance, BuildResult, Classification, DownstreamResult, DownstreamSource, ImpactReport,
    LockfileEvidence,
    report::bundle::{self, BundleError},
    runner::BuildRecipe,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn report(label: &str) -> ImpactReport {
    ImpactReport::failed("demo", label)
}

#[test]
fn publication_retains_immutable_generations_and_reuses_identical_rendering() {
    let directory = TempDir::new().unwrap();
    let first = report("first evidence");
    first.write_artifacts(directory.path()).unwrap();
    let verified = bundle::verify(directory.path()).unwrap();
    let manifest = verified.manifest.unwrap();
    let saved = directory.path().join(manifest.directory());
    let old_html = fs::read(saved.join("index.html")).unwrap();
    first.write_artifacts(directory.path()).unwrap();
    assert_eq!(
        bundle::verify(directory.path())
            .unwrap()
            .manifest
            .unwrap()
            .id,
        manifest.id
    );
    report("second evidence")
        .write_artifacts(directory.path())
        .unwrap();
    let current = bundle::verify(directory.path()).unwrap();
    assert_ne!(current.manifest.as_ref().unwrap().id, manifest.id);
    assert_eq!(fs::read(saved.join("index.html")).unwrap(), old_html);
    assert_eq!(
        ImpactReport::load(&saved.join("report.json"))
            .unwrap()
            .error
            .as_deref(),
        Some("first evidence")
    );
    let markdown = fs::read_to_string(directory.path().join("report.md")).unwrap();
    assert!(markdown.contains(&format!(
        "bundles/{}/report.json",
        current.manifest.unwrap().id
    )));
}

#[test]
fn changed_artifact_and_unpublished_checkpoint_are_rejected() {
    let directory = TempDir::new().unwrap();
    report("saved evidence")
        .write_artifacts(directory.path())
        .unwrap();
    let manifest = bundle::verify(directory.path()).unwrap().manifest.unwrap();
    let html = directory
        .path()
        .join(manifest.directory())
        .join("index.html");
    let original = fs::read(&html).unwrap();
    fs::write(&html, b"altered during transport").unwrap();
    assert!(matches!(
        bundle::verify(directory.path()),
        Err(BundleError::Changed(_))
    ));
    fs::write(&html, original).unwrap();
    fs::write(
        directory.path().join("report.json"),
        serde_json::to_vec(&report("new checkpoint")).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        bundle::verify(directory.path()),
        Err(BundleError::UnpublishedCheckpoint)
    ));
}

#[test]
fn incomplete_secondary_views_never_commit_a_new_publication() {
    let directory = TempDir::new().unwrap();
    report("previous complete publication")
        .write_artifacts(directory.path())
        .unwrap();
    let pointer = fs::read(directory.path().join("bundle.json")).unwrap();
    fs::remove_file(directory.path().join("index.html")).unwrap();
    fs::create_dir(directory.path().join("index.html")).unwrap();
    assert!(
        report("durable latest evidence")
            .write_artifacts(directory.path())
            .is_err()
    );
    assert_eq!(
        fs::read(directory.path().join("bundle.json")).unwrap(),
        pointer
    );
    assert_eq!(
        ImpactReport::load(&directory.path().join("report.json"))
            .unwrap()
            .error
            .as_deref(),
        Some("durable latest evidence")
    );
    assert!(matches!(
        bundle::verify(directory.path()),
        Err(BundleError::UnpublishedCheckpoint)
    ));
}

#[test]
fn checkpoints_have_a_distinct_verifiable_contract() {
    let directory = TempDir::new().unwrap();
    let mut partial = report("pending");
    partial.run.status = cargo_impact::RunStatus::Running;
    partial.write_checkpoint(directory.path()).unwrap();
    let verified = bundle::verify(directory.path()).unwrap();
    assert!(verified.manifest.is_none());
    assert_eq!(verified.report.run.status, cargo_impact::RunStatus::Running);
    fs::write(directory.path().join("report.json"), b"{}").unwrap();
    assert!(matches!(
        bundle::verify(directory.path()),
        Err(BundleError::Changed(_))
    ));
}

#[test]
fn manifest_cannot_direct_the_verifier_outside_the_report() {
    let directory = TempDir::new().unwrap();
    report("evidence")
        .write_artifacts(directory.path())
        .unwrap();
    let pointer = directory.path().join("bundle.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&pointer).unwrap()).unwrap();
    manifest["manifest"]["artifacts"]["../outside"] =
        serde_json::json!({"bytes":0,"sha256":"a".repeat(64)});
    fs::write(pointer, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(matches!(
        bundle::verify(directory.path()),
        Err(BundleError::UnsafePath(_))
    ));
}

#[cfg(unix)]
#[test]
fn internal_symlinks_are_rejected_for_reading_and_publication() {
    let directory = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let sentinel = outside.path().join("notes");
    fs::write(&sentinel, "user owned").unwrap();
    std::os::unix::fs::symlink(outside.path(), directory.path().join("bundles")).unwrap();
    assert!(
        report("evidence")
            .write_artifacts(directory.path())
            .is_err()
    );
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "user owned");
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
    fs::remove_file(directory.path().join("bundles")).unwrap();
    report("evidence")
        .write_artifacts(directory.path())
        .unwrap();
    let manifest = bundle::verify(directory.path()).unwrap().manifest.unwrap();
    let html = directory
        .path()
        .join(manifest.directory())
        .join("index.html");
    fs::remove_file(&html).unwrap();
    std::os::unix::fs::symlink(sentinel, html).unwrap();
    assert!(matches!(
        bundle::verify(directory.path()),
        Err(BundleError::UnsafePath(_))
    ));
}

#[test]
fn resolved_locks_are_exported_verified_and_cannot_disagree_with_provenance() {
    let directory = TempDir::new().unwrap();
    let mut report = report("lock evidence");
    let contents = "version = 4\n".to_owned();
    let sha256 = format!("{:x}", Sha256::digest(contents.as_bytes()));
    let build = BuildResult {
        success: true,
        provenance: BuildProvenance {
            lock_fingerprint: Some(sha256.clone()),
            ..Default::default()
        },
        lockfile: Some(LockfileEvidence {
            sha256: sha256.clone(),
            contents: contents.clone(),
        }),
        ..Default::default()
    };
    report.downstreams.push(DownstreamResult {
        experiment_id: None,
        lifecycle: Default::default(),
        name: "app".into(),
        revision: None,
        source: DownstreamSource::Local { path: "app".into() },
        manifest: "Cargo.toml".into(),
        source_fingerprint: Some("source".into()),
        recipe: BuildRecipe::default(),
        classification: Classification::Compatible,
        baseline: build.clone(),
        candidate: build,
        message: None,
        elapsed_ms: 0,
    });
    report.write_artifacts(directory.path()).unwrap();
    let manifest = bundle::verify(directory.path()).unwrap().manifest.unwrap();
    let locks: Vec<_> = manifest
        .artifacts
        .iter()
        .filter(|(path, _)| path.starts_with("locks"))
        .collect();
    assert_eq!(locks.len(), 2);
    for (path, digest) in locks {
        assert_eq!(digest.sha256, sha256);
        assert_eq!(
            fs::read_to_string(directory.path().join(manifest.directory()).join(path)).unwrap(),
            contents
        );
    }
    report.downstreams[0].candidate.provenance.lock_fingerprint = Some("b".repeat(64));
    assert!(report.write_artifacts(directory.path()).is_err());
    assert!(matches!(
        bundle::verify(directory.path()),
        Err(BundleError::UnpublishedCheckpoint)
    ));
}

#[test]
fn a_checkpoint_failure_preserves_user_notes_and_completed_evidence() {
    let directory = TempDir::new().unwrap();
    fs::create_dir(directory.path().join("issues")).unwrap();
    fs::write(directory.path().join("issues/notes.md"), "user notes").unwrap();
    fs::create_dir(directory.path().join("index.html")).unwrap();
    let report = report("durable error");
    assert!(report.write_checkpoint(directory.path()).is_err());
    assert!(directory.path().join("report.json").is_file());
    assert_eq!(
        fs::read_to_string(directory.path().join("issues/notes.md")).unwrap(),
        "user notes"
    );
}

#[test]
fn unlisted_files_and_empty_directories_are_not_verified_evidence() {
    let directory = TempDir::new().unwrap();
    report("evidence")
        .write_artifacts(directory.path())
        .unwrap();
    let generation = directory.path().join(
        bundle::verify(directory.path())
            .unwrap()
            .manifest
            .unwrap()
            .directory(),
    );
    fs::write(generation.join("unlisted.html"), "unexpected view").unwrap();
    assert!(matches!(
        bundle::verify(directory.path()),
        Err(BundleError::UnsafePath(_))
    ));
    fs::remove_file(generation.join("unlisted.html")).unwrap();
    fs::create_dir(generation.join("empty-unlisted")).unwrap();
    assert!(matches!(
        bundle::verify(directory.path()),
        Err(BundleError::UnsafePath(_))
    ));
}

#[test]
fn retention_preserves_current_corrupt_unknown_and_user_owned_files() {
    let directory = TempDir::new().unwrap();
    let mut previous = Vec::new();
    for label in ["first", "second", "current"] {
        report(label).write_artifacts(directory.path()).unwrap();
        previous.push(
            bundle::verify(directory.path())
                .unwrap()
                .manifest
                .unwrap()
                .directory(),
        );
    }
    let corrupt = directory.path().join(&previous[0]);
    fs::write(corrupt.join("index.html"), "changed evidence").unwrap();
    let notes = directory.path().join("bundles/notes");
    fs::create_dir(&notes).unwrap();
    fs::write(notes.join("research.md"), "user owned").unwrap();
    let preview = bundle::prune(directory.path(), 0, false).unwrap();
    assert_eq!(preview.removable, vec![previous[1].clone()]);
    assert!(directory.path().join(&previous[1]).is_dir());
    let applied = bundle::prune(directory.path(), 0, true).unwrap();
    assert!(applied.applied && applied.bytes > 0);
    assert!(!directory.path().join(&previous[1]).exists());
    assert!(corrupt.is_dir() && notes.join("research.md").is_file());
    assert_eq!(
        bundle::verify(directory.path())
            .unwrap()
            .report
            .error
            .as_deref(),
        Some("current")
    );
}

#[test]
fn retention_refuses_a_checkpoint_or_uncommitted_report() {
    let directory = TempDir::new().unwrap();
    report("checkpoint")
        .write_checkpoint(directory.path())
        .unwrap();
    assert!(matches!(
        bundle::prune(directory.path(), 0, true),
        Err(BundleError::UnpublishedCheckpoint)
    ));
}

#[test]
fn concurrent_publication_refuses_to_overwrite_an_owned_checkpoint() {
    use fs2::FileExt;
    let directory = TempDir::new().unwrap();
    report("owned checkpoint")
        .write_artifacts(directory.path())
        .unwrap();
    let lock = fs::File::open(directory.path().join(".cargo-impact-report.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    assert!(matches!(
        report("concurrent replacement").write_artifacts(directory.path()),
        Err(cargo_impact::report::ReportError::Bundle(BundleError::Busy))
    ));
    assert_eq!(
        ImpactReport::load(&directory.path().join("report.json"))
            .unwrap()
            .error
            .as_deref(),
        Some("owned checkpoint")
    );
    FileExt::unlock(&lock).unwrap();
}

#[test]
fn a_verified_checkpoint_returns_the_exact_bytes_named_by_its_digest_during_replacement() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let directory = TempDir::new().unwrap();
    let mut variants = Vec::new();
    for label in ['a', 'b'] {
        let report = report(&label.to_string().repeat(256 * 1024));
        let bytes = serde_json::to_vec_pretty(&report).unwrap();
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let pointer = serde_json::to_vec(&serde_json::json!({
            "kind":"checkpoint", "version":1,
            "report":{"bytes":bytes.len(),"sha256":sha256}
        }))
        .unwrap();
        variants.push((bytes, pointer));
    }
    fs::write(directory.path().join("report.json"), &variants[0].0).unwrap();
    fs::write(directory.path().join("bundle.json"), &variants[0].1).unwrap();
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for index in 0..100 {
                let (bytes, pointer) = &variants[index % 2];
                fs::write(directory.path().join("next-report"), bytes).unwrap();
                fs::rename(
                    directory.path().join("next-report"),
                    directory.path().join("report.json"),
                )
                .unwrap();
                fs::write(directory.path().join("next-pointer"), pointer).unwrap();
                fs::rename(
                    directory.path().join("next-pointer"),
                    directory.path().join("bundle.json"),
                )
                .unwrap();
            }
            done.store(true, Ordering::Release);
        });
        while !done.load(Ordering::Acquire) {
            if let Ok(evidence) = bundle::verify(directory.path()) {
                let bytes = serde_json::to_vec_pretty(&evidence.report).unwrap();
                assert_eq!(
                    format!("{:x}", Sha256::digest(bytes)),
                    evidence.report_sha256
                );
            }
        }
    });
    let evidence = bundle::verify(directory.path()).unwrap();
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec_pretty(&evidence.report).unwrap())
        ),
        evidence.report_sha256
    );
}
