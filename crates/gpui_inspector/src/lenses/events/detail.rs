//! The lower pane: the selected event's story, or the key tester.
//!
//! An event reads top to bottom: one sentence of what happened, the
//! elements under the pointer (topmost first; hover to highlight one in the
//! app, click to select it), the key contexts it was dispatched in, the
//! actions it ran, and the frame it caused.

use super::{EventsLens, Pane, key_tester::context_chips, log::kind_icon};
use crate::{
    analysis::{
        events::{FrameLink, context_label, kind_label, record_at, record_sentence},
        format,
    },
    state::Lens,
    theme::{MONO_FONT, Theme},
    widgets::{
        Button, ButtonStyle, EmptyState, Icon, IconName, Kbd, Pill, Prose, SectionHeader, Segment,
        Segmented, Tone, Tooltip,
    },
};
use gpui::{
    AnyElement, Context, Hsla, IntoElement, SharedString, Window, div,
    inspector::{ActionRecord, ElementKey, ElementKind, InputRecord, InspectorCapture},
    prelude::*,
    px,
};

/// One element of a hit path, as the detail lists it.
struct HitRow {
    key: ElementKey,
    name: String,
    glyph: IconName,
    color: Hsla,
    size: Option<String>,
    source: Option<(String, String)>,
}

impl EventsLens {
    pub(super) fn render_pane(
        &mut self,
        theme: &'static Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = &theme.colors;
        let switch = Segmented::new("events-pane")
            .segment(Segment::label("Event"))
            .segment(Segment::label("Key tester"))
            .selected(match self.pane {
                Pane::Event => 0,
                Pane::KeyTester => 1,
            })
            .on_select(cx.listener(|this, ix: &usize, _, cx| {
                this.show_pane(
                    if *ix == 0 {
                        Pane::Event
                    } else {
                        Pane::KeyTester
                    },
                    cx,
                )
            }));
        let body = match self.pane {
            Pane::Event => self.render_event(theme, window, cx),
            Pane::KeyTester => self.key_tester.clone().into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(theme.metrics.toolbar)
                    .px(theme.metrics.gutter)
                    .flex()
                    .items_center()
                    .bg(colors.surface)
                    .border_b_1()
                    .border_color(colors.line)
                    .child(switch),
            )
            .child(div().flex_1().min_h_0().child(body))
    }

