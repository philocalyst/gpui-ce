//! The root view docked in the inspected window: the shell around the lenses.
//!
//! ```text
//! toolbar      pick · overlays · freeze · find · dock · close
//! pulse strip  one bar per frame · fps · p95
//! lens rail    Elements · Frames · Events · Entities · Audit (live counts)
//! lens body    the active lens (a cached view)
//! status bar   selection breadcrumb · app ms · loupe ms · memory · FROZEN
//! ```
//!
//! Loupe never calls `window.refresh()`. A 100 ms timer compares the
//! capture's generation with the one it last saw and notifies the shared
//! [`LoupeState`] only when the app recorded something new; frames that only
//! Loupe caused do not move the generation, so there is no redraw loop.

use crate::{
    analysis::{format, stats::frame_stats},
    commands::{Command, DockSide, keys},
    lenses::{Lenses, entity_label, entity_names},
    palette::{Palette, PaletteEvent, PaletteGlyph, PaletteItem, PaletteTarget},
    shell::{
        pulse::{self, capture_bars, frame_at, paint_pulse},
        status::{StatusBar, breadcrumb},
        toolbar::{Run, Toolbar},
    },
    state::{Lens, LoupeState},
    theme::{LoupeSettings, MONO_FONT, Theme, UI_FONT},
    widgets::{self, RailTab, SplitDrag, SplitHandle, TabRail, clamp_split, floating_surface},
};
use gpui::{
    Anchor, App, AppContext as _, Axis, Bounds, ColorExt as _, Context, DragMoveEvent, Entity,
    FocusHandle, Focusable, IntoElement, KeyBinding, MouseButton, MouseMoveEvent, Pixels, Point,
    Render, SharedString, StyleRefinement, Subscription, Task, Window, actions, anchored, canvas,
    deferred, div,
    inspector::{ElementKind, InspectorDock, InspectorEvent},
    point,
    prelude::*,
    px,
};
use std::{cell::Cell, rc::Rc, time::Duration};

/// Key context of Loupe's root.
pub const LOUPE_CONTEXT: &str = "Loupe";

/// How often Loupe checks the capture for new app data.
pub const REFRESH_INTERVAL: Duration = Duration::from_millis(100);

const DOCK_EDGE: &str = "loupe-dock-edge";
const MIN_DOCK_WIDTH: Pixels = px(360.);
const MIN_DOCK_HEIGHT: Pixels = px(220.);
const MIN_APP_SIZE: Pixels = px(160.);

/// The app keeps at least 160 px, and at least the 15% that
/// `InspectorDock::split` reserves for it.
fn min_app_size(viewport: Pixels) -> Pixels {
    (viewport * 0.15).max(MIN_APP_SIZE)
}

actions!(
    loupe,
    [
        /// Opens (or closes) the command palette.
        OpenPalette,
        /// Freezes or resumes recording.
        ToggleFreeze,
        /// Closes the palette or stops picking.
        Cancel,
        /// Shows the Elements lens.
        ShowElements,
        /// Shows the Frames lens.
        ShowFrames,
        /// Shows the Events lens.
        ShowEvents,
        /// Shows the Entities lens.
        ShowEntities,
        /// Shows the Audit lens.
        ShowAudit,
    ]
);

