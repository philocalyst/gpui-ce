//! Loupe over a live app with the real engine recording: what Loupe shows
//! comes from frames the window actually drew, and Loupe never keeps the
//! window busy on its own.

use gpui::{
    AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div,
    inspector::{CauseKind, ElementKind, InspectorCapture, ViewOutcome},
    px, rgb, size,
};
use gpui_inspector::harness::LoupeHarness;
use std::{cell::Cell, rc::Rc, time::Duration};

/// A counter whose button notifies on click, counting its own renders.
struct Counter {
    count: usize,
    renders: Rc<Cell<usize>>,
}

impl Render for Counter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size_full().bg(rgb(0xffffff)).p_4().child(
            div()
                .id("counter")
                .p_2()
                .bg(rgb(0x3366ff))
                .text_color(rgb(0xffffff))
                .child(format!("Count: {}", self.count))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.count += 1;
                    cx.notify();
                })),
        )
    }
}

fn frames(capture: &InspectorCapture) -> usize {
    capture.frames().len()
}

#[test]
fn loupe_reports_real_frames_and_never_keeps_the_window_busy() {
    let app_renders = Rc::new(Cell::new(0));
    let mut harness = LoupeHarness::new(size(px(1100.), px(700.)), {
        let renders = app_renders.clone();
        |_, cx| cx.new(|_| Counter { count: 0, renders })
    });
    harness.open_loupe();
    harness.advance(Duration::from_secs(1));
    let loupe = harness.loupe();
    let loupe_renders =
        |harness: &mut LoupeHarness| harness.app(|cx| loupe.read(cx).render_count());

    // Settled: a quiet app draws nothing, and Loupe does not re-render.
    let settled_frames = harness.capture(frames);
    let settled_loupe = loupe_renders(&mut harness);
    let settled_app = app_renders.get();
    harness.advance(Duration::from_secs(2));
    assert_eq!(harness.capture(frames), settled_frames, "idle window drew");
    assert_eq!(loupe_renders(&mut harness), settled_loupe);
    assert_eq!(app_renders.get(), settled_app);

    // The recorded tree is the app's, with the counter's view and button.
    let tree = harness.capture(|capture| capture.latest_tree().cloned().expect("a tree"));
    assert!(tree.elements.iter().any(|record| matches!(
        record.kind,
        ElementKind::View { type_name, .. } if type_name.ends_with("Counter")
    )));
    assert!(
        tree.elements
            .iter()
            .any(|record| record.id.as_deref() == Some("counter"))
    );
    assert!(
        tree.elements
            .iter()
            .all(|record| !record.kind.type_name().contains("gpui_inspector")),
        "Loupe's own elements are never recorded"
    );

    // A click re-renders the app (on press and on release) and wakes Loupe
    // exactly once. Every app render Loupe reports really happened, and the
    // frame Loupe's own refresh drew replayed the app instead of rendering it.
    harness.click_text("Count: 0");
    harness.advance(Duration::from_millis(250));
    harness.assert_text_visible("Count: 1");
    assert_eq!(loupe_renders(&mut harness), settled_loupe + 1);
    let recorded_renders = harness.capture(|capture| {
        capture
            .frames()
            .iter()
            .skip(settled_frames)
            .flat_map(|frame| &frame.views)
            .filter(|view| {
                view.type_name.ends_with("Counter") && view.outcome == ViewOutcome::Rendered
            })
            .count()
    });
    assert!(recorded_renders >= 1);
    assert_eq!(app_renders.get(), settled_app + recorded_renders);
    harness.capture(|capture| {
        for frame in capture.frames().iter().filter(|frame| frame.inspector_only) {
            assert!(
                frame
                    .views
                    .iter()
                    .filter(|view| view.type_name.ends_with("Counter"))
                    .all(|view| view.outcome == ViewOutcome::Cached),
                "frame {} drawn only for Loupe re-rendered the app",
                frame.id
            );
        }
    });
    let app_renders_after_click = app_renders.get();

    let click_frame = harness.capture(|capture| capture.latest_app_frame().cloned().unwrap());
    let notify = click_frame
        .causes
        .iter()
        .find(|cause| matches!(cause.kind, CauseKind::Notify { .. }))
        .expect("the click's notify caused the frame");
    assert!(notify.site.unwrap().file().ends_with("live.rs"));
    assert!(!notify.from_inspector);
    assert!(click_frame.views.iter().any(|view| {
        view.type_name.ends_with("Counter") && view.outcome == ViewOutcome::Rendered
    }));
    assert!(click_frame.timings.total > Duration::ZERO);
    assert!(
        !click_frame.input.is_empty(),
        "the click is linked to its frame"
    );

    // Loupe's own refresh drew a frame that is flagged as the inspector's.
    let latest = harness.capture(|capture| capture.latest_frame().cloned().unwrap());
    assert!(latest.id > click_frame.id);
    assert!(latest.inspector_only);

    // And then everything is quiet again: no feedback loop.
    let frames_after = harness.capture(frames);
    harness.advance(Duration::from_secs(3));
    assert_eq!(harness.capture(frames), frames_after);
    assert_eq!(loupe_renders(&mut harness), settled_loupe + 1);
    assert_eq!(app_renders.get(), app_renders_after_click);
    harness.screenshot("live-counter");
}

#[test]
fn holding_keeps_the_app_on_screen_until_release() {
    let app_renders = Rc::new(Cell::new(0));
    let mut harness = LoupeHarness::new(size(px(1100.), px(700.)), {
        let renders = app_renders.clone();
        |_, cx| cx.new(|_| Counter { count: 0, renders })
    });
    harness.open_loupe();
    harness.advance(Duration::from_secs(1));

    // Hold from the toolbar: the app still handles the click, but what is on
    // screen stays exactly as it was.
    harness.click_selector("loupe-hold");
    assert!(harness.capture(|capture| capture.is_holding()));
    harness.assert_text_visible("HELD");
    let held_renders = app_renders.get();
    harness.click_text("Count: 0");
    harness.advance(Duration::from_millis(250));
    harness.assert_text_visible("Count: 0");
    assert_eq!(app_renders.get(), held_renders, "a held app never renders");
    harness.screenshot("live-held");

    // Releasing draws the deferred change at once.
    harness.click_selector("loupe-hold");
    harness.draw();
    assert!(!harness.capture(|capture| capture.is_holding()));
    harness.assert_text_visible("Count: 1");
    assert!(app_renders.get() > held_renders);

    // The global shortcut holds too, even with the pointer over the app.
    harness.type_keys(if cfg!(target_os = "macos") {
        "cmd-shift-h"
    } else {
        "ctrl-shift-h"
    });
    assert!(harness.capture(|capture| capture.is_holding()));
}
