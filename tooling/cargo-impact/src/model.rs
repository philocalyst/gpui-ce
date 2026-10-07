use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    time::Duration,
};

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
    /// Wall-clock budget for preparation, gate, discovery and all consumer pairs.
    pub scan_timeout_seconds: u64,
    /// Successful controlled comparisons required when the downstream gate opens.
    pub minimum_exercised: usize,
}

impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            jobs: 2,
            cargo_jobs: 1,
            memory_mib: 2048,
            cpus: 1,
            max_work_bytes: Some(5 * 1024 * 1024 * 1024),
            prune_before_run: false,
            scan_timeout_seconds: 3300,
            minimum_exercised: 1,
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
            || self.scan_timeout_seconds == 0
        {
            return Err(std::io::Error::other(
                "execution requires 1..=32 workers, 1..=256 Cargo jobs, at least 128 MiB per worker, positive CPUs, storage budget and scan timeout",
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
    /// The application's own executable implementing the hidden semver helper.
    /// `None` runs the embedded checker in this trusted host process.
    pub semver_helper: Option<PathBuf>,
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
            semver_helper: None,
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
    pub engine_fingerprint: Option<String>,
    /// Digest of the retained input identity being rebuilt, never cached success.
    pub replay_of: Option<String>,
    /// Recorded outcome for comparison with a fresh replay; inputs can be deterministic
    /// while a build script's behavior is not.
    pub replay_expected: Option<Classification>,
    pub status: RunStatus,
    pub elapsed_ms: u64,
    pub execution: ExecutionOptions,
    pub storage_bytes: u64,
    pub planned_downstreams: usize,
    pub completed_downstreams: usize,
    pub exercised_downstreams: usize,
    /// None for an unfinished run or reports from an older schema.
    pub coverage_sufficient: Option<bool>,
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
    #[serde(default)]
    pub experiment_id: Option<ExperimentId>,
    #[serde(default)]
    pub lifecycle: ExperimentLifecycle,
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
    #[serde(default)]
    pub lockfile: Option<LockfileEvidence>,
    #[serde(default)]
    pub dependency_graph: Option<DependencyGraph>,
    /// Resolved registry/Git patch tables applied before the controlled phase.
    #[serde(default)]
    pub injection_sources: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildProvenance {
    pub rustc: Option<String>,
    pub cargo: Option<String>,
    pub runner_identity: String,
    pub lock_fingerprint: Option<String>,
    pub image_id: Option<String>,
    /// Registry-pullable digest, when Docker has retained one for this image.
    pub image_reference: Option<String>,
}

/// Identity of a configured experiment, independent of its display label.
/// Values are full lowercase SHA-256 digests and safe as artifact path segments.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(try_from = "String", into = "String")]
pub struct ExperimentId(String);

impl ExperimentId {
    pub fn for_spec(
        library: &str,
        spec: &DownstreamSpec,
        recipe: &BuildRecipe,
    ) -> std::io::Result<Self> {
        let mut recipe = recipe.clone();
        recipe.features.sort();
        recipe.features.dedup();
        recipe.packages.sort();
        recipe.packages.dedup();
        let identity = (
            "cargo-impact-experiment-v1",
            library,
            &spec.name,
            &spec.source,
            &spec.manifest,
            recipe,
        );
        serde_json::to_vec(&identity)
            .map(|bytes| Self(crate::source::key(&bytes)))
            .map_err(std::io::Error::other)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ExperimentId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 64
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            Ok(Self(value))
        } else {
            Err("experiment ID must be 64 lowercase hexadecimal characters".into())
        }
    }
}
impl From<ExperimentId> for String {
    fn from(value: ExperimentId) -> Self {
        value.0
    }
}
impl std::fmt::Display for ExperimentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentStatus {
    #[default]
    Unknown,
    Queued,
    Preparing,
    Baseline,
    Candidate,
    Complete,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentStage {
    Scheduling,
    SourcePreparation,
    Baseline,
    Candidate,
    IntegrityCheck,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuildPhase {
    Baseline,
    Candidate,
}

impl BuildPhase {
    pub fn stage(self) -> ExperimentStage {
        match self {
            Self::Baseline => ExperimentStage::Baseline,
            Self::Candidate => ExperimentStage::Candidate,
        }
    }
    pub fn status(self) -> ExperimentStatus {
        match self {
            Self::Baseline => ExperimentStatus::Baseline,
            Self::Candidate => ExperimentStatus::Candidate,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Candidate => "candidate",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExperimentFailure {
    pub stage: ExperimentStage,
    pub cause: HarnessFailure,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExperimentLifecycle {
    pub status: ExperimentStatus,
    pub failure: Option<ExperimentFailure>,
}

impl ExperimentLifecycle {
    pub fn is_finished(&self) -> bool {
        matches!(
            self.status,
            ExperimentStatus::Complete | ExperimentStatus::Cancelled
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LockfileEvidence {
    pub sha256: String,
    pub contents: String,
}

impl LockfileEvidence {
    pub fn verify(&self) -> bool {
        crate::source::key(self.contents.as_bytes()) == self.sha256
    }
}

/// The resolved, feature-filtered Cargo graph used by one build phase.
/// Edges explain dependency paths; matching compiler symptoms alone do not prove causality.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DependencyGraph {
    pub roots: Vec<String>,
    pub selected_library: String,
    pub packages: Vec<DependencyPackage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DependencyPackage {
    pub id: String,
    /// Source-relative identity used when comparing equivalent graphs across work directories.
    pub identity: String,
    pub name: String,
    pub version: String,
    pub source: Option<String>,
    pub manifest: PathBuf,
    pub origin: DiagnosticOrigin,
    pub features: Vec<String>,
    pub dependencies: Vec<String>,
}

impl DependencyGraph {
    pub fn fingerprint(&self) -> Result<String, String> {
        let ids: BTreeMap<_, _> = self
            .packages
            .iter()
            .map(|p| (p.id.as_str(), p.identity.as_str()))
            .collect();
        let identities: BTreeSet<_> = self
            .packages
            .iter()
            .map(|package| package.identity.as_str())
            .collect();
        if ids.len() != self.packages.len() || identities.len() != self.packages.len() {
            return Err("dependency graph contains colliding package identities".into());
        }
        let lookup = |id: &str| {
            ids.get(id)
                .copied()
                .ok_or_else(|| "dependency graph references an absent package".to_owned())
        };
        let mut roots = self
            .roots
            .iter()
            .map(|id| lookup(id))
            .collect::<Result<Vec<_>, _>>()?;
        roots.sort();
        let mut packages = Vec::new();
        for package in &self.packages {
            if ExperimentId::try_from(package.identity.clone()).is_err() {
                return Err("dependency graph lacks a valid source-relative identity".into());
            }
            let expected = serde_json::to_vec(&(
                package.origin,
                &package.name,
                &package.version,
                &package.source,
                &package.manifest,
            ))
            .map_err(|error| error.to_string())?;
            if crate::source::key(&expected) != package.identity {
                return Err(
                    "dependency graph package identity does not match retained source metadata"
                        .into(),
                );
            }
            let mut dependencies = package
                .dependencies
                .iter()
                .map(|id| lookup(id))
                .collect::<Result<Vec<_>, _>>()?;
            dependencies.sort();
            dependencies.dedup();
            let mut features = package.features.clone();
            features.sort();
            features.dedup();
            packages.push((
                &package.identity,
                &package.name,
                &package.version,
                package.origin,
                features,
                dependencies,
            ));
        }
        packages.sort_by(|a, b| a.0.cmp(b.0));
        serde_json::to_vec(&(
            "cargo-impact-graph-v1",
            roots,
            lookup(&self.selected_library)?,
            packages,
        ))
        .map(|bytes| crate::source::key(&bytes))
        .map_err(|error| error.to_string())
    }

    /// One shortest resolved path, bounded by the graph's node count even with cycles.
    pub fn dependency_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        let packages: BTreeMap<_, _> = self.packages.iter().map(|p| (p.id.as_str(), p)).collect();
        if !packages.contains_key(from) || !packages.contains_key(to) {
            return None;
        }
        let mut queue = VecDeque::from([from]);
        let mut visited = BTreeSet::from([from]);
        let mut previous = BTreeMap::new();
        while let Some(current) = queue.pop_front() {
            if current == to {
                let mut path = vec![to.to_owned()];
                let mut current = to;
                while current != from {
                    current = *previous.get(current)?;
                    path.push(current.to_owned());
                }
                path.reverse();
                return Some(path);
            }
            for dependency in &packages.get(current)?.dependencies {
                if packages.contains_key(dependency.as_str()) && visited.insert(dependency.as_str())
                {
                    previous.insert(dependency.as_str(), current);
                    queue.push_back(dependency.as_str());
                }
            }
        }
        None
    }
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
    EvidenceMismatch,
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
