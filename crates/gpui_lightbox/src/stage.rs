//! The [`Stage`]: a headless, deterministic window to mount a view in, drive
//! with real input and a fake clock, and photograph.

use crate::{
    compose::Compositor,
    output::Suite,
    shot::{QuadInfo, Rect, Shot, ShotMeta, TextLine, TextQueryError, find_text},
};
use gpui::inspector::{ElementFlags, InspectorDock};
use gpui::{
    AnyWindowHandle, App, AssetSource, Entity, HeadlessAppContext, Modifiers, Pixels,
    PlatformHeadlessRenderer, Point, Render, Size, Window, WindowAppearance, px, size,
};
use gpui_wgpu::{CosmicTextSystem, WgpuHeadlessRenderer};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fmt, panic::Location, sync::Arc, time::Duration};

/// The UI font Lightbox bundles and uses for `.SystemUIFont`.
pub const UI_FONT: &str = "IBM Plex Sans";
/// The monospace font Lightbox bundles.
pub const MONO_FONT: &str = "Lilex";

const BUNDLED_FONTS: [&[u8]; 8] = [
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Italic.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBoldItalic.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Bold.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Italic.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-BoldItalic.ttf"),
];

/// Light or dark: the system appearance the window reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Light mode.
    #[default]
    Light,
    /// Dark mode.
    Dark,
}

impl Appearance {
    /// `Light` or `Dark`.
    pub fn label(self) -> &'static str {
        match self {
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }
}

impl From<Appearance> for WindowAppearance {
    fn from(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => WindowAppearance::Light,
            Appearance::Dark => WindowAppearance::Dark,
        }
    }
}

impl From<WindowAppearance> for Appearance {
    fn from(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
        }
    }
}

/// The fonts a stage can render with. No system fonts are ever loaded, so a
/// stage renders the same text on every machine.
#[derive(Clone)]
pub struct Fonts {
    data: Vec<Cow<'static, [u8]>>,
    system_ui: String,
}

impl Fonts {
    /// IBM Plex Sans (regular, italic, semibold, semibold italic) and Lilex
    /// (regular, bold, italic, bold italic) from `assets/fonts`, with
    /// `.SystemUIFont` resolving to IBM Plex Sans.
    pub fn bundled() -> Self {
        Self {
            data: BUNDLED_FONTS
                .iter()
                .map(|font| Cow::Borrowed(*font))
                .collect(),
            system_ui: UI_FONT.into(),
        }
    }

    /// Adds a font file (TTF/OTF bytes).
    pub fn with(mut self, font: impl Into<Cow<'static, [u8]>>) -> Self {
        self.data.push(font.into());
        self
    }

    /// Sets the family that `.SystemUIFont` (gpui's default) resolves to.
    pub fn system_ui(mut self, family: &str) -> Self {
        self.system_ui = family.into();
        self
    }

    /// The family `.SystemUIFont` resolves to.
    pub fn system_ui_family(&self) -> &str {
        &self.system_ui
    }

    /// The font files, for registering with a text system.
    pub(crate) fn data(&self) -> Vec<Cow<'static, [u8]>> {
        self.data.clone()
    }
}

impl Default for Fonts {
    fn default() -> Self {
        Self::bundled()
    }
}

impl fmt::Debug for Fonts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fonts")
            .field("files", &self.data.len())
            .field("system_ui", &self.system_ui)
            .finish()
    }
}

/// How a [`Stage`] renders: window size, scale, appearance, fonts and assets.
///
/// ```ignore
/// let config = StageConfig::default().size(480., 320.).scale(1.).appearance(Appearance::Dark);
/// ```
#[derive(Clone)]
pub struct StageConfig {
    /// The window's size in logical pixels.
    pub size: Size<Pixels>,
    /// Device pixels per logical pixel: 1.0 or 2.0 (retina).
    pub scale: f32,
    /// Light or dark system appearance.
    pub appearance: Appearance,
    /// The fonts available to text.
    pub fonts: Fonts,
    /// Where `svg()` and `img()` paths load from.
    pub assets: Arc<dyn AssetSource>,
}

