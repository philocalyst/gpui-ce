//! "Why is this element this size?": one explanation per axis.
//!
//! Layout is not re-run. Instead the explanation is reconstructed from what
//! the capture recorded: the element's [`LayoutFacts`] (as written in its
//! style), its box model, its parent's facts and the measured bounds of its
//! siblings and children. Each rule checks that its arithmetic reproduces the
//! measured size (within [`TOLERANCE`]) before it claims to be the reason, so
//! an explanation is either exact or honestly marked as unknown.

use super::format;
use gpui::{
    Along as _, Axis, Pixels,
    inspector::{
        BoxModel, ElementFlags, ElementIndex, ElementRecord, ElementTree, LayoutFacts, SizeSpec,
    },
    px,
};

/// Layout rounds to device pixels, so measured and computed sizes may differ
/// by up to this much and still be the same size.
pub const TOLERANCE: Pixels = px(0.5);

/// Why an element has its size on both axes.
#[derive(Clone, Debug, PartialEq)]
pub struct SizeExplanation {
    /// The horizontal axis.
    pub width: AxisExplanation,
    /// The vertical axis.
    pub height: AxisExplanation,
}

/// Why an element has its size along one axis.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisExplanation {
    /// The axis explained.
    pub axis: Axis,
    /// The measured length along [`Self::axis`].
    pub size: Pixels,
    /// The rule that produced it.
    pub reason: SizeReason,
}

/// The rule that sized an element along one axis.
#[derive(Clone, Debug, PartialEq)]
pub enum SizeReason {
    /// The style sets an explicit length.
    Fixed,
    /// The style sets a fraction of the parent's content box.
    Fraction {
        /// The fraction, 1.0 = 100%.
        fraction: f32,
        /// The parent's content size it applies to.
        of: Pixels,
    },
    /// A flex item grew into the space its siblings left on the main axis.
    Grew {
        /// The parent's content size along the main axis.
        available: Pixels,
        /// The in-flow siblings' outer sizes, summed.
        siblings: Pixels,
        /// The parent's gaps between items, summed.
        gaps: Pixels,
        /// The element's own margins along the axis.
        margins: Pixels,
        /// Other siblings that also grow and so share the space.
        other_growers: usize,
    },
    /// A flex item shrank below its requested size because the line overflowed.
    Shrunk {
        /// The size it asked for (its flex basis or explicit size).
        requested: Pixels,
        /// How far the line overflowed its container before shrinking.
        overflow: Pixels,
        /// Its flex shrink factor.
        shrink: f32,
    },
    /// Stretched across the parent's cross axis (align stretch, the default).
    Stretched {
        /// The parent's content size along the cross axis.
        container: Pixels,
    },
    /// Sized by what it contains.
    Content(ContentSource),
    /// Held at its minimum size.
    MinClamped {
        /// The minimum, resolved to pixels.
        min: Pixels,
    },
    /// Capped at its maximum size.
    MaxClamped {
        /// The maximum, resolved to pixels.
        max: Pixels,
    },
    /// Derived from the other axis through an aspect ratio (width / height).
    AspectRatio {
        /// Width divided by height.
        ratio: f32,
        /// The measured size of the other axis.
        other: Pixels,
    },
    /// Absolutely positioned: out of its parent's flow, sized by insets or content.
    Absolute,
    /// The window root, which fills the app's area.
    ViewportRoot,
    /// The element reported no layout facts, so no rule can be checked.
    NotCaptured,
    /// Facts were captured but no single rule reproduces the measured size.
    Unexplained,
}

/// What a content-sized element's size comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum ContentSource {
    /// Its children's extent plus its own padding and border.
    Children {
        /// From the first child's outer start to the last child's outer end.
        extent: Pixels,
        /// Its own padding plus border along the axis.
        insets: Pixels,
    },
    /// Its text.
    Text,
    /// Nothing: no explicit size and nothing inside.
    Empty,
}

