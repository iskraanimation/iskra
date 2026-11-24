//! Swatches that are more than one colour (plan §4.20): a gradient or a picture, painted
//! where a drawing's pixels name the swatch. The pixels are as for any swatch, an id and a
//! coverage; what is new is where the gradient or the picture lies. That is kept on the
//! drawing, one [`Placement`] for each filled area: a run of one swatch's pixels that touch,
//! in one art layer. A placement holds a point inside its area, so an area takes the
//! placement whose point lies in it, and painting needs to keep no ids of areas.

use serde::{Deserialize, Serialize};

use crate::{ArtContent, ArtLayerKind, Drawing, IndexedSurface, Rect, SwatchId, over_straight};

/// A gradient's way of spreading its colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GradientShape {
    /// Along the line from its start to its end, the same across it.
    #[default]
    Linear,
    /// Out from its start, its end on the circle of the last colour; its colours from its
    /// focal point, which may be off its middle.
    Radial,
}

/// A colour of a gradient and its place on it, from 0 at the start to 1 at the end.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub at: f32,
    /// sRGB with straight alpha.
    pub color: [u8; 4],
}

/// A gradient's colours: two or more, by their places.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gradient {
    #[serde(default)]
    pub shape: GradientShape,
    /// In the order of their places.
    pub stops: Vec<GradientStop>,
}

// A stop's place is always finite.
impl Eq for Gradient {}

impl Gradient {
    /// Its colours put in the order of their places, each place from 0 to 1.
    pub fn sorted(mut self) -> Gradient {
        for stop in &mut self.stops {
            stop.at = if stop.at.is_finite() { stop.at.clamp(0.0, 1.0) } else { 0.0 };
        }
        self.stops.sort_by(|a, b| a.at.total_cmp(&b.at));
        self
    }

    /// The colour at `t` along it (0 to 1, held at the ends), blended between the stops
    /// around it in linear light when `linear`, else as sRGB numbers.
    pub fn color_at(&self, t: f32, linear: bool) -> [u8; 4] {
        let Some(first) = self.stops.first() else { return [0; 4] };
        let t = if t.is_finite() { t } else { 0.0 };
        let after = self.stops.iter().position(|stop| stop.at > t);
        let (a, b) = match after {
            None => return self.stops[self.stops.len() - 1].color,
            Some(0) => return first.color,
            Some(at) => (self.stops[at - 1], self.stops[at]),
        };
        let k = (t - a.at) / (b.at - a.at).max(f32::EPSILON);
        mix(a.color, b.color, k, linear)
    }
}

/// `a` and `b` mixed, `k` of the way to `b`; with their alpha premultiplied, so a clear end
/// fades the colour out and does not darken it.
fn mix(a: [u8; 4], b: [u8; 4], k: f32, linear: bool) -> [u8; 4] {
    let into = |v: u8| if linear { to_linear(v) } else { f32::from(v) / 255.0 };
    let (aa, ba) = (f32::from(a[3]) / 255.0, f32::from(b[3]) / 255.0);
    let alpha = aa + (ba - aa) * k;
    if alpha <= 0.0 {
        return [0; 4];
    }
    let channel = |c: usize| {
        let v = (into(a[c]) * aa + (into(b[c]) * ba - into(a[c]) * aa) * k) / alpha;
        if linear { from_linear(v) } else { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 }
    };
    [channel(0), channel(1), channel(2), (alpha * 255.0 + 0.5) as u8]
}

fn to_linear(v: u8) -> f32 {
    let v = f32::from(v) / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// A texture swatch's picture: its pixels as straight sRGB, the top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 4]>,
}

impl Picture {
    /// A picture of `width` × `height` pixels from straight RGBA bytes.
    pub fn of_rgba(width: u32, height: u32, rgba: &[u8]) -> Picture {
        Picture { width, height, pixels: rgba.as_chunks::<4>().0.to_vec() }
    }

