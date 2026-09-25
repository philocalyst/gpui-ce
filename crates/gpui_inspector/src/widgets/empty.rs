//! Empty states: what a pane would show, and how to get there.

use crate::{
    theme::{Theme, UI_FONT},
    widgets::{Icon, IconName},
};
use gpui::{
    AnyElement, App, IntoElement, RenderOnce, SharedString, Styled, Window, div, prelude::*, px,
};

/// A centered icon, title, explanation and optional action.
#[derive(IntoElement)]
pub struct EmptyState {
    icon: Option<IconName>,
    title: SharedString,
    description: Option<SharedString>,
    action: Option<AnyElement>,
}

impl EmptyState {
    /// An empty state titled `title`.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            title: title.into(),
            description: None,
            action: None,
        }
    }

    /// Shows an icon above the title.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Explains what would appear here and why it is empty.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// A control that fills the pane (e.g. "Start picking").
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        div()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1p5()
            .font_family(UI_FONT)
            .text_size(theme.metrics.text)
            .line_height(theme.metrics.line_height)
            .children(
                self.icon
                    .map(|icon| Icon::new(icon).size(px(20.)).color(theme.colors.text_faint)),
            )
            .child(
                div()
                    .text_color(theme.colors.text_muted)
                    .text_center()
                    .child(self.title),
            )
            .children(self.description.map(|description| {
                div()
                    .max_w(px(320.))
                    .text_size(theme.metrics.text_small)
                    .text_color(theme.colors.text_faint)
                    .text_center()
                    .child(description)
            }))
            .children(self.action.map(|action| div().pt_1().child(action)))
    }
}
