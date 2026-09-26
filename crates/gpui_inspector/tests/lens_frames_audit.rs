//! End-to-end tests of the Frames and Audit lenses, rendered headless over
//! the Inbox fixture and over a small live app, driven with real input.
//! Every surface and state is saved to `target/loupe-shots/` for review.

use gpui::{
    AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement, Modifiers,
    ParentElement as _, Pixels, Render, StatefulInteractiveElement as _, Styled as _, Window, div,
    inspector::{
        CaptureLevel, CauseKind, ElementKey, ElementTree, FrameRecord, InspectorCapture,
        InspectorDock,
    },
    point, px, rgb, size,
};
use gpui_inspector::{
    ExportDirectory, Lens,
    fixtures::{self, FrameBuilder, InboxElements, entities, ms, steady_frames},
    harness::LoupeHarness,
    theme::Appearance,
};
use std::{
    panic::Location,
    path::PathBuf,
    time::{Duration, Instant},
};

/// A stand-in for the inspected app behind fixture captures.
struct Backdrop;

impl Render for Backdrop {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = |width: f32| div().h(px(40.)).w(px(width)).bg(rgb(0xf0f2f5)).rounded_md();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(rgb(0xffffff))
            .font_family(gpui_inspector::UI_FONT)
            .text_color(rgb(0x1f2328))
            .child("Inbox")
            .children([420., 360., 480., 300.].map(row))
    }
}

/// A `width`×`height` window with Loupe open over the Inbox fixture, on `lens`.
fn inbox(width: f32, height: f32, lens: Lens) -> (LoupeHarness, InboxElements) {
    let (capture, elements) = fixtures::inbox();
    let harness = fixture(width, height, lens, capture);
    (harness, elements)
}

/// A `width`×`height` window with Loupe open over `capture`, on `lens`.
fn fixture(width: f32, height: f32, lens: Lens, capture: InspectorCapture) -> LoupeHarness {
    let mut harness = LoupeHarness::new(size(px(width), px(height)), |_, cx| cx.new(|_| Backdrop));
    harness.open_loupe();
    harness.install_capture(capture);
    harness.set_appearance(Appearance::Dark);
    harness.update_state(|state, cx| state.set_lens(lens, cx));
    harness
}

fn dock(harness: &mut LoupeHarness, dock: InspectorDock) {
    harness.update(|window, _| window.inspector_capture_mut().unwrap().set_dock(dock));
    harness.redraw_all();
}

/// Painted lines inside `bounds`, top to bottom then left to right.
fn texts_in(harness: &mut LoupeHarness, bounds: Bounds<Pixels>) -> Vec<(Bounds<Pixels>, String)> {
    let mut lines: Vec<(Bounds<Pixels>, String)> = harness
        .painted_text()
        .into_iter()
        .map(|line| (line.visible_bounds(), line.text.to_string()))
        .filter(|(line, _)| line.size.width > px(0.) && bounds.contains(&line.center()))
        .collect();
    lines.sort_by(|(a, _), (b, _)| {
        (a.origin.y.round(), a.origin.x)
            .partial_cmp(&(b.origin.y.round(), b.origin.x))
            .unwrap()
    });
    lines
}

/// The bounds of the painted line `text` inside the element `selector`.
fn text_within(harness: &mut LoupeHarness, selector: &str, text: &str) -> Bounds<Pixels> {
    let area = harness.bounds_of(selector);
    texts_in(harness, area)
        .into_iter()
        .find(|(_, line)| line == text)
        .map(|(bounds, _)| bounds)
        .unwrap_or_else(|| {
            let painted: Vec<String> = texts_in(harness, area)
                .into_iter()
                .map(|(_, t)| t)
                .collect();
            panic!("{text:?} is not painted in {selector}; it shows {painted:?}")
        })
}

/// The leftmost painted line in the flame chart starting with `prefix`
/// (narrow bars truncate their labels).
fn flame_bar(harness: &mut LoupeHarness, prefix: &str) -> Bounds<Pixels> {
    let chart = harness.bounds_of("loupe-flame");
    texts_in(harness, chart)
        .into_iter()
        .filter(|(_, line)| line.starts_with(prefix))
        .min_by(|(a, _), (b, _)| a.origin.x.partial_cmp(&b.origin.x).unwrap())
        .map(|(bounds, _)| bounds)
        .unwrap_or_else(|| panic!("no flame bar labelled {prefix:?}"))
}

