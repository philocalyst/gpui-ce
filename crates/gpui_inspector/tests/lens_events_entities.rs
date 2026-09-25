//! End-to-end tests of the Events and Entities lenses, rendered headless
//! and driven with real input: against the Inbox fixture, and against a
//! small live mail app with a keymap (bindings in nested contexts, a
//! multi-key binding, a `NoAction` override, an unhandled action) whose
//! handlers and renders are counted, so the tests can tell what really
//! happened. Every surface is saved to `target/loupe-shots/`.

use gpui::{
    Action as _, AppContext as _, Context, Entity, EntityId, EventEmitter, FocusHandle,
    InteractiveElement as _, IntoElement, KeyBinding, Keystroke, NoAction, ParentElement as _,
    Pixels, PlatformInput, Render, ScrollDelta, ScrollWheelEvent, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, actions, div,
    inspector::{
        CauseKind, ElementKey, ElementKind, EntityInfo, InputKind, InspectorCapture, InspectorDock,
        NotifyStats, ViewOutcome,
    },
    px, rgb, size,
};
use gpui_inspector::{
    Lens, LensLayout, REFRESH_INTERVAL,
    fixtures::{self, InboxElements, InputBuilder, ms},
    harness::LoupeHarness,
    theme::Appearance,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    time::{Duration, Instant},
};

// ---------------------------------------------------------------------------
// The Inbox fixture.

/// A stand-in for the inspected app behind fixture captures.
struct Backdrop;

impl Render for Backdrop {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .bg(rgb(0xffffff))
            .font_family(gpui_inspector::UI_FONT)
            .text_color(rgb(0x1f2328))
            .child("Inbox")
    }
}

fn inbox_harness(width: f32, height: f32) -> (LoupeHarness, InboxElements) {
    let mut harness = LoupeHarness::new(size(px(width), px(height)), |_, cx| cx.new(|_| Backdrop));
    harness.open_loupe();
    let (capture, elements) = fixtures::inbox();
    harness.install_capture(capture);
    harness.set_appearance(Appearance::Dark);
    (harness, elements)
}

fn show(harness: &mut LoupeHarness, lens: Lens) {
    harness.click_text(lens.label());
    assert_eq!(harness.state(|state| state.lens()), lens);
}

/// The app records in the ring, oldest first.
fn app_input(harness: &mut LoupeHarness) -> Vec<gpui::inspector::InputRecord> {
    harness.capture(|capture| {
        capture
            .input()
            .iter()
            .filter(|record| !record.inspector)
            .cloned()
            .collect()
    })
}

/// Pushes a fixture record and lets Loupe catch up.
fn push_input(harness: &mut LoupeHarness, build: impl FnOnce(&mut InspectorCapture) -> u64) -> u64 {
    let seq = harness.update(|window, _| build(window.inspector_capture_mut().unwrap()));
    harness.advance(REFRESH_INTERVAL);
    seq
}

/// Panics unless every line Loupe painted fits its container across: text
/// too long for its box must be truncated with an ellipsis, never clipped.
fn assert_nothing_overflows(harness: &mut LoupeHarness) {
    let dock = harness.update(|window, _| window.inspector_bounds().unwrap());
    for line in harness.painted_text() {
        let visible = line.visible_bounds();
        if visible.size.width <= px(0.) || visible.left() < dock.left() {
            continue;
        }
        assert!(
            line.bounds.right() <= line.clip.right() + px(1.),
            "{:?} is clipped at {:?} instead of truncated: {:?}",
            line.text,
            line.clip.right(),
            line.bounds
        );
    }
}

#[test]
fn the_log_lists_inbox_input_with_kind_text_and_loupe_filters() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    show(&mut harness, Lens::Events);
    let input = app_input(&mut harness);
    let loupe = harness.capture(|capture| capture.input().len()) - input.len();
    harness.assert_text_visible(&format!("{} events", input.len()));
    // The rail counts app input, and the question waits for a selection.
    harness.assert_text_visible(&input.len().to_string());
    harness.assert_text_visible(Lens::Events.question());
    // Following: the newest record is on screen.
    let newest = input.last().unwrap();
    assert!(harness.find_text(&clock(newest.at)).is_some());
    harness.screenshot("events-log-dark");
    assert_nothing_overflows(&mut harness);

    // Kind chips.
    let keys = input
        .iter()
        .filter(|record| record.kind == InputKind::KeyDown)
        .count();
    harness.click_selector("events-kind-1");
    harness.assert_text_visible(&format!("{keys} of {} events", input.len()));
    assert!(
        harness.find_text("left ×1 at").is_none(),
        "clicks are hidden"
    );
    harness.screenshot("events-filter-keys");
    let with_actions = input
        .iter()
        .filter(|record| !record.actions.is_empty())
        .count();
    harness.click_selector("events-kind-3");
    harness.assert_text_visible(&format!("{with_actions} of {} events", input.len()));

    // Text filter with an exclusion: key presses that did not select.
    harness.click_selector("events-kind-0");
    harness.click_text("Filter events · -exclude");
    harness.type_text("key -select");
    let archived = input
        .iter()
        .filter(|record| {
            record.kind == InputKind::KeyDown
                && !record
                    .actions
                    .iter()
                    .any(|action| action.name.contains("Select"))
        })
        .count();
    harness.assert_text_visible(&format!("{archived} of {} events", input.len()));
    assert!(harness.find_text("SelectNext").is_none());
    harness.screenshot("events-filter-exclusion");
    assert_eq!(
        harness.state(|state| state.filters().events.to_string()),
        "key -select"
    );
    for _ in 0.."key -select".len() {
        harness.type_keys("backspace");
    }

    // Loupe's own input is hidden until asked for.
    harness.assert_text_visible(&format!("{} events", input.len()));
    harness.click_text("Show Loupe's input");
    harness.assert_text_visible(&format!("{} events", input.len() + loupe));
    harness.screenshot("events-loupe-input");
}

