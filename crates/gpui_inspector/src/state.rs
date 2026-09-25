//! State shared by Loupe's shell and its lenses.
//!
//! One [`LoupeState`] entity exists per open Loupe. The shell and every lens
//! read it during render (so they re-render when it changes) and update it
//! through its setters, which notify only on real changes. The capture itself
//! is never copied here: views read `window.inspector_capture()` directly.

use gpui::{
    Bounds, Context, EntityId, Pixels, SharedString, Window,
    inspector::{ElementKey, InspectorDock},
    px,
};

/// The five lenses, in rail order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lens {
    /// What is this, where did it come from, why does it look like that?
    #[default]
    Elements,
    /// Why was that frame drawn, and why was it slow?
    Frames,
    /// Where did my click go? What will this key do here?
    Events,
    /// What's alive, who's watching it, who keeps poking it?
    Entities,
    /// What should I fix first?
    Audit,
}

impl Lens {
    /// Every lens, in rail order.
    pub const ALL: [Lens; 5] = [
        Lens::Elements,
        Lens::Frames,
        Lens::Events,
        Lens::Entities,
        Lens::Audit,
    ];

    /// The rail label.
    pub fn label(self) -> &'static str {
        match self {
            Lens::Elements => "Elements",
            Lens::Frames => "Frames",
            Lens::Events => "Events",
            Lens::Entities => "Entities",
            Lens::Audit => "Audit",
        }
    }

    /// The question the lens answers.
    pub fn question(self) -> &'static str {
        match self {
            Lens::Elements => "What is this, where did it come from, why does it look like that?",
            Lens::Frames => "Why was that frame drawn, and why was it slow?",
            Lens::Events => "Where did my click go? What will this key do here?",
            Lens::Entities => "What's alive, who's watching it, who keeps poking it?",
            Lens::Audit => "What should I fix first?",
        }
    }

    /// Position in the rail (0-based); `alt-1` shows index 0.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Per-lens filter text, kept across lens switches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    /// Elements tree filter.
    pub elements: SharedString,
    /// Events log filter.
    pub events: SharedString,
    /// Entities table filter.
    pub entities: SharedString,
    /// Audit findings filter.
    pub audit: SharedString,
}

/// How a lens should arrange its master and detail panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LensLayout {
    /// Master above detail, for narrow right docks.
    Stacked,
    /// Master beside detail, for wide right docks and bottom docks.
    SideBySide,
}

/// Right docks narrower than this stack their lenses.
pub const STACK_BELOW_WIDTH: Pixels = px(520.);

/// The layout for a dock of `bounds` docked at `dock`.
pub fn lens_layout(dock: InspectorDock, bounds: Bounds<Pixels>) -> LensLayout {
    match dock {
        InspectorDock::Bottom { .. } => LensLayout::SideBySide,
        InspectorDock::Right { .. } if bounds.size.width < STACK_BELOW_WIDTH => LensLayout::Stacked,
        InspectorDock::Right { .. } => LensLayout::SideBySide,
    }
}

impl LensLayout {
    /// The layout for Loupe's dock in `window` (stacked when closed).
    pub fn of(window: &Window) -> Self {
        match (window.inspector_capture(), window.inspector_bounds()) {
            (Some(capture), Some(bounds)) => lens_layout(capture.dock(), bounds),
            _ => LensLayout::Stacked,
        }
    }
}

/// Selection, active lens and filters, shared by the shell and the lenses.
#[derive(Debug, Default)]
pub struct LoupeState {
    lens: Lens,
    selected_frame: Option<u64>,
    selected_element: Option<ElementKey>,
    selected_entity: Option<EntityId>,
    selected_input: Option<u64>,
    filters: Filters,
    generation: u64,
}

impl LoupeState {
    /// Fresh state: Elements lens, nothing selected.
    pub fn new() -> Self {
        Self::default()
    }

    /// The lens shown in the body.
    pub fn lens(&self) -> Lens {
        self.lens
    }

    /// The selected frame id. `None` follows the latest frame.
    pub fn selected_frame(&self) -> Option<u64> {
        self.selected_frame
    }

    /// The selected element.
    pub fn selected_element(&self) -> Option<ElementKey> {
        self.selected_element
    }

    /// The selected entity.
    pub fn selected_entity(&self) -> Option<EntityId> {
        self.selected_entity
    }

    /// The selected input record's sequence number.
    pub fn selected_input(&self) -> Option<u64> {
        self.selected_input
    }

    /// Filter text per lens.
    pub fn filters(&self) -> &Filters {
        &self.filters
    }

    /// The capture generation the UI last caught up with. Lenses key their
    /// memoized derived data on it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Shows `lens`.
    pub fn set_lens(&mut self, lens: Lens, cx: &mut Context<Self>) {
        self.set(|state| &mut state.lens, lens, cx);
    }

    /// Selects a frame (`None` follows the latest).
    pub fn select_frame(&mut self, frame: Option<u64>, cx: &mut Context<Self>) {
        self.set(|state| &mut state.selected_frame, frame, cx);
    }

    /// Selects an element.
    pub fn select_element(&mut self, element: Option<ElementKey>, cx: &mut Context<Self>) {
        self.set(|state| &mut state.selected_element, element, cx);
    }

    /// Selects an entity.
    pub fn select_entity(&mut self, entity: Option<EntityId>, cx: &mut Context<Self>) {
        self.set(|state| &mut state.selected_entity, entity, cx);
    }

    /// Selects an input record.
    pub fn select_input(&mut self, input: Option<u64>, cx: &mut Context<Self>) {
        self.set(|state| &mut state.selected_input, input, cx);
    }

    /// Replaces the filters.
    pub fn set_filters(&mut self, filters: Filters, cx: &mut Context<Self>) {
        self.set(|state| &mut state.filters, filters, cx);
    }

    /// Records the capture's generation; notifies (so every view re-renders)
    /// only when it moved. Returns whether it did.
    pub fn catch_up(&mut self, generation: u64, cx: &mut Context<Self>) -> bool {
        let changed = self.generation != generation;
        self.set(|state| &mut state.generation, generation, cx);
        changed
    }

    fn set<T: PartialEq>(
        &mut self,
        field: impl FnOnce(&mut Self) -> &mut T,
        value: T,
        cx: &mut Context<Self>,
    ) {
        let slot = field(self);
        if *slot != value {
            *slot = value;
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    #[test]
    fn narrow_right_docks_stack_and_wide_or_bottom_docks_do_not() {
        let bounds = |width| Bounds::new(point(px(0.), px(0.)), size(px(width), px(700.)));
        let right = InspectorDock::Right { width: px(560.) };
        let bottom = InspectorDock::Bottom { height: px(300.) };
        assert_eq!(lens_layout(right, bounds(400.)), LensLayout::Stacked);
        assert_eq!(lens_layout(right, bounds(519.)), LensLayout::Stacked);
        assert_eq!(lens_layout(right, bounds(520.)), LensLayout::SideBySide);
        assert_eq!(lens_layout(bottom, bounds(400.)), LensLayout::SideBySide);
    }

    #[test]
    fn lenses_are_indexed_in_rail_order() {
        for (ix, lens) in Lens::ALL.iter().enumerate() {
            assert_eq!(lens.index(), ix);
        }
    }
}
