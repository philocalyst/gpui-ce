//! Engine capture tests: every fact the inspector shows is checked against
//! what the window actually drew.

use super::*;
use crate::{
    self as gpui, AnyElement, AnyView, App, AppContext as _, Bounds, Context, Entity, EntityId,
    FocusHandle, Hsla, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _, Pixels,
    Point, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, TestAppContext, TouchPhase,
    VisualTestContext, Window, WindowControlArea, blue, deferred, div, point,
    proptest::{collection::vec, prelude::*},
    px, red, size, uniform_list,
};
use std::{cell::RefCell, panic::Location, rc::Rc, sync::Arc, time::Duration};

// Harness.

fn open(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.toggle_inspector(cx));
}

fn close(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.toggle_inspector(cx));
}

fn read<R>(cx: &mut VisualTestContext, f: impl FnOnce(&InspectorCapture) -> R) -> R {
    cx.update(|window, _| f(window.inspector_capture().expect("the inspector is open")))
}

fn write<R>(cx: &mut VisualTestContext, f: impl FnOnce(&mut InspectorCapture) -> R) -> R {
    cx.update(|window, _| {
        f(window
            .inspector_capture_mut()
            .expect("the inspector is open"))
    })
}

/// Draws a frame after changing the capture (as the inspector UI does by
/// notifying its own view).
fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
}

fn latest_frame(cx: &mut VisualTestContext) -> FrameRecord {
    read(cx, |capture| {
        capture.latest_frame().expect("a frame").clone()
    })
}

fn latest_tree(cx: &mut VisualTestContext) -> Arc<ElementTree> {
    read(cx, |capture| capture.latest_tree().expect("a tree").clone())
}

fn app_bounds(cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.update(|window, _| window.app_bounds())
}

fn find_id(tree: &ElementTree, id: &str) -> ElementIndex {
    let matches = tree
        .elements
        .iter()
        .enumerate()
        .filter(|(_, record)| record.id.as_deref() == Some(id))
        .map(|(ix, _)| ix as ElementIndex)
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "expected exactly one element #{id}");
    matches[0]
}

fn record<'a>(tree: &'a ElementTree, id: &str) -> &'a ElementRecord {
    tree.get(find_id(tree, id)).unwrap()
}

fn key_of(tree: &ElementTree, id: &str) -> ElementKey {
    record(tree, id).key.expect("the element has a key")
}

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
}

fn events(cx: &mut VisualTestContext) -> Rc<RefCell<Vec<InspectorEvent>>> {
    let events = Rc::new(RefCell::new(Vec::new()));
    cx.update(|window, cx| {
        let inspector = window.inspector_entity().expect("the inspector is open");
        let events = events.clone();
        cx.subscribe(&inspector, move |_, event: &InspectorEvent, _| {
            events.borrow_mut().push(event.clone())
        })
        .detach();
    });
    events
}

#[track_caller]
fn notify_here<T: 'static>(cx: &mut Context<T>) -> &'static Location<'static> {
    cx.notify();
    Location::caller()
}

#[track_caller]
fn refresh_here(window: &mut Window) -> &'static Location<'static> {
    window.refresh();
    Location::caller()
}

// Views.

/// `#root > #outer > (#inner, #sibling)`, with `#outer` placed absolutely.
struct Nested {
    clicks: Rc<RefCell<usize>>,
    outer_width: Pixels,
}

impl Nested {
    fn new() -> Self {
        Nested {
            clicks: Rc::default(),
            outer_width: px(200.),
        }
    }
}

impl Render for Nested {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div().id("root").size_full().child(
            div()
                .id("outer")
                .flex()
                .absolute()
                .left(px(10.))
                .top(px(20.))
                .w(self.outer_width)
                .h(px(100.))
                .child(
                    div()
                        .id("inner")
                        .w(px(50.))
                        .h(px(40.))
                        .bg(red())
                        .on_click(move |_, _, _| *clicks.borrow_mut() += 1),
                )
                .child(div().id("sibling").w(px(30.)).h(px(10.)).bg(blue())),
        )
    }
}

struct Leaf;

impl Render for Leaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("leaf-root")
            .size_full()
            .bg(blue())
            .child(div().id("leaf").w(px(10.)).h(px(10.)).bg(red()))
    }
}

#[derive(IntoElement)]
struct Badge;

impl crate::RenderOnce for Badge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().id("badge").w(px(8.)).h(px(8.))
    }
}

/// A view embedding a child view (cached or not) and a component.
struct Parent {
    child: Entity<Leaf>,
    cached: bool,
}

impl Render for Parent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let child: AnyElement = if self.cached {
            self.child
                .clone()
                .cached(StyleRefinement::default().w(px(300.)).h(px(200.)))
                .into_any_element()
        } else {
            div()
                .w(px(300.))
                .h(px(200.))
                .child(self.child.clone())
                .into_any_element()
        };
        div()
            .id("parent")
            .size_full()
            .flex()
            .flex_col()
            .child(child)
            .child(Badge)
    }
}

fn parent(cx: &mut TestAppContext, cached: bool) -> (Entity<Parent>, &mut VisualTestContext) {
    cx.add_window_view(move |_, cx| Parent {
        child: cx.new(|_| Leaf),
        cached,
    })
}

/// A view that renders whatever its closure builds.
struct Scene(Box<dyn Fn(&mut Window, &mut App) -> AnyElement>);

impl Render for Scene {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.0)(window, cx)
    }
}

fn scene<E: IntoElement>(
    cx: &mut TestAppContext,
    build: impl Fn() -> E + 'static,
) -> (Entity<Scene>, &mut VisualTestContext) {
    cx.add_window_view(move |_, _| Scene(Box::new(move |_, _| build().into_any_element())))
}

// Element tree.

#[gpui::test]
fn nested_divs_have_exact_links_depths_kinds_and_bounds(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let tree = latest_tree(cx);

    assert_eq!(tree.roots.len(), 1, "one root: the window's root view");
    let root_view = tree.roots[0];
    let root_record = tree.get(root_view).unwrap();
    assert_eq!(
        root_record.kind,
        ElementKind::View {
            entity: view.entity_id(),
            type_name: std::any::type_name::<Nested>(),
        }
    );
    assert_eq!(root_record.depth, 0);
    assert_eq!(root_record.bounds, app_bounds(cx));

    let root = find_id(&tree, "root");
    let outer = find_id(&tree, "outer");
    let inner = find_id(&tree, "inner");
    let sibling = find_id(&tree, "sibling");
    assert_eq!(tree.get(root).unwrap().parent, Some(root_view));
    assert_eq!(tree.get(outer).unwrap().parent, Some(root));
    assert_eq!(tree.get(inner).unwrap().parent, Some(outer));
    assert_eq!(tree.get(sibling).unwrap().parent, Some(outer));
    assert_eq!(tree.children(outer), &[inner, sibling]);
    assert_eq!(tree.children(root_view), &[root]);

    for (id, depth) in [("root", 1), ("outer", 2), ("inner", 3), ("sibling", 3)] {
        let record = record(&tree, id);
        assert_eq!(record.depth, depth, "#{id}");
        assert_eq!(
            record.kind,
            ElementKind::Element {
                type_name: std::any::type_name::<crate::Div>(),
            },
            "#{id}"
        );
        assert!(record.key.is_some(), "#{id} has a key");
        assert!(!record.flags.contains(ElementFlags::CLIPPED));
    }
    assert_eq!(record(&tree, "root").bounds, app_bounds(cx));
    assert_eq!(record(&tree, "outer").bounds, bounds(10., 20., 200., 100.));
    assert_eq!(record(&tree, "inner").bounds, bounds(10., 20., 50., 40.));
    assert_eq!(record(&tree, "sibling").bounds, bounds(60., 20., 30., 10.));
    assert_eq!(
        record(&tree, "inner").visible_bounds,
        Some(bounds(10., 20., 50., 40.))
    );

    // Keys are stable across frames and point at the construction site.
    let inner_key = key_of(&tree, "inner");
    redraw(cx);
    assert_eq!(key_of(&latest_tree(cx), "inner"), inner_key);
    let source = read(cx, |capture| {
        capture.path_info(inner_key.path).unwrap().source
    });
    assert!(source.file().ends_with("inspector/tests.rs"));
    assert_ne!(inner_key, key_of(&tree, "sibling"));
    assert!(
        record(&tree, "inner")
            .flags
            .contains(ElementFlags::CLICKABLE | ElementFlags::HITBOX)
    );
}

#[gpui::test]
fn views_and_components_are_boundaries(cx: &mut TestAppContext) {
    let (view, cx) = parent(cx, false);
    open(cx);
    let tree = latest_tree(cx);
    let child = view.read_with(cx, |parent, _| parent.child.entity_id());

    let leaf_view = tree
        .elements
        .iter()
        .position(
            |record| matches!(record.kind, ElementKind::View { entity, .. } if entity == child),
        )
        .expect("the child view is recorded") as ElementIndex;
    assert_eq!(
        tree.get(leaf_view).unwrap().kind,
        ElementKind::View {
            entity: child,
            type_name: std::any::type_name::<Leaf>(),
        }
    );
    assert_eq!(
        tree.get(find_id(&tree, "leaf-root")).unwrap().parent,
        Some(leaf_view)
    );
    assert_eq!(tree.owning_view(find_id(&tree, "leaf")), Some(leaf_view));
    let view_key = tree.get(leaf_view).unwrap().key.expect("views have keys");
    let view_site = read(cx, |capture| {
        capture.path_info(view_key.path).unwrap().source
    });
    assert!(
        view_site.file().ends_with("inspector/tests.rs"),
        "a view is sited where it is embedded, not in gpui: {view_site}"
    );

    let badge = find_id(&tree, "badge");
    let component = tree.get(badge).unwrap().parent.unwrap();
    assert_eq!(
        tree.get(component).unwrap().kind,
        ElementKind::Component {
            type_name: std::any::type_name::<Badge>(),
        }
    );
    assert_eq!(tree.get(component).unwrap().kind.short_name(), "Badge");

    // Both views rendered; the component is not a view.
    let frame = latest_frame(cx);
    let views: Vec<_> = frame
        .views
        .iter()
        .map(|span| (span.entity, span.outcome))
        .collect();
    assert_eq!(
        views,
        vec![
            (view.entity_id(), ViewOutcome::Rendered),
            (child, ViewOutcome::Rendered)
        ]
    );
    assert_eq!(frame.views[0].depth, 0);
    assert_eq!(
        frame.views[1].depth, 1,
        "the child renders inside the parent"
    );
    assert_eq!(frame.views[1].element, Some(leaf_view));
    assert!(frame.views[1].start >= frame.views[0].start);
    assert!(
        frame.views[1].start + frame.views[1].duration
            <= frame.views[0].start + frame.views[0].duration
    );
}

#[gpui::test]
fn reused_cached_views_keep_their_subtree(cx: &mut TestAppContext) {
    let (view, cx) = parent(cx, true);
    open(cx);
    let child = view.read_with(cx, |parent, _| parent.child.clone());
    let fresh = latest_tree(cx);
    let fresh_frame = latest_frame(cx);
    assert!(
        fresh_frame
            .views
            .iter()
            .any(|span| span.entity == child.entity_id() && span.outcome == ViewOutcome::Rendered)
    );

    // Only the parent re-renders: the child's paint is reused.
    view.update(cx, |_, cx| cx.notify());
    let reused = latest_tree(cx);
    let frame = latest_frame(cx);
    let child_span = frame
        .views
        .iter()
        .find(|span| span.entity == child.entity_id())
        .expect("the reused view has a span");
    assert_eq!(child_span.outcome, ViewOutcome::Cached);
    assert_eq!(
        reused.elements.len(),
        fresh.elements.len(),
        "the tree stays complete"
    );
    for id in ["leaf-root", "leaf"] {
        let (before, after) = (record(&fresh, id), record(&reused, id));
        assert!(
            after.flags.contains(ElementFlags::REUSED),
            "#{id} is reused"
        );
        assert!(!before.flags.contains(ElementFlags::REUSED));
        assert_eq!(after.bounds, before.bounds, "#{id}");
        assert_eq!(after.key, before.key, "#{id}");
        assert_eq!(after.primitives, before.primitives, "#{id}");
        assert!(after.paint_order > 0);
    }
    let view_record = child_span.element.unwrap();
    assert_eq!(
        reused.get(find_id(&reused, "leaf-root")).unwrap().parent,
        Some(view_record)
    );
    assert!(
        !reused
            .get(view_record)
            .unwrap()
            .flags
            .contains(ElementFlags::REUSED)
    );
    let mut orders: Vec<u32> = reused
        .elements
        .iter()
        .map(|record| record.paint_order)
        .collect();
    orders.sort();
    orders.dedup();
    assert_eq!(
        orders.len(),
        reused.elements.len(),
        "paint orders are unique"
    );
    assert!(record(&reused, "leaf").paint_order > record(&reused, "leaf-root").paint_order);
    assert_eq!(
        record(&reused, "leaf").primitives + 1,
        reused.get(view_record).unwrap().primitives,
        "the view's paint (the root's quad and the leaf's) is replayed"
    );

    // Notifying the child renders it again.
    child.update(cx, |_, cx| cx.notify());
    let rendered = latest_tree(cx);
    assert!(
        !record(&rendered, "leaf")
            .flags
            .contains(ElementFlags::REUSED)
    );
}

