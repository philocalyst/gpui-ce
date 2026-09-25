//! Elements: what is this, where did it come from, why does it look like that?
//!
//! ```text
//! ┌ tree ─────────────────────────┐┌ detail ─────────────────────────────┐
//! │ [⏷ Filter…         ] Your code ││ ◆ IssueList #issue-list    620×800 │
//! │ ◷ Frame #231 · 2.1 s ago  Live ││ ‹› issue_list.rs:52 ⧉  in ◆ InboxApp│
//! │ ◆ InboxApp                     ││ ▾ WHY THIS SIZE                     │
//! │   • div#root                   ││ ▾ BOX MODEL                         │
//! │     ◆ Sidebar            ×12   ││ ▾ STYLE · INTERACTIVITY · DETAILS   │
//! └────────────────────────────────┘└─────────────────────────────────────┘
//! ```
//!
//! The lens derives everything it shows in [`ElementsLens::sync`], which
//! runs when the shared [`LoupeState`] changes (new app data, selection,
//! filter, time travel) or the user edits something here, and memoizes it
//! by the tree it came from: the rows of a 5,000 element tree are built once
//! per captured tree and once per filter, never per render.

mod box_model;
mod detail;
mod rows;
mod style;
mod tree;

use super::{LensView, RailBadge};
use crate::{
    analysis::{
        format, source,
        style_grid::{self, PropertyValue, StyleEditError},
        why_size::{self, SizeExplanation},
    },
    loupe::Cancel,
    shell::pulse::cause_summary,
    state::{LensLayout, LoupeState},
    theme::{LoupeSettings, Theme},
    widgets::{Split, TreeEvent, TreeState, text_field_state},
};
use box_model::BoxFields;
use detail::Section;
use gpui::{
    App, AppContext as _, Axis, ClipboardItem, Context, Entity, EntityId, IntoElement, Pixels,
    SharedString, StyleRefinement, Subscription, Window, div,
    inspector::{
        ElementIndex, ElementKey, ElementKind, ElementTree, ForcedStates, InspectorCapture,
        OverlayHighlight, PathKey,
    },
    prelude::*,
    px,
};
use gpui_elements::editable_text::{EditableTextState, TextChanged};
use rows::{NodeKey, RowSet, TreeIndex, ViewStats, view_stats};
use std::{
    collections::{HashMap, HashSet},
    panic::Location,
    rc::Rc,
    sync::Arc,
};
use style::{StyleEditors, StyleModel};

/// Rows shallower than this start expanded the first time a tree shows.
const INITIAL_DEPTH: u16 = 3;
/// Height of Loupe's chrome around a lens body: toolbar, pulse strip, rail
/// and status bar at compact density (for the default split).
const CHROME_HEIGHT: Pixels = px(122.);

/// The tree Elements shows: the live one, or an earlier frame's.
#[derive(Clone)]
struct Shown {
    /// The tree.
    pub tree: Arc<ElementTree>,
    /// The frame that drew it.
    pub frame: u64,
    /// Set when time traveling.
    pub past: Option<PastFrame>,
}

/// An earlier frame whose tree is shown instead of the live one.
#[derive(Clone, Debug, PartialEq)]
struct PastFrame {
    /// Its id.
    pub frame: u64,
    /// How long before the latest frame it was drawn: `2.1 s ago`.
    pub ago: String,
}

/// The tree for the frame selected in the pulse strip, if that frame kept
/// one; otherwise the latest tree.
fn shown_tree(capture: &InspectorCapture, selected_frame: Option<u64>) -> Option<Shown> {
    let (latest_frame, latest) = capture
        .frames()
        .iter()
        .rev()
        .find_map(|frame| Some((frame.id, frame.tree.as_ref()?)))?;
    let past = selected_frame
        .and_then(|id| capture.frame(id))
        .and_then(|frame| Some((frame, frame.tree.as_ref()?)))
        .filter(|(_, tree)| !Arc::ptr_eq(tree, latest));
    Some(match past {
        Some((frame, tree)) => {
            let now = capture
                .latest_frame()
                .map_or(frame.start, |latest| latest.start);
            Shown {
                tree: tree.clone(),
                frame: frame.id,
                past: Some(PastFrame {
                    frame: frame.id,
                    ago: format::relative_time(now, frame.start),
                }),
            }
        }
        None => Shown {
            tree: latest.clone(),
            frame: latest_frame,
            past: None,
        },
    })
}

