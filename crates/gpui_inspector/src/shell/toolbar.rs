//! The toolbar: pick, overlay toggles, freeze, the palette entry and the dock.

use crate::{
    commands::{Command, DockSide, OVERLAYS, keys},
    theme::Theme,
    widgets::{Button, IconName, Kbd, Tooltip},
};
use gpui::{
    App, IntoElement, RenderOnce, Styled, Window, div, inspector::OverlayModes, prelude::*, px,
};
use std::rc::Rc;

/// Runs a command on the Loupe that rendered the control.
pub(crate) type Run = Rc<dyn Fn(Command, &mut Window, &mut App)>;

/// Docks narrower than this drop the toolbar's text labels.
const COMPACT_BELOW: gpui::Pixels = px(440.);
/// The search field shows its shortcut chip from this toolbar width.
const SEARCH_KEYS_FROM: gpui::Pixels = px(600.);

/// The icon and a slug (for debug selectors) of an overlay toggle.
fn overlay_meta(mode: OverlayModes) -> (IconName, &'static str) {
    if mode == OverlayModes::OUTLINES {
        (IconName::Outline, "outlines")
    } else if mode == OverlayModes::PAINT_FLASH {
        (IconName::Flash, "paint-flash")
    } else if mode == OverlayModes::HITBOXES {
        (IconName::Hitbox, "hitboxes")
    } else if mode == OverlayModes::SLOW_FRAMES {
        (IconName::Warning, "slow-frames")
    } else if mode == OverlayModes::OVERFLOW {
        (IconName::Overflow, "overflow")
    } else {
        (IconName::BoxModel, "box-model")
    }
}

/// Debug selector of an overlay toggle, e.g. `loupe-overlay-hitboxes`.
pub(crate) fn overlay_selector(mode: OverlayModes) -> String {
    format!("loupe-overlay-{}", overlay_meta(mode).1)
}

/// What the toolbar shows.
#[derive(IntoElement)]
pub(crate) struct Toolbar {
    pub picking: bool,
    pub overlays: OverlayModes,
    pub frozen: bool,
    pub holding: bool,
    pub dock: DockSide,
    pub width: gpui::Pixels,
    pub run: Run,
}

impl RenderOnce for Toolbar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let compact = self.width < COMPACT_BELOW;
        let run = self.run;
        let command = |command: Command| {
            let run = run.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| run(command, window, cx)
        };
        let separator = || {
            div()
                .flex_none()
                .w(px(1.))
                .h(px(16.))
                .mx(px(4.))
                .bg(colors.line)
        };

        let pick = Button::new("loupe-pick")
            .icon(IconName::Pick)
            .when(!compact, |this| this.label("Pick"))
            .toggle_state(self.picking)
            .tooltip_keys(Command::TogglePick.label(), keys::PICK)
            .on_click(command(Command::TogglePick));

        let overlays = OVERLAYS.map(|mode| {
            Button::new(overlay_selector(mode))
                .icon(overlay_meta(mode).0)
                .toggle_state(self.overlays.contains(mode))
                .tooltip(Command::ToggleOverlay(mode).label())
                .on_click(command(Command::ToggleOverlay(mode)))
        });

        let freeze = Button::new("loupe-freeze")
            .icon(IconName::Pause)
            .when(!compact, |this| this.label("Freeze"))
            .toggle_state(self.frozen)
            .tooltip_keys(
                if self.frozen {
                    "Resume recording"
                } else {
                    Command::ToggleFreeze.label()
                },
                keys::FREEZE,
            )
            .on_click(command(Command::ToggleFreeze));

        // Icon only: the pin reads as "hold", and the status bar says HELD.
        let hold = Button::new("loupe-hold")
            .icon(IconName::Hold)
            .toggle_state(self.holding)
            .tooltip_keys(
                if self.holding {
                    "Release the app"
                } else {
                    Command::ToggleHold.label()
                },
                keys::HOLD,
            )
            .on_click(command(Command::ToggleHold));

        let search = div()
            .id("loupe-search")
            .debug_selector(|| "loupe-search".into())
            .flex_1()
            .min_w(px(28.))
            .max_w(px(400.))
            .h(theme.metrics.control)
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .overflow_hidden()
            .rounded(theme.metrics.radius)
            .bg(colors.bg)
            .border_1()
            .border_color(colors.line_strong)
            .cursor_pointer()
            .hover(|style| style.border_color(colors.text_faint))
            .text_color(colors.text_faint)
            .child(
                crate::widgets::Icon::new(IconName::Search)
                    .size(theme.metrics.icon_small)
                    .color(colors.text_faint),
            )
            .child(div().flex_1().min_w_0().truncate().child(if compact {
                "Find…"
            } else {
                "Find anything…"
            }))
            // The shortcut chip needs room the placeholder deserves more;
            // the tooltip always carries it.
            .when(self.width >= SEARCH_KEYS_FROM, |this| {
                this.child(Kbd::new(keys::PALETTE))
            })
            .tooltip(Tooltip::with_keys(
                "Commands, elements, entities",
                keys::PALETTE,
            ))
            .on_click(command(Command::OpenPalette));

        // One toggle that moves the dock to the other edge.
        let (dock_icon, target) = match self.dock {
            DockSide::Right => (IconName::DockBottom, DockSide::Bottom),
            DockSide::Bottom => (IconName::DockRight, DockSide::Right),
        };
        let dock = Button::new("loupe-dock")
            .icon(dock_icon)
            .tooltip(Command::Dock(target).label())
            .on_click(command(Command::Dock(target)));

        let close = Button::new("loupe-close")
            .icon(IconName::Close)
            .tooltip_keys(Command::Close.label(), keys::TOGGLE)
            .on_click(command(Command::Close));

        div()
            .flex_none()
            .h(theme.metrics.toolbar)
            .w_full()
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(2.))
            .overflow_hidden()
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.line)
            .child(pick)
            .child(separator())
            .children(overlays)
            .child(separator())
            .child(freeze)
            .child(hold)
            .child(search.mx(px(4.)))
            // Takes whatever the capped search field leaves, keeping the
            // dock and close buttons at the trailing edge.
            .child(div().ml_auto().child(dock))
            .child(close)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_selectors_are_readable() {
        assert_eq!(
            overlay_selector(OverlayModes::PAINT_FLASH),
            "loupe-overlay-paint-flash"
        );
        assert_eq!(
            overlay_selector(OverlayModes::BOX_MODEL),
            "loupe-overlay-box-model"
        );
    }
}