/// `12.480`, as the log's time column shows it.
fn clock(at: Duration) -> String {
    gpui_inspector::analysis::events::clock(at)
}

#[test]
fn the_log_follows_new_input_until_the_user_selects_or_scrolls_up() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    show(&mut harness, Lens::Events);
    harness.assert_text_visible("Follow");

    // New input scrolls into view while following.
    let seq = push_input(&mut harness, |capture| {
        InputBuilder::key(ms(9_000.), "x")
            .action("inbox::Extra", true, None)
            .push(capture)
    });
    harness.assert_text_visible(&clock(ms(9_000.)));

    // Selecting pauses; new input is counted, not scrolled to.
    harness.click_selector(&format!("events-row-{seq}"));
    assert_eq!(harness.state(|state| state.selected_input()), Some(seq));
    for ix in 0..3 {
        push_input(&mut harness, |capture| {
            InputBuilder::key(ms(9_100. + ix as f64 * 10.), "y").push(capture)
        });
    }
    harness.assert_text_visible("Paused · 3 new");
    harness.screenshot("events-paused");

    // j/k move the selection through the listed rows (Loupe's own are hidden).
    let listed: Vec<u64> = app_input(&mut harness)
        .iter()
        .map(|record| record.seq)
        .collect();
    let row = listed.iter().position(|&listed| listed == seq).unwrap();
    harness.type_keys("k");
    assert_eq!(
        harness.state(|state| state.selected_input()),
        Some(listed[row - 1])
    );
    harness.type_keys("j j");
    assert_eq!(
        harness.state(|state| state.selected_input()),
        Some(listed[row + 1])
    );

    // The chip resumes following: the newest record comes back into view.
    harness.click_text("Paused · 3 new");
    harness.assert_text_visible("Follow");
    harness.assert_text_visible(&clock(ms(9_120.)));

    // Scrolling up pauses too, and scrolling back to the end resumes.
    let newest = harness.bounds_of(&format!("events-row-{}", seq + 3));
    scroll(&mut harness, newest.center(), 400.);
    harness.assert_text_visible("Paused");
    scroll(&mut harness, newest.center(), -40_000.);
    harness.assert_text_visible("Follow");
}

/// Scrolls the wheel by `delta_y` pixels at `position` (positive scrolls up).
fn scroll(harness: &mut LoupeHarness, position: gpui::Point<Pixels>, delta_y: f32) {
    harness.hover(position);
    harness.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(gpui::point(px(0.), px(delta_y))),
                ..ScrollWheelEvent::default()
            }),
            cx,
        );
    });
    harness.draw();
}

#[test]
fn the_detail_tells_where_a_fixture_click_went() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    show(&mut harness, Lens::Events);
    let click = app_input(&mut harness)
        .into_iter()
        .rev()
        .find(|record| record.kind == InputKind::MouseDown)
        .unwrap();
    harness.click_selector(&format!("events-row-{}", click.seq));
    harness.assert_text_visible(" on div#row-3 → inbox::OpenIssue · handled in ");
    harness.assert_text_visible("HIT PATH");
    harness.screenshot("events-detail-click");

    // Hovering a hit-path row highlights it in the app; clicking selects it.
    let hit = harness.bounds_of("events-hit-0");
    harness.hover(hit.center());
    assert_eq!(
        harness.capture(|capture| capture.overlay().hovered),
        Some(elements.row_3)
    );
    harness.click_selector("events-hit-1");
    assert_eq!(
        harness.state(|state| state.selected_element()),
        Some(elements.issue_list)
    );
    assert_eq!(
        harness.capture(|capture| capture.overlay().selected),
        Some(elements.issue_list)
    );
    harness.hover(gpui::point(px(100.), px(400.)));
    assert_eq!(harness.capture(|capture| capture.overlay().hovered), None);

    // The frame it caused opens in Frames.
    harness.click_selector("events-frame");
    assert_eq!(harness.state(|state| state.lens()), Lens::Frames);
    assert_eq!(harness.state(|state| state.selected_frame()), click.frame);

    harness.set_appearance(Appearance::Light);
    show(&mut harness, Lens::Events);
    harness.screenshot("events-detail-click-light");
}

