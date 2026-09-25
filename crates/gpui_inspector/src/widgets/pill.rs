//! Pills: short status labels such as `FROZEN`, counts and severities.

use crate::theme::{Theme, UI_FONT};
use gpui::{
    App, ColorExt as _, FontWeight, Hsla, IntoElement, RenderOnce, SharedString, Styled, Window,
    div, prelude::*, px,
};

/// What a pill's color means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    /// No state; a plain count or tag.
    #[default]
    Neutral,
    /// Selection or an active mode.
    Accent,
    /// Healthy.
    Ok,
    /// Needs attention.
    Warn,
    /// Broken or well over budget.
    Crit,
}

impl Tone {
    /// The tone's foreground color in `theme`.
    pub fn color(self, theme: &Theme) -> Hsla {
        match self {
            Tone::Neutral => theme.colors.text_muted,
            Tone::Accent => theme.colors.accent,
            Tone::Ok => theme.colors.ok,
            Tone::Warn => theme.colors.warn,
            Tone::Crit => theme.colors.crit,
        }
    }
}

/// A small tinted label.
#[derive(IntoElement)]
pub struct Pill {
    label: SharedString,
    tone: Tone,
    strong: bool,
}

impl Pill {
    /// A neutral pill.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            tone: Tone::Neutral,
            strong: false,
        }
    }

    /// Sets the tone.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Uppercase, semibold and tracked: for modes like `FROZEN`.
    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }
}

impl RenderOnce for Pill {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let color = self.tone.color(theme);
        let fill = match self.tone {
            Tone::Neutral => theme.colors.surface_2,
            _ => color.opacity(if theme.is_dark { 0.16 } else { 0.12 }),
        };
        let label = if self.strong {
            self.label.to_uppercase().into()
        } else {
            self.label
        };
        div()
            .flex_none()
            .h(px(16.))
            .px(px(5.))
            .flex()
            .items_center()
            .rounded(px(3.))
            .bg(fill)
            .font_family(UI_FONT)
            .text_size(theme.metrics.label)
            .line_height(px(14.))
            .text_color(color)
            .whitespace_nowrap()
            .when(self.strong, |this| {
                this.font_weight(FontWeight::SEMIBOLD)
                    .letter_spacing(px(0.6))
            })
            .child(label)
    }
}
