//! The inspector's overlays: painted over the app after every root, from the
//! frame's own element tree and the capture's [`OverlayState`], clipped to the
//! app's area. They draw raw quads (only label chips shape text) and never
//! invalidate a view.

use super::*;
use crate::inspector::{
    CauseKind, ElementFlags, ElementKey, ElementRecord, ElementTree, InspectorCapture,
    OverlayHighlight, OverlayModes,
    recorder::{FLASH_DURATION, RecordMode},
};
use crate::{PathBuilder, TextAlign, hsla, rgba};

/// Web inspector conventions, translucent so the app shows through.
const MARGIN_COLOR: u32 = 0xf6b26ba8;
const BORDER_COLOR: u32 = 0xffe599a8;
const PADDING_COLOR: u32 = 0x93c47d8c;
const CONTENT_COLOR: u32 = 0x6fa8dca8;
const SELECTED_OUTLINE: u32 = 0x3b82f6ff;
const CHIP_BACKGROUND: u32 = 0x1d1f23f0;
const SLOW_FRAME_COLOR: u32 = 0xef4444ff;
const OVERFLOW_COLOR: u32 = 0xef4444b0;

const CHIP_FONT_SIZE: Pixels = px(11.);
const CHIP_LINE_HEIGHT: Pixels = px(16.);
const CHIP_PADDING: Point<Pixels> = point(px(6.), px(2.));
const CHIP_GAP: Pixels = px(2.);
const SLOW_FRAME_BORDER: Pixels = px(3.);
const STRIPE_SPACING: f32 = 6.;

/// Flags that make a hitbox interactive rather than merely present.
const LISTENING: ElementFlags = ElementFlags::CLICKABLE
    .union(ElementFlags::DRAG_DROP)
    .union(ElementFlags::TOOLTIP)
    .union(ElementFlags::SCROLLABLE)
    .union(ElementFlags::KEYBOARD);

/// What one overlay pass paints, copied out of the capture so the window can
/// paint while reading it.
struct OverlayPass {
    modes: OverlayModes,
    tree: Option<Arc<ElementTree>>,
    targets: SmallVec<[BoxTarget; 2]>,
    highlights: Vec<OverlayHighlight>,
    flashes: Vec<(Bounds<Pixels>, f32)>,
    hitboxes: Vec<(Hitbox, bool)>,
    slow_frame: bool,
}

/// A hovered or selected element, drawn with its box model and a label.
struct BoxTarget {
    record: ElementRecord,
    label: SharedString,
    selected: bool,
}

impl Window {
    /// Paints the overlays of the capture's [`OverlayState`] above everything
    /// else, clipped to [`Self::app_bounds`].
    pub(super) fn paint_inspector_overlays(&mut self, cx: &mut App) {
        let Some(capture) = self.inspector_capture.as_deref() else {
            return;
        };
        if capture.recorder.mode == RecordMode::Off {
            return;
        }
        let pass = OverlayPass::new(capture, &self.next_frame.hitboxes);
        let app_bounds = self.app_bounds();
        let phase = self.inspector_suspend(cx);
        self.with_content_mask(Some(ContentMask { bounds: app_bounds }), |window| {
            window.paint_overlay_pass(&pass, app_bounds, cx);
        });
        self.inspector_resume(phase, cx);
        if !pass.flashes.is_empty() {
            // Keep the flashes fading; these frames are the inspector's own.
            self.request_inspector_frame(CauseKind::Animation);
        }
    }

