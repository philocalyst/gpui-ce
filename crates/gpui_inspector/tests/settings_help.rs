//! End-to-end tests of the settings popover, the keyboard shortcuts overlay
//! and the first-run empty states, rendered headless with real input.
//! Every test saves screenshots to `target/loupe-shots/` for review.

use gpui::{
    AppContext as _, Context, Hsla, IntoElement, ParentElement as _, Pixels, Point, Render,
    Styled as _, Window, div, hsla_to_rgba,
    inspector::{CaptureLevel, ElementKind, InspectorCapture, InspectorDock},
    point, px, rgb, size,
};
use gpui_inspector::{
    Editor, EditorUrl, FrameBudget, Lens, LoupeSettings, REFRESH_INTERVAL,
    analysis::stats::Grade,
    fixtures::{self, InboxElements},
    harness::LoupeHarness,
    theme::{Appearance, Density, Theme},
};

/// A stand-in for the inspected app, so screenshots show both sides.
struct App;

impl Render for App {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = |sender: &'static str, subject: &'static str| {
            div()
                .h(px(56.))
                .px_4()
                .flex()
                .flex_col()
                .justify_center()
                .border_b_1()
                .border_color(rgb(0xeaeef2))
                .child(div().text_sm().text_color(rgb(0x1f2328)).child(sender))
                .child(div().text_xs().text_color(rgb(0x656d76)).child(subject))
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .font_family(gpui_inspector::UI_FONT)
            .child(row("Grace Hopper", "Re: flaky layout test on CI"))
            .child(row("Alan Turing", "Release notes for 0.2.3"))
            .child(row("Katherine Johnson", "Scroll jank in the issue list"))
    }
}

/// A `width`×`height` window with Loupe open over the live app.
fn live(width: f32, height: f32) -> LoupeHarness {
    let mut harness = LoupeHarness::new(size(px(width), px(height)), |_, cx| cx.new(|_| App));
    harness.open_loupe();
    harness.set_appearance(Appearance::Dark);
    harness.advance(REFRESH_INTERVAL);
    harness
}

/// The same, over the Inbox fixture's recording.
fn inbox(width: f32, height: f32) -> (LoupeHarness, InboxElements) {
    let mut harness = live(width, height);
    let (capture, elements) = fixtures::inbox();
    harness.install_capture(capture);
    (harness, elements)
}

fn is_settings_open(harness: &mut LoupeHarness) -> bool {
    let loupe = harness.loupe();
    harness.app(|cx| loupe.read(cx).is_settings_open())
}

fn is_help_open(harness: &mut LoupeHarness) -> bool {
    let loupe = harness.loupe();
    harness.app(|cx| loupe.read(cx).is_help_open())
}

fn settings(harness: &mut LoupeHarness) -> LoupeSettings {
    harness.app(|cx| LoupeSettings::get(cx).clone())
}

/// Whether a painted pixel shows `color` (within rounding of the renderer's
/// color conversions).
fn shows(pixel: [u8; 4], color: Hsla) -> bool {
    let color = hsla_to_rgba(color);
    let expected = [color.red, color.green, color.blue, color.alpha];
    pixel
        .iter()
        .zip(expected)
        .all(|(painted, expected)| (f32::from(*painted) - expected * 255.).abs() <= 2.)
}

/// A point on the toolbar's empty surface, between the search field and
/// the settings button.
fn toolbar_surface(harness: &mut LoupeHarness) -> Point<Pixels> {
    let settings = harness.bounds_of("loupe-settings-button");
    point(settings.left() - px(3.), settings.center().y)
}

