//! A virtualized, sortable table keyed by stable ids.
//!
//! [`TableModel`] is the pure part: the row keys, the sort and the selection.
//! [`TableState`] adds focus and scrolling and emits [`TableEvent`]s, and
//! [`Table`] renders a clickable header plus the visible rows through
//! `uniform_list`, delegating each cell to a closure.

use crate::{
    theme::{Theme, UI_FONT},
    widgets::{
        ActivateSelected, Icon, IconName, LIST_CONTEXT, SelectFirst, SelectLast, SelectNext,
        SelectPrevious,
    },
};
use gpui::{
    Action, AnyElement, App, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, IntoElement, Pixels, RenderOnce, ScrollStrategy, SharedString, Styled,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use std::{cmp::Ordering, hash::Hash, rc::Rc};

/// Sort direction of a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

impl SortDirection {
    fn flipped(self) -> Self {
        match self {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        }
    }
}

/// How wide a column is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    /// A fixed width.
    Fixed(Pixels),
    /// A share of the remaining width.
    Flex(f32),
}

/// A column definition.
#[derive(Clone, Debug)]
pub struct Column {
    title: SharedString,
    width: ColumnWidth,
    numeric: bool,
    sortable: bool,
    first_direction: SortDirection,
}

impl Column {
    /// A flexible, sortable text column.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            width: ColumnWidth::Flex(1.),
            numeric: false,
            sortable: true,
            first_direction: SortDirection::Ascending,
        }
    }

    /// Sets the width.
    pub fn width(mut self, width: ColumnWidth) -> Self {
        self.width = width;
        self
    }

    /// A numeric column: right-aligned, sorting largest first on first click.
    pub fn numeric(mut self) -> Self {
        self.numeric = true;
        self.first_direction = SortDirection::Descending;
        self
    }

    /// Disables sorting by this column.
    pub fn unsortable(mut self) -> Self {
        self.sortable = false;
        self
    }
}

/// Row order for a sort: data indices `0..len`, stably sorted by `compare(column, a, b)`.
/// Equal rows keep their data order in both directions.
pub fn sort_order(
    len: usize,
    sort: Option<(usize, SortDirection)>,
    compare: &dyn Fn(usize, usize, usize) -> Ordering,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if let Some((column, direction)) = sort {
        order.sort_by(|&a, &b| match direction {
            SortDirection::Ascending => compare(column, a, b),
            SortDirection::Descending => compare(column, b, a),
        });
    }
    order
}

type Comparator = Rc<dyn Fn(usize, usize, usize) -> Ordering>;

/// The pure state of a table: row keys in data order, the sort, the visible
/// order and the selection (by key).
pub struct TableModel<K> {
    keys: Vec<K>,
    compare: Comparator,
    sort: Option<(usize, SortDirection)>,
    order: Vec<usize>,
    selected: Option<K>,
}

impl<K> Default for TableModel<K> {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            compare: Rc::new(|_, _, _| Ordering::Equal),
            sort: None,
            order: Vec::new(),
            selected: None,
        }
    }
}

impl<K: Clone + Eq + Hash> TableModel<K> {
    /// Replaces the rows. `compare(column, a, b)` compares data rows `a` and
    /// `b` by `column`; the current sort and selection are kept.
    pub fn set_rows(
        &mut self,
        keys: Vec<K>,
        compare: impl Fn(usize, usize, usize) -> Ordering + 'static,
    ) {
        self.keys = keys;
        self.compare = Rc::new(compare);
        self.resort();
    }

    /// Sorts by `column`: a new column sorts in its first direction, the
    /// current one flips.
    pub fn sort_by(&mut self, column: usize, first_direction: SortDirection) {
        self.sort = Some(match self.sort {
            Some((current, direction)) if current == column => (column, direction.flipped()),
            _ => (column, first_direction),
        });
        self.resort();
    }

    /// The current sort.
    pub fn sort(&self) -> Option<(usize, SortDirection)> {
        self.sort
    }

    /// Number of rows.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether there are no rows.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The data index shown at visible `row`.
    pub fn data_ix(&self, row: usize) -> Option<usize> {
        self.order.get(row).copied()
    }

