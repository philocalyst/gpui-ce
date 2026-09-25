//! Human-readable formatting for durations, counts, sizes and source locations.
//!
//! Every number Loupe shows goes through here, so the whole UI agrees on
//! precision and units: `412 µs`, `7.9 ms`, `1.24 s`, `1,284`, `2.1 MB`,
//! `320×24`, `issue_list.rs:52`.

use gpui::{
    Hsla, Pixels, Point, Size, hsla_to_rgba,
    inspector::{ElementKind, ElementRecord, short_type_name},
};
use std::{borrow::Cow, panic::Location, time::Duration};

const NANOS_PER_MICRO: f64 = 1e3;
const NANOS_PER_MILLI: f64 = 1e6;
const NANOS_PER_SECOND: f64 = 1e9;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
const HOURS_PER_DAY: u64 = 24;

/// Below this, a relative time reads "just now" rather than a number.
const JUST_NOW: Duration = Duration::from_millis(100);
/// Below this, relative times keep one decimal (`2.1 s ago`).
const PRECISE_AGO: Duration = Duration::from_secs(10);

/// Shown for values that cannot be expressed as a number (NaN).
pub const NO_VALUE: &str = "–";

/// Formats a duration with three significant digits in the most readable unit:
/// `850 ns`, `412 µs`, `7.9 ms`, `23.4 ms`, `1.24 s`, `12.4 s`, `2m 05s`, `1h 02m`.
///
/// The unit is picked after rounding, so `999.97 µs` reads `1.0 ms`, never
/// `1000 µs`.
pub fn duration(duration: Duration) -> String {
    let nanos = duration.as_nanos() as f64;
    if nanos < NANOS_PER_MICRO {
        return format!("{nanos} ns");
    }
    let micros = (nanos / NANOS_PER_MICRO).round();
    if micros < 1_000.0 {
        return format!("{micros} µs");
    }
    if let Some(millis) = rounded_below(nanos / NANOS_PER_MILLI, 1, 1_000.0) {
        return format!("{millis:.1} ms");
    }
    let seconds = nanos / NANOS_PER_SECOND;
    if let Some(seconds) = rounded_below(seconds, 2, 10.0) {
        return format!("{seconds:.2} s");
    }
    if let Some(seconds) = rounded_below(seconds, 1, SECONDS_PER_MINUTE as f64) {
        return format!("{seconds:.1} s");
    }
    const HALF_SECOND_NANOS: u32 = 500_000_000;
    let whole_seconds = duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() >= HALF_SECOND_NANOS));
    if whole_seconds < SECONDS_PER_HOUR {
        let (minutes, seconds) = (
            whole_seconds / SECONDS_PER_MINUTE,
            whole_seconds % SECONDS_PER_MINUTE,
        );
        return format!("{minutes}m {seconds:02}s");
    }
    let whole_minutes = whole_seconds / SECONDS_PER_MINUTE;
    let (hours, minutes) = (
        whole_minutes / SECONDS_PER_MINUTE,
        whole_minutes % SECONDS_PER_MINUTE,
    );
    format!("{hours}h {minutes:02}m")
}

/// Formats a duration as a bare number of milliseconds with one decimal
/// (`3.1`), for dense places that print the unit once, like the status bar's
/// `app 3.1 · loupe 0.4 ms`.
pub fn millis(duration: Duration) -> String {
    format!("{:.1}", duration.as_nanos() as f64 / NANOS_PER_MILLI)
}

/// Rounds `value` to `decimals` places and returns it if the rounded value is
/// still below `limit`, so callers can move to the next unit otherwise.
fn rounded_below(value: f64, decimals: i32, limit: f64) -> Option<f64> {
    let scale = 10f64.powi(decimals);
    let rounded = (value * scale).round() / scale;
    (rounded < limit).then_some(rounded)
}