/// A cached view that embeds another cached view.
struct Outer {
    inner: Entity<Leaf>,
}

impl Render for Outer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("outer-root").size_full().child(
            self.inner
                .clone()
                .cached(StyleRefinement::default().w(px(300.)).h(px(200.))),
        )
    }
}

struct Shell {
    outer: Entity<Outer>,
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("shell").size_full().child(
            self.outer
                .clone()
                .cached(StyleRefinement::default().w(px(400.)).h(px(300.))),
        )
    }
}

#[gpui::test]
fn nested_cached_views_replay_with_their_parent(cx: &mut TestAppContext) {
    let (shell, cx) = cx.add_window_view(|_, cx| Shell {
        outer: cx.new(|cx| Outer {
            inner: cx.new(|_| Leaf),
        }),
    });
    open(cx);
    let outer = shell.read_with(cx, |shell, _| shell.outer.clone());
    let inner = outer.read_with(cx, |outer, _| outer.inner.entity_id());
    let fresh = latest_tree(cx);
    let outcome = |frame: &FrameRecord, entity: EntityId| {
        frame
            .views
            .iter()
            .find(|span| span.entity == entity)
            .map(|span| span.outcome)
    };

    // The shell renders; the outer view (and the inner one within it) replay.
    shell.update(cx, |_, cx| cx.notify());
    let frame = latest_frame(cx);
    assert_eq!(
        outcome(&frame, outer.entity_id()),
        Some(ViewOutcome::Cached)
    );
    assert_eq!(
        outcome(&frame, inner),
        None,
        "replayed inside the outer view"
    );
    let replayed = latest_tree(cx);
    assert_eq!(replayed.elements.len(), fresh.elements.len());
    for id in ["outer-root", "leaf-root", "leaf"] {
        assert!(
            record(&replayed, id).flags.contains(ElementFlags::REUSED),
            "#{id}"
        );
        assert_eq!(
            record(&replayed, id).bounds,
            record(&fresh, id).bounds,
            "#{id}"
        );
    }

    // A cached view that renders again renders the cached views inside it.
    outer.update(cx, |_, cx| cx.notify());
    let frame = latest_frame(cx);
    assert_eq!(
        outcome(&frame, outer.entity_id()),
        Some(ViewOutcome::Rendered)
    );
    assert_eq!(outcome(&frame, inner), Some(ViewOutcome::Rendered));
    let tree = latest_tree(cx);
    assert!(
        tree.elements
            .iter()
            .all(|record| !record.flags.contains(ElementFlags::REUSED))
    );
    assert_eq!(tree.elements.len(), fresh.elements.len());
}

#[gpui::test]
fn clipped_and_scrolled_content(cx: &mut TestAppContext) {
    let scroll = ScrollHandle::new();
    scroll.set_offset(point(px(0.), px(-30.)));
    let (_, cx) = scene(cx, {
        move || {
            div()
                .flex()
                .child(
                    div()
                        .id("clip")
                        .relative()
                        .w(px(100.))
                        .h(px(100.))
                        .overflow_hidden()
                        .child(
                            div()
                                .id("partial")
                                .absolute()
                                .left(px(50.))
                                .w(px(100.))
                                .h(px(20.)),
                        )
                        .child(
                            div()
                                .id("outside")
                                .absolute()
                                .left(px(200.))
                                .w(px(10.))
                                .h(px(10.)),
                        ),
                )
                .child(
                    div().id("loose").relative().w(px(100.)).h(px(100.)).child(
                        div()
                            .id("overflowing")
                            .absolute()
                            .left(px(60.))
                            .w(px(100.))
                            .h(px(20.)),
                    ),
                )
                .child(
                    div()
                        .id("scroller")
                        .w(px(100.))
                        .h(px(50.))
                        .overflow_y_scroll()
                        .track_scroll(&scroll)
                        .child(div().id("tall").w(px(100.)).h(px(200.))),
                )
        }
    });
    open(cx);
    let tree = latest_tree(cx);

    let partial = record(&tree, "partial");
    assert_eq!(partial.bounds, bounds(50., 0., 100., 20.));
    assert_eq!(partial.visible_bounds, Some(bounds(50., 0., 50., 20.)));
    assert!(
        !partial.flags.contains(ElementFlags::OVERFLOWS_PARENT),
        "clipped by its parent"
    );

    let outside = record(&tree, "outside");
    assert_eq!(outside.visible_bounds, None);
    assert!(outside.flags.contains(ElementFlags::CLIPPED));
    assert!(
        tree.hit_test(point(px(205.), px(5.))).iter().all(|&ix| tree
            .get(ix)
            .unwrap()
            .id
            .as_deref()
            != Some("outside"))
    );

    let overflowing = record(&tree, "overflowing");
    assert_eq!(
        overflowing.visible_bounds,
        Some(bounds(160., 0., 100., 20.))
    );
    assert!(overflowing.flags.contains(ElementFlags::OVERFLOWS_PARENT));

    let scroller = record(&tree, "scroller");
    assert!(scroller.flags.contains(ElementFlags::SCROLLABLE));
    let tall = record(&tree, "tall");
    assert_eq!(tall.bounds, bounds(200., -30., 100., 200.));
    assert_eq!(tall.visible_bounds, Some(bounds(200., 0., 100., 50.)));
    let details = scroller
        .details
        .as_ref()
        .expect("full capture reports details");
    assert_eq!(details.scroll_offset, Some(point(px(0.), px(-30.))));
    assert_eq!(details.content_size, Some(size(px(100.), px(200.))));
}

#[gpui::test]
fn deferred_elements_are_roots_painted_last(cx: &mut TestAppContext) {
    let (_, cx) = scene(cx, || {
        div().id("root").size_full().child(
            div()
                .id("anchor")
                .w(px(40.))
                .h(px(40.))
                .bg(blue())
                .child(deferred(
                    div()
                        .id("popover")
                        .absolute()
                        .w(px(80.))
                        .h(px(30.))
                        .bg(red()),
                )),
        )
    });
    open(cx);
    let tree = latest_tree(cx);
    let popover = find_id(&tree, "popover");
    let record = tree.get(popover).unwrap();
    assert_eq!(record.parent, None);
    assert_eq!(record.depth, 0);
    assert!(record.flags.contains(ElementFlags::DEFERRED));
    assert!(tree.roots.contains(&popover));
    let highest_main = tree
        .elements
        .iter()
        .filter(|record| !record.flags.contains(ElementFlags::DEFERRED))
        .map(|record| record.paint_order)
        .max()
        .unwrap();
    assert!(
        record.paint_order > highest_main,
        "deferred draws paint above the tree"
    );
    assert_eq!(tree.hit_test(point(px(5.), px(5.)))[0], popover);
}

#[gpui::test]
fn hit_test_follows_paint_order(cx: &mut TestAppContext) {
    let (_, cx) = scene(cx, || {
        div()
            .id("root")
            .size_full()
            .child(div().id("below").absolute().w(px(100.)).h(px(100.)))
            .child(
                div()
                    .id("above")
                    .absolute()
                    .left(px(50.))
                    .w(px(100.))
                    .h(px(100.))
                    .child(div().id("child").w(px(10.)).h(px(10.))),
            )
    });
    open(cx);
    let tree = latest_tree(cx);
    let ids = |position: Point<Pixels>| {
        tree.hit_test(position)
            .into_iter()
            .filter_map(|ix| tree.get(ix).unwrap().id.clone())
            .collect::<Vec<SharedString>>()
    };
    assert_eq!(
        ids(point(px(55.), px(5.))),
        ["child", "above", "below", "root"]
    );
    assert_eq!(ids(point(px(75.), px(50.))), ["above", "below", "root"]);
    assert_eq!(ids(point(px(25.), px(50.))), ["below", "root"]);
    assert!(record(&tree, "above").paint_order > record(&tree, "below").paint_order);
    assert!(record(&tree, "child").paint_order > record(&tree, "above").paint_order);
}

// Frames.

#[gpui::test]
fn primitive_counts_match_scene_stats(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let (tree, frame) = (latest_tree(cx), latest_frame(cx));
    let from_tree: u32 = tree
        .roots
        .iter()
        .map(|&root| tree.get(root).unwrap().primitives)
        .sum();
    assert_eq!(from_tree, frame.scene.primitives());
    assert_eq!(frame.scene.quads, 2);
    assert_eq!(record(&tree, "inner").primitives, 1);
    assert_eq!(record(&tree, "outer").primitives, 2);
    assert_eq!(record(&tree, "root").primitives, 2);
    assert_eq!(frame.element_count, tree.elements.len() as u32);
    let quads = cx.update(|window, _| window.painted_quads().len() as u32);
    assert_eq!(quads, frame.scene.quads);
}

#[gpui::test]
fn phase_timings_are_consistent(cx: &mut TestAppContext) {
    let (_, cx) =
        scene(cx, || {
            div().id("grid").flex().flex_wrap().children(
                (0..200).map(|ix| div().id(ix).w(px(20.)).h(px(20.)).bg(red()).child("x")),
            )
        });
    open(cx);
    redraw(cx);
    let timings = latest_frame(cx).timings;
    for (phase, duration) in [
        ("render", timings.render),
        ("layout", timings.layout),
        ("prepaint", timings.prepaint),
        ("paint", timings.paint),
        ("total", timings.total),
    ] {
        assert!(!duration.is_zero(), "{phase} is measured");
    }
    let phases = timings.render + timings.layout + timings.prepaint + timings.paint;
    assert!(phases + timings.inspector <= timings.total);
    assert_eq!(timings.app_total(), timings.total - timings.inspector);
    assert_eq!(timings.present, None, "tests draw without presenting");
}

#[gpui::test]
fn causes_explain_each_frame(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    // Frames drawn while closed are not recorded.
    view.update(cx, |_, cx| cx.notify());
    open(cx);
    let first = latest_frame(cx);
    assert_eq!(first.causes.len(), 1);
    assert_eq!(first.causes[0].kind, CauseKind::Initial);
    assert!(!first.inspector_only);

    let site = view.update(cx, |_, cx| notify_here(cx));
    let frame = latest_frame(cx);
    assert_eq!(frame.causes.len(), 1);
    let cause = &frame.causes[0];
    assert_eq!(
        cause.kind,
        CauseKind::Notify {
            entity: view.entity_id(),
            type_name: Some(std::any::type_name::<Nested>()),
        }
    );
    assert_eq!(cause.site, Some(site));
    assert!(!cause.from_inspector);
    assert!(!frame.inspector_only);

    let site = cx.update(|window, _| refresh_here(window));
    let frame = latest_frame(cx);
    assert_eq!(frame.causes.len(), 1);
    assert_eq!(frame.causes[0].kind, CauseKind::Refresh);
    assert_eq!(frame.causes[0].site, Some(site));

    cx.simulate_resize(size(px(1000.), px(700.)));
    cx.update(|_, _| {});
    let frame = latest_frame(cx);
    assert!(
        frame
            .causes
            .iter()
            .any(|cause| cause.kind == CauseKind::Resize)
    );
    assert_eq!(frame.viewport, app_bounds(cx).size);
}

#[gpui::test]
fn animation_frames_are_a_cause(cx: &mut TestAppContext) {
    struct Animated {
        site: Option<&'static Location<'static>>,
    }
    impl Render for Animated {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            if self.site.is_none() {
                self.site = Some(request_animation_frame_here(window));
            }
            div().w(px(10.)).h(px(10.))
        }
    }
    #[track_caller]
    fn request_animation_frame_here(window: &mut Window) -> &'static Location<'static> {
        window.request_animation_frame();
        Location::caller()
    }

    let (view, cx) = cx.add_window_view(|_, _| Animated { site: None });
    open(cx);
    let site = view.read_with(cx, |view, _| view.site).expect("rendered");
    // Tests deliver next-frame callbacks by hand.
    cx.update(|window, cx| window.simulate_next_frame(cx));
    let frame = latest_frame(cx);
    assert_eq!(frame.causes.len(), 1, "{:?}", frame.causes);
    assert_eq!(frame.causes[0].kind, CauseKind::Animation);
    assert_eq!(frame.causes[0].site, Some(site));
    assert!(!frame.inspector_only);
}

