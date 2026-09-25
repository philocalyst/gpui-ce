//! Synthetic captures for tests and screenshots (`test-support`).
//!
//! [`inbox`] builds a realistic, deterministic recording of an "Inbox" mail
//! app: a ~300 element tree (views, components, ids, text, a scrolling list,
//! clipped and overflowing elements, a style override), 240 frames of mostly
//! 3–8 ms with a few 20–40 ms spikes (causes, view spans, user spans and
//! foreground slices), Loupe-only frames, ~200 input records and notify
//! statistics. [`TreeBuilder`], [`FrameBuilder`] and [`InputBuilder`] make
//! targeted captures for focused tests.
//!
//! Source locations cannot be fabricated, so every fabricated element and
//! call site points into this file.

use gpui::{
    Bounds, EntityId, KeyContext, Keystroke, Pixels, Point, SharedString, StyleRefinement,
    Styled as _,
    inspector::{
        ActionRecord, BoxModel, CauseKind, ElementDetails, ElementFlags, ElementIndex, ElementKey,
        ElementKind, ElementRecord, ElementTree, ForegroundKind, ForegroundSlice, FrameRecord,
        InputKind, InputRecord, InspectorCapture, NotifyStats, PhaseTimings, RenderCause,
        SceneStats, UserSpan, ViewOutcome, ViewSpan,
    },
    point, px, rgb, rgb_to_hsla, size,
};
use smallvec::SmallVec;
use std::{collections::VecDeque, panic::Location, sync::Arc, time::Duration};

/// Milliseconds as a [`Duration`].
pub fn ms(value: f64) -> Duration {
    Duration::from_secs_f64(value.max(0.) / 1000.)
}