    /// The key shown at visible `row`.
    pub fn key_at(&self, row: usize) -> Option<&K> {
        self.keys.get(self.data_ix(row)?)
    }

    /// The selected key.
    pub fn selected(&self) -> Option<&K> {
        self.selected.as_ref()
    }

    /// Selects `key` (or clears the selection).
    pub fn select(&mut self, key: Option<K>) {
        self.selected = key;
    }

    /// The visible row of the selection.
    pub fn selected_row(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.order
            .iter()
            .position(|&data_ix| self.keys.get(data_ix) == Some(selected))
    }

    /// Moves the selection by `delta` rows, clamped; with no selection, down
    /// selects the first row and up the last.
    pub fn move_selection(&mut self, delta: isize) {
        let Some(last) = self.order.len().checked_sub(1) else {
            return;
        };
        let row = match self.selected_row() {
            Some(row) => row.saturating_add_signed(delta).min(last),
            None if delta >= 0 => 0,
            None => last,
        };
        self.selected = self.key_at(row).cloned();
    }

    /// Selects the first row.
    pub fn select_first(&mut self) {
        self.selected = self.key_at(0).cloned();
    }

    /// Selects the last row.
    pub fn select_last(&mut self) {
        if let Some(last) = self.order.len().checked_sub(1) {
            self.selected = self.key_at(last).cloned();
        }
    }

    fn resort(&mut self) {
        self.order = sort_order(self.keys.len(), self.sort, self.compare.as_ref());
    }
}

/// What a [`TableState`] tells its owner.
#[derive(Clone, Debug, PartialEq)]
pub enum TableEvent<K> {
    /// The selection moved to this key.
    Selected(K),
    /// Enter or double-click on this key.
    Activated(K),
}

/// A table's model plus focus and scrolling.
pub struct TableState<K> {
    model: TableModel<K>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl<K: 'static> EventEmitter<TableEvent<K>> for TableState<K> {}

impl<K: 'static> Focusable for TableState<K> {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl<K: Clone + Eq + Hash + 'static> TableState<K> {
    /// An empty table.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            model: TableModel::default(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// The pure model.
    pub fn model(&self) -> &TableModel<K> {
        &self.model
    }

    /// Replaces the rows; see [`TableModel::set_rows`].
    pub fn set_rows(
        &mut self,
        keys: Vec<K>,
        compare: impl Fn(usize, usize, usize) -> Ordering + 'static,
        cx: &mut Context<Self>,
    ) {
        self.model.set_rows(keys, compare);
        cx.notify();
    }

    /// Selects `key` and scrolls it into view, without emitting an event.
    pub fn reveal(&mut self, key: K, cx: &mut Context<Self>) {
        self.model.select(Some(key));
        self.scroll_to_selection();
        cx.notify();
    }

    fn scroll_to_selection(&self) {
        if let Some(row) = self.model.selected_row() {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    fn navigate(&mut self, cx: &mut Context<Self>, step: fn(&mut TableModel<K>)) {
        let before = self.model.selected().cloned();
        step(&mut self.model);
        self.scroll_to_selection();
        if let Some(selected) = self.model.selected().cloned()
            && Some(&selected) != before.as_ref()
        {
            cx.emit(TableEvent::Selected(selected));
        }
        cx.notify();
    }

    fn click_row(&mut self, row: usize, click_count: usize, cx: &mut Context<Self>) {
        let Some(key) = self.model.key_at(row).cloned() else {
            return;
        };
        if self.model.selected() != Some(&key) {
            self.model.select(Some(key.clone()));
            cx.emit(TableEvent::Selected(key.clone()));
        }
        if click_count >= 2 {
            cx.emit(TableEvent::Activated(key));
        }
        cx.notify();
    }
}

type CellRenderer = Rc<dyn Fn(usize, usize, &mut Window, &mut App) -> AnyElement>;

/// Renders a [`TableState`]: a header row and the visible rows. Fills its parent.
#[derive(IntoElement)]
pub struct Table<K: Clone + Eq + Hash + 'static> {
    id: ElementId,
    state: Entity<TableState<K>>,
    columns: Rc<[Column]>,
    render_cell: CellRenderer,
}

impl<K: Clone + Eq + Hash + 'static> Table<K> {
    /// A table over `state`. `render_cell(data_ix, column, window, cx)`
    /// renders one cell's content.
    pub fn new(
        id: impl Into<ElementId>,
        state: &Entity<TableState<K>>,
        columns: Vec<Column>,
        render_cell: impl Fn(usize, usize, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            columns: columns.into(),
            render_cell: Rc::new(render_cell),
        }
    }
}

fn cell(column: &Column, gutter: Pixels) -> gpui::Div {
    let cell = div()
        .h_full()
        .px(gutter / 2.)
        .flex()
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap();
    let cell = match column.width {
        ColumnWidth::Fixed(width) => cell.flex_none().w(width),
        ColumnWidth::Flex(grow) => cell.flex_grow(grow).flex_basis(px(0.)).min_w_0(),
    };
    if column.numeric {
        cell.justify_end()
    } else {
        cell
    }
}

impl<K: Clone + Eq + Hash + 'static> RenderOnce for Table<K> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let state = self.state.clone();
        let (row_count, sort, focus, scroll) = {
            let table = state.read(cx);
            (
                table.model.len(),
                table.model.sort(),
                table.focus.clone(),
                table.scroll.clone(),
            )
        };
        let gutter = theme.metrics.gutter;

