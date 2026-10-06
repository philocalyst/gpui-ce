use core::mem;
use std::{
    any::Any,
    cell::{Cell, Ref, RefCell, RefMut},
    collections::BTreeSet,
    ffi::c_void,
    io::Write,
    ops::{Deref, DerefMut},
    os::fd::AsFd,
    ptr::NonNull,
    rc::Rc,
    sync::Arc,
};

use anyhow::Context as _;
use calloop::ping::Ping;
use collections::{FxHashMap, FxHashSet, HashMap};
use filedescriptor::FileDescriptor;
use futures::channel::oneshot::Receiver;

use image::{ImageBuffer, Rgba, imageops::FilterType};
use raw_window_handle as rwh;
use wayland_backend::client::ObjectId;
use wayland_client::WEnum;
use wayland_client::{
    Proxy,
    protocol::{wl_callback, wl_output, wl_seat, wl_shm, wl_subsurface, wl_surface},
};
use wayland_protocols::wp::viewporter::client::wp_viewport;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1;
use wayland_protocols::xdg::shell::client::xdg_popup;
use wayland_protocols::xdg::shell::client::xdg_positioner;
use wayland_protocols::xdg::shell::client::xdg_surface;
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self};
use wayland_protocols::{
    wp::fractional_scale::v1::client::wp_fractional_scale_v1,
    xdg::dialog::v1::client::xdg_dialog_v1::XdgDialogV1,
};
use wayland_protocols_plasma::blur::client::org_kde_kwin_blur;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1;

use crate::linux::wayland::{display::WaylandDisplay, serial::SerialKind};
use crate::linux::{Globals, Output, WaylandClientStatePtr, get_window};
use gpui::{
    AnyWindowHandle, Bounds, Capslock, CompositionContent, CompositionFrame, CompositionHitRegion,
    CompositionSurfaceId, Decorations, DevicePixels, ExternalDragPayload, GpuSpecs, GpuiSurfaceId,
    GpuiSurfaceRole, Modifiers, Pixels, PlatformAtlas, PlatformDisplay, PlatformInput,
    PlatformInputHandler, PlatformSurfaceAttachment, PlatformSurfaceHandle, PlatformWindow, Point,
    PromptButton, PromptLevel, RequestFrameOptions, ResizeEdge, Scene, Size, Tiling,
    WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowControls,
    WindowDecorations, WindowKind, WindowParams,
    layer_shell::{Anchor, LayerShellNotSupportedError},
    popup::PopupOptions,
    px, size,
};
use gpui_wgpu::{CompositorGpuHint, WgpuRenderer, WgpuSurfaceConfig, wgpu};

struct WaylandSubsurface {
    surface: wl_surface::WlSurface,
    viewport: Option<wp_viewport::WpViewport>,
    role: Option<wl_subsurface::WlSubsurface>,
    parent: Option<ObjectId>,
    destination: Option<(i32, i32)>,
    buffer_scale: Option<i32>,
    position: Option<(i32, i32)>,
    synchronized: bool,
}

impl WaylandSubsurface {
    fn new(globals: &Globals, synchronized: bool) -> anyhow::Result<Self> {
        anyhow::ensure!(
            globals.subcompositor.is_some(),
            "wl_subcompositor is unavailable"
        );
        let surface = globals.compositor.create_surface(&globals.qh, ());
        let viewport = globals
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, &globals.qh, ()));
        Ok(Self {
            surface,
            viewport,
            role: None,
            parent: None,
            destination: None,
            buffer_scale: None,
            position: None,
            synchronized,
        })
    }

    fn attach(&mut self, globals: &Globals, parent: &wl_surface::WlSurface) -> bool {
        if self.parent.as_ref() != Some(&parent.id()) {
            self.detach();
        }
        if self.role.is_none() {
            let role = globals
                .subcompositor
                .as_ref()
                .expect("composition was enabled")
                .get_subsurface(&self.surface, parent, &globals.qh, ());
            if self.synchronized {
                role.set_sync();
            } else {
                role.set_desync();
            }
            self.parent = Some(parent.id());
            self.role = Some(role);
            self.position = None;
            return true;
        }
        false
    }

    fn detach(&mut self) -> bool {
        if let Some(role) = self.role.take() {
            role.destroy();
            self.parent = None;
            self.position = None;
            return true;
        }
        self.parent = None;
        false
    }

    fn set_size(&mut self, size: Size<DevicePixels>, scale: f32) {
        if let Some(viewport) = &self.viewport {
            let destination = (
                ((size.width.0 as f32 / scale).round() as i32).max(1),
                ((size.height.0 as f32 / scale).round() as i32).max(1),
            );
            if self.destination != Some(destination) {
                viewport.set_destination(destination.0, destination.1);
                self.destination = Some(destination);
            }
        } else {
            let buffer_scale = scale.ceil().max(1.0) as i32;
            if self.buffer_scale != Some(buffer_scale) {
                self.surface.set_buffer_scale(buffer_scale);
                self.buffer_scale = Some(buffer_scale);
            }
        }
    }

    fn set_position(&mut self, x: i32, y: i32) {
        if self.position != Some((x, y)) {
            self.role
                .as_ref()
                .expect("position requires an attached subsurface")
                .set_position(x, y);
            self.position = Some((x, y));
        }
    }

    fn role(&self) -> Option<&wl_subsurface::WlSubsurface> {
        self.role.as_ref()
    }

    fn input_scale(&self, scale: f32) -> f32 {
        if self.viewport.is_some() {
            1.0
        } else {
            scale / scale.ceil().max(1.0)
        }
    }

    fn destroy(&mut self) {
        self.detach();
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        self.surface.destroy();
    }
}

fn wayland_input_regions(
    hit_regions: &[CompositionHitRegion],
    surface: GpuiSurfaceId,
    size: Size<DevicePixels>,
    scale: f32,
    input_scale: f32,
) -> Vec<(i32, i32, i32, i32)> {
    let logical_width = size.width.0 as f32 / scale * input_scale;
    let logical_height = size.height.0 as f32 / scale * input_scale;
    hit_regions
        .iter()
        .filter(|region| region.surface == surface)
        .filter_map(|region| {
            let left = (f32::from(region.bounds.origin.x) * input_scale)
                .floor()
                .max(0.0);
            let top = (f32::from(region.bounds.origin.y) * input_scale)
                .floor()
                .max(0.0);
            let right = ((f32::from(region.bounds.origin.x) + f32::from(region.bounds.size.width))
                * input_scale)
                .ceil()
                .min(logical_width);
            let bottom = ((f32::from(region.bounds.origin.y)
                + f32::from(region.bounds.size.height))
                * input_scale)
                .ceil()
                .min(logical_height);
            (right > left && bottom > top).then_some((
                left as i32,
                top as i32,
                (right - left) as i32,
                (bottom - top) as i32,
            ))
        })
        .collect()
}

fn wayland_subsurface_position(local_origin: Point<i64>, scale: f32) -> anyhow::Result<(i32, i32)> {
    anyhow::ensure!(
        scale.is_finite() && scale > 0.0,
        "Wayland composition scale must be positive and finite"
    );
    let convert = |coordinate: i64| -> anyhow::Result<i32> {
        let position = (coordinate as f64 / f64::from(scale)).round();
        anyhow::ensure!(
            position.is_finite() && (i32::MIN as f64..=i32::MAX as f64).contains(&position),
            "Wayland composition surface position exceeds protocol range"
        );
        Ok(position as i32)
    };
    Ok((convert(local_origin.x)?, convert(local_origin.y)?))
}

#[cfg(test)]
mod composition_geometry_tests {
    use super::*;

    #[test]
    fn child_positions_translate_absolute_window_bounds_to_parent_coordinates() {
        assert_eq!(
            wayland_subsurface_position(Point::new(-30_i64, -12_i64), 1.5).unwrap(),
            (-20, -8),
        );
        assert_eq!(
            wayland_subsurface_position(Point::new(45_i64, 30_i64), 1.5).unwrap(),
            (30, 20),
        );
        assert!(wayland_subsurface_position(Point::new(i64::from(i32::MAX) + 1, 0), 1.0).is_err());
    }
}

struct WaylandGpuiSurface {
    child: WaylandSubsurface,
    renderer: WgpuRenderer,
    input_regions: Option<Vec<(i32, i32, i32, i32)>>,
    input_registered: bool,
}

#[derive(Default)]
struct StagedWaylandGpuiSurfaces(FxHashMap<GpuiSurfaceId, WaylandGpuiSurface>);

impl Deref for StagedWaylandGpuiSurfaces {
    type Target = FxHashMap<GpuiSurfaceId, WaylandGpuiSurface>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for StagedWaylandGpuiSurfaces {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for StagedWaylandGpuiSurfaces {
    fn drop(&mut self) {
        for surface in self.0.values_mut() {
            surface.renderer.destroy();
            surface.child.destroy();
        }
    }
}

struct WaylandNativeSurfaceState {
    child: WaylandSubsurface,
    bounds: Bounds<DevicePixels>,
}

struct WaylandNativeSurface {
    state: Rc<RefCell<WaylandNativeSurfaceState>>,
}

unsafe impl PlatformSurfaceAttachment for WaylandNativeSurface {
    fn platform_handle(&self) -> PlatformSurfaceHandle<'_> {
        let state = self.state.borrow();
        let pointer = NonNull::new(state.child.surface.id().as_ptr().cast::<c_void>())
            .expect("Wayland surface proxy has a null pointer");
        let handle = rwh::WaylandWindowHandle::new(pointer);
        PlatformSurfaceHandle::Window(unsafe {
            rwh::WindowHandle::borrow_raw(rwh::RawWindowHandle::Wayland(handle))
        })
    }
}

impl Drop for WaylandNativeSurfaceState {
    fn drop(&mut self) {
        self.child.destroy();
    }
}

struct ResolvedWaylandSurface {
    parent_index: Option<usize>,
    origin: Point<DevicePixels>,
    local_origin: Point<i64>,
    size: Size<DevicePixels>,
    surface: wl_surface::WlSurface,
    content: ResolvedWaylandContent,
}

enum ResolvedWaylandContent {
    Gpui {
        id: GpuiSurfaceId,
        role: GpuiSurfaceRole,
    },
    Native {
        id: CompositionSurfaceId,
        state: Rc<RefCell<WaylandNativeSurfaceState>>,
    },
}

impl ResolvedWaylandContent {
    fn id(&self) -> CompositionSurfaceId {
        match self {
            Self::Gpui { id, .. } => (*id).into(),
            Self::Native { id, .. } => *id,
        }
    }
}

struct WaylandComposition {
    globals: Globals,
    client: WaylandClientStatePtr,
    window_surface: wl_surface::WlSurface,
    size: Size<DevicePixels>,
    scale: f32,
    enabled: bool,
    pending_gpui: Option<WaylandGpuiSurface>,
    gpui_surfaces: FxHashMap<GpuiSurfaceId, WaylandGpuiSurface>,
    native_surfaces: FxHashMap<CompositionSurfaceId, Rc<RefCell<WaylandNativeSurfaceState>>>,
    base_surface: Option<GpuiSurfaceId>,
    last_tree: Vec<(CompositionSurfaceId, Option<usize>)>,
    recreate_renderers: bool,
}

impl WaylandComposition {
    fn enable(&mut self, renderer: &WgpuRenderer) -> anyhow::Result<()> {
        if self.enabled {
            return Ok(());
        }
        anyhow::ensure!(
            self.globals.subcompositor.is_some(),
            "the Wayland compositor does not support wl_subcompositor"
        );
        // Probe the real subsurface target here so unsupported transparent alpha modes fail at
        // enable time rather than on the first frame that tries to draw an overlay.
        let pending = self.new_gpui_surface(renderer)?;
        self.pending_gpui = Some(pending);
        self.enabled = true;
        Ok(())
    }

    fn raw_window(&self, surface: &wl_surface::WlSurface) -> anyhow::Result<RawWindow> {
        Ok(RawWindow {
            window: surface.id().as_ptr().cast::<c_void>(),
            display: surface
                .backend()
                .upgrade()
                .context("Wayland connection closed")?
                .display_ptr()
                .cast::<c_void>(),
        })
    }

    fn native(
        attachment: &Rc<dyn PlatformSurfaceAttachment>,
    ) -> anyhow::Result<Rc<RefCell<WaylandNativeSurfaceState>>> {
        let attachment: &dyn Any = attachment.as_ref();
        attachment
            .downcast_ref::<WaylandNativeSurface>()
            .map(|surface| surface.state.clone())
            .context("composition surface is not a Wayland native surface")
    }

