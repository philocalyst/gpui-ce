//! Every action Loupe can take, as data: the toolbar, the key bindings and
//! the palette all go through [`Command`], so each has one implementation
//! (`Loupe::run`) and one label.

use crate::{
    analysis::source::Editor,
    settings::{CAPTURE_LEVELS, FrameBudget, LoupeSettings},
    state::{Lens, LoupeState},
    theme::{Appearance, Density},
};
use gpui::inspector::{CaptureLevel, InspectorCapture, InspectorDock, OverlayModes};

/// A dock edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSide {
    /// A column on the right.
    Right,
    /// A row along the bottom.
    Bottom,
}

impl DockSide {
    /// The side of `dock`.
    pub fn of(dock: InspectorDock) -> Self {
        match dock {
            InspectorDock::Right { .. } | InspectorDock::Hidden => DockSide::Right,
            InspectorDock::Bottom { .. } => DockSide::Bottom,
        }
    }
}

/// Something the user can ask Loupe to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Start or stop picking an element in the app.
    TogglePick,
    /// Toggle one overlay mode.
    ToggleOverlay(OverlayModes),
    /// Freeze or resume recording.
    ToggleFreeze,
    /// Hold the app still, or release it.
    ToggleHold,
    /// Hold the app after a short delay, so a hover menu can be opened first.
    HoldSoon,
    /// Dock on a side.
    Dock(DockSide),
    /// Switch between the right and bottom docks.
    ToggleDock,
    /// Show a lens.
    ShowLens(Lens),
    /// Drop every recorded frame and input record.
    ClearRecording,
    /// Choose the color theme.
    SetAppearance(Appearance),
    /// Choose the row and control sizing.
    SetDensity(Density),
    /// Choose the editor source links open in.
    SetEditor(Editor),
    /// Choose the refresh rate frames are graded against.
    SetBudget(FrameBudget),
    /// Choose what each frame records.
    SetCaptureLevel(CaptureLevel),
    /// Open the command palette.
    OpenPalette,
    /// Open or close the settings popover.
    ToggleSettings,
    /// Show or hide the keyboard shortcuts.
    ToggleHelp,
    /// Close Loupe.
    Close,
}

/// The overlay toggles, in toolbar order.
pub const OVERLAYS: [OverlayModes; 6] = [
    OverlayModes::OUTLINES,
    OverlayModes::PAINT_FLASH,
    OverlayModes::HITBOXES,
    OverlayModes::SLOW_FRAMES,
    OverlayModes::OVERFLOW,
    OverlayModes::BOX_MODEL,
];

/// Key strokes, in gpui syntax, for the commands that have bindings.
pub(crate) mod keys {
    /// Opens the palette.
    pub const PALETTE: &str = if cfg!(target_os = "macos") {
        "cmd-k"
    } else {
        "ctrl-k"
    };
    /// Toggles picking.
    pub const PICK: &str = if cfg!(target_os = "macos") {
        "cmd-shift-c"
    } else {
        "ctrl-shift-c"
    };
    /// Toggles Loupe.
    pub const TOGGLE: &str = if cfg!(target_os = "macos") {
        "cmd-alt-i"
    } else {
        "ctrl-shift-i"
    };
    /// Freezes recording.
    pub const FREEZE: &str = "space";
    /// Holds the app; global, since the pointer is usually over the app.
    pub const HOLD: &str = if cfg!(target_os = "macos") {
        "cmd-shift-h"
    } else {
        "ctrl-shift-h"
    };
    /// Opens the settings popover.
    pub const SETTINGS: &str = if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    };
    /// Shows the keyboard shortcuts.
    pub const HELP: &str = "?";
    /// Shows a lens, by rail position.
    pub const LENSES: [&str; 5] = ["alt-1", "alt-2", "alt-3", "alt-4", "alt-5"];
}

