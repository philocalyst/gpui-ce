//! A virtualized tree keyed by stable ids.
//!
//! [`TreeModel`] is the pure part: preorder nodes, the expanded set, the
//! visible rows and keyboard navigation. [`TreeState`] wraps it with a focus
//! handle and scroll handle and emits [`TreeEvent`]s. [`Tree`] renders the
//! visible rows through `uniform_list`, drawing indentation and disclosure
//! chevrons and delegating each row's content to a closure.
//!
//! Selection and expansion are keyed by `K`, so replacing the nodes with a
//! newer snapshot (the next frame's element tree) keeps the user's place.

use crate::{
    theme::{Theme, UI_FONT},
    widgets::{
        ActivateSelected, CollapseSelected, ExpandSelected, Icon, IconName, LIST_CONTEXT,
        SelectFirst, SelectLast, SelectNext, SelectPrevious,
    },
};
use gpui::{
    Action, AnyElement, App, Bounds, Context, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, IntoElement, Pixels, Point, RenderOnce, ScrollStrategy, Styled,
    UniformListDecoration, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    rc::Rc,
};

/// One node of a tree, listed in preorder (every node directly follows its
/// parent or its previous sibling's subtree).
#[derive(Clone, Debug, PartialEq)]
pub struct TreeNode<K> {
    /// Stable identity across snapshots.
    pub key: K,
    /// Index of the parent node, if any.
    pub parent: Option<usize>,
    /// Depth from the roots (roots are 0).
    pub depth: u16,
    /// Whether the node has children (and so a disclosure chevron).
    pub has_children: bool,
}

/// Flattens a tree into preorder [`TreeNode`]s.
///
/// `children(item)` lists an item's children in order and `key(item)` gives
/// its stable key.
pub fn flatten<I, K, C>(
    roots: impl IntoIterator<Item = I>,
    children: impl Fn(&I) -> C,
    key: impl Fn(&I) -> K,
) -> Vec<TreeNode<K>>
where
    C: IntoIterator<Item = I>,
{
    let mut nodes = Vec::new();
    let mut stack: Vec<(I, Option<usize>, u16)> = Vec::new();
    let mut pending: Vec<I> = roots.into_iter().collect();
    pending.reverse();
    stack.extend(pending.into_iter().map(|item| (item, None, 0)));
    while let Some((item, parent, depth)) = stack.pop() {
        let mut kids: Vec<I> = children(&item).into_iter().collect();
        let ix = nodes.len();
        nodes.push(TreeNode {
            key: key(&item),
            parent,
            depth,
            has_children: !kids.is_empty(),
        });
        kids.reverse();
        stack.extend(kids.into_iter().map(|kid| (kid, Some(ix), depth + 1)));
    }
    nodes
}

/// The pure state of a tree: nodes, expansion, visible rows and selection.
#[derive(Clone, Debug)]
pub struct TreeModel<K> {
    nodes: Vec<TreeNode<K>>,
    index: HashMap<K, usize>,
    expanded: HashSet<K>,
    rows: Vec<usize>,
    selected: Option<K>,
}

impl<K> Default for TreeModel<K> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            index: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: None,
        }
    }
}

impl<K: Clone + Eq + Hash> TreeModel<K> {
    /// Replaces the nodes, keeping expansion and selection by key.
    pub fn set_nodes(&mut self, nodes: Vec<TreeNode<K>>) {
        self.index = nodes
            .iter()
            .enumerate()
            .map(|(ix, node)| (node.key.clone(), ix))
            .collect();
        self.nodes = nodes;
        self.rebuild_rows();
    }

    /// All nodes, in preorder.
    pub fn nodes(&self) -> &[TreeNode<K>] {
        &self.nodes
    }

    /// Visible rows, as indices into [`Self::nodes`].
    pub fn rows(&self) -> &[usize] {
        &self.rows
    }

    /// The node index for `key`.
    pub fn node_ix(&self, key: &K) -> Option<usize> {
        self.index.get(key).copied()
    }

