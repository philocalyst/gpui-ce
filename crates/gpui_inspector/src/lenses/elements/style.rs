//! The Style section: the selected element's style as an editable grid.
//!
//! Rows come from [`style_grid::rows`] over the element's own style (as its
//! code wrote it, from `selected_style()`) with Loupe's override on top.
//! Numbers scrub, colors take hex, enums cycle, flags toggle; every edit
//! rewrites the override for the element's path and the app relayouts on
//! its next frame. *Copy Rust* turns the override into builder calls.

use super::ElementsLens;
use crate::{
    analysis::{
        format,
        rust_patch::{self, DiffKind, DiffLine, RustPatch},
        style_grid::{self, PropertyGroup, PropertyRow, PropertyValue},
    },
    fuzzy::fuzzy_match,
    theme::{MONO_FONT, Theme},
    widgets::{
        Button, ButtonSize, ButtonStyle, ColorSwatch, Icon, IconName, ScrubChanged, ScrubField,
        TextField, Tooltip, text_field_state,
    },
};
use gpui::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, Focusable as _, Hsla, IntoElement,
    SharedString, StyleRefinement, Subscription, Window, div, inspector::PathKey, prelude::*, px,
    rgb_to_hsla, rgba,
};
use gpui_elements::editable_text::{EditableTextState, StringStorage, TextChanged};
use std::collections::HashMap;

/// Most matches the "Add property" list shows.
const ADD_MATCHES: usize = 8;

/// The selected element's style: what its code wrote, what Loupe overrides,
/// and the rows the grid shows.
#[derive(Clone, Debug)]
pub(super) struct StyleModel {
    /// The element path overrides are keyed by.
    pub path: PathKey,
    /// The element's own style; `None` until a frame drew the selection.
    pub base: Option<StyleRefinement>,
    /// How many properties Loupe's override sets.
    pub overridden: usize,
    /// Every set property of the base with the override applied, by group.
    pub rows: Vec<StyleRow>,
    /// The builder calls "Copy Rust" copies: the calls to append for the
    /// override, or the whole style when nothing is overridden.
    pub patch: RustPatch,
    /// The base's calls against the overridden ones, line by line.
    pub diff: Vec<DiffLine>,
    /// How many elements share the path (and so the override).
    pub instances: usize,
}

/// One grid row.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StyleRow {
    /// The property and its effective value.
    pub row: PropertyRow,
    /// Whether Loupe's override sets it.
    pub overridden: bool,
}

impl StyleModel {
    /// The grid for `base` (if captured) with `overrides` applied.
    pub fn new(
        path: PathKey,
        base: Option<StyleRefinement>,
        overrides: StyleRefinement,
        instances: usize,
    ) -> Self {
        let overridden: Vec<String> = style_grid::rows(&overrides)
            .into_iter()
            .map(|row| row.path)
            .collect();
        let base_style = base.clone().unwrap_or_default();
        let merged = style_grid::merge(&base_style, &overrides);
        let mut rows: Vec<StyleRow> = style_grid::rows(&merged)
            .into_iter()
            .map(|row| StyleRow {
                overridden: overridden.contains(&row.path),
                row,
            })
            .collect();
        // One heading per group; the style's own field order within it.
        rows.sort_by_key(|row| row.row.group);
        let has_overrides = !overridden.is_empty();
        let patch = if has_overrides {
            rust_patch::rust_patch(&base_style, &overrides)
        } else {
            rust_patch::builder_calls(&merged)
        };
        let diff = if has_overrides {
            rust_patch::patch_diff(&base_style, &overrides)
        } else {
            Vec::new()
        };
        Self {
            path,
            base,
            overridden: overridden.len(),
            rows,
            patch,
            diff,
            instances,
        }
    }

    /// Whether Loupe overrides anything.
    pub fn has_overrides(&self) -> bool {
        self.overridden > 0
    }

    /// The "Copy patch" text: changed lines as `- …` / `+ …`.
    pub fn patch_text(&self) -> String {
        rust_patch::diff_text(&self.diff)
    }
}

