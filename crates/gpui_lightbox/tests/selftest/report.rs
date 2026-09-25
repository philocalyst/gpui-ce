//! The report builds from real records, and every file it references exists.

use crate::{
    card_config, scratch_dir,
    views::{Card, Flaw},
};
use gpui::{App, AppContext as _, Window};
use gpui_lightbox::{
    BenchSpec, FilmSpec, GoldenTolerance, Stage, StyleSpec, Suite, bench, golden::golden_dir,
    report,
};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path, time::Duration};

/// Every relative path in `src="…"` / `href="…"` attributes.
fn attribute_paths(html: &str) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for attribute in [" src=\"", " href=\""] {
        for (start, _) in html.match_indices(attribute) {
            let rest = &html[start + attribute.len()..];
            let Some(end) = rest.find('"') else { continue };
            let value = rest[..end].replace("&amp;", "&");
            if !value.is_empty() && !value.starts_with('#') && !value.contains("://") {
                paths.insert(value);
            }
        }
    }
    paths
}

/// Every string in the embedded data that names an image.
fn data_paths(value: &Value, paths: &mut BTreeSet<String>) {
    match value {
        Value::String(text) if text.ends_with(".png") && !text.contains(' ') => {
            paths.insert(text.clone());
        }
        Value::Array(values) => values.iter().for_each(|value| data_paths(value, paths)),
        Value::Object(map) => {
            for (key, value) in map {
                // The golden's own path is relative to the workspace, not the report.
                if key != "golden" {
                    data_paths(value, paths);
                }
            }
        }
        _ => {}
    }
}

fn embedded_data(html: &str) -> Value {
    let start = html
        .find(r#"<script type="application/json" id="lightbox-data">"#)
        .expect("the page embeds its data");
    let json = &html[start..];
    let json =
        &json[json.find('>').expect("tag end") + 1..json.find("</script>").expect("script end")];
    serde_json::from_str(&json.replace("<\\/", "</")).expect("the embedded data is JSON")
}

fn missing(root: &Path, paths: &BTreeSet<String>) -> Vec<String> {
    paths
        .iter()
        .filter(|path| !root.join(path).is_file())
        .cloned()
        .collect()
}

#[test]
fn the_report_builds_and_every_link_resolves() {
    let root = scratch_dir("report");
    let suite = Suite::at("mini", root.join("mini"));
    suite.describe("A tiny suite for the report test.").unwrap();
    let mut stage = Stage::in_suite(suite, card_config());
    stage.mount(|_, cx| cx.new(|_| Card::new(Flaw::None)));
    let shot = stage.shot("card");
    shot.lint(&StyleSpec::loupe()).assert_clean();
    let golden = shot.golden_at(&golden_dir(), "card", GoldenTolerance::default(), false);
    assert!(golden.passed, "{}", golden.message);
    stage
        .capture("captured")
        .lint(&StyleSpec::default())
        .assert_clean();
    let mut film = stage.film(
        "hover",
        FilmSpec::frames(Duration::from_millis(160), 5),
        |stage| {
            stage.hover_text("Save");
        },
    );
    film.track("frame", |shot| shot.meta.time_ms as f32);
    drop(film);
    let spec = BenchSpec {
        warmup: 1,
        iterations: 5,
        phases: false,
        stage: card_config(),
        history_dir: Some(root.join("bench")),
        ..BenchSpec::default()
    };
    bench("mini", spec, |stage| {
        stage.mount(|_, cx| cx.new(|_| Card::new(Flaw::None)));
        |_: &mut Window, _: &mut App| {}
    });

    let (index, summary) = report::build(&root).expect("the report builds");
    assert_eq!(
        (
            summary.suites,
            summary.shots,
            summary.films,
            summary.lints,
            summary.goldens,
            summary.benches
        ),
        (1, 1, 1, 2, 1, 1)
    );
    assert!(
        root.join("mini/sheet.png").is_file(),
        "the suite has a contact sheet"
    );

    let html = fs::read_to_string(&index).unwrap();
    assert!(html.contains("A tiny suite for the report test."));
    let attributes = attribute_paths(&html);
    assert!(attributes.contains("mini/card.png"), "{attributes:?}");
    assert!(attributes.contains("mini/sheet.png"), "{attributes:?}");
    assert_eq!(missing(&root, &attributes), Vec::<String>::new());

    let mut referenced = BTreeSet::new();
    data_paths(&embedded_data(&html), &mut referenced);
    assert!(referenced.contains("mini/hover.anim.png"), "{referenced:?}");
    assert!(
        referenced.contains("mini/hover.frames/004.png"),
        "{referenced:?}"
    );
    assert!(
        referenced.contains("mini/captured.png"),
        "lint-only shots are viewable: {referenced:?}"
    );
    assert_eq!(missing(&root, &referenced), Vec::<String>::new());
}
