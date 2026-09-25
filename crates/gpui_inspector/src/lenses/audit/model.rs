//! What the Audit lens says, as plain data: findings grouped by rule,
//! counts by severity, a one-line verdict, which checks could not run, and
//! keyboard navigation through the list. Unit-tested pure functions.

use crate::analysis::{Severity, audit::Finding, format};
use gpui::{
    EntityId,
    inspector::{CaptureLevel, ElementKey},
};

/// A rule's name as a heading.
pub(crate) fn rule_name(id: &'static str) -> &'static str {
    match id {
        "duplicate-id" => "Duplicate ids",
        "keyboard-access" => "Keyboard access",
        "text-contrast" => "Text contrast",
        "accessible-name" => "Accessible names",
        "dead-hitbox" => "Dead pointer targets",
        "expensive-render" => "Expensive renders",
        "render-hot-spot" => "Render hot spots",
        "overflow" => "Overflow",
        other => other,
    }
}

/// Which severities the list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Filter {
    /// Everything.
    #[default]
    All,
    /// Only this severity.
    Only(Severity),
}

impl Filter {
    /// The filters in the order the control shows them.
    pub const ALL: [Filter; 4] = [
        Filter::All,
        Filter::Only(Severity::Critical),
        Filter::Only(Severity::Warning),
        Filter::Only(Severity::Info),
    ];

    /// Whether `finding` passes.
    pub fn admits(self, finding: &Finding) -> bool {
        match self {
            Filter::All => true,
            Filter::Only(severity) => finding.severity == severity,
        }
    }

    /// `Critical 2`.
    pub fn label(self, counts: &Counts) -> String {
        match self {
            Filter::All => format!("All {}", counts.total()),
            Filter::Only(Severity::Critical) => format!("Critical {}", counts.critical),
            Filter::Only(Severity::Warning) => format!("Warnings {}", counts.warning),
            Filter::Only(Severity::Info) => format!("Notes {}", counts.info),
        }
    }
}

/// Findings per severity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Critical findings.
    pub critical: usize,
    /// Warnings.
    pub warning: usize,
    /// Notes.
    pub info: usize,
}

impl Counts {
    /// Counts `findings` by severity.
    pub fn of(findings: &[Finding]) -> Self {
        findings
            .iter()
            .fold(Self::default(), |mut counts, finding| {
                match finding.severity {
                    Severity::Critical => counts.critical += 1,
                    Severity::Warning => counts.warning += 1,
                    Severity::Info => counts.info += 1,
                }
                counts
            })
    }

    /// Every finding.
    pub fn total(&self) -> usize {
        self.critical + self.warning + self.info
    }
}

/// The verdict at the top of the lens.
pub(crate) fn health(counts: &Counts, elements: usize, frames: usize) -> String {
    let plural = |count: usize, one: &str, many: &str| {
        format!(
            "{} {}",
            format::count(count as u64),
            if count == 1 { one } else { many }
        )
    };
    if counts.critical > 0 {
        format!(
            "{} to fix first",
            plural(counts.critical, "critical problem", "critical problems")
        )
    } else if counts.warning > 0 {
        format!(
            "{} worth fixing",
            plural(counts.warning, "problem", "problems")
        )
    } else if counts.info > 0 {
        "Nothing broken, a few notes".to_string()
    } else if frames == 0 {
        "Nothing recorded yet: the checks run as the app draws".to_string()
    } else if elements == 0 {
        format!("Nothing to fix in {}", plural(frames, "frame", "frames"))
    } else {
        format!(
            "Nothing to fix: {} and {} pass every check",
            plural(elements, "element", "elements"),
            plural(frames, "frame", "frames")
        )
    }
}

/// The checks the current capture cannot run, in a sentence.
pub(crate) fn blind_spots(level: CaptureLevel, has_tree: bool) -> Option<&'static str> {
    match (level, has_tree) {
        (CaptureLevel::Frames, _) => Some(
            "Element checks need a retained element tree: set the capture level to Tree or Full.",
        ),
        (_, false) => Some("Element checks run on the next element tree the app draws."),
        (CaptureLevel::Tree, true) => Some(
            "Contrast and accessible-name checks need element details: set the capture level \
             to Full.",
        ),
        (CaptureLevel::Full, true) => None,
    }
}

/// The findings of one rule.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Group {
    /// The rule's id.
    pub rule: &'static str,
    /// Its worst finding's severity.
    pub severity: Severity,
    /// Indices into the findings, in their order.
    pub findings: Vec<usize>,
    /// Elements concerned across the group.
    pub instances: u32,
}

/// Groups the findings `filter` admits by rule: the worst groups first,
/// then in the order the rules reported.
pub(crate) fn group(findings: &[Finding], filter: Filter) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for (ix, finding) in findings.iter().enumerate() {
        if !filter.admits(finding) {
            continue;
        }
        match groups.iter_mut().find(|group| group.rule == finding.rule) {
            Some(group) => {
                group.findings.push(ix);
                group.severity = group.severity.max(finding.severity);
                group.instances += finding.instances;
            }
            None => groups.push(Group {
                rule: finding.rule,
                severity: finding.severity,
                findings: vec![ix],
                instances: finding.instances,
            }),
        }
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.severity));
    groups
}

