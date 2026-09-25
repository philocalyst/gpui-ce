//! Filming animations: sample frames at exact times on the fake clock, then
//! measure what moved, track properties and assert on the motion.

mod apng;
pub mod detect;
mod strip;

use crate::{
    compose::Compositor,
    manifest::{Assertion, FilmRecord, Finding, FindingKind, Origin, Record},
    output::{Suite, slug},
    shot::{Rect, Shot, trim},
    stage::Stage,
};
use anyhow::Result;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{panic::Location, time::Duration};

/// When to sample an animation.
///
/// ```ignore
/// FilmSpec::fps(Duration::from_millis(400), 30.)  // 13 frames: 0, 33.3, …, 400 ms
/// FilmSpec::frames(Duration::from_millis(300), 7) // 0, 50, …, 300 ms
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilmSpec {
    /// How long to film after the trigger.
    pub duration: Duration,
    /// How many frames to take, including the ones at 0 and at `duration`.
    pub frames: usize,
}

impl FilmSpec {
    /// Frames every `1 / fps` seconds from 0 to `duration` (rounded to a
    /// whole number of intervals, so the last frame is at exactly `duration`).
    pub fn fps(duration: Duration, fps: f32) -> Self {
        let intervals = (duration.as_secs_f32() * fps).round().max(1.) as usize;
        Self {
            duration,
            frames: intervals + 1,
        }
    }

    /// `frames` evenly spaced frames from 0 to `duration` inclusive (at least 2).
    pub fn frames(duration: Duration, frames: usize) -> Self {
        Self {
            duration,
            frames: frames.max(2),
        }
    }

    /// The exact sample times.
    pub fn times(&self) -> Vec<Duration> {
        let intervals = self.frames.max(2) - 1;
        let total = self.duration.as_nanos();
        (0..=intervals)
            .map(|ix| Duration::from_nanos((total * ix as u128 / intervals as u128) as u64))
            .collect()
    }
}

/// What changed between a frame and the one before it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameMetrics {
    /// Frame number, from 0.
    pub index: usize,
    /// Time since the trigger, in ms.
    pub t_ms: f64,
    /// The frame's PNG, relative to the suite directory.
    pub image: String,
    /// Device pixels that differ from the previous frame.
    pub changed_pixels: u64,
    /// Bounding box of the changed pixels (logical), if any changed.
    pub changed: Option<Rect>,
    /// Mean absolute change per pixel and channel, `0..=1`.
    pub energy: f32,
}

/// A tracked property's value in each frame (`None` where it couldn't be measured).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// The property's name, e.g. `x` or `opacity`.
    pub name: String,
    /// One value per frame.
    pub values: Vec<Option<f32>>,
}

impl Track {
    /// The first measured value.
    pub fn first(&self) -> Option<f32> {
        self.values.iter().find_map(|value| *value)
    }

    /// The last measured value.
    pub fn last(&self) -> Option<f32> {
        self.values.iter().rev().find_map(|value| *value)
    }

    /// The smallest and largest measured values.
    pub fn range(&self) -> Option<(f32, f32)> {
        let mut values = self.values.iter().flatten().copied();
        let first = values.next()?;
        Some(values.fold((first, first), |(min, max), value| {
            (min.min(value), max.max(value))
        }))
    }
}

impl Stage {
    /// Films an animation: runs `trigger`, then takes a frame at each of
    /// `spec`'s times, advancing the fake clock exactly between them.
    ///
    /// Frames, a strip PNG (for agents: every frame with its time, then the
    /// curves) and an animated PNG (for humans) are written to
    /// `target/lightbox/<suite>/` when the film is dropped or saved.
    ///
    /// ```ignore
    /// let mut film = stage.film("panel-open", FilmSpec::fps(ms(400), 30.), |stage| {
    ///     stage.click_text("Open");
    /// });
    /// film.track("x", |shot| shot.quad_filled(accent).map(|quad| quad.bounds.x));
    /// film.assert_monotonic("x").assert_settles_by(Duration::from_millis(350));
    /// ```
    #[track_caller]
    pub fn film(&mut self, name: &str, spec: FilmSpec, trigger: impl FnOnce(&mut Stage)) -> Film {
        let origin = Origin::at(Location::caller());
        trigger(self);
        let times = spec.times();
        let mut frames = Vec::with_capacity(times.len());
        let mut now = Duration::ZERO;
        for (ix, time) in times.iter().enumerate() {
            if *time > now {
                self.advance(*time - now);
                now = *time;
            }
            frames.push(self.capture(&format!("{name} #{ix}")));
        }
        Film::new(
            name,
            self.suite().clone(),
            self.compositor.clone(),
            frames,
            times,
            origin,
        )
    }
}

