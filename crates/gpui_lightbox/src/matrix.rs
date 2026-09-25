//! Variants: the same view shot across appearances, scale factors and window
//! sizes, composed into one labeled grid.

use crate::{
    compose::{Composer, backdrop, heading, mono, picture, pill, placed, resample, section},
    manifest::{MatrixRecord, Origin, Record},
    output::slug,
    shot::{Rect, Shot, trim},
    stage::{Appearance, Stage},
    theme::DARK,
};
use anyhow::Result;
use gpui::{AnyElement, IntoElement, ParentElement as _, RenderImage, Styled as _, div, px};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{panic::Location, path::PathBuf, rc::Rc, sync::Arc};

/// One combination of appearance, scale and window size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Variant {
    /// Light or dark.
    pub appearance: Appearance,
    /// Device pixels per logical pixel.
    pub scale: f32,
    /// Window size in logical pixels.
    pub size: (f32, f32),
}

impl Variant {
    /// A label such as `Dark · 2× · 640×200`.
    pub fn label(&self) -> String {
        format!(
            "{} · {}× · {}×{}",
            self.appearance.label(),
            trim(self.scale),
            trim(self.size.0),
            trim(self.size.1)
        )
    }
}

/// Which variants to shoot: the cross product of appearances × scales ×
/// sizes. An empty list keeps the stage's current setting.
///
/// ```ignore
/// Matrix::new()
///     .appearances([Appearance::Light, Appearance::Dark])
///     .scales([1., 2.])
///     .sizes([(320., 200.), (480., 200.)])
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Matrix {
    appearances: Vec<Appearance>,
    scales: Vec<f32>,
    sizes: Vec<(f32, f32)>,
}

impl Matrix {
    /// A matrix with a single variant: the stage's current settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Light and dark at 1× and 2×, at the stage's size.
    pub fn standard() -> Self {
        Self::new()
            .appearances([Appearance::Light, Appearance::Dark])
            .scales([1., 2.])
    }

    /// The appearances to shoot.
    pub fn appearances(mut self, appearances: impl IntoIterator<Item = Appearance>) -> Self {
        self.appearances = appearances.into_iter().collect();
        self
    }

    /// The scale factors to shoot.
    pub fn scales(mut self, scales: impl IntoIterator<Item = f32>) -> Self {
        self.scales = scales.into_iter().collect();
        self
    }

    /// The window sizes to shoot, in logical pixels.
    pub fn sizes(mut self, sizes: impl IntoIterator<Item = (f32, f32)>) -> Self {
        self.sizes = sizes.into_iter().collect();
        self
    }

    /// Every variant, appearance-major, given the stage's current settings.
    pub fn variants(&self, current: Variant) -> Vec<Variant> {
        fn or<T: Copy>(values: &[T], current: T) -> Vec<T> {
            if values.is_empty() {
                vec![current]
            } else {
                values.to_vec()
            }
        }
        let mut variants = Vec::new();
        for appearance in or(&self.appearances, current.appearance) {
            for size in or(&self.sizes, current.size) {
                for scale in or(&self.scales, current.scale) {
                    variants.push(Variant {
                        appearance,
                        scale,
                        size,
                    });
                }
            }
        }
        variants
    }
}

/// A variant as recorded for the report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatrixCell {
    /// The variant's label.
    pub label: String,
    /// The variant's shot name.
    pub shot: String,
    /// The variant's PNG, relative to the suite.
    pub image: String,
}

/// The shots a [`Stage::matrix`] took, and its grid image.
pub struct MatrixShots {
    /// Each variant with its shot, in grid order.
    pub shots: Vec<(Variant, Shot)>,
    /// The grid PNG.
    pub grid: PathBuf,
}