/// A small deterministic generator (SplitMix64), so fixtures never change.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator seeded with `seed`.
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0.0..1.0`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `low..high`.
    pub fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    /// Uniform index below `len` (`len` must be positive).
    pub fn index(&mut self, len: usize) -> usize {
        (self.next_u64() % len.max(1) as u64) as usize
    }
}

/// The kind and facts of an element added through a [`TreeBuilder`].
#[derive(Clone, Debug)]
pub struct ElementSpec {
    kind: ElementKind,
    id: Option<SharedString>,
    bounds: Bounds<Pixels>,
    flags: ElementFlags,
    details: Option<ElementDetails>,
}

const DIV: &str = "gpui::elements::div::Div";
const TEXT: &str = "gpui::elements::text::StyledText";

impl ElementSpec {
    /// A stateful view boundary.
    pub fn view(type_name: &'static str, entity: u64) -> Self {
        Self::of(ElementKind::View {
            entity: EntityId::from(entity),
            type_name,
        })
    }

    /// A `RenderOnce` component boundary.
    pub fn component(type_name: &'static str) -> Self {
        Self::of(ElementKind::Component { type_name })
    }

    /// A plain `div`.
    pub fn div() -> Self {
        Self::of(ElementKind::Element { type_name: DIV })
    }

    /// Any other element type.
    pub fn element(type_name: &'static str) -> Self {
        Self::of(ElementKind::Element { type_name })
    }

    /// A text element showing `text`.
    pub fn text(text: impl Into<SharedString>) -> Self {
        let mut spec = Self::of(ElementKind::Element { type_name: TEXT });
        spec.details = Some(ElementDetails {
            text: Some(text.into()),
            font_size: Some(px(13.)),
            font_family: Some("Inter".into()),
            text_color: Some(rgb_to_hsla(rgb(0x1f2328))),
            ..Default::default()
        });
        spec
    }

    fn of(kind: ElementKind) -> Self {
        Self {
            kind,
            id: None,
            bounds: Bounds::default(),
            flags: ElementFlags::empty(),
            details: None,
        }
    }

    /// Gives the element an id.
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Sets the layout bounds, in window coordinates.
    pub fn bounds(mut self, x: f32, y: f32, width: f32, height: f32) -> Self {
        self.bounds = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
        self
    }

    /// Adds flags.
    pub fn flags(mut self, flags: ElementFlags) -> Self {
        self.flags |= flags;
        self
    }

    /// A clickable, hoverable control (hitbox, click listener, hover style).
    pub fn clickable(self) -> Self {
        self.flags(ElementFlags::HITBOX | ElementFlags::CLICKABLE | ElementFlags::STATEFUL_STYLE)
    }

    /// Sets element details.
    pub fn details(mut self, details: ElementDetails) -> Self {
        self.details = Some(details);
        self
    }

    /// Sets a solid background and padding, the most common div details.
    pub fn styled(mut self, background: u32, padding: f32) -> Self {
        let details = self.details.get_or_insert_with(Default::default);
        details.background = Some(rgb_to_hsla(rgb(background)));
        details.box_model = Some(BoxModel {
            padding: gpui::Edges::all(px(padding)),
            ..Default::default()
        });
        self
    }
}

/// Builds an [`ElementTree`] depth-first, interning a path per element.
pub struct TreeBuilder<'a> {
    capture: &'a mut InspectorCapture,
    tree: ElementTree,
    stack: Vec<ElementIndex>,
    scopes: Vec<SharedString>,
}

impl<'a> TreeBuilder<'a> {
    /// A builder whose element paths are interned in `capture`.
    pub fn new(capture: &'a mut InspectorCapture) -> Self {
        Self {
            capture,
            tree: ElementTree::default(),
            stack: Vec::new(),
            scopes: Vec::new(),
        }
    }

    /// Adds an element under the current parent and makes it the parent of
    /// the following elements until [`Self::close`].
    #[track_caller]
    pub fn open(&mut self, spec: ElementSpec) -> ElementKey {
        if let Some(id) = &spec.id {
            self.scopes.push(id.clone());
        }
        let key = self.add(spec);
        self.stack
            .push(self.tree.elements.len() as ElementIndex - 1);
        key
    }

    /// Adds a childless element under the current parent.
    #[track_caller]
    pub fn leaf(&mut self, spec: ElementSpec) -> ElementKey {
        let id = spec.id.clone();
        if let Some(id) = &id {
            self.scopes.push(id.clone());
        }
        let key = self.add(spec);
        if id.is_some() {
            self.scopes.pop();
        }
        key
    }

    /// Closes the current parent.
    pub fn close(&mut self) {
        if let Some(ix) = self.stack.pop()
            && self.tree.elements[ix as usize].id.is_some()
        {
            self.scopes.pop();
        }
    }

    /// Records `spec` under the current parent; its id (if any) is already
    /// the innermost scope.
    #[track_caller]
    fn add(&mut self, spec: ElementSpec) -> ElementKey {
        let scope = format!("{:?}", self.scopes);
        let path = self.capture.intern_path_for_test(&scope);
        let key = ElementKey { path, instance: 0 };
        let parent = self.stack.last().copied();
        let depth = self.stack.len() as u16;
        let ix = self.tree.elements.len() as u32;
        self.tree.elements.push(ElementRecord {
            key: Some(key),
            parent,
            depth,
            kind: spec.kind,
            id: spec.id,
            bounds: spec.bounds,
            visible_bounds: (!spec.flags.contains(ElementFlags::CLIPPED)).then_some(spec.bounds),
            paint_order: ix,
            primitives: 1,
            flags: spec.flags,
            details: spec.details.map(Box::new),
        });
        key
    }

    /// Finishes the tree for `frame`.
    pub fn build(mut self, frame: u64) -> ElementTree {
        self.tree.frame = frame;
        self.tree.rebuild_children();
        self.tree
    }
}

/// Builds one [`FrameRecord`].
pub struct FrameBuilder {
    record: FrameRecord,
}

impl Default for FrameBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameBuilder {
    /// An empty 1280×800 frame starting at the epoch.
    pub fn new() -> Self {
        Self {
            record: FrameRecord {
                id: 0,
                start: Duration::ZERO,
                viewport: size(px(1280.), px(800.)),
                timings: PhaseTimings::default(),
                causes: Vec::new(),
                views: Vec::new(),
                spans: Vec::new(),
                scene: SceneStats::default(),
                element_count: 0,
                tree: None,
                input: 0..0,
                foreground: Vec::new(),
                inspector_only: false,
            },
        }
    }

    /// Starts at `start` after the capture epoch.
    pub fn at(mut self, start: Duration) -> Self {
        self.record.start = start;
        self
    }

    /// Splits `app` time over the phases (input 4%, render 52%, layout 16%,
    /// prepaint 10%, paint 18%) and adds `inspector` time on top.
    pub fn app_time(mut self, app: Duration, inspector: Duration) -> Self {
        let share = |fraction: f64| app.mul_f64(fraction);
        self.record.timings = PhaseTimings {
            input: share(0.04),
            render: share(0.52),
            layout: share(0.16),
            prepaint: share(0.10),
            paint: share(0.18),
            inspector,
            present: Some(ms(0.4)),
            total: app + inspector,
        };
        self
    }

    /// Sets the phases explicitly; `total` is their sum.
    pub fn phases(mut self, timings: PhaseTimings) -> Self {
        self.record.timings = timings;
        self
    }

    /// Adds a cause, sited at the caller.
    #[track_caller]
    pub fn cause(mut self, kind: CauseKind, from_inspector: bool) -> Self {
        self.record.causes.push(RenderCause {
            kind,
            site: Some(Location::caller()),
            before_frame: ms(0.3),
            from_inspector,
        });
        self
    }

    /// Adds a view render (or cache hit) span.
    pub fn view(
        mut self,
        entity: u64,
        type_name: &'static str,
        depth: u16,
        start: Duration,
        duration: Duration,
        outcome: ViewOutcome,
    ) -> Self {
        self.record.views.push(ViewSpan {
            entity: EntityId::from(entity),
            type_name,
            element: None,
            depth,
            start,
            duration,
            outcome,
        });
        self
    }

    /// Adds a user span.
    #[track_caller]
    pub fn span(
        mut self,
        name: &'static str,
        depth: u16,
        start: Duration,
        duration: Duration,
    ) -> Self {
        self.record.spans.push(UserSpan {
            name: name.into(),
            site: Location::caller(),
            depth,
            start,
            duration,
        });
        self
    }

    /// Adds foreground work that ran before the frame.
    pub fn foreground(mut self, kind: ForegroundKind, start: Duration, duration: Duration) -> Self {
        self.record.foreground.push(ForegroundSlice {
            kind,
            start,
            duration,
        });
        self
    }

    /// Sets scene statistics.
    pub fn scene(mut self, scene: SceneStats) -> Self {
        self.record.scene = scene;
        self
    }

    /// Retains `tree` with the frame.
    pub fn tree(mut self, tree: Arc<ElementTree>) -> Self {
        self.record.element_count = tree.elements.len() as u32;
        self.record.tree = Some(tree);
        self
    }

    /// Sets the element count without retaining a tree.
    pub fn element_count(mut self, count: u32) -> Self {
        self.record.element_count = count;
        self
    }

    /// Input records (by sequence number) handled before this frame.
    pub fn input(mut self, input: std::ops::Range<u64>) -> Self {
        self.record.input = input;
        self
    }

    /// Marks the frame as caused only by Loupe.
    pub fn inspector_only(mut self) -> Self {
        self.record.inspector_only = true;
        self
    }

    /// The record (its id is assigned when pushed).
    pub fn build(self) -> FrameRecord {
        self.record
    }

    /// Pushes the frame into `capture`, returning its id.
    pub fn push(self, capture: &mut InspectorCapture) -> u64 {
        capture.push_frame_for_test(self.record)
    }
}

/// Builds one [`InputRecord`].
pub struct InputBuilder {
    record: InputRecord,
}

impl InputBuilder {
    fn new(kind: InputKind, at: Duration, detail: impl Into<SharedString>) -> Self {
        Self {
            record: InputRecord {
                seq: 0,
                at,
                frame: None,
                kind,
                detail: detail.into(),
                position: None,
                keystroke: None,
                hit_path: SmallVec::new(),
                context_stack: SmallVec::new(),
                actions: SmallVec::new(),
                handled: false,
                duration: ms(0.08),
                caused_redraw: false,
                coalesced: 1,
                inspector: false,
            },
        }
    }

    /// A left click (mouse down) at `position`.
    pub fn click(at: Duration, position: Point<Pixels>) -> Self {
        let mut builder = Self::new(
            InputKind::MouseDown,
            at,
            format!(
                "left ×1 at ({}, {})",
                f32::from(position.x).round(),
                f32::from(position.y).round()
            ),
        );
        builder.record.position = Some(position);
        builder
    }

    /// A mouse up at `position`.
    pub fn release(at: Duration, position: Point<Pixels>) -> Self {
        let mut builder = Self::new(InputKind::MouseUp, at, "left");
        builder.record.position = Some(position);
        builder
    }

    /// `count` coalesced mouse moves ending at `position`.
    pub fn moves(at: Duration, position: Point<Pixels>, count: u32) -> Self {
        let mut builder = Self::new(
            InputKind::MouseMove,
            at,
            format!(
                "to ({}, {})",
                f32::from(position.x).round(),
                f32::from(position.y).round()
            ),
        );
        builder.record.position = Some(position);
        builder.record.coalesced = count.max(1);
        builder
    }

    /// A scroll at `position`.
    pub fn scroll(at: Duration, position: Point<Pixels>, delta: f32) -> Self {
        let mut builder = Self::new(InputKind::Scroll, at, format!("Δy {delta:+.0}"));
        builder.record.position = Some(position);
        builder
    }

    /// A key press, e.g. `"cmd-k"`.
    pub fn key(at: Duration, keystroke: &str) -> Self {
        let mut builder = Self::new(InputKind::KeyDown, at, keystroke.to_string());
        builder.record.keystroke = Keystroke::parse(keystroke).ok();
        builder
    }

    /// Elements under the pointer, topmost first.
    pub fn hit_path(mut self, path: impl IntoIterator<Item = ElementKey>) -> Self {
        self.record.hit_path = path.into_iter().collect();
        self
    }

    /// The key context stack, outermost first (`"Workspace"`, `"IssueList"`).
    pub fn contexts(mut self, contexts: &[&str]) -> Self {
        self.record.context_stack = contexts
            .iter()
            .filter_map(|context| KeyContext::parse(context).ok())
            .collect();
        self
    }

    /// An action dispatched while handling the event.
    pub fn action(mut self, name: &'static str, handled: bool, context: Option<&str>) -> Self {
        let keystrokes = self.record.keystroke.as_ref().map(|k| k.unparse().into());
        self.record.actions.push(ActionRecord {
            name,
            handled,
            keystrokes,
            context: context.map(|context| SharedString::from(context.to_string())),
        });
        self.record.handled |= handled;
        self
    }

    /// Marks the event handled (and whether it invalidated the window).
    pub fn handled(mut self, caused_redraw: bool) -> Self {
        self.record.handled = true;
        self.record.caused_redraw = caused_redraw;
        self
    }

    /// Marks the event as consumed by Loupe.
    pub fn inspector(mut self) -> Self {
        self.record.inspector = true;
        self
    }

    /// Pushes the record into `capture`, returning its sequence number.
    pub fn push(self, capture: &mut InspectorCapture) -> u64 {
        capture.push_input_for_test(self.record)
    }
}

/// Well-known elements of the [`inbox`] fixture.
#[derive(Clone, Debug)]
pub struct InboxElements {
    /// The `InboxApp` root view.
    pub app: ElementKey,
    /// The `IssueList` view.
    pub issue_list: ElementKey,
    /// `div#row-3` in the list.
    pub row_3: ElementKey,
    /// The `div#close` button in the detail header (24×24).
    pub close: ElementKey,
    /// The subject text of row 5, which overflows its row.
    pub overflowing_subject: ElementKey,
    /// The composer's `div#send` button, which carries a style override.
    pub send: ElementKey,
}

