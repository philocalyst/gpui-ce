//! The style grid: a [`StyleRefinement`] as editable rows, and edits back.
//!
//! The refinement is serialized to JSON (which omits unset fields) and
//! flattened into one [`PropertyRow`] per set value, addressed by a JSON
//! pointer (`/padding/left`). An edit writes a value at a pointer and
//! deserializes the result, so every edit is validated by the same serde code
//! that defines the style, and the grid never needs a hand-written schema of
//! gpui's style fields beyond how to label, group and parse them.

use super::format;
use gpui::{Background, Hsla, Refineable as _, StyleRefinement, solid_background};
use serde_json::{Map, Value, json};
use std::fmt;

/// Where a property belongs in the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PropertyGroup {
    /// Display, visibility, overflow, grid.
    Layout,
    /// Flex direction, wrap, grow, shrink, basis, alignment, gap.
    Flex,
    /// Margin and padding.
    Spacing,
    /// Size, min and max size, aspect ratio.
    Size,
    /// Position and insets.
    Position,
    /// Background, borders, corners, shadows, rings, filters, opacity, cursor.
    Visual,
    /// Text style.
    Text,
    /// Anything else.
    Other,
}

impl PropertyGroup {
    /// The group's heading.
    pub fn label(self) -> &'static str {
        match self {
            PropertyGroup::Layout => "Layout",
            PropertyGroup::Flex => "Flex",
            PropertyGroup::Spacing => "Spacing",
            PropertyGroup::Size => "Size",
            PropertyGroup::Position => "Position",
            PropertyGroup::Visual => "Visual",
            PropertyGroup::Text => "Text",
            PropertyGroup::Other => "Other",
        }
    }
}

/// A property's value, typed for editing (scrubbing numbers, picking colors
/// and enum options).
#[derive(Clone, Debug, PartialEq)]
pub enum PropertyValue {
    /// Logical pixels.
    Pixels(f32),
    /// Rems (relative to the window's rem size).
    Rems(f32),
    /// A fraction of the parent, 1.0 = 100%.
    Relative(f32),
    /// `auto`.
    Auto,
    /// A plain number (grow factor, opacity, font weight, counts).
    Number(f64),
    /// A solid color.
    Color(Hsla),
    /// One of a fixed set of options (`Flex`, `Column`, `PointingHand`...).
    Enum {
        /// The current option.
        value: String,
        /// Every valid option.
        options: &'static [&'static str],
    },
    /// A flag.
    Bool(bool),
    /// Free text (font family).
    Text(String),
    /// Anything the grid cannot edit field by field (shadows, gradients,
    /// grid placement), as JSON.
    Other(Value),
}

impl fmt::Display for PropertyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropertyValue::Pixels(value) => write!(f, "{value}px"),
            PropertyValue::Rems(value) => write!(f, "{value}rem"),
            PropertyValue::Relative(fraction) => write!(f, "{}%", fraction * 100.0),
            PropertyValue::Auto => write!(f, "auto"),
            PropertyValue::Number(value) => write!(f, "{value}"),
            PropertyValue::Color(color) => write!(f, "{}", format::color(*color)),
            PropertyValue::Enum { value, .. } => write!(f, "{value}"),
            PropertyValue::Bool(value) => write!(f, "{value}"),
            PropertyValue::Text(text) => write!(f, "{text:?}"),
            PropertyValue::Other(value) => write!(f, "{value}"),
        }
    }
}

/// One set property of a style.
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyRow {
    /// JSON pointer into the serialized refinement: `/padding/left`.
    pub path: String,
    /// Human label: `padding.left`.
    pub label: String,
    /// Where it belongs in the grid.
    pub group: PropertyGroup,
    /// Its value.
    pub value: PropertyValue,
}

/// A property whose value differs between two styles.
#[derive(Clone, Debug, PartialEq)]
pub struct RowChange {
    /// JSON pointer.
    pub path: String,
    /// Human label.
    pub label: String,
    /// Where it belongs in the grid.
    pub group: PropertyGroup,
    /// The value before, if it was set.
    pub before: Option<PropertyValue>,
    /// The value after, if it is set.
    pub after: Option<PropertyValue>,
}

/// Why an edit could not be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StyleEditError {
    /// Not a JSON pointer into the style (`/padding/left`), or it goes
    /// through a value that has no fields.
    InvalidPath(String),
    /// The value cannot be written as JSON (NaN or infinite numbers).
    InvalidValue {
        /// Where it was written.
        path: String,
        /// The value.
        value: String,
    },
    /// The edited style is not a valid [`StyleRefinement`]: wrong type,
    /// unknown option, missing field or unknown property.
    Rejected {
        /// Where the edit was made.
        path: String,
        /// What serde reported.
        reason: String,
    },
}

impl fmt::Display for StyleEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StyleEditError::InvalidPath(path) => write!(f, "`{path}` is not a style property"),
            StyleEditError::InvalidValue { path, value } => {
                write!(f, "{value} is not a valid value for `{path}`")
            }
            StyleEditError::Rejected { path, reason } => {
                write!(f, "`{path}` cannot be set that way: {reason}")
            }
        }
    }
}

