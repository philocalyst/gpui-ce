//! Section headers: an uppercase, tracked label with an optional trailing action.

use crate::theme::{Theme, UI_FONT};
use gpui::{
    AnyElement, App, FontWeight, IntoElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::*, px,
};

/// `CAUSES  3                                   [Copy]`
#[derive(IntoElement)]
pub struct SectionHeader {
    label: SharedString,
    detail: Option<SharedString>,
    actions: Vec<AnyElement>,
    rule: bool,
}

impl SectionHeader {
    /// A header titled `label` (shown uppercase).
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            actions: Vec::new(),
            rule: false,
        }
    }

    /// A short muted value after the label, such as a count.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Adds a trailing control, e.g. a small [`crate::widgets::Button`].
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// Draws a hairline above the header, to separate stacked sections.
    pub fn rule(mut self) -> Self {
        self.rule = true;
        self
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        div()
            .flex_none()
            .h(theme.metrics.property_row)
            .px(theme.metrics.gutter)
            .flex()
            .items_center()
            .gap_2()
            .when(self.rule, |this| {
                this.border_t_1().border_color(theme.colors.line)
            })
            .font_family(UI_FONT)
            .child(
                div()
                    .flex_none()
                    .text_size(theme.metrics.label)
                    .line_height(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .letter_spacing(px(0.6))
                    .text_color(theme.colors.text_muted)
                    .child(self.label.to_uppercase()),
            )
            .children(self.detail.map(|detail| {
                div()
                    .text_size(theme.metrics.text_small)
                    .text_color(theme.colors.text_faint)
                    .child(detail)
            }))
            .child(div().flex_1())
            .children(self.actions)
    }
}
