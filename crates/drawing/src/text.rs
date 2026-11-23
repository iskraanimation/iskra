//! Text (plan §4.19): a box of text in a drawing, and its layout into the vector fills it is
//! drawn as.
//!
//! A box's text is shaped by `harfrust`, which gives the font's own kerning and ligatures, and
//! its glyphs' outlines are read by `skrifa`. Breaking it into lines, aligning them, tracking,
//! kerning by hand and leading are done here, over the shaped glyphs. What comes out is
//! outlines in the drawing's pixels, which the drawing keeps as vector art: nothing that draws a
//! text reads a font, only what lays it out.

use std::collections::BTreeMap;

use kurbo::{BezPath, Point, Shape as _};
use serde::{Deserialize, Serialize};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, MetadataProvider};

use crate::SwatchId;
use crate::fonts::{BUILT_IN_FAMILY, Fonts, REGULAR};

/// How a box's lines lie across it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    #[default]
    Left,
    Centre,
    Right,
    /// Each line but a paragraph's last stretched to the box's width at its spaces.
    Justified,
}

/// How a box's text looks: one format to a box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TextFormat {
    /// The font's family and its style in it ("Regular", "Bold", "Italic"…), by name. A family
    /// the machine lacks is laid out in Inter, and keeps its name.
    pub font: String,
    pub style: String,
    /// In the drawing's pixels: the height of a line at leading 100 %.
    pub size: f64,
    pub align: Align,
    /// Space added after every letter, in thousandths of the size.
    pub tracking: f64,
    /// The height of a line, in percent of the size.
    pub leading: f64,
    pub swatch: SwatchId,
    /// A swatch the box is filled with behind its letters, as a subtitle's: none for none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<SwatchId>,
    /// How far the background reaches past the letters, in percent of the size.
    pub padding: f64,
}

impl Default for TextFormat {
    fn default() -> Self {
        TextFormat {
            font: BUILT_IN_FAMILY.to_string(),
            style: REGULAR.to_string(),
            size: 48.0,
            align: Align::Left,
            tracking: 0.0,
            leading: 120.0,
            swatch: SwatchId(1),
            background: None,
            padding: 25.0,
        }
    }
}

impl TextFormat {
    /// Whether every number is one a layout can take.
    pub fn is_valid(&self) -> bool {
        self.size.is_finite() && self.size > 0.0 && self.tracking.is_finite() && self.leading.is_finite() && self.leading > 0.0 && self.padding.is_finite()
    }
}

/// A box of text in a drawing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBox {
    /// Lines are broken where it has a `\n`.
    pub text: String,
    /// Its top left corner, in the drawing's pixels.
    pub at: [f64; 2],
    /// The width its lines are wrapped in, in the drawing's pixels. Without one it is as wide as
    /// its longest line, and its lines break only at a `\n`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// How tall the box is at least, in the drawing's pixels, as its handles were dragged: its
    /// lines are not cut by it, and go on below. Without one it is as tall as its lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default)]
    pub format: TextFormat,
    /// Kerning by hand: space put before a letter, by its place in the text (in letters, from
    /// 0), in thousandths of the size.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kerning: BTreeMap<u32, f64>,
}

impl TextBox {
    pub fn new(at: [f64; 2], width: Option<f64>, format: TextFormat) -> TextBox {
        TextBox { text: String::new(), at, width, height: None, format, kerning: BTreeMap::new() }
    }

    /// How many letters it has, line breaks counted.
    pub fn letters(&self) -> usize {
        self.text.chars().count()
    }

    /// Its letters from `from` up to `to` (places between letters, from 0) taken out and
    /// `insert` put there. The kerning set before a letter goes with the letter; that of a letter
    /// taken out goes too. Places past the end are taken as the end.
    pub fn splice(&mut self, from: usize, to: usize, insert: &str) {
        let letters = self.letters();
        let (from, to) = (from.min(letters), to.min(letters).max(from.min(letters)));
        let byte = |at: usize| self.text.char_indices().nth(at).map_or(self.text.len(), |(byte, _)| byte);
        let (start, end) = (byte(from), byte(to));
        self.text.replace_range(start..end, insert);
        let (added, removed) = (insert.chars().count() as i64, (to - from) as i64);
        self.kerning = std::mem::take(&mut self.kerning)
            .into_iter()
            .filter_map(|(at, kern)| match at as usize {
                at if at < from => Some((at as u32, kern)),
                at if at < to => None,
                at => Some(((at as i64 - removed + added) as u32, kern)),
            })
            .collect();
    }
}

