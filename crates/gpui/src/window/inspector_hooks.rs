//! The window's side of the inspector capture: everything that happens while
//! a frame is drawn with the inspector open.
//!
//! Every hook starts by checking for the window's capture (or for a record
//! slot, which only exists while a tree is recorded), so a closed inspector
//! costs one branch per hook. Nothing is recorded while the inspector draws
//! its own root; that time goes to [`PhaseTimings::inspector`].

use super::*;
use crate::inspector::{
    CaptureLevel, CauseKind, ElementDetails, ElementFlags, ElementKey, FrameRecord,
    InspectorCapture, InspectorEvent, OverlayModes, SceneStats, SelectedStyle, ViewOutcome,
    causes::{self, CauseLog},
    recorder::{FLASH_DURATION, Flash, Phase, RecordMode, RecordSlot},
};
use crate::inspector::{ForcedStates, user_span::recording as spans};
use crate::{InspectorElementId, InspectorElementPath, StyleRefinement};
use std::panic::Location;

/// What [`Window::inspector_suspend`] saved, restored by [`Window::inspector_resume`].
pub(super) struct InspectorPhase {
    previous: Phase,
    app_accessed: FxHashSet<EntityId>,
}

impl WindowInvalidator {
    /// Starts or stops collecting render causes for the window's inspector.
    pub(crate) fn set_cause_log(&self, log: Option<Box<CauseLog>>) {
        self.inner.borrow_mut().inspector_causes = log;
    }

    /// Records a render cause, if the window's inspector is open.
    pub(crate) fn note_cause(
        &self,
        kind: CauseKind,
        site: Option<&'static Location<'static>>,
        from_inspector: bool,
    ) {
        if let Some(log) = self.inner.borrow_mut().inspector_causes.as_mut() {
            log.push(kind, site, from_inspector);
        }
    }

    /// Whether causes are being collected (the window's inspector is open).
    #[cfg(test)]
    pub(crate) fn has_cause_log(&self) -> bool {
        self.inner.borrow().inspector_causes.is_some()
    }

    /// Moves out the causes recorded since the last frame.
    fn drain_causes(
        &self,
        causes: &mut Vec<causes::PendingCause>,
        notifies: &mut FxHashMap<EntityId, causes::NotifyDelta>,
    ) {
        if let Some(log) = self.inner.borrow_mut().inspector_causes.as_mut() {
            log.drain(causes, notifies);
        }
    }
}

impl InspectorCapture {
    /// Stable identity of an element across frames.
    pub(crate) fn element_key(&mut self, id: &InspectorElementId) -> ElementKey {
        ElementKey {
            path: self.intern(&id.path),
            instance: id.instance_id as u32,
        }
    }

    /// Whether overlays or picking need this frame's tree even at
    /// [`CaptureLevel::Frames`].
    fn overlays_need_tree(&self) -> bool {
        let overlay = self.overlay();
        self.pick.active
            || overlay.hovered.is_some()
            || overlay.selected.is_some()
            || overlay.modes.intersects(
                OverlayModes::OUTLINES
                    | OverlayModes::PAINT_FLASH
                    | OverlayModes::OVERFLOW
                    | OverlayModes::HITBOXES,
            )
    }
}

impl Window {
    // Frame lifecycle.

    /// Starts recording the frame about to be drawn: resolves why it is
    /// drawn and decides what it records.
    pub(super) fn inspector_begin_frame(&mut self, cx: &mut App) {
        if cx.mode.skip_drawing() {
            return;
        }
        let viewport = self.app_bounds().size;
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return;
        };
        let now = Instant::now();
        let mode = RecordMode::for_level(capture.config.level, capture.overlays_need_tree());
        let recording = capture.is_recording();
        let recorder = &mut capture.recorder;
        recorder.begin_frame(mode, now);

