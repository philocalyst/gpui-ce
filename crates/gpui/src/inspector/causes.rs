//! Render causes waiting for a window's next frame.
//!
//! A [`CauseLog`] lives in the window's invalidator while the window's
//! inspector is open, so `App::notify` can reach it even while the window is
//! leased out for an update. The window drains it when it starts drawing.

use super::{
    capture::{CaptureClock, NotifyStats},
    model::{CauseKind, RenderCause},
};
use crate::EntityId;
use collections::FxHashMap;
use scheduler::Instant;
use smallvec::SmallVec;
use std::{collections::VecDeque, mem, panic::Location, time::Duration};

/// Width of one [`NotifyStats::buckets`] entry.
pub(crate) const NOTIFY_BUCKET: Duration = Duration::from_millis(500);
/// Number of [`NotifyStats::buckets`] kept per entity (60 s).
pub(crate) const NOTIFY_BUCKETS: usize = 120;
/// Distinct causes kept per frame; identical causes (same kind and site) are
/// merged, so this only bounds pathological cases.
const MAX_PENDING_CAUSES: usize = 256;

/// A cause recorded before the frame it explains began.
pub(crate) struct PendingCause {
    pub(crate) kind: CauseKind,
    pub(crate) site: Option<&'static Location<'static>>,
    /// When it happened, on the capture's clock.
    pub(crate) at: Instant,
    pub(crate) from_inspector: bool,
    /// The notified entity, for notifies and animation frames.
    pub(crate) entity: Option<EntityId>,
}

/// Notifies of one entity since the log was last drained.
#[derive(Default)]
pub(crate) struct NotifyDelta {
    total: u64,
    /// `(bucket index on the capture's time line, count)`, oldest first.
    buckets: SmallVec<[(u64, u32); 2]>,
    last_site: Option<&'static Location<'static>>,
}

/// Causes and notifies recorded since the window last drew.
pub(crate) struct CauseLog {
    clock: CaptureClock,
    causes: Vec<PendingCause>,
    notifies: FxHashMap<EntityId, NotifyDelta>,
}

impl CauseLog {
    /// A log that times causes on the capture's `clock`.
    pub(crate) fn new(clock: CaptureClock) -> Self {
        Self {
            clock,
            causes: Vec::new(),
            notifies: FxHashMap::default(),
        }
    }

    /// Records a cause, merging it into an identical earlier one.
    pub(crate) fn push(
        &mut self,
        kind: CauseKind,
        site: Option<&'static Location<'static>>,
        from_inspector: bool,
    ) {
        self.push_cause(kind, site, from_inspector, None);
    }

    fn push_cause(
        &mut self,
        kind: CauseKind,
        site: Option<&'static Location<'static>>,
        from_inspector: bool,
        entity: Option<EntityId>,
    ) {
        let duplicate = self.causes.iter().any(|cause| {
            cause.kind == kind
                && cause.site == site
                && cause.from_inspector == from_inspector
                && cause.entity == entity
        });
        if !duplicate && self.causes.len() < MAX_PENDING_CAUSES {
            self.causes.push(PendingCause {
                kind,
                site,
                at: self.clock.instant(),
                from_inspector,
                entity,
            });
        }
    }

    /// Records a notify of `entity` from `site`, explained as `kind`
    /// ([`CauseKind::Notify`], or [`CauseKind::Animation`] for an animation
    /// frame), and counts it in the entity's [`NotifyStats`].
    pub(crate) fn push_notify(
        &mut self,
        entity: EntityId,
        kind: CauseKind,
        site: &'static Location<'static>,
    ) {
        self.push_cause(kind, Some(site), false, Some(entity));
        let bucket = bucket_index(self.clock.now());
        let delta = self.notifies.entry(entity).or_default();
        delta.total += 1;
        delta.last_site = Some(site);
        match delta.buckets.last_mut() {
            Some((index, count)) if *index == bucket => *count += 1,
            _ => {
                if delta.buckets.len() == NOTIFY_BUCKETS {
                    delta.buckets.remove(0);
                }
                delta.buckets.push((bucket, 1));
            }
        }
    }

    /// Moves the pending causes and notifies out, keeping allocations around.
    pub(crate) fn drain(
        &mut self,
        causes: &mut Vec<PendingCause>,
        notifies: &mut FxHashMap<EntityId, NotifyDelta>,
    ) {
        causes.clear();
        notifies.clear();
        mem::swap(&mut self.causes, causes);
        mem::swap(&mut self.notifies, notifies);
    }
}

