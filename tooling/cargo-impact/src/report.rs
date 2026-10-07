//! One evidence model, several views: Markdown, interactive HTML, SARIF and issue bundles.

use crate::{
    discovery::Discovery,
    model::{GateResult, ImpactReport},
};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};
use thiserror::Error;

pub mod bundle;
mod html;
mod issues;
mod markdown;
mod presentation;
mod sarif;
mod view;
pub use issues::IssueDraft;
pub use view::ReportSummary;

const MAX_REPORT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("report I/O: {0}")]
    Io(#[from] io::Error),
    #[error("invalid report JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("report exceeds the 512 MiB input limit")]
    TooLarge,
    #[error("unsupported report schema {0}; this scanner supports schema 1")]
    Schema(u32),
    #[error(transparent)]
    Bundle(#[from] bundle::BundleError),
}

impl ImpactReport {
    pub fn failed(library: &str, error: &str) -> Self {
        Self {
            schema_version: 1,
            run: Default::default(),
            library: library.into(),
            baseline_fingerprint: None,
            candidate_fingerprint: None,
            error: Some(error.into()),
            gate: GateResult {
                ran: false,
                required_bump: None,
                reason: "The experiment did not complete; its result is inconclusive.".into(),
                log: String::new(),
            },
            discovery: Discovery::default(),
            downstreams: Vec::new(),
        }
    }
    pub fn load(path: &Path) -> Result<Self, ReportError> {
        let file = fs::File::open(path)?;
        if file.metadata()?.len() > MAX_REPORT_BYTES {
            return Err(ReportError::TooLarge);
        }
        let mut bytes = Vec::new();
        file.take(MAX_REPORT_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_REPORT_BYTES {
            return Err(ReportError::TooLarge);
        }
        Self::from_bytes(&bytes)
    }
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, ReportError> {
        if bytes.len() as u64 > MAX_REPORT_BYTES {
            return Err(ReportError::TooLarge);
        }
        let report: Self = serde_json::from_slice(bytes)?;
        if report.schema_version != 1 {
            return Err(ReportError::Schema(report.schema_version));
        }
        Ok(report)
    }
    pub fn has_regressions(&self) -> bool {
        ReportSummary::from_report(self).regressions > 0
    }
    pub fn has_harness_failures(&self) -> bool {
        self.error.is_some() || ReportSummary::from_report(self).harness_failures > 0
    }
    pub fn markdown(&self) -> String {
        markdown::render(self, false)
    }
    pub fn html(&self) -> String {
        html::render(self)
    }
    pub fn sarif(&self) -> serde_json::Value {
        sarif::render(self)
    }
    pub(crate) fn comment_markdown(&self) -> String {
        let value = markdown::render(self, true);
        if value.chars().count() <= 30_000 && self.error.as_ref().is_none_or(|e| e.len() <= 30_000)
        {
            return value;
        }
        let summary = ReportSummary::from_report(self);
        format!(
            "## Downstream impact · {}\n\n**{} regressions** · {} compatible · {} baseline failures · {} harness failures · {} not exercised · {} pending\n\n{}\n\nThis report exceeds the comment limit. Download the artifact bundle and open index.html for highlighted spans, consumer filtering, issue drafts, reproduction recipes and all retained evidence.",
            presentation::inline(&presentation::clip(&self.library, 100)),
            summary.regressions,
            summary.compatible,
            summary.baseline_failures,
            summary.harness_failures,
            summary.not_exercised,
            summary.pending,
            presentation::escape(&presentation::clip(&self.gate.reason, 1_000))
        )
    }
    /// Atomic checkpoints preserve completed evidence when a runner cancels the job.
    pub fn write_checkpoint(&self, directory: &Path) -> Result<(), ReportError> {
        Ok(bundle::checkpoint(self, directory)?)
    }
    pub fn write_artifacts(&self, directory: &Path) -> Result<(), ReportError> {
        Ok(bundle::publish(self, directory)?)
    }
}

/// Remove only our hashed filenames. User notes and other files remain intact.
fn clear_issue_drafts(directory: &Path) -> io::Result<()> {
    let issues = directory.join("issues");
    fs::create_dir_all(&issues)?;
    for entry in fs::read_dir(&issues)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some((stem, extension)) = name.rsplit_once('.') else {
            continue;
        };
        let owned = stem.strip_prefix("consumer-").is_some_and(|hash| {
            matches!(hash.len(), 16 | 64) && hash.bytes().all(|c| c.is_ascii_hexdigit())
        }) && matches!(extension, "md" | "toml");
        if owned && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
        }
    }
    atomic_bytes(&issues.join("index.json"), b"[]\n")?;
    atomic_bytes(&issues.join("index.md"), b"# Issue drafts\n\nThis run has no exported drafts yet. Render report.json to export completed regressions.\n")
}

fn atomic_bytes(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("artifact has no parent"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(contents)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}
