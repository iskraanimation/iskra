//! A vector line drawn by a picture's tip stamped along it, as Moho's brushes are (the user's
//! of 2026-11-23, after Moho 14's manual, 13.03). The line stays its middle line and its width:
//! only when its pixels are made is the tip stamped along it, as large as the line is wide
//! there, at even steps of its width, each stamp turned with the line, by chance and by a
//! drift. The tip paints in the line's swatch, as Moho's brush tinted by the stroke's colour.
//!
//! A tip is kept once, by its key, the hash of its pixels ([`remember_tip`]); a stroke keeps
//! only the key. The table of tips is the process's, and as a key names what it holds it never
//! changes what a key gives: the project file keeps the tips its strokes use and puts them back
//! in the table when it is read.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use kurbo::Point;
use photocraft_geom::{TILE_SIZE, TileCoord};
use photocraft_paint::{BrushContext, BrushSettings, Dab, GrayTile, TipShape};
use serde::{Deserialize, Serialize};

use crate::{Nib, Rect};

/// The largest side of a stamp's tip, in pixels: a larger picture is made smaller to fit.
/// A stroke is seldom wider, and the brush, which keeps the tip, goes with each stroke drawn.
pub const MAX_STAMP_TIP: u32 = 512;

/// How a vector brush draws its line with a tip stamped along it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StampBrush {
    pub enabled: bool,
    /// The tip: a picture's ([`TipShape::Sampled`]). A round one stamps nothing.
    pub tip: TipShape,
    /// How far apart the stamps are, in parts of the line's width there.
    pub spacing: f32,
    /// Each stamp turned with the line.
    pub align: bool,
    /// Each stamp turned by chance, up to this far either way, in degrees.
    pub jitter: f32,
    /// The stamps' angle wandering along the line, up to this far either way, in degrees.
    pub drift: f32,
    /// The stamps build up where they overlap; else the line is as dense as its densest stamp
    /// there (Moho's *Merge alpha*).
    pub build_up: bool,
}

impl Default for StampBrush {
    fn default() -> Self {
        Self { enabled: false, tip: TipShape::Round, spacing: 0.25, align: true, jitter: 0.0, drift: 0.0, build_up: false }
    }
}

impl StampBrush {
    /// Whether it stamps: it is on and its tip is a picture's.
    pub fn stamps(&self) -> bool {
        self.enabled && self.tip.bitmap().is_some_and(|tile| tile.width > 0 && tile.height > 0)
    }

    /// What a stroke drawn with it keeps, with chance of its own from `seed`; its tip is
    /// remembered by its key.
    pub fn stamp(&self, seed: u64) -> Option<Box<Stamp>> {
        if !self.stamps() {
            return None;
        }
        let tip = remember_tip(self.tip.bitmap()?);
        let finite = |value: f32, none: f32| f64::from(if value.is_finite() { value } else { none });
        Some(Box::new(Stamp {
            tip,
            spacing: finite(self.spacing, 0.25).clamp(MIN_SPACING, MAX_SPACING),
            align: self.align,
            jitter: finite(self.jitter, 0.0).clamp(0.0, 180.0),
            drift: finite(self.drift, 0.0).clamp(0.0, 180.0),
            build_up: self.build_up,
            seed,
        }))
    }
}

/// The least and the most the stamps are apart, in widths.
pub const MIN_SPACING: f64 = 0.02;
pub const MAX_SPACING: f64 = 4.0;

/// What a stroke drawn by a stamped tip keeps: its tip's key, and how it was stamped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stamp {
    /// The tip's key ([`tip_key`]).
    pub tip: String,
    /// How far apart the stamps are, in parts of the width.
    pub spacing: f64,
    #[serde(default)]
    pub align: bool,
    /// Degrees either way.
    #[serde(default)]
    pub jitter: f64,
    #[serde(default)]
    pub drift: f64,
    #[serde(default)]
    pub build_up: bool,
    /// What chance does to its stamps, the same each time it is drawn.
    #[serde(default)]
    pub seed: u64,
}

fn tips() -> &'static RwLock<HashMap<String, Arc<GrayTile>>> {
    static TIPS: OnceLock<RwLock<HashMap<String, Arc<GrayTile>>>> = OnceLock::new();
    TIPS.get_or_init(Default::default)
}