impl Command {
    /// Every command the palette offers, in a sensible browsing order.
    pub fn all() -> Vec<Command> {
        let mut commands = vec![
            Command::TogglePick,
            Command::ToggleFreeze,
            Command::ToggleHold,
            Command::HoldSoon,
        ];
        commands.extend(Lens::ALL.map(Command::ShowLens));
        commands.extend(OVERLAYS.map(Command::ToggleOverlay));
        commands.extend([
            Command::Dock(DockSide::Right),
            Command::Dock(DockSide::Bottom),
            Command::ClearRecording,
            Command::ToggleSettings,
            Command::ToggleHelp,
            Command::SetAppearance(Appearance::System),
            Command::SetAppearance(Appearance::Dark),
            Command::SetAppearance(Appearance::Light),
            Command::SetDensity(Density::Compact),
            Command::SetDensity(Density::Comfortable),
        ]);
        commands.extend(Editor::ALL.map(Command::SetEditor));
        commands.extend(FrameBudget::ALL.map(Command::SetBudget));
        commands.extend(CAPTURE_LEVELS.map(Command::SetCaptureLevel));
        commands.push(Command::Close);
        commands
    }

    /// The command's name, as the palette and tooltips show it.
    pub fn label(self) -> &'static str {
        match self {
            Command::TogglePick => "Pick an element",
            Command::ToggleOverlay(mode) => overlay_label(mode),
            Command::ToggleFreeze => "Freeze recording",
            Command::ToggleHold => "Hold the app",
            Command::HoldSoon => "Hold the app in 3 seconds",
            Command::Dock(DockSide::Right) => "Dock right",
            Command::Dock(DockSide::Bottom) => "Dock bottom",
            Command::ToggleDock => "Move dock",
            Command::ShowLens(Lens::Elements) => "Show Elements",
            Command::ShowLens(Lens::Frames) => "Show Frames",
            Command::ShowLens(Lens::Events) => "Show Events",
            Command::ShowLens(Lens::Entities) => "Show Entities",
            Command::ShowLens(Lens::Audit) => "Show Audit",
            Command::ClearRecording => "Clear recording",
            Command::SetAppearance(Appearance::System) => "Theme: follow window",
            Command::SetAppearance(Appearance::Dark) => "Theme: dark",
            Command::SetAppearance(Appearance::Light) => "Theme: light",
            Command::SetDensity(Density::Compact) => "Density: compact",
            Command::SetDensity(Density::Comfortable) => "Density: comfortable",
            Command::SetEditor(Editor::Zed) => "Editor: Zed",
            Command::SetEditor(Editor::VsCode) => "Editor: VS Code",
            Command::SetEditor(Editor::Cursor) => "Editor: Cursor",
            Command::SetEditor(Editor::Idea) => "Editor: IntelliJ",
            Command::SetBudget(FrameBudget::Hz60) => "Frame budget: 60 Hz",
            Command::SetBudget(FrameBudget::Hz120) => "Frame budget: 120 Hz",
            Command::SetBudget(FrameBudget::Hz144) => "Frame budget: 144 Hz",
            Command::SetCaptureLevel(CaptureLevel::Frames) => "Capture level: Frames",
            Command::SetCaptureLevel(CaptureLevel::Tree) => "Capture level: Tree",
            Command::SetCaptureLevel(CaptureLevel::Full) => "Capture level: Full",
            Command::OpenPalette => "Find anything",
            Command::ToggleSettings => "Settings",
            Command::ToggleHelp => "Keyboard shortcuts",
            Command::Close => "Close Loupe",
        }
    }

    /// The key binding, in gpui syntax, if the command has one.
    pub fn keys(self) -> Option<&'static str> {
        match self {
            Command::TogglePick => Some(keys::PICK),
            Command::ToggleFreeze => Some(keys::FREEZE),
            Command::ToggleHold => Some(keys::HOLD),
            Command::ShowLens(lens) => Some(keys::LENSES[lens.index()]),
            Command::OpenPalette => Some(keys::PALETTE),
            Command::ToggleSettings => Some(keys::SETTINGS),
            Command::ToggleHelp => Some(keys::HELP),
            Command::Close => Some(keys::TOGGLE),
            _ => None,
        }
    }

    /// For toggles and choices: whether the command's state is currently on.
    pub fn is_on(
        self,
        capture: &InspectorCapture,
        state: &LoupeState,
        settings: &LoupeSettings,
    ) -> Option<bool> {
        match self {
            Command::TogglePick => Some(capture.pick().active),
            Command::ToggleOverlay(mode) => Some(capture.overlay().modes.contains(mode)),
            Command::ToggleFreeze => Some(capture.is_frozen()),
            Command::ToggleHold => Some(capture.is_holding()),
            Command::Dock(side) => Some(DockSide::of(capture.dock()) == side),
            Command::ShowLens(lens) => Some(state.lens() == lens),
            Command::SetAppearance(appearance) => Some(settings.appearance == appearance),
            Command::SetDensity(density) => Some(settings.density == density),
            Command::SetEditor(editor) => Some(settings.editor.editor() == Some(editor)),
            Command::SetBudget(budget) => Some(settings.budget == budget),
            Command::SetCaptureLevel(level) => Some(settings.capture_level == level),
            Command::ToggleDock
            | Command::HoldSoon
            | Command::ClearRecording
            | Command::OpenPalette
            | Command::ToggleSettings
            | Command::ToggleHelp
            | Command::Close => None,
        }
    }

    /// Applies a settings choice to `settings`. Returns false, changing
    /// nothing, for commands that are not settings.
    pub fn apply_to(self, settings: &mut LoupeSettings) -> bool {
        match self {
            Command::SetAppearance(appearance) => settings.appearance = appearance,
            Command::SetDensity(density) => settings.density = density,
            Command::SetEditor(editor) => settings.editor = editor.into(),
            Command::SetBudget(budget) => settings.budget = budget,
            Command::SetCaptureLevel(level) => settings.capture_level = level,
            _ => return false,
        }
        true
    }
}

