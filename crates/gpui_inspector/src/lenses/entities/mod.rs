//! Entities: what's alive, who's watching it, who keeps poking it?
//!
//! ```text
//! [All|Views|Models] [filter · -exclude                       ]   toolbar
//! ID  Type        Kind  Refs Obs Subs  /s   60 s   Last notify   table, busiest first
//! 312 entities · 6 views · 40 notifying          [Show Loupe's entities]
//! ─────────────────────────────────────────────────────────────  splitter
//! IssueStore  model · inbox::IssueStore #6                        detail
//! "IssueStore is notifying 9×/s, most recently from store.rs:88…"
//! refs · observers · subscribers · notifies, 60 s sparkline, last
//! notify site, Reveal in Elements, Notify
//! ```
//!
//! Rows are rebuilt once per capture generation from
//! `Window::inspector_entities` and the capture's notify statistics; the
//! table re-filters and re-sorts only when rows, filter or columns change.

mod detail;

use super::{LensView, RailBadge};
pub(crate) use crate::analysis::entities::entity_label;
use crate::{
    analysis::{
        entities::{
            EntityColumn, EntityFilter, EntityRow, KindFilter, columns_for_width, compare,
            entity_rows, format_rate, history, is_loupe_type,
        },
        filter::TextFilter,
        format,
        insights::NOTIFY_STORM_PER_SECOND,
    },
    state::{LensLayout, LoupeState},
    theme::{MONO_FONT, Theme, UI_FONT},
    widgets::{
        Button, ButtonSize, Column, ColumnWidth, EmptyState, IconName, Segment, Segmented,
        SortDirection, Sparkline, Split, Table, TableEvent, TableState, TextField, Tooltip,
        text_field_state,
    },
};
use gpui::{
    AnyElement, App, AppContext as _, Axis, ColorExt as _, Context, Entity, EntityId, IntoElement,
    Pixels, Render, SharedString, Subscription, Window, div,
    inspector::{CauseKind, EntityInfo, InspectorCapture, short_type_name},
    prelude::*,
    px,
};
use gpui_elements::editable_text::{EditableTextState, TextChanged};
use std::{cell::Cell, collections::HashMap, rc::Rc};

/// Short type names for every entity Loupe has seen: live entities from the
/// registry, views from recorded view spans, and notified entities from
/// recorded causes.
pub(crate) fn entity_names(
    capture: &InspectorCapture,
    live: &[EntityInfo],
) -> HashMap<EntityId, &'static str> {
    let mut names = HashMap::new();
    for frame in capture.frames() {
        for view in &frame.views {
            names.insert(view.entity, short_type_name(view.type_name));
        }
        for cause in &frame.causes {
            if let CauseKind::Notify {
                entity,
                type_name: Some(type_name),
            } = cause.kind
            {
                names.insert(entity, short_type_name(type_name));
            }
        }
    }
    for entity in live {
        names.insert(entity.id, short_type_name(entity.type_name));
    }
    names
}

/// What the table's rows were last computed from.
#[derive(Clone, Debug, PartialEq)]
struct ListedKey {
    generation: u64,
    filter: EntityFilter,
    columns: Vec<EntityColumn>,
}

/// The Entities lens.
pub(crate) struct EntitiesLens {
    state: Entity<LoupeState>,
    filter_field: Entity<EditableTextState>,
    filter: EntityFilter,
    table: Entity<TableState<EntityId>>,
    /// Every live entity, as of `rows_generation`.
    rows: Rc<[EntityRow]>,
    rows_generation: Option<u64>,
    listed_key: Option<ListedKey>,
    /// Indices into `rows` of the listed entities, in data order.
    listed: Rc<[usize]>,
    columns: Rc<[EntityColumn]>,
    sort: (EntityColumn, SortDirection),
    /// The table pane's size along the split, once the user dragged it.
    split: Option<Pixels>,
    /// The entity whose last notify site was just copied.
    copied: Option<EntityId>,
    badge: Cell<Option<(u64, usize)>>,
    _subscriptions: Vec<Subscription>,
}

