# Loupe — design contract

Loupe is gpui's inspector and profiler. It answers *why*: why this element is
this size, why this frame was drawn, why it was slow, why a view re-rendered,
where a click went, what a key will do. Every answer links to a source
location and to the live element.

This document is the contract between the engine capture (in `crates/gpui`)
and the UI (this crate). Read it fully before changing either side.

## Principles

1. **Source is the address.** Every row, span, log line and finding links to a
   `file:line` and to the element. Identity across frames is `ElementKey`
   (interned `InspectorElementPath` + instance), never a bare hash.
2. **Answer why, then what.** The first line of every detail is an
   explanation ("flex_1 grew into what was left: 628 − 232"); raw values come
   second.
3. **Always recording while open.** No record button. The last 240 frames and
   1000 input events are always there; click the red bar.
4. **Live, never jittery.** The UI refreshes at most every 100 ms and only when
   the *app* produced new data. Selection is keyed by `ElementKey`, so the tree
   never loses its place.
5. **Honest about cost.** Zero cost when closed (one `Option` check per hook).
   When open, Loupe's own draw time is measured, shown, and excluded from app
   numbers. Frames caused only by Loupe are flagged `inspector_only`.
6. **Compact, dense, calm.** One screen, five lenses, nothing behind overflow
   menus. Rows are 22 px, hairline rules, color only means state.
7. **Keyboard first.** Every action has a key; `j/k`/arrows work in every list.

## Architecture

```
┌─ crates/gpui ─────────────────────────────────────────────────────────┐
│ inspector/model.rs    plain data: FrameRecord, ElementTree, InputRecord │
│ inspector/capture.rs  InspectorCapture: rings, interner, overlay, pick, │
│                       overrides, forced states, dock; query helpers     │
│ hooks                 window.rs draw/dispatch, element.rs Drawable,     │
│                       view.rs, app.rs notify, div.rs style + details    │
└──────────────────────────────┬────────────────────────────────────────┘
                               │ window.inspector_capture()      (read, in render)
                               │ window.inspector_capture_mut()  (commands)
┌─ crates/gpui_inspector ──────┴────────────────────────────────────────┐
│ Loupe (root view, cached)  shell: toolbar · pulse strip · lens rail ·  │
│                            lens body · status bar · palette           │
│ lenses/   elements · frames · events · entities · audit                │
│ analysis/ pure functions over captures (stats, why-size, flame layout, │
│           bottom-up, insights, trace export, audit rules, rust patch)  │
│ widgets/  tree, table, splitter, tabs, pill, kbd, sparkline, strip,    │
│           scrub field, text field, icon, section, tooltip, menu        │
│ theme.rs  tokens · harness.rs headless screenshots (test-support)      │
└───────────────────────────────────────────────────────────────────────┘
```

* The window owns `Option<Box<InspectorCapture>>`, created by
  `Window::toggle_inspector` and dropped when closed. **The capture is the only
  store**; the UI reads it directly in `render` (it is not being mutated then:
  the inspector subtree is excluded from capture).
* The UI is drawn as the window's inspector root inside
  `InspectorDock::split(viewport).1`; the app root gets the rest
  (`Window::app_bounds`).
* The `Loupe` entity lives in `Inspector::ui_state` and is embedded as a
  **cached** view, so app frames never re-render Loupe and Loupe updates never
  re-render the app.
* **Refresh loop, loop-free by construction:** `Loupe` runs a 100 ms timer
  that compares `capture.generation()` with the generation it last rendered
  and calls `cx.notify()` only on change. `generation` is bumped only by
  frames that are not `inspector_only` and by input not consumed by Loupe.
  Loupe must **never** call `window.refresh()` (that re-renders every view);
  it notifies its own entities, and uses `capture_mut()` for commands.
* Heavy derived data (percentiles, flame layout, bottom-up, audit) is memoized
  per lens keyed by `(generation, selection)`; never recomputed per render.

## Engine API

Status: ✅ implemented in Phase 0 · 🔧 stub, implemented by the engine slice.

