//! Linework Inbox: a small issue tracker that exercises everything Loupe
//! explains. Views observe models, the detail pane is a cached view, the list
//! is virtualized, keys resolve through nested contexts (including a
//! multi-stroke binding), a sync client notifies on a timer, and a few
//! deliberate problems are left in for Loupe to find: a jank toggle, a
//! clickable that keyboards cannot reach, a faint footnote and an overflowing
//! label.
//!
//! Shared by the `loupe_demo` example and Loupe's end-to-end tests.

use gpui::{
    Animation, AnimationExt as _, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    Hsla, InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render,
    ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    UniformListScrollHandle, Window, actions, div, prelude::FluentBuilder as _, px, rgb,
    rgb_to_hsla, uniform_list,
};
use gpui_elements::editable_text::{
    EditableTextState, StringStorage, TextChanged, actions::default_bindings, text_input,
};
use std::time::Duration;

actions!(
    inbox,
    [
        /// Moves the selection down the list.
        SelectNext,
        /// Moves the selection up the list.
        SelectPrevious,
        /// Closes the selected issue.
        CloseIssue,
        /// Shows the inbox (`g i`).
        GoToInbox,
        /// Shows starred issues (`g s`).
        GoToStarred,
        /// Focuses the search field.
        FocusSearch,
        /// Makes the issue list slow to render, to have something to profile.
        ToggleJank,
    ]
);

/// The font the demo renders with (registered by `gpui_inspector::init`).
const FONT: &str = "IBM Plex Sans";

/// Binds the demo's keys.
pub fn init(cx: &mut App) {
    let close = if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    };
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, Some("IssueList")),
        KeyBinding::new("down", SelectNext, Some("IssueList")),
        KeyBinding::new("k", SelectPrevious, Some("IssueList")),
        KeyBinding::new("up", SelectPrevious, Some("IssueList")),
        KeyBinding::new(close, CloseIssue, Some("Inbox")),
        KeyBinding::new("g i", GoToInbox, Some("Inbox && !EditableText")),
        KeyBinding::new("g s", GoToStarred, Some("Inbox && !EditableText")),
        KeyBinding::new("/", FocusSearch, Some("Inbox && !EditableText")),
        KeyBinding::new("ctrl-shift-j", ToggleJank, Some("Inbox")),
    ]);
    cx.bind_keys(default_bindings().as_keybindings(Some("Inbox > EditableText")));
}

/// How the demo behaves; tests turn the live parts off to stay deterministic.
#[derive(Clone, Copy, Debug)]
pub struct InboxOptions {
    /// Run the sync client's timer (it notifies ten times a second).
    pub live_sync: bool,
}

impl Default for InboxOptions {
    fn default() -> Self {
        Self { live_sync: true }
    }
}

// Models.

/// A folder in the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    /// Open issues.
    Inbox,
    /// Starred issues.
    Starred,
    /// Closed issues.
    Done,
}

impl Folder {
    const ALL: [Folder; 3] = [Folder::Inbox, Folder::Starred, Folder::Done];

    fn label(self) -> &'static str {
        match self {
            Folder::Inbox => "Inbox",
            Folder::Starred => "Starred",
            Folder::Done => "Done",
        }
    }

    fn contains(self, issue: &Issue) -> bool {
        match self {
            Folder::Inbox => issue.open,
            Folder::Starred => issue.starred,
            Folder::Done => !issue.open,
        }
    }
}

/// One issue.
#[derive(Clone, Debug)]
pub struct Issue {
    /// Issue number.
    pub number: u32,
    /// Title.
    pub title: SharedString,
    /// Who opened it.
    pub author: SharedString,
    /// Labels.
    pub labels: Vec<Label>,
    /// Body text.
    pub body: SharedString,
    /// Minutes since the last update.
    pub updated: u32,
    /// Open or closed.
    pub open: bool,
    /// Starred by you.
    pub starred: bool,
}

/// A colored issue label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// Something is broken.
    Bug,
    /// It is slow.
    Performance,
    /// Layout and styling.
    Layout,
    /// Accessibility.
    A11y,
    /// A new capability.
    Feature,
}