impl EntitiesLens {
    /// The lens over the shared `state`.
    pub fn new(state: Entity<LoupeState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter_field = text_field_state(cx);
        let table = cx.new(TableState::new);
        let subscriptions = vec![
            // This view is cached: re-render when what it draws changes (the
            // shared state, the field's caret and focus, the table's sort,
            // selection and scroll).
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.observe(&filter_field, |_, _, cx| cx.notify()),
            cx.observe(&table, |_, _, cx| cx.notify()),
            cx.subscribe(&filter_field, |this, field, _: &TextChanged, cx| {
                let text = field.read(cx).as_str().to_string();
                this.filter.text = TextFilter::parse(&text);
                this.state.update(cx, |state, cx| {
                    let mut filters = state.filters().clone();
                    filters.entities = text.into();
                    state.set_filters(filters, cx);
                });
                cx.notify();
            }),
            cx.subscribe_in(&table, window, |this, _, event, _, cx| match event {
                TableEvent::Selected(id) | TableEvent::Activated(id) => {
                    let id = *id;
                    this.state
                        .update(cx, |state, cx| state.select_entity(Some(id), cx));
                }
            }),
        ];
        Self {
            state,
            filter_field,
            filter: EntityFilter::default(),
            table,
            rows: Rc::from([]),
            rows_generation: None,
            listed_key: None,
            listed: Rc::from([]),
            columns: Rc::from([]),
            sort: (EntityColumn::Rate, SortDirection::Descending),
            split: None,
            copied: None,
            badge: Cell::new(None),
            _subscriptions: subscriptions,
        }
    }

    /// The selected entity's row, if it is alive.
    fn selected_row(&self, cx: &App) -> Option<&EntityRow> {
        let id = self.state.read(cx).selected_entity()?;
        self.rows.iter().find(|row| row.id == id)
    }

    fn set_kind(&mut self, kind: KindFilter, cx: &mut Context<Self>) {
        if self.filter.kind != kind {
            self.filter.kind = kind;
            cx.notify();
        }
    }

    fn toggle_loupe_entities(&mut self, cx: &mut Context<Self>) {
        self.filter.show_loupe = !self.filter.show_loupe;
        cx.notify();
    }

    /// Rebuilds the rows when the capture moved on, then re-lists them when
    /// rows, filter or columns changed, keeping the sort on the same column.
    fn refresh(&mut self, columns: Vec<EntityColumn>, window: &Window, cx: &mut Context<Self>) {
        let Some(capture) = window.inspector_capture() else {
            return;
        };
        let generation = capture.generation();
        if self.rows_generation != Some(generation) {
            let live = window.inspector_entities(cx);
            let stats = capture.notify_stats();
            self.rows = entity_rows(&live, |id| stats.get(&id)).into();
            self.rows_generation = Some(generation);
        }

        // Follow header clicks: the table sorts by position, we keep the column.
        if let Some((ix, direction)) = self.table.read(cx).model().sort()
            && let Some(&column) = self.columns.get(ix)
        {
            self.sort = (column, direction);
        }
        let key = ListedKey {
            generation,
            filter: self.filter.clone(),
            columns: columns.clone(),
        };
        if self.listed_key.as_ref() != Some(&key) {
            if !columns.contains(&self.sort.0) {
                self.sort = (EntityColumn::Rate, SortDirection::Descending);
            }
            let sort_ix = columns.iter().position(|column| *column == self.sort.0);
            self.columns = columns.into();
            self.listed = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| self.filter.matches(row))
                .map(|(ix, _)| ix)
                .collect();
            let keys = self.listed.iter().map(|&ix| self.rows[ix].id).collect();
            let (rows, listed, columns) =
                (self.rows.clone(), self.listed.clone(), self.columns.clone());
            let sort = sort_ix.map(|ix| (ix, self.sort.1));
            self.table.update(cx, |table, cx| {
                table.set_rows(
                    keys,
                    move |column, a, b| {
                        compare(columns[column], &rows[listed[a]], &rows[listed[b]])
                    },
                    cx,
                );
                table.set_sort(sort, cx);
            });
            self.listed_key = Some(key);
        }

