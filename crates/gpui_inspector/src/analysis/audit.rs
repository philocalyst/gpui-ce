//! Audit: continuous checks over the latest element tree and recent frames,
//! answering "what should I fix first?".
//!
//! Each check is a [`Rule`]; [`default_rules`] lists Loupe's. Per-element
//! findings are folded by construction site, so a list of 500 rows built at
//! one `div()` call yields one finding with `instances: 500`, pointing at the
//! first row.

use super::{
    Severity, contrast, format,
    insights::{self, HOT_VIEW_RENDERS},
};
use gpui::{
    EntityId,
    inspector::{
        ElementFlags, ElementIndex, ElementKey, ElementRecord, ElementTree, FrameRecord, PathKey,
    },
};
use std::{
    cmp::Reverse,
    collections::{HashMap, VecDeque},
    panic::Location,
    time::Duration,
};

/// Flags that make an element respond to the pointer.
const POINTER_TARGET: ElementFlags = ElementFlags::HITBOX
    .union(ElementFlags::CLICKABLE)
    .union(ElementFlags::DRAG_DROP)
    .union(ElementFlags::TOOLTIP);
/// Flags that make an element interactive for assistive technology.
const INTERACTIVE: ElementFlags = ElementFlags::CLICKABLE
    .union(ElementFlags::FOCUSABLE)
    .union(ElementFlags::TAB_STOP);
/// A view render longer than this share of the budget is expensive.
pub const EXPENSIVE_RENDER_BUDGET_SHARE: f64 = 0.5;
/// Quoted text in messages is cut to this many characters.
const QUOTE_CHARS: usize = 32;

/// What the rules read.
pub struct AuditInput<'a> {
    /// The latest retained element tree, if any.
    pub tree: Option<&'a ElementTree>,
    /// Recorded frames, oldest first.
    pub frames: &'a VecDeque<FrameRecord>,
    /// Frame budget.
    pub budget: Duration,
    /// Where the element at an interned path was constructed, e.g.
    /// `&|path| capture.path_info(path).map(|info| info.source)`.
    pub source: &'a dyn Fn(PathKey) -> Option<&'static Location<'static>>,
}

impl AuditInput<'_> {
    fn site(&self, key: Option<ElementKey>) -> Option<&'static Location<'static>> {
        (self.source)(key?.path)
    }
}

/// One thing worth fixing.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// How much it matters.
    pub severity: Severity,
    /// The [`Rule::id`] that found it.
    pub rule: &'static str,
    /// The finding in a few words.
    pub title: String,
    /// Why it matters and what to do, in a sentence or two.
    pub detail: String,
    /// The (first) element concerned.
    pub element: Option<ElementKey>,
    /// The entity concerned (view rules).
    pub entity: Option<EntityId>,
    /// Where the element was constructed.
    pub site: Option<&'static Location<'static>>,
    /// How many elements built at the same site have the same problem.
    pub instances: u32,
}

/// One audit check.
pub trait Rule {
    /// Stable identifier, e.g. `keyboard-access`.
    fn id(&self) -> &'static str;
    /// Appends what it finds.
    fn check(&self, cx: &AuditInput, findings: &mut Vec<Finding>);
}

/// Loupe's rules, in the order their findings are listed within a severity.
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(DuplicateIds),
        Box::new(KeyboardAccess),
        Box::new(TextContrast),
        Box::new(MissingAccessibleName),
        Box::new(DeadHitbox),
        Box::new(ExpensiveRender),
        Box::new(RenderHotSpot),
        Box::new(UnclippedOverflow),
    ]
}

/// Runs `rules`, most severe findings first (rule order within a severity).
pub fn audit(cx: &AuditInput, rules: &[Box<dyn Rule>]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for rule in rules {
        rule.check(cx, &mut findings);
    }
    findings.sort_by_key(|finding| Reverse(finding.severity));
    findings
}

/// Collects one rule's per-element findings, folding elements built at the
/// same site into a single finding.
struct PerSite<'a, 'b> {
    cx: &'a AuditInput<'b>,
    rule: &'static str,
    findings: &'a mut Vec<Finding>,
    by_path: HashMap<PathKey, usize>,
}

