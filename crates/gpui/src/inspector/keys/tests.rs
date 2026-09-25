use super::resolve_keystrokes;
use crate::{
    self as gpui, Action, App, Context, Div, DummyKeyboardMapper, FocusHandle, InputOutcome,
    InteractiveElement as _, IntoElement, KeyBinding, KeyBindingContextPredicate, KeyContext,
    Keymap, Keystroke, NoAction, ParentElement as _, Render, Styled as _, TestAppContext, Unbind,
    VisualTestContext, Window, div,
    inspector::{BindingVerdict, KeyResolution},
    proptest::{collection::vec, prelude::*, sample::select},
};
use std::{cell::RefCell, mem, rc::Rc, time::Duration};

actions!(inspector_keys_test, [Alpha, Beta, Gamma, Delta]);

fn keystrokes(source: &str) -> Vec<Keystroke> {
    source
        .split(' ')
        .map(|keystroke| Keystroke::parse(keystroke).unwrap())
        .collect()
}

fn contexts(sources: &[&str]) -> Vec<KeyContext> {
    sources
        .iter()
        .map(|source| KeyContext::parse(source).unwrap())
        .collect()
}

/// Resolves `input` with every action handled except those named in `unhandled`.
fn resolve(
    bindings: Vec<KeyBinding>,
    input: &str,
    stack: &[&str],
    unhandled: &[&dyn Action],
) -> KeyResolution {
    let keymap = Keymap::new(bindings);
    resolve_keystrokes(&keymap, &keystrokes(input), contexts(stack), |action| {
        !unhandled
            .iter()
            .any(|unhandled| unhandled.partial_eq(action))
    })
}

/// `(action, verdict)` for every candidate, in order.
fn verdicts(resolution: &KeyResolution) -> Vec<(&str, BindingVerdict)> {
    resolution
        .candidates
        .iter()
        .map(|candidate| (candidate.action.as_ref(), candidate.verdict.clone()))
        .collect()
}

#[test]
fn deepest_context_wins_and_the_others_are_shadowed() {
    let resolution = resolve(
        vec![
            KeyBinding::new("ctrl-a", Alpha, None),
            KeyBinding::new("ctrl-a", Beta, Some("pane")),
            KeyBinding::new("ctrl-a", Gamma, Some("editor")),
        ],
        "ctrl-a",
        &["pane", "editor"],
        &[],
    );

    assert_eq!(
        verdicts(&resolution),
        [
            ("inspector_keys_test::Gamma", BindingVerdict::Wins),
            // Bindings without a context rank at the innermost context, below
            // later bindings there.
            (
                "inspector_keys_test::Alpha",
                BindingVerdict::Shadowed { by: 0 }
            ),
            (
                "inspector_keys_test::Beta",
                BindingVerdict::Shadowed { by: 0 }
            ),
        ]
    );
    let depths: Vec<_> = resolution
        .candidates
        .iter()
        .map(|candidate| candidate.matched_depth)
        .collect();
    assert_eq!(depths, [Some(0), Some(0), Some(1)]);
    let predicates: Vec<_> = resolution
        .candidates
        .iter()
        .map(|candidate| candidate.predicate.as_deref())
        .collect();
    assert_eq!(predicates, [Some("editor"), None, Some("pane")]);
    assert_eq!(resolution.winner().unwrap().keystrokes.as_ref(), "ctrl-a");
    assert_eq!(resolution.context_stack, contexts(&["pane", "editor"]));
    assert!(!resolution.is_pending());
}

#[test]
fn context_mismatch_names_the_failing_predicate() {
    let resolution = resolve(
        vec![
            KeyBinding::new("ctrl-a", Alpha, Some("terminal")),
            KeyBinding::new("ctrl-a", Beta, Some("editor && mode == full")),
        ],
        "ctrl-a",
        &["editor mode=vim"],
        &[],
    );

    assert_eq!(
        verdicts(&resolution),
        [
            ("inspector_keys_test::Beta", BindingVerdict::ContextMismatch),
            (
                "inspector_keys_test::Alpha",
                BindingVerdict::ContextMismatch
            ),
        ]
    );
    let candidate = &resolution.candidates[0];
    assert_eq!(
        candidate.predicate.as_deref(),
        Some("editor && mode == full")
    );
    assert_eq!(candidate.matched_depth, None);
    assert!(resolution.winner().is_none());
}

