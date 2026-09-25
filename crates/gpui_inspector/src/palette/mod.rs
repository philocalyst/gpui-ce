//! The command palette: fuzzy search across commands (`>`), elements (`#`)
//! and entities (`@`), opened with `cmd-k` / `ctrl-k`.
//!
//! The palette is a floating layer over Loupe. It snapshots its candidates
//! when opened, ranks them with [`fuzzy::rank`] as the query changes and
//! emits [`PaletteEvent::Confirmed`] with the chosen target; the shell runs it.

pub mod fuzzy;

use crate::{
    commands::Command,
    theme::{MONO_FONT, Theme, UI_FONT},
    widgets::{Icon, IconName, Kbd, TextField, floating_surface},
};
use fuzzy::{FuzzyMatch, highlight_ranges, rank};
use gpui::{
    AnyElement, App, Context, EntityId, EventEmitter, FocusHandle, Focusable, FontWeight,
    HighlightStyle, Hsla, IntoElement, Render, ScrollStrategy, SharedString, StyledText,
    Subscription, UniformListScrollHandle, Window, div, inspector::ElementKey, prelude::*, px,
    uniform_list,
};
use gpui_elements::editable_text::{
    EditableTextState, StringStorage, TextChanged,
    actions::{Enter, Escape, NavDown, NavUp},
};

/// What a palette row does when chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PaletteTarget {
    /// Run a command.
    Command(Command),
    /// Select and reveal an element.
    Element(ElementKey),
    /// Select and reveal an entity.
    Entity(EntityId),
}

/// The glyph drawn before a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteGlyph {
    /// A command.
    Command,
    /// A stateful view.
    View,
    /// A `RenderOnce` component.
    Component,
    /// Any other element.
    Element,
    /// An entity.
    Entity,
}

/// One candidate row.
#[derive(Clone, Debug)]
pub struct PaletteItem {
    /// What choosing it does.
    pub target: PaletteTarget,
    /// The matched text.
    pub label: SharedString,
    /// Muted context after the label (a source location, an entity id...).
    pub detail: Option<SharedString>,
    /// Glyph before the label.
    pub glyph: PaletteGlyph,
    /// Key binding hint, in gpui syntax.
    pub keys: Option<&'static str>,
    /// For toggles and choices: whether it is on (shown as a check).
    pub checked: Option<bool>,
}

/// Which candidates a query searches, from its first character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// No prefix: commands, elements and entities.
    All,
    /// `>`: commands only.
    Commands,
    /// `#`: elements only.
    Elements,
    /// `@`: entities only.
    Entities,
}

impl Scope {
    /// Splits a raw query into its scope and the text to match.
    pub fn parse(query: &str) -> (Scope, &str) {
        let query = query.trim_start();
        match query.chars().next() {
            Some('>') => (Scope::Commands, &query[1..]),
            Some('#') => (Scope::Elements, &query[1..]),
            Some('@') => (Scope::Entities, &query[1..]),
            _ => (Scope::All, query),
        }
    }

    fn admits(self, item: &PaletteItem, text: &str) -> bool {
        match (self, item.target) {
            (Scope::Commands, PaletteTarget::Command(_)) => true,
            (Scope::Elements, PaletteTarget::Element(_)) => true,
            (Scope::Entities, PaletteTarget::Entity(_)) => true,
            // An empty unscoped query lists commands only, not every element.
            (Scope::All, PaletteTarget::Command(_)) => true,
            (Scope::All, _) => !text.trim().is_empty(),
            _ => false,
        }
    }
}

/// Matches `query` against `items`: `(item index, match)`, best first,
/// capped at `limit`.
pub fn search(items: &[PaletteItem], query: &str, limit: usize) -> Vec<(usize, FuzzyMatch)> {
    let (scope, text) = Scope::parse(query);
    let admitted: Vec<usize> = (0..items.len())
        .filter(|&ix| scope.admits(&items[ix], text))
        .collect();
    let mut ranked = rank(text, admitted.iter().map(|&ix| items[ix].label.as_ref()));
    ranked.truncate(limit);
    ranked
        .into_iter()
        .map(|(ix, found)| (admitted[ix], found))
        .collect()
}