#[gpui::test]
fn input_that_invalidates_is_a_cause(cx: &mut TestAppContext) {
    struct Hover;
    impl Render for Hover {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("target")
                .w(px(50.))
                .h(px(50.))
                .hover(|style| style.bg(red()))
        }
    }
    let (_, cx) = cx.add_window_view(|_, _| Hover);
    open(cx);
    cx.simulate_mouse_move(point(px(10.), px(10.)), None, Modifiers::none());
    let frame = latest_frame(cx);
    assert!(
        frame.causes.iter().any(|cause| cause.kind
            == CauseKind::Input {
                event: "mouse_move"
            }
            && !cause.from_inspector),
        "{:?}",
        frame.causes
    );
}

/// The inspector's own dock view, for frames the inspector causes.
struct Dock;

impl Render for Dock {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("dock").size_full().bg(blue())
    }
}

fn dock_renderer(cx: &mut VisualTestContext) -> Rc<RefCell<Option<Entity<Dock>>>> {
    let dock = Rc::new(RefCell::new(None));
    let slot = dock.clone();
    cx.update(|_, cx| {
        cx.set_inspector_renderer(Box::new(move |inspector, _, cx| {
            let view = inspector.ui_state(|| cx.new(|_| Dock)).clone();
            *slot.borrow_mut() = Some(view.clone());
            view.into_any_element()
        }))
    });
    dock
}

#[gpui::test]
fn frames_caused_only_by_the_inspector_are_flagged(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    let dock = dock_renderer(cx);
    open(cx);
    let generation = read(cx, |capture| capture.generation());
    let dock = dock.borrow().clone().expect("the dock rendered");

    dock.update(cx, |_, cx| cx.notify());
    let frame = latest_frame(cx);
    assert!(frame.inspector_only, "{:?}", frame.causes);
    assert!(frame.causes.iter().all(|cause| cause.from_inspector));
    assert_eq!(read(cx, |capture| capture.generation()), generation);
    assert!(
        latest_tree(cx)
            .elements
            .iter()
            .all(|record| record.id.as_deref() != Some("dock")),
        "the inspector's own root is never recorded"
    );
    assert!(!frame.timings.inspector.is_zero());

    // Restyling the app from the inspector is inspector-caused, but the
    // restyled tree is new data for the inspector's UI.
    let key = key_of(&latest_tree(cx), "inner");
    write(cx, |capture| {
        capture.set_override(key.path, Some(StyleRefinement::default().w(px(70.))))
    });
    dock.update(cx, |_, cx| cx.notify());
    assert!(latest_frame(cx).inspector_only);
    assert_eq!(record(&latest_tree(cx), "inner").bounds.size.width, px(70.));
    let restyled = read(cx, |capture| capture.generation());
    assert_eq!(restyled, generation + 1);

    view.update(cx, |_, cx| cx.notify());
    assert!(!latest_frame(cx).inspector_only);
    assert_eq!(read(cx, |capture| capture.generation()), restyled + 1);
}

#[gpui::test]
fn present_time_is_recorded_when_the_frame_is_presented(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    assert_eq!(latest_frame(cx).timings.present, None);
    let handle = cx.update(|window, _| window.window_handle());
    cx.test_window(handle)
        .simulate_frame_request(crate::RequestFrameOptions::default());
    let present = latest_frame(cx).timings.present;
    assert!(present.is_some(), "the platform presented the frame");

    // Frames that are not drawn again are not presented again.
    cx.test_window(handle)
        .simulate_frame_request(crate::RequestFrameOptions::default());
    assert_eq!(latest_frame(cx).timings.present, present);
}

#[gpui::test]
fn notify_stats_count_per_entity(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let mut site = None;
    for _ in 0..3 {
        site = Some(view.update(cx, |_, cx| notify_here(cx)));
    }
    let stats = read(cx, |capture| {
        capture.notify_stats()[&view.entity_id()].clone()
    });
    assert_eq!(stats.total, 3);
    assert_eq!(stats.buckets.iter().sum::<u32>(), 3);
    assert!(stats.buckets.len() <= causes::NOTIFY_BUCKETS);
    assert_eq!(stats.last_site, site);
}

#[gpui::test]
fn user_spans_nest_within_the_frame(cx: &mut TestAppContext) {
    struct Spanned;
    impl Render for Spanned {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            crate::inspector_span!("outer");
            let inner = crate::inspector::span("inner");
            let value = crate::inspector_span!("value", 40 + 2);
            drop(inner);
            div().w(px(value as f32))
        }
    }
    // Spans opened while no inspector records are ignored.
    drop(crate::inspector::span("ignored"));
    let (_, cx) = cx.add_window_view(|_, _| Spanned);
    open(cx);
    let spans = latest_frame(cx).spans;
    let names: Vec<_> = spans
        .iter()
        .map(|span| (span.name.as_ref(), span.depth))
        .collect();
    assert_eq!(names, [("outer", 0), ("inner", 1), ("value", 2)]);
    let (outer, inner, value) = (&spans[0], &spans[1], &spans[2]);
    assert!(inner.start >= outer.start);
    assert!(inner.start + inner.duration <= outer.start + outer.duration);
    assert!(value.start + value.duration <= inner.start + inner.duration);
    assert!(outer.site.file().ends_with("inspector/tests.rs"));
    assert!(inner.site.line() > outer.site.line());
}

#[gpui::test]
fn input_ranges_are_assigned_to_frames(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let input = |kind| InputRecord {
        seq: 0,
        at: Default::default(),
        frame: None,
        kind,
        detail: "fixture".into(),
        position: None,
        keystroke: None,
        hit_path: Default::default(),
        context_stack: Default::default(),
        actions: Default::default(),
        handled: false,
        duration: Duration::from_millis(2),
        caused_redraw: true,
        coalesced: 1,
        inspector: false,
    };
    write(cx, |capture| {
        capture.push_input_for_test(input(InputKind::MouseDown));
        capture.push_input_for_test(input(InputKind::MouseUp));
    });
    redraw(cx);
    let frame = latest_frame(cx);
    assert_eq!(frame.input, 0..2);
    assert_eq!(frame.timings.input, Duration::from_millis(4));
    let frames: Vec<_> = read(cx, |capture| {
        capture.input().iter().map(|record| record.frame).collect()
    });
    assert_eq!(frames, [Some(frame.id), Some(frame.id)]);
    redraw(cx);
    assert!(latest_frame(cx).input.is_empty());
}

#[gpui::test]
fn capture_levels(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    assert!(record(&latest_tree(cx), "inner").details.is_some());

    write(cx, |capture| {
        capture.config_mut().level = CaptureLevel::Tree
    });
    redraw(cx);
    let tree = latest_tree(cx);
    assert!(tree.elements.iter().all(|record| record.details.is_none()));
    assert_eq!(
        latest_frame(cx).tree.as_ref().map(|tree| tree.frame),
        Some(latest_frame(cx).id)
    );

    write(cx, |capture| {
        capture.config_mut().level = CaptureLevel::Frames
    });
    redraw(cx);
    let frame = latest_frame(cx);
    assert!(frame.tree.is_none());
    assert!(frame.element_count >= 5, "elements are still counted");
    assert!(!frame.views.is_empty());
}

#[gpui::test]
fn the_capture_clock_is_the_executors(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let opened = read(cx, |capture| capture.now());
    assert_eq!(latest_frame(cx).start, opened, "fake time stood still");

    cx.executor().advance_clock(Duration::from_millis(2_500));
    let now = read(cx, |capture| capture.now());
    assert_eq!(now, opened + Duration::from_millis(2_500));
    view.update(cx, |_, cx| cx.notify());
    let frame = latest_frame(cx);
    assert_eq!(frame.start, now, "frames start on the same clock");
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.simulate_click(point(px(20.), px(30.)), Modifiers::none());
    let clicked = read(cx, |capture| capture.input().back().unwrap().at);
    assert_eq!(clicked, now + Duration::from_millis(400));

    // An installed fixture goes on from the end of its recording.
    let mut fixture = InspectorCapture::new_for_test();
    let mut recorded = frame;
    recorded.start = Duration::from_secs(12);
    recorded.timings.total = Duration::from_millis(5);
    fixture.push_frame_for_test(recorded);
    cx.update(|window, _| window.replace_inspector_capture_for_test(fixture));
    let end = Duration::from_millis(12_005);
    assert_eq!(read(cx, |capture| capture.now()), end);
    cx.executor().advance_clock(Duration::from_secs(3));
    assert_eq!(
        read(cx, |capture| capture.now()),
        end + Duration::from_secs(3)
    );
}

// Details.

#[gpui::test]
fn elements_report_details(cx: &mut TestAppContext) {
    let (_, cx) = scene(cx, || {
        div()
            .id("root")
            .size_full()
            .child(
                div()
                    .id("box")
                    .m(px(4.))
                    .p(px(6.))
                    .border_2()
                    .border_color(red())
                    .rounded(px(5.))
                    .bg(blue())
                    .opacity(0.5)
                    .w(px(120.))
                    .h_1_2()
                    .flex_grow(1.)
                    .key_context("Editor")
                    .child("hello"),
            )
            .child(
                uniform_list("list", 100, |range, _, _| {
                    range
                        .map(|ix| div().h(px(10.)).child(format!("row {ix}")))
                        .collect()
                })
                .h(px(50.)),
            )
            .child(crate::svg().path("icons/close.svg").id("icon").size_4())
            .child(crate::img("images/logo.png").id("logo").size_4())
    });
    open(cx);
    let tree = latest_tree(cx);
    let details = record(&tree, "box").details.clone().expect("details");
    let box_model = details.box_model.unwrap();
    assert_eq!(box_model.margin, crate::Edges::all(px(4.)));
    assert_eq!(box_model.padding, crate::Edges::all(px(6.)));
    assert_eq!(box_model.border, crate::Edges::all(px(2.)));
    assert_eq!(details.background, Some(blue()));
    assert_eq!(details.border_color, Some(red()));
    assert_eq!(details.corner_radius, Some(px(5.)));
    assert_eq!(details.opacity, Some(0.5));
    assert_eq!(details.key_context.as_deref(), Some("Editor"));
    let layout = details.layout.unwrap();
    assert_eq!(
        layout.display.as_ref(),
        "block",
        "divs are blocks unless flex"
    );
    assert_eq!(layout.flex_direction, None);
    assert_eq!(layout.size.width, SizeSpec::Pixels(px(120.)));
    assert_eq!(layout.size.height, SizeSpec::Fraction(0.5));
    assert_eq!(layout.flex_grow, 1.);

    let text = tree.children(find_id(&tree, "box"))[0];
    let text_details = tree
        .get(text)
        .unwrap()
        .details
        .clone()
        .expect("text details");
    assert_eq!(text_details.text.as_deref(), Some("hello"));
    assert!(text_details.font_size.is_some());
    assert!(text_details.text_color.is_some());

    let list = record(&tree, "list").details.clone().expect("list details");
    assert_eq!(list.list, Some((100, 0..5)));

    let source = |id| {
        record(&tree, id)
            .details
            .clone()
            .and_then(|details| details.source)
    };
    assert_eq!(source("icon").as_deref(), Some("icons/close.svg"));
    assert_eq!(source("logo").as_deref(), Some("images/logo.png"));
}

// Style overrides.

#[gpui::test]
fn overrides_restyle_and_relayout(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let key = key_of(&latest_tree(cx), "inner");
    write(cx, |capture| {
        capture.set_override(key.path, Some(StyleRefinement::default().w(px(120.))))
    });
    redraw(cx);
    let tree = latest_tree(cx);
    assert_eq!(record(&tree, "inner").bounds, bounds(10., 20., 120., 40.));
    assert!(
        record(&tree, "inner")
            .flags
            .contains(ElementFlags::OVERRIDDEN)
    );
    assert_eq!(
        record(&tree, "sibling").bounds.origin.x,
        px(130.),
        "siblings relayout"
    );
    assert!(
        !record(&tree, "outer")
            .flags
            .contains(ElementFlags::OVERRIDDEN)
    );

    write(cx, |capture| capture.set_override(key.path, None));
    redraw(cx);
    let tree = latest_tree(cx);
    assert_eq!(record(&tree, "inner").bounds, bounds(10., 20., 50., 40.));
    assert!(
        !record(&tree, "inner")
            .flags
            .contains(ElementFlags::OVERRIDDEN)
    );
}

