//! Loupe over Linework Inbox (the `loupe_demo` example) with the real engine:
//! every lens shows real data, and the problems left in the demo on purpose
//! are found.

#[path = "../examples/loupe_demo/inbox.rs"]
mod inbox;

use gpui::{AppContext as _, point, px, size};
use gpui_inspector::{Lens, harness::LoupeHarness};
use inbox::{InboxApp, InboxOptions};
use std::time::Duration;

fn demo() -> LoupeHarness {
    demo_with(InboxOptions { live_sync: false })
}

fn demo_with(options: InboxOptions) -> LoupeHarness {
    let mut harness = LoupeHarness::new(size(px(1480.), px(900.)), move |window, cx| {
        inbox::init(cx);
        cx.new(|cx| InboxApp::new(options, window, cx))
    });
    harness.open_loupe();
    harness.advance(Duration::from_secs(1));
    harness
}

#[test]
fn the_app_draws_after_a_frame_loupe_drew_for_itself() {
    let mut harness = demo();
    // The store re-renders the cached detail view (and a slow list)...
    harness.click_text("Simulate jank");
    harness.advance(Duration::from_millis(100));
    // ...then Loupe draws alone (the app is replayed)...
    harness.hover(point(px(1300.), px(500.)));
    harness.advance(Duration::from_millis(100));
    // ...and the app draws again, reusing the detail view.
    harness.hover(point(px(400.), px(300.)));
    harness.advance(Duration::from_millis(100));
    harness.assert_text_visible("Jank on");
    harness.assert_text_visible("Scroll jank in the issue list");
}

#[test]
fn loupe_keeps_up_with_an_app_that_draws_every_frame() {
    let mut harness = demo_with(InboxOptions { live_sync: true });
    let loupe = harness.loupe();
    let renders = |harness: &mut LoupeHarness| harness.app(|cx| loupe.read(cx).render_count());
    let before = renders(&mut harness);
    for _ in 0..40 {
        harness.advance(Duration::from_millis(25));
    }
    assert!(
        renders(&mut harness) > before,
        "Loupe follows the app's new frames"
    );
    harness.update_state(|state, cx| state.set_lens(Lens::Entities, cx));
    harness.advance(Duration::from_millis(25));
    harness.assert_text_visible("SyncClient");
}

#[test]
fn the_demo_renders_with_loupe_docked() {
    let mut harness = demo();
    for text in [
        "Linework",
        "Inbox",
        "Scroll jank in the issue list",
        "Close issue",
    ] {
        harness.assert_text_visible(text);
    }
    harness.screenshot("demo-overview");
}
