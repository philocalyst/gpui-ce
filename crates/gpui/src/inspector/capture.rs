//! The inspector's always-on capture: rings of recent frames and input.
//!
//! A window owns an [`InspectorCapture`] only while its inspector is open, so
//! every recording hook costs a single `Option` check when it is closed.

use super::model::*;
use crate::{Bounds, Hsla, Pixels, SharedString, Size, StyleRefinement, px};
use collections::FxHashMap;
use scheduler::Instant;
use std::{collections::VecDeque, panic::Location, rc::Rc, sync::Arc, time::Duration};

/// Where the inspector UI docks inside its window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InspectorDock {
    /// A column on the right edge, `width` wide.
    Right {
        /// Column width.
        width: Pixels,
    },
    /// A row along the bottom edge, `height` tall.
    Bottom {
        /// Row height.
        height: Pixels,
    },
}

impl Default for InspectorDock {
    fn default() -> Self {
        InspectorDock::Right { width: px(560.) }
    }
}

impl InspectorDock {
    /// The dock is never narrower (or shorter) than this, unless the window
    /// is too small to also give the app [`Self::MIN_APP_EXTENT`].
    pub const MIN_DOCK_EXTENT: Pixels = px(240.);
    /// The app keeps at least this much width (or height) next to the dock.
    pub const MIN_APP_EXTENT: Pixels = px(120.);

    /// Splits a viewport into the app's area and the dock's area. The dock's
    /// requested extent is clamped so both stay usable.
    pub fn split(&self, viewport: Size<Pixels>) -> (Bounds<Pixels>, Bounds<Pixels>) {
        match *self {
            InspectorDock::Right { width } => {
                let width = Self::clamp_extent(width, viewport.width);
                let app = Bounds::new(
                    crate::point(px(0.), px(0.)),
                    crate::size(viewport.width - width, viewport.height),
                );
                let dock = Bounds::new(
                    crate::point(viewport.width - width, px(0.)),
                    crate::size(width, viewport.height),
                );
                (app, dock)
            }
            InspectorDock::Bottom { height } => {
                let height = Self::clamp_extent(height, viewport.height);
                let app = Bounds::new(
                    crate::point(px(0.), px(0.)),
                    crate::size(viewport.width, viewport.height - height),
                );
                let dock = Bounds::new(
                    crate::point(px(0.), viewport.height - height),
                    crate::size(viewport.width, height),
                );
                (app, dock)
            }
        }
    }

    fn clamp_extent(requested: Pixels, available: Pixels) -> Pixels {
        let max = (available - Self::MIN_APP_EXTENT).max(px(0.));
        requested.max(Self::MIN_DOCK_EXTENT).min(max)
    }
}

bitflags::bitflags! {
    /// Overlays painted on top of the app, above everything else in the window.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct OverlayModes: u32 {
        /// Outline every element, hue by depth.
        const OUTLINES = 1 << 0;
        /// Flash the bounds of views that re-rendered.
        const PAINT_FLASH = 1 << 1;
        /// Shade every hitbox; opaque hitboxes darker.
        const HITBOXES = 1 << 2;
        /// Red border around the window on frames over budget.
        const SLOW_FRAMES = 1 << 3;
        /// Stripe elements that overflow their parent.
        const OVERFLOW = 1 << 4;
        /// Show the box model (margin/padding/content) for hovered and selected elements.
        const BOX_MODEL = 1 << 5;
    }
}

/// A UI-requested highlight, painted by the overlay pass.
#[derive(Clone, Debug)]
pub struct OverlayHighlight {
    /// Region, in window coordinates.
    pub bounds: Bounds<Pixels>,
    /// Fill (the border uses the same hue, opaque).
    pub color: Hsla,
    /// Optional label chip.
    pub label: Option<SharedString>,
}

/// What the overlay pass paints this frame.
#[derive(Clone, Debug)]
pub struct OverlayState {
    /// Enabled modes.
    pub modes: OverlayModes,
    /// Element under the pointer while picking, or hovered in the inspector tree.
    pub hovered: Option<ElementKey>,
    /// The selected element; its outline persists after picking.
    pub selected: Option<ElementKey>,
    /// Extra highlights requested by the UI (findings, flame hover...).
    pub highlights: Vec<OverlayHighlight>,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            modes: OverlayModes::BOX_MODEL,
            hovered: None,
            selected: None,
            highlights: Vec::new(),
        }
    }
}

