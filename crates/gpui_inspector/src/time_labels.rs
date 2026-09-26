//! Labels read against the capture's clock: `2.1 s ago`, `idle`.
//!
//! A view draws such labels through its [`TimeLabels`], which remembers how
//! each one read. About once a second of capture time ([`TICK`]), Loupe asks
//! the shell and the lens on screen whether a label they drew would read
//! differently now, and re-renders only those that would. A frame title that
//! says `3 min ago` re-renders its lens once a minute, and a screen without
//! such labels wakes nothing.

use crate::analysis::{format, stats::FPS_WINDOW};
use std::{
    cell::{Cell, RefCell},
    time::Duration,
};

/// How often Loupe re-reads the labels on screen.
pub(crate) const TICK: Duration = Duration::from_secs(1);

/// One label drawn against the capture's clock.
#[derive(Clone, Debug, PartialEq)]
enum TimeLabel {
    /// How long before now `then` was, and how that read.
    Ago { then: Duration, text: String },
    /// Something drawn reads differently from this moment on.
    Until(Duration),
}

/// The labels a view drew against the capture's clock in its latest render.
#[derive(Debug, Default)]
pub(crate) struct TimeLabels {
    now: Cell<Duration>,
    drawn: RefCell<Vec<TimeLabel>>,
}

impl TimeLabels {
    /// Starts a render at `now`, on the capture's clock: forgets the labels
    /// drawn before.
    pub fn begin(&self, now: Duration) {
        self.now.set(now);
        self.drawn.borrow_mut().clear();
    }

    /// The now the render reads labels at.
    pub fn now(&self) -> Duration {
        self.now.get()
    }

    /// `2.1 s ago`: how long before now `then` was.
    pub fn ago(&self, then: Duration) -> String {
        let text = format::relative_time(self.now(), then);
        self.drawn.borrow_mut().push(TimeLabel::Ago {
            then,
            text: text.clone(),
        });
        text
    }

    /// The app's live frame rate, `118`, until the app has not drawn for
    /// [`FPS_WINDOW`] since its latest frame started: then `None`, idle.
    pub fn fps(&self, fps: f64, latest_app_start: Option<Duration>) -> Option<String> {
        let idle_at = latest_app_start? + FPS_WINDOW;
        (self.now() < idle_at).then(|| {
            self.drawn.borrow_mut().push(TimeLabel::Until(idle_at));
            format!("{fps:.0}")
        })
    }

    /// Whether a label drawn would read differently at `now`.
    pub fn stale(&self, now: Duration) -> bool {
        self.drawn.borrow().iter().any(|label| match label {
            TimeLabel::Ago { then, text } => format::relative_time(now, *then) != *text,
            TimeLabel::Until(at) => now >= *at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::ms;

    #[test]
    fn labels_go_stale_only_when_they_would_read_differently() {
        let labels = TimeLabels::default();
        assert!(!labels.stale(ms(90_000.)), "no labels, never stale");

        labels.begin(ms(62_000.));
        assert_eq!(labels.ago(ms(0.)), "1 min ago");
        assert!(!labels.stale(ms(62_000.)));
        assert!(!labels.stale(ms(119_000.)), "still a minute ago");
        assert!(labels.stale(ms(120_000.)));

        labels.begin(ms(3_000.));
        assert_eq!(labels.ago(ms(900.)), "2.1 s ago");
        assert!(labels.stale(ms(4_000.)), "3.1 s ago now");
        labels.begin(ms(4_000.));
        assert!(!labels.stale(ms(90_000.)), "a new render forgets them");
    }

    #[test]
    fn the_frame_rate_turns_idle_a_second_after_the_latest_frame() {
        let labels = TimeLabels::default();
        labels.begin(ms(1_500.));
        assert_eq!(labels.fps(59.6, Some(ms(1_000.))).as_deref(), Some("60"));
        assert!(!labels.stale(ms(1_999.)));
        assert!(labels.stale(ms(2_000.)));

        labels.begin(ms(2_000.));
        assert_eq!(labels.fps(59.6, Some(ms(1_000.))), None, "idle");
        assert_eq!(labels.fps(0., None), None, "nothing drawn yet");
        assert!(!labels.stale(ms(90_000.)), "idle stays idle");
    }
}
