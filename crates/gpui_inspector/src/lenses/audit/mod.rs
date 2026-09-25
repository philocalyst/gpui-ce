//! Audit: what should I fix first?
//!
//! ```text
//! verdict     1 critical problem to fix first · 1 critical · 5 warnings · 2 notes
//! filter      All 8 | Critical 1 | Warnings 5 | Notes 2
//! findings    grouped by rule, worst first: severity, title, ×instances, site
//! detail      the selected finding: why it matters, every instance, links
//! capture     capture level, budget, what recording costs
//! ```
//!
//! The rules ([`crate::analysis::audit::default_rules`]) run over the latest
//! element tree and the recorded frames once per capture generation; the
//! rail badge and the lens share that result. Selecting a finding highlights
//! every element it concerns in the app and selects the first. Narrow docks
//! expand the selected finding in place; wide ones show it on the right.

mod model;

use self::model::{Counts, Filter, FindingKey, blind_spots, group, health, rule_name, step};
use super::{
    LensView, RailBadge,
    capture::CaptureCard,
    highlights::{LensHighlights, element_highlight},
    links::{Navigate, Target, follow, link, site_link},
    memo::Memo,
    observe_state, severity_glyph,
};
use crate::{
    analysis::{
        Severity,
        audit::{AuditInput, Finding, Rule, audit, default_rules},
        format,
        stats::{frame_stats, mean},
    },
    state::{Lens, LensLayout, LoupeState},
    theme::{MONO_FONT, Theme, UI_FONT},
    widgets::{EmptyState, Icon, IconName, Pill, SectionHeader, Segment, Segmented, Tone, Tooltip},
};
use gpui::{
    AnyElement, App, ColorExt as _, Context, Entity, FocusHandle, FontWeight, IntoElement,
    KeyBinding, MouseButton, Pixels, Render, SharedString, Styled, Subscription, Window, actions,
    div,
    inspector::{ElementKey, ElementTree, InspectorCapture, OverlayHighlight},
    prelude::*,
    px, relative,
};
use std::{cell::RefCell, rc::Rc, time::Duration};

actions!(
    loupe_audit,
    [
        /// Selects the next finding.
        NextFinding,
        /// Selects the previous finding.
        PreviousFinding,
        /// Shows the selected finding's element in Elements.
        RevealFinding,
    ]
);

/// Key context of the Audit lens.
const CONTEXT: &str = "LoupeAudit";
/// Below this width the selected finding expands in the list instead of
/// taking a column of its own.
const TWO_COLUMNS_WIDTH: Pixels = px(720.);
/// Instances listed in a finding's detail before `and N more`.
const LISTED_INSTANCES: usize = 8;

/// Binds the lens' keys.
pub(crate) fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", NextFinding, context),
        KeyBinding::new("j", NextFinding, context),
        KeyBinding::new("up", PreviousFinding, context),
        KeyBinding::new("k", PreviousFinding, context),
        KeyBinding::new("enter", RevealFinding, context),
    ]);
}

fn two_columns(window: &Window) -> bool {
    LensLayout::of(window) == LensLayout::SideBySide
        && window
            .inspector_bounds()
            .is_some_and(|bounds| bounds.size.width >= TWO_COLUMNS_WIDTH)
}

/// Runs the rules over `capture`.
fn run(capture: &InspectorCapture, rules: &[Box<dyn Rule>]) -> Vec<Finding> {
    let source = |path| capture.path_info(path).map(|info| info.source);
    let tree = capture.latest_tree().map(|tree| tree.as_ref());
    audit(
        &AuditInput {
            tree,
            frames: capture.frames(),
            budget: capture.config().budget,
            source: &source,
        },
        rules,
    )
}

