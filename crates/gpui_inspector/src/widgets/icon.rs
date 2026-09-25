//! Vector icons: a tiny embedded set of 16×16 stroke glyphs.
//!
//! Icons are inline SVG rasterized by gpui into its sprite atlas at the
//! window's device pixel size, so they stay crisp at 1× and 2× without any
//! asset files. Each glyph is drawn on a 16 unit grid with a 1.4 unit stroke,
//! round caps and joins; the fill color comes from the element.

use crate::theme::Theme;
use gpui::{
    App, Hsla, IntoElement, Pixels, RenderOnce, SharedString, Styled, TransformationMatrix, Window,
    canvas,
};

macro_rules! glyph {
    ($body:literal) => {
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">"##,
            $body,
            "</svg>"
        )
    };
}

/// The glyphs Loupe draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconName {
    /// Crosshair: pick an element.
    Pick,
    /// Nested rectangles: outline every element.
    Outline,
    /// Lightning bolt: paint flashing.
    Flash,
    /// Dashed box with a pointer: hitboxes.
    Hitbox,
    /// Triangle with an exclamation mark.
    Warning,
    /// Arrow escaping a box: overflowing content.
    Overflow,
    /// Box with shaded content: the box model.
    BoxModel,
    /// Two bars.
    Pause,
    /// Right-pointing triangle.
    Play,
    /// Window with a right column.
    DockRight,
    /// Window with a bottom row.
    DockBottom,
    /// Diagonal cross.
    Close,
    /// Disclosure, collapsed.
    ChevronRight,
    /// Disclosure, expanded.
    ChevronDown,
    /// Magnifier.
    Search,
    /// Solid diamond: a stateful view.
    View,
    /// Four diamonds: a `RenderOnce` component.
    Component,
    /// Small dot: any other element.
    ElementDot,
    /// Angle brackets: a source location.
    Source,
    /// Chain link.
    Link,
    /// Two stacked sheets.
    Copy,
    /// Funnel.
    Filter,
    /// Arrow up (ascending sort).
    ArrowUp,
    /// Arrow down (descending sort).
    ArrowDown,
    /// Check mark.
    Check,
    /// Isometric cube: an entity.
    Entity,
    /// Prompt chevron: a command.
    Command,
    /// Small solid triangle pointing up: over budget.
    TriangleUp,
    /// Mouse pointer: clickable.
    Cursor,
    /// Viewfinder corners: focusable.
    Focus,
    /// Vertical double arrow: scrolls.
    Scroll,
    /// Keyboard: key or action listeners.
    Keyboard,
    /// Counter-clockwise arrow: revert.
    Revert,
    /// Plus: add.
    Plus,
    /// Clock face: an earlier point in time.
    Clock,
}

impl IconName {
    /// Every glyph, for galleries and pickers.
    pub const ALL: [IconName; 35] = [
        IconName::Pick,
        IconName::Outline,
        IconName::Flash,
        IconName::Hitbox,
        IconName::Warning,
        IconName::Overflow,
        IconName::BoxModel,
        IconName::Pause,
        IconName::Play,
        IconName::DockRight,
        IconName::DockBottom,
        IconName::Close,
        IconName::ChevronRight,
        IconName::ChevronDown,
        IconName::Search,
        IconName::View,
        IconName::Component,
        IconName::ElementDot,
        IconName::Source,
        IconName::Link,
        IconName::Copy,
        IconName::Filter,
        IconName::ArrowUp,
        IconName::ArrowDown,
        IconName::Check,
        IconName::Entity,
        IconName::Command,
        IconName::TriangleUp,
        IconName::Cursor,
        IconName::Focus,
        IconName::Scroll,
        IconName::Keyboard,
        IconName::Revert,
        IconName::Plus,
        IconName::Clock,
    ];