/// Entity ids used by the [`inbox`] fixture's views and models.
pub mod entities {
    /// `inbox::InboxApp` (root view).
    pub const APP: u64 = 1;
    /// `inbox::Sidebar` view.
    pub const SIDEBAR: u64 = 2;
    /// `inbox::IssueList` view.
    pub const ISSUE_LIST: u64 = 3;
    /// `inbox::SearchField` view.
    pub const SEARCH: u64 = 4;
    /// `inbox::IssueDetail` view.
    pub const DETAIL: u64 = 5;
    /// `inbox::IssueStore` model.
    pub const STORE: u64 = 6;
    /// `inbox::SyncClient` model.
    pub const SYNC: u64 = 7;
}

const SENDERS: [&str; 8] = [
    "Grace Hopper",
    "Alan Turing",
    "Katherine Johnson",
    "Edsger Dijkstra",
    "Barbara Liskov",
    "Donald Knuth",
    "Margaret Hamilton",
    "Ken Thompson",
];

const SUBJECTS: [&str; 8] = [
    "Re: flaky layout test on CI",
    "Release notes for 0.2.3",
    "Scroll jank in the issue list when syncing hundreds of labels at once",
    "Design review: new sidebar",
    "Weekly sync notes",
    "Crash when closing the last tab while a drag is in progress",
    "Question about text shaping",
    "Welcome to the team!",
];

