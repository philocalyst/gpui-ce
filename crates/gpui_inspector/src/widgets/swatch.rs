//! Color swatches: a small square of a color, over a checkerboard when translucent.

use crate::theme::Theme;
use gpui::{
    App, Hsla, IntoElement, Pixels, RenderOnce, Styled, Window, checkerboard, div, prelude::*, px,
};

/// A color chip, e.g. next to a `bg` value in the style grid.
#[derive(IntoElement)]
pub struct ColorSwatch {
    color: Hsla,
    size: Pixels,
}

impl ColorSwatch {
    /// A 12 px swatch of `color`.
    pub fn new(color: Hsla) -> Self {
        Self {
            color,
            size: px(12.),
        }
    }

    /// Sets the square size.
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for ColorSwatch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let translucent = self.color.alpha < 1.;
        div()
            .flex_none()
            .size(self.size)
            .rounded(px(2.))
            .overflow_hidden()
            .border_1()
            .border_color(theme.colors.line_strong)
            .when(translucent, |this| {
                this.bg(checkerboard(theme.colors.line_strong, 4.))
            })
            .child(div().size_full().bg(self.color))
    }
}