/// A filmed animation: frames at exact times, their motion metrics, tracked
/// properties, detector findings and assertions.
///
/// Assertions panic on failure after saving the film, so the failing strip
/// is on disk; the film is also saved when dropped.
pub struct Film {
    name: String,
    suite: Suite,
    compositor: Compositor,
    frames: Vec<Shot>,
    times: Vec<Duration>,
    metrics: Vec<FrameMetrics>,
    tracks: Vec<Track>,
    assertions: Vec<Assertion>,
    origin: Origin,
    frames_written: bool,
    dirty: bool,
}

impl Film {
    fn new(
        name: &str,
        suite: Suite,
        compositor: Compositor,
        frames: Vec<Shot>,
        times: Vec<Duration>,
        origin: Origin,
    ) -> Self {
        let stem = slug(name);
        let mut metrics = Vec::with_capacity(frames.len());
        for (ix, frame) in frames.iter().enumerate() {
            let previous = ix.checked_sub(1).map(|ix| &frames[ix].image);
            let diff = previous.map(|previous| motion(previous, &frame.image));
            let (changed_pixels, changed, energy) = diff.map_or((0, None, 0.), |diff| {
                (
                    diff.changed_pixels,
                    diff.bounds.map(|(x, y, w, h)| {
                        let scale = frame.scale();
                        Rect::new(
                            x as f32 / scale,
                            y as f32 / scale,
                            w as f32 / scale,
                            h as f32 / scale,
                        )
                    }),
                    diff.energy,
                )
            });
            metrics.push(FrameMetrics {
                index: ix,
                t_ms: times[ix].as_secs_f64() * 1000.,
                image: format!("{stem}.frames/{ix:03}.png"),
                changed_pixels,
                changed,
                energy,
            });
        }
        Self {
            name: name.into(),
            suite,
            compositor,
            frames,
            times,
            metrics,
            tracks: Vec::new(),
            assertions: Vec::new(),
            origin,
            frames_written: false,
            dirty: true,
        }
    }

    /// The film's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The frames, in order.
    pub fn frames(&self) -> &[Shot] {
        &self.frames
    }

    /// Each frame's time since the trigger.
    pub fn times(&self) -> &[Duration] {
        &self.times
    }

    /// Each frame's motion metrics.
    pub fn metrics(&self) -> &[FrameMetrics] {
        &self.metrics
    }

    /// Measures a property in every frame, e.g. a quad's x or a pixel's
    /// alpha. Return `None` (or nothing, via `Option`) where it can't be found.
    pub fn track<V: Into<Option<f32>>>(
        &mut self,
        name: &str,
        measure: impl Fn(&Shot) -> V,
    ) -> &mut Self {
        let values = self
            .frames
            .iter()
            .map(|frame| measure(frame).into())
            .collect();
        self.tracks.retain(|track| track.name != name);
        self.tracks.push(Track {
            name: name.into(),
            values,
        });
        self.dirty = true;
        self
    }

    /// A tracked property's values, one per frame.
    ///
    /// # Panics
    ///
    /// If `name` wasn't tracked.
    #[track_caller]
    pub fn values(&self, name: &str) -> &[Option<f32>] {
        &self.tracked(name).values
    }

    /// What the detectors find: freezes, jumps, reversals and overshoot.
    pub fn findings(&self) -> Vec<Finding> {
        detect::detect(&self.metrics, &self.tracks)
    }

    /// The time of the last frame in which any pixel changed.
    pub fn settled_at(&self) -> Duration {
        self.metrics
            .iter()
            .rev()
            .find(|frame| frame.changed_pixels > 0)
            .map_or(Duration::ZERO, |frame| self.times[frame.index])
    }

    /// Asserts that `name` never moves against its overall direction.
    #[track_caller]
    pub fn assert_monotonic(&mut self, name: &str) -> &mut Self {
        let reversals = detect::reversals(self.tracked(name), &self.metrics);
        let message = reversals.first().map_or_else(
            || format!("{name} is monotonic"),
            |finding| finding.message.clone(),
        );
        self.check(format!("monotonic({name})"), reversals.is_empty(), message)
    }