/// The key of a tip: a hash of its size and its pixels, the same for the same tip anywhere.
pub fn tip_key(tile: &GrayTile) -> String {
    // FNV-1a, 64 bits: stable from one build to the next, as the file keeps it.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&tile.width.to_le_bytes());
    eat(&tile.height.to_le_bytes());
    for value in tile.data.iter() {
        eat(&value.to_le_bytes());
    }
    format!("{hash:016x}")
}

/// Keep `tile` in the table of tips, if it is not there already, and give its key.
pub fn remember_tip(tile: &GrayTile) -> String {
    let key = tip_key(tile);
    remember_tip_as(&key, tile);
    key
}

/// Keep `tile` under `key`, as a file names it, if no tip is kept under it already.
pub fn remember_tip_as(key: &str, tile: &GrayTile) {
    let known = tips().read().is_ok_and(|tips| tips.contains_key(key));
    if !known && let Ok(mut tips) = tips().write() {
        tips.entry(key.to_string()).or_insert_with(|| Arc::new(tile.clone()));
    }
}

/// The tip kept under `key`, if one is.
pub fn tip(key: &str) -> Option<Arc<GrayTile>> {
    tips().read().ok().and_then(|tips| tips.get(key).cloned())
}

/// `tile` made no larger than [`MAX_STAMP_TIP`] a side, keeping its shape.
pub fn stamp_tip(tile: GrayTile) -> GrayTile {
    let side = tile.width.max(tile.height);
    if side <= MAX_STAMP_TIP || side == 0 {
        return tile;
    }
    let shrink = side as f32 / MAX_STAMP_TIP as f32;
    let (wide, high) = (((tile.width as f32 / shrink).round() as u32).max(1), ((tile.height as f32 / shrink).round() as u32).max(1));
    GrayTile::from_fn(wide, high, |x, y| {
        let (x0, y0) = ((x as f32 * shrink) as u32, (y as f32 * shrink) as u32);
        let (x1, y1) =
            ((((x + 1) as f32 * shrink).ceil() as u32).clamp(x0 + 1, tile.width), (((y + 1) as f32 * shrink).ceil() as u32).clamp(y0 + 1, tile.height));
        let (mut sum, mut count) = (0.0, 0.0);
        for py in y0.min(tile.height - 1)..y1 {
            for px in x0.min(tile.width - 1)..x1 {
                (sum, count) = (sum + tile.get(px, py), count + 1.0);
            }
        }
        if count > 0.0 { sum / count } else { 0.0 }
    })
}

/// A place along a line: where it is, how far along, the line's half width there, and its
/// opacity and colour drift.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Place {
    pub at: Point,
    pub along: f64,
    pub radius: f64,
    pub opacity: f64,
    pub drift: f64,
}

/// How much a stamped line covers each pixel, and how far its colour drifts there, by tile.
#[derive(Default)]
pub(crate) struct Laid {
    pub coverage: HashMap<TileCoord, Vec<f32>>,
    pub drift: HashMap<TileCoord, Vec<f32>>,
}

/// Lays a tip's stamps along a line, piece by piece as the line goes.
pub(crate) struct Stamper {
    context: BrushContext,
    stamp: Stamp,
    nib: Nib,
    /// How far along the line the next stamp goes.
    next: f64,
    index: u64,
    scratch: Vec<f32>,
}

impl Stamper {
    /// A stamper of `stamp`'s tip with the nib's angle and roundness, or `None` if its tip is
    /// not known.
    pub fn new(stamp: &Stamp, nib: Nib) -> Option<Stamper> {
        let tile = tip(&stamp.tip)?;
        let brush = BrushSettings { tip: TipShape::Sampled((*tile).clone()), size: 100.0, hardness: 1.0, ..BrushSettings::default() };
        Some(Stamper { context: BrushContext::new(&brush), stamp: stamp.clone(), nib, next: 0.0, index: 0, scratch: Vec::new() })
    }

