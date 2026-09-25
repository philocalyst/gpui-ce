//! The phase bar: one frame's time as a stacked bar in the fixed phase hues,
//! with a tick where the budget runs out.
//!
//! Segments run in the order work happens: the input handled before the
//! frame, the app's render, layout, prepaint and paint, draw time no phase
//! claimed, Loupe's own drawing, then presenting. The budget tick sits one
//! budget after the input, so the app's phases cross it exactly when the
//! frame is graded over budget.

use super::text::shape;
use crate::{
    analysis::{format, stats},
    theme::{MONO_FONT, Phase, Theme},
};
use gpui::{
    App, Bounds, ColorExt as _, Hsla, IntoElement, Pixels, RenderOnce, Styled, TextAlign, Window,
    canvas, div, fill, inspector::PhaseTimings, point, prelude::*, px, size,
};
use std::time::Duration;

/// A phase that takes at least this share of the app's time dominates it.
pub(crate) const DOMINANT_SHARE: f64 = 0.5;

/// One stretch of the bar.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PhaseSegment {
    /// The phase, or `None` for draw time no phase claimed.
    pub phase: Option<Phase>,
    /// How long it took.
    pub duration: Duration,
    /// Where it starts, as a fraction of the bar.
    pub start: f32,
    /// How much of the bar it takes.
    pub width: f32,
}

impl PhaseSegment {
    /// `render`, or `other` for unclaimed time.
    pub fn name(&self) -> &'static str {
        self.phase.map_or("other", Phase::label)
    }

    /// `render 12.2`, milliseconds implied.
    pub fn label(&self) -> String {
        format!("{} {}", self.name(), format::millis(self.duration))
    }
}

/// The bar for one frame.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PhaseBar {
    /// Non-empty segments, in order.
    pub segments: Vec<PhaseSegment>,
    /// Where the budget runs out, as a fraction of the bar.
    pub budget_at: f32,
    /// The frame budget.
    pub budget: Duration,
    /// The app phase with the most time and its share of the app's time.
    pub largest: Option<(Phase, f64)>,
}

impl PhaseBar {
    /// Lays out `timings` against `budget`. The bar spans the frame or the
    /// budget, whichever is longer.
    pub fn new(timings: &PhaseTimings, budget: Duration) -> Self {
        let claimed = timings.render + timings.layout + timings.prepaint + timings.paint;
        let other = timings.total.saturating_sub(claimed + timings.inspector);
        let parts = [
            (Some(Phase::Input), timings.input),
            (Some(Phase::Render), timings.render),
            (Some(Phase::Layout), timings.layout),
            (Some(Phase::Prepaint), timings.prepaint),
            (Some(Phase::Paint), timings.paint),
            (None, other),
            (Some(Phase::Inspector), timings.inspector),
            (Some(Phase::Present), timings.present.unwrap_or_default()),
        ];
        let sum: Duration = parts.iter().map(|(_, duration)| *duration).sum();
        let scale = sum.max(timings.input + budget);
        let fraction = |duration: Duration| stats::ratio(duration, scale) as f32;

        let mut cursor = Duration::ZERO;
        let segments = parts
            .into_iter()
            .filter(|(_, duration)| !duration.is_zero())
            .map(|(phase, duration)| {
                let segment = PhaseSegment {
                    phase,
                    duration,
                    start: fraction(cursor),
                    width: fraction(duration),
                };
                cursor += duration;
                segment
            })
            .collect();

        let largest = [
            (Phase::Render, timings.render),
            (Phase::Layout, timings.layout),
            (Phase::Prepaint, timings.prepaint),
            (Phase::Paint, timings.paint),
        ]
        .into_iter()
        .filter(|(_, duration)| !duration.is_zero())
        .max_by_key(|(_, duration)| *duration)
        .map(|(phase, duration)| (phase, stats::ratio(duration, timings.app_total())));

        Self {
            segments,
            budget_at: fraction(timings.input + budget),
            budget,
            largest,
        }
    }