/// A color from `#rgb`, `#rrggbb` or `#rrggbbaa` (the `#` is optional).
pub(super) fn parse_hex(text: &str) -> Option<Hsla> {
    let digits = text.trim().trim_start_matches('#');
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    let rgba_value = match digits.len() {
        3 => {
            let [r, g, b] = [(value >> 8) & 0xf, (value >> 4) & 0xf, value & 0xf];
            (r * 0x11) << 24 | (g * 0x11) << 16 | (b * 0x11) << 8 | 0xff
        }
        6 => value << 8 | 0xff,
        8 => value,
        _ => return None,
    };
    Some(rgb_to_hsla(rgba(rgba_value)))
}

/// Properties "Add property" offers, with the value a new one starts at.
fn addable() -> Vec<(&'static str, PropertyValue)> {
    let px = PropertyValue::Pixels;
    let option = |value: &str, options: &'static [&'static str]| PropertyValue::Enum {
        value: value.into(),
        options,
    };
    let color = |hex| PropertyValue::Color(rgb_to_hsla(gpui::rgb(hex)));
    vec![
        (
            "/display",
            option("Flex", &["Block", "Flex", "Grid", "None"]),
        ),
        (
            "/flex_direction",
            option("Column", &["Row", "Column", "RowReverse", "ColumnReverse"]),
        ),
        (
            "/align_items",
            option(
                "Center",
                &[
                    "Start",
                    "End",
                    "FlexStart",
                    "FlexEnd",
                    "Center",
                    "Baseline",
                    "Stretch",
                ],
            ),
        ),
        (
            "/justify_content",
            option(
                "Center",
                &[
                    "Start",
                    "End",
                    "FlexStart",
                    "FlexEnd",
                    "Center",
                    "Stretch",
                    "SpaceBetween",
                    "SpaceEvenly",
                    "SpaceAround",
                ],
            ),
        ),
        ("/flex_grow", PropertyValue::Number(1.)),
        ("/flex_shrink", PropertyValue::Number(1.)),
        ("/gap/width", px(8.)),
        ("/gap/height", px(8.)),
        ("/size/width", px(100.)),
        ("/size/height", px(100.)),
        ("/min_size/width", px(0.)),
        ("/min_size/height", px(0.)),
        ("/max_size/width", px(400.)),
        ("/max_size/height", px(400.)),
        ("/padding/top", px(8.)),
        ("/padding/right", px(8.)),
        ("/padding/bottom", px(8.)),
        ("/padding/left", px(8.)),
        ("/margin/top", px(8.)),
        ("/margin/right", px(8.)),
        ("/margin/bottom", px(8.)),
        ("/margin/left", px(8.)),
        ("/border_widths/top", px(1.)),
        ("/border_widths/right", px(1.)),
        ("/border_widths/bottom", px(1.)),
        ("/border_widths/left", px(1.)),
        ("/border_color", color(0xd0d7de)),
        ("/background", color(0xffffff)),
        ("/corner_radii/top_left", px(4.)),
        ("/corner_radii/top_right", px(4.)),
        ("/corner_radii/bottom_right", px(4.)),
        ("/corner_radii/bottom_left", px(4.)),
        ("/opacity", PropertyValue::Number(1.)),
        ("/text/color", color(0x1f2328)),
        ("/text/font_size", px(14.)),
        ("/text/font_weight", PropertyValue::Number(600.)),
        ("/position", option("Absolute", &["Relative", "Absolute"])),
        ("/inset/top", px(0.)),
        ("/inset/right", px(0.)),
        ("/inset/bottom", px(0.)),
        ("/inset/left", px(0.)),
        (
            "/overflow/x",
            option("Hidden", &["Visible", "Clip", "Hidden", "Scroll"]),
        ),
        (
            "/overflow/y",
            option("Hidden", &["Visible", "Clip", "Hidden", "Scroll"]),
        ),
        ("/visibility", option("Hidden", &["Visible", "Hidden"])),
    ]
}

