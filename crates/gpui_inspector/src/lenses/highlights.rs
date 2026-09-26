//! Highlights a lens paints on the app through the capture's overlay.
//!
//! Several lenses point at elements in the app (a hovered flame bar, a
//! selected audit finding). The overlay keeps each owner's highlights apart,
//! so a lens sets and clears its own without touching another's.
//! [`LensHighlights`] holds a lens' pinned set (a selection) and a transient
//! hover set that wins while it lasts, and shows them only while the lens is
//! on screen.

use crate::state::Lens;
use gpui::{
    EntityId, Hsla, SharedString, Window,
    inspector::{ElementKey, ElementTree, OverlayHighlight},
};

/// One lens' highlights on the app.
pub(crate) struct LensHighlights {
    lens: Lens,
    /// Whose highlights they are in the overlay: the lens' entity.
    owner: EntityId,
    shown: bool,
    pinned: Vec<OverlayHighlight>,
    hover: Option<Vec<OverlayHighlight>>,
}

impl LensHighlights {
    /// Highlights of `lens`, set in the overlay as `owner`'s (the lens'
    /// entity), hidden until [`Self::sync`] shows them.
    pub fn new(lens: Lens, owner: EntityId) -> Self {
        Self {
            lens,
            owner,
            shown: false,
            pinned: Vec::new(),
            hover: None,
        }
    }

    /// Replaces the pinned highlights. Returns whether the overlay changed
    /// (the caller then notifies, so the window repaints it).
    pub fn pin(&mut self, highlights: Vec<OverlayHighlight>, window: &mut Window) -> bool {
        self.pinned = highlights;
        self.apply(window)
    }

    /// Shows `highlights` instead of the pinned ones until called with `None`.
    pub fn hover(
        &mut self,
        highlights: Option<Vec<OverlayHighlight>>,
        window: &mut Window,
    ) -> bool {
        self.hover = highlights;
        self.apply(window)
    }

    /// The pinned highlights.
    #[cfg(test)]
    pub fn pinned(&self) -> &[OverlayHighlight] {
        &self.pinned
    }

    /// Shows or withdraws the highlights as `active` becomes (or stops
    /// being) this lens. Returns whether the overlay changed.
    pub fn sync(&mut self, active: Lens, window: &mut Window) -> bool {
        let shown = active == self.lens;
        if shown == self.shown {
            return false;
        }
        self.shown = shown;
        if !shown {
            self.hover = None;
        }
        self.apply(window)
    }

    fn apply(&mut self, window: &mut Window) -> bool {
        let Some(capture) = window.inspector_capture_mut() else {
            return false;
        };
        let shown = match (self.shown, &self.hover) {
            (false, _) => Vec::new(),
            (true, Some(hover)) => hover.clone(),
            (true, None) => self.pinned.clone(),
        };
        capture.overlay_mut().set_highlights(self.owner, shown)
    }
}

/// A highlight over the element `key` in `tree`, if it is there.
pub(crate) fn element_highlight(
    tree: &ElementTree,
    key: ElementKey,
    color: Hsla,
    label: Option<SharedString>,
) -> Option<OverlayHighlight> {
    let record = tree.get(tree.find(key)?)?;
    Some(OverlayHighlight {
        bounds: record.bounds,
        color,
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext, div, point, px, size};

    struct Empty;

    impl gpui::Render for Empty {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            div()
        }
    }

    fn bounds_of<'a>(
        highlights: impl IntoIterator<Item = &'a OverlayHighlight>,
    ) -> Vec<Bounds<Pixels>> {
        highlights
            .into_iter()
            .map(|highlight| highlight.bounds)
            .collect()
    }

    fn highlight(x: f32) -> OverlayHighlight {
        OverlayHighlight {
            bounds: Bounds::new(point(px(x), px(0.)), size(px(10.), px(10.))),
            color: Hsla::default(),
            label: None,
        }
    }

    fn overlay(cx: &mut VisualTestContext) -> Vec<Bounds<Pixels>> {
        cx.update(|window, _| bounds_of(window.inspector_capture().unwrap().overlay().highlights()))
    }

    #[gpui::test]
    fn only_the_shown_lens_paints_and_hiding_leaves_other_lenses_alone(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Empty);
        cx.update(|window, cx| {
            window.toggle_inspector(cx);
            window.replace_inspector_capture_for_test(fixtures::steady_frames(1, 4.));
        });
        let mut frames = LensHighlights::new(Lens::Frames, EntityId::from(1u64));
        let mut audit = LensHighlights::new(Lens::Audit, EntityId::from(2u64));

        cx.update(|window, _| {
            assert!(
                !frames.pin(vec![highlight(1.)], window),
                "hidden: nothing painted"
            );
            assert!(frames.sync(Lens::Frames, window));
        });
        assert_eq!(overlay(cx), bounds_of(&[highlight(1.)]));

        cx.update(|window, _| {
            assert!(frames.hover(Some(vec![highlight(2.)]), window));
        });
        assert_eq!(overlay(cx), bounds_of(&[highlight(2.)]), "hover wins");
        cx.update(|window, _| {
            frames.hover(None, window);
        });
        assert_eq!(
            overlay(cx),
            bounds_of(&[highlight(1.)]),
            "then the pin returns"
        );

        // Audit shows first, then Frames hides: Audit's highlights survive.
        cx.update(|window, _| {
            audit.pin(vec![highlight(3.)], window);
            audit.sync(Lens::Audit, window);
            frames.sync(Lens::Audit, window);
        });
        assert_eq!(overlay(cx), bounds_of(&[highlight(3.)]));

        // Frames can't touch Audit's highlights while hidden, whatever it pins.
        cx.update(|window, _| {
            assert!(!frames.pin(vec![highlight(4.)], window));
            assert!(!frames.hover(None, window));
        });
        assert_eq!(overlay(cx), bounds_of(&[highlight(3.)]));

        // Hiding the owner withdraws its highlights.
        cx.update(|window, _| {
            assert!(audit.sync(Lens::Elements, window));
        });
        assert!(overlay(cx).is_empty());
        assert_eq!(audit.pinned().len(), 1, "kept for when it is shown again");
    }
}
