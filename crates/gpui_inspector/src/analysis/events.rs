//! The Events lens' logic: which input records the log shows, how each one
//! reads, and the one-sentence story of where it went.
//!
//! The log filters the capture's input ring (up to 1,000 records) on every
//! keystroke in its filter field, so each record's searchable text is built
//! once, incrementally, by a [`LogIndex`].

use crate::analysis::{
    filter::{TextFilter, haystack},
    format,
    sentence::Sentence,
};
use gpui::{
    KeyContext, SharedString,
    inspector::{ActionRecord, ElementKey, InputKind, InputRecord},
};
use std::{collections::VecDeque, time::Duration};

/// Which kinds of input the log shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KindFilter {
    /// Everything.
    #[default]
    All,
    /// Key presses, releases and modifier changes.
    Keys,
    /// Pointer input: clicks, moves, scrolls, gestures, touches, file drops.
    Mouse,
    /// Input that dispatched an action, and actions dispatched on their own.
    Actions,
}

impl KindFilter {
    /// Every filter, in the order the chips show them.
    pub const ALL: [KindFilter; 4] = [
        KindFilter::All,
        KindFilter::Keys,
        KindFilter::Mouse,
        KindFilter::Actions,
    ];

    /// The chip label.
    pub fn label(self) -> &'static str {
        match self {
            KindFilter::All => "All",
            KindFilter::Keys => "Keys",
            KindFilter::Mouse => "Mouse",
            KindFilter::Actions => "Actions",
        }
    }

    /// Whether `record` is of a kind this filter shows.
    pub fn matches(self, record: &InputRecord) -> bool {
        match self {
            KindFilter::All => true,
            KindFilter::Keys => is_key(record.kind),
            KindFilter::Mouse => is_pointer(record.kind),
            KindFilter::Actions => record.kind == InputKind::Action || !record.actions.is_empty(),
        }
    }
}

/// Whether `kind` is keyboard input.
pub fn is_key(kind: InputKind) -> bool {
    matches!(
        kind,
        InputKind::KeyDown | InputKind::KeyUp | InputKind::Modifiers
    )
}

/// Whether `kind` is pointer input.
pub fn is_pointer(kind: InputKind) -> bool {
    matches!(
        kind,
        InputKind::MouseDown
            | InputKind::MouseUp
            | InputKind::MouseMove
            | InputKind::MouseExit
            | InputKind::Scroll
            | InputKind::Gesture
            | InputKind::Touch
            | InputKind::FileDrop
    )
}

/// A short, lowercase name for `kind`, as searched and shown in tooltips.
pub fn kind_label(kind: InputKind) -> &'static str {
    match kind {
        InputKind::KeyDown => "key down",
        InputKind::KeyUp => "key up",
        InputKind::Modifiers => "modifiers",
        InputKind::MouseDown => "mouse down",
        InputKind::MouseUp => "mouse up",
        InputKind::MouseMove => "mouse move",
        InputKind::MouseExit => "mouse exit",
        InputKind::Scroll => "scroll",
        InputKind::Gesture => "gesture",
        InputKind::Touch => "touch",
        InputKind::FileDrop => "file drop",
        InputKind::Action => "action",
    }
}

/// Everything the log filters by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogFilter {
    /// The kind chip.
    pub kind: KindFilter,
    /// The filter field.
    pub text: TextFilter,
    /// Whether input that Loupe itself consumed is listed.
    pub show_loupe: bool,
}

impl LogFilter {
    /// Whether `record`, whose searchable text is `haystack`, is listed.
    pub fn matches(&self, record: &InputRecord, haystack: &str) -> bool {
        (self.show_loupe || !record.inspector)
            && self.kind.matches(record)
            && self.text.matches(haystack)
    }
}

/// Searchable text for every record in the capture's input ring, kept in
/// step with the ring incrementally: new records are indexed once, the
/// newest is re-indexed (pointer moves coalesce into it), and records that
/// fell out of the ring are dropped.
#[derive(Debug, Default)]
pub struct LogIndex {
    entries: VecDeque<(u64, String)>,
}