#[test]
fn no_action_disables_the_bindings_it_outranks() {
    let resolution = resolve(
        vec![
            KeyBinding::new("ctrl-a", Alpha, Some("pane")),
            KeyBinding::new("ctrl-a", NoAction, Some("editor")),
            KeyBinding::new("ctrl-a", Beta, Some("editor")),
        ],
        "ctrl-a",
        &["pane", "editor"],
        &[],
    );

    assert_eq!(
        verdicts(&resolution),
        [
            ("inspector_keys_test::Beta", BindingVerdict::Wins),
            ("zed::NoAction", BindingVerdict::Disabled),
            (
                "inspector_keys_test::Alpha",
                BindingVerdict::Shadowed { by: 1 }
            ),
        ]
    );
}

#[test]
fn unbind_disables_only_the_action_it_names() {
    let resolution = resolve(
        vec![
            KeyBinding::new("ctrl-a", Alpha, None),
            KeyBinding::new("ctrl-a", Beta, None),
            KeyBinding::new("ctrl-a", Unbind(Beta.name().into()), None),
        ],
        "ctrl-a",
        &[],
        &[],
    );

    assert_eq!(
        verdicts(&resolution),
        [
            ("inspector_keys_test::Alpha", BindingVerdict::Wins),
            (
                "zed::Unbind(inspector_keys_test::Beta)",
                BindingVerdict::Disabled
            ),
            (
                "inspector_keys_test::Beta",
                BindingVerdict::Shadowed { by: 1 }
            ),
        ]
    );
    assert_eq!(resolution.candidates[0].matched_depth, Some(0));
}

#[test]
fn prefix_of_a_longer_binding_is_pending() {
    let resolution = resolve(
        vec![KeyBinding::new("ctrl-k ctrl-t", Alpha, None)],
        "ctrl-k",
        &["editor"],
        &[],
    );

    assert_eq!(
        verdicts(&resolution),
        [("inspector_keys_test::Alpha", BindingVerdict::Pending)]
    );
    assert_eq!(
        resolution.candidates[0].keystrokes.as_ref(),
        "ctrl-k ctrl-t"
    );
    assert!(resolution.is_pending());
    assert!(resolution.winner().is_none());
}

#[test]
fn complete_match_added_later_stops_the_wait_for_a_longer_binding() {
    let later = resolve(
        vec![
            KeyBinding::new("ctrl-k ctrl-t", Alpha, None),
            KeyBinding::new("ctrl-k", Beta, None),
        ],
        "ctrl-k",
        &[],
        &[],
    );
    assert_eq!(
        verdicts(&later),
        [
            ("inspector_keys_test::Beta", BindingVerdict::Wins),
            (
                "inspector_keys_test::Alpha",
                BindingVerdict::Shadowed { by: 0 }
            ),
        ]
    );
    assert!(!later.is_pending());

    let earlier = resolve(
        vec![
            KeyBinding::new("ctrl-k", Beta, None),
            KeyBinding::new("ctrl-k ctrl-t", Alpha, None),
        ],
        "ctrl-k",
        &[],
        &[],
    );
    assert_eq!(
        verdicts(&earlier),
        [
            ("inspector_keys_test::Beta", BindingVerdict::Wins),
            ("inspector_keys_test::Alpha", BindingVerdict::Pending),
        ]
    );
    assert!(earlier.is_pending(), "GPUI waits, then runs the winner");
}

#[test]
fn disabling_a_longer_binding_cancels_the_wait() {
    let resolution = resolve(
        vec![
            KeyBinding::new("ctrl-k ctrl-t", Alpha, None),
            KeyBinding::new("ctrl-k ctrl-t", NoAction, Some("editor")),
        ],
        "ctrl-k",
        &["editor"],
        &[],
    );

    assert_eq!(
        verdicts(&resolution),
        [
            ("zed::NoAction", BindingVerdict::Disabled),
            (
                "inspector_keys_test::Alpha",
                BindingVerdict::Shadowed { by: 0 }
            ),
        ]
    );
    assert!(!resolution.is_pending());
}

#[test]
fn multi_stroke_sequence_resolves_to_its_binding() {
    let resolution = resolve(
        vec![
            KeyBinding::new("ctrl-k", Beta, None),
            KeyBinding::new("ctrl-k ctrl-t", Alpha, Some("editor")),
            KeyBinding::new("ctrl-k ctrl-t ctrl-t", Gamma, None),
        ],
        "ctrl-k ctrl-t",
        &["workspace", "editor"],
        &[],
    );

    assert_eq!(
        verdicts(&resolution),
        [
            ("inspector_keys_test::Alpha", BindingVerdict::Wins),
            ("inspector_keys_test::Gamma", BindingVerdict::Pending),
        ]
    );
    assert_eq!(
        resolution.keystrokes.as_slice(),
        keystrokes("ctrl-k ctrl-t")
    );
}

