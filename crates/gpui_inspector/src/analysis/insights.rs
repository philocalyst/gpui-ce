//! Plain-language findings about recent frames: what was slow, and what to do.
//!
//! Each rule reads the capture and, when its named threshold is crossed,
//! produces an [`Insight`]: a title that states the finding with its number,
//! one sentence of explanation, a suggested fix, and links (frame, element,
//! entity, source site) so the UI can jump straight to the cause.

use super::{Severity, bottom_up, format, stats};
use gpui::{
    EntityId,
    inspector::{
        CauseKind, ElementKey, EntityInfo, ForegroundKind, ForegroundSlice, FrameRecord,
        InputRecord, NotifyStats, SceneStats, ViewOutcome,
    },
};
use std::{
    borrow::Cow,
    cmp::Reverse,
    collections::{HashMap, VecDeque},
    panic::Location,
    time::Duration,
};

/// A phase taking at least this share of a slow frame is named as the cause.
pub const DOMINANT_PHASE_SHARE: f64 = 0.5;
/// A view type whose self time is at least this share of render time
/// "dominates" render.
pub const DOMINANT_VIEW_SHARE: f64 = 0.5;
/// Rates ("per second", "in the last second") are measured over this window,
/// ending at the latest app frame.
pub const RECENT_WINDOW: Duration = Duration::from_secs(1);
/// A view rendering at least this often within [`RECENT_WINDOW`] is a hot spot.
pub const HOT_VIEW_RENDERS: u32 = 30;
/// An entity notifying at least this many times per second is storming.
pub const NOTIFY_STORM_PER_SECOND: u64 = 60;
/// Main-thread work before a frame at least this share of the budget is "long".
pub const LONG_TASK_BUDGET_SHARE: f64 = 0.5;
/// Primitive counts growing by at least this factor between frames is a jump...
pub const PRIMITIVE_JUMP_FACTOR: f64 = 2.0;
/// ...if they also grew by at least this many primitives.
pub const PRIMITIVE_JUMP_MIN: u32 = 1_000;
/// A view that has hit the view cache is "cache ineffective" when at least
/// this share of its frames still rendered...
pub const CACHE_MISS_SHARE: f64 = 0.9;
/// ...over at least this many renders.
pub const CACHE_MIN_RENDERS: u32 = 10;
/// Input taking longer than this from arrival to drawn frame feels laggy.
pub const SLOW_INPUT: Duration = Duration::from_millis(50);
/// Input taking longer than this feels broken.
pub const VERY_SLOW_INPUT: Duration = Duration::from_millis(200);
/// The engine's notify bucket width (see [`NotifyStats::buckets`]).
pub const NOTIFY_BUCKET: Duration = Duration::from_millis(500);

/// What an [`Insight`] is about; stable for grouping, icons and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InsightKind {
    /// A frame over budget, decomposed by phase.
    SlowFrame,
    /// One view type's own render dominated a slow frame's render phase.
    RenderDominated,
    /// A view re-rendered very often.
    HotView,
    /// A cached view that re-renders nearly every frame anyway.
    CacheIneffective,
    /// An entity notified very often.
    NotifyStorm,
    /// Long main-thread work delayed a frame.
    LongTask,
    /// Scene primitives jumped between frames.
    PrimitiveJump,
    /// Input took long to reach the screen.
    SlowInput,
}

/// Where an insight points, so the UI can select and reveal it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Links {
    /// The frame to select.
    pub frame: Option<u64>,
    /// The element to reveal.
    pub element: Option<ElementKey>,
    /// The entity involved.
    pub entity: Option<EntityId>,
    /// The source location responsible.
    pub site: Option<&'static Location<'static>>,
}

/// One plain-language finding.
#[derive(Clone, Debug, PartialEq)]
pub struct Insight {
    /// How much it matters.
    pub severity: Severity,
    /// Which rule produced it.
    pub kind: InsightKind,
    /// The finding, with its number: "62% of frame #18372 was layout".
    pub title: String,
    /// One sentence on why it matters.
    pub explanation: String,
    /// What to try.
    pub suggestion: String,
    /// Where to go next.
    pub links: Links,
}

/// Everything the rules read.
pub struct InsightInput<'a, N> {
    /// Recorded frames, oldest first ([`gpui::inspector::InspectorCapture::frames`]).
    pub frames: &'a VecDeque<FrameRecord>,
    /// Recorded input, oldest first.
    pub input: &'a VecDeque<InputRecord>,
    /// Notify statistics per entity, e.g. `capture.notify_stats()`.
    pub notify: N,
    /// Live entities, used for type names (may be empty).
    pub entities: &'a [EntityInfo],
    /// The frame the user selected; the slowest app frame when `None`.
    pub selected_frame: Option<u64>,
    /// Frame budget.
    pub budget: Duration,
}