impl std::error::Error for StyleEditError {}

/// Enum properties and their options, as serde spells them.
const ENUMS: &[(&str, &[&str])] = &[
    ("/display", &["Block", "Flex", "Grid", "None"]),
    ("/visibility", &["Visible", "Hidden"]),
    ("/overflow/x", OVERFLOW),
    ("/overflow/y", OVERFLOW),
    ("/position", &["Relative", "Absolute"]),
    ("/align_items", ALIGN_ITEMS),
    ("/align_self", ALIGN_ITEMS),
    ("/align_content", ALIGN_CONTENT),
    ("/justify_content", ALIGN_CONTENT),
    (
        "/flex_direction",
        &["Row", "Column", "RowReverse", "ColumnReverse"],
    ),
    ("/flex_wrap", &["NoWrap", "Wrap", "WrapReverse"]),
    ("/border_style", &["Solid", "Dashed"]),
    ("/mouse_cursor", CURSORS),
    ("/ring/color", &["CurrentColor"]),
    ("/inset_ring/color", &["CurrentColor"]),
    ("/grid_cols/min_size", GRID_MIN_SIZE),
    ("/grid_rows/min_size", GRID_MIN_SIZE),
    ("/text/font_style", &["Normal", "Italic", "Oblique"]),
    ("/text/white_space", &["Normal", "Nowrap"]),
    ("/text/text_align", &["Left", "Center", "Right"]),
    (
        "/text/text_transform",
        &["None", "Uppercase", "Lowercase", "Capitalize"],
    ),
];
const OVERFLOW: &[&str] = &["Visible", "Clip", "Hidden", "Scroll"];
const ALIGN_ITEMS: &[&str] = &[
    "Start",
    "End",
    "FlexStart",
    "FlexEnd",
    "Center",
    "Baseline",
    "Stretch",
];
const ALIGN_CONTENT: &[&str] = &[
    "Start",
    "End",
    "FlexStart",
    "FlexEnd",
    "Center",
    "Stretch",
    "SpaceBetween",
    "SpaceEvenly",
    "SpaceAround",
];
const GRID_MIN_SIZE: &[&str] = &["Zero", "MinContent", "MaxContent"];
const CURSORS: &[&str] = &[
    "Arrow",
    "IBeam",
    "Crosshair",
    "ClosedHand",
    "OpenHand",
    "PointingHand",
    "ResizeLeft",
    "ResizeRight",
    "ResizeLeftRight",
    "ResizeUp",
    "ResizeDown",
    "ResizeUpDown",
    "ResizeUpLeftDownRight",
    "ResizeUpRightDownLeft",
    "ResizeColumn",
    "ResizeRow",
    "IBeamCursorForVerticalLayout",
    "OperationNotAllowed",
    "DragLink",
    "DragCopy",
    "ContextualMenu",
];

/// Top-level properties whose string values are lengths (`12px`, `0.5rem`,
/// `50%`, `auto`).
const LENGTH_PROPERTIES: &[&str] = &[
    "inset",
    "size",
    "min_size",
    "max_size",
    "margin",
    "padding",
    "border_widths",
    "gap",
    "corner_radii",
    "flex_basis",
    "scrollbar_width",
];
/// Text properties that are lengths.
const TEXT_LENGTHS: &[&str] = &["/text/font_size", "/text/line_height"];
/// Properties serialized as bare numbers that are logical pixels.
const PIXEL_NUMBERS: &[&str] = &[
    "/ring/width",
    "/inset_ring/width",
    "/text/letter_spacing",
    "/text/underline/thickness",
    "/text/strikethrough/thickness",
];
/// Properties shown as one [`PropertyValue::Other`] row rather than field by field.
const OPAQUE_PROPERTIES: &[&str] = &[
    "/grid_location",
    "/text/font_features",
    "/text/font_fallbacks",
];
/// Where a color is stored as a `Fill` or `RingColor` (`{"Color": background}`).
const WRAPPED_COLORS: &[&str] = &["/background", "/ring/color", "/inset_ring/color"];
/// Where a color is stored as a bare `Background`.
const BACKGROUND_COLORS: &[&str] = &["/border_color"];

/// Every set property of `style`, in the style's field order.
pub fn rows(style: &StyleRefinement) -> Vec<PropertyRow> {
    let mut rows = Vec::new();
    if let Ok(json) = to_json(style) {
        flatten(&json, &mut String::new(), &mut rows);
    }
    rows
}

/// `style` with the property at `path` set to `value`. Setting a property to
/// the value it already has returns `style` unchanged.
pub fn set(
    style: &StyleRefinement,
    path: &str,
    value: &PropertyValue,
) -> Result<StyleRefinement, StyleEditError> {
    pointer_tokens(path)?;
    let json = to_json(style)?;
    let existing = json.pointer(path).filter(|existing| !existing.is_null());
    if existing.is_some_and(|existing| classify(path, existing) == *value) {
        return Ok(style.clone());
    }
    write(style, path, value)
}