        self.invalidator
            .drain_causes(&mut recorder.pending_causes, &mut recorder.pending_notifies);
        let resized = recorder
            .last_viewport
            .is_some_and(|last_viewport| last_viewport != viewport);
        if resized
            && !recorder
                .pending_causes
                .iter()
                .any(|cause| cause.kind == CauseKind::Resize)
        {
            recorder.pending_causes.push(causes::PendingCause {
                kind: CauseKind::Resize,
                site: None,
                at: now,
                from_inspector: false,
                entity: None,
            });
        }
        recorder.last_viewport = Some(viewport);
        recorder.causes = causes::resolve_causes(
            &mut recorder.pending_causes,
            now,
            &recorder.inspector_entities,
            &recorder.view_types,
        );
        recorder.inspector_only =
            !recorder.causes.is_empty() && recorder.causes.iter().all(|cause| cause.from_inspector);
        if !recording {
            recorder.pending_notifies.clear();
        } else {
            causes::fold_notify_stats(
                &mut capture.notify_stats,
                &mut recorder.notify_bucket,
                &mut recorder.pending_notifies,
                capture.epoch,
                now,
            );
        }

        // Overrides, forced states and the selection's base style are
        // applied while elements render, so views re-render once when they change.
        let selected = capture.overlay.selected;
        recorder.restyling = recorder.style_inputs_changed || selected != recorder.last_selected;
        if recorder.restyling {
            if selected != recorder.last_selected {
                capture.selected_style = None;
            }
            recorder.style_inputs_changed = false;
            recorder.last_selected = selected;
            self.refreshing = true;
        }