impl Label {
    fn name(self) -> &'static str {
        match self {
            Label::Bug => "bug",
            Label::Performance => "performance",
            Label::Layout => "layout",
            Label::A11y => "accessibility",
            Label::Feature => "feature",
        }
    }

    fn colors(self) -> (Hsla, Hsla) {
        let (fg, bg) = match self {
            Label::Bug => (0xb42318, 0xfef3f2),
            Label::Performance => (0xb54708, 0xfffaeb),
            Label::Layout => (0x3538cd, 0xeef4ff),
            Label::A11y => (0x067647, 0xecfdf3),
            Label::Feature => (0x6941c6, 0xf4f3ff),
        };
        (rgb_to_hsla(rgb(fg)), rgb_to_hsla(rgb(bg)))
    }
}

/// The issues, what is shown and what is selected.
pub struct IssueStore {
    issues: Vec<Issue>,
    folder: Folder,
    query: String,
    selected: Option<u32>,
    jank: bool,
}

impl IssueStore {
    fn new() -> Self {
        let issues = sample_issues();
        let selected = issues.first().map(|issue| issue.number);
        Self {
            issues,
            folder: Folder::Inbox,
            query: String::new(),
            selected,
            jank: false,
        }
    }

    /// The issues in the current folder that match the search.
    pub fn visible(&self) -> Vec<&Issue> {
        let query = self.query.to_lowercase();
        self.issues
            .iter()
            .filter(|issue| self.folder.contains(issue))
            .filter(|issue| query.is_empty() || issue.title.to_lowercase().contains(&query))
            .collect()
    }

    fn count(&self, folder: Folder) -> usize {
        self.issues
            .iter()
            .filter(|issue| folder.contains(issue))
            .count()
    }

    /// The selected issue.
    pub fn selected(&self) -> Option<&Issue> {
        let number = self.selected?;
        self.issues.iter().find(|issue| issue.number == number)
    }

    fn select(&mut self, number: u32, cx: &mut Context<Self>) {
        if self.selected != Some(number) {
            self.selected = Some(number);
            cx.notify();
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let visible = self.visible();
        let current = visible
            .iter()
            .position(|issue| Some(issue.number) == self.selected);
        let next = match current {
            Some(ix) => ix
                .saturating_add_signed(delta)
                .min(visible.len().saturating_sub(1)),
            None => 0,
        };
        if let Some(issue) = visible.get(next) {
            let number = issue.number;
            self.select(number, cx);
        }
    }

    fn show(&mut self, folder: Folder, cx: &mut Context<Self>) {
        self.folder = folder;
        self.selected = self.visible().first().map(|issue| issue.number);
        cx.notify();
    }

    fn search(&mut self, query: &str, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query.to_string();
            cx.notify();
        }
    }

    fn toggle_star(&mut self, number: u32, cx: &mut Context<Self>) {
        if let Some(issue) = self.issues.iter_mut().find(|issue| issue.number == number) {
            issue.starred = !issue.starred;
            cx.notify();
        }
    }

    fn close_selected(&mut self, cx: &mut Context<Self>) {
        let Some(number) = self.selected else { return };
        if let Some(issue) = self.issues.iter_mut().find(|issue| issue.number == number) {
            issue.open = false;
        }
        self.step(1, cx);
        cx.notify();
    }

    /// Whether the issue list renders slowly on purpose.
    pub fn jank(&self) -> bool {
        self.jank
    }

    fn toggle_jank(&mut self, cx: &mut Context<Self>) {
        self.jank = !self.jank;
        cx.notify();
    }
}

/// Talks to a pretend server: notifies ten times a second, but its visible
/// state only changes once a second — a render hot spot for Loupe to find.
pub struct SyncClient {
    ticks: u64,
    _timer: Option<Task<()>>,
}

/// Sync ticks per second.
const SYNC_TICKS_PER_SECOND: u64 = 10;

impl SyncClient {
    fn new(live: bool, cx: &mut Context<Self>) -> Self {
        let timer = live.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(1000 / SYNC_TICKS_PER_SECOND))
                        .await;
                    let alive = this.update(cx, |this, cx| {
                        this.ticks += 1;
                        cx.notify();
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
        });
        Self {
            ticks: 0,
            _timer: timer,
        }
    }

    fn seconds(&self) -> u64 {
        self.ticks / SYNC_TICKS_PER_SECOND
    }
}

// Palette.

