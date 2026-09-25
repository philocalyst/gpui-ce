//! The help overlay: every key binding and what each lens answers, opened
//! with `?` or the palette.
//!
//! The cheat sheet is generated from the keymap, not written by hand: every
//! binding of one of Loupe's actions is listed under the key context it
//! applies in, described by the action's doc comment. Rebinding a key, or
//! adding an action, changes the sheet. Only the keys the engine handles
//! itself while picking are listed here by hand ([`PICKING`]).

use crate::{
    lenses::KEY_CONTEXTS,
    loupe::{LOUPE_CONTEXT, lens_action},
    state::Lens,
    theme::Theme,
    widgets::{Kbd, LIST_CONTEXT, SCRUB_CONTEXT, SectionHeader, floating_surface},
};
use gpui::{
    AnyElement, App, FontWeight, IntoElement, KeyBinding, KeyBindingContextPredicate, Pixels,
    RenderOnce, SharedString, Window, div, prelude::*, px,
};
use std::{collections::HashMap, hash::BuildHasher};

/// One line of the cheat sheet: the keys that run an action, and what it
/// does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KeyEntry {
    /// The action's name, e.g. `loupe::OpenPalette` (empty for keys the
    /// engine handles itself).
    pub action: &'static str,
    /// Every keystroke sequence bound to it, in gpui syntax.
    pub keys: Vec<SharedString>,
    /// What it does, from the action's doc comment.
    pub description: SharedString,
}

/// The keys that apply in one place.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KeyGroup {
    /// Where they apply: `Global`, `Loupe`, `Lists`…
    pub title: &'static str,
    /// In registration order.
    pub entries: Vec<KeyEntry>,
}

/// The whole sheet.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CheatSheet {
    /// Each lens with the keys that show it.
    pub lenses: Vec<(Lens, Vec<SharedString>)>,
    /// Every other key, by where it applies.
    pub groups: Vec<KeyGroup>,
}

/// Keys bound outside Loupe's own key contexts.
const GLOBAL: &str = "Global";
/// Keys anywhere in Loupe.
const LOUPE: &str = "Loupe";
/// Keys the engine handles while picking.
const PICKING: &str = "While picking";
/// Keys in focused trees and tables.
const LISTS: &str = "Lists and tables";
/// Keys in focused number fields.
const NUMBERS: &str = "Number fields";

/// The keys the engine handles itself while picking. They are not key
/// bindings, so the keymap cannot list them.
pub(crate) const PICKING_KEYS: [(&str, &str); 3] = [
    ("]", "Picks the enclosing element (or scroll up)"),
    ("[", "Picks back towards the pointer (or scroll down)"),
    ("escape", "Stops picking"),
];

/// Loupe's key contexts and the group each selects, most specific first.
fn context_groups() -> Vec<(&'static str, &'static str)> {
    KEY_CONTEXTS
        .iter()
        .map(|(lens, context)| (*context, lens.label()))
        .chain([
            (LIST_CONTEXT, LISTS),
            (SCRUB_CONTEXT, NUMBERS),
            (LOUPE_CONTEXT, LOUPE),
        ])
        .collect()
}

