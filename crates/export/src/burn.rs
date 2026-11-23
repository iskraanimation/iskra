//! What is burned into a render's picture over the film (plan §7.14): the guides and the
//! aspect mask as the viewer shows them, the shot's name, and the frame's timecode or number.
//! The Export page's player shows the same over the film, from the same settings.

use iskra_model::{FrameRate, WorkingSpace};
use serde::{Deserialize, Serialize};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider};
use vello_cpu::kurbo::{BezPath, Point, Rect, Stroke};
use vello_cpu::{Pixmap, RenderContext, Resources};

/// A guide drawn over the camera's frame (plan §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Guide {
    /// The inner nine tenths of the frame: what a screen is sure to show of the action.
    ActionSafe,
    /// The inner eight tenths: where titles are safe.
    TitleSafe,
    Thirds,
    /// A cross through the middle of the frame.
    Centre,
    /// The 12-field guide of the animation stand: twelve frames, one inside the other, about
    /// the middle, with the lines through it and from corner to corner.
    Fields,
}

impl Guide {
    pub const ALL: [Guide; 5] = [Guide::ActionSafe, Guide::TitleSafe, Guide::Thirds, Guide::Centre, Guide::Fields];
}

/// What a guide of the user's own draws on the frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GuideKind {
    /// The largest frame of this aspect ratio, width over height, that fits in the middle.
    Aspect(f64),
    /// A frame this many percent in from each side.
    Margin(f64),
    /// The frame cut into this many columns and rows.
    Grid { columns: u32, rows: u32 },
}

/// A place in the frame, across and down, each from 0 to 1.
pub type Spot2 = [f32; 2];

/// The lines of a guide over the frame, as places in it (see [`Spot2`]).
pub fn guide_lines(guide: Guide) -> Vec<Vec<Spot2>> {
    let inset = |by: f32| vec![vec![[by, by], [1.0 - by, by], [1.0 - by, 1.0 - by], [by, 1.0 - by], [by, by]]];
    match guide {
        Guide::ActionSafe => inset(0.05),
        Guide::TitleSafe => inset(0.1),
        Guide::Thirds => [1.0 / 3.0, 2.0 / 3.0].into_iter().flat_map(|part| [vec![[part, 0.0], [part, 1.0]], vec![[0.0, part], [1.0, part]]]).collect(),
        Guide::Centre => vec![vec![[0.5, 0.45], [0.5, 0.55]], vec![[0.45, 0.5], [0.55, 0.5]]],
        Guide::Fields => {
            // Field 12 is the frame; field 1 is a twelfth of it, about the middle.
            let fields = (1..=12).flat_map(|field| inset(0.5 - field as f32 / 24.0));
            let through = [vec![[0.5, 0.0], [0.5, 1.0]], vec![[0.0, 0.5], [1.0, 0.5]], vec![[0.0, 0.0], [1.0, 1.0]], vec![[1.0, 0.0], [0.0, 1.0]]];
            fields.chain(through).collect()
        }
    }
}

/// How far in from the sides, across and down, each as a part of the frame, the largest frame
/// of aspect ratio `aspect` lies that fits in the middle of a frame of ratio `frame_aspect`.
pub fn aspect_inset(aspect: f64, frame_aspect: f64) -> (f32, f32) {
    if !(aspect > 0.0 && frame_aspect > 0.0) {
        return (0.0, 0.0);
    }
    if aspect >= frame_aspect { (0.0, ((1.0 - frame_aspect / aspect) / 2.0) as f32) } else { (((1.0 - aspect / frame_aspect) / 2.0) as f32, 0.0) }
}

/// The lines of a guide of the user's own over a frame whose aspect ratio is `frame_aspect`,
/// as places in it.
pub fn own_guide_lines(kind: GuideKind, frame_aspect: f64) -> Vec<Vec<Spot2>> {
    let inset = |x: f32, y: f32| vec![vec![[x, y], [1.0 - x, y], [1.0 - x, 1.0 - y], [x, 1.0 - y], [x, y]]];
    match kind {
        GuideKind::Aspect(aspect) => {
            let (x, y) = aspect_inset(aspect, frame_aspect);
            inset(x, y)
        }
        GuideKind::Margin(percent) => {
            let by = (percent / 100.0).clamp(0.0, 0.5) as f32;
            inset(by, by)
        }
        GuideKind::Grid { columns, rows } => {
            let part = |step: u32, of: u32| step as f32 / of as f32;
            let down = (1..columns.clamp(1, 64)).map(|column| vec![[part(column, columns), 0.0], [part(column, columns), 1.0]]);
            let across = (1..rows.clamp(1, 64)).map(|row| vec![[0.0, part(row, rows)], [1.0, part(row, rows)]]);
            down.chain(across).collect()
        }
    }
}