/// Writes `value` at `path` and validates the result.
fn write(
    style: &StyleRefinement,
    path: &str,
    value: &PropertyValue,
) -> Result<StyleRefinement, StyleEditError> {
    let tokens = pointer_tokens(path)?;
    let mut json = to_json(style)?;
    let existing = json.pointer(path).filter(|existing| !existing.is_null());
    let encoded = encode(path, value, existing)?;
    *slot(&mut json, &tokens, path)? = encoded;
    let edited = from_json(json, path)?;
    let written = to_json(&edited)?;
    if written.pointer(path).is_none_or(Value::is_null) {
        return Err(StyleEditError::InvalidPath(path.to_string()));
    }
    Ok(edited)
}

/// `style` with the property at `path` unset (reverted to inherited or
/// default). Removing a property that is not set returns `style` unchanged.
pub fn remove(style: &StyleRefinement, path: &str) -> Result<StyleRefinement, StyleEditError> {
    let tokens = pointer_tokens(path)?;
    let Some(last) = tokens.last() else {
        return Err(StyleEditError::InvalidPath(path.to_string()));
    };
    let mut json = to_json(style)?;
    let parent_path = &path[..path.rfind('/').unwrap_or_default()];
    match json.pointer_mut(parent_path) {
        Some(Value::Object(map)) => {
            map.remove(last);
        }
        Some(Value::Array(items)) => {
            if let Some(ix) = last.parse::<usize>().ok().filter(|&ix| ix < items.len()) {
                items.remove(ix);
            }
        }
        _ => return Ok(style.clone()),
    }
    from_json(json, path)
}

/// `overrides` applied on top of `base`, as the engine applies inspector
/// overrides to an element's own style.
pub fn merge(base: &StyleRefinement, overrides: &StyleRefinement) -> StyleRefinement {
    let mut merged = base.clone();
    merged.refine(overrides);
    merged
}

/// The properties whose values differ between `before` and `after`: changed
/// and added ones in `after`'s order, then removed ones.
pub fn diff(before: &StyleRefinement, after: &StyleRefinement) -> Vec<RowChange> {
    let (before, after) = (rows(before), rows(after));
    let find = |rows: &[PropertyRow], path: &str| {
        rows.iter()
            .find(|row| row.path == path)
            .map(|row| row.value.clone())
    };
    let changed = after.iter().filter_map(|row| {
        let old = find(&before, &row.path);
        (old.as_ref() != Some(&row.value)).then(|| RowChange {
            path: row.path.clone(),
            label: row.label.clone(),
            group: row.group,
            before: old,
            after: Some(row.value.clone()),
        })
    });
    let removed = before
        .iter()
        .filter(|row| find(&after, &row.path).is_none())
        .map(|row| RowChange {
            path: row.path.clone(),
            label: row.label.clone(),
            group: row.group,
            before: Some(row.value.clone()),
            after: None,
        });
    changed.chain(removed).collect()
}

fn to_json(style: &StyleRefinement) -> Result<Value, StyleEditError> {
    serde_json::to_value(style).map_err(|error| StyleEditError::Rejected {
        path: String::new(),
        reason: error.to_string(),
    })
}

fn from_json(json: Value, path: &str) -> Result<StyleRefinement, StyleEditError> {
    serde_json::from_value(json).map_err(|error| StyleEditError::Rejected {
        path: path.to_string(),
        reason: error.to_string(),
    })
}

fn flatten(value: &Value, path: &mut String, rows: &mut Vec<PropertyRow>) {
    match value {
        Value::Null => {}
        Value::Object(map) if !is_leaf_object(path, map) => {
            for (key, child) in map {
                let len = path.len();
                path.push('/');
                path.push_str(&key.replace('~', "~0").replace('/', "~1"));
                flatten(child, path, rows);
                path.truncate(len);
            }
        }
        _ => rows.push(PropertyRow {
            path: path.clone(),
            label: label(path),
            group: group(path),
            value: classify(path, value),
        }),
    }
}

/// Objects shown as one row: colors, other backgrounds, enum variants with
/// data (`{"Truncate": "…"}`) and opaque properties.
fn is_leaf_object(path: &str, map: &Map<String, Value>) -> bool {
    let is_variant = map.len() == 1
        && map
            .keys()
            .next()
            .is_some_and(|key| key.starts_with(|c: char| c.is_ascii_uppercase()));
    is_variant || is_background(map) || is_palette_color(map) || OPAQUE_PROPERTIES.contains(&path)
}

fn is_background(map: &Map<String, Value>) -> bool {
    map.contains_key("tag") && map.contains_key("solid")
}

fn is_palette_color(map: &Map<String, Value>) -> bool {
    ["hue", "saturation", "lightness", "alpha"]
        .iter()
        .all(|key| map.contains_key(*key))
}