/// Builds the Inbox app's element tree (about 300 elements) into `capture`.
pub fn inbox_tree(capture: &mut InspectorCapture, frame: u64) -> (ElementTree, InboxElements) {
    let mut tree = TreeBuilder::new(capture);
    let app =
        tree.open(ElementSpec::view("inbox::InboxApp", entities::APP).bounds(0., 0., 1280., 800.));
    tree.open(
        ElementSpec::div()
            .id("root")
            .bounds(0., 0., 1280., 800.)
            .styled(0xffffff, 0.),
    );

    // Sidebar: account, navigation and labels.
    tree.open(ElementSpec::view("inbox::Sidebar", entities::SIDEBAR).bounds(0., 0., 240., 800.));
    tree.open(
        ElementSpec::div()
            .id("sidebar")
            .bounds(0., 0., 240., 800.)
            .styled(0xf6f8fa, 12.),
    );
    tree.open(
        ElementSpec::div()
            .id("account")
            .bounds(12., 12., 216., 40.)
            .clickable(),
    );
    tree.leaf(ElementSpec::component("inbox::ui::Avatar").bounds(12., 16., 32., 32.));
    tree.leaf(ElementSpec::text("Ada Lovelace").bounds(52., 20., 120., 18.));
    tree.leaf(ElementSpec::text("ada@example.com").bounds(52., 36., 140., 14.));
    tree.close();
    tree.open(ElementSpec::div().id("nav").bounds(12., 64., 216., 216.));
    for (ix, (name, count)) in [
        ("Inbox", Some("24")),
        ("Starred", None),
        ("Snoozed", Some("3")),
        ("Sent", None),
        ("Drafts", Some("1")),
        ("Archive", None),
    ]
    .into_iter()
    .enumerate()
    {
        let y = 64. + ix as f32 * 32.;
        tree.open(ElementSpec::component("inbox::ui::NavItem").bounds(12., y, 216., 28.));
        tree.open(
            ElementSpec::div()
                .id(format!("nav-{}", name.to_lowercase()))
                .bounds(12., y, 216., 28.)
                .clickable()
                .flags(ElementFlags::FOCUSABLE | ElementFlags::TAB_STOP),
        );
        tree.leaf(ElementSpec::element("gpui::elements::svg::Svg").bounds(20., y + 6., 16., 16.));
        tree.leaf(ElementSpec::text(name).bounds(44., y + 6., 100., 16.));
        if let Some(count) = count {
            tree.open(ElementSpec::component("inbox::ui::Badge").bounds(196., y + 6., 24., 16.));
            tree.leaf(ElementSpec::text(count).bounds(202., y + 6., 12., 16.));
            tree.close();
        }
        tree.close();
        tree.close();
    }
    tree.close();
    tree.open(
        ElementSpec::div()
            .id("labels")
            .bounds(12., 296., 216., 180.),
    );
    tree.leaf(ElementSpec::text("LABELS").bounds(12., 296., 60., 14.));
    for (ix, label) in ["bug", "design", "perf", "docs", "triage"]
        .into_iter()
        .enumerate()
    {
        let y = 318. + ix as f32 * 28.;
        tree.open(ElementSpec::component("inbox::ui::LabelChip").bounds(12., y, 216., 24.));
        tree.open(
            ElementSpec::div()
                .id(format!("label-{label}"))
                .bounds(12., y, 216., 24.)
                .clickable(),
        );
        tree.leaf(
            ElementSpec::div()
                .bounds(20., y + 8., 8., 8.)
                .styled(0x8250df, 0.),
        );
        tree.leaf(ElementSpec::text(label).bounds(36., y + 4., 80., 16.));
        tree.close();
        tree.close();
    }
    tree.close();
    tree.close();
    tree.close();

    // Issue list: search toolbar and a scrolling list of rows.
    let issue_list = tree.open(
        ElementSpec::view("inbox::IssueList", entities::ISSUE_LIST).bounds(240., 0., 620., 800.),
    );
    tree.open(
        ElementSpec::div()
            .id("issue-list")
            .bounds(240., 0., 620., 800.),
    );
    tree.open(
        ElementSpec::div()
            .id("list-toolbar")
            .bounds(240., 0., 620., 48.)
            .styled(0xffffff, 8.),
    );
    tree.open(
        ElementSpec::view("inbox::SearchField", entities::SEARCH).bounds(248., 8., 380., 32.),
    );
    tree.open(
        ElementSpec::element("gpui_ce_elements::editable_text::EditableTextElement")
            .id("search")
            .bounds(248., 8., 380., 32.)
            .flags(
                ElementFlags::HITBOX
                    | ElementFlags::FOCUSABLE
                    | ElementFlags::TAB_STOP
                    | ElementFlags::KEYBOARD,
            ),
    );
    tree.leaf(ElementSpec::text("Search mail").bounds(260., 16., 120., 16.));
    tree.close();
    tree.close();
    for (ix, name) in ["refresh", "mark-read", "more"].into_iter().enumerate() {
        let x = 740. + ix as f32 * 36.;
        tree.open(ElementSpec::component("inbox::ui::IconButton").bounds(x, 10., 28., 28.));
        tree.open(
            ElementSpec::div()
                .id(name)
                .bounds(x, 10., 28., 28.)
                .clickable(),
        );
        tree.leaf(ElementSpec::element("gpui::elements::svg::Svg").bounds(x + 6., 16., 16., 16.));
        tree.close();
        tree.close();
    }
    tree.close();
    tree.open(
        ElementSpec::element("gpui::elements::uniform_list::UniformList")
            .id("rows")
            .bounds(240., 48., 620., 752.)
            .flags(ElementFlags::SCROLLABLE | ElementFlags::HITBOX)
            .details(ElementDetails {
                list: Some((40, 0..18)),
                content_size: Some(size(px(620.), px(2560.))),
                scroll_offset: Some(point(px(0.), px(0.))),
                ..Default::default()
            }),
    );
    let mut row_3 = None;
    let mut overflowing_subject = None;
    for ix in 0..18 {
        let y = 48. + ix as f32 * 64.;
        let clipped = y >= 800.;
        let clip = if clipped {
            ElementFlags::CLIPPED
        } else {
            ElementFlags::empty()
        };
        tree.open(
            ElementSpec::component("inbox::IssueRow")
                .bounds(240., y, 620., 64.)
                .flags(clip),
        );
        let row = tree.open(
            ElementSpec::div()
                .id(format!("row-{ix}"))
                .bounds(240., y, 620., 64.)
                .clickable()
                .flags(clip)
                .styled(if ix == 3 { 0xddf4ff } else { 0xffffff }, 12.),
        );
        if ix == 3 {
            row_3 = Some(row);
        }
        tree.leaf(
            ElementSpec::component("inbox::ui::Avatar")
                .bounds(252., y + 16., 32., 32.)
                .flags(clip),
        );
        tree.open(
            ElementSpec::div()
                .bounds(296., y + 12., 480., 40.)
                .flags(clip),
        );
        tree.leaf(
            ElementSpec::text(SENDERS[ix % SENDERS.len()])
                .bounds(296., y + 12., 200., 18.)
                .flags(clip),
        );
        let subject = SUBJECTS[ix % SUBJECTS.len()];
        let overflows = subject.len() > 50;
        let subject_key = tree.leaf(
            ElementSpec::text(subject)
                .bounds(296., y + 32., if overflows { 540. } else { 380. }, 18.)
                .flags(clip)
                .flags(if overflows {
                    ElementFlags::OVERFLOWS_PARENT
                } else {
                    ElementFlags::empty()
                }),
        );
        if ix == 5 {
            overflowing_subject = Some(subject_key);
        }
        tree.close();
        tree.leaf(
            ElementSpec::text(format!("{}:{:02}", 9 + ix / 3, (ix * 7) % 60))
                .bounds(784., y + 12., 40., 16.)
                .flags(clip),
        );
        tree.leaf(
            ElementSpec::div()
                .id(format!("star-{ix}"))
                .bounds(828., y + 20., 20., 20.)
                .clickable()
                .flags(clip),
        );
        tree.close();
        tree.close();
    }
    tree.close();
    tree.close();
    tree.close();

    // Detail: header actions, the thread and the composer.
    tree.open(
        ElementSpec::view("inbox::IssueDetail", entities::DETAIL).bounds(860., 0., 420., 800.),
    );
    tree.open(
        ElementSpec::div()
            .id("detail")
            .bounds(860., 0., 420., 800.)
            .styled(0xffffff, 16.),
    );
    tree.open(
        ElementSpec::div()
            .id("detail-header")
            .bounds(860., 0., 420., 56.),
    );
    tree.leaf(ElementSpec::text("Scroll jank in the issue list").bounds(876., 16., 260., 22.));
    tree.open(
        ElementSpec::div()
            .id("actions")
            .bounds(1140., 16., 124., 24.),
    );
    let mut close = None;
    for (ix, action) in ["archive", "snooze", "reply", "close"]
        .into_iter()
        .enumerate()
    {
        let x = 1140. + ix as f32 * 32.;
        tree.open(ElementSpec::component("inbox::ui::IconButton").bounds(x, 16., 24., 24.));
        let key = tree.open(
            ElementSpec::div()
                .id(action)
                .bounds(x, 16., 24., 24.)
                .clickable()
                .flags(ElementFlags::TOOLTIP)
                .styled(0xf6f8fa, 4.),
        );
        if action == "close" {
            close = Some(key);
        }
        tree.leaf(ElementSpec::element("gpui::elements::svg::Svg").bounds(x + 4., 20., 16., 16.));
        tree.close();
        tree.close();
    }
    tree.close();
    tree.close();
    tree.open(
        ElementSpec::div()
            .id("thread")
            .bounds(860., 56., 420., 600.)
            .flags(ElementFlags::SCROLLABLE | ElementFlags::HITBOX),
    );
    for ix in 0..6 {
        let y = 64. + ix as f32 * 116.;
        tree.open(ElementSpec::component("inbox::Message").bounds(876., y, 388., 108.));
        tree.open(
            ElementSpec::div()
                .id(format!("message-{ix}"))
                .bounds(876., y, 388., 108.)
                .styled(0xffffff, 12.),
        );
        tree.open(ElementSpec::div().bounds(888., y + 12., 364., 32.));
        tree.leaf(ElementSpec::component("inbox::ui::Avatar").bounds(888., y + 12., 24., 24.));
        tree.leaf(ElementSpec::text(SENDERS[(ix + 2) % SENDERS.len()]).bounds(
            920.,
            y + 14.,
            160.,
            18.,
        ));
        tree.leaf(ElementSpec::text("2h ago").bounds(1200., y + 14., 50., 16.));
        tree.close();
        tree.leaf(
            ElementSpec::text(
                "I can reproduce this on main: scrolling while a sync is running drops frames \
                 whenever the label counts update.",
            )
            .bounds(888., y + 48., 364., 48.),
        );
        tree.close();
        tree.close();
    }
    tree.close();
    tree.open(
        ElementSpec::div()
            .id("composer")
            .bounds(860., 656., 420., 144.)
            .styled(0xf6f8fa, 12.),
    );
    tree.leaf(
        ElementSpec::element("gpui_ce_elements::editable_text::EditableTextElement")
            .id("reply")
            .bounds(872., 668., 396., 88.)
            .flags(
                ElementFlags::HITBOX
                    | ElementFlags::FOCUSABLE
                    | ElementFlags::TAB_STOP
                    | ElementFlags::KEYBOARD,
            ),
    );
    let send = tree.open(
        ElementSpec::div()
            .id("send")
            .bounds(1196., 764., 72., 28.)
            .clickable()
            .flags(ElementFlags::OVERRIDDEN)
            .styled(0x1f883d, 6.),
    );
    tree.leaf(ElementSpec::text("Send").bounds(1210., 770., 44., 16.));
    tree.close();
    tree.close();
    tree.close();
    tree.close();
    tree.close();
    tree.close();

    // A deferred tooltip, painted above everything as its own root.
    tree.open(
        ElementSpec::component("inbox::ui::Tooltip")
            .bounds(1180., 44., 96., 24.)
            .flags(ElementFlags::DEFERRED),
    );
    tree.leaf(ElementSpec::text("Close issue").bounds(1188., 48., 80., 16.));
    tree.close();

    let tree = tree.build(frame);
    let elements = InboxElements {
        app,
        issue_list,
        row_3: row_3.unwrap_or(app),
        close: close.unwrap_or(app),
        overflowing_subject: overflowing_subject.unwrap_or(app),
        send,
    };
    (tree, elements)
}

