#![cfg(target_os = "macos")]

use anyhow::{Context as _, ensure};
use gpui::{
    App, Bounds, Context, DevicePixels, IntoElement, NativeSurface, PlatformSurfaceHandle, Render,
    Window, WindowBounds, WindowComposition, WindowOptions, deferred, div, point, prelude::*, px,
    rgb, size,
};
use gpui_ce_macos::MacPlatform;
use objc2::{
    class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool},
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::RawWindowHandle;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VerifyStage {
    Mounted,
    Resized,
    Reparented,
    Promoted,
    Done,
}

struct NativeComposition {
    parents: Option<[NativeSurface; 2]>,
    container: Option<NativeSurface>,
    editor: Option<NativeSurface>,
    field: Option<Retained<AnyObject>>,
    editor_parent: usize,
    container_removed: bool,
    parent_width: f32,
    overlay_clicks: usize,
    verify_stage: Option<VerifyStage>,
    verify_callback_scheduled: bool,
    verify_warmup_complete: bool,
}

impl NativeComposition {
    fn new(verify: bool) -> Self {
        Self {
            parents: None,
            container: None,
            editor: None,
            field: None,
            editor_parent: 0,
            container_removed: false,
            parent_width: 360.,
            overlay_clicks: 0,
            verify_stage: verify.then_some(VerifyStage::Mounted),
            verify_callback_scheduled: false,
            verify_warmup_complete: false,
        }
    }

