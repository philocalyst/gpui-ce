//! Replaying the app on frames the inspector draws for itself.
//!
//! The inspector redraws its dock often (its own refresh, the picker's hover,
//! fading paint flashes) while the app has nothing new to show. Rendering the
//! app on those frames would cost time and skew what the inspector reports
//! about it, so the window replays the app's layers from the previous frame
//! instead, the way a cached view replays its subtree: the same scene,
//! hitboxes, dispatch tree, listeners, deferred draws and tooltip, and no
//! view renders. The capture records such a frame without a tree and with
//! every app view [`ViewOutcome::Cached`](crate::inspector::ViewOutcome).
//!
//! A frame replays the app when the previous frame drew it, nothing changed
//! its geometry (bounds, viewport, scale, rem size, root view), accessibility
//! and drags are inactive, the last render of the app recorded all the
//! capture now asks for (a tree for a new overlay, details at a higher
//! level), and either the frame is the inspector's alone (no app cause, no
//! app view or entity dirty, no refresh) or the app is held
//! ([`InspectorCapture::set_holding`](crate::inspector::InspectorCapture::set_holding)).
//! While held, the app's invalidations are kept, and applied on release or
//! as soon as the app must render anyway (after a resize, for instance).

use super::*;

/// What the window keeps to replay the app, while the inspector is open.
#[derive(Default)]
pub(super) struct AppReplay {
    /// The app as the latest frame drew it.
    latest: Option<Box<DrawnApp>>,
    /// The app as the frame before the one being drawn drew it: the only
    /// layers this frame can replay.
    previous: Option<Box<DrawnApp>>,
    /// Whether the frame being drawn replays the app.
    replaying: bool,
    /// Views invalidated while the app was held, applied on release.
    held_views: FxHashSet<EntityId>,
    /// Whether the window was refreshed while the app was held.
    held_refresh: bool,
}

/// Where a frame drew the app, and what the app read doing so.
struct DrawnApp {
    layers: AppLayers,
    geometry: AppGeometry,
    /// Entities the app read; replaying keeps the window observing them.
    accessed: FxHashSet<EntityId>,
}

/// Everything outside the app that decides its layout.
#[derive(Clone, Copy, PartialEq)]
struct AppGeometry {
    root: Option<EntityId>,
    bounds: Bounds<Pixels>,
    viewport: Size<Pixels>,
    scale_factor: f32,
    rem_size: Pixels,
}

impl AppGeometry {
    fn of(window: &Window) -> Self {
        AppGeometry {
            root: window.root.as_ref().map(AnyView::entity_id),
            bounds: window.app_bounds(),
            viewport: window.viewport_size,
            scale_factor: window.scale_factor,
            rem_size: window.rem_size(),
        }
    }
}

impl Window {
    /// Starts a frame: only the layers of the frame just before it may be
    /// replayed.
    pub(super) fn begin_app_replay(&mut self) {
        let replay = &mut self.app_replay;
        replay.replaying = false;
        replay.previous = replay.latest.take();
    }

    /// Forgets the app's layers and ends any hold, when the inspector opens or
    /// closes. Invalidations kept by the hold apply on the next frame.
    pub(super) fn reset_app_replay(&mut self) {
        let replay = mem::take(&mut self.app_replay);
        self.dirty_views.extend(replay.held_views);
        self.refreshing |= replay.held_refresh;
    }