/// How much the capture records per frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CaptureLevel {
    /// Timings, causes, views and scene stats only.
    Frames,
    /// Plus element trees with bounds, kinds and flags.
    Tree,
    /// Plus [`ElementDetails`] for every element.
    Full,
}

/// Capture tuning.
#[derive(Clone, Debug)]
pub struct CaptureConfig {
    /// Frames kept in the ring.
    pub frame_capacity: usize,
    /// Input records kept in the ring.
    pub input_capacity: usize,
    /// Most recent frames whose element trees are kept.
    pub recent_trees: usize,
    /// Slow frames whose element trees are kept in addition to the recent ones.
    pub slow_trees: usize,
    /// What to record.
    pub level: CaptureLevel,
    /// Frame budget used to grade frames (and to decide which trees are "slow").
    pub budget: Duration,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            frame_capacity: 240,
            input_capacity: 1000,
            recent_trees: 4,
            slow_trees: 8,
            level: CaptureLevel::Full,
            budget: Duration::from_micros(16_667),
        }
    }
}

/// Picking state, driven by the window's mouse handling while picking.
#[derive(Clone, Debug, Default)]
pub struct PickState {
    /// Whether the next click in the app selects instead of interacting.
    pub active: bool,
    /// How many ancestors above the deepest element under the pointer to target.
    pub depth: usize,
    /// Pointer position of the last pick move.
    pub position: Option<crate::Point<Pixels>>,
}

/// Per-entity notify bookkeeping.
#[derive(Clone, Debug, Default)]
pub struct NotifyStats {
    /// Total notifies since capture started.
    pub total: u64,
    /// Notifies per 500 ms bucket, most recent last (up to 120 buckets = 60 s).
    pub buckets: VecDeque<u32>,
    /// Most recent call site.
    pub last_site: Option<&'static Location<'static>>,
}

/// The base style of the selected element, as it was before overrides.
#[derive(Clone, Debug)]
pub struct SelectedStyle {
    /// The element it belongs to.
    pub key: ElementKey,
    /// The element's own style refinement, as written in code.
    pub base: Box<StyleRefinement>,
}

/// The recording for one window.
pub struct InspectorCapture {
    pub(crate) epoch: Instant,
    pub(crate) config: CaptureConfig,
    pub(crate) frames: VecDeque<FrameRecord>,
    pub(crate) input: VecDeque<InputRecord>,
    pub(crate) next_frame_id: u64,
    pub(crate) next_input_seq: u64,
    pub(crate) paths: FxHashMap<Rc<crate::InspectorElementPath>, PathKey>,
    pub(crate) path_infos: Vec<PathInfo>,
    pub(crate) notify_stats: FxHashMap<crate::EntityId, NotifyStats>,
    pub(crate) frozen: bool,
    /// Set for captures installed by tests: their rings hold fixture data,
    /// which live frames and input must not be mixed into.
    pub(crate) replaying: bool,
    /// The entities a replaying capture reports instead of the live registry.
    pub(crate) replay_entities: Vec<EntityInfo>,
    pub(crate) generation: u64,
    pub(crate) overlay: OverlayState,
    pub(crate) pick: PickState,
    pub(crate) dock: InspectorDock,
    pub(crate) overrides: FxHashMap<PathKey, StyleRefinement>,
    pub(crate) forced_states: FxHashMap<PathKey, ForcedStates>,
    pub(crate) selected_style: Option<SelectedStyle>,
    pub(crate) recorder: super::recorder::Recorder,
}

impl InspectorCapture {
    pub(crate) fn new(dock: InspectorDock) -> Self {
        Self {
            epoch: Instant::now(),
            config: CaptureConfig::default(),
            frames: VecDeque::new(),
            input: VecDeque::new(),
            next_frame_id: 0,
            next_input_seq: 0,
            paths: FxHashMap::default(),
            path_infos: Vec::new(),
            notify_stats: FxHashMap::default(),
            frozen: false,
            replaying: false,
            replay_entities: Vec::new(),
            generation: 0,
            overlay: OverlayState::default(),
            pick: PickState::default(),
            dock,
            overrides: FxHashMap::default(),
            forced_states: FxHashMap::default(),
            selected_style: None,
            recorder: Default::default(),
        }
    }

