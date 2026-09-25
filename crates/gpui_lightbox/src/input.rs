//! Driving a [`Stage`] with real input. Every event goes through
//! `Window::dispatch_event`, the same path platform input takes, so hitboxes,
//! hover styles, focus and key bindings all behave as in the app.

use crate::{shot::TextLine, stage::Stage};
use gpui::{
    Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    PlatformInput, Point, ScrollDelta, ScrollWheelEvent, TouchPhase, point, px,
};

impl Stage {
    /// Dispatches a raw platform event, then runs pending tasks and draws.
    #[track_caller]
    pub fn dispatch(&mut self, event: PlatformInput) -> &mut Self {
        if let PlatformInput::MouseMove(event) = &event {
            self.pointer.position = event.position;
        }
        self.update(|window, cx| {
            window.dispatch_event(event, cx);
        });
        self
    }

    /// Moves the mouse to `at` (logical pixels), hovering whatever is there.
    #[track_caller]
    pub fn hover(&mut self, at: Point<Pixels>) -> &mut Self {
        self.move_mouse(at, None)
    }

    /// Hovers the center of the unique visible text `text`.
    ///
    /// # Panics
    ///
    /// If no visible text or several match, listing what is visible.
    #[track_caller]
    pub fn hover_text(&mut self, text: &str) -> &mut Self {
        let at = self.text_center(text);
        self.hover(at)
    }

    /// Clicks the primary button at `at`: move, press, release.
    #[track_caller]
    pub fn click(&mut self, at: Point<Pixels>) -> &mut Self {
        self.move_mouse(at, None);
        self.press(at, MouseButton::Left);
        self.release(at, MouseButton::Left)
    }

    /// Clicks the center of the visible bounds of the unique visible text `text`.
    ///
    /// # Panics
    ///
    /// If no visible text or several match; the message lists the closest
    /// visible texts with their positions.
    #[track_caller]
    pub fn click_text(&mut self, text: &str) -> &mut Self {
        let at = self.text_center(text);
        self.click(at)
    }

    /// Drags with the primary button from `from` to `to` in `steps` moves.
    #[track_caller]
    pub fn drag(&mut self, from: Point<Pixels>, to: Point<Pixels>, steps: usize) -> &mut Self {
        self.move_mouse(from, None);
        self.press(from, MouseButton::Left);
        let steps = steps.max(1);
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            self.move_mouse(at, Some(MouseButton::Left));
        }
        self.release(to, MouseButton::Left)
    }

    /// Scrolls by `delta` pixels with the mouse at `at`. Positive `y` moves
    /// the content down (toward the top), as a trackpad swipe down does.
    #[track_caller]
    pub fn scroll(&mut self, at: Point<Pixels>, delta: Point<Pixels>) -> &mut Self {
        self.move_mouse(at, None);
        let modifiers = self.pointer.modifiers;
        self.dispatch(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(delta),
            modifiers,
            touch_phase: TouchPhase::Moved,
        }))
    }

    /// Types `text` one character at a time into the focused element, as key
    /// presses that fall through to its input handler.
    #[track_caller]
    pub fn type_text(&mut self, text: &str) -> &mut Self {
        for char in text.chars() {
            let keystroke = match char {
                ' ' => keystroke("space", " ", false),
                '\n' => keystroke("enter", "\n", false),
                '\t' => keystroke("tab", "\t", false),
                char => keystroke(
                    &char.to_lowercase().to_string(),
                    &char.to_string(),
                    char.is_uppercase(),
                ),
            };
            self.update(|window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            });
        }
        self
    }

    /// Presses a space-separated sequence of keystrokes, in gpui's key
    /// binding syntax: `"cmd-k escape"`, `"ctrl-shift-p down enter"`.
    ///
    /// # Panics
    ///
    /// If a keystroke doesn't parse.
    #[track_caller]
    pub fn keys(&mut self, keystrokes: &str) -> &mut Self {
        for source in keystrokes.split_whitespace() {
            let keystroke = Keystroke::parse(source)
                .unwrap_or_else(|error| panic!("lightbox: bad keystroke {source:?}: {error}"));
            self.update(|window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            });
        }
        self
    }

    /// Holds `modifiers` for subsequent mouse events (e.g. shift-click).
    pub fn hold(&mut self, modifiers: Modifiers) -> &mut Self {
        self.pointer.modifiers = modifiers;
        self
    }

    /// Where the mouse is.
    pub fn mouse_position(&self) -> Point<Pixels> {
        self.pointer.position
    }

    #[track_caller]
    fn text_center(&mut self, text: &str) -> Point<Pixels> {
        match self.find_text(text) {
            Ok(line) => center(&line),
            Err(error) => panic!("lightbox: {error}"),
        }
    }

    #[track_caller]
    fn move_mouse(&mut self, at: Point<Pixels>, pressed_button: Option<MouseButton>) -> &mut Self {
        let modifiers = self.pointer.modifiers;
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            modifiers,
            pressed_button,
        }))
    }

    #[track_caller]
    fn press(&mut self, at: Point<Pixels>, button: MouseButton) -> &mut Self {
        let modifiers = self.pointer.modifiers;
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            position: at,
            modifiers,
            button,
            click_count: 1,
            first_mouse: false,
        }))
    }

    #[track_caller]
    fn release(&mut self, at: Point<Pixels>, button: MouseButton) -> &mut Self {
        let modifiers = self.pointer.modifiers;
        self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
            position: at,
            modifiers,
            button,
            click_count: 1,
        }))
    }
}

fn center(line: &TextLine) -> Point<Pixels> {
    let (x, y) = line.visible.center();
    point(px(x), px(y))
}

fn keystroke(key: &str, key_char: &str, shift: bool) -> Keystroke {
    Keystroke {
        modifiers: Modifiers {
            shift,
            ..Modifiers::default()
        },
        key: key.into(),
        key_char: Some(key_char.into()),
    }
}
