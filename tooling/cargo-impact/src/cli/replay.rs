//! A replay always recompiles and preserves evidence for a rejected comparison.

use std::{io, time::Duration};

use cargo_impact::{
    ExperimentId, ImpactReport, RunStatus, analyze_replay_with_progress, replay::ReplayRequest,
    report::bundle,
};

use super::{
    Result,
    args::ReplayArgs,
    artifacts::{emit_report, failed_check},
};

pub(super) fn replay(args: ReplayArgs) -> Result<i32> {
    for (name, path) in [
        ("output-dir", &args.output_dir),
        ("work-dir", &args.work_dir),
    ] {
        super::paths::disjoint(
            path,
            &args.report_dir,
            &format!("replay --{name} must be outside its input report tree"),
        )?;
    }
    super::paths::report_layout(
        &args.output_dir,
        &args.work_dir,
        [args.baseline.as_path(), args.candidate.as_path()],
    )?;
    let evidence = match bundle::verify(&args.report_dir) {
        Ok(evidence) => evidence,
        Err(error) => {
            return failed_check(
                &args.output_dir,
                "unknown",
                format!("replay input failed verification: {error}"),
            );
        }
    };
    super::paths::report_layout(
        &args.output_dir,
        &args.work_dir,
        evidence
            .report
            .downstreams
            .iter()
            .filter_map(|result| match &result.source {
                cargo_impact::DownstreamSource::Local { path } => Some(path.as_path()),
                _ => None,
            }),
    )?;
    let library = evidence.report.library.clone();
    let id = match ExperimentId::try_from(args.experiment) {
        Ok(id) => id,
        Err(error) => return failed_check(&args.output_dir, &library, error),
    };
    let mut request = match ReplayRequest::new(evidence.report, id, args.baseline, args.candidate) {
        Ok(request) => request,
        Err(error) => {
            return failed_check(
                &args.output_dir,
                &library,
                format!("replay is unavailable: {error}"),
            );
        }
    };
    args.execution.apply(&mut request.execution);
    request.work_dir = Some(args.work_dir);
    request.timeout = Duration::from_secs(args.timeout_seconds);
    let mut initial = ImpactReport::failed(&library, "Preparing verified replay");
    initial.error = None;
    initial.run.status = RunStatus::Running;
    initial.run.replay_of = Some(request.identity().digest()?);
    initial.run.execution = request.execution.clone();
    initial.write_checkpoint(&args.output_dir)?;
    let report = analyze_replay_with_progress(&request, |partial| {
        partial
            .write_checkpoint(&args.output_dir)
            .map_err(io::Error::other)
    })
    .unwrap_or_else(|error| {
        let mut partial =
            ImpactReport::load(&args.output_dir.join("report.json")).unwrap_or(initial);
        partial.error = Some(error.to_string());
        partial
    });
    emit_report(&args.output_dir, &report)?;
    Ok(super::check::exit_status(&report))
}
