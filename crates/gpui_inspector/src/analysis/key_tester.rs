//! The key tester's logic: collecting keystrokes the way GPUI's keymap
//! does, and explaining a [`KeyResolution`] in plain language.
//!
//! Keys pressed in quick succession build a sequence while some binding
//! waits for more of it (up to three strokes, one second apart, like
//! GPUI's own pending input); any other key starts a new sequence. Escape
//! twice clears, and a third escape leaves the tester.

use crate::analysis::sentence::Sentence;
use gpui::{
    Keystroke,
    inspector::{BindingCandidate, BindingVerdict, KeyResolution},
};
use smallvec::{SmallVec, smallvec};
use std::{
    fmt,
    time::{Duration, Instant},
};

/// How long GPUI waits for the next key of a longer binding.
pub const STROKE_TIMEOUT: Duration = Duration::from_secs(1);

/// The longest sequence the tester collects.
pub const MAX_STROKES: usize = 3;

/// What a key press did to the tester.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// The sequence changed and was resolved again.
    Resolved,
    /// Escape twice: the sequence and its resolution were cleared.
    Cleared,
    /// Escape three times: the tester should stop listening.
    Leave,
}

/// The keys typed into the tester and what they resolve to.
#[derive(Clone, Debug, Default)]
pub struct KeySequence {
    strokes: SmallVec<[Keystroke; MAX_STROKES]>,
    resolution: Option<KeyResolution>,
    last_press: Option<Instant>,
    cleared: bool,
}

impl KeySequence {
    /// The keys typed so far, oldest first.
    pub fn strokes(&self) -> &[Keystroke] {
        &self.strokes
    }

    /// What the keys resolve to, once any were typed.
    pub fn resolution(&self) -> Option<&KeyResolution> {
        self.resolution.as_ref()
    }

    /// Forgets the keys and their resolution.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Adds a key pressed at `now`. It extends the sequence while a binding
    /// waits for more keys, the sequence is short enough and the previous
    /// key was recent enough; otherwise, or when no binding starts with the
    /// extended sequence, it starts a new one (GPUI replays the old keys on
    /// their own then). `resolve` explains a sequence without dispatching.
    pub fn press(
        &mut self,
        keystroke: Keystroke,
        now: Instant,
        resolve: impl Fn(&[Keystroke]) -> KeyResolution,
    ) -> Press {
        if is_escape(&keystroke) {
            if self.strokes.is_empty() && self.cleared {
                self.reset();
                return Press::Leave;
            }
            if self.strokes.last().is_some_and(is_escape) {
                self.reset();
                self.cleared = true;
                return Press::Cleared;
            }
        }
        self.cleared = false;
        let continues = self
            .resolution
            .as_ref()
            .is_some_and(KeyResolution::is_pending)
            && self.strokes.len() < MAX_STROKES
            && self
                .last_press
                .is_some_and(|last| now.saturating_duration_since(last) <= STROKE_TIMEOUT);
        self.last_press = Some(now);
        if continues {
            let mut extended = self.strokes.clone();
            extended.push(keystroke.clone());
            let resolution = resolve(&extended);
            if !resolution.candidates.is_empty() {
                self.strokes = extended;
                self.resolution = Some(resolution);
                return Press::Resolved;
            }
        }
        self.strokes = smallvec![keystroke];
        self.resolution = Some(resolve(&self.strokes));
        Press::Resolved
    }
}

fn is_escape(keystroke: &Keystroke) -> bool {
    keystroke.key == "escape" && !keystroke.modifiers.modified()
}

/// How the tester labels a candidate binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Its action runs.
    Runs,
    /// Another binding takes precedence.
    Shadowed,
    /// A `NoAction` / `Unbind` binding turns it off.
    Disabled,
    /// It is a `NoAction` / `Unbind` binding, turning others off.
    Disables,
    /// Its context predicate is false here.
    Context,
    /// It needs more keys.
    Pending,
    /// Nothing handles its action here.
    Unhandled,
}

