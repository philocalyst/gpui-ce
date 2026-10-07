//! One evidence model, several views: Markdown, interactive HTML, SARIF and issue bundles.

use crate::{
    discovery::Discovery,
    model::{Classification, GateResult, ImpactReport},
};
use serde::Serialize;
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};
use thiserror::Error;

mod html;
mod issues;
mod markdown;
mod presentation;
mod sarif;
pub use issues::IssueDraft;

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
        let report: Self = serde_json::from_slice(&bytes)?;
        if report.schema_version != 1 {
            return Err(ReportError::Schema(report.schema_version));
        }
        Ok(report)
    }
    pub fn has_regressions(&self) -> bool {
        self.downstreams
            .iter()
            .any(|r| r.classification == Classification::Regression)
    }
    pub fn has_harness_failures(&self) -> bool {
        self.error.is_some()
            || self
                .downstreams
                .iter()
                .any(|r| r.classification == Classification::HarnessFailure)
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
        let count = |kind| {
            self.downstreams
                .iter()
                .filter(|r| r.classification == kind)
                .count()
        };
        format!(
            "## Downstream impact · {}\n\n**{} regressions** · {} compatible · {} baseline failures · {} harness failures · {} not exercised\n\n{}\n\nThis report exceeds the comment limit. Download the artifact bundle and open index.html for highlighted spans, consumer filtering, issue drafts, reproduction recipes and all retained evidence.",
            presentation::inline(&presentation::clip(&self.library, 100)),
            count(Classification::Regression),
            count(Classification::Compatible),
            count(Classification::PreExistingFailure),
            count(Classification::HarnessFailure),
            count(Classification::NotExercised),
            presentation::escape(&presentation::clip(&self.gate.reason, 1_000))
        )
    }
    /// Atomic checkpoints preserve completed evidence when a runner cancels the job.
    pub fn write_checkpoint(&self, directory: &Path) -> Result<(), ReportError> {
        fs::create_dir_all(directory)?;
        // Completed evidence takes precedence over secondary view failures.
        atomic_json(&directory.join("report.json"), self)?;
        // Replace the previous run's views before exposing the new checkpoint.
        // Re-rendering is offline; no untrusted build runs when opening a checkpoint.
        let partial = format!(
            "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Partial cargo-impact report</title><h1>Partial downstream impact report</h1><p>{}/{} consumers completed. Missing results are inconclusive.</p><p>Render the current evidence with <code>cargo impact report --input report.json --output-dir .</code>.</p><a href=\"report.json\">Current JSON evidence</a></html>",
            self.run.completed_downstreams, self.run.planned_downstreams
        );
        atomic_bytes(&directory.join("index.html"), partial.as_bytes())?;
        clear_issue_drafts(directory)?;
        atomic_json(&directory.join("report.sarif"), &self.sarif())?;
        atomic_bytes(
            &directory.join("report.md"),
            self.comment_markdown().as_bytes(),
        )?;
        atomic_bytes(
            &directory.join("summary.md"),
            self.comment_markdown().as_bytes(),
        )?;
        Ok(())
    }
    pub fn write_artifacts(&self, directory: &Path) -> Result<(), ReportError> {
        fs::create_dir_all(directory)?;
        atomic_json(&directory.join("report.json"), self)?;
        clear_issue_drafts(directory)?;
        atomic_bytes(&directory.join("report.md"), self.markdown().as_bytes())?;
        atomic_bytes(
            &directory.join("summary.md"),
            self.comment_markdown().as_bytes(),
        )?;
        atomic_bytes(&directory.join("index.html"), self.html().as_bytes())?;
        atomic_json(&directory.join("report.sarif"), &self.sarif())?;
        let issues = directory.join("issues");
        fs::create_dir_all(&issues)?;
        let drafts = self.issue_drafts();
        let mut index = String::from(
            "# Issue drafts\n\nThese are local drafts for human review. No issue has been submitted.\n\n| Consumer | Draft | Reproduction recipe | Issue form |\n| --- | --- | --- | --- |\n",
        );
        for draft in &drafts {
            use std::fmt::Write;
            atomic_bytes(&issues.join(&draft.filename), draft.body.as_bytes())?;
            atomic_bytes(
                &issues.join(&draft.reproduction_filename),
                draft.reproduction_config.as_bytes(),
            )?;
            let link = draft
                .composer_url
                .as_ref()
                .map(|url| format!("[Review draft](<{url}>)"))
                .unwrap_or_else(|| "Copy Markdown into your tracker".into());
            let _ = writeln!(
                index,
                "| {} | [{}]({}) | [TOML]({}) | {} |",
                presentation::escape(&draft.downstream),
                presentation::escape(&draft.title),
                draft.filename,
                draft.reproduction_filename,
                link
            );
        }
        if drafts.is_empty() {
            index.push_str(
                "\nNo proven regressions were recorded, so no issue drafts were generated.\n",
            );
        }
        atomic_bytes(&issues.join("index.md"), index.as_bytes())?;
        atomic_json(&issues.join("index.json"), &drafts)?;
        Ok(())
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
        let owned = stem
            .strip_prefix("consumer-")
            .is_some_and(|hash| hash.len() == 16 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
            && matches!(extension, "md" | "toml");
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
fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ReportError> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("artifact has no parent"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(file.as_file_mut(), value)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}
