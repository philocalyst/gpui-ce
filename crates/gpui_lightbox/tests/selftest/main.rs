//! Lightbox's own suite: purpose-built views driven through the real
//! pipeline, proving that nothing Lightbox reports is a lie.

mod animation;
mod benches;
mod goldens;
mod input;
mod report;
mod style;
mod views;

use gpui::AppContext as _;
use gpui_lightbox::{Stage, StageConfig};
use std::path::PathBuf;
use views::{Card, Flaw};

/// A suite's name and one-sentence description.
pub type SuiteName = (&'static str, &'static str);

/// The clean card, as a design would ship it.
pub const CARD: SuiteName = (
    "card",
    "The reference card in Loupe's visual language: light and dark at 1× and 2×, linted and golden-checked.",
);
/// Input through real dispatch.
pub const INTERACTION: SuiteName = (
    "interaction",
    "Clicks, hovers, drags, scrolls and keys go through Window::dispatch_event, like platform input.",
);
/// Films of animations.
pub const ANIMATIONS: SuiteName = (
    "animations",
    "Style transitions, springs and with_animation sampled at exact times on the fake clock.",
);
/// Deliberately broken views.
pub const CATCHES: SuiteName = (
    "catches",
    "Deliberately broken views and animations. Everything here must be flagged: red means Lightbox caught it.",
);

/// A stage in `suite`, whose description it records.
pub fn stage(suite: SuiteName, config: StageConfig) -> Stage {
    let stage = Stage::with_config(suite.0, config);
    stage
        .suite()
        .describe(suite.1)
        .expect("describing the suite");
    stage
}

/// A stage showing a card with `flaw`.
pub fn card_stage(
    suite: SuiteName,
    flaw: Flaw,
    config: StageConfig,
) -> (Stage, gpui::Entity<Card>) {
    let mut stage = stage(suite, config);
    let card = stage.mount(|_, cx| cx.new(|_| Card::new(flaw)));
    (stage, card)
}

/// The config every card test uses unless it says otherwise: 440×336 at 2×.
pub fn card_config() -> StageConfig {
    StageConfig::default().size(440., 336.)
}

/// A fresh directory under the system temp dir, for tests that must not
/// touch the real output.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightbox-selftest-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("creating a scratch directory");
    dir
}