    fn resolved_role(
        &self,
        surface: &ResolvedWaylandSurface,
        added: &StagedWaylandGpuiSurfaces,
        pending_id: Option<GpuiSurfaceId>,
    ) -> Option<wl_subsurface::WlSubsurface> {
        match &surface.content {
            ResolvedWaylandContent::Gpui { id, .. } => self
                .gpui_surfaces
                .get(id)
                .or_else(|| added.get(id))
                .or_else(|| {
                    (Some(*id) == pending_id)
                        .then(|| self.pending_gpui.as_ref())
                        .flatten()
                })
                .and_then(|gpui| gpui.child.role().cloned()),
            ResolvedWaylandContent::Native { state, .. } => state.borrow().child.role().cloned(),
        }
    }

    fn new_gpui_surface(&self, base: &WgpuRenderer) -> anyhow::Result<WaylandGpuiSurface> {
        let mut child = WaylandSubsurface::new(&self.globals, true)?;
        child.set_size(self.size, self.scale);
        let raw_window = self.raw_window(&child.surface)?;
        let renderer = match WgpuRenderer::new_composition_surface(
            base,
            &raw_window,
            WgpuSurfaceConfig {
                size: self.size,
                transparent: true,
                preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
            },
        ) {
            Ok(renderer) => renderer,
            Err(error) => {
                child.destroy();
                return Err(error);
            }
        };
        Ok(WaylandGpuiSurface {
            child,
            renderer,
            input_regions: None,
            input_registered: false,
        })
    }

    fn present(
        &mut self,
        frame: CompositionFrame<'_>,
        renderer: &mut WgpuRenderer,
        owner: WaylandWindowStatePtr,
        redraw_requested: &mut bool,
        pending_frame_callback: &mut Option<wl_callback::WlCallback>,
        presentation: &mut PresentationState,
        frame_loop: &Cell<FrameLoop>,
    ) -> anyhow::Result<()> {
        let mut desired_gpui = FxHashSet::default();
        let mut desired_native = FxHashMap::default();
        let mut pending_id = None;
        let base = frame
            .surfaces
            .iter()
            .next()
            .expect("CompositionFrame always begins with a base GPUI surface");
        let base_id = match base.content {
            CompositionContent::Gpui {
                id,
                role: GpuiSurfaceRole::Base,
            } => id,
            _ => unreachable!("CompositionFrame always begins with a base GPUI surface"),
        };
        for surface in frame.surfaces {
            match surface.content {
                CompositionContent::Gpui { id, role } => {
                    if role != GpuiSurfaceRole::Base {
                        desired_gpui.insert(id);
                        if pending_id.is_none()
                            && self.pending_gpui.is_some()
                            && !self.gpui_surfaces.contains_key(&id)
                        {
                            pending_id = Some(id);
                        }
                    }
                }
                CompositionContent::Native {
                    id,
                    bounds: _,
                    attachment,
                } => {
                    let id = id.into();
                    let state = Self::native(attachment)?;
                    desired_native.insert(id, state.clone());
                }
                CompositionContent::ExternalGpu {
                    id,
                    bounds: _,
                    attachment,
                } => {
                    let id = id.into();
                    let state = Self::native(attachment)?;
                    desired_native.insert(id, state.clone());
                }
            }
        }
        if renderer.device_lost() {
            let raw_window = self.raw_window(&self.window_surface)?;
            if let Err(error) = renderer.recover(&raw_window) {
                log::warn!("GPU recovery failed, will retry on next frame: {error}");
            } else {
                self.recreate_renderers = true;
            }
            *redraw_requested = true;
            return Ok(());
        }

        if self.recreate_renderers {
            let mut replacements = FxHashMap::default();
            for (id, surface) in &self.gpui_surfaces {
                let raw_window = self.raw_window(&surface.child.surface)?;
                match WgpuRenderer::new_composition_surface(
                    renderer,
                    &raw_window,
                    WgpuSurfaceConfig {
                        size: self.size,
                        transparent: true,
                        preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
                    },
                ) {
                    Ok(replacement) => {
                        replacements.insert(*id, replacement);
                    }
                    Err(error) => {
                        for (id, mut replacement) in replacements {
                            replacement.destroy();
                            let _ = id;
                        }
                        return Err(error);
                    }
                }
            }
            let mut pending_replacement = if let Some(pending) = &self.pending_gpui {
                let raw_window = self.raw_window(&pending.child.surface)?;
                match WgpuRenderer::new_composition_surface(
                    renderer,
                    &raw_window,
                    WgpuSurfaceConfig {
                        size: self.size,
                        transparent: true,
                        preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
                    },
                ) {
                    Ok(replacement) => Some(replacement),
                    Err(error) => {
                        for mut replacement in replacements.into_values() {
                            replacement.destroy();
                        }
                        return Err(error);
                    }
                }
            } else {
                None
            };
            for (id, surface) in &mut self.gpui_surfaces {
                let replacement = replacements
                    .remove(id)
                    .expect("every live Wayland composition renderer was staged");
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

        anyhow::ensure!(
            self.enabled,
            "enable Wayland composition before presenting a frame"
        );
        let mut added = StagedWaylandGpuiSurfaces::default();
        for surface in frame.surfaces {
            if let CompositionContent::Gpui { id, role } = surface.content
                && role != GpuiSurfaceRole::Base
                && !self.gpui_surfaces.contains_key(&id)
                && Some(id) != pending_id
            {
                added.insert(id, self.new_gpui_surface(renderer)?);
            }
        }

        let mut resolved = Vec::with_capacity(frame.surfaces.len());
        let mut indices = FxHashMap::default();
        for surface in frame.surfaces {
            let id = surface.id();
            let parent_index = surface.parent.map(|parent| indices[&parent]);
            let local_origin = surface.local_origin();
            let (surface_wl, origin, size, content) = match surface.content {
                CompositionContent::Gpui { id, role } => {
                    if role == GpuiSurfaceRole::Base {
                        (
                            self.window_surface.clone(),
                            Point::default(),
                            self.size,
                            ResolvedWaylandContent::Gpui { id, role },
                        )
                    } else {
                        let gpui = self
                            .gpui_surfaces
                            .get(&id)
                            .or_else(|| added.get(&id))
                            .or_else(|| {
                                (Some(id) == pending_id)
                                    .then(|| self.pending_gpui.as_ref())
                                    .flatten()
                            })
                            .unwrap();
                        (
                            gpui.child.surface.clone(),
                            Point::default(),
                            self.size,
                            ResolvedWaylandContent::Gpui { id, role },
                        )
                    }
                }
                CompositionContent::Native { id, bounds, .. } => {
                    let id = id.into();
                    let state = desired_native[&id].clone();
                    let native_surface = state.borrow().child.surface.clone();
                    (
                        native_surface,
                        bounds.origin,
                        bounds.size,
                        ResolvedWaylandContent::Native { id, state },
                    )
                }
                CompositionContent::ExternalGpu { id, bounds, .. } => {
                    let id = id.into();
                    let state = desired_native[&id].clone();
                    let native_surface = state.borrow().child.surface.clone();
                    (
                        native_surface,
                        bounds.origin,
                        bounds.size,
                        ResolvedWaylandContent::Native { id, state },
                    )
                }
            };
            resolved.push(ResolvedWaylandSurface {
                parent_index,
                origin,
                local_origin,
                size,
                surface: surface_wl,
                content,
            });
            indices.insert(id, resolved.len() - 1);
        }
        let topology_changed = self.last_tree.len() != resolved.len()
            || resolved.iter().enumerate().any(|(index, surface)| {
                self.last_tree.get(index) != Some(&(surface.content.id(), surface.parent_index))
            });
        let mut reorder = topology_changed;
        for index in 0..resolved.len() {
            let parent_surface = resolved[index]
                .parent_index
                .map(|parent| resolved[parent].surface.clone())
                .unwrap_or_else(|| self.window_surface.clone());
            let surface = &mut resolved[index];
            match &surface.content {
                ResolvedWaylandContent::Gpui {
                    role: GpuiSurfaceRole::Base,
                    ..
                } => continue,
                ResolvedWaylandContent::Native { state: native, .. } => {
                    let mut native = native.borrow_mut();
                    native.bounds = Bounds::new(surface.origin, surface.size);
                    if surface.size.width.0 <= 0 || surface.size.height.0 <= 0 {
                        reorder |= native.child.detach();
                        continue;
                    }
                    let position = wayland_subsurface_position(surface.local_origin, self.scale)?;
                    native.child.set_size(surface.size, self.scale);
                    reorder |= native.child.attach(&self.globals, &parent_surface);
                    native.child.set_position(position.0, position.1);
                }
                ResolvedWaylandContent::Gpui { id, .. } => {
                    let position = wayland_subsurface_position(surface.local_origin, self.scale)?;
                    let gpui = self
                        .gpui_surfaces
                        .get_mut(id)
                        .or_else(|| added.get_mut(id))
                        .or_else(|| {
                            (Some(*id) == pending_id)
                                .then(|| self.pending_gpui.as_mut())
                                .flatten()
                        })
                        .unwrap();
                    gpui.child.set_size(surface.size, self.scale);
                    reorder |= gpui.child.attach(&self.globals, &parent_surface);
                    gpui.child.set_position(position.0, position.1);
                }
            }
        }

        if reorder {
            let mut sibling = self.window_surface.clone();
            for surface in resolved
                .iter()
                .skip(1)
                .filter(|surface| surface.parent_index.is_none())
            {
                if let Some(role) = self.resolved_role(surface, &added, pending_id) {
                    role.place_above(&sibling);
                    sibling = surface.surface.clone();
                }
            }
            let mut last_child = FxHashMap::default();
            for surface in &resolved {
                let Some(parent) = surface.parent_index else {
                    continue;
                };
                let previous = last_child
                    .get(&parent)
                    .cloned()
                    .unwrap_or_else(|| resolved[parent].surface.clone());
                if let Some(role) = self.resolved_role(surface, &added, pending_id) {
                    role.place_above(&previous);
                    last_child.insert(parent, surface.surface.clone());
                }
            }
            self.last_tree = resolved
                .iter()
                .map(|surface| (surface.content.id(), surface.parent_index))
                .collect();
        }

        for (id, native) in std::mem::take(&mut self.native_surfaces) {
            if !desired_native.contains_key(&id) {
                native.borrow_mut().child.detach();
            }
        }
        self.native_surfaces = desired_native;
        if let Some(id) = pending_id {
            self.gpui_surfaces.insert(
                id,
                self.pending_gpui
                    .take()
                    .expect("pending GPUI renderer has a target ID"),
            );
        }
        for (id, surface) in mem::take(&mut added.0) {
            self.gpui_surfaces.insert(id, surface);
        }
        let removed = self
            .gpui_surfaces
            .keys()
            .filter(|id| !desired_gpui.contains(id))
            .copied()
            .collect::<Vec<_>>();
        for id in removed {
            if let Some(mut surface) = self.gpui_surfaces.remove(&id) {
                self.client
                    .unregister_composition_input_surface(&surface.child.surface.id());
                surface.renderer.destroy();
                surface.child.destroy();
            }
        }
        self.base_surface = Some(base_id);

        let mut empty_scene = Scene::default();
        empty_scene.finish();
        for surface in &resolved {
            let (id, role) = match &surface.content {
                ResolvedWaylandContent::Gpui { id, role } => (*id, *role),
                ResolvedWaylandContent::Native { .. } => continue,
            };
            let scene = frame.scene.layer(id).unwrap_or(&empty_scene);
            if role != GpuiSurfaceRole::Base {
                let gpui = self.gpui_surfaces.get_mut(&id).unwrap();
                let input_regions = wayland_input_regions(
                    frame.hit_regions,
                    id,
                    self.size,
                    self.scale,
                    gpui.child.input_scale(self.scale),
                );
                if gpui.input_regions.as_ref() != Some(&input_regions) {
                    let region = self.globals.compositor.create_region(&self.globals.qh, ());
                    for &(x, y, width, height) in &input_regions {
                        region.add(x, y, width, height);
                    }
                    gpui.child.surface.set_input_region(Some(&region));
                    region.destroy();
                    gpui.input_regions = Some(input_regions);
                }
                if !gpui.input_registered {
                    let offset = surface.origin.map(|value| px(value.0 as f32 / self.scale));
                    self.client.register_composition_input_surface(
                        gpui.child.surface.id(),
                        owner.clone(),
                        offset,
                    );
                    gpui.input_registered = true;
                }
                gpui.renderer.draw(scene);
                if gpui.renderer.needs_redraw() {
                    *redraw_requested = true;
                }
            }
        }
        if pending_frame_callback.is_none() {
            *pending_frame_callback = Some(
                self.window_surface
                    .frame(&self.globals.qh, self.window_surface.id()),
            );
        }
        let base_scene = frame.scene.layer(base_id).unwrap_or(&empty_scene);
        if renderer.draw(base_scene) {
            *presentation = PresentationState::Presented;
            frame_loop.set(FrameLoop::AwaitingCallback);
        } else {
            *presentation = presentation.failed();
            frame_loop.set(FrameLoop::PresentationFailed);
            *redraw_requested = true;
        }
        if renderer.needs_redraw() {
            *redraw_requested = true;
        }
        Ok(())
    }

    fn resize(&mut self, size: Size<DevicePixels>, scale: f32) {
        self.size = size;
        self.scale = scale;
        for surface in self.gpui_surfaces.values_mut() {
            surface.renderer.update_drawable_size(size);
            surface.child.set_size(size, scale);
        }
        if let Some(surface) = &mut self.pending_gpui {
            surface.renderer.update_drawable_size(size);
            surface.child.set_size(size, scale);
        }
        for surface in self.native_surfaces.values() {
            let mut state = surface.borrow_mut();
            let size = state.bounds.size;
            state.child.set_size(size, scale);
        }
    }

    fn destroy(&mut self) {
        if let Some(mut surface) = self.pending_gpui.take() {
            surface.renderer.destroy();
            surface.child.destroy();
        }
        for (_, mut surface) in self.gpui_surfaces.drain() {
            self.client
                .unregister_composition_input_surface(&surface.child.surface.id());
            surface.renderer.destroy();
            surface.child.destroy();
        }
        for surface in self.native_surfaces.values() {
            surface.borrow_mut().child.detach();
        }
        self.native_surfaces.clear();
        self.base_surface = None;
        self.last_tree.clear();
        self.enabled = false;
    }
}

#[derive(Default)]
pub(crate) struct Callbacks {
    request_frame: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    input: Option<Box<dyn FnMut(gpui::PlatformInput) -> gpui::DispatchEventResult>>,
    active_status_change: Option<Box<dyn FnMut(bool)>>,
    hover_status_change: Option<Box<dyn FnMut(bool)>>,
    resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    moved: Option<Box<dyn FnMut()>>,
    should_close: Option<Box<dyn FnMut() -> bool>>,
    close: Option<Box<dyn FnOnce()>>,
    appearance_changed: Option<Box<dyn FnMut()>>,
    button_layout_changed: Option<Box<dyn FnMut()>>,
}

#[derive(Debug, Clone, Copy)]
struct RawWindow {
    window: *mut c_void,
    display: *mut c_void,
}

// Safety: The raw pointers in RawWindow point to Wayland surface/display
// which are valid for the window's lifetime. These are used only for
// passing to wgpu which needs Send+Sync for surface creation.
unsafe impl Send for RawWindow {}
unsafe impl Sync for RawWindow {}

impl rwh::HasWindowHandle for RawWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let window = NonNull::new(self.window).unwrap();
        let handle = rwh::WaylandWindowHandle::new(window);
        Ok(unsafe { rwh::WindowHandle::borrow_raw(handle.into()) })
    }
}
impl rwh::HasDisplayHandle for RawWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        let display = NonNull::new(self.display).unwrap();
        let handle = rwh::WaylandDisplayHandle::new(display);
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(handle.into()) })
    }
}