#[test]
fn unhandled_action_falls_through_to_the_next_binding() {
    let bindings = || {
        vec![
            KeyBinding::new("ctrl-a", Alpha, None),
            KeyBinding::new("ctrl-a", Beta, None),
            KeyBinding::new("ctrl-a", Gamma, Some("pane")),
        ]
    };

    let resolution = resolve(bindings(), "ctrl-a", &["pane", "editor"], &[&Beta]);
    assert_eq!(
        verdicts(&resolution),
        [
            ("inspector_keys_test::Alpha", BindingVerdict::Wins),
            ("inspector_keys_test::Beta", BindingVerdict::Unhandled),
            (
                "inspector_keys_test::Gamma",
                BindingVerdict::Shadowed { by: 0 }
            ),
        ]
    );

    let resolution = resolve(bindings(), "ctrl-a", &["pane"], &[&Alpha, &Beta, &Gamma]);
    assert!(resolution.winner().is_none());
    assert!(
        resolution
            .candidates
            .iter()
            .all(|candidate| candidate.verdict == BindingVerdict::Unhandled)
    );
}

#[test]
fn bindings_without_a_context_match_an_empty_stack() {
    let resolution = resolve(
        vec![
            KeyBinding::new("a", Alpha, None),
            KeyBinding::new("a", Beta, Some("!editor")),
        ],
        "a",
        &[],
        &[],
    );

    assert_eq!(
        verdicts(&resolution),
        [
            ("inspector_keys_test::Alpha", BindingVerdict::Wins),
            // Predicates never match an empty stack, not even negations.
            ("inspector_keys_test::Beta", BindingVerdict::ContextMismatch),
        ]
    );
    assert_eq!(resolution.candidates[0].matched_depth, Some(0));
}

/// A view nesting key contexts; the innermost element is focused.
struct Nested {
    focus: FocusHandle,
    contexts: Vec<&'static str>,
    handled: Vec<Vec<usize>>,
    log: Rc<RefCell<Vec<usize>>>,
}

impl Render for Nested {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut element = div().track_focus(&self.focus).size_full();
        for (context, handled) in self.contexts.iter().zip(&self.handled).rev() {
            let mut level = div().size_full();
            if !context.is_empty() {
                level = level.key_context(KeyContext::parse(context).unwrap());
            }
            for &action in handled {
                level = on_test_action(level, action, self.log.clone());
            }
            element = level.child(element);
        }
        element
    }
}

/// Registers a listener for the `action`th test action that logs it.
fn on_test_action(level: Div, action: usize, log: Rc<RefCell<Vec<usize>>>) -> Div {
    fn logged<A: Action>(level: Div, action: usize, log: Rc<RefCell<Vec<usize>>>) -> Div {
        level.on_action(move |_: &A, _, _| log.borrow_mut().push(action))
    }
    match action {
        0 => logged::<Alpha>(level, action, log),
        1 => logged::<Beta>(level, action, log),
        2 => logged::<Gamma>(level, action, log),
        _ => logged::<Delta>(level, action, log),
    }
}

fn test_action(action: usize) -> Box<dyn Action> {
    match action {
        0 => Box::new(Alpha),
        1 => Box::new(Beta),
        2 => Box::new(Gamma),
        _ => Box::new(Delta),
    }
}

/// Opens a [`Nested`] view with its innermost element focused.
fn open_nested<'a>(
    cx: &'a mut TestAppContext,
    contexts: Vec<&'static str>,
    handled: Vec<Vec<usize>>,
) -> (Rc<RefCell<Vec<usize>>>, &'a mut VisualTestContext) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view({
        let log = log.clone();
        |_, cx| Nested {
            focus: cx.focus_handle(),
            contexts,
            handled,
            log,
        }
    });
    cx.update(|window, cx| view.read(cx).focus.clone().focus(window, cx));
    cx.run_until_parked();
    (log, cx)
}

