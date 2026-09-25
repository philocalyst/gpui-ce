//! Prose: a [`Sentence`] with inline code, wrapped, with code set in the
//! mono font and the primary text color.

use crate::{
    analysis::sentence::Sentence,
    theme::{MONO_FONT, Theme, UI_FONT},
};
use gpui::{
    App, HighlightStyle, Hsla, IntoElement, Pixels, RenderOnce, SharedString, Styled, StyledText,
    Window, div, prelude::*,
};
use std::ops::Range;

/// A wrapped sentence whose code fragments use the mono font.
///
/// ```ignore
/// Prose::new(record_sentence(record, target)).color(colors.text_muted)
/// ```
#[derive(IntoElement)]
pub struct Prose {
    sentence: Sentence,
    color: Option<Hsla>,
    code_color: Option<Hsla>,
    size: Option<Pixels>,
}

impl Prose {
    /// Prose for `sentence`, in the body text color and size.
    pub fn new(sentence: Sentence) -> Self {
        Self {
            sentence,
            color: None,
            code_color: None,
            size: None,
        }
    }

    /// The color of plain text (code keeps the primary text color).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// The color of code fragments.
    pub fn code_color(mut self, color: Hsla) -> Self {
        self.code_color = Some(color);
        self
    }

    /// The text size.
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = Some(size);
        self
    }
}

/// The sentence's text and the byte ranges of its code fragments.
fn code_ranges(sentence: &Sentence) -> (String, Vec<Range<usize>>) {
    let mut text = String::new();
    let mut ranges = Vec::new();
    for fragment in sentence.fragments() {
        let start = text.len();
        text.push_str(&fragment.text);
        if fragment.code {
            ranges.push(start..text.len());
        }
    }
    (text, ranges)
}

impl RenderOnce for Prose {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let (text, ranges) = code_ranges(&self.sentence);
        let code_color = self.code_color.unwrap_or(theme.colors.text);
        let mono = SharedString::new_static(MONO_FONT);
        let styled = StyledText::new(text)
            .with_font_family_overrides(ranges.iter().map(|range| (range.clone(), mono.clone())))
            .with_highlights(ranges.into_iter().map(|range| {
                (
                    range,
                    HighlightStyle {
                        color: Some(code_color),
                        ..HighlightStyle::default()
                    },
                )
            }));
        div()
            .min_w_0()
            .font_family(UI_FONT)
            .text_size(self.size.unwrap_or(theme.metrics.text))
            .line_height(theme.metrics.line_height + gpui::px(2.))
            .text_color(self.color.unwrap_or(theme.colors.text))
            .child(styled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_ranges_cover_exactly_the_code() {
        let sentence = Sentence::new().code("ctrl-s").text(" → ").code("a::Save");
        let (text, ranges) = code_ranges(&sentence);
        assert_eq!(text, "ctrl-s → a::Save");
        let code: Vec<&str> = ranges.iter().map(|range| &text[range.clone()]).collect();
        assert_eq!(code, ["ctrl-s", "a::Save"]);
    }
}
