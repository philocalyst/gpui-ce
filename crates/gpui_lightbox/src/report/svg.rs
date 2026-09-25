//! Inline SVG charts for the report. Colors are CSS variables (`--series-1`,
//! `--ok`, …) so the charts follow the page's light or dark theme; every mark
//! carries a `<title>` tooltip, with a hit target larger than the mark.

use super::html::esc;
use std::fmt::Write as _;

/// `1`, `2` or `5` times a power of ten, stepping `min..=max` in about `count` ticks.
pub fn nice_ticks(min: f64, max: f64, count: usize) -> Vec<f64> {
    let (min, max) = if (max - min).abs() < f64::EPSILON {
        (min - 1., max + 1.)
    } else {
        (min, max)
    };
    let raw = (max - min) / count.max(1) as f64;
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1., 2., 5., 10.]
        .into_iter()
        .map(|factor| factor * magnitude)
        .find(|step| *step >= raw)
        .unwrap_or(10. * magnitude);
    // Round through the step's decimal places, so 3 × 0.1 is 0.3.
    let decimals = (-step.log10().floor()).max(0.) as usize;
    let clean = |value: f64| {
        format!("{value:.decimals$}")
            .parse::<f64>()
            .unwrap_or(value)
    };
    let first = (min / step).floor() as i64;
    let last = (max / step).ceil() as i64;
    (first..=last).map(|k| clean(k as f64 * step)).collect()
}

/// A number with as few decimals as its size needs: `12.3`, `0.482`, `1,204`.
pub fn number(value: f64) -> String {
    let abs = value.abs();
    if value.fract() == 0. && abs < 1000. {
        format!("{value:.0}")
    } else if abs >= 1000. {
        let rounded = value.round() as i64;
        let digits = rounded.abs().to_string();
        let mut grouped = String::new();
        for (ix, digit) in digits.chars().enumerate() {
            if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(digit);
        }
        if rounded < 0 {
            format!("-{grouped}")
        } else {
            grouped
        }
    } else if abs >= 100. {
        format!("{value:.0}")
    } else if abs >= 10. {
        format!("{value:.1}")
    } else if abs >= 1. {
        format!("{value:.2}")
    } else {
        format!("{value:.3}")
    }
}

/// A tiny line of one series with its last point marked. `values` oldest first.
pub fn sparkline(values: &[f64], titles: &[String], width: f64, height: f64) -> String {
    let mut svg = open(width, height, "spark");
    if values.len() < 2 {
        if let (Some(value), Some(title)) = (values.first(), titles.first()) {
            let _ = write!(
                svg,
                r#"<circle class="mark end" cx="{:.1}" cy="{:.1}" r="3"><title>{}</title></circle>"#,
                width - 4.,
                height / 2.,
                esc(&format!("{title}: {} ms", number(*value)))
            );
        }
        svg.push_str("</svg>");
        return svg;
    }
    let (min, max) = extent(values);
    let x = |ix: usize| 4. + ix as f64 / (values.len() - 1) as f64 * (width - 8.);
    let y = |value: f64| height - 4. - (value - min) / (max - min).max(1e-9) * (height - 8.);
    let points = values
        .iter()
        .enumerate()
        .map(|(ix, value)| format!("{:.1},{:.1}", x(ix), y(*value)))
        .collect::<Vec<_>>()
        .join(" ");
    let _ = write!(svg, r#"<polyline class="line" points="{points}"/>"#);
    for (ix, value) in values.iter().enumerate() {
        let title = titles.get(ix).map(String::as_str).unwrap_or_default();
        let last = ix + 1 == values.len();
        let _ = write!(
            svg,
            r#"<g><circle class="hit" cx="{cx:.1}" cy="{cy:.1}" r="6"/>{mark}<title>{title}</title></g>"#,
            cx = x(ix),
            cy = y(*value),
            mark = if last {
                format!(
                    r#"<circle class="mark end" cx="{:.1}" cy="{:.1}" r="3"/>"#,
                    x(ix),
                    y(*value)
                )
            } else {
                String::new()
            },
            title = esc(&format!("{title}: {} ms", number(*value)))
        );
    }
    svg.push_str("</svg>");
    svg
}

/// One series of a line chart.
pub struct Series<'a> {
    /// Legend label.
    pub label: &'a str,
    /// CSS class choosing the color (`s1`, `s2`…).
    pub class: &'a str,
    /// One value per x position (`None` for gaps).
    pub values: Vec<Option<f64>>,
}

