//! The element tree as rows: stable identity, labels, "your code" and filtering.
//!
//! [`TreeIndex`] is built once per captured tree: a [`NodeKey`] and the
//! searchable, displayable facts of every element. [`RowSet::build`]
//! projects it into the rows a tree shows (preorder [`TreeNode`]s plus the
//! element each one is and why it matched), hiding library elements when
//! asked and keeping only matches and their ancestors while filtering. Both
//! are plain data, memoized by the lens and never rebuilt per render.

use crate::{
    analysis::{format, source},
    widgets::TreeNode,
};
use gpui::{
    EntityId, SharedString,
    inspector::{ElementIndex, ElementKey, ElementKind, ElementRecord, ElementTree, PathKey},
};
use std::{collections::HashMap, ops::Range, sync::Arc};

/// Text previews longer than this many characters end in `…`.
const PREVIEW_CHARS: usize = 40;

/// Identity of a tree row across frames.
///
/// Elements built with a source location have an [`ElementKey`]. The others
/// (text built from a plain string) are addressed by their nearest keyed
/// ancestor and the child positions leading to them, which is just as stable
/// while the surrounding structure stays the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum NodeKey {
    /// An element with a source location.
    Element(ElementKey),
    /// An element without one.
    Anonymous {
        /// Its nearest keyed ancestor.
        anchor: Option<ElementKey>,
        /// A hash of the child positions from the anchor down to it.
        path: u64,
    },
}

impl NodeKey {
    /// The element's own key, if it has one.
    pub fn element(self) -> Option<ElementKey> {
        match self {
            NodeKey::Element(key) => Some(key),
            NodeKey::Anonymous { .. } => None,
        }
    }

    /// The key that stands for this row wherever only [`ElementKey`]s are
    /// understood (selection, overlays): its own, or its nearest keyed
    /// ancestor's.
    pub fn anchor(self) -> Option<ElementKey> {
        match self {
            NodeKey::Element(key) => Some(key),
            NodeKey::Anonymous { anchor, .. } => anchor,
        }
    }
}

/// How a row is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowKind {
    /// A stateful view.
    View,
    /// A `RenderOnce` component.
    Component,
    /// Text: shown as a quoted preview.
    Text,
    /// Any other element.
    Element,
}

/// What an element shows in the tree and what a filter searches.
#[derive(Clone, Debug)]
pub(crate) struct RowInfo {
    /// Identity across frames.
    pub key: NodeKey,
    /// How it is drawn.
    pub kind: RowKind,
    /// `IssueList`, `div#close`, `"Hello"`.
    pub label: SharedString,
    /// Where `#id` starts in [`Self::label`] (to draw it dimmer).
    pub id_start: Option<usize>,
    /// The full text of text elements.
    pub text: Option<SharedString>,
    /// The file it was constructed in: `issue_list.rs`.
    pub file: Option<SharedString>,
    /// The element type without its module path: `Div`, `StyledText`.
    pub type_name: &'static str,
    /// `620×64`.
    pub size: SharedString,
    /// The view's entity, for views.
    pub entity: Option<EntityId>,
    /// Built by library code (gpui, a dependency) rather than the app.
    pub library: bool,
    /// Lowercased [`Self::label`], [`Self::text`], [`Self::file`] and
    /// [`Self::type_name`], in [`MatchField`] order, for filtering.
    search: [Option<SharedString>; 4],
}

/// Which searchable field of a row a filter matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MatchField {
    /// The label (`div#close`, `"Hello"`).
    Label,
    /// The full text of a text element.
    Text,
    /// The source file name.
    File,
    /// The element's type name.
    Type,
}

const MATCH_FIELDS: [MatchField; 4] = [
    MatchField::Label,
    MatchField::Text,
    MatchField::File,
    MatchField::Type,
];

