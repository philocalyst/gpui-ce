//! The widget kit, rendered headless in a gallery and driven with real input.
//! Screenshots land in `target/loupe-shots/widgets-*.png`.

use gpui::{
    AnyElement, AppContext as _, Axis, Context, Entity, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Styled as _, Window, div, point, px, rgb, rgb_to_hsla, rgba, size,
};
use gpui_elements::editable_text::EditableTextState;
use gpui_elements::editable_text::actions::{DEFAULT_INPUT_CONTEXT, default_bindings};
use gpui_inspector::{
    harness::LoupeHarness,
    theme::{Appearance, MONO_FONT, Theme, UI_FONT},
    widgets::{
        Button, ButtonSize, ButtonStyle, ColorSwatch, Column, ColumnWidth, EmptyState, Icon,
        IconName, Kbd, Pill, RailTab, ScrubChanged, ScrubField, SectionHeader, Segment, Segmented,
        Sparkline, Split, TabRail, Table, TableState, TextField, Tone, Tree, TreeRow, TreeState,
        flatten, text_field_state,
    },
};
use std::time::Duration;

/// Children of each node in the gallery's tree.
fn children(name: &&'static str) -> Vec<&'static str> {
    match *name {
        "InboxApp" => vec!["div#root"],
        "div#root" => vec!["Sidebar", "IssueList", "IssueDetail"],
        "Sidebar" => vec!["div#nav", "div#labels"],
        "IssueList" => vec!["div#toolbar", "uniform_list#rows"],
        "uniform_list#rows" => vec!["IssueRow", "div#row-1", "div#row-2", "div#row-3"],
        "IssueRow" => vec!["Avatar", "text"],
        "IssueDetail" => vec!["div#header", "div#thread"],
        _ => vec![],
    }
}

fn glyph(name: &str) -> IconName {
    match name {
        "InboxApp" | "Sidebar" | "IssueList" | "IssueDetail" => IconName::View,
        "IssueRow" | "Avatar" => IconName::Component,
        _ => IconName::ElementDot,
    }
}

const ENTITIES: [(&str, &str, u32); 8] = [
    ("IssueStore", "model", 412),
    ("IssueList", "view", 188),
    ("SyncClient", "model", 64),
    ("SearchField", "view", 21),
    ("Sidebar", "view", 6),
    ("Settings", "model", 3),
    ("InboxApp", "view", 2),
    ("Theme", "model", 0),
];

struct Gallery {
    tree: Entity<TreeState<&'static str>>,
    table: Entity<TableState<usize>>,
    scrub: Entity<ScrubField>,
    filter: Entity<EditableTextState>,
    split: Pixels,
    rail: usize,
    segment: usize,
    scrubbed: Option<f32>,
}

impl Gallery {
    fn new(cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| {
            let mut tree = TreeState::new(cx);
            tree.set_nodes(flatten(["InboxApp"], children, |name| *name), cx);
            tree.model_mut().expand_to_depth(3);
            tree
        });
        let table = cx.new(|cx| {
            let mut table = TableState::new(cx);
            table.set_rows(
                (0..ENTITIES.len()).collect(),
                |column, a, b| match column {
                    0 => ENTITIES[a].0.cmp(ENTITIES[b].0),
                    1 => ENTITIES[a].1.cmp(ENTITIES[b].1),
                    _ => ENTITIES[a].2.cmp(&ENTITIES[b].2),
                },
                cx,
            );
            table
        });
        let scrub = cx.new(|cx| ScrubField::new(12., 1., cx).with_unit("px"));
        cx.subscribe(&scrub, |this, _, ScrubChanged(value), cx| {
            this.scrubbed = Some(*value);
            cx.notify();
        })
        .detach();
        Self {
            tree,
            table,
            scrub,
            filter: text_field_state(cx),
            split: px(210.),
            rail: 0,
            segment: 0,
            scrubbed: None,
        }
    }

    fn section(title: &'static str, content: impl IntoElement) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .child(SectionHeader::new(title))
            .child(div().px_2().pb_2().child(content))
    }
}

