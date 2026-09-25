//! Text painted straight into canvases (phase bar labels, flame bars, axis
//! ticks): shaped in Loupe's fonts and truncated with an ellipsis to fit.

use crate::theme::UI_FONT;
use gpui::{Hsla, Pixels, ShapedLine, SharedString, TextRun, Window, font};

/// Shapes `text` in the UI font.
pub(crate) fn shape(
    text: SharedString,
    font_size: Pixels,
    color: Hsla,
    window: &mut Window,
) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: font(UI_FONT),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
        letter_spacing: None,
    };
    window
        .text_system()
        .shape_line(text, font_size, &[run], None)
}

/// `text` shaped to fit `available` pixels: whole if it fits, else cut
/// before the glyph that would overflow and ended with `…`. `None` when not
/// even a character and the ellipsis fit.
pub(crate) fn fit(
    text: &SharedString,
    available: Pixels,
    font_size: Pixels,
    color: Hsla,
    window: &mut Window,
) -> Option<ShapedLine> {
    if available <= Pixels::ZERO {
        return None;
    }
    let line = shape(text.clone(), font_size, color, window);
    if line.width() <= available {
        return Some(line);
    }
    let ellipsis = shape(SharedString::new_static("…"), font_size, color, window).width();
    let cut = line.index_for_x(available - ellipsis)?;
    let kept = text.get(..cut)?.trim_end();
    if kept.is_empty() {
        return None;
    }
    Some(shape(format!("{kept}…").into(), font_size, color, window))
}
