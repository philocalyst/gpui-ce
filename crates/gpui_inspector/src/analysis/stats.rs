//! Frame statistics: rate, percentiles, grades, phase means, input latency
//! and Loupe's own overhead.
//!
//! App numbers exclude `inspector_only` frames (drawn only because Loupe
//! changed) and Loupe's own draw time ([`PhaseTimings::app_total`]); those are
//! reported separately in [`Overhead`], so Loupe never inflates what it shows.

use gpui::inspector::{FrameRecord, InputRecord, PhaseTimings};
use std::time::Duration;

/// A frame up to this multiple of the budget is graded [`Grade::Warn`];
/// beyond it, [`Grade::Crit`].
pub const WARN_BUDGET_FACTOR: f64 = 1.5;

/// The pulse strip caps bar height at this multiple of the budget.
pub const PULSE_CAP_BUDGETS: f64 = 3.0;

/// Frame rate is measured over the app frames in this window, ending at the
/// latest one.
pub const FPS_WINDOW: Duration = Duration::from_secs(1);

/// How a frame's cost compares with the frame budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Grade {
    /// Within budget.
    Ok,
    /// Over budget, up to [`WARN_BUDGET_FACTOR`]× of it: a dropped frame.
    Warn,
    /// Beyond [`WARN_BUDGET_FACTOR`]× the budget: visible jank.
    Crit,
}

impl Grade {
    /// Grades `duration` against `budget`.
    pub fn of(duration: Duration, budget: Duration) -> Self {
        if duration <= budget {
            Grade::Ok
        } else if duration.as_secs_f64() <= budget.as_secs_f64() * WARN_BUDGET_FACTOR {
            Grade::Warn
        } else {
            Grade::Crit
        }
    }

    /// Grades a frame by its app-attributable time.
    pub fn of_frame(frame: &FrameRecord, budget: Duration) -> Self {
        Self::of(frame.timings.app_total(), budget)
    }
}

/// Nearest-rank percentiles of a set of durations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Percentiles {
    /// Median.
    pub p50: Duration,
    /// 95th percentile.
    pub p95: Duration,
    /// 99th percentile.
    pub p99: Duration,
    /// Largest value.
    pub max: Duration,
}

impl Percentiles {
    /// Computes percentiles with the nearest-rank method (every reported
    /// value is one that actually occurred). Sorts `values` in place; all
    /// zero when empty.
    pub fn of(values: &mut [Duration]) -> Self {
        values.sort_unstable();
        Self {
            p50: nearest_rank(values, 50.0),
            p95: nearest_rank(values, 95.0),
            p99: nearest_rank(values, 99.0),
            max: values.last().copied().unwrap_or_default(),
        }
    }
}

/// The value at rank `ceil(p/100 × n)` of sorted `values`.
fn nearest_rank(sorted: &[Duration], percentile: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (percentile / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// How many frames fell in each [`Grade`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GradeCounts {
    /// Within budget.
    pub ok: usize,
    /// Slightly over budget.
    pub warn: usize,
    /// Far over budget.
    pub crit: usize,
}

impl GradeCounts {
    /// Frames over budget ([`Grade::Warn`] or [`Grade::Crit`]).
    pub fn over_budget(&self) -> usize {
        self.warn + self.crit
    }

    fn add(&mut self, grade: Grade) {
        match grade {
            Grade::Ok => self.ok += 1,
            Grade::Warn => self.warn += 1,
            Grade::Crit => self.crit += 1,
        }
    }
}

/// What Loupe itself cost while recording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overhead {
    /// Loupe's own draw time ([`PhaseTimings::inspector`]) summed over every frame.
    pub inspector_time: Duration,
    /// Frames drawn only because of Loupe.
    pub inspector_only_frames: usize,
    /// App time spent in those frames: the app redrew only because Loupe changed.
    pub inspector_only_time: Duration,
    /// Every frame's whole draw time, app and Loupe.
    pub total_time: Duration,
}

