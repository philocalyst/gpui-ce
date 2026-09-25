//! Audit: what should I fix first?

use super::{Fact, LensOverview, LensView, RailBadge, observe_state};
use crate::{
    analysis::format,
    state::{Lens, LensLayout, LoupeState},
    widgets::{IconName, Tone},
};
use gpui::{
    App, Context, Entity, IntoElement, Render, Window,
    inspector::{ElementFlags, ElementTree},
};

/// The Audit lens (overview until the rule engine lands).
pub(crate) struct AuditLens;

impl AuditLens {
    pub fn new(state: Entity<LoupeState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        Self
    }
}

/// Element flags that already point at a likely problem, counted over one tree.
#[derive(Debug, Default, PartialEq)]
struct FlagCounts {
    overflowing: usize,
    clickable_without_keyboard: usize,
    empty_hitboxes: usize,
}

impl FlagCounts {
    fn of(tree: &ElementTree) -> Self {
        let mut counts = Self::default();
        for record in &tree.elements {
            let flags = record.flags;
            if flags.contains(ElementFlags::OVERFLOWS_PARENT) {
                counts.overflowing += 1;
            }
            if flags.contains(ElementFlags::CLICKABLE)
                && !flags.intersects(ElementFlags::FOCUSABLE | ElementFlags::KEYBOARD)
            {
                counts.clickable_without_keyboard += 1;
            }
            if flags.contains(ElementFlags::HITBOX)
                && flags.intersects(ElementFlags::CLICKABLE | ElementFlags::DRAG_DROP)
                && (record.bounds.size.width <= gpui::px(0.)
                    || record.bounds.size.height <= gpui::px(0.))
            {
                counts.empty_hitboxes += 1;
            }
        }
        counts
    }

    fn total(&self) -> usize {
        self.overflowing + self.clickable_without_keyboard + self.empty_hitboxes
    }
}

fn warn_if_any(count: usize) -> Tone {
    if count > 0 { Tone::Warn } else { Tone::Neutral }
}

impl Render for AuditLens {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut overview = LensOverview::new(Lens::Audit, LensLayout::of(window))
            .when_empty(IconName::Warning, "Audits run over the latest element tree");
        let Some(tree) = window
            .inspector_capture()
            .and_then(|capture| capture.latest_tree())
        else {
            return overview;
        };
        let counts = FlagCounts::of(tree);
        overview = overview.section(
            "Flagged in the latest tree",
            vec![
                Fact::new(
                    "Overflowing parent",
                    format::count(counts.overflowing as u64),
                )
                .tone(warn_if_any(counts.overflowing)),
                Fact::new(
                    "Clickable, no keyboard",
                    format::count(counts.clickable_without_keyboard as u64),
                )
                .tone(warn_if_any(counts.clickable_without_keyboard)),
                Fact::new(
                    "Empty hitboxes",
                    format::count(counts.empty_hitboxes as u64),
                )
                .tone(warn_if_any(counts.empty_hitboxes)),
            ],
        );
        overview
    }
}

impl LensView for AuditLens {
    fn rail_badge(&self, window: &Window, _cx: &App) -> Option<RailBadge> {
        let tree = window.inspector_capture()?.latest_tree()?;
        match FlagCounts::of(tree).total() {
            0 => None,
            count => Some(RailBadge::alert(format::count(count as u64), Tone::Warn)),
        }
    }
}
