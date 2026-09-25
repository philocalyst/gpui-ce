//! Turns one `Window::draw` into a [`FrameRecord`].
//!
//! The recorder lives inside the window's [`InspectorCapture`] and is only
//! reached through the window's inspector hooks, so a closed inspector costs
//! one `Option` check per hook. While a frame is drawn it builds the element
//! tree (records are opened in `request_layout`, measured in `prepaint`,
//! ordered and counted in `paint`), times the draw phases and view renders,
//! and splices in the records of cached views reused from the previous frame.

use super::{capture::CaptureLevel, causes::NotifyDelta, causes::PendingCause, model::*};
use crate::{
    Bounds, ElementId, EntityId, GlobalElementId, HitboxId, Pixels, Scene, SharedString, Size,
};
use collections::{FxHashMap, FxHashSet};
use scheduler::Instant;
use std::{mem, ops::Range, sync::Arc, time::Duration};

/// What the frame being drawn records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RecordMode {
    /// No frame is being recorded.
    #[default]
    Off,
    /// Timings, causes, views and element counts, but no tree.
    Count,
    /// The element tree.
    Tree,
    /// The element tree with [`ElementDetails`].
    Details,
}

impl RecordMode {
    /// The mode for a frame at `level`. The tree is also built at
    /// [`CaptureLevel::Frames`] when overlays or picking need it.
    pub(crate) fn for_level(level: CaptureLevel, needs_tree: bool) -> Self {
        match level {
            CaptureLevel::Full => RecordMode::Details,
            CaptureLevel::Tree => RecordMode::Tree,
            CaptureLevel::Frames if needs_tree => RecordMode::Tree,
            CaptureLevel::Frames => RecordMode::Count,
        }
    }

    pub(crate) fn builds_tree(self) -> bool {
        self >= RecordMode::Tree
    }
}

/// How the element being drawn is represented in the tree under construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordSlot {
    /// The element has its own record.
    Own(ElementIndex),
    /// A transparent wrapper (e.g. `AnyElement`): the record of the first
    /// element drawn inside it.
    Alias(ElementIndex),
}

impl RecordSlot {
    pub(crate) fn index(self) -> ElementIndex {
        match self {
            RecordSlot::Own(ix) | RecordSlot::Alias(ix) => ix,
        }
    }
}

/// The part of a frame being timed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Outside the phases below (bookkeeping, a11y, focus listeners...).
    #[default]
    Idle,
    /// The app root's `request_layout`, where views render.
    Render,
    /// Prepaint of every app root.
    Prepaint,
    /// Paint of every app root.
    Paint,
    /// The inspector's own root and overlays.
    Inspector,
}

impl Phase {
    const COUNT: usize = 5;

    fn index(self) -> usize {
        self as usize
    }
}

/// Wall time per phase, with layout and out-of-phase view renders tracked
/// separately so each lands in exactly one [`PhaseTimings`] field.
struct PhaseClock {
    phase: Phase,
    phase_start: Instant,
    wall: [Duration; Phase::COUNT],
    layout: [Duration; Phase::COUNT],
    render: [Duration; Phase::COUNT],
}

impl PhaseClock {
    fn new(now: Instant) -> Self {
        Self {
            phase: Phase::Idle,
            phase_start: now,
            wall: Default::default(),
            layout: Default::default(),
            render: Default::default(),
        }
    }

    fn switch(&mut self, phase: Phase, now: Instant) -> Phase {
        let previous = mem::replace(&mut self.phase, phase);
        self.wall[previous.index()] += now.saturating_duration_since(self.phase_start);
        self.phase_start = now;
        previous
    }

    fn timings(&self, total: Duration) -> PhaseTimings {
        use Phase::*;
        let own = |phase: Phase| {
            self.wall[phase.index()]
                .saturating_sub(self.layout[phase.index()])
                .saturating_sub(self.render[phase.index()])
        };
        PhaseTimings {
            input: Duration::ZERO,
            render: own(Render) + self.render[Prepaint.index()] + self.render[Paint.index()],
            layout: self.layout[Render.index()]
                + self.layout[Prepaint.index()]
                + self.layout[Paint.index()],
            prepaint: own(Prepaint),
            paint: own(Paint),
            inspector: self.wall[Inspector.index()],
            present: None,
            total,
        }
    }
}

/// A view render in progress.
struct OpenView {
    span: usize,
    started: Instant,
    outermost: bool,
}

/// A cached view's records from the frame that last rendered it, replayed
/// into later frames that reuse the view's paint.
struct CachedSubtree {
    records: Vec<StoredRecord>,
    /// Main-tree records (local indices) in paint order.
    paint_sequence: Vec<u32>,
    /// Records drawn through `deferred` (local indices) in paint order.
    deferred_paint_sequence: Vec<u32>,
    /// Drawn (rendered or reused) in the current frame.
    used: bool,
}