    /// Stamp the line from `from` to `to`: each stamp whose place falls between them, the
    /// first at the line's start. Gives the pixels that changed.
    pub fn between(&mut self, from: &Place, to: &Place, laid: &mut Laid) -> Rect {
        let mut changed = Rect::EMPTY;
        let long = to.along - from.along;
        let way = to.at - from.at;
        // A turn with the line, measured as the tip's angle is: against the clock, y up.
        let heading = if way.hypot2() > 1e-12 { (-way.y).atan2(way.x) } else { 0.0 };
        while self.next <= to.along + 1e-9 {
            let t = if long > 1e-9 { ((self.next - from.along) / long).clamp(0.0, 1.0) } else { 1.0 };
            let lerp = |a: f64, b: f64| a + (b - a) * t;
            let place = Place {
                at: from.at.lerp(to.at, t),
                along: self.next,
                radius: lerp(from.radius, to.radius),
                opacity: lerp(from.opacity, to.opacity),
                drift: lerp(from.drift, to.drift),
            };
            changed = changed.union(&self.lay(&place, heading, laid));
            self.next += (self.stamp.spacing * place.radius * 2.0).max(0.5);
            self.index += 1;
        }
        changed
    }

    /// Lay one stamp at `place`, the line going `heading` there.
    fn lay(&mut self, place: &Place, heading: f64, laid: &mut Laid) -> Rect {
        if place.radius <= 0.05 || place.opacity <= 0.0 {
            return Rect::EMPTY;
        }
        let stamp = &self.stamp;
        let chance = unit(stamp.seed, self.index) * 2.0 - 1.0;
        // Wandering over about eight stamps either way.
        let wander = smooth(stamp.seed ^ 0xD81F_7A3C_5E21_9B47, self.index as f64 / 8.0);
        let mut angle = -self.nib.angle.to_radians() + chance * stamp.jitter.to_radians() + wander * stamp.drift.to_radians();
        if stamp.align {
            angle += heading;
        }
        let dab = Dab {
            center: photocraft_geom::Point { x: place.at.x, y: place.at.y },
            radius: place.radius as f32,
            alpha: 1.0,
            angle: angle as f32,
            roundness: self.nib.roundness.clamp(0.01, 1.0) as f32,
            flip_x: false,
            flip_y: false,
            opacity: 1.0,
            color: [0.0; 4],
            depth: 1.0,
            index: self.index,
            proj_angle: 0.0,
            proj_scale: 1.0,
            wet: 1.0,
            mix: 1.0,
        };
        let rect = self.context.dab_rect(&dab, false);
        if rect.is_empty() {
            return rect;
        }
        self.context.rasterize(&dab, false, rect, &mut self.scratch);
        let (opacity, drift) = (place.opacity.clamp(0.0, 1.0) as f32, place.drift.clamp(0.0, 1.0) as f32);
        let width = rect.width() as usize;
        for (row, y) in (rect.y0..rect.y1).enumerate() {
            for (column, x) in (rect.x0..rect.x1).enumerate() {
                let value = self.scratch[row * width + column] * opacity;
                if value <= 0.0 {
                    continue;
                }
                let coord = TileCoord::new(x.div_euclid(TILE_SIZE), y.div_euclid(TILE_SIZE));
                let at = (y.rem_euclid(TILE_SIZE) * TILE_SIZE + x.rem_euclid(TILE_SIZE)) as usize;
                let tile = laid.coverage.entry(coord).or_insert_with(|| vec![0.0; (TILE_SIZE * TILE_SIZE) as usize]);
                tile[at] = if stamp.build_up { tile[at] + value * (1.0 - tile[at]) } else { tile[at].max(value) };
                if drift > 0.0 {
                    let tile = laid.drift.entry(coord).or_insert_with(|| vec![0.0; (TILE_SIZE * TILE_SIZE) as usize]);
                    tile[at] = tile[at].max(drift);
                }
            }
        }
        rect
    }
}

