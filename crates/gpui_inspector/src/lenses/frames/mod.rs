//! Frames: why was that frame drawn, and why was it slow?
//!
//! ```text
//! stats       118 fps · p50 p95 p99 · over budget · input p95 · Loupe's share · budget
//! frame       ‹ #18372 · 23.4 ms · 2.1 s ago  2.1× budget ›      Latest · Jump to worst
//!             phase bar with the budget tick, legend, verdict
//! why drawn   one sentence per cause with its site; the input before the frame
//! flame       views, user spans and main-thread work; wheel zoom, drag pan
//! bottom-up   self time by view type, for this frame or all frames
//! insights    plain-language findings, with links to their causes
//! export      the recording as Chrome trace JSON, for ui.perfetto.dev
//! ```
//!
//! The lens shows the frame selected in the pulse strip, or follows the
//! latest app frame; zooming or clicking the flame chart pins the frame on
//! screen. Frames Loupe drew for itself replay the app, and say so. Narrow
//! docks stack everything in one scrolling column; wide and bottom docks
//! put the frame's story on the left and where its time went (flame chart,
//! bottom-up, insights) on the right. Everything derived from the capture
//! is memoized per capture generation and selection.

mod bottom_up;
mod export;
mod flame;
mod insights;
mod model;
mod phase_bar;
mod render;
mod text;

pub use export::ExportDirectory;

use self::{
    bottom_up::Scope,
    export::{Trace, default_directory},
    flame::{FlameData, FlameState, WHEEL_LINE},
    model::{StatsLine, shown_frame, step_frame},
};
use super::{
    RailBadge,
    highlights::{LensHighlights, element_highlight},
    links::{Navigate, follow},
    memo::Memo,
    observe_state,
};
use crate::{
    analysis::{
        bottom_up::BottomUpRow,
        format,
        insights::{Insight, recent_app_frames},
        stats::{Grade, worst_frame},
    },
    state::{Lens, LensLayout, LoupeState},
    theme::Theme,
    widgets::{IconName, TableEvent, TableState, Tone},
};
use gpui::{
    App, AppContext as _, ClipboardItem, ColorExt as _, Context, Entity, FocusHandle, Hsla,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollWheelEvent, SharedString, Subscription, Task, Window, actions,
    inspector::{ElementKey, ElementKind, InspectorCapture, OverlayHighlight},
    px,
};
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};

actions!(
    loupe_frames,
    [
        /// Shows the previous app frame.
        PreviousFrame,
        /// Shows the next app frame.
        NextFrame,
        /// Shows the slowest app frame in the recording.
        JumpToWorst,
        /// Follows the latest app frame again.
        FollowLatest,
        /// Zooms the flame chart in.
        ZoomIn,
        /// Zooms the flame chart out.
        ZoomOut,
        /// Shows the whole frame in the flame chart.
        ZoomToFit,
    ]
);

/// Key context of the Frames lens.
pub(crate) const CONTEXT: &str = "LoupeFrames";
/// Below this width the lens stacks in one column: the flame chart and the
/// bottom-up table need the room.
const TWO_COLUMNS_WIDTH: Pixels = px(760.);
/// Most bottom-up rows shown before the table scrolls.
const TABLE_ROWS: usize = 10;

/// The lens' keys.
pub(crate) fn key_bindings() -> [KeyBinding; 8] {
    let context = Some(CONTEXT);
    [
        KeyBinding::new("left", PreviousFrame, context),
        KeyBinding::new("right", NextFrame, context),
        KeyBinding::new("w", JumpToWorst, context),
        KeyBinding::new("l", FollowLatest, context),
        KeyBinding::new("=", ZoomIn, context),
        KeyBinding::new("+", ZoomIn, context),
        KeyBinding::new("-", ZoomOut, context),
        KeyBinding::new("0", ZoomToFit, context),
    ]
}

/// How the lens lays itself out in its dock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Columns {
    /// The frame's story beside where its time went, or one column.
    two: bool,
    /// The width of the pane holding the flame chart and bottom-up table.
    detail: Pixels,
}

/// The summary column's share of a two-column lens, and its limits.
const SUMMARY_SHARE: f32 = 0.42;
const SUMMARY_MIN: Pixels = px(340.);
const SUMMARY_MAX: Pixels = px(520.);
/// Detail panes at least this wide show every bottom-up column.
const WIDE_TABLE: Pixels = px(600.);

