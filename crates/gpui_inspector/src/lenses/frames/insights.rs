//! Insights: plain-language findings about the shown frame and the ring,
//! each with the links that lead to its cause.

use crate::{
    analysis::{format, insights::Insight},
    lenses::{
        links::{Navigate, Target, link},
        severity_glyph,
    },
    theme::Theme,
    widgets::Icon,
};
use gpui::{AnyElement, FontWeight, IntoElement, Styled, div, prelude::*, px};

/// The links an insight offers, in order: its frame (unless shown), its
/// element, its entity and its source site.
pub(crate) fn insight_links(insight: &Insight, shown_frame: Option<u64>) -> Vec<(Target, String)> {
    let links = &insight.links;
    let mut targets = Vec::new();
    if let Some(frame) = links.frame.filter(|frame| Some(*frame) != shown_frame) {
        targets.push((Target::Frame(frame), format!("Frame #{frame}")));
    }
    if let Some(element) = links.element {
        targets.push((Target::Element(element), "Reveal element".to_string()));
    }
    if let Some(entity) = links.entity {
        targets.push((Target::Entity(entity), "Show entity".to_string()));
    }
    if let Some(site) = links.site {
        targets.push((Target::Site(site), format::location(site)));
    }
    targets
}

/// One insight: severity, title, explanation, suggestion and links.
pub(crate) fn insight_row(
    ix: usize,
    insight: &Insight,
    shown_frame: Option<u64>,
    navigate: &Navigate,
    theme: &Theme,
) -> AnyElement {
    let colors = &theme.colors;
    let (icon, tone) = severity_glyph(insight.severity);
    let links = insight_links(insight, shown_frame);
    div()
        .px(theme.metrics.gutter)
        .py(px(8.))
        .flex()
        .gap_2()
        .child(
            div()
                .flex_none()
                .h(theme.metrics.line_height)
                .flex()
                .items_center()
                .child(
                    Icon::new(icon)
                        .size(theme.metrics.icon)
                        .color(tone.color(theme)),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text)
                        .child(insight.title.clone()),
                )
                .child(
                    div()
                        .text_size(theme.metrics.text_small)
                        .text_color(colors.text_muted)
                        .child(insight.explanation.clone()),
                )
                .child(
                    div()
                        .text_size(theme.metrics.text_small)
                        .text_color(colors.text)
                        .child(insight.suggestion.clone()),
                )
                .when(!links.is_empty(), |this| {
                    this.child(
                        div()
                            .pt(px(4.))
                            .ml(px(-4.))
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .children(links.into_iter().enumerate().map(
                                |(link_ix, (target, label))| {
                                    link(
                                        format!("loupe-insight-{ix}-{link_ix}"),
                                        label,
                                        target,
                                        navigate,
                                        theme,
                                    )
                                },
                            )),
                    )
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{
        Severity,
        insights::{InsightKind, Links},
    };
    use gpui::{
        EntityId,
        inspector::{ElementKey, PathKey},
    };
    use std::panic::Location;

    fn insight(links: Links) -> Insight {
        Insight {
            severity: Severity::Warning,
            kind: InsightKind::HotView,
            title: "List re-rendered 60× in the last second".into(),
            explanation: String::new(),
            suggestion: String::new(),
            links,
        }
    }

    #[test]
    fn links_skip_the_frame_already_shown() {
        let site = Location::caller();
        let element = ElementKey {
            path: PathKey(4),
            instance: 0,
        };
        let full = insight(Links {
            frame: Some(9),
            element: Some(element),
            entity: Some(EntityId::from(3u64)),
            site: Some(site),
        });
        let labels = |shown| {
            insight_links(&full, shown)
                .into_iter()
                .map(|(_, label)| label)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(Some(2)),
            [
                "Frame #9".to_string(),
                "Reveal element".to_string(),
                "Show entity".to_string(),
                format::location(site),
            ]
        );
        assert_eq!(labels(Some(9)).len(), 3);
        assert!(insight_links(&insight(Links::default()), None).is_empty());
    }
}
