//! Single-line text fields for filters and search, built on `gpui_elements`'
//! editable text (IME, selection, clipboard and undo included).

use crate::{
    theme::{MONO_FONT, Theme, UI_FONT},
    widgets::{Icon, IconName},
};
use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Focusable as _, FontFeatures,
    IntoElement, RenderOnce, SharedString, Styled, Window, div, prelude::*, px,
};
use gpui_elements::editable_text::{EditableTextState, StringStorage, text_input};

/// Creates the state behind a [`TextField`]; keep it in the owning view and
/// subscribe to `gpui_elements::editable_text::TextChanged` for edits.
pub fn text_field_state<T>(cx: &mut Context<T>) -> Entity<EditableTextState> {
    cx.new(|cx| EditableTextState::new(StringStorage::default(), cx))
}

/// A bordered single-line input with an optional leading icon and trailing
/// element (e.g. a key hint).
#[derive(IntoElement)]
pub struct TextField {
    id: ElementId,
    state: Entity<EditableTextState>,
    placeholder: Option<SharedString>,
    icon: Option<IconName>,
    trailing: Option<AnyElement>,
    borderless: bool,
    mono: bool,
}

impl TextField {
    /// A field editing `state`.
    pub fn new(id: impl Into<ElementId>, state: &Entity<EditableTextState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            placeholder: None,
            icon: None,
            trailing: None,
            borderless: false,
            mono: false,
        }
    }

    /// Text shown while empty.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// A leading icon, e.g. [`IconName::Search`] or [`IconName::Filter`].
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A trailing element inside the field.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    /// Drops the border and fill, for fields embedded in another surface
    /// (the palette's query line).
    pub fn borderless(mut self) -> Self {
        self.borderless = true;
        self
    }

    /// Sets the text in the mono font, for code such as URL templates.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let focus = self.state.read(cx).focus_handle(cx);
        let focused = focus.is_focused(window);
        div()
            .id(self.id.clone())
            .h(theme.metrics.control)
            .min_w_0()
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .map(|this| {
                if self.mono {
                    this.font_family(MONO_FONT)
                        .font_features(FontFeatures::disable_ligatures())
                        .text_size(theme.metrics.mono)
                } else {
                    this.font_family(UI_FONT).text_size(theme.metrics.text)
                }
            })
            .line_height(theme.metrics.line_height)
            .text_color(colors.text)
            .cursor_text()
            .when(!self.borderless, |this| {
                this.rounded(theme.metrics.radius)
                    .bg(colors.bg)
                    .border_1()
                    .border_color(if focused {
                        colors.accent
                    } else {
                        colors.line_strong
                    })
            })
            .children(self.icon.map(|icon| {
                Icon::new(icon)
                    .size(theme.metrics.icon_small)
                    .color(colors.text_faint)
            }))
            .child(
                text_input(self.id)
                    .state(self.state.downgrade())
                    .placeholder(self.placeholder.unwrap_or_default())
                    .placeholder_color(colors.text_faint)
                    .caret_color(colors.accent)
                    .selection_color(colors.selected)
                    .caret_w(px(1.5))
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_x_hidden(),
            )
            .children(self.trailing)
            .on_click(move |_, window, cx| window.focus(&focus, cx))
    }
}