#[test]
fn the_gear_the_shortcut_and_the_palette_open_settings() {
    let (mut harness, _) = inbox(1280., 800.);
    harness.click_selector("loupe-settings-button");
    assert!(is_settings_open(&mut harness));
    for text in [
        "Settings",
        "APPEARANCE",
        "Theme",
        "Density",
        "RECORDING",
        "Budget",
        "Capture",
        "SOURCE LINKS",
        "Editor",
        "zed://file/home/me/app/src/main.rs:42:7",
    ] {
        harness.assert_text_visible(text);
    }
    for level in [CaptureLevel::Frames, CaptureLevel::Tree, CaptureLevel::Full] {
        harness.assert_text_visible(gpui_inspector::settings::capture_level_summary(level));
    }
    let popover = harness.bounds_of("loupe-settings");
    let gear = harness.bounds_of("loupe-settings-button");
    assert!(
        popover.top() > gear.bottom(),
        "the popover hangs below the gear"
    );
    assert!(popover.right() <= px(1280.) && popover.right() > gear.right() - px(40.));
    harness.screenshot("settings-dark");
    harness.set_appearance(Appearance::Light);
    harness.screenshot("settings-light");
    harness.set_appearance(Appearance::Dark);

    harness.type_keys("escape");
    assert!(!is_settings_open(&mut harness), "escape closes it");

    harness.type_keys("secondary-,");
    assert!(is_settings_open(&mut harness), "so does the shortcut");
    harness.type_keys("secondary-,");
    assert!(!is_settings_open(&mut harness), "which also closes it");

    harness.type_keys("secondary-k");
    harness.type_text(">settings");
    harness.type_keys("enter");
    assert!(is_settings_open(&mut harness), "the palette opens it too");

    // Clicking anywhere outside closes it, without clicking what is there.
    let lens = harness.state(|state| state.lens());
    harness.click_text("Frames");
    assert!(!is_settings_open(&mut harness));
    assert_eq!(harness.state(|state| state.lens()), lens);
}

#[test]
fn theme_and_density_restyle_loupe() {
    let (mut harness, _) = inbox(1280., 800.);
    harness.update_settings(|settings| settings.appearance = Appearance::System);
    let colors = |dark: bool| &Theme::get(dark, Density::Compact).colors;
    // Chooses a theme in the popover, then reads the toolbar without it
    // (and its shadow) in the way.
    let mut choose = |segment: &str| {
        harness.click_selector("loupe-settings-button");
        harness.click_selector(segment);
        harness.type_keys("escape");
        let surface = toolbar_surface(&mut harness);
        (settings(&mut harness).appearance, harness.pixel(surface))
    };
    for (segment, appearance, dark) in [
        ("loupe-settings-theme-1", Appearance::Dark, true),
        ("loupe-settings-theme-2", Appearance::Light, false),
    ] {
        let (chosen, pixel) = choose(segment);
        assert_eq!(chosen, appearance);
        assert!(
            shows(pixel, colors(dark).surface),
            "{appearance:?}: {pixel:?}"
        );
        assert!(!shows(pixel, colors(!dark).surface));
    }
    // Following the window: the headless window is light.
    let (chosen, pixel) = choose("loupe-settings-theme-0");
    assert_eq!(chosen, Appearance::System);
    assert!(shows(pixel, colors(false).surface), "{pixel:?}");
    choose("loupe-settings-theme-1");

    // Rows in every list grow by 4 px.
    harness.click_text("Events");
    assert_eq!(event_row_height(&mut harness), px(22.));
    harness.click_selector("loupe-settings-button");
    harness.click_selector("loupe-settings-density-1");
    assert_eq!(settings(&mut harness).density, Density::Comfortable);
    assert_eq!(event_row_height(&mut harness), px(26.));
    harness.screenshot("settings-comfortable");
    harness.click_selector("loupe-settings-density-0");
    assert_eq!(event_row_height(&mut harness), px(22.));
}

/// The height of a row in the Events log (whichever the log shows first).
fn event_row_height(harness: &mut LoupeHarness) -> Pixels {
    let seqs: Vec<u64> =
        harness.capture(|capture| capture.input().iter().map(|record| record.seq).collect());
    harness.redraw_all();
    seqs.iter()
        .find_map(|seq| {
            let row = format!("events-row-{seq}");
            harness.update(|window, _| window.debug_bounds(&row))
        })
        .expect("the log shows the fixture's input")
        .size
        .height
}