fn classify(path: &str, value: &Value) -> PropertyValue {
    if let Some(color) = decode_color(value) {
        return PropertyValue::Color(color);
    }
    match value {
        Value::Bool(flag) => PropertyValue::Bool(*flag),
        Value::Number(number) => {
            let number = number.as_f64().unwrap_or_default();
            if PIXEL_NUMBERS.contains(&path) {
                PropertyValue::Pixels(number as f32)
            } else {
                PropertyValue::Number(number)
            }
        }
        Value::String(text) => classify_string(path, text),
        _ => PropertyValue::Other(value.clone()),
    }
}

fn classify_string(path: &str, text: &str) -> PropertyValue {
    if let Some(options) = enum_options(path) {
        return PropertyValue::Enum {
            value: text.to_string(),
            options,
        };
    }
    if is_length(path)
        && let Some(length) = parse_length(text)
    {
        return length;
    }
    PropertyValue::Text(text.to_string())
}

fn enum_options(path: &str) -> Option<&'static [&'static str]> {
    ENUMS
        .iter()
        .find(|(enum_path, _)| *enum_path == path)
        .map(|(_, options)| *options)
}

fn is_length(path: &str) -> bool {
    let top = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    LENGTH_PROPERTIES.contains(&top) || TEXT_LENGTHS.contains(&path)
}

/// `auto`, `12px`, `0.5rem`, `50%`.
fn parse_length(text: &str) -> Option<PropertyValue> {
    if text == "auto" {
        return Some(PropertyValue::Auto);
    }
    if let Some(number) = text.strip_suffix("px") {
        return number.parse().ok().map(PropertyValue::Pixels);
    }
    if let Some(number) = text.strip_suffix("rem") {
        return number.parse().ok().map(PropertyValue::Rems);
    }
    let percent: f32 = text.strip_suffix('%')?.parse().ok()?;
    Some(PropertyValue::Relative(percent / 100.0))
}

/// A solid color, in any of the shapes a style stores one: a palette `Hsla`,
/// a `Background`, or either wrapped as `{"Color": …}` (`Fill`, `RingColor`).
fn decode_color(value: &Value) -> Option<Hsla> {
    let map = value.as_object()?;
    if map.len() == 1
        && let Some(inner) = map.get("Color")
    {
        return decode_color(inner);
    }
    if is_background(map) {
        let background: Background = serde_json::from_value(value.clone()).ok()?;
        return background.as_solid();
    }
    if is_palette_color(map) {
        return serde_json::from_value(value.clone()).ok();
    }
    None
}

fn encode(
    path: &str,
    value: &PropertyValue,
    existing: Option<&Value>,
) -> Result<Value, StyleEditError> {
    let invalid = || StyleEditError::InvalidValue {
        path: path.to_string(),
        value: value.to_string(),
    };
    if let PropertyValue::Pixels(length)
    | PropertyValue::Rems(length)
    | PropertyValue::Relative(length) = value
        && !length.is_finite()
    {
        return Err(invalid());
    }
    Ok(match value {
        PropertyValue::Pixels(pixels) => {
            if existing.is_some_and(Value::is_number) || PIXEL_NUMBERS.contains(&path) {
                number(f64::from(*pixels)).ok_or_else(invalid)?
            } else {
                json!(format!("{pixels}px"))
            }
        }
        PropertyValue::Rems(rems) => json!(format!("{rems}rem")),
        PropertyValue::Relative(fraction) => json!(format!("{}%", fraction * 100.0)),
        PropertyValue::Auto => json!("auto"),
        PropertyValue::Number(value) => number(*value).ok_or_else(invalid)?,
        PropertyValue::Color(color) => encode_color(path, *color, existing).ok_or_else(invalid)?,
        PropertyValue::Enum { value, .. } => json!(value),
        PropertyValue::Bool(flag) => json!(flag),
        PropertyValue::Text(text) => json!(text),
        PropertyValue::Other(value) => value.clone(),
    })
}

/// A JSON number; integral values are written as integers so they also fit
/// integer fields (`line_clamp`, grid repeats).
fn number(value: f64) -> Option<Value> {
    const EXACT_INTEGER_LIMIT: f64 = 9_007_199_254_740_992.0;
    if value.fract() == 0.0 && value.abs() < EXACT_INTEGER_LIMIT {
        Some(json!(value as i64))
    } else {
        serde_json::Number::from_f64(value).map(Value::Number)
    }
}

/// Encodes `color` in the shape the property stores: that of the value it
/// replaces, or the known shape of the property.
fn encode_color(path: &str, color: Hsla, existing: Option<&Value>) -> Option<Value> {
    let background = || serde_json::to_value(solid_background(color)).ok();
    let existing_map = existing.and_then(Value::as_object);
    let wrapped = match existing_map {
        Some(map) if map.len() == 1 && map.contains_key("Color") => true,
        Some(map) if is_background(map) => false,
        Some(map) if is_palette_color(map) => return serde_json::to_value(color).ok(),
        _ => WRAPPED_COLORS.contains(&path),
    };
    if wrapped {
        Some(json!({ "Color": background()? }))
    } else if existing_map.is_some() || BACKGROUND_COLORS.contains(&path) {
        background()
    } else {
        serde_json::to_value(color).ok()
    }
}