#[gpui::test]
fn forced_states_apply_state_styles(cx: &mut TestAppContext) {
    let hovered_color: Hsla = red();
    let (_, cx) = scene(cx, move || {
        div()
            .id("button")
            .w(px(40.))
            .h(px(20.))
            .bg(blue())
            .hover(move |style| style.bg(hovered_color))
    });
    open(cx);
    let tree = latest_tree(cx);
    assert!(
        record(&tree, "button")
            .flags
            .contains(ElementFlags::STATEFUL_STYLE)
    );
    let key = key_of(&tree, "button");
    let background = |cx: &mut VisualTestContext| {
        record(&latest_tree(cx), "button")
            .details
            .clone()
            .unwrap()
            .background
    };
    assert_eq!(background(cx), Some(blue()));

    write(cx, |capture| {
        capture.set_forced_states(key.path, ForcedStates::HOVER)
    });
    redraw(cx);
    assert_eq!(
        background(cx),
        Some(red()),
        "hover is forced without a pointer"
    );
    let painted_red = cx.update(|window, _| {
        window
            .painted_quads()
            .iter()
            .any(|quad| quad.background.as_solid() == Some(red()))
    });
    assert!(painted_red);

    write(cx, |capture| {
        capture.set_forced_states(key.path, ForcedStates::empty())
    });
    redraw(cx);
    assert_eq!(background(cx), Some(blue()));
}

#[gpui::test]
fn the_selected_element_keeps_its_base_style(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let key = key_of(&latest_tree(cx), "inner");
    write(cx, |capture| {
        capture.overlay_mut().selected = Some(key);
        capture.set_override(key.path, Some(StyleRefinement::default().w(px(99.))));
    });
    redraw(cx);
    let selected = read(cx, |capture| capture.selected_style().cloned()).expect("recorded");
    assert_eq!(selected.key, key);
    let expected = StyleRefinement::default().w(px(50.)).h(px(40.)).bg(red());
    assert_eq!(
        *selected.base, expected,
        "the base style precedes overrides"
    );

    write(cx, |capture| capture.overlay_mut().selected = None);
    redraw(cx);
    assert!(read(cx, |capture| capture.selected_style().is_none()));
}

// Picking.

#[gpui::test]
fn picking_selects_the_deepest_element_and_walks_up(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let events = events(cx);
    let tree = latest_tree(cx);
    let (inner, outer) = (key_of(&tree, "inner"), key_of(&tree, "outer"));
    let clicks = view.read_with(cx, |view, _| view.clicks.clone());

    cx.update(|window, _| window.start_inspector_pick());
    assert!(cx.update(|window, cx| window.is_inspector_picking(cx)));
    let position = point(px(20.), px(30.));
    cx.simulate_mouse_move(position, None, Modifiers::none());
    assert_eq!(read(cx, |capture| capture.overlay().hovered()), Some(inner));
    assert_eq!(
        events.borrow().last(),
        Some(&InspectorEvent::PickHovered(Some(inner)))
    );
    let frame = latest_frame(cx);
    assert!(
        frame.inspector_only,
        "picking only redraws the overlay: {:?}",
        frame.causes
    );

    cx.simulate_keystrokes("]");
    assert_eq!(read(cx, |capture| capture.overlay().hovered()), Some(outer));
    cx.simulate_keystrokes("[");
    assert_eq!(read(cx, |capture| capture.overlay().hovered()), Some(inner));
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(36.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    assert_eq!(read(cx, |capture| capture.overlay().hovered()), Some(outer));
    assert_eq!(read(cx, |capture| capture.pick().depth), 1);

    cx.simulate_click(position, Modifiers::none());
    assert_eq!(
        *clicks.borrow(),
        0,
        "the app does not see the picking click"
    );
    assert_eq!(read(cx, |capture| capture.overlay().selected), Some(outer));
    assert!(!read(cx, |capture| capture.pick().active));
    assert_eq!(events.borrow().last(), Some(&InspectorEvent::Picked(outer)));

    // Once picking ends, the app gets its clicks again.
    cx.simulate_click(position, Modifiers::none());
    assert_eq!(*clicks.borrow(), 1);
}

#[gpui::test]
fn escape_or_stopping_cancels_picking(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let events = events(cx);

    cx.update(|window, _| window.start_inspector_pick());
    cx.simulate_mouse_move(point(px(20.), px(30.)), None, Modifiers::none());
    cx.simulate_keystrokes("escape");
    assert!(!read(cx, |capture| capture.pick().active));
    assert_eq!(read(cx, |capture| capture.overlay().hovered()), None);
    assert_eq!(events.borrow().last(), Some(&InspectorEvent::PickCancelled));

    events.borrow_mut().clear();
    cx.update(|window, _| {
        window.start_inspector_pick();
        window.stop_inspector_pick();
    });
    assert!(!read(cx, |capture| capture.pick().active));
    assert_eq!(events.borrow().as_slice(), [InspectorEvent::PickCancelled]);
}

// Overlays.

#[gpui::test]
fn overlays_paint_inside_the_app_only(cx: &mut TestAppContext) {
    let (_, cx) = scene(cx, || {
        div().id("root").size_full().child(
            div()
                .id("wide")
                .absolute()
                .left(px(1300.))
                .w(px(200.))
                .h(px(40.))
                .bg(red())
                .on_click(|_, _, _| {}),
        )
    });
    open(cx);
    let app = app_bounds(cx);
    let scale = cx.update(|window, _| window.scale_factor());
    let outside_app = |cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            window
                .painted_quads()
                .iter()
                .filter(|quad| {
                    let mask = quad.content_mask.bounds;
                    mask.right() > (app.right() + px(0.5)).scale(scale)
                })
                .count()
        })
    };
    let (quads_before, outside_before) = (
        cx.update(|window, _| window.painted_quads().len()),
        outside_app(cx),
    );

    let key = key_of(&latest_tree(cx), "wide");
    write(cx, |capture| {
        let overlay = capture.overlay_mut();
        overlay.modes = OverlayModes::all();
        overlay.selected = Some(key);
        overlay.set_highlights(
            EntityId::from(1u64),
            vec![OverlayHighlight {
                bounds: bounds(1200., 0., 600., 100.),
                color: blue(),
                label: Some("highlight".into()),
            }],
        );
    });
    redraw(cx);
    let quads_after = cx.update(|window, _| window.painted_quads().len());
    assert!(quads_after > quads_before + 10, "overlays were painted");
    assert_eq!(
        outside_app(cx),
        outside_before,
        "no overlay quad reaches the dock"
    );

    // Overlays are the inspector's: excluded from the app's scene stats.
    let frame = latest_frame(cx);
    assert_eq!(frame.scene.quads as usize, quads_before);
}

#[gpui::test]
fn paint_flash_fades_in_inspector_only_frames(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    write(cx, |capture| {
        capture.overlay_mut().modes = OverlayModes::PAINT_FLASH
    });
    view.update(cx, |_, cx| cx.notify());
    assert!(!latest_frame(cx).inspector_only);
    assert!(
        cx.update(|window, _| window.invalidator.is_dirty()),
        "the flash requests a frame"
    );

    let id = latest_frame(cx).id;
    cx.update(|_, _| {});
    let frame = latest_frame(cx);
    assert!(frame.id > id);
    assert!(frame.inspector_only, "{:?}", frame.causes);
    assert!(
        frame
            .causes
            .iter()
            .all(|cause| cause.kind == CauseKind::Animation)
    );
}

/// The paint flash quads the latest frame painted, in window pixels: the
/// faint fills and the 2 px outlines (which the scene splits into strips,
/// gathered back here), each with its color.
#[derive(Debug, Default)]
struct FlashQuads {
    fills: Vec<(Bounds<Pixels>, Hsla)>,
    outlines: Vec<(Bounds<Pixels>, Hsla)>,
}

impl FlashQuads {
    fn of(cx: &mut VisualTestContext) -> Self {
        cx.update(|window, _| {
            let scale = window.scale_factor();
            let unscale = |bounds: Bounds<crate::ScaledPixels>| {
                Bounds::new(
                    point(px(bounds.origin.x.0 / scale), px(bounds.origin.y.0 / scale)),
                    size(
                        px(bounds.size.width.0 / scale),
                        px(bounds.size.height.0 / scale),
                    ),
                )
            };
            let mut quads = FlashQuads::default();
            for quad in window.painted_quads() {
                let background = quad.background.as_solid().unwrap_or_default();
                let border = quad.border_widths.top.0 / scale;
                let rounded = quad.corner_radii.top_left.0 > 0.;
                if border == 0. && !rounded && background.alpha > 0. {
                    if background.alpha <= flash::FLASH_FILL_ALPHA + 1e-4 {
                        quads.fills.push((unscale(quad.bounds), background));
                    }
                } else if (border - 2.).abs() < 1e-3 && background.alpha == 0. {
                    let outline = (
                        unscale(quad.bounds),
                        quad.border_color.as_solid().unwrap_or_default(),
                    );
                    if !quads.outlines.contains(&outline) {
                        quads.outlines.push(outline);
                    }
                }
            }
            quads
        })
    }

    fn outline_at(&self, bounds: Bounds<Pixels>) -> Option<Hsla> {
        self.outlines
            .iter()
            .find(|(outline, _)| *outline == bounds)
            .map(|(_, color)| *color)
    }
}

fn flash_on(cx: &mut VisualTestContext) {
    write(cx, |capture| {
        capture.overlay_mut().modes = OverlayModes::PAINT_FLASH
    });
}

fn hue(color: Hsla) -> f32 {
    color.color.hue.into_positive_degrees()
}

#[gpui::test]
fn paint_flashes_fade_on_the_executor_clock(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    flash_on(cx);
    view.update(cx, |_, cx| cx.notify());
    let tree = latest_tree(cx);
    let view_bounds = tree.get(tree.roots[0]).unwrap().bounds;
    // The view's fill and outline opacities, while painted.
    let flash = |cx: &mut VisualTestContext| {
        let quads = FlashQuads::of(cx);
        let fill = quads
            .fills
            .iter()
            .find(|(bounds, _)| *bounds == view_bounds)
            .map(|(_, color)| color.alpha);
        fill.zip(quads.outline_at(view_bounds).map(|color| color.alpha))
    };
    let (fill, outline) = flash(cx).expect("the flash is painted");
    assert!((fill - flash::FLASH_FILL_ALPHA).abs() < 1e-6, "{fill}");
    assert!((outline - 1.).abs() < 1e-6, "{outline}");

    // Half the fade later, on fake time, exactly half as opaque.
    cx.executor().advance_clock(flash::FLASH_DURATION / 2);
    redraw_inspector_only(cx);
    let (fill, outline) = flash(cx).expect("still fading");
    assert!((fill - flash::FLASH_FILL_ALPHA / 2.).abs() < 1e-6, "{fill}");
    assert!((outline - 0.5).abs() < 1e-6, "{outline}");

    cx.executor().advance_clock(flash::FLASH_DURATION / 2);
    redraw_inspector_only(cx);
    assert_eq!(flash(cx), None, "faded out");
    assert!(
        !cx.update(|window, _| window.invalidator.is_dirty()),
        "and nothing more is drawn for it"
    );
}

#[gpui::test]
fn nested_views_flash_one_faint_fill_and_an_outline_each(cx: &mut TestAppContext) {
    let (view, cx) = parent(cx, false);
    open(cx);
    flash_on(cx);
    view.update(cx, |_, cx| cx.notify());
    let tree = latest_tree(cx);
    let view_bounds = |entity: EntityId| {
        let ix = tree
            .elements
            .iter()
            .position(|record| {
                matches!(record.kind, ElementKind::View { entity: drawn, .. } if drawn == entity)
            })
            .expect("the view is drawn");
        tree.elements[ix].bounds
    };
    let child = view.read_with(cx, |parent, _| parent.child.entity_id());
    let (outer, inner) = (view_bounds(view.entity_id()), view_bounds(child));
    assert_ne!(outer, inner);

    let quads = FlashQuads::of(cx);
    assert_eq!(quads.outlines.len(), 2, "{quads:?}");
    assert!(quads.outline_at(outer).is_some() && quads.outline_at(inner).is_some());
    // Under both flashes, the app shows through all but the faint fill.
    let center = inner.center();
    let covering: Vec<f32> = quads
        .fills
        .iter()
        .filter(|(bounds, _)| bounds.contains(&center))
        .map(|(_, color)| color.alpha)
        .collect();
    assert_eq!(covering.len(), 1, "{quads:?}");
    assert!(covering.iter().sum::<f32>() <= flash::FLASH_FILL_ALPHA + 1e-6);
}

