//! A scan is a series of controlled baseline/candidate experiments, not semver lint enforcement.

use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
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
        ImpactReport, ImpactRequest, RunMetadata, RunStatus,
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
    analyze_with_progress(request, discover, |_| Ok(()))
}

/// The coordinator publishes durable observations without sharing the callback
/// with build workers. A failed observer fails the scan rather than losing evidence.
pub fn analyze_with_progress(
    request: &ImpactRequest,
    discover: impl FnOnce() -> Result<(Vec<DownstreamSpec>, Discovery), String>,
    mut progress: impl FnMut(&ImpactReport) -> io::Result<()>,
) -> Result<ImpactReport, ImpactError> {
    request.execution.validate()?;
    if let Some(upstream) = &request.upstream {
        upstream.validate()?;
    }
    let started = Instant::now();
    let scan_deadline = started
        .checked_add(Duration::from_secs(request.execution.scan_timeout_seconds))
        .ok_or_else(|| io::Error::other("scan timeout exceeds the host clock range"))?;
    let area = WorkArea::open(request.work_dir.as_deref())?;
    let root = area.path();
    prepare_storage(root, &request.execution)?;
    let baseline = root.join("upstream/baseline");
    let candidate = root.join("upstream/candidate");
    let baseline_fingerprint =
        snapshot_in(request, root, &request.baseline, &baseline, scan_deadline)?;
    let candidate_fingerprint =
        snapshot_in(request, root, &request.candidate, &candidate, scan_deadline)?;
    // Each gate phase sees only its own working copy. The canonical upstream
    // snapshots are never writable in an untrusted build container.
    let gate_baseline = root.join("gate-sources/baseline");
    let gate_candidate = root.join("gate-sources/candidate");
    snapshot_in(request, root, &baseline, &gate_baseline, scan_deadline)?;
    snapshot_in(request, root, &candidate, &gate_candidate, scan_deadline)?;
    let gate_scope = root.join("gate-worker");
    let images = Mutex::new(std::collections::BTreeMap::new());
    let builder = Builder {
        recipe: &request.recipe,
        root,
        target: root.join("gate-target"),
        timeout: request.timeout,
        deadline: Instant::now()
            .checked_add(request.timeout)
            .ok_or_else(|| io::Error::other("timeout exceeds the host clock range"))?
            .min(scan_deadline),
        scope: &gate_scope,
        execution: &request.execution,
        images: &images,
    };
    let baseline_package = cargo::library_package(
        &cargo::metadata(
            &builder,
            &gate_baseline,
            &gate_baseline.join("Cargo.toml"),
            true,
        )?,
        &request.library,
    )?;
    let candidate_package = cargo::library_package(
        &cargo::metadata(
            &builder,
            &gate_candidate,
            &gate_candidate.join("Cargo.toml"),
            true,
        )?,
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
            (&gate_baseline, &baseline_package),
            (&gate_candidate, &candidate_package),
            &builder,
            request.semver_helper.as_deref(),
        )
        .map_err(ImpactError::Semver)?
    };
    let mut report = ImpactReport {
        schema_version: 1,
        library: request.library.clone(),
        baseline_fingerprint: Some(baseline_fingerprint.clone()),
        candidate_fingerprint: Some(candidate_fingerprint.clone()),
        error: None,
        gate,
        discovery: Discovery::default(),
        downstreams: Vec::new(),
        run: RunMetadata {
            status: RunStatus::Running,
            execution: request.execution.clone(),
            upstream: request.upstream.clone(),
            ..Default::default()
        },
    };
    if !request.recipe.runner.is_isolated()
        && (source::fingerprint(&baseline)? != baseline_fingerprint
            || source::fingerprint(&candidate)? != candidate_fingerprint)
    {
        return Err(io::Error::other(
            "gate execution mutated canonical upstream inputs; this experiment is inconclusive",
        )
        .into());
    }
    if Instant::now() >= scan_deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "scan deadline exceeded during API preparation or analysis",
        )
        .into());
    }
    report.run.storage_bytes = work_bytes(root)?;
    report.run.elapsed_ms = milliseconds(started.elapsed());
    progress(&report)?;
    if !report.gate.ran {
        report.run.status = RunStatus::Complete;
        report.run.coverage_sufficient = Some(true);
        progress(&report)?;
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
    if Instant::now() >= scan_deadline {
        report.error = Some("The scan wall-clock budget was exhausted during discovery; consumers are inconclusive.".into());
    }
    // A major version change should test source compatibility, rather than reject every ^old dependency.
    // Normalize only the copied candidate manifest, after semver comparison, never either input tree.
    if baseline_package.version != candidate_package.version {
        let candidate_manifest = candidate.join(
            candidate_package
                .manifest_path
                .as_std_path()
                .strip_prefix(&gate_candidate)
                .map_err(io::Error::other)?,
        );
        cargo::normalize_version(&candidate_manifest, &baseline_package.version.to_string())?;
        report.discovery.notes.push(format!("Candidate version {} was injected as baseline version {} to test source compatibility.", candidate_package.version, baseline_package.version));
    }
    let mut specs = request.downstreams.clone();
    specs.extend(discovered);
    let mut names = std::collections::BTreeSet::new();
    specs.retain(|spec| {
        if !names.insert(spec.name.clone()) {
            report.discovery.notes.push(format!("Duplicate downstream name {} omitted; use unique names for multiple configurations.", spec.name));
            false
        } else {
            true
        }
    });
    let detect_drift = !request.recipe.runner.is_isolated()
        || specs
            .iter()
            .filter_map(|spec| spec.recipe.as_ref())
            .any(|recipe| !recipe.runner.is_isolated());
    let tested_candidate_fingerprint = source::fingerprint(&candidate)?;
    report.run.planned_downstreams = specs.len();
    progress(&report)?;
    let baseline_manifest = baseline.join(
        baseline_package
            .manifest_path
            .as_std_path()
            .strip_prefix(&gate_baseline)
            .map_err(io::Error::other)?,
    );
    let candidate_manifest = candidate.join(
        candidate_package
            .manifest_path
            .as_std_path()
            .strip_prefix(&gate_candidate)
            .map_err(io::Error::other)?,
    );
    let mut results = std::collections::BTreeMap::new();
    let mut completed_count = 0;
    std::thread::scope(|scope| -> io::Result<()> {
        let workers = request.execution.jobs.min(specs.len());
        let (completed, receiver) = mpsc::channel();
        let mut queues = Vec::new();
        for worker in 0..workers {
            let (sender, tasks) = mpsc::channel::<usize>();
            queues.push(sender);
            let completed = completed.clone();
            let specs = &specs;
            let baseline_manifest = &baseline_manifest;
            let candidate_manifest = &candidate_manifest;
            let images = &images;
            scope.spawn(move || {
                for index in tasks {
                    let spec = &specs[index];
                    let pair_started = Instant::now();
                    let mut result = empty_result(request, spec);
                    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        experiment(
                            request,
                            spec,
                            root,
                            (baseline_manifest, candidate_manifest),
                            images,
                            scan_deadline,
                            &mut result,
                        )
                    }));
                    if let Err(error) = attempt.unwrap_or_else(|_| {
                        Err(io::Error::other(
                            "build worker panicked; experiment is inconclusive",
                        ))
                    }) {
                        result.message = Some(error.to_string());
                        result.candidate.failure = Some(match error.kind() {
                            io::ErrorKind::TimedOut => HarnessFailure::Timeout,
                            io::ErrorKind::StorageFull => HarnessFailure::StorageLimit,
                            _ => HarnessFailure::Preparation,
                        });
                    }
                    result.elapsed_ms = milliseconds(pair_started.elapsed());
                    if completed.send((worker, index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(completed);
        let mut next = 0;
        for queue in &queues {
            queue.send(next).map_err(io::Error::other)?;
            next += 1;
        }
        let mut active = workers;
        while active > 0 {
            let (worker, index, result) = receiver.recv().map_err(io::Error::other)?;
            active -= 1;
            completed_count += 1;
            results.insert(index, result);
            report.downstreams = results.values().cloned().collect();
            report.run.completed_downstreams = completed_count;
            report.run.exercised_downstreams = exercised_count(&report);
            report.run.elapsed_ms = milliseconds(started.elapsed());
            report.run.storage_bytes = work_bytes(root)?;
            let inputs_mutated = detect_drift
                && (source::fingerprint(&baseline)? != baseline_fingerprint
                    || source::fingerprint(&candidate)? != tested_candidate_fingerprint);
            if inputs_mutated {
                for result in results.values_mut() {
                    result.classification = Classification::HarnessFailure;
                    result.candidate.failure = Some(HarnessFailure::InputMutation);
                    result.message = Some("Canonical upstream source changed during host execution; results are inconclusive. Use an isolated recipe and retry.".into());
                }
                report.downstreams = results.values().cloned().collect();
                report.run.exercised_downstreams = exercised_count(&report);
            }
            progress(&report)?;
            let exceeded = request
                .execution
                .max_work_bytes
                .is_some_and(|budget| report.run.storage_bytes > budget);
            let expired = Instant::now() >= scan_deadline;
            if expired {
                report.error = Some("The scan wall-clock budget was exhausted; queued or unfinished consumers are inconclusive.".into());
            }
            if exceeded || inputs_mutated || expired {
                while next < specs.len() {
                    let mut omitted = empty_result(request, &specs[next]);
                    omitted.candidate.failure = Some(if expired {
                        HarnessFailure::Timeout
                    } else if inputs_mutated {
                        HarnessFailure::InputMutation
                    } else {
                        HarnessFailure::StorageLimit
                    });
                    omitted.message = Some(if expired { "Scan deadline exceeded; this queued experiment was not started. Increase execution.scan_timeout_seconds or narrow the consumer list." } else if inputs_mutated { "Canonical upstream inputs changed; queued experiments were not started. Use an isolated recipe and retry." } else { "Work directory exceeds its storage budget; no new workers were started. Prune managed build caches and retry." }.into());
                    results.insert(next, omitted);
                    next += 1;
                }
            } else if next < specs.len() {
                queues[worker].send(next).map_err(io::Error::other)?;
                next += 1;
                active += 1;
            }
        }
        drop(queues);
        Ok(())
    })?;
    report.downstreams = results.into_values().collect();
    report.run.elapsed_ms = milliseconds(started.elapsed());
    report.run.storage_bytes = work_bytes(root)?;
    report.run.status = RunStatus::Complete;
    report.run.exercised_downstreams = exercised_count(&report);
    let sufficient = report.run.exercised_downstreams >= request.execution.minimum_exercised;
    report.run.coverage_sufficient = Some(sufficient);
    if !sufficient {
        report.discovery.notes.push(format!("Coverage is inconclusive: {} consumer comparisons exercised the library; at least {} were required. Select a consuming package or feature, expand discovery, or adjust execution.minimum_exercised explicitly.", report.run.exercised_downstreams, request.execution.minimum_exercised));
    }
    progress(&report)?;
    Ok(report)
}

fn milliseconds(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn exercised_count(report: &ImpactReport) -> usize {
    report
        .downstreams
        .iter()
        .filter(|result| {
            matches!(
                result.classification,
                Classification::Compatible | Classification::Regression
            )
        })
        .count()
}

fn empty_result(request: &ImpactRequest, spec: &DownstreamSpec) -> DownstreamResult {
    DownstreamResult {
        name: spec.name.clone(),
        source: spec.source.clone(),
        manifest: spec.manifest.clone(),
        source_fingerprint: None,
        recipe: spec.recipe.as_ref().unwrap_or(&request.recipe).clone(),
        revision: None,
        classification: Classification::HarnessFailure,
        baseline: Default::default(),
        candidate: Default::default(),
        message: None,
        elapsed_ms: 0,
    }
}

pub fn analyze(request: &ImpactRequest) -> Result<ImpactReport, ImpactError> {
    analyze_with_discovery(request, || Ok((Vec::new(), Discovery::default())))
}

fn experiment(
    request: &ImpactRequest,
    spec: &DownstreamSpec,
    root: &Path,
    upstream: (&Path, &Path),
    images: &Mutex<std::collections::BTreeMap<String, String>>,
    scan_deadline: Instant,
    result: &mut DownstreamResult,
) -> io::Result<()> {
    let (baseline, candidate) = upstream;
    let deadline = Instant::now()
        .checked_add(request.timeout)
        .ok_or_else(|| io::Error::other("timeout exceeds the host clock range"))?
        .min(scan_deadline);
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
        deadline.saturating_duration_since(Instant::now()),
        root,
        request.execution.max_work_bytes,
    )?;
    result.revision = revision;
    let working = root.join("downstreams").join(&key);
    result.source_fingerprint = Some(snapshot_in(request, root, &checkout, &working, deadline)?);
    let baseline_scope = root.join("workers").join(&key).join("baseline");
    let candidate_scope = root.join("workers").join(&key).join("candidate");
    let builder = Builder {
        recipe: &result.recipe.clone(),
        root,
        target: root.join("targets").join(&key).join("baseline"),
        timeout: request.timeout,
        deadline,
        scope: &baseline_scope,
        execution: &request.execution,
        images,
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
    let workspace = ConsumerWorkspace {
        root: &working,
        original: &checkout,
        manifest: &manifest,
        workspace_manifest: &workspace_manifest,
        manifests: &manifests,
    };
    result.baseline = build(&builder, &workspace, &request.library, baseline)?;
    // Restore original manifests and sources to prevent baseline build scripts from modifying candidate inputs.
    let lock = fs::read(workspace_manifest.with_file_name("Cargo.lock")).ok();
    snapshot_in(request, root, &checkout, &working, deadline)?;
    if let Some(lock) = lock {
        fs::write(workspace_manifest.with_file_name("Cargo.lock"), lock)?;
    }
    let candidate_builder = Builder {
        target: root.join("targets").join(&key).join("candidate"),
        scope: &candidate_scope,
        ..builder
    };
    result.candidate = build(&candidate_builder, &workspace, &request.library, candidate)?;
    result.classification = classify(&result.baseline, &result.candidate);
    result.message = Some(match result.classification {
        Classification::Compatible => "Both versions compiled with the requested library selected and built.",
        Classification::Regression => "Baseline compiled; candidate introduced compiler errors.",
        Classification::PreExistingFailure => "Baseline already has compiler errors; this comparison cannot establish a regression.",
        Classification::NotExercised => "The requested library was not compiled in this configuration; enable its feature or select the consuming package.",
        Classification::HarnessFailure if result.baseline.failure == Some(HarnessFailure::LibraryCompilation) || result.candidate.failure == Some(HarnessFailure::LibraryCompilation) => "The injected library failed to compile in this configuration; fix its build before attributing errors to downstream code.",
        Classification::HarnessFailure => "The build environment, dependency resolution, or output limits prevented a reliable comparison; adjust this downstream's recipe and retry.",
    }.into());
    Ok(())
}

struct ConsumerWorkspace<'a> {
    root: &'a Path,
    original: &'a Path,
    manifest: &'a Path,
    workspace_manifest: &'a Path,
    manifests: &'a [PathBuf],
}

fn build(
    builder: &Builder<'_>,
    workspace: &ConsumerWorkspace<'_>,
    library: &str,
    upstream: &Path,
) -> io::Result<BuildResult> {
    let ConsumerWorkspace {
        root,
        original,
        manifest,
        workspace_manifest,
        manifests,
    } = workspace;
    cargo::inject(root, workspace_manifest, manifests, library, upstream)?;
    let prepare = || {
        cargo::fetch(builder, root, manifest)?;
        cargo::metadata(builder, root, manifest, false)
    };
    let mut metadata = match prepare() {
        Ok(metadata) => metadata,
        Err(error) => {
            return Ok(BuildResult {
                failure: Some(match error.kind() {
                    io::ErrorKind::TimedOut => HarnessFailure::Timeout,
                    io::ErrorKind::StorageFull => HarnessFailure::StorageLimit,
                    _ => HarnessFailure::DependencyResolution,
                }),
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
                    failure: Some(match error.kind() {
                        io::ErrorKind::TimedOut => HarnessFailure::Timeout,
                        io::ErrorKind::StorageFull => HarnessFailure::StorageLimit,
                        _ => HarnessFailure::DependencyResolution,
                    }),
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
    cargo::check(builder, root, manifest, selected, &metadata, original)
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
        let marker = root.join("cargo-impact-work-v1");
        if fs::symlink_metadata(&marker).is_ok_and(|m| m.is_symlink())
            || fs::symlink_metadata(root.join("scan.lock")).is_ok_and(|m| m.is_symlink())
        {
            return Err(io::Error::other(
                "work directory marker and lock must not be symlinks",
            ));
        }
        if !marker.exists() {
            // Accept the previous version's known managed layout, but never mark
            // a repository or arbitrary user directory as safe to prune.
            for entry in fs::read_dir(&root)? {
                let name = entry?.file_name();
                if ![
                    "home",
                    "scan.lock",
                    "upstream",
                    "downstreams",
                    "checkouts",
                    "targets",
                    "cache",
                    "gate-target",
                    "gate-worker",
                    "gate-sources",
                    "workers",
                ]
                .iter()
                .any(|allowed| name == *allowed)
                {
                    return Err(io::Error::other(
                        "choose a dedicated work directory: this directory contains data not owned by cargo-impact",
                    ));
                }
            }
            fs::write(marker, b"cargo-impact managed work directory v1\n")?;
        } else if fs::read(&marker)? != b"cargo-impact managed work directory v1\n" {
            return Err(io::Error::other(
                "invalid cargo-impact work directory marker",
            ));
        }
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

/// Count only this managed tree. Symlinks are never followed into user files.
pub(crate) fn work_bytes(root: &Path) -> io::Result<u64> {
    let mut pending = vec![root.to_owned()];
    let mut bytes = 0u64;
    while let Some(path) = pending.pop() {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.is_dir() {
            let entries = match fs::read_dir(path) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                match entry {
                    Ok(entry) => pending.push(entry.path()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                }
            }
        } else {
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Ok(bytes)
}

fn prepare_storage(root: &Path, execution: &crate::model::ExecutionOptions) -> io::Result<()> {
    // A lock is held and no workers have started. Pruning never races live builds.
    if execution.prune_before_run {
        for directory in [
            "targets",
            "workers",
            "gate-target",
            "gate-worker",
            "gate-sources",
            "cache",
        ] {
            let path = root.join(directory);
            if fs::symlink_metadata(&path).is_ok() {
                fs::remove_dir_all(path)?;
            }
        }
    }
    let bytes = work_bytes(root)?;
    if execution
        .max_work_bytes
        .is_some_and(|budget| bytes > budget)
    {
        return Err(io::Error::other(format!(
            "work directory uses {bytes} bytes and exceeds the configured storage budget; enable execution.prune_before_run or choose a new work directory"
        )));
    }
    Ok(())
}

fn snapshot_in(
    request: &ImpactRequest,
    root: &Path,
    source: &Path,
    destination: &Path,
    deadline: Instant,
) -> io::Result<String> {
    crate::runner::safe_directory(root, destination)?;
    let available = request
        .execution
        .max_work_bytes
        .map(|budget| work_bytes(root).map(|used| budget.saturating_sub(used)))
        .transpose()?
        .unwrap_or(2 * 1024 * 1024 * 1024);
    crate::source::snapshot_before(
        source,
        destination,
        available,
        Some(deadline),
        request
            .execution
            .max_work_bytes
            .map(|budget| (root, budget)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_accounting_tolerates_parallel_snapshot_replacement() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().to_owned();
        std::thread::scope(|scope| {
            let root = &root;
            scope.spawn(move || {
                for _ in 0..200 {
                    let directory = root.join("changing/nested");
                    fs::create_dir_all(&directory).unwrap();
                    fs::write(directory.join("file"), "temporary snapshot").unwrap();
                    fs::remove_dir_all(root.join("changing")).unwrap();
                }
            });
            for _ in 0..500 {
                work_bytes(root).unwrap();
            }
        });
    }
}
