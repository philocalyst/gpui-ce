//! The JSON records tests write and the report reads: the contract between
//! the two. One record per artifact, stored by [`Suite::write_record`].
//!
//! [`Suite::write_record`]: crate::Suite::write_record

use crate::{
    film::{FrameMetrics, Track},
    golden::{DiffStats, GoldenTolerance},
    lint::{LintCounts, Violation},
    matrix::MatrixCell,
    output::now_ms,
    shot::{QuadInfo, ShotMeta, TextLine},
};
use serde::{Deserialize, Serialize};
use std::panic::Location;

/// Where and when an artifact was produced.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    /// The test that produced it (the test thread's name).
    pub test: Option<String>,
    /// The `file:line` of the call that produced it.
    pub location: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub created_ms: u64,
}

impl Origin {
    /// The origin of an artifact produced by the call at `location`.
    pub fn at(location: &Location<'_>) -> Self {
        let test = std::thread::current()
            .name()
            .filter(|name| *name != "main")
            .map(str::to_string);
        Self {
            test,
            location: Some(format!("{}:{}", location.file(), location.line())),
            created_ms: now_ms(),
        }
    }
}

/// One artifact in a suite.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// A screenshot.
    Shot(ShotRecord),
    /// A grid of variants of one view.
    Matrix(MatrixRecord),
    /// An animation sampled at fixed times.
    Film(FilmRecord),
    /// A style lint of a shot.
    Lint(LintRecord),
    /// A comparison against a golden image.
    Golden(GoldenRecord),
}

impl Record {
    /// The artifact's name, unique per kind within its suite.
    pub fn name(&self) -> &str {
        match self {
            Record::Shot(record) => &record.name,
            Record::Matrix(record) => &record.name,
            Record::Film(record) => &record.name,
            Record::Lint(record) => &record.name,
            Record::Golden(record) => &record.name,
        }
    }

    /// The kind, as used in record file names.
    pub fn kind(&self) -> &'static str {
        match self {
            Record::Shot(_) => "shot",
            Record::Matrix(_) => "matrix",
            Record::Film(_) => "film",
            Record::Lint(_) => "lint",
            Record::Golden(_) => "golden",
        }
    }

    /// Where the artifact came from.
    pub fn origin(&self) -> &Origin {
        match self {
            Record::Shot(record) => &record.origin,
            Record::Matrix(record) => &record.origin,
            Record::Film(record) => &record.origin,
            Record::Lint(record) => &record.origin,
            Record::Golden(record) => &record.origin,
        }
    }
}

/// A saved screenshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotRecord {
    /// Shot name.
    pub name: String,
    /// PNG file, relative to the suite directory.
    pub image: String,
    /// How the shot was rendered.
    pub meta: ShotMeta,
    /// Painted text, for overlays.
    pub texts: Vec<TextLine>,
    /// Painted quads, for overlays.
    pub quads: Vec<QuadInfo>,
    /// Where it came from.
    pub origin: Origin,
}

/// A labeled grid of variants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatrixRecord {
    /// Matrix name.
    pub name: String,
    /// The grid PNG, relative to the suite directory.
    pub image: String,
    /// One cell per variant.
    pub cells: Vec<MatrixCell>,
    /// Where it came from.
    pub origin: Origin,
}

/// A filmed animation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilmRecord {
    /// Film name.
    pub name: String,
    /// The strip PNG (frames with timestamps, then curves), relative to the suite.
    pub strip: String,
    /// The animated PNG, relative to the suite.
    pub animation: String,
    /// Per-frame images and metrics.
    pub frames: Vec<FrameMetrics>,
    /// Tracked properties.
    pub tracks: Vec<Track>,
    /// What the detectors found.
    pub findings: Vec<Finding>,
    /// Assertions made on the film, in order.
    pub assertions: Vec<Assertion>,
    /// Total filmed time in ms.
    pub duration_ms: f64,
    /// Logical window size.
    pub size: [f32; 2],
    /// Device pixels per logical pixel.
    pub scale: f32,
    /// Where it came from.
    pub origin: Origin,
}

/// A detector's finding about a film.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// What was found.
    pub kind: FindingKind,
    /// The track it concerns, if any.
    pub track: Option<String>,
    /// The frame where it happens.
    pub frame: usize,
    /// A plain-language explanation.
    pub message: String,
}

/// Kinds of film findings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Nothing moved for a stretch, then motion resumed: a hitch.
    Frozen,
    /// A value changed far more in one frame than in its neighbors.
    Jump,
    /// A value reversed direction.
    NonMonotonic,
    /// A value went past where it finally settled.
    Overshoot,
}

impl FindingKind {
    /// A short label.
    pub fn label(self) -> &'static str {
        match self {
            FindingKind::Frozen => "frozen",
            FindingKind::Jump => "jump",
            FindingKind::NonMonotonic => "non-monotonic",
            FindingKind::Overshoot => "overshoot",
        }
    }

    /// Whether this kind of finding usually means something is wrong (a
    /// spring's overshoot, say, is often intended).
    pub fn is_problem(self) -> bool {
        matches!(self, FindingKind::Frozen | FindingKind::Jump)
    }
}

/// The outcome of one assertion, kept so the report can show what was checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assertion {
    /// What was asserted, e.g. `monotonic(x)`.
    pub name: String,
    /// Whether it held.
    pub passed: bool,
    /// Details.
    pub message: String,
}

/// A style lint of a shot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LintRecord {
    /// The linted shot's name.
    pub name: String,
    /// The style spec's name.
    pub spec: String,
    /// The linted shot's PNG, relative to the suite.
    pub shot_image: String,
    /// The linted shot's logical size.
    pub size: [f32; 2],
    /// The linted shot's scale factor.
    pub scale: f32,
    /// The annotated PNG, relative to the suite.
    pub annotated: String,
    /// Violations, numbered in order (1-based in the annotation).
    pub violations: Vec<Violation>,
    /// How much was checked.
    pub checked: LintCounts,
    /// The assertion made on the report, if any.
    pub verdict: Option<Assertion>,
    /// Where it came from.
    pub origin: Origin,
}

/// A comparison against a golden image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoldenRecord {
    /// The compared shot's name.
    pub name: String,
    /// The golden file, relative to the workspace root.
    pub golden: String,
    /// A copy of the golden, relative to the suite (absent when just recorded).
    pub expected: Option<String>,
    /// The shot compared, relative to the suite.
    pub actual: String,
    /// The ΔE heatmap, relative to the suite, when the images differ.
    pub diff: Option<String>,
    /// Expected, actual and diff side by side, relative to the suite.
    pub comparison: Option<String>,
    /// Whether the shot matched within tolerance.
    pub passed: bool,
    /// Whether the golden was (re)recorded by this run.
    pub updated: bool,
    /// Pixel statistics.
    pub stats: DiffStats,
    /// The tolerance applied.
    pub tolerance: GoldenTolerance,
    /// A plain-language verdict.
    pub message: String,
    /// Where it came from.
    pub origin: Origin,
}