struct StoredRecord {
    record: ElementRecord,
    /// The record's parent; for deferred roots the element that deferred them.
    parent: StoredParent,
    deferred_root: bool,
}

#[derive(Clone, Copy)]
enum StoredParent {
    View,
    Local(u32),
}

/// A reused view's stored records, appended to this frame's tree.
struct Splice {
    view: ElementIndex,
    base: ElementIndex,
    global_id: GlobalElementId,
}

/// A view that re-rendered recently; the overlay flashes its bounds.
pub(crate) struct Flash {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) started: Instant,
}

/// How long a paint flash takes to fade out.
pub(crate) const FLASH_DURATION: Duration = Duration::from_millis(300);

/// Engine-side state of an [`super::InspectorCapture`]: the frame being
/// recorded plus what carries over between frames.
pub(crate) struct Recorder {
    pub(crate) mode: RecordMode,
    /// Set while the inspector draws its own root and overlays.
    pub(crate) suspended: bool,
    frame_start: Instant,
    clock: PhaseClock,

    // The tree under construction.
    elements: Vec<ElementRecord>,
    prepainted: Vec<bool>,
    stack: Vec<ElementIndex>,
    deferred_from: Vec<(ElementIndex, ElementIndex)>,
    fresh_cached_views: Vec<(GlobalElementId, ElementIndex)>,
    splices: Vec<Splice>,
    paint_counter: u32,
    element_count: u32,
    pub(crate) hitbox_owners: Vec<(HitboxId, ElementIndex)>,
    /// The previous frame's hitbox owners, kept for frames that replay it.
    previous_hitbox_owners: Vec<(HitboxId, ElementIndex)>,
    /// Scratch for finishing the tree, reused across frames.
    scratch: TreeScratch,

    // Everything else recorded for the frame.
    views: Vec<ViewSpan>,
    open_views: Vec<OpenView>,
    pub(crate) causes: Vec<RenderCause>,
    pub(crate) inspector_only: bool,
    inspector_scene: SceneStats,
    inspector_scene_mark: SceneStats,
    /// Hitboxes inserted by the inspector's own root this frame.
    pub(crate) inspector_hitboxes: Range<usize>,
    inspector_accessed: FxHashSet<EntityId>,

    // Carried across frames.
    /// Distinguishes frames for `path_instances`.
    frame_serial: u64,
    /// Per interned path: the frame it was last drawn in and its next instance.
    path_instances: Vec<(u64, u32)>,
    cache: FxHashMap<GlobalElementId, CachedSubtree>,
    cache_mode: RecordMode,
    pub(crate) view_types: FxHashMap<EntityId, &'static str>,
    pub(crate) inspector_entities: FxHashSet<EntityId>,
    element_labels: FxHashMap<ElementId, SharedString>,
    pub(crate) live_tree: Option<Arc<ElementTree>>,
    pub(crate) flashes: Vec<Flash>,
    pub(crate) last_viewport: Option<Size<Pixels>>,
    pub(crate) last_input_seq: u64,
    pub(crate) notify_bucket: u64,
    pub(crate) last_selected: Option<ElementKey>,
    pub(crate) style_inputs_changed: bool,
    /// The frame re-renders the app because the inspector's style inputs changed.
    pub(crate) restyling: bool,
    /// The recorded frame awaiting its present time.
    pub(crate) unpresented: Option<u64>,
    /// This frame replays the app's layers instead of rendering them.
    app_replayed: bool,
    /// The views of the last frame that rendered the app.
    app_views: Vec<ViewSpan>,
    /// The element count of the last frame that rendered the app.
    app_element_count: u32,
    /// What the last frame that rendered the app recorded.
    app_mode: RecordMode,
    /// The app was released from a hold and renders on the next frame.
    pub(crate) release_pending: bool,
    /// This frame renders the app on its release from a hold.
    pub(crate) app_released: bool,
    pub(crate) pending_events: Vec<InspectorEvent>,
    pub(crate) pending_causes: Vec<PendingCause>,
    pub(crate) pending_notifies: FxHashMap<EntityId, NotifyDelta>,
    pub(crate) pick_scroll: f32,
    pub(crate) pick_swallow_mouse_up: bool,
    pub(crate) input_consumed: bool,
    #[cfg(feature = "profiler")]
    pub(crate) journal: Option<crate::profiler::journal::ForegroundJournalCollector>,
}

