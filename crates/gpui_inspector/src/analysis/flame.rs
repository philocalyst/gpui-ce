//! Flame-chart layout for one frame, and the zoomable time range it is drawn in.
//!
//! [`flame_layout`] turns a [`FrameRecord`] into rows of bars on one time axis
//! (offsets from the capture epoch): the frame itself, its views by nesting
//! depth, user spans by depth, and the main-thread work that ran before it.
//! [`visible_bars`] maps those bars to pixels for the current [`RangeZoom`],
//! merging runs of bars too narrow to read into a single "n items" bar.

use super::format;
use gpui::{
    EntityId, SharedString,
    inspector::{ElementIndex, ForegroundKind, FrameRecord, ViewOutcome},
};
use std::{ops::Range, panic::Location, time::Duration};

/// The narrowest visible range [`RangeZoom`] allows by default.
pub const DEFAULT_MIN_SPAN: Duration = Duration::from_micros(10);

/// Which kind of work a row of the flame chart shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lane {
    /// The frame as a whole.
    Frame,
    /// Views rendered or reused, one row per nesting depth.
    Views,
    /// `inspector_span!` spans, one row per nesting depth.
    UserSpans,
    /// Main-thread work before the frame, from the profiler journal.
    MainThread,
}

/// One row of the flame chart, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlameRow {
    /// What the row shows.
    pub lane: Lane,
    /// Nesting depth within the lane.
    pub depth: u16,
}

/// What a bar stands for; drives its color and its click action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarKind {
    /// The whole frame.
    Frame,
    /// A view that rendered or was served from the view cache.
    View(ViewOutcome),
    /// A user span.
    UserSpan,
    /// A foreground task poll.
    Task,
    /// An action handler.
    Action,
    /// Platform input dispatch.
    Input,
    /// Frame submission.
    Present,
    /// Short task polls folded together.
    SmallPolls {
        /// How many polls were folded.
        count: u32,
    },
}

/// One bar of the flame chart.
#[derive(Clone, Debug, PartialEq)]
pub struct FlameBar {
    /// Index into [`FlameLayout::rows`].
    pub row: usize,
    /// Start, as an offset from the capture epoch.
    pub start: Duration,
    /// Length.
    pub duration: Duration,
    /// What to print in the bar.
    pub label: SharedString,
    /// What it stands for.
    pub kind: BarKind,
    /// The element to reveal when clicked (views), in the frame's tree.
    pub element: Option<ElementIndex>,
    /// The entity behind it (views).
    pub entity: Option<EntityId>,
    /// Where it came from in source (user spans, tasks).
    pub site: Option<&'static Location<'static>>,
}

impl FlameBar {
    /// When the bar ends, as an offset from the capture epoch.
    pub fn end(&self) -> Duration {
        self.start + self.duration
    }
}

/// A frame laid out as rows of bars on one time axis.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlameLayout {
    /// Everything the chart covers: the frame and the work before it.
    pub range: Range<Duration>,
    /// Rows, top to bottom.
    pub rows: Vec<FlameRow>,
    /// Bars sorted by row, then start.
    pub bars: Vec<FlameBar>,
}

/// Lays out `frame` as a flame chart. View and user-span offsets (relative to
/// the frame) are moved onto the capture-epoch axis the main-thread slices
/// already use, so everything lines up.
pub fn flame_layout(frame: &FrameRecord) -> FlameLayout {
    let mut layout = FlameLayout::default();
    let frame_end = frame.start + frame.timings.total;

    let frame_row = layout.push_row(Lane::Frame, 0);
    layout.bars.push(FlameBar {
        row: frame_row,
        start: frame.start,
        duration: frame.timings.total,
        label: format!(
            "Frame #{} · {}",
            frame.id,
            format::duration(frame.timings.total)
        )
        .into(),
        kind: BarKind::Frame,
        element: None,
        entity: None,
        site: None,
    });

    let view_rows = layout.push_rows(Lane::Views, frame.views.iter().map(|view| view.depth));
    for view in &frame.views {
        layout.bars.push(FlameBar {
            row: view_rows.row(view.depth),
            start: frame.start + view.start,
            duration: view.duration,
            label: shared(format::type_name(view.type_name)),
            kind: BarKind::View(view.outcome),
            element: view.element,
            entity: Some(view.entity),
            site: None,
        });
    }

    let span_rows = layout.push_rows(Lane::UserSpans, frame.spans.iter().map(|span| span.depth));
    for span in &frame.spans {
        layout.bars.push(FlameBar {
            row: span_rows.row(span.depth),
            start: frame.start + span.start,
            duration: span.duration,
            label: span.name.clone(),
            kind: BarKind::UserSpan,
            element: None,
            entity: None,
            site: Some(span.site),
        });
    }

    if !frame.foreground.is_empty() {
        let main_row = layout.push_row(Lane::MainThread, 0);
        for slice in &frame.foreground {
            let (label, kind, site) = foreground_bar(&slice.kind);
            layout.bars.push(FlameBar {
                row: main_row,
                start: slice.start,
                duration: slice.duration,
                label,
                kind,
                element: None,
                entity: None,
                site,
            });
        }
    }

    layout.range = layout
        .bars
        .iter()
        .fold(frame.start..frame_end, |range, bar| {
            range.start.min(bar.start)..range.end.max(bar.end())
        });
    layout
        .bars
        .sort_by(|a, b| a.row.cmp(&b.row).then(a.start.cmp(&b.start)));
    layout
}

