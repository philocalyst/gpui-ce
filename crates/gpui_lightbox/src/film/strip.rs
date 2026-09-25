//! The film strip: every frame with its time and what changed, then one
//! curve per tracked property, the findings and the assertions — one PNG an
//! agent can read top to bottom.

use super::{FrameMetrics, Track};
use crate::{
    color::Color,
    compose::{Composer, backdrop, heading, label, mono, picture, pill, placed, resample, section},
    manifest::{Assertion, Finding},
    shot::{Rect, Shot, trim},
    theme::{DARK, SERIES},
};
use anyhow::Result;
use gpui::{
    AnyElement, Bounds, IntoElement, ParentElement as _, PathBuilder, Pixels, RenderImage,
    Styled as _, canvas, div, point, px,
};
use image::RgbaImage;
use std::{rc::Rc, sync::Arc};

const MARGIN: f32 = 24.;
const CONTENT: f32 = 1152.;
const GAP: f32 = 12.;
const CAPTION: f32 = 22.;
const CHART: f32 = 96.;
const GUTTER: f32 = 148.;
const ROW: f32 = 22.;
const SCALE: f32 = 2.;

/// What a strip shows.
pub(super) struct Strip<'a> {
    pub(super) name: &'a str,
    pub(super) frames: &'a [Shot],
    pub(super) metrics: &'a [FrameMetrics],
    pub(super) tracks: &'a [Track],
    pub(super) findings: &'a [Finding],
    pub(super) assertions: &'a [Assertion],
}

/// Lays out and renders a strip.
pub(super) fn render(composer: &mut Composer, strip: Strip<'_>) -> Result<RgbaImage> {
    let Some(first) = strip.frames.first() else {
        return composer.render(64., 64., 1., |_, _| backdrop().into_any_element());
    };
    let [frame_w, frame_h] = first.meta.size;
    let columns = ((CONTENT + GAP) / (frame_w.min(232.) + GAP))
        .floor()
        .clamp(1., strip.frames.len() as f32) as usize;
    let thumb_w = ((CONTENT - GAP * (columns as f32 - 1.)) / columns as f32).floor();
    let thumb_h = (thumb_w * frame_h / frame_w).round();
    let thumbs = strip
        .frames
        .iter()
        .map(|frame| {
            composer.image(&resample(
                &frame.image,
                (thumb_w * SCALE) as u32,
                (thumb_h * SCALE) as u32,
            ))
        })
        .collect();
    let data = Rc::new(StripData {
        name: strip.name.to_string(),
        subtitle: format!(
            "{}×{} @{}× · {}",
            trim(frame_w),
            trim(frame_h),
            trim(first.scale()),
            first.meta.appearance.label()
        ),
        frame_w,
        columns,
        thumb: (thumb_w, thumb_h),
        thumbs,
        metrics: strip.metrics.to_vec(),
        tracks: strip.tracks.to_vec(),
        findings: strip.findings.to_vec(),
        assertions: strip.assertions.to_vec(),
    });
    let (_, height) = build(&data);
    let width = CONTENT + MARGIN * 2.;
    let scale = SCALE.min(16_000. / height.max(width));
    composer.render(width, height, scale, move |_, _| {
        backdrop().children(build(&data).0).into_any_element()
    })
}

/// A strip's data, owned so the composition can be rebuilt on every render.
struct StripData {
    name: String,
    subtitle: String,
    frame_w: f32,
    columns: usize,
    thumb: (f32, f32),
    thumbs: Vec<Arc<RenderImage>>,
    metrics: Vec<FrameMetrics>,
    tracks: Vec<Track>,
    findings: Vec<Finding>,
    assertions: Vec<Assertion>,
}