#[derive(Debug)]
struct InProgressConfigure {
    size: Option<Size<Pixels>>,
    fullscreen: bool,
    maximized: bool,
    resizing: bool,
    tiling: Tiling,
}

pub struct WaylandWindowState {
    surface_state: WaylandSurfaceState,
    parent: Option<WaylandWindowStatePtr>,
    /// Child surfaces mapped to whether they block this window's input (dialogs
    /// block, popups don't). Children are closed before this window closes.
    children: FxHashMap<ObjectId, bool>,
    pub surface: wl_surface::WlSurface,
    app_id: Option<String>,
    title: Option<String>,
    window_min_size: Option<Size<Pixels>>,
    appearance: WindowAppearance,
    blur: Option<org_kde_kwin_blur::OrgKdeKwinBlur>,
    viewport: Option<wp_viewport::WpViewport>,
    outputs: HashMap<ObjectId, Output>,
    display: Option<(ObjectId, Output)>,
    globals: Globals,
    renderer: WgpuRenderer,
    composition: WaylandComposition,
    bounds: Bounds<Pixels>,
    scale: f32,
    input_handler: Option<PlatformInputHandler>,
    decorations: WindowDecorations,
    background_appearance: WindowBackgroundAppearance,
    fullscreen: bool,
    maximized: bool,
    tiling: Tiling,
    window_bounds: Bounds<Pixels>,
    client: WaylandClientStatePtr,
    handle: AnyWindowHandle,
    active: bool,
    hovered: bool,
    visible: bool,
    redraw_requested: bool,
    presentation: PresentationState,
    pending_frame_callback: Option<wl_callback::WlCallback>,
    in_progress_configure: Option<InProgressConfigure>,
    resize_throttle: bool,
    in_progress_window_controls: Option<WindowControls>,
    window_controls: WindowControls,
    client_inset: Option<Pixels>,
    accesskit_adapter: Option<accesskit_unix::Adapter>,
    icon: Option<Arc<ImageBuffer<Rgba<u8>, Vec<u8>>>>,
    generated_icons: Vec<FileDescriptor>,
}

pub enum WaylandSurfaceState {
    Xdg(WaylandXdgSurfaceState),
    LayerShell(WaylandLayerSurfaceState),
    Popup(WaylandPopupSurfaceState),
}

impl WaylandSurfaceState {
    fn new(
        surface: &wl_surface::WlSurface,
        globals: &Globals,
        params: &WindowParams,
        parent: Option<WaylandWindowStatePtr>,
        popup_grab: Option<(u32, wl_seat::WlSeat)>,
        target_output: Option<wl_output::WlOutput>,
    ) -> anyhow::Result<Self> {
        // For layer_shell windows, create a layer surface instead of an xdg surface
        if let WindowKind::LayerShell(options) = &params.kind {
            let Some(layer_shell) = globals.layer_shell.as_ref() else {
                return Err(LayerShellNotSupportedError.into());
            };

            let layer_surface = layer_shell.get_layer_surface(
                &surface,
                target_output.as_ref(),
                super::layer_shell::wayland_layer(options.layer),
                options.namespace.clone(),
                &globals.qh,
                surface.id(),
            );

            let width = f32::from(params.bounds.size.width);
            let height = f32::from(params.bounds.size.height);
            layer_surface.set_size(width as u32, height as u32);

            layer_surface.set_anchor(super::layer_shell::wayland_anchor(options.anchor));
            layer_surface.set_keyboard_interactivity(
                super::layer_shell::wayland_keyboard_interactivity(options.keyboard_interactivity),
            );

            if let Some(margin) = options.margin {
                layer_surface.set_margin(
                    f32::from(margin.0) as i32,
                    f32::from(margin.1) as i32,
                    f32::from(margin.2) as i32,
                    f32::from(margin.3) as i32,
                )
            }

            if let Some(exclusive_zone) = options.exclusive_zone {
                layer_surface.set_exclusive_zone(f32::from(exclusive_zone) as i32);
            }

            if let Some(exclusive_edge) = options.exclusive_edge {
                Self::apply_exclusive_edge(&layer_surface, options.anchor, exclusive_edge);
            }

            return Ok(WaylandSurfaceState::LayerShell(WaylandLayerSurfaceState {
                layer_surface,
                anchor: options.anchor,
            }));
        }

        if let WindowKind::AnchoredPopup(options) = &params.kind {
            let Some(parent) = parent.as_ref() else {
                return Err(anyhow::anyhow!("popup parent window not found"));
            };

            let positioner = build_popup_positioner(
                globals,
                options,
                params.bounds.size,
                parent.window_geometry(),
            );

            let xdg_surface = globals
                .wm_base
                .get_xdg_surface(&surface, &globals.qh, surface.id());

            // A layer-shell parent takes a null xdg parent and is attached via the layer
            // surface. Every other surface kind has an xdg_surface to parent to directly.
            let xdg_popup = if let Some(parent_layer_surface) = parent.layer_surface() {
                let xdg_popup = xdg_surface.get_popup(None, &positioner, &globals.qh, surface.id());
                parent_layer_surface.get_popup(&xdg_popup);
                xdg_popup
            } else {
                xdg_surface.get_popup(
                    parent.xdg_surface().as_ref(),
                    &positioner,
                    &globals.qh,
                    surface.id(),
                )
            };
            positioner.destroy();

            if let Some((serial, seat)) = popup_grab {
                xdg_popup.grab(&seat, serial);
            }

            // Non-blocking: the parent keeps its input so it can dismiss the popup on
            // clicks in its own window.
            parent.add_child(surface.id(), false);

            return Ok(WaylandSurfaceState::Popup(WaylandPopupSurfaceState {
                xdg_surface,
                xdg_popup,
                options: options.clone(),
                next_reposition_token: Cell::new(0),
            }));
        }

        // All other WindowKinds result in a regular xdg surface
        let xdg_surface = globals
            .wm_base
            .get_xdg_surface(&surface, &globals.qh, surface.id());

        let toplevel = xdg_surface.get_toplevel(&globals.qh, surface.id());
        let xdg_parent = parent.as_ref().and_then(|w| w.toplevel());

        if params.kind == WindowKind::Floating || params.kind == WindowKind::Dialog {
            toplevel.set_parent(xdg_parent.as_ref());
        }

        let dialog = if params.kind == WindowKind::Dialog {
            let dialog = globals.dialog.as_ref().map(|dialog| {
                let xdg_dialog = dialog.get_xdg_dialog(&toplevel, &globals.qh, ());
                xdg_dialog.set_modal();
                xdg_dialog
            });

            if let Some(parent) = parent.as_ref() {
                parent.add_child(surface.id(), true);
            }

            dialog
        } else {
            None
        };

        if let Some(size) = params.window_min_size {
            toplevel.set_min_size(f32::from(size.width) as i32, f32::from(size.height) as i32);
        }

        // Attempt to set up window decorations based on the requested configuration
        let decoration = globals
            .decoration_manager
            .as_ref()
            .map(|decoration_manager| {
                decoration_manager.get_toplevel_decoration(&toplevel, &globals.qh, surface.id())
            });

        Ok(WaylandSurfaceState::Xdg(WaylandXdgSurfaceState {
            xdg_surface,
            toplevel,
            decoration,
            dialog,
        }))
    }
}

pub struct WaylandXdgSurfaceState {
    xdg_surface: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
    decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    dialog: Option<XdgDialogV1>,
}

pub struct WaylandLayerSurfaceState {
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    anchor: Anchor,
}

pub struct WaylandPopupSurfaceState {
    xdg_surface: xdg_surface::XdgSurface,
    xdg_popup: xdg_popup::XdgPopup,
    // Kept so the popup can be re-anchored via `xdg_popup.reposition` when resized.
    options: PopupOptions,
    next_reposition_token: Cell<u32>,
}

fn build_popup_positioner(
    globals: &Globals,
    options: &PopupOptions,
    size: Size<Pixels>,
    parent_geometry: Bounds<Pixels>,
) -> xdg_positioner::XdgPositioner {
    let positioner = globals.wm_base.create_positioner(&globals.qh, ());
    // A zero or negative size is a protocol error.
    positioner.set_size(
        f32::from(size.width).max(1.0) as i32,
        f32::from(size.height).max(1.0) as i32,
    );

    // The protocol wants the anchor rect relative to the parent's window geometry, while
    // `options.anchor_rect` is in gpui window coordinates (surface-local). A rect extending
    // outside the geometry or with a zero size is a protocol error, so translate, then clamp
    // to at least one pixel inside the geometry, pulling the origin inward at the edges.
    let anchor_rect = Bounds {
        origin: options.anchor_rect.origin - parent_geometry.origin,
        size: options.anchor_rect.size,
    };
    let one = Point::new(px(1.0), px(1.0));
    let geometry_bottom_right: Point<Pixels> = parent_geometry.size.into();
    let top_left = anchor_rect
        .origin
        .min(&(geometry_bottom_right - one))
        .max(&Point::default());
    let bottom_right = anchor_rect
        .bottom_right()
        .min(&geometry_bottom_right)
        .max(&(top_left + one));
    let anchor_rect = Bounds::from_corners(top_left, bottom_right);
    positioner.set_anchor_rect(
        f32::from(anchor_rect.origin.x) as i32,
        f32::from(anchor_rect.origin.y) as i32,
        f32::from(anchor_rect.size.width) as i32,
        f32::from(anchor_rect.size.height) as i32,
    );

    positioner.set_anchor(super::popup::wayland_anchor(options.anchor));
    positioner.set_gravity(super::popup::wayland_gravity(options.gravity));
    positioner.set_constraint_adjustment(super::popup::wayland_constraint_adjustment(
        options.constraint_adjustment,
    ));
    positioner.set_offset(
        f32::from(options.offset.x) as i32,
        f32::from(options.offset.y) as i32,
    );
    positioner
}

