use std::{
    any::Any,
    cell::{Cell, RefCell},
    ffi::c_void,
    num::NonZeroU32,
    ops::{Deref, DerefMut},
    ptr::NonNull,
    rc::{Rc, Weak},
};

use anyhow::Context as _;
use collections::{FxHashMap, FxHashSet};
use gpui::{
    Bounds, CompositionContent, CompositionFrame, CompositionHitRegion, CompositionSurfaceId,
    DevicePixels, GpuiSurfaceId, GpuiSurfaceRole, Pixels, PlatformSurfaceAttachment,
    PlatformSurfaceHandle, Point, Scene, Size, point, px,
};
use gpui_wgpu::{WgpuRenderer, WgpuSurfaceConfig, wgpu};
use raw_window_handle as rwh;
use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::{
        shape,
        shape::ConnectionExt as ShapeConnectionExt,
        xinput::{self, ConnectionExt as XInputConnectionExt},
        xproto::{self, ConnectionExt as _},
    },
    xcb_ffi::XCBConnection,
};

use super::{X11ClientStatePtr, X11WindowStatePtr, XINPUT_ALL_DEVICE_GROUPS, check_reply};

type ShapeRect = (i16, i16, u16, u16);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Placement {
    parent: xproto::Window,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    input: Option<Vec<ShapeRect>>,
    visible: bool,
}

struct ChildWindow {
    xcb: Rc<XCBConnection>,
    window: xproto::Window,
    visual_id: u32,
    screen_id: usize,
    owner: Weak<Cell<bool>>,
    root: xproto::Window,
    applied: Option<Placement>,
}

#[derive(Clone)]
pub(super) struct X11ChildConfig {
    xcb: Rc<XCBConnection>,
    owner_window: xproto::Window,
    root: xproto::Window,
    screen_id: usize,
    visual_id: u32,
    depth: u8,
    transparent_visual: bool,
    colormap: xproto::Colormap,
    border_pixel: u32,
    owner: Rc<Cell<bool>>,
}

impl X11ChildConfig {
    pub(super) fn new(
        xcb: Rc<XCBConnection>,
        owner_window: xproto::Window,
        root: xproto::Window,
        screen_id: usize,
        visual_id: u32,
        depth: u8,
        transparent_visual: bool,
        colormap: xproto::Colormap,
        border_pixel: u32,
        owner: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            xcb,
            owner_window,
            root,
            screen_id,
            visual_id,
            depth,
            transparent_visual,
            colormap,
            border_pixel,
            owner,
        }
    }
}

impl ChildWindow {
    fn create(
        xcb: Rc<XCBConnection>,
        parent: xproto::Window,
        root: xproto::Window,
        screen_id: usize,
        visual_id: u32,
        depth: u8,
        colormap: xproto::Colormap,
        border_pixel: u32,
        owner: &Rc<Cell<bool>>,
    ) -> anyhow::Result<Self> {
        let window = xcb
            .generate_id()
            .context("X11 failed to allocate composition window ID")?;
        check_reply(
            || "X11 failed to create composition child window",
            xcb.create_window(
                depth,
                window,
                parent,
                0,
                0,
                1,
                1,
                0,
                xproto::WindowClass::INPUT_OUTPUT,
                visual_id,
                &xproto::CreateWindowAux::new()
                    .border_pixel(border_pixel)
                    .background_pixel(0)
                    .colormap(colormap),
            ),
        )?;

        Ok(Self {
            xcb,
            window,
            visual_id,
            screen_id,
            owner: Rc::downgrade(owner),
            root,
            applied: None,
        })
    }

    fn apply(&mut self, placement: &Placement) -> anyhow::Result<()> {
        let previous = self.applied.as_ref();
        if previous.is_none_or(|old| {
            old.parent != placement.parent || old.x != placement.x || old.y != placement.y
        }) {
            check_reply(
                || "X11 failed to position composition child",
                self.xcb
                    .reparent_window(self.window, placement.parent, placement.x, placement.y),
            )?;
        }
        if previous.is_none_or(|old| old.width != placement.width || old.height != placement.height)
        {
            check_reply(
                || "X11 failed to resize composition child",
                self.xcb.configure_window(
                    self.window,
                    &xproto::ConfigureWindowAux::new()
                        .width(u32::from(placement.width))
                        .height(u32::from(placement.height)),
                ),
            )?;
        }
        if let Some(input) = &placement.input {
            if previous.is_none_or(|old| old.input.as_ref() != Some(input)) {
                set_input_shape(&self.xcb, self.window, input)?;
            }
        }
        if previous.is_none_or(|old| old.visible != placement.visible) {
            if placement.visible {
                check_reply(
                    || "X11 failed to show composition child",
                    self.xcb.map_window(self.window),
                )?;
            } else {
                check_reply(
                    || "X11 failed to hide composition child",
                    self.xcb.unmap_window(self.window),
                )?;
            }
        }
        self.applied = Some(placement.clone());
        Ok(())
    }