#[gpui::test]
fn window_resolution_uses_focus_handlers_and_global_listeners(cx: &mut TestAppContext) {
    let (log, cx) = open_nested(
        cx,
        vec!["workspace", "", "editor"],
        vec![vec![0], vec![], vec![1]],
    );
    cx.update(|_, cx| {
        cx.on_action(|_: &Gamma, _| {});
        cx.bind_keys([
            KeyBinding::new("a", Alpha, None),
            KeyBinding::new("a", Delta, Some("editor")),
            KeyBinding::new("b", Gamma, Some("workspace")),
            KeyBinding::new("c", Beta, Some("workspace > editor")),
            KeyBinding::new("c d", Alpha, None),
        ]);
    });

    let resolution =
        cx.update(|window, cx| window.inspector_resolve_keystrokes(&keystrokes("a"), cx));
    assert_eq!(resolution.context_stack, contexts(&["workspace", "editor"]));
    assert_eq!(
        verdicts(&resolution),
        [
            ("inspector_keys_test::Alpha", BindingVerdict::Wins),
            ("inspector_keys_test::Delta", BindingVerdict::Unhandled),
        ]
    );

    let winner = |cx: &mut VisualTestContext, input: &str| {
        cx.update(|window, cx| {
            window
                .inspector_resolve_keystrokes(&keystrokes(input), cx)
                .winner()
                .map(|winner| winner.action.clone())
        })
    };
    assert_eq!(
        winner(cx, "b").as_deref(),
        Some("inspector_keys_test::Gamma")
    );
    assert_eq!(
        winner(cx, "c").as_deref(),
        Some("inspector_keys_test::Beta")
    );
    assert_eq!(
        winner(cx, "c d").as_deref(),
        Some("inspector_keys_test::Alpha")
    );

    // Resolving is pure: nothing ran and no keystroke is pending.
    assert!(log.borrow().is_empty());
    cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
}

// Property tests.

const KEYS: [&str; 4] = ["a", "b", "ctrl-a", "ctrl-b"];
const ACTIONS: usize = 4;

/// What a generated binding does.
#[derive(Clone, Debug)]
enum TestTarget {
    Action(usize),
    NoAction,
    Unbind(usize),
}

#[derive(Clone, Debug)]
struct TestBinding {
    keystrokes: Vec<&'static str>,
    target: TestTarget,
    predicate: Option<String>,
}

impl TestBinding {
    fn build(&self) -> KeyBinding {
        let action: Box<dyn Action> = match self.target {
            TestTarget::Action(action) => test_action(action),
            TestTarget::NoAction => Box::new(NoAction),
            TestTarget::Unbind(action) => Box::new(Unbind(test_action(action).name().into())),
        };
        let predicate = self
            .predicate
            .as_deref()
            .map(|predicate| KeyBindingContextPredicate::parse(predicate).unwrap().into());
        KeyBinding::load(
            &self.keystrokes.join(" "),
            action,
            predicate,
            false,
            None,
            &DummyKeyboardMapper,
        )
        .unwrap()
    }
}

/// A random keymap, focused context stack (with the actions each level
/// handles), actions handled globally, and keystrokes to type.
#[derive(Clone, Debug)]
struct DispatchCase {
    bindings: Vec<TestBinding>,
    contexts: Vec<&'static str>,
    handled: Vec<Vec<usize>>,
    global: Vec<usize>,
    input: Vec<&'static str>,
}

/// Predicates over `x`, `y`, `z` and `mode`, using `&&`, `||`, `!`, `>` and
/// `==` / `!=`, weighted towards ones that often match the generated stacks.
fn predicate_strategy() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        4 => select(vec!["x", "y", "z"]),
        1 => select(vec!["mode == m1", "mode != m2"]),
    ]
    .prop_map(String::from);
    leaf.prop_recursive(2, 6, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("({a}) && ({b})")),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("({a}) || ({b})")),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("({a}) > ({b})")),
            inner.prop_map(|a| format!("!({a})")),
        ]
    })
}

/// Bindings over a small alphabet, mostly short, so that they collide.
fn binding_strategy() -> impl Strategy<Value = TestBinding> {
    let key = || select(KEYS.to_vec());
    let target = prop_oneof![
        6 => (0..ACTIONS).prop_map(TestTarget::Action),
        1 => Just(TestTarget::NoAction),
        1 => (0..ACTIONS).prop_map(TestTarget::Unbind),
    ];
    (
        prop_oneof![3 => vec(key(), 1), 2 => vec(key(), 2), 1 => vec(key(), 3)],
        target,
        prop::option::weighted(0.6, predicate_strategy()),
    )
        .prop_map(|(keystrokes, target, predicate)| TestBinding {
            keystrokes,
            target,
            predicate,
        })
}

