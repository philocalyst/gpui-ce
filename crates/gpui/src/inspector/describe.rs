//! Turns an element's resolved [`Style`] into the [`ElementDetails`] the
//! inspector shows: the box model in pixels and the layout inputs that
//! explain the element's size.

use super::model::{BoxModel, ElementDetails, LayoutFacts, SizeSpec};
use crate::{
    AbsoluteLength, Bounds, DefiniteLength, Display, FlexDirection, Length, Pixels, Position,
    SharedString, Size, Style,
};

/// Fills `details` from `style`, as laid out at `bounds`. Relative lengths
/// are resolved against the element's own size, as gpui resolves padding.
pub(crate) fn describe_style(
    details: &mut ElementDetails,
    style: &Style,
    bounds: Bounds<Pixels>,
    rem_size: Pixels,
) {
    let basis = Size {
        width: AbsoluteLength::Pixels(bounds.size.width),
        height: AbsoluteLength::Pixels(bounds.size.height),
    };
    details.box_model = Some(BoxModel {
        margin: style.margin.map(|length| match *length {
            Length::Definite(length) => length.to_pixels(basis.width, rem_size),
            Length::Auto => Pixels::ZERO,
        }),
        border: style.border_widths.to_pixels(rem_size),
        padding: style.padding.to_pixels(basis, rem_size),
    });
    details.layout = Some(layout_facts(style, basis, rem_size));
    details.background = style
        .background
        .as_ref()
        .and_then(|fill| fill.color())
        .and_then(|background| background.as_solid());
    details.border_color = style
        .border_color
        .and_then(|border_color| border_color.as_solid());
    let corners = style.corner_radii.to_pixels(rem_size);
    let corner_radius = corners
        .top_left
        .max(corners.top_right)
        .max(corners.bottom_right)
        .max(corners.bottom_left);
    details.corner_radius = (corner_radius > Pixels::ZERO).then_some(corner_radius);
    details.opacity = style.opacity.filter(|&opacity| opacity != 1.);
    if let Some(color) = style.text.color {
        details.text_color = Some(color);
    }
    if let Some(font_size) = style.text.font_size {
        details.font_size = Some(font_size.to_pixels(rem_size));
    }
    if let Some(font_family) = &style.text.font_family {
        details.font_family = Some(font_family.clone());
    }
}

fn layout_facts(style: &Style, basis: Size<AbsoluteLength>, rem_size: Pixels) -> LayoutFacts {
    let display = match style.display {
        Display::Flex => "flex",
        Display::Grid => "grid",
        Display::Block => "block",
        Display::None => "none",
    };
    let flex_direction = (style.display == Display::Flex).then(|| {
        SharedString::new_static(match style.flex_direction {
            FlexDirection::Row => "row",
            FlexDirection::Column => "column",
            FlexDirection::RowReverse => "row-reverse",
            FlexDirection::ColumnReverse => "column-reverse",
        })
    });
    let sizes = |size: Size<Length>| Size {
        width: size_spec(size.width, rem_size),
        height: size_spec(size.height, rem_size),
    };
    let gap = |length: DefiniteLength, basis: AbsoluteLength| length.to_pixels(basis, rem_size);
    LayoutFacts {
        display: SharedString::new_static(display),
        flex_direction,
        size: sizes(style.size),
        min_size: sizes(style.min_size),
        max_size: sizes(style.max_size),
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        flex_basis: size_spec(style.flex_basis, rem_size),
        gap: Size {
            width: gap(style.gap.width, basis.width),
            height: gap(style.gap.height, basis.height),
        },
        absolute: style.position == Position::Absolute,
        aspect_ratio: style.aspect_ratio,
    }
}

fn size_spec(length: Length, rem_size: Pixels) -> SizeSpec {
    match length {
        Length::Auto => SizeSpec::Auto,
        Length::Definite(DefiniteLength::Absolute(length)) => {
            SizeSpec::Pixels(length.to_pixels(rem_size))
        }
        Length::Definite(DefiniteLength::Relative(fraction)) => {
            SizeSpec::Fraction(fraction.as_f32())
        }
    }
}
