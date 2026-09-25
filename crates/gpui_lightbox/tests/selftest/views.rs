//! Purpose-built views in Loupe's visual language (DESIGN.md), and broken
//! variants with one deliberate flaw each.

use gpui::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, ClickEvent, Context,
    DurationWithEasing as _, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    KeyBinding, KeyDownEvent, MouseButton, MouseMoveEvent, ParentElement as _, Pixels, Render,
    SharedString, SpringAnimation, SpringConfig, StatefulInteractiveElement as _, Styled as _,
    Window, WindowAppearance, actions, div, ease_in_out, prelude::FluentBuilder as _, px,
};
use gpui_lightbox::{
    MONO_FONT, UI_FONT,
    theme::{DARK, LIGHT, Palette},
};
use std::time::Duration;

actions!(selftest, [Toggle]);

/// The palette matching the window's appearance.
pub fn palette(window: &Window) -> Palette {
    match window.appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => DARK,
        WindowAppearance::Light | WindowAppearance::VibrantLight => LIGHT,
    }
}

/// How long hover transitions take.
pub const HOVER: Duration = Duration::from_millis(160);

/// The deliberate mistake a [`Card`] makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flaw {
    /// A clean card.
    None,
    /// 12 px padding inside a 1 px border: content 13 px in, off the 4 px grid.
    OffGridPadding,
    /// The title asks for a font the design doesn't use.
    RogueFont,
    /// Body text in the faint color, below WCAG AA against the card.
    LowContrast,
    /// A file name cut off by a fixed-width container, without an ellipsis.
    ClippedText,
    /// Two stat labels drawn on top of each other.
    OverlappingLabels,
    /// The whole card moved 1 px right (for golden tests).
    ShiftedOnePixel,
}

/// A styled card: title with a status dot, body copy, stats in Lilex, an
/// ellipsized attachment, and Cancel / Save buttons with hover transitions.
pub struct Card {
    flaw: Flaw,
    saves: usize,
}

impl Card {
    pub fn new(flaw: Flaw) -> Self {
        Self { flaw, saves: 0 }
    }

    pub fn saves(&self) -> usize {
        self.saves
    }
}

impl Render for Card {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(window);
        let dark = p == DARK;
        let flaw = self.flaw;
        let padding = if flaw == Flaw::OffGridPadding {
            12.
        } else {
            15.
        };
        let inset = if flaw == Flaw::ShiftedOnePixel {
            33.
        } else {
            32.
        };

        let title = div()
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .child("Render pipeline")
            .when(flaw == Flaw::RogueFont, |title| {
                title.font_family("Comic Sans MS")
            });
        let status = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(div().size(px(8.)).rounded(px(4.)).bg(p.ok))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(p.text_muted)
                    .child("healthy"),
            );
        let body_color = if flaw == Flaw::LowContrast {
            p.text_faint
        } else {
            p.text_muted
        };
        let stat = |label: &'static str, value: &'static str| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(16.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(p.text_muted)
                        .child(label),
                )
                .child(div().font_family(MONO_FONT).child(value))
        };
        let stats = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(stat("render", "3.1 ms"))
            .child(stat("layout", "0.8 ms"))
            .child(stat("paint", "1.2 ms"))
            .when(flaw == Flaw::OverlappingLabels, |stats| {
                stats.child(
                    div()
                        .relative()
                        .h(px(16.))
                        .child(div().absolute().left_0().child("prepaint"))
                        .child(div().absolute().left(px(28.)).child("present")),
                )
            });
        let attachment = if flaw == Flaw::ClippedText {
            div()
                .w(px(120.))
                .overflow_hidden()
                .whitespace_nowrap()
                .child("quarterly-report-final-v2.pdf")
        } else {
            div()
                .w(px(200.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child("quarterly-report-final-v2-reviewed.pdf")
        };

        let save_bg = if dark {
            p.accent.with_alpha(0.16)
        } else {
            p.accent
        };
        let save_hover = if dark {
            p.accent.with_alpha(0.26)
        } else {
            gpui_lightbox::Color::from_u32(0x1f58c9)
        };
        let save = button("save", "Save")
            .bg(save_bg)
            .border_color(if dark {
                p.accent.with_alpha(0.5)
            } else {
                save_bg
            })
            .text_color(if dark {
                p.accent
            } else {
                gpui_lightbox::Color::WHITE
            })
            .hover(move |style| style.bg(save_hover))
            .on_click(cx.listener(|card, _: &ClickEvent, _, cx| {
                card.saves += 1;
                cx.notify();
            }));
        let cancel = button("cancel", "Cancel")
            .bg(p.surface_2)
            .border_color(p.line_strong)
            .hover(move |style| style.bg(p.line));

        div()
            .size_full()
            .bg(p.bg)
            .font_family(UI_FONT)
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(p.text)
            .p(px(inset))
            .child(
                div()
                    .w(px(360.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .p(px(padding))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.line)
                    .rounded(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(title)
                            .child(status),
                    )
                    .child(div().text_color(body_color).child(
                        "Frames are drawn in five phases. The slowest one decides whether a frame fits its budget.",
                    ))
                    .child(stats)
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .text_color(p.text_muted)
                            .child(div().text_size(px(11.)).child("Attachment"))
                            .child(attachment),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(p.text_muted)
                                    .child(saved_label(self.saves)),
                            )
                            .child(div().flex().gap(px(8.)).child(cancel).child(save)),
                    ),
            )
    }
}