    fn points(scale_factor: f32, x: f32, y: f32, width: f32, height: f32) -> Bounds<DevicePixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height))).to_device_pixels(scale_factor)
    }

    fn mount_editor(surface: &NativeSurface) -> Retained<AnyObject> {
        let PlatformSurfaceHandle::Window(handle) = surface.platform_handle();
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            panic!("native composition example requires an AppKit view handle")
        };
        let parent = handle.ns_view.as_ptr().cast::<AnyObject>();
        unsafe {
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(328., 38.));
            let field: *mut AnyObject = msg_send![class!(NSTextField), alloc];
            let field: *mut AnyObject = msg_send![field, initWithFrame: frame];
            assert!(!field.is_null(), "AppKit failed to create NSTextField");
            let field = Retained::from_raw(field).expect("initialized NSTextField");
            let text = NSString::from_str("Type here; this is a real native NSTextField");
            let _: () = msg_send![&*field, setStringValue: &*text];
            let _: () = msg_send![parent, addSubview: &*field];
            field
        }
    }

    fn field_view(&self) -> *mut AnyObject {
        Retained::as_ptr(self.field.as_ref().expect("editor view was created"))
            .cast_mut()
            .cast()
    }

    fn ensure_surfaces(&mut self, window: &mut Window) {
        if self.parents.is_some() {
            return;
        }
        let scale_factor = window.scale_factor();
        let composition = window
            .enable_window_composition()
            .expect("macOS native composition must initialize; there is no flat fallback");
        let parents = [
            composition
                .create_native_surface(Self::points(scale_factor, 36., 116., 360., 180.), None)
                .expect("create the left native composition container"),
            composition
                .create_native_surface(Self::points(scale_factor, 464., 116., 360., 180.), None)
                .expect("create the right native composition container"),
        ];
        let container = composition
            .create_native_surface(
                Self::points(scale_factor, 52., 156., 328., 54.),
                Some(parents[0].id().into()),
            )
            .expect("create the nested native container");
        let editor = composition
            .create_native_surface(
                Self::points(scale_factor, 52., 156., 328., 38.),
                Some(container.id().into()),
            )
            .expect("create the native text-field surface");
        self.field = Some(Self::mount_editor(&editor));
        self.parents = Some(parents);
        self.container = Some(container);
        self.editor = Some(editor);
    }

    fn apply_tree(
        &self,
        scale_factor: f32,
        composition: &WindowComposition<'_>,
    ) -> anyhow::Result<()> {
        let parents = self
            .parents
            .as_ref()
            .context("native parents were not created")?;
        let container = self
            .container
            .as_ref()
            .context("native container was not created")?;
        let editor = self
            .editor
            .as_ref()
            .context("native editor was not created")?;
        for (index, parent) in parents.iter().enumerate() {
            let x = if index == 0 { 36. } else { 464. };
            composition.set_bounds(
                parent.id(),
                Self::points(
                    scale_factor,
                    x,
                    116.,
                    if index == 0 { self.parent_width } else { 360. },
                    180.,
                ),
            )?;
        }

        let editor_x = if self.editor_parent == 0 { 52. } else { 480. };
        let container_parent = parents[self.editor_parent].id();
        if !self.container_removed {
            composition.set_bounds(
                container.id(),
                Self::points(scale_factor, editor_x, 156., 328., 54.),
            )?;
            composition.reparent(container.id(), Some(container_parent.into()))?;
            composition.set_bounds(
                editor.id(),
                Self::points(scale_factor, editor_x, 156., 328., 38.),
            )?;
            composition.reparent(editor.id(), Some(container.id().into()))?;
        } else {
            composition.set_bounds(
                editor.id(),
                Self::points(scale_factor, editor_x, 156., 328., 38.),
            )?;
            composition.reparent(editor.id(), Some(container_parent.into()))?;
        }
        Ok(())
    }

    fn verify_after_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.verify_callback_scheduled = false;
        let Some(stage) = self.verify_stage else {
            return;
        };
        // on_next_frame callbacks run just before GPUI draws and presents that frame.
        // Wait one turn so the first assertion observes the mounted AppKit graph.
        if !self.verify_warmup_complete {
            self.verify_warmup_complete = true;
            self.verify_callback_scheduled = true;
            cx.on_next_frame(window, |this, window, cx| {
                this.verify_after_frame(window, cx)
            });
            return;
        }
        if let Err(error) = self.verify_presented_tree(stage) {
            eprintln!("native composition verification failed at {stage:?}: {error:#}");
            std::process::exit(1);
        }

        match stage {
            VerifyStage::Mounted => {
                self.parent_width = 420.;
                self.verify_stage = Some(VerifyStage::Resized);
            }
            VerifyStage::Resized => {
                self.editor_parent = 1;
                self.verify_stage = Some(VerifyStage::Reparented);
            }
            VerifyStage::Reparented => {
                let container = self
                    .container
                    .as_ref()
                    .expect("created before presentation");
                if let Err(error) = window
                    .enable_window_composition()
                    .and_then(|composition| composition.remove_surface(container.id()))
                {
                    eprintln!("native composition remove failed: {error:#}");
                    std::process::exit(1);
                }
                self.container_removed = true;
                self.verify_stage = Some(VerifyStage::Promoted);
            }
            VerifyStage::Promoted => {
                self.verify_stage = Some(VerifyStage::Done);
                println!(
                    "native composition verified: mount, resize, reparent, remove/promote, hit-test passthrough, GPUI forwarding, and responder handoff"
                );
                cx.quit();
                return;
            }
            VerifyStage::Done => return,
        }
        window.refresh();
        cx.notify();
    }

    fn verify_presented_tree(&self, stage: VerifyStage) -> anyhow::Result<()> {
        let parents = self
            .parents
            .as_ref()
            .context("native parents were not created")?;
        let container = self
            .container
            .as_ref()
            .context("container was not created")?;
        let editor = self.editor.as_ref().context("editor was not created")?;
        let left = native_view(&parents[0])?;
        let right = native_view(&parents[1])?;
        let container_view = native_view(container)?;
        let editor_view = native_view(editor)?;
        let root = unsafe { superview(left) };
        let right_root = unsafe { superview(right) };
        ensure!(
            !root.is_null() && root == right_root,
            "root containers are not mounted as siblings (left parent {root:p}, right parent {right_root:p})"
        );
        ensure!(
            unsafe { superview(self.field_view()) } == editor_view,
            "NSTextField is not a child of the native editor slot"
        );

        match stage {
            VerifyStage::Mounted => {
                ensure!(
                    unsafe { superview(container_view) } == left,
                    "container did not mount under the left root"
                );
                ensure!(
                    unsafe { superview(editor_view) } == container_view,
                    "editor did not mount under the nested container"
                );
                assert_view_frame(left, (36., 124., 360., 180.))?;
                assert_view_frame(container_view, (16., 86., 328., 54.))?;
                assert_view_frame(editor_view, (0., 16., 328., 38.))?;
                assert_view_frame(self.field_view(), (0., 0., 328., 38.))?;
                let overlay = unsafe { composition_overlay_view(root)? };
                let left_order = unsafe { subview_index(root, left) }
                    .context("left native root is missing from AppKit subviews")?;
                let right_order = unsafe { subview_index(root, right) }
                    .context("right native root is missing from AppKit subviews")?;
                let overlay_order = unsafe { subview_index(root, overlay) }
                    .context("GPUI overlay is missing from AppKit subviews")?;
                ensure!(
                    left_order < right_order && right_order < overlay_order,
                    "AppKit sibling order does not match composition preorder"
                );
                self.verify_input_routing(root)?;
            }
            VerifyStage::Resized => {
                ensure!(
                    unsafe { superview(container_view) } == left,
                    "resize moved the container unexpectedly"
                );
                assert_view_frame(left, (36., 124., 420., 180.))?;
                assert_view_frame(container_view, (16., 86., 328., 54.))?;
            }
            VerifyStage::Reparented => {
                ensure!(
                    unsafe { superview(container_view) } == right,
                    "container did not reparent to the right root"
                );
                ensure!(
                    unsafe { superview(editor_view) } == container_view,
                    "editor left its nested container during reparent"
                );
                assert_view_frame(container_view, (16., 86., 328., 54.))?;
                assert_view_frame(editor_view, (0., 16., 328., 38.))?;
            }
            VerifyStage::Promoted => {
                let promoted_parent = unsafe { superview(editor_view) };
                ensure!(
                    promoted_parent == right,
                    "removing the container did not promote its editor child"
                );
                let removed_parent = unsafe { superview(container_view) };
                ensure!(
                    removed_parent.is_null(),
                    "removed native container remains in the AppKit tree"
                );
                assert_view_frame(editor_view, (16., 102., 328., 38.))?;
                let root_window: *mut AnyObject = unsafe { msg_send![root, window] };
                ensure!(
                    !root_window.is_null(),
                    "composition root is not attached to a window"
                );
            }
            VerifyStage::Done => {}
        }
        Ok(())
    }

    fn verify_input_routing(&self, root: *mut AnyObject) -> anyhow::Result<()> {
        let overlay = unsafe { composition_overlay_view(root)? };
        let native_window: *mut AnyObject = unsafe { msg_send![root, window] };
        ensure!(
            !native_window.is_null(),
            "composition root is not attached to a window"
        );

        let accepted: Bool =
            unsafe { msg_send![native_window, makeFirstResponder: self.field_view()] };
        ensure!(
            accepted.as_bool(),
            "AppKit refused the native text field as first responder"
        );
        ensure!(
            unsafe { native_field_has_focus(native_window, self.field_view()) },
            "native text field did not receive focus (first responder {:p}, field {:p})",
            unsafe { first_responder(native_window) },
            self.field_view()
        );

        // The overlay is a full-window view. A point within the deferred GPUI button returns the
        // GPUI root; an uncovered point over the NSTextField returns nil so AppKit keeps searching.
        let covered: *mut AnyObject =
            unsafe { msg_send![overlay, hitTest: NSPoint::new(120., 420. - 200.)] };
        ensure!(
            covered == root,
            "covered overlay input did not route to the GPUI root"
        );
        ensure!(
            unsafe { first_responder(native_window) } == root,
            "GPUI did not regain key focus after an overlay hit"
        );

        let accepted: Bool =
            unsafe { msg_send![native_window, makeFirstResponder: self.field_view()] };
        ensure!(
            accepted.as_bool(),
            "AppKit refused to restore native text-field focus"
        );
        let passthrough: *mut AnyObject =
            unsafe { msg_send![overlay, hitTest: NSPoint::new(300., 420. - 170.)] };
        ensure!(
            passthrough.is_null(),
            "uncovered native text-field input was intercepted by GPUI"
        );
        ensure!(
            unsafe { native_field_has_focus(native_window, self.field_view()) },
            "native text field lost focus outside the GPUI overlay region"
        );
        Ok(())
    }
}