/// `/padding/top` → `padding.top`.
fn label_of(pointer: &str) -> String {
    pointer.trim_start_matches('/').replace('/', ".")
}

/// The properties matching `query` (fuzzily, best first) that are not
/// already `set`, at most [`ADD_MATCHES`].
pub(super) fn add_matches(query: &str, set: &[StyleRow]) -> Vec<(&'static str, PropertyValue)> {
    let mut matches: Vec<(i32, usize, (&'static str, PropertyValue))> = addable()
        .into_iter()
        .enumerate()
        .filter(|(_, (pointer, _))| set.iter().all(|row| row.row.path != *pointer))
        .filter_map(|(order, property)| {
            let score = fuzzy_match(query, &label_of(property.0))?.score;
            Some((score, order, property))
        })
        .collect();
    matches.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    matches
        .into_iter()
        .take(ADD_MATCHES)
        .map(|(_, _, property)| property)
        .collect()
}

/// How a number row is scrubbed and written back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum NumberKind {
    /// Logical pixels.
    Pixels,
    /// Rems.
    Rems,
    /// A fraction, edited as a percentage.
    Percent,
    /// A plain number with its own step.
    Plain(f32),
}

impl NumberKind {
    /// The kind and the displayed value of an editable number.
    pub fn of(pointer: &str, value: &PropertyValue) -> Option<(Self, f32)> {
        Some(match *value {
            PropertyValue::Pixels(pixels) => (NumberKind::Pixels, pixels),
            PropertyValue::Rems(rems) => (NumberKind::Rems, rems),
            PropertyValue::Relative(fraction) => (NumberKind::Percent, fraction * 100.),
            PropertyValue::Number(number) => {
                let step = match pointer {
                    "/opacity" => 0.05,
                    "/text/font_weight" => 100.,
                    _ => 1.,
                };
                (NumberKind::Plain(step), number as f32)
            }
            _ => return None,
        })
    }

    fn step(self) -> f32 {
        match self {
            NumberKind::Pixels | NumberKind::Percent => 1.,
            NumberKind::Rems => 0.125,
            NumberKind::Plain(step) => step,
        }
    }

    fn unit(self) -> Option<&'static str> {
        match self {
            NumberKind::Pixels => Some("px"),
            NumberKind::Rems => Some("rem"),
            NumberKind::Percent => Some("%"),
            NumberKind::Plain(_) => None,
        }
    }

    /// The property value for a displayed number.
    pub fn value(self, number: f32) -> PropertyValue {
        match self {
            NumberKind::Pixels => PropertyValue::Pixels(number),
            NumberKind::Rems => PropertyValue::Rems(number),
            NumberKind::Percent => PropertyValue::Relative(number / 100.),
            NumberKind::Plain(_) => PropertyValue::Number(f64::from(number)),
        }
    }
}

/// An editor entity behind one grid row.
pub(super) enum RowEditor {
    /// A scrub field for numbers.
    Number {
        field: Entity<ScrubField>,
        kind: NumberKind,
        _changed: Subscription,
    },
    /// A text field for colors (hex) and free text.
    Text {
        state: Entity<EditableTextState>,
        color: bool,
        _changed: Subscription,
        _observed: Subscription,
    },
}

/// The editors of the selected element's grid, keyed by property pointer,
/// plus the "Add property" query.
#[derive(Default)]
pub(super) struct StyleEditors {
    path: Option<PathKey>,
    rows: HashMap<String, RowEditor>,
    /// The "Add property" query while the list is open.
    pub adding: Option<(Entity<EditableTextState>, Subscription)>,
}