// ---------------------------------------------------------------------------
// The live mail app.

actions!(
    mail,
    [
        /// Saves the draft (the editor handles it).
        Save,
        /// Saves everything (the workspace handles it).
        SaveAll,
        /// `ctrl-k ctrl-t`: trims the draft.
        Trim,
        /// Closes the pane.
        Kill,
        /// Bound only in vim mode.
        Close,
        /// Deletes (the editor turns it off with `NoAction`).
        Delete,
        /// Archives (the workspace handles it).
        Archive,
        /// Bound in the editor, handled nowhere.
        Orphan,
        /// Dispatched by the compose button.
        Compose,
    ]
);

/// The actions whose handlers ran, in order.
type ActionLog = Rc<RefCell<Vec<&'static str>>>;

/// Render counts per view.
#[derive(Clone, Default)]
struct Renders {
    workspace: Rc<Cell<usize>>,
    pane: Rc<Cell<usize>>,
    editor: Rc<Cell<usize>>,
}

impl Renders {
    fn total(&self) -> usize {
        self.workspace.get() + self.pane.get() + self.editor.get()
    }
}

/// A model the workspace observes and the pane subscribes to.
struct Store {
    drafts: usize,
}

struct StoreChanged;

impl EventEmitter<StoreChanged> for Store {}

struct Editor {
    focus: FocusHandle,
    log: ActionLog,
    renders: Renders,
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.editor.set(self.renders.editor.get() + 1);
        let log = |name: &'static str, log: &ActionLog| {
            let log = log.clone();
            move || log.borrow_mut().push(name)
        };
        let (save, trim) = (log("Save", &self.log), log("Trim", &self.log));
        div()
            .id("mail-editor")
            .debug_selector(|| "mail-editor".into())
            .key_context("Editor mode=full")
            .track_focus(&self.focus)
            .on_action(move |_: &Save, _, _| save())
            .on_action(move |_: &Trim, _, _| trim())
            .flex_1()
            .p_3()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0xd0d7de))
            .child("Dear Ada, …")
    }
}

struct Pane {
    editor: Entity<Editor>,
    _store: Entity<Store>,
    log: ActionLog,
    renders: Renders,
    _subscription: Subscription,
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.pane.set(self.renders.pane.get() + 1);
        let log = self.log.clone();
        div()
            .key_context("Pane")
            .on_action(move |_: &Kill, _, _| log.borrow_mut().push("Kill"))
            .flex_1()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .child(
                div()
                    .id("compose-button")
                    .debug_selector(|| "compose-button".into())
                    .w(px(120.))
                    .p_2()
                    .bg(rgb(0x1f883d))
                    .text_color(rgb(0xffffff))
                    .child("Compose")
                    .on_click(|_, window, cx| window.dispatch_action(Compose.boxed_clone(), cx)),
            )
            .child(self.editor.clone())
    }
}

struct Workspace {
    pane: Entity<Pane>,
    store: Entity<Store>,
    log: ActionLog,
    renders: Renders,
    _observation: Subscription,
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.workspace.set(self.renders.workspace.get() + 1);
        let on = |name: &'static str, log: &ActionLog| {
            let log = log.clone();
            move || log.borrow_mut().push(name)
        };
        let (save_all, archive, delete, compose) = (
            on("SaveAll", &self.log),
            on("Archive", &self.log),
            on("Delete", &self.log),
            on("Compose", &self.log),
        );
        let drafts = self.store.read(cx).drafts;
        div()
            .key_context("Workspace")
            .on_action(move |_: &SaveAll, _, _| save_all())
            .on_action(move |_: &Archive, _, _| archive())
            .on_action(move |_: &Delete, _, _| delete())
            .on_action(move |_: &Compose, _, _| compose())
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xf6f8fa))
            .font_family(gpui_inspector::UI_FONT)
            .text_color(rgb(0x1f2328))
            .child(div().p_3().child(format!("Mail · {drafts} drafts")))
            .child(self.pane.clone())
    }
}

/// The live app and what it exposes to the tests.
struct Mail {
    editor: Entity<Editor>,
    store: Entity<Store>,
    log: ActionLog,
    renders: Renders,
    // Last, so the app's entities are released before the app goes away.
    harness: LoupeHarness,
}

