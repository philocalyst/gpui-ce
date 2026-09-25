//! Text contrast as WCAG 2 defines it, from gpui's [`Hsla`] colors.
//!
//! Colors are converted to gamma-encoded sRGB, translucent layers are
//! composited in that space (as browsers and gpui's renderer blend), and the
//! contrast ratio is computed from relative luminance.

use gpui::{Hsla, Pixels, hsla_to_rgba, px};

/// WCAG AA minimum contrast for normal text.
pub const AA_NORMAL_TEXT: f32 = 4.5;
/// WCAG AA minimum contrast for large text.
pub const AA_LARGE_TEXT: f32 = 3.0;
/// WCAG "large" text: 18 pt, which is 24 logical pixels. (14 pt bold also
/// counts as large, but font weight is not captured, so bold text is held to
/// the normal-text minimum.)
pub const LARGE_TEXT: Pixels = px(24.0);

/// Alpha at or above this counts as opaque.
const OPAQUE_ALPHA: f32 = 1.0 - 1e-3;
/// Below this, a gamma-encoded channel is on the linear segment of the sRGB curve.
const SRGB_LINEAR_KNEE: f32 = 0.04045;
const SRGB_LINEAR_SLOPE: f32 = 12.92;
const SRGB_GAMMA: f32 = 2.4;
const SRGB_OFFSET: f32 = 0.055;
/// Luminance weights of the sRGB primaries.
const LUMINANCE_WEIGHTS: [f32; 3] = [0.2126, 0.7152, 0.0722];
/// Flare term that keeps the ratio finite for black.
const FLARE: f32 = 0.05;

/// An opaque, gamma-encoded sRGB color with channels in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Srgb {
    /// Red.
    pub red: f32,
    /// Green.
    pub green: f32,
    /// Blue.
    pub blue: f32,
}