/// A line of a laid-out box.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The place in the text of its first letter.
    pub start: usize,
    /// Where the cursor goes before each of its letters, and after its last, across.
    pub carets: Vec<f64>,
    pub top: f64,
    pub baseline: f64,
    pub height: f64,
}

/// A box laid out, in the drawing's pixels.
#[derive(Clone, Debug, Default)]
pub struct Laid {
    /// The glyphs' outlines, each of the box's swatch.
    pub glyphs: Vec<BezPath>,
    pub lines: Vec<Line>,
    /// Its width and height.
    pub size: [f64; 2],
    /// The font asked for, where the machine lacks it and Inter was used.
    pub stand_in: Option<String>,
}

impl Laid {
    /// The line the cursor is on at the place `at` in the text, and where across it is.
    pub fn caret(&self, at: usize) -> Option<(usize, f64)> {
        let line = self.lines.iter().rposition(|line| line.start <= at)?;
        let carets = &self.lines[line].carets;
        Some((line, carets[(at - self.lines[line].start).min(carets.len() - 1)]))
    }

    /// The place in the text nearest the point (`x`, `y`) of the drawing: on the line it is
    /// on, or the nearest line, between the two letters it is between.
    pub fn place_at(&self, x: f64, y: f64) -> usize {
        let Some(line) = self.lines.iter().rev().find(|line| y >= line.top).or_else(|| self.lines.first()) else { return 0 };
        let nearest = line.carets.iter().enumerate().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()));
        line.start + nearest.map_or(0, |(i, _)| i)
    }
}

impl Laid {
    /// The background of `text` laid out so, where its format has one: a box round its lines,
    /// as wide as its width or its widest line, the padding past them.
    pub fn background(&self, text: &TextBox) -> Option<BezPath> {
        text.format.background?;
        let pad = text.format.padding / 100.0 * text.format.size;
        let [x, y] = text.at;
        let [width, height] = [self.size[0], self.size[1].max(text.height.unwrap_or(0.0))];
        Some(kurbo::Rect::new(x - pad, y - pad, x + width + pad, y + height + pad).to_path(0.1))
    }
}

/// Lays out `text` in a face of `fonts`.
pub fn lay_out(text: &TextBox, fonts: &Fonts) -> Laid {
    let format = &text.format;
    let Some((face, stand_in)) = fonts.face_or_stand_in(&format.font, &format.style) else { return Laid::default() };
    let stand_in = stand_in.then(|| format.font.clone());
    let Some((data, index)) = face.data() else { return Laid { stand_in, ..Laid::default() } };
    Laid { stand_in, ..lay_out_with(text, &data, index) }
}

/// A few letters of each of the scripts a font may be for alone, to show it by where it can't
/// write its own name.
const SAMPLES: [&str; 12] =
    ["Абв Где", "Αβγ Δεζ", "אבג דהו", "أبجد هوز", "กขค งจฉ", "अआइ ईउऊ", "あいう アイウ", "漢字 文字", "가나다 라마", "ابپ تٹث", "ሀለሐ መሠ", "ᐁᐃᐅ ᐊᐱᐳ"];

/// What a preview of the face `index` of the font file `data` shows, called `name`: the name
/// where the font has its letters; or else letters of a script it has, or the first it has of
/// any, with `false` for not the name. `None` where it has none.
pub fn preview_text(data: &[u8], index: u32, name: &str) -> Option<(String, bool)> {
    let font = FontRef::from_index(data, index).ok()?;
    let charmap = font.charmap();
    let draws = |text: &str| text.chars().all(|c| c.is_whitespace() || charmap.map(c).is_some());
    if draws(name) {
        return Some((name.to_string(), true));
    }
    if let Some(sample) = SAMPLES.iter().find(|sample| draws(sample)) {
        return Some((sample.to_string(), false));
    }
    let any: String = charmap.mappings().filter_map(|(code, _)| char::from_u32(code)).filter(|c| !c.is_control() && !c.is_whitespace()).take(8).collect();
    (!any.is_empty()).then_some((any, false))
}

/// Lays out `text` in the face `index` of the font file `data`, whatever font it names.
pub fn lay_out_with(text: &TextBox, data: &[u8], index: u32) -> Laid {
    match FontRef::from_index(data, index) {
        Ok(font) => lay_out_in(text, &font),
        Err(_) => Laid::default(),
    }
}

/// A paragraph of a box, shaped: each of its letters' advance and kerning before it, and the
/// glyphs of each cluster at the letter it starts at.
struct Shaped {
    /// The place in the text of its first letter.
    start: usize,
    letters: Vec<char>,
    advance: Vec<f64>,
    kern: Vec<f64>,
    /// Each glyph: the letter (of the paragraph) its cluster starts at, its id, and where it
    /// is from the start of that letter, across and up.
    glyphs: Vec<(usize, u32, f64, f64)>,
}