    fn paint_overlay_pass(&mut self, pass: &OverlayPass, app_bounds: Bounds<Pixels>, cx: &mut App) {
        if let Some(tree) = pass.tree.as_deref() {
            if pass.modes.contains(OverlayModes::OUTLINES) {
                self.paint_outlines(tree);
            }
            if pass.modes.contains(OverlayModes::OVERFLOW) {
                self.paint_overflow(tree);
            }
        }
        for (hitbox, listening) in &pass.hitboxes {
            self.paint_hitbox(hitbox, *listening);
        }
        for &(bounds, alpha) in &pass.flashes {
            self.paint_quad(fill(bounds, hsla(0.83, 0.9, 0.6, alpha)));
        }
        for highlight in &pass.highlights {
            let border = crate::Hsla {
                alpha: 1.,
                ..highlight.color
            };
            self.paint_quad(quad(
                highlight.bounds,
                px(0.),
                highlight.color,
                px(1.),
                border,
                BorderStyle::Solid,
            ));
            if let Some(label) = &highlight.label {
                self.paint_label_chip(label.clone(), highlight.bounds, app_bounds, cx);
            }
        }
        for target in &pass.targets {
            if pass.modes.contains(OverlayModes::BOX_MODEL) {
                self.paint_box_model(&target.record);
            }
            if target.selected {
                self.paint_quad(outline(
                    target.record.bounds,
                    rgba(SELECTED_OUTLINE),
                    BorderStyle::Solid,
                ));
            }
            self.paint_label_chip(target.label.clone(), target.record.bounds, app_bounds, cx);
        }
        if pass.slow_frame {
            self.paint_quad(quad(
                app_bounds,
                px(0.),
                transparent_black(),
                SLOW_FRAME_BORDER,
                rgba(SLOW_FRAME_COLOR),
                BorderStyle::Solid,
            ));
        }
    }

    /// Outlines every visible element, hue by depth.
    fn paint_outlines(&mut self, tree: &ElementTree) {
        for record in &tree.elements {
            if record.visible_bounds.is_some() {
                let hue = (record.depth as f32 * 0.13).fract();
                self.paint_quad(outline(
                    record.bounds,
                    hsla(hue, 0.75, 0.55, 0.8),
                    BorderStyle::Solid,
                ));
            }
        }
    }

    /// Stripes elements that overflow their parent.
    fn paint_overflow(&mut self, tree: &ElementTree) {
        for record in &tree.elements {
            if !record.flags.contains(ElementFlags::OVERFLOWS_PARENT) {
                continue;
            }
            let Some(visible) = record.visible_bounds else {
                continue;
            };
            self.with_content_mask(Some(ContentMask { bounds: visible }), |window| {
                if let Some(stripes) = stripes(visible) {
                    window.paint_path(stripes, rgba(OVERFLOW_COLOR));
                }
                window.paint_quad(outline(visible, rgba(OVERFLOW_COLOR), BorderStyle::Solid));
            });
        }
    }

    /// Shades a hitbox, darker when it listens for input or blocks the ones
    /// behind it.
    fn paint_hitbox(&mut self, hitbox: &Hitbox, listening: bool) {
        let emphasized = listening || hitbox.behavior != HitboxBehavior::Normal;
        let alpha = if emphasized { 0.28 } else { 0.1 };
        self.with_content_mask(Some(hitbox.content_mask), |window| {
            window.paint_quad(quad(
                hitbox.bounds,
                px(0.),
                hsla(0.55, 0.8, 0.5, alpha),
                px(1.),
                hsla(0.55, 0.8, 0.45, alpha * 2.),
                BorderStyle::Solid,
            ));
        });
    }

    /// Paints margin, border, padding and content boxes. Without element
    /// details (below [`crate::inspector::CaptureLevel::Full`]) only the
    /// border box is known.
    fn paint_box_model(&mut self, record: &ElementRecord) {
        let bounds = record.bounds;
        let Some(box_model) = record
            .details
            .as_ref()
            .and_then(|details| details.box_model)
        else {
            self.paint_quad(fill(bounds, rgba(CONTENT_COLOR)));
            return;
        };
        let margin = box_model.margin.map(|edge| (*edge).max(px(0.)));
        let border = box_model.border.map(|edge| (*edge).max(px(0.)));
        let padding = box_model.padding.map(|edge| (*edge).max(px(0.)));
        let padding_box = inset(bounds, &border);
        let content_box = inset(padding_box, &padding);
        let rings = [
            (bounds.extend(margin), margin, MARGIN_COLOR),
            (bounds, border, BORDER_COLOR),
            (padding_box, padding, PADDING_COLOR),
        ];
        for (ring_bounds, widths, color) in rings {
            if widths.any(|width| *width > px(0.)) {
                self.paint_quad(quad(
                    ring_bounds,
                    px(0.),
                    transparent_black(),
                    widths,
                    rgba(color),
                    BorderStyle::Solid,
                ));
            }
        }
        self.paint_quad(fill(content_box, rgba(CONTENT_COLOR)));
    }