        // Keep the table's selection on the shared selection (the palette
        // selects entities too).
        let selected = self.state.read(cx).selected_entity();
        if let Some(id) = selected
            && self.table.read(cx).model().selected() != Some(&id)
        {
            self.table.update(cx, |table, cx| table.reveal(id, cx));
        }
    }

    /// The table pane's size: what the user dragged it to, or about half
    /// the lens (60% side by side), measured from the dock.
    fn split_size(&self, layout: LensLayout, theme: &Theme, window: &Window) -> Pixels {
        if let Some(split) = self.split {
            return split;
        }
        let dock = window.inspector_bounds().map(|bounds| bounds.size);
        let metrics = &theme.metrics;
        let chrome = metrics.toolbar + metrics.pulse + metrics.rail + metrics.status;
        match (layout, dock) {
            (LensLayout::Stacked, Some(size)) => ((size.height - chrome) * 0.55).max(px(160.)),
            (LensLayout::SideBySide, Some(size)) => (size.width * 0.6).max(px(300.)),
            (_, None) => px(320.),
        }
    }

    /// How wide the table is, to choose its columns.
    fn table_width(&self, layout: LensLayout, split: Pixels, window: &Window) -> Pixels {
        match layout {
            LensLayout::SideBySide => split,
            LensLayout::Stacked => window
                .inspector_bounds()
                .map_or(px(400.), |bounds| bounds.size.width),
        }
    }

    fn render_toolbar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let selected = KindFilter::ALL
            .iter()
            .position(|kind| *kind == self.filter.kind)
            .unwrap_or_default();
        let kinds = KindFilter::ALL
            .into_iter()
            .fold(Segmented::new("entities-kind"), |segmented, kind| {
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
                    TextField::new("entities-filter", &self.filter_field)
                        .icon(IconName::Filter)
                        .placeholder("Filter entities · -exclude"),
                ),
            )
    }

    fn render_table(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = &theme.colors;
        let body = if self.listed.is_empty() {
            let (title, description) = if self.rows.is_empty() {
                (
                    "No entities yet",
                    "Entities appear here as soon as the app creates them.",
                )
            } else {
                (
                    "No entities match",
                    "Try another kind, or remove terms from the filter.",
                )
            };
            EmptyState::new(title)
                .icon(IconName::Entity)
                .description(description)
                .into_any_element()
        } else {
            let (rows, listed, columns) =
                (self.rows.clone(), self.listed.clone(), self.columns.clone());
            Table::new(
                "entities-table",
                &self.table,
                self.columns.iter().copied().map(table_column).collect(),
                move |data_ix, column_ix, window, cx| {
                    let row = &rows[listed[data_ix]];
                    cell(row, columns[column_ix], window, cx)
                },
            )
            .into_any_element()
        };
        let views = self
            .listed
            .iter()
            .filter(|&&ix| self.rows[ix].is_view)
            .count();
        let notifying = self
            .listed
            .iter()
            .filter(|&&ix| self.rows[ix].recent > 0)
            .count();
        let hidden = if self.filter.show_loupe {
            0
        } else {
            self.rows.iter().filter(|row| row.loupe).count()
        };
        // Loupe's own types are recognized by their crate; the editor state
        // behind its text fields is a shared widget type and stays listed.
        let loupe_tooltip = match hidden {
            0 => "List Loupe's own views and state".to_string(),
            hidden => format!(
                "List Loupe's own views and state ({} entities)",
                format::count(hidden as u64)
            ),
        };
        let summary = format!(
            "{} entities · {} views · {} notifying",
            format::count(self.listed.len() as u64),
            format::count(views as u64),
            format::count(notifying as u64)
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().child(body))
            .child(
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
                            .id("entities-summary")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme.metrics.text_small)
                            .text_color(colors.text_muted)
                            .tooltip(Tooltip::text(summary.clone()))
                            .child(summary),
                    )
                    .child(
                        Button::new("entities-show-loupe")
                            .label("Show Loupe's entities")
                            .size(ButtonSize::Small)
                            .toggle_state(self.filter.show_loupe)
                            .tooltip(loupe_tooltip)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_loupe_entities(cx))),
                    ),
            )
    }
}

/// The table column for `column`.
fn table_column(column: EntityColumn) -> Column {
    let base = Column::new(column.title());
    let base = if column.is_numeric() {
        base.numeric()
    } else {
        base
    };
    base.width(match column {
        EntityColumn::Id => ColumnWidth::Fixed(px(44.)),
        EntityColumn::Type => ColumnWidth::Flex(1.4),
        EntityColumn::Kind => ColumnWidth::Fixed(px(54.)),
        EntityColumn::Refs | EntityColumn::Observers | EntityColumn::Subscribers => {
            ColumnWidth::Fixed(px(42.))
        }
        EntityColumn::Rate => ColumnWidth::Fixed(px(44.)),
        EntityColumn::History => ColumnWidth::Fixed(px(76.)),
        EntityColumn::Site => ColumnWidth::Flex(1.),
    })
}

/// A small `view` / `model` chip; views wear the view color.
pub(super) fn kind_chip(is_view: bool, theme: &Theme) -> impl IntoElement + use<> {
    let colors = &theme.colors;
    let (label, color, fill) = if is_view {
        (
            "view",
            colors.view,
            colors.view.opacity(if theme.is_dark { 0.16 } else { 0.12 }),
        )
    } else {
        ("model", colors.text_muted, colors.surface_2)
    };
    div()
        .flex_none()
        .h(px(16.))
        .px(px(5.))
        .flex()
        .items_center()
        .rounded(px(3.))
        .bg(fill)
        .font_family(UI_FONT)
        .text_size(theme.metrics.label)
        .line_height(px(14.))
        .text_color(color)
        .child(label)
}

