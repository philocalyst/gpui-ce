//! Frames: why was that frame drawn, and why was it slow?

use super::{Fact, LensOverview, LensView, RailBadge, observe_state};
use crate::{
    analysis::{
        format,
        stats::{FrameStats, Grade, GradeCounts, frame_stats},
    },
    shell::pulse,
    state::{Lens, LensLayout, LoupeState},
    widgets::{IconName, Tone},
};
use gpui::{App, Context, Entity, IntoElement, Render, Window, inspector::FrameRecord};
use std::time::Duration;

/// The Frames lens (overview until the flame chart and insights land).
pub(crate) struct FramesLens {
    state: Entity<LoupeState>,
}

impl FramesLens {
    pub fn new(state: Entity<LoupeState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        Self { state }
    }
}

/// The tone of the worst grade among `grades`.
fn worst_tone(grades: &GradeCounts) -> Tone {
    if grades.crit > 0 {
        Tone::Crit
    } else if grades.warn > 0 {
        Tone::Warn
    } else {
        Tone::Neutral
    }
}

fn millis(duration: Duration) -> String {
    format!("{} ms", format::millis(duration))
}

fn recording_facts(stats: &FrameStats, frames: usize, budget: Duration) -> Vec<Fact> {
    let percentiles = &stats.app_total;
    vec![
        Fact::new(
            "Frames",
            format!(
                "{} app · {} Loupe only",
                format::count(stats.app_frames as u64),
                format::count((frames - stats.app_frames) as u64)
            ),
        ),
        Fact::new("Frame rate", format!("{:.0} fps", stats.fps)),
        Fact::new(
            "p50 · p95 · p99",
            format!(
                "{} · {} · {}",
                millis(percentiles.p50),
                millis(percentiles.p95),
                millis(percentiles.p99)
            ),
        ),
        Fact::new(
            "Over budget",
            format!(
                "{} of {} ({} budget)",
                format::count(stats.grades.over_budget() as u64),
                format::count(stats.app_frames as u64),
                millis(budget)
            ),
        )
        .tone(worst_tone(&stats.grades)),
    ]
}

fn frame_facts(frame: &FrameRecord, budget: Duration) -> Vec<Fact> {
    let timings = &frame.timings;
    let total = timings.app_total();
    let tone = match Grade::of(total, budget) {
        Grade::Ok => Tone::Neutral,
        Grade::Warn => Tone::Warn,
        Grade::Crit => Tone::Crit,
    };
    let mut facts = vec![
        Fact::new("Frame", format!("#{}", frame.id)),
        Fact::new("App time", millis(total)).tone(tone),
        Fact::new(
            "Render · layout",
            format!(
                "{} · {} ms",
                format::millis(timings.render),
                format::millis(timings.layout)
            ),
        ),
        Fact::new(
            "Prepaint · paint",
            format!(
                "{} · {} ms",
                format::millis(timings.prepaint),
                format::millis(timings.paint)
            ),
        ),
        Fact::new("Loupe's share", millis(timings.inspector)),
        Fact::new(
            "Views rendered",
            format!(
                "{} of {}",
                frame.rendered_views().count(),
                frame.views.len()
            ),
        ),
        Fact::new(
            "Primitives",
            format::count(u64::from(frame.scene.primitives())),
        ),
    ];
    if let Some(cause) = frame.causes.iter().find(|cause| !cause.from_inspector) {
        let site = cause
            .site
            .map(|site| format!(" at {}", format::location(site)))
            .unwrap_or_default();
        facts.push(Fact::new(
            "First cause",
            format!("{}{site}", pulse::cause_summary(&cause.kind)),
        ));
    }
    facts
}

impl Render for FramesLens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.state.read(cx).selected_frame();
        let mut overview = LensOverview::new(Lens::Frames, LensLayout::of(window))
            .when_empty(IconName::Pause, "No frames recorded yet");
        let Some(capture) = window.inspector_capture() else {
            return overview;
        };
        let frames = capture.frames();
        if frames.is_empty() {
            return overview;
        }
        let budget = capture.config().budget;
        let stats = frame_stats(frames, budget);
        overview = overview.section("Recording", recording_facts(&stats, frames.len(), budget));
        let frame = match selected {
            Some(id) => capture.frame(id),
            None => capture.latest_app_frame(),
        };
        if let Some(frame) = frame {
            let title = if selected.is_some() {
                "Selected frame"
            } else {
                "Latest frame"
            };
            overview = overview.section(title, frame_facts(frame, budget));
        }
        overview
    }
}

impl LensView for FramesLens {
    fn rail_badge(&self, window: &Window, _cx: &App) -> Option<RailBadge> {
        let capture = window.inspector_capture()?;
        let stats = frame_stats(capture.frames(), capture.config().budget);
        match stats.grades.over_budget() {
            0 if stats.app_frames == 0 => None,
            0 => Some(RailBadge::count(format::count(stats.app_frames as u64))),
            slow => Some(
                RailBadge::alert(format::count(slow as u64), worst_tone(&stats.grades))
                    .marker(IconName::TriangleUp),
            ),
        }
    }
}