impl<'a, 'b> PerSite<'a, 'b> {
    fn new(cx: &'a AuditInput<'b>, rule: &'static str, findings: &'a mut Vec<Finding>) -> Self {
        Self {
            cx,
            rule,
            findings,
            by_path: HashMap::new(),
        }
    }

    /// Reports `record`; `describe` (title, detail) runs only for the first
    /// element of each site.
    fn report(
        &mut self,
        record: &ElementRecord,
        severity: Severity,
        describe: impl FnOnce() -> (String, String),
    ) {
        let path = record.key.map(|key| key.path);
        if let Some(&existing) = path.and_then(|path| self.by_path.get(&path)) {
            self.findings[existing].instances += 1;
            return;
        }
        let (title, detail) = describe();
        if let Some(path) = path {
            self.by_path.insert(path, self.findings.len());
        }
        self.findings.push(Finding {
            severity,
            rule: self.rule,
            title,
            detail,
            element: record.key,
            entity: None,
            site: self.cx.site(record.key),
            instances: 1,
        });
    }
}

/// Clickable elements that keyboard users cannot focus.
pub struct KeyboardAccess;

impl Rule for KeyboardAccess {
    fn id(&self) -> &'static str {
        "keyboard-access"
    }

    fn check(&self, cx: &AuditInput, findings: &mut Vec<Finding>) {
        let Some(tree) = cx.tree else { return };
        let mut report = PerSite::new(cx, self.id(), findings);
        for record in &tree.elements {
            let flags = record.flags;
            if flags.contains(ElementFlags::CLICKABLE)
                && !flags.intersects(ElementFlags::FOCUSABLE | ElementFlags::TAB_STOP)
            {
                report.report(record, Severity::Warning, || {
                    (
                        "Clickable but not reachable by keyboard".to_string(),
                        format!(
                            "{} handles clicks but has no focus handle or tab stop, so keyboard \
                             users cannot reach it. Add .track_focus(&focus_handle) and \
                             .tab_index(0), and bind its action to a key.",
                            format::element_label(record)
                        ),
                    )
                });
            }
        }
    }
}

/// Text below the WCAG AA contrast minimum against its background.
pub struct TextContrast;

impl Rule for TextContrast {
    fn id(&self) -> &'static str {
        "text-contrast"
    }

    fn check(&self, cx: &AuditInput, findings: &mut Vec<Finding>) {
        let Some(tree) = cx.tree else { return };
        let mut report = PerSite::new(cx, self.id(), findings);
        for (ix, record) in tree.elements.iter().enumerate() {
            let Some(details) = record.details.as_deref() else {
                continue;
            };
            let (Some(text), Some(text_color)) = (&details.text, details.text_color) else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            let backgrounds = std::iter::once(ix as ElementIndex)
                .chain(tree.ancestors(ix as ElementIndex))
                .filter_map(|ix| tree.get(ix)?.details.as_ref()?.background);
            let Some(background) = contrast::composite(backgrounds) else {
                continue;
            };
            let foreground = background.under(text_color);
            let ratio = contrast::contrast_ratio(foreground, background);
            let required = contrast::required_ratio(details.font_size);
            if ratio >= required {
                continue;
            }
            report.report(record, Severity::Warning, || {
                (
                    format!("Low text contrast: {ratio:.1}:1"),
                    format!(
                        "“{}” is {} on {}: {ratio:.1}:1, below the WCAG AA minimum of \
                         {}:1 for text this size. Darken the text or lighten the background \
                         (or the reverse).",
                        quote(text),
                        foreground.hex(),
                        background.hex(),
                        format::number(required)
                    ),
                )
            });
        }
    }
}

/// Interactive elements with neither text nor an accessibility label.
pub struct MissingAccessibleName;

impl Rule for MissingAccessibleName {
    fn id(&self) -> &'static str {
        "accessible-name"
    }

    fn check(&self, cx: &AuditInput, findings: &mut Vec<Finding>) {
        let Some(tree) = cx.tree else { return };
        let names = named_subtrees(tree);
        let mut report = PerSite::new(cx, self.id(), findings);
        for (record, name) in tree.elements.iter().zip(&names) {
            if record.flags.intersects(INTERACTIVE) && *name == Named::No {
                report.report(record, Severity::Warning, || {
                    (
                        "Interactive element has no accessible name".to_string(),
                        format!(
                            "{} can be clicked or focused but has no text and no accessibility \
                             label, so assistive technology announces nothing. Add \
                             .aria_label(\"…\").",
                            format::element_label(record)
                        ),
                    )
                });
            }
        }
    }
}

