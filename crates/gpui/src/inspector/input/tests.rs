use super::describe;
use crate::{
    self as gpui, Bounds, Context, Entity, FileDropEvent, FocusHandle, InteractiveElement as _,
    IntoElement, KeyBinding, KeyDownEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement as _, PlatformInput, Render, ScrollDelta, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, VisualTestContext, Window, div,
    inspector::{
        CaptureLevel, ElementFlags, ElementKey, ElementKind, ElementRecord, ElementTree,
        FrameRecord, InputKind, InputRecord, InspectorCapture, PhaseTimings, SceneStats,
    },
    point, px, size,
};
use std::{cell::RefCell, ops::Range, rc::Rc, sync::Arc, time::Duration};

actions!(inspector_input_test, [Save, Close, Unhandled]);

/// Window size for every test: the default right dock (560 px) leaves the app
/// the left 440 px.
const WINDOW: gpui::Size<gpui::Pixels> = size(px(1000.), px(600.));

/// A 40 × 40 button at the top-left of a toolbar inside a workspace.
struct Workspace {
    toolbar: FocusHandle,
    saves: usize,
    closes: usize,
}

impl Workspace {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            toolbar: cx.focus_handle(),
            saves: 0,
            closes: 0,
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = self.toolbar.clone();
        div()
            .id("workspace")
            .key_context("Workspace")
            .size_full()
            .on_action(cx.listener(|this, _: &Save, _, _| this.saves += 1))
            .child(
                div()
                    .id("toolbar")
                    .key_context("Toolbar")
                    .track_focus(&self.toolbar)
                    .size_full()
                    .on_action(cx.listener(|this, _: &Close, _, _| this.closes += 1))
                    .child(
                        div()
                            .id("button")
                            .size(px(40.))
                            .on_click(move |_, window, cx| {
                                toolbar.dispatch_action(&Save, window, cx);
                                cx.stop_propagation();
                            }),
                    ),
            )
    }
}

