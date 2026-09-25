//! Capture controls and Loupe's self-cost, shared by the Frames and Audit
//! lenses: the frame budget (which grades every frame), the capture level
//! (what each frame records) and what recording costs.

use crate::{
    analysis::format,
    state::LoupeState,
    theme::{MONO_FONT, Theme},
    widgets::{Segment, Segmented},
};
use gpui::{
    App, Entity, IntoElement, RenderOnce, SharedString, Styled, Window, div,
    inspector::CaptureLevel, prelude::*, px,
};
use std::time::Duration;

/// The frame budgets Loupe offers: refresh rate and time per frame.
pub(crate) const BUDGETS: [(u32, Duration); 3] = [
    (60, Duration::from_micros(16_667)),
    (120, Duration::from_micros(8_333)),
    (144, Duration::from_micros(6_944)),
];

/// The capture levels, cheapest first, with what each adds.
pub(crate) const LEVELS: [(CaptureLevel, &str, &str); 3] = [
    (
        CaptureLevel::Frames,
        "Frames",
        "Timings, causes and view renders only",
    ),
    (
        CaptureLevel::Tree,
        "Tree",
        "Plus element trees: picking, layout and hit checks",
    ),
    (
        CaptureLevel::Full,
        "Full",
        "Plus element details: styles, text, contrast and accessibility checks",
    ),
];

/// The offered budget closest to `budget`.
pub(crate) fn budget_index(budget: Duration) -> usize {
    BUDGETS
        .iter()
        .enumerate()
        .min_by_key(|(_, (_, offered))| offered.abs_diff(budget))
        .map_or(0, |(ix, _)| ix)
}

/// The position of `level` in [`LEVELS`].
pub(crate) fn level_index(level: CaptureLevel) -> usize {
    LEVELS
        .iter()
        .position(|(offered, _, _)| *offered == level)
        .unwrap_or(0)
}

/// Re-renders the shell and every lens: grades and memo keys changed
/// without the capture's generation moving.
fn refresh_everything(state: &Entity<LoupeState>, cx: &mut App) {
    state.update(cx, |_, cx| cx.notify());
}

/// `60 Hz | 120 Hz | 144 Hz`, setting the capture's frame budget.
pub(crate) fn budget_control(
    id: &'static str,
    budget: Duration,
    state: &Entity<LoupeState>,
) -> Segmented {
    let state = state.clone();
    BUDGETS
        .iter()
        .fold(Segmented::new(id), |control, (hz, per_frame)| {
            control.segment(Segment::label(format!("{hz} Hz")).tooltip(format!(
                "Grade frames against a {} budget",
                format::duration(*per_frame)
            )))
        })
        .selected(budget_index(budget))
        .on_select(move |ix, window, cx| {
            if let Some(capture) = window.inspector_capture_mut() {
                capture.config_mut().budget = BUDGETS[*ix].1;
            }
            refresh_everything(&state, cx);
        })
}

/// `Frames | Tree | Full`, setting what the capture records.
pub(crate) fn level_control(
    id: &'static str,
    level: CaptureLevel,
    state: &Entity<LoupeState>,
) -> Segmented {
    let state = state.clone();
    LEVELS
        .iter()
        .fold(Segmented::new(id), |control, (_, label, what)| {
            control.segment(Segment::label(*label).tooltip(*what))
        })
        .selected(level_index(level))
        .on_select(move |ix, window, cx| {
            if let Some(capture) = window.inspector_capture_mut() {
                capture.config_mut().level = LEVELS[*ix].0;
            }
            refresh_everything(&state, cx);
        })
}

/// What recording costs, and the controls that change it.
#[derive(IntoElement)]
pub(crate) struct CaptureCard {
    /// Shared state, notified when a control changes the capture.
    pub state: Entity<LoupeState>,
    /// Current capture level.
    pub level: CaptureLevel,
    /// Current frame budget.
    pub budget: Duration,
    /// `InspectorCapture::retained_bytes`.
    pub retained: usize,
    /// Share of all drawing Loupe caused, 0.0..=1.0.
    pub overhead: f64,
    /// Loupe's mean draw time per frame.
    pub loupe_per_frame: Duration,
    /// Frames and input records in the rings.
    pub frames: usize,
    /// Input records in the ring.
    pub inputs: usize,
}

impl RenderOnce for CaptureCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let row = |label: &'static str| {
            div()
                .min_h(theme.metrics.property_row)
                .px(theme.metrics.gutter)
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .w(px(64.))
                        .text_color(colors.text_muted)
                        .child(label),
                )
        };
        let value = |text: String| {
            div()
                .flex_none()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(colors.text)
                .child(text)
        };
        let note = |text: SharedString| {
            div()
                .min_w_0()
                .truncate()
                .text_size(theme.metrics.text_small)
                .text_color(colors.text_faint)
                .child(text)
        };
        let level_note = LEVELS[level_index(self.level)].2;
        div()
            .flex()
            .flex_col()
            .pb_1()
            .child(
                row("Level")
                    .child(level_control(
                        "loupe-capture-level",
                        self.level,
                        &self.state,
                    ))
                    .child(note(level_note.into())),
            )
            .child(
                row("Budget")
                    .child(budget_control(
                        "loupe-capture-budget",
                        self.budget,
                        &self.state,
                    ))
                    .child(note(
                        format!("{} per frame", format::duration(self.budget)).into(),
                    )),
            )
            .child(
                row("Retained")
                    .child(value(format::bytes(self.retained as u64)))
                    .child(note(
                        format!(
                            "{} frames · {} input records",
                            format::count(self.frames as u64),
                            format::count(self.inputs as u64)
                        )
                        .into(),
                    )),
            )
            .child(
                row("Overhead")
                    .child(value(format::percent(self.overhead)))
                    .child(note(
                        format!(
                            "of all drawing · {} of Loupe per frame",
                            format::duration(self.loupe_per_frame)
                        )
                        .into(),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_snap_to_the_nearest_rate() {
        assert_eq!(budget_index(Duration::from_micros(16_667)), 0);
        assert_eq!(budget_index(Duration::from_millis(16)), 0);
        assert_eq!(budget_index(Duration::from_millis(8)), 1);
        assert_eq!(budget_index(Duration::from_millis(7)), 2);
        assert_eq!(budget_index(Duration::from_millis(1)), 2);
        assert_eq!(budget_index(Duration::from_secs(1)), 0);
    }

    #[test]
    fn levels_round_trip() {
        for (ix, (level, _, _)) in LEVELS.iter().enumerate() {
            assert_eq!(level_index(*level), ix);
        }
    }
}