impl Default for StageConfig {
    fn default() -> Self {
        Self {
            size: size(px(800.), px(600.)),
            scale: 2.,
            appearance: Appearance::Light,
            fonts: Fonts::bundled(),
            assets: Arc::new(()),
        }
    }
}

impl StageConfig {
    /// Sets the window size in logical pixels.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = size(px(width), px(height));
        self
    }

    /// Sets the scale factor.
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    /// Sets the appearance.
    pub fn appearance(mut self, appearance: Appearance) -> Self {
        self.appearance = appearance;
        self
    }

    /// Sets the fonts.
    pub fn fonts(mut self, fonts: Fonts) -> Self {
        self.fonts = fonts;
        self
    }

    /// Sets the asset source.
    pub fn assets(mut self, assets: impl AssetSource) -> Self {
        self.assets = Arc::new(assets);
        self
    }
}

impl fmt::Debug for StageConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StageConfig")
            .field("size", &self.size)
            .field("scale", &self.scale)
            .field("appearance", &self.appearance)
            .field("fonts", &self.fonts)
            .finish_non_exhaustive()
    }
}

/// The mouse as the stage last moved it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Pointer {
    pub(crate) position: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
}

/// A headless, deterministic window: mount a view, drive it with real input
/// and a fake clock, and take [`Shot`]s.
///
/// Everything is reproducible: fonts are bundled (no system fonts), the clock
/// only moves on [`advance`](Self::advance), tasks run in a seeded order
/// (`SEED`, default 0) and pixels come from the same wgpu pipeline as the
/// real app (Mesa lavapipe when there is no GPU).
///
/// Lightbox is a test harness: like `assert!`, its methods panic with a
/// descriptive message instead of returning errors.
///
/// ```ignore
/// let mut stage = Stage::new("settings");
/// stage.mount(|_, cx| cx.new(|cx| Settings::new(cx)));
/// stage.click_text("Advanced").advance(Duration::from_millis(200));
/// let shot = stage.shot("advanced-open");
/// shot.lint(&StyleSpec::loupe()).assert_clean();
/// ```
pub struct Stage {
    cx: HeadlessAppContext,
    window: Option<AnyWindowHandle>,
    config: StageConfig,
    suite: Suite,
    elapsed: Duration,
    pub(crate) compositor: Compositor,
    pub(crate) pointer: Pointer,
}

impl Stage {
    /// A stage with the default config (800×600 at 2×, light, bundled fonts)
    /// that saves shots to `target/lightbox/<suite>/`.
    pub fn new(suite: &str) -> Self {
        Self::with_config(suite, StageConfig::default())
    }

    /// A stage with an explicit config.
    pub fn with_config(suite: &str, config: StageConfig) -> Self {
        Self::in_suite(Suite::new(suite), config)
    }

    /// A stage writing into an explicit [`Suite`].
    pub fn in_suite(suite: Suite, config: StageConfig) -> Self {
        let text_system = Arc::new(CosmicTextSystem::new_without_system_fonts(
            config.fonts.system_ui_family(),
        ));
        let mut cx = HeadlessAppContext::with_platform(text_system, config.assets.clone(), || {
            WgpuHeadlessRenderer::new()
                .map_err(|error| log::error!("lightbox: no headless GPU renderer: {error:#}"))
                .ok()
                .map(|renderer| Box::new(renderer) as Box<dyn PlatformHeadlessRenderer>)
        });
        let fonts = config.fonts.data();
        if let Err(error) = cx.update(|cx| cx.text_system().add_fonts(fonts)) {
            panic!("lightbox: loading the stage's fonts: {error:#}");
        }
        Self {
            cx,
            window: None,
            config,
            suite,
            elapsed: Duration::ZERO,
            compositor: Compositor::default(),
            pointer: Pointer::default(),
        }
    }

    /// The suite shots are saved to.
    pub fn suite(&self) -> &Suite {
        &self.suite
    }

    /// The current configuration (kept up to date by [`resize`](Self::resize),
    /// [`set_scale`](Self::set_scale) and [`set_appearance`](Self::set_appearance)).
    pub fn config(&self) -> &StageConfig {
        &self.config
    }

