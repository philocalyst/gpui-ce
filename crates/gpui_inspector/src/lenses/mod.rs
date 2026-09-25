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
//! * lays itself out for [`crate::state::LensLayout::of`] (stacked in narrow
//!   right docks, side by side otherwise).

mod audit;
mod capture;
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
use gpui::{AnyView, App, AppContext as _, Context, Entity, Render, SharedString, Window};

/// Binds the lenses' own keys (each within its lens' key context).
pub(crate) fn bind_keys(cx: &mut App) {
    frames::bind_keys(cx);
    audit::bind_keys(cx);
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

/// The five lens entities.
pub(crate) struct Lenses {
    elements: Entity<ElementsLens>,
    frames: Entity<FramesLens>,
    events: Entity<EventsLens>,
    entities: Entity<EntitiesLens>,
    audit: Entity<AuditLens>,
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