    /// The phase that took at least half the app's time, if one did.
    pub fn dominant(&self) -> Option<Phase> {
        self.largest
            .filter(|(_, share)| *share >= DOMINANT_SHARE)
            .map(|(phase, _)| phase)
    }

    /// One sentence on where the time went: `render took 62% of the app's time`.
    pub fn verdict(&self) -> Option<String> {
        let (phase, share) = self.largest?;
        let verb = if self.dominant().is_some() {
            "dominated"
        } else {
            "led"
        };
        Some(format!(
            "{} {verb} with {} of the app's time",
            capitalize(phase.label()),
            format::percent(share)
        ))
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The color of a segment.
pub(crate) fn segment_color(phase: Option<Phase>, theme: &Theme) -> Hsla {
    match phase {
        Some(phase) => theme.phase(phase),
        None => theme.colors.line_strong,
    }
}

const BAR_HEIGHT: f32 = 16.;
const TICK_OVERHANG: f32 = 3.;
const LABEL_PADDING: f32 = 5.;

/// Renders a [`PhaseBar`]: the stacked bar with inline labels where they
/// fit, the budget tick, and a legend with every phase's time.
#[derive(IntoElement)]
pub(crate) struct PhaseBarView {
    bar: PhaseBar,
}

impl PhaseBarView {
    /// A view of `bar`.
    pub fn new(bar: PhaseBar) -> Self {
        Self { bar }
    }
}

impl RenderOnce for PhaseBarView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let segments = self.bar.segments.clone();
        let budget_at = self.bar.budget_at;
        let legend = self.bar.segments.iter().map(|segment| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .size(px(8.))
                        .rounded(px(2.))
                        .bg(segment_color(segment.phase, theme)),
                )
                .child(div().text_color(colors.text_muted).child(segment.name()))
                .child(
                    div()
                        .font_family(MONO_FONT)
                        .text_size(theme.metrics.mono)
                        .text_color(colors.text)
                        .child(format::millis(segment.duration)),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        paint_bar(bounds, &segments, budget_at, theme, window, cx)
                    },
                )
                .w_full()
                .h(px(BAR_HEIGHT + 2. * TICK_OVERHANG)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(10.))
                    .gap_y(px(2.))
                    .text_size(theme.metrics.text_small)
                    .children(legend)
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(div().w(px(1.5)).h(px(10.)).bg(colors.text))
                            .child(div().text_color(colors.text_muted).child("budget"))
                            .child(
                                div()
                                    .font_family(MONO_FONT)
                                    .text_size(theme.metrics.mono)
                                    .text_color(colors.text)
                                    .child(format::millis(self.bar.budget)),
                            ),
                    ),
            )
            .children(self.bar.verdict().map(|verdict| {
                div()
                    .text_size(theme.metrics.text_small)
                    .text_color(colors.text_muted)
                    .child(verdict)
            }))
    }
}

