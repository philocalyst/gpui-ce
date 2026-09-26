//! End-to-end tests of the Elements lens, rendered headless with real input:
//! the Inbox fixture for every surface and state, and a small live app for
//! real behavior (picking, editing styles, forcing states). Every test saves
//! screenshots to `target/loupe-shots/` for review.

use gpui::{
    AppContext as _, Bounds, Context, IntoElement, Modifiers, ParentElement as _, PlatformInput,
    Render, ScaledPixels, ScrollDelta, ScrollWheelEvent, Styled as _, TouchPhase, Window, div,
    inspector::{
        CauseKind, ElementFlags, ElementKey, ElementTree, InspectorDock, LayoutFacts, SizeSpec,
    },
    point,
    prelude::*,
    px, rgb, rgb_to_hsla, size,
};
use gpui_inspector::{
    Lens, LensLayout, REFRESH_INTERVAL,
    fixtures::{self, ElementSpec, FrameBuilder, InboxElements, TreeBuilder, ms},
    harness::LoupeHarness,
    theme::Appearance,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

// ---------------------------------------------------------------------------
// The Inbox fixture.

/// A stand-in for the inspected app, so screenshots show both sides.
struct InboxApp;

impl Render for InboxApp {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = |sender: &'static str, subject: &'static str, selected: bool| {
            div()
                .h(px(56.))
                .px_4()
                .flex()
                .flex_col()
                .justify_center()
                .border_b_1()
                .border_color(rgb(0xeaeef2))
                .when(selected, |this| this.bg(rgb(0xddf4ff)))
                .child(div().text_sm().text_color(rgb(0x1f2328)).child(sender))
                .child(div().text_xs().text_color(rgb(0x656d76)).child(subject))
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .font_family(gpui_inspector::UI_FONT)
            .child(row("Grace Hopper", "Re: flaky layout test on CI", false))
            .child(row("Alan Turing", "Release notes for 0.2.3", false))
            .child(row(
                "Katherine Johnson",
                "Scroll jank in the issue list",
                false,
            ))
            .child(row("Edsger Dijkstra", "Design review: new sidebar", true))
    }
}

/// The Inbox recording, with one more frame whose tree also explains why
/// `div#row-3` has its size (the fixture's trees carry no layout facts).
fn inbox_capture() -> (gpui::inspector::InspectorCapture, InboxElements) {
    let (mut capture, elements) = fixtures::inbox();
    let mut tree: ElementTree = (**capture.latest_tree().expect("inbox keeps trees")).clone();
    let row_3 = tree.find(elements.row_3).expect("row 3") as usize;
    let details = tree.elements[row_3].details.get_or_insert_default();
    details.layout = Some(LayoutFacts {
        display: "flex".into(),
        flex_direction: Some("row".into()),
        size: size(SizeSpec::Fraction(1.), SizeSpec::Pixels(px(64.))),
        flex_shrink: 1.,
        ..Default::default()
    });
    details.text_color = Some(rgb_to_hsla(rgb(0x1f2328)));
    let start = capture.latest_frame().map_or(ms(0.), |frame| frame.start) + ms(16.);
    FrameBuilder::new()
        .at(start)
        .app_time(ms(4.2), ms(0.4))
        .cause(CauseKind::Animation, false)
        .tree(Arc::new(tree))
        .push(&mut capture);
    (capture, elements)
}

/// A `width`×`height` window with Loupe open on the Elements lens over the
/// Inbox recording.
fn inbox_harness(width: f32, height: f32) -> (LoupeHarness, InboxElements) {
    let mut harness = LoupeHarness::new(size(px(width), px(height)), |_, cx| cx.new(|_| InboxApp));
    harness.open_loupe();
    let (capture, elements) = inbox_capture();
    harness.install_capture(capture);
    harness.set_appearance(Appearance::Dark);
    (harness, elements)
}

fn select(harness: &mut LoupeHarness, key: ElementKey) {
    harness.update_state(|state, cx| state.select_element(Some(key), cx));
}

fn selected(harness: &mut LoupeHarness) -> Option<ElementKey> {
    harness.state(|state| state.selected_element())
}

/// The top edge of the first painted line equal to (or containing) `text`.
fn top_of(harness: &mut LoupeHarness, text: &str) -> gpui::Pixels {
    harness
        .find_text(text)
        .unwrap_or_else(|| panic!("{text:?} is not visible"))
        .origin
        .y
}

#[test]
fn the_tree_and_every_detail_section_render_in_both_themes() {
    let (mut harness, elements) = inbox_harness(1280., 1000.);
    assert_eq!(harness.state(|state| state.lens()), Lens::Elements);
    // The first levels start expanded.
    for text in [
        "InboxApp",
        "div#root",
        "Sidebar",
        "IssueList",
        "IssueDetail",
    ] {
        harness.assert_text_visible(text);
    }
    select(&mut harness, elements.row_3);
    harness.assert_text_visible("div#row-3");
    for text in [
        "WHY THIS SIZE",
        "BOX MODEL",
        "STYLE",
        "INTERACTIVITY",
        "DETAILS",
        "COST",
    ] {
        harness.assert_text_visible(text);
    }
    // Why first: one sentence per axis.
    harness.assert_text_visible("100% of its parent's 620 = 620.");
    harness.assert_text_visible("Fixed: the style sets its height to 64.");
    // The header: name, id, size, source and owning view.
    harness.assert_text_visible("#row-3");
    harness.assert_text_visible("620×64");
    harness.assert_text_visible("fixtures.rs:");
    harness.assert_text_visible("IssueList");
    // The box model draws the padding in place and the content box inside it.
    harness.assert_text_visible("padding");
    harness.assert_text_visible("596×40");
    // Interactivity, details and cost.
    harness.assert_text_visible("Clickable");
    harness.assert_text_visible("#ddf4ff");
    harness.assert_text_visible("Contrast");
    harness.assert_text_visible("Primitives");
    harness.assert_text_visible("Its view");
    harness.screenshot("elements-wide-dark");

    harness.set_appearance(Appearance::Light);
    harness.assert_text_visible("WHY THIS SIZE");
    harness.screenshot("elements-wide-light");
}

#[test]
fn views_show_their_renders_and_why_they_last_rendered() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    select(&mut harness, elements.issue_list);
    let rendered = harness.capture(|capture| {
        capture
            .frames()
            .iter()
            .filter(|frame| !frame.inspector_only)
            .filter(|frame| {
                frame.views.iter().any(|view| {
                    view.entity == gpui::EntityId::from(fixtures::entities::ISSUE_LIST)
                        && view.outcome == gpui::inspector::ViewOutcome::Rendered
                })
            })
            .count()
    });
    // The tree row carries the render count; the Cost section explains it.
    harness.assert_text_visible(&format!("×{rendered}"));
    harness.assert_text_visible("Renders");
    harness.assert_text_visible(&format!("{rendered} of "));
    harness.assert_text_visible("Last render");
    harness.screenshot("elements-view-cost");
}