/// The Audit lens.
pub(crate) struct AuditLens {
    state: Entity<LoupeState>,
    focus: FocusHandle,
    rules: Vec<Box<dyn Rule>>,
    /// Shared by the lens and its rail badge: computed once per generation.
    findings: RefCell<Memo<(u64, Duration), Vec<Finding>>>,
    overhead: Memo<(u64, Duration), (f64, Duration)>,
    filter: Filter,
    selected: Option<FindingKey>,
    highlights: LensHighlights,
    notice: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl AuditLens {
    pub fn new(state: Entity<LoupeState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        let subscriptions = vec![cx.observe_in(&state, window, |this, state, window, cx| {
            let lens = state.read(cx).lens();
            let mut changed = this.highlights.sync(lens, window);
            if lens == Lens::Audit && this.selected.is_some() {
                // Elements may have moved in newer frames.
                changed |= this.highlight_selection(window, cx);
            }
            if changed {
                cx.notify();
            }
        })];
        Self {
            state,
            focus: cx.focus_handle(),
            rules: default_rules(),
            findings: RefCell::default(),
            overhead: Memo::default(),
            filter: Filter::All,
            selected: None,
            highlights: LensHighlights::new(Lens::Audit),
            notice: None,
            _subscriptions: subscriptions,
        }
    }

    fn findings(&self, capture: &InspectorCapture) -> Rc<Vec<Finding>> {
        let key = (capture.generation(), capture.config().budget);
        self.findings
            .borrow_mut()
            .get(key, || run(capture, &self.rules))
    }

    fn navigator(&self, cx: &mut Context<Self>) -> Navigate {
        let this = cx.entity().downgrade();
        Rc::new(move |target, _, cx| {
            this.update(cx, |this, cx| {
                if let Some(message) = follow(target, &this.state, cx) {
                    this.notice = Some(message.into());
                    cx.notify();
                }
            })
            .ok();
        })
    }

    /// Selects the finding at `ix` of the current findings: highlights its
    /// elements in the app and selects the first.
    fn select(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(capture) = window.inspector_capture() else {
            return;
        };
        let findings = self.findings(capture);
        let finding = ix.and_then(|ix| findings.get(ix));
        self.selected = finding.map(FindingKey::of);
        self.notice = None;
        if let Some(element) = finding.and_then(|finding| finding.element) {
            self.state
                .update(cx, |state, cx| state.select_element(Some(element), cx));
        }
        self.highlight_selection(window, cx);
        cx.notify();
    }

    /// Pins highlights over every element of the selected finding. Returns
    /// whether the overlay changed.
    fn highlight_selection(&mut self, window: &mut Window, cx: &App) -> bool {
        let theme = Theme::of(window, cx);
        let highlights = window
            .inspector_capture()
            .and_then(|capture| {
                let findings = self.findings(capture);
                let finding = findings.get(self.selected?.find(&findings)?)?;
                Some(finding_highlights(capture, finding, theme))
            })
            .unwrap_or_default();
        self.highlights.pin(highlights, window)
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(capture) = window.inspector_capture() else {
            return;
        };
        let findings = self.findings(capture);
        let groups = group(&findings, self.filter);
        let current = self.selected.and_then(|key| key.find(&findings));
        if let Some(next) = step(&groups, current, delta) {
            self.select(Some(next), window, cx);
        }
    }

    fn reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let element = window.inspector_capture().and_then(|capture| {
            let findings = self.findings(capture);
            findings.get(self.selected?.find(&findings)?)?.element
        });
        if let Some(element) = element {
            follow(Target::Element(element), &self.state, cx);
        }
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        cx.notify();
    }
}

/// Highlights over every element `finding` concerns, in its severity's
/// color; the first carries a label chip.
fn finding_highlights(
    capture: &InspectorCapture,
    finding: &Finding,
    theme: &Theme,
) -> Vec<OverlayHighlight> {
    let Some(tree) = capture.latest_tree() else {
        return Vec::new();
    };
    let (_, tone) = severity_glyph(finding.severity);
    let color = tone.color(theme).opacity(0.2);
    finding
        .elements()
        .enumerate()
        .filter_map(|(ix, key)| {
            let label = (ix == 0).then(|| {
                let name = tree
                    .find(key)
                    .and_then(|found| tree.get(found))
                    .map(format::element_label)
                    .unwrap_or_else(|| rule_name(finding.rule).to_string());
                SharedString::from(match finding.instances {
                    1 => name,
                    count => format!("{name} ×{count}"),
                })
            });
            element_highlight(tree, key, color, label)
        })
        .collect()
}