/// The parts of a frame that an aspect mask darkens, each from one corner to the other as
/// places in it: the two bands outside the largest frame of ratio `aspect` that fits in the
/// middle. None if that is the frame itself.
pub fn mask_bands(aspect: f64, frame_aspect: f64) -> Vec<[Spot2; 2]> {
    match aspect_inset(aspect, frame_aspect) {
        (x, y) if x <= 0.0 && y <= 0.0 => Vec::new(),
        (x, _) if x > 0.0 => vec![[[0.0, 0.0], [x, 1.0]], [[1.0 - x, 0.0], [1.0, 1.0]]],
        (_, y) => vec![[[0.0, 0.0], [1.0, y]], [[0.0, 1.0 - y], [1.0, 1.0]]],
    }
}

/// How dark the aspect mask makes what it covers, of 255.
pub const MASK_ALPHA: u8 = 150;
/// How strong the lines of the guides are, of 255: the app's guides, and the user's own.
pub const GUIDE_ALPHA: u8 = 190;
pub const OWN_GUIDE_ALPHA: u8 = 220;
/// The colour of the guides that have none of their own.
pub const GUIDE_COLOUR: [u8; 3] = [40, 170, 220];
/// How many of the user's own guides are burned in, at most.
pub const OWN_GUIDES: usize = 8;

/// One of nine places in the frame a text is put at: a corner, the middle of a side, or the
/// middle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Spot {
    #[default]
    TopLeft,
    Top,
    TopRight,
    Left,
    Middle,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Spot {
    /// In rows, from the top left.
    pub const ALL: [Spot; 9] =
        [Spot::TopLeft, Spot::Top, Spot::TopRight, Spot::Left, Spot::Middle, Spot::Right, Spot::BottomLeft, Spot::Bottom, Spot::BottomRight];

    /// How far along, across and down, from 0 to 1.
    pub fn along(self) -> [f32; 2] {
        let at = Spot::ALL.iter().position(|spot| *spot == self).unwrap_or(0);
        [(at % 3) as f32 / 2.0, (at / 3) as f32 / 2.0]
    }

    pub fn name(self) -> &'static str {
        match self {
            Spot::TopLeft => "Top Left",
            Spot::Top => "Top",
            Spot::TopRight => "Top Right",
            Spot::Left => "Left",
            Spot::Middle => "Middle",
            Spot::Right => "Right",
            Spot::BottomLeft => "Bottom Left",
            Spot::Bottom => "Bottom",
            Spot::BottomRight => "Bottom Right",
        }
    }
}

/// How the frame is counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Counting {
    /// Hours, minutes, seconds and frames.
    #[default]
    Timecode,
    /// The frame's number, from 1.
    Frames,
}

/// A guide of the user's own, as it is burned in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnGuide {
    pub kind: GuideKind,
    pub colour: [u8; 3],
}

/// What is burned into the picture, and how (plan §7.14).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BurnIn {
    /// The guides the viewer shows, in `shown` and `own`.
    pub guides: bool,
    /// The viewer's aspect mask, of the ratio in `aspect`.
    pub mask: bool,
    /// The name of the shot on the frame.
    pub name: bool,
    /// The frame, counted as `counting` says: of the film where the storyboard is rendered as
    /// one movie, else of the shot.
    pub counter: bool,
    pub name_at: Spot,
    pub counter_at: Spot,
    pub counting: Counting,
    /// The texts on a box, or bare with an outline round them.
    pub boxed: bool,
    /// How much the box hides of what is under it, from 0 to 1; bare, how much the text does.
    pub opacity: f32,
    /// How tall the text is, as a part of the frame's height.
    pub size: f32,
    /// What the viewer shows, taken from it as the render is asked for: which of
    /// [`Guide::ALL`], the user's own guides that are shown, and the mask's ratio.
    pub shown: [bool; 5],
    pub own: [Option<OwnGuide>; OWN_GUIDES],
    pub aspect: Option<f64>,
    /// The colour of the guides that have none of their own, and the mask's with how dark it
    /// is, as the viewer shows them: the user can set both.
    pub guide_colour: [u8; 3],
    pub mask_colour: [u8; 4],
}