#[test]
fn narrow_docks_stack_the_tree_over_the_detail() {
    let (mut harness, elements) = inbox_harness(900., 760.);
    harness.update(|window, _| {
        window
            .inspector_capture_mut()
            .unwrap()
            .set_dock(InspectorDock::Right { width: px(420.) })
    });
    harness.redraw_all();
    assert_eq!(
        harness.update(|window, _| LensLayout::of(window)),
        LensLayout::Stacked
    );
    select(&mut harness, elements.close);
    harness.assert_text_visible("div#close");
    let row = top_of(&mut harness, "div#close");
    let why = top_of(&mut harness, "WHY THIS SIZE");
    assert!(why > row, "the detail sits under the tree");
    let dock_left = harness.update(|window, _| window.inspector_bounds().unwrap().origin.x);
    for line in harness.painted_text() {
        let bounds = line.visible_bounds();
        if bounds.origin.x >= dock_left && bounds.size.width > px(0.) {
            assert!(
                bounds.right() <= dock_left + px(420.) + px(0.5),
                "{:?} overflows the dock",
                line.text
            );
        }
    }
    harness.screenshot("elements-narrow-dark");
    harness.set_appearance(Appearance::Light);
    harness.screenshot("elements-narrow-light");
}

#[test]
fn bottom_docks_lay_the_tree_beside_the_detail() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    harness.update(|window, _| {
        window
            .inspector_capture_mut()
            .unwrap()
            .set_dock(InspectorDock::Bottom { height: px(360.) })
    });
    harness.redraw_all();
    select(&mut harness, elements.send);
    let row = harness.find_text("div#send").unwrap();
    let why = harness.find_text("WHY THIS SIZE").unwrap();
    assert!(
        why.origin.x > row.right(),
        "the detail sits beside the tree"
    );
    harness.screenshot("elements-bottom-dock");
}

