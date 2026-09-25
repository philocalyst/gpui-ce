//! The detail pane: the selected element's header and its sections, why
//! first: why this size, box model, style, interactivity, details, cost.

use super::{ElementsLens, Selection, rows::RowKind, style::render_value};
use crate::{
    analysis::{
        contrast::{self, Srgb},
        format,
        style_grid::PropertyValue,
        why_size::AxisExplanation,
    },
    commands::keys,
    state::Lens,
    theme::{LoupeSettings, MONO_FONT, Theme},
    widgets::{
        Button, ButtonSize, ButtonStyle, EmptyState, Icon, IconName, Kbd, Pill, SectionHeader,
        Tone, Tooltip,
    },
};
use gpui::{
    AnyElement, ColorExt as _, Context, FontWeight, IntoElement, SharedString, div,
    inspector::{ElementDetails, ElementFlags, ElementIndex, ElementTree, ForcedStates},
    prelude::*,
    px,
};

/// A collapsible section of the detail pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Section {
    /// One sentence per axis.
    WhySize,
    /// The nested margin / border / padding / content diagram.
    BoxModel,
    /// The style grid.
    Style,
    /// Input flags and forced states.
    Interactivity,
    /// Text, colors, contrast, accessibility and element-specific facts.
    Details,
    /// Primitives and renders.
    Cost,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::WhySize => "Why this size",
            Section::BoxModel => "Box model",
            Section::Style => "Style",
            Section::Interactivity => "Interactivity",
            Section::Details => "Details",
            Section::Cost => "Cost",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Section::WhySize => "elements-section-why-size",
            Section::BoxModel => "elements-section-box-model",
            Section::Style => "elements-section-style",
            Section::Interactivity => "elements-section-interactivity",
            Section::Details => "elements-section-details",
            Section::Cost => "elements-section-cost",
        }
    }
}

/// Text contrast against the composited background, as WCAG measures it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Contrast {
    /// 1.0 to 21.0.
    pub ratio: f32,
    /// The AA minimum for the text's size.
    pub required: f32,
}

/// The contrast of element `ix`'s text color with the backgrounds under it
/// (its own and its ancestors', composited). `None` without a text color or
/// an opaque background.
pub(super) fn text_contrast(tree: &ElementTree, ix: ElementIndex) -> Option<Contrast> {
    let details = tree.get(ix)?.details.as_deref()?;
    let text_color = details.text_color?;
    let backgrounds = std::iter::once(ix)
        .chain(tree.ancestors(ix))
        .filter_map(|ix| tree.get(ix)?.details.as_ref()?.background);
    let background = contrast::composite(backgrounds)?;
    let foreground: Srgb = background.under(text_color);
    Some(Contrast {
        ratio: contrast::contrast_ratio(foreground, background),
        required: contrast::required_ratio(details.font_size),
    })
}

/// The flags worth naming, with how much each matters.
pub(super) fn flag_pills(flags: ElementFlags) -> Vec<(&'static str, Tone)> {
    [
        (ElementFlags::CLICKABLE, "Clickable", Tone::Accent),
        (ElementFlags::FOCUSABLE, "Focusable", Tone::Accent),
        (ElementFlags::TAB_STOP, "Tab stop", Tone::Accent),
        (ElementFlags::SCROLLABLE, "Scrolls", Tone::Accent),
        (ElementFlags::KEYBOARD, "Keyboard", Tone::Accent),
        (ElementFlags::TOOLTIP, "Tooltip", Tone::Accent),
        (ElementFlags::DRAG_DROP, "Drag & drop", Tone::Accent),
        (ElementFlags::HITBOX, "Hitbox", Tone::Neutral),
        (ElementFlags::STATEFUL_STYLE, "State styles", Tone::Neutral),
        (ElementFlags::DEFERRED, "Deferred", Tone::Neutral),
        (ElementFlags::REUSED, "Cached", Tone::Neutral),
        (ElementFlags::OVERRIDDEN, "Overridden", Tone::Accent),
        (ElementFlags::CLIPPED, "Clipped away", Tone::Warn),
        (
            ElementFlags::OVERFLOWS_PARENT,
            "Overflows parent",
            Tone::Warn,
        ),
    ]
    .into_iter()
    .filter(|(flag, ..)| flags.contains(*flag))
    .map(|(_, label, tone)| (label, tone))
    .collect()
}