    /// A stable name, also the sprite atlas key.
    pub fn key(self) -> &'static str {
        match self {
            IconName::Pick => "loupe/pick",
            IconName::Outline => "loupe/outline",
            IconName::Flash => "loupe/flash",
            IconName::Hitbox => "loupe/hitbox",
            IconName::Warning => "loupe/warning",
            IconName::Overflow => "loupe/overflow",
            IconName::BoxModel => "loupe/box-model",
            IconName::Pause => "loupe/pause",
            IconName::Play => "loupe/play",
            IconName::DockRight => "loupe/dock-right",
            IconName::DockBottom => "loupe/dock-bottom",
            IconName::Close => "loupe/close",
            IconName::ChevronRight => "loupe/chevron-right",
            IconName::ChevronDown => "loupe/chevron-down",
            IconName::Search => "loupe/search",
            IconName::View => "loupe/view",
            IconName::Component => "loupe/component",
            IconName::ElementDot => "loupe/element-dot",
            IconName::Source => "loupe/source",
            IconName::Link => "loupe/link",
            IconName::Copy => "loupe/copy",
            IconName::Filter => "loupe/filter",
            IconName::ArrowUp => "loupe/arrow-up",
            IconName::ArrowDown => "loupe/arrow-down",
            IconName::Check => "loupe/check",
            IconName::Entity => "loupe/entity",
            IconName::Command => "loupe/command",
            IconName::TriangleUp => "loupe/triangle-up",
            IconName::Cursor => "loupe/cursor",
            IconName::Focus => "loupe/focus",
            IconName::Scroll => "loupe/scroll",
            IconName::Keyboard => "loupe/keyboard",
            IconName::Revert => "loupe/revert",
            IconName::Plus => "loupe/plus",
            IconName::Clock => "loupe/clock",
        }
    }

    /// The glyph as a complete SVG document.
    pub fn svg(self) -> &'static str {
        match self {
            IconName::Pick => glyph!(
                r##"<circle cx="8" cy="8" r="4.6"/><path d="M8 1.2v3M8 11.8v3M1.2 8h3M11.8 8h3"/><circle cx="8" cy="8" r="1.1" fill="#000" stroke="none"/>"##
            ),
            IconName::Outline => glyph!(
                r##"<rect x="1.7" y="1.7" width="12.6" height="12.6" rx="1.2"/><rect x="4.7" y="4.7" width="6.6" height="6.6" rx=".6"/>"##
            ),
            IconName::Flash => glyph!(r##"<path d="M9.2 1.4 3.4 9.1h4.2l-.9 5.5 5.9-7.8H8.4z"/>"##),
            IconName::Hitbox => glyph!(
                r##"<path d="M10.8 5.2V3a1 1 0 0 0-1-1H3a1 1 0 0 0-1 1v6.8a1 1 0 0 0 1 1h2.2" stroke-dasharray="1.6 1.9"/><path d="M7.4 7.4 14 9.8l-2.9 1.3-1.3 2.9z" fill="#000"/>"##
            ),
            IconName::Warning => glyph!(
                r##"<path d="M7.1 2.5a1 1 0 0 1 1.8 0l5.4 9.9a1 1 0 0 1-.9 1.5H2.6a1 1 0 0 1-.9-1.5z"/><path d="M8 6.3v3"/><circle cx="8" cy="11.4" r=".8" fill="#000" stroke="none"/>"##
            ),
            IconName::Overflow => glyph!(
                r##"<path d="M8.6 2.3H3.3a1 1 0 0 0-1 1v9.4a1 1 0 0 0 1 1h9.4a1 1 0 0 0 1-1V7.4"/><path d="M8.2 7.8 14 2M10.4 2H14v3.6"/>"##
            ),
            IconName::BoxModel => glyph!(
                r##"<rect x="1.7" y="1.7" width="12.6" height="12.6" rx="1.2" stroke-dasharray="2 1.8"/><rect x="4.6" y="4.6" width="6.8" height="6.8" rx=".6" fill="#000" fill-opacity=".38"/>"##
            ),
            IconName::Pause => glyph!(
                r##"<rect x="3.9" y="2.8" width="2.8" height="10.4" rx=".8" fill="#000" stroke="none"/><rect x="9.3" y="2.8" width="2.8" height="10.4" rx=".8" fill="#000" stroke="none"/>"##
            ),
            IconName::Play => glyph!(r##"<path d="M4.8 2.9v10.2L13 8z" fill="#000"/>"##),
            IconName::DockRight => glyph!(
                r##"<rect x="1.7" y="2.7" width="12.6" height="10.6" rx="1.4"/><path d="M9.6 3.2h3.5a.7.7 0 0 1 .7.7v8.2a.7.7 0 0 1-.7.7H9.6z" fill="#000" fill-opacity=".38" stroke="none"/><path d="M9.6 2.7v10.6"/>"##
            ),
            IconName::DockBottom => glyph!(
                r##"<rect x="1.7" y="2.7" width="12.6" height="10.6" rx="1.4"/><path d="M2.2 9.3h11.6v2.8a.7.7 0 0 1-.7.7H2.9a.7.7 0 0 1-.7-.7z" fill="#000" fill-opacity=".38" stroke="none"/><path d="M1.7 9.3h12.6"/>"##
            ),
            IconName::Close => glyph!(r##"<path d="M4 4l8 8M12 4l-8 8"/>"##),
            IconName::ChevronRight => glyph!(r##"<path d="M6.2 3.6 10.6 8l-4.4 4.4"/>"##),
            IconName::ChevronDown => glyph!(r##"<path d="M3.6 6.2 8 10.6l4.4-4.4"/>"##),
            IconName::Search => {
                glyph!(r##"<circle cx="7" cy="7" r="4.4"/><path d="m10.4 10.4 3.4 3.4"/>"##)
            }
            IconName::View => glyph!(r##"<path d="M8 2.4 13.6 8 8 13.6 2.4 8z" fill="#000"/>"##),
            IconName::Component => glyph!(
                r##"<path d="M8 1.6 10.2 3.8 8 6 5.8 3.8zM3.8 5.8 6 8 3.8 10.2 1.6 8zM12.2 5.8 14.4 8l-2.2 2.2L10 8zM8 10l2.2 2.2L8 14.4l-2.2-2.2z"/>"##
            ),
            IconName::ElementDot => {
                glyph!(r##"<circle cx="8" cy="8" r="2.6" fill="#000" stroke="none"/>"##)
            }
            IconName::Source => {
                glyph!(r##"<path d="M5.4 4.4 1.8 8l3.6 3.6M10.6 4.4 14.2 8l-3.6 3.6"/>"##)
            }
            IconName::Link => glyph!(
                r##"<path d="M6.6 9.4l2.8-2.8"/><path d="M7.3 4.5l1.3-1.3a2.6 2.6 0 0 1 3.7 3.7L11 8.2M8.7 11.5l-1.3 1.3a2.6 2.6 0 0 1-3.7-3.7L5 7.8"/>"##
            ),
            IconName::Copy => glyph!(
                r##"<rect x="5.4" y="5.4" width="8.6" height="8.6" rx="1.3"/><path d="M10.6 5.4V3.3a1.3 1.3 0 0 0-1.3-1.3H3.3A1.3 1.3 0 0 0 2 3.3v6a1.3 1.3 0 0 0 1.3 1.3h2.1"/>"##
            ),
            IconName::Filter => glyph!(r##"<path d="M2.2 3h11.6L9.4 8.4v4.4l-2.8 1.4V8.4z"/>"##),
            IconName::ArrowUp => glyph!(r##"<path d="M8 13V3M4.2 6.8 8 3l3.8 3.8"/>"##),
            IconName::ArrowDown => glyph!(r##"<path d="M8 3v10M4.2 9.2 8 13l3.8-3.8"/>"##),
            IconName::Check => glyph!(r##"<path d="m3 8.4 3.2 3.2L13 4.6"/>"##),
            IconName::Entity => glyph!(
                r##"<path d="M8 1.6 13.8 4.8v6.4L8 14.4l-5.8-3.2V4.8z"/><path d="M2.2 4.8 8 8l5.8-3.2M8 8v6.4"/>"##
            ),
            IconName::Command => glyph!(r##"<path d="M3.4 4.4 7 8l-3.6 3.6M8.6 11.8h4"/>"##),
            IconName::TriangleUp => glyph!(r##"<path d="M8 3.6 13.2 12H2.8z" fill="#000"/>"##),
            IconName::Cursor => glyph!(
                r##"<path d="M3.6 2.2v10.4l2.9-2.7 2 4.2 1.9-.9-2-4.1h4z" fill="#000" fill-opacity=".2"/>"##
            ),
            IconName::Focus => glyph!(
                r##"<path d="M2 5.4V3a1 1 0 0 1 1-1h2.4M10.6 2H13a1 1 0 0 1 1 1v2.4M14 10.6V13a1 1 0 0 1-1 1h-2.4M5.4 14H3a1 1 0 0 1-1-1v-2.4"/><circle cx="8" cy="8" r="1.7" fill="#000" stroke="none"/>"##
            ),
            IconName::Scroll => {
                glyph!(r##"<path d="M8 2.2v11.6M5 5 8 2.2 11 5M5 11 8 13.8 11 11"/>"##)
            }
            IconName::Keyboard => glyph!(
                r##"<rect x="1.6" y="3.8" width="12.8" height="8.4" rx="1.4"/><path d="M4.5 6.7h.01M7 6.7h.01M9.5 6.7h.01M11.8 6.7h.01M5.2 9.4h5.6"/>"##
            ),
            IconName::Revert => {
                glyph!(r##"<path d="M3.2 7.2a5 5 0 1 1 1.4 4.3"/><path d="M2.6 3.4v3.9h3.9"/>"##)
            }
            IconName::Plus => glyph!(r##"<path d="M8 3v10M3 8h10"/>"##),
            IconName::Clock => {
                glyph!(r##"<circle cx="8" cy="8" r="6.1"/><path d="M8 4.6V8l2.4 1.6"/>"##)
            }
        }
    }
}

/// An icon at a fixed square size.
///
/// ```ignore
/// Icon::new(IconName::Pick).size(px(14.)).color(theme.colors.accent)
/// ```
#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: Option<Pixels>,
    color: Option<Hsla>,
}

impl Icon {
    /// An icon in the theme's muted text color at the regular icon size.
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: None,
            color: None,
        }
    }

    /// Sets the square size.
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = Some(size);
        self
    }

    /// Sets the color.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(window, cx);
        let size = self.size.unwrap_or(theme.metrics.icon);
        let color = self.color.unwrap_or(theme.colors.text_muted);
        let name = self.name;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                window
                    .paint_svg(
                        bounds,
                        SharedString::new_static(name.key()),
                        Some(name.svg().as_bytes()),
                        TransformationMatrix::unit(),
                        color,
                        cx,
                    )
                    .ok();
            },
        )
        .size(size)
        .flex_none()
    }
}