/// Opens a [`Workspace`] with the inspector open and the toolbar focused.
fn open_workspace(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    let window = cx.open_window(WINDOW, |_, cx| Workspace::new(cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.update(|window, cx| {
        window.toggle_inspector(cx);
        workspace.read(cx).toolbar.clone().focus(window, cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn input(cx: &mut VisualTestContext) -> Vec<InputRecord> {
    cx.update(|window, _| {
        window
            .inspector_capture()
            .map(|capture| capture.input().iter().cloned().collect())
            .unwrap_or_default()
    })
}

fn capture<R>(cx: &mut VisualTestContext, f: impl FnOnce(&mut InspectorCapture) -> R) -> R {
    cx.update(|window, _| f(window.inspector_capture_mut().unwrap()))
}

fn move_to(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::none());
}

fn frame(input: Range<u64>, tree: Option<ElementTree>) -> FrameRecord {
    FrameRecord {
        id: 0,
        start: Duration::ZERO,
        viewport: WINDOW,
        timings: PhaseTimings::default(),
        causes: Vec::new(),
        views: Vec::new(),
        spans: Vec::new(),
        scene: SceneStats::default(),
        element_count: tree.as_ref().map_or(0, |tree| tree.elements.len() as u32),
        tree: tree.map(Arc::new),
        input,
        foreground: Vec::new(),
        inspector_only: false,
    }
}

/// Records a frame whose tree mirrors [`Workspace`]'s layout, returning the
/// keys of the workspace, toolbar and button. Later frames keep no trees, so
/// this stays the latest captured tree.
fn push_workspace_tree(cx: &mut VisualTestContext) -> [ElementKey; 3] {
    capture(cx, |capture| {
        capture.config_mut().level = CaptureLevel::Frames;
        let app = Bounds::new(point(px(0.), px(0.)), size(px(440.), px(600.)));
        let button = Bounds::new(point(px(0.), px(0.)), size(px(40.), px(40.)));
        let keys = ["workspace", "toolbar", "button"].map(|scope| ElementKey {
            path: capture.intern_path_for_test(scope),
            instance: 0,
        });
        let mut tree = ElementTree::default();
        for (ix, (key, bounds)) in keys.iter().zip([app, app, button]).enumerate() {
            tree.elements.push(ElementRecord {
                key: Some(*key),
                parent: ix.checked_sub(1).map(|parent| parent as u32),
                depth: ix as u16,
                kind: ElementKind::Element {
                    type_name: "gpui::Div",
                },
                id: None,
                bounds,
                visible_bounds: Some(bounds),
                paint_order: ix as u32,
                primitives: 0,
                flags: ElementFlags::empty(),
                details: None,
            });
        }
        tree.rebuild_children();
        capture.push_frame_for_test(frame(0..0, Some(tree)));
        keys
    })
}

#[gpui::test]
fn click_on_nested_button_records_hit_path_action_and_handled(cx: &mut TestAppContext) {
    let (workspace, cx) = open_workspace(cx);
    let [workspace_key, toolbar_key, button_key] = push_workspace_tree(cx);

    cx.simulate_click(point(px(20.), px(20.)), Modifiers::none());

    let records = input(cx);
    let [down, up] = records.as_slice() else {
        panic!("expected a mouse down and a mouse up, got {records:#?}");
    };
    assert_eq!(down.kind, InputKind::MouseDown);
    assert_eq!(down.detail.as_ref(), "left ×1 at (20, 20)");
    assert_eq!(down.position, Some(point(px(20.), px(20.))));
    assert_eq!(
        down.hit_path.as_slice(),
        [button_key, toolbar_key, workspace_key]
    );
    assert!(down.actions.is_empty());
    assert!(!down.inspector);

    assert_eq!(up.kind, InputKind::MouseUp);
    assert_eq!(up.hit_path, down.hit_path);
    let [save] = up.actions.as_slice() else {
        panic!("expected the click to dispatch Save, got {:#?}", up.actions);
    };
    assert_eq!(save.name, "inspector_input_test::Save");
    assert!(save.handled);
    assert_eq!(save.keystrokes, None);
    assert_eq!(save.context, None);
    assert!(up.handled);
    assert!(up.seq > down.seq);
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.saves), 1);
}

#[gpui::test]
fn key_bound_in_context_records_keystrokes_context_and_action(cx: &mut TestAppContext) {
    let (workspace, cx) = open_workspace(cx);
    cx.update(|_, cx| {
        cx.bind_keys([KeyBinding::new(
            "ctrl-s",
            Save,
            Some("Workspace > Toolbar && !Modal"),
        )])
    });

    cx.simulate_keystrokes("ctrl-s");

    let records = input(cx);
    let [key] = records.as_slice() else {
        panic!("expected one key down, got {records:#?}");
    };
    assert_eq!(key.kind, InputKind::KeyDown);
    assert_eq!(key.detail.as_ref(), "ctrl-s");
    assert_eq!(key.keystroke, Some(Keystroke::parse("ctrl-s").unwrap()));
    assert_eq!(key.position, None);
    assert!(key.hit_path.is_empty());
    let contexts: Vec<String> = key
        .context_stack
        .iter()
        .map(|context| format!("{context:?}"))
        .collect();
    assert_eq!(contexts, ["Workspace", "Toolbar"]);
    let [save] = key.actions.as_slice() else {
        panic!("expected Save, got {:#?}", key.actions);
    };
    assert_eq!(save.name, "inspector_input_test::Save");
    assert!(save.handled);
    assert_eq!(save.keystrokes.as_deref(), Some("ctrl-s"));
    assert_eq!(
        save.context.as_deref(),
        Some("Workspace > Toolbar && !Modal")
    );
    assert!(key.handled);
    assert!(!key.inspector);
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.saves), 1);
}

#[gpui::test]
fn unbound_key_records_no_action_and_is_not_handled(cx: &mut TestAppContext) {
    let (_, cx) = open_workspace(cx);

    cx.simulate_keystrokes("x y");

    let records = input(cx);
    let [first, second] = records.as_slice() else {
        panic!("expected two key downs, got {records:#?}");
    };
    for key in [first, second] {
        assert_eq!(key.kind, InputKind::KeyDown);
        assert!(key.actions.is_empty());
        assert!(!key.handled);
    }
    assert_eq!(second.detail.as_ref(), "y");
    assert!(
        first.caused_redraw,
        "switching to keyboard modality redraws focus-visible styles"
    );
    assert!(!second.caused_redraw);
}

#[gpui::test]
fn multi_stroke_binding_records_pending_then_the_action(cx: &mut TestAppContext) {
    let (workspace, cx) = open_workspace(cx);
    cx.update(|_, cx| cx.bind_keys([KeyBinding::new("ctrl-k ctrl-w", Close, Some("Toolbar"))]));

    cx.simulate_keystrokes("ctrl-k ctrl-w");

    let records = input(cx);
    let [first, second] = records.as_slice() else {
        panic!("expected two key downs, got {records:#?}");
    };
    assert_eq!(first.detail.as_ref(), "ctrl-k (pending)");
    assert!(first.handled, "the keymap holds a pending prefix");
    assert!(first.actions.is_empty());
    assert_eq!(second.detail.as_ref(), "ctrl-w");
    let [close] = second.actions.as_slice() else {
        panic!("expected Close, got {:#?}", second.actions);
    };
    assert_eq!(close.name, "inspector_input_test::Close");
    assert_eq!(close.keystrokes.as_deref(), Some("ctrl-k ctrl-w"));
    assert_eq!(close.context.as_deref(), Some("Toolbar"));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.closes), 1);
}