    fn force_apply(&mut self, placement: &Placement) -> anyhow::Result<()> {
        check_reply(
            || "X11 failed to restore composition child parent",
            self.xcb
                .reparent_window(self.window, placement.parent, placement.x, placement.y),
        )?;
        check_reply(
            || "X11 failed to restore composition child size",
            self.xcb.configure_window(
                self.window,
                &xproto::ConfigureWindowAux::new()
                    .width(u32::from(placement.width))
                    .height(u32::from(placement.height)),
            ),
        )?;
        if let Some(input) = &placement.input {
            set_input_shape(&self.xcb, self.window, input)?;
        }
        if placement.visible {
            check_reply(
                || "X11 failed to restore composition child visibility",
                self.xcb.map_window(self.window),
            )?;
        } else {
            check_reply(
                || "X11 failed to restore hidden composition child",
                self.xcb.unmap_window(self.window),
            )?;
        }
        self.applied = Some(placement.clone());
        Ok(())
    }

    fn hide(&mut self) -> anyhow::Result<()> {
        if self
            .applied
            .as_ref()
            .is_some_and(|placement| !placement.visible)
        {
            return Ok(());
        }
        check_reply(
            || "X11 failed to hide unused composition child",
            self.xcb.unmap_window(self.window),
        )?;
        if let Some(placement) = &mut self.applied {
            placement.visible = false;
        }
        Ok(())
    }

    fn raw_window(&self) -> CompositionRawWindow {
        CompositionRawWindow {
            connection: as_raw_xcb_connection::AsRawXcbConnection::as_raw_xcb_connection(&*self.xcb)
                as *mut _,
            screen_id: self.screen_id,
            window_id: self.window,
            visual_id: self.visual_id,
        }
    }
}

impl Drop for ChildWindow {
    fn drop(&mut self) {
        check_reply(
            || "X11 failed to destroy composition child",
            self.xcb.destroy_window(self.window),
        )
        .ok();
        let _ = self.xcb.flush();
    }
}

struct X11NativeSurface {
    child: RefCell<ChildWindow>,
}

impl X11NativeSurface {
    fn detach_from_window(&self) {
        let mut child = self.child.borrow_mut();
        if child.owner.upgrade().is_some_and(|owner| owner.get()) {
            check_reply(
                || "X11 failed to hide detached composition surface",
                child.xcb.unmap_window(child.window),
            )
            .ok();
            check_reply(
                || "X11 failed to detach native composition surface",
                child.xcb.reparent_window(child.window, child.root, 0, 0),
            )
            .ok();
            child.applied = None;
        }
    }
}

unsafe impl PlatformSurfaceAttachment for X11NativeSurface {
    fn platform_handle(&self) -> PlatformSurfaceHandle<'_> {
        let child = self.child.borrow();
        let id = NonZeroU32::new(child.window).expect("X11 composition window is valid");
        let mut handle = rwh::XcbWindowHandle::new(id);
        handle.visual_id = NonZeroU32::new(child.visual_id);
        PlatformSurfaceHandle::Window(unsafe {
            rwh::WindowHandle::borrow_raw(rwh::RawWindowHandle::Xcb(handle))
        })
    }
}

struct X11GpuiSurface {
    child: ChildWindow,
    renderer: WgpuRenderer,
    registered: bool,
}

enum PlannedContent {
    Gpui {
        id: GpuiSurfaceId,
        role: GpuiSurfaceRole,
    },
    Native {
        id: CompositionSurfaceId,
        bounds: Bounds<DevicePixels>,
        surface: Rc<X11NativeSurface>,
    },
}

struct PlannedSurface {
    parent: Option<CompositionSurfaceId>,
    local_origin: Point<i64>,
    content: PlannedContent,
}

#[derive(Clone)]
enum ResolvedContent {
    Gpui(GpuiSurfaceId),
    Native(Rc<X11NativeSurface>),
}

