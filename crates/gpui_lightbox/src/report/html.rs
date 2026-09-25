//! The report page: static HTML with inline CSS, JS and data.

use super::{
    Report, SuiteData,
    svg::{self, Segment, Series, number},
};
use crate::{
    bench::{BenchHistory, BenchRun, Verdict},
    lint::Severity,
    manifest::{FilmRecord, GoldenRecord, LintRecord, Record, ShotRecord},
    output::slug,
    sheet::{Status, shot_status},
    shot::trim,
};
use anyhow::Result;
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, fmt::Write as _, path::Path};

const CSS: &str = include_str!("assets/report.css");
const JS: &str = include_str!("assets/report.js");

/// Escapes text for HTML content and attribute values.
pub fn esc(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for char in text.chars() {
        match char {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            char => escaped.push(char),
        }
    }
    escaped
}

/// An item the viewer can open, keyed by id.
type Items = Map<String, Value>;

/// Something that needs attention, for the overview.
struct Attention {
    status: Status,
    suite: String,
    name: String,
    message: String,
    open: String,
}

/// Renders the whole page.
pub(super) fn render(report: &Report, root: &Path) -> Result<String> {
    let mut items = Items::new();
    let mut attention = Vec::new();
    let mut suites_html = String::new();
    for suite in &report.suites {
        suites_html.push_str(&suite_section(suite, root, &mut items, &mut attention));
    }
    let benches_html = benches_section(&report.benches, &mut attention);
    let overview = overview(report, &attention);
    let nav = nav(report);

    let summary = report.summary();
    let data = serde_json::to_string(&json!({ "items": items }))?.replace("</", "<\\/");
    let mut page = String::new();
    let _ = write!(
        page,
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Lightbox report</title>
<meta name="description" content="{description}">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500&family=IBM+Plex+Sans:ital,wght@0,400;0,500;0,600;1,400&display=swap">
<style>{CSS}</style>
</head>
<body>
<header class="top">
  <a class="brand" href="#overview"><span class="mark" aria-hidden="true"></span>Lightbox</a>
  <span class="run mono">{sha}{branch} · {time} · {profile}</span>
  <span class="grow"></span>
  <label class="search"><input id="filter" type="search" placeholder="Filter shots, films, benchmarks" autocomplete="off"><kbd>/</kbd></label>
  <button class="button" id="theme" type="button" title="Theme: follows the system">Auto</button>
</header>
<div class="layout">
{nav}
<main>
{overview}
{suites_html}
{benches_html}
</main>
</div>
<div class="viewer" id="viewer" hidden></div>
<script type="application/json" id="lightbox-data">{data}</script>
<script>{JS}</script>
</body>
</html>
"##,
        description = esc(&summary.to_string()),
        sha = esc(&report.run.git_sha),
        branch = report
            .run
            .git_branch
            .as_ref()
            .map(|branch| format!(" · {}", esc(branch)))
            .unwrap_or_default(),
        time = esc(&report.run.timestamp),
        profile = esc(&report.run.profile),
    );
    Ok(page)
}

fn item_id(suite: &SuiteData, kind: &str, name: &str) -> String {
    format!("{}:{kind}:{}", suite.dir_name, slug(name))
}

fn path(suite: &SuiteData, file: &str) -> String {
    format!("{}/{}", suite.dir_name, file)
}