#[gpui::test]
fn pending_prefix_flushed_by_timeout_records_an_action(cx: &mut TestAppContext) {
    let (workspace, cx) = open_workspace(cx);
    cx.update(|_, cx| {
        cx.bind_keys([
            KeyBinding::new("ctrl-k", Save, None),
            KeyBinding::new("ctrl-k ctrl-w", Close, None),
        ])
    });

    cx.simulate_keystrokes("ctrl-k");
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();

    let records = input(cx);
    let [key, flushed] = records.as_slice() else {
        panic!("expected a key down and a flushed action, got {records:#?}");
    };
    assert_eq!(key.detail.as_ref(), "ctrl-k (pending)");
    assert!(key.actions.is_empty());
    assert_eq!(flushed.kind, InputKind::Action);
    assert_eq!(flushed.detail.as_ref(), "inspector_input_test::Save");
    let [save] = flushed.actions.as_slice() else {
        panic!("expected Save, got {:#?}", flushed.actions);
    };
    assert_eq!(save.keystrokes.as_deref(), Some("ctrl-k"));
    assert!(flushed.handled);
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.saves), 1);
}

#[gpui::test]
fn directly_dispatched_action_gets_its_own_record(cx: &mut TestAppContext) {
    let (workspace, cx) = open_workspace(cx);

    cx.dispatch_action(Save);
    cx.dispatch_action(Unhandled);

    let records = input(cx);
    let [save, unhandled] = records.as_slice() else {
        panic!("expected two action records, got {records:#?}");
    };
    assert_eq!(save.kind, InputKind::Action);
    assert_eq!(save.detail.as_ref(), "inspector_input_test::Save");
    assert!(save.handled);
    assert_eq!(save.actions.len(), 1);
    assert_eq!(save.actions[0].keystrokes, None);
    assert_eq!(
        save.context_stack.len(),
        2,
        "dispatched to the focused toolbar"
    );
    assert!(!unhandled.handled);
    assert!(!unhandled.actions[0].handled);
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.saves), 1);
}

