//! Detectors over a film's frame metrics and tracked values: pure functions,
//! so what they flag is easy to test and explain.

use super::{FrameMetrics, Track};
use crate::{
    manifest::{Finding, FindingKind},
    shot::trim,
};

/// Everything the detectors find, ordered by frame.
pub fn detect(frames: &[FrameMetrics], tracks: &[Track]) -> Vec<Finding> {
    let mut findings = frozen(frames);
    for track in tracks {
        findings.extend(jumps(track, frames));
        findings.extend(reversals(track, frames).into_iter().take(1));
        findings.extend(overshoot(track, frames));
    }
    findings.sort_by_key(|finding| finding.frame);
    findings
}

/// Stretches of two or more frames where nothing changed while the animation
/// was moving at speed on both sides: it hitched and then carried on.
///
/// "At speed" means the motion in the two frames on either side is at least
/// a quarter of the film's median motion. A single still frame doesn't
/// count: a spring at the turning point of an oscillation rounds to the same
/// device pixel for a moment, and that is not a hitch.
pub fn frozen(frames: &[FrameMetrics]) -> Vec<Finding> {
    let moving = frames
        .iter()
        .map(|frame| frame.changed_pixels > 0)
        .collect::<Vec<_>>();
    let mut energies = frames
        .iter()
        .filter(|frame| frame.changed_pixels > 0)
        .map(|frame| frame.energy)
        .collect::<Vec<_>>();
    energies.sort_by(f32::total_cmp);
    let brisk = energies.get(energies.len() / 2).copied().unwrap_or(0.) * 0.25;
    let peak = |range: std::ops::Range<usize>| {
        frames[range]
            .iter()
            .map(|frame| frame.energy)
            .fold(0f32, f32::max)
    };
    let mut findings = Vec::new();
    let mut ix = 1;
    while ix < frames.len() {
        if moving[ix] {
            ix += 1;
            continue;
        }
        let start = ix;
        while ix < frames.len() && !moving[ix] {
            ix += 1;
        }
        let before = peak(start.saturating_sub(2).max(1)..start);
        let after = peak(ix..(ix + 2).min(frames.len()));
        let still = ix - start;
        if still >= 2 && ix < frames.len() && before >= brisk && after >= brisk && before > 0. {
            findings.push(Finding {
                kind: FindingKind::Frozen,
                track: None,
                frame: start,
                message: format!(
                    "nothing changed for {still} frames ({}–{} ms), then motion resumed",
                    trim(frames[start - 1].t_ms as f32),
                    trim(frames[ix - 1].t_ms as f32),
                ),
            });
        }
    }
    findings
}

/// Frame-to-frame changes far larger than both neighboring changes: the
/// value teleported instead of animating.
pub fn jumps(track: &Track, frames: &[FrameMetrics]) -> Vec<Finding> {
    let steps = steps(track);
    let range = track.range().map_or(0., |(min, max)| max - min);
    let mut findings = Vec::new();
    for (ix, &(frame, delta)) in steps.iter().enumerate() {
        let before = ix.checked_sub(1).map(|ix| steps[ix].1.abs());
        let after = steps.get(ix + 1).map(|step| step.1.abs());
        let neighbor = before.into_iter().chain(after).fold(0f32, f32::max);
        if before.is_none() && after.is_none() {
            continue;
        }
        if delta.abs() > 3. * neighbor && delta.abs() > 0.1 * range && delta.abs() > 1e-3 {
            findings.push(Finding {
                kind: FindingKind::Jump,
                track: Some(track.name.clone()),
                frame,
                message: format!(
                    "{} jumped by {} at {} ms; neighboring frames moved at most {}",
                    track.name,
                    trim(delta),
                    trim(frames[frame].t_ms as f32),
                    trim(neighbor),
                ),
            });
        }
    }
    findings
}

/// Frames where the value moved against its overall direction by more than
/// 0.5 % of its range.
pub fn reversals(track: &Track, frames: &[FrameMetrics]) -> Vec<Finding> {
    let (Some(first), Some(last)) = (track.first(), track.last()) else {
        return Vec::new();
    };
    let range = track.range().map_or(0., |(min, max)| max - min);
    let direction = if last >= first { 1. } else { -1. };
    let tolerance = (range * 0.005).max(1e-3);
    steps(track)
        .into_iter()
        .filter(|(_, delta)| delta * direction < -tolerance)
        .map(|(frame, delta)| Finding {
            kind: FindingKind::NonMonotonic,
            track: Some(track.name.clone()),
            frame,
            message: format!(
                "{} reversed by {} at {} ms while heading from {} to {}",
                track.name,
                trim(delta),
                trim(frames[frame].t_ms as f32),
                trim(first),
                trim(last),
            ),
        })
        .collect()
}

