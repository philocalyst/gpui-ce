//! The inspector's input capture costs nothing while the inspector is closed:
//! dispatching pointer input, which gpui otherwise handles without allocating,
//! still allocates nothing.
#![cfg(feature = "test-support")]

use gpui::{
    Context, IntoElement, Modifiers, MouseMoveEvent, PlatformInput, Render, ScrollDelta,
    ScrollWheelEvent, Styled as _, TestAppContext, Window, WindowHandle, div, point, px,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

/// Counts allocations made on the current thread while counting is enabled.
struct CountingAllocator;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn count_allocation() {
    if COUNTING.get() {
        ALLOCATIONS.set(ALLOCATIONS.get() + 1);
    }
}

// SAFETY: defers every operation to the system allocator unchanged.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        // SAFETY: forwarded with the caller's guarantees.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        // SAFETY: forwarded with the caller's guarantees.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_allocation();
        // SAFETY: forwarded with the caller's guarantees.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded with the caller's guarantees.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Allocations made by `f` on this thread.
fn allocations<R>(f: impl FnOnce() -> R) -> usize {
    ALLOCATIONS.set(0);
    COUNTING.set(true);
    let result = f();
    COUNTING.set(false);
    drop(result);
    ALLOCATIONS.get()
}

struct Static;

impl Render for Static {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn pointer_events() -> [PlatformInput; 2] {
    let position = point(px(20.), px(30.));
    [
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::none(),
        }),
        PlatformInput::ScrollWheel(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        }),
    ]
}

/// Allocations made while dispatching each pointer event, after a warm-up.
fn dispatch_allocations(window: WindowHandle<Static>, cx: &mut TestAppContext) -> Vec<usize> {
    window
        .update(cx, |_, window, cx| {
            for event in pointer_events() {
                window.dispatch_event(event, cx);
            }
            pointer_events()
                .into_iter()
                .map(|event| allocations(|| window.dispatch_event(event, cx)))
                .collect()
        })
        .unwrap()
}

#[gpui::test]
fn closed_inspector_adds_no_allocations_to_dispatch(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Static);
    cx.run_until_parked();

    assert_eq!(dispatch_allocations(window, cx), [0, 0]);

    // The counter does see the capture's allocations once the inspector is open.
    window
        .update(cx, |_, window, cx| window.toggle_inspector(cx))
        .unwrap();
    cx.run_until_parked();
    assert!(
        dispatch_allocations(window, cx)
            .iter()
            .all(|&count| count > 0)
    );
}