impl Render for AuditLens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let root = div()
            .id("loupe-audit")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
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
            .on_action(cx.listener(|this, _: &NextFinding, window, cx| this.step(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousFinding, window, cx| this.step(-1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &RevealFinding, window, cx| this.reveal(window, cx)));
        let Some(capture) = window.inspector_capture() else {
            return root.child(EmptyState::new("Loupe is closed"));
        };
        let findings = self.findings(capture);
        let columns = two_columns(window);
        let navigate = self.navigator(cx);
        let selected = self.selected.and_then(|key| key.find(&findings));
        let tree_elements = capture.latest_tree().map_or(0, |tree| tree.elements.len());
        let app_frames = capture
            .frames()
            .iter()
            .filter(|frame| !frame.inspector_only)
            .count();
        let counts = Counts::of(&findings);
        let blind = blind_spots(capture.config().level, capture.latest_tree().is_some());

        let caption = div()
            .px(theme.metrics.gutter)
            .pt(px(8.))
            .text_size(theme.metrics.text_small)
            .text_color(colors.text_faint)
            .child(Lens::Audit.question());
        let summary = render_summary(
            &findings,
            &counts,
            tree_elements,
            app_frames,
            blind,
            theme,
            cx,
        );
        let filter = self.render_filter(&counts, cx);
        let list = self.render_list(capture, selected, columns, &navigate, theme, cx);
        let detail = selected.and_then(|ix| findings.get(ix)).map(|finding| {
            render_detail(
                finding,
                capture.latest_tree().map(|tree| tree.as_ref()),
                &navigate,
                theme,
            )
        });
        let capture_card = self.render_capture(capture);
        let notice = self.notice.clone().map(|notice| {
            div()
                .px(theme.metrics.gutter)
                .py(px(4.))
                .text_size(theme.metrics.text_small)
                .text_color(colors.text_muted)
                .child(notice)
        });

        let body = if columns {
            div()
                .size_full()
                .flex()
                .child(
                    div()
                        .id("loupe-audit-list")
                        .flex_none()
                        .w(relative(0.5))
                        .max_w(px(560.))
                        .h_full()
                        .overflow_y_scroll()
                        .border_r_1()
                        .border_color(colors.line)
                        .pb_4()
                        .child(caption)
                        .child(summary)
                        .child(filter)
                        .child(list),
                )
                .child(
                    div()
                        .id("loupe-audit-detail")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .pb_4()
                        .child(SectionHeader::new("Finding"))
                        .child(detail.unwrap_or_else(|| {
                            div()
                                .h(px(120.))
                                .child(
                                    EmptyState::new("Select a finding")
                                        .description("Its elements light up in the app."),
                                )
                                .into_any_element()
                        }))
                        .children(notice)
                        .child(SectionHeader::new("Capture").rule())
                        .child(capture_card),
                )
        } else {
            div().size_full().child(
                div()
                    .id("loupe-audit-column")
                    .size_full()
                    .overflow_y_scroll()
                    .pb_4()
                    .child(caption)
                    .child(summary)
                    .child(filter)
                    .child(list)
                    .children(notice)
                    .child(SectionHeader::new("Capture").rule())
                    .child(capture_card),
            )
        };
        root.child(body)
    }
}

impl LensView for AuditLens {
    fn rail_badge(&self, window: &Window, _cx: &App) -> Option<RailBadge> {
        let findings = self.findings(window.inspector_capture()?);
        let counts = Counts::of(&findings);
        match counts.critical + counts.warning {
            0 => None,
            count => Some(RailBadge::alert(
                format::count(count as u64),
                if counts.critical > 0 {
                    Tone::Crit
                } else {
                    Tone::Warn
                },
            )),
        }
    }
}

