//! The lens host: five views, one per question Loupe answers.
//!
//! Each lens is its own entity, created once per Loupe and embedded by the
//! shell as a cached view, so a lens re-renders only when it is notified:
//! by its own state changes, or through [`observe_state`] when the shared
//! [`LoupeState`] changes (its generation moves whenever the app records new
//! data). A lens:
//!
//! * is built with `new(state: Entity<LoupeState>, window, cx)` and calls
//!   [`observe_state`] there,
//! * reads the capture in `render` through `window.inspector_capture()`,
//! * reads and updates selection and filters through [`LoupeState`],
//! * implements [`LensView::rail_badge`] for its live count in the rail,
//! * points at the app through the capture's overlay as its own entity
//!   (`cx.entity_id()`), which the host withdraws when the lens is hidden,
//! * lays itself out for [`crate::state::LensLayout::of`] (stacked in narrow
//!   right docks, side by side otherwise).

mod audit;
pub(crate) mod capture;
mod elements;
mod entities;
mod events;
mod frames;
mod highlights;
mod links;
mod memo;

pub(crate) use audit::AuditLens;
pub(crate) use elements::ElementsLens;
pub(crate) use entities::{EntitiesLens, entity_label, entity_names};
pub(crate) use events::EventsLens;
pub use frames::ExportDirectory;
pub(crate) use frames::FramesLens;

use crate::{
    analysis::Severity,
    state::{Lens, LoupeState},
    widgets::{IconName, Tone},
};
use gpui::{
    AnyView, App, AppContext as _, Context, Entity, KeyBinding, Render, SharedString, Window,
};
use std::mem;

/// The lenses with keys of their own, and the key context those apply in.
pub(crate) const KEY_CONTEXTS: [(Lens, &str); 2] = [
    (Lens::Frames, frames::CONTEXT),
    (Lens::Audit, audit::CONTEXT),
];

/// The lenses' own keys (each within its lens' key context).
pub(crate) fn key_bindings() -> impl Iterator<Item = KeyBinding> {
    frames::key_bindings()
        .into_iter()
        .chain(audit::key_bindings())
}

/// The glyph and tone of a finding's severity: circled `i`, triangle,
/// octagon, so severity reads without color too.
pub(crate) fn severity_glyph(severity: Severity) -> (IconName, Tone) {
    match severity {
        Severity::Info => (IconName::Info, Tone::Accent),
        Severity::Warning => (IconName::Warning, Tone::Warn),
        Severity::Critical => (IconName::Critical, Tone::Crit),
    }
}

/// Re-renders the calling view whenever the shared state changes. Cached
/// views are not invalidated by merely reading an entity, so every lens
/// observes the state it depends on.
pub(crate) fn observe_state<T: 'static>(state: &Entity<LoupeState>, cx: &mut Context<T>) {
    cx.observe(state, |_, _, cx| cx.notify()).detach();
}

/// A lens' live count in the rail.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RailBadge {
    /// Short text, e.g. `1,284`.
    pub text: SharedString,
    /// Neutral unless the count signals a problem.
    pub tone: Tone,
    /// A glyph after the count, e.g. ▲ for frames over budget.
    pub marker: Option<IconName>,
}

impl RailBadge {
    /// A neutral count.
    pub fn count(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Neutral,
            marker: None,
        }
    }

    /// A count that signals a problem.
    pub fn alert(text: impl Into<SharedString>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            marker: None,
        }
    }

    /// Adds a glyph after the count.
    pub fn marker(mut self, marker: IconName) -> Self {
        self.marker = Some(marker);
        self
    }
}

/// What the shell needs from every lens besides rendering.
pub(crate) trait LensView: Render {
    /// The rail's live count for this lens. Called whenever the shell
    /// renders, so it must be cheap (memoize on `LoupeState::generation`).
    fn rail_badge(&self, window: &Window, cx: &App) -> Option<RailBadge>;
}

/// The five lens entities, and which one is on screen.
pub(crate) struct Lenses {
    elements: Entity<ElementsLens>,
    frames: Entity<FramesLens>,
    events: Entity<EventsLens>,
    entities: Entity<EntitiesLens>,
    audit: Entity<AuditLens>,
    shown: Lens,
}

impl Lenses {
    /// Creates every lens over the shared `state`.
    pub fn new(state: &Entity<LoupeState>, window: &mut Window, cx: &mut App) -> Self {
        Self {
            elements: cx.new(|cx| ElementsLens::new(state.clone(), window, cx)),
            frames: cx.new(|cx| FramesLens::new(state.clone(), window, cx)),
            events: cx.new(|cx| EventsLens::new(state.clone(), window, cx)),
            entities: cx.new(|cx| EntitiesLens::new(state.clone(), window, cx)),
            audit: cx.new(|cx| AuditLens::new(state.clone(), window, cx)),
            shown: state.read(cx).lens(),
        }
    }

    /// Puts `lens` on screen. Whatever the lens it replaces put on the app's
    /// overlay (highlights, a hovered element) is withdrawn, so nothing a
    /// hidden lens points at lingers over the app, however the lens changed
    /// (a key, the rail, the palette, a link from another lens).
    pub fn show(&mut self, lens: Lens, window: &mut Window) {
        let hidden = mem::replace(&mut self.shown, lens);
        if hidden != lens
            && let Some(capture) = window.inspector_capture_mut()
        {
            capture
                .overlay_mut()
                .withdraw(self.view(hidden).entity_id());
        }
    }

    /// The view of `lens`, for the shell to embed.
    pub fn view(&self, lens: Lens) -> AnyView {
        match lens {
            Lens::Elements => self.elements.clone().into(),
            Lens::Frames => self.frames.clone().into(),
            Lens::Events => self.events.clone().into(),
            Lens::Entities => self.entities.clone().into(),
            Lens::Audit => self.audit.clone().into(),
        }
    }

    /// The rail badge of `lens`.
    pub fn badge(&self, lens: Lens, window: &Window, cx: &App) -> Option<RailBadge> {
        match lens {
            Lens::Elements => self.elements.read(cx).rail_badge(window, cx),
            Lens::Frames => self.frames.read(cx).rail_badge(window, cx),
            Lens::Events => self.events.read(cx).rail_badge(window, cx),
            Lens::Entities => self.entities.read(cx).rail_badge(window, cx),
            Lens::Audit => self.audit.read(cx).rail_badge(window, cx),
        }
    }
}