fn saved_label(saves: usize) -> SharedString {
    match saves {
        0 => "Not saved yet".into(),
        1 => "Saved 1 time".into(),
        count => format!("Saved {count} times").into(),
    }
}

/// A 24 px button whose background eases on hover.
fn button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(24.))
        .px(px(11.))
        .border_1()
        .rounded(px(4.))
        .cursor_pointer()
        .child(label)
        .transitions(|transitions| transitions.bg(HOVER.with_easing(ease_in_out)))
}

/// A panel that slides in on a spring when "Open" is clicked.
pub struct SpringPanel {
    open: bool,
    config: SpringConfig,
}

/// Where the closed panel waits, off screen.
pub const PANEL_CLOSED: f32 = -260.;
/// Where the open panel rests.
pub const PANEL_OPEN: f32 = 24.;
/// The panel's width, to find its quad.
pub const PANEL_WIDTH: f32 = 240.;

impl SpringPanel {
    /// A bouncy (underdamped) spring.
    pub fn bouncy() -> Self {
        Self {
            open: false,
            config: SpringConfig::new(260., 14., 1.),
        }
    }

    /// A critically damped spring: fast, no overshoot.
    pub fn smooth() -> Self {
        Self {
            open: false,
            config: SpringConfig::new(170., 26.1, 1.),
        }
    }
}

impl Render for SpringPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(window);
        let target = px(if self.open { PANEL_OPEN } else { PANEL_CLOSED });
        div()
            .size_full()
            .relative()
            .bg(p.bg)
            .font_family(UI_FONT)
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(p.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(32.))
                    .px(px(12.))
                    .bg(p.surface)
                    .border_b_1()
                    .border_color(p.line)
                    .child("Inspector")
                    .child(
                        button("toggle", if self.open { "Close" } else { "Open" })
                            .bg(p.surface_2)
                            .border_color(p.line_strong)
                            .on_click(cx.listener(|panel, _: &ClickEvent, _, cx| {
                                panel.open = !panel.open;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px(56.))
                    .w(px(PANEL_WIDTH))
                    .h(px(128.))
                    .p(px(15.))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.line_strong)
                    .rounded(px(8.))
                    .child("Frame #18372")
                    .child(
                        div()
                            .text_color(p.text_muted)
                            .child("23.4 ms · render 14.1"),
                    )
                    .with_spring(
                        "panel",
                        SpringAnimation::new(self.config)
                            .to(target)
                            .from(px(PANEL_CLOSED))
                            .with_epsilon(0.05),
                        |panel, x: Pixels| panel.left(x),
                    ),
            )
    }
}

/// A bar that slides 0 → 200 px with a style transition when toggled,
/// using `easing` over [`SLIDE`].
pub struct Slider {
    right: bool,
    easing: fn(f32) -> f32,
}

/// How long the slide takes.
pub const SLIDE: Duration = Duration::from_millis(300);
/// The bar's left edge before sliding.
pub const SLIDE_FROM: f32 = 24.;
/// How far it slides.
pub const SLIDE_BY: f32 = 200.;

impl Slider {
    pub fn new(easing: fn(f32) -> f32) -> Self {
        Self {
            right: false,
            easing,
        }
    }
}

