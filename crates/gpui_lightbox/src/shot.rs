//! A [`Shot`]: one rendered frame with everything needed to check it — the
//! pixels, every painted text line and quad, and how it was rendered.

use crate::{
    color::Color,
    compose::Compositor,
    manifest::{Origin, Record, ShotRecord},
    output::{Suite, slug},
    stage::Appearance,
};
use gpui::{Bounds, Pixels, Quad, ScaledPixels, Window, font};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{fmt, panic::Location, path::PathBuf};

/// A rectangle in logical pixels (window coordinates).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle from its origin and size.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// The right edge.
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// The bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// The center point.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2., self.y + self.h / 2.)
    }

    /// Whether the rectangle covers no area.
    pub fn is_empty(&self) -> bool {
        self.w <= 0. || self.h <= 0.
    }

    /// The area.
    pub fn area(&self) -> f32 {
        self.w.max(0.) * self.h.max(0.)
    }

    /// The overlap of two rectangles (empty if they don't overlap).
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Rect::new(x, y, (right - x).max(0.), (bottom - y).max(0.))
    }

    /// The smallest rectangle containing both.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// Whether `other` lies entirely inside this rectangle, within `tolerance`.
    pub fn contains_rect(&self, other: &Rect, tolerance: f32) -> bool {
        other.x >= self.x - tolerance
            && other.y >= self.y - tolerance
            && other.right() <= self.right() + tolerance
            && other.bottom() <= self.bottom() + tolerance
    }

    /// Whether the point lies inside the rectangle.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    /// The rectangle grown by `amount` on every side.
    pub fn inflate(&self, amount: f32) -> Rect {
        Rect::new(
            self.x - amount,
            self.y - amount,
            self.w + amount * 2.,
            self.h + amount * 2.,
        )
    }

    /// The distance between the closest points of two rectangles (0 when they touch).
    pub fn distance(&self, other: &Rect) -> f32 {
        let dx = (other.x - self.right()).max(self.x - other.right()).max(0.);
        let dy = (other.y - self.bottom())
            .max(self.y - other.bottom())
            .max(0.);
        (dx * dx + dy * dy).sqrt()
    }

    /// The rectangle in device pixels, rounded outward and clamped to `width × height`.
    pub(crate) fn to_device(self, scale: f32, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let x0 = ((self.x * scale).floor().max(0.) as u32).min(width);
        let y0 = ((self.y * scale).floor().max(0.) as u32).min(height);
        let x1 = ((self.right() * scale).ceil().max(0.) as u32).min(width);
        let y1 = ((self.bottom() * scale).ceil().max(0.) as u32).min(height);
        (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    }
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}×{} at ({}, {})",
            trim(self.w),
            trim(self.h),
            trim(self.x),
            trim(self.y)
        )
    }
}

/// Formats a pixel value without a trailing `.0`.
pub(crate) fn trim(value: f32) -> String {
    let rounded = (value * 100.).round() / 100.;
    if rounded.fract() == 0. {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

impl From<Bounds<Pixels>> for Rect {
    fn from(bounds: Bounds<Pixels>) -> Self {
        Rect::new(
            bounds.origin.x.as_f32(),
            bounds.origin.y.as_f32(),
            bounds.size.width.as_f32(),
            bounds.size.height.as_f32(),
        )
    }
}

impl Rect {
    fn from_scaled(bounds: Bounds<ScaledPixels>, scale: f32) -> Self {
        Rect::new(
            bounds.origin.x.as_f32() / scale,
            bounds.origin.y.as_f32() / scale,
            bounds.size.width.as_f32() / scale,
            bounds.size.height.as_f32() / scale,
        )
    }
}

/// A line of text painted in a shot (all wrapped rows of it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextLine {
    /// The text.
    pub text: String,
    /// The line's box (line height × width), before clipping.
    pub bounds: Rect,
    /// The part of the line inside its clip.
    pub visible: Rect,
    /// The requested font family (`.SystemUIFont` is reported unresolved).
    pub font_family: String,
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Font weight (400 regular, 600 semibold, 700 bold).
    pub font_weight: f32,
    /// The color of the line's first run.
    pub color: Color,
    /// Where the baseline sits, for lines that are a single row.
    pub baseline: Option<f32>,
}

impl TextLine {
    pub(crate) fn from_painted(line: &gpui::PaintedText, window: &Window) -> Self {
        let bounds = Rect::from(line.bounds);
        let font_size = line.font_size.as_f32();
        // A single row is shorter than two rows at any sane line height.
        let baseline = (bounds.h < font_size * 2.4).then(|| {
            let text_system = window.text_system();
            let font_id = text_system.resolve_font(&gpui::Font {
                weight: line.font_weight,
                ..font(line.font_family.clone())
            });
            let ascent = text_system.ascent(font_id, line.font_size).as_f32();
            let descent = text_system.descent(font_id, line.font_size).as_f32();
            bounds.y + (bounds.h - ascent - descent) / 2. + ascent
        });
        Self {
            text: line.text.to_string(),
            bounds,
            visible: Rect::from(line.visible_bounds()),
            font_family: line.font_family.to_string(),
            font_size,
            font_weight: line.font_weight.0,
            color: line.color.into(),
            baseline,
        }
    }

    /// Whether part of the line is hidden by its clip (by more than float noise).
    pub fn is_clipped(&self) -> bool {
        let (a, b) = (&self.visible, &self.bounds);
        [
            (a.x, b.x),
            (a.y, b.y),
            (a.right(), b.right()),
            (a.bottom(), b.bottom()),
        ]
        .iter()
        .any(|(a, b)| (a - b).abs() > 0.01)
    }

    /// Whether any of the line is visible.
    pub fn is_visible(&self) -> bool {
        !self.visible.is_empty()
    }
}

/// A quad (background, border, divider…) painted in a shot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuadInfo {
    /// The quad's bounds, before clipping.
    pub bounds: Rect,
    /// The part of the quad inside its content mask.
    pub visible: Rect,
    /// The fill, when it is a solid color.
    pub background: Option<Color>,
    /// Whether the fill is a gradient or pattern.
    pub patterned: bool,
    /// The border color, when it is a solid color.
    pub border_color: Option<Color>,
    /// Border widths: top, right, bottom, left.
    pub border_widths: [f32; 4],
    /// Corner radii: top-left, top-right, bottom-right, bottom-left.
    pub corner_radii: [f32; 4],
}