fn mono(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .min_w_0()
        .truncate()
        .font_family(MONO_FONT)
        .text_size(theme.metrics.mono)
        .child(text.into())
}

/// One table cell.
fn cell(row: &EntityRow, column: EntityColumn, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = Theme::of(window, cx);
    let colors = &theme.colors;
    let faint_zero = |count: usize| {
        mono(format::count(count as u64), theme).text_color(if count == 0 {
            colors.text_faint
        } else {
            colors.text
        })
    };
    let label = entity_label(row.id);
    match column {
        EntityColumn::Id => mono(label, theme)
            .text_color(colors.text_muted)
            .into_any_element(),
        EntityColumn::Type => {
            let selector = format!("entities-row-{}", label.trim_start_matches('#'));
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .min_w_0()
                .truncate()
                .text_color(if row.loupe {
                    colors.text_muted
                } else {
                    colors.text
                })
                .tooltip(Tooltip::with_meta(row.type_name, label))
                .child(row.name.clone())
                .into_any_element()
        }
        EntityColumn::Kind => kind_chip(row.is_view, theme).into_any_element(),
        EntityColumn::Refs => faint_zero(row.strong_count).into_any_element(),
        EntityColumn::Observers => faint_zero(row.observers).into_any_element(),
        EntityColumn::Subscribers => faint_zero(row.subscribers).into_any_element(),
        EntityColumn::Rate => mono(format_rate(row.rate), theme)
            .text_color(if row.rate >= NOTIFY_STORM_PER_SECOND as f32 {
                colors.warn
            } else if row.rate > 0. {
                colors.text
            } else {
                colors.text_faint
            })
            .into_any_element(),
        EntityColumn::History => {
            let values = window
                .inspector_capture()
                .and_then(|capture| capture.notify_stats().get(&row.id))
                .filter(|_| row.recent > 0)
                .map(|stats| history(&stats.buckets));
            match values {
                Some(values) => Sparkline::new(values)
                    .size(px(64.), px(14.))
                    .color(colors.accent)
                    .into_any_element(),
                None => mono(format::NO_VALUE, theme)
                    .text_color(colors.text_faint)
                    .into_any_element(),
            }
        }
        EntityColumn::Site => match row.last_site {
            Some(site) => mono(format::location(site), theme)
                .id(SharedString::from(format!("entities-site-{label}")))
                .text_color(colors.text_muted)
                .tooltip(Tooltip::text(format::location_with_path(site)))
                .into_any_element(),
            None => mono(format::NO_VALUE, theme)
                .text_color(colors.text_faint)
                .into_any_element(),
        },
    }
}

impl Render for EntitiesLens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let layout = LensLayout::of(window);
        let split = self.split_size(layout, theme, window);
        let width = self.table_width(layout, split, window);
        self.refresh(columns_for_width(width.as_f32()), window, cx);
        let axis = match layout {
            LensLayout::Stacked => Axis::Vertical,
            LensLayout::SideBySide => Axis::Horizontal,
        };
        let detail_width = match layout {
            LensLayout::Stacked => width,
            LensLayout::SideBySide => window
                .inspector_bounds()
                .map_or(px(240.), |bounds| bounds.size.width - split),
        };
        let table = self.render_table(theme, cx);
        let detail = self.render_detail(detail_width, theme, window, cx);
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
            .child(
                div().flex_1().min_h_0().child(
                    Split::new("entities-split", axis, split)
                        .min_sizes(px(120.), px(140.))
                        .first(table)
                        .second(detail)
                        .on_resize(cx.listener(|this, size: &Pixels, _, cx| {
                            this.split = Some(*size);
                            cx.notify();
                        })),
                ),
            )
    }
}

impl LensView for EntitiesLens {
    fn rail_badge(&self, window: &Window, cx: &App) -> Option<RailBadge> {
        // Live app entities (Loupe's own are not counted), once per generation.
        let generation = window.inspector_capture()?.generation();
        let count = match self.badge.get() {
            Some((cached, count)) if cached == generation => count,
            _ => {
                let count = if self.rows_generation == Some(generation) {
                    self.rows.iter().filter(|row| !row.loupe).count()
                } else {
                    window
                        .inspector_entities(cx)
                        .iter()
                        .filter(|entity| !is_loupe_type(entity.type_name))
                        .count()
                };
                self.badge.set(Some((generation, count)));
                count
            }
        };
        (count > 0).then(|| RailBadge::count(format::count(count as u64)))
    }
}