/// The states Loupe can force, in the order the toggles show them.
const FORCED: [(ForcedStates, &str); 4] = [
    (ForcedStates::HOVER, "Hover"),
    (ForcedStates::ACTIVE, "Active"),
    (ForcedStates::FOCUS, "Focus"),
    (ForcedStates::FOCUS_VISIBLE, "Focus visible"),
];

/// A value in the Details section.
enum Fact {
    Text(String),
    Value(PropertyValue),
}

/// The Details rows of an element, in reading order.
fn detail_facts(details: &ElementDetails, bounds_text: String) -> Vec<(SharedString, Fact)> {
    let mut facts = vec![("Bounds".into(), Fact::Text(bounds_text))];
    if let Some(text) = &details.text {
        facts.push(("Text".into(), Fact::Text(text.to_string())));
    }
    match (&details.font_family, details.font_size) {
        (Some(family), Some(size)) => facts.push((
            "Font".into(),
            Fact::Text(format!("{family} · {}", format::pixels(size))),
        )),
        (Some(family), None) => facts.push(("Font".into(), Fact::Text(family.to_string()))),
        (None, Some(size)) => facts.push(("Font size".into(), Fact::Text(format::pixels(size)))),
        (None, None) => {}
    }
    for (label, color) in [
        ("Text color", details.text_color),
        ("Background", details.background),
        ("Border color", details.border_color),
    ] {
        if let Some(color) = color {
            facts.push((label.into(), Fact::Value(PropertyValue::Color(color))));
        }
    }
    if let Some(radius) = details.corner_radius {
        facts.push(("Corner radius".into(), Fact::Text(format::pixels(radius))));
    }
    if let Some(opacity) = details.opacity {
        facts.push(("Opacity".into(), Fact::Text(format::number(opacity))));
    }
    if let Some(layout) = &details.layout {
        let mut text = layout.display.to_string();
        if let Some(direction) = &layout.flex_direction {
            text.push(' ');
            text.push_str(direction);
        }
        if layout.absolute {
            text.push_str(" · absolute");
        }
        facts.push(("Layout".into(), Fact::Text(text)));
    }
    if let Some(role) = &details.a11y_role {
        facts.push(("Role".into(), Fact::Text(role.to_string())));
    }
    if let Some(label) = &details.a11y_label {
        facts.push(("A11y label".into(), Fact::Text(label.to_string())));
    }
    if let Some(context) = &details.key_context {
        facts.push(("Key context".into(), Fact::Text(context.to_string())));
    }
    if let Some(source) = &details.source {
        facts.push(("Image".into(), Fact::Text(source.to_string())));
    }
    if let Some((count, visible)) = &details.list {
        facts.push((
            "List".into(),
            Fact::Text(format!(
                "{} items · showing {}–{}",
                format::count(*count as u64),
                visible.start,
                visible.end.saturating_sub(1).max(visible.start)
            )),
        ));
    }
    if let Some(offset) = details.scroll_offset {
        let mut text = format!("offset {}", format::point(offset));
        if let Some(content) = details.content_size {
            text.push_str(&format!(" · content {}", format::size(content)));
        }
        facts.push(("Scroll".into(), Fact::Text(text)));
    }
    for (label, value) in &details.extra {
        facts.push((label.clone(), Fact::Text(value.to_string())));
    }
    facts
}