impl CheatSheet {
    /// The sheet for `bindings` (e.g. the app's whole keymap), describing
    /// each action by its entry in `docs` (the action registry's doc
    /// comments). Bindings of other actions than Loupe's are ignored.
    pub fn new<'a>(
        bindings: impl IntoIterator<Item = &'a KeyBinding>,
        docs: &HashMap<&'static str, &'static str, impl BuildHasher>,
    ) -> Self {
        let contexts = context_groups();
        let lens_groups = KEY_CONTEXTS.iter().map(|(lens, _)| lens.label());
        let mut groups: Vec<KeyGroup> = [GLOBAL, LOUPE, PICKING, LISTS, NUMBERS]
            .into_iter()
            .chain(lens_groups)
            .map(|title| KeyGroup {
                title,
                entries: Vec::new(),
            })
            .collect();
        let lens_actions = Lens::ALL.map(|lens| (lens, lens_action(lens)));
        let mut lenses: Vec<(Lens, Vec<SharedString>)> =
            Lens::ALL.iter().map(|lens| (*lens, Vec::new())).collect();

        for binding in bindings {
            let action = binding.action().name();
            if !is_loupe_action(action) {
                continue;
            }
            let keys: SharedString = binding
                .keystrokes()
                .iter()
                .map(|keystroke| keystroke.unparse())
                .collect::<Vec<_>>()
                .join(" ")
                .into();
            if let Some((lens, _)) = lens_actions.iter().find(|(_, name)| *name == action) {
                let (_, lens_keys) = &mut lenses[lens.index()];
                push_unique(lens_keys, keys);
                continue;
            }
            let title = group_title(binding.predicate().as_deref(), &contexts);
            let Some(group) = groups.iter_mut().find(|group| group.title == title) else {
                continue;
            };
            match group
                .entries
                .iter_mut()
                .find(|entry| entry.action == action)
            {
                Some(entry) => push_unique(&mut entry.keys, keys),
                None => group.entries.push(KeyEntry {
                    action,
                    keys: vec![keys],
                    description: describe(action, docs),
                }),
            }
        }

        if let Some(picking) = groups.iter_mut().find(|group| group.title == PICKING) {
            picking.entries = PICKING_KEYS
                .iter()
                .map(|(keys, description)| KeyEntry {
                    action: "",
                    keys: vec![SharedString::new_static(keys)],
                    description: SharedString::new_static(description),
                })
                .collect();
        }
        groups.retain(|group| !group.entries.is_empty());
        Self { lenses, groups }
    }

    /// The sheet for the app's current keymap.
    pub fn of(cx: &App) -> Self {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        Self::new(keymap.bindings(), cx.action_documentation())
    }

    /// Lays the groups out in at most `columns` columns, in order, keeping
    /// the tallest column as short as it can be (a header counts as a row).
    /// Returns how many groups each column holds.
    pub fn columns(&self, columns: usize) -> Vec<usize> {
        let heights: Vec<usize> = self
            .groups
            .iter()
            .map(|group| group.entries.len() + 1)
            .collect();
        // Fills columns up to `limit` rows each, left to right.
        let pack = |limit: usize| {
            let mut runs = vec![0];
            let mut height = 0;
            for &group in &heights {
                if height + group > limit && height > 0 {
                    runs.push(0);
                    height = 0;
                }
                if let Some(run) = runs.last_mut() {
                    *run += 1;
                }
                height += group;
            }
            runs
        };
        let tallest = heights.iter().copied().max().unwrap_or(0);
        let total = heights.iter().sum();
        (tallest..=total)
            .map(pack)
            .find(|runs| runs.len() <= columns.max(1))
            .unwrap_or_else(|| vec![heights.len()])
    }

    /// Every entry, in every group.
    #[cfg(test)]
    pub fn entries(&self) -> impl Iterator<Item = &KeyEntry> {
        self.groups.iter().flat_map(|group| &group.entries)
    }
}

/// Whether `action` is one of Loupe's (`loupe::…`, `loupe_list::…`).
fn is_loupe_action(action: &str) -> bool {
    action
        .split("::")
        .next()
        .is_some_and(|namespace| namespace == "loupe" || namespace.starts_with("loupe_"))
}

fn push_unique(keys: &mut Vec<SharedString>, new: SharedString) {
    if !keys.contains(&new) {
        keys.push(new);
    }
}

/// The group of a binding: the most specific of Loupe's key contexts its
/// predicate requires, or `Global`.
fn group_title(
    predicate: Option<&KeyBindingContextPredicate>,
    contexts: &[(&'static str, &'static str)],
) -> &'static str {
    let mut required = Vec::new();
    if let Some(predicate) = predicate {
        required_contexts(predicate, &mut required);
    }
    contexts
        .iter()
        .find(|(context, _)| required.contains(context))
        .map_or(GLOBAL, |(_, title)| *title)
}

/// The identifiers a predicate requires (not the ones it excludes).
fn required_contexts<'a>(predicate: &'a KeyBindingContextPredicate, out: &mut Vec<&'a str>) {
    match predicate {
        KeyBindingContextPredicate::Identifier(name) => out.push(name),
        KeyBindingContextPredicate::And(left, right)
        | KeyBindingContextPredicate::Or(left, right)
        | KeyBindingContextPredicate::Descendant(left, right) => {
            required_contexts(left, out);
            required_contexts(right, out);
        }
        KeyBindingContextPredicate::Not(_)
        | KeyBindingContextPredicate::Equal(..)
        | KeyBindingContextPredicate::NotEqual(..) => {}
    }
}

/// An action's doc comment without its final period, or its name.
fn describe(
    action: &'static str,
    docs: &HashMap<&'static str, &'static str, impl BuildHasher>,
) -> SharedString {
    match docs.get(action) {
        Some(doc) => {
            let first_line = doc.lines().next().unwrap_or_default().trim();
            SharedString::from(first_line.trim_end_matches('.').to_string())
        }
        None => SharedString::new_static(action.rsplit("::").next().unwrap_or(action)),
    }
}

/// Each column of groups needs this much width: panels this wide or wider
/// list the groups side by side, up to [`MAX_COLUMNS`].
const COLUMN_WIDTH: Pixels = px(340.);
/// The most columns of groups.
const MAX_COLUMNS: usize = 3;
/// The widest the panel gets: three columns.
pub(crate) const MAX_WIDTH: Pixels = px(1080.);