impl AxisExplanation {
    /// One sentence that answers why first, then gives the numbers:
    /// "Grew into the space left in its row: parent 628 − siblings 232 = 396."
    pub fn sentence(&self) -> String {
        let size = format::pixels(self.size);
        let dimension = dimension(self.axis);
        match &self.reason {
            SizeReason::Fixed => format!("Fixed: the style sets its {dimension} to {size}."),
            SizeReason::Fraction { fraction, of } => format!(
                "{} of its parent's {} = {size}.",
                format::percent(f64::from(*fraction)),
                format::pixels(*of)
            ),
            SizeReason::Grew {
                available,
                siblings,
                gaps,
                margins,
                other_growers,
            } => {
                let mut sentence = format!(
                    "Grew into the space left in its {}: parent {} − siblings {}",
                    line_name(self.axis),
                    format::pixels(*available),
                    format::pixels(*siblings)
                );
                for (amount, label) in [(gaps, "gaps"), (margins, "margins")] {
                    if *amount != Pixels::ZERO {
                        sentence.push_str(&format!(" − {label} {}", format::pixels(*amount)));
                    }
                }
                sentence.push_str(&format!(" = {size}."));
                match other_growers {
                    0 => {}
                    1 => sentence.push_str(" Shared with 1 other growing item."),
                    count => {
                        sentence.push_str(&format!(" Shared with {count} other growing items."))
                    }
                }
                sentence
            }
            SizeReason::Shrunk {
                requested,
                overflow,
                shrink,
            } => format!(
                "Shrank from {} to {size}: its {} was {} too {} (flex_shrink {}).",
                format::pixels(*requested),
                line_name(self.axis),
                format::pixels(*overflow),
                too_small(self.axis),
                format::number(*shrink)
            ),
            SizeReason::Stretched { container } => format!(
                "Stretched to fill its {}'s {dimension}: {}.",
                line_name(self.axis.invert()),
                format::pixels(*container)
            ),
            SizeReason::Content(ContentSource::Children { extent, insets }) => {
                if *insets == Pixels::ZERO {
                    format!("Sized by its content: its children span {size}.")
                } else {
                    format!(
                        "Sized by its content: children {} + padding and border {} = {size}.",
                        format::pixels(*extent),
                        format::pixels(*insets)
                    )
                }
            }
            SizeReason::Content(ContentSource::Text) => {
                format!("Sized by its text: {size}.")
            }
            SizeReason::Content(ContentSource::Empty) => {
                format!("Empty: no {dimension} is set and nothing is inside, so it is {size}.")
            }
            SizeReason::MinClamped { min } => {
                format!("Held at its minimum {dimension}: {}.", format::pixels(*min))
            }
            SizeReason::MaxClamped { max } => {
                format!(
                    "Capped at its maximum {dimension}: {}.",
                    format::pixels(*max)
                )
            }
            SizeReason::AspectRatio { ratio, other } => match self.axis {
                Axis::Horizontal => format!(
                    "Follows its aspect ratio: height {} × {} = {size}.",
                    format::pixels(*other),
                    format::number(*ratio)
                ),
                Axis::Vertical => format!(
                    "Follows its aspect ratio: width {} ÷ {} = {size}.",
                    format::pixels(*other),
                    format::number(*ratio)
                ),
            },
            SizeReason::Absolute => format!(
                "Positioned absolutely: its insets or content set its {dimension} ({size}), \
                 not its parent's layout."
            ),
            SizeReason::ViewportRoot => {
                format!(
                    "The window root: it fills the app area, {size} {}.",
                    extent_word(self.axis)
                )
            }
            SizeReason::NotCaptured => "Sized by layout; details not captured.".to_string(),
            SizeReason::Unexplained => format!(
                "Sized by layout: no single rule explains {size} \
                 (margins, alignment or grid placement may apply)."
            ),
        }
    }
}

fn dimension(axis: Axis) -> &'static str {
    match axis {
        Axis::Horizontal => "width",
        Axis::Vertical => "height",
    }
}

fn extent_word(axis: Axis) -> &'static str {
    match axis {
        Axis::Horizontal => "wide",
        Axis::Vertical => "tall",
    }
}

fn too_small(axis: Axis) -> &'static str {
    match axis {
        Axis::Horizontal => "narrow",
        Axis::Vertical => "short",
    }
}

/// What a flex line along `axis` is called.
fn line_name(axis: Axis) -> &'static str {
    match axis {
        Axis::Horizontal => "row",
        Axis::Vertical => "column",
    }
}

