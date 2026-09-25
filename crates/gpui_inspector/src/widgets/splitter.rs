//! Splitters: drag a hairline to resize two panes.
//!
//! [`Split`] lays out two panes with a draggable [`SplitHandle`] between them
//! and reports the new size of the first pane, clamped by [`clamp_split`] so
//! both panes keep their minimum size. A bare [`SplitHandle`] can also resize
//! something else (Loupe's dock edge): listen for [`SplitDrag`] moves with
//! `on_drag_move` on the element whose bounds the drag is relative to.

use crate::theme::Theme;
use gpui::{
    Along as _, AnyElement, App, AppContext as _, Axis, ColorExt as _, DragMoveEvent, Empty,
    IntoElement, Pixels, RenderOnce, SharedString, Styled, Window, div, prelude::*, px,
};
use std::rc::Rc;

/// The value carried while a [`SplitHandle`] is dragged.
#[derive(Clone, Debug)]
pub struct SplitDrag {
    /// Id of the handle being dragged.
    pub id: SharedString,
}

/// The size of the first pane when the handle is dragged to `desired`,
/// keeping the first pane at least `min_first` and the second at least
/// `min_second` wide. When both minimums cannot fit, the first pane wins.
pub fn clamp_split(
    desired: Pixels,
    total: Pixels,
    min_first: Pixels,
    min_second: Pixels,
) -> Pixels {
    let max_first = (total - min_second).max(min_first);
    desired.max(min_first).min(max_first)
}

/// A hairline that can be dragged. `Axis::Horizontal` separates panes laid
/// out side by side (the line is vertical); `Axis::Vertical` separates stacked
/// panes.
#[derive(IntoElement)]
pub struct SplitHandle {
    id: SharedString,
    axis: Axis,
}

impl SplitHandle {
    /// A handle whose drags carry [`SplitDrag`] with this id.
    pub fn new(id: impl Into<SharedString>, axis: Axis) -> Self {
        Self {
            id: id.into(),
            axis,
        }
    }
}

impl RenderOnce for SplitHandle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let hover = theme.colors.accent.opacity(0.5);
        let drag = SplitDrag {
            id: self.id.clone(),
        };
        let selector = self.id.to_string();
        let hit_area = div()
            .id(self.id)
            .debug_selector(move || selector)
            .absolute()
            .hover(|style| style.bg(hover))
            .on_drag(drag, |_, _, _, cx| cx.new(|_| Empty));
        let hit_area = match self.axis {
            Axis::Horizontal => hit_area
                .top_0()
                .bottom_0()
                .left(px(-2.))
                .w(px(5.))
                .cursor_col_resize(),
            Axis::Vertical => hit_area
                .left_0()
                .right_0()
                .top(px(-2.))
                .h(px(5.))
                .cursor_row_resize(),
        };
        let line = div().relative().flex_none().bg(theme.colors.line);
        match self.axis {
            Axis::Horizontal => line.w(px(1.)).h_full(),
            Axis::Vertical => line.h(px(1.)).w_full(),
        }
        .child(hit_area)
    }
}

/// Two panes and a handle. The first pane is `size` along `axis`; the second
/// takes the rest.
///
/// ```ignore
/// Split::new("elements-split", Axis::Vertical, self.tree_height)
///     .min_sizes(px(80.), px(120.))
///     .first(tree)
///     .second(detail)
///     .on_resize(cx.listener(|this, size, _, cx| { this.tree_height = *size; cx.notify() }))
/// ```
#[derive(IntoElement)]
pub struct Split {
    id: SharedString,
    axis: Axis,
    size: Pixels,
    min_first: Pixels,
    min_second: Pixels,
    first: Option<AnyElement>,
    second: Option<AnyElement>,
    on_resize: Option<Rc<dyn Fn(&Pixels, &mut Window, &mut App)>>,
}

impl Split {
    /// A split whose first pane is `size` along `axis`.
    pub fn new(id: impl Into<SharedString>, axis: Axis, size: Pixels) -> Self {
        Self {
            id: id.into(),
            axis,
            size,
            min_first: px(48.),
            min_second: px(48.),
            first: None,
            second: None,
            on_resize: None,
        }
    }

    /// Minimum sizes of the two panes (48 px each by default).
    pub fn min_sizes(mut self, min_first: Pixels, min_second: Pixels) -> Self {
        self.min_first = min_first;
        self.min_second = min_second;
        self
    }

    /// The first (leading or top) pane.
    pub fn first(mut self, pane: impl IntoElement) -> Self {
        self.first = Some(pane.into_any_element());
        self
    }

    /// The second (trailing or bottom) pane.
    pub fn second(mut self, pane: impl IntoElement) -> Self {
        self.second = Some(pane.into_any_element());
        self
    }

    /// Called with the new, clamped size of the first pane while dragging.
    pub fn on_resize(mut self, handler: impl Fn(&Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.on_resize = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Split {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let axis = self.axis;
        let (min_first, min_second) = (self.min_first, self.min_second);
        let handle_id = SharedString::from(format!("{}-handle", self.id));
        let container = div()
            .id(self.id)
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .overflow_hidden();
        let container = match axis {
            Axis::Horizontal => container.flex_row(),
            Axis::Vertical => container.flex_col(),
        };
        let first = div()
            .flex_shrink_1()
            .overflow_hidden()
            .flex()
            .flex_col()
            .children(self.first);
        let first = match axis {
            Axis::Horizontal => first.w(self.size).min_w(min_first).h_full(),
            Axis::Vertical => first.h(self.size).min_h(min_first).w_full(),
        };
        let second = div()
            .flex_1()
            .overflow_hidden()
            .flex()
            .flex_col()
            .children(self.second);
        let second = match axis {
            Axis::Horizontal => second.min_w(min_second).h_full(),
            Axis::Vertical => second.min_h(min_second).w_full(),
        };
        container
            .child(first)
            .child(SplitHandle::new(handle_id.clone(), axis))
            .child(second)
            .when_some(self.on_resize, |this, on_resize| {
                this.on_drag_move(move |event: &DragMoveEvent<SplitDrag>, window, cx| {
                    if event.drag(cx).id != handle_id {
                        return;
                    }
                    let origin = event.bounds.origin.along(axis);
                    let total = event.bounds.size.along(axis);
                    let desired = event.event.position.along(axis) - origin;
                    let size = clamp_split(desired, total, min_first, min_second);
                    on_resize(&size, window, cx);
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::clamp_split;
    use gpui::px;

    #[test]
    fn clamps_to_both_minimums() {
        let clamp = |desired| clamp_split(px(desired), px(400.), px(100.), px(150.));
        assert_eq!(clamp(200.), px(200.));
        assert_eq!(clamp(20.), px(100.));
        assert_eq!(clamp(390.), px(250.));
        assert_eq!(clamp(-50.), px(100.));
    }

    #[test]
    fn first_pane_wins_when_both_cannot_fit() {
        assert_eq!(clamp_split(px(60.), px(120.), px(100.), px(80.)), px(100.));
    }
}