/// The overlay itself.
#[derive(IntoElement)]
pub(crate) struct HelpOverlay {
    /// What to list.
    pub sheet: std::rc::Rc<CheatSheet>,
    /// The panel's width.
    pub width: Pixels,
}

impl RenderOnce for HelpOverlay {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let columns = ((self.width / COLUMN_WIDTH).floor() as usize).clamp(1, MAX_COLUMNS);

        let lenses = self.sheet.lenses.iter().map(|(lens, keys)| {
            div()
                .min_h(theme.metrics.row)
                .px(px(12.))
                .flex()
                .items_center()
                .gap_2()
                .child(keys_cell(keys, px(44.)))
                .child(
                    div()
                        .flex_none()
                        .w(px(64.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(lens.label()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(colors.text_muted)
                        .child(lens.question()),
                )
                .into_any_element()
        });

        let group = |group: &KeyGroup| {
            div()
                .flex()
                .flex_col()
                .pb_1()
                .child(SectionHeader::new(group.title).gutter(px(12.)))
                .children(group.entries.iter().map(|entry| {
                    div()
                        .min_h(theme.metrics.row)
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(colors.text_muted)
                                .child(entry.description.clone()),
                        )
                        .child(keys_cell(&entry.keys, px(0.)))
                }))
                .into_any_element()
        };
        let mut groups = self.sheet.groups.iter().map(group);
        let columns = self
            .sheet
            .columns(columns)
            .into_iter()
            .enumerate()
            .map(|(ix, count)| {
                div()
                    .flex_1()
                    .min_w_0()
                    .when(ix > 0, |this| this.border_l_1().border_color(colors.line))
                    .children(groups.by_ref().take(count).collect::<Vec<AnyElement>>())
            });
        let columns = div().flex().items_start().children(columns);

        floating_surface(theme)
            .id("loupe-help")
            .debug_selector(|| "loupe-help".into())
            .w(self.width)
            .max_h_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .text_size(theme.metrics.text)
            .text_color(colors.text)
            .occlude()
            .child(
                div()
                    .flex_none()
                    .h(px(36.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.line)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Keyboard shortcuts"),
                    )
                    .child(Kbd::new("escape")),
            )
            .child(SectionHeader::new("Lenses").gutter(px(12.)))
            .children(lenses)
            .child(
                div()
                    .mt_1()
                    .border_t_1()
                    .border_color(colors.line)
                    .child(columns),
            )
    }
}

/// Key caps for `keys`, right-aligned in at least `min_width`.
fn keys_cell(keys: &[SharedString], min_width: Pixels) -> impl IntoElement {
    div()
        .flex_none()
        .min_w(min_width)
        .flex()
        .items_center()
        .gap_1()
        .children(keys.iter().cloned().map(Kbd::new))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, commands::keys, key_bindings};
    use gpui::Keystroke;

    gpui::actions!(
        loupe_test,
        [
            /// Tests a documented action.
            Documented,
            Undocumented,
        ]
    );

    fn docs() -> HashMap<&'static str, &'static str> {
        gpui::generate_list_of_all_registered_actions()
            .filter_map(|action| Some((action.name, action.documentation?)))
            .collect()
    }

    fn loupe_sheet() -> CheatSheet {
        CheatSheet::new(&key_bindings(), &docs())
    }

