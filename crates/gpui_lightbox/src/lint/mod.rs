//! Style lint: check a [`Shot`] against a declarative [`StyleSpec`] — fonts,
//! sizes, palette, spacing grid, radii, borders, contrast measured on the
//! rendered pixels, clipping, overlaps, baselines, hit targets — and get an
//! annotated PNG with every violation numbered.

mod annotate;
pub mod rules;

use crate::{
    color::Color,
    manifest::{Assertion, LintRecord, Origin, Record},
    output::{Suite, slug},
    shot::{Rect, Shot},
    theme::{DARK, LIGHT},
};
use serde::{Deserialize, Serialize};
use std::{fmt, panic::Location, path::PathBuf};

/// A text style the design allows: a family, and optionally the sizes and
/// weights it may be used at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextRole {
    /// The role's name, e.g. `ui` or `mono`.
    pub name: String,
    /// Allowed families.
    pub families: Vec<String>,
    /// Allowed sizes in logical pixels; empty allows any.
    pub sizes: Vec<f32>,
    /// Allowed weights (400 regular, 600 semibold, 700 bold); empty allows any.
    pub weights: Vec<f32>,
}

impl Default for TextRole {
    fn default() -> Self {
        Self {
            name: "text".into(),
            families: Vec::new(),
            sizes: Vec::new(),
            weights: Vec::new(),
        }
    }
}

impl TextRole {
    /// A role for `families` at any size and weight.
    pub fn new(name: &str, families: &[&str]) -> Self {
        Self {
            name: name.into(),
            families: families.iter().map(|family| family.to_string()).collect(),
            ..Self::default()
        }
    }

    /// Restricts the role to these sizes.
    pub fn sizes(mut self, sizes: &[f32]) -> Self {
        self.sizes = sizes.to_vec();
        self
    }

    /// Restricts the role to these weights.
    pub fn weights(mut self, weights: &[f32]) -> Self {
        self.weights = weights.to_vec();
        self
    }
}

/// Spacing must follow a grid: every painted box (and line of text) sits a
/// grid multiple from an edge of its container or from a neighbor, or is
/// centered. See [`rules::spacing`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpacingRule {
    /// The grid step in logical pixels, e.g. 4.
    pub grid: f32,
    /// Sizes that may break the grid along their axis (e.g. 22 px rows):
    /// a box this tall (or wide) isn't checked vertically (horizontally).
    pub exempt_sizes: Vec<f32>,
    /// Whether lines of text are checked too (padding around text).
    pub include_text: bool,
}

impl Default for SpacingRule {
    fn default() -> Self {
        Self {
            grid: 4.,
            exempt_sizes: Vec::new(),
            include_text: true,
        }
    }
}