struct Palette;

impl Palette {
    const BG: u32 = 0xfbfbfa;
    const SIDEBAR: u32 = 0xf4f4f2;
    const LINE: u32 = 0xe7e6e3;
    const TEXT: u32 = 0x1c1c1e;
    const MUTED: u32 = 0x6b6b70;
    const FAINT: u32 = 0x9a9a9f;
    const HOVER: u32 = 0xf0f0ee;
    const SELECTED: u32 = 0xe8edfb;
    const ACCENT: u32 = 0x3451d1;
}

// Views.

/// The app's root: title bar, sidebar, issue list and the cached detail pane.
pub struct InboxApp {
    store: Entity<IssueStore>,
    sidebar: Entity<Sidebar>,
    list: Entity<IssueList>,
    detail: Entity<IssueDetail>,
    status: Entity<SyncStatus>,
    focus: FocusHandle,
}

impl InboxApp {
    /// Builds the app with its models and views.
    pub fn new(options: InboxOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.new(|_| IssueStore::new());
        let sync = cx.new(|cx| SyncClient::new(options.live_sync, cx));
        let sidebar = cx.new(|cx| Sidebar::new(store.clone(), cx));
        let list = cx.new(|cx| IssueList::new(store.clone(), window, cx));
        let detail = cx.new(|cx| IssueDetail::new(store.clone(), cx));
        let status = cx.new(|cx| SyncStatus::new(sync, cx));
        Self {
            store,
            sidebar,
            list,
            detail,
            status,
            focus: cx.focus_handle(),
        }
    }

    /// The issue store.
    #[allow(
        dead_code,
        reason = "used by Loupe's end-to-end tests, which include this module"
    )]
    pub fn store(&self) -> &Entity<IssueStore> {
        &self.store
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let jank = self.store.read(cx).jank();
        div()
            .flex_none()
            .h(px(44.))
            .px(px(16.))
            .flex()
            .items_center()
            .gap(px(12.))
            .border_b_1()
            .border_color(rgb(Palette::LINE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .size(px(20.))
                            .rounded(px(6.))
                            .bg(rgb(Palette::ACCENT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(0xffffff))
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("L"),
                    )
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Linework"),
                    ),
            )
            .child(div().flex_1())
            .child(self.status.clone())
            .child(
                div()
                    .id("jank")
                    .h(px(28.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(if jank { 0xf04438 } else { Palette::LINE }))
                    .text_size(px(12.))
                    .text_color(rgb(if jank { 0xb42318 } else { Palette::MUTED }))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(Palette::HOVER)))
                    .child(if jank { "Jank on" } else { "Simulate jank" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.store.update(cx, |store, cx| store.toggle_jank(cx))
                    })),
            )
    }
}

impl Focusable for InboxApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for InboxApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Inbox")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseIssue, _, cx| {
                this.store.update(cx, |store, cx| store.close_selected(cx))
            }))
            .on_action(cx.listener(|this, _: &GoToInbox, _, cx| {
                this.store
                    .update(cx, |store, cx| store.show(Folder::Inbox, cx))
            }))
            .on_action(cx.listener(|this, _: &GoToStarred, _, cx| {
                this.store
                    .update(cx, |store, cx| store.show(Folder::Starred, cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleJank, _, cx| {
                this.store.update(cx, |store, cx| store.toggle_jank(cx))
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.list
                    .update(cx, |list, cx| list.focus_search(window, cx))
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(Palette::BG))
            .font_family(FONT)
            .text_size(px(13.))
            .text_color(rgb(Palette::TEXT))
            .child(self.title_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar.clone())
                    .child(self.list.clone())
                    .child(
                        self.detail
                            .clone()
                            .cached(gpui::StyleRefinement::default().flex_1().h_full()),
                    ),
            )
    }
}

/// Folders with counts.
pub struct Sidebar {
    store: Entity<IssueStore>,
    _observe: Subscription,
}

impl Sidebar {
    fn new(store: Entity<IssueStore>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            store,
            _observe: observe,
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let current = store.folder;
        let folders = Folder::ALL.map(|folder| {
            let count = store.count(folder);
            let selected = folder == current;
            div()
                .id(folder.label())
                .h(px(32.))
                .px(px(12.))
                .flex()
                .items_center()
                .justify_between()
                .rounded(px(6.))
                .cursor_pointer()
                .when(selected, |this| {
                    this.bg(rgb(Palette::SELECTED))
                        .text_color(rgb(Palette::ACCENT))
                })
                .when(!selected, |this| {
                    this.hover(|style| style.bg(rgb(Palette::HOVER)))
                })
                .child(folder.label())
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(Palette::MUTED))
                        .child(count.to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.store.update(cx, |store, cx| store.show(folder, cx))
                }))
        });
        div()
            .flex_none()
            .w(px(200.))
            .h_full()
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .bg(rgb(Palette::SIDEBAR))
            .border_r_1()
            .border_color(rgb(Palette::LINE))
            .children(folders)
    }
}

