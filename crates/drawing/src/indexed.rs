//! Indexed pixels: each pixel stores which swatch it uses and how much of the pixel it covers,
//! and gets its colour from the palette when rendered (plan §4.8). Changing a swatch then
//! recolours every drawing that uses it without touching the drawings.

use std::collections::BTreeSet;

use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_geom::{Affine, Point, Rect, TILE_SIZE, TileCoord};
use photocraft_raster::Surface;
use serde::{Deserialize, Serialize};

/// A palette swatch's permanent id. 0 is "no swatch", the id of an empty pixel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SwatchId(pub u16);

impl SwatchId {
    pub const NONE: SwatchId = SwatchId(0);

    /// The ids from here up, but the last, are a swatch's colour drifted (the user's of
    /// 2026-11-04): a vector stroke's colour drifts by its points towards its swatch's drift,
    /// and a pixel holds one id, so a pixel of drifted colour holds its swatch's id with how far
    /// it drifted, of [`SwatchId::DRIFTS`], in the bits above it. Only swatches below 2047 drift.
    const TONES: u16 = 0x8000;

    /// How many steps a colour drifts in, past its own.
    pub const DRIFTS: u8 = 15;

    /// The id of this swatch's colour drifted `level` steps of [`SwatchId::DRIFTS`]: itself at
    /// 0, and for a swatch that can't drift.
    pub fn drifted(self, level: u8) -> SwatchId {
        if level == 0 || self.0 == 0 || self.0 >= 0x7FF || self.is_drifted() {
            return self;
        }
        SwatchId(Self::TONES | u16::from(level.min(Self::DRIFTS)) << 11 | self.0)
    }

    /// Whether it is a swatch's colour drifted.
    pub fn is_drifted(self) -> bool {
        self.0 >= Self::TONES && self.0 != u16::MAX
    }

    /// The swatch whose colour it is: itself, or the one it is drifted from.
    pub fn base(self) -> SwatchId {
        if self.is_drifted() { SwatchId(self.0 & 0x7FF) } else { self }
    }

