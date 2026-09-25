//! The event log: a virtualized list of input records, newest last, that
//! follows new input until the user scrolls up or selects a record.

use super::{EventsLens, Pane};
use crate::{
    analysis::{
        events::{LogFilter, LogIndex, clock, kind_label, primary_action, record_at, record_keys},
        format,
        insights::{SLOW_INPUT, VERY_SLOW_INPUT},
    },
    theme::{MONO_FONT, Theme},
    widgets::{
        Button, ButtonSize, ButtonStyle, EmptyState, Icon, IconName, Kbd, LIST_CONTEXT, Pill,
        SelectFirst, SelectLast, SelectNext, SelectPrevious, Tooltip,
    },
};
use gpui::{
    AnyElement, App, ClickEvent, Context, FocusHandle, FontWeight, IntoElement, Pixels,
    ScrollStrategy, SharedString, UniformListScrollHandle, Window, div,
    inspector::{
        ElementIndex, ElementKey, ElementRecord, InputKind, InputRecord, InspectorCapture,
    },
    prelude::*,
    px, uniform_list,
};
use std::{collections::HashMap, ops::Range};

const TIME_WIDTH: Pixels = px(46.);
const KIND_WIDTH: Pixels = px(14.);
const TOOK_WIDTH: Pixels = px(50.);
const REDRAW_WIDTH: Pixels = px(8.);
const HANDLED_WIDTH: Pixels = px(12.);
const COLUMN_GAP: Pixels = px(6.);

/// Whether the log keeps the newest record in view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Follow {
    /// New records scroll into view.
    Following,
    /// The view stays put.
    Paused {
        /// The newest listed record when following stopped.
        after: Option<u64>,
        /// Paused by scrolling up, so scrolling back down resumes.
        by_scroll: bool,
    },
}

/// What the listed rows were computed from.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RowsKey {
    generation: u64,
    len: usize,
    newest: Option<(u64, u32)>,
    filter: LogFilter,
}

/// Element names from the latest tree, for searching hit paths and naming
/// them in the detail. Indexed once per tree.
#[derive(Debug, Default)]
struct ElementNames {
    frame: Option<u64>,
    index: HashMap<ElementKey, ElementIndex>,
}

impl ElementNames {
    fn refresh(&mut self, capture: &InspectorCapture) {
        let tree = capture.latest_tree();
        let frame = tree.map(|tree| tree.frame);
        if frame == self.frame {
            return;
        }
        self.frame = frame;
        self.index.clear();
        if let Some(tree) = tree {
            self.index.extend(
                tree.elements
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, record)| Some((record.key?, ix as ElementIndex))),
            );
        }
    }

    fn record<'a>(
        &self,
        capture: &'a InspectorCapture,
        key: ElementKey,
    ) -> Option<&'a ElementRecord> {
        let ix = *self.index.get(&key)?;
        capture.latest_tree()?.get(ix)
    }

    /// `div#row-3` from the latest tree, else where the element was built.
    fn name(&self, capture: &InspectorCapture, key: ElementKey) -> Option<String> {
        match self.record(capture, key) {
            Some(record) => Some(format::element_label(record)),
            None => capture
                .path_info(key.path)
                .map(|info| format::location(info.source)),
        }
    }
}

/// The log's state: the index, the listed rows and following.
pub(super) struct EventLog {
    index: LogIndex,
    names: ElementNames,
    key: Option<RowsKey>,
    /// Sequence numbers of the listed records, oldest first.
    rows: Vec<u64>,
    /// Records that pass the Loupe toggle, before the kind and text filters.
    listed: usize,
    /// Loupe's own records hidden by the toggle.
    hidden_loupe: usize,
    follow: Follow,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
}

impl EventLog {
    pub fn new(cx: &mut App) -> Self {
        Self {
            index: LogIndex::default(),
            names: ElementNames::default(),
            key: None,
            rows: Vec::new(),
            listed: 0,
            hidden_loupe: 0,
            follow: Follow::Following,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
        }
    }

    /// Catches up with the capture, re-filtering only when the input or
    /// the filter changed. While following, new rows scroll into view.
    pub fn refresh(&mut self, capture: &InspectorCapture, filter: &LogFilter) {
        let input = capture.input();
        let key = RowsKey {
            generation: capture.generation(),
            len: input.len(),
            newest: input.back().map(|record| (record.seq, record.coalesced)),
            filter: filter.clone(),
        };
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.names.refresh(capture);
        let names = &self.names;
        self.index.sync(input, |key| names.name(capture, key));
        let newest = self.rows.last().copied();
        self.rows = self.index.visible(input, filter);
        let loupe = input.iter().filter(|record| record.inspector).count();
        self.hidden_loupe = if filter.show_loupe { 0 } else { loupe };
        self.listed = input.len() - self.hidden_loupe;
        if self.follow == Follow::Following && self.rows.last().copied() != newest {
            self.scroll.scroll_to_bottom();
        }
        self.key = Some(key);
    }