#[gpui::test]
fn consecutive_mouse_moves_coalesce_until_a_frame_is_drawn(cx: &mut TestAppContext) {
    let (_, cx) = open_workspace(cx);

    for x in [10., 20., 30., 40., 50.] {
        move_to(cx, x, 100.);
    }

    let records = input(cx);
    let [moves] = records.as_slice() else {
        panic!("expected one coalesced record, got {records:#?}");
    };
    assert_eq!(moves.kind, InputKind::MouseMove);
    assert_eq!(moves.coalesced, 4);
    assert_eq!(moves.position, Some(point(px(50.), px(100.))));
    assert_eq!(moves.detail.as_ref(), "to (50, 100)");

    let frame_id = capture(cx, |capture| {
        let next_seq = capture.input().back().unwrap().seq + 1;
        capture.push_frame_for_test(frame(0..next_seq, None))
    });
    move_to(cx, 60., 100.);
    move_to(cx, 70., 100.);

    let records = input(cx);
    let [before, after] = records.as_slice() else {
        panic!("expected the frame to split the moves, got {records:#?}");
    };
    assert_eq!(before.frame, Some(frame_id));
    assert_eq!(before.coalesced, 4);
    assert_eq!(after.frame, None);
    assert_eq!(after.coalesced, 1);
    assert_eq!(after.position, Some(point(px(70.), px(100.))));
}

#[gpui::test]
fn frame_claims_the_input_in_its_range(cx: &mut TestAppContext) {
    let (_, cx) = open_workspace(cx);
    // The first key switches to keyboard modality, which redraws.
    cx.simulate_keystrokes("a");
    cx.simulate_keystrokes("b c");

    let (frame_id, records) = capture(cx, |capture| {
        let b = capture.input()[1].seq;
        let frame_id = capture.push_frame_for_test(frame(b..b + 1, None));
        (frame_id, capture.input().clone())
    });

    assert_eq!(records[1].detail.as_ref(), "b");
    assert_eq!(records[1].frame, Some(frame_id));
    assert_eq!(records[2].frame, None, "after the frame's input range");
}

#[gpui::test]
fn scroll_and_modifier_changes_are_described(cx: &mut TestAppContext) {
    let (_, cx) = open_workspace(cx);

    cx.simulate_event(ScrollWheelEvent {
        position: point(px(100.), px(200.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-36.))),
        ..Default::default()
    });
    cx.simulate_modifiers_change(Modifiers {
        control: true,
        shift: true,
        ..Modifiers::none()
    });
    cx.simulate_modifiers_change(Modifiers::none());

    let records = input(cx);
    let [scroll, pressed, released] = records.as_slice() else {
        panic!("expected a scroll and two modifier changes, got {records:#?}");
    };
    assert_eq!(scroll.kind, InputKind::Scroll);
    assert_eq!(scroll.detail.as_ref(), "scroll Δ(0, -36)");
    assert_eq!(scroll.position, Some(point(px(100.), px(200.))));
    assert_eq!(pressed.kind, InputKind::Modifiers);
    assert_eq!(pressed.detail.as_ref(), "ctrl+shift");
    assert_eq!(pressed.context_stack.len(), 2);
    assert_eq!(released.detail.as_ref(), "none");
}

#[gpui::test]
fn pointer_input_inside_the_dock_or_while_picking_belongs_to_the_inspector(
    cx: &mut TestAppContext,
) {
    let (_, cx) = open_workspace(cx);
    let generation = capture(cx, |capture| capture.generation());

    move_to(cx, 900., 300.);
    let after_dock_move = capture(cx, |capture| capture.generation());
    move_to(cx, 100., 300.);
    cx.update(|window, _| window.start_inspector_pick());
    cx.simulate_event(MouseDownEvent {
        position: point(px(100.), px(300.)),
        button: MouseButton::Left,
        click_count: 1,
        ..Default::default()
    });

    let records = input(cx);
    let [dock, app, picked] = records.as_slice() else {
        panic!("expected moves on each side of the dock and a pick, got {records:#?}");
    };
    assert!(dock.inspector);
    assert_eq!(dock.coalesced, 0);
    assert!(!app.inspector, "moves out of the dock start a new record");
    assert!(picked.inspector);
    assert_eq!(
        after_dock_move, generation,
        "input to the inspector doesn't wake it"
    );
}