/// Where a filter matched a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RowMatch {
    /// The field.
    pub field: MatchField,
    /// Byte range in that field, when it can be highlighted.
    pub range: Option<Range<usize>>,
}

/// Every element of one tree, with its row facts.
pub(crate) struct TreeIndex {
    /// The tree.
    pub tree: Arc<ElementTree>,
    /// Facts per element, by [`ElementIndex`].
    pub infos: Vec<RowInfo>,
    by_key: HashMap<NodeKey, ElementIndex>,
}

impl TreeIndex {
    /// Indexes `tree`; `source_file` resolves an element path to the file
    /// it was constructed in.
    pub fn build(
        tree: Arc<ElementTree>,
        source_file: impl Fn(PathKey) -> Option<&'static str>,
    ) -> Self {
        let mut infos: Vec<RowInfo> = Vec::with_capacity(tree.elements.len());
        let mut by_key = HashMap::with_capacity(tree.elements.len());
        // Records list parents before their children, so a parent's key and
        // classification are known when its children are reached.
        let mut ordinals: Vec<u32> = vec![0; tree.elements.len()];
        let mut labels = Labels::default();
        for (ix, record) in tree.elements.iter().enumerate() {
            let parent = record.parent.map(|parent| parent as usize);
            let key = match record.key {
                Some(key) => NodeKey::Element(key),
                None => {
                    let ordinal = parent.map_or(ix as u32, |parent| {
                        let ordinal = ordinals[parent];
                        ordinals[parent] += 1;
                        ordinal
                    });
                    anonymous_key(parent.and_then(|parent| infos.get(parent)), ordinal)
                }
            };
            if record.key.is_some()
                && let Some(parent) = parent
            {
                ordinals[parent] += 1;
            }
            let source = record.key.and_then(|key| source_file(key.path));
            let library = labels.is_library(&record.kind, source).unwrap_or_else(|| {
                parent
                    .and_then(|parent| infos.get(parent))
                    .is_some_and(|parent| parent.library)
            });
            let info = row_info(
                &tree,
                ix as ElementIndex,
                record,
                key,
                source,
                library,
                &mut labels,
            );
            by_key.insert(key, ix as ElementIndex);
            infos.push(info);
        }
        Self {
            tree,
            infos,
            by_key,
        }
    }

    /// The element shown by the row `key`.
    pub fn find(&self, key: NodeKey) -> Option<ElementIndex> {
        self.by_key.get(&key).copied()
    }

    /// The row key of element `ix`.
    pub fn key(&self, ix: ElementIndex) -> Option<NodeKey> {
        self.infos.get(ix as usize).map(|info| info.key)
    }

    /// The facts of element `ix`.
    pub fn info(&self, ix: ElementIndex) -> Option<&RowInfo> {
        self.infos.get(ix as usize)
    }
}

/// The key of an element without a source location: its parent's anchor
/// and the position path below it.
fn anonymous_key(parent: Option<&RowInfo>, ordinal: u32) -> NodeKey {
    let (anchor, parent_path) = match parent.map(|parent| parent.key) {
        Some(NodeKey::Element(key)) => (Some(key), 0),
        Some(NodeKey::Anonymous { anchor, path }) => (anchor, path),
        None => (None, 0),
    };
    // FxHash-style mixing: deterministic across runs and platforms.
    const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;
    let path = (parent_path.rotate_left(5) ^ u64::from(ordinal).wrapping_add(1)).wrapping_mul(SEED);
    NodeKey::Anonymous { anchor, path }
}

/// Strings shared by many elements of a tree (names of the same type,
/// files, sizes), built once per tree instead of once per element.
#[derive(Default)]
struct Labels {
    /// Label without `#id` and its lowercase, by element type.
    names: HashMap<&'static str, (SharedString, SharedString)>,
    /// Lowercased short type names.
    types: HashMap<&'static str, SharedString>,
    /// File name and its lowercase, by source path.
    files: HashMap<&'static str, (SharedString, SharedString)>,
    /// `620×64`, by width and height bits.
    sizes: HashMap<(u32, u32), SharedString>,
    /// Whether a source path is library code.
    library_paths: HashMap<&'static str, bool>,
}