fn context_strategy() -> impl Strategy<Value = &'static str> {
    select(vec![
        "",
        "x",
        "y",
        "z",
        "x y",
        "x mode=m1",
        "y mode=m2",
        "z mode=m1",
        "x y z mode=m2",
    ])
}

fn dispatch_case_strategy() -> impl Strategy<Value = DispatchCase> {
    let handled = || vec(0..ACTIONS, 0..=3);
    (
        vec(binding_strategy(), 1..14),
        vec((context_strategy(), handled()), 0..=4),
        vec(0..ACTIONS, 0..=1),
        vec(select(KEYS.to_vec()), 1..=5),
    )
        .prop_map(|(bindings, levels, global, input)| {
            let (contexts, handled) = levels.into_iter().unzip();
            DispatchCase {
                bindings,
                contexts,
                handled,
                global,
                input,
            }
        })
}

/// A random keymap, context stack and input.
#[derive(Clone, Debug)]
struct KeymapCase {
    bindings: Vec<TestBinding>,
    contexts: Vec<&'static str>,
    input: Vec<&'static str>,
}

fn keymap_case_strategy() -> impl Strategy<Value = KeymapCase> {
    let key = || select(KEYS.to_vec());
    (
        vec(binding_strategy(), 1..16),
        vec(context_strategy(), 0..=4),
        prop_oneof![3 => vec(key(), 1), 2 => vec(key(), 2), 1 => vec(key(), 3)],
    )
        .prop_map(|(bindings, contexts, input)| KeymapCase {
            bindings,
            contexts,
            input,
        })
}

proptest! {
    #![proptest_config(gpui::apply_seed_to_proptest_config(ProptestConfig::with_cases(4096)))]

    /// `explain_input` mirrors `bindings_for_input` rule for rule.
    #[test]
    fn explain_input_agrees_with_bindings_for_input(case in keymap_case_strategy()) {
        check_explain_input(case);
    }
}

proptest! {
    #![proptest_config(gpui::apply_seed_to_proptest_config(ProptestConfig::with_cases(1000)))]

    /// The resolver predicts what real dispatch does for each keystroke typed,
    /// including multi-stroke waits, replays after a mismatch and the timeout.
    #[test]
    fn resolution_matches_real_dispatch(case in dispatch_case_strategy()) {
        gpui::run_test_once(0, Box::new(move |dispatcher| {
            let mut cx = TestAppContext::build(dispatcher, None);
            check_dispatch(&mut cx, case);
            cx.quit();
        }));
    }
}

fn check_explain_input(case: KeymapCase) {
    let KeymapCase {
        bindings,
        contexts: stack,
        input,
    } = case;
    let keymap = Keymap::new(bindings.iter().map(TestBinding::build).collect());
    let stack = contexts(&stack);
    let input: Vec<Keystroke> = input
        .iter()
        .map(|keystroke| Keystroke::parse(keystroke).unwrap())
        .collect();

    let (dispatched, pending) = keymap.bindings_for_input(&input, &stack);
    let explained = keymap.explain_input(&input, &stack);

    let describe = |binding: &KeyBinding| {
        (
            binding.action().name(),
            super::binding_keystrokes(binding),
            super::binding_predicate(binding),
        )
    };
    let expected: Vec<_> = dispatched.iter().map(describe).collect();
    let actual: Vec<_> = explained
        .iter()
        .filter(|explained| explained.outcome == InputOutcome::Dispatched)
        .map(|explained| describe(explained.binding))
        .collect();
    assert_eq!(actual, expected);
    let waits = explained
        .iter()
        .any(|explained| explained.outcome == InputOutcome::Waits);
    assert_eq!(waits, pending);
}

