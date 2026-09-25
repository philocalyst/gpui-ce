//! Rendering of the Frames lens: its sections and how they are laid out.

use super::{
    CONTEXT, Columns, FollowLatest, FramesLens, JumpToWorst, NextFrame, Notice, PreviousFrame,
    SUMMARY_MAX, SUMMARY_MIN, SUMMARY_SHARE, TABLE_ROWS, ZoomIn, ZoomOut, ZoomToFit,
    bottom_up::{Scope, cell, columns, compare, max_self, scope_frames, table_column},
    export::PERFETTO_URL,
    flame::{BarDetails, FlameData, Geometry, bar_color, bar_details, kind_name, legend},
    model::{
        StatsLine, capture_now, cause_lines, frame_title, grade_pill, input_summary, shown_frame,
    },
    over_budget_badge,
    phase_bar::{PhaseBar, PhaseBarView},
};
use crate::{
    analysis::{
        bottom_up::{BottomUpRow, bottom_up},
        format,
        insights::{Insight, InsightInput, insights},
    },
    lenses::{
        LensView, RailBadge,
        capture::budget_control,
        links::{Navigate, Target, link, site_link},
    },
    settings::FrameBudget,
    state::Lens,
    theme::{MONO_FONT, Theme, UI_FONT},
    widgets::{
        Button, ButtonSize, ButtonStyle, EmptyState, Icon, IconName, Pill, SectionHeader, Segment,
        Segmented, Table, Tone, Tooltip, floating_surface,
    },
};
use gpui::{
    AnyElement, App, Context, DispatchPhase, FontWeight, IntoElement, MouseButton, MouseUpEvent,
    Render, Styled, Window, anchored, canvas, deferred, div,
    inspector::{FrameRecord, InspectorCapture},
    point,
    prelude::*,
    px, relative,
};
use std::{rc::Rc, time::Duration};

/// Everything one render shows, derived from the capture.
struct Shown<'a> {
    capture: &'a InspectorCapture,
    frame: &'a FrameRecord,
    pinned: bool,
    now: Duration,
    budget: Duration,
    stats: Rc<StatsLine>,
    rows: Rc<Vec<BottomUpRow>>,
    insights: Rc<Vec<Insight>>,
}

impl Render for FramesLens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let root = div()
            .id("loupe-frames")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .bg(colors.bg)
            .font_family(UI_FONT)
            .text_size(theme.metrics.text)
            .line_height(theme.metrics.line_height)
            .text_color(colors.text)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &PreviousFrame, window, cx| this.step(-1, window, cx)))
            .on_action(cx.listener(|this, _: &NextFrame, window, cx| this.step(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &JumpToWorst, window, cx| this.jump_to_worst(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FollowLatest, _, cx| this.select_frame(None, cx)))
            .on_action(
                cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(Some(super::flame::KEY_ZOOM), cx)),
            )
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                this.zoom(Some(1. / super::flame::KEY_ZOOM), cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| this.zoom(None, cx)));

        let window_ref: &Window = window;
        let layout = Columns::of(window_ref);
        let Some(shown) = window_ref
            .inspector_capture()
            .and_then(|capture| self.derive(capture, window_ref, layout, cx))
        else {
            self.idle_check = None;
            return root.child(
                EmptyState::new("No app frames recorded yet")
                    .icon(IconName::Pause)
                    .description(format!(
                        "{} Use the app: every frame it draws lands here, with why it was \
                         drawn and where its time went.",
                        Lens::Frames.question()
                    )),
            );
        };
        let generation = shown.capture.generation();
        let idle_in = shown.stats.idle_in(shown.now);
        let navigate = self.navigator(cx);
        let tooltip = self.render_tooltip(&shown, theme);

        let caption = div()
            .px(theme.metrics.gutter)
            .pt(px(8.))
            .text_size(theme.metrics.text_small)
            .text_color(colors.text_faint)
            .child(Lens::Frames.question());
        let summary = vec![
            caption.into_any_element(),
            self.render_stats(&shown, theme),
            self.render_frame(&shown, theme, cx),
            self.render_causes(&shown, &navigate, theme),
        ];
        let flame = self.render_flame(&shown, &navigate, theme, cx);
        let bottom_up = self.render_bottom_up(&shown, layout.wide_table(), theme, cx);
        let insights = self.render_insights(&shown, &navigate, theme);
        let export = self.render_export(theme, cx);
        let body = if layout.two {
            // The frame's story on the left; where its time went on the right.
            div()
                .size_full()
                .flex()
                .child(
                    div()
                        .id("loupe-frames-summary")
                        .flex_none()
                        .w(relative(SUMMARY_SHARE))
                        .min_w(SUMMARY_MIN)
                        .max_w(SUMMARY_MAX)
                        .h_full()
                        .overflow_y_scroll()
                        .border_r_1()
                        .border_color(colors.line)
                        .pb_4()
                        .children(summary)
                        .child(export),
                )
                .child(
                    div()
                        .id("loupe-frames-detail")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .pb_4()
                        // The first section's rule would double the pane's edge.
                        .child(div().mt(px(-1.)).child(flame))
                        .child(bottom_up)
                        .child(insights),
                )
        } else {
            div().size_full().child(
                div()
                    .id("loupe-frames-column")
                    .size_full()
                    .overflow_y_scroll()
                    .pb_4()
                    .children(summary)
                    .child(flame)
                    .child(bottom_up)
                    .child(insights)
                    .child(export),
            )
        };
        self.schedule_idle_check(generation, idle_in, window, cx);
        root.child(body).children(tooltip)
    }
}

