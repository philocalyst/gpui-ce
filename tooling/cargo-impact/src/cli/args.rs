//! Command grammar; runtime behavior lives in the corresponding handler.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use url::Url;

#[derive(Parser)]
#[command(
    version,
    about = "Find actual downstream breakage from Rust API changes"
)]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Subcommand)]
pub(super) enum Command {
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
    /// Recompile one completed Docker experiment using its verified retained evidence.
    Replay(ReplayArgs),
    /// Verify a downloaded bundle and its current evidence without building code.
    Verify {
        #[arg(long, default_value = "impact-report")]
        report_dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Preview removal of old verified report generations; --apply performs it.
    PruneReports {
        #[arg(long, default_value = "impact-report")]
        report_dir: PathBuf,
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(0..=1000))]
        keep_previous: u32,
        #[arg(long)]
        apply: bool,
    },
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
pub(super) struct ReplayArgs {
    #[command(flatten)]
    pub(super) execution: super::resources::ExecutionArgs,
    #[arg(long)]
    pub(super) report_dir: PathBuf,
    /// Full experiment ID recorded in report.json and the consumer's report anchor.
    #[arg(long)]
    pub(super) experiment: String,
    #[arg(long)]
    pub(super) baseline: PathBuf,
    #[arg(long)]
    pub(super) candidate: PathBuf,
    #[arg(long, default_value = "impact-replay")]
    pub(super) output_dir: PathBuf,
    #[arg(long, default_value = ".cargo-impact-replay")]
    pub(super) work_dir: PathBuf,
    #[arg(long, default_value_t = 1800, value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) timeout_seconds: u64,
}

#[derive(Args)]
pub(super) struct CheckArgs {
    #[arg(long)]
    pub(super) library: Option<String>,
    #[arg(long)]
    pub(super) baseline: PathBuf,
    #[arg(long)]
    pub(super) candidate: PathBuf,
    #[arg(long)]
    pub(super) config: Option<PathBuf>,
    #[arg(long = "downstream", value_name = "NAME=PATH")]
    pub(super) downstreams: Vec<String>,
    #[arg(long, default_value = ".cargo-impact")]
    pub(super) work_dir: PathBuf,
    #[arg(long, default_value = "impact-report")]
    pub(super) report_dir: PathBuf,
    #[arg(long, default_value_t = 1800, value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) timeout_seconds: u64,
    #[arg(long)]
    pub(super) force: bool,
    #[arg(long)]
    pub(super) no_discovery: bool,
    /// Allow host execution for local/Nix recipes; never needed for Docker.
    #[arg(long)]
    pub(super) allow_local: bool,
    /// Choose a local recipe instead of the default Docker environment.
    #[arg(long, requires = "allow_local")]
    pub(super) local: bool,
    #[command(flatten)]
    pub(super) execution: super::resources::ExecutionArgs,
    #[arg(long)]
    pub(super) upstream_repository: Option<String>,
    #[arg(long)]
    pub(super) baseline_sha: Option<String>,
    #[arg(long)]
    pub(super) candidate_sha: Option<String>,
}