fn tree_row(row: &TreeRow<&'static str>, window: &mut Window, cx: &mut gpui::App) -> AnyElement {
    let theme = Theme::of(window, cx);
    let icon = glyph(row.key);
    let color = match icon {
        IconName::View => theme.colors.view,
        IconName::Component => theme.colors.component,
        _ => theme.colors.text_faint,
    };
    let (name, id) = match row.key.split_once('#') {
        Some((name, id)) => (name, Some(id)),
        None => (row.key, None),
    };
    div()
        .size_full()
        .flex()
        .items_center()
        .gap_1p5()
        .child(Icon::new(icon).size(px(10.)).color(color))
        .child(div().child(SharedString::new_static(name)))
        .children(id.map(|id| {
            div()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(theme.colors.text_muted)
                .child(format!("#{id}"))
        }))
        .child(div().flex_1())
        .child(
            div()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(theme.colors.text_faint)
                .child(format!("{}×{}", 40 + row.node * 13, 24 + row.depth * 8)),
        )
        .into_any_element()
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let buttons = div()
            .flex()
            .items_center()
            .gap_1()
            .child(Button::new("pick").icon(IconName::Pick).label("Pick"))
            .child(
                Button::new("outlines")
                    .icon(IconName::Outline)
                    .toggle_state(true)
                    .tooltip("Outline elements"),
            )
            .child(
                Button::new("flash")
                    .icon(IconName::Flash)
                    .tooltip("Flash repaints"),
            )
            .child(
                Button::new("freeze")
                    .icon(IconName::Pause)
                    .label("Freeze")
                    .style(ButtonStyle::Subtle),
            )
            .child(
                Button::new("copy")
                    .icon(IconName::Copy)
                    .label("Copy Rust")
                    .size(ButtonSize::Small),
            )
            .child(Button::new("revert").label("Revert").disabled(true))
            .child(
                Button::new("delete")
                    .icon(IconName::Close)
                    .color(colors.crit)
                    .tooltip("Remove override"),
            );
        let segmented = Segmented::new("scope")
            .segment(Segment::label("All"))
            .segment(Segment::label("Views"))
            .segment(Segment::icon(IconName::Filter).tooltip("Filtered"))
            .selected(self.segment)
            .on_select(cx.listener(|this, ix, _, cx| {
                this.segment = *ix;
                cx.notify();
            }));
        let pills = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(Pill::new("Frozen").tone(Tone::Accent).strong())
            .child(Pill::new("12 views"))
            .child(Pill::new("ok").tone(Tone::Ok))
            .child(Pill::new("1.4× budget").tone(Tone::Warn))
            .child(Pill::new("crit").tone(Tone::Crit))
            .child(Kbd::new("secondary-k"))
            .child(Kbd::new("alt-1"))
            .child(Kbd::new("escape"));
        let icons = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(IconName::ALL.map(|icon| Icon::new(icon).color(colors.text)));
        let fields = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div().w(px(200.)).child(
                    TextField::new("filter", &self.filter)
                        .icon(IconName::Filter)
                        .placeholder("Filter elements"),
                ),
            )
            .child(div().w(px(72.)).child(self.scrub.clone()))
            .child(ColorSwatch::new(rgb_to_hsla(rgb(0x2da44e))))
            .child(ColorSwatch::new(rgb_to_hsla(rgba(0x0969da80))))
            .child(ColorSwatch::new(colors.warn))
            .children(self.scrubbed.map(|value| {
                div()
                    .text_color(colors.text_muted)
                    .child(format!("scrubbed to {value}"))
            }));
        let sparkline = div()
            .flex()
            .items_center()
            .gap_3()
            .child(Sparkline::new(
                [
                    2., 3., 2., 5., 9., 4., 3., 3., 12., 6., 4., 3., 2., 2., 7., 4.,
                ]
                .as_slice(),
            ))
            .child(
                Sparkline::new([1., 1., 2., 1., 1., 1., 6., 1., 1., 2., 1., 1.].as_slice())
                    .color(colors.warn)
                    .size(px(96.), px(16.)),
            );

        let left = div()
            .w(px(460.))
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors.line)
            .child(Self::section("Buttons", buttons))
            .child(Self::section("Segmented", div().flex().child(segmented)))
            .child(Self::section("Pills and keys", pills))
            .child(Self::section("Icons", icons))
            .child(Self::section("Fields", fields))
            .child(Self::section("Sparklines", sparkline))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(colors.line)
                    .child(
                        EmptyState::new("No element selected")
                            .icon(IconName::Pick)
                            .description("Pick an element in the app, or choose one in the tree.")
                            .action(
                                Button::new("start-pick")
                                    .icon(IconName::Pick)
                                    .label("Start picking"),
                            ),
                    ),
            );

        let table = Table::new(
            "entities",
            &self.table,
            vec![
                Column::new("Entity"),
                Column::new("Kind").width(ColumnWidth::Fixed(px(64.))),
                Column::new("Notifies")
                    .numeric()
                    .width(ColumnWidth::Fixed(px(80.))),
            ],
            |row, column, window, cx| {
                let theme = Theme::of(window, cx);
                let (name, kind, notifies) = ENTITIES[row];
                match column {
                    0 => div().truncate().child(name).into_any_element(),
                    1 => div()
                        .text_color(theme.colors.text_muted)
                        .child(kind)
                        .into_any_element(),
                    _ => div()
                        .font_family(MONO_FONT)
                        .text_size(theme.metrics.mono)
                        .child(notifies.to_string())
                        .into_any_element(),
                }
            },
        );
        let right = div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .child(
                TabRail::new("rail")
                    .tab(RailTab::new("Elements").count("1,284"))
                    .tab(
                        RailTab::new("Frames")
                            .count("7")
                            .tone(Tone::Crit)
                            .marker(IconName::TriangleUp),
                    )
                    .tab(RailTab::new("Events").count("42"))
                    .tab(RailTab::new("Audit").count("5").tone(Tone::Warn))
                    .selected(self.rail)
                    .on_select(cx.listener(|this, ix, _, cx| {
                        this.rail = *ix;
                        cx.notify();
                    })),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Split::new("gallery-split", Axis::Vertical, self.split)
                        .min_sizes(px(80.), px(120.))
                        .first(Tree::new("tree", &self.tree, tree_row))
                        .second(table)
                        .on_resize(cx.listener(|this, size, _, cx| {
                            this.split = *size;
                            cx.notify();
                        })),
                ),
            );

        div()
            .size_full()
            .flex()
            .bg(colors.bg)
            .font_family(UI_FONT)
            .text_size(theme.metrics.text)
            .line_height(theme.metrics.line_height)
            .text_color(colors.text)
            .child(left)
            .child(right)
    }
}

