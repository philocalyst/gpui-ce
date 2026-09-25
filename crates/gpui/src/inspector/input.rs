//! Input capture: turns dispatched events and actions into [`InputRecord`]s.
//!
//! The window builds one [`InputInFlight`] per event while the inspector is
//! capturing, adds the actions dispatched while handling it, and commits it to
//! the capture when dispatch returns.

use super::{capture::InspectorCapture, model::*};
use crate::{
    Action, FileDropEvent, KeyBinding, Keystroke, Modifiers, MouseButton, NavigationDirection,
    Pixels, PlatformInput, Point, ScrollDelta, SharedString, TouchPhase,
};
use scheduler::Instant;
use smallvec::SmallVec;
use std::fmt::Write as _;

/// The most elements recorded in an [`InputRecord::hit_path`].
const HIT_PATH_LEN: usize = 8;

/// Returned when a window starts building an input record. Holds the record
/// of the dispatch it is nested in, if any, which becomes current again once
/// this one is committed.
pub(crate) struct InputScope {
    pub(crate) outer: Option<Box<InputInFlight>>,
}

/// An input record being built while its event, or a directly dispatched
/// action, is handled.
pub(crate) struct InputInFlight {
    /// The record so far; `seq` is assigned on commit.
    pub(crate) record: InputRecord,
    started: Instant,
    update_count: usize,
}

impl InputInFlight {
    /// Starts a record of `kind` now. `update_count` is the window
    /// invalidator's count, to tell whether handling invalidated the window.
    pub(crate) fn start(
        capture: &InspectorCapture,
        kind: InputKind,
        detail: SharedString,
        update_count: usize,
    ) -> Self {
        let started = Instant::now();
        Self {
            record: InputRecord {
                seq: 0,
                at: capture.now(),
                frame: None,
                kind,
                detail,
                position: None,
                keystroke: None,
                hit_path: SmallVec::new(),
                context_stack: SmallVec::new(),
                actions: SmallVec::new(),
                handled: false,
                duration: Default::default(),
                caused_redraw: false,
                coalesced: 0,
                inspector: false,
            },
            started,
            update_count,
        }
    }

    /// Starts a record describing a platform event, with the elements under
    /// the pointer from the latest captured tree.
    pub(crate) fn for_event(
        capture: &InspectorCapture,
        event: &PlatformInput,
        update_count: usize,
    ) -> Self {
        let (kind, detail) = describe(event);
        let mut in_flight = Self::start(capture, kind, detail, update_count);
        let record = &mut in_flight.record;
        record.position = position(event);
        record.keystroke = match event {
            PlatformInput::KeyDown(event) => Some(event.keystroke.clone()),
            PlatformInput::KeyUp(event) => Some(event.keystroke.clone()),
            _ => None,
        };
        if let Some((position, tree)) = record.position.zip(capture.latest_tree()) {
            record.hit_path = tree
                .hit_test(position)
                .into_iter()
                .filter_map(|ix| tree.get(ix)?.key)
                .take(HIT_PATH_LEN)
                .collect();
        }
        in_flight
    }

    /// Adds an action dispatched while handling the record's event.
    pub(crate) fn push_action(
        &mut self,
        action: &dyn Action,
        binding: Option<&KeyBinding>,
        handled: bool,
    ) {
        self.record.actions.push(ActionRecord {
            name: action.name(),
            handled,
            keystrokes: binding.map(super::keys::binding_keystrokes),
            context: binding.and_then(super::keys::binding_predicate),
        });
    }

    /// Completes the record once handling returned. `pending` is the window's
    /// pending multi-stroke input afterwards: a key down it ends with is
    /// marked as held by the keymap.
    pub(crate) fn finish(
        mut self,
        handled: bool,
        update_count: usize,
        pending: Option<&[Keystroke]>,
    ) -> InputRecord {
        let record = &mut self.record;
        record.handled = handled;
        record.duration = self.started.elapsed();
        record.caused_redraw = update_count != self.update_count;
        if record.kind == InputKind::KeyDown
            && pending
                .and_then(<[Keystroke]>::last)
                .is_some_and(|last| record.keystroke.as_ref() == Some(last))
        {
            record.detail = format!("{} (pending)", record.detail).into();
        }
        self.record
    }
}