impl Verdict {
    /// The verdict of `resolution.candidates[ix]`.
    pub fn of(resolution: &KeyResolution, ix: usize) -> Self {
        match resolution
            .candidates
            .get(ix)
            .map(|candidate| &candidate.verdict)
        {
            Some(BindingVerdict::Wins) => Verdict::Runs,
            Some(BindingVerdict::Shadowed { by }) => {
                let by_disabler = resolution
                    .candidates
                    .get(*by)
                    .is_some_and(|other| other.verdict == BindingVerdict::Disabled);
                if by_disabler {
                    Verdict::Disabled
                } else {
                    Verdict::Shadowed
                }
            }
            Some(BindingVerdict::ContextMismatch) | None => Verdict::Context,
            Some(BindingVerdict::Disabled) => Verdict::Disables,
            Some(BindingVerdict::Pending) => Verdict::Pending,
            Some(BindingVerdict::Unhandled) => Verdict::Unhandled,
        }
    }

    /// The chip label.
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Runs => "Runs",
            Verdict::Shadowed => "Shadowed",
            Verdict::Disabled => "Disabled",
            Verdict::Disables => "Disables",
            Verdict::Context => "Context",
            Verdict::Pending => "Pending",
            Verdict::Unhandled => "Unhandled",
        }
    }
}

/// What happens overall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    /// A binding's action runs.
    Runs,
    /// GPUI waits for more keys before anything runs.
    Waiting,
    /// Nothing runs.
    Nothing,
}

/// The overall answer for a resolution: a short title ("Runs
/// `editor::Save`") and the detail that explains it ("bound in `Editor` ·
/// matched 2 levels up").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// What happens.
    pub kind: OutcomeKind,
    /// The answer in a few words.
    pub title: Sentence,
    /// Where the winner is bound, or why nothing runs.
    pub detail: Sentence,
}

impl Outcome {
    /// The answer for `resolution`.
    pub fn of(resolution: &KeyResolution) -> Self {
        if let Some(winner) = resolution.winner() {
            return Outcome {
                kind: OutcomeKind::Runs,
                title: Sentence::new().text("Runs ").code(winner.action.clone()),
                detail: binding_place(winner),
            };
        }
        let keys = typed_keys(resolution);
        let find = |verdict: BindingVerdict| {
            resolution
                .candidates
                .iter()
                .find(move |candidate| candidate.verdict == verdict)
        };
        if let Some(pending) = find(BindingVerdict::Pending) {
            return Outcome {
                kind: OutcomeKind::Waiting,
                title: Sentence::new().text("Waiting for more keys"),
                detail: Sentence::new()
                    .code(remaining_keys(resolution, pending))
                    .text(" runs ")
                    .code(pending.action.clone()),
            };
        }
        let detail = if let Some(disabler) = find(BindingVerdict::Disabled) {
            Sentence::new()
                .code(disabler.action.clone())
                .text(" turns ")
                .code(keys)
                .text(" off here")
        } else if let Some(unhandled) = find(BindingVerdict::Unhandled) {
            Sentence::new()
                .text("Nothing on the focus path handles ")
                .code(unhandled.action.clone())
        } else if !resolution.candidates.is_empty() {
            Sentence::new()
                .code(keys)
                .text(" is bound, but not in this context")
        } else {
            Sentence::new()
                .code(keys)
                .text(" is not bound; key listeners and text input get it")
        };
        Outcome {
            kind: OutcomeKind::Nothing,
            title: Sentence::new().text("Nothing runs"),
            detail,
        }
    }
}

impl fmt::Display for Outcome {
    /// `title: detail`, code in backticks.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.title, self.detail)
    }
}

