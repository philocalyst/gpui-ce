//! Buttons: icon, text or both; momentary or toggle; 20 or 24 px tall.

use crate::{
    theme::{Theme, UI_FONT},
    widgets::{Icon, IconName, Tooltip},
};
use gpui::{
    App, ClickEvent, FontWeight, Hsla, IntoElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::*, px,
};
use std::rc::Rc;

/// Control height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    /// 20 px: inline in rows, section headers and table headers.
    Small,
    /// 24 px: toolbars and forms.
    #[default]
    Regular,
}

/// Visual weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonStyle {
    /// No fill until hovered. The default for toolbars.
    #[default]
    Ghost,
    /// A raised fill with a hairline border, for primary actions in panes.
    Subtle,
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A pressable control.
///
/// ```ignore
/// Button::new("freeze")
///     .icon(IconName::Pause)
///     .label("Freeze")
///     .toggle_state(frozen)
///     .tooltip_keys("Freeze recording", "space")
///     .on_click(|_, window, cx| ...)
/// ```
///
/// Every button carries a debug selector equal to its id, so tests can find it.
#[derive(IntoElement)]
pub struct Button {
    id: SharedString,
    icon: Option<IconName>,
    label: Option<SharedString>,
    size: ButtonSize,
    style: ButtonStyle,
    toggled: bool,
    disabled: bool,
    color: Option<Hsla>,
    tooltip: Option<(SharedString, Option<SharedString>)>,
    on_click: Option<ClickHandler>,
}

impl Button {
    /// A button with a unique (per parent) id.
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon: None,
            label: None,
            size: ButtonSize::default(),
            style: ButtonStyle::default(),
            toggled: false,
            disabled: false,
            color: None,
            tooltip: None,
            on_click: None,
        }
    }

    /// Shows an icon before the label.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Shows a text label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Sets the height.
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Sets the visual weight.
    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    /// Makes this a toggle and sets whether it is on.
    pub fn toggle_state(mut self, toggled: bool) -> Self {
        self.toggled = toggled;
        self
    }

    /// Disables the button: dimmed and not clickable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Overrides the icon and label color (e.g. crit for destructive actions).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Shows a tooltip on hover.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some((text.into(), None));
        self
    }

    /// Shows a tooltip with a key hint on hover.
    pub fn tooltip_keys(
        mut self,
        text: impl Into<SharedString>,
        keys: impl Into<SharedString>,
    ) -> Self {
        self.tooltip = Some((text.into(), Some(keys.into())));
        self
    }

    /// Runs `handler` when clicked (not while disabled).
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let height = match self.size {
            ButtonSize::Small => theme.metrics.control_small,
            ButtonSize::Regular => theme.metrics.control,
        };
        let icon_size = match self.size {
            ButtonSize::Small => theme.metrics.icon_small,
            ButtonSize::Regular => theme.metrics.icon,
        };
        let foreground = if self.disabled {
            colors.text_faint
        } else if let Some(color) = self.color {
            color
        } else if self.toggled {
            colors.accent
        } else if self.label.is_some() {
            colors.text
        } else {
            colors.text_muted
        };
        let icon_only = self.label.is_none();
        let selector = self.id.to_string();

        div()
            .id(self.id.clone())
            .debug_selector(move || selector)
            .flex_none()
            .h(height)
            .when(icon_only, |this| this.w(height))
            .when(!icon_only, |this| {
                this.px(if self.icon.is_some() { px(6.) } else { px(8.) })
            })
            .flex()
            .items_center()
            .justify_center()
            .gap(px(5.))
            .rounded(theme.metrics.radius)
            .font_family(UI_FONT)
            .text_size(theme.metrics.text)
            .line_height(theme.metrics.line_height)
            .font_weight(FontWeight::NORMAL)
            .whitespace_nowrap()
            .text_color(foreground)
            .when(self.style == ButtonStyle::Subtle, |this| {
                this.bg(colors.surface_2)
                    .border_1()
                    .border_color(colors.line_strong)
            })
            .when(self.toggled, |this| this.bg(colors.selected))
            .when(!self.disabled, |this| {
                let hover = if self.toggled {
                    colors.selected
                } else {
                    colors.hover
                };
                this.cursor_pointer()
                    .hover(|style| style.bg(hover))
                    .active(|style| style.bg(colors.pressed))
            })
            .children(
                self.icon
                    .map(|icon| Icon::new(icon).size(icon_size).color(foreground)),
            )
            .children(self.label.clone())
            .when_some(self.tooltip, |this, (text, keys)| {
                this.tooltip(Tooltip::build(text, keys, None))
            })
            .when_some(self.on_click.filter(|_| !self.disabled), |this, handler| {
                this.on_click(move |event, window, cx| handler(event, window, cx))
            })
    }
}