    /// The name of an element in a hit path.
    pub fn element_name(&self, capture: &InspectorCapture, key: ElementKey) -> String {
        self.names
            .name(capture, key)
            .unwrap_or_else(|| "element".to_string())
    }

    /// The latest tree's record of an element, if it is still drawn.
    pub fn element<'a>(
        &self,
        capture: &'a InspectorCapture,
        key: ElementKey,
    ) -> Option<&'a ElementRecord> {
        self.names.record(capture, key)
    }

    /// Focuses the list, for keyboard navigation.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    /// The newest listed record.
    pub fn newest(&self) -> Option<u64> {
        self.rows.last().copied()
    }

    fn row_of(&self, seq: u64) -> Option<usize> {
        self.rows.binary_search(&seq).ok()
    }

    /// Stops following; scrolling back to the end resumes only when
    /// scrolling up paused it.
    fn pause(&mut self, by_scroll: bool) {
        if self.follow == Follow::Following {
            self.follow = Follow::Paused {
                after: self.rows.last().copied(),
                by_scroll,
            };
        }
    }

    fn resume(&mut self) {
        self.follow = Follow::Following;
        self.scroll.scroll_to_bottom();
    }

    /// Records listed since following stopped.
    fn new_since_pause(&self) -> usize {
        match self.follow {
            Follow::Following => 0,
            Follow::Paused { after: None, .. } => self.rows.len(),
            Follow::Paused {
                after: Some(after), ..
            } => self.rows.len() - self.rows.partition_point(|&seq| seq <= after),
        }
    }

    /// After a scroll: scrolling up pauses, scrolling back to the end
    /// resumes a pause that scrolling caused.
    fn note_scroll(&mut self) -> bool {
        let at_end = self.scroll.is_scrolled_to_end().unwrap_or(true);
        match self.follow {
            Follow::Following if !at_end => {
                self.pause(true);
                true
            }
            Follow::Paused {
                by_scroll: true, ..
            } if at_end => {
                self.follow = Follow::Following;
                true
            }
            _ => false,
        }
    }
}

impl EventsLens {
    /// Selects the record `seq`, which pauses following.
    pub(super) fn select_input(&mut self, seq: u64, cx: &mut Context<Self>) {
        self.log.pause(false);
        self.pane = Pane::Event;
        if let Some(row) = self.log.row_of(seq) {
            self.log.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
        self.state
            .update(cx, |state, cx| state.select_input(Some(seq), cx));
        cx.notify();
    }

    /// Moves the selection `delta` rows; with nothing selected, selects the newest.
    fn step_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(last) = self.log.rows.len().checked_sub(1) else {
            return;
        };
        let selected = self.state.read(cx).selected_input();
        let row = match selected.and_then(|seq| self.log.row_of(seq)) {
            Some(row) => row.saturating_add_signed(delta).min(last),
            None => last,
        };
        self.select_input(self.log.rows[row], cx);
    }

