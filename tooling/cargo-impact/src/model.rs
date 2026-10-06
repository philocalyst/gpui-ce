use std::{path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{discovery::Discovery, runner::BuildRecipe};

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
    pub recipe: BuildRecipe,
    pub classification: Classification,
    pub baseline: BuildResult,
    pub candidate: BuildResult,
    pub message: Option<String>,
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
}

/// Keep rustc's diagnostic tree, suggestions, macro expansions, byte ranges, and snippets.
/// Reusing Cargo's types avoids a lossy parallel diagnostic schema.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompilerDiagnostic {
    pub package_id: String,
    #[serde(flatten)]
    pub diagnostic: cargo_metadata::diagnostic::Diagnostic,
}

impl std::ops::Deref for CompilerDiagnostic {
    type Target = cargo_metadata::diagnostic::Diagnostic;
    fn deref(&self) -> &Self::Target {
        &self.diagnostic
    }
}