/// A number from 0 to 1 for `index`, the same for the same `seed`.
fn unit(seed: u64, index: u64) -> f64 {
    let mut h = seed ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// A number from -1 to 1 going smoothly along `t`, a new one at each whole `t`.
fn smooth(seed: u64, t: f64) -> f64 {
    let (whole, part) = (t.floor(), t - t.floor());
    let at = |i: f64| unit(seed, i as i64 as u64) * 2.0 - 1.0;
    let ease = part * part * (3.0 - 2.0 * part);
    at(whole) + (at(whole + 1.0) - at(whole)) * ease
}

/// A tip made of a PNG picture's bytes, as [`crate::tile_of_picture`] makes one (what is dark
/// and solid paints), no larger than [`MAX_STAMP_TIP`]; `None` if it can't be read.
pub fn tip_of_png(bytes: &[u8]) -> Option<GrayTile> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    let pixels = &buffer[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => pixels.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    crate::tile_of_picture(info.width, info.height, &rgba, true).map(stamp_tip)
}

/// The tips Iskra comes with, by name: pictures the user gave (2026-11-23).
pub fn builtin_tip(name: &str) -> Option<GrayTile> {
    let bytes: &[u8] = match name {
        "blob-01" => include_bytes!("../assets/tips/blob-01.png"),
        "blob-02" => include_bytes!("../assets/tips/blob-02.png"),
        "blob-03" => include_bytes!("../assets/tips/blob-03.png"),
        "blob-04" => include_bytes!("../assets/tips/blob-04.png"),
        _ => return None,
    };
    tip_of_png(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Brush, IndexedSurface, LiveVector, ShapeId, ShapeKind, SwatchId, VectorArt, VectorBrush, VectorShape, VectorStroke, vector_stroke};
    use photocraft_paint::StrokePoint;

    fn preset(name: &str) -> VectorBrush {
        let preset = crate::builtin_brushes().iter().find(|preset| preset.name == name).expect(name);
        match &preset.brush {
            Brush::Vector(brush) => brush.clone(),
            Brush::Raster(_) => panic!("{name} is a vector brush"),
        }
    }

    /// A wavy line from left to right, pressed evenly.
    fn wave() -> Vec<StrokePoint> {
        (0..=60).map(|i| StrokePoint::new(30.0 + f64::from(i) * 6.0, 100.0 + 25.0 * (f64::from(i) / 9.0).sin(), 1.0)).collect()
    }

    fn drawn(stroke: VectorStroke) -> IndexedSurface {
        VectorArt::new(vec![VectorShape { id: ShapeId(1), swatch: SwatchId(3), kind: ShapeKind::Stroke(stroke) }]).raster().clone()
    }

    fn coverage(surface: &IndexedSurface) -> Vec<f32> {
        surface.pixels(Rect::new(0, 0, 460, 200)).iter().map(|pixel| f32::from(pixel.coverage) / 65535.0).collect()
    }

    #[test]
    fn a_tip_is_kept_once_by_the_hash_of_its_pixels() {
        let tile = GrayTile::from_fn(5, 3, |x, y| (x + y) as f32 / 6.0);
        let key = remember_tip(&tile);
        assert_eq!(key, tip_key(&tile.clone()), "the same pixels, the same key");
        assert_eq!(*tip(&key).unwrap(), tile);
        assert_ne!(tip_key(&GrayTile::from_fn(3, 5, |x, y| (x + y) as f32 / 6.0)), key, "another size, another key");
        // A file's tip under its own key stays as it was given first.
        remember_tip_as("from-a-file", &tile);
        remember_tip_as("from-a-file", &GrayTile::from_fn(1, 1, |_, _| 1.0));
        assert_eq!(*tip("from-a-file").unwrap(), tile);
        assert!(tip("never-kept").is_none());
    }

    #[test]
    fn the_engines_stamped_brushes_have_the_users_pictures_as_tips() {
        for name in ["Splatter", "Crayon Hatch", "Dry Roller", "Ink Blots"] {
            let brush = preset(name);
            assert!(brush.stamp.stamps(), "{name} stamps a picture");
            let tile = brush.stamp.tip.bitmap().unwrap();
            assert!(tile.width <= MAX_STAMP_TIP && tile.height <= MAX_STAMP_TIP);
            assert!(brush.grain().is_none(), "{name}: a stamped tip takes the grain's place");
        }
        assert!(crate::stamp::builtin_tip("none").is_none());
    }

    #[test]
    fn a_stamped_line_is_drawn_by_its_stamps_the_same_each_time() {
        let brush = VectorBrush { seed: 11, ..preset("Splatter") };
        let stroke = vector_stroke(&wave(), &brush).unwrap();
        let stamp = stroke.stamp.clone().expect("the stroke keeps its stamp");
        assert_eq!((stamp.seed, stamp.jitter), (11, 180.0));
        let stamped = coverage(&drawn(stroke.clone()));
        let plain = coverage(&drawn(VectorStroke { stamp: None, ..stroke.clone() }));
        let painted = |c: &[f32]| c.iter().filter(|c| **c > 0.0).count();
        let partial = stamped.iter().filter(|c| **c > 0.05 && **c < 0.95).count();
        // The picture's grain: much of what it paints is partly painted, where the plain line
        // is solid but at its edge.
        assert!(partial * 3 > painted(&stamped), "{partial} partly painted of {}", painted(&stamped));
        assert!(painted(&stamped) > painted(&plain) / 3, "it paints along the line");
        assert_eq!(stamped, coverage(&drawn(stroke.clone())), "the same each time it is drawn");
        // Another seed, other stamps.
        let other = VectorStroke { stamp: Some(Box::new(Stamp { seed: 12, ..*stamp.clone() })), ..stroke.clone() };
        assert_ne!(stamped, coverage(&drawn(other)));
        // Built up, never less than merged.
        let built = coverage(&drawn(VectorStroke { stamp: Some(Box::new(Stamp { build_up: true, ..*stamp.clone() })), ..stroke.clone() }));
        assert!(built.iter().zip(&stamped).all(|(b, m)| *b >= *m - 1e-3) && built.iter().sum::<f32>() > stamped.iter().sum::<f32>());
        // A tip that is not known draws the plain line.
        let unknown = VectorStroke { stamp: Some(Box::new(Stamp { tip: "never-kept".to_string(), ..*stamp })), ..stroke };
        assert_eq!(coverage(&drawn(unknown)), plain);
    }

    #[test]
    fn a_faint_stamped_line_is_fainter_and_drifts() {
        let brush = VectorBrush { seed: 3, ..preset("Dry Roller") };
        let stroke = vector_stroke(&wave(), &brush).unwrap();
        let faint = VectorStroke { opacities: vec![0.4; stroke.widths.len()], drifts: vec![1.0; stroke.widths.len()], ..stroke.clone() };
        let (full, half) = (coverage(&drawn(stroke)), coverage(&drawn(faint.clone())));
        assert!(half.iter().cloned().fold(0.0, f32::max) <= 0.41 && half.iter().sum::<f32>() > 0.0);
        assert!(full.iter().sum::<f32>() > half.iter().sum::<f32>() * 2.0);
        let drifted = drawn(faint).pixels(Rect::new(0, 0, 460, 200)).iter().filter(|pixel| pixel.coverage > 0).all(|pixel| pixel.swatch != SwatchId(3));
        assert!(drifted, "its colour drifts where it is drawn");
    }

    #[test]
    fn a_surface_of_a_stamped_brush_is_plain_and_a_boil_stamps_anew() {
        let brush = VectorBrush { seed: 5, boil: 2.0, ..preset("Ink Blots") };
        let stroke = vector_stroke(&wave(), &brush).unwrap();
        let crate::ShapeKind::Fill(fill) = crate::surface_shape(&stroke, &brush) else { panic!("a fill") };
        assert!(fill.tracks.iter().all(|track| track.stamp.is_none()));
        let art = VectorArt::new(vec![VectorShape { id: ShapeId(1), swatch: SwatchId(3), kind: ShapeKind::Stroke(stroke) }]);
        let turned = art.boiled(brush.boil_every).expect("a turn of its boil");
        let ShapeKind::Stroke(boiled) = &turned.shapes()[0].kind else { panic!("a stroke") };
        assert_ne!(boiled.stamp.as_ref().unwrap().seed, 5, "its chance laid anew");
    }

    #[test]
    fn a_stamped_line_is_seen_stamped_as_it_is_drawn() {
        let brush = VectorBrush { seed: 2, smoothing: 0.0, ..preset("Crayon Hatch") };
        let mut live = LiveVector::new(&IndexedSurface::new(), &brush, SwatchId(3));
        assert!(!live.push(&wave()).is_empty());
        let seen = coverage(live.layer());
        let partial = seen.iter().filter(|c| **c > 0.05 && **c < 0.95).count();
        let painted = seen.iter().filter(|c| **c > 0.0).count();
        assert!(painted > 1000 && partial * 4 > painted, "{partial} of {painted}");
    }

    #[test]
    fn a_stamped_lines_stamps_are_kept_until_it_changes() {
        let stroke = vector_stroke(&wave(), &VectorBrush { seed: 21, ..preset("Ink Blots") }).unwrap();
        let first = kept(&stroke, || stroke.stamped()).unwrap();
        let again = kept(&stroke.clone(), || panic!("kept, not made again")).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        let moved = VectorStroke { widths: stroke.widths.iter().map(|w| w + 1.0).collect(), ..stroke.clone() };
        let mut made = false;
        let other = kept(&moved, || {
            made = true;
            moved.stamped()
        });
        assert!(made && other.is_some_and(|other| !Arc::ptr_eq(&other, &first)), "a changed line is stamped anew");
        // Drawn from what was kept, as it was drawn when made.
        assert_eq!(coverage(&drawn(stroke.clone())), coverage(&drawn(stroke)));
    }

    #[test]
    fn a_brush_without_a_stamped_tip_writes_none() {
        let json = serde_json::to_value(VectorBrush::default()).unwrap();
        assert!(json.get("stamp").is_none());
        let stroke = vector_stroke(&wave(), &VectorBrush::default()).unwrap();
        assert!(serde_json::to_value(&stroke).unwrap().get("stamp").is_none());
        let stamped = vector_stroke(&wave(), &preset("Splatter")).unwrap();
        let back: VectorStroke = serde_json::from_value(serde_json::to_value(&stamped).unwrap()).unwrap();
        assert_eq!(back, stamped);
    }
}

/// A stamped line's stamps as they are kept between one making of its art layer's pixels and
/// the next, by tile: how much they cover each pixel (0..=65535) and, where its colour drifts,
/// how far (0..=255).
pub(crate) struct Kept {
    pub tiles: Vec<(TileCoord, Box<[u16]>, Option<Box<[u8]>>)>,
}

impl Kept {
    fn of(laid: Laid) -> Kept {
        let mut drift = laid.drift;
        let tiles = laid
            .coverage
            .into_iter()
            .map(|(coord, coverage)| {
                let coverage: Box<[u16]> = coverage.iter().map(|c| (c.clamp(0.0, 1.0) * 65535.0).round() as u16).collect();
                let drift = drift.remove(&coord).map(|drift| drift.iter().map(|d| (d.clamp(0.0, 1.0) * 255.0).round() as u8).collect());
                (coord, coverage, drift)
            })
            .collect();
        Kept { tiles }
    }

    fn bytes(&self) -> usize {
        self.tiles.iter().map(|(_, coverage, drift)| coverage.len() * 2 + drift.as_ref().map_or(0, |drift| drift.len()) + 64).sum()
    }
}

/// The most the kept stamps may take, in bytes: past it the longest unused go.
const KEPT_BYTES: usize = 256 << 20;

#[derive(Default)]
struct KeptStamps {
    by_stroke: HashMap<u64, Arc<Kept>>,
    /// The strokes' hashes, the least lately used first.
    order: std::collections::VecDeque<u64>,
    bytes: usize,
}

fn kept_stamps() -> &'static std::sync::Mutex<KeptStamps> {
    static KEPT: OnceLock<std::sync::Mutex<KeptStamps>> = OnceLock::new();
    KEPT.get_or_init(Default::default)
}

