//! Key hints: `⌘K` on macOS, `Ctrl K` elsewhere.

use crate::theme::{Theme, UI_FONT};
use gpui::{App, IntoElement, RenderOnce, SharedString, Styled, Window, div, prelude::*, px};

/// A keystroke sequence rendered as a small key cap, e.g. `Kbd::new("cmd-k")`.
///
/// Accepts gpui keystroke syntax (`ctrl-shift-c`, `alt-1`, `cmd-k cmd-t`) and
/// formats it for the current platform.
#[derive(IntoElement)]
pub struct Kbd {
    keys: SharedString,
}

impl Kbd {
    /// A key hint for `keys`, in gpui keystroke syntax.
    pub fn new(keys: impl Into<SharedString>) -> Self {
        Self { keys: keys.into() }
    }
}

impl RenderOnce for Kbd {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        div()
            .flex_none()
            .h(px(16.))
            .px(px(4.))
            .flex()
            .items_center()
            .rounded(px(3.))
            .border_1()
            .border_color(theme.colors.line_strong)
            .font_family(UI_FONT)
            .text_size(theme.metrics.label)
            .line_height(px(14.))
            .text_color(theme.colors.text_muted)
            .whitespace_nowrap()
            .child(format_keys(&self.keys, cfg!(target_os = "macos")))
    }
}

/// Formats a keystroke sequence for display: `cmd-shift-c` becomes `⇧⌘C` on
/// macOS, and `ctrl-shift-c` becomes `Ctrl+Shift+C` elsewhere.
pub fn format_keys(keys: &str, mac: bool) -> String {
    keys.split_whitespace()
        .map(|stroke| format_keystroke(stroke, mac))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_keystroke(stroke: &str, mac: bool) -> String {
    let mut parts: Vec<&str> = stroke.split('-').collect();
    // A trailing empty part means the key itself is `-` (e.g. `cmd--`).
    let key = match parts.pop() {
        Some("") => "-",
        Some(key) => key,
        None => "",
    };
    // `secondary` is gpui's platform-neutral modifier: cmd on macOS, ctrl elsewhere.
    let secondary = if mac { "cmd" } else { "ctrl" };
    let has = |name: &str| {
        parts
            .iter()
            .any(|part| *part == name || (*part == "secondary" && name == secondary))
    };
    let key = format_key(key, mac);
    if mac {
        // Apple's canonical modifier order: ⌃ ⌥ ⇧ ⌘.
        let mut out = String::new();
        for (name, glyph) in [
            ("ctrl", "⌃"),
            ("alt", "⌥"),
            ("shift", "⇧"),
            ("cmd", "⌘"),
            ("fn", "fn "),
        ] {
            if has(name) {
                out.push_str(glyph);
            }
        }
        out + &key
    } else {
        let mut words: Vec<String> = Vec::new();
        for (name, word) in [
            ("ctrl", "Ctrl"),
            ("cmd", "Super"),
            ("alt", "Alt"),
            ("shift", "Shift"),
            ("fn", "Fn"),
        ] {
            if has(name) {
                words.push(word.to_string());
            }
        }
        words.push(key);
        words.join("+")
    }
}

fn format_key(key: &str, mac: bool) -> String {
    let named = match key {
        "enter" => Some(if mac { "↩" } else { "Enter" }),
        "escape" => Some("Esc"),
        "space" => Some("Space"),
        "tab" => Some(if mac { "⇥" } else { "Tab" }),
        "backspace" => Some(if mac { "⌫" } else { "Backspace" }),
        "up" => Some("↑"),
        "down" => Some("↓"),
        "left" => Some("←"),
        "right" => Some("→"),
        _ => None,
    };
    match named {
        Some(named) => named.to_string(),
        None => key.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::format_keys;

    #[test]
    fn formats_for_mac() {
        assert_eq!(format_keys("cmd-k", true), "⌘K");
        assert_eq!(format_keys("cmd-shift-c", true), "⇧⌘C");
        assert_eq!(format_keys("alt-1", true), "⌥1");
        assert_eq!(format_keys("cmd-k cmd-t", true), "⌘K ⌘T");
        assert_eq!(format_keys("escape", true), "Esc");
        assert_eq!(format_keys("secondary-k", true), "⌘K");
    }

    #[test]
    fn formats_for_other_platforms() {
        assert_eq!(format_keys("ctrl-k", false), "Ctrl+K");
        assert_eq!(format_keys("ctrl-shift-c", false), "Ctrl+Shift+C");
        assert_eq!(format_keys("space", false), "Space");
        assert_eq!(format_keys("secondary-shift-z", false), "Ctrl+Shift+Z");
        assert_eq!(format_keys("ctrl--", false), "Ctrl+-");
        assert_eq!(format_keys("[", false), "[");
    }
}