/// The strip's elements and its total height.
fn build(data: &StripData) -> (Vec<AnyElement>, f32) {
    let (thumb_w, thumb_h) = data.thumb;
    let zoom = thumb_w / data.frame_w;
    let rows = data.metrics.len().div_ceil(data.columns) as f32;

    let mut children: Vec<AnyElement> = Vec::new();
    let mut y = MARGIN;
    children.push(header(data, y));
    y += 56.;
    children.push(rule(y));
    y += 16.;

    // Frames, each outlined where it changed since the previous one.
    let max_energy = data
        .metrics
        .iter()
        .map(|metrics| metrics.energy)
        .fold(0f32, f32::max);
    for (ix, (thumb, metrics)) in data.thumbs.iter().zip(&data.metrics).enumerate() {
        let column = (ix % data.columns) as f32;
        let row = (ix / data.columns) as f32;
        let x = MARGIN + column * (thumb_w + GAP);
        let top = y + row * (thumb_h + CAPTION + GAP);
        let bounds = Rect::new(x, top, thumb_w, thumb_h);
        children.push(picture(thumb.clone(), bounds));
        children.push(
            placed(bounds.inflate(1.))
                .border_1()
                .border_color(DARK.line_strong)
                .into_any_element(),
        );
        if let Some(changed) = metrics.changed {
            let changed = Rect::new(
                x + changed.x * zoom,
                top + changed.y * zoom,
                (changed.w * zoom).max(2.),
                (changed.h * zoom).max(2.),
            );
            children.push(
                placed(changed.inflate(1.).intersect(&bounds))
                    .border_1()
                    .border_color(DARK.accent.with_alpha(0.8))
                    .into_any_element(),
            );
        }
        children.push(caption(
            metrics,
            max_energy,
            Rect::new(x, top + thumb_h + 4., thumb_w, CAPTION - 4.),
        ));
    }
    y += rows * (thumb_h + CAPTION + GAP) + 4.;

    // Curves: tracked properties, then motion energy.
    let motion = Track {
        name: "motion".into(),
        values: data
            .metrics
            .iter()
            .map(|metrics| Some(metrics.energy * 100.))
            .collect(),
    };
    let times = data
        .metrics
        .iter()
        .map(|metrics| metrics.t_ms as f32)
        .collect::<Rc<[f32]>>();
    children.push(rule(y));
    y += 16.;
    children.push(section_at("Curves", y));
    y += 24.;
    for (ix, track) in data.tracks.iter().chain([&motion]).enumerate() {
        let is_motion = ix == data.tracks.len();
        let color = if is_motion {
            DARK.text_faint
        } else {
            SERIES[ix % SERIES.len()]
        };
        // Track findings mark their curve; freezes (no track) mark motion.
        let marks = data
            .findings
            .iter()
            .filter(|finding| match &finding.track {
                Some(name) => !is_motion && *name == track.name,
                None => is_motion,
            })
            .map(|finding| {
                let color = if finding.kind.is_problem() {
                    DARK.crit
                } else {
                    DARK.warn
                };
                (finding.frame, color)
            })
            .collect::<Vec<_>>();
        children.extend(chart(track, &times, &marks, color, is_motion, y));
        y += CHART + 12.;
    }
    children.push(axis(&times, y - 8.));
    y += 20.;

    // Findings and assertions.
    if !data.findings.is_empty() {
        children.push(rule(y));
        y += 16.;
        children.push(section_at("Findings", y));
        y += 24.;
        for finding in &data.findings {
            let color = if finding.kind.is_problem() {
                DARK.crit
            } else {
                DARK.warn
            };
            children.push(list_row(finding.kind.label(), &finding.message, color, y));
            y += ROW;
        }
        y += 8.;
    }
    if !data.assertions.is_empty() {
        children.push(rule(y));
        y += 16.;
        children.push(section_at("Assertions", y));
        y += 24.;
        for assertion in &data.assertions {
            let (mark, color) = if assertion.passed {
                ("pass", DARK.ok)
            } else {
                ("fail", DARK.crit)
            };
            let text = format!("{}  —  {}", assertion.name, assertion.message);
            children.push(list_row(mark, &text, color, y));
            y += ROW;
        }
        y += 8.;
    }
    (children, y + MARGIN - 8.)
}