fn inbox_scene(rng: &mut Rng) -> SceneStats {
    SceneStats {
        quads: 780 + rng.index(80) as u32,
        shadows: 12,
        paths: 4,
        underlines: 2,
        monochrome_sprites: 1_400 + rng.index(200) as u32,
        subpixel_sprites: 0,
        polychrome_sprites: 14,
        surfaces: 0,
        backdrop_filters: 0,
        paint_operations: 2_600 + rng.index(300) as u32,
    }
}

const VIEWS: [(u64, &str, u16); 5] = [
    (entities::APP, "inbox::InboxApp", 0),
    (entities::SIDEBAR, "inbox::Sidebar", 1),
    (entities::ISSUE_LIST, "inbox::IssueList", 1),
    (entities::SEARCH, "inbox::SearchField", 2),
    (entities::DETAIL, "inbox::IssueDetail", 1),
];

/// Adds view spans for a frame whose render phase took `render`: the listed
/// entities re-rendered, the rest were served from the view cache.
fn with_views(mut frame: FrameBuilder, render: Duration, rendered: &[u64]) -> FrameBuilder {
    let mut start = Duration::ZERO;
    for (entity, type_name, depth) in VIEWS {
        let outcome = if rendered.contains(&entity) {
            ViewOutcome::Rendered
        } else {
            ViewOutcome::Cached
        };
        let duration = match (outcome, depth) {
            (ViewOutcome::Cached, _) => ms(0.02),
            (ViewOutcome::Rendered, 0) => render,
            (ViewOutcome::Rendered, _) => render.mul_f64(0.8 / rendered.len().max(1) as f64),
        };
        frame = frame.view(entity, type_name, depth, start, duration, outcome);
        if depth > 0 {
            start += duration;
        }
    }
    frame
}

