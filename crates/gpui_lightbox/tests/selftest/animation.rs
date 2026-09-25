//! Films: exact sampling on the fake clock, deterministic frames, tracked
//! curves, detectors and assertions.

use crate::views::{
    JANK, Janky, PANEL_CLOSED, PANEL_OPEN, PANEL_WIDTH, SLIDE, SLIDE_BY, SLIDE_FROM, Slider,
    SpringPanel, janky,
};
use crate::{ANIMATIONS, CATCHES};
use gpui::{AppContext as _, ease_in_out, linear};
use gpui_lightbox::{FilmSpec, Shot, Stage, StageConfig, manifest::FindingKind, theme::LIGHT};
use std::{
    io::Cursor,
    panic::{AssertUnwindSafe, catch_unwind},
    time::Duration,
};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn slider_stage(suite: crate::SuiteName, easing: fn(f32) -> f32) -> Stage {
    let mut stage = crate::stage(suite, StageConfig::default().size(320., 112.));
    stage.mount(move |_, cx| cx.new(|_| Slider::new(easing)));
    stage
}

/// The bar's left edge, found by its accent fill.
fn bar_x(shot: &Shot) -> Option<f32> {
    shot.quad_filled(LIGHT.accent).map(|quad| quad.bounds.x)
}

fn panel_x(shot: &Shot) -> Option<f32> {
    shot.quad(|quad| quad.bounds.w == PANEL_WIDTH && quad.background.is_some())
        .map(|quad| quad.bounds.x)
}

fn png_bytes(shot: &Shot) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    shot.image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encoding a frame");
    bytes.into_inner()
}

#[test]
fn tracked_curve_matches_the_easing_function() {
    let mut stage = slider_stage(ANIMATIONS, ease_in_out);
    let mut film = stage.film("slide-ease-in-out", FilmSpec::fps(ms(360), 30.), |stage| {
        stage.click_text("Slide");
    });
    film.track("x", bar_x);
    let expected =
        |t: f32| SLIDE_FROM + SLIDE_BY * ease_in_out((t / SLIDE.as_millis() as f32).min(1.));
    // Layout lands on device pixels, so a sample is within half a device pixel.
    film.assert_follows("x", expected, 0.25)
        .assert_monotonic("x")
        .assert_no_jumps("x")
        .assert_no_overshoot("x")
        .assert_settles_by(SLIDE);
    assert_eq!(film.values("x")[0], Some(SLIDE_FROM));
    assert_eq!(
        film.values("x").last().copied().flatten(),
        Some(SLIDE_FROM + SLIDE_BY)
    );
    assert!(film.findings().is_empty(), "{:?}", film.findings());

    // A linear slide is linear, and doesn't match the ease-in-out curve.
    let mut stage = slider_stage(CATCHES, linear);
    let mut film = stage.film(
        "slide-linear-is-not-ease-in-out",
        FilmSpec::fps(ms(360), 30.),
        |stage| {
            stage.click_text("Slide");
        },
    );
    film.track("x", bar_x);
    let linear_expected = |t: f32| SLIDE_FROM + SLIDE_BY * (t / SLIDE.as_millis() as f32).min(1.);
    film.assert_follows("x", linear_expected, 0.25);
    let mismatch = catch_unwind(AssertUnwindSafe(|| {
        film.assert_follows("x", expected, 0.25);
    }));
    assert!(
        mismatch.is_err(),
        "a linear slide doesn't follow ease-in-out"
    );
}

#[test]
fn frames_are_byte_identical_across_runs() {
    let run = |name: &str| {
        let mut stage = slider_stage(ANIMATIONS, ease_in_out);
        let film = stage.film(name, FilmSpec::frames(ms(300), 7), |stage| {
            stage.click_text("Slide");
        });
        film.frames().iter().map(png_bytes).collect::<Vec<_>>()
    };
    let first = run("determinism-a");
    let second = run("determinism-b");
    assert_eq!(first.len(), 7);
    for (ix, (a, b)) in first.iter().zip(&second).enumerate() {
        assert!(a == b, "frame {ix} differs between runs");
    }
    assert!(first[1] != first[2], "the frames do change over time");
}