#[gpui::test]
fn a_cached_view_that_did_not_render_does_not_flash(cx: &mut TestAppContext) {
    let (view, cx) = parent(cx, true);
    open(cx);
    flash_on(cx);
    view.update(cx, |_, cx| cx.notify());
    let tree = latest_tree(cx);
    let parent_bounds = record(&tree, "parent").bounds;
    let quads = FlashQuads::of(cx);
    assert_eq!(
        quads.outlines.len(),
        1,
        "only the parent rendered: {quads:?}"
    );
    assert!(quads.outline_at(parent_bounds).is_some());
}

#[gpui::test]
fn flashes_are_colored_by_render_rate_and_hot_views_labeled(cx: &mut TestAppContext) {
    let (view, cx) = parent(cx, true);
    open(cx);
    flash_on(cx);
    let child = view.read_with(cx, |parent, _| parent.child.clone());
    let labels = |cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            window
                .painted_text()
                .iter()
                .map(|line| line.text.to_string())
                .filter(|text| text.contains(" ×") && text.ends_with("/s"))
                .collect::<Vec<_>>()
        })
    };

    // A one-off render, long after anything else rendered: a teal blink.
    cx.executor().advance_clock(Duration::from_secs(2));
    child.update(cx, |_, cx| cx.notify());
    let tree = latest_tree(cx);
    let child_bounds = record(&tree, "leaf-root").bounds;
    let blink = FlashQuads::of(cx)
        .outline_at(child_bounds)
        .expect("the child flashed");
    assert!((hue(blink) - 180.).abs() < 1., "teal: {}", hue(blink));
    assert!(labels(cx).is_empty());

    // The parent renders every frame for a second: it glows red, labeled.
    for _ in 0..60 {
        cx.executor().advance_clock(Duration::from_millis(16));
        view.update(cx, |_, cx| cx.notify());
    }
    let parent_bounds = record(&latest_tree(cx), "parent").bounds;
    let glow = FlashQuads::of(cx)
        .outline_at(parent_bounds)
        .expect("the parent flashed");
    assert!(hue(glow) < 1. || hue(glow) > 359., "red: {}", hue(glow));
    let labels = labels(cx);
    assert_eq!(labels.len(), 1, "{labels:?}");
    let rate: u32 = labels[0]
        .trim_start_matches("Parent ×")
        .trim_end_matches("/s")
        .parse()
        .unwrap_or_else(|_| panic!("{labels:?}"));
    assert!(rate >= HOT_RENDERS_PER_SECOND, "{rate}");
}

/// Draws the frame a fading paint flash asks for: the inspector's alone.
fn redraw_inspector_only(cx: &mut VisualTestContext) {
    assert!(cx.update(|window, _| window.invalidator.is_dirty()));
    cx.update(|_, _| {});
    assert!(latest_frame(cx).inspector_only);
}

#[gpui::test]
fn highlights_are_set_and_cleared_per_owner(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let (frames, audit) = (EntityId::from(1u64), EntityId::from(2u64));
    let highlight = |x: f32| OverlayHighlight {
        bounds: bounds(x, 300., 10., 10.),
        color: blue(),
        label: None,
    };
    let painted = |cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            let scale = window.scale_factor();
            [1., 50., 80.]
                .into_iter()
                .filter(|&x| {
                    let bounds = bounds(x, 300., 10., 10.).scale(scale);
                    window
                        .painted_quads()
                        .iter()
                        .any(|quad| quad.bounds == bounds)
                })
                .collect::<Vec<_>>()
        })
    };

    write(cx, |capture| {
        let overlay = capture.overlay_mut();
        assert!(overlay.set_highlights(frames, vec![highlight(1.)]));
        assert!(overlay.set_highlights(audit, vec![highlight(50.), highlight(80.)]));
        assert!(
            !overlay.set_highlights(frames, vec![highlight(1.)]),
            "the same highlights change nothing"
        );
        assert_eq!(overlay.highlights().count(), 3);
    });
    redraw(cx);
    assert_eq!(painted(cx), [1., 50., 80.]);

    // One owner clearing its highlights leaves the other's alone.
    write(cx, |capture| {
        let overlay = capture.overlay_mut();
        assert!(overlay.clear_highlights(frames));
        assert!(!overlay.clear_highlights(frames), "already clear");
        assert_eq!(overlay.highlights().count(), 2);
    });
    redraw(cx);
    assert_eq!(painted(cx), [50., 80.]);
    write(cx, |capture| {
        assert!(capture.overlay_mut().set_highlights(audit, Vec::new()));
        assert_eq!(capture.overlay().highlights().count(), 0);
    });
    redraw(cx);
    assert!(painted(cx).is_empty());
}

#[test]
fn hovers_belong_to_whoever_started_them() {
    let (elements, events) = (EntityId::from(1u64), EntityId::from(2u64));
    let key = |path| ElementKey {
        path: PathKey(path),
        instance: 0,
    };
    let mut overlay = OverlayState::default();

    assert!(overlay.set_hovered(elements, Some(key(1))));
    assert!(!overlay.set_hovered(elements, Some(key(1))), "unchanged");
    // Another view takes the hover over; the first one ending its own no
    // longer touches it.
    assert!(overlay.set_hovered(events, Some(key(2))));
    assert!(!overlay.set_hovered(elements, None));
    assert_eq!(overlay.hovered(), Some(key(2)));
    // Nor does the picker ending its hover.
    assert!(!overlay.set_pick_hovered(None));
    assert!(overlay.set_hovered(events, None));
    assert_eq!(overlay.hovered(), None);

    // Withdrawing a view takes its highlights and its hover, nobody else's.
    let highlight = OverlayHighlight {
        bounds: bounds(0., 0., 10., 10.),
        color: blue(),
        label: None,
    };
    overlay.set_highlights(elements, vec![highlight.clone()]);
    overlay.set_highlights(events, vec![highlight]);
    overlay.set_hovered(elements, Some(key(1)));
    assert!(overlay.withdraw(elements));
    assert_eq!(overlay.hovered(), None);
    assert_eq!(overlay.highlights().count(), 1);
    assert!(!overlay.withdraw(elements), "nothing left to withdraw");
    assert!(overlay.set_pick_hovered(Some(key(3))));
    assert!(overlay.withdraw(events));
    assert_eq!(overlay.hovered(), Some(key(3)), "the picker's hover stays");
    assert_eq!(overlay.highlights().count(), 0);
}

// Lifetime, freezing, dock.

#[gpui::test]
fn nothing_is_recorded_while_closed(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    view.update(cx, |_, cx| cx.notify());
    assert!(cx.update(|window, _| window.inspector_capture().is_none()));
    assert!(cx.update(|window, _| !window.invalidator.has_cause_log()));

    open(cx);
    assert_eq!(read(cx, |capture| capture.frames().len()), 1);
    view.update(cx, |_, cx| cx.notify());
    assert_eq!(read(cx, |capture| capture.frames().len()), 2);

    close(cx);
    assert!(cx.update(|window, _| window.inspector_capture().is_none()));
    assert!(cx.update(|window, _| !window.invalidator.has_cause_log()));
    view.update(cx, |_, cx| cx.notify());

    open(cx);
    let frames = read(cx, |capture| capture.frames().len());
    assert_eq!(frames, 1, "a new capture starts empty");
    assert_eq!(latest_frame(cx).causes[0].kind, CauseKind::Initial);
    assert!(read(cx, |capture| capture.notify_stats().is_empty()));
}

#[gpui::test]
fn freezing_stops_the_rings_but_not_picking(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    write(cx, |capture| capture.set_frozen(true));
    let (frames, generation) = read(cx, |capture| (capture.frames().len(), capture.generation()));
    view.update(cx, |view, cx| {
        view.outer_width = px(400.);
        cx.notify();
    });
    assert_eq!(read(cx, |capture| capture.frames().len()), frames);
    assert_eq!(read(cx, |capture| capture.generation()), generation);
    assert!(read(cx, |capture| capture.notify_stats().is_empty()));

    // Picking walks the live tree, which follows the new layout.
    cx.update(|window, _| window.start_inspector_pick());
    cx.simulate_mouse_move(point(px(300.), px(50.)), None, Modifiers::none());
    let outer = key_of(&latest_tree(cx), "outer");
    assert_eq!(read(cx, |capture| capture.overlay().hovered()), Some(outer));

    write(cx, |capture| capture.set_frozen(false));
    view.update(cx, |_, cx| cx.notify());
    assert!(read(cx, |capture| capture.frames().len()) > frames);
}

#[gpui::test]
fn dock_changes_relayout_the_app(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    let viewport = cx.update(|window, _| window.viewport_size());
    write(cx, |capture| {
        capture.set_dock(InspectorDock::Bottom { height: px(300.) })
    });
    redraw(cx);
    let app = app_bounds(cx);
    assert_eq!(
        app,
        Bounds::new(
            Point::default(),
            size(viewport.width, viewport.height - px(300.))
        )
    );
    let tree = latest_tree(cx);
    assert_eq!(tree.get(tree.roots[0]).unwrap().bounds, app);
    assert_eq!(record(&tree, "root").bounds, app);
    let frame = latest_frame(cx);
    assert_eq!(frame.viewport, app.size);
    assert!(
        frame
            .causes
            .iter()
            .any(|cause| cause.kind == CauseKind::Resize && !cause.from_inspector)
    );
    assert_eq!(
        cx.update(|window, _| window.inspector_bounds()),
        Some(Bounds::new(
            point(px(0.), viewport.height - px(300.)),
            size(viewport.width, px(300.))
        ))
    );
}

#[test]
fn dock_extents_are_clamped() {
    let viewport = size(px(1000.), px(600.));
    let width = |dock: InspectorDock, viewport| dock.split(viewport).1.size.width;
    assert_eq!(
        width(InspectorDock::Right { width: px(10.) }, viewport),
        px(240.)
    );
    assert_eq!(
        width(InspectorDock::Right { width: px(5000.) }, viewport),
        px(880.)
    );
    assert_eq!(
        width(InspectorDock::Right { width: px(400.) }, viewport),
        px(400.)
    );
    let (app, dock) = InspectorDock::Right { width: px(400.) }.split(size(px(300.), px(600.)));
    assert_eq!(app.size.width, px(120.), "the app keeps its minimum");
    assert_eq!(dock.size.width, px(180.));
    let (app, dock) = InspectorDock::Bottom { height: px(100.) }.split(viewport);
    assert_eq!(dock.size.height, px(240.));
    assert_eq!(app.size.height, px(360.));
    assert_eq!(dock.origin.y, px(360.));
}

#[gpui::test]
fn a_hidden_dock_records_with_the_app_in_the_whole_window(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Nested::new());
    open(cx);
    write(cx, |capture| capture.set_dock(InspectorDock::Hidden));
    redraw(cx);
    let viewport = cx.update(|window, _| window.viewport_size());
    assert_eq!(app_bounds(cx), Bounds::new(Point::default(), viewport));
    assert_eq!(cx.update(|window, _| window.inspector_bounds()), None);
    let tree = latest_tree(cx);
    assert_eq!(record(&tree, "root").bounds.size, viewport);
}

// Overhead.