fn gallery(appearance: Appearance) -> (LoupeHarness, Entity<Gallery>) {
    let mut gallery = None;
    let mut harness = LoupeHarness::new(size(px(1000.), px(680.)), |_, cx| {
        let view = cx.new(Gallery::new);
        gallery = Some(view.clone());
        view
    });
    // Outside Loupe, apps bind the editable text keys themselves.
    harness.app(|cx| cx.bind_keys(default_bindings().as_keybindings(Some(DEFAULT_INPUT_CONTEXT))));
    harness.set_appearance(appearance);
    (harness, gallery.unwrap())
}

#[test]
fn the_gallery_renders_every_widget_in_both_themes() {
    for (appearance, name) in [
        (Appearance::Dark, "widgets-dark"),
        (Appearance::Light, "widgets-light"),
    ] {
        let (mut harness, _) = gallery(appearance);
        for text in [
            "BUTTONS",
            "Freeze",
            "FROZEN",
            "Ctrl+K",
            "IssueList",
            "Notifies",
            "12",
        ] {
            harness.assert_text_visible(text);
        }
        harness.screenshot(name);
    }
}

#[test]
fn trees_navigate_with_the_keyboard_and_keep_selection_by_key() {
    let (mut harness, gallery) = gallery(Appearance::Dark);
    let tree = harness.app(|cx| gallery.read(cx).tree.clone());
    let selected =
        |harness: &mut LoupeHarness| harness.app(|cx| tree.read(cx).model().selected().copied());
    harness.click_text("Sidebar");
    assert_eq!(selected(&mut harness), Some("Sidebar"));
    harness.type_keys("j j");
    assert_eq!(selected(&mut harness), Some("div#labels"));
    harness.type_keys("k k k");
    assert_eq!(selected(&mut harness), Some("div#root"));
    harness.type_keys("down right");
    assert_eq!(
        selected(&mut harness),
        Some("div#nav"),
        "Sidebar was already expanded"
    );
    harness.type_keys("left left");
    assert_eq!(selected(&mut harness), Some("Sidebar"));
    assert!(!harness.app(|cx| tree.read(cx).model().is_expanded(&"Sidebar")));
    harness.screenshot("widgets-tree-keyboard");

    // A new snapshot with an extra root keeps the selection on "Sidebar".
    harness.app(|cx| {
        tree.update(cx, |tree, cx| {
            tree.set_nodes(flatten(["Tooltip", "InboxApp"], children, |name| *name), cx)
        })
    });
    harness.draw();
    assert_eq!(selected(&mut harness), Some("Sidebar"));
}