#[test]
fn the_editor_setting_drives_the_source_link() {
    let (mut harness, elements) = inbox(1280., 800.);
    harness.click_selector("loupe-settings-button");
    harness.click_selector("loupe-settings-editor-1");
    assert_eq!(
        settings(&mut harness).editor,
        EditorUrl::from(Editor::VsCode)
    );
    harness.assert_text_visible("vscode://file/home/me/app/src/main.rs:42:7");
    harness.click_selector("loupe-settings-editor-3");
    harness.assert_text_visible("idea://open?file=/home/me/app/src/main.rs&line=42&column=7");

    // A custom template starts from an example and previews as you type.
    harness.click_selector("loupe-settings-editor-4");
    harness.assert_text_visible("subl://open?url=file:///home/me/app/src/main.rs&line=42&column=7");
    harness.screenshot("settings-custom-editor");
    harness.type_keys("secondary-a");
    harness.type_text("ed://{path}:{line}");
    assert_eq!(
        settings(&mut harness).editor,
        EditorUrl::Custom("ed://{path}:{line}".into())
    );
    harness.assert_text_visible("ed:///home/me/app/src/main.rs:42");
    harness.type_keys("escape");
    assert!(
        !is_settings_open(&mut harness),
        "escape in the field closes too"
    );

    // The Elements lens opens sources with it.
    harness.update_state(|state, cx| state.select_element(Some(elements.close), cx));
    let location =
        harness.capture(|capture| capture.path_info(elements.close.path).unwrap().source);
    harness.click_selector("elements-source");
    let url = harness.opened_url().expect("the source link opened a URL");
    assert!(url.starts_with("ed:///"), "{url}");
    assert!(
        url.ends_with(&format!("fixtures.rs:{}", location.line())),
        "{url}"
    );

    // Reopened, the popover shows the custom template again.
    harness.click_selector("loupe-settings-button");
    harness.assert_text_visible("ed://{path}:{line}");
}

#[test]
fn the_budget_regrades_the_pulse_strip() {
    let (mut harness, _) = inbox(1280., 800.);
    // A frame within 60 Hz's budget but over 120 Hz's.
    let frame = harness.capture(|capture| {
        let frames: Vec<_> = capture.frames().iter().collect();
        frames[frames.len() - 100..]
            .iter()
            .filter(|frame| !frame.inspector_only)
            .find(|frame| {
                let total = frame.timings.app_total();
                Grade::of(total, FrameBudget::Hz60.duration()) == Grade::Ok
                    && Grade::of(total, FrameBudget::Hz120.duration()) == Grade::Warn
            })
            .map(|frame| frame.id)
            .expect("the fixture has a frame between 8.3 and 12.5 ms")
    });
    // Just above the baseline, in the middle of the frame's bar.
    let strip = harness.bounds_of("loupe-pulse");
    let bar = point(harness.pulse_bar(frame).x, strip.bottom() - px(8.5));
    let warn = Theme::get(true, Density::Compact).colors.warn;
    assert!(!shows(harness.pixel(bar), warn));

    harness.click_selector("loupe-settings-button");
    harness.click_selector("loupe-settings-budget-1");
    assert_eq!(settings(&mut harness).budget, FrameBudget::Hz120);
    assert_eq!(
        harness.capture(|capture| capture.config().budget),
        FrameBudget::Hz120.duration()
    );
    harness.assert_text_visible("8.3 ms per frame");
    harness.screenshot("settings-budget-120");
    harness.type_keys("escape");
    assert!(shows(harness.pixel(bar), warn), "regraded");

    // The palette sets it too.
    harness.type_keys("secondary-k");
    harness.type_text(">budget 60");
    harness.type_keys("enter");
    assert_eq!(settings(&mut harness).budget, FrameBudget::Hz60);
    assert!(!shows(harness.pixel(bar), warn));
}

