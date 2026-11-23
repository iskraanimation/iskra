//! A storyboard's pages written (plan §4.7): as one PDF, with krilla, the pictures as images and
//! the texts as text in Inter, embedded, so that they can be searched and copied; and as a
//! picture of each page, a PNG, drawn from the same layout. What is on the pages is
//! [`crate::pages`]'s.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use iskra_model::{Project, ShotId, StoryboardId, Work};
use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Size, Transform};
use krilla::image::Image;
use krilla::metadata::Metadata;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::text::{Font, TextDirection};
use rayon::prelude::*;
use vello_cpu::kurbo::{Affine, BezPath, Rect};
use vello_cpu::{Pixmap, RenderContext, Resources};

use crate::ExportError;
use crate::pages::{self, Heading, Laid, PageOptions, Role};

/// A panel's picture made: rows of sRGB pixels, three bytes each, on the paper's white.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Made {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// The pictures of a layout, by the shot and the frame of it they are of.
pub type Pictures = HashMap<(ShotId, u32), Made>;

/// The colour each kind of text is printed in.
fn ink(role: Role) -> [u8; 3] {
    match role {
        Role::Title | Role::Number | Role::Action => [17, 17, 17],
        Role::Dialogue | Role::Length | Role::Timecode | Role::Version | Role::Footer => [80, 80, 80],
        Role::Notes => [110, 110, 110],
        Role::Transition => [255, 255, 255],
    }
}

/// The frame round a picture, and the lines to write on.
const FRAME: ([u8; 3], f32) = ([60, 60, 60], 0.75);
const RULE: ([u8; 3], f32) = ([180, 180, 180], 0.5);
/// Behind a transition's mark, so that it can be read over any picture.
const CHIP: [u8; 3] = [0, 0, 0];

/// Make the pictures of the panels on `laid`'s pages, of their shots' boards, as the Panels
/// view's cards are, in grey if `grey`: several at a time. Every shot that has a panel on
/// them has to be loaded.
pub fn make_pictures(project: &Project, laid: &Laid, grey: bool) -> Result<Pictures, ExportError> {
    let mut wanted: HashMap<(ShotId, u32), [u32; 2]> = HashMap::new();
    for picture in laid.pages.iter().flat_map(|page| &page.pictures) {
        let size = wanted.entry((picture.shot, picture.frame)).or_default();
        *size = [size[0].max(picture.pixels[0]), size[1].max(picture.pixels[1])];
    }
    for (shot, _) in wanted.keys() {
        let shot = project.shot(*shot).ok_or_else(|| ExportError::Invalid(format!("no shot {}", shot.0)))?;
        if !shot.loaded {
            return Err(ExportError::Invalid(format!("the drawings of `{}` are not loaded: load the shot first", shot.name)));
        }
    }
    let wanted: Vec<((ShotId, u32), [u32; 2])> = wanted.into_iter().collect();
    let made = wanted.par_iter().filter_map(|((id, frame), [width, _])| {
        let shot = project.shot(*id)?;
        let view = iskra_render::View::frame(&project.settings, f64::from(*width) / f64::from(project.settings.width.max(1)));
        let image = iskra_render::render(&shot.only(Work::Boards), &project.palette, project.settings.working_space, *frame, &view);
        Some(((*id, *frame), on_paper(&image, grey)))
    });
    Ok(made.collect())
}

/// `image` laid on white, as it is printed; in grey if `grey`.
fn on_paper(image: &iskra_render::Image, grey: bool) -> Made {
    let mut rgb = Vec::with_capacity(image.pixels.len() * 3);
    for [r, g, b, a] in &image.pixels {
        let alpha = f32::from(*a) / 255.0;
        let over = |value: u8| f32::from(value) * alpha + 255.0 * (1.0 - alpha);
        let [r, g, b] = [over(*r), over(*g), over(*b)];
        if grey {
            let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).round() as u8;
            rgb.extend([luma; 3]);
        } else {
            rgb.extend([r, g, b].map(|value| value.round() as u8));
        }
    }
    Made { width: image.width, height: image.height, rgb }
}

