//! Export to the Chrome Trace Event Format, for ui.perfetto.dev or
//! chrome://tracing.
//!
//! One process with a thread lane per kind of work: frames (with their
//! phases nested inside), views, user spans, main-thread slices (plus input
//! as instant events) and the causes of each frame, connected to the frame
//! they caused by flow arrows. Timestamps are microseconds from the capture
//! epoch.

use super::format;
use gpui::inspector::{
    CauseKind, ForegroundKind, FrameRecord, InputRecord, RenderCause, ViewOutcome,
};
use serde_json::{Value, json};
use std::{panic::Location, time::Duration};

/// The single process all lanes belong to.
const PID: u32 = 1;
/// Lane (thread id) of each kind of event, in display order.
const FRAMES_LANE: u32 = 1;
const VIEWS_LANE: u32 = 2;
const USER_SPANS_LANE: u32 = 3;
const MAIN_THREAD_LANE: u32 = 4;
const CAUSES_LANE: u32 = 5;
const LANES: [(u32, &str); 5] = [
    (FRAMES_LANE, "Frames"),
    (VIEWS_LANE, "Views"),
    (USER_SPANS_LANE, "User spans"),
    (MAIN_THREAD_LANE, "Main thread"),
    (CAUSES_LANE, "Causes"),
];
/// Causes are instants; they are drawn as slices this long so flow arrows
/// have a slice to start from.
const CAUSE_MARKER: Duration = Duration::from_micros(1);
/// Explains how phases are laid out, in the trace's metadata.
const PHASE_NOTE: &str = "Phases are laid out in order from the frame's start; layout time \
                          is accumulated across lazy layout passes, so its position is \
                          approximate.";

/// Builds a Chrome trace of `frames` and `input` (e.g. a capture's rings,
/// or a selected range of them). `process_name` names the process, e.g. the
/// window title.
pub fn chrome_trace<'a>(
    frames: impl IntoIterator<Item = &'a FrameRecord>,
    input: impl IntoIterator<Item = &'a InputRecord>,
    process_name: &str,
) -> Value {
    let mut events = Vec::new();
    let mut frame_count = 0usize;
    let mut next_flow_id = 0u64;
    for frame in frames {
        frame_count += 1;
        push_frame(&mut events, frame);
        push_phases(&mut events, frame);
        push_views(&mut events, frame);
        push_user_spans(&mut events, frame);
        push_main_thread(&mut events, frame);
        for cause in &frame.causes {
            push_cause(&mut events, frame, cause, next_flow_id);
            next_flow_id += 1;
        }
    }
    for record in input {
        push_input(&mut events, record);
    }
    // Stable, so a parent emitted before its children stays first at equal times.
    events.sort_by(|a, b| timestamp(a).total_cmp(&timestamp(b)));

    let mut trace_events = metadata(process_name);
    trace_events.extend(events);
    json!({
        "traceEvents": trace_events,
        "displayTimeUnit": "ms",
        "otherData": {
            "source": "Loupe",
            "frames": frame_count,
            "phases": PHASE_NOTE,
        },
    })
}

/// [`chrome_trace`] serialized, ready to save as a `.json` file.
pub fn chrome_trace_json<'a>(
    frames: impl IntoIterator<Item = &'a FrameRecord>,
    input: impl IntoIterator<Item = &'a InputRecord>,
    process_name: &str,
) -> String {
    chrome_trace(frames, input, process_name).to_string()
}

fn metadata(process_name: &str) -> Vec<Value> {
    let mut events = vec![json!({
        "name": "process_name", "ph": "M", "pid": PID, "tid": 0,
        "args": { "name": process_name },
    })];
    for (tid, name) in LANES {
        events.push(json!({
            "name": "thread_name", "ph": "M", "pid": PID, "tid": tid,
            "args": { "name": name },
        }));
        events.push(json!({
            "name": "thread_sort_index", "ph": "M", "pid": PID, "tid": tid,
            "args": { "sort_index": tid },
        }));
    }
    events
}

/// Microseconds, with nanosecond precision.
fn micros(duration: Duration) -> f64 {
    duration.as_nanos() as f64 / 1e3
}