/// Median `Window::draw` time of a ~2,000 element scene with the inspector
/// closed and open at each capture level. Prints its measurements:
/// `cargo test -p gpui-ce --features test-support --lib capture_overhead -- --ignored --nocapture`
#[gpui::test]
#[ignore = "a measurement, not a check"]
fn capture_overhead(cx: &mut TestAppContext) {
    const ROWS: usize = 40;
    const COLUMNS: usize = 48;
    let (_, cx) = scene(cx, || {
        div()
            .id("grid")
            .size_full()
            .flex()
            .flex_col()
            .children((0..ROWS).map(|row| {
                div()
                    .id(("row", row))
                    .flex()
                    .children((0..COLUMNS).map(|column| {
                        div()
                            .id(column)
                            .w(px(8.))
                            .h(px(8.))
                            .bg(red())
                            .hover(|style| style.bg(blue()))
                    }))
            }))
    });
    let median_draw = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            for _ in 0..10 {
                window.draw(cx).clear(cx);
            }
            let mut samples = (0..60)
                .map(|_| {
                    let start = scheduler::Instant::now();
                    window.draw(cx).clear(cx);
                    start.elapsed()
                })
                .collect::<Vec<_>>();
            samples.sort();
            samples[samples.len() / 2]
        })
    };

    let closed = median_draw(cx);
    println!("closed      {closed:>10.2?}");
    open(cx);
    let elements = latest_frame(cx).element_count;
    for level in [CaptureLevel::Frames, CaptureLevel::Tree, CaptureLevel::Full] {
        write(cx, |capture| capture.config_mut().level = level);
        let open = median_draw(cx);
        let overhead = open.as_secs_f64() / closed.as_secs_f64() - 1.;
        println!(
            "{:<11} {open:>10.2?}  {:+.0}%  ({elements} elements)",
            format!("{level:?}"),
            overhead * 100.
        );
    }

    // Frames the inspector draws for itself replay the app instead.
    let replayed = cx.update(|window, cx| {
        let mut samples = (0..60)
            .map(|_| {
                window
                    .invalidator
                    .note_cause(CauseKind::Refresh, None, true);
                let start = scheduler::Instant::now();
                window.draw(cx).clear(cx);
                start.elapsed()
            })
            .collect::<Vec<_>>();
        samples.sort();
        samples[samples.len() / 2]
    });
    assert!(read(cx, |capture| capture.recorder.app_replayed()));
    println!("replayed    {replayed:>10.2?}");
}

// Replaying the app.

/// A model the app reads while rendering, without observing it.
struct Label(usize);

/// A child view drawn cached, counting its renders.
struct Counted {
    renders: usize,
}

impl Render for Counted {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div().id("counted").size_full().bg(red())
    }
}

struct Tip;

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("tip").w(px(60.)).h(px(20.)).bg(blue())
    }
}

/// An app that counts its renders, reads a model, listens for clicks and
/// draws a deferred popover, a tooltip, a window control area and a cached
/// child: everything a replayed frame must keep.
struct Replayed {
    renders: usize,
    clicks: usize,
    label: Entity<Label>,
    child: Entity<Counted>,
}

impl Render for Replayed {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let label = self.label.read(cx).0;
        div()
            .id("root")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("button")
                    .w(px(40.))
                    .h(px(40.))
                    .bg(red())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clicks += 1;
                        cx.notify();
                    }))
                    .child(deferred(
                        div()
                            .id("popover")
                            .absolute()
                            .top(px(300.))
                            .w(px(80.))
                            .h(px(30.))
                            .bg(blue())
                            .on_click(|_, _, _| {}),
                    )),
            )
            .child(
                div()
                    .id("trigger")
                    .w(px(40.))
                    .h(px(40.))
                    .bg(blue())
                    .tooltip(|_, cx| cx.new(|_| Tip).into()),
            )
            .child(
                div()
                    .id("drag")
                    .w(px(20.))
                    .h(px(20.))
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                self.child
                    .clone()
                    .cached(StyleRefinement::default().w(px(50.)).h(px(50.))),
            )
            .child(format!("label {label}"))
    }
}

fn replayed(cx: &mut TestAppContext) -> (Entity<Replayed>, &mut VisualTestContext) {
    cx.add_window_view(|_, cx| Replayed {
        renders: 0,
        clicks: 0,
        label: cx.new(|_| Label(0)),
        child: cx.new(|_| Counted { renders: 0 }),
    })
}

/// How often the app and its cached child rendered.
fn renders(view: &Entity<Replayed>, cx: &mut VisualTestContext) -> (usize, usize) {
    cx.update(|_, cx| {
        let view = view.read(cx);
        (view.renders, view.child.read(cx).renders)
    })
}

/// Everything the latest frame drew, down to what input hits.
#[derive(Debug, PartialEq)]
struct Drawn {
    quads: String,
    text: Vec<crate::PaintedText>,
    hitboxes: Vec<(crate::HitboxId, Bounds<Pixels>, bool)>,
    window_controls: Vec<(WindowControlArea, crate::HitboxId)>,
    deferred_draws: usize,
    tooltip: Option<Bounds<Pixels>>,
}

fn drawn(cx: &mut VisualTestContext) -> Drawn {
    cx.update(|window, _| {
        let frame = &window.rendered_frame;
        Drawn {
            quads: format!("{:?}", window.painted_quads()),
            text: window.painted_text().to_vec(),
            hitboxes: frame
                .hitboxes
                .iter()
                .map(|hitbox| (hitbox.id, hitbox.bounds, hitbox.is_hovered(window)))
                .collect(),
            window_controls: frame
                .window_control_hitboxes
                .iter()
                .map(|(area, hitbox)| (*area, hitbox.id))
                .collect(),
            deferred_draws: frame.deferred_draws.len(),
            tooltip: window.tooltip_bounds.as_ref().map(|tooltip| tooltip.bounds),
        }
    })
}

fn app_replayed(cx: &mut VisualTestContext) -> bool {
    read(cx, |capture| capture.recorder.app_replayed())
}

/// The views a frame reports, and how each was drawn.
fn view_outcomes(frame: &FrameRecord) -> Vec<(&'static str, ViewOutcome)> {
    frame
        .views
        .iter()
        .map(|view| {
            let name = view.type_name.rsplit("::").next().unwrap();
            (name, view.outcome)
        })
        .collect()
}

fn painted(cx: &mut VisualTestContext, text: &str) -> bool {
    cx.update(|window, _| window.painted_text().iter().any(|line| line.text == text))
}

#[gpui::test]
fn inspector_only_frames_replay_the_app(cx: &mut TestAppContext) {
    let (view, cx) = replayed(cx);
    let dock = dock_renderer(cx);
    open(cx);
    let dock = dock.borrow().clone().expect("the dock rendered");
    cx.simulate_mouse_move(point(px(10.), px(310.)), None, Modifiers::none());
    let app_frame = latest_frame(cx);
    let tree = latest_tree(cx);
    let generation = read(cx, |capture| capture.generation());
    let before = drawn(cx);
    let rendered = renders(&view, cx);
    assert_eq!(before.window_controls.len(), 1);
    assert_eq!(before.deferred_draws, 1);
    let popover = record(&tree, "popover").bounds;
    assert!(
        before
            .hitboxes
            .iter()
            .any(|&(_, bounds, hovered)| bounds == popover && hovered),
        "the mouse is over the popover"
    );

    dock.update(cx, |_, cx| cx.notify());
    let frame = latest_frame(cx);
    assert!(frame.id > app_frame.id);
    assert!(frame.inspector_only);
    assert!(app_replayed(cx));
    assert_eq!(renders(&view, cx), rendered, "no app view rendered");
    assert_eq!(drawn(cx), before, "the app is drawn exactly as before");
    assert_eq!(frame.scene, app_frame.scene);
    assert_eq!(frame.element_count, app_frame.element_count);
    assert!(frame.timings.render.is_zero());

    // The capture tells the truth: every app view was replayed, not served
    // from its cache, and the frame has no tree of its own; the last live
    // tree still applies.
    assert!(frame.tree.is_none());
    assert!(Arc::ptr_eq(&latest_tree(cx), &tree));
    assert_eq!(read(cx, |capture| capture.generation()), generation);
    let entities = |frame: &FrameRecord| {
        frame
            .views
            .iter()
            .map(|view| (view.entity, view.depth))
            .collect::<Vec<_>>()
    };
    assert_eq!(entities(&frame), entities(&app_frame));
    assert!(
        frame
            .views
            .iter()
            .all(|view| view.outcome == ViewOutcome::Replayed
                && view.element.is_none()
                && view.duration.is_zero())
    );
    assert!(frame.replayed_app() && !app_frame.replayed_app());

    // Picking walks the last live tree over the replayed app.
    cx.update(|window, _| window.start_inspector_pick());
    cx.simulate_mouse_move(point(px(12.), px(12.)), None, Modifiers::none());
    assert!(app_replayed(cx));
    assert_eq!(
        read(cx, |capture| capture.overlay().hovered()),
        Some(key_of(&tree, "button"))
    );
    cx.update(|window, _| window.stop_inspector_pick());
    assert_eq!(renders(&view, cx), rendered);

    // The replayed listeners still handle input, and the app renders again
    // (on press and on release, which refresh the window).
    cx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
    assert_eq!(cx.update(|_, cx| view.read(cx).clicks), 1);
    let clicked = renders(&view, cx);
    assert!(clicked.0 > rendered.0);
    let frame = latest_frame(cx);
    assert!(!frame.inspector_only);
    assert!(frame.tree.is_some());
    assert_eq!(
        view_outcomes(&frame),
        [
            ("Replayed", ViewOutcome::Rendered),
            ("Counted", ViewOutcome::Rendered)
        ]
    );

    // The window still observes what the app read before the replays.
    dock.update(cx, |_, cx| cx.notify());
    dock.update(cx, |_, cx| cx.notify());
    assert!(app_replayed(cx));
    let label = cx.update(|_, cx| view.read(cx).label.clone());
    label.update(cx, |label, cx| {
        label.0 = 7;
        cx.notify();
    });
    assert!(!app_replayed(cx));
    assert_eq!(renders(&view, cx).0, clicked.0 + 1);
    assert!(painted(cx, "label 7"));
}

#[gpui::test]
fn replay_keeps_deferred_draws_and_tooltips(cx: &mut TestAppContext) {
    let (view, cx) = replayed(cx);
    let dock = dock_renderer(cx);
    open(cx);
    let dock = dock.borrow().clone().expect("the dock rendered");
    let trigger = record(&latest_tree(cx), "trigger").bounds;
    cx.simulate_mouse_move(trigger.center(), None, Modifiers::none());
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    let before = drawn(cx);
    let tooltip = before.tooltip.expect("the tooltip shows");
    assert_eq!(before.deferred_draws, 1);
    let rendered = renders(&view, cx);

    dock.update(cx, |_, cx| cx.notify());
    assert!(app_replayed(cx));
    assert_eq!(renders(&view, cx), rendered);
    assert_eq!(drawn(cx), before, "the tooltip and the popover are kept");
    let tip_quads = cx.update(|window, _| {
        let scale_factor = window.scale_factor();
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.bounds == tooltip.scale(scale_factor))
            .count()
    });
    assert_eq!(tip_quads, 1, "the tooltip is painted");

    // The tooltip's listeners were replayed too: leaving the trigger hides it.
    cx.simulate_mouse_move(point(px(300.), px(300.)), None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(drawn(cx).tooltip, None);
}

#[gpui::test]
fn app_notified_during_a_replayed_frame_renders_on_the_next(cx: &mut TestAppContext) {
    let (view, cx) = replayed(cx);
    let poke: Rc<RefCell<Option<Entity<Replayed>>>> = Rc::default();
    cx.update(|_, cx| {
        let poke = poke.clone();
        cx.set_inspector_renderer(Box::new(move |_, _, cx| {
            if let Some(view) = poke.borrow_mut().take() {
                view.update(cx, |_, cx| cx.notify());
            }
            div().id("dock").size_full().into_any_element()
        }))
    });
    open(cx);
    let inspector = cx.update(|window, _| window.inspector_entity().unwrap());
    let rendered = renders(&view, cx);

    // The inspector's own frame replays the app, which is notified while
    // the inspector draws.
    *poke.borrow_mut() = Some(view.clone());
    inspector.update(cx, |_, cx| cx.notify());
    assert!(poke.borrow().is_none(), "the inspector drew");
    assert!(app_replayed(cx));
    assert!(latest_frame(cx).inspector_only);
    assert_eq!(renders(&view, cx), rendered);

    // The next frame renders the app, even though the inspector asked for it.
    inspector.update(cx, |_, cx| cx.notify());
    assert!(!app_replayed(cx));
    assert_eq!(renders(&view, cx).0, rendered.0 + 1);
    let frame = latest_frame(cx);
    assert!(!frame.inspector_only);
    assert!(frame.causes.iter().any(|cause| matches!(
        cause.kind,
        CauseKind::Notify { entity, .. } if entity == view.entity_id()
    )));
    assert_eq!(
        view_outcomes(&frame)[0],
        ("Replayed", ViewOutcome::Rendered)
    );
}