impl Default for BurnIn {
    fn default() -> Self {
        Self {
            guides: false,
            mask: false,
            name: false,
            counter: false,
            name_at: Spot::TopLeft,
            counter_at: Spot::BottomRight,
            counting: Counting::Timecode,
            boxed: true,
            opacity: 0.6,
            size: 0.04,
            shown: [false; 5],
            own: [None; OWN_GUIDES],
            aspect: None,
            guide_colour: GUIDE_COLOUR,
            mask_colour: [0, 0, 0, MASK_ALPHA],
        }
    }
}

/// The smallest and largest a text is, as a part of the frame's height.
pub const SIZES: std::ops::RangeInclusive<f32> = 0.015..=0.15;

/// The typeface the texts are in: the interface's monospaced one, whose figures are all as
/// wide, so a counter does not shift as it counts.
const FONT: &[u8] = include_bytes!("../../ui/assets/fonts/JetBrainsMono-Regular.ttf");

impl BurnIn {
    /// Whether anything is burned in at all.
    pub fn any(&self) -> bool {
        self.guides || self.mask || self.name || self.counter
    }

    /// As a render can take it: the size and the opacity within what they can be.
    pub fn fitted(mut self) -> Self {
        self.size = if self.size.is_finite() { self.size.clamp(*SIZES.start(), *SIZES.end()) } else { BurnIn::default().size };
        self.opacity = if self.opacity.is_finite() { self.opacity.clamp(0.0, 1.0) } else { BurnIn::default().opacity };
        self
    }

    /// The frame `frame`, from 0, as the counter says it, at `rate`.
    pub fn count(&self, frame: i64, rate: FrameRate) -> String {
        match self.counting {
            Counting::Timecode => iskra_model::timecode(frame, rate),
            Counting::Frames => (frame + 1).to_string(),
        }
    }

    /// The lines of the guides that are burned in, each as places in the frame with its
    /// colour and strength.
    pub fn guide_lines(&self, frame_aspect: f64) -> Vec<(Vec<Spot2>, [u8; 4])> {
        if !self.guides {
            return Vec::new();
        }
        let [r, g, b] = self.guide_colour;
        let standard = Guide::ALL.into_iter().zip(self.shown).filter(|(_, shown)| *shown).flat_map(|(guide, _)| guide_lines(guide));
        let standard = standard.map(|line| (line, [r, g, b, GUIDE_ALPHA]));
        let own = self.own.iter().flatten().flat_map(|own| {
            let [r, g, b] = own.colour;
            own_guide_lines(own.kind, frame_aspect).into_iter().map(move |line| (line, [r, g, b, OWN_GUIDE_ALPHA]))
        });
        standard.chain(own).collect()
    }

    /// The bands the mask darkens, if it is burned in.
    pub fn mask_bands(&self, frame_aspect: f64) -> Vec<[Spot2; 2]> {
        match self.aspect.filter(|_| self.mask) {
            Some(aspect) => mask_bands(aspect, frame_aspect),
            None => Vec::new(),
        }
    }

    /// Where a text of `width` by `height` lies in a frame of `frame_width` by `frame_height`
    /// at `spot`, its top left corner: a margin of half the text's size in from the sides.
    pub fn place(&self, spot: Spot, width: f32, height: f32, frame_width: f32, frame_height: f32) -> [f32; 2] {
        let margin = self.size * frame_height / 2.0;
        let [across, down] = spot.along();
        let x = margin + (frame_width - 2.0 * margin - width) * across;
        let y = margin + (frame_height - 2.0 * margin - height) * down;
        [x, y]
    }

    /// The texts on the frame: the shot's name, where there is a shot, and the counter.
    pub fn texts(&self, name: Option<&str>, counter: &str) -> Vec<(String, Spot)> {
        let name = name.filter(|_| self.name).map(|name| (name.to_string(), self.name_at));
        let counter = self.counter.then(|| (counter.to_string(), self.counter_at));
        name.into_iter().chain(counter).collect()
    }
}

/// How much room a text's box gives round it, as a part of the text's size.
pub const PADDING: f32 = 0.3;

/// A text as a path, in pixels from the left of its first letter and down from the top of its
/// line, at `px` pixels tall; and how wide and tall its line is.
fn text_path(text: &str, px: f32) -> (BezPath, f32, f32) {
    text_path_in(FONT, text, px)
}