        spans::begin_frame(now);
        let events = mem::take(&mut capture.recorder.pending_events);
        for event in events {
            self.emit_inspector_event(event, cx);
        }
    }

    /// Switches the phase the frame's time is attributed to.
    pub(super) fn inspector_set_phase(&mut self, phase: Phase) {
        if let Some(capture) = self.inspector_capture.as_deref_mut()
            && capture.recorder.mode != RecordMode::Off
        {
            capture.recorder.set_phase(phase);
        }
    }

    /// Stops recording while the inspector draws its own root or overlays.
    /// Entities its UI reads are collected apart from the app's.
    pub(super) fn inspector_suspend(&mut self, cx: &mut App) -> Option<InspectorPhase> {
        let capture = self.inspector_capture.as_deref_mut()?;
        if capture.recorder.mode == RecordMode::Off {
            return None;
        }
        let previous = capture.recorder.suspend(&self.next_frame.scene);
        let app_accessed = mem::take(cx.entities.accessed_entities.get_mut());
        Some(InspectorPhase {
            previous,
            app_accessed,
        })
    }

    /// Resumes recording after [`Self::inspector_suspend`].
    pub(super) fn inspector_resume(&mut self, phase: Option<InspectorPhase>, cx: &mut App) {
        let Some(phase) = phase else { return };
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return;
        };
        let inspector_accessed =
            mem::replace(cx.entities.accessed_entities.get_mut(), phase.app_accessed);
        capture.recorder.note_inspector_access(inspector_accessed);
        capture
            .recorder
            .resume(&self.next_frame.scene, phase.previous);
    }

    /// Merges the entities the inspector's UI read into the window's, before
    /// the window registers them for invalidation.
    pub(super) fn inspector_merge_accessed(&mut self, cx: &mut App) {
        if let Some(capture) = self.inspector_capture.as_deref_mut() {
            capture
                .recorder
                .merge_inspector_access(cx.entities.accessed_entities.get_mut());
        }
    }

    /// Finishes the frame's element tree and flashes the views it rendered,
    /// ahead of the overlay pass that paints them.
    pub(super) fn inspector_finish_tree(&mut self) {
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return;
        };
        if capture.recorder.mode == RecordMode::Off {
            return;
        }
        let frame = capture.next_frame_id;
        let tree = capture.recorder.finish_tree(frame);
        let flashing = capture.overlay.modes.contains(OverlayModes::PAINT_FLASH);
        let recorder = &mut capture.recorder;
        let now = Instant::now();
        recorder
            .flashes
            .retain(|flash| flashing && now.duration_since(flash.started) < FLASH_DURATION);
        if flashing
            && !recorder.inspector_only
            && let Some(tree) = tree
        {
            let rendered = recorder
                .views_rendered()
                .filter_map(|element| tree.get(element))
                .map(|record| Flash {
                    bounds: record.bounds,
                    started: now,
                })
                .collect::<Vec<_>>();
            recorder.flashes.extend(rendered);
        }
    }

    /// Completes the frame's record and appends it to the capture.
    pub(super) fn inspector_end_frame(&mut self) {
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return;
        };
        if capture.recorder.mode == RecordMode::Off {
            return;
        }
        let recorder = &mut capture.recorder;
        let frame_start = recorder.frame_start();
        recorder.set_phase(Phase::Idle);
        let total = Instant::now().saturating_duration_since(frame_start);
        let mut timings = recorder.timings(total);
        let spans = spans::end_frame();
        let scene = SceneStats::of(&self.rendered_frame.scene)
            .combine(recorder.inspector_scene(), u32::saturating_sub);
        let input = recorder.last_input_seq..capture.next_input_seq;
        recorder.last_input_seq = capture.next_input_seq;
        timings.input = input_time(capture, &input);
        let recorder = &mut capture.recorder;
        let tree = recorder
            .live_tree
            .clone()
            .filter(|tree| tree.frame == capture.next_frame_id)
            .filter(|_| capture.config.level >= CaptureLevel::Tree);
        let frame = FrameRecord {
            id: 0,
            start: frame_start.saturating_duration_since(capture.epoch),
            viewport: recorder.last_viewport.unwrap_or_default(),
            timings,
            causes: mem::take(&mut recorder.causes),
            views: recorder.take_views(),
            spans,
            scene,
            element_count: recorder.element_count(),
            tree,
            input,
            foreground: foreground_slices(recorder, capture.epoch),
            inspector_only: recorder.inspector_only,
        };
        recorder.mode = RecordMode::Off;
        let restyled = recorder.restyling && frame.inspector_only;
        if capture.is_recording() {
            let id = capture.record_frame(frame);
            capture.recorder.unpresented = Some(id);
            if restyled {
                // The inspector restyled the app: its tree is new app data.
                capture.generation += 1;
            }
        }
    }

    /// Fills in how long the platform took to present the frame just drawn.
    pub(super) fn inspector_note_present(&mut self, duration: Duration) {
        if let Some(capture) = self.inspector_capture.as_deref_mut()
            && let Some(id) = capture.recorder.unpresented.take()
            && let Some(frame) = capture.frames.back_mut()
            && frame.id == id
        {
            frame.timings.present = Some(duration);
        }
    }

    /// Records why the window refreshes.
    pub(super) fn note_refresh_reason(&self, reason: RefreshReason) {
        let (kind, site) = match reason {
            RefreshReason::Code(site) => (CauseKind::Refresh, Some(site)),
            RefreshReason::Resize => (CauseKind::Resize, None),
            RefreshReason::WindowState => (CauseKind::WindowState, None),
            RefreshReason::Inspector => return,
        };
        self.invalidator.note_cause(kind, site, false);
    }

    /// Schedules a frame for the inspector's overlays without invalidating
    /// any view, so only the overlay (and uncached views) redraw.
    pub(super) fn request_inspector_frame(&self, kind: CauseKind) {
        self.invalidator.note_cause(kind, None, true);
        self.invalidator.set_dirty(true);
    }

    /// The window's inspector entity, while the inspector is open.
    #[cfg(test)]
    pub(crate) fn inspector_entity(&self) -> Option<Entity<Inspector>> {
        self.inspector.clone()
    }

    pub(super) fn emit_inspector_event(&mut self, event: InspectorEvent, cx: &mut App) {
        if let Some(inspector) = self.inspector.clone() {
            inspector.update(cx, |_, cx| cx.emit(event));
        }
    }

    // Element hooks, called by `Drawable`.

    /// Identifies an element entering `request_layout` (when it has a source
    /// location) and opens its record while a tree is recorded.
    #[inline]
    pub(crate) fn inspector_begin_element<E: Element>(
        &mut self,
        element: &E,
        global_id: Option<&GlobalElementId>,
        path: Option<InspectorElementPath>,
    ) -> (Option<InspectorElementId>, Option<RecordSlot>) {
        match self.inspector_capture.as_deref_mut() {
            Some(capture) if capture.recorder.building_tree() => {
                return open_element(capture, element, global_id, path);
            }
            Some(capture) if capture.recorder.counting() && element.inspector_kind().is_some() => {
                capture.recorder.count_element();
            }
            _ => {}
        }
        (path.map(|path| self.build_inspector_element_id(path)), None)
    }

    /// Closes the record opened by [`Self::inspector_begin_element`].
    #[inline]
    pub(crate) fn inspector_close_element(
        &mut self,
        slot: Option<RecordSlot>,
    ) -> Option<RecordSlot> {
        let slot = slot?;
        let capture = self.inspector_capture.as_deref_mut()?;
        capture.recorder.close_element(slot)
    }

    /// Records an element's bounds as its prepaint starts.
    #[inline]
    pub(crate) fn inspector_enter_prepaint(
        &mut self,
        slot: Option<RecordSlot>,
        bounds: Bounds<Pixels>,
    ) {
        let Some(RecordSlot::Own(ix)) = slot else {
            return;
        };
        let mask = self.content_mask().bounds;
        if let Some(capture) = self.inspector_capture.as_deref_mut() {
            capture.recorder.enter_prepaint(ix, bounds, mask);
        }
    }

    #[inline]
    pub(crate) fn inspector_exit_prepaint(&mut self, slot: Option<RecordSlot>) {
        if let Some(RecordSlot::Own(_)) = slot
            && let Some(capture) = self.inspector_capture.as_deref_mut()
        {
            capture.recorder.exit();
        }
    }

    /// Orders an element as its paint starts.
    #[inline]
    pub(crate) fn inspector_enter_paint(&mut self, slot: Option<RecordSlot>) {
        let Some(RecordSlot::Own(ix)) = slot else {
            return;
        };
        let primitives = SceneStats::of(&self.next_frame.scene).primitives();
        if let Some(capture) = self.inspector_capture.as_deref_mut() {
            capture.recorder.enter_paint(ix, primitives);
        }
    }

    /// Counts the primitives an element painted.
    #[inline]
    pub(crate) fn inspector_exit_paint(&mut self, slot: Option<RecordSlot>) {
        let Some(RecordSlot::Own(ix)) = slot else {
            return;
        };
        let primitives = SceneStats::of(&self.next_frame.scene).primitives();
        if let Some(capture) = self.inspector_capture.as_deref_mut() {
            capture.recorder.exit_paint(ix, primitives);
        }
    }

    /// Detaches a deferred element's record into a root of its own.
    pub(super) fn inspector_defer(&mut self, element: &AnyElement) {
        if let Some(capture) = self.inspector_capture.as_deref_mut()
            && let Some(slot) = element.inspector_record()
        {
            capture.recorder.defer(slot.index());
        }
    }

    /// Flags the current element as a hitbox owner.
    pub(super) fn inspector_note_hitbox(&mut self, id: HitboxId) {
        if let Some(capture) = self.inspector_capture.as_deref_mut()
            && capture.recorder.building_tree()
            && let Some(owner) = capture.recorder.current()
        {
            capture.recorder.add_flags(ElementFlags::HITBOX);
            capture.recorder.hitbox_owners.push((id, owner));
        }
    }

    /// The tree's size before a prepaint transaction.
    pub(super) fn inspector_mark(&self) -> Option<usize> {
        let capture = self.inspector_capture.as_deref()?;
        capture
            .recorder
            .building_tree()
            .then(|| capture.recorder.mark())
    }

    /// Drops the records of a failed prepaint transaction.
    pub(super) fn inspector_rollback(&mut self, mark: Option<usize>) {
        if let Some(mark) = mark
            && let Some(capture) = self.inspector_capture.as_deref_mut()
        {
            capture.recorder.rollback(mark);
        }
    }

    /// Starts timing a taffy layout computation.
    #[inline]
    pub(super) fn inspector_layout_started(&self) -> Option<Instant> {
        let capture = self.inspector_capture.as_deref()?;
        (capture.recorder.mode != RecordMode::Off).then(Instant::now)
    }

    /// Adds a finished layout computation to the phase it ran in.
    #[inline]
    pub(super) fn inspector_layout_finished(&mut self, started: Option<Instant>) {
        if let Some(started) = started
            && let Some(capture) = self.inspector_capture.as_deref_mut()
        {
            capture.recorder.add_layout_time(started.elapsed());
        }
    }

    // View hooks, called by `ViewElement`.

    /// Starts a view span; returns its token.
    pub(crate) fn inspector_begin_view(
        &mut self,
        entity: EntityId,
        type_name: &'static str,
    ) -> Option<usize> {
        let capture = self.inspector_capture.as_deref_mut()?;
        let recorder = &mut capture.recorder;
        (recorder.mode != RecordMode::Off && !recorder.suspended)
            .then(|| recorder.begin_view(entity, type_name))
    }

    /// Ends a view span started by [`Self::inspector_begin_view`].
    pub(crate) fn inspector_end_view(&mut self, span: Option<usize>, outcome: ViewOutcome) {
        if let Some(span) = span
            && let Some(capture) = self.inspector_capture.as_deref_mut()
        {
            capture.recorder.end_view(span, outcome);
        }
    }

    /// Remembers a cached view that is rendering, to replay its records when
    /// it is reused.
    pub(crate) fn inspector_note_cached_render(&mut self, global_id: &GlobalElementId) {
        if let Some(capture) = self.inspector_capture.as_deref_mut()
            && capture.recorder.building_tree()
        {
            capture.recorder.note_cached_render(global_id);
        }
    }

    /// Appends a reused cached view's records from the frame that rendered it.
    pub(crate) fn inspector_splice_view(&mut self, global_id: &GlobalElementId) {
        if let Some(capture) = self.inspector_capture.as_deref_mut()
            && capture.recorder.building_tree()
        {
            capture.recorder.splice_view(global_id);
        }
    }

    /// Orders a reused cached view's records as its paint is replayed.
    pub(crate) fn inspector_paint_reused_view(&mut self, global_id: &GlobalElementId) {
        if let Some(capture) = self.inspector_capture.as_deref_mut()
            && capture.recorder.building_tree()
        {
            capture.recorder.paint_reused_view(global_id);
        }
    }

    // Element facts, reported by built-in elements.

    /// Whether elements should report [`ElementDetails`] right now.
    #[inline]
    pub(crate) fn inspector_wants_details(&self) -> bool {
        self.inspector_capture
            .as_deref()
            .is_some_and(|capture| capture.recorder.wants_details())
    }

    /// Adds flags to the record of the element being drawn; `flags` only runs
    /// while a tree is recorded.
    #[inline]
    pub(crate) fn inspector_add_flags(&mut self, flags: impl FnOnce() -> ElementFlags) {
        if let Some(capture) = self.inspector_capture.as_deref_mut()
            && capture.recorder.building_tree()
        {
            capture.recorder.add_flags(flags());
        }
    }

    /// Applies the inspector's style inputs to an element as it computes its
    /// style: records the selected element's base style, refines `base_style`
    /// with a live override, and returns the states forced on the element.
    pub(crate) fn inspector_style(
        &mut self,
        inspector_id: Option<&InspectorElementId>,
        base_style: &mut StyleRefinement,
    ) -> ForcedStates {
        let (Some(inspector_id), Some(capture)) =
            (inspector_id, self.inspector_capture.as_deref_mut())
        else {
            return ForcedStates::empty();
        };
        if capture.recorder.suspended
            || (capture.overrides.is_empty()
                && capture.forced_states.is_empty()
                && capture.overlay.selected.is_none())
        {
            return ForcedStates::empty();
        }
        let key = capture
            .recorder
            .current_record()
            .and_then(|record| record.key)
            .unwrap_or_else(|| capture.element_key(inspector_id));
        if capture.overlay.selected == Some(key) {
            capture.selected_style = Some(SelectedStyle {
                key,
                base: Box::new(base_style.clone()),
            });
        }
        if let Some(refinement) = capture.overrides.get(&key.path) {
            base_style.refine(refinement);
            if capture.recorder.building_tree() {
                capture.recorder.add_flags(ElementFlags::OVERRIDDEN);
            }
        }
        capture.forced_states(key.path)
    }

    /// Lets the element currently being drawn report rich facts (text, box
    /// model, colors, list range...) to the inspector. `f` only runs while the
    /// inspector is capturing at [`CaptureLevel::Full`], so computing the
    /// facts inside `f` costs nothing otherwise.
    pub fn inspect_current_element(&mut self, f: impl FnOnce(&mut ElementDetails)) {
        if let Some(capture) = self.inspector_capture.as_deref_mut()
            && capture.recorder.wants_details()
            && let Some(record) = capture.recorder.current_record()
        {
            f(record.details.get_or_insert_default());
        }
    }

    // Lifetime of the capture.

    /// Creates the capture when the inspector opens.
    pub(super) fn open_inspector_capture(&mut self, cx: &mut App) {
        let capture = InspectorCapture::new(self.inspector_dock);
        let log = CauseLog::new(capture.epoch());
        self.inspector_capture = Some(Box::new(capture));
        self.invalidator.set_cause_log(Some(Box::new(log)));
        self.invalidator.note_cause(CauseKind::Initial, None, false);
        #[cfg(feature = "profiler")]
        if let Some(capture) = self.inspector_capture.as_deref_mut() {
            capture.recorder.journal = Some(cx.foreground_journal().collector());
        }
        #[cfg(not(feature = "profiler"))]
        let _ = cx;
    }

    /// Drops the capture when the inspector closes, remembering the dock.
    pub(super) fn close_inspector_capture(&mut self) {
        if let Some(capture) = self.inspector_capture.take() {
            self.inspector_dock = capture.dock();
        }
        self.invalidator.set_cause_log(None);
    }

    /// Legacy per-element inspector state (see
    /// [`App::register_inspector_element`]): `f` receives the state kept for
    /// the selected element, or `None` for every other element.
    pub fn with_inspector_state<T: 'static, R>(
        &mut self,
        inspector_id: Option<&InspectorElementId>,
        cx: &mut App,
        f: impl FnOnce(&mut Option<T>, &mut Self) -> R,
    ) -> R {
        if let Some(inspector_id) = inspector_id
            && let Some(inspector) = self.inspector.clone()
            && self.is_selected_by_inspector(inspector_id)
        {
            return inspector.update(cx, |inspector, _| {
                inspector.set_active_element_id(inspector_id);
                inspector.with_active_element_state(self, f)
            });
        }
        f(&mut None, self)
    }

    fn is_selected_by_inspector(&mut self, inspector_id: &InspectorElementId) -> bool {
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return false;
        };
        let Some(selected) = capture.overlay.selected else {
            return false;
        };
        capture.paths.get(&*inspector_id.path).is_some_and(|&path| {
            selected
                == ElementKey {
                    path,
                    instance: inspector_id.instance_id as u32,
                }
        })
    }
}