impl WaylandSurfaceState {
    fn ack_configure(&self, serial: u32) {
        match self {
            WaylandSurfaceState::Xdg(WaylandXdgSurfaceState { xdg_surface, .. }) => {
                xdg_surface.ack_configure(serial);
            }
            WaylandSurfaceState::LayerShell(WaylandLayerSurfaceState { layer_surface, .. }) => {
                layer_surface.ack_configure(serial);
            }
            WaylandSurfaceState::Popup(WaylandPopupSurfaceState { xdg_surface, .. }) => {
                xdg_surface.ack_configure(serial);
            }
        }
    }

    fn decoration(&self) -> Option<&zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1> {
        if let WaylandSurfaceState::Xdg(WaylandXdgSurfaceState { decoration, .. }) = self {
            decoration.as_ref()
        } else {
            None
        }
    }

    fn toplevel(&self) -> Option<&xdg_toplevel::XdgToplevel> {
        if let WaylandSurfaceState::Xdg(WaylandXdgSurfaceState { toplevel, .. }) = self {
            Some(toplevel)
        } else {
            None
        }
    }

    fn xdg_surface(&self) -> Option<&xdg_surface::XdgSurface> {
        match self {
            WaylandSurfaceState::Xdg(WaylandXdgSurfaceState { xdg_surface, .. }) => {
                Some(xdg_surface)
            }
            WaylandSurfaceState::Popup(WaylandPopupSurfaceState { xdg_surface, .. }) => {
                Some(xdg_surface)
            }
            WaylandSurfaceState::LayerShell(_) => None,
        }
    }

    fn layer_surface(&self) -> Option<&zwlr_layer_surface_v1::ZwlrLayerSurfaceV1> {
        if let WaylandSurfaceState::LayerShell(WaylandLayerSurfaceState { layer_surface, .. }) =
            self
        {
            Some(layer_surface)
        } else {
            None
        }
    }

    fn set_geometry(&self, x: i32, y: i32, width: i32, height: i32) {
        match self {
            WaylandSurfaceState::Xdg(WaylandXdgSurfaceState { xdg_surface, .. }) => {
                xdg_surface.set_window_geometry(x, y, width, height);
            }
            WaylandSurfaceState::LayerShell(WaylandLayerSurfaceState { layer_surface, .. }) => {
                // cannot set window position of a layer surface
                layer_surface.set_size(width as u32, height as u32);
            }
            WaylandSurfaceState::Popup(WaylandPopupSurfaceState { xdg_surface, .. }) => {
                xdg_surface.set_window_geometry(x, y, width, height);
            }
        }
    }

    // Re-anchors a mapped popup at a new size via `xdg_popup.reposition`. Repositioning an
    // unmapped popup (before the first configure) is a protocol error.
    fn reposition_popup(
        &self,
        globals: &Globals,
        size: Size<Pixels>,
        parent_geometry: Bounds<Pixels>,
    ) {
        if let WaylandSurfaceState::Popup(WaylandPopupSurfaceState {
            xdg_popup,
            options,
            next_reposition_token,
            ..
        }) = self
            && xdg_popup.version() >= xdg_popup::REQ_REPOSITION_SINCE
        {
            let token = next_reposition_token.get();
            next_reposition_token.set(token.wrapping_add(1));

            let positioner = build_popup_positioner(globals, options, size, parent_geometry);
            xdg_popup.reposition(&positioner, token);
            positioner.destroy();
        }
    }

    fn set_exclusive_zone(&self, zone: i32) -> bool {
        if let WaylandSurfaceState::LayerShell(WaylandLayerSurfaceState { layer_surface, .. }) =
            self
        {
            layer_surface.set_exclusive_zone(zone);
            true
        } else {
            false
        }
    }

    /// An exclusive edge must be a single edge that the surface is anchored to,
    /// otherwise the compositor raises a fatal `invalid_exclusive_edge` protocol
    /// error. An invalid edge is logged and ignored. Returns whether it applied.
    fn apply_exclusive_edge(
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        anchor: Anchor,
        edge: Anchor,
    ) -> bool {
        if edge.bits().count_ones() == 1 && anchor.contains(edge) {
            layer_surface.set_exclusive_edge(super::layer_shell::wayland_anchor(edge));
            true
        } else {
            log::warn!(
                "ignoring exclusive edge {edge:?}: must be a single edge of the surface anchor {anchor:?}"
            );
            false
        }
    }

    fn set_exclusive_edge(&self, edge: Anchor) -> bool {
        if let WaylandSurfaceState::LayerShell(WaylandLayerSurfaceState {
            layer_surface,
            anchor,
            ..
        }) = self
        {
            Self::apply_exclusive_edge(layer_surface, *anchor, edge)
        } else {
            false
        }
    }

    fn destroy(&mut self) {
        match self {
            WaylandSurfaceState::Xdg(WaylandXdgSurfaceState {
                xdg_surface,
                toplevel,
                decoration: _decoration,
                dialog,
            }) => {
                // drop the dialog before toplevel so compositor can explicitly unapply it's effects
                if let Some(dialog) = dialog {
                    dialog.destroy();
                }

                // The role object (toplevel) must always be destroyed before the xdg_surface.
                // See https://wayland.app/protocols/xdg-shell#xdg_surface:request:destroy
                toplevel.destroy();
                xdg_surface.destroy();
            }
            WaylandSurfaceState::LayerShell(WaylandLayerSurfaceState { layer_surface, .. }) => {
                layer_surface.destroy();
            }
            WaylandSurfaceState::Popup(WaylandPopupSurfaceState {
                xdg_surface,
                xdg_popup,
                ..
            }) => {
                // Role object before its xdg_surface, as with the toplevel above.
                xdg_popup.destroy();
                xdg_surface.destroy();
            }
        }
    }
}

#[derive(Clone)]
pub struct WaylandWindowStatePtr {
    state: Rc<RefCell<WaylandWindowState>>,
    callbacks: Rc<RefCell<Callbacks>>,
    frame_loop: Rc<Cell<FrameLoop>>,
    frame_ping: Ping,
}