    /// Interns an element path, returning the same key for the same path for
    /// the lifetime of the capture.
    pub(crate) fn intern(&mut self, path: &crate::InspectorElementPath) -> PathKey {
        if let Some(&key) = self.paths.get(path) {
            return key;
        }
        self.insert_path(Rc::new(path.clone()))
    }

    /// Interns an element path, also returning the capture's shared copy of
    /// it, so elements drawn at the same site share one allocation.
    pub(crate) fn intern_shared(
        &mut self,
        path: crate::InspectorElementPath,
    ) -> (Rc<crate::InspectorElementPath>, PathKey) {
        if let Some((shared, &key)) = self.paths.get_key_value(&path) {
            return (shared.clone(), key);
        }
        let shared = Rc::new(path);
        let key = self.insert_path(shared.clone());
        (shared, key)
    }

    fn insert_path(&mut self, path: Rc<crate::InspectorElementPath>) -> PathKey {
        let key = PathKey(self.path_infos.len() as u32);
        self.path_infos.push(PathInfo {
            source: path.source_location,
            scope: format!("{:?}", path.global_id).into(),
        });
        self.paths.insert(path, key);
        key
    }

    /// Appends a finished frame, assigning its id, trimming the ring and
    /// dropping element trees that are neither recent nor slow.
    pub(crate) fn record_frame(&mut self, mut frame: FrameRecord) -> u64 {
        frame.id = self.next_frame_id;
        self.next_frame_id += 1;
        let id = frame.id;
        if !frame.inspector_only {
            self.generation += 1;
        }
        for record in self.input.iter_mut().rev() {
            if record.seq < frame.input.start {
                break;
            }
            if frame.input.contains(&record.seq) {
                record.frame = Some(id);
            }
        }
        self.frames.push_back(frame);
        while self.frames.len() > self.config.frame_capacity {
            self.frames.pop_front();
        }
        self.trim_trees();
        id
    }

    /// Keeps trees for the `recent_trees` newest frames that have one, plus the
    /// `slow_trees` slowest frames over budget.
    fn trim_trees(&mut self) {
        let budget = self.config.budget;
        let mut with_trees: Vec<usize> = (0..self.frames.len())
            .filter(|&ix| self.frames[ix].tree.is_some())
            .collect();
        let keep_recent = with_trees.len().saturating_sub(self.config.recent_trees);
        let older = &mut with_trees[..keep_recent];
        older.sort_by_key(|&ix| std::cmp::Reverse(self.frames[ix].timings.app_total()));
        for (rank, &ix) in older.iter().enumerate() {
            let frame = &self.frames[ix];
            let slow = frame.timings.app_total() > budget;
            if !(slow && rank < self.config.slow_trees) {
                self.frames[ix].tree = None;
            }
        }
    }

    /// Appends an input record, assigning its sequence number.
    pub(crate) fn record_input(&mut self, mut record: InputRecord) -> u64 {
        record.seq = self.next_input_seq;
        self.next_input_seq += 1;
        let seq = record.seq;
        if !record.inspector {
            self.generation += 1;
        }
        self.input.push_back(record);
        while self.input.len() > self.config.input_capacity {
            self.input.pop_front();
        }
        seq
    }

