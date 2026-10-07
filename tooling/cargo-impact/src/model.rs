use std::{path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{discovery::Discovery, runner::BuildRecipe};

/// Caller-supplied revision context; snapshot fingerprints remain the tested evidence.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpstreamRevisions {
    pub repository: Option<crate::forge::Repository>,
    pub baseline_sha: Option<String>,
    pub candidate_sha: Option<String>,
}

impl UpstreamRevisions {
    pub(crate) fn validate(&self) -> std::io::Result<()> {
        if [&self.baseline_sha, &self.candidate_sha]
            .into_iter()
            .flatten()
            .any(|sha| !matches!(sha.len(), 40 | 64) || !sha.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(std::io::Error::other(
                "upstream revisions must be full 40- or 64-character Git commit IDs",
            ));
        }
        Ok(())
    }
}

/// Bounds apply to a scan's workers; a pair always uses one worker sequentially.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionOptions {
    pub jobs: usize,
    pub cargo_jobs: usize,
    pub memory_mib: u64,
    pub cpus: u16,
    pub max_work_bytes: Option<u64>,
    pub prune_before_run: bool,
}

impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            jobs: 2,
            cargo_jobs: 1,
            memory_mib: 2048,
            cpus: 1,
            max_work_bytes: Some(20 * 1024 * 1024 * 1024),
            prune_before_run: false,
        }
    }
}

impl ExecutionOptions {
    pub(crate) fn validate(&self) -> std::io::Result<()> {
        if !(1..=32).contains(&self.jobs)
            || !(1..=256).contains(&self.cargo_jobs)
            || self.memory_mib < 128
            || self.cpus == 0
            || self.max_work_bytes == Some(0)
        {
            return Err(std::io::Error::other(
                "execution requires 1..=32 workers, 1..=256 Cargo jobs, at least 128 MiB per worker, positive CPUs and storage budget",
            ));
        }
        Ok(())
    }
}

/// The same recipe and downstream revision are used for both halves of an experiment.
#[derive(Clone, Debug)]
pub struct ImpactRequest {
    pub library: String,
    pub baseline: PathBuf,
    pub candidate: PathBuf,
    pub downstreams: Vec<DownstreamSpec>,
    pub work_dir: Option<PathBuf>,
    pub timeout: Duration,
    pub recipe: BuildRecipe,
    pub force: bool,
    pub execution: ExecutionOptions,
    pub upstream: Option<UpstreamRevisions>,
}

impl ImpactRequest {
    pub fn new(
        library: impl Into<String>,
        baseline: impl Into<PathBuf>,
        candidate: impl Into<PathBuf>,
    ) -> Self {
        Self {
            library: library.into(),
            baseline: baseline.into(),
            candidate: candidate.into(),
            downstreams: Vec::new(),
            work_dir: None,
            timeout: Duration::from_secs(1800),
            recipe: BuildRecipe::default(),
            force: false,
            execution: ExecutionOptions::default(),
            upstream: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownstreamSpec {
    pub name: String,
    pub source: DownstreamSource,
    #[serde(default = "default_manifest")]
    pub manifest: PathBuf,
    /// Overrides are explicit complete recipes and apply on the next attempt.
    #[serde(default)]
    pub recipe: Option<BuildRecipe>,
}

fn default_manifest() -> PathBuf {
    "Cargo.toml".into()
}

impl DownstreamSpec {
    pub fn local(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            source: DownstreamSource::Local { path: path.into() },
            manifest: default_manifest(),
            recipe: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DownstreamSource {
    Local {
        path: PathBuf,
    },
    Git {
        url: String,
        #[serde(default = "default_revision")]
        revision: String,
        #[serde(default)]
        forge: Option<crate::forge::Forge>,
    },
}

fn default_revision() -> String {
    "HEAD".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImpactReport {
    pub schema_version: u32,
    pub library: String,
    pub baseline_fingerprint: Option<String>,
    pub candidate_fingerprint: Option<String>,
    pub error: Option<String>,
    pub gate: GateResult,
    pub discovery: Discovery,
    pub downstreams: Vec<DownstreamResult>,
    #[serde(default)]
    pub run: RunMetadata,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    #[default]
    Complete,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunMetadata {
    pub status: RunStatus,
    pub elapsed_ms: u64,
    pub execution: ExecutionOptions,
    pub storage_bytes: u64,
    pub planned_downstreams: usize,
    pub completed_downstreams: usize,
    pub upstream: Option<UpstreamRevisions>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SemverBump {
    Patch,
    Minor,
    Major,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateResult {
    /// Whether the gate decision calls for downstream experiments, including forced runs.
    /// A false value is a skip decision only when the report has no fatal error.
    pub ran: bool,
    pub required_bump: Option<SemverBump>,
    pub reason: String,
    /// Embedded checker output, useful for linking API changes to compiler diagnostics.
    pub log: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownstreamResult {
    pub name: String,
    pub revision: Option<String>,
    pub source: DownstreamSource,
    #[serde(default = "default_manifest")]
    pub manifest: PathBuf,
    #[serde(default)]
    pub source_fingerprint: Option<String>,
    pub recipe: BuildRecipe,
    pub classification: Classification,
    pub baseline: BuildResult,
    pub candidate: BuildResult,
    pub message: Option<String>,
    #[serde(default)]
    pub elapsed_ms: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Compatible,
    Regression,
    PreExistingFailure,
    HarnessFailure,
    NotExercised,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BuildResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub diagnostics: Vec<CompilerDiagnostic>,
    pub log: String,
    pub log_truncated: bool,
    pub diagnostics_truncated: bool,
    pub compiled_packages: Vec<String>,
    pub selected_library: Option<String>,
    pub failure: Option<HarnessFailure>,
    #[serde(default)]
    pub provenance: BuildProvenance,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildProvenance {
    pub rustc: Option<String>,
    pub cargo: Option<String>,
    pub runner_identity: String,
    pub lock_fingerprint: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HarnessFailure {
    Preparation,
    DependencyResolution,
    Timeout,
    OutputLimit,
    InternalCompilerError,
    Environment,
    LibraryCompilation,
    StorageLimit,
    InputMutation,
}

/// Keep rustc's diagnostic tree, suggestions, macro expansions, byte ranges, and snippets.
/// Reusing Cargo's types avoids a lossy parallel diagnostic schema.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompilerDiagnostic {
    pub package_id: String,
    #[serde(default)]
    pub target: Option<cargo_metadata::Target>,
    #[serde(default)]
    pub package: Option<DiagnosticPackage>,
    /// Original rustc filenames mapped to verified files in the downstream repository.
    /// Generated files and sources from other repositories deliberately have no mapping.
    #[serde(default)]
    pub source_files: std::collections::BTreeMap<String, PathBuf>,
    #[serde(flatten)]
    pub diagnostic: cargo_metadata::diagnostic::Diagnostic,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticPackage {
    pub name: String,
    pub manifest: PathBuf,
    pub origin: DiagnosticOrigin,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOrigin {
    Downstream,
    Library,
    Dependency,
}

impl std::ops::Deref for CompilerDiagnostic {
    type Target = cargo_metadata::diagnostic::Diagnostic;
    fn deref(&self) -> &Self::Target {
        &self.diagnostic
    }
}