impl Overhead {
    /// The share of all drawing that Loupe caused, 0.0..=1.0: its own draw
    /// time plus the app time of frames that only Loupe caused.
    pub fn share(&self) -> f64 {
        ratio(
            self.inspector_time + self.inspector_only_time,
            self.total_time,
        )
    }
}

/// Summary statistics for a run of frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// Frames the app caused (not `inspector_only`); every other app number
    /// is computed over these.
    pub app_frames: usize,
    /// App frames per second over the [`FPS_WINDOW`] ending at the latest app
    /// frame; zero with fewer than two frames in that window.
    pub fps: f64,
    /// Percentiles of [`PhaseTimings::app_total`].
    pub app_total: Percentiles,
    /// Mean [`PhaseTimings::app_total`].
    pub mean: Duration,
    /// App frames per grade.
    pub grades: GradeCounts,
    /// Mean of every phase. `present` averages only presented frames.
    pub phase_means: PhaseTimings,
    /// What Loupe cost, including `inspector_only` frames.
    pub overhead: Overhead,
}

/// Computes [`FrameStats`] over `frames` (a whole ring, or any range of it),
/// in capture order.
pub fn frame_stats<'a>(
    frames: impl IntoIterator<Item = &'a FrameRecord>,
    budget: Duration,
) -> FrameStats {
    let mut stats = FrameStats::default();
    let mut totals = Vec::new();
    let mut starts = Vec::new();
    let mut phase_sums = PhaseSums::default();

    for frame in frames {
        let timings = &frame.timings;
        stats.overhead.inspector_time += timings.inspector;
        stats.overhead.total_time += timings.total;
        if frame.inspector_only {
            stats.overhead.inspector_only_frames += 1;
            stats.overhead.inspector_only_time += timings.app_total();
            continue;
        }
        stats.grades.add(Grade::of(timings.app_total(), budget));
        totals.push(timings.app_total());
        starts.push(frame.start);
        phase_sums.add(timings);
    }

    stats.app_frames = totals.len();
    stats.fps = fps(&starts);
    stats.mean = mean(totals.iter().sum(), totals.len());
    stats.app_total = Percentiles::of(&mut totals);
    stats.phase_means = phase_sums.means();
    stats
}

/// Frames per second from app frame start times: the frames within
/// [`FPS_WINDOW`] of the latest start, counted as intervals over the time
/// they span.
fn fps(starts: &[Duration]) -> f64 {
    let Some(&latest) = starts.iter().max() else {
        return 0.0;
    };
    let window_start = latest.saturating_sub(FPS_WINDOW);
    let (count, earliest) = starts
        .iter()
        .filter(|&&start| start >= window_start)
        .fold((0usize, latest), |(count, earliest), &start| {
            (count + 1, earliest.min(start))
        });
    let span = (latest - earliest).as_secs_f64();
    if count < 2 || span == 0.0 {
        0.0
    } else {
        (count - 1) as f64 / span
    }
}

#[derive(Default)]
struct PhaseSums {
    frames: usize,
    presented: usize,
    sums: PhaseTimings,
    present: Duration,
}

impl PhaseSums {
    fn add(&mut self, timings: &PhaseTimings) {
        self.frames += 1;
        let sums = &mut self.sums;
        sums.input += timings.input;
        sums.render += timings.render;
        sums.layout += timings.layout;
        sums.prepaint += timings.prepaint;
        sums.paint += timings.paint;
        sums.inspector += timings.inspector;
        sums.total += timings.total;
        if let Some(present) = timings.present {
            self.presented += 1;
            self.present += present;
        }
    }

    fn means(&self) -> PhaseTimings {
        let sums = &self.sums;
        let n = self.frames;
        PhaseTimings {
            input: mean(sums.input, n),
            render: mean(sums.render, n),
            layout: mean(sums.layout, n),
            prepaint: mean(sums.prepaint, n),
            paint: mean(sums.paint, n),
            inspector: mean(sums.inspector, n),
            present: (self.presented > 0).then(|| mean(self.present, self.presented)),
            total: mean(sums.total, n),
        }
    }
}