fn paint_bar(
    bounds: Bounds<Pixels>,
    segments: &[PhaseSegment],
    budget_at: f32,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) {
    let colors = &theme.colors;
    let width = bounds.size.width;
    let bar = Bounds::new(
        point(bounds.left(), bounds.top() + px(TICK_OVERHANG)),
        size(width, px(BAR_HEIGHT)),
    );
    window.paint_quad(fill(bar, colors.surface_2).corner_radii(px(3.)));
    let font_size = theme.metrics.text_small;
    for segment in segments {
        let left = bar.left() + width * segment.start;
        let segment_width = (width * segment.width).max(px(1.));
        let rect = Bounds::new(point(left, bar.top()), size(segment_width, bar.size.height));
        let color = segment_color(segment.phase, theme);
        window.paint_quad(fill(rect, color));

        // The name and time if they fit, else the name, else nothing.
        let available = segment_width - px(2. * LABEL_PADDING);
        let label = [segment.label(), segment.name().to_string()]
            .into_iter()
            .map(|text| shape(text.into(), font_size, theme.phase_ink, window))
            .find(|line| line.width() <= available);
        if let Some(line) = label {
            let origin = point(left + px(LABEL_PADDING), bar.top());
            line.paint(origin, bar.size.height, TextAlign::Left, None, window, cx)
                .ok();
        }
    }
    let tick_x = (bar.left() + width * budget_at).min(bar.right() - px(1.5));
    window.paint_quad(fill(
        Bounds::new(
            point(tick_x, bounds.top()),
            size(px(1.5), bounds.size.height),
        ),
        colors.text,
    ));
    window.paint_quad(fill(
        Bounds::new(point(tick_x - px(2.), bounds.top()), size(px(5.5), px(1.5))),
        colors.text.opacity(0.9),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::ms;

    const BUDGET: Duration = Duration::from_micros(16_667);

    fn timings(render: f64, layout: f64, paint: f64, inspector: f64) -> PhaseTimings {
        PhaseTimings {
            input: ms(1.0),
            render: ms(render),
            layout: ms(layout),
            prepaint: Duration::ZERO,
            paint: ms(paint),
            inspector: ms(inspector),
            present: Some(ms(0.5)),
            total: ms(render + layout + paint + inspector + 0.4),
        }
    }

    fn names(bar: &PhaseBar) -> Vec<&'static str> {
        bar.segments.iter().map(PhaseSegment::name).collect()
    }

    #[test]
    fn segments_run_in_order_and_skip_empty_phases() {
        let bar = PhaseBar::new(&timings(20.0, 5.0, 3.0, 0.6), BUDGET);
        assert_eq!(
            names(&bar),
            [
                "input", "render", "layout", "paint", "other", "loupe", "present"
            ]
        );
        let other = &bar.segments[4];
        assert!((other.duration.as_secs_f64() * 1e3 - 0.4).abs() < 1e-6);
        // Contiguous, and the slow frame fills the bar.
        for pair in bar.segments.windows(2) {
            assert!((pair[0].start + pair[0].width - pair[1].start).abs() < 1e-5);
        }
        let last = bar.segments.last().unwrap();
        assert!((last.start + last.width - 1.0).abs() < 1e-5);
        assert_eq!(bar.segments[1].label(), "render 20.0");
    }

    #[test]
    fn the_budget_tick_sits_one_budget_after_the_input() {
        // A fast frame: the bar spans input + budget, the tick sits at its end.
        let fast = PhaseBar::new(&timings(2.0, 1.0, 1.0, 0.2), BUDGET);
        assert!((fast.budget_at - 1.0).abs() < 1e-6);
        let app_end = fast.segments[..5]
            .iter()
            .map(|segment| segment.start + segment.width)
            .fold(0.0, f32::max);
        assert!(app_end < fast.budget_at);

        // A slow frame: the app's phases cross the tick.
        let slow = PhaseBar::new(&timings(20.0, 5.0, 3.0, 0.6), BUDGET);
        let render = &slow.segments[1];
        assert!(render.start < slow.budget_at && slow.budget_at < render.start + render.width);
    }

    #[test]
    fn the_verdict_names_the_largest_app_phase() {
        let render_heavy = PhaseBar::new(&timings(20.0, 5.0, 3.0, 0.6), BUDGET);
        assert_eq!(render_heavy.dominant(), Some(Phase::Render));
        assert_eq!(
            render_heavy.verdict().as_deref(),
            Some("Render dominated with 70% of the app's time")
        );
        let balanced = PhaseBar::new(&timings(4.0, 3.0, 3.0, 0.2), BUDGET);
        assert_eq!(balanced.dominant(), None);
        assert_eq!(
            balanced.verdict().as_deref(),
            Some("Render led with 38% of the app's time")
        );
        assert_eq!(
            PhaseBar::new(&PhaseTimings::default(), BUDGET).verdict(),
            None
        );
    }

    #[test]
    fn an_empty_frame_is_all_budget() {
        let bar = PhaseBar::new(&PhaseTimings::default(), BUDGET);
        assert!(bar.segments.is_empty());
        assert_eq!(bar.budget_at, 1.0);
    }
}
