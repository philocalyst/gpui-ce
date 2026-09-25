//! Golden images: compare a shot with a recorded PNG using a perceptual
//! tolerance, and explain any difference with a heatmap.
//!
//! Goldens live in `tests/golden/<name>.png` of the crate whose tests run
//! (`$LIGHTBOX_GOLDEN_DIR` overrides). Run with `LIGHTBOX_UPDATE=1` to
//! record or re-record them.
//!
//! **Goldens are renderer-specific.** They are recorded with the headless
//! wgpu renderer; Mesa lavapipe (Linux, CI) and Metal or a discrete GPU
//! rasterize anti-aliased edges and glyphs slightly differently. The default
//! tolerance ignores per-pixel differences up to ΔE 0.04 (a faint change in
//! edge coverage) and allows 0.01 % of pixels to differ by more, which absorbs
//! driver noise but still fails on a 1 px shift of anything. Record goldens on
//! the platform that checks them (CI's lavapipe), or loosen the tolerance with
//! [`Shot::assert_golden_with`] for cross-renderer comparisons.

use crate::{
    color::{Color, LinearTable},
    compose::{Composer, backdrop, heading, mono, picture, pill, placed, resample, section},
    manifest::{GoldenRecord, Origin, Record},
    output::{slug, workspace_root},
    shot::{Rect, Shot, trim},
    theme::DARK,
};
use anyhow::{Context as _, Result};
use gpui::{IntoElement, ParentElement as _, Styled as _, div, px};
use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    panic::Location,
    path::{Path, PathBuf},
    rc::Rc,
};

/// How different a shot may be from its golden and still match.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoldenTolerance {
    /// Pixels closer than this (Oklab ΔE) count as equal.
    pub max_delta_e: f32,
    /// The fraction of pixels (0..1) allowed to differ by more.
    pub max_fraction: f64,
}

impl Default for GoldenTolerance {
    fn default() -> Self {
        Self {
            max_delta_e: 0.04,
            max_fraction: 0.0001,
        }
    }
}

impl GoldenTolerance {
    /// Only identical pixels match.
    pub fn exact() -> Self {
        Self {
            max_delta_e: 0.,
            max_fraction: 0.,
        }
    }
}

/// How two images differ.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffStats {
    /// Pixels differing by more than the tolerance's ΔE.
    pub differing: u64,
    /// Pixels differing at all (including sub-threshold noise).
    pub changed: u64,
    /// Pixels compared.
    pub total: u64,
    /// `differing / total`.
    pub fraction: f64,
    /// The largest per-pixel ΔE.
    pub max_delta_e: f32,
    /// Bounding box of the differing pixels, in logical pixels.
    pub bounds: Option<Rect>,
    /// Whether the sizes differed (then nothing else was compared).
    pub size_mismatch: bool,
}

/// Compares two images pixel by pixel in Oklab. Returns the statistics and a
/// heatmap: the actual image dimmed to gray, with sub-threshold noise in
/// blue and real differences from amber (just over) to red (ΔE ≥ 0.3).
pub fn compare(
    expected: &RgbaImage,
    actual: &RgbaImage,
    tolerance: GoldenTolerance,
    scale: f32,
) -> (DiffStats, RgbaImage) {
    let total = u64::from(actual.width()) * u64::from(actual.height());
    if expected.dimensions() != actual.dimensions() {
        let stats = DiffStats {
            differing: total,
            changed: total,
            total,
            fraction: 1.,
            max_delta_e: 1.,
            bounds: Some(Rect::new(
                0.,
                0.,
                actual.width() as f32 / scale,
                actual.height() as f32 / scale,
            )),
            size_mismatch: true,
        };
        return (stats, actual.clone());
    }
    let table = LinearTable::new();
    let mut heatmap = RgbaImage::new(actual.width(), actual.height());
    let mut stats = DiffStats {
        total,
        ..DiffStats::default()
    };
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for ((x, y, actual), expected) in actual.enumerate_pixels().zip(expected.pixels()) {
        let [r, g, b, _] = actual.0;
        let gray = ((u32::from(r) * 2 + u32::from(g) * 5 + u32::from(b)) / 8 / 4 + 24) as u8;
        let mut out = Rgba([gray, gray, gray, 255]);
        if actual != expected {
            stats.changed += 1;
            let delta = table.oklab(actual.0).distance(table.oklab(expected.0));
            stats.max_delta_e = stats.max_delta_e.max(delta);
            if delta > tolerance.max_delta_e {
                stats.differing += 1;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                let heat = ((delta - tolerance.max_delta_e) / 0.3).clamp(0., 1.);
                out = mix(DARK.warn, DARK.crit, heat);
            } else {
                out = mix(Color::from_u32(0x2b3d63), DARK.accent, 0.3);
            }
        }
        heatmap.put_pixel(x, y, out);
    }
    stats.fraction = stats.differing as f64 / total.max(1) as f64;
    if stats.differing > 0 {
        stats.bounds = Some(Rect::new(
            x0 as f32 / scale,
            y0 as f32 / scale,
            (x1 - x0 + 1) as f32 / scale,
            (y1 - y0 + 1) as f32 / scale,
        ));
    }
    (stats, heatmap)
}

