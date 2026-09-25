//! The box model diagram: margin, border, padding and content, nested as
//! the web draws them, with every edge's value in place. In the live tree
//! each value is a scrub field that writes a style override.

use super::ElementsLens;
use crate::{
    analysis::{contrast::Srgb, format},
    theme::{MONO_FONT, Theme, UI_FONT},
    widgets::{ScrubChanged, ScrubField},
};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, Focusable as _, Hsla, IntoElement, Pixels, Rgba,
    Size, Styled, Subscription, Window, div, inspector::BoxModel, prelude::*, px,
};
use std::collections::HashMap;

/// A ring of the box model, outside in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Layer {
    /// Outside the border.
    Margin,
    /// The border itself.
    Border,
    /// Between the border and the content.
    Padding,
}

impl Layer {
    /// Outside in.
    pub const ALL: [Layer; 3] = [Layer::Margin, Layer::Border, Layer::Padding];

    fn label(self) -> &'static str {
        match self {
            Layer::Margin => "margin",
            Layer::Border => "border",
            Layer::Padding => "padding",
        }
    }

    /// The style property the layer's edges are written to.
    fn property(self) -> &'static str {
        match self {
            Layer::Margin => "margin",
            Layer::Border => "border_widths",
            Layer::Padding => "padding",
        }
    }

    fn color(self, theme: &Theme) -> Hsla {
        let colors = &theme.colors;
        match self {
            Layer::Margin => colors.box_margin,
            Layer::Border => colors.box_border,
            Layer::Padding => colors.box_padding,
        }
    }
}

/// An edge of a box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Side {
    /// Top.
    Top,
    /// Right.
    Right,
    /// Bottom.
    Bottom,
    /// Left.
    Left,
}

impl Side {
    /// Clockwise from the top.
    pub const ALL: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];

    fn name(self) -> &'static str {
        match self {
            Side::Top => "top",
            Side::Right => "right",
            Side::Bottom => "bottom",
            Side::Left => "left",
        }
    }
}

/// The style pointer an edge edits: `/padding/top`.
pub(super) fn pointer(layer: Layer, side: Side) -> String {
    format!("/{}/{}", layer.property(), side.name())
}

/// The resolved width of an edge.
pub(super) fn edge(box_model: &BoxModel, layer: Layer, side: Side) -> Pixels {
    let edges = match layer {
        Layer::Margin => &box_model.margin,
        Layer::Border => &box_model.border,
        Layer::Padding => &box_model.padding,
    };
    match side {
        Side::Top => edges.top,
        Side::Right => edges.right,
        Side::Bottom => edges.bottom,
        Side::Left => edges.left,
    }
}

/// The content box of a border box `size`: minus padding and border.
pub(super) fn content_size(size: Size<Pixels>, box_model: &BoxModel) -> Size<Pixels> {
    let inset =
        |layer: Layer, a: Side, b: Side| edge(box_model, layer, a) + edge(box_model, layer, b);
    Size {
        width: (size.width
            - inset(Layer::Padding, Side::Left, Side::Right)
            - inset(Layer::Border, Side::Left, Side::Right))
        .max(Pixels::ZERO),
        height: (size.height
            - inset(Layer::Padding, Side::Top, Side::Bottom)
            - inset(Layer::Border, Side::Top, Side::Bottom))
        .max(Pixels::ZERO),
    }
}

/// `color` composited over `under`, as an opaque color, so nested
/// translucent layers keep their own hue instead of mixing.
fn opaque_over(color: Hsla, under: Hsla) -> Hsla {
    let blended = Srgb::from_hsla(under).under(color);
    gpui::rgb_to_hsla(Rgba::new(blended.red, blended.green, blended.blue, 1.))
}

/// The twelve scrub fields of the diagram, one per layer and side.
pub(super) struct BoxFields {
    fields: HashMap<(Layer, Side), Entity<ScrubField>>,
    _changed: Vec<Subscription>,
}

impl BoxFields {
    /// Creates the fields; edits go through [`ElementsLens::edit`].
    pub fn new(window: &mut Window, cx: &mut Context<ElementsLens>) -> Self {
        let mut fields = HashMap::new();
        let mut changed = Vec::new();
        for layer in Layer::ALL {
            for side in Side::ALL {
                let field = cx.new(|cx| ScrubField::new(0., 1., cx).with_precision(1).compact());
                let pointer = pointer(layer, side);
                changed.push(cx.subscribe_in(
                    &field,
                    window,
                    move |this, _, event: &ScrubChanged, window, cx| {
                        this.edit(
                            &pointer,
                            crate::analysis::style_grid::PropertyValue::Pixels(event.0),
                            window,
                            cx,
                        )
                    },
                ));
                fields.insert((layer, side), field);
            }
        }
        Self {
            fields,
            _changed: changed,
        }
    }