/// A declarative, serializable description of a visual style. Every rule is
/// optional; the default spec checks only what is always a bug (clipped or
/// overlapping text, content outside the window).
///
/// ```ignore
/// let spec = StyleSpec {
///     name: "cards".into(),
///     text_roles: vec![TextRole::new("ui", &["IBM Plex Sans"]).sizes(&[12., 14.])],
///     text_colors: vec![Color::from_u32(0x1c1f24), Color::from_u32(0x5d636e)],
///     spacing: Some(SpacingRule::default()),
///     corner_radii: Some(vec![0., 4., 8.]),
///     min_contrast: Some(4.5),
///     ..StyleSpec::default()
/// };
/// // or: StyleSpec::from_json(include_str!("style.json"))?
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StyleSpec {
    /// Shown in reports.
    pub name: String,
    /// Allowed text styles; every visible line must match one. Empty: unchecked.
    pub text_roles: Vec<TextRole>,
    /// Allowed text colors. Empty: unchecked.
    pub text_colors: Vec<Color>,
    /// How far (Oklab ΔE) a text color may be from a palette color.
    pub color_tolerance: f32,
    /// The spacing grid for quad edges.
    pub spacing: Option<SpacingRule>,
    /// Allowed corner radii in logical pixels.
    pub corner_radii: Option<Vec<f32>>,
    /// Whether fully rounded ends (radius ≥ half the short side) are allowed.
    pub allow_pills: bool,
    /// Allowed border widths in logical pixels.
    pub border_widths: Option<Vec<f32>>,
    /// Minimum WCAG contrast of text against the pixels behind it.
    pub min_contrast: Option<f32>,
    /// Minimum contrast for large text (≥ 24 px, or ≥ 18.66 px bold).
    pub min_contrast_large: Option<f32>,
    /// Flag text cut off by its clip (ellipsized text isn't cut off).
    pub no_clipped_text: bool,
    /// Allow text clipped only vertically (rows scrolled partly out of view).
    pub allow_vertical_clip: bool,
    /// Flag text lines that overlap each other.
    pub no_overlapping_text: bool,
    /// Flag side-by-side lines whose baselines differ by more than this.
    pub baseline_tolerance: Option<f32>,
    /// Minimum width and height of clickable elements (needs the element
    /// tree: see [`Stage::record_elements`](crate::Stage::record_elements)).
    pub min_hit_target: Option<f32>,
    /// Flag text and quads that extend outside the window.
    pub no_content_outside_window: bool,
    /// Text lines (exact text) exempt from every rule.
    pub ignore_text: Vec<String>,
    /// Regions (logical pixels) whose content is exempt from every rule.
    pub ignore_regions: Vec<Rect>,
}

impl Default for StyleSpec {
    fn default() -> Self {
        Self {
            name: "default".into(),
            text_roles: Vec::new(),
            text_colors: Vec::new(),
            color_tolerance: 0.02,
            spacing: None,
            corner_radii: None,
            allow_pills: true,
            border_widths: None,
            min_contrast: None,
            min_contrast_large: None,
            no_clipped_text: true,
            allow_vertical_clip: true,
            no_overlapping_text: true,
            baseline_tolerance: None,
            min_hit_target: None,
            no_content_outside_window: true,
            ignore_text: Vec::new(),
            ignore_regions: Vec::new(),
        }
    }
}

impl StyleSpec {
    /// Loupe's visual language (`crates/gpui_inspector/DESIGN.md`): IBM Plex
    /// Sans at 12 / 11 / 10.5 px for UI and Lilex for values, the Loupe
    /// palette (dark and light), a 4 px grid (22 px rows exempt), radii of
    /// 0 / 4 / 8 px, hairline borders, WCAG AA contrast, 20 px hit targets.
    pub fn loupe() -> Self {
        let palette = |palette: crate::theme::Palette| {
            [
                palette.text,
                palette.text_muted,
                palette.text_faint,
                palette.accent,
                palette.ok,
                palette.warn,
                palette.crit,
            ]
        };
        Self {
            name: "Loupe design".into(),
            text_roles: vec![
                TextRole::new("ui", &["IBM Plex Sans"])
                    .sizes(&[12., 11., 10.5])
                    .weights(&[400., 600.]),
                TextRole::new("mono", &["Lilex"])
                    .sizes(&[12., 11., 10.5])
                    .weights(&[400., 700.]),
            ],
            text_colors: palette(DARK)
                .into_iter()
                .chain(palette(LIGHT))
                .chain([Color::WHITE])
                .collect(),
            spacing: Some(SpacingRule {
                exempt_sizes: vec![22.],
                ..SpacingRule::default()
            }),
            corner_radii: Some(vec![0., 4., 8.]),
            border_widths: Some(vec![0., 1.]),
            min_contrast: Some(4.5),
            min_contrast_large: Some(3.),
            baseline_tolerance: Some(1.),
            min_hit_target: Some(20.),
            ..Self::default()
        }
    }

