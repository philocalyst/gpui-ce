//! Elements: what is this, where did it come from, why does it look like that?

use super::{Fact, LensOverview, LensView, RailBadge, observe_state};
use crate::{
    analysis::format,
    state::{Lens, LensLayout, LoupeState},
    widgets::IconName,
};
use gpui::{
    App, Context, Entity, IntoElement, Render, Window,
    inspector::{ElementFlags, ElementKind, ElementTree},
};

/// The Elements lens (overview until the tree and detail land).
pub(crate) struct ElementsLens {
    state: Entity<LoupeState>,
}

impl ElementsLens {
    pub fn new(state: Entity<LoupeState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        Self { state }
    }
}

struct TreeCounts {
    elements: usize,
    views: usize,
    components: usize,
    depth: u16,
    overridden: usize,
}

fn tree_counts(tree: &ElementTree) -> TreeCounts {
    let mut counts = TreeCounts {
        elements: tree.elements.len(),
        views: 0,
        components: 0,
        depth: 0,
        overridden: 0,
    };
    for record in &tree.elements {
        match record.kind {
            ElementKind::View { .. } => counts.views += 1,
            ElementKind::Component { .. } => counts.components += 1,
            ElementKind::Element { .. } => {}
        }
        counts.depth = counts.depth.max(record.depth);
        if record.flags.contains(ElementFlags::OVERRIDDEN) {
            counts.overridden += 1;
        }
    }
    counts
}

impl Render for ElementsLens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.state.read(cx).selected_element();
        let mut overview = LensOverview::new(Lens::Elements, LensLayout::of(window))
            .when_empty(IconName::Pick, "No element tree captured yet");
        let Some(capture) = window.inspector_capture() else {
            return overview;
        };
        if let Some(tree) = capture.latest_tree() {
            let counts = tree_counts(tree);
            overview = overview.section(
                "Latest tree",
                vec![
                    Fact::new("Captured in frame", format!("#{}", tree.frame)),
                    Fact::new("Elements", format::count(counts.elements as u64)),
                    Fact::new("Views", format::count(counts.views as u64)),
                    Fact::new("Components", format::count(counts.components as u64)),
                    Fact::new("Deepest nesting", counts.depth.to_string()),
                    Fact::new("Style overrides", format::count(counts.overridden as u64)),
                ],
            );
            let selection = selected.and_then(|key| {
                let ix = tree.find(key)?;
                let record = tree.get(ix)?;
                let mut facts = vec![Fact::new("Element", format::element_label(record))];
                if let Some(info) = capture.path_info(key.path) {
                    facts.push(Fact::new("Source", format::location(info.source)));
                }
                let size = record.bounds.size;
                facts.push(Fact::new(
                    "Size",
                    format!("{} × {}", f32::from(size.width), f32::from(size.height)),
                ));
                if let Some(view) = tree
                    .owning_view(ix)
                    .filter(|&view| view != ix)
                    .and_then(|view| tree.get(view))
                {
                    facts.push(Fact::new("Owning view", format::element_label(view)));
                }
                Some(facts)
            });
            overview = overview.section("Selection", selection.unwrap_or_default());
        }
        overview
    }
}

impl LensView for ElementsLens {
    fn rail_badge(&self, window: &Window, _cx: &App) -> Option<RailBadge> {
        let capture = window.inspector_capture()?;
        let count = match capture.latest_tree() {
            Some(tree) => tree.elements.len(),
            None => capture.latest_app_frame()?.element_count as usize,
        };
        Some(RailBadge::count(format::count(count as u64)))
    }
}