/// The stamps of `stroke`, kept from the last time its pixels were made if it has not changed
/// since (the user's of 2026-11-23: each edit of a drawing makes all its shapes' pixels anew,
/// and stamping them all again made a grab with Select wait a second); else made by `make` and
/// kept. `None` where `make` gives none.
pub(crate) fn kept(stroke: &crate::VectorStroke, make: impl FnOnce() -> Option<Laid>) -> Option<Arc<Kept>> {
    use std::hash::{Hash, Hasher};
    // What it is: all that draws it, its tip's key and its chance with it.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(stroke).ok()?.hash(&mut hasher);
    let key = hasher.finish();
    if let Ok(mut kept) = kept_stamps().lock()
        && let Some(found) = kept.by_stroke.get(&key).cloned()
    {
        if let Some(at) = kept.order.iter().position(|k| *k == key) {
            kept.order.remove(at);
        }
        kept.order.push_back(key);
        return Some(found);
    }
    let made = Arc::new(Kept::of(make()?));
    if let Ok(mut kept) = kept_stamps().lock() {
        let bytes = made.bytes();
        if bytes <= KEPT_BYTES && kept.by_stroke.insert(key, made.clone()).is_none() {
            kept.order.push_back(key);
            kept.bytes += bytes;
        }
        while kept.bytes > KEPT_BYTES {
            let Some(oldest) = kept.order.pop_front() else { break };
            if let Some(gone) = kept.by_stroke.remove(&oldest) {
                kept.bytes -= gone.bytes();
            }
        }
    }
    Some(made)
}
