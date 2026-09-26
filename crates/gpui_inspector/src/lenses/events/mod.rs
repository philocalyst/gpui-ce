//! Events: where did my click go? What will this key do here?
//!
//! ```text
//! [All|Keys|Mouse|Actions] [filter · -exclude          ]   toolbar
//! time    ◇ event                actions       took ● ✓   log, newest last
//! 184 of 212 events                [Show Loupe's input] [Follow]
//! ───────────────────────────────────────────────────────  splitter
//! [Event | Key tester]                                     pane switch
//! the selected event's story, hit path, contexts, actions and frame,
//! or the key tester
//! ```
//!
//! The log reads the capture's input ring; its filtering is memoized per
//! capture generation and filter (see [`log::EventLog`]). The key tester
//! resolves keys against the app's focus without dispatching anything.

mod detail;
mod key_tester;
mod log;

use super::{LensView, RailBadge};
use crate::{
    analysis::{
        events::{KindFilter, LogFilter},
        filter::TextFilter,
        format,
    },
    state::{LensLayout, LoupeState},
    theme::{Theme, UI_FONT},
    time_labels::TimeLabels,
    widgets::{IconName, Segment, Segmented, Split, TextField, text_field_state},
};
use gpui::{
    App, AppContext as _, Axis, Context, Entity, IntoElement, Pixels, Render, Subscription, Window,
    div,
    inspector::{ElementKey, InspectorCapture},
    prelude::*,
    px,
};
use gpui_elements::editable_text::{EditableTextState, TextChanged};
use key_tester::{KeyTester, KeyTesterEvent};
use log::EventLog;
use std::{cell::Cell, time::Duration};

/// What the lower (or right) pane shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pane {
    /// The selected event.
    #[default]
    Event,
    /// The key tester.
    KeyTester,
}

/// The Events lens.
pub(crate) struct EventsLens {
    state: Entity<LoupeState>,
    filter_field: Entity<EditableTextState>,
    filter: LogFilter,
    log: EventLog,
    pane: Pane,
    key_tester: Entity<KeyTester>,
    /// The log pane's size along the split, once the user dragged it.
    split: Option<Pixels>,
    /// The shared input selection this lens last showed, to tell when
    /// something else (a link from Frames, the palette) selected a record.
    shown_input: Option<u64>,
    badge: Cell<Option<(BadgeKey, usize)>>,
    /// The shown event's age, as the latest render read it.
    time_labels: TimeLabels,
    _subscriptions: Vec<Subscription>,
}

/// What the rail badge's count depends on.
type BadgeKey = (u64, usize, Option<u64>);

impl EventsLens {
    /// The lens over the shared `state`.
    pub fn new(state: Entity<LoupeState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter_field = text_field_state(cx);
        let key_tester = cx.new(|cx| KeyTester::new(window, cx));
        let subscriptions = vec![
            // Every generation the app moves, note where its focus is, so the
            // key tester resolves against it after Loupe takes the focus.
            cx.observe_in(&state, window, |this, _, window, cx| {
                this.key_tester
                    .update(cx, |tester, cx| tester.note_app_focus(window, cx));
                this.reveal_selection(window, cx);
                cx.notify();
            }),
            // This view is cached: re-render when the field's caret, selection
            // or focus changes, not only its text.
            cx.observe(&filter_field, |_, _, cx| cx.notify()),
            cx.subscribe(&filter_field, |this, field, _: &TextChanged, cx| {
                let text = field.read(cx).as_str().to_string();
                this.filter.text = TextFilter::parse(&text);
                this.state.update(cx, |state, cx| {
                    let mut filters = state.filters().clone();
                    filters.events = text.into();
                    state.set_filters(filters, cx);
                });
                cx.notify();
            }),
            cx.subscribe_in(
                &key_tester,
                window,
                |this, _, event, window, cx| match event {
                    KeyTesterEvent::Left => this.log.focus(window, cx),
                },
            ),
        ];
        let shown_input = state.read(cx).selected_input();
        Self {
            state,
            filter_field,
            filter: LogFilter::default(),
            log: EventLog::new(cx),
            pane: Pane::default(),
            key_tester,
            split: None,
            shown_input,
            badge: Cell::new(None),
            time_labels: TimeLabels::default(),
            _subscriptions: subscriptions,
        }
    }

    fn set_kind(&mut self, kind: KindFilter, cx: &mut Context<Self>) {
        if self.filter.kind != kind {
            self.filter.kind = kind;
            cx.notify();
        }
    }

    fn toggle_loupe_input(&mut self, cx: &mut Context<Self>) {
        self.filter.show_loupe = !self.filter.show_loupe;
        cx.notify();
    }

