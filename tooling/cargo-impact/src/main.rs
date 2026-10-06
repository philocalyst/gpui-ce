use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use cargo_impact::{
    DownstreamSource, DownstreamSpec, ImpactRequest, analyze_with_discovery, bot,
    config::{Config, DiscoveryConfig},
    discovery::{CratesIo, Discover, Discovery, GitHubSearch},
    http::Api,
    runner::Runner,
};
use clap::{Parser, Subcommand};
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
    Check {
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
        Command::Check {
            library,
            baseline,
            candidate,
            config,
            downstreams,
            work_dir,
            report_dir,
            timeout_seconds,
            force,
            no_discovery,
            allow_local,
            local,
        } => {
            let config_path = config.or_else(|| {
                Path::new("impact.toml")
                    .is_file()
                    .then(|| PathBuf::from("impact.toml"))
            });
            let mut config = config_path
                .as_ref()
                .map(|path| Config::load(path))
                .transpose()?
                .unwrap_or_default();
            if local {
                config.recipe.runner = Runner::Local;
            }
            if config.requires_local_execution() && !allow_local {
                return Err("local and Nix recipes execute build scripts on the host; supply --allow-local for trusted inputs".into());
            }
            let library = library
                .or_else(|| config.library.clone())
                .ok_or("provide --library or set library in impact.toml")?;
            let mut request = ImpactRequest::new(library, baseline, candidate);
            request.recipe = config.recipe.clone();
            request.force = force;
            request.work_dir = Some(work_dir);
            request.timeout = Duration::from_secs(timeout_seconds);
            request.downstreams = config.downstreams.clone();
            for value in downstreams {
                let (name, path) = value
                    .split_once('=')
                    .ok_or("--downstream requires NAME=PATH")?;
                request.downstreams.push(DownstreamSpec::local(name, path));
            }
            for spec in &mut request.downstreams {
                config.apply_override(spec);
            }
            let report = analyze_with_discovery(&request, || {
                if no_discovery {
                    return Ok((Vec::new(), Discovery::default()));
                }
                let mut discovery =
                    discover(&request.library, &config.discovery).map_err(|e| e.to_string())?;
                let specs = candidates(&mut discovery, &config);
                Ok((specs, discovery))
            })
            .unwrap_or_else(|error| {
                cargo_impact::ImpactReport::failed(&request.library, &error.to_string())
            });
            fs::create_dir_all(&report_dir)?;
            fs::write(
                report_dir.join("report.json"),
                serde_json::to_vec_pretty(&report)?,
            )?;
            fs::write(report_dir.join("report.md"), report.markdown())?;
            println!("{}", report.markdown());
            eprintln!("Reports: {}", report_dir.display());
            // An API lint alone never fails CI. Actual regressions and inconclusive scans differ.
            Ok(if report.has_regressions() {
                1
            } else if report.has_harness_failures()
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
                serde_json::to_string_pretty(&discover(&library, &config)?)?
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
            let report = serde_json::from_slice(&fs::read(report)?)?;
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
            Ok(0)
        }
    }
}

fn discover(library: &str, config: &DiscoveryConfig) -> Result<Discovery> {
    let mut sources = Vec::new();
    if config.crates_io {
        match (CratesIo {
            api: Api::new(Url::parse("https://crates.io/")?, None)?,
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
            api: github_api()?,
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

fn candidates(discovery: &mut Discovery, config: &Config) -> Vec<DownstreamSpec> {
    let mut specs = Vec::new();
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
        // Registry package names locate workspace consumers; code-search manifests locate unpublished projects.
        let manifests: Vec<_> = if candidate.manifests.is_empty() {
            vec!["Cargo.toml".to_owned()]
        } else {
            candidate.manifests.iter().cloned().collect()
        };
        for manifest in manifests {
            let mut spec = DownstreamSpec {
                name: if manifest == "Cargo.toml" {
                    name.clone()
                } else {
                    format!("{name}:{manifest}")
                },
                source: DownstreamSource::Git {
                    url: candidate.repository.url().to_string(),
                    revision: "HEAD".into(),
                    forge: Some(candidate.repository.forge()),
                },
                manifest: manifest.into(),
                recipe: if candidate.packages.is_empty() {
                    None
                } else {
                    let mut recipe = config.recipe.clone();
                    recipe.packages = candidate.packages.iter().cloned().collect();
                    Some(recipe)
                },
            };
            config.apply_override(&mut spec);
            specs.push(spec);
        }
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
