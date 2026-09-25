//! Loupe's widget kit: small, generic, themed building blocks.
//!
//! Every widget reads its look from [`crate::theme::Theme::of`], so it matches
//! the window's appearance and the user's density. Stateless widgets are
//! `RenderOnce` builders; the stateful ones ([`TreeState`], [`TableState`],
//! [`ScrubField`]) are entities that emit events to their owner.

mod button;
mod empty;
mod icon;
mod kbd;
mod pill;
mod prose;
mod scrub;
mod section;
mod segmented;
mod sparkline;
mod splitter;
mod swatch;
mod tab_rail;
mod table;
mod text_field;
mod tooltip;
mod tree;

pub use button::{Button, ButtonSize, ButtonStyle};
pub use empty::EmptyState;
pub use icon::{Icon, IconName};
pub use kbd::{Kbd, format_keys};
pub use pill::{Pill, Tone};
pub use prose::Prose;
pub use scrub::{ScrubChanged, ScrubField, format_number, nudge, parse_number, scrub_value};
pub use section::SectionHeader;
pub use segmented::{Segment, Segmented};
pub use sparkline::{Sparkline, sparkline_points};
pub use splitter::{Split, SplitDrag, SplitHandle, clamp_split};
pub use swatch::ColorSwatch;
pub use tab_rail::{RailTab, TabRail};
pub use table::{
    Column, ColumnWidth, SortDirection, Table, TableEvent, TableModel, TableState, sort_order,
};
pub use text_field::{TextField, text_field_state};
pub use tooltip::{Tooltip, floating_surface};
pub use tree::{Tree, TreeEvent, TreeModel, TreeNode, TreeRow, TreeState, flatten};

use gpui::{App, KeyBinding, actions};
use gpui_elements::editable_text::actions::default_bindings;

/// Key context of focused trees and tables.
pub const LIST_CONTEXT: &str = "LoupeList";
/// Key context of a focused scrub field.
pub const SCRUB_CONTEXT: &str = "LoupeScrub";

actions!(
    loupe_list,
    [
        /// Selects the next row.
        SelectNext,
        /// Selects the previous row.
        SelectPrevious,
        /// Selects the first row.
        SelectFirst,
        /// Selects the last row.
        SelectLast,
        /// Expands the selected tree node, or steps into it.
        ExpandSelected,
        /// Collapses the selected tree node, or steps out to its parent.
        CollapseSelected,
        /// Opens the selected row.
        ActivateSelected,
    ]
);

actions!(
    loupe_scrub,
    [
        /// Adds one step.
        ScrubIncrement,
        /// Subtracts one step.
        ScrubDecrement,
        /// Adds ten steps.
        ScrubIncrementCoarse,
        /// Subtracts ten steps.
        ScrubDecrementCoarse,
    ]
);

/// Binds the widgets' keys: list navigation, scrubbing, and editable text
/// inside `root_context` (so Loupe's text fields work even if the app never
/// bound `gpui_elements`' defaults).
pub(crate) fn bind_keys(root_context: &str, cx: &mut App) {
    let list = Some(LIST_CONTEXT);
    let scrub = Some(SCRUB_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, list),
        KeyBinding::new("j", SelectNext, list),
        KeyBinding::new("up", SelectPrevious, list),
        KeyBinding::new("k", SelectPrevious, list),
        KeyBinding::new("home", SelectFirst, list),
        KeyBinding::new("end", SelectLast, list),
        KeyBinding::new("right", ExpandSelected, list),
        KeyBinding::new("left", CollapseSelected, list),
        KeyBinding::new("enter", ActivateSelected, list),
        KeyBinding::new("up", ScrubIncrement, scrub),
        KeyBinding::new("down", ScrubDecrement, scrub),
        KeyBinding::new("shift-up", ScrubIncrementCoarse, scrub),
        KeyBinding::new("shift-down", ScrubDecrementCoarse, scrub),
    ]);
    let text_context = format!("{root_context} > EditableText");
    cx.bind_keys(default_bindings().as_keybindings(Some(&text_context)));
}
