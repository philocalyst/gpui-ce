//! Contact sheets: many shots composed into one labeled grid, rendered with
//! gpui, so a reviewer (human or agent) looks at one image per suite.

use crate::{
    color::Color,
    compose::{Composer, backdrop, heading, label, mono, picture, pill, placed, resample, section},
    lint::Severity,
    manifest::Record,
    output::{RunInfo, Suite},
    shot::{Rect, trim},
    theme::DARK,
};
use anyhow::{Context as _, Result};
use gpui::{
    IntoElement, ParentElement as _, RenderImage, Styled as _, div, prelude::FluentBuilder as _, px,
};
use image::RgbaImage;
use std::{collections::HashSet, path::PathBuf, rc::Rc, sync::Arc};

/// How an item fared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Nothing was checked.
    #[default]
    Neutral,
    /// Checked and clean.
    Ok,
    /// Warnings.
    Warn,
    /// Failures.
    Crit,
}

impl Status {
    fn color(self) -> Color {
        match self {
            Status::Neutral => DARK.text_faint,
            Status::Ok => DARK.ok,
            Status::Warn => DARK.warn,
            Status::Crit => DARK.crit,
        }
    }
}

/// One cell of a contact sheet.
#[derive(Clone, Debug)]
pub struct SheetItem {
    /// The caption, e.g. the shot's name.
    pub label: String,
    /// A secondary line: size, scale, appearance, kind.
    pub detail: String,
    /// The image.
    pub image: RgbaImage,
    /// The status dot's color.
    pub status: Status,
    /// The status text, e.g. `clean` or `3 lint`.
    pub note: String,
}

/// A grid of labeled images with a header, split into pages that stay
/// legible when viewed whole.
///
/// ```ignore
/// let mut sheet = ContactSheet::new("buttons");
/// sheet.push(SheetItem { label: "hover".into(), image: shot.image.clone(), ..item });
/// let pages = sheet.save(&mut Composer::new()?, suite.dir())?;
/// ```
#[derive(Clone, Debug, Default)]
pub struct ContactSheet {
    /// The title (the suite's name).
    pub title: String,
    /// Summary counts, shown top right.
    pub summary: String,
    /// One sentence about the suite, shown under the title.
    pub description: String,
    /// Run facts, shown under the description.
    pub subtitle: String,
    /// The cells, in order.
    pub items: Vec<SheetItem>,
}

const WIDTH: f32 = 1200.;
const MARGIN: f32 = 24.;
const COLUMNS: usize = 3;
const GAP: f32 = 16.;
const THUMB_H: f32 = 232.;
const CAPTION: f32 = 48.;
const HEADER: f32 = 104.;
const ROWS_PER_PAGE: usize = 5;
const SCALE: f32 = 2.;

impl ContactSheet {
    /// An empty sheet titled `title`.
    pub fn new(title: &str) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    /// Adds a cell.
    pub fn push(&mut self, item: SheetItem) {
        self.items.push(item);
    }

    /// Renders every page.
    pub fn render(&self, composer: &mut Composer) -> Result<Vec<RgbaImage>> {
        let per_page = COLUMNS * ROWS_PER_PAGE;
        let pages = self.items.len().div_ceil(per_page).max(1);
        (0..pages)
            .map(|page| {
                let items = self.items.iter().skip(page * per_page).take(per_page);
                self.render_page(composer, items.collect(), page, pages)
            })
            .collect()
    }

    /// Renders and writes `sheet.png` (then `sheet-2.png`, …) into `dir`,
    /// removing pages left over from a longer sheet.
    pub fn save(&self, composer: &mut Composer, dir: &std::path::Path) -> Result<Vec<PathBuf>> {
        std::fs::create_dir_all(dir)?;
        let pages = self.render(composer)?;
        let mut paths = Vec::new();
        for (ix, image) in pages.iter().enumerate() {
            let path = dir.join(page_file(ix));
            image
                .save_with_format(&path, image::ImageFormat::Png)
                .with_context(|| format!("writing {}", path.display()))?;
            paths.push(path);
        }
        let mut stale = pages.len();
        while dir.join(page_file(stale)).exists() {
            std::fs::remove_file(dir.join(page_file(stale)))?;
            stale += 1;
        }
        Ok(paths)
    }