impl ElementsLens {
    /// Creates, updates and drops row editors to match the selection's grid.
    pub(super) fn sync_style_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let style = self
            .selection
            .as_ref()
            .filter(|selection| selection.editable)
            .and_then(|selection| selection.style.clone());
        let Some(style) = style else {
            self.style_editors.rows.clear();
            self.style_editors.path = None;
            return;
        };
        if self.style_editors.path != Some(style.path) {
            self.style_editors.rows.clear();
            self.style_editors.adding = None;
            self.style_editors.path = Some(style.path);
        }
        self.style_editors
            .rows
            .retain(|pointer, _| style.rows.iter().any(|row| &row.row.path == pointer));
        for StyleRow { row, .. } in &style.rows {
            let pointer = row.path.clone();
            match (&row.value, NumberKind::of(&pointer, &row.value)) {
                (_, Some((kind, number))) => {
                    let current = match self.style_editors.rows.get(&pointer) {
                        Some(RowEditor::Number {
                            field, kind: known, ..
                        }) if *known == kind => Some(field.clone()),
                        _ => None,
                    };
                    match current {
                        Some(field) => {
                            if !field.focus_handle(cx).contains_focused(window, cx) {
                                field.update(cx, |field, cx| field.set_value(number, cx));
                            }
                        }
                        None => {
                            let editor =
                                self.number_editor(pointer.clone(), kind, number, window, cx);
                            self.style_editors.rows.insert(pointer, editor);
                        }
                    }
                }
                (PropertyValue::Color(color), None) => {
                    self.sync_text_editor(pointer, format::color(*color), true, window, cx)
                }
                (PropertyValue::Text(text), None) => {
                    self.sync_text_editor(pointer, text.clone(), false, window, cx)
                }
                _ => {
                    self.style_editors.rows.remove(&pointer);
                }
            }
        }
    }

    fn number_editor(
        &mut self,
        pointer: String,
        kind: NumberKind,
        number: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> RowEditor {
        let field = cx.new(|cx| {
            let field = ScrubField::new(number, kind.step(), cx).with_precision(3);
            match kind.unit() {
                Some(unit) => field.with_unit(unit),
                None => field,
            }
        });
        let changed = cx.subscribe_in(
            &field,
            window,
            move |this, _, event: &ScrubChanged, window, cx| {
                this.edit(&pointer, kind.value(event.0), window, cx);
            },
        );
        RowEditor::Number {
            field,
            kind,
            _changed: changed,
        }
    }

    fn sync_text_editor(
        &mut self,
        pointer: String,
        text: String,
        color: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(RowEditor::Text {
            state,
            color: known,
            ..
        }) = self.style_editors.rows.get(&pointer)
            && *known == color
        {
            let state = state.clone();
            let focused = state.focus_handle(cx).is_focused(window);
            if !focused && state.read(cx).as_str() != text {
                state.update(cx, |state, cx| state.emplace(&text, cx));
            }
            return;
        }
        let state = cx.new(|cx| EditableTextState::new(StringStorage::from(text.as_str()), cx));
        let row_pointer = pointer.clone();
        let changed = cx.subscribe_in(
            &state,
            window,
            move |this, state, _: &TextChanged, window, cx| {
                let text = state.read(cx).as_str().to_string();
                let value = if color {
                    match parse_hex(&text) {
                        Some(color) => PropertyValue::Color(color),
                        None => return,
                    }
                } else {
                    PropertyValue::Text(text)
                };
                if this.current_value(&row_pointer).as_ref() != Some(&value) {
                    this.edit(&row_pointer, value, window, cx);
                }
            },
        );
        let observed = cx.observe(&state, |_, _, cx| cx.notify());
        self.style_editors.rows.insert(
            pointer,
            RowEditor::Text {
                state,
                color,
                _changed: changed,
                _observed: observed,
            },
        );
    }

    /// The effective value of a grid property of the selection.
    fn current_value(&self, pointer: &str) -> Option<PropertyValue> {
        let style = self.selection.as_ref()?.style.as_ref()?;
        style
            .rows
            .iter()
            .find(|row| row.row.path == pointer)
            .map(|row| row.row.value.clone())
    }

    /// Opens the "Add property" list with a focused query field.
    pub(super) fn open_add_property(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = text_field_state(cx);
        let focus = query.focus_handle(cx);
        window.focus(&focus, cx);
        let observed = cx.observe(&query, |_, _, cx| cx.notify());
        self.style_editors.adding = Some((query, observed));
        cx.notify();
    }

    fn add_property(
        &mut self,
        pointer: &str,
        value: PropertyValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.style_editors.adding = None;
        self.edit(pointer, value, window, cx);
    }

    /// The Style section's body.
    pub(super) fn render_style(
        &self,
        style: &StyleModel,
        editable: bool,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        let mut body = div().flex().flex_col().pb_1();
        if style.base.is_none() {
            body = body.child(
                div()
                    .px(theme.metrics.gutter)
                    .py_1()
                    .text_size(theme.metrics.text_small)
                    .text_color(colors.text_faint)
                    .child("The element's own style arrives with its next frame."),
            );
        }
        let mut group: Option<PropertyGroup> = None;
        for row in &style.rows {
            if group != Some(row.row.group) {
                group = Some(row.row.group);
                body = body.child(
                    div()
                        .h(px(20.))
                        .px(theme.metrics.gutter)
                        .flex()
                        .items_end()
                        .text_size(theme.metrics.label)
                        .text_color(colors.text_faint)
                        .child(row.row.group.label()),
                );
            }
            body = body.child(self.render_style_row(row, editable, theme, cx));
        }
        if let Some(error) = &self.edit_error {
            body = body.child(
                div()
                    .px(theme.metrics.gutter)
                    .py_1()
                    .text_size(theme.metrics.text_small)
                    .text_color(colors.crit)
                    .child(error.clone()),
            );
        }
        if editable {
            body = body.child(self.render_add_property(style, theme, cx));
        }
        if !style.diff.is_empty() {
            body = body.child(render_diff(&style.diff, theme));
        }
        if style.instances > 1 && style.has_overrides() {
            body = body.child(
                div()
                    .px(theme.metrics.gutter)
                    .pt_1()
                    .text_size(theme.metrics.text_small)
                    .text_color(colors.text_faint)
                    .child(format!(
                        "Edits apply to all {} elements built at this site.",
                        style.instances
                    )),
            );
        }
        body.child(self.render_style_actions(style, editable, theme, cx))
            .into_any_element()
    }

    fn render_style_row(
        &self,
        row: &StyleRow,
        editable: bool,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = &theme.colors;
        let pointer = row.row.path.clone();
        let editor = editable
            .then(|| self.render_value_editor(&row.row, cx))
            .flatten()
            .unwrap_or_else(|| render_value(&row.row.value, theme));
        div()
            .id(SharedString::from(format!("style{pointer}")))
            .h(theme.metrics.property_row)
            .pl(theme.metrics.gutter)
            .pr(px(4.))
            .flex()
            .items_center()
            .gap_1()
            .hover(|style| style.bg(colors.hover))
            .child(
                div()
                    .flex_none()
                    .size(px(10.))
                    .when(row.overridden, |this| {
                        this.child(
                            Icon::new(IconName::ElementDot)
                                .size(px(10.))
                                .color(colors.accent),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(112.))
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(theme.metrics.mono)
                    .text_color(if row.overridden {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .child(row.row.label.clone()),
            )
            .child(div().flex_1().min_w_0().flex().items_center().child(editor))
            // Every row keeps the revert slot, so editors line up.
            .child(div().flex_none().size(theme.metrics.control_small).when(
                row.overridden && editable,
                |this| {
                    this.child(
                        Button::new(SharedString::from(format!("revert{pointer}")))
                            .icon(IconName::Revert)
                            .size(ButtonSize::Small)
                            .tooltip("Revert to the code's value")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.revert_property(&pointer, window, cx)
                            })),
                    )
                },
            ))
    }

    /// An editor for `row`, if its value type is editable.
    fn render_value_editor(&self, row: &PropertyRow, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pointer = row.path.clone();
        Some(match (&row.value, self.style_editors.rows.get(&row.path)) {
            (_, Some(RowEditor::Number { field, .. })) => div()
                .flex_none()
                .w(px(80.))
                .child(field.clone())
                .into_any_element(),
            (PropertyValue::Color(color), Some(RowEditor::Text { state, .. })) => div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(ColorSwatch::new(*color))
                .child(
                    div()
                        .w(px(96.))
                        .min_w_0()
                        .flex_shrink_1()
                        .child(TextField::new(
                            SharedString::from(format!("hex{pointer}")),
                            state,
                        )),
                )
                .into_any_element(),
            (PropertyValue::Text(_), Some(RowEditor::Text { state, .. })) => div()
                .flex_1()
                .min_w_0()
                .child(TextField::new(
                    SharedString::from(format!("text{pointer}")),
                    state,
                ))
                .into_any_element(),
            (PropertyValue::Enum { value, options }, _) => {
                let next = options
                    .iter()
                    .position(|option| option == value)
                    .map_or(0, |ix| (ix + 1) % options.len());
                let next_value = PropertyValue::Enum {
                    value: options[next].to_string(),
                    options,
                };
                Button::new(SharedString::from(format!("enum{pointer}")))
                    .label(value.clone())
                    .icon(IconName::ChevronRight)
                    .size(ButtonSize::Small)
                    .style(ButtonStyle::Subtle)
                    .tooltip(format!("Click for {}", options[next]))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit(&pointer, next_value.clone(), window, cx)
                    }))
                    .into_any_element()
            }
            (PropertyValue::Bool(flag), _) => {
                let flipped = PropertyValue::Bool(!flag);
                Button::new(SharedString::from(format!("bool{pointer}")))
                    .label(if *flag { "true" } else { "false" })
                    .icon(if *flag {
                        IconName::Check
                    } else {
                        IconName::Close
                    })
                    .size(ButtonSize::Small)
                    .style(ButtonStyle::Subtle)
                    .toggle_state(*flag)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit(&pointer, flipped.clone(), window, cx)
                    }))
                    .into_any_element()
            }
            _ => return None,
        })
    }

    fn render_add_property(
        &self,
        style: &StyleModel,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        let Some((query, _)) = &self.style_editors.adding else {
            return div()
                .px(px(4.))
                .pt_1()
                .flex()
                .child(
                    Button::new("elements-add-property")
                        .icon(IconName::Plus)
                        .label("Add property")
                        .size(ButtonSize::Small)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_add_property(window, cx)),
                        ),
                )
                .into_any_element();
        };
        let matches = add_matches(query.read(cx).as_str(), &style.rows);
        let first = matches.first().cloned();
        div()
            .mx(theme.metrics.gutter)
            .mt_1()
            .flex()
            .flex_col()
            .rounded(theme.metrics.radius)
            .border_1()
            .border_color(colors.line_strong)
            .bg(colors.surface)
            .child(
                div()
                    .p_1()
                    .capture_action(cx.listener(
                        move |this,
                              _: &gpui_elements::editable_text::actions::Enter,
                              window,
                              cx| {
                            if let Some((pointer, value)) = first.clone() {
                                this.add_property(pointer, value, window, cx);
                            }
                            cx.stop_propagation();
                        },
                    ))
                    .capture_action(cx.listener(
                        |this, _: &gpui_elements::editable_text::actions::Escape, _, cx| {
                            this.style_editors.adding = None;
                            cx.stop_propagation();
                            cx.notify();
                        },
                    ))
                    .child(
                        TextField::new("elements-add-query", query)
                            .icon(IconName::Plus)
                            .placeholder("Property, e.g. padding"),
                    ),
            )
            .children(
                matches
                    .into_iter()
                    .enumerate()
                    .map(|(ix, (pointer, value))| {
                        let label = label_of(pointer);
                        let preview = value.to_string();
                        div()
                            .id(("elements-add", ix))
                            .h(theme.metrics.row)
                            .px(theme.metrics.gutter)
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .when(ix == 0, |this| this.bg(colors.selected))
                            .hover(|style| style.bg(colors.hover))
                            .font_family(MONO_FONT)
                            .text_size(theme.metrics.mono)
                            .child(div().flex_1().min_w_0().truncate().child(label))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(colors.text_faint)
                                    .child(preview),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_property(pointer, value.clone(), window, cx)
                            }))
                    }),
            )
            .into_any_element()
    }

    fn render_style_actions(
        &self,
        style: &StyleModel,
        editable: bool,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_overrides = style.has_overrides();
        let code = style.patch.to_code();
        let patch = style.patch_text();
        div()
            .px(px(4.))
            .pt_1()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(
                Button::new("elements-copy-rust")
                    .icon(IconName::Copy)
                    .label("Copy Rust")
                    .size(ButtonSize::Small)
                    .style(ButtonStyle::Subtle)
                    .disabled(code.is_empty())
                    .tooltip(if has_overrides {
                        "Copy the builder calls that make the code match"
                    } else {
                        "Copy this style as builder calls"
                    })
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                    }),
            )
            .when(has_overrides, |this| {
                this.child(
                    Button::new("elements-copy-patch")
                        .icon(IconName::Copy)
                        .label("Copy patch")
                        .size(ButtonSize::Small)
                        .tooltip("Copy the change as - / + lines")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(patch.clone()))
                        }),
                )
                .when(editable, |this| {
                    this.child(
                        Button::new("elements-revert-all")
                            .icon(IconName::Revert)
                            .label("Revert all")
                            .size(ButtonSize::Small)
                            .color(theme.colors.crit)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.revert_all(window, cx)),
                            ),
                    )
                })
            })
    }
}

