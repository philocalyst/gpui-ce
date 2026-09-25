//! The lint rules: pure functions from what a frame painted (and its pixels)
//! to violations.

use super::{LintCounts, Rule, SpacingRule, StyleSpec, Violation};
use crate::{
    color::Color,
    shot::{QuadInfo, Rect, Shot, TextLine, merge_quads, trim},
};
use image::RgbaImage;
use std::collections::HashMap;

/// Everything a lint looks at.
pub struct LintInput<'a> {
    /// Painted text.
    pub texts: &'a [TextLine],
    /// Painted boxes: quads merged by bounds (see [`merge_quads`]).
    pub quads: Vec<QuadInfo>,
    /// The rendered pixels.
    pub image: &'a RgbaImage,
    /// Device pixels per logical pixel.
    pub scale: f32,
    /// The window's bounds.
    pub window: Rect,
    /// The family `.SystemUIFont` resolved to.
    pub system_font: &'a str,
    /// Clickable elements' visible bounds, when the element tree was recorded.
    pub clickables: Option<&'a [Rect]>,
}

impl<'a> LintInput<'a> {
    /// The input for a shot.
    pub fn from_shot(shot: &'a Shot) -> Self {
        Self {
            texts: &shot.texts,
            quads: merge_quads(&shot.quads),
            image: &shot.image,
            scale: shot.scale(),
            window: shot.bounds(),
            system_font: &shot.meta.system_font,
            clickables: shot.clickables.as_deref(),
        }
    }
}

