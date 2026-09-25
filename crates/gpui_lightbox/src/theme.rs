//! Loupe's design tokens (see `crates/gpui_inspector/DESIGN.md`), used for
//! everything Lightbox draws itself: contact sheets, film strips, lint
//! annotations and comparisons. Color only means state.

use crate::color::Color;

/// Semantic colors for one appearance. Names describe roles, not hues.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Backdrop behind everything.
    pub bg: Color,
    /// Panes and cards.
    pub surface: Color,
    /// Raised controls, headers, chips.
    pub surface_2: Color,
    /// Hairline rules.
    pub line: Color,
    /// Stronger rules and control borders.
    pub line_strong: Color,
    /// Primary text.
    pub text: Color,
    /// Secondary text.
    pub text_muted: Color,
    /// Tertiary text.
    pub text_faint: Color,
    /// Selection, focus, the current thing.
    pub accent: Color,
    /// Within budget; passing.
    pub ok: Color,
    /// Worth a look.
    pub warn: Color,
    /// Failing.
    pub crit: Color,
}

/// Loupe's dark palette, the backdrop for Lightbox's composed images.
pub const DARK: Palette = Palette {
    bg: Color::from_u32(0x16171a),
    surface: Color::from_u32(0x1c1d21),
    surface_2: Color::from_u32(0x24252a),
    line: Color::from_u32(0x2a2b30),
    line_strong: Color::from_u32(0x3a3c43),
    text: Color::from_u32(0xdcdee4),
    text_muted: Color::from_u32(0x9ca0aa),
    text_faint: Color::from_u32(0x6b6f79),
    accent: Color::from_u32(0x5c9aff),
    ok: Color::from_u32(0x45b582),
    warn: Color::from_u32(0xe3a83f),
    crit: Color::from_u32(0xef625d),
};

/// Loupe's light palette.
pub const LIGHT: Palette = Palette {
    bg: Color::from_u32(0xffffff),
    surface: Color::from_u32(0xf7f7f9),
    surface_2: Color::from_u32(0xeff0f3),
    line: Color::from_u32(0xe4e6ea),
    line_strong: Color::from_u32(0xd0d3d9),
    text: Color::from_u32(0x1c1f24),
    text_muted: Color::from_u32(0x5d636e),
    text_faint: Color::from_u32(0x8d939d),
    accent: Color::from_u32(0x2c6be8),
    ok: Color::from_u32(0x1f9a5b),
    warn: Color::from_u32(0xbf7d12),
    crit: Color::from_u32(0xd63c3c),
};

/// Categorical colors for tracked-property curves on the dark backdrop, in
/// fixed order (never cycled past six). Validated for color-vision
/// deficiencies on `DARK.surface`: adjacent pairs ΔE ≥ 8.4 under protan
/// simulation, ≥ 19 for normal vision, all ≥ 3:1 against the surface. Distinct
/// from the ok / warn / crit status colors, which marks reserve for findings.
pub const SERIES: [Color; 6] = [
    Color::from_u32(0x3987e5),
    Color::from_u32(0xd95926),
    Color::from_u32(0x199e70),
    Color::from_u32(0xc98500),
    Color::from_u32(0xd55181),
    Color::from_u32(0x008300),
];