impl Columns {
    /// The layout for a lens `width` wide, docked where `layout` says.
    fn new(layout: LensLayout, width: Pixels) -> Self {
        let two = layout == LensLayout::SideBySide && width >= TWO_COLUMNS_WIDTH;
        let detail = if two {
            width - (width * SUMMARY_SHARE).clamp(SUMMARY_MIN, SUMMARY_MAX)
        } else {
            width
        };
        Self { two, detail }
    }

    /// The layout for Loupe's dock in `window`.
    fn of(window: &Window) -> Self {
        let width = window
            .inspector_bounds()
            .map_or(Pixels::ZERO, |bounds| bounds.size.width);
        Self::new(LensLayout::of(window), width)
    }

    /// Whether the bottom-up table has room for every column.
    fn wide_table(self) -> bool {
        self.detail >= WIDE_TABLE
    }
}

/// A message about the last thing the lens did for the user.
struct Notice {
    text: SharedString,
    /// The file it wrote, to reveal.
    path: Option<PathBuf>,
    tone: Tone,
}

/// The Frames lens.
pub(crate) struct FramesLens {
    state: Entity<LoupeState>,
    focus: FocusHandle,
    flame: FlameState,
    /// A view type picked in the bottom-up table, highlighted everywhere.
    emphasized: Option<&'static str>,
    scope: Scope,
    table: Entity<TableState<&'static str>>,
    /// The rows the table was last given, and whether with wide columns.
    table_rows: Option<Rc<Vec<BottomUpRow>>>,
    table_wide: bool,
    highlights: LensHighlights,
    notice: Option<Notice>,
    stats: Memo<(u64, Duration), StatsLine>,
    flame_data: Memo<(u64, u64, Duration), FlameData>,
    rows: Memo<(u64, u64, Scope), Vec<BottomUpRow>>,
    insights: Memo<(u64, Option<u64>, Duration), Vec<Insight>>,
    badge: RefCell<Memo<(u64, Duration), Option<RailBadge>>>,
    idle_check: Option<(u64, Task<()>)>,
    _subscriptions: Vec<Subscription>,
}

