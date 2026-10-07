//! Explicit resource overrides shared by normal scans and verified replay.

use cargo_impact::ExecutionOptions;
use clap::Args;

#[derive(Args, Default)]
pub(super) struct ExecutionArgs {
    /// Maximum concurrent consumers; each baseline/candidate pair is sequential.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=32))]
    pub(super) jobs: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=256))]
    pub(super) cargo_jobs: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(128..))]
    pub(super) memory_mib: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    pub(super) cpus: Option<u16>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=16384))]
    pub(super) max_work_gib: Option<u64>,
    /// Discard managed build artifacts before the run; preserve user files.
    #[arg(long)]
    pub(super) prune: bool,
    /// Total scan budget, including API comparison and consumer experiments.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) scan_timeout_seconds: Option<u64>,
}

impl ExecutionArgs {
    pub(super) fn apply(self, policy: &mut ExecutionOptions) {
        if let Some(jobs) = self.jobs {
            policy.jobs = jobs as usize;
        }
        if let Some(jobs) = self.cargo_jobs {
            policy.cargo_jobs = jobs as usize;
        }
        if let Some(memory) = self.memory_mib {
            policy.memory_mib = memory;
        }
        if let Some(cpus) = self.cpus {
            policy.cpus = cpus;
        }
        if let Some(gib) = self.max_work_gib {
            policy.max_work_bytes = Some(gib * 1024 * 1024 * 1024);
        }
        if let Some(seconds) = self.scan_timeout_seconds {
            policy.scan_timeout_seconds = seconds;
        }
        policy.prune_before_run |= self.prune;
    }
}