impl LensView for FramesLens {
    fn rail_badge(&self, window: &Window, _cx: &App) -> Option<RailBadge> {
        let capture = window.inspector_capture()?;
        let key = (capture.generation(), capture.config().budget);
        self.badge
            .borrow_mut()
            .get(key, || over_budget_badge(capture))
            .as_ref()
            .clone()
    }
}

impl FramesLens {
    /// Refreshes the memoized data for the shown frame: the selection, or
    /// the latest app frame. `None` before the app has drawn anything.
    fn derive<'a>(
        &mut self,
        capture: &'a InspectorCapture,
        window: &Window,
        layout: Columns,
        cx: &mut Context<Self>,
    ) -> Option<Shown<'a>> {
        let generation = capture.generation();
        let budget = capture.config().budget;
        let selected = self.state.read(cx).selected_frame();
        let (frame, pinned) = shown_frame(capture, selected)?;
        let stats = self
            .stats
            .get((generation, budget), || StatsLine::of(capture));
        let data = self.flame_data.get((generation, frame.id, budget), || {
            FlameData::new(frame, budget)
        });
        self.flame.show(data);
        let scope = self.scope;
        let rows = self.rows.get((generation, frame.id, scope), || {
            bottom_up(scope_frames(capture.frames(), frame, scope))
        });
        self.sync_table(&rows, layout.wide_table(), cx);
        let focus = pinned.then_some(frame.id);
        let insights = self.insights.get((generation, focus, budget), || {
            let entities = window.inspector_entities(cx);
            insights(&InsightInput {
                frames: capture.frames(),
                input: capture.input(),
                notify: capture.notify_stats(),
                entities: &entities,
                selected_frame: focus,
                budget,
            })
        });
        Some(Shown {
            capture,
            frame,
            pinned,
            now: capture_now(capture),
            budget,
            stats,
            rows,
            insights,
        })
    }

    /// Hands the table new rows when the memo recomputed them.
    fn sync_table(&mut self, rows: &Rc<Vec<BottomUpRow>>, wide: bool, cx: &mut Context<Self>) {
        if self
            .table_rows
            .as_ref()
            .is_some_and(|shown| Rc::ptr_eq(shown, rows) && self.table_wide == wide)
        {
            return;
        }
        self.table_rows = Some(rows.clone());
        self.table_wide = wide;
        let keys = rows.iter().map(|row| row.type_name).collect();
        let data = rows.clone();
        let columns = columns(wide);
        self.table.update(cx, |table, cx| {
            table.set_rows(
                keys,
                move |column, a, b| compare(&data[a], &data[b], columns[column]),
                cx,
            )
        });
    }

    fn render_stats(&self, shown: &Shown, theme: &'static Theme) -> AnyElement {
        let colors = &theme.colors;
        let stats = &shown.stats;
        let label = |text: &'static str| div().text_color(colors.text_muted).child(text);
        let value = |text: String| {
            div()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.text)
                .text_color(colors.text)
                .child(text)
        };
        let fps = stats.fps_text(shown.now);
        let idle = fps == "idle";
        let over_tone = if stats.grades.crit > 0 {
            Tone::Crit
        } else if stats.grades.warn > 0 {
            Tone::Warn
        } else {
            Tone::Ok
        };
        let [ok, warn, crit] = stats.grade_shares();
        let grade_bar = div()
            .flex_none()
            .w(px(64.))
            .h(px(4.))
            .overflow_hidden()
            .flex()
            .bg(colors.surface_2)
            .child(div().h_full().w(relative(ok)).bg(colors.ok))
            .child(div().h_full().w(relative(warn)).bg(colors.warn))
            .child(div().h_full().w(relative(crit)).bg(colors.crit));
        div()
            .id("loupe-frames-stats")
            .debug_selector(|| "loupe-frames-stats".into())
            .px(theme.metrics.gutter)
            .pt(px(4.))
            .pb(px(8.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap_x(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(4.))
                            .child(
                                div()
                                    .font_family(MONO_FONT)
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if idle { colors.text_muted } else { colors.text })
                                    .child(fps),
                            )
                            .when(!idle, |this| this.child(label("fps"))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(4.))
                            .child(label("p50"))
                            .child(value(format::millis(stats.p50)))
                            .child(label("p95"))
                            .child(value(format::millis(stats.p95)))
                            .child(label("p99"))
                            .child(value(format::millis(stats.p99)))
                            .child(label("ms")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(12.))
                    .gap_y(px(4.))
                    .text_size(theme.metrics.text_small)
                    .child(
                        div()
                            .id("loupe-frames-grades")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(grade_bar)
                            .child(
                                div()
                                    .text_color(over_tone.color(theme))
                                    .child(stats.over_budget_text()),
                            )
                            .tooltip(Tooltip::text(format!(
                                "{} within budget · {} over · {} over 1.5× budget",
                                stats.grades.ok, stats.grades.warn, stats.grades.crit
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(4.))
                            .child(label("input p95"))
                            .child(value(stats.input_p95.map_or_else(
                                || format::NO_VALUE.to_string(),
                                format::duration,
                            ))),
                    )
                    .child(
                        div()
                            .id("loupe-frames-overhead")
                            .flex()
                            .items_baseline()
                            .gap(px(4.))
                            .child(label("Loupe"))
                            .child(value(format::percent(stats.overhead)))
                            .tooltip(Tooltip::with_meta(
                                "Loupe's share of all drawing",
                                format!(
                                    "Its own drawing plus frames only it caused; {} per frame",
                                    format::duration(stats.loupe_per_frame)
                                ),
                            )),
                    )
                    .child(div().flex_1())
                    .child(budget_control(
                        "loupe-frames-budget",
                        FrameBudget::nearest(shown.budget),
                    )),
            )
            .into_any_element()
    }

    fn render_frame(
        &self,
        shown: &Shown,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        let frame = shown.frame;
        let (tone, grade) = grade_pill(frame.timings.app_total(), shown.budget);
        let header = SectionHeader::new(if shown.pinned {
            "Selected frame"
        } else {
            "Latest frame"
        })
        .rule()
        .when(shown.pinned, |this| {
            this.action(
                Button::new("loupe-frames-latest")
                    .label("Latest")
                    .size(ButtonSize::Small)
                    .tooltip_keys("Follow the latest frame", "l")
                    .on_click(cx.listener(|this, _, _, cx| this.select_frame(None, cx))),
            )
        })
        .action(
            Button::new("loupe-frames-worst")
                .label("Jump to worst")
                .size(ButtonSize::Small)
                .tooltip_keys("Show the slowest frame recorded", "w")
                .on_click(cx.listener(|this, _, window, cx| this.jump_to_worst(window, cx))),
        );
        let title = div()
            .px(theme.metrics.gutter)
            .h(theme.metrics.control)
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                Button::new("loupe-frames-previous")
                    .icon(IconName::ChevronLeft)
                    .size(ButtonSize::Small)
                    .tooltip_keys("Previous frame", "left")
                    .on_click(cx.listener(|this, _, window, cx| this.step(-1, window, cx))),
            )
            .child(
                div()
                    .id("loupe-frames-title")
                    .min_w_0()
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_color(colors.text)
                    .font_weight(FontWeight::BOLD)
                    .child(frame_title(frame, shown.now)),
            )
            .child(Pill::new(grade).tone(tone))
            .when(frame.inspector_only, |this| {
                this.child(Pill::new("Loupe only").tone(Tone::Accent))
            })
            .child(div().flex_1())
            .child(
                Button::new("loupe-frames-next")
                    .icon(IconName::ChevronRight)
                    .size(ButtonSize::Small)
                    .tooltip_keys("Next frame", "right")
                    .on_click(cx.listener(|this, _, window, cx| this.step(1, window, cx))),
            );
        div()
            .child(header)
            .child(title)
            .child(
                div()
                    .id("loupe-phase-bar")
                    .debug_selector(|| "loupe-phase-bar".into())
                    .px(theme.metrics.gutter)
                    .pt(px(4.))
                    .pb(px(8.))
                    .child(
                        PhaseBarView::new(PhaseBar::new(&frame.timings, shown.budget))
                            .replayed(frame.inspector_only),
                    ),
            )
            .into_any_element()
    }

    fn render_causes(
        &self,
        shown: &Shown,
        navigate: &Navigate,
        theme: &'static Theme,
    ) -> AnyElement {
        let colors = &theme.colors;
        let frame = shown.frame;
        let lines = cause_lines(&frame.causes);
        let rows =
            lines.iter().enumerate().map(|(ix, line)| {
                let summary = match line.entity {
                    Some(entity) if !line.from_inspector => link(
                        format!("loupe-cause-{ix}"),
                        line.summary_text(),
                        Target::Entity(entity),
                        navigate,
                        theme,
                    ),
                    _ => div()
                        .flex_none()
                        .px(px(4.))
                        .text_color(colors.text)
                        .child(line.summary_text())
                        .into_any_element(),
                };
                div()
                    .px(theme.metrics.gutter)
                    .min_h(theme.metrics.row)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(4.))
                    .when(line.from_inspector, |this| this.opacity(0.55))
                    .child(div().flex_none().size(px(5.)).mx(px(4.)).rounded_full().bg(
                        if line.from_inspector {
                            colors.text_faint
                        } else {
                            colors.accent
                        },
                    ))
                    .child(summary)
                    .children(line.site.map(|site| {
                        site_link(format!("loupe-cause-site-{ix}"), site, navigate, theme)
                    }))
                    .children(line.before_text().map(|before| {
                        div()
                            .flex_none()
                            .px(px(4.))
                            .text_size(theme.metrics.text_small)
                            .text_color(colors.text_muted)
                            .child(before)
                    }))
                    .when(line.from_inspector, |this| {
                        this.child(Pill::new("Loupe").tone(Tone::Neutral))
                    })
            });
        let input = input_summary(shown.capture.input(), frame.input.clone());
        let input_row =
            div()
                .px(theme.metrics.gutter)
                .min_h(theme.metrics.row)
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(4.))
                .text_size(theme.metrics.text_small)
                .child(
                    div()
                        .flex_none()
                        .pl(px(4.))
                        .text_color(colors.text_muted)
                        .child("Input"),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_color(colors.text)
                        .child(input.text().unwrap_or_else(|| {
                            match (input.loupe, input.evicted) {
                                (0, 0) => "none since the previous frame".to_string(),
                                (_, 0) => "only Loupe's".to_string(),
                                (_, evicted) => format!("{evicted} records, no longer recorded"),
                            }
                        })),
                )
                .children(input.focus.map(|seq| {
                    link(
                        "loupe-frame-input",
                        "Open in Events",
                        Target::Input(seq),
                        navigate,
                        theme,
                    )
                }));
        div()
            .child(
                SectionHeader::new("Why drawn")
                    .detail(format::count(frame.causes.len() as u64))
                    .rule(),
            )
            .when(lines.is_empty(), |this| {
                this.child(
                    div()
                        .px(theme.metrics.gutter)
                        .text_color(colors.text_muted)
                        .child("No cause recorded"),
                )
            })
            .children(rows)
            .child(input_row)
            .child(div().h(px(8.)))
            .into_any_element()
    }

    fn render_flame(
        &self,
        shown: &Shown,
        navigate: &Navigate,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        let Some(data) = self.flame.data().cloned() else {
            return div().into_any_element();
        };
        let zoom = self.flame.zoom();
        let visible = zoom.visible();
        let total = zoom.total();
        let detail = if zoom.is_zoomed() {
            format!(
                "{} of {}",
                format::duration(visible.end - visible.start),
                format::duration(total.end - total.start)
            )
        } else {
            format::duration(total.end - total.start)
        };
        let header = SectionHeader::new("Flame chart")
            .detail(detail)
            .rule()
            .action(
                Button::new("loupe-flame-out")
                    .label("−")
                    .size(ButtonSize::Small)
                    .disabled(!zoom.is_zoomed())
                    .tooltip_keys("Zoom out", "-")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.zoom(Some(1. / super::flame::KEY_ZOOM), cx)
                    })),
            )
            .action(
                Button::new("loupe-flame-in")
                    .label("+")
                    .size(ButtonSize::Small)
                    .tooltip_keys("Zoom in", "=")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.zoom(Some(super::flame::KEY_ZOOM), cx)),
                    ),
            )
            .action(
                Button::new("loupe-flame-fit")
                    .label("Fit")
                    .size(ButtonSize::Small)
                    .disabled(!zoom.is_zoomed())
                    .tooltip_keys("Show the whole frame", "0")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(None, cx))),
            );

        let painter = self.flame.painter(self.emphasized);
        let bounds_cell = self.flame.bounds_cell();
        let dragging = self.flame.is_dragging();
        let chart = div()
            .id("loupe-flame")
            .debug_selector(|| "loupe-flame".into())
            .relative()
            .w_full()
            .h(Geometry::height(data.rows()))
            .when(dragging, |this| this.cursor_grabbing())
            .child(
                canvas(
                    move |bounds, _, _| bounds_cell.set(Some(bounds)),
                    move |bounds, _, window, cx| {
                        if let Some(painter) = &painter {
                            painter.paint(bounds, theme, window, cx);
                        }
                    },
                )
                .size_full(),
            )
            .on_mouse_move(cx.listener(Self::flame_hover))
            .on_mouse_move_all({
                let this = cx.entity().downgrade();
                move |event, phase, _, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        this.update(cx, |this, cx| this.flame_drag(event, window, cx))
                            .ok();
                    }
                }
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::flame_press))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::flame_release))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.flame.cancel_press();
                    cx.notify();
                }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                if !*hovered {
                    this.flame_leave(window, cx);
                }
            }))
            .on_scroll_wheel(cx.listener(Self::flame_wheel));

        let legend_row = div()
            .px(theme.metrics.gutter)
            .pt(px(4.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(8.))
            .gap_y(px(4.))
            .text_size(theme.metrics.text_small)
            .text_color(colors.text_muted)
            .children(legend(&data.layout).into_iter().map(|kind| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(div().size(px(8.)).bg(bar_color(kind, theme)))
                    .child(kind_name(kind))
            }))
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .text_color(colors.text_faint)
                    .child("wheel zooms · drag pans · double-click fits"),
            );

        let replayed = data.replayed.then(|| {
            div()
                .px(theme.metrics.gutter)
                .pb(px(4.))
                .text_size(theme.metrics.text_small)
                .text_color(colors.text_muted)
                .child(
                    "Loupe drew this frame for itself: the app was replayed from its previous \
                     frame, not rendered, so its views show as reused.",
                )
        });
        div()
            .child(header)
            .children(replayed)
            .child(chart)
            .child(legend_row)
            .children(self.render_selected_bar(&data, shown, navigate, theme, cx))
            .child(div().h(px(8.)))
            .into_any_element()
    }

    /// The strip under the chart describing the clicked bar.
    fn render_selected_bar(
        &self,
        data: &FlameData,
        shown: &Shown,
        navigate: &Navigate,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let colors = &theme.colors;
        let bar = self.flame.selected()?;
        let details = bar_details(data, bar..bar + 1)?;
        let flame_bar = data.layout.bars.get(bar)?;
        let element = details
            .view
            .and_then(|_| self.bar_element_in(shown.capture, bar));
        let range = flame_bar.start..flame_bar.end();
        let facts = details
            .facts
            .iter()
            .map(|(label, value)| format!("{} {value}", label.to_lowercase()))
            .collect::<Vec<_>>()
            .join(" · ");
        Some(
            div()
                .mx(theme.metrics.gutter)
                .mt(px(8.))
                .px(px(8.))
                .py(px(4.))
                .rounded(theme.metrics.radius)
                .bg(colors.surface)
                .border_1()
                .border_color(colors.line)
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(8.))
                .gap_y(px(4.))
                .child(div().size(px(8.)).bg(bar_color(flame_bar.kind, theme)))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(details.title.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(theme.metrics.text_small)
                        .text_color(colors.text_muted)
                        .child(facts),
                )
                .children(
                    details.site.map(|(_, site)| {
                        site_link("loupe-flame-selected-site", site, navigate, theme)
                    }),
                )
                .child(div().flex_1())
                .children(element.map(|element| {
                    link(
                        "loupe-flame-reveal",
                        "Reveal in Elements",
                        Target::Element(element),
                        navigate,
                        theme,
                    )
                }))
                .child(
                    Button::new("loupe-flame-zoom-bar")
                        .label("Zoom to bar")
                        .size(ButtonSize::Small)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.flame.show_range(range.clone());
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// The hovered bar's tooltip, next to the pointer.
    fn render_tooltip(&self, shown: &Shown, theme: &'static Theme) -> Option<AnyElement> {
        if self.flame.is_dragging() {
            return None;
        }
        let colors = &theme.colors;
        let (bars, anchor) = self.flame.hovered()?;
        let data = self.flame.data()?;
        let details: BarDetails = bar_details(data, bars.clone())?;
        let embedded = details
            .view
            .and_then(|_| self.bar_element_in(shown.capture, bars.start))
            .and_then(|key| shown.capture.path_info(key.path))
            .map(|info| ("Embedded at", info.source));
        let site = details.site.or(embedded);
        let fact = |label: &'static str, value: String| {
            div()
                .flex()
                .gap_3()
                .child(div().w(px(72.)).text_color(colors.text_muted).child(label))
                .child(
                    div()
                        .font_family(MONO_FONT)
                        .text_size(theme.metrics.mono)
                        .text_color(colors.text)
                        .child(value),
                )
        };
        let hint = match details.view {
            Some(_) => Some("Click selects its element · shift-click reveals it"),
            None if bars.len() > 1 => Some("Click to zoom in on them"),
            None => None,
        };
        Some(
            deferred(
                anchored()
                    .position(*anchor + point(px(-12.), px(6.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        floating_surface(theme)
                            .id("loupe-flame-tooltip")
                            .debug_selector(|| "loupe-flame-tooltip".into())
                            .px_2()
                            .py(px(8.))
                            .min_w(px(180.))
                            .max_w(px(360.))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap_2()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.text)
                                            .child(details.title.clone()),
                                    )
                                    .child(div().text_color(colors.text_muted).child(details.kind)),
                            )
                            .children(
                                details
                                    .facts
                                    .iter()
                                    .map(|(label, value)| fact(label, value.clone())),
                            )
                            .children(site.map(|(label, site)| fact(label, format::location(site))))
                            .children(hint.map(|hint| {
                                div().pt(px(4.)).text_color(colors.text_faint).child(hint)
                            })),
                    ),
            )
            .priority(1)
            .into_any_element(),
        )
    }

    fn render_bottom_up(
        &self,
        shown: &Shown,
        wide: bool,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        let rows = shown.rows.clone();
        let columns = columns(wide);
        let largest = max_self(&rows);
        let header = SectionHeader::new("Bottom-up")
            .detail("self time by view type")
            .rule()
            .when(self.emphasized.is_some(), |this| {
                this.action(
                    Button::new("loupe-bottom-up-clear")
                        .label("Clear highlight")
                        .size(ButtonSize::Small)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.emphasize(None, window, cx)),
                        ),
                )
            })
            .action(
                Segmented::new("loupe-bottom-up-scope")
                    .segment(Segment::label("This frame").tooltip("Views in the shown frame"))
                    .segment(
                        Segment::label("All frames").tooltip("Views in every app frame recorded"),
                    )
                    .selected(match self.scope {
                        Scope::Frame => 0,
                        Scope::All => 1,
                    })
                    .on_select(cx.listener(|this, ix: &usize, _, cx| {
                        this.set_scope(if *ix == 0 { Scope::Frame } else { Scope::All }, cx)
                    })),
            );
        let body = if rows.is_empty() {
            div()
                .px(theme.metrics.gutter)
                .pb(px(8.))
                .text_color(colors.text_muted)
                .child("No view rendered or reused in this frame")
                .into_any_element()
        } else {
            let height = theme.metrics.control
                + theme.metrics.row * rows.len().min(TABLE_ROWS) as f32
                + px(2.);
            let cell_columns = columns.clone();
            let table = Table::new(
                "loupe-bottom-up",
                &self.table,
                columns.iter().map(|column| table_column(*column)).collect(),
                move |data_ix, column, window, cx| {
                    let theme = Theme::of(window, cx);
                    cell(&rows[data_ix], cell_columns[column], largest, theme)
                },
            );
            div()
                .id("loupe-bottom-up-table")
                .debug_selector(|| "loupe-bottom-up".into())
                .h(height)
                .mx(theme.metrics.gutter)
                .border_1()
                .border_color(colors.line)
                .rounded(theme.metrics.radius)
                .overflow_hidden()
                .child(table)
                .into_any_element()
        };
        div()
            .child(header)
            .child(body)
            .child(div().h(px(8.)))
            .into_any_element()
    }

    fn render_insights(
        &self,
        shown: &Shown,
        navigate: &Navigate,
        theme: &'static Theme,
    ) -> AnyElement {
        let colors = &theme.colors;
        let list = &shown.insights;
        let focus = Some(shown.frame.id);
        div()
            .child(
                SectionHeader::new("Insights")
                    .detail(format::count(list.len() as u64))
                    .rule(),
            )
            .when(list.is_empty(), |this| {
                this.child(
                    div()
                        .px(theme.metrics.gutter)
                        .pb(px(8.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Check)
                                .size(theme.metrics.icon)
                                .color(colors.ok),
                        )
                        .child(
                            div()
                                .text_color(colors.text_muted)
                                .child("Nothing stands out in the recorded frames"),
                        ),
                )
            })
            .children(list.iter().enumerate().map(|(ix, insight)| {
                super::insights::insight_row(ix, insight, focus, navigate, theme)
            }))
            .into_any_element()
    }

    fn render_export(&self, theme: &'static Theme, cx: &mut Context<Self>) -> AnyElement {
        let colors = &theme.colors;
        let notice = self.notice.as_ref().map(|Notice { text, path, tone }| {
            let path = path.clone();
            div()
                .px(theme.metrics.gutter)
                .pt(px(4.))
                .flex()
                .items_center()
                .gap_2()
                .text_size(theme.metrics.text_small)
                .child(
                    Icon::new(match tone {
                        Tone::Crit => IconName::Warning,
                        _ => IconName::Check,
                    })
                    .size(theme.metrics.icon_small)
                    .color(tone.color(theme)),
                )
                .child(
                    div()
                        .id("loupe-frames-notice")
                        .min_w_0()
                        .truncate()
                        .text_color(colors.text)
                        .child(text.clone())
                        .tooltip(Tooltip::text(text.clone())),
                )
                .children(path.map(|path| {
                    Button::new("loupe-export-reveal")
                        .label("Reveal")
                        .size(ButtonSize::Small)
                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                }))
        });
        div()
            .child(
                SectionHeader::new("Export")
                    .detail("Chrome trace JSON")
                    .rule(),
            )
            .child(
                div()
                    .px(theme.metrics.gutter)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("loupe-export-trace")
                            .label("Export trace…")
                            .style(ButtonStyle::Subtle)
                            .tooltip("Save every recorded frame and input as a trace file")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.export_trace(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("loupe-copy-trace")
                            .label("Copy trace")
                            .tooltip("Copy the trace JSON to the clipboard")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.copy_trace(window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("loupe-open-perfetto")
                            .flex_none()
                            .text_size(theme.metrics.text_small)
                            .text_color(colors.text_muted)
                            .cursor_pointer()
                            .hover(|style| style.text_color(colors.accent))
                            .child("Open it in ui.perfetto.dev ↗")
                            .on_click(|_, _, cx| cx.open_url(PERFETTO_URL)),
                    ),
            )
            .children(notice)
            .into_any_element()
    }
}