/// Runs every rule, most severe findings first (rule order within a severity).
pub fn insights<'a, N>(cx: &InsightInput<'a, N>) -> Vec<Insight>
where
    N: IntoIterator<Item = (&'a EntityId, &'a NotifyStats)> + Clone,
{
    let names = TypeNames::new(cx.frames, cx.entities);
    let mut found = Vec::new();
    let focus = cx
        .selected_frame
        .and_then(|id| cx.frames.iter().find(|frame| frame.id == id))
        .or_else(|| stats::worst_frame(cx.frames));
    if let Some(frame) = focus {
        slow_frame(frame, cx.budget, &mut found);
    }
    hot_views(cx.frames, &mut found);
    cache_ineffective(cx.frames, &mut found);
    notify_storms(cx.notify.clone(), &names, &mut found);
    long_tasks(cx.frames, cx.budget, &mut found);
    primitive_jumps(cx.frames, &mut found);
    slow_input(cx.frames, cx.input, &mut found);
    found.sort_by_key(|insight| Reverse(insight.severity));
    found
}

/// How one view entity behaved over a run of app frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewActivity {
    /// The view entity.
    pub entity: EntityId,
    /// Its type.
    pub type_name: &'static str,
    /// Frames in which `render()` ran.
    pub rendered: u32,
    /// Frames in which it was served from the view cache.
    pub cached: u32,
    /// Renders in frames whose scene primitive counts equal the previous app
    /// frame's: a hint (not proof) that the render changed nothing visible.
    pub unchanged_scene: u32,
    /// The latest frame that drew it.
    pub latest_frame: u64,
    /// Its element in the latest frame that retained a tree.
    pub element: Option<ElementKey>,
    /// Its longest render.
    pub slowest: Duration,
    /// The frame of its longest render.
    pub slowest_frame: u64,
}

/// Per-entity view activity over `frames` (app frames only), most rendered first.
pub fn view_activity<'a>(frames: impl IntoIterator<Item = &'a FrameRecord>) -> Vec<ViewActivity> {
    let mut activity: HashMap<EntityId, ViewActivity> = HashMap::new();
    let mut previous_scene: Option<SceneStats> = None;
    for frame in frames.into_iter().filter(|frame| !frame.inspector_only) {
        let scene_unchanged = previous_scene == Some(frame.scene);
        previous_scene = Some(frame.scene);
        for view in &frame.views {
            let entry = activity.entry(view.entity).or_insert_with(|| ViewActivity {
                entity: view.entity,
                type_name: view.type_name,
                rendered: 0,
                cached: 0,
                unchanged_scene: 0,
                latest_frame: frame.id,
                element: None,
                slowest: Duration::ZERO,
                slowest_frame: frame.id,
            });
            entry.latest_frame = frame.id;
            let element = frame
                .tree
                .as_ref()
                .zip(view.element)
                .and_then(|(tree, ix)| tree.get(ix)?.key);
            entry.element = element.or(entry.element);
            match view.outcome {
                ViewOutcome::Cached => entry.cached += 1,
                ViewOutcome::Rendered => {
                    entry.rendered += 1;
                    entry.unchanged_scene += u32::from(scene_unchanged);
                    if view.duration > entry.slowest {
                        entry.slowest = view.duration;
                        entry.slowest_frame = frame.id;
                    }
                }
            }
        }
    }
    let mut activity: Vec<ViewActivity> = activity.into_values().collect();
    activity.sort_by(|a, b| {
        b.rendered
            .cmp(&a.rendered)
            .then(a.type_name.cmp(b.type_name))
            .then(a.latest_frame.cmp(&b.latest_frame))
    });
    activity
}

/// App frames that started within [`RECENT_WINDOW`] of the latest app frame.
pub fn recent_app_frames(
    frames: &VecDeque<FrameRecord>,
) -> impl Iterator<Item = &FrameRecord> + Clone {
    let latest = stats::latest_app_start(frames);
    let since = latest.map(|latest| latest.saturating_sub(RECENT_WINDOW));
    frames.iter().filter(move |frame| {
        !frame.inspector_only && since.is_some_and(|since| frame.start >= since)
    })
}