/// The flame chart header's `3.2 ms of 40.0 ms`, shown only while zoomed.
fn zoomed_to(harness: &mut LoupeHarness) -> Option<String> {
    harness
        .painted_text()
        .into_iter()
        .map(|line| line.text.to_string())
        .find(|text| {
            text.ends_with(" ms")
                && text.contains(" of ")
                && text.starts_with(|c: char| c.is_ascii_digit())
        })
}

fn highlights(harness: &mut LoupeHarness) -> Vec<Bounds<Pixels>> {
    harness.capture(|capture| {
        capture
            .overlay()
            .highlights()
            .map(|highlight| highlight.bounds)
            .collect()
    })
}

fn element_bounds(tree: &ElementTree, key: ElementKey) -> Bounds<Pixels> {
    tree.get(tree.find(key).unwrap()).unwrap().bounds
}

fn worst_frame(harness: &mut LoupeHarness) -> u64 {
    harness.capture(|capture| {
        capture
            .frames()
            .iter()
            .filter(|frame| !frame.inspector_only)
            .max_by_key(|frame| frame.timings.app_total())
            .unwrap()
            .id
    })
}

fn clipboard(harness: &mut LoupeHarness) -> String {
    harness
        .app(|cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .unwrap_or_default()
}

fn export_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("loupe-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn frames_lens_answers_why_over_the_inbox_fixture() {
    let (mut harness, _) = inbox(1280., 800., Lens::Frames);
    harness.assert_text_visible(Lens::Frames.question());
    for text in [
        "fps",
        "p50",
        "p95",
        "p99",
        "over budget",
        "input p95",
        "Loupe",
        "60 Hz",
    ] {
        harness.assert_text_visible(text);
    }
    // The latest app frame, why it was drawn, and where its time went.
    harness.assert_text_visible("LATEST FRAME");
    harness.assert_text_visible("#239 · ");
    harness.assert_text_visible("within budget");
    harness.assert_text_visible("IssueList notified");
    harness.assert_text_visible("fixtures.rs:");
    harness.assert_text_visible("300 µs before");
    harness.assert_text_visible("none since the previous frame");
    text_within(&mut harness, "loupe-flame", "Frame #239 · 4.5 ms");
    text_within(&mut harness, "loupe-flame", "IssueList");
    text_within(&mut harness, "loupe-bottom-up", "IssueList");
    harness.assert_text_visible("INSIGHTS");
    harness.screenshot("frames-right-dark");

    harness.set_appearance(Appearance::Light);
    harness.screenshot("frames-right-light");
}

#[test]
fn frames_step_jump_and_follow_with_buttons_and_keys() {
    let (mut harness, _) = inbox(1280., 800., Lens::Frames);
    let worst = worst_frame(&mut harness);
    harness.click_text("Jump to worst");
    assert_eq!(harness.state(|state| state.selected_frame()), Some(worst));
    harness.assert_text_visible("SELECTED FRAME");
    harness.assert_text_visible(&format!("#{worst} · "));
    harness.assert_text_visible("× budget");
    harness.assert_text_visible("IssueStore notified");
    harness.screenshot("frames-worst-frame");

    harness.click_text("Latest");
    assert_eq!(harness.state(|state| state.selected_frame()), None);

    // Keys work once the lens has focus; Loupe-only frames are skipped.
    harness.click_text(Lens::Frames.question());
    harness.type_keys("left");
    assert_eq!(harness.state(|state| state.selected_frame()), Some(237));
    harness.type_keys("right");
    assert_eq!(harness.state(|state| state.selected_frame()), Some(239));
    harness.type_keys("w");
    assert_eq!(harness.state(|state| state.selected_frame()), Some(worst));
    harness.type_keys("l");
    assert_eq!(harness.state(|state| state.selected_frame()), None);

    // The chevrons step too.
    harness.click_selector("loupe-frames-previous");
    assert_eq!(harness.state(|state| state.selected_frame()), Some(237));
    harness.click_selector("loupe-frames-next");
    assert_eq!(harness.state(|state| state.selected_frame()), Some(239));
}

#[test]
fn the_flame_chart_zooms_pans_hovers_and_selects() {
    let (mut harness, elements) = inbox(1280., 900., Lens::Frames);
    let worst = worst_frame(&mut harness);
    harness.update_state(|state, cx| state.select_frame(Some(worst), cx));
    for label in [
        "InboxApp",
        "Issu",
        "sort_issues",
        "group_by",
        "fixtures.",
        "Frame #",
    ] {
        flame_bar(&mut harness, label);
    }
    for lane in ["Frame", "Views", "Spans", "Main"] {
        text_within(&mut harness, "loupe-flame", lane);
    }
    harness.screenshot("flame-spike");

    // Hovering a view bar explains it and highlights its element in the app.
    // Narrow bars truncate their labels; the first "Issu…" is IssueList.
    let list = flame_bar(&mut harness, "Issu").center();
    harness.hover(list);
    let tooltip = harness.bounds_of("loupe-flame-tooltip");
    let lines: Vec<String> = texts_in(&mut harness, tooltip)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    for expected in [
        "IssueList",
        "rendered view",
        "Duration",
        "Self",
        "Starts",
        "Embedded at",
    ] {
        assert!(
            lines.iter().any(|line| line == expected),
            "{expected:?} not in {lines:?}"
        );
    }
    let tree = harness.capture(|capture| capture.latest_tree().unwrap().clone());
    assert_eq!(
        highlights(&mut harness),
        [element_bounds(&tree, elements.issue_list)]
    );
    harness.screenshot("flame-hover-tooltip");
    harness.hover(point(px(100.), px(100.)));
    assert!(
        highlights(&mut harness).is_empty(),
        "leaving clears the highlight"
    );

    // Clicking selects the view's element; shift-click reveals it in Elements.
    harness.click(list);
    assert_eq!(
        harness.state(|state| state.selected_element()),
        Some(elements.issue_list)
    );
    assert_eq!(
        harness.capture(|capture| capture.overlay().selected),
        Some(elements.issue_list),
        "the app outlines it too"
    );
    harness.assert_text_visible("Zoom to bar");
    harness.assert_text_visible("Reveal in Elements");
    harness.screenshot("flame-selected-bar");

    // The wheel zooms at the pointer, a drag pans, a double-click fits.
    let chart = harness.bounds_of("loupe-flame");
    let at = point(chart.left() + px(300.), chart.center().y);
    assert_eq!(zoomed_to(&mut harness), None);
    harness.scroll(at, point(px(0.), px(240.)));
    let zoomed = zoomed_to(&mut harness).expect("the wheel zooms in");
    let spans: Vec<f64> = zoomed
        .split(" of ")
        .map(|part| part.trim_end_matches(" ms").parse().unwrap())
        .collect();
    assert!(spans[0] < spans[1] * 0.5, "{zoomed}");
    let axis = |harness: &mut LoupeHarness| -> Vec<String> {
        let chart = harness.bounds_of("loupe-flame");
        let top = Bounds::new(chart.origin, size(chart.size.width, px(18.)));
        texts_in(harness, top).into_iter().map(|(_, t)| t).collect()
    };
    let before = axis(&mut harness);
    harness.drag(at, point(at.x + px(120.), at.y), 6);
    assert_ne!(axis(&mut harness), before, "dragging pans the axis");
    harness.screenshot("flame-zoomed");
    harness.click_with(at, Modifiers::default(), 2);
    assert_eq!(zoomed_to(&mut harness), None, "double-click fits the frame");

    // Keys zoom too, with the lens focused.
    harness.type_keys("=");
    assert!(zoomed_to(&mut harness).is_some());
    harness.type_keys("-");
    assert_eq!(
        zoomed_to(&mut harness),
        None,
        "zooming out stops at the frame"
    );
    harness.type_keys("= = 0");
    assert_eq!(zoomed_to(&mut harness), None);

    harness.click_with(list, Modifiers::shift(), 1);
    assert_eq!(harness.state(|state| state.lens()), Lens::Elements);
}

#[test]
fn bottom_up_scopes_sorts_and_highlights_a_view_type() {
    let (mut harness, elements) = inbox(1280., 900., Lens::Frames);
    let rows = |harness: &mut LoupeHarness| -> Vec<String> {
        let table = harness.bounds_of("loupe-bottom-up");
        texts_in(harness, table)
            .into_iter()
            .map(|(_, t)| t)
            .collect()
    };
    let first = rows(&mut harness);
    assert_eq!(
        &first[..5],
        ["View type", "Calls", "R / C", "Total", "Self"]
    );
    assert_eq!(
        first[5], "IssueList",
        "the latest frame's only render ranks first"
    );

    harness.click_text("All frames");
    let all = rows(&mut harness);
    let app_frames = harness.capture(|capture| {
        capture
            .frames()
            .iter()
            .filter(|frame| !frame.inspector_only)
            .count()
    });
    assert!(
        all.contains(&app_frames.to_string()),
        "every app frame drew InboxApp once: {all:?}"
    );
    harness.screenshot("bottom-up-all-frames");

    // Sorting by name puts InboxApp first; clicking a row highlights it.
    let header = text_within(&mut harness, "loupe-bottom-up", "View type");
    harness.click(header.center());
    assert_eq!(rows(&mut harness)[5], "InboxApp");
    let row = text_within(&mut harness, "loupe-bottom-up", "IssueList");
    harness.click(row.center());
    let tree = harness.capture(|capture| capture.latest_tree().unwrap().clone());
    assert_eq!(
        highlights(&mut harness),
        [element_bounds(&tree, elements.issue_list)]
    );
    harness.screenshot("bottom-up-highlight");
    harness.click_text("Clear highlight");
    assert!(highlights(&mut harness).is_empty());
}

#[test]
fn links_lead_to_entities_events_frames_and_sources() {
    let (mut harness, _) = inbox(1280., 1400., Lens::Frames);
    // The latest frame's cause names IssueList: its entity opens in Entities.
    harness.click_text("IssueList notified");
    assert_eq!(harness.state(|state| state.lens()), Lens::Entities);
    assert_eq!(
        harness.state(|state| state.selected_entity()),
        Some(entities::ISSUE_LIST.into())
    );

    // The cause's site copies its path.
    harness.update_state(|state, cx| state.set_lens(Lens::Frames, cx));
    let site = harness
        .painted_text()
        .into_iter()
        .find(|line| line.text.starts_with("fixtures.rs:"))
        .unwrap();
    harness.click(site.visible_bounds().center());
    let copied = clipboard(&mut harness);
    assert!(
        copied.ends_with(&site.text.replace("fixtures.rs", "")),
        "{copied}"
    );
    assert!(
        copied.contains("gpui_inspector/src/fixtures.rs:"),
        "{copied}"
    );
    harness.assert_text_visible("Copied ");

    // A frame after a click: its input opens in Events.
    let frame = 234;
    harness.update_state(|state, cx| state.select_frame(Some(frame), cx));
    harness.assert_text_visible("2 events · MouseDown, MouseUp");
    harness.click_text("Open in Events");
    assert_eq!(harness.state(|state| state.lens()), Lens::Events);
    let input = harness.state(|state| state.selected_input()).unwrap();
    let redrew = harness.capture(|capture| {
        let record = capture
            .input()
            .iter()
            .find(|record| record.seq == input)
            .unwrap();
        (record.frame, record.kind)
    });
    assert_eq!(redrew, (Some(frame), gpui::inspector::InputKind::MouseDown));

    // Insights about the worst frame link to it.
    harness.update_state(|state, cx| {
        state.select_frame(None, cx);
        state.set_lens(Lens::Frames, cx);
    });
    let worst = worst_frame(&mut harness);
    harness.screenshot("frames-insights");
    harness.click_text(&format!("Frame #{worst}"));
    assert_eq!(harness.state(|state| state.selected_frame()), Some(worst));
}

#[test]
fn export_saves_and_copies_a_perfetto_trace() {
    let (mut harness, _) = inbox(1280., 1400., Lens::Frames);
    let dir = export_dir("export");
    harness.app(|cx| cx.set_global(ExportDirectory(dir.clone())));
    harness.click_text("Export trace…");
    let path = dir.join("loupe-frames-0-239.json");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let events = json["traceEvents"].as_array().unwrap();
    assert!(
        events
            .iter()
            .any(|event| event["cat"] == "view" && event["name"] == "IssueList")
    );
    assert_eq!(json["otherData"]["frames"], 240);
    harness.assert_text_visible("Saved 240 frames");
    harness.assert_text_visible("Open it in ui.perfetto.dev ↗");
    harness.screenshot("frames-exported");

    harness.click_text("Copy trace");
    let copied: serde_json::Value = serde_json::from_str(&clipboard(&mut harness)).unwrap();
    assert_eq!(copied["otherData"]["frames"], 240);
    harness.assert_text_visible("Copied 240 frames");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_budget_regrades_frames_and_the_rail() {
    let (mut harness, _) = inbox(1280., 800., Lens::Frames);
    harness.assert_text_visible("5 over budget");
    harness.click_text("120 Hz");
    assert_eq!(
        harness.capture(|capture| capture.config().budget),
        Duration::from_micros(8_333)
    );
    let over = harness.capture(|capture| {
        capture
            .frames()
            .iter()
            .filter(|frame| !frame.inspector_only && frame.timings.app_total() > ms(8.333))
            .count()
    });
    harness.assert_text_visible(&format!("{over} over budget"));
    harness.screenshot("frames-120hz");
}

#[test]
fn empty_and_idle_states_say_so() {
    let mut empty = fixture(1280., 800., Lens::Frames, InspectorCapture::new_for_test());
    empty.assert_text_visible("No app frames recorded yet");
    empty.screenshot("frames-empty");

    // Frames only Loupe drew, two seconds after the app's last one.
    let mut capture = steady_frames(30, 5.0);
    FrameBuilder::new()
        .at(ms(2_500.))
        .app_time(ms(0.8), ms(0.4))
        .cause(CauseKind::Input { event: "MouseMove" }, true)
        .inspector_only()
        .push(&mut capture);
    let mut idle = fixture(1280., 800., Lens::Frames, capture);
    idle.assert_text_visible("idle");
    idle.assert_text_visible("Animation frame requested");
    idle.screenshot("frames-idle");

    let mut audit = fixture(1280., 800., Lens::Audit, InspectorCapture::new_for_test());
    audit.assert_text_visible("Nothing recorded yet");
    audit.assert_text_visible("Element checks run on the next element tree the app draws");
    audit.screenshot("audit-empty");
}

#[test]
fn both_lenses_lay_out_narrow_wide_and_docked_at_the_bottom() {
    for (lens, name) in [(Lens::Frames, "frames"), (Lens::Audit, "audit")] {
        let (mut harness, _) = inbox(900., 800., lens);
        dock(&mut harness, InspectorDock::Right { width: px(400.) });
        harness.assert_text_visible(lens.question());
        harness.screenshot(&format!("{name}-narrow-dark"));
        harness.set_appearance(Appearance::Light);
        harness.screenshot(&format!("{name}-narrow-light"));

        let (mut harness, _) = inbox(1440., 900., lens);
        dock(&mut harness, InspectorDock::Right { width: px(900.) });
        harness.screenshot(&format!("{name}-wide-dark"));

        let (mut harness, _) = inbox(1280., 800., lens);
        dock(&mut harness, InspectorDock::Bottom { height: px(380.) });
        harness.screenshot(&format!("{name}-bottom-dark"));
        harness.set_appearance(Appearance::Light);
        harness.screenshot(&format!("{name}-bottom-light"));
    }
}

#[test]
fn audit_lens_ranks_findings_and_highlights_every_instance() {
    let (mut harness, _) = inbox(1280., 900., Lens::Audit);
    harness.assert_text_visible(Lens::Audit.question());
    harness.assert_text_visible("KEYBOARD ACCESS");
    harness.assert_text_visible("Start with");
    harness.screenshot("audit-right-dark");

    // Selecting a finding highlights all of its elements and selects the first.
    let findings = harness.capture(|capture| {
        let tree = capture.latest_tree().unwrap().clone();
        (tree, capture.frames().len())
    });
    let (tree, _) = findings;
    // The list's rows, built at one site, fold into one finding.
    harness.click_text("×18");
    let lit = highlights(&mut harness);
    assert_eq!(lit.len(), 18, "every instance is highlighted");
    let first = harness.state(|state| state.selected_element()).unwrap();
    assert_eq!(lit[0], element_bounds(&tree, first));
    harness.screenshot("audit-finding-selected");

    // j / k walk the list; the filter narrows it.
    harness.type_keys("j");
    assert_ne!(harness.state(|state| state.selected_element()), Some(first));
    harness.type_keys("k");
    assert_eq!(harness.state(|state| state.selected_element()), Some(first));
    harness.click_text("Notes");
    assert!(harness.find_text("KEYBOARD ACCESS").is_none());
    harness.screenshot("audit-filtered");

    // Switching lenses withdraws the highlights; coming back restores them.
    harness.update_state(|state, cx| state.set_lens(Lens::Frames, cx));
    assert!(highlights(&mut harness).is_empty());
    harness.update_state(|state, cx| state.set_lens(Lens::Audit, cx));
    assert_eq!(highlights(&mut harness), lit);

    harness.set_appearance(Appearance::Light);
    harness.screenshot("audit-right-light");
}

#[test]
fn capture_controls_change_what_is_recorded() {
    let (mut harness, _) = inbox(1280., 1100., Lens::Audit);
    for text in ["CAPTURE", "Level", "Retained", "Overhead", "240 frames"] {
        harness.assert_text_visible(text);
    }
    harness.click_text("Tree");
    assert_eq!(
        harness.capture(|capture| capture.config().level),
        CaptureLevel::Tree
    );
    harness.assert_text_visible("Contrast and accessible-name checks need element details");
    harness.click_text("144 Hz");
    assert_eq!(
        harness.capture(|capture| capture.config().budget),
        Duration::from_micros(6_944)
    );
    harness.screenshot("audit-capture-controls");
}

// A live app: a view that renders slowly on demand, a button without
// keyboard access and a faint label.

struct SlowPanel {
    slow: bool,
}

/// How long the slow render takes.
const SLOW: Duration = Duration::from_millis(32);

impl Render for SlowPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.slow {
            let _span = gpui::inspector::span("crunch numbers");
            let started = Instant::now();
            while started.elapsed() < SLOW {
                std::hint::spin_loop();
            }
        }
        div()
            .id("slow-panel")
            .h(px(64.))
            .p_3()
            .bg(rgb(0xddf4ff))
            .child(if self.slow { "Crunched" } else { "Ready" })
    }
}