impl Default for Recorder {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            mode: RecordMode::Off,
            suspended: false,
            frame_start: now,
            clock: PhaseClock::new(now),
            elements: Vec::new(),
            prepainted: Vec::new(),
            stack: Vec::new(),
            deferred_from: Vec::new(),
            fresh_cached_views: Vec::new(),
            splices: Vec::new(),
            paint_counter: 0,
            element_count: 0,
            hitbox_owners: Vec::new(),
            previous_hitbox_owners: Vec::new(),
            scratch: TreeScratch::default(),
            views: Vec::new(),
            open_views: Vec::new(),
            causes: Vec::new(),
            inspector_only: false,
            inspector_scene: SceneStats::default(),
            inspector_scene_mark: SceneStats::default(),
            inspector_hitboxes: 0..0,
            inspector_accessed: FxHashSet::default(),
            frame_serial: 0,
            path_instances: Vec::new(),
            cache: FxHashMap::default(),
            cache_mode: RecordMode::Off,
            view_types: FxHashMap::default(),
            inspector_entities: FxHashSet::default(),
            element_labels: FxHashMap::default(),
            live_tree: None,
            flashes: Vec::new(),
            last_viewport: None,
            last_input_seq: 0,
            notify_bucket: 0,
            last_selected: None,
            style_inputs_changed: false,
            restyling: false,
            unpresented: None,
            app_replayed: false,
            app_views: Vec::new(),
            app_element_count: 0,
            app_mode: RecordMode::Off,
            release_pending: false,
            app_released: false,
            pending_events: Vec::new(),
            pending_causes: Vec::new(),
            pending_notifies: FxHashMap::default(),
            pick_scroll: 0.,
            pick_swallow_mouse_up: false,
            input_consumed: false,
            #[cfg(feature = "profiler")]
            journal: None,
        }
    }
}

/// Distinct element-id labels remembered before the cache is reset.
const MAX_ELEMENT_LABELS: usize = 4096;

impl Recorder {
    /// Starts recording a frame.
    pub(crate) fn begin_frame(&mut self, mode: RecordMode, now: Instant) {
        self.mode = mode;
        self.frame_serial += 1;
        self.suspended = false;
        self.frame_start = now;
        self.clock = PhaseClock::new(now);
        let capacity = self.elements.capacity().max(self.element_count as usize);
        self.elements = Vec::with_capacity(if mode.builds_tree() { capacity } else { 0 });
        self.prepainted.clear();
        self.stack.clear();
        self.deferred_from.clear();
        self.fresh_cached_views.clear();
        self.splices.clear();
        self.paint_counter = 0;
        self.element_count = 0;
        self.app_replayed = false;
        self.app_released = false;
        mem::swap(&mut self.hitbox_owners, &mut self.previous_hitbox_owners);
        self.hitbox_owners.clear();
        self.views.clear();
        self.open_views.clear();
        self.inspector_scene = SceneStats::default();
        self.inspector_hitboxes = 0..0;
        if mode != self.cache_mode {
            // Stored subtrees were recorded at another level (or not at all
            // since some views last rendered); views re-render once to refill.
            self.cache.clear();
            self.cache_mode = mode;
        }
        for subtree in self.cache.values_mut() {
            subtree.used = false;
        }
    }

    pub(crate) fn frame_start(&self) -> Instant {
        self.frame_start
    }

    /// Whether element records are being built right now.
    #[inline]
    pub(crate) fn building_tree(&self) -> bool {
        self.mode.builds_tree() && !self.suspended
    }

    /// Whether elements are counted (but not recorded) right now.
    #[inline]
    pub(crate) fn counting(&self) -> bool {
        self.mode == RecordMode::Count && !self.suspended
    }

    /// Whether elements should report [`ElementDetails`] right now.
    #[inline]
    pub(crate) fn wants_details(&self) -> bool {
        self.mode == RecordMode::Details && !self.suspended
    }

    /// Switches the timed phase, returning the previous one.
    pub(crate) fn set_phase(&mut self, phase: Phase) -> Phase {
        self.clock.switch(phase, Instant::now())
    }

    /// Adds taffy time to the phase it ran in.
    pub(crate) fn add_layout_time(&mut self, duration: Duration) {
        let phase = self.clock.phase.index();
        self.clock.layout[phase] += duration;
    }

    /// Marks the start of the inspector's own drawing.
    pub(crate) fn suspend(&mut self, scene: &Scene) -> Phase {
        self.suspended = true;
        self.inspector_scene_mark = SceneStats::of(scene);
        super::user_span::recording::set_suspended(true);
        self.set_phase(Phase::Inspector)
    }

    /// Marks the end of the inspector's own drawing.
    pub(crate) fn resume(&mut self, scene: &Scene, phase: Phase) {
        let drawn = SceneStats::of(scene).combine(self.inspector_scene_mark, u32::saturating_sub);
        self.inspector_scene = self.inspector_scene.combine(drawn, u32::saturating_add);
        self.suspended = false;
        super::user_span::recording::set_suspended(false);
        self.set_phase(phase);
    }

