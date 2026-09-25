//! The flame chart: one frame's views (by nesting depth), user spans and the
//! main-thread work before it, on one time axis, painted by a single canvas.
//!
//! [`FlameData`] is computed once per frame (layout, self times, lanes).
//! [`FlameState`] holds what the user did to it (zoom, hover, press,
//! selection) and turns pointer input into zooms, pans and hits; painting
//! places only the bars in the visible range, merging runs too narrow to
//! read, so zooming redraws in time proportional to what is on screen.

use super::text::{fit, shape};
use crate::{
    analysis::{
        bottom_up, contrast,
        flame::{BarKind, FlameLayout, Lane, RangeZoom, VisibleBar, flame_layout, visible_bars},
        format,
    },
    theme::{Phase, Theme},
};
use gpui::{
    App, Bounds, ColorExt as _, EntityId, Hsla, Pixels, Point, SharedString, TextAlign, Window,
    fill,
    inspector::{ElementIndex, FrameRecord, ViewOutcome},
    outline, point, px, size, white,
};
use std::{cell::Cell, collections::HashMap, ops::Range, panic::Location, rc::Rc, time::Duration};

/// Height of one row of bars.
pub(crate) const ROW_HEIGHT: Pixels = px(18.);
/// Space between rows.
const ROW_GAP: Pixels = px(2.);
/// The time axis above the rows.
const AXIS_HEIGHT: Pixels = px(18.);
/// Space under the last row.
const BOTTOM_PADDING: Pixels = px(4.);
/// Lane names on the left.
pub(crate) const GUTTER: Pixels = px(52.);
/// Bars narrower than this (in pixels) merge with narrow neighbours.
const MIN_BAR_WIDTH: f32 = 3.;
/// Bars narrower than this get no label.
const MIN_LABEL_WIDTH: Pixels = px(18.);
/// Space between a bar's edge and its label.
const LABEL_PADDING: Pixels = px(4.);
/// Axis ticks are at least this many pixels apart.
const TICK_SPACING: f32 = 80.;
/// Wheel zoom: the zoom factor is `exp(pixels scrolled × this)`.
pub(crate) const WHEEL_ZOOM_PER_PIXEL: f64 = 0.004;
/// One keyboard zoom step.
pub(crate) const KEY_ZOOM: f64 = 1.5;
/// A press that moves less than this is a click, not a drag.
const DRAG_THRESHOLD: Pixels = px(3.);
/// Wheel deltas in lines scroll this many pixels per line.
pub(crate) const WHEEL_LINE: Pixels = px(20.);

/// The view a bar stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ViewInfo {
    /// `type_name::<V>()`.
    pub type_name: &'static str,
    /// Its render without the views nested in it.
    pub self_time: Duration,
    /// Its element in the frame's tree, if the frame kept one.
    pub element: Option<ElementIndex>,
    /// The view entity.
    pub entity: EntityId,
}

/// Everything the chart shows for one frame.
#[derive(Debug)]
pub(crate) struct FlameData {
    /// The frame's id.
    pub frame: u64,
    /// When the frame started.
    pub frame_start: Duration,
    /// When the frame should have ended: its start plus the budget.
    pub budget_end: Duration,
    /// The app's share of the frame.
    pub app_total: Duration,
    /// Loupe's share of the frame.
    pub loupe: Duration,
    /// Drawn only for Loupe: the app was replayed, not rendered.
    pub replayed: bool,
    /// Rows and bars.
    pub layout: FlameLayout,
    /// For each bar of `layout.bars`, the view it stands for.
    pub views: Vec<Option<ViewInfo>>,
    /// Each lane with its first row and its number of rows, top to bottom.
    pub lanes: Vec<(Lane, usize, usize)>,
}

impl FlameData {
    /// Lays out `frame` against `budget`.
    pub fn new(frame: &FrameRecord, budget: Duration) -> Self {
        let layout = flame_layout(frame);
        let self_times = bottom_up::self_times(&frame.views);
        let by_start: HashMap<(EntityId, Duration), usize> = frame
            .views
            .iter()
            .enumerate()
            .map(|(ix, view)| ((view.entity, frame.start + view.start), ix))
            .collect();
        let views = layout
            .bars
            .iter()
            .map(|bar| {
                if !matches!(bar.kind, BarKind::View(_)) {
                    return None;
                }
                let ix = *by_start.get(&(bar.entity?, bar.start))?;
                let view = &frame.views[ix];
                Some(ViewInfo {
                    type_name: view.type_name,
                    self_time: self_times[ix],
                    element: view.element,
                    entity: view.entity,
                })
            })
            .collect();
        let mut lanes: Vec<(Lane, usize, usize)> = Vec::new();
        for (row, flame_row) in layout.rows.iter().enumerate() {
            match lanes.last_mut() {
                Some((lane, _, rows)) if *lane == flame_row.lane => *rows += 1,
                _ => lanes.push((flame_row.lane, row, 1)),
            }
        }
        Self {
            frame: frame.id,
            frame_start: frame.start,
            budget_end: frame.start + budget,
            app_total: frame.timings.app_total(),
            loupe: frame.timings.inspector,
            replayed: frame.inspector_only,
            layout,
            views,
            lanes,
        }
    }