/// Identifies an element through the capture's interner, which also numbers
/// instances (so each element costs one hash), and opens its record.
fn open_element<E: Element>(
    capture: &mut InspectorCapture,
    element: &E,
    global_id: Option<&GlobalElementId>,
    path: Option<InspectorElementPath>,
) -> (Option<InspectorElementId>, Option<RecordSlot>) {
    let (inspector_id, key) = match path {
        Some(path) => {
            let (path, path_key) = capture.intern_shared(path);
            let instance = capture.recorder.next_instance(path_key);
            let key = ElementKey {
                path: path_key,
                instance,
            };
            let id = InspectorElementId {
                path,
                instance_id: instance as usize,
            };
            (Some(id), Some(key))
        }
        None => (None, None),
    };
    // A view's generated id adds nothing to the entity its kind carries.
    let id = global_id
        .and_then(|global_id| global_id.last())
        .filter(|id| !matches!(id, ElementId::View(_)));
    let slot = capture
        .recorder
        .open_element(element.inspector_kind(), key, id);
    (inspector_id, Some(slot))
}

/// Time spent handling the app's input since the previous frame.
fn input_time(capture: &InspectorCapture, seqs: &Range<u64>) -> Duration {
    capture
        .input()
        .iter()
        .rev()
        .take_while(|record| record.seq >= seqs.start)
        .filter(|record| seqs.contains(&record.seq) && !record.inspector)
        .map(|record| record.duration)
        .sum()
}