/// `sum / count`, zero for no values.
pub(crate) fn mean(sum: Duration, count: usize) -> Duration {
    match u32::try_from(count) {
        Ok(0) => Duration::ZERO,
        Ok(count) => sum / count,
        Err(_) => Duration::from_secs_f64(sum.as_secs_f64() / count as f64),
    }
}

/// `part / whole` as a fraction, zero when `whole` is zero.
pub(crate) fn ratio(part: Duration, whole: Duration) -> f64 {
    if whole.is_zero() {
        0.0
    } else {
        part.as_secs_f64() / whole.as_secs_f64()
    }
}

/// When the latest app frame (not `inspector_only`) started.
pub fn latest_app_start<'a>(frames: impl IntoIterator<Item = &'a FrameRecord>) -> Option<Duration> {
    frames
        .into_iter()
        .filter(|frame| !frame.inspector_only)
        .map(|frame| frame.start)
        .max()
}

/// The slowest app frame (by [`PhaseTimings::app_total`]), for "jump to
/// worst". The latest one wins ties.
pub fn worst_frame<'a>(
    frames: impl IntoIterator<Item = &'a FrameRecord>,
) -> Option<&'a FrameRecord> {
    frames
        .into_iter()
        .filter(|frame| !frame.inspector_only)
        .max_by_key(|frame| frame.timings.app_total())
}

/// Height of a pulse-strip bar as a fraction of the strip, 0.0..=1.0: the
/// square root of `app_total` over [`PULSE_CAP_BUDGETS`] budgets, so small
/// frames stay visible and outliers don't flatten everything else.
pub fn pulse_height(app_total: Duration, budget: Duration) -> f32 {
    let cap = budget.as_secs_f64() * PULSE_CAP_BUDGETS;
    if cap == 0.0 {
        return if app_total.is_zero() { 0.0 } else { 1.0 };
    }
    (app_total.as_secs_f64() / cap).sqrt().min(1.0) as f32
}

/// When the drawing of `frame` finished, as an offset from the capture epoch.
pub fn frame_end(frame: &FrameRecord) -> Duration {
    frame.start + frame.timings.total
}

/// How long one input event took to reach the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputLatency {
    /// The input record's sequence number.
    pub seq: u64,
    /// The frame that drew its effects.
    pub frame: u64,
    /// From the event's arrival to the end of that frame.
    pub latency: Duration,
}

/// The latency of every app input event whose frame is still known: from the
/// event's arrival (`at`) to the end of the frame that drew it ([`frame_end`]).
/// Input consumed by Loupe is skipped. `frame` looks frames up by id, e.g.
/// `|id| capture.frame(id)`.
pub fn input_latencies<'a, 'f>(
    input: impl IntoIterator<Item = &'a InputRecord>,
    frame: impl Fn(u64) -> Option<&'f FrameRecord>,
) -> Vec<InputLatency> {
    input
        .into_iter()
        .filter(|record| !record.inspector)
        .filter_map(|record| {
            let drawn_by = frame(record.frame?)?;
            Some(InputLatency {
                seq: record.seq,
                frame: drawn_by.id,
                latency: frame_end(drawn_by).saturating_sub(record.at),
            })
        })
        .collect()
}

