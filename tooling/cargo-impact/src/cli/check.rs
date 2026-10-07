//! Turn config and CLI policy into a scan request, retaining evidence on failure.

use super::{
    Result,
    args::CheckArgs,
    artifacts::{emit_report, failed_check},
    discovery::{candidates, discover},
};
use cargo_impact::{
    DownstreamSpec, ImpactReport, ImpactRequest, RunStatus, UpstreamRevisions,
    analyze_with_progress, config::Config, discovery::Discovery, forge::Repository, runner::Runner,
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) fn check(args: CheckArgs) -> Result<i32> {
    let CheckArgs {
        library,
        baseline,
        candidate,
        config: config_arg,
        downstreams,
        work_dir,
        report_dir,
        timeout_seconds,
        force,
        no_discovery,
        allow_local,
        local,
        execution,
        upstream_repository,
        baseline_sha,
        candidate_sha,
    } = args;
    super::paths::report_layout(
        &report_dir,
        &work_dir,
        [baseline.as_path(), candidate.as_path()].into_iter().chain(
            downstreams
                .iter()
                .filter_map(|value| value.split_once('=').map(|(_, path)| Path::new(path))),
        ),
    )?;
    let config_path = config_arg.or_else(|| {
        Path::new("impact.toml")
            .is_file()
            .then(|| PathBuf::from("impact.toml"))
    });
    let mut config = match config_path.as_ref() {
        Some(path) => match Config::load(path) {
            Ok(config) => config,
            Err(error) => {
                return failed_check(
                    &report_dir,
                    library.as_deref().unwrap_or("unknown"),
                    format!(
                        "could not load configuration file {}: {error}",
                        path.display()
                    ),
                );
            }
        },
        None => Config::default(),
    };
    super::paths::report_layout(
        &report_dir,
        &work_dir,
        config
            .downstreams
            .iter()
            .filter_map(|spec| match &spec.source {
                cargo_impact::DownstreamSource::Local { path } => Some(path.as_path()),
                _ => None,
            }),
    )?;
    if local {
        config.recipe.runner = Runner::Local;
    }
    execution.apply(&mut config.execution);
    let library = match library.or_else(|| config.library.clone()) {
        Some(library) => library,
        None => {
            return failed_check(
                &report_dir,
                "unknown",
                "provide --library or set library in impact.toml",
            );
        }
    };
    if config.requires_local_execution() && !allow_local {
        return failed_check(
            &report_dir,
            &library,
            "local and Nix recipes execute build scripts on the host; supply --allow-local for trusted inputs",
        );
    }

    let mut request = ImpactRequest::new(library, baseline, candidate);
    request.recipe = config.recipe.clone();
    request.execution = config.execution.clone();
    request.semver_helper = Some(std::env::current_exe()?);
    if upstream_repository.is_some() || baseline_sha.is_some() || candidate_sha.is_some() {
        let repository = match upstream_repository
            .as_deref()
            .map(|url| Repository::parse(url, None))
            .transpose()
        {
            Ok(repository) => repository,
            Err(error) => return failed_check(&report_dir, &request.library, error),
        };
        request.upstream = Some(UpstreamRevisions {
            repository,
            baseline_sha,
            candidate_sha,
        });
    }
    request.force = force;
    request.timeout = Duration::from_secs(timeout_seconds);
    request.downstreams = config.downstreams.clone();
    for value in downstreams {
        let Some((name, path)) = value.split_once('=') else {
            return failed_check(
                &report_dir,
                &request.library,
                "--downstream requires NAME=PATH",
            );
        };
        request.downstreams.push(DownstreamSpec::local(name, path));
    }
    for spec in &mut request.downstreams {
        config.apply_override(spec);
    }
    request.work_dir = Some(work_dir);
    let mut initial = ImpactReport::failed(&request.library, "Preparing experiment");
    initial.error = None;
    initial.run.status = RunStatus::Running;
    initial.run.execution = request.execution.clone();
    initial.run.upstream = request.upstream.clone();
    initial.gate.reason =
        "Preparing the API comparison; no compatibility result is available yet.".into();
    initial.write_checkpoint(&report_dir)?;
    let discovery_deadline = Instant::now()
        .checked_add(Duration::from_secs(request.execution.scan_timeout_seconds))
        .ok_or("scan timeout is too large")?;
    let report = analyze_with_progress(
        &request,
        || {
            if no_discovery {
                return Ok((Vec::new(), Discovery::default()));
            }
            let mut discovery = discover(
                &request.library,
                &config.discovery,
                Some(discovery_deadline),
            )
            .map_err(|e| e.to_string())?;
            let specs = candidates(&mut discovery, &config);
            Ok((specs, discovery))
        },
        |partial| {
            partial
                .write_checkpoint(&report_dir)
                .map_err(std::io::Error::other)
        },
    )
    .unwrap_or_else(|error| {
        let mut partial = ImpactReport::load(&report_dir.join("report.json")).unwrap_or(initial);
        partial.error = Some(error.to_string());
        partial.gate.reason =
            "The experiment did not complete; missing results are inconclusive.".into();
        partial
    });
    emit_report(&report_dir, &report)?;
    // An API lint alone never fails CI. Actual regressions and inconclusive scans differ.
    Ok(exit_status(&report))
}

pub(super) fn exit_status(report: &ImpactReport) -> i32 {
    // Proven impact, inconclusive evidence and compatible coverage remain distinct.
    if report.has_regressions() {
        1
    } else if report.has_harness_failures()
        || report.run.status == RunStatus::Running
        || report.run.coverage_sufficient == Some(false)
        || report
            .discovery
            .notes
            .iter()
            .any(|n| n.starts_with("Discovery failed:"))
    {
        2
    } else {
        0
    }
}