    /// Records fabricated frames and input, for tests of code that reads a capture.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new_for_test() -> Self {
        Self::new(InspectorDock::default())
    }

    /// Appends a fabricated frame, as if the window had drawn it. Returns its id.
    #[cfg(any(test, feature = "test-support"))]
    pub fn push_frame_for_test(&mut self, frame: FrameRecord) -> u64 {
        self.record_frame(frame)
    }

    /// Appends a fabricated input record. Returns its sequence number.
    #[cfg(any(test, feature = "test-support"))]
    pub fn push_input_for_test(&mut self, record: InputRecord) -> u64 {
        self.record_input(record)
    }

    /// Sets the entities the window reports while this capture is installed
    /// with `Window::replace_inspector_capture_for_test`, in place of the
    /// live registry (whose ids would not match the fixture's).
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_entities_for_test(&mut self, entities: Vec<EntityInfo>) {
        self.replay_entities = entities;
    }

    /// Interns a fabricated path for a construction site.
    #[cfg(any(test, feature = "test-support"))]
    #[track_caller]
    pub fn intern_path_for_test(&mut self, scope: &str) -> PathKey {
        let key = PathKey(self.path_infos.len() as u32);
        self.path_infos.push(PathInfo {
            source: Location::caller(),
            scope: SharedString::from(scope.to_string()),
        });
        key
    }

    /// Sets fabricated notify statistics for an entity.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_notify_stats_for_test(&mut self, entity: crate::EntityId, stats: NotifyStats) {
        self.notify_stats.insert(entity, stats);
    }

    /// Recorded frames, oldest first.
    pub fn frames(&self) -> &VecDeque<FrameRecord> {
        &self.frames
    }

    /// The frame with the given id, if still in the ring.
    pub fn frame(&self, id: u64) -> Option<&FrameRecord> {
        let first = self.frames.front()?.id;
        let ix = id.checked_sub(first)? as usize;
        self.frames.get(ix).filter(|frame| frame.id == id)
    }

    /// The most recent frame that the app (not only the inspector) caused.
    pub fn latest_app_frame(&self) -> Option<&FrameRecord> {
        self.frames.iter().rev().find(|frame| !frame.inspector_only)
    }

    /// The most recent frame.
    pub fn latest_frame(&self) -> Option<&FrameRecord> {
        self.frames.back()
    }

    /// The most recent retained element tree.
    pub fn latest_tree(&self) -> Option<&Arc<ElementTree>> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.tree.as_ref())
    }

    /// Recorded input, oldest first.
    pub fn input(&self) -> &VecDeque<InputRecord> {
        &self.input
    }

    /// Source and scope of an interned path.
    pub fn path_info(&self, key: PathKey) -> Option<&PathInfo> {
        self.path_infos.get(key.0 as usize)
    }

    /// Notify statistics per entity.
    pub fn notify_stats(&self) -> &FxHashMap<crate::EntityId, NotifyStats> {
        &self.notify_stats
    }

    /// Time origin of every `Duration` offset in the capture.
    pub fn epoch(&self) -> Instant {
        self.epoch
    }

    /// Bumped whenever new app data lands (a frame or input record). The UI
    /// polls this to decide whether to re-render, instead of observing frames.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Current configuration.
    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }

    /// Mutable configuration.
    pub fn config_mut(&mut self) -> &mut CaptureConfig {
        &mut self.config
    }

    /// While frozen the rings stop updating; the app keeps running.
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Whether live frames and input are being added to the rings: not
    /// frozen, and not replaying fixture data installed by a test.
    pub(crate) fn is_recording(&self) -> bool {
        !self.frozen && !self.replaying
    }

    /// Freezes or resumes recording.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// Overlay settings.
    pub fn overlay(&self) -> &OverlayState {
        &self.overlay
    }

    /// Mutable overlay settings. The window repaints overlays on its next frame.
    pub fn overlay_mut(&mut self) -> &mut OverlayState {
        &mut self.overlay
    }

    /// Picking state.
    pub fn pick(&self) -> &PickState {
        &self.pick
    }

    /// Where the inspector docks.
    pub fn dock(&self) -> InspectorDock {
        self.dock
    }

    /// Moves or resizes the dock. The window lays the app out in the
    /// remaining space on its next frame.
    pub fn set_dock(&mut self, dock: InspectorDock) {
        self.dock = dock;
    }

    /// Live style overrides, keyed by element path. Applied on top of the
    /// element's own style every frame while the inspector is open.
    pub fn overrides(&self) -> &FxHashMap<PathKey, StyleRefinement> {
        &self.overrides
    }

    /// Sets (or with `None`, reverts) the style override for a path.
    pub fn set_override(&mut self, path: PathKey, style: Option<StyleRefinement>) {
        match style {
            Some(style) => self.overrides.insert(path, style),
            None => self.overrides.remove(&path),
        };
        self.recorder.style_inputs_changed = true;
    }

    /// States forced on the element at `path`.
    pub fn forced_states(&self, path: PathKey) -> ForcedStates {
        self.forced_states.get(&path).copied().unwrap_or_default()
    }

    /// Forces interaction states on the element at `path`.
    pub fn set_forced_states(&mut self, path: PathKey, states: ForcedStates) {
        if states.is_empty() {
            self.forced_states.remove(&path);
        } else {
            self.forced_states.insert(path, states);
        }
        self.recorder.style_inputs_changed = true;
    }

    /// The selected element's own style, from the latest frame that drew it.
    pub fn selected_style(&self) -> Option<&SelectedStyle> {
        self.selected_style.as_ref()
    }

    /// Clears the rings (not the path interner, so keys stay valid).
    pub fn clear(&mut self) {
        self.frames.clear();
        self.input.clear();
        self.notify_stats.clear();
        self.generation += 1;
    }

    /// Approximate heap bytes retained by the capture, for self-accounting.
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        let trees: usize = self
            .frames
            .iter()
            .filter_map(|frame| frame.tree.as_ref())
            .map(|tree| {
                tree.elements.len() * size_of::<ElementRecord>()
                    + tree
                        .elements
                        .iter()
                        .filter(|record| record.details.is_some())
                        .count()
                        * size_of::<ElementDetails>()
                    + tree.children.len() * size_of::<ElementIndex>()
            })
            .sum();
        let frames: usize = self
            .frames
            .iter()
            .map(|frame| {
                size_of::<FrameRecord>()
                    + frame.views.len() * size_of::<ViewSpan>()
                    + frame.causes.len() * size_of::<RenderCause>()
                    + frame.spans.len() * size_of::<UserSpan>()
            })
            .sum();
        trees
            + frames
            + self.input.len() * size_of::<InputRecord>()
            + self.path_infos.len() * size_of::<PathInfo>()
            + self.recorder.retained_bytes()
    }
}

