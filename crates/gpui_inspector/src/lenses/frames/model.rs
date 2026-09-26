//! What the Frames lens says, as plain data: the stats line, the selected
//! frame's header, why it was drawn and the input before it, and frame
//! stepping. Pure functions over the capture, so every sentence the lens
//! prints is unit-tested.

use crate::{
    analysis::{
        format,
        stats::{
            self, FPS_WINDOW, Grade, GradeCounts, frame_stats, input_latencies, latency_percentiles,
        },
    },
    widgets::Tone,
};
use gpui::{
    EntityId,
    inspector::{CauseKind, FrameRecord, InputKind, InputRecord, InspectorCapture, RenderCause},
};
use std::{collections::VecDeque, ops::Range, panic::Location, time::Duration};

/// The numbers at the top of the lens, over the whole ring.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StatsLine {
    /// App frames per second over the last second of app frames.
    pub fps: f64,
    /// When the latest app frame started.
    pub latest_app_start: Option<Duration>,
    /// Median app time.
    pub p50: Duration,
    /// 95th percentile app time.
    pub p95: Duration,
    /// 99th percentile app time.
    pub p99: Duration,
    /// App frames per grade.
    pub grades: GradeCounts,
    /// 95th percentile input latency (arrival to drawn), if any input drew.
    pub input_p95: Option<Duration>,
    /// Loupe's share of all drawing, 0.0..=1.0.
    pub overhead: f64,
    /// Loupe's mean draw time per frame.
    pub loupe_per_frame: Duration,
}

impl StatsLine {
    /// Computes the line for `capture`. Memoize it per generation: nothing
    /// in it changes until the capture records something.
    pub fn of(capture: &InspectorCapture) -> Self {
        let frames = capture.frames();
        let stats = frame_stats(frames, capture.config().budget);
        let latencies = input_latencies(capture.input(), |id| capture.frame(id));
        Self {
            fps: stats.fps,
            latest_app_start: stats::latest_app_start(frames),
            p50: stats.app_total.p50,
            p95: stats.app_total.p95,
            p99: stats.app_total.p99,
            grades: stats.grades,
            input_p95: (!latencies.is_empty()).then(|| latency_percentiles(&latencies).p95),
            overhead: stats.overhead.share(),
            loupe_per_frame: stats::mean(stats.overhead.inspector_time, frames.len()),
        }
    }

    /// How long until the app counts as idle at `now` (no app frame in
    /// the last second), or `None` if it already is.
    pub fn idle_in(&self, now: Duration) -> Option<Duration> {
        let since = now.saturating_sub(self.latest_app_start?);
        FPS_WINDOW.checked_sub(since).filter(|left| !left.is_zero())
    }

    /// `118`, or `idle` when the app has not drawn in the last second.
    pub fn fps_text(&self, now: Duration) -> String {
        match self.idle_in(now) {
            Some(_) => format!("{:.0}", self.fps),
            None => "idle".to_string(),
        }
    }

    /// `7 over budget`, or `all within budget`.
    pub fn over_budget_text(&self) -> String {
        match self.grades.over_budget() {
            0 if self.app_frames() == 0 => "no app frames".to_string(),
            0 => "all within budget".to_string(),
            over => format!("{} over budget", format::count(over as u64)),
        }
    }

    /// App frames counted in the percentiles.
    pub fn app_frames(&self) -> usize {
        self.grades.ok + self.grades.over_budget()
    }

    /// The share of app frames in each grade (ok, warn, crit), summing to
    /// 1.0, or all zero without app frames.
    pub fn grade_shares(&self) -> [f32; 3] {
        let total = self.app_frames();
        if total == 0 {
            return [0.0; 3];
        }
        let share = |count: usize| count as f32 / total as f32;
        [
            share(self.grades.ok),
            share(self.grades.warn),
            share(self.grades.crit),
        ]
    }
}