    /// Opens the stage's window with the view `build` returns as its root,
    /// replacing any previously mounted view, and draws the first frame.
    pub fn mount<V: Render + 'static>(
        &mut self,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Entity<V> {
        if let Some(window) = self.window.take() {
            self.cx
                .update_window(window, |_, window, _| window.remove_window())
                .ok();
        }
        // The first frame already has the configured appearance and scale, so
        // style transitions don't animate from the platform's defaults.
        self.cx
            .set_window_defaults(self.config.appearance.into(), self.config.scale);
        let handle = self
            .cx
            .open_window(self.config.size, build)
            .unwrap_or_else(|error| panic!("lightbox: opening the stage window: {error:#}"));
        let root = handle
            .root(&mut self.cx)
            .unwrap_or_else(|error| panic!("lightbox: reading the stage's root view: {error:#}"));
        self.window = Some(handle.into());
        self.update(|window, _| window.activate_window());
        root
    }

    /// The mounted window.
    ///
    /// # Panics
    ///
    /// If nothing is mounted.
    #[track_caller]
    pub fn window(&self) -> AnyWindowHandle {
        self.window
            .expect("lightbox: mount a view first (stage.mount(|window, cx| …))")
    }

    /// Runs `f` with the window and app, then runs pending tasks and redraws
    /// what changed. The escape hatch for anything Lightbox doesn't wrap.
    #[track_caller]
    pub fn update<R>(&mut self, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        let window = self.window();
        let result = self
            .cx
            .update_window(window, |_, window, cx| f(window, cx))
            .unwrap_or_else(|error| panic!("lightbox: the stage window closed: {error:#}"));
        self.settle();
        result
    }

    /// The underlying headless app, for what [`update`](Self::update) can't reach.
    pub fn app(&mut self) -> &mut HeadlessAppContext {
        &mut self.cx
    }

    /// Runs pending tasks; windows that changed are redrawn as effects flush.
    pub(crate) fn settle(&mut self) {
        self.cx.run_until_parked();
        if let Some(window) = self.window {
            // An empty update flushes effects, which draws dirty windows.
            self.cx.update_window(window, |_, _, _| {}).ok();
            self.cx.run_until_parked();
        }
    }

    /// Draws a frame now, even if nothing changed.
    #[track_caller]
    pub fn draw(&mut self) -> &mut Self {
        self.settle();
        let window = self.window();
        self.cx
            .update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .ok();
        self
    }

    /// Re-renders every view (as a window resize would) and draws.
    #[track_caller]
    pub fn refresh(&mut self) -> &mut Self {
        self.update(|window, _| window.refresh());
        self
    }

    /// Advances the fake clock by `by`, fires the timers that fall due,
    /// delivers the animation frame views asked for, and draws. Animations,
    /// transitions and springs then show exactly their state at the new time.
    #[track_caller]
    pub fn advance(&mut self, by: Duration) -> &mut Self {
        let window = self.window();
        self.cx.advance_clock(by);
        self.elapsed += by;
        self.cx
            .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
            .ok();
        self.settle();
        self
    }

    /// Total time the clock was advanced since the stage was created.
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Resizes the window (logical pixels) and draws.
    pub fn resize(&mut self, width: f32, height: f32) -> &mut Self {
        self.config.size = size(px(width), px(height));
        if self.window.is_some() {
            let new_size = self.config.size;
            self.update(|window, cx| {
                window.resize(new_size);
                window.bounds_changed(cx);
            });
        }
        self
    }

    /// Changes the scale factor (device pixels per logical pixel) and draws.
    pub fn set_scale(&mut self, scale: f32) -> &mut Self {
        self.config.scale = scale;
        if let Some(window) = self.window {
            if let Err(error) = self.cx.simulate_scale_factor_change(window, scale) {
                panic!("lightbox: changing the scale factor: {error:#}");
            }
            self.settle();
        }
        self
    }

    /// Changes the system appearance, re-renders every view and draws. As in
    /// the app, style transitions animate the change: advance the clock to
    /// see the settled state, or configure the appearance before mounting.
    pub fn set_appearance(&mut self, appearance: Appearance) -> &mut Self {
        self.config.appearance = appearance;
        if let Some(window) = self.window {
            if let Err(error) = self
                .cx
                .simulate_appearance_change(window, appearance.into())
            {
                panic!("lightbox: changing the appearance: {error:#}");
            }
            self.cx.run_until_parked();
            self.refresh();
        }
        self
    }

