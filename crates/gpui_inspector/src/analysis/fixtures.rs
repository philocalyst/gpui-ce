//! Builders for the fabricated captures the analysis tests run on.

use gpui::{
    Bounds, EntityId, Pixels, SharedString,
    inspector::{
        ElementDetails, ElementFlags, ElementIndex, ElementKey, ElementKind, ElementRecord,
        ElementTree, FrameRecord, InputKind, InputRecord, LayoutFacts, PathKey, PhaseTimings,
        SceneStats, ViewOutcome, ViewSpan,
    },
    point, px, size,
};
use std::time::Duration;

/// The type name fixtures give plain elements.
pub const DIV: &str = "gpui::elements::div::Div";

/// Milliseconds, rounded to the nanosecond.
pub fn ms(millis: f64) -> Duration {
    Duration::from_nanos((millis * 1e6).round() as u64)
}

/// Microseconds.
pub fn us(micros: u64) -> Duration {
    Duration::from_micros(micros)
}

/// An entity id for tests.
pub fn entity(id: u64) -> EntityId {
    EntityId::from(id)
}

/// A frame drawn at `start` that took `total`, all of it attributed to render.
pub fn frame(id: u64, start: Duration, total: Duration) -> FrameRecord {
    FrameRecord {
        id,
        start,
        viewport: size(px(800.), px(600.)),
        timings: PhaseTimings {
            render: total,
            total,
            ..PhaseTimings::default()
        },
        causes: Vec::new(),
        views: Vec::new(),
        spans: Vec::new(),
        scene: SceneStats::default(),
        element_count: 0,
        tree: None,
        input: 0..0,
        foreground: Vec::new(),
        inspector_only: false,
    }
}

/// `count` frames, one every `interval` from zero, each taking `total`.
pub fn frames_every(count: u64, interval: Duration, total: Duration) -> Vec<FrameRecord> {
    (0..count)
        .map(|id| frame(id, interval * id as u32, total))
        .collect()
}

/// A rendered view span.
pub fn view(
    entity_id: u64,
    type_name: &'static str,
    depth: u16,
    start: Duration,
    duration: Duration,
) -> ViewSpan {
    ViewSpan {
        entity: entity(entity_id),
        type_name,
        element: None,
        depth,
        start,
        duration,
        outcome: ViewOutcome::Rendered,
    }
}

/// A view span served from the view cache.
pub fn cached_view(
    entity_id: u64,
    type_name: &'static str,
    depth: u16,
    start: Duration,
    duration: Duration,
) -> ViewSpan {
    ViewSpan {
        outcome: ViewOutcome::Cached,
        ..view(entity_id, type_name, depth, start, duration)
    }
}

/// A view span of a frame that replayed the app: drawn by none of its own
/// work, so it takes no time.
pub fn replayed_view(
    entity_id: u64,
    type_name: &'static str,
    depth: u16,
    start: Duration,
) -> ViewSpan {
    ViewSpan {
        outcome: ViewOutcome::Replayed,
        ..view(entity_id, type_name, depth, start, Duration::ZERO)
    }
}

/// An app input record at `at`, drawn by `frame`.
pub fn input(seq: u64, at: Duration, kind: InputKind, frame: Option<u64>) -> InputRecord {
    InputRecord {
        seq,
        at,
        frame,
        kind,
        detail: SharedString::from(format!("{kind:?}")),
        position: None,
        keystroke: None,
        hit_path: Default::default(),
        context_stack: Default::default(),
        actions: Default::default(),
        handled: true,
        duration: Duration::ZERO,
        caused_redraw: true,
        coalesced: 1,
        inspector: false,
    }
}

/// Bounds from plain numbers.
pub fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
}

/// Builds an [`ElementTree`] top-down. Every element gets a distinct key
/// (`PathKey(index)`), visible bounds equal to its bounds, and paint order
/// equal to its index.
#[derive(Default)]
pub struct TreeBuilder {
    elements: Vec<ElementRecord>,
}

impl TreeBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a root element.
    pub fn root(&mut self, bounds: Bounds<Pixels>) -> ElementIndex {
        self.push(None, bounds)
    }

    /// Adds a child of `parent`.
    pub fn child(&mut self, parent: ElementIndex, bounds: Bounds<Pixels>) -> ElementIndex {
        self.push(Some(parent), bounds)
    }

    fn push(&mut self, parent: Option<ElementIndex>, bounds: Bounds<Pixels>) -> ElementIndex {
        let ix = self.elements.len() as ElementIndex;
        let depth = parent.map_or(0, |parent| self.elements[parent as usize].depth + 1);
        self.elements.push(ElementRecord {
            key: Some(ElementKey {
                path: PathKey(ix),
                instance: 0,
            }),
            parent,
            depth,
            kind: ElementKind::Element { type_name: DIV },
            id: None,
            bounds,
            visible_bounds: Some(bounds),
            paint_order: ix,
            primitives: 0,
            flags: ElementFlags::empty(),
            details: None,
        });
        ix
    }

    /// The record at `ix`, for arbitrary tweaks.
    pub fn record(&mut self, ix: ElementIndex) -> &mut ElementRecord {
        &mut self.elements[ix as usize]
    }

    /// The details of `ix`, created on first use.
    pub fn details(&mut self, ix: ElementIndex) -> &mut ElementDetails {
        self.record(ix).details.get_or_insert_with(Default::default)
    }

    /// The layout facts of `ix`, created on first use (flex row, shrink 1).
    pub fn layout(&mut self, ix: ElementIndex) -> &mut LayoutFacts {
        self.details(ix).layout.get_or_insert_with(|| LayoutFacts {
            display: "flex".into(),
            flex_shrink: 1.0,
            ..LayoutFacts::default()
        })
    }

    /// Finishes the tree, filling in the child lists.
    pub fn build(self) -> ElementTree {
        let mut tree = ElementTree {
            frame: 0,
            elements: self.elements,
            ..ElementTree::default()
        };
        tree.rebuild_children();
        tree
    }
}