impl Labels {
    fn is_library(&mut self, kind: &ElementKind, source: Option<&'static str>) -> Option<bool> {
        match (kind, source) {
            (ElementKind::Element { .. }, Some(path)) => Some(
                *self
                    .library_paths
                    .entry(path)
                    .or_insert_with(|| source::is_library_path(path)),
            ),
            _ => source::is_library_element(kind, source),
        }
    }

    fn name(&mut self, record: &ElementRecord) -> (SharedString, SharedString) {
        self.names
            .entry(record.kind.type_name())
            .or_insert_with(|| {
                let label = format::element_label(record);
                let id_len = record.id.as_ref().map_or(0, |id| id.len() + 1);
                let name = label[..label.len() - id_len].to_string();
                (name.to_lowercase().into(), name.into())
            })
            .clone()
    }

    fn type_lower(&mut self, type_name: &'static str) -> SharedString {
        self.types
            .entry(type_name)
            .or_insert_with(|| type_name.to_lowercase().into())
            .clone()
    }

    fn file(&mut self, path: &'static str) -> (SharedString, SharedString) {
        self.files
            .entry(path)
            .or_insert_with(|| {
                let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
                (file.to_string().into(), file.to_lowercase().into())
            })
            .clone()
    }

    fn size(&mut self, size: gpui::Size<gpui::Pixels>) -> SharedString {
        let bits = (
            f32::from(size.width).to_bits(),
            f32::from(size.height).to_bits(),
        );
        self.sizes
            .entry(bits)
            .or_insert_with(|| format::size(size).into())
            .clone()
    }
}

fn row_info(
    tree: &ElementTree,
    ix: ElementIndex,
    record: &ElementRecord,
    key: NodeKey,
    source: Option<&'static str>,
    library: bool,
    labels: &mut Labels,
) -> RowInfo {
    let text = record
        .details
        .as_deref()
        .and_then(|details| details.text.clone());
    let is_text = text.is_some()
        && matches!(record.kind, ElementKind::Element { .. })
        && tree.children(ix).is_empty();
    let (kind, entity) = match record.kind {
        ElementKind::View { entity, .. } => (RowKind::View, Some(entity)),
        ElementKind::Component { .. } => (RowKind::Component, None),
        ElementKind::Element { .. } if is_text => (RowKind::Text, None),
        ElementKind::Element { .. } => (RowKind::Element, None),
    };
    let (label, label_lower, id_start) = match (&text, kind) {
        (Some(text), RowKind::Text) => {
            let preview = text_preview(text);
            let lower = preview.to_lowercase();
            (preview.into(), lower.into(), None)
        }
        _ => {
            let (name_lower, name) = labels.name(record);
            match &record.id {
                Some(id) => (
                    SharedString::from(format!("{name}#{id}")),
                    SharedString::from(format!("{name_lower}#{}", id.to_lowercase())),
                    Some(name.len()),
                ),
                None => (name, name_lower, None),
            }
        }
    };
    let file = source.map(|path| labels.file(path));
    let type_name = record.kind.short_name();
    let search = [
        Some(label_lower),
        text.as_deref().map(|text| text.to_lowercase().into()),
        file.as_ref().map(|(_, lower)| lower.clone()),
        Some(labels.type_lower(type_name)),
    ];
    RowInfo {
        key,
        kind,
        label,
        id_start,
        text,
        file: file.map(|(file, _)| file),
        type_name,
        size: labels.size(record.bounds.size),
        entity,
        library,
        search,
    }
}

