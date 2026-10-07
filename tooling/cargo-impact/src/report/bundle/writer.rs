//! Render into a private stage, validate its inventory, then publish one pointer.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::ImpactReport;

use super::super::{IssueDraft, MAX_REPORT_BYTES, markdown, presentation};
use super::{
    ArtifactDigest, BundleError, BundleManifest, MAX_ARTIFACTS, MAX_BUNDLE_BYTES, Publication,
    VERSION, io, verify::verify_generation,
};

pub(in crate::report) fn checkpoint(
    report: &ImpactReport,
    directory: &Path,
) -> Result<(), BundleError> {
    let root = output_root(directory)?;
    let _lock = io::WriterLock::acquire(&root)?;
    let digest = io::atomic_json_bounded(&root.join("report.json"), report, MAX_REPORT_BYTES)?;
    let partial = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Partial cargo-impact report</title><h1>Partial downstream impact report</h1><p>{}/{} consumers completed. Missing results are inconclusive.</p><p>Render the current evidence with <code>cargo impact report --input report.json --output-dir .</code>.</p><a href=\"report.json\">Current JSON evidence</a></html>",
        report.run.completed_downstreams, report.run.planned_downstreams
    );
    io::atomic_bytes(&root.join("index.html"), partial.as_bytes())?;
    io::directory(&root, Path::new("issues"))?;
    super::super::clear_issue_drafts(&root)?;
    io::atomic_json_bounded(
        &root.join("report.sarif"),
        &report.sarif(),
        MAX_REPORT_BYTES,
    )?;
    let summary = report.comment_markdown();
    io::atomic_bytes(&root.join("report.md"), summary.as_bytes())?;
    io::atomic_bytes(&root.join("summary.md"), summary.as_bytes())?;
    io::atomic_json(
        &root.join("bundle.json"),
        &Publication::Checkpoint {
            version: VERSION,
            report: digest,
        },
    )
}

pub(in crate::report) fn publish(
    report: &ImpactReport,
    directory: &Path,
) -> Result<(), BundleError> {
    let root = output_root(directory)?;
    let _lock = io::WriterLock::acquire(&root)?;
    // Authoritative compiler evidence survives every later rendering failure.
    let report_digest =
        io::atomic_json_bounded(&root.join("report.json"), report, MAX_REPORT_BYTES)?;
    let bundles = io::directory(&root, Path::new("bundles"))?;
    let stage = tempfile::Builder::new()
        .prefix(".staging-")
        .tempdir_in(&bundles)?;
    let mut artifacts = Artifacts::new(stage.path());
    artifacts.copy("report.json", &root.join("report.json"), &report_digest)?;
    artifacts.bytes("report.md", report.markdown().as_bytes())?;
    artifacts.bytes("summary.md", report.comment_markdown().as_bytes())?;
    artifacts.bytes("index.html", report.html().as_bytes())?;
    artifacts.json("report.sarif", &report.sarif())?;
    let drafts = report.issue_drafts();
    artifacts.bytes("issues/index.md", issue_index(&drafts, "").as_bytes())?;
    artifacts.json("issues/index.json", &drafts)?;
    for draft in &drafts {
        artifacts.bytes(&format!("issues/{}", draft.filename), draft.body.as_bytes())?;
        artifacts.bytes(
            &format!("issues/{}", draft.reproduction_filename),
            draft.reproduction_config.as_bytes(),
        )?;
    }
    for result in &report.downstreams {
        for (phase, build) in [
            ("baseline", &result.baseline),
            ("candidate", &result.candidate),
        ] {
            if let Some(lockfile) = &build.lockfile {
                let path = format!("locks/{}/{phase}.lock", presentation::result_id(result));
                if !lockfile.verify()
                    || build
                        .provenance
                        .lock_fingerprint
                        .as_deref()
                        .is_some_and(|fingerprint| fingerprint != lockfile.sha256)
                {
                    return Err(BundleError::Changed(path.into()));
                }
                artifacts.bytes(&path, lockfile.contents.as_bytes())?;
            }
        }
    }
    let manifest = BundleManifest::from_artifacts(artifacts.inventory)?;
    manifest.validate()?;
    io::atomic_json(&stage.path().join("manifest.json"), &manifest)?;
    let destination = root.join(manifest.directory());
    if destination.exists() {
        verify_generation(&root, &manifest)?;
    } else {
        fs::rename(stage.path(), &destination)?;
        io::sync_directory(&bundles)?;
    }
    // Compatibility aliases are individually replaced. Every human view links
    // to its immutable generation, even if a later root alias fails to publish.
    let prefix = format!("{}/", manifest.directory().to_string_lossy());
    io::atomic_bytes(
        &root.join("report.md"),
        markdown::render_with_links(report, false, &prefix).as_bytes(),
    )?;
    io::atomic_bytes(
        &root.join("summary.md"),
        report.comment_markdown().as_bytes(),
    )?;
    io::atomic_json_bounded(
        &root.join("report.sarif"),
        &report.sarif(),
        MAX_REPORT_BYTES,
    )?;
    let landing = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0;url={prefix}index.html\"><title>cargo-impact report</title><h1>Downstream impact report</h1><p><a href=\"{prefix}index.html\">Open the immutable report</a></p><p>Bundle: <code>{}</code>. Verify with <code>cargo impact verify --report-dir .</code>.</p></html>",
        manifest.id
    );
    io::atomic_bytes(&root.join("index.html"), landing.as_bytes())?;
    let issues = io::directory(&root, Path::new("issues"))?;
    super::super::clear_issue_drafts(&root)?;
    for draft in &drafts {
        io::atomic_bytes(&issues.join(&draft.filename), draft.body.as_bytes())?;
        io::atomic_bytes(
            &issues.join(&draft.reproduction_filename),
            draft.reproduction_config.as_bytes(),
        )?;
    }
    io::atomic_bytes(
        &issues.join("index.md"),
        issue_index(&drafts, &format!("../{prefix}issues/")).as_bytes(),
    )?;
    io::atomic_json(&issues.join("index.json"), &drafts)?;
    // This is the commit point. A reader validates its complete generation and
    // checks that the durable root JSON describes the same report.
    io::atomic_json(&root.join("bundle.json"), &Publication::Bundle { manifest })
}