fn header(data: &StripData, y: f32) -> AnyElement {
    let duration = data.metrics.last().map_or(0., |metrics| metrics.t_ms);
    let settled = data
        .metrics
        .iter()
        .rev()
        .find(|metrics| metrics.changed_pixels > 0)
        .map_or(0., |metrics| metrics.t_ms);
    let failed = data
        .assertions
        .iter()
        .filter(|assertion| !assertion.passed)
        .count();
    let mut pills = div()
        .flex()
        .gap(px(8.))
        .child(pill(
            format!(
                "{} frames · {} ms",
                data.metrics.len(),
                trim(duration as f32)
            ),
            DARK.accent,
        ))
        .child(pill(
            format!("settled {} ms", trim(settled as f32)),
            DARK.text_faint,
        ));
    if !data.findings.is_empty() {
        let problem = data
            .findings
            .iter()
            .any(|finding| finding.kind.is_problem());
        pills = pills.child(pill(
            format!(
                "{} finding{}",
                data.findings.len(),
                plural(data.findings.len())
            ),
            if problem { DARK.crit } else { DARK.warn },
        ));
    }
    if !data.assertions.is_empty() {
        pills = pills.child(if failed > 0 {
            pill(format!("{failed} failed"), DARK.crit)
        } else {
            pill(
                format!(
                    "{} assertion{} pass",
                    data.assertions.len(),
                    plural(data.assertions.len())
                ),
                DARK.ok,
            )
        });
    }
    placed(Rect::new(MARGIN, y, CONTENT, 48.))
        .flex()
        .justify_between()
        .items_center()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(section("Film"))
                .child(heading(data.name.clone(), 18., DARK.text))
                .child(mono(data.subtitle.clone(), 11., DARK.text_muted)),
        )
        .child(pills)
        .into_any_element()
}

fn caption(metrics: &FrameMetrics, max_energy: f32, bounds: Rect) -> AnyElement {
    let bar = if max_energy > 0. {
        let floor = if metrics.changed_pixels > 0 { 2. } else { 0. };
        (metrics.energy / max_energy * 40.).max(floor)
    } else {
        0.
    };
    let changed = if metrics.changed_pixels == 0 && metrics.index > 0 {
        "still".to_string()
    } else {
        compact(metrics.changed_pixels)
    };
    placed(bounds)
        .flex()
        .items_center()
        .gap(px(6.))
        .child(mono(format!("#{:02}", metrics.index), 11., DARK.text_faint))
        .child(mono(
            format!("{} ms", trim(metrics.t_ms as f32)),
            11.,
            DARK.text,
        ))
        .child(div().flex_1())
        .child(mono(changed, 10.5, DARK.text_muted))
        .child(
            div()
                .w(px(40.))
                .h(px(3.))
                .bg(DARK.line)
                .child(div().w(px(bar)).h_full().bg(DARK.accent)),
        )
        .into_any_element()
}

