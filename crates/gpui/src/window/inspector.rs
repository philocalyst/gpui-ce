//! The inspector's public surface on [`Window`]: opening it, reading and
//! steering its capture, and drawing its root in the dock.

use super::*;

impl Window {
    /// Toggles the inspector mode on this window.
    pub fn toggle_inspector(&mut self, cx: &mut App) {
        if self.inspector.take().is_some() {
            self.close_inspector_capture();
        } else {
            self.inspector = Some(cx.new(|_| Inspector::new()));
            self.open_inspector_capture(cx);
        }
        self.refresh_for(RefreshReason::Inspector);
    }

    /// The inspector's recording of this window, while the inspector is open.
    pub fn inspector_capture(&self) -> Option<&crate::inspector::InspectorCapture> {
        self.inspector_capture.as_deref()
    }

    /// Mutable access to the inspector's recording: overlays, picking, dock,
    /// overrides, freezing and configuration. Changes show on the next frame.
    pub fn inspector_capture_mut(&mut self) -> Option<&mut crate::inspector::InspectorCapture> {
        self.inspector_capture.as_deref_mut()
    }

    /// Replaces the open inspector's recording with a fabricated one (keeping
    /// the current dock), so inspector UI can be rendered against fixtures.
    /// Does nothing while the inspector is closed.
    /// The installed capture replays: live frames and input are not added to
    /// it, while overlays, picking and overrides keep working against it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn replace_inspector_capture_for_test(
        &mut self,
        mut capture: crate::inspector::InspectorCapture,
    ) {
        if let Some(current) = self.inspector_capture.as_deref_mut() {
            capture.set_dock(current.dock());
            capture.replaying = true;
            *current = capture;
            self.refresh_for(RefreshReason::Inspector);
        }
    }

    /// Holds or releases the app (see
    /// [`crate::inspector::InspectorCapture::set_holding`]): while held, the
    /// app stays exactly as it is on screen, so a hover menu or a tooltip
    /// can be inspected. Releasing draws a frame right away with everything
    /// the app deferred. Emits [`crate::inspector::InspectorEvent::HoldChanged`].
    pub fn set_inspector_holding(&mut self, holding: bool, cx: &mut App) {
        let Some(capture) = self.inspector_capture.as_deref_mut() else {
            return;
        };
        if capture.is_holding() == holding {
            return;
        }
        capture.set_holding(holding);
        if !holding {
            self.request_inspector_frame(crate::inspector::CauseKind::Refresh);
        }
        self.emit_inspector_event(crate::inspector::InspectorEvent::HoldChanged(holding), cx);
    }

    /// Where the inspector UI is drawn, while it is open.
    pub fn inspector_bounds(&self) -> Option<Bounds<Pixels>> {
        let capture = self.inspector_capture.as_ref()?;
        let dock = capture.dock();
        (dock != crate::inspector::InspectorDock::Hidden).then(|| dock.split(self.viewport_size).1)
    }

    /// Resolves keystrokes against the keymap and the focused context stack
    /// without dispatching anything: which binding wins, and why each other
    /// candidate loses.
    ///
    /// `keystrokes` is the whole sequence, as if typed from scratch with the
    /// current focus. The result matches what key dispatch does with the
    /// latest rendered frame: the winner is the highest-precedence matching
    /// binding whose action is handled on the focus path or by a global
    /// listener (bindings above it resolve to
    /// [`crate::inspector::BindingVerdict::Unhandled`]). It assumes action
    /// handlers don't call `cx.propagate()`, keystroke interceptors
    /// (`cx.intercept_keystrokes`) don't stop the keystroke, and the platform
    /// didn't prefer character input for it (e.g. AltGr on some layouts).
    pub fn inspector_resolve_keystrokes(
        &self,
        keystrokes: &[Keystroke],
        cx: &App,
    ) -> crate::inspector::KeyResolution {
        self.resolve_keystrokes_at(keystrokes, self.focus, cx)
    }

    /// Like [`Self::inspector_resolve_keystrokes`], but as if `focus` were
    /// focused (`None`: nothing is, so the window root receives the keys).
    /// The inspector's key tester holds the window's focus while it listens,
    /// so it resolves against the app element that had focus before.
    pub fn inspector_resolve_keystrokes_for(
        &self,
        keystrokes: &[Keystroke],
        focus: Option<&FocusHandle>,
        cx: &App,
    ) -> crate::inspector::KeyResolution {
        self.resolve_keystrokes_at(keystrokes, focus.map(|focus| focus.id), cx)
    }

    fn resolve_keystrokes_at(
        &self,
        keystrokes: &[Keystroke],
        focus: Option<FocusId>,
        cx: &App,
    ) -> crate::inspector::KeyResolution {
        let node_id = self.focus_node_id_in_rendered_frame(focus);
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        let context_stack = dispatch_tree
            .dispatch_path(node_id)
            .iter()
            .filter_map(|&node_id| dispatch_tree.node(node_id).context.clone())
            .collect();
        let keymap = cx.keymap.borrow();
        crate::inspector::resolve_keystrokes(&keymap, keystrokes, context_stack, |action| {
            dispatch_tree.is_action_available(action, node_id)
                || cx
                    .global_action_listeners
                    .get(&action.as_any().type_id())
                    .is_some_and(|listeners| !listeners.is_empty())
        })
    }

    /// Every live entity with its type, handle count, observers and notify
    /// counts, sorted by id. Like [`App::inspector_entities`], but also sees
    /// this window while it is being updated (for example during render), and
    /// reports a test's fixture entities while its capture is installed.
    pub fn inspector_entities(&self, cx: &App) -> Vec<crate::inspector::EntityInfo> {
        if let Some(capture) = self.inspector_capture.as_deref()
            && capture.replaying
        {
            return capture.replay_entities.clone();
        }
        let other_windows = cx
            .windows
            .values()
            .filter_map(Option::as_deref)
            .filter(|window| window.handle.window_id() != self.handle.window_id());
        crate::inspector::live_entities(cx, std::iter::once(self).chain(other_windows))
    }

    pub(crate) fn build_inspector_element_id(
        &mut self,
        path: crate::InspectorElementPath,
    ) -> crate::InspectorElementId {
        self.invalidator.debug_assert_paint_or_prepaint();
        let path = Rc::new(path);
        let next_instance_id = self
            .next_frame
            .next_inspector_instance_ids
            .entry(path.clone())
            .or_insert(0);
        let instance_id = *next_instance_id;
        *next_instance_id += 1;
        crate::InspectorElementId { path, instance_id }
    }

    /// Lays out the inspector's own root in the dock. Nothing it draws is
    /// recorded; its time goes to [`crate::inspector::PhaseTimings::inspector`].
    pub(super) fn prepaint_inspector(&mut self, cx: &mut App) -> Option<AnyElement> {
        let bounds = self.inspector_bounds()?;
        // Taken while drawing, so the inspector's own elements are never inspected.
        let inspector = self.inspector.take()?;
        let phase = self.inspector_suspend(cx);
        let hitboxes_start = self.next_frame.hitboxes.len();
        let refreshing = self.suspend_app_refresh();
        let mut inspector_element = AnyView::from(inspector.clone()).into_any_element();
        inspector_element.prepaint_as_root(bounds.origin, bounds.size.into(), self, cx);
        self.refreshing = refreshing;
        if let Some(capture) = self.inspector_capture.as_deref_mut() {
            capture.recorder.inspector_hitboxes = hitboxes_start..self.next_frame.hitboxes.len();
        }
        self.inspector_resume(phase, cx);
        self.inspector = Some(inspector);
        Some(inspector_element)
    }

    pub(super) fn paint_inspector(&mut self, inspector_element: Option<AnyElement>, cx: &mut App) {
        if let Some(mut inspector_element) = inspector_element {
            let inspector = self.inspector.take();
            let phase = self.inspector_suspend(cx);
            let refreshing = self.suspend_app_refresh();
            inspector_element.paint(self, cx);
            self.refreshing = refreshing;
            self.inspector_resume(phase, cx);
            self.inspector = inspector;
        }
    }

    /// Re-renders every view in the window, the inspector's included (an
    /// ordinary [`Window::refresh`] re-renders the app alone).
    #[cfg(any(test, feature = "test-support"))]
    pub fn refresh_with_inspector(&mut self) {
        self.refresh_for(RefreshReason::Inspector);
    }

    /// Hides an app-caused refresh from the inspector's cached views while
    /// they draw, returning the flag to restore afterwards.
    fn suspend_app_refresh(&mut self) -> bool {
        let refreshing = self.refreshing;
        self.refreshing = refreshing && self.refresh_reaches_inspector;
        refreshing
    }
}