fn timestamp(event: &Value) -> f64 {
    event["ts"].as_f64().unwrap_or(f64::NEG_INFINITY)
}

fn complete(
    name: &str,
    category: &str,
    lane: u32,
    start: Duration,
    duration: Duration,
    args: Value,
) -> Value {
    json!({
        "name": name, "cat": category, "ph": "X",
        "ts": micros(start), "dur": micros(duration),
        "pid": PID, "tid": lane, "args": args,
    })
}

fn site(location: &Location<'_>) -> String {
    format::location_with_path(location)
}

fn push_frame(events: &mut Vec<Value>, frame: &FrameRecord) {
    let name = if frame.inspector_only {
        format!("Frame #{} (Loupe)", frame.id)
    } else {
        format!("Frame #{}", frame.id)
    };
    let causes: Vec<String> = frame.causes.iter().map(cause_name).collect();
    events.push(complete(
        &name,
        "frame",
        FRAMES_LANE,
        frame.start,
        frame.timings.total,
        json!({
            "id": frame.id,
            "app_ms": micros(frame.timings.app_total()) / 1e3,
            "inspector_ms": micros(frame.timings.inspector) / 1e3,
            "input_ms": micros(frame.timings.input) / 1e3,
            "inspector_only": frame.inspector_only,
            "replayed_app": frame.replayed_app(),
            "viewport": format::size(frame.viewport),
            "elements": frame.element_count,
            "primitives": frame.scene.primitives(),
            "causes": causes,
        }),
    ));
}

/// Phases nested in the frame, laid out in order and clipped to its end.
fn push_phases(events: &mut Vec<Value>, frame: &FrameRecord) {
    let timings = &frame.timings;
    let end = frame.start + timings.total;
    let phases = [
        ("render", timings.render),
        ("layout", timings.layout),
        ("prepaint", timings.prepaint),
        ("paint", timings.paint),
        ("inspector", timings.inspector),
        ("present", timings.present.unwrap_or_default()),
    ];
    let mut cursor = frame.start;
    for (name, duration) in phases {
        let clipped = duration.min(end.saturating_sub(cursor));
        if clipped.is_zero() {
            continue;
        }
        events.push(complete(
            name,
            "phase",
            FRAMES_LANE,
            cursor,
            clipped,
            json!({ "ms": micros(duration) / 1e3, "clipped": clipped < duration }),
        ));
        cursor += clipped;
    }
}

fn push_views(events: &mut Vec<Value>, frame: &FrameRecord) {
    for view in &frame.views {
        let outcome = match view.outcome {
            ViewOutcome::Rendered => "rendered",
            ViewOutcome::Cached => "cached",
            ViewOutcome::Replayed => "replayed",
        };
        events.push(complete(
            &format::type_name(view.type_name),
            "view",
            VIEWS_LANE,
            frame.start + view.start,
            view.duration,
            json!({
                "type": view.type_name,
                "entity": view.entity.as_u64(),
                "outcome": outcome,
                "frame": frame.id,
            }),
        ));
    }
}

fn push_user_spans(events: &mut Vec<Value>, frame: &FrameRecord) {
    for span in &frame.spans {
        events.push(complete(
            &span.name,
            "span",
            USER_SPANS_LANE,
            frame.start + span.start,
            span.duration,
            json!({ "site": site(span.site), "frame": frame.id }),
        ));
    }
}

fn push_main_thread(events: &mut Vec<Value>, frame: &FrameRecord) {
    for slice in &frame.foreground {
        let (name, args) = match &slice.kind {
            ForegroundKind::Task { site: spawned } => (
                format!("Task {}", format::location(spawned)),
                json!({ "site": site(spawned) }),
            ),
            ForegroundKind::Action { name } => (format!("Action {name}"), json!({})),
            ForegroundKind::Input { kind } => (format!("Input {kind}"), json!({})),
            ForegroundKind::Present => ("Present".to_string(), json!({})),
            ForegroundKind::SmallPolls { count } => (
                format!("{} small polls", format::count(u64::from(*count))),
                json!({ "count": count }),
            ),
        };
        events.push(complete(
            &name,
            "main",
            MAIN_THREAD_LANE,
            slice.start,
            slice.duration,
            args,
        ));
    }
}