    /// How many steps of [`SwatchId::DRIFTS`] its colour has drifted.
    pub fn drift(self) -> u8 {
        if self.is_drifted() { ((self.0 >> 11) & 0xF) as u8 } else { 0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexedPixel {
    pub swatch: SwatchId,
    /// How much of the pixel is painted, 0..=65535. It carries antialiasing and pen pressure.
    pub coverage: u16,
}

/// Bytes per pixel in a tile: the swatch id then the coverage, both `u16` in native byte order.
const BPP: usize = 4;

/// The tiles reuse the sparse copy-on-write [`Surface`] as two-channel 16-bit storage.
const FORMAT: PixelFormat = PixelFormat::new(ColorMode::Grayscale, SampleType::U16, true);

/// A sparse, unbounded plane of indexed pixels in copy-on-write 256×256 tiles.
///
/// An unpainted pixel is all zero (coverage 0 and [`SwatchId::NONE`]), so empty areas cost
/// nothing and [`IndexedSurface::bounds`] is exact.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexedSurface {
    surface: Surface,
}

impl Default for IndexedSurface {
    fn default() -> Self {
        Self::new()
    }
}

fn decode(bytes: &[u8; BPP]) -> IndexedPixel {
    IndexedPixel { swatch: SwatchId(u16::from_ne_bytes([bytes[0], bytes[1]])), coverage: u16::from_ne_bytes([bytes[2], bytes[3]]) }
}

fn encode(pixel: IndexedPixel, bytes: &mut [u8; BPP]) {
    let pixel = if pixel.coverage == 0 { IndexedPixel::default() } else { pixel };
    bytes[..2].copy_from_slice(&pixel.swatch.0.to_ne_bytes());
    bytes[2..4].copy_from_slice(&pixel.coverage.to_ne_bytes());
}

/// `coverage` (0..1) of `swatch` painted over a pixel. A pixel holds one swatch: the new one
/// takes the pixel where its coverage is at least what was there, and the coverage builds up
/// either way, so painting never opens a hole.
fn over(old: IndexedPixel, swatch: SwatchId, coverage: f32) -> IndexedPixel {
    let before = to_unit(old.coverage);
    // Compared as stored, so equal coverages tie exactly and the new swatch takes the tie.
    let swatch = if old.swatch.base() == swatch.base() || from_unit(coverage) >= old.coverage { swatch } else { old.swatch };
    IndexedPixel { swatch, coverage: from_unit(before + coverage * (1.0 - before)) }
}

/// How a brush lays its swatch on what is painted already.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaintMode {
    /// Over it, as paint is laid.
    #[default]
    Over,
    /// Under it: what is there stays in front, and the new paint shows where it is thin.
    Behind,
    /// Only on what is painted: its swatch changes where the brush goes, and the shape and how
    /// thick it is stay; the empty canvas stays empty.
    Repaint,
}

/// `old` with `swatch` laid under it with `coverage`.
fn under(old: IndexedPixel, swatch: SwatchId, coverage: f32) -> IndexedPixel {
    let before = to_unit(old.coverage);
    let added = coverage * (1.0 - before);
    // What was there keeps the pixel unless what shows of the new paint is more.
    let swatch = if old.coverage == 0 || old.swatch.base() == swatch.base() || from_unit(added) > old.coverage { swatch } else { old.swatch };
    IndexedPixel { swatch, coverage: from_unit(before + added) }
}

/// Whether a pixel is painted with a swatch that is protected, its colour or drifted, and so
/// stays as it is.
fn kept(pixel: IndexedPixel, protected: &[SwatchId]) -> bool {
    pixel.coverage > 0 && protected.contains(&pixel.swatch.base())
}

fn to_unit(coverage: u16) -> f32 {
    f32::from(coverage) / 65535.0
}

fn from_unit(coverage: f32) -> u16 {
    (coverage.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

impl IndexedSurface {
    pub fn new() -> Self {
        Self { surface: Surface::new(FORMAT) }
    }

    /// The tiles, for storage and for upload to the GPU.
    pub fn tiles(&self) -> &Surface {
        &self.surface
    }

    /// The tiles for a project store to fill when it loads a drawing. What is written must keep
    /// the layout [`IndexedSurface::tiles`] has: an unpainted pixel is all zero.
    pub fn raw_tiles_mut(&mut self) -> &mut Surface {
        &mut self.surface
    }

    pub fn is_empty(&self) -> bool {
        self.bounds().is_empty()
    }

    /// Exact bounds of the painted pixels.
    pub fn bounds(&self) -> Rect {
        self.surface.content_bounds()
    }

    /// The swatches that anything is painted with, in id order: a drifted colour's swatch.
    pub fn swatches(&self) -> BTreeSet<SwatchId> {
        let pixels = self.surface.tiles().flat_map(|(_, tile)| tile.bytes().as_chunks::<BPP>().0);
        pixels.map(decode).filter(|pixel| pixel.coverage > 0).map(|pixel| pixel.swatch.base()).collect()
    }

    /// Whether anything is painted with a swatch that `wanted` says yes to, asked of each
    /// swatch as it is met: of a drifted colour, its swatch.
    pub fn paints_with(&self, wanted: impl Fn(SwatchId) -> bool) -> bool {
        let mut last: Option<(SwatchId, bool)> = None;
        let pixels = self.surface.tiles().flat_map(|(_, tile)| tile.bytes().as_chunks::<BPP>().0);
        pixels.map(decode).filter(|pixel| pixel.coverage > 0).any(|pixel| match last {
            Some((swatch, answer)) if swatch == pixel.swatch => answer,
            _ => {
                let answer = wanted(pixel.swatch.base());
                last = Some((pixel.swatch, answer));
                answer
            }
        })
    }

    pub fn pixel(&self, x: i32, y: i32) -> IndexedPixel {
        let Some(tile) = self.surface.tile(TileCoord::containing(x, y)) else { return IndexedPixel::default() };
        let at = (y.rem_euclid(TILE_SIZE) * TILE_SIZE + x.rem_euclid(TILE_SIZE)) as usize;
        tile.bytes().as_chunks::<BPP>().0.get(at).map_or(IndexedPixel::default(), decode)
    }

    /// Change the pixels of `rect` where `coverage` (one value per pixel, in rows) is above
    /// zero. `edit` gets the old pixel and that coverage (0..1) and returns the new pixel.
    /// Tiles the coverage doesn't touch are neither allocated nor copied.
    fn edit(&mut self, rect: Rect, coverage: &[f32], edit: impl Fn(IndexedPixel, f32) -> IndexedPixel) {
        let width = rect.width() as usize;
        assert_eq!(coverage.len(), width * rect.height() as usize, "one coverage value per pixel of the rect");
        for coord in rect.tiles() {
            let origin = coord.rect();
            let part = rect.intersect(&origin);
            let row = |y: i32| {
                let start = (y - rect.y0) as usize * width + (part.x0 - rect.x0) as usize;
                &coverage[start..start + part.width() as usize]
            };
            if !(part.y0..part.y1).any(|y| row(y).iter().any(|&c| c > 0.0)) {
                continue;
            }
            let pixels = self.surface.tile_mut(coord).bytes_mut().as_chunks_mut::<BPP>().0;
            for y in part.y0..part.y1 {
                let start = ((y - origin.y0) * TILE_SIZE + (part.x0 - origin.x0)) as usize;
                for (&c, px) in row(y).iter().zip(&mut pixels[start..]) {
                    if c > 0.0 {
                        encode(edit(decode(px), c.min(1.0)), px);
                    }
                }
            }
        }
    }

    /// A surface of the `pixels` of `rect` (straight-alpha RGBA8, in rows): each with the
    /// swatch `swatch` gives for its colour, and its alpha as its coverage.
    pub fn of_rgba8(rect: Rect, pixels: &[[u8; 4]], swatch: impl Fn([u8; 4]) -> SwatchId) -> IndexedSurface {
        let mut out = IndexedSurface::new();
        let width = rect.width().max(1) as usize;
        for (i, pixel) in pixels.iter().enumerate().filter(|(_, pixel)| pixel[3] > 0) {
            let made = IndexedPixel { swatch: swatch(*pixel), coverage: u16::from(pixel[3]) * 257 };
            if made.swatch != SwatchId::NONE {
                out.set(rect.x0 + (i % width) as i32, rect.y0 + (i / width) as i32, made);
            }
        }
        out
    }

    /// The pixels of `rect`, in rows.
    pub fn pixels(&self, rect: Rect) -> Vec<IndexedPixel> {
        let width = rect.width() as usize;
        let mut out = vec![IndexedPixel::default(); width * rect.height() as usize];
        for coord in rect.tiles() {
            let Some(tile) = self.surface.tile(coord) else { continue };
            let pixels = tile.bytes().as_chunks::<BPP>().0;
            let origin = coord.rect();
            let part = rect.intersect(&origin);
            for y in part.y0..part.y1 {
                let from = ((y - origin.y0) * TILE_SIZE + (part.x0 - origin.x0)) as usize;
                let to = (y - rect.y0) as usize * width + (part.x0 - rect.x0) as usize;
                for (px, out) in pixels[from..].iter().zip(&mut out[to..to + part.width() as usize]) {
                    *out = decode(px);
                }
            }
        }
        out
    }

    /// Make the pixels of `rect` that `mask` marks (one value per pixel, in rows) `pixel`.
    /// Tiles the mask doesn't touch are neither allocated nor copied.
    pub fn set_where(&mut self, rect: Rect, mask: &[bool], pixel: IndexedPixel) {
        let coverage: Vec<f32> = mask.iter().map(|&marked| if marked { 1.0 } else { 0.0 }).collect();
        self.edit(rect, &coverage, |_, _| pixel);
    }

    /// Give the pixels of `rect` that `mask` marks (one value per pixel, in rows) the swatch
    /// `swatch`, each with the coverage it has; [`SwatchId::NONE`] makes them empty.
    pub fn swatch_where(&mut self, rect: Rect, mask: &[bool], swatch: SwatchId) {
        let coverage: Vec<f32> = mask.iter().map(|&marked| if marked { 1.0 } else { 0.0 }).collect();
        self.edit(rect, &coverage, |old, _| if swatch == SwatchId::NONE { IndexedPixel::default() } else { IndexedPixel { swatch, ..old } });
    }

    /// Paint `swatch` with a stroke's `coverage` (one value 0..1 per pixel of `rect`, in rows).
    ///
    /// A pixel holds one swatch, so there is no blending between two colours inside a pixel:
    /// the new swatch takes the pixel where its coverage is at least what was there, and the
    /// pixel's coverage builds up either way, so painting never opens a hole.
    pub fn paint(&mut self, rect: Rect, coverage: &[f32], swatch: SwatchId) {
        self.paint_protecting(rect, coverage, swatch, &[]);
    }

    /// [`IndexedSurface::paint`], leaving every pixel painted with one of the `protected`
    /// swatches as it is.
    pub fn paint_protecting(&mut self, rect: Rect, coverage: &[f32], swatch: SwatchId, protected: &[SwatchId]) {
        if swatch == SwatchId::NONE {
            return;
        }
        self.edit(rect, coverage, |old, c| if kept(old, protected) { old } else { over(old, swatch, c) });
    }

    /// [`IndexedSurface::paint_as`] of a stroke whose colour drifts: `drift` (one value 0..1 a
    /// pixel, as `coverage`) is how far each pixel's is drifted, to the nearest step. `None`
    /// for one whose colour does not drift.
    pub fn paint_drifted(&mut self, mode: PaintMode, rect: Rect, coverage: &[f32], drift: Option<&[f32]>, swatch: SwatchId, protected: &[SwatchId]) {
        let Some(drift) = drift.filter(|_| swatch.drifted(1) != swatch) else {
            return self.paint_as(mode, rect, coverage, swatch, protected);
        };
        let level = |d: f32| (d.clamp(0.0, 1.0) * f32::from(SwatchId::DRIFTS)).round() as u8;
        let mut levels = [false; SwatchId::DRIFTS as usize + 1];
        coverage.iter().zip(drift).filter(|(c, _)| **c > 0.0).for_each(|(_, d)| levels[usize::from(level(*d))] = true);
        for (at, _) in levels.iter().enumerate().filter(|(_, used)| **used) {
            let of_level: Vec<f32> = coverage.iter().zip(drift).map(|(c, d)| if usize::from(level(*d)) == at { *c } else { 0.0 }).collect();
            self.paint_as(mode, rect, &of_level, swatch.drifted(at as u8), protected);
        }
    }

    /// [`IndexedSurface::paint_protecting`] in `mode`: over what is there, under it, or on it
    /// alone, where the brush covers a pixel by a quarter or more.
    pub fn paint_as(&mut self, mode: PaintMode, rect: Rect, coverage: &[f32], swatch: SwatchId, protected: &[SwatchId]) {
        if swatch == SwatchId::NONE {
            return;
        }
        match mode {
            PaintMode::Over => self.paint_protecting(rect, coverage, swatch, protected),
            PaintMode::Behind => self.edit(rect, coverage, |old, c| if kept(old, protected) { old } else { under(old, swatch, c) }),
            PaintMode::Repaint => {
                self.edit(rect, coverage, |old, c| if kept(old, protected) || old.coverage == 0 || c < 0.25 { old } else { IndexedPixel { swatch, ..old } })
            }
        }
    }

    /// Erase with a stroke's `coverage` (one value 0..1 per pixel of `rect`, in rows). Tiles
    /// it empties stay allocated until [`IndexedSurface::prune`].
    pub fn erase(&mut self, rect: Rect, coverage: &[f32]) {
        self.erase_protecting(rect, coverage, &[]);
    }

    /// [`IndexedSurface::erase`], leaving every pixel painted with one of the `protected`
    /// swatches as it is.
    pub fn erase_protecting(&mut self, rect: Rect, coverage: &[f32], protected: &[SwatchId]) {
        let erased = |old: IndexedPixel, c: f32| IndexedPixel { swatch: old.swatch, coverage: from_unit(to_unit(old.coverage) * (1.0 - c)) };
        self.edit(rect, coverage, |old, c| if kept(old, protected) { old } else { erased(old, c) });
    }

    /// Give every pixel painted with `from` the swatch `to`, with the coverage it has, and its
    /// colour drifted as far. Returns whether anything was painted with `from`.
    pub fn recolour(&mut self, from: SwatchId, to: SwatchId) -> bool {
        let of = |pixel: &IndexedPixel| pixel.coverage > 0 && pixel.swatch.base() == from;
        let has = |tile: &photocraft_raster::Tile| tile.bytes().as_chunks::<BPP>().0.iter().map(decode).any(|pixel| of(&pixel));
        let with: Vec<TileCoord> = self.surface.tiles().filter(|(_, tile)| has(tile)).map(|(coord, _)| *coord).collect();
        for coord in &with {
            for px in self.surface.tile_mut(*coord).bytes_mut().as_chunks_mut::<BPP>().0 {
                let pixel = decode(px);
                if of(&pixel) {
                    encode(IndexedPixel { swatch: to.drifted(pixel.swatch.drift()), ..pixel }, px);
                }
            }
        }
        !with.is_empty()
    }

    /// Free the tiles that have no painted pixel left.
    pub fn prune(&mut self) {
        self.surface.prune();
    }

    /// Make the tiles that overlap `rect` the same as `other`'s. The tiles are shared, not
    /// copied.
    pub fn copy_tiles_from(&mut self, other: &IndexedSurface, rect: Rect) {
        self.surface.take_tiles(rect);
        self.surface.put_tiles(rect.tiles().filter_map(|c| other.surface.tile(c).map(|tile| (c, tile.clone()))));
    }

    /// The painted pixels as (x, y, pixel), in no particular order.
    fn painted(&self) -> impl Iterator<Item = (i32, i32, IndexedPixel)> + '_ {
        self.surface.tiles().flat_map(|(coord, tile)| {
            let origin = coord.rect();
            let pixels = tile.bytes().as_chunks::<BPP>().0.iter().enumerate();
            pixels.map(move |(i, px)| (origin.x0 + i as i32 % TILE_SIZE, origin.y0 + i as i32 / TILE_SIZE, decode(px))).filter(|(_, _, p)| p.coverage > 0)
        })
    }

    /// Set one pixel, allocating its tile.
    fn set(&mut self, x: i32, y: i32, pixel: IndexedPixel) {
        let at = (y.rem_euclid(TILE_SIZE) * TILE_SIZE + x.rem_euclid(TILE_SIZE)) as usize;
        encode(pixel, &mut self.surface.tile_mut(TileCoord::containing(x, y)).bytes_mut().as_chunks_mut::<BPP>().0[at]);
    }

    /// The pixels inside `rect`, as a surface of their own (a selection lifted off the drawing).
    pub fn extract(&self, rect: Rect) -> IndexedSurface {
        let mut out = IndexedSurface::new();
        // Whole tiles are shared; the tiles on the rect's edge are then trimmed.
        out.surface.put_tiles(rect.tiles().filter_map(|c| self.surface.tile(c).map(|tile| (c, tile.clone()))));
        let outside: Vec<(i32, i32)> = out.painted().filter(|&(x, y, _)| !rect.contains(x, y)).map(|(x, y, _)| (x, y)).collect();
        for (x, y) in outside {
            out.set(x, y, IndexedPixel::default());
        }
        out.prune();
        out
    }

    /// Erase everything inside `rect`.
    pub fn clear(&mut self, rect: Rect) {
        let inside: Vec<(i32, i32)> = self.extract(rect).painted().map(|(x, y, _)| (x, y)).collect();
        for (x, y) in inside {
            self.set(x, y, IndexedPixel::default());
        }
        self.prune();
    }

    /// What a selection takes of this surface, as a surface of its own: of what is in its
    /// rectangle, what is inside its lasso, of its swatch, and not protected.
    pub fn extract_picked(&self, picked: &crate::Picked) -> IndexedSurface {
        let mut out = self.extract(picked.rect);
        if picked.is_all() {
            return out;
        }
        let left: Vec<(i32, i32)> = out.painted().filter(|(x, y, pixel)| !picked.takes(*x, *y, *pixel)).map(|(x, y, _)| (x, y)).collect();
        for (x, y) in left {
            out.set(x, y, IndexedPixel::default());
        }
        out.prune();
        out
    }

    /// Give what a selection takes of this surface the swatch `to`, each pixel with the
    /// coverage it has. Returns whether it changed anything.
    pub fn recolour_picked(&mut self, picked: &crate::Picked, to: SwatchId) -> bool {
        let taken: Vec<(i32, i32, IndexedPixel)> = self.extract_picked(picked).painted().filter(|(_, _, pixel)| pixel.swatch.base() != to).collect();
        for (x, y, pixel) in &taken {
            self.set(*x, *y, IndexedPixel { swatch: to.drifted(pixel.swatch.drift()), ..*pixel });
        }
        !taken.is_empty()
    }

    /// Erase what a selection takes of this surface.
    pub fn clear_picked(&mut self, picked: &crate::Picked) {
        let taken: Vec<(i32, i32)> = self.extract_picked(picked).painted().map(|(x, y, _)| (x, y)).collect();
        for (x, y) in taken {
            self.set(x, y, IndexedPixel::default());
        }
        self.prune();
    }

    /// Paint `other` over this surface, each pixel with its own swatch (pasting a selection).
    pub fn stamp(&mut self, other: &IndexedSurface) {
        // On nothing, it is what it is.
        if self.is_empty() {
            *self = other.clone();
            return;
        }
        let pixels: Vec<(i32, i32, IndexedPixel)> = other.painted().collect();
        for (x, y, pixel) in pixels {
            self.set(x, y, over(self.pixel(x, y), pixel.swatch, to_unit(pixel.coverage)));
        }
    }

    /// The surface moved by `dx` and `dy` whole pixels, tile by tile.
    pub fn translated(&self, dx: i32, dy: i32) -> IndexedSurface {
        IndexedSurface { surface: self.surface.translated(dx, dy, self.bounds()) }
    }

    /// The surface moved, scaled or turned by `transform`: each new pixel is read back from
    /// where it came from, with coverage interpolated and the swatch that contributes most.
    /// A whole-pixel move is exact.
    pub fn transformed(&self, transform: &Affine) -> IndexedSurface {
        // Moved by whole pixels: its tiles copied across, not each pixel read back.
        let [a, b, c, d, e, f] = transform.m;
        if [a, b, c, d] == [1.0, 0.0, 0.0, 1.0] && e.fract() == 0.0 && f.fract() == 0.0 && e.abs() < 1e9 && f.abs() < 1e9 {
            return self.translated(e as i32, f as i32);
        }
        let mut out = IndexedSurface::new();
        let (bounds, Some(inverse)) = (self.bounds(), transform.inverse()) else { return out };
        if bounds.is_empty() {
            return out;
        }
        let corners = [(bounds.x0, bounds.y0), (bounds.x1, bounds.y0), (bounds.x1, bounds.y1), (bounds.x0, bounds.y1)]
            .map(|(x, y)| transform.apply(Point::new(f64::from(x), f64::from(y))));
        let (xs, ys) = (corners.map(|p| p.x), corners.map(|p| p.y));
        let low = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min).floor() as i32 - 1;
        let high = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max).ceil() as i32 + 1;
        let target = Rect::new(low(xs), low(ys), high(xs), high(ys));
        for y in target.y0..target.y1 {
            for x in target.x0..target.x1 {
                // The centre of the new pixel, back in the source, as the corner of four pixels.
                let source = inverse.apply(Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5));
                let (fx, fy) = (source.x - 0.5, source.y - 0.5);
                let (x0, y0) = (fx.floor(), fy.floor());
                let (wx, wy) = (fx - x0, fy - y0);
                let mut total = 0.0;
                let mut best = (SwatchId::NONE, 0.0);
                let mut shares: [(SwatchId, f64); 4] = [(SwatchId::NONE, 0.0); 4];
                for (i, (dx, dy, weight)) in
                    [(0, 0, (1.0 - wx) * (1.0 - wy)), (1, 0, wx * (1.0 - wy)), (0, 1, (1.0 - wx) * wy), (1, 1, wx * wy)].into_iter().enumerate()
                {
                    let pixel = self.pixel(x0 as i32 + dx, y0 as i32 + dy);
                    let share = weight * f64::from(to_unit(pixel.coverage));
                    total += share;
                    shares[i] = (pixel.swatch, share);
                }
                for (swatch, _) in shares {
                    let sum: f64 = shares.iter().filter(|(s, _)| *s == swatch).map(|(_, share)| share).sum();
                    if sum > best.1 {
                        best = (swatch, sum);
                    }
                }
                let coverage = from_unit(total as f32);
                if coverage > 0 {
                    out.set(x, y, IndexedPixel { swatch: best.0, coverage });
                }
            }
        }
        out
    }

    /// The surface scaled by `scale` about the origin, for changing a drawing's resolution.
    /// Coverage is interpolated when enlarging and averaged when reducing; each new pixel takes
    /// the swatch that contributes most to it. Enlarging adds pixels, never detail.
    pub fn resampled(&self, scale: f64) -> IndexedSurface {
        let mut out = IndexedSurface::new();
        let bounds = self.bounds();
        if bounds.is_empty() || !scale.is_finite() || scale <= 0.0 {
            return out;
        }
        let grow = |v: i32, up: bool| if up { (f64::from(v) * scale).ceil() as i32 + 1 } else { (f64::from(v) * scale).floor() as i32 - 1 };
        let target = Rect::new(grow(bounds.x0, false), grow(bounds.y0, false), grow(bounds.x1, true), grow(bounds.y1, true));
        // The source pixels that feed one new pixel along one axis, each with its weight.
        let taps = |v: i32| -> Vec<(i32, f64)> {
            if scale >= 1.0 {
                // Enlarging: the two nearest source pixels, weighted by distance.
                let at = (f64::from(v) + 0.5) / scale - 0.5;
                let low = at.floor();
                vec![(low as i32, 1.0 - (at - low)), (low as i32 + 1, at - low)]
            } else {
                // Reducing: every source pixel the new one covers, weighted by overlap.
                let (from, to) = (f64::from(v) / scale, f64::from(v + 1) / scale);
                (from.floor() as i32..to.ceil() as i32).map(|s| (s, (to.min(f64::from(s + 1)) - from.max(f64::from(s))) * scale)).collect()
            }
        };
        let columns: Vec<Vec<(i32, f64)>> = (target.x0..target.x1).map(taps).collect();
        let width = target.width() as usize;
        // One row at a time, so a large drawing doesn't need one large buffer per swatch.
        for y in target.y0..target.y1 {
            let rows = taps(y);
            let mut by_swatch: Vec<(SwatchId, Vec<f32>)> = Vec::new();
            for (column, x_taps) in columns.iter().enumerate() {
                let mut total = 0.0;
                let mut shares: Vec<(SwatchId, f64)> = Vec::new();
                for &(sy, wy) in &rows {
                    for &(sx, wx) in x_taps {
                        let pixel = self.pixel(sx, sy);
                        let share = wx * wy * f64::from(to_unit(pixel.coverage));
                        if share > 0.0 {
                            total += share;
                            match shares.iter_mut().find(|(swatch, _)| *swatch == pixel.swatch) {
                                Some(entry) => entry.1 += share,
                                None => shares.push((pixel.swatch, share)),
                            }
                        }
                    }
                }
                if let Some(&(swatch, _)) = shares.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
                    let row = match by_swatch.iter().position(|(s, _)| *s == swatch) {
                        Some(index) => &mut by_swatch[index].1,
                        None => {
                            by_swatch.push((swatch, vec![0.0; width]));
                            &mut by_swatch.last_mut().expect("just pushed").1
                        }
                    };
                    row[column] = total as f32;
                }
            }
            for (swatch, coverage) in by_swatch {
                out.paint(Rect::new(target.x0, y, target.x1, y + 1), &coverage, swatch);
            }
        }
        out
    }

    /// The pixels of `rect` as straight-alpha RGBA8 in rows, coloured by `color` (a swatch's
    /// straight-alpha sRGB colour). Coverage scales the swatch's alpha.
    pub fn to_rgba8(&self, rect: Rect, color: impl Fn(SwatchId) -> [u8; 4]) -> Vec<[u8; 4]> {
        let width = rect.width() as usize;
        let mut out = vec![[0u8; 4]; width * rect.height() as usize];
        // The colour of the swatch met last, which the next pixel most likely has too.
        let mut last: Option<(SwatchId, [u8; 4])> = None;
        for coord in rect.tiles() {
            let Some(tile) = self.surface.tile(coord) else { continue };
            let pixels = tile.bytes().as_chunks::<BPP>().0;
            let origin = coord.rect();
            let part = rect.intersect(&origin);
            for y in part.y0..part.y1 {
                let from = ((y - origin.y0) * TILE_SIZE + (part.x0 - origin.x0)) as usize;
                let to = (y - rect.y0) as usize * width + (part.x0 - rect.x0) as usize;
                for (px, out) in pixels[from..].iter().zip(&mut out[to..to + part.width() as usize]) {
                    let pixel = decode(px);
                    if pixel.coverage > 0 {
                        let [r, g, b, a] = match last {
                            Some((swatch, known)) if swatch == pixel.swatch => known,
                            _ => {
                                let known = color(pixel.swatch);
                                last = Some((pixel.swatch, known));
                                known
                            }
                        };
                        *out = [r, g, b, (f32::from(a) * to_unit(pixel.coverage) + 0.5) as u8];
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: SwatchId = SwatchId(1);
    const SKIN: SwatchId = SwatchId(2);

    fn coverage(s: &IndexedSurface, x: i32, y: i32) -> f32 {
        to_unit(s.pixel(x, y).coverage)
    }

    #[test]
    fn painting_stores_the_swatch_and_the_coverage() {
        let mut s = IndexedSurface::new();
        assert!(s.is_empty());
        s.paint(Rect::new(3, 5, 6, 6), &[1.0, 0.5, 0.0], LINE);
        assert_eq!(s.pixel(3, 5), IndexedPixel { swatch: LINE, coverage: 65535 });
        assert!((coverage(&s, 4, 5) - 0.5).abs() < 1e-4);
        assert_eq!(s.pixel(5, 5), IndexedPixel::default(), "zero coverage paints nothing");
        assert_eq!(s.bounds(), Rect::new(3, 5, 5, 6));
    }

    #[test]
    fn the_same_swatch_builds_up() {
        let mut s = IndexedSurface::new();
        let r = Rect::new(0, 0, 1, 1);
        s.paint(r, &[0.5], LINE);
        s.paint(r, &[0.5], LINE);
        assert!((coverage(&s, 0, 0) - 0.75).abs() < 1e-4);
    }

    #[test]
    fn another_swatch_takes_the_pixel_only_where_its_coverage_wins() {
        let mut s = IndexedSurface::new();
        let r = Rect::new(0, 0, 3, 1);
        s.paint(r, &[1.0, 0.25, 0.5], LINE);
        s.paint(r, &[0.25, 0.5, 0.5], SKIN);
        // A faint edge over a solid line leaves the line; a stronger or equal one takes over.
        assert_eq!(s.pixel(0, 0), IndexedPixel { swatch: LINE, coverage: 65535 });
        assert_eq!(s.pixel(1, 0).swatch, SKIN);
        assert_eq!(s.pixel(2, 0).swatch, SKIN);
        // Coverage builds up whichever swatch wins, so there is no hole between two colours.
        assert!((coverage(&s, 1, 0) - 0.625).abs() < 1e-4);
        assert!((coverage(&s, 2, 0) - 0.75).abs() < 1e-4);
        assert_eq!(s.swatches(), BTreeSet::from([LINE, SKIN]), "the swatches in use");
    }

    #[test]
    fn protected_swatches_are_not_painted_or_erased_and_a_swatch_is_given_another() {
        let mut s = IndexedSurface::new();
        let r = Rect::new(0, 0, 3, 1);
        s.paint(r, &[1.0, 0.5, 0.0], LINE);
        // Over a protected line, paint lands only where there is none of it.
        s.paint_protecting(r, &[1.0, 1.0, 1.0], SKIN, &[LINE]);
        assert_eq!((s.pixel(0, 0).swatch, s.pixel(1, 0), s.pixel(2, 0).swatch), (LINE, IndexedPixel { swatch: LINE, coverage: 32768 }, SKIN));
        s.erase_protecting(r, &[1.0, 1.0, 1.0], &[LINE]);
        assert_eq!((s.pixel(0, 0).coverage, s.pixel(2, 0)), (65535, IndexedPixel::default()), "the eraser leaves it too");

        assert!(s.recolour(LINE, SKIN) && !s.recolour(LINE, SKIN), "once there is none left, nothing is done");
        assert_eq!((s.pixel(0, 0), s.pixel(1, 0)), (IndexedPixel { swatch: SKIN, coverage: 65535 }, IndexedPixel { swatch: SKIN, coverage: 32768 }));
    }

    #[test]
    fn erasing_lowers_coverage_and_frees_empty_tiles() {
        let mut s = IndexedSurface::new();
        let r = Rect::new(0, 0, 2, 1);
        s.paint(r, &[1.0, 1.0], LINE);
        s.erase(r, &[0.5, 0.0]);
        assert!((coverage(&s, 0, 0) - 0.5).abs() < 1e-4);
        assert_eq!(s.pixel(0, 0).swatch, LINE);
        assert_eq!(s.pixel(1, 0).coverage, 65535, "untouched");
        s.erase(r, &[1.0, 1.0]);
        assert_eq!(s.pixel(0, 0), IndexedPixel::default(), "a fully erased pixel forgets its swatch");
        assert_eq!(s.tiles().tile_count(), 1);
        s.prune();
        assert!(s.is_empty());
        assert_eq!(s.tiles().tile_count(), 0);
    }

    #[test]
    fn painting_across_tiles_allocates_only_the_tiles_it_touches() {
        let mut s = IndexedSurface::new();
        // A 3-pixel row over the corner of four tiles, painted only left of x = 0.
        let r = Rect::new(-2, -1, 1, 1);
        s.paint(r, &[1.0, 1.0, 0.0, 0.0, 1.0, 0.0], LINE);
        assert_eq!(s.pixel(-2, -1).swatch, LINE);
        assert_eq!(s.pixel(-1, -1).swatch, LINE);
        assert_eq!(s.pixel(-1, 0).swatch, LINE);
        assert_eq!(s.pixel(0, 0), IndexedPixel::default());
        assert_eq!(s.tiles().tile_count(), 2, "the two tiles right of x = 0 got no paint");
        assert_eq!(s.bounds(), Rect::new(-2, -1, 0, 1));
    }

    #[test]
    fn resampling_scales_about_the_origin_and_keeps_swatches_apart() {
        let mut s = IndexedSurface::new();
        // A 4 × 2 block: the left half line, the right half skin, at (10, 6).
        s.paint(Rect::new(10, 6, 12, 8), &[1.0; 4], LINE);
        s.paint(Rect::new(12, 6, 14, 8), &[1.0; 4], SKIN);

        let double = s.resampled(2.0);
        // Twice the size, twice as far from the origin, plus the soft pixel interpolation puts
        // around a hard edge.
        assert_eq!(double.bounds(), Rect::new(19, 11, 29, 17));
        assert!((coverage(&double, 19, 13) - 0.25).abs() < 1e-3 && double.pixel(19, 13).swatch == LINE);
        assert_eq!(double.pixel(21, 13), IndexedPixel { swatch: LINE, coverage: 65535 });
        assert_eq!((double.pixel(23, 13).swatch, double.pixel(24, 13).swatch), (LINE, SKIN), "the swatches meet where they met");
        assert_eq!(double.pixel(24, 13).coverage, 65535, "and there is no gap between them");

        let half = s.resampled(0.5);
        assert_eq!(half.bounds(), Rect::new(5, 3, 7, 4));
        assert_eq!((half.pixel(5, 3), half.pixel(6, 3)), (IndexedPixel { swatch: LINE, coverage: 65535 }, IndexedPixel { swatch: SKIN, coverage: 65535 }));

        // A single pixel reduced to a quarter covers a quarter of the new pixel.
        let mut dot = IndexedSurface::new();
        dot.paint(Rect::new(0, 0, 1, 1), &[1.0], LINE);
        assert!((coverage(&dot.resampled(0.5), 0, 0) - 0.25).abs() < 1e-3);
        assert!(IndexedSurface::new().resampled(2.0).is_empty());
        assert_eq!(s.resampled(1.0), s, "the same size is the same pixels");
    }

    #[test]
    fn a_selection_can_be_lifted_cleared_and_stamped_back() {
        let mut s = IndexedSurface::new();
        s.paint(Rect::new(250, 0, 260, 4), &[1.0; 40], LINE);
        let lifted = s.extract(Rect::new(254, 1, 258, 3));
        assert_eq!(lifted.bounds(), Rect::new(254, 1, 258, 3), "only what is inside, across a tile edge");
        assert_eq!(s.bounds(), Rect::new(250, 0, 260, 4), "lifting copies");

        s.clear(Rect::new(254, 1, 258, 3));
        assert_eq!((s.pixel(254, 1), s.pixel(253, 1).coverage, s.pixel(254, 0).coverage), (IndexedPixel::default(), 65535, 65535));

        // Stamped back over other paint, each pixel keeps its swatch and wins where it is solid.
        s.paint(Rect::new(254, 1, 256, 2), &[0.5, 0.5], SKIN);
        s.stamp(&lifted);
        assert_eq!(s.pixel(254, 1), IndexedPixel { swatch: LINE, coverage: 65535 });
        assert_eq!(s.pixel(257, 2), IndexedPixel { swatch: LINE, coverage: 65535 });
        assert!(s.extract(Rect::new(1000, 1000, 1010, 1010)).is_empty());
    }

    #[test]
    fn a_transform_moves_scales_and_turns_pixels() {
        let mut s = IndexedSurface::new();
        s.paint(Rect::new(0, 0, 4, 2), &[1.0; 8], LINE);
        s.paint(Rect::new(4, 0, 8, 2), &[1.0; 8], SKIN);

        let moved = s.transformed(&Affine::translate(300.0, -7.0));
        assert_eq!(moved.bounds(), Rect::new(300, -7, 308, -5), "a whole-pixel move is exact");
        assert_eq!((moved.pixel(303, -7).swatch, moved.pixel(304, -7).swatch), (LINE, SKIN));
        assert_eq!(moved.transformed(&Affine::translate(-300.0, 7.0)), s);

        // A quarter turn clockwise about the origin: x → y, so the 8 × 2 block stands 2 × 8.
        let turned = s.transformed(&Affine::rotate(std::f64::consts::FRAC_PI_2));
        assert_eq!(turned.bounds(), Rect::new(-2, 0, 0, 8));
        assert_eq!((turned.pixel(-1, 1).swatch, turned.pixel(-1, 6).swatch), (LINE, SKIN));

        let doubled = s.transformed(&Affine::scale(2.0));
        assert_eq!(doubled.pixel(4, 2), IndexedPixel { swatch: LINE, coverage: 65535 });
        assert_eq!(doubled.pixel(12, 2).swatch, SKIN);
        assert!(s.transformed(&Affine { m: [0.0; 6] }).is_empty(), "a transform that can't be undone gives nothing");
    }

    #[test]
    fn rendering_looks_colours_up_in_the_palette() {
        let mut s = IndexedSurface::new();
        s.paint(Rect::new(0, 0, 2, 1), &[1.0, 0.5], LINE);
        let day = |_: SwatchId| [10, 20, 30, 255];
        let night = |_: SwatchId| [1, 2, 3, 255];
        assert_eq!(s.to_rgba8(Rect::new(-1, 0, 2, 1), day), [[0, 0, 0, 0], [10, 20, 30, 255], [10, 20, 30, 128]]);
        // The same pixels with another palette: nothing in the surface changed.
        assert_eq!(s.to_rgba8(Rect::new(0, 0, 1, 1), night), [[1, 2, 3, 255]]);
    }
}
