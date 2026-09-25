//! Loupe: an inspector and profiler for gpui applications.
//!
//! Call [`init`] once at startup, then press `cmd-alt-i` (macOS) or
//! `ctrl-shift-i` (elsewhere) in any window to open Loupe docked inside it.
//! Everything Loupe shows comes from the window's
//! [`gpui::inspector::InspectorCapture`], which records frames, element trees
//! and input only while Loupe is open. See `DESIGN.md` for the architecture.

pub mod analysis;
mod loupe;

use gpui::{
    App, AppContext as _, IntoElement as _, KeyBinding, StyleRefinement, Styled as _, actions,
};
use std::borrow::Cow;

pub use loupe::Loupe;

actions!(
    loupe,
    [
        /// Opens or closes Loupe in the active window.
        ToggleInspector,
        /// Starts or stops picking an element in the app.
        TogglePick,
    ]
);

/// The UI font Loupe renders with. Embedded, so every platform looks the same.
pub const UI_FONT: &str = "IBM Plex Sans";
/// The font Loupe uses for values, paths and code.
pub const MONO_FONT: &str = "Lilex";

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

    let toggle = if cfg!(target_os = "macos") {
        "cmd-alt-i"
    } else {
        "ctrl-shift-i"
    };
    let pick = if cfg!(target_os = "macos") {
        "cmd-shift-c"
    } else {
        "ctrl-shift-c"
    };
    cx.bind_keys([
        KeyBinding::new(toggle, ToggleInspector, None),
        KeyBinding::new(pick, TogglePick, None),
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