    /// Its colour at (`u`, `v`), in its pixels, blended between the four around it; repeated
    /// past its edges when `tile`, else its edge drawn out.
    pub fn sample(&self, u: f32, v: f32, tile: bool) -> [u8; 4] {
        if self.width == 0 || self.height == 0 {
            return [0; 4];
        }
        let (w, h) = (self.width as i64, self.height as i64);
        let at = |x: i64, y: i64| {
            let (x, y) = if tile { (x.rem_euclid(w), y.rem_euclid(h)) } else { (x.clamp(0, w - 1), y.clamp(0, h - 1)) };
            self.pixels[(y * w + x) as usize]
        };
        let (u, v) = (u - 0.5, v - 0.5);
        let (x, y) = (u.floor(), v.floor());
        let (fx, fy) = (u - x, v - y);
        let (x, y) = (x as i64, y as i64);
        let top = mix(at(x, y), at(x + 1, y), fx, false);
        let bottom = mix(at(x, y + 1), at(x + 1, y + 1), fx, false);
        mix(top, bottom, fy, false)
    }

    /// Its colour as a whole: the mean of its pixels, by their alpha.
    pub fn average(&self) -> [u8; 4] {
        let mut sum = [0f64; 4];
        for pixel in &self.pixels {
            let a = f64::from(pixel[3]);
            (0..3).for_each(|c| sum[c] += f64::from(pixel[c]) * a);
            sum[3] += a;
        }
        if sum[3] <= 0.0 {
            return [0; 4];
        }
        let n = self.pixels.len() as f64;
        [(sum[0] / sum[3]) as u8, (sum[1] / sum[3]) as u8, (sum[2] / sum[3]) as u8, (sum[3] / n + 0.5) as u8]
    }
}

/// How a swatch is painted, as a palette gives it to a drawing.
#[derive(Clone, Copy, Debug)]
pub enum Shade<'a> {
    /// One colour, straight sRGB.
    Solid([u8; 4]),
    Gradient(&'a Gradient),
    /// A picture, repeated when `tile`, one of its pixels `scale` of a drawing's where it is
    /// fitted to an area. Shown `once`, the area past it is `color`, else its edges are drawn
    /// out. Without its picture (not read, or its file gone) it is `color`.
    Texture {
        picture: Option<&'a Picture>,
        tile: bool,
        once: bool,
        scale: f32,
        color: [u8; 4],
    },
}

impl Shade<'_> {
    /// Painted with one colour: the swatch's chip, or what a reader of solid swatches shows.
    pub fn is_solid(&self) -> bool {
        matches!(self, Shade::Solid(_) | Shade::Texture { picture: None, .. })
    }

    /// The one colour it has where it is solid.
    /// A picture's is the swatch's own, its mean as it was read: this is asked for each pixel
    /// painted, and the mean of a large picture is long to find.
    fn color(&self) -> [u8; 4] {
        match self {
            Shade::Solid(color) | Shade::Texture { color, .. } => *color,
            Shade::Gradient(gradient) => gradient.stops.first().map_or([0; 4], |stop| stop.color),
        }
    }

    /// Its colour at the pixel (`x`, `y`) of an area of `bounds`, fitted to the area: what
    /// free colour is painted with where it is a gradient or a picture.
    pub fn in_area(&self, x: i32, y: i32, bounds: Rect, linear: bool) -> [u8; 4] {
        self.at([x as f32 + 0.5, y as f32 + 0.5], &self.fitted(bounds), linear)
    }

    /// Where it lies on an area of `bounds` that has no placement of its own: a gradient
    /// across it from left to right, or out from its middle to its widest; a picture from
    /// its top left corner.
    pub fn fitted(&self, bounds: Rect) -> Frame {
        let (x0, y0, x1, y1) = (bounds.x0 as f32, bounds.y0 as f32, bounds.x1 as f32, bounds.y1 as f32);
        let middle = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
        let (start, end) = match self {
            Shade::Gradient(Gradient { shape: GradientShape::Radial, .. }) => {
                let reach = (x1 - x0).max(y1 - y0) / 2.0;
                (middle, [middle[0] + reach.max(1.0), middle[1]])
            }
            Shade::Texture { picture: Some(picture), scale, .. } => ([x0, y0], [x0 + picture.width as f32 * scale.max(f32::EPSILON), y0]),
            _ => ([x0, middle[1]], [x1.max(x0 + 1.0), middle[1]]),
        };
        self.square(start, end)
    }

    /// Lying from `start` to `end`, not skewed: its other side square to that, as long, or
    /// a picture's as long as its height is to its width.
    pub fn square(&self, start: [f32; 2], end: [f32; 2]) -> Frame {
        let high = match self {
            Shade::Texture { picture: Some(picture), .. } if picture.width > 0 => picture.height as f32 / picture.width as f32,
            _ => 1.0,
        };
        let (dx, dy) = (end[0] - start[0], end[1] - start[1]);
        Frame { start, end, across: [start[0] - dy * high, start[1] + dx * high], focus: [0.0; 2] }
    }

    /// Its colour at the drawing's point `p`, where it lies as `frame` says.
    fn at(&self, p: [f32; 2], frame: &Frame, linear: bool) -> [u8; 4] {
        let [u, v] = frame.local(p);
        match self {
            Shade::Gradient(gradient) => {
                let t = match gradient.shape {
                    GradientShape::Linear => u,
                    GradientShape::Radial => focal([u, v], frame.focus),
                };
                gradient.color_at(t, linear)
            }
            // The picture's top edge runs from the start to the end, its left from the start
            // to `across`.
            Shade::Texture { picture: Some(picture), tile, once, color, .. } => {
                if *once && !*tile && !((0.0..1.0).contains(&u) && (0.0..1.0).contains(&v)) {
                    return *color;
                }
                picture.sample(u * picture.width as f32, v * picture.height as f32, *tile)
            }
            _ => self.color(),
        }
    }
}