| API | Status | Notes |
|---|---|---|
| `Window::toggle_inspector(cx)` | ✅ | creates/drops the capture; dock remembered |
| `Window::is_inspector_open()` | ✅ | |
| `Window::inspector_capture() / _mut()` | ✅ | |
| `Window::app_bounds() / inspector_bounds()` | ✅ | dock split |
| `InspectorCapture` queries (`frames`, `frame(id)`, `latest_app_frame`, `latest_tree`, `input`, `path_info`, `notify_stats`, `generation`, `overlay(_mut)`, `pick`, `dock`, `set_frozen`, `config_mut`, `overrides`, `set_override`, `forced_states`, `set_forced_states`, `selected_style`, `retained_bytes`, `clear`) | ✅ | pure store |
| `InspectorCapture::{new_for_test, push_frame_for_test, push_input_for_test, intern_path_for_test}` | ✅ | fixtures for UI tests |
| `ElementTree::{children, get, find, ancestors, owning_view, hit_test, rebuild_children}` | ✅ | |
| `Inspector::ui_state(init)` / `InspectorEvent` | ✅ / 🔧 | events emitted by engine picking |
| `Window::start_inspector_pick / stop_inspector_pick` | 🔧 | pick over the captured tree |
| `Window::inspect_current_element(f)` | 🔧 | elements report `ElementDetails` |
| `Window::inspector_resolve_keystrokes(&[Keystroke], cx)` | 🔧 | pure key tester |
| `App::inspector_entities()` | 🔧 | entity registry |
| `gpui::inspector_span!` / `inspector::span(name)` | 🔧 | user spans (guard, `#[track_caller]`) |
| capture hooks filling `FrameRecord`, `ElementTree`, `InputRecord`, causes, view spans, scene stats, foreground slices | 🔧 | |
| overlay pass painting `OverlayState` | 🔧 | replaces `paint_inspector_hitbox` |
| `capture_mut().set_override / set_forced_states` applied in `Div` | 🔧 | replaces `DivInspectorState` flow |
| `SelectedStyle` recorded for the selected element | 🔧 | |
| `dock` changes via `capture_mut().dock = …` → relayout | 🔧 | add `set_dock` |
| `Window::painted_text()` (test-support) | 🔧 (UI foundation slice) | text runs painted last frame with bounds |

### Engine rules

* Every hook is `if let Some(capture) = self.inspector_capture.as_deref_mut()`
  (or a cheap flag), inside `#[cfg(any(feature = "inspector", debug_assertions))]`.
* Nothing is recorded while drawing the inspector's own root; that time goes to
  `PhaseTimings::inspector`. Input routed to the dock is recorded with
  `inspector: true`.
* A frame is `inspector_only` when every cause is `from_inspector` (a notify on
  an entity whose view lives under the inspector root, or input to the dock).
* No per-frame heap churn beyond the records themselves: reuse scratch
  buffers (parent stack, path lookups) across frames.
* When frozen, rings stop updating; overlays, picking and overrides still work.
* Picking walks the **captured tree** (`ElementTree::hit_test`), so any element
  with bounds can be picked, not only hitbox owners. `[`/`]` or wheel change
  depth; click selects and emits `InspectorEvent::Picked`; Escape cancels.
* Overlays are painted after all roots with raw quads (no layout, no text
  shaping except label chips) and never invalidate the app.

## UI

### Anatomy (right dock, ~560 px; bottom dock lays lenses out side-by-side)

```
┌────────────────────────────────────────────────────────────┐
│ ◎ Pick  ▢ ◌ ▦ ⚠ ⇲ ⊡  ❚❚   [ Find anything…        ⌘K ]  ⇆ │ toolbar 32
├────────────────────────────────────────────────────────────┤
│ ▁▁▂▁▁▁█▁▁▂▁▁▁▁▁▃▁▁▁▁▁▁▁▁▁▁▁▁▅▁▁▁▁▁▁  118 fps · p95 7.9 ms │ pulse strip 40
├────────────────────────────────────────────────────────────┤
│ Elements 1,284 │ Frames 7▲ │ Events 42 │ Entities 311 │ Audit 5 │ rail 28
├────────────────────────────────────────────────────────────┤
│                                                            │
│   lens body (tree + detail, stacked or side by side)       │
│                                                            │
├────────────────────────────────────────────────────────────┤
│ Root › Sidebar › IssueList › div#row-3   app 3.1 · loupe 0.4 ms · 2.1 MB │ status 22
└────────────────────────────────────────────────────────────┘
```

* **Pulse strip** (always visible): one bar per recorded frame, height ∝
  `app_total` (sqrt scale, capped at 3× budget), colored ok / warn (≤1.5×) /
  crit; inspector-only frames are faint ticks; frames with a retained tree carry
  a dot; the selected frame is outlined; a dashed budget line; hover shows
  `#18372 · 23.4 ms · render 14.1 · IssueStore notified`. Click selects the
  frame (and switches to Frames); the selection also drives Elements'
  time-travel. Right side: live fps and p95.
* **Rail** tabs carry live counts, so the rail itself is a dashboard.
* **Status bar**: selection breadcrumb (clickable), app ms, Loupe's own ms,
  retained memory, `FROZEN` pill.
* **Palette** (`⌘K`/`ctrl-k`): fuzzy search across elements (`#`), entities
  (`@`), commands (`>`) and lenses. Every toolbar toggle is a command.
* **Responsive**: lenses read the dock size; below 520 px wide they stack
  master/detail vertically with a splitter, otherwise side by side. Nothing
  overflows or clips text without an ellipsis.

### Lenses

1. **Elements** — *What is this, where did it come from, why does it look like
   that?* Virtual tree (kind glyph, name, `#id`, size, flag icons, view render
   count, override dot); filter; time-travel banner when viewing an old frame.
   Detail: title + source link + owning view; **Why this size** (one sentence
   per axis); box model diagram (scrub-editable); Style grid over the
   `StyleRefinement` JSON (set fields only, scrub numbers, color swatches,
   overridden rows marked, revert, *Copy Rust*); Interactivity flags + forced
   states; Cost (primitives, render count, last cause).
