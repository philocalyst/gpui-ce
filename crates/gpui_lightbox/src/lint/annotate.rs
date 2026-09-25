//! The annotated lint image: the shot with every violation boxed and
//! numbered, and a legend explaining each number.

use super::{LintCounts, Severity, StyleSpec, Violation};
use crate::{
    color::Color,
    compose::{
        Composer, backdrop, badge, heading, label, mono, picture, pill, placed, resample, section,
    },
    shot::{Rect, Shot, trim},
    theme::DARK,
};
use anyhow::Result;
use gpui::{AnyElement, IntoElement, ParentElement as _, RenderImage, Styled as _, div, px};
use image::RgbaImage;
use std::{rc::Rc, sync::Arc};

const MARGIN: f32 = 24.;
const HEADER: f32 = 64.;
const LEGEND: f32 = 440.;
const MAX_IMAGE: f32 = 1100.;
/// Average advance of 12 px IBM Plex Sans, for estimating wrapped lines.
const CHARS_PER_LINE: f32 = (LEGEND - 48.) / 6.4;

struct Annotation {
    title: String,
    subtitle: String,
    image: Arc<RenderImage>,
    frame: Rect,
    zoom: f32,
    violations: Vec<Violation>,
    checked: LintCounts,
}

/// Renders the annotated image for `violations` found in `shot`.
pub(super) fn render(
    composer: &mut Composer,
    shot: &Shot,
    spec: &StyleSpec,
    violations: &[Violation],
    checked: &LintCounts,
) -> Result<RgbaImage> {
    let [w, h] = shot.meta.size;
    let zoom = (MAX_IMAGE / w).min(1.);
    let scale = shot.scale().clamp(1., 2.);
    let frame = Rect::new(
        MARGIN,
        MARGIN + HEADER,
        (w * zoom).round(),
        (h * zoom).round(),
    );
    let image = composer.image(&resample(
        &shot.image,
        (frame.w * scale) as u32,
        (frame.h * scale) as u32,
    ));
    let annotation = Rc::new(Annotation {
        title: shot.name.clone(),
        subtitle: format!(
            "{} · {}×{} @{}× · {} texts · {} quads",
            spec.name,
            trim(w),
            trim(h),
            trim(shot.scale()),
            checked.texts,
            checked.quads
        ),
        image,
        frame,
        zoom,
        violations: violations.to_vec(),
        checked: checked.clone(),
    });
    let legend_height = legend_height(&annotation);
    let width = frame.right() + 32. + LEGEND + MARGIN;
    let height = frame.bottom().max(frame.y + legend_height) + MARGIN;
    composer.render(width, height, scale, move |_, _| {
        backdrop()
            .children(elements(&annotation, width))
            .into_any_element()
    })
}

fn color(severity: Severity) -> Color {
    match severity {
        Severity::Error => DARK.crit,
        Severity::Warning => DARK.warn,
    }
}

fn message_lines(violation: &Violation) -> f32 {
    (violation.message.chars().count() as f32 / CHARS_PER_LINE)
        .ceil()
        .max(1.)
}

fn row_height(violation: &Violation) -> f32 {
    22. + message_lines(violation) * 16. + 10.
}

fn legend_height(annotation: &Annotation) -> f32 {
    let rows = if annotation.violations.is_empty() {
        72.
    } else {
        annotation.violations.iter().map(row_height).sum()
    };
    rows + 8. + footer_height(annotation)
}