/// How far out a radial gradient is at `p`, in its own measure (its middle at the origin,
/// its ring at 1): from 0 at its `focus` to 1 on the ring, along the line out from the focus.
fn focal(p: [f32; 2], focus: [f32; 2]) -> f32 {
    let f = Frame::inside_ring(focus);
    let d = [p[0] - f[0], p[1] - f[1]];
    let reach = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if reach < f32::EPSILON {
        return 0.0;
    }
    // Where the line from the focus through `p` meets the ring, how far from the focus.
    let way = [d[0] / reach, d[1] / reach];
    let b = f[0] * way[0] + f[1] * way[1];
    let ring = -b + (b * b - (f[0] * f[0] + f[1] * f[1]) + 1.0).max(0.0).sqrt();
    reach / ring.max(f32::EPSILON)
}

/// Where a gradient or a picture lies, in the drawing's pixels: its sides from `start` to
/// `end` and from `start` to `across`. A gradient runs from its first colour at the start to
/// its last at the end, the same along `across`; a radial one is a ring through the end and
/// `across` about the start. A picture's top edge runs from the start to the end, its left
/// edge from the start to `across`. `focus` is a radial gradient's focal point in its own
/// measure: (0, 0) at its middle, 1 on its ring.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub start: [f32; 2],
    pub end: [f32; 2],
    pub across: [f32; 2],
    pub focus: [f32; 2],
}

impl Frame {
    /// How far from a ring's middle its focal point can be, of the way to the ring.
    const FOCUS_REACH: f32 = 0.95;

    /// `focus` kept inside the ring.
    pub fn inside_ring(focus: [f32; 2]) -> [f32; 2] {
        let reach = (focus[0] * focus[0] + focus[1] * focus[1]).sqrt();
        if reach > Self::FOCUS_REACH { [focus[0] / reach * Self::FOCUS_REACH, focus[1] / reach * Self::FOCUS_REACH] } else { focus }
    }

    /// The drawing's point `p` in its own measure: the start at (0, 0), the end at (1, 0),
    /// `across` at (0, 1).
    pub fn local(&self, p: [f32; 2]) -> [f32; 2] {
        let (x, y) = ([self.end[0] - self.start[0], self.end[1] - self.start[1]], [self.across[0] - self.start[0], self.across[1] - self.start[1]]);
        let det = x[0] * y[1] - x[1] * y[0];
        if det.abs() < f32::EPSILON {
            return [0.0; 2];
        }
        let (px, py) = (p[0] - self.start[0], p[1] - self.start[1]);
        [(px * y[1] - py * y[0]) / det, (py * x[0] - px * x[1]) / det]
    }
}

/// Where a gradient or a picture lies on one filled area of a drawing, as a [`Frame`] has
/// it, in the drawing's pixels. Without `across` it is not skewed: its other side is square
/// to the first, as [`Shade::square`] makes it. `at` is a pixel inside the area: the area it
/// is of is the one `at` lies in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub art: ArtLayerKind,
    pub swatch: SwatchId,
    pub at: [i32; 2],
    pub start: [f32; 2],
    pub end: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub across: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "is_origin")]
    pub focus: [f32; 2],
}