impl Render for NativeComposition {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_surfaces(window);
        let scale_factor = window.scale_factor();
        let composition = window
            .enable_window_composition()
            .expect("composition remains enabled");
        self.apply_tree(scale_factor, &composition)
            .expect("native composition tree must remain valid");

        let overlay_clicks = self.overlay_clicks;
        let mut controls = div().flex().gap_3().child(
            div()
                .id("move-native-subtree")
                .px_3()
                .py_2()
                .rounded_md()
                .bg(rgb(0x315c94))
                .cursor_pointer()
                .child("Move native subtree")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.editor_parent ^= 1;
                    window.refresh();
                    cx.notify();
                })),
        );
        controls = controls.child(
            div()
                .id("resize-left-container")
                .px_3()
                .py_2()
                .rounded_md()
                .bg(rgb(0x315c94))
                .cursor_pointer()
                .child("Resize left container")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.parent_width = if this.parent_width < 420. { 420. } else { 360. };
                    window.refresh();
                    cx.notify();
                })),
        );
        if !self.container_removed {
            controls = controls.child(
                div()
                    .id("remove-and-promote")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(0x315c94))
                    .cursor_pointer()
                    .child("Remove and promote")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let container = this.container.as_ref().expect("created before render");
                        window
                            .enable_window_composition()
                            .and_then(|composition| composition.remove_surface(container.id()))
                            .expect("remove the native container and promote its child");
                        this.container_removed = true;
                        window.refresh();
                        cx.notify();
                    })),
            );
        }

        if self
            .verify_stage
            .is_some_and(|stage| stage != VerifyStage::Done)
            && !self.verify_callback_scheduled
        {
            self.verify_callback_scheduled = true;
            cx.on_next_frame(window, |this, window, cx| {
                this.verify_after_frame(window, cx)
            });
        }

        div()
            .size_full()
            .bg(rgb(0x151922))
            .text_color(rgb(0xe8edf5))
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .child("GPUI + AppKit composition")
            .child("Type in the native field. The floating GPUI button overlaps it; uncovered clicks stay native.")
            .child(controls)
            .child(format!("The deferred overlay button has received {overlay_clicks} clicks."))
            .child(deferred(
                div()
                    .id("gpui-overlay-button")
                    .absolute()
                    .left(px(66.))
                    .top(px(176.))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(0xb64456))
                    .cursor_pointer()
                    .child("GPUI overlay")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.overlay_clicks += 1;
                        window.refresh();
                        cx.notify();
                    })),
            ))
    }
}

