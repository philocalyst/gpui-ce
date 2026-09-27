//! The pulse strip: one bar per recorded frame, always visible.
//!
//! Bar height is `app_total` on a square-root scale capped at three budgets,
//! so a 2 ms frame is visible and a 60 ms spike does not flatten the rest.
//! Bars are graded ok / warn / crit; inspector-only frames are faint ticks;
//! frames that kept their element tree carry a dot. Everything is painted by
//! one canvas straight from the capture, and hit testing reuses the same
//! layout function.

use crate::{
    analysis::{
        format,
        stats::{Grade, pulse_height},
    },
    theme::Theme,
};
use gpui::{
    Bounds, ColorExt as _, Pixels, Point, Window, fill,
    inspector::{CauseKind, FrameRecord, InspectorCapture},
    outline, point, px, size,
};
use std::time::Duration;

/// Geometry of one frame in the strip.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PulseBar {
    /// Frame id.
    pub frame: u64,
    /// The full-height column the frame owns (hit target, selection outline).
    pub slot: Bounds<Pixels>,
    /// The painted bar.
    pub bar: Bounds<Pixels>,
    /// Budget grade of `app_total`.
    pub grade: Grade,
    /// Drawn only because of Loupe itself.
    pub inspector_only: bool,
    /// The frame's element tree was retained.
    pub has_tree: bool,
}

/// Vertical layout of the strip's plot area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PulseArea {
    /// Where bars and slots live.
    pub bounds: Bounds<Pixels>,
    /// Bars grow up from here.
    pub baseline: Pixels,
    /// Tallest possible bar (3× budget).
    pub max_bar: Pixels,
}

impl PulseArea {
    /// Plot area inside the strip's `bounds`: 5 px of headroom, and a 5 px
    /// row under the baseline for tree dots.
    pub fn new(bounds: Bounds<Pixels>) -> Self {
        let baseline = bounds.bottom() - px(7.);
        Self {
            bounds,
            baseline,
            max_bar: (baseline - bounds.top() - px(5.)).max(px(0.)),
        }
    }

    /// Where the dashed budget line sits.
    pub fn budget_y(&self) -> Pixels {
        let budget = Duration::from_secs(1);
        self.baseline - self.max_bar * pulse_height(budget, budget)
    }
}

/// Narrowest slot a frame gets; narrower strips show only the newest frames.
pub(crate) const MIN_SLOT: Pixels = px(2.);

/// Lays out `frames` (oldest first) right-aligned in `area`, one slot per
/// ring entry so bars never jitter as the ring fills. Slots are at least
/// [`MIN_SLOT`] wide; frames that do not fit on the left are skipped.
pub(crate) fn layout_bars<'a>(
    frames: impl ExactSizeIterator<Item = &'a FrameRecord>,
    capacity: usize,
    budget: Duration,
    area: &PulseArea,
) -> Vec<PulseBar> {
    let slots = capacity.max(frames.len()).max(1);
    let slot_width = (area.bounds.size.width / slots as f32).max(MIN_SLOT);
    let fitting = (area.bounds.size.width / slot_width).floor() as usize;
    let skipped = frames.len().saturating_sub(fitting);
    let count = frames.len() - skipped;
    let frames = frames.skip(skipped);
    let gap = if slot_width >= px(3.) {
        px(1.)
    } else if slot_width >= px(1.5) {
        px(0.5)
    } else {
        px(0.)
    };
    frames
        .enumerate()
        .map(|(ix, frame)| {
            let left = area.bounds.right() - slot_width * (count - ix) as f32;
            let app_total = frame.timings.app_total();
            let height = if frame.inspector_only {
                px(2.)
            } else {
                (area.max_bar * pulse_height(app_total, budget)).max(px(1.5))
            };
            PulseBar {
                frame: frame.id,
                slot: Bounds::new(
                    point(left, area.bounds.top()),
                    size(slot_width, area.bounds.size.height),
                ),
                bar: Bounds::new(
                    point(left, area.baseline - height),
                    size((slot_width - gap).max(px(0.5)), height),
                ),
                grade: Grade::of(app_total, budget),
                inspector_only: frame.inspector_only,
                has_tree: frame.tree.is_some(),
            }
        })
        .collect()
}

/// The frame whose slot contains `x`.
pub(crate) fn bar_at(bars: &[PulseBar], x: Pixels) -> Option<&PulseBar> {
    let ix = bars.partition_point(|bar| bar.slot.right() <= x);
    bars.get(ix).filter(|bar| bar.slot.left() <= x)
}

/// Bars for the capture's frames in the strip's `bounds`.
pub(crate) fn capture_bars(capture: &InspectorCapture, bounds: Bounds<Pixels>) -> Vec<PulseBar> {
    let config = capture.config();
    layout_bars(
        capture.frames().iter(),
        config.frame_capacity,
        config.budget,
        &PulseArea::new(bounds),
    )
}