    /// Opens gpui's inspector capture with a zero-width dock: the engine
    /// then records each frame's element tree and phase timings while the
    /// app keeps the whole window. Shots taken afterwards know which elements
    /// are clickable (for the hit-target lint rule), and benchmarks report
    /// per-phase times.
    pub fn record_elements(&mut self) -> &mut Self {
        self.update(|window, cx| {
            if !window.is_inspector_open() {
                window.toggle_inspector(cx);
            }
            if let Some(capture) = window.inspector_capture_mut() {
                capture.set_dock(InspectorDock::Hidden);
            }
            window.refresh();
        });
        self
    }

    /// Every line of text painted in the last frame.
    #[track_caller]
    pub fn texts(&mut self) -> Vec<TextLine> {
        self.settle();
        self.read(|window| {
            window
                .painted_text()
                .iter()
                .map(|line| TextLine::from_painted(line, window))
                .collect()
        })
    }

    /// The unique visible line of text that is exactly `text`.
    #[track_caller]
    pub fn find_text(&mut self, text: &str) -> Result<TextLine, TextQueryError> {
        find_text(&self.texts(), text).cloned()
    }

    /// Every quad painted in the last frame, in logical pixels.
    #[track_caller]
    pub fn quads(&mut self) -> Vec<QuadInfo> {
        self.settle();
        self.read(|window| {
            let scale = window.scale_factor();
            window
                .painted_quads()
                .iter()
                .map(|quad| QuadInfo::from_quad(quad, scale))
                .collect()
        })
    }

    #[track_caller]
    fn read<R>(&mut self, f: impl FnOnce(&Window) -> R) -> R {
        let window = self.window();
        self.cx
            .update_window(window, |_, window, _| f(window))
            .unwrap_or_else(|error| panic!("lightbox: the stage window closed: {error:#}"))
    }

    /// Renders the current frame and saves it as `target/lightbox/<suite>/<name>.png`,
    /// with a record the report uses for its overlays.
    #[track_caller]
    pub fn shot(&mut self, name: &str) -> Shot {
        let location = Location::caller();
        let shot = self.capture(name);
        shot.save(location);
        shot
    }

    /// Renders the current frame into a [`Shot`] without saving it.
    #[track_caller]
    pub fn capture(&mut self, name: &str) -> Shot {
        self.settle();
        let system_font = self.config.fonts.system_ui_family().to_string();
        let time_ms = self.elapsed.as_secs_f64() * 1000.;
        let (image, texts, quads, meta, clickables) = self.read(|window| {
            let scale = window.scale_factor();
            let viewport = window.viewport_size();
            let image = window.render_to_image().unwrap_or_else(|error| {
                panic!(
                    "lightbox: rendering {name:?}: {error:#}\n  Lightbox renders through wgpu; \
                         without a GPU, install a software Vulkan driver (Mesa lavapipe)."
                )
            });
            let texts = window
                .painted_text()
                .iter()
                .map(|line| TextLine::from_painted(line, window))
                .collect();
            let quads = window
                .painted_quads()
                .iter()
                .map(|quad| QuadInfo::from_quad(quad, scale))
                .collect();
            let meta = ShotMeta {
                size: [viewport.width.as_f32(), viewport.height.as_f32()],
                scale,
                appearance: window.appearance().into(),
                time_ms,
                system_font,
            };
            (image, texts, quads, meta, clickables(window))
        });
        Shot {
            name: name.into(),
            image,
            texts,
            quads,
            clickables,
            meta,
            suite: self.suite.clone(),
            compositor: self.compositor.clone(),
        }
    }
}

/// The visible bounds of clickable elements in the latest recorded element
/// tree, or `None` when no tree was recorded.
fn clickables(window: &Window) -> Option<Vec<Rect>> {
    let tree = window.inspector_capture()?.latest_tree()?;
    if tree.elements.is_empty() {
        return None;
    }
    Some(
        tree.elements
            .iter()
            .filter(|element| element.flags.contains(ElementFlags::CLICKABLE))
            .filter_map(|element| element.visible_bounds)
            .map(Rect::from)
            .collect(),
    )
}