impl FramesLens {
    pub fn new(state: Entity<LoupeState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        let table = cx.new(TableState::<&'static str>::new);
        let subscriptions = vec![
            cx.observe_in(&state, window, |this, state, window, cx| {
                let lens = state.read(cx).lens();
                let mut changed = this.highlights.sync(lens, window);
                if lens == Lens::Frames && this.emphasized.is_some() {
                    changed |= this.pin_emphasized(window, cx);
                }
                if changed {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&table, window, |this, _, event, window, cx| match *event {
                TableEvent::Selected(type_name) | TableEvent::Activated(type_name) => {
                    this.emphasize(Some(type_name), window, cx)
                }
            }),
        ];
        Self {
            state,
            focus: cx.focus_handle(),
            flame: FlameState::default(),
            emphasized: None,
            scope: Scope::Frame,
            table,
            table_rows: None,
            table_wide: false,
            highlights: LensHighlights::new(Lens::Frames),
            notice: None,
            stats: Memo::default(),
            flame_data: Memo::default(),
            rows: Memo::default(),
            insights: Memo::default(),
            badge: RefCell::default(),
            idle_check: None,
            _subscriptions: subscriptions,
        }
    }

    fn navigator(&self, cx: &mut Context<Self>) -> Navigate {
        let this = cx.entity().downgrade();
        Rc::new(move |target, _, cx| {
            this.update(cx, |this, cx| {
                if let Some(message) = follow(target, &this.state, cx) {
                    this.notify_user(message, None, Tone::Neutral, cx);
                }
            })
            .ok();
        })
    }

    fn notify_user(
        &mut self,
        text: impl Into<SharedString>,
        path: Option<PathBuf>,
        tone: Tone,
        cx: &mut Context<Self>,
    ) {
        self.notice = Some(Notice {
            text: text.into(),
            path,
            tone,
        });
        cx.notify();
    }

    /// The frame on screen: the selection, or the latest app frame.
    fn shown_frame_id(&self, window: &Window, cx: &App) -> Option<u64> {
        let capture = window.inspector_capture()?;
        shown_frame(capture, self.state.read(cx).selected_frame()).map(|(frame, _)| frame.id)
    }

    fn select_frame(&mut self, frame: Option<u64>, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, cx| state.select_frame(frame, cx));
    }

    /// Stops following the latest frame, so what the user is looking at
    /// stays put while they zoom and click.
    fn pin_shown_frame(&mut self, cx: &mut Context<Self>) {
        let shown = self.flame.data().map(|data| data.frame);
        if self.state.read(cx).selected_frame().is_none() && shown.is_some() {
            self.select_frame(shown, cx);
        }
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.shown_frame_id(window, cx) else {
            return;
        };
        let target = window
            .inspector_capture()
            .and_then(|capture| step_frame(capture.frames(), current, delta));
        if let Some(frame) = target {
            self.select_frame(Some(frame), cx);
        }
    }

    fn jump_to_worst(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let worst = window
            .inspector_capture()
            .and_then(|capture| worst_frame(capture.frames()))
            .map(|frame| frame.id);
        if worst.is_some() {
            self.select_frame(worst, cx);
        }
    }

    fn zoom(&mut self, factor: Option<f64>, cx: &mut Context<Self>) {
        match factor {
            Some(factor) => self.flame.zoom_by(factor),
            None => self.flame.fit(),
        }
        self.pin_shown_frame(cx);
        cx.notify();
    }

    // Flame chart input.

    fn flame_hover(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.flame.is_dragging() {
            return;
        }
        if !self.flame.hover(Some(event.position)) {
            return;
        }
        let highlight = self
            .flame
            .hovered()
            .and_then(|(bars, _)| self.bar_highlight(bars.start, window, cx));
        self.highlights
            .hover(highlight.map(|highlight| vec![highlight]), window);
        cx.notify();
    }

    fn flame_leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.flame.hover(None) {
            self.highlights.hover(None, window);
            cx.notify();
        }
    }

    fn flame_drag(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button == Some(MouseButton::Left) && self.flame.drag_to(event.position) {
            self.pin_shown_frame(cx);
            cx.notify();
        }
    }

    fn flame_press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if event.click_count >= 2 {
            self.zoom(None, cx);
            return;
        }
        self.flame.press(event.position);
    }

    fn flame_release(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let dragged = self.flame.is_dragging();
        let Some(bars) = self.flame.release(event.position) else {
            if dragged {
                cx.notify();
            }
            return;
        };
        self.pin_shown_frame(cx);
        let Some(data) = self.flame.data().cloned() else {
            return;
        };
        if bars.len() > 1 {
            // A merged run: zoom until its bars stand alone.
            let run = &data.layout.bars[bars];
            let start = run.iter().map(|bar| bar.start).min().unwrap_or_default();
            let end = run.iter().map(|bar| bar.end()).max().unwrap_or(start);
            self.flame.show_range(start..end);
            cx.notify();
            return;
        }
        let reveal = event.modifiers.shift || event.modifiers.secondary();
        self.select_bar(bars.start, reveal, window, cx);
    }

