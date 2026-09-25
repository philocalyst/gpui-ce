//! The key tester: explains how keystrokes resolve against the keymap and a
//! context stack, without dispatching anything.

use super::model::{BindingCandidate, BindingVerdict, KeyResolution};
use crate::{
    Action, BindingIndex, ExplainedBinding, InputOutcome, KeyBinding, KeyContext, Keymap,
    Keystroke, SharedString, Unbind,
};

/// Resolves `keystrokes` the way key dispatch would with `context_stack`
/// (outermost first) focused. `is_handled` reports whether a listener on the
/// focus path, or a global one, handles an action: dispatch tries matching
/// bindings in precedence order and stops at the first handled action.
pub(crate) fn resolve_keystrokes(
    keymap: &Keymap,
    keystrokes: &[Keystroke],
    context_stack: Vec<KeyContext>,
    is_handled: impl Fn(&dyn Action) -> bool,
) -> KeyResolution {
    let mut explained = keymap.explain_input(keystrokes, &context_stack);
    let winner = explained.iter().position(|explained| {
        explained.outcome == InputOutcome::Dispatched && is_handled(explained.binding.action())
    });
    if let Some(winner) = winner {
        let winner = explained.remove(winner);
        explained.insert(0, winner);
    }

    let candidates = explained
        .iter()
        .enumerate()
        .map(|(ix, binding)| BindingCandidate {
            keystrokes: binding_keystrokes(binding.binding),
            action: action_label(binding.binding.action()),
            predicate: binding_predicate(binding.binding),
            matched_depth: binding.depth.map(|depth| context_stack.len() - depth),
            verdict: verdict(&explained, ix, winner),
        })
        .collect();

    KeyResolution {
        keystrokes: keystrokes.iter().cloned().collect(),
        context_stack,
        candidates,
    }
}

/// The verdict for `explained[ix]`. When some binding wins, it has been moved
/// to the front from its precedence position `winner`, so the bindings ranked
/// above it now sit at `1..=winner`.
fn verdict(explained: &[ExplainedBinding], ix: usize, winner: Option<usize>) -> BindingVerdict {
    match (explained[ix].outcome, winner) {
        (InputOutcome::Dispatched, Some(_)) if ix == 0 => BindingVerdict::Wins,
        // Dispatch tries bindings in precedence order until an action is
        // handled: the ones ranked above the winner were tried and fell
        // through, the ones below it never run.
        (InputOutcome::Dispatched, Some(winner)) if ix > winner => {
            BindingVerdict::Shadowed { by: 0 }
        }
        (InputOutcome::Dispatched, _) => BindingVerdict::Unhandled,
        (InputOutcome::Disables, _) => BindingVerdict::Disabled,
        (InputOutcome::OutrankedBy(index), _) => BindingVerdict::Shadowed {
            by: candidate_index(explained, index),
        },
        (InputOutcome::Waits, _) => BindingVerdict::Pending,
        (InputOutcome::ContextMismatch, _) => BindingVerdict::ContextMismatch,
    }
}

/// Where the binding at keymap `index` sits among the candidates.
fn candidate_index(explained: &[ExplainedBinding], index: BindingIndex) -> usize {
    explained
        .iter()
        .position(|explained| explained.index == index)
        .expect("explain_input only outranks bindings by bindings it also returns")
}

/// A binding's keystrokes as written in a keymap, e.g. `ctrl-k ctrl-t`.
pub(crate) fn binding_keystrokes(binding: &KeyBinding) -> SharedString {
    let keystrokes = binding.keystrokes();
    match keystrokes {
        [keystroke] => keystroke.unparse().into(),
        _ => keystrokes
            .iter()
            .map(|keystroke| keystroke.unparse())
            .collect::<Vec<_>>()
            .join(" ")
            .into(),
    }
}

/// A binding's context predicate as written, if it has one.
pub(crate) fn binding_predicate(binding: &KeyBinding) -> Option<SharedString> {
    binding
        .predicate()
        .map(|predicate| predicate.to_string().into())
}

/// The action's name; `Unbind` bindings also name the action they unbind.
fn action_label(action: &dyn Action) -> SharedString {
    match action.as_any().downcast_ref::<Unbind>() {
        Some(unbind) => format!("{}({})", action.name(), unbind.0).into(),
        None => action.name().into(),
    }
}

#[cfg(test)]
mod tests;
