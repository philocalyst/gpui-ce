//! Loupe's design tokens: colors, the phase ramp, frame grades, sizes and fonts.
//!
//! Every surface reads its colors and sizes from the [`Theme`] returned by
//! [`Theme::of`], which follows the window's appearance unless the user picked
//! one in [`LoupeSettings`]. Color only ever means state: neutrals for chrome,
//! the accent for selection and focus, and ok / warn / crit for budgets.

use gpui::{App, Global, Hsla, Pixels, Window, WindowAppearance, px, rgb, rgb_to_hsla, rgba};
use std::{sync::LazyLock, time::Duration};

/// The UI font Loupe renders with. Embedded, so every platform looks the same.
pub const UI_FONT: &str = "IBM Plex Sans";
/// The font Loupe uses for values, paths, ids and code.
pub const MONO_FONT: &str = "Lilex";

/// Which palette Loupe uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    /// Follow the inspected window's light or dark appearance.
    #[default]
    System,
    /// Always dark.
    Dark,
    /// Always light.
    Light,
}

/// How tightly Loupe packs rows and controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    /// 22 px rows, 20/24 px controls: the default, per the design contract.
    #[default]
    Compact,
    /// 4 px taller rows and controls and 1 px larger text.
    Comfortable,
}

/// Loupe's user preferences. A global because they outlive any one Loupe and
/// apply to every inspected window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoupeSettings {
    /// Palette choice.
    pub appearance: Appearance,
    /// Row and control sizing.
    pub density: Density,
}

impl Global for LoupeSettings {}

impl LoupeSettings {
    /// The current settings, or the defaults if none were set.
    pub fn get(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }
}

/// Semantic colors. Names describe roles, not hues.
#[derive(Clone, Debug)]
pub struct Colors {
    /// Content panes (lens bodies, lists).
    pub bg: Hsla,
    /// Chrome: toolbar, pulse strip, rail and status bar.
    pub surface: Hsla,
    /// Raised controls, inputs, table headers and floating layers.
    pub surface_2: Hsla,
    /// Translucent wash for hovered rows and controls.
    pub hover: Hsla,
    /// Translucent wash for pressed controls.
    pub pressed: Hsla,
    /// Translucent accent wash for selected rows and toggled controls.
    pub selected: Hsla,
    /// Hairline rules between panes and rows.
    pub line: Hsla,
    /// Stronger rules: control borders, floating layer edges.
    pub line_strong: Hsla,
    /// Primary text.
    pub text: Hsla,
    /// Secondary text: labels, units, inactive tabs.
    pub text_muted: Hsla,
    /// Tertiary text: placeholders, hints, disabled controls.
    pub text_faint: Hsla,
    /// Selection, focus and toggled state.
    pub accent: Hsla,
    /// Text and icons drawn on a solid accent fill.
    pub on_accent: Hsla,
    /// Within budget.
    pub ok: Hsla,
    /// Over budget, up to 1.5×.
    pub warn: Hsla,
    /// Well over budget, errors.
    pub crit: Hsla,
    /// Stateful views (`Entity<V: Render>`).
    pub view: Hsla,
    /// Stateless `RenderOnce` components.
    pub component: Hsla,
    /// Drop shadow under floating layers.
    pub shadow: Hsla,
    /// Box model diagram: margin (web convention: orange).
    pub box_margin: Hsla,
    /// Box model diagram: border (yellow).
    pub box_border: Hsla,
    /// Box model diagram: padding (green).
    pub box_padding: Hsla,
    /// Box model diagram: content (blue).
    pub box_content: Hsla,
}

/// The phases of a frame, in the order they happen. Used identically by the
/// pulse strip, phase bars and flame charts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Input handling that led to the frame.
    Input,
    /// `Render::render` and `request_layout`.
    Render,
    /// Taffy layout.
    Layout,
    /// Prepaint: bounds and hitboxes.
    Prepaint,
    /// Paint: scene building.
    Paint,
    /// GPU submission.
    Present,
    /// Loupe's own share of the frame.
    Inspector,
}

impl Phase {
    /// Every phase, in frame order.
    pub const ALL: [Phase; 7] = [
        Phase::Input,
        Phase::Render,
        Phase::Layout,
        Phase::Prepaint,
        Phase::Paint,
        Phase::Present,
        Phase::Inspector,
    ];

    /// Lowercase label, as shown in legends.
    pub fn label(self) -> &'static str {
        match self {
            Phase::Input => "input",
            Phase::Render => "render",
            Phase::Layout => "layout",
            Phase::Prepaint => "prepaint",
            Phase::Paint => "paint",
            Phase::Present => "present",
            Phase::Inspector => "loupe",
        }
    }
}