fn cause_name(cause: &RenderCause) -> String {
    match &cause.kind {
        CauseKind::Notify {
            type_name: Some(type_name),
            ..
        } => format!("notify {}", format::type_name(type_name)),
        CauseKind::Notify { entity, .. } => format!("notify entity {entity}"),
        CauseKind::Refresh => "refresh".to_string(),
        CauseKind::Focus => "focus".to_string(),
        CauseKind::Resize => "resize".to_string(),
        CauseKind::WindowState => "window state".to_string(),
        CauseKind::Input { event } => format!("input {event}"),
        CauseKind::Animation => "animation".to_string(),
        CauseKind::Initial => "initial".to_string(),
    }
}

/// A marker slice for the cause, and a flow arrow from it to the frame.
fn push_cause(events: &mut Vec<Value>, frame: &FrameRecord, cause: &RenderCause, flow_id: u64) {
    let at = frame.start.saturating_sub(cause.before_frame);
    let mut args = json!({
        "frame": frame.id,
        "from_inspector": cause.from_inspector,
    });
    if let Some(cause_site) = cause.site {
        args["site"] = json!(site(cause_site));
    }
    if let CauseKind::Notify { entity, .. } = &cause.kind {
        args["entity"] = json!(entity.as_u64());
    }
    events.push(complete(
        &cause_name(cause),
        "cause",
        CAUSES_LANE,
        at,
        CAUSE_MARKER,
        args,
    ));
    events.push(json!({
        "name": "caused", "cat": "cause", "ph": "s", "id": flow_id,
        "ts": micros(at), "pid": PID, "tid": CAUSES_LANE,
    }));
    events.push(json!({
        "name": "caused", "cat": "cause", "ph": "f", "bp": "e", "id": flow_id,
        "ts": micros(frame.start), "pid": PID, "tid": FRAMES_LANE,
    }));
}

