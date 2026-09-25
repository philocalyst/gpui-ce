//! Colors as Lightbox measures them: sRGB with straight alpha, compared in
//! Oklab (perceptual ΔE) and rated with the WCAG contrast ratio.

use gpui::{Hsla, Rgba};
use serde::{Deserialize, Serialize};
use std::fmt;

/// An sRGB color with straight (non-premultiplied) alpha, channels in `0..=1`.
///
/// Serializes as a `#rrggbb` / `#rrggbbaa` hex string, so style specs and
/// manifests stay readable.
///
/// ```ignore
/// let text = Color::from_hex("#dcdee4").unwrap();
/// let bg = Color::from_hex("#16171a").unwrap();
/// assert!(text.contrast(bg) > 12.0);
/// assert!(text.delta_e(Color::from_hex("#dddfe5").unwrap()) < 0.01);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color {
    /// Red, `0..=1`.
    pub r: f32,
    /// Green, `0..=1`.
    pub g: f32,
    /// Blue, `0..=1`.
    pub b: f32,
    /// Alpha, `0..=1`.
    pub a: f32,
}

impl Color {
    /// Opaque black.
    pub const BLACK: Self = Self::rgb(0., 0., 0.);
    /// Opaque white.
    pub const WHITE: Self = Self::rgb(1., 1., 1.);

    /// An opaque color from channels in `0..=1`.
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1. }
    }

    /// A color from 8-bit RGBA channels, e.g. a pixel of an `RgbaImage`.
    pub fn from_rgba8([r, g, b, a]: [u8; 4]) -> Self {
        Self {
            r: f32::from(r) / 255.,
            g: f32::from(g) / 255.,
            b: f32::from(b) / 255.,
            a: f32::from(a) / 255.,
        }
    }

    /// An opaque color from a `0xrrggbb` literal.
    pub const fn from_u32(rgb: u32) -> Self {
        let [_, r, g, b] = rgb.to_be_bytes();
        Self::rgb(r as f32 / 255., g as f32 / 255., b as f32 / 255.)
    }

    /// Parses `#rgb`, `#rrggbb` or `#rrggbbaa` (the `#` is optional).
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim().trim_start_matches('#');
        let channel = |ix: usize| u8::from_str_radix(hex.get(ix..ix + 2)?, 16).ok();
        match hex.len() {
            3 => {
                let mut channels = [255; 4];
                for (ix, digit) in hex.chars().enumerate() {
                    let value = u8::try_from(digit.to_digit(16)?).ok()?;
                    channels[ix] = value * 17;
                }
                Some(Self::from_rgba8(channels))
            }
            6 => Some(Self::from_rgba8([
                channel(0)?,
                channel(2)?,
                channel(4)?,
                255,
            ])),
            8 => Some(Self::from_rgba8([
                channel(0)?,
                channel(2)?,
                channel(4)?,
                channel(6)?,
            ])),
            _ => None,
        }
    }

    /// The color as 8-bit RGBA, rounded.
    pub fn to_rgba8(self) -> [u8; 4] {
        let quantize = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
        [
            quantize(self.r),
            quantize(self.g),
            quantize(self.b),
            quantize(self.a),
        ]
    }

    /// `#rrggbb` for opaque colors, `#rrggbbaa` otherwise.
    pub fn hex(self) -> String {
        let [r, g, b, a] = self.to_rgba8();
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }

    /// The same color with a different alpha.
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// This color composited over `background` (source-over, in sRGB space
    /// as the renderer blends), returning an opaque color when the background
    /// is opaque.
    pub fn over(self, background: Color) -> Color {
        let a = self.a + background.a * (1. - self.a);
        if a <= 0. {
            return Color {
                a: 0.,
                ..background
            };
        }
        let blend = |fg: f32, bg: f32| (fg * self.a + bg * background.a * (1. - self.a)) / a;
        Color {
            r: blend(self.r, background.r),
            g: blend(self.g, background.g),
            b: blend(self.b, background.b),
            a,
        }
    }

    /// The color in Oklab, ignoring alpha.
    pub fn oklab(self) -> Oklab {
        Oklab::from_linear_srgb(
            srgb_to_linear(self.r),
            srgb_to_linear(self.g),
            srgb_to_linear(self.b),
        )
    }

    /// Perceptual distance to `other`: Euclidean distance in Oklab (ΔEok),
    /// where `0.02` is about one just-noticeable difference and black to
    /// white is `1.0`. Alpha is ignored; composite first with [`Self::over`].
    pub fn delta_e(self, other: Color) -> f32 {
        self.oklab().distance(other.oklab())
    }

    /// WCAG 2 relative luminance, ignoring alpha.
    pub fn relative_luminance(self) -> f32 {
        0.2126 * srgb_to_linear(self.r)
            + 0.7152 * srgb_to_linear(self.g)
            + 0.0722 * srgb_to_linear(self.b)
    }

    /// WCAG 2 contrast ratio against `other`, from 1 (none) to 21.
    pub fn contrast(self, other: Color) -> f32 {
        let (a, b) = (self.relative_luminance(), other.relative_luminance());
        let (light, dark) = if a > b { (a, b) } else { (b, a) };
        (light + 0.05) / (dark + 0.05)
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl From<Hsla> for Color {
    fn from(hsla: Hsla) -> Self {
        gpui::hsla_to_rgba(hsla).into()
    }
}

impl From<Rgba> for Color {
    fn from(rgba: Rgba) -> Self {
        Self {
            r: rgba.red,
            g: rgba.green,
            b: rgba.blue,
            a: rgba.alpha,
        }
    }
}

impl From<Color> for Rgba {
    fn from(color: Color) -> Self {
        Rgba::new(color.r, color.g, color.b, color.a)
    }
}

impl From<Color> for Hsla {
    fn from(color: Color) -> Self {
        gpui::rgb_to_hsla(color.into())
    }
}

/// Lets a [`Color`] go anywhere gpui takes a color (`.bg()`, `.text_color()`…).
impl palette::convert::FromColorUnclamped<Color> for Hsla {
    fn from_color_unclamped(color: Color) -> Self {
        color.into()
    }
}

impl From<Color> for String {
    fn from(color: Color) -> Self {
        color.hex()
    }
}

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(hex: String) -> Result<Self, Self::Error> {
        Color::from_hex(&hex).ok_or_else(|| format!("invalid color {hex:?}, expected #rrggbb"))
    }
}