/// Loupe's root view: one per inspected window, alive while the dock is open.
pub struct Loupe {
    state: Entity<LoupeState>,
    lenses: Lenses,
    focus: FocusHandle,
    palette: Option<(Entity<Palette>, Subscription)>,
    pulse_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pulse_hover: Option<u64>,
    right_width: Pixels,
    bottom_height: Pixels,
    #[cfg(any(test, feature = "test-support"))]
    renders: usize,
    _refresh: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for Loupe {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Loupe {
    pub(crate) fn new(
        inspector: Entity<gpui::Inspector>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = cx.new(|_| LoupeState::new());
        let lenses = Lenses::new(&state, window, cx);
        let (right_width, bottom_height) = match window.inspector_capture().map(|c| c.dock()) {
            Some(InspectorDock::Bottom { height }) => (px(560.), height),
            Some(InspectorDock::Right { width }) => (width, px(320.)),
            None => (px(560.), px(320.)),
        };
        let subscriptions = vec![
            cx.subscribe_in(&inspector, window, Self::on_inspector_event),
            cx.observe_in(&state, window, |this, state, window, cx| {
                this.sync_overlay_selection(&state, window, cx);
                cx.notify();
            }),
            cx.observe_global_in::<LoupeSettings>(window, |this, _, cx| this.restyle(cx)),
            cx.observe_window_appearance(window, |this, _, cx| this.restyle(cx)),
        ];
        Self {
            state,
            lenses,
            focus: cx.focus_handle(),
            palette: None,
            pulse_bounds: Rc::default(),
            pulse_hover: None,
            right_width,
            bottom_height,
            #[cfg(any(test, feature = "test-support"))]
            renders: 0,
            _refresh: Self::spawn_refresh(window, cx),
            _subscriptions: subscriptions,
        }
    }

    /// The state shared with the lenses.
    pub fn state(&self) -> &Entity<LoupeState> {
        &self.state
    }

    /// How many times Loupe's shell has rendered (tests prove it stays idle).
    #[cfg(any(test, feature = "test-support"))]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// Where the pulse strip drew `frame`'s bar (center of its column).
    #[cfg(any(test, feature = "test-support"))]
    pub fn pulse_bar_center(&self, frame: u64, window: &Window) -> Option<Point<Pixels>> {
        let bounds = self.pulse_bounds.get()?;
        capture_bars(window.inspector_capture()?, bounds)
            .into_iter()
            .find(|bar| bar.frame == frame)
            .map(|bar| bar.slot.center())
    }

    /// Whether the palette is open.
    pub fn is_palette_open(&self) -> bool {
        self.palette.is_some()
    }

    /// The palette's current matches, best first (empty when closed).
    pub fn palette_matches(&self, cx: &App) -> Vec<SharedString> {
        self.palette
            .as_ref()
            .map(|(palette, _)| palette.read(cx).match_labels())
            .unwrap_or_default()
    }

    fn spawn_refresh(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                if this
                    .update_in(cx, |this, window, cx| this.poll(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    /// Catches up with the capture: notifies only if the app recorded new data.
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(generation) = window
            .inspector_capture()
            .map(|capture| capture.generation())
        else {
            return;
        };
        self.state
            .update(cx, |state, cx| state.catch_up(generation, cx));
    }

    /// Re-renders everything after the theme changed.
    fn restyle(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |_, cx| cx.notify());
    }

    fn on_inspector_event(
        &mut self,
        _: &Entity<gpui::Inspector>,
        event: &InspectorEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InspectorEvent::Picked(key) => self.state.update(cx, |state, cx| {
                state.select_element(Some(*key), cx);
                state.set_lens(Lens::Elements, cx);
            }),
            InspectorEvent::PickHovered(_) | InspectorEvent::PickCancelled => cx.notify(),
        }
    }

    fn sync_overlay_selection(
        &mut self,
        state: &Entity<LoupeState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = state.read(cx).selected_element();
        if let Some(capture) = window.inspector_capture_mut()
            && capture.overlay().selected != selected
        {
            capture.overlay_mut().selected = selected;
        }
    }

    /// Runs a command. Every toolbar control, key binding and palette
    /// command ends up here.
    pub fn run(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::TogglePick => toggle_pick(window),
            Command::ToggleOverlay(mode) => {
                if let Some(capture) = window.inspector_capture_mut() {
                    capture.overlay_mut().modes.toggle(mode);
                }
            }
            Command::ToggleFreeze => {
                if let Some(capture) = window.inspector_capture_mut() {
                    let frozen = capture.is_frozen();
                    capture.set_frozen(!frozen);
                }
            }
            Command::Dock(side) => self.set_dock(side, window),
            Command::ToggleDock => {
                if let Some(capture) = window.inspector_capture() {
                    let side = match DockSide::of(capture.dock()) {
                        DockSide::Right => DockSide::Bottom,
                        DockSide::Bottom => DockSide::Right,
                    };
                    self.set_dock(side, window);
                }
            }
            Command::ShowLens(lens) => self.state.update(cx, |state, cx| state.set_lens(lens, cx)),
            Command::ClearRecording => {
                if let Some(capture) = window.inspector_capture_mut() {
                    capture.clear();
                }
                self.state
                    .update(cx, |state, cx| state.select_frame(None, cx));
            }
            Command::SetAppearance(appearance) => {
                cx.default_global::<LoupeSettings>().appearance = appearance;
            }
            Command::SetDensity(density) => {
                cx.default_global::<LoupeSettings>().density = density;
            }
            Command::OpenPalette => self.open_palette("", window, cx),
            Command::Close => {
                window.toggle_inspector(cx);
                return;
            }
        }
        cx.notify();
    }

    fn set_dock(&mut self, side: DockSide, window: &mut Window) {
        let dock = match side {
            DockSide::Right => InspectorDock::Right {
                width: self.right_width,
            },
            DockSide::Bottom => InspectorDock::Bottom {
                height: self.bottom_height,
            },
        };
        if let Some(capture) = window.inspector_capture_mut() {
            capture.set_dock(dock);
        }
    }

    fn resize_dock(
        &mut self,
        event: &DragMoveEvent<SplitDrag>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.drag(cx).id != DOCK_EDGE {
            return;
        }
        let viewport = window.viewport_size();
        let pointer = event.event.position;
        let Some(capture) = window.inspector_capture_mut() else {
            return;
        };
        let dock = match capture.dock() {
            InspectorDock::Right { .. } => {
                let width = clamp_split(
                    viewport.width - pointer.x,
                    viewport.width,
                    MIN_DOCK_WIDTH,
                    min_app_size(viewport.width),
                );
                self.right_width = width;
                InspectorDock::Right { width }
            }
            InspectorDock::Bottom { .. } => {
                let height = clamp_split(
                    viewport.height - pointer.y,
                    viewport.height,
                    MIN_DOCK_HEIGHT,
                    min_app_size(viewport.height),
                );
                self.bottom_height = height;
                InspectorDock::Bottom { height }
            }
        };
        if capture.dock() != dock {
            capture.set_dock(dock);
            cx.notify();
        }
    }

    fn open_palette(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.palette_items(window, cx);
        let palette = cx.new(|cx| Palette::new(items, query, window, cx));
        let subscription = cx.subscribe_in(&palette, window, Self::on_palette_event);
        self.palette = Some((palette, subscription));
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn on_palette_event(
        &mut self,
        _: &Entity<Palette>,
        event: &PaletteEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_palette(window, cx);
        let PaletteEvent::Confirmed(target) = event else {
            return;
        };
        match *target {
            PaletteTarget::Command(command) => self.run(command, window, cx),
            PaletteTarget::Element(key) => self.state.update(cx, |state, cx| {
                state.select_element(Some(key), cx);
                state.set_lens(Lens::Elements, cx);
            }),
            PaletteTarget::Entity(entity) => self.state.update(cx, |state, cx| {
                state.select_entity(Some(entity), cx);
                state.set_lens(Lens::Entities, cx);
            }),
        }
    }

    /// Everything the palette can find: commands, the latest tree's elements
    /// and the entities Loupe knows about.
    fn palette_items(&self, window: &Window, cx: &App) -> Vec<PaletteItem> {
        let Some(capture) = window.inspector_capture() else {
            return Vec::new();
        };
        let state = self.state.read(cx);
        let settings = LoupeSettings::get(cx);
        let mut items: Vec<PaletteItem> = Command::all()
            .into_iter()
            .map(|command| PaletteItem {
                target: PaletteTarget::Command(command),
                label: command.label().into(),
                detail: None,
                glyph: PaletteGlyph::Command,
                keys: command.keys(),
                checked: command.is_on(capture, state, settings),
            })
            .collect();
        if let Some(tree) = capture.latest_tree() {
            items.extend(tree.elements.iter().filter_map(|record| {
                let key = record.key?;
                let glyph = match record.kind {
                    ElementKind::View { .. } => PaletteGlyph::View,
                    ElementKind::Component { .. } => PaletteGlyph::Component,
                    ElementKind::Element { .. } => PaletteGlyph::Element,
                };
                let detail = capture
                    .path_info(key.path)
                    .map(|info| SharedString::from(format::location(info.source)));
                Some(PaletteItem {
                    target: PaletteTarget::Element(key),
                    label: format::element_label(record).into(),
                    detail,
                    glyph,
                    keys: None,
                    checked: None,
                })
            }));
        }
        let live = window.inspector_entities(cx);
        let mut entities: Vec<(gpui::EntityId, &'static str)> =
            entity_names(capture, &live).into_iter().collect();
        for id in capture.notify_stats().keys() {
            if !entities.iter().any(|(known, _)| known == id) {
                entities.push((*id, "Entity"));
            }
        }
        entities.sort_by_key(|(id, _)| *id);
        items.extend(entities.into_iter().map(|(id, name)| PaletteItem {
            target: PaletteTarget::Entity(id),
            label: name.into(),
            detail: Some(entity_label(id).into()),
            glyph: PaletteGlyph::Entity,
            keys: None,
            checked: None,
        }));
        items
    }

    fn hover_pulse(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hovered = match (window.inspector_capture(), self.pulse_bounds.get()) {
            (Some(capture), Some(bounds)) => frame_at(capture, bounds, position),
            _ => None,
        };
        if hovered != self.pulse_hover {
            self.pulse_hover = hovered;
            cx.notify();
        }
    }

    fn click_pulse(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let frame = match (window.inspector_capture(), self.pulse_bounds.get()) {
            (Some(capture), Some(bounds)) => frame_at(capture, bounds, position),
            _ => None,
        };
        if let Some(frame) = frame {
            self.state.update(cx, |state, cx| {
                state.select_frame(Some(frame), cx);
                state.set_lens(Lens::Frames, cx);
            });
        }
    }

    fn runner(&self, cx: &mut Context<Self>) -> Run {
        let this = cx.entity().downgrade();
        Rc::new(move |command, window, cx| {
            this.update(cx, |loupe, cx| loupe.run(command, window, cx))
                .ok();
        })
    }

    fn render_pulse(
        &self,
        theme: &'static Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = &theme.colors;
        let selected = self.state.read(cx).selected_frame();
        let hovered = self.pulse_hover;
        let bounds_cell = self.pulse_bounds.clone();
        let stats = window
            .inspector_capture()
            .map(|capture| frame_stats(capture.frames(), capture.config().budget))
            .filter(|stats| stats.app_frames > 0);
        let fps = stats.as_ref().map(|stats| format!("{:.0}", stats.fps));
        let p95 = stats
            .as_ref()
            .map(|stats| format::millis(stats.app_total.p95));
        let stat = |value: String, unit: &'static str| {
            div()
                .flex()
                .items_baseline()
                .gap(px(3.))
                .child(
                    div()
                        .font_family(MONO_FONT)
                        .text_color(colors.text)
                        .child(value),
                )
                .child(div().text_color(colors.text_faint).child(unit))
        };
        div()
            .flex_none()
            .h(theme.metrics.pulse)
            .w_full()
            .flex()
            .items_center()
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.line)
            .child(
                div()
                    .id("loupe-pulse")
                    .debug_selector(|| "loupe-pulse".into())
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pl(theme.metrics.gutter)
                    .cursor_pointer()
                    .child(
                        canvas(
                            move |bounds, _, _| bounds_cell.set(Some(bounds)),
                            move |bounds, _, window, _| {
                                paint_pulse(bounds, selected, hovered, theme, window)
                            },
                        )
                        .size_full(),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                        this.hover_pulse(event.position, window, cx)
                    }))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if !*hovered && this.pulse_hover.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                            this.click_pulse(event.position, window, cx)
                        }),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(theme.metrics.gutter)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(theme.metrics.text_small)
                    .child(stat(fps.unwrap_or_else(|| format::NO_VALUE.into()), "fps"))
                    .child(div().text_color(colors.text_faint).child("·"))
                    .child(div().text_color(colors.text_faint).child("p95"))
                    .child(stat(p95.unwrap_or_else(|| format::NO_VALUE.into()), "ms")),
            )
    }

    /// The floating summary of the hovered pulse bar.
    fn render_pulse_tooltip(
        &self,
        theme: &Theme,
        window: &Window,
    ) -> Option<impl IntoElement + use<>> {
        let frame_id = self.pulse_hover?;
        let bounds = self.pulse_bounds.get()?;
        let capture = window.inspector_capture()?;
        let frame = capture.frame(frame_id)?;
        let bar = capture_bars(capture, bounds)
            .into_iter()
            .find(|bar| bar.frame == frame_id)?;
        let x = bar.slot.center().x;
        let right_half = x > bounds.center().x;
        let position = point(x, bounds.bottom() + px(6.));
        Some(deferred(
            anchored()
                .position(position)
                .anchor(if right_half {
                    Anchor::TopRight
                } else {
                    Anchor::TopLeft
                })
                .child(
                    floating_surface(theme)
                        .px_2()
                        .py_1()
                        .whitespace_nowrap()
                        .text_color(theme.colors.text)
                        .child(pulse::frame_summary(frame)),
                ),
        ))
    }

    fn render_rail(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let lens = self.state.read(cx).lens();
        let state = self.state.clone();
        let tabs: Vec<RailTab> = Lens::ALL
            .iter()
            .map(|&lens| {
                let tab = RailTab::new(lens.label())
                    .tooltip(lens.question(), Some(keys::LENSES[lens.index()].into()));
                match self.lenses.badge(lens, window, cx) {
                    Some(badge) => {
                        let tab = tab.count(badge.text).tone(badge.tone);
                        match badge.marker {
                            Some(marker) => tab.marker(marker),
                            None => tab,
                        }
                    }
                    None => tab,
                }
            })
            .collect();
        tabs.into_iter()
            .fold(TabRail::new("loupe-rail"), TabRail::tab)
            .selected(lens.index())
            .on_select(move |ix, _, cx| {
                state.update(cx, |state, cx| state.set_lens(Lens::ALL[*ix], cx));
            })
    }

    fn render_status(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let selected = self.state.read(cx).selected_element();
        let state = self.state.clone();
        let on_crumb = Rc::new(move |key, _: &mut Window, cx: &mut App| {
            state.update(cx, |state, cx| state.select_element(Some(key), cx));
        });
        let capture = window.inspector_capture();
        let (crumbs, elided) = match (capture.and_then(|c| c.latest_tree()), selected) {
            (Some(tree), Some(key)) => breadcrumb(tree, key),
            _ => (Vec::new(), false),
        };
        let selected_frame = self.state.read(cx).selected_frame();
        let frame = capture.and_then(|capture| match selected_frame {
            Some(id) => capture.frame(id),
            None => capture.latest_app_frame(),
        });
        let loupe_time = capture
            .and_then(|capture| capture.latest_frame())
            .map(|frame| frame.timings.inspector);
        StatusBar {
            crumbs,
            elided,
            app_time: frame.map(|frame| frame.timings.app_total()),
            loupe_time,
            retained: capture.map_or(0, |capture| capture.retained_bytes()),
            frozen: capture.is_some_and(|capture| capture.is_frozen()),
            on_crumb,
        }
    }

    fn render_palette(&self, theme: &Theme) -> Option<impl IntoElement + use<>> {
        let (palette, _) = self.palette.as_ref()?;
        Some(
            // A scrim that swallows clicks: clicking outside only dismisses.
            div()
                .id("loupe-palette-scrim")
                .absolute()
                .inset_0()
                .occlude()
                .bg(theme.colors.shadow.opacity(0.35))
                .flex()
                .justify_center()
                .pt(theme.metrics.toolbar + px(6.))
                .px(px(12.))
                .child(div().w_full().max_w(px(560.)).child(palette.clone())),
        )
    }
}

