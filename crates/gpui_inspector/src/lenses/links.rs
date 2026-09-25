//! Links between lenses: every row that names a frame, element, entity,
//! input record or source site leads there in one click.

use crate::{
    analysis::format,
    state::{Lens, LoupeState},
    theme::{MONO_FONT, Theme},
    widgets::{Icon, IconName, Tooltip},
};
use gpui::{
    AnyElement, App, ClipboardItem, Entity, EntityId, IntoElement, SharedString, Styled, Window,
    div, inspector::ElementKey, prelude::*, px,
};
use std::{panic::Location, rc::Rc};

/// Where a link leads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Target {
    /// Shows a frame in Frames.
    Frame(u64),
    /// Selects an element and shows it in Elements.
    Element(ElementKey),
    /// Selects an entity and shows it in Entities.
    Entity(EntityId),
    /// Selects an input record and shows it in Events.
    Input(u64),
    /// Copies `path/to/file.rs:line`.
    Site(&'static Location<'static>),
}

impl Target {
    /// The glyph shown before the link.
    pub fn icon(self) -> IconName {
        match self {
            Target::Frame(_) => IconName::ChevronRight,
            Target::Element(_) => IconName::Pick,
            Target::Entity(_) => IconName::Entity,
            Target::Input(_) => IconName::Command,
            Target::Site(_) => IconName::Source,
        }
    }

    /// What clicking does.
    pub fn tooltip(self) -> Option<SharedString> {
        Some(match self {
            Target::Frame(_) => "Show this frame".into(),
            Target::Element(_) => "Select it and show it in Elements".into(),
            Target::Entity(_) => "Show it in Entities".into(),
            Target::Input(_) => "Show it in Events".into(),
            Target::Site(site) => format!("Copy {}:{}", site.file(), site.line()).into(),
        })
    }
}

/// Runs a link from a lens view.
pub(crate) type Navigate = Rc<dyn Fn(Target, &mut Window, &mut App)>;

/// Follows `target`: selects and switches lenses through `state`, or
/// copies a site. Returns what to tell the user, if anything.
pub(crate) fn follow(target: Target, state: &Entity<LoupeState>, cx: &mut App) -> Option<String> {
    match target {
        Target::Frame(frame) => state.update(cx, |state, cx| {
            state.select_frame(Some(frame), cx);
            state.set_lens(Lens::Frames, cx);
        }),
        Target::Element(element) => state.update(cx, |state, cx| {
            state.select_element(Some(element), cx);
            state.set_lens(Lens::Elements, cx);
        }),
        Target::Entity(entity) => state.update(cx, |state, cx| {
            state.select_entity(Some(entity), cx);
            state.set_lens(Lens::Entities, cx);
        }),
        Target::Input(seq) => state.update(cx, |state, cx| {
            state.select_input(Some(seq), cx);
            state.set_lens(Lens::Events, cx);
        }),
        Target::Site(site) => {
            let path = format!("{}:{}", site.file(), site.line());
            cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
            return Some(format!("Copied {path}"));
        }
    }
    None
}

/// A small inline link with a glyph, e.g. `⟨⟩ issue_list.rs:52`.
pub(crate) fn link(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    target: Target,
    navigate: &Navigate,
    theme: &Theme,
) -> AnyElement {
    let navigate = navigate.clone();
    let id = id.into();
    let selector = id.to_string();
    let mono = matches!(target, Target::Site(_));
    div()
        .id(id)
        .debug_selector(move || selector)
        .flex_none()
        .h(px(18.))
        .px(px(4.))
        .flex()
        .items_center()
        .gap(px(3.))
        .rounded(theme.metrics.radius)
        .text_size(theme.metrics.text_small)
        .text_color(theme.colors.accent)
        .cursor_pointer()
        .hover(|style| style.bg(theme.colors.hover))
        .when(mono, |this| {
            this.font_family(MONO_FONT).text_size(theme.metrics.mono)
        })
        .child(
            Icon::new(target.icon())
                .size(theme.metrics.icon_small)
                .color(theme.colors.accent),
        )
        .child(label.into())
        .when_some(target.tooltip(), |this, tooltip| {
            this.tooltip(Tooltip::text(tooltip))
        })
        .on_click(move |_, window, cx| navigate(target, window, cx))
        .into_any_element()
}

/// `issue_list.rs:52`, as a link that copies the full path.
pub(crate) fn site_link(
    id: impl Into<SharedString>,
    site: &'static Location<'static>,
    navigate: &Navigate,
    theme: &Theme,
) -> AnyElement {
    link(
        id,
        format::location(site),
        Target::Site(site),
        navigate,
        theme,
    )
}