    /// Adds entities the inspector's UI read while drawing.
    pub(crate) fn note_inspector_access(&mut self, accessed: FxHashSet<EntityId>) {
        self.inspector_accessed.extend(accessed);
    }

    /// Settles which entities only the inspector reads this frame (their
    /// notifies are the inspector's own doing) and merges its accesses into
    /// the window's, so its views are still invalidated by them.
    pub(crate) fn merge_inspector_access(&mut self, window_accessed: &mut FxHashSet<EntityId>) {
        self.inspector_entities.clear();
        for entity in self.inspector_accessed.drain() {
            if window_accessed.insert(entity) {
                self.inspector_entities.insert(entity);
            }
        }
    }

    /// Numbers the elements drawn at the same path within a frame, like
    /// [`crate::InspectorElementId::instance_id`].
    pub(crate) fn next_instance(&mut self, path: PathKey) -> u32 {
        let ix = path.0 as usize;
        if self.path_instances.len() <= ix {
            self.path_instances.resize(ix + 1, (0, 0));
        }
        let (serial, next) = &mut self.path_instances[ix];
        if *serial != self.frame_serial {
            *serial = self.frame_serial;
            *next = 0;
        }
        let instance = *next;
        *next += 1;
        instance
    }

    /// Counts an element drawn while no tree is built.
    pub(crate) fn count_element(&mut self) {
        self.element_count += 1;
    }

    /// Opens a record for an element entering `request_layout`; `None` for
    /// `kind` marks a transparent wrapper.
    pub(crate) fn open_element(
        &mut self,
        kind: Option<ElementKind>,
        key: Option<ElementKey>,
        id: Option<&ElementId>,
    ) -> RecordSlot {
        let next = self.elements.len() as ElementIndex;
        let Some(kind) = kind else {
            return RecordSlot::Alias(next);
        };
        let id = id.map(|id| self.element_label(id));
        self.elements.push(ElementRecord {
            key,
            parent: self.stack.last().copied(),
            depth: 0,
            kind,
            id,
            bounds: Bounds::default(),
            visible_bounds: None,
            paint_order: 0,
            primitives: 0,
            flags: ElementFlags::empty(),
            details: None,
        });
        self.prepainted.push(false);
        self.stack.push(next);
        RecordSlot::Own(next)
    }

    /// Closes the record opened by [`Self::open_element`] once the element's
    /// `request_layout` returned. Transparent elements resolve to the first
    /// record drawn inside them, if any.
    pub(crate) fn close_element(&mut self, slot: RecordSlot) -> Option<RecordSlot> {
        match slot {
            RecordSlot::Own(ix) => {
                self.stack.pop();
                Some(RecordSlot::Own(ix))
            }
            RecordSlot::Alias(ix) => {
                ((ix as usize) < self.elements.len()).then_some(RecordSlot::Alias(ix))
            }
        }
    }

    fn element_label(&mut self, id: &ElementId) -> SharedString {
        if let ElementId::Name(name) = id {
            return name.clone();
        }
        if let Some(label) = self.element_labels.get(id) {
            return label.clone();
        }
        if self.element_labels.len() >= MAX_ELEMENT_LABELS {
            self.element_labels.clear();
        }
        let label = SharedString::from(id.to_string());
        self.element_labels.insert(id.clone(), label.clone());
        label
    }

    /// Records an element's layout bounds as its prepaint starts.
    pub(crate) fn enter_prepaint(
        &mut self,
        ix: ElementIndex,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
    ) {
        let Some(record) = self.elements.get_mut(ix as usize) else {
            return;
        };
        record.bounds = bounds;
        record.visible_bounds = visible_part(bounds, mask);
        record
            .flags
            .set(ElementFlags::CLIPPED, record.visible_bounds.is_none());
        self.prepainted[ix as usize] = true;
        self.stack.push(ix);
    }

    /// Pops the element entered by [`Self::enter_prepaint`].
    pub(crate) fn exit(&mut self) {
        self.stack.pop();
    }

    /// Assigns the element's paint order as its paint starts.
    pub(crate) fn enter_paint(&mut self, ix: ElementIndex, primitives: u32) {
        let Some(record) = self.elements.get_mut(ix as usize) else {
            return;
        };
        self.paint_counter += 1;
        record.paint_order = self.paint_counter;
        // Holds the scene's count until `exit_paint` turns it into a delta.
        record.primitives = primitives;
        self.stack.push(ix);
    }