impl QuadInfo {
    pub(crate) fn from_quad(quad: &Quad, scale: f32) -> Self {
        let bounds = Rect::from_scaled(quad.bounds, scale);
        let clip = Rect::from_scaled(quad.content_mask.bounds, scale);
        let borders = &quad.border_widths;
        let radii = &quad.corner_radii;
        let background = quad.background.as_solid().map(Color::from);
        Self {
            bounds,
            visible: bounds.intersect(&clip),
            patterned: background.is_none(),
            background: background.filter(|color| color.a > 0.),
            border_color: quad.border_color.as_solid().map(Color::from),
            border_widths: [borders.top, borders.right, borders.bottom, borders.left]
                .map(|width| width.as_f32() / scale),
            corner_radii: [
                radii.top_left,
                radii.top_right,
                radii.bottom_right,
                radii.bottom_left,
            ]
            .map(|radius| radius.as_f32() / scale),
        }
    }

    /// Whether the quad has a visible border.
    pub fn has_border(&self) -> bool {
        self.border_widths.iter().any(|width| *width > 0.)
            && self.border_color.is_none_or(|color| color.a > 0.)
    }

    /// Whether the quad paints anything: a fill or a border.
    pub fn is_painted(&self) -> bool {
        self.background.is_some() || self.patterned || self.has_border()
    }
}

/// Merges quads into the boxes a view drew. gpui paints a box's fill and
/// each border edge as separate quads (every edge clipped to its own strip);
/// quads with the same bounds and radii are one box.
pub fn merge_quads(quads: &[QuadInfo]) -> Vec<QuadInfo> {
    let mut boxes: Vec<QuadInfo> = Vec::new();
    for quad in quads {
        let same_box = boxes
            .iter_mut()
            .rev()
            .find(|other| other.bounds == quad.bounds && other.corner_radii == quad.corner_radii);
        let Some(merged) = same_box else {
            boxes.push(quad.clone());
            continue;
        };
        merged.visible = merged.visible.union(&quad.visible);
        merged.background = merged.background.or(quad.background);
        merged.patterned |= quad.patterned;
        if quad.has_border() {
            merged.border_color = quad.border_color;
            for (width, other) in merged.border_widths.iter_mut().zip(quad.border_widths) {
                *width = width.max(other);
            }
        }
    }
    boxes
}

/// How and when a shot was rendered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotMeta {
    /// The window's size in logical pixels.
    pub size: [f32; 2],
    /// Device pixels per logical pixel.
    pub scale: f32,
    /// The window's appearance.
    pub appearance: Appearance,
    /// Stage time when the shot was taken (sum of `advance` calls), in ms.
    pub time_ms: f64,
    /// The family `.SystemUIFont` resolves to on the stage.
    pub system_font: String,
}