/// The budget pill: `within budget`, or how many budgets the frame took.
pub(crate) fn grade_pill(app_total: Duration, budget: Duration) -> (Tone, String) {
    match Grade::of(app_total, budget) {
        Grade::Ok => (Tone::Ok, "within budget".to_string()),
        grade => {
            let budgets = stats::ratio(app_total, budget);
            let tone = if grade == Grade::Crit {
                Tone::Crit
            } else {
                Tone::Warn
            };
            (tone, format!("{budgets:.1}× budget"))
        }
    }
}

/// `#18372 · 23.4 ms · 2.1 s ago`.
pub(crate) fn frame_title(frame: &FrameRecord, now: Duration) -> String {
    format!(
        "#{} · {} · {}",
        frame.id,
        format::duration(frame.timings.app_total()),
        format::relative_time(now, frame.start)
    )
}

/// How `frame` drew the app, when not simply by rendering what the app
/// asked for: it replayed the app's previous frame (on a frame Loupe drew
/// for itself, or while the app was held), or rendered the app on a frame
/// Loupe drew for itself.
pub(crate) fn app_note(frame: &FrameRecord) -> Option<&'static str> {
    match (frame.inspector_only, frame.replayed_app()) {
        (true, true) => Some(
            "Loupe drew this frame for itself: the app was replayed from its previous frame, \
             so none of its views rendered.",
        ),
        (false, true) => Some(
            "The app was held: this frame replayed its previous frame, so none of its views \
             rendered, and what the app asked for waited for the release.",
        ),
        (true, false) => Some("Loupe drew this frame for itself, and the app rendered with it."),
        (false, false) => None,
    }
}

/// The frame the lens shows: the selected one while it is still recorded,
/// else the latest app frame. The flag says whether it is the selection.
pub(crate) fn shown_frame(
    capture: &InspectorCapture,
    selected: Option<u64>,
) -> Option<(&FrameRecord, bool)> {
    match selected.and_then(|id| capture.frame(id)) {
        Some(frame) => Some((frame, true)),
        None => capture.latest_app_frame().map(|frame| (frame, false)),
    }
}

/// The app frame `delta` app frames after (or, negative, before) `current`,
/// stopping at the ends of the ring. `None` when there is nowhere to go.
/// `current` may be a Loupe-only frame; stepping lands on app frames.
pub(crate) fn step_frame(
    frames: &VecDeque<FrameRecord>,
    current: u64,
    delta: isize,
) -> Option<u64> {
    let app = frames.iter().filter(|frame| !frame.inspector_only);
    let target = if delta > 0 {
        app.filter(|frame| frame.id > current)
            .take(delta.unsigned_abs())
            .last()
    } else if delta < 0 {
        app.rev()
            .filter(|frame| frame.id < current)
            .take(delta.unsigned_abs())
            .last()
    } else {
        None
    };
    target.map(|frame| frame.id)
}

/// Why a frame was drawn, one line per distinct cause: repeated notifies
/// of the same entity from the same site fold into one line with a count.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CauseLine {
    /// What happened: `IssueStore notified`, `MouseDown event`.
    pub summary: String,
    /// How many times it happened before this frame.
    pub count: usize,
    /// Where it was triggered.
    pub site: Option<&'static Location<'static>>,
    /// How long before the frame the earliest of them happened.
    pub before: Duration,
    /// The notified entity.
    pub entity: Option<EntityId>,
    /// Loupe caused it (its own views, input to its dock).
    pub from_inspector: bool,
}

impl CauseLine {
    /// `3.2 ms before`, or `None` at the frame's start.
    pub fn before_text(&self) -> Option<String> {
        (!self.before.is_zero()).then(|| format!("{} before", format::duration(self.before)))
    }

    /// `IssueStore notified ×3`.
    pub fn summary_text(&self) -> String {
        match self.count {
            1 => self.summary.clone(),
            count => format!("{} ×{count}", self.summary),
        }
    }