fn slow_frame(frame: &FrameRecord, budget: Duration, found: &mut Vec<Insight>) {
    let app_total = frame.timings.app_total();
    let severity = match stats::Grade::of(app_total, budget) {
        stats::Grade::Ok => return,
        stats::Grade::Warn => Severity::Warning,
        stats::Grade::Crit => Severity::Critical,
    };
    let timings = &frame.timings;
    let phases = [
        (Phase::Render, timings.render),
        (Phase::Layout, timings.layout),
        (Phase::Prepaint, timings.prepaint),
        (Phase::Paint, timings.paint),
    ];
    let (largest, largest_time) = phases
        .iter()
        .copied()
        .max_by_key(|(_, time)| *time)
        .unwrap_or((Phase::Render, Duration::ZERO));
    let share = stats::ratio(largest_time, app_total);
    let title = if share >= DOMINANT_PHASE_SHARE {
        format!(
            "{} of frame #{} was {}",
            format::percent(share),
            frame.id,
            largest.name()
        )
    } else {
        format!(
            "Frame #{} took {}, {} over budget",
            frame.id,
            format::duration(app_total),
            format::duration(app_total - budget)
        )
    };
    let breakdown: Vec<String> = phases
        .iter()
        .filter(|(_, time)| !time.is_zero())
        .map(|(phase, time)| {
            format!(
                "{} {}",
                phase.name(),
                format::percent(stats::ratio(*time, app_total))
            )
        })
        .collect();
    found.push(Insight {
        severity,
        kind: InsightKind::SlowFrame,
        title,
        explanation: format!(
            "Frame #{} took {} against a {} budget: {}.",
            frame.id,
            format::duration(app_total),
            format::duration(budget),
            breakdown.join(" · ")
        ),
        suggestion: largest.suggestion().to_string(),
        links: Links {
            frame: Some(frame.id),
            ..Links::default()
        },
    });
    if largest == Phase::Render {
        render_dominated(frame, severity, found);
    }
}

/// Names the view type whose own render time dominates a frame's render phase.
fn render_dominated(frame: &FrameRecord, severity: Severity, found: &mut Vec<Insight>) {
    let render = frame.timings.render;
    let Some(top) = bottom_up::bottom_up([frame]).into_iter().next() else {
        return;
    };
    if stats::ratio(top.self_time, render) < DOMINANT_VIEW_SHARE {
        return;
    }
    let self_times = bottom_up::self_times(&frame.views);
    let heaviest = frame
        .views
        .iter()
        .zip(&self_times)
        .filter(|(view, _)| view.type_name == top.type_name)
        .max_by_key(|(_, self_time)| **self_time)
        .map(|(view, _)| view);
    let name = format::type_name(top.type_name);
    let instances = if top.calls > 1 {
        format!(" across {} instances", top.calls)
    } else {
        String::new()
    };
    found.push(Insight {
        severity,
        kind: InsightKind::RenderDominated,
        title: format!(
            "Render dominated by {name} ({})",
            format::duration(top.self_time)
        ),
        explanation: format!(
            "{name}'s own render took {} of the {} spent rendering frame #{}{instances}.",
            format::duration(top.self_time),
            format::duration(render),
            frame.id
        ),
        suggestion: "Virtualize long lists with uniform_list, cache subtrees that did not \
                     change with AnyView::cached, and keep heavy work out of render()."
            .to_string(),
        links: Links {
            frame: Some(frame.id),
            element: heaviest.and_then(|view| frame.tree.as_ref()?.get(view.element?)?.key),
            entity: heaviest.map(|view| view.entity),
            site: None,
        },
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Render,
    Layout,
    Prepaint,
    Paint,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Render => "render",
            Phase::Layout => "layout",
            Phase::Prepaint => "prepaint",
            Phase::Paint => "paint",
        }
    }

    fn suggestion(self) -> &'static str {
        match self {
            Phase::Render => {
                "Render less: cache views that did not change and split views so a notify \
                 rebuilds a smaller subtree."
            }
            Phase::Layout => {
                "Lay out less: give list rows fixed heights, virtualize long lists with \
                 uniform_list, and avoid deeply nested flex containers."
            }
            Phase::Prepaint => {
                "Prepaint less: fewer hitboxes and tooltips, and fewer elements that measure \
                 text during prepaint."
            }
            Phase::Paint => {
                "Paint less: fewer shadows, blurs and borders, and clip or virtualize content \
                 that is off screen."
            }
        }
    }
}

fn hot_views(frames: &VecDeque<FrameRecord>, found: &mut Vec<Insight>) {
    let recent = recent_app_frames(frames);
    let frame_count = recent.clone().count();
    for view in view_activity(recent) {
        if view.rendered < HOT_VIEW_RENDERS {
            break;
        }
        let name = format::type_name(view.type_name);
        let unchanged = match view.unchanged_scene {
            0 => String::new(),
            count => format!(
                "; the scene's primitive counts did not change in {count} of them, so those \
                 renders may have changed nothing visible"
            ),
        };
        found.push(Insight {
            severity: Severity::Warning,
            kind: InsightKind::HotView,
            title: format!("{name} re-rendered {}× in the last second", view.rendered),
            explanation: format!(
                "It rendered in {} of {frame_count} recent frames{unchanged}.",
                view.rendered
            ),
            suggestion: "Find what notifies it in Entities; notify only on real changes, or \
                         move the changing part into its own small view."
                .to_string(),
            links: Links {
                frame: Some(view.latest_frame),
                element: view.element,
                entity: Some(view.entity),
                site: None,
            },
        });
    }
}