struct LiveApp {
    panel: Entity<SlowPanel>,
}

impl Render for LiveApp {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0xffffff))
            .font_family(gpui_inspector::UI_FONT)
            .text_color(rgb(0x1f2328))
            .child(self.panel.clone())
            .child(
                div()
                    .id("delete")
                    .w(px(96.))
                    .px_3()
                    .py_1()
                    .bg(rgb(0xcf222e))
                    .text_color(rgb(0xffffff))
                    .on_click(|_, _, _| {})
                    .child("Delete"),
            )
            .child(
                div()
                    .id("faint")
                    .text_color(rgb(0xdadada))
                    .child("Last synced a while ago"),
            )
    }
}

#[track_caller]
fn notify_here<T: 'static>(cx: &mut Context<T>) -> &'static Location<'static> {
    cx.notify();
    Location::caller()
}

/// A live app with Loupe open whose panel just rendered slowly. Returns the
/// harness, the slow frame and where the notify that caused it came from.
fn slow_live_app() -> (LoupeHarness, u64, &'static Location<'static>) {
    let panel = std::cell::RefCell::new(None);
    let mut harness = LoupeHarness::new(size(px(1280.), px(1300.)), |_, cx| {
        let slow = cx.new(|_| SlowPanel { slow: false });
        *panel.borrow_mut() = Some(slow.clone());
        cx.new(|_| LiveApp { panel: slow })
    });
    let panel = panel.into_inner().unwrap();
    harness.set_appearance(Appearance::Dark);
    harness.open_loupe();
    harness.advance(gpui_inspector::REFRESH_INTERVAL);

    let site = harness.app(|cx| {
        panel.update(cx, |panel, cx| {
            panel.slow = true;
            notify_here(cx)
        })
    });
    harness.draw();
    harness.app(|cx| panel.update(cx, |panel, _| panel.slow = false));
    harness.advance(gpui_inspector::REFRESH_INTERVAL);
    let slow = harness.capture(|capture| {
        capture
            .frames()
            .iter()
            .find(|frame| slow_view(frame).is_some())
            .expect("the slow render was recorded")
            .id
    });
    (harness, slow, site)
}

