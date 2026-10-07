//! Immutable report generations and an explicit publication record.
//!
//! A checkpoint keeps the newest authoritative JSON. A published bundle names
//! a complete, content-addressed artifact set; its views never link to mutable
//! root aliases. No symlinks are required, so ZIP downloads and file:// work.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::ImpactReport;

mod io;
mod retention;
mod verify;
mod writer;

pub use retention::{RetentionReport, prune};
pub use verify::verify;
pub(super) use writer::{checkpoint, publish};

const VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BUNDLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARTIFACTS: usize = 50_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDigest {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    pub version: u32,
    pub id: String,
    pub artifacts: BTreeMap<PathBuf, ArtifactDigest>,
}

impl BundleManifest {
    pub fn directory(&self) -> PathBuf {
        PathBuf::from("bundles").join(&self.id)
    }

    pub fn bytes(&self) -> u64 {
        self.artifacts
            .values()
            .fold(0, |total, artifact| total.saturating_add(artifact.bytes))
    }

    fn from_artifacts(artifacts: BTreeMap<PathBuf, ArtifactDigest>) -> Result<Self, BundleError> {
        let mut hash = Sha256::new();
        hash.update(b"cargo-impact/bundle/v1\0");
        for (path, artifact) in &artifacts {
            io::relative_path(path)?;
            let path = path
                .to_str()
                .ok_or_else(|| BundleError::UnsafePath(path.clone()))?;
            hash.update((path.len() as u64).to_le_bytes());
            hash.update(path.as_bytes());
            hash.update(artifact.bytes.to_le_bytes());
            hash.update(artifact.sha256.as_bytes());
        }
        Ok(Self {
            version: VERSION,
            id: format!("{:x}", hash.finalize()),
            artifacts,
        })
    }

    fn validate(&self) -> Result<(), BundleError> {
        if self.version != VERSION {
            return Err(BundleError::Version(self.version));
        }
        if !is_digest(&self.id)
            || self.artifacts.len() > MAX_ARTIFACTS
            || [
                "report.json",
                "report.md",
                "summary.md",
                "index.html",
                "report.sarif",
                "issues/index.md",
                "issues/index.json",
            ]
            .iter()
            .any(|path| !self.artifacts.contains_key(Path::new(path)))
            || self.artifacts.values().any(|file| !is_digest(&file.sha256))
            || self
                .artifacts
                .values()
                .try_fold(0u64, |sum, file| sum.checked_add(file.bytes))
                .is_none_or(|sum| sum > MAX_BUNDLE_BYTES)
        {
            return Err(BundleError::InvalidManifest);
        }
        for path in self.artifacts.keys() {
            io::relative_path(path)?;
        }
        if Self::from_artifacts(self.artifacts.clone())?.id != self.id {
            return Err(BundleError::InvalidManifest);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Publication {
    Checkpoint {
        version: u32,
        report: ArtifactDigest,
    },
    Bundle {
        manifest: BundleManifest,
    },
}

/// The verifier returns the exact JSON from the verified artifact set.
/// A checkpoint is useful evidence, but does not include a complete view bundle.
#[derive(Debug)]
pub struct VerifiedEvidence {
    pub report: ImpactReport,
    /// Hash of the exact JSON bytes verified and deserialized, including checkpoints.
    pub report_sha256: String,
    pub manifest: Option<BundleManifest>,
}

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("report bundle I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid report bundle JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported report bundle version {0}")]
    Version(u32),
    #[error("invalid report bundle manifest or content identity")]
    InvalidManifest,
    #[error("unsafe report artifact path: {0}")]
    UnsafePath(PathBuf),
    #[error("report artifact digest or size differs: {0}")]
    Changed(PathBuf),
    #[error("report bundle exceeds its artifact, manifest or byte limit")]
    TooLarge,
    #[error("another writer is publishing to this report directory")]
    Busy,
    #[error(
        "the root checkpoint differs from the published bundle; render report.json to recover the latest evidence"
    )]
    UnpublishedCheckpoint,
    #[error("report evidence cannot be loaded: {0}")]
    Report(String),
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn read_publication(root: &Path) -> Result<Publication, BundleError> {
    let path = io::owned_file(root, Path::new("bundle.json"))?;
    Ok(serde_json::from_slice(&io::bounded_read(
        &path,
        MAX_MANIFEST_BYTES,
    )?)?)
}

fn generation_manifest(directory: &Path) -> Result<BundleManifest, BundleError> {
    let path = io::owned_file(directory, Path::new("manifest.json"))?;
    Ok(serde_json::from_slice(&io::bounded_read(
        &path,
        MAX_MANIFEST_BYTES,
    )?)?)
}