fn shared(text: std::borrow::Cow<'static, str>) -> SharedString {
    match text {
        std::borrow::Cow::Borrowed(text) => SharedString::new_static(text),
        std::borrow::Cow::Owned(text) => SharedString::from(text),
    }
}

fn foreground_bar(
    kind: &ForegroundKind,
) -> (SharedString, BarKind, Option<&'static Location<'static>>) {
    match kind {
        ForegroundKind::Task { site } => (format::location(site).into(), BarKind::Task, Some(site)),
        ForegroundKind::Action { name } => (SharedString::new_static(name), BarKind::Action, None),
        ForegroundKind::Input { kind } => (SharedString::new_static(kind), BarKind::Input, None),
        ForegroundKind::Present => (SharedString::new_static("Present"), BarKind::Present, None),
        ForegroundKind::SmallPolls { count } => (
            format!("{} small polls", format::count(u64::from(*count))).into(),
            BarKind::SmallPolls { count: *count },
            None,
        ),
    }
}

/// The rows of one lane: depths `min..=max`, contiguous.
struct LaneRows {
    first_row: usize,
    min_depth: u16,
}

impl LaneRows {
    fn row(&self, depth: u16) -> usize {
        self.first_row + usize::from(depth - self.min_depth)
    }
}

impl FlameLayout {
    fn push_row(&mut self, lane: Lane, depth: u16) -> usize {
        self.rows.push(FlameRow { lane, depth });
        self.rows.len() - 1
    }

    /// Adds one row per depth between the smallest and largest of `depths`.
    fn push_rows(&mut self, lane: Lane, depths: impl Iterator<Item = u16>) -> LaneRows {
        let first_row = self.rows.len();
        let (min_depth, max_depth) = depths.fold((u16::MAX, 0), |(min, max), depth| {
            (min.min(depth), max.max(depth))
        });
        if min_depth <= max_depth {
            for depth in min_depth..=max_depth {
                self.push_row(lane, depth);
            }
        }
        LaneRows {
            first_row,
            min_depth,
        }
    }
}

/// A bar (or a run of merged bars) placed in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibleBar {
    /// Index into [`FlameLayout::rows`].
    pub row: usize,
    /// Left edge in pixels, clipped to the chart.
    pub x: f32,
    /// Width in pixels, clipped to the chart.
    pub width: f32,
    /// The bars drawn here: indices into [`FlameLayout::bars`], one for a
    /// plain bar, more for a merged run.
    pub bars: Range<usize>,
}

impl VisibleBar {
    /// Whether this stands for several bars too narrow to draw one by one.
    pub fn is_merged(&self) -> bool {
        self.bars.len() > 1
    }

    /// The bar's own label, or `n items` for a merged run.
    pub fn label(&self, layout: &FlameLayout) -> SharedString {
        if self.is_merged() {
            format!("{} items", format::count(self.bars.len() as u64)).into()
        } else {
            layout
                .bars
                .get(self.bars.start)
                .map(|bar| bar.label.clone())
                .unwrap_or_default()
        }
    }
}

