//! Loupe: an inspector and profiler for gpui applications.
//!
//! Loupe docks inside the window it inspects and answers *why*: why an
//! element has its size, why a frame was drawn and why it was slow, where a
//! click went, what a key will do, which entities keep notifying. Register it
//! once at startup:
//!
//! ```ignore
//! gpui_platform::application().run(|cx: &mut gpui::App| {
//!     gpui_inspector::init(cx);
//!     // Open windows as usual.
//! });
//! ```
//!
//! Then press `cmd-alt-i` (macOS) or `ctrl-shift-i` (elsewhere) in any
//! window. `?` inside Loupe lists every other key. To choose Loupe's
//! defaults (theme, density, editor, frame budget, capture level), register
//! it with [`init_with`] and a [`LoupeSettings`].
//!
//! Everything Loupe shows comes from the window's
//! [`gpui::inspector::InspectorCapture`], which exists and records frames,
//! element trees and input only while Loupe is open. Closed, every engine
//! hook costs one `Option` check. The README covers usage; `DESIGN.md` the
//! architecture.

pub mod analysis;
mod commands;
mod lenses;
mod loupe;
mod palette;
pub mod settings;
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

pub use analysis::source::{Editor, EditorUrl};
pub use commands::{Command, DockSide};
pub use lenses::ExportDirectory;
pub use loupe::{Loupe, REFRESH_INTERVAL};
pub use palette::fuzzy;
pub use settings::{FrameBudget, LoupeSettings};
pub use state::{Filters, Lens, LensLayout, LoupeState};
pub use theme::{MONO_FONT, UI_FONT};

actions!(
    loupe,
    [
        /// Opens or closes Loupe in the active window.
        ToggleInspector,
        /// Starts or stops picking an element in the app.
        TogglePick,
        /// Holds the app still, or releases it.
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

/// Registers Loupe: its fonts, actions, key bindings and the window
/// renderer. Settings keep their defaults, or whatever the app already set.
pub fn init(cx: &mut App) {
    if let Err(error) = cx
        .text_system()
        .add_fonts(FONTS.iter().map(|font| Cow::Borrowed(*font)).collect())
    {
        log::warn!("loupe: failed to load its fonts: {error}");
    }

    cx.bind_keys(key_bindings());

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

/// Registers Loupe like [`init`], starting from the app's own `settings`.
/// The user can still change them for the session, in the settings popover
/// or the palette.
///
/// ```ignore
/// gpui_inspector::init_with(
///     cx,
///     LoupeSettings {
///         editor: Editor::VsCode.into(),
///         budget: FrameBudget::Hz120,
///         ..LoupeSettings::default()
///     },
/// );
/// ```
pub fn init_with(cx: &mut App, settings: LoupeSettings) {
    cx.set_global(settings);
    init(cx);
}

/// Every key binding Loupe registers: the global shortcuts, then the ones
/// that apply inside Loupe (see `DESIGN.md`, "Keys").
pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    let mut bindings = vec![
        KeyBinding::new(commands::keys::TOGGLE, ToggleInspector, None),
        KeyBinding::new(commands::keys::PICK, TogglePick, None),
        KeyBinding::new(commands::keys::HOLD, ToggleHold, None),
    ];
    bindings.extend(loupe::key_bindings());
    bindings
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
