//! Lightbox draws its own images (contact sheets, film strips, lint
//! annotations, comparisons) with gpui itself: a small headless app with the
//! bundled fonts renders an element tree to pixels.

use crate::{
    color::Color,
    shot::Rect,
    stage::{Fonts, MONO_FONT, UI_FONT},
    theme::DARK,
};
use anyhow::{Context as _, Result, anyhow};
use gpui::{
    AnyElement, App, AppContext as _, Context, Div, FontWeight, HeadlessAppContext, IntoElement,
    ParentElement as _, PlatformHeadlessRenderer, Render, RenderImage, SharedString, Styled as _,
    Window, WindowHandle, div, img, px, size,
};
use gpui_wgpu::{CosmicTextSystem, WgpuHeadlessRenderer};
use image::{RgbaImage, imageops::FilterType};
use std::{cell::RefCell, rc::Rc, sync::Arc};

/// The largest image side a composition may have, in device pixels.
const MAX_SIDE: f32 = 16_384.;

type Content = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

struct Canvas {
    content: Option<Content>,
}

impl Render for Canvas {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.content {
            Some(content) => content(window, cx),
            None => div().into_any_element(),
        }
    }
}

/// Renders element trees to images in a headless gpui app of its own.
///
/// ```ignore
/// let mut composer = Composer::new()?;
/// let photo = composer.image(&shot.image);
/// let image = composer.render(size(px(400.), px(300.)), 2., move |_, _| {
///     div().size_full().bg(DARK.bg).child(picture(photo.clone(), Rect::new(0., 0., 400., 300.)))
///         .into_any_element()
/// })?;
/// ```
pub struct Composer {
    cx: HeadlessAppContext,
    window: WindowHandle<Canvas>,
    scale: f32,
    images: Vec<Arc<RenderImage>>,
}

impl Composer {
    /// Starts the composer's app and window.
    pub fn new() -> Result<Self> {
        let fonts = Fonts::bundled();
        let text_system = Arc::new(CosmicTextSystem::new_without_system_fonts(
            fonts.system_ui_family(),
        ));
        let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(()), || {
            WgpuHeadlessRenderer::new()
                .ok()
                .map(|renderer| Box::new(renderer) as Box<dyn PlatformHeadlessRenderer>)
        });
        cx.update(|cx| cx.text_system().add_fonts(fonts.data()))?;
        let window = cx.open_window(size(px(64.), px(64.)), |_, cx| {
            cx.new(|_| Canvas { content: None })
        })?;
        Ok(Self {
            cx,
            window,
            scale: 2.,
            images: Vec::new(),
        })
    }

    /// Uploads `image` for the next [`render`](Self::render); it is released
    /// from the GPU atlas afterwards.
    pub fn image(&mut self, image: &RgbaImage) -> Arc<RenderImage> {
        let mut bgra = image.clone();
        for pixel in bgra.pixels_mut() {
            pixel.0.swap(0, 2);
        }
        let image = Arc::new(RenderImage::new([image::Frame::new(bgra)]));
        self.images.push(image.clone());
        image
    }

    /// Renders what `content` builds into an image of `width × height`
    /// logical pixels at `scale`.
    pub fn render(
        &mut self,
        width: f32,
        height: f32,
        scale: f32,
        content: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Result<RgbaImage> {
        let (width, height) = (width.ceil().max(1.), height.ceil().max(1.));
        if width * scale > MAX_SIDE || height * scale > MAX_SIDE {
            return Err(anyhow!(
                "a {width}×{height} composition at {scale}× exceeds the {MAX_SIDE} px texture limit"
            ));
        }
        let handle = self.window.into();
        if scale != self.scale {
            self.cx.simulate_scale_factor_change(handle, scale)?;
            self.scale = scale;
        }
        self.window.update(&mut self.cx, |canvas, window, cx| {
            window.resize(size(px(width), px(height)));
            window.bounds_changed(cx);
            canvas.content = Some(Rc::new(content));
            cx.notify();
        })?;
        self.cx.run_until_parked();
        let image = self
            .cx
            .update_window(handle, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.render_to_image()
            })?
            .context("rendering a composition");

        let images = std::mem::take(&mut self.images);
        self.window.update(&mut self.cx, |canvas, window, _| {
            canvas.content = None;
            for image in images {
                window.drop_image(image).ok();
            }
        })?;
        image
    }
}