fn cache_ineffective(frames: &VecDeque<FrameRecord>, found: &mut Vec<Insight>) {
    for view in view_activity(frames) {
        let drawn = view.rendered + view.cached;
        let misses = f64::from(view.rendered) / f64::from(drawn.max(1));
        if view.cached == 0 || view.rendered < CACHE_MIN_RENDERS || misses < CACHE_MISS_SHARE {
            continue;
        }
        let name = format::type_name(view.type_name);
        found.push(Insight {
            severity: Severity::Warning,
            kind: InsightKind::CacheIneffective,
            title: format!(
                "{name}'s view cache missed {} of {drawn} frames",
                view.rendered
            ),
            explanation: format!(
                "It is drawn as a cached view, yet it re-rendered in {} of the {drawn} frames \
                 that drew it, so caching saves almost nothing.",
                view.rendered
            ),
            suggestion: "Something notifies it (or what it observes) nearly every frame: find \
                         the notifier in Entities and keep fast-changing state out of cached \
                         views."
                .to_string(),
            links: Links {
                frame: Some(view.latest_frame),
                element: view.element,
                entity: Some(view.entity),
                site: None,
            },
        });
    }
}

fn notify_storms<'a>(
    notify: impl IntoIterator<Item = (&'a EntityId, &'a NotifyStats)>,
    names: &TypeNames,
    found: &mut Vec<Insight>,
) {
    let buckets_per_window =
        (RECENT_WINDOW.as_millis() / NOTIFY_BUCKET.as_millis()).max(1) as usize;
    let mut storms: Vec<(EntityId, u64, &NotifyStats)> = notify
        .into_iter()
        .map(|(entity, stats)| {
            let recent: u64 = stats
                .buckets
                .iter()
                .rev()
                .take(buckets_per_window)
                .map(|&count| u64::from(count))
                .sum();
            (*entity, recent, stats)
        })
        .filter(|(_, recent, _)| *recent >= NOTIFY_STORM_PER_SECOND)
        .collect();
    storms.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    for (entity, recent, stats) in storms {
        let name = names.name(entity);
        let site = stats
            .last_site
            .map(|site| format!(", most recently from {}", format::location(site)))
            .unwrap_or_default();
        found.push(Insight {
            severity: Severity::Warning,
            kind: InsightKind::NotifyStorm,
            title: format!(
                "{name} notified {}× in the last second",
                format::count(recent)
            ),
            explanation: format!("Every notify re-renders the views that observe it{site}."),
            suggestion: "Batch updates and notify once per change, or throttle with a timer."
                .to_string(),
            links: Links {
                frame: None,
                element: None,
                entity: Some(entity),
                site: stats.last_site,
            },
        });
    }
}

fn long_tasks(frames: &VecDeque<FrameRecord>, budget: Duration, found: &mut Vec<Insight>) {
    let threshold = budget.mul_f64(LONG_TASK_BUDGET_SHARE);
    let long: Vec<(&FrameRecord, &ForegroundSlice)> = frames
        .iter()
        .filter(|frame| !frame.inspector_only)
        .flat_map(|frame| frame.foreground.iter().map(move |slice| (frame, slice)))
        .filter(|(_, slice)| {
            slice.duration >= threshold
                && matches!(
                    slice.kind,
                    ForegroundKind::Task { .. }
                        | ForegroundKind::Action { .. }
                        | ForegroundKind::Input { .. }
                )
        })
        .collect();
    let Some(&(frame, slice)) = long.iter().max_by_key(|(_, slice)| slice.duration) else {
        return;
    };
    let count = long.len();

    let duration = format::duration(slice.duration);
    let (title, what, site) = match &slice.kind {
        ForegroundKind::Task { site } => (
            format!(
                "A {duration} task blocked the main thread before frame #{}",
                frame.id
            ),
            format!("The task spawned at {} ran", format::location(site)),
            Some(*site),
        ),
        ForegroundKind::Action { name } => (
            format!("The {name} action blocked the main thread for {duration}"),
            "Its handler ran".to_string(),
            None,
        ),
        ForegroundKind::Input { kind } => (
            format!("Handling {kind} blocked the main thread for {duration}"),
            "Input dispatch ran".to_string(),
            None,
        ),
        ForegroundKind::Present | ForegroundKind::SmallPolls { .. } => return,
    };
    let others = match count - 1 {
        0 => String::new(),
        1 => " (1 other long task too)".to_string(),
        others => format!(" ({others} other long tasks too)"),
    };
    found.push(Insight {
        severity: Severity::Warning,
        kind: InsightKind::LongTask,
        title,
        explanation: format!(
            "{what} for {duration} in one go, so frame #{} could not start until it \
             finished{others}.",
            frame.id
        ),
        suggestion: "Move heavy work to cx.background_spawn and apply the result on the main \
                     thread."
            .to_string(),
        links: Links {
            frame: Some(frame.id),
            element: None,
            entity: None,
            site,
        },
    });
}

