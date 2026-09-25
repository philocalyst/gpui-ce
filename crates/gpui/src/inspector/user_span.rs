//! User spans: named timings that app code records into the inspector's
//! frames. See [`span`] and [`inspector_span!`](crate::inspector_span).

use crate::SharedString;

/// Opens a span named `name` at the caller's location, ending when the
/// returned guard is dropped. Spans show in the inspector's flame chart next
/// to the view renders of the same frame, so they explain where a slow
/// render spent its time.
///
/// ```ignore
/// fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
///     let rows = gpui::inspector_span!("filter rows", self.filtered_rows());
///     let _build = gpui::inspector::span("build list");
///     div().children(rows.iter().map(|row| self.render_row(row)))
/// }
/// ```
///
/// A span is recorded (into [`FrameRecord::spans`](crate::inspector::FrameRecord::spans))
/// when it is opened while a window whose inspector is open draws a frame;
/// spans still open when the frame ends are closed at its end. Anywhere else,
/// opening a span costs one thread-local check and records nothing, and spans
/// opened while the inspector draws its own UI are ignored. Spans nest: each
/// records its depth among the spans open around it.
#[track_caller]
#[must_use = "the span ends when the guard is dropped"]
pub fn span(name: impl Into<SharedString>) -> SpanGuard {
    #[cfg(any(feature = "inspector", debug_assertions))]
    {
        SpanGuard {
            open: recording::open(name, std::panic::Location::caller()),
        }
    }
    #[cfg(not(any(feature = "inspector", debug_assertions)))]
    {
        let _ = name;
        SpanGuard {}
    }
}

/// Keeps a span opened by [`span`] open; dropping it ends the span.
pub struct SpanGuard {
    #[cfg(any(feature = "inspector", debug_assertions))]
    open: Option<recording::OpenSpan>,
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        #[cfg(any(feature = "inspector", debug_assertions))]
        if let Some(open) = self.open.take() {
            recording::close(open);
        }
    }
}

/// Records a named span for the inspector's flame chart, sited at the macro
/// call.
///
/// `inspector_span!("name")` spans the rest of the enclosing block;
/// `inspector_span!("name", expr)` spans only `expr` and evaluates to its
/// value. See [`gpui::inspector::span`](crate::inspector::span) for when
/// spans are recorded and what they cost.
#[macro_export]
macro_rules! inspector_span {
    ($name:expr) => {
        let _inspector_span = $crate::inspector::span($name);
    };
    ($name:expr, $body:expr) => {{
        let _inspector_span = $crate::inspector::span($name);
        $body
    }};
}

#[cfg(any(feature = "inspector", debug_assertions))]
pub(crate) mod recording {
    //! The thread-local sink spans are recorded into while a frame is drawn.

    use crate::{SharedString, inspector::UserSpan};
    use scheduler::Instant;
    use std::{
        cell::{Cell, RefCell},
        mem,
        panic::Location,
    };

    thread_local! {
        /// True while spans opened on this thread are recorded; the only thing
        /// [`open`] reads when nothing is recording.
        static RECORDING: Cell<bool> = const { Cell::new(false) };
        static SINK: RefCell<Sink> = RefCell::new(Sink::default());
    }

    #[derive(Default)]
    struct Sink {
        /// Distinguishes frames, so a guard outliving its frame is ignored.
        frame: u64,
        start: Option<Instant>,
        depth: u16,
        suspended: bool,
        spans: Vec<UserSpan>,
        /// Parallel to `spans`: whether each span's guard was dropped.
        closed: Vec<bool>,
    }

    /// A span recorded in the current frame, to be closed by its guard.
    pub(crate) struct OpenSpan {
        frame: u64,
        index: usize,
    }

    pub(super) fn open(
        name: impl Into<SharedString>,
        site: &'static Location<'static>,
    ) -> Option<OpenSpan> {
        if !RECORDING.get() {
            return None;
        }
        SINK.with_borrow_mut(|sink| {
            let start = sink.start?;
            let index = sink.spans.len();
            sink.spans.push(UserSpan {
                name: name.into(),
                site,
                depth: sink.depth,
                start: Instant::now().saturating_duration_since(start),
                duration: Default::default(),
            });
            sink.closed.push(false);
            sink.depth += 1;
            Some(OpenSpan {
                frame: sink.frame,
                index,
            })
        })
    }

    pub(super) fn close(open: OpenSpan) {
        SINK.with_borrow_mut(|sink| {
            let Some(start) = sink.start else { return };
            if sink.frame != open.frame {
                return;
            }
            if let Some(span) = sink.spans.get_mut(open.index) {
                span.duration = Instant::now()
                    .saturating_duration_since(start)
                    .saturating_sub(span.start);
                sink.closed[open.index] = true;
                sink.depth = sink.depth.saturating_sub(1);
            }
        })
    }

    /// Starts recording spans for a frame that began at `start`.
    pub(crate) fn begin_frame(start: Instant) {
        SINK.with_borrow_mut(|sink| {
            sink.frame += 1;
            sink.start = Some(start);
            sink.depth = 0;
            sink.suspended = false;
            sink.spans.clear();
            sink.closed.clear();
        });
        RECORDING.set(true);
    }

    /// Pauses or resumes recording while the inspector draws its own UI.
    pub(crate) fn set_suspended(suspended: bool) {
        let recording = SINK.with_borrow_mut(|sink| {
            sink.suspended = suspended;
            sink.start.is_some() && !suspended
        });
        RECORDING.set(recording);
    }

    /// Stops recording and returns the frame's spans; spans still open are
    /// closed at the frame's end.
    pub(crate) fn end_frame() -> Vec<UserSpan> {
        RECORDING.set(false);
        SINK.with_borrow_mut(|sink| {
            let Some(start) = sink.start.take() else {
                return Vec::new();
            };
            let end = Instant::now().saturating_duration_since(start);
            let mut spans = mem::take(&mut sink.spans);
            for (span, closed) in spans.iter_mut().zip(sink.closed.drain(..)) {
                if !closed {
                    span.duration = end.saturating_sub(span.start);
                }
            }
            spans
        })
    }
}
