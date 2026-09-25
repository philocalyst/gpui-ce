//! The status bar: where the selection lives, and what everything costs.

use crate::{
    analysis::format,
    theme::{MONO_FONT, Theme},
    widgets::{Pill, Tone},
};
use gpui::{
    App, IntoElement, RenderOnce, SharedString, Styled, Window, div,
    inspector::{ElementKey, ElementKind, ElementTree},
    prelude::*,
    px,
};
use std::{rc::Rc, time::Duration};

/// Most breadcrumb segments shown; older ancestors collapse into `…`.
const MAX_CRUMBS: usize = 4;

/// A breadcrumb segment: a name and the element it selects.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Crumb {
    pub name: SharedString,
    pub key: ElementKey,
}

/// The path to `key` through its landmarks (enclosing views and components),
/// then the element itself. Keeps the last [`MAX_CRUMBS`] segments and
/// reports whether older ones were dropped.
pub(crate) fn breadcrumb(tree: &ElementTree, key: ElementKey) -> (Vec<Crumb>, bool) {
    let Some(ix) = tree.find(key) else {
        return (Vec::new(), false);
    };
    let mut crumbs: Vec<Crumb> = std::iter::once(ix)
        .chain(tree.ancestors(ix))
        .enumerate()
        .filter_map(|(depth, ix)| {
            let record = tree.get(ix)?;
            let landmark = depth == 0 || !matches!(record.kind, ElementKind::Element { .. });
            if !landmark {
                return None;
            }
            Some(Crumb {
                name: format::element_label(record).into(),
                key: record.key?,
            })
        })
        .collect();
    crumbs.reverse();
    let elided = crumbs.len() > MAX_CRUMBS;
    if elided {
        crumbs.drain(..crumbs.len() - MAX_CRUMBS);
    }
    (crumbs, elided)
}

/// What the status bar shows.
#[derive(IntoElement)]
pub(crate) struct StatusBar {
    pub crumbs: Vec<Crumb>,
    pub elided: bool,
    pub app_time: Option<Duration>,
    pub loupe_time: Option<Duration>,
    pub retained: usize,
    pub frozen: bool,
    /// The app is held still.
    pub held: bool,
    /// "Hold the app in 3 seconds" is counting down.
    pub holding_soon: bool,
    pub on_crumb: Rc<dyn Fn(ElementKey, &mut Window, &mut App)>,
}