impl Render for Loupe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-support"))]
        {
            self.renders += 1;
        }
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let Some(capture) = window.inspector_capture() else {
            return div().size_full().bg(colors.bg).into_any_element();
        };
        let dock = capture.dock();
        let toolbar = Toolbar {
            picking: capture.pick().active,
            overlays: capture.overlay().modes,
            frozen: capture.is_frozen(),
            dock: DockSide::of(dock),
            width: window
                .inspector_bounds()
                .map_or(px(0.), |bounds| bounds.size.width),
            run: self.runner(cx),
        };
        let lens = self.state.read(cx).lens();
        let edge_axis = match dock {
            InspectorDock::Right { .. } => Axis::Horizontal,
            InspectorDock::Bottom { .. } => Axis::Vertical,
        };
        let edge = match edge_axis {
            Axis::Horizontal => div().absolute().top_0().bottom_0().left_0(),
            Axis::Vertical => div().absolute().left_0().right_0().top_0(),
        }
        .child(SplitHandle::new(DOCK_EDGE, edge_axis));

        div()
            .id("loupe")
            .key_context(LOUPE_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors.bg)
            .font_family(UI_FONT)
            .text_size(theme.metrics.text)
            .line_height(theme.metrics.line_height)
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                if this.palette.is_some() {
                    this.close_palette(window, cx);
                } else {
                    this.open_palette("", window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleFreeze, window, cx| {
                this.run(Command::ToggleFreeze, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Cancel, window, cx| {
                if this.palette.is_some() {
                    this.close_palette(window, cx);
                } else if window.inspector_capture().is_some_and(|c| c.pick().active) {
                    this.run(Command::TogglePick, window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &ShowElements, window, cx| {
                this.run(Command::ShowLens(Lens::Elements), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowFrames, window, cx| {
                this.run(Command::ShowLens(Lens::Frames), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowEvents, window, cx| {
                this.run(Command::ShowLens(Lens::Events), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowEntities, window, cx| {
                this.run(Command::ShowLens(Lens::Entities), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowAudit, window, cx| {
                this.run(Command::ShowLens(Lens::Audit), window, cx)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .on_drag_move(cx.listener(Self::resize_dock))
            .child(toolbar)
            .child(self.render_pulse(theme, window, cx))
            .child(self.render_rail(window, cx))
            .child(
                div().flex_1().min_h_0().child(
                    self.lenses
                        .view(lens)
                        .cached(StyleRefinement::default().size_full()),
                ),
            )
            .child(self.render_status(window, cx))
            .child(edge)
            .children(self.render_palette(theme))
            .children(self.render_pulse_tooltip(theme, window))
            .into_any_element()
    }
}

/// Binds Loupe's keys (see `DESIGN.md`, "Keys").
pub(crate) fn bind_keys(cx: &mut App) {
    let loupe = Some(LOUPE_CONTEXT);
    let [elements, frames, events, entities, audit] = keys::LENSES;
    cx.bind_keys([
        KeyBinding::new("secondary-k", OpenPalette, loupe),
        KeyBinding::new(elements, ShowElements, loupe),
        KeyBinding::new(frames, ShowFrames, loupe),
        KeyBinding::new(events, ShowEvents, loupe),
        KeyBinding::new(entities, ShowEntities, loupe),
        KeyBinding::new(audit, ShowAudit, loupe),
        KeyBinding::new("escape", Cancel, loupe),
        KeyBinding::new(keys::FREEZE, ToggleFreeze, Some("Loupe && !EditableText")),
    ]);
    widgets::bind_keys(LOUPE_CONTEXT, cx);
    crate::lenses::bind_keys(cx);
}

pub(crate) fn toggle_pick(window: &mut Window) {
    let picking = window
        .inspector_capture()
        .is_some_and(|capture| capture.pick().active);
    if picking {
        window.stop_inspector_pick();
    } else {
        window.start_inspector_pick();
    }
}