#[test]
fn empty_states_say_what_to_do() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    harness.assert_text_visible("Select an element in the tree, or pick one in the app");
    harness.assert_text_visible(Lens::Elements.question());
    harness.assert_text_visible("Start picking");
    harness.screenshot("elements-nothing-selected");
    harness.click_text("Start picking");
    assert!(harness.capture(|capture| capture.pick().active));
    harness.type_keys("escape");
    assert!(!harness.capture(|capture| capture.pick().active));

    // Holding keeps the app still while you pick (say, a hover menu).
    harness.click_text("Hold app");
    assert!(harness.capture(|capture| capture.is_holding()));
    harness.assert_text_visible("Release app");
    harness.click_text("Release app");
    assert!(!harness.capture(|capture| capture.is_holding()));
    // The toolbar's pin is the same command: the lens follows it.
    harness.click_selector("loupe-hold");
    harness.assert_text_visible("Release app");
    harness.click_selector("loupe-hold");
    harness.assert_text_visible("Hold app");

    let mut empty = LoupeHarness::new(size(px(1280.), px(800.)), |_, cx| cx.new(|_| InboxApp));
    empty.open_loupe();
    empty.install_capture(fixtures::steady_frames(30, 5.));
    empty.set_appearance(Appearance::Dark);
    empty.assert_text_visible("No element tree yet");
    empty.assert_text_visible("Use the app: Loupe records its element tree");
    empty.screenshot("elements-no-tree");
}

#[test]
fn filtering_keeps_matches_with_their_ancestors() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    harness.click_text("Filter elements");
    harness.type_text("row-3");
    assert_eq!(
        harness.state(|state| state.filters().elements.to_string()),
        "row-3"
    );
    harness.assert_text_visible("div#row-3");
    harness.assert_text_visible("IssueList");
    assert!(
        harness.find_text("div#row-4").is_none(),
        "other rows are filtered out"
    );
    let total = harness.capture(|capture| capture.latest_tree().unwrap().elements.len());
    harness.assert_text_visible(&format!("1 of {total}"));
    harness.screenshot("elements-filter");

    // Matching text, too: a sender's name.
    harness.type_keys("secondary-a");
    harness.type_text("Dijkstra");
    harness.assert_text_visible("\"Edsger Dijks");
    harness.screenshot("elements-filter-text");

    // Selecting a match and clearing the filter keeps it selected and shown.
    harness.click_text("\"Edsger Dijks");
    let dijkstra = selected(&mut harness).expect("a match is selected");
    harness.type_keys("escape");
    assert_eq!(
        harness.state(|state| state.filters().elements.to_string()),
        ""
    );
    harness.assert_text_visible("div#list-toolbar");
    harness.assert_text_visible("div#row-3");
    assert_eq!(selected(&mut harness), Some(dijkstra));
    harness.assert_text_visible("\"Edsger Dijks");
}