impl WaylandWindowState {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        surface: wl_surface::WlSurface,
        surface_state: WaylandSurfaceState,
        appearance: WindowAppearance,
        viewport: Option<wp_viewport::WpViewport>,
        client: WaylandClientStatePtr,
        globals: Globals,
        gpu_context: gpui_wgpu::GpuContext,
        compositor_gpu: Option<CompositorGpuHint>,
        gpu_requirements: Option<gpui_wgpu::WgpuDeviceRequirements>,
        options: WindowParams,
        parent: Option<WaylandWindowStatePtr>,
    ) -> anyhow::Result<Self> {
        let renderer = {
            let raw_window = RawWindow {
                window: surface.id().as_ptr().cast::<c_void>(),
                display: surface
                    .backend()
                    .upgrade()
                    .unwrap()
                    .display_ptr()
                    .cast::<c_void>(),
            };
            let config = WgpuSurfaceConfig {
                size: Size {
                    width: DevicePixels(f32::from(options.bounds.size.width) as i32),
                    height: DevicePixels(f32::from(options.bounds.size.height) as i32),
                },
                transparent: true,
                // Prefer Mailbox to avoid blocking. Falls back to FIFO if Mailbox is unsupported.
                preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
            };
            WgpuRenderer::new(
                gpu_context,
                &raw_window,
                config,
                compositor_gpu,
                gpu_requirements,
            )?
        };

        let title = options
            .titlebar
            .as_ref()
            .and_then(|titlebar| titlebar.title.as_ref())
            .map(ToString::to_string);

        if let WaylandSurfaceState::Xdg(ref xdg_state) = surface_state {
            if let Some(title) = title.as_ref() {
                xdg_state.toplevel.set_title(title.clone());
            }

            if let Some(app_id) = options.app_id.as_ref() {
                xdg_state.toplevel.set_app_id(app_id.clone());
            }

            // Set max window size based on the GPU's maximum texture dimension.
            // This prevents the window from being resized larger than what the GPU can render.
            let max_texture_size = renderer.max_texture_size() as i32;
            xdg_state
                .toplevel
                .set_max_size(max_texture_size, max_texture_size);
        }

        let composition = WaylandComposition {
            globals: globals.clone(),
            client: client.clone(),
            window_surface: surface.clone(),
            size: Size {
                width: DevicePixels(f32::from(options.bounds.size.width) as i32),
                height: DevicePixels(f32::from(options.bounds.size.height) as i32),
            },
            scale: 1.0,
            enabled: false,
            pending_gpui: None,
            gpui_surfaces: FxHashMap::default(),
            native_surfaces: FxHashMap::default(),
            base_surface: None,
            last_tree: Vec::new(),
            recreate_renderers: false,
        };

        Ok(Self {
            surface_state,
            parent,
            children: FxHashMap::default(),
            surface,
            app_id: options.app_id,
            title,
            window_min_size: options.window_min_size,
            icon: options.icon,
            generated_icons: Vec::default(),
            blur: None,
            viewport,
            globals,
            outputs: HashMap::default(),
            display: None,
            renderer,
            composition,
            bounds: options.bounds,
            scale: 1.0,
            input_handler: None,
            decorations: WindowDecorations::Client,
            background_appearance: WindowBackgroundAppearance::Opaque,
            fullscreen: false,
            maximized: false,
            tiling: Tiling::default(),
            window_bounds: options.bounds,
            in_progress_configure: None,
            resize_throttle: false,
            client,
            appearance,
            handle,
            active: false,
            hovered: false,
            visible: options.show,
            redraw_requested: false,
            presentation: PresentationState::Unpresented,
            pending_frame_callback: None,
            in_progress_window_controls: None,
            window_controls: WindowControls::default(),
            client_inset: None,
            accesskit_adapter: None,
        })
    }

    pub fn is_transparent(&self) -> bool {
        self.decorations == WindowDecorations::Client || self.background_appearance.is_transparent()
    }

    fn prepare_to_map(&self) {
        let WaylandSurfaceState::Xdg(xdg_state) = &self.surface_state else {
            return;
        };

        if let Some(title) = self.title.as_ref() {
            xdg_state.toplevel.set_title(title.clone());
        }
        if let Some(app_id) = self.app_id.as_ref() {
            xdg_state.toplevel.set_app_id(app_id.clone());
        }
        if let Some(size) = self.window_min_size {
            xdg_state
                .toplevel
                .set_min_size(f32::from(size.width) as i32, f32::from(size.height) as i32);
        }

        let max_texture_size = self.renderer.max_texture_size() as i32;
        xdg_state
            .toplevel
            .set_max_size(max_texture_size, max_texture_size);

        if let Some(parent) = self.parent.as_ref() {
            let parent_toplevel = parent.toplevel();
            xdg_state.toplevel.set_parent(parent_toplevel.as_ref());
        }

        if self.fullscreen {
            xdg_state.toplevel.set_fullscreen(None);
        } else if self.maximized {
            xdg_state.toplevel.set_maximized();
        }
    }

    fn update_subpixel_layout(&mut self) {
        use wayland_client::protocol::wl_output::Subpixel;
        let is_bgr = self
            .display
            .as_ref()
            .and_then(|(_, output)| output.subpixel)
            .is_some_and(|s| s == Subpixel::HorizontalBgr);
        self.renderer.set_subpixel_layout(is_bgr);
    }

    pub fn primary_output_scale(&mut self) -> i32 {
        let mut scale = 1;
        let mut current_output = self.display.take();
        for (id, output) in self.outputs.iter() {
            if let Some((_, output_data)) = &current_output {
                if output.scale > output_data.scale {
                    current_output = Some((id.clone(), output.clone()));
                }
            } else {
                current_output = Some((id.clone(), output.clone()));
            }
            scale = scale.max(output.scale);
        }
        self.display = current_output;
        scale
    }

    pub fn inset(&self) -> Pixels {
        match self.decorations {
            WindowDecorations::Server => px(0.0),
            WindowDecorations::Client => self.client_inset.unwrap_or(px(0.0)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresentationState {
    Unpresented,
    Presented,
    RetryBeforeFirstPresent,
    RetryAfterPresent,
}

impl PresentationState {
    fn requires_presentation(self) -> bool {
        matches!(
            self,
            Self::RetryBeforeFirstPresent | Self::RetryAfterPresent
        )
    }

    fn failed(self) -> Self {
        match self {
            Self::Unpresented | Self::RetryBeforeFirstPresent => Self::RetryBeforeFirstPresent,
            Self::Presented | Self::RetryAfterPresent => Self::RetryAfterPresent,
        }
    }
}

#[cfg(test)]
mod presentation_state_tests {
    use super::PresentationState;

    #[test]
    fn failure_tracks_whether_the_surface_has_presented() {
        assert_eq!(
            PresentationState::Unpresented.failed(),
            PresentationState::RetryBeforeFirstPresent
        );
        assert_eq!(
            PresentationState::RetryBeforeFirstPresent.failed(),
            PresentationState::RetryBeforeFirstPresent
        );
        assert_eq!(
            PresentationState::Presented.failed(),
            PresentationState::RetryAfterPresent
        );
        assert_eq!(
            PresentationState::RetryAfterPresent.failed(),
            PresentationState::RetryAfterPresent
        );
    }

    #[test]
    fn only_retry_states_require_presentation() {
        assert!(!PresentationState::Unpresented.requires_presentation());
        assert!(!PresentationState::Presented.requires_presentation());
        assert!(PresentationState::RetryBeforeFirstPresent.requires_presentation());
        assert!(PresentationState::RetryAfterPresent.requires_presentation());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameLoop {
    Unconfigured,
    Ticking,
    RescheduleRequested,
    PresentationFailed,
    AwaitingCallback,
    Scheduled,
    RetryScheduled,
    Parked,
}

pub(crate) struct WaylandWindow(pub WaylandWindowStatePtr);
pub enum ImeInput {
    InsertText(String),
    SetMarkedText(String),
    UnmarkText,
    DeleteText,
}

impl Drop for WaylandWindow {
    fn drop(&mut self) {
        self.0.frame_loop.set(FrameLoop::Parked);

        let mut state = self.0.state.borrow_mut();
        let surface_id = state.surface.id();

        if let Some(parent) = state.parent.as_ref() {
            parent.state.borrow_mut().children.remove(&surface_id);
        }

        let client = state.client.clone();

        state.composition.destroy();
        state.renderer.destroy();

        // Destroy blur first, this has no dependencies.
        if let Some(blur) = &state.blur {
            blur.release();
        }

        // Decorations must be destroyed before the xdg state.
        // See https://wayland.app/protocols/xdg-decoration-unstable-v1#zxdg_toplevel_decoration_v1
        if let Some(decoration) = &state.surface_state.decoration() {
            decoration.destroy();
        }

        // Surface state might contain xdg_toplevel/xdg_surface which can be destroyed now that
        // decorations are gone. layer_surface has no dependencies.
        state.surface_state.destroy();

        // Viewport must be destroyed before the wl_surface.
        // See https://wayland.app/protocols/viewporter#wp_viewport
        if let Some(viewport) = &state.viewport {
            viewport.destroy();
        }

        // The wl_surface itself should always be destroyed last.
        state.surface.destroy();

        let state_ptr = self.0.clone();
        state
            .globals
            .executor
            .spawn(async move {
                state_ptr.close();
                client.drop_window(&surface_id)
            })
            .detach();
        drop(state);
    }
}

impl WaylandWindow {
    fn borrow(&self) -> Ref<'_, WaylandWindowState> {
        self.0.state.borrow()
    }

    fn borrow_mut(&self) -> RefMut<'_, WaylandWindowState> {
        self.0.state.borrow_mut()
    }

    pub fn new(
        handle: AnyWindowHandle,
        globals: Globals,
        gpu_context: gpui_wgpu::GpuContext,
        compositor_gpu: Option<CompositorGpuHint>,
        gpu_requirements: Option<gpui_wgpu::WgpuDeviceRequirements>,
        client: WaylandClientStatePtr,
        params: WindowParams,
        appearance: WindowAppearance,
        parent: Option<WaylandWindowStatePtr>,
        popup_grab: Option<(u32, wl_seat::WlSeat)>,
        target_output: Option<wl_output::WlOutput>,
    ) -> anyhow::Result<(Self, ObjectId)> {
        let surface = globals.compositor.create_surface(&globals.qh, ());
        let surface_state = WaylandSurfaceState::new(
            &surface,
            &globals,
            &params,
            parent.clone(),
            popup_grab,
            target_output,
        )?;

        if let Some(fractional_scale_manager) = globals.fractional_scale_manager.as_ref() {
            fractional_scale_manager.get_fractional_scale(&surface, &globals.qh, surface.id());
        }

        let viewport = globals
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, &globals.qh, ()));

        let frame_ping = globals.frame_ping.clone();
        let this = Self(WaylandWindowStatePtr {
            state: Rc::new(RefCell::new(WaylandWindowState::new(
                handle,
                surface.clone(),
                surface_state,
                appearance,
                viewport,
                client,
                globals,
                gpu_context,
                compositor_gpu,
                gpu_requirements,
                params,
                parent,
            )?)),
            callbacks: Rc::new(RefCell::new(Callbacks::default())),
            frame_loop: Rc::new(Cell::new(FrameLoop::Unconfigured)),
            frame_ping,
        });

        // An initial empty commit asks the compositor to configure the surface. If the caller
        // requested a hidden window, defer that commit until the window is shown.
        if this.borrow().visible {
            surface.commit();
        }

        Ok((this, surface.id()))
    }
}

impl WaylandWindowStatePtr {
    pub fn handle(&self) -> AnyWindowHandle {
        self.state.borrow().handle
    }

    pub fn surface(&self) -> wl_surface::WlSurface {
        self.state.borrow().surface.clone()
    }

    pub fn toplevel(&self) -> Option<xdg_toplevel::XdgToplevel> {
        self.state.borrow().surface_state.toplevel().cloned()
    }

    /// The `xdg_surface` backing this window, if it has one. Used to anchor child popups.
    pub fn xdg_surface(&self) -> Option<xdg_surface::XdgSurface> {
        self.state.borrow().surface_state.xdg_surface().cloned()
    }

    /// The layer-shell surface backing this window, if it is one. Used to anchor child popups.
    pub fn layer_surface(&self) -> Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1> {
        self.state.borrow().surface_state.layer_surface().cloned()
    }

    /// This window's xdg window geometry in surface-local coordinates. Child popup anchor
    /// rectangles are relative to it, while gpui coordinates are surface-local.
    pub fn window_geometry(&self) -> Bounds<Pixels> {
        let state = self.state.borrow();
        inset_by_tiling(
            state.bounds.map_origin(|_| px(0.0)),
            state.inset(),
            state.tiling,
        )
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.state, &other.state)
    }

    pub fn add_child(&self, child: ObjectId, blocking: bool) {
        let mut state = self.state.borrow_mut();
        state.children.insert(child, blocking);
    }

    pub fn is_blocked(&self) -> bool {
        let state = self.state.borrow();
        state.children.values().any(|&blocking| blocking)
    }

    pub fn frame(&self) {
        self.frame_loop.set(FrameLoop::Ticking);
        let mut state = self.state.borrow_mut();
        state.resize_throttle = false;
        // GPUI may throttle this tick without calling draw, so leave the request
        // latched until a draw actually reaches the renderer.
        let force_render = state.redraw_requested;
        let require_presentation = state.presentation.requires_presentation();
        drop(state);

        let mut callbacks = self.callbacks.borrow_mut();
        let Some(request_frame_callback) = callbacks.request_frame.as_mut() else {
            self.frame_loop.set(FrameLoop::Parked);
            return;
        };
        request_frame_callback(RequestFrameOptions {
            force_render,
            require_presentation,
        });
        self.update_ime_enabled();
        drop(callbacks);

        self.complete_frame();
    }

    fn complete_frame(&self) {
        let mut state = self.state.borrow_mut();

        let frame_loop = self.frame_loop.get();
        // Hiding during a frame resets the loop; wait for a new configure before retrying.
        if frame_loop == FrameLoop::Unconfigured || frame_loop == FrameLoop::AwaitingCallback {
            return;
        }

        if state.presentation.requires_presentation() {
            // Before the first present, or when throttling skipped draw, a
            // callback may never arrive. Otherwise let the compositor pace
            // retries so an occluded window does not keep polling.
            if frame_loop == FrameLoop::PresentationFailed
                && state.presentation == PresentationState::RetryAfterPresent
            {
                if state.pending_frame_callback.is_none() {
                    let callback = state.surface.frame(&state.globals.qh, state.surface.id());
                    state.pending_frame_callback = Some(callback);
                }
                state.surface.commit();
                self.frame_loop.set(FrameLoop::AwaitingCallback);
                return;
            }

            self.frame_loop.set(FrameLoop::RetryScheduled);
            let surface_id = state.surface.id();
            let client = state.client.clone();
            drop(state);
            client.schedule_frame_retry(&surface_id);
            return;
        }

        if frame_loop == FrameLoop::RescheduleRequested || state.redraw_requested {
            self.frame_loop.set(FrameLoop::RetryScheduled);
            let surface_id = state.surface.id();
            let client = state.client.clone();
            drop(state);
            client.schedule_frame_retry(&surface_id);
            return;
        }

        self.frame_loop.set(FrameLoop::Parked);
    }

    pub fn frame_callback_fired(&self) {
        // Another wl_surface commit may have carried this callback while a retry
        // timer owned the render-loop wakeup.
        self.state.borrow_mut().pending_frame_callback = None;
        if self.frame_loop.get() == FrameLoop::AwaitingCallback {
            self.frame();
        }
    }

    pub fn scheduled_frame_fired(&self) {
        if self.frame_loop.get() == FrameLoop::Scheduled {
            self.frame();
        }
    }

    pub fn retry_timer_fired(&self) {
        if self.frame_loop.get() == FrameLoop::RetryScheduled {
            self.frame();
        }
    }

    pub fn is_configured(&self) -> bool {
        self.frame_loop.get() != FrameLoop::Unconfigured
    }

    pub fn schedule_frame(&self) {
        match self.frame_loop.get() {
            FrameLoop::Parked => {
                self.frame_loop.set(FrameLoop::Scheduled);
                self.frame_ping.ping();
            }
            FrameLoop::Ticking => {
                self.frame_loop.set(FrameLoop::RescheduleRequested);
            }
            // A wake is already armed: a ping or retry timer is in flight, or a
            // presented buffer guarantees a compositor frame callback.
            _ => {}
        }
    }

    fn request_redraw(&self) {
        self.state.borrow_mut().redraw_requested = true;
        self.schedule_frame();
    }

    fn update_ime_enabled(&self) {
        let mut state = self.state.borrow_mut();
        if !state.active {
            return;
        }
        let client = state.client.clone();
        let ime_enabled = if let Some(mut input_handler) = state.input_handler.take() {
            drop(state);
            let accepts_text_input = input_handler.query_accepts_text_input();
            self.state.borrow_mut().input_handler = Some(input_handler);
            accepts_text_input
        } else {
            drop(state);
            true
        };
        if Some(ime_enabled) == client.ime_enabled() {
            return;
        }

        if ime_enabled {
            client.enable_ime();
        } else {
            client.disable_ime();
        }
    }

    pub fn handle_xdg_surface_event(&self, event: xdg_surface::Event) {
        if let xdg_surface::Event::Configure { serial } = event {
            {
                let mut state = self.state.borrow_mut();
                if let Some(window_controls) = state.in_progress_window_controls.take() {
                    state.window_controls = window_controls;

                    drop(state);
                    let mut callbacks = self.callbacks.borrow_mut();
                    if let Some(appearance_changed) = callbacks.appearance_changed.as_mut() {
                        appearance_changed();
                    }
                }
            }
            {
                let mut state = self.state.borrow_mut();

                if let Some(mut configure) = state.in_progress_configure.take() {
                    let got_unmaximized = state.maximized && !configure.maximized;
                    state.fullscreen = configure.fullscreen;
                    state.maximized = configure.maximized;
                    state.tiling = configure.tiling;
                    // Limit interactive resizes to once per vblank
                    if configure.resizing && state.resize_throttle {
                        state.surface_state.ack_configure(serial);
                        return;
                    } else if configure.resizing {
                        state.resize_throttle = true;
                    }
                    if !configure.fullscreen && !configure.maximized {
                        configure.size = if got_unmaximized {
                            Some(state.window_bounds.size)
                        } else {
                            compute_outer_size(state.inset(), configure.size, state.tiling)
                        };
                        if let Some(size) = configure.size {
                            state.window_bounds = Bounds {
                                origin: Point::default(),
                                size,
                            };
                        }
                    }
                    drop(state);
                    if let Some(size) = configure.size {
                        self.resize(size);
                    }
                }
            }
            let state = self.state.borrow_mut();
            state.surface_state.ack_configure(serial);

            let window_geometry = inset_by_tiling(
                state.bounds.map_origin(|_| px(0.0)),
                state.inset(),
                state.tiling,
            )
            .map(|v| f32::from(v) as i32)
            .map_size(|v| if v <= 0 { 1 } else { v });

            state.surface_state.set_geometry(
                window_geometry.origin.x,
                window_geometry.origin.y,
                window_geometry.size.width,
                window_geometry.size.height,
            );

            let initial_configure = self.frame_loop.get() == FrameLoop::Unconfigured;
            let visible = state.visible;
            drop(state);
            if !visible {
                return;
            } else if initial_configure {
                self.frame();
            } else {
                self.request_redraw();
            }
        }
    }

    pub fn handle_toplevel_decoration_event(&self, event: zxdg_toplevel_decoration_v1::Event) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            let decorations = match mode {
                WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ServerSide) => {
                    WindowDecorations::Server
                }
                WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ClientSide) => {
                    WindowDecorations::Client
                }
                WEnum::Value(_) => {
                    log::warn!("Unknown decoration mode");
                    return;
                }
                WEnum::Unknown(v) => {
                    log::warn!("Unknown decoration mode: {}", v);
                    return;
                }
            };
            let previous = self.state.borrow().decorations;
            self.state.borrow_mut().decorations = decorations;
            if let Err(error) = update_window(self.state.borrow_mut()) {
                let mut state = self.state.borrow_mut();
                state.decorations = previous;
                if let Some(decoration) = state.surface_state.decoration() {
                    decoration.set_mode(previous.to_xdg());
                }
                log::error!("failed to update Wayland renderer transparency: {error:#}");
                return;
            }
            let callback = self.callbacks.borrow_mut().appearance_changed.take();
            if let Some(mut fun) = callback {
                fun();
                self.callbacks.borrow_mut().appearance_changed = Some(fun);
            }
            self.request_redraw();
        }
    }

    pub fn handle_fractional_scale_event(&self, event: wp_fractional_scale_v1::Event) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            self.rescale(scale as f32 / 120.0);
            self.request_redraw();
        }
    }

    pub fn handle_toplevel_event(&self, event: xdg_toplevel::Event) -> bool {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                let size = if width == 0 || height == 0 {
                    None
                } else {
                    Some(size(px(width as f32), px(height as f32)))
                };

                let states = extract_states::<xdg_toplevel::State>(&states);

                let mut tiling = Tiling::default();
                let mut fullscreen = false;
                let mut maximized = false;
                let mut resizing = false;

                for state in states {
                    match state {
                        xdg_toplevel::State::Maximized => {
                            maximized = true;
                        }
                        xdg_toplevel::State::Fullscreen => {
                            fullscreen = true;
                        }
                        xdg_toplevel::State::Resizing => resizing = true,
                        xdg_toplevel::State::TiledTop => {
                            tiling.top = true;
                        }
                        xdg_toplevel::State::TiledLeft => {
                            tiling.left = true;
                        }
                        xdg_toplevel::State::TiledRight => {
                            tiling.right = true;
                        }
                        xdg_toplevel::State::TiledBottom => {
                            tiling.bottom = true;
                        }
                        _ => {
                            // noop
                        }
                    }
                }

                if fullscreen || maximized {
                    tiling = Tiling::tiled();
                }

                let mut state = self.state.borrow_mut();
                state.in_progress_configure = Some(InProgressConfigure {
                    size,
                    fullscreen,
                    maximized,
                    resizing,
                    tiling,
                });

                false
            }
            xdg_toplevel::Event::Close => {
                let mut cb = self.callbacks.borrow_mut();
                if let Some(mut should_close) = cb.should_close.take() {
                    let result = (should_close)();
                    cb.should_close = Some(should_close);
                    if result {
                        drop(cb);
                        self.close();
                    }
                    result
                } else {
                    true
                }
            }
            xdg_toplevel::Event::WmCapabilities { capabilities } => {
                let mut window_controls = WindowControls {
                    maximize: false,
                    minimize: false,
                    fullscreen: false,
                    window_menu: false,
                };

                let states = extract_states::<xdg_toplevel::WmCapabilities>(&capabilities);

                for state in states {
                    match state {
                        xdg_toplevel::WmCapabilities::Maximize => {
                            window_controls.maximize = true;
                        }
                        xdg_toplevel::WmCapabilities::Minimize => {
                            window_controls.minimize = true;
                        }
                        xdg_toplevel::WmCapabilities::Fullscreen => {
                            window_controls.fullscreen = true;
                        }
                        xdg_toplevel::WmCapabilities::WindowMenu => {
                            window_controls.window_menu = true;
                        }
                        _ => {}
                    }
                }

                let mut state = self.state.borrow_mut();
                state.in_progress_window_controls = Some(window_controls);
                false
            }
            _ => false,
        }
    }

    pub fn handle_layersurface_event(&self, event: zwlr_layer_surface_v1::Event) -> bool {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                width,
                height,
                serial,
            } => {
                let size = if width == 0 || height == 0 {
                    None
                } else {
                    Some(size(px(width as f32), px(height as f32)))
                };

                let mut state = self.state.borrow_mut();
                state.in_progress_configure = Some(InProgressConfigure {
                    size,
                    fullscreen: false,
                    maximized: false,
                    resizing: false,
                    tiling: Tiling::default(),
                });
                drop(state);

                // just do the same thing we'd do as an xdg_surface
                self.handle_xdg_surface_event(xdg_surface::Event::Configure { serial });

                false
            }
            zwlr_layer_surface_v1::Event::Closed => {
                // unlike xdg, we don't have a choice here: the surface is closing.
                true
            }
            _ => false,
        }
    }

    // Returns `true` if the popup should be closed.
    pub fn handle_popup_event(&self, event: xdg_popup::Event) -> bool {
        match event {
            // Only the size is needed, the position is the compositor's. The following
            // xdg_surface.configure applies the change.
            xdg_popup::Event::Configure { width, height, .. } => {
                let size = if width <= 0 || height <= 0 {
                    None
                } else {
                    Some(size(px(width as f32), px(height as f32)))
                };

                self.state.borrow_mut().in_progress_configure = Some(InProgressConfigure {
                    size,
                    fullscreen: false,
                    maximized: false,
                    resizing: false,
                    tiling: Tiling::default(),
                });

                false
            }
            xdg_popup::Event::PopupDone => true,
            // Precedes the reposition's Configure, which does the work. The token is not needed.
            xdg_popup::Event::Repositioned { .. } => false,
            _ => false,
        }
    }

    #[allow(clippy::mutable_key_type)]
    pub fn handle_surface_event(
        &self,
        event: wl_surface::Event,
        outputs: HashMap<ObjectId, Output>,
    ) {
        let mut state = self.state.borrow_mut();

        match event {
            wl_surface::Event::Enter { output } => {
                let id = output.id();

                let Some(output) = outputs.get(&id) else {
                    return;
                };

                state.outputs.insert(id, output.clone());

                let scale = state.primary_output_scale();
                state.update_subpixel_layout();

                // We use `PreferredBufferScale` instead to set the scale if it's available
                if state.surface.version() < wl_surface::EVT_PREFERRED_BUFFER_SCALE_SINCE {
                    state.surface.set_buffer_scale(scale);
                    drop(state);
                    self.rescale(scale as f32);
                } else {
                    drop(state);
                }
                self.request_redraw();
            }
            wl_surface::Event::Leave { output } => {
                state.outputs.remove(&output.id());

                let scale = state.primary_output_scale();
                state.update_subpixel_layout();

                // We use `PreferredBufferScale` instead to set the scale if it's available
                if state.surface.version() < wl_surface::EVT_PREFERRED_BUFFER_SCALE_SINCE {
                    state.surface.set_buffer_scale(scale);
                    drop(state);
                    self.rescale(scale as f32);
                } else {
                    drop(state);
                }
                self.request_redraw();
            }
            wl_surface::Event::PreferredBufferScale { factor } => {
                // We use `WpFractionalScale` instead to set the scale if it's available
                if state.globals.fractional_scale_manager.is_none() {
                    state.surface.set_buffer_scale(factor);
                    drop(state);
                    self.rescale(factor as f32);
                    self.request_redraw();
                }
            }
            _ => {}
        }
    }

    pub fn handle_ime(&self, ime: ImeInput) {
        if self.is_blocked() {
            return;
        }
        let mut state = self.state.borrow_mut();
        if let Some(mut input_handler) = state.input_handler.take() {
            drop(state);
            match ime {
                ImeInput::InsertText(text) => {
                    input_handler.replace_text_in_range(None, &text);
                }
                ImeInput::SetMarkedText(text) => {
                    input_handler.replace_and_mark_text_in_range(None, &text, None);
                }
                ImeInput::UnmarkText => {
                    input_handler.unmark_text();
                }
                ImeInput::DeleteText => {
                    if let Some(marked) = input_handler.marked_text_range() {
                        input_handler.replace_text_in_range(Some(marked), "");
                    }
                }
            }
            self.state.borrow_mut().input_handler = Some(input_handler);
        }
    }

    pub fn get_ime_area(&self) -> Option<Bounds<Pixels>> {
        let mut state = self.state.borrow_mut();
        let mut bounds: Option<Bounds<Pixels>> = None;
        if let Some(mut input_handler) = state.input_handler.take() {
            drop(state);
            bounds = input_handler.ime_candidate_bounds();
            self.state.borrow_mut().input_handler = Some(input_handler);
        }
        bounds
    }

    pub fn set_size_and_scale(&self, size: Option<Size<Pixels>>, scale: Option<f32>) {
        let (size, scale) = {
            let mut state = self.state.borrow_mut();
            if size.is_none_or(|size| size == state.bounds.size)
                && scale.is_none_or(|scale| scale == state.scale)
            {
                return;
            }
            if let Some(size) = size {
                state.bounds.size = size;
            }
            if let Some(scale) = scale {
                state.scale = scale;
            }
            let device_bounds = state.bounds.to_device_pixels(state.scale);
            state.renderer.update_drawable_size(device_bounds.size);
            let scale = state.scale;
            state.composition.resize(device_bounds.size, scale);
            (state.bounds.size, state.scale)
        };

        let callback = self.callbacks.borrow_mut().resize.take();
        if let Some(mut fun) = callback {
            fun(size, scale);
            self.callbacks.borrow_mut().resize = Some(fun);
        }

        {
            let state = self.state.borrow();
            if let Some(viewport) = &state.viewport {
                viewport
                    .set_destination(f32::from(size.width) as i32, f32::from(size.height) as i32);
            }
        }
    }

    pub fn resize(&self, size: Size<Pixels>) {
        self.set_size_and_scale(Some(size), None);
    }

    pub fn rescale(&self, scale: f32) {
        self.set_size_and_scale(None, Some(scale));
    }

    pub fn close(&self) {
        let state = self.state.borrow();
        let client = state.client.get_client();
        let children = state.children.keys().cloned().collect::<Vec<_>>();
        drop(state);

        for child in children {
            let mut client_state = client.borrow_mut();
            let window = get_window(&mut client_state, &child);
            drop(client_state);

            if let Some(child) = window {
                child.close();
            }
        }
        let mut callbacks = self.callbacks.borrow_mut();
        if let Some(fun) = callbacks.close.take() {
            fun()
        }
    }

    pub fn handle_input(&self, input: PlatformInput) {
        if self.is_blocked() {
            return;
        }
        let callback = self.callbacks.borrow_mut().input.take();
        if let Some(mut fun) = callback {
            let result = fun(input.clone());
            self.callbacks.borrow_mut().input = Some(fun);
            if !result.propagate {
                return;
            }
        }
        if let PlatformInput::KeyDown(event) = input
            && event.keystroke.modifiers.is_subset_of(&Modifiers::shift())
            && let Some(key_char) = &event.keystroke.key_char
        {
            let mut state = self.state.borrow_mut();
            if let Some(mut input_handler) = state.input_handler.take() {
                drop(state);
                input_handler.replace_text_in_range(None, key_char);
                self.state.borrow_mut().input_handler = Some(input_handler);
            }
        }
    }

    pub fn set_focused(&self, focus: bool) {
        self.state.borrow_mut().active = focus;
        let callback = self.callbacks.borrow_mut().active_status_change.take();
        if let Some(mut fun) = callback {
            fun(focus);
            self.callbacks.borrow_mut().active_status_change = Some(fun);
        }
        if let Some(adapter) = self.state.borrow_mut().accesskit_adapter.as_mut() {
            adapter.update_window_focus_state(focus);
        }
    }

    pub fn set_hovered(&self, focus: bool) {
        let callback = self.callbacks.borrow_mut().hover_status_change.take();
        if let Some(mut fun) = callback {
            fun(focus);
            self.callbacks.borrow_mut().hover_status_change = Some(fun);
        }
    }

    pub fn set_appearance(&mut self, appearance: WindowAppearance) {
        self.state.borrow_mut().appearance = appearance;

        let callback = self.callbacks.borrow_mut().appearance_changed.take();
        if let Some(mut fun) = callback {
            fun();
            self.callbacks.borrow_mut().appearance_changed = Some(fun);
        }
    }

    pub fn set_button_layout(&self) {
        let callback = self.callbacks.borrow_mut().button_layout_changed.take();
        if let Some(mut fun) = callback {
            fun();
            self.callbacks.borrow_mut().button_layout_changed = Some(fun);
        }
    }

    pub fn primary_output_scale(&self) -> i32 {
        self.state.borrow_mut().primary_output_scale()
    }

    pub fn regenerate_icons(&self, icon_sizes: &BTreeSet<i32>) {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;

        let Some(icon) = &state.icon else { return };
        let Some(icon_manager) = &state.globals.icon_manager else {
            return;
        };

        let squared = {
            let (width, height) = icon.dimensions();
            let size = width.max(height);

            let mut image = ImageBuffer::from_pixel(size, size, Rgba([0u8, 0, 0, 0]));

            image::imageops::overlay(
                &mut image,
                icon.as_ref(),
                (size - width) as i64 / 2,
                (size - height) as i64 / 2,
            );

            image
        };

        let icons: Vec<_> = icon_sizes
            .iter()
            .copied()
            .filter_map(|size| {
                let mem = memfd::MemfdOptions::new()
                    .allow_sealing(true)
                    .create(format!("icon-{size}x{size}"))
                    .ok()?;

                let buffer_len = size * size * mem::size_of::<Rgba<u8>>() as i32;
                mem.as_file()
                    .set_len(u64::try_from(buffer_len).ok()?)
                    .ok()?;

                let mut fd = FileDescriptor::new(mem);

                let pool =
                    state
                        .globals
                        .shm
                        .create_pool(fd.as_fd(), buffer_len, &state.globals.qh, ());

                let buffer = pool.create_buffer(
                    0,
                    size,
                    size,
                    size * 4,
                    wl_shm::Format::Argb8888,
                    &state.globals.qh,
                    (),
                );

                let image = image::imageops::resize(
                    &squared,
                    size as u32,
                    size as u32,
                    FilterType::CatmullRom,
                )
                .into_raw_bgra();

                fd.write_all(&image).unwrap();

                Some((buffer, fd))
            })
            .collect();

        let icon = icon_manager.create_icon(&state.globals.qh, ());
        icon.set_name(state.app_id.clone().unwrap_or_default());
        state.generated_icons = Vec::new();
        for (ref buffer, fd) in icons {
            icon.add_buffer(buffer, 1);
            state.generated_icons.push(fd)
        }
        icon_manager.set_icon(state.surface_state.toplevel().unwrap(), Some(&icon));
    }
}

