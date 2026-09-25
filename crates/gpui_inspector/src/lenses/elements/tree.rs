//! The tree pane: filter, "your code", time-travel banner, the virtual
//! element tree and a sticky breadcrumb of the ancestors scrolled away.

use super::{
    ElementsLens, PastFrame,
    rows::{MatchField, NodeKey, RowInfo, RowKind, RowMatch, RowSet, TreeIndex, ViewStats},
};
use crate::{
    analysis::format,
    theme::{MONO_FONT, Theme, UI_FONT},
    widgets::{
        Button, ButtonSize, EmptyState, Icon, IconName, TextField, Tooltip, Tree, TreeModel,
        TreeRow, TreeState,
    },
};
use gpui::{
    AnyElement, App, Context, Entity, EntityId, FontWeight, HighlightStyle, IntoElement,
    StyledText, WeakEntity, Window, div,
    inspector::{ElementFlags, ElementRecord},
    prelude::*,
    px,
};
use gpui_elements::editable_text::actions::Escape;
use std::{collections::HashMap, ops::Range, rc::Rc};

/// Ancestors the sticky breadcrumb names before eliding older ones.
const MAX_CRUMBS: usize = 3;

impl ElementsLens {
    /// The tree pane.
    pub(super) fn render_tree_pane(
        &self,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        let rows = self.active_rows().filter(|_| self.shown.is_some());
        let index = self.memo.index.clone().filter(|_| self.shown.is_some());
        let state = self.active_tree().clone();
        let body = match (&index, &rows) {
            (Some(index), Some(rows)) if !rows.nodes.is_empty() => render_rows(
                index.clone(),
                rows.clone(),
                self.stats(),
                &state,
                cx.entity().downgrade(),
            )
            .into_any_element(),
            (Some(_), Some(_)) if self.filtering() => EmptyState::new("No elements match")
                .icon(IconName::Filter)
                .description(format!(
                    "No label, text, file or type contains “{}”.",
                    self.query.trim()
                ))
                .into_any_element(),
            (Some(_), Some(_)) => EmptyState::new("Nothing from your code")
                .icon(IconName::Filter)
                .description("Every element here was built by gpui or a library.")
                .into_any_element(),
            // Without a tree the whole lens shows one empty state instead.
            _ => div().into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .child(self.render_tree_header(rows.as_deref(), theme, cx))
            .children(
                self.shown
                    .as_ref()
                    .and_then(|shown| shown.past.clone())
                    .map(|past| self.render_time_travel(past, theme, cx)),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    fn render_tree_header(
        &self,
        rows: Option<&RowSet>,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = &theme.colors;
        let total = self
            .memo
            .index
            .as_ref()
            .map_or(0, |index| index.infos.len());
        let count = rows.filter(|_| self.filtering()).map(|rows| {
            div()
                .flex_none()
                .text_size(theme.metrics.text_small)
                .text_color(colors.text_faint)
                .child(format!(
                    "{} of {}",
                    format::count(rows.matches as u64),
                    format::count(total as u64)
                ))
        });
        let filter = TextField::new("elements-filter", &self.filter)
            .icon(IconName::Filter)
            .placeholder("Filter elements");
        let filter = match count {
            Some(count) => filter.trailing(count),
            None => filter,
        };
        div()
            .flex_none()
            .h(theme.metrics.toolbar)
            .px(px(6.))
            .flex()
            .items_center()
            .gap_1()
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.line)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                        if this.filtering() {
                            this.clear_filter(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(filter),
            )
            .child(
                Button::new("elements-your-code")
                    .label("Your code")
                    .size(ButtonSize::Small)
                    .toggle_state(self.your_code)
                    .tooltip("Hide elements built by gpui and other libraries")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_your_code(window, cx))),
            )
    }

    fn render_time_travel(
        &self,
        past: PastFrame,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = &theme.colors;
        div()
            .flex_none()
            .h(px(28.))
            .pl(theme.metrics.gutter)
            .pr(px(4.))
            .flex()
            .items_center()
            .gap(px(6.))
            .bg(colors.selected)
            .border_b_1()
            .border_color(colors.line)
            .child(
                Icon::new(IconName::Clock)
                    .size(theme.metrics.icon_small)
                    .color(colors.accent),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(colors.text)
                    .child(format!("Frame #{} · {}", past.frame, past.ago)),
            )
            .child(
                Button::new("elements-back-to-live")
                    .label("Back to live")
                    .size(ButtonSize::Small)
                    .tooltip("Show the latest frame's tree")
                    .on_click(cx.listener(|this, _, _, cx| this.back_to_live(cx))),
            )
    }
}

/// The ancestors of the visible row `row` (the first one under the pinned
/// bar): its parent, then the landmarks (views and components) above it,
/// like the status bar's breadcrumb. Clicking one selects it.
fn render_crumbs(
    index: &TreeIndex,
    rows: &RowSet,
    model: &TreeModel<NodeKey>,
    row: usize,
    lens: &WeakEntity<ElementsLens>,
    theme: &Theme,
) -> Option<AnyElement> {
    let colors = &theme.colors;
    let &node = model.rows().get(row)?;
    let mut chain = Vec::new();
    let mut parent = model.nodes().get(node)?.parent;
    while let Some(ix) = parent {
        chain.push(ix);
        parent = model.nodes()[ix].parent;
    }
    let nearest = *chain.first()?;
    let mut chain: Vec<usize> = chain
        .into_iter()
        .filter(|&node| {
            node == nearest
                || rows.rows.get(node).is_some_and(|row| {
                    matches!(
                        index.infos[row.ix as usize].kind,
                        RowKind::View | RowKind::Component
                    )
                })
        })
        .collect();
    chain.reverse();
    let elided = chain.len() > MAX_CRUMBS;
    let chain = chain.split_off(chain.len().saturating_sub(MAX_CRUMBS));
    let last = chain.len() - 1;
    let crumbs = chain.into_iter().enumerate().flat_map(|(position, node)| {
        let label = rows
            .rows
            .get(node)
            .map(|row| index.infos[row.ix as usize].label.clone())
            .unwrap_or_default();
        let key = model.nodes()[node].key;
        let lens = lens.clone();
        let crumb = div()
            .id(("elements-crumb", position))
            .min_w_0()
            .truncate()
            .when(position == last, |this| this.flex_none().max_w(px(180.)))
            .when(position < last, |this| this.flex_shrink_1())
            .cursor_pointer()
            .text_color(colors.text_muted)
            .hover(|style| style.text_color(colors.accent))
            .child(label)
            .on_click(move |_, window, cx| {
                lens.update(cx, |lens, cx| lens.select_node(key, window, cx))
                    .ok();
            })
            .into_any_element();
        let separator = (position < last).then(|| {
            div()
                .flex_none()
                .text_color(colors.text_faint)
                .child("›")
                .into_any_element()
        });
        std::iter::once(crumb).chain(separator)
    });
    Some(
        div()
            .size_full()
            .px(theme.metrics.gutter)
            .flex()
            .items_center()
            .gap(px(5.))
            .overflow_hidden()
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.line)
            .font_family(UI_FONT)
            .text_size(theme.metrics.text_small)
            .when(elided, |this| {
                this.child(div().flex_none().text_color(colors.text_faint).child("… ›"))
            })
            .children(crumbs)
            .into_any_element(),
    )
}

