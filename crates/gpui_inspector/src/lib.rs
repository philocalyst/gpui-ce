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
    ]);
    loupe::bind_keys(cx);

    cx.on_action(|_: &ToggleInspector, cx| {
        if let Some(window) = cx.active_window() {
            window
                .update(cx, |_, window, cx| window.toggle_inspector(cx))
                .ok();
        }
    });
    cx.on_action(|_: &TogglePick, cx| {
        if let Some(window) = cx.active_window() {
            window
                .update(cx, |_, window, cx| {
                    if !window.is_inspector_open() {
                        window.toggle_inspector(cx);
                    }
                    loupe::toggle_pick(window);
                })
                .ok();
        }
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