/// Explains both axes of element `ix`. `None` if `ix` is not in `tree`.
pub fn explain_size(tree: &ElementTree, ix: ElementIndex) -> Option<SizeExplanation> {
    Some(SizeExplanation {
        width: explain_axis(tree, ix, Axis::Horizontal)?,
        height: explain_axis(tree, ix, Axis::Vertical)?,
    })
}

/// Explains element `ix`'s size along `axis`. `None` if `ix` is not in `tree`.
pub fn explain_axis(tree: &ElementTree, ix: ElementIndex, axis: Axis) -> Option<AxisExplanation> {
    let element = Element::new(tree, ix)?;
    let size = element.record.bounds.size.along(axis);
    Some(AxisExplanation {
        axis,
        size,
        reason: element.reason(axis, size),
    })
}

/// An element with the facts the rules read.
struct Element<'a> {
    tree: &'a ElementTree,
    ix: ElementIndex,
    record: &'a ElementRecord,
    facts: Option<&'a LayoutFacts>,
    box_model: BoxModel,
    parent: Option<ElementIndex>,
}

impl<'a> Element<'a> {
    fn new(tree: &'a ElementTree, ix: ElementIndex) -> Option<Self> {
        let record = tree.get(ix)?;
        let details = record.details.as_deref();
        Some(Self {
            tree,
            ix,
            record,
            facts: details.and_then(|details| details.layout.as_ref()),
            box_model: details
                .and_then(|details| details.box_model)
                .unwrap_or_default(),
            parent: record.parent,
        })
    }

    fn is_window_root(&self) -> bool {
        self.parent.is_none() && !self.record.flags.contains(ElementFlags::DEFERRED)
    }

    fn reason(&self, axis: Axis, size: Pixels) -> SizeReason {
        let Some(facts) = self.facts else {
            return if self.is_window_root() {
                SizeReason::ViewportRoot
            } else {
                SizeReason::NotCaptured
            };
        };
        let parent = self
            .parent
            .and_then(|parent| Element::new(self.tree, parent));
        let parent_inner = parent.as_ref().map(|parent| parent.inner(axis));
        let requested = facts.size.along(axis);

        if let SizeSpec::Pixels(length) = requested
            && close(length, size)
        {
            return SizeReason::Fixed;
        }
        if let Some(reason) = self.clamp(facts, axis, size, parent_inner) {
            return reason;
        }
        if let (SizeSpec::Fraction(fraction), Some(of)) = (requested, parent_inner)
            && close(of * fraction, size)
        {
            return SizeReason::Fraction { fraction, of };
        }
        if self.is_window_root() {
            return SizeReason::ViewportRoot;
        }
        if facts.absolute {
            return SizeReason::Absolute;
        }
        if let Some(reason) = self.aspect_ratio(facts, axis, size) {
            return reason;
        }
        if let Some(parent) = &parent
            && let Some(reason) = self.in_flow(parent, facts, axis, size)
        {
            return reason;
        }
        self.content(axis, size).unwrap_or(SizeReason::Unexplained)
    }

    /// Min or max constraints that the measured size sits on.
    fn clamp(
        &self,
        facts: &LayoutFacts,
        axis: Axis,
        size: Pixels,
        parent_inner: Option<Pixels>,
    ) -> Option<SizeReason> {
        let resolve = |spec: SizeSpec| match spec {
            SizeSpec::Auto => None,
            SizeSpec::Pixels(length) => Some(length),
            SizeSpec::Fraction(fraction) => parent_inner.map(|inner| inner * fraction),
        };
        if let Some(min) = resolve(facts.min_size.along(axis))
            && close(min, size)
        {
            return Some(SizeReason::MinClamped { min });
        }
        if let Some(max) = resolve(facts.max_size.along(axis))
            && close(max, size)
        {
            return Some(SizeReason::MaxClamped { max });
        }
        None
    }

