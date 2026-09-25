//! The input the inspector records: one [`InputRecord`] per dispatched event
//! or stand-alone action, and the render cause each event contributes.
//!
//! Whether input is the inspector's own (consumed by picking, aimed at the
//! dock, or dispatched to an element inside it) is decided once per event, so
//! an event never counts as app input in one place and inspector input in
//! another.

use super::*;
use crate::inspector::{CauseKind, InputRecord, InputScope};

impl Window {
    /// Starts an input record for `event` while the inspector is capturing.
    /// Pointer events inside the dock, or while picking, belong to the inspector.
    pub(super) fn begin_input_capture(&mut self, event: &PlatformInput) -> Option<InputScope> {
        let capture = self
            .inspector_capture
            .as_deref()
            .filter(|capture| capture.is_recording())?;
        let mut in_flight = crate::inspector::InputInFlight::for_event(
            capture,
            event,
            self.invalidator.update_count(),
        );
        if let Some(position) = in_flight.record.position {
            in_flight.record.inspector = capture.pick().active
                || self
                    .inspector_bounds()
                    .is_some_and(|dock| dock.contains(&position));
        }
        Some(InputScope {
            outer: self.inspector_input.replace(Box::new(in_flight)),
        })
    }

    /// Records the key context stack of the element a key event is dispatched
    /// to, and whether that element is inside the inspector.
    pub(super) fn note_input_target(&mut self, dispatch_path: &[DispatchNodeId]) {
        if let Some(mut in_flight) = self.inspector_input.take() {
            self.describe_input_target(dispatch_path, &mut in_flight.record);
            self.inspector_input = Some(in_flight);
        }
    }

    /// Starts an [`crate::inspector::InputKind::Action`] record for an action
    /// dispatched outside of any event while the inspector is capturing.
    /// Actions dispatched while handling an event join that event's record.
    pub(super) fn begin_action_capture(
        &mut self,
        node_id: DispatchNodeId,
        action: &dyn Action,
    ) -> Option<InputScope> {
        if self.inspector_input.is_some() {
            return None;
        }
        let capture = self
            .inspector_capture
            .as_deref()
            .filter(|capture| capture.is_recording())?;
        let mut in_flight = crate::inspector::InputInFlight::start(
            capture,
            crate::inspector::InputKind::Action,
            action.name().into(),
            self.invalidator.update_count(),
        );
        let dispatch_path = self.rendered_frame.dispatch_tree.dispatch_path(node_id);
        self.describe_input_target(&dispatch_path, &mut in_flight.record);
        self.inspector_input = Some(Box::new(in_flight));
        Some(InputScope { outer: None })
    }

    /// Adds a dispatched action to the record being built, and commits the
    /// record if [`Self::begin_action_capture`] started it for this action.
    pub(super) fn finish_action_capture(
        &mut self,
        scope: Option<InputScope>,
        action: &dyn Action,
        binding: Option<&KeyBinding>,
        handled: bool,
    ) {
        if let Some(in_flight) = self.inspector_input.as_deref_mut() {
            in_flight.push_action(action, binding, handled);
        }
        self.finish_input_capture(scope, handled);
    }

    /// Finishes capturing `event` once it has been dispatched: decides once
    /// whether it was the inspector's own input, notes it as a render cause
    /// if it invalidated the window, and commits its record.
    pub(super) fn finish_event_capture(
        &mut self,
        scope: Option<InputScope>,
        event: &PlatformInput,
        caused_invalidation: bool,
        handled: bool,
    ) {
        let ours = scope.is_some();
        let from_inspector = self.is_inspector_input(event)
            || (ours
                && self
                    .inspector_input
                    .as_ref()
                    .is_some_and(|in_flight| in_flight.record.inspector));
        if let Some(capture) = self.inspector_capture.as_deref_mut() {
            capture.recorder.input_consumed = false;
            if caused_invalidation {
                self.invalidator.note_cause(
                    CauseKind::Input {
                        event: event.kind_name(),
                    },
                    None,
                    from_inspector,
                );
            }
        }
        if ours && let Some(in_flight) = self.inspector_input.as_deref_mut() {
            in_flight.record.inspector = from_inspector;
        }
        self.finish_input_capture(scope, handled);
    }

    /// Commits the record started with `scope` and makes the enclosing
    /// dispatch's record current again.
    fn finish_input_capture(&mut self, scope: Option<InputScope>, handled: bool) {
        let Some(scope) = scope else {
            return;
        };
        let Some(in_flight) = mem::replace(&mut self.inspector_input, scope.outer) else {
            return;
        };
        let record = in_flight.finish(
            handled,
            self.invalidator.update_count(),
            self.pending_input_keystrokes(),
        );
        if let Some(capture) = self.inspector_capture.as_deref_mut() {
            capture.commit_input(record);
        }
    }

    /// Fills in the key context stack along `dispatch_path`, outermost first,
    /// and flags the record when the path runs through the inspector's view.
    fn describe_input_target(&self, dispatch_path: &[DispatchNodeId], record: &mut InputRecord) {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        record.context_stack = dispatch_path
            .iter()
            .filter_map(|&node_id| dispatch_tree.node(node_id).context.clone())
            .collect();
        let inspector_node = self
            .inspector
            .as_ref()
            .and_then(|inspector| dispatch_tree.view_node_id(inspector.entity_id()));
        record.inspector |= inspector_node.is_some_and(|node_id| dispatch_path.contains(&node_id));
    }

    /// Whether `event` belongs to the inspector: consumed by picking, a
    /// pointer event over the dock, or a key event while the dock has focus.
    pub(crate) fn is_inspector_input(&self, event: &PlatformInput) -> bool {
        let Some(capture) = self.inspector_capture.as_deref() else {
            return false;
        };
        if capture.recorder.input_consumed {
            return true;
        }
        if event.mouse_event().is_some() {
            return self
                .inspector_bounds()
                .is_some_and(|bounds| bounds.contains(&self.mouse_position));
        }
        event.keyboard_event().is_some() && self.inspector_has_focus()
    }

    fn inspector_has_focus(&self) -> bool {
        let (Some(focus), Some(inspector)) = (self.focus, self.inspector.as_ref()) else {
            return false;
        };
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        dispatch_tree
            .focusable_node_id(focus)
            .is_some_and(|node| dispatch_tree.view_contains(inspector.entity_id(), node))
    }
}