/// [`text_path`] in the font `font`, the bytes of its file.
pub(crate) fn text_path_in(font: &[u8], text: &str, px: f32) -> (BezPath, f32, f32) {
    let Ok(font) = FontRef::new(font) else { return (BezPath::new(), 0.0, 0.0) };
    let size = Size::new(px);
    let (charmap, outlines) = (font.charmap(), font.outline_glyphs());
    let metrics = font.metrics(size, LocationRef::default());
    let advances = font.glyph_metrics(size, LocationRef::default());
    let mut pen = Pen { path: BezPath::new(), x: 0.0, baseline: metrics.ascent };
    for c in text.chars() {
        let glyph = charmap.map(c).or_else(|| charmap.map('?')).unwrap_or_default();
        if let Some(outline) = outlines.get(glyph) {
            let _ = outline.draw(DrawSettings::unhinted(size, LocationRef::default()), &mut pen);
        }
        pen.x += advances.advance_width(glyph).unwrap_or(px * 0.6);
    }
    (pen.path, pen.x, metrics.ascent - metrics.descent)
}

/// Takes a glyph's outline, whose y goes up from the baseline, into a path whose y goes down.
struct Pen {
    path: BezPath,
    x: f32,
    baseline: f32,
}

impl Pen {
    fn at(&self, x: f32, y: f32) -> Point {
        Point::new(f64::from(self.x + x), f64::from(self.baseline - y))
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.at(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.at(x, y));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.path.quad_to(self.at(cx0, cy0), self.at(x, y));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.path.curve_to(self.at(cx0, cy0), self.at(cx1, cy1), self.at(x, y));
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}

fn colour([r, g, b]: [u8; 3], alpha: f32) -> vello_cpu::color::AlphaColor<vello_cpu::color::Srgb> {
    vello_cpu::color::AlphaColor::from_rgba8(r, g, b, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Burn `burn` into `picture`, which is `width` by `height`, colour times alpha in `space`:
/// over the frame `[left, top, frame_width, frame_height]` in it, with `name` as the shot's
/// name and `counter` as the frame's count.
#[allow(clippy::too_many_arguments)]
pub fn burn(picture: &mut [[f32; 4]], width: u32, height: u32, frame: [u32; 4], space: WorkingSpace, burn: &BurnIn, name: Option<&str>, counter: &str) {
    let burn = burn.fitted();
    let (Ok(wide), Ok(tall)) = (u16::try_from(width), u16::try_from(height)) else { return };
    if !burn.any() || wide == 0 || tall == 0 || picture.len() != width as usize * height as usize {
        return;
    }
    let [left, top, frame_width, frame_height] = frame.map(|value| value as f32);
    let frame_aspect = f64::from(frame_width) / f64::from(frame_height.max(1.0));
    let at = |[across, down]: Spot2| Point::new(f64::from(left + across * frame_width), f64::from(top + down * frame_height));
    let mut context = RenderContext::new(wide, tall);
    for [from, to] in burn.mask_bands(frame_aspect) {
        let [r, g, b, a] = burn.mask_colour;
        context.set_paint(colour([r, g, b], f32::from(a) / 255.0));
        context.fill_rect(&Rect::from_points(at(from), at(to)));
    }
    // As the viewer draws them, a point of the screen wide: a pixel of a picture as tall as HD.
    context.set_stroke(Stroke::new(f64::from((frame_height / 1080.0).max(1.0))));
    for (line, [r, g, b, a]) in burn.guide_lines(frame_aspect) {
        let mut path = BezPath::new();
        for (index, spot) in line.into_iter().enumerate() {
            if index == 0 { path.move_to(at(spot)) } else { path.line_to(at(spot)) }
        }
        context.set_paint(colour([r, g, b], f32::from(a) / 255.0));
        context.stroke_path(&path);
    }
    let px = burn.size * frame_height;
    for (text, spot) in burn.texts(name, counter) {
        let (path, text_width, text_height) = text_path(&text, px);
        let pad = PADDING * px;
        let [x, y] = burn.place(spot, text_width + 2.0 * pad, text_height + 2.0 * pad, frame_width, frame_height);
        let (x, y) = (left + x, top + y);
        let placed = vello_cpu::kurbo::Affine::translate((f64::from(x + pad), f64::from(y + pad))) * path;
        if burn.boxed {
            context.set_paint(colour([0, 0, 0], burn.opacity));
            context.fill_rect(&Rect::new(f64::from(x), f64::from(y), f64::from(x + text_width + 2.0 * pad), f64::from(y + text_height + 2.0 * pad)));
            context.set_paint(colour([255, 255, 255], 1.0));
            context.fill_path(&placed);
        } else {
            context.set_stroke(Stroke::new(f64::from(px / 8.0)));
            context.set_paint(colour([0, 0, 0], burn.opacity));
            context.stroke_path(&placed);
            context.set_paint(colour([255, 255, 255], burn.opacity));
            context.fill_path(&placed);
        }
    }
    context.flush();
    let mut pixmap = Pixmap::new(wide, tall);
    context.render(&mut pixmap, &mut Resources::new());
    // Laid over the picture, in its space.
    let into = |value: u8, alpha: f32| {
        let straight = f32::from(value) / 255.0 / alpha;
        let straight = if space == WorkingSpace::Linear { crate::to_linear(straight.min(1.0)) } else { straight };
        straight * alpha
    };
    for (pixel, over) in picture.iter_mut().zip(pixmap.data()) {
        if over.a == 0 {
            continue;
        }
        let alpha = f32::from(over.a) / 255.0;
        let [r, g, b, a] = *pixel;
        let keep = 1.0 - alpha;
        *pixel = [into(over.r, alpha) + r * keep, into(over.g, alpha) + g * keep, into(over.b, alpha) + b * keep, alpha + a * keep];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(width: u32, height: u32) -> Vec<[f32; 4]> {
        vec![[0.0, 0.0, 0.0, 0.0]; (width * height) as usize]
    }

    #[test]
    fn nothing_switched_on_burns_nothing() {
        let mut picture = blank(64, 36);
        burn(&mut picture, 64, 36, [0, 0, 64, 36], WorkingSpace::Srgb, &BurnIn::default(), Some("Shot_01"), "00:00:00:00");
        assert!(picture.iter().all(|pixel| *pixel == [0.0; 4]));
    }

    #[test]
    fn a_text_lies_on_its_box_at_its_spot() {
        let (width, height) = (320, 180);
        let on = BurnIn { name: true, counter: true, boxed: true, opacity: 1.0, size: 0.1, ..BurnIn::default() };
        let mut picture = blank(width, height);
        burn(&mut picture, width, height, [0, 0, width, height], WorkingSpace::Srgb, &on, Some("Shot_01"), "00:00:01:02");
        let at = |x: u32, y: u32| picture[(y * width + x) as usize];
        // The name in the top left, the counter in the bottom right: dark boxes, light letters.
        let region = |x0: u32, x1: u32, y0: u32, y1: u32| (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| at(x, y)).collect::<Vec<_>>();
        let (top_left, bottom_right) = (region(0, 160, 0, 40), region(160, 320, 140, 180));
        for corner in [&top_left, &bottom_right] {
            assert!(corner.iter().any(|pixel| pixel[3] == 1.0 && pixel[0] < 0.05), "a box");
            assert!(corner.iter().any(|pixel| pixel[0] > 0.9), "letters on it");
        }
        // Nothing elsewhere: the middle and the other corners.
        assert!(region(0, 100, 140, 180).iter().chain(&region(220, 320, 0, 40)).chain(&region(120, 200, 70, 110)).all(|pixel| pixel[3] == 0.0));
    }

    #[test]
    fn the_mask_darkens_outside_the_ratio_and_the_guides_are_lines() {
        let (width, height) = (200, 100);
        // A frame of 2:1 masked to 4:1: bands of a quarter of its height above and below.
        let on = BurnIn { mask: true, aspect: Some(4.0), ..BurnIn::default() };
        let mut picture = blank(width, height);
        burn(&mut picture, width, height, [0, 0, width, height], WorkingSpace::Srgb, &on, None, "");
        let alpha = |x: u32, y: u32| picture[(y * width + x) as usize][3];
        assert!((alpha(100, 10) - f32::from(MASK_ALPHA) / 255.0).abs() < 0.01 && alpha(100, 50) == 0.0 && alpha(100, 90) > 0.5);
        // The thirds: a line down a third of the way across, nothing beside it.
        let on = BurnIn { guides: true, shown: [false, false, true, false, false], ..BurnIn::default() };
        let mut picture = blank(width, height);
        burn(&mut picture, width, height, [0, 0, width, height], WorkingSpace::Srgb, &on, None, "");
        let alpha = |x: u32, y: u32| picture[(y * width + x) as usize][3];
        assert!(alpha(66, 50) > 0.3 && alpha(50, 50) == 0.0 && alpha(100, 33) > 0.3);
    }

    #[test]
    fn the_counter_says_timecode_or_the_frame_from_one() {
        let rate = FrameRate { num: 24, den: 1 };
        let on = BurnIn::default();
        assert_eq!(on.count(26, rate), "00:00:01:02");
        assert_eq!(BurnIn { counting: Counting::Frames, ..on }.count(26, rate), "27");
    }
}