fn is_origin(at: &[f32; 2]) -> bool {
    *at == [0.0; 2]
}

// Its points are always finite.
impl Eq for Placement {}

/// The filled areas of a drawing's swatches that are not solid, each with where its gradient
/// or picture lies: what [`Drawing::to_rgba8_shaded`] paints by. Found anew for each look at a
/// drawing, by [`Drawing::areas`].
#[derive(Clone, Debug, Default)]
pub struct Areas {
    layers: Vec<LayerAreas>,
}

#[derive(Clone, Debug)]
struct LayerAreas {
    art: ArtLayerKind,
    /// What is labelled: the bounds of the art layer's pixels of those swatches.
    bounds: Rect,
    /// For each pixel of `bounds`, in rows, the area it is in, counted from 1; 0 for none.
    labels: Vec<u32>,
    /// Each area's swatch and where it lies, by its label less one.
    areas: Vec<Area>,
}

/// Pixels side by side in a row, of one swatch: what areas are found from.
#[derive(Clone, Copy)]
struct Run {
    y: i32,
    x0: i32,
    x1: i32,
    swatch: SwatchId,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub swatch: SwatchId,
    pub bounds: Rect,
    /// A pixel of it: the first found, of its top row.
    pub inside: [i32; 2],
    pub frame: Frame,
    /// Whether the drawing holds a placement for it, or it is fitted.
    pub placed: bool,
}

impl Areas {
    /// Nothing to paint other than solid colour.
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// The area of art layer `art` that the pixel (`x`, `y`) is in, where it is of a swatch
    /// that is not solid.
    pub fn at(&self, art: ArtLayerKind, x: i32, y: i32) -> Option<&Area> {
        let layer = self.layers.iter().find(|layer| layer.art == art)?;
        let label = layer.label(x, y)?;
        layer.areas.get(label as usize - 1)
    }

    /// Every area, with its art layer.
    pub fn all(&self) -> impl Iterator<Item = (ArtLayerKind, &Area)> {
        self.layers.iter().flat_map(|layer| layer.areas.iter().map(move |area| (layer.art, area)))
    }
}

impl LayerAreas {
    fn label(&self, x: i32, y: i32) -> Option<u32> {
        let b = self.bounds;
        if x < b.x0 || y < b.y0 || x >= b.x1 || y >= b.y1 {
            return None;
        }
        let label = self.labels[(y - b.y0) as usize * b.width() as usize + (x - b.x0) as usize];
        (label > 0).then_some(label)
    }