/// Formats a count with thousands separators: `1,284`.
pub fn count(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Formats a byte count with SI units (powers of 1000): `512 B`, `2.1 kB`,
/// `12.3 MB`, `123 MB`, `1.5 GB`.
///
/// One decimal below 100 of a unit, whole numbers above; the unit is chosen
/// after rounding so `999.96 kB` reads `1.0 MB`.
pub fn bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    const STEP: f64 = 1_000.0;
    const DECIMAL_LIMIT: f64 = 100.0;

    if bytes < STEP as u64 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / STEP;
    let mut unit = 0;
    loop {
        if let Some(rounded) = rounded_below(value, 1, DECIMAL_LIMIT) {
            return format!("{rounded:.1} {}", UNITS[unit]);
        }
        if let Some(rounded) = rounded_below(value, 0, STEP) {
            return format!("{rounded} {}", UNITS[unit]);
        }
        if unit + 1 == UNITS.len() {
            return format!("{} {}", value.round(), UNITS[unit]);
        }
        value /= STEP;
        unit += 1;
    }
}

/// Formats a logical length as a bare number (Loupe shows every length in
/// logical pixels): whole numbers print without decimals (`320`), others with
/// one (`12.5`). NaN prints [`NO_VALUE`].
pub fn number(value: f32) -> String {
    if value.is_nan() {
        return NO_VALUE.to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "∞" } else { "-∞" }.to_string();
    }
    let tenths = (f64::from(value) * 10.0).round();
    if tenths == 0.0 {
        // Avoids printing `-0`.
        "0".to_string()
    } else if tenths % 10.0 == 0.0 {
        format!("{}", tenths / 10.0)
    } else {
        format!("{:.1}", tenths / 10.0)
    }
}

/// Formats a length in logical pixels as a bare number; see [`number`].
pub fn pixels(pixels: Pixels) -> String {
    number(pixels.as_f32())
}

/// Formats a size as `width×height`: `320×24`.
pub fn size(size: Size<Pixels>) -> String {
    format!("{}×{}", pixels(size.width), pixels(size.height))
}

/// Formats a point as `(x, y)`: `(120, 44)`.
pub fn point(point: Point<Pixels>) -> String {
    format!("({}, {})", pixels(point.x), pixels(point.y))
}

/// Formats a color as hex: `#3366ff`, or `#3366ff80` when translucent.
pub fn color(color: Hsla) -> String {
    let rgba = hsla_to_rgba(color);
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (red, green, blue, alpha) = (
        byte(rgba.color.red),
        byte(rgba.color.green),
        byte(rgba.color.blue),
        byte(rgba.alpha),
    );
    if alpha == u8::MAX {
        format!("#{red:02x}{green:02x}{blue:02x}")
    } else {
        format!("#{red:02x}{green:02x}{blue:02x}{alpha:02x}")
    }
}

/// Formats how long ago `then` happened, seen from `now` (both offsets from
/// the capture epoch): `just now`, `2.1 s ago`, `14 s ago`, `3 min ago`,
/// `2 h ago`, `4 d ago`. A `then` after `now` reads `just now`.
pub fn relative_time(now: Duration, then: Duration) -> String {
    let elapsed = now.saturating_sub(then);
    if elapsed < JUST_NOW {
        return "just now".to_string();
    }
    if elapsed < PRECISE_AGO {
        return format!("{:.1} s ago", elapsed.as_secs_f64());
    }
    let seconds = elapsed.as_secs();
    if seconds < SECONDS_PER_MINUTE {
        format!("{seconds} s ago")
    } else if seconds < SECONDS_PER_HOUR {
        format!("{} min ago", seconds / SECONDS_PER_MINUTE)
    } else if seconds < SECONDS_PER_HOUR * HOURS_PER_DAY {
        format!("{} h ago", seconds / SECONDS_PER_HOUR)
    } else {
        format!("{} d ago", seconds / (SECONDS_PER_HOUR * HOURS_PER_DAY))
    }
}

/// Formats a fraction (`0.62`) as a percentage: `62%` from 10% up, `4.2%`
/// below, `<0.1%` for tiny non-zero values. Non-finite input prints
/// [`NO_VALUE`].
pub fn percent(fraction: f64) -> String {
    const SMALLEST_SHOWN: f64 = 0.1;
    const WHOLE_FROM: f64 = 10.0;

    if !fraction.is_finite() {
        return NO_VALUE.to_string();
    }
    let percent = fraction * 100.0;
    if percent == 0.0 {
        "0%".to_string()
    } else if percent.abs() < SMALLEST_SHOWN - f64::EPSILON {
        if percent > 0.0 { "<0.1%" } else { ">-0.1%" }.to_string()
    } else if percent.abs() < WHOLE_FROM - 0.05 {
        format!("{percent:.1}%")
    } else {
        format!("{percent:.0}%")
    }
}