/// The unescaped tokens of a JSON pointer.
fn pointer_tokens(path: &str) -> Result<Vec<String>, StyleEditError> {
    if !path.starts_with('/') {
        return Err(StyleEditError::InvalidPath(path.to_string()));
    }
    Ok(path[1..]
        .split('/')
        .map(|token| token.replace("~1", "/").replace("~0", "~"))
        .collect())
}

/// The value at `tokens`, creating objects along the way.
fn slot<'a>(
    mut value: &'a mut Value,
    tokens: &[String],
    path: &str,
) -> Result<&'a mut Value, StyleEditError> {
    let invalid = || StyleEditError::InvalidPath(path.to_string());
    for token in tokens {
        if value.is_null() {
            *value = Value::Object(Map::new());
        }
        value = match value {
            Value::Object(map) => map.entry(token.clone()).or_insert(Value::Null),
            Value::Array(items) => token
                .parse::<usize>()
                .ok()
                .and_then(|ix| items.get_mut(ix))
                .ok_or_else(invalid)?,
            _ => return Err(invalid()),
        };
    }
    Ok(value)
}

/// `/text/underline/color` → `text.underline.color`.
fn label(path: &str) -> String {
    pointer_tokens(path)
        .map(|tokens| tokens.join("."))
        .unwrap_or_else(|_| path.to_string())
}