impl Render for Slider {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(window);
        let easing = self.easing;
        div()
            .size_full()
            .relative()
            .bg(p.bg)
            .font_family(UI_FONT)
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(p.text)
            .child(
                button("slide", "Slide")
                    .absolute()
                    .top(px(16.))
                    .left(px(24.))
                    .bg(p.surface_2)
                    .border_color(p.line_strong)
                    .on_click(cx.listener(|slider, _: &ClickEvent, _, cx| {
                        slider.right = !slider.right;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("bar")
                    .absolute()
                    .top(px(64.))
                    .left(px(if self.right {
                        SLIDE_FROM + SLIDE_BY
                    } else {
                        SLIDE_FROM
                    }))
                    .w(px(64.))
                    .h(px(24.))
                    .rounded(px(4.))
                    .bg(p.accent)
                    .transitions(move |transitions| transitions.left(SLIDE.with_easing(easing))),
            )
    }
}

/// A list of 40 rows in a scroll container.
pub struct ScrollList;

impl Render for ScrollList {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = palette(window);
        div()
            .size_full()
            .bg(p.bg)
            .p(px(24.))
            .font_family(UI_FONT)
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(p.text)
            .child(
                div()
                    .id("list")
                    .w(px(280.))
                    .h(px(177.))
                    .overflow_y_scroll()
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.line)
                    .children((0..40).map(|ix| row(ix, &p))),
            )
    }
}

fn row(ix: usize, p: &Palette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .h(px(22.))
        .px(px(12.))
        .when(ix % 2 == 1, |row| row.bg(p.surface_2))
        .child(format!("Row {ix:02}"))
        .child(
            div()
                .font_family(MONO_FONT)
                .text_color(p.text_muted)
                .child(format!("{:.1} ms", 1. + ix as f32 * 0.3)),
        )
        .into_any_element()
}

/// Records typed characters and toggles on `ctrl-k`, to prove keyboard
/// input goes through real dispatch and key bindings.
pub struct Keys {
    pub typed: String,
    pub toggled: bool,
    focus: FocusHandle,
}

impl Keys {
    pub fn mount(window: &mut Window, cx: &mut gpui::App) -> Entity<Self> {
        cx.bind_keys([KeyBinding::new("ctrl-k", Toggle, Some("Keys"))]);
        let keys = cx.new(|cx| Self {
            typed: String::new(),
            toggled: false,
            focus: cx.focus_handle(),
        });
        let focus = keys.read(cx).focus.clone();
        window.focus(&focus, cx);
        keys
    }
}

impl Render for Keys {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(window);
        div()
            .track_focus(&self.focus)
            .key_context("Keys")
            .on_action(cx.listener(|keys, _: &Toggle, _, cx| {
                keys.toggled = !keys.toggled;
                cx.notify();
            }))
            .on_key_down(cx.listener(|keys, event: &KeyDownEvent, _, cx| {
                if let Some(typed) = &event.keystroke.key_char
                    && !event.keystroke.modifiers.control
                {
                    keys.typed.push_str(typed);
                    cx.notify();
                }
            }))
            .size_full()
            .bg(p.bg)
            .p(px(24.))
            .font_family(UI_FONT)
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(p.text)
            .child(format!("Typed: {}", self.typed))
            .child(if self.toggled {
                "Palette open"
            } else {
                "Palette closed"
            })
    }
}

/// A track with a knob that follows the mouse while the button is held.
pub struct Knob {
    pub x: f32,
}

impl Render for Knob {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(window);
        div().size_full().bg(p.bg).p(px(24.)).child(
            div()
                .id("track")
                .relative()
                .w(px(320.))
                .h(px(24.))
                .bg(p.surface_2)
                .rounded(px(4.))
                .on_mouse_move(cx.listener(|knob, event: &MouseMoveEvent, _, cx| {
                    if event.pressed_button == Some(MouseButton::Left) {
                        knob.x = (event.position.x.as_f32() - 24. - 8.).clamp(0., 304.);
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .absolute()
                        .left(px(self.x))
                        .top(px(4.))
                        .size(px(16.))
                        .rounded(px(8.))
                        .bg(p.accent),
                ),
        )
    }
}

/// How long the janky animation runs.
pub const JANK: Duration = Duration::from_millis(400);

/// An easing with two bugs: it stalls from 20 % to 46 % of the time, and
/// teleports to the end at 70 %.
pub fn janky(t: f32) -> f32 {
    if t < 0.2 {
        t
    } else if t < 0.46 {
        0.2
    } else if t < 0.7 {
        t - 0.2
    } else {
        1.
    }
}

/// A bar animated with [`janky`] from the moment it mounts.
pub struct Janky;

impl Render for Janky {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = palette(window);
        div().size_full().relative().bg(p.bg).child(
            div()
                .absolute()
                .top(px(24.))
                .w(px(64.))
                .h(px(24.))
                .rounded(px(4.))
                .bg(p.accent)
                .with_animation(
                    "janky",
                    Animation::new(JANK).with_easing(janky),
                    |bar, t| bar.left(px(SLIDE_FROM + SLIDE_BY * t)),
                ),
        )
    }
}