impl Stage {
    /// Shoots the mounted view in every variant of `matrix` and composes
    /// the shots into one labeled grid, `target/lightbox/<suite>/<name>.matrix.png`.
    ///
    /// `prepare` runs after the stage switches to each variant and before
    /// the shot. Mounting the view there renders each variant from its first
    /// frame; a view mounted before is switched in place, and its style
    /// transitions animate the switch like the app would. The stage returns
    /// to its previous settings afterwards.
    ///
    /// ```ignore
    /// stage.matrix("button", Matrix::standard(), |stage, _| {
    ///     stage.mount(|_, cx| cx.new(|_| Button::new("Save")));
    ///     stage.hover_text("Save");
    /// });
    /// ```
    #[track_caller]
    pub fn matrix(
        &mut self,
        name: &str,
        matrix: Matrix,
        mut prepare: impl FnMut(&mut Stage, &Variant),
    ) -> MatrixShots {
        let location = Location::caller();
        let config = self.config().clone();
        let current = Variant {
            appearance: config.appearance,
            scale: config.scale,
            size: (config.size.width.as_f32(), config.size.height.as_f32()),
        };
        let mut shots = Vec::new();
        for variant in matrix.variants(current) {
            self.resize(variant.size.0, variant.size.1)
                .set_scale(variant.scale)
                .set_appearance(variant.appearance);
            prepare(self, &variant);
            let shot = self.capture(&format!("{name} · {}", variant.label()));
            shot.save(location);
            shots.push((variant, shot));
        }
        self.resize(current.size.0, current.size.1)
            .set_scale(current.scale)
            .set_appearance(current.appearance);

        let file = format!("{}.matrix.png", slug(name));
        let grid = self
            .compositor
            .with(|composer| render(composer, name, &shots))
            .and_then(|image| self.suite().write_png(&file, &image))
            .unwrap_or_else(|error| panic!("lightbox: composing matrix {name:?}: {error:#}"));
        let record = Record::Matrix(MatrixRecord {
            name: name.into(),
            image: file,
            cells: shots
                .iter()
                .map(|(variant, shot)| MatrixCell {
                    label: variant.label(),
                    shot: shot.name.clone(),
                    image: shot.file_name(),
                })
                .collect(),
            origin: Origin::at(location),
        });
        if let Err(error) = self.suite().write_record(&record) {
            panic!("lightbox: saving matrix {name:?}: {error:#}");
        }
        MatrixShots { shots, grid }
    }
}

const MARGIN: f32 = 24.;
const GAP: f32 = 24.;
const LABEL: f32 = 26.;
const MAX_WIDTH: f32 = 2400.;

struct Cell {
    label: String,
    image: Arc<RenderImage>,
    bounds: Rect,
}

/// Lays variants out appearance by row, and renders the grid.
fn render(composer: &mut Composer, name: &str, shots: &[(Variant, Shot)]) -> Result<RgbaImage> {
    let per_row = shots
        .iter()
        .filter(|(variant, _)| variant.appearance == shots[0].0.appearance)
        .count()
        .max(1);
    let rows = shots.chunks(per_row).collect::<Vec<_>>();
    let natural_width = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|(variant, _)| variant.size.0 + GAP)
                .sum::<f32>()
                - GAP
        })
        .fold(0f32, f32::max);
    // Keep the grid a readable size; shrink every cell alike when needed.
    let zoom = (MAX_WIDTH / natural_width).min(1.);
    let scale = 2.;

    let mut cells = Vec::new();
    let mut y = MARGIN + 64.;
    for row in &rows {
        let mut x = MARGIN;
        let row_height = row
            .iter()
            .map(|(variant, _)| variant.size.1 * zoom)
            .fold(0f32, f32::max);
        for (variant, shot) in row.iter() {
            let (w, h) = (
                (variant.size.0 * zoom).round(),
                (variant.size.1 * zoom).round(),
            );
            let image = composer.image(&resample(
                &shot.image,
                (w * scale) as u32,
                (h * scale) as u32,
            ));
            cells.push(Cell {
                label: variant.label(),
                image,
                bounds: Rect::new(x, y + LABEL, w, h),
            });
            x += w + GAP;
        }
        y += LABEL + row_height + GAP;
    }
    let width = (natural_width * zoom).ceil() + MARGIN * 2.;
    let height = y - GAP + MARGIN;
    let title = name.to_string();
    let subtitle = format!(
        "{} variant{}",
        shots.len(),
        if shots.len() == 1 { "" } else { "s" }
    );
    let cells = Rc::new(cells);
    composer.render(width, height, scale, move |_, _| {
        let mut root = backdrop().child(
            placed(Rect::new(MARGIN, MARGIN, width - MARGIN * 2., 44.))
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(section("Matrix"))
                        .child(heading(title.clone(), 18., DARK.text)),
                )
                .child(pill(subtitle.clone(), DARK.accent)),
        );
        root = root.child(placed(Rect::new(0., MARGIN + 52., width, 1.)).bg(DARK.line));
        for cell in cells.iter() {
            root = root.children(cell_elements(cell));
        }
        root.into_any_element()
    })
}

fn cell_elements(cell: &Cell) -> Vec<AnyElement> {
    let bounds = cell.bounds;
    vec![
        placed(Rect::new(bounds.x, bounds.y - LABEL + 4., bounds.w, 18.))
            .child(mono(cell.label.clone(), 11., DARK.text_muted))
            .into_any_element(),
        placed(bounds.inflate(1.))
            .border_1()
            .border_color(DARK.line_strong)
            .into_any_element(),
        picture(cell.image.clone(), bounds),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_cross_every_dimension() {
        let current = Variant {
            appearance: Appearance::Light,
            scale: 2.,
            size: (800., 600.),
        };
        let variants = Matrix::standard()
            .sizes([(320., 200.), (640., 200.)])
            .variants(current);
        assert_eq!(variants.len(), 8);
        assert_eq!(variants[0].label(), "Light · 1× · 320×200");
        assert_eq!(variants[7].label(), "Dark · 2× · 640×200");
        assert_eq!(Matrix::new().variants(current), vec![current]);
    }
}
