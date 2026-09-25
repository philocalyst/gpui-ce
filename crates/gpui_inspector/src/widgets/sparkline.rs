//! Sparklines: a tiny line + area chart of recent values, drawn with one canvas.

use crate::theme::Theme;
use gpui::{
    App, Bounds, ColorExt as _, Hsla, IntoElement, PathBuilder, Pixels, Point, RenderOnce, Styled,
    Window, canvas, point, px,
};
use std::sync::Arc;

/// Points of a sparkline over `bounds`: values spread evenly left to right,
/// scaled so `max` touches the top (values are clamped to `0..=max`). A
/// single value draws a flat line across.
pub fn sparkline_points(values: &[f32], max: f32, bounds: Bounds<Pixels>) -> Vec<Point<Pixels>> {
    let height = bounds.size.height;
    let y = |value: f32| {
        let ratio = if max > 0. {
            (value / max).clamp(0., 1.)
        } else {
            0.
        };
        bounds.bottom() - height * ratio
    };
    match values {
        [] => Vec::new(),
        [only] => vec![
            point(bounds.left(), y(*only)),
            point(bounds.right(), y(*only)),
        ],
        values => {
            let step = bounds.size.width / (values.len() - 1) as f32;
            values
                .iter()
                .enumerate()
                .map(|(ix, value)| point(bounds.left() + step * ix as f32, y(*value)))
                .collect()
        }
    }
}

/// A sparkline of `values`, oldest first.
#[derive(IntoElement)]
pub struct Sparkline {
    values: Arc<[f32]>,
    max: Option<f32>,
    color: Option<Hsla>,
    width: Pixels,
    height: Pixels,
}

impl Sparkline {
    /// A 64×16 sparkline scaled to the largest value.
    pub fn new(values: impl Into<Arc<[f32]>>) -> Self {
        Self {
            values: values.into(),
            max: None,
            color: None,
            width: px(64.),
            height: px(16.),
        }
    }

    /// Scales to a fixed maximum instead of the largest value.
    pub fn max(mut self, max: f32) -> Self {
        self.max = Some(max);
        self
    }

    /// Sets the line color (the area is a faint wash of it).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Sets the size.
    pub fn size(mut self, width: Pixels, height: Pixels) -> Self {
        self.width = width;
        self.height = height;
        self
    }
}

impl RenderOnce for Sparkline {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let color = self.color.unwrap_or(theme.colors.accent);
        let values = self.values;
        let max = self
            .max
            .unwrap_or_else(|| values.iter().copied().fold(0., f32::max));
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                // Keep the 1 px stroke inside the bounds.
                let inner = Bounds::from_corners(
                    point(bounds.left(), bounds.top() + px(1.)),
                    point(bounds.right(), bounds.bottom() - px(0.5)),
                );
                let points = sparkline_points(&values, max, inner);
                let (Some(first), Some(last)) = (points.first(), points.last()) else {
                    return;
                };
                let mut area = PathBuilder::fill();
                area.move_to(point(first.x, bounds.bottom()));
                for point in &points {
                    area.line_to(*point);
                }
                area.line_to(point(last.x, bounds.bottom()));
                area.close();
                if let Ok(area) = area.build() {
                    window.paint_path(area, color.opacity(0.16));
                }
                let mut line = PathBuilder::stroke(px(1.));
                line.move_to(*first);
                for point in &points[1..] {
                    line.line_to(*point);
                }
                if let Ok(line) = line.build() {
                    window.paint_path(line, color);
                }
            },
        )
        .flex_none()
        .w(self.width)
        .h(self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::sparkline_points;
    use gpui::{Bounds, point, px, size};

    fn bounds() -> Bounds<gpui::Pixels> {
        Bounds::new(point(px(10.), px(0.)), size(px(100.), px(20.)))
    }

    #[test]
    fn spreads_values_across_the_width_and_scales_to_max() {
        let points = sparkline_points(&[0., 5., 10.], 10., bounds());
        assert_eq!(
            points,
            [
                point(px(10.), px(20.)),
                point(px(60.), px(10.)),
                point(px(110.), px(0.))
            ]
        );
    }

    #[test]
    fn clamps_out_of_range_values_and_handles_degenerate_input() {
        let points = sparkline_points(&[-3., 30.], 10., bounds());
        assert_eq!(points[0].y, px(20.));
        assert_eq!(points[1].y, px(0.));
        assert!(sparkline_points(&[], 1., bounds()).is_empty());
        let flat = sparkline_points(&[4.], 0., bounds());
        assert_eq!(flat, [point(px(10.), px(20.)), point(px(110.), px(20.))]);
    }
}