fn mix(a: Color, b: Color, t: f32) -> Rgba<u8> {
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    Rgba(Color::rgb(lerp(a.r, b.r), lerp(a.g, b.g), lerp(a.b, b.b)).to_rgba8())
}

/// The outcome of a golden comparison.
#[derive(Clone, Debug)]
pub struct GoldenOutcome {
    /// Whether the shot matched (or the golden was just recorded).
    pub passed: bool,
    /// Whether the golden was (re)recorded.
    pub updated: bool,
    /// Pixel statistics.
    pub stats: DiffStats,
    /// A plain-language verdict.
    pub message: String,
    /// The golden file.
    pub golden: PathBuf,
    /// Expected, actual and heatmap side by side, when they differ.
    pub comparison: Option<PathBuf>,
}

/// Where goldens are read from and recorded to.
pub fn golden_dir() -> PathBuf {
    if let Some(dir) = env::var_os("LIGHTBOX_GOLDEN_DIR") {
        return PathBuf::from(dir);
    }
    env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(workspace_root)
        .join("tests")
        .join("golden")
}

fn update_requested() -> bool {
    env::var("LIGHTBOX_UPDATE").is_ok_and(|value| !value.is_empty() && value != "0")
}

impl Shot {
    /// Asserts the shot matches `tests/golden/<name>.png` within the default
    /// [`GoldenTolerance`]. With `LIGHTBOX_UPDATE=1` it records the golden
    /// instead. On mismatch, writes a heatmap and a side-by-side comparison.
    #[track_caller]
    pub fn assert_golden(&self, name: &str) -> &Self {
        self.assert_golden_with(name, GoldenTolerance::default())
    }

    /// [`assert_golden`](Self::assert_golden) with an explicit tolerance.
    #[track_caller]
    pub fn assert_golden_with(&self, name: &str, tolerance: GoldenTolerance) -> &Self {
        let outcome = self.golden(name, tolerance);
        assert!(
            outcome.passed,
            "lightbox: {name}: {}\n  golden: {}{}",
            outcome.message,
            outcome.golden.display(),
            outcome
                .comparison
                .map(|path| format!("\n  comparison: {}", path.display()))
                .unwrap_or_default()
        );
        self
    }

    /// Compares the shot with `tests/golden/<name>.png` (recording it with
    /// `LIGHTBOX_UPDATE=1`) and reports the outcome without panicking.
    #[track_caller]
    pub fn golden(&self, name: &str, tolerance: GoldenTolerance) -> GoldenOutcome {
        self.golden_at(&golden_dir(), name, tolerance, update_requested())
    }

    /// Compares the shot with `<dir>/<name>.png`, recording it instead when
    /// `update` is set. The report lists the comparison under the shot's name.
    #[track_caller]
    pub fn golden_at(
        &self,
        dir: &Path,
        name: &str,
        tolerance: GoldenTolerance,
        update: bool,
    ) -> GoldenOutcome {
        let location = Location::caller();
        self.compare_golden(dir, name, tolerance, update, location)
            .unwrap_or_else(|error| panic!("lightbox: golden {name:?}: {error:#}"))
    }