/// A read-only value: colors as swatch and hex, the rest as text.
pub(super) fn render_value(value: &PropertyValue, theme: &Theme) -> AnyElement {
    let colors = &theme.colors;
    let text = match value {
        PropertyValue::Color(color) => {
            return div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(ColorSwatch::new(*color))
                .child(
                    div()
                        .font_family(MONO_FONT)
                        .text_size(theme.metrics.mono)
                        .child(format::color(*color)),
                )
                .into_any_element();
        }
        PropertyValue::Pixels(value) => format!("{}px", format::number(*value)),
        PropertyValue::Rems(value) => format!("{}rem", crate::widgets::format_number(*value, 3)),
        PropertyValue::Relative(value) => format::percent(f64::from(*value)),
        PropertyValue::Text(text) => text.clone(),
        other => other.to_string(),
    };
    div()
        .id(SharedString::from(format!("value-{text}")))
        .min_w_0()
        .truncate()
        .font_family(MONO_FONT)
        .text_size(theme.metrics.mono)
        .text_color(colors.text)
        .tooltip(Tooltip::text(text.clone()))
        .child(text)
        .into_any_element()
}

/// The code change as a compact, colored line diff (changed lines only).
fn render_diff(diff: &[DiffLine], theme: &Theme) -> impl IntoElement {
    let colors = &theme.colors;
    div()
        .mx(theme.metrics.gutter)
        .mt_1()
        .py_1()
        .flex()
        .flex_col()
        .rounded(theme.metrics.radius)
        .bg(colors.surface)
        .border_1()
        .border_color(colors.line)
        .font_family(MONO_FONT)
        .text_size(theme.metrics.mono)
        .children(
            diff.iter()
                .filter(|line| line.kind != DiffKind::Kept)
                .map(|line| {
                    let (sign, color) = match line.kind {
                        DiffKind::Removed => ("−", colors.crit),
                        _ => ("+", colors.ok),
                    };
                    div()
                        .px(px(6.))
                        .flex()
                        .gap(px(6.))
                        .text_color(color)
                        .child(div().flex_none().child(sign))
                        .child(div().min_w_0().truncate().child(line.text.clone()))
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{px as gpx, rgb};

    #[test]
    fn hex_colors_parse_in_every_short_and_long_form() {
        let hex = |text| parse_hex(text).map(format::color);
        assert_eq!(hex("#3366ff").as_deref(), Some("#3366ff"));
        assert_eq!(hex("3366FF").as_deref(), Some("#3366ff"));
        assert_eq!(hex("#36f").as_deref(), Some("#3366ff"));
        assert_eq!(hex(" #3366ff80 ").as_deref(), Some("#3366ff80"));
        assert_eq!(hex("#3366f"), None);
        assert_eq!(hex("#zzzzzz"), None);
        assert_eq!(hex(""), None);
    }

    #[test]
    fn the_model_marks_overridden_rows_and_patches_only_what_changed() {
        let path = PathKey(3);
        let base = StyleRefinement::default().p(gpx(8.)).bg(rgb(0x2255ee));
        let overrides = style_grid::set(
            &StyleRefinement::default(),
            "/padding/top",
            &PropertyValue::Pixels(12.),
        )
        .unwrap();
        let model = StyleModel::new(path, Some(base.clone()), overrides, 1);
        let top = model
            .rows
            .iter()
            .find(|row| row.row.path == "/padding/top")
            .unwrap();
        assert!(top.overridden);
        assert_eq!(top.row.value, PropertyValue::Pixels(12.));
        assert!(
            model
                .rows
                .iter()
                .filter(|row| row.row.path != "/padding/top")
                .all(|row| !row.overridden)
        );
        assert_eq!(model.patch.to_code(), ".pt(px(12.))");
        assert!(model.has_overrides());
        let patch = model.patch_text();
        assert!(patch.contains("- .p(px(8.))"), "{patch}");
        assert!(patch.contains("+ .pt(px(12.))"), "{patch}");

        let untouched = StyleModel::new(path, Some(base), StyleRefinement::default(), 1);
        assert!(!untouched.has_overrides());
        assert!(untouched.diff.is_empty());
        assert_eq!(untouched.patch.to_code(), ".p(px(8.))\n.bg(rgb(0x2255ee))");
    }

    #[test]
    fn every_addable_property_can_be_set_on_an_empty_style() {
        for (pointer, value) in addable() {
            let style = style_grid::set(&StyleRefinement::default(), pointer, &value)
                .unwrap_or_else(|error| panic!("{pointer}: {error}"));
            let rows = style_grid::rows(&style);
            assert!(
                rows.iter().any(|row| row.path == pointer),
                "{pointer} not in {rows:?}"
            );
        }
    }

    #[test]
    fn add_matches_are_fuzzy_and_skip_set_properties() {
        let set = vec![StyleRow {
            row: PropertyRow {
                path: "/padding/top".into(),
                label: "padding.top".into(),
                group: PropertyGroup::Spacing,
                value: PropertyValue::Pixels(8.),
            },
            overridden: false,
        }];
        let labels: Vec<String> = add_matches("padt", &set)
            .into_iter()
            .map(|(pointer, _)| label_of(pointer))
            .collect();
        assert!(!labels.contains(&"padding.top".to_string()));
        assert!(
            labels.iter().all(|label| label.starts_with("padding")),
            "{labels:?}"
        );
        assert_eq!(add_matches("", &[]).len(), ADD_MATCHES);
        assert!(add_matches("zzzz", &[]).is_empty());
    }

    #[test]
    fn numbers_scrub_in_their_own_units() {
        assert_eq!(
            NumberKind::of("/padding/top", &PropertyValue::Pixels(8.)),
            Some((NumberKind::Pixels, 8.))
        );
        assert_eq!(
            NumberKind::of("/size/width", &PropertyValue::Relative(0.5)),
            Some((NumberKind::Percent, 50.))
        );
        assert_eq!(
            NumberKind::Percent.value(25.),
            PropertyValue::Relative(0.25)
        );
        assert_eq!(
            NumberKind::of("/opacity", &PropertyValue::Number(0.5)),
            Some((NumberKind::Plain(0.05), 0.5))
        );
        assert_eq!(NumberKind::of("/display", &PropertyValue::Auto), None);
    }
}