/// The full Inbox recording: tree, 240 frames, ~200 input records and
/// notify statistics. Deterministic.
pub fn inbox() -> (InspectorCapture, InboxElements) {
    let mut capture = InspectorCapture::new_for_test();
    let (tree, elements) = inbox_tree(&mut capture, 0);
    let tree = Arc::new(tree);
    let element_count = tree.elements.len() as u32;
    let mut rng = Rng::new(0x1B0C);
    let mut clock = ms(120.);
    let mut seq = 0u64;
    let frame_count = 240;
    let spikes = [37usize, 61, 102, 150, 188, 217];

    capture.set_override(
        elements.send.path,
        Some(StyleRefinement::default().bg(rgb(0x2da44e))),
    );

    for ix in 0..frame_count {
        // Bursts of 60 fps activity separated by idle gaps.
        clock += if ix % 24 == 0 && ix > 0 {
            ms(rng.between(180., 700.))
        } else {
            ms(rng.between(16.2, 17.2))
        };
        let input_start = seq;
        let loupe_only = ix % 11 == 7;
        let spike = spikes.contains(&ix);

        // Input handled before this frame.
        if loupe_only {
            InputBuilder::moves(clock - ms(4.), point(px(1320.), px(90.)), 3)
                .inspector()
                .push(&mut capture);
            seq += 1;
        } else {
            let row = rng.index(12);
            let row_y = 48. + row as f32 * 64. + 30.;
            let position = point(px(rng.between(300., 820.) as f32), px(row_y));
            let hit_path = [elements.row_3, elements.issue_list, elements.app];
            match ix % 6 {
                0 => {
                    InputBuilder::click(clock - ms(6.), position)
                        .hit_path(hit_path)
                        .action("inbox::OpenIssue", true, Some("IssueList"))
                        .handled(true)
                        .push(&mut capture);
                    InputBuilder::release(clock - ms(3.), position)
                        .hit_path(hit_path)
                        .push(&mut capture);
                    seq += 2;
                }
                1 | 4 => {
                    let count = 2 + rng.index(14) as u32;
                    InputBuilder::moves(clock - ms(5.), position, count)
                        .hit_path(hit_path)
                        .handled(ix % 2 == 0)
                        .push(&mut capture);
                    seq += 1;
                }
                2 => {
                    let (key, action) = [
                        ("j", "inbox::SelectNext"),
                        ("k", "inbox::SelectPrevious"),
                        ("e", "inbox::Archive"),
                        ("cmd-k", "inbox::OpenCommandBar"),
                    ][rng.index(4)];
                    InputBuilder::key(clock - ms(5.), key)
                        .contexts(&["Workspace", "IssueList"])
                        .action(action, true, Some("IssueList"))
                        .push(&mut capture);
                    seq += 1;
                }
                3 => {
                    InputBuilder::scroll(clock - ms(5.), position, -rng.between(8., 64.) as f32)
                        .hit_path(hit_path)
                        .handled(true)
                        .push(&mut capture);
                    seq += 1;
                }
                _ => {}
            }
        }

        let app = if spike {
            ms(rng.between(21., 39.))
        } else if rng.chance(0.08) {
            ms(rng.between(8., 14.))
        } else {
            ms(rng.between(2.8, 7.6))
        };
        let inspector = ms(rng.between(0.25, 0.7));
        let mut frame = FrameBuilder::new()
            .at(clock)
            .app_time(if loupe_only { ms(0.9) } else { app }, inspector)
            .scene(inbox_scene(&mut rng))
            .input(input_start..seq);
        frame = if loupe_only {
            with_views(frame.inspector_only(), ms(0.4), &[])
                .cause(CauseKind::Input { event: "MouseMove" }, true)
        } else if ix == 0 {
            with_views(frame, app.mul_f64(0.52), &VIEWS.map(|view| view.0))
                .cause(CauseKind::Initial, false)
        } else if spike {
            let render = app.mul_f64(0.52);
            with_views(
                frame,
                render,
                &[
                    entities::APP,
                    entities::SIDEBAR,
                    entities::ISSUE_LIST,
                    entities::DETAIL,
                ],
            )
            .cause(
                CauseKind::Notify {
                    entity: EntityId::from(entities::STORE),
                    type_name: Some("inbox::IssueStore"),
                },
                false,
            )
            .span("sort_issues", 0, ms(1.2), render.mul_f64(0.55))
            .span("group_by_label", 1, ms(1.6), render.mul_f64(0.3))
            .foreground(
                ForegroundKind::Task {
                    site: Location::caller(),
                },
                clock.saturating_sub(ms(9.)),
                ms(rng.between(3., 7.)),
            )
            .foreground(
                ForegroundKind::Action {
                    name: "inbox::SyncNow",
                },
                clock.saturating_sub(ms(3.)),
                ms(0.6),
            )
        } else {
            let cause = match ix % 6 {
                2 => CauseKind::Input { event: "KeyDown" },
                3 => CauseKind::Input {
                    event: "ScrollWheel",
                },
                0 => CauseKind::Input { event: "MouseDown" },
                _ => CauseKind::Notify {
                    entity: EntityId::from(entities::ISSUE_LIST),
                    type_name: Some("inbox::IssueList"),
                },
            };
            with_views(frame, app.mul_f64(0.52), &[entities::ISSUE_LIST]).cause(cause, false)
        };
        let keep_tree = spike || ix + 6 >= frame_count;
        frame = if keep_tree {
            let mut snapshot = (*tree).clone();
            snapshot.frame = capture.frames().back().map_or(0, |last| last.id + 1);
            frame.tree(Arc::new(snapshot))
        } else {
            frame.element_count(element_count)
        };
        frame.push(&mut capture);
    }

    let site = Location::caller();
    for (entity, total, rate) in [
        (entities::STORE, 412u64, 9u32),
        (entities::ISSUE_LIST, 188, 4),
        (entities::SYNC, 64, 2),
        (entities::SEARCH, 21, 1),
        (entities::SIDEBAR, 6, 0),
    ] {
        let buckets: VecDeque<u32> = (0..120)
            .map(|ix| rate + ((ix * 7 + entity as usize) % 5) as u32)
            .collect();
        capture.set_notify_stats_for_test(
            EntityId::from(entity),
            NotifyStats {
                total,
                buckets,
                last_site: Some(site),
            },
        );
    }
    (capture, elements)
}