/// One rendered frame: pixels, painted text and quads, and metadata.
///
/// Created by [`Stage::shot`](crate::Stage::shot) (saved to
/// `target/lightbox/<suite>/<name>.png`) or [`Stage::capture`](crate::Stage::capture)
/// (in memory only).
///
/// ```ignore
/// let shot = stage.shot("card");
/// assert_eq!(shot.find_text("Save")?.font_family, "IBM Plex Sans");
/// shot.lint(&StyleSpec::loupe()).assert_clean();
/// shot.assert_golden("card");
/// ```
#[derive(Clone)]
pub struct Shot {
    /// The shot's name, unique within its suite.
    pub name: String,
    /// The rendered pixels (device pixels: logical size × scale).
    pub image: RgbaImage,
    /// Every line of text painted, in paint order.
    pub texts: Vec<TextLine>,
    /// Every quad painted, in paint order.
    pub quads: Vec<QuadInfo>,
    /// Visible bounds of clickable elements, when the element tree was
    /// recorded (see [`Stage::record_elements`](crate::Stage::record_elements)).
    pub clickables: Option<Vec<Rect>>,
    /// How and when the shot was rendered.
    pub meta: ShotMeta,
    pub(crate) suite: Suite,
    pub(crate) compositor: Compositor,
}

impl Shot {
    /// The suite the shot belongs to.
    pub fn suite(&self) -> &Suite {
        &self.suite
    }

    /// Where the shot's PNG is (or would be) saved.
    pub fn path(&self) -> PathBuf {
        self.suite.path(&self.file_name())
    }

    pub(crate) fn file_name(&self) -> String {
        format!("{}.png", slug(&self.name))
    }

    /// Device pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        self.meta.scale
    }

    /// The window's bounds in logical pixels.
    pub fn bounds(&self) -> Rect {
        Rect::new(0., 0., self.meta.size[0], self.meta.size[1])
    }

    /// The color of the pixel at logical position `(x, y)`.
    ///
    /// # Panics
    ///
    /// If the point is outside the shot.
    #[track_caller]
    pub fn pixel(&self, x: f32, y: f32) -> Color {
        let (dx, dy) = ((x * self.scale()) as u32, (y * self.scale()) as u32);
        assert!(
            dx < self.image.width() && dy < self.image.height(),
            "({x}, {y}) is outside the {}×{} shot",
            self.meta.size[0],
            self.meta.size[1]
        );
        Color::from_rgba8(self.image.get_pixel(dx, dy).0)
    }

    /// The pixels inside `rect` (logical pixels), as a new image.
    pub fn crop(&self, rect: Rect) -> RgbaImage {
        let (x, y, w, h) = rect.to_device(self.scale(), self.image.width(), self.image.height());
        image::imageops::crop_imm(&self.image, x, y, w, h).to_image()
    }

    /// Visible text lines, in paint order.
    pub fn visible_texts(&self) -> impl Iterator<Item = &TextLine> {
        self.texts.iter().filter(|line| line.is_visible())
    }

    /// The unique visible line whose text is exactly `text` (ignoring
    /// surrounding whitespace).
    pub fn find_text(&self, text: &str) -> Result<&TextLine, TextQueryError> {
        find_text(&self.texts, text)
    }

    /// The boxes the view drew: quads merged by bounds (see [`merge_quads`]).
    pub fn boxes(&self) -> Vec<QuadInfo> {
        merge_quads(&self.quads)
    }

    /// The first quad matching `predicate`, in paint order.
    pub fn quad(&self, predicate: impl Fn(&QuadInfo) -> bool) -> Option<&QuadInfo> {
        self.quads.iter().find(|quad| predicate(quad))
    }

    /// The first quad whose solid fill is within ΔE 0.01 of `color`.
    pub fn quad_filled(&self, color: Color) -> Option<&QuadInfo> {
        self.quad(|quad| {
            quad.background
                .is_some_and(|fill| fill.delta_e(color) < 0.01 && (fill.a - color.a).abs() < 0.02)
        })
    }

    /// Saves the PNG and its record (used by the report and contact sheet).
    pub(crate) fn save(&self, location: &'static Location<'static>) {
        let file = self.file_name();
        if let Err(error) = self.suite.write_png(&file, &self.image) {
            panic!("lightbox: saving shot {:?}: {error:#}", self.name);
        }
        let record = Record::Shot(ShotRecord {
            name: self.name.clone(),
            image: file,
            meta: self.meta.clone(),
            texts: self.texts.clone(),
            quads: self.quads.clone(),
            origin: Origin::at(location),
        });
        if let Err(error) = self.suite.write_record(&record) {
            panic!("lightbox: saving shot {:?}: {error:#}", self.name);
        }
    }
}

impl fmt::Debug for Shot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shot")
            .field("name", &self.name)
            .field("suite", &self.suite.name())
            .field("size", &self.meta.size)
            .field("scale", &self.meta.scale)
            .field("texts", &self.texts.len())
            .field("quads", &self.quads.len())
            .finish()
    }
}

