//! The key tester: press keys and see what they would do in the app,
//! without dispatching anything.
//!
//! While its capture box has the focus, a keystroke interceptor (which runs
//! before GPUI matches any binding) stops every key: nothing reaches the
//! app's bindings, Loupe's own bindings or any key listener's handling. The
//! keys are resolved with `Window::inspector_resolve_keystrokes_for`
//! against the app's focus: the element focused before Loupe took the focus.

use crate::{
    analysis::{
        events::context_label,
        key_tester::{KeySequence, Outcome, Press, Verdict, explain, waiting_note},
    },
    loupe::LOUPE_CONTEXT,
    theme::{MONO_FONT, Theme},
    widgets::{Button, ButtonSize, Icon, IconName, Kbd, Pill, Prose, SectionHeader, Tone},
};
use gpui::{
    Context, EventEmitter, FocusHandle, IntoElement, KeystrokeEvent, MouseButton, Render,
    Subscription, WeakFocusHandle, Window, div,
    inspector::{BindingVerdict, KeyResolution},
    prelude::*,
    px,
};
use std::time::Instant;

/// What the key tester tells the Events lens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyTesterEvent {
    /// Escape three times: stop listening and hand the focus back.
    Left,
}

/// The key tester's state.
pub(super) struct KeyTester {
    focus: FocusHandle,
    app_focus: Option<WeakFocusHandle>,
    sequence: KeySequence,
    _subscriptions: [Subscription; 3],
}

impl EventEmitter<KeyTesterEvent> for KeyTester {}

/// Whether the window's focus is inside Loupe (under its key context).
fn loupe_has_focus(window: &Window) -> bool {
    window
        .context_stack()
        .iter()
        .any(|context| context.contains(LOUPE_CONTEXT))
}

impl KeyTester {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let tester = cx.entity().downgrade();
        let subscriptions = [
            cx.intercept_keystrokes(move |event, window, cx| {
                tester
                    .update(cx, |tester, cx| tester.intercept(event, window, cx))
                    .ok();
            }),
            // Programmatic focus changes. Focus listeners run while the window
            // draws, when a notify can't schedule another frame, so re-render
            // once the draw is over. (Clicks re-render directly: see the box.)
            cx.on_focus(&focus, window, |_, window, cx| {
                cx.defer_in(window, |_, _, cx| cx.notify())
            }),
            cx.on_blur(&focus, window, |_, window, cx| {
                cx.defer_in(window, |_, _, cx| cx.notify())
            }),
        ];
        let mut tester = Self {
            focus,
            app_focus: None,
            sequence: KeySequence::default(),
            _subscriptions: subscriptions,
        };
        tester.note_app_focus(window, cx);
        tester
    }

    /// Remembers what has the focus in the app, unless Loupe has it.
    pub fn note_app_focus(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !loupe_has_focus(window) {
            self.app_focus = window.focused(cx).map(|focus| focus.downgrade());
        }
    }

    /// Captures a keystroke while listening: stops it before any binding
    /// sees it and resolves it instead.
    fn intercept(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        cx.stop_propagation();
        let app_focus = self.app_focus.as_ref().and_then(WeakFocusHandle::upgrade);
        let app: &gpui::App = cx;
        let press = self
            .sequence
            .press(event.keystroke.clone(), Instant::now(), |keys| {
                window.inspector_resolve_keystrokes_for(keys, app_focus.as_ref(), app)
            });
        if press == Press::Leave {
            cx.emit(KeyTesterEvent::Left);
        }
        cx.notify();
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sequence.reset();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn render_capture_box(
        &self,
        listening: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = &theme.colors;
        let strokes = self.sequence.strokes();
        let waiting = self
            .sequence
            .resolution()
            .is_some_and(KeyResolution::is_pending);
        let prompt = if listening {
            "Listening… press keys"
        } else {
            "Press keys to see what they do here…"
        };
        div()
            .id("events-key-tester")
            .debug_selector(|| "events-key-tester".into())
            .track_focus(&self.focus)
            .flex_none()
            .min_h(px(40.))
            .px(px(10.))
            .py(px(6.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .rounded(theme.metrics.radius)
            .border_1()
            .border_color(if listening {
                colors.accent
            } else {
                colors.line_strong
            })
            .bg(if listening {
                colors.selected
            } else {
                colors.surface
            })
            .cursor_pointer()
            // Runs before the click moves the focus here: note where it was,
            // and re-render with the new focus once the click is handled.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.note_app_focus(window, cx);
                    cx.notify();
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.focus.is_focused(window) {
                    cx.notify();
                }
            }))
            .child(Icon::new(IconName::Keyboard).color(if listening {
                colors.accent
            } else {
                colors.text_muted
            }))
            .when(strokes.is_empty(), |this| {
                this.child(
                    div()
                        .text_color(if listening {
                            colors.text
                        } else {
                            colors.text_muted
                        })
                        .child(prompt),
                )
            })
            .children(strokes.iter().map(|stroke| Kbd::new(stroke.unparse())))
            .when(waiting, |this| {
                this.child(div().text_color(colors.text_faint).child("…"))
            })
            .child(div().flex_1())
            .when(listening, |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_size(theme.metrics.text_small)
                        .text_color(colors.text_faint)
                        .child("Esc Esc clears"),
                )
            })
    }

    fn render_resolution(
        &self,
        resolution: &KeyResolution,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = &theme.colors;
        let (bar, sentence) = match Outcome::of(resolution) {
            Outcome::Runs(sentence) => (colors.ok, sentence),
            Outcome::Waiting(sentence) => (colors.accent, sentence),
            Outcome::Nothing(sentence) => (colors.text_faint, sentence),
        };
        let card = div()
            .flex()
            .rounded(theme.metrics.radius)
            .overflow_hidden()
            .bg(colors.surface_2)
            .border_1()
            .border_color(colors.line)
            .child(div().flex_none().w(px(3.)).bg(bar))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(Prose::new(sentence))
                    .children(waiting_note(resolution).map(|note| {
                        Prose::new(note)
                            .color(colors.text_muted)
                            .size(theme.metrics.text_small)
                    })),
            );

        let contexts = if resolution.context_stack.is_empty() {
            div()
                .px(theme.metrics.gutter)
                .text_color(colors.text_muted)
                .child("Nothing in the app had focus, so the keys go to the window root.")
                .into_any_element()
        } else {
            context_chips(resolution.context_stack.iter().map(context_label), theme)
                .px(theme.metrics.gutter)
                .into_any_element()
        };

        let losers: Vec<usize> = (0..resolution.candidates.len())
            .filter(|&ix| resolution.candidates[ix].verdict != BindingVerdict::Wins)
            .collect();
        let loser_count = losers.len();
        let rows = losers
            .into_iter()
            .map(|ix| candidate_row(resolution, ix, theme));

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(card)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        SectionHeader::new("Resolved in").action(
                            Button::new("events-key-reset")
                                .label("Reset")
                                .size(ButtonSize::Small)
                                .tooltip_keys("Clear the keys", "escape escape")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.reset(window, cx)),
                                ),
                        ),
                    )
                    .child(contexts),
            )
            .when(loser_count > 0, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(SectionHeader::new("Other bindings").detail(loser_count.to_string()))
                        .children(rows),
                )
            })
    }
}