#[gpui::test]
fn key_input_focused_inside_the_inspector_belongs_to_the_inspector(cx: &mut TestAppContext) {
    let inspector_focus = Rc::new(RefCell::new(None::<FocusHandle>));
    cx.update(|cx| {
        let inspector_focus = inspector_focus.clone();
        cx.set_inspector_renderer(Box::new(move |inspector, _, cx| {
            let focus = inspector.ui_state(|| cx.focus_handle()).clone();
            inspector_focus.replace(Some(focus.clone()));
            div()
                .key_context("Loupe")
                .track_focus(&focus)
                .size_full()
                .into_any_element()
        }));
    });
    let (_, cx) = open_workspace(cx);
    let focus = inspector_focus.borrow().clone().unwrap();
    cx.update(|window, cx| focus.focus(window, cx));
    cx.run_until_parked();

    cx.simulate_keystrokes("j");

    let records = input(cx);
    let [key] = records.as_slice() else {
        panic!("expected one key down, got {records:#?}");
    };
    assert!(key.inspector);
    let contexts: Vec<String> = key
        .context_stack
        .iter()
        .map(|context| format!("{context:?}"))
        .collect();
    assert_eq!(contexts, ["Loupe"]);
}

#[gpui::test]
fn frozen_capture_records_nothing(cx: &mut TestAppContext) {
    let (workspace, cx) = open_workspace(cx);
    capture(cx, |capture| capture.set_frozen(true));

    move_to(cx, 10., 10.);
    cx.simulate_keystrokes("x");
    cx.dispatch_action(Save);

    assert!(input(cx).is_empty());
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.saves), 1);
}

#[gpui::test]
fn closed_inspector_records_nothing(cx: &mut TestAppContext) {
    let window = cx.open_window(WINDOW, |_, cx| Workspace::new(cx));
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();

    move_to(cx, 10., 10.);
    cx.simulate_keystrokes("x");
    cx.dispatch_action(Save);

    cx.update(|window, _| assert!(window.inspector_capture().is_none()));

    // Opening the inspector afterwards starts from an empty recording.
    cx.update(|window, cx| window.toggle_inspector(cx));
    assert!(input(cx).is_empty());
}

#[gpui::test]
fn input_ring_keeps_the_most_recent_records(cx: &mut TestAppContext) {
    let (_, cx) = open_workspace(cx);
    capture(cx, |capture| capture.config_mut().input_capacity = 3);

    cx.simulate_keystrokes("a b c d e");

    let records = input(cx);
    let details: Vec<&str> = records
        .iter()
        .map(|record| record.detail.as_ref())
        .collect();
    assert_eq!(details, ["c", "d", "e"]);
    let seqs: Vec<u64> = records.iter().map(|record| record.seq).collect();
    assert_eq!(seqs, [2, 3, 4]);
}

#[test]
fn describe_summarizes_events() {
    let detail = |event: PlatformInput| describe(&event).1;
    assert_eq!(
        detail(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Right,
            position: point(px(120.), px(44.5)),
            modifiers: Modifiers::shift(),
            click_count: 2,
            first_mouse: false,
        })),
        "shift-right ×2 at (120, 44.5)"
    );
    assert_eq!(
        detail(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(3.), px(4.)),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::none(),
        })),
        "left drag to (3, 4)"
    );
    assert_eq!(
        detail(PlatformInput::ScrollWheel(ScrollWheelEvent {
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        })),
        "scroll Δ(0, -3) lines"
    );
    assert_eq!(
        detail(PlatformInput::KeyDown(KeyDownEvent {
            keystroke: Keystroke::parse("ctrl-shift-k").unwrap(),
            is_held: true,
            prefer_character_input: false,
        })),
        "ctrl-shift-k (held)"
    );
    assert_eq!(
        detail(PlatformInput::FileDrop(FileDropEvent::Submit {
            position: point(px(1.5), px(2.)),
        })),
        "files dropped at (1.5, 2)"
    );
}