    fn render_page(
        &self,
        composer: &mut Composer,
        items: Vec<&SheetItem>,
        page: usize,
        pages: usize,
    ) -> Result<RgbaImage> {
        let cell_w = ((WIDTH - MARGIN * 2. - GAP * (COLUMNS as f32 - 1.)) / COLUMNS as f32).floor();
        let rows = items.len().div_ceil(COLUMNS).max(1);
        let height = MARGIN + HEADER + rows as f32 * (THUMB_H + CAPTION + GAP) - GAP + MARGIN;
        let cells = items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let column = (ix % COLUMNS) as f32;
                let row = (ix / COLUMNS) as f32;
                let cell = Rect::new(
                    MARGIN + column * (cell_w + GAP),
                    MARGIN + HEADER + row * (THUMB_H + CAPTION + GAP),
                    cell_w,
                    THUMB_H,
                );
                let (w, h) = item.image.dimensions();
                let fit = ((cell.w - 16.) / w as f32).min((cell.h - 16.) / h as f32);
                let (tw, th) = ((w as f32 * fit).round(), (h as f32 * fit).round());
                let thumb = Rect::new(
                    (cell.x + (cell.w - tw) / 2.).round(),
                    (cell.y + (cell.h - th) / 2.).round(),
                    tw,
                    th,
                );
                let image = composer.image(&resample(
                    &item.image,
                    (tw * SCALE) as u32,
                    (th * SCALE) as u32,
                ));
                Cell {
                    item: (*item).clone(),
                    cell,
                    thumb,
                    image,
                }
            })
            .collect::<Vec<_>>();
        let cells = Rc::new(cells);
        let title = self.title.clone();
        let subtitle = if pages > 1 {
            format!("{} · page {} of {pages}", self.subtitle, page + 1)
        } else {
            self.subtitle.clone()
        };
        let summary = self.summary.clone();
        let description = self.description.clone();
        composer.render(WIDTH, height, SCALE, move |_, _| {
            let mut root = backdrop()
                .child(
                    placed(Rect::new(MARGIN, MARGIN, WIDTH - MARGIN * 2., HEADER - 24.))
                        .flex()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(section("Lightbox"))
                                .child(heading(title.clone(), 20., DARK.text))
                                .when(!description.is_empty(), |header| {
                                    header.child(label(description.clone(), 12., DARK.text_muted))
                                })
                                .child(mono(subtitle.clone(), 11., DARK.text_faint)),
                        )
                        .child(pill(summary.clone(), DARK.accent)),
                )
                .child(placed(Rect::new(0., MARGIN + HEADER - 16., WIDTH, 1.)).bg(DARK.line));
            for cell in cells.iter() {
                root = root.children(cell.elements());
            }
            root.into_any_element()
        })
    }
}

fn page_file(ix: usize) -> String {
    if ix == 0 {
        "sheet.png".into()
    } else {
        format!("sheet-{}.png", ix + 1)
    }
}

struct Cell {
    item: SheetItem,
    cell: Rect,
    thumb: Rect,
    image: Arc<RenderImage>,
}

impl Cell {
    fn elements(&self) -> Vec<gpui::AnyElement> {
        let Cell {
            item,
            cell,
            thumb,
            image,
        } = self;
        let caption = Rect::new(cell.x, cell.bottom(), cell.w, CAPTION);
        vec![
            placed(Rect::new(cell.x, cell.y, cell.w, cell.h + CAPTION))
                .bg(DARK.surface)
                .border_1()
                .border_color(DARK.line)
                .into_any_element(),
            placed(Rect::new(cell.x + 1., cell.bottom(), cell.w - 2., 1.))
                .bg(DARK.line)
                .into_any_element(),
            placed(thumb.inflate(1.))
                .border_1()
                .border_color(DARK.line_strong)
                .into_any_element(),
            picture(image.clone(), *thumb),
            placed(Rect::new(
                caption.x + 12.,
                caption.y + 7.,
                caption.w - 24.,
                36.,
            ))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(heading(item.label.clone(), 13., DARK.text))
                    .child(mono(item.detail.clone(), 11., DARK.text_muted)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .child(div().size(px(8.)).rounded(px(4.)).bg(item.status.color()))
                    .child(label(item.note.clone(), 11., DARK.text_muted)),
            )
            .into_any_element(),
        ]
    }
}

