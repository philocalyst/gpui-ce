//! The Entities lens' logic: one row per live entity with its notify rate
//! and history, filtering and sorting, and the sentence that answers "who is
//! watching it, and who keeps poking it?".
//!
//! Notify history comes from [`NotifyStats::buckets`]: counts per 500 ms,
//! newest last, as of the latest frame (buckets advance when frames draw).

use crate::analysis::{
    filter::{TextFilter, haystack},
    format,
    insights::NOTIFY_BUCKET,
    sentence::Sentence,
};
use gpui::{
    EntityId, SharedString,
    inspector::{ElementKey, ElementKind, ElementTree, EntityInfo, NotifyStats},
};
use std::{
    cmp::Ordering,
    collections::VecDeque,
    panic::Location,
    path::{Path, PathBuf},
    time::Duration,
};

/// Buckets of notify history the engine keeps (60 s).
pub const HISTORY_BUCKETS: usize = 120;

/// The window a notify rate is averaged over.
pub const RATE_WINDOW: Duration = Duration::from_secs(2);

fn buckets_per(window: Duration) -> usize {
    (window.as_millis() / NOTIFY_BUCKET.as_millis()).max(1) as usize
}

fn per_second(count: f32) -> f32 {
    count * 1000. / NOTIFY_BUCKET.as_millis() as f32
}

/// Notifies per second over the last [`RATE_WINDOW`] of buckets.
pub fn notify_rate(buckets: &VecDeque<u32>) -> f32 {
    let window = buckets_per(RATE_WINDOW);
    let recent: u32 = buckets.iter().rev().take(window).sum();
    recent as f32 / RATE_WINDOW.as_secs_f32()
}

/// Notifies per second in each bucket of the last 60 s, oldest first,
/// padded with zeros so every entity's history ends now and spans 60 s.
pub fn history(buckets: &VecDeque<u32>) -> Vec<f32> {
    let kept = buckets.len().min(HISTORY_BUCKETS);
    std::iter::repeat_n(0., HISTORY_BUCKETS - kept)
        .chain(
            buckets
                .iter()
                .skip(buckets.len() - kept)
                .map(|&count| per_second(count as f32)),
        )
        .collect()
}

/// The busiest bucket of a history.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    /// Notifies per second in that bucket.
    pub rate: f32,
    /// How long before the latest bucket it was.
    pub ago: Duration,
}

/// The busiest bucket (the most recent one on ties), if any notified.
pub fn peak(buckets: &VecDeque<u32>) -> Option<Peak> {
    let (ix, &count) = buckets
        .iter()
        .enumerate()
        .max_by(|(a_ix, a), (b_ix, b)| a.cmp(b).then(a_ix.cmp(b_ix)))?;
    (count > 0).then(|| Peak {
        rate: per_second(count as f32),
        ago: NOTIFY_BUCKET * (buckets.len() - 1 - ix) as u32,
    })
}

/// A rate as a short number: `0`, `4.5`, `9`, `42`, `1,250`.
pub fn format_rate(rate: f32) -> String {
    if !rate.is_finite() {
        return format::NO_VALUE.to_string();
    }
    if rate >= 100. || rate.fract() == 0. {
        format::count(rate.round() as u64)
    } else {
        format!("{rate:.1}")
    }
}

/// An entity id as its slot number, `#6` (the version bits are noise to a reader).
pub fn entity_label(id: EntityId) -> String {
    format!("#{}", id.as_u64() & 0xffff_ffff)
}

/// Whether a type belongs to Loupe itself (its views and state) rather than
/// to the app being inspected.
pub fn is_loupe_type(type_name: &str) -> bool {
    type_name.starts_with("gpui_inspector::") || type_name.starts_with("gpui::inspector::")
}