/// How a duration compares to the frame budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grade {
    /// At or under budget.
    Ok,
    /// Over budget by at most half of it.
    Warn,
    /// More than 1.5× the budget.
    Crit,
}

impl Grade {
    /// Grades `duration` against `budget`: ok ≤ 1×, warn ≤ 1.5×, crit above.
    pub fn of(duration: Duration, budget: Duration) -> Self {
        if duration <= budget {
            Grade::Ok
        } else if duration.as_secs_f64() <= budget.as_secs_f64() * 1.5 {
            Grade::Warn
        } else {
            Grade::Crit
        }
    }
}

/// Sizes on Loupe's 4 px grid.
#[derive(Clone, Debug)]
pub struct Metrics {
    /// Tree and table rows.
    pub row: Pixels,
    /// Property grid rows.
    pub property_row: Pixels,
    /// Toolbar height.
    pub toolbar: Pixels,
    /// Pulse strip height.
    pub pulse: Pixels,
    /// Lens rail height.
    pub rail: Pixels,
    /// Status bar height.
    pub status: Pixels,
    /// Regular controls (buttons, fields).
    pub control: Pixels,
    /// Small controls (inline buttons in rows and headers).
    pub control_small: Pixels,
    /// Icon size inside regular controls.
    pub icon: Pixels,
    /// Icon size inside small controls and rows.
    pub icon_small: Pixels,
    /// Body text.
    pub text: Pixels,
    /// Secondary text.
    pub text_small: Pixels,
    /// Uppercase section labels.
    pub label: Pixels,
    /// Monospace values.
    pub mono: Pixels,
    /// Line height for single-line text in rows and controls.
    pub line_height: Pixels,
    /// Corner radius of pressables.
    pub radius: Pixels,
    /// Corner radius of floating layers.
    pub radius_floating: Pixels,
    /// Horizontal padding inside panes.
    pub gutter: Pixels,
    /// Indentation per tree level.
    pub indent: Pixels,
}

impl Metrics {
    fn for_density(density: Density) -> Self {
        let (grow, text_grow) = match density {
            Density::Compact => (px(0.), px(0.)),
            Density::Comfortable => (px(4.), px(1.)),
        };
        Metrics {
            row: px(22.) + grow,
            property_row: px(24.) + grow,
            toolbar: px(32.) + grow,
            pulse: px(40.) + grow,
            rail: px(28.) + grow,
            status: px(22.) + grow,
            control: px(24.) + grow,
            control_small: px(20.) + grow,
            icon: px(14.),
            icon_small: px(12.),
            text: px(12.) + text_grow,
            text_small: px(11.) + text_grow,
            label: px(10.5) + text_grow,
            mono: px(10.5) + text_grow,
            line_height: px(16.) + text_grow,
            radius: px(4.),
            radius_floating: px(8.),
            gutter: px(8.),
            indent: px(12.),
        }
    }
}

/// Colors, phase ramp and sizes for one appearance and density.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Whether this is the dark palette.
    pub is_dark: bool,
    /// The density the metrics were built for.
    pub density: Density,
    /// Semantic colors.
    pub colors: Colors,
    /// Sizes.
    pub metrics: Metrics,
    phases: [Hsla; 7],
    /// Text drawn on phase fills (the same dark ink in both palettes).
    pub phase_ink: Hsla,
}

fn hex(value: u32) -> Hsla {
    rgb_to_hsla(rgb(value))
}

fn hexa(value: u32) -> Hsla {
    rgb_to_hsla(rgba(value))
}

static THEMES: LazyLock<[Theme; 4]> = LazyLock::new(|| {
    [
        Theme::build(true, Density::Compact),
        Theme::build(true, Density::Comfortable),
        Theme::build(false, Density::Compact),
        Theme::build(false, Density::Comfortable),
    ]
});

impl Theme {
    /// The theme for `window`, honoring [`LoupeSettings`].
    pub fn of(window: &Window, cx: &App) -> &'static Theme {
        let settings = LoupeSettings::get(cx);
        let dark = match settings.appearance {
            Appearance::System => matches!(
                window.appearance(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
            Appearance::Dark => true,
            Appearance::Light => false,
        };
        Theme::get(dark, settings.density)
    }

    /// The dark or light theme at `density`.
    pub fn get(dark: bool, density: Density) -> &'static Theme {
        let ix = match (dark, density) {
            (true, Density::Compact) => 0,
            (true, Density::Comfortable) => 1,
            (false, Density::Compact) => 2,
            (false, Density::Comfortable) => 3,
        };
        &THEMES[ix]
    }

