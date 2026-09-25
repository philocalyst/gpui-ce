//! Entities: what's alive, who's watching it, who keeps poking it?

use super::{Fact, LensOverview, LensView, RailBadge, observe_state};
use crate::{
    analysis::format,
    state::{Lens, LensLayout, LoupeState},
    widgets::IconName,
};
use gpui::{
    App, Context, Entity, EntityId, IntoElement, Render, Window,
    inspector::{CauseKind, EntityInfo, InspectorCapture, short_type_name},
};
use std::collections::HashMap;

/// An entity id as its slot number, `#6` (the version bits are noise to a reader).
pub(crate) fn entity_label(id: EntityId) -> String {
    format!("#{}", id.as_u64() & 0xffff_ffff)
}

/// Short type names for every entity Loupe has seen: live entities from the
/// registry, views from recorded view spans, and notified entities from
/// recorded causes.
pub(crate) fn entity_names(
    capture: &InspectorCapture,
    live: &[EntityInfo],
) -> HashMap<EntityId, &'static str> {
    let mut names = HashMap::new();
    for frame in capture.frames() {
        for view in &frame.views {
            names.insert(view.entity, short_type_name(view.type_name));
        }
        for cause in &frame.causes {
            if let CauseKind::Notify {
                entity,
                type_name: Some(type_name),
            } = cause.kind
            {
                names.insert(entity, short_type_name(type_name));
            }
        }
    }
    for entity in live {
        names.insert(entity.id, short_type_name(entity.type_name));
    }
    names
}

/// The Entities lens (overview until the table lands).
pub(crate) struct EntitiesLens {
    state: Entity<LoupeState>,
}

impl EntitiesLens {
    pub fn new(state: Entity<LoupeState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        Self { state }
    }
}

impl Render for EntitiesLens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.state.read(cx).selected_entity();
        let live = window.inspector_entities(cx);
        let mut overview = LensOverview::new(Lens::Entities, LensLayout::of(window))
            .when_empty(IconName::Entity, "No entity activity recorded yet");
        let Some(capture) = window.inspector_capture() else {
            return overview;
        };
        let stats = capture.notify_stats();
        let views = live.iter().filter(|entity| entity.is_view).count();
        let notifies: u64 = stats.values().map(|stats| stats.total).sum();
        let mut facts = Vec::new();
        if !live.is_empty() {
            facts.push(Fact::new("Live entities", format::count(live.len() as u64)));
            facts.push(Fact::new("Rendered as views", format::count(views as u64)));
        }
        if !stats.is_empty() {
            facts.push(Fact::new(
                "Entities that notified",
                format::count(stats.len() as u64),
            ));
            facts.push(Fact::new("Notifies recorded", format::count(notifies)));
        }
        overview = overview.section("Registry", facts);

        let mut busiest: Vec<_> = stats.iter().collect();
        busiest.sort_by_key(|(id, stats)| (std::cmp::Reverse(stats.total), **id));
        let names = entity_names(capture, &live);
        let name = |id| names.get(&id).copied().unwrap_or("Entity");
        let busiest_facts = busiest
            .into_iter()
            .take(5)
            .map(|(id, stats)| {
                let site = stats
                    .last_site
                    .map(|site| format!(" · {}", format::location(site)))
                    .unwrap_or_default();
                Fact::new(
                    format!("{} {}", name(*id), entity_label(*id)),
                    format!("{} notifies{site}", format::count(stats.total)),
                )
            })
            .collect();
        overview = overview.section("Most notified", busiest_facts);

        if let Some(id) = selected {
            let facts = match live.iter().find(|entity| entity.id == id) {
                Some(entity) => vec![
                    Fact::new("Type", entity.type_name),
                    Fact::new("Strong handles", format::count(entity.strong_count as u64)),
                    Fact::new(
                        "Observers · subscribers",
                        format!("{} · {}", entity.observers, entity.subscribers),
                    ),
                ],
                None => vec![Fact::new(
                    "Entity",
                    format!("{} {}", name(id), entity_label(id)),
                )],
            };
            overview = overview.section("Selected entity", facts);
        }
        overview
    }
}

impl LensView for EntitiesLens {
    fn rail_badge(&self, window: &Window, cx: &App) -> Option<RailBadge> {
        // Live entities when the registry reports them, otherwise the
        // entities seen notifying while recording.
        let live = window.inspector_entities(cx).len();
        let count = if live > 0 {
            live
        } else {
            window.inspector_capture()?.notify_stats().len()
        };
        (count > 0).then(|| RailBadge::count(format::count(count as u64)))
    }
}