fn mail_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("ctrl-s", Save, Some("Editor")),
        KeyBinding::new("ctrl-s", SaveAll, Some("Workspace")),
        KeyBinding::new("ctrl-k ctrl-t", Trim, Some("Editor")),
        KeyBinding::new("ctrl-w", Kill, Some("Pane")),
        KeyBinding::new("ctrl-w", Close, Some("Editor && mode == vim")),
        KeyBinding::new("ctrl-d", Delete, Some("Workspace")),
        KeyBinding::new("ctrl-d", NoAction, Some("Editor")),
        KeyBinding::new("ctrl-u", Archive, Some("Workspace")),
        KeyBinding::new("ctrl-u", Orphan, Some("Editor")),
    ]
}

impl Mail {
    /// Opens the mail app with the editor focused, then Loupe.
    fn open(width: f32, height: f32) -> Self {
        let log = ActionLog::default();
        let renders = Renders::default();
        let built = Rc::new(RefCell::new(None));
        let harness = LoupeHarness::new(size(px(width), px(height)), {
            let (log, renders, built) = (log.clone(), renders.clone(), built.clone());
            move |window, cx| {
                let store = cx.new(|_| Store { drafts: 2 });
                let editor = cx.new(|cx| Editor {
                    focus: cx.focus_handle(),
                    log: log.clone(),
                    renders: renders.clone(),
                });
                let pane = cx.new(|cx| Pane {
                    editor: editor.clone(),
                    _store: store.clone(),
                    log: log.clone(),
                    renders: renders.clone(),
                    _subscription: cx.subscribe(&store, |_, _, _: &StoreChanged, _| {}),
                });
                let workspace = cx.new(|cx| Workspace {
                    pane,
                    store: store.clone(),
                    log: log.clone(),
                    renders: renders.clone(),
                    _observation: cx.observe(&store, |_, _, cx| cx.notify()),
                });
                editor.read(cx).focus.clone().focus(window, cx);
                *built.borrow_mut() = Some((editor, store));
                workspace
            }
        });
        let (editor, store) = built.borrow_mut().take().unwrap();
        let mut mail = Self {
            harness,
            editor,
            store,
            log,
            renders,
        };
        mail.harness.app(|cx| cx.bind_keys(mail_bindings()));
        mail.harness.draw();
        mail.harness.open_loupe();
        mail.harness.set_appearance(Appearance::Dark);
        mail
    }

    fn editor_focus(&mut self) -> FocusHandle {
        let editor = self.editor.clone();
        self.harness.app(|cx| editor.read(cx).focus.clone())
    }

    /// Clicks the editor, which focuses it.
    fn focus_editor(&mut self) {
        self.harness.click_selector("mail-editor");
        let focus = self.editor_focus();
        assert!(self.harness.update(|window, _| focus.is_focused(window)));
    }

    fn actions_run(&self) -> Vec<&'static str> {
        self.log.borrow().clone()
    }

    /// The key of the element with `id` in the latest captured tree.
    fn element(&mut self, id: &str) -> ElementKey {
        self.harness.capture(|capture| {
            capture
                .latest_tree()
                .unwrap()
                .elements
                .iter()
                .find(|record| record.id.as_deref() == Some(id))
                .and_then(|record| record.key)
                .unwrap_or_else(|| panic!("no element #{id}"))
        })
    }

    fn entity_info(&mut self, id: EntityId) -> EntityInfo {
        self.harness.update(|window, cx| {
            window
                .inspector_entities(cx)
                .into_iter()
                .find(|entity| entity.id == id)
                .unwrap()
        })
    }
}

fn keys(source: &str) -> Vec<Keystroke> {
    source
        .split_whitespace()
        .map(|key| Keystroke::parse(key).unwrap())
        .collect()
}