/// When a binding runs but GPUI first waits for the keys of a longer one:
/// "GPUI first waits up to 1 s for `ctrl-t` (`editor::Trim`)".
pub fn waiting_note(resolution: &KeyResolution) -> Option<Sentence> {
    resolution.winner()?;
    let pending = resolution
        .candidates
        .iter()
        .find(|candidate| candidate.verdict == BindingVerdict::Pending)?;
    Some(
        Sentence::new()
            .text("GPUI first waits up to 1 s for ")
            .code(remaining_keys(resolution, pending))
            .text(" (")
            .code(pending.action.clone())
            .text(")"),
    )
}

/// Why `resolution.candidates[ix]` wins or loses, in plain language.
pub fn explain(resolution: &KeyResolution, ix: usize) -> Sentence {
    let Some(candidate) = resolution.candidates.get(ix) else {
        return Sentence::new();
    };
    match &candidate.verdict {
        BindingVerdict::Wins => binding_place(candidate),
        BindingVerdict::Shadowed { by } => {
            let Some(other) = resolution.candidates.get(*by) else {
                return Sentence::new().text("Outranked by another binding");
            };
            if other.verdict == BindingVerdict::Disabled {
                let sentence = Sentence::new()
                    .text("Turned off by ")
                    .code(other.action.clone());
                return match &other.predicate {
                    Some(predicate) => sentence.text(" in ").code(predicate.clone()),
                    None => sentence,
                };
            }
            if stroke_count(&candidate.keystrokes) > resolution.keystrokes.len() {
                return Sentence::new()
                    .code(typed_keys(resolution))
                    .text(" alone runs ")
                    .code(other.action.clone())
                    .text(", so GPUI doesn't wait for more keys");
            }
            let why = match (candidate.matched_depth, other.matched_depth) {
                (Some(own), Some(theirs)) if own > theirs => ": its context is deeper",
                (Some(own), Some(theirs)) if own == theirs => ": bound later in the keymap",
                _ => "",
            };
            Sentence::new()
                .text("Outranked by ")
                .code(other.action.clone())
                .text(why)
        }
        BindingVerdict::ContextMismatch => match &candidate.predicate {
            Some(predicate) => Sentence::new()
                .text("Context ")
                .code(predicate.clone())
                .text(" is false here"),
            None => Sentence::new().text("Its context doesn't match here"),
        },
        BindingVerdict::Disabled => {
            Sentence::new().text("Turns off the bindings it outranks for these keys")
        }
        BindingVerdict::Pending => Sentence::new()
            .text("Needs more keys: type ")
            .code(remaining_keys(resolution, candidate))
            .text(" next"),
        BindingVerdict::Unhandled => Sentence::new()
            .text("Nothing on the focus path handles ")
            .code(candidate.action.clone())
            .text(", so dispatch falls through"),
    }
}

/// Where a binding applies: "bound in `Editor` · matched 2 levels up", or
/// "bound globally (no context)".
pub fn binding_place(candidate: &BindingCandidate) -> Sentence {
    let Some(predicate) = &candidate.predicate else {
        return Sentence::new().text("bound globally (no context)");
    };
    let sentence = Sentence::new().text("bound in ").code(predicate.clone());
    match candidate.matched_depth {
        Some(0) => sentence.text(" · matched the innermost context"),
        Some(1) => sentence.text(" · matched 1 level up"),
        Some(depth) => sentence.text(format!(" · matched {depth} levels up")),
        None => sentence,
    }
}