#[test]
fn springs_overshoot_or_settle_smoothly() {
    let mut stage = crate::stage(ANIMATIONS, StageConfig::default().size(320., 216.));
    stage.mount(|_, cx| cx.new(|_| SpringPanel::bouncy()));
    let mut bouncy = stage.film("spring-bouncy", FilmSpec::fps(ms(900), 30.), |stage| {
        stage.click_text("Open");
    });
    bouncy.track("x", panel_x);
    assert_eq!(
        bouncy.values("x")[0],
        None,
        "off screen at -260 px, so not painted"
    );
    assert!(bouncy.values("x")[1].is_some_and(|x| x > PANEL_CLOSED));
    let kinds = bouncy
        .findings()
        .iter()
        .map(|finding| finding.kind)
        .collect::<Vec<_>>();
    assert!(kinds.contains(&FindingKind::Overshoot), "{kinds:?}");
    assert!(kinds.contains(&FindingKind::NonMonotonic), "{kinds:?}");
    bouncy.assert_no_jumps("x").assert_no_freezes();
    drop(bouncy);

    let mut stage = crate::stage(ANIMATIONS, StageConfig::default().size(320., 216.));
    stage.mount(|_, cx| cx.new(|_| SpringPanel::smooth()));
    let mut smooth = stage.film("spring-smooth", FilmSpec::fps(ms(900), 30.), |stage| {
        stage.click_text("Open");
    });
    smooth
        .track("x", panel_x)
        .assert_monotonic("x")
        .assert_no_overshoot("x")
        .assert_no_jumps("x")
        .assert_settles_by(ms(800));
    assert_eq!(
        smooth.values("x").last().copied().flatten(),
        Some(PANEL_OPEN)
    );
}

#[test]
fn detectors_catch_a_janky_animation() {
    let mut stage = crate::stage(CATCHES, StageConfig::default().size(320., 72.));
    stage.mount(|_, cx| cx.new(|_| Janky));
    let mut film = stage.film("janky", FilmSpec::fps(JANK, 30.), |_| {});
    film.track("x", bar_x);
    let findings = film.findings();
    let frame_of = |kind| {
        findings
            .iter()
            .find(|finding| finding.kind == kind)
            .map(|finding| finding.frame)
    };
    // 30 fps over 400 ms: a frame every 33.3 ms. The stall holds frames 3–5
    // (100–167 ms) still; the teleport lands on frame 9 (300 ms).
    assert_eq!(frame_of(FindingKind::Frozen), Some(4), "{findings:#?}");
    assert_eq!(frame_of(FindingKind::Jump), Some(9), "{findings:#?}");
    assert_eq!(
        film.values("x")[3],
        Some(SLIDE_FROM + SLIDE_BY * janky(0.3))
    );

    let jump = catch_unwind(AssertUnwindSafe(|| {
        film.assert_no_jumps("x");
    }))
    .expect_err("the teleport is a jump");
    let message = jump.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("x jumped by"), "{message}");
    assert!(
        message.contains("janky.film.png"),
        "points at the strip: {message}"
    );
}

#[test]
fn settling_is_asserted_on_pixels() {
    let mut stage = slider_stage(CATCHES, ease_in_out);
    let mut film = stage.film("slide-settling", FilmSpec::fps(ms(400), 30.), |stage| {
        stage.click_text("Slide");
    });
    // Frames every 33.3 ms: the last change lands on the frame at 300 ms.
    assert_eq!(film.settled_at(), SLIDE);
    film.assert_settles_by(SLIDE);
    let early = catch_unwind(AssertUnwindSafe(|| {
        film.assert_settles_by(ms(200));
    }))
    .expect_err("still moving at 233 ms");
    let message = early.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("still moving at 233.33 ms"), "{message}");
}