/// A capture with `count` app frames of `app_ms` each (16 ms apart) and no tree.
pub fn steady_frames(count: usize, app_ms: f64) -> InspectorCapture {
    let mut capture = InspectorCapture::new_for_test();
    for ix in 0..count {
        FrameBuilder::new()
            .at(ms(ix as f64 * 16.667))
            .app_time(ms(app_ms), ms(0.3))
            .cause(CauseKind::Animation, false)
            .push(&mut capture);
    }
    capture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inbox_fixture_is_realistic_and_deterministic() {
        let (capture, elements) = inbox();
        let frames = capture.frames();
        assert_eq!(frames.len(), 240);
        let tree = capture
            .latest_tree()
            .expect("recent frames keep their tree");
        assert!(
            (250..=340).contains(&tree.elements.len()),
            "{}",
            tree.elements.len()
        );
        assert!(tree.find(elements.row_3).is_some());
        assert!(
            tree.elements
                .iter()
                .any(|record| record.flags.contains(ElementFlags::CLIPPED))
        );
        assert!(
            tree.elements
                .iter()
                .any(|record| record.flags.contains(ElementFlags::OVERFLOWS_PARENT))
        );
        let slow = frames
            .iter()
            .filter(|frame| frame.timings.app_total() > ms(20.))
            .count();
        assert!((5..=10).contains(&slow), "{slow} spikes");
        assert!(frames.iter().any(|frame| frame.inspector_only));
        assert!((150..=260).contains(&capture.input().len()));
        assert_eq!(capture.notify_stats().len(), 5);

        let (again, _) = inbox();
        let totals = |capture: &InspectorCapture| -> Vec<Duration> {
            capture
                .frames()
                .iter()
                .map(|frame| frame.timings.total)
                .collect()
        };
        assert_eq!(totals(&capture), totals(&again));
    }

    #[test]
    fn tree_builder_links_parents_and_depths() {
        let mut capture = InspectorCapture::new_for_test();
        let mut builder = TreeBuilder::new(&mut capture);
        let root = builder.open(ElementSpec::view("app::Root", 1).bounds(0., 0., 100., 100.));
        let child = builder.leaf(ElementSpec::div().id("child"));
        builder.close();
        let tree = builder.build(7);
        assert_eq!(tree.frame, 7);
        assert_eq!(tree.roots, [0]);
        let child_ix = tree.find(child).unwrap();
        assert_eq!(tree.get(child_ix).unwrap().parent, tree.find(root));
        assert_eq!(tree.get(child_ix).unwrap().depth, 1);
    }
}