    /// The number of rows.
    pub fn rows(&self) -> usize {
        self.layout.rows.len()
    }
}

/// The name of a lane, as printed in the gutter.
pub(crate) fn lane_name(lane: Lane) -> &'static str {
    match lane {
        Lane::Frame => "Frame",
        Lane::Views => "Views",
        Lane::UserSpans => "Spans",
        Lane::MainThread => "Main",
    }
}

/// Where the chart's parts sit inside its bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geometry {
    /// The whole chart.
    pub bounds: Bounds<Pixels>,
    /// The bars' area: right of the gutter, below the axis.
    pub plot: Bounds<Pixels>,
}

impl Geometry {
    /// The geometry of a chart drawn in `bounds`.
    pub fn new(bounds: Bounds<Pixels>) -> Self {
        let plot = Bounds::new(
            point(bounds.left() + GUTTER, bounds.top() + AXIS_HEIGHT),
            size(
                (bounds.size.width - GUTTER).max(px(1.)),
                (bounds.size.height - AXIS_HEIGHT).max(Pixels::ZERO),
            ),
        );
        Self { bounds, plot }
    }

    /// The height a chart with `rows` rows needs.
    pub fn height(rows: usize) -> Pixels {
        AXIS_HEIGHT + (ROW_HEIGHT + ROW_GAP) * rows as f32 + BOTTOM_PADDING
    }

    /// The plot's width in pixels.
    pub fn width(&self) -> f32 {
        f32::from(self.plot.size.width)
    }

    /// The top of `row`.
    pub fn row_top(&self, row: usize) -> Pixels {
        self.plot.top() + (ROW_HEIGHT + ROW_GAP) * row as f32
    }

    /// The row whose bars cover `y` (not the gaps between rows).
    pub fn row_at(&self, y: Pixels, rows: usize) -> Option<usize> {
        if y < self.plot.top() {
            return None;
        }
        let pitch = ROW_HEIGHT + ROW_GAP;
        let row = ((y - self.plot.top()) / pitch).floor() as usize;
        let within = y - self.row_top(row);
        (row < rows && within < ROW_HEIGHT).then_some(row)
    }

    /// Where `x` falls along the plot, 0.0 (left edge) to 1.0 (right edge).
    pub fn anchor(&self, x: Pixels) -> f64 {
        f64::from((x - self.plot.left()) / self.plot.size.width).clamp(0.0, 1.0)
    }

    /// A visible bar's rectangle.
    pub fn bar_bounds(&self, bar: &VisibleBar) -> Bounds<Pixels> {
        Bounds::new(
            point(self.plot.left() + px(bar.x), self.row_top(bar.row)),
            size(px(bar.width.max(1.)), ROW_HEIGHT),
        )
    }
}

/// The bars of `layout` placed for `zoom` in `geometry`.
pub(crate) fn place(
    layout: &FlameLayout,
    zoom: &RangeZoom,
    geometry: &Geometry,
) -> Vec<VisibleBar> {
    visible_bars(layout, zoom, geometry.width(), MIN_BAR_WIDTH)
}

/// The placed bar under `position`.
pub(crate) fn bar_at<'a>(
    placed: &'a [VisibleBar],
    geometry: &Geometry,
    rows: usize,
    position: Point<Pixels>,
) -> Option<&'a VisibleBar> {
    if position.x < geometry.plot.left() || position.x > geometry.plot.right() {
        return None;
    }
    let row = geometry.row_at(position.y, rows)?;
    placed.iter().find(|bar| {
        let bounds = geometry.bar_bounds(bar);
        bar.row == row && bounds.left() <= position.x && position.x <= bounds.right()
    })
}

/// A tick on the time axis: its x in the plot and its label.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tick {
    /// Pixels from the plot's left edge.
    pub x: f32,
    /// Time from the frame's start: `0`, `5.0 ms`, `−2.0 ms`.
    pub label: String,
}

/// Axis ticks every 1, 2 or 5 × 10ⁿ ns, at least `spacing` pixels apart,
/// labelled relative to `origin` (the frame's start).
pub(crate) fn axis_ticks(
    zoom: &RangeZoom,
    origin: Duration,
    width: f32,
    spacing: f32,
) -> Vec<Tick> {
    let visible = zoom.visible();
    let span = (visible.end - visible.start).as_nanos() as f64;
    if span <= 0.0 || width <= 0.0 {
        return Vec::new();
    }
    let step = nice_step(span * f64::from(spacing) / f64::from(width));
    let origin = origin.as_nanos() as f64;
    let start = visible.start.as_nanos() as f64 - origin;
    let end = visible.end.as_nanos() as f64 - origin;
    let mut ticks = Vec::new();
    let mut offset = (start / step).ceil() * step;
    while offset <= end {
        let time = Duration::from_nanos((origin + offset).max(0.0).round() as u64);
        ticks.push(Tick {
            x: zoom.time_to_x(time, width),
            label: offset_label(offset),
        });
        offset += step;
    }
    ticks
}