/// What the palette tells the shell.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteEvent {
    /// A row was chosen.
    Confirmed(PaletteTarget),
    /// Escape, or a click outside.
    Dismissed,
}

const MAX_RESULTS: usize = 200;
const VISIBLE_ROWS: usize = 9;
const ROW_HEIGHT: f32 = 28.;

/// The palette view.
pub struct Palette {
    items: Vec<PaletteItem>,
    query: gpui::Entity<EditableTextState>,
    matches: Vec<(usize, FuzzyMatch)>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _query_changes: Subscription,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Focusable for Palette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.query.read(cx).focus_handle(cx)
    }
}

impl Palette {
    /// A palette over `items`, with the query prefilled (e.g. `">"`) and focused.
    pub fn new(
        items: Vec<PaletteItem>,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query_state = cx.new(|cx| {
            let mut state = EditableTextState::new(StringStorage::from(query), cx);
            state.move_to(query.len(), cx);
            state
        });
        let changes = cx.subscribe(&query_state, |this, _, _: &TextChanged, cx| {
            this.update_matches(cx);
        });
        let focus = query_state.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        let mut palette = Self {
            items,
            query: query_state,
            matches: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            _query_changes: changes,
        };
        palette.update_matches(cx);
        palette
    }

    /// Labels of the current matches, best first.
    pub fn match_labels(&self) -> Vec<SharedString> {
        self.matches
            .iter()
            .map(|(ix, _)| self.items[*ix].label.clone())
            .collect()
    }

    fn update_matches(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).as_str().to_string();
        self.matches = search(&self.items, &query, MAX_RESULTS);
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }
        let last = self.matches.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn confirm(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some((item, _)) = self.matches.get(ix) {
            cx.emit(PaletteEvent::Confirmed(self.items[*item].target));
        }
    }

    fn render_row(&self, ix: usize, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (item_ix, found) = &self.matches[ix];
        let item = &self.items[*item_ix];
        let colors = &theme.colors;
        let selected = ix == self.selected;
        let (glyph, glyph_color) = glyph(item.glyph, theme);
        let highlights = highlight_ranges(&item.label, &found.positions)
            .into_iter()
            .map(|range| {
                (
                    range,
                    HighlightStyle {
                        color: Some(colors.accent),
                        font_weight: Some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    },
                )
            });
        let label = StyledText::new(item.label.clone()).with_highlights(highlights);
        div()
            .id(ix)
            .h(px(ROW_HEIGHT))
            .w_full()
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(theme.metrics.radius)
            .when(selected, |this| this.bg(colors.selected))
            .when(!selected, |this| this.hover(|style| style.bg(colors.hover)))
            .cursor_pointer()
            .child(
                Icon::new(glyph)
                    .size(theme.metrics.icon_small)
                    .color(glyph_color),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(280.))
                    .truncate()
                    .text_color(colors.text)
                    .child(label),
            )
            .children(item.detail.clone().map(|detail| {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(theme.metrics.mono)
                    .text_color(colors.text_faint)
                    .child(detail)
            }))
            .when(item.detail.is_none(), |this| this.child(div().flex_1()))
            .children(item.checked.filter(|on| *on).map(|_| {
                Icon::new(IconName::Check)
                    .size(theme.metrics.icon_small)
                    .color(colors.accent)
            }))
            .children(item.keys.map(Kbd::new))
            .on_click(cx.listener(move |this, _, _, cx| this.confirm(ix, cx)))
            .into_any_element()
    }
}