    fn select_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(&seq) = self.log.rows.get(row) {
            self.select_input(seq, cx);
        }
    }

    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        match self.log.follow {
            Follow::Following => self.log.pause(false),
            Follow::Paused { .. } => self.log.resume(),
        }
        cx.notify();
    }

    pub(super) fn render_log(
        &mut self,
        theme: &'static Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = &theme.colors;
        let body = if self.log.rows.is_empty() {
            self.render_log_empty(window).into_any_element()
        } else {
            uniform_list(
                "events-log",
                self.log.rows.len(),
                cx.processor(|this, range: Range<usize>, window, cx| {
                    this.render_rows(range, window, cx)
                }),
            )
            .size_full()
            .track_scroll(&self.log.scroll)
            .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(log_header(theme))
            .child(
                div()
                    .id("events-log-body")
                    .key_context(LIST_CONTEXT)
                    .track_focus(&self.log.focus)
                    .flex_1()
                    .min_h_0()
                    .bg(colors.bg)
                    .on_action(
                        cx.listener(|this, _: &SelectNext, _, cx| this.step_selection(1, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &SelectPrevious, _, cx| this.step_selection(-1, cx)),
                    )
                    .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.select_row(0, cx)))
                    .on_action(cx.listener(|this, _: &SelectLast, _, cx| {
                        this.select_row(this.log.rows.len().saturating_sub(1), cx)
                    }))
                    .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                        if this.log.note_scroll() {
                            cx.notify();
                        }
                    }))
                    .child(body),
            )
            .child(self.render_footer(theme, cx))
    }

    fn render_log_empty(&self, window: &Window) -> impl IntoElement + use<> {
        let recorded = window
            .inspector_capture()
            .is_some_and(|capture| !capture.input().is_empty());
        if recorded && self.log.listed > 0 {
            EmptyState::new("No events match")
                .icon(IconName::Filter)
                .description("Try another kind, or remove terms from the filter.")
        } else {
            EmptyState::new("No input recorded yet")
                .icon(IconName::Keyboard)
                .description(
                    "Click, scroll or type in the app: every event lands here with where it went.",
                )
        }
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = Theme::of(window, cx);
        let Some(capture) = window.inspector_capture() else {
            return Vec::new();
        };
        let selected = self.state.read(cx).selected_input();
        self.log.rows[range.start.min(self.log.rows.len())..range.end.min(self.log.rows.len())]
            .iter()
            .filter_map(|&seq| record_at(capture.input(), seq))
            .map(|record| {
                let seq = record.seq;
                log_row(record, selected == Some(seq), theme)
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.select_input(seq, cx)),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn render_footer(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = &theme.colors;
        let shown = self.log.rows.len();
        let summary = if shown == self.log.listed {
            format!("{} events", format::count(shown as u64))
        } else {
            format!(
                "{} of {} events",
                format::count(shown as u64),
                format::count(self.log.listed as u64)
            )
        };
        let loupe_tooltip = match self.log.hidden_loupe {
            0 => "List the input Loupe itself consumed".to_string(),
            hidden => format!(
                "List the {} events Loupe itself consumed",
                format::count(hidden as u64)
            ),
        };
        let follow = match self.log.follow {
            Follow::Following => Button::new("events-follow")
                .icon(IconName::ArrowDown)
                .label("Follow")
                .toggle_state(true)
                .tooltip("Following the newest event; scroll up or select one to pause"),
            Follow::Paused { .. } => {
                let fresh = self.log.new_since_pause();
                let label = if fresh > 0 {
                    format!("Paused · {} new", format::count(fresh as u64))
                } else {
                    "Paused".to_string()
                };
                Button::new("events-follow")
                    .icon(IconName::Pause)
                    .label(label)
                    .style(ButtonStyle::Subtle)
                    .tooltip("Jump to the newest event and keep following")
            }
        };
        div()
            .flex_none()
            .h(theme.metrics.status + px(4.))
            .px(theme.metrics.gutter)
            .flex()
            .items_center()
            .gap_2()
            .bg(colors.surface)
            .border_t_1()
            .border_color(colors.line)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme.metrics.text_small)
                    .text_color(colors.text_muted)
                    .child(summary),
            )
            .child(
                Button::new("events-show-loupe")
                    .label("Show Loupe's input")
                    .size(ButtonSize::Small)
                    .toggle_state(self.filter.show_loupe)
                    .tooltip(loupe_tooltip)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_loupe_input(cx))),
            )
            .child(
                follow
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_follow(cx))),
            )
    }
}

/// The glyph of an input kind.
pub(super) fn kind_icon(kind: InputKind) -> IconName {
    match kind {
        InputKind::KeyDown | InputKind::KeyUp | InputKind::Modifiers => IconName::Keyboard,
        InputKind::MouseDown | InputKind::MouseUp => IconName::Mouse,
        InputKind::MouseMove | InputKind::MouseExit | InputKind::Gesture | InputKind::Touch => {
            IconName::Pointer
        }
        InputKind::Scroll => IconName::Scroll,
        InputKind::FileDrop => IconName::Copy,
        InputKind::Action => IconName::Command,
    }
}

fn log_header(theme: &Theme) -> impl IntoElement + use<> {
    let colors = &theme.colors;
    let title = |title: &'static str| div().truncate().child(title);
    div()
        .flex_none()
        .h(theme.metrics.control)
        .px(theme.metrics.gutter)
        .flex()
        .items_center()
        .gap(COLUMN_GAP)
        .bg(colors.surface)
        .border_b_1()
        .border_color(colors.line)
        .text_size(theme.metrics.text_small)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_muted)
        .child(
            div()
                .flex_none()
                .w(TIME_WIDTH)
                .flex()
                .justify_end()
                .child(title("Time")),
        )
        .child(div().flex_none().w(KIND_WIDTH))
        .child(event_cell().child(title("Event")))
        .child(actions_cell().child(title("Actions")))
        .child(
            div()
                .flex_none()
                .w(TOOK_WIDTH)
                .flex()
                .justify_end()
                .child(title("Took")),
        )
        .child(
            div()
                .flex_none()
                .w(REDRAW_WIDTH + HANDLED_WIDTH + COLUMN_GAP),
        )
}