fn shape(paragraph: &str, start: usize, text: &TextBox, font: &FontRef, scale: f64) -> Shaped {
    let format = &text.format;
    let letters: Vec<char> = paragraph.chars().collect();
    let letter_of: BTreeMap<usize, usize> = paragraph.char_indices().enumerate().map(|(letter, (byte, _))| (byte, letter)).collect();
    let mut advance = vec![0.0; letters.len()];
    let mut glyphs = Vec::new();
    if !letters.is_empty() {
        let data = harfrust::ShaperData::new(font);
        let shaper = data.shaper(font).build();
        let mut buffer = harfrust::UnicodeBuffer::new();
        buffer.push_str(paragraph);
        buffer.guess_segment_properties();
        let shaped = shaper.shape(buffer, harfrust::ShapeOptions::new());
        let (infos, positions) = (shaped.glyph_infos(), shaped.glyph_positions());
        // Each cluster's glyphs, and how far they reach, at the first of its letters.
        let mut clusters: Vec<(usize, f64)> = Vec::new();
        for (info, position) in infos.iter().zip(positions) {
            let letter = letter_of.range(..=info.cluster as usize).next_back().map_or(0, |(_, letter)| *letter);
            if clusters.last().is_none_or(|(at, _)| *at != letter) {
                clusters.push((letter, 0.0));
            }
            let reach = &mut clusters.last_mut().expect("pushed").1;
            glyphs.push((letter, info.glyph_id, *reach + position.x_offset as f64 * scale, position.y_offset as f64 * scale));
            *reach += position.x_advance as f64 * scale;
        }
        // A cluster's advance is shared among its letters, so that the cursor can stand inside a
        // ligature.
        let mut sorted = clusters.clone();
        sorted.sort_by_key(|(at, _)| *at);
        for (i, (at, reach)) in sorted.iter().enumerate() {
            let end = sorted.get(i + 1).map_or(letters.len(), |(next, _)| *next);
            let share = reach / (end - at).max(1) as f64;
            advance[*at..end].iter_mut().for_each(|advance| *advance = share);
        }
    }
    let tracking = format.tracking / 1000.0 * format.size;
    advance.iter_mut().for_each(|advance| *advance += tracking);
    let kern = (0..letters.len()).map(|i| text.kerning.get(&((start + i) as u32)).map_or(0.0, |kern| kern / 1000.0 * format.size)).collect();
    Shaped { start, letters, advance, kern, glyphs }
}

/// Where the paragraph breaks into lines that fit in `width`: the letter each line starts at,
/// and the one after its last. A line breaks after a space, or inside a word too long for a
/// line alone.
fn break_lines(shaped: &Shaped, width: Option<f64>) -> Vec<(usize, usize)> {
    let count = shaped.letters.len();
    let Some(width) = width else { return vec![(0, count)] };
    let mut lines = Vec::new();
    let (mut start, mut run, mut last_space) = (0, 0.0, None);
    let mut i = 0;
    while i < count {
        let step = shaped.kern[i] + shaped.advance[i];
        let space = shaped.letters[i].is_whitespace();
        // A space may hang past the edge; a letter may not, unless it is the line's first.
        if !space && run + step > width && i > start {
            let end = last_space.map_or(i, |space: usize| space + 1);
            lines.push((start, end));
            (start, run, last_space) = (end, 0.0, None);
            i = end;
            continue;
        }
        if space {
            last_space = Some(i);
        }
        run += step;
        i += 1;
    }
    lines.push((start, count));
    lines
}