impl ElementTree {
    /// Children of a record, in order.
    pub fn children(&self, ix: ElementIndex) -> &[ElementIndex] {
        let ix = ix as usize;
        match (self.children_start.get(ix), self.children_start.get(ix + 1)) {
            (Some(&start), Some(&end)) => &self.children[start as usize..end as usize],
            _ => &[],
        }
    }

    /// The record at `ix`.
    pub fn get(&self, ix: ElementIndex) -> Option<&ElementRecord> {
        self.elements.get(ix as usize)
    }

    /// Finds the record with the given key.
    pub fn find(&self, key: ElementKey) -> Option<ElementIndex> {
        self.elements
            .iter()
            .position(|record| record.key == Some(key))
            .map(|ix| ix as ElementIndex)
    }

    /// Ancestors of `ix`, nearest first (excluding `ix`).
    pub fn ancestors(&self, ix: ElementIndex) -> impl Iterator<Item = ElementIndex> + '_ {
        std::iter::successors(self.get(ix).and_then(|record| record.parent), |&parent| {
            self.get(parent).and_then(|record| record.parent)
        })
    }

    /// The nearest enclosing view of `ix` (including `ix` itself).
    pub fn owning_view(&self, ix: ElementIndex) -> Option<ElementIndex> {
        std::iter::once(ix)
            .chain(self.ancestors(ix))
            .find(|&ix| matches!(self.elements[ix as usize].kind, ElementKind::View { .. }))
    }

    /// Elements whose visible bounds contain `point`, topmost (latest painted) first.
    pub fn hit_test(&self, point: crate::Point<Pixels>) -> Vec<ElementIndex> {
        let mut hits: Vec<ElementIndex> = self
            .elements
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                record
                    .visible_bounds
                    .is_some_and(|bounds| bounds.contains(&point))
            })
            .map(|(ix, _)| ix as ElementIndex)
            .collect();
        hits.sort_by(|&a, &b| {
            let (a, b) = (&self.elements[a as usize], &self.elements[b as usize]);
            b.paint_order
                .cmp(&a.paint_order)
                .then(b.depth.cmp(&a.depth))
        });
        hits
    }

    /// Rebuilds [`Self::children`] and [`Self::children_start`] from parent links.
    pub fn rebuild_children(&mut self) {
        let len = self.elements.len();
        let mut counts = vec![0u32; len + 1];
        self.roots.clear();
        for (ix, record) in self.elements.iter().enumerate() {
            match record.parent {
                Some(parent) => counts[parent as usize] += 1,
                None => self.roots.push(ix as ElementIndex),
            }
        }
        self.children_start.clear();
        self.children_start.reserve(len + 1);
        let mut offset = 0;
        for count in &counts[..len] {
            self.children_start.push(offset);
            offset += count;
        }
        self.children_start.push(offset);
        let mut cursor = self.children_start.clone();
        self.children.clear();
        self.children.resize(offset as usize, 0);
        for (ix, record) in self.elements.iter().enumerate() {
            if let Some(parent) = record.parent {
                let slot = &mut cursor[parent as usize];
                self.children[*slot as usize] = ix as ElementIndex;
                *slot += 1;
            }
        }
    }
}