/// Percentiles of a set of input latencies.
pub fn latency_percentiles(latencies: &[InputLatency]) -> Percentiles {
    let mut values: Vec<Duration> = latencies.iter().map(|latency| latency.latency).collect();
    Percentiles::of(&mut values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{frame, frames_every, input, ms};
    use gpui::inspector::InputKind;

    const BUDGET: Duration = Duration::from_micros(16_667);

    #[test]
    fn grades_follow_the_budget() {
        assert_eq!(Grade::of(Duration::ZERO, BUDGET), Grade::Ok);
        assert_eq!(Grade::of(BUDGET, BUDGET), Grade::Ok);
        assert_eq!(
            Grade::of(BUDGET + Duration::from_nanos(1), BUDGET),
            Grade::Warn
        );
        assert_eq!(Grade::of(ms(25.0), BUDGET), Grade::Warn);
        assert_eq!(Grade::of(ms(25.1), BUDGET), Grade::Crit);
        assert_eq!(Grade::of(Duration::MAX, BUDGET), Grade::Crit);
        assert_eq!(Grade::of(ms(1.0), Duration::ZERO), Grade::Crit);
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut values: Vec<Duration> = (1..=100).rev().map(|v| ms(v as f64)).collect();
        let percentiles = Percentiles::of(&mut values);
        assert_eq!(percentiles.p50, ms(50.0));
        assert_eq!(percentiles.p95, ms(95.0));
        assert_eq!(percentiles.p99, ms(99.0));
        assert_eq!(percentiles.max, ms(100.0));

        let mut four = vec![ms(4.0), ms(1.0), ms(3.0), ms(2.0)];
        let four = Percentiles::of(&mut four);
        assert_eq!((four.p50, four.p95, four.max), (ms(2.0), ms(4.0), ms(4.0)));
    }

    #[test]
    fn percentiles_of_nothing_and_one() {
        assert_eq!(Percentiles::of(&mut []), Percentiles::default());
        let one = Percentiles::of(&mut [ms(7.0)]);
        assert_eq!((one.p50, one.p99, one.max), (ms(7.0), ms(7.0), ms(7.0)));
    }

    #[test]
    fn stats_of_an_empty_ring_are_zero() {
        assert_eq!(frame_stats(&[], BUDGET), FrameStats::default());
    }

    #[test]
    fn stats_of_a_single_frame() {
        let stats = frame_stats(&[frame(0, ms(5.0), ms(20.0))], BUDGET);
        assert_eq!(stats.app_frames, 1);
        assert_eq!(stats.fps, 0.0);
        assert_eq!(stats.mean, ms(20.0));
        assert_eq!(stats.app_total.p50, ms(20.0));
        assert_eq!(stats.grades.warn, 1);
        assert_eq!(stats.grades.over_budget(), 1);
    }

    #[test]
    fn fps_counts_the_last_second_of_app_frames() {
        // 3 s of 60 fps (a frame every 16.667 ms), then silence.
        let frames = frames_every(180, Duration::from_micros(16_667), ms(4.0));
        let stats = frame_stats(&frames, BUDGET);
        assert!((stats.fps - 60.0).abs() < 0.1, "{}", stats.fps);

        // The fps window ends at the latest app frame, not at the latest frame.
        let mut with_loupe_tail = frames;
        let mut loupe = frame(180, ms(10_000.0), ms(1.0));
        loupe.inspector_only = true;
        with_loupe_tail.push(loupe);
        let stats = frame_stats(&with_loupe_tail, BUDGET);
        assert!((stats.fps - 60.0).abs() < 0.1, "{}", stats.fps);
    }

    #[test]
    fn fps_with_frames_at_the_same_instant_is_zero() {
        let frames = [frame(0, ms(1.0), ms(1.0)), frame(1, ms(1.0), ms(1.0))];
        assert_eq!(frame_stats(&frames, BUDGET).fps, 0.0);
    }

    #[test]
    fn inspector_only_frames_are_excluded_from_app_stats() {
        let mut frames = vec![
            frame(0, ms(0.0), ms(10.0)),
            frame(1, ms(16.0), ms(30.0)),
            frame(2, ms(32.0), ms(50.0)),
        ];
        frames[1].timings.inspector = ms(2.0);
        frames[2].inspector_only = true;
        frames[2].timings.inspector = ms(5.0);

        let stats = frame_stats(&frames, BUDGET);
        assert_eq!(stats.app_frames, 2);
        assert_eq!(stats.app_total.max, ms(28.0));
        assert_eq!(stats.mean, ms(19.0));
        assert_eq!(
            stats.grades,
            GradeCounts {
                ok: 1,
                warn: 0,
                crit: 1
            }
        );
        assert_eq!(stats.overhead.inspector_only_frames, 1);
        assert_eq!(stats.overhead.inspector_time, ms(7.0));
        assert_eq!(stats.overhead.inspector_only_time, ms(45.0));
        assert_eq!(stats.overhead.total_time, ms(90.0));
        assert!((stats.overhead.share() - 52.0 / 90.0).abs() < 1e-9);
    }

    #[test]
    fn phase_means_average_presented_frames_only_for_present() {
        let mut frames = vec![frame(0, ms(0.0), ms(10.0)), frame(1, ms(16.0), ms(20.0))];
        frames[0].timings.layout = ms(4.0);
        frames[1].timings.layout = ms(2.0);
        frames[1].timings.present = Some(ms(3.0));

        let means = frame_stats(&frames, BUDGET).phase_means;
        assert_eq!(means.render, ms(15.0));
        assert_eq!(means.layout, ms(3.0));
        assert_eq!(means.present, Some(ms(3.0)));
        assert_eq!(means.total, ms(15.0));
        assert_eq!(frame_stats(&frames[..1], BUDGET).phase_means.present, None);
    }

    #[test]
    fn huge_values_do_not_overflow() {
        let frames = [frame(
            0,
            Duration::ZERO,
            Duration::from_secs(u32::MAX as u64),
        )];
        let stats = frame_stats(&frames, BUDGET);
        assert_eq!(stats.grades.crit, 1);
        assert_eq!(stats.mean, Duration::from_secs(u32::MAX as u64));
        assert!(stats.overhead.share().is_finite());
    }

    #[test]
    fn worst_frame_skips_loupe_frames_and_prefers_the_latest_tie() {
        let mut frames = vec![
            frame(0, ms(0.0), ms(30.0)),
            frame(1, ms(16.0), ms(30.0)),
            frame(2, ms(32.0), ms(99.0)),
        ];
        frames[2].inspector_only = true;
        assert_eq!(worst_frame(&frames).map(|frame| frame.id), Some(1));
        assert!(worst_frame(&[]).is_none());
    }

    #[test]
    fn pulse_height_is_sqrt_scaled_and_capped() {
        assert_eq!(pulse_height(Duration::ZERO, BUDGET), 0.0);
        assert!((pulse_height(BUDGET * 3, BUDGET) - 1.0).abs() < 1e-6);
        assert_eq!(pulse_height(BUDGET * 30, BUDGET), 1.0);
        let at_budget = pulse_height(BUDGET, BUDGET);
        assert!((at_budget - (1.0f32 / 3.0).sqrt()).abs() < 1e-6);
        assert_eq!(pulse_height(ms(1.0), Duration::ZERO), 1.0);
        assert_eq!(pulse_height(Duration::ZERO, Duration::ZERO), 0.0);
    }

    #[test]
    fn input_latency_runs_to_the_end_of_the_drawing_frame() {
        let frames = [frame(7, ms(100.0), ms(12.0))];
        let lookup = |id| frames.iter().find(|frame| frame.id == id);
        let mut from_loupe = input(3, ms(95.0), InputKind::KeyDown, Some(7));
        from_loupe.inspector = true;
        let records = [
            input(1, ms(90.0), InputKind::MouseDown, Some(7)),
            input(2, ms(99.0), InputKind::MouseMove, None),
            from_loupe,
            input(4, ms(80.0), InputKind::KeyDown, Some(6)),
            // Arrived after the frame ended (clock skew): zero, not a panic.
            input(5, ms(200.0), InputKind::KeyUp, Some(7)),
        ];

        let latencies = input_latencies(&records, lookup);
        assert_eq!(
            latencies,
            vec![
                InputLatency {
                    seq: 1,
                    frame: 7,
                    latency: ms(22.0)
                },
                InputLatency {
                    seq: 5,
                    frame: 7,
                    latency: Duration::ZERO
                },
            ]
        );
        let percentiles = latency_percentiles(&latencies);
        assert_eq!(
            (percentiles.p50, percentiles.max),
            (Duration::ZERO, ms(22.0))
        );
    }
}