/// Key context chips, outermost first: `[Workspace] › [Pane] › [Editor]`.
pub(super) fn context_chips(
    contexts: impl IntoIterator<Item = String>,
    theme: &Theme,
) -> gpui::Div {
    let colors = &theme.colors;
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .children(contexts.into_iter().enumerate().map(|(ix, context)| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .when(ix > 0, |this| {
                    this.child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(10.))
                            .color(colors.text_faint),
                    )
                })
                .child(
                    div()
                        .h(px(18.))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(colors.line_strong)
                        .bg(colors.surface_2)
                        .font_family(MONO_FONT)
                        .text_size(theme.metrics.mono)
                        .text_color(colors.text)
                        .whitespace_nowrap()
                        .child(context),
                )
        }))
}

fn verdict_tone(verdict: Verdict) -> Tone {
    match verdict {
        Verdict::Runs => Tone::Ok,
        Verdict::Pending => Tone::Accent,
        Verdict::Disabled | Verdict::Disables | Verdict::Unhandled => Tone::Warn,
        Verdict::Shadowed | Verdict::Context => Tone::Neutral,
    }
}

/// A losing binding: its keys, action and verdict, then why it loses.
fn candidate_row(resolution: &KeyResolution, ix: usize, theme: &Theme) -> impl IntoElement + use<> {
    let colors = &theme.colors;
    let candidate = &resolution.candidates[ix];
    let verdict = Verdict::of(resolution, ix);
    div()
        .px(theme.metrics.gutter)
        .py(px(5.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .border_b_1()
        .border_color(colors.line)
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(Kbd::new(candidate.keystrokes.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(MONO_FONT)
                        .text_size(theme.metrics.mono)
                        .text_color(colors.text)
                        .child(candidate.action.clone()),
                )
                .child(Pill::new(verdict.label()).tone(verdict_tone(verdict))),
        )
        .child(
            Prose::new(explain(resolution, ix))
                .color(colors.text_muted)
                .size(theme.metrics.text_small),
        )
}

impl Render for KeyTester {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let listening = self.focus.is_focused(window);
        let body = match self.sequence.resolution() {
            Some(resolution) => self
                .render_resolution(resolution, theme, cx)
                .into_any_element(),
            None => div()
                .px(px(2.))
                .text_size(theme.metrics.text_small)
                .text_color(colors.text_faint)
                .child(
                    "Keys resolve against the app's focus without being dispatched: nothing runs, \
                     not even Loupe's shortcuts. Type the keys of a multi-key binding within a \
                     second. Click elsewhere to stop.",
                )
                .into_any_element(),
        };
        div()
            .id("events-key-tester-pane")
            .size_full()
            .overflow_y_scroll()
            .p(theme.metrics.gutter)
            .flex()
            .flex_col()
            .gap_2()
            .child(self.render_capture_box(listening, theme, cx))
            .child(body)
    }
}