/// Resolves pending causes against the frame that is about to be drawn.
/// `inspector_entities` are the entities only the inspector's UI reads, whose
/// notifies are the inspector's own doing.
pub(crate) fn resolve_causes(
    pending: &mut Vec<PendingCause>,
    frame_start: Instant,
    inspector_entities: &collections::FxHashSet<EntityId>,
    view_types: &FxHashMap<EntityId, &'static str>,
) -> Vec<RenderCause> {
    pending
        .drain(..)
        .map(|mut cause| {
            if let Some(entity) = cause.entity {
                cause.from_inspector |= inspector_entities.contains(&entity);
            }
            if let CauseKind::Notify { entity, type_name } = &mut cause.kind
                && type_name.is_none()
            {
                *type_name = view_types.get(entity).copied();
            }
            RenderCause {
                kind: cause.kind,
                site: cause.site,
                before_frame: frame_start.saturating_duration_since(cause.at),
                from_inspector: cause.from_inspector,
            }
        })
        .collect()
}

/// Folds drained notifies into per-entity statistics whose last bucket is the
/// one containing `now` (an offset on the capture's time line). Buckets of
/// entities that were not notified still advance, so every entity's
/// sparkline ends at the same instant.
pub(crate) fn fold_notify_stats(
    stats: &mut FxHashMap<EntityId, NotifyStats>,
    current_bucket: &mut u64,
    deltas: &mut FxHashMap<EntityId, NotifyDelta>,
    now: Duration,
) {
    let now_bucket = bucket_index(now);
    if now_bucket > *current_bucket {
        let elapsed = (now_bucket - *current_bucket).min(NOTIFY_BUCKETS as u64) as usize;
        for entity_stats in stats.values_mut() {
            advance_buckets(&mut entity_stats.buckets, elapsed);
        }
        *current_bucket = now_bucket;
    }
    for (entity, delta) in deltas.drain() {
        let entity_stats = stats.entry(entity).or_default();
        entity_stats.total += delta.total;
        entity_stats.last_site = delta.last_site.or(entity_stats.last_site);
        for (bucket, count) in delta.buckets {
            let age = now_bucket.saturating_sub(bucket) as usize;
            if age >= NOTIFY_BUCKETS {
                continue;
            }
            let buckets = &mut entity_stats.buckets;
            while buckets.len() <= age {
                buckets.push_front(0);
            }
            let ix = buckets.len() - 1 - age;
            buckets[ix] += count;
        }
    }
}

fn advance_buckets(buckets: &mut VecDeque<u32>, elapsed: usize) {
    if buckets.is_empty() {
        return;
    }
    buckets.extend(std::iter::repeat_n(0, elapsed));
    while buckets.len() > NOTIFY_BUCKETS {
        buckets.pop_front();
    }
}

/// The bucket containing `at`, an offset on the capture's time line.
fn bucket_index(at: Duration) -> u64 {
    (at.as_millis() / NOTIFY_BUCKET.as_millis()) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(id: u64) -> EntityId {
        EntityId::from(id)
    }

    #[test]
    fn identical_causes_merge() {
        let mut log = CauseLog::new(CaptureClock::new(std::rc::Rc::new(Instant::now)));
        let site = Location::caller();
        log.push(CauseKind::Refresh, Some(site), false);
        log.push(CauseKind::Refresh, Some(site), false);
        log.push(CauseKind::Resize, None, false);
        let (mut causes, mut notifies) = (Vec::new(), FxHashMap::default());
        log.drain(&mut causes, &mut notifies);
        assert_eq!(causes.len(), 2);
    }

    #[test]
    fn notify_buckets_end_at_now() {
        let mut stats = FxHashMap::default();
        let mut current = 0;
        let mut deltas = FxHashMap::default();
        let site = Location::caller();
        deltas.insert(
            entity(1),
            NotifyDelta {
                total: 5,
                buckets: SmallVec::from_slice(&[(0, 2), (3, 3)]),
                last_site: Some(site),
            },
        );
        fold_notify_stats(&mut stats, &mut current, &mut deltas, NOTIFY_BUCKET * 4);
        let entity_stats = &stats[&entity(1)];
        assert_eq!(entity_stats.total, 5);
        assert_eq!(entity_stats.last_site, Some(site));
        assert_eq!(
            entity_stats.buckets.iter().copied().collect::<Vec<_>>(),
            vec![2, 0, 0, 3, 0]
        );

        // Two buckets later, with no notifies, the sparkline shifts left.
        fold_notify_stats(&mut stats, &mut current, &mut deltas, NOTIFY_BUCKET * 6);
        assert_eq!(
            stats[&entity(1)]
                .buckets
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![2, 0, 0, 3, 0, 0, 0]
        );
    }

    #[test]
    fn notify_buckets_are_capped() {
        let mut stats = FxHashMap::default();
        let mut current = 0;
        let mut deltas = FxHashMap::default();
        deltas.insert(
            entity(1),
            NotifyDelta {
                total: 1,
                buckets: SmallVec::from_slice(&[(0, 1)]),
                last_site: None,
            },
        );
        fold_notify_stats(&mut stats, &mut current, &mut deltas, Duration::ZERO);
        fold_notify_stats(&mut stats, &mut current, &mut deltas, NOTIFY_BUCKET * 500);
        let buckets = &stats[&entity(1)].buckets;
        assert_eq!(buckets.len(), NOTIFY_BUCKETS);
        assert!(buckets.iter().all(|&count| count == 0));
    }
}
