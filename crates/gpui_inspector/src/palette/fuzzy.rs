//! Fuzzy matching for the palette.
//!
//! A query matches a candidate when its characters appear in order,
//! ignoring case and spaces in the query. Among the possible alignments the
//! best-scoring one is chosen by dynamic programming (O(query × candidate)):
//! matches at the start of the candidate, at word boundaries (after
//! punctuation or at a camelCase hump) and in contiguous runs score higher;
//! gaps cost a little. Shorter candidates win ties.

use std::ops::Range;

/// A successful match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Higher is better.
    pub score: i32,
    /// Char indices (not bytes) of the candidate's matched characters.
    pub positions: Vec<usize>,
}

const MATCH: i32 = 16;
const PREFIX: i32 = 30;
const BOUNDARY: i32 = 24;
const CONSECUTIVE: i32 = 24;
const EXACT_CASE: i32 = 1;
const GAP: i32 = 2;
const LEADING_GAP: i32 = 1;
const MAX_LEADING_GAP: i32 = 12;

fn is_boundary(previous: char, current: char) -> bool {
    !previous.is_alphanumeric()
        || (previous.is_lowercase() && current.is_uppercase())
        || (previous.is_alphabetic() && current.is_numeric())
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Matches `query` against `candidate`, returning the best alignment.
/// An empty query matches everything with score 0.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    let query: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if query.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            positions: Vec::new(),
        });
    }
    let text: Vec<char> = candidate.chars().collect();
    let (n, m) = (query.len(), text.len());
    if n > m {
        return None;
    }
    let query_lower: Vec<char> = query.iter().copied().map(lower).collect();
    let text_lower: Vec<char> = text.iter().copied().map(lower).collect();

    // Quick reject: the query must be a subsequence of the candidate.
    let mut remaining = query_lower.iter().peekable();
    for c in &text_lower {
        if remaining.peek() == Some(&c) {
            remaining.next();
        }
    }
    if remaining.peek().is_some() {
        return None;
    }

    let bonus = |i: usize, j: usize| {
        let position = if j == 0 {
            PREFIX
        } else if is_boundary(text[j - 1], text[j]) {
            BOUNDARY
        } else {
            0
        };
        let case = if query[i] == text[j] { EXACT_CASE } else { 0 };
        MATCH + position + case
    };

    const NONE: i32 = i32::MIN / 2;
    // score[i][j]: best score with query[i] matched at text[j].
    let mut score = vec![vec![NONE; m]; n];
    let mut back = vec![vec![0usize; m]; n];
    for j in 0..m {
        if text_lower[j] == query_lower[0] {
            let leading = (j as i32 * LEADING_GAP).min(MAX_LEADING_GAP);
            score[0][j] = bonus(0, j) - leading;
        }
    }
    for i in 1..n {
        // Best `score[i-1][k] + GAP·k` over k ≤ j-2, for gapped predecessors.
        let mut best_gapped: Option<(i32, usize)> = None;
        for j in i..m {
            if j >= 2 {
                let k = j - 2;
                if score[i - 1][k] > NONE {
                    let value = score[i - 1][k] + GAP * k as i32;
                    if best_gapped.is_none_or(|(best, _)| value > best) {
                        best_gapped = Some((value, k));
                    }
                }
            }
            if text_lower[j] != query_lower[i] {
                continue;
            }
            let consecutive =
                (score[i - 1][j - 1] > NONE).then(|| (score[i - 1][j - 1] + CONSECUTIVE, j - 1));
            let gapped = best_gapped.map(|(value, k)| (value - GAP * (j as i32 - 1), k));
            let best = match (consecutive, gapped) {
                (Some(a), Some(b)) => Some(if a.0 >= b.0 { a } else { b }),
                (a, b) => a.or(b),
            };
            if let Some((value, k)) = best {
                score[i][j] = value + bonus(i, j);
                back[i][j] = k;
            }
        }
    }

    let (end, &best) = score[n - 1]
        .iter()
        .enumerate()
        .filter(|(_, value)| **value > NONE)
        .max_by_key(|(j, value)| (**value, std::cmp::Reverse(*j)))?;
    let mut positions = vec![0; n];
    let mut j = end;
    for i in (0..n).rev() {
        positions[i] = j;
        j = back[i][j];
    }
    // Shorter candidates win ties.
    let length_penalty = ((m - n) as i32).min(40) / 4;
    Some(FuzzyMatch {
        score: best - length_penalty,
        positions,
    })
}