    /// The whole line: `IssueStore notified · issue_store.rs:88 · 3.2 ms before`.
    #[cfg(test)]
    pub fn sentence(&self) -> String {
        let mut parts = vec![self.summary_text()];
        parts.extend(self.site.map(|site| format::location(site)));
        parts.extend(self.before_text());
        if self.from_inspector {
            parts.push("by Loupe".to_string());
        }
        parts.join(" · ")
    }
}

/// A few words for a cause.
pub(crate) fn cause_summary(kind: &CauseKind) -> String {
    match kind {
        CauseKind::Notify {
            type_name: Some(type_name),
            ..
        } => format!("{} notified", format::type_name(type_name)),
        CauseKind::Notify { entity, .. } => format!("Entity {entity} notified"),
        CauseKind::Refresh => "window.refresh() called".to_string(),
        CauseKind::Resize => "Window resized".to_string(),
        CauseKind::WindowState => "Window state changed".to_string(),
        CauseKind::Input { event } => format!("{event} event"),
        CauseKind::Animation => "Animation frame requested".to_string(),
        CauseKind::Initial => "First frame".to_string(),
    }
}

/// The frame's causes as lines, the app's in the order they first happened,
/// then Loupe's (folded into one line when there are several).
pub(crate) fn cause_lines(causes: &[RenderCause]) -> Vec<CauseLine> {
    let mut lines: Vec<(&RenderCause, CauseLine)> = Vec::new();
    for cause in causes {
        let same = lines.iter_mut().find(|(first, _)| {
            first.kind == cause.kind
                && first.site == cause.site
                && first.from_inspector == cause.from_inspector
        });
        match same {
            Some((_, line)) => {
                line.count += 1;
                line.before = line.before.max(cause.before_frame);
            }
            None => lines.push((
                cause,
                CauseLine {
                    summary: cause_summary(&cause.kind),
                    count: 1,
                    site: cause.site,
                    before: cause.before_frame,
                    entity: match cause.kind {
                        CauseKind::Notify { entity, .. } => Some(entity),
                        _ => None,
                    },
                    from_inspector: cause.from_inspector,
                },
            )),
        }
    }
    let (mut app, loupe): (Vec<CauseLine>, Vec<CauseLine>) = lines
        .into_iter()
        .map(|(_, line)| line)
        .partition(|line| !line.from_inspector);
    // Loupe's own views notifying each other is one fact, not a list.
    match loupe.len() {
        0 | 1 => app.extend(loupe),
        _ => app.push(CauseLine {
            summary: "Loupe updated itself".to_string(),
            count: loupe.iter().map(|line| line.count).sum(),
            site: None,
            before: loupe
                .iter()
                .map(|line| line.before)
                .max()
                .unwrap_or_default(),
            entity: None,
            from_inspector: true,
        }),
    }
    app
}

/// The input handled before a frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct InputSummary {
    /// App input records.
    pub app: usize,
    /// Input records Loupe consumed.
    pub loupe: usize,
    /// App input kinds with their counts, in the order first seen.
    pub kinds: Vec<(InputKind, usize)>,
    /// The record to open in Events: the first app record that invalidated
    /// the window, else the last app record.
    pub focus: Option<u64>,
    /// Records in the frame's range that the input ring no longer holds.
    pub evicted: usize,
}

impl InputSummary {
    /// `3 events · MouseDown, MouseUp, MouseMove ×2`, or `None` without app input.
    pub fn text(&self) -> Option<String> {
        if self.app == 0 {
            return None;
        }
        let kinds: Vec<String> = self
            .kinds
            .iter()
            .map(|(kind, count)| match count {
                1 => format!("{kind:?}"),
                count => format!("{kind:?} ×{count}"),
            })
            .collect();
        let events = match self.app {
            1 => "1 event".to_string(),
            count => format!("{count} events"),
        };
        Some(format!("{events} · {}", kinds.join(", ")))
    }
}

