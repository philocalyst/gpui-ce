//! Golden images: a match, a 1 px shift caught, sub-threshold noise ignored.
//!
//! `tests/golden/card.png` was recorded on Mesa lavapipe with
//! `LIGHTBOX_UPDATE=1 cargo test -p gpui_ce_lightbox --test selftest goldens`.

use crate::{CARD, CATCHES, card_config, card_stage, views::Flaw};
use gpui_lightbox::{
    GoldenTolerance,
    golden::{compare, golden_dir},
};

fn recorded_card() -> image::RgbaImage {
    image::open(golden_dir().join("card.png"))
        .expect("tests/golden/card.png is checked in")
        .into_rgba8()
}

#[test]
fn the_card_matches_its_golden() {
    let (mut stage, _) = card_stage(CARD, Flaw::None, card_config());
    stage.shot("golden").assert_golden("card");
}

#[test]
fn a_one_pixel_shift_fails_the_golden() {
    let (mut stage, _) = card_stage(CATCHES, Flaw::ShiftedOnePixel, card_config());
    let shot = stage.shot("shifted-one-pixel");
    let outcome = shot.golden_at(&golden_dir(), "card", GoldenTolerance::default(), false);
    assert!(!outcome.passed, "{}", outcome.message);
    assert!(outcome.stats.differing > 1_000, "{:?}", outcome.stats);
    // The difference is the card, not the (unchanged) window background.
    let bounds = outcome.stats.bounds.expect("where the pixels differ");
    assert!(bounds.x >= 32. && bounds.right() <= 394., "{bounds}");
    assert!(outcome.comparison.expect("a side-by-side").exists());
}

#[test]
fn noise_below_the_threshold_is_ignored() {
    let (mut stage, _) = card_stage(CARD, Flaw::None, card_config());
    let mut shot = stage.capture("golden-with-noise");
    // Nudge a third of the pixels by 1–2 levels, as another GPU might.
    for (ix, pixel) in shot.image.pixels_mut().enumerate() {
        if ix % 3 == 0 {
            pixel.0[0] = pixel.0[0].saturating_sub(2);
            pixel.0[1] = pixel.0[1].saturating_add(1);
        }
    }
    let outcome = shot.golden_at(&golden_dir(), "card", GoldenTolerance::default(), false);
    assert!(outcome.passed, "{}", outcome.message);
    assert!(outcome.stats.changed > 10_000, "{:?}", outcome.stats);
    assert_eq!(outcome.stats.differing, 0);
    assert!(
        outcome.message.contains("differ only by noise"),
        "{}",
        outcome.message
    );

    let (exact, _) = compare(&recorded_card(), &shot.image, GoldenTolerance::exact(), 2.);
    assert_eq!(
        exact.differing, exact.changed,
        "an exact comparison would fail"
    );
}