#[test]
fn live_clicks_and_keys_land_in_the_log_with_where_they_went() {
    let mut mail = Mail::open(1280., 800.);
    let harness = &mut mail.harness;
    show(harness, Lens::Events);

    // A real click on the compose button, after clicking into the editor
    // (Loupe had the focus, and actions go where the focus is).
    let compose = mail.element("compose-button");
    mail.focus_editor();
    let harness = &mut mail.harness;
    harness.click_selector("compose-button");
    harness.advance(REFRESH_INTERVAL);
    assert_eq!(mail.actions_run(), ["Compose"]);
    let input = app_input(&mut mail.harness);
    let down = input
        .iter()
        .rev()
        .find(|record| record.kind == InputKind::MouseDown)
        .unwrap();
    assert_eq!(
        down.hit_path.first(),
        Some(&compose),
        "topmost is the button"
    );
    let action = input
        .iter()
        .rev()
        .find(|record| record.kind == InputKind::Action)
        .unwrap();
    assert_eq!(action.detail.as_ref(), "mail::Compose");
    assert!(action.handled);
    let harness = &mut mail.harness;
    harness.click_selector(&format!("events-row-{}", down.seq));
    harness.assert_text_visible(" on div#compose-button · ");
    harness.assert_text_visible("div#compose-button");
    harness.screenshot("events-live-click");
    harness.click_selector("events-hit-0");
    assert_eq!(
        harness.state(|state| state.selected_element()),
        Some(compose)
    );
    harness.click_selector(&format!("events-row-{}", action.seq));
    harness.assert_text_visible("mail::Compose dispatched in Workspace > Pane > Editor mode=full");
    harness.screenshot("events-live-action");
    assert_nothing_overflows(harness);

    // Real keys, typed into the focused editor.
    mail.focus_editor();
    let harness = &mut mail.harness;
    harness.type_keys("ctrl-s ctrl-u ctrl-d");
    harness.advance(REFRESH_INTERVAL);
    assert_eq!(mail.actions_run(), ["Compose", "Save", "Archive"]);
    let input = app_input(&mut mail.harness);
    let key = |text: &str| {
        input
            .iter()
            .find(|record| record.kind == InputKind::KeyDown && record.detail.as_ref() == text)
            .unwrap()
            .clone()
    };
    let (save, archive, delete) = (key("ctrl-s"), key("ctrl-u"), key("ctrl-d"));
    assert!(save.handled && archive.handled && !delete.handled);

    let harness = &mut mail.harness;
    harness.click_selector(&format!("events-row-{}", save.seq));
    harness.assert_text_visible(
        "ctrl-s → mail::Save in Workspace > Pane > Editor mode=full · handled in ",
    );
    for context in ["Workspace", "Pane", "Editor mode=full"] {
        harness.assert_text_visible(context);
    }
    harness.screenshot("events-live-key");

    // The unhandled binding fell through to the workspace's.
    harness.click_selector(&format!("events-row-{}", archive.seq));
    harness.assert_text_visible("ctrl-u → mail::Archive (+1 more) in ");
    harness.assert_text_visible("mail::Orphan");
    harness.screenshot("events-live-fallthrough");
    // NoAction turned ctrl-d off: nothing handled it.
    harness.click_selector(&format!("events-row-{}", delete.seq));
    harness.assert_text_visible("ctrl-d → no binding; nothing handled it in ");
}

/// Resolves `input` in the key tester and returns the painted verdict of
/// the engine for the same keys against `focus`.
fn engine_winner(harness: &mut LoupeHarness, input: &str, focus: &FocusHandle) -> Option<String> {
    let keys = keys(input);
    harness.update(|window, cx| {
        window
            .inspector_resolve_keystrokes_for(&keys, Some(focus), cx)
            .winner()
            .map(|winner| winner.action.to_string())
    })
}

