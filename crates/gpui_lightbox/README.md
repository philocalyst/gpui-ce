# Lightbox

Visual testing for gpui: screenshots, variant matrices, filmstrips of
animations, style lint, golden images and UI benchmarks — and one report to
review them all.

Everything runs headless and deterministically: bundled fonts (IBM Plex Sans
and Lilex, never system fonts), a fake clock that only moves when you say so,
real input through `Window::dispatch_event`, and pixels from the same wgpu
pipeline the app uses (Mesa lavapipe when there is no GPU).

```sh
just lightbox     # run every Lightbox suite, build contact sheets and the report
just bench-ui     # run UI benchmarks (release, one at a time), then the report
cargo run -p gpui_ce_lightbox -- report    # rebuild the report from what tests wrote
```

The report is `target/lightbox/index.html` (open it from disk). Each suite also
gets `target/lightbox/<suite>/sheet.png`: one image to review the whole suite.

## A test

```rust
use gpui::AppContext as _;
use gpui_lightbox::{FilmSpec, Stage, StageConfig, StyleSpec};
use std::time::Duration;

#[test]
fn settings_panel() {
    let mut stage = Stage::with_config("settings", StageConfig::default().size(480., 360.));
    stage.mount(|_, cx| cx.new(|cx| Settings::new(cx)));

    // Real input: clicks the center of the visible text "Advanced".
    let shot = stage.click_text("Advanced").shot("advanced");

    // Does it follow the design? Numbered violations land in advanced.lint.png.
    shot.lint(&StyleSpec::loupe()).assert_clean();

    // Did anything move? Compared with tests/golden/settings-advanced.png.
    shot.assert_golden("settings-advanced");

    // Film the collapse at 60 fps and check its motion.
    let mut film = stage.film("collapse", FilmSpec::fps(Duration::from_millis(300), 60.), |stage| {
        stage.click_text("Collapse");
    });
    film.track("height", |frame| frame.find_text("Advanced").ok().map(|text| text.bounds.y));
    film.assert_monotonic("height").assert_settles_by(Duration::from_millis(300));
}
```

Add Lightbox as a dev-dependency: `gpui_lightbox = { path = "../gpui_lightbox", package = "gpui_ce_lightbox" }`.

Lightbox is a test harness: like `assert!`, its methods panic with a message
that says what went wrong and where the evidence is, instead of returning
errors.

## Reviewing a suite (for AI agents)

Images are the source of truth; look at them, don't guess.

1. Run `just lightbox` (or `cargo test -p <crate>` then
   `cargo run -p gpui_ce_lightbox -- report`).
2. Read `target/lightbox/<suite>/sheet.png` (then `sheet-2.png`, … for long
   suites). Every cell is a shot, film, matrix or failed golden with a status
   dot: green passed, amber has warnings, red failed. The note names what
   failed: the lint rules (`contrast`, `spacing ×5`), `golden mismatch`,
   `assertion failed`.
3. For each amber or red shot, read `<suite>/<shot>.lint.png`: the shot with
   every violation boxed and numbered, and a legend explaining each number
   with the measured and allowed values (`contrast 2.89:1 (#8d939d on
   #f7f7f9), needs 4.5:1`). The legend's footer lists which rules ran and
   which were skipped (and why).
4. For films, read `<suite>/<film>.film.png`: every frame with its time and a
   blue box around what changed since the previous frame, then one curve per
   tracked property (flagged frames are marked), the detectors' findings and
   the assertions. `<film>.frames/NNN.png` holds each frame at full size.
5. For a failed golden, read `<suite>/<shot>.golden-compare.png`: golden,
   shot and a heatmap side by side (gray: unchanged; blue: noise below the
   tolerance; amber to red: real differences).
6. Exact numbers are in `<suite>/records/*.json`: every painted text line
   (bounds, font, size, weight, color, clip), every box, every violation.

To check a style before it exists in code, write the rules as a `StyleSpec`
(below) and lint shots of the views against it.

## The stage