/// Builds and writes a suite's contact sheet (`sheet.png`, …) from its
/// records: every shot (with its lint and golden status), matrix and film.
pub fn write_suite_sheet(suite: &Suite, composer: &mut Composer) -> Result<Vec<PathBuf>> {
    let records = suite.records()?;
    let mut sheet = ContactSheet::new(suite.name());
    let in_matrix = records
        .iter()
        .filter_map(|record| match record {
            Record::Matrix(matrix) => Some(matrix.cells.iter().map(|cell| cell.shot.clone())),
            _ => None,
        })
        .flatten()
        .collect::<HashSet<_>>();
    let load = |file: &str| -> Result<RgbaImage> {
        let path = suite.path(file);
        Ok(image::open(&path)
            .with_context(|| format!("reading {}", path.display()))?
            .into_rgba8())
    };
    let (mut shots, mut films, mut lints, mut goldens) = (0, 0, 0, (0, 0));
    for record in &records {
        match record {
            Record::Shot(shot) if !in_matrix.contains(&shot.name) => {
                shots += 1;
                let (status, note) = shot_status(&records, &shot.name);
                sheet.push(SheetItem {
                    label: shot.name.clone(),
                    detail: format!(
                        "{}×{} @{}× · {}",
                        trim(shot.meta.size[0]),
                        trim(shot.meta.size[1]),
                        trim(shot.meta.scale),
                        shot.meta.appearance.label().to_lowercase()
                    ),
                    image: load(&shot.image)?,
                    status,
                    note,
                });
            }
            Record::Matrix(matrix) => sheet.push(SheetItem {
                label: matrix.name.clone(),
                detail: format!("matrix · {} variants", matrix.cells.len()),
                image: load(&matrix.image)?,
                status: Status::Neutral,
                note: "grid".into(),
            }),
            Record::Film(film) => {
                films += 1;
                let failed = film.assertions.iter().any(|assertion| !assertion.passed);
                let problems = film
                    .findings
                    .iter()
                    .filter(|finding| finding.kind.is_problem())
                    .count();
                let (status, note) = if failed {
                    (Status::Crit, "assertion failed".to_string())
                } else if problems > 0 {
                    (
                        Status::Warn,
                        format!("{problems} finding{}", plural(problems)),
                    )
                } else if film.assertions.is_empty() {
                    (Status::Neutral, "film".into())
                } else {
                    (Status::Ok, format!("{} passed", film.assertions.len()))
                };
                let middle = &film.frames[film.frames.len() / 2];
                sheet.push(SheetItem {
                    label: film.name.clone(),
                    detail: format!(
                        "film · {} frames · {} ms · frame {} shown",
                        film.frames.len(),
                        trim(film.duration_ms as f32),
                        middle.index
                    ),
                    image: load(&middle.image)?,
                    status,
                    note,
                });
            }
            Record::Lint(lint) => {
                lints += 1;
                // Linted captures have no shot record: show their annotation.
                let captured = !records
                    .iter()
                    .any(|record| matches!(record, Record::Shot(shot) if shot.name == lint.name));
                if captured {
                    let (status, note) = shot_status(&records, &lint.name);
                    sheet.push(SheetItem {
                        label: lint.name.clone(),
                        detail: format!("lint · {} · annotated", lint.spec),
                        image: load(&lint.annotated)?,
                        status,
                        note,
                    });
                }
            }
            Record::Golden(golden) => {
                if golden.passed {
                    goldens.0 += 1;
                } else {
                    goldens.1 += 1;
                    let file = golden.comparison.as_deref().unwrap_or(&golden.actual);
                    sheet.push(SheetItem {
                        label: golden.name.clone(),
                        detail: "golden · expected | actual | diff".into(),
                        image: load(file)?,
                        status: Status::Crit,
                        note: "mismatch".into(),
                    });
                }
            }
            Record::Shot(_) => {}
        }
    }
    let mut summary = vec![format!("{shots} shot{}", plural(shots))];
    if films > 0 {
        summary.push(format!("{films} film{}", plural(films)));
    }
    if lints > 0 {
        summary.push(format!("{lints} lint{}", plural(lints)));
    }
    if goldens.0 + goldens.1 > 0 {
        summary.push(format!("{} / {} goldens", goldens.0, goldens.0 + goldens.1));
    }
    sheet.summary = summary.join(" · ");
    sheet.description = suite.description().unwrap_or_default();
    let run = RunInfo::current();
    sheet.subtitle = format!("{} · {} · {}", run.timestamp, run.git_sha, run.profile);
    sheet.save(composer, suite.dir())
}

/// A shot's status from its lint (and a golden of the same name).
pub(crate) fn shot_status(records: &[Record], name: &str) -> (Status, String) {
    let mut status = Status::Neutral;
    let mut notes = Vec::new();
    for record in records {
        match record {
            Record::Lint(lint) if lint.name == name => {
                let errors = lint
                    .violations
                    .iter()
                    .filter(|violation| violation.severity == Severity::Error)
                    .count();
                let lint_status = match (lint.violations.len(), errors) {
                    (0, _) => Status::Ok,
                    (_, 0) => Status::Warn,
                    _ => Status::Crit,
                };
                let note = if lint.violations.is_empty() {
                    "lint clean".to_string()
                } else {
                    rules_note(lint)
                };
                status = status.max(lint_status);
                notes.push(note);
            }
            Record::Golden(golden) if golden.name == name => {
                status = status.max(if golden.passed {
                    Status::Ok
                } else {
                    Status::Crit
                });
                notes.push(
                    if golden.passed {
                        "golden match"
                    } else {
                        "golden mismatch"
                    }
                    .into(),
                );
            }
            _ => {}
        }
    }
    if notes.is_empty() {
        notes.push("shot".into());
    }
    (status, notes.join(" · "))
}

/// `contrast`, `spacing ×5`, or `4 rules ×9` when there are many.
fn rules_note(lint: &crate::manifest::LintRecord) -> String {
    let mut rules: Vec<(&str, usize)> = Vec::new();
    for violation in &lint.violations {
        match rules
            .iter_mut()
            .find(|(rule, _)| *rule == violation.rule.name())
        {
            Some((_, count)) => *count += 1,
            None => rules.push((violation.rule.name(), 1)),
        }
    }
    let one = |(rule, count): &(&str, usize)| {
        if *count == 1 {
            rule.to_string()
        } else {
            format!("{rule} ×{count}")
        }
    };
    match rules.as_slice() {
        [single] => one(single),
        [first, second] => format!("{}, {}", one(first), one(second)),
        _ => format!("{} rules ×{}", rules.len(), lint.violations.len()),
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