    fn flame_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let delta = event.delta.pixel_delta(WHEEL_LINE);
        if self.flame.wheel(event.position, delta) {
            self.pin_shown_frame(cx);
            cx.notify();
        }
    }

    /// Selects a flame bar; a view's element is selected in the app too,
    /// and with `reveal` shown in Elements.
    fn select_bar(
        &mut self,
        bar: usize,
        reveal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.flame.select(Some(bar));
        if let Some(element) = self.bar_element(bar, window) {
            self.state.update(cx, |state, cx| {
                state.select_element(Some(element), cx);
                if reveal {
                    state.set_lens(Lens::Elements, cx);
                }
            });
        }
        cx.notify();
    }

    /// The element of the view a bar stands for: from the frame's own tree,
    /// else the latest tree's record of the same view entity.
    fn bar_element_in(&self, capture: &InspectorCapture, bar: usize) -> Option<ElementKey> {
        let data = self.flame.data()?;
        let view = data.views.get(bar).copied().flatten()?;
        let from_frame = capture
            .frame(data.frame)
            .and_then(|frame| frame.tree.as_ref()?.get(view.element?)?.key);
        from_frame.or_else(|| {
            let tree = capture.latest_tree()?;
            tree.elements
                .iter()
                .find(|record| {
                    matches!(record.kind, ElementKind::View { entity, .. } if entity == view.entity)
                })?
                .key
        })
    }

    fn bar_element(&self, bar: usize, window: &Window) -> Option<ElementKey> {
        self.bar_element_in(window.inspector_capture()?, bar)
    }

    /// A highlight over the element of the view a bar stands for.
    fn bar_highlight(&self, bar: usize, window: &Window, cx: &App) -> Option<OverlayHighlight> {
        let data = self.flame.data()?;
        let flame_bar = data.layout.bars.get(bar)?;
        let key = self.bar_element(bar, window)?;
        let label = format!(
            "{} · {}",
            flame_bar.label,
            format::duration(flame_bar.duration)
        );
        let color = Theme::of(window, cx).colors.accent.opacity(0.22);
        highlight_element(window.inspector_capture()?, data.frame, key, label, color)
    }

    // Bottom-up.

    fn emphasize(
        &mut self,
        type_name: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.emphasized == type_name {
            return;
        }
        self.emphasized = type_name;
        if type_name.is_none() {
            self.table.update(cx, |table, cx| table.deselect(cx));
        }
        self.pin_emphasized(window, cx);
        cx.notify();
    }

    /// Pins highlights over every element of the emphasized view type.
    /// Returns whether the overlay changed.
    fn pin_emphasized(&mut self, window: &mut Window, cx: &App) -> bool {
        let color = Theme::of(window, cx).colors.accent.opacity(0.16);
        let highlights = match (self.emphasized, window.inspector_capture()) {
            (Some(type_name), Some(capture)) => view_highlights(capture, type_name, color),
            _ => Vec::new(),
        };
        self.highlights.pin(highlights, window)
    }

    fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        self.scope = scope;
        cx.notify();
    }

    // Export.

    fn export_trace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(capture) = window.inspector_capture() else {
            return;
        };
        let trace = Trace::of(capture, &window.window_title());
        let summary = trace.summary();
        let directory = cx.try_global::<ExportDirectory>().map(|dir| dir.0.clone());
        let prompt = match &directory {
            Some(_) => None,
            None => Some(cx.prompt_for_new_path(&default_directory(), Some(&trace.file_name))),
        };
        let fixed_path = directory.map(|directory| directory.join(&trace.file_name));
        cx.spawn_in(window, async move |this, cx| {
            let path = match (fixed_path, prompt) {
                (Some(path), _) => Some(path),
                (None, Some(prompt)) => prompt.await.ok().and_then(Result::ok).flatten(),
                (None, None) => None,
            };
            let Some(path) = path else {
                return;
            };
            let json = trace.json;
            let written = cx
                .background_spawn(async move { export::write(path, &json) })
                .await;
            this.update(cx, |this, cx| match written {
                Ok(path) => this.notify_user(
                    format!("Saved {summary} to {}", path.display()),
                    Some(path),
                    Tone::Ok,
                    cx,
                ),
                Err(error) => this.notify_user(
                    format!("Could not save the trace: {error}"),
                    None,
                    Tone::Crit,
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    fn copy_trace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(capture) = window.inspector_capture() else {
            return;
        };
        let trace = Trace::of(capture, &window.window_title());
        let summary = trace.summary();
        cx.write_to_clipboard(ClipboardItem::new_string(trace.json));
        self.notify_user(
            format!("Copied {summary} of Chrome trace JSON"),
            None,
            Tone::Ok,
            cx,
        );
    }

    /// Re-renders once the app has been quiet for a second, so the stats
    /// line can say `idle` (new frames re-render the lens anyway).
    fn schedule_idle_check(
        &mut self,
        generation: u64,
        idle_in: Option<Duration>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(idle_in) = idle_in else {
            self.idle_check = None;
            return;
        };
        if self
            .idle_check
            .as_ref()
            .is_some_and(|(scheduled, _)| *scheduled == generation)
        {
            return;
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(idle_in + Duration::from_millis(50))
                .await;
            this.update(cx, |_, cx| cx.notify()).ok();
        });
        self.idle_check = Some((generation, task));
    }
}

/// A highlight over `key`, where the latest tree has it (else where the
/// frame's own tree had it).
fn highlight_element(
    capture: &InspectorCapture,
    frame: u64,
    key: ElementKey,
    label: String,
    color: Hsla,
) -> Option<OverlayHighlight> {
    let label = Some(SharedString::from(label));
    capture
        .latest_tree()
        .and_then(|tree| element_highlight(tree, key, color, label.clone()))
        .or_else(|| {
            let tree = capture.frame(frame)?.tree.as_ref()?;
            element_highlight(tree, key, color, label)
        })
}

/// Highlights over every view of `type_name` in the latest tree.
fn view_highlights(
    capture: &InspectorCapture,
    type_name: &'static str,
    color: Hsla,
) -> Vec<OverlayHighlight> {
    let Some(tree) = capture.latest_tree() else {
        return Vec::new();
    };
    let label = SharedString::from(format::type_name(type_name).into_owned());
    tree.elements
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                ElementKind::View { type_name: drawn, .. } if drawn == type_name
            )
        })
        .map(|record| OverlayHighlight {
            bounds: record.bounds,
            color,
            label: Some(label.clone()),
        })
        .collect()
}