| | |
|---|---|
| `Stage::new(suite)`, `Stage::with_config(suite, config)` | 800×600 at 2× by default; `StageConfig::default().size(w, h).scale(1.).appearance(Appearance::Dark)` |
| `mount(\|window, cx\| entity)` | opens the window; its first frame already has the configured appearance and scale |
| `draw()`, `refresh()`, `update(\|window, cx\| …)` | draw now; re-render every view; anything else |
| `advance(duration)` | moves the fake clock, fires due timers, delivers the animation frame views asked for, draws |
| `click(point)`, `click_text("Save")`, `hover(point)`, `hover_text(…)`, `drag(from, to, steps)`, `scroll(at, delta)`, `type_text("…")`, `keys("ctrl-k escape")`, `dispatch(event)` | real input through `Window::dispatch_event` |
| `texts()`, `find_text("Save")`, `quads()` | what the last frame painted |
| `shot(name)`, `capture(name)` | a `Shot`: pixels, painted text and quads, metadata; `shot` also saves `<suite>/<name>.png` |
| `resize(w, h)`, `set_scale(s)`, `set_appearance(a)` | change the window in place (style transitions animate the change, as in the app) |
| `record_elements()` | opens gpui's inspector capture with a zero-width dock, so shots know clickable elements (hit-target lint) and benchmarks get phase timings |
| `matrix(name, Matrix::standard(), \|stage, variant\| …)` | shoots every appearance × scale × size into `<name>.matrix.png` |

When `click_text` finds no unique match it fails with the closest visible
texts and where they are.

## Filming animations

`stage.film(name, spec, trigger)` runs `trigger`, then takes a frame at each
of `spec`'s times, advancing the fake clock exactly between them.
Transitions, style transitions, springs, `with_animation` and animated images
all read the executor clock, so frame `n` shows exactly the state at `n × dt`,
and two runs produce byte-identical frames.

```rust
let mut film = stage.film("panel", FilmSpec::fps(Duration::from_millis(600), 30.), |stage| {
    stage.click_text("Open");
});
film.track("x", |frame| frame.quad(|quad| quad.bounds.w == 240.).map(|quad| quad.bounds.x));
film.assert_monotonic("x")                    // never moves backwards
    .assert_no_overshoot("x")                 // never passes where it settles
    .assert_no_jumps("x")                     // no step far larger than its neighbors
    .assert_no_freezes()                      // motion never stalls mid-way
    .assert_follows("x", |t| 24. + 200. * ease_in_out(t / 300.), 0.25)
    .assert_settles_by(Duration::from_millis(500));
```

A film writes `<name>.film.png` (the strip), `<name>.anim.png` (an animated
PNG: open it in a browser) and `<name>.frames/`. Failing assertions save the
film first, then panic with the strip's path. Layout snaps to device pixels,
so tracked positions are exact to half a device pixel.

## Style specs

A `StyleSpec` describes a visual language as data. Every rule is optional;
the default spec checks only what is always a bug (clipped or overlapping
text, content outside the window). `StyleSpec::loupe()` is Loupe's language
from `crates/gpui_inspector/DESIGN.md`.

```rust
let spec = StyleSpec {
    name: "cards".into(),
    text_roles: vec![
        TextRole::new("ui", &["IBM Plex Sans"]).sizes(&[12., 11.]).weights(&[400., 600.]),
        TextRole::new("mono", &["Lilex"]).sizes(&[12.]),
    ],
    text_colors: vec![Color::from_u32(0x1c1f24), Color::from_u32(0x5d636e)],
    spacing: Some(SpacingRule { exempt_sizes: vec![22.], ..SpacingRule::default() }),
    corner_radii: Some(vec![0., 4., 8.]),
    border_widths: Some(vec![0., 1.]),
    min_contrast: Some(4.5),
    min_contrast_large: Some(3.),
    baseline_tolerance: Some(1.),
    min_hit_target: Some(20.),
    ..StyleSpec::default()
};
```

The same spec as JSON (`StyleSpec::from_json`; omitted fields take defaults,
unknown fields are errors):

```json
{
  "name": "cards",
  "text_roles": [
    { "name": "ui", "families": ["IBM Plex Sans"], "sizes": [12, 11], "weights": [400, 600] },
    { "name": "mono", "families": ["Lilex"], "sizes": [12] }
  ],
  "text_colors": ["#1c1f24", "#5d636e"],
  "spacing": { "grid": 4, "exempt_sizes": [22] },
  "corner_radii": [0, 4, 8],
  "min_contrast": 4.5
}
```