/// Whether an element's subtree gives it a name (text or an accessibility label).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Named {
    Yes,
    No,
    /// Some element in the subtree reported no details, so text may be missing from the capture.
    Unknown,
}

/// [`Named`] for every element, computed bottom-up in one pass (parents
/// precede children in the tree).
fn named_subtrees(tree: &ElementTree) -> Vec<Named> {
    let mut names = vec![Named::No; tree.elements.len()];
    for ix in (0..tree.elements.len()).rev() {
        let own = match tree.elements[ix].details.as_deref() {
            None => Named::Unknown,
            Some(details) => {
                let non_empty = |text: &Option<gpui::SharedString>| {
                    text.as_ref().is_some_and(|text| !text.trim().is_empty())
                };
                if non_empty(&details.text) || non_empty(&details.a11y_label) {
                    Named::Yes
                } else {
                    Named::No
                }
            }
        };
        names[ix] = tree
            .children(ix as ElementIndex)
            .iter()
            .filter_map(|&child| names.get(child as usize).copied())
            .fold(own, |name, child| match (name, child) {
                (Named::Yes, _) | (_, Named::Yes) => Named::Yes,
                (Named::Unknown, _) | (_, Named::Unknown) => Named::Unknown,
                (Named::No, Named::No) => Named::No,
            });
    }
    names
}

/// Pointer targets that can never be hit: zero-size, or clipped away by an
/// ancestor that does not scroll.
pub struct DeadHitbox;

impl Rule for DeadHitbox {
    fn id(&self) -> &'static str {
        "dead-hitbox"
    }

    fn check(&self, cx: &AuditInput, findings: &mut Vec<Finding>) {
        let Some(tree) = cx.tree else { return };
        let mut report = PerSite::new(cx, self.id(), findings);
        for (ix, record) in tree.elements.iter().enumerate() {
            if !record.flags.intersects(POINTER_TARGET) {
                continue;
            }
            let size = record.bounds.size;
            let why = if size.width <= gpui::Pixels::ZERO || size.height <= gpui::Pixels::ZERO {
                format!("zero-size ({})", format::size(size))
            } else if is_clipped(record) && !scrolls_into_view(tree, ix as ElementIndex) {
                "clipped away entirely by an ancestor".to_string()
            } else {
                continue;
            };
            report.report(record, Severity::Warning, || {
                (
                    "Pointer target can never be hit".to_string(),
                    format!(
                        "{} listens for the pointer but is {why}, so it never receives \
                         input. Give it a size, or remove the listeners if it is dead.",
                        format::element_label(record)
                    ),
                )
            });
        }
    }
}

fn is_clipped(record: &ElementRecord) -> bool {
    record.visible_bounds.is_none() || record.flags.contains(ElementFlags::CLIPPED)
}

/// Whether an ancestor scrolls, so the element may just be scrolled out of view.
fn scrolls_into_view(tree: &ElementTree, ix: ElementIndex) -> bool {
    tree.ancestors(ix).any(|ancestor| {
        tree.get(ancestor)
            .is_some_and(|record| record.flags.contains(ElementFlags::SCROLLABLE))
    })
}

/// Content that spills out of a parent that does not clip it.
pub struct UnclippedOverflow;

impl Rule for UnclippedOverflow {
    fn id(&self) -> &'static str {
        "overflow"
    }

    fn check(&self, cx: &AuditInput, findings: &mut Vec<Finding>) {
        let Some(tree) = cx.tree else { return };
        let mut report = PerSite::new(cx, self.id(), findings);
        for record in &tree.elements {
            if !record.flags.contains(ElementFlags::OVERFLOWS_PARENT) {
                continue;
            }
            let parent = record.parent.and_then(|parent| tree.get(parent));
            report.report(record, Severity::Info, || {
                let parent = parent.map_or_else(
                    || "its parent".to_string(),
                    |parent| {
                        format!(
                            "{} ({})",
                            format::element_label(parent),
                            format::size(parent.bounds.size)
                        )
                    },
                );
                (
                    "Overflows its parent".to_string(),
                    format!(
                        "{} ({}) extends past {parent}, which does not clip it, so it may paint \
                         over its neighbours. Clip the parent with .overflow_hidden(), let it \
                         scroll, or constrain the child.",
                        format::element_label(record),
                        format::size(record.bounds.size)
                    ),
                )
            });
        }
    }
}