#[test]
fn the_key_tester_explains_keys_without_dispatching_anything() {
    let mut mail = Mail::open(1280., 800.);
    let editor_focus = mail.editor_focus();
    let toggle = if cfg!(target_os = "macos") {
        "cmd-alt-i"
    } else {
        "ctrl-shift-i"
    };
    let harness = &mut mail.harness;
    show(harness, Lens::Events);
    harness.click_text("Key tester");
    harness.assert_text_visible("Press keys to see what they do here…");
    harness.screenshot("key-tester-idle");

    harness.click_selector("events-key-tester");
    harness.assert_text_visible("Listening… press keys");
    harness.screenshot("key-tester-listening");
    let frozen = harness.capture(|capture| capture.is_frozen());
    // The first key after the click switches GPUI's input modality from
    // mouse to keyboard, which refreshes the whole window once (before any
    // dispatch, with or without Loupe); count the app's renders after it.
    harness.type_keys("escape");
    let renders = mail.renders.total();

    // Each case: what the tester shows, checked against the engine.
    let cases: [(&str, &str, Option<&str>); 6] = [
        (
            "ctrl-s",
            "Runs mail::Save · bound in Editor · matched the innermost context",
            Some("mail::Save"),
        ),
        (
            "ctrl-w",
            "Runs mail::Kill · bound in Pane · matched 1 level up",
            Some("mail::Kill"),
        ),
        (
            "ctrl-d",
            "Nothing runs: zed::NoAction turns ctrl-d off here",
            None,
        ),
        (
            "ctrl-u",
            "Runs mail::Archive · bound in Workspace · matched 2 levels up",
            Some("mail::Archive"),
        ),
        (
            toggle,
            "Runs loupe::ToggleInspector · bound globally (no context)",
            Some("loupe::ToggleInspector"),
        ),
        (
            "space",
            "Nothing runs: space is bound, but not in this context",
            None,
        ),
    ];
    let shots = [
        "key-tester-save",
        "key-tester-context",
        "key-tester-disabled",
        "key-tester-unhandled",
        "key-tester-global",
        "key-tester-unbound",
    ];
    for ((input, shown, winner), shot) in cases.iter().zip(shots) {
        let harness = &mut mail.harness;
        harness.type_keys(input);
        harness.assert_text_visible(shown);
        assert_eq!(
            engine_winner(harness, input, &editor_focus).as_deref(),
            *winner,
            "{input}"
        );
        harness.screenshot(shot);
    }
    let harness = &mut mail.harness;
    // Every loser is explained.
    harness.type_keys("ctrl-s");
    harness.assert_text_visible("Outranked by mail::Save: its context is deeper");
    harness.type_keys("ctrl-w");
    harness.assert_text_visible("Context Editor && mode == vim is false here");
    harness.type_keys("ctrl-d");
    harness.assert_text_visible("Turned off by zed::NoAction in Editor");
    harness.type_keys("ctrl-u");
    harness.assert_text_visible(
        "Nothing on the focus path handles mail::Orphan, so dispatch falls through",
    );
    harness.assert_text_visible("Workspace");
    harness.assert_text_visible("Editor mode=full");

    // A multi-key binding: the first key waits, the second completes it.
    harness.type_keys("ctrl-k");
    harness.assert_text_visible("Waiting for more keys: ctrl-t runs mail::Trim");
    harness.screenshot("key-tester-pending");
    harness.type_keys("ctrl-t");
    harness.assert_text_visible("Runs mail::Trim · bound in Editor");
    assert_eq!(
        engine_winner(harness, "ctrl-k ctrl-t", &editor_focus).as_deref(),
        Some("mail::Trim")
    );
    harness.screenshot("key-tester-multi-key");

    // Loupe's own shortcuts were captured too: no lens switch, no freeze,
    // no palette, and Loupe is still open.
    harness.type_keys("alt-2 secondary-k");
    assert_eq!(harness.state(|state| state.lens()), Lens::Events);
    assert_eq!(harness.capture(|capture| capture.is_frozen()), frozen);
    let loupe = harness.loupe();
    assert!(!harness.app(|cx| loupe.read(cx).is_palette_open()));
    assert!(harness.update(|window, _| window.is_inspector_open()));

    // Nothing ran in the app, and the app never re-rendered.
    assert!(mail.actions_run().is_empty(), "{:?}", mail.actions_run());
    assert_eq!(mail.renders.total(), renders, "the app re-rendered");

    // Escape twice clears, a third escape stops listening.
    let harness = &mut mail.harness;
    harness.type_keys("escape escape");
    harness.assert_text_visible("Listening… press keys");
    harness.type_keys("escape");
    harness.assert_text_visible("Press keys to see what they do here…");

    // Real dispatch agrees with every verdict.
    mail.focus_editor();
    let mut expected = Vec::new();
    for (input, _, winner) in &cases[..4] {
        mail.harness.type_keys(input);
        expected.extend(winner.map(|winner| winner.trim_start_matches("mail::")));
    }
    mail.harness.type_keys("ctrl-k ctrl-t");
    expected.push("Trim");
    assert_eq!(mail.actions_run(), expected);
}

#[test]
fn the_key_tester_looks_right_in_light_and_narrow_docks() {
    let mut mail = Mail::open(900., 720.);
    let harness = &mut mail.harness;
    harness.update(|window, _| {
        window
            .inspector_capture_mut()
            .unwrap()
            .set_dock(InspectorDock::Right { width: px(400.) })
    });
    harness.redraw_all();
    assert_eq!(
        harness.update(|window, _| LensLayout::of(window)),
        LensLayout::Stacked
    );
    show(harness, Lens::Events);
    harness.click_text("Key tester");
    harness.click_selector("events-key-tester");
    harness.type_keys("ctrl-s");
    harness.assert_text_visible("Runs mail::Save");
    harness.screenshot("key-tester-narrow-dark");
    assert_nothing_overflows(harness);
    harness.set_appearance(Appearance::Light);
    harness.type_keys("ctrl-d");
    harness.screenshot("key-tester-narrow-light");
    assert_nothing_overflows(harness);
}

// ---------------------------------------------------------------------------
// Entities.

/// The painted top of the first line equal to `text`.
fn top_of(harness: &mut LoupeHarness, text: &str) -> Pixels {
    harness
        .find_text(text)
        .unwrap_or_else(|| panic!("{text:?} is not visible"))
        .origin
        .y
}

