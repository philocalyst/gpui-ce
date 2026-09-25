//! The selected entity: what it is, who pokes it and who watches it.
//!
//! One sentence first (see [`summary`]), then its counts, a 60 s notify
//! history with its peak, where it was last notified (copy, open), and two
//! actions: *Reveal in Elements* (views) and *Notify*, which re-renders what
//! depends on it and shows up as a Notify cause in the next frame.

use super::{EntitiesLens, entity_label, kind_chip};
use crate::{
    analysis::{
        entities::{EntityRow, format_rate, history, peak, source_path, summary, view_element},
        format,
    },
    state::Lens,
    theme::{MONO_FONT, Theme},
    widgets::{
        Button, ButtonSize, ButtonStyle, EmptyState, Icon, IconName, Prose, SectionHeader,
        Sparkline, Tooltip,
    },
};
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, EntityId, FontWeight, IntoElement,
    Pixels, SharedString, Window, div, prelude::*, px,
};
use std::{collections::VecDeque, panic::Location, time::Duration};

const HISTORY_HEIGHT: Pixels = px(56.);

impl EntitiesLens {
    pub(super) fn render_detail(
        &mut self,
        width: Pixels,
        theme: &'static Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.selected_row(cx).cloned() else {
            return EmptyState::new(Lens::Entities.question())
                .icon(IconName::Entity)
                .description(
                    "Select an entity to see who watches it, how often it notifies and where \
                     from. The busiest entities are listed first.",
                )
                .into_any_element();
        };
        let colors = &theme.colors;
        let capture = window.inspector_capture();
        let buckets = capture
            .and_then(|capture| capture.notify_stats().get(&row.id))
            .map(|stats| stats.buckets.clone())
            .unwrap_or_default();
        let view_key = row
            .is_view
            .then(|| capture.and_then(|capture| view_element(capture.latest_tree()?, row.id)))
            .flatten();
        let history_width = (width - theme.metrics.gutter * 2.).max(px(80.));

        let title = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text)
                    .child(row.name.clone()),
            )
            .child(kind_chip(row.is_view, theme));
        let type_line = div()
            .id("entities-type-name")
            .min_w_0()
            .truncate()
            .font_family(MONO_FONT)
            .text_size(theme.metrics.mono)
            .text_color(colors.text_muted)
            .tooltip(Tooltip::text(row.type_name))
            .child(format!("{} {}", row.type_name, entity_label(row.id)));

        div()
            .id("entities-detail")
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
                    .child(title)
                    .child(type_line)
                    .child(div().pt_1().child(Prose::new(summary(&row)))),
            )
            .child(counts(&row, theme))
            .child(history_section(&row, &buckets, history_width, theme))
            .child(self.render_site(&row, theme, cx))
            .child(
                div()
                    .px(theme.metrics.gutter)
                    .pt(px(10.))
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(reveal_button(row.id, row.is_view, view_key, cx))
                    .child(
                        Button::new("entities-notify")
                            .icon(IconName::Flash)
                            .label("Notify")
                            .style(ButtonStyle::Subtle)
                            .tooltip(
                                "Notify it now: what depends on it re-renders in the next frame",
                            )
                            .on_click(notify_handler(row.id)),
                    ),
            )
            .into_any_element()
    }

    fn render_site(
        &self,
        row: &EntityRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = &theme.colors;
        let header = SectionHeader::new("Last notify").rule();
        let Some(site) = row.last_site else {
            return div().flex().flex_col().child(header).child(
                div()
                    .px(theme.metrics.gutter)
                    .text_color(colors.text_muted)
                    .child("Not notified since recording started"),
            );
        };
        let id = row.id;
        let copied = self.copied == Some(id);
        let file = std::env::current_dir()
            .ok()
            .and_then(|cwd| source_path(site.file(), &cwd));
        div().flex().flex_col().child(header).child(
            div()
                .px(theme.metrics.gutter)
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("entities-site")
                        .debug_selector(|| "entities-site".into())
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .rounded(theme.metrics.radius)
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.hover))
                        .tooltip(Tooltip::with_meta(
                            format::location_with_path(site),
                            "Click to copy",
                        ))
                        .child(
                            Icon::new(IconName::Source)
                                .size(theme.metrics.icon_small)
                                .color(colors.text_muted),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(MONO_FONT)
                                .text_size(theme.metrics.mono)
                                .text_color(colors.accent)
                                .child(format::location(site)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(site_text(site)));
                            this.copied = Some(id);
                            cx.notify();
                        })),
                )
                .when(copied, |this| {
                    this.child(
                        div()
                            .flex_none()
                            .text_size(theme.metrics.text_small)
                            .text_color(colors.ok)
                            .child("Copied"),
                    )
                })
                .child(div().flex_1())
                .child(
                    Button::new("entities-open-site")
                        .label("Open")
                        .size(ButtonSize::Small)
                        .disabled(file.is_none())
                        .tooltip(if file.is_some() {
                            "Open the file with the system's default app"
                        } else {
                            "The file isn't under the working directory"
                        })
                        .when_some(file, |this, file| {
                            this.on_click(move |_, _, cx| cx.open_with_system(&file))
                        }),
                ),
        )
    }
}

