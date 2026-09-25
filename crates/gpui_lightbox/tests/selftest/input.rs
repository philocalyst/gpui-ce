//! The stage: rendering configuration, queries and real input.

use crate::{
    CARD, INTERACTION, card_config, card_stage, stage,
    views::{Card, Flaw, HOVER, Keys, Knob, ScrollList},
};
use gpui::{AppContext as _, point, px};
use gpui_lightbox::{
    Appearance, Color, Matrix, StageConfig, UI_FONT,
    theme::{DARK, LIGHT},
};
use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn renders_at_the_requested_scale_and_appearance() {
    let (mut stage, _) = card_stage(CARD, Flaw::None, card_config().scale(1.));
    let light = stage.shot("light-1x");
    assert_eq!(light.image.dimensions(), (440, 336));
    assert_eq!(light.pixel(4., 4.), LIGHT.bg);
    assert_eq!(light.meta.appearance, Appearance::Light);

    stage.set_appearance(Appearance::Dark).set_scale(2.);
    let dark = stage.shot("dark-2x");
    assert_eq!(dark.image.dimensions(), (880, 672));
    assert_eq!(dark.pixel(4., 4.), DARK.bg);
    assert_eq!(dark.meta.appearance, Appearance::Dark);
}

#[test]
fn shots_record_painted_text_and_quads() {
    let (mut stage, _) = card_stage(CARD, Flaw::None, card_config());
    let shot = stage.shot("light-2x");
    let title = shot.find_text("Render pipeline").unwrap();
    assert_eq!(
        (
            title.font_family.as_str(),
            title.font_size,
            title.font_weight
        ),
        (UI_FONT, 12., 600.)
    );
    assert_eq!(
        title.bounds.x, 48.,
        "32 px window padding + 1 px border + 15 px padding"
    );
    assert!(title.baseline.is_some());
    // gpui paints the fill and each border edge as separate quads; boxes merge them.
    assert!(shot.quad_filled(LIGHT.surface).is_some());
    let card = shot
        .boxes()
        .into_iter()
        .find(|quad| quad.background == Some(LIGHT.surface))
        .expect("the card's box");
    assert_eq!(card.bounds.x, 32.);
    assert_eq!(card.bounds.w, 360.);
    assert_eq!(card.corner_radii, [8.; 4]);
    assert_eq!(card.border_widths, [1.; 4]);
    assert!(shot.path().exists());
}

#[test]
fn click_text_drives_real_handlers() {
    let (mut stage, card) = card_stage(INTERACTION, Flaw::None, card_config());
    assert!(stage.find_text("Not saved yet").is_ok());
    stage.click_text("Save");
    assert_eq!(stage.app().read_entity(&card, |card, _| card.saves()), 1);
    assert!(stage.find_text("Saved 1 time").is_ok());
    stage.click_text("Save");
    assert!(stage.find_text("Saved 2 times").is_ok());
    stage.shot("saved-twice");
}

#[test]
fn click_text_misses_list_what_is_visible() {
    let (mut stage, _) = card_stage(INTERACTION, Flaw::None, card_config());
    let panic = catch_unwind(AssertUnwindSafe(|| {
        stage.click_text("Sav");
    }))
    .expect_err("there is no text \"Sav\"");
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(
        message.contains("no visible text is exactly \"Sav\""),
        "{message}"
    );
    assert!(
        message.contains("\"Save\""),
        "suggests the closest texts:\n{message}"
    );
}

#[test]
fn hover_transitions_ease_on_the_fake_clock() {
    let (mut stage, _) = card_stage(INTERACTION, Flaw::None, card_config());
    let save = stage.find_text("Save").unwrap();
    let (x, y) = (save.bounds.x - 6., save.visible.center().1);
    let base = stage.capture("before").pixel(x, y);
    assert_eq!(base, LIGHT.accent);

    stage.hover_text("Save");
    assert_eq!(
        stage.capture("t0").pixel(x, y),
        base,
        "hover starts easing at t=0"
    );
    let halfway = stage
        .advance(HOVER / 2)
        .shot("save-hover-halfway")
        .pixel(x, y);
    let hovered = stage.advance(HOVER).shot("save-hovered").pixel(x, y);
    let target = Color::from_u32(0x1f58c9);
    assert!(hovered.delta_e(target) < 0.005, "{hovered} vs {target}");
    assert!(halfway.delta_e(base) > 0.01 && halfway.delta_e(target) > 0.01);
}

#[test]
fn keys_and_typing_go_through_dispatch() {
    let mut stage = stage(INTERACTION, StageConfig::default().size(320., 96.));
    let keys = stage.mount(Keys::mount);
    stage.keys("ctrl-k");
    assert!(stage.app().read_entity(&keys, |keys, _| keys.toggled));
    stage.type_text("Hi there");
    assert!(stage.find_text("Typed: Hi there").is_ok());
    assert!(stage.find_text("Palette open").is_ok());
    stage.keys("ctrl-k");
    assert!(stage.find_text("Palette closed").is_ok());
    stage.shot("keys");
}

#[test]
fn drag_moves_the_knob() {
    let mut stage = stage(INTERACTION, StageConfig::default().size(368., 72.));
    let knob = stage.mount(|_, cx| cx.new(|_| Knob { x: 0. }));
    stage.drag(point(px(32.), px(36.)), point(px(232.), px(36.)), 8);
    let x = stage.app().read_entity(&knob, |knob, _| knob.x);
    assert_eq!(x, 200.);
    stage.shot("knob-dragged");
}

#[test]
fn scrolling_moves_the_list() {
    let mut stage = stage(INTERACTION, StageConfig::default().size(328., 226.));
    stage.mount(|_, cx| cx.new(|_| ScrollList));
    let top = stage.find_text("Row 00").unwrap().visible.y;
    stage.shot("list");
    stage.scroll(point(px(160.), px(100.)), point(px(0.), px(-110.)));
    let row = stage.find_text("Row 05").unwrap();
    assert_eq!(row.visible.y, top, "five 22 px rows scrolled away");
    assert!(stage.find_text("Row 00").is_err(), "scrolled out of view");
    stage.shot("list-scrolled");
}

#[test]
fn matrix_shoots_every_variant_into_one_grid() {
    let (mut stage, _) = card_stage(CARD, Flaw::None, card_config());
    let matrix = stage.matrix("variants", Matrix::standard(), |stage, _| {
        stage.mount(|_, cx| cx.new(|_| Card::new(Flaw::None)));
        stage.hover_text("Save").advance(HOVER);
    });
    assert_eq!(matrix.shots.len(), 4);
    let sizes = matrix
        .shots
        .iter()
        .map(|(variant, shot)| (variant.label(), shot.image.width()))
        .collect::<Vec<_>>();
    assert_eq!(
        sizes,
        [
            ("Light · 1× · 440×336".to_string(), 440),
            ("Light · 2× · 440×336".to_string(), 880),
            ("Dark · 1× · 440×336".to_string(), 440),
            ("Dark · 2× · 440×336".to_string(), 880),
        ]
    );
    let grid = image::open(&matrix.grid).unwrap();
    assert!(grid.width() > 880 && grid.height() > 672);
    assert_eq!(
        stage.config().scale,
        2.,
        "the stage returns to its settings"
    );
}