    fn aspect_ratio(&self, facts: &LayoutFacts, axis: Axis, size: Pixels) -> Option<SizeReason> {
        let ratio = facts
            .aspect_ratio
            .filter(|ratio| ratio.is_finite() && *ratio > 0.0)?;
        if facts.size.along(axis) != SizeSpec::Auto {
            return None;
        }
        let other = self.record.bounds.size.along(axis.invert());
        let derived = match axis {
            Axis::Horizontal => other * ratio,
            Axis::Vertical => other / ratio,
        };
        close(derived, size).then_some(SizeReason::AspectRatio { ratio, other })
    }

    /// Rules that depend on the parent's layout: flex grow and shrink on the
    /// main axis, stretch on the cross axis, block children filling the width.
    fn in_flow(
        &self,
        parent: &Element<'_>,
        facts: &LayoutFacts,
        axis: Axis,
        size: Pixels,
    ) -> Option<SizeReason> {
        let parent_facts = parent.facts?;
        let inner = parent.inner(axis);
        let margins = self.margins(axis);
        let fills_container = || {
            (facts.size.along(axis) == SizeSpec::Auto && close(inner - margins, size))
                .then_some(SizeReason::Stretched { container: inner })
        };
        if is_display(parent_facts, "block") && axis == Axis::Horizontal {
            return fills_container();
        }
        let main_axis = flex_main_axis(parent_facts)?;
        if axis != main_axis {
            return fills_container();
        }

        let siblings = parent.in_flow_children().filter(|&child| child != self.ix);
        let (mut sibling_sizes, mut other_growers) = (Pixels::ZERO, 0);
        for sibling in siblings
            .clone()
            .filter_map(|ix| Element::new(self.tree, ix))
        {
            sibling_sizes += sibling.record.bounds.size.along(axis) + sibling.margins(axis);
            other_growers += usize::from(sibling.facts.is_some_and(|facts| facts.flex_grow > 0.0));
        }
        let item_count = siblings.count() + 1;
        let gaps = parent_facts.gap.along(axis) * (item_count - 1) as f32;
        let used = sibling_sizes + gaps + margins;

        if facts.flex_grow > 0.0 && close(inner - used, size) {
            return Some(SizeReason::Grew {
                available: inner,
                siblings: sibling_sizes,
                gaps,
                margins,
                other_growers,
            });
        }
        let requested = match (facts.flex_basis, facts.size.along(axis)) {
            (SizeSpec::Pixels(basis), _) | (SizeSpec::Auto, SizeSpec::Pixels(basis)) => basis,
            _ => return None,
        };
        (facts.flex_shrink > 0.0 && size + TOLERANCE < requested).then(|| SizeReason::Shrunk {
            requested,
            overflow: used + requested - inner,
            shrink: facts.flex_shrink,
        })
    }

    /// Sized by children, text, or nothing at all.
    fn content(&self, axis: Axis, size: Pixels) -> Option<SizeReason> {
        let insets = self.insets(axis);
        let mut children = self
            .in_flow_children()
            .filter_map(|ix| Element::new(self.tree, ix));
        if let Some(first) = children.next() {
            let (start, end) = children.fold(first.outer_span(axis), |(start, end), child| {
                let (child_start, child_end) = child.outer_span(axis);
                (start.min(child_start), end.max(child_end))
            });
            let extent = end - start;
            return close(extent + insets, size).then_some(SizeReason::Content(
                ContentSource::Children { extent, insets },
            ));
        }
        let has_text = self
            .record
            .details
            .as_ref()
            .is_some_and(|details| details.text.as_ref().is_some_and(|text| !text.is_empty()));
        if has_text {
            return Some(SizeReason::Content(ContentSource::Text));
        }
        close(insets, size).then_some(SizeReason::Content(ContentSource::Empty))
    }