/// `width` and `height` attributes for an image under `root`, when readable.
fn dimensions(root: &Path, relative: &str) -> String {
    image::image_dimensions(root.join(relative))
        .map(|(w, h)| format!(r#" width="{w}" height="{h}""#))
        .unwrap_or_default()
}

fn chip(status: Status, text: &str) -> String {
    format!(
        r#"<span class="chip {}"><i></i>{}</span>"#,
        status_class(status),
        esc(text)
    )
}

fn status_class(status: Status) -> &'static str {
    match status {
        Status::Neutral => "neutral",
        Status::Ok => "ok",
        Status::Warn => "warn",
        Status::Crit => "crit",
    }
}

fn origin_json(record: &Record) -> Value {
    let origin = record.origin();
    json!({ "test": origin.test, "location": origin.location })
}

fn suite_section(
    suite: &SuiteData,
    root: &Path,
    items: &mut Items,
    attention: &mut Vec<Attention>,
) -> String {
    let records = &suite.records;
    let shots = records
        .iter()
        .filter_map(|record| match record {
            Record::Shot(shot) => Some(shot),
            _ => None,
        })
        .collect::<Vec<_>>();
    let lints = records
        .iter()
        .filter_map(|record| match record {
            Record::Lint(lint) => Some(lint),
            _ => None,
        })
        .collect::<Vec<_>>();
    let goldens = records
        .iter()
        .filter_map(|record| match record {
            Record::Golden(golden) => Some(golden),
            _ => None,
        })
        .collect::<Vec<_>>();
    let films = records
        .iter()
        .filter_map(|record| match record {
            Record::Film(film) => Some(film),
            _ => None,
        })
        .collect::<Vec<_>>();
    let matrices = records
        .iter()
        .filter_map(|record| match record {
            Record::Matrix(matrix) => Some(matrix),
            _ => None,
        })
        .collect::<Vec<_>>();

    // Items first, so cards and rows can point at them.
    for shot in &shots {
        let lint = lints.iter().find(|lint| lint.name == shot.name).copied();
        let golden = goldens
            .iter()
            .find(|golden| golden.name == shot.name)
            .copied();
        items.insert(
            item_id(suite, "shot", &shot.name),
            shot_item(suite, shot, lint, golden, &Record::Shot((*shot).clone())),
        );
    }
    for lint in &lints {
        if !shots.iter().any(|shot| shot.name == lint.name) {
            items.insert(
                item_id(suite, "shot", &lint.name),
                lint_only_item(suite, lint),
            );
        }
    }
    for golden in &goldens {
        items.insert(
            item_id(suite, "golden", &golden.name),
            golden_item(suite, root, golden),
        );
    }
    for film in &films {
        items.insert(item_id(suite, "film", &film.name), film_item(suite, film));
    }
    for matrix in &matrices {
        items.insert(
            item_id(suite, "matrix", &matrix.name),
            image_item(
                suite,
                &matrix.name,
                "matrix",
                &path(suite, &matrix.image),
                &Record::Matrix((*matrix).clone()),
            ),
        );
    }
    for (ix, sheet) in suite.sheets.iter().enumerate() {
        items.insert(
            format!("{}:sheet:{}", suite.dir_name, ix + 1),
            json!({
                "kind": "image",
                "title": format!("{} — contact sheet{}", suite.suite.name(), if suite.sheets.len() > 1 { format!(" {}", ix + 1) } else { String::new() }),
                "suite": suite.suite.name(),
                "image": sheet,
            }),
        );
    }

    // Attention.
    for lint in &lints {
        let errors = lint
            .violations
            .iter()
            .filter(|violation| violation.severity == Severity::Error)
            .count();
        if !lint.violations.is_empty() {
            attention.push(Attention {
                status: if errors > 0 {
                    Status::Crit
                } else {
                    Status::Warn
                },
                suite: suite.suite.name().into(),
                name: lint.name.clone(),
                message: format!(
                    "{} style violation{}: {}",
                    lint.violations.len(),
                    if lint.violations.len() == 1 { "" } else { "s" },
                    rule_counts(lint)
                ),
                open: item_id(suite, "shot", &lint.name),
            });
        }
    }
    for golden in goldens.iter().filter(|golden| !golden.passed) {
        attention.push(Attention {
            status: Status::Crit,
            suite: suite.suite.name().into(),
            name: golden.name.clone(),
            message: golden.message.clone(),
            open: item_id(suite, "golden", &golden.name),
        });
    }
    for film in &films {
        if let Some(failed) = film.assertions.iter().find(|assertion| !assertion.passed) {
            attention.push(Attention {
                status: Status::Crit,
                suite: suite.suite.name().into(),
                name: film.name.clone(),
                message: format!("{}: {}", failed.name, failed.message),
                open: item_id(suite, "film", &film.name),
            });
        }
    }

    let mut html = String::new();
    let description = suite.suite.description().unwrap_or_default();
    let mut counts = Vec::new();
    for (count, noun) in [
        (shots.len(), "shot"),
        (matrices.len(), "matrix"),
        (films.len(), "film"),
        (lints.len(), "lint"),
        (goldens.len(), "golden"),
    ] {
        if count > 0 {
            let plural = match (count, noun) {
                (1, _) => noun.to_string(),
                (_, "matrix") => "matrices".into(),
                _ => format!("{noun}s"),
            };
            counts.push(format!(
                r#"<span class="count-chip">{count} {plural}</span>"#
            ));
        }
    }
    let _ = write!(
        html,
        r#"<section class="suite" id="suite-{dir}">
<header class="suite-head">
  <div><div class="label">Suite</div><h2>{name}</h2>{description}</div>
  <div class="counts">{counts}</div>
</header>"#,
        dir = esc(&suite.dir_name),
        name = esc(suite.suite.name()),
        description = if description.is_empty() {
            String::new()
        } else {
            format!(r#"<p class="desc">{}</p>"#, esc(&description))
        },
        counts = counts.join(""),
    );

    if !suite.sheets.is_empty() {
        html.push_str(r#"<div class="block"><div class="block-head"><h3>Contact sheet</h3><span class="hint">One image per page: what an agent reviews first.</span></div><div class="sheets">"#);
        for (ix, sheet) in suite.sheets.iter().enumerate() {
            let id = format!("{}:sheet:{}", suite.dir_name, ix + 1);
            let _ = write!(
                html,
                r#"<button class="sheet" type="button" data-open="{id}" data-list="{list}"><img src="{src}" alt="Contact sheet page {page}" loading="lazy" decoding="async"{dims}></button>"#,
                id = esc(&id),
                list = esc(&format!("{}:sheets", suite.dir_name)),
                src = esc(sheet),
                page = ix + 1,
                dims = dimensions(root, sheet)
            );
        }
        html.push_str("</div></div>");
    }

    if !shots.is_empty() {
        let list = format!("{}:shots", suite.dir_name);
        html.push_str(
            r#"<div class="block"><div class="block-head"><h3>Shots</h3></div><div class="cards">"#,
        );
        for shot in &shots {
            let (status, note) = shot_status(records, &shot.name);
            let [w, h] = shot.meta.size;
            let _ = write!(
                html,
                r#"<article class="card" tabindex="0" data-open="{id}" data-list="{list}" data-name="{filter}">
<div class="thumb"><img src="{src}" alt="" loading="lazy" decoding="async" width="{pw}" height="{ph}"></div>
<div class="caption"><div class="text"><div class="name">{name}</div><div class="meta">{w}×{h} @{scale}× · {appearance}</div></div>{chip}</div>
</article>"#,
                id = esc(&item_id(suite, "shot", &shot.name)),
                list = esc(&list),
                filter = esc(&shot.name.to_lowercase()),
                src = esc(&path(suite, &shot.image)),
                pw = (w * shot.meta.scale) as u32,
                ph = (h * shot.meta.scale) as u32,
                name = esc(&shot.name),
                w = trim(w),
                h = trim(h),
                scale = trim(shot.meta.scale),
                appearance = shot.meta.appearance.label().to_lowercase(),
                chip = chip(status, &note),
            );
        }
        html.push_str("</div></div>");
    }

    if !matrices.is_empty() {
        html.push_str(r#"<div class="block"><div class="block-head"><h3>Matrices</h3></div><div class="cards wide">"#);
        for matrix in &matrices {
            let src = path(suite, &matrix.image);
            let _ = write!(
                html,
                r#"<article class="card" tabindex="0" data-open="{id}" data-name="{filter}">
<div class="thumb tall"><img src="{src}" alt="" loading="lazy" decoding="async"{dims}></div>
<div class="caption"><div class="text"><div class="name">{name}</div><div class="meta">{variants}</div></div></div>
</article>"#,
                id = esc(&item_id(suite, "matrix", &matrix.name)),
                filter = esc(&matrix.name.to_lowercase()),
                src = esc(&src),
                dims = dimensions(root, &src),
                name = esc(&matrix.name),
                variants = esc(&matrix
                    .cells
                    .iter()
                    .map(|cell| cell.label.as_str())
                    .collect::<Vec<_>>()
                    .join("  ·  ")),
            );
        }
        html.push_str("</div></div>");
    }

    if !films.is_empty() {
        let list = format!("{}:films", suite.dir_name);
        html.push_str(r#"<div class="block"><div class="block-head"><h3>Films</h3><span class="hint">Animated PNGs; open one to scrub frames against its curves.</span></div><div class="cards">"#);
        for film in &films {
            let failed = film
                .assertions
                .iter()
                .filter(|assertion| !assertion.passed)
                .count();
            let problems = film
                .findings
                .iter()
                .filter(|finding| finding.kind.is_problem())
                .count();
            let (status, note) = if failed > 0 {
                (Status::Crit, format!("{failed} failed"))
            } else if problems > 0 {
                (
                    Status::Warn,
                    format!("{problems} finding{}", if problems == 1 { "" } else { "s" }),
                )
            } else if film.assertions.is_empty() {
                (Status::Neutral, format!("{} findings", film.findings.len()))
            } else {
                (Status::Ok, format!("{} passed", film.assertions.len()))
            };
            let settled = film
                .frames
                .iter()
                .rev()
                .find(|frame| frame.changed_pixels > 0)
                .map_or(0., |frame| frame.t_ms);
            let track = film.tracks.first();
            let spark = track.map_or_else(String::new, |track| {
                let values = track.values.iter().map(|value| value.map(f64::from)).collect::<Vec<_>>();
                let times = film.frames.iter().map(|frame| frame.t_ms).collect::<Vec<_>>();
                let marks = film
                    .findings
                    .iter()
                    .filter(|finding| finding.track.as_deref() == Some(track.name.as_str()))
                    .map(|finding| (finding.frame, finding_class(finding.kind.is_problem()), finding.message.clone()))
                    .collect::<Vec<_>>();
                let range = match (track.first(), track.last()) {
                    (Some(first), Some(last)) => format!("{} → {}", trim(first), trim(last)),
                    _ => "not measured".into(),
                };
                format!(
                    r#"<div class="curve-mini"><div class="curve-head"><span><i class="key s1"></i>{}</span><span>{}</span></div>{}</div>"#,
                    esc(&track.name),
                    esc(&range),
                    svg::curve(&values, &times, "s1", &marks, 300., 72.)
                )
            });
            let [w, h] = film.size;
            let _ = write!(
                html,
                r#"<article class="card" tabindex="0" data-open="{id}" data-list="{list}" data-name="{filter}">
<div class="thumb"><img src="{src}" alt="" loading="lazy" decoding="async" width="{pw}" height="{ph}"></div>
<div class="caption"><div class="text"><div class="name">{name}</div><div class="meta">{frames} frames · {duration} ms · settled {settled} ms</div></div>{chip}</div>
{spark}
</article>"#,
                id = esc(&item_id(suite, "film", &film.name)),
                list = esc(&list),
                filter = esc(&film.name.to_lowercase()),
                src = esc(&path(suite, &film.animation)),
                pw = (w * film.scale) as u32,
                ph = (h * film.scale) as u32,
                name = esc(&film.name),
                frames = film.frames.len(),
                duration = trim(film.duration_ms as f32),
                settled = trim(settled as f32),
                chip = chip(status, &note),
            );
        }
        html.push_str("</div></div>");
    }

    if !lints.is_empty() {
        html.push_str(r#"<div class="block"><div class="block-head"><h3>Style lint</h3><span class="hint">Open a row to see its violations over the shot.</span></div>
<table class="data"><thead><tr><th class="status"></th><th>Shot</th><th>Spec</th><th class="num">Texts</th><th class="num">Boxes</th><th>Violations</th><th>Verdict</th></tr></thead><tbody>"#);
        let list = format!("{}:lints", suite.dir_name);
        for lint in &lints {
            let errors = lint
                .violations
                .iter()
                .filter(|violation| violation.severity == Severity::Error)
                .count();
            let status = match (lint.violations.len(), errors) {
                (0, _) => Status::Ok,
                (_, 0) => Status::Warn,
                _ => Status::Crit,
            };
            let verdict = lint.verdict.as_ref().map_or_else(
                || r#"<span class="muted">—</span>"#.to_string(),
                |verdict| {
                    chip(
                        if verdict.passed {
                            Status::Ok
                        } else {
                            Status::Crit
                        },
                        &format!(
                            "{} {}",
                            verdict.name,
                            if verdict.passed { "passed" } else { "failed" }
                        ),
                    )
                },
            );
            let _ = write!(
                html,
                r#"<tr tabindex="0" data-open="{id}" data-list="{list}" data-view="lint" data-name="{filter}"><td class="status"><i class="dot {status}"></i></td><td class="strong">{name}</td><td>{spec}</td><td class="num">{texts}</td><td class="num">{quads}</td><td>{rules}</td><td>{verdict}</td></tr>"#,
                id = esc(&item_id(suite, "shot", &lint.name)),
                list = esc(&list),
                filter = esc(&lint.name.to_lowercase()),
                status = status_class(status),
                name = esc(&lint.name),
                spec = esc(&lint.spec),
                texts = lint.checked.texts,
                quads = lint.checked.quads,
                rules = if lint.violations.is_empty() {
                    r#"<span class="muted">none</span>"#.into()
                } else {
                    rule_chips(lint)
                },
            );
        }
        html.push_str("</tbody></table></div>");
    }

    if !goldens.is_empty() {
        let list = format!("{}:goldens", suite.dir_name);
        html.push_str(r#"<div class="block"><div class="block-head"><h3>Goldens</h3><span class="hint">Open one to swipe between the golden and the shot.</span></div><div class="cards">"#);
        for golden in &goldens {
            let status = if golden.passed {
                Status::Ok
            } else {
                Status::Crit
            };
            let note = if golden.updated {
                "recorded"
            } else if golden.passed {
                "match"
            } else {
                "mismatch"
            };
            let src = path(suite, golden.diff.as_deref().unwrap_or(&golden.actual));
            let _ = write!(
                html,
                r#"<article class="card" tabindex="0" data-open="{id}" data-list="{list}" data-name="{filter}">
<div class="thumb"><img src="{src}" alt="" loading="lazy" decoding="async"{dims}></div>
<div class="caption"><div class="text"><div class="name">{name}</div><div class="meta" title="{message}">{message}</div></div>{chip}</div>
</article>"#,
                id = esc(&item_id(suite, "golden", &golden.name)),
                list = esc(&list),
                filter = esc(&golden.name.to_lowercase()),
                src = esc(&src),
                dims = dimensions(root, &src),
                name = esc(&golden.name),
                message = esc(&golden.message),
                chip = chip(status, note),
            );
        }
        html.push_str("</div></div>");
    }
    html.push_str("</section>");
    html
}

fn finding_class(problem: bool) -> &'static str {
    if problem { "crit" } else { "warn" }
}

fn rule_counts(lint: &LintRecord) -> String {
    let mut counts = BTreeMap::new();
    for violation in &lint.violations {
        *counts.entry(violation.rule.name()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|(rule, count)| {
            if count == 1 {
                rule.to_string()
            } else {
                format!("{rule} ×{count}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn rule_chips(lint: &LintRecord) -> String {
    let mut counts = BTreeMap::new();
    for violation in &lint.violations {
        let entry = counts
            .entry(violation.rule.name())
            .or_insert((0, violation.severity));
        entry.0 += 1;
    }
    counts
        .into_iter()
        .map(|(rule, (count, severity))| {
            let status = if severity == Severity::Error {
                Status::Crit
            } else {
                Status::Warn
            };
            let text = if count == 1 {
                rule.to_string()
            } else {
                format!("{rule} ×{count}")
            };
            chip(status, &text)
        })
        .collect()
}

fn violations_json(lint: &LintRecord) -> Value {
    json!({
        "spec": lint.spec,
        "annotated": lint.annotated,
        "checked": {
            "texts": lint.checked.texts,
            "quads": lint.checked.quads,
            "rules": lint.checked.rules.iter().map(|rule| rule.name()).collect::<Vec<_>>(),
            "skipped": lint.checked.skipped,
        },
        "verdict": lint.verdict.as_ref().map(|verdict| json!({ "name": verdict.name, "passed": verdict.passed })),
        "violations": lint.violations.iter().map(|violation| json!({
            "rule": violation.rule.name(),
            "severity": if violation.severity == Severity::Error { "crit" } else { "warn" },
            "subject": violation.subject,
            "message": violation.message,
            "box": [violation.bounds.x, violation.bounds.y, violation.bounds.w, violation.bounds.h],
        })).collect::<Vec<_>>(),
    })
}

fn shot_item(
    suite: &SuiteData,
    shot: &ShotRecord,
    lint: Option<&LintRecord>,
    golden: Option<&GoldenRecord>,
    record: &Record,
) -> Value {
    let texts = shot
        .texts
        .iter()
        .map(|text| {
            json!([
                text.visible.x,
                text.visible.y,
                text.visible.w,
                text.visible.h,
                text.text,
                text.font_family,
                text.font_size,
                text.font_weight,
                text.color.hex(),
                text.is_clipped(),
            ])
        })
        .collect::<Vec<_>>();
    let quads = crate::shot::merge_quads(&shot.quads)
        .iter()
        .filter(|quad| !quad.visible.is_empty())
        .map(|quad| {
            json!([
                quad.bounds.x,
                quad.bounds.y,
                quad.bounds.w,
                quad.bounds.h,
                quad.background.map(|color| color.hex()),
                quad.border_widths[0],
                quad.corner_radii[0],
            ])
        })
        .collect::<Vec<_>>();
    json!({
        "kind": "shot",
        "title": shot.name,
        "suite": suite.suite.name(),
        "image": path(suite, &shot.image),
        "size": shot.meta.size,
        "scale": shot.meta.scale,
        "appearance": shot.meta.appearance.label(),
        "time": shot.meta.time_ms,
        "texts": texts,
        "quads": quads,
        "lint": lint.map(|lint| {
            let mut value = violations_json(lint);
            value["annotated"] = json!(path(suite, &lint.annotated));
            value
        }),
        "golden": golden.map(|golden| item_id(suite, "golden", &golden.name)),
        "origin": origin_json(record),
    })
}

fn lint_only_item(suite: &SuiteData, lint: &LintRecord) -> Value {
    let mut value = violations_json(lint);
    value["annotated"] = json!(path(suite, &lint.annotated));
    json!({
        "kind": "shot",
        "title": lint.name,
        "suite": suite.suite.name(),
        "image": path(suite, &lint.shot_image),
        "size": lint.size,
        "scale": lint.scale,
        "texts": [],
        "quads": [],
        "lint": value,
        "origin": origin_json(&Record::Lint(lint.clone())),
    })
}

fn golden_item(suite: &SuiteData, root: &Path, golden: &GoldenRecord) -> Value {
    let actual = path(suite, &golden.actual);
    let (width, height) = image::image_dimensions(root.join(&actual)).unwrap_or((0, 0));
    json!({
        "kind": "golden",
        "title": golden.name,
        "suite": suite.suite.name(),
        "golden": golden.golden,
        "expected": golden.expected.as_ref().map(|file| path(suite, file)),
        "actual": actual,
        "diff": golden.diff.as_ref().map(|file| path(suite, file)),
        "comparison": golden.comparison.as_ref().map(|file| path(suite, file)),
        "pixels": [width, height],
        "passed": golden.passed,
        "updated": golden.updated,
        "message": golden.message,
        "stats": golden.stats,
        "tolerance": golden.tolerance,
        "origin": origin_json(&Record::Golden(golden.clone())),
    })
}

fn film_item(suite: &SuiteData, film: &FilmRecord) -> Value {
    let times = film
        .frames
        .iter()
        .map(|frame| frame.t_ms)
        .collect::<Vec<_>>();
    let mut curves = film
        .tracks
        .iter()
        .enumerate()
        .map(|(ix, track)| {
            let class = format!("s{}", ix % 6 + 1);
            let values = track
                .values
                .iter()
                .map(|value| value.map(f64::from))
                .collect::<Vec<_>>();
            let marks = film
                .findings
                .iter()
                .filter(|finding| finding.track.as_deref() == Some(track.name.as_str()))
                .map(|finding| {
                    (
                        finding.frame,
                        finding_class(finding.kind.is_problem()),
                        finding.message.clone(),
                    )
                })
                .collect::<Vec<_>>();
            json!({
                "name": track.name,
                "class": class,
                "range": track.range().map(|(min, max)| format!("{} … {}", trim(min), trim(max))),
                "svg": svg::curve(&values, &times, &class, &marks, 340., 96.),
            })
        })
        .collect::<Vec<_>>();
    let motion = film
        .frames
        .iter()
        .map(|frame| Some(f64::from(frame.energy) * 100.))
        .collect::<Vec<_>>();
    let freezes = film
        .findings
        .iter()
        .filter(|finding| finding.track.is_none())
        .map(|finding| {
            (
                finding.frame,
                finding_class(finding.kind.is_problem()),
                finding.message.clone(),
            )
        })
        .collect::<Vec<_>>();
    curves.push(json!({
        "name": "motion",
        "class": "motion",
        "range": "pixel change, %",
        "svg": svg::curve(&motion, &times, "motion", &freezes, 340., 96.),
    }));
    json!({
        "kind": "film",
        "title": film.name,
        "suite": suite.suite.name(),
        "animation": path(suite, &film.animation),
        "strip": path(suite, &film.strip),
        "size": film.size,
        "scale": film.scale,
        "frames": film.frames.iter().map(|frame| json!({
            "t": frame.t_ms,
            "image": path(suite, &frame.image),
            "changed": frame.changed_pixels,
            "box": frame.changed.map(|rect| [rect.x, rect.y, rect.w, rect.h]),
        })).collect::<Vec<_>>(),
        "curves": curves,
        "findings": film.findings.iter().map(|finding| json!({
            "kind": finding.kind.label(),
            "status": finding_class(finding.kind.is_problem()),
            "frame": finding.frame,
            "message": finding.message,
        })).collect::<Vec<_>>(),
        "assertions": film.assertions,
        "origin": origin_json(&Record::Film(film.clone())),
    })
}

fn image_item(suite: &SuiteData, title: &str, kind: &str, image: &str, record: &Record) -> Value {
    json!({
        "kind": "image",
        "title": title,
        "subtitle": kind,
        "suite": suite.suite.name(),
        "image": image,
        "origin": origin_json(record),
    })
}

fn nav(report: &Report) -> String {
    let mut html = String::from(
        r##"<nav class="side"><a class="nav-item" href="#overview">Overview</a><div class="nav-label">Suites</div>"##,
    );
    for suite in &report.suites {
        let status = suite_status(suite);
        let _ = write!(
            html,
            r##"<a class="nav-item" href="#suite-{dir}"><i class="dot {status}"></i><span class="grow">{name}</span><span class="count">{count}</span></a>"##,
            dir = esc(&suite.dir_name),
            status = status_class(status),
            name = esc(suite.suite.name()),
            count = suite.records.len(),
        );
    }
    let regressions = report
        .benches
        .iter()
        .any(|history| latest_verdict(history) == Some(Verdict::Regression));
    let _ = write!(
        html,
        r##"<div class="nav-label">Performance</div><a class="nav-item" href="#benchmarks"><i class="dot {}"></i><span class="grow">Benchmarks</span><span class="count">{}</span></a>
<div class="keys"><span><kbd>/</kbd>filter</span><span><kbd>Esc</kbd>close</span><span><kbd>←</kbd><kbd>→</kbd>browse</span><span><kbd>L</kbd>loupe</span><span><kbd>T</kbd>text</span><span><kbd>B</kbd>boxes</span><span><kbd>V</kbd>lint</span><span><kbd>0</kbd>–<kbd>4</kbd>zoom</span></div></nav>"##,
        if regressions {
            "crit"
        } else if report.benches.is_empty() {
            "neutral"
        } else {
            "ok"
        },
        report.benches.len(),
    );
    html
}

fn suite_status(suite: &SuiteData) -> Status {
    let mut status = Status::Neutral;
    for record in &suite.records {
        let record_status = match record {
            Record::Lint(lint) => {
                if lint
                    .violations
                    .iter()
                    .any(|violation| violation.severity == Severity::Error)
                {
                    Status::Crit
                } else if lint.violations.is_empty() {
                    Status::Ok
                } else {
                    Status::Warn
                }
            }
            Record::Golden(golden) => {
                if golden.passed {
                    Status::Ok
                } else {
                    Status::Crit
                }
            }
            Record::Film(film) => {
                if film.assertions.iter().any(|assertion| !assertion.passed) {
                    Status::Crit
                } else if film.assertions.is_empty() {
                    Status::Neutral
                } else {
                    Status::Ok
                }
            }
            Record::Shot(_) | Record::Matrix(_) => Status::Neutral,
        };
        status = status.max(record_status);
    }
    status
}

fn overview(report: &Report, attention: &[Attention]) -> String {
    let summary = report.summary();
    let tile = |label: &str, value: String, note: String| {
        format!(
            r#"<div class="tile"><div class="tile-label">{}</div><div class="tile-value">{value}</div><div class="tile-note">{note}</div></div>"#,
            esc(label)
        )
    };
    let failed = |count: usize, noun: &str| {
        if count == 0 {
            chip(Status::Ok, &format!("no {noun}"))
        } else {
            chip(Status::Crit, &format!("{count} {noun}"))
        }
    };
    let tiles = [
        tile(
            "Suites",
            summary.suites.to_string(),
            format!(
                "{} records",
                report
                    .suites
                    .iter()
                    .map(|suite| suite.records.len())
                    .sum::<usize>()
            ),
        ),
        tile(
            "Shots",
            summary.shots.to_string(),
            format!(
                "{} matri{}",
                summary.matrices,
                if summary.matrices == 1 { "x" } else { "ces" }
            ),
        ),
        tile(
            "Films",
            summary.films.to_string(),
            failed(summary.failed_films, "failed"),
        ),
        tile(
            "Style violations",
            summary.violations.to_string(),
            if summary.lints_with_errors == 0 {
                chip(Status::Ok, &format!("{} lints, no errors", summary.lints))
            } else {
                chip(
                    Status::Crit,
                    &format!(
                        "{} of {} lints with errors",
                        summary.lints_with_errors, summary.lints
                    ),
                )
            },
        ),
        tile(
            "Goldens",
            format!(
                "{}<span class=\"of\"> / {}</span>",
                summary.goldens - summary.failed_goldens,
                summary.goldens
            ),
            failed(summary.failed_goldens, "mismatched"),
        ),
        tile(
            "Benchmarks",
            summary.benches.to_string(),
            failed(summary.regressions, "regressed"),
        ),
    ]
    .concat();

    let mut needs = String::new();
    let mut sorted = attention.iter().collect::<Vec<_>>();
    sorted.sort_by(|a, b| b.status.cmp(&a.status).then(a.suite.cmp(&b.suite)));
    for item in sorted.iter().take(12) {
        // Benchmarks link to their row; everything else opens in the viewer.
        let target = if item.open.starts_with('#') {
            format!(r#"a class="row" href="{}""#, esc(&item.open))
        } else {
            format!(
                r#"button class="row" type="button" data-open="{}""#,
                esc(&item.open)
            )
        };
        let tag = if item.open.starts_with('#') {
            "a"
        } else {
            "button"
        };
        let _ = write!(
            needs,
            r#"<{target}><i class="dot {status}"></i><span class="where">{suite} <b>›</b> {name}</span><span class="what">{message}</span></{tag}>"#,
            status = status_class(item.status),
            suite = esc(&item.suite),
            name = esc(&item.name),
            message = esc(&item.message),
        );
    }
    if sorted.len() > 12 {
        let _ = write!(
            needs,
            r#"<div class="more">and {} more below</div>"#,
            sorted.len() - 12
        );
    }
    if needs.is_empty() {
        needs = r#"<div class="empty"><i class="dot ok"></i>Nothing is failing.</div>"#.into();
    }

    let mut slowest = report
        .benches
        .iter()
        .filter_map(|history| Some((history, history.runs.last()?)))
        .collect::<Vec<_>>();
    slowest.sort_by(|a, b| b.1.stats.p50.total_cmp(&a.1.stats.p50));
    let peak = slowest
        .first()
        .map_or(1., |(_, run)| run.stats.p50.max(1e-9));
    let mut slow = String::new();
    for (history, run) in slowest.iter().take(6) {
        let _ = write!(
            slow,
            r##"<a class="row" href="#bench-{id}"><span class="where">{name}</span><span class="meter"><i style="width:{pct:.1}%"></i></span><span class="mono num">{p50} ms</span>{badge}</a>"##,
            id = esc(&slug(&history.name)),
            name = esc(&history.name),
            pct = run.stats.p50 / peak * 100.,
            p50 = number(run.stats.p50),
            badge = verdict_badge(run),
        );
    }
    if slow.is_empty() {
        slow =
            r#"<div class="empty">No benchmarks yet. Run <code>just bench-ui</code>.</div>"#.into();
    }

    format!(
        r#"<section id="overview">
<div class="label">Overview</div>
<h1>Lightbox report</h1>
<p class="lede">{summary}</p>
<div class="tiles">{tiles}</div>
<div class="split">
  <div class="panel"><div class="panel-head"><h3>Needs attention</h3><span class="count-chip">{count}</span></div>{needs}</div>
  <div class="panel"><div class="panel-head"><h3>Slowest benchmarks</h3><span class="hint">median frame</span></div>{slow}</div>
</div>
</section>"#,
        summary = esc(&summary.to_string()),
        count = attention.len(),
    )
}

fn latest_verdict(history: &BenchHistory) -> Option<Verdict> {
    Some(history.runs.last()?.comparison.as_ref()?.verdict)
}

fn verdict_badge(run: &BenchRun) -> String {
    match &run.comparison {
        None => r#"<span class="badge neutral">first run</span>"#.into(),
        Some(comparison) => {
            let (class, arrow) = match comparison.verdict {
                Verdict::Regression => ("crit", "▲"),
                Verdict::Improvement => ("ok", "▼"),
                Verdict::Unchanged => ("neutral", "≈"),
            };
            format!(
                r#"<span class="badge {class}" title="{title}">{arrow} {change:+.1}%</span>"#,
                title = esc(&comparison.to_string()),
                change = comparison.p50_change * 100.,
            )
        }
    }
}

/// Phases in Loupe's fixed order, with their ramp classes.
const PHASES: [(&str, &str); 4] = [
    ("render", "ph-render"),
    ("layout", "ph-layout"),
    ("prepaint", "ph-prepaint"),
    ("paint", "ph-paint"),
];

fn benches_section(benches: &[BenchHistory], attention: &mut Vec<Attention>) -> String {
    let mut html = String::from(
        r#"<section class="suite" id="benchmarks"><header class="suite-head"><div><div class="label">Performance</div><h2>Benchmarks</h2><p class="desc">Per-frame draw time (render, layout, prepaint and paint) for each benchmark's latest run, compared with the previous run of the same build profile.</p></div></header>"#,
    );
    if benches.is_empty() {
        html.push_str(r#"<div class="empty">No benchmarks yet. Run <code>just bench-ui</code>.</div></section>"#);
        return html;
    }
    html.push_str(r#"<div class="bench-table"><div class="bench-row head"><span>Benchmark</span><span class="num">Frames</span><span class="num">p50</span><span class="num">p95</span><span class="num">p99</span><span class="num">max</span><span class="num">mean ± sd</span><span>History</span><span>Change</span><span>Phases</span></div>"#);
    for history in benches {
        let Some(run) = history.runs.last() else {
            continue;
        };
        if latest_verdict(history) == Some(Verdict::Regression) {
            attention.push(Attention {
                status: Status::Crit,
                suite: "benchmarks".into(),
                name: history.name.clone(),
                message: run
                    .comparison
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                open: format!("#bench-{}", slug(&history.name)),
            });
        }
        let same_profile = history
            .runs
            .iter()
            .filter(|previous| previous.run.profile == run.run.profile)
            .collect::<Vec<_>>();
        let recent = &same_profile[same_profile.len().saturating_sub(24)..];
        let spark = svg::sparkline(
            &recent.iter().map(|run| run.stats.p50).collect::<Vec<_>>(),
            &recent
                .iter()
                .map(|run| format!("{} · {}", run.run.git_sha, run.run.timestamp))
                .collect::<Vec<_>>(),
            120.,
            24.,
        );
        let phases = run.phases.as_ref().map_or_else(
            || r#"<span class="muted" title="Per-phase timings come from the engine's inspector capture, which didn't record any.">not recorded</span>"#.to_string(),
            |phases| {
                let segments = PHASES
                    .iter()
                    .filter_map(|(phase, class)| {
                        Some(Segment {
                            label: phase,
                            class,
                            value: phases.get(*phase)?.p50,
                        })
                    })
                    .collect::<Vec<_>>();
                svg::stacked_bar(&segments, 120., 10.)
            },
        );
        let stats = &run.stats;
        let history_chart = svg::lines(
            &[
                Series {
                    label: "p50",
                    class: "s1",
                    values: recent.iter().map(|run| Some(run.stats.p50)).collect(),
                },
                Series {
                    label: "p95",
                    class: "s2",
                    values: recent.iter().map(|run| Some(run.stats.p95)).collect(),
                },
            ],
            &recent
                .iter()
                .map(|run| run.run.git_sha.chars().take(7).collect())
                .collect::<Vec<_>>(),
            " ms",
            440.,
            168.,
        );
        let histogram = svg::histogram(&run.samples, stats.p50, stats.p95, 440., 168.);
        let phase_legend = run.phases.as_ref().map_or_else(String::new, |phases| {
            let mut legend = String::from(r#"<div class="legend">"#);
            for (phase, class) in PHASES {
                if let Some(stats) = phases.get(phase) {
                    let _ = write!(
                        legend,
                        r#"<span><i class="key {class}"></i>{phase} {} ms</span>"#,
                        number(stats.p50)
                    );
                }
            }
            legend.push_str("</div>");
            legend
        });
        let settings = &run.settings;
        let _ = write!(
            html,
            r#"<details class="bench" id="bench-{id}" data-name="{filter}"><summary class="bench-row">
<span class="strong">{name}</span><span class="num">{count}</span><span class="num strong">{p50}</span><span class="num">{p95}</span><span class="num">{p99}</span><span class="num">{max}</span><span class="num">{mean} ± {sd}</span><span>{spark}</span><span>{badge}</span><span>{phases}</span></summary>
<div class="bench-body">
  <figure><figcaption>Frame times, this run</figcaption>{histogram}</figure>
  <figure><figcaption>History ({profile} runs)</figcaption>{history_chart}</figure>
  <div class="facts">{phase_legend}<dl>
    <dt>commit</dt><dd class="mono">{sha}{branch}</dd>
    <dt>when</dt><dd class="mono">{time}</dd>
    <dt>profile</dt><dd>{profile}</dd>
    <dt>window</dt><dd class="mono">{w}×{h} @{scale}×</dd>
    <dt>frames</dt><dd>{warmup} warm-up + {iterations} measured, {refresh}</dd>
    <dt>vs previous</dt><dd>{comparison}</dd>
  </dl></div>
</div></details>"#,
            id = esc(&slug(&history.name)),
            filter = esc(&history.name.to_lowercase()),
            name = esc(&history.name),
            count = stats.count,
            p50 = number(stats.p50),
            p95 = number(stats.p95),
            p99 = number(stats.p99),
            max = number(stats.max),
            mean = number(stats.mean),
            sd = number(stats.stddev),
            badge = verdict_badge(run),
            profile = esc(&run.run.profile),
            sha = esc(&run.run.git_sha),
            branch = run
                .run
                .git_branch
                .as_ref()
                .map(|branch| format!(" · {}", esc(branch)))
                .unwrap_or_default(),
            time = esc(&run.run.timestamp),
            w = trim(settings.size[0]),
            h = trim(settings.size[1]),
            scale = trim(settings.scale),
            warmup = settings.warmup,
            iterations = settings.iterations,
            refresh = if settings.refresh {
                "every view re-rendered"
            } else {
                "only what each step invalidated"
            },
            comparison = run.comparison.as_ref().map_or_else(
                || "first run of this profile".to_string(),
                |comparison| esc(&comparison.to_string())
            ),
        );
    }
    html.push_str("</div></section>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup() {
        assert_eq!(
            esc(r#"<a href="x">&'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }
}