/// Memoized derived data, keyed by what it was derived from.
#[derive(Default)]
struct Memo {
    index: Option<Rc<TreeIndex>>,
    all: Option<(RowsKey, Rc<RowSet>)>,
    filtered: Option<(RowsKey, Rc<RowSet>)>,
    stats: Option<(u64, Rc<HashMap<EntityId, ViewStats>>)>,
    builds: MemoBuilds,
}

/// What rows were built from.
#[derive(Clone, Debug, PartialEq)]
struct RowsKey {
    tree: *const ElementTree,
    your_code: bool,
    query: SharedString,
}

/// How often each memo was rebuilt (tests prove renders reuse them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MemoBuilds {
    /// Tree indexes built.
    pub index: usize,
    /// Row sets built (unfiltered and filtered).
    pub rows: usize,
    /// View statistics computed.
    pub stats: usize,
}

/// What the detail pane shows about the selected element.
#[derive(Clone)]
struct Selection {
    /// The row.
    pub node: NodeKey,
    /// The tree the record comes from (the shown one, or where it was last seen).
    pub index: Rc<TreeIndex>,
    /// The record.
    pub ix: ElementIndex,
    /// The shown tree no longer has it: this is its last known record.
    pub missing: bool,
    /// The frame the record was drawn in.
    pub seen_in: u64,
    /// Edits apply: a keyed element of the live tree.
    pub editable: bool,
    /// Where it was constructed.
    pub source: Option<&'static Location<'static>>,
    /// Its nearest enclosing view (other than itself).
    pub owner: Option<(ElementKey, SharedString)>,
    /// Why it has its size.
    pub why: Option<SizeExplanation>,
    /// Its style grid (keyed elements).
    pub style: Option<StyleModel>,
    /// States Loupe forces on it.
    pub forced: ForcedStates,
    /// What drawing it costs.
    pub cost: Cost,
}

/// What drawing an element costs.
#[derive(Clone, Debug, Default, PartialEq)]
struct Cost {
    /// Scene primitives painted by it and its descendants.
    pub primitives: u32,
    /// For views: renders and cache hits over the recorded frames.
    pub view: Option<ViewCost>,
    /// For other elements: their view's renders.
    pub owner: Option<(SharedString, ViewStats)>,
}

/// A view's render history.
#[derive(Clone, Debug, Default, PartialEq)]
struct ViewCost {
    /// Renders and cache hits.
    pub stats: ViewStats,
    /// App frames recorded.
    pub frames: usize,
    /// Why its latest render happened, and when: `IssueStore notified`,
    /// frame id and `1.2 s ago`.
    pub last: Option<(String, u64, String)>,
}