    fn compare_golden(
        &self,
        dir: &Path,
        name: &str,
        tolerance: GoldenTolerance,
        update: bool,
        location: &'static Location<'static>,
    ) -> Result<GoldenOutcome> {
        let golden = dir.join(format!("{}.png", slug(name)));
        let stem = slug(&self.name);
        let actual_file = format!("{stem}.golden-actual.png");
        self.suite.write_png(&actual_file, &self.image)?;

        let expected = if golden.exists() {
            Some(
                image::open(&golden)
                    .with_context(|| format!("reading {}", golden.display()))?
                    .into_rgba8(),
            )
        } else {
            None
        };
        let mut record = GoldenRecord {
            name: self.name.clone(),
            golden: golden
                .strip_prefix(workspace_root())
                .unwrap_or(&golden)
                .display()
                .to_string(),
            expected: None,
            actual: actual_file,
            diff: None,
            comparison: None,
            passed: false,
            updated: false,
            stats: DiffStats::default(),
            tolerance,
            message: String::new(),
            origin: Origin::at(location),
        };
        let mut comparison = None;

        match (&expected, update) {
            (_, true) => {
                fs::create_dir_all(dir)?;
                self.image
                    .save_with_format(&golden, image::ImageFormat::Png)
                    .with_context(|| format!("recording {}", golden.display()))?;
                record.passed = true;
                record.updated = true;
                record.message = format!("recorded {}", record.golden);
            }
            (None, false) => {
                record.message = format!(
                    "no golden at {}; run with LIGHTBOX_UPDATE=1 to record it",
                    record.golden
                );
            }
            (Some(expected), false) => {
                let (stats, heatmap) = compare(expected, &self.image, tolerance, self.scale());
                let expected_file = format!("{stem}.golden-expected.png");
                self.suite.write_png(&expected_file, expected)?;
                record.expected = Some(expected_file);
                if stats.changed > 0 {
                    let diff_file = format!("{stem}.golden-diff.png");
                    self.suite.write_png(&diff_file, &heatmap)?;
                    record.diff = Some(diff_file);
                }
                record.passed = !stats.size_mismatch && stats.fraction <= tolerance.max_fraction;
                record.message = verdict(&stats, tolerance, expected, &self.image);
                if !record.passed {
                    let file = format!("{stem}.golden-compare.png");
                    let image = self.compositor.with(|composer| {
                        side_by_side(
                            composer,
                            &self.name,
                            expected,
                            &self.image,
                            &heatmap,
                            &record.message,
                        )
                    })?;
                    comparison = Some(self.suite.write_png(&file, &image)?);
                    record.comparison = Some(file);
                }
                record.stats = stats;
            }
        }
        self.suite.write_record(&Record::Golden(record.clone()))?;
        Ok(GoldenOutcome {
            passed: record.passed,
            updated: record.updated,
            stats: record.stats,
            message: record.message,
            golden,
            comparison,
        })
    }
}

fn verdict(
    stats: &DiffStats,
    tolerance: GoldenTolerance,
    expected: &RgbaImage,
    actual: &RgbaImage,
) -> String {
    if stats.size_mismatch {
        return format!(
            "size changed from {}×{} to {}×{} device pixels",
            expected.width(),
            expected.height(),
            actual.width(),
            actual.height()
        );
    }
    let percent = |fraction: f64| format!("{:.4}%", fraction * 100.);
    if stats.differing == 0 {
        return if stats.changed == 0 {
            "identical".into()
        } else {
            format!(
                "matches: {} pixels differ only by noise (max ΔE {:.3} ≤ {})",
                stats.changed,
                stats.max_delta_e,
                trim(tolerance.max_delta_e)
            )
        };
    }
    let region = stats
        .bounds
        .map(|bounds| format!(" in {bounds}"))
        .unwrap_or_default();
    let relation = if stats.fraction <= tolerance.max_fraction {
        "within"
    } else {
        "over"
    };
    format!(
        "{} pixels ({}) differ by more than ΔE {}{region}, {relation} the {} allowed; max ΔE {:.3}",
        stats.differing,
        percent(stats.fraction),
        trim(tolerance.max_delta_e),
        percent(tolerance.max_fraction),
        stats.max_delta_e
    )
}