impl LogIndex {
    /// Catches up with `input` (oldest first, consecutive sequence numbers).
    /// `name` names an element of a hit path, when it can.
    pub fn sync(
        &mut self,
        input: &VecDeque<InputRecord>,
        name: impl Fn(ElementKey) -> Option<String>,
    ) {
        let (Some(first), Some(last)) = (input.front(), input.back()) else {
            self.entries.clear();
            return;
        };
        let replaced = match (self.entries.front(), self.entries.back()) {
            (Some((front, _)), Some((back, _))) => *front > first.seq || *back > last.seq,
            _ => false,
        };
        if replaced {
            self.entries.clear();
        }
        while self
            .entries
            .front()
            .is_some_and(|(seq, _)| *seq < first.seq)
        {
            self.entries.pop_front();
        }
        // The newest record may have absorbed more moves since it was indexed.
        self.entries.pop_back();
        let next = self
            .entries
            .back()
            .map_or(first.seq, |(seq, _)| seq + 1);
        let skip = next.saturating_sub(first.seq) as usize;
        for record in input.iter().skip(skip) {
            self.entries.push_back((record.seq, record_haystack(record, &name)));
        }
    }

    /// Sequence numbers of the records `filter` keeps, oldest first.
    pub fn visible(&self, input: &VecDeque<InputRecord>, filter: &LogFilter) -> Vec<u64> {
        input
            .iter()
            .zip(&self.entries)
            .filter(|(record, (seq, haystack))| {
                record.seq == *seq && filter.matches(record, haystack)
            })
            .map(|(record, _)| record.seq)
            .collect()
    }

    /// Number of indexed records.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The searchable text of `record`: its kind, detail, keystroke, actions
/// (with their bindings), key contexts and the elements it hit.
pub fn record_haystack(
    record: &InputRecord,
    name: impl Fn(ElementKey) -> Option<String>,
) -> String {
    let keystroke = record.keystroke.as_ref().map(|keystroke| keystroke.unparse());
    let contexts: Vec<String> = record.context_stack.iter().map(context_label).collect();
    let elements: Vec<String> = record.hit_path.iter().filter_map(|&key| name(key)).collect();
    let actions = record.actions.iter().flat_map(|action| {
        [
            action.name,
            action.keystrokes.as_deref().unwrap_or_default(),
            action.context.as_deref().unwrap_or_default(),
        ]
    });
    haystack(
        [kind_label(record.kind), record.detail.as_ref()]
            .into_iter()
            .chain(keystroke.as_deref())
            .chain(actions)
            .chain(contexts.iter().map(String::as_str))
            .chain(elements.iter().map(String::as_str)),
    )
}

/// The record at `seq` in `input` (consecutive sequence numbers).
pub fn record_at(input: &VecDeque<InputRecord>, seq: u64) -> Option<&InputRecord> {
    let first = input.front()?.seq;
    let ix = seq.checked_sub(first)?;
    input
        .get(usize::try_from(ix).ok()?)
        .filter(|record| record.seq == seq)
}

/// An offset from the capture epoch as a compact clock: `12.480`,
/// `1:02.480`, `1:02:03.480`.
pub fn clock(at: Duration) -> String {
    let millis = at.as_millis();
    let (seconds, millis) = (millis / 1000, millis % 1000);
    let (minutes, seconds) = (seconds / 60, seconds % 60);
    let (hours, minutes) = (minutes / 60, minutes % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}.{millis:03}")
    } else if minutes > 0 {
        format!("{minutes}:{seconds:02}.{millis:03}")
    } else {
        format!("{seconds}.{millis:03}")
    }
}

/// A key context as written in keymaps: `Editor mode=full`.
pub fn context_label(context: &KeyContext) -> String {
    format!("{context:?}")
}

/// A context stack, outermost first: `Workspace > Pane > Editor`.
pub fn context_path(stack: &[KeyContext]) -> String {
    stack
        .iter()
        .map(context_label)
        .collect::<Vec<_>>()
        .join(" > ")
}

/// The keys of a key record for key caps, and a note on how the keymap
/// treated them (`pending`, `held`), if any.
pub fn record_keys(record: &InputRecord) -> Option<(SharedString, Option<&'static str>)> {
    if !matches!(record.kind, InputKind::KeyDown | InputKind::KeyUp) {
        return None;
    }
    let keys: SharedString = match &record.keystroke {
        Some(keystroke) => keystroke.unparse().into(),
        None => record.detail.clone(),
    };
    let note = if record.detail.ends_with("(pending)") {
        Some("pending")
    } else if record.detail.ends_with("(held)") {
        Some("held")
    } else if record.kind == InputKind::KeyUp {
        Some("up")
    } else {
        None
    };
    Some((keys, note))
}

/// The action that answers "what did this input do": the first handled
/// one, else the first dispatched.
pub fn primary_action(record: &InputRecord) -> Option<&ActionRecord> {
    record
        .actions
        .iter()
        .find(|action| action.handled)
        .or_else(|| record.actions.first())
}

