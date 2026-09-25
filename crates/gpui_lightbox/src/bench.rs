//! UI benchmarks with one call: time every frame's draw, summarize, keep a
//! history per benchmark and flag regressions against the previous run.

use crate::{
    output::{RunInfo, Suite, root, slug},
    stage::{Stage, StageConfig},
};
use anyhow::{Context as _, Result};
use gpui::{App, Window};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

/// Benchmarks in one process run one at a time, so they don't skew each other.
static BENCH_LOCK: Mutex<()> = Mutex::new(());

/// How to run a benchmark.
#[derive(Clone, Debug)]
pub struct BenchSpec {
    /// Frames drawn before measuring (caches, atlases, layout warm up).
    pub warmup: usize,
    /// Frames measured.
    pub iterations: usize,
    /// Re-render every view each frame (`window.refresh()`), measuring the
    /// full cost of the view. When `false`, a frame draws only what the
    /// step invalidated.
    pub refresh: bool,
    /// Record per-phase timings (render, layout, prepaint, paint) through
    /// the engine's inspector capture, when the engine provides them.
    pub phases: bool,
    /// A p50 slower than the previous comparable run by more than this
    /// fraction is a regression.
    pub regression_threshold: f64,
    /// The stage the view is mounted on.
    pub stage: StageConfig,
    /// Where histories are kept (default `target/lightbox/bench`).
    pub history_dir: Option<PathBuf>,
}

impl Default for BenchSpec {
    fn default() -> Self {
        Self {
            warmup: 10,
            iterations: 100,
            refresh: true,
            phases: true,
            regression_threshold: 0.10,
            stage: StageConfig::default(),
            history_dir: None,
        }
    }
}

/// Summary statistics of a sample, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    /// Samples.
    pub count: usize,
    /// Smallest.
    pub min: f64,
    /// Median.
    pub p50: f64,
    /// 95th percentile.
    pub p95: f64,
    /// 99th percentile.
    pub p99: f64,
    /// Largest.
    pub max: f64,
    /// Mean.
    pub mean: f64,
    /// Standard deviation.
    pub stddev: f64,
}

impl Stats {
    /// Statistics of `samples` (nearest-rank percentiles).
    pub fn of(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let rank =
            |p: f64| sorted[((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1];
        let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
        let variance = sorted
            .iter()
            .map(|sample| (sample - mean).powi(2))
            .sum::<f64>()
            / sorted.len() as f64;
        Self {
            count: sorted.len(),
            min: sorted[0],
            p50: rank(0.5),
            p95: rank(0.95),
            p99: rank(0.99),
            max: sorted[sorted.len() - 1],
            mean,
            stddev: variance.sqrt(),
        }
    }
}

/// How the run was configured.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BenchSettings {
    /// Warm-up frames.
    pub warmup: usize,
    /// Measured frames.
    pub iterations: usize,
    /// Whether every frame re-rendered every view.
    pub refresh: bool,
    /// Window size.
    pub size: [f32; 2],
    /// Scale factor.
    pub scale: f32,
}

/// Whether a run got slower, faster, or neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Slower than the threshold allows.
    Regression,
    /// Faster by more than the threshold.
    Improvement,
    /// Within the threshold.
    Unchanged,
}

/// A run compared with the previous comparable run (same benchmark, build
/// profile and settings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// The baseline's commit.
    pub baseline_sha: String,
    /// When the baseline ran.
    pub baseline_timestamp: String,
    /// The baseline's median, in ms.
    pub baseline_p50: f64,
    /// Relative change of the median (`0.25` = 25 % slower).
    pub p50_change: f64,
    /// Relative change of the 95th percentile.
    pub p95_change: f64,
    /// The probability that a frame of this run is slower than a frame of
    /// the baseline (0.5: the same distribution; 1: every frame slower).
    #[serde(default = "even_odds")]
    pub slower_odds: f64,
    /// The verdict.
    pub verdict: Verdict,
}

impl fmt::Display for Comparison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let word = match self.verdict {
            Verdict::Regression => "regressed",
            Verdict::Improvement => "improved",
            Verdict::Unchanged => "unchanged",
        };
        write!(
            f,
            "{word}: p50 {:+.1}%, p95 {:+.1}% vs {} ({:.3} ms); {:.0}% of frames slower",
            self.p50_change * 100.,
            self.p95_change * 100.,
            self.baseline_sha,
            self.baseline_p50,
            self.slower_odds * 100.
        )
    }
}

/// One run of a benchmark.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchRun {
    /// Benchmark name.
    pub name: String,
    /// Commit, time and build profile.
    pub run: RunInfo,
    /// How it was configured.
    pub settings: BenchSettings,
    /// Per-frame draw time.
    pub stats: Stats,
    /// Per-phase times, when the engine recorded them.
    pub phases: Option<BTreeMap<String, Stats>>,
    /// Every measured frame, in ms.
    pub samples: Vec<f64>,
    /// How it compares with the previous comparable run.
    pub comparison: Option<Comparison>,
}