impl InspectorCapture {
    /// Records finished input unless frozen (or replaying fixtures). A pointer
    /// move directly following another, with no frame drawn in between, is
    /// merged into it.
    pub(crate) fn commit_input(&mut self, record: InputRecord) {
        if !self.is_recording() {
            return;
        }
        match self.input.back_mut() {
            Some(last) if can_coalesce(last, &record) => {
                coalesce(last, record);
                if !last.inspector {
                    self.generation += 1;
                }
            }
            _ => {
                self.record_input(record);
            }
        }
    }
}

/// Whether `next` continues the pointer movement recorded in `last`: both are
/// moves on the same side of the inspector, and no frame was drawn after
/// `last` (a drawn frame claims the input before it through its range).
fn can_coalesce(last: &InputRecord, next: &InputRecord) -> bool {
    last.kind == InputKind::MouseMove
        && next.kind == InputKind::MouseMove
        && last.frame.is_none()
        && last.inspector == next.inspector
}

/// Merges `next` into `last`: the latest position wins and handling time adds up.
fn coalesce(last: &mut InputRecord, next: InputRecord) {
    last.coalesced += 1 + next.coalesced;
    last.detail = next.detail;
    last.position = next.position;
    last.hit_path = next.hit_path;
    last.actions.extend(next.actions);
    last.handled |= next.handled;
    last.duration += next.duration;
    last.caused_redraw |= next.caused_redraw;
}

/// The category and human-readable summary of an event, e.g.
/// `(MouseDown, "left ×1 at (120, 44)")`.
pub(crate) fn describe(event: &PlatformInput) -> (InputKind, SharedString) {
    let mut detail = String::new();
    let kind = match event {
        PlatformInput::KeyDown(event) => {
            detail.push_str(&event.keystroke.unparse());
            if event.is_held {
                detail.push_str(" (held)");
            }
            InputKind::KeyDown
        }
        PlatformInput::KeyUp(event) => {
            detail.push_str(&event.keystroke.unparse());
            InputKind::KeyUp
        }
        PlatformInput::ModifiersChanged(event) => {
            let mut names = modifier_names(&event.modifiers);
            if event.capslock.on {
                names.push("capslock");
            }
            if names.is_empty() {
                detail.push_str("none");
            } else {
                detail.push_str(&names.join("+"));
            }
            InputKind::Modifiers
        }
        PlatformInput::MouseDown(event) => {
            push_click(
                &mut detail,
                &event.modifiers,
                event.button,
                event.click_count,
            );
            push_at(&mut detail, event.position);
            InputKind::MouseDown
        }
        PlatformInput::MouseUp(event) => {
            push_click(
                &mut detail,
                &event.modifiers,
                event.button,
                event.click_count,
            );
            push_at(&mut detail, event.position);
            InputKind::MouseUp
        }
        PlatformInput::MouseMove(event) => {
            if let Some(button) = event.pressed_button {
                write!(detail, "{} drag ", button_name(button)).ok();
            }
            push_modifiers(&mut detail, &event.modifiers);
            write!(detail, "to {}", fmt_point(event.position)).ok();
            InputKind::MouseMove
        }
        PlatformInput::MouseExited(event) => {
            write!(detail, "left the window at {}", fmt_point(event.position)).ok();
            InputKind::MouseExit
        }
        PlatformInput::MousePressure(event) => {
            write!(detail, "pressure {:.2}", event.pressure).ok();
            push_at(&mut detail, event.position);
            InputKind::Gesture
        }
        PlatformInput::ScrollWheel(event) => {
            push_modifiers(&mut detail, &event.modifiers);
            match event.delta {
                ScrollDelta::Pixels(delta) => write!(detail, "scroll Δ{}", fmt_point(delta)),
                ScrollDelta::Lines(delta) => {
                    write!(detail, "scroll Δ({}, {}) lines", delta.x, delta.y)
                }
            }
            .ok();
            InputKind::Scroll
        }
        PlatformInput::Pinch(event) => {
            push_modifiers(&mut detail, &event.modifiers);
            write!(detail, "pinch {:+.2}", event.delta).ok();
            InputKind::Gesture
        }
        PlatformInput::LongPress(event) => {
            write!(
                detail,
                "long press {} at {}",
                phase_name(event.phase),
                fmt_point(event.position)
            )
            .ok();
            InputKind::Gesture
        }
        PlatformInput::TouchDrag(event) => {
            write!(
                detail,
                "touch drag {} to {}",
                phase_name(event.phase),
                fmt_point(event.position)
            )
            .ok();
            InputKind::Gesture
        }
        PlatformInput::Touch(event) => {
            write!(
                detail,
                "touch {} {} at {}",
                event.id.0,
                phase_name(event.phase),
                fmt_point(event.position)
            )
            .ok();
            InputKind::Touch
        }
        PlatformInput::FileDrop(event) => {
            match event {
                FileDropEvent::Entered { position, paths } => write!(
                    detail,
                    "{} file(s) entered at {}",
                    paths.paths().len(),
                    fmt_point(*position)
                ),
                FileDropEvent::Pending { position } => {
                    write!(detail, "files over {}", fmt_point(*position))
                }
                FileDropEvent::Submit { position } => {
                    write!(detail, "files dropped at {}", fmt_point(*position))
                }
                FileDropEvent::Exited => write!(detail, "files left the window"),
                FileDropEvent::Ended => write!(detail, "file drag ended"),
            }
            .ok();
            InputKind::FileDrop
        }
    };
    (kind, detail.into())
}