/// The keys of `candidate` still to type after the resolved ones.
pub fn remaining_keys(resolution: &KeyResolution, candidate: &BindingCandidate) -> String {
    candidate
        .keystrokes
        .split_whitespace()
        .skip(resolution.keystrokes.len())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The resolved keys, as written in keymaps: `ctrl-k ctrl-t`.
pub fn typed_keys(resolution: &KeyResolution) -> String {
    resolution
        .keystrokes
        .iter()
        .map(Keystroke::unparse)
        .collect::<Vec<_>>()
        .join(" ")
}

fn stroke_count(keystrokes: &str) -> usize {
    keystrokes.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::SharedString;

    fn keys(source: &str) -> Vec<Keystroke> {
        source
            .split_whitespace()
            .map(|key| Keystroke::parse(key).unwrap())
            .collect()
    }

    fn candidate(
        keystrokes: &str,
        action: &str,
        predicate: Option<&str>,
        matched_depth: Option<usize>,
        verdict: BindingVerdict,
    ) -> BindingCandidate {
        BindingCandidate {
            keystrokes: SharedString::from(keystrokes.to_string()),
            action: SharedString::from(action.to_string()),
            predicate: predicate.map(|predicate| SharedString::from(predicate.to_string())),
            matched_depth,
            verdict,
        }
    }

    fn resolution(typed: &str, candidates: Vec<BindingCandidate>) -> KeyResolution {
        KeyResolution {
            keystrokes: keys(typed).into_iter().collect(),
            context_stack: Vec::new(),
            candidates,
        }
    }

    /// A keymap with one binding, `ctrl-k ctrl-t`, and nothing else.
    fn trim_keymap(typed: &[Keystroke]) -> KeyResolution {
        let written: Vec<String> = typed.iter().map(Keystroke::unparse).collect();
        let verdict = match written.join(" ").as_str() {
            "ctrl-k" => BindingVerdict::Pending,
            "ctrl-k ctrl-t" => BindingVerdict::Wins,
            _ => return resolution(&written.join(" "), Vec::new()),
        };
        resolution(
            &written.join(" "),
            vec![candidate(
                "ctrl-k ctrl-t",
                "e::Trim",
                None,
                Some(0),
                verdict,
            )],
        )
    }

    fn press(sequence: &mut KeySequence, key: &str, at: Instant) -> Press {
        sequence.press(Keystroke::parse(key).unwrap(), at, trim_keymap)
    }

    fn typed(sequence: &KeySequence) -> String {
        sequence
            .strokes()
            .iter()
            .map(Keystroke::unparse)
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn keys_accumulate_while_a_binding_waits_for_more() {
        let start = Instant::now();
        let mut sequence = KeySequence::default();
        assert_eq!(press(&mut sequence, "ctrl-k", start), Press::Resolved);
        assert!(sequence.resolution().unwrap().is_pending());
        press(&mut sequence, "ctrl-t", start + Duration::from_millis(300));
        assert_eq!(typed(&sequence), "ctrl-k ctrl-t");
        assert_eq!(
            sequence
                .resolution()
                .unwrap()
                .winner()
                .unwrap()
                .action
                .as_ref(),
            "e::Trim"
        );

        // Nothing waits after a complete match: the next key starts over.
        press(&mut sequence, "x", start + Duration::from_millis(400));
        assert_eq!(typed(&sequence), "x");
    }

    #[test]
    fn a_key_that_continues_no_binding_starts_over() {
        let start = Instant::now();
        let mut sequence = KeySequence::default();
        press(&mut sequence, "ctrl-k", start);
        press(&mut sequence, "q", start);
        assert_eq!(typed(&sequence), "q");
    }

    #[test]
    fn a_slow_key_starts_over_like_gpuis_timeout() {
        let start = Instant::now();
        let mut sequence = KeySequence::default();
        press(&mut sequence, "ctrl-k", start);
        press(&mut sequence, "ctrl-t", start + STROKE_TIMEOUT * 2);
        assert_eq!(typed(&sequence), "ctrl-t");
    }

    #[test]
    fn sequences_stop_at_three_keys() {
        let start = Instant::now();
        let mut sequence = KeySequence::default();
        let always_pending = |typed: &[Keystroke]| {
            let written: Vec<String> = typed.iter().map(Keystroke::unparse).collect();
            resolution(
                &written.join(" "),
                vec![candidate(
                    "a a a a",
                    "e::Four",
                    None,
                    Some(0),
                    BindingVerdict::Pending,
                )],
            )
        };
        for _ in 0..4 {
            sequence.press(Keystroke::parse("a").unwrap(), start, always_pending);
        }
        assert_eq!(typed(&sequence), "a");
        for _ in 0..2 {
            sequence.press(Keystroke::parse("a").unwrap(), start, always_pending);
        }
        assert_eq!(typed(&sequence), "a a a");
    }

    #[test]
    fn escape_is_testable_twice_clears_and_thrice_leaves() {
        let start = Instant::now();
        let mut sequence = KeySequence::default();
        assert_eq!(press(&mut sequence, "escape", start), Press::Resolved);
        assert_eq!(typed(&sequence), "escape");
        assert_eq!(press(&mut sequence, "escape", start), Press::Cleared);
        assert!(sequence.strokes().is_empty());
        assert!(sequence.resolution().is_none());
        assert_eq!(press(&mut sequence, "escape", start), Press::Leave);

        // Leaving forgets the clear, so escape is testable again next time.
        assert_eq!(press(&mut sequence, "escape", start), Press::Resolved);
        // Any other key between escapes cancels the pending clear.
        let mut sequence = KeySequence::default();
        press(&mut sequence, "escape", start);
        press(&mut sequence, "escape", start);
        press(&mut sequence, "x", start);
        assert_eq!(press(&mut sequence, "escape", start), Press::Resolved);
        assert_eq!(typed(&sequence), "escape");
        // Modified escapes are ordinary keys.
        assert_eq!(press(&mut sequence, "shift-escape", start), Press::Resolved);
    }

    /// The resolution of `ctrl-s` in `Workspace > Pane > Editor`.
    fn save() -> KeyResolution {
        resolution(
            "ctrl-s",
            vec![
                candidate(
                    "ctrl-s",
                    "mail::Save",
                    Some("Editor"),
                    Some(0),
                    BindingVerdict::Wins,
                ),
                candidate(
                    "ctrl-s",
                    "mail::SaveAll",
                    Some("Workspace"),
                    Some(2),
                    BindingVerdict::Shadowed { by: 0 },
                ),
                candidate(
                    "ctrl-s",
                    "mail::Later",
                    Some("Editor"),
                    Some(0),
                    BindingVerdict::Shadowed { by: 0 },
                ),
                candidate(
                    "ctrl-s",
                    "mail::Vim",
                    Some("Editor && mode == vim"),
                    None,
                    BindingVerdict::ContextMismatch,
                ),
                candidate(
                    "ctrl-s ctrl-s",
                    "mail::Twice",
                    None,
                    Some(0),
                    BindingVerdict::Shadowed { by: 0 },
                ),
            ],
        )
    }

    #[test]
    fn the_winner_says_what_runs_and_where_it_is_bound() {
        let save = save();
        let outcome = Outcome::of(&save);
        assert_eq!(outcome.kind, OutcomeKind::Runs);
        assert_eq!(
            outcome.to_string(),
            "Runs `mail::Save`: bound in `Editor` · matched the innermost context"
        );
        let explained: Vec<String> = (0..save.candidates.len())
            .map(|ix| explain(&save, ix).to_string())
            .collect();
        assert_eq!(
            explained,
            [
                "bound in `Editor` · matched the innermost context",
                "Outranked by `mail::Save`: its context is deeper",
                "Outranked by `mail::Save`: bound later in the keymap",
                "Context `Editor && mode == vim` is false here",
                "`ctrl-s` alone runs `mail::Save`, so GPUI doesn't wait for more keys",
            ]
        );
        let verdicts: Vec<Verdict> = (0..save.candidates.len())
            .map(|ix| Verdict::of(&save, ix))
            .collect();
        assert_eq!(
            verdicts,
            [
                Verdict::Runs,
                Verdict::Shadowed,
                Verdict::Shadowed,
                Verdict::Context,
                Verdict::Shadowed,
            ]
        );
        assert_eq!(waiting_note(&save), None);
    }

    #[test]
    fn depth_and_global_bindings_read_naturally() {
        let at = |depth| {
            binding_place(&candidate(
                "a",
                "a::A",
                Some("Workspace"),
                depth,
                BindingVerdict::Wins,
            ))
            .to_string()
        };
        assert_eq!(at(Some(1)), "bound in `Workspace` · matched 1 level up");
        assert_eq!(at(Some(3)), "bound in `Workspace` · matched 3 levels up");
        assert_eq!(
            binding_place(&candidate("a", "a::A", None, Some(0), BindingVerdict::Wins)).to_string(),
            "bound globally (no context)"
        );
    }

    #[test]
    fn no_action_explains_both_sides() {
        let delete = resolution(
            "ctrl-d",
            vec![
                candidate(
                    "ctrl-d",
                    "zed::NoAction",
                    Some("Editor"),
                    Some(0),
                    BindingVerdict::Disabled,
                ),
                candidate(
                    "ctrl-d",
                    "mail::Delete",
                    Some("Workspace"),
                    Some(2),
                    BindingVerdict::Shadowed { by: 0 },
                ),
            ],
        );
        assert_eq!(Verdict::of(&delete, 0), Verdict::Disables);
        assert_eq!(Verdict::of(&delete, 1), Verdict::Disabled);
        assert_eq!(
            explain(&delete, 0).to_string(),
            "Turns off the bindings it outranks for these keys"
        );
        assert_eq!(
            explain(&delete, 1).to_string(),
            "Turned off by `zed::NoAction` in `Editor`"
        );
        let outcome = Outcome::of(&delete);
        assert_eq!(outcome.kind, OutcomeKind::Nothing);
        assert_eq!(
            outcome.to_string(),
            "Nothing runs: `zed::NoAction` turns `ctrl-d` off here"
        );
    }

    #[test]
    fn pending_unhandled_and_unbound_keys_say_what_happens_instead() {
        let waiting = trim_keymap(&keys("ctrl-k"));
        let outcome = Outcome::of(&waiting);
        assert_eq!(outcome.kind, OutcomeKind::Waiting);
        assert_eq!(
            outcome.to_string(),
            "Waiting for more keys: `ctrl-t` runs `e::Trim`"
        );
        assert_eq!(
            explain(&waiting, 0).to_string(),
            "Needs more keys: type `ctrl-t` next"
        );

        let unhandled = resolution(
            "ctrl-u",
            vec![candidate(
                "ctrl-u",
                "mail::Orphan",
                Some("Editor"),
                Some(0),
                BindingVerdict::Unhandled,
            )],
        );
        assert_eq!(Verdict::of(&unhandled, 0), Verdict::Unhandled);
        assert_eq!(
            Outcome::of(&unhandled).to_string(),
            "Nothing runs: Nothing on the focus path handles `mail::Orphan`"
        );
        assert_eq!(
            explain(&unhandled, 0).to_string(),
            "Nothing on the focus path handles `mail::Orphan`, so dispatch falls through"
        );

        let mismatch = resolution(
            "ctrl-w",
            vec![candidate(
                "ctrl-w",
                "mail::Close",
                Some("Editor && mode == vim"),
                None,
                BindingVerdict::ContextMismatch,
            )],
        );
        assert_eq!(
            Outcome::of(&mismatch).to_string(),
            "Nothing runs: `ctrl-w` is bound, but not in this context"
        );
        assert_eq!(
            Outcome::of(&resolution("ctrl-q", Vec::new())).to_string(),
            "Nothing runs: `ctrl-q` is not bound; key listeners and text input get it"
        );
    }

    #[test]
    fn a_winner_that_waits_first_says_so() {
        let resolution = resolution(
            "ctrl-k",
            vec![
                candidate("ctrl-k", "e::Kill", None, Some(0), BindingVerdict::Wins),
                candidate(
                    "ctrl-k ctrl-t",
                    "e::Trim",
                    None,
                    Some(0),
                    BindingVerdict::Pending,
                ),
            ],
        );
        assert_eq!(
            waiting_note(&resolution).unwrap().to_string(),
            "GPUI first waits up to 1 s for `ctrl-t` (`e::Trim`)"
        );
    }
}