/// `"Hello"`, or `"The first forty characters…"` for long text, on one line.
pub(crate) fn text_preview(text: &str) -> String {
    let line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(PREVIEW_CHARS + 1)
        .collect();
    if line.chars().count() > PREVIEW_CHARS {
        let cut: String = line.chars().take(PREVIEW_CHARS).collect();
        format!("\"{}…\"", cut.trim_end())
    } else {
        format!("\"{line}\"")
    }
}

/// Where `query` (lowercased, non-empty) matches `info`, if anywhere.
fn match_row(info: &RowInfo, query: &str) -> Option<RowMatch> {
    MATCH_FIELDS
        .iter()
        .zip(&info.search)
        .find_map(|(&field, haystack)| {
            let haystack = haystack.as_deref()?;
            let start = haystack.find(query)?;
            // Lowercasing can change byte lengths outside ASCII; only
            // highlight when the original has the same layout.
            let original_len = match field {
                MatchField::Label => info.label.len(),
                MatchField::Text => info.text.as_ref().map_or(0, |text| text.len()),
                MatchField::File => info.file.as_ref().map_or(0, |file| file.len()),
                MatchField::Type => info.type_name.len(),
            };
            let range = (original_len == haystack.len()).then(|| start..start + query.len());
            Some(RowMatch { field, range })
        })
}

/// One visible row: which element, and why it matched a filter.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    /// The element.
    pub ix: ElementIndex,
    /// Where the filter matched it; `None` for ancestors kept for context
    /// (and for every row when not filtering).
    pub matched: Option<RowMatch>,
}

/// The rows a tree shows: preorder nodes and, in parallel, their elements.
#[derive(Clone, Debug, Default)]
pub(crate) struct RowSet {
    /// For the tree widget.
    pub nodes: Vec<TreeNode<NodeKey>>,
    /// `rows[i]` is the element of `nodes[i]`.
    pub rows: Vec<Row>,
    /// How many rows matched the filter (all rows when not filtering).
    pub matches: usize,
}

impl RowSet {
    /// The rows of `index`: without library elements when `your_code` (their
    /// children move up to the nearest shown ancestor), and when `query` is
    /// not blank only the elements matching it (in their label, text, file
    /// or type, ignoring case) and their ancestors.
    pub fn build(index: &TreeIndex, your_code: bool, query: &str) -> Self {
        let query = query.trim().to_lowercase();
        let tree = &index.tree;
        let shown = |ix: ElementIndex| !(your_code && index.infos[ix as usize].library);

        // Visible elements in preorder, each with its visible parent.
        let mut order: Vec<(ElementIndex, Option<usize>)> = Vec::with_capacity(tree.elements.len());
        let mut stack: Vec<(ElementIndex, Option<usize>)> =
            tree.roots.iter().rev().map(|&root| (root, None)).collect();
        while let Some((ix, parent)) = stack.pop() {
            let parent = if shown(ix) {
                order.push((ix, parent));
                Some(order.len() - 1)
            } else {
                parent
            };
            stack.extend(tree.children(ix).iter().rev().map(|&child| (child, parent)));
        }

        let matched: Vec<Option<RowMatch>> = if query.is_empty() {
            vec![None; order.len()]
        } else {
            order
                .iter()
                .map(|&(ix, _)| match_row(&index.infos[ix as usize], &query))
                .collect()
        };
        let keep: Vec<bool> = if query.is_empty() {
            vec![true; order.len()]
        } else {
            let mut keep = vec![false; order.len()];
            for position in (0..order.len()).filter(|&position| matched[position].is_some()) {
                let mut next = Some(position);
                while let Some(position) = next.filter(|&position| !keep[position]) {
                    keep[position] = true;
                    next = order[position].1;
                }
            }
            keep
        };

        let mut set = RowSet::default();
        let mut node_of: Vec<Option<usize>> = vec![None; order.len()];
        for (position, (&(ix, parent), matched)) in order.iter().zip(matched).enumerate() {
            if !keep[position] {
                continue;
            }
            let parent = parent.and_then(|parent| node_of[parent]);
            let depth = parent.map_or(0, |parent| set.nodes[parent].depth + 1);
            if let Some(parent) = parent {
                set.nodes[parent].has_children = true;
            }
            node_of[position] = Some(set.nodes.len());
            set.matches += usize::from(query.is_empty() || matched.is_some());
            set.nodes.push(TreeNode {
                key: index.infos[ix as usize].key,
                parent,
                depth,
                has_children: false,
            });
            set.rows.push(Row { ix, matched });
        }
        set
    }
}