    /// Paints `text` in a chip next to `anchor`, kept inside `area`.
    fn paint_label_chip(
        &mut self,
        text: SharedString,
        anchor: Bounds<Pixels>,
        area: Bounds<Pixels>,
        cx: &mut App,
    ) {
        let mut text_style = self.text_style();
        text_style.color = hsla(0., 0., 0.95, 1.);
        let run = text_style.to_run(text.len());
        let line = self
            .text_system()
            .shape_line(text, CHIP_FONT_SIZE, &[run], None);
        let chip_size = size(
            line.width + CHIP_PADDING.x * 2.,
            CHIP_LINE_HEIGHT + CHIP_PADDING.y * 2.,
        );
        let origin = chip_origin(anchor, chip_size, area);
        self.paint_quad(
            fill(Bounds::new(origin, chip_size), rgba(CHIP_BACKGROUND)).corner_radii(px(3.)),
        );
        line.paint(
            origin + CHIP_PADDING,
            CHIP_LINE_HEIGHT,
            TextAlign::Left,
            None,
            self,
            cx,
        )
        .log_err();
    }
}

impl OverlayPass {
    fn new(capture: &InspectorCapture, hitboxes: &[Hitbox]) -> Self {
        let overlay = capture.overlay();
        let modes = overlay.modes;
        let recorder = &capture.recorder;
        let tree = recorder.live_tree.clone();
        let mut targets = SmallVec::new();
        if let Some(tree) = tree.as_deref() {
            let mut add_target = |key: Option<ElementKey>, selected: bool| {
                let Some(record) = key
                    .and_then(|key| tree.find(key))
                    .and_then(|ix| tree.get(ix))
                else {
                    return;
                };
                targets.push(BoxTarget {
                    label: element_label(record, capture),
                    record: record.clone(),
                    selected,
                });
            };
            add_target(overlay.selected, true);
            if overlay.hovered != overlay.selected {
                add_target(overlay.hovered, false);
            }
        }
        let now = capture.clock.instant();
        let flashes = recorder
            .flashes
            .iter()
            .map(|flash| {
                let progress = now.saturating_duration_since(flash.started).as_secs_f32()
                    / FLASH_DURATION.as_secs_f32();
                (flash.bounds, 0.45 * (1. - progress).clamp(0., 1.))
            })
            .collect();
        OverlayPass {
            modes,
            highlights: overlay.highlights.clone(),
            hitboxes: if modes.contains(OverlayModes::HITBOXES) {
                app_hitboxes(capture, hitboxes)
            } else {
                Vec::new()
            },
            slow_frame: modes.contains(OverlayModes::SLOW_FRAMES)
                && capture
                    .latest_app_frame()
                    .is_some_and(|frame| frame.timings.app_total() > capture.config().budget),
            tree,
            targets,
            flashes,
        }
    }
}

/// The app's hitboxes (not the dock's), each with whether its owner listens
/// for input.
fn app_hitboxes(capture: &InspectorCapture, hitboxes: &[Hitbox]) -> Vec<(Hitbox, bool)> {
    let recorder = &capture.recorder;
    let tree = recorder.live_tree.as_deref();
    let owners: FxHashMap<HitboxId, ElementFlags> = recorder
        .hitbox_owners
        .iter()
        .filter_map(|(id, owner)| Some((*id, tree?.get(*owner)?.flags)))
        .collect();
    hitboxes
        .iter()
        .enumerate()
        .filter(|(ix, _)| !recorder.inspector_hitboxes.contains(ix))
        .map(|(_, hitbox)| {
            let listening = owners
                .get(&hitbox.id)
                .is_some_and(|flags| flags.intersects(LISTENING));
            (hitbox.clone(), listening)
        })
        .collect()
}