fn event_cell() -> gpui::Div {
    div()
        .flex_grow(1.4)
        .flex_basis(px(0.))
        .min_w_0()
        .flex()
        .items_center()
        .gap_1()
        .overflow_hidden()
}

fn actions_cell() -> gpui::Div {
    div()
        .flex_grow(1.)
        .flex_basis(px(0.))
        .min_w_0()
        .overflow_hidden()
}

/// One log row, without its click handler.
fn log_row(
    record: &InputRecord,
    selected: bool,
    theme: &'static Theme,
) -> gpui::Stateful<gpui::Div> {
    let colors = &theme.colors;
    let seq = record.seq;
    let took_color = if record.duration >= VERY_SLOW_INPUT {
        colors.crit
    } else if record.duration >= SLOW_INPUT {
        colors.warn
    } else {
        colors.text_muted
    };
    let action = primary_action(record);
    let names = |short: bool| -> SharedString {
        record
            .actions
            .iter()
            .map(|action| {
                if short {
                    short_action(action.name)
                } else {
                    action.name
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
            .into()
    };
    let (actions, full_actions) = (names(true), names(false));
    div()
        .id(("events-row", seq))
        .debug_selector(move || format!("events-row-{seq}"))
        .h(theme.metrics.row)
        .w_full()
        .px(theme.metrics.gutter)
        .flex()
        .items_center()
        .gap(COLUMN_GAP)
        .cursor_pointer()
        .when(record.inspector, |this| this.opacity(0.55))
        .when(selected, |this| this.bg(colors.selected))
        .when(!selected, |this| this.hover(|style| style.bg(colors.hover)))
        .child(
            div()
                .flex_none()
                .w(TIME_WIDTH)
                .flex()
                .justify_end()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(colors.text_faint)
                .child(clock(record.at)),
        )
        .child(
            div().flex_none().w(KIND_WIDTH).flex().items_center().child(
                Icon::new(kind_icon(record.kind))
                    .size(theme.metrics.icon_small)
                    .color(colors.text_muted),
            ),
        )
        .child(event_summary(record, theme))
        .child(
            actions_cell()
                .id(("events-row-actions", seq))
                .truncate()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(match action {
                    Some(action) if !action.handled => colors.warn,
                    _ => colors.text,
                })
                .when(!actions.is_empty(), |this| {
                    this.tooltip(Tooltip::text(full_actions))
                })
                .child(actions),
        )
        .child(
            div()
                .flex_none()
                .w(TOOK_WIDTH)
                .flex()
                .justify_end()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(took_color)
                .child(format::duration(record.duration)),
        )
        .child(
            div()
                .flex_none()
                .w(REDRAW_WIDTH)
                .flex()
                .justify_center()
                .when(record.caused_redraw, |this| {
                    this.child(div().size(px(6.)).rounded_full().bg(colors.accent))
                }),
        )
        .child(
            div()
                .flex_none()
                .w(HANDLED_WIDTH)
                .flex()
                .justify_center()
                .when(record.handled, |this| {
                    this.child(
                        Icon::new(IconName::Check)
                            .size(theme.metrics.icon_small)
                            .color(colors.ok),
                    )
                }),
        )
}

/// An action name without its namespace: `inbox::OpenIssue` → `OpenIssue`.
fn short_action(name: &'static str) -> &'static str {
    name.rsplit("::").next().unwrap_or(name)
}

/// The event cell: key caps for keys, the pointer detail otherwise, with a
/// `×N` pill for coalesced moves.
fn event_summary(record: &InputRecord, theme: &'static Theme) -> impl IntoElement + use<> {
    let colors = &theme.colors;
    let cell = event_cell()
        .id(("events-row-event", record.seq))
        .tooltip(Tooltip::with_meta(
            record.detail.clone(),
            kind_label(record.kind),
        ));
    if let Some((keys, note)) = record_keys(record) {
        return cell.child(Kbd::new(keys)).children(note.map(|note| {
            div()
                .flex_none()
                .text_size(theme.metrics.text_small)
                .text_color(colors.text_faint)
                .child(note)
        }));
    }
    let faint = matches!(record.kind, InputKind::MouseMove | InputKind::MouseExit);
    cell.child(
        div()
            .min_w_0()
            .truncate()
            .text_color(if faint {
                colors.text_muted
            } else {
                colors.text
            })
            .child(record.detail.clone()),
    )
    .when(record.coalesced > 1, |this| {
        this.child(Pill::new(format!(
            "×{}",
            format::count(u64::from(record.coalesced))
        )))
    })
}
