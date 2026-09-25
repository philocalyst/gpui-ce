//! Loupe's preferences: one app-wide [`LoupeSettings`] global.
//!
//! Settings outlive any one Loupe and apply to every inspected window. The
//! user changes them in the settings popover (the gear in the toolbar) or
//! the palette; they last for the app session. An app picks its own
//! defaults when it registers Loupe:
//!
//! ```ignore
//! gpui_inspector::init_with(
//!     cx,
//!     LoupeSettings {
//!         density: Density::Comfortable,
//!         editor: Editor::VsCode.into(),
//!         budget: FrameBudget::Hz120,
//!         ..LoupeSettings::default()
//!     },
//! );
//! ```
//!
//! The frame budget and capture level are also the capture's own tuning
//! ([`gpui::inspector::CaptureConfig`]): Loupe copies them into each
//! window's capture when it opens and whenever they change.

use crate::{
    analysis::source::{Editor, EditorUrl},
    theme::{Appearance, Density},
};
use gpui::{App, Global, inspector::CaptureLevel};
use std::time::Duration;

/// Loupe's user preferences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoupeSettings {
    /// Palette: follow the window, or always dark or light.
    pub appearance: Appearance,
    /// Row and control sizing.
    pub density: Density,
    /// How source links open in an editor.
    pub editor: EditorUrl,
    /// The refresh rate frames are graded against.
    pub budget: FrameBudget,
    /// What each frame records.
    pub capture_level: CaptureLevel,
}

/// The defaults, readable without a global.
static DEFAULTS: LoupeSettings = LoupeSettings::DEFAULT;

impl LoupeSettings {
    /// Follow the window's appearance, compact rows, open links in Zed,
    /// grade frames against 60 Hz and record everything.
    pub const DEFAULT: LoupeSettings = LoupeSettings {
        appearance: Appearance::System,
        density: Density::Compact,
        editor: EditorUrl::Editor(Editor::Zed),
        budget: FrameBudget::Hz60,
        capture_level: CaptureLevel::Full,
    };

    /// The current settings, or the defaults if none were set.
    pub fn get(cx: &App) -> &Self {
        cx.try_global::<Self>().unwrap_or(&DEFAULTS)
    }

    /// Changes the settings; every open Loupe restyles and retunes its
    /// capture.
    pub fn update(cx: &mut App, change: impl FnOnce(&mut Self)) {
        let mut settings = Self::get(cx).clone();
        change(&mut settings);
        if *Self::get(cx) != settings {
            cx.set_global(settings);
        }
    }
}

impl Default for LoupeSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Global for LoupeSettings {}

/// A display refresh rate: each frame has one refresh interval to finish.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FrameBudget {
    /// 16.7 ms per frame.
    #[default]
    Hz60,
    /// 8.3 ms per frame.
    Hz120,
    /// 6.9 ms per frame.
    Hz144,
}

impl FrameBudget {
    /// Every budget, slowest display first.
    pub const ALL: [FrameBudget; 3] = [FrameBudget::Hz60, FrameBudget::Hz120, FrameBudget::Hz144];

    /// Refreshes per second.
    pub fn hz(self) -> u32 {
        match self {
            FrameBudget::Hz60 => 60,
            FrameBudget::Hz120 => 120,
            FrameBudget::Hz144 => 144,
        }
    }

    /// The time one frame may take, to the microsecond.
    pub fn duration(self) -> Duration {
        Duration::from_micros(match self {
            FrameBudget::Hz60 => 16_667,
            FrameBudget::Hz120 => 8_333,
            FrameBudget::Hz144 => 6_944,
        })
    }

    /// `60 Hz`.
    pub fn label(self) -> &'static str {
        match self {
            FrameBudget::Hz60 => "60 Hz",
            FrameBudget::Hz120 => "120 Hz",
            FrameBudget::Hz144 => "144 Hz",
        }
    }

    /// The budget closest to `duration`.
    pub fn nearest(duration: Duration) -> Self {
        Self::ALL
            .into_iter()
            .min_by_key(|budget| budget.duration().abs_diff(duration))
            .unwrap_or_default()
    }
}

/// Every capture level, cheapest first.
pub const CAPTURE_LEVELS: [CaptureLevel; 3] =
    [CaptureLevel::Frames, CaptureLevel::Tree, CaptureLevel::Full];

/// The name of a capture level, as its control shows it.
pub fn capture_level_label(level: CaptureLevel) -> &'static str {
    match level {
        CaptureLevel::Frames => "Frames",
        CaptureLevel::Tree => "Tree",
        CaptureLevel::Full => "Full",
    }
}

/// One line on what a capture level records and what that costs.
pub fn capture_level_summary(level: CaptureLevel) -> &'static str {
    match level {
        CaptureLevel::Frames => "Timings, causes and renders. Cheapest.",
        CaptureLevel::Tree => "Adds element trees for picking. Costs more.",
        CaptureLevel::Full => "Adds styles, text and a11y details. Costliest.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_snap_to_the_nearest_rate() {
        assert_eq!(
            FrameBudget::nearest(Duration::from_micros(16_667)),
            FrameBudget::Hz60
        );
        assert_eq!(
            FrameBudget::nearest(Duration::from_millis(16)),
            FrameBudget::Hz60
        );
        assert_eq!(
            FrameBudget::nearest(Duration::from_millis(8)),
            FrameBudget::Hz120
        );
        assert_eq!(
            FrameBudget::nearest(Duration::from_millis(7)),
            FrameBudget::Hz144
        );
        assert_eq!(
            FrameBudget::nearest(Duration::from_millis(1)),
            FrameBudget::Hz144
        );
        assert_eq!(
            FrameBudget::nearest(Duration::from_secs(1)),
            FrameBudget::Hz60
        );
        for budget in FrameBudget::ALL {
            assert_eq!(FrameBudget::nearest(budget.duration()), budget);
            let hz = budget.duration().as_secs_f64().recip().round() as u32;
            assert_eq!(hz, budget.hz(), "{budget:?}");
        }
    }

    #[test]
    fn defaults_follow_the_design_contract() {
        let settings = LoupeSettings::default();
        assert_eq!(settings.appearance, Appearance::System);
        assert_eq!(settings.density, Density::Compact);
        assert_eq!(settings.editor, EditorUrl::from(Editor::Zed));
        assert_eq!(settings.budget.duration(), Duration::from_micros(16_667));
        assert_eq!(settings.capture_level, CaptureLevel::Full);
    }

    #[test]
    fn every_capture_level_says_what_it_records_and_costs() {
        for level in CAPTURE_LEVELS {
            let summary = capture_level_summary(level);
            assert!(summary.ends_with('.'), "{summary}");
            assert!(summary.len() <= 48, "one short line: {summary}");
        }
    }
}