    /// Parses a spec from JSON; omitted fields take their defaults.
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }

    /// The spec as pretty-printed JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// What a violation breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rule {
    /// A font family no text role allows.
    FontFamily,
    /// A size the matching role doesn't allow.
    FontSize,
    /// A weight the matching role doesn't allow.
    FontWeight,
    /// A text color outside the palette.
    TextColor,
    /// A quad edge off the spacing grid.
    Spacing,
    /// A corner radius that isn't allowed.
    CornerRadius,
    /// A border width that isn't allowed.
    BorderWidth,
    /// Text contrast below the minimum.
    Contrast,
    /// Text cut off by its clip.
    ClippedText,
    /// Two text lines overlapping.
    TextOverlap,
    /// Side-by-side text with misaligned baselines.
    Baseline,
    /// A clickable element smaller than the minimum hit target.
    HitTarget,
    /// Content extending outside the window.
    OutsideWindow,
}

impl Rule {
    /// The rule's kebab-case name.
    pub fn name(self) -> &'static str {
        match self {
            Rule::FontFamily => "font-family",
            Rule::FontSize => "font-size",
            Rule::FontWeight => "font-weight",
            Rule::TextColor => "text-color",
            Rule::Spacing => "spacing",
            Rule::CornerRadius => "corner-radius",
            Rule::BorderWidth => "border-width",
            Rule::Contrast => "contrast",
            Rule::ClippedText => "clipped-text",
            Rule::TextOverlap => "text-overlap",
            Rule::Baseline => "baseline",
            Rule::HitTarget => "hit-target",
            Rule::OutsideWindow => "outside-window",
        }
    }

    /// How serious a violation of the rule is.
    pub fn severity(self) -> Severity {
        match self {
            Rule::Contrast
            | Rule::ClippedText
            | Rule::TextOverlap
            | Rule::OutsideWindow
            | Rule::FontFamily => Severity::Error,
            Rule::FontSize
            | Rule::FontWeight
            | Rule::TextColor
            | Rule::Spacing
            | Rule::CornerRadius
            | Rule::BorderWidth
            | Rule::Baseline
            | Rule::HitTarget => Severity::Warning,
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How serious a violation is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Off-spec: worth fixing.
    Warning,
    /// Broken for users: unreadable, cut off, overlapping.
    Error,
}

/// One broken rule, where it happened, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Violation {
    /// The rule broken.
    pub rule: Rule,
    /// How serious it is.
    pub severity: Severity,
    /// What was checked: the text (quoted) or the quad's size.
    pub subject: String,
    /// A plain-language explanation with the measured and allowed values.
    pub message: String,
    /// Where, in logical pixels.
    pub bounds: Rect,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {}: {} ({})",
            self.rule, self.subject, self.message, self.bounds
        )
    }
}

/// How much a lint looked at.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LintCounts {
    /// Visible text lines checked.
    pub texts: usize,
    /// Painted quads checked.
    pub quads: usize,
    /// Clickable elements checked, if the element tree was recorded.
    pub hit_targets: Option<usize>,
    /// Rules that ran.
    pub rules: Vec<Rule>,
    /// Rules that were asked for but couldn't run, and why.
    pub skipped: Vec<String>,
}

/// The result of [`Shot::lint`]: violations, what was checked, and where the
/// annotated PNG is.
#[derive(Clone, Debug)]
pub struct LintReport {
    /// Violations, in the order they are numbered in the annotation.
    pub violations: Vec<Violation>,
    /// How much was checked.
    pub checked: LintCounts,
    /// The annotated PNG: the shot with numbered boxes, and a legend.
    pub annotated: PathBuf,
    record: LintRecord,
    suite: Suite,
}

impl LintReport {
    /// Whether nothing was flagged.
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    /// The violations of one rule.
    pub fn of(&self, rule: Rule) -> Vec<&Violation> {
        self.violations
            .iter()
            .filter(|violation| violation.rule == rule)
            .collect()
    }

    /// Asserts there are no violations.
    #[track_caller]
    pub fn assert_clean(&self) -> &Self {
        self.assert_at_most(0)
    }