    /// The areas of `surface`'s pixels whose swatches `shade` gives as not solid.
    fn of<'a>(art: ArtLayerKind, surface: &IndexedSurface, shade: &(dyn Fn(SwatchId) -> Shade<'a> + Sync), placements: &[Placement]) -> Option<LayerAreas> {
        use rayon::prelude::*;
        let bounds = surface.bounds();
        if bounds.is_empty() {
            return None;
        }
        let width = bounds.width() as usize;
        // Each row's runs of pixels of one swatch that is not solid, read a row at a time and
        // the rows at once, the shade of the swatch last met in the row kept.
        let rows: Vec<Vec<Run>> = (bounds.y0..bounds.y1)
            .into_par_iter()
            .map(|y| {
                let mut runs: Vec<Run> = Vec::new();
                let mut last: Option<(SwatchId, bool)> = None;
                for (i, pixel) in surface.pixels(Rect::new(bounds.x0, y, bounds.x1, y + 1)).into_iter().enumerate() {
                    if pixel.coverage == 0 {
                        continue;
                    }
                    let shaded = match last {
                        Some((id, shaded)) if id == pixel.swatch => shaded,
                        _ => {
                            let shaded = !shade(pixel.swatch).is_solid();
                            last = Some((pixel.swatch, shaded));
                            shaded
                        }
                    };
                    if !shaded {
                        continue;
                    }
                    let x = bounds.x0 + i as i32;
                    match runs.last_mut() {
                        Some(run) if run.x1 == x && run.swatch == pixel.swatch => run.x1 += 1,
                        _ => runs.push(Run { y, x0: x, x1: x + 1, swatch: pixel.swatch }),
                    }
                }
                runs
            })
            .collect();
        let runs: Vec<Run> = rows.iter().flatten().copied().collect();
        if runs.is_empty() {
            return None;
        }
        // Runs of one swatch that touch, side by side in rows next to each other, are of one
        // area; each area's root is its first run, so its first pixel is its root's first.
        let mut parent: Vec<usize> = (0..runs.len()).collect();
        fn root(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let (mut above, mut start) = (0..0, 0);
        for row in &rows {
            let here = start..start + row.len();
            let mut k = above.start;
            for i in here.clone() {
                let run = runs[i];
                // The runs above that end before this one starts touch neither it nor any after.
                while k < above.end && runs[k].x1 <= run.x0 {
                    k += 1;
                }
                let mut j = k;
                while j < above.end && runs[j].x0 < run.x1 {
                    if runs[j].swatch == run.swatch {
                        let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                        parent[a.max(b)] = a.min(b);
                    }
                    j += 1;
                }
            }
            (above, start) = (here.clone(), here.end);
        }
        let mut labels = vec![0u32; width * bounds.height() as usize];
        let mut areas: Vec<Area> = Vec::new();
        let mut label_of = vec![0u32; runs.len()];
        for (i, run) in runs.iter().enumerate() {
            let first = root(&mut parent, i);
            if first == i {
                areas.push(Area {
                    swatch: run.swatch,
                    bounds: Rect::new(run.x0, run.y, run.x1, run.y + 1),
                    inside: [run.x0, run.y],
                    frame: Frame::default(),
                    placed: false,
                });
                label_of[i] = areas.len() as u32;
            } else {
                label_of[i] = label_of[first];
                let area = &mut areas[label_of[i] as usize - 1];
                area.bounds = area.bounds.union(&Rect::new(run.x0, run.y, run.x1, run.y + 1));
            }
            let at = (run.y - bounds.y0) as usize * width;
            labels[at + (run.x0 - bounds.x0) as usize..at + (run.x1 - bounds.x0) as usize].fill(label_of[i]);
        }
        let mut layer = LayerAreas { art, bounds, labels, areas };
        // Each takes the first placement of its art layer and swatch whose point lies in it;
        // of two areas painted into one, the first placement wins.
        for placement in placements.iter().filter(|placement| placement.art == art) {
            let Some(label) = layer.label(placement.at[0], placement.at[1]) else { continue };
            let area = &mut layer.areas[label as usize - 1];
            if area.swatch == placement.swatch && !area.placed {
                let square = shade(area.swatch).square(placement.start, placement.end);
                let across = placement.across.unwrap_or(square.across);
                (area.frame, area.placed) = (Frame { across, focus: placement.focus, ..square }, true);
            }
        }
        for area in layer.areas.iter_mut().filter(|area| !area.placed) {
            area.frame = shade(area.swatch).fitted(area.bounds);
        }
        Some(layer)
    }
}

/// The pixels of `rect` of `surface`, art layer `art` of a drawing whose `areas` they are, as
/// straight-alpha RGBA8 in rows: its gradients and pictures where `areas` has them, and its
/// other swatches in their one colour.
pub fn shaded<'a>(surface: &IndexedSurface, art: ArtLayerKind, rect: Rect, areas: &Areas, shade: impl Fn(SwatchId) -> Shade<'a>, linear: bool) -> Vec<[u8; 4]> {
    let width = rect.width() as usize;
    let mut pixels = vec![[0u8; 4]; width * rect.height() as usize];
    if width == 0 {
        return pixels;
    }
    let mut shades = Shades::new(&shade);
    for (row, out) in pixels.chunks_exact_mut(width).enumerate() {
        shaded_row(surface, art, rect.x0, rect.y0 + row as i32, areas, &mut shades, linear, out);
    }
    pixels
}

/// The shades of swatches as they are asked for, the last one kept: the next pixel most
/// likely has the same swatch, and a palette finds a swatch by going through them all.
struct Shades<'s, 'a, F: Fn(SwatchId) -> Shade<'a>> {
    shade: &'s F,
    last: Option<(SwatchId, Shade<'a>)>,
}

impl<'s, 'a, F: Fn(SwatchId) -> Shade<'a>> Shades<'s, 'a, F> {
    fn new(shade: &'s F) -> Self {
        Shades { shade, last: None }
    }

    fn of(&mut self, swatch: SwatchId) -> Shade<'a> {
        match self.last {
            Some((id, shade)) if id == swatch => shade,
            _ => {
                let shade = (self.shade)(swatch);
                self.last = Some((swatch, shade));
                shade
            }
        }
    }
}

