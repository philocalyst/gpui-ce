//! Retention removes only complete, verified generations and never the publication.

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde::Serialize;

use super::{BundleError, generation_manifest, io, is_digest, verify, verify::verify_generation};

#[derive(Debug, Serialize)]
pub struct RetentionReport {
    pub applied: bool,
    pub removable: Vec<PathBuf>,
    pub bytes: u64,
    /// Unknown, incomplete or altered directories require inspection and remain untouched.
    pub preserved: Vec<PathBuf>,
}

/// Keep the publication plus `keep_previous` newest verified generations.
/// Planning and deletion hold the same writer lock. `apply = false` only previews.
pub fn prune(
    directory: &Path,
    keep_previous: usize,
    apply: bool,
) -> Result<RetentionReport, BundleError> {
    let root = directory.canonicalize()?;
    let _lock = io::WriterLock::acquire(&root)?;
    let current = verify(&root)?.manifest;
    // A checkpoint may refer to work whose publication was interrupted. Retain
    // all generations until it is rendered into a complete publication.
    let current = current.ok_or(BundleError::UnpublishedCheckpoint)?;
    let mut candidates = Vec::new();
    let mut report = RetentionReport {
        applied: apply,
        removable: Vec::new(),
        bytes: 0,
        preserved: Vec::new(),
    };
    for entry in fs::read_dir(root.join("bundles"))? {
        let entry = entry?;
        let name = entry.file_name();
        let relative = PathBuf::from("bundles").join(&name);
        if name == current.id.as_str() {
            continue;
        }
        if !entry.file_type()?.is_dir() || !name.to_str().is_some_and(is_digest) {
            report.preserved.push(relative);
            continue;
        }
        let manifest = match generation_manifest(&entry.path()) {
            Ok(manifest)
                if manifest.directory() == relative
                    && verify_generation(&root, &manifest).is_ok() =>
            {
                manifest
            }
            _ => {
                report.preserved.push(relative);
                continue;
            }
        };
        let modified = entry
            .metadata()?
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        candidates.push((modified, relative, manifest.bytes()));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (_, relative, bytes) in candidates.into_iter().skip(keep_previous) {
        report.bytes = report.bytes.saturating_add(bytes);
        report.removable.push(relative);
    }
    report.preserved.sort();
    if apply {
        for relative in &report.removable {
            // Quarantine before deletion so an interruption cannot leave a
            // partially removed directory looking like a complete generation.
            let quarantine = tempfile::Builder::new()
                .prefix(".pruning-")
                .tempdir_in(root.join("bundles"))?;
            fs::rename(root.join(relative), quarantine.path().join("generation"))?;
            quarantine.close()?;
        }
        io::sync_directory(&root.join("bundles"))?;
    }
    Ok(report)
}