/// A benchmark's runs, oldest first, stored as `<history_dir>/<name>.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BenchHistory {
    /// Benchmark name.
    pub name: String,
    /// Runs, oldest first.
    pub runs: Vec<BenchRun>,
}

impl BenchHistory {
    /// The history file for `name` in `dir`.
    pub fn path(dir: &Path, name: &str) -> PathBuf {
        dir.join(format!("{}.json", slug(name)))
    }

    /// Loads a history, or an empty one if the file doesn't exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    /// Every history in `dir`, sorted by name.
    pub fn all(dir: &Path) -> Result<Vec<Self>> {
        let mut histories = Vec::new();
        if !dir.is_dir() {
            return Ok(histories);
        }
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                histories.push(Self::load(&path)?);
            }
        }
        histories.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(histories)
    }

    /// Compares `run` with the latest run of the same build profile and
    /// settings, stores the comparison on it, and appends it.
    pub fn record(&mut self, mut run: BenchRun, threshold: f64) -> &BenchRun {
        let baseline = self.runs.iter().rev().find(|previous| {
            previous.run.profile == run.run.profile && previous.settings == run.settings
        });
        run.comparison = baseline.map(|baseline| compare(baseline, &run, threshold));
        self.name.clone_from(&run.name);
        self.runs.push(run);
        &self.runs[self.runs.len() - 1]
    }

    /// Writes the history as pretty JSON.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }
}

/// Compares medians (and reports p95). A regression must be real on three
/// counts: the median moved by more than the relative `threshold`, by more
/// than a 10 µs floor, and the whole distribution shifted — at least 75 % odds
/// that a frame of this run is slower than one of the baseline — so a few
/// noisy frames (another process grabbing the CPU) don't flag a regression.
pub fn compare(baseline: &BenchRun, run: &BenchRun, threshold: f64) -> Comparison {
    let change = |old: f64, new: f64| if old > 0. { new / old - 1. } else { 0. };
    let p50_change = change(baseline.stats.p50, run.stats.p50);
    let p95_change = change(baseline.stats.p95, run.stats.p95);
    let delta = run.stats.p50 - baseline.stats.p50;
    let slower_odds = slower_odds(&baseline.samples, &run.samples);
    let verdict = if p50_change > threshold && delta > 0.01 && slower_odds >= 0.75 {
        Verdict::Regression
    } else if p50_change < -threshold && -delta > 0.01 && slower_odds <= 0.25 {
        Verdict::Improvement
    } else {
        Verdict::Unchanged
    };
    Comparison {
        baseline_sha: baseline.run.git_sha.clone(),
        baseline_timestamp: baseline.run.timestamp.clone(),
        baseline_p50: baseline.stats.p50,
        p50_change,
        p95_change,
        slower_odds,
        verdict,
    }
}

/// Histories written before odds were recorded read as unchanged.
fn even_odds() -> f64 {
    0.5
}

/// P(a sample of `new` > a sample of `old`), ties counting half: the
/// Mann–Whitney U statistic as a probability.
fn slower_odds(old: &[f64], new: &[f64]) -> f64 {
    if old.is_empty() || new.is_empty() {
        return 0.5;
    }
    let mut old = old.to_vec();
    old.sort_by(f64::total_cmp);
    let wins = new
        .iter()
        .map(|sample| {
            let below = old.partition_point(|value| value < sample);
            let ties = old[below..].partition_point(|value| value <= sample);
            below as f64 + ties as f64 / 2.
        })
        .sum::<f64>();
    wins / (old.len() * new.len()) as f64
}

/// A finished benchmark: the run and where its history is.
#[derive(Clone, Debug)]
pub struct BenchReport {
    /// This run, with its comparison.
    pub run: BenchRun,
    /// The history file it was appended to.
    pub history: PathBuf,
}

impl BenchReport {
    /// Whether this run regressed against the previous comparable run.
    pub fn regressed(&self) -> bool {
        self.run
            .comparison
            .as_ref()
            .is_some_and(|comparison| comparison.verdict == Verdict::Regression)
    }

    /// Asserts this run didn't regress.
    #[track_caller]
    pub fn assert_no_regression(&self) -> &Self {
        if let Some(comparison) = self.run.comparison.as_ref().filter(|_| self.regressed()) {
            panic!("lightbox: bench {:?} {comparison}", self.run.name);
        }
        self
    }
}