/// Expected, actual and heatmap side by side, for reviewing a failure in one image.
fn side_by_side(
    composer: &mut Composer,
    name: &str,
    expected: &RgbaImage,
    actual: &RgbaImage,
    heatmap: &RgbaImage,
    message: &str,
) -> Result<RgbaImage> {
    const MARGIN: f32 = 24.;
    const GAP: f32 = 16.;
    const PANEL: f32 = 420.;
    let aspect = actual.height() as f32 / actual.width().max(1) as f32;
    let (w, h) = (PANEL, (PANEL * aspect).round());
    let scale = 2.;
    let images = [
        ("Expected", expected),
        ("Actual", actual),
        ("Difference", heatmap),
    ]
    .map(|(title, image)| {
        (
            title,
            composer.image(&resample(image, (w * scale) as u32, (h * scale) as u32)),
        )
    });
    let width = MARGIN * 2. + w * 3. + GAP * 2.;
    let height = MARGIN * 2. + 64. + 24. + h;
    let title = name.to_string();
    let message = message.to_string();
    let images = Rc::new(images);
    composer.render(width, height, scale, move |_, _| {
        let mut root = backdrop()
            .child(
                placed(Rect::new(MARGIN, MARGIN, width - MARGIN * 2., 48.))
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(section("Golden"))
                            .child(heading(title.clone(), 18., DARK.text))
                            .child(mono(message.clone(), 11., DARK.text_muted)),
                    )
                    .child(pill("mismatch", DARK.crit)),
            )
            .child(placed(Rect::new(0., MARGIN + 52., width, 1.)).bg(DARK.line));
        for (ix, (title, image)) in images.iter().enumerate() {
            let x = MARGIN + ix as f32 * (w + GAP);
            let top = MARGIN + 64.;
            root = root
                .child(placed(Rect::new(x, top, w, 18.)).child(mono(
                    title.to_string(),
                    11.,
                    DARK.text_muted,
                )))
                .child(
                    placed(Rect::new(x, top + 24., w, h).inflate(1.))
                        .border_1()
                        .border_color(DARK.line_strong),
                )
                .child(picture(image.clone(), Rect::new(x, top + 24., w, h)));
        }
        root.into_any_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(offset: u32) -> RgbaImage {
        let mut image = RgbaImage::from_pixel(64, 48, Rgba([250, 250, 250, 255]));
        for y in 10..30 {
            for x in 10 + offset..40 + offset {
                image.put_pixel(x, y, Rgba([44, 107, 232, 255]));
            }
        }
        image
    }

    #[test]
    fn detects_a_one_pixel_shift() {
        let (stats, _) = compare(&card(0), &card(1), GoldenTolerance::default(), 1.);
        assert_eq!(stats.differing, 40, "a column enters and one leaves");
        assert!(stats.fraction > GoldenTolerance::default().max_fraction);
        assert_eq!(stats.bounds, Some(Rect::new(10., 10., 31., 20.)));
    }

    #[test]
    fn ignores_sub_threshold_noise() {
        let expected = card(0);
        let mut noisy = expected.clone();
        for (ix, pixel) in noisy.pixels_mut().enumerate() {
            let nudge = if ix % 3 == 0 { 2 } else { 0 };
            pixel.0[0] = pixel.0[0].saturating_sub(nudge);
            pixel.0[2] = pixel.0[2].saturating_add(nudge / 2);
        }
        let (stats, _) = compare(&expected, &noisy, GoldenTolerance::default(), 1.);
        assert!(stats.changed > 1000);
        assert_eq!(stats.differing, 0, "max ΔE {}", stats.max_delta_e);
        let (exact, _) = compare(&expected, &noisy, GoldenTolerance::exact(), 1.);
        assert_eq!(exact.differing, exact.changed);
    }
}