/// What became of the frame after an input record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameLink {
    /// Handling it invalidated the window, and this frame redrew it.
    Caused(u64),
    /// It invalidated the window; the frame is not drawn yet.
    Pending,
    /// This frame came next, but the input did not invalidate anything.
    Followed(u64),
    /// Nothing was redrawn.
    None,
}

impl FrameLink {
    /// The link for `record`.
    pub fn of(record: &InputRecord) -> Self {
        match (record.caused_redraw, record.frame) {
            (true, Some(frame)) => FrameLink::Caused(frame),
            (true, None) => FrameLink::Pending,
            (false, Some(frame)) => FrameLink::Followed(frame),
            (false, None) => FrameLink::None,
        }
    }
}

/// The one-sentence story of `record`, e.g. "`cmd-k` → `workspace::OpenPalette`
/// in `Workspace > Pane` · handled in 400 µs". `target` names the topmost
/// element under the pointer.
pub fn record_sentence(record: &InputRecord, target: Option<&str>) -> Sentence {
    let mut sentence = match record.kind {
        InputKind::KeyDown => key_down_sentence(record),
        InputKind::KeyUp => {
            let keys = record_keys(record).map_or(record.detail.clone(), |(keys, _)| keys);
            Sentence::new().code(keys).text(" released")
        }
        InputKind::Modifiers => Sentence::new()
            .text("Modifiers changed to ")
            .code(record.detail.clone()),
        InputKind::Action => Sentence::new().code(record.detail.clone()).text(
            if record.handled {
                " dispatched"
            } else {
                " dispatched, but nothing handles it"
            },
        ),
        InputKind::MouseMove => {
            let count = record.coalesced.max(1);
            let moves = if count == 1 {
                "Moved ".to_string()
            } else {
                format!("{} moves ", format::count(u64::from(count)))
            };
            Sentence::new().text(moves + &record.detail)
        }
        _ => Sentence::new().text(capitalized(&record.detail)),
    };

    if let Some(target) = target.filter(|_| record.position.is_some()) {
        let preposition = match record.kind {
            InputKind::MouseDown | InputKind::MouseUp => " on ",
            _ => " over ",
        };
        sentence = sentence.text(preposition).code(target.to_string());
    }
    if record.kind != InputKind::KeyDown
        && let Some(action) = primary_action(record)
    {
        sentence = sentence.text(" → ").code(action.name);
        if !action.handled {
            sentence = sentence.text(" (unhandled)");
        }
    }
    if matches!(record.kind, InputKind::KeyDown | InputKind::Action)
        && !record.context_stack.is_empty()
    {
        sentence = sentence
            .text(" in ")
            .code(context_path(&record.context_stack));
    }

    let took = format::duration(record.duration);
    let outcome = match record.kind {
        InputKind::MouseMove if record.coalesced > 1 => format!(" · {took} in total"),
        InputKind::MouseMove => format!(" · {took}"),
        _ if record.handled => format!(" · handled in {took}"),
        _ => format!(" · not handled ({took})"),
    };
    sentence.text(outcome)
}