        let header = div()
            .flex_none()
            .h(theme.metrics.control)
            .px(gutter / 2.)
            .flex()
            .items_stretch()
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.line)
            .text_size(theme.metrics.text_small)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.text_muted)
            .children(self.columns.iter().enumerate().map(|(ix, column)| {
                let sorted = sort.filter(|(sorted, _)| *sorted == ix).map(|(_, dir)| dir);
                let first_direction = column.first_direction;
                let sort_state = state.clone();
                cell(column, gutter)
                    .id(ix)
                    .gap_1()
                    .child(div().truncate().child(column.title.clone()))
                    .children(sorted.map(|direction| {
                        Icon::new(match direction {
                            SortDirection::Ascending => IconName::ArrowUp,
                            SortDirection::Descending => IconName::ArrowDown,
                        })
                        .size(px(10.))
                        .color(colors.text)
                    }))
                    .when(sorted.is_some(), |this| this.text_color(colors.text))
                    .when(column.sortable, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.text_color(colors.text))
                            .on_click(move |_, _, cx| {
                                sort_state.update(cx, |table, cx| {
                                    table.model.sort_by(ix, first_direction);
                                    cx.notify();
                                })
                            })
                    })
            }));

        let columns = self.columns.clone();
        let render_cell = self.render_cell.clone();
        let list_state = state.clone();
        let rows = uniform_list(self.id.clone(), row_count, move |range, window, cx| {
            let theme = Theme::of(window, cx);
            let visible: Vec<(usize, usize, bool)> = {
                let model = &list_state.read(cx).model;
                range
                    .filter_map(|row| {
                        let data_ix = model.data_ix(row)?;
                        let selected =
                            model.selected().is_some() && model.selected() == model.key_at(row);
                        Some((row, data_ix, selected))
                    })
                    .collect()
            };
            visible
                .into_iter()
                .map(|(row, data_ix, selected)| {
                    let click_state = list_state.clone();
                    div()
                        .id(row)
                        .h(theme.metrics.row)
                        .w_full()
                        .px(theme.metrics.gutter / 2.)
                        .flex()
                        .items_stretch()
                        .when(selected, |this| this.bg(theme.colors.selected))
                        .when(!selected, |this| {
                            this.hover(|style| style.bg(theme.colors.hover))
                        })
                        .children(columns.iter().enumerate().map(|(column_ix, column)| {
                            cell(column, theme.metrics.gutter)
                                .child(render_cell(data_ix, column_ix, window, cx))
                        }))
                        .on_click(move |event, window, cx| {
                            let focus = click_state.read(cx).focus.clone();
                            window.focus(&focus, cx);
                            let clicks = event.click_count();
                            click_state.update(cx, |table, cx| table.click_row(row, clicks, cx));
                        })
                })
                .collect()
        })
        .flex_1()
        .min_h_0()
        .track_scroll(&scroll);

        let activate_state = state.clone();
        div()
            .key_context(LIST_CONTEXT)
            .track_focus(&focus)
            .size_full()
            .flex()
            .flex_col()
            .font_family(UI_FONT)
            .text_size(theme.metrics.text)
            .line_height(theme.metrics.line_height)
            .text_color(colors.text)
            .on_action(navigate::<K, SelectNext>(&state, |model| {
                model.move_selection(1)
            }))
            .on_action(navigate::<K, SelectPrevious>(&state, |model| {
                model.move_selection(-1)
            }))
            .on_action(navigate::<K, SelectFirst>(&state, TableModel::select_first))
            .on_action(navigate::<K, SelectLast>(&state, TableModel::select_last))
            .on_action(move |_: &ActivateSelected, _, cx| {
                activate_state.update(cx, |table, cx| {
                    if let Some(key) = table.model.selected().cloned() {
                        cx.emit(TableEvent::Activated(key));
                    }
                })
            })
            .child(header)
            .child(rows)
    }
}