/// Types `case.input` into a live window, checking before each keystroke that
/// resolutions predict the actions dispatch runs and what it leaves pending.
fn check_dispatch(cx: &mut TestAppContext, case: DispatchCase) {
    let (log, cx) = open_nested(cx, case.contexts.clone(), case.handled.clone());
    cx.update(|_, cx| {
        for &action in &case.global {
            on_global_test_action(cx, action, log.clone());
        }
        cx.bind_keys(case.bindings.iter().map(TestBinding::build));
    });

    let resolve = |cx: &mut VisualTestContext, input: &[Keystroke]| {
        cx.update(|window, cx| window.inspector_resolve_keystrokes(input, cx))
    };
    let mut pending: Vec<Keystroke> = Vec::new();
    let mut needs_timeout = false;
    for keystroke in &case.input {
        let keystroke = Keystroke::parse(keystroke).unwrap().with_simulated_ime();
        let mut expected = Vec::new();
        let outcome = expect_dispatch(
            &mut |input| resolve(cx, input),
            mem::take(&mut pending),
            keystroke.clone(),
            &mut expected,
        );
        needs_timeout = !outcome.pending.is_empty() && (needs_timeout || outcome.has_binding);
        pending = outcome.pending;

        cx.update(|window, cx| window.dispatch_keystroke(keystroke.clone(), cx));
        cx.run_until_parked();

        let fired: Vec<_> = log.borrow_mut().drain(..).collect();
        assert_eq!(fired, expected, "actions run for {keystroke}");
        let actual_pending = cx.update(|window, _| {
            window
                .pending_input_keystrokes()
                .map(<[Keystroke]>::to_vec)
                .unwrap_or_default()
        });
        assert_eq!(actual_pending, pending, "pending after {keystroke}");
    }

    let mut expected = Vec::new();
    if needs_timeout {
        expect_flush(&mut |input| resolve(cx, input), pending, &mut expected);
    }
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    let fired: Vec<_> = log.borrow_mut().drain(..).collect();
    assert_eq!(fired, expected, "actions run by the timeout");
}

/// Registers a global listener for the `action`th test action that logs it.
fn on_global_test_action(cx: &mut App, action: usize, log: Rc<RefCell<Vec<usize>>>) {
    fn logged<A: Action>(cx: &mut App, action: usize, log: Rc<RefCell<Vec<usize>>>) {
        cx.on_action(move |_: &A, _| log.borrow_mut().push(action));
    }
    match action {
        0 => logged::<Alpha>(cx, action, log),
        1 => logged::<Beta>(cx, action, log),
        2 => logged::<Gamma>(cx, action, log),
        _ => logged::<Delta>(cx, action, log),
    }
}

/// What dispatching `keystroke` after `pending` leaves pending.
struct Outcome {
    pending: Vec<Keystroke>,
    /// Whether the pending input also completes a binding.
    has_binding: bool,
}

/// Predicts `DispatchTree::dispatch_key` from resolutions alone, pushing the
/// actions that would run onto `fired`.
fn expect_dispatch(
    resolve: &mut impl FnMut(&[Keystroke]) -> KeyResolution,
    mut input: Vec<Keystroke>,
    keystroke: Keystroke,
    fired: &mut Vec<usize>,
) -> Outcome {
    input.push(keystroke.clone());
    let resolution = resolve(&input);
    if resolution.is_pending() {
        return Outcome {
            pending: input,
            has_binding: has_binding(&resolution),
        };
    }
    if has_binding(&resolution) || input.len() == 1 {
        fire_winner(&resolution, fired);
        return Outcome {
            pending: Vec::new(),
            has_binding: false,
        };
    }
    input.pop();
    let suffix = expect_replay_prefix(resolve, input, fired);
    expect_dispatch(resolve, suffix, keystroke, fired)
}

/// Predicts `DispatchTree::flush_dispatch`.
fn expect_flush(
    resolve: &mut impl FnMut(&[Keystroke]) -> KeyResolution,
    mut input: Vec<Keystroke>,
    fired: &mut Vec<usize>,
) {
    while !input.is_empty() {
        input = expect_replay_prefix(resolve, input, fired);
    }
}

/// Predicts `DispatchTree::replay_prefix`: the longest prefix that completes a
/// binding runs, or the first keystroke is replayed as plain input.
fn expect_replay_prefix(
    resolve: &mut impl FnMut(&[Keystroke]) -> KeyResolution,
    mut input: Vec<Keystroke>,
    fired: &mut Vec<usize>,
) -> Vec<Keystroke> {
    for last in (0..input.len()).rev() {
        let resolution = resolve(&input[..=last]);
        if has_binding(&resolution) {
            fire_winner(&resolution, fired);
            return input.split_off(last + 1);
        }
    }
    input.remove(0);
    input
}

/// Whether some binding for exactly this input is tried by dispatch.
fn has_binding(resolution: &KeyResolution) -> bool {
    resolution.candidates.iter().any(|candidate| {
        matches!(
            candidate.verdict,
            BindingVerdict::Wins | BindingVerdict::Unhandled
        )
    })
}

fn fire_winner(resolution: &KeyResolution, fired: &mut Vec<usize>) {
    if let Some(winner) = resolution.winner() {
        let action = (0..ACTIONS)
            .find(|&action| test_action(action).name() == winner.action.as_ref())
            .expect("only test actions win");
        fired.push(action);
    }
}