    /// Asserts that `name` never goes past the value it settles at.
    #[track_caller]
    pub fn assert_no_overshoot(&mut self, name: &str) -> &mut Self {
        let overshoot = detect::overshoot(self.tracked(name), &self.metrics);
        let passed = overshoot.is_none();
        let message = overshoot.map_or_else(
            || format!("{name} never passes its final value"),
            |finding| finding.message,
        );
        self.check(format!("no_overshoot({name})"), passed, message)
    }

    /// Asserts that `name` never jumps (a step far larger than its neighbors).
    #[track_caller]
    pub fn assert_no_jumps(&mut self, name: &str) -> &mut Self {
        let jumps = detect::jumps(self.tracked(name), &self.metrics);
        let message = jumps.first().map_or_else(
            || format!("{name} moves smoothly"),
            |finding| finding.message.clone(),
        );
        self.check(format!("no_jumps({name})"), jumps.is_empty(), message)
    }

    /// Asserts that motion never stalls and then resumes.
    #[track_caller]
    pub fn assert_no_freezes(&mut self) -> &mut Self {
        let freezes = detect::frozen(&self.metrics);
        let message = freezes.first().map_or_else(
            || "motion never stalls".into(),
            |finding| finding.message.clone(),
        );
        self.check("no_freezes".into(), freezes.is_empty(), message)
    }

    /// Asserts that no pixel changes after `by` (the animation has settled).
    #[track_caller]
    pub fn assert_settles_by(&mut self, by: Duration) -> &mut Self {
        let late = self
            .metrics
            .iter()
            .find(|frame| frame.changed_pixels > 0 && self.times[frame.index] > by);
        let message = match late {
            Some(frame) => format!(
                "still moving at {} ms: {} px changed in {}",
                trim(frame.t_ms as f32),
                frame.changed_pixels,
                frame.changed.unwrap_or_default()
            ),
            None => format!(
                "settled at {} ms",
                trim(self.settled_at().as_secs_f32() * 1000.)
            ),
        };
        let passed = late.is_none();
        self.check(
            format!("settles_by({} ms)", trim(by.as_secs_f32() * 1000.)),
            passed,
            message,
        )
    }

    /// Asserts that `name` follows `expected(t)` (t in ms since the trigger)
    /// within `tolerance` in every frame where it was measured.
    #[track_caller]
    pub fn assert_follows(
        &mut self,
        name: &str,
        expected: impl Fn(f32) -> f32,
        tolerance: f32,
    ) -> &mut Self {
        let track = self.tracked(name);
        let worst = track
            .values
            .iter()
            .enumerate()
            .filter_map(|(ix, value)| {
                let t = self.metrics[ix].t_ms as f32;
                let want = expected(t);
                Some((ix, value.as_ref()?, want, (value.as_ref()? - want).abs()))
            })
            .max_by(|a, b| a.3.total_cmp(&b.3));
        let (passed, message) = match worst {
            Some((ix, got, want, error)) => (
                error <= tolerance,
                format!(
                    "largest error {} at {} ms (got {}, expected {}; tolerance {})",
                    trim(error),
                    trim(self.metrics[ix].t_ms as f32),
                    trim(*got),
                    trim(want),
                    trim(tolerance)
                ),
            ),
            None => (false, format!("{name} was never measured")),
        };
        self.check(format!("follows({name})"), passed, message)
    }

