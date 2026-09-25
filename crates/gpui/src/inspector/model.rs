//! Plain data recorded by the inspector's capture while it is open.
//!
//! Everything here is owned, cheap to clone and free of entity handles, so the
//! inspector UI (or an exporter) can read frames long after the app moved on.
//! Identity across frames is carried by [`ElementKey`]: an interned
//! [`crate::InspectorElementPath`] plus its instance number.

use crate::{
    Bounds, EntityId, Hsla, KeyContext, Keystroke, Pixels, Point, SharedString, Size,
    geometry::Edges,
};
use smallvec::SmallVec;
use std::{panic::Location, sync::Arc, time::Duration};

/// Interned [`crate::InspectorElementPath`]: the same construction site under the
/// same element-id scope maps to the same key for the whole capture session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PathKey(pub u32);

/// Stable identity of an element across frames: its interned path plus the
/// instance number that disambiguates siblings built at the same site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementKey {
    /// Interned path of the element.
    pub path: PathKey,
    /// Mirrors [`crate::InspectorElementId::instance_id`].
    pub instance: u32,
}

/// Index of an [`ElementRecord`] inside one [`ElementTree`]. Only meaningful
/// for the tree it came from; use [`ElementKey`] across frames.
pub type ElementIndex = u32;

/// What the interner knows about a [`PathKey`].
#[derive(Clone, Debug)]
pub struct PathInfo {
    /// Where the element was constructed.
    pub source: &'static Location<'static>,
    /// `Debug` rendering of the nearest ancestor `GlobalElementId`, e.g.
    /// `["root", "issue-list", View(7)]`, for breadcrumbs and search.
    pub scope: SharedString,
}

/// The coarse category of a recorded element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementKind {
    /// A stateful view boundary (`Entity<V: Render>` drawn as an element).
    View {
        /// The rendered entity.
        entity: EntityId,
        /// `type_name::<V>()`.
        type_name: &'static str,
    },
    /// A stateless `RenderOnce` component boundary.
    Component {
        /// `type_name::<C>()`.
        type_name: &'static str,
    },
    /// Any other element; the type name of the `Element` impl.
    Element {
        /// `type_name::<E>()`, e.g. `gpui::elements::div::Div`.
        type_name: &'static str,
    },
}

impl ElementKind {
    /// Full Rust type name of the element, view or component.
    pub fn type_name(&self) -> &'static str {
        match self {
            ElementKind::View { type_name, .. }
            | ElementKind::Component { type_name }
            | ElementKind::Element { type_name } => type_name,
        }
    }

    /// Type name without module path or generic arguments: `gpui::Div<T>` → `Div`.
    pub fn short_name(&self) -> &'static str {
        short_type_name(self.type_name())
    }
}

/// Strips module paths and generic arguments from a `type_name`.
pub fn short_type_name(type_name: &'static str) -> &'static str {
    let without_generics = type_name.split('<').next().unwrap_or(type_name);
    without_generics
        .rsplit("::")
        .next()
        .unwrap_or(without_generics)
}

bitflags::bitflags! {
    /// Cheap per-element facts recorded for every element while capturing.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct ElementFlags: u32 {
        /// The element registered a hitbox this frame.
        const HITBOX = 1 << 0;
        /// Has click or mouse-down/up listeners.
        const CLICKABLE = 1 << 1;
        /// Tracks a focus handle.
        const FOCUSABLE = 1 << 2;
        /// Participates in tab navigation.
        const TAB_STOP = 1 << 3;
        /// Scrolls on at least one axis.
        const SCROLLABLE = 1 << 4;
        /// Has hover, active, focus or group styles.
        const STATEFUL_STYLE = 1 << 5;
        /// Has a tooltip.
        const TOOLTIP = 1 << 6;
        /// Has drag or drop listeners.
        const DRAG_DROP = 1 << 7;
        /// Has key or action listeners.
        const KEYBOARD = 1 << 8;
        /// Belongs to a cached view subtree that was reused this frame; the
        /// record was carried over from the frame that last rendered it.
        const REUSED = 1 << 9;
        /// Painted through `deferred`, above its siblings.
        const DEFERRED = 1 << 10;
        /// Clipped away entirely by an ancestor content mask.
        const CLIPPED = 1 << 11;
        /// Has an inspector style override applied this frame.
        const OVERRIDDEN = 1 << 12;
        /// Overflows its parent's bounds without the parent clipping it.
        const OVERFLOWS_PARENT = 1 << 13;
    }
}