fn primitive_jumps(frames: &VecDeque<FrameRecord>, found: &mut Vec<Insight>) {
    let app_frames: Vec<&FrameRecord> = frames
        .iter()
        .filter(|frame| !frame.inspector_only)
        .collect();
    let biggest = app_frames
        .windows(2)
        .filter_map(|pair| {
            let (before, after) = (pair[0].scene.primitives(), pair[1].scene.primitives());
            let grew = after.saturating_sub(before);
            let is_jump = grew >= PRIMITIVE_JUMP_MIN
                && f64::from(after) >= f64::from(before) * PRIMITIVE_JUMP_FACTOR;
            is_jump.then_some((pair[1], before, after))
        })
        .max_by_key(|(_, before, after)| after - before);
    let Some((frame, before, after)) = biggest else {
        return;
    };
    found.push(Insight {
        severity: Severity::Info,
        kind: InsightKind::PrimitiveJump,
        title: format!(
            "Primitives jumped from {} to {} in frame #{}",
            format::count(u64::from(before)),
            format::count(u64::from(after)),
            frame.id
        ),
        explanation: format!(
            "The scene grew by {} primitives in one frame; each one costs paint and GPU time.",
            format::count(u64::from(after - before))
        ),
        suggestion: "Check what appeared in that frame: an unvirtualized list, or shadows and \
                     borders repeated on every row."
            .to_string(),
        links: Links {
            frame: Some(frame.id),
            ..Links::default()
        },
    });
}

fn slow_input(
    frames: &VecDeque<FrameRecord>,
    input: &VecDeque<InputRecord>,
    found: &mut Vec<Insight>,
) {
    let lookup = |id: u64| {
        frames
            .binary_search_by_key(&id, |frame| frame.id)
            .ok()
            .and_then(|ix| frames.get(ix))
    };
    let slow: Vec<stats::InputLatency> = stats::input_latencies(input, lookup)
        .into_iter()
        .filter(|latency| latency.latency > SLOW_INPUT)
        .collect();
    let Some(worst) = slow.iter().max_by_key(|latency| latency.latency) else {
        return;
    };
    let Some(record) = input.iter().find(|record| record.seq == worst.seq) else {
        return;
    };
    let detail = if record.detail.is_empty() {
        String::new()
    } else {
        format!(" ({})", record.detail)
    };
    let count = match slow.len() {
        1 => "1 input event".to_string(),
        count => format!("{count} input events"),
    };
    found.push(Insight {
        severity: if worst.latency > VERY_SLOW_INPUT {
            Severity::Critical
        } else {
            Severity::Warning
        },
        kind: InsightKind::SlowInput,
        title: format!(
            "A {:?}{detail} took {} to reach the screen",
            record.kind,
            format::duration(worst.latency)
        ),
        explanation: format!(
            "{count} took longer than {} from arrival to the end of the frame that drew them.",
            format::duration(SLOW_INPUT)
        ),
        suggestion: format!(
            "Open frame #{} and look at the main-thread work before it and its slowest phase.",
            worst.frame
        ),
        links: Links {
            frame: Some(worst.frame),
            ..Links::default()
        },
    });
}