/// Matches `query` against every candidate and returns `(index, match)`
/// best first (ties: shorter, then earlier candidates first). An empty query
/// keeps the candidates' order.
pub fn rank<'a>(
    query: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Vec<(usize, FuzzyMatch)> {
    let mut matches: Vec<(usize, usize, FuzzyMatch)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(ix, candidate)| {
            fuzzy_match(query, candidate).map(|found| (ix, candidate.chars().count(), found))
        })
        .collect();
    if query.trim().is_empty() {
        return matches
            .into_iter()
            .map(|(ix, _, found)| (ix, found))
            .collect();
    }
    matches.sort_by(|(a_ix, a_len, a), (b_ix, b_len, b)| {
        b.score
            .cmp(&a.score)
            .then(a_len.cmp(b_len))
            .then(a_ix.cmp(b_ix))
    });
    matches
        .into_iter()
        .map(|(ix, _, found)| (ix, found))
        .collect()
}

/// Byte ranges of `text` covering the char `positions`, merged into runs,
/// for highlighting.
pub fn highlight_ranges(text: &str, positions: &[usize]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut wanted = positions.iter().copied().peekable();
    for (char_ix, (byte_ix, c)) in text.char_indices().enumerate() {
        if wanted.peek() != Some(&char_ix) {
            continue;
        }
        wanted.next();
        let end = byte_ix + c.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == byte_ix => last.end = end,
            _ => ranges.push(byte_ix..end),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranked<'a>(query: &str, candidates: &[&'a str]) -> Vec<&'a str> {
        rank(query, candidates.iter().copied())
            .into_iter()
            .map(|(ix, _)| candidates[ix])
            .collect()
    }

    #[test]
    fn matches_subsequences_ignoring_case_and_query_spaces() {
        let found = fuzzy_match("frz", "Freeze recording").unwrap();
        assert_eq!(found.positions, [0, 1, 4]);
        assert!(fuzzy_match("FREEZE", "freeze").is_some());
        assert!(fuzzy_match("show aud", "Show Audit").is_some());
        assert_eq!(fuzzy_match("xyz", "Freeze recording"), None);
        assert_eq!(fuzzy_match("long query", "short"), None);
        assert_eq!(
            fuzzy_match("", "anything").map(|found| found.score),
            Some(0)
        );
    }

    #[test]
    fn prefix_matches_rank_above_matches_inside_words() {
        assert_eq!(
            ranked("el", &["Show panel", "Elements"]),
            ["Elements", "Show panel"]
        );
    }

    #[test]
    fn contiguous_matches_rank_above_scattered_ones() {
        assert_eq!(
            ranked("box", &["Block outline x", "Show box model"]),
            ["Show box model", "Block outline x"]
        );
    }

    #[test]
    fn word_boundaries_and_camel_humps_beat_mid_word_hits() {
        assert_eq!(
            ranked("pf", &["Stop frames", "Toggle paint flashing"])[0],
            "Toggle paint flashing"
        );
        let found = fuzzy_match("il", "IsolatedList").unwrap();
        assert_eq!(
            found.positions,
            [0, 8],
            "the camel hump beats the nearer `l`"
        );
    }

    #[test]
    fn prefers_the_best_alignment_not_the_first() {
        // Greedy matching would take the first `s`, inside "Dismiss".
        let found = fuzzy_match("sa", "Dismiss Show Audit").unwrap();
        assert_eq!(found.positions, [8, 13]);
    }

    #[test]
    fn empty_queries_keep_the_candidate_order() {
        assert_eq!(
            ranked("", &["Show Elements", "Show"]),
            ["Show Elements", "Show"]
        );
    }

    #[test]
    fn shorter_candidates_win_ties() {
        assert_eq!(
            ranked("show", &["Show Elements", "Show"]),
            ["Show", "Show Elements"]
        );
    }

    #[test]
    fn highlight_ranges_merge_runs_and_respect_utf8() {
        assert_eq!(highlight_ranges("Freeze", &[0, 1, 4]), [0..2, 4..5]);
        assert_eq!(highlight_ranges("é-box", &[0, 2, 3]), [0..2, 3..5]);
        assert!(highlight_ranges("abc", &[]).is_empty());
    }
}