/// The smallest 1, 2 or 5 × 10ⁿ at least `raw`.
fn nice_step(raw: f64) -> f64 {
    let magnitude = 10f64.powf(raw.max(1.0).log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|factor| factor * magnitude)
        .find(|step| *step >= raw)
        .unwrap_or(10.0 * magnitude)
}

/// `0`, `5.0 ms` or `−2.0 ms`: time from the frame's start, for the axis.
fn offset_label(nanos: f64) -> String {
    let magnitude = format::duration(Duration::from_nanos(nanos.abs().round() as u64));
    if nanos.abs() < 0.5 {
        "0".to_string()
    } else if nanos < 0.0 {
        format!("−{magnitude}")
    } else {
        magnitude
    }
}

/// `+5.0 ms`, `−2.0 ms` or `at the start`: when a bar starts, relative to
/// the frame's start.
fn start_label(nanos: f64) -> String {
    match offset_label(nanos) {
        zero if zero == "0" => "at the frame's start".to_string(),
        label if nanos > 0.0 => format!("+{label}"),
        label => label,
    }
}

/// The fill of a bar.
pub(crate) fn bar_color(kind: BarKind, theme: &Theme) -> Hsla {
    match kind {
        BarKind::Frame => theme.colors.line_strong,
        BarKind::View(ViewOutcome::Rendered) => theme.phase(Phase::Render),
        BarKind::View(ViewOutcome::Cached) => theme.phase(Phase::Render).opacity(0.35),
        BarKind::UserSpan => theme.colors.component,
        BarKind::Task => theme.phase(Phase::Paint),
        BarKind::Action => theme.phase(Phase::Prepaint),
        BarKind::Input => theme.phase(Phase::Input),
        BarKind::Present => theme.phase(Phase::Present),
        BarKind::SmallPolls { .. } => theme.colors.text_faint.opacity(0.35),
    }
}

/// What a bar's color means, for the legend.
pub(crate) fn kind_name(kind: BarKind) -> &'static str {
    match kind {
        BarKind::Frame => "frame",
        BarKind::View(ViewOutcome::Rendered) => "rendered view",
        BarKind::View(ViewOutcome::Cached) => "cached view",
        BarKind::UserSpan => "user span",
        BarKind::Task => "task",
        BarKind::Action => "action",
        BarKind::Input => "input",
        BarKind::Present => "present",
        BarKind::SmallPolls { .. } => "small polls",
    }
}

