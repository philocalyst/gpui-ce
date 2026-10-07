//! A configured experiment owns one sequential pair. Workers emit evidence by
//! value; the coordinator alone owns the evolving downstream result.

use super::{ScanContext, preparation::PreparedScan, storage::snapshot_in};
use crate::{
    cargo,
    model::{
        BuildPhase, BuildResult, Classification, DownstreamResult, DownstreamSpec,
        ExperimentFailure, ExperimentId, ExperimentLifecycle, ExperimentStage, ExperimentStatus,
        HarnessFailure,
    },
    runner::{BuildRecipe, Builder},
    source,
};
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Instant,
};

pub(super) struct ExperimentPlan {
    pub id: ExperimentId,
    pub spec: DownstreamSpec,
    pub recipe: BuildRecipe,
}

pub(super) fn plans(
    context: &ScanContext<'_>,
    specs: impl IntoIterator<Item = DownstreamSpec>,
    notes: &mut Vec<String>,
) -> io::Result<Vec<ExperimentPlan>> {
    let mut seen = BTreeSet::new();
    let mut plans = Vec::new();
    for spec in specs {
        let recipe = spec
            .recipe
            .as_ref()
            .unwrap_or(&context.request.recipe)
            .clone();
        let id = ExperimentId::for_spec(&context.request.library, &spec, &recipe)?;
        if !seen.insert(id.clone()) {
            notes.push(format!("Duplicate experiment {id} ({}) omitted; source, manifest and effective recipe were identical.", spec.name));
            continue;
        }
        plans.push(ExperimentPlan { id, spec, recipe });
    }
    Ok(plans)
}

impl ExperimentPlan {
    pub fn queued(&self) -> DownstreamResult {
        DownstreamResult {
            experiment_id: Some(self.id.clone()),
            lifecycle: ExperimentLifecycle {
                status: ExperimentStatus::Queued,
                failure: None,
            },
            name: self.spec.name.clone(),
            source: self.spec.source.clone(),
            manifest: self.spec.manifest.clone(),
            source_fingerprint: None,
            recipe: self.recipe.clone(),
            revision: None,
            classification: Classification::HarnessFailure,
            baseline: Default::default(),
            candidate: Default::default(),
            message: None,
            elapsed_ms: 0,
        }
    }
}

pub(super) enum ExperimentEvent {
    Preparing,
    PhaseStarted(BuildPhase),
    Source {
        revision: Option<String>,
        fingerprint: String,
    },
    Phase {
        phase: BuildPhase,
        build: Box<BuildResult>,
        persisted: mpsc::Sender<()>,
    },
}

