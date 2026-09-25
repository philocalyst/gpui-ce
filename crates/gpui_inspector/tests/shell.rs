//! End-to-end tests of Loupe's shell, rendered headless against the Inbox
//! fixture with real input. Every test saves screenshots to
//! `target/loupe-shots/` for review.

use gpui::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div,
    inspector::{CauseKind, InspectorDock, OverlayModes},
    point,
    prelude::FluentBuilder as _,
    px, rgb, size,
};
use gpui_inspector::{
    Lens, LensLayout, LoupeSettings, REFRESH_INTERVAL,
    fixtures::{self, FrameBuilder, InboxElements, ms},
    harness::LoupeHarness,
    theme::{Appearance, Density},
};
use std::time::Duration;

/// A small stand-in for the inspected app, so screenshots show both sides.
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
            .bg(rgb(0xffffff))
            .font_family(gpui_inspector::UI_FONT)
            .child(
                div()
                    .w(px(200.))
                    .h_full()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .bg(rgb(0xf6f8fa))
                    .border_r_1()
                    .border_color(rgb(0xd0d7de))
                    .text_sm()
                    .text_color(rgb(0x1f2328))
                    .child("Inbox")
                    .child("Starred")
                    .child("Snoozed")
                    .child("Sent"),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(row("Grace Hopper", "Re: flaky layout test on CI", false))
                    .child(row("Alan Turing", "Release notes for 0.2.3", false))
                    .child(row(
                        "Katherine Johnson",
                        "Scroll jank in the issue list",
                        false,
                    ))
                    .child(row("Edsger Dijkstra", "Design review: new sidebar", true))
                    .child(row("Barbara Liskov", "Weekly sync notes", false)),
            )
    }
}

/// A `width`×`height` window with Loupe open over the Inbox fixture.
fn inbox_harness(width: f32, height: f32) -> (LoupeHarness, InboxElements) {
    let mut harness = LoupeHarness::new(size(px(width), px(height)), |_, cx| cx.new(|_| InboxApp));
    harness.open_loupe();
    let (capture, elements) = fixtures::inbox();
    harness.install_capture(capture);
    harness.set_appearance(Appearance::Dark);
    (harness, elements)
}

#[test]
fn shell_renders_every_surface_in_both_themes() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    harness.screenshot("shell-right-dark");
    for text in ["Pick", "Freeze", "Find anything…", "fps", "p95"] {
        harness.assert_text_visible(text);
    }
    for lens in Lens::ALL {
        harness.assert_text_visible(lens.label());
    }
    // Live counts in the rail: elements in the latest tree, app input records.
    let (elements, input) = harness.capture(|capture| {
        (
            capture.latest_tree().unwrap().elements.len(),
            capture
                .input()
                .iter()
                .filter(|record| !record.inspector)
                .count(),
        )
    });
    harness.assert_text_visible(&elements.to_string());
    harness.assert_text_visible(&input.to_string());
    for text in ["No element selected", "app", "loupe", "mem"] {
        harness.assert_text_visible(text);
    }
    let pick = harness.find_text("Pick").unwrap();
    assert!(pick.origin.x >= px(720.), "the toolbar lives in the dock");

    harness.set_appearance(Appearance::Light);
    harness.screenshot("shell-right-light");
    harness.assert_text_visible("Freeze");

    // Comfortable density grows rows and controls by 4 px.
    let compact_rail = harness.find_text("Events").unwrap();
    harness.app(|cx| {
        cx.set_global(LoupeSettings {
            appearance: Appearance::Dark,
            density: Density::Comfortable,
        })
    });
    harness.draw();
    let comfortable_rail = harness.find_text("Events").unwrap();
    assert!(comfortable_rail.origin.y > compact_rail.origin.y + px(8.));
    harness.screenshot("shell-comfortable");
}