/// A lazily started [`Composer`] shared by a stage and the shots it takes.
#[derive(Clone, Default)]
pub(crate) struct Compositor(Rc<RefCell<Option<Composer>>>);

impl Compositor {
    /// Runs `f` with the composer, starting it on first use.
    pub(crate) fn with<R>(&self, f: impl FnOnce(&mut Composer) -> Result<R>) -> Result<R> {
        let mut slot = self.0.borrow_mut();
        if slot.is_none() {
            *slot = Some(Composer::new()?);
        }
        let composer = slot.as_mut().context("composer just started")?;
        f(composer)
    }
}

/// Resizes to exactly `width × height`: a sharp downsampling filter when
/// shrinking, nearest-neighbor when enlarging (magnified pixels stay honest).
pub fn resample(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let (width, height) = (width.max(1), height.max(1));
    if (width, height) == image.dimensions() {
        return image.clone();
    }
    let filter = if width < image.width() {
        FilterType::CatmullRom
    } else {
        FilterType::Nearest
    };
    image::imageops::resize(image, width, height, filter)
}

/// An image placed at exact logical `bounds`. Upload it resampled to
/// `bounds × scale` device pixels so it's drawn one to one.
pub(crate) fn picture(image: Arc<RenderImage>, bounds: Rect) -> AnyElement {
    img(image)
        .absolute()
        .left(px(bounds.x))
        .top(px(bounds.y))
        .w(px(bounds.w))
        .h(px(bounds.h))
        .into_any_element()
}

/// A box placed at exact logical `bounds`.
pub(crate) fn placed(bounds: Rect) -> Div {
    div()
        .absolute()
        .left(px(bounds.x))
        .top(px(bounds.y))
        .w(px(bounds.w))
        .h(px(bounds.h))
}

/// A composition's root: the dark backdrop in the UI font.
pub(crate) fn backdrop() -> Div {
    div()
        .relative()
        .size_full()
        .bg(DARK.bg)
        .font_family(UI_FONT)
        .text_color(DARK.text)
        .text_size(px(12.))
        .line_height(px(16.))
}

/// A single line of UI text.
pub(crate) fn label(text: impl Into<SharedString>, size: f32, color: Color) -> Div {
    div()
        .text_size(px(size))
        .line_height(px((size * 1.34).round()))
        .text_color(color)
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .child(text.into())
}

/// A single line of monospace text (values, sizes, times).
pub(crate) fn mono(text: impl Into<SharedString>, size: f32, color: Color) -> Div {
    label(text, size, color).font_family(MONO_FONT)
}

/// A semibold line of UI text.
pub(crate) fn heading(text: impl Into<SharedString>, size: f32, color: Color) -> Div {
    label(text, size, color).font_weight(FontWeight::SEMIBOLD)
}

/// An uppercase section label with tracking, as in Loupe.
pub(crate) fn section(text: &str) -> Div {
    label(text.to_uppercase(), 10.5, DARK.text_faint).font_weight(FontWeight::SEMIBOLD)
}

/// A small pill: a colored dot and a label.
pub(crate) fn pill(text: impl Into<SharedString>, color: Color) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(20.))
        .px(px(8.))
        .bg(DARK.surface_2)
        .border_1()
        .border_color(DARK.line_strong)
        .rounded(px(10.))
        .child(div().size(px(6.)).rounded(px(3.)).bg(color))
        .child(label(text, 11., DARK.text_muted))
}

/// A numbered badge for annotations.
pub(crate) fn badge(number: usize, color: Color) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_w(px(18.))
        .h(px(18.))
        .px(px(4.))
        .rounded(px(9.))
        .bg(color)
        .child(
            mono(number.to_string(), 11., Color::from_u32(0x111214)).font_weight(FontWeight::BOLD),
        )
}
