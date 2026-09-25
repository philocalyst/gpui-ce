//! Tooltips: a floating label with an optional key hint and secondary line.

use crate::{
    theme::{Theme, UI_FONT},
    widgets::Kbd,
};
use gpui::{
    AnyView, App, AppContext as _, BoxShadow, Context, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, px,
};

/// A tooltip view. Build one through [`Tooltip::text`] or [`Tooltip::with_keys`]
/// and hand the result to `.tooltip(...)`.
pub struct Tooltip {
    title: SharedString,
    keys: Option<SharedString>,
    meta: Option<SharedString>,
}

impl Tooltip {
    /// A builder for a plain text tooltip.
    pub fn text(title: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        Self::build(title.into(), None, None)
    }

    /// A builder for a tooltip with a key hint (e.g. `"cmd-k"`), shown as [`Kbd`].
    pub fn with_keys(
        title: impl Into<SharedString>,
        keys: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        Self::build(title.into(), Some(keys.into()), None)
    }

    /// A builder for a tooltip with a secondary, muted line.
    pub fn with_meta(
        title: impl Into<SharedString>,
        meta: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        Self::build(title.into(), None, Some(meta.into()))
    }

    /// A builder from optional parts.
    pub fn build(
        title: SharedString,
        keys: Option<SharedString>,
        meta: Option<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        move |_, cx| {
            let tooltip = Tooltip {
                title: title.clone(),
                keys: keys.clone(),
                meta: meta.clone(),
            };
            cx.new(|_| tooltip).into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        floating_surface(theme)
            .px_2()
            .py_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(theme.colors.text)
                    .child(self.title.clone())
                    .children(self.keys.clone().map(Kbd::new)),
            )
            .children(
                self.meta
                    .clone()
                    .map(|meta| div().text_color(theme.colors.text_muted).child(meta)),
            )
    }
}

/// The shared look of floating layers (tooltips, palette, pop-overs):
/// raised surface, strong hairline, 8 px corners and a soft shadow.
pub fn floating_surface(theme: &Theme) -> gpui::Div {
    div()
        .font_family(UI_FONT)
        .text_size(theme.metrics.text_small)
        .line_height(theme.metrics.line_height)
        .bg(theme.colors.surface_2)
        .border_1()
        .border_color(theme.colors.line_strong)
        .rounded(theme.metrics.radius_floating)
        .shadow(vec![
            BoxShadow::new(px(0.), px(4.), theme.colors.shadow).blur_radius(px(16.)),
        ])
}