impl Srgb {
    /// The color's RGB channels, ignoring its alpha.
    pub fn from_hsla(color: Hsla) -> Self {
        let rgba = hsla_to_rgba(color);
        let channel = |value: f32| {
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        Self {
            red: channel(rgba.color.red),
            green: channel(rgba.color.green),
            blue: channel(rgba.color.blue),
        }
    }

    /// `color` painted over this color, blended by its alpha.
    pub fn under(self, color: Hsla) -> Self {
        let top = Self::from_hsla(color);
        let alpha = if color.alpha.is_finite() {
            color.alpha.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let blend = |top: f32, bottom: f32| top * alpha + bottom * (1.0 - alpha);
        Self {
            red: blend(top.red, self.red),
            green: blend(top.green, self.green),
            blue: blend(top.blue, self.blue),
        }
    }

    /// WCAG relative luminance, 0.0 (black) to 1.0 (white).
    pub fn relative_luminance(self) -> f32 {
        let linear = |channel: f32| {
            if channel <= SRGB_LINEAR_KNEE {
                channel / SRGB_LINEAR_SLOPE
            } else {
                ((channel + SRGB_OFFSET) / (1.0 + SRGB_OFFSET)).powf(SRGB_GAMMA)
            }
        };
        let [red, green, blue] = LUMINANCE_WEIGHTS;
        red * linear(self.red) + green * linear(self.green) + blue * linear(self.blue)
    }

    /// `#rrggbb`, for messages.
    pub fn hex(self) -> String {
        let byte = |channel: f32| (channel * 255.0).round() as u8;
        format!(
            "#{:02x}{:02x}{:02x}",
            byte(self.red),
            byte(self.green),
            byte(self.blue)
        )
    }
}

/// WCAG relative luminance of `color`, ignoring its alpha.
pub fn relative_luminance(color: Hsla) -> f32 {
    Srgb::from_hsla(color).relative_luminance()
}

/// The WCAG contrast ratio between two opaque colors, 1.0 to 21.0; symmetric.
pub fn contrast_ratio(a: Srgb, b: Srgb) -> f32 {
    let (a, b) = (a.relative_luminance(), b.relative_luminance());
    (a.max(b) + FLARE) / (a.min(b) + FLARE)
}

/// Whether `color` is (effectively) opaque.
pub fn is_opaque(color: Hsla) -> bool {
    color.alpha >= OPAQUE_ALPHA
}

/// Composites `layers`, listed from the top (nearest to the text) down, and
/// returns the resulting opaque color. `None` when no layer is opaque, since
/// then what shows through is unknown.
pub fn composite(layers: impl IntoIterator<Item = Hsla>) -> Option<Srgb> {
    let mut translucent = Vec::new();
    for layer in layers {
        if is_opaque(layer) {
            let base = Srgb::from_hsla(layer);
            return Some(translucent.into_iter().rev().fold(base, Srgb::under));
        }
        translucent.push(layer);
    }
    None
}

/// The AA minimum for text of `font_size` (normal text when unknown).
pub fn required_ratio(font_size: Option<Pixels>) -> f32 {
    match font_size {
        Some(size) if size >= LARGE_TEXT => AA_LARGE_TEXT,
        _ => AA_NORMAL_TEXT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{black, hsla, rgb, rgb_to_hsla, white};

    fn color(hex: u32) -> Hsla {
        rgb_to_hsla(rgb(hex))
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn luminance_of_the_extremes_and_primaries() {
        assert!(close(relative_luminance(black()), 0.0));
        assert!(close(relative_luminance(white()), 1.0));
        assert!(close(relative_luminance(color(0xff0000)), 0.2126));
        assert!(close(relative_luminance(color(0x00ff00)), 0.7152));
        assert!(close(relative_luminance(color(0x0000ff)), 0.0722));
        // Mid grey #777777 (a common "is this accessible" reference).
        assert!(close(relative_luminance(color(0x777777)), 0.1845));
    }

    #[test]
    fn contrast_matches_wcag_reference_values() {
        let ratio = |a, b| contrast_ratio(Srgb::from_hsla(color(a)), Srgb::from_hsla(color(b)));
        assert!(close(ratio(0x000000, 0xffffff), 21.0));
        assert!(close(ratio(0xffffff, 0x000000), 21.0));
        assert!(close(ratio(0x777777, 0xffffff), 4.478));
        assert!(close(ratio(0x767676, 0xffffff), 4.542));
        assert!(close(ratio(0x3366ff, 0x3366ff), 1.0));
    }

    #[test]
    fn translucent_layers_composite_over_the_first_opaque_one() {
        let half_black = hsla(0., 0., 0., 0.5);
        let over_white = composite([half_black, white()]).unwrap();
        assert!(close(over_white.red, 0.5) && close(over_white.blue, 0.5));
        assert_eq!(over_white.hex(), "#808080");
        // Layers under the first opaque one don't matter.
        assert_eq!(composite([white(), black()]).unwrap().hex(), "#ffffff");
        // Two translucent layers stack.
        let stacked = composite([half_black, half_black, white()]).unwrap();
        assert!(close(stacked.red, 0.25));
        // Nothing opaque: unknown.
        assert_eq!(composite([half_black]), None);
        assert_eq!(composite(std::iter::empty()), None);
    }

    #[test]
    fn non_finite_colors_do_not_poison_the_math() {
        let broken = hsla(0., 0., 0., 1.);
        let broken = Hsla {
            alpha: f32::NAN,
            ..broken
        };
        let base = Srgb::from_hsla(white());
        assert_eq!(base.under(broken), base);
        assert!(contrast_ratio(base, base).is_finite());
    }

    #[test]
    fn large_text_has_a_lower_minimum() {
        assert_eq!(required_ratio(None), AA_NORMAL_TEXT);
        assert_eq!(required_ratio(Some(px(12.))), AA_NORMAL_TEXT);
        assert_eq!(required_ratio(Some(px(24.))), AA_LARGE_TEXT);
    }
}
