//! Segmented controls: one choice out of a few, shown side by side.

use crate::{
    theme::{Theme, UI_FONT},
    widgets::{Icon, IconName, Tooltip},
};
use gpui::{App, IntoElement, RenderOnce, SharedString, Styled, Window, div, prelude::*, px};
use std::rc::Rc;

/// One segment: an icon, a label, or both.
pub struct Segment {
    icon: Option<IconName>,
    label: Option<SharedString>,
    tooltip: Option<SharedString>,
}

impl Segment {
    /// A text segment.
    pub fn label(label: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            label: Some(label.into()),
            tooltip: None,
        }
    }

    /// An icon-only segment; give it a tooltip.
    pub fn icon(icon: IconName) -> Self {
        Self {
            icon: Some(icon),
            label: None,
            tooltip: None,
        }
    }

    /// Shows a tooltip on hover.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

/// A row of mutually exclusive segments inside a recessed track.
///
/// ```ignore
/// Segmented::new("dock")
///     .segment(Segment::icon(IconName::DockRight).tooltip("Dock right"))
///     .segment(Segment::icon(IconName::DockBottom).tooltip("Dock bottom"))
///     .selected(0)
///     .on_select(|ix, window, cx| ...)
/// ```
#[derive(IntoElement)]
pub struct Segmented {
    id: SharedString,
    segments: Vec<Segment>,
    selected: usize,
    on_select: Option<Rc<dyn Fn(&usize, &mut Window, &mut App)>>,
}

impl Segmented {
    /// An empty control with a unique id.
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            segments: Vec::new(),
            selected: 0,
            on_select: None,
        }
    }

    /// Appends a segment.
    pub fn segment(mut self, segment: Segment) -> Self {
        self.segments.push(segment);
        self
    }

    /// Index of the selected segment.
    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    /// Called with the index of a clicked, unselected segment.
    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let height = theme.metrics.control_small;
        div()
            .id(self.id.clone())
            .flex_none()
            .h(theme.metrics.control)
            .p(px(2.))
            .flex()
            .items_center()
            .gap(px(2.))
            .rounded(theme.metrics.radius)
            .bg(colors.surface_2)
            .font_family(UI_FONT)
            .text_size(theme.metrics.text_small)
            .children(self.segments.into_iter().enumerate().map(|(ix, segment)| {
                let selected = ix == self.selected;
                let foreground = if selected {
                    colors.text
                } else {
                    colors.text_muted
                };
                let selector = format!("{}-{ix}", self.id);
                div()
                    .id(ix)
                    .debug_selector(move || selector)
                    .h(height)
                    .when(segment.label.is_none(), |this| this.w(height))
                    .when(segment.label.is_some(), |this| this.px(px(8.)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded(px(3.))
                    .text_color(foreground)
                    .whitespace_nowrap()
                    .when(selected, |this| {
                        this.bg(colors.bg)
                            .border_1()
                            .border_color(colors.line_strong)
                    })
                    .when(!selected, |this| {
                        this.cursor_pointer().hover(|style| style.bg(colors.hover))
                    })
                    .children(segment.icon.map(|icon| {
                        Icon::new(icon)
                            .size(theme.metrics.icon_small)
                            .color(foreground)
                    }))
                    .children(segment.label)
                    .when_some(segment.tooltip, |this, tooltip| {
                        this.tooltip(Tooltip::text(tooltip))
                    })
                    .when_some(
                        self.on_select.clone().filter(|_| !selected),
                        |this, handler| {
                            this.on_click(move |_, window, cx| handler(&ix, window, cx))
                        },
                    )
            }))
    }
}
