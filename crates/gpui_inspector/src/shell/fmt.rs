//! Minimal number and name formatting for the shell.
//!
//! Deliberately small and private: the analysis library owns the canonical
//! formatting helpers, and these fold into them when the two meet.

use gpui::{
    EntityId,
    inspector::{ElementKind, ElementRecord, short_type_name},
};
use std::{panic::Location, time::Duration};

/// Milliseconds with one decimal: `3.1`, `23.4`, `0.4`.
pub(crate) fn ms(duration: Duration) -> String {
    format!("{:.1}", duration.as_secs_f64() * 1000.)
}

/// A count with thousands separators: `1,284`.
pub(crate) fn count(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Bytes in binary units with one decimal: `812 B`, `4.0 KB`, `2.1 MB`.
pub(crate) fn bytes(value: usize) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024. && unit < UNITS.len() - 1 {
        amount /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{amount:.1} {}", UNITS[unit])
    }
}

/// An entity id as its slot number, `#6` (the version bits are noise to a reader).
pub(crate) fn entity(id: EntityId) -> String {
    format!("#{}", id.as_u64() & 0xffff_ffff)
}

/// A source location as `file.rs:line` (the file name without its directories).
pub(crate) fn location(location: &Location<'static>) -> String {
    let file = location.file();
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    format!("{name}:{}", location.line())
}

/// A short, recognizable name for an element: the type for views and
/// components (`IssueList`), the lowercase element type plus its id for
/// elements (`div#row-3`, `text`).
pub(crate) fn element_name(record: &ElementRecord) -> String {
    match record.kind {
        ElementKind::View { type_name, .. } | ElementKind::Component { type_name } => {
            short_type_name(type_name).to_string()
        }
        ElementKind::Element { type_name } => {
            let name = element_type_name(type_name).to_lowercase();
            match &record.id {
                Some(id) => format!("{name}#{id}"),
                None => name,
            }
        }
    }
}

/// `Stateful<Div>` reads as `Div` (the wrapper is an implementation detail)
/// and every text element (`&str`, `SharedString`, `StyledText`...) as `text`.
fn element_type_name(type_name: &'static str) -> &'static str {
    let short = short_type_name(type_name);
    if short == "Stateful"
        && let Some(inner) = type_name
            .split_once('<')
            .and_then(|(_, rest)| rest.strip_suffix('>'))
    {
        return short_type_name(inner);
    }
    match short {
        "&str" | "str" | "String" | "SharedString" | "StyledText" | "InteractiveText" => "text",
        short => short,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, inspector::ElementFlags};

    #[test]
    fn formats_numbers() {
        assert_eq!(ms(Duration::from_micros(3_140)), "3.1");
        assert_eq!(ms(Duration::from_micros(23_450)), "23.4");
        assert_eq!(count(7), "7");
        assert_eq!(count(1_284), "1,284");
        assert_eq!(count(1_234_567), "1,234,567");
        assert_eq!(bytes(812), "812 B");
        assert_eq!(bytes(2_200_000), "2.1 MB");
    }

    fn record(kind: ElementKind, id: Option<&str>) -> ElementRecord {
        ElementRecord {
            key: None,
            parent: None,
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

    #[test]
    fn names_elements_views_and_components() {
        let div = ElementKind::Element {
            type_name: "gpui::elements::div::Stateful<gpui::elements::div::Div>",
        };
        assert_eq!(element_name(&record(div, Some("row-3"))), "div#row-3");
        let text = ElementKind::Element {
            type_name: "gpui::elements::text::StyledText",
        };
        assert_eq!(element_name(&record(text, None)), "text");
        let component = ElementKind::Component {
            type_name: "inbox::ui::Avatar",
        };
        assert_eq!(element_name(&record(component, None)), "Avatar");
    }
}