/// One element as it was drawn in one frame.
#[derive(Clone, Debug)]
pub struct ElementRecord {
    /// Identity across frames. `None` for elements without a source location.
    pub key: Option<ElementKey>,
    /// Parent record in the same tree.
    pub parent: Option<ElementIndex>,
    /// Depth from the window root (root = 0).
    pub depth: u16,
    /// What kind of element this is.
    pub kind: ElementKind,
    /// Own `ElementId`, if the element has one (e.g. `.id("close")`).
    pub id: Option<SharedString>,
    /// Layout bounds in window coordinates.
    pub bounds: Bounds<Pixels>,
    /// Bounds intersected with the active content mask; `None` when fully clipped.
    pub visible_bounds: Option<Bounds<Pixels>>,
    /// Position in paint order across the whole frame (higher paints later / on top).
    pub paint_order: u32,
    /// Scene primitives painted by this element and its descendants.
    pub primitives: u32,
    /// Cheap facts; see [`ElementFlags`].
    pub flags: ElementFlags,
    /// Rich, element-specific facts reported through
    /// [`crate::Window::inspect_current_element`].
    pub details: Option<Box<ElementDetails>>,
}

/// A size requirement along one axis, as written in the style.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SizeSpec {
    /// No explicit size.
    #[default]
    Auto,
    /// A definite length in pixels (rems already resolved).
    Pixels(Pixels),
    /// A fraction of the parent's size, 0.0..=1.0.
    Fraction(f32),
}

/// The layout inputs needed to explain "why is this element this size".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutFacts {
    /// `display: flex` / `grid` / `block` / `none`, as written.
    pub display: SharedString,
    /// Flex direction when this element is a flex container.
    pub flex_direction: Option<SharedString>,
    /// Requested width / height.
    pub size: Size<SizeSpec>,
    /// Min width / height.
    pub min_size: Size<SizeSpec>,
    /// Max width / height.
    pub max_size: Size<SizeSpec>,
    /// Flex grow factor.
    pub flex_grow: f32,
    /// Flex shrink factor.
    pub flex_shrink: f32,
    /// Flex basis.
    pub flex_basis: SizeSpec,
    /// Gap between children.
    pub gap: Size<Pixels>,
    /// `position: absolute`.
    pub absolute: bool,
    /// Aspect ratio constraint.
    pub aspect_ratio: Option<f32>,
}

/// Resolved box model in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoxModel {
    /// Margin edges.
    pub margin: Edges<Pixels>,
    /// Border widths.
    pub border: Edges<Pixels>,
    /// Padding edges.
    pub padding: Edges<Pixels>,
}

/// Element-specific facts. Elements fill in whatever applies to them.
#[derive(Clone, Debug, Default)]
pub struct ElementDetails {
    /// Text content (text elements) or a short label.
    pub text: Option<SharedString>,
    /// Resolved box model.
    pub box_model: Option<BoxModel>,
    /// Layout inputs.
    pub layout: Option<LayoutFacts>,
    /// Solid background fill, if any.
    pub background: Option<Hsla>,
    /// Border color, if any.
    pub border_color: Option<Hsla>,
    /// Text color in effect.
    pub text_color: Option<Hsla>,
    /// Font size in effect.
    pub font_size: Option<Pixels>,
    /// Font family in effect.
    pub font_family: Option<SharedString>,
    /// Largest corner radius.
    pub corner_radius: Option<Pixels>,
    /// Opacity, if not 1.
    pub opacity: Option<f32>,
    /// Content size for scrollable elements.
    pub content_size: Option<Size<Pixels>>,
    /// Current scroll offset for scrollable elements.
    pub scroll_offset: Option<Point<Pixels>>,
    /// Key context pushed by the element.
    pub key_context: Option<SharedString>,
    /// Accessibility role, as an AccessKit role name.
    pub a11y_role: Option<SharedString>,
    /// Accessibility label.
    pub a11y_label: Option<SharedString>,
    /// Image / SVG source.
    pub source: Option<SharedString>,
    /// `(item_count, visible_range)` for virtual lists.
    pub list: Option<(usize, std::ops::Range<usize>)>,
    /// Free-form extra rows: `(label, value)`.
    pub extra: SmallVec<[(SharedString, SharedString); 2]>,
}