fn extract_states<'a, S: TryFrom<u32> + 'a>(states: &'a [u8]) -> impl Iterator<Item = S> + 'a
where
    <S as TryFrom<u32>>::Error: 'a,
{
    states
        .chunks_exact(4)
        .flat_map(TryInto::<[u8; 4]>::try_into)
        .map(u32::from_ne_bytes)
        .flat_map(S::try_from)
}

impl rwh::HasWindowHandle for WaylandWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let surface = self.0.surface().id().as_ptr() as *mut libc::c_void;
        let c_ptr = NonNull::new(surface).ok_or(rwh::HandleError::Unavailable)?;
        let handle = rwh::WaylandWindowHandle::new(c_ptr);
        let raw_handle = rwh::RawWindowHandle::Wayland(handle);
        Ok(unsafe { rwh::WindowHandle::borrow_raw(raw_handle) })
    }
}

impl rwh::HasDisplayHandle for WaylandWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        let display = self
            .0
            .surface()
            .backend()
            .upgrade()
            .ok_or(rwh::HandleError::Unavailable)?
            .display_ptr() as *mut libc::c_void;

        let c_ptr = NonNull::new(display).ok_or(rwh::HandleError::Unavailable)?;
        let handle = rwh::WaylandDisplayHandle::new(c_ptr);
        let raw_handle = rwh::RawDisplayHandle::Wayland(handle);
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(raw_handle) })
    }
}