    fn render_event(
        &mut self,
        theme: &'static Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.state.read(cx).selected_input();
        let Some(capture) = window.inspector_capture() else {
            return div().into_any_element();
        };
        let Some(record) = selected.and_then(|seq| record_at(capture.input(), seq)) else {
            return self.render_no_event(cx).into_any_element();
        };
        let selected_element = self.state.read(cx).selected_element();
        let hits: Vec<HitRow> = record
            .hit_path
            .iter()
            .map(|&key| self.hit_row(capture, key, theme))
            .collect();
        let target = hits.first().map(|hit| hit.name.as_str());
        let sentence = record_sentence(record, target);
        let meta = event_meta(record, theme);
        let frame = self.render_frame_link(record, capture, theme, cx);
        let contexts: Vec<String> = record.context_stack.iter().map(context_label).collect();
        let actions: Vec<ActionRecord> = record.actions.iter().cloned().collect();

        div()
            .id("events-detail")
            .size_full()
            .overflow_y_scroll()
            .pb_2()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(theme.metrics.gutter)
                    .pt(px(10.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(Prose::new(sentence))
                    .child(meta),
            )
            .when(!hits.is_empty(), |this| {
                this.child(
                    SectionHeader::new("Hit path")
                        .detail(format!("{} · topmost first", hits.len()))
                        .rule(),
                )
                .children(
                    hits.into_iter()
                        .enumerate()
                        .map(|(ix, hit)| self.render_hit(ix, hit, selected_element, theme, cx)),
                )
            })
            .when(!contexts.is_empty(), |this| {
                this.child(
                    SectionHeader::new("Key context")
                        .detail("outermost first")
                        .rule(),
                )
                .child(
                    context_chips(contexts, theme)
                        .px(theme.metrics.gutter)
                        .pb(px(6.)),
                )
            })
            .when(!actions.is_empty(), |this| {
                this.child(
                    SectionHeader::new("Actions")
                        .detail(actions.len().to_string())
                        .rule(),
                )
                .children(actions.iter().map(|action| action_row(action, theme)))
            })
            .child(SectionHeader::new("Frame").rule())
            .child(frame)
            .into_any_element()
    }

    fn render_no_event(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let newest = self.log_newest();
        let description = if newest.is_some() {
            "Select an event to see where it went: the elements under the pointer, the key \
             contexts, the actions it ran and the frame it caused."
        } else {
            "Use the app, then select an event to see where it went. To see what a key would \
             do right now, open the Key tester."
        };
        EmptyState::new(Lens::Events.question())
            .icon(IconName::Mouse)
            .description(description)
            .when_some(newest, |this, seq| {
                this.action(
                    Button::new("events-select-newest")
                        .label("Show the newest event")
                        .style(ButtonStyle::Subtle)
                        .on_click(cx.listener(move |this, _, _, cx| this.select_input(seq, cx))),
                )
            })
    }

    fn hit_row(&self, capture: &InspectorCapture, key: ElementKey, theme: &Theme) -> HitRow {
        let colors = &theme.colors;
        let element = self.log.element(capture, key);
        let (glyph, color) = match element.map(|record| record.kind) {
            Some(ElementKind::View { .. }) => (IconName::View, colors.view),
            Some(ElementKind::Component { .. }) => (IconName::Component, colors.component),
            _ => (IconName::ElementDot, colors.text_faint),
        };
        HitRow {
            key,
            name: self.log.element_name(capture, key),
            glyph,
            color,
            size: element.map(|record| format::size(record.bounds.size)),
            source: capture.path_info(key.path).map(|info| {
                (
                    format::location(info.source),
                    format::location_with_path(info.source),
                )
            }),
        }
    }

    fn render_hit(
        &self,
        ix: usize,
        hit: HitRow,
        selected: Option<ElementKey>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = &theme.colors;
        let key = hit.key;
        let tooltip = match &hit.source {
            Some((_, path)) => Tooltip::with_meta(hit.name.clone(), path.clone()),
            None => Tooltip::with_meta(
                hit.name.clone(),
                "Click to select, hover to highlight".to_string(),
            ),
        };
        div()
            .id(("events-hit", ix))
            .debug_selector(move || format!("events-hit-{ix}"))
            .h(theme.metrics.row)
            .px(theme.metrics.gutter)
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when(selected == Some(key), |this| this.bg(colors.selected))
            .when(selected != Some(key), |this| {
                this.hover(|style| style.bg(colors.hover))
            })
            .tooltip(tooltip)
            .child(
                Icon::new(hit.glyph)
                    .size(theme.metrics.icon_small)
                    .color(hit.color),
            )
            // The name keeps at least 96 px; the source gives way first.
            .child(
                div()
                    .flex_1()
                    .min_w(px(96.))
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(theme.metrics.mono)
                    .text_color(colors.text)
                    .when(ix == 0, |this| this.font_weight(gpui::FontWeight::SEMIBOLD))
                    .child(hit.name),
            )
            .children(hit.size.map(|size| {
                div()
                    .flex_none()
                    .font_family(MONO_FONT)
                    .text_size(theme.metrics.mono)
                    .text_color(colors.text_faint)
                    .child(size)
            }))
            .children(hit.source.map(|(short, _)| {
                div()
                    .flex_shrink(1.)
                    .min_w_0()
                    .max_w(px(160.))
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(theme.metrics.mono)
                    .text_color(colors.text_muted)
                    .child(short)
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                this.hover_element(hovered.then_some(key), window, cx)
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state
                    .update(cx, |state, cx| state.select_element(Some(key), cx))
            }))
    }

    fn render_frame_link(
        &self,
        record: &InputRecord,
        capture: &InspectorCapture,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = &theme.colors;
        let row = div()
            .min_h(theme.metrics.row)
            .px(theme.metrics.gutter)
            .flex()
            .items_center()
            .gap_2();
        let (frame, text) = match FrameLink::of(record) {
            FrameLink::Caused(frame) => (Some(frame), "Caused frame"),
            FrameLink::Followed(frame) => (Some(frame), "Didn't redraw; next frame"),
            FrameLink::Pending => (None, "Asked for a redraw; the frame isn't drawn yet"),
            FrameLink::None => (None, "Didn't redraw anything"),
        };
        let Some(frame) = frame else {
            return row
                .text_color(colors.text_muted)
                .child(text)
                .into_any_element();
        };
        let took = capture
            .frame(frame)
            .map(|record| format!(" · {}", format::duration(record.timings.app_total())))
            .unwrap_or_default();
        row.id("events-frame")
            .debug_selector(|| "events-frame".into())
            .cursor_pointer()
            .hover(|style| style.bg(colors.hover))
            .tooltip(Tooltip::with_meta(
                format!("{text} #{frame}{took}"),
                "Click to show it in Frames",
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(colors.text_muted)
                    .child(text),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(MONO_FONT)
                    .text_size(theme.metrics.mono)
                    .text_color(colors.accent)
                    .child(format!("#{frame}{took}")),
            )
            .child(
                Icon::new(IconName::ChevronRight)
                    .size(theme.metrics.icon_small)
                    .color(colors.text_muted),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.select_frame(Some(frame), cx);
                    state.set_lens(Lens::Frames, cx);
                })
            }))
            .into_any_element()
    }

    /// The newest listed record, if any.
    fn log_newest(&self) -> Option<u64> {
        self.log.newest()
    }
}

