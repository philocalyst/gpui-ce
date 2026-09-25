//! Events: where did my click go? What will this key do here?

use super::{Fact, LensOverview, LensView, RailBadge, observe_state};
use crate::{
    shell::fmt,
    state::{Lens, LensLayout, LoupeState},
    widgets::IconName,
};
use gpui::{
    App, Context, Entity, IntoElement, Render, Window,
    inspector::{InputKind, InputRecord},
};

/// The Events lens (overview until the log and key tester land).
pub(crate) struct EventsLens {
    state: Entity<LoupeState>,
}

impl EventsLens {
    pub fn new(state: Entity<LoupeState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        Self { state }
    }
}

fn app_input<'a>(
    input: impl Iterator<Item = &'a InputRecord>,
) -> impl Iterator<Item = &'a InputRecord> {
    input.filter(|record| !record.inspector)
}

fn input_facts(record: &InputRecord) -> Vec<Fact> {
    let mut facts = vec![
        Fact::new("Event", format!("{:?} · {}", record.kind, record.detail)),
        Fact::new(
            "Handled",
            if record.handled {
                "yes"
            } else {
                "no listener stopped it"
            },
        ),
    ];
    if let Some(action) = record.actions.first() {
        facts.push(Fact::new(
            "Action",
            format!(
                "{}{}",
                action.name,
                if action.handled { "" } else { " (unhandled)" }
            ),
        ));
    }
    if !record.hit_path.is_empty() {
        facts.push(Fact::new(
            "Elements under pointer",
            fmt::count(record.hit_path.len()),
        ));
    }
    if let Some(frame) = record.frame {
        facts.push(Fact::new("Drew frame", format!("#{frame}")));
    }
    facts
}

impl Render for EventsLens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.state.read(cx).selected_input();
        let mut overview = LensOverview::new(Lens::Events, LensLayout::of(window))
            .when_empty(IconName::Pick, "No input recorded yet");
        let Some(capture) = window.inspector_capture() else {
            return overview;
        };
        let input = capture.input();
        if app_input(input.iter()).next().is_none() {
            return overview;
        }
        let count = |kinds: &[InputKind]| {
            app_input(input.iter())
                .filter(|record| kinds.contains(&record.kind))
                .count()
        };
        let moves: u32 = app_input(input.iter())
            .filter(|record| record.kind == InputKind::MouseMove)
            .map(|record| record.coalesced.max(1))
            .sum();
        let total = app_input(input.iter()).count();
        let handled = app_input(input.iter())
            .filter(|record| record.handled)
            .count();
        let actions: usize = app_input(input.iter())
            .map(|record| record.actions.len())
            .sum();
        overview = overview.section(
            "Recorded input",
            vec![
                Fact::new("Records", fmt::count(total)),
                Fact::new(
                    "Clicks · keys · scrolls",
                    format!(
                        "{} · {} · {}",
                        fmt::count(count(&[InputKind::MouseDown])),
                        fmt::count(count(&[InputKind::KeyDown])),
                        fmt::count(count(&[InputKind::Scroll]))
                    ),
                ),
                Fact::new("Mouse moves (coalesced)", fmt::count(moves as usize)),
                Fact::new(
                    "Handled",
                    format!("{} of {}", fmt::count(handled), fmt::count(total)),
                ),
                Fact::new("Actions dispatched", fmt::count(actions)),
            ],
        );
        let record = match selected {
            Some(seq) => input.iter().find(|record| record.seq == seq),
            None => app_input(input.iter()).last(),
        };
        if let Some(record) = record {
            let title = if selected.is_some() {
                "Selected event"
            } else {
                "Latest event"
            };
            overview = overview.section(title, input_facts(record));
        }
        overview
    }
}

impl LensView for EventsLens {
    fn rail_badge(&self, window: &Window, _cx: &App) -> Option<RailBadge> {
        let capture = window.inspector_capture()?;
        let count = app_input(capture.input().iter()).count();
        (count > 0).then(|| RailBadge::count(fmt::count(count)))
    }
}