/// The 60 s notify history: the rate now and the peak in the header, then
/// the sparkline between "60 s ago" and "now".
fn history_section(
    row: &EntityRow,
    buckets: &VecDeque<u32>,
    width: Pixels,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let colors = &theme.colors;
    let peak = peak(buckets);
    let mut summary = format!("{}/s now", format_rate(row.rate));
    if let Some(peak) = peak {
        let when = if peak.ago < Duration::from_secs(1) {
            "now".to_string()
        } else {
            format!("{} s ago", peak.ago.as_secs())
        };
        summary.push_str(&format!(" · peak {}/s {when}", format_rate(peak.rate)));
    }
    let chart = div()
        .h(HISTORY_HEIGHT)
        .rounded(theme.metrics.radius)
        .bg(colors.surface)
        .border_1()
        .border_color(colors.line)
        .overflow_hidden()
        .flex()
        .justify_center()
        .items_end();
    let chart = match peak {
        Some(peak) => chart.child(
            Sparkline::new(history(buckets))
                .max(peak.rate)
                .size(width - px(2.), HISTORY_HEIGHT - px(6.))
                .color(colors.accent),
        ),
        None => chart.items_center().child(
            div()
                .text_size(theme.metrics.text_small)
                .text_color(colors.text_faint)
                .child("No notifies in the last minute"),
        ),
    };
    div()
        .flex()
        .flex_col()
        .child(SectionHeader::new("Notifies").detail(summary).rule())
        .child(
            div()
                .px(theme.metrics.gutter)
                .flex()
                .flex_col()
                .gap_1()
                .child(chart)
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_size(theme.metrics.text_small)
                        .text_color(colors.text_faint)
                        .child("60 s ago")
                        .child("now"),
                ),
        )
}

/// `path/to/file.rs:88`, as copied to the clipboard.
fn site_text(site: &Location<'_>) -> String {
    format!("{}:{}", site.file(), site.line())
}

/// Four small tiles: refs, observers, subscribers and notifies.
fn counts(row: &EntityRow, theme: &Theme) -> impl IntoElement + use<> {
    let colors = &theme.colors;
    let tile = |value: String, label: &'static str| {
        div()
            .flex_1()
            .min_w(px(64.))
            .px_2()
            .py_1()
            .flex()
            .flex_col()
            .rounded(theme.metrics.radius)
            .bg(colors.surface)
            .border_1()
            .border_color(colors.line)
            .child(
                div()
                    .font_family(MONO_FONT)
                    .text_size(px(13.))
                    .text_color(colors.text)
                    .child(value),
            )
            .child(
                div()
                    .text_size(theme.metrics.text_small)
                    .text_color(colors.text_muted)
                    .child(label),
            )
    };
    div()
        .px(theme.metrics.gutter)
        .pb(px(8.))
        .flex()
        .flex_wrap()
        .gap_2()
        .child(tile(format::count(row.strong_count as u64), "strong refs"))
        .child(tile(format::count(row.observers as u64), "observers"))
        .child(tile(format::count(row.subscribers as u64), "subscribers"))
        .child(tile(format::count(row.notifies), "notifies"))
}

/// *Reveal in Elements*: selects the view's element and shows Elements.
fn reveal_button(
    id: EntityId,
    is_view: bool,
    key: Option<gpui::inspector::ElementKey>,
    cx: &mut Context<EntitiesLens>,
) -> impl IntoElement + use<> {
    let tooltip: SharedString = match (is_view, key) {
        (_, Some(_)) => "Select its element in the Elements lens".into(),
        (true, None) => "Not drawn in the latest captured tree".into(),
        (false, None) => format!("{} is not a view", entity_label(id)).into(),
    };
    Button::new("entities-reveal")
        .icon(IconName::Pick)
        .label("Reveal in Elements")
        .style(ButtonStyle::Subtle)
        .disabled(key.is_none())
        .tooltip(tooltip)
        .when_some(key, |this, key| {
            this.on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.select_element(Some(key), cx);
                    state.set_lens(Lens::Elements, cx);
                })
            }))
        })
}

/// Notifies entity `id`, as if its own code had called `cx.notify()`.
fn notify_handler(id: EntityId) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| cx.notify(id)
}