    /// Children that take part in layout (not absolutely positioned).
    fn in_flow_children(&self) -> impl Iterator<Item = ElementIndex> + Clone + '_ {
        self.tree
            .children(self.ix)
            .iter()
            .copied()
            .filter(|&child| {
                !self
                    .tree
                    .get(child)
                    .and_then(|record| record.details.as_ref()?.layout.as_ref())
                    .is_some_and(|facts| facts.absolute)
            })
    }

    /// Start and end along `axis`, including margins.
    fn outer_span(&self, axis: Axis) -> (Pixels, Pixels) {
        let origin = self.record.bounds.origin.along(axis);
        let length = self.record.bounds.size.along(axis);
        let (margin_start, margin_end) = edges(&self.box_model.margin, axis);
        (origin - margin_start, origin + length + margin_end)
    }

    /// The content-box size along `axis`: bounds minus padding and border.
    fn inner(&self, axis: Axis) -> Pixels {
        self.record.bounds.size.along(axis) - self.insets(axis)
    }

    /// Padding plus border along `axis`.
    fn insets(&self, axis: Axis) -> Pixels {
        let (padding_start, padding_end) = edges(&self.box_model.padding, axis);
        let (border_start, border_end) = edges(&self.box_model.border, axis);
        padding_start + padding_end + border_start + border_end
    }

    fn margins(&self, axis: Axis) -> Pixels {
        let (start, end) = edges(&self.box_model.margin, axis);
        start + end
    }
}

/// The two edges that bound `axis`: left/right or top/bottom.
fn edges(edges: &gpui::Edges<Pixels>, axis: Axis) -> (Pixels, Pixels) {
    match axis {
        Axis::Horizontal => (edges.left, edges.right),
        Axis::Vertical => (edges.top, edges.bottom),
    }
}

/// Whether `facts.display` names `display`, however the engine spelled it.
fn is_display(facts: &LayoutFacts, display: &str) -> bool {
    facts.display.eq_ignore_ascii_case(display)
}

/// The main axis of a flex container, `None` for other displays. Accepts
/// `row`, `Row`, `column`, `ColumnReverse`, `column-reverse`...
fn flex_main_axis(facts: &LayoutFacts) -> Option<Axis> {
    if !is_display(facts, "flex") {
        return None;
    }
    let is_column = facts
        .flex_direction
        .as_ref()
        .is_some_and(|direction| direction.to_ascii_lowercase().starts_with("col"));
    Some(if is_column {
        Axis::Vertical
    } else {
        Axis::Horizontal
    })
}