fn render_summary(
    findings: &[Finding],
    counts: &Counts,
    elements: usize,
    frames: usize,
    blind: Option<&'static str>,
    theme: &'static Theme,
    cx: &mut Context<AuditLens>,
) -> AnyElement {
    let colors = &theme.colors;
    let worst = findings.first().map(|finding| finding.severity);
    let (icon, tone) = worst.map_or((IconName::Check, Tone::Ok), severity_glyph);
    let chips = [
        (Severity::Critical, counts.critical),
        (Severity::Warning, counts.warning),
        (Severity::Info, counts.info),
    ]
    .into_iter()
    .map(|(severity, count)| {
        let (icon, tone) = severity_glyph(severity);
        let color = if count > 0 {
            tone.color(theme)
        } else {
            colors.text_faint
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(Icon::new(icon).size(theme.metrics.icon_small).color(color))
            .child(
                div()
                    .font_family(MONO_FONT)
                    .text_size(theme.metrics.mono)
                    .text_color(if count > 0 {
                        colors.text
                    } else {
                        colors.text_faint
                    })
                    .child(count.to_string()),
            )
            .child(
                div()
                    .text_color(colors.text_muted)
                    .child(match (severity, count) {
                        (Severity::Critical, _) => "critical",
                        (Severity::Warning, 1) => "warning",
                        (Severity::Warning, _) => "warnings",
                        (Severity::Info, 1) => "note",
                        (Severity::Info, _) => "notes",
                    }),
            )
    });
    let start_with = findings.first().map(|first| {
        let title = first.title.clone();
        div()
            .id("loupe-audit-start")
            .flex()
            .items_center()
            .gap_1()
            .text_size(theme.metrics.text_small)
            .child(
                div()
                    .flex_none()
                    .text_color(colors.text_muted)
                    .child("Start with"),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(colors.accent)
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .child(title.clone()),
            )
            .tooltip(Tooltip::text(title))
            .on_click(cx.listener(|this, _, window, cx| this.select(Some(0), window, cx)))
    });
    div()
        .id("loupe-audit-summary")
        .debug_selector(|| "loupe-audit-summary".into())
        .mx(theme.metrics.gutter)
        .mt(px(4.))
        .mb(px(8.))
        .p(px(12.))
        .rounded(theme.metrics.radius)
        .border_1()
        .border_color(colors.line)
        .bg(colors.surface)
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(Icon::new(icon).size(px(16.)).color(tone.color(theme)))
                .child(
                    div()
                        .min_w_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(health(counts, elements, frames)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_x(px(16.))
                .gap_y(px(4.))
                .children(chips),
        )
        .children(start_with)
        .children(blind.map(|blind| {
            div()
                .flex()
                .gap_2()
                .text_size(theme.metrics.text_small)
                .text_color(colors.text_muted)
                .child(
                    Icon::new(IconName::Info)
                        .size(theme.metrics.icon_small)
                        .color(colors.text_muted),
                )
                .child(div().min_w_0().child(blind))
        }))
        .into_any_element()
}

impl AuditLens {
    fn render_filter(&self, counts: &Counts, cx: &mut Context<Self>) -> AnyElement {
        let selected = Filter::ALL
            .iter()
            .position(|filter| *filter == self.filter)
            .unwrap_or(0);
        let control = Filter::ALL
            .iter()
            .fold(Segmented::new("loupe-audit-filter"), |control, filter| {
                control.segment(Segment::label(filter.label(counts)))
            })
            .selected(selected)
            .on_select(
                cx.listener(|this, ix: &usize, _, cx| this.set_filter(Filter::ALL[*ix], cx)),
            );
        div()
            .px(px(8.))
            .pb(px(4.))
            .flex()
            .child(control)
            .into_any_element()
    }

    fn render_list(
        &self,
        capture: &InspectorCapture,
        selected: Option<usize>,
        columns: bool,
        navigate: &Navigate,
        theme: &'static Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        let findings = self.findings(capture);
        let tree = capture.latest_tree();
        let groups = group(&findings, self.filter);
        if groups.is_empty() {
            let message = if findings.is_empty() {
                "Nothing to fix"
            } else {
                "No findings of this severity"
            };
            return div()
                .h(px(96.))
                .child(EmptyState::new(message).icon(IconName::Check))
                .into_any_element();
        }
        div()
            .children(groups.into_iter().map(|group| {
                let rows = group.findings.iter().map(|&ix| {
                    let finding = &findings[ix];
                    let is_selected = selected == Some(ix);
                    let label = finding
                        .element
                        .zip(tree)
                        .and_then(|(key, tree)| tree.get(tree.find(key)?))
                        .map(format::element_label);
                    let row = finding_row(ix, finding, label, is_selected, theme, cx);
                    let expanded = (is_selected && !columns).then(|| {
                        div()
                            .mx(theme.metrics.gutter)
                            .mb(px(8.))
                            .rounded(theme.metrics.radius)
                            .border_1()
                            .border_color(colors.line)
                            .bg(colors.surface)
                            .p(px(8.))
                            .child(finding_body(
                                finding,
                                tree.map(|tree| tree.as_ref()),
                                navigate,
                                theme,
                            ))
                    });
                    div().child(row).children(expanded)
                });
                div()
                    .child(
                        SectionHeader::new(rule_name(group.rule))
                            .detail(match group.instances {
                                1 => "1 element".to_string(),
                                count => format!("{} elements", format::count(u64::from(count))),
                            })
                            .rule(),
                    )
                    .children(rows)
            }))
            .into_any_element()
    }

    fn render_capture(&mut self, capture: &InspectorCapture) -> AnyElement {
        let budget = capture.config().budget;
        let overhead = self.overhead.get((capture.generation(), budget), || {
            let stats = frame_stats(capture.frames(), budget);
            (
                stats.overhead.share(),
                mean(stats.overhead.inspector_time, capture.frames().len()),
            )
        });
        CaptureCard {
            state: self.state.clone(),
            level: capture.config().level,
            budget,
            retained: capture.retained_bytes(),
            overhead: overhead.0,
            loupe_per_frame: overhead.1,
            frames: capture.frames().len(),
            inputs: capture.input().len(),
        }
        .into_any_element()
    }
}

/// One finding in the list: severity, title, the (first) element it
/// concerns, instances and site.
fn finding_row(
    ix: usize,
    finding: &Finding,
    element: Option<String>,
    selected: bool,
    theme: &'static Theme,
    cx: &mut Context<AuditLens>,
) -> AnyElement {
    let colors = &theme.colors;
    let (icon, tone) = severity_glyph(finding.severity);
    let selector = format!("loupe-finding-{ix}");
    div()
        .id(("loupe-finding", ix))
        .debug_selector(move || selector)
        .h(theme.metrics.property_row)
        .px(theme.metrics.gutter)
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .when(selected, |this| this.bg(colors.selected))
        .when(!selected, |this| this.hover(|style| style.bg(colors.hover)))
        .child(
            Icon::new(icon)
                .size(theme.metrics.icon_small)
                .color(tone.color(theme)),
        )
        .child(
            div()
                .flex_initial()
                .min_w_0()
                .truncate()
                .text_color(colors.text)
                .child(finding.title.clone()),
        )
        .children(element.clone().map(|element| {
            div()
                .flex_initial()
                .min_w(px(24.))
                .max_w(px(110.))
                .truncate()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(colors.text_faint)
                .child(element)
        }))
        .child(div().flex_1())
        .when(finding.instances > 1, |this| {
            this.child(Pill::new(format!(
                "×{}",
                format::count(u64::from(finding.instances))
            )))
        })
        .children(finding.site.map(|site| {
            div()
                .flex_none()
                .max_w(px(160.))
                .truncate()
                .font_family(MONO_FONT)
                .text_size(theme.metrics.mono)
                .text_color(colors.text_muted)
                .child(format::location(site))
        }))
        .tooltip(Tooltip::with_meta(
            finding.title.clone(),
            [element, finding.site.map(format::location_with_path)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · "),
        ))
        .on_click(cx.listener(move |this, _, window, cx| this.select(Some(ix), window, cx)))
        .into_any_element()
}

/// The selected finding, as the right column shows it.
fn render_detail(
    finding: &Finding,
    tree: Option<&ElementTree>,
    navigate: &Navigate,
    theme: &'static Theme,
) -> AnyElement {
    let (icon, tone) = severity_glyph(finding.severity);
    div()
        .px(theme.metrics.gutter)
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(icon)
                        .size(theme.metrics.icon)
                        .color(tone.color(theme)),
                )
                .child(
                    div()
                        .min_w_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(finding.title.clone()),
                ),
        )
        .child(finding_body(finding, tree, navigate, theme))
        .into_any_element()
}

/// Why a finding matters, its links and, for several instances, the
/// elements highlighted in the app. Shared by both layouts.
fn finding_body(
    finding: &Finding,
    tree: Option<&ElementTree>,
    navigate: &Navigate,
    theme: &'static Theme,
) -> AnyElement {
    let colors = &theme.colors;
    let instances: Vec<(ElementKey, String)> = finding
        .elements()
        .take(LISTED_INSTANCES)
        .filter_map(|key| {
            let record = tree.and_then(|tree| tree.get(tree.find(key)?))?;
            let label = format!(
                "{} {}",
                format::element_label(record),
                format::size(record.bounds.size)
            );
            Some((key, label))
        })
        .collect();
    let more = (finding.instances as usize).saturating_sub(instances.len());
    let links = div()
        .ml(px(-4.))
        .flex()
        .flex_wrap()
        .gap(px(4.))
        .children(finding.element.map(|element| {
            link(
                "loupe-finding-reveal",
                "Reveal in Elements",
                Target::Element(element),
                navigate,
                theme,
            )
        }))
        .children(finding.entity.map(|entity| {
            link(
                "loupe-finding-entity",
                "Show entity",
                Target::Entity(entity),
                navigate,
                theme,
            )
        }))
        .children(
            finding
                .site
                .map(|site| site_link("loupe-finding-site", site, navigate, theme)),
        );
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .text_size(theme.metrics.text_small)
                .text_color(colors.text_muted)
                .child(finding.detail.clone()),
        )
        .child(links)
        .when(finding.instances > 1, |this| {
            this.child(
                div()
                    .pt(px(4.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(theme.metrics.text_small)
                            .text_color(colors.text_muted)
                            .child(format!(
                                "{} elements highlighted in the app",
                                format::count(u64::from(finding.instances))
                            )),
                    )
                    .children(instances.into_iter().enumerate().map(|(ix, (key, label))| {
                        div().ml(px(-4.)).child(link(
                            format!("loupe-finding-instance-{ix}"),
                            label,
                            Target::Element(key),
                            navigate,
                            theme,
                        ))
                    }))
                    .when(more > 0, |this| {
                        this.child(
                            div()
                                .text_size(theme.metrics.text_small)
                                .text_color(colors.text_faint)
                                .child(format!("and {} more", format::count(more as u64))),
                        )
                    }),
            )
        })
        .into_any_element()
}