/// A line chart with a y axis, x labels and a legend (for two or more series).
pub fn lines(
    series: &[Series<'_>],
    x_labels: &[String],
    unit: &str,
    width: f64,
    height: f64,
) -> String {
    let (left, right, top, bottom) = (44., 12., 12., 24.);
    let plot_w = width - left - right;
    let plot_h = height - top - bottom;
    let all = series
        .iter()
        .flat_map(|series| series.values.iter().flatten().copied())
        .collect::<Vec<_>>();
    let (min, max) = extent(&all);
    let ticks = nice_ticks(min.min(0.), max, 4);
    let (lo, hi) = (ticks[0], *ticks.last().unwrap_or(&max));
    let count = x_labels.len().max(2);
    let x = |ix: usize| left + ix as f64 / (count - 1) as f64 * plot_w;
    let y = |value: f64| top + plot_h - (value - lo) / (hi - lo).max(1e-9) * plot_h;

    let mut svg = open(width, height, "chart");
    for tick in &ticks {
        let _ = write!(
            svg,
            r#"<line class="grid" x1="{left}" x2="{:.1}" y1="{y:.1}" y2="{y:.1}"/><text class="tick" x="{:.1}" y="{:.1}" text-anchor="end">{}</text>"#,
            width - right,
            left - 6.,
            y(*tick) + 3.5,
            esc(&format!("{}{unit}", number(*tick))),
            y = y(*tick),
        );
    }
    let label_every = (x_labels.len() / 6).max(1);
    for (ix, label) in x_labels.iter().enumerate() {
        if ix % label_every == 0 || ix + 1 == x_labels.len() {
            let _ = write!(
                svg,
                r#"<text class="tick" x="{:.1}" y="{:.1}" text-anchor="middle">{}</text>"#,
                x(ix),
                height - 6.,
                esc(label)
            );
        }
    }
    for series in series {
        let mut path = String::new();
        let mut pen_down = false;
        for (ix, value) in series.values.iter().enumerate() {
            match value {
                Some(value) => {
                    let _ = write!(
                        path,
                        "{}{:.1},{:.1} ",
                        if pen_down { "L" } else { "M" },
                        x(ix),
                        y(*value)
                    );
                    pen_down = true;
                }
                None => pen_down = false,
            }
        }
        let _ = write!(svg, r#"<path class="line {}" d="{path}"/>"#, series.class);
        for (ix, value) in series.values.iter().enumerate() {
            let Some(value) = value else { continue };
            let label = x_labels.get(ix).map(String::as_str).unwrap_or_default();
            let _ = write!(
                svg,
                r#"<g><circle class="hit" cx="{cx:.1}" cy="{cy:.1}" r="8"/><circle class="mark {class}" cx="{cx:.1}" cy="{cy:.1}" r="4"/><title>{title}</title></g>"#,
                cx = x(ix),
                cy = y(*value),
                class = series.class,
                title = esc(&format!(
                    "{} · {label}: {}{unit}",
                    series.label,
                    number(*value)
                ))
            );
        }
    }
    svg.push_str("</svg>");
    if series.len() > 1 {
        let mut legend = String::from(r#"<div class="legend">"#);
        for series in series {
            let _ = write!(
                legend,
                r#"<span><i class="key {}"></i>{}</span>"#,
                series.class,
                esc(series.label)
            );
        }
        legend.push_str("</div>");
        legend.push_str(&svg);
        return legend;
    }
    svg
}

/// A histogram of frame times with the median and 95th percentile marked.
pub fn histogram(samples: &[f64], p50: f64, p95: f64, width: f64, height: f64) -> String {
    let (left, right, top, bottom) = (36., 12., 16., 24.);
    let plot_w = width - left - right;
    let plot_h = height - top - bottom;
    let (min, max) = extent(samples);
    let bins = 24usize;
    let span = (max - min).max(1e-6);
    let mut counts = vec![0usize; bins];
    for sample in samples {
        let bin = (((sample - min) / span) * bins as f64).floor() as usize;
        counts[bin.min(bins - 1)] += 1;
    }
    let peak = counts.iter().copied().max().unwrap_or(1).max(1) as f64;
    let slot = plot_w / bins as f64;
    let bar = (slot - 2.).clamp(1., 24.);
    let x_of = |value: f64| left + (value - min) / span * plot_w;

    let mut svg = open(width, height, "chart");
    let _ = write!(
        svg,
        r#"<line class="axis" x1="{left}" x2="{:.1}" y1="{:.1}" y2="{:.1}"/>"#,
        width - right,
        top + plot_h,
        top + plot_h
    );
    for (ix, count) in counts.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let h = (*count as f64 / peak * plot_h).max(2.);
        let x0 = left + ix as f64 * slot + (slot - bar) / 2.;
        let y0 = top + plot_h - h;
        let r = 4f64.min(bar / 2.).min(h);
        let from = min + ix as f64 * span / bins as f64;
        let to = from + span / bins as f64;
        let _ = write!(
            svg,
            r#"<g><rect class="hit" x="{:.1}" y="{top}" width="{slot:.1}" height="{plot_h:.1}"/><path class="bar" d="M{x0:.1},{base:.1} V{r0:.1} Q{x0:.1},{y0:.1} {x1:.1},{y0:.1} H{x2:.1} Q{x3:.1},{y0:.1} {x3:.1},{r0:.1} V{base:.1} Z"/><title>{title}</title></g>"#,
            left + ix as f64 * slot,
            base = top + plot_h,
            r0 = y0 + r,
            x1 = x0 + r,
            x2 = x0 + bar - r,
            x3 = x0 + bar,
            title = esc(&format!(
                "{count} frame{} · {}–{} ms",
                if *count == 1 { "" } else { "s" },
                number(from),
                number(to)
            ))
        );
    }
    // Labels sit left and right of their markers, so close markers don't collide.
    for (label, value, anchor, nudge) in [("p50", p50, "end", -4.), ("p95", p95, "start", 4.)] {
        let x = x_of(value);
        let _ = write!(
            svg,
            r#"<line class="marker" x1="{x:.1}" x2="{x:.1}" y1="{:.1}" y2="{:.1}"/><text class="tick strong" x="{:.1}" y="{:.1}" text-anchor="{anchor}">{label} {}</text>"#,
            top - 6.,
            top + plot_h,
            x + nudge,
            top - 5.,
            number(value)
        );
    }
    for tick in nice_ticks(min, max, 5) {
        if tick < min - span * 0.01 || tick > max + span * 0.01 {
            continue;
        }
        let _ = write!(
            svg,
            r#"<text class="tick" x="{:.1}" y="{:.1}" text-anchor="middle">{} ms</text>"#,
            x_of(tick),
            height - 6.,
            number(tick)
        );
    }
    svg.push_str("</svg>");
    svg
}

/// One segment of a stacked bar.
pub struct Segment<'a> {
    /// Legend label.
    pub label: &'a str,
    /// CSS class choosing the color.
    pub class: &'a str,
    /// The value (ms).
    pub value: f64,
}

/// A horizontal stacked bar with 2 px gaps between segments and rounded ends.
pub fn stacked_bar(segments: &[Segment<'_>], width: f64, height: f64) -> String {
    let total = segments.iter().map(|segment| segment.value).sum::<f64>();
    let mut svg = open(width, height, "stack");
    if total <= 0. {
        svg.push_str("</svg>");
        return svg;
    }
    let visible = segments
        .iter()
        .filter(|segment| segment.value > 0.)
        .collect::<Vec<_>>();
    let gaps = 2. * visible.len().saturating_sub(1) as f64;
    let mut x = 0.;
    for (ix, segment) in visible.iter().enumerate() {
        let w = (segment.value / total * (width - gaps)).max(1.);
        let r = if ix == 0 || ix + 1 == visible.len() {
            (height / 2.).min(4.)
        } else {
            0.
        };
        let _ = write!(
            svg,
            r#"<rect class="seg {}" x="{x:.1}" y="0" width="{w:.1}" height="{height}" rx="{r}"><title>{}</title></rect>"#,
            segment.class,
            esc(&format!(
                "{} {} ms ({:.0}%)",
                segment.label,
                number(segment.value),
                segment.value / total * 100.
            ))
        );
        x += w + 2.;
    }
    svg.push_str("</svg>");
    svg
}

/// A film's tracked property over time, with finding markers; `cursor` lets
/// the viewer scrub. `marks` are `(frame, css class, text)`.
pub fn curve(
    values: &[Option<f64>],
    times: &[f64],
    class: &str,
    marks: &[(usize, &str, String)],
    width: f64,
    height: f64,
) -> String {
    let (left, right, top, bottom) = (48., 12., 10., 20.);
    let plot_w = width - left - right;
    let plot_h = height - top - bottom;
    let measured = values.iter().flatten().copied().collect::<Vec<_>>();
    let (min, max) = extent(&measured);
    let end = times.last().copied().unwrap_or(1.).max(1e-6);
    let x = |t: f64| left + t / end * plot_w;
    let y = |value: f64| top + plot_h - (value - min) / (max - min).max(1e-9) * plot_h;

    let mut svg = open(width, height, "chart curve");
    for value in [min, max] {
        let _ = write!(
            svg,
            r#"<line class="grid" x1="{left}" x2="{:.1}" y1="{y:.1}" y2="{y:.1}"/><text class="tick" x="{:.1}" y="{:.1}" text-anchor="end">{}</text>"#,
            width - right,
            left - 6.,
            y(value) + 3.5,
            number(value),
            y = y(value)
        );
    }
    for tick in nice_ticks(0., end, 4) {
        if tick > end * 1.001 {
            continue;
        }
        let _ = write!(
            svg,
            r#"<text class="tick" x="{:.1}" y="{:.1}" text-anchor="middle">{} ms</text>"#,
            x(tick),
            height - 4.,
            number(tick)
        );
    }
    let mut path = String::new();
    let mut pen_down = false;
    for (value, t) in values.iter().zip(times) {
        match value {
            Some(value) => {
                let _ = write!(
                    path,
                    "{}{:.1},{:.1} ",
                    if pen_down { "L" } else { "M" },
                    x(*t),
                    y(*value)
                );
                pen_down = true;
            }
            None => pen_down = false,
        }
    }
    let _ = write!(svg, r#"<path class="line {class}" d="{path}"/>"#);
    for (ix, (value, t)) in values.iter().zip(times).enumerate() {
        let Some(value) = value else { continue };
        let mark = marks.iter().find(|(frame, _, _)| *frame == ix);
        let (mark_class, note) = mark.map_or((class, String::new()), |(_, class, text)| {
            (*class, format!(" · {text}"))
        });
        let _ = write!(
            svg,
            r#"<g data-frame="{ix}"><circle class="hit" cx="{cx:.1}" cy="{cy:.1}" r="8"/><circle class="mark {mark_class}{big}" cx="{cx:.1}" cy="{cy:.1}" r="{r}"/><title>{title}</title></g>"#,
            cx = x(*t),
            cy = y(*value),
            big = if mark.is_some() { " flag" } else { "" },
            r = if mark.is_some() { 5 } else { 3 },
            title = esc(&format!(
                "#{ix} · {} ms: {}{note}",
                number(*t),
                number(*value)
            ))
        );
    }
    let _ = write!(
        svg,
        r#"<line class="cursor" x1="{left}" x2="{left}" y1="{top}" y2="{:.1}" data-left="{left}" data-width="{plot_w:.1}" data-end="{end}"/>"#,
        top + plot_h
    );
    svg.push_str("</svg>");
    svg
}

fn open(width: f64, height: f64, class: &str) -> String {
    format!(
        r#"<svg class="{class}" viewBox="0 0 {width} {height}" width="{width}" height="{height}" role="img">"#
    )
}

fn extent(values: &[f64]) -> (f64, f64) {
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if min.is_finite() && max.is_finite() {
        (min, max)
    } else {
        (0., 1.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_round_numbers() {
        assert_eq!(nice_ticks(0., 9.3, 5), vec![0., 2., 4., 6., 8., 10.]);
        assert_eq!(nice_ticks(0., 9.3, 4), vec![0., 5., 10.]);
        assert_eq!(nice_ticks(0.12, 0.48, 4), vec![0.1, 0.2, 0.3, 0.4, 0.5]);
        assert_eq!(number(1234.4), "1,234");
        assert_eq!(number(0.4821), "0.482");
    }

    #[test]
    fn charts_are_self_contained_svg() {
        let svg = histogram(&[1., 2., 2., 3., 9.], 2., 9., 320., 120.);
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("p95 9<"), "{svg}");
        let chart = lines(
            &[
                Series {
                    label: "p50",
                    class: "s1",
                    values: vec![Some(1.), Some(2.)],
                },
                Series {
                    label: "p95",
                    class: "s2",
                    values: vec![Some(2.), None],
                },
            ],
            &["a".into(), "b".into()],
            " ms",
            320.,
            120.,
        );
        assert!(
            chart.starts_with(r#"<div class="legend">"#),
            "two series get a legend"
        );
    }
}
