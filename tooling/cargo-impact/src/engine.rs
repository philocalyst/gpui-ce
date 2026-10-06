//! A scan is a series of controlled baseline/candidate experiments, not semver lint enforcement.

use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use fs2::FileExt;
use tempfile::TempDir;
use thiserror::Error;

use crate::{
    cargo,
    discovery::Discovery,
    gate,
    model::{
        BuildResult, Classification, DownstreamResult, DownstreamSpec, GateResult, HarnessFailure,
        ImpactReport, ImpactRequest,
    },
    runner::Builder,
    source,
};

#[derive(Debug, Error)]
pub enum ImpactError {
    #[error("preparing impact run: {0}")]
    Io(#[from] io::Error),
    #[error("embedded semver gate failed: {0}")]
    Semver(String),
}

/// Discovery is lazy: even authenticated API calls are skipped when the gate stays closed.
pub fn analyze_with_discovery(
    request: &ImpactRequest,
    discover: impl FnOnce() -> Result<(Vec<DownstreamSpec>, Discovery), String>,
) -> Result<ImpactReport, ImpactError> {
    let area = WorkArea::open(request.work_dir.as_deref())?;
    let root = area.path();
    let baseline = root.join("upstream/baseline");
    let candidate = root.join("upstream/candidate");
    let baseline_fingerprint = source::snapshot(&request.baseline, &baseline)?;
    let candidate_fingerprint = source::snapshot(&request.candidate, &candidate)?;
    let builder = Builder {
        recipe: &request.recipe,
        root,
        target: root.join("gate-target"),
        timeout: request.timeout,
    };
    let baseline_package = cargo::library_package(
        &cargo::metadata(&builder, &baseline, &baseline.join("Cargo.toml"), true)?,
        &request.library,
    )?;
    let candidate_package = cargo::library_package(
        &cargo::metadata(&builder, &candidate, &candidate.join("Cargo.toml"), true)?,
        &request.library,
    )?;
    let gate = if request.force {
        GateResult {
            ran: true,
            required_bump: None,
            reason: "Downstream checks explicitly forced; the API gate was bypassed.".into(),
            log: String::new(),
        }
    } else {
        gate::compare(
            &request.library,
            (&baseline, &baseline_package),
            (&candidate, &candidate_package),
            &builder,
        )
        .map_err(ImpactError::Semver)?
    };
    let mut report = ImpactReport {
        schema_version: 1,
        library: request.library.clone(),
        baseline_fingerprint: Some(baseline_fingerprint),
        candidate_fingerprint: Some(candidate_fingerprint),
        error: None,
        gate,
        discovery: Discovery::default(),
        downstreams: Vec::new(),
    };
    if !report.gate.ran {
        return Ok(report);
    }
    let (discovered, discovery) = match discover() {
        Ok(result) => result,
        Err(error) => (
            Vec::new(),
            Discovery {
                candidates: Vec::new(),
                notes: vec![format!("Discovery failed: {error}")],
            },
        ),
    };
    report.discovery = discovery;
    // A major version change should test source compatibility, rather than reject every ^old dependency.
    // Normalize only the copied candidate manifest, after semver comparison, never either input tree.
    if baseline_package.version != candidate_package.version {
        cargo::normalize_version(
            candidate_package.manifest_path.as_std_path(),
            &baseline_package.version.to_string(),
        )?;
        report.discovery.notes.push(format!("Candidate version {} was injected as baseline version {} to test source compatibility.", candidate_package.version, baseline_package.version));
    }
    let mut specs = request.downstreams.clone();
    specs.extend(discovered);
    let mut names = std::collections::BTreeSet::new();
    for spec in specs {
        if !names.insert(spec.name.clone()) {
            report.discovery.notes.push(format!("Duplicate downstream name {} omitted; use unique names for multiple configurations.", spec.name));
            continue;
        }
        let recipe = spec.recipe.as_ref().unwrap_or(&request.recipe);
        let mut result = DownstreamResult {
            name: spec.name.clone(),
            source: spec.source.clone(),
            recipe: recipe.clone(),
            revision: None,
            classification: Classification::HarnessFailure,
            baseline: BuildResult::default(),
            candidate: BuildResult::default(),
            message: None,
        };
        if let Err(error) = experiment(
            request,
            &spec,
            root,
            baseline_package.manifest_path.as_std_path(),
            candidate_package.manifest_path.as_std_path(),
            &mut result,
        ) {
            result.message = Some(error.to_string());
            result.classification = Classification::HarnessFailure;
            if result.candidate.failure.is_none() {
                result.candidate.failure = Some(HarnessFailure::Preparation);
            }
        }
        report.downstreams.push(result);
    }
    Ok(report)
}

pub fn analyze(request: &ImpactRequest) -> Result<ImpactReport, ImpactError> {
    analyze_with_discovery(request, || Ok((Vec::new(), Discovery::default())))
}

fn experiment(
    request: &ImpactRequest,
    spec: &DownstreamSpec,
    root: &Path,
    baseline: &Path,
    candidate: &Path,
    result: &mut DownstreamResult,
) -> io::Result<()> {
    source::validate_manifest(&spec.manifest)?;
    let identity = serde_json::to_vec(&(
        &request.library,
        &spec.source,
        &spec.name,
        &spec.manifest,
        &result.recipe,
    ))
    .map_err(io::Error::other)?;
    let key = source::key(&identity);
    let (checkout, revision) = source::checkout(
        &spec.source,
        &root.join("checkouts").join(&key),
        request.timeout,
    )?;
    result.revision = revision;
    let working = root.join("downstreams").join(&key);
    source::snapshot(&checkout, &working)?;
    let builder = Builder {
        recipe: &result.recipe.clone(),
        root,
        target: root.join("targets").join(&key),
        timeout: request.timeout,
    };
    let manifest = working.join(&spec.manifest);
    let metadata = cargo::metadata(&builder, &working, &manifest, true)?;
    let workspace_manifest = metadata
        .workspace_root
        .join("Cargo.toml")
        .into_std_path_buf();
    if !workspace_manifest.starts_with(&working) {
        return Err(io::Error::other(
            "downstream workspace escapes its snapshot",
        ));
    }
    let mut manifests: Vec<_> = metadata
        .packages
        .iter()
        .filter(|p| metadata.workspace_members.contains(&p.id))
        .map(|p| p.manifest_path.clone().into_std_path_buf())
        .collect();
    manifests.push(workspace_manifest.clone());
    result.baseline = build(
        &builder,
        &working,
        &manifest,
        &workspace_manifest,
        &manifests,
        &request.library,
        baseline,
    )?;
    // Restore original manifests and sources to prevent baseline build scripts from modifying candidate inputs.
    let lock = fs::read(workspace_manifest.with_file_name("Cargo.lock")).ok();
    source::snapshot(&checkout, &working)?;
    if let Some(lock) = lock {
        fs::write(workspace_manifest.with_file_name("Cargo.lock"), lock)?;
    }
    result.candidate = build(
        &builder,
        &working,
        &manifest,
        &workspace_manifest,
        &manifests,
        &request.library,
        candidate,
    )?;
    result.classification = classify(&result.baseline, &result.candidate);
    result.message = Some(match result.classification {
        Classification::Compatible => "Both versions compiled with the requested library selected and built.",
        Classification::Regression => "Baseline compiled; candidate introduced compiler errors.",
        Classification::PreExistingFailure => "Baseline already has compiler errors; this comparison cannot establish a regression.",
        Classification::NotExercised => "The requested library was not compiled in this configuration; enable its feature or select the consuming package.",
        Classification::HarnessFailure => "The build environment, dependency resolution, or output limits prevented a reliable comparison; adjust this downstream's recipe and retry.",
    }.into());
    Ok(())
}

fn build(
    builder: &Builder<'_>,
    root: &Path,
    manifest: &Path,
    workspace_manifest: &Path,
    manifests: &[PathBuf],
    library: &str,
    upstream: &Path,
) -> io::Result<BuildResult> {
    cargo::inject(root, workspace_manifest, manifests, library, upstream)?;
    let prepare = || {
        cargo::fetch(builder, root, manifest)?;
        cargo::metadata(builder, root, manifest, false)
    };
    let mut metadata = match prepare() {
        Ok(metadata) => metadata,
        Err(error) => {
            return Ok(BuildResult {
                failure: Some(HarnessFailure::DependencyResolution),
                log: error.to_string(),
                ..BuildResult::default()
            });
        }
    };
    if cargo::patch_resolved_sources(&metadata, workspace_manifest, library, upstream)? {
        metadata = match prepare() {
            Ok(metadata) => metadata,
            Err(error) => {
                return Ok(BuildResult {
                    failure: Some(HarnessFailure::DependencyResolution),
                    log: error.to_string(),
                    ..Default::default()
                });
            }
        };
    }
    let Some(selected) = cargo::selected_library(&metadata, library, upstream) else {
        return Ok(BuildResult {
            failure: metadata
                .packages
                .iter()
                .any(|p| p.name.as_str() == library)
                .then_some(HarnessFailure::DependencyResolution),
            log: "Cargo did not select the requested library source.".into(),
            ..BuildResult::default()
        });
    };
    cargo::check(builder, root, manifest, selected)
}

fn exercised(build: &BuildResult) -> bool {
    build
        .selected_library
        .as_ref()
        .is_some_and(|id| build.compiled_packages.contains(id))
}

fn classify(baseline: &BuildResult, candidate: &BuildResult) -> Classification {
    if baseline.failure.is_some() || candidate.failure.is_some() {
        return Classification::HarnessFailure;
    }
    if !exercised(baseline) {
        return Classification::NotExercised;
    }
    if !baseline.success {
        return Classification::PreExistingFailure;
    }
    if !candidate.success {
        return if candidate
            .diagnostics
            .iter()
            .any(|d| matches!(d.level, cargo_metadata::diagnostic::DiagnosticLevel::Error))
        {
            Classification::Regression
        } else {
            Classification::NotExercised
        };
    }
    if !exercised(candidate) {
        return Classification::NotExercised;
    }
    Classification::Compatible
}

struct WorkArea {
    root: PathBuf,
    _temporary: Option<TempDir>,
    _lock: File,
}

impl WorkArea {
    fn open(path: Option<&Path>) -> io::Result<Self> {
        let temporary = if path.is_none() {
            Some(TempDir::new()?)
        } else {
            None
        };
        let root = path
            .or_else(|| temporary.as_ref().map(TempDir::path))
            .ok_or_else(|| io::Error::other("missing work directory"))?;
        fs::create_dir_all(root)?;
        let root = root.canonicalize()?;
        fs::create_dir_all(root.join("home"))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("scan.lock"))?;
        lock.try_lock_exclusive()
            .map_err(|e| io::Error::other(format!("another scan owns this work directory: {e}")))?;
        Ok(Self {
            root,
            _temporary: temporary,
            _lock: lock,
        })
    }
    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for WorkArea {
    fn drop(&mut self) {
        // Explicitly release the shared open-file-description lock before a retry.
        // Relying on descriptor closure can leave it briefly held by an exiting child.
        let _ = FileExt::unlock(&self._lock);
    }
}