/// Best-known type names of entities, from the entity registry, rendered
/// views and notify causes.
struct TypeNames(HashMap<EntityId, &'static str>);

impl TypeNames {
    fn new(frames: &VecDeque<FrameRecord>, entities: &[EntityInfo]) -> Self {
        let mut names = HashMap::new();
        for frame in frames {
            for cause in &frame.causes {
                if let CauseKind::Notify {
                    entity,
                    type_name: Some(type_name),
                } = cause.kind
                {
                    names.insert(entity, type_name);
                }
            }
            for view in &frame.views {
                names.insert(view.entity, view.type_name);
            }
        }
        for entity in entities {
            names.insert(entity.id, entity.type_name);
        }
        Self(names)
    }

    fn name(&self, entity: EntityId) -> Cow<'static, str> {
        match self.0.get(&entity) {
            Some(&type_name) => format::type_name(type_name),
            None => Cow::Owned(format!("Entity {entity}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{
        TreeBuilder, bounds, cached_view, entity, frames_every, input, ms, view,
    };
    use gpui::inspector::{InputKind, PhaseTimings, RenderCause};
    use std::sync::Arc;

    const BUDGET: Duration = Duration::from_micros(16_667);

    fn run(frames: &VecDeque<FrameRecord>) -> Vec<Insight> {
        run_with(frames, &VecDeque::new(), &HashMap::new(), None)
    }

    fn run_with(
        frames: &VecDeque<FrameRecord>,
        input: &VecDeque<InputRecord>,
        notify: &HashMap<EntityId, NotifyStats>,
        selected_frame: Option<u64>,
    ) -> Vec<Insight> {
        insights(&InsightInput {
            frames,
            input,
            notify,
            entities: &[],
            selected_frame,
            budget: BUDGET,
        })
    }

    fn kinds(insights: &[Insight]) -> Vec<InsightKind> {
        insights.iter().map(|insight| insight.kind).collect()
    }

    fn ring(frames: Vec<FrameRecord>) -> VecDeque<FrameRecord> {
        frames.into()
    }

    /// 60 calm frames at 60 fps: nothing to report.
    fn calm() -> Vec<FrameRecord> {
        frames_every(60, Duration::from_micros(16_667), ms(4.0))
    }

    #[test]
    fn a_calm_capture_has_no_insights() {
        assert!(run(&ring(calm())).is_empty());
        assert!(run(&VecDeque::new()).is_empty());
    }

    #[test]
    fn slow_frame_names_the_dominant_phase() {
        let mut frames = calm();
        frames[10].timings = PhaseTimings {
            render: ms(5.0),
            layout: ms(15.5),
            paint: ms(4.5),
            total: ms(25.0),
            ..PhaseTimings::default()
        };
        let found = run(&ring(frames));
        assert_eq!(kinds(&found), vec![InsightKind::SlowFrame]);
        let insight = &found[0];
        assert_eq!(insight.title, "62% of frame #10 was layout");
        assert_eq!(insight.severity, Severity::Warning);
        assert_eq!(
            insight.explanation,
            "Frame #10 took 25.0 ms against a 16.7 ms budget: \
             render 20% · layout 62% · paint 18%."
        );
        assert!(insight.suggestion.starts_with("Lay out less"));
        assert_eq!(insight.links.frame, Some(10));
    }

    #[test]
    fn slow_frame_without_a_dominant_phase_states_the_overrun() {
        let mut frames = calm();
        frames[3].timings = PhaseTimings {
            render: ms(12.0),
            layout: ms(10.0),
            prepaint: ms(4.0),
            paint: ms(10.0),
            total: ms(36.0),
            ..PhaseTimings::default()
        };
        let found = run(&ring(frames));
        assert_eq!(found[0].title, "Frame #3 took 36.0 ms, 19.3 ms over budget");
        assert_eq!(found[0].severity, Severity::Critical);
    }

    #[test]
    fn slow_frame_excludes_loupes_own_time() {
        let mut frames = calm();
        frames[5].timings.total = ms(20.0);
        frames[5].timings.inspector = ms(6.0);
        assert!(run(&ring(frames)).is_empty());
    }

    #[test]
    fn the_selected_frame_is_analyzed_instead_of_the_worst() {
        let mut frames = calm();
        frames[2].timings.total = ms(40.0);
        frames[2].timings.render = ms(40.0);
        frames[7].timings.total = ms(20.0);
        frames[7].timings.render = ms(20.0);
        let selected = run_with(
            &ring(frames.clone()),
            &VecDeque::new(),
            &HashMap::new(),
            Some(7),
        );
        assert_eq!(selected[0].links.frame, Some(7));
        let worst = run(&ring(frames));
        assert_eq!(worst[0].links.frame, Some(2));
        // A selected frame within budget reports nothing, even if another is slow.
        let mut calm_selected = calm();
        calm_selected[2].timings.total = ms(40.0);
        assert!(
            run_with(
                &ring(calm_selected),
                &VecDeque::new(),
                &HashMap::new(),
                Some(4)
            )
            .is_empty()
        );
    }

    #[test]
    fn render_dominated_by_one_view_type() {
        let mut frames = calm();
        let slow = &mut frames[20];
        slow.timings.total = ms(24.0);
        slow.timings.render = ms(14.0);
        slow.views = vec![
            view(1, "app::Workspace", 0, ms(0.0), ms(13.0)),
            view(2, "app::IssueList", 1, ms(1.0), ms(11.0)),
            view(3, "app::Row", 2, ms(2.0), ms(1.9)),
        ];
        let mut tree = TreeBuilder::new();
        let root = tree.root(bounds(0., 0., 800., 600.));
        let list = tree.child(root, bounds(0., 0., 800., 600.));
        slow.views[1].element = Some(list);
        slow.tree = Some(Arc::new(tree.build()));

        let found = run(&ring(frames));
        assert_eq!(
            kinds(&found),
            vec![InsightKind::SlowFrame, InsightKind::RenderDominated]
        );
        let dominated = &found[1];
        assert_eq!(dominated.title, "Render dominated by IssueList (9.1 ms)");
        assert_eq!(
            dominated.explanation,
            "IssueList's own render took 9.1 ms of the 14.0 ms spent rendering frame #20."
        );
        assert_eq!(dominated.links.entity, Some(entity(2)));
        assert_eq!(dominated.links.element.map(|key| key.path.0), Some(list));
    }

    #[test]
    fn render_spread_across_views_is_not_dominated() {
        let mut frames = calm();
        let slow = &mut frames[20];
        slow.timings.total = ms(24.0);
        slow.timings.render = ms(20.0);
        slow.views = vec![
            view(1, "app::A", 0, ms(0.0), ms(6.0)),
            view(2, "app::B", 0, ms(6.0), ms(6.0)),
            view(3, "app::C", 0, ms(12.0), ms(6.0)),
        ];
        assert_eq!(kinds(&run(&ring(frames))), vec![InsightKind::SlowFrame]);
    }

    #[test]
    fn hot_views_fire_at_the_threshold() {
        let mut frames = calm();
        for (ix, frame) in frames.iter_mut().enumerate() {
            frame.scene.quads = 10;
            if ix >= 60 - HOT_VIEW_RENDERS as usize {
                frame.views = vec![view(9, "app::Clock", 0, ms(0.0), ms(0.2))];
            }
        }
        let found = run(&ring(frames.clone()));
        assert_eq!(kinds(&found), vec![InsightKind::HotView]);
        assert_eq!(found[0].title, "Clock re-rendered 30× in the last second");
        assert_eq!(
            found[0].explanation,
            "It rendered in 30 of 60 recent frames; the scene's primitive counts did not change \
             in 30 of them, so those renders may have changed nothing visible."
        );
        assert_eq!(found[0].links.entity, Some(entity(9)));
        assert_eq!(found[0].links.frame, Some(59));

        frames[59].views.clear();
        assert!(run(&ring(frames)).is_empty());
    }

    #[test]
    fn hot_views_only_count_the_last_second() {
        // 30 renders, but spread over two seconds.
        let mut frames = frames_every(60, Duration::from_millis(33), ms(1.0));
        for frame in &mut frames {
            frame.views = vec![view(9, "app::Clock", 0, ms(0.0), ms(0.2))];
        }
        let frames: Vec<FrameRecord> = frames.into_iter().step_by(2).collect();
        assert!(run(&ring(frames)).is_empty());
    }

    #[test]
    fn cache_ineffective_needs_a_cache_hit_and_mostly_misses() {
        let mut frames = frames_every(20, Duration::from_millis(100), ms(1.0));
        for frame in &mut frames {
            frame.views = vec![view(4, "app::Sidebar", 0, ms(0.0), ms(0.3))];
        }
        // Never cached: not known to be a cached view.
        assert!(run(&ring(frames.clone())).is_empty());

        frames[0].views = vec![cached_view(4, "app::Sidebar", 0, ms(0.0), ms(0.01))];
        let found = run(&ring(frames.clone()));
        assert_eq!(kinds(&found), vec![InsightKind::CacheIneffective]);
        assert_eq!(
            found[0].title,
            "Sidebar's view cache missed 19 of 20 frames"
        );

        frames[1].views = vec![cached_view(4, "app::Sidebar", 0, ms(0.0), ms(0.01))];
        frames[2].views = vec![cached_view(4, "app::Sidebar", 0, ms(0.0), ms(0.01))];
        assert!(run(&ring(frames)).is_empty(), "17 of 20 is under 90%");
    }

    fn notify_stats(buckets: &[u32]) -> NotifyStats {
        NotifyStats {
            total: buckets.iter().map(|&count| u64::from(count)).sum(),
            buckets: buckets.iter().copied().collect(),
            last_site: Some(Location::caller()),
        }
    }

    #[test]
    fn notify_storms_use_the_last_second_of_buckets() {
        let mut frames = calm();
        frames[0].causes.push(RenderCause {
            kind: CauseKind::Notify {
                entity: entity(7),
                type_name: Some("app::IssueStore"),
            },
            site: None,
            before_frame: Duration::ZERO,
            from_inspector: false,
        });
        let notify = HashMap::from([
            (entity(7), notify_stats(&[500, 1, 120, 120])),
            (entity(8), notify_stats(&[400, 400, 30, 29])),
            (entity(9), notify_stats(&[])),
        ]);
        let found = run_with(&ring(frames), &VecDeque::new(), &notify, None);
        assert_eq!(kinds(&found), vec![InsightKind::NotifyStorm]);
        assert_eq!(
            found[0].title,
            "IssueStore notified 240× in the last second"
        );
        assert!(found[0].explanation.starts_with(
            "Every notify re-renders the views that observe it, most recently from insights.rs:"
        ));
        assert!(found[0].links.site.is_some());
    }

    #[test]
    fn notify_storms_on_unknown_entities_use_the_id() {
        let notify = HashMap::from([(entity(42), notify_stats(&[30, 30]))]);
        let found = run_with(&ring(calm()), &VecDeque::new(), &notify, None);
        assert_eq!(
            found[0].title,
            format!("Entity {} notified 60× in the last second", entity(42))
        );
    }

    #[test]
    fn long_tasks_report_the_longest() {
        let mut frames = calm();
        frames[4].foreground = vec![ForegroundSlice {
            kind: ForegroundKind::Task {
                site: Location::caller(),
            },
            start: ms(40.0),
            duration: ms(9.0),
        }];
        frames[8].foreground = vec![
            ForegroundSlice {
                kind: ForegroundKind::Action { name: "app::Open" },
                start: ms(100.0),
                duration: ms(30.0),
            },
            ForegroundSlice {
                kind: ForegroundKind::SmallPolls { count: 9 },
                start: ms(131.0),
                duration: ms(50.0),
            },
        ];
        let found = run(&ring(frames.clone()));
        assert_eq!(kinds(&found), vec![InsightKind::LongTask]);
        assert_eq!(
            found[0].title,
            "The app::Open action blocked the main thread for 30.0 ms"
        );
        assert_eq!(
            found[0].explanation,
            "Its handler ran for 30.0 ms in one go, so frame #8 could not start until it \
             finished (1 other long task too)."
        );

        frames[8].foreground.clear();
        let found = run(&ring(frames.clone()));
        assert_eq!(
            found[0].title,
            "A 9.0 ms task blocked the main thread before frame #4"
        );
        assert!(found[0].links.site.is_some());

        frames[4].foreground[0].duration = ms(8.0);
        assert!(run(&ring(frames)).is_empty(), "under half the budget");
    }

    #[test]
    fn primitive_jumps_need_both_factor_and_size() {
        let mut frames = calm();
        for frame in &mut frames {
            frame.scene.quads = 1_200;
        }
        frames[30].scene.quads = 4_800;
        let found = run(&ring(frames.clone()));
        assert_eq!(kinds(&found), vec![InsightKind::PrimitiveJump]);
        assert_eq!(
            found[0].title,
            "Primitives jumped from 1,200 to 4,800 in frame #30"
        );
        assert_eq!(found[0].severity, Severity::Info);

        frames[30].scene.quads = 2_300;
        assert!(run(&ring(frames.clone())).is_empty(), "under 2×");
        for frame in &mut frames {
            frame.scene.quads = 10;
        }
        frames[30].scene.quads = 900;
        assert!(run(&ring(frames)).is_empty(), "under 1,000 new primitives");
    }

    #[test]
    fn slow_input_reports_the_worst_outlier() {
        let frames = calm();
        let at = |frame: usize, before: f64| frames[frame].start.saturating_sub(ms(before));
        let input_ring: VecDeque<InputRecord> = vec![
            input(0, at(10, 10.0), InputKind::MouseDown, Some(10)),
            input(1, at(20, 60.0), InputKind::KeyDown, Some(20)),
            input(2, at(30, 250.0), InputKind::MouseUp, Some(30)),
        ]
        .into();
        let found = run_with(&ring(frames.clone()), &input_ring, &HashMap::new(), None);
        assert_eq!(kinds(&found), vec![InsightKind::SlowInput]);
        assert_eq!(
            found[0].title,
            "A MouseUp (MouseUp) took 254.0 ms to reach the screen"
        );
        assert_eq!(found[0].severity, Severity::Critical);
        assert_eq!(
            found[0].explanation,
            "2 input events took longer than 50.0 ms from arrival to the end of the frame that \
             drew them."
        );
        assert_eq!(found[0].links.frame, Some(30));

        let quick: VecDeque<InputRecord> =
            vec![input(0, at(10, 10.0), InputKind::MouseDown, Some(10))].into();
        assert!(run_with(&ring(frames), &quick, &HashMap::new(), None).is_empty());
    }

    #[test]
    fn insights_are_ordered_by_severity() {
        let mut frames = calm();
        frames[30].scene.quads = 5_000;
        frames[10].timings.total = ms(40.0);
        frames[10].timings.render = ms(40.0);
        let found = run(&ring(frames));
        assert_eq!(
            kinds(&found),
            vec![InsightKind::SlowFrame, InsightKind::PrimitiveJump]
        );
        assert!(found[0].severity > found[1].severity);
    }

    #[test]
    fn view_activity_counts_renders_and_cache_hits_per_entity() {
        let mut frames = frames_every(3, ms(16.0), ms(1.0));
        frames[0].views = vec![view(1, "app::A", 0, ms(0.0), ms(2.0))];
        frames[1].views = vec![
            view(1, "app::A", 0, ms(0.0), ms(5.0)),
            view(2, "app::B", 1, ms(1.0), ms(1.0)),
        ];
        frames[2].views = vec![cached_view(1, "app::A", 0, ms(0.0), ms(0.1))];
        frames[2].inspector_only = true;

        let activity = view_activity(&frames);
        assert_eq!(activity.len(), 2);
        let a = &activity[0];
        assert_eq!((a.type_name, a.rendered, a.cached), ("app::A", 2, 0));
        assert_eq!(
            (a.slowest, a.slowest_frame, a.latest_frame),
            (ms(5.0), 1, 1)
        );
        assert_eq!(a.unchanged_scene, 1);
    }
}