/// The pointer position of an event, if it has one.
pub(crate) fn position(event: &PlatformInput) -> Option<Point<Pixels>> {
    match event {
        PlatformInput::MouseDown(event) => Some(event.position),
        PlatformInput::MouseUp(event) => Some(event.position),
        PlatformInput::MouseMove(event) => Some(event.position),
        PlatformInput::MouseExited(event) => Some(event.position),
        PlatformInput::MousePressure(event) => Some(event.position),
        PlatformInput::ScrollWheel(event) => Some(event.position),
        PlatformInput::Pinch(event) => Some(event.position),
        PlatformInput::LongPress(event) => Some(event.position),
        PlatformInput::TouchDrag(event) => Some(event.position),
        PlatformInput::Touch(event) => Some(event.position),
        PlatformInput::FileDrop(
            FileDropEvent::Entered { position, .. }
            | FileDropEvent::Pending { position }
            | FileDropEvent::Submit { position },
        ) => Some(*position),
        PlatformInput::FileDrop(FileDropEvent::Exited | FileDropEvent::Ended)
        | PlatformInput::KeyDown(_)
        | PlatformInput::KeyUp(_)
        | PlatformInput::ModifiersChanged(_) => None,
    }
}

/// `shift-left ×2`.
fn push_click(detail: &mut String, modifiers: &Modifiers, button: MouseButton, clicks: usize) {
    push_modifiers(detail, modifiers);
    write!(detail, "{} ×{clicks}", button_name(button)).ok();
}

/// ` at (120, 44)`.
fn push_at(detail: &mut String, position: Point<Pixels>) {
    write!(detail, " at {}", fmt_point(position)).ok();
}

/// Held modifiers as a keystroke prefix, e.g. `ctrl-shift-`.
fn push_modifiers(detail: &mut String, modifiers: &Modifiers) {
    for name in modifier_names(modifiers) {
        detail.push_str(name);
        detail.push('-');
    }
}

/// Names of the held modifiers, in keystroke order.
fn modifier_names(modifiers: &Modifiers) -> SmallVec<[&'static str; 6]> {
    let mut names = SmallVec::new();
    for (held, name) in [
        (modifiers.function, "fn"),
        (modifiers.control, "ctrl"),
        (modifiers.alt, "alt"),
        (modifiers.platform, PLATFORM_MODIFIER),
        (modifiers.shift, "shift"),
    ] {
        if held {
            names.push(name);
        }
    }
    names
}

/// How keystrokes spell the platform modifier, as in [`Keystroke::unparse`].
#[cfg(target_os = "macos")]
const PLATFORM_MODIFIER: &str = "cmd";
#[cfg(target_os = "windows")]
const PLATFORM_MODIFIER: &str = "win";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const PLATFORM_MODIFIER: &str = "super";

fn button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::Navigate(NavigationDirection::Back) => "back",
        MouseButton::Navigate(NavigationDirection::Forward) => "forward",
    }
}

fn phase_name(phase: TouchPhase) -> &'static str {
    match phase {
        TouchPhase::Started => "started",
        TouchPhase::Moved => "moved",
        TouchPhase::Ended => "ended",
        TouchPhase::Cancelled => "cancelled",
    }
}

/// `(120, 44.5)`: whole pixels without decimals, others with one.
fn fmt_point(point: Point<Pixels>) -> String {
    format!("({}, {})", fmt_pixels(point.x), fmt_pixels(point.y))
}

fn fmt_pixels(pixels: Pixels) -> String {
    let value = pixels.0;
    if value.fract() == 0. {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

#[cfg(test)]
mod tests;