/// Identifies a finding across recomputations, so the selection survives
/// new frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FindingKey {
    rule: &'static str,
    element: Option<ElementKey>,
    entity: Option<EntityId>,
}

impl FindingKey {
    /// The key of `finding`.
    pub fn of(finding: &Finding) -> Self {
        Self {
            rule: finding.rule,
            element: finding.element,
            entity: finding.entity,
        }
    }

    /// The finding with this key in `findings`.
    pub fn find(self, findings: &[Finding]) -> Option<usize> {
        findings
            .iter()
            .position(|finding| Self::of(finding) == self)
    }
}

/// The finding `delta` rows from `current` in the list's order, clamped;
/// without a selection, down picks the first and up the last.
pub(crate) fn step(groups: &[Group], current: Option<usize>, delta: isize) -> Option<usize> {
    let order: Vec<usize> = groups
        .iter()
        .flat_map(|group| group.findings.iter().copied())
        .collect();
    let last = order.len().checked_sub(1)?;
    let row = match current.and_then(|current| order.iter().position(|&ix| ix == current)) {
        Some(row) => row.saturating_add_signed(delta).min(last),
        None if delta >= 0 => 0,
        None => last,
    };
    order.get(row).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::inspector::PathKey;

    fn finding(rule: &'static str, severity: Severity, instances: u32, path: u32) -> Finding {
        Finding {
            severity,
            rule,
            title: format!("{rule} {path}"),
            detail: String::new(),
            element: Some(ElementKey {
                path: PathKey(path),
                instance: 0,
            }),
            entity: None,
            site: None,
            instances,
            others: Vec::new(),
        }
    }

    fn findings() -> Vec<Finding> {
        vec![
            finding("duplicate-id", Severity::Critical, 1, 1),
            finding("keyboard-access", Severity::Warning, 12, 2),
            finding("text-contrast", Severity::Warning, 1, 3),
            finding("keyboard-access", Severity::Warning, 1, 4),
            finding("overflow", Severity::Info, 3, 5),
        ]
    }

    #[test]
    fn findings_group_by_rule_worst_first() {
        let groups = group(&findings(), Filter::All);
        let summary: Vec<(&str, Vec<usize>, u32)> = groups
            .iter()
            .map(|group| (group.rule, group.findings.clone(), group.instances))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("duplicate-id", vec![0], 1),
                ("keyboard-access", vec![1, 3], 13),
                ("text-contrast", vec![2], 1),
                ("overflow", vec![4], 3),
            ]
        );
        let warnings = group(&findings(), Filter::Only(Severity::Warning));
        assert_eq!(warnings.len(), 2);
        assert!(group(&[], Filter::All).is_empty());
    }

    #[test]
    fn counts_and_health_read_plainly() {
        let counts = Counts::of(&findings());
        assert_eq!((counts.critical, counts.warning, counts.info), (1, 3, 1));
        assert_eq!(health(&counts, 300, 240), "1 critical problem to fix first");
        let warnings = Counts {
            warning: 2,
            ..Counts::default()
        };
        assert_eq!(health(&warnings, 300, 240), "2 problems worth fixing");
        let notes = Counts {
            info: 1,
            ..Counts::default()
        };
        assert_eq!(health(&notes, 300, 240), "Nothing broken, a few notes");
        assert_eq!(
            health(&Counts::default(), 1_284, 1),
            "Nothing to fix: 1,284 elements and 1 frame pass every check"
        );
        assert_eq!(
            health(&Counts::default(), 0, 0),
            "Nothing recorded yet: the checks run as the app draws"
        );
        assert_eq!(
            health(&Counts::default(), 0, 3),
            "Nothing to fix in 3 frames"
        );
        assert_eq!(Filter::Only(Severity::Warning).label(&counts), "Warnings 3");
        assert_eq!(Filter::All.label(&counts), "All 5");
    }

    #[test]
    fn blind_spots_follow_the_capture_level() {
        assert!(
            blind_spots(CaptureLevel::Frames, false)
                .unwrap()
                .contains("Tree or Full")
        );
        assert!(
            blind_spots(CaptureLevel::Full, false).is_some(),
            "no tree yet"
        );
        assert!(
            blind_spots(CaptureLevel::Tree, true)
                .unwrap()
                .contains("Full")
        );
        assert_eq!(blind_spots(CaptureLevel::Full, true), None);
    }

    #[test]
    fn stepping_follows_the_grouped_order_and_clamps() {
        let groups = group(&findings(), Filter::All);
        assert_eq!(step(&groups, None, 1), Some(0));
        assert_eq!(step(&groups, None, -1), Some(4));
        assert_eq!(step(&groups, Some(1), 1), Some(3), "within the group first");
        assert_eq!(step(&groups, Some(3), 1), Some(2));
        assert_eq!(step(&groups, Some(4), 1), Some(4));
        assert_eq!(step(&groups, Some(0), -1), Some(0));
        assert_eq!(step(&[], None, 1), None);
    }

    #[test]
    fn selections_survive_recomputation_by_key() {
        let before = findings();
        let key = FindingKey::of(&before[3]);
        let mut after = findings();
        after.remove(0);
        assert_eq!(key.find(&after), Some(2));
        after.remove(2);
        assert_eq!(key.find(&after), None);
        assert_eq!(rule_name("keyboard-access"), "Keyboard access");
        assert_eq!(rule_name("custom-rule"), "custom-rule");
    }
}