/// The searchable, keyboard-driven list of issues.
pub struct IssueList {
    store: Entity<IssueStore>,
    search: Entity<EditableTextState>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    _subscriptions: [Subscription; 2],
}

impl IssueList {
    fn new(store: Entity<IssueStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
        let subscriptions = [
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe_in(&search, window, |this, search, _: &TextChanged, _, cx| {
                let query = search.read(cx).as_str().to_string();
                this.store.update(cx, |store, cx| store.search(&query, cx));
            }),
        ];
        Self {
            store,
            search,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.search.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| store.step(delta, cx));
        let store = self.store.read(cx);
        if let Some(ix) = store
            .visible()
            .iter()
            .position(|issue| Some(issue.number) == store.selected)
        {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
    }
}

/// Stands in for real work done during render ("Simulate jank").
fn rank_issues_slowly() {
    let _span = gpui::inspector::span("IssueList::rank");
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(32) {
        std::hint::spin_loop();
    }
}

impl Render for IssueList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        if store.jank() {
            rank_issues_slowly();
        }
        let count = store.visible().len();
        let search = self.search.clone();
        div()
            .key_context("IssueList")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.step(-1, cx)))
            .flex_none()
            .w(px(380.))
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(Palette::LINE))
            .child(
                div()
                    .flex_none()
                    .p(px(8.))
                    .border_b_1()
                    .border_color(rgb(Palette::LINE))
                    .child(
                        div()
                            .h(px(32.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .bg(rgb(0xffffff))
                            .border_1()
                            .border_color(rgb(Palette::LINE))
                            .child(
                                text_input("search")
                                    .state(search.downgrade())
                                    .placeholder("Search issues")
                                    .placeholder_color(rgb(Palette::FAINT))
                                    .caret_color(rgb_to_hsla(rgb(Palette::ACCENT)))
                                    .flex_1()
                                    .whitespace_nowrap()
                                    .overflow_x_hidden(),
                            ),
                    ),
            )
            .child(
                uniform_list(
                    "issues",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let store = this.store.read(cx);
                        let selected = store.selected;
                        let visible = store.visible();
                        let rows: Vec<(usize, Issue)> = range
                            .filter_map(|ix| Some((ix, Issue::clone(visible.get(ix)?))))
                            .collect();
                        rows.into_iter()
                            .map(|(ix, issue)| {
                                let is_selected = selected == Some(issue.number);
                                issue_row(ix, issue, is_selected, cx)
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1(),
            )
    }
}

fn issue_row(
    ix: usize,
    issue: Issue,
    selected: bool,
    cx: &mut Context<IssueList>,
) -> gpui::Stateful<gpui::Div> {
    let number = issue.number;
    div()
        .id(("issue", ix))
        .w_full()
        .h(px(64.))
        .px(px(16.))
        .flex()
        .items_center()
        .gap(px(12.))
        .border_b_1()
        .border_color(rgb(Palette::LINE))
        .cursor_pointer()
        .when(selected, |this| this.bg(rgb(Palette::SELECTED)))
        .when(!selected, |this| {
            this.hover(|style| style.bg(rgb(Palette::HOVER)))
        })
        .child(avatar(&issue.author))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().truncate().child(issue.title.clone()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .text_color(rgb(Palette::MUTED))
                        .child(format!("#{number}"))
                        .children(issue.labels.first().map(|label| label_chip(*label))),
                ),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(Palette::FAINT))
                .child(ago(issue.updated)),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            window.focus(&this.focus, cx);
            this.store.update(cx, |store, cx| store.select(number, cx));
        }))
}

