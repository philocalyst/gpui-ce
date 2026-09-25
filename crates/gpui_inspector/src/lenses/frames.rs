//! Frames: why was that frame drawn, and why was it slow?

use super::{Fact, LensOverview, LensView, RailBadge, observe_state};
use crate::{
    shell::{fmt, pulse},
    state::{Lens, LensLayout, LoupeState},
    theme::Grade,
    widgets::{IconName, Tone},
};
use gpui::{
    App, Context, Entity, IntoElement, Render, Window,
    inspector::{FrameRecord, InspectorCapture},
};
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

/// App frames over budget, and the worst grade among them.
fn over_budget(capture: &InspectorCapture) -> (usize, Option<Grade>) {
    let budget = capture.config().budget;
    capture
        .frames()
        .iter()
        .filter(|frame| !frame.inspector_only)
        .map(|frame| Grade::of(frame.timings.app_total(), budget))
        .filter(|grade| *grade != Grade::Ok)
        .fold((0, None), |(count, worst), grade| {
            (count + 1, worst.max(Some(grade)))
        })
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
        Fact::new("App time", format!("{} ms", fmt::ms(total))).tone(tone),
        Fact::new(
            "Render · layout",
            format!(
                "{} · {} ms",
                fmt::ms(timings.render),
                fmt::ms(timings.layout)
            ),
        ),
        Fact::new(
            "Prepaint · paint",
            format!(
                "{} · {} ms",
                fmt::ms(timings.prepaint),
                fmt::ms(timings.paint)
            ),
        ),
        Fact::new(
            "Loupe's share",
            format!("{} ms", fmt::ms(timings.inspector)),
        ),
        Fact::new(
            "Views rendered",
            format!(
                "{} of {}",
                frame.rendered_views().count(),
                frame.views.len()
            ),
        ),
        Fact::new("Primitives", fmt::count(frame.scene.primitives() as usize)),
    ];
    if let Some(cause) = frame.causes.iter().find(|cause| !cause.from_inspector) {
        let site = cause
            .site
            .map(|site| format!(" at {}", fmt::location(site)))
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
        let app_frames = frames.iter().filter(|frame| !frame.inspector_only).count();
        let (slow, worst) = over_budget(capture);
        let slow_tone = match worst {
            Some(Grade::Crit) => Tone::Crit,
            Some(_) => Tone::Warn,
            None => Tone::Neutral,
        };
        let percentile = |p| {
            pulse::app_percentile(frames, p)
                .map(|value| format!("{} ms", fmt::ms(value)))
                .unwrap_or_else(|| "–".into())
        };
        overview = overview.section(
            "Recording",
            vec![
                Fact::new(
                    "Frames",
                    format!(
                        "{} app · {} Loupe only",
                        fmt::count(app_frames),
                        fmt::count(frames.len() - app_frames)
                    ),
                ),
                Fact::new(
                    "Frame rate",
                    pulse::fps(frames)
                        .map(|fps| format!("{fps:.0} fps"))
                        .unwrap_or_else(|| "–".into()),
                ),
                Fact::new(
                    "p50 · p95 · p99",
                    format!(
                        "{} · {} · {}",
                        percentile(50.),
                        percentile(95.),
                        percentile(99.)
                    ),
                ),
                Fact::new(
                    "Over budget",
                    format!(
                        "{} of {} ({} ms budget)",
                        slow,
                        app_frames,
                        fmt::ms(capture.config().budget)
                    ),
                )
                .tone(slow_tone),
            ],
        );
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
            overview = overview.section(title, frame_facts(frame, capture.config().budget));
        }
        overview
    }
}

impl LensView for FramesLens {
    fn rail_badge(&self, window: &Window, _cx: &App) -> Option<RailBadge> {
        let capture = window.inspector_capture()?;
        match over_budget(capture) {
            (0, _) => {
                let app = capture
                    .frames()
                    .iter()
                    .filter(|frame| !frame.inspector_only)
                    .count();
                (app > 0).then(|| RailBadge::count(fmt::count(app)))
            }
            (slow, worst) => Some(
                RailBadge::alert(
                    fmt::count(slow),
                    if worst == Some(Grade::Crit) {
                        Tone::Crit
                    } else {
                        Tone::Warn
                    },
                )
                .marker(IconName::TriangleUp),
            ),
        }
    }
}