/// The virtual list of rows, with the ancestors of the rows scrolled past
/// pinned over its top.
fn render_rows(
    index: Rc<TreeIndex>,
    rows: Rc<RowSet>,
    stats: Rc<HashMap<EntityId, ViewStats>>,
    state: &Entity<TreeState<NodeKey>>,
    lens: WeakEntity<ElementsLens>,
) -> impl IntoElement {
    let (crumb_index, crumb_rows) = (index.clone(), rows.clone());
    Tree::new("elements-tree", state, move |row, window, cx| {
        render_row(&index, &rows, &stats, row, window, cx)
    })
    .pinned_header(move |model, row, window, cx| {
        let theme = Theme::of(window, cx);
        render_crumbs(&crumb_index, &crumb_rows, model, row, &lens, theme)
    })
}

/// One row's content (the tree draws indentation and the chevron).
fn render_row(
    index: &TreeIndex,
    rows: &RowSet,
    stats: &HashMap<EntityId, ViewStats>,
    row: &TreeRow<NodeKey>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = Theme::of(window, cx);
    let colors = &theme.colors;
    let Some(entry) = rows.rows.get(row.node) else {
        return div().into_any_element();
    };
    let (Some(info), Some(record)) = (index.info(entry.ix), index.tree.get(entry.ix)) else {
        return div().into_any_element();
    };
    let clipped = record.flags.contains(ElementFlags::CLIPPED);
    let (glyph, glyph_color) = match info.kind {
        RowKind::View => (IconName::View, colors.view),
        RowKind::Component => (IconName::Component, colors.component),
        RowKind::Text | RowKind::Element => (IconName::ElementDot, colors.text_faint),
    };
    let label_color = match info.kind {
        _ if clipped => colors.text_faint,
        RowKind::Text => colors.text_muted,
        _ => colors.text,
    };
    let label = StyledText::new(info.label.clone()).with_highlights(label_highlights(
        info,
        entry.matched.as_ref(),
        theme,
    ));
    let file_match = entry
        .matched
        .as_ref()
        .filter(|matched| matched.field == MatchField::File)
        .and_then(|matched| Some((info.file.clone()?, matched.range.clone())));
    let renders = info
        .entity
        .and_then(|entity| stats.get(&entity))
        .filter(|stats| stats.rendered > 0);

    div()
        .h_full()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(Icon::new(glyph).size(px(10.)).color(glyph_color))
        .child(
            div()
                .id(("label", row.node))
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(label_color)
                .child(label)
                .tooltip(Tooltip::with_meta(
                    info.text.clone().unwrap_or_else(|| info.label.clone()),
                    match &info.file {
                        Some(file) => format!("{} · {file}", info.type_name),
                        None => info.type_name.to_string(),
                    },
                )),
        )
        .children(file_match.map(|(file, range)| {
            div()
                .flex_none()
                .max_w(px(140.))
                .truncate()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(colors.text_faint)
                .child(
                    StyledText::new(file)
                        .with_highlights(range.map(|range| (range, match_style(theme)))),
                )
        }))
        .child(render_flags(record, theme))
        .children(renders.map(|stats| {
            div()
                .id(("renders", row.node))
                .flex_none()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(colors.text_muted)
                .child(format!("×{}", stats.rendered))
                .tooltip(Tooltip::with_meta(
                    format!("Rendered in {} recorded frames", stats.rendered),
                    format!("{} cache hits", stats.cached),
                ))
        }))
        .when(record.flags.contains(ElementFlags::OVERRIDDEN), |this| {
            this.child(
                div()
                    .id(("overridden", row.node))
                    .flex_none()
                    .child(
                        Icon::new(IconName::ElementDot)
                            .size(px(12.))
                            .color(colors.accent),
                    )
                    .tooltip(Tooltip::text("Loupe overrides its style")),
            )
        })
        .child(
            div()
                .flex_none()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(colors.text_faint)
                .child(info.size.clone()),
        )
        .into_any_element()
}