/// `div#close 24×24 · issue_detail.rs:97`
fn element_label(record: &ElementRecord, capture: &InspectorCapture) -> SharedString {
    let mut label = record.kind.display_name().into_owned();
    if let Some(id) = &record.id {
        label.push('#');
        label.push_str(id);
    }
    let size = record.bounds.size;
    label.push_str(&format!(
        " {}×{}",
        format_pixels(size.width),
        format_pixels(size.height)
    ));
    if let Some(info) = record.key.and_then(|key| capture.path_info(key.path)) {
        let file = std::path::Path::new(info.source.file())
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(info.source.file());
        label.push_str(&format!(" · {file}:{}", info.source.line()));
    }
    label.into()
}

fn format_pixels(pixels: Pixels) -> String {
    let value = f32::from(pixels);
    if value.fract().abs() < 0.05 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// Where a label chip for `anchor` goes: above it, else below it, else
/// inside its top edge, always within `area`.
fn chip_origin(anchor: Bounds<Pixels>, chip: Size<Pixels>, area: Bounds<Pixels>) -> Point<Pixels> {
    let max_x = (area.right() - chip.width).max(area.left());
    let max_y = (area.bottom() - chip.height).max(area.top());
    let x = anchor.left().clamp(area.left(), max_x);
    let above = anchor.top() - CHIP_GAP - chip.height;
    let below = anchor.bottom() + CHIP_GAP;
    let y = if above >= area.top() {
        above
    } else if below <= max_y {
        below
    } else {
        anchor.top().clamp(area.top(), max_y)
    };
    point(x, y)
}

fn inset(bounds: Bounds<Pixels>, edges: &Edges<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(bounds.left() + edges.left, bounds.top() + edges.top),
        size(
            (bounds.size.width - edges.left - edges.right).max(px(0.)),
            (bounds.size.height - edges.top - edges.bottom).max(px(0.)),
        ),
    )
}

/// Diagonal hatching across `bounds`.
fn stripes(bounds: Bounds<Pixels>) -> Option<Path<Pixels>> {
    let mut builder = PathBuilder::stroke(px(1.5));
    let height = f32::from(bounds.size.height);
    let width = f32::from(bounds.size.width);
    let mut offset = -height;
    while offset < width {
        builder.move_to(bounds.origin + point(px(offset), px(height)));
        builder.line_to(bounds.origin + point(px(offset + height), px(0.)));
        offset += STRIPE_SPACING;
    }
    builder.build().log_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_stay_inside_the_area() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(100.)));
        let chip = size(px(80.), px(20.));

        // Room above: the chip sits on top of the element.
        let anchor = Bounds::new(point(px(10.), px(50.)), size(px(20.), px(20.)));
        assert_eq!(chip_origin(anchor, chip, area), point(px(10.), px(28.)));

        // No room above: below.
        let anchor = Bounds::new(point(px(10.), px(0.)), size(px(20.), px(20.)));
        assert_eq!(chip_origin(anchor, chip, area), point(px(10.), px(22.)));

        // Near the right edge: shifted left to stay visible.
        let anchor = Bounds::new(point(px(190.), px(50.)), size(px(10.), px(10.)));
        assert_eq!(chip_origin(anchor, chip, area).x, px(120.));

        // Filling the area: inside its top edge.
        let chip_origin = chip_origin(area, chip, area);
        assert_eq!(chip_origin, point(px(0.), px(0.)));
    }

    #[test]
    fn pixel_labels_are_compact() {
        assert_eq!(format_pixels(px(24.)), "24");
        assert_eq!(format_pixels(px(24.5)), "24.5");
    }
}