/// The pixels of row `y` of `surface` from `x0`, as many as `out` holds, as [`shaded`] paints
/// them: each pixel read once.
#[allow(clippy::too_many_arguments)]
fn shaded_row<'a, F: Fn(SwatchId) -> Shade<'a>>(
    surface: &IndexedSurface,
    art: ArtLayerKind,
    x0: i32,
    y: i32,
    areas: &Areas,
    shades: &mut Shades<'_, 'a, F>,
    linear: bool,
    out: &mut [[u8; 4]],
) {
    let own = areas.layers.iter().find(|own| own.art == art);
    let painted = surface.pixels(Rect::new(x0, y, x0 + out.len() as i32, y + 1));
    for (i, (pixel, out)) in painted.iter().zip(out.iter_mut()).enumerate() {
        if pixel.coverage == 0 {
            *out = [0; 4];
            continue;
        }
        let x = x0 + i as i32;
        let coverage = f32::from(pixel.coverage) / 65535.0;
        let [r, g, b, a] = match own.and_then(|own| Some(&own.areas[own.label(x, y)? as usize - 1])) {
            Some(area) => shades.of(area.swatch).at([x as f32 + 0.5, y as f32 + 0.5], &area.frame, linear),
            None => shades.of(pixel.swatch).color(),
        };
        *out = [r, g, b, (f32::from(a) * coverage + 0.5) as u8];
    }
}