impl fmt::Display for BenchReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stats = &self.run.stats;
        write!(
            f,
            "{}: p50 {:.3} ms · p95 {:.3} · p99 {:.3} · max {:.3} · mean {:.3} ± {:.3} ({} frames)",
            self.run.name,
            stats.p50,
            stats.p95,
            stats.p99,
            stats.max,
            stats.mean,
            stats.stddev,
            stats.count
        )?;
        if let Some(comparison) = &self.run.comparison {
            write!(f, "\n  {comparison}")?;
        }
        Ok(())
    }
}

/// Benchmarks a view's frames: `setup` mounts it and returns the step to run
/// before each frame (a state change, input, or nothing); Lightbox then times
/// the frame's draw (render, layout, prepaint and paint; not GPU work).
///
/// Results are appended to `target/lightbox/bench/<name>.json` with the
/// commit, time and build profile, and compared with the previous run of the
/// same profile. Run benchmarks with `--release` (`just bench-ui`) for
/// meaningful numbers.
///
/// ```ignore
/// let report = lightbox::bench("list/scroll", BenchSpec::default(), |stage| {
///     let list = stage.mount(|_, cx| cx.new(|_| List::new(1_000)));
///     move |_, cx| list.update(cx, |list, cx| list.scroll_by(24., cx))
/// });
/// println!("{report}");
/// ```
pub fn bench<Step>(
    name: &str,
    spec: BenchSpec,
    setup: impl FnOnce(&mut Stage) -> Step,
) -> BenchReport
where
    Step: FnMut(&mut Window, &mut App),
{
    let _serial = BENCH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = spec
        .history_dir
        .clone()
        .unwrap_or_else(|| root().join("bench"));
    let mut stage = Stage::in_suite(Suite::at("bench", dir.clone()), spec.stage.clone());
    let mut step = setup(&mut stage);
    if spec.phases {
        stage.record_elements();
    }

    let mut samples = Vec::with_capacity(spec.iterations);
    let mut phases: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut last_frame = None;
    for iteration in 0..spec.warmup + spec.iterations {
        let (elapsed, timings) = stage.update(|window, cx| {
            step(window, cx);
            if spec.refresh {
                window.refresh();
            }
            let start = Instant::now();
            window.draw(cx).clear(cx);
            let elapsed = start.elapsed();
            let timings = window
                .inspector_capture()
                .and_then(|capture| capture.latest_frame())
                .filter(|frame| Some(frame.id) != last_frame)
                .map(|frame| (frame.id, frame.timings));
            (elapsed, timings)
        });
        if iteration < spec.warmup {
            continue;
        }
        samples.push(ms(elapsed));
        if let Some((id, timings)) = timings {
            last_frame = Some(id);
            for (phase, duration) in [
                ("render", timings.render),
                ("layout", timings.layout),
                ("prepaint", timings.prepaint),
                ("paint", timings.paint),
            ] {
                phases.entry(phase.into()).or_default().push(ms(duration));
            }
        }
    }

    let config = stage.config();
    let phases = phases
        .into_iter()
        .filter(|(_, samples)| samples.iter().any(|sample| *sample > 0.))
        .map(|(phase, samples)| (phase, Stats::of(&samples)))
        .collect::<BTreeMap<_, _>>();
    let run = BenchRun {
        name: name.into(),
        run: RunInfo::current(),
        settings: BenchSettings {
            warmup: spec.warmup,
            iterations: spec.iterations,
            refresh: spec.refresh,
            size: [config.size.width.as_f32(), config.size.height.as_f32()],
            scale: config.scale,
        },
        stats: Stats::of(&samples),
        phases: (!phases.is_empty()).then_some(phases),
        samples,
        comparison: None,
    };
    let path = BenchHistory::path(&dir, name);
    let result = BenchHistory::load(&path).and_then(|mut history| {
        let run = history.record(run, spec.regression_threshold).clone();
        history.save(&path)?;
        Ok(run)
    });
    match result {
        Ok(run) => BenchReport { run, history: path },
        Err(error) => panic!("lightbox: saving bench {name:?}: {error:#}"),
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_use_nearest_rank_percentiles() {
        let samples = (1..=100).map(f64::from).collect::<Vec<_>>();
        let stats = Stats::of(&samples);
        assert_eq!(
            (stats.min, stats.p50, stats.p95, stats.p99, stats.max),
            (1., 50., 95., 99., 100.)
        );
        assert_eq!(stats.mean, 50.5);
        assert!((stats.stddev - 28.866).abs() < 1e-3);
    }

    #[test]
    fn odds_measure_a_shifted_distribution() {
        let old = [1., 2., 3., 4.];
        assert_eq!(slower_odds(&old, &old), 0.5);
        assert_eq!(slower_odds(&old, &[10., 11.]), 1.);
        assert_eq!(slower_odds(&old, &[0.5]), 0.);
    }
}