fn close(a: Pixels, b: Pixels) -> bool {
    (a - b).abs() <= TOLERANCE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{TreeBuilder, bounds};
    use gpui::{Edges, size};

    fn width_reason(tree: &ElementTree, ix: ElementIndex) -> SizeReason {
        explain_axis(tree, ix, Axis::Horizontal).unwrap().reason
    }

    fn height_reason(tree: &ElementTree, ix: ElementIndex) -> SizeReason {
        explain_axis(tree, ix, Axis::Vertical).unwrap().reason
    }

    fn width_sentence(tree: &ElementTree, ix: ElementIndex) -> String {
        explain_axis(tree, ix, Axis::Horizontal).unwrap().sentence()
    }

    /// A 628×24 flex row at the origin with facts, under a window root.
    fn row(builder: &mut TreeBuilder) -> ElementIndex {
        let root = builder.root(bounds(0., 0., 1000., 600.));
        let row = builder.child(root, bounds(0., 0., 628., 24.));
        builder.layout(row).flex_direction = Some("row".into());
        row
    }

    #[test]
    fn out_of_range_index_explains_nothing() {
        let tree = TreeBuilder::new().build();
        assert!(explain_size(&tree, 0).is_none());
    }

    #[test]
    fn fixed_width() {
        let mut builder = TreeBuilder::new();
        let row = row(&mut builder);
        let button = builder.child(row, bounds(0., 0., 120., 24.));
        builder.layout(button).size.width = SizeSpec::Pixels(px(120.));
        let tree = builder.build();

        assert_eq!(width_reason(&tree, button), SizeReason::Fixed);
        assert_eq!(
            width_sentence(&tree, button),
            "Fixed: the style sets its width to 120."
        );
    }

    #[test]
    fn fraction_of_the_parents_content_box() {
        let mut builder = TreeBuilder::new();
        let row = row(&mut builder);
        builder.details(row).box_model = Some(BoxModel {
            padding: Edges {
                left: px(14.),
                right: px(14.),
                ..Edges::default()
            },
            ..BoxModel::default()
        });
        let half = builder.child(row, bounds(14., 0., 300., 24.));
        builder.layout(half).size.width = SizeSpec::Fraction(0.5);
        let tree = builder.build();

        assert_eq!(
            width_reason(&tree, half),
            SizeReason::Fraction {
                fraction: 0.5,
                of: px(600.)
            }
        );
        assert_eq!(
            width_sentence(&tree, half),
            "50% of its parent's 600 = 300."
        );
    }

    #[test]
    fn flex_one_grew_into_what_its_siblings_left() {
        let mut builder = TreeBuilder::new();
        let row = row(&mut builder);
        let icon = builder.child(row, bounds(0., 0., 100., 24.));
        let grower = builder.child(row, bounds(100., 0., 396., 24.));
        let label = builder.child(row, bounds(496., 0., 132., 24.));
        builder.layout(icon).size.width = SizeSpec::Pixels(px(100.));
        builder.layout(grower).flex_grow = 1.0;
        builder.layout(label);
        let tree = builder.build();

        assert_eq!(
            width_reason(&tree, grower),
            SizeReason::Grew {
                available: px(628.),
                siblings: px(232.),
                gaps: px(0.),
                margins: px(0.),
                other_growers: 0,
            }
        );
        assert_eq!(
            width_sentence(&tree, grower),
            "Grew into the space left in its row: parent 628 − siblings 232 = 396."
        );
    }

    #[test]
    fn growth_accounts_for_gaps_margins_and_other_growers() {
        let mut builder = TreeBuilder::new();
        let row = row(&mut builder);
        builder.layout(row).gap = size(px(6.), px(0.));
        // Three growers with 6 px gaps; the last has 4 px margins on each side.
        let first = builder.child(row, bounds(0., 0., 200., 24.));
        let second = builder.child(row, bounds(206., 0., 200., 24.));
        let third = builder.child(row, bounds(416., 0., 208., 24.));
        builder.layout(first).flex_grow = 1.0;
        builder.layout(second).flex_grow = 1.0;
        builder.layout(third).flex_grow = 1.0;
        builder.details(third).box_model = Some(BoxModel {
            margin: Edges {
                left: px(4.),
                right: px(4.),
                ..Edges::default()
            },
            ..BoxModel::default()
        });
        let tree = builder.build();

        assert_eq!(
            width_reason(&tree, third),
            SizeReason::Grew {
                available: px(628.),
                siblings: px(400.),
                gaps: px(12.),
                margins: px(8.),
                other_growers: 2,
            }
        );
        assert_eq!(
            width_sentence(&tree, third),
            "Grew into the space left in its row: parent 628 − siblings 400 − gaps 12 \
             − margins 8 = 208. Shared with 2 other growing items."
        );
        // A sibling's margins count towards what the others leave.
        assert_eq!(
            width_reason(&tree, first),
            SizeReason::Grew {
                available: px(628.),
                siblings: px(416.),
                gaps: px(12.),
                margins: px(0.),
                other_growers: 2,
            }
        );
    }

    #[test]
    fn grew_on_a_column_main_axis() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 300., 800.));
        let column = builder.child(root, bounds(0., 0., 300., 800.));
        builder.layout(column).flex_direction = Some("Column".into());
        let header = builder.child(column, bounds(0., 0., 300., 40.));
        let body = builder.child(column, bounds(0., 40., 300., 760.));
        builder.layout(header).size.height = SizeSpec::Pixels(px(40.));
        builder.layout(body).flex_grow = 1.0;
        let tree = builder.build();

        let explanation = explain_axis(&tree, body, Axis::Vertical).unwrap();
        assert!(matches!(explanation.reason, SizeReason::Grew { .. }));
        assert_eq!(
            explanation.sentence(),
            "Grew into the space left in its column: parent 800 − siblings 40 = 760."
        );
        // Its width is the column's: stretched across the cross axis.
        assert_eq!(
            width_reason(&tree, body),
            SizeReason::Stretched {
                container: px(300.)
            }
        );
        assert_eq!(
            width_sentence(&tree, body),
            "Stretched to fill its column's width: 300."
        );
    }

    #[test]
    fn shrunk_below_its_requested_size() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 1000., 600.));
        let row = builder.child(root, bounds(0., 0., 500., 24.));
        builder.layout(row);
        let sidebar = builder.child(row, bounds(0., 0., 260., 24.));
        let panel = builder.child(row, bounds(260., 0., 240., 24.));
        builder.layout(sidebar).size.width = SizeSpec::Pixels(px(260.));
        builder.layout(sidebar).flex_shrink = 0.0;
        builder.layout(panel).size.width = SizeSpec::Pixels(px(300.));
        let tree = builder.build();

        assert_eq!(
            width_reason(&tree, panel),
            SizeReason::Shrunk {
                requested: px(300.),
                overflow: px(60.),
                shrink: 1.0
            }
        );
        assert_eq!(
            width_sentence(&tree, panel),
            "Shrank from 300 to 240: its row was 60 too narrow (flex_shrink 1)."
        );
    }

    #[test]
    fn stretched_on_the_cross_axis() {
        let mut builder = TreeBuilder::new();
        let row = row(&mut builder);
        let cell = builder.child(row, bounds(0., 0., 50., 24.));
        builder.layout(cell).size.width = SizeSpec::Pixels(px(50.));
        let tree = builder.build();

        assert_eq!(
            height_reason(&tree, cell),
            SizeReason::Stretched { container: px(24.) }
        );
        let sentence = explain_axis(&tree, cell, Axis::Vertical)
            .unwrap()
            .sentence();
        assert_eq!(sentence, "Stretched to fill its row's height: 24.");
    }

    #[test]
    fn block_children_fill_the_width() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 400., 600.));
        let block = builder.child(root, bounds(0., 0., 400., 100.));
        builder.layout(block).display = "block".into();
        let paragraph = builder.child(block, bounds(0., 0., 400., 20.));
        builder.layout(paragraph);
        let tree = builder.build();

        assert_eq!(
            width_reason(&tree, paragraph),
            SizeReason::Stretched {
                container: px(400.)
            }
        );
    }

    #[test]
    fn content_sized_by_children_and_padding() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 1000., 600.));
        let toolbar = builder.child(root, bounds(0., 0., 1000., 40.));
        builder.layout(toolbar).flex_direction = Some("row".into());
        let group = builder.child(toolbar, bounds(0., 0., 196., 40.));
        builder.layout(group).flex_direction = Some("row".into());
        builder.details(group).box_model = Some(BoxModel {
            padding: Edges::all(px(8.)),
            ..BoxModel::default()
        });
        builder.child(group, bounds(8., 8., 90., 24.));
        builder.child(group, bounds(98., 8., 90., 24.));
        let tree = builder.build();

        assert_eq!(
            width_reason(&tree, group),
            SizeReason::Content(ContentSource::Children {
                extent: px(180.),
                insets: px(16.)
            })
        );
        assert_eq!(
            width_sentence(&tree, group),
            "Sized by its content: children 180 + padding and border 16 = 196."
        );
    }

    #[test]
    fn content_ignores_absolutely_positioned_children() {
        let mut builder = TreeBuilder::new();
        let row = row(&mut builder);
        let chip = builder.child(row, bounds(0., 0., 60., 24.));
        builder.layout(chip);
        builder.child(chip, bounds(0., 0., 60., 24.));
        let badge = builder.child(chip, bounds(50., -4., 300., 12.));
        builder.layout(badge).absolute = true;
        let tree = builder.build();

        assert_eq!(
            width_sentence(&tree, chip),
            "Sized by its content: its children span 60."
        );
        assert_eq!(width_reason(&tree, badge), SizeReason::Absolute);
    }

    #[test]
    fn text_and_empty_leaves() {
        let mut builder = TreeBuilder::new();
        let row = row(&mut builder);
        let label = builder.child(row, bounds(0., 0., 84., 24.));
        builder.layout(label);
        builder.details(label).text = Some("Ünïcødé ✓".into());
        let spacer = builder.child(row, bounds(84., 0., 0., 24.));
        builder.layout(spacer);
        let tree = builder.build();

        assert_eq!(
            width_reason(&tree, label),
            SizeReason::Content(ContentSource::Text)
        );
        assert_eq!(width_sentence(&tree, label), "Sized by its text: 84.");
        assert_eq!(
            width_reason(&tree, spacer),
            SizeReason::Content(ContentSource::Empty)
        );
    }

    #[test]
    fn min_and_max_clamps() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 1000., 600.));
        let row = builder.child(root, bounds(0., 0., 1000., 24.));
        builder.layout(row);
        let button = builder.child(row, bounds(0., 0., 80., 24.));
        builder.layout(button).min_size.width = SizeSpec::Pixels(px(80.));
        builder.details(button).text = Some("OK".into());
        let reader = builder.child(row, bounds(80., 0., 640., 24.));
        let facts = builder.layout(reader);
        facts.flex_grow = 1.0;
        facts.max_size.width = SizeSpec::Fraction(0.64);
        let tree = builder.build();

        assert_eq!(
            width_reason(&tree, button),
            SizeReason::MinClamped { min: px(80.) }
        );
        assert_eq!(
            width_sentence(&tree, button),
            "Held at its minimum width: 80."
        );
        assert_eq!(
            width_reason(&tree, reader),
            SizeReason::MaxClamped { max: px(640.) }
        );
    }

    #[test]
    fn aspect_ratio_derives_one_axis_from_the_other() {
        let mut builder = TreeBuilder::new();
        let row = row(&mut builder);
        let thumb = builder.child(row, bounds(0., 0., 36., 24.));
        let facts = builder.layout(thumb);
        facts.size.height = SizeSpec::Pixels(px(24.));
        facts.aspect_ratio = Some(1.5);
        let tree = builder.build();

        assert_eq!(
            width_reason(&tree, thumb),
            SizeReason::AspectRatio {
                ratio: 1.5,
                other: px(24.)
            }
        );
        assert_eq!(
            width_sentence(&tree, thumb),
            "Follows its aspect ratio: height 24 × 1.5 = 36."
        );
        assert_eq!(height_reason(&tree, thumb), SizeReason::Fixed);
    }

    #[test]
    fn window_root_fills_the_viewport() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 1280., 800.));
        let tooltip = builder.root(bounds(10., 10., 0., 0.));
        builder.record(tooltip).flags = ElementFlags::DEFERRED;
        let tree = builder.build();

        let explanation = explain_size(&tree, root).unwrap();
        assert_eq!(explanation.width.reason, SizeReason::ViewportRoot);
        assert_eq!(
            explanation.width.sentence(),
            "The window root: it fills the app area, 1280 wide."
        );
        assert_eq!(
            explanation.height.sentence(),
            "The window root: it fills the app area, 800 tall."
        );
        assert_eq!(width_reason(&tree, tooltip), SizeReason::NotCaptured);
    }

    #[test]
    fn missing_facts_are_admitted() {
        let mut builder = TreeBuilder::new();
        let row = row(&mut builder);
        let unknown = builder.child(row, bounds(0., 0., 10., 10.));
        let tree = builder.build();

        assert_eq!(width_reason(&tree, unknown), SizeReason::NotCaptured);
        assert_eq!(
            width_sentence(&tree, unknown),
            "Sized by layout; details not captured."
        );
    }

    #[test]
    fn unexplained_sizes_say_so() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 1000., 600.));
        let grid = builder.child(root, bounds(0., 0., 600., 300.));
        builder.layout(grid).display = "grid".into();
        let cell = builder.child(grid, bounds(0., 0., 123., 45.));
        builder.layout(cell);
        builder.child(cell, bounds(0., 0., 20., 20.));
        let tree = builder.build();

        assert_eq!(width_reason(&tree, cell), SizeReason::Unexplained);
        assert_eq!(
            width_sentence(&tree, cell),
            "Sized by layout: no single rule explains 123 \
             (margins, alignment or grid placement may apply)."
        );
    }

    #[test]
    fn zero_and_huge_sizes_stay_finite() {
        let mut builder = TreeBuilder::new();
        let row = row(&mut builder);
        let huge = builder.child(row, bounds(0., 0., 1e9, 0.));
        let facts = builder.layout(huge);
        facts.size.width = SizeSpec::Pixels(px(1e9));
        facts.aspect_ratio = Some(0.0);
        let tree = builder.build();

        let explanation = explain_size(&tree, huge).unwrap();
        assert_eq!(explanation.width.reason, SizeReason::Fixed);
        assert_eq!(
            explanation.height.reason,
            SizeReason::Content(ContentSource::Empty)
        );
        assert!(explanation.width.sentence().contains("1000000000"));
    }
}
