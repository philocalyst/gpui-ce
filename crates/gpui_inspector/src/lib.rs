//! Loupe: an inspector and profiler for gpui applications.
//!
//! Call [`init`] once at startup, then press `cmd-alt-i` (macOS) or
//! `ctrl-shift-i` (elsewhere) in any window to open Loupe docked inside it.
//! Everything Loupe shows comes from the window's
//! [`gpui::inspector::InspectorCapture`], which records frames, element trees
//! and input only while Loupe is open. See `DESIGN.md` for the architecture.

pub mod analysis;
mod commands;
mod lenses;
mod loupe;
mod palette;
mod shell;
mod state;
pub mod theme;
pub mod widgets;

#[cfg(any(test, feature = "test-support"))]
pub mod fixtures;
#[cfg(any(test, feature = "test-support"))]
pub mod harness;

use gpui::{
    App, AppContext as _, IntoElement as _, KeyBinding, StyleRefinement, Styled as _, actions,
};
use std::borrow::Cow;

pub use commands::{Command, DockSide};
pub use lenses::ExportDirectory;
pub use loupe::{Loupe, REFRESH_INTERVAL};
pub use palette::fuzzy;
pub use state::{Filters, Lens, LensLayout, LoupeState};
pub use theme::{LoupeSettings, MONO_FONT, UI_FONT};

actions!(
    loupe,
    [
        /// Opens or closes Loupe in the active window.
        ToggleInspector,
        /// Starts or stops picking an element in the app.
        TogglePick,
        /// Holds the app still (or releases it) so it can be inspected as is.
        ToggleHold,
    ]
);

const FONTS: [&[u8]; 6] = [
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Italic.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Bold.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Italic.ttf"),
];

/// Registers Loupe: its fonts, actions, key bindings and the window renderer.
pub fn init(cx: &mut App) {
    if let Err(error) = cx
        .text_system()
        .add_fonts(FONTS.iter().map(|font| Cow::Borrowed(*font)).collect())
    {
        log::warn!("loupe: failed to load its fonts: {error}");
    }

    cx.bind_keys([
        KeyBinding::new(commands::keys::TOGGLE, ToggleInspector, None),
        KeyBinding::new(commands::keys::PICK, TogglePick, None),
        KeyBinding::new(commands::keys::HOLD, ToggleHold, None),
    ]);
    loupe::bind_keys(cx);

    on_active_window::<ToggleInspector>(cx, |window, cx| window.toggle_inspector(cx));
    on_active_window::<TogglePick>(cx, |window, cx| {
        if !window.is_inspector_open() {
            window.toggle_inspector(cx);
        }
        loupe::toggle_pick(window);
    });
    on_active_window::<ToggleHold>(cx, |window, cx| {
        if !window.is_inspector_open() {
            window.toggle_inspector(cx);
        }
        loupe::toggle_hold(window, cx);
    });

    cx.set_inspector_renderer(Box::new(|inspector, window, cx| {
        let handle = cx.entity();
        let loupe = inspector
            .ui_state(|| cx.new(|cx| Loupe::new(handle, window, cx)))
            .clone();
        gpui::AnyView::from(loupe)
            .cached(StyleRefinement::default().size_full())
            .into_any_element()
    }));
}

/// Handles a global action on the active window. Global shortcuts are
/// dispatched while that window is being updated, so the handler runs once
/// the dispatch is over.
fn on_active_window<A: gpui::Action>(
    cx: &mut App,
    handler: impl Fn(&mut gpui::Window, &mut App) + Copy + 'static,
) {
    cx.on_action(move |_: &A, cx| {
        let Some(window) = cx.active_window() else {
            return;
        };
        cx.defer(move |cx| {
            if let Err(error) = window.update(cx, |_, window, cx| handler(window, cx)) {
                log::warn!("loupe: {}: {error}", A::name_for_type());
            }
        });
    });
}