fn slow_view(frame: &FrameRecord) -> Option<Duration> {
    frame
        .views
        .iter()
        .find(|view| view.type_name.ends_with("SlowPanel"))
        .map(|view| view.duration)
        .filter(|duration| *duration >= SLOW)
}

#[test]
fn a_slow_render_is_explained_from_pulse_to_perfetto() {
    let (mut harness, slow, site) = slow_live_app();
    // Freeze, so the pulse strip holds still while the pointer moves over it.
    harness.click_text("Freeze");
    harness.click_pulse_frame(slow);
    assert_eq!(harness.state(|state| state.lens()), Lens::Frames);
    assert_eq!(harness.state(|state| state.selected_frame()), Some(slow));

    // Crit, render-dominated, caused by the notify in this file.
    let app_total = harness.capture(|capture| capture.frame(slow).unwrap().timings.app_total());
    assert!(app_total > Duration::from_micros(25_000), "{app_total:?}");
    harness.assert_text_visible(&format!("#{slow} · "));
    harness.assert_text_visible("× budget");
    harness.assert_text_visible("Render dominated with");
    harness.assert_text_visible("SlowPanel notified");
    harness.assert_text_visible(&format!("lens_frames_audit.rs:{}", site.line()));

    // The flame chart has the view's bar at 30+ ms, and the user span.
    text_within(&mut harness, "loupe-flame", "crunch numbers");
    let bar = text_within(&mut harness, "loupe-flame", "SlowPanel").center();
    harness.hover(bar);
    let tooltip = harness.bounds_of("loupe-flame-tooltip");
    let lines: Vec<String> = texts_in(&mut harness, tooltip)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    let duration = lines
        .iter()
        .position(|line| line == "Duration")
        .map(|ix| lines[ix + 1].clone())
        .unwrap_or_else(|| panic!("no duration in {lines:?}"));
    let millis: f64 = duration.trim_end_matches(" ms").parse().unwrap();
    assert!(millis >= 30.0, "{duration}");
    harness.screenshot("live-slow-frame");

    // Bottom-up ranks it first.
    harness.hover(point(px(100.), px(100.)));
    let table = harness.bounds_of("loupe-bottom-up");
    let rows: Vec<String> = texts_in(&mut harness, table)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(rows[5], "SlowPanel", "{rows:?}");

    // The export is valid JSON holding the view span.
    let dir = export_dir("live");
    harness.app(|cx| cx.set_global(ExportDirectory(dir.clone())));
    harness.click_text("Export trace…");
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let span = json["traceEvents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| {
            event["cat"] == "view" && event["name"] == "SlowPanel" && event["args"]["frame"] == slow
        })
        .expect("the view span is exported");
    assert!(span["dur"].as_f64().unwrap() >= 30_000.0, "{span}");
    std::fs::remove_dir_all(dir).ok();

    // A frame Loupe drew for itself replayed the app: it says so, and its
    // views are reused, not rendered.
    let replayed = harness.capture(|capture| {
        let frame = capture
            .frames()
            .iter()
            .find(|frame| frame.inspector_only && frame.id > slow)
            .expect("hovering Loupe drew frames for itself");
        assert!(
            frame
                .views
                .iter()
                .all(|view| view.outcome == gpui::inspector::ViewOutcome::Cached)
        );
        frame.id
    });
    harness.update_state(|state, cx| state.select_frame(Some(replayed), cx));
    harness.assert_text_visible("Loupe only");
    harness.assert_text_visible("Loupe drew this frame for itself");
    harness.assert_text_visible("Drawn only for Loupe");
    harness.assert_text_visible("Loupe updated itself");
    harness.screenshot("live-replayed-frame");
    harness.click_text(Lens::Frames.question());
    harness.type_keys("left");
    assert_eq!(
        harness.state(|state| state.selected_frame()),
        Some(slow),
        "stepping back from Loupe's frame lands on the app's"
    );
}

#[test]
fn audit_finds_the_inaccessible_button_and_the_faint_label() {
    let (mut harness, _, _) = slow_live_app();
    harness.update_state(|state, cx| state.set_lens(Lens::Audit, cx));
    harness.assert_text_visible("Clickable but not reachable by keyboard");
    harness.assert_text_visible("Low text contrast");
    harness.assert_text_visible("Expensive render: SlowPanel took");
    harness.assert_text_visible("1 critical problem to fix first");

    let tree = harness.capture(|capture| capture.latest_tree().unwrap().clone());
    let by_id = |id: &str| {
        tree.elements
            .iter()
            .find(|record| record.id.as_deref() == Some(id))
            .unwrap()
            .bounds
    };
    harness.click_text("Clickable but not reachable by keyboard");
    assert_eq!(highlights(&mut harness), [by_id("delete")]);
    harness.screenshot("live-audit-keyboard");

    harness.click_text("Low text contrast");
    assert_eq!(highlights(&mut harness), [by_id("faint")]);
    harness.assert_text_visible("“Last synced a while ago”");
    harness.screenshot("live-audit-contrast");
}