impl ElementsLens {
    /// The detail pane.
    /// The detail pane; `wide` lays the sections out in two columns.
    pub(super) fn render_detail_pane(
        &self,
        holding: bool,
        wide: bool,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        let Some(selection) = self.selection.clone() else {
            return self.render_empty_detail(holding, theme, cx);
        };
        let tree = selection.index.tree.clone();
        let Some(record) = tree.get(selection.ix) else {
            return self.render_empty_detail(holding, theme, cx);
        };
        let details = record.details.as_deref().cloned().unwrap_or_default();

        let why = self.section(Section::WhySize, None, theme, cx, |_, _| {
            render_why(selection.why.as_ref(), theme)
        });
        let box_model = self.section(Section::BoxModel, None, theme, cx, |this, _| {
            this.render_box_model(
                record.bounds.size,
                &details.box_model.unwrap_or_default(),
                selection.editable,
                theme,
            )
        });
        let style = selection.style.as_ref().map(|style| {
            let detail = style
                .has_overrides()
                .then(|| format!("{} overridden", style.overridden));
            self.section(Section::Style, detail, theme, cx, |this, cx| {
                this.render_style(style, selection.editable, theme, cx)
            })
        });
        let interactivity = self.section(Section::Interactivity, None, theme, cx, |this, cx| {
            this.render_interactivity(&selection, record.flags, theme, cx)
        });
        let facts = self.section(Section::Details, None, theme, cx, |_, _| {
            render_details(&tree, selection.ix, &details, theme)
        });
        let cost_detail = selection
            .cost
            .view
            .as_ref()
            .map(|view| format!("×{}", view.stats.rendered));
        let cost = self.section(Section::Cost, cost_detail, theme, cx, |_, _| {
            render_cost(&selection, theme)
        });

        let column = || div().flex_1().min_w_0().flex().flex_col();
        let sections = if wide {
            // Geometry on the left, code and facts on the right.
            div()
                .flex()
                .items_start()
                .child(column().child(why).child(box_model).child(interactivity))
                .child(
                    column()
                        .border_l_1()
                        .border_color(colors.line)
                        .children(style)
                        .child(facts)
                        .child(cost),
                )
        } else {
            column()
                .child(why)
                .child(box_model)
                .children(style)
                .child(interactivity)
                .child(facts)
                .child(cost)
        };
        div()
            .id("elements-detail")
            .size_full()
            .overflow_y_scroll()
            .bg(colors.bg)
            .flex()
            .flex_col()
            .pb_4()
            .child(self.render_header(&selection, theme, cx))
            .when(selection.missing, |this| {
                this.child(render_missing(&selection, self.shown_frame(), theme))
            })
            .child(sections)
            .into_any_element()
    }

    /// The frame the tree on screen is from.
    fn shown_frame(&self) -> Option<u64> {
        self.shown.as_ref().map(|shown| shown.frame)
    }

    /// Nothing selected, or the selection is not in the shown tree.
    fn render_empty_detail(
        &self,
        holding: bool,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        let missing = self.selected.filter(|_| self.shown.is_some());
        if missing.is_some() {
            return EmptyState::new("Not in this frame")
                .icon(IconName::Warning)
                .description(format!(
                    "The selected element isn't in frame #{}'s tree.",
                    self.shown_frame().unwrap_or_default()
                ))
                .into_any_element();
        }
        div()
            .size_full()
            .bg(colors.bg)
            .child(
                EmptyState::new("Nothing selected")
                    .icon(IconName::Pick)
                    .description(Lens::Elements.question())
                    .action(self.render_pick_actions(holding, cx)),
            )
            .into_any_element()
    }

