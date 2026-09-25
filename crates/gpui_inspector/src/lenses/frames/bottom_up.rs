//! The bottom-up table: where render time went, by view type, for the shown
//! frame or for every app frame in the ring.

use crate::{
    analysis::{bottom_up::BottomUpRow, format, stats},
    theme::{MONO_FONT, Phase, Theme},
    widgets::{self, ColumnWidth, Tooltip},
};
use gpui::{
    AnyElement, IntoElement, Styled, div, inspector::FrameRecord, prelude::*, px, relative,
};
use std::{cmp::Ordering, collections::VecDeque, rc::Rc, time::Duration};

/// Width of the share bar in the self time cells.
const SHARE_BAR: f32 = 24.;

/// Which frames the table aggregates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The shown frame.
    #[default]
    Frame,
    /// Every app frame in the ring.
    All,
}

/// A column of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Column {
    /// The view type.
    Type,
    /// Spans of the type.
    Calls,
    /// Rendered / cached.
    Outcomes,
    /// Inclusive time.
    Total,
    /// Time excluding nested views.
    SelfTime,
    /// Mean span.
    Mean,
    /// Longest span.
    Max,
}

/// The columns that fit: all of them in wide panes, the essentials in
/// narrow ones.
pub(crate) fn columns(wide: bool) -> Rc<[Column]> {
    if wide {
        Rc::new([
            Column::Type,
            Column::Calls,
            Column::Outcomes,
            Column::Total,
            Column::SelfTime,
            Column::Mean,
            Column::Max,
        ])
    } else {
        Rc::new([
            Column::Type,
            Column::Calls,
            Column::Outcomes,
            Column::Total,
            Column::SelfTime,
        ])
    }
}

/// The table widget's definition of `column`.
pub(crate) fn table_column(column: Column) -> widgets::Column {
    let numeric = |title: &'static str, width: f32| {
        widgets::Column::new(title)
            .numeric()
            .width(ColumnWidth::Fixed(px(width)))
    };
    match column {
        Column::Type => widgets::Column::new("View type"),
        Column::Calls => numeric("Calls", 46.),
        Column::Outcomes => numeric("R / C", 54.).unsortable(),
        Column::Total => numeric("Total", 62.),
        Column::SelfTime => numeric("Self", 60. + SHARE_BAR),
        Column::Mean => numeric("Mean", 58.),
        Column::Max => numeric("Max", 58.),
    }
}

/// Orders two rows by `column`, ascending.
pub(crate) fn compare(a: &BottomUpRow, b: &BottomUpRow, column: Column) -> Ordering {
    match column {
        Column::Type => format::type_name(a.type_name).cmp(&format::type_name(b.type_name)),
        Column::Calls => a.calls.cmp(&b.calls),
        Column::Outcomes => a.rendered.cmp(&b.rendered),
        Column::Total => a.total.cmp(&b.total),
        Column::SelfTime => a.self_time.cmp(&b.self_time),
        Column::Mean => a.mean.cmp(&b.mean),
        Column::Max => a.max.cmp(&b.max),
    }
}

/// The frames `scope` aggregates: the shown frame, or every app frame.
pub(crate) fn scope_frames<'a>(
    frames: &'a VecDeque<FrameRecord>,
    shown: &'a FrameRecord,
    scope: Scope,
) -> Vec<&'a FrameRecord> {
    match scope {
        Scope::Frame => vec![shown],
        Scope::All => frames
            .iter()
            .filter(|frame| !frame.inspector_only)
            .collect(),
    }
}

/// The largest self time among `rows`, for the cells' proportion bars.
pub(crate) fn max_self(rows: &[BottomUpRow]) -> Duration {
    rows.iter()
        .map(|row| row.self_time)
        .max()
        .unwrap_or_default()
}