fn key_down_sentence(record: &InputRecord) -> Sentence {
    let (keys, note) = record_keys(record).unwrap_or((record.detail.clone(), None));
    let sentence = Sentence::new().code(keys).text(" → ");
    if note == Some("pending") {
        return sentence.text("waiting for the next key of a longer binding");
    }
    match primary_action(record) {
        Some(action) if action.handled => {
            let sentence = sentence.code(action.name);
            match record.actions.len() - 1 {
                0 => sentence,
                more => sentence.text(format!(" (+{more} more)")),
            }
        }
        Some(action) => sentence.text("no handler for ").code(action.name),
        None if record.handled => sentence.text("handled by a key listener or text input"),
        None => sentence.text("no binding; nothing handled it"),
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{input, ms, us};
    use gpui::{Keystroke, point, px};

    /// An unhandled record that took 80 µs and drew nothing.
    fn blank(seq: u64, kind: InputKind) -> InputRecord {
        InputRecord {
            handled: false,
            caused_redraw: false,
            duration: us(80),
            ..input(seq, ms(seq as f64), kind, None)
        }
    }

    fn key(seq: u64, keys: &str) -> InputRecord {
        let mut record = blank(seq, InputKind::KeyDown);
        record.keystroke = Keystroke::parse(keys).ok();
        record.detail = keys.to_string().into();
        record
    }

    fn with_action(mut record: InputRecord, name: &'static str, handled: bool) -> InputRecord {
        record.actions.push(ActionRecord {
            name,
            handled,
            keystrokes: record.keystroke.as_ref().map(|k| k.unparse().into()),
            context: Some("Editor".into()),
        });
        record.handled |= handled;
        record
    }

    fn contexts(record: &mut InputRecord, stack: &[&str]) {
        record.context_stack = stack
            .iter()
            .map(|context| KeyContext::parse(context).unwrap())
            .collect();
    }

    fn click(seq: u64) -> InputRecord {
        let mut record = blank(seq, InputKind::MouseDown);
        record.detail = "left ×1 at (120, 44)".into();
        record.position = Some(point(px(120.), px(44.)));
        record
    }

    #[test]
    fn kind_filters_split_keys_pointers_and_actions() {
        let key = key(0, "ctrl-k");
        let click = click(1);
        let action = with_action(click.clone(), "inbox::Open", true);
        let standalone = blank(2, InputKind::Action);
        let filters = |record: &InputRecord| -> Vec<KindFilter> {
            KindFilter::ALL
                .into_iter()
                .filter(|filter| filter.matches(record))
                .collect()
        };
        assert_eq!(filters(&key), [KindFilter::All, KindFilter::Keys]);
        assert_eq!(filters(&click), [KindFilter::All, KindFilter::Mouse]);
        assert_eq!(
            filters(&action),
            [KindFilter::All, KindFilter::Mouse, KindFilter::Actions]
        );
        assert_eq!(filters(&standalone), [KindFilter::All, KindFilter::Actions]);
    }

    #[test]
    fn log_filters_hide_loupe_input_unless_asked_and_honor_exclusions() {
        let mut ours = click(0);
        ours.inspector = true;
        let theirs = with_action(key(1, "ctrl-k"), "workspace::OpenPalette", true);
        let mut filter = LogFilter::default();
        let hay = |record: &InputRecord| record_haystack(record, |_| None);
        assert!(!filter.matches(&ours, &hay(&ours)));
        assert!(filter.matches(&theirs, &hay(&theirs)));
        filter.show_loupe = true;
        assert!(filter.matches(&ours, &hay(&ours)));
        filter.text = TextFilter::parse("palette -mouse");
        assert!(filter.matches(&theirs, &hay(&theirs)));
        assert!(!filter.matches(&ours, &hay(&ours)));
        filter.text = TextFilter::parse("-palette");
        assert!(!filter.matches(&theirs, &hay(&theirs)));
    }

    #[test]
    fn haystacks_cover_keys_actions_contexts_and_hit_elements() {
        let mut record = with_action(key(0, "ctrl-s"), "mail::Save", true);
        contexts(&mut record, &["Workspace", "Editor mode=full"]);
        let element = ElementKey {
            path: gpui::inspector::PathKey(4),
            instance: 0,
        };
        record.hit_path.push(element);
        let hay = record_haystack(&record, |key| {
            (key == element).then(|| "div#save-button".to_string())
        });
        for needle in [
            "key down",
            "ctrl-s",
            "mail::save",
            "editor mode=full",
            "div#save-button",
        ] {
            assert!(hay.contains(needle), "{needle:?} in {hay:?}");
        }
    }

    #[test]
    fn the_index_follows_the_ring_incrementally() {
        let mut ring: VecDeque<InputRecord> = (0..3).map(|seq| key(seq, "a")).collect();
        let mut index = LogIndex::default();
        let all = LogFilter::default();
        index.sync(&ring, |_| None);
        assert_eq!(index.visible(&ring, &all), [0, 1, 2]);

        // New records are appended, trimmed ones dropped.
        ring.push_back(key(3, "b"));
        ring.pop_front();
        index.sync(&ring, |_| None);
        assert_eq!(index.len(), 3);
        assert_eq!(index.visible(&ring, &all), [1, 2, 3]);

        // The newest record is re-indexed: moves coalesce into it.
        ring.back_mut().unwrap().detail = "zebra".into();
        index.sync(&ring, |_| None);
        let zebra = LogFilter {
            text: TextFilter::parse("zebra"),
            ..LogFilter::default()
        };
        assert_eq!(index.visible(&ring, &zebra), [3]);

        // A replaced ring (a cleared or fixture capture) starts over.
        let replaced: VecDeque<InputRecord> = (0..2).map(|seq| key(seq, "c")).collect();
        index.sync(&replaced, |_| None);
        assert_eq!(index.visible(&replaced, &all), [0, 1]);
        index.sync(&VecDeque::new(), |_| None);
        assert!(index.is_empty());
    }

    #[test]
    fn records_are_found_by_sequence_number() {
        let ring: VecDeque<InputRecord> = (5..9).map(|seq| key(seq, "a")).collect();
        assert_eq!(record_at(&ring, 7).map(|record| record.seq), Some(7));
        assert!(record_at(&ring, 4).is_none());
        assert!(record_at(&ring, 9).is_none());
    }

    #[test]
    fn clocks_are_compact_and_grow_units_as_needed() {
        assert_eq!(clock(ms(0.)), "0.000");
        assert_eq!(clock(ms(12_480.)), "12.480");
        assert_eq!(clock(ms(62_480.)), "1:02.480");
        assert_eq!(clock(ms(3_723_004.)), "1:02:03.004");
    }

    #[test]
    fn key_sentences_say_what_ran_where() {
        let mut record = with_action(key(0, "ctrl-k"), "workspace::OpenPalette", true);
        contexts(&mut record, &["Workspace", "Pane"]);
        record.duration = Duration::from_micros(400);
        assert_eq!(
            record_sentence(&record, None).to_string(),
            "`ctrl-k` → `workspace::OpenPalette` in `Workspace > Pane` · handled in 400 µs"
        );

        let mut unhandled = with_action(key(1, "ctrl-u"), "mail::Orphan", false);
        contexts(&mut unhandled, &["Editor"]);
        assert_eq!(
            record_sentence(&unhandled, None).to_string(),
            "`ctrl-u` → no handler for `mail::Orphan` in `Editor` · not handled (80 µs)"
        );

        let mut pending = key(2, "ctrl-k");
        pending.detail = "ctrl-k (pending)".into();
        pending.handled = true;
        assert_eq!(
            record_sentence(&pending, None).to_string(),
            "`ctrl-k` → waiting for the next key of a longer binding · handled in 80 µs"
        );

        let typed = InputRecord {
            handled: true,
            ..key(3, "a")
        };
        assert!(
            record_sentence(&typed, None)
                .to_string()
                .starts_with("`a` → handled by a key listener or text input")
        );
        assert!(
            record_sentence(&key(4, "q"), None)
                .to_string()
                .starts_with("`q` → no binding; nothing handled it")
        );

        let mut two = with_action(key(5, "enter"), "a::First", true);
        two = with_action(two, "a::Second", true);
        assert!(
            record_sentence(&two, None)
                .to_string()
                .starts_with("`enter` → `a::First` (+1 more)")
        );
    }

    #[test]
    fn pointer_sentences_name_the_target_and_the_action() {
        let record = with_action(click(0), "inbox::OpenIssue", true);
        assert_eq!(
            record_sentence(&record, Some("div#row-3")).to_string(),
            "Left ×1 at (120, 44) on `div#row-3` → `inbox::OpenIssue` · handled in 80 µs"
        );

        let mut moves = blank(1, InputKind::MouseMove);
        moves.detail = "to (300, 400)".into();
        moves.position = Some(point(px(300.), px(400.)));
        moves.coalesced = 12;
        assert_eq!(
            record_sentence(&moves, Some("IssueList")).to_string(),
            "12 moves to (300, 400) over `IssueList` · 80 µs in total"
        );
        moves.coalesced = 1;
        assert!(
            record_sentence(&moves, None)
                .to_string()
                .starts_with("Moved to (300, 400) ·")
        );
        let unhandled = click(2);
        assert!(
            record_sentence(&unhandled, None)
                .to_string()
                .ends_with("· not handled (80 µs)")
        );
    }

    #[test]
    fn key_records_expose_their_keys_and_how_the_keymap_treated_them() {
        let mut held = key(0, "j");
        held.detail = "j (held)".into();
        assert_eq!(record_keys(&held), Some(("j".into(), Some("held"))));
        let mut up = key(1, "j");
        up.kind = InputKind::KeyUp;
        assert_eq!(record_keys(&up), Some(("j".into(), Some("up"))));
        assert_eq!(record_keys(&click(2)), None);
    }

    #[test]
    fn frame_links_distinguish_caused_from_followed() {
        let mut record = click(0);
        record.frame = Some(9);
        assert_eq!(FrameLink::of(&record), FrameLink::Followed(9));
        record.caused_redraw = true;
        assert_eq!(FrameLink::of(&record), FrameLink::Caused(9));
        record.frame = None;
        assert_eq!(FrameLink::of(&record), FrameLink::Pending);
        record.caused_redraw = false;
        assert_eq!(FrameLink::of(&record), FrameLink::None);
    }
}