/// Places the bars of `layout` that intersect the zoom's visible range on a
/// chart `width` pixels wide. Consecutive bars in a row that are each
/// narrower than `min_width` pixels, and no further apart than that, are
/// merged into one [`VisibleBar`].
pub fn visible_bars(
    layout: &FlameLayout,
    zoom: &RangeZoom,
    width: f32,
    min_width: f32,
) -> Vec<VisibleBar> {
    let mut placed = Vec::new();
    let mut run: Option<VisibleBar> = None;
    let visible = zoom.visible();

    for (ix, bar) in layout.bars.iter().enumerate() {
        if bar.end() < visible.start || bar.start > visible.end {
            continue;
        }
        let x = zoom.time_to_x(bar.start, width).max(0.0);
        let right = zoom.time_to_x(bar.end(), width).min(width);
        let current = VisibleBar {
            row: bar.row,
            x,
            width: (right - x).max(0.0),
            bars: ix..ix + 1,
        };
        let narrow = current.width < min_width;
        match &mut run {
            Some(open)
                if narrow
                    && open.row == current.row
                    && open.bars.end == ix
                    && current.x - (open.x + open.width) < min_width =>
            {
                open.width = (right - open.x).max(open.width);
                open.bars.end = ix + 1;
            }
            _ => {
                placed.extend(run.take());
                if narrow {
                    run = Some(current);
                } else {
                    placed.push(current);
                }
            }
        }
    }
    placed.extend(run);
    placed
}

/// The visible part of a time range, zoomed and panned by the user.
///
/// Keeps the visible range inside the total range, never narrower than a
/// minimum span, and zooms around an anchor so the point under the cursor
/// stays put. Times are held as `f64` nanoseconds, exact for captures shorter
/// than 104 days.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeZoom {
    total: Range<f64>,
    visible: Range<f64>,
    min_span: f64,
}

impl RangeZoom {
    /// Shows all of `total`. A reversed range is treated as empty at its start.
    pub fn new(total: Range<Duration>) -> Self {
        let start = nanos(total.start);
        let end = nanos(total.end).max(start);
        Self {
            total: start..end,
            visible: start..end,
            min_span: nanos(DEFAULT_MIN_SPAN),
        }
    }

    /// Sets the narrowest span zooming in may reach.
    pub fn with_min_span(mut self, min_span: Duration) -> Self {
        self.min_span = nanos(min_span);
        self.clamp();
        self
    }

    /// The whole range.
    pub fn total(&self) -> Range<Duration> {
        duration(self.total.start)..duration(self.total.end)
    }

    /// The part currently shown.
    pub fn visible(&self) -> Range<Duration> {
        duration(self.visible.start)..duration(self.visible.end)
    }

    /// Whether something is hidden (zoomed in).
    pub fn is_zoomed(&self) -> bool {
        self.visible != self.total
    }

    /// Zooms by `factor` (above 1 zooms in, below 1 zooms out) around
    /// `anchor`, a fraction of the visible range (0.0 = left edge, 1.0 =
    /// right edge) that stays at the same place on screen. Invalid factors
    /// (zero, negative, NaN) are ignored.
    pub fn zoom(&mut self, factor: f64, anchor: f64) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let anchor = if anchor.is_finite() {
            anchor.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let span = self.span();
        let pivot = self.visible.start + anchor * span;
        let new_span = self.clamp_span(span / factor);
        let start = pivot - anchor * new_span;
        self.visible = start..start + new_span;
        self.clamp();
    }

    /// Pans by `delta` pixels on a chart `width` pixels wide. Positive deltas
    /// move the view later in time (as scrolling right does); a drag that
    /// moves the content right should pass the negated drag distance.
    pub fn pan(&mut self, delta: f32, width: f32) {
        if !(width > 0.0 && delta.is_finite()) {
            return;
        }
        let shift = f64::from(delta) / f64::from(width) * self.span();
        self.visible = self.visible.start + shift..self.visible.end + shift;
        self.clamp();
    }

    /// Shows everything.
    pub fn fit_all(&mut self) {
        self.visible = self.total.clone();
    }

    /// Shows `range` (e.g. a clicked bar), widened to the minimum span and
    /// kept inside the total range.
    pub fn show(&mut self, range: Range<Duration>) {
        let start = nanos(range.start);
        let end = nanos(range.end).max(start);
        let span = self.clamp_span(end - start);
        let center = (start + end) / 2.0;
        self.visible = center - span / 2.0..center + span / 2.0;
        self.clamp();
    }

    /// The x position of `time` on a chart `width` pixels wide. May fall
    /// outside `0..=width` for times outside the visible range.
    pub fn time_to_x(&self, time: Duration, width: f32) -> f32 {
        let span = self.span();
        if span == 0.0 {
            return 0.0;
        }
        ((nanos(time) - self.visible.start) / span * f64::from(width)) as f32
    }

