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
use storage::{WorkArea, prepare_storage};

#[derive(Debug, Error)]
pub enum ImpactError {
    #[error("preparing impact run: {0}")]
    Io(#[from] io::Error),
    #[error("embedded semver gate failed: {0}")]
    Semver(String),
}

pub(super) struct ScanContext<'a> {
    pub request: &'a ImpactRequest,
    pub root: &'a Path,
    pub deadline: Instant,
    pub images: &'a Mutex<BTreeMap<String, String>>,
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
    let deadline = started
        .checked_add(Duration::from_secs(request.execution.scan_timeout_seconds))
        .ok_or_else(|| io::Error::other("scan timeout exceeds the host clock range"))?;
    let area = WorkArea::open(request.work_dir.as_deref())?;
    prepare_storage(area.path(), &request.execution)?;
    let images = Mutex::new(BTreeMap::new());
    let context = ScanContext {
        request,
        root: area.path(),
        deadline,
        images: &images,
    };
    let mut prepared = PreparedScan::prepare(&context)?;
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
    scheduler::run(
        &context,
        &prepared,
        plans,
        started,
        &mut report,
        &mut progress,
    )?;
    Ok(report)
}

pub fn analyze(request: &ImpactRequest) -> Result<ImpactReport, ImpactError> {
    analyze_with_discovery(request, || Ok((Vec::new(), Discovery::default())))
}