    /// Shows `box_model`'s values, except in a field the user is editing.
    pub fn sync(&self, box_model: &BoxModel, window: &Window, cx: &mut Context<ElementsLens>) {
        for (&(layer, side), field) in &self.fields {
            if field.focus_handle(cx).contains_focused(window, cx) {
                continue;
            }
            let value = f32::from(edge(box_model, layer, side));
            field.update(cx, |field, cx| field.set_value(value, cx));
        }
    }

    fn field(&self, layer: Layer, side: Side) -> Option<&Entity<ScrubField>> {
        self.fields.get(&(layer, side))
    }
}

impl ElementsLens {
    /// The diagram for an element of `size` with `box_model`; editable
    /// values are scrub fields.
    pub(super) fn render_box_model(
        &self,
        size: Size<Pixels>,
        box_model: &BoxModel,
        editable: bool,
        theme: &'static Theme,
    ) -> AnyElement {
        let colors = &theme.colors;
        let value = |layer: Layer, side: Side| -> AnyElement {
            let field = self.box_fields.field(layer, side).filter(|_| editable);
            match field {
                Some(field) => div()
                    .flex_none()
                    .debug_selector(|| format!("elements-box-{}-{}", layer.label(), side.name()))
                    .child(field.clone())
                    .into_any_element(),
                None => {
                    let pixels = edge(box_model, layer, side);
                    div()
                        .flex_none()
                        .min_w(px(22.))
                        .h(theme.metrics.control_small)
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(MONO_FONT)
                        .text_size(theme.metrics.mono)
                        .text_color(if pixels == Pixels::ZERO {
                            colors.text_faint
                        } else {
                            colors.text
                        })
                        .child(format::pixels(pixels))
                        .into_any_element()
                }
            }
        };

        let content = content_size(size, box_model);
        let mut inner = div()
            .min_w(px(64.))
            .h(px(24.))
            .px_2()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(2.))
            .bg(opaque_over(colors.box_content, colors.bg))
            .font_family(MONO_FONT)
            .text_size(theme.metrics.mono)
            .text_color(colors.text)
            .child(format::size(content))
            .into_any_element();
        for layer in Layer::ALL.into_iter().rev() {
            let fill = opaque_over(layer.color(theme), colors.bg);
            inner = div()
                .flex()
                .flex_col()
                .rounded(px(3.))
                .bg(fill)
                .border_1()
                .border_color(colors.line_strong)
                .border_dashed()
                .child(
                    div()
                        .relative()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .absolute()
                                .left(px(4.))
                                .top(px(1.))
                                .font_family(UI_FONT)
                                .text_size(px(9.5))
                                .text_color(colors.text_muted)
                                .child(layer.label()),
                        )
                        .child(value(layer, Side::Top)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .w(px(30.))
                                .flex()
                                .justify_center()
                                .child(value(layer, Side::Left)),
                        )
                        .child(div().flex_1().child(inner))
                        .child(
                            div()
                                .w(px(30.))
                                .flex()
                                .justify_center()
                                .child(value(layer, Side::Right)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .child(value(layer, Side::Bottom)),
                )
                .into_any_element();
        }
        div()
            .px(theme.metrics.gutter)
            .pb_1()
            .flex()
            .justify_center()
            .child(div().w_full().max_w(px(380.)).child(inner))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Edges, size};

    #[test]
    fn content_is_the_border_box_minus_padding_and_border() {
        let box_model = BoxModel {
            margin: Edges::all(px(40.)),
            border: Edges::all(px(1.)),
            padding: Edges {
                top: px(8.),
                right: px(12.),
                bottom: px(8.),
                left: px(12.),
            },
        };
        assert_eq!(
            content_size(size(px(120.), px(32.)), &box_model),
            size(px(94.), px(14.))
        );
        assert_eq!(
            content_size(size(px(10.), px(10.)), &box_model),
            size(px(0.), px(0.)),
            "never negative"
        );
    }

    #[test]
    fn edges_edit_their_style_property() {
        assert_eq!(pointer(Layer::Padding, Side::Top), "/padding/top");
        assert_eq!(pointer(Layer::Border, Side::Left), "/border_widths/left");
        assert_eq!(pointer(Layer::Margin, Side::Bottom), "/margin/bottom");
    }
}
