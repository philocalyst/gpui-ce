//! Capture controls and Loupe's self-cost, shared by the Frames and Audit
//! lenses and the settings popover: the frame budget (which grades every
//! frame), the capture level (what each frame records) and what recording
//! costs.
//!
//! The controls change [`LoupeSettings`]; every open Loupe copies the new
//! values into its window's capture and re-renders.

use crate::{
    analysis::format,
    settings::{
        CAPTURE_LEVELS, FrameBudget, LoupeSettings, capture_level_label, capture_level_summary,
    },
    theme::{MONO_FONT, Theme},
    widgets::{Segment, Segmented},
};
use gpui::{
    App, IntoElement, RenderOnce, SharedString, Styled, Window, div, inspector::CaptureLevel,
    prelude::*, px,
};
use std::time::Duration;

/// `60 Hz | 120 Hz | 144 Hz`, setting the frame budget.
pub(crate) fn budget_control(id: &'static str, budget: FrameBudget) -> Segmented {
    FrameBudget::ALL
        .iter()
        .fold(Segmented::new(id), |control, budget| {
            control.segment(Segment::label(budget.label()).tooltip(format!(
                "Grade frames against a {} budget",
                format::duration(budget.duration())
            )))
        })
        .selected(
            FrameBudget::ALL
                .iter()
                .position(|b| *b == budget)
                .unwrap_or(0),
        )
        .on_select(|ix, _, cx| {
            LoupeSettings::update(cx, |settings| settings.budget = FrameBudget::ALL[*ix]);
        })
}

/// `Frames | Tree | Full`, setting what the capture records.
pub(crate) fn level_control(id: &'static str, level: CaptureLevel) -> Segmented {
    CAPTURE_LEVELS
        .iter()
        .fold(Segmented::new(id), |control, level| {
            control.segment(
                Segment::label(capture_level_label(*level)).tooltip(capture_level_summary(*level)),
            )
        })
        .selected(CAPTURE_LEVELS.iter().position(|l| *l == level).unwrap_or(0))
        .on_select(|ix, _, cx| {
            LoupeSettings::update(cx, |settings| {
                settings.capture_level = CAPTURE_LEVELS[*ix];
            });
        })
}

/// What recording costs, and the controls that change it.
#[derive(IntoElement)]
pub(crate) struct CaptureCard {
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
        div()
            .flex()
            .flex_col()
            .pb_1()
            .child(
                row("Level")
                    .child(level_control("loupe-capture-level", self.level))
                    .child(note(capture_level_summary(self.level).into())),
            )
            .child(
                row("Budget")
                    .child(budget_control(
                        "loupe-capture-budget",
                        FrameBudget::nearest(self.budget),
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