2. **Frames** — *Why was that frame drawn, and why was it slow?* Stats (fps,
   p50/p95/p99, over-budget count), phase bar, causes with sites, flame chart
   (views, user spans, main-thread tasks) with wheel zoom / drag pan, bottom-up
   self-time table, Insights (plain-language findings), *Jump to worst*,
   *Export trace* (Chrome JSON for Perfetto).
3. **Events** — *Where did my click go? What will this key do here?* Coalesced
   input log with filter chips and pause; selected event's hit path (click to
   select element), context stack, actions and whether handled; **Key tester**:
   a capture box that resolves keystrokes via `inspector_resolve_keystrokes`
   without dispatching, showing the winner and why each other binding loses.
4. **Entities** — *What's alive, who's watching it, who keeps poking it?*
   Sortable table (id, type, kind, refs, observers, notifies/s sparkline, last
   notify site); detail with 60 s sparkline, *Reveal in Elements*, *Notify*.
5. **Audit** — *What should I fix first?* Continuous rules over the latest tree,
   frames and entities (clickable without keyboard access, low contrast,
   zero-size hitboxes with listeners, render hot spots, expensive render,
   overflowing content, missing a11y labels, duplicate ids) with severity,
   explanation and links; capture self-cost.

### Overlays (painted by the engine from `OverlayState`)

Box model on hover/selection with a label chip (`div#close 24×24 ·
issue_detail.rs:97`), outlines by depth, paint flashing for re-rendered views
(fades over ~300 ms), hitboxes, slow-frame border, overflow stripes, and
UI-requested highlights (e.g. hovering a flame span highlights its element).

### Visual language

* Panes are rectangles; 4 px radius only on pressables; 8 px on floating layers.
* 4 px grid. Rows 22 px (trees/tables), 24 px (property grids). Toolbar 32,
  rail 28, status 22. Controls 20 or 24 px.
* `IBM Plex Sans` for UI (12 px body, 11 px secondary, 10.5 px uppercase section
  labels with tracking), `Lilex` for values, paths, ids and code.
* Tokens (`theme.rs`, dark + light, follows window appearance): `bg`,
  `surface`, `surface_2`, `hover`, `selected`, `line`, `line_strong`, `text`,
  `text_muted`, `text_faint`, `accent`, `ok`, `warn`, `crit`, `view`,
  `component`, and a fixed phase ramp `input · render · layout · prepaint ·
  paint · present · inspector` used identically everywhere.
* Overlays use web conventions: content blue, padding green, margin orange.

### Keys (within the `Loupe` key context)

`⌘K`/`ctrl-k` palette · `alt-1…5` lenses · `ctrl-shift-c`/`cmd-shift-c` pick ·
`[`/`]` pick depth · `space` freeze (when no text field is focused) · `j/k`,
arrows, `left/right` collapse/expand in trees · `enter` open/select ·
`escape` cancel pick / close palette.

## Code standards

* Match gpui's idiom and comment density; doc comments on every `pub` item.
* No `unwrap()`/`expect()` in non-test code without a reason in the message;
  no `todo!`, `dbg!`, dead code, or `#[allow]` without a reason.
* Views stay thin: layout + event wiring. Logic lives in `analysis/` as pure,
  unit-tested functions over model types.
* One widget, one file; widgets are generic and reusable (candidate for
  promotion to `gpui_ce_elements`).
* Clippy clean with `-D warnings` on nightly and stable; `cargo fmt`.

## Testing standard ("nothing is lying")

* **Engine**: tests that build known trees and assert parent links, kinds,
  bounds, visible bounds, paint order, flags; causes for notify / refresh /
  resize / input; cached-view reuse keeps the tree complete; input records with
  hit paths and actions; key resolution cross-checked against real dispatch;
  zero records while closed; `inspector_only` detection; overhead measured.
* **UI logic**: unit tests for every `analysis/` function with edge cases.
* **UI rendering**: fixture captures (`push_frame_for_test`) rendered headless;
  assertions via `Window::painted_text()` (text + bounds) and pixel samples.
* **End to end**: `harness.rs` opens a real app window with Loupe docked
  (`HeadlessAppContext` + `CosmicTextSystem::new_without_system_fonts` +
  `WgpuHeadlessRenderer`), drives real input (`window.dispatch_event`), clicks
  every button by its text, and saves `target/loupe-shots/<name>.png` for
  visual review. Every surface must have a screenshot.

## Building

* Nightly toolchain (repo override) with a shared build dir
  (`~/.cargo/config.toml`), so parallel worktrees share compiled deps.
* `gck <cargo args>` runs cargo with one-line diagnostics and without nightly
  manifest-lint noise, e.g. `gck test -p gpui_ce_inspector`.
* Headless GPU rendering uses Mesa lavapipe (installed).