    /// Keys in gpui syntax, normalized (`secondary-k` is `ctrl-k` here).
    fn normalized(keys: &str) -> String {
        keys.split_whitespace()
            .map(|key| Keystroke::parse(key).expect("valid keystroke").unparse())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn strings(keys: &[SharedString]) -> Vec<&str> {
        keys.iter().map(SharedString::as_ref).collect()
    }

    fn group<'a>(sheet: &'a CheatSheet, title: &str) -> &'a KeyGroup {
        sheet
            .groups
            .iter()
            .find(|group| group.title == title)
            .unwrap_or_else(|| panic!("no {title} group"))
    }

    #[test]
    fn every_command_with_a_key_is_on_the_sheet() {
        let sheet = loupe_sheet();
        let listed: Vec<String> = sheet
            .entries()
            .flat_map(|entry| &entry.keys)
            .chain(sheet.lenses.iter().flat_map(|(_, keys)| keys))
            .map(|keys| normalized(keys))
            .collect();
        let mut commands = Command::all();
        commands.extend([Command::OpenPalette, Command::Close]);
        for command in commands {
            if let Some(keys) = command.keys() {
                assert!(
                    listed.contains(&normalized(keys)),
                    "{command:?} ({keys}) is missing from {listed:?}"
                );
            }
        }
    }

    #[test]
    fn every_loupe_binding_is_listed_once_under_its_context() {
        let sheet = loupe_sheet();
        let titles: Vec<_> = sheet.groups.iter().map(|group| group.title).collect();
        assert_eq!(
            titles,
            [GLOBAL, LOUPE, PICKING, LISTS, NUMBERS, "Frames", "Audit"]
        );
        let bindings = key_bindings()
            .into_iter()
            .filter(|binding| is_loupe_action(binding.action().name()))
            .count();
        let listed = sheet
            .entries()
            .filter(|entry| !entry.action.is_empty())
            .map(|entry| entry.keys.len())
            .chain(sheet.lenses.iter().map(|(_, keys)| keys.len()))
            .sum::<usize>();
        assert_eq!(listed, bindings, "every binding, and no text-field keys");

        let global: Vec<_> = group(&sheet, GLOBAL)
            .entries
            .iter()
            .map(|entry| entry.action)
            .collect();
        assert_eq!(
            global,
            [
                "loupe::ToggleInspector",
                "loupe::TogglePick",
                "loupe::ToggleHold"
            ]
        );
        let next = group(&sheet, LISTS)
            .entries
            .iter()
            .find(|entry| entry.action == "loupe_list::SelectNext")
            .expect("list navigation is listed");
        assert_eq!(
            strings(&next.keys),
            ["down", "j"],
            "keys of one action merge"
        );
        assert_eq!(next.description.as_ref(), "Selects the next row");
        let freeze = group(&sheet, LOUPE)
            .entries
            .iter()
            .find(|entry| strings(&entry.keys) == [keys::FREEZE])
            .expect("freeze is listed");
        assert_eq!(freeze.description.as_ref(), "Freezes or resumes recording");
    }

    #[test]
    fn lenses_list_the_keys_that_show_them() {
        let sheet = loupe_sheet();
        for (lens, lens_keys) in &sheet.lenses {
            assert_eq!(strings(lens_keys), [keys::LENSES[lens.index()]], "{lens:?}");
        }
        assert!(
            sheet
                .entries()
                .all(|entry| !entry.action.starts_with("loupe::Show")),
            "lens keys are not repeated in the Loupe group"
        );
    }

    #[test]
    fn columns_split_the_rows_evenly() {
        let group = |title, entries: usize| KeyGroup {
            title,
            entries: vec![
                KeyEntry {
                    action: "",
                    keys: Vec::new(),
                    description: SharedString::default(),
                };
                entries
            ],
        };
        let sheet = |sizes: &[usize]| CheatSheet {
            lenses: Vec::new(),
            groups: sizes.iter().map(|size| group("", *size)).collect(),
        };
        // Loupe's own groups: Global, Loupe, picking | lists, numbers | …
        let loupe = sheet(&[3, 5, 3, 7, 4, 7, 3]);
        assert_eq!(loupe.columns(1), [7]);
        assert_eq!(loupe.columns(2), [4, 3]);
        assert_eq!(loupe.columns(3), [3, 2, 2]);
        assert_eq!(sheet(&[10, 1, 1, 1]).columns(2), [1, 3]);
        assert_eq!(sheet(&[1, 1, 1, 10]).columns(2), [3, 1]);
        assert_eq!(sheet(&[2]).columns(3), [1], "never empty columns");
        assert_eq!(sheet(&[]).columns(2), [0]);
    }

    #[test]
    fn other_contexts_are_global_and_undocumented_actions_use_their_name() {
        let bindings = [
            KeyBinding::new("x", Documented, Some("LoupeList && !EditableText")),
            KeyBinding::new("y", Undocumented, Some("Workspace")),
            KeyBinding::new("secondary-y", Undocumented, Some("Workspace")),
            KeyBinding::new(
                "z",
                gpui_elements::editable_text::actions::Enter,
                Some("Loupe > EditableText"),
            ),
        ];
        let sheet = CheatSheet::new(&bindings, &docs());
        let titles: Vec<_> = sheet.groups.iter().map(|group| group.title).collect();
        assert_eq!(titles, [GLOBAL, PICKING, LISTS], "only groups with keys");

        let documented = &group(&sheet, LISTS).entries[0];
        assert_eq!(documented.description.as_ref(), "Tests a documented action");
        let undocumented = &group(&sheet, GLOBAL).entries[0];
        assert_eq!(undocumented.description.as_ref(), "Undocumented");
        assert_eq!(
            strings(&undocumented.keys),
            ["y", normalized("secondary-y").as_str()]
        );
        assert!(
            sheet
                .entries()
                .all(|entry| !entry.action.starts_with("editable_text")),
            "text editing keys are not Loupe's"
        );
    }
}
