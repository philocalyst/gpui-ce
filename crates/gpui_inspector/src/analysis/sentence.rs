//! Sentences with inline code: the one-line explanations that come first in
//! Loupe's detail panes ("`cmd-k` → `workspace::OpenPalette` in `Workspace`").
//!
//! A [`Sentence`] is plain text interleaved with code fragments (keys, action
//! and type names, contexts, source locations), which the UI sets in the mono
//! font. Its `Display` wraps code in backticks, which is also how tests read it.

use gpui::SharedString;
use std::fmt;

/// One run of a [`Sentence`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fragment {
    /// The run's text.
    pub text: SharedString,
    /// Whether the run is code (set in the mono font).
    pub code: bool,
}

/// Plain text with inline code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sentence {
    fragments: Vec<Fragment>,
}

impl Sentence {
    /// An empty sentence.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends plain text.
    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.push(text.into(), false);
        self
    }

    /// Appends code.
    pub fn code(mut self, code: impl Into<SharedString>) -> Self {
        self.push(code.into(), true);
        self
    }

    /// Appends plain text in place.
    pub fn push_text(&mut self, text: impl Into<SharedString>) {
        self.push(text.into(), false);
    }

    /// Appends code in place.
    pub fn push_code(&mut self, code: impl Into<SharedString>) {
        self.push(code.into(), true);
    }

    /// Appends another sentence.
    pub fn append(mut self, other: Sentence) -> Self {
        for fragment in other.fragments {
            self.push(fragment.text, fragment.code);
        }
        self
    }

    /// The runs, in order. Adjacent runs of the same kind are merged.
    pub fn fragments(&self) -> &[Fragment] {
        &self.fragments
    }

    /// Whether there is no text at all.
    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    /// The text without code markers.
    pub fn plain(&self) -> String {
        self.fragments
            .iter()
            .map(|fragment| fragment.text.as_ref())
            .collect()
    }

    fn push(&mut self, text: SharedString, code: bool) {
        if text.is_empty() {
            return;
        }
        match self.fragments.last_mut() {
            Some(last) if last.code == code => {
                last.text = format!("{}{text}", last.text).into();
            }
            _ => self.fragments.push(Fragment { text, code }),
        }
    }
}

impl fmt::Display for Sentence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for fragment in &self.fragments {
            if fragment.code {
                write!(f, "`{}`", fragment.text)?;
            } else {
                f.write_str(&fragment.text)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_reads_in_backticks_and_plain_text_without() {
        let sentence = Sentence::new()
            .code("cmd-k")
            .text(" → ")
            .code("workspace::OpenPalette");
        assert_eq!(sentence.to_string(), "`cmd-k` → `workspace::OpenPalette`");
        assert_eq!(sentence.plain(), "cmd-k → workspace::OpenPalette");
    }

    #[test]
    fn adjacent_runs_merge_and_empty_runs_vanish() {
        let sentence = Sentence::new()
            .text("a")
            .text("")
            .text("b")
            .code("c")
            .append(Sentence::new().code("d").text("e"));
        assert_eq!(sentence.fragments().len(), 3);
        assert_eq!(sentence.to_string(), "ab`cd`e");
        assert!(Sentence::new().text("").is_empty());
    }
}