/// The Elements lens.
pub(crate) struct ElementsLens {
    state: Entity<LoupeState>,
    /// The unfiltered tree (without gpui's elements in "your code" mode).
    tree: Entity<TreeState<NodeKey>>,
    /// The filter's results, fully expanded.
    filtered: Entity<TreeState<NodeKey>>,
    filter: Entity<EditableTextState>,
    /// The filter text last exchanged with the shared state.
    query: SharedString,
    your_code: bool,
    memo: Memo,
    shown: Option<Shown>,
    /// The selected row; may be an element without a key, which the shared
    /// state knows by its nearest keyed ancestor.
    selected: Option<NodeKey>,
    /// Where the selection was last seen, for when it disappears.
    last_seen: Option<(NodeKey, Rc<TreeIndex>, ElementIndex, u64)>,
    selection: Option<Selection>,
    box_fields: BoxFields,
    style_editors: StyleEditors,
    edit_error: Option<SharedString>,
    collapsed: HashSet<Section>,
    /// The highlight Loupe added for a hovered row without a key.
    hover_highlight: Option<OverlayHighlight>,
    split: Option<Pixels>,
    badge: Option<RailBadge>,
    expanded_once: bool,
    #[cfg(test)]
    opened_urls: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

impl ElementsLens {
    pub fn new(state: Entity<LoupeState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(TreeState::new);
        let filtered = cx.new(TreeState::new);
        let filter = text_field_state(cx);
        let subscriptions = vec![
            cx.observe_in(&state, window, |this, _, window, cx| this.sync(window, cx)),
            cx.subscribe_in(&tree, window, Self::on_tree_event),
            cx.subscribe_in(&filtered, window, Self::on_tree_event),
            // The lens is a cached view: models it draws must re-render it
            // themselves (folding a row, moving the caret in the filter).
            cx.observe(&tree, |_, _, cx| cx.notify()),
            cx.observe(&filtered, |_, _, cx| cx.notify()),
            cx.observe(&filter, |_, _, cx| cx.notify()),
            cx.subscribe(&filter, |this, filter, _: &TextChanged, cx| {
                let text = SharedString::from(filter.read(cx).as_str().to_string());
                if text == this.query {
                    return;
                }
                this.query = text.clone();
                this.state.update(cx, |state, cx| {
                    let mut filters = state.filters().clone();
                    filters.elements = text;
                    state.set_filters(filters, cx);
                });
            }),
        ];
        let box_fields = BoxFields::new(window, cx);
        let mut this = Self {
            state,
            tree,
            filtered,
            filter,
            query: SharedString::default(),
            your_code: false,
            memo: Memo::default(),
            shown: None,
            selected: None,
            last_seen: None,
            selection: None,
            box_fields,
            style_editors: StyleEditors::default(),
            edit_error: None,
            collapsed: HashSet::new(),
            hover_highlight: None,
            split: None,
            badge: None,
            expanded_once: false,
            #[cfg(test)]
            opened_urls: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.sync(window, cx);
        this
    }

    /// How often the memoized data was rebuilt.
    #[cfg(test)]
    pub(crate) fn memo_builds(&self) -> MemoBuilds {
        self.memo.builds
    }

    fn filtering(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// The tree state on screen: the filter's while filtering.
    fn active_tree(&self) -> &Entity<TreeState<NodeKey>> {
        if self.filtering() {
            &self.filtered
        } else {
            &self.tree
        }
    }

    /// The rows on screen.
    fn active_rows(&self) -> Option<Rc<RowSet>> {
        let slot = if self.filtering() {
            &self.memo.filtered
        } else {
            &self.memo.all
        };
        slot.as_ref().map(|(_, rows)| rows.clone())
    }

    /// Brings everything derived up to date with the capture and the shared
    /// state, then re-renders.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let (selected_frame, selected_key) = (state.selected_frame(), state.selected_element());
        let query = state.filters().elements.clone();
        // The field leads while the user types; follow the shared filter only
        // when something else changed it.
        if query != self.query {
            self.query = query.clone();
            if self.filter.read(cx).as_str() != query.as_ref() {
                self.filter
                    .update(cx, |filter, cx| filter.emplace(&query, cx));
            }
        }

        let Some(capture) = window.inspector_capture() else {
            return;
        };
        self.shown = shown_tree(capture, selected_frame);
        self.badge = Some(RailBadge::count(format::count(match &self.shown {
            Some(shown) => shown.tree.elements.len() as u64,
            None => capture
                .latest_app_frame()
                .map_or(0, |frame| u64::from(frame.element_count)),
        })));
        self.sync_stats(capture);
        let index = self.sync_index(capture);
        if let Some(index) = &index {
            self.sync_rows(index, &query, cx);
        }
        self.sync_selection(index.as_ref(), selected_key, window, cx);
        self.sync_style_editors(window, cx);
        if let Some(selection) = &self.selection
            && let Some(box_model) = selection.index.tree.get(selection.ix).and_then(|record| {
                record
                    .details
                    .as_deref()
                    .and_then(|details| details.box_model)
            })
        {
            self.box_fields.sync(&box_model, window, cx);
        }
        cx.notify();
    }

    fn sync_stats(&mut self, capture: &InspectorCapture) {
        let generation = capture.generation();
        if self
            .memo
            .stats
            .as_ref()
            .is_none_or(|(at, _)| *at != generation)
        {
            self.memo.builds.stats += 1;
            self.memo.stats = Some((generation, Rc::new(view_stats(capture.frames()))));
        }
    }

    fn stats(&self) -> Rc<HashMap<EntityId, ViewStats>> {
        self.memo
            .stats
            .as_ref()
            .map(|(_, stats)| stats.clone())
            .unwrap_or_default()
    }

    fn sync_index(&mut self, capture: &InspectorCapture) -> Option<Rc<TreeIndex>> {
        let shown = self.shown.as_ref()?;
        if let Some(index) = &self.memo.index
            && Arc::ptr_eq(&index.tree, &shown.tree)
        {
            return Some(index.clone());
        }
        self.memo.builds.index += 1;
        let index = Rc::new(TreeIndex::build(shown.tree.clone(), |path| {
            capture.path_info(path).map(|info| info.source.file())
        }));
        self.memo.index = Some(index.clone());
        Some(index)
    }

    fn sync_rows(&mut self, index: &Rc<TreeIndex>, query: &SharedString, cx: &mut Context<Self>) {
        let filtering = !query.trim().is_empty();
        let key = RowsKey {
            tree: Arc::as_ptr(&index.tree),
            your_code: self.your_code,
            query: if filtering {
                query.clone()
            } else {
                SharedString::default()
            },
        };
        let slot = if filtering {
            &mut self.memo.filtered
        } else {
            &mut self.memo.all
        };
        if slot.as_ref().is_some_and(|(built, _)| *built == key) {
            return;
        }
        self.memo.builds.rows += 1;
        let rows = RowSet::build(index, self.your_code, query);
        let nodes = rows.nodes.clone();
        *slot = Some((key, Rc::new(rows)));
        let expand_once = !filtering && !self.expanded_once;
        self.expanded_once |= expand_once;
        let target = if filtering {
            &self.filtered
        } else {
            &self.tree
        };
        target.update(cx, |tree, cx| {
            tree.set_nodes(nodes, cx);
            if filtering {
                tree.model_mut().expand_to_depth(u16::MAX);
            } else if expand_once {
                tree.model_mut().expand_to_depth(INITIAL_DEPTH);
            }
        });
    }

    /// Follows the shared selection (revealing it in the tree when it came
    /// from elsewhere) and rebuilds the detail model.
    fn sync_selection(
        &mut self,
        index: Option<&Rc<TreeIndex>>,
        selected_key: Option<ElementKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = match (self.selected, selected_key) {
            (Some(node), Some(key)) if node.anchor() == Some(key) => Some(node),
            (_, key) => key.map(NodeKey::Element),
        };
        self.reveal_selection(index, cx);

        let Some(node) = self.selected else {
            self.selection = None;
            return;
        };
        let found = index.and_then(|index| Some((index.clone(), index.find(node)?)));
        let shown_frame = self.shown.as_ref().map_or(0, |shown| shown.frame);
        let (index, ix, seen_in, missing) = match found {
            Some((index, ix)) => {
                self.last_seen = Some((node, index.clone(), ix, shown_frame));
                (index, ix, shown_frame, false)
            }
            None => match &self.last_seen {
                Some((seen, index, ix, frame)) if *seen == node => {
                    (index.clone(), *ix, *frame, true)
                }
                _ => {
                    self.selection = None;
                    return;
                }
            },
        };
        let Some(capture) = window.inspector_capture() else {
            return;
        };
        let live = self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.past.is_none());
        let mut selection = selection_model(capture, &self.stats(), node, index, ix, missing, live);
        selection.seen_in = seen_in;
        self.selection = Some(selection);
    }

