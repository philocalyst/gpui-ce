//! Bottom-up self time by view type: where render time actually goes.
//!
//! A view's span covers its own `render()` plus its subtree's layout
//! requests, including nested views. Its *self* time subtracts the time of the
//! views nested directly inside it, so a slow leaf is blamed on the leaf, not
//! on every ancestor.

use gpui::inspector::{FrameRecord, ViewOutcome, ViewSpan};
use std::{collections::HashMap, time::Duration};

/// Aggregated render cost of one view type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BottomUpRow {
    /// `type_name::<V>()`.
    pub type_name: &'static str,
    /// Spans of this type, rendered or cached.
    pub calls: u32,
    /// Spans where `render()` ran.
    pub rendered: u32,
    /// Spans served from the view cache.
    pub cached: u32,
    /// Inclusive time. A type nested inside itself is counted once, at its
    /// outermost span, so recursion does not double the total.
    pub total: Duration,
    /// Time in the type's own spans, excluding nested views.
    pub self_time: Duration,
    /// Mean span length.
    pub mean: Duration,
    /// Longest single span.
    pub max: Duration,
}

/// The self time of each span in `views`, in the same order: its duration
/// minus the part of each directly nested view's span that overlaps it.
///
/// Nesting comes from `depth` and time containment, so spans may be recorded
/// in any order (pre- or post-order).
pub fn self_times(views: &[ViewSpan]) -> Vec<Duration> {
    let mut self_times: Vec<Duration> = views.iter().map(|view| view.duration).collect();
    for (ix, parent) in nesting(views) {
        if let Some(parent) = parent {
            let overlap = overlap(&views[parent], &views[ix]);
            self_times[parent] = self_times[parent].saturating_sub(overlap);
        }
    }
    self_times
}

/// Aggregates the views of `frames` by type, sorted by self time (then total
/// time, then name). Pass one frame for a frame's breakdown, or a selection
/// for a range; `inspector_only` frames are included if passed. Frames that
/// replayed the app add nothing: none of its views did any work in them.
pub fn bottom_up<'a>(frames: impl IntoIterator<Item = &'a FrameRecord>) -> Vec<BottomUpRow> {
    let mut rows: HashMap<&'static str, Accumulator> = HashMap::new();
    for frame in frames.into_iter().filter(|frame| !frame.replayed_app()) {
        let views = &frame.views;
        let self_times = self_times(views);
        let mut ancestors: Vec<usize> = Vec::new();
        for (ix, parent) in nesting(views) {
            ancestors.truncate(match parent {
                Some(parent) => ancestors
                    .iter()
                    .position(|&a| a == parent)
                    .map_or(0, |p| p + 1),
                None => 0,
            });
            let view = &views[ix];
            let is_recursive = ancestors
                .iter()
                .any(|&ancestor| views[ancestor].type_name == view.type_name);
            rows.entry(view.type_name)
                .or_default()
                .add(view, self_times[ix], !is_recursive);
            ancestors.push(ix);
        }
    }

    let mut rows: Vec<BottomUpRow> = rows
        .into_iter()
        .map(|(type_name, accumulator)| accumulator.finish(type_name))
        .collect();
    rows.sort_by(|a, b| {
        b.self_time
            .cmp(&a.self_time)
            .then(b.total.cmp(&a.total))
            .then(a.type_name.cmp(b.type_name))
    });
    rows
}

/// Each span's index with its direct parent's index, in pre-order (by start,
/// outer before inner).
fn nesting(views: &[ViewSpan]) -> Vec<(usize, Option<usize>)> {
    let mut order: Vec<usize> = (0..views.len()).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = (&views[a], &views[b]);
        a.start.cmp(&b.start).then(a.depth.cmp(&b.depth))
    });

    let mut stack: Vec<usize> = Vec::new();
    order
        .into_iter()
        .map(|ix| {
            let view = &views[ix];
            while let Some(&top) = stack.last() {
                let open = &views[top];
                if open.depth < view.depth && end(open) > view.start {
                    break;
                }
                stack.pop();
            }
            let parent = stack.last().copied();
            stack.push(ix);
            (ix, parent)
        })
        .collect()
}

fn end(view: &ViewSpan) -> Duration {
    view.start.saturating_add(view.duration)
}

fn overlap(a: &ViewSpan, b: &ViewSpan) -> Duration {
    end(a).min(end(b)).saturating_sub(a.start.max(b.start))
}

#[derive(Default)]
struct Accumulator {
    calls: u32,
    rendered: u32,
    cached: u32,
    total: Duration,
    self_time: Duration,
    span_sum: Duration,
    max: Duration,
}

impl Accumulator {
    fn add(&mut self, view: &ViewSpan, self_time: Duration, counts_in_total: bool) {
        self.calls += 1;
        self.rendered += u32::from(view.outcome == ViewOutcome::Rendered);
        self.cached += u32::from(view.outcome == ViewOutcome::Cached);
        if counts_in_total {
            self.total += view.duration;
        }
        self.self_time += self_time;
        self.span_sum += view.duration;
        self.max = self.max.max(view.duration);
    }