/// Runs every rule `spec` enables. Violations come in reading order.
pub fn check(spec: &StyleSpec, input: &LintInput<'_>) -> (Vec<Violation>, LintCounts) {
    let exempt = |rect: &Rect| {
        let (x, y) = rect.center();
        spec.ignore_regions
            .iter()
            .any(|region| region.contains(x, y))
    };
    let texts = input
        .texts
        .iter()
        .filter(|line| line.is_visible())
        .filter(|line| !spec.ignore_text.contains(&line.text))
        .filter(|line| !exempt(&line.visible))
        .collect::<Vec<_>>();
    let quads = input
        .quads
        .iter()
        .filter(|quad| quad.is_painted() && !quad.visible.is_empty())
        .filter(|quad| !exempt(&quad.visible))
        .collect::<Vec<_>>();

    let mut counts = LintCounts {
        texts: texts.len(),
        quads: quads.len(),
        ..LintCounts::default()
    };
    let mut ran = |rule: Rule, enabled: bool| {
        if enabled {
            counts.rules.push(rule);
        }
        enabled
    };
    let roles = ran(Rule::FontFamily, !spec.text_roles.is_empty());
    if roles {
        ran(Rule::FontSize, true);
        ran(Rule::FontWeight, true);
    }
    let palette = ran(Rule::TextColor, !spec.text_colors.is_empty());
    let contrast = ran(Rule::Contrast, spec.min_contrast.is_some());
    let clipped = ran(Rule::ClippedText, spec.no_clipped_text);
    let outside = ran(Rule::OutsideWindow, spec.no_content_outside_window);
    let overlap = ran(Rule::TextOverlap, spec.no_overlapping_text);
    let baseline = ran(Rule::Baseline, spec.baseline_tolerance.is_some());
    let spacing = ran(Rule::Spacing, spec.spacing.is_some());
    let radii = ran(Rule::CornerRadius, spec.corner_radii.is_some());
    let borders = ran(Rule::BorderWidth, spec.border_widths.is_some());
    let hit_targets = match (spec.min_hit_target, input.clickables) {
        (Some(_), Some(clickables)) => {
            counts.hit_targets = Some(clickables.len());
            ran(Rule::HitTarget, true)
        }
        (Some(_), None) => {
            counts.skipped.push(
                "hit-target: no element tree recorded (call stage.record_elements() first)".into(),
            );
            false
        }
        (None, _) => false,
    };

    let mut violations = Vec::new();
    for line in &texts {
        let cut_by_window = outside && beyond(&line.bounds, &line.visible, &input.window);
        if roles {
            violations.extend(font(spec, line, input.system_font));
        }
        if palette {
            violations.extend(text_color(spec, line));
        }
        if contrast {
            violations.extend(text_contrast(spec, line, input));
        }
        if cut_by_window {
            violations.push(text_violation(
                Rule::OutsideWindow,
                line,
                format!(
                    "extends past the window edge; {} of {} px visible",
                    trim(line.visible.w),
                    trim(line.bounds.w)
                ),
            ));
        } else if clipped {
            violations.extend(clipped_text(spec, line));
        }
    }
    if overlap {
        violations.extend(overlaps(&texts));
    }
    if let Some(tolerance) = spec.baseline_tolerance.filter(|_| baseline) {
        violations.extend(baselines(&texts, tolerance));
    }
    if let Some(rule) = spec.spacing.as_ref().filter(|_| spacing) {
        violations.extend(self::spacing(rule, &quads, &texts, &input.window));
    }
    for quad in &quads {
        if let Some(allowed) = spec.corner_radii.as_ref().filter(|_| radii) {
            violations.extend(corner_radius(allowed, spec.allow_pills, quad));
        }
        if let Some(allowed) = spec.border_widths.as_ref().filter(|_| borders) {
            violations.extend(border_width(allowed, quad));
        }
        if outside && beyond(&quad.bounds, &quad.visible, &input.window) {
            violations.push(Violation {
                rule: Rule::OutsideWindow,
                severity: Rule::OutsideWindow.severity(),
                subject: quad_subject(quad),
                message: format!("extends past the window edge to {}", quad.bounds),
                bounds: quad.visible,
            });
        }
    }
    if let (Some(min), Some(clickables)) = (spec.min_hit_target, input.clickables)
        && hit_targets
    {
        violations.extend(small_targets(min, clickables));
    }

    violations.sort_by(|a, b| {
        let key = |violation: &Violation| ((violation.bounds.y / 4.).floor(), violation.bounds.x);
        key(a)
            .partial_cmp(&key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    (violations, counts)
}

fn text_subject(line: &TextLine) -> String {
    let text = line.text.trim();
    if text.chars().count() > 40 {
        format!("“{}…”", text.chars().take(39).collect::<String>())
    } else {
        format!("“{text}”")
    }
}

fn quad_subject(quad: &QuadInfo) -> String {
    format!("quad {}×{}", trim(quad.bounds.w), trim(quad.bounds.h))
}

fn text_violation(rule: Rule, line: &TextLine, message: String) -> Violation {
    Violation {
        rule,
        severity: rule.severity(),
        subject: text_subject(line),
        message,
        bounds: line.visible,
    }
}

fn list(values: &[f32]) -> String {
    values
        .iter()
        .map(|value| trim(*value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

/// Family, then size and weight within the roles that allow the family.
pub fn font(spec: &StyleSpec, line: &TextLine, system_font: &str) -> Option<Violation> {
    let family = if line.font_family == ".SystemUIFont" {
        system_font
    } else {
        &line.font_family
    };
    let roles = spec
        .text_roles
        .iter()
        .filter(|role| role.families.iter().any(|allowed| allowed == family))
        .collect::<Vec<_>>();
    if roles.is_empty() {
        let allowed = spec
            .text_roles
            .iter()
            .map(|role| format!("{}: {}", role.name, role.families.join(", ")))
            .collect::<Vec<_>>()
            .join("; ");
        // gpui reports the family a line was shaped with; its own fallback
        // families only appear when the requested font isn't available.
        let message = if family.starts_with(".Zed") {
            format!(
                "rendered in {family}, gpui's fallback: the font it asked for isn't loaded (allowed: {allowed})"
            )
        } else {
            format!("set in {family}, which no role allows ({allowed})")
        };
        return Some(text_violation(Rule::FontFamily, line, message));
    }
    let sized = roles
        .iter()
        .filter(|role| {
            role.sizes.is_empty() || role.sizes.iter().any(|size| near(*size, line.font_size))
        })
        .collect::<Vec<_>>();
    if sized.is_empty() {
        let allowed = roles
            .iter()
            .map(|role| format!("{} {}", role.name, list(&role.sizes)))
            .collect::<Vec<_>>()
            .join("; ");
        return Some(text_violation(
            Rule::FontSize,
            line,
            format!(
                "{} px isn't an allowed size for {family} ({allowed})",
                trim(line.font_size)
            ),
        ));
    }
    let weighted = sized.iter().any(|role| {
        role.weights.is_empty()
            || role
                .weights
                .iter()
                .any(|weight| near(*weight, line.font_weight))
    });
    (!weighted).then(|| {
        let allowed = sized
            .iter()
            .map(|role| format!("{} {}", role.name, list(&role.weights)))
            .collect::<Vec<_>>()
            .join("; ");
        text_violation(
            Rule::FontWeight,
            line,
            format!(
                "weight {} isn't allowed at {} px ({allowed})",
                trim(line.font_weight),
                trim(line.font_size)
            ),
        )
    })
}

/// The text color must be within ΔE of a palette color, with matching alpha.
pub fn text_color(spec: &StyleSpec, line: &TextLine) -> Option<Violation> {
    let (nearest, distance) = spec
        .text_colors
        .iter()
        .map(|color| {
            let alpha = (color.a - line.color.a).abs() * 0.5;
            (*color, line.color.delta_e(*color) + alpha)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    (distance > spec.color_tolerance).then(|| {
        text_violation(
            Rule::TextColor,
            line,
            format!(
                "{} isn't in the palette (nearest {}, ΔE {:.3}, tolerance {})",
                line.color,
                nearest,
                distance,
                trim(spec.color_tolerance)
            ),
        )
    })
}

/// WCAG contrast of the text color against the pixels actually behind it.
pub fn text_contrast(
    spec: &StyleSpec,
    line: &TextLine,
    input: &LintInput<'_>,
) -> Option<Violation> {
    let normal = spec.min_contrast?;
    let large = line.font_size >= 24. || (line.font_size >= 18.66 && line.font_weight >= 700.);
    let minimum = if large {
        spec.min_contrast_large.unwrap_or(normal)
    } else {
        normal
    };
    let background = background_behind(input.image, input.scale, line.visible, line.color)?;
    let ink = line.color.over(background);
    let ratio = ink.contrast(background);
    (ratio + 1e-3 < minimum).then(|| {
        text_violation(
            Rule::Contrast,
            line,
            format!(
                "contrast {:.2}:1 ({} on {}), needs {}:1",
                ratio,
                ink,
                background,
                trim(minimum)
            ),
        )
    })
}

/// The dominant color around and between a text line's glyphs: the most
/// common color in the line's box (and a 2 px ring around it) that isn't the
/// ink itself. Measured on the rendered pixels, so it sees whatever is really
/// behind the text: gradients, images, overlapping panes.
pub fn background_behind(image: &RgbaImage, scale: f32, rect: Rect, ink: Color) -> Option<Color> {
    let (x0, y0, w, h) = rect
        .inflate(2.)
        .to_device(scale, image.width(), image.height());
    if w == 0 || h == 0 {
        return None;
    }
    let mut buckets: HashMap<u16, (u32, [u32; 3])> = HashMap::new();
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            let key = (u16::from(r >> 3) << 10) | (u16::from(g >> 3) << 5) | u16::from(b >> 3);
            let bucket = buckets.entry(key).or_default();
            bucket.0 += 1;
            bucket.1[0] += u32::from(r);
            bucket.1[1] += u32::from(g);
            bucket.1[2] += u32::from(b);
        }
    }
    let mut buckets = buckets.into_values().collect::<Vec<_>>();
    buckets.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let average = |(count, sum): &(u32, [u32; 3])| {
        let channel = |sum: u32| (sum as f32 / *count as f32) / 255.;
        Color::rgb(channel(sum[0]), channel(sum[1]), channel(sum[2]))
    };
    buckets
        .iter()
        .map(average)
        .find(|color| color.delta_e(ink.over(*color)) > 0.1)
        .or_else(|| buckets.first().map(average))
}

/// Text cut off by its clip. Ellipsized text fits, so it never counts.
pub fn clipped_text(spec: &StyleSpec, line: &TextLine) -> Option<Violation> {
    if !line.is_clipped() {
        return None;
    }
    let cut_x = line.visible.w < line.bounds.w - 0.5;
    let cut_y = line.visible.h < line.bounds.h * 0.75;
    (cut_x || (cut_y && !spec.allow_vertical_clip)).then(|| {
        text_violation(
            Rule::ClippedText,
            line,
            format!(
                "cut off by its container: {}×{} of {}×{} px visible",
                trim(line.visible.w),
                trim(line.visible.h),
                trim(line.bounds.w),
                trim(line.bounds.h)
            ),
        )
    })
}

/// Whether `bounds` reaches past the window and that is where it was clipped.
fn beyond(bounds: &Rect, visible: &Rect, window: &Rect) -> bool {
    let tolerance = 0.5;
    (bounds.x < window.x - tolerance && near(visible.x, window.x))
        || (bounds.y < window.y - tolerance && near(visible.y, window.y))
        || (bounds.right() > window.right() + tolerance && near(visible.right(), window.right()))
        || (bounds.bottom() > window.bottom() + tolerance
            && near(visible.bottom(), window.bottom()))
}

/// Pairs of text lines drawn on top of each other.
pub fn overlaps(texts: &[&TextLine]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (ix, a) in texts.iter().enumerate() {
        for b in &texts[ix + 1..] {
            let overlap = a.visible.intersect(&b.visible);
            if overlap.w > 2. && overlap.h > 0.3 * a.visible.h.min(b.visible.h) {
                violations.push(Violation {
                    rule: Rule::TextOverlap,
                    severity: Rule::TextOverlap.severity(),
                    subject: format!("{} and {}", text_subject(a), text_subject(b)),
                    message: format!("overlap by {}×{} px", trim(overlap.w), trim(overlap.h)),
                    bounds: a.visible.union(&b.visible),
                });
            }
        }
    }
    violations
}

/// Side-by-side single-row lines whose baselines differ.
pub fn baselines(texts: &[&TextLine], tolerance: f32) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (ix, a) in texts.iter().enumerate() {
        for b in &texts[ix + 1..] {
            let (Some(baseline_a), Some(baseline_b)) = (a.baseline, b.baseline) else {
                continue;
            };
            let (ra, rb) = (a.visible, b.visible);
            let shared = ra.bottom().min(rb.bottom()) - ra.y.max(rb.y);
            let gap = (rb.x - ra.right()).max(ra.x - rb.right());
            let side_by_side = shared >= 0.5 * ra.h.min(rb.h) && (-1. ..=48.).contains(&gap);
            let drift = (baseline_a - baseline_b).abs();
            if side_by_side && drift > tolerance {
                violations.push(Violation {
                    rule: Rule::Baseline,
                    severity: Rule::Baseline.severity(),
                    subject: format!("{} and {}", text_subject(a), text_subject(b)),
                    message: format!(
                        "sit side by side but their baselines differ by {} px (tolerance {})",
                        trim(drift),
                        trim(tolerance)
                    ),
                    bounds: ra.union(&rb),
                });
            }
        }
    }
    violations
}

/// Something laid out: a painted quad or a line of text.
struct Placed {
    bounds: Rect,
    subject: String,
    is_quad: bool,
}

/// Boxes that aren't on the spacing grid relative to anything.
///
/// A design grid constrains *spacing* — padding, gaps, insets — not absolute
/// positions: a right-aligned, content-sized button has a floating left edge.
/// So each box passes an axis when one of its edges is a grid multiple away
/// from the matching edge of its container (the smallest painted quad around
/// it, else the window) or from its nearest neighbor in that container, or
/// when it is centered in its container.
pub fn spacing(
    rule: &SpacingRule,
    quads: &[&QuadInfo],
    texts: &[&TextLine],
    window: &Rect,
) -> Vec<Violation> {
    let mut boxes = quads
        .iter()
        .map(|quad| Placed {
            bounds: quad.bounds,
            subject: quad_subject(quad),
            is_quad: true,
        })
        .collect::<Vec<_>>();
    if rule.include_text {
        boxes.extend(texts.iter().map(|line| Placed {
            bounds: line.bounds,
            subject: text_subject(line),
            is_quad: false,
        }));
    }
    // Each box's container: the smallest strictly larger quad around it.
    let parents = boxes
        .iter()
        .map(|placed| {
            boxes
                .iter()
                .enumerate()
                .filter(|(_, other)| {
                    other.is_quad
                        && other.bounds.area() > placed.bounds.area() + 0.5
                        && other.bounds.contains_rect(&placed.bounds, 0.5)
                })
                .min_by(|a, b| a.1.bounds.area().total_cmp(&b.1.bounds.area()))
                .map(|(ix, _)| ix)
        })
        .collect::<Vec<_>>();
    let on_grid = |value: f32| {
        let steps = value / rule.grid;
        (steps - steps.round()).abs() * rule.grid < 0.01
    };
    // A text box is as wide as its glyph advances, while the layout box around
    // it may be up to a pixel wider: gaps after a text's trailing edge can
    // measure up to a pixel more than the spacing that was laid out.
    let on_grid_after_text = |value: f32| {
        let past = value.rem_euclid(rule.grid);
        past < 1. + 0.01 || rule.grid - past < 0.01
    };
    let exempt = |size: f32| rule.exempt_sizes.iter().any(|exempt| near(*exempt, size));

    let mut violations = Vec::new();
    for (ix, placed) in boxes.iter().enumerate() {
        let parent = parents[ix].map_or(*window, |parent| boxes[parent].bounds);
        let siblings = boxes
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != ix && parents[*other] == parents[ix])
            .map(|(_, sibling)| (sibling.bounds, !sibling.is_quad))
            .collect::<Vec<_>>();
        let is_text = !placed.is_quad;
        let mut problems = Vec::new();
        for horizontal in [true, false] {
            let span = |rect: &Rect| {
                if horizontal {
                    (rect.x, rect.right())
                } else {
                    (rect.y, rect.bottom())
                }
            };
            let cross = |rect: &Rect| {
                if horizontal {
                    (rect.y, rect.bottom())
                } else {
                    (rect.x, rect.right())
                }
            };
            let (start, end) = span(&placed.bounds);
            if exempt(end - start) {
                continue;
            }
            let (parent_start, parent_end) = span(&parent);
            let (inset_start, inset_end) = (start - parent_start, parent_end - end);
            let (cross_start, cross_end) = cross(&placed.bounds);
            let beside = siblings.iter().filter(|(sibling, _)| {
                let (a, b) = cross(sibling);
                b.min(cross_end) - a.max(cross_start) > 0.
            });
            // The nearest neighbor on each side, and whether the gap to it
            // starts at a text's trailing edge.
            let mut before = None::<(f32, bool)>;
            let mut after = None::<(f32, bool)>;
            for (sibling, sibling_is_text) in beside {
                let (a, b) = span(sibling);
                if b <= start + 0.5 && before.is_none_or(|(gap, _)| start - b < gap) {
                    before = Some((start - b, horizontal && *sibling_is_text));
                }
                if a >= end - 0.5 && after.is_none_or(|(gap, _)| a - end < gap) {
                    after = Some((a - end, horizontal && is_text));
                }
            }
            let gap_on_grid = |(gap, after_text): (f32, bool)| {
                if after_text {
                    on_grid_after_text(gap)
                } else {
                    on_grid(gap)
                }
            };
            let end_on_grid = if horizontal && is_text {
                on_grid_after_text(inset_end)
            } else {
                on_grid(inset_end)
            };
            let anchored = on_grid(inset_start)
                || end_on_grid
                || (inset_start - inset_end).abs() < 0.5
                || before.is_some_and(gap_on_grid)
                || after.is_some_and(gap_on_grid);
            if !anchored {
                let (from, to) = if horizontal {
                    ("left", "right")
                } else {
                    ("top", "bottom")
                };
                let mut description = format!(
                    "{} px from its container's {from} edge, {} px from its {to} edge",
                    trim(inset_start),
                    trim(inset_end)
                );
                if let Some((gap, _)) = before.or(after) {
                    description.push_str(&format!(", {} px from its neighbor", trim(gap)));
                }
                problems.push(description);
            }
        }
        if !problems.is_empty() {
            violations.push(Violation {
                rule: Rule::Spacing,
                severity: Rule::Spacing.severity(),
                subject: placed.subject.clone(),
                message: format!(
                    "off the {} px grid: {}",
                    trim(rule.grid),
                    problems.join("; ")
                ),
                bounds: placed.bounds,
            });
        }
    }
    violations
}

/// Corner radii outside the allowed set (pills allowed if `pills`).
pub fn corner_radius(allowed: &[f32], pills: bool, quad: &QuadInfo) -> Option<Violation> {
    let half = quad.bounds.w.min(quad.bounds.h) / 2.;
    let bad = quad
        .corner_radii
        .iter()
        .copied()
        .filter(|radius| {
            !allowed.iter().any(|allowed| near(*allowed, *radius))
                && !(pills && *radius >= half - 0.5)
        })
        .fold(None::<f32>, |worst, radius| {
            Some(worst.map_or(radius, |worst| worst.max(radius)))
        })?;
    Some(Violation {
        rule: Rule::CornerRadius,
        severity: Rule::CornerRadius.severity(),
        subject: quad_subject(quad),
        message: format!(
            "corner radius {} isn't allowed ({})",
            trim(bad),
            list(allowed)
        ),
        bounds: quad.visible,
    })
}

/// Border widths outside the allowed set.
pub fn border_width(allowed: &[f32], quad: &QuadInfo) -> Option<Violation> {
    if !quad.has_border() {
        return None;
    }
    let bad = quad
        .border_widths
        .iter()
        .copied()
        .find(|width| *width > 0. && !allowed.iter().any(|allowed| near(*allowed, *width)))?;
    Some(Violation {
        rule: Rule::BorderWidth,
        severity: Rule::BorderWidth.severity(),
        subject: quad_subject(quad),
        message: format!(
            "border width {} isn't allowed ({})",
            trim(bad),
            list(allowed)
        ),
        bounds: quad.visible,
    })
}

/// Clickable areas smaller than `min` on either side.
pub fn small_targets(min: f32, clickables: &[Rect]) -> Vec<Violation> {
    clickables
        .iter()
        .filter(|rect| !rect.is_empty() && (rect.w < min - 0.01 || rect.h < min - 0.01))
        .map(|rect| Violation {
            rule: Rule::HitTarget,
            severity: Rule::HitTarget.severity(),
            subject: format!("clickable {}×{}", trim(rect.w), trim(rect.h)),
            message: format!(
                "smaller than the {}×{} px minimum hit target",
                trim(min),
                trim(min)
            ),
            bounds: *rect,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::TextRole;

    fn line(text: &str, bounds: Rect) -> TextLine {
        TextLine {
            text: text.into(),
            bounds,
            visible: bounds,
            font_family: "IBM Plex Sans".into(),
            font_size: 12.,
            font_weight: 400.,
            color: Color::from_u32(0x1c1f24),
            baseline: Some(bounds.y + 12.),
        }
    }

    fn quad(bounds: Rect) -> QuadInfo {
        QuadInfo {
            bounds,
            visible: bounds,
            background: Some(Color::WHITE),
            patterned: false,
            border_color: None,
            border_widths: [0.; 4],
            corner_radii: [0.; 4],
        }
    }

    #[test]
    fn fonts_match_roles_by_family_size_and_weight() {
        let spec = StyleSpec {
            text_roles: vec![
                TextRole::new("ui", &["IBM Plex Sans"])
                    .sizes(&[12., 11.])
                    .weights(&[400.]),
                TextRole::new("mono", &["Lilex"]),
            ],
            ..StyleSpec::default()
        };
        let mut text = line("Save", Rect::new(0., 0., 30., 16.));
        assert_eq!(font(&spec, &text, "IBM Plex Sans"), None);
        text.font_family = ".SystemUIFont".into();
        assert_eq!(
            font(&spec, &text, "IBM Plex Sans"),
            None,
            "system font resolves"
        );
        text.font_size = 13.;
        let violation = font(&spec, &text, "IBM Plex Sans").unwrap();
        assert_eq!(violation.rule, Rule::FontSize);
        assert!(
            violation.message.contains("13 px isn't an allowed size"),
            "{}",
            violation.message
        );
        text.font_size = 12.;
        text.font_weight = 600.;
        assert_eq!(
            font(&spec, &text, "IBM Plex Sans").unwrap().rule,
            Rule::FontWeight
        );
        text.font_family = "Comic Neue".into();
        let violation = font(&spec, &text, "IBM Plex Sans").unwrap();
        assert_eq!(violation.rule, Rule::FontFamily);
        assert!(violation.message.contains("ui: IBM Plex Sans; mono: Lilex"));
    }

    #[test]
    fn spacing_is_measured_against_containers_and_neighbors() {
        let rule = SpacingRule {
            grid: 4.,
            exempt_sizes: vec![22.],
            include_text: true,
        };
        let window = Rect::new(0., 0., 801., 601.);
        let card = quad(Rect::new(16., 16., 360., 200.));
        // Right-aligned, content-sized buttons: Save sits 16 px from the
        // card's right edge, Cancel 8 px from Save. Floating left edges are fine.
        let save = quad(Rect::new(319.3, 176., 40.7, 24.));
        let cancel = quad(Rect::new(255.1, 176., 56.2, 24.));
        let title = line("Title", Rect::new(32., 32., 57.3, 16.));
        let clean = spacing(&rule, &[&card, &save, &cancel], &[&title], &window);
        assert_eq!(clean, vec![], "the window's odd size doesn't matter either");

        // 13 px of padding puts the title off the grid on both axes.
        let title = line("Title", Rect::new(29., 29., 57.3, 16.));
        let violations = spacing(&rule, &[&card], &[&title], &window);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].subject, "“Title”");
        assert!(
            violations[0]
                .message
                .contains("13 px from its container's left edge"),
            "{}",
            violations[0].message
        );

        // Centered boxes and 22 px rows are exempt.
        let badge = quad(Rect::new(16. + 100.3, 18., 19.4, 20.));
        let centered = quad(Rect::new(16. + (360. - 19.4) / 2., 106., 19.4, 20.));
        let row = quad(Rect::new(16., 66., 360., 22.));
        assert_eq!(
            spacing(&rule, &[&card, &centered, &row], &[], &window),
            vec![]
        );
        assert_eq!(spacing(&rule, &[&card, &badge], &[], &window).len(), 1);
    }

    #[test]
    fn radii_allow_pills() {
        let mut pill = quad(Rect::new(0., 0., 60., 20.));
        pill.corner_radii = [10.; 4];
        assert_eq!(corner_radius(&[0., 4.], true, &pill), None);
        assert!(corner_radius(&[0., 4.], false, &pill).is_some());
        pill.corner_radii = [6.; 4];
        assert!(
            corner_radius(&[0., 4.], true, &pill)
                .unwrap()
                .message
                .contains("radius 6")
        );
    }

    #[test]
    fn overlap_and_baseline_rules() {
        let a = line("Name", Rect::new(0., 0., 40., 16.));
        let b = line("Alice", Rect::new(30., 4., 40., 16.));
        assert_eq!(overlaps(&[&a, &b]).len(), 1);
        let c = line("Alice", Rect::new(48., 0., 40., 16.));
        assert!(overlaps(&[&a, &c]).is_empty());
        let mut d = c.clone();
        d.baseline = Some(15.);
        assert!(baselines(&[&a, &c], 1.).is_empty());
        let drift = baselines(&[&a, &d], 1.);
        assert!(
            drift[0].message.contains("differ by 3 px"),
            "{}",
            drift[0].message
        );
    }

    #[test]
    fn background_is_measured_between_the_glyphs() {
        // A light-gray box with a few "glyph" pixels of the ink color.
        let mut image = RgbaImage::from_pixel(40, 20, image::Rgba([200, 200, 200, 255]));
        for x in 10..20 {
            image.put_pixel(x, 8, image::Rgba([90, 90, 90, 255]));
        }
        let ink = Color::from_u32(0x5a5a5a);
        let background = background_behind(&image, 1., Rect::new(4., 4., 30., 12.), ink).unwrap();
        assert_eq!(background.hex(), "#c8c8c8");
    }
}
