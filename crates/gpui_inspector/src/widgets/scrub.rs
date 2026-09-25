//! Scrub number fields: drag sideways to change a value, nudge it with the
//! arrow keys (shift for ×10), or click to type an exact number.

use crate::{
    theme::{MONO_FONT, Theme},
    widgets::{
        SCRUB_CONTEXT, ScrubDecrement, ScrubDecrementCoarse, ScrubIncrement, ScrubIncrementCoarse,
    },
};
use gpui::{
    App, AppContext as _, Context, DragMoveEvent, Empty, Entity, EntityId, EventEmitter,
    FocusHandle, Focusable, IntoElement, Modifiers, MouseButton, MouseDownEvent, Pixels, Render,
    SharedString, Styled, Subscription, Window, div, prelude::*, px,
};
use gpui_elements::editable_text::{
    EditableTextState, StringStorage,
    actions::{Enter, Escape},
    text_input,
};

/// Pixels of pointer travel per step while scrubbing.
pub const PIXELS_PER_STEP: f32 = 2.;

/// The value after scrubbing `dx` from `start`: one `step` per
/// `PIXELS_PER_STEP` pixels, ×10 with shift, ×0.1 with alt, snapped to the
/// finer of the two steps.
pub fn scrub_value(start: f32, dx: Pixels, step: f32, modifiers: Modifiers) -> f32 {
    let scaled = step * step_scale(modifiers);
    let steps = (f32::from(dx) / PIXELS_PER_STEP).trunc();
    snap(start + steps * scaled, scaled.min(step))
}

/// The value after one arrow key press: ±`step`, ×10 with shift, snapped to `step`.
pub fn nudge(value: f32, step: f32, up: bool, coarse: bool) -> f32 {
    let delta = if coarse { step * 10. } else { step };
    snap(if up { value + delta } else { value - delta }, step)
}

/// Parses a typed number, ignoring surrounding space and a trailing unit
/// (`12`, `-3.5`, `12px`, ` 40 % `).
pub fn parse_number(text: &str) -> Option<f32> {
    let text = text.trim();
    let number = text.trim_end_matches(|c: char| c.is_alphabetic() || c == '%' || c == ' ');
    number.parse::<f32>().ok().filter(|value| value.is_finite())
}

/// Formats a value with at most `precision` decimals and no trailing zeros.
pub fn format_number(value: f32, precision: usize) -> String {
    let text = format!("{value:.precision$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        &text
    };
    match text {
        "-0" => "0".to_string(),
        text => text.to_string(),
    }
}

fn step_scale(modifiers: Modifiers) -> f32 {
    if modifiers.shift {
        10.
    } else if modifiers.alt {
        0.1
    } else {
        1.
    }
}

fn snap(value: f32, step: f32) -> f32 {
    if step > 0. {
        (value / step).round() * step
    } else {
        value
    }
}

/// Emitted by a [`ScrubField`] whenever its value changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrubChanged(pub f32);

/// The value carried while a scrub field is dragged.
struct ScrubDrag {
    field: EntityId,
}

struct DragOrigin {
    x: Pixels,
    value: f32,
}

/// A numeric field that can be scrubbed, nudged or typed into.
///
/// Focus it and press up / down (shift for ×10) to nudge, drag sideways to
/// scrub (shift ×10, alt ×0.1), or click (or press enter) to type a value;
/// enter or blur commits, escape cancels. Emits [`ScrubChanged`].
pub struct ScrubField {
    value: f32,
    step: f32,
    precision: usize,
    unit: Option<SharedString>,
    compact: bool,
    focus: FocusHandle,
    drag: Option<DragOrigin>,
    editor: Option<(Entity<EditableTextState>, Subscription)>,
}

impl EventEmitter<ScrubChanged> for ScrubField {}

impl Focusable for ScrubField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ScrubField {
    /// A field showing `value`, changing by `step` per notch.
    pub fn new(value: f32, step: f32, cx: &mut Context<Self>) -> Self {
        Self {
            value,
            step,
            precision: 2,
            unit: None,
            compact: false,
            focus: cx.focus_handle(),
            drag: None,
            editor: None,
        }
    }

    /// Shows a unit after the value, e.g. `px`.
    pub fn with_unit(mut self, unit: impl Into<SharedString>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// Draws only the value until hovered or focused, centered and as
    /// narrow as it fits, for values embedded in a diagram; zero is dimmed.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// Maximum decimals shown (2 by default).
    pub fn with_precision(mut self, precision: usize) -> Self {
        self.precision = precision;
        self
    }

    /// The current value.
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Whether the field is showing its text editor.
    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    /// Replaces the value without emitting an event (e.g. new data arrived).
    pub fn set_value(&mut self, value: f32, cx: &mut Context<Self>) {
        if self.value != value {
            self.value = value;
            cx.notify();
        }
    }

    fn change(&mut self, value: f32, cx: &mut Context<Self>) {
        if self.value != value {
            self.value = value;
            cx.emit(ScrubChanged(value));
            cx.notify();
        }
    }

    fn nudge_by(&mut self, up: bool, coarse: bool, cx: &mut Context<Self>) {
        let value = nudge(self.value, self.step, up, coarse);
        self.change(value, cx);
    }

    /// Switches to typing: shows an editor with the value selected.
    pub fn start_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() {
            return;
        }
        let text = format_number(self.value, self.precision);
        let editor = cx.new(|cx| {
            let mut state = EditableTextState::new(StringStorage::from(text.as_str()), cx);
            state.select_document(cx);
            state
        });
        let editor_focus = editor.read(cx).focus_handle(cx);
        window.focus(&editor_focus, cx);
        let blur = cx.on_blur(&editor_focus, window, |this, _, cx| {
            this.finish_editing(true, cx)
        });
        self.editor = Some((editor, blur));
        cx.notify();
    }