/// The kinds of bar in `layout` (except the frame itself), in lane order.
pub(crate) fn legend(layout: &FlameLayout) -> Vec<BarKind> {
    let mut kinds: Vec<BarKind> = Vec::new();
    for bar in &layout.bars {
        let kind = match bar.kind {
            BarKind::SmallPolls { .. } => BarKind::SmallPolls { count: 0 },
            kind => kind,
        };
        if kind != BarKind::Frame && !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

/// Text that reads on `fill`: dark ink on light fills, white on dark ones,
/// the theme's text on translucent ones.
fn ink(fill: Hsla, theme: &Theme) -> Hsla {
    if fill.alpha < 0.9 {
        theme.colors.text
    } else if contrast::relative_luminance(fill) > 0.3 {
        theme.phase_ink
    } else {
        white()
    }
}

/// A press on the chart: a click unless it moves, then a pan.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Press {
    origin: Point<Pixels>,
    last_x: Pixels,
    dragging: bool,
}

/// What the user has done to the chart of the shown frame.
pub(crate) struct FlameState {
    data: Option<Rc<FlameData>>,
    zoom: RangeZoom,
    /// The bars under the pointer, and where their tooltip goes.
    hovered: Option<(Range<usize>, Point<Pixels>)>,
    pointer: Option<Point<Pixels>>,
    selected: Option<usize>,
    press: Option<Press>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Default for FlameState {
    fn default() -> Self {
        Self {
            data: None,
            zoom: RangeZoom::new(Duration::ZERO..Duration::ZERO),
            hovered: None,
            pointer: None,
            selected: None,
            press: None,
            bounds: Rc::default(),
        }
    }
}

impl FlameState {
    /// Shows `data`. A different frame resets zoom, hover and selection; the
    /// same frame (recomputed, e.g. for a new budget) keeps them.
    pub fn show(&mut self, data: Rc<FlameData>) {
        let same_frame = self
            .data
            .as_ref()
            .is_some_and(|shown| shown.frame == data.frame);
        if !same_frame {
            self.zoom = RangeZoom::new(data.layout.range.clone());
            self.hovered = None;
            self.selected = None;
            self.press = None;
        }
        self.data = Some(data);
    }

    /// The shown frame's data.
    pub fn data(&self) -> Option<&Rc<FlameData>> {
        self.data.as_ref()
    }

    /// The current zoom.
    pub fn zoom(&self) -> &RangeZoom {
        &self.zoom
    }

    /// The hovered bars (one, or a merged run) and where their tooltip goes.
    pub fn hovered(&self) -> Option<&(Range<usize>, Point<Pixels>)> {
        self.hovered.as_ref()
    }

    /// The selected bar, an index into the layout's bars.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Selects a bar (or nothing).
    pub fn select(&mut self, bar: Option<usize>) {
        self.selected = bar;
    }

    /// Where the canvas stores its bounds when it lays out.
    pub fn bounds_cell(&self) -> Rc<Cell<Option<Bounds<Pixels>>>> {
        self.bounds.clone()
    }

    fn geometry(&self) -> Option<Geometry> {
        self.bounds.get().map(Geometry::new)
    }

    /// The bars under `position`: one bar, or a merged run.
    pub fn hit(&self, position: Point<Pixels>) -> Option<Range<usize>> {
        let data = self.data.as_ref()?;
        let geometry = self.geometry()?;
        let placed = place(&data.layout, &self.zoom, &geometry);
        bar_at(&placed, &geometry, data.rows(), position).map(|bar| bar.bars.clone())
    }

    /// Tracks the pointer. Returns whether the hovered bars changed (moving
    /// along one bar changes nothing: its tooltip stays where the pointer
    /// entered it, under the bar, so hovering does not re-render the lens).
    pub fn hover(&mut self, position: Option<Point<Pixels>>) -> bool {
        self.pointer = position;
        let bars = position.and_then(|position| self.hit(position));
        if bars.as_ref() == self.hovered.as_ref().map(|(bars, _)| bars) {
            return false;
        }
        self.hovered = bars.zip(position).map(|(bars, position)| {
            let bottom = self
                .data
                .as_ref()
                .and_then(|data| data.layout.bars.get(bars.start))
                .zip(self.geometry())
                .map_or(position.y, |(bar, geometry)| {
                    geometry.row_top(bar.row) + ROW_HEIGHT
                });
            (bars, point(position.x, bottom))
        });
        true
    }

    /// Zooms by a wheel `delta` (pixels; up zooms in) around `position`,
    /// or pans on horizontal scrolls. Returns whether the view changed.
    pub fn wheel(&mut self, position: Point<Pixels>, delta: Point<Pixels>) -> bool {
        let Some(geometry) = self.geometry() else {
            return false;
        };
        let before = self.zoom.clone();
        if delta.x.abs() > delta.y.abs() {
            self.zoom.pan(-f32::from(delta.x), geometry.width());
        } else {
            let factor = (f64::from(f32::from(delta.y)) * WHEEL_ZOOM_PER_PIXEL).exp();
            self.zoom.zoom(factor, geometry.anchor(position.x));
        }
        self.zoom != before
    }

    /// Zooms by `factor` around the hovered point, or the middle.
    pub fn zoom_by(&mut self, factor: f64) {
        let anchor = self
            .pointer
            .zip(self.geometry())
            .map_or(0.5, |(at, geometry)| geometry.anchor(at.x));
        self.zoom.zoom(factor, anchor);
    }

    /// Shows the whole frame.
    pub fn fit(&mut self) {
        self.zoom.fit_all();
    }

    /// Zooms to `range`, e.g. a merged run of bars.
    pub fn show_range(&mut self, range: Range<Duration>) {
        self.zoom.show(range);
    }

    /// Starts a press at `position`.
    pub fn press(&mut self, position: Point<Pixels>) {
        self.press = Some(Press {
            origin: position,
            last_x: position.x,
            dragging: false,
        });
    }

    /// Moves a press; once it has moved past a few pixels it pans the chart
    /// with the pointer. Returns whether the view changed.
    pub fn drag_to(&mut self, position: Point<Pixels>) -> bool {
        let (Some(press), Some(geometry)) =
            (self.press.as_mut(), self.bounds.get().map(Geometry::new))
        else {
            return false;
        };
        if !press.dragging {
            let distance = (position - press.origin).magnitude();
            if distance < f64::from(f32::from(DRAG_THRESHOLD)) {
                return false;
            }
            press.dragging = true;
            self.hovered = None;
        }
        let delta = position.x - press.last_x;
        press.last_x = position.x;
        self.zoom.pan(-f32::from(delta), geometry.width());
        true
    }

    /// Ends a press without a click (the button went up elsewhere).
    pub fn cancel_press(&mut self) {
        self.press = None;
    }

    /// Whether a press is panning the chart.
    pub fn is_dragging(&self) -> bool {
        self.press.is_some_and(|press| press.dragging)
    }

    /// Ends a press. A press that did not move is a click: returns the
    /// bars it landed on.
    pub fn release(&mut self, position: Point<Pixels>) -> Option<Range<usize>> {
        let press = self.press.take()?;
        if press.dragging {
            return None;
        }
        self.hit(position)
    }

    /// Everything the canvas needs to paint the current state.
    pub fn painter(&self, emphasized: Option<&'static str>) -> Option<FlamePainter> {
        Some(FlamePainter {
            data: self.data.clone()?,
            zoom: self.zoom.clone(),
            hovered: self.hovered.as_ref().map(|(bars, _)| bars.clone()),
            selected: self.selected,
            emphasized,
        })
    }
}

/// A snapshot of the chart to paint.
pub(crate) struct FlamePainter {
    data: Rc<FlameData>,
    zoom: RangeZoom,
    hovered: Option<Range<usize>>,
    selected: Option<usize>,
    emphasized: Option<&'static str>,
}

impl FlamePainter {
    /// Paints the chart into `bounds`.
    pub fn paint(&self, bounds: Bounds<Pixels>, theme: &Theme, window: &mut Window, cx: &mut App) {
        let colors = &theme.colors;
        let geometry = Geometry::new(bounds);
        let data = &self.data;
        let font_size = theme.metrics.text_small;
        window.paint_quad(fill(
            Bounds::new(bounds.origin, size(GUTTER, bounds.size.height)),
            colors.surface,
        ));

        // Axis ticks and grid lines, labelled from the frame's start.
        let ticks = axis_ticks(&self.zoom, data.frame_start, geometry.width(), TICK_SPACING);
        for tick in &ticks {
            let x = geometry.plot.left() + px(tick.x);
            window.paint_quad(fill(
                Bounds::new(
                    point(x, bounds.top() + px(4.)),
                    size(px(1.), bounds.size.height - px(4.)),
                ),
                colors.line,
            ));
            let label = shape(
                tick.label.clone().into(),
                font_size,
                colors.text_faint,
                window,
            );
            // Right of the line, unless that runs off the chart.
            let label_x = if x + px(3.) + label.width() > bounds.right() {
                x - px(3.) - label.width()
            } else {
                x + px(3.)
            };
            label
                .paint(
                    point(label_x, bounds.top()),
                    AXIS_HEIGHT,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
        }

        // Lanes: a rule above each but the first, and its name in the gutter.
        for (ix, (lane, first_row, _)) in data.lanes.iter().enumerate() {
            let top = geometry.row_top(*first_row);
            if ix > 0 {
                window.paint_quad(fill(
                    Bounds::new(
                        point(bounds.left(), top - ROW_GAP / 2.),
                        size(bounds.size.width, px(1.)),
                    ),
                    colors.line,
                ));
            }
            let name = shape(
                lane_name(*lane).into(),
                font_size,
                colors.text_muted,
                window,
            );
            name.paint(
                point(bounds.left() + px(8.), top),
                ROW_HEIGHT,
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .ok();
        }

        let placed = place(&data.layout, &self.zoom, &geometry);
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: geometry.plot,
            }),
            |window| {
                for bar in &placed {
                    self.paint_bar(bar, &geometry, theme, window, cx);
                }
                self.paint_budget(&geometry, theme, window);
            },
        );
    }

    fn paint_bar(
        &self,
        placed: &VisibleBar,
        geometry: &Geometry,
        theme: &Theme,
        window: &mut Window,
        cx: &mut App,
    ) {
        let colors = &theme.colors;
        let layout = &self.data.layout;
        let Some(first) = layout.bars.get(placed.bars.start) else {
            return;
        };
        let rect = geometry.bar_bounds(placed);
        let mut color = bar_color(first.kind, theme);
        if placed.is_merged() {
            color = color.opacity(0.55);
        }
        let view = self.data.views.get(placed.bars.start).copied().flatten();
        let emphasized = match self.emphasized {
            Some(type_name) => view.is_some_and(|view| view.type_name == type_name),
            None => false,
        };
        if self.emphasized.is_some() && !emphasized && first.kind != BarKind::Frame {
            color = color.opacity(0.3);
        }
        window.paint_quad(fill(rect, color));

        let hovered = self.hovered.as_ref() == Some(&placed.bars);
        let selected = !placed.is_merged() && self.selected == Some(placed.bars.start);
        if selected || emphasized {
            window.paint_quad(outline(rect, colors.accent, gpui::BorderStyle::Solid));
        } else if hovered {
            window.paint_quad(outline(rect, colors.text, gpui::BorderStyle::Solid));
        }

        if rect.size.width < MIN_LABEL_WIDTH {
            return;
        }
        let label: SharedString = placed.label(layout);
        let text_color = if self.emphasized.is_some() && !emphasized && first.kind != BarKind::Frame
        {
            colors.text_faint
        } else {
            ink(color, theme)
        };
        let available = rect.size.width - LABEL_PADDING * 2.;
        // Keep labels of bars that start left of the view readable.
        let left = rect.left().max(geometry.plot.left());
        let available = available.min(rect.right() - left - LABEL_PADDING * 2.);
        if let Some(line) = fit(
            &label,
            available,
            theme.metrics.text_small,
            text_color,
            window,
        ) {
            window.with_content_mask(Some(gpui::ContentMask { bounds: rect }), |window| {
                line.paint(
                    point(left + LABEL_PADDING, rect.top()),
                    ROW_HEIGHT,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            });
        }
    }

    /// A dashed line where the frame's budget ran out.
    fn paint_budget(&self, geometry: &Geometry, theme: &Theme, window: &mut Window) {
        let x = self.zoom.time_to_x(self.data.budget_end, geometry.width());
        if !(0.0..=geometry.width()).contains(&x) {
            return;
        }
        let x = geometry.plot.left() + px(x);
        let mut y = geometry.plot.top();
        while y < geometry.plot.bottom() {
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(px(1.), px(3.))),
                theme.colors.crit.opacity(0.8),
            ));
            y += px(6.);
        }
    }
}