#[test]
fn the_capture_level_changes_what_the_next_frame_records() {
    let mut harness = live(1280., 800.);
    let next_frame = |harness: &mut LoupeHarness| {
        harness.update(|window, _| window.refresh());
        harness.advance(REFRESH_INTERVAL);
        harness.capture(|capture| {
            let frame = capture.latest_app_frame().expect("the app drew a frame");
            let tree = frame.tree.as_ref();
            let details = tree
                .is_some_and(|tree| tree.elements.iter().any(|record| record.details.is_some()));
            (frame.id, tree.is_some(), details)
        })
    };
    let (full, tree, details) = next_frame(&mut harness);
    assert!(tree && details, "Full records trees and details");

    harness.click_selector("loupe-settings-button");
    harness.click_selector("loupe-settings-level-frames");
    assert_eq!(settings(&mut harness).capture_level, CaptureLevel::Frames);
    assert_eq!(
        harness.capture(|capture| capture.config().level),
        CaptureLevel::Frames
    );
    harness.screenshot("settings-level-frames");
    let (frames, tree, _) = next_frame(&mut harness);
    assert!(frames > full);
    assert!(!tree, "Frames records no element tree");

    harness.click_selector("loupe-settings-level-tree");
    let (_, tree, details) = next_frame(&mut harness);
    assert!(tree && !details, "Tree records trees without details");

    // Loupe opened later, in any window, starts at the chosen level.
    harness.type_keys("escape");
    harness.update(|window, cx| window.toggle_inspector(cx));
    harness.open_loupe();
    assert_eq!(
        harness.capture(|capture| capture.config().level),
        CaptureLevel::Tree
    );
}

#[test]
fn defaults_come_from_init_with() {
    let mut harness = LoupeHarness::new(size(px(1100.), px(700.)), |_, cx| {
        gpui_inspector::init_with(
            cx,
            LoupeSettings {
                appearance: Appearance::Light,
                editor: Editor::Idea.into(),
                budget: FrameBudget::Hz144,
                capture_level: CaptureLevel::Tree,
                ..LoupeSettings::default()
            },
        );
        cx.new(|_| App)
    });
    harness.open_loupe();
    let config = harness.capture(|capture| (capture.config().budget, capture.config().level));
    assert_eq!(config, (FrameBudget::Hz144.duration(), CaptureLevel::Tree));
    harness.click_selector("loupe-settings-button");
    harness.assert_text_visible("idea://open?file=/home/me/app/src/main.rs&line=42&column=7");
    harness.assert_text_visible("6.9 ms per frame");
}