/// One line about a frame: `#18372 · 23.4 ms · render 14.1 · IssueStore notified`.
pub(crate) fn frame_summary(frame: &FrameRecord) -> String {
    let timings = &frame.timings;
    let mut parts = vec![
        format!("#{}", frame.id),
        format!("{} ms", format::millis(timings.app_total())),
    ];
    let phases = [
        ("render", timings.render),
        ("layout", timings.layout),
        ("prepaint", timings.prepaint),
        ("paint", timings.paint),
    ];
    if let Some((name, duration)) = phases.iter().max_by_key(|(_, duration)| *duration)
        && !duration.is_zero()
    {
        parts.push(format!("{name} {}", format::millis(*duration)));
    }
    if frame.inspector_only {
        parts.push("Loupe only".into());
    } else if let Some(cause) = frame.causes.iter().find(|cause| !cause.from_inspector) {
        parts.push(cause_summary(&cause.kind));
    }
    parts.join(" · ")
}

/// A few words about why a frame was drawn: `IssueStore notified`, `MouseDown`.
pub(crate) fn cause_summary(kind: &CauseKind) -> String {
    match kind {
        CauseKind::Notify {
            type_name: Some(type_name),
            ..
        } => format!("{} notified", format::type_name(type_name)),
        CauseKind::Notify { entity, .. } => format!("entity {entity:?} notified"),
        CauseKind::Refresh => "window.refresh()".into(),
        CauseKind::Focus => "focus moved".into(),
        CauseKind::Resize => "resized".into(),
        CauseKind::WindowState => "window state".into(),
        CauseKind::Input { event } => (*event).into(),
        CauseKind::Animation => "animation".into(),
        CauseKind::Initial => "first frame".into(),
    }
}

/// Paints the strip's bars, budget line, tree dots and selection into `bounds`.
pub(crate) fn paint_pulse(
    bounds: Bounds<Pixels>,
    selected: Option<u64>,
    hovered: Option<u64>,
    theme: &Theme,
    window: &mut Window,
) {
    let Some(capture) = window.inspector_capture() else {
        return;
    };
    let area = PulseArea::new(bounds);
    let bars = capture_bars(capture, bounds);
    let colors = &theme.colors;

    // Dashed budget line behind the bars.
    let budget_y = area.budget_y().round();
    let mut x = bounds.left();
    let mut quads = Vec::new();
    while x < bounds.right() {
        let width = px(3.).min(bounds.right() - x);
        quads.push(fill(
            Bounds::new(point(x, budget_y), size(width, px(1.))),
            colors.line_strong,
        ));
        x += px(6.);
    }

    for bar in &bars {
        if Some(bar.frame) == hovered {
            quads.push(fill(bar.slot, colors.hover));
        }
        let color = match bar.grade {
            _ if bar.inspector_only => colors.text_faint.opacity(0.5),
            // Healthy frames stay calm so the spikes read first.
            Grade::Ok => colors.ok.opacity(0.75),
            grade => theme.grade_color(grade),
        };
        quads.push(fill(bar.bar, color));
        if bar.has_tree {
            let dot = px(2.);
            let center = bar.bar.center().x;
            quads.push(fill(
                Bounds::new(
                    point(center - dot / 2., area.baseline + px(2.5)),
                    size(dot, dot),
                ),
                colors.text_muted,
            ));
        }
    }
    for quad in quads {
        window.paint_quad(quad);
    }

    if let Some(bar) = selected.and_then(|frame| bars.iter().find(|bar| bar.frame == frame)) {
        let column = Bounds::new(
            point(bar.slot.left() - px(1.), bounds.top() + px(1.)),
            size(bar.slot.size.width + px(2.), bounds.size.height - px(2.)),
        );
        window.paint_quad(fill(column, colors.selected));
        window.paint_quad(outline(column, colors.accent, gpui::BorderStyle::Solid));
    }
}

