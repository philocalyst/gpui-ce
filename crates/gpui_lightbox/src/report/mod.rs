//! The report: `target/lightbox/index.html`, built from every suite's records
//! and every benchmark history, with a contact sheet per suite.
//!
//! One static page (HTML, CSS and a little vanilla JS, all inline) that works
//! from `file://`: images are referenced relatively and lazy-loaded, and the
//! data for the viewer (painted text, quads, lint, films, goldens) is embedded
//! as JSON.

pub mod html;
pub mod svg;

use crate::{
    bench::{BenchHistory, Verdict},
    compose::Composer,
    lint::Severity,
    manifest::Record,
    output::{RunInfo, Suite},
    sheet::write_suite_sheet,
};
use anyhow::{Context as _, Result};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

/// A suite as the report shows it.
pub struct SuiteData {
    /// The suite.
    pub suite: Suite,
    /// Its directory's name under the root, which prefixes its image paths.
    pub dir_name: String,
    /// Its records.
    pub records: Vec<Record>,
    /// Its contact sheet pages, relative to the root.
    pub sheets: Vec<String>,
}

/// Everything a report shows.
pub struct Report {
    /// The build the report was made from.
    pub run: RunInfo,
    /// Suites, by name.
    pub suites: Vec<SuiteData>,
    /// Benchmark histories, by name.
    pub benches: Vec<BenchHistory>,
}

impl Report {
    /// Reads every suite and benchmark history under `root`.
    pub fn collect(root: &Path) -> Result<Self> {
        let mut suites = Vec::new();
        for suite in Suite::all(root)? {
            let dir_name = suite
                .dir()
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let records = suite.records()?;
            suites.push(SuiteData {
                suite,
                dir_name,
                records,
                sheets: Vec::new(),
            });
        }
        Ok(Self {
            run: RunInfo::current(),
            suites,
            benches: BenchHistory::all(&root.join("bench"))?,
        })
    }

    /// Counts for the summary.
    pub fn summary(&self) -> Summary {
        let mut summary = Summary {
            suites: self.suites.len(),
            benches: self.benches.len(),
            ..Summary::default()
        };
        for record in self.suites.iter().flat_map(|suite| &suite.records) {
            match record {
                Record::Shot(_) => summary.shots += 1,
                Record::Matrix(_) => summary.matrices += 1,
                Record::Film(film) => {
                    summary.films += 1;
                    if film.assertions.iter().any(|assertion| !assertion.passed) {
                        summary.failed_films += 1;
                    }
                }
                Record::Lint(lint) => {
                    summary.lints += 1;
                    summary.violations += lint.violations.len();
                    if lint
                        .violations
                        .iter()
                        .any(|violation| violation.severity == Severity::Error)
                    {
                        summary.lints_with_errors += 1;
                    }
                }
                Record::Golden(golden) => {
                    summary.goldens += 1;
                    if !golden.passed {
                        summary.failed_goldens += 1;
                    }
                }
            }
        }
        summary.regressions = self
            .benches
            .iter()
            .filter(|history| {
                history
                    .runs
                    .last()
                    .and_then(|run| run.comparison.as_ref())
                    .is_some_and(|comparison| comparison.verdict == Verdict::Regression)
            })
            .count();
        summary
    }
}

/// What a report contains.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// Suites.
    pub suites: usize,
    /// Screenshots.
    pub shots: usize,
    /// Variant matrices.
    pub matrices: usize,
    /// Films.
    pub films: usize,
    /// Films with a failed assertion.
    pub failed_films: usize,
    /// Lints.
    pub lints: usize,
    /// Lints with at least one error-severity violation.
    pub lints_with_errors: usize,
    /// Violations across all lints.
    pub violations: usize,
    /// Golden comparisons.
    pub goldens: usize,
    /// Golden comparisons that failed.
    pub failed_goldens: usize,
    /// Benchmarks.
    pub benches: usize,
    /// Benchmarks whose latest run regressed.
    pub regressions: usize,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} suites · {} shots · {} films ({} failed) · {} lints ({} violations) · {} goldens ({} failed) · {} benchmarks ({} regressed)",
            self.suites,
            self.shots,
            self.films,
            self.failed_films,
            self.lints,
            self.violations,
            self.goldens,
            self.failed_goldens,
            self.benches,
            self.regressions
        )
    }
}

/// Builds contact sheets for every suite and `index.html` under `root`.
/// Returns the index's path and what it contains.
pub fn build(root: &Path) -> Result<(PathBuf, Summary)> {
    let mut report = Report::collect(root)?;
    if !report.suites.is_empty() {
        let mut composer = Composer::new().context("starting the composer")?;
        for suite in &mut report.suites {
            let pages = write_suite_sheet(&suite.suite, &mut composer)
                .with_context(|| format!("contact sheet for {}", suite.suite.name()))?;
            suite.sheets = pages
                .iter()
                .filter_map(|page| page.file_name())
                .map(|file| format!("{}/{}", suite.dir_name, file.to_string_lossy()))
                .collect();
        }
    }
    let page = html::render(&report, root)?;
    fs::create_dir_all(root)?;
    let index = root.join("index.html");
    fs::write(&index, page).with_context(|| format!("writing {}", index.display()))?;
    Ok((index, report.summary()))
}