impl RenderOnce for StatusBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let colors = &theme.colors;
        let last = self.crumbs.len().saturating_sub(1);
        let separator = || div().flex_none().text_color(colors.text_faint).child("›");

        let path = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(5.))
            .overflow_hidden()
            .when(self.crumbs.is_empty(), |this| {
                this.text_color(colors.text_faint)
                    .child(div().truncate().child("No element selected"))
            })
            .when(self.elided, |this| {
                this.child(div().flex_none().text_color(colors.text_faint).child("…"))
                    .child(separator())
            })
            .children(self.crumbs.into_iter().enumerate().flat_map(|(ix, crumb)| {
                let on_crumb = self.on_crumb.clone();
                let key = crumb.key;
                let segment = div()
                    .id(ix)
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .text_color(if ix == last {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .hover(|style| style.text_color(colors.accent))
                    .when(ix < last, |this| this.flex_shrink_1())
                    .when(ix == last, |this| this.flex_none().max_w(px(200.)))
                    .child(crumb.name)
                    .on_click(move |_, window, cx| on_crumb(key, window, cx))
                    .into_any_element();
                let separator = (ix < last).then(|| separator().into_any_element());
                std::iter::once(segment).chain(separator)
            }));

        let metric = |label: &'static str, value: String| {
            div()
                .flex_none()
                .flex()
                .items_baseline()
                .gap(px(3.))
                .child(div().text_color(colors.text_faint).child(label))
                .child(
                    div()
                        .font_family(MONO_FONT)
                        .text_color(colors.text_muted)
                        .child(value),
                )
        };
        let ms = |duration: Option<Duration>| match duration {
            Some(duration) => format!("{} ms", format::millis(duration)),
            None => "–".into(),
        };

        div()
            .flex_none()
            .h(theme.metrics.status)
            .w_full()
            .px(theme.metrics.gutter)
            .flex()
            .items_center()
            .gap(px(12.))
            .overflow_hidden()
            .bg(colors.surface)
            .border_t_1()
            .border_color(colors.line)
            .text_size(theme.metrics.text_small)
            .child(path)
            .child(metric("app", ms(self.app_time)))
            .child(metric("loupe", ms(self.loupe_time)))
            .child(metric("mem", format::bytes(self.retained as u64)))
            .when(self.frozen, |this| {
                this.child(Pill::new("Frozen").tone(Tone::Accent).strong())
            })
            .when(self.holding_soon, |this| {
                this.child(Pill::new("Holding in 3 s").tone(Tone::Warn))
            })
            .when(self.held, |this| {
                this.child(Pill::new("Held").tone(Tone::Warn).strong())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Bounds,
        inspector::{ElementFlags, ElementRecord, PathKey},
    };

    fn record(parent: Option<u32>, kind: ElementKind, id: Option<&str>) -> ElementRecord {
        ElementRecord {
            key: None,
            parent,
            depth: 0,
            kind,
            id: id.map(|id| id.to_string().into()),
            bounds: Bounds::default(),
            visible_bounds: None,
            paint_order: 0,
            primitives: 0,
            flags: ElementFlags::empty(),
            details: None,
        }
    }

    fn tree(records: Vec<ElementRecord>) -> ElementTree {
        let mut tree = ElementTree {
            elements: records
                .into_iter()
                .enumerate()
                .map(|(ix, mut record)| {
                    record.key = Some(ElementKey {
                        path: PathKey(ix as u32),
                        instance: 0,
                    });
                    record
                })
                .collect(),
            ..Default::default()
        };
        tree.rebuild_children();
        tree
    }

    const DIV: ElementKind = ElementKind::Element {
        type_name: "gpui::elements::div::Div",
    };

    fn view(name: &'static str) -> ElementKind {
        ElementKind::View {
            entity: gpui::EntityId::from(1u64),
            type_name: name,
        }
    }

    fn key(ix: u32) -> ElementKey {
        ElementKey {
            path: PathKey(ix),
            instance: 0,
        }
    }

    fn names(crumbs: &[Crumb]) -> Vec<&str> {
        crumbs.iter().map(|crumb| crumb.name.as_ref()).collect()
    }

    #[test]
    fn breadcrumbs_list_landmarks_down_to_the_selection() {
        let tree = tree(vec![
            record(None, view("app::Root"), None),
            record(Some(0), DIV, None),
            record(Some(1), view("app::IssueList"), None),
            record(Some(2), DIV, Some("row-3")),
            record(Some(3), DIV, None),
        ]);
        let (crumbs, elided) = breadcrumb(&tree, key(3));
        assert_eq!(names(&crumbs), ["Root", "IssueList", "div#row-3"]);
        assert!(!elided);
        assert_eq!(crumbs[1].key, key(2));
        let (crumbs, _) = breadcrumb(&tree, key(4));
        assert_eq!(names(&crumbs), ["Root", "IssueList", "div"]);
    }

    #[test]
    fn long_breadcrumbs_keep_the_last_segments() {
        let mut records = vec![record(None, view("app::Root"), None)];
        for ix in 0..7 {
            records.push(record(Some(ix), view("app::Level"), None));
        }
        let tree = tree(records);
        let (crumbs, elided) = breadcrumb(&tree, key(7));
        assert!(elided);
        assert_eq!(crumbs.len(), MAX_CRUMBS);
        assert_eq!(crumbs.last().map(|crumb| crumb.key), Some(key(7)));
        assert!(breadcrumb(&tree, key(99)).0.is_empty());
    }
}