#[gpui::test]
fn views_are_rendered_served_from_cache_or_replayed(cx: &mut TestAppContext) {
    let (view, cx) = replayed(cx);
    let dock = dock_renderer(cx);
    open(cx);
    let dock = dock.borrow().clone().expect("the dock rendered");
    let child = cx.update(|_, cx| view.read(cx).child.clone());

    // The app renders; its cached child has nothing new: a real cache hit.
    view.update(cx, |_, cx| cx.notify());
    let frame = latest_frame(cx);
    assert_eq!(
        view_outcomes(&frame),
        [
            ("Replayed", ViewOutcome::Rendered),
            ("Counted", ViewOutcome::Cached)
        ]
    );
    assert!(!frame.replayed_app());

    // A frame drawn for the inspector alone draws nothing of the app.
    dock.update(cx, |_, cx| cx.notify());
    let frame = latest_frame(cx);
    assert_eq!(
        view_outcomes(&frame),
        [
            ("Replayed", ViewOutcome::Replayed),
            ("Counted", ViewOutcome::Replayed)
        ]
    );
    assert!(frame.replayed_app());

    // The child notified: it renders, inside the (uncached) root.
    child.update(cx, |_, cx| cx.notify());
    assert_eq!(
        view_outcomes(&latest_frame(cx)),
        [
            ("Replayed", ViewOutcome::Rendered),
            ("Counted", ViewOutcome::Rendered)
        ]
    );
}

#[gpui::test]
fn holding_replays_the_app_until_release(cx: &mut TestAppContext) {
    let (view, cx) = replayed(cx);
    let dock = dock_renderer(cx);
    open(cx);
    let dock = dock.borrow().clone().expect("the dock rendered");
    write(cx, |capture| capture.set_holding(true));
    assert!(read(cx, |capture| capture.is_holding()));
    let before = drawn(cx);
    let rendered = renders(&view, cx);
    let generation = read(cx, |capture| capture.generation());
    let (label, child) = cx.update(|_, cx| {
        let view = view.read(cx);
        (view.label.clone(), view.child.clone())
    });

    // The app changes while held, but frames replay it as it was.
    label.update(cx, |label, cx| {
        label.0 = 5;
        cx.notify();
    });
    child.update(cx, |_, cx| cx.notify());
    view.update(cx, |_, cx| cx.notify());
    cx.update(|window, _| window.refresh());
    let frame = latest_frame(cx);
    assert!(app_replayed(cx));
    assert!(!frame.inspector_only, "the app asked for these frames");
    assert!(frame.tree.is_none());
    assert!(
        frame
            .views
            .iter()
            .all(|view| view.outcome == ViewOutcome::Replayed)
    );
    assert_eq!(renders(&view, cx), rendered);
    assert_eq!(drawn(cx), before);

    // Input still reaches the held app; what it invalidates waits too.
    cx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
    assert_eq!(cx.update(|_, cx| view.read(cx).clicks), 1);
    assert_eq!(renders(&view, cx), rendered);
    assert!(painted(cx, "label 0"));

    // Releasing renders the app once, with everything that was held.
    write(cx, |capture| capture.set_holding(false));
    assert!(!read(cx, |capture| capture.is_holding()));
    dock.update(cx, |_, cx| cx.notify());
    assert!(!app_replayed(cx));
    let released = renders(&view, cx);
    assert_eq!(released.0, rendered.0 + 1);
    assert_eq!(released.1, rendered.1 + 1, "the held child re-renders");
    assert!(painted(cx, "label 5"));
    let frame = latest_frame(cx);
    assert!(frame.tree.is_some());
    assert_eq!(
        view_outcomes(&frame),
        [
            ("Replayed", ViewOutcome::Rendered),
            ("Counted", ViewOutcome::Rendered)
        ]
    );
    assert!(read(cx, |capture| capture.generation()) > generation);

    // Released, the app replays only on the inspector's own frames again.
    dock.update(cx, |_, cx| cx.notify());
    assert!(app_replayed(cx));
    view.update(cx, |_, cx| cx.notify());
    assert!(!app_replayed(cx));
    assert_eq!(renders(&view, cx).0, released.0 + 1);
}

crate::actions!(inspector_tests, [HoldApp]);

/// A button with a hover color and a tooltip.
struct Hoverable;

impl Render for Hoverable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("button")
                .w(px(40.))
                .h(px(40.))
                .bg(blue())
                .hover(|style| style.bg(red()))
                .tooltip(|_, cx| cx.new(|_| Tip).into())
                .on_click(cx.listener(|_, _, _, _| {})),
        )
    }
}

#[gpui::test]
fn holding_from_the_keyboard_keeps_the_hover_state_on_screen(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| Hoverable);
    open(cx);
    // Held from a shortcut the way Loupe does it: a global action whose
    // handler runs once the key's dispatch is over.
    cx.update(|window, cx| {
        let handle = window.window_handle();
        cx.bind_keys([crate::KeyBinding::new("ctrl-shift-h", HoldApp, None)]);
        cx.on_action(move |_: &HoldApp, cx| {
            cx.defer(move |cx| {
                handle
                    .update(cx, |_, window, cx| window.set_inspector_holding(true, cx))
                    .ok();
            })
        });
    });
    let painted_color = |cx: &mut VisualTestContext, color: Hsla| {
        cx.update(|window, _| {
            window
                .painted_quads()
                .iter()
                .any(|quad| quad.background.as_solid() == Some(color))
        })
    };
    let button = record(&latest_tree(cx), "button").bounds;
    cx.simulate_mouse_move(button.center(), None, Modifiers::none());
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    let hovered = drawn(cx);
    assert!(hovered.tooltip.is_some(), "the tooltip shows");
    assert!(painted_color(cx, red()), "the hover color shows");

    cx.simulate_keystrokes("ctrl-shift-h");
    assert!(read(cx, |capture| capture.is_holding()));
    assert!(app_replayed(cx));
    let held = drawn(cx);
    assert_eq!(held.tooltip, hovered.tooltip, "the tooltip stays");
    assert!(painted_color(cx, red()), "and so does the hover color");
    assert_eq!(held.quads, hovered.quads);

    // Released, the app draws as the keyboard left it: no hover, no tooltip.
    cx.update(|window, cx| window.set_inspector_holding(false, cx));
    cx.run_until_parked();
    assert!(!app_replayed(cx));
    assert_eq!(drawn(cx).tooltip, None);
    assert!(!painted_color(cx, red()));
    assert!(painted_color(cx, blue()));
}

/// The inspector's UI, drawn cached like Loupe: it takes the focus on a
/// mouse down, has a button with an active style that `on_press` handles,
/// and refreshes the window on any key.
struct Controls {
    focus: FocusHandle,
    renders: usize,
    on_press: Rc<dyn Fn(&mut App)>,
}

impl Render for Controls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let on_press = self.on_press.clone();
        div()
            .id("controls")
            .size_full()
            .track_focus(&self.focus)
            .on_mouse_down(
                crate::MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
            )
            .on_key_down(|_, window, _| window.refresh())
            .child(
                div()
                    .id("press")
                    .w(px(40.))
                    .h(px(40.))
                    .bg(blue())
                    .active(|style| style.bg(red()))
                    .on_click(move |_, _, cx| on_press(cx)),
            )
    }
}

type ControlsSlot = Rc<RefCell<Option<Entity<Controls>>>>;

fn controls_dock(
    cx: &mut VisualTestContext,
    on_press: impl Fn(&mut App) + 'static,
) -> ControlsSlot {
    let dock: ControlsSlot = Rc::default();
    let slot = dock.clone();
    let on_press: Rc<dyn Fn(&mut App)> = Rc::new(on_press);
    cx.update(|_, cx| {
        cx.set_inspector_renderer(Box::new(move |inspector, _, cx| {
            let view = inspector
                .ui_state(|| {
                    cx.new(|cx| Controls {
                        focus: cx.focus_handle(),
                        renders: 0,
                        on_press: on_press.clone(),
                    })
                })
                .clone();
            *slot.borrow_mut() = Some(view.clone());
            AnyView::from(view)
                .cached(StyleRefinement::default().size_full())
                .into_any_element()
        }))
    });
    dock
}

/// Where the inspector drew its button.
fn press_position(cx: &mut VisualTestContext) -> Point<Pixels> {
    cx.update(|window, _| window.inspector_bounds().unwrap().origin) + point(px(20.), px(20.))
}

#[gpui::test]
fn the_inspectors_own_input_does_not_render_the_app(cx: &mut TestAppContext) {
    let (view, cx) = replayed(cx);
    let dock = controls_dock(cx, |_| {});
    open(cx);
    let dock = dock.borrow().clone().expect("the dock rendered");
    let dock_renders = |cx: &mut VisualTestContext| dock.read_with(cx, |dock, _| dock.renders);
    let (rendered, dock_rendered) = (renders(&view, cx), dock_renders(cx));
    let first_frame = latest_frame(cx).id;

    // A click in the dock: the button's active state and the dock taking
    // the focus refresh the window, and only the inspector draws.
    let press = press_position(cx);
    cx.simulate_click(press, Modifiers::none());
    let frames = read(cx, |capture| {
        capture
            .frames()
            .iter()
            .filter(|frame| frame.id > first_frame)
            .cloned()
            .collect::<Vec<_>>()
    });
    assert!(!frames.is_empty());
    for frame in &frames {
        assert!(frame.inspector_only, "{:?}", frame.causes);
        assert!(frame.tree.is_none(), "the app was replayed");
        assert!(frame.causes.iter().all(|cause| cause.from_inspector));
    }
    assert_eq!(renders(&view, cx), rendered, "no app view rendered");
    assert!(
        dock_renders(cx) > dock_rendered,
        "the dock showed its press"
    );
    assert!(cx.update(|window, cx| dock.read(cx).focus.is_focused(window)));

    // So does a key the focused dock handles by refreshing the window.
    let dock_rendered = dock_renders(cx);
    cx.simulate_keystrokes("x");
    assert!(latest_frame(cx).inspector_only);
    assert_eq!(renders(&view, cx), rendered);
    assert_eq!(dock_renders(cx), dock_rendered + 1);

    // The app's own refresh still renders the app alone.
    cx.update(|window, _| window.refresh());
    assert!(!latest_frame(cx).inspector_only);
    assert_eq!(renders(&view, cx).0, rendered.0 + 1);
    assert_eq!(dock_renders(cx), dock_rendered + 1);
}

#[gpui::test]
fn the_inspectors_input_still_renders_the_app_views_it_changes(cx: &mut TestAppContext) {
    let (view, cx) = parent(cx, true);
    let child = view.read_with(cx, |parent, _| parent.child.entity_id());
    let notified = view.clone();
    controls_dock(cx, move |cx| notified.update(cx, |_, cx| cx.notify()));
    open(cx);

    // The dock's button notifies the app's root view: it renders, but its
    // cached child is reused, since the dock's refreshes stay the
    // inspector's.
    let press = press_position(cx);
    cx.simulate_click(press, Modifiers::none());
    let frame = read(cx, |capture| {
        capture
            .frames()
            .iter()
            .rev()
            .find(|frame| !frame.inspector_only)
            .cloned()
    })
    .expect("the app drew");
    let outcome = |entity: EntityId| {
        frame
            .views
            .iter()
            .find(|span| span.entity == entity)
            .map(|span| span.outcome)
    };
    assert_eq!(outcome(view.entity_id()), Some(ViewOutcome::Rendered));
    assert_eq!(outcome(child), Some(ViewOutcome::Cached));
    let notify = frame
        .causes
        .iter()
        .find(|cause| {
            matches!(cause.kind, CauseKind::Notify { entity, .. } if entity == view.entity_id())
        })
        .expect("the notify caused the frame");
    assert!(!notify.from_inspector);
    assert!(
        frame
            .causes
            .iter()
            .filter(|cause| cause.kind == CauseKind::Refresh)
            .all(|cause| cause.from_inspector)
    );
}

#[gpui::test]
fn resizing_renders_a_held_app(cx: &mut TestAppContext) {
    let (view, cx) = replayed(cx);
    open(cx);
    write(cx, |capture| capture.set_holding(true));
    let rendered = renders(&view, cx);
    let child = cx.update(|_, cx| view.read(cx).child.clone());
    child.update(cx, |_, cx| cx.notify());
    assert!(app_replayed(cx));
    assert_eq!(renders(&view, cx), rendered);

    cx.simulate_resize(size(px(900.), px(700.)));
    assert!(!app_replayed(cx));
    assert!(read(cx, |capture| capture.is_holding()));
    assert_eq!(renders(&view, cx).0, rendered.0 + 1);
    assert_eq!(
        renders(&view, cx).1,
        rendered.1 + 1,
        "the child notified while held renders with the app"
    );
    let tree = latest_tree(cx);
    assert_eq!(tree.get(tree.roots[0]).unwrap().bounds, app_bounds(cx));

    // Still held: the app replays again at its new size.
    view.update(cx, |_, cx| cx.notify());
    assert!(app_replayed(cx));
    assert_eq!(renders(&view, cx).0, rendered.0 + 1);
}