pub(super) fn execute(
    context: &ScanContext<'_>,
    upstream: &PreparedScan,
    plan: &ExperimentPlan,
    mut emit: impl FnMut(ExperimentEvent) -> io::Result<()>,
) -> Result<(), ExperimentFailure> {
    let mut stage = ExperimentStage::SourcePreparation;
    let result = (|| -> io::Result<Option<ExperimentFailure>> {
        emit(ExperimentEvent::Preparing)?;
        let deadline = Instant::now()
            .checked_add(context.request.timeout)
            .ok_or_else(|| io::Error::other("timeout exceeds the host clock range"))?
            .min(context.deadline);
        source::validate_manifest(&plan.spec.manifest)?;
        let key = plan.id.as_str();
        if context
            .replay
            .is_some_and(|replay| replay.identity.experiment_id != plan.id)
        {
            return Ok(Some(replay_mismatch(
                stage,
                "configured experiment identity differs from retained evidence",
            )));
        }
        if context.replay.is_some() {
            super::storage::prepare_fresh_targets(context.root, &plan.id)?;
        }
        let source = context
            .replay
            .map(|replay| &replay.checkout)
            .unwrap_or(&plan.spec.source);
        let (checkout, revision) = source::checkout(
            source,
            &context.root.join("checkouts").join(key),
            deadline.saturating_duration_since(Instant::now()),
            context.root,
            context.request.execution.max_work_bytes,
        )?;
        let working = context.root.join("downstreams").join(key);
        let fingerprint =
            snapshot_in(context.request, context.root, &checkout, &working, deadline)?;
        emit(ExperimentEvent::Source {
            revision,
            fingerprint: fingerprint.clone(),
        })?;
        if context
            .replay
            .is_some_and(|replay| replay.identity.consumer_source != fingerprint)
        {
            return Ok(Some(replay_mismatch(
                stage,
                "consumer source differs from retained evidence",
            )));
        }
        let scope = context.root.join("workers").join(key).join("baseline");
        let builder = Builder {
            recipe: &plan.recipe,
            root: context.root,
            target: context.root.join("targets").join(key).join("baseline"),
            scope: &scope,
            timeout: context.request.timeout,
            deadline,
            execution: &context.request.execution,
            images: context.images,
        };
        let manifest = working.join(&plan.spec.manifest);
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
            root: working,
            original: checkout,
            manifest,
            workspace_manifest,
            manifests,
        };
        stage = ExperimentStage::Baseline;
        emit(ExperimentEvent::PhaseStarted(BuildPhase::Baseline))?;
        let baseline = build(
            &builder,
            &workspace,
            &context.request.library,
            &upstream.baseline.manifest,
            context.replay.map(|replay| (BuildPhase::Baseline, replay)),
        )?;
        // Retain the pre-build lock, rather than trusting a build script's mutable
        // post-build lock, when resetting the workspace for the candidate phase.
        let lock = baseline.lockfile.as_ref().map(|lock| lock.contents.clone());
        let failure = phase_failure(stage, &baseline);
        checkpoint_phase(&mut emit, BuildPhase::Baseline, baseline, deadline)?;
        if failure.is_some() {
            return Ok(failure);
        }
        stage = ExperimentStage::Candidate;
        emit(ExperimentEvent::PhaseStarted(BuildPhase::Candidate))?;
        snapshot_in(
            context.request,
            context.root,
            &workspace.original,
            &workspace.root,
            deadline,
        )?;
        if let Some(lock) = lock {
            write_lock(
                &workspace.workspace_manifest.with_file_name("Cargo.lock"),
                &lock,
            )?;
        }
        let candidate_scope = context.root.join("workers").join(key).join("candidate");
        let candidate_builder = Builder {
            target: context.root.join("targets").join(key).join("candidate"),
            scope: &candidate_scope,
            ..builder
        };
        let candidate = build(
            &candidate_builder,
            &workspace,
            &context.request.library,
            &upstream.candidate.manifest,
            context.replay.map(|replay| (BuildPhase::Candidate, replay)),
        )?;
        let failure = phase_failure(stage, &candidate);
        checkpoint_phase(&mut emit, BuildPhase::Candidate, candidate, deadline)?;
        Ok(failure)
    })();
    match result {
        Ok(None) => Ok(()),
        Ok(Some(failure)) => Err(failure),
        Err(error) => Err(failure(stage, error)),
    }
}

fn checkpoint_phase(
    emit: &mut impl FnMut(ExperimentEvent) -> io::Result<()>,
    phase: BuildPhase,
    build: BuildResult,
    deadline: Instant,
) -> io::Result<()> {
    let (persisted, acknowledgement) = mpsc::channel();
    emit(ExperimentEvent::Phase {
        phase,
        build: Box::new(build),
        persisted,
    })?;
    acknowledgement
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => io::Error::new(
                io::ErrorKind::TimedOut,
                "consumer deadline exceeded while checkpointing phase evidence",
            ),
            mpsc::RecvTimeoutError::Disconnected => io::Error::new(
                io::ErrorKind::BrokenPipe,
                "report observer failed before acknowledging retained phase evidence",
            ),
        })
}

pub(super) fn failure(stage: ExperimentStage, error: io::Error) -> ExperimentFailure {
    ExperimentFailure {
        stage,
        cause: match error.kind() {
            io::ErrorKind::TimedOut => HarnessFailure::Timeout,
            io::ErrorKind::StorageFull => HarnessFailure::StorageLimit,
            _ => HarnessFailure::Preparation,
        },
        message: error.to_string(),
    }
}

fn phase_failure(stage: ExperimentStage, build: &BuildResult) -> Option<ExperimentFailure> {
    build.failure.map(|cause| ExperimentFailure {
        stage,
        cause,
        message: build.log.chars().take(2000).collect(),
    })
}