/// Views that rendered very often in the last second.
pub struct RenderHotSpot;

impl Rule for RenderHotSpot {
    fn id(&self) -> &'static str {
        "render-hot-spot"
    }

    fn check(&self, cx: &AuditInput, findings: &mut Vec<Finding>) {
        let recent = insights::recent_app_frames(cx.frames);
        for view in insights::view_activity(recent) {
            if view.rendered < HOT_VIEW_RENDERS {
                break;
            }
            findings.push(Finding {
                severity: Severity::Warning,
                rule: self.id(),
                title: format!(
                    "Render hot spot: {} rendered {}× in the last second",
                    format::type_name(view.type_name),
                    view.rendered
                ),
                detail: "Each render rebuilds its whole subtree. Find what notifies it in \
                         Entities, or move the changing part into a smaller view."
                    .to_string(),
                element: view.element,
                entity: Some(view.entity),
                site: cx.site(view.element),
                instances: 1,
            });
        }
    }
}

/// Views whose render took more than half the frame budget.
pub struct ExpensiveRender;

impl Rule for ExpensiveRender {
    fn id(&self) -> &'static str {
        "expensive-render"
    }

    fn check(&self, cx: &AuditInput, findings: &mut Vec<Finding>) {
        let threshold = cx.budget.mul_f64(EXPENSIVE_RENDER_BUDGET_SHARE);
        let mut expensive: Vec<insights::ViewActivity> = insights::view_activity(cx.frames)
            .into_iter()
            .filter(|view| view.slowest > threshold)
            .collect();
        expensive.sort_by_key(|view| Reverse(view.slowest));
        for view in expensive {
            findings.push(Finding {
                severity: if view.slowest > cx.budget {
                    Severity::Critical
                } else {
                    Severity::Warning
                },
                rule: self.id(),
                title: format!(
                    "Expensive render: {} took {}",
                    format::type_name(view.type_name),
                    format::duration(view.slowest)
                ),
                detail: format!(
                    "Its render (with its subtree's layout requests) took {} in frame #{}, \
                     more than half the {} budget. Cache it, split it, or virtualize long \
                     content.",
                    format::duration(view.slowest),
                    view.slowest_frame,
                    format::duration(cx.budget)
                ),
                element: view.element,
                entity: Some(view.entity),
                site: cx.site(view.element),
                instances: 1,
            });
        }
    }
}

/// Siblings sharing an element id, which makes them share element state.
pub struct DuplicateIds;

impl Rule for DuplicateIds {
    fn id(&self) -> &'static str {
        "duplicate-id"
    }

    fn check(&self, cx: &AuditInput, findings: &mut Vec<Finding>) {
        let Some(tree) = cx.tree else { return };
        let mut report = PerSite::new(cx, self.id(), findings);
        let mut seen: HashMap<&str, (ElementIndex, u32)> = HashMap::new();
        for (parent, parent_record) in tree.elements.iter().enumerate() {
            seen.clear();
            for &child in tree.children(parent as ElementIndex) {
                let Some(id) = tree.get(child).and_then(|record| record.id.as_deref()) else {
                    continue;
                };
                seen.entry(id).or_insert((child, 0)).1 += 1;
            }
            let mut duplicates: Vec<(&str, ElementIndex, u32)> = seen
                .iter()
                .filter(|(_, (_, count))| *count > 1)
                .map(|(id, (first, count))| (*id, *first, *count))
                .collect();
            duplicates.sort_by_key(|(_, first, _)| *first);
            for (id, first, count) in duplicates {
                let Some(record) = tree.get(first) else {
                    continue;
                };
                report.report(record, Severity::Critical, || {
                    (
                        format!("Duplicate id \"{id}\" among {count} siblings"),
                        format!(
                            "{count} children of {} share the id \"{id}\", so they share \
                             element state (scroll, hover, focus, animation). Give each a \
                             unique id, e.g. .id((\"{id}\", ix)).",
                            format::element_label(parent_record)
                        ),
                    )
                });
            }
        }
    }
}