/// The selected issue. Drawn as a cached view: it re-renders only when the
/// store notifies.
pub struct IssueDetail {
    store: Entity<IssueStore>,
    _observe: Subscription,
}

impl IssueDetail {
    fn new(store: Entity<IssueStore>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            store,
            _observe: observe,
        }
    }
}

impl Render for IssueDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(issue) = self.store.read(cx).selected().cloned() else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(Palette::MUTED))
                .child("No issue selected");
        };
        let number = issue.number;
        div()
            .size_full()
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .text_color(rgb(Palette::MUTED))
                    .child(format!("#{number}"))
                    .child("·")
                    .child(if issue.open { "Open" } else { "Closed" })
                    .child("·")
                    .child(format!("updated {}", ago(issue.updated)))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("star")
                            .h(px(28.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(if issue.starred {
                                0xfedf89
                            } else {
                                Palette::LINE
                            }))
                            .cursor_pointer()
                            .text_color(rgb(if issue.starred {
                                0xb54708
                            } else {
                                Palette::MUTED
                            }))
                            .hover(|style| style.bg(rgb(Palette::HOVER)))
                            .child(if issue.starred { "Starred" } else { "Star" })
                            .tooltip(|_, cx| cx.new(|_| Hint("Star this issue")).into())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.store
                                    .update(cx, |store, cx| store.toggle_star(number, cx))
                            })),
                    )
                    // A clickable that keyboards cannot reach: no focus handle, no tab stop.
                    .child(
                        div()
                            .id("more")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(Palette::HOVER)))
                            .text_color(rgb(Palette::MUTED))
                            .child("•••")
                            .on_click(|_, _, _| {}),
                    ),
            )
            .child(
                div()
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(issue.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(avatar(&issue.author))
                    .child(issue.author.clone())
                    .children(issue.labels.iter().map(|label| label_chip(*label))),
            )
            .child(
                div()
                    .line_height(px(20.))
                    .text_color(rgb(0x3a3a3f))
                    .child(issue.body.clone()),
            )
            // An overflowing tag: its fixed width is narrower than its text.
            .child(
                div()
                    .id("milestone")
                    .w(px(96.))
                    .whitespace_nowrap()
                    .text_size(px(12.))
                    .text_color(rgb(Palette::MUTED))
                    .child("Milestone: Loupe preview"),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        button("close-issue", "Close issue", true).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.store.update(cx, |store, cx| store.close_selected(cx))
                            },
                        )),
                    )
                    .child(button("assign", "Assign to me", false)),
            )
            .child(div().flex_1())
            // A footnote too faint to read comfortably.
            .child(
                div()
                    .id("footnote")
                    .text_size(px(12.))
                    .text_color(rgb(0xc9c9cc))
                    .child("Synced from the Linework server. Edits may take a moment to appear."),
            )
    }
}

/// The sync indicator: pulses while a sync is in flight.
pub struct SyncStatus {
    sync: Entity<SyncClient>,
    _observe: Subscription,
}

impl SyncStatus {
    fn new(sync: Entity<SyncClient>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&sync, |_, _, cx| cx.notify());
        Self {
            sync,
            _observe: observe,
        }
    }
}

impl Render for SyncStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let seconds = self.sync.read(cx).seconds();
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.))
            .text_color(rgb(Palette::MUTED))
            .child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .bg(rgb(0x12b76a))
                    .with_animation(
                        ("sync-pulse", seconds),
                        Animation::new(Duration::from_millis(600)),
                        |dot, delta| dot.opacity(0.35 + 0.65 * delta),
                    ),
            )
            .child(if seconds == 0 {
                "Synced".to_string()
            } else {
                format!("Synced · {seconds}s")
            })
    }
}

struct Hint(&'static str);

impl Render for Hint {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(rgb(0x1c1c1e))
            .text_color(rgb(0xffffff))
            .font_family(FONT)
            .text_size(px(12.))
            .child(self.0)
    }
}