    fn finish_editing(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some((editor, _)) = self.editor.take() else {
            return;
        };
        if commit && let Some(value) = parse_number(editor.read(cx).as_str()) {
            self.change(value, cx);
        }
        cx.notify();
    }

    fn begin_drag(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.drag = Some(DragOrigin {
            x: event.position.x,
            value: self.value,
        });
        window.focus(&self.focus, cx);
    }

    fn drag_to(&mut self, event: &DragMoveEvent<ScrubDrag>, cx: &mut Context<Self>) {
        if event.drag(cx).field != cx.entity_id() {
            return;
        }
        let Some(origin) = &self.drag else {
            return;
        };
        let dx = event.event.position.x - origin.x;
        let value = scrub_value(origin.value, dx, self.step, event.event.modifiers);
        self.change(value, cx);
    }
}

impl Render for ScrubField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let focused = self.focus.contains_focused(window, cx);
        let (fill, border) = match (self.compact, focused) {
            (_, true) => (colors.surface_2, colors.accent),
            (true, false) => (gpui::transparent_black(), gpui::transparent_black()),
            (false, false) => (colors.surface_2, colors.line),
        };
        let text_color = if self.compact && self.value == 0. && !focused {
            colors.text_faint
        } else {
            colors.text
        };
        let field = div()
            .id("scrub")
            .key_context(SCRUB_CONTEXT)
            .track_focus(&self.focus)
            .h(theme.metrics.control_small)
            .min_w(if self.compact { px(22.) } else { px(40.) })
            .px(if self.compact { px(3.) } else { px(5.) })
            .flex()
            .items_center()
            .when(self.compact, |this| this.justify_center())
            .gap(px(2.))
            .rounded(theme.metrics.radius)
            .bg(fill)
            .border_1()
            .border_color(border)
            .font_family(MONO_FONT)
            .text_size(theme.metrics.mono)
            .line_height(theme.metrics.line_height)
            .text_color(text_color)
            .on_action(
                cx.listener(|this, _: &ScrubIncrement, _, cx| this.nudge_by(true, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ScrubDecrement, _, cx| this.nudge_by(false, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ScrubIncrementCoarse, _, cx| this.nudge_by(true, true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ScrubDecrementCoarse, _, cx| this.nudge_by(false, true, cx)),
            );

        if let Some((editor, _)) = &self.editor {
            return field
                .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                    this.finish_editing(true, cx);
                    window.focus(&this.focus, cx);
                    cx.stop_propagation();
                }))
                .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                    this.finish_editing(false, cx);
                    window.focus(&this.focus, cx);
                    cx.stop_propagation();
                }))
                .child(
                    text_input("scrub-editor")
                        .state(editor.downgrade())
                        .caret_color(colors.accent)
                        .selection_color(colors.selected)
                        .caret_w(px(1.5))
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_x_hidden(),
                );
        }

        let field_id = cx.entity_id();
        field
            .cursor_ew_resize()
            .hover(|style| style.bg(colors.surface_2).border_color(colors.line_strong))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.begin_drag(event, window, cx)),
            )
            .on_drag(ScrubDrag { field: field_id }, |_, _, _, cx| {
                cx.new(|_| Empty)
            })
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ScrubDrag>, _, cx| {
                    this.drag_to(event, cx)
                }),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                // A press that never became a drag (or enter while focused):
                // type a value instead.
                this.drag = None;
                this.start_editing(window, cx);
            }))
            .child(format_number(self.value, self.precision))
            .children(
                self.unit
                    .clone()
                    .map(|unit| div().text_color(colors.text_faint).child(unit)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn scrubbing_moves_one_step_per_two_pixels() {
        let none = Modifiers::default();
        assert_eq!(scrub_value(10., px(0.), 1., none), 10.);
        assert_eq!(scrub_value(10., px(1.), 1., none), 10.);
        assert_eq!(scrub_value(10., px(9.), 1., none), 14.);
        assert_eq!(scrub_value(10., px(-6.), 1., none), 7.);
    }

    #[test]
    fn shift_scrubs_coarse_and_alt_scrubs_fine() {
        let shift = Modifiers::shift();
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        assert_eq!(scrub_value(10., px(4.), 1., shift), 30.);
        assert!((scrub_value(10., px(4.), 1., alt) - 10.2).abs() < 1e-4);
    }

    #[test]
    fn arrows_nudge_by_one_or_ten_steps() {
        assert_eq!(nudge(4., 1., true, false), 5.);
        assert_eq!(nudge(4., 1., false, false), 3.);
        assert_eq!(nudge(4., 1., true, true), 14.);
        assert_eq!(nudge(0.5, 0.5, false, true), -4.5);
    }

    #[test]
    fn parses_numbers_with_units_and_space() {
        assert_eq!(parse_number("12"), Some(12.));
        assert_eq!(parse_number(" -3.5 "), Some(-3.5));
        assert_eq!(parse_number("12px"), Some(12.));
        assert_eq!(parse_number("40 %"), Some(40.));
        assert_eq!(parse_number("wide"), None);
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("inf"), None);
    }

    #[test]
    fn formats_without_trailing_zeros() {
        assert_eq!(format_number(12., 2), "12");
        assert_eq!(format_number(12.5, 2), "12.5");
        assert_eq!(format_number(0.126, 2), "0.13");
        assert_eq!(format_number(-0.001, 2), "0");
        assert_eq!(format_number(100., 0), "100");
    }
}