fn chart(
    track: &Track,
    times: &Rc<[f32]>,
    marks: &[(usize, Color)],
    color: Color,
    is_motion: bool,
    y: f32,
) -> Vec<AnyElement> {
    let mut children = Vec::new();
    let (min, max) = track.range().unwrap_or((0., 0.));
    let (lo, hi) = if is_motion {
        (0., max.max(1e-3))
    } else if (max - min).abs() < 1e-6 {
        (min - 1., max + 1.)
    } else {
        (min, max)
    };
    let subtitle = if is_motion {
        "pixel change, %".to_string()
    } else {
        match (track.first(), track.last()) {
            (Some(first), Some(last)) => format!("{} → {}", trim(first), trim(last)),
            _ => "not measured".into(),
        }
    };
    children.push(
        placed(Rect::new(MARGIN, y, GUTTER - 16., CHART))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().size(px(8.)).rounded(px(4.)).bg(color))
                    .child(heading(track.name.clone(), 12., DARK.text)),
            )
            .child(mono(subtitle, 11., DARK.text_muted))
            .child(mono(
                format!("range {} … {}", trim(min), trim(max)),
                10.5,
                DARK.text_faint,
            ))
            .into_any_element(),
    );
    let area = Rect::new(MARGIN + GUTTER, y, CONTENT - GUTTER, CHART);
    children.push(
        placed(area)
            .bg(DARK.surface)
            .border_1()
            .border_color(DARK.line)
            .into_any_element(),
    );
    let plot = Rect::new(area.x + 8., area.y + 10., area.w - 16., area.h - 20.);
    let end = times.last().copied().unwrap_or(1.).max(1e-3);
    let points = track
        .values
        .iter()
        .zip(times.iter())
        .enumerate()
        .filter_map(|(ix, (value, t))| {
            let value = (*value)?;
            let x = plot.x + t / end * plot.w;
            let y = plot.bottom() - (value - lo) / (hi - lo) * plot.h;
            Some((ix, x, y))
        })
        .collect::<Vec<_>>();
    for guide in [plot.y, plot.bottom()] {
        children.push(
            placed(Rect::new(plot.x, guide.round(), plot.w, 1.))
                .bg(DARK.line)
                .into_any_element(),
        );
    }
    let line = points.iter().map(|(_, x, y)| (*x, *y)).collect::<Vec<_>>();
    children.push(
        placed(area)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds: Bounds<Pixels>, _, window, _| {
                        if line.len() < 2 {
                            return;
                        }
                        let mut path = PathBuilder::stroke(px(2.));
                        for (ix, (x, y)) in line.iter().enumerate() {
                            let at = point(
                                bounds.origin.x + px(x - area.x),
                                bounds.origin.y + px(y - area.y),
                            );
                            if ix == 0 {
                                path.move_to(at);
                            } else {
                                path.line_to(at);
                            }
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element(),
    );
    for (ix, x, y) in points {
        let mark = marks
            .iter()
            .find(|(frame, _)| *frame == ix)
            .map(|(_, color)| *color);
        // Every frame is a marker with a 2 px ring in the chart's surface
        // color; flagged frames are larger and wear their finding's color.
        let radius = if mark.is_some() { 6. } else { 4. };
        children.push(
            placed(Rect::new(x - radius, y - radius, radius * 2., radius * 2.))
                .rounded(px(radius))
                .bg(mark.unwrap_or(color))
                .border_2()
                .border_color(DARK.surface)
                .into_any_element(),
        );
    }
    children
}

fn axis(times: &[f32], y: f32) -> AnyElement {
    let end = times.last().copied().unwrap_or(0.);
    let plot_x = MARGIN + GUTTER + 8.;
    let plot_w = CONTENT - GUTTER - 16.;
    let mut row = placed(Rect::new(plot_x - 40., y, plot_w + 80., 16.));
    for step in 0..=4 {
        let t = end * step as f32 / 4.;
        let x = step as f32 / 4. * plot_w;
        row = row.child(
            div()
                .absolute()
                .left(px(x))
                .w(px(80.))
                .flex()
                .justify_center()
                .child(mono(format!("{} ms", trim(t)), 10.5, DARK.text_faint)),
        );
    }
    row.into_any_element()
}

fn list_row(tag: &str, text: &str, color: Color, y: f32) -> AnyElement {
    placed(Rect::new(MARGIN, y, CONTENT, ROW))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().size(px(8.)).rounded(px(4.)).bg(color))
        .child(div().w(px(96.)).child(mono(tag.to_string(), 11., color)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(label(text.to_string(), 12., DARK.text)),
        )
        .into_any_element()
}

fn section_at(text: &str, y: f32) -> AnyElement {
    placed(Rect::new(MARGIN, y, CONTENT, 16.))
        .child(section(text))
        .into_any_element()
}

fn rule(y: f32) -> AnyElement {
    placed(Rect::new(0., y, CONTENT + MARGIN * 2., 1.))
        .bg(DARK.line)
        .into_any_element()
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// `1234` → `1.2k px`.
fn compact(pixels: u64) -> String {
    if pixels >= 1_000_000 {
        format!("{:.1}M px", pixels as f64 / 1e6)
    } else if pixels >= 1_000 {
        format!("{:.1}k px", pixels as f64 / 1e3)
    } else {
        format!("{pixels} px")
    }
}
