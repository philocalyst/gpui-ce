//! Paint flashing: which views just rendered, how often they render, and
//! how the overlay shows it.
//!
//! Every view that rendered in a frame gets an outline that fades out over
//! [`FLASH_DURATION`], colored by how often the view rendered in the last
//! second ([`heat_color`]): a one-off render blinks teal, a view rendering
//! every frame glows red. Only the views no other flash encloses also get
//! a faint fill, so nested views read as outlines inside outlines and the
//! app stays legible. The hottest views are labeled with their rate.

use super::model::{FrameRecord, HOT_RENDERS_PER_SECOND, ViewOutcome};
use crate::{Bounds, EntityId, Hsla, Pixels, hsla};
use collections::FxHashMap;
use scheduler::Instant;
use std::{collections::VecDeque, time::Duration};

/// How long a paint flash takes to fade out.
pub(crate) const FLASH_DURATION: Duration = Duration::from_millis(300);
/// Opacity of a flash's fill when it starts.
pub(crate) const FLASH_FILL_ALPHA: f32 = 0.12;
/// Rates are counted over this window, ending now.
const RATE_WINDOW: Duration = Duration::from_secs(1);
/// At most this many hot views are labeled at once, hottest first.
pub(crate) const MAX_FLASH_LABELS: usize = 3;

/// A view that rendered recently; the overlay flashes its bounds.
#[derive(Clone, Debug)]
pub(crate) struct Flash {
    /// The view.
    pub(crate) entity: EntityId,
    /// `type_name::<V>()`, for a hot view's label.
    pub(crate) type_name: &'static str,
    /// Where the view was drawn.
    pub(crate) bounds: Bounds<Pixels>,
    /// Depth of the view's record in the element tree, to pick the outer
    /// one of two views drawn at the same bounds.
    pub(crate) depth: u16,
    /// How many times the view rendered in the last second, this render
    /// included.
    pub(crate) rate: u32,
    /// When the view rendered, on the capture's clock.
    pub(crate) started: Instant,
}

impl Flash {
    /// How far the flash has faded at `now`: 1 when it starts, 0 once over.
    pub(crate) fn strength(&self, now: Instant) -> f32 {
        let progress = now.saturating_duration_since(self.started).as_secs_f32()
            / FLASH_DURATION.as_secs_f32();
        (1. - progress).clamp(0., 1.)
    }

    /// `IssueList ×32/s`.
    pub(crate) fn label(&self) -> String {
        format!(
            "{} ×{}/s",
            super::short_type_name(self.type_name),
            self.rate
        )
    }
}

/// Keeps the flashes still fading at `now` and adds `fresh` ones, which
/// restart the flash of a view that was already flashing.
pub(crate) fn update(flashes: &mut Vec<Flash>, now: Instant, fresh: Vec<Flash>) {
    flashes.retain(|flash| {
        now.saturating_duration_since(flash.started) < FLASH_DURATION
            && fresh.iter().all(|new| new.entity != flash.entity)
    });
    flashes.extend(fresh);
}

/// How many times each view rendered in the app frames that started at or
/// after `now` minus a second (`now` being an offset on the capture's clock).
pub(crate) fn render_counts(
    frames: &VecDeque<FrameRecord>,
    now: Duration,
) -> FxHashMap<EntityId, u32> {
    let since = now.saturating_sub(RATE_WINDOW);
    let mut counts = FxHashMap::default();
    let recent = frames
        .iter()
        .rev()
        .take_while(|frame| frame.start >= since)
        .filter(|frame| !frame.inspector_only);
    for frame in recent {
        for view in &frame.views {
            if view.outcome == ViewOutcome::Rendered {
                *counts.entry(view.entity).or_default() += 1;
            }
        }
    }
    counts
}

/// The flash color of a view rendering `rate` times per second: teal for a
/// one-off render, through green and amber, to red for a render hot spot
/// ([`HOT_RENDERS_PER_SECOND`] and up). The rate climbs on a log scale, so
/// a few renders a second already read warm.
pub(crate) fn heat_color(rate: u32) -> Hsla {
    const TEAL: f32 = 0.5;
    const AMBER: f32 = 0.11;
    const RED: f32 = 0.;
    let hot = HOT_RENDERS_PER_SECOND as f32;
    let heat = ((rate.max(1) as f32).ln() / hot.ln()).clamp(0., 1.);
    let hue = if heat < 0.5 {
        TEAL + (AMBER - TEAL) * heat * 2.
    } else {
        AMBER + (RED - AMBER) * (heat - 0.5) * 2.
    };
    hsla(hue, 0.9, 0.5, 1.)
}

