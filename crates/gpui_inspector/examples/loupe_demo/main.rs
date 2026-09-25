//! Loupe over Linework Inbox, a small issue tracker built to be inspected.
//!
//! ```sh
//! cargo run -p gpui_ce_inspector --example loupe_demo            # Loupe open
//! cargo run -p gpui_ce_inspector --example loupe_demo -- --closed # press ctrl-shift-i / cmd-alt-i
//! ```
//!
//! Things to try: pick the ••• button (`ctrl-shift-c` / `cmd-shift-c`) and read
//! why it is flagged in Audit; click "Simulate jank" and open the red bar in
//! the pulse strip; type `g i` in the app and see it in Events; hover the
//! star, hold the app (`ctrl-shift-h` / `cmd-shift-h`) and inspect the tooltip.

mod inbox;

use gpui::{App, AppContext as _, Bounds, Focusable as _, WindowBounds, WindowOptions, px, size};
use inbox::{InboxApp, InboxOptions};

fn main() {
    let open_loupe = !std::env::args().any(|arg| arg == "--closed");
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_inspector::init(cx);
        inbox::init(cx);
        let bounds = Bounds::centered(None, size(px(1480.), px(900.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| InboxApp::new(InboxOptions::default(), window, cx)),
            )
            .expect("the demo window opens");
        window
            .update(cx, |app, window, cx| {
                window.focus(&app.focus_handle(cx), cx);
                if open_loupe {
                    window.toggle_inspector(cx);
                }
            })
            .expect("the demo window is open");
        cx.activate(true);
    });
}