    /// Records the primitives the element painted and pops it.
    pub(crate) fn exit_paint(&mut self, ix: ElementIndex, primitives: u32) {
        if let Some(record) = self.elements.get_mut(ix as usize) {
            record.primitives = primitives.saturating_sub(record.primitives);
        }
        self.stack.pop();
    }

    /// The record of the element being drawn.
    pub(crate) fn current(&self) -> Option<ElementIndex> {
        self.stack.last().copied()
    }

    pub(crate) fn current_record(&mut self) -> Option<&mut ElementRecord> {
        let ix = *self.stack.last()?;
        self.elements.get_mut(ix as usize)
    }

    /// Adds flags to the record of the element being drawn.
    pub(crate) fn add_flags(&mut self, flags: ElementFlags) {
        if let Some(record) = self.current_record() {
            record.flags |= flags;
        }
    }

    /// Detaches a deferred element's record: it becomes a root, drawn above
    /// the main tree.
    pub(crate) fn defer(&mut self, ix: ElementIndex) {
        let Some(record) = self.elements.get_mut(ix as usize) else {
            return;
        };
        record.flags |= ElementFlags::DEFERRED;
        if let Some(parent) = record.parent.take() {
            self.deferred_from.push((ix, parent));
        }
    }

    /// The number of records, to roll back to if a prepaint transaction fails.
    pub(crate) fn mark(&self) -> usize {
        self.elements.len()
    }

    /// Drops records created after `mark` (a failed `Window::transact`).
    pub(crate) fn rollback(&mut self, mark: usize) {
        if mark >= self.elements.len() {
            return;
        }
        self.elements.truncate(mark);
        self.prepainted.truncate(mark);
        let mark = mark as ElementIndex;
        self.deferred_from.retain(|&(root, _)| root < mark);
        self.fresh_cached_views.retain(|(_, view)| *view < mark);
        self.splices.retain(|splice| splice.view < mark);
        self.hitbox_owners.retain(|&(_, owner)| owner < mark);
        for view in &mut self.views {
            if view.element.is_some_and(|element| element >= mark) {
                view.element = None;
            }
        }
    }

    /// Starts a view span; returns its token.
    pub(crate) fn begin_view(&mut self, entity: EntityId, type_name: &'static str) -> usize {
        let started = Instant::now();
        let span = self.views.len();
        self.view_types.insert(entity, type_name);
        self.views.push(ViewSpan {
            entity,
            type_name,
            element: self.current(),
            depth: self.open_views.len() as u16,
            start: started.saturating_duration_since(self.frame_start),
            duration: Duration::ZERO,
            outcome: ViewOutcome::Rendered,
        });
        self.open_views.push(OpenView {
            span,
            started,
            outermost: self.open_views.is_empty(),
        });
        span
    }

    /// Ends the view span `span`.
    pub(crate) fn end_view(&mut self, span: usize, outcome: ViewOutcome) {
        let Some(open) = self.open_views.pop() else {
            return;
        };
        debug_assert_eq!(open.span, span, "view spans must nest");
        let duration = Instant::now().saturating_duration_since(open.started);
        if let Some(view) = self.views.get_mut(span) {
            view.duration = duration;
            view.outcome = outcome;
        }
        let phase = self.clock.phase;
        if open.outermost && outcome == ViewOutcome::Rendered && phase != Phase::Render {
            self.clock.render[phase.index()] += duration;
        }
    }

    /// Remembers a cached view that rendered this frame, to store its records.
    pub(crate) fn note_cached_render(&mut self, global_id: &GlobalElementId) {
        if let Some(view) = self.current() {
            self.fresh_cached_views.push((global_id.clone(), view));
        }
    }

    /// Whether a cached view may reuse its previous paint without losing its
    /// part of the tree.
    pub(crate) fn can_reuse_view(&self, global_id: &GlobalElementId) -> bool {
        !self.building_tree() || self.cache.contains_key(global_id)
    }

    /// Appends a reused view's stored records under the view's own record.
    pub(crate) fn splice_view(&mut self, global_id: &GlobalElementId) {
        let Some(view) = self.current() else { return };
        let Some(subtree) = self.cache.get_mut(global_id) else {
            return;
        };
        subtree.used = true;
        let base = self.elements.len() as ElementIndex;
        for stored in &subtree.records {
            let mut record = stored.record.clone();
            let logical_parent = match stored.parent {
                StoredParent::View => view,
                StoredParent::Local(local) => base + local,
            };
            if stored.deferred_root {
                record.parent = None;
                self.deferred_from
                    .push((self.elements.len() as ElementIndex, logical_parent));
            } else {
                record.parent = Some(logical_parent);
            }
            record.flags |= ElementFlags::REUSED;
            record.paint_order = 0;
            self.elements.push(record);
            self.prepainted.push(true);
        }
        self.splices.push(Splice {
            view,
            base,
            global_id: global_id.clone(),
        });
    }

