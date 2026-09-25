//! Headless end-to-end harness for Loupe (`test-support`).
//!
//! [`LoupeHarness`] opens a real window (headless, real text shaping through
//! cosmic-text, real pixels through wgpu) with Loupe initialized, drives it
//! with real input events and saves screenshots to
//! `target/loupe-shots/<name>.png` (or `$LOUPE_SHOTS`). Text assertions use
//! `Window::painted_text`; controls without text are found by their debug
//! selector.

use crate::{Loupe, LoupeSettings, LoupeState, REFRESH_INTERVAL, UI_FONT, theme::Appearance};
use gpui::{
    AnyWindowHandle, App, Bounds, Entity, HeadlessAppContext, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintedText, Pixels, PlatformInput, Point,
    Render, ScrollDelta, ScrollWheelEvent, Size, Subscription, TouchPhase, WeakEntity, Window,
    inspector::InspectorCapture,
};
use gpui_wgpu::{CosmicTextSystem, WgpuHeadlessRenderer};
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

/// A headless window with Loupe, driven like a user would.
pub struct LoupeHarness {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    loupe: Rc<RefCell<Option<WeakEntity<Loupe>>>>,
    _loupe_created: Subscription,
}

impl LoupeHarness {
    /// Opens a `size` window whose root view is built by `build_root`, with
    /// Loupe initialized but closed. Rendering uses the Loupe fonts only, so
    /// screenshots are identical everywhere.
    pub fn new<V: Render + 'static>(
        size: Size<Pixels>,
        build_root: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Self {
        let text = Arc::new(CosmicTextSystem::new_without_system_fonts(UI_FONT));
        let mut cx = HeadlessAppContext::with_platform(text, Arc::new(()), || {
            WgpuHeadlessRenderer::new()
                .ok()
                .map(|renderer| Box::new(renderer) as Box<dyn gpui::PlatformHeadlessRenderer>)
        });
        cx.update(crate::init);
        let loupe = Rc::new(RefCell::new(None));
        let loupe_created = cx.update(|cx| {
            let loupe = loupe.clone();
            cx.observe_new(move |_: &mut Loupe, _, cx| {
                *loupe.borrow_mut() = Some(cx.entity().downgrade());
            })
        });
        let window = cx
            .open_window(size, build_root)
            .expect("headless window opens")
            .into();
        let mut harness = Self {
            cx,
            window,
            loupe,
            _loupe_created: loupe_created,
        };
        harness.draw();
        harness
    }

    /// Opens Loupe docked in the window.
    pub fn open_loupe(&mut self) {
        self.update(|window, cx| {
            if !window.is_inspector_open() {
                window.toggle_inspector(cx);
            }
        });
        self.draw();
    }

    /// Replaces Loupe's recording with `capture` (Loupe must be open) and
    /// lets Loupe catch up.
    pub fn install_capture(&mut self, capture: InspectorCapture) {
        self.update(|window, _| window.replace_inspector_capture_for_test(capture));
        self.advance(REFRESH_INTERVAL);
    }

    /// Chooses Loupe's palette.
    pub fn set_appearance(&mut self, appearance: Appearance) {
        self.cx.update(|cx| {
            cx.set_global(LoupeSettings {
                appearance,
                ..LoupeSettings::get(cx)
            })
        });
        self.draw();
    }

    /// Runs `f` with the window.
    pub fn update<R>(&mut self, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        self.cx
            .update_window(self.window, |_, window, cx| f(window, cx))
            .expect("window is open")
    }

    /// Runs pending work; like a platform frame loop, the window draws a
    /// frame only if something invalidated it (test mode draws dirty windows
    /// whenever effects flush).
    pub fn draw(&mut self) {
        self.cx.run_until_parked();
        self.update(|_, _| {});
    }

    /// Redraws every view, including cached ones (so debug bounds are fresh).
    pub fn redraw_all(&mut self) {
        self.cx.run_until_parked();
        self.update(|window, cx| {
            window.refresh_with_inspector();
            window.draw(cx).clear(cx);
        });
    }

    /// Advances the simulated clock (firing Loupe's refresh timer) and draws.
    pub fn advance(&mut self, duration: Duration) {
        self.cx.advance_clock(duration);
        self.draw();
    }

    /// The Loupe root view.
    pub fn loupe(&self) -> Entity<Loupe> {
        self.loupe
            .borrow()
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .expect("Loupe is open and has been drawn")
    }

    /// Reads Loupe's shared state.
    pub fn state<R>(&mut self, f: impl FnOnce(&LoupeState) -> R) -> R {
        let loupe = self.loupe();
        self.cx.update(|cx| {
            let state = loupe.read(cx).state().clone();
            f(state.read(cx))
        })
    }

    /// Updates Loupe's shared state (e.g. to select something) and draws.
    pub fn update_state(
        &mut self,
        f: impl FnOnce(&mut LoupeState, &mut gpui::Context<LoupeState>),
    ) {
        let loupe = self.loupe();
        self.cx.update(|cx| {
            let state = loupe.read(cx).state().clone();
            state.update(cx, f);
        });
        self.draw();
    }

    /// Reads the window's capture.
    pub fn capture<R>(&mut self, f: impl FnOnce(&InspectorCapture) -> R) -> R {
        self.update(|window, _| f(window.inspector_capture().expect("Loupe is open")))
    }