    /// Makes the tree on screen select (and show) the selected row.
    fn reveal_selection(&mut self, index: Option<&Rc<TreeIndex>>, cx: &mut Context<Self>) {
        let tree = self.active_tree().clone();
        let selected = self.selected;
        tree.update(cx, |tree, cx| {
            if tree.model().selected() == selected.as_ref() {
                return;
            }
            let Some(node) = selected else {
                tree.model_mut().select(None);
                cx.notify();
                return;
            };
            if tree.model().node_ix(&node).is_some() {
                tree.reveal(&node, cx);
                return;
            }
            // Hidden (by "your code" or the filter): show its nearest shown
            // ancestor, but keep the selection on the element itself.
            let ancestor = index.and_then(|index| {
                let ix = index.find(node)?;
                index
                    .tree
                    .ancestors(ix)
                    .filter_map(|ancestor| index.key(ancestor))
                    .find(|key| tree.model().node_ix(key).is_some())
            });
            if let Some(ancestor) = ancestor {
                tree.reveal(&ancestor, cx);
            }
            tree.model_mut().select(Some(node));
            cx.notify();
        });
    }

    fn on_tree_event(
        &mut self,
        _: &Entity<TreeState<NodeKey>>,
        event: &TreeEvent<NodeKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TreeEvent::Selected(node) => self.select_node(*node, window, cx),
            TreeEvent::Activated(node) => {
                self.select_node(*node, window, cx);
                self.open_source(cx);
            }
            TreeEvent::Hovered(node) => self.hover_node(*node, window, cx),
        }
    }