impl PlatformWindow for WaylandWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.borrow().bounds
    }

    fn is_maximized(&self) -> bool {
        self.borrow().maximized
    }

    fn window_bounds(&self) -> WindowBounds {
        let state = self.borrow();
        if state.fullscreen {
            WindowBounds::Fullscreen(state.window_bounds)
        } else if state.maximized {
            WindowBounds::Maximized(state.window_bounds)
        } else {
            drop(state);
            WindowBounds::Windowed(self.bounds())
        }
    }

    fn inner_window_bounds(&self) -> WindowBounds {
        let state = self.borrow();
        if state.fullscreen {
            WindowBounds::Fullscreen(state.window_bounds)
        } else if state.maximized {
            WindowBounds::Maximized(state.window_bounds)
        } else {
            let inset = state.inset();
            drop(state);
            WindowBounds::Windowed(self.bounds().inset(inset))
        }
    }

    fn content_size(&self) -> Size<Pixels> {
        self.borrow().bounds.size
    }

    fn resize(&mut self, size: Size<Pixels>) {
        let state = self.borrow();
        let state_ptr = self.0.clone();

        // A popup's placement is the compositor's, so a resize re-runs the positioner and the
        // configure reply drives the buffer resize. Before the first configure the popup is
        // unmapped and cannot reposition, but the initial positioner already carries the size.
        if matches!(state.surface_state, WaylandSurfaceState::Popup(_)) {
            if self.0.is_configured() {
                let parent_geometry = state
                    .parent
                    .as_ref()
                    .map(|parent| parent.window_geometry())
                    .unwrap_or_default();
                state
                    .surface_state
                    .reposition_popup(&state.globals, size, parent_geometry);
            }
            return;
        }

        // Keep window geometry consistent with configure handling. On Wayland, window geometry is
        // surface-local: resizing should not attempt to translate the window; the compositor
        // controls placement. We also account for client-side decoration insets and tiling.
        let window_geometry = inset_by_tiling(
            Bounds {
                origin: Point::default(),
                size,
            },
            state.inset(),
            state.tiling,
        )
        .map(|v| f32::from(v) as i32)
        .map_size(|v| if v <= 0 { 1 } else { v });

        state.surface_state.set_geometry(
            window_geometry.origin.x,
            window_geometry.origin.y,
            window_geometry.size.width,
            window_geometry.size.height,
        );

        state
            .globals
            .executor
            .spawn(async move { state_ptr.resize(size) })
            .detach();
    }

    fn scale_factor(&self) -> f32 {
        self.borrow().scale
    }

    fn appearance(&self) -> WindowAppearance {
        self.borrow().appearance
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        let state = self.borrow();
        state.display.as_ref().map(|(id, display)| {
            Rc::new(WaylandDisplay {
                id: id.clone(),
                name: display.name.clone(),
                bounds: display.bounds.to_pixels(state.scale),
            }) as Rc<dyn PlatformDisplay>
        })
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.borrow()
            .client
            .get_client()
            .borrow()
            .mouse_location
            .unwrap_or_default()
    }

    fn modifiers(&self) -> Modifiers {
        self.borrow().client.get_client().borrow().modifiers
    }

    fn capslock(&self) -> Capslock {
        self.borrow().client.get_client().borrow().capslock
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.borrow_mut().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.borrow_mut().input_handler.take()
    }

    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<Receiver<usize>> {
        None
    }

    fn activate(&self, token: Option<&str>) -> bool {
        let state = self.borrow();
        if let Some(token) = token {
            let Some(activation) = &state.globals.activation else {
                return false;
            };
            if token.is_empty() {
                return false;
            }
            activation.activate(token.to_owned(), &state.surface);
            return true;
        }

        // Try to request an activation token. Even though the activation is likely going to be rejected,
        // KWin and Mutter can use the app_id to visually indicate we're requesting attention.
        if let (Some(activation), Some(app_id)) = (&state.globals.activation, state.app_id.clone())
        {
            state.client.set_pending_activation(state.surface.id());
            let token = activation.get_activation_token(&state.globals.qh, ());
            // The serial isn't exactly important here, since the activation is probably going to be rejected anyway.
            let serial = state.client.get_serial(SerialKind::MousePress);
            token.set_app_id(app_id);
            token.set_serial(serial.as_raw(), &state.globals.seat);
            token.set_surface(&state.surface);
            token.commit();
            true
        } else {
            false
        }
    }

    fn request_attention(&self) {}

    fn is_active(&self) -> bool {
        self.borrow().active
    }

    fn is_hovered(&self) -> bool {
        self.borrow().hovered
    }

    fn set_title(&mut self, title: &str) {
        let mut state = self.borrow_mut();
        state.title = Some(title.to_owned());
        if let Some(toplevel) = state.surface_state.toplevel() {
            toplevel.set_title(title.to_string());
        }
    }

    fn set_app_id(&mut self, app_id: &str) {
        let mut state = self.borrow_mut();
        if let Some(toplevel) = state.surface_state.toplevel() {
            toplevel.set_app_id(app_id.to_owned());
        }
        state.app_id = Some(app_id.to_owned());
    }

    fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        let mut state = self.borrow_mut();
        if state.background_appearance == background_appearance {
            return;
        }
        let previous = state.background_appearance;
        state.background_appearance = background_appearance;
        if let Err(error) = update_window(state) {
            self.borrow_mut().background_appearance = previous;
            log::error!("failed to update Wayland renderer transparency: {error:#}");
            return;
        }
        self.0.request_redraw();
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        self.borrow().background_appearance
    }

    fn set_visible(&self, visible: bool) {
        let mut state = self.borrow_mut();
        if state.visible == visible {
            return;
        }

        state.visible = visible;
        if visible {
            // Remapping an xdg surface starts with an empty commit and a fresh configure cycle.
            state.prepare_to_map();
            state.surface.commit();
        } else {
            // Attaching a null buffer unmaps an xdg surface without destroying its role objects.
            // The next map must wait for a new configure before presenting another buffer.
            state.pending_frame_callback.take();
            state.surface.attach(None, 0, 0);
            state.surface.commit();
            state.presentation = PresentationState::Unpresented;
            state.redraw_requested = true;
            self.0.frame_loop.set(FrameLoop::Unconfigured);
        }
    }

    fn is_subpixel_rendering_supported(&self) -> bool {
        let client = self.borrow().client.get_client();
        let state = client.borrow();
        state
            .gpu_context
            .borrow()
            .as_ref()
            .is_some_and(|ctx| ctx.supports_dual_source_blending())
    }

    fn minimize(&self) {
        if let Some(toplevel) = self.borrow().surface_state.toplevel() {
            toplevel.set_minimized();
        }
    }

    fn zoom(&self) {
        let state = self.borrow();
        if let Some(toplevel) = state.surface_state.toplevel() {
            if !state.maximized {
                toplevel.set_maximized();
            } else {
                toplevel.unset_maximized();
            }
        }
    }

    fn toggle_fullscreen(&self) {
        let state = self.borrow();
        if let Some(toplevel) = state.surface_state.toplevel() {
            if !state.fullscreen {
                toplevel.set_fullscreen(None);
            } else {
                toplevel.unset_fullscreen();
            }
        }
    }

    fn is_fullscreen(&self) -> bool {
        self.borrow().fullscreen
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.callbacks.borrow_mut().request_frame = Some(callback);
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> gpui::DispatchEventResult>) {
        self.0.callbacks.borrow_mut().input = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.callbacks.borrow_mut().active_status_change = Some(callback);
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.callbacks.borrow_mut().hover_status_change = Some(callback);
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.callbacks.borrow_mut().resize = Some(callback);
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.0.callbacks.borrow_mut().moved = Some(callback);
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.callbacks.borrow_mut().should_close = Some(callback);
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.0.callbacks.borrow_mut().close = Some(callback);
    }

    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.callbacks.borrow_mut().appearance_changed = Some(callback);
    }

    fn on_button_layout_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.callbacks.borrow_mut().button_layout_changed = Some(callback);
    }

    fn draw(&self, scene: &Scene) {
        let mut state = self.borrow_mut();

        if !state.visible {
            state.redraw_requested = true;
            self.0.frame_loop.set(FrameLoop::Unconfigured);
            return;
        }

        if state.renderer.device_lost() {
            let raw_window = RawWindow {
                window: state.surface.id().as_ptr().cast::<std::ffi::c_void>(),
                display: state
                    .surface
                    .backend()
                    .upgrade()
                    .unwrap()
                    .display_ptr()
                    .cast::<std::ffi::c_void>(),
            };
            match state.renderer.recover(&raw_window) {
                Ok(()) => state.composition.recreate_renderers = true,
                Err(err) => {
                    log::warn!("GPU recovery failed, will retry on next frame: {err}");
                }
            }

            state.redraw_requested = true;
            return;
        }

        // Surface state changed during this GPUI tick is included in this presentation.
        state.redraw_requested = false;
        if state.pending_frame_callback.is_none() {
            let callback = state.surface.frame(&state.globals.qh, state.surface.id());
            state.pending_frame_callback = Some(callback);
        }
        if state.renderer.draw(scene) {
            state.presentation = PresentationState::Presented;
            self.0.frame_loop.set(FrameLoop::AwaitingCallback);
        } else {
            state.presentation = state.presentation.failed();
            self.0.frame_loop.set(FrameLoop::PresentationFailed);
        }

        if state.renderer.needs_redraw() {
            state.redraw_requested = true;
        }
    }

    fn enable_composition(&self) -> anyhow::Result<()> {
        let mut state = self.borrow_mut();
        let WaylandWindowState {
            composition,
            renderer,
            ..
        } = &mut *state;
        composition.enable(renderer)
    }

    fn create_composition_surface(&self) -> anyhow::Result<Rc<dyn PlatformSurfaceAttachment>> {
        let state = self.borrow();
        let child = WaylandSubsurface::new(&state.globals, false)?;
        let surface = Rc::new(RefCell::new(WaylandNativeSurfaceState {
            child,
            bounds: Bounds::default(),
        }));
        Ok(Rc::new(WaylandNativeSurface { state: surface }))
    }

    fn present_composition(&self, frame: CompositionFrame<'_>) -> anyhow::Result<()> {
        let owner = self.0.clone();
        let frame_loop = self.0.frame_loop.clone();
        let mut state = self.borrow_mut();
        if !state.visible {
            state.redraw_requested = true;
            frame_loop.set(FrameLoop::Unconfigured);
            return Ok(());
        }
        let WaylandWindowState {
            renderer,
            composition,
            redraw_requested,
            pending_frame_callback,
            presentation,
            ..
        } = &mut *state;
        composition.present(
            frame,
            renderer,
            owner,
            redraw_requested,
            pending_frame_callback,
            presentation,
            &frame_loop,
        )
    }

    fn schedule_frame(&self) {
        self.0.schedule_frame();
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        let state = self.borrow();
        state.renderer.sprite_atlas().clone()
    }

    fn show_window_menu(&self, position: Point<Pixels>) {
        let state = self.borrow();
        let serial = state.client.get_serial(SerialKind::MousePress);
        if let Some(toplevel) = state.surface_state.toplevel() {
            toplevel.show_window_menu(
                &state.globals.seat,
                serial.as_raw(),
                f32::from(position.x) as i32,
                f32::from(position.y) as i32,
            );
        }
    }

    fn start_window_move(&self) {
        let state = self.borrow();
        let serial = state.client.get_serial(SerialKind::MousePress);
        if let Some(toplevel) = state.surface_state.toplevel() {
            toplevel._move(&state.globals.seat, serial.as_raw());
        }
    }

    fn can_start_external_drag(&self) -> bool {
        true
    }

    fn start_external_drag(&self, payload: &ExternalDragPayload) -> bool {
        let state = self.borrow();
        state.client.start_external_drag(&state.surface, payload)
    }

    fn start_window_resize(&self, edge: gpui::ResizeEdge) {
        let state = self.borrow();
        if let Some(toplevel) = state.surface_state.toplevel() {
            toplevel.resize(
                &state.globals.seat,
                state.client.get_serial(SerialKind::MousePress).as_raw(),
                edge.to_xdg(),
            )
        }
    }

    fn set_exclusive_zone(&self, zone: Pixels) {
        let state = self.borrow();
        if state
            .surface_state
            .set_exclusive_zone(f32::from(zone) as i32)
        {
            // Commit to apply it immediately, otherwise it only takes effect
            // on the next frame.
            state.surface.commit();
        }
    }

    fn set_exclusive_edge(&self, edge: Anchor) {
        let state = self.borrow();
        if state.surface_state.set_exclusive_edge(edge) {
            // Commit to apply it immediately, otherwise it only takes effect
            // on the next frame.
            state.surface.commit();
        }
    }

    fn set_input_region(&self, region: Option<&[Bounds<Pixels>]>) {
        let state = self.borrow();
        match region {
            // No region means the whole surface receives input.
            None => state.surface.set_input_region(None),
            // A region restricts input to its rectangles. An empty region
            // receives no input at all.
            Some(rects) => {
                let wl_region = state
                    .globals
                    .compositor
                    .create_region(&state.globals.qh, ());
                for rect in rects {
                    let rect = rect.map(|pixels| f32::from(pixels) as i32);
                    wl_region.add(
                        rect.origin.x,
                        rect.origin.y,
                        rect.size.width,
                        rect.size.height,
                    );
                }
                state.surface.set_input_region(Some(&wl_region));
                wl_region.destroy();
            }
        }

        // Commit so the new input region applies immediately. Otherwise it
        // waits for the next frame, which could be the very click we want to
        // allow passing through.
        state.surface.commit();
    }

    fn window_decorations(&self) -> Decorations {
        let state = self.borrow();
        match state.decorations {
            WindowDecorations::Server => Decorations::Server,
            WindowDecorations::Client => Decorations::Client {
                tiling: state.tiling,
            },
        }
    }

    fn request_decorations(&self, decorations: WindowDecorations) {
        let mut state = self.borrow_mut();
        let previous = state.decorations;
        match state.surface_state.decoration().as_ref() {
            Some(decoration) => {
                decoration.set_mode(decorations.to_xdg());
                state.decorations = decorations;
            }
            None => {
                if matches!(decorations, WindowDecorations::Server) {
                    log::info!(
                        "Server-side decorations requested, but the Wayland server does not support them. Falling back to client-side decorations."
                    );
                }
                state.decorations = WindowDecorations::Client;
            }
        }
        if let Err(error) = update_window(state) {
            let mut state = self.borrow_mut();
            state.decorations = previous;
            if let Some(decoration) = state.surface_state.decoration() {
                decoration.set_mode(previous.to_xdg());
            }
            log::error!("failed to update Wayland renderer transparency: {error:#}");
            return;
        }
        self.0.request_redraw();
    }

    fn window_controls(&self) -> WindowControls {
        self.borrow().window_controls
    }

    fn set_client_inset(&self, inset: Pixels) {
        let mut state = self.borrow_mut();
        if Some(inset) != state.client_inset {
            let previous = state.client_inset;
            state.client_inset = Some(inset);
            if let Err(error) = update_window(state) {
                self.borrow_mut().client_inset = previous;
                log::error!("failed to update Wayland renderer transparency: {error:#}");
                return;
            }
            self.0.request_redraw();
        }
    }

    fn update_ime_position(&self, bounds: Bounds<Pixels>) {
        let state = self.borrow();
        if !state.active {
            return;
        }
        state.client.update_ime_position(bounds);
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.borrow().renderer.gpu_specs().into()
    }

    fn gpu_context(&self) -> Option<Box<dyn std::any::Any>> {
        let (device, queue) = self.borrow().renderer.gpu_context();
        Some(Box::new((device, queue)))
    }

    fn gpu_context_info(&self) -> Option<Box<dyn std::any::Any>> {
        self.borrow()
            .renderer
            .gpu_context_info()
            .map(|context| Box::new(context) as Box<dyn std::any::Any>)
    }

    fn gpu_device_lost(&self) -> Option<bool> {
        // Only loads an atomic flag — safe even mid-recovery, when
        // `gpu_context` would panic on the torn-down resources.
        Some(self.borrow().renderer.device_lost())
    }

    fn play_system_bell(&self) {
        let state = self.borrow();
        let surface = if state.surface_state.toplevel().is_some() {
            Some(&state.surface)
        } else {
            None
        };
        if let Some(bell) = state.globals.system_bell.as_ref() {
            bell.ring(surface);
        }
    }

    fn a11y_init(&self, callbacks: gpui::A11yCallbacks) {
        let activation_handler = TrivialActivationHandler {
            callback: callbacks.activation,
        };
        let action_handler = TrivialActionHandler(callbacks.action);
        let deactivation_handler = TrivialDeactivationHandler {
            callback: callbacks.deactivation,
        };

        let adapter =
            accesskit_unix::Adapter::new(activation_handler, action_handler, deactivation_handler);

        self.borrow_mut().accesskit_adapter = Some(adapter);
    }

    fn a11y_tree_update(&self, tree_update: accesskit::TreeUpdate) {
        let mut state = self.borrow_mut();
        if let Some(adapter) = state.accesskit_adapter.as_mut() {
            adapter.update_if_active(|| tree_update);
        }
    }

    fn a11y_update_window_bounds(&self) {
        // Wayland doesn't expose window position, so this is a no-op
    }
}