    /// The visible row showing `key`.
    pub fn row_of(&self, key: &K) -> Option<usize> {
        let node = self.node_ix(key)?;
        self.rows.iter().position(|&candidate| candidate == node)
    }

    /// Whether `key` is expanded.
    pub fn is_expanded(&self, key: &K) -> bool {
        self.expanded.contains(key)
    }

    /// Expands or collapses `key`.
    pub fn set_expanded(&mut self, key: &K, expanded: bool) {
        let changed = if expanded {
            self.expanded.insert(key.clone())
        } else {
            self.expanded.remove(key)
        };
        if changed {
            self.rebuild_rows();
        }
    }

    /// Flips the expansion of `key`.
    pub fn toggle(&mut self, key: &K) {
        let expanded = self.is_expanded(key);
        self.set_expanded(key, !expanded);
    }

    /// Expands every node shallower than `depth`.
    pub fn expand_to_depth(&mut self, depth: u16) {
        for node in &self.nodes {
            if node.has_children && node.depth < depth {
                self.expanded.insert(node.key.clone());
            }
        }
        self.rebuild_rows();
    }

    /// The selected key (it may not be visible or present in the current nodes).
    pub fn selected(&self) -> Option<&K> {
        self.selected.as_ref()
    }

    /// The visible row of the selection.
    pub fn selected_row(&self) -> Option<usize> {
        self.row_of(self.selected.as_ref()?)
    }

    /// Selects `key` (or clears the selection).
    pub fn select(&mut self, key: Option<K>) {
        self.selected = key;
    }

    /// Selects the row `delta` rows away from the selection, clamped. With no
    /// visible selection, moving down selects the first row and up the last.
    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let row = match self.selected_row() {
            Some(row) => row.saturating_add_signed(delta).min(last),
            None if delta >= 0 => 0,
            None => last,
        };
        self.select_row(row);
    }

    /// Selects the first visible row.
    pub fn select_first(&mut self) {
        if !self.rows.is_empty() {
            self.select_row(0);
        }
    }

    /// Selects the last visible row.
    pub fn select_last(&mut self) {
        if let Some(last) = self.rows.len().checked_sub(1) {
            self.select_row(last);
        }
    }

    /// Right arrow: expands a collapsed parent, or steps into its first child.
    pub fn expand_or_descend(&mut self) {
        let Some(node) = self.selected_node() else {
            return;
        };
        if !node.has_children {
            return;
        }
        let key = node.key.clone();
        if self.is_expanded(&key) {
            self.move_selection(1);
        } else {
            self.set_expanded(&key, true);
        }
    }

    /// Left arrow: collapses an expanded parent, or steps out to the parent.
    pub fn collapse_or_ascend(&mut self) {
        let Some(node) = self.selected_node() else {
            return;
        };
        let key = node.key.clone();
        if node.has_children && self.is_expanded(&key) {
            self.set_expanded(&key, false);
        } else if let Some(parent) = node.parent {
            self.selected = Some(self.nodes[parent].key.clone());
        }
    }

    /// Expands every ancestor of `key` and selects it.
    pub fn reveal(&mut self, key: &K) {
        let Some(mut ix) = self.node_ix(key) else {
            return;
        };
        while let Some(parent) = self.nodes[ix].parent {
            self.expanded.insert(self.nodes[parent].key.clone());
            ix = parent;
        }
        self.selected = Some(key.clone());
        self.rebuild_rows();
    }

    fn selected_node(&self) -> Option<&TreeNode<K>> {
        let ix = self.node_ix(self.selected.as_ref()?)?;
        self.nodes.get(ix)
    }

    fn select_row(&mut self, row: usize) {
        if let Some(&node) = self.rows.get(row) {
            self.selected = Some(self.nodes[node].key.clone());
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        let mut hidden_below: Option<u16> = None;
        for (ix, node) in self.nodes.iter().enumerate() {
            if let Some(depth) = hidden_below {
                if node.depth > depth {
                    continue;
                }
                hidden_below = None;
            }
            self.rows.push(ix);
            if node.has_children && !self.expanded.contains(&node.key) {
                hidden_below = Some(node.depth);
            }
        }
    }
}