fn elements(annotation: &Annotation, width: f32) -> Vec<AnyElement> {
    let frame = annotation.frame;
    let errors = annotation
        .violations
        .iter()
        .filter(|violation| violation.severity == Severity::Error)
        .count();
    let count = annotation.violations.len();
    let verdict = match (count, errors) {
        (0, _) => pill("clean", DARK.ok),
        (_, 0) => pill(format!("{count} warning{}", plural(count)), DARK.warn),
        _ => pill(format!("{count} violation{}", plural(count)), DARK.crit),
    };
    let mut children = vec![
        placed(Rect::new(MARGIN, MARGIN, width - MARGIN * 2., 48.))
            .flex()
            .justify_between()
            .items_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section("Style lint"))
                    .child(heading(annotation.title.clone(), 18., DARK.text))
                    .child(mono(annotation.subtitle.clone(), 11., DARK.text_muted)),
            )
            .child(verdict)
            .into_any_element(),
        placed(Rect::new(0., MARGIN + HEADER - 12., width, 1.))
            .bg(DARK.line)
            .into_any_element(),
        placed(frame.inflate(1.))
            .border_1()
            .border_color(DARK.line_strong)
            .into_any_element(),
        picture(annotation.image.clone(), frame),
    ];

    // Boxes first, then badges, so every number stays readable.
    let boxes = annotation
        .violations
        .iter()
        .map(|violation| {
            let bounds = violation.bounds;
            Rect::new(
                frame.x + bounds.x * annotation.zoom,
                frame.y + bounds.y * annotation.zoom,
                (bounds.w * annotation.zoom).max(2.),
                (bounds.h * annotation.zoom).max(2.),
            )
            .inflate(2.)
        })
        .collect::<Vec<_>>();
    for (violation, bounds) in annotation.violations.iter().zip(&boxes) {
        let color = color(violation.severity);
        children.push(
            placed(*bounds)
                .border_2()
                .border_color(color)
                .bg(color.with_alpha(0.12))
                .rounded(px(2.))
                .into_any_element(),
        );
    }
    // Each badge sits just left of its box, level with its first line, so it
    // never covers what it points at; boxes at the left edge get it above.
    for (ix, (violation, bounds)) in annotation.violations.iter().zip(&boxes).enumerate() {
        let badge = badge(ix + 1, color(violation.severity));
        let slot = if bounds.x - 26. >= frame.x - 20. {
            // A slot ending 3 px left of the box, the badge flush right in it.
            placed(Rect::new(
                bounds.x - 43.,
                bounds.y + bounds.h.min(24.) / 2. - 9.,
                40.,
                18.,
            ))
            .flex()
            .justify_end()
        } else {
            placed(Rect::new(
                bounds.x,
                (bounds.y - 21.).max(frame.y - 18.),
                40.,
                18.,
            ))
            .flex()
        };
        children.push(slot.child(badge).into_any_element());
    }

    // Legend.
    let legend_x = frame.right() + 32.;
    let mut y = frame.y;
    if annotation.violations.is_empty() {
        children.push(
            placed(Rect::new(legend_x, y, LEGEND, 64.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(heading("No violations", 16., DARK.ok))
                .child(label(
                    format!(
                        "{} texts and {} quads match the spec.",
                        annotation.checked.texts, annotation.checked.quads
                    ),
                    12.,
                    DARK.text_muted,
                ))
                .into_any_element(),
        );
        y += 72.;
    }
    for (ix, violation) in annotation.violations.iter().enumerate() {
        let height = row_height(violation);
        let color = color(violation.severity);
        children.push(
            placed(Rect::new(legend_x, y, LEGEND, height))
                .flex()
                .gap(px(10.))
                .child(badge(ix + 1, color))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(mono(violation.rule.name(), 11., color))
                                .child(div().flex_1().min_w_0().child(heading(
                                    violation.subject.clone(),
                                    12.,
                                    DARK.text,
                                ))),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(DARK.text_muted)
                                .child(violation.message.clone()),
                        ),
                )
                .into_any_element(),
        );
        y += height;
    }
    let rules = annotation
        .checked
        .rules
        .iter()
        .map(|rule| rule.name())
        .collect::<Vec<_>>()
        .join(" · ");
    let mut footer = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .pt(px(8.))
        .border_t_1()
        .border_color(DARK.line)
        .child(note(format!("Checked: {rules}"), DARK.text_faint));
    for skipped in &annotation.checked.skipped {
        footer = footer.child(note(format!("Skipped {skipped}"), DARK.warn));
    }
    children.push(
        placed(Rect::new(
            legend_x,
            y + 4.,
            LEGEND,
            footer_height(annotation),
        ))
        .child(footer)
        .into_any_element(),
    );
    children
}

/// Wrapping 11 px text.
fn note(text: String, color: Color) -> gpui::Div {
    div()
        .text_size(px(11.))
        .line_height(px(15.))
        .text_color(color)
        .child(text)
}

fn footer_height(annotation: &Annotation) -> f32 {
    let lines = |text_len: usize| (text_len as f32 / (CHARS_PER_LINE * 1.1)).ceil().max(1.);
    let rules = annotation.checked.rules.len() * 15 + 9;
    let skipped = annotation
        .checked
        .skipped
        .iter()
        .map(|skipped| lines(skipped.len() + 8))
        .sum::<f32>();
    12. + (lines(rules) + skipped) * 15. + annotation.checked.skipped.len() as f32 * 4.
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