    /// Orders the records of a reused view as its paint is replayed.
    pub(crate) fn paint_reused_view(&mut self, global_id: &GlobalElementId) {
        let Some(view) = self.current() else { return };
        let Some(splice) = self.splices.iter().find(|splice| splice.view == view) else {
            return;
        };
        let Some(subtree) = self.cache.get(global_id) else {
            return;
        };
        for &local in &subtree.paint_sequence {
            self.paint_counter += 1;
            if let Some(record) = self.elements.get_mut((splice.base + local) as usize) {
                record.paint_order = self.paint_counter;
            }
        }
    }

    /// Finishes the tree: orders reused deferred records (their paint is
    /// replayed with the deferred draws, after the main tree), drops records
    /// that were never prepainted (measured and discarded), computes depths
    /// and overflow, and stores the records of cached views that rendered.
    pub(crate) fn finish_tree(&mut self, frame: u64) -> Option<Arc<ElementTree>> {
        if self.app_replayed {
            // Nothing of the app was drawn: the live tree, the stored cached
            // views and last frame's hitbox owners all still describe it.
            return None;
        }
        if !self.mode.builds_tree() {
            self.cache.clear();
            self.live_tree = None;
            return None;
        }
        self.order_reused_deferred();
        if self.prune_unprepainted() {
            self.remap_frame_indices();
        }
        let mut tree = ElementTree {
            frame,
            elements: mem::take(&mut self.elements),
            ..Default::default()
        };
        compute_depths(&mut tree.elements);
        flag_overflow(&mut tree.elements);
        tree.rebuild_children();
        self.element_count = tree.elements.len() as u32;
        self.store_cached_views(&tree);
        self.cache.retain(|_, subtree| subtree.used);
        let tree = Arc::new(tree);
        self.live_tree = Some(tree.clone());
        Some(tree)
    }

    fn order_reused_deferred(&mut self) {
        for splice in &self.splices {
            let Some(subtree) = self.cache.get(&splice.global_id) else {
                continue;
            };
            for &local in &subtree.deferred_paint_sequence {
                self.paint_counter += 1;
                if let Some(record) = self.elements.get_mut((splice.base + local) as usize) {
                    record.paint_order = self.paint_counter;
                }
            }
        }
    }

    /// Removes records whose element was never prepainted, leaving the
    /// old-to-new index map in the scratch. Returns whether anything moved.
    fn prune_unprepainted(&mut self) -> bool {
        if self.prepainted.iter().all(|&prepainted| prepainted) {
            return false;
        }
        let remap = &mut self.scratch.remap;
        remap.clear();
        remap.resize(self.elements.len(), REMOVED);
        let mut write = 0;
        for read in 0..self.elements.len() {
            if !self.prepainted[read] {
                continue;
            }
            self.elements.swap(write, read);
            let record = &mut self.elements[write];
            record.parent = record
                .parent
                .map(|parent| remap[parent as usize])
                .filter(|&parent| parent != REMOVED);
            remap[read] = write as ElementIndex;
            write += 1;
        }
        self.elements.truncate(write);
        true
    }

    fn remap_frame_indices(&mut self) {
        let remap = &self.scratch.remap;
        let map = |ix: ElementIndex| Some(remap[ix as usize]).filter(|&ix| ix != REMOVED);
        self.deferred_from = mem::take(&mut self.deferred_from)
            .into_iter()
            .filter_map(|(root, from)| Some((map(root)?, map(from)?)))
            .collect();
        self.fresh_cached_views = mem::take(&mut self.fresh_cached_views)
            .into_iter()
            .filter_map(|(global_id, view)| Some((global_id, map(view)?)))
            .collect();
        for (_, owner) in &mut self.hitbox_owners {
            *owner = map(*owner).unwrap_or(REMOVED);
        }
        for view in &mut self.views {
            view.element = view.element.and_then(map);
        }
    }

