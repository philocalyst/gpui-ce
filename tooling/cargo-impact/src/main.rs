use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use cargo_impact::{
    DownstreamSource, DownstreamSpec, ImpactReport, ImpactRequest, RunStatus, UpstreamRevisions,
    analyze_with_progress, bot,
    config::{Config, DiscoveryConfig},
    discovery::{CratesIo, Discover, Discovery, GitHubSearch},
    forge::Repository,
    http::Api,
    runner::Runner,
};
use clap::{Args, Parser, Subcommand};
use url::Url;

mod setup;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(
    version,
    about = "Find actual downstream breakage from Rust API changes"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(name = "__semver-helper", hide = true)]
    SemverHelper {
        #[arg(long)]
        library: String,
        #[arg(long)]
        baseline_doc: PathBuf,
        #[arg(long)]
        candidate_doc: PathBuf,
        #[arg(long)]
        memory_mib: u64,
    },
    Check(CheckArgs),
    /// Render a saved report offline without rebuilding any consumer.
    Report {
        #[arg(long, required_unless_present = "failure", conflicts_with = "failure")]
        input: Option<PathBuf>,
        /// Export an inconclusive report when a CI worker failed before producing evidence.
        #[arg(long, conflicts_with = "input")]
        failure: Option<String>,
        #[arg(long, default_value = "unknown")]
        library: String,
        #[arg(long, default_value = "impact-report")]
        output_dir: PathBuf,
        #[arg(long)]
        open: bool,
    },
    /// Check runner availability and setup without executing project code.
    Doctor {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Discover {
        #[arg(long)]
        library: String,
        #[arg(long)]
        github: bool,
        #[arg(long, default_value_t = 5)]
        max_pages: u32,
    },
    Bot {
        #[arg(long, env = "GITHUB_EVENT_PATH")]
        event: PathBuf,
        #[arg(long, env = "GITHUB_REPOSITORY")]
        repository: String,
        #[arg(long, default_value = "cargo-impact")]
        name: String,
        #[arg(long)]
        acknowledge: bool,
    },
    /// Publish a saved report only if the PR still points at the tested head.
    Publish {
        #[arg(long, env = "GITHUB_REPOSITORY")]
        repository: String,
        #[arg(long)]
        pull_request: u64,
        #[arg(long)]
        candidate_sha: String,
        #[arg(long)]
        report: PathBuf,
        #[arg(long)]
        run_url: Url,
    },
    /// Write a minimal config and workflows without overwriting existing files.
    Init {
        #[arg(long)]
        library: String,
        #[arg(long, default_value = ".")]
        directory: PathBuf,
    },
}

#[derive(Args)]
struct CheckArgs {
    #[arg(long)]
    library: Option<String>,
    #[arg(long)]
    baseline: PathBuf,
    #[arg(long)]
    candidate: PathBuf,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long = "downstream", value_name = "NAME=PATH")]
    downstreams: Vec<String>,
    #[arg(long, default_value = ".cargo-impact")]
    work_dir: PathBuf,
    #[arg(long, default_value = "impact-report")]
    report_dir: PathBuf,
    #[arg(long, default_value_t = 1800, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_seconds: u64,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    no_discovery: bool,
    /// Allow host execution for local/Nix recipes; never needed for Docker.
    #[arg(long)]
    allow_local: bool,
    /// Choose a local recipe instead of the default Docker environment.
    #[arg(long, requires = "allow_local")]
    local: bool,
    /// Maximum concurrent consumers; each baseline/candidate pair is sequential.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=32))]
    jobs: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=256))]
    cargo_jobs: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(128..))]
    memory_mib: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    cpus: Option<u16>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=16384))]
    max_work_gib: Option<u64>,
    /// Discard managed build artifacts before the run; preserve user files.
    #[arg(long)]
    prune: bool,
    #[arg(long)]
    upstream_repository: Option<String>,
    #[arg(long)]
    baseline_sha: Option<String>,
    #[arg(long)]
    candidate_sha: Option<String>,
    /// Total scan budget, including API comparison and consumer experiments.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    scan_timeout_seconds: Option<u64>,
}

fn main() {
    // Cargo passes its subcommand name as argv[1] when executing cargo-impact.
    let mut args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|v| v == "impact") {
        args.remove(1);
    }
    let exit = match run(Cli::parse_from(args)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("cargo-impact: {error}");
            2
        }
    };
    std::process::exit(exit);
}

fn run(cli: Cli) -> Result<i32> {
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
            let report = ImpactReport::load(&report)?;
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

fn check(args: CheckArgs) -> Result<i32> {
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
        jobs,
        cargo_jobs,
        memory_mib,
        cpus,
        max_work_gib,
        prune,
        upstream_repository,
        baseline_sha,
        candidate_sha,
        scan_timeout_seconds,
    } = args;
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
    if local {
        config.recipe.runner = Runner::Local;
    }
    if let Some(jobs) = jobs {
        config.execution.jobs = jobs as usize;
    }
    if let Some(jobs) = cargo_jobs {
        config.execution.cargo_jobs = jobs as usize;
    }
    if let Some(memory) = memory_mib {
        config.execution.memory_mib = memory;
    }
    if let Some(cpus) = cpus {
        config.execution.cpus = cpus;
    }
    if let Some(gib) = max_work_gib {
        config.execution.max_work_bytes = Some(gib * 1024 * 1024 * 1024);
    }
    config.execution.prune_before_run |= prune;
    if let Some(seconds) = scan_timeout_seconds {
        config.execution.scan_timeout_seconds = seconds;
    }
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
    request.work_dir = Some(work_dir);
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
    Ok(if report.has_regressions() {
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
    })
}