/// The element tree of one frame, in capture order.
#[derive(Clone, Debug, Default)]
pub struct ElementTree {
    /// Frame this tree was captured in.
    pub frame: u64,
    /// Records; parents always precede their children.
    pub elements: Vec<ElementRecord>,
    /// Children of each record, in order: `children[children_start[i]..children_start[i + 1]]`.
    pub children: Vec<ElementIndex>,
    /// Offsets into [`Self::children`], one per record plus a trailing end.
    pub children_start: Vec<u32>,
    /// Indices of the root records (window root, deferred draws, tooltips...).
    pub roots: Vec<ElementIndex>,
}

/// Durations of the phases of one frame. `render` covers `request_layout`
/// (which is where views render), `layout` is taffy time accumulated across
/// every lazy layout computation, `prepaint` and `paint` exclude layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhaseTimings {
    /// Input handling that led to this frame (since the previous frame).
    pub input: Duration,
    /// Building element trees (`Render::render` + `request_layout`).
    pub render: Duration,
    /// Taffy layout.
    pub layout: Duration,
    /// Prepaint (bounds, hitboxes), excluding layout.
    pub prepaint: Duration,
    /// Paint (scene building), excluding layout.
    pub paint: Duration,
    /// Time spent drawing the inspector's own UI and overlays this frame.
    pub inspector: Duration,
    /// Platform present (GPU submission), if the frame has been presented.
    pub present: Option<Duration>,
    /// Whole `Window::draw`.
    pub total: Duration,
}

impl PhaseTimings {
    /// App-attributable time: the draw minus the inspector's own share.
    pub fn app_total(&self) -> Duration {
        self.total.saturating_sub(self.inspector)
    }
}

/// One view's render (or reuse) within a frame, for the flame chart.
#[derive(Clone, Debug)]
pub struct ViewSpan {
    /// The view entity.
    pub entity: EntityId,
    /// `type_name::<V>()`.
    pub type_name: &'static str,
    /// The view's element record in this frame's tree, if a tree was captured.
    pub element: Option<ElementIndex>,
    /// Nesting depth among views.
    pub depth: u16,
    /// Start offset from the beginning of the frame.
    pub start: Duration,
    /// Duration of `render()` plus the subtree's `request_layout`.
    pub duration: Duration,
    /// Whether the view rendered or was served from the view cache.
    pub outcome: ViewOutcome,
}

/// Whether a view produced a new element tree this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewOutcome {
    /// `render()` ran.
    Rendered,
    /// Prepaint and paint were reused from the previous frame.
    Cached,
}

/// A named user span (`gpui::inspector_span!`) recorded inside a frame.
#[derive(Clone, Debug)]
pub struct UserSpan {
    /// Span label.
    pub name: SharedString,
    /// Where the span was opened.
    pub site: &'static Location<'static>,
    /// Nesting depth among user spans.
    pub depth: u16,
    /// Start offset from the beginning of the frame.
    pub start: Duration,
    /// Duration.
    pub duration: Duration,
}

/// Why a frame was drawn.
#[derive(Clone, Debug)]
pub struct RenderCause {
    /// What happened.
    pub kind: CauseKind,
    /// Call site that triggered it, when known (`#[track_caller]`).
    pub site: Option<&'static Location<'static>>,
    /// How long before the frame began this happened.
    pub before_frame: Duration,
    /// True when the inspector itself caused it (its own views, input to its dock).
    pub from_inspector: bool,
}