/// One live entity, as the table shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityRow {
    /// Entity id.
    pub id: EntityId,
    /// Full type name.
    pub type_name: &'static str,
    /// Readable type name without module paths.
    pub name: SharedString,
    /// Drawn as a view in the latest frame.
    pub is_view: bool,
    /// Belongs to Loupe itself.
    pub loupe: bool,
    /// Strong handles.
    pub strong_count: usize,
    /// `observe` callbacks.
    pub observers: usize,
    /// `subscribe` callbacks.
    pub subscribers: usize,
    /// Notifies since recording started.
    pub notifies: u64,
    /// Notifies in the last 60 s.
    pub recent: u64,
    /// Notifies per second over the last [`RATE_WINDOW`].
    pub rate: f32,
    /// Where the latest notify came from.
    pub last_site: Option<&'static Location<'static>>,
    haystack: String,
}

impl EntityRow {
    /// The row for `entity`, with its notify statistics if it notified.
    pub fn new(entity: &EntityInfo, stats: Option<&NotifyStats>) -> Self {
        let name = format::type_name(entity.type_name);
        let label = entity_label(entity.id);
        let site = entity.last_notify_site.map(format::location);
        let haystack = haystack(
            [
                name.as_ref(),
                entity.type_name,
                label.as_str(),
                if entity.is_view { "view" } else { "model" },
            ]
            .into_iter()
            .chain(site.as_deref()),
        );
        Self {
            id: entity.id,
            type_name: entity.type_name,
            name: name.into_owned().into(),
            is_view: entity.is_view,
            loupe: is_loupe_type(entity.type_name),
            strong_count: entity.strong_count,
            observers: entity.observers,
            subscribers: entity.subscribers,
            notifies: entity.notifies,
            recent: stats.map_or(0, |stats| {
                stats.buckets.iter().map(|&count| u64::from(count)).sum()
            }),
            rate: stats.map_or(0., |stats| notify_rate(&stats.buckets)),
            last_site: entity.last_notify_site,
            haystack,
        }
    }
}

/// Rows for `entities`; `stats` looks up an entity's notify statistics.
pub fn entity_rows<'a>(
    entities: &[EntityInfo],
    stats: impl Fn(EntityId) -> Option<&'a NotifyStats>,
) -> Vec<EntityRow> {
    entities
        .iter()
        .map(|entity| EntityRow::new(entity, stats(entity.id)))
        .collect()
}

/// Which entities the table shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KindFilter {
    /// Every entity.
    #[default]
    All,
    /// Entities drawn as views.
    Views,
    /// Every other entity.
    Models,
}

impl KindFilter {
    /// Every filter, in segment order.
    pub const ALL: [KindFilter; 3] = [KindFilter::All, KindFilter::Views, KindFilter::Models];

    /// The segment label.
    pub fn label(self) -> &'static str {
        match self {
            KindFilter::All => "All",
            KindFilter::Views => "Views",
            KindFilter::Models => "Models",
        }
    }
}

/// Everything the table filters by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntityFilter {
    /// Views, models or all.
    pub kind: KindFilter,
    /// The filter field.
    pub text: TextFilter,
    /// Whether Loupe's own entities are listed.
    pub show_loupe: bool,
}

impl EntityFilter {
    /// Whether `row` is listed.
    pub fn matches(&self, row: &EntityRow) -> bool {
        let kind = match self.kind {
            KindFilter::All => true,
            KindFilter::Views => row.is_view,
            KindFilter::Models => !row.is_view,
        };
        kind && (self.show_loupe || !row.loupe) && self.text.matches(&row.haystack)
    }
}

/// The table's columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityColumn {
    /// `#6`.
    Id,
    /// Short type name.
    Type,
    /// View or model.
    Kind,
    /// Strong handles.
    Refs,
    /// Observers.
    Observers,
    /// Subscribers.
    Subscribers,
    /// Notifies per second.
    Rate,
    /// 60 s notify sparkline.
    History,
    /// Last notify site.
    Site,
}

impl EntityColumn {
    /// Every column, in table order.
    pub const ALL: [EntityColumn; 9] = [
        EntityColumn::Id,
        EntityColumn::Type,
        EntityColumn::Kind,
        EntityColumn::Refs,
        EntityColumn::Observers,
        EntityColumn::Subscribers,
        EntityColumn::Rate,
        EntityColumn::History,
        EntityColumn::Site,
    ];

