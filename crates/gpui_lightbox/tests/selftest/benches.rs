//! Benchmarks: real frame timings with history, and a synthetic regression
//! that must be flagged.

use crate::{
    card_config, scratch_dir,
    views::{Card, Flaw, ScrollList},
};
use gpui::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window,
    div, point, px,
};
use gpui_lightbox::{
    BenchSpec, StageConfig, bench,
    bench::{BenchHistory, Verdict},
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    time::Duration,
};

/// A view whose render takes at least `delay`: a regression on demand.
struct Heavy {
    delay: Duration,
}

impl Render for Heavy {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        std::thread::sleep(self.delay);
        div().size_full().child("Heavy")
    }
}

#[test]
fn history_appends_and_flags_a_synthetic_regression() {
    let spec = BenchSpec {
        warmup: 2,
        iterations: 12,
        phases: false,
        stage: StageConfig::default().size(320., 120.),
        history_dir: Some(scratch_dir("bench-history")),
        ..BenchSpec::default()
    };
    let run = |delay: Duration| {
        bench("synthetic", spec.clone(), move |stage| {
            stage.mount(move |_, cx| cx.new(|_| Heavy { delay }));
            |_: &mut Window, _: &mut App| {}
        })
    };

    let baseline = run(Duration::ZERO);
    assert_eq!(baseline.run.stats.count, 12);
    assert!(
        baseline.run.comparison.is_none(),
        "the first run has nothing to compare with"
    );
    assert_ne!(baseline.run.run.git_sha, "unknown");
    assert!(["debug", "release"].contains(&baseline.run.run.profile.as_str()));

    let slow = run(Duration::from_millis(4));
    let comparison = slow
        .run
        .comparison
        .as_ref()
        .expect("compared with the baseline");
    assert_eq!(comparison.verdict, Verdict::Regression, "{slow}");
    assert!(slow.run.stats.p50 >= 4., "{slow}");
    let panic = catch_unwind(AssertUnwindSafe(|| {
        slow.assert_no_regression();
    }))
    .expect_err("the regression is reported");
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("regressed: p50 +"), "{message}");

    let history = BenchHistory::load(&slow.history).expect("the history is JSON");
    assert_eq!(history.runs.len(), 2);
    assert_eq!(history.runs[0].run, baseline.run.run);
    assert_eq!(history.runs[1].samples.len(), slow.run.samples.len());
    assert_eq!(
        history.runs[1]
            .comparison
            .as_ref()
            .map(|comparison| comparison.verdict),
        Some(Verdict::Regression)
    );
}

#[test]
fn bench_card_full_frame() {
    let spec = BenchSpec {
        stage: card_config(),
        ..BenchSpec::default()
    };
    let report = bench("card/full-frame", spec, |stage| {
        stage.mount(|_, cx| cx.new(|_| Card::new(Flaw::None)));
        |_: &mut Window, _: &mut App| {}
    });
    println!("{report}");
    let stats = report.run.stats;
    assert_eq!(stats.count, 100);
    assert!(stats.min > 0. && stats.min <= stats.p50 && stats.p50 <= stats.p99);
}

#[test]
fn bench_list_scroll() {
    let spec = BenchSpec {
        refresh: false,
        stage: StageConfig::default().size(328., 226.),
        ..BenchSpec::default()
    };
    let report = bench("list/scroll", spec, |stage| {
        stage.mount(|_, cx| cx.new(|_| ScrollList));
        let mut down = true;
        move |window: &mut Window, cx: &mut App| {
            // Scroll a row at a time, back and forth: only the list re-renders.
            let delta = if down { -22. } else { 22. };
            down = !down;
            window.dispatch_event(
                gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                    position: point(px(160.), px(100.)),
                    delta: gpui::ScrollDelta::Pixels(point(px(0.), px(delta))),
                    ..Default::default()
                }),
                cx,
            );
        }
    });
    println!("{report}");
    assert_eq!(report.run.stats.count, 100);
}