/// The frame under `position` in a strip painted at `bounds`.
pub(crate) fn frame_at(
    capture: &InspectorCapture,
    bounds: Bounds<Pixels>,
    position: Point<Pixels>,
) -> Option<u64> {
    if !bounds.contains(&position) {
        return None;
    }
    let bars = capture_bars(capture, bounds);
    bar_at(&bars, position.x).map(|bar| bar.frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::inspector::{PhaseTimings, RenderCause, SceneStats};

    fn ms(value: f64) -> Duration {
        Duration::from_secs_f64(value / 1000.)
    }

    fn frame(id: u64, start_ms: f64, total_ms: f64, inspector_only: bool) -> FrameRecord {
        FrameRecord {
            id,
            start: ms(start_ms),
            viewport: size(px(800.), px(600.)),
            timings: PhaseTimings {
                render: ms(total_ms * 0.6),
                layout: ms(total_ms * 0.2),
                total: ms(total_ms),
                ..Default::default()
            },
            causes: Vec::new(),
            views: Vec::new(),
            spans: Vec::new(),
            scene: SceneStats::default(),
            element_count: 0,
            tree: None,
            input: 0..0,
            foreground: Vec::new(),
            inspector_only,
        }
    }

    fn area() -> PulseArea {
        PulseArea::new(Bounds::new(point(px(0.), px(0.)), size(px(240.), px(40.))))
    }

    #[test]
    fn bars_are_sqrt_scaled_up_from_the_baseline() {
        let frames = [frame(0, 0., 48., false), frame(1, 16., 12., false)];
        let area = area();
        let bars = layout_bars(frames.iter(), 2, ms(16.), &area);
        assert_eq!(
            bars[0].bar.size.height, area.max_bar,
            "three budgets fill the strip"
        );
        assert_eq!(
            bars[1].bar.size.height,
            area.max_bar * 0.5,
            "a quarter of that is half"
        );
        assert_eq!(
            area.budget_y(),
            area.baseline - area.max_bar * (1. / 3f32).sqrt()
        );
    }

    #[test]
    fn bars_are_right_aligned_one_slot_per_ring_entry() {
        let frames: Vec<_> = (0..3)
            .map(|id| frame(id, id as f64 * 16., 4., false))
            .collect();
        let bars = layout_bars(frames.iter(), 120, ms(16.), &area());
        assert_eq!(bars.len(), 3);
        assert_eq!(bars[2].slot.right(), px(240.));
        assert_eq!(bars[2].slot.size.width, px(2.));
        assert_eq!(bars[0].slot.left(), px(234.));
        assert!(bars.iter().all(|bar| bar.bar.bottom() == area().baseline));
    }

    #[test]
    fn narrow_strips_keep_a_minimum_slot_and_show_the_newest_frames() {
        let frames: Vec<_> = (0..240).map(|id| frame(id, 0., 4., false)).collect();
        let narrow = PulseArea::new(Bounds::new(point(px(0.), px(0.)), size(px(100.), px(40.))));
        let bars = layout_bars(frames.iter(), 240, ms(16.), &narrow);
        assert_eq!(bars.len(), 50);
        assert_eq!(bars[0].frame, 190);
        assert_eq!(bars[0].slot.left(), px(0.));
        assert_eq!(bars[49].slot.size.width, MIN_SLOT);
    }

    #[test]
    fn bars_are_graded_and_inspector_frames_are_ticks() {
        let frames = [
            frame(0, 0., 5., false),
            frame(1, 16., 20., false),
            frame(2, 32., 40., false),
            frame(3, 48., 30., true),
        ];
        let bars = layout_bars(frames.iter(), 4, ms(16.), &area());
        let grades: Vec<_> = bars.iter().map(|bar| bar.grade).collect();
        assert_eq!(grades[..3], [Grade::Ok, Grade::Warn, Grade::Crit]);
        assert_eq!(bars[3].bar.size.height, px(2.));
        assert!(bars[3].inspector_only);
    }

    #[test]
    fn hit_testing_finds_the_slot_under_the_pointer() {
        let frames: Vec<_> = (10..14).map(|id| frame(id, 0., 4., false)).collect();
        let bars = layout_bars(frames.iter(), 4, ms(16.), &area());
        assert_eq!(bar_at(&bars, px(0.)).map(|bar| bar.frame), Some(10));
        assert_eq!(bar_at(&bars, px(59.9)).map(|bar| bar.frame), Some(10));
        assert_eq!(bar_at(&bars, px(60.)).map(|bar| bar.frame), Some(11));
        assert_eq!(bar_at(&bars, px(239.)).map(|bar| bar.frame), Some(13));
        assert_eq!(bar_at(&bars, px(241.)), None);
    }

    #[test]
    fn summaries_name_the_heaviest_phase_and_the_first_app_cause() {
        let mut slow = frame(18372, 0., 23.4, false);
        slow.causes = vec![
            RenderCause {
                kind: CauseKind::Input { event: "MouseMove" },
                site: None,
                before_frame: Duration::ZERO,
                from_inspector: true,
            },
            RenderCause {
                kind: CauseKind::Notify {
                    entity: gpui::EntityId::from(7u64),
                    type_name: Some("inbox::IssueStore"),
                },
                site: None,
                before_frame: Duration::ZERO,
                from_inspector: false,
            },
        ];
        assert_eq!(
            frame_summary(&slow),
            "#18372 · 23.4 ms · render 14.0 · IssueStore notified"
        );
        assert_eq!(
            frame_summary(&frame(3, 0., 1., true)),
            "#3 · 1.0 ms · render 0.6 · Loupe only"
        );
    }
}