/// An action handler that runs a keyboard navigation step on the table.
fn navigate<K: Clone + Eq + Hash + 'static, A: Action>(
    state: &Entity<TableState<K>>,
    step: fn(&mut TableModel<K>),
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |_, _, cx| state.update(cx, |table, cx| table.navigate(cx, step))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 4] = ["delta", "alpha", "charlie", "bravo"];
    const SIZES: [u32; 4] = [3, 7, 3, 1];

    fn compare(column: usize, a: usize, b: usize) -> Ordering {
        match column {
            0 => NAMES[a].cmp(NAMES[b]),
            _ => SIZES[a].cmp(&SIZES[b]),
        }
    }

    fn model() -> TableModel<&'static str> {
        let mut model = TableModel::default();
        model.set_rows(NAMES.to_vec(), compare);
        model
    }

    fn visible(model: &TableModel<&'static str>) -> Vec<&'static str> {
        (0..model.len())
            .filter_map(|row| model.key_at(row).copied())
            .collect()
    }

    #[test]
    fn unsorted_tables_keep_data_order() {
        assert_eq!(visible(&model()), NAMES);
    }

    #[test]
    fn clicking_a_column_sorts_then_flips() {
        let mut model = model();
        model.sort_by(0, SortDirection::Ascending);
        assert_eq!(visible(&model), ["alpha", "bravo", "charlie", "delta"]);
        model.sort_by(0, SortDirection::Ascending);
        assert_eq!(visible(&model), ["delta", "charlie", "bravo", "alpha"]);
    }

    #[test]
    fn numeric_columns_sort_largest_first_and_ties_keep_data_order() {
        let mut model = model();
        model.sort_by(1, SortDirection::Descending);
        assert_eq!(visible(&model), ["alpha", "delta", "charlie", "bravo"]);
        model.sort_by(1, SortDirection::Descending);
        assert_eq!(visible(&model), ["bravo", "delta", "charlie", "alpha"]);
    }

    #[test]
    fn selection_follows_its_key_through_sorts_and_refreshes() {
        let mut model = model();
        model.select(Some("charlie"));
        assert_eq!(model.selected_row(), Some(2));
        model.sort_by(0, SortDirection::Ascending);
        assert_eq!(model.selected_row(), Some(2));
        model.sort_by(0, SortDirection::Ascending);
        assert_eq!(model.selected_row(), Some(1));
        model.set_rows(vec!["charlie"], |_, _, _| Ordering::Equal);
        assert_eq!(model.selected_row(), Some(0));
    }

    #[test]
    fn keyboard_navigation_clamps() {
        let mut model = model();
        model.move_selection(1);
        assert_eq!(model.selected(), Some(&"delta"));
        model.move_selection(-5);
        assert_eq!(model.selected(), Some(&"delta"));
        model.move_selection(99);
        assert_eq!(model.selected(), Some(&"bravo"));
        model.select_first();
        assert_eq!(model.selected(), Some(&"delta"));
        model.select_last();
        assert_eq!(model.selected(), Some(&"bravo"));
    }
}