#[test]
fn keyboard_navigation_walks_and_folds_the_tree() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    harness.click_text("IssueList");
    assert_eq!(selected(&mut harness), Some(elements.issue_list));
    // Right steps into an expanded row, then expands a folded one.
    harness.type_keys("right");
    let child = selected(&mut harness).expect("stepped into IssueList");
    assert_ne!(child, elements.issue_list);
    harness.type_keys("right");
    harness.assert_text_visible("div#list-toolbar");
    // Left folds it again, then steps out to the parent, then folds that.
    harness.type_keys("left");
    assert!(harness.find_text("div#list-toolbar").is_none());
    assert_eq!(selected(&mut harness), Some(child));
    harness.type_keys("left");
    assert_eq!(selected(&mut harness), Some(elements.issue_list));
    harness.type_keys("left");
    assert!(
        harness.find_text("div#issue-list").is_none(),
        "left folds an expanded row"
    );
    // j and k walk the visible rows.
    harness.type_keys("j");
    let next = selected(&mut harness);
    assert_ne!(next, Some(elements.issue_list));
    harness.type_keys("k");
    assert_eq!(selected(&mut harness), Some(elements.issue_list));
    harness.type_keys("down");
    assert_eq!(selected(&mut harness), next);
}

#[test]
fn hovering_a_row_outlines_its_element_in_the_app() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    let row = harness.find_text("IssueList").unwrap();
    harness.hover(row.center());
    assert_eq!(
        harness.capture(|capture| capture.overlay().hovered()),
        Some(elements.issue_list)
    );
    harness.hover(point(px(100.), px(100.)));
    assert_eq!(harness.capture(|capture| capture.overlay().hovered()), None);
}

#[test]
fn time_travel_shows_an_earlier_frame_until_back_to_live() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    let spike = harness.capture(|capture| {
        let latest = capture.latest_tree().unwrap().clone();
        capture
            .frames()
            .iter()
            .find(|frame| {
                frame
                    .tree
                    .as_ref()
                    .is_some_and(|tree| !Arc::ptr_eq(tree, &latest))
            })
            .map(|frame| frame.id)
            .unwrap()
    });
    select(&mut harness, elements.row_3);
    harness.update_state(|state, cx| state.select_frame(Some(spike), cx));
    harness.assert_text_visible(&format!("Frame #{spike} · "));
    // The banner tells the frame's age, and it ticks with the clock.
    let banner = |harness: &mut LoupeHarness| {
        harness
            .painted_text()
            .into_iter()
            .map(|line| line.text.to_string())
            .find(|text| text.starts_with(&format!("Frame #{spike} · ")))
            .expect("the time travel banner")
    };
    // Past the second after which the pulse strip reads idle (which redraws
    // the shell, and the lens with it), only the lens' own tick updates it.
    harness.advance(Duration::from_secs(2));
    let before = banner(&mut harness);
    assert!(before.ends_with(" ago"), "{before}");
    harness.advance(Duration::from_secs(1));
    assert_ne!(banner(&mut harness), before, "its age ticks");
    harness.assert_text_visible("Back to live");
    // The earlier tree has no layout facts for row 3: the detail says so
    // instead of borrowing the live frame's.
    harness.assert_text_visible("Sized by layout; details not captured.");
    harness.screenshot("elements-time-travel");

    harness.click_text("Back to live");
    assert_eq!(harness.state(|state| state.selected_frame()), None);
    assert!(harness.find_text("Back to live").is_none());
    harness.assert_text_visible("Fixed: the style sets its height to 64.");
}

#[test]
fn a_vanished_selection_keeps_its_last_known_record() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    select(&mut harness, elements.row_3);
    // The app draws a frame without row 3.
    let frame = harness.update(|window, _| {
        let capture = window.inspector_capture_mut().unwrap();
        let mut builder = TreeBuilder::new(capture);
        builder.open(ElementSpec::view("inbox::InboxApp", 1).bounds(0., 0., 1280., 800.));
        builder.leaf(ElementSpec::text("Signed out"));
        builder.close();
        let tree = builder.build(0);
        let start = capture.latest_frame().unwrap().start + ms(16.);
        FrameBuilder::new()
            .at(start)
            .app_time(ms(3.), ms(0.3))
            .cause(CauseKind::Animation, false)
            .tree(Arc::new(tree))
            .push(capture)
    });
    harness.advance(REFRESH_INTERVAL);
    assert_eq!(selected(&mut harness), Some(elements.row_3), "no jumping");
    harness.assert_text_visible(&format!("Not in frame #{frame}."));
    harness.assert_text_visible("620×64");
    harness.screenshot("elements-vanished");
}