/// `text`, cut to [`QUOTE_CHARS`] characters.
fn quote(text: &str) -> String {
    let mut chars = text.chars();
    let quoted: String = chars.by_ref().take(QUOTE_CHARS).collect();
    if chars.next().is_some() {
        format!("{quoted}…")
    } else {
        quoted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{
        TreeBuilder, bounds, cached_view, frame, frames_every, ms, view,
    };
    use gpui::{Hsla, rgb, rgb_to_hsla};

    const BUDGET: Duration = Duration::from_micros(16_667);

    fn color(hex: u32) -> Hsla {
        rgb_to_hsla(rgb(hex))
    }

    fn run_rule(
        rule: &dyn Rule,
        tree: Option<&ElementTree>,
        frames: &VecDeque<FrameRecord>,
    ) -> Vec<Finding> {
        let source = |_: PathKey| Some(Location::caller());
        let cx = AuditInput {
            tree,
            frames,
            budget: BUDGET,
            source: &source,
        };
        let mut findings = Vec::new();
        rule.check(&cx, &mut findings);
        findings
    }

    fn check_tree(rule: &dyn Rule, tree: &ElementTree) -> Vec<Finding> {
        run_rule(rule, Some(tree), &VecDeque::new())
    }

    fn check_frames(rule: &dyn Rule, frames: Vec<FrameRecord>) -> Vec<Finding> {
        run_rule(rule, None, &frames.into())
    }

    /// A root with one 100×24 child; returns the tree builder and the child.
    fn one_child() -> (TreeBuilder, ElementIndex) {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 800., 600.));
        let child = builder.child(root, bounds(10., 10., 100., 24.));
        (builder, child)
    }

    #[test]
    fn rules_without_input_find_nothing() {
        for rule in default_rules() {
            assert!(
                run_rule(rule.as_ref(), None, &VecDeque::new()).is_empty(),
                "{}",
                rule.id()
            );
        }
    }

    #[test]
    fn rule_ids_are_unique() {
        let mut ids: Vec<&str> = default_rules().iter().map(|rule| rule.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), default_rules().len());
    }

    #[test]
    fn keyboard_access() {
        let (mut builder, button) = one_child();
        builder.record(button).flags = ElementFlags::CLICKABLE;
        builder.record(button).id = Some("save".into());
        let tree = builder.build();
        let findings = check_tree(&KeyboardAccess, &tree);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "keyboard-access");
        assert_eq!(findings[0].element, tree.elements[button as usize].key);
        assert!(findings[0].detail.starts_with("div#save handles clicks"));
        assert!(findings[0].site.is_some());

        for reachable in [ElementFlags::FOCUSABLE, ElementFlags::TAB_STOP] {
            let (mut builder, button) = one_child();
            builder.record(button).flags = ElementFlags::CLICKABLE | reachable;
            assert!(check_tree(&KeyboardAccess, &builder.build()).is_empty());
        }
    }

    #[test]
    fn findings_fold_elements_built_at_the_same_site() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 800., 600.));
        for row in 0..3 {
            let ix = builder.child(root, bounds(0., row as f32 * 20., 100., 20.));
            builder.record(ix).flags = ElementFlags::CLICKABLE;
            builder.record(ix).key = Some(ElementKey {
                path: PathKey(99),
                instance: row,
            });
        }
        let findings = check_tree(&KeyboardAccess, &builder.build());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].instances, 3);
        assert_eq!(findings[0].element.map(|key| key.instance), Some(0));
    }

    fn text_on(builder: &mut TreeBuilder, ix: ElementIndex, text: &str, fg: Hsla) {
        let details = builder.details(ix);
        details.text = Some(text.to_string().into());
        details.text_color = Some(fg);
    }

    #[test]
    fn text_contrast() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 800., 600.));
        builder.details(root).background = Some(color(0xffffff));
        let faint = builder.child(root, bounds(0., 0., 100., 20.));
        text_on(
            &mut builder,
            faint,
            "Last updated 2 minutes ago, by someone",
            color(0xaaaaaa),
        );
        let fine = builder.child(root, bounds(0., 20., 100., 20.));
        text_on(&mut builder, fine, "OK", color(0x595959));
        let big = builder.child(root, bounds(0., 40., 100., 40.));
        text_on(&mut builder, big, "Title", color(0x949494));
        builder.details(big).font_size = Some(gpui::px(28.));
        let tree = builder.build();

        let findings = check_tree(&TextContrast, &tree);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].title, "Low text contrast: 2.3:1");
        assert_eq!(
            findings[0].detail,
            "“Last updated 2 minutes ago, by s…” is #aaaaaa on #ffffff: 2.3:1, below the \
             WCAG AA minimum of 4.5:1 for text this size. Darken the text or lighten the \
             background (or the reverse)."
        );
    }

    #[test]
    fn text_contrast_composites_translucent_colors() {
        let (mut builder, label) = one_child();
        builder.details(0).background = Some(color(0x000000));
        builder.details(label).background = Some(gpui::hsla(0., 0., 1., 0.1));
        // Opaque-looking white text at 20% alpha over a near-black background.
        text_on(&mut builder, label, "dim", gpui::hsla(0., 0., 1., 0.2));
        let findings = check_tree(&TextContrast, &builder.build());
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].detail.contains("#1a1a1a"),
            "{}",
            findings[0].detail
        );
    }

    #[test]
    fn text_contrast_needs_a_known_background() {
        let (mut builder, label) = one_child();
        text_on(
            &mut builder,
            label,
            "no background anywhere",
            color(0xeeeeee),
        );
        assert!(check_tree(&TextContrast, &builder.build()).is_empty());
    }

    #[test]
    fn accessible_name() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 800., 600.));
        builder.details(root);
        let icon_button = builder.child(root, bounds(0., 0., 24., 24.));
        builder.record(icon_button).flags = ElementFlags::CLICKABLE | ElementFlags::FOCUSABLE;
        builder.details(icon_button);
        let svg = builder.child(icon_button, bounds(4., 4., 16., 16.));
        builder.details(svg).source = Some("icons/close.svg".into());

        let text_button = builder.child(root, bounds(30., 0., 60., 24.));
        builder.record(text_button).flags = ElementFlags::CLICKABLE | ElementFlags::FOCUSABLE;
        builder.details(text_button);
        let label = builder.child(text_button, bounds(30., 0., 60., 24.));
        builder.details(label).text = Some("Save".into());

        let labelled = builder.child(root, bounds(100., 0., 24., 24.));
        builder.record(labelled).flags = ElementFlags::TAB_STOP;
        builder.details(labelled).a11y_label = Some("Close".into());

        let unknown = builder.child(root, bounds(130., 0., 24., 24.));
        builder.record(unknown).flags = ElementFlags::CLICKABLE;
        builder.details(unknown);
        builder.child(unknown, bounds(130., 0., 24., 24.));
        let tree = builder.build();

        let findings = check_tree(&MissingAccessibleName, &tree);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].element, tree.elements[icon_button as usize].key);
        assert_eq!(
            findings[0].title,
            "Interactive element has no accessible name"
        );
    }

    #[test]
    fn dead_hitbox() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 800., 600.));
        let zero = builder.child(root, bounds(0., 0., 0., 24.));
        builder.record(zero).flags = ElementFlags::HITBOX | ElementFlags::CLICKABLE;
        let clipped = builder.child(root, bounds(0., 900., 50., 24.));
        builder.record(clipped).flags = ElementFlags::TOOLTIP | ElementFlags::CLIPPED;
        builder.record(clipped).visible_bounds = None;
        let scroller = builder.child(root, bounds(0., 0., 200., 200.));
        builder.record(scroller).flags = ElementFlags::SCROLLABLE;
        let scrolled_away = builder.child(scroller, bounds(0., 400., 200., 24.));
        builder.record(scrolled_away).flags = ElementFlags::HITBOX | ElementFlags::CLIPPED;
        builder.record(scrolled_away).visible_bounds = None;
        let decorative = builder.child(root, bounds(0., 0., 0., 0.));
        builder.record(decorative).flags = ElementFlags::empty();
        let tree = builder.build();

        let findings = check_tree(&DeadHitbox, &tree);
        let details: Vec<&str> = findings
            .iter()
            .map(|finding| finding.detail.as_str())
            .collect();
        assert_eq!(findings.len(), 2, "{details:?}");
        assert!(details[0].contains("is zero-size (0×24)"));
        assert!(details[1].contains("clipped away entirely by an ancestor"));
    }

    #[test]
    fn unclipped_overflow() {
        let (mut builder, child) = one_child();
        builder.record(child).flags = ElementFlags::OVERFLOWS_PARENT;
        builder.record(child).bounds = bounds(700., 0., 300., 24.);
        let findings = check_tree(&UnclippedOverflow, &builder.build());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Info);
        assert!(
            findings[0]
                .detail
                .starts_with("div (300×24) extends past div (800×600)")
        );

        let (builder, _) = one_child();
        assert!(check_tree(&UnclippedOverflow, &builder.build()).is_empty());
    }

    #[test]
    fn render_hot_spot() {
        let mut frames = frames_every(60, Duration::from_micros(16_667), ms(2.0));
        for frame in &mut frames {
            frame.views = vec![view(3, "app::Clock", 0, ms(0.0), ms(0.1))];
        }
        let findings = check_frames(&RenderHotSpot, frames.clone());
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].title,
            "Render hot spot: Clock rendered 60× in the last second"
        );

        for frame in frames.iter_mut().skip(1).step_by(2) {
            frame.views = vec![cached_view(3, "app::Clock", 0, ms(0.0), ms(0.01))];
        }
        frames[0].views.clear();
        assert!(
            check_frames(&RenderHotSpot, frames).is_empty(),
            "29 renders"
        );
    }

    #[test]
    fn expensive_render() {
        let mut frames = vec![frame(0, ms(0.0), ms(30.0)), frame(1, ms(20.0), ms(30.0))];
        frames[0].views = vec![
            view(1, "app::Chart", 0, ms(0.0), ms(9.0)),
            view(2, "app::Legend", 0, ms(9.0), ms(8.0)),
        ];
        frames[1].views = vec![view(2, "app::Legend", 0, ms(0.0), ms(20.0))];
        let findings = check_frames(&ExpensiveRender, frames);
        let titles: Vec<&str> = findings
            .iter()
            .map(|finding| finding.title.as_str())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Expensive render: Legend took 20.0 ms",
                "Expensive render: Chart took 9.0 ms"
            ]
        );
        assert_eq!(findings[0].severity, Severity::Critical);
        assert_eq!(findings[1].severity, Severity::Warning);
        assert!(findings[0].detail.contains("in frame #1"));

        let cheap = vec![frame(0, ms(0.0), ms(10.0))];
        assert!(check_frames(&ExpensiveRender, cheap).is_empty());
    }

    #[test]
    fn duplicate_ids_among_siblings() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 800., 600.));
        let list = builder.child(root, bounds(0., 0., 800., 600.));
        builder.record(list).id = Some("list".into());
        for (ix, id) in ["row", "row", "other", "row"].into_iter().enumerate() {
            let child = builder.child(list, bounds(0., ix as f32 * 20., 800., 20.));
            builder.record(child).id = Some(id.into());
        }
        // Same id under different parents is fine.
        let elsewhere = builder.child(root, bounds(0., 0., 10., 10.));
        builder.record(elsewhere).id = Some("row".into());
        let tree = builder.build();

        let findings = check_tree(&DuplicateIds, &tree);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Duplicate id \"row\" among 3 siblings");
        assert_eq!(findings[0].severity, Severity::Critical);
        assert!(
            findings[0]
                .detail
                .starts_with("3 children of div#list share the id \"row\"")
        );
    }

    #[test]
    fn audit_orders_by_severity() {
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 800., 600.));
        let a = builder.child(root, bounds(0., 0., 10., 10.));
        let b = builder.child(root, bounds(0., 0., 10., 10.));
        builder.record(a).flags = ElementFlags::OVERFLOWS_PARENT;
        builder.record(a).id = Some("same".into());
        builder.record(b).id = Some("same".into());
        let tree = builder.build();
        let frames = VecDeque::new();
        let source = |_: PathKey| None;
        let cx = AuditInput {
            tree: Some(&tree),
            frames: &frames,
            budget: BUDGET,
            source: &source,
        };
        let findings = audit(&cx, &default_rules());
        let rules: Vec<&str> = findings.iter().map(|finding| finding.rule).collect();
        assert_eq!(rules, vec!["duplicate-id", "overflow"]);
    }

    #[test]
    fn quotes_are_cut_on_character_boundaries() {
        assert_eq!(quote("short"), "short");
        let long = "ü".repeat(40);
        assert_eq!(quote(&long), format!("{}…", "ü".repeat(32)));
    }
}
