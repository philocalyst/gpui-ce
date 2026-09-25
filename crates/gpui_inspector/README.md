# Loupe

Loupe is the inspector and profiler for gpui apps. It docks inside the
window it inspects and answers *why*:

* Why is this element this size, and where in my code was it built?
* Why was that frame drawn, and why was it slow?
* Where did my click go? What will this key do here?
* What's alive, who's watching it, who keeps poking it?
* What should I fix first?

Every answer links to a `file:line` and to the live element.

## Enable it

Add the crate (its package is `gpui_ce_inspector`, its library
`gpui_inspector`) and register it once at startup:

```toml
[dependencies]
gpui_inspector = { version = "0.1", package = "gpui_ce_inspector" }
```

```rust
gpui_platform::application().run(|cx: &mut gpui::App| {
    gpui_inspector::init(cx);
    // Open windows as usual.
});
```

Then press `ctrl-shift-i` (`cmd-alt-i` on macOS) in any window. The crate
turns on gpui's `inspector` feature, which compiles the engine's capture in;
it is also compiled into every debug build. To ship release builds without
it, put Loupe behind a feature of your app:

```toml
[features]
inspector = ["dep:gpui_inspector"]
```

```rust
#[cfg(feature = "inspector")]
gpui_inspector::init(cx);
```

Loupe's own features:

| Feature | Adds |
|---|---|
| `profiler` | The profiler journal's main-thread tasks in the Frames flame chart. |
| `test-support` | The headless harness and fixtures (see [Testing](#testing)). |

### Settings

The gear in the toolbar (or `ctrl-,`, or the palette) sets the theme
(follow the window, dark, light), the density (compact, comfortable), the
frame budget (60, 120 or 144 Hz), the capture level (below) and the editor
that source links open in: Zed, VS Code, Cursor, IntelliJ, or any URL
template with `{path}`, `{line}` and `{col}`. The popover previews the URL.

Settings are one app-wide `LoupeSettings` and last until the app quits.
Choose the defaults when you register Loupe:

```rust
use gpui_inspector::{Editor, FrameBudget, LoupeSettings};

gpui_inspector::init_with(
    cx,
    LoupeSettings {
        editor: Editor::VsCode.into(),
        budget: FrameBudget::Hz120,
        ..LoupeSettings::default()
    },
);
```

## Keys

Press `?` in Loupe for the full list. It is generated from the keymap, so it
also shows keys your app rebinds.

| Key | Does |
|---|---|
| `ctrl-shift-i` / `cmd-alt-i` | Open or close Loupe (anywhere) |
| `ctrl-shift-c` / `cmd-shift-c` | Pick an element in the app (anywhere) |
| `ctrl-shift-h` / `cmd-shift-h` | Hold the app still, or release it (anywhere) |
| `ctrl-k` / `cmd-k` | Find anything: commands, `#elements`, `@entities` |
| `alt-1` … `alt-5` | Show Elements, Frames, Events, Entities, Audit |
| `space` | Freeze or resume recording |
| `ctrl-,` / `cmd-,` | Settings |
| `?` | Keyboard shortcuts |
| `escape` | Close the open panel, or stop picking |
| `j` `k` `↑` `↓` `←` `→` `enter` | Move, collapse, expand and open in lists and trees |
| `]` `[` or the wheel | While picking: the enclosing element, and back |

## The lenses

The rail under the pulse strip switches lenses; each tab carries a live
count.

* **Elements.** The element tree of any recorded frame, with a filter and
  *Your code* (hides gpui and library elements). The detail explains the
  size in a sentence per axis, draws the box model, and lists the style as
  editable rows: scrub a number, pick a color, force `:hover`, then *Copy
  Rust* to take the change back to your code. The source link opens your
  editor at the line.
* **Frames.** fps and percentiles, the phase bar, why the frame was drawn
  (with the call site that caused it), a flame chart of view renders and your
  own spans, a bottom-up table and plain-language insights. *Jump to worst*
  finds the slowest frame.
* **Events.** Every input event with the elements it hit, the key contexts,
  the actions it ran and the frame it caused. The *Key tester* resolves a
  keystroke against the app's current focus and says which binding wins and
  why the others lose.
* **Entities.** Every live entity with its observers and how often it
  notifies, and the line that notified it last.
* **Audit.** Continuous checks: clickables keyboards cannot reach, low
  contrast, missing accessible names, zero-size hitboxes, render hot spots,
  overflowing content, duplicate ids. Worst first, each linked to its
  elements.

The pulse strip on top shows one bar per frame, graded against the budget.
Click a bar to open that frame in Frames; Elements then shows that frame's
tree.

## Overlays, pick, hold and freeze

The toolbar toggles overlays drawn over the app: element outlines, paint
flashing for re-rendered views, hitboxes, a border on slow frames, overflow
stripes and the box model of the hovered and selected element.

* **Pick** (`ctrl-shift-c`) turns the next click in the app into a
  selection. Move to hover, `]` / `[` or the wheel to walk up and down the
  ancestry, click to select, `escape` to stop.
* **Hold** (`ctrl-shift-h`) keeps the app still: frames are replayed, so a
  hover menu or tooltip stays open while you pick it. *Hold the app in 3
  seconds* (palette) gives you time to open the menu first.
* **Freeze** (`space`) stops recording, so the last 240 frames and 1000
  events stay put while you read them. Overlays and picking still work.

## Traces and your own spans

*Export trace…* in Frames saves the whole recording as Chrome trace JSON
(*Copy trace* puts it on the clipboard). Open it in
[ui.perfetto.dev](https://ui.perfetto.dev) or `chrome://tracing`. Set an
`ExportDirectory` global to save without a file dialog.

Mark your own work so it shows in the flame chart next to the view that ran
it:

```rust
fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let rows = gpui::inspector_span!("filter rows", self.filtered_rows());
    let _build = gpui::inspector::span("build list"); // until the end of the block
    div().children(rows.iter().map(|row| self.render_row(row)))
}
```

A span costs one thread-local check while Loupe is closed.

## What it costs

* **Closed:** nothing is recorded and nothing is allocated. Every engine hook
  is one `Option` check (a test proves pointer dispatch allocates nothing).
* **Open:** Loupe records the last 240 frames and 1000 input events. Its own
  drawing is measured, shown in the status bar (`loupe` ms) and in Audit
  (*Overhead*), and left out of the app's numbers. The capture level trades
  detail for cost:

  | Level | Records | Cost |
  |---|---|---|
  | Frames | Timings, causes, view renders | Cheapest |
  | Tree | Plus element trees (picking, layout checks) | More per frame |
  | Full (default) | Plus styles, text and accessibility details | The most |

  Measure it on your machine:
  `cargo test -p gpui-ce --features test-support --lib capture_overhead -- --ignored --nocapture`.

## Try it

```sh
cargo run -p gpui_ce_inspector --example loupe_demo            # opens with Loupe docked
cargo run -p gpui_ce_inspector --example loupe_demo -- --closed
```

The demo is a small issue tracker with problems left in on purpose: pick the
`•••` button and read why Audit flags it, click *Simulate jank* and open the
red bar in the pulse strip, type `g i` and find it in Events.

## Testing

`test-support` adds a headless harness that renders real text and pixels
(wgpu, Mesa lavapipe without a GPU) and drives Loupe with real input:

```rust
use gpui_inspector::{fixtures, harness::LoupeHarness};

let mut harness = LoupeHarness::new(size(px(1280.), px(800.)), |_, cx| cx.new(|_| MyApp));
harness.open_loupe();
harness.install_capture(fixtures::inbox().0); // or leave the live recording
harness.click_text("Frames");
harness.type_keys("secondary-k");
harness.assert_text_visible("Find commands, #elements, @entities…");
harness.screenshot("palette"); // target/loupe-shots/palette.png ($LOUPE_SHOTS)
```

`fixtures` builds deterministic recordings (`inbox()`: a small issue tracker
with 240 frames and its input; `steady_frames()`). Run the suite with
`cargo test -p gpui_ce_inspector --features test-support`, then look at the
screenshots. `DESIGN.md` has the architecture and the testing standard;
Lightbox (`crates/gpui_lightbox`) adds contact sheets, goldens and style lint.