/// How often each view rendered and was served from the cache, over the
/// recorded frames.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ViewStats {
    /// Frames in which `render()` ran.
    pub rendered: u32,
    /// Frames that reused its cached output.
    pub cached: u32,
    /// The latest frame that rendered it.
    pub last_rendered: Option<u64>,
}

/// [`ViewStats`] for every view in `frames` (app frames only).
pub(crate) fn view_stats<'a>(
    frames: impl IntoIterator<Item = &'a gpui::inspector::FrameRecord>,
) -> HashMap<EntityId, ViewStats> {
    let mut stats: HashMap<EntityId, ViewStats> = HashMap::new();
    for frame in frames.into_iter().filter(|frame| !frame.inspector_only) {
        for view in &frame.views {
            let entry = stats.entry(view.entity).or_default();
            match view.outcome {
                gpui::inspector::ViewOutcome::Rendered => {
                    entry.rendered += 1;
                    entry.last_rendered = Some(frame.id);
                }
                gpui::inspector::ViewOutcome::Cached => entry.cached += 1,
            }
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{ElementSpec, TreeBuilder};
    use gpui::inspector::InspectorCapture;
    use std::time::Instant;

    /// ```text
    /// Root (view)
    /// └─ div#list
    ///    ├─ div#row-1
    ///    │  └─ "Hello world"
    ///    └─ Badge (component)
    ///       └─ "3"
    /// ```
    fn sample(capture: &mut InspectorCapture) -> Arc<ElementTree> {
        let mut builder = TreeBuilder::new(capture);
        builder.open(ElementSpec::view("app::Root", 1).bounds(0., 0., 100., 100.));
        builder.open(ElementSpec::div().id("list").bounds(0., 0., 100., 40.));
        builder.open(ElementSpec::div().id("row-1").bounds(0., 0., 100., 20.));
        builder.leaf(ElementSpec::text("Hello world"));
        builder.close();
        builder.open(ElementSpec::component("app::Badge"));
        builder.leaf(ElementSpec::text("3"));
        builder.close();
        builder.close();
        builder.close();
        Arc::new(builder.build(1))
    }

    fn index(capture: &InspectorCapture, tree: Arc<ElementTree>) -> TreeIndex {
        TreeIndex::build(tree, |path| source_file(capture, path))
    }

    fn source_file(capture: &InspectorCapture, path: PathKey) -> Option<&'static str> {
        capture.path_info(path).map(|info| info.source.file())
    }

    fn labels(index: &TreeIndex, set: &RowSet) -> Vec<String> {
        set.nodes
            .iter()
            .zip(&set.rows)
            .map(|(node, row)| {
                format!(
                    "{}{}",
                    "  ".repeat(node.depth as usize),
                    index.infos[row.ix as usize].label
                )
            })
            .collect()
    }

    #[test]
    fn rows_list_every_element_in_preorder_with_readable_labels() {
        let mut capture = InspectorCapture::new_for_test();
        let tree = sample(&mut capture);
        let index = index(&capture, tree);
        let set = RowSet::build(&index, false, "");
        assert_eq!(
            labels(&index, &set),
            [
                "Root",
                "  div#list",
                "    div#row-1",
                "      \"Hello world\"",
                "    Badge",
                "      \"3\""
            ]
        );
        assert_eq!(set.matches, 6);
        let info = &index.infos[1];
        assert_eq!(info.id_start, Some(3), "`#list` starts after `div`");
        assert_eq!(info.size.as_ref(), "100×40");
        assert_eq!(
            info.file.as_deref(),
            Some("rows.rs"),
            "the builder's caller"
        );
        assert_eq!(index.infos[0].kind, RowKind::View);
        assert_eq!(index.infos[3].kind, RowKind::Text);
        assert!(set.nodes[2].has_children && !set.nodes[3].has_children);
        assert_eq!(set.nodes[4].parent, Some(1));
    }

    #[test]
    fn filtering_keeps_matches_and_their_ancestors_and_says_where_it_matched() {
        let mut capture = InspectorCapture::new_for_test();
        let tree = sample(&mut capture);
        let index = index(&capture, tree);
        let set = RowSet::build(&index, false, "  WORLD ");
        assert_eq!(
            labels(&index, &set),
            [
                "Root",
                "  div#list",
                "    div#row-1",
                "      \"Hello world\""
            ]
        );
        assert_eq!(set.matches, 1);
        assert_eq!(set.rows[0].matched, None, "ancestors are context");
        assert_eq!(
            set.rows[3].matched,
            Some(RowMatch {
                field: MatchField::Label,
                range: Some(7..12)
            })
        );

        let by_id = RowSet::build(&index, false, "#row");
        assert_eq!(by_id.matches, 1);
        assert_eq!(
            by_id.rows.last().unwrap().matched.as_ref().unwrap().range,
            Some(3..7)
        );
        let by_type = RowSet::build(&index, false, "styledtext");
        assert_eq!(by_type.matches, 2, "both texts are StyledText");
        assert_eq!(
            by_type.rows.last().unwrap().matched.as_ref().unwrap().field,
            MatchField::Type
        );
        let by_file = RowSet::build(&index, false, "rows.rs");
        assert_eq!(by_file.matches, 6);
        assert!(
            RowSet::build(&index, false, "nothing like it")
                .nodes
                .is_empty()
        );
    }

    #[test]
    fn long_text_is_previewed_on_one_line() {
        assert_eq!(text_preview("Hi"), "\"Hi\"");
        assert_eq!(text_preview("a\nb"), "\"a b\"");
        let long = "Scroll jank in the issue list when syncing hundreds of labels";
        assert_eq!(
            text_preview(long),
            "\"Scroll jank in the issue list when synci…\""
        );
        assert_eq!(text_preview(&"x".repeat(40)).chars().count(), 42);
    }

    #[test]
    fn anonymous_elements_get_keys_that_survive_new_frames() {
        let mut capture = InspectorCapture::new_for_test();
        let mut tree = (*sample(&mut capture)).clone();
        for ix in [3, 5] {
            tree.elements[ix].key = None;
        }
        let anchor_3 = tree.elements[2].key;
        let first = index(&capture, Arc::new(tree.clone()));
        let text = first.key(3).unwrap();
        assert_eq!(text.element(), None);
        assert_eq!(text.anchor(), anchor_3, "anchored to div#row-1");
        assert_ne!(first.key(5), Some(text));

        // The next frame's tree has the same shape: same keys, and they find
        // the same elements.
        let second = index(&capture, Arc::new(tree));
        assert_eq!(second.key(3), Some(text));
        assert_eq!(second.find(text), Some(3));
    }

    #[test]
    fn your_code_hides_library_elements_and_lifts_their_children() {
        let mut capture = InspectorCapture::new_for_test();
        let mut tree = (*sample(&mut capture)).clone();
        // Make div#list a gpui-internal element and the root a gpui view.
        tree.elements[0].kind = ElementKind::View {
            entity: EntityId::from(1),
            type_name: "gpui::window::Root",
        };
        let list_path = tree.elements[1].key.unwrap().path;
        let index = TreeIndex::build(Arc::new(tree), |path| {
            if path == list_path {
                Some("crates/gpui/src/elements/div.rs")
            } else {
                source_file(&capture, path)
            }
        });
        let set = RowSet::build(&index, true, "");
        assert_eq!(
            labels(&index, &set),
            ["div#row-1", "  \"Hello world\"", "Badge", "  \"3\""]
        );
        assert!(index.infos[1].library);
        assert!(!index.infos[3].library, "text follows its own parent");
    }

    #[test]
    fn view_stats_count_renders_and_cache_hits_of_app_frames() {
        use crate::fixtures::{FrameBuilder, ms};
        use gpui::inspector::ViewOutcome;
        let mut capture = InspectorCapture::new_for_test();
        for (ix, outcome) in [
            ViewOutcome::Rendered,
            ViewOutcome::Cached,
            ViewOutcome::Rendered,
        ]
        .into_iter()
        .enumerate()
        {
            FrameBuilder::new()
                .at(ms(ix as f64 * 16.))
                .view(7, "app::List", 0, ms(0.), ms(1.), outcome)
                .push(&mut capture);
        }
        FrameBuilder::new()
            .view(7, "app::List", 0, ms(0.), ms(1.), ViewOutcome::Rendered)
            .inspector_only()
            .push(&mut capture);
        let stats = view_stats(capture.frames());
        assert_eq!(
            stats[&EntityId::from(7)],
            ViewStats {
                rendered: 2,
                cached: 1,
                last_rendered: Some(2)
            }
        );
    }

    #[test]
    fn five_thousand_elements_index_and_filter_well_within_a_frame() {
        let mut capture = InspectorCapture::new_for_test();
        let tree = Arc::new(large_tree(&mut capture, 5_000));
        assert!(tree.elements.len() >= 5_000);

        let start = Instant::now();
        let index = index(&capture, tree);
        let indexed = start.elapsed();
        let start = Instant::now();
        let all = RowSet::build(&index, false, "");
        let rows = start.elapsed();
        let start = Instant::now();
        let filtered = RowSet::build(&index, false, "row-42");
        let filter = start.elapsed();
        assert_eq!(all.nodes.len(), index.infos.len());
        assert!(filtered.matches >= 1 && filtered.nodes.len() < 20);
        // Unoptimized test builds are several times slower than release;
        // these bounds still leave room within a 16 ms frame there.
        eprintln!("5k elements: index {indexed:?}, rows {rows:?}, filter {filter:?}");
        assert!(indexed.as_millis() < 40, "index took {indexed:?}");
        assert!(rows.as_millis() < 16, "rows took {rows:?}");
        assert!(filter.as_millis() < 16, "filter took {filter:?}");
    }

    /// A list of `rows` rows, each a div with an avatar, two texts and a
    /// button: about `rows × 5` elements.
    pub(crate) fn large_tree(capture: &mut InspectorCapture, elements: usize) -> ElementTree {
        let mut builder = TreeBuilder::new(capture);
        builder.open(ElementSpec::view("app::Root", 1).bounds(0., 0., 800., 600.));
        builder.open(ElementSpec::div().id("list").bounds(0., 0., 800., 600.));
        for row in 0..elements.div_ceil(5) {
            let y = row as f32 * 24.;
            builder.open(
                ElementSpec::div()
                    .id(format!("row-{row}"))
                    .bounds(0., y, 800., 24.),
            );
            builder.leaf(ElementSpec::component("app::Avatar").bounds(4., y, 16., 16.));
            builder.leaf(ElementSpec::text(format!("Sender {row}")).bounds(24., y, 200., 16.));
            builder.leaf(ElementSpec::text("Subject line").bounds(240., y, 300., 16.));
            builder.leaf(
                ElementSpec::div()
                    .id("star")
                    .clickable()
                    .bounds(780., y, 16., 16.),
            );
            builder.close();
        }
        builder.close();
        builder.close();
        builder.build(1)
    }
}
