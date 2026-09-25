//! The entity registry behind the inspector's Entities lens.

use super::model::EntityInfo;
use crate::{App, Window};
use smallvec::SmallVec;

/// Every live entity, sorted by id. Views and notify counts come from
/// `windows`: an entity is a view if one of their latest frames drew it as
/// one, and notifies are summed over their inspector captures. The last
/// notify site comes from the first window that recorded one.
pub(crate) fn live_entities<'a>(
    cx: &App,
    windows: impl IntoIterator<Item = &'a Window>,
) -> Vec<EntityInfo> {
    let windows: SmallVec<[&Window; 4]> = windows.into_iter().collect();
    cx.entities
        .live_entities()
        .into_iter()
        .map(|(id, type_name, strong_count)| {
            let notify_stats = windows
                .iter()
                .filter_map(|window| window.inspector_capture()?.notify_stats().get(&id));
            EntityInfo {
                id,
                type_name,
                strong_count,
                is_view: windows.iter().any(|window| {
                    window
                        .rendered_frame
                        .dispatch_tree
                        .view_node_id(id)
                        .is_some()
                }),
                observers: cx.observers.count(&id),
                subscribers: cx.event_listeners.count(&id),
                notifies: notify_stats.clone().map(|stats| stats.total).sum(),
                last_notify_site: notify_stats.filter_map(|stats| stats.last_site).next(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
