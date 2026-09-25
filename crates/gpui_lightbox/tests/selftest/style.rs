//! The style lint doesn't lie: the clean card passes Loupe's spec in every
//! variant, and each deliberately broken card is caught by exactly the rule
//! it breaks.

use crate::{CARD, CATCHES, card_config, card_stage, views::Flaw};
use gpui_lightbox::{Appearance, LintReport, Rule, StyleSpec, TextRole};

fn rules(report: &LintReport) -> Vec<Rule> {
    let mut rules = report
        .violations
        .iter()
        .map(|violation| violation.rule)
        .collect::<Vec<_>>();
    rules.dedup();
    rules
}

#[test]
fn the_clean_card_passes_the_loupe_spec() {
    let spec = StyleSpec::loupe();
    for (appearance, scale) in [
        (Appearance::Light, 2.),
        (Appearance::Dark, 2.),
        (Appearance::Light, 1.),
        (Appearance::Dark, 1.),
    ] {
        let config = card_config().appearance(appearance).scale(scale);
        let (mut stage, _) = card_stage(CARD, Flaw::None, config);
        stage.record_elements();
        let name = format!("{}-{scale}x", appearance.label().to_lowercase());
        let report = stage.shot(&name).lint(&spec);
        report.assert_clean();
        assert!(report.checked.texts >= 12, "{:?}", report.checked);
        assert!(report.checked.quads >= 5, "{:?}", report.checked);
        assert!(report.annotated.exists());
    }
}

#[test]
fn each_broken_card_is_caught_by_the_rule_it_breaks() {
    let spec = StyleSpec::loupe();
    for (flaw, name, rule, subject) in [
        (
            Flaw::OffGridPadding,
            "off-grid-padding",
            Rule::Spacing,
            "“Render pipeline”",
        ),
        (
            Flaw::RogueFont,
            "rogue-font",
            Rule::FontFamily,
            "“Render pipeline”",
        ),
        (
            Flaw::LowContrast,
            "low-contrast",
            Rule::Contrast,
            "“Frames are drawn in five phases.",
        ),
        (
            Flaw::ClippedText,
            "clipped-text",
            Rule::ClippedText,
            "“quarterly-report-final-v2.pdf”",
        ),
        (
            Flaw::OverlappingLabels,
            "overlapping-labels",
            Rule::TextOverlap,
            "“prepaint” and “present”",
        ),
    ] {
        let (mut stage, _) = card_stage(CATCHES, flaw, card_config());
        let report = stage.shot(name).lint(&spec);
        report.assert_caught(rule);
        assert_eq!(
            rules(&report),
            [rule],
            "{name} breaks only {rule}:\n{report}"
        );
        assert!(
            report
                .violations
                .iter()
                .any(|violation| violation.subject.starts_with(subject)),
            "{name}: expected a violation about {subject}:\n{report}"
        );
    }
}

#[test]
fn the_violation_says_what_was_measured() {
    let (mut stage, _) = card_stage(CATCHES, Flaw::LowContrast, card_config());
    let report = stage
        .capture("low-contrast-explained")
        .lint(&StyleSpec::loupe());
    let violation = &report.of(Rule::Contrast)[0];
    // The faint text color on the card's surface, measured from the pixels.
    assert!(
        violation.message.contains("#8d939d on #f7f7f9"),
        "{}",
        violation.message
    );
    assert!(
        violation.message.contains("needs 4.5:1"),
        "{}",
        violation.message
    );
    let ratio = violation.message["contrast ".len()..]
        .split(':')
        .next()
        .and_then(|ratio| ratio.parse::<f32>().ok())
        .expect("a ratio");
    assert!((2.8..3.1).contains(&ratio), "{ratio}");
}

#[test]
fn specs_are_data() {
    // A spec written as JSON that only knows the UI font: the card's Lilex
    // values break it.
    let ui_only = r##"{
        "name": "ui only",
        "text_roles": [{ "name": "ui", "families": ["IBM Plex Sans"], "sizes": [12, 11] }],
        "text_colors": ["#1c1f24", "#5d636e", "#2c6be8", "#ffffff"],
        "min_contrast": 4.5
    }"##;
    let spec = StyleSpec::from_json(ui_only).expect("a valid spec");
    let (mut stage, _) = card_stage(CATCHES, Flaw::None, card_config());
    let report = stage.capture("ui-only-spec").lint(&spec);
    assert_eq!(rules(&report), [Rule::FontFamily]);
    let values = report
        .violations
        .iter()
        .map(|violation| violation.subject.as_str())
        .collect::<Vec<_>>();
    assert_eq!(values, ["“3.1 ms”", "“0.8 ms”", "“1.2 ms”"]);

    // Add the mono role, and the same card passes.
    let mut with_mono = spec;
    with_mono
        .text_roles
        .push(TextRole::new("mono", &["Lilex"]).sizes(&[12.]));
    stage
        .capture("ui-and-mono-spec")
        .lint(&with_mono)
        .assert_clean();

    // The rogue title fell back to gpui's default mono font, and says so.
    let (mut stage, _) = card_stage(CATCHES, Flaw::RogueFont, card_config());
    let report = stage
        .capture("rogue-font-explained")
        .lint(&StyleSpec::loupe());
    assert!(
        report.violations[0].message.contains("gpui's fallback"),
        "{report}"
    );
}

#[test]
fn hit_targets_need_the_element_tree() {
    let (mut stage, _) = card_stage(CATCHES, Flaw::None, card_config());
    let report = stage.capture("no-element-tree").lint(&StyleSpec::loupe());
    assert_eq!(report.checked.hit_targets, None);
    assert!(
        report.checked.skipped[0].starts_with("hit-target: no element tree"),
        "{:?}",
        report.checked.skipped
    );
    stage.record_elements();
    let shot = stage.capture("with-element-tree");
    let report = shot.lint(&StyleSpec::loupe());
    // Until the engine records element trees, the rule reports itself skipped
    // instead of passing silently.
    match &shot.clickables {
        Some(clickables) => assert_eq!(report.checked.hit_targets, Some(clickables.len())),
        None => assert!(!report.checked.skipped.is_empty()),
    }
}