    /// Start picking (with its key) and hold the app, for empty states.
    pub(super) fn render_pick_actions(
        &self,
        holding: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pick = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Button::new("elements-start-picking")
                    .icon(IconName::Pick)
                    .label("Start picking")
                    .style(ButtonStyle::Subtle)
                    .on_click(|_, window, _| window.start_inspector_pick()),
            )
            .child(Kbd::new(keys::PICK));
        // The same command as the toolbar's pin, so both stay in step.
        let hold = Button::new("elements-hold-app")
            .icon(IconName::Hold)
            .label(if holding { "Release app" } else { "Hold app" })
            .size(ButtonSize::Small)
            .toggle_state(holding)
            .tooltip_keys(
                "Keep the app still (a hover menu stays open) while you pick",
                keys::HOLD,
            )
            .on_click(cx.listener(|_, _, window, cx| {
                crate::loupe::toggle_hold(window, cx);
                cx.notify();
            }));
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(pick)
            .child(hold)
    }

    /// A collapsible section: its header, and its body when expanded.
    fn section(
        &self,
        section: Section,
        detail: Option<String>,
        theme: &'static Theme,
        cx: &mut Context<Self>,
        body: impl FnOnce(&Self, &mut Context<Self>) -> AnyElement,
    ) -> AnyElement {
        let expanded = !self.collapsed.contains(&section);
        let header = SectionHeader::new(section.title())
            .disclosure(expanded)
            .rule();
        let header = match detail {
            Some(detail) => header.detail(detail),
            None => header,
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(section.id())
                    .debug_selector(|| section.id().into())
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.colors.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
                    .child(header),
            )
            .when(expanded, |this| this.child(body(self, cx)))
            .into_any_element()
    }

    fn render_header(
        &self,
        selection: &Selection,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = &theme.colors;
        let index = &selection.index;
        let (Some(info), Some(record)) = (index.info(selection.ix), index.tree.get(selection.ix))
        else {
            return div();
        };
        let (glyph, glyph_color) = match info.kind {
            RowKind::View => (IconName::View, colors.view),
            RowKind::Component => (IconName::Component, colors.component),
            RowKind::Text | RowKind::Element => (IconName::ElementDot, colors.text_faint),
        };
        let (name, id) = match info.id_start {
            Some(start) => (
                info.label[..start].to_string(),
                Some(info.label[start..].to_string()),
            ),
            None => (info.label.to_string(), None),
        };
        let title = div()
            .h(px(28.))
            .flex()
            .items_center()
            .gap(px(6.))
            .child(Icon::new(glyph).size(px(12.)).color(glyph_color))
            .child(
                div()
                    .id("elements-title")
                    .min_w_0()
                    .flex_shrink_1()
                    .truncate()
                    .text_size(theme.metrics.text)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text)
                    .child(name.clone())
                    .tooltip(Tooltip::with_meta(name, info.type_name)),
            )
            .children(id.map(|id| {
                div()
                    .min_w_0()
                    .flex_shrink_1()
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(theme.metrics.mono)
                    .text_color(colors.text_muted)
                    .child(id)
            }))
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .font_family(MONO_FONT)
                    .text_size(theme.metrics.mono)
                    .text_color(colors.text_muted)
                    .child(format::size(record.bounds.size)),
            );

        let source = match selection.source.clone() {
            Some((location, path)) => {
                let editor = LoupeSettings::get(cx).editor;
                let full_path = format!("{path}:{}:{}", location.line(), location.column());
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        div()
                            .id("elements-source")
                            .debug_selector(|| "elements-source".into())
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .cursor_pointer()
                            .text_color(colors.accent)
                            .hover(|style| style.underline())
                            .child(
                                Icon::new(IconName::Source)
                                    .size(theme.metrics.icon_small)
                                    .color(colors.accent),
                            )
                            .child(
                                div()
                                    .font_family(MONO_FONT)
                                    .text_size(theme.metrics.mono)
                                    .child(format::location(location)),
                            )
                            .tooltip(Tooltip::with_meta(
                                format!("Open in {}", editor.label()),
                                full_path,
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.open_source(cx))),
                    )
                    .child(
                        Button::new("elements-copy-source")
                            .icon(IconName::Copy)
                            .size(ButtonSize::Small)
                            .tooltip("Copy path:line:column")
                            .on_click(cx.listener(|this, _, _, cx| this.copy_source(cx))),
                    )
                    .into_any_element()
            }
            None => div()
                .flex_none()
                .text_size(theme.metrics.text_small)
                .text_color(colors.text_faint)
                .child(if info.kind == RowKind::Text {
                    "Text from a string"
                } else {
                    "No source location"
                })
                .into_any_element(),
        };
        let owner =
            selection.owner.clone().map(|owner| {
                let (glyph, glyph_color, tooltip) = if owner.view {
                    (
                        IconName::View,
                        colors.view,
                        "Select the view that renders it",
                    )
                } else {
                    (
                        IconName::ElementDot,
                        colors.text_faint,
                        "Select the element it is part of",
                    )
                };
                let key = owner.key;
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(div().flex_none().text_color(colors.text_faint).child("in"))
                    .child(Icon::new(glyph).size(px(10.)).color(glyph_color))
                    .child(
                        div()
                            .id("elements-owner")
                            .min_w_0()
                            .truncate()
                            .cursor_pointer()
                            .text_color(colors.text_muted)
                            .hover(|style| style.text_color(colors.accent))
                            .child(owner.label)
                            .tooltip(Tooltip::text(tooltip))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_key(key, window, cx)
                            })),
                    )
            });
        div()
            .flex_none()
            .px(theme.metrics.gutter)
            .pb_1()
            .flex()
            .flex_col()
            .child(title)
            .child(
                div()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(theme.metrics.text_small)
                    .child(source)
                    .children(owner),
            )
    }

    fn render_interactivity(
        &self,
        selection: &Selection,
        flags: ElementFlags,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        let pills = flag_pills(flags);
        let flags_row = div()
            .px(theme.metrics.gutter)
            .py_1()
            .flex()
            .flex_wrap()
            .gap_1()
            .when(pills.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(theme.metrics.text_small)
                        .text_color(colors.text_faint)
                        .child("Not interactive"),
                )
            })
            .children(
                pills
                    .into_iter()
                    .map(|(label, tone)| Pill::new(label).tone(tone)),
            );
        let toggles = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(FORCED.into_iter().map(|(state, label)| {
                Button::new(SharedString::from(format!(
                    "elements-force-{}",
                    label.to_lowercase().replace(' ', "-")
                )))
                .label(label)
                .size(ButtonSize::Small)
                .style(ButtonStyle::Subtle)
                .toggle_state(selection.forced.contains(state))
                .disabled(!selection.editable)
                .tooltip(format!("Style it as if {}", label.to_lowercase()))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.toggle_forced(state, window, cx)),
                )
            }));
        let forced = div()
            .px(theme.metrics.gutter)
            .py(px(2.))
            .flex()
            .items_start()
            .gap_1()
            .child(
                div()
                    .flex_none()
                    .w(px(44.))
                    .h(theme.metrics.control_small)
                    .flex()
                    .items_center()
                    .text_size(theme.metrics.text_small)
                    .text_color(colors.text_muted)
                    .child("Force"),
            )
            .child(toggles);
        div()
            .flex()
            .flex_col()
            .child(flags_row)
            .when(selection.node.element().is_some(), |this| {
                this.child(forced)
            })
            .into_any_element()
    }
}