/// The rail badge: frames over budget in the last second of app frames.
fn over_budget_badge(capture: &InspectorCapture) -> Option<RailBadge> {
    let budget = capture.config().budget;
    let (warn, crit) =
        recent_app_frames(capture.frames()).fold(
            (0, 0),
            |(warn, crit), frame| match Grade::of_frame(frame, budget) {
                Grade::Ok => (warn, crit),
                Grade::Warn => (warn + 1, crit),
                Grade::Crit => (warn, crit + 1),
            },
        );
    let tone = match (warn, crit) {
        (0, 0) => return None,
        (_, 0) => Tone::Warn,
        _ => Tone::Crit,
    };
    Some(RailBadge::alert(format::count(warn + crit), tone).marker(IconName::TriangleUp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{FrameBuilder, ms, steady_frames};
    use gpui::inspector::CauseKind;

    #[test]
    fn columns_split_only_wide_docks() {
        let narrow = Columns::new(LensLayout::Stacked, px(400.));
        assert_eq!((narrow.two, narrow.detail), (false, px(400.)));
        assert!(!narrow.wide_table());
        let default_dock = Columns::new(LensLayout::SideBySide, px(560.));
        assert!(!default_dock.two, "the default right dock keeps one column");
        assert!(!default_dock.wide_table());
        let wide = Columns::new(LensLayout::SideBySide, px(1280.));
        assert_eq!((wide.two, wide.detail), (true, px(760.)));
        assert!(wide.wide_table());
        let just = Columns::new(LensLayout::SideBySide, px(760.));
        assert_eq!((just.two, just.detail), (true, px(420.)));
        assert!(!just.wide_table());
    }

    #[test]
    fn the_rail_counts_recent_frames_over_budget() {
        let mut capture = steady_frames(30, 4.0);
        assert_eq!(over_budget_badge(&capture), None, "all within budget");
        FrameBuilder::new()
            .at(ms(510.))
            .app_time(ms(20.), ms(0.3))
            .push(&mut capture);
        let warn = over_budget_badge(&capture).unwrap();
        assert_eq!((warn.text.as_ref(), warn.tone), ("1", Tone::Warn));
        assert_eq!(warn.marker, Some(IconName::TriangleUp));
        FrameBuilder::new()
            .at(ms(540.))
            .app_time(ms(40.), ms(0.3))
            .push(&mut capture);
        let crit = over_budget_badge(&capture).unwrap();
        assert_eq!((crit.text.as_ref(), crit.tone), ("2", Tone::Crit));

        // Only the last second counts, and Loupe's own frames never do.
        FrameBuilder::new()
            .at(ms(3_000.))
            .app_time(ms(4.), ms(0.3))
            .push(&mut capture);
        FrameBuilder::new()
            .at(ms(3_020.))
            .app_time(ms(90.), ms(0.3))
            .cause(CauseKind::Input { event: "MouseMove" }, true)
            .inspector_only()
            .push(&mut capture);
        assert_eq!(over_budget_badge(&capture), None);
    }
}
