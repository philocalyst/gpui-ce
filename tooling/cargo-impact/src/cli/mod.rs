//! CLI dispatch is separate from scan preparation and offline report handling.

use crate::setup;
use args::{Cli, Command};
use artifacts::open_report;
use cargo_impact::{
    ImpactReport, bot,
    config::{Config, DiscoveryConfig},
};
use check::check;
use clap::Parser;
use discovery::{discover, github_api};
use std::{
    fs,
    path::{Path, PathBuf},
};

mod args;
mod artifacts;
mod check;
mod discovery;
mod replay;
mod resources;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(crate) fn run() -> Result<i32> {
    let mut args: Vec<_> = std::env::args_os().collect();
    // Cargo adds its subcommand name before forwarding arguments.
    if args.get(1).is_some_and(|value| value == "impact") {
        args.remove(1);
    }
    dispatch(Cli::parse_from(args))
}

fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::SemverHelper {
            library,
            baseline_doc,
            candidate_doc,
            memory_mib,
        } => {
            let result = cargo_impact::run_semver_helper(
                &library,
                &baseline_doc,
                &candidate_doc,
                memory_mib,
            )
            .map_err(std::io::Error::other)?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(0)
        }
        Command::Check(args) => check(args),
        Command::Replay(args) => replay::replay(args),
        Command::PruneReports {
            report_dir,
            keep_previous,
            apply,
        } => {
            let report =
                cargo_impact::report::bundle::prune(&report_dir, keep_previous as usize, apply)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(0)
        }
        Command::Verify { report_dir, json } => {
            let verified = cargo_impact::report::bundle::verify(&report_dir)?;
            let summary = serde_json::json!({
                "valid":true,
                "report_sha256":verified.report_sha256,
                "bundle":verified.manifest.as_ref().map(|manifest| &manifest.id),
                "artifacts":verified.manifest.as_ref().map(|manifest| manifest.artifacts.len()),
                "bytes":verified.manifest.as_ref().map(|manifest| manifest.bytes()),
                "library":verified.report.library,
                "run_status":verified.report.run.status,
                "consumers":verified.report.downstreams.len(),
                "outcomes":cargo_impact::report::ReportSummary::from_report(&verified.report),
            });
            if json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else if let Some(manifest) = &verified.manifest {
                println!(
                    "Verified {} artifacts ({:.1} MiB) in bundle {}",
                    manifest.artifacts.len(),
                    manifest.bytes() as f64 / (1024.0 * 1024.0),
                    manifest.id
                );
            } else {
                println!(
                    "Verified checkpoint JSON; render report.json to obtain a complete view bundle."
                );
            }
            Ok(0)
        }
        Command::Report {
            input,
            failure,
            library,
            output_dir,
            open,
        } => {
            let report = match input {
                Some(input) => ImpactReport::load(&input)?,
                None => ImpactReport::failed(
                    &library,
                    failure
                        .as_deref()
                        .ok_or("report requires --input or --failure")?,
                ),
            };
            report.write_artifacts(&output_dir)?;
            let index = output_dir.join("index.html").canonicalize()?;
            println!("Rendered report: {}", index.display());
            if open {
                open_report(&index)?;
            }
            Ok(0)
        }
        Command::Doctor { config, json } => {
            let path = config.or_else(|| {
                Path::new("impact.toml")
                    .is_file()
                    .then(|| PathBuf::from("impact.toml"))
            });
            let config = path
                .as_deref()
                .map(Config::load)
                .transpose()?
                .unwrap_or_default();
            let report = cargo_impact::doctor::diagnose(&config);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                for check in &report.checks {
                    println!(
                        "{} {}: {}",
                        if check.ready { "✓" } else { "✗" },
                        check.name,
                        check.detail
                    );
                    if let Some(remedy) = &check.remedy {
                        println!("  {remedy}");
                    }
                }
            }
            Ok(if report.ready { 0 } else { 2 })
        }
        Command::Discover {
            library,
            github,
            max_pages,
        } => {
            let config = DiscoveryConfig {
                github,
                max_pages,
                ..Default::default()
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&discover(&library, &config, None)?)?
            );
            Ok(0)
        }
        Command::Bot {
            event,
            repository,
            name,
            acknowledge,
        } => {
            if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
                return Err("bot requires a trusted GitHub Actions event file".into());
            }
            let api = github_api()?;
            let event: bot::CommentEvent = serde_json::from_slice(&fs::read(event)?)?;
            let plan = bot::plan(&api, &event, &repository, &name)?;
            if acknowledge && let Some(plan) = &plan {
                bot::acknowledge(&api, plan)?;
            }
            println!("{}", serde_json::to_string(&plan)?);
            Ok(0)
        }
        Command::Publish {
            repository,
            pull_request,
            candidate_sha,
            report,
            run_url,
        } => {
            let directory = report
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let verified = cargo_impact::report::bundle::verify(directory)?;
            if report.file_name().is_none_or(|name| name != "report.json") {
                return Err("publish requires the verified bundle's report.json".into());
            }
            let report = verified.report;
            if bot::publish(
                &github_api()?,
                &repository,
                pull_request,
                &candidate_sha,
                &report,
                &run_url,
            )? {
                println!("Published report for {candidate_sha}");
            } else {
                println!("PR moved or closed; retained the report without commenting");
            }
            Ok(0)
        }
        Command::Init { library, directory } => {
            setup::initialize(&directory, &library)?;
            println!(
                "Created impact.toml and example workflows in {}",
                directory.display()
            );
            println!(
                "Next: inspect impact.toml, run cargo impact doctor, and commit the generated files to the default branch. The comment bot needs Actions comment permissions enabled."
            );
            Ok(0)
        }
    }
}