fn lay_out_in(text: &TextBox, font: &FontRef) -> Laid {
    let format = &text.format;
    if !format.is_valid() {
        return Laid::default();
    }
    let upem = font.head().map_or(1000.0, |head| head.units_per_em() as f64);
    let scale = format.size / upem;
    let metrics = font.metrics(Size::new(format.size as f32), LocationRef::default());
    let (ascent, descent) = (metrics.ascent as f64, metrics.descent as f64);
    let height = format.size * format.leading / 100.0;
    // Half the leading above the letters and half below.
    let baseline_in = (height - (ascent - descent)) / 2.0 + ascent;
    let mut start = 0;
    let paragraphs: Vec<Shaped> = text
        .text
        .split('\n')
        .map(|paragraph| {
            let shaped = shape(paragraph, start, text, font, scale);
            start += shaped.letters.len() + 1;
            shaped
        })
        .collect();
    // The lines of each paragraph, with where its spaces at the end start and how wide it is
    // without them.
    let lines: Vec<(usize, usize, usize, usize, f64)> = paragraphs
        .iter()
        .enumerate()
        .flat_map(|(p, shaped)| {
            break_lines(shaped, text.width).into_iter().map(move |(from, to)| {
                let ink = (from..to).rev().find(|i| !shaped.letters[*i].is_whitespace()).map_or(from, |i| i + 1);
                (p, from, to, ink, (from..ink).map(|i| shaped.kern[i] + shaped.advance[i]).sum::<f64>())
            })
        })
        .collect();
    let box_width = text.width.unwrap_or_else(|| lines.iter().map(|line| line.4).fold(0.0, f64::max));
    let outlines = font.outline_glyphs();
    let mut laid = Laid { size: [box_width, height * lines.len() as f64], ..Laid::default() };
    for (n, &(p, from, to, ink_end, ink)) in lines.iter().enumerate() {
        let shaped = &paragraphs[p];
        let top = text.at[1] + height * n as f64;
        let baseline = top + baseline_in;
        let last_of_paragraph = lines.get(n + 1).is_none_or(|next| next.0 != p);
        // Justified, the spaces between its words take what it lacks of the width.
        let stretched = |i: usize| i < ink_end && shaped.letters[i].is_whitespace();
        let spaces = (from..to).filter(|i| stretched(*i)).count();
        let (mut x, stretch) = match format.align {
            Align::Left => (0.0, 0.0),
            Align::Centre => ((box_width - ink) / 2.0, 0.0),
            Align::Right => (box_width - ink, 0.0),
            Align::Justified if text.width.is_some() && !last_of_paragraph && spaces > 0 => (0.0, (box_width - ink).max(0.0) / spaces as f64),
            Align::Justified => (0.0, 0.0),
        };
        x += text.at[0];
        let mut carets = Vec::with_capacity(to - from + 1);
        let mut at_letter = BTreeMap::new();
        for i in from..to {
            carets.push(x);
            x += shaped.kern[i];
            at_letter.insert(i, x);
            x += shaped.advance[i] + if stretched(i) { stretch } else { 0.0 };
        }
        carets.push(x);
        for &(letter, glyph, across, up) in shaped.glyphs.iter().filter(|glyph| (from..to).contains(&glyph.0)) {
            let Some(outline) = outlines.get(skrifa::GlyphId::new(glyph)) else { continue };
            let mut pen = Pen { path: BezPath::new(), x: at_letter[&letter] + across, y: baseline - up };
            if outline.draw(DrawSettings::unhinted(Size::new(format.size as f32), LocationRef::default()), &mut pen).is_ok() && !pen.path.elements().is_empty()
            {
                laid.glyphs.push(pen.path);
            }
        }
        laid.lines.push(Line { start: shaped.start + from, carets, top, baseline, height });
    }
    laid
}

/// Draws a glyph's outline into a path, from the font's space (up from the baseline) to the
/// drawing's (down).
struct Pen {
    path: BezPath,
    x: f64,
    y: f64,
}