    /// Selects a row: the shared state gets its key (or its anchor's).
    fn select_node(&mut self, node: NodeKey, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(node);
        self.state
            .update(cx, |state, cx| state.select_element(node.anchor(), cx));
        self.sync(window, cx);
    }

    /// Selects an element by key and shows it in the tree.
    pub(super) fn select_key(
        &mut self,
        key: ElementKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_node(NodeKey::Element(key), window, cx);
    }

    /// Shows the hovered row's box in the app: the element's own box model
    /// overlay, or for elements without a key a plain highlight.
    fn hover_node(&mut self, node: Option<NodeKey>, window: &mut Window, cx: &mut Context<Self>) {
        let highlight = node
            .filter(|node| node.element().is_none())
            .and_then(|node| {
                let index = self.memo.index.as_ref()?;
                let ix = index.find(node)?;
                let record = index.tree.get(ix)?;
                let theme = Theme::of(window, cx);
                Some(OverlayHighlight {
                    bounds: record.bounds,
                    color: theme.colors.box_content,
                    label: Some(index.info(ix)?.label.clone()),
                })
            });
        let Some(capture) = window.inspector_capture_mut() else {
            return;
        };
        let overlay = capture.overlay_mut();
        overlay.hovered = node.and_then(NodeKey::element);
        if let Some(previous) = self.hover_highlight.take()
            && let Some(position) = overlay.highlights.iter().position(|highlight| {
                highlight.bounds == previous.bounds && highlight.label == previous.label
            })
        {
            overlay.highlights.remove(position);
        }
        if let Some(highlight) = highlight {
            overlay.highlights.push(highlight.clone());
            self.hover_highlight = Some(highlight);
        }
        // The overlay is painted with the next frame.
        cx.notify();
    }