/// The small icons for what an element does with input.
fn render_flags(record: &ElementRecord, theme: &Theme) -> impl IntoElement {
    let flags = record.flags;
    let icons = [
        (ElementFlags::CLICKABLE, IconName::Cursor, "Clickable"),
        (ElementFlags::FOCUSABLE, IconName::Focus, "Focusable"),
        (ElementFlags::SCROLLABLE, IconName::Scroll, "Scrolls"),
    ]
    .into_iter()
    .filter(|(flag, ..)| flags.contains(*flag))
    .chain(
        // A hitbox is implied by clicking and scrolling; show it alone.
        (flags.contains(ElementFlags::HITBOX)
            && !flags.intersects(ElementFlags::CLICKABLE | ElementFlags::SCROLLABLE))
        .then_some((ElementFlags::HITBOX, IconName::Hitbox, "Hitbox")),
    );
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(3.))
        .children(icons.enumerate().map(|(ix, (_, icon, meaning))| {
            div()
                .id(("flag", ix))
                .child(Icon::new(icon).size(px(11.)).color(theme.colors.text_faint))
                .tooltip(Tooltip::text(meaning))
        }))
}

fn match_style(theme: &Theme) -> HighlightStyle {
    HighlightStyle {
        color: Some(theme.colors.text),
        background_color: Some(theme.colors.selected),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    }
}

