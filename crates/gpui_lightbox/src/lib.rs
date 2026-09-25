//! Lightbox: visual testing for gpui — screenshots, variant matrices,
//! filmstrips of animations, style lint, golden images and UI benchmarks,
//! with a report to review them all.
//!
//! Everything starts on a [`Stage`]: a headless, deterministic window with
//! bundled fonts, a fake clock and real input dispatch.
//!
//! ```ignore
//! use gpui_lightbox::{FilmSpec, Stage, StyleSpec};
//!
//! let mut stage = Stage::new("settings");
//! stage.mount(|_, cx| cx.new(|cx| Settings::new(cx)));
//!
//! // A screenshot, linted against the design's style and compared with a golden.
//! let shot = stage.click_text("Advanced").shot("advanced");
//! shot.lint(&StyleSpec::loupe()).assert_clean();
//! shot.assert_golden("settings-advanced");
//!
//! // An animation, sampled at exact times on the fake clock.
//! let mut film = stage.film("collapse", FilmSpec::fps(ms(300), 30.), |stage| {
//!     stage.click_text("Collapse");
//! });
//! film.track("height", |frame| frame.find_text("Advanced").ok().map(|t| t.bounds.y));
//! film.assert_monotonic("height").assert_settles_by(ms(300));
//! ```
//!
//! Output goes to `target/lightbox/<suite>/`; `cargo run -p gpui_ce_lightbox
//! -- report` builds contact sheets and `target/lightbox/index.html`. See the
//! README for the review workflow.

pub mod bench;
pub mod color;
pub mod compose;
pub mod film;
pub mod golden;
mod input;
pub mod lint;
pub mod manifest;
pub mod matrix;
pub mod output;
pub mod report;
pub mod sheet;
pub mod shot;
pub mod stage;
pub mod theme;

pub use bench::{BenchReport, BenchSpec, bench};
pub use color::Color;
pub use film::{Film, FilmSpec};
pub use golden::GoldenTolerance;
pub use lint::{LintReport, Rule, Severity, SpacingRule, StyleSpec, TextRole};
pub use matrix::{Matrix, Variant};
pub use output::Suite;
pub use shot::{QuadInfo, Rect, Shot, TextLine};
pub use stage::{Appearance, Fonts, MONO_FONT, Stage, StageConfig, UI_FONT};