    fn toggle_your_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.your_code = !self.your_code;
        self.sync(window, cx);
    }

    fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&section) {
            self.collapsed.insert(section);
        }
        cx.notify();
    }

    /// Escape: clears the filter (anywhere in the lens).
    fn clear_filter(&mut self, cx: &mut Context<Self>) {
        self.filter.update(cx, |filter, cx| filter.emplace("", cx));
    }

    fn back_to_live(&mut self, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, cx| state.select_frame(None, cx));
    }

    /// The path edits go to: the selection's, when it is a keyed element of
    /// the live tree.
    fn editable_path(&self) -> Option<PathKey> {
        let selection = self
            .selection
            .as_ref()
            .filter(|selection| selection.editable)?;
        Some(selection.node.element()?.path)
    }

    /// Sets a style property of the selection's override.
    fn edit(
        &mut self,
        pointer: &str,
        value: PropertyValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.change_override(window, cx, |style| style_grid::set(style, pointer, &value));
    }

    /// Unsets a property of the selection's override.
    fn revert_property(&mut self, pointer: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.change_override(window, cx, |style| style_grid::remove(style, pointer));
    }

    /// Drops the selection's override.
    fn revert_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.change_override(window, cx, |_| Ok(StyleRefinement::default()));
    }

    fn change_override(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&StyleRefinement) -> Result<StyleRefinement, StyleEditError>,
    ) {
        let Some(path) = self.editable_path() else {
            return;
        };
        let Some(capture) = window.inspector_capture_mut() else {
            return;
        };
        let current = capture.overrides().get(&path).cloned().unwrap_or_default();
        match change(&current) {
            Ok(style) => {
                let empty = style_grid::rows(&style).is_empty();
                capture.set_override(path, (!empty).then_some(style));
                self.edit_error = None;
            }
            Err(error) => self.edit_error = Some(error.to_string().into()),
        }
        self.sync(window, cx);
    }

    /// Forces (or stops forcing) `states` on the selection.
    fn toggle_forced(&mut self, states: ForcedStates, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.editable_path() else {
            return;
        };
        if let Some(capture) = window.inspector_capture_mut() {
            let forced = capture.forced_states(path) ^ states;
            capture.set_forced_states(path, forced);
        }
        self.sync(window, cx);
    }

    /// The absolute path of the selection's source file.
    fn source_path(&self) -> Option<(String, &'static Location<'static>)> {
        let location = self.selection.as_ref()?.source?;
        let cwd = std::env::current_dir().unwrap_or_default();
        let path = source::resolve_source_path(location.file(), &cwd, |path| path.exists());
        Some((path.to_string_lossy().into_owned(), location))
    }

    /// Opens the selection's construction site in the configured editor.
    fn open_source(&mut self, cx: &mut Context<Self>) {
        let Some((path, location)) = self.source_path() else {
            return;
        };
        let url = LoupeSettings::get(cx)
            .editor
            .url(&path, location.line(), location.column());
        cx.open_url(&url);
        #[cfg(test)]
        self.opened_urls.push(url);
    }

    /// Copies `path:line:column` of the selection's construction site.
    fn copy_source(&mut self, cx: &mut Context<Self>) {
        if let Some((path, location)) = self.source_path() {
            let text = format!("{path}:{}:{}", location.line(), location.column());
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn resize_split(&mut self, size: Pixels, cx: &mut Context<Self>) {
        if self.split != Some(size) {
            self.split = Some(size);
            cx.notify();
        }
    }
}

/// Builds the detail model of element `ix` of `index`.
fn selection_model(
    capture: &InspectorCapture,
    stats: &HashMap<EntityId, ViewStats>,
    node: NodeKey,
    index: Rc<TreeIndex>,
    ix: ElementIndex,
    missing: bool,
    live: bool,
) -> Selection {
    let tree = &index.tree;
    let record = &tree.elements[ix as usize];
    let key = node.element();
    let editable = live && !missing && key.is_some();
    let source = key
        .and_then(|key| capture.path_info(key.path))
        .map(|info| info.source);
    let owner: Option<(ElementKey, SharedString)> = tree
        .owning_view(ix)
        .filter(|&view| view != ix)
        .and_then(|view| {
            let record = tree.get(view)?;
            Some((record.key?, format::element_label(record).into()))
        });
    let style = key.filter(|_| editable).map(|key| {
        let base = capture
            .selected_style()
            .filter(|selected| selected.key == key)
            .map(|selected| (*selected.base).clone());
        let overrides = capture
            .overrides()
            .get(&key.path)
            .cloned()
            .unwrap_or_default();
        let instances = tree
            .elements
            .iter()
            .filter(|record| record.key.is_some_and(|other| other.path == key.path))
            .count();
        StyleModel::new(key.path, base, overrides, instances)
    });
    let forced = key.map_or(ForcedStates::empty(), |key| capture.forced_states(key.path));
    let app_frames = capture
        .frames()
        .iter()
        .filter(|frame| !frame.inspector_only)
        .count();
    let view = match record.kind {
        ElementKind::View { entity, .. } => Some(ViewCost {
            stats: stats.get(&entity).cloned().unwrap_or_default(),
            frames: app_frames,
            last: last_render(capture, stats.get(&entity)),
        }),
        _ => None,
    };
    let owner_cost = owner.as_ref().and_then(|(_, label)| {
        let view = tree.owning_view(ix)?;
        let ElementKind::View { entity, .. } = tree.get(view)?.kind else {
            return None;
        };
        Some((
            label.clone(),
            stats.get(&entity).cloned().unwrap_or_default(),
        ))
    });
    Selection {
        node,
        why: why_size::explain_size(tree, ix),
        index: index.clone(),
        ix,
        missing,
        seen_in: tree.frame,
        editable,
        source,
        owner,
        style,
        forced,
        cost: Cost {
            primitives: record.primitives,
            view,
            owner: owner_cost.filter(|_| !matches!(record.kind, ElementKind::View { .. })),
        },
    }
}

/// Why and when a view last rendered.
fn last_render(
    capture: &InspectorCapture,
    stats: Option<&ViewStats>,
) -> Option<(String, u64, String)> {
    let frame = capture.frame(stats?.last_rendered?)?;
    let cause = frame
        .causes
        .iter()
        .find(|cause| !cause.from_inspector)
        .map(|cause| cause_summary(&cause.kind))
        .unwrap_or_else(|| "no recorded cause".into());
    let now = capture
        .latest_frame()
        .map_or(frame.start, |latest| latest.start);
    Some((cause, frame.id, format::relative_time(now, frame.start)))
}

impl Render for ElementsLens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let tree = self.render_tree_pane(theme, cx);
        let detail = self.render_detail_pane(theme, cx);
        let dock = window
            .inspector_bounds()
            .map_or(gpui::size(px(560.), px(700.)), |bounds| bounds.size);
        let (axis, default, min_first, min_second) = match LensLayout::of(window) {
            LensLayout::Stacked => (
                Axis::Vertical,
                ((dock.height - CHROME_HEIGHT) * 0.42).max(px(120.)),
                px(96.),
                px(160.),
            ),
            LensLayout::SideBySide => (
                Axis::Horizontal,
                (dock.width * 0.5).clamp(px(240.), px(480.)),
                px(200.),
                px(280.),
            ),
        };
        div()
            .size_full()
            // Loupe's views are cached and app refreshes skip them, so a
            // click that moves focus (into a text field, out of the tree)
            // must re-render the lens for the change to show and for a
            // newly focused field to receive typed text.
            .capture_any_mouse_down(cx.listener(|_, _, _, cx| cx.notify()))
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                if this.filtering() {
                    this.clear_filter(cx);
                } else {
                    cx.propagate();
                }
            }))
            .child(
                Split::new("elements-split", axis, self.split.unwrap_or(default))
                    .min_sizes(min_first, min_second)
                    .first(tree)
                    .second(detail)
                    .on_resize(
                        cx.listener(|this, size: &Pixels, _, cx| this.resize_split(*size, cx)),
                    ),
            )
    }
}

impl LensView for ElementsLens {
    fn rail_badge(&self, _: &Window, _: &App) -> Option<RailBadge> {
        self.badge.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn the_selected_frame_shows_its_tree_when_it_kept_one() {
        let (capture, _) = fixtures::inbox();
        let live = shown_tree(&capture, None).unwrap();
        assert!(live.past.is_none());
        assert!(Arc::ptr_eq(&live.tree, capture.latest_tree().unwrap()));

        let spike = capture
            .frames()
            .iter()
            .find(|frame| frame.tree.is_some())
            .unwrap();
        let past = shown_tree(&capture, Some(spike.id)).unwrap();
        let frame = past.past.expect("an older frame with a tree");
        assert_eq!(frame.frame, spike.id);
        assert!(frame.ago.ends_with("ago"), "{}", frame.ago);
        assert_eq!(past.tree.frame, spike.tree.as_ref().unwrap().frame);

        let treeless = capture
            .frames()
            .iter()
            .find(|frame| frame.tree.is_none())
            .unwrap();
        let fallback = shown_tree(&capture, Some(treeless.id)).unwrap();
        assert!(
            fallback.past.is_none(),
            "frames without a tree show the latest"
        );
    }
}