fn native_view(surface: &NativeSurface) -> anyhow::Result<*mut AnyObject> {
    let PlatformSurfaceHandle::Window(handle) = surface.platform_handle();
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        anyhow::bail!("native composition example requires AppKit views");
    };
    Ok(handle.ns_view.as_ptr().cast())
}

unsafe fn superview(view: *mut AnyObject) -> *mut AnyObject {
    unsafe { msg_send![view, superview] }
}

unsafe fn subview_index(root: *mut AnyObject, child: *mut AnyObject) -> Option<usize> {
    let subviews: *mut AnyObject = unsafe { msg_send![root, subviews] };
    let count: usize = unsafe { msg_send![subviews, count] };
    (0..count).find(|index| {
        let view: *mut AnyObject = unsafe { msg_send![subviews, objectAtIndex: *index] };
        view == child
    })
}

unsafe fn first_responder(window: *mut AnyObject) -> *mut AnyObject {
    unsafe { msg_send![window, firstResponder] }
}

unsafe fn native_field_has_focus(window: *mut AnyObject, field: *mut AnyObject) -> bool {
    let first = unsafe { first_responder(window) };
    let editor: *mut AnyObject = unsafe { msg_send![field, currentEditor] };
    first == field || (!editor.is_null() && first == editor)
}

unsafe fn composition_overlay_view(root: *mut AnyObject) -> anyhow::Result<*mut AnyObject> {
    let subviews: *mut AnyObject = unsafe { msg_send![root, subviews] };
    let count: usize = unsafe { msg_send![subviews, count] };
    let root_bounds: NSRect = unsafe { msg_send![root, bounds] };
    for index in 0..count {
        let view: *mut AnyObject = unsafe { msg_send![subviews, objectAtIndex: index] };
        let is_composition_view: Bool =
            unsafe { msg_send![view, isKindOfClass: class!(GPUICompositionView)] };
        if is_composition_view.as_bool() {
            let frame: NSRect = unsafe { msg_send![view, frame] };
            if (frame.size.width - root_bounds.size.width).abs() < 0.01
                && (frame.size.height - root_bounds.size.height).abs() < 0.01
            {
                return Ok(view);
            }
        }
    }
    anyhow::bail!("full-window GPUI overlay NSView was not mounted under the AppKit root")
}

fn assert_view_frame(view: *mut AnyObject, expected: (f64, f64, f64, f64)) -> anyhow::Result<()> {
    let frame: NSRect = unsafe { msg_send![view, frame] };
    let actual = (
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
    );
    ensure!(
        (actual.0 - expected.0).abs() < 0.01
            && (actual.1 - expected.1).abs() < 0.01
            && (actual.2 - expected.2).abs() < 0.01
            && (actual.3 - expected.3).abs() < 0.01,
        "unexpected AppKit frame {actual:?}, expected {expected:?}"
    );
    Ok(())
}

fn main() {
    let verify = std::env::args().any(|argument| argument == "--verify");
    let application = gpui::Application::with_platform(Rc::new(MacPlatform::new(false)));
    application.run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(860.), px(420.)), cx);
        cx.open_window(
            WindowOptions::new().window_bounds(Some(WindowBounds::Windowed(bounds))),
            |window, cx| {
                window
                    .enable_window_composition()
                    .expect("macOS composition must initialize before drawing");
                cx.new(|_| NativeComposition::new(verify))
            },
        )
        .expect("open native composition example window");
        cx.activate(true);
    });
}