    /// Shows a record that something else selected (a link from Frames):
    /// the Event pane, following paused, and the log focused and scrolled
    /// to the record, with the filter widened if it hid it.
    fn reveal_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.state.read(cx).selected_input();
        if selected == self.shown_input {
            return;
        }
        self.shown_input = selected;
        let Some(seq) = selected else {
            return;
        };
        self.log.focus(window, cx);
        self.pane = Pane::Event;
        let row_height = Theme::of(window, cx).metrics.row;
        let Some(capture) = window.inspector_capture() else {
            self.log.pause(false);
            return;
        };
        self.log.refresh(capture, &self.filter, row_height);
        self.log.pause(false);
        if let Some(widened) = self.log.widened_filter(capture, &self.filter, seq) {
            let clears_text = widened.text != self.filter.text;
            self.filter = widened;
            self.log.refresh(capture, &self.filter, row_height);
            if clears_text {
                self.filter_field
                    .update(cx, |field, cx| field.emplace("", cx));
            }
        }
        self.log.scroll_to(seq);
    }

    fn show_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        if self.pane != pane {
            self.pane = pane;
            cx.notify();
        }
    }

    /// Hovers `key` in the app while the pointer is over its hit-path row
    /// (`hovered`), and stops when it leaves the row.
    fn hover_element(
        &mut self,
        key: ElementKey,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity_id();
        let Some(capture) = window.inspector_capture_mut() else {
            return;
        };
        let overlay = capture.overlay_mut();
        let changed = if hovered {
            overlay.set_hovered(owner, Some(key))
        } else {
            // The row the pointer entered may have taken the hover over.
            overlay.hovered() == Some(key) && overlay.set_hovered(owner, None)
        };
        if changed {
            // The overlay is painted with the next frame.
            cx.notify();
        }
    }

    /// The log pane's size: what the user dragged it to, or 45% of the lens
    /// stacked (60% side by side), measured from the dock.
    fn split_size(&self, layout: LensLayout, theme: &Theme, window: &Window) -> Pixels {
        if let Some(split) = self.split {
            return split;
        }
        let dock = window.inspector_bounds().map(|bounds| bounds.size);
        let metrics = &theme.metrics;
        let chrome = metrics.toolbar + metrics.pulse + metrics.rail + metrics.status;
        match (layout, dock) {
            (LensLayout::Stacked, Some(size)) => ((size.height - chrome) * 0.45).max(px(160.)),
            (LensLayout::SideBySide, Some(size)) => (size.width * 0.6).max(px(280.)),
            (_, None) => px(320.),
        }
    }

    fn render_toolbar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let selected = KindFilter::ALL
            .iter()
            .position(|kind| *kind == self.filter.kind)
            .unwrap_or_default();
        let kinds = KindFilter::ALL
            .into_iter()
            .fold(Segmented::new("events-kind"), |segmented, kind| {
                segmented.segment(Segment::label(kind.label()))
            })
            .selected(selected)
            .on_select(
                cx.listener(|this, ix: &usize, _, cx| this.set_kind(KindFilter::ALL[*ix], cx)),
            );
        div()
            .flex_none()
            .min_h(theme.metrics.toolbar)
            .px(theme.metrics.gutter)
            .py(px(4.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .bg(theme.colors.surface)
            .border_b_1()
            .border_color(theme.colors.line)
            .child(kinds)
            .child(
                div().flex_1().min_w(px(120.)).child(
                    TextField::new("events-filter", &self.filter_field)
                        .icon(IconName::Filter)
                        .placeholder("Filter events · -exclude"),
                ),
            )
    }
}

impl Render for EventsLens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let layout = LensLayout::of(window);
        let capture = window.inspector_capture();
        self.time_labels
            .begin(capture.map_or(Duration::ZERO, |capture| capture.now()));
        if let Some(capture) = capture {
            self.log.refresh(capture, &self.filter, theme.metrics.row);
        }
        let split_size = self.split_size(layout, theme, window);
        let log_width = match layout {
            LensLayout::SideBySide => split_size,
            LensLayout::Stacked => window
                .inspector_bounds()
                .map_or(split_size, |bounds| bounds.size.width),
        };
        self.log.set_width(log_width);
        let log = self.render_log(theme, window, cx);
        let pane = self.render_pane(theme, window, cx);
        let axis = match layout {
            LensLayout::Stacked => Axis::Vertical,
            LensLayout::SideBySide => Axis::Horizontal,
        };
        let split = Split::new("events-split", axis, split_size)
            .min_sizes(px(96.), px(120.))
            .first(log)
            .second(pane)
            .on_resize(cx.listener(|this, size: &Pixels, _, cx| {
                this.split = Some(*size);
                cx.notify();
            }));
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.colors.bg)
            .font_family(UI_FONT)
            .text_size(theme.metrics.text)
            .line_height(theme.metrics.line_height)
            .text_color(theme.colors.text)
            .child(self.render_toolbar(theme, cx))
            .child(div().flex_1().min_h_0().child(split))
    }
}

/// App input records in the ring: the rail's count.
fn app_input_count(capture: &InspectorCapture) -> usize {
    capture
        .input()
        .iter()
        .filter(|record| !record.inspector)
        .count()
}

impl LensView for EventsLens {
    fn rail_badge(&self, window: &Window, _cx: &App) -> Option<RailBadge> {
        let capture = window.inspector_capture()?;
        let input = capture.input();
        let key = (
            capture.generation(),
            input.len(),
            input.back().map(|record| record.seq),
        );
        let count = match self.badge.get() {
            Some((cached, count)) if cached == key => count,
            _ => {
                let count = app_input_count(capture);
                self.badge.set(Some((key, count)));
                count
            }
        };
        (count > 0).then(|| RailBadge::count(format::count(count as u64)))
    }

    fn time_labels(&self) -> Option<&TimeLabels> {
        Some(&self.time_labels)
    }
}