/// `⌨ key down · 12.480 s · #184` with pills for Loupe's own and coalesced input.
fn event_meta(record: &InputRecord, theme: &Theme) -> impl IntoElement + use<> {
    let colors = &theme.colors;
    let when = SharedString::from(format!(
        "{} · {} s · input #{}",
        kind_label(record.kind),
        crate::analysis::events::clock(record.at),
        record.seq
    ));
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(theme.metrics.text_small)
        .text_color(colors.text_muted)
        .child(
            Icon::new(kind_icon(record.kind))
                .size(theme.metrics.icon_small)
                .color(colors.text_muted),
        )
        .child(when)
        .when(record.coalesced > 1, |this| {
            this.child(Pill::new(format!(
                "{} merged",
                format::count(u64::from(record.coalesced))
            )))
        })
        .when(record.inspector, |this| {
            this.child(Pill::new("Loupe's own").tone(Tone::Accent))
        })
}

/// An action the event dispatched: handled or not, its name, and the
/// binding that produced it.
fn action_row(action: &ActionRecord, theme: &Theme) -> impl IntoElement + use<> {
    let colors = &theme.colors;
    let (icon, color, verdict) = if action.handled {
        (IconName::Check, colors.ok, "handled")
    } else {
        (
            IconName::Close,
            colors.warn,
            "not handled: nothing on the path listens for it",
        )
    };
    let binding = match (&action.keystrokes, &action.context) {
        (Some(keys), Some(context)) => format!("bound to {keys} in {context}"),
        (Some(keys), None) => format!("bound to {keys}"),
        _ => "dispatched directly".to_string(),
    };
    div()
        .id(SharedString::from(format!("events-action-{}", action.name)))
        .min_h(theme.metrics.row)
        .px(theme.metrics.gutter)
        .flex()
        .items_center()
        .gap_2()
        .tooltip(Tooltip::with_meta(
            action.name,
            format!("{verdict} · {binding}"),
        ))
        .child(Icon::new(icon).size(theme.metrics.icon_small).color(color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(colors.text)
                .child(action.name),
        )
        .children(action.keystrokes.clone().map(Kbd::new))
        .children(action.context.clone().map(|context| {
            div()
                .flex_shrink(1.)
                .min_w_0()
                .max_w(px(180.))
                .truncate()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(colors.text_muted)
                .child(context)
        }))
}