    /// The header title.
    pub fn title(self) -> &'static str {
        match self {
            EntityColumn::Id => "ID",
            EntityColumn::Type => "Type",
            EntityColumn::Kind => "Kind",
            EntityColumn::Refs => "Refs",
            EntityColumn::Observers => "Obs",
            EntityColumn::Subscribers => "Subs",
            EntityColumn::Rate => "/s",
            EntityColumn::History => "60 s",
            EntityColumn::Site => "Last notify",
        }
    }

    /// Whether the column holds numbers (right-aligned, largest first).
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            EntityColumn::Refs
                | EntityColumn::Observers
                | EntityColumn::Subscribers
                | EntityColumn::Rate
                | EntityColumn::History
        )
    }
}

/// The columns that fit a table `width` logical pixels wide: everything
/// from 600 px, without subscribers and sites from 440 px, and just the
/// essentials below.
pub fn columns_for_width(width: f32) -> Vec<EntityColumn> {
    use EntityColumn::*;
    if width >= 600. {
        EntityColumn::ALL.to_vec()
    } else if width >= 440. {
        vec![Id, Type, Kind, Refs, Observers, Rate, History]
    } else {
        vec![Id, Type, Kind, Rate, History]
    }
}

/// Orders two rows by `column`, ascending.
pub fn compare(column: EntityColumn, a: &EntityRow, b: &EntityRow) -> Ordering {
    match column {
        EntityColumn::Id => a.id.as_u64().cmp(&b.id.as_u64()),
        EntityColumn::Type => a
            .name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.type_name.cmp(b.type_name)),
        EntityColumn::Kind => a.is_view.cmp(&b.is_view),
        EntityColumn::Refs => a.strong_count.cmp(&b.strong_count),
        EntityColumn::Observers => a.observers.cmp(&b.observers),
        EntityColumn::Subscribers => a.subscribers.cmp(&b.subscribers),
        EntityColumn::Rate => a.rate.total_cmp(&b.rate).then(a.recent.cmp(&b.recent)),
        EntityColumn::History => a.recent.cmp(&b.recent).then(a.notifies.cmp(&b.notifies)),
        EntityColumn::Site => {
            let site = |row: &EntityRow| row.last_site.map(format::location);
            match (site(a), site(b)) {
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        }
    }
}

/// "`IssueStore` is notifying 9/s, most recently from `store.rs:88`. 3
/// observers run on each notify, 2 subscribers listen for its events and 4
/// strong handles keep it alive."
pub fn summary(row: &EntityRow) -> Sentence {
    let mut sentence = Sentence::new().code(row.name.clone());
    if row.rate > 0. {
        sentence.push_text(format!(" is notifying {}×/s", format_rate(row.rate)));
    } else if row.recent > 0 {
        sentence.push_text(format!(
            " notified {} in the last minute, none in the last {} s",
            times(row.recent),
            RATE_WINDOW.as_secs()
        ));
    } else if row.notifies > 0 {
        sentence.push_text(format!(
            " notified {} since recording started, none in the last minute",
            times(row.notifies)
        ));
    } else {
        sentence.push_text(" hasn't notified since recording started");
    }
    if let Some(site) = row.last_site.filter(|_| row.notifies > 0) {
        sentence.push_text(", most recently from ");
        sentence.push_code(format::location(site));
    }
    sentence.push_text(". ");

    let mut facts = Vec::new();
    if row.is_view {
        facts.push("it is drawn as a view, so each notify re-renders it".to_string());
    }
    if row.observers > 0 {
        facts.push(format!(
            "{} on each notify",
            plural(row.observers, "observer runs", "observers run")
        ));
    }
    if row.subscribers > 0 {
        facts.push(format!(
            "{} for its events",
            plural(row.subscribers, "subscriber listens", "subscribers listen")
        ));
    }
    facts.push(format!(
        "{} alive",
        plural(
            row.strong_count,
            "strong handle keeps it",
            "strong handles keep it"
        )
    ));
    sentence.push_text(capitalize(&join_facts(&facts)) + ".");
    sentence
}

fn times(count: u64) -> String {
    if count == 1 {
        "once".to_string()
    } else {
        format!("{} times", format::count(count))
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        format::count(count as u64),
        if count == 1 { one } else { many }
    )
}