fn render_missing(selection: &Selection, shown: Option<u64>, theme: &Theme) -> impl IntoElement {
    let colors = &theme.colors;
    let last_seen = selection.seen_in;
    div()
        .mx(theme.metrics.gutter)
        .mb_1()
        .px_2()
        .py_1()
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(theme.metrics.radius)
        .bg(colors.warn.opacity(0.14))
        .text_size(theme.metrics.text_small)
        .text_color(colors.text)
        .child(
            Icon::new(IconName::Warning)
                .size(theme.metrics.icon_small)
                .color(colors.warn),
        )
        .child(div().flex_1().min_w_0().child(match shown {
            Some(frame) => {
                format!("Not in frame #{frame}. Showing what it was in frame #{last_seen}.")
            }
            None => format!("Showing what it was in frame #{last_seen}."),
        }))
}

fn render_why(
    why: Option<&crate::analysis::why_size::SizeExplanation>,
    theme: &Theme,
) -> AnyElement {
    let colors = &theme.colors;
    let Some(why) = why else {
        return div()
            .px(theme.metrics.gutter)
            .pb_1()
            .text_color(colors.text_faint)
            .child("Not captured for this element.")
            .into_any_element();
    };
    let line = |label: &'static str, axis: &AxisExplanation| {
        div()
            .flex()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .w(px(44.))
                    .text_size(theme.metrics.text_small)
                    .text_color(colors.text_muted)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(colors.text)
                    .child(axis.sentence()),
            )
    };
    div()
        .px(theme.metrics.gutter)
        .pb_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(line("Width", &why.width))
        .child(line("Height", &why.height))
        .into_any_element()
}

fn render_details(
    tree: &ElementTree,
    ix: ElementIndex,
    details: &ElementDetails,
    theme: &Theme,
) -> AnyElement {
    let colors = &theme.colors;
    let Some(record) = tree.get(ix) else {
        return div().into_any_element();
    };
    let bounds = format!(
        "{} at {}",
        format::size(record.bounds.size),
        format::point(record.bounds.origin)
    );
    let row = |label: SharedString, value: AnyElement| {
        div()
            .h(theme.metrics.property_row)
            .px(theme.metrics.gutter)
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .w(px(96.))
                    .truncate()
                    .text_color(colors.text_muted)
                    .child(label),
            )
            .child(div().flex_1().min_w_0().flex().items_center().child(value))
    };
    let mut rows: Vec<AnyElement> = detail_facts(details, bounds)
        .into_iter()
        .map(|(label, fact)| {
            let value = match fact {
                Fact::Text(text) => render_value(&PropertyValue::Text(text), theme),
                Fact::Value(value) => render_value(&value, theme),
            };
            row(label, value).into_any_element()
        })
        .collect();
    if let Some(contrast) = text_contrast(tree, ix) {
        let passes = contrast.ratio >= contrast.required;
        let verdict = if passes {
            Pill::new("AA").tone(Tone::Ok)
        } else {
            Pill::new(format!(
                "fails AA · needs {}:1",
                format::number(contrast.required)
            ))
            .tone(Tone::Crit)
        };
        rows.push(
            row(
                "Contrast".into(),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_family(MONO_FONT)
                            .text_size(theme.metrics.mono)
                            .child(format!("{:.1}:1", contrast.ratio)),
                    )
                    .child(verdict)
                    .into_any_element(),
            )
            .into_any_element(),
        );
    }
    div()
        .pb_1()
        .flex()
        .flex_col()
        .children(rows)
        .into_any_element()
}