    /// Asserts there are at most `limit` violations.
    #[track_caller]
    pub fn assert_at_most(&self, limit: usize) -> &Self {
        let passed = self.violations.len() <= limit;
        let name = if limit == 0 {
            "clean".to_string()
        } else {
            format!("at_most({limit})")
        };
        self.verdict(
            name,
            passed,
            format!("{} violations", self.violations.len()),
        );
        assert!(
            passed,
            "lightbox: {} has {} style violation{} (at most {limit} allowed):\n{self}",
            self.record.name,
            self.violations.len(),
            if self.violations.len() == 1 { "" } else { "s" },
        );
        self
    }

    /// Asserts the lint caught at least one violation of `rule`: for tests
    /// proving a deliberately broken view is flagged.
    #[track_caller]
    pub fn assert_caught(&self, rule: Rule) -> &Self {
        let caught = self.of(rule).len();
        assert!(
            caught > 0,
            "lightbox: expected {} to break {rule}, but it didn't:\n{self}",
            self.record.name
        );
        self
    }

    fn verdict(&self, name: String, passed: bool, message: String) {
        let mut record = self.record.clone();
        record.verdict = Some(Assertion {
            name,
            passed,
            message,
        });
        if let Err(error) = self.suite.write_record(&Record::Lint(record)) {
            log::error!("lightbox: saving lint verdict: {error:#}");
        }
    }
}

impl fmt::Display for LintReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (ix, violation) in self.violations.iter().enumerate() {
            writeln!(f, "  {:>2}. {violation}", ix + 1)?;
        }
        write!(f, "  annotated: {}", self.annotated.display())
    }
}

impl Shot {
    /// Checks the shot against `spec`, writes `<name>.lint.png` (the shot
    /// with each violation boxed and numbered, and a legend) and returns the
    /// report.
    ///
    /// ```ignore
    /// stage.shot("settings").lint(&StyleSpec::loupe()).assert_clean();
    /// ```
    #[track_caller]
    pub fn lint(&self, spec: &StyleSpec) -> LintReport {
        let location = Location::caller();
        let (violations, checked) = rules::check(spec, &rules::LintInput::from_shot(self));
        let annotated_file = format!("{}.lint.png", slug(&self.name));
        let annotated = self
            .compositor
            .with(|composer| annotate::render(composer, self, spec, &violations, &checked))
            .and_then(|image| self.suite.write_png(&annotated_file, &image))
            .unwrap_or_else(|error| panic!("lightbox: annotating {:?}: {error:#}", self.name));
        let shot_image = self.file_name();
        if !self.suite.path(&shot_image).exists()
            && let Err(error) = self.suite.write_png(&shot_image, &self.image)
        {
            panic!("lightbox: saving {:?}: {error:#}", self.name);
        }
        let record = LintRecord {
            name: self.name.clone(),
            spec: spec.name.clone(),
            shot_image,
            size: self.meta.size,
            scale: self.meta.scale,
            annotated: annotated_file,
            violations: violations.clone(),
            checked: checked.clone(),
            verdict: None,
            origin: Origin::at(location),
        };
        if let Err(error) = self.suite.write_record(&Record::Lint(record.clone())) {
            panic!("lightbox: saving lint of {:?}: {error:#}", self.name);
        }
        LintReport {
            violations,
            checked,
            annotated,
            record,
            suite: self.suite.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_round_trip_through_json() {
        let spec = StyleSpec::loupe();
        let json = spec.to_json();
        assert!(json.contains("\"#dcdee4\""), "colors serialize as hex");
        assert_eq!(StyleSpec::from_json(&json).unwrap(), spec);

        let partial = StyleSpec::from_json(r#"{ "name": "tiny", "min_contrast": 3 }"#).unwrap();
        assert_eq!(partial.min_contrast, Some(3.));
        assert!(partial.no_clipped_text, "omitted fields take defaults");
        assert!(
            StyleSpec::from_json(r#"{ "min_contrst": 3 }"#).is_err(),
            "typos are errors"
        );
    }
}