/// `a`, `a and b`, `a, b and c`.
fn join_facts(facts: &[String]) -> String {
    match facts {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The element that draws view `id` in `tree`.
pub fn view_element(tree: &ElementTree, id: EntityId) -> Option<ElementKey> {
    tree.elements.iter().find_map(|record| match record.kind {
        ElementKind::View { entity, .. } if entity == id => record.key,
        _ => None,
    })
}

/// Where the source file `file` (as recorded by `#[track_caller]`) lives on
/// disk: itself when absolute, otherwise the first existing match under
/// `cwd` or one of its ancestors (paths are relative to the build's root).
pub fn source_path(file: &str, cwd: &Path) -> Option<PathBuf> {
    let path = Path::new(file);
    if path.is_absolute() {
        return path.exists().then(|| path.to_path_buf());
    }
    cwd.ancestors()
        .map(|dir| dir.join(path))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::entity;

    fn buckets(counts: &[u32]) -> VecDeque<u32> {
        counts.iter().copied().collect()
    }

    fn info(id: u64, type_name: &'static str, is_view: bool) -> EntityInfo {
        EntityInfo {
            id: entity(id),
            type_name,
            strong_count: 1,
            is_view,
            observers: 0,
            subscribers: 0,
            notifies: 0,
            last_notify_site: None,
        }
    }

    fn row(id: u64, type_name: &'static str, counts: &[u32]) -> EntityRow {
        let stats = NotifyStats {
            total: counts.iter().map(|&count| u64::from(count)).sum(),
            buckets: buckets(counts),
            last_site: Some(Location::caller()),
        };
        let mut info = info(id, type_name, false);
        info.notifies = stats.total;
        info.last_notify_site = stats.last_site;
        EntityRow::new(&info, Some(&stats))
    }

    #[test]
    fn rates_average_the_last_two_seconds_of_buckets() {
        assert_eq!(notify_rate(&buckets(&[])), 0.);
        assert_eq!(notify_rate(&buckets(&[100, 1, 2, 3, 4])), 5.);
        assert_eq!(notify_rate(&buckets(&[3])), 1.5);
    }

    #[test]
    fn histories_end_now_and_span_a_minute() {
        let history = history(&buckets(&[1, 0, 3]));
        assert_eq!(history.len(), HISTORY_BUCKETS);
        assert_eq!(&history[HISTORY_BUCKETS - 3..], [2., 0., 6.]);
        assert!(
            history[..HISTORY_BUCKETS - 3]
                .iter()
                .all(|&rate| rate == 0.)
        );
        let long: VecDeque<u32> = (0..200).map(|ix| ix as u32).collect();
        assert_eq!(super::history(&long)[0], 160.);
    }

    #[test]
    fn peaks_prefer_the_most_recent_busiest_bucket() {
        let peak = peak(&buckets(&[4, 9, 2, 9, 0, 0])).unwrap();
        assert_eq!(peak.rate, 18.);
        assert_eq!(peak.ago, Duration::from_millis(1000));
        assert_eq!(super::peak(&buckets(&[0, 0])), None);
        assert_eq!(super::peak(&buckets(&[])), None);
    }

    #[test]
    fn rates_format_compactly() {
        assert_eq!(format_rate(0.), "0");
        assert_eq!(format_rate(4.5), "4.5");
        assert_eq!(format_rate(9.), "9");
        assert_eq!(format_rate(142.5), "143");
        assert_eq!(format_rate(1250.), "1,250");
    }

    #[test]
    fn filters_combine_kind_text_and_loupe_ownership() {
        let rows = entity_rows(
            &[
                info(1, "inbox::IssueList", true),
                info(2, "inbox::IssueStore", false),
                info(3, "gpui_inspector::state::LoupeState", false),
            ],
            |_| None,
        );
        let listed = |filter: &EntityFilter| -> Vec<EntityId> {
            rows.iter()
                .filter(|row| filter.matches(row))
                .map(|row| row.id)
                .collect()
        };
        let mut filter = EntityFilter::default();
        assert_eq!(listed(&filter), [entity(1), entity(2)]);
        filter.show_loupe = true;
        assert_eq!(listed(&filter), [entity(1), entity(2), entity(3)]);
        filter.kind = KindFilter::Views;
        assert_eq!(listed(&filter), [entity(1)]);
        filter.kind = KindFilter::Models;
        filter.text = TextFilter::parse("-loupe");
        assert_eq!(listed(&filter), [entity(2)]);
        filter.kind = KindFilter::All;
        filter.text = TextFilter::parse("#3");
        assert_eq!(listed(&filter), [entity(3)]);
        assert!(rows[2].loupe && !rows[0].loupe);
        assert!(is_loupe_type("gpui::inspector::conditional::Inspector"));
    }

    #[test]
    fn rows_sort_by_any_column() {
        let mut quiet = row(1, "inbox::Zebra", &[]);
        quiet.observers = 3;
        let busy = row(2, "inbox::apple", &[0, 4, 4, 4, 4]);
        let order = |column| compare(column, &quiet, &busy);
        assert_eq!(order(EntityColumn::Id), Ordering::Less);
        assert_eq!(order(EntityColumn::Type), Ordering::Greater);
        assert_eq!(order(EntityColumn::Rate), Ordering::Less);
        assert_eq!(order(EntityColumn::History), Ordering::Less);
        assert_eq!(order(EntityColumn::Observers), Ordering::Greater);
        assert_eq!(busy.rate, 8.);
        assert_eq!(busy.recent, 16);
    }

    #[test]
    fn narrow_tables_keep_the_essential_columns() {
        assert_eq!(columns_for_width(800.).len(), 9);
        assert!(!columns_for_width(500.).contains(&EntityColumn::Site));
        let narrow = columns_for_width(300.);
        assert_eq!(
            narrow,
            [
                EntityColumn::Id,
                EntityColumn::Type,
                EntityColumn::Kind,
                EntityColumn::Rate,
                EntityColumn::History
            ]
        );
    }

    #[test]
    fn summaries_answer_who_pokes_it_and_who_watches() {
        let mut store = row(6, "inbox::IssueStore", &[1, 5, 4, 5, 4]);
        store.observers = 3;
        store.subscribers = 2;
        store.strong_count = 4;
        let site = format::location(store.last_site.unwrap());
        assert_eq!(
            summary(&store).to_string(),
            format!(
                "`IssueStore` is notifying 9×/s, most recently from `{site}`. 3 observers run \
                 on each notify, 2 subscribers listen for its events and 4 strong handles keep \
                 it alive."
            )
        );

        let mut list = row(3, "inbox::IssueList", &[2, 0, 0, 0, 0]);
        list.is_view = true;
        assert_eq!(
            summary(&list).to_string(),
            format!(
                "`IssueList` notified 2 times in the last minute, none in the last 2 s, most \
                 recently from `{site}`. It is drawn as a view, so each notify re-renders it \
                 and 1 strong handle keeps it alive."
            )
        );
        let mut once = row(4, "inbox::Search", &[]);
        once.notifies = 1;
        assert!(
            summary(&once)
                .to_string()
                .starts_with("`Search` notified once since recording started")
        );

        let idle = EntityRow::new(&info(7, "inbox::SyncClient", false), None);
        assert_eq!(
            summary(&idle).to_string(),
            "`SyncClient` hasn't notified since recording started. 1 strong handle keeps it alive."
        );
    }

    #[test]
    fn views_are_found_in_the_tree_by_entity() {
        use crate::analysis::fixtures::{TreeBuilder, bounds};
        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 100., 100.));
        let child = builder.child(root, bounds(0., 0., 10., 10.));
        builder.record(child).kind = ElementKind::View {
            entity: entity(9),
            type_name: "app::Pane",
        };
        let key = builder.record(child).key;
        let tree = builder.build();
        assert_eq!(view_element(&tree, entity(9)), key);
        assert_eq!(view_element(&tree, entity(8)), None);
    }

    #[test]
    fn source_files_resolve_against_the_working_directory_or_its_ancestors() {
        let cwd = std::env::current_dir().unwrap();
        let this_file = Location::caller().file();
        assert!(source_path(this_file, &cwd).is_some(), "{this_file}");
        assert!(source_path("no/such/file.rs", &cwd).is_none());
    }
}