#[test]
fn scrolled_trees_pin_the_ancestors_of_the_first_row() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    // Reveal a deep row so the tree is long enough to scroll.
    select(&mut harness, elements.overflowing_subject);
    let tree = harness.find_text("InboxApp").unwrap();
    harness.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: tree.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-400.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            }),
            cx,
        );
    });
    harness.draw();
    harness.screenshot("elements-sticky-crumbs");
    // The pinned bar names the ancestors of the rows under it, down to
    // the uniform list the rows scrolled past.
    let bar = harness.find_text("›").expect("a pinned breadcrumb");
    let list = harness
        .find_text("IssueList")
        .expect("IssueList in the breadcrumb");
    assert!((list.center().y - bar.center().y).abs() < px(4.));
    assert!(
        bar.origin.y < tree.origin.y + px(8.),
        "pinned to the top of the list"
    );
}

#[test]
fn overrides_are_marked_and_copy_rust_writes_the_builder_calls() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    select(&mut harness, elements.send);
    harness.assert_text_visible("1 overridden");
    harness.assert_text_visible("background");
    harness.assert_text_visible("#2da44e");
    harness.assert_text_visible(".bg(rgb(0x2da44e))");
    harness.screenshot("elements-style-override");

    harness.click_text("Copy Rust");
    assert_eq!(clipboard(&mut harness), ".bg(rgb(0x2da44e))");
    harness.click_text("Copy patch");
    assert_eq!(clipboard(&mut harness), "+ .bg(rgb(0x2da44e))");

    harness.click_text("Revert all");
    assert!(harness.capture(|capture| capture.overrides().get(&elements.send.path).is_none()));
    assert!(harness.find_text("1 overridden").is_none());
}

#[test]
fn sections_collapse_and_expand() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    select(&mut harness, elements.row_3);
    harness.click_selector("elements-section-why-size");
    assert!(
        harness
            .find_text("Fixed: the style sets its height to 64.")
            .is_none()
    );
    harness.click_selector("elements-section-box-model");
    assert!(harness.find_text("596×40").is_none());
    harness.screenshot("elements-collapsed");
    harness.click_selector("elements-section-why-size");
    harness.assert_text_visible("Fixed: the style sets its height to 64.");
}