    #[track_caller]
    fn tracked(&self, name: &str) -> &Track {
        self.tracks
            .iter()
            .find(|track| track.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "lightbox: film {:?} has no track {name:?}; call film.track({name:?}, …) first",
                    self.name
                )
            })
    }

    #[track_caller]
    fn check(&mut self, name: String, passed: bool, message: String) -> &mut Self {
        self.assertions.push(Assertion {
            name: name.clone(),
            passed,
            message: message.clone(),
        });
        self.dirty = true;
        if !passed {
            if let Err(error) = self.save() {
                log::error!("lightbox: saving film {:?}: {error:#}", self.name);
            }
            panic!(
                "lightbox: film {:?}: {name} failed: {message}\n  strip: {}",
                self.name,
                self.suite.path(&self.strip_file()).display()
            );
        }
        self
    }

    fn strip_file(&self) -> String {
        format!("{}.film.png", slug(&self.name))
    }

    fn animation_file(&self) -> String {
        format!("{}.anim.png", slug(&self.name))
    }

    /// Writes the frames, the animated PNG, the strip and the record.
    pub fn save(&mut self) -> Result<()> {
        if !self.frames_written {
            for (frame, metrics) in self.frames.iter().zip(&self.metrics) {
                self.suite.write_png(&metrics.image, &frame.image)?;
            }
            let frames = self
                .frames
                .iter()
                .zip(&self.metrics)
                .map(|(frame, metrics)| apng::Frame {
                    image: &frame.image,
                    changed: metrics.changed.map(|changed| {
                        changed.to_device(frame.scale(), frame.image.width(), frame.image.height())
                    }),
                })
                .collect::<Vec<_>>();
            apng::write(
                &self.suite.path(&self.animation_file()),
                &frames,
                &self.times,
            )?;
            self.frames_written = true;
        }
        let findings = self.findings();
        let strip = self.compositor.with(|composer| {
            strip::render(
                composer,
                strip::Strip {
                    name: &self.name,
                    frames: &self.frames,
                    metrics: &self.metrics,
                    tracks: &self.tracks,
                    findings: &findings,
                    assertions: &self.assertions,
                },
            )
        })?;
        self.suite.write_png(&self.strip_file(), &strip)?;

        let first = self.frames.first();
        let record = Record::Film(FilmRecord {
            name: self.name.clone(),
            strip: self.strip_file(),
            animation: self.animation_file(),
            frames: self.metrics.clone(),
            tracks: self.tracks.clone(),
            findings,
            assertions: self.assertions.clone(),
            duration_ms: self.times.last().map_or(0., |t| t.as_secs_f64() * 1000.),
            size: first.map_or([0.; 2], |frame| frame.meta.size),
            scale: first.map_or(1., |frame| frame.scale()),
            origin: self.origin.clone(),
        });
        self.suite.write_record(&record)?;
        self.dirty = false;
        Ok(())
    }
}

impl Drop for Film {
    fn drop(&mut self) {
        if self.dirty && !std::thread::panicking() {
            if let Err(error) = self.save() {
                log::error!("lightbox: saving film {:?}: {error:#}", self.name);
            }
        }
    }
}

/// How two frames differ.
pub(crate) struct Motion {
    /// Pixels where any channel differs.
    pub(crate) changed_pixels: u64,
    /// Device-pixel bounding box `(x, y, w, h)` of the changed pixels.
    pub(crate) bounds: Option<(u32, u32, u32, u32)>,
    /// Mean absolute channel change, `0..=1`.
    pub(crate) energy: f32,
}

/// Compares two same-sized frames pixel by pixel.
pub(crate) fn motion(previous: &RgbaImage, current: &RgbaImage) -> Motion {
    if previous.dimensions() != current.dimensions() {
        let (w, h) = current.dimensions();
        return Motion {
            changed_pixels: u64::from(w) * u64::from(h),
            bounds: Some((0, 0, w, h)),
            energy: 1.,
        };
    }
    let width = current.width() as usize;
    let (mut changed, mut total) = (0u64, 0u64);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (ix, (a, b)) in previous
        .as_raw()
        .chunks_exact(4)
        .zip(current.as_raw().chunks_exact(4))
        .enumerate()
    {
        if a == b {
            continue;
        }
        let delta = a
            .iter()
            .zip(b)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum::<u64>();
        total += delta;
        changed += 1;
        let (x, y) = ((ix % width) as u32, (ix / width) as u32);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let channels = (current.as_raw().len() as f64).max(1.);
    Motion {
        changed_pixels: changed,
        bounds: (changed > 0).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1)),
        energy: (total as f64 / 255. / channels) as f32,
    }
}

impl FindingKind {
    /// Whether this kind of finding usually means something is wrong (a
    /// spring's overshoot, say, is often intended).
    pub fn is_problem(self) -> bool {
        matches!(self, FindingKind::Frozen | FindingKind::Jump)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_times_are_exact() {
        let spec = FilmSpec::fps(Duration::from_millis(400), 30.);
        let times = spec.times();
        assert_eq!(times.len(), 13);
        assert_eq!(times[0], Duration::ZERO);
        assert_eq!(*times.last().unwrap(), Duration::from_millis(400));
        let spec = FilmSpec::frames(Duration::from_millis(300), 7);
        assert_eq!(spec.times()[1], Duration::from_millis(50));
    }

    #[test]
    fn motion_measures_changed_region() {
        let a = RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 255]));
        let mut b = a.clone();
        b.put_pixel(3, 4, image::Rgba([255, 255, 255, 255]));
        b.put_pixel(5, 6, image::Rgba([255, 255, 255, 255]));
        let motion = motion(&a, &b);
        assert_eq!(motion.changed_pixels, 2);
        assert_eq!(motion.bounds, Some((3, 4, 3, 3)));
        assert!(motion.energy > 0.);
    }
}