    /// Runs `f` with the app.
    pub fn app<R>(&mut self, f: impl FnOnce(&mut App) -> R) -> R {
        self.cx.update(f)
    }

    /// Every text line painted in the last frame.
    pub fn painted_text(&mut self) -> Vec<PaintedText> {
        self.update(|window, _| window.painted_text().to_vec())
    }

    /// The visible bounds of the first painted line equal to `text` (or,
    /// failing that, containing it).
    pub fn find_text(&mut self, text: &str) -> Option<Bounds<Pixels>> {
        let lines = self.painted_text();
        let visible = |line: &&PaintedText| {
            let bounds = line.visible_bounds();
            bounds.size.width > Pixels::ZERO && bounds.size.height > Pixels::ZERO
        };
        lines
            .iter()
            .filter(visible)
            .find(|line| line.text.as_ref() == text)
            .or_else(|| {
                lines
                    .iter()
                    .filter(visible)
                    .find(|line| line.text.contains(text))
            })
            .map(PaintedText::visible_bounds)
    }

    /// Panics, listing what was painted, unless `text` is visible.
    pub fn assert_text_visible(&mut self, text: &str) {
        if self.find_text(text).is_none() {
            let painted: Vec<String> = self
                .painted_text()
                .iter()
                .map(|line| line.text.to_string())
                .collect();
            panic!("{text:?} is not visible; painted: {painted:?}");
        }
    }

    /// The bounds of the element with `debug_selector` (after a full redraw).
    pub fn bounds_of(&mut self, selector: &str) -> Bounds<Pixels> {
        self.redraw_all();
        self.update(|window, _| window.debug_bounds(selector))
            .unwrap_or_else(|| panic!("nothing painted with debug selector {selector:?}"))
    }

    /// Where the pulse strip shows `frame`.
    pub fn pulse_bar(&mut self, frame: u64) -> Point<Pixels> {
        let loupe = self.loupe();
        self.update(|window, cx| loupe.read(cx).pulse_bar_center(frame, window))
            .unwrap_or_else(|| panic!("frame #{frame} is not in the pulse strip"))
    }

    /// Clicks `frame`'s bar in the pulse strip.
    pub fn click_pulse_frame(&mut self, frame: u64) {
        let position = self.pulse_bar(frame);
        self.click(position);
    }

    /// Moves the pointer to `position`.
    pub fn hover(&mut self, position: Point<Pixels>) {
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }));
    }

    /// Clicks at `position` (move, press, release).
    pub fn click(&mut self, position: Point<Pixels>) {
        self.click_with(position, Modifiers::default(), 1);
    }

    /// Clicks at `position` holding `modifiers`, as the `click_count`th
    /// click in a row (2 for the second click of a double-click).
    pub fn click_with(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        click_count: usize,
    ) {
        self.hover(position);
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count,
            first_mouse: false,
        }));
        self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count,
        }));
    }

    /// Scrolls the wheel by `delta` pixels at `position` (positive `y`
    /// scrolls up, towards the top of the content).
    pub fn scroll(&mut self, position: Point<Pixels>, delta: Point<Pixels>) {
        self.hover(position);
        self.dispatch(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(delta),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        }));
    }

    /// Clicks the center of the painted line `text`.
    pub fn click_text(&mut self, text: &str) {
        let bounds = self
            .find_text(text)
            .unwrap_or_else(|| panic!("no visible text {text:?} to click"));
        self.click(bounds.center());
    }

    /// Clicks the center of the element with `debug_selector`.
    pub fn click_selector(&mut self, selector: &str) {
        let bounds = self.bounds_of(selector);
        self.click(bounds.center());
    }

    /// Drags with the left button from `from` to `to` in `steps` moves.
    pub fn drag(&mut self, from: Point<Pixels>, to: Point<Pixels>, steps: usize) {
        self.hover(from);
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: from,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }));
        let steps = steps.max(1);
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let position = from + (to - from) * t;
            self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }));
        }
        self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: to,
            modifiers: Modifiers::default(),
            click_count: 1,
        }));
    }

    /// Types space-separated keystrokes, e.g. `"secondary-k"` or `"alt-2 j j"`.
    pub fn type_keys(&mut self, keys: &str) {
        for keystroke in keys.split_whitespace() {
            let keystroke = Keystroke::parse(keystroke)
                .unwrap_or_else(|error| panic!("bad keystroke {keystroke:?}: {error}"));
            self.update(|window, cx| window.dispatch_keystroke(keystroke, cx));
            self.draw();
        }
    }

    /// Types `text` character by character into the focused field.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let keystroke = if c == ' ' {
                "space".to_string()
            } else {
                c.to_string()
            };
            self.type_keys(&keystroke);
        }
    }

    /// Saves the last frame as `target/loupe-shots/<name>.png` and returns its path.
    pub fn screenshot(&mut self, name: &str) -> PathBuf {
        self.draw();
        let image = self
            .cx
            .capture_screenshot(self.window)
            .expect("headless renderer captures screenshots");
        let dir = std::env::var_os("LOUPE_SHOTS")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/loupe-shots")
            });
        std::fs::create_dir_all(&dir).expect("screenshot directory is writable");
        let path = dir.join(format!("{name}.png"));
        image.save(&path).expect("screenshot saves");
        path
    }

    fn dispatch(&mut self, event: PlatformInput) {
        self.update(|window, cx| {
            window.dispatch_event(event, cx);
        });
        self.draw();
    }
}