| Rule | What it checks |
|---|---|
| `font-family`, `font-size`, `font-weight` | every visible line matches a role; `.SystemUIFont` resolves to the stage's UI font, and a missing font shows up as gpui's fallback (`.ZedMono`) |
| `text-color` | the text color is within `color_tolerance` (Oklab ΔE, default 0.02) of a palette color |
| `contrast` | WCAG contrast of the text color against the pixels actually behind it: the dominant color around and between the glyphs, so gradients, images and overlapping panes count |
| `spacing` | every box and line of text sits a grid multiple from its container's edge or a neighbor, or is centered. It checks spacing, not absolute positions: a right-aligned button's left edge may float. Gaps after text allow a pixel, as layout rounds text boxes up |
| `corner-radius`, `border-width` | radii and borders come from the allowed sets (`allow_pills` permits fully rounded ends) |
| `clipped-text` | text cut off by its container (an ellipsis isn't a cut); `allow_vertical_clip` lets scrolled rows through |
| `text-overlap` | two lines drawn over each other |
| `baseline` | lines side by side whose baselines differ by more than the tolerance |
| `hit-target` | clickable elements smaller than the minimum; needs `stage.record_elements()` and the engine's element tree, and says it was skipped otherwise |
| `outside-window` | content cut off by the window's edge |

`ignore_text` and `ignore_regions` exempt content (a scrolled list's rows,
say). gpui paints a box's fill and each border edge as separate quads;
lint rules see them merged back into the box (`Shot::boxes()`).

`report.assert_clean()`, `assert_at_most(n)` and `assert_caught(rule)` record
their verdict for the report.

## Golden images

`shot.assert_golden("name")` compares with `tests/golden/<name>.png` of the
crate under test; run with `LIGHTBOX_UPDATE=1` to record or re-record. A pixel
differs when its Oklab ΔE exceeds `max_delta_e` (0.04), and the shot fails
when more than `max_fraction` of pixels (0.01 %) differ, so driver noise
passes and a 1 px shift of anything fails. A mismatch writes a heatmap and a
side-by-side comparison, and the report offers a swipe between them.

Goldens are renderer-specific: lavapipe, Metal and discrete GPUs anti-alias
edges and glyphs slightly differently. Record goldens where they are checked
(CI's lavapipe), or loosen `GoldenTolerance` for cross-renderer comparisons.

## Benchmarks

```rust
let report = gpui_lightbox::bench("list/scroll", BenchSpec::default(), |stage| {
    let list = stage.mount(|_, cx| cx.new(|_| List::new(1_000)));
    move |_window, cx| list.update(cx, |list, cx| list.scroll_by(24., cx))
});
println!("{report}"); // p50 / p95 / p99 / max / mean ± sd, and the change vs the previous run
```

The closure mounts the view and returns the step to run before each frame;
Lightbox then times the frame's draw (render, layout, prepaint, paint — not
GPU work). With `refresh: true` (default) every view re-renders each frame;
set it to `false` to time only what the step invalidates. Runs append to
`target/lightbox/bench/<name>.json` with the commit, time and build profile,
and are compared with the previous run of the same profile and settings: a
regression needs the median to move by more than `regression_threshold`
(10 %) and 10 µs, and at least 75 % odds that a frame got slower (so noise
doesn't cry wolf). Per-phase times appear when the engine's inspector capture
records them (`phases: true`). Name benchmark tests `bench_*` so
`just bench-ui` runs them in release, one at a time.

## Output

```
target/lightbox/
  index.html                     the report
  <suite>/sheet.png              contact sheet (sheet-2.png, … when long)
  <suite>/<shot>.png             shots
  <suite>/<shot>.lint.png        annotated lint
  <suite>/<film>.film.png        film strip; .anim.png, .frames/NNN.png
  <suite>/<name>.matrix.png      variant grid
  <suite>/<shot>.golden-*.png    expected, actual, diff, compare
  <suite>/records/*.json         one record per artifact (the report's input)
  bench/<name>.json              benchmark history
```

`$LIGHTBOX_DIR` moves the output; `cargo run -p gpui_ce_lightbox -- clean`
clears suites but keeps benchmark history. Tests in parallel threads and
processes write separate record files, so they never contend.
