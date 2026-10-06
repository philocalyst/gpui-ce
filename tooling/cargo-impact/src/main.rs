use std::{path::PathBuf, time::Duration};

use cargo_impact::{DownstreamSource, DownstreamSpec, ImpactRequest, analyze};
use clap::Parser;

#[derive(Parser)]
#[command(about = "Find downstream Rust repositories affected by an API change")]
struct Args {
    #[arg(long)]
    library: String,
    #[arg(long)]
    baseline: PathBuf,
    #[arg(long)]
    candidate: PathBuf,
    #[arg(long = "downstream", value_name = "NAME=PATH")]
    downstreams: Vec<String>,
    #[arg(long)]
    work_dir: Option<PathBuf>,
    #[arg(long, default_value_t = 1800)]
    timeout_seconds: u64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("cargo-impact: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let downstreams = args
        .downstreams
        .iter()
        .map(|value| {
            let (name, path) = value
                .split_once('=')
                .ok_or_else(|| format!("invalid --downstream `{value}`; expected NAME=PATH"))?;
            Ok(DownstreamSpec {
                name: name.to_owned(),
                source: DownstreamSource::Local(PathBuf::from(path)),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let request = ImpactRequest {
        library: args.library,
        baseline: args.baseline,
        candidate: args.candidate,
        downstreams,
        work_dir: args.work_dir,
        timeout: Duration::from_secs(args.timeout_seconds),
    };
    let report = analyze(&request)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