impl Drawing {
    /// The filled areas of its swatches that `shade` gives as not solid, each with where its
    /// gradient or picture lies: by the drawing's placement for it, or fitted to it.
    pub fn areas<'a>(&self, shade: impl Fn(SwatchId) -> Shade<'a> + Sync) -> Areas {
        let layers = self.art_layers.iter().filter_map(|layer| {
            let surface = match &layer.content {
                ArtContent::Indexed(surface) => surface,
                ArtContent::Vector(art) => art.raster(),
                ArtContent::Rgba(_) => return None,
            };
            LayerAreas::of(layer.kind, surface, &shade, &self.placements)
        });
        Areas { layers: layers.collect() }
    }

    /// As [`Drawing::to_rgba8`], with the swatches that `shade` gives as gradients and
    /// pictures painted so, where `areas` (of the same drawing and shades) says they lie.
    /// Gradients are blended in linear light when `linear`.
    pub fn to_rgba8_shaded<'a>(&self, rect: Rect, areas: &Areas, shade: impl Fn(SwatchId) -> Shade<'a> + Sync, linear: bool) -> Vec<[u8; 4]> {
        use rayon::prelude::*;
        if areas.is_empty() {
            return self.to_rgba8(rect, |swatch| shade(swatch).color());
        }
        let width = rect.width() as usize;
        let mut out = vec![[0u8; 4]; width * rect.height() as usize];
        if width == 0 {
            return out;
        }
        // Each art layer only over the part of `rect` it has paint in, and the rows each at
        // once: this is done for every look at a drawing being drawn on with a gradient.
        let layers: Vec<_> =
            self.art_layers.iter().map(|layer| (layer, layer.content.bounds().intersect(&rect))).filter(|(_, part)| !part.is_empty()).collect();
        out.par_chunks_mut(width).enumerate().for_each(|(row, out)| {
            let y = rect.y0 + row as i32;
            let mut shades = Shades::new(&shade);
            let mut pixels = Vec::new();
            for (layer, part) in layers.iter().filter(|(_, part)| (part.y0..part.y1).contains(&y)) {
                pixels.clear();
                pixels.resize(part.width() as usize, [0u8; 4]);
                match &layer.content {
                    ArtContent::Rgba(surface) => surface.read_rgba8_into(Rect::new(part.x0, y, part.x1, y + 1), &mut pixels),
                    content => shaded_row(
                        content.indexed().expect("indexed or vector art has indexed pixels"),
                        layer.kind,
                        part.x0,
                        y,
                        areas,
                        &mut shades,
                        linear,
                        &mut pixels,
                    ),
                }
                let from = (part.x0 - rect.x0) as usize;
                for (under, over) in out[from..from + pixels.len()].iter_mut().zip(&pixels) {
                    *under = over_straight(*over, *under);
                }
            }
        });
        out
    }

    /// Put `placement` where the area its point is in lies, in place of any the area had.
    /// Placements of no area any more are let go. `areas` are the drawing's, found by
    /// [`Drawing::areas`].
    pub fn place(&mut self, areas: &Areas, placement: Placement) {
        self.unplace(areas, placement.art, placement.at);
        self.placements.push(placement);
    }

    /// Let go of the placement of the area the pixel `at` of art layer `art` is in, so it is
    /// fitted to its area again; and of any that is of no area any more.
    pub fn unplace(&mut self, areas: &Areas, art: ArtLayerKind, at: [i32; 2]) {
        let target = areas.at(art, at[0], at[1]).copied();
        self.placements.retain(|placement| match areas.at(placement.art, placement.at[0], placement.at[1]) {
            Some(area) => area.swatch == placement.swatch && !(placement.art == art && Some(*area) == target),
            None => false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    fn ramp() -> Gradient {
        Gradient { shape: GradientShape::Linear, stops: vec![GradientStop { at: 0.0, color: RED }, GradientStop { at: 1.0, color: BLUE }] }
    }

    fn drawing_of(rects: &[(Rect, u16)]) -> Drawing {
        let mut drawing = Drawing::default();
        let surface = match &mut drawing.art_layers[0].content {
            ArtContent::Indexed(surface) => surface,
            _ => unreachable!(),
        };
        for (rect, swatch) in rects {
            surface.paint(*rect, &vec![1.0; (rect.width() * rect.height()) as usize], SwatchId(*swatch));
        }
        drawing
    }

    #[test]
    fn a_gradient_blends_its_stops_and_holds_its_ends() {
        let g = ramp();
        assert_eq!((g.color_at(-1.0, false), g.color_at(2.0, false)), (RED, BLUE));
        assert_eq!(g.color_at(0.5, false), [128, 0, 128, 255]);
        // In linear light the middle is lighter.
        assert_eq!(g.color_at(0.5, true), [188, 0, 188, 255]);
        let sorted = Gradient { stops: vec![GradientStop { at: 1.5, color: BLUE }, GradientStop { at: -1.0, color: RED }], ..ramp() }.sorted();
        assert_eq!(sorted, ramp());
    }

    #[test]
    fn each_area_of_a_gradient_swatch_has_it_across_itself() {
        // Two squares of the gradient swatch apart, and a solid one between them.
        let drawing = drawing_of(&[(Rect::new(0, 0, 10, 10), 2), (Rect::new(20, 0, 30, 10), 2), (Rect::new(10, 0, 20, 10), 1)]);
        let g = ramp();
        let shade = |swatch: SwatchId| if swatch == SwatchId(2) { Shade::Gradient(&g) } else { Shade::Solid([0, 255, 0, 255]) };
        let areas = drawing.areas(shade);
        assert_eq!(areas.all().count(), 2, "the solid swatch has no areas");
        let pixels = drawing.to_rgba8_shaded(Rect::new(0, 0, 30, 1), &areas, shade, false);
        // Each square runs from red at its left to blue at its right.
        assert_eq!((pixels[0], pixels[20]), ([242, 0, 13, 255], [242, 0, 13, 255]));
        assert_eq!((pixels[9], pixels[29]), ([13, 0, 242, 255], [13, 0, 242, 255]));
        assert_eq!(pixels[15], [0, 255, 0, 255]);
    }

    #[test]
    fn a_placement_goes_with_its_area_and_is_let_go_with_it() {
        let mut drawing = drawing_of(&[(Rect::new(0, 0, 10, 10), 2), (Rect::new(20, 0, 30, 10), 2)]);
        let g = ramp();
        let shade = |_| Shade::Gradient(&g);
        // The right square's gradient turned round: blue at its left.
        let turned =
            Placement { art: ArtLayerKind::ColourArt, swatch: SwatchId(2), at: [25, 5], start: [30.0, 0.0], end: [20.0, 0.0], across: None, focus: [0.0; 2] };
        drawing.place(&drawing.areas(shade), turned);
        let areas = drawing.areas(shade);
        let pixels = drawing.to_rgba8_shaded(Rect::new(0, 0, 30, 1), &areas, shade, false);
        assert_eq!((pixels[0], pixels[20]), ([242, 0, 13, 255], [13, 0, 242, 255]));
        // Placed again, it is in place of the first; let go, it is fitted again.
        drawing.place(&areas, Placement { at: [22, 2], ..turned });
        assert_eq!(drawing.placements.len(), 1);
        drawing.unplace(&drawing.areas(shade), ArtLayerKind::ColourArt, [21, 1]);
        assert!(drawing.placements.is_empty());
    }

    #[test]
    fn a_frame_is_skewed_and_a_ring_has_its_focus_off_its_middle() {
        let g = ramp();
        let linear = Shade::Gradient(&g);
        // Its other side leant over by as much as it is long: the bands lean with it.
        let leant = Frame { start: [0.0, 0.0], end: [10.0, 0.0], across: [10.0, 10.0], focus: [0.0; 2] };
        assert_eq!(leant.local([15.0, 10.0]), [0.5, 1.0]);
        assert_eq!(linear.at([5.0, 0.0], &leant, false), linear.at([15.0, 10.0], &leant, false));
        assert_ne!(linear.at([5.0, 10.0], &leant, false), linear.at([5.0, 0.0], &leant, false));
        // Not skewed, it is square to its line, as long, and as before.
        assert_eq!(linear.square([0.0, 0.0], [10.0, 0.0]).across, [0.0, 10.0]);

        // A ring of 10 about the origin, its focus halfway to the right: the first colour there,
        // the last on the ring either side, and a third of the way at the middle.
        let radial = Gradient { shape: GradientShape::Radial, ..ramp() };
        let ring = Frame { focus: [0.5, 0.0], ..linear.square([0.0, 0.0], [10.0, 0.0]) };
        assert_eq!(focal([0.5, 0.0], ring.focus), 0.0);
        assert!((focal([1.0, 0.0], ring.focus) - 1.0).abs() < 1e-5 && (focal([-1.0, 0.0], ring.focus) - 1.0).abs() < 1e-5);
        assert!((focal([0.0, 0.0], ring.focus) - 1.0 / 3.0).abs() < 1e-5);
        assert_eq!(Shade::Gradient(&radial).at([5.0, 0.0], &ring, false), RED);
        // A focus on or past the ring is kept inside it.
        assert_eq!(Frame::inside_ring([2.0, 0.0]), [0.95, 0.0]);
    }

    #[test]
    fn a_picture_shown_once_leaves_the_rest_in_the_swatchs_colour() {
        let picture = Picture { width: 2, height: 1, pixels: vec![RED, BLUE] };
        let grey = [9, 9, 9, 255];
        let once = Shade::Texture { picture: Some(&picture), tile: false, once: true, scale: 1.0, color: grey };
        let frame = once.square([0.0, 0.0], [2.0, 0.0]);
        assert_eq!(frame.across, [0.0, 1.0], "as high as the picture is to its width");
        assert_eq!((once.at([0.5, 0.5], &frame, false), once.at([3.5, 0.5], &frame, false)), (RED, grey));
        // With its edges drawn out, the area past it is its edge.
        let edges = Shade::Texture { picture: Some(&picture), tile: false, once: false, scale: 1.0, color: grey };
        assert_eq!(edges.at([3.5, 0.5], &frame, false), BLUE);
    }

    #[test]
    fn a_picture_is_tiled_from_its_areas_corner() {
        let picture = Picture { width: 2, height: 1, pixels: vec![RED, BLUE] };
        let drawing = drawing_of(&[(Rect::new(0, 0, 8, 1), 3)]);
        let shade = |_| Shade::Texture { picture: Some(&picture), tile: true, once: false, scale: 1.0, color: RED };
        let areas = drawing.areas(shade);
        let pixels = drawing.to_rgba8_shaded(Rect::new(0, 0, 8, 1), &areas, shade, false);
        assert_eq!(pixels[0..4], [RED, BLUE, RED, BLUE]);
        // Its one colour is the swatch's, not worked out from the picture again for each pixel.
        let large = Picture { width: 4000, height: 3000, pixels: vec![RED; 12_000_000] };
        let texture = Shade::Texture { picture: Some(&large), tile: true, once: false, scale: 1.0, color: [1, 2, 3, 255] };
        let started = std::time::Instant::now();
        assert!((0..100_000).all(|_| texture.color() == [1, 2, 3, 255]));
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
        // Without its picture it is its colour, and has no areas.
        let missing = |_| Shade::Texture { picture: None, tile: true, once: false, scale: 1.0, color: [9, 9, 9, 255] };
        assert!(drawing.areas(missing).is_empty());
    }
}
