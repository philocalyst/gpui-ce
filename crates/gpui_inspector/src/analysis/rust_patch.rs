//! "Copy Rust": a style as idiomatic `Styled` builder calls.
//!
//! Shorthand helpers are emitted only when they reproduce the value exactly
//! (`.p_2()` is `rems(0.5)`, so 8 px padding becomes `.p(px(8.))`), using the
//! same constants gpui's style macros expand to. Every mapped field is taken
//! out of a working copy of the style, and whatever is left is listed as
//! `// unmapped: field = value`, so nothing is silently dropped.

use super::style_grid;
use gpui::{
    AbsoluteLength, AlignContent, AlignItems, BorderStyle, CursorStyle, DefiniteLength, Display,
    Fill, Filter, FlexDirection, FlexWrap, FontStyle, FontWeight, GridTemplate,
    GridTemplateMinSize, Hsla, Length, Overflow, Pixels, Position, Refineable as _, RingColor,
    SizeRefinement, StrikethroughStyle, StyleRefinement, Styled, TextAlign, TextOverflow,
    UnderlineStyle, Visibility, WhiteSpace, hsla_to_rgba, px, relative, rems,
};

/// The spacing and size scale shared by padding, margin, gap, inset and
/// size helpers (`_0` … `_128`, `_px`, `_full`, fractions).
const SPACING_SCALE: &[(&str, DefiniteLength)] = &[
    ("0", absolute_px(0.)),
    ("0p5", absolute_rems(0.125)),
    ("1", absolute_rems(0.25)),
    ("1p5", absolute_rems(0.375)),
    ("2", absolute_rems(0.5)),
    ("2p5", absolute_rems(0.625)),
    ("3", absolute_rems(0.75)),
    ("3p5", absolute_rems(0.875)),
    ("4", absolute_rems(1.)),
    ("5", absolute_rems(1.25)),
    ("6", absolute_rems(1.5)),
    ("7", absolute_rems(1.75)),
    ("8", absolute_rems(2.0)),
    ("9", absolute_rems(2.25)),
    ("10", absolute_rems(2.5)),
    ("11", absolute_rems(2.75)),
    ("12", absolute_rems(3.)),
    ("16", absolute_rems(4.)),
    ("20", absolute_rems(5.)),
    ("24", absolute_rems(6.)),
    ("32", absolute_rems(8.)),
    ("40", absolute_rems(10.)),
    ("48", absolute_rems(12.)),
    ("56", absolute_rems(14.)),
    ("64", absolute_rems(16.)),
    ("72", absolute_rems(18.)),
    ("80", absolute_rems(20.)),
    ("96", absolute_rems(24.)),
    ("112", absolute_rems(28.)),
    ("128", absolute_rems(32.)),
    ("px", absolute_px(1.)),
    ("full", fraction(1.)),
    ("1_2", fraction(0.5)),
    ("1_3", fraction(1. / 3.)),
    ("2_3", fraction(2. / 3.)),
    ("1_4", fraction(0.25)),
    ("2_4", fraction(0.5)),
    ("3_4", fraction(0.75)),
    ("1_5", fraction(0.2)),
    ("2_5", fraction(0.4)),
    ("3_5", fraction(0.6)),
    ("4_5", fraction(0.8)),
    ("1_6", fraction(1. / 6.)),
    ("5_6", fraction(5. / 6.)),
    ("1_12", fraction(1. / 12.)),
];

/// Border width helpers (`border_1`, `border_t_2`...), in pixels.
const BORDER_SCALE: [(&str, f32); 17] = [
    ("0", 0.),
    ("1", 1.),
    ("2", 2.),
    ("3", 3.),
    ("4", 4.),
    ("5", 5.),
    ("6", 6.),
    ("7", 7.),
    ("8", 8.),
    ("9", 9.),
    ("10", 10.),
    ("11", 11.),
    ("12", 12.),
    ("16", 16.),
    ("20", 20.),
    ("24", 24.),
    ("32", 32.),
];

/// Corner radius helpers (`rounded_md`, `rounded_t_lg`...).
const CORNER_SCALE: [(&str, AbsoluteLength); 9] = [
    ("none", AbsoluteLength::Pixels(px(0.))),
    ("xs", AbsoluteLength::Rems(rems(0.125))),
    ("sm", AbsoluteLength::Rems(rems(0.25))),
    ("md", AbsoluteLength::Rems(rems(0.375))),
    ("lg", AbsoluteLength::Rems(rems(0.5))),
    ("xl", AbsoluteLength::Rems(rems(0.75))),
    ("2xl", AbsoluteLength::Rems(rems(1.))),
    ("3xl", AbsoluteLength::Rems(rems(1.5))),
    ("full", AbsoluteLength::Pixels(px(9999.))),
];

/// Text size helpers (`text_sm`...).
const TEXT_SIZES: [(&str, f32); 7] = [
    ("xs", 0.75),
    ("sm", 0.875),
    ("base", 1.0),
    ("lg", 1.125),
    ("xl", 1.25),
    ("2xl", 1.5),
    ("3xl", 1.875),
];

/// Ring width helpers (`ring_2`...), in pixels.
const RING_SCALE: [(&str, f32); 5] = [("0", 0.), ("1", 1.), ("2", 2.), ("4", 4.), ("8", 8.)];

/// Corner smoothing helpers (`rounded_smoothing_0p5`...).
const SMOOTHING_SCALE: [(&str, f32); 11] = [
    ("0", 0.0),
    ("0p1", 0.1),
    ("0p2", 0.2),
    ("0p3", 0.3),
    ("0p4", 0.4),
    ("0p5", 0.5),
    ("0p6", 0.6),
    ("0p7", 0.7),
    ("0p8", 0.8),
    ("0p9", 0.9),
    ("1", 1.0),
];

/// Box shadow presets, compared against what the helpers actually produce.
const SHADOW_PRESETS: [(&str, fn(StyleRefinement) -> StyleRefinement); 7] = [
    ("shadow_2xs", StyleRefinement::shadow_2xs),
    ("shadow_xs", StyleRefinement::shadow_xs),
    ("shadow_sm", StyleRefinement::shadow_sm),
    ("shadow_md", StyleRefinement::shadow_md),
    ("shadow_lg", StyleRefinement::shadow_lg),
    ("shadow_xl", StyleRefinement::shadow_xl),
    ("shadow_2xl", StyleRefinement::shadow_2xl),
];