fn button(id: &'static str, label: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(32.))
        .px(px(14.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .cursor_pointer()
        .when(primary, |this| {
            this.bg(rgb(Palette::ACCENT))
                .text_color(rgb(0xffffff))
                .hover(|style| style.bg(rgb(0x2a43b8)))
        })
        .when(!primary, |this| {
            this.border_1()
                .border_color(rgb(Palette::LINE))
                .hover(|style| style.bg(rgb(Palette::HOVER)))
        })
        .child(label)
}

fn avatar(name: &str) -> impl IntoElement + use<> {
    const HUES: [u32; 5] = [0xdbe4ff, 0xd3f9d8, 0xffe8cc, 0xf3d9fa, 0xd0ebff];
    let initials: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect();
    let hue = HUES[name.len() % HUES.len()];
    div()
        .flex_none()
        .size(px(28.))
        .rounded_full()
        .bg(rgb(hue))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(0x3a3a3f))
        .child(initials)
}

fn label_chip(label: Label) -> impl IntoElement {
    let (fg, bg) = label.colors();
    div()
        .px(px(8.))
        .h(px(20.))
        .flex()
        .items_center()
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(11.))
        .child(label.name())
}

fn ago(minutes: u32) -> String {
    match minutes {
        0 => "now".into(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

fn sample_issues() -> Vec<Issue> {
    use Label::*;
    let rows: [(&str, &str, &[Label], u32, bool, bool); 18] = [
        (
            "Scroll jank in the issue list",
            "Katherine Johnson",
            &[Performance, Bug],
            4,
            true,
            true,
        ),
        (
            "Hover menu closes before it can be inspected",
            "Grace Hopper",
            &[Bug],
            12,
            true,
            false,
        ),
        (
            "Sidebar counts flicker while syncing",
            "Alan Turing",
            &[Bug, Layout],
            25,
            true,
            false,
        ),
        (
            "Keyboard users cannot reach the ••• menu",
            "Barbara Liskov",
            &[A11y],
            40,
            true,
            true,
        ),
        (
            "Footnote text is hard to read",
            "Edsger Dijkstra",
            &[A11y, Layout],
            75,
            true,
            false,
        ),
        (
            "Detail pane re-renders on every sync tick",
            "Margaret Hamilton",
            &[Performance],
            110,
            true,
            false,
        ),
        (
            "Milestone tag overflows its column",
            "Donald Knuth",
            &[Layout],
            160,
            true,
            false,
        ),
        (
            "Add a keyboard shortcut to close issues",
            "Ken Thompson",
            &[Feature],
            240,
            true,
            false,
        ),
        (
            "Search should match issue numbers",
            "Frances Allen",
            &[Feature],
            300,
            true,
            false,
        ),
        (
            "Avatars are blurry at 150% scale",
            "Radia Perlman",
            &[Bug],
            420,
            true,
            false,
        ),
        (
            "Star button needs a tooltip",
            "Adele Goldberg",
            &[A11y, Feature],
            600,
            true,
            true,
        ),
        (
            "Paint flashing shows the whole window",
            "John Backus",
            &[Performance],
            900,
            true,
            false,
        ),
        (
            "Label colors fail contrast in dark mode",
            "Lynn Conway",
            &[A11y],
            1440,
            true,
            false,
        ),
        (
            "Draft: two-pane layout on narrow windows",
            "Dennis Ritchie",
            &[Layout, Feature],
            2880,
            true,
            false,
        ),
        (
            "Measure frame time on the release build",
            "Hedy Lamarr",
            &[Performance],
            4320,
            false,
            false,
        ),
        (
            "Cache the sidebar view",
            "Ada Lovelace",
            &[Performance],
            5760,
            false,
            true,
        ),
        (
            "Document the Inbox keymap",
            "Leslie Lamport",
            &[Feature],
            7200,
            false,
            false,
        ),
        (
            "Crash when the list is empty",
            "Tony Hoare",
            &[Bug],
            10080,
            false,
            false,
        ),
    ];
    rows.into_iter()
        .enumerate()
        .map(
            |(ix, (title, author, labels, updated, open, starred))| Issue {
                number: 2412 - ix as u32,
                title: title.into(),
                author: author.into(),
                labels: labels.to_vec(),
                body: format!(
                    "{title}. Steps to reproduce: open the Inbox, select the issue and watch the \
                 pane. Expected it to stay smooth and readable; it does not. Loupe should show \
                 why within two clicks."
                )
                .into(),
                updated,
                open,
                starred,
            },
        )
        .collect()
}
