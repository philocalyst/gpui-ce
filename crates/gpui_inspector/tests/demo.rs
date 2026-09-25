//! Loupe over Linework Inbox (the `loupe_demo` example) with the real engine:
//! every lens shows real data, and the problems left in the demo on purpose
//! are found.

#[path = "../examples/loupe_demo/inbox.rs"]
mod inbox;

use gpui::{AppContext as _, px, size};
use gpui_inspector::harness::LoupeHarness;
use inbox::{InboxApp, InboxOptions};
use std::time::Duration;

fn demo() -> LoupeHarness {
    let mut harness = LoupeHarness::new(size(px(1480.), px(900.)), |window, cx| {
        inbox::init(cx);
        cx.new(|cx| InboxApp::new(InboxOptions { live_sync: false }, window, cx))
    });
    harness.open_loupe();
    harness.advance(Duration::from_secs(1));
    harness
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