/// What the tooltip says about a bar, or a merged run of bars.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BarDetails {
    /// The bar's name, or `12 items`.
    pub title: String,
    /// What it is: `rendered view`, `user span`…
    pub kind: &'static str,
    /// Labelled values.
    pub facts: Vec<(&'static str, String)>,
    /// Where it came from, and how to say so (`Opened at`, `Spawned at`).
    pub site: Option<(&'static str, &'static Location<'static>)>,
    /// The view behind it.
    pub view: Option<ViewInfo>,
}

/// Describes the bars `bars` (one, or a merged run) of `data`.
pub(crate) fn bar_details(data: &FlameData, bars: Range<usize>) -> Option<BarDetails> {
    let layout = &data.layout;
    let run = layout.bars.get(bars.clone())?;
    let first = run.first()?;
    let starts = start_label(first.start.as_nanos() as f64 - data.frame_start.as_nanos() as f64);
    if run.len() > 1 {
        let busy: Duration = run.iter().map(|bar| bar.duration).sum();
        let end = run.iter().map(|bar| bar.end()).max().unwrap_or(first.start);
        return Some(BarDetails {
            title: format!("{} items", format::count(run.len() as u64)),
            kind: "too narrow to show one by one: zoom in",
            facts: vec![
                ("Busy", format::duration(busy)),
                ("Spans", format::duration(end.saturating_sub(first.start))),
                ("Starts", starts),
            ],
            site: None,
            view: None,
        });
    }
    let view = data.views.get(bars.start).copied().flatten();
    let mut facts = vec![("Duration", format::duration(first.duration))];
    if let Some(view) = view {
        facts.push(("Self", format::duration(view.self_time)));
    }
    if first.kind == BarKind::Frame {
        facts.push(("App", format::duration(data.app_total)));
        facts.push(("Loupe", format::duration(data.loupe)));
    } else {
        facts.push(("Starts", starts));
    }
    let (title, site) = match first.kind {
        BarKind::View(_) => (
            view.map_or_else(
                || first.label.to_string(),
                |view| format::type_name(view.type_name).into_owned(),
            ),
            None,
        ),
        BarKind::UserSpan => (
            first.label.to_string(),
            first.site.map(|site| ("Opened at", site)),
        ),
        BarKind::Task => (
            "Task".to_string(),
            first.site.map(|site| ("Spawned at", site)),
        ),
        BarKind::Frame => (format!("Frame #{}", data.frame), None),
        _ => (first.label.to_string(), None),
    };
    Some(BarDetails {
        title,
        kind: kind_name(first.kind),
        facts,
        site,
        view,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{FrameBuilder, ms};
    use gpui::inspector::{ForegroundKind, PhaseTimings};

    const BUDGET: Duration = Duration::from_micros(16_667);

    /// A 30 ms frame at 100 ms: App (0..28) ⊃ List (2..26, slow), Sidebar
    /// (26..27, cached); one user span; a task before the frame.
    fn frame() -> FrameRecord {
        FrameBuilder::new()
            .at(ms(100.))
            .phases(PhaseTimings {
                render: ms(28.),
                total: ms(30.),
                ..PhaseTimings::default()
            })
            .view(1, "app::App", 0, ms(0.), ms(28.), ViewOutcome::Rendered)
            .view(2, "app::List", 1, ms(2.), ms(24.), ViewOutcome::Rendered)
            .view(3, "app::Sidebar", 1, ms(26.), ms(1.), ViewOutcome::Cached)
            .span("sort", 0, ms(3.), ms(20.))
            .foreground(
                ForegroundKind::Action { name: "app::Open" },
                ms(90.),
                ms(5.),
            )
            .build()
    }

    fn state(width: f32) -> FlameState {
        let data = Rc::new(FlameData::new(&frame(), BUDGET));
        let mut state = FlameState::default();
        let height = Geometry::height(data.rows());
        state.show(data);
        state.bounds.set(Some(Bounds::new(
            point(px(0.), px(0.)),
            size(px(width), height),
        )));
        state
    }

    fn center_of(state: &FlameState, label: &str) -> Point<Pixels> {
        let data = state.data().unwrap();
        let geometry = state.geometry().unwrap();
        let placed = place(&data.layout, state.zoom(), &geometry);
        let bar = placed
            .iter()
            .find(|bar| bar.label(&data.layout) == label)
            .unwrap_or_else(|| panic!("no bar {label}"));
        geometry.bar_bounds(bar).center()
    }

    #[test]
    fn data_links_view_bars_to_their_views_and_self_times() {
        let data = FlameData::new(&frame(), BUDGET);
        let list = data
            .layout
            .bars
            .iter()
            .position(|bar| bar.label == "List")
            .unwrap();
        let info = data.views[list].unwrap();
        assert_eq!(info.type_name, "app::List");
        assert_eq!(info.self_time, ms(24.));
        let app = data
            .layout
            .bars
            .iter()
            .position(|bar| bar.label == "App")
            .unwrap();
        assert_eq!(data.views[app].unwrap().self_time, ms(3.), "28 − 24 − 1");
        assert!(data.views[0].is_none(), "the frame bar is no view");
        assert_eq!(
            data.lanes,
            vec![
                (Lane::Frame, 0, 1),
                (Lane::Views, 1, 2),
                (Lane::UserSpans, 3, 1),
                (Lane::MainThread, 4, 1),
            ]
        );
        assert_eq!(data.budget_end, ms(100.) + BUDGET);
    }

    #[test]
    fn rows_and_hits_follow_the_geometry() {
        let state = state(652.);
        let geometry = state.geometry().unwrap();
        assert_eq!(geometry.row_at(geometry.row_top(2) + px(1.), 5), Some(2));
        assert_eq!(
            geometry.row_at(geometry.row_top(2) - px(1.), 5),
            None,
            "gap"
        );
        assert_eq!(
            geometry.row_at(geometry.plot.top() - px(1.), 5),
            None,
            "axis"
        );
        assert_eq!(geometry.row_at(geometry.row_top(5), 5), None, "below");

        let data = state.data().unwrap().clone();
        let hit = |label: &str| {
            state
                .hit(center_of(&state, label))
                .map(|bars| data.layout.bars[bars.start].label.clone())
        };
        for label in ["List", "App", "sort", "app::Open"] {
            assert_eq!(hit(label).as_deref(), Some(label));
        }
        assert_eq!(
            state.hit(point(px(10.), geometry.row_top(1) + px(4.))),
            None,
            "gutter"
        );
    }

    #[test]
    fn wheel_zooms_around_the_pointer_and_pans_sideways() {
        let mut state = state(652.);
        let geometry = state.geometry().unwrap();
        let at = point(geometry.plot.left() + px(300.), geometry.row_top(1));
        let time_at = state.zoom().x_to_time(300., geometry.width());
        assert!(state.wheel(at, point(px(0.), px(120.))));
        assert!(state.zoom().is_zoomed());
        let x = state.zoom().time_to_x(time_at, geometry.width());
        assert!(
            (x - 300.).abs() < 0.5,
            "the time under the pointer stays put: {x}"
        );

        let visible = state.zoom().visible();
        assert!(state.wheel(at, point(px(-40.), px(0.))));
        assert!(
            state.zoom().visible().start > visible.start,
            "scrolling right pans later"
        );

        assert!(state.wheel(at, point(px(0.), px(-100_000.))));
        assert!(
            !state.zoom().is_zoomed(),
            "zooming out stops at the whole frame"
        );
        assert!(
            !state.wheel(at, point(px(0.), px(-10.))),
            "nothing left to zoom out"
        );
    }

    #[test]
    fn a_press_is_a_click_until_it_moves_then_it_pans() {
        let mut state = state(652.);
        let list = center_of(&state, "List");
        state.press(list);
        assert!(
            !state.drag_to(list + point(px(1.), px(1.))),
            "within the threshold"
        );
        let bars = state.release(list).expect("a click hits the bar");
        assert_eq!(state.data().unwrap().layout.bars[bars.start].label, "List");

        state.zoom_by(4.);
        let visible = state.zoom().visible();
        state.press(list);
        assert!(state.drag_to(list + point(px(40.), px(0.))));
        assert!(state.is_dragging());
        assert!(
            state.zoom().visible().start < visible.start,
            "dragging right shows earlier time"
        );
        assert_eq!(state.release(list), None, "a drag is not a click");
        assert!(!state.is_dragging());
        state.fit();
        assert!(!state.zoom().is_zoomed());
    }

    #[test]
    fn showing_another_frame_resets_the_view() {
        let mut state = state(652.);
        state.zoom_by(3.);
        state.select(Some(2));
        state.show(Rc::new(FlameData::new(&frame(), BUDGET)));
        assert!(state.zoom().is_zoomed(), "same frame keeps its zoom");
        assert_eq!(state.selected(), Some(2));
        let mut other = frame();
        other.id = 7;
        state.show(Rc::new(FlameData::new(&other, BUDGET)));
        assert!(!state.zoom().is_zoomed());
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn hover_reports_changes_only() {
        let mut state = state(652.);
        let list = center_of(&state, "List");
        assert!(state.hover(Some(list)));
        assert!(!state.hover(Some(list)));
        assert!(
            !state.hover(Some(list + point(px(2.), px(0.)))),
            "along the same bar the tooltip stays put"
        );
        let geometry = state.geometry().unwrap();
        let (_, anchor) = state.hovered().unwrap().clone();
        assert_eq!(anchor, point(list.x, geometry.row_top(2) + ROW_HEIGHT));
        assert!(state.hover(Some(center_of(&state, "App"))));
        assert!(state.hover(None));
        assert!(state.hovered().is_none());
    }

    #[test]
    fn axis_ticks_are_round_and_relative_to_the_frame() {
        let zoom = RangeZoom::new(ms(90.)..ms(130.));
        let ticks = axis_ticks(&zoom, ms(100.), 400., 80.);
        let labels: Vec<&str> = ticks.iter().map(|tick| tick.label.as_str()).collect();
        assert_eq!(labels, ["−10.0 ms", "0", "10.0 ms", "20.0 ms", "30.0 ms"]);
        assert!((ticks[1].x - 100.).abs() < 1e-3);

        let mut zoom = zoom;
        zoom.show(ms(100.)..ms(100.5));
        let ticks = axis_ticks(&zoom, ms(100.), 400., 80.);
        assert_eq!(ticks[1].label, "100 µs");
        assert!(axis_ticks(&RangeZoom::new(ms(1.)..ms(1.)), ms(1.), 400., 80.).is_empty());
    }

    #[test]
    fn nice_steps_are_one_two_or_five() {
        let steps: Vec<f64> = [0.5, 1.0, 1.1, 3.0, 7.0, 42.0, 999.0]
            .into_iter()
            .map(nice_step)
            .collect();
        assert_eq!(steps, [1.0, 1.0, 2.0, 5.0, 10.0, 50.0, 1000.0]);
    }

    #[test]
    fn details_describe_views_spans_and_merged_runs() {
        let data = FlameData::new(&frame(), BUDGET);
        let position = |label: &str| {
            data.layout
                .bars
                .iter()
                .position(|bar| bar.label == label)
                .unwrap()
        };
        let list = bar_details(&data, position("List")..position("List") + 1).unwrap();
        assert_eq!(list.title, "List");
        assert_eq!(list.kind, "rendered view");
        assert_eq!(
            list.facts,
            [
                ("Duration", "24.0 ms".to_string()),
                ("Self", "24.0 ms".to_string()),
                ("Starts", "+2.0 ms".to_string()),
            ]
        );
        let sort = bar_details(&data, position("sort")..position("sort") + 1).unwrap();
        assert_eq!(sort.site.map(|(label, _)| label), Some("Opened at"));
        let action = bar_details(&data, position("app::Open")..position("app::Open") + 1).unwrap();
        assert_eq!(action.facts[1], ("Starts", "−10.0 ms".to_string()));
        let frame_bar = bar_details(&data, 0..1).unwrap();
        assert_eq!(frame_bar.title, "Frame #0");
        assert_eq!(
            frame_bar.facts,
            [
                ("Duration", "30.0 ms".to_string()),
                ("App", "30.0 ms".to_string()),
                ("Loupe", "0 ns".to_string()),
            ]
        );
        let views = position("App")..position("Sidebar") + 1;
        let merged = bar_details(&data, views).unwrap();
        assert_eq!(merged.title, "3 items");
        assert_eq!(merged.facts[0], ("Busy", "53.0 ms".to_string()));
        assert_eq!(bar_details(&data, 99..100), None);
    }

    #[test]
    fn the_legend_lists_each_kind_once() {
        let data = FlameData::new(&frame(), BUDGET);
        let names: Vec<&str> = legend(&data.layout).into_iter().map(kind_name).collect();
        assert_eq!(
            names,
            ["rendered view", "cached view", "user span", "action"]
        );
    }
}