/// Styles for a row label: the `#id` dimmed, the filter match marked.
fn label_highlights(
    info: &RowInfo,
    matched: Option<&RowMatch>,
    theme: &Theme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let label = info.label.as_ref();
    let matched = matched
        .and_then(|matched| match (matched.field, info.kind) {
            (MatchField::Label, _) => matched.range.clone(),
            // Text rows show their text quoted: shift past the quote.
            (MatchField::Text, RowKind::Text) => matched
                .range
                .clone()
                .map(|range| range.start + 1..range.end + 1)
                .filter(|range| range.end < label.len()),
            _ => None,
        })
        .filter(|range| label.is_char_boundary(range.start) && label.is_char_boundary(range.end));
    let id = info.id_start.map(|start| start..label.len());
    let mut bounds = vec![0, label.len()];
    for range in [&matched, &id].into_iter().flatten() {
        bounds.extend([range.start, range.end]);
    }
    bounds.sort_unstable();
    bounds.dedup();
    bounds
        .windows(2)
        .filter_map(|window| {
            let segment = window[0]..window[1];
            let in_match = matched
                .as_ref()
                .is_some_and(|range| range.start <= segment.start && segment.end <= range.end);
            let in_id = id
                .as_ref()
                .is_some_and(|range| range.start <= segment.start);
            let style = match (in_match, in_id) {
                (true, _) => match_style(theme),
                (false, true) => HighlightStyle {
                    color: Some(theme.colors.text_muted),
                    ..Default::default()
                },
                (false, false) => return None,
            };
            Some((segment, style))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Density;

    fn info(label: &str, id_start: Option<usize>, kind: RowKind) -> RowInfo {
        let mut capture = gpui::inspector::InspectorCapture::new_for_test();
        let mut builder = crate::fixtures::TreeBuilder::new(&mut capture);
        builder.leaf(crate::fixtures::ElementSpec::div());
        let tree = std::sync::Arc::new(builder.build(0));
        let mut info = TreeIndex::build(tree, |_| None).infos.remove(0);
        info.label = label.to_string().into();
        info.id_start = id_start;
        info.kind = kind;
        info
    }

    #[test]
    fn ids_are_dimmed_and_matches_marked_without_overlap() {
        let theme = Theme::get(true, Density::Compact);
        let row = info("div#row-3", Some(3), RowKind::Element);
        let runs = label_highlights(
            &row,
            Some(&RowMatch {
                field: MatchField::Label,
                range: Some(2..6),
            }),
            theme,
        );
        let ranges: Vec<Range<usize>> = runs.iter().map(|(range, _)| range.clone()).collect();
        assert_eq!(ranges, [2..3, 3..6, 6..9]);
        assert_eq!(runs[0].1, match_style(theme));
        assert_eq!(runs[1].1, match_style(theme));
        assert_eq!(runs[2].1.color, Some(theme.colors.text_muted));

        let text = info("\"Hello\"", None, RowKind::Text);
        let runs = label_highlights(
            &text,
            Some(&RowMatch {
                field: MatchField::Text,
                range: Some(1..3),
            }),
            theme,
        );
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, 2..4, "shifted past the opening quote");
        assert!(label_highlights(&text, None, theme).is_empty());
    }
}