/// The kinds of invalidation the capture distinguishes.
#[derive(Clone, Debug, PartialEq)]
pub enum CauseKind {
    /// `cx.notify()` on an entity that a rendered view depends on.
    Notify {
        /// Notified entity.
        entity: EntityId,
        /// Its type, if known.
        type_name: Option<&'static str>,
    },
    /// `window.refresh()`.
    Refresh,
    /// The window was resized or its scale factor changed.
    Resize,
    /// Window appearance, activation or focus changed.
    WindowState,
    /// An input event was dispatched.
    Input {
        /// Event kind, e.g. `MouseDown`.
        event: &'static str,
    },
    /// An animation or `request_animation_frame`.
    Animation,
    /// First frame of the window or of the capture.
    Initial,
}

/// Scene primitive counts for one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    /// Quads.
    pub quads: u32,
    /// Shadows.
    pub shadows: u32,
    /// Paths.
    pub paths: u32,
    /// Underlines.
    pub underlines: u32,
    /// Monochrome (glyph) sprites.
    pub monochrome_sprites: u32,
    /// Subpixel (glyph) sprites.
    pub subpixel_sprites: u32,
    /// Polychrome (image, emoji) sprites.
    pub polychrome_sprites: u32,
    /// Platform surfaces.
    pub surfaces: u32,
    /// Backdrop filters.
    pub backdrop_filters: u32,
    /// Paint operations, including layer and clip pushes.
    pub paint_operations: u32,
}

impl SceneStats {
    /// Total primitive count.
    pub fn primitives(&self) -> u32 {
        self.quads
            + self.shadows
            + self.paths
            + self.underlines
            + self.monochrome_sprites
            + self.subpixel_sprites
            + self.polychrome_sprites
            + self.surfaces
            + self.backdrop_filters
    }
}

/// Everything captured for one drawn frame.
#[derive(Clone, Debug)]
pub struct FrameRecord {
    /// Monotonic frame number for the window.
    pub id: u64,
    /// Start time as an offset from the capture epoch.
    pub start: Duration,
    /// Viewport size when drawn (excluding the dock).
    pub viewport: Size<Pixels>,
    /// Phase durations.
    pub timings: PhaseTimings,
    /// Why this frame was drawn.
    pub causes: Vec<RenderCause>,
    /// Views rendered or reused, in render order.
    pub views: Vec<ViewSpan>,
    /// Named user spans.
    pub spans: Vec<UserSpan>,
    /// Scene stats.
    pub scene: SceneStats,
    /// Element count (also known when the tree was not retained).
    pub element_count: u32,
    /// The element tree, when retained for this frame.
    pub tree: Option<Arc<ElementTree>>,
    /// Input records (by sequence number) dispatched since the previous frame.
    pub input: std::ops::Range<u64>,
    /// Foreground work between the previous frame and this one, from the
    /// profiler journal. Empty unless gpui's `profiler` feature is enabled.
    pub foreground: Vec<ForegroundSlice>,
    /// True when every cause came from the inspector itself.
    pub inspector_only: bool,
}

impl FrameRecord {
    /// Views that actually rendered this frame.
    pub fn rendered_views(&self) -> impl Iterator<Item = &ViewSpan> {
        self.views
            .iter()
            .filter(|view| view.outcome == ViewOutcome::Rendered)
    }
}

/// A piece of main-thread work recorded by the profiler journal.
#[derive(Clone, Debug)]
pub struct ForegroundSlice {
    /// What ran.
    pub kind: ForegroundKind,
    /// Start offset from the capture epoch.
    pub start: Duration,
    /// How long it ran.
    pub duration: Duration,
}

/// The kinds of main-thread work the journal distinguishes.
#[derive(Clone, Debug, PartialEq)]
pub enum ForegroundKind {
    /// One poll of a foreground task at least 100µs long.
    Task {
        /// Where the task was spawned.
        site: &'static Location<'static>,
    },
    /// An action handler.
    Action {
        /// `Action::name()`.
        name: &'static str,
    },
    /// Platform input dispatch.
    Input {
        /// Event kind, e.g. `MouseDown`.
        kind: &'static str,
    },
    /// Frame submission to the platform.
    Present,
    /// Task polls under 100µs, folded together.
    SmallPolls {
        /// How many polls were folded.
        count: u32,
    },
}