/// Cursor helpers for every cursor style.
const CURSORS: [(CursorStyle, &str); 21] = [
    (CursorStyle::Arrow, "cursor_default"),
    (CursorStyle::IBeam, "cursor_text"),
    (CursorStyle::Crosshair, "cursor_crosshair"),
    (CursorStyle::ClosedHand, "cursor_grabbing"),
    (CursorStyle::OpenHand, "cursor_grab"),
    (CursorStyle::PointingHand, "cursor_pointer"),
    (CursorStyle::ResizeLeft, "cursor_w_resize"),
    (CursorStyle::ResizeRight, "cursor_e_resize"),
    (CursorStyle::ResizeLeftRight, "cursor_ew_resize"),
    (CursorStyle::ResizeUp, "cursor_n_resize"),
    (CursorStyle::ResizeDown, "cursor_s_resize"),
    (CursorStyle::ResizeUpDown, "cursor_ns_resize"),
    (CursorStyle::ResizeUpLeftDownRight, "cursor_nwse_resize"),
    (CursorStyle::ResizeUpRightDownLeft, "cursor_nesw_resize"),
    (CursorStyle::ResizeColumn, "cursor_col_resize"),
    (CursorStyle::ResizeRow, "cursor_row_resize"),
    (
        CursorStyle::IBeamCursorForVerticalLayout,
        "cursor_vertical_text",
    ),
    (CursorStyle::OperationNotAllowed, "cursor_not_allowed"),
    (CursorStyle::DragLink, "cursor_alias"),
    (CursorStyle::DragCopy, "cursor_copy"),
    (CursorStyle::ContextualMenu, "cursor_context_menu"),
];

/// Named font weights.
const FONT_WEIGHTS: [(FontWeight, &str); 9] = [
    (FontWeight::THIN, "THIN"),
    (FontWeight::EXTRA_LIGHT, "EXTRA_LIGHT"),
    (FontWeight::LIGHT, "LIGHT"),
    (FontWeight::NORMAL, "NORMAL"),
    (FontWeight::MEDIUM, "MEDIUM"),
    (FontWeight::SEMIBOLD, "SEMIBOLD"),
    (FontWeight::BOLD, "BOLD"),
    (FontWeight::EXTRA_BOLD, "EXTRA_BOLD"),
    (FontWeight::BLACK, "BLACK"),
];

/// The ellipsis `text_ellipsis()` and `truncate()` use.
const ELLIPSIS: &str = "…";
/// A color channel within this of an 8-bit step prints as hex.
const HEX_TOLERANCE: f32 = 0.01;
/// Decimals printed for `hsla(...)` components.
const HSLA_DECIMALS: usize = 3;

const fn absolute_px(pixels: f32) -> DefiniteLength {
    DefiniteLength::Absolute(AbsoluteLength::Pixels(px(pixels)))
}

const fn absolute_rems(value: f32) -> DefiniteLength {
    DefiniteLength::Absolute(AbsoluteLength::Rems(rems(value)))
}

const fn fraction(value: f32) -> DefiniteLength {
    DefiniteLength::Relative(relative(value))
}

/// Builder calls for a style, plus what could not be expressed as calls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RustPatch {
    /// One call per entry, e.g. `.p_2()`, `.w(px(120.))`.
    pub calls: Vec<String>,
    /// `field = value` for each property no builder call reproduces.
    pub unmapped: Vec<String>,
}

impl RustPatch {
    /// Whether there is nothing to write.
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty() && self.unmapped.is_empty()
    }

    /// The lines of code: one call per line, then a `// unmapped:` comment per
    /// unmapped property.
    pub fn lines(&self) -> Vec<String> {
        self.calls
            .iter()
            .cloned()
            .chain(
                self.unmapped
                    .iter()
                    .map(|line| format!("// unmapped: {line}")),
            )
            .collect()
    }

    /// [`Self::lines`] joined, ready to paste after `div()`.
    pub fn to_code(&self) -> String {
        self.lines().join("\n")
    }
}

/// Every set property of `style` as builder calls, as if chained on a fresh
/// element.
pub fn builder_calls(style: &StyleRefinement) -> RustPatch {
    Emitter::new(style.clone(), style, None).run()
}

/// The calls to append to an element's existing chain (which produces
/// `base`) so it looks like `base` with `overrides` applied. Only properties
/// whose value changes are included.
pub fn rust_patch(base: &StyleRefinement, overrides: &StyleRefinement) -> RustPatch {
    let merged = style_grid::merge(base, overrides);
    let changed = merged.subtract(base);
    Emitter::new(changed, &merged, Some(base)).run()
}

/// Whether a line of a [`patch_diff`] is kept, removed or added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    /// In both.
    Kept,
    /// Only in the base.
    Removed,
    /// Only with the overrides applied.
    Added,
}

/// One line of a [`patch_diff`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// Kept, removed or added.
    pub kind: DiffKind,
    /// The line of code.
    pub text: String,
}

/// The base's builder calls against the calls with `overrides` applied, as
/// a line diff: `- .p_2()` / `+ .p_3()`.
pub fn patch_diff(base: &StyleRefinement, overrides: &StyleRefinement) -> Vec<DiffLine> {
    let before = builder_calls(base).lines();
    let after = builder_calls(&style_grid::merge(base, overrides)).lines();
    line_diff(&before, &after)
}

