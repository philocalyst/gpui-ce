//! The settings popover, opened by the gear in the toolbar, `cmd-,` /
//! `ctrl-,` or the palette.
//!
//! ```text
//! Settings                                   Esc
//! APPEARANCE
//! Theme     [ Window | Dark | Light ]
//! Density   [ Compact | Comfortable ]
//! RECORDING
//! Budget    [ 60 Hz | 120 Hz | 144 Hz ]  16.7 ms per frame
//! Capture   ○ Frames  what it records · what it costs
//!           ○ Tree    …
//!           ◉ Full    …
//! SOURCE LINKS
//! Editor    [ Zed | VS Code | Cursor | IntelliJ | Custom ]
//!           [ subl://open?url=file://{path}&line={line}   ]
//!           zed://file/home/me/app/src/main.rs:42:7
//! ```
//!
//! Every control writes [`LoupeSettings`] straight away; the popover only
//! owns the text of a custom editor template.

use crate::{
    analysis::{
        format,
        source::{Editor, EditorUrl},
    },
    lenses::capture::budget_control,
    settings::{CAPTURE_LEVELS, LoupeSettings, capture_level_label, capture_level_summary},
    theme::{Appearance, Density, MONO_FONT, Theme},
    widgets::{Kbd, SectionHeader, Segment, Segmented, TextField, Tooltip, floating_surface},
};
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, Focusable as _, FontFeatures, FontWeight,
    IntoElement, Pixels, Render, SharedString, Subscription, Window, div, inspector::CaptureLevel,
    prelude::*, px,
};
use gpui_elements::editable_text::{
    EditableTextState, StringStorage, TextChanged, actions::Escape,
};

/// The file the editor preview links to.
pub(crate) const SAMPLE_SOURCE: (&str, u32, u32) = ("/home/me/app/src/main.rs", 42, 7);

/// The template a new custom editor starts from: Sublime Text's scheme,
/// which shows every placeholder.
const CUSTOM_TEMPLATE: &str = "subl://open?url=file://{path}&line={line}&column={col}";

/// Width of the labels before each control.
const LABEL_WIDTH: Pixels = px(60.);
/// Horizontal padding of every row, as in the palette.
const PADDING: Pixels = px(12.);

/// What the popover tells the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsEvent {
    /// Escape in the template field.
    Dismissed,
}

/// The popover's view: the controls, and the custom template being edited.
pub(crate) struct SettingsPanel {
    template: Entity<EditableTextState>,
    _template_changes: Subscription,
}

impl EventEmitter<SettingsEvent> for SettingsPanel {}

impl SettingsPanel {
    /// A popover showing the current settings.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let custom = match &LoupeSettings::get(cx).editor {
            EditorUrl::Custom(template) => template.to_string(),
            EditorUrl::Editor(_) => String::new(),
        };
        let template =
            cx.new(|cx| EditableTextState::new(StringStorage::from(custom.as_str()), cx));
        let changes = cx.subscribe(&template, |_, template, _: &TextChanged, cx| {
            let text = template.read(cx).as_str().trim().to_string();
            if !text.is_empty() {
                LoupeSettings::update(cx, |settings| {
                    settings.editor = EditorUrl::Custom(text.into());
                });
            }
        });
        Self {
            template,
            _template_changes: changes,
        }
    }

    /// Switches to the custom template, starting one if the field is empty,
    /// and focuses the field.
    fn use_custom_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.template.read(cx).as_str().trim().to_string();
        if text.is_empty() {
            // Emits `TextChanged`, which stores the template.
            self.template
                .update(cx, |template, cx| template.emplace(CUSTOM_TEMPLATE, cx));
        } else {
            LoupeSettings::update(cx, |settings| {
                settings.editor = EditorUrl::Custom(text.into());
            });
        }
        let focus = self.template.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn render_editor(
        &self,
        editor: &EditorUrl,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let selected = editor
            .editor()
            .and_then(|editor| Editor::ALL.iter().position(|known| *known == editor))
            .unwrap_or(Editor::ALL.len());
        let picker = Editor::ALL
            .iter()
            .fold(
                Segmented::new("loupe-settings-editor"),
                |control, editor| control.segment(Segment::label(editor.label())),
            )
            .segment(Segment::label("Custom").tooltip("Any URL with {path}, {line} and {col}"))
            .selected(selected)
            .on_select(
                cx.listener(|this, ix: &usize, window, cx| match Editor::ALL.get(*ix) {
                    Some(editor) => {
                        LoupeSettings::update(cx, |settings| settings.editor = (*editor).into());
                    }
                    None => this.use_custom_editor(window, cx),
                }),
            );
        let (path, line, column) = SAMPLE_SOURCE;
        // Wrapped rather than cut: the end of a URL (line, column) matters.
        let preview = div()
            .id("loupe-settings-editor-preview")
            .debug_selector(|| "loupe-settings-editor-preview".into())
            .min_w_0()
            .py(px(4.))
            .font_family(MONO_FONT)
            .font_features(FontFeatures::disable_ligatures())
            .text_size(theme.metrics.mono)
            .text_color(theme.colors.text_muted)
            .child(editor.url(path, line, column))
            .tooltip(Tooltip::text(format!(
                "Where main.rs:{line}:{column} would open"
            )));
        div()
            .flex()
            .flex_col()
            .child(row("Editor", theme).child(picker))
            .when(editor.editor().is_none(), |this| {
                this.child(
                    indented(theme).child(
                        div().flex_1().min_w_0().child(
                            TextField::new("loupe-settings-template", &self.template)
                                .mono()
                                .placeholder(CUSTOM_TEMPLATE),
                        ),
                    ),
                )
            })
            .child(indented(theme).child(preview))
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let settings = LoupeSettings::get(cx).clone();

        let appearance = [
            (
                Appearance::System,
                "Window",
                "Follow the window's appearance",
            ),
            (Appearance::Dark, "Dark", "Always dark"),
            (Appearance::Light, "Light", "Always light"),
        ];
        let theme_picker = choice(
            "loupe-settings-theme",
            &appearance,
            settings.appearance,
            |settings, appearance| settings.appearance = appearance,
        );
        let density = [
            (Density::Compact, "Compact", "22 px rows"),
            (
                Density::Comfortable,
                "Comfortable",
                "26 px rows, larger text",
            ),
        ];
        let density_picker = choice(
            "loupe-settings-density",
            &density,
            settings.density,
            |settings, density| settings.density = density,
        );
        let budget = div()
            .flex_none()
            .text_size(theme.metrics.text_small)
            .text_color(colors.text_faint)
            .child(format!(
                "{} per frame",
                format::duration(settings.budget.duration())
            ));

        floating_surface(theme)
            .id("loupe-settings")
            .debug_selector(|| "loupe-settings".into())
            // The chrome's surface, so segmented tracks stand out.
            .bg(colors.surface)
            .w_full()
            .max_h_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .text_size(theme.metrics.text)
            .text_color(colors.text)
            .occlude()
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.emit(SettingsEvent::Dismissed);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex_none()
                    .h(px(36.))
                    .px(PADDING)
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.line)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Settings"))
                    .child(Kbd::new("escape")),
            )
            .child(section("Appearance"))
            .child(row("Theme", theme).child(theme_picker))
            .child(row("Density", theme).child(density_picker))
            .child(section("Recording").rule())
            .child(
                row("Budget", theme)
                    .child(budget_control("loupe-settings-budget", settings.budget))
                    .child(budget),
            )
            .child(capture_levels(settings.capture_level, theme))
            .child(section("Source links").rule())
            .child(self.render_editor(&settings.editor, theme, cx))
            .child(
                div()
                    .flex_none()
                    .mt_1()
                    .px(PADDING)
                    .py(px(8.))
                    .border_t_1()
                    .border_color(colors.line)
                    .text_size(theme.metrics.text_small)
                    .text_color(colors.text_faint)
                    .child("Kept until the app quits. Apps set defaults with init_with."),
            )
    }
}