/// What a [`TreeState`] tells its owner.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeEvent<K> {
    /// The selection moved to this key (by click or keyboard).
    Selected(K),
    /// Enter or double-click on this key.
    Activated(K),
    /// The pointer entered this row, or left the tree.
    Hovered(Option<K>),
}

/// A tree's model plus its focus and scroll handles. Owners replace the nodes
/// with [`TreeState::set_nodes`] and subscribe to [`TreeEvent`]s.
pub struct TreeState<K> {
    model: TreeModel<K>,
    hovered: Option<K>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl<K: 'static> EventEmitter<TreeEvent<K>> for TreeState<K> {}

impl<K: 'static> Focusable for TreeState<K> {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl<K: Clone + Eq + Hash + 'static> TreeState<K> {
    /// An empty tree.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            model: TreeModel::default(),
            hovered: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// The pure model.
    pub fn model(&self) -> &TreeModel<K> {
        &self.model
    }

    /// Mutable access to the model; call `cx.notify()` afterwards.
    pub fn model_mut(&mut self) -> &mut TreeModel<K> {
        &mut self.model
    }

    /// Replaces the nodes, keeping expansion and selection.
    pub fn set_nodes(&mut self, nodes: Vec<TreeNode<K>>, cx: &mut Context<Self>) {
        self.model.set_nodes(nodes);
        cx.notify();
    }

    /// Selects `key`, expanding its ancestors and scrolling it into view.
    /// Does not emit [`TreeEvent::Selected`]: the caller already knows.
    pub fn reveal(&mut self, key: &K, cx: &mut Context<Self>) {
        self.model.reveal(key);
        self.scroll_to_selection();
        cx.notify();
    }

    /// The row under the pointer.
    pub fn hovered(&self) -> Option<&K> {
        self.hovered.as_ref()
    }

    fn scroll_to_selection(&self) {
        if let Some(row) = self.model.selected_row() {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    fn navigate(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut TreeModel<K>)) {
        let before = self.model.selected().cloned();
        f(&mut self.model);
        self.scroll_to_selection();
        if let Some(selected) = self.model.selected().cloned()
            && Some(&selected) != before.as_ref()
        {
            cx.emit(TreeEvent::Selected(selected));
        }
        cx.notify();
    }

    fn click_row(&mut self, node: usize, click_count: usize, cx: &mut Context<Self>) {
        let Some(key) = self.model.nodes().get(node).map(|node| node.key.clone()) else {
            return;
        };
        if self.model.selected() != Some(&key) {
            self.model.select(Some(key.clone()));
            cx.emit(TreeEvent::Selected(key.clone()));
        }
        if click_count >= 2 {
            cx.emit(TreeEvent::Activated(key));
        }
        cx.notify();
    }

    fn toggle_node(&mut self, node: usize, cx: &mut Context<Self>) {
        if let Some(key) = self.model.nodes().get(node).map(|node| node.key.clone()) {
            self.model.toggle(&key);
            cx.notify();
        }
    }

    fn hovered_node(&self) -> Option<usize> {
        self.model.node_ix(self.hovered.as_ref()?)
    }

    fn hover_node(&mut self, node: Option<usize>, cx: &mut Context<Self>) {
        let key = node.and_then(|node| self.model.nodes().get(node).map(|node| node.key.clone()));
        if key != self.hovered {
            self.hovered = key.clone();
            cx.emit(TreeEvent::Hovered(key));
        }
    }
}

/// What a row renderer gets for one visible row.
#[derive(Clone, Debug)]
pub struct TreeRow<K> {
    /// The node's key.
    pub key: K,
    /// Index into [`TreeModel::nodes`].
    pub node: usize,
    /// Depth from the roots.
    pub depth: u16,
    /// Whether this row is selected.
    pub selected: bool,
}

type RowRenderer<K> = Rc<dyn Fn(&TreeRow<K>, &mut Window, &mut App) -> AnyElement>;
type HeaderRenderer<K> = Rc<dyn Fn(&TreeModel<K>, usize, &Window, &App) -> Option<AnyElement>>;

/// Renders a [`TreeState`]. Fills its parent; rows are `Theme::metrics.row` tall.
#[derive(IntoElement)]
pub struct Tree<K: Clone + Eq + Hash + 'static> {
    id: ElementId,
    state: Entity<TreeState<K>>,
    render_row: RowRenderer<K>,
    pinned_header: Option<HeaderRenderer<K>>,
}

impl<K: Clone + Eq + Hash + 'static> Tree<K> {
    /// A tree over `state`, rendering each row's content with `render_row`
    /// (indentation and the disclosure chevron are drawn by the tree).
    pub fn new(
        id: impl Into<ElementId>,
        state: &Entity<TreeState<K>>,
        render_row: impl Fn(&TreeRow<K>, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            render_row: Rc::new(render_row),
            pinned_header: None,
        }
    }