    /// The time at `x` on a chart `width` pixels wide, clamped to the total range.
    pub fn x_to_time(&self, x: f32, width: f32) -> Duration {
        if !(width > 0.0 && x.is_finite()) {
            return duration(self.visible.start);
        }
        let time = self.visible.start + f64::from(x) / f64::from(width) * self.span();
        duration(time.clamp(self.total.start, self.total.end))
    }

    fn span(&self) -> f64 {
        self.visible.end - self.visible.start
    }

    fn clamp_span(&self, span: f64) -> f64 {
        let total = self.total.end - self.total.start;
        span.clamp(self.min_span.min(total), total)
    }

    /// Restores the invariants: span within `min_span..=total`, visible
    /// range inside the total range.
    fn clamp(&mut self) {
        let span = self.clamp_span(self.span());
        let start = self
            .visible
            .start
            .clamp(self.total.start, self.total.end - span);
        self.visible = start..start + span;
    }
}

fn nanos(duration: Duration) -> f64 {
    duration.as_nanos() as f64
}

fn duration(nanos: f64) -> Duration {
    Duration::from_nanos(nanos.round().max(0.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{cached_view, entity, frame, ms, us, view};
    use gpui::inspector::{ForegroundSlice, UserSpan};

    const WIDTH: f32 = 1_000.0;

    fn zoom_over(millis: f64) -> RangeZoom {
        RangeZoom::new(Duration::ZERO..ms(millis)).with_min_span(us(1))
    }

    fn assert_visible(zoom: &RangeZoom, start: f64, end: f64) {
        let visible = zoom.visible();
        let (actual_start, actual_end) = (
            visible.start.as_secs_f64() * 1e3,
            visible.end.as_secs_f64() * 1e3,
        );
        assert!(
            (actual_start - start).abs() < 1e-5 && (actual_end - end).abs() < 1e-5,
            "visible {actual_start}..{actual_end}, expected {start}..{end}"
        );
    }

    #[test]
    fn layout_of_a_bare_frame_is_one_bar() {
        let layout = flame_layout(&frame(3, ms(100.0), ms(8.0)));
        assert_eq!(layout.range, ms(100.0)..ms(108.0));
        assert_eq!(
            layout.rows,
            vec![FlameRow {
                lane: Lane::Frame,
                depth: 0
            }]
        );
        assert_eq!(layout.bars.len(), 1);
        assert_eq!(layout.bars[0].label, "Frame #3 · 8.0 ms");
        assert_eq!(layout.bars[0].kind, BarKind::Frame);
    }

    #[test]
    fn layout_puts_views_spans_and_main_thread_work_on_one_axis() {
        let mut record = frame(1, ms(100.0), ms(10.0));
        record.views = vec![
            view(1, "app::Workspace", 1, ms(0.0), ms(9.0)),
            view(2, "app::Sidebar", 2, ms(1.0), ms(2.0)),
            cached_view(3, "app::IssueList<app::Row>", 2, ms(3.0), us(20)),
        ];
        record.views[0].element = Some(4);
        record.spans = vec![UserSpan {
            name: "load rows".into(),
            site: Location::caller(),
            depth: 0,
            start: ms(3.5),
            duration: ms(1.0),
        }];
        record.foreground = vec![
            ForegroundSlice {
                kind: ForegroundKind::Action { name: "app::Open" },
                start: ms(80.0),
                duration: ms(6.0),
            },
            ForegroundSlice {
                kind: ForegroundKind::SmallPolls { count: 1_200 },
                start: ms(90.0),
                duration: ms(1.0),
            },
        ];

        let layout = flame_layout(&record);
        assert_eq!(layout.range, ms(80.0)..ms(110.0));
        assert_eq!(
            layout.rows,
            vec![
                FlameRow {
                    lane: Lane::Frame,
                    depth: 0
                },
                FlameRow {
                    lane: Lane::Views,
                    depth: 1
                },
                FlameRow {
                    lane: Lane::Views,
                    depth: 2
                },
                FlameRow {
                    lane: Lane::UserSpans,
                    depth: 0
                },
                FlameRow {
                    lane: Lane::MainThread,
                    depth: 0
                },
            ]
        );
        let summary: Vec<(usize, Duration, &str)> = layout
            .bars
            .iter()
            .map(|bar| (bar.row, bar.start, bar.label.as_ref()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (0, ms(100.0), "Frame #1 · 10.0 ms"),
                (1, ms(100.0), "Workspace"),
                (2, ms(101.0), "Sidebar"),
                (2, ms(103.0), "IssueList<Row>"),
                (3, ms(103.5), "load rows"),
                (4, ms(80.0), "app::Open"),
                (4, ms(90.0), "1,200 small polls"),
            ]
        );
        assert_eq!(layout.bars[1].element, Some(4));
        assert_eq!(layout.bars[1].entity, Some(entity(1)));
        assert_eq!(layout.bars[3].kind, BarKind::View(ViewOutcome::Cached));
        assert!(layout.bars[4].site.is_some());
        assert_eq!(layout.bars[6].kind, BarKind::SmallPolls { count: 1_200 });
    }

    #[test]
    fn narrow_neighbours_merge_into_one_bar() {
        let mut record = frame(0, Duration::ZERO, ms(100.0));
        // Ten 10 µs views back to back, then one wide view.
        record.views = (0..10)
            .map(|ix| view(ix, "app::Row", 0, us(10 * ix), us(10)))
            .chain([view(99, "app::Detail", 0, ms(50.0), ms(40.0))])
            .collect();
        let layout = flame_layout(&record);
        let zoom = RangeZoom::new(layout.range.clone());

        let placed = visible_bars(&layout, &zoom, WIDTH, 2.0);
        let rows: Vec<(usize, bool, String)> = placed
            .iter()
            .map(|bar| (bar.row, bar.is_merged(), bar.label(&layout).to_string()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (0, false, "Frame #0 · 100.0 ms".to_string()),
                (1, true, "10 items".to_string()),
                (1, false, "Detail".to_string()),
            ]
        );
        assert!((placed[1].width - 1.0).abs() < 1e-3);
        assert!((placed[2].x - 500.0).abs() < 1e-3 && (placed[2].width - 400.0).abs() < 1e-3);

        // Zoomed in on the rows, each stands alone and `Detail` is culled.
        let mut zoom = zoom;
        zoom.show(Duration::ZERO..us(100));
        let placed = visible_bars(&layout, &zoom, WIDTH, 2.0);
        assert_eq!(placed.len(), 11);
        assert!(placed.iter().all(|bar| !bar.is_merged()));
    }

    #[test]
    fn narrow_bars_far_apart_stay_separate() {
        let mut record = frame(0, Duration::ZERO, ms(100.0));
        record.views = vec![
            view(1, "app::A", 0, ms(10.0), us(5)),
            view(2, "app::B", 0, ms(60.0), us(5)),
        ];
        let layout = flame_layout(&record);
        let zoom = RangeZoom::new(layout.range.clone());
        let placed = visible_bars(&layout, &zoom, WIDTH, 2.0);
        assert_eq!(placed.len(), 3);
        assert!(placed.iter().all(|bar| !bar.is_merged()));
    }

    #[test]
    fn bars_outside_the_visible_range_are_culled_and_clipped() {
        let mut record = frame(0, Duration::ZERO, ms(100.0));
        record.views = vec![
            view(1, "app::Early", 0, ms(0.0), ms(10.0)),
            view(2, "app::Late", 0, ms(90.0), ms(10.0)),
        ];
        let layout = flame_layout(&record);
        let mut zoom = RangeZoom::new(layout.range.clone());
        zoom.show(ms(40.0)..ms(60.0));

        let placed = visible_bars(&layout, &zoom, WIDTH, 2.0);
        assert_eq!(placed.len(), 1, "only the frame bar spans 40..60 ms");
        assert_eq!((placed[0].x, placed[0].width), (0.0, WIDTH));
    }

    #[test]
    fn empty_layout_places_nothing() {
        let layout = FlameLayout::default();
        let zoom = RangeZoom::new(layout.range.clone());
        assert!(visible_bars(&layout, &zoom, WIDTH, 2.0).is_empty());
    }

    #[test]
    fn zoom_keeps_the_anchor_fixed() {
        let mut zoom = zoom_over(100.0);
        let anchor_time = zoom.x_to_time(300.0, WIDTH);
        zoom.zoom(4.0, 0.3);
        assert_visible(&zoom, 22.5, 47.5);
        assert!((zoom.time_to_x(anchor_time, WIDTH) - 300.0).abs() < 1e-3);
        assert!(zoom.is_zoomed());
    }

    #[test]
    fn zoom_in_then_out_returns_to_the_start() {
        let mut zoom = zoom_over(100.0);
        zoom.show(ms(20.0)..ms(60.0));
        let before = zoom.clone();
        zoom.zoom(1.25, 0.37);
        zoom.zoom(1.0 / 1.25, 0.37);
        assert_visible(&zoom, 20.0, 60.0);
        zoom.zoom(2.0, 0.5);
        zoom.zoom(0.5, 0.5);
        assert_eq!(zoom, before);
    }

    #[test]
    fn zoom_is_clamped_to_min_span_and_total() {
        let mut zoom = RangeZoom::new(Duration::ZERO..ms(100.0)).with_min_span(ms(1.0));
        zoom.zoom(1e9, 0.5);
        assert_visible(&zoom, 49.5, 50.5);
        zoom.zoom(1e-9, 0.5);
        assert_visible(&zoom, 0.0, 100.0);
        assert!(!zoom.is_zoomed());
    }

    #[test]
    fn zoom_near_an_edge_stays_inside() {
        let mut zoom = zoom_over(100.0);
        zoom.zoom(2.0, 0.0);
        assert_visible(&zoom, 0.0, 50.0);
        zoom.zoom(0.5, 1.0);
        assert_visible(&zoom, 0.0, 100.0);
    }

    #[test]
    fn invalid_zoom_input_is_ignored() {
        let mut zoom = zoom_over(100.0);
        for factor in [0.0, -2.0, f64::NAN, f64::INFINITY] {
            zoom.zoom(factor, 0.5);
        }
        assert_visible(&zoom, 0.0, 100.0);
        zoom.zoom(2.0, f64::NAN);
        assert_visible(&zoom, 25.0, 75.0);
    }

    #[test]
    fn pan_moves_by_pixels_and_clamps() {
        let mut zoom = zoom_over(100.0);
        zoom.zoom(4.0, 0.5);
        assert_visible(&zoom, 37.5, 62.5);
        zoom.pan(100.0, WIDTH);
        assert_visible(&zoom, 40.0, 65.0);
        zoom.pan(-1e6, WIDTH);
        assert_visible(&zoom, 0.0, 25.0);
        zoom.pan(1e6, WIDTH);
        assert_visible(&zoom, 75.0, 100.0);
        zoom.pan(10.0, 0.0);
        zoom.pan(f32::NAN, WIDTH);
        assert_visible(&zoom, 75.0, 100.0);
    }

    #[test]
    fn fit_all_and_show() {
        let mut zoom = zoom_over(100.0);
        zoom.show(ms(10.0)..ms(20.0));
        assert_visible(&zoom, 10.0, 20.0);
        zoom.fit_all();
        assert_visible(&zoom, 0.0, 100.0);
        // Too narrow: widened around its center to the minimum span.
        let mut zoom = RangeZoom::new(Duration::ZERO..ms(100.0)).with_min_span(ms(2.0));
        zoom.show(ms(99.9)..ms(100.0));
        assert_visible(&zoom, 98.0, 100.0);
    }

    #[test]
    fn pixel_time_mapping_round_trips() {
        let mut zoom = RangeZoom::new(ms(1_000.0)..ms(1_100.0));
        zoom.show(ms(1_020.0)..ms(1_040.0));
        for x in [0.0, 1.0, 333.0, 999.0, WIDTH] {
            let time = zoom.x_to_time(x, WIDTH);
            assert!((zoom.time_to_x(time, WIDTH) - x).abs() < 1e-2, "{x}");
        }
        assert_eq!(zoom.x_to_time(-50.0, WIDTH), ms(1_019.0));
        assert_eq!(zoom.x_to_time(1e9, WIDTH), ms(1_100.0));
        assert_eq!(zoom.x_to_time(10.0, 0.0), ms(1_020.0));
    }

    #[test]
    fn empty_and_reversed_ranges_are_safe() {
        let mut zoom = RangeZoom::new(ms(5.0)..ms(5.0));
        zoom.zoom(2.0, 0.5);
        zoom.pan(10.0, WIDTH);
        assert_eq!(zoom.visible(), ms(5.0)..ms(5.0));
        assert_eq!(zoom.time_to_x(ms(5.0), WIDTH), 0.0);
        assert_eq!(zoom.x_to_time(500.0, WIDTH), ms(5.0));

        let reversed = RangeZoom::new(ms(9.0)..ms(3.0));
        assert_eq!(reversed.total(), ms(9.0)..ms(9.0));
    }
}