/// A color in the Oklab perceptual space (Björn Ottosson, 2020).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklab {
    /// Perceived lightness, `0..=1`.
    pub l: f32,
    /// Green (negative) to red (positive).
    pub a: f32,
    /// Blue (negative) to yellow (positive).
    pub b: f32,
}

impl Oklab {
    /// Converts linear-light sRGB channels to Oklab.
    pub fn from_linear_srgb(r: f32, g: f32, b: f32) -> Self {
        let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        Self {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        }
    }

    /// Euclidean distance (ΔEok).
    pub fn distance(self, other: Oklab) -> f32 {
        ((self.l - other.l).powi(2) + (self.a - other.a).powi(2) + (self.b - other.b).powi(2))
            .sqrt()
    }
}

/// Converts one sRGB-encoded channel to linear light.
pub fn srgb_to_linear(channel: f32) -> f32 {
    if channel <= 0.040_45 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// A lookup table of Oklab values for 8-bit channels, to compare millions of
/// pixels quickly: linearization is the expensive part.
pub(crate) struct LinearTable([f32; 256]);

impl LinearTable {
    pub(crate) fn new() -> Self {
        let mut table = [0.; 256];
        for (ix, value) in table.iter_mut().enumerate() {
            *value = srgb_to_linear(ix as f32 / 255.);
        }
        Self(table)
    }

    /// The Oklab value of an 8-bit pixel. Translucent pixels are
    /// premultiplied (composited over black), so they compare sensibly.
    pub(crate) fn oklab(&self, [r, g, b, a]: [u8; 4]) -> Oklab {
        if a == 255 {
            return Oklab::from_linear_srgb(
                self.0[r as usize],
                self.0[g as usize],
                self.0[b as usize],
            );
        }
        let alpha = f32::from(a) / 255.;
        Oklab::from_linear_srgb(
            self.0[r as usize] * alpha,
            self.0[g as usize] * alpha,
            self.0[b as usize] * alpha,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        for hex in ["#16171a", "#5c9aff", "#ffffff", "#00000080"] {
            assert_eq!(Color::from_hex(hex).unwrap().hex(), hex);
        }
        assert_eq!(Color::from_hex("#fff").unwrap(), Color::WHITE);
        assert_eq!(Color::from_hex("nope"), None);
        let json = serde_json::to_string(&Color::from_u32(0x2c6be8)).unwrap();
        assert_eq!(json, "\"#2c6be8\"");
        assert_eq!(
            serde_json::from_str::<Color>(&json).unwrap().hex(),
            "#2c6be8"
        );
    }

    #[test]
    fn oklab_matches_reference_values() {
        // Reference values from Ottosson's post.
        let white = Color::WHITE.oklab();
        assert!((white.l - 1.0).abs() < 1e-3 && white.a.abs() < 1e-3 && white.b.abs() < 1e-3);
        let red = Color::rgb(1., 0., 0.).oklab();
        assert!((red.l - 0.628).abs() < 1e-3);
        assert!((red.a - 0.2249).abs() < 1e-3);
        assert!((red.b - 0.1258).abs() < 1e-3);
        assert!((Color::BLACK.delta_e(Color::WHITE) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((Color::BLACK.contrast(Color::WHITE) - 21.).abs() < 1e-3);
        // #767676 on white is the classic 4.54:1 minimum gray.
        let gray = Color::from_hex("#767676").unwrap();
        assert!((gray.contrast(Color::WHITE) - 4.54).abs() < 0.01);
    }

    #[test]
    fn compositing_blends_in_srgb() {
        let half_white = Color::WHITE.with_alpha(0.5);
        let mixed = half_white.over(Color::BLACK);
        assert!((mixed.r - 0.5).abs() < 1e-6 && mixed.a == 1.);
    }

    #[test]
    fn lookup_table_matches_direct_conversion() {
        let table = LinearTable::new();
        let pixel = [0x5c, 0x9a, 0xff, 255];
        let direct = Color::from_rgba8(pixel).oklab();
        assert!(table.oklab(pixel).distance(direct) < 1e-5);
    }
}