    /// Stores, for every cached view that rendered this frame, the records
    /// drawn inside it (including deferred ones and nested cached views).
    fn store_cached_views(&mut self, tree: &ElementTree) {
        if self.fresh_cached_views.is_empty() {
            return;
        }
        let records = &tree.elements;
        let slot_of: FxHashMap<ElementIndex, usize> = self
            .fresh_cached_views
            .iter()
            .enumerate()
            .map(|(slot, (_, view))| (*view, slot))
            .collect();
        let deferred_from: FxHashMap<ElementIndex, ElementIndex> =
            self.deferred_from.iter().copied().collect();
        let logical_parent = |ix: ElementIndex| {
            records[ix as usize]
                .parent
                .or_else(|| deferred_from.get(&ix).copied())
        };

        // The innermost cached view that contains each record.
        let owner = &mut self.scratch.owner;
        owner.clear();
        owner.resize(records.len(), None);
        for ix in 0..records.len() {
            owner[ix] = logical_parent(ix as ElementIndex)
                .and_then(|parent| slot_of.get(&parent).copied().or(owner[parent as usize]));
        }
        let outer_slot: Vec<Option<usize>> = self
            .fresh_cached_views
            .iter()
            .map(|(_, view)| owner[*view as usize])
            .collect();
        let mut members: Vec<Vec<ElementIndex>> = vec![Vec::new(); slot_of.len()];
        for (ix, owner) in owner.iter().enumerate() {
            let mut slot = *owner;
            while let Some(current) = slot {
                members[current].push(ix as ElementIndex);
                slot = outer_slot[current];
            }
        }

        let local = &mut self.scratch.local;
        local.clear();
        local.resize(records.len(), u32::MAX);
        for (slot, members) in members.into_iter().enumerate() {
            let (global_id, view) = &self.fresh_cached_views[slot];
            for (position, &ix) in members.iter().enumerate() {
                local[ix as usize] = position as u32;
            }
            let subtree =
                CachedSubtree::from_members(records, &members, *view, local, logical_parent);
            for &ix in &members {
                local[ix as usize] = u32::MAX;
            }
            self.cache.insert(global_id.clone(), subtree);
        }
    }

    /// The finished frame's element count.
    pub(crate) fn element_count(&self) -> u32 {
        self.element_count
    }

    /// The records of the views that rendered this frame.
    pub(crate) fn views_rendered(&self) -> impl Iterator<Item = ElementIndex> + '_ {
        self.views
            .iter()
            .filter(|view| view.outcome == ViewOutcome::Rendered)
            .filter_map(|view| view.element)
    }

    /// Whether the last frame that rendered the app recorded all this frame
    /// records, so the app can be replayed rather than rendered.
    pub(crate) fn can_replay_app(&self) -> bool {
        self.mode <= self.app_mode && (!self.mode.builds_tree() || self.live_tree.is_some())
    }

    /// Records that this frame replays the app's layers from the previous
    /// frame: the app's views are reported as served from cache, and the
    /// previous frame's tree and hitbox owners still apply.
    pub(crate) fn replay_app(&mut self) {
        self.app_replayed = true;
        mem::swap(&mut self.hitbox_owners, &mut self.previous_hitbox_owners);
        let start = Instant::now().saturating_duration_since(self.frame_start);
        self.views = self
            .app_views
            .iter()
            .map(|view| ViewSpan {
                element: None,
                start,
                duration: Duration::ZERO,
                outcome: ViewOutcome::Cached,
                ..view.clone()
            })
            .collect();
        self.element_count = self.app_element_count;
    }

    /// Whether the latest frame replayed the app instead of rendering it.
    #[cfg(test)]
    pub(crate) fn app_replayed(&self) -> bool {
        self.app_replayed
    }

    /// View spans recorded this frame.
    pub(crate) fn take_views(&mut self) -> Vec<ViewSpan> {
        if !self.app_replayed {
            self.app_views.clone_from(&self.views);
            self.app_element_count = self.element_count;
            self.app_mode = self.mode;
        }
        mem::take(&mut self.views)
    }

    /// The frame's timings; `total` is the whole draw.
    pub(crate) fn timings(&self, total: Duration) -> PhaseTimings {
        self.clock.timings(total)
    }

    /// Scene primitives drawn by the inspector's root and overlays.
    pub(crate) fn inspector_scene(&self) -> SceneStats {
        self.inspector_scene
    }

    /// Approximate heap bytes kept for cached views.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.cache
            .values()
            .map(|subtree| subtree.records.len() * mem::size_of::<StoredRecord>())
            .sum()
    }
}

const REMOVED: ElementIndex = ElementIndex::MAX;

/// Buffers used while finishing a tree, kept to avoid reallocating them
/// every frame.
#[derive(Default)]
struct TreeScratch {
    /// Old-to-new record indices after pruning.
    remap: Vec<ElementIndex>,
    /// Per record, the innermost cached view (slot) that contains it.
    owner: Vec<Option<usize>>,
    /// Per record, its index within the cached subtree being stored.
    local: Vec<u32>,
}