struct TrivialActivationHandler {
    callback: Box<dyn Fn() -> Option<accesskit::TreeUpdate> + Send + 'static>,
}

impl accesskit::ActivationHandler for TrivialActivationHandler {
    fn request_initial_tree(&mut self) -> Option<accesskit::TreeUpdate> {
        (self.callback)()
    }
}

struct TrivialActionHandler(Box<dyn Fn(accesskit::ActionRequest) + Send + 'static>);

impl accesskit::ActionHandler for TrivialActionHandler {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        (self.0)(request);
    }
}

struct TrivialDeactivationHandler {
    callback: Box<dyn Fn() + Send + 'static>,
}

impl accesskit::DeactivationHandler for TrivialDeactivationHandler {
    fn deactivate_accessibility(&mut self) {
        (self.callback)();
    }
}

fn update_window(mut state: RefMut<WaylandWindowState>) -> anyhow::Result<()> {
    let opaque = !state.is_transparent();

    state.renderer.update_transparency(!opaque)?;
    let opaque_area = state.window_bounds.map(|v| f32::from(v) as i32);
    opaque_area.inset(f32::from(state.inset()) as i32);

    let region = state
        .globals
        .compositor
        .create_region(&state.globals.qh, ());
    region.add(
        opaque_area.origin.x,
        opaque_area.origin.y,
        opaque_area.size.width,
        opaque_area.size.height,
    );

    // Note that rounded corners make this rectangle API hard to work with.
    // As this is common when using CSD, let's just disable this API.
    if state.background_appearance.is_opaque() && state.decorations == WindowDecorations::Server {
        // Promise the compositor that this region of the window surface
        // contains no transparent pixels. This allows the compositor to skip
        // updating whatever is behind the surface for better performance.
        state.surface.set_opaque_region(Some(&region));
    } else {
        state.surface.set_opaque_region(None);
    }

    if let Some(ref blur_manager) = state.globals.blur_manager {
        if state.background_appearance == WindowBackgroundAppearance::Blurred {
            if state.blur.is_none() {
                let blur = blur_manager.create(&state.surface, &state.globals.qh, ());
                state.blur = Some(blur);
            }
            state.blur.as_ref().unwrap().commit();
        } else {
            // It probably doesn't hurt to clear the blur for opaque windows
            blur_manager.unset(&state.surface);
            if let Some(b) = state.blur.take() {
                b.release()
            }
        }
    }

    region.destroy();
    Ok(())
}

pub(crate) trait WindowDecorationsExt {
    fn to_xdg(self) -> zxdg_toplevel_decoration_v1::Mode;
}

impl WindowDecorationsExt for WindowDecorations {
    fn to_xdg(self) -> zxdg_toplevel_decoration_v1::Mode {
        match self {
            WindowDecorations::Client => zxdg_toplevel_decoration_v1::Mode::ClientSide,
            WindowDecorations::Server => zxdg_toplevel_decoration_v1::Mode::ServerSide,
        }
    }
}

pub(crate) trait ResizeEdgeWaylandExt {
    fn to_xdg(self) -> xdg_toplevel::ResizeEdge;
}

impl ResizeEdgeWaylandExt for ResizeEdge {
    fn to_xdg(self) -> xdg_toplevel::ResizeEdge {
        match self {
            ResizeEdge::Top => xdg_toplevel::ResizeEdge::Top,
            ResizeEdge::TopRight => xdg_toplevel::ResizeEdge::TopRight,
            ResizeEdge::Right => xdg_toplevel::ResizeEdge::Right,
            ResizeEdge::BottomRight => xdg_toplevel::ResizeEdge::BottomRight,
            ResizeEdge::Bottom => xdg_toplevel::ResizeEdge::Bottom,
            ResizeEdge::BottomLeft => xdg_toplevel::ResizeEdge::BottomLeft,
            ResizeEdge::Left => xdg_toplevel::ResizeEdge::Left,
            ResizeEdge::TopLeft => xdg_toplevel::ResizeEdge::TopLeft,
        }
    }
}

/// The configuration event is in terms of the window geometry, which we are constantly
/// updating to account for the client decorations. But that's not the area we want to render
/// to, due to our intrusize CSD. So, here we calculate the 'actual' size, by adding back in the insets
fn compute_outer_size(
    inset: Pixels,
    new_size: Option<Size<Pixels>>,
    tiling: Tiling,
) -> Option<Size<Pixels>> {
    new_size.map(|mut new_size| {
        if !tiling.top {
            new_size.height += inset;
        }
        if !tiling.bottom {
            new_size.height += inset;
        }
        if !tiling.left {
            new_size.width += inset;
        }
        if !tiling.right {
            new_size.width += inset;
        }

        new_size
    })
}

fn inset_by_tiling(mut bounds: Bounds<Pixels>, inset: Pixels, tiling: Tiling) -> Bounds<Pixels> {
    if !tiling.top {
        bounds.origin.y += inset;
        bounds.size.height -= inset;
    }
    if !tiling.bottom {
        bounds.size.height -= inset;
    }
    if !tiling.left {
        bounds.origin.x += inset;
        bounds.size.width -= inset;
    }
    if !tiling.right {
        bounds.size.width -= inset;
    }

    bounds
}