fn fill(colour: [u8; 3]) -> Fill {
    Fill { paint: rgb::Color::new(colour[0], colour[1], colour[2]).into(), opacity: NormalizedF32::ONE, ..Fill::default() }
}

fn stroke((colour, width): ([u8; 3], f32)) -> Stroke {
    Stroke { paint: rgb::Color::new(colour[0], colour[1], colour[2]).into(), width, ..Stroke::default() }
}

/// The rectangle of `area` as a path, or none if it has no size.
fn rect_path(area: pages::Area) -> Option<krilla::geom::Path> {
    let mut path = PathBuilder::new();
    path.push_rect(krilla::geom::Rect::from_xywh(area.x, area.y, area.width, area.height)?);
    path.finish()
}

/// The room behind a transition's mark.
fn chip(text: &pages::Text) -> pages::Area {
    let pad = text.size * 0.25;
    let width = pages::text_width(&text.text, text.size, false);
    pages::Area { x: text.at[0] - pad, y: text.at[1] - pad, width: width + 2.0 * pad, height: text.size * pages::LINE + 2.0 * pad }
}

/// The PDF of `laid`, with `pictures` for its panels, titled `title`.
pub fn pdf(laid: &Laid, pictures: &Pictures, title: &str) -> Result<Vec<u8>, ExportError> {
    let invalid = |why: String| ExportError::Invalid(why);
    let font = |bytes: &'static [u8]| Font::new(bytes.into(), 0).ok_or_else(|| invalid("the font Inter could not be read".to_string()));
    let (regular, semi_bold) = (font(pages::REGULAR)?, font(pages::SEMI_BOLD)?);
    // Each picture once in the file, however many pages show it.
    let images: HashMap<(ShotId, u32), Image> = pictures
        .iter()
        .map(|(at, made)| {
            let rgba = made.rgb.as_chunks::<3>().0.iter().flat_map(|[r, g, b]| [*r, *g, *b, 255]).collect();
            (*at, Image::from_rgba8(rgba, made.width, made.height))
        })
        .collect();
    let mut document = Document::new();
    document.set_metadata(Metadata::new().title(title.to_string()).creator("Iskra".to_string()));
    let [width, height] = laid.size;
    for page in &laid.pages {
        let settings = PageSettings::from_wh(width, height).ok_or_else(|| invalid(format!("a page of {width} by {height} points")))?;
        let mut sheet = document.start_page_with(settings);
        let mut surface = sheet.surface();
        for picture in &page.pictures {
            let area = picture.area;
            if let (Some(image), Some(size)) = (images.get(&(picture.shot, picture.frame)), Size::from_wh(area.width, area.height)) {
                surface.push_transform(&Transform::from_translate(area.x, area.y));
                surface.draw_image(image.clone(), size);
                surface.pop();
            }
            if picture.framed
                && let Some(path) = rect_path(area)
            {
                surface.set_fill(None);
                surface.set_stroke(Some(stroke(FRAME)));
                surface.draw_path(&path);
            }
        }
        surface.set_fill(None);
        surface.set_stroke(Some(stroke(RULE)));
        for [[x0, y0], [x1, y1]] in &page.lines {
            let mut path = PathBuilder::new();
            path.move_to(*x0, *y0);
            path.line_to(*x1, *y1);
            if let Some(path) = path.finish() {
                surface.draw_path(&path);
            }
        }
        surface.set_stroke(None);
        for text in &page.texts {
            if text.role == Role::Transition
                && let Some(path) = rect_path(chip(text))
            {
                surface.set_fill(Some(Fill { opacity: NormalizedF32::new(0.6).unwrap_or(NormalizedF32::ONE), ..fill(CHIP) }));
                surface.draw_path(&path);
            }
            let bold = text.role.bold();
            surface.set_fill(Some(fill(ink(text.role))));
            let baseline = Point::from_xy(text.at[0], text.at[1] + pages::ascent(text.size, bold));
            let face = if bold { semi_bold.clone() } else { regular.clone() };
            surface.draw_text(baseline, face, text.size, &text.text, false, TextDirection::Auto);
        }
        surface.finish();
        sheet.finish();
    }
    document.finish().map_err(|why| invalid(format!("the PDF could not be made: {why:?}")))
}