    /// While the list is scrolled, pins the element `header` returns over
    /// its top edge (one row tall), given the model and the visible row just
    /// below it: e.g. a breadcrumb of that row's ancestors. It is laid out
    /// with the rows, so it always matches the scroll position.
    pub fn pinned_header(
        mut self,
        header: impl Fn(&TreeModel<K>, usize, &Window, &App) -> Option<AnyElement> + 'static,
    ) -> Self {
        self.pinned_header = Some(Rc::new(header));
        self
    }
}

/// Draws a [`Tree::pinned_header`] as a decoration of the list.
struct PinnedHeader<K: Clone + Eq + Hash + 'static> {
    state: Entity<TreeState<K>>,
    header: HeaderRenderer<K>,
}

impl<K: Clone + Eq + Hash + 'static> UniformListDecoration for PinnedHeader<K> {
    fn compute(
        &self,
        _visible_range: std::ops::Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        item_height: Pixels,
        item_count: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        // Decorations are laid out in content space; the header sits at the
        // viewport's top edge.
        let scrolled = -scroll_offset.y;
        let root = div().w(bounds.size.width).h(bounds.size.height);
        if scrolled <= Pixels::ZERO || item_height <= Pixels::ZERO || item_count == 0 {
            return root.into_any_element();
        }
        // The first row whose top is at or below the header's bottom edge.
        let below = ((scrolled + item_height) / item_height - 0.01).ceil() as usize;
        let state = self.state.read(cx);
        let header = (self.header)(&state.model, below.min(item_count - 1), window, cx);
        root.children(header.map(|header| {
            div()
                .absolute()
                .top(scrolled)
                .left_0()
                .w(bounds.size.width)
                .h(item_height)
                .child(header)
        }))
        .into_any_element()
    }
}