fn failed_check(report_dir: &Path, library: &str, error: impl ToString) -> Result<i32> {
    let report = ImpactReport::failed(library, &error.to_string());
    emit_report(report_dir, &report)?;
    Ok(2)
}

fn emit_report(report_dir: &Path, report: &ImpactReport) -> Result<()> {
    report.write_artifacts(report_dir)?;
    let count = |kind| {
        report
            .downstreams
            .iter()
            .filter(|r| r.classification == kind)
            .count()
    };
    println!(
        "{}: {} regressions, {} compatible, {} harness failures. {}",
        report.library,
        count(cargo_impact::Classification::Regression),
        count(cargo_impact::Classification::Compatible),
        count(cargo_impact::Classification::HarnessFailure),
        report.gate.reason
    );
    println!("Inspect: {}", report_dir.join("index.html").display());
    Ok(())
}

fn open_report(path: &Path) -> Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if !std::process::Command::new(program)
        .arg(path)
        .status()?
        .success()
    {
        return Err("could not open the report; open index.html manually".into());
    }
    Ok(())
}

fn discover(
    library: &str,
    config: &DiscoveryConfig,
    deadline: Option<Instant>,
) -> Result<Discovery> {
    let mut sources = Vec::new();
    if config.crates_io {
        match (CratesIo {
            api: bounded_api(Api::new(Url::parse("https://crates.io/")?, None)?, deadline),
            max_pages: config.max_pages,
        })
        .discover(library)
        {
            Ok(discovery) => sources.push(discovery),
            Err(error) => sources.push(Discovery {
                candidates: Vec::new(),
                notes: vec![format!("Discovery failed: crates.io: {error}")],
            }),
        }
    }
    if config.github {
        match (GitHubSearch {
            api: bounded_api(github_api()?, deadline),
            max_pages: config.max_pages,
        })
        .discover(library)
        {
            Ok(discovery) => sources.push(discovery),
            Err(error) => sources.push(Discovery {
                candidates: Vec::new(),
                notes: vec![format!("Discovery failed: GitHub: {error}")],
            }),
        }
    }
    Ok(Discovery::merge(sources))
}

fn bounded_api(api: Api, deadline: Option<Instant>) -> Api {
    match deadline {
        Some(deadline) => api.with_deadline(deadline),
        None => api,
    }
}

fn candidates(discovery: &mut Discovery, config: &Config) -> Vec<DownstreamSpec> {
    let mut specs = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut omitted = 0;
    for candidate in discovery
        .candidates
        .iter()
        .take(config.discovery.max_repositories)
    {
        let name = format!(
            "{}/{}",
            candidate.repository.url().host_str().unwrap_or("forge"),
            candidate.repository.name()
        );
        // Published packages are independent experiments: one removed package must
        // not poison another valid workspace consumer in the same repository.
        let selections: Vec<(String, Option<String>)> = candidate
            .packages
            .iter()
            .map(|package| ("Cargo.toml".into(), Some(package.clone())))
            .chain(
                candidate
                    .manifests
                    .iter()
                    .map(|manifest| (manifest.clone(), None)),
            )
            .collect();
        let selections = if selections.is_empty() {
            vec![("Cargo.toml".into(), None)]
        } else {
            selections
        };
        for (manifest, package) in selections {
            let suffix = package.as_deref().unwrap_or(&manifest);
            let mut spec = DownstreamSpec {
                name: if suffix == "Cargo.toml" {
                    name.clone()
                } else {
                    format!("{name}:{suffix}")
                },
                source: DownstreamSource::Git {
                    url: candidate.repository.url().to_string(),
                    revision: "HEAD".into(),
                    forge: Some(candidate.repository.forge()),
                },
                manifest: manifest.into(),
                recipe: package.map(|package| {
                    let mut recipe = config.recipe.clone();
                    recipe.packages = vec![package];
                    recipe
                }),
            };
            if let Some(recipe) = config.overrides.get(&name) {
                spec.recipe = Some(recipe.clone());
            }
            config.apply_override(&mut spec);
            let identity = serde_json::to_string(&(
                &spec.source,
                &spec.manifest,
                spec.recipe.as_ref().unwrap_or(&config.recipe),
            ))
            .expect("selection is serializable");
            if seen.insert(identity) {
                if specs.len() < config.discovery.max_experiments {
                    specs.push(spec);
                } else {
                    omitted += 1;
                }
            }
        }
    }
    if omitted > 0 {
        discovery.notes.push(format!("Experiment budget: {} package/manifest selections omitted; {} indexed experiments selected.",omitted,specs.len()));
    }
    if discovery
        .candidates
        .iter()
        .any(|candidate| !candidate.packages.is_empty() && !candidate.manifests.is_empty())
    {
        discovery.notes.push("Registry package and code-search manifest selections can overlap after Cargo resolves the workspace; they are not evidence of distinct repositories or all users.".into());
    }
    if discovery.candidates.len() > config.discovery.max_repositories {
        discovery.notes.push(format!(
            "Build budget: {}/{} repositories selected.",
            config.discovery.max_repositories,
            discovery.candidates.len()
        ));
    }
    specs
}

fn github_api() -> Result<Api> {
    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .or_else(|| std::env::var("GH_TOKEN").ok());
    let base = std::env::var("GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com/".into());
    let base = Url::parse(&format!("{}/", base.trim_end_matches('/')))?;
    if base.scheme() != "https" || !base.username().is_empty() || base.password().is_some() {
        return Err("GitHub API requires an HTTPS URL without credentials".into());
    }
    Ok(Api::new(base, token)?)
}