fn overlay_label(mode: OverlayModes) -> &'static str {
    if mode == OverlayModes::OUTLINES {
        "Outline elements"
    } else if mode == OverlayModes::PAINT_FLASH {
        "Flash repaints"
    } else if mode == OverlayModes::HITBOXES {
        "Show hitboxes"
    } else if mode == OverlayModes::SLOW_FRAMES {
        "Flag slow frames"
    } else if mode == OverlayModes::OVERFLOW {
        "Show overflow"
    } else if mode == OverlayModes::BOX_MODEL {
        "Show box model"
    } else {
        "Toggle overlay"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_a_distinct_label() {
        let commands = Command::all();
        let mut labels: Vec<_> = commands.iter().map(|command| command.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), commands.len());
    }

    #[test]
    fn every_toolbar_toggle_and_lens_is_a_command() {
        let commands = Command::all();
        for mode in OVERLAYS {
            assert!(commands.contains(&Command::ToggleOverlay(mode)));
        }
        for lens in Lens::ALL {
            assert!(commands.contains(&Command::ShowLens(lens)));
        }
        assert!(commands.contains(&Command::TogglePick));
        assert!(commands.contains(&Command::ToggleFreeze));
    }

    #[test]
    fn every_setting_choice_is_a_command_that_sets_it() {
        let settings_commands: Vec<Command> = Command::all()
            .into_iter()
            .filter(|command| command.apply_to(&mut LoupeSettings::default()))
            .collect();
        let choices = 3 + 2 + Editor::ALL.len() + FrameBudget::ALL.len() + CAPTURE_LEVELS.len();
        assert_eq!(settings_commands.len(), choices);

        let capture = InspectorCapture::new_for_test();
        let state = LoupeState::new();
        for command in settings_commands {
            let mut settings = LoupeSettings::default();
            command.apply_to(&mut settings);
            let on = command.is_on(&capture, &state, &settings);
            assert_eq!(on, Some(true), "{command:?}");
        }
        let mut settings = LoupeSettings::default();
        assert!(!Command::ToggleHelp.apply_to(&mut settings));
        assert_eq!(settings, LoupeSettings::default());
    }
}