fn output_root(directory: &Path) -> Result<PathBuf, BundleError> {
    fs::create_dir_all(directory)?;
    Ok(directory.canonicalize()?)
}

struct Artifacts<'a> {
    root: &'a Path,
    inventory: BTreeMap<PathBuf, ArtifactDigest>,
    bytes: u64,
}

impl<'a> Artifacts<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            inventory: BTreeMap::new(),
            bytes: 0,
        }
    }

    fn destination(&self, relative: &str) -> Result<PathBuf, BundleError> {
        let relative = Path::new(relative);
        io::relative_path(relative)?;
        if let Some(parent) = relative
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            io::directory(self.root, parent)?;
        }
        Ok(self.root.join(relative))
    }

    fn record(&mut self, relative: &str, digest: ArtifactDigest) -> Result<(), BundleError> {
        if self.inventory.len() >= MAX_ARTIFACTS || self.inventory.contains_key(Path::new(relative))
        {
            return Err(BundleError::InvalidManifest);
        }
        self.bytes = self
            .bytes
            .checked_add(digest.bytes)
            .ok_or(BundleError::TooLarge)?;
        if self.bytes > MAX_BUNDLE_BYTES {
            return Err(BundleError::TooLarge);
        }
        self.inventory.insert(relative.into(), digest);
        Ok(())
    }

    fn bytes(&mut self, relative: &str, bytes: &[u8]) -> Result<(), BundleError> {
        if bytes.len() as u64 > self.limit() {
            return Err(BundleError::TooLarge);
        }
        let destination = self.destination(relative)?;
        io::atomic_bytes(&destination, bytes)?;
        self.record(
            relative,
            ArtifactDigest {
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
            },
        )
    }

    fn json<T: Serialize>(&mut self, relative: &str, value: &T) -> Result<(), BundleError> {
        let destination = self.destination(relative)?;
        let digest = io::atomic_json_bounded(&destination, value, self.limit())?;
        self.record(relative, digest)
    }

    fn copy(
        &mut self,
        relative: &str,
        source: &Path,
        expected: &ArtifactDigest,
    ) -> Result<(), BundleError> {
        if expected.bytes > self.limit() {
            return Err(BundleError::TooLarge);
        }
        let destination = self.destination(relative)?;
        fs::copy(source, &destination)?;
        let digest = io::digest(&destination, self.limit())?;
        if digest != *expected {
            return Err(BundleError::Changed(relative.into()));
        }
        fs::File::open(destination)?.sync_all()?;
        self.record(relative, digest)
    }

    fn limit(&self) -> u64 {
        MAX_REPORT_BYTES.min(MAX_BUNDLE_BYTES.saturating_sub(self.bytes))
    }
}

fn issue_index(drafts: &[IssueDraft], prefix: &str) -> String {
    use std::fmt::Write;
    let mut index = String::from(
        "# Issue drafts\n\nThese are local drafts for human review. No issue has been submitted.\n\n| Consumer | Draft | Reproduction recipe | Issue form |\n| --- | --- | --- | --- |\n",
    );
    for draft in drafts {
        let link = draft
            .composer_url
            .as_ref()
            .map(|url| format!("[Review draft](<{url}>)"))
            .unwrap_or_else(|| "Copy Markdown into your tracker".into());
        let _ = writeln!(
            index,
            "| {} | [{}]({prefix}{}) | [TOML]({prefix}{}) | {} |",
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
    index
}