#[test]
fn toolbar_toggles_overlays_freeze_pick_and_dock() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    let modes = |harness: &mut LoupeHarness| harness.capture(|capture| capture.overlay().modes);
    assert_eq!(modes(&mut harness), OverlayModes::BOX_MODEL);

    harness.click_selector("loupe-overlay-outlines");
    harness.click_selector("loupe-overlay-hitboxes");
    harness.click_selector("loupe-overlay-box-model");
    assert_eq!(
        modes(&mut harness),
        OverlayModes::OUTLINES | OverlayModes::HITBOXES
    );

    harness.click_text("Freeze");
    assert!(harness.capture(|capture| capture.is_frozen()));
    harness.assert_text_visible("FROZEN");

    harness.click_text("Pick");
    assert!(harness.capture(|capture| capture.pick().active));
    harness.screenshot("toolbar-toggled");

    harness.type_keys("escape");
    assert!(
        !harness.capture(|capture| capture.pick().active),
        "escape stops picking"
    );
    harness.click_text("Freeze");
    assert!(!harness.capture(|capture| capture.is_frozen()));

    harness.click_selector("loupe-dock");
    assert!(matches!(
        harness.capture(|capture| capture.dock()),
        InspectorDock::Bottom { .. }
    ));
    harness.click_selector("loupe-dock");
    assert!(matches!(
        harness.capture(|capture| capture.dock()),
        InspectorDock::Right { .. }
    ));
}

#[test]
fn hovering_and_clicking_a_pulse_bar_selects_that_frame() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    let spike = harness.capture(|capture| {
        capture
            .frames()
            .iter()
            .filter(|frame| !frame.inspector_only)
            .max_by_key(|frame| frame.timings.app_total())
            .map(|frame| frame.id)
            .unwrap()
    });
    let bar = harness.pulse_bar(spike);
    harness.hover(bar);
    harness.assert_text_visible(&format!("#{spike} · "));
    harness.assert_text_visible("IssueStore notified");
    harness.screenshot("pulse-hover");

    harness.click_pulse_frame(spike);
    assert_eq!(harness.state(|state| state.selected_frame()), Some(spike));
    assert_eq!(harness.state(|state| state.lens()), Lens::Frames);
    harness.hover(point(px(100.), px(400.)));
    harness.assert_text_visible("SELECTED FRAME");
    harness.assert_text_visible(&format!("#{spike}"));
    harness.screenshot("pulse-selected-frame");
}

#[test]
fn rail_switches_lenses_by_click_and_alt_keys() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    harness.click_text("Events");
    assert_eq!(harness.state(|state| state.lens()), Lens::Events);
    harness.assert_text_visible("Where did my click go? What will this key do here?");
    harness.screenshot("lens-events");

    for (keys, lens, shot) in [
        ("alt-2", Lens::Frames, "lens-frames"),
        ("alt-4", Lens::Entities, "lens-entities"),
        ("alt-5", Lens::Audit, "lens-audit"),
        ("alt-1", Lens::Elements, "lens-elements"),
    ] {
        harness.type_keys(keys);
        assert_eq!(harness.state(|state| state.lens()), lens, "{keys}");
        harness.assert_text_visible(lens.question());
        harness.screenshot(shot);
    }
}

#[test]
fn palette_filters_and_runs_commands_and_finds_elements() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    harness.click_text("Elements");
    harness.type_keys("secondary-k");
    let loupe = harness.loupe();
    assert!(harness.app(|cx| loupe.read(cx).is_palette_open()));
    harness.assert_text_visible("Pick an element");
    harness.screenshot("palette-open");

    harness.type_text("frz");
    let matches = harness.app(|cx| loupe.read(cx).palette_matches(cx));
    assert_eq!(
        matches.first().map(|m| m.as_ref()),
        Some("Freeze recording")
    );
    harness.screenshot("palette-filtered");
    harness.type_keys("enter");
    assert!(!harness.app(|cx| loupe.read(cx).is_palette_open()));
    assert!(harness.capture(|capture| capture.is_frozen()));

    harness.type_keys("secondary-k");
    harness.type_text("#row3");
    let matches = harness.app(|cx| loupe.read(cx).palette_matches(cx));
    assert_eq!(matches.first().map(|m| m.as_ref()), Some("div#row-3"));
    harness.screenshot("palette-elements");
    harness.type_keys("enter");
    assert_eq!(
        harness.state(|state| state.selected_element()),
        Some(elements.row_3)
    );

    harness.type_keys("secondary-k");
    harness.type_text("@store");
    let matches = harness.app(|cx| loupe.read(cx).palette_matches(cx));
    assert_eq!(matches.first().map(|m| m.as_ref()), Some("IssueStore"));
    harness.screenshot("palette-entities");
    harness.type_keys("escape");
    assert!(!harness.app(|cx| loupe.read(cx).is_palette_open()));
}