/// A labelled row: `Theme  [control]`.
fn row(label: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .min_h(theme.metrics.property_row + px(4.))
        .px(PADDING)
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_none()
                .w(LABEL_WIDTH)
                .text_color(theme.colors.text_muted)
                .child(label),
        )
}

/// A row aligned with the controls of labelled rows.
fn indented(theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .min_h(theme.metrics.property_row)
        .pl(PADDING + LABEL_WIDTH + px(8.))
        .pr(PADDING)
        .flex()
        .items_center()
}

/// A section header lined up with the rows.
fn section(label: &'static str) -> SectionHeader {
    SectionHeader::new(label).gutter(PADDING)
}

/// A segmented control over `options` that writes the chosen one with `set`.
fn choice<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, &'static str, &'static str)],
    current: T,
    set: fn(&mut LoupeSettings, T),
) -> Segmented {
    let values: Vec<T> = options.iter().map(|(value, _, _)| *value).collect();
    options
        .iter()
        .fold(Segmented::new(id), |control, (_, label, tooltip)| {
            control.segment(Segment::label(*label).tooltip(*tooltip))
        })
        .selected(
            values
                .iter()
                .position(|value| *value == current)
                .unwrap_or(0),
        )
        .on_select(move |ix, _, cx| {
            let value = values[*ix];
            LoupeSettings::update(cx, |settings| set(settings, value));
        })
}

/// The capture levels as a radio list, each with what it records and costs.
fn capture_levels(current: CaptureLevel, theme: &Theme) -> impl IntoElement {
    let colors = &theme.colors;
    let options = CAPTURE_LEVELS.map(|level| {
        let selected = level == current;
        let selector = format!(
            "loupe-settings-level-{}",
            capture_level_label(level).to_lowercase()
        );
        let radio = div()
            .flex_none()
            .mt(px(2.))
            .size(px(12.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border_1()
            .border_color(if selected {
                colors.accent
            } else {
                colors.line_strong
            })
            .when(selected, |this| {
                this.child(div().size(px(6.)).rounded_full().bg(colors.accent))
            });
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .px(px(6.))
            .py(px(4.))
            .flex()
            .items_start()
            .gap_2()
            .rounded(theme.metrics.radius)
            .cursor_pointer()
            .when(selected, |this| this.bg(colors.selected))
            .when(!selected, |this| this.hover(|style| style.bg(colors.hover)))
            .child(radio)
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(capture_level_label(level))
                    .child(
                        div()
                            .text_size(theme.metrics.text_small)
                            .text_color(colors.text_faint)
                            .child(capture_level_summary(level)),
                    ),
            )
            .on_click(move |_, _, cx| {
                LoupeSettings::update(cx, |settings| settings.capture_level = level);
            })
            .into_any_element()
    });
    div()
        .flex_none()
        .px(PADDING)
        .pb_1()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .flex_none()
                .w(LABEL_WIDTH)
                .pt(px(4.))
                .text_color(colors.text_muted)
                .child("Capture"),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .ml(px(-6.))
                .flex()
                .flex_col()
                .children(options),
        )
}