impl<K: Clone + Eq + Hash + 'static> RenderOnce for Tree<K> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let state = self.state.clone();
        let (row_count, focus, scroll) = {
            let tree = state.read(cx);
            (
                tree.model.rows().len(),
                tree.focus.clone(),
                tree.scroll.clone(),
            )
        };
        let list_state = state.clone();
        let render_row = self.render_row.clone();
        let pinned_header = self.pinned_header.clone().map(|header| PinnedHeader {
            state: state.clone(),
            header,
        });
        let list = uniform_list(self.id.clone(), row_count, move |range, window, cx| {
            let theme = Theme::of(window, cx);
            let rows: Vec<(TreeRow<K>, bool, bool)> = {
                let tree = list_state.read(cx);
                let model = &tree.model;
                model.rows()[range]
                    .iter()
                    .map(|&node_ix| {
                        let node = &model.nodes()[node_ix];
                        (
                            TreeRow {
                                key: node.key.clone(),
                                node: node_ix,
                                depth: node.depth,
                                selected: model.selected() == Some(&node.key),
                            },
                            node.has_children,
                            model.is_expanded(&node.key),
                        )
                    })
                    .collect()
            };
            rows.into_iter()
                .map(|(row, has_children, expanded)| {
                    let node = row.node;
                    let content = render_row(&row, window, cx);
                    let click_state = list_state.clone();
                    let hover_state = list_state.clone();
                    let toggle_state = list_state.clone();
                    div()
                        .id(node)
                        .h(theme.metrics.row)
                        .w_full()
                        .flex()
                        .items_center()
                        .pl(theme.metrics.gutter + theme.metrics.indent * row.depth as f32)
                        .pr(theme.metrics.gutter)
                        .when(row.selected, |this| this.bg(theme.colors.selected))
                        .when(!row.selected, |this| {
                            this.hover(|style| style.bg(theme.colors.hover))
                        })
                        .child(
                            div()
                                .id("disclosure")
                                .flex_none()
                                .size(px(14.))
                                .mr(px(2.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(has_children, |this| {
                                    this.cursor_pointer()
                                        .child(
                                            Icon::new(if expanded {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            })
                                            .size(px(10.))
                                            .color(theme.colors.text_faint),
                                        )
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            toggle_state
                                                .update(cx, |tree, cx| tree.toggle_node(node, cx));
                                        })
                                }),
                        )
                        .child(div().flex_1().min_w_0().h_full().child(content))
                        .on_click(move |event, window, cx| {
                            let focus = click_state.read(cx).focus.clone();
                            window.focus(&focus, cx);
                            let clicks = event.click_count();
                            click_state.update(cx, |tree, cx| tree.click_row(node, clicks, cx));
                        })
                        .on_hover(move |hovered, _, cx| {
                            hover_state.update(cx, |tree, cx| {
                                if *hovered {
                                    tree.hover_node(Some(node), cx);
                                } else if tree.hovered_node() == Some(node) {
                                    // Leaving a row only clears the hover if no
                                    // other row claimed it in the meantime.
                                    tree.hover_node(None, cx);
                                }
                            })
                        })
                })
                .collect()
        })
        .size_full()
        .track_scroll(&scroll);
        let list = match pinned_header {
            Some(header) => list.with_decoration(header),
            None => list,
        };

        let activate_state = state.clone();
        div()
            .key_context(LIST_CONTEXT)
            .track_focus(&focus)
            .size_full()
            .font_family(UI_FONT)
            .text_size(theme.metrics.text)
            .line_height(theme.metrics.line_height)
            .text_color(theme.colors.text)
            .on_action(navigate::<K, SelectNext>(&state, |model| {
                model.move_selection(1)
            }))
            .on_action(navigate::<K, SelectPrevious>(&state, |model| {
                model.move_selection(-1)
            }))
            .on_action(navigate::<K, SelectFirst>(&state, TreeModel::select_first))
            .on_action(navigate::<K, SelectLast>(&state, TreeModel::select_last))
            .on_action(navigate::<K, ExpandSelected>(
                &state,
                TreeModel::expand_or_descend,
            ))
            .on_action(navigate::<K, CollapseSelected>(
                &state,
                TreeModel::collapse_or_ascend,
            ))
            .on_action(move |_: &ActivateSelected, _, cx| {
                activate_state.update(cx, |tree, cx| {
                    if let Some(key) = tree.model.selected().cloned() {
                        cx.emit(TreeEvent::Activated(key));
                    }
                })
            })
            .child(list)
    }
}

