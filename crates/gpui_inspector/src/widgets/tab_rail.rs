//! Tab rails: a row of tabs that carry live counts, so the rail doubles as a dashboard.

use crate::{
    theme::{Theme, UI_FONT},
    widgets::{Icon, IconName, Tone, Tooltip},
};
use gpui::{App, IntoElement, RenderOnce, SharedString, Styled, Window, div, prelude::*, px};
use std::rc::Rc;

/// One tab: a label and an optional count, tinted when it signals a problem.
pub struct RailTab {
    label: SharedString,
    count: Option<SharedString>,
    tone: Tone,
    marker: Option<IconName>,
    tooltip: Option<(SharedString, Option<SharedString>)>,
}

impl RailTab {
    /// A tab labelled `label`.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            count: None,
            tone: Tone::Neutral,
            marker: None,
            tooltip: None,
        }
    }

    /// Shows a count after the label, e.g. `1,284`.
    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    /// Tints the count (neutral counts are faint).
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// A small glyph after the count, in its tint (e.g. ▲ for "over budget").
    pub fn marker(mut self, marker: IconName) -> Self {
        self.marker = Some(marker);
        self
    }

    /// Shows a tooltip, with an optional key hint.
    pub fn tooltip(mut self, text: impl Into<SharedString>, keys: Option<SharedString>) -> Self {
        self.tooltip = Some((text.into(), keys));
        self
    }
}

/// A horizontal tab rail with a 2 px accent underline under the selected tab.
#[derive(IntoElement)]
pub struct TabRail {
    id: SharedString,
    tabs: Vec<RailTab>,
    selected: usize,
    on_select: Option<Rc<dyn Fn(&usize, &mut Window, &mut App)>>,
}

impl TabRail {
    /// An empty rail with a unique id.
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            tabs: Vec::new(),
            selected: 0,
            on_select: None,
        }
    }

    /// Appends a tab.
    pub fn tab(mut self, tab: RailTab) -> Self {
        self.tabs.push(tab);
        self
    }

    /// Index of the selected tab.
    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    /// Called with the index of a clicked tab.
    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for TabRail {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        div()
            .id(self.id.clone())
            .flex_none()
            .h(theme.metrics.rail)
            .w_full()
            .min_w_0()
            .px(px(4.))
            .flex()
            .items_stretch()
            .overflow_hidden()
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.line)
            .font_family(UI_FONT)
            .text_size(theme.metrics.text)
            .line_height(theme.metrics.line_height)
            .children(self.tabs.into_iter().enumerate().map(|(ix, tab)| {
                let selected = ix == self.selected;
                let count_color = match tab.tone {
                    Tone::Neutral if selected => colors.text_muted,
                    Tone::Neutral => colors.text_faint,
                    tone => tone.color(theme),
                };
                let selector = format!("{}-{ix}", self.id);
                div()
                    .id(ix)
                    .debug_selector(move || selector)
                    .relative()
                    .flex_shrink_1()
                    .min_w_0()
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .text_color(if selected {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .when(!selected, |this| {
                        this.hover(|style| style.text_color(colors.text))
                    })
                    .child(div().min_w_0().truncate().child(tab.label))
                    .children(tab.count.map(|count| {
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .text_size(theme.metrics.text_small)
                            .text_color(count_color)
                            .child(count)
                            .children(
                                tab.marker.map(|marker| {
                                    Icon::new(marker).size(px(8.)).color(count_color)
                                }),
                            )
                    }))
                    .when(selected, |this| {
                        this.child(
                            div()
                                .absolute()
                                .left(px(6.))
                                .right(px(6.))
                                .bottom_0()
                                .h(px(2.))
                                .rounded_t(px(1.))
                                .bg(colors.accent),
                        )
                    })
                    .when_some(tab.tooltip, |this, (text, keys)| {
                        this.tooltip(Tooltip::build(text, keys, None))
                    })
                    .when_some(self.on_select.clone(), |this, handler| {
                        this.on_click(move |_, window, cx| handler(&ix, window, cx))
                    })
            }))
    }
}