fn colour([r, g, b]: [u8; 3], alpha: f32) -> vello_cpu::color::AlphaColor<vello_cpu::color::Srgb> {
    vello_cpu::color::AlphaColor::from_rgba8(r, g, b, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// A picture of page `at` of `laid`, `dpi` pixels an inch: rows of sRGB pixels, three bytes
/// each, its width and height.
pub fn page_image(laid: &Laid, at: usize, pictures: &Pictures, dpi: u32) -> Option<Made> {
    let page = laid.pages.get(at)?;
    let scale = f64::from(dpi.max(1)) / 72.0;
    let [width, height] = laid.size.map(|points| (f64::from(points) * scale).round().max(1.0) as u32);
    let (wide, tall) = (u16::try_from(width).ok()?, u16::try_from(height).ok()?);
    let mut rgb = vec![255u8; width as usize * height as usize * 3];
    // The pictures first, each scaled into its place.
    for picture in &page.pictures {
        let Some(made) = pictures.get(&(picture.shot, picture.frame)) else { continue };
        let area = picture.area;
        let (left, top) = ((f64::from(area.x) * scale).round() as i64, (f64::from(area.y) * scale).round() as i64);
        let (across, down) = ((f64::from(area.width) * scale).round() as i64, (f64::from(area.height) * scale).round() as i64);
        for y in 0..down.max(0) {
            for x in 0..across.max(0) {
                let (px, py) = (left + x, top + y);
                if px < 0 || py < 0 || px >= i64::from(width) || py >= i64::from(height) {
                    continue;
                }
                let sx = ((x as f64 + 0.5) / across as f64 * f64::from(made.width)) as usize;
                let sy = ((y as f64 + 0.5) / down as f64 * f64::from(made.height)) as usize;
                let from = (sy.min(made.height as usize - 1) * made.width as usize + sx.min(made.width as usize - 1)) * 3;
                let to = (py as usize * width as usize + px as usize) * 3;
                rgb[to..to + 3].copy_from_slice(&made.rgb[from..from + 3]);
            }
        }
    }
    // Over them, the frames, the lines and the texts.
    let mut context = RenderContext::new(wide, tall);
    context.set_transform(Affine::scale(scale));
    let area_rect = |area: pages::Area| Rect::new(f64::from(area.x), f64::from(area.y), f64::from(area.right()), f64::from(area.bottom()));
    for picture in page.pictures.iter().filter(|picture| picture.framed) {
        context.set_stroke(vello_cpu::kurbo::Stroke::new(f64::from(FRAME.1)));
        context.set_paint(colour(FRAME.0, 1.0));
        context.stroke_rect(&area_rect(picture.area));
    }
    for [[x0, y0], [x1, y1]] in &page.lines {
        let mut path = BezPath::new();
        path.move_to((f64::from(*x0), f64::from(*y0)));
        path.line_to((f64::from(*x1), f64::from(*y1)));
        context.set_stroke(vello_cpu::kurbo::Stroke::new(f64::from(RULE.1)));
        context.set_paint(colour(RULE.0, 1.0));
        context.stroke_path(&path);
    }
    for text in &page.texts {
        if text.role == Role::Transition {
            context.set_paint(colour(CHIP, 0.6));
            context.fill_rect(&area_rect(chip(text)));
        }
        let face = if text.role.bold() { pages::SEMI_BOLD } else { pages::REGULAR };
        // The path's top is the top of the line, as the layout's is.
        let (path, ..) = crate::burn::text_path_in(face, &text.text, text.size);
        context.set_paint(colour(ink(text.role), 1.0));
        context.fill_path(&(Affine::translate((f64::from(text.at[0]), f64::from(text.at[1]))) * path));
    }
    context.flush();
    let mut pixmap = Pixmap::new(wide, tall);
    context.render(&mut pixmap, &mut Resources::new());
    for (pixel, over) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(pixmap.data()) {
        if over.a == 0 {
            continue;
        }
        // The pixmap's colours are times their alpha.
        let keep = 1.0 - f32::from(over.a) / 255.0;
        for (value, on) in pixel.iter_mut().zip([over.r, over.g, over.b]) {
            *value = (f32::from(on) + f32::from(*value) * keep).round().min(255.0) as u8;
        }
    }
    Some(Made { width, height, rgb })
}

/// What [`export_pages`] wrote: the PDF, and the pictures of its pages if they were asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub pdf: PathBuf,
    pub pages: Vec<PathBuf>,
}

/// Lay out storyboard `storyboard` of `project` on pages as `options` say, and write them to
/// the PDF at `path`; with `pngs`, a picture of each page beside it too, at the options' dpi,
/// named for the PDF and the page's number. A file that is there already is written over: the
/// window asks first.
pub fn export_pages(
    project: &Project,
    storyboard: StoryboardId,
    options: &PageOptions,
    heading: &Heading,
    path: &Path,
    pngs: bool,
) -> Result<Written, ExportError> {
    let Some(board) = project.storyboard(storyboard) else { return Err(ExportError::Invalid(format!("no storyboard {}", storyboard.0))) };
    let laid = pages::lay_out(project, board, options, heading);
    if laid.pages.is_empty() {
        return Err(ExportError::Invalid(format!("`{}` has no panels to print", board.name)));
    }
    let pictures = make_pictures(project, &laid, options.grey)?;
    let failed = |path: &Path, why: String| ExportError::Io { path: path.to_path_buf(), why };
    let bytes = pdf(&laid, &pictures, &format!("{} · {}", heading.project, board.name))?;
    std::fs::write(path, bytes).map_err(|why| failed(path, why.to_string()))?;
    let mut written = Written { pdf: path.to_path_buf(), pages: Vec::new() };
    if pngs {
        let stem = path.file_stem().map_or_else(|| "Storyboard".to_string(), |stem| stem.to_string_lossy().into_owned());
        let digits = laid.pages.len().to_string().len().max(2);
        let files: Vec<Result<PathBuf, ExportError>> = (0..laid.pages.len())
            .into_par_iter()
            .map(|at| {
                let file = path.with_file_name(format!("{stem}_{:0digits$}.png", at + 1));
                let made =
                    page_image(&laid, at, &pictures, options.dpi).ok_or_else(|| ExportError::Invalid(format!("page {} is too large to draw", at + 1)))?;
                image::save_buffer(&file, &made.rgb, made.width, made.height, image::ColorType::Rgb8).map_err(|why| failed(&file, why.to_string()))?;
                Ok(file)
            })
            .collect();
        written.pages = files.into_iter().collect::<Result<_, _>>()?;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_model::{Clip, Panel};

    use super::*;
    use crate::pages::Mode;

    /// A storyboard of one shot of 24 frames, two panels, a stroke of red on its board on
    /// frame 0: the project, and the storyboard.
    fn film() -> (Project, StoryboardId) {
        let mut project = Project::default();
        let id = project.shots[0].id;
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.duration = 24;
        shot.panels = vec![
            Panel { frame: 0, action: "A dark hall.".to_string(), dialogue: "Hello?".to_string(), ..Panel::default() },
            Panel { frame: 12, ..Panel::default() },
        ];
        let board = Arc::make_mut(&mut project.storyboards[0]);
        board.put(Clip { shot: id, start: 0, in_point: 0, out_point: 24 }, false).unwrap();
        let board = board.id;
        (project, board)
    }

    #[test]
    fn the_pdf_has_a_page_for_each_page_laid_its_texts_as_text_and_its_pictures() {
        let (project, board) = film();
        let options = PageOptions::of(Mode::Presentation);
        let heading = Heading { project: "Hall".to_string(), date: "2026-10-28".to_string() };
        let laid = pages::lay_out(&project, project.storyboard(board).unwrap(), &options, &heading);
        let pictures = make_pictures(&project, &laid, false).unwrap();
        assert_eq!(pictures.len(), 2, "one a panel");
        let bytes = pdf(&laid, &pictures, "Hall").unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(bytes.starts_with(b"%PDF-"), "a PDF");
        // Two pages, Inter embedded in both weights, and the pictures: the two are blank
        // alike, and krilla keeps a picture once however often it is drawn.
        assert_eq!(text.matches("/Type/Page/").count(), 2, "two pages");
        assert!(text.contains("+Inter-Regular") && text.contains("+Inter-SemiBold") && text.contains("/FontFile2"), "the fonts are embedded");
        assert!(text.contains("/ToUnicode"), "the texts can be copied");
        assert_eq!(text.matches("/Subtype/Image").count(), 1);
    }

    #[test]
    fn a_page_is_drawn_white_with_its_pictures_and_texts_where_they_were_laid() {
        let (project, board) = film();
        let options = PageOptions::of(Mode::Sheets);
        let laid = pages::lay_out(&project, project.storyboard(board).unwrap(), &options, &Heading::default());
        // A picture of plain blue, to find on the page.
        let mut pictures = Pictures::new();
        for picture in &laid.pages[0].pictures {
            let [width, height] = picture.pixels;
            pictures.insert((picture.shot, picture.frame), Made { width, height, rgb: [0, 0, 255].repeat((width * height) as usize) });
        }
        let made = page_image(&laid, 0, &pictures, 72).unwrap();
        assert_eq!([made.width, made.height], laid.size.map(|points| points.round() as u32));
        let at = |x: f32, y: f32| {
            let from = (y.round() as usize * made.width as usize + x.round() as usize) * 3;
            [made.rgb[from], made.rgb[from + 1], made.rgb[from + 2]]
        };
        let area = laid.pages[0].pictures[0].area;
        assert_eq!(at(area.x + area.width / 2.0, area.y + area.height / 2.0), [0, 0, 255], "the picture");
        assert_eq!(at(laid.size[0] / 2.0, laid.size[1] - 2.0), [255, 255, 255], "the paper");
        // The first panel's action is ink, somewhere on its line.
        let action = laid.pages[0].texts.iter().find(|text| text.role == Role::Action).unwrap();
        let width = pages::text_width(&action.text, action.size, false);
        let inked = (0..width as usize).any(|x| (0..action.size as usize).any(|y| at(action.at[0] + x as f32, action.at[1] + y as f32)[0] < 128));
        assert!(inked, "the action is drawn where it was laid");
    }

    #[test]
    fn export_writes_the_pdf_and_a_png_of_each_page_and_wants_the_shots_loaded() {
        let (mut project, board) = film();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Hall boards.pdf");
        let options = PageOptions { rows: 1, columns: 1, dpi: 36, ..PageOptions::of(Mode::Sheets) };
        let written = export_pages(&project, board, &options, &Heading::default(), &path, true).unwrap();
        assert_eq!(written.pdf, path);
        assert!(std::fs::read(&path).unwrap().starts_with(b"%PDF-"));
        let names: Vec<String> = written.pages.iter().map(|page| page.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["Hall boards_01.png", "Hall boards_02.png"]);
        assert!(written.pages.iter().all(|page| page.exists()));
        // A shot not in memory is not printed from nothing.
        Arc::make_mut(&mut project.shots[0]).loaded = false;
        assert!(matches!(export_pages(&project, board, &options, &Heading::default(), &path, false), Err(ExportError::Invalid(_))));
    }
}
