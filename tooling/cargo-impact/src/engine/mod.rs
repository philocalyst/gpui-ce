//! Public scan orchestration. Preparation, execution, scheduling and storage
//! each own one boundary and communicate through typed plans and worker events.

mod experiment;
mod preparation;
mod scheduler;
mod storage;

use std::{
    collections::BTreeMap,
    io,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use thiserror::Error;

use crate::{
    discovery::Discovery,
    model::{DownstreamSpec, ImpactReport, ImpactRequest, RunMetadata, RunStatus},
};
pub(crate) use experiment::classify;
use preparation::PreparedScan;
pub(crate) use storage::work_bytes;
use storage::{WorkArea, prepare_storage, validate_inputs};

#[derive(Debug, Error)]
pub enum ImpactError {
    #[error("preparing impact run: {0}")]
    Io(#[from] io::Error),
    #[error("embedded semver gate failed: {0}")]
    Semver(String),
    #[error("replay evidence: {0}")]
    Replay(#[from] crate::replay::ReplayIneligible),
}

pub(super) struct ScanContext<'a> {
    pub request: &'a ImpactRequest,
    pub root: &'a Path,
    pub deadline: Instant,
    pub images: &'a Mutex<BTreeMap<String, String>>,
    pub replay: Option<&'a crate::replay::ReplayEvidence>,
}

pub(super) fn milliseconds(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

/// Discovery is lazy: no authenticated queries run when the API gate closes.
pub fn analyze_with_discovery(
    request: &ImpactRequest,
    discover: impl FnOnce() -> Result<(Vec<DownstreamSpec>, Discovery), String>,
) -> Result<ImpactReport, ImpactError> {
    analyze_with_progress(request, discover, |_| Ok(()))
}

/// Only the coordinator observes reports. Worker events move retained evidence
/// into its report; no worker shares the callback or clones another pair's logs.
/// Local sources returned by discovery must be outside the managed work directory.
/// Only inputs supplied on the request can be protected before requested pruning;
/// callback sources are validated once discovery returns and before dispatch.
pub fn analyze_with_progress(
    request: &ImpactRequest,
    discover: impl FnOnce() -> Result<(Vec<DownstreamSpec>, Discovery), String>,
    progress: impl FnMut(&ImpactReport) -> io::Result<()>,
) -> Result<ImpactReport, ImpactError> {
    analyze_run(request, discover, progress, None)
}

pub fn analyze_replay(request: &crate::replay::ReplayRequest) -> Result<ImpactReport, ImpactError> {
    analyze_replay_with_progress(request, |_| Ok(()))
}

/// Rebuild exactly one experiment; retained success is never reused.
pub fn analyze_replay_with_progress(
    replay: &crate::replay::ReplayRequest,
    progress: impl FnMut(&ImpactReport) -> io::Result<()>,
) -> Result<ImpactReport, ImpactError> {
    let request = replay.impact_request();
    analyze_run(
        &request,
        || Ok((Vec::new(), Discovery::default())),
        progress,
        Some(&replay.evidence),
    )
}

fn analyze_run(
    request: &ImpactRequest,
    discover: impl FnOnce() -> Result<(Vec<DownstreamSpec>, Discovery), String>,
    mut progress: impl FnMut(&ImpactReport) -> io::Result<()>,
    replay: Option<&crate::replay::ReplayEvidence>,
) -> Result<ImpactReport, ImpactError> {
    request.execution.validate()?;
    if let Some(upstream) = &request.upstream {
        upstream.validate()?;
    }
    let started = Instant::now();
    let deadline = started
        .checked_add(Duration::from_secs(request.execution.scan_timeout_seconds))
        .ok_or_else(|| io::Error::other("scan timeout exceeds the host clock range"))?;
    let area = WorkArea::open(request.work_dir.as_deref())?;
    validate_inputs(area.path(), request)?;
    prepare_storage(area.path(), &request.execution)?;
    let images = Mutex::new(BTreeMap::new());
    let context = ScanContext {
        request,
        root: area.path(),
        deadline,
        images: &images,
        replay,
    };
    let mut prepared = match PreparedScan::prepare(&context) {
        Ok(prepared) => prepared,
        Err(error) if replay.is_some() => {
            let report = refused_replay(&context, error, started)?;
            progress(&report)?;
            return Ok(report);
        }
        Err(error) => return Err(error),
    };
    let mut report = ImpactReport {
        schema_version: 1,
        library: request.library.clone(),
        baseline_fingerprint: Some(prepared.baseline.fingerprint.clone()),
        candidate_fingerprint: Some(prepared.candidate.fingerprint.clone()),
        error: None,
        gate: prepared.gate.clone(),
        discovery: Discovery::default(),
        downstreams: Vec::new(),
        run: RunMetadata {
            engine_fingerprint: option_env!("CARGO_IMPACT_ENGINE_SHA256").map(str::to_owned),
            replay_of: replay
                .map(|evidence| evidence.identity.digest())
                .transpose()
                .map_err(io::Error::other)?,
            replay_expected: replay.map(|evidence| evidence.expected),
            status: RunStatus::Running,
            execution: request.execution.clone(),
            upstream: request.upstream.clone(),
            ..Default::default()
        },
    };
    report.run.storage_bytes = work_bytes(context.root)?;
    report.run.elapsed_ms = milliseconds(started.elapsed());
    progress(&report)?;
    if !report.gate.ran {
        report.run.status = RunStatus::Complete;
        report.run.coverage_sufficient = Some(true);
        progress(&report)?;
        return Ok(report);
    }
    let (discovered, discovery) = discover().unwrap_or_else(|error| {
        (
            Vec::new(),
            Discovery {
                candidates: Vec::new(),
                notes: vec![format!("Discovery failed: {error}")],
            },
        )
    });
    report.discovery = discovery;
    prepared.prepare_injection(&mut report.discovery.notes)?;
    let plans = experiment::plans(
        &context,
        request.downstreams.iter().cloned().chain(discovered),
        &mut report.discovery.notes,
    )?;
    if let Some(replay) = replay {
        report.discovery.notes.push(format!(
            "Rebuilding experiment {} from retained input identity {}; both phases require fresh source, lock, graph, compiler and image proof before compilation.",
            replay.identity.experiment_id, replay.identity.digest().map_err(io::Error::other)?
        ));
    }
    scheduler::run(
        &context,
        &prepared,
        plans,
        started,
        &mut report,
        &mut progress,
    )?;
    if let Some(evidence) = replay
        && let Some(result) = report.downstreams.first()
        && result.classification != evidence.expected
    {
        report.discovery.notes.push(format!(
            "Replay outcome differs: the original was {:?}; this fresh attempt is {:?}. Retained inputs do not prove deterministic build-script behavior or outcome equality.",
            evidence.expected, result.classification
        ));
        progress(&report)?;
    }
    Ok(report)
}

fn refused_replay(
    context: &ScanContext<'_>,
    error: ImpactError,
    started: Instant,
) -> io::Result<ImpactReport> {
    let mut notes = Vec::new();
    let plans = experiment::plans(
        context,
        context.request.downstreams.iter().cloned(),
        &mut notes,
    )?;
    let mut results: Vec<_> = plans
        .iter()
        .map(experiment::ExperimentPlan::queued)
        .collect();
    let cause = match &error {
        ImpactError::Replay(_) => crate::HarnessFailure::EvidenceMismatch,
        ImpactError::Io(error) => match error.kind() {
            io::ErrorKind::TimedOut => crate::HarnessFailure::Timeout,
            io::ErrorKind::StorageFull => crate::HarnessFailure::StorageLimit,
            _ => crate::HarnessFailure::Preparation,
        },
        ImpactError::Semver(_) => crate::HarnessFailure::Preparation,
    };
    for result in &mut results {
        experiment::finish(
            result,
            Some(crate::ExperimentFailure {
                stage: crate::ExperimentStage::SourcePreparation,
                cause,
                message: error.to_string(),
            }),
        );
    }
    let mut report = ImpactReport::failed(&context.request.library, &error.to_string());
    report.gate.ran = true;
    report.gate.reason =
        "Replay preparation failed before compilation; retained outcomes were not reused.".into();
    report.downstreams = results;
    report.run.engine_fingerprint = option_env!("CARGO_IMPACT_ENGINE_SHA256").map(str::to_owned);
    report.run.replay_of = context
        .replay
        .map(|evidence| evidence.identity.digest())
        .transpose()
        .map_err(io::Error::other)?;
    report.run.replay_expected = context.replay.map(|evidence| evidence.expected);
    report.run.elapsed_ms = milliseconds(started.elapsed());
    report.run.execution = context.request.execution.clone();
    report.run.planned_downstreams = report.downstreams.len();
    report.run.completed_downstreams = report.downstreams.len();
    report.run.coverage_sufficient = Some(false);
    report.run.storage_bytes = work_bytes(context.root)?;
    report.run.upstream = context.request.upstream.clone();
    Ok(report)
}

pub fn analyze(request: &ImpactRequest) -> Result<ImpactReport, ImpactError> {
    analyze_with_discovery(request, || Ok((Vec::new(), Discovery::default())))
}
