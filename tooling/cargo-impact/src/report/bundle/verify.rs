//! Verify transport integrity before trusting a downloaded report or replay.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::ImpactReport;

use super::{
    BundleError, BundleManifest, MAX_BUNDLE_BYTES, Publication, VERSION, VerifiedEvidence,
    generation_manifest, io, read_publication,
};

pub fn verify(directory: &Path) -> Result<VerifiedEvidence, BundleError> {
    let root = directory.canonicalize()?;
    let (report, report_sha256, manifest) = match read_publication(&root)? {
        Publication::Checkpoint { version, report } => {
            if version != VERSION {
                return Err(BundleError::Version(version));
            }
            let path = io::owned_file(&root, Path::new("report.json"))?;
            (verified_report(&path, &report)?, report.sha256, None)
        }
        Publication::Bundle { manifest } => {
            verify_generation(&root, &manifest)?;
            let report_path = io::owned_file(&root, &manifest.directory().join("report.json"))?;
            let current = io::owned_file(&root, Path::new("report.json"))?;
            if io::digest(&current, super::super::MAX_REPORT_BYTES)?
                != manifest.artifacts[Path::new("report.json")]
            {
                return Err(BundleError::UnpublishedCheckpoint);
            }
            let report =
                verified_report(&report_path, &manifest.artifacts[Path::new("report.json")])?;
            let report_sha256 = manifest.artifacts[Path::new("report.json")].sha256.clone();
            (report, report_sha256, Some(manifest))
        }
    };
    Ok(VerifiedEvidence {
        report,
        report_sha256,
        manifest,
    })
}

fn verified_report(
    path: &Path,
    expected: &super::ArtifactDigest,
) -> Result<ImpactReport, BundleError> {
    use sha2::{Digest, Sha256};
    // Parse the same bounded bytes that were hashed. Reopening a checkpoint
    // could consume a newer publication whose bytes have never been verified.
    let bytes = io::bounded_read(path, super::super::MAX_REPORT_BYTES)?;
    if bytes.len() as u64 != expected.bytes
        || format!("{:x}", Sha256::digest(&bytes)) != expected.sha256
    {
        return Err(BundleError::Changed("report.json".into()));
    }
    ImpactReport::from_bytes(&bytes).map_err(|error| BundleError::Report(error.to_string()))
}

pub(super) fn verify_generation(root: &Path, manifest: &BundleManifest) -> Result<(), BundleError> {
    manifest.validate()?;
    let directory = root.join(manifest.directory());
    // Validate every component from the caller's chosen root, including bundles/id.
    io::owned_file(root, &manifest.directory().join("manifest.json"))?;
    if generation_manifest(&directory)? != *manifest {
        return Err(BundleError::InvalidManifest);
    }
    verify_inventory(&directory, manifest)?;
    for (relative, expected) in &manifest.artifacts {
        let file = io::owned_file(root, &manifest.directory().join(relative))?;
        if io::digest(&file, MAX_BUNDLE_BYTES)? != *expected {
            return Err(BundleError::Changed(relative.clone()));
        }
    }
    Ok(())
}

fn verify_inventory(directory: &Path, manifest: &BundleManifest) -> Result<(), BundleError> {
    let mut pending = vec![PathBuf::new()];
    let mut seen = BTreeSet::new();
    let mut entries = 0usize;
    while let Some(relative) = pending.pop() {
        for entry in fs::read_dir(directory.join(&relative))? {
            entries += 1;
            if entries > super::MAX_ARTIFACTS * 8 {
                return Err(BundleError::TooLarge);
            }
            let entry = entry?;
            let path = relative.join(entry.file_name());
            io::relative_path(&path)?;
            let kind = entry.file_type()?;
            if kind.is_dir()
                && manifest
                    .artifacts
                    .keys()
                    .any(|file| file.starts_with(&path))
            {
                pending.push(path);
            } else if kind.is_file()
                && (path == Path::new("manifest.json") || manifest.artifacts.contains_key(&path))
            {
                seen.insert(path);
            } else {
                return Err(BundleError::UnsafePath(path));
            }
        }
    }
    if seen.len() != manifest.artifacts.len() + 1 {
        return Err(BundleError::InvalidManifest);
    }
    Ok(())
}