/// Formats a source location compactly as `file.rs:line`: `issue_list.rs:52`.
/// This is the label Loupe prints next to everything it shows.
pub fn location(location: &Location<'_>) -> String {
    let file = location.file();
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    format!("{name}:{}", location.line())
}

/// Formats a source location with a shortened path, for tooltips and
/// disambiguation: `gpui/elements/div.rs:1204`. See [`short_path`].
pub fn location_with_path(location: &Location<'_>) -> String {
    format!("{}:{}", short_path(location.file()), location.line())
}

/// Shortens a source path to what a reader needs to recognize it:
///
/// * workspace crates: `crates/gpui/src/elements/div.rs` → `gpui/elements/div.rs`
/// * registry crates: `~/.cargo/registry/src/index…/serde-1.0.2/src/de.rs` → `serde/de.rs`
/// * git dependencies: `~/.cargo/git/checkouts/zed-1a2b/3c4d/crates/gpui/src/app.rs` →
///   `gpui/app.rs`
/// * the standard library: `/rustc/<hash>/library/core/src/ops/function.rs` →
///   `core/ops/function.rs`
/// * an application's own sources: `src/issue_list.rs` → `issue_list.rs`
/// * any other absolute path keeps its last two components: `…/proj/main.rs`
pub fn short_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let path = normalized.as_str();

    if let Some(rest) = after(path, "/registry/src/") {
        // `<index>/<crate>-<version>/<file>`
        let mut parts = rest.splitn(3, '/');
        if let (Some(_index), Some(package), Some(file)) =
            (parts.next(), parts.next(), parts.next())
        {
            return join_crate(crate_name(package), file);
        }
    }
    if let Some(rest) = after(path, "/git/checkouts/") {
        // `<repo>-<hash>/<rev>/<file>`
        let mut parts = rest.splitn(3, '/');
        if let (Some(repo), Some(_rev), Some(file)) = (parts.next(), parts.next(), parts.next()) {
            if let Some(short) = workspace_crate_path(file) {
                return short;
            }
            let repo = repo.rsplit_once('-').map_or(repo, |(name, _hash)| name);
            return join_crate(repo, file);
        }
    }
    if let Some(rest) = after(path, "/library/").filter(|_| path.starts_with("/rustc/")) {
        if let Some((krate, file)) = rest.split_once('/') {
            return join_crate(krate, file);
        }
    }
    if let Some(short) = workspace_crate_path(path) {
        return short;
    }
    if let Some(file) = path.strip_prefix("src/") {
        return file.to_string();
    }
    if path.starts_with('/') || path.get(1..3) == Some(":/") {
        let mut components = path.rsplit('/').filter(|component| !component.is_empty());
        if let (Some(file), Some(dir)) = (components.next(), components.next()) {
            return format!("…/{dir}/{file}");
        }
    }
    path.to_string()
}

/// `crates/<name>/[src/]<file>` anywhere in the path → `<name>/<file>`.
fn workspace_crate_path(path: &str) -> Option<String> {
    let rest = if let Some(rest) = path.strip_prefix("crates/") {
        rest
    } else {
        after(path, "/crates/")?
    };
    let (krate, file) = rest.split_once('/')?;
    Some(join_crate(krate, file))
}

fn join_crate(krate: &str, file: &str) -> String {
    let file = file.strip_prefix("src/").unwrap_or(file);
    format!("{krate}/{file}")
}

/// The text after the last occurrence of `needle`.
fn after<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
    haystack
        .rfind(needle)
        .map(|ix| &haystack[ix + needle.len()..])
}

/// `serde_json-1.0.144` → `serde_json`, `tree-sitter-0.20.1-alpha` → `tree-sitter`.
fn crate_name(package: &str) -> &str {
    let bytes = package.as_bytes();
    (0..bytes.len().saturating_sub(1))
        .find(|&ix| bytes[ix] == b'-' && bytes[ix + 1].is_ascii_digit())
        .map_or(package, |ix| &package[..ix])
}

/// A short label for an element, spelled like the code that built it:
/// `div#close`, `uniform_list`, `IssueList` (views and components keep their
/// type name; plain elements use the snake-case name of their builder).
pub fn element_label(record: &ElementRecord) -> String {
    let name = match record.kind {
        ElementKind::Element { type_name } => snake_case(short_type_name(type_name)),
        ElementKind::View { type_name, .. } | ElementKind::Component { type_name } => {
            self::type_name(type_name).into_owned()
        }
    };
    match &record.id {
        Some(id) => format!("{name}#{id}"),
        None => name,
    }
}