#[gpui::test]
fn the_app_renders_when_the_capture_asks_for_more(cx: &mut TestAppContext) {
    let (view, cx) = replayed(cx);
    let dock = dock_renderer(cx);
    open(cx);
    let dock = dock.borrow().clone().expect("the dock rendered");
    write(cx, |capture| {
        capture.config_mut().level = CaptureLevel::Frames
    });
    view.update(cx, |_, cx| cx.notify());
    assert!(latest_frame(cx).tree.is_none());
    let rendered = renders(&view, cx);

    // An overlay needs a tree the last render did not record.
    write(cx, |capture| {
        capture.overlay_mut().modes = OverlayModes::OUTLINES
    });
    dock.update(cx, |_, cx| cx.notify());
    assert!(!app_replayed(cx));
    assert_eq!(renders(&view, cx).0, rendered.0 + 1);
    assert!(read(cx, |capture| capture.recorder.live_tree.is_some()));
    dock.update(cx, |_, cx| cx.notify());
    assert!(app_replayed(cx));

    // Details need a render too.
    write(cx, |capture| {
        capture.config_mut().level = CaptureLevel::Full
    });
    dock.update(cx, |_, cx| cx.notify());
    assert!(!app_replayed(cx));
    assert!(record(&latest_tree(cx), "button").details.is_some());
    dock.update(cx, |_, cx| cx.notify());
    assert!(app_replayed(cx));
    assert_eq!(renders(&view, cx).0, rendered.0 + 2);
}

/// A model the inspector's UI creates along with it.
struct DockModel(usize);

/// The inspector's UI as Loupe builds it: created on the inspector's first
/// draw, with a model it reads and observes (a cached view only renders
/// again when it is notified), and drawn cached.
struct DockView {
    model: Entity<DockModel>,
    renders: usize,
    _observation: gpui::Subscription,
}

impl Render for DockView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let value = self.model.read(cx).0;
        div().size_full().child(format!("dock {value}"))
    }
}

type CreatedDock = Rc<RefCell<Option<(Entity<DockView>, Entity<DockModel>)>>>;

fn created_dock(cx: &mut VisualTestContext) -> CreatedDock {
    let dock: CreatedDock = Rc::default();
    let slot = dock.clone();
    cx.update(|_, cx| {
        cx.set_inspector_renderer(Box::new(move |inspector, _, cx| {
            let (view, model) = inspector
                .ui_state(|| {
                    let model = cx.new(|_| DockModel(0));
                    let view = cx.new(|cx| DockView {
                        model: model.clone(),
                        renders: 0,
                        _observation: cx.observe(&model, |_, _, cx| cx.notify()),
                    });
                    (view, model)
                })
                .clone();
            *slot.borrow_mut() = Some((view.clone(), model));
            AnyView::from(view)
                .cached(StyleRefinement::default().size_full())
                .into_any_element()
        }))
    });
    dock
}

#[gpui::test]
fn the_inspectors_ui_stays_invalidated_through_a_stream_of_app_frames(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Nested::new());
    let dock = created_dock(cx);
    open(cx);
    let (dock, model) = dock.borrow().clone().expect("the dock rendered");
    let dock_renders = |cx: &mut VisualTestContext| dock.read_with(cx, |dock, _| dock.renders);
    let rendered = dock_renders(cx);

    // Frame after frame of the app: the dock's cached UI is reused.
    for _ in 0..3 {
        view.update(cx, |_, cx| cx.notify());
        assert!(!latest_frame(cx).inspector_only);
    }
    assert_eq!(dock_renders(cx), rendered);

    // What the dock reads changes: it renders on the next frame.
    model.update(cx, |model, cx| {
        model.0 = 1;
        cx.notify();
    });
    assert_eq!(dock_renders(cx), rendered + 1);
    assert!(painted(cx, "dock 1"));

    // So does notifying the dock itself, after more of the app's frames.
    for _ in 0..3 {
        view.update(cx, |_, cx| cx.notify());
    }
    dock.update(cx, |_, cx| cx.notify());
    assert_eq!(dock_renders(cx), rendered + 2);
}

/// A view of as many lines of text as it holds.
struct Lines(usize);

impl Render for Lines {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .children((0..self.0).map(|ix| format!("line {ix}")))
    }
}

/// An app whose (uncached) root draws some text and a cached view of lines.
struct Host {
    lines: Entity<Lines>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("host").child(
            self.lines
                .clone()
                .cached(StyleRefinement::default().w(px(200.)).h(px(200.))),
        )
    }
}

/// Everything the latest frame painted, in paint order.
fn painted_scene(cx: &mut VisualTestContext) -> (String, Vec<crate::PaintedText>) {
    cx.update(|window, _| {
        (
            format!("{:?}", window.painted_quads()),
            window.painted_text().to_vec(),
        )
    })
}

#[gpui::test]
fn a_cached_view_is_reused_correctly_after_a_replayed_frame(cx: &mut TestAppContext) {
    let (host, cx) = cx.add_window_view(|_, cx| Host {
        lines: cx.new(|_| Lines(3)),
    });
    // The inspector's UI lays out as many lines as `dock_lines` says, so
    // its share of the frame's line layouts changes from frame to frame.
    let dock_lines = Rc::new(std::cell::Cell::new(20));
    cx.update(|_, cx| {
        let dock_lines = dock_lines.clone();
        cx.set_inspector_renderer(Box::new(move |_, _, _| {
            div()
                .children((0..dock_lines.get()).map(|ix| format!("dock {ix}")))
                .into_any_element()
        }))
    });
    open(cx);
    let inspector = cx.update(|window, _| window.inspector_entity().unwrap());
    let lines = host.read_with(cx, |host, _| host.lines.clone());

    // The cached view renders, painting after the inspector's 20 lines.
    lines.update(cx, |_, cx| cx.notify());
    // The inspector draws alone, with fewer lines: the app is replayed.
    dock_lines.set(2);
    inspector.update(cx, |_, cx| cx.notify());
    assert!(app_replayed(cx));
    // The app draws again and reuses the cached view from the replayed frame.
    host.update(cx, |_, cx| cx.notify());
    assert!(!app_replayed(cx));
    let frame = latest_frame(cx);
    assert_eq!(
        view_outcomes(&frame),
        [
            ("Host", ViewOutcome::Rendered),
            ("Lines", ViewOutcome::Cached)
        ]
    );
    let reused = painted_scene(cx);
    assert!(
        reused.1.iter().any(|line| line.text == "line 2"),
        "{:?}",
        reused.1
    );

    // Exactly what a fresh render paints.
    cx.update(|window, _| window.refresh_with_inspector());
    assert_eq!(painted_scene(cx), reused);
}

/// Labeled lines of text.
struct Labeled {
    label: &'static str,
    lines: usize,
}

impl Render for Labeled {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let label = self.label;
        div()
            .flex()
            .flex_col()
            .children((0..self.lines).map(move |ix| format!("{label} {ix}")))
    }
}

/// A cached view with lines of its own above a nested cached view.
struct Nest {
    lines: usize,
    inner: Entity<Labeled>,
}

impl Render for Nest {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(80.))
                    .overflow_hidden()
                    .children((0..self.lines).map(|ix| format!("nest {ix}"))),
            )
            .child(
                self.inner
                    .clone()
                    .cached(StyleRefinement::default().w(px(200.)).h(px(80.))),
            )
    }
}

/// The app's (uncached) root: lines of its own, a [`Nest`] and a side view.
struct Tree {
    lines: usize,
    nest: Entity<Nest>,
    side: Entity<Labeled>,
}

impl Render for Tree {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(80.))
                    .overflow_hidden()
                    .children((0..self.lines).map(|ix| format!("root {ix}"))),
            )
            .child(
                self.nest
                    .clone()
                    .cached(StyleRefinement::default().w(px(300.)).h(px(200.))),
            )
            .child(
                self.side
                    .clone()
                    .cached(StyleRefinement::default().w(px(200.)).h(px(80.))),
            )
    }
}

/// One step of [`replays_keep_every_cached_view_in_step`].
#[derive(Clone, Debug)]
enum Step {
    /// A view of the app changes its line count.
    Root(usize),
    Nest(usize),
    Inner(usize),
    Side(usize),
    /// The inspector draws alone, with this many lines of its own.
    Inspector(usize),
    /// The app is held or released.
    Hold(bool),
}

/// How many lines each view of the [`Tree`] draws.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LineCounts {
    root: usize,
    nest: usize,
    inner: usize,
    side: usize,
}

impl LineCounts {
    /// The app's text, in paint order.
    fn text(&self) -> Vec<String> {
        [
            ("root", self.root),
            ("nest", self.nest),
            ("inner", self.inner),
            ("side", self.side),
        ]
        .into_iter()
        .flat_map(|(label, lines)| (0..lines).map(move |ix| format!("{label} {ix}")))
        .collect()
    }
}

fn step_strategy() -> impl Strategy<Value = Step> {
    let lines = || 0..5usize;
    prop_oneof![
        lines().prop_map(Step::Root),
        lines().prop_map(Step::Nest),
        lines().prop_map(Step::Inner),
        lines().prop_map(Step::Side),
        (0..30usize).prop_map(Step::Inspector),
        any::<bool>().prop_map(Step::Hold),
    ]
}

proptest! {
    #![proptest_config(gpui::apply_seed_to_proptest_config(ProptestConfig::with_cases(64)))]

    /// Whatever mix of app frames, frames the inspector draws alone (which
    /// replay the app) and holds, every frame paints the app as it is (as it
    /// was when held), and reusing cached views never reads out of range.
    #[test]
    fn replays_keep_every_cached_view_in_step(steps in vec(step_strategy(), 1..32)) {
        gpui::run_test_once(0, Box::new(move |dispatcher| {
            let mut cx = TestAppContext::build(dispatcher, None);
            check_replays(&mut cx, steps);
            cx.quit();
        }));
    }
}

fn check_replays(cx: &mut TestAppContext, steps: Vec<Step>) {
    let (tree, cx) = cx.add_window_view(|_, cx| Tree {
        lines: 1,
        nest: cx.new(|cx| Nest {
            lines: 1,
            inner: cx.new(|_| Labeled {
                label: "inner",
                lines: 1,
            }),
        }),
        side: cx.new(|_| Labeled {
            label: "side",
            lines: 1,
        }),
    });
    let dock_lines = Rc::new(std::cell::Cell::new(3));
    cx.update(|_, cx| {
        let dock_lines = dock_lines.clone();
        cx.set_inspector_renderer(Box::new(move |_, _, _| {
            div()
                .children((0..dock_lines.get()).map(|ix| format!("dock {ix}")))
                .into_any_element()
        }))
    });
    open(cx);
    let inspector = cx.update(|window, _| window.inspector_entity().unwrap());
    let (nest, side) = tree.read_with(cx, |tree, _| (tree.nest.clone(), tree.side.clone()));
    let inner = nest.read_with(cx, |nest, _| nest.inner.clone());
    let mut lines = LineCounts {
        root: 1,
        nest: 1,
        inner: 1,
        side: 1,
    };
    let mut shown = lines;
    let mut held = false;
    for step in steps {
        match step {
            Step::Root(count) => {
                lines.root = count;
                tree.update(cx, |tree, cx| {
                    tree.lines = count;
                    cx.notify();
                });
            }
            Step::Nest(count) => {
                lines.nest = count;
                nest.update(cx, |nest, cx| {
                    nest.lines = count;
                    cx.notify();
                });
            }
            Step::Inner(count) => {
                lines.inner = count;
                inner.update(cx, |inner, cx| {
                    inner.lines = count;
                    cx.notify();
                });
            }
            Step::Side(count) => {
                lines.side = count;
                side.update(cx, |side, cx| {
                    side.lines = count;
                    cx.notify();
                });
            }
            Step::Inspector(count) => {
                dock_lines.set(count);
                inspector.update(cx, |_, cx| cx.notify());
            }
            Step::Hold(hold) => {
                held = hold;
                cx.update(|window, cx| window.set_inspector_holding(hold, cx));
            }
        }
        if !held {
            shown = lines;
        }
        let painted: Vec<String> = cx.update(|window, _| {
            window
                .painted_text()
                .iter()
                .map(|line| line.text.to_string())
                .filter(|text| !text.starts_with("dock"))
                .collect()
        });
        assert_eq!(painted, shown.text(), "after {step:?}");
    }
}