#[test]
fn table_headers_sort_and_rows_select() {
    let (mut harness, gallery) = gallery(Appearance::Dark);
    let table = harness.app(|cx| gallery.read(cx).table.clone());
    let first_row =
        |harness: &mut LoupeHarness| harness.app(|cx| table.read(cx).model().key_at(0).copied());
    assert_eq!(first_row(&mut harness), Some(0));
    harness.click_text("Notifies");
    assert_eq!(
        first_row(&mut harness),
        Some(0),
        "largest first: IssueStore"
    );
    harness.click_text("Notifies");
    assert_eq!(
        first_row(&mut harness),
        Some(7),
        "then smallest first: Theme"
    );
    harness.click_text("Entity");
    assert_eq!(
        harness.app(|cx| table.read(cx).model().key_at(0).copied()),
        Some(6),
        "InboxApp sorts first by name"
    );
    harness.click_text("SyncClient");
    harness.type_keys("j");
    assert_eq!(
        harness.app(|cx| table.read(cx).model().selected().copied()),
        Some(7),
        "Theme follows SyncClient"
    );
    harness.screenshot("widgets-table-sorted");
}

#[test]
fn scrub_fields_take_typed_values_and_arrow_nudges() {
    let (mut harness, gallery) = gallery(Appearance::Dark);
    let scrub = harness.app(|cx| gallery.read(cx).scrub.clone());
    let value = |harness: &mut LoupeHarness| harness.app(|cx| scrub.read(cx).value());

    harness.click_text("12");
    assert!(harness.app(|cx| scrub.read(cx).is_editing()));
    harness.type_keys("secondary-a");
    harness.type_text("42");
    harness.type_keys("enter");
    assert_eq!(value(&mut harness), 42.);
    assert!(!harness.app(|cx| scrub.read(cx).is_editing()));
    harness.type_keys("up up shift-up");
    assert_eq!(value(&mut harness), 54.);
    harness.type_keys("down");
    assert_eq!(value(&mut harness), 53.);
    assert_eq!(harness.app(|cx| gallery.read(cx).scrubbed), Some(53.));

    // Dragging sideways scrubs one step per two pixels.
    let field = harness.find_text("53").unwrap();
    harness.drag(
        field.center(),
        point(field.center().x + px(20.), field.center().y),
        4,
    );
    assert_eq!(value(&mut harness), 63.);
    harness.screenshot("widgets-scrub");
}

#[test]
fn text_fields_accept_typing_and_show_placeholders() {
    let (mut harness, gallery) = gallery(Appearance::Light);
    harness.assert_text_visible("Filter elements");
    harness.click_text("Filter elements");
    harness.type_text("row 3");
    let filter = harness.app(|cx| gallery.read(cx).filter.read(cx).as_str().to_string());
    assert_eq!(filter, "row 3");
    harness.assert_text_visible("row 3");
    harness.screenshot("widgets-text-field");
}

#[test]
fn tooltips_appear_on_hover() {
    let (mut harness, _) = gallery(Appearance::Dark);
    let flash = harness.bounds_of("flash");
    harness.hover(flash.center());
    harness.advance(Duration::from_secs(1));
    harness.assert_text_visible("Flash repaints");
    harness.screenshot("widgets-tooltip");
}

#[test]
fn splitters_resize_within_their_minimums() {
    let (mut harness, gallery) = gallery(Appearance::Dark);
    let handle = harness.bounds_of("gallery-split-handle").center();
    harness.drag(handle, point(handle.x, handle.y + px(60.)), 4);
    let split = harness.app(|cx| gallery.read(cx).split);
    assert!((split - px(270.)).abs() <= px(1.), "{split:?}");
    harness.drag(
        point(handle.x, handle.y + px(60.)),
        point(handle.x, px(0.)),
        4,
    );
    assert_eq!(harness.app(|cx| gallery.read(cx).split), px(80.));
    harness.screenshot("widgets-split");
}

#[test]
fn rail_tabs_and_segments_select_on_click() {
    let (mut harness, gallery) = gallery(Appearance::Dark);
    harness.click_text("Audit");
    assert_eq!(harness.app(|cx| gallery.read(cx).rail), 3);
    harness.click_text("Views");
    assert_eq!(harness.app(|cx| gallery.read(cx).segment), 1);
}