/// An action handler that runs a keyboard navigation step on the tree.
fn navigate<K: Clone + Eq + Hash + 'static, A: Action>(
    state: &Entity<TreeState<K>>,
    step: fn(&mut TreeModel<K>),
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |_, _, cx| state.update(cx, |tree, cx| tree.navigate(cx, step))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn children(name: &&'static str) -> Vec<&'static str> {
        match *name {
            "a" => vec!["b", "d"],
            "b" => vec!["c"],
            _ => vec![],
        }
    }

    /// ```text
    /// a
    /// ├─ b
    /// │  └─ c
    /// └─ d
    /// e
    /// ```
    fn sample() -> TreeModel<&'static str> {
        let mut model = TreeModel::default();
        model.set_nodes(flatten(["a", "e"], children, |name| *name));
        model
    }

    fn visible(model: &TreeModel<&'static str>) -> Vec<&'static str> {
        model
            .rows()
            .iter()
            .map(|&ix| model.nodes()[ix].key)
            .collect()
    }

    #[test]
    fn flatten_lists_nodes_in_preorder_with_parents_and_depths() {
        let model = sample();
        let nodes = model.nodes();
        let keys: Vec<_> = nodes.iter().map(|node| node.key).collect();
        assert_eq!(keys, ["a", "b", "c", "d", "e"]);
        assert_eq!(nodes[2].parent, Some(1));
        assert_eq!(nodes[2].depth, 2);
        assert_eq!(nodes[3].parent, Some(0));
        assert!(nodes[0].has_children && !nodes[3].has_children);
    }

    #[test]
    fn collapsed_nodes_hide_their_subtrees() {
        let mut model = sample();
        assert_eq!(visible(&model), ["a", "e"]);
        model.set_expanded(&"a", true);
        assert_eq!(visible(&model), ["a", "b", "d", "e"]);
        model.expand_to_depth(3);
        assert_eq!(visible(&model), ["a", "b", "c", "d", "e"]);
        model.set_expanded(&"b", false);
        assert_eq!(visible(&model), ["a", "b", "d", "e"]);
    }

    #[test]
    fn up_and_down_move_through_visible_rows_and_clamp() {
        let mut model = sample();
        model.expand_to_depth(3);
        model.move_selection(1);
        assert_eq!(model.selected(), Some(&"a"));
        model.move_selection(2);
        assert_eq!(model.selected(), Some(&"c"));
        model.move_selection(10);
        assert_eq!(model.selected(), Some(&"e"));
        model.move_selection(-1);
        assert_eq!(model.selected(), Some(&"d"));
        model.select_first();
        model.move_selection(-1);
        assert_eq!(model.selected(), Some(&"a"));
    }

    #[test]
    fn moving_up_without_a_selection_selects_the_last_row() {
        let mut model = sample();
        model.move_selection(-1);
        assert_eq!(model.selected(), Some(&"e"));
    }

    #[test]
    fn right_expands_then_descends_and_left_collapses_then_ascends() {
        let mut model = sample();
        model.select(Some("a"));
        model.expand_or_descend();
        assert!(model.is_expanded(&"a"));
        assert_eq!(model.selected(), Some(&"a"));
        model.expand_or_descend();
        assert_eq!(model.selected(), Some(&"b"));
        model.collapse_or_ascend();
        assert_eq!(
            model.selected(),
            Some(&"a"),
            "b is collapsed, so left steps out to its parent"
        );
        model.collapse_or_ascend();
        assert!(!model.is_expanded(&"a"));
        assert_eq!(visible(&model), ["a", "e"]);
    }

    #[test]
    fn selection_and_expansion_survive_new_snapshots() {
        let mut model = sample();
        model.reveal(&"c");
        assert_eq!(model.selected_row(), Some(2));
        // The next snapshot inserts a root before `a`; `c` keeps its place by key.
        model.set_nodes(flatten(["z", "a", "e"], children, |name| *name));
        assert_eq!(model.selected(), Some(&"c"));
        assert_eq!(visible(&model), ["z", "a", "b", "c", "d", "e"]);
        assert_eq!(model.selected_row(), Some(3));
    }

    #[test]
    fn a_selection_missing_from_the_snapshot_is_kept_but_not_visible() {
        let mut model = sample();
        model.select(Some("d"));
        model.set_nodes(flatten(["e"], |_| Vec::new(), |name| *name));
        assert_eq!(model.selected(), Some(&"d"));
        assert_eq!(model.selected_row(), None);
        model.move_selection(1);
        assert_eq!(model.selected(), Some(&"e"));
    }
}