#[test]
fn question_mark_and_the_palette_show_every_key() {
    let (mut harness, _) = inbox(1280., 800.);
    harness.click_text("Elements");
    harness.type_keys("?");
    assert!(is_help_open(&mut harness));
    for text in [
        "Keyboard shortcuts",
        "LENSES",
        "GLOBAL",
        "LOUPE",
        "WHILE PICKING",
    ] {
        harness.assert_text_visible(text);
    }
    for lens in Lens::ALL {
        harness.assert_text_visible(lens.question());
    }
    for description in [
        "Opens or closes Loupe in the active window",
        "Opens or closes the command palette",
        "Shows or hides the keyboard shortcuts",
        "Picks the enclosing element (or scroll up)",
        "Selects the next row",
    ] {
        harness.assert_text_visible(description);
    }
    harness.screenshot("help-dark");
    harness.set_appearance(Appearance::Light);
    harness.screenshot("help-light");
    harness.set_appearance(Appearance::Dark);

    // The rest is a scroll away.
    let help = harness.bounds_of("loupe-help");
    harness.scroll(help.center(), point(px(0.), px(-2000.)));
    for description in [
        "Shows the slowest app frame in the recording",
        "Shows the selected finding's element in Elements",
    ] {
        harness.assert_text_visible(description);
    }
    harness.screenshot("help-scrolled");

    harness.type_keys("escape");
    assert!(!is_help_open(&mut harness), "escape closes it");
    harness.type_keys("?");
    harness.type_keys("?");
    assert!(!is_help_open(&mut harness), "so does ? again");

    harness.type_keys("secondary-k");
    harness.type_text(">keyboard");
    harness.type_keys("enter");
    assert!(is_help_open(&mut harness), "the palette opens it");
    // On the toolbar's Pick button, under the scrim: it only closes help.
    let pick = harness.bounds_of("loupe-pick").center();
    harness.click(pick);
    assert!(!is_help_open(&mut harness), "a click outside closes it");
    assert!(!harness.capture(|capture| capture.pick().active));

    // In a text field, ? is just a character.
    harness.type_keys("secondary-k");
    harness.type_text("?");
    assert!(!is_help_open(&mut harness));
    harness.type_keys("escape");
}

#[test]
fn help_and_settings_fit_narrow_and_wide_docks() {
    let (mut harness, _) = inbox(900., 700.);
    harness.update(|window, _| {
        window
            .inspector_capture_mut()
            .expect("Loupe is open")
            .set_dock(InspectorDock::Right { width: px(380.) })
    });
    harness.redraw_all();
    harness.click_text("Elements");
    harness.type_keys("?");
    let help = harness.bounds_of("loupe-help");
    let dock = harness.update(|window, _| window.inspector_bounds().unwrap());
    assert!(help.left() >= dock.left() && help.right() <= dock.right());
    harness.screenshot("help-narrow");

    harness.type_keys("escape");
    harness.click_selector("loupe-settings-button");
    let popover = harness.bounds_of("loupe-settings");
    assert!(popover.left() >= dock.left() && popover.right() <= dock.right());
    harness.screenshot("settings-narrow");

    // Docked at the bottom, the groups sit in two columns.
    let (mut wide, _) = inbox(1480., 900.);
    wide.update(|window, _| {
        window
            .inspector_capture_mut()
            .expect("Loupe is open")
            .set_dock(InspectorDock::Bottom { height: px(560.) })
    });
    wide.redraw_all();
    wide.click_text("Elements");
    wide.type_keys("?");
    let global = wide.find_text("GLOBAL").expect("the Global group shows");
    let frames = wide.find_text("FRAMES").expect("so does Frames");
    assert!(frames.left() > global.right(), "side by side");
    wide.screenshot("help-bottom-dock");
    wide.type_keys("escape");
    wide.click_selector("loupe-settings-button");
    wide.screenshot("settings-bottom-dock");
}

#[test]
fn an_empty_recording_says_what_to_do_in_every_lens() {
    let mut harness = live(1280., 800.);
    harness.install_capture(InspectorCapture::new_for_test());
    harness.click_text("Elements");
    for lens in Lens::ALL {
        harness.type_keys(&format!("alt-{}", lens.index() + 1));
        assert_eq!(harness.state(|state| state.lens()), lens);
        harness.screenshot(&format!("empty-{}", lens.label().to_lowercase()));
    }
}

#[test]
fn a_fresh_loupe_over_a_live_app_points_at_picking() {
    let mut harness = live(1280., 800.);
    let view = harness.capture(|capture| {
        capture.latest_tree().is_some_and(|tree| {
            tree.elements
                .iter()
                .any(|record| matches!(record.kind, ElementKind::View { .. }))
        })
    });
    assert!(
        view,
        "the live app's tree is recorded as soon as Loupe opens"
    );
    for text in [Lens::Elements.question(), "Start picking", "Hold app"] {
        harness.assert_text_visible(text);
    }
    harness.screenshot("fresh-live");
}