/// One cell of `row`.
pub(crate) fn cell(
    row: &BottomUpRow,
    column: Column,
    max_self: Duration,
    theme: &Theme,
) -> AnyElement {
    let colors = &theme.colors;
    let mono = |text: String| {
        div()
            .font_family(MONO_FONT)
            .text_size(theme.metrics.mono)
            .child(text)
    };
    match column {
        Column::Type => div()
            .id(row.type_name)
            .min_w_0()
            .truncate()
            .child(format::type_name(row.type_name).into_owned())
            .tooltip(Tooltip::text(row.type_name))
            .into_any_element(),
        Column::Calls => mono(format::count(u64::from(row.calls))).into_any_element(),
        Column::Outcomes => div()
            .flex()
            .gap(px(4.))
            .font_family(MONO_FONT)
            .text_size(theme.metrics.mono)
            .child(format::count(u64::from(row.rendered)))
            .child(div().text_color(colors.text_faint).child("/"))
            .child(
                div()
                    .text_color(colors.text_muted)
                    .child(format::count(u64::from(row.cached))),
            )
            .into_any_element(),
        Column::Total => mono(format::duration(row.total)).into_any_element(),
        Column::SelfTime => {
            // A small bar of its share of the largest self time, then the number.
            let share = stats::ratio(row.self_time, max_self) as f32;
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .flex_none()
                        .w(px(SHARE_BAR))
                        .h(px(4.))
                        .bg(colors.surface_2)
                        .child(
                            div()
                                .h_full()
                                .w(relative(share))
                                .bg(theme.phase(Phase::Render)),
                        ),
                )
                .child(mono(format::duration(row.self_time)))
                .into_any_element()
        }
        Column::Mean => mono(format::duration(row.mean)).into_any_element(),
        Column::Max => mono(format::duration(row.max)).into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        analysis::bottom_up::bottom_up,
        fixtures::{FrameBuilder, ms},
    };
    use gpui::inspector::ViewOutcome;

    fn frame(id: u64, list_ms: f64) -> FrameRecord {
        let mut frame = FrameBuilder::new()
            .at(ms(id as f64 * 16.))
            .app_time(ms(list_ms + 2.), ms(0.2))
            .view(
                1,
                "app::App",
                0,
                ms(0.),
                ms(list_ms + 1.),
                ViewOutcome::Rendered,
            )
            .view(
                2,
                "app::List",
                1,
                ms(0.5),
                ms(list_ms),
                ViewOutcome::Rendered,
            )
            .view(
                3,
                "app::Side",
                1,
                ms(list_ms + 0.5),
                ms(0.02),
                ViewOutcome::Cached,
            )
            .build();
        frame.id = id;
        frame
    }

    #[test]
    fn scope_picks_the_frame_or_every_app_frame() {
        let mut frames: VecDeque<FrameRecord> = [frame(0, 4.), frame(1, 30.)].into();
        let mut loupe = frame(2, 1.);
        loupe.inspector_only = true;
        frames.push_back(loupe);
        let shown = &frames[1];
        let one = bottom_up(scope_frames(&frames, shown, Scope::Frame));
        assert_eq!(one[0].type_name, "app::List");
        assert_eq!(one[0].self_time, ms(30.));
        let all = bottom_up(scope_frames(&frames, shown, Scope::All));
        assert_eq!(all[0].calls, 2, "Loupe's frame is left out");
        assert_eq!(all[0].self_time, ms(34.));
        assert_eq!(max_self(&all), ms(34.));
    }

    #[test]
    fn columns_compare_what_they_show() {
        let frames: VecDeque<FrameRecord> = [frame(0, 4.)].into();
        let rows = bottom_up(scope_frames(&frames, &frames[0], Scope::Frame));
        let by = |column| {
            let mut sorted: Vec<&BottomUpRow> = rows.iter().collect();
            sorted.sort_by(|a, b| compare(a, b, column));
            sorted.iter().map(|row| row.type_name).collect::<Vec<_>>()
        };
        assert_eq!(by(Column::Type), ["app::App", "app::List", "app::Side"]);
        assert_eq!(by(Column::SelfTime), ["app::Side", "app::App", "app::List"]);
        assert_eq!(by(Column::Total), ["app::Side", "app::List", "app::App"]);
        assert_eq!(columns(true).len(), 7);
        assert!(!columns(false).contains(&Column::Mean));
    }
}