fn group(path: &str) -> PropertyGroup {
    let top = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    match top {
        "display"
        | "visibility"
        | "overflow"
        | "scrollbar_width"
        | "allow_concurrent_scroll"
        | "restrict_scroll_to_axis"
        | "grid_cols"
        | "grid_rows"
        | "grid_location" => PropertyGroup::Layout,
        "flex_direction" | "flex_wrap" | "flex_basis" | "flex_grow" | "flex_shrink"
        | "align_items" | "align_self" | "align_content" | "justify_content" | "gap" => {
            PropertyGroup::Flex
        }
        "margin" | "padding" => PropertyGroup::Spacing,
        "size" | "min_size" | "max_size" | "aspect_ratio" => PropertyGroup::Size,
        "position" | "inset" => PropertyGroup::Position,
        "background"
        | "border_color"
        | "border_widths"
        | "border_style"
        | "border_dashed_length"
        | "border_dashed_gap"
        | "corner_radii"
        | "corner_smoothing"
        | "box_shadow"
        | "ring"
        | "inset_ring"
        | "filter"
        | "backdrop_filter"
        | "opacity"
        | "mouse_cursor" => PropertyGroup::Visual,
        "text" => PropertyGroup::Text,
        _ => PropertyGroup::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        AbsoluteLength, DefiniteLength, Display, Fill, FontWeight, Length, RingColor, Styled as _,
        linear_color_stop, linear_gradient, px, relative, rems, rgb, rgb_to_hsla,
    };

    fn row<'a>(rows: &'a [PropertyRow], path: &str) -> &'a PropertyRow {
        rows.iter()
            .find(|row| row.path == path)
            .unwrap_or_else(|| panic!("no row {path} in {rows:#?}"))
    }

    fn hex(value: &PropertyValue) -> String {
        match value {
            PropertyValue::Color(color) => format::color(*color),
            other => panic!("not a color: {other:?}"),
        }
    }

    /// Styles built with the real builder methods, covering every shape of
    /// value a refinement serializes to.
    fn styles() -> Vec<StyleRefinement> {
        let base = StyleRefinement::default;
        vec![
            base(),
            base().p_2().m_neg_1().mx_auto(),
            base()
                .w(px(120.))
                .h_full()
                .min_w(rems(2.))
                .max_h(relative(0.5))
                .size_1_3(),
            base()
                .flex()
                .flex_col()
                .gap(px(6.))
                .flex_1()
                .items_center()
                .justify_between(),
            base()
                .flex_grow(2.5)
                .flex_shrink_0()
                .flex_basis(px(40.))
                .flex_wrap(),
            base().absolute().top(px(3.)).inset_0().left_auto(),
            base()
                .bg(rgb(0x3366ff))
                .border_1()
                .border_color(rgb(0xff0000))
                .rounded_md(),
            base()
                .border_dashed()
                .border_dashed_length(2.)
                .rounded_smoothing_0p5(),
            base()
                .shadow_sm()
                .ring_2()
                .ring_color(rgb(0x00ff00))
                .inset_ring(px(1.5)),
            base()
                .blur(px(4.))
                .backdrop_blur(px(8.))
                .opacity(0.5)
                .cursor_pointer(),
            base()
                .bg(linear_gradient(
                    90.,
                    linear_color_stop(rgb(0xff0000), 0.),
                    linear_color_stop(rgb(0x0000ff), 1.),
                ))
                .invisible(),
            base()
                .text_color(rgb(0x112233))
                .text_sm()
                .font_family("Lilex ünïcødé")
                .font_weight(FontWeight::BOLD)
                .italic()
                .line_height(relative(1.5)),
            base()
                .underline()
                .text_decoration_wavy()
                .text_decoration_color(rgb(0xabcdef))
                .line_through()
                .letter_spacing(px(0.5)),
            base().truncate().text_center().line_clamp(3),
            base()
                .grid()
                .grid_cols(3)
                .grid_rows_min_content(2)
                .col_span(2)
                .aspect_square(),
            base()
                .overflow_x_hidden()
                .scrollbar_width(px(8.))
                .text_bg(rgb(0xffff00)),
        ]
    }

    #[test]
    fn an_empty_style_has_no_rows() {
        assert!(rows(&StyleRefinement::default()).is_empty());
    }

    #[test]
    fn rows_are_typed_labelled_and_grouped() {
        let style = StyleRefinement::default()
            .p_2()
            .mx_auto()
            .w(px(120.))
            .h_full()
            .flex()
            .flex_grow(2.)
            .bg(rgb(0x3366ff))
            .ring_2()
            .text_color(rgb(0x112233))
            .font_family("Lilex")
            .underline()
            .text_ellipsis()
            .cursor_pointer()
            .shadow_sm()
            .grid_cols(3)
            .col_span(2);
        let rows = rows(&style);

        let padding = row(&rows, "/padding/left");
        assert_eq!(padding.label, "padding.left");
        assert_eq!(padding.group, PropertyGroup::Spacing);
        assert_eq!(padding.value, PropertyValue::Rems(0.5));
        assert_eq!(row(&rows, "/margin/left").value, PropertyValue::Auto);
        assert_eq!(row(&rows, "/size/width").value, PropertyValue::Pixels(120.));
        assert_eq!(
            row(&rows, "/size/height").value,
            PropertyValue::Relative(1.)
        );
        assert_eq!(row(&rows, "/size/height").group, PropertyGroup::Size);
        assert_eq!(
            row(&rows, "/display").value,
            PropertyValue::Enum {
                value: "Flex".into(),
                options: &["Block", "Flex", "Grid", "None"]
            }
        );
        assert_eq!(row(&rows, "/flex_grow").value, PropertyValue::Number(2.));
        assert_eq!(row(&rows, "/flex_grow").group, PropertyGroup::Flex);

        let background = row(&rows, "/background");
        assert_eq!(background.label, "background");
        assert_eq!(background.group, PropertyGroup::Visual);
        assert_eq!(hex(&background.value), "#3366ff");
        assert_eq!(row(&rows, "/ring/width").value, PropertyValue::Pixels(2.));

        let text_color = row(&rows, "/text/color");
        assert_eq!(text_color.group, PropertyGroup::Text);
        assert_eq!(hex(&text_color.value), "#112233");
        assert_eq!(
            row(&rows, "/text/font_family").value,
            PropertyValue::Text("Lilex".into())
        );
        assert_eq!(
            row(&rows, "/text/underline/thickness").value,
            PropertyValue::Pixels(1.)
        );
        assert_eq!(
            row(&rows, "/text/underline/wavy").value,
            PropertyValue::Bool(false)
        );
        assert!(rows.iter().all(|row| row.path != "/text/underline/color"));
        assert_eq!(
            row(&rows, "/text/text_overflow").value,
            PropertyValue::Other(json!({ "Truncate": "…" }))
        );
        assert!(matches!(
            row(&rows, "/box_shadow").value,
            PropertyValue::Other(_)
        ));
        assert_eq!(
            row(&rows, "/grid_cols/repeat").value,
            PropertyValue::Number(3.)
        );
        assert!(matches!(
            row(&rows, "/grid_location").value,
            PropertyValue::Other(_)
        ));
        assert_eq!(row(&rows, "/grid_location").group, PropertyGroup::Layout);
    }

    #[test]
    fn gradients_are_not_colors() {
        let style = StyleRefinement::default().bg(linear_gradient(
            45.,
            linear_color_stop(rgb(0xff0000), 0.),
            linear_color_stop(rgb(0x0000ff), 1.),
        ));
        let rows = rows(&style);
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0].value, PropertyValue::Other(_)));
    }

    /// Rows compared with colors by hex, since HSL → RGB → HSL may move a
    /// channel by a float ulp without changing the color.
    fn comparable(rows: Vec<PropertyRow>) -> Vec<(String, String)> {
        rows.into_iter()
            .map(|row| match &row.value {
                PropertyValue::Color(_) => (row.path.clone(), hex(&row.value)),
                value => (row.path.clone(), format!("{value:?}")),
            })
            .collect()
    }

    #[test]
    fn writing_every_row_back_reproduces_the_style() {
        for style in styles() {
            let expected = comparable(rows(&style));
            for row in rows(&style) {
                let written = write(&style, &row.path, &row.value)
                    .unwrap_or_else(|error| panic!("{}: {error}", row.path));
                assert_eq!(comparable(rows(&written)), expected, "{}", row.path);
                assert_eq!(set(&style, &row.path, &row.value).as_ref(), Ok(&style));
            }
        }
    }

    #[test]
    fn serde_round_trips_every_style() {
        for style in styles() {
            let json = serde_json::to_value(&style).unwrap();
            let back: StyleRefinement = serde_json::from_value(json).unwrap();
            assert_eq!(rows(&back), rows(&style));
        }
    }

    #[test]
    fn set_writes_typed_values() {
        let style = StyleRefinement::default().p_2();
        let edited = set(&style, "/padding/left", &PropertyValue::Pixels(12.)).unwrap();
        assert_eq!(edited.padding.left, Some(DefiniteLength::from(px(12.))));
        assert_eq!(edited.padding.right, Some(DefiniteLength::from(rems(0.5))));

        let edited = set(&style, "/size/width", &PropertyValue::Auto).unwrap();
        assert_eq!(edited.size.width, Some(Length::Auto));
        let edited = set(&style, "/size/height", &PropertyValue::Relative(0.25)).unwrap();
        assert_eq!(edited.size.height, Some(Length::from(relative(0.25))));
        let edited = set(&style, "/text/font_size", &PropertyValue::Rems(1.25)).unwrap();
        assert_eq!(
            edited.text.font_size,
            Some(AbsoluteLength::from(rems(1.25)))
        );

        let grid = PropertyValue::Enum {
            value: "Grid".into(),
            options: &[],
        };
        assert_eq!(
            set(&style, "/display", &grid).unwrap().display,
            Some(Display::Grid)
        );
        let edited = set(&style, "/text/line_clamp", &PropertyValue::Number(3.)).unwrap();
        assert_eq!(edited.text.line_clamp, Some(3));
        let edited = set(&style, "/opacity", &PropertyValue::Number(0.25)).unwrap();
        assert_eq!(edited.opacity, Some(0.25));
        let edited = set(&style, "/text/letter_spacing", &PropertyValue::Pixels(1.5)).unwrap();
        assert_eq!(edited.text.letter_spacing, Some(px(1.5)));
        let font = PropertyValue::Text("Lilex ✓".into());
        let edited = set(&style, "/text/font_family", &font).unwrap();
        assert_eq!(edited.text.font_family.as_deref(), Some("Lilex ✓"));
    }

    #[test]
    fn set_writes_colors_in_the_shape_each_property_stores() {
        let blue = rgb_to_hsla(rgb(0x3366ff));
        let color = PropertyValue::Color(blue);
        let style = StyleRefinement::default();

        let edited = set(&style, "/background", &color).unwrap();
        let Some(Fill::Color(background)) = edited.background else {
            panic!("background not set: {edited:?}");
        };
        assert_eq!(
            background.as_solid().map(format::color).as_deref(),
            Some("#3366ff")
        );

        let edited = set(&style, "/border_color", &color).unwrap();
        let border = edited.border_color.and_then(|border| border.as_solid());
        assert_eq!(border.map(format::color).as_deref(), Some("#3366ff"));

        let edited = set(&style, "/ring/color", &color).unwrap();
        assert!(matches!(edited.ring.color, Some(RingColor::Color(_))));

        let edited = set(&style, "/text/color", &color).unwrap();
        assert_eq!(
            edited.text.color.map(format::color).as_deref(),
            Some("#3366ff")
        );

        // Replacing an existing color keeps its shape.
        let red = StyleRefinement::default().bg(rgb(0xff0000));
        let edited = set(&red, "/background", &color).unwrap();
        assert_eq!(hex(&row(&rows(&edited), "/background").value), "#3366ff");
    }

    #[test]
    fn invalid_edits_are_rejected() {
        let style = StyleRefinement::default().p_2().underline();
        let sideways = PropertyValue::Enum {
            value: "Sideways".into(),
            options: &[],
        };
        assert!(matches!(
            set(&style, "/display", &sideways),
            Err(StyleEditError::Rejected { .. })
        ));
        assert!(matches!(
            set(&style, "/padding/left", &PropertyValue::Pixels(f32::NAN)),
            Err(StyleEditError::InvalidValue { .. })
        ));
        assert!(matches!(
            set(
                &style,
                "/size/width",
                &PropertyValue::Relative(f32::INFINITY)
            ),
            Err(StyleEditError::InvalidValue { .. })
        ));
        // A grid template needs its repeat count too.
        let min_content = PropertyValue::Enum {
            value: "MinContent".into(),
            options: GRID_MIN_SIZE,
        };
        assert!(matches!(
            set(&style, "/grid_cols/min_size", &min_content),
            Err(StyleEditError::Rejected { .. })
        ));
        assert!(matches!(
            set(&style, "/opacity", &PropertyValue::Number(f64::INFINITY)),
            Err(StyleEditError::InvalidValue { .. })
        ));
        assert_eq!(
            set(&style, "padding/left", &PropertyValue::Auto),
            Err(StyleEditError::InvalidPath("padding/left".into()))
        );
        assert_eq!(
            set(&style, "/padding/left/deeper", &PropertyValue::Auto),
            Err(StyleEditError::InvalidPath("/padding/left/deeper".into()))
        );
        assert_eq!(
            set(&style, "/no_such_property", &PropertyValue::Number(1.)),
            Err(StyleEditError::InvalidPath("/no_such_property".into()))
        );
        let error = set(&style, "/display", &sideways).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("`/display` cannot be set that way: ")
        );
    }

    #[test]
    fn enum_options_are_all_valid() {
        // Grid templates only take a min size alongside their repeat count.
        let base = StyleRefinement::default().grid_cols(3).grid_rows(2);
        for (path, options) in ENUMS {
            for option in *options {
                let value = PropertyValue::Enum {
                    value: option.to_string(),
                    options,
                };
                let edited = set(&base, path, &value)
                    .unwrap_or_else(|error| panic!("{path} = {option}: {error}"));
                assert_eq!(row(&rows(&edited), path).value, value, "{path}");
            }
        }
    }

    #[test]
    fn remove_unsets_one_property() {
        let style = StyleRefinement::default().p_2().bg(rgb(0xff0000));
        let edited = remove(&style, "/padding/left").unwrap();
        assert_eq!(edited.padding.left, None);
        assert_eq!(edited.padding.top, Some(DefiniteLength::from(rems(0.5))));
        let edited = remove(&edited, "/background").unwrap();
        assert_eq!(edited.background, None);
        assert_eq!(remove(&style, "/margin/left").unwrap(), style);
        assert_eq!(remove(&style, "/nothing/here").unwrap(), style);
        assert!(matches!(remove(&style, "/"), Ok(_)));
        assert!(matches!(
            remove(&style, ""),
            Err(StyleEditError::InvalidPath(_))
        ));
        // A field that is required inside its value cannot be removed alone.
        let underlined = StyleRefinement::default().underline();
        assert!(matches!(
            remove(&underlined, "/text/underline/thickness"),
            Err(StyleEditError::Rejected { .. })
        ));
    }

    #[test]
    fn merge_applies_overrides_on_top() {
        let base = StyleRefinement::default().p_2().bg(rgb(0xff0000));
        let overrides = StyleRefinement::default().pl(px(12.)).flex();
        let merged = merge(&base, &overrides);
        assert_eq!(merged.padding.left, Some(DefiniteLength::from(px(12.))));
        assert_eq!(merged.padding.top, Some(DefiniteLength::from(rems(0.5))));
        assert_eq!(merged.display, Some(Display::Flex));
        assert!(merged.background.is_some());
        assert_eq!(merge(&base, &StyleRefinement::default()), base);
    }

    #[test]
    fn diff_lists_changed_added_and_removed_rows() {
        let before = StyleRefinement::default().p_2().bg(rgb(0xff0000));
        let after = StyleRefinement::default().p_2().pl(px(12.)).flex();
        let changes = diff(&before, &after);
        let summary: Vec<(&str, Option<String>, Option<String>)> = changes
            .iter()
            .map(|change| {
                (
                    change.label.as_str(),
                    change.before.as_ref().map(ToString::to_string),
                    change.after.as_ref().map(ToString::to_string),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("display", None, Some("Flex".into())),
                ("padding.left", Some("0.5rem".into()), Some("12px".into())),
                ("background", Some("#ff0000".into()), None),
            ]
        );
        assert!(diff(&after, &after).is_empty());
    }

    #[test]
    fn values_display_compactly() {
        assert_eq!(PropertyValue::Pixels(12.5).to_string(), "12.5px");
        assert_eq!(PropertyValue::Rems(0.5).to_string(), "0.5rem");
        assert_eq!(PropertyValue::Relative(0.5).to_string(), "50%");
        assert_eq!(PropertyValue::Auto.to_string(), "auto");
        assert_eq!(PropertyValue::Number(1.5).to_string(), "1.5");
        assert_eq!(PropertyValue::Bool(true).to_string(), "true");
        assert_eq!(PropertyValue::Text("Lilex".into()).to_string(), "\"Lilex\"");
        assert_eq!(
            PropertyValue::Other(json!({ "Span": 2 })).to_string(),
            "{\"Span\":2}"
        );
        assert_eq!(PropertyGroup::Spacing.label(), "Spacing");
    }

    #[test]
    fn pointers_escape_and_unescape() {
        assert_eq!(pointer_tokens("/a~1b/c~0d").unwrap(), vec!["a/b", "c~d"]);
        assert_eq!(label("/a~1b/c~0d"), "a/b.c~d");
        assert_eq!(label("/text/underline/color"), "text.underline.color");
        assert!(pointer_tokens("x").is_err());
    }
}