/// Input event category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputKind {
    /// Key down.
    KeyDown,
    /// Key up.
    KeyUp,
    /// Modifier change.
    Modifiers,
    /// Mouse button down.
    MouseDown,
    /// Mouse button up.
    MouseUp,
    /// Mouse moved (consecutive moves are coalesced).
    MouseMove,
    /// Mouse left the window.
    MouseExit,
    /// Scroll wheel / trackpad scroll.
    Scroll,
    /// Pinch or other gesture.
    Gesture,
    /// Touch event.
    Touch,
    /// File drag and drop.
    FileDrop,
    /// An action dispatched directly (not from a keystroke).
    Action,
}

/// An action dispatched while handling an input record.
#[derive(Clone, Debug)]
pub struct ActionRecord {
    /// `Action::name()`.
    pub name: &'static str,
    /// Whether a listener handled it: an action listener ran and didn't call
    /// `cx.propagate()` (bubble-phase listeners stop propagation by default).
    pub handled: bool,
    /// The keystrokes of the binding that produced it, e.g. `ctrl-k ctrl-t`,
    /// when dispatched from a binding.
    pub keystrokes: Option<SharedString>,
    /// The context predicate of that binding as written, e.g. `Editor && mode == full`.
    /// `None` for bindings without one and for actions not dispatched from a binding.
    pub context: Option<SharedString>,
}

/// One dispatched input event.
#[derive(Clone, Debug)]
pub struct InputRecord {
    /// Monotonic sequence number.
    pub seq: u64,
    /// Offset from the capture epoch.
    pub at: Duration,
    /// Frame drawn after this input (the frame whose `input` range contains `seq`),
    /// filled in when that frame completes.
    pub frame: Option<u64>,
    /// Category.
    pub kind: InputKind,
    /// Human-readable summary, e.g. `left ×1 at (120, 44)` or `cmd-k`.
    pub detail: SharedString,
    /// Pointer position, for pointer events.
    pub position: Option<Point<Pixels>>,
    /// Keystroke, for key events.
    pub keystroke: Option<Keystroke>,
    /// Elements under the pointer, topmost first, from the latest captured
    /// element tree (at most 8; empty before a tree was captured).
    pub hit_path: SmallVec<[ElementKey; 8]>,
    /// Key context stack of the dispatch target, outermost first: the focused
    /// element for key events, the target element for [`InputKind::Action`].
    pub context_stack: SmallVec<[KeyContext; 4]>,
    /// Actions dispatched while handling this event, in the order they completed.
    pub actions: SmallVec<[ActionRecord; 1]>,
    /// Whether the event was handled, as reported back to the platform: a
    /// listener stopped propagation (action handlers do by default, and the
    /// keymap does while it holds a pending multi-stroke prefix) or called
    /// `window.prevent_default()`. For [`InputKind::Action`], whether the
    /// action was handled.
    pub handled: bool,
    /// Handling time; for coalesced moves, the total over all merged events.
    pub duration: Duration,
    /// Whether handling it invalidated the window (a view notified or the
    /// window refreshed before dispatch returned).
    pub caused_redraw: bool,
    /// Number of events merged into this record (mouse moves).
    pub coalesced: u32,
    /// The event was consumed by the inspector: pointer events inside its dock
    /// or while picking, key events and actions targeting an element inside it.
    pub inspector: bool,
}

/// A live entity, as reported by [`crate::App::inspector_entities`].
#[derive(Clone, Debug)]
pub struct EntityInfo {
    /// Entity id.
    pub id: EntityId,
    /// `type_name::<T>()`.
    pub type_name: &'static str,
    /// Strong handle count.
    pub strong_count: usize,
    /// Whether the entity is drawn as a view (an `Entity<V: Render>` used as an
    /// element, cached or not) in the latest rendered frame of an open window.
    pub is_view: bool,
    /// Registered `observe` callbacks watching it.
    pub observers: usize,
    /// Registered `subscribe` callbacks listening to it.
    pub subscribers: usize,
    /// `notify` calls since capture started.
    pub notifies: u64,
    /// Where the most recent `notify` came from.
    pub last_notify_site: Option<&'static Location<'static>>,
}

