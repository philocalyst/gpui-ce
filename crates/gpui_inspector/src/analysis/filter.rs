//! Filter text for Loupe's lists: whitespace-separated terms that must all
//! match, and `-term` exclusions that must not.
//!
//! `cmd -mouse` keeps rows mentioning `cmd` and not `mouse`. Terms match
//! case-insensitively anywhere in a row's searchable text (its haystack).

/// A parsed filter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextFilter {
    include: Vec<String>,
    exclude: Vec<String>,
}

impl TextFilter {
    /// Parses `text`. A term starting with `-` excludes; a lone `-` is ignored.
    pub fn parse(text: &str) -> Self {
        let mut filter = Self::default();
        for term in text.split_whitespace() {
            let term = term.to_lowercase();
            match term.strip_prefix('-') {
                Some("") => {}
                Some(excluded) => filter.exclude.push(excluded.to_string()),
                None => filter.include.push(term),
            }
        }
        filter
    }

    /// Whether the filter keeps every row.
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    /// Whether a row whose haystack (see [`haystack`]) is `haystack` passes.
    pub fn matches(&self, haystack: &str) -> bool {
        self.include
            .iter()
            .all(|term| haystack.contains(term.as_str()))
            && !self
                .exclude
                .iter()
                .any(|term| haystack.contains(term.as_str()))
    }
}

/// A row's searchable text: its parts, lowercased and separated by spaces.
pub fn haystack<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut haystack = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !haystack.is_empty() {
            haystack.push(' ');
        }
        haystack.extend(part.chars().flat_map(char::to_lowercase));
    }
    haystack
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_must_all_match_and_exclusions_must_not() {
        let row = haystack(["KeyDown", "cmd-k", "workspace::OpenPalette"]);
        assert!(TextFilter::parse("").matches(&row));
        assert!(TextFilter::parse("cmd").matches(&row));
        assert!(TextFilter::parse("CMD palette").matches(&row));
        assert!(!TextFilter::parse("cmd mouse").matches(&row));
        assert!(!TextFilter::parse("cmd -palette").matches(&row));
        assert!(TextFilter::parse("-mouse").matches(&row));
        assert!(!TextFilter::parse("-KEY").matches(&row));
    }

    #[test]
    fn a_lone_dash_is_ignored_and_whitespace_is_insignificant() {
        let filter = TextFilter::parse("  -   open \t -move ");
        assert_eq!(filter, TextFilter::parse("open -move"));
        assert!(!filter.is_empty());
        assert!(TextFilter::parse(" - ").is_empty());
    }

    #[test]
    fn haystacks_skip_empty_parts() {
        assert_eq!(haystack(["A", "", "Bc"]), "a bc");
        assert_eq!(haystack([]), "");
    }
}