    fn finish(self, type_name: &'static str) -> BottomUpRow {
        BottomUpRow {
            type_name,
            calls: self.calls,
            rendered: self.rendered,
            cached: self.cached,
            total: self.total,
            self_time: self.self_time,
            mean: super::stats::mean(self.span_sum, self.calls as usize),
            max: self.max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{cached_view, frame, ms, replayed_view, view};

    /// Workspace (0..10) ⊃ Sidebar (1..3), IssueList (3..9) ⊃ Row ×2.
    fn workspace() -> FrameRecord {
        let mut record = frame(0, Duration::ZERO, ms(12.0));
        record.views = vec![
            view(1, "app::Workspace", 0, ms(0.0), ms(10.0)),
            view(2, "app::Sidebar", 1, ms(1.0), ms(2.0)),
            view(3, "app::IssueList", 1, ms(3.0), ms(6.0)),
            view(4, "app::Row", 2, ms(4.0), ms(1.5)),
            cached_view(5, "app::Row", 2, ms(6.0), ms(0.5)),
        ];
        record
    }

    #[test]
    fn self_time_subtracts_direct_children() {
        assert_eq!(
            self_times(&workspace().views),
            vec![ms(2.0), ms(2.0), ms(4.0), ms(1.5), ms(0.5)]
        );
    }

    #[test]
    fn self_time_does_not_depend_on_recording_order() {
        let mut views = workspace().views;
        views.reverse();
        assert_eq!(
            self_times(&views),
            vec![ms(0.5), ms(1.5), ms(4.0), ms(2.0), ms(2.0)]
        );
    }

    #[test]
    fn children_overhanging_their_parent_only_subtract_the_overlap() {
        let views = vec![
            view(1, "app::Parent", 0, ms(0.0), ms(4.0)),
            view(2, "app::Child", 1, ms(3.0), ms(5.0)),
        ];
        assert_eq!(self_times(&views), vec![ms(3.0), ms(5.0)]);
    }

    #[test]
    fn sequential_views_at_the_same_depth_are_siblings() {
        let views = vec![
            view(1, "app::A", 0, ms(0.0), ms(2.0)),
            view(2, "app::B", 0, ms(2.0), ms(2.0)),
            view(3, "app::C", 1, ms(2.5), ms(1.0)),
        ];
        assert_eq!(self_times(&views), vec![ms(2.0), ms(1.0), ms(1.0)]);
    }

    #[test]
    fn empty_frames_have_no_rows() {
        assert!(bottom_up(&[frame(0, Duration::ZERO, ms(1.0))]).is_empty());
        assert!(self_times(&[]).is_empty());
        assert!(bottom_up(&[]).is_empty());
    }

    #[test]
    fn frames_that_replayed_the_app_add_nothing() {
        let mut replayed = frame(1, ms(20.0), ms(0.5));
        replayed.views = vec![
            replayed_view(1, "app::Workspace", 0, ms(0.1)),
            replayed_view(3, "app::IssueList", 1, ms(0.1)),
        ];
        assert!(bottom_up(&[replayed.clone()]).is_empty());
        assert_eq!(
            bottom_up(&[workspace(), replayed]),
            bottom_up(&[workspace()])
        );
    }

    #[test]
    fn rows_are_sorted_by_self_time() {
        let rows = bottom_up(&[workspace()]);
        let summary: Vec<(&str, u32, u32, u32, Duration, Duration)> = rows
            .iter()
            .map(|row| {
                (
                    row.type_name,
                    row.calls,
                    row.rendered,
                    row.cached,
                    row.total,
                    row.self_time,
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("app::IssueList", 1, 1, 0, ms(6.0), ms(4.0)),
                ("app::Workspace", 1, 1, 0, ms(10.0), ms(2.0)),
                ("app::Row", 2, 1, 1, ms(2.0), ms(2.0)),
                ("app::Sidebar", 1, 1, 0, ms(2.0), ms(2.0)),
            ]
        );
        assert_eq!(rows[2].mean, ms(1.0));
        assert_eq!(rows[2].max, ms(1.5));
    }

    #[test]
    fn rows_aggregate_across_frames() {
        let rows = bottom_up(&[workspace(), workspace()]);
        let list = rows
            .iter()
            .find(|row| row.type_name == "app::IssueList")
            .unwrap();
        assert_eq!(
            (list.calls, list.total, list.self_time),
            (2, ms(12.0), ms(8.0))
        );
    }

    #[test]
    fn recursive_types_count_total_once() {
        let mut record = frame(0, Duration::ZERO, ms(10.0));
        record.views = vec![
            view(1, "app::TreeNode", 0, ms(0.0), ms(8.0)),
            view(2, "app::TreeNode", 1, ms(1.0), ms(6.0)),
            view(3, "app::TreeNode", 2, ms(2.0), ms(3.0)),
        ];
        let rows = bottom_up(&[record]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].total, ms(8.0));
        assert_eq!(rows[0].self_time, ms(8.0));
        assert_eq!(rows[0].mean, ms(17.0) / 3);
    }

    #[test]
    fn zero_length_spans_are_harmless() {
        let mut record = frame(0, Duration::ZERO, ms(1.0));
        record.views = vec![
            view(1, "app::A", 0, ms(0.0), Duration::ZERO),
            view(2, "app::B", 1, ms(0.0), Duration::ZERO),
        ];
        let rows = bottom_up(&[record]);
        assert!(
            rows.iter()
                .all(|row| row.total.is_zero() && row.self_time.is_zero())
        );
        assert_eq!(rows.len(), 2);
    }
}