/// `UniformList` → `uniform_list`, `HTMLView` → `html_view`.
fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut snake = String::with_capacity(name.len() + 4);
    for (ix, &char) in chars.iter().enumerate() {
        if char.is_uppercase() && ix > 0 {
            let previous_lower = chars[ix - 1].is_lowercase();
            let next_lower = chars.get(ix + 1).is_some_and(|next| next.is_lowercase());
            if previous_lower || (chars[ix - 1].is_uppercase() && next_lower) {
                snake.push('_');
            }
        }
        snake.extend(char.to_lowercase());
    }
    snake
}

/// A readable version of a `type_name`: module paths are dropped from every
/// path in it, generic arguments are kept (and shortened), and closures keep
/// their enclosing function:
///
/// * `gpui::elements::div::Div` → `Div`
/// * `app::List<app::Row, alloc::string::String>` → `List<Row, String>`
/// * `app::build::{{closure}}` → `build::{closure}`
/// * `alloc::sync::Arc<dyn app::Store>` → `Arc<dyn Store>`
///
/// Plain paths reuse [`short_type_name`] without allocating.
pub fn type_name(type_name: &'static str) -> Cow<'static, str> {
    const PATH_DELIMITERS: &[char] = &['<', '>', ',', '(', ')', '[', ']', ';', '&', '*', ' '];

    if !type_name.contains(PATH_DELIMITERS) && !type_name.contains('{') {
        return Cow::Borrowed(short_type_name(type_name));
    }
    let mut short = String::with_capacity(type_name.len());
    let mut rest = type_name;
    while !rest.is_empty() {
        let token_len = rest.find(PATH_DELIMITERS).unwrap_or(rest.len());
        if token_len == 0 {
            let delimiter_len = rest.chars().next().map_or(1, char::len_utf8);
            short.push_str(&rest[..delimiter_len]);
            rest = &rest[delimiter_len..];
        } else {
            push_short_path(&mut short, &rest[..token_len]);
            rest = &rest[token_len..];
        }
    }
    Cow::Owned(short)
}

/// Appends the readable form of one `a::b::C` path token.
fn push_short_path(out: &mut String, path: &str) {
    const CLOSURE: &str = "{{closure}}";

    if path.starts_with("::") {
        // An associated item after a qualified path: `<A as B>::Item`.
        out.push_str("::");
    }
    let mut segments = path.rsplit("::").filter(|segment| !segment.is_empty());
    let Some(last) = segments.next() else {
        return;
    };
    if last != CLOSURE {
        out.push_str(last);
        return;
    }
    match segments.find(|segment| *segment != CLOSURE) {
        Some(function) => {
            out.push_str(function);
            out.push_str("::{closure}");
        }
        None => out.push_str("{closure}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point as gpui_point, px, size as gpui_size};

    fn micros(micros: u64) -> Duration {
        Duration::from_micros(micros)
    }

    #[test]
    fn durations_pick_the_readable_unit() {
        assert_eq!(duration(Duration::ZERO), "0 ns");
        assert_eq!(duration(Duration::from_nanos(850)), "850 ns");
        assert_eq!(duration(micros(1)), "1 µs");
        assert_eq!(duration(micros(412)), "412 µs");
        assert_eq!(duration(Duration::from_nanos(412_499)), "412 µs");
        assert_eq!(duration(micros(7_900)), "7.9 ms");
        assert_eq!(duration(micros(23_449)), "23.4 ms");
        assert_eq!(duration(micros(412_300)), "412.3 ms");
        assert_eq!(duration(Duration::from_millis(1_240)), "1.24 s");
        assert_eq!(duration(Duration::from_millis(12_400)), "12.4 s");
        assert_eq!(duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(duration(Duration::from_secs(3_720)), "1h 02m");
    }

    #[test]
    fn durations_move_to_the_next_unit_after_rounding() {
        assert_eq!(duration(Duration::from_nanos(999_600)), "1.0 ms");
        assert_eq!(duration(Duration::from_nanos(999_960_000)), "1.00 s");
        assert_eq!(duration(Duration::from_nanos(9_999_000_000)), "10.0 s");
        assert_eq!(duration(Duration::from_nanos(59_990_000_000)), "1m 00s");
        assert_eq!(duration(Duration::from_millis(3_599_600)), "1h 00m");
    }

    #[test]
    fn huge_durations_do_not_panic() {
        assert!(duration(Duration::MAX).ends_with('m'));
        assert_eq!(duration(Duration::from_secs(90 * 3_600)), "90h 00m");
    }

    #[test]
    fn millis_is_a_bare_number() {
        assert_eq!(millis(micros(3_140)), "3.1");
        assert_eq!(millis(Duration::ZERO), "0.0");
        assert_eq!(millis(Duration::from_secs(2)), "2000.0");
    }

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_284), "1,284");
        assert_eq!(count(1_000_000), "1,000,000");
        assert_eq!(count(u64::MAX), "18,446,744,073,709,551,615");
    }

    #[test]
    fn bytes_use_si_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_000), "1.0 kB");
        assert_eq!(bytes(2_100_000), "2.1 MB");
        assert_eq!(bytes(12_345_678), "12.3 MB");
        assert_eq!(bytes(123_456_789), "123 MB");
        assert_eq!(bytes(999_960), "1.0 MB");
        assert_eq!(bytes(1_500_000_000), "1.5 GB");
        assert_eq!(bytes(u64::MAX), "18446744 TB");
    }

    #[test]
    fn numbers_drop_needless_decimals() {
        assert_eq!(number(320.0), "320");
        assert_eq!(number(12.5), "12.5");
        assert_eq!(number(12.04), "12");
        assert_eq!(number(-0.01), "0");
        assert_eq!(number(-3.5), "-3.5");
        assert_eq!(number(f32::NAN), NO_VALUE);
        assert_eq!(number(f32::INFINITY), "∞");
        assert_eq!(number(f32::NEG_INFINITY), "-∞");
        assert_eq!(number(1e9), "1000000000");
    }

    #[test]
    fn sizes_and_points() {
        assert_eq!(size(gpui_size(px(320.), px(24.))), "320×24");
        assert_eq!(size(gpui_size(px(0.), px(10.5))), "0×10.5");
        assert_eq!(point(gpui_point(px(120.), px(44.))), "(120, 44)");
        assert_eq!(pixels(px(7.25)), "7.3");
    }

    #[test]
    fn colors_print_as_hex() {
        use gpui::{hsla, rgb, rgb_to_hsla, rgba};
        assert_eq!(color(rgb_to_hsla(rgb(0x3366ff))), "#3366ff");
        assert_eq!(color(rgb_to_hsla(rgba(0x3366ff80))), "#3366ff80");
        assert_eq!(color(hsla(0., 0., 0., 0.)), "#00000000");
        assert_eq!(color(gpui::white()), "#ffffff");
    }

    #[test]
    fn relative_times() {
        let now = Duration::from_secs(1_000);
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now, now + micros(5)), "just now");
        assert_eq!(
            relative_time(now, now - Duration::from_millis(2_100)),
            "2.1 s ago"
        );
        assert_eq!(
            relative_time(now, now - Duration::from_secs(14)),
            "14 s ago"
        );
        assert_eq!(
            relative_time(now, now - Duration::from_secs(185)),
            "3 min ago"
        );
        assert_eq!(
            relative_time(Duration::from_secs(10 * 3_600), Duration::ZERO),
            "10 h ago"
        );
        assert_eq!(
            relative_time(Duration::from_secs(5 * 86_400), Duration::ZERO),
            "5 d ago"
        );
    }

    #[test]
    fn percentages() {
        assert_eq!(percent(0.62), "62%");
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(1.5), "150%");
        assert_eq!(percent(0.042), "4.2%");
        assert_eq!(percent(0.0999), "10%");
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.000_01), "<0.1%");
        assert_eq!(percent(-0.000_01), ">-0.1%");
        assert_eq!(percent(f64::NAN), NO_VALUE);
        assert_eq!(percent(f64::INFINITY), NO_VALUE);
    }

    #[test]
    fn locations_print_file_and_line() {
        let here = Location::caller();
        assert_eq!(location(here), format!("format.rs:{}", here.line()));
        assert_eq!(
            location_with_path(here),
            format!("gpui_inspector/analysis/format.rs:{}", here.line())
        );
    }

    #[test]
    fn paths_are_shortened() {
        let cases = [
            ("crates/gpui/src/elements/div.rs", "gpui/elements/div.rs"),
            ("crates/gpui/examples/hello.rs", "gpui/examples/hello.rs"),
            ("/work/zed/crates/editor/src/editor.rs", "editor/editor.rs"),
            (
                "/home/me/.cargo/registry/src/index.crates.io-6f17d22bba15001f/serde_json-1.0.144/src/de.rs",
                "serde_json/de.rs",
            ),
            (
                "/home/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tree-sitter-0.20.10/binding_rust/lib.rs",
                "tree-sitter/binding_rust/lib.rs",
            ),
            (
                "/home/me/.cargo/git/checkouts/zed-a1b2c3d4/5e6f7a8/crates/gpui/src/app.rs",
                "gpui/app.rs",
            ),
            (
                "/home/me/.cargo/git/checkouts/taffy-a1b2c3d4/5e6f7a8/src/tree/mod.rs",
                "taffy/tree/mod.rs",
            ),
            (
                "/rustc/90b35a6239c3d8bdabc530a6a0816f7ff89a0aaf/library/core/src/ops/function.rs",
                "core/ops/function.rs",
            ),
            ("src/issue_list.rs", "issue_list.rs"),
            ("examples/todo.rs", "examples/todo.rs"),
            ("/home/me/proj/main.rs", "…/proj/main.rs"),
            ("C:\\Users\\me\\proj\\main.rs", "…/proj/main.rs"),
            ("main.rs", "main.rs"),
            ("", ""),
            ("crates/ünïcødé/src/файл.rs", "ünïcødé/файл.rs"),
        ];
        for (path, expected) in cases {
            assert_eq!(short_path(path), expected, "{path}");
        }
    }

    #[test]
    fn element_labels_read_like_the_builder() {
        use crate::analysis::fixtures::{TreeBuilder, bounds, entity};

        let mut builder = TreeBuilder::new();
        let root = builder.root(bounds(0., 0., 10., 10.));
        let list = builder.child(root, bounds(0., 0., 10., 10.));
        let view = builder.child(root, bounds(0., 0., 10., 10.));
        let html = builder.child(root, bounds(0., 0., 10., 10.));
        builder.record(root).id = Some("close".into());
        builder.record(list).kind = ElementKind::Element {
            type_name: "gpui::elements::uniform_list::UniformList",
        };
        builder.record(view).kind = ElementKind::View {
            entity: entity(1),
            type_name: "app::IssueList<app::Row>",
        };
        builder.record(html).kind = ElementKind::Component {
            type_name: "app::HTMLView",
        };
        builder.record(html).id = Some("ünï".into());
        let tree = builder.build();

        let labels: Vec<String> = tree.elements.iter().map(element_label).collect();
        assert_eq!(
            labels,
            vec![
                "div#close",
                "uniform_list",
                "IssueList<Row>",
                "HTMLView#ünï"
            ]
        );
        assert_eq!(snake_case("HTMLView"), "html_view");
        assert_eq!(snake_case("Svg"), "svg");
        assert_eq!(snake_case(""), "");
    }

    #[test]
    fn plain_type_names_are_borrowed() {
        assert!(matches!(
            type_name("gpui::elements::div::Div"),
            Cow::Borrowed("Div")
        ));
        assert_eq!(type_name("Div"), "Div");
    }

    #[test]
    fn type_names_keep_generics_and_closures_readable() {
        let cases = [
            (
                "app::List<app::Row, alloc::string::String>",
                "List<Row, String>",
            ),
            ("app::build::{{closure}}", "build::{closure}"),
            ("app::build::{{closure}}::{{closure}}", "build::{closure}"),
            ("{{closure}}", "{closure}"),
            ("alloc::sync::Arc<dyn app::Store>", "Arc<dyn Store>"),
            ("&'static str", "&'static str"),
            ("[u8; 4]", "[u8; 4]"),
            ("(app::A, core::option::Option<app::B>)", "(A, Option<B>)"),
            ("<app::A as app::Tr>::Assoc", "<A as Tr>::Assoc"),
            ("app::Ünïcødé<app::日本>", "Ünïcødé<日本>"),
        ];
        for (name, expected) in cases {
            assert_eq!(type_name(name), expected, "{name}");
        }
    }
}