/// How the keymap resolves keystrokes against the focused context stack,
/// without dispatching anything. Produced by
/// [`crate::Window::inspector_resolve_keystrokes`].
#[derive(Clone, Debug, Default)]
pub struct KeyResolution {
    /// The keystrokes that were resolved, e.g. `[cmd-k, cmd-t]`.
    pub keystrokes: SmallVec<[Keystroke; 2]>,
    /// The focused key context stack, outermost first.
    pub context_stack: Vec<KeyContext>,
    /// Every binding whose keystrokes match or start with the input: the
    /// winner, if any, first; then the other complete matches, the longer
    /// bindings and the context mismatches, each in the keymap's precedence
    /// order.
    pub candidates: Vec<BindingCandidate>,
}

impl KeyResolution {
    /// The binding that would run, if any.
    pub fn winner(&self) -> Option<&BindingCandidate> {
        self.candidates
            .iter()
            .find(|candidate| candidate.verdict == BindingVerdict::Wins)
    }

    /// Whether GPUI would wait for more keystrokes before running anything.
    /// The winner still runs if the next keystroke doesn't continue a pending
    /// binding, or after a one second timeout.
    pub fn is_pending(&self) -> bool {
        self.candidates
            .iter()
            .any(|candidate| candidate.verdict == BindingVerdict::Pending)
    }
}

/// One binding considered by a [`KeyResolution`].
#[derive(Clone, Debug)]
pub struct BindingCandidate {
    /// The binding's full keystroke sequence, e.g. `cmd-k cmd-t`.
    pub keystrokes: SharedString,
    /// Name of the bound action (`zed::NoAction` style names included).
    pub action: SharedString,
    /// The binding's context predicate as written, if any.
    pub predicate: Option<SharedString>,
    /// Depth in the context stack at which the predicate matched
    /// (0 = innermost), when it matched.
    pub matched_depth: Option<usize>,
    /// Why it wins or loses.
    pub verdict: BindingVerdict,
}

/// The outcome for one [`BindingCandidate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingVerdict {
    /// This binding's action would be dispatched.
    Wins,
    /// Matches, but a binding at a deeper context or later in the keymap takes
    /// precedence: the winner, a `NoAction` / `Unbind` binding that disables
    /// this one, or, for a binding the input is a prefix of, the complete match
    /// that makes GPUI stop waiting for more keys.
    Shadowed {
        /// Index into [`KeyResolution::candidates`] of the binding that takes precedence.
        by: usize,
    },
    /// The keystrokes match but the context predicate is false here.
    ContextMismatch,
    /// Matches, but the action is `NoAction` / unbound, which disables lower bindings.
    Disabled,
    /// The input is a prefix of this binding: GPUI would wait for more keys.
    Pending,
    /// Matches and outranks the winner, but nothing on the focus path (and no
    /// global listener) handles its action, so dispatch falls through to the
    /// next binding.
    Unhandled,
}

bitflags::bitflags! {
    /// Interaction states the inspector can force on an element so its
    /// `.hover()`, `.active()` or `.focus()` refinements can be styled live.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct ForcedStates: u8 {
        /// Treat as hovered (and group-hovered).
        const HOVER = 1 << 0;
        /// Treat as pressed.
        const ACTIVE = 1 << 1;
        /// Treat as focused.
        const FOCUS = 1 << 2;
        /// Treat as focus-visible.
        const FOCUS_VISIBLE = 1 << 3;
    }
}

/// Events the [`crate::Inspector`] entity emits for the inspector UI.
#[derive(Clone, Debug, PartialEq)]
pub enum InspectorEvent {
    /// The pointer moved over a new element while picking.
    PickHovered(Option<ElementKey>),
    /// An element was picked with a click.
    Picked(ElementKey),
    /// Picking ended without a selection (escape, or picking was toggled off).
    PickCancelled,
}