#[test]
fn the_entities_table_sorts_filters_and_explains_the_inbox() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    show(&mut harness, Lens::Entities);
    harness.assert_text_visible(Lens::Entities.question());
    // Busiest first: the store storms at the top.
    assert!(top_of(&mut harness, "IssueStore") < top_of(&mut harness, "IssueList"));
    assert!(top_of(&mut harness, "IssueList") < top_of(&mut harness, "InboxApp"));
    harness.assert_text_visible("7 entities · 5 views · 5 notifying");
    harness.screenshot("entities-dark");
    assert_nothing_overflows(&mut harness);

    // Sort by type, then views only, then a filter with an exclusion.
    harness.click_text("Type");
    assert!(top_of(&mut harness, "InboxApp") < top_of(&mut harness, "SyncClient"));
    harness.click_selector("entities-kind-1");
    assert!(harness.find_text("IssueStore").is_none());
    harness.assert_text_visible("5 entities · 5 views");
    harness.screenshot("entities-views");
    harness.click_selector("entities-kind-0");
    harness.click_text("Filter entities · -exclude");
    harness.type_text("issue -list");
    harness.assert_text_visible("2 entities");
    assert!(harness.find_text("IssueList").is_none());
    for _ in 0.."issue -list".len() {
        harness.type_keys("backspace");
    }

    // The store's detail: the sentence first, counts, history and site.
    let store = EntityId::from(fixtures::entities::STORE);
    harness.click_selector("entities-row-6");
    assert_eq!(harness.state(|state| state.selected_entity()), Some(store));
    harness.assert_text_visible("IssueStore is notifying");
    harness.assert_text_visible("3 observers run on each notify, 2 subscribers listen");
    harness.assert_text_visible("peak ");
    harness.assert_text_visible("fixtures.rs:");
    harness.screenshot("entities-store");
    harness.click_selector("entities-site");
    harness.assert_text_visible("Copied");
    let copied = harness.app(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert!(copied.is_some_and(|text| text.contains("fixtures.rs:")));

    // A view reveals its element in Elements.
    harness.click_selector("entities-row-3");
    harness.assert_text_visible("It is drawn as a view, so each notify re-renders it");
    harness.screenshot("entities-view");
    harness.click_text("Reveal in Elements");
    assert_eq!(harness.state(|state| state.lens()), Lens::Elements);
    assert_eq!(
        harness.state(|state| state.selected_element()),
        Some(elements.issue_list)
    );

    harness.set_appearance(Appearance::Light);
    show(&mut harness, Lens::Entities);
    harness.screenshot("entities-light");
}

#[test]
fn live_entities_match_the_app_and_notify_forces_a_render() {
    let mut mail = Mail::open(1280., 800.);
    let store = mail.store.entity_id();
    let editor = mail.editor.entity_id();
    show(&mut mail.harness, Lens::Entities);

    // Ground truth from how the app was built: the workspace, the pane and
    // this test hold the store, the workspace observes it and the pane
    // subscribes to it.
    let info = mail.entity_info(store);
    assert_eq!(
        (
            info.strong_count,
            info.observers,
            info.subscribers,
            info.is_view
        ),
        (3, 1, 1, false)
    );
    assert!(mail.entity_info(editor).is_view);
    let harness = &mut mail.harness;
    harness.click_text("Store");
    assert_eq!(harness.state(|state| state.selected_entity()), Some(store));
    harness.assert_text_visible(
        "1 observer runs on each notify, 1 subscriber listens for its events and 3 strong \
         handles keep it alive.",
    );
    harness.assert_text_visible("Store hasn't notified since recording started");
    harness.screenshot("entities-live-store");

    // Notify the editor: the next frame is caused by it, and it re-renders.
    let editor_label = format!("entities-row-{}", editor.as_u64() & 0xffff_ffff);
    harness.click_selector(&editor_label);
    mail.harness.click_text("Notify");
    // The frame drawn right after the click is the app's, caused by the
    // notify, and it rendered the editor.
    let frame = mail
        .harness
        .capture(|capture| capture.latest_frame().unwrap().clone());
    assert!(!frame.inspector_only);
    assert!(
        frame.causes.iter().any(|cause| matches!(
            cause.kind,
            CauseKind::Notify { entity, .. } if entity == editor
        )),
        "{:?}",
        frame.causes
    );
    assert!(
        frame
            .rendered_views()
            .any(|view| view.entity == editor && view.outcome == ViewOutcome::Rendered)
    );
    mail.harness.advance(REFRESH_INTERVAL);
    mail.harness
        .assert_text_visible("Editor is notifying 0.5×/s");

    // And it reveals in Elements.
    mail.harness.click_text("Reveal in Elements");
    let revealed = mail
        .harness
        .state(|state| state.selected_element())
        .unwrap();
    let kind = mail.harness.capture(|capture| {
        let tree = capture.latest_tree().unwrap();
        tree.get(tree.find(revealed).unwrap()).unwrap().kind
    });
    assert!(matches!(kind, ElementKind::View { entity, .. } if entity == editor));
}

// ---------------------------------------------------------------------------
// Layout and scale.

