//! Picking an element with the pointer, over the captured element tree.
//!
//! While picking, mouse input inside the app's area goes to the picker and
//! never reaches the app: moving hovers the element under the pointer, the
//! wheel or `[` / `]` walk towards its ancestors and back, a left click
//! selects, and escape cancels. The dock keeps working normally. Because the
//! picker hit-tests the captured tree, any element with bounds can be picked,
//! not only the ones that insert hitboxes.

use super::*;
use crate::inspector::{CauseKind, ElementTree, InspectorEvent, PickState};
use crate::{KeyDownEvent, MouseDownEvent, ScrollWheelEvent};

/// Wheel travel that moves the pick one element up or down the ancestry:
/// one wheel notch (three lines).
const PIXELS_PER_PICK_STEP: f32 = 36.;
/// Height of a scrolled line, for wheels that report lines.
const SCROLL_LINE_HEIGHT: Pixels = px(PIXELS_PER_PICK_STEP / 3.);

impl Window {
    /// Starts picking: mouse input in the app's area picks an element
    /// instead of interacting with it, until a click selects one or
    /// [`Self::stop_inspector_pick`] is called. Emits
    /// [`InspectorEvent::PickHovered`] as the pointer moves,
    /// [`InspectorEvent::Picked`] on a click and
    /// [`InspectorEvent::PickCancelled`] when picking ends without one.
    pub fn start_inspector_pick(&mut self) {
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return;
        };
        capture.pick = PickState {
            active: true,
            depth: 0,
            position: None,
        };
        capture.recorder.pick_scroll = 0.;
        self.request_inspector_frame(CauseKind::Refresh);
    }

    /// Ends picking without selecting anything; emits
    /// [`InspectorEvent::PickCancelled`] (on the next frame, as this method
    /// has no access to the app).
    pub fn stop_inspector_pick(&mut self) {
        if self.end_pick() {
            if let Some(capture) = self.inspector_capture.as_deref_mut() {
                capture
                    .recorder
                    .pending_events
                    .push(InspectorEvent::PickCancelled);
            }
            self.request_inspector_frame(CauseKind::Refresh);
        }
    }

    /// Routes a mouse event to the picker. Returns true when the picker
    /// consumed it, in which case the app must not see it.
    pub(super) fn dispatch_inspector_pick_mouse(&mut self, event: &dyn Any, cx: &mut App) -> bool {
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return false;
        };
        capture.recorder.input_consumed = false;
        if !capture.pick.active {
            // The button release that follows the picking click.
            if capture.recorder.pick_swallow_mouse_up && event.is::<MouseUpEvent>() {
                capture.recorder.pick_swallow_mouse_up = false;
                capture.recorder.input_consumed = true;
                cx.stop_propagation();
                return true;
            }
            return false;
        }
        let position = self.mouse_position;
        if !self.app_bounds().contains(&position) {
            return false;
        }

        if event.is::<MouseMoveEvent>() {
            self.update_pick_hover(position, cx);
        } else if let Some(event) = event.downcast_ref::<ScrollWheelEvent>() {
            let delta = f32::from(event.delta.pixel_delta(SCROLL_LINE_HEIGHT).y);
            let steps = self.accumulate_pick_scroll(delta);
            self.move_pick_depth(steps, position, cx);
        } else if let Some(event) = event.downcast_ref::<MouseDownEvent>() {
            if event.button == MouseButton::Left {
                self.pick_at(position, cx);
            }
        }
        if let Some(capture) = self.inspector_capture.as_deref_mut() {
            capture.recorder.input_consumed = true;
        }
        cx.stop_propagation();
        true
    }

    /// Routes a key event to the picker: escape cancels, `[` / `]` move the
    /// pick down / up the ancestry. Returns true when the picker consumed it.
    pub(super) fn dispatch_inspector_pick_key(&mut self, event: &dyn Any, cx: &mut App) -> bool {
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return false;
        };
        capture.recorder.input_consumed = false;
        let Some(event) = event.downcast_ref::<KeyDownEvent>() else {
            return false;
        };
        if !capture.pick.active || event.keystroke.modifiers.modified() {
            return false;
        }
        let position = capture.pick.position.unwrap_or(self.mouse_position);
        match event.keystroke.key.as_str() {
            "escape" => {
                self.end_pick();
                self.emit_inspector_event(InspectorEvent::PickCancelled, cx);
                self.request_inspector_frame(CauseKind::Refresh);
            }
            "[" => self.move_pick_depth(-1, position, cx),
            "]" => self.move_pick_depth(1, position, cx),
            _ => return false,
        }
        if let Some(capture) = self.inspector_capture.as_deref_mut() {
            capture.recorder.input_consumed = true;
        }
        cx.stop_propagation();
        true
    }

    /// Turns wheel travel into whole pick steps, keeping the remainder.
    fn accumulate_pick_scroll(&mut self, delta: f32) -> isize {
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return 0;
        };
        let scroll = &mut capture.recorder.pick_scroll;
        *scroll += delta;
        let steps = (*scroll / PIXELS_PER_PICK_STEP).trunc();
        *scroll -= steps * PIXELS_PER_PICK_STEP;
        steps as isize
    }

    /// Moves the pick `steps` elements up (positive) or down the ancestry
    /// of the deepest element under `position`.
    fn move_pick_depth(&mut self, steps: isize, position: Point<Pixels>, cx: &mut App) {
        let hits = self.pick_hits(position).len();
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return;
        };
        let max = hits.saturating_sub(1);
        capture.pick.depth = capture.pick.depth.saturating_add_signed(steps).min(max);
        self.update_pick_hover(position, cx);
    }

    /// Hovers the element picked at `position` and requests a frame so the
    /// overlay follows.
    fn update_pick_hover(&mut self, position: Point<Pixels>, cx: &mut App) {
        let hovered = self.picked_key(position);
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return;
        };
        capture.pick.position = Some(position);
        if capture.overlay.hovered != hovered {
            capture.overlay.hovered = hovered;
            self.emit_inspector_event(InspectorEvent::PickHovered(hovered), cx);
        }
        self.invalidator.set_dirty(true);
    }

    /// Selects the element picked at `position` and ends picking.
    fn pick_at(&mut self, position: Point<Pixels>, cx: &mut App) {
        let picked = self.picked_key(position);
        self.end_pick();
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return;
        };
        capture.recorder.pick_swallow_mouse_up = true;
        match picked {
            Some(key) => {
                capture.overlay.selected = Some(key);
                self.emit_inspector_event(InspectorEvent::Picked(key), cx);
            }
            None => self.emit_inspector_event(InspectorEvent::PickCancelled, cx),
        }
        self.invalidator.set_dirty(true);
    }

    /// Clears picking state; returns whether picking was active.
    fn end_pick(&mut self) -> bool {
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return false;
        };
        let was_active = capture.pick.active;
        capture.pick = PickState::default();
        capture.recorder.pick_scroll = 0.;
        if was_active {
            capture.overlay.hovered = None;
        }
        was_active
    }

    /// The element the picker targets at `position`: the element under the
    /// pointer that has a key, [`PickState::depth`] steps from the topmost.
    fn picked_key(&self, position: Point<Pixels>) -> Option<crate::inspector::ElementKey> {
        let capture = self.inspector_capture.as_deref()?;
        let tree = capture.recorder.live_tree.as_deref()?;
        let hits = keyed_hits(tree, position);
        let ix = *hits.get(capture.pick.depth.min(hits.len().checked_sub(1)?))?;
        tree.get(ix)?.key
    }

    fn pick_hits(&self, position: Point<Pixels>) -> Vec<crate::inspector::ElementIndex> {
        self.inspector_capture
            .as_deref()
            .and_then(|capture| capture.recorder.live_tree.as_deref())
            .map(|tree| keyed_hits(tree, position))
            .unwrap_or_default()
    }
}

/// Elements under `position` that can be selected, topmost first.
fn keyed_hits(tree: &ElementTree, position: Point<Pixels>) -> Vec<crate::inspector::ElementIndex> {
    tree.hit_test(position)
        .into_iter()
        .filter(|&ix| tree.get(ix).is_some_and(|record| record.key.is_some()))
        .collect()
}