/// The changed lines of a diff, prefixed `- ` and `+ `, for "Copy patch".
pub fn diff_text(lines: &[DiffLine]) -> String {
    lines
        .iter()
        .filter_map(|line| match line.kind {
            DiffKind::Kept => None,
            DiffKind::Removed => Some(format!("- {}", line.text)),
            DiffKind::Added => Some(format!("+ {}", line.text)),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A longest-common-subsequence line diff; removals come before additions
/// at each change.
fn line_diff(before: &[String], after: &[String]) -> Vec<DiffLine> {
    let (rows, columns) = (before.len(), after.len());
    let mut common = vec![vec![0u32; columns + 1]; rows + 1];
    for row in (0..rows).rev() {
        for column in (0..columns).rev() {
            common[row][column] = if before[row] == after[column] {
                common[row + 1][column + 1] + 1
            } else {
                common[row + 1][column].max(common[row][column + 1])
            };
        }
    }
    let mut lines = Vec::with_capacity(rows.max(columns));
    let (mut row, mut column) = (0, 0);
    while row < rows || column < columns {
        let line = if row < rows && column < columns && before[row] == after[column] {
            row += 1;
            column += 1;
            (DiffKind::Kept, &before[row - 1])
        } else if row < rows
            && (column == columns || common[row + 1][column] >= common[row][column + 1])
        {
            row += 1;
            (DiffKind::Removed, &before[row - 1])
        } else {
            column += 1;
            (DiffKind::Added, &after[column - 1])
        };
        lines.push(DiffLine {
            kind: line.0,
            text: line.1.clone(),
        });
    }
    lines
}

/// Names of the helpers for a four-sided property.
struct EdgeNames {
    all: &'static str,
    horizontal: Option<&'static str>,
    vertical: Option<&'static str>,
    /// Top, right, bottom, left.
    sides: [&'static str; 4],
}

const PADDING: EdgeNames = EdgeNames {
    all: "p",
    horizontal: Some("px"),
    vertical: Some("py"),
    sides: ["pt", "pr", "pb", "pl"],
};
const MARGIN: EdgeNames = EdgeNames {
    all: "m",
    horizontal: Some("mx"),
    vertical: Some("my"),
    sides: ["mt", "mr", "mb", "ml"],
};
const INSET: EdgeNames = EdgeNames {
    all: "inset",
    horizontal: None,
    vertical: None,
    sides: ["top", "right", "bottom", "left"],
};
const BORDER: EdgeNames = EdgeNames {
    all: "border",
    horizontal: Some("border_x"),
    vertical: Some("border_y"),
    sides: ["border_t", "border_r", "border_b", "border_l"],
};

/// Turns a style into calls, taking each mapped field out of `rest`.
struct Emitter<'a> {
    rest: StyleRefinement,
    /// The style the element ends up with, for helpers that set several fields.
    effective: &'a StyleRefinement,
    /// The chain the calls are appended to, if any.
    prior: Option<&'a StyleRefinement>,
    calls: Vec<String>,
}

impl<'a> Emitter<'a> {
    fn new(
        rest: StyleRefinement,
        effective: &'a StyleRefinement,
        prior: Option<&'a StyleRefinement>,
    ) -> Self {
        Self {
            rest,
            effective,
            prior,
            calls: Vec::new(),
        }
    }

    fn run(mut self) -> RustPatch {
        self.layout();
        self.flex();
        self.size();
        self.position();
        self.spacing();
        self.visual();
        self.text();
        let unmapped = style_grid::rows(&self.rest)
            .into_iter()
            .map(|row| format!("{} = {}", row.label, row.value))
            .collect();
        RustPatch {
            calls: self.calls,
            unmapped,
        }
    }

    fn call(&mut self, call: impl Into<String>) {
        self.calls.push(format!(".{}", call.into()));
    }

    fn layout(&mut self) {
        if let Some(display) = self.rest.display.take() {
            self.call(match display {
                Display::Block => "block()",
                Display::Flex => "flex()",
                Display::Grid => "grid()",
                Display::None => "hidden()",
            });
        }
        if let Some(visibility) = self.rest.visibility.take() {
            self.call(match visibility {
                Visibility::Visible => "visible()",
                Visibility::Hidden => "invisible()",
            });
        }
        self.truncate_and_clamp();
        self.overflow();
        for (template, name) in [
            (self.rest.grid_cols.take(), "grid_cols"),
            (self.rest.grid_rows.take(), "grid_rows"),
        ] {
            if let Some(GridTemplate { repeat, min_size }) = template {
                let suffix = match min_size {
                    GridTemplateMinSize::Zero => "",
                    GridTemplateMinSize::MinContent => "_min_content",
                    GridTemplateMinSize::MaxContent => "_max_content",
                };
                self.call(format!("{name}{suffix}({repeat})"));
            }
        }
        if let Some(width) = self.rest.scrollbar_width.take() {
            self.call(format!("scrollbar_width({})", absolute_expr(width)));
        }
    }

    /// `.truncate()` (overflow hidden + nowrap + ellipsis) and
    /// `.line_clamp(n)` (which also hides overflow).
    fn truncate_and_clamp(&mut self) {
        let rest = &self.rest;
        let hidden = Some(Overflow::Hidden);
        let is_truncate = rest.overflow.x == hidden
            && rest.overflow.y == hidden
            && rest.text.white_space == Some(WhiteSpace::Nowrap)
            && rest.text.text_overflow == Some(TextOverflow::Truncate(ELLIPSIS.into()));
        if is_truncate {
            self.rest.overflow.x = None;
            self.rest.overflow.y = None;
            self.rest.text.white_space = None;
            self.rest.text.text_overflow = None;
            self.call("truncate()");
        }
        let effective = &self.effective.overflow;
        if effective.x == hidden
            && effective.y == hidden
            && let Some(lines) = self.rest.text.line_clamp.take()
        {
            self.rest.overflow.x = None;
            self.rest.overflow.y = None;
            self.call(format!("line_clamp({lines})"));
        }
    }

    fn overflow(&mut self) {
        let hidden = Some(Overflow::Hidden);
        match (
            self.rest.overflow.x == hidden,
            self.rest.overflow.y == hidden,
        ) {
            (true, true) => {
                self.rest.overflow.x = None;
                self.rest.overflow.y = None;
                self.call("overflow_hidden()");
            }
            (true, false) => {
                self.rest.overflow.x = None;
                self.call("overflow_x_hidden()");
            }
            (false, true) => {
                self.rest.overflow.y = None;
                self.call("overflow_y_hidden()");
            }
            (false, false) => {}
        }
    }

    fn flex(&mut self) {
        if let Some(direction) = self.rest.flex_direction.take() {
            self.call(match direction {
                FlexDirection::Row => "flex_row()",
                FlexDirection::Column => "flex_col()",
                FlexDirection::RowReverse => "flex_row_reverse()",
                FlexDirection::ColumnReverse => "flex_col_reverse()",
            });
        }
        if let Some(wrap) = self.rest.flex_wrap.take() {
            self.call(match wrap {
                FlexWrap::Wrap => "flex_wrap()",
                FlexWrap::WrapReverse => "flex_wrap_reverse()",
                FlexWrap::NoWrap => "flex_nowrap()",
            });
        }
        self.flex_factors();
        self.alignment();
        let (width, height) = (self.rest.gap.width.take(), self.rest.gap.height.take());
        self.pair(("gap", "gap_x", "gap_y"), width, height, definite_call);
    }

    /// `flex_1` / `flex_auto` / `flex_initial` / `flex_none`, or the factors
    /// one by one.
    fn flex_factors(&mut self) {
        let rest = &mut self.rest;
        let presets = [
            ("flex_1()", 1., 1., Length::from(relative(0.))),
            ("flex_auto()", 1., 1., Length::Auto),
            ("flex_initial()", 0., 1., Length::Auto),
            ("flex_none()", 0., 0., Length::Auto),
        ];
        let current = (rest.flex_grow, rest.flex_shrink, rest.flex_basis);
        if let Some((call, ..)) = presets
            .iter()
            .find(|(_, grow, shrink, basis)| current == (Some(*grow), Some(*shrink), Some(*basis)))
        {
            rest.flex_grow = None;
            rest.flex_shrink = None;
            rest.flex_basis = None;
            self.call(*call);
            return;
        }
        for (factor, name) in [
            (self.rest.flex_grow.take(), "flex_grow"),
            (self.rest.flex_shrink.take(), "flex_shrink"),
        ] {
            match factor {
                Some(0.) => self.call(format!("{name}_0()")),
                Some(1.) => self.call(format!("{name}_1()")),
                Some(value) => self.call(format!("{name}({})", float(value))),
                None => {}
            }
        }
        if let Some(basis) = self.rest.flex_basis.take() {
            self.call(format!("flex_basis({})", length_expr(basis)));
        }
    }

    fn alignment(&mut self) {
        let rest = &mut self.rest;
        let items = rest.align_items.and_then(|align| match align {
            AlignItems::FlexStart => Some("items_start()"),
            AlignItems::FlexEnd => Some("items_end()"),
            AlignItems::Center => Some("items_center()"),
            AlignItems::Baseline => Some("items_baseline()"),
            AlignItems::Stretch => Some("items_stretch()"),
            AlignItems::Start | AlignItems::End => None,
        });
        let this = rest.align_self.map(|align| match align {
            AlignItems::Start => "self_start()",
            AlignItems::End => "self_end()",
            AlignItems::FlexStart => "self_flex_start()",
            AlignItems::FlexEnd => "self_flex_end()",
            AlignItems::Center => "self_center()",
            AlignItems::Baseline => "self_baseline()",
            AlignItems::Stretch => "self_stretch()",
        });
        let content = rest.align_content.and_then(|align| match align {
            AlignContent::Center => Some("content_center()"),
            AlignContent::FlexStart => Some("content_start()"),
            AlignContent::FlexEnd => Some("content_end()"),
            AlignContent::SpaceBetween => Some("content_between()"),
            AlignContent::SpaceAround => Some("content_around()"),
            AlignContent::SpaceEvenly => Some("content_evenly()"),
            AlignContent::Stretch => Some("content_stretch()"),
            AlignContent::Start | AlignContent::End => None,
        });
        let justify = rest.justify_content.and_then(|justify| match justify {
            AlignContent::Start => Some("justify_start()"),
            AlignContent::End => Some("justify_end()"),
            AlignContent::Center => Some("justify_center()"),
            AlignContent::SpaceBetween => Some("justify_between()"),
            AlignContent::SpaceAround => Some("justify_around()"),
            AlignContent::SpaceEvenly => Some("justify_evenly()"),
            AlignContent::FlexStart | AlignContent::FlexEnd | AlignContent::Stretch => None,
        });
        if items.is_some() {
            rest.align_items = None;
        }
        if this.is_some() {
            rest.align_self = None;
        }
        if content.is_some() {
            rest.align_content = None;
        }
        if justify.is_some() {
            rest.justify_content = None;
        }
        for call in [items, this, content, justify].into_iter().flatten() {
            self.call(call);
        }
    }

    fn size(&mut self) {
        let sizes = [
            (("size", "w", "h"), take_size(&mut self.rest.size)),
            (
                ("min_size", "min_w", "min_h"),
                take_size(&mut self.rest.min_size),
            ),
            (
                ("max_size", "max_w", "max_h"),
                take_size(&mut self.rest.max_size),
            ),
        ];
        for (names, (width, height)) in sizes {
            self.pair(names, width, height, |prefix, length| {
                length_call(prefix, length, false)
            });
        }
        if let Some(ratio) = self.rest.aspect_ratio.take() {
            if ratio == 1.0 {
                self.call("aspect_square()");
            } else {
                self.call(format!("aspect_ratio({})", float(ratio)));
            }
        }
    }

    fn position(&mut self) {
        if let Some(position) = self.rest.position.take() {
            self.call(match position {
                Position::Relative => "relative()",
                Position::Absolute => "absolute()",
            });
        }
        let inset = &mut self.rest.inset;
        let sides = [
            inset.top.take(),
            inset.right.take(),
            inset.bottom.take(),
            inset.left.take(),
        ];
        self.edges(&INSET, sides, |prefix, length| {
            length_call(prefix, length, true)
        });
    }

    fn spacing(&mut self) {
        let margin = &mut self.rest.margin;
        let sides = [
            margin.top.take(),
            margin.right.take(),
            margin.bottom.take(),
            margin.left.take(),
        ];
        self.edges(&MARGIN, sides, |prefix, length| {
            length_call(prefix, length, true)
        });
        let padding = &mut self.rest.padding;
        let sides = [
            padding.top.take(),
            padding.right.take(),
            padding.bottom.take(),
            padding.left.take(),
        ];
        self.edges(&PADDING, sides, definite_call);
    }

    fn visual(&mut self) {
        let solid = |fill: &Option<Fill>| fill.as_ref()?.color()?.as_solid();
        if let Some(color) = solid(&self.rest.background) {
            self.rest.background = None;
            self.call(format!("bg({})", color_expr(color)));
        }
        let border = &mut self.rest.border_widths;
        let sides = [
            border.top.take(),
            border.right.take(),
            border.bottom.take(),
            border.left.take(),
        ];
        self.edges(&BORDER, sides, border_call);
        if let Some(color) = self.rest.border_color.and_then(|color| color.as_solid()) {
            self.rest.border_color = None;
            self.call(format!("border_color({})", color_expr(color)));
        }
        if self.rest.border_style == Some(BorderStyle::Dashed) {
            self.rest.border_style = None;
            self.call("border_dashed()");
        }
        for (value, name) in [
            (&mut self.rest.border_dashed_length, "border_dashed_length"),
            (&mut self.rest.border_dashed_gap, "border_dashed_gap"),
        ] {
            if let Some(amount) = value.take_if(|amount| *amount >= 0.0) {
                self.calls.push(format!(".{name}({})", float(amount)));
            }
        }
        self.corners();
        if let Some(amount) = self
            .rest
            .corner_smoothing
            .take_if(|amount| (0.0..=1.0).contains(amount))
        {
            match SMOOTHING_SCALE.iter().find(|(_, value)| *value == amount) {
                Some((suffix, _)) => self.call(format!("rounded_smoothing_{suffix}()")),
                None => self.call(format!("rounded_smoothing({})", float(amount))),
            }
        }
        self.shadows();
        self.rings();
        self.filters();
        if let Some(opacity) = self.rest.opacity.take() {
            self.call(format!("opacity({})", float(opacity)));
        }
        if let Some(cursor) = self.rest.mouse_cursor.take() {
            let method = CURSORS
                .iter()
                .find(|(style, _)| *style == cursor)
                .map_or("cursor_default", |(_, method)| method);
            self.call(format!("{method}()"));
        }
    }

    /// `rounded` for all four corners, then top/bottom/left/right pairs, then
    /// single corners.
    fn corners(&mut self) {
        let radii = &mut self.rest.corner_radii;
        let mut corners = [
            radii.top_left.take(),
            radii.top_right.take(),
            radii.bottom_right.take(),
            radii.bottom_left.take(),
        ];
        const TOP_LEFT: usize = 0;
        const TOP_RIGHT: usize = 1;
        const BOTTOM_RIGHT: usize = 2;
        const BOTTOM_LEFT: usize = 3;
        if corners[0].is_some() && corners.iter().all(|corner| *corner == corners[0]) {
            if let Some(radius) = corners[0] {
                self.call(corner_call("rounded", radius));
            }
            return;
        }
        for (name, a, b) in [
            ("rounded_t", TOP_LEFT, TOP_RIGHT),
            ("rounded_b", BOTTOM_LEFT, BOTTOM_RIGHT),
            ("rounded_l", TOP_LEFT, BOTTOM_LEFT),
            ("rounded_r", TOP_RIGHT, BOTTOM_RIGHT),
        ] {
            if let Some(radius) = corners[a].filter(|_| corners[a] == corners[b]) {
                corners[a] = None;
                corners[b] = None;
                self.call(corner_call(name, radius));
            }
        }
        for (name, corner) in ["rounded_tl", "rounded_tr", "rounded_br", "rounded_bl"]
            .into_iter()
            .zip(corners)
        {
            if let Some(radius) = corner {
                self.call(corner_call(name, radius));
            }
        }
    }

    fn shadows(&mut self) {
        let Some(shadows) = &self.rest.box_shadow else {
            return;
        };
        let preset = if shadows.is_empty() {
            Some("shadow_none")
        } else {
            SHADOW_PRESETS
                .iter()
                .find(|(_, preset)| {
                    preset(StyleRefinement::default()).box_shadow.as_ref() == Some(shadows)
                })
                .map(|(name, _)| *name)
        };
        if let Some(preset) = preset {
            self.rest.box_shadow = None;
            self.call(format!("{preset}()"));
        }
    }

    fn rings(&mut self) {
        for name in ["ring", "inset_ring"] {
            let ring = if name == "ring" {
                &mut self.rest.ring
            } else {
                &mut self.rest.inset_ring
            };
            let width = ring.width.take();
            let color = match ring.color {
                Some(RingColor::Color(background)) => background.as_solid(),
                _ => None,
            };
            if color.is_some() {
                ring.color = None;
            }
            if let Some(width) = width {
                match RING_SCALE.iter().find(|(_, value)| px(*value) == width) {
                    Some((suffix, _)) => self.call(format!("{name}_{suffix}()")),
                    None => self.call(format!("{name}({})", pixels_expr(width))),
                }
            }
            if let Some(color) = color {
                self.call(format!("{name}_color({})", color_expr(color)));
            }
        }
    }

    /// `.blur(px(r))` appends to the chain's filters, so it only reproduces
    /// a single-filter list when the chain had none; otherwise the list is
    /// written out.
    fn filters(&mut self) {
        let prior = self.prior;
        for (name, append) in [("filter", "blur"), ("backdrop_filter", "backdrop_blur")] {
            let (filters, had_filters) = if name == "filter" {
                (
                    self.rest.filter.take(),
                    prior.is_some_and(|prior| prior.filter.is_some()),
                )
            } else {
                (
                    self.rest.backdrop_filter.take(),
                    prior.is_some_and(|prior| prior.backdrop_filter.is_some()),
                )
            };
            let Some(filters) = filters else { continue };
            match filters.as_slice() {
                [Filter::Blur(radius)] if !had_filters => {
                    self.call(format!("{append}({})", pixels_expr(*radius)));
                }
                _ => {
                    let list: Vec<String> = filters
                        .iter()
                        .map(|Filter::Blur(radius)| {
                            format!("Filter::Blur({})", pixels_expr(*radius))
                        })
                        .collect();
                    self.call(format!("{name}(vec![{}])", list.join(", ")));
                }
            }
        }
    }

    fn text(&mut self) {
        let text = &mut self.rest.text;
        let mut calls = Vec::new();
        if let Some(color) = text.color.take() {
            calls.push(format!("text_color({})", color_expr(color)));
        }
        if let Some(color) = text.background_color.take() {
            calls.push(format!("text_bg({})", color_expr(color)));
        }
        if let Some(family) = text.font_family.take() {
            calls.push(format!("font_family({:?})", family.as_ref()));
        }
        if let Some(size) = text.font_size.take() {
            let named = TEXT_SIZES
                .iter()
                .find(|(_, value)| AbsoluteLength::Rems(rems(*value)) == size);
            calls.push(match named {
                Some((suffix, _)) => format!("text_{suffix}()"),
                None => format!("text_size({})", absolute_expr(size)),
            });
        }
        if let Some(height) = text.line_height.take() {
            calls.push(format!("line_height({})", definite_expr(height)));
        }
        if let Some(weight) = text.font_weight.take() {
            calls.push(
                match FONT_WEIGHTS.iter().find(|(named, _)| *named == weight) {
                    Some((_, name)) => format!("font_weight(FontWeight::{name})"),
                    None => format!("font_weight(FontWeight({}))", float(weight.0)),
                },
            );
        }
        match text.font_style {
            Some(FontStyle::Italic) => calls.push("italic()".into()),
            Some(FontStyle::Normal) => calls.push("not_italic()".into()),
            _ => {}
        }
        if matches!(text.font_style, Some(FontStyle::Italic | FontStyle::Normal)) {
            text.font_style = None;
        }
        if let Some(underline) = text
            .underline
            .take_if(|underline| underline_calls(*underline).is_some())
        {
            calls.extend(underline_calls(underline).into_iter().flatten());
        }
        let line_through = StrikethroughStyle {
            thickness: px(1.),
            ..StrikethroughStyle::default()
        };
        if text
            .strikethrough
            .take_if(|strike| *strike == line_through)
            .is_some()
        {
            calls.push("line_through()".into());
        }
        if let Some(white_space) = text.white_space.take() {
            calls.push(match white_space {
                WhiteSpace::Normal => "whitespace_normal()".into(),
                WhiteSpace::Nowrap => "whitespace_nowrap()".into(),
            });
        }
        if let Some(overflow) = text.text_overflow.take() {
            calls.push(text_overflow_call(&overflow));
        }
        if let Some(align) = text.text_align.take() {
            calls.push(match align {
                TextAlign::Left => "text_left()".into(),
                TextAlign::Center => "text_center()".into(),
                TextAlign::Right => "text_right()".into(),
            });
        }
        if let Some(spacing) = text.letter_spacing.take() {
            calls.push(format!("letter_spacing({})", pixels_expr(spacing)));
        }
        if let Some(transform) = text.text_transform.take() {
            calls.push(format!("text_transform(TextTransform::{transform:?})"));
        }
        for call in calls {
            self.call(call);
        }
    }

    /// Emits a two-axis property (`size`/`w`/`h`, `gap`/`gap_x`/`gap_y`).
    fn pair<T: PartialEq + Copy>(
        &mut self,
        (both, horizontal, vertical): (&str, &str, &str),
        width: Option<T>,
        height: Option<T>,
        render: impl Fn(&str, T) -> String,
    ) {
        match (width, height) {
            (Some(width), Some(height)) if width == height => self.call(render(both, width)),
            _ => {
                if let Some(width) = width {
                    self.call(render(horizontal, width));
                }
                if let Some(height) = height {
                    self.call(render(vertical, height));
                }
            }
        }
    }

    /// Emits a four-sided property: all sides, then horizontal and vertical
    /// pairs, then single sides (top, right, bottom, left).
    fn edges<T: PartialEq + Copy>(
        &mut self,
        names: &EdgeNames,
        sides: [Option<T>; 4],
        render: impl Fn(&str, T) -> String,
    ) {
        let [top, right, bottom, left] = sides;
        if let Some(value) = top
            && sides.iter().all(|side| *side == top)
        {
            self.call(render(names.all, value));
            return;
        }
        let mut remaining = sides;
        const TOP: usize = 0;
        const RIGHT: usize = 1;
        const BOTTOM: usize = 2;
        const LEFT: usize = 3;
        if let (Some(name), Some(value)) = (names.horizontal, left.filter(|_| left == right)) {
            remaining[LEFT] = None;
            remaining[RIGHT] = None;
            self.call(render(name, value));
        }
        if let (Some(name), Some(value)) = (names.vertical, top.filter(|_| top == bottom)) {
            remaining[TOP] = None;
            remaining[BOTTOM] = None;
            self.call(render(name, value));
        }
        for (name, side) in names.sides.into_iter().zip(remaining) {
            if let Some(value) = side {
                self.call(render(name, value));
            }
        }
    }
}

fn take_size(size: &mut SizeRefinement<Length>) -> (Option<Length>, Option<Length>) {
    (size.width.take(), size.height.take())
}

/// `.underline()` then adjustments, when they reproduce `underline` exactly.
fn underline_calls(underline: UnderlineStyle) -> Option<Vec<String>> {
    let mut calls = vec!["underline()".to_string()];
    let thickness = underline.thickness;
    if thickness != px(1.) {
        let step = [0., 2., 4., 8.]
            .into_iter()
            .find(|step| px(*step) == thickness)?;
        calls.push(format!("text_decoration_{step}()"));
    }
    if let Some(color) = underline.color {
        calls.push(format!("text_decoration_color({})", color_expr(color)));
    }
    if underline.wavy {
        calls.push("text_decoration_wavy()".into());
    }
    Some(calls)
}

fn text_overflow_call(overflow: &TextOverflow) -> String {
    match overflow {
        TextOverflow::Truncate(ellipsis) if ellipsis.as_ref() == ELLIPSIS => {
            "text_ellipsis()".into()
        }
        TextOverflow::TruncateStart(ellipsis) if ellipsis.as_ref() == ELLIPSIS => {
            "text_ellipsis_start()".into()
        }
        TextOverflow::TruncateMiddle(ellipsis) if ellipsis.as_ref() == ELLIPSIS => {
            "text_ellipsis_middle()".into()
        }
        TextOverflow::Truncate(ellipsis) => {
            format!(
                "text_overflow(TextOverflow::Truncate({:?}.into()))",
                ellipsis.as_ref()
            )
        }
        TextOverflow::TruncateStart(ellipsis) => format!(
            "text_overflow(TextOverflow::TruncateStart({:?}.into()))",
            ellipsis.as_ref()
        ),
        TextOverflow::TruncateMiddle(ellipsis) => format!(
            "text_overflow(TextOverflow::TruncateMiddle({:?}.into()))",
            ellipsis.as_ref()
        ),
    }
}

/// A `Length` helper call: a scale suffix (`w_4`, `mx_auto`, `m_neg_2`) when
/// one reproduces `length`, else the custom setter (`w(px(120.))`).
fn length_call(prefix: &str, length: Length, negatable: bool) -> String {
    let Length::Definite(definite) = length else {
        return format!("{prefix}_auto()");
    };
    if let Some(suffix) = spacing_suffix(definite) {
        return format!("{prefix}_{suffix}()");
    }
    if negatable && let Some(suffix) = spacing_suffix(-definite) {
        return format!("{prefix}_neg_{suffix}()");
    }
    format!("{prefix}({})", definite_expr(definite))
}

/// A `DefiniteLength` helper call (padding, gap).
fn definite_call(prefix: &str, length: DefiniteLength) -> String {
    match spacing_suffix(length) {
        Some(suffix) => format!("{prefix}_{suffix}()"),
        None => format!("{prefix}({})", definite_expr(length)),
    }
}

fn border_call(prefix: &str, width: AbsoluteLength) -> String {
    let named = BORDER_SCALE
        .iter()
        .find(|(_, value)| AbsoluteLength::Pixels(px(*value)) == width);
    match named {
        Some((suffix, _)) => format!("{prefix}_{suffix}()"),
        None => format!("{prefix}({})", absolute_expr(width)),
    }
}

fn corner_call(prefix: &str, radius: AbsoluteLength) -> String {
    match CORNER_SCALE.iter().find(|(_, value)| *value == radius) {
        Some((suffix, _)) => format!("{prefix}_{suffix}()"),
        None => format!("{prefix}({})", absolute_expr(radius)),
    }
}

fn spacing_suffix(length: DefiniteLength) -> Option<&'static str> {
    SPACING_SCALE
        .iter()
        .find(|(_, value)| *value == length)
        .map(|(suffix, _)| *suffix)
}

/// A Rust `f32` literal: `12.`, `0.5`, `-4.` (shortest form that parses back
/// to the same value).
fn float(value: f32) -> String {
    if value.is_nan() {
        return "f32::NAN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            "f32::INFINITY"
        } else {
            "f32::NEG_INFINITY"
        }
        .into();
    }
    let digits = value.to_string();
    if digits.contains('.') {
        digits
    } else {
        format!("{digits}.")
    }
}

fn pixels_expr(pixels: Pixels) -> String {
    format!("px({})", float(pixels.as_f32()))
}

fn absolute_expr(length: AbsoluteLength) -> String {
    match length {
        AbsoluteLength::Pixels(pixels) => pixels_expr(pixels),
        AbsoluteLength::Rems(rems) => format!("rems({})", float(rems.0)),
    }
}

fn definite_expr(length: DefiniteLength) -> String {
    match length {
        DefiniteLength::Absolute(absolute) => absolute_expr(absolute),
        DefiniteLength::Relative(fraction) => format!("relative({})", float(fraction.as_f32())),
    }
}

fn length_expr(length: Length) -> String {
    match length {
        Length::Definite(definite) => definite_expr(definite),
        Length::Auto => "auto()".into(),
    }
}

/// `rgb(0x3366ff)` / `rgba(0x3366ff80)` for colors on the 8-bit grid,
/// `hsla(h, s, l, a)` otherwise.
fn color_expr(color: Hsla) -> String {
    let rgba = hsla_to_rgba(color);
    let byte = |channel: f32| {
        let scaled = channel * 255.0;
        ((scaled - scaled.round()).abs() <= HEX_TOLERANCE
            && (0.0..=255.0).contains(&scaled.round()))
        .then(|| scaled.round() as u32)
    };
    let (red, green, blue, alpha) = (
        byte(rgba.color.red),
        byte(rgba.color.green),
        byte(rgba.color.blue),
        byte(rgba.alpha),
    );
    match (red, green, blue, alpha) {
        (Some(red), Some(green), Some(blue), Some(255)) => {
            format!("rgb(0x{:06x})", red << 16 | green << 8 | blue)
        }
        (Some(red), Some(green), Some(blue), Some(alpha)) => {
            format!(
                "rgba(0x{:08x})",
                red << 24 | green << 16 | blue << 8 | alpha
            )
        }
        _ => {
            let component = |value: f32| {
                let text = format!("{value:.HSLA_DECIMALS$}");
                let text = text.trim_end_matches('0').trim_end_matches('.');
                if text.is_empty() || text == "-" {
                    "0".to_string()
                } else {
                    text.to_string()
                }
            };
            format!(
                "hsla({}, {}, {}, {})",
                component(color.hue.into_positive_degrees() / 360.0),
                component(color.saturation),
                component(color.lightness),
                component(color.alpha)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TextTransform, hsla, linear_color_stop, linear_gradient, rgb, rgba};

    fn s() -> StyleRefinement {
        StyleRefinement::default()
    }

    fn calls(style: StyleRefinement) -> Vec<String> {
        let patch = builder_calls(&style);
        assert!(
            patch.unmapped.is_empty(),
            "unexpected unmapped: {:?}",
            patch.unmapped
        );
        patch.calls
    }

    /// Each style is built with exactly the calls it must print, so every
    /// case proves the mapping reproduces the value.
    fn assert_cases(cases: Vec<(StyleRefinement, &[&str])>) {
        for (style, expected) in cases {
            assert_eq!(calls(style), expected.to_vec());
        }
    }

    #[test]
    fn empty_style_has_no_calls() {
        let patch = builder_calls(&s());
        assert!(patch.is_empty());
        assert_eq!(patch.to_code(), "");
    }

    #[test]
    fn spacing_uses_exact_shorthands_only() {
        assert_cases(vec![
            (s().p_2(), &[".p_2()"]),
            (s().p(px(8.)), &[".p(px(8.))"]),
            (s().p_px(), &[".p_px()"]),
            (s().px_2().py_1(), &[".px_2()", ".py_1()"]),
            (s().pt_3(), &[".pt_3()"]),
            (
                s().p_2().pl(px(12.)),
                &[".py_2()", ".pr_2()", ".pl(px(12.))"],
            ),
            (s().p(rems(0.3)), &[".p(rems(0.3))"]),
            (s().p_1_2(), &[".p_1_2()"]),
            (s().m_neg_2(), &[".m_neg_2()"]),
            (s().mx_auto(), &[".mx_auto()"]),
            (s().mt(px(-3.)), &[".mt(px(-3.))"]),
            (s().my_4().ml_neg_1(), &[".my_4()", ".ml_neg_1()"]),
            (s().gap_2(), &[".gap_2()"]),
            (s().gap_x(px(6.)), &[".gap_x(px(6.))"]),
            (s().gap_x_1().gap_y_3(), &[".gap_x_1()", ".gap_y_3()"]),
        ]);
    }

    #[test]
    fn sizes_and_positions() {
        assert_cases(vec![
            (s().w(px(120.)), &[".w(px(120.))"]),
            (s().w_full(), &[".w_full()"]),
            (s().w_1_3(), &[".w_1_3()"]),
            (s().w_5_6(), &[".w_5_6()"]),
            (s().size_4(), &[".size_4()"]),
            (s().h(rems(1.5)), &[".h_6()"]),
            (
                s().w_auto().h(relative(0.3)),
                &[".w_auto()", ".h(relative(0.3))"],
            ),
            (
                s().min_w(px(80.)).max_h_96(),
                &[".min_w(px(80.))", ".max_h_96()"],
            ),
            (s().min_size_0(), &[".min_size_0()"]),
            (s().aspect_square(), &[".aspect_square()"]),
            (s().aspect_ratio(1.5), &[".aspect_ratio(1.5)"]),
            (s().absolute().inset_0(), &[".absolute()", ".inset_0()"]),
            (
                s().top(px(3.)).left_neg_1(),
                &[".top(px(3.))", ".left_neg_1()"],
            ),
            (
                s().relative().right_auto(),
                &[".relative()", ".right_auto()"],
            ),
        ]);
    }

    #[test]
    fn layout_and_flex() {
        assert_cases(vec![
            (s().flex().flex_col(), &[".flex()", ".flex_col()"]),
            (s().grid().grid_cols(3), &[".grid()", ".grid_cols(3)"]),
            (s().grid_rows_max_content(2), &[".grid_rows_max_content(2)"]),
            (s().hidden(), &[".hidden()"]),
            (s().invisible(), &[".invisible()"]),
            (s().flex_1(), &[".flex_1()"]),
            (s().flex_auto(), &[".flex_auto()"]),
            (s().flex_initial(), &[".flex_initial()"]),
            (s().flex_none(), &[".flex_none()"]),
            (s().flex_grow(2.), &[".flex_grow(2.)"]),
            (
                s().flex_grow_1().flex_shrink_0(),
                &[".flex_grow_1()", ".flex_shrink_0()"],
            ),
            (s().flex_basis(px(40.)), &[".flex_basis(px(40.))"]),
            (s().flex_wrap_reverse(), &[".flex_wrap_reverse()"]),
            (
                s().items_center().justify_between(),
                &[".items_center()", ".justify_between()"],
            ),
            (
                s().self_end().content_around(),
                &[".self_end()", ".content_around()"],
            ),
            (s().overflow_hidden(), &[".overflow_hidden()"]),
            (s().overflow_y_hidden(), &[".overflow_y_hidden()"]),
            (s().scrollbar_width(px(8.)), &[".scrollbar_width(px(8.))"]),
        ]);
    }

    #[test]
    fn visuals() {
        assert_cases(vec![
            (s().bg(rgb(0x3366ff)), &[".bg(rgb(0x3366ff))"]),
            (s().bg(rgba(0x3366ff80)), &[".bg(rgba(0x3366ff80))"]),
            (
                s().bg(hsla(0.5, 0.5, 0.5, 0.123)),
                &[".bg(hsla(0.5, 0.5, 0.5, 0.123))"],
            ),
            (s().border_1(), &[".border_1()"]),
            (
                s().border_x_2().border_t(px(1.5)),
                &[".border_x_2()", ".border_t(px(1.5))"],
            ),
            (
                s().border_color(rgb(0xff0000)).border_dashed(),
                &[".border_color(rgb(0xff0000))", ".border_dashed()"],
            ),
            (s().border_dashed_gap(2.), &[".border_dashed_gap(2.)"]),
            (s().rounded_md(), &[".rounded_md()"]),
            (s().rounded_full(), &[".rounded_full()"]),
            (s().rounded_t_lg(), &[".rounded_t_lg()"]),
            (
                s().rounded_l_sm().rounded_tr(px(5.)),
                &[".rounded_l_sm()", ".rounded_tr(px(5.))"],
            ),
            (s().rounded_smoothing_0p5(), &[".rounded_smoothing_0p5()"]),
            (s().rounded_smoothing(0.55), &[".rounded_smoothing(0.55)"]),
            (s().shadow_md(), &[".shadow_md()"]),
            (s().shadow_none(), &[".shadow_none()"]),
            (
                s().ring_2().ring_color(rgb(0x00ff00)),
                &[".ring_2()", ".ring_color(rgb(0x00ff00))"],
            ),
            (s().inset_ring(px(3.)), &[".inset_ring(px(3.))"]),
            (s().blur(px(4.)), &[".blur(px(4.))"]),
            (
                s().blur(px(4.)).blur(px(2.)),
                &[".filter(vec![Filter::Blur(px(4.)), Filter::Blur(px(2.))])"],
            ),
            (s().backdrop_blur(px(12.)), &[".backdrop_blur(px(12.))"]),
            (s().opacity(0.5), &[".opacity(0.5)"]),
            (s().cursor_pointer(), &[".cursor_pointer()"]),
            (s().cursor_ns_resize(), &[".cursor_ns_resize()"]),
        ]);
    }

    #[test]
    fn text() {
        assert_cases(vec![
            (s().text_sm(), &[".text_sm()"]),
            (s().text_size(px(13.)), &[".text_size(px(13.))"]),
            (s().text_size(rems(0.8)), &[".text_size(rems(0.8))"]),
            (
                s().text_color(rgb(0x112233)),
                &[".text_color(rgb(0x112233))"],
            ),
            (s().text_bg(rgb(0xffff00)), &[".text_bg(rgb(0xffff00))"]),
            (
                s().font_family("Lilex \"Mono\""),
                &[".font_family(\"Lilex \\\"Mono\\\"\")"],
            ),
            (
                s().font_weight(FontWeight::BOLD),
                &[".font_weight(FontWeight::BOLD)"],
            ),
            (
                s().font_weight(FontWeight(450.)),
                &[".font_weight(FontWeight(450.))"],
            ),
            (s().italic(), &[".italic()"]),
            (
                s().line_height(relative(1.5)),
                &[".line_height(relative(1.5))"],
            ),
            (s().underline(), &[".underline()"]),
            (
                s().underline().text_decoration_2().text_decoration_wavy(),
                &[
                    ".underline()",
                    ".text_decoration_2()",
                    ".text_decoration_wavy()",
                ],
            ),
            (
                s().underline().text_decoration_color(rgb(0xabcdef)),
                &[".underline()", ".text_decoration_color(rgb(0xabcdef))"],
            ),
            (s().line_through(), &[".line_through()"]),
            (s().truncate(), &[".truncate()"]),
            (s().line_clamp(3), &[".line_clamp(3)"]),
            (s().text_ellipsis_middle(), &[".text_ellipsis_middle()"]),
            (s().whitespace_nowrap(), &[".whitespace_nowrap()"]),
            (s().text_center(), &[".text_center()"]),
            (s().letter_spacing(px(0.5)), &[".letter_spacing(px(0.5))"]),
            (
                s().text_transform(TextTransform::Uppercase),
                &[".text_transform(TextTransform::Uppercase)"],
            ),
        ]);
    }

    #[test]
    fn calls_follow_a_stable_order() {
        let style = s()
            .text_color(rgb(0x000000))
            .bg(rgb(0xffffff))
            .p_2()
            .w_full()
            .items_center()
            .flex();
        assert_eq!(
            builder_calls(&style).to_code(),
            ".flex()\n.items_center()\n.w_full()\n.p_2()\n.bg(rgb(0xffffff))\n.text_color(rgb(0x000000))"
        );
    }

    #[test]
    fn unmappable_values_become_comments() {
        let mut style = s()
            .p_2()
            .bg(linear_gradient(
                90.,
                linear_color_stop(rgb(0xff0000), 0.),
                linear_color_stop(rgb(0x0000ff), 1.),
            ))
            .col_span(2);
        style.align_items = Some(AlignItems::Start);
        style.allow_concurrent_scroll = Some(true);
        let patch = builder_calls(&style);
        assert_eq!(patch.calls, vec![".p_2()"]);
        let labels: Vec<&str> = patch
            .unmapped
            .iter()
            .map(|line| line.split(" = ").next().unwrap())
            .collect();
        assert_eq!(
            labels,
            vec![
                "allow_concurrent_scroll",
                "align_items",
                "background",
                "grid_location"
            ]
        );
        assert!(
            patch
                .to_code()
                .contains("\n// unmapped: allow_concurrent_scroll = true")
        );
        assert!(patch.to_code().contains("// unmapped: align_items = Start"));
    }

    #[test]
    fn a_patch_contains_only_what_changed() {
        let base = s().p_2().flex().bg(rgb(0xff0000));
        let overrides = s().pl(px(12.)).flex();
        assert_eq!(rust_patch(&base, &overrides).calls, vec![".pl(px(12.))"]);
        assert_eq!(rust_patch(&base, &s().p_3()).calls, vec![".p_3()"]);
        assert!(rust_patch(&base, &base).is_empty());
        assert!(rust_patch(&base, &s()).is_empty());
    }

    #[test]
    fn appending_blur_to_a_chain_with_filters_writes_the_list() {
        let base = s().blur(px(4.));
        let overrides = s().blur(px(8.));
        assert_eq!(
            rust_patch(&base, &overrides).calls,
            vec![".filter(vec![Filter::Blur(px(8.))])"]
        );
        assert_eq!(rust_patch(&s(), &overrides).calls, vec![".blur(px(8.))"]);
    }

    #[test]
    fn line_clamp_in_a_patch_relies_on_the_merged_overflow() {
        let base = s().overflow_hidden();
        let overrides = {
            let mut overrides = s();
            overrides.text.line_clamp = Some(2);
            overrides
        };
        assert_eq!(rust_patch(&base, &overrides).calls, vec![".line_clamp(2)"]);
        // Without hidden overflow, `.line_clamp` would change it too.
        let patch = rust_patch(&s(), &overrides);
        assert!(patch.calls.is_empty());
        assert_eq!(patch.unmapped, vec!["text.line_clamp = 2"]);
    }

    #[test]
    fn patch_diff_shows_removed_and_added_calls() {
        let base = s().flex().p_2();
        let lines = patch_diff(&base, &s().p_3());
        assert_eq!(
            lines,
            vec![
                DiffLine {
                    kind: DiffKind::Kept,
                    text: ".flex()".into()
                },
                DiffLine {
                    kind: DiffKind::Removed,
                    text: ".p_2()".into()
                },
                DiffLine {
                    kind: DiffKind::Added,
                    text: ".p_3()".into()
                },
            ]
        );
        assert_eq!(diff_text(&lines), "- .p_2()\n+ .p_3()");

        let added = patch_diff(&base, &s().bg(rgb(0xff0000)));
        assert_eq!(diff_text(&added), "+ .bg(rgb(0xff0000))");
        assert!(diff_text(&patch_diff(&base, &s())).is_empty());
    }

    #[test]
    fn line_diff_handles_empty_sides() {
        let lines = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        assert!(line_diff(&[], &[]).is_empty());
        assert_eq!(diff_text(&line_diff(&lines(&["a"]), &[])), "- a");
        assert_eq!(diff_text(&line_diff(&[], &lines(&["a", "b"]))), "+ a\n+ b");
        assert_eq!(
            diff_text(&line_diff(
                &lines(&["a", "b", "c"]),
                &lines(&["a", "x", "c"])
            )),
            "- b\n+ x"
        );
    }

    #[test]
    fn floats_print_as_rust_literals() {
        assert_eq!(float(12.), "12.");
        assert_eq!(float(-4.), "-4.");
        assert_eq!(float(0.5), "0.5");
        assert_eq!(float(1e-7), "0.0000001");
        assert_eq!(float(f32::NAN), "f32::NAN");
        assert_eq!(float(f32::NEG_INFINITY), "f32::NEG_INFINITY");
        assert_eq!(float(3e9), "3000000000.");
    }
}
