//! A replay always recompiles and preserves evidence for a rejected comparison.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

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
    let input = destination(&args.report_dir)?;
    for (name, path) in [
        ("output-dir", &args.output_dir),
        ("work-dir", &args.work_dir),
    ] {
        let path = destination(path)?;
        if path.starts_with(&input) || input.starts_with(&path) {
            return Err(format!("replay --{name} must be outside its input report tree").into());
        }
    }
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

/// Resolve existing aliases before checking a destination that may not yet exist.
fn destination(path: &Path) -> io::Result<PathBuf> {
    let mut existing = std::path::absolute(path)?;
    let mut tail = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(mut root) => {
                for component in tail.into_iter().rev() {
                    root.push(component);
                }
                return Ok(root);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or_else(|| {
                    io::Error::other("create destinations containing unresolved '..' before replay")
                })?;
                tail.push(name.to_owned());
                if !existing.pop() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
}