fn replay_mismatch(stage: ExperimentStage, message: &str) -> ExperimentFailure {
    ExperimentFailure {
        stage,
        cause: HarnessFailure::EvidenceMismatch,
        message: message.into(),
    }
}

pub(super) fn finish(result: &mut DownstreamResult, failure: Option<ExperimentFailure>) {
    result.lifecycle = ExperimentLifecycle {
        status: ExperimentStatus::Complete,
        failure,
    };
    result.classification = if result.lifecycle.failure.is_some() {
        Classification::HarnessFailure
    } else {
        classify(&result.baseline, &result.candidate)
    };
    result.message = Some(if let Some(failure) = &result.lifecycle.failure {
        if failure.cause == HarnessFailure::LibraryCompilation {
            format!(
                "{:?}: The injected library failed to compile in this configuration; fix its build before attributing errors to downstream code. {}",
                failure.stage, failure.message
            )
        } else {
            format!("{:?}: {}", failure.stage, failure.message)
        }
    } else {
        outcome_message(result.classification).into()
    });
}

fn outcome_message(classification: Classification) -> &'static str {
    match classification {
        Classification::Compatible => {
            "Both versions compiled with the requested library selected and built."
        }
        Classification::Regression => "Baseline compiled; candidate introduced compiler errors.",
        Classification::PreExistingFailure => {
            "Baseline already has compiler errors; this comparison cannot establish a regression."
        }
        Classification::NotExercised => {
            "The requested library was not compiled in this configuration; enable its feature or select the consuming package."
        }
        Classification::HarnessFailure => {
            "The build environment prevented a controlled comparison; inspect the recorded phase failure and retry."
        }
    }
}

struct ConsumerWorkspace {
    root: PathBuf,
    original: PathBuf,
    manifest: PathBuf,
    workspace_manifest: PathBuf,
    manifests: Vec<PathBuf>,
}

fn build(
    builder: &Builder<'_>,
    workspace: &ConsumerWorkspace,
    library: &str,
    upstream: &Path,
    replay: Option<(BuildPhase, &crate::replay::ReplayEvidence)>,
) -> io::Result<BuildResult> {
    let ConsumerWorkspace {
        root,
        original,
        manifest,
        workspace_manifest,
        manifests,
    } = workspace;
    if let Some((phase, evidence)) = replay {
        write_lock(
            &workspace_manifest.with_file_name("Cargo.lock"),
            &evidence.phase(phase).1.contents,
        )?;
    }
    cargo::inject(root, workspace_manifest, manifests, library, upstream)?;
    let mut injection_sources = replay
        .map(|(phase, evidence)| evidence.phase(phase).0.injection_sources.clone())
        .unwrap_or_default();
    cargo::apply_source_patches(workspace_manifest, &injection_sources, library, upstream)?;
    let prepare = || {
        if replay.is_some() {
            cargo::fetch_with_lock(builder, root, manifest, true)?;
            cargo::metadata_with_lock(builder, root, manifest, false, true)
        } else {
            cargo::fetch(builder, root, manifest)?;
            cargo::metadata(builder, root, manifest, false)
        }
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
    let additional_sources =
        cargo::patch_resolved_sources(&metadata, workspace_manifest, library, upstream)?;
    if !additional_sources.is_empty() {
        injection_sources.extend(additional_sources);
        injection_sources.sort();
        injection_sources.dedup();
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
    let resolved = cargo::ResolvedBuild {
        selected,
        metadata,
        injection_sources,
    };
    cargo::check(
        builder,
        root,
        manifest,
        &resolved,
        original,
        replay.map(|(phase, evidence)| (phase, evidence.phase(phase).0, library)),
    )
}

fn write_lock(path: &Path, contents: &str) -> io::Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.is_file() || metadata.is_symlink())
    {
        return Err(io::Error::other(
            "phase Cargo.lock must be a regular file without symlinks",
        ));
    }
    fs::write(path, contents)
}

fn exercised(build: &BuildResult) -> bool {
    build
        .selected_library
        .as_ref()
        .is_some_and(|id| build.compiled_packages.contains(id))
}

pub(crate) fn classify(baseline: &BuildResult, candidate: &BuildResult) -> Classification {
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