    /// Decides whether this frame replays the app and returns the layers to
    /// replay. Applies the invalidations kept during a hold once it is over.
    pub(super) fn take_app_replay(&mut self, cx: &App) -> Option<AppLayers> {
        let geometry = AppGeometry::of(self);
        let capture = self.inspector_capture.as_deref_mut()?;
        let released = mem::take(&mut capture.recorder.release_pending);
        capture.recorder.app_released = released;
        let holding = capture.holding && !released;
        let replay = &mut self.app_replay;
        let previous = replay
            .previous
            .as_deref()
            .filter(|previous| previous.geometry == geometry)
            .filter(|_| !released && !self.a11y.is_active() && cx.active_drag.is_none())
            .filter(|_| capture.recorder.can_replay_app());
        let app_untouched = |previous: &DrawnApp| {
            capture.recorder.inspector_only
                && !self.refreshing
                && !self.dirty_views.iter().any(|entity| {
                    geometry.root == Some(*entity) || previous.accessed.contains(entity)
                })
        };
        let Some(previous) = previous.filter(|previous| holding || app_untouched(previous)) else {
            // The app renders, with everything a hold kept from it.
            self.dirty_views.extend(replay.held_views.drain());
            self.refreshing |= mem::take(&mut replay.held_refresh);
            return None;
        };
        if holding {
            replay.held_views.extend(self.dirty_views.iter().copied());
            replay.held_refresh |= self.refreshing;
        }
        let layers = previous.layers.clone();
        replay.replaying = true;
        capture.recorder.replay_app();
        Some(layers)
    }

    /// Whether reusing a range of the previous frame also moves the ranges
    /// that the cached views inside it recorded (see
    /// [`Self::rebase_nested_views`]): while the inspector is open, since a
    /// replayed app reads them.
    pub(super) fn keeps_nested_views_in_step(&self) -> bool {
        self.inspector_capture.is_some()
    }

    /// Makes the cached views drawn inside a range that this frame reuses
    /// from the previous one follow what they drew to where it now sits.
    /// `accessed` is the range's span of the previous frame's element states.
    ///
    /// A cached view inside a reused one is copied with it, but keeps the
    /// ranges it recorded when it last drew. That is harmless on its own: it
    /// renders again whenever the view around it does, so those ranges are
    /// never read. A replay, though, copies the app's layers wholesale, and
    /// a cached view directly inside the app's (uncached) root is reused on
    /// the next frame that renders the root, from the ranges it recorded.
    /// Those moved: what the replayed frame drew before the copy (in the
    /// paint phase, everything prepainted, the inspector included) differs
    /// from the frame that recorded them. So while the inspector is open,
    /// every reuse keeps the ranges of the views it copies in step, and they
    /// are right whenever a replay copies them again.
    pub(super) fn rebase_nested_views(
        &mut self,
        accessed: Range<usize>,
        rebase: impl Fn(&mut crate::view::ViewElementState),
    ) {
        let view_state = TypeId::of::<crate::view::ViewElementState>();
        let frame = &mut self.rendered_frame;
        for key in &frame.accessed_element_states[accessed] {
            if key.1 != view_state {
                continue;
            }
            let state = frame
                .element_states
                .get_mut(key)
                .and_then(|state| {
                    state
                        .inner
                        .downcast_mut::<Option<crate::view::ViewElementState>>()
                })
                .and_then(Option::as_mut);
            if let Some(state) = state {
                rebase(state);
            }
        }
    }

    /// Keeps where this frame drew (or replayed) the app, for the next one.
    pub(super) fn store_app_layers(&mut self, layers: AppLayers, replayed: bool) {
        if self.inspector_capture.is_none() {
            return;
        }
        let geometry = AppGeometry::of(self);
        let replay = &mut self.app_replay;
        let accessed = replay
            .previous
            .take()
            .filter(|_| replayed)
            .map(|previous| previous.accessed)
            .unwrap_or_default();
        replay.latest = Some(Box::new(DrawnApp {
            layers,
            geometry,
            accessed,
        }));
    }

    /// Settles what the app read this frame, before the window registers it
    /// for invalidation: a replayed app keeps reading what it read when it
    /// was drawn. `accessed` holds the app's reads alone (the inspector's are
    /// kept apart until after this).
    pub(super) fn settle_app_access(&mut self, accessed: &mut FxHashSet<EntityId>) {
        let replay = &mut self.app_replay;
        if let Some(latest) = replay.latest.as_deref_mut() {
            if replay.replaying {
                accessed.extend(latest.accessed.iter().copied());
            } else {
                latest.accessed.clone_from(accessed);
            }
        }
    }
}
