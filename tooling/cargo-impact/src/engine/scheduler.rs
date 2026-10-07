//! Bounded dispatch and authoritative lifecycle/evidence ownership. A completed
//! baseline is checkpointed before its candidate begins consuming resources.

use super::{
    ScanContext,
    experiment::{self, ExperimentEvent, ExperimentPlan},
    milliseconds,
    preparation::PreparedScan,
    storage::work_bytes,
};
use crate::model::{
    BuildPhase, Classification, ExperimentFailure, ExperimentLifecycle, ExperimentStage,
    ExperimentStatus, HarnessFailure, ImpactReport, RunStatus,
};
use std::{io, sync::mpsc, time::Instant};

enum WorkerEvent {
    Experiment(ExperimentEvent),
    Finished {
        elapsed_ms: u64,
        outcome: Result<(), ExperimentFailure>,
    },
}

pub(super) fn run(
    context: &ScanContext<'_>,
    upstream: &PreparedScan,
    plans: Vec<ExperimentPlan>,
    started: Instant,
    report: &mut ImpactReport,
    progress: &mut impl FnMut(&ImpactReport) -> io::Result<()>,
) -> io::Result<()> {
    report.run.planned_downstreams = plans.len();
    report.downstreams = plans.iter().map(ExperimentPlan::queued).collect();
    progress(report)?;
    let detect_drift = plans.iter().any(|plan| !plan.recipe.runner.is_isolated());
    let mut stopped = scheduling_failure(context, report, false);
    if let Some(reason) = &stopped {
        cancel_queued(report, 0, reason);
    }
    std::thread::scope(|scope| -> io::Result<()> {
        let worker_count = if stopped.is_some() {
            0
        } else {
            context.request.execution.jobs.min(plans.len())
        };
        let (events, receiver) = mpsc::channel();
        let mut queues = Vec::new();
        for worker in 0..worker_count {
            let (queue, tasks) = mpsc::channel::<usize>();
            queues.push(queue);
            let events = events.clone();
            let plans = &plans;
            scope.spawn(move || {
                for index in tasks {
                    let started = Instant::now();
                    let mut current_stage = ExperimentStage::SourcePreparation;
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        experiment::execute(context, upstream, &plans[index], |event| {
                            match &event {
                                ExperimentEvent::Preparing => {
                                    current_stage = ExperimentStage::SourcePreparation
                                }
                                ExperimentEvent::PhaseStarted(phase) => {
                                    current_stage = phase.stage()
                                }
                                _ => {}
                            }
                            events
                                .send((worker, index, WorkerEvent::Experiment(event)))
                                .map_err(|_| {
                                    io::Error::new(
                                        io::ErrorKind::BrokenPipe,
                                        "report coordinator stopped",
                                    )
                                })
                        })
                    }))
                    .unwrap_or_else(|_| {
                        Err(ExperimentFailure {
                            stage: current_stage,
                            cause: HarnessFailure::Preparation,
                            message: "build worker panicked; experiment is inconclusive".into(),
                        })
                    });
                    if events
                        .send((
                            worker,
                            index,
                            WorkerEvent::Finished {
                                elapsed_ms: milliseconds(started.elapsed()),
                                outcome,
                            },
                        ))
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
        drop(events);
        let mut next = 0;
        for queue in &queues {
            queue.send(next).map_err(io::Error::other)?;
            next += 1;
        }
        let mut active = worker_count;
        while active > 0 {
            let (worker, index, event) = receiver.recv().map_err(io::Error::other)?;
            let result = &mut report.downstreams[index];
            let mut acknowledgement = None;
            let finished = match event {
                WorkerEvent::Experiment(ExperimentEvent::Preparing) => {
                    result.lifecycle.status = ExperimentStatus::Preparing;
                    false
                }
                WorkerEvent::Experiment(ExperimentEvent::PhaseStarted(phase)) => {
                    result.lifecycle.status = phase.status();
                    false
                }
                WorkerEvent::Experiment(ExperimentEvent::Source {
                    revision,
                    fingerprint,
                }) => {
                    result.revision = revision;
                    result.source_fingerprint = Some(fingerprint);
                    false
                }
                WorkerEvent::Experiment(ExperimentEvent::Phase {
                    phase,
                    build,
                    persisted,
                }) => {
                    match phase {
                        BuildPhase::Baseline => result.baseline = *build,
                        BuildPhase::Candidate => result.candidate = *build,
                    };
                    acknowledgement = Some(persisted);
                    false
                }
                WorkerEvent::Finished {
                    elapsed_ms,
                    outcome,
                } => {
                    result.elapsed_ms = elapsed_ms;
                    experiment::finish(result, outcome.err());
                    report.run.completed_downstreams += 1;
                    active -= 1;
                    true
                }
            };
            report.run.elapsed_ms = milliseconds(started.elapsed());
            report.run.exercised_downstreams = exercised(report);
            if finished {
                report.run.storage_bytes = work_bytes(context.root)?;
                let mutated = detect_drift && upstream.inputs_changed()?;
                if mutated {
                    for result in &mut report.downstreams {
                        if result.lifecycle.status == ExperimentStatus::Complete {
                            experiment::finish(
                                result,
                                Some(ExperimentFailure {
                                    stage: ExperimentStage::IntegrityCheck,
                                    cause: HarnessFailure::InputMutation,
                                    message: "Canonical upstream source changed during host execution; results are inconclusive. Use an isolated recipe and retry.".into(),
                                }),
                            );
                        }
                    }
                    report.run.exercised_downstreams = exercised(report);
                }
                stopped = stopped.or_else(|| scheduling_failure(context, report, mutated));
                if let Some(reason) = &stopped {
                    cancel_queued(report, next, reason);
                    next = plans.len();
                }
            }
            progress(report)?;
            if let Some(persisted) = acknowledgement {
                let _ = persisted.send(());
            }
            if finished && stopped.is_none() && next < plans.len() {
                queues[worker].send(next).map_err(io::Error::other)?;
                next += 1;
                active += 1;
            }
        }
        drop(queues);
        Ok(())
    })?;
    report.run.elapsed_ms = milliseconds(started.elapsed());
    report.run.storage_bytes = work_bytes(context.root)?;
    report.run.status = RunStatus::Complete;
    report.run.exercised_downstreams = exercised(report);
    let sufficient =
        report.run.exercised_downstreams >= context.request.execution.minimum_exercised;
    report.run.coverage_sufficient = Some(sufficient);
    if !sufficient {
        report.discovery.notes.push(format!("Coverage is inconclusive: {} consumer comparisons exercised the library; at least {} were required. Select a consuming package or feature, expand discovery, or adjust execution.minimum_exercised explicitly.", report.run.exercised_downstreams, context.request.execution.minimum_exercised));
    }
    progress(report)
}

fn exercised(report: &ImpactReport) -> usize {
    report
        .downstreams
        .iter()
        .filter(|result| {
            result.lifecycle.status == ExperimentStatus::Complete
                && matches!(
                    result.classification,
                    Classification::Compatible | Classification::Regression
                )
        })
        .count()
}

fn scheduling_failure(
    context: &ScanContext<'_>,
    report: &mut ImpactReport,
    mutated: bool,
) -> Option<ExperimentFailure> {
    let (cause, message) = if Instant::now() >= context.deadline {
        (
            HarnessFailure::Timeout,
            "The scan wall-clock budget was exhausted; queued experiments were not started.",
        )
    } else if mutated {
        (
            HarnessFailure::InputMutation,
            "Canonical upstream inputs changed; queued experiments were not started.",
        )
    } else if context
        .request
        .execution
        .max_work_bytes
        .is_some_and(|limit| report.run.storage_bytes > limit)
    {
        (
            HarnessFailure::StorageLimit,
            "Work directory exceeds its storage budget; queued experiments were not started. Prune managed caches or increase the budget.",
        )
    } else {
        return None;
    };
    report.error = Some(message.into());
    Some(ExperimentFailure {
        stage: ExperimentStage::Scheduling,
        cause,
        message: message.into(),
    })
}

fn cancel_queued(report: &mut ImpactReport, next: usize, failure: &ExperimentFailure) {
    for result in report.downstreams.iter_mut().skip(next) {
        result.lifecycle = ExperimentLifecycle {
            status: ExperimentStatus::Cancelled,
            failure: Some(failure.clone()),
        };
        result.message = Some(failure.message.clone());
    }
}