#[test]
fn narrow_and_bottom_docks_lay_both_lenses_out_without_overflow() {
    let (mut narrow, _) = inbox_harness(900., 720.);
    narrow.update(|window, _| {
        window
            .inspector_capture_mut()
            .unwrap()
            .set_dock(InspectorDock::Right { width: px(400.) })
    });
    narrow.redraw_all();
    for (lens, shot) in [
        (Lens::Events, "events-narrow"),
        (Lens::Entities, "entities-narrow"),
    ] {
        show(&mut narrow, lens);
        narrow.screenshot(shot);
        assert_nothing_overflows(&mut narrow);
    }
    let seq = app_input(&mut narrow).last().unwrap().seq;
    show(&mut narrow, Lens::Events);
    narrow.click_selector(&format!("events-row-{seq}"));
    narrow.screenshot("events-narrow-detail");
    assert_nothing_overflows(&mut narrow);

    let (mut bottom, _) = inbox_harness(1280., 800.);
    bottom.update(|window, _| {
        window
            .inspector_capture_mut()
            .unwrap()
            .set_dock(InspectorDock::Bottom { height: px(360.) })
    });
    bottom.redraw_all();
    for (lens, shot) in [
        (Lens::Events, "events-bottom"),
        (Lens::Entities, "entities-bottom"),
    ] {
        show(&mut bottom, lens);
        bottom.screenshot(shot);
        assert_nothing_overflows(&mut bottom);
    }
    bottom.click_selector("entities-row-6");
    bottom.screenshot("entities-bottom-detail");
    assert_nothing_overflows(&mut bottom);
}

/// A capture with `records` input records and `entities` live entities
/// (every tenth notifying).
fn large_capture(records: usize, entities: u64) -> InspectorCapture {
    let (mut capture, _) = fixtures::inbox();
    for ix in 0..records {
        let at = ms(10_000. + ix as f64 * 7.);
        match ix % 3 {
            0 => InputBuilder::key(at, "j")
                .contexts(&["Workspace", "IssueList"])
                .action("inbox::SelectNext", true, Some("IssueList"))
                .push(&mut capture),
            1 => InputBuilder::click(at, gpui::point(px(400.), px(300.))).push(&mut capture),
            _ => InputBuilder::scroll(at, gpui::point(px(400.), px(300.)), -12.).push(&mut capture),
        };
    }
    let infos = (100..100 + entities)
        .map(|id| {
            let entity = EntityId::from(id);
            if id % 10 == 0 {
                capture.set_notify_stats_for_test(
                    entity,
                    NotifyStats {
                        total: id,
                        buckets: (0..120)
                            .map(|ix| ((ix + id) % 7) as u32)
                            .collect::<VecDeque<_>>(),
                        last_site: None,
                    },
                );
            }
            EntityInfo {
                id: entity,
                type_name: if id % 3 == 0 {
                    "app::Row"
                } else {
                    "app::Model"
                },
                strong_count: 1 + (id % 4) as usize,
                is_view: id % 3 == 0,
                observers: (id % 5) as usize,
                subscribers: (id % 2) as usize,
                notifies: if id % 10 == 0 { id } else { 0 },
                last_notify_site: None,
            }
        })
        .collect();
    capture.set_entities_for_test(infos);
    capture
}

#[test]
fn the_lenses_stay_fast_with_a_thousand_events_and_two_thousand_entities() {
    let mut harness = LoupeHarness::new(size(px(1280.), px(800.)), |_, cx| cx.new(|_| Backdrop));
    harness.open_loupe();
    harness.install_capture(large_capture(1_000, 2_000));
    assert_eq!(harness.capture(|capture| capture.input().len()), 1_000);

    let timed = |harness: &mut LoupeHarness, what: &str, f: &dyn Fn(&mut LoupeHarness)| {
        let start = Instant::now();
        f(harness);
        let took = start.elapsed();
        eprintln!("{what}: {took:?}");
        took
    };
    let budget = Duration::from_millis(1_500);
    assert!(timed(&mut harness, "open Events", &|h| show(h, Lens::Events)) < budget);
    assert!(
        timed(&mut harness, "filter 1,000 events", &|h| {
            h.click_text("Filter events · -exclude");
            h.type_text("select");
        }) < budget * 4
    );
    // Only the visible rows are drawn.
    let rows = harness
        .painted_text()
        .iter()
        .filter(|line| line.text.as_ref() == "SelectNext")
        .count();
    assert!((10..80).contains(&rows), "{rows} rows painted");
    harness.screenshot("events-thousand");

    assert!(timed(&mut harness, "open Entities", &|h| show(h, Lens::Entities)) < budget);
    harness.assert_text_visible("2,000 entities");
    assert!(
        timed(&mut harness, "sort 2,000 by type", &|h| h
            .click_text("Type"))
            < budget
    );
    assert!(
        timed(&mut harness, "new generation", &|h| {
            h.update(|window, _| {
                InputBuilder::key(ms(30_000.), "k").push(window.inspector_capture_mut().unwrap());
            });
            h.advance(REFRESH_INTERVAL);
        }) < budget
    );
    harness.screenshot("entities-two-thousand");
}