    fn build(dark: bool, density: Density) -> Self {
        let colors = if dark { dark_colors() } else { light_colors() };
        let phases = if dark {
            [
                hex(0x7fa7f5),
                hex(0xb69cf3),
                hex(0x5ec8b2),
                hex(0xd9c168),
                hex(0xef9a7c),
                hex(0x8e95a3),
                hex(0x4d525d),
            ]
        } else {
            [
                hex(0x8fb0f7),
                hex(0xbea6f5),
                hex(0x74cdbb),
                hex(0xe2c878),
                hex(0xf3a78b),
                hex(0xa9afbb),
                hex(0xd5d8de),
            ]
        };
        Theme {
            is_dark: dark,
            density,
            colors,
            metrics: Metrics::for_density(density),
            phases,
            phase_ink: hex(0x17181c),
        }
    }

    /// The fill color of a frame phase.
    pub fn phase(&self, phase: Phase) -> Hsla {
        self.phases[phase as usize]
    }

    /// The color of a grade.
    pub fn grade_color(&self, grade: Grade) -> Hsla {
        match grade {
            Grade::Ok => self.colors.ok,
            Grade::Warn => self.colors.warn,
            Grade::Crit => self.colors.crit,
        }
    }

    /// The color of `duration` graded against `budget`.
    pub fn grade(&self, duration: Duration, budget: Duration) -> Hsla {
        self.grade_color(Grade::of(duration, budget))
    }
}

fn dark_colors() -> Colors {
    Colors {
        bg: hex(0x16171a),
        surface: hex(0x1c1d21),
        surface_2: hex(0x24252a),
        hover: hexa(0xffffff0d),
        pressed: hexa(0xffffff1a),
        selected: hexa(0x5c9aff33),
        line: hex(0x2a2b30),
        line_strong: hex(0x3a3c43),
        text: hex(0xdcdee4),
        text_muted: hex(0x9ca0aa),
        text_faint: hex(0x6b6f79),
        accent: hex(0x5c9aff),
        on_accent: hex(0xffffff),
        ok: hex(0x45b582),
        warn: hex(0xe3a83f),
        crit: hex(0xef625d),
        view: hex(0xb392f0),
        component: hex(0x4fc1c9),
        shadow: hexa(0x00000080),
        box_margin: hexa(0xf6a35c59),
        box_border: hexa(0xf2d46b59),
        box_padding: hexa(0x8fcf7e59),
        box_content: hexa(0x7eb0f559),
    }
}

fn light_colors() -> Colors {
    Colors {
        bg: hex(0xffffff),
        surface: hex(0xf7f7f9),
        surface_2: hex(0xeff0f3),
        hover: hexa(0x0000000b),
        pressed: hexa(0x00000017),
        selected: hexa(0x2c6be824),
        line: hex(0xe4e6ea),
        line_strong: hex(0xd0d3d9),
        text: hex(0x1c1f24),
        text_muted: hex(0x5d636e),
        text_faint: hex(0x8d939d),
        accent: hex(0x2c6be8),
        on_accent: hex(0xffffff),
        ok: hex(0x1f9a5b),
        warn: hex(0xbf7d12),
        crit: hex(0xd63c3c),
        view: hex(0x7a4fd6),
        component: hex(0x0b8d93),
        shadow: hexa(0x1c1f2433),
        box_margin: hexa(0xf6a35c66),
        box_border: hexa(0xf2d46b66),
        box_padding: hexa(0x8fcf7e66),
        box_content: hexa(0x7eb0f566),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: f64) -> Duration {
        Duration::from_secs_f64(value / 1000.)
    }

    #[test]
    fn grades_follow_the_budget() {
        let budget = ms(16.667);
        assert_eq!(Grade::of(ms(3.), budget), Grade::Ok);
        assert_eq!(Grade::of(budget, budget), Grade::Ok);
        assert_eq!(Grade::of(ms(20.), budget), Grade::Warn);
        assert_eq!(Grade::of(ms(25.), budget), Grade::Warn);
        assert_eq!(Grade::of(ms(25.1), budget), Grade::Crit);
    }

    #[test]
    fn phases_have_distinct_colors_in_both_themes() {
        for dark in [true, false] {
            let theme = Theme::get(dark, Density::Compact);
            for (ix, a) in Phase::ALL.iter().enumerate() {
                for b in &Phase::ALL[ix + 1..] {
                    assert_ne!(theme.phase(*a), theme.phase(*b), "{a:?} vs {b:?}");
                }
            }
        }
    }

    #[test]
    fn comfortable_density_grows_rows() {
        let compact = Theme::get(true, Density::Compact);
        let comfortable = Theme::get(true, Density::Comfortable);
        assert_eq!(compact.metrics.row, px(22.));
        assert!(comfortable.metrics.row > compact.metrics.row);
    }
}