/// Main-thread work since the previous frame, from the profiler journal.
#[cfg(feature = "profiler")]
fn foreground_slices(
    recorder: &mut crate::inspector::recorder::Recorder,
    epoch: Instant,
) -> Vec<crate::inspector::ForegroundSlice> {
    use crate::inspector::{ForegroundKind, ForegroundSlice};
    use crate::profiler::journal::{ForegroundEvent, ForegroundJournalEntry};

    /// Keeps a frame record bounded after a long idle stretch.
    const MAX_SLICES: usize = 4096;

    let Some(journal) = recorder.journal.as_mut() else {
        return Vec::new();
    };
    journal
        .collect_unseen()
        .entries
        .into_iter()
        .filter_map(|entry| match entry {
            ForegroundJournalEntry::Event(event) => Some(event),
            _ => None,
        })
        .filter_map(|event| {
            let (kind, duration) = match event {
                ForegroundEvent::TaskPoll(timing) => (
                    ForegroundKind::Task {
                        site: timing.location,
                    },
                    event.duration(),
                ),
                ForegroundEvent::Action(timing) => (
                    ForegroundKind::Action { name: timing.name },
                    event.duration(),
                ),
                ForegroundEvent::Input(timing) => (
                    ForegroundKind::Input { kind: timing.kind },
                    event.duration(),
                ),
                ForegroundEvent::Present(_) => (ForegroundKind::Present, event.duration()),
                // Folded polls report the time they ran, not the span they cover.
                ForegroundEvent::SmallPolls(flush) => (
                    ForegroundKind::SmallPolls {
                        count: flush.summary.count.min(u32::MAX as u64) as u32,
                    },
                    flush.summary.total,
                ),
                // The frame itself.
                ForegroundEvent::Draw(_) => return None,
            };
            Some(ForegroundSlice {
                kind,
                start: event.start_time().saturating_duration_since(epoch),
                duration,
            })
        })
        .take(MAX_SLICES)
        .collect()
}

#[cfg(not(feature = "profiler"))]
fn foreground_slices(
    _recorder: &mut crate::inspector::recorder::Recorder,
    _epoch: Instant,
) -> Vec<crate::inspector::ForegroundSlice> {
    Vec::new()
}