/// Which flashes get the faint fill: the ones no other flash encloses, so
/// fills never stack where views nest. Of flashes at the same bounds, the
/// shallowest (then the first) one is filled.
pub(crate) fn filled(flashes: &[Flash]) -> Vec<bool> {
    let encloses = |outer: &Flash, inner: &Flash| {
        outer.bounds.left() <= inner.bounds.left()
            && outer.bounds.top() <= inner.bounds.top()
            && outer.bounds.right() >= inner.bounds.right()
            && outer.bounds.bottom() >= inner.bounds.bottom()
    };
    flashes
        .iter()
        .enumerate()
        .map(|(ix, flash)| {
            !flashes.iter().enumerate().any(|(other_ix, other)| {
                other_ix != ix
                    && encloses(other, flash)
                    && (!encloses(flash, other) || (other.depth, other_ix) < (flash.depth, ix))
            })
        })
        .collect()
}

/// The flashes to label: render hot spots, hottest first, at most
/// [`MAX_FLASH_LABELS`].
pub(crate) fn hot(flashes: &[Flash]) -> Vec<usize> {
    let mut hot: Vec<usize> = (0..flashes.len())
        .filter(|&ix| flashes[ix].rate >= HOT_RENDERS_PER_SECOND)
        .collect();
    hot.sort_by_key(|&ix| std::cmp::Reverse(flashes[ix].rate));
    hot.truncate(MAX_FLASH_LABELS);
    hot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{point, px, size};

    fn flash(entity: u64, bounds: (f32, f32, f32, f32), depth: u16, rate: u32) -> Flash {
        let (x, y, width, height) = bounds;
        Flash {
            entity: EntityId::from(entity),
            type_name: "app::View",
            bounds: Bounds::new(point(px(x), px(y)), size(px(width), px(height))),
            depth,
            rate,
            started: Instant::now(),
        }
    }

    #[test]
    fn heat_goes_from_teal_through_amber_to_red() {
        let hue = |rate| heat_color(rate).color.hue.into_positive_degrees();
        assert!((hue(1) - 180.).abs() < 0.5, "one render: teal");
        assert!((hue(0) - 180.).abs() < 0.5, "never less than one");
        let amber = hue(30f32.sqrt().round() as u32);
        assert!(
            (30. ..=60.).contains(&amber),
            "a few a second: amber ({amber})"
        );
        assert!(hue(HOT_RENDERS_PER_SECOND) < 0.5, "a hot spot: red");
        assert!(hue(240) < 0.5, "hotter stays red");
        let hues: Vec<f32> = (1..=HOT_RENDERS_PER_SECOND).map(hue).collect();
        assert!(
            hues.windows(2).all(|pair| pair[1] <= pair[0]),
            "warmer the more it renders"
        );
    }

    #[test]
    fn only_flashes_nothing_encloses_are_filled() {
        // A root view with a nested view inside it, and a sibling next to it.
        let flashes = [
            flash(1, (0., 0., 800., 600.), 0, 1),
            flash(2, (10., 10., 200., 100.), 3, 1),
            flash(3, (20., 20., 50., 50.), 5, 1),
        ];
        assert_eq!(filled(&flashes), [true, false, false]);
        assert_eq!(filled(&flashes[1..]), [true, false]);

        // Side by side: both filled. The same bounds: the shallower one.
        let apart = [
            flash(1, (0., 0., 100., 100.), 2, 1),
            flash(2, (200., 0., 100., 100.), 2, 1),
        ];
        assert_eq!(filled(&apart), [true, true]);
        let same = [
            flash(1, (0., 0., 100., 100.), 4, 1),
            flash(2, (0., 0., 100., 100.), 2, 1),
            flash(3, (0., 0., 100., 100.), 2, 1),
        ];
        assert_eq!(filled(&same), [false, true, false]);
    }

    #[test]
    fn a_view_flashing_again_restarts_its_flash_and_old_ones_fade() {
        let start = Instant::now();
        let halfway = start + FLASH_DURATION / 2;
        let rendered = |entity, rate, started| Flash {
            started,
            ..flash(entity, (0., 0., 10., 10.), 0, rate)
        };
        let mut flashes = Vec::new();
        update(&mut flashes, start, vec![rendered(1, 1, start)]);
        update(&mut flashes, start, vec![rendered(2, 1, start)]);
        update(&mut flashes, halfway, vec![rendered(1, 2, halfway)]);
        let rates: Vec<(EntityId, u32)> = flashes
            .iter()
            .map(|flash| (flash.entity, flash.rate))
            .collect();
        let entity = EntityId::from;
        assert_eq!(
            rates,
            [(entity(2), 1), (entity(1), 2)],
            "one flash per view"
        );
        update(&mut flashes, start + FLASH_DURATION, Vec::new());
        assert_eq!(flashes.len(), 1, "the first view's flash is over");
        assert!((flashes[0].strength(start + FLASH_DURATION) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_hottest_views_are_labeled() {
        let flashes = [
            flash(1, (0., 0., 10., 10.), 0, 5),
            flash(2, (0., 0., 10., 10.), 0, 45),
            flash(3, (0., 0., 10., 10.), 0, 30),
            flash(4, (0., 0., 10., 10.), 0, 60),
            flash(5, (0., 0., 10., 10.), 0, 31),
        ];
        assert_eq!(hot(&flashes), [3, 1, 4]);
        assert_eq!(flashes[3].label(), "View ×60/s");
    }
}