#[test]
fn status_bar_shows_breadcrumb_timings_and_memory() {
    let (mut harness, elements) = inbox_harness(1280., 800.);
    harness.update_state(|state, cx| state.select_element(Some(elements.row_3), cx));
    for text in [
        "InboxApp",
        "IssueList",
        "IssueRow",
        "div#row-3",
        "app",
        "loupe",
        "mem",
        "MB",
    ] {
        harness.assert_text_visible(text);
    }
    harness.assert_text_visible("SELECTION");
    harness.screenshot("status-breadcrumb");
    assert_eq!(
        harness.capture(|capture| capture.overlay().selected),
        Some(elements.row_3),
        "the overlay outlines the selection"
    );

    // The breadcrumb is the lowest "IssueList" on screen (the lens also names it).
    let crumb = harness
        .painted_text()
        .into_iter()
        .filter(|line| line.text.as_ref() == "IssueList")
        .map(|line| line.visible_bounds())
        .max_by(|a, b| f32::from(a.origin.y).total_cmp(&f32::from(b.origin.y)))
        .unwrap();
    harness.click(crumb.center());
    assert_eq!(
        harness.state(|state| state.selected_element()),
        Some(elements.issue_list)
    );
}

#[test]
fn bottom_docks_lay_lenses_out_wide_and_narrow_docks_stack() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    harness.update(|window, _| {
        window
            .inspector_capture_mut()
            .unwrap()
            .set_dock(InspectorDock::Bottom { height: px(340.) })
    });
    harness.redraw_all();
    assert_eq!(
        harness.update(|window, _| LensLayout::of(window)),
        LensLayout::SideBySide
    );
    harness.assert_text_visible("Find anything…");
    harness.screenshot("dock-bottom");

    let (mut narrow, _) = inbox_harness(900., 700.);
    narrow.update(|window, _| {
        window
            .inspector_capture_mut()
            .unwrap()
            .set_dock(InspectorDock::Right { width: px(400.) })
    });
    narrow.redraw_all();
    assert_eq!(
        narrow.update(|window, _| LensLayout::of(window)),
        LensLayout::Stacked
    );
    assert!(
        narrow.find_text("Pick").is_none(),
        "narrow docks drop the toolbar labels"
    );
    narrow.assert_text_visible("Elements");
    narrow.screenshot("dock-narrow");
}

#[test]
fn dragging_the_dock_edge_resizes_the_dock() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    let edge = harness.bounds_of("loupe-dock-edge");
    let start = edge.center();
    harness.drag(start, point(start.x - px(100.), start.y), 5);
    let InspectorDock::Right { width } = harness.capture(|capture| capture.dock()) else {
        panic!("still docked right");
    };
    assert!((width - px(660.)).abs() <= px(1.), "{width:?}");
    harness.screenshot("dock-resized");

    // The app keeps its minimum width.
    let edge = harness.bounds_of("loupe-dock-edge").center();
    harness.drag(edge, point(px(10.), edge.y), 3);
    let InspectorDock::Right { width } = harness.capture(|capture| capture.dock()) else {
        panic!("still docked right");
    };
    assert_eq!(
        width,
        px(1088.),
        "85% of the window, as the dock split allows"
    );
}

#[test]
fn the_refresh_loop_stays_idle_until_the_app_records_new_data() {
    let (mut harness, _) = inbox_harness(1280., 800.);
    let loupe = harness.loupe();
    let renders = |harness: &mut LoupeHarness| harness.app(|cx| loupe.read(cx).render_count());
    let before = renders(&mut harness);

    // A second of timer ticks with nothing new: no re-render.
    harness.advance(Duration::from_secs(1));
    assert_eq!(renders(&mut harness), before);

    // Frames caused only by Loupe itself do not wake it either.
    harness.update(|window, _| {
        let capture = window.inspector_capture_mut().unwrap();
        FrameBuilder::new()
            .at(ms(20_000.))
            .app_time(ms(0.8), ms(0.4))
            .cause(CauseKind::Input { event: "MouseMove" }, true)
            .inspector_only()
            .push(capture);
    });
    harness.advance(Duration::from_secs(1));
    assert_eq!(renders(&mut harness), before);

    // A new app frame is picked up within one tick, exactly once.
    harness.update(|window, _| {
        let capture = window.inspector_capture_mut().unwrap();
        FrameBuilder::new()
            .at(ms(20_100.))
            .app_time(ms(45.), ms(0.4))
            .cause(CauseKind::Animation, false)
            .push(capture);
    });
    harness.advance(REFRESH_INTERVAL);
    assert_eq!(renders(&mut harness), before + 1);
    harness.advance(Duration::from_secs(1));
    assert_eq!(renders(&mut harness), before + 1);
}
