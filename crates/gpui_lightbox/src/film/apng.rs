//! Animated PNG output. After the first (full) frame, each frame stores only
//! the region that changed, so films stay small at full resolution.

use anyhow::{Context as _, Result};
use image::RgbaImage;
use png::{BlendOp, DisposeOp};
use std::{fs, io::BufWriter, path::Path, time::Duration};

/// How long the last frame holds before the animation loops, in ms.
const HOLD_MS: u16 = 1200;

/// One frame: its pixels and the device-pixel region `(x, y, w, h)` that
/// changed since the previous frame (`None` when nothing did).
pub(super) struct Frame<'a> {
    pub(super) image: &'a RgbaImage,
    pub(super) changed: Option<(u32, u32, u32, u32)>,
}

/// Writes `frames`, shown at `times`, as a looping APNG at `path`.
pub(super) fn write(path: &Path, frames: &[Frame<'_>], times: &[Duration]) -> Result<()> {
    let Some(first) = frames.first() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let (width, height) = first.image.dimensions();
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_animated(frames.len() as u32, 0)?;
    let mut writer = encoder.write_header()?;

    for (ix, frame) in frames.iter().enumerate() {
        let delay_ms = match times.get(ix + 1) {
            Some(next) => (next.saturating_sub(times[ix]).as_millis() as u16).max(1),
            None => HOLD_MS,
        };
        writer.set_frame_delay(delay_ms, 1000)?;
        writer.set_blend_op(BlendOp::Source)?;
        writer.set_dispose_op(DisposeOp::None)?;

        // The default image must cover the canvas; later frames only their change.
        let region = match (ix, frame.changed) {
            (0, _) => (0, 0, width, height),
            (_, Some(changed)) => changed,
            (_, None) => (0, 0, 1, 1),
        };
        writer.reset_frame_position()?;
        writer.set_frame_dimension(region.2, region.3)?;
        writer.set_frame_position(region.0, region.1)?;
        let pixels = if region == (0, 0, width, height) {
            frame.image.as_raw().clone()
        } else {
            image::imageops::crop_imm(frame.image, region.0, region.1, region.2, region.3)
                .to_image()
                .into_raw()
        };
        writer.write_image_data(&pixels)?;
    }
    writer.finish()?;
    Ok(())
}