/// Why a text query found no unique match. Its `Display` lists the visible
/// texts that come closest, so a failing test says what *is* on screen.
#[derive(Clone, Debug, PartialEq)]
pub enum TextQueryError {
    /// No visible line has this text.
    NotFound {
        /// The text searched for.
        query: String,
        /// The closest visible lines, best first.
        nearby: Vec<TextLine>,
        /// How many lines were visible.
        visible: usize,
    },
    /// Several visible lines have this text.
    Ambiguous {
        /// The text searched for.
        query: String,
        /// Every matching line.
        matches: Vec<TextLine>,
    },
}

impl fmt::Display for TextQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let line = |f: &mut fmt::Formatter<'_>, text: &TextLine| {
            writeln!(
                f,
                "  {:?}  {}  ({} {}px)",
                text.text,
                text.visible,
                text.font_family,
                trim(text.font_size)
            )
        };
        match self {
            TextQueryError::NotFound {
                query,
                nearby,
                visible,
            } => {
                writeln!(f, "no visible text is exactly {query:?}.")?;
                if nearby.is_empty() {
                    writeln!(f, "Nothing is visible ({visible} lines).")?;
                } else {
                    writeln!(f, "Closest of the {visible} visible lines:")?;
                    for text in nearby {
                        line(f, text)?;
                    }
                }
                Ok(())
            }
            TextQueryError::Ambiguous { query, matches } => {
                writeln!(
                    f,
                    "{} visible lines are {query:?}; click by position instead:",
                    matches.len()
                )?;
                for text in matches {
                    line(f, text)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for TextQueryError {}

pub(crate) fn find_text<'a>(
    texts: &'a [TextLine],
    query: &str,
) -> Result<&'a TextLine, TextQueryError> {
    let wanted = query.trim();
    let visible = texts
        .iter()
        .filter(|line| line.is_visible())
        .collect::<Vec<_>>();
    let matches = visible
        .iter()
        .copied()
        .filter(|line| line.text.trim() == wanted)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [only] => Ok(only),
        [] => {
            let mut nearby = visible.clone();
            nearby.sort_by_key(|line| similarity_rank(&line.text, wanted));
            nearby.truncate(8);
            Err(TextQueryError::NotFound {
                query: query.into(),
                nearby: nearby.into_iter().cloned().collect(),
                visible: visible.len(),
            })
        }
        _ => Err(TextQueryError::Ambiguous {
            query: query.into(),
            matches: matches.into_iter().cloned().collect(),
        }),
    }
}

/// Lower is more similar: containment first, then edit distance.
fn similarity_rank(candidate: &str, query: &str) -> (u8, usize) {
    let candidate = candidate.to_lowercase();
    let query = query.to_lowercase();
    let containment = if candidate.contains(&query) || query.contains(&candidate) {
        0
    } else {
        1
    };
    (containment, edit_distance(&candidate, &query))
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b = b.chars().collect::<Vec<_>>();
    let mut row = (0..=b.len()).collect::<Vec<_>>();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous + usize::from(ca != *cb);
            previous = row[j + 1];
            row[j + 1] = substitution.min(row[j] + 1).min(previous + 1);
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f32) -> TextLine {
        let bounds = Rect::new(x, 10., 40., 16.);
        TextLine {
            text: text.into(),
            bounds,
            visible: bounds,
            font_family: "IBM Plex Sans".into(),
            font_size: 12.,
            font_weight: 400.,
            color: Color::BLACK,
            baseline: Some(22.),
        }
    }

    #[test]
    fn finds_unique_text_and_explains_misses() {
        let texts = vec![line("Save", 0.), line("Cancel", 50.), line("Saved!", 100.)];
        assert_eq!(find_text(&texts, " Save ").unwrap().bounds.x, 0.);

        let error = find_text(&texts, "Sav").unwrap_err().to_string();
        assert!(
            error.contains("no visible text is exactly \"Sav\""),
            "{error}"
        );
        let save = error.find("\"Save\"").unwrap();
        let cancel = error.find("\"Cancel\"").unwrap();
        assert!(save < cancel, "closest first:\n{error}");

        let texts = vec![line("OK", 0.), line("OK", 50.)];
        let error = find_text(&texts, "OK").unwrap_err().to_string();
        assert!(error.starts_with("2 visible lines are \"OK\""), "{error}");
    }

    #[test]
    fn rect_math() {
        let a = Rect::new(0., 0., 10., 10.);
        let b = Rect::new(5., 5., 10., 10.);
        assert_eq!(a.intersect(&b), Rect::new(5., 5., 5., 5.));
        assert_eq!(a.union(&b), Rect::new(0., 0., 15., 15.));
        assert_eq!(a.distance(&Rect::new(13., 0., 1., 1.)), 3.);
        assert_eq!(a.to_device(2., 100, 100), (0, 0, 20, 20));
        assert_eq!(a.to_string(), "10×10 at (0, 0)");
    }
}