fn push_input(events: &mut Vec<Value>, record: &InputRecord) {
    let actions: Vec<Value> = record
        .actions
        .iter()
        .map(|action| json!({ "name": action.name, "handled": action.handled }))
        .collect();
    events.push(json!({
        "name": format!("{:?}", record.kind), "cat": "input", "ph": "i", "s": "t",
        "ts": micros(record.at), "pid": PID, "tid": MAIN_THREAD_LANE,
        "args": {
            "seq": record.seq,
            "detail": record.detail.as_ref(),
            "handled": record.handled,
            "frame": record.frame,
            "inspector": record.inspector,
            "actions": actions,
        },
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{
        cached_view, entity, frame, input, ms, replayed_view, us, view,
    };
    use gpui::inspector::{ActionRecord, ForegroundSlice, InputKind, PhaseTimings, UserSpan};

    fn parse(frames: &[FrameRecord], input: &[InputRecord]) -> Value {
        let text = chrome_trace_json(frames, input, "Issues — ünïcødé");
        serde_json::from_str(&text).expect("the export is valid JSON")
    }

    fn events(trace: &Value) -> &Vec<Value> {
        trace["traceEvents"]
            .as_array()
            .expect("traceEvents is an array")
    }

    fn find<'a>(trace: &'a Value, phase: &str, name: &str) -> Vec<&'a Value> {
        events(trace)
            .iter()
            .filter(|event| event["ph"] == phase && event["name"] == name)
            .collect()
    }

    fn span(event: &Value) -> (f64, f64) {
        let start = event["ts"].as_f64().unwrap();
        (start, start + event["dur"].as_f64().unwrap())
    }

    fn contains(outer: &Value, inner: &Value) -> bool {
        let ((outer_start, outer_end), (inner_start, inner_end)) = (span(outer), span(inner));
        outer_start <= inner_start && inner_end <= outer_end
    }

    fn busy_frame() -> FrameRecord {
        let mut record = frame(7, ms(1_000.0), ms(20.0));
        record.timings = PhaseTimings {
            input: ms(1.0),
            render: ms(8.0),
            layout: ms(5.0),
            prepaint: ms(2.0),
            paint: ms(3.0),
            inspector: ms(1.0),
            present: Some(ms(4.0)),
            total: ms(20.0),
        };
        record.views = vec![
            view(1, "app::Workspace", 0, ms(0.0), ms(8.0)),
            view(2, "app::IssueList<app::Row>", 1, ms(1.0), ms(6.0)),
            cached_view(3, "app::Sidebar", 1, ms(7.0), us(50)),
        ];
        record.spans = vec![UserSpan {
            name: "filter issues".into(),
            site: Location::caller(),
            depth: 0,
            start: ms(2.0),
            duration: ms(3.0),
        }];
        record.foreground = vec![ForegroundSlice {
            kind: ForegroundKind::Task {
                site: Location::caller(),
            },
            start: ms(990.0),
            duration: ms(6.0),
        }];
        record.causes = vec![RenderCause {
            kind: CauseKind::Notify {
                entity: entity(9),
                type_name: Some("app::IssueStore"),
            },
            site: Some(Location::caller()),
            before_frame: ms(4.0),
            from_inspector: false,
        }];
        record
    }

    #[test]
    fn empty_capture_exports_only_metadata() {
        let trace = parse(&[], &[]);
        assert!(events(&trace).iter().all(|event| event["ph"] == "M"));
        assert_eq!(trace["otherData"]["frames"], 0);
        let process = find(&trace, "M", "process_name");
        assert_eq!(process[0]["args"]["name"], "Issues — ünïcødé");
        let lanes: Vec<&str> = find(&trace, "M", "thread_name")
            .iter()
            .map(|event| event["args"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            lanes,
            vec!["Frames", "Views", "User spans", "Main thread", "Causes"]
        );
    }

    #[test]
    fn frames_contain_their_phases_in_order() {
        let trace = parse(&[busy_frame()], &[]);
        let frame_event = find(&trace, "X", "Frame #7")[0];
        assert_eq!(span(frame_event), (1_000_000.0, 1_020_000.0));
        assert_eq!(frame_event["tid"], FRAMES_LANE);
        assert_eq!(frame_event["args"]["app_ms"], 19.0);
        assert_eq!(frame_event["args"]["causes"][0], "notify IssueStore");

        let phases: Vec<(&str, (f64, f64))> = events(&trace)
            .iter()
            .filter(|event| event["cat"] == "phase")
            .map(|event| (event["name"].as_str().unwrap(), span(event)))
            .collect();
        assert_eq!(
            phases,
            vec![
                ("render", (1_000_000.0, 1_008_000.0)),
                ("layout", (1_008_000.0, 1_013_000.0)),
                ("prepaint", (1_013_000.0, 1_015_000.0)),
                ("paint", (1_015_000.0, 1_018_000.0)),
                ("inspector", (1_018_000.0, 1_019_000.0)),
                ("present", (1_019_000.0, 1_020_000.0)),
            ]
        );
        let present = find(&trace, "X", "present")[0];
        assert_eq!(present["args"]["clipped"], true);
        assert!(
            events(&trace)
                .iter()
                .filter(|event| event["cat"] == "phase")
                .all(|phase| contains(frame_event, phase))
        );
    }

    #[test]
    fn views_nest_by_time_on_their_lane() {
        let trace = parse(&[busy_frame()], &[]);
        let workspace = find(&trace, "X", "Workspace")[0];
        let list = find(&trace, "X", "IssueList<Row>")[0];
        let sidebar = find(&trace, "X", "Sidebar")[0];
        assert!(contains(workspace, list) && contains(workspace, sidebar));
        assert_eq!(list["tid"], VIEWS_LANE);
        assert_eq!(list["args"]["type"], "app::IssueList<app::Row>");
        assert_eq!(sidebar["args"]["outcome"], "cached");
        assert_eq!(list["args"]["entity"], entity(2).as_u64());
        assert_eq!(
            find(&trace, "X", "Frame #7")[0]["args"]["replayed_app"],
            false
        );
    }

    #[test]
    fn a_frame_that_replayed_the_app_says_so() {
        let mut record = frame(8, ms(1_030.0), ms(0.5));
        record.views = vec![replayed_view(1, "app::Workspace", 0, ms(0.1))];
        let trace = parse(&[record], &[]);
        assert_eq!(
            find(&trace, "X", "Frame #8")[0]["args"]["replayed_app"],
            true
        );
        let workspace = find(&trace, "X", "Workspace")[0];
        assert_eq!(workspace["args"]["outcome"], "replayed");
        assert_eq!(workspace["dur"], 0.0);
    }

    #[test]
    fn user_spans_and_main_thread_work_have_sites() {
        let trace = parse(&[busy_frame()], &[]);
        let filter = find(&trace, "X", "filter issues")[0];
        assert_eq!(filter["tid"], USER_SPANS_LANE);
        assert_eq!(span(filter), (1_002_000.0, 1_005_000.0));
        assert!(
            filter["args"]["site"]
                .as_str()
                .unwrap()
                .starts_with("gpui_inspector/analysis/trace.rs:")
        );
        let task = events(&trace)
            .iter()
            .find(|event| event["cat"] == "main")
            .unwrap();
        assert_eq!(task["tid"], MAIN_THREAD_LANE);
        assert!(task["name"].as_str().unwrap().starts_with("Task trace.rs:"));
        assert_eq!(span(task), (990_000.0, 996_000.0));
    }

    #[test]
    fn causes_flow_into_the_frame_they_caused() {
        let trace = parse(&[busy_frame()], &[]);
        let marker = find(&trace, "X", "notify IssueStore")[0];
        assert_eq!(marker["tid"], CAUSES_LANE);
        assert_eq!(marker["ts"], 996_000.0);
        assert_eq!(marker["args"]["entity"], entity(9).as_u64());

        let start = find(&trace, "s", "caused")[0];
        let finish = find(&trace, "f", "caused")[0];
        assert_eq!(start["id"], finish["id"]);
        assert_eq!(start["tid"], CAUSES_LANE);
        assert_eq!(start["ts"], marker["ts"]);
        assert_eq!(finish["tid"], FRAMES_LANE);
        assert_eq!(finish["bp"], "e");
        assert_eq!(finish["ts"], find(&trace, "X", "Frame #7")[0]["ts"]);
    }

    #[test]
    fn input_is_an_instant_on_the_main_thread() {
        let mut record = input(3, ms(995.0), InputKind::MouseDown, Some(7));
        record.actions.push(ActionRecord {
            name: "app::Open",
            handled: true,
            keystrokes: None,
            context: None,
        });
        let trace = parse(&[busy_frame()], &[record]);
        let instant = find(&trace, "i", "MouseDown")[0];
        assert_eq!(instant["s"], "t");
        assert_eq!(instant["tid"], MAIN_THREAD_LANE);
        assert_eq!(instant["ts"], 995_000.0);
        assert_eq!(instant["args"]["frame"], 7);
        assert_eq!(instant["args"]["actions"][0]["name"], "app::Open");
    }

    #[test]
    fn events_are_sorted_by_time_after_metadata() {
        let mut second = busy_frame();
        second.id = 8;
        second.start = ms(1_030.0);
        second.inspector_only = true;
        let trace = parse(&[busy_frame(), second], &[]);
        let timed: Vec<f64> = events(&trace)
            .iter()
            .skip_while(|event| event["ph"] == "M")
            .map(|event| event["ts"].as_f64().unwrap())
            .collect();
        assert!(timed.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(
            events(&trace)
                .iter()
                .skip_while(|event| event["ph"] == "M")
                .all(|event| event["ph"] != "M")
        );
        assert_eq!(find(&trace, "X", "Frame #8 (Loupe)").len(), 1);
        // Flow ids are unique per cause.
        let ids: Vec<&Value> = find(&trace, "s", "caused")
            .iter()
            .map(|e| &e["id"])
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn extreme_durations_export_without_panicking() {
        let mut record = frame(0, Duration::ZERO, Duration::from_secs(86_400 * 365));
        record.causes = vec![RenderCause {
            kind: CauseKind::Initial,
            site: None,
            before_frame: Duration::MAX,
            from_inspector: true,
        }];
        let trace = parse(&[record], &[]);
        assert_eq!(find(&trace, "X", "initial")[0]["ts"], 0.0);
        assert!(find(&trace, "X", "Frame #0")[0]["dur"].as_f64().unwrap() > 0.0);
    }
}
