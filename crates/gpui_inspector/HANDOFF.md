# Loupe: handoff brief

What is left to do on Loupe (gpui-ce's inspector and profiler), written when
the first build-out session ended. It lists everything that was planned,
started, found or promised and is not finished, with enough context to pick
each item up cold. Read `DESIGN.md` (the contract) and `README.md` (the user
view) first; this file is the gap between them and done.

Branch: `claude/vibrant-archimedes-1vwkyh` on `philocalyst/gpui-ce`, based on
upstream `gpui-ce/gpui-ce` at `c39bf5ab`. Crates: `crates/gpui` (capture,
behind `cfg(any(feature = "inspector", debug_assertions))`),
`crates/gpui_inspector` (package `gpui_ce_inspector`, the UI),
`crates/gpui_lightbox` (package `gpui_ce_lightbox`, headless visual testing).

## 1. Work that was in flight

These were cut off mid-way by usage limits. Each has a precise spec.

### 1.1 Paint-flash redesign (engine, overlay)

**Problem** (seen on the Lightbox filmstrip `loupe-motion/paint-flash`): every
re-rendered view gets a 0.45-alpha magenta fill and nested views stack, so a
root re-render paints the app nearly opaque for ~130 ms: the app is hidden
exactly when you want to see what changed.

**Spec**:
1. Every view that rendered (not cached, not replayed) gets a 2 px inset
   outline that fades over 300 ms on the executor clock. Only the outermost
   rendered view of each rendered subtree also gets a faint fill (peak alpha
   ≈ 0.12), so fills never stack.
2. Color encodes render rate over the last second: a one-off render is a cool
   teal blink, rising through amber to red at ≥ 30 renders/s (the same
   `HOT_RENDERS_PER_SECOND` constant as the render hot-spot insight and audit
   rule). One pure `heat_color(rate)` function, legible over light and dark
   apps (1 px dark halo outside the outline).
3. Views at or above the threshold get a chip (`IssueList ×32/s`), at most
   three, hottest first, not overlapping.
4. Zero cost with paint flash off; inspector-only and replayed frames never
   flash the app.

**Tests to write**: pure tests for the ramp and the outermost-fill selection
(nested rendered views → one fill, N outlines; a cached child inside a
rendered parent → no outline for the child); an engine test sampling a pixel
under nested flashes at the peak (still close to the app's color); rate color
for a view rendered every frame versus once. Then re-film
`loupe-motion/paint-flash` and assert the app stays legible.

### 1.2 Style-lint fixes over every Loupe surface (visual QA)

The Lightbox lint (`tests/visual.rs`: 118 shots, 6 suites × right/bottom/narrow
docks × dark/light) first reported ~35k violations, mostly lint inaccuracy.
Planned and partly done:

* **Lightbox accuracy**, each with a selftest keeping a true positive:
  measure spacing inside borders with 1 px centering tolerance; exempt
  data-drawn plots (pulse bars, dashed budget line, flame and phase bars,
  sparklines) from the 4 px grid; stop linting text hidden under a later
  opaque quad, or outside its content mask. The last needs a
  test-support-only draw order on `PaintedText` (the max `DrawOrder` of its
  glyph sprites) in `crates/gpui/src/{scene.rs,text_system/line.rs,window.rs}`.
* **Real Loupe fixes**:
  * `text_faint` fails AA on every surface (dark 3.0–3.7:1, light 2.7–3.1:1).
  * Light-theme warn/ok/accent text fails AA on surfaces and selection washes.
  * Box-model labels are unreadable on the colored layers.
  * Kbd and kind chips use a 3 px radius instead of the spec's.
  * The key tester and Entities detail use 13 px text, which is off the scale.
  * Lilex at 10.5 px is set SemiBold.
  * The palette overflows the window in the bottom dock.
  * The custom-editor template preview in Settings ellipsizes too late.
  * Single-letter keycaps render uppercase (`J`); they should be `j`.
  * Floating layers (palette, settings, help) need a subtle dim scrim.
* **Contrast rule**:
  * Text that conveys information must meet AA 4.5:1 on every surface it
    sits on, including hover rows and selection washes, in both themes.
  * Only disabled controls may go down to 3:1, and the spec must state that
    exception explicitly.
  * Keep the hierarchy with size and weight steps that all still clear AA:
    roughly faint ≥ 4.5, muted ≈ 7, primary ≥ 12.

If `tests/visual.rs` still fails when you pick this up, this is why.

### 1.3 Performance: the refresh target

The benchmark suite (`crates/gpui_inspector/tests/bench.rs`) runs in release
with `cargo test --release -p gpui_ce_inspector --test bench -- --test-threads=1`
or `just bench-ui`. Measurements on lavapipe (GPU work runs on the CPU there):

| Benchmark | p50, release | Target | Status |
|---|---|---|---|
| App frame, Loupe closed | 1.65 ms | — | baseline |
| App frame, capture only (no dock) | 1.84 ms | — | about 5–10 % over closed |
| App frame, Loupe open (per lens) | 2.3–3.0 ms | — | includes drawing the dock |
| Pointer move, closed / open | 0.001 / 0.012 ms | — | ok |
| Filter keystroke, 5k elements | 1.6 ms | ≤ 8 ms | met |
| Filter keystroke, 1k events | 1.6 ms | ≤ 8 ms | met |
| Sort 2k entities | 5.1 ms | ≤ 8 ms | met |
| Flame zoom / pan step | 3.5 ms | ≤ 8 ms | met |
| Open a lens | 2.7–7.6 ms | ≤ 16 ms | met |
| Palette open / query keystroke | 5.2 / 3.2 ms | ≤ 16 / 8 ms | met |
| **Loupe refresh after a new app frame** | Events 1.4, Elements 2.9, Audit 3.5, Entities 3.7, Frames 5.8 ms | **≤ 2 ms** | **missed except Events** |

Next steps:
* Profile the refresh per lens. The Frames refresh is the worst: it
  re-lays out the flame chart and the stats for every new frame, but it
  could append to them incrementally. Entities rebuilds its table rows.
  Audit re-runs every rule over the latest tree; run the rules off the main
  thread on the frozen capture, as the plan says, or incrementally per
  generation.
* The per-view cached shell (`aa6140db`, "re-render only what changed") and
  its `renders::RenderCounts` guards are the pattern to extend.
* The measurements above predate the "Loupe clicks don't render the app" fix,
  so re-measure the interaction benchmarks.

### 1.4 Engine items that were queued and not started

1. **Owned overlay highlights.** `OverlayHighlight` gains an owner, with
   `overlay_mut().set_highlights(owner, …)` and `clear_highlights(owner)`, so
   lenses never clobber each other. This replaces the fingerprint comparison
   in `gpui_inspector/src/lenses/highlights.rs`.
2. **`ViewOutcome::Replayed`.** App views on frames the window replayed
   currently read `Cached` with zero duration. Add the variant and update the
   recorder, flame chart, bottom-up, insights, Elements view stats, Frames
   notes, fixtures and tests, so that `Cached` means a real cache hit again.
3. **Ticking "ago" labels.** The capture clock exists
   (`InspectorCapture::now()`). The "2.1 s ago" and "idle" labels in Frames,
   Events and the status bar should refresh about once a second while visible,
   without waking anything when nothing on screen changes.
4. **Focus changes reach the inspector.** When focus moves into or out of the
   inspector subtree, re-render its cached views. Then remove the lens
   workarounds: the mouse-down notifies on lens roots, and the `observe →
   notify` on each model in Elements and the Events key tester.
5. **Addressable text.** Text built from `&str`/`SharedString` has no source
   location, so it has no `ElementKey` and can't be picked, box-model
   hovered or style-inspected. Give it a stable key: an interned synthetic
   path from the parent's path plus the child's ordinal among keyless
   siblings. Then simplify the Elements lens's anchor keys and the Audit
   contrast rule's "blame the nearest sited ancestor".
6. **Loupe's own entities.** `EntityInfo` should say when an entity belongs
   to the inspector: its text-field state and caret currently count as app
   entities. Hide them in Entities by default, with a toggle.
7. **Fixture base style.**
   `InspectorCapture::set_selected_style_for_test(key, StyleRefinement)`, so
   fixture captures show a base style in the Elements style grid; use it in
   `fixtures::inbox()`.
8. **Interceptor quirk.** `dispatch_key_down_up_event` still calls the first
   capture-phase key listener on the path after a keystroke interceptor
   stopped the keystroke. Fix it with a test if it's a gpui bug, otherwise
   document why.

## 2. Known limitations of what exists

**Elements**
* Keyless text can't be picked or style-edited (see 1.4 item 5); hovering it
  shows a highlight rather than the box model.
* "Your code" only hides library elements, classified by path, plus views
  classified by type. There are no configurable path prefixes: the plan's
  `set_framework_paths` and the per-project "user code" setting are missing.
* Style editing is disabled while time traveling. Fixture captures have no
  base style (see 1.4 item 7).
* Overrides apply per element path, so every instance changes (the UI says
  so). They last for the session only and aren't persisted per project.
* The "Add property" list is fixed. Reflection of single-argument style
  methods (the plan's M5) doesn't exist, and neither does the "unmapped"
  block for the Rust view.
* A new tree with new keys starts expanded to depth 3.
* The Elements lens follows the Hold command through the generation
  catch-up, not a direct `HoldChanged` notification.

**Events and the key tester**
* A Loupe click on an element with a click listener used to be recorded as an
  app refresh. That is fixed by `a3d56ed3`; re-check the log presentation.
* The first key after a mouse click refreshes the window once (gpui's
  input-modality switch). The refresh now happens after dispatch.
* Headless windows are inactive, so focus listeners don't fire in tests. The
  key tester re-renders from mouse-down listeners.
* Bindings on a lone modifier key can't be intercepted. Pressing Escape twice
  clears the tester, so Escape itself can't be tested.
* When another lens selects a record, the log takes keyboard focus.
* Entity rates are as of the latest drawn frame. Re-sorting live by rate can
  reorder rows under the pointer; consider freezing the order while hovered.

**Settings and help**
* Capture-level costs are qualitative ("Cheapest", "Costs more", "Costliest").
  Show measured numbers from the capture's own overhead benchmark instead.
* While a floating layer is open, a click elsewhere in Loupe only closes it
  (by design); revisit if it feels sticky.

**Overlays**
* Paint flash: see 1.1.
* The replayed app keeps drawing an element's focus ring after focus moves
  into Loupe, until the app next renders. This is documented as intended:
  inspecting doesn't perturb the app.

## 3. Planned features that were not built

The original plan (nine panels, milestones M0–M6) was deliberately
consolidated into five lenses. These parts of it do not exist yet, roughly in
order of value:

1. **Renderer answers** (plan M4): `PlatformRenderer::stats() ->
   Option<RendererStats>` with GPU time per pass (Metal command-buffer
   timestamps, wgpu `TIMESTAMP_QUERY`, D3D11 queries), batches in draw
   order, instance-buffer growth, a public `ScenePlanRequirements` getter, an
   atlas page viewer with `PlatformAtlas::debug_snapshot()` and a leak check.
   Today only scene primitive counts exist (`SceneStats`). This fits in
   Frames (GPU lane, batches) and Audit (atlas leak rule) rather than a new
   lens.
2. **Text answers** (plan M4): `LineLayoutCache`/`WrapCache` hit rates,
   shaping time per frame, font fallback events per platform, and for the
   selected text its shaped runs, glyph boxes, font and cache hit, plus a
   glyph-box overlay. Natural home: the Elements detail for text, with the
   counters in Frames.
3. **Accessibility tree** (plan M5): the inspector registers as an
   AccessKit client (`window/a11y/debug.rs` only builds the tree while a
   client is attached), and shows the tree cross-highlighted with the app,
   with Copy TreeUpdate JSON. Today there are role/label details and a
   missing-label audit rule.
4. **Logs and hangs**: a log sink (level, target, message, callsite) with
   filters, and the existing `profiler/hang.rs` `HangDetector` incidents
   shown inline and linked to their frame. `HangDetector` is still consumed
   by nothing. Both could be a Logs pane under Events.
5. **Background executor lanes** in the flame chart, and task timings on
   Linux and web (only macOS and Windows dispatchers record them; the flame
   chart has Frame, Views, UserSpans and MainThread lanes).
6. **Entities depth**:
   * weak counts, creation site, and Globals as a kind;
   * the "observed by" list with subscription call sites (only counts exist);
   * a leak view (released entities still held weakly), and break on notify
     (freeze plus backtrace);
   * `cx.register_inspectable::<T: Debug>()` for a Debug dump, and
     `#[derive(Inspect)]` editable fields (v2).
7. **Detached and remote Loupe** (plan M6): a Loupe window of its own bound
   to a target window (overlays still paint in the target), an
   `InspectorSource` boundary with a serde bridge over WebSocket or a Unix
   socket, a `gpui_web` build of the UI, and attaching to a release build
   with `--features inspector`. The model types are plain data, but not
   serde yet.
8. **Time travel, part 2**: Elements can view an earlier captured tree, but
   comparing two frames, or two traces, side by side doesn't exist. Nor does
   an `.inspector-trace` file a bug report could carry.
9. **Action record and replay** (following Iced's Comet) for reproducing bugs.
10. **3D exploded layers** (Xcode's "what covers my button").
11. **Degrading under budget**: capture trees every Nth frame, plus every slow
    frame, when capture goes over its budget, and subtract Loupe's own cost
    from the app numbers it reports. Loupe's cost is shown in the status bar
    but not subtracted.
12. **API for app authors**, from plan section 07: `set_framework_paths`,
    `register_inspectable`, `register_panel`, a host-theme bridge
    (`InspectorTheme::from_app`), and
    `VisualTestContext::capture_inspector_trace` for CI traces on test
    failure.
13. **Keys from the plan**: `⌘O` opens the selection in the editor (today
    it's a click on the source link) and `⌘⇧R` copies it as Rust (a button
    today). Hold in 3 s is a palette command with no key (the plan had
    `⌘⌥H`).
14. **Key-tester property test**: random keymaps and context stacks checked
    against real dispatch. Today there are six hand-written cases, each
    matched against the engine and real dispatch.

## 4. Getting it merged

* **CI will likely fail on GPUs.** Loupe's and Lightbox's headless tests
  render real pixels through `WgpuHeadlessRenderer`.
  * Linux runners need a Vulkan driver: install `mesa-vulkan-drivers`
    (lavapipe) in the `ci.yml` test jobs.
  * macOS and Windows runners are untested.
  * Either install a software adapter per OS, or skip pixel tests with a
    clear message when no adapter exists. Keep the logic tests running
    everywhere.
  * Goldens are lavapipe-specific; say so, or tolerate small differences.
* **CI parity** was last checked at `5ea86607`, before Phase 3 merged, and
  passed then:
  * stable clippy `--workspace --all-targets -D warnings`, fmt, MSRV 1.95
    check, cargo machete, the wasm check, and typos;
  * rustdoc `-D warnings` for the new crates.

  Re-run all of these on the final head.
* **Machine-local setup in `DESIGN.md` "Building"**: it refers to `gck` (a
  local wrapper at `/usr/local/bin/gck` that runs cargo with one-line
  diagnostics and without nightly manifest-lint noise), to a user-level
  `~/.cargo/config.toml` (nightly `-Z` features and a shared build dir), and
  to lavapipe. None of these are in the repo. Rewrite the section for plain
  `cargo`, or add `gck` as a script plus a documented config.
* **Pre-existing issues this branch did not fix**:
  * nightly clippy `approx_constant` at `crates/gpui/src/geometry.rs`, where
    `phi()` uses 1.618_034; fixing it changes a public value;
  * about 24 broken or private intra-doc links in `crates/gpui/src/**`;
  * gpui_wgpu's `headless_primitives::smoothed_primitives_share_one_contour`
    fails on lavapipe;
  * manifest warnings (unused workspace dependencies, missing `[lints]`
    inheritance).
* **Size**: about 85 non-merge commits and 68k added lines on top of
  upstream. For upstream review, split it:
  1. gpui core fixes that stand alone:
     * `fc0d597c`, cached views track every entity they read;
     * `32a92b12`, redraw for a new input modality after the event;
     * the replay rebasing, `815affa1`.
  2. The engine capture.
  3. Lightbox.
  4. The Loupe UI.
* **Open questions from the plan**: whether Loupe should be on by default in
  debug builds (the plan recommends opt-in with one line), whether to adopt
  gpui-kit's theme JSON, whether to promote the widgets (virtual tree, data
  table, scrub field) into `gpui_ce_elements`, and the final name.

## 5. Picking this up

* Toolchain: stable 1.95+ builds and tests everything (CI's gates are on
  stable). The session used nightly only for cargo's build-dir features, so
  that parallel worktrees could share one build.
* Headless rendering on Linux: `apt install mesa-vulkan-drivers`. Then
  `cargo test -p gpui_ce_inspector --features test-support` runs every
  surface, and writes screenshots to `target/loupe-shots/` and Lightbox
  output to `target/lightbox/`. `just lightbox` builds the contact sheets
  and `target/lightbox/index.html`.
* The fastest way to see Loupe is the demo:
  `cargo run -p gpui_ce_inspector --example loupe_demo`, then
  `ctrl-shift-i` / `cmd-alt-i`.
* The stories in `tests/demo.rs` are the best specification of end-to-end
  behavior. Each test is a user question ("Why is the issue list slow?",
  "What will `g i` do here?", …) answered through real input.