fn clipboard(harness: &mut LoupeHarness) -> String {
    harness
        .app(|cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .expect("something was copied")
}

#[test]
fn a_five_thousand_element_tree_stays_responsive() {
    let mut harness = LoupeHarness::new(size(px(1280.), px(800.)), |_, cx| cx.new(|_| InboxApp));
    harness.open_loupe();
    let mut capture = gpui::inspector::InspectorCapture::new_for_test();
    let mut builder = TreeBuilder::new(&mut capture);
    builder.open(ElementSpec::view("app::Root", 1).bounds(0., 0., 720., 800.));
    builder.open(ElementSpec::div().id("list").bounds(0., 0., 720., 800.));
    for row in 0..1_000 {
        let y = row as f32 * 24.;
        builder.open(
            ElementSpec::div()
                .id(format!("row-{row}"))
                .bounds(0., y, 720., 24.),
        );
        builder.leaf(ElementSpec::component("app::Avatar").bounds(4., y, 16., 16.));
        builder.leaf(ElementSpec::text(format!("Sender {row}")).bounds(24., y, 200., 16.));
        builder.leaf(ElementSpec::text("Subject").bounds(240., y, 300., 16.));
        builder.leaf(
            ElementSpec::div()
                .id("star")
                .clickable()
                .bounds(700., y, 16., 16.),
        );
        builder.close();
    }
    builder.close();
    builder.close();
    let tree = Arc::new(builder.build(0));
    assert!(tree.elements.len() >= 5_000);
    FrameBuilder::new()
        .app_time(ms(6.), ms(0.4))
        .cause(CauseKind::Animation, false)
        .tree(tree)
        .push(&mut capture);
    harness.install_capture(capture);
    harness.set_appearance(Appearance::Dark);
    harness.assert_text_visible("5,002");

    // Expanding the list shows its first rows only (the tree is virtual).
    harness.click_text("div#list");
    harness.type_keys("right");
    harness.assert_text_visible("div#row-0");

    // Every keystroke refilters 5,000 elements and redraws Loupe.
    harness.click_text("Filter elements");
    let start = Instant::now();
    harness.type_text("row-999");
    let per_keystroke = start.elapsed() / 7;
    harness.assert_text_visible("1 of 5,002");
    harness.assert_text_visible("div#row-999");
    harness.screenshot("elements-5k-filtered");
    eprintln!("5k tree: {per_keystroke:?} per filter keystroke (debug build)");
    assert!(
        per_keystroke < Duration::from_millis(250),
        "{per_keystroke:?} per keystroke"
    );
}

// ---------------------------------------------------------------------------
// A live app.

thread_local! {
    /// The lines where the live app builds `div#card` and `div#save`: the
    /// source locations Loupe must link to.
    static SITES: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

struct Profile;

impl Render for Profile {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let card_line = line!() + 1;
        let card = div()
            .id("card")
            .p(px(8.))
            .bg(rgb(0xf6f8fa))
            .text_color(rgb(0x1f2328))
            .child("Hello");
        let save_line = line!() + 1;
        let save = div()
            .id("save")
            .w(px(120.))
            .h(px(32.))
            .bg(rgb(0x2255ee))
            .hover(|style| style.bg(rgb(0xee3322)))
            .text_color(rgb(0xffffff))
            .child("Save");
        SITES.set((card_line, save_line));
        div()
            .id("app")
            .size_full()
            .flex()
            .flex_col()
            .p(px(16.))
            .gap(px(8.))
            .bg(rgb(0xffffff))
            .font_family(gpui_inspector::UI_FONT)
            .child(card)
            .child(save)
    }
}

fn live_harness() -> LoupeHarness {
    let mut harness = LoupeHarness::new(size(px(1200.), px(800.)), |_, cx| cx.new(|_| Profile));
    harness.open_loupe();
    harness.set_appearance(Appearance::Dark);
    settle(&mut harness);
    harness
}

/// Lets the app draw and Loupe catch up (a few refresh ticks).
fn settle(harness: &mut LoupeHarness) {
    for _ in 0..3 {
        harness.advance(REFRESH_INTERVAL);
    }
}

/// The latest tree's record with element id `id`.
fn live_record(harness: &mut LoupeHarness, id: &str) -> gpui::inspector::ElementRecord {
    harness.capture(|capture| {
        capture
            .latest_tree()
            .unwrap()
            .elements
            .iter()
            .find(|record| record.id.as_deref() == Some(id))
            .cloned()
            .unwrap_or_else(|| panic!("no element #{id}"))
    })
}

#[test]
fn picking_an_element_selects_it_with_its_real_source_and_size() {
    let mut harness = live_harness();
    harness.click_text("Start picking");
    assert!(harness.capture(|capture| capture.pick().active));
    let save = live_record(&mut harness, "save");
    harness.click(save.bounds.center());
    settle(&mut harness);
    assert!(!harness.capture(|capture| capture.pick().active));
    assert_eq!(selected(&mut harness), save.key);

    // The tree reveals it; the detail links its real construction site.
    harness.assert_text_visible("div#save");
    let save_line = SITES.get().1;
    harness.assert_text_visible(&format!("lens_elements.rs:{save_line}"));
    harness.assert_text_visible("Fixed: the style sets its width to 120.");
    harness.assert_text_visible("Fixed: the style sets its height to 32.");
    // The element's own style, from its code.
    harness.assert_text_visible("size.width");
    harness.assert_text_visible("#2255ee");
    harness.screenshot("elements-live-picked");
}

#[test]
fn scrubbing_padding_relayouts_the_app_and_copy_rust_writes_the_patch() {
    let mut harness = live_harness();
    let card = live_record(&mut harness, "card");
    harness.click_text("div#card");
    settle(&mut harness);
    assert_eq!(selected(&mut harness), card.key);
    let card_line = SITES.get().0;
    harness.assert_text_visible(&format!("lens_elements.rs:{card_line}"));

    // Drag the padding-top value 8 px right: +4 steps.
    let field = harness.bounds_of("elements-box-padding-top").center();
    harness.drag(field, point(field.x + px(8.), field.y), 4);
    settle(&mut harness);
    let edited = live_record(&mut harness, "card");
    assert_eq!(
        edited.bounds.size.height,
        card.bounds.size.height + px(4.),
        "the app relaid out with 12 px of top padding"
    );
    assert!(edited.flags.contains(ElementFlags::OVERRIDDEN));
    harness.assert_text_visible("1 overridden");
    harness.assert_text_visible("padding.top");
    harness.screenshot("elements-live-scrubbed");

    harness.click_text("Copy Rust");
    assert_eq!(clipboard(&mut harness), ".pt(px(12.))");

    harness.click_text("Revert all");
    settle(&mut harness);
    let reverted = live_record(&mut harness, "card");
    assert_eq!(reverted.bounds, card.bounds);
    assert!(!reverted.flags.contains(ElementFlags::OVERRIDDEN));
}

#[test]
fn adding_properties_and_typing_colors_restyle_the_app() {
    let mut harness = live_harness();
    let card = live_record(&mut harness, "card");
    harness.click_text("div#card");
    settle(&mut harness);

    // Add a property by name: it starts at a sensible value, overridden.
    harness.click_text("Add property");
    harness.type_text("margin.top");
    harness.screenshot("elements-live-add-property");
    harness.type_keys("enter");
    settle(&mut harness);
    let moved = live_record(&mut harness, "card");
    assert_eq!(
        moved.bounds.origin.y,
        card.bounds.origin.y + px(8.),
        "margin.top 8 px pushed the card down"
    );
    harness.assert_text_visible("margin.top");

    // Type a color: the app repaints with it as soon as the hex is valid.
    let field = harness.find_text("#f6f8fa").expect("the background's hex");
    harness.click(field.center());
    harness.type_keys("secondary-a");
    harness.type_text("#ffe0b2");
    settle(&mut harness);
    let painted = harness.update(|window, _| {
        let app_right = f32::from(window.app_bounds().right()) * window.scale_factor();
        window.painted_quads().iter().any(|quad| {
            quad.bounds.origin.x.as_f32() < app_right
                && quad.background.as_solid() == Some(rgb_to_hsla(rgb(0xffe0b2)))
        })
    });
    assert!(painted, "the app paints the typed color");
    harness.assert_text_visible("2 overridden");
    harness.screenshot("elements-live-color");

    // Reverting one property keeps the other.
    harness.click_selector("revert/margin/top");
    settle(&mut harness);
    assert_eq!(
        live_record(&mut harness, "card").bounds.origin,
        card.bounds.origin
    );
    harness.assert_text_visible("1 overridden");
}

#[test]
fn enums_cycle_through_their_options() {
    let mut harness = live_harness();
    let save = live_record(&mut harness, "save");
    harness.click_text("div#app");
    settle(&mut harness);
    harness.assert_text_visible("flex_direction");
    // Column → RowReverse: the card and the button now share a row.
    harness.click_selector("enum/flex_direction");
    settle(&mut harness);
    let card = live_record(&mut harness, "card");
    let moved = live_record(&mut harness, "save");
    assert_eq!(moved.bounds.origin.y, card.bounds.origin.y);
    assert_ne!(moved.bounds.origin, save.bounds.origin);
    harness.assert_text_visible("RowReverse");
    harness.screenshot("elements-live-enum");
}

#[test]
fn forcing_hover_paints_the_hover_style() {
    let mut harness = live_harness();
    harness.click_text("div#save");
    settle(&mut harness);
    // Loupe's own swatches paint these colors too: only count the app's quads.
    let painted = |harness: &mut LoupeHarness, color: u32| {
        harness.update(|window, _| {
            let app_right = f32::from(window.app_bounds().right()) * window.scale_factor();
            window.painted_quads().iter().any(|quad| {
                quad.bounds.origin.x.as_f32() < app_right
                    && quad.background.as_solid() == Some(rgb_to_hsla(rgb(color)))
            })
        })
    };
    assert!(painted(&mut harness, 0x2255ee));
    assert!(!painted(&mut harness, 0xee3322));

    harness.click_selector("elements-force-hover");
    settle(&mut harness);
    assert!(
        painted(&mut harness, 0xee3322),
        "the .hover() color is painted"
    );
    assert!(!painted(&mut harness, 0x2255ee));
    harness.screenshot("elements-live-forced-hover");

    harness.click_selector("elements-force-hover");
    settle(&mut harness);
    assert!(painted(&mut harness, 0x2255ee));
}

#[test]
fn text_without_a_source_location_is_selectable_with_its_contrast() {
    let mut harness = live_harness();
    let card = live_record(&mut harness, "card");
    harness.assert_text_visible("\"Hello\"");
    harness.click_text("\"Hello\"");
    settle(&mut harness);
    assert_eq!(
        selected(&mut harness),
        card.key,
        "the shared selection is its keyed parent"
    );
    harness.assert_text_visible("Text from a string");
    harness.assert_text_visible("Contrast");
    harness.assert_text_visible("AA");
    harness.screenshot("elements-live-text");
}

#[test]
fn a_lens_switched_away_from_leaves_nothing_over_the_app() {
    let mut harness = live_harness();
    // Loupe has the keyboard, as it does once the user clicked in it.
    harness.click_text("Elements");
    let bare = overlay_quads(&mut harness);

    // A hovered row outlines its element in the app...
    let card = live_record(&mut harness, "card");
    let row = harness.find_text("div#card").expect("the card's row");
    harness.hover(row.center());
    assert_eq!(
        harness.capture(|capture| capture.overlay().hovered()),
        card.key
    );
    assert_ne!(overlay_quads(&mut harness), bare, "the hover box shows");
    // ...until another lens shows: the next frame paints the app bare.
    harness.type_keys("alt-2");
    assert_eq!(harness.state(|state| state.lens()), Lens::Frames);
    assert_eq!(harness.capture(|capture| capture.overlay().hovered()), None);
    assert_eq!(overlay_quads(&mut harness), bare);

    // Back in Elements, the same row outlines its element again.
    harness.type_keys("alt-1");
    harness.hover(row.center());
    assert_eq!(
        harness.capture(|capture| capture.overlay().hovered()),
        card.key
    );

    // A row without a key highlights its element instead; that goes too.
    let row = harness.find_text("\"Hello\"").expect("the text's row");
    harness.hover(row.center());
    let highlights = |harness: &mut LoupeHarness| {
        harness.capture(|capture| capture.overlay().highlights().count())
    };
    assert_eq!(highlights(&mut harness), 1);
    assert_ne!(overlay_quads(&mut harness), bare, "the highlight shows");
    harness.type_keys("alt-2");
    assert_eq!(highlights(&mut harness), 0);
    assert_eq!(overlay_quads(&mut harness), bare);
}

/// The quads the last frame painted over the app, left of the dock: the
/// app's own and the overlay's.
fn overlay_quads(harness: &mut LoupeHarness) -> Vec<Bounds<ScaledPixels>> {
    harness.update(|window, _| {
        let app_right = f32::from(window.app_bounds().right()) * window.scale_factor();
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.bounds.origin.x.as_f32() < app_right)
            .map(|quad| quad.bounds)
            .collect()
    })
}

#[test]
fn your_code_keeps_the_apps_own_views_and_elements() {
    let mut harness = live_harness();
    harness.click_text("Your code");
    for text in ["Profile", "div#app", "div#card", "div#save"] {
        harness.assert_text_visible(text);
    }
    harness.screenshot("elements-live-your-code");
}
