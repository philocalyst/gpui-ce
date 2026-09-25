use crate::{
    self as gpui, AppContext as _, Context, Entity, EntityId, EventEmitter, IntoElement,
    ParentElement as _, Render, Subscription, TestAppContext, VisualTestContext, Window, div,
    inspector::{EntityInfo, NotifyStats},
    px, size,
};
use std::{any::type_name, panic::Location};

/// A model other entities observe.
struct Counter;

/// A model other entities subscribe to.
struct Feed;

struct Ping;

impl EventEmitter<Ping> for Feed {}

/// A view rendered by [`Root`].
struct Leaf;

impl Render for Leaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The window's root view: observes the counter once, subscribes to the feed
/// twice and renders a leaf view.
struct Root {
    counter: Entity<Counter>,
    feed: Entity<Feed>,
    leaf: Entity<Leaf>,
    subscriptions: Vec<Subscription>,
}

impl Root {
    fn new(cx: &mut Context<Self>) -> Self {
        let counter = cx.new(|_| Counter);
        let feed = cx.new(|_| Feed);
        let leaf = cx.new(|_| Leaf);
        let subscriptions = vec![
            cx.observe(&counter, |_, _, _| {}),
            cx.subscribe(&feed, |_, _, _: &Ping, _| {}),
            cx.subscribe(&feed, |_, _, _: &Ping, _| {}),
        ];
        Self {
            counter,
            feed,
            leaf,
            subscriptions,
        }
    }
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(self.leaf.clone())
    }
}

fn open_root(cx: &mut TestAppContext) -> (Entity<Root>, VisualTestContext) {
    let window = cx.open_window(size(px(800.), px(600.)), |_, cx| Root::new(cx));
    let root = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    (root, cx)
}

/// The registry as seen from outside any window update.
fn app_entities(cx: &VisualTestContext) -> Vec<EntityInfo> {
    cx.cx.update(|cx| cx.inspector_entities())
}

fn find(entities: &[EntityInfo], id: EntityId) -> &EntityInfo {
    entities
        .iter()
        .find(|info| info.id == id)
        .unwrap_or_else(|| panic!("entity {id} is not listed"))
}

#[gpui::test]
fn registry_reports_types_handles_listeners_and_kinds(cx: &mut TestAppContext) {
    let (root, mut cx) = open_root(cx);
    let (counter, feed, leaf) = root.read_with(&cx, |root, _| {
        (root.counter.clone(), root.feed.clone(), root.leaf.clone())
    });
    cx.update(|_, cx| {
        cx.observe(&counter, |_, _| {}).detach();
        cx.observe(&counter, |_, _| {}).detach();
    });

    let entities = app_entities(&cx);

    let ids: Vec<EntityId> = entities.iter().map(|info| info.id).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "sorted by id");

    let counter_info = find(&entities, counter.entity_id());
    assert_eq!(counter_info.type_name, type_name::<Counter>());
    assert_eq!(
        counter_info.strong_count, 2,
        "held by the root and the test"
    );
    assert_eq!(counter_info.observers, 3);
    assert_eq!(counter_info.subscribers, 0);
    assert!(!counter_info.is_view);
    assert_eq!(counter_info.notifies, 0);
    assert_eq!(counter_info.last_notify_site, None);

    let feed_info = find(&entities, feed.entity_id());
    assert_eq!(feed_info.type_name, type_name::<Feed>());
    assert_eq!(feed_info.strong_count, 2);
    assert_eq!(feed_info.observers, 0);
    assert_eq!(feed_info.subscribers, 2);
    assert!(!feed_info.is_view);

    let leaf_info = find(&entities, leaf.entity_id());
    assert_eq!(leaf_info.type_name, type_name::<Leaf>());
    assert_eq!(leaf_info.strong_count, 2);
    assert_eq!((leaf_info.observers, leaf_info.subscribers), (0, 0));
    assert!(leaf_info.is_view);

    let root_info = find(&entities, root.entity_id());
    assert_eq!(root_info.type_name, type_name::<Root>());
    assert!(root_info.is_view);
}

#[gpui::test]
fn dropped_entities_and_their_listeners_disappear(cx: &mut TestAppContext) {
    let (root, mut cx) = open_root(cx);
    let (counter, feed) = root.read_with(&cx, |root, _| (root.counter.clone(), root.feed.clone()));
    let temporary = cx.update(|_, cx| {
        let temporary = cx.new(|_| Counter);
        cx.observe(&temporary, |_, _| {}).detach();
        temporary
    });
    let temporary_id = temporary.entity_id();
    assert_eq!(find(&app_entities(&cx), temporary_id).observers, 1);

    drop(temporary);
    // Releases the temporary model and drops the root's subscriptions.
    cx.update(|_, cx| root.update(cx, |root, _| root.subscriptions.clear()));

    let entities = app_entities(&cx);
    assert!(entities.iter().all(|info| info.id != temporary_id));
    assert_eq!(find(&entities, counter.entity_id()).observers, 0);
    assert_eq!(find(&entities, feed.entity_id()).subscribers, 0);
}

#[gpui::test]
fn only_the_window_itself_sees_its_views_while_it_is_updated(cx: &mut TestAppContext) {
    let (root, mut cx) = open_root(cx);
    let root_id = root.entity_id();

    let (from_window, from_app) = cx.update(|window, cx| {
        (
            find(&window.inspector_entities(cx), root_id).is_view,
            find(&cx.inspector_entities(), root_id).is_view,
        )
    });

    assert!(from_window);
    assert!(!from_app, "the window is leased while it is being updated");
    assert!(find(&app_entities(&cx), root_id).is_view);
}

#[gpui::test]
fn notifies_are_summed_across_inspected_windows(cx: &mut TestAppContext) {
    let (first_root, mut first) = open_root(cx);
    let (_, mut second) = open_root(cx);
    let id = first_root.read_with(&first, |root, _| root.counter.entity_id());
    let first_site = Location::caller();
    let second_site = Location::caller();
    for (cx, total, site) in [(&mut first, 3, first_site), (&mut second, 4, second_site)] {
        cx.update(|window, cx| {
            window.toggle_inspector(cx);
            let capture = window.inspector_capture_mut().unwrap();
            capture.notify_stats.insert(
                id,
                NotifyStats {
                    total,
                    last_site: Some(site),
                    ..Default::default()
                },
            );
        });
    }

    let from_first = first.update(|window, cx| find(&window.inspector_entities(cx), id).clone());
    let from_second = second.update(|window, cx| find(&window.inspector_entities(cx), id).clone());
    let from_app = find(&app_entities(&first), id).clone();

    assert_eq!(from_first.notifies, 7);
    assert_eq!(from_first.last_notify_site, Some(first_site));
    assert_eq!(from_second.notifies, 7);
    assert_eq!(from_second.last_notify_site, Some(second_site));
    assert_eq!(from_app.notifies, 7);
}