/// Summarizes the input records with sequence numbers in `range` (a frame's
/// [`FrameRecord::input`]).
pub(crate) fn input_summary(input: &VecDeque<InputRecord>, range: Range<u64>) -> InputSummary {
    let first = input.partition_point(|record| record.seq < range.start);
    let records = input
        .range(first..)
        .take_while(|record| record.seq < range.end);
    let mut summary = InputSummary::default();
    let mut found = 0;
    let mut redrew = None;
    for record in records {
        found += 1;
        if record.inspector {
            summary.loupe += 1;
            continue;
        }
        summary.app += 1;
        summary.focus = Some(record.seq);
        if record.caused_redraw && redrew.is_none() {
            redrew = Some(record.seq);
        }
        match summary
            .kinds
            .iter_mut()
            .find(|(kind, _)| *kind == record.kind)
        {
            Some((_, count)) => *count += 1,
            None => summary.kinds.push((record.kind, 1)),
        }
    }
    summary.focus = redrew.or(summary.focus);
    summary.evicted = (range.end.saturating_sub(range.start) as usize).saturating_sub(found);
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{FrameBuilder, InputBuilder, ms, steady_frames};
    use gpui::{inspector::ViewOutcome, point, px};

    const BUDGET: Duration = Duration::from_micros(16_667);

    fn cause(
        kind: CauseKind,
        site: &'static Location<'static>,
        before: f64,
        from_inspector: bool,
    ) -> RenderCause {
        RenderCause {
            kind,
            site: Some(site),
            before_frame: ms(before),
            from_inspector,
        }
    }

    fn notify(entity: u64) -> CauseKind {
        CauseKind::Notify {
            entity: EntityId::from(entity),
            type_name: Some("inbox::IssueStore"),
        }
    }

    #[test]
    fn stats_line_reads_fps_percentiles_and_overhead() {
        let capture = steady_frames(120, 4.0);
        let line = StatsLine::of(&capture);
        assert_eq!(line.fps_text(capture.now()), "60");
        assert_eq!(line.p95, ms(4.0));
        assert_eq!(line.over_budget_text(), "all within budget");
        assert_eq!(line.grade_shares(), [1.0, 0.0, 0.0]);
        assert_eq!(line.input_p95, None);
        assert_eq!(format::percent(line.overhead), "7.0%");
        assert_eq!(line.loupe_per_frame, ms(0.3));
    }

    #[test]
    fn stats_line_is_idle_a_second_after_the_last_app_frame() {
        let capture = steady_frames(10, 4.0);
        let last = stats::latest_app_start(capture.frames()).unwrap();
        let line = StatsLine::of(&capture);
        assert_eq!(line.fps_text(last + ms(999.0)), "60");
        let left = line.idle_in(last + ms(990.0)).unwrap();
        assert!((left.as_secs_f64() - 0.010).abs() < 1e-6, "{left:?}");
        assert_eq!(line.fps_text(last + ms(1_001.0)), "idle");
        assert_eq!(line.idle_in(last + ms(1_001.0)), None);
        let empty = StatsLine::of(&InspectorCapture::new_for_test());
        assert_eq!(empty.fps_text(Duration::ZERO), "idle");
        assert_eq!(empty.over_budget_text(), "no app frames");
    }

    #[test]
    fn stats_line_grades_and_input_latency() {
        let mut capture = steady_frames(3, 4.0);
        FrameBuilder::new()
            .at(ms(60.))
            .app_time(ms(20.), ms(0.3))
            .push(&mut capture);
        FrameBuilder::new()
            .at(ms(90.))
            .app_time(ms(30.), ms(0.3))
            .push(&mut capture);
        let seq = InputBuilder::click(ms(100.), point(px(1.), px(1.))).push(&mut capture);
        FrameBuilder::new()
            .at(ms(110.))
            .app_time(ms(8.), ms(0.3))
            .input(seq..seq + 1)
            .push(&mut capture);
        let line = StatsLine::of(&capture);
        assert_eq!(line.over_budget_text(), "2 over budget");
        assert_eq!(line.grade_shares(), [4. / 6., 1. / 6., 1. / 6.]);
        // Arrived at 100 ms, drawn by the frame ending at 110 + 8.3 ms.
        let latency = line.input_p95.unwrap().as_secs_f64() * 1e3;
        assert!((latency - 18.3).abs() < 1e-3, "{latency}");
    }

    #[test]
    fn grade_pills_say_how_many_budgets() {
        assert_eq!(
            grade_pill(ms(8.0), BUDGET),
            (Tone::Ok, "within budget".to_string())
        );
        assert_eq!(
            grade_pill(ms(20.0), BUDGET),
            (Tone::Warn, "1.2× budget".to_string())
        );
        assert_eq!(
            grade_pill(ms(35.0), BUDGET),
            (Tone::Crit, "2.1× budget".to_string())
        );
    }

    #[test]
    fn frame_titles_read_id_time_and_age() {
        let capture = steady_frames(3, 23.4);
        let frame = capture.frame(1).unwrap();
        assert_eq!(
            frame_title(frame, frame.start + ms(2_100.0)),
            "#1 · 23.4 ms · 2.1 s ago"
        );
    }

    #[test]
    fn app_notes_say_how_a_frame_drew_the_app() {
        let frame = |outcome, inspector_only: bool| {
            let frame = FrameBuilder::new().view(1, "app::App", 0, ms(0.), ms(0.), outcome);
            if inspector_only {
                frame.inspector_only().build()
            } else {
                frame.build()
            }
        };
        let note = |outcome, inspector_only| app_note(&frame(outcome, inspector_only));
        assert_eq!(note(ViewOutcome::Rendered, false), None);
        assert_eq!(note(ViewOutcome::Cached, false), None);
        assert!(
            note(ViewOutcome::Replayed, true)
                .unwrap()
                .contains("replayed")
        );
        assert!(
            note(ViewOutcome::Replayed, false)
                .unwrap()
                .starts_with("The app was held")
        );
        assert!(
            note(ViewOutcome::Rendered, true)
                .unwrap()
                .ends_with("rendered with it.")
        );
    }

    #[test]
    fn the_shown_frame_is_the_selection_or_the_latest_app_frame() {
        let mut capture = steady_frames(3, 4.0);
        FrameBuilder::new()
            .at(ms(60.))
            .app_time(ms(1.), ms(0.3))
            .inspector_only()
            .push(&mut capture);
        let shown = |selected| shown_frame(&capture, selected).map(|(f, pinned)| (f.id, pinned));
        assert_eq!(
            shown(None),
            Some((2, false)),
            "Loupe-only frames are skipped"
        );
        assert_eq!(shown(Some(1)), Some((1, true)));
        assert_eq!(shown(Some(3)), Some((3, true)), "an explicit pick is kept");
        assert_eq!(shown(Some(99)), Some((2, false)), "evicted: back to latest");
    }

    #[test]
    fn stepping_moves_over_app_frames_and_stops_at_the_ends() {
        let mut capture = steady_frames(3, 4.0);
        FrameBuilder::new()
            .at(ms(60.))
            .app_time(ms(1.), ms(0.3))
            .inspector_only()
            .push(&mut capture);
        FrameBuilder::new()
            .at(ms(80.))
            .app_time(ms(4.), ms(0.3))
            .push(&mut capture);
        let frames = capture.frames();
        assert_eq!(step_frame(frames, 2, 1), Some(4), "skips the Loupe frame");
        assert_eq!(step_frame(frames, 4, -1), Some(2));
        assert_eq!(step_frame(frames, 3, -1), Some(2), "from a Loupe frame");
        assert_eq!(step_frame(frames, 0, 10), Some(4), "clamped to the latest");
        assert_eq!(step_frame(frames, 4, 1), None, "already the latest");
        assert_eq!(step_frame(frames, 0, -1), None, "already the oldest");
        assert_eq!(step_frame(frames, 1, 0), None);
    }

    #[test]
    fn worst_frame_is_the_slowest_app_frame() {
        let mut capture = steady_frames(3, 4.0);
        FrameBuilder::new()
            .at(ms(60.))
            .app_time(ms(30.), ms(0.3))
            .push(&mut capture);
        FrameBuilder::new()
            .at(ms(90.))
            .app_time(ms(90.), ms(0.3))
            .inspector_only()
            .push(&mut capture);
        assert_eq!(stats::worst_frame(capture.frames()).map(|f| f.id), Some(3));
    }

    #[test]
    fn causes_read_as_sentences_and_fold_repeats() {
        let here = Location::caller();
        let there = Location::caller();
        let causes = [
            cause(CauseKind::Input { event: "MouseMove" }, here, 1.0, true),
            cause(notify(6), there, 3.2, false),
            cause(notify(6), there, 1.0, false),
            cause(CauseKind::Refresh, here, 0.0, false),
        ];
        let lines = cause_lines(&causes);
        let site = format::location(causes[1].site.unwrap());
        let sentences: Vec<String> = lines.iter().map(CauseLine::sentence).collect();
        assert_eq!(
            sentences,
            vec![
                format!("IssueStore notified ×2 · {site} · 3.2 ms before"),
                format!(
                    "window.refresh() called · {}",
                    format::location(causes[3].site.unwrap())
                ),
                format!(
                    "MouseMove event · {} · 1.0 ms before · by Loupe",
                    format::location(causes[0].site.unwrap())
                ),
            ]
        );
        assert_eq!(lines[0].entity, Some(EntityId::from(6u64)));
        assert!(lines[2].from_inspector, "Loupe's causes come last");
    }

    #[test]
    fn loupes_own_causes_fold_into_one_line() {
        let site = Location::caller();
        let causes = [
            cause(notify(90), site, 0.3, true),
            cause(CauseKind::Initial, site, 0.0, false),
            cause(notify(91), site, 0.1, true),
            cause(notify(91), site, 0.2, true),
        ];
        let sentences: Vec<String> = cause_lines(&causes)
            .iter()
            .map(CauseLine::sentence)
            .collect();
        assert_eq!(
            sentences,
            [
                format!("First frame · {}", format::location(site)),
                "Loupe updated itself ×3 · 300 µs before · by Loupe".to_string(),
            ]
        );
    }

    #[test]
    fn every_cause_kind_has_words() {
        let summaries: Vec<String> = [
            CauseKind::Notify {
                entity: EntityId::from(3u64),
                type_name: None,
            },
            CauseKind::Resize,
            CauseKind::WindowState,
            CauseKind::Animation,
            CauseKind::Initial,
        ]
        .iter()
        .map(cause_summary)
        .collect();
        assert_eq!(
            summaries,
            [
                format!("Entity {} notified", EntityId::from(3u64)),
                "Window resized".to_string(),
                "Window state changed".to_string(),
                "Animation frame requested".to_string(),
                "First frame".to_string(),
            ]
        );
    }

    #[test]
    fn input_summaries_count_kinds_and_pick_the_redrawing_record() {
        let mut capture = InspectorCapture::new_for_test();
        let at = point(px(4.), px(4.));
        InputBuilder::moves(ms(1.), at, 3).push(&mut capture);
        let click = InputBuilder::click(ms(2.), at)
            .handled(true)
            .push(&mut capture);
        InputBuilder::moves(ms(3.), at, 2).push(&mut capture);
        InputBuilder::moves(ms(4.), at, 1)
            .inspector()
            .push(&mut capture);
        let summary = input_summary(capture.input(), 0..5);
        assert_eq!(
            summary.text().as_deref(),
            Some("3 events · MouseMove ×2, MouseDown")
        );
        assert_eq!((summary.app, summary.loupe, summary.evicted), (3, 1, 1));
        assert_eq!(summary.focus, Some(click));

        assert_eq!(
            input_summary(capture.input(), 3..4).text(),
            None,
            "Loupe's only"
        );
        assert_eq!(
            input_summary(capture.input(), 9..9),
            InputSummary::default()
        );
    }
}