/// The largest excursion past the value the track finally settled at.
pub fn overshoot(track: &Track, frames: &[FrameMetrics]) -> Option<Finding> {
    let (first, last) = (track.first()?, track.last()?);
    let travel = last - first;
    if travel.abs() < 1e-6 {
        return None;
    }
    let direction = travel.signum();
    let (frame, excess) = track
        .values
        .iter()
        .enumerate()
        .filter_map(|(frame, value)| Some((frame, (value.as_ref()? - last) * direction)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    (excess > (travel.abs() * 0.005).max(1e-3)).then(|| Finding {
        kind: FindingKind::Overshoot,
        track: Some(track.name.clone()),
        frame,
        message: format!(
            "{} overshot its final {} by {} ({}%) at {} ms",
            track.name,
            trim(last),
            trim(excess),
            trim(excess / travel.abs() * 100.),
            trim(frames[frame].t_ms as f32),
        ),
    })
}

/// `(frame, value - previous value)` for consecutive frames that both have a value.
fn steps(track: &Track) -> Vec<(usize, f32)> {
    track
        .values
        .windows(2)
        .enumerate()
        .filter_map(|(ix, pair)| Some((ix + 1, pair[1]? - pair[0]?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames 100 ms apart whose motion (changed pixels, and energy in
    /// proportion) is `changed(ix)`.
    fn frames(count: usize, changed: impl Fn(usize) -> u64) -> Vec<FrameMetrics> {
        (0..count)
            .map(|ix| {
                let changed_pixels = if ix == 0 { 0 } else { changed(ix) };
                FrameMetrics {
                    index: ix,
                    t_ms: ix as f64 * 100.,
                    image: String::new(),
                    changed_pixels,
                    changed: None,
                    energy: changed_pixels as f32 / 1000.,
                }
            })
            .collect()
    }

    fn track(values: &[f32]) -> Track {
        Track {
            name: "x".into(),
            values: values.iter().copied().map(Some).collect(),
        }
    }

    #[test]
    fn smooth_easing_is_clean() {
        let values = (0..=10)
            .map(|ix| {
                let t = ix as f32 / 10.;
                100. * (1. - (1. - t).powi(3))
            })
            .collect::<Vec<_>>();
        let frames = frames(values.len(), |_| 10);
        assert_eq!(detect(&frames, &[track(&values)]), vec![]);
    }

    #[test]
    fn flags_a_jump() {
        let values = [0., 1., 2., 40., 41., 42.];
        let frames = frames(values.len(), |_| 10);
        let findings = jumps(&track(&values), &frames);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].frame, 3);
        assert!(findings[0].message.contains("jumped by 38"));
    }

    #[test]
    fn flags_reversal_and_overshoot_of_a_spring() {
        let values = [0., 60., 110., 125., 112., 100., 100.];
        let frames = frames(values.len(), |_| 10);
        let track = track(&values);
        let reversal = reversals(&track, &frames);
        assert_eq!(reversal[0].frame, 4);
        let overshoot = overshoot(&track, &frames).unwrap();
        assert_eq!(overshoot.frame, 3);
        assert!(
            overshoot.message.contains("by 25 (25%)"),
            "{}",
            overshoot.message
        );
    }

    #[test]
    fn flags_a_hitch_but_not_settling() {
        // Moving, a two-frame freeze, moving again, then settled.
        let frames = frames(8, |ix| {
            if (3..5).contains(&ix) || ix >= 6 {
                0
            } else {
                5
            }
        });
        let findings = frozen(&frames);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].frame, 3);
        assert!(findings[0].message.contains("2 frames (200–400 ms)"));
    }

    #[test]
    fn a_slow_turning_point_is_not_a_freeze() {
        // A spring: brisk at first, then a still frame between two slow ones.
        let motion = [0, 90, 60, 30, 3, 0, 2, 1, 0, 0];
        let frames = frames(motion.len(), |ix| motion[ix]);
        assert_eq!(frozen(&frames), vec![]);
    }
}
