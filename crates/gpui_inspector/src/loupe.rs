//! The root view docked in the inspected window.

use gpui::{
    App, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window, div,
    inspector::InspectorEvent, rgb,
};

/// Loupe's root view: one per inspected window, alive while the dock is open.
pub struct Loupe {
    _inspector: Entity<gpui::Inspector>,
}

impl Loupe {
    pub(crate) fn new(
        inspector: Entity<gpui::Inspector>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&inspector, |_, _, _: &InspectorEvent, cx| cx.notify())
            .detach();
        Self {
            _inspector: inspector,
        }
    }
}

impl Render for Loupe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x16171a))
            .text_color(rgb(0xd4d7dd))
            .font_family(crate::UI_FONT)
            .child("Loupe")
    }
}

pub(crate) fn bind_keys(_cx: &mut App) {}

pub(crate) fn toggle_pick(window: &mut Window) {
    let picking = window
        .inspector_capture()
        .is_some_and(|capture| capture.pick().active);
    if picking {
        window.stop_inspector_pick();
    } else {
        window.start_inspector_pick();
    }
}