impl CachedSubtree {
    fn from_members(
        records: &[ElementRecord],
        members: &[ElementIndex],
        view: ElementIndex,
        local: &[u32],
        logical_parent: impl Fn(ElementIndex) -> Option<ElementIndex>,
    ) -> Self {
        let mut stored = Vec::with_capacity(members.len());
        let mut in_deferred = Vec::with_capacity(members.len());
        for &ix in members {
            let record = &records[ix as usize];
            let parent = match logical_parent(ix) {
                Some(parent) if parent != view => StoredParent::Local(local[parent as usize]),
                _ => StoredParent::View,
            };
            let deferred_root = record.parent.is_none();
            in_deferred.push(
                deferred_root
                    || matches!(parent, StoredParent::Local(parent) if in_deferred[parent as usize]),
            );
            let mut record = record.clone();
            record.flags.remove(ElementFlags::REUSED);
            stored.push(StoredRecord {
                record,
                parent,
                deferred_root,
            });
        }
        let mut painted: Vec<u32> = (0..stored.len() as u32)
            .filter(|&local| stored[local as usize].record.paint_order > 0)
            .collect();
        painted.sort_by_key(|&local| stored[local as usize].record.paint_order);
        let (deferred_paint_sequence, paint_sequence) = painted
            .into_iter()
            .partition(|&local| in_deferred[local as usize]);
        CachedSubtree {
            records: stored,
            paint_sequence,
            deferred_paint_sequence,
            used: true,
        }
    }
}

/// The part of `bounds` inside `mask`; `None` when the element is clipped
/// away entirely. Zero-size elements are visible where they sit.
pub(crate) fn visible_part(bounds: Bounds<Pixels>, mask: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    if bounds.is_empty() {
        let origin = bounds.origin;
        let inside = origin.x >= mask.left()
            && origin.x <= mask.right()
            && origin.y >= mask.top()
            && origin.y <= mask.bottom();
        return inside.then_some(bounds);
    }
    let visible = bounds.intersect(&mask);
    (!visible.is_empty()).then_some(visible)
}

/// Parents precede their children, so one pass in index order suffices.
fn compute_depths(elements: &mut [ElementRecord]) {
    for ix in 0..elements.len() {
        let depth = match elements[ix].parent {
            Some(parent) => elements[parent as usize].depth.saturating_add(1),
            None => 0,
        };
        elements[ix].depth = depth;
    }
}

/// Sub-pixel differences from snapping are not overflow.
const OVERFLOW_TOLERANCE: Pixels = Pixels(0.5);

/// Flags elements whose visible part extends past their parent's bounds,
/// which can only happen when the parent does not clip them.
fn flag_overflow(elements: &mut [ElementRecord]) {
    for ix in 0..elements.len() {
        let Some(parent) = elements[ix].parent else {
            continue;
        };
        let parent_bounds = elements[parent as usize].bounds;
        let record = &mut elements[ix];
        let Some(visible) = record.visible_bounds.filter(|visible| !visible.is_empty()) else {
            continue;
        };
        let overflows = visible.left() < parent_bounds.left() - OVERFLOW_TOLERANCE
            || visible.top() < parent_bounds.top() - OVERFLOW_TOLERANCE
            || visible.right() > parent_bounds.right() + OVERFLOW_TOLERANCE
            || visible.bottom() > parent_bounds.bottom() + OVERFLOW_TOLERANCE;
        record.flags.set(ElementFlags::OVERFLOWS_PARENT, overflows);
    }
}

impl SceneStats {
    /// Counts the primitives currently in `scene`.
    pub(crate) fn of(scene: &Scene) -> Self {
        SceneStats {
            quads: scene.quads.len() as u32,
            shadows: scene.shadows.len() as u32,
            paths: scene.paths.len() as u32,
            underlines: scene.underlines.len() as u32,
            monochrome_sprites: scene.monochrome_sprites.len() as u32,
            subpixel_sprites: scene.subpixel_sprites.len() as u32,
            polychrome_sprites: scene.polychrome_sprites.len() as u32,
            surfaces: scene.surfaces.len() as u32,
            backdrop_filters: scene.backdrop_filters.len() as u32,
            paint_operations: scene.paint_operations.len() as u32,
        }
    }
}

impl SceneStats {
    /// Applies `f` to each pair of counts.
    pub(crate) fn combine(self, other: Self, f: impl Fn(u32, u32) -> u32) -> Self {
        SceneStats {
            quads: f(self.quads, other.quads),
            shadows: f(self.shadows, other.shadows),
            paths: f(self.paths, other.paths),
            underlines: f(self.underlines, other.underlines),
            monochrome_sprites: f(self.monochrome_sprites, other.monochrome_sprites),
            subpixel_sprites: f(self.subpixel_sprites, other.subpixel_sprites),
            polychrome_sprites: f(self.polychrome_sprites, other.polychrome_sprites),
            surfaces: f(self.surfaces, other.surfaces),
            backdrop_filters: f(self.backdrop_filters, other.backdrop_filters),
            paint_operations: f(self.paint_operations, other.paint_operations),
        }
    }
}