#[derive(Default)]
struct StagedGpuiSurfaces(FxHashMap<GpuiSurfaceId, X11GpuiSurface>);

impl Deref for StagedGpuiSurfaces {
    type Target = FxHashMap<GpuiSurfaceId, X11GpuiSurface>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for StagedGpuiSurfaces {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for StagedGpuiSurfaces {
    fn drop(&mut self) {
        for surface in self.0.values_mut() {
            surface.renderer.destroy();
        }
    }
}

#[derive(Default)]
pub(super) struct X11Composition {
    enabled: bool,
    child_config: Option<X11ChildConfig>,
    pending_gpui: Option<X11GpuiSurface>,
    active_native: FxHashMap<CompositionSurfaceId, Rc<X11NativeSurface>>,
    native_attachments: Vec<Weak<X11NativeSurface>>,
    gpui: FxHashMap<GpuiSurfaceId, X11GpuiSurface>,
    stacking: FxHashMap<xproto::Window, Vec<xproto::Window>>,
    recreate_renderers: bool,
}

struct Resolved {
    content: ResolvedContent,
    window: xproto::Window,
    placement: Placement,
    origin: Point<DevicePixels>,
}

impl X11Composition {
    pub(super) fn enable(
        &mut self,
        config: X11ChildConfig,
        client: &X11ClientStatePtr,
        renderer: &WgpuRenderer,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<()> {
        if self.enabled {
            return Ok(());
        }
        anyhow::ensure!(
            config
                .xcb
                .extension_information(shape::X11_EXTENSION_NAME)?
                .is_some(),
            "X11 composition requires the Shape extension for per-surface input passthrough"
        );
        let shape_version = config
            .xcb
            .shape_query_version()?
            .reply()
            .context("X11 Shape extension query failed")?;
        anyhow::ensure!(
            (shape_version.major_version, shape_version.minor_version) >= (1, 1),
            "X11 composition requires Shape 1.1 input regions"
        );
        anyhow::ensure!(
            config.transparent_visual,
            "X11 composition requires an alpha-capable child-window visual"
        );
        ensure_compositor(&config.xcb, config.screen_id)?;
        // Validate the same ARGB child-window surface and transparent WGPU mode that overlays
        // will use. The root renderer may use a different visual and cannot prove this works.
        let pending_gpui = Self::new_gpui_surface(&config, client, renderer, size)?;
        self.child_config = Some(config);
        self.pending_gpui = Some(pending_gpui);
        self.enabled = true;
        Ok(())
    }

    pub(super) fn create_surface(&mut self) -> anyhow::Result<Rc<dyn PlatformSurfaceAttachment>> {
        anyhow::ensure!(
            self.enabled,
            "enable X11 composition before creating a surface"
        );
        let config = self
            .child_config
            .as_ref()
            .expect("enabled has child config");
        let child = Self::new_child(config, config.owner_window)?;
        let surface = Rc::new(X11NativeSurface {
            child: RefCell::new(child),
        });
        self.native_attachments.push(Rc::downgrade(&surface));
        Ok(surface)
    }

    pub(super) fn detach_native_surfaces(&mut self) {
        self.native_attachments.retain(|surface| {
            if let Some(surface) = surface.upgrade() {
                surface.detach_from_window();
                true
            } else {
                false
            }
        });
    }

    fn new_child(config: &X11ChildConfig, parent: xproto::Window) -> anyhow::Result<ChildWindow> {
        ChildWindow::create(
            config.xcb.clone(),
            parent,
            config.root,
            config.screen_id,
            config.visual_id,
            config.depth,
            config.colormap,
            config.border_pixel,
            &config.owner,
        )
    }

    fn new_gpui_surface(
        config: &X11ChildConfig,
        client: &X11ClientStatePtr,
        renderer: &WgpuRenderer,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<X11GpuiSurface> {
        let child = Self::new_child(config, config.owner_window)?;
        select_composition_events(&child.xcb, child.window, client)?;
        let raw_window = child.raw_window();
        let renderer = match WgpuRenderer::new_composition_surface(
            renderer,
            &raw_window,
            WgpuSurfaceConfig {
                size,
                transparent: true,
                preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
            },
        ) {
            Ok(renderer) => renderer,
            Err(error) => {
                drop(child);
                return Err(error);
            }
        };
        Ok(X11GpuiSurface {
            child,
            renderer,
            registered: false,
        })
    }

    pub(super) fn present(
        &mut self,
        frame: CompositionFrame<'_>,
        client: &X11ClientStatePtr,
        owner_state: &X11WindowStatePtr,
        size: Size<DevicePixels>,
        renderer: &mut WgpuRenderer,
        raw_window: &CompositionRawWindow,
        scale_factor: f32,
        force_render_after_recovery: &mut bool,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.enabled,
            "enable X11 composition before presenting a frame"
        );
        let config = self
            .child_config
            .as_ref()
            .expect("enabled has child config");
        let xcb = &config.xcb;
        let owner_window = config.owner_window;
        let mut desired_gpui = FxHashSet::default();
        let mut desired_native = FxHashMap::default();
        let mut pending_id = None;
        let mut plan = Vec::with_capacity(frame.surfaces.len());
        for surface in frame.surfaces {
            let content = match surface.content {
                CompositionContent::Gpui { id, role } => {
                    if role != GpuiSurfaceRole::Base {
                        desired_gpui.insert(id);
                        if pending_id.is_none()
                            && self.pending_gpui.is_some()
                            && !self.gpui.contains_key(&id)
                        {
                            pending_id = Some(id);
                        }
                    }
                    PlannedContent::Gpui { id, role }
                }
                CompositionContent::Native {
                    id,
                    bounds,
                    attachment,
                } => {
                    let native = downcast_x11_surface(attachment)?;
                    let child = native.child.borrow();
                    anyhow::ensure!(
                        child
                            .owner
                            .upgrade()
                            .is_some_and(|window| Rc::ptr_eq(&window, &config.owner)),
                        "composition surface belongs to another or closed X11 window"
                    );
                    let id = id.into();
                    desired_native.insert(id, native.clone());
                    PlannedContent::Native {
                        id,
                        bounds,
                        surface: native.clone(),
                    }
                }
                CompositionContent::ExternalGpu {
                    id,
                    bounds,
                    attachment,
                } => {
                    let native = downcast_x11_surface(attachment)?;
                    let child = native.child.borrow();
                    anyhow::ensure!(
                        child
                            .owner
                            .upgrade()
                            .is_some_and(|window| Rc::ptr_eq(&window, &config.owner)),
                        "composition surface belongs to another or closed X11 window"
                    );
                    let id = id.into();
                    desired_native.insert(id, native.clone());
                    PlannedContent::Native {
                        id,
                        bounds,
                        surface: native.clone(),
                    }
                }
            };
            plan.push(PlannedSurface {
                parent: surface.parent,
                local_origin: surface.local_origin(),
                content,
            });
        }
        let base = match plan.first().map(|surface| &surface.content) {
            Some(PlannedContent::Gpui {
                id,
                role: GpuiSurfaceRole::Base,
            }) => *id,
            _ => unreachable!("CompositionFrame always begins with a base GPUI surface"),
        };
        if renderer.device_lost() {
            ensure_compositor(xcb, config.screen_id)?;
            match renderer.recover(raw_window) {
                Ok(()) => self.recreate_renderers = true,
                Err(error) => {
                    log::warn!("X11 GPU recovery failed; retrying on the next frame: {error}")
                }
            }
            *force_render_after_recovery = true;
            return Ok(());
        }
        if self.recreate_renderers {
            ensure_compositor(xcb, config.screen_id)?;
            let mut replacements = FxHashMap::default();
            for (id, surface) in &self.gpui {
                let raw_window = surface.child.raw_window();
                match WgpuRenderer::new_composition_surface(
                    renderer,
                    &raw_window,
                    WgpuSurfaceConfig {
                        size,
                        transparent: true,
                        preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
                    },
                ) {
                    Ok(replacement) => {
                        replacements.insert(*id, replacement);
                    }
                    Err(error) => {
                        for (_, mut renderer) in replacements {
                            renderer.destroy();
                        }
                        return Err(error);
                    }
                }
            }
            let mut pending_replacement = if let Some(pending) = &self.pending_gpui {
                let raw_window = pending.child.raw_window();
                match WgpuRenderer::new_composition_surface(
                    renderer,
                    &raw_window,
                    WgpuSurfaceConfig {
                        size,
                        transparent: true,
                        preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
                    },
                ) {
                    Ok(replacement) => Some(replacement),
                    Err(error) => {
                        for (_, mut renderer) in replacements {
                            renderer.destroy();
                        }
                        return Err(error);
                    }
                }
            } else {
                None
            };
            for (id, surface) in &mut self.gpui {
                let replacement = replacements
                    .remove(id)
                    .expect("every live X11 composition renderer was staged");
                surface.renderer.destroy();
                surface.renderer = replacement;
            }
            if let (Some(pending), Some(replacement)) =
                (&mut self.pending_gpui, pending_replacement.take())
            {
                pending.renderer.destroy();
                pending.renderer = replacement;
            }
            self.recreate_renderers = false;
        }

        let mut added = StagedGpuiSurfaces::default();
        for surface in &plan {
            if let PlannedContent::Gpui { id, role } = surface.content
                && role != GpuiSurfaceRole::Base
                && !self.gpui.contains_key(&id)
                && Some(id) != pending_id
            {
                let surface = Self::new_gpui_surface(config, client, renderer, size)?;
                added.insert(id, surface);
            }
        }

        let scene_rect = Bounds::new(Point::default(), size);
        let mut known = FxHashMap::<CompositionSurfaceId, xproto::Window>::default();
        let mut resolved = Vec::with_capacity(frame.surfaces.len());
        let mut hit_regions = FxHashMap::<GpuiSurfaceId, Vec<Bounds<DevicePixels>>>::default();
        for CompositionHitRegion { surface, bounds } in frame.hit_regions {
            hit_regions
                .entry(*surface)
                .or_default()
                .push(bounds.to_device_pixels(scale_factor));
        }
        for surface in plan {
            let parent_window = match surface.parent {
                Some(parent) => known[&parent],
                None => owner_window,
            };
            match surface.content {
                PlannedContent::Gpui { id, role } => {
                    let surface_id = CompositionSurfaceId::Gpui(id);
                    if role == GpuiSurfaceRole::Base {
                        known.insert(surface_id, owner_window);
                        continue;
                    }
                    let child = self
                        .gpui
                        .get(&id)
                        .map(|surface| &surface.child)
                        .or_else(|| added.get(&id).map(|surface| &surface.child))
                        .or_else(|| {
                            (Some(id) == pending_id)
                                .then(|| self.pending_gpui.as_ref().map(|surface| &surface.child))
                                .flatten()
                        })
                        .unwrap();
                    let input = shape_rectangles(
                        hit_regions.get(&id).map(Vec::as_slice).unwrap_or(&[]),
                        scene_rect,
                    )?;
                    let placement =
                        placement(parent_window, surface.local_origin, size, Some(input), true)?;
                    known.insert(surface_id, child.window);
                    resolved.push(Resolved {
                        content: ResolvedContent::Gpui(id),
                        window: child.window,
                        placement,
                        origin: Point::default(),
                    });
                }
                PlannedContent::Native {
                    id,
                    bounds,
                    surface: native,
                } => {
                    let child = native.child.borrow();
                    let placement = native_placement(parent_window, surface.local_origin, bounds)?;
                    let visible = placement.visible;
                    let origin = if visible {
                        bounds.origin
                    } else {
                        Point::default()
                    };
                    let window = child.window;
                    drop(child);
                    known.insert(id, window);
                    resolved.push(Resolved {
                        content: ResolvedContent::Native(native),
                        window,
                        placement,
                        origin,
                    });
                }
            }
        }

        let mut stacking = FxHashMap::<xproto::Window, Vec<xproto::Window>>::default();
        for surface in &resolved {
            if surface.placement.visible {
                stacking
                    .entry(surface.placement.parent)
                    .or_default()
                    .push(surface.window);
            }
        }

        let mut previous_placements = FxHashMap::default();
        for surface in &resolved {
            let applied = match &surface.content {
                ResolvedContent::Gpui(id) => self
                    .gpui
                    .get(id)
                    .and_then(|gpui| gpui.child.applied.clone())
                    .or_else(|| added.get(id).and_then(|gpui| gpui.child.applied.clone()))
                    .or_else(|| {
                        (Some(*id) == pending_id)
                            .then(|| self.pending_gpui.as_ref())
                            .flatten()
                            .and_then(|gpui| gpui.child.applied.clone())
                    }),
                ResolvedContent::Native(native) => native.child.borrow().applied.clone(),
            };
            previous_placements.insert(surface.window, applied);
        }
        let old_native_placements = self
            .active_native
            .iter()
            .map(|(id, native)| (*id, native.child.borrow().applied.clone()))
            .collect::<FxHashMap<_, _>>();
        let apply_result = (|| {
            for surface in &resolved {
                let target = &surface.placement;
                match &surface.content {
                    ResolvedContent::Gpui(id) => {
                        if let Some(gpui) = self.gpui.get_mut(id) {
                            gpui.child.apply(target)?;
                        } else if let Some(gpui) = added.get_mut(id) {
                            gpui.child.apply(target)?;
                        } else if Some(*id) == pending_id {
                            self.pending_gpui
                                .as_mut()
                                .expect("pending surface was selected for this ID")
                                .child
                                .apply(target)?;
                        }
                    }
                    ResolvedContent::Native(native) => {
                        native.child.borrow_mut().apply(target)?;
                    }
                }
            }
            for (id, native) in &self.active_native {
                if !desired_native.contains_key(id) {
                    native.child.borrow_mut().hide()?;
                }
            }
            if stacking != self.stacking {
                set_stacking(xcb, &stacking)?;
            }
            Ok(())
        })();

        if let Err(error) = apply_result {
            for surface in &resolved {
                let previous = previous_placements.get(&surface.window).cloned().flatten();
                let result = match &surface.content {
                    ResolvedContent::Gpui(id) => {
                        if let Some(placement) = previous {
                            if let Some(gpui) = self.gpui.get_mut(id) {
                                gpui.child.force_apply(&placement)
                            } else if let Some(gpui) = added.get_mut(id) {
                                gpui.child.hide()
                            } else if Some(*id) == pending_id {
                                self.pending_gpui
                                    .as_mut()
                                    .expect("pending surface was selected for this ID")
                                    .child
                                    .hide()
                            } else {
                                Ok(())
                            }
                        } else if let Some(gpui) = added.get_mut(id) {
                            gpui.child.hide()
                        } else if Some(*id) == pending_id {
                            self.pending_gpui
                                .as_mut()
                                .expect("pending surface was selected for this ID")
                                .child
                                .hide()
                        } else if let Some(gpui) = self.gpui.get_mut(id) {
                            gpui.child.hide()
                        } else {
                            Ok(())
                        }
                    }
                    ResolvedContent::Native(native) => match previous {
                        Some(placement) => native.child.borrow_mut().force_apply(&placement),
                        None => native.child.borrow_mut().hide(),
                    },
                };
                if let Err(rollback) = result {
                    log::error!("X11 composition rollback failed: {rollback}");
                }
            }
            for (id, native) in &self.active_native {
                if let Some(previous) = old_native_placements.get(id).and_then(Clone::clone) {
                    if let Err(rollback) = native.child.borrow_mut().force_apply(&previous) {
                        log::error!("X11 native composition rollback failed: {rollback}");
                    }
                }
            }
            if let Err(rollback) = set_stacking(xcb, &self.stacking) {
                log::error!("X11 composition stacking rollback failed: {rollback}");
            }
            return Err(error);
        }

        self.active_native = desired_native;
        self.stacking = stacking;
        if let Some(id) = pending_id {
            self.gpui.insert(
                id,
                self.pending_gpui
                    .take()
                    .expect("pending GPUI renderer has a target ID"),
            );
        }
        self.gpui.extend(std::mem::take(&mut added.0));
        let removed = self
            .gpui
            .keys()
            .filter(|id| !desired_gpui.contains(id))
            .copied()
            .collect::<Vec<_>>();
        for id in removed {
            if let Some(mut surface) = self.gpui.remove(&id) {
                client.unregister_composition_window(surface.child.window);
                surface.renderer.destroy();
            }
        }

        for surface in &resolved {
            if let ResolvedContent::Gpui(id) = &surface.content
                && let Some(gpui) = self.gpui.get_mut(id)
                && !gpui.registered
            {
                let origin = gpui_window_origin(surface.origin, scale_factor);
                client.register_composition_window(gpui.child.window, owner_state.clone(), origin);
                gpui.registered = true;
            }
        }

        let mut empty_scene = Scene::default();
        empty_scene.finish();
        for surface in &resolved {
            let ResolvedContent::Gpui(id) = &surface.content else {
                continue;
            };
            let scene = frame.scene.layer(*id).unwrap_or(&empty_scene);
            if let Some(gpui) = self.gpui.get_mut(id) {
                gpui.renderer.draw(scene);
                if gpui.renderer.needs_redraw() {
                    *force_render_after_recovery = true;
                }
            }
        }
        let base_scene = frame.scene.layer(base).unwrap_or(&empty_scene);
        renderer.draw(base_scene);
        if renderer.needs_redraw() {
            *force_render_after_recovery = true;
        }
        xcb.flush().context("X11 composition flush failed")?;
        Ok(())
    }

    pub(super) fn resize(&mut self, size: Size<DevicePixels>) {
        for surface in self.gpui.values_mut() {
            surface.renderer.update_drawable_size(size);
        }
        if let Some(surface) = &mut self.pending_gpui {
            surface.renderer.update_drawable_size(size);
        }
    }

    pub(super) fn mark_device_recovered(&mut self) {
        self.recreate_renderers = true;
    }

    pub(super) fn destroy(&mut self, client: &X11ClientStatePtr) {
        if let Some(mut surface) = self.pending_gpui.take() {
            client.unregister_composition_window(surface.child.window);
            surface.renderer.destroy();
        }
        for (_, mut surface) in self.gpui.drain() {
            client.unregister_composition_window(surface.child.window);
            surface.renderer.destroy();
        }
        for native in self.active_native.values() {
            let _ = native.child.borrow_mut().hide();
        }
        self.active_native.clear();
        self.child_config = None;
        self.enabled = false;
    }
}

fn gpui_window_origin(origin: Point<DevicePixels>, scale_factor: f32) -> Point<gpui::Pixels> {
    origin.map(|pixel| gpui::px(pixel.0 as f32 / scale_factor))
}

pub(super) fn xinput_position(
    origin: Point<Pixels>,
    x: i32,
    y: i32,
    scale_factor: f32,
) -> Point<Pixels> {
    origin
        + point(
            px(x as f32 / 65536.0 / scale_factor),
            px(y as f32 / 65536.0 / scale_factor),
        )
}

fn native_placement(
    parent: xproto::Window,
    local_origin: Point<i64>,
    bounds: Bounds<DevicePixels>,
) -> anyhow::Result<Placement> {
    let visible = bounds.size.width.0 > 0 && bounds.size.height.0 > 0;
    let size = if visible {
        bounds.size
    } else {
        Size::new(DevicePixels(1), DevicePixels(1))
    };
    placement(parent, local_origin, size, None, visible)
}

fn select_composition_events(
    xcb: &XCBConnection,
    window: xproto::Window,
    client: &X11ClientStatePtr,
) -> anyhow::Result<()> {
    let supports_gestures = client
        .get_client()
        .is_some_and(|client| client.0.borrow().supports_xinput_gestures);
    let mut mask = xinput::XIEventMask::MOTION
        | xinput::XIEventMask::BUTTON_PRESS
        | xinput::XIEventMask::BUTTON_RELEASE;
    if supports_gestures {
        mask |= xinput::XIEventMask::from(1u32 << xinput::GESTURE_PINCH_BEGIN_EVENT)
            | xinput::XIEventMask::from(1u32 << xinput::GESTURE_PINCH_UPDATE_EVENT)
            | xinput::XIEventMask::from(1u32 << xinput::GESTURE_PINCH_END_EVENT);
    }
    check_reply(
        || "X11 failed to select input for GPUI composition surface",
        xcb.xinput_xi_select_events(
            window,
            &[xinput::EventMask {
                deviceid: XINPUT_ALL_DEVICE_GROUPS,
                mask: vec![mask],
            }],
        ),
    )
}

fn downcast_x11_surface(
    attachment: &Rc<dyn PlatformSurfaceAttachment>,
) -> anyhow::Result<Rc<X11NativeSurface>> {
    let attachment: Rc<dyn Any> = attachment.clone();
    attachment
        .downcast::<X11NativeSurface>()
        .map_err(|_| anyhow::anyhow!("composition surface is not an X11 native surface"))
}

fn ensure_compositor(xcb: &XCBConnection, screen_id: usize) -> anyhow::Result<()> {
    let selection = format!("_NET_WM_CM_S{screen_id}");
    let selection = xcb
        .intern_atom(false, selection.as_bytes())?
        .reply()
        .context("X11 compositing selection query failed")?
        .atom;
    anyhow::ensure!(
        selection != x11rb::NONE,
        "X11 compositing selection atom is unavailable"
    );
    anyhow::ensure!(
        xcb.get_selection_owner(selection)?
            .reply()
            .context("X11 compositing manager owner query failed")?
            .owner
            != x11rb::NONE,
        "X11 composition requires an active compositing manager"
    );
    Ok(())
}

fn set_stacking(
    xcb: &XCBConnection,
    windows_by_parent: &FxHashMap<xproto::Window, Vec<xproto::Window>>,
) -> anyhow::Result<()> {
    for windows in windows_by_parent.values() {
        let mut previous = None;
        for &window in windows {
            let aux = if let Some(sibling) = previous {
                xproto::ConfigureWindowAux::new()
                    .sibling(sibling)
                    .stack_mode(xproto::StackMode::ABOVE)
            } else {
                xproto::ConfigureWindowAux::new().stack_mode(xproto::StackMode::BELOW)
            };
            check_reply(
                || "X11 failed to order composition child windows",
                xcb.configure_window(window, &aux),
            )?;
            previous = Some(window);
        }
    }
    Ok(())
}

fn placement(
    parent: xproto::Window,
    local_origin: Point<i64>,
    size: Size<DevicePixels>,
    input: Option<Vec<ShapeRect>>,
    visible: bool,
) -> anyhow::Result<Placement> {
    anyhow::ensure!(
        size.width.0 > 0 && size.height.0 > 0,
        "visible X11 composition surfaces need positive bounds"
    );
    Ok(Placement {
        parent,
        x: i16::try_from(local_origin.x)
            .context("composition surface x position exceeds X11 range")?,
        y: i16::try_from(local_origin.y)
            .context("composition surface y position exceeds X11 range")?,
        width: u16::try_from(size.width.0)
            .context("composition surface width exceeds X11 range")?,
        height: u16::try_from(size.height.0)
            .context("composition surface height exceeds X11 range")?,
        input,
        visible,
    })
}

fn shape_rectangles(
    regions: &[Bounds<DevicePixels>],
    clip: Bounds<DevicePixels>,
) -> anyhow::Result<Vec<ShapeRect>> {
    let mut rectangles = Vec::with_capacity(regions.len());
    for region in regions {
        let left = region.origin.x.0.max(clip.origin.x.0);
        let top = region.origin.y.0.max(clip.origin.y.0);
        let right = region
            .origin
            .x
            .0
            .saturating_add(region.size.width.0)
            .min(clip.origin.x.0.saturating_add(clip.size.width.0));
        let bottom = region
            .origin
            .y
            .0
            .saturating_add(region.size.height.0)
            .min(clip.origin.y.0.saturating_add(clip.size.height.0));
        if right <= left || bottom <= top {
            continue;
        }
        rectangles.push((
            i16::try_from(left).context("X11 input region x exceeds protocol range")?,
            i16::try_from(top).context("X11 input region y exceeds protocol range")?,
            u16::try_from(right - left).context("X11 input region width exceeds protocol range")?,
            u16::try_from(bottom - top)
                .context("X11 input region height exceeds protocol range")?,
        ));
    }
    Ok(rectangles)
}

fn set_input_shape(
    xcb: &XCBConnection,
    window: xproto::Window,
    rectangles: &[ShapeRect],
) -> anyhow::Result<()> {
    let rectangles = rectangles
        .iter()
        .map(|&(x, y, width, height)| xproto::Rectangle {
            x,
            y,
            width,
            height,
        })
        .collect::<Vec<_>>();
    check_reply(
        || "X11 failed to update composition input region",
        xcb.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            xproto::ClipOrdering::UNSORTED,
            window,
            0,
            0,
            &rectangles,
        ),
    )
}



#[derive(Clone, Copy, Debug)]
pub(super) struct CompositionRawWindow {
    connection: *mut c_void,
    screen_id: usize,
    window_id: xproto::Window,
    visual_id: u32,
}

impl CompositionRawWindow {
    pub(super) fn new(
        xcb: &XCBConnection,
        screen_id: usize,
        window_id: xproto::Window,
        visual_id: u32,
    ) -> Self {
        Self {
            connection: as_raw_xcb_connection::AsRawXcbConnection::as_raw_xcb_connection(xcb)
                as *mut _,
            screen_id,
            window_id,
            visual_id,
        }
    }
}

unsafe impl Send for CompositionRawWindow {}
unsafe impl Sync for CompositionRawWindow {}

impl rwh::HasWindowHandle for CompositionRawWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let id = NonZeroU32::new(self.window_id).ok_or(rwh::HandleError::Unavailable)?;
        let mut handle = rwh::XcbWindowHandle::new(id);
        handle.visual_id = NonZeroU32::new(self.visual_id);
        Ok(unsafe { rwh::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl rwh::HasDisplayHandle for CompositionRawWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        let connection = NonNull::new(self.connection).ok_or(rwh::HandleError::Unavailable)?;
        let handle = rwh::XcbDisplayHandle::new(Some(connection), self.screen_id as i32);
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(handle.into()) })
    }
}