fn glyph(glyph: PaletteGlyph, theme: &Theme) -> (IconName, Hsla) {
    let colors = &theme.colors;
    match glyph {
        PaletteGlyph::Command => (IconName::Command, colors.text_muted),
        PaletteGlyph::View => (IconName::View, colors.view),
        PaletteGlyph::Component => (IconName::Component, colors.component),
        PaletteGlyph::Element => (IconName::ElementDot, colors.text_faint),
        PaletteGlyph::Entity => (IconName::Entity, colors.text_muted),
    }
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let rows = self.matches.len();
        let list_height = px(ROW_HEIGHT) * rows.clamp(1, VISIBLE_ROWS) as f32 + px(8.);
        let entity = cx.entity();
        let results = if rows == 0 {
            div()
                .h(list_height)
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.text_faint)
                .child("No matches")
                .into_any_element()
        } else {
            uniform_list("palette-results", rows, move |range, window, cx| {
                let theme = Theme::of(window, cx);
                entity.update(cx, |palette, cx| {
                    range
                        .map(|ix| palette.render_row(ix, theme, cx))
                        .collect::<Vec<_>>()
                })
            })
            .p(px(4.))
            .h(list_height)
            .track_scroll(&self.scroll)
            .into_any_element()
        };

        floating_surface(theme)
            .id("loupe-palette")
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_size(theme.metrics.text)
            .text_color(colors.text)
            .occlude()
            .capture_action(cx.listener(|this, _: &NavDown, _, cx| {
                this.move_selection(1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &NavUp, _, cx| {
                this.move_selection(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &Enter, _, cx| {
                this.confirm(this.selected, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.emit(PaletteEvent::Dismissed);
                cx.stop_propagation();
            }))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Dismissed)))
            .child(
                div()
                    .h(px(36.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.line)
                    .child(
                        div().flex_1().child(
                            TextField::new("palette-query", &self.query)
                                .icon(IconName::Search)
                                .placeholder("Find commands, #elements, @entities…")
                                .trailing(Kbd::new("escape"))
                                .borderless(),
                        ),
                    ),
            )
            .child(results)
            .child(
                div()
                    .h(px(24.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .border_t_1()
                    .border_color(colors.line)
                    .font_family(UI_FONT)
                    .text_size(theme.metrics.text_small)
                    .text_color(colors.text_faint)
                    .child("↑↓ move · ↩ run")
                    .child(div().flex_1())
                    .child("> commands   # elements   @ entities"),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Lens;

    fn command(command: Command) -> PaletteItem {
        PaletteItem {
            target: PaletteTarget::Command(command),
            label: command.label().into(),
            detail: None,
            glyph: PaletteGlyph::Command,
            keys: command.keys(),
            checked: None,
        }
    }

    fn entity(id: u64, label: &'static str) -> PaletteItem {
        PaletteItem {
            target: PaletteTarget::Entity(EntityId::from(id)),
            label: label.into(),
            detail: None,
            glyph: PaletteGlyph::Entity,
            keys: None,
            checked: None,
        }
    }

    fn labels(items: &[PaletteItem], query: &str) -> Vec<SharedString> {
        search(items, query, 10)
            .into_iter()
            .map(|(ix, _)| items[ix].label.clone())
            .collect()
    }

    #[test]
    fn prefixes_pick_a_scope() {
        assert_eq!(Scope::parse(">fr"), (Scope::Commands, "fr"));
        assert_eq!(Scope::parse("#row"), (Scope::Elements, "row"));
        assert_eq!(Scope::parse(" @store"), (Scope::Entities, "store"));
        assert_eq!(Scope::parse("freeze"), (Scope::All, "freeze"));
    }

    #[test]
    fn empty_queries_list_commands_and_scoped_queries_filter_kinds() {
        let items = vec![
            command(Command::ToggleFreeze),
            command(Command::ShowLens(Lens::Frames)),
            entity(3, "FrameStore"),
        ];
        assert_eq!(labels(&items, ""), ["Freeze recording", "Show Frames"]);
        assert_eq!(
            labels(&items, "fr"),
            ["FrameStore", "Freeze recording", "Show Frames"]
        );
        assert_eq!(labels(&items, "@fr"), ["FrameStore"]);
        assert_eq!(labels(&items, ">store"), Vec::<SharedString>::new());
    }
}