fn render_cost(selection: &Selection, theme: &Theme) -> AnyElement {
    let colors = &theme.colors;
    let cost = &selection.cost;
    let line = |label: &'static str, value: String| {
        div()
            .px(theme.metrics.gutter)
            .py(px(4.))
            .flex()
            .items_start()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .w(px(96.))
                    .text_color(colors.text_muted)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(colors.text)
                    .child(value),
            )
    };
    let mut lines = vec![line(
        "Primitives",
        format!(
            "{} with its children",
            format::count(u64::from(cost.primitives))
        ),
    )];
    if let Some(view) = &cost.view {
        lines.push(line(
            "Renders",
            format!(
                "{} of {} frames · {} cached",
                view.stats.rendered, view.frames, view.stats.cached
            ),
        ));
        lines.push(line(
            "Last render",
            match &view.last {
                Some((cause, frame, ago)) => format!("{cause} · #{frame} · {ago}"),
                None => "Not in the recorded frames".into(),
            },
        ));
    }
    if let Some((owner, stats)) = &cost.owner {
        lines.push(line(
            "Its view",
            format!(
                "{owner} · {} renders · {} cached",
                stats.rendered, stats.cached
            ),
        ));
    }
    div()
        .pb_1()
        .flex()
        .flex_col()
        .children(lines)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{self, ElementSpec, TreeBuilder};
    use gpui::{inspector::InspectorCapture, rgb, rgb_to_hsla};

    #[test]
    fn contrast_composites_the_backgrounds_under_the_text() {
        let mut capture = InspectorCapture::new_for_test();
        let mut builder = TreeBuilder::new(&mut capture);
        builder.open(ElementSpec::div().styled(0xffffff, 0.));
        builder.open(ElementSpec::div());
        builder.leaf(ElementSpec::text("Hello"));
        builder.close();
        builder.close();
        let tree = builder.build(0);
        let contrast = text_contrast(&tree, 2).expect("text on white");
        // #1f2328 on white.
        assert!((contrast.ratio - 15.8).abs() < 0.2, "{contrast:?}");
        assert_eq!(contrast.required, 4.5);

        let mut pale = tree.clone();
        pale.elements[2].details.as_mut().unwrap().text_color = Some(rgb_to_hsla(rgb(0xcccccc)));
        let contrast = text_contrast(&pale, 2).unwrap();
        assert!(contrast.ratio < contrast.required);
        assert_eq!(text_contrast(&tree, 1), None, "no text color");
    }

    #[test]
    fn flags_are_named_most_relevant_first() {
        let pills =
            flag_pills(ElementFlags::HITBOX | ElementFlags::CLICKABLE | ElementFlags::CLIPPED);
        assert_eq!(
            pills,
            [
                ("Clickable", Tone::Accent),
                ("Hitbox", Tone::Neutral),
                ("Clipped away", Tone::Warn)
            ]
        );
        assert!(flag_pills(ElementFlags::empty()).is_empty());
    }

    #[test]
    fn details_list_what_the_element_reported() {
        let (capture, _) = fixtures::inbox();
        let tree = capture.latest_tree().unwrap();
        let rows_list = tree
            .elements
            .iter()
            .find(|record| record.id.as_deref() == Some("rows"))
            .unwrap();
        let facts = detail_facts(rows_list.details.as_deref().unwrap(), "b".into());
        let labels: Vec<&str> = facts.iter().map(|(label, _)| label.as_ref()).collect();
        assert_eq!(labels, ["Bounds", "List", "Scroll"]);
        let Fact::Text(list) = &facts[1].1 else {
            panic!("text")
        };
        assert_eq!(list, "40 items · showing 0–17");
        let Fact::Text(scroll) = &facts[2].1 else {
            panic!("text")
        };
        assert_eq!(scroll, "offset (0, 0) · content 620×2560");
    }
}