impl Pen {
    fn at(&self, x: f32, y: f32) -> Point {
        Point::new(self.x + x as f64, self.y - y as f64)
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.at(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.at(x, y));
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.path.quad_to(self.at(cx, cy), self.at(x, y));
    }

    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        self.path.curve_to(self.at(c0x, c0y), self.at(c1x, c1y), self.at(x, y));
    }

    fn close(&mut self) {
        self.path.close_path();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn boxed(text: &str, width: Option<f64>) -> TextBox {
        let mut text_box = TextBox::new([10.0, 20.0], width, TextFormat::default());
        text_box.text = text.to_string();
        text_box
    }

    fn laid(text: &TextBox) -> Laid {
        lay_out(text, &Fonts::built_in())
    }

    #[test]
    fn a_text_is_laid_out_as_the_outlines_of_its_letters() {
        let laid = laid(&boxed("Hi o", None));
        // "H", "i" and "o": the space has no outline.
        assert_eq!(laid.glyphs.len(), 3);
        let bounds = laid.glyphs.iter().map(|glyph| glyph.bounding_box()).reduce(|a, b| a.union(b)).unwrap();
        assert!(bounds.x0 >= 10.0 && bounds.y0 > 20.0 && bounds.y1 < 20.0 + 48.0 * 1.2, "{bounds:?}");
        assert_eq!(laid.lines.len(), 1);
        assert_eq!(laid.lines[0].carets.len(), 5);
        assert!(laid.stand_in.is_none());
    }

    #[test]
    fn a_fixed_width_wraps_at_the_spaces() {
        let text = boxed("one two three four", Some(150.0));
        let laid = laid(&text);
        assert!(laid.lines.len() >= 2, "{} lines", laid.lines.len());
        // Every line but a space at its end fits.
        for line in &laid.lines {
            let ends = line.carets.len() - 1;
            let ink = (0..ends).rev().find(|i| text.text.chars().nth(line.start + i) != Some(' ')).map_or(0, |i| i + 1);
            assert!(line.carets[ink] - 10.0 <= 150.0 + 1e-9, "{line:?}");
        }
        // And the lines go down by the leading.
        assert!((laid.lines[1].top - laid.lines[0].top - 48.0 * 1.2).abs() < 1e-9);
        // Without a width, only a line break breaks it.
        assert_eq!(super::lay_out(&boxed("one two\nthree", None), &Fonts::built_in()).lines.len(), 2);
    }

    #[test]
    fn alignment_tracking_and_kerning_move_the_letters() {
        let base = boxed("ab\nabcdef", None);
        let left = laid(&base).lines[0].carets[0];
        let mut centred = base.clone();
        centred.format.align = Align::Centre;
        let mut right = base.clone();
        right.format.align = Align::Right;
        let (centre, right_x) = (laid(&centred).lines[0].carets[0], laid(&right).lines[0].carets[0]);
        assert!(left < centre && centre < right_x, "{left} {centre} {right_x}");
        let wide = |edit: &dyn Fn(&mut TextBox)| {
            let mut text = base.clone();
            edit(&mut text);
            *laid(&text).lines[1].carets.last().unwrap()
        };
        let plain = wide(&|_| {});
        assert!((wide(&|t| t.format.tracking = 100.0) - plain - 6.0 * 4.8).abs() < 1e-6, "tracking adds after each of its six letters");
        // Kerning before the third letter of "abcdef", place 5 of the text.
        assert!((wide(&|t| _ = t.kerning.insert(5, 250.0)) - plain - 12.0).abs() < 1e-6);
    }

    #[test]
    fn justified_lines_reach_the_edge_but_the_last() {
        let mut text = boxed("aa bb cc dd ee ff gg hh", Some(200.0));
        text.format.align = Align::Justified;
        let laid = laid(&text);
        assert!(laid.lines.len() >= 2);
        let first = &laid.lines[0];
        let ink = first.carets.len() - 2;
        assert!((first.carets[ink] - 210.0).abs() < 1e-6, "{:?}", first.carets);
        let last = laid.lines.last().unwrap();
        assert!(*last.carets.last().unwrap() < 210.0 - 1.0);
    }

    #[test]
    fn splicing_keeps_kerning_with_its_letter() {
        let mut text = boxed("abcd", None);
        text.kerning = [(1, 10.0), (2, 20.0), (3, 30.0)].into();
        text.splice(2, 3, "XY");
        assert_eq!(text.text, "abXYd");
        assert_eq!(text.kerning, [(1, 10.0), (4, 30.0)].into());
        text.splice(9, 9, "!");
        assert_eq!(text.text, "abXYd!");
    }

    #[test]
    fn the_cursor_is_found_from_a_point_and_back() {
        let laid = laid(&boxed("ab\ncd", None));
        let (line, x) = laid.caret(4).unwrap();
        assert_eq!(line, 1);
        assert_eq!(laid.place_at(x + 0.1, laid.lines[1].top + 1.0), 4);
        assert_eq!(laid.place_at(-100.0, 0.0), 0);
        // An empty box still has a line to put the cursor on.
        assert_eq!(super::lay_out(&boxed("", None), &Fonts::built_in()).lines.len(), 1);
    }

    #[test]
    fn a_preview_writes_the_name_or_else_letters_the_font_has() {
        let (data, index) = Fonts::built_in().face("Inter", "Regular").unwrap().data().unwrap();
        assert_eq!(preview_text(&data, index, "Inter"), Some(("Inter".to_string(), true)));
        // Inter has no Hebrew: a sample of a script it has is shown, the first of them.
        assert_eq!(preview_text(&data, index, "אבג"), Some(("Абв Где".to_string(), false)));
    }

    #[test]
    fn the_systems_own_fonts_are_not_listed() {
        assert!(Fonts::system().families().iter().all(|family| !family.starts_with('.')));
    }

    #[test]
    fn a_missing_font_is_laid_out_in_inter_and_said() {
        let mut text = boxed("Hi", None);
        text.format.font = "Nowhere Sans".to_string();
        let laid = laid(&text);
        assert_eq!(laid.stand_in.as_deref(), Some("Nowhere Sans"));
        assert_eq!(laid.glyphs.len(), 2);
    }
}
