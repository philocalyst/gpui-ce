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
    state::{Lens, LensLayout, LoupeState},
    theme::{MONO_FONT, Theme, UI_FONT},
    widgets::{EmptyState, IconName, SectionHeader, Tone},
};
use gpui::{
    AnyView, App, AppContext as _, Context, Entity, FontWeight, IntoElement, Render, RenderOnce,
    SharedString, Styled, Window, div, prelude::*, px,
};

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

/// A labelled value in a lens overview.
pub(crate) struct Fact {
    label: SharedString,
    value: SharedString,
    tone: Tone,
}

impl Fact {
    /// A neutral fact.
    pub fn new(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            tone: Tone::Neutral,
        }
    }
}

/// The overview a lens shows until its full UI lands: the question it
/// answers and the numbers already derivable from the capture.
#[derive(IntoElement)]
pub(crate) struct LensOverview {
    lens: Lens,
    layout: LensLayout,
    sections: Vec<(SharedString, Vec<Fact>)>,
    empty: Option<(IconName, SharedString)>,
}

impl LensOverview {
    /// An overview of `lens`.
    pub fn new(lens: Lens, layout: LensLayout) -> Self {
        Self {
            lens,
            layout,
            sections: Vec::new(),
            empty: None,
        }
    }

    /// Adds a titled group of facts (skipped when empty).
    pub fn section(mut self, title: impl Into<SharedString>, facts: Vec<Fact>) -> Self {
        if !facts.is_empty() {
            self.sections.push((title.into(), facts));
        }
        self
    }

    /// What to show when there is nothing to report yet.
    pub fn when_empty(mut self, icon: IconName, message: impl Into<SharedString>) -> Self {
        self.empty = Some((icon, message.into()));
        self
    }
}

impl RenderOnce for LensOverview {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let intro = div()
            .flex_none()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text)
                    .child(self.lens.label()),
            )
            .child(
                div()
                    .text_color(colors.text_muted)
                    .child(self.lens.question()),
            );

        let facts = if self.sections.is_empty() {
            let (icon, message) = self
                .empty
                .unwrap_or((IconName::Search, "Nothing recorded yet".into()));
            div()
                .flex_1()
                .child(EmptyState::new(message).icon(icon))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .children(
                    self.sections
                        .into_iter()
                        .enumerate()
                        .map(|(ix, (title, facts))| {
                            div()
                                .flex()
                                .flex_col()
                                .child(SectionHeader::new(title).when(ix > 0, |this| this.rule()))
                                .children(facts.into_iter().map(|fact| fact_row(fact, theme)))
                        }),
                )
                .into_any_element()
        };

        let body = div()
            .id("lens-overview")
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .bg(colors.bg)
            .font_family(UI_FONT)
            .text_size(theme.metrics.text)
            .line_height(theme.metrics.line_height);
        match self.layout {
            LensLayout::Stacked => body
                .flex_col()
                .child(intro.border_b_1().border_color(colors.line))
                .child(facts),
            LensLayout::SideBySide => body
                .flex_row()
                .child(
                    intro
                        .w(px(200.))
                        .h_full()
                        .border_r_1()
                        .border_color(colors.line),
                )
                .child(facts),
        }
    }
}

fn fact_row(fact: Fact, theme: &Theme) -> impl IntoElement {
    let colors = &theme.colors;
    div()
        .h(theme.metrics.property_row)
        .px(theme.metrics.gutter)
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_none()
                .w(px(140.))
                .truncate()
                .text_color(colors.text_muted)
                .child(fact.label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(match fact.tone {
                    Tone::Neutral => colors.text,
                    tone => tone.color(theme),
                })
                .child(fact.value),
        )
}
