//! Vector art (plan §4.16): an art layer of shapes instead of pixels.
//!
//! A [`VectorArt`] is a list of [`VectorShape`]s, bottom first, each one swatch whole: a stroke, kept
//! as its middle line and its width along it, or a fill, kept as its outline. Points are in the
//! drawing's pixels, as bitmap art's are, so whatever moves a drawing's pixels moves its points.
//!
//! It is drawn as indexed pixels: [`VectorArt::raster`] is the shapes turned into a swatch and a
//! coverage a pixel, made when the shapes are, and from there every compositor takes vector art
//! as it takes indexed art.

use std::collections::BTreeSet;
use std::sync::Arc;

use kurbo::{BezPath, Ellipse, ParamCurve, ParamCurveArclen, PathEl, PathSeg, Shape as _, Vec2};
use serde::{Deserialize, Serialize};

use crate::{IndexedSurface, Rect, SwatchId};

/// A shape's id within its art layer. It stays while the shape is edited, so that a fill can
/// name the strokes that bound it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ShapeId(pub u32);

/// One shape of vector art, painted with one swatch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorShape {
    pub id: ShapeId,
    pub swatch: SwatchId,
    #[serde(flatten)]
    pub kind: ShapeKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "shape")]
pub enum ShapeKind {
    Stroke(VectorStroke),
    Fill(VectorFill),
}

/// A line drawn with a vector brush. Its outline is made from its middle line when it is drawn
/// ([`VectorStroke::outline`]), never stored, so moving the middle line's points moves the
/// stroke and keeps its width.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorStroke {
    /// The middle line: one open path.
    #[serde(with = "svg_path")]
    pub path: BezPath,
    /// The width at each of the path's points, in order (one for each element of the path that
    /// ends at a point); between two points it goes evenly from one to the other.
    pub widths: Vec<f64>,
    /// The nib it is drawn with: round, or an oval at an angle, which a calligraphic pen has.
    #[serde(default, skip_serializing_if = "Nib::is_round")]
    pub nib: Nib,
    /// How its start and its end are finished: round, as the nib has them, unless a cut gave
    /// them another.
    #[serde(default, skip_serializing_if = "Cap::both_round")]
    pub caps: [Cap; 2],
    /// A paper texture and a second tip that cut what it covers, as the pixel brush has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grain: Option<Box<Grain>>,
    /// How opaque it is at each of the path's points (0..1), as the widths are kept, going
    /// evenly from one to the next (the user's of 2026-11-04): a light touch leaves it faint.
    /// Empty where it is opaque all along.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opacities: Vec<f64>,
    /// How far its colour has drifted at each of the path's points (0..1), towards its swatch's
    /// drift colour, as the widths are kept. Empty where it has not drifted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drifts: Vec<f64>,
    /// How it boils, drawn anew from frame to frame; `None` where it is still.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boil: Option<Boil>,
}

/// How a shape boils (the user's of 2026-11-04), as hand-drawn lines shimmer from one drawing
/// of them to the next: every `every` frames it is drawn the next of [`Boil::TURNS`] ways in
/// turn, the first as it is kept, the others with its edge moved by up to `amount` pixels and
/// its grain laid anew.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Boil {
    pub amount: f64,
    pub every: u32,
}

impl Boil {
    /// How many ways a boiling shape is drawn in turn.
    pub const TURNS: u32 = 3;

    /// Which of its ways it is drawn on `frame`: 0 as it is kept.
    pub fn turn(&self, frame: u32) -> u32 {
        (frame / self.every.max(1)) % Self::TURNS
    }
}

/// What cuts a vector stroke's coverage as the pixel brush's texture and dual brush cut its
/// paint: the texture over the whole stroke, and a second tip stamped along its middle line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Grain {
    pub texture: photocraft_paint::Texture,
    pub dual_brush: photocraft_paint::DualBrush,
    /// What chance does to the second tip, the same each time the stroke is drawn.
    pub seed: u64,
}

impl Grain {
    /// The coverage the grain leaves of a stroke whose middle line is sampled as `samples`, each
    /// a point and the radius there: a renderer of a hard tip wide enough to cover the whole
    /// width, with the texture and the second tip, to read the coverage of.
    fn mask(&self, samples: &[(kurbo::Point, f64)]) -> photocraft_paint::StrokeRenderer {
        let widest = samples.iter().map(|(_, radius)| *radius).fold(0.0, f64::max);
        let brush = photocraft_paint::BrushSettings {
            size: (widest * 2.0 + 4.0) as f32,
            hardness: 1.0,
            pressure_size: false,
            pressure_opacity: false,
            texture: self.texture.clone(),
            dual_brush: self.dual_brush.clone(),
            seed: self.seed,
            ..photocraft_paint::BrushSettings::default()
        };
        let mut renderer = photocraft_paint::StrokeRenderer::new(&brush, None, 1.0);
        let points: Vec<photocraft_paint::StrokePoint> = samples.iter().map(|(at, _)| photocraft_paint::StrokePoint::new(at.x, at.y, 1.0)).collect();
        renderer.push(&points);
        renderer.finish();
        renderer
    }
}

/// How the end of a stroke is finished.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Cap {
    /// Round, as the nib is.
    #[default]
    Round,
    /// Square across, at the end of the middle line.
    Straight,
    /// Square across, past the end by half the width, which the nib would reach.
    Miter,
}

impl Cap {
    fn both_round(caps: &[Cap; 2]) -> bool {
        *caps == [Cap::Round; 2]
    }
}

/// The shape of a vector stroke's nib: an oval as wide as the stroke's width, `roundness`
/// times that across, its width turned `angle` degrees from the page's x axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Nib {
    pub angle: f64,
    pub roundness: f64,
}

impl Default for Nib {
    fn default() -> Self {
        Nib { angle: 0.0, roundness: 1.0 }
    }
}

impl Nib {
    pub fn is_round(&self) -> bool {
        self.roundness >= 1.0
    }

    /// The oval's axes as unit vectors, and its half-widths along them for a nib of `radius`.
    fn axes(&self, radius: f64) -> (Vec2, Vec2, f64, f64) {
        let (sin, cos) = self.angle.to_radians().sin_cos();
        (Vec2::new(cos, sin), Vec2::new(-sin, cos), radius, radius * self.roundness.clamp(0.01, 1.0))
    }

    /// The point of the oval of `radius` furthest out in the direction `towards` (a unit
    /// vector), from its middle: where a line along the stroke touches it.
    fn reach(&self, radius: f64, towards: Vec2) -> Vec2 {
        let (u, v, a, b) = self.axes(radius);
        let (du, dv) = (u.dot(towards), v.dot(towards));
        let across = ((a * du).powi(2) + (b * dv).powi(2)).sqrt();
        if across <= 1e-12 { Vec2::ZERO } else { (u * (a * a * du) + v * (b * b * dv)) / across }
    }
}

/// A closed area painted with a swatch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorFill {
    /// Its outline, filled by the non-zero rule.
    #[serde(with = "svg_path")]
    pub outline: BezPath,
    /// A point inside it, where its region is found again when the strokes round it change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<[f64; 2]>,
    /// The strokes that bound it, on the drawing's art layers of vector art.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bound_by: Vec<Bound>,
    /// The gaps in its lines that were closed when it was made, in pixels: closed again when it
    /// is found again.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gap: f64,
    /// Its lines no longer close round it: it keeps the outline it had.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    /// A surface's grain (the user's of 2026-11-04): the strokes it was drawn from, each with
    /// its paper texture and second tip, which cut its coverage along them as they cut a
    /// line's. None for a fill of no grain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<VectorStroke>,
    /// How it boils, if it was drawn as a surface by a brush that boils.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boil: Option<Boil>,
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

/// A stroke that bounds a fill: which of the drawing's art layers it is on, and its id there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bound {
    pub art: crate::ArtLayerKind,
    pub shape: ShapeId,
}

impl VectorFill {
    /// A fill of `outline` that follows no lines.
    pub fn plain(outline: BezPath) -> VectorFill {
        VectorFill { outline, inside: None, bound_by: Vec::new(), gap: 0.0, open: false, tracks: Vec::new(), boil: None }
    }

    /// Whether it has a grain that cuts its coverage.
    pub fn grained(&self) -> bool {
        self.tracks.iter().any(VectorStroke::grained)
    }

    /// Whether it was found between lines, by the paint bucket, and follows them: it has a
    /// point inside or lines that bound it. A fill of its own outline (merged, flattened, or a
    /// shape's) has neither, and keeps its outline whatever is drawn over it.
    pub fn between_lines(&self) -> bool {
        self.inside.is_some() || !self.bound_by.is_empty()
    }
}

/// How rough the edge of a surface is (the user's of 2026-11-04): how far it goes in and out,
/// as a part of the stroke's half width there, how long a bump is along it in pixels, and the
/// chance the bumps are laid out by.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rough {
    pub amount: f64,
    pub bump: f64,
    pub seed: u64,
}

/// `samples` with more put between them, none further apart than `step`, the radius going
/// evenly between them. At most some tens of thousands.
pub(crate) fn denser(samples: &[(kurbo::Point, f64)], step: f64) -> Vec<(kurbo::Point, f64)> {
    let total: f64 = samples.windows(2).map(|pair| pair[0].0.distance(pair[1].0)).sum();
    let step = step.max(total / 40_000.0);
    let mut out = Vec::with_capacity(samples.len());
    out.extend(samples.first().copied());
    for pair in samples.windows(2) {
        let ((from, a), (to, b)) = (pair[0], pair[1]);
        let count = (from.distance(to) / step).ceil().max(1.0) as usize;
        for n in 1..=count {
            let t = n as f64 / count as f64;
            out.push((from.lerp(to, t), a + (b - a) * t));
        }
    }
    out
}

/// A number from -1 to 1 at each place on a plane, going smoothly from one to the next a unit
/// apart: the same for the same place and `seed`.
pub(crate) fn noise2(x: f64, y: f64, seed: u64) -> f64 {
    let hashed = |x: i64, y: i64| {
        let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed;
        h ^= h >> 30;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 27;
        h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
        h ^= h >> 31;
        (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let (cx, cy) = (x.floor(), y.floor());
    let ease = |t: f64| t * t * (3.0 - 2.0 * t);
    let (tx, ty) = (ease(x - cx), ease(y - cy));
    let (cx, cy) = (cx as i64, cy as i64);
    let top = hashed(cx, cy) + (hashed(cx + 1, cy) - hashed(cx, cy)) * tx;
    let bottom = hashed(cx, cy + 1) + (hashed(cx + 1, cy + 1) - hashed(cx, cy + 1)) * tx;
    top + (bottom - top) * ty
}

/// A number from -1 to 1 at each place along a line, going smoothly from one to the next a
/// unit apart, with a finer wobble on it: the same for the same place and `seed`.
pub(crate) fn noise(at: f64, seed: u64) -> f64 {
    fn hashed(at: i64, seed: u64) -> f64 {
        let mut x = (at as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed;
        x ^= x >> 30;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 31;
        (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
    fn smooth(at: f64, seed: u64) -> f64 {
        let (whole, part) = (at.floor(), at - at.floor());
        let eased = part * part * (3.0 - 2.0 * part);
        let (a, b) = (hashed(whole as i64, seed), hashed(whole as i64 + 1, seed));
        a + (b - a) * eased
    }
    0.65 * smooth(at, seed) + 0.35 * smooth(at * 2.7 + 17.0, seed ^ 0x5555_5555)
}

impl VectorStroke {
    /// Whether it has a grain that cuts its coverage: a texture or a second tip that is on.
    pub fn grained(&self) -> bool {
        self.grain.as_ref().is_some_and(|grain| grain.texture.enabled || grain.dual_brush.enabled)
    }

    /// Whether its tone changes along it (the user's of 2026-11-04): it is fainter than opaque
    /// somewhere, or its colour has drifted somewhere.
    pub fn toned(&self) -> bool {
        self.opacities.iter().any(|opacity| *opacity < 1.0 - 1e-6) || self.drifts.iter().any(|drift| *drift > 1e-6)
    }

    /// Whether a surface drawn with it keeps it as a track: for its grain or its tone.
    pub fn marks(&self) -> bool {
        self.grained() || self.toned()
    }

    /// Its per-point lists, the widths, the opacities and the drifts, for what changes its
    /// points to change all three alike. Empty lists stay empty.
    pub fn per_point_mut(&mut self) -> [&mut Vec<f64>; 3] {
        [&mut self.widths, &mut self.opacities, &mut self.drifts]
    }

    /// The opacity and the drift at each place [`VectorStroke::samples`] gives, going evenly
    /// from point to point as the width does.
    pub(crate) fn tones(&self) -> Vec<[f64; 2]> {
        let value = |values: &[f64], at: usize, none: f64| values.get(at).or(values.last()).copied().unwrap_or(none);
        let tone = |at: usize| [value(&self.opacities, at, 1.0).clamp(0.0, 1.0), value(&self.drifts, at, 0.0).clamp(0.0, 1.0)];
        self.sampled(|index, t| {
            let (from, to) = (tone(index), tone(index + 1));
            [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t]
        })
    }

    /// The stroke's opacity and drift at the pixels of `rect`, in rows: at each, the most of
    /// those of the places along its middle line whose nib, `reach` times as wide, comes within
    /// a pixel and a half of it, and 0 where none does. `None` for what does not change along
    /// it, the opacity being full and the drift none.
    pub(crate) fn tone_in(&self, rect: Rect, reach: f64) -> [Option<Vec<f32>>; 2] {
        let wanted = [self.opacities.iter().any(|o| *o < 1.0 - 1e-6), self.drifts.iter().any(|d| *d > 1e-6)];
        if !wanted.contains(&true) || rect.is_empty() {
            return [None, None];
        }
        let (samples, tones) = (self.samples(), self.tones());
        let (width, height) = (rect.width() as usize, rect.height() as usize);
        let mut fields = [vec![0.0f32; width * height], vec![0.0f32; width * height]];
        let pairs: Vec<[usize; 2]> = if samples.len() == 1 { vec![[0, 0]] } else { (1..samples.len()).map(|at| [at - 1, at]).collect() };
        for [i, j] in pairs {
            let ((a, ra), (b, rb)) = (samples[i], samples[j]);
            let out = ra.max(rb) * reach + 1.5;
            let x0 = ((a.x.min(b.x) - out).floor() as i32).max(rect.x0);
            let x1 = ((a.x.max(b.x) + out).ceil() as i32).min(rect.x1);
            let y0 = ((a.y.min(b.y) - out).floor() as i32).max(rect.y0);
            let y1 = ((a.y.max(b.y) + out).ceil() as i32).min(rect.y1);
            let way = b - a;
            let long = way.hypot2();
            for y in y0..y1 {
                for x in x0..x1 {
                    let at = kurbo::Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5);
                    let t = if long > 1e-12 { ((at - a).dot(way) / long).clamp(0.0, 1.0) } else { 0.0 };
                    let near = a + way * t;
                    if at.distance(near) > (ra + (rb - ra) * t) * reach + 1.5 {
                        continue;
                    }
                    let pixel = (y - rect.y0) as usize * width + (x - rect.x0) as usize;
                    for (part, field) in fields.iter_mut().enumerate() {
                        let value = (tones[i][part] + (tones[j][part] - tones[i][part]) * t) as f32;
                        field[pixel] = field[pixel].max(value);
                    }
                }
            }
        }
        let [opacity, drift] = fields;
        [wanted[0].then_some(opacity), wanted[1].then_some(drift)]
    }

    /// A stroke of `width` all along `path`.
    pub fn even(path: BezPath, width: f64) -> VectorStroke {
        let points = path.elements().iter().filter(|el| !matches!(el, PathEl::ClosePath)).count();
        VectorStroke { path, widths: vec![width; points], nib: Nib::default(), caps: Default::default(), grain: None, ..Default::default() }
    }

    /// The area the stroke covers, as a path to fill by the non-zero rule: its nib swept along
    /// the middle line, its width going from point to point. It is made of the nib at every
    /// point it is sampled at and a quadrilateral between each two, from where a line along
    /// the stroke touches one nib to where it touches the next; all turn the same way, so where
    /// they overlap they add up and never cancel.
    pub fn outline(&self) -> BezPath {
        let samples = self.samples();
        let mut out = BezPath::new();
        let angle = self.nib.angle.to_radians();
        for (at, radius) in &samples {
            if *radius > 0.0 {
                let (_, _, a, b) = self.nib.axes(*radius);
                out.extend(Ellipse::new(*at, (a, b), angle).path_elements(0.05));
            }
        }
        // The way kurbo's ellipses turn, which every quadrilateral is made to turn too.
        let turn = |corners: &[kurbo::Point; 4]| (0..4).map(|i| corners[i].to_vec2().cross(corners[(i + 1) % 4].to_vec2())).sum::<f64>();
        let ellipses_turn = Ellipse::new((0.0, 0.0), (1.0, 0.5), 0.0).to_path(0.1).area() > 0.0;
        for pair in samples.windows(2) {
            let [(a, ra), (b, rb)] = [pair[0], pair[1]];
            let along = b - a;
            let length = along.hypot();
            if length <= 1e-9 {
                continue;
            }
            let normal = Vec2::new(-along.y, along.x) / length;
            let (reach_a, reach_b) = (self.nib.reach(ra, normal), self.nib.reach(rb, normal));
            let mut corners = [a - reach_a, b - reach_b, b + reach_b, a + reach_a];
            if (turn(&corners) > 0.0) != ellipses_turn {
                corners.reverse();
            }
            out.move_to(corners[0]);
            corners[1..].iter().for_each(|corner| out.line_to(*corner));
            out.close_path();
        }
        out
    }

    /// The stroke's area as one closed line, to draw and for path booleans: its edge along one side, round
    /// its end, back along the other side and round its start. Where the middle line turns
    /// sharply the outer edge goes round the nib; the loops the inner edge makes there are
    /// filled by the non-zero rule. [`VectorStroke::outline`] is the same area made of many
    /// pieces, much slower for a boolean and drawn a little too full at its edges.
    pub fn contour(&self) -> BezPath {
        self.edge(None)
    }

    /// [`VectorStroke::contour`] with one point at each sample along the sides, facing between
    /// the chords either side, and the nib gone round only where the line turns sharply: an
    /// edge smooth enough to fit with a few curves, for a surface.
    /// With `rough`, its edge goes in and out along the sides as a rough brush's does.
    pub(crate) fn smooth_contour(&self, rough: Rough) -> BezPath {
        self.edge(Some(rough))
    }

    fn edge(&self, smooth: Option<Rough>) -> BezPath {
        let mut samples = self.samples();
        // Rough, the samples close enough together for the bumps to show, and how far along
        // each one is, which the bumps are laid out by.
        let rough = smooth.filter(|rough| rough.amount > 0.0 && rough.bump > 0.0);
        if let Some(rough) = rough {
            samples = denser(&samples, (rough.bump / 4.0).max(0.5));
        }
        let mut along = vec![0.0];
        for pair in samples.windows(2) {
            along.push(along[along.len() - 1] + pair[0].0.distance(pair[1].0));
        }
        let length = along[along.len() - 1];
        // How much wider or narrower each side is at a place along it: the way back along the
        // second side goes on from where the first ended, round the end.
        let scale = |at: f64| rough.map_or(1.0, |rough| 1.0 + rough.amount * noise(at / rough.bump, rough.seed));
        let forward: Vec<f64> = along.iter().map(|at| scale(*at)).collect();
        let back: Vec<f64> = along.iter().map(|at| scale(2.0 * length - at)).collect();
        let smooth = smooth.is_some();
        let mut out = BezPath::new();
        let Some(&(first, first_radius)) = samples.first() else { return out };
        // The way along at each sample: of the chord to the next, the last of the one before.
        let mut ways: Vec<Vec2> = samples.windows(2).map(|pair| pair[1].0 - pair[0].0).filter(|way| way.hypot2() > 1e-18).map(|way| way.normalize()).collect();
        if ways.is_empty() {
            // A dot: the nib alone.
            let (_, _, a, b) = self.nib.axes(first_radius);
            out.extend(Ellipse::new(first, (a, b), self.nib.angle.to_radians()).path_elements(0.05));
            return out;
        }
        ways.push(ways[ways.len() - 1]);
        let left = |way: Vec2| Vec2::new(way.y, -way.x);
        // The nib's edge from facing `from` to facing `to`, turning the short way, or by half a
        // turn through `through` at an end.
        let fan = |out: &mut BezPath, at: kurbo::Point, radius: f64, from: Vec2, to: Vec2| {
            let turn = from.cross(to).atan2(from.dot(to));
            let steps = (turn.abs() / 0.15).ceil().max(1.0) as usize;
            for step in 0..=steps {
                let angle = turn * step as f64 / steps as f64;
                let (sin, cos) = angle.sin_cos();
                let facing = Vec2::new(from.x * cos - from.y * sin, from.x * sin + from.y * cos);
                out.line_to(at + self.nib.reach(radius, facing));
            }
        };
        // The edge at a sample the stroke arrives at along `arrive` and leaves along `leave`:
        // round the nib from facing one to facing the other, or smooth, where the turn is
        // slight, the one point facing between them.
        let side = |out: &mut BezPath, at: kurbo::Point, radius: f64, arrive: Vec2, leave: Vec2| {
            let between = arrive + leave;
            if smooth && arrive.dot(leave) > 0.94 && between.hypot2() > 1e-18 {
                out.line_to(at + self.nib.reach(radius, left(between.normalize())));
            } else {
                fan(out, at, radius, left(arrive), left(leave));
            }
        };
        // At each sample the stroke arrives along the chord before it and leaves along the one
        // after: forward, `ways[i - 1]` then `ways[i]`; coming back, the two reversed.
        let last = samples.len() - 1;
        out.move_to(first + self.nib.reach(first_radius * forward[0], left(ways[0])));
        // Round the start, the way back having come to it at its own width.
        let first_radius = first_radius * back[0];
        for (i, &(at, radius)) in samples.iter().enumerate() {
            let arrive = if i == 0 { ways[0] } else { ways[i - 1] };
            side(&mut out, at, radius * forward[i], arrive, ways[i]);
        }
        // Round the end, from its left to its right through straight ahead; or square across.
        let (end, end_radius) = (samples[last].0, samples[last].1 * forward[last]);
        let square = |out: &mut BezPath, at: kurbo::Point, radius: f64, way: Vec2, cap: Cap| {
            let past = if cap == Cap::Miter { way * self.nib.reach(radius, way).dot(way) } else { Vec2::ZERO };
            out.line_to(at + self.nib.reach(radius, left(way)) + past);
            out.line_to(at + self.nib.reach(radius, -left(way)) + past);
        };
        match self.caps[1] {
            Cap::Round => {
                fan(&mut out, end, end_radius, left(ways[last]), ways[last]);
                fan(&mut out, end, end_radius, ways[last], -left(ways[last]));
            }
            cap => square(&mut out, end, end_radius, ways[last], cap),
        }
        for (i, &(at, radius)) in samples.iter().enumerate().rev() {
            let leave = if i == 0 { ways[0] } else { ways[i - 1] };
            side(&mut out, at, radius * back[i], -ways[i], -leave);
        }
        match self.caps[0] {
            Cap::Round => {
                fan(&mut out, first, first_radius, left(-ways[0]), -ways[0]);
                fan(&mut out, first, first_radius, -ways[0], left(ways[0]));
            }
            cap => square(&mut out, first, first_radius, -ways[0], cap),
        }
        out.close_path();
        out
    }

    /// Points along the middle line, close enough together that the quadrilaterals between them
    /// follow its curves and its changes of width, each with the radius there.
    pub(crate) fn samples(&self) -> Vec<(kurbo::Point, f64)> {
        let radius = |at: usize| self.widths.get(at).or(self.widths.last()).copied().unwrap_or(0.0).max(0.0) / 2.0;
        self.places().into_iter().map(|(at, index, t)| (at, radius(index) + (radius(index + 1) - radius(index)) * t)).collect()
    }

    /// What `each` makes of the element and how far along it each of the places
    /// [`VectorStroke::samples`] gives is: values kept by point, going evenly between them.
    fn sampled<T>(&self, each: impl Fn(usize, f64) -> T) -> Vec<T> {
        self.places().into_iter().map(|(_, index, t)| each(index, t)).collect()
    }

    /// The places along the middle line it is sampled at: each point, the index of the point
    /// before it, and how far it is from that one to the next.
    fn places(&self) -> Vec<(kurbo::Point, usize, f64)> {
        let radius = |at: usize| self.widths.get(at).or(self.widths.last()).copied().unwrap_or(0.0).max(0.0) / 2.0;
        let mut places = Vec::new();
        for (index, segment) in self.path.segments().enumerate() {
            let (from, to) = (radius(index), radius(index + 1));
            if places.is_empty() {
                places.push((segment.eval(0.0), index, 0.0));
            }
            // Steps short enough that a chord strays less than a tenth of a pixel from the
            // curve, and that the width changes by less than a pixel between them.
            let length = segment.arclen(0.1);
            let bend = match segment {
                PathSeg::Line(_) => 1.0,
                _ => (length / 2.0).max(1.0),
            };
            let steps = bend.max((to - from).abs() * 2.0).ceil().min(4096.0) as usize;
            for step in 1..=steps {
                let t = step as f64 / steps as f64;
                places.push((segment.eval(t), index, t));
            }
        }
        if places.is_empty() {
            // A dot: a path that is one point.
            if let Some(PathEl::MoveTo(at)) = self.path.elements().first() {
                places.push((*at, 0, 0.0));
            }
        }
        places
    }
}

/// Points along a shape's middle line, or a fill's outline, a pixel or two apart: what a
/// selection takes it by.
fn samples(shape: &VectorShape) -> Vec<kurbo::Point> {
    let path = match &shape.kind {
        ShapeKind::Stroke(stroke) => &stroke.path,
        ShapeKind::Fill(fill) => &fill.outline,
    };
    let mut points: Vec<kurbo::Point> = path.elements().first().and_then(|element| element.end_point()).into_iter().collect();
    for segment in path.segments() {
        let steps = (segment.arclen(0.1) / 1.5).ceil().clamp(1.0, 4096.0) as usize;
        points.extend((1..=steps).map(|step| segment.eval(step as f64 / steps as f64)));
    }
    points
}

impl ShapeKind {
    /// How it boils, if it does.
    pub fn boil(&self) -> Option<Boil> {
        match self {
            ShapeKind::Stroke(stroke) => stroke.boil,
            ShapeKind::Fill(fill) => fill.boil,
        }
    }

    /// The shape with every point moved by `bend`, as [`VectorArt::bent`] moves them.
    fn bend(&mut self, step: f64, bend: &impl Fn(kurbo::Point) -> kurbo::Point) {
        let step = step.max(0.5);
        let bend_stroke = |stroke: &mut VectorStroke| {
            let (path, [widths, opacities, drifts]) = cut_up(&stroke.path, [&stroke.widths, &stroke.opacities, &stroke.drifts], step);
            let ends: Vec<kurbo::Point> = path.elements().iter().filter_map(PathEl::end_point).collect();
            let widths = widths.iter().enumerate().map(|(at, width)| width * across(&ends, at, width / 2.0, bend)).collect();
            (stroke.path, stroke.widths, stroke.opacities, stroke.drifts) = (moved(&path, bend), widths, opacities, drifts);
        };
        match self {
            ShapeKind::Stroke(stroke) => bend_stroke(stroke),
            ShapeKind::Fill(fill) => {
                fill.tracks.iter_mut().for_each(bend_stroke);
                fill.outline = moved(&cut_up(&fill.outline, [&[], &[], &[]], step).0, bend);
                fill.inside = fill.inside.map(|[x, y]| {
                    let at = bend(kurbo::Point::new(x, y));
                    [at.x, at.y]
                });
            }
        }
    }

    /// The area painted, as a path to fill by the non-zero rule. A stroke's is its one closed
    /// line, not the pieces of [`VectorStroke::outline`], whose coverage adds up where they
    /// overlap at an edge pixel and draws the edge too full.
    pub fn area(&self) -> BezPath {
        match self {
            ShapeKind::Stroke(stroke) => stroke.contour(),
            ShapeKind::Fill(fill) => fill.outline.clone(),
        }
    }

    /// The shape with its points moved by `transform`; a stroke's width is scaled by how much
    /// `transform` scales areas.
    pub fn transformed(&self, transform: kurbo::Affine) -> ShapeKind {
        match self {
            ShapeKind::Stroke(stroke) => {
                let scale = transform.determinant().abs().sqrt();
                // The nib turns as the x axis does.
                let [a, b, ..] = transform.as_coeffs();
                let nib = Nib { angle: stroke.nib.angle + b.atan2(a).to_degrees(), ..stroke.nib };
                ShapeKind::Stroke(VectorStroke {
                    path: transform * &stroke.path,
                    widths: stroke.widths.iter().map(|w| w * scale).collect(),
                    nib,
                    caps: stroke.caps,
                    grain: stroke.grain.clone(),
                    opacities: stroke.opacities.clone(),
                    drifts: stroke.drifts.clone(),
                    boil: stroke.boil,
                })
            }
            ShapeKind::Fill(fill) => ShapeKind::Fill(VectorFill {
                outline: transform * &fill.outline,
                inside: fill.inside.map(|[x, y]| {
                    let at = transform * kurbo::Point::new(x, y);
                    [at.x, at.y]
                }),
                gap: fill.gap * transform.determinant().abs().sqrt(),
                tracks: fill
                    .tracks
                    .iter()
                    .filter_map(|track| match ShapeKind::Stroke(track.clone()).transformed(transform) {
                        ShapeKind::Stroke(track) => Some(track),
                        ShapeKind::Fill(_) => None,
                    })
                    .collect(),
                ..fill.clone()
            }),
        }
    }
}

/// An art layer's vector art: its shapes, bottom first, and the pixels they make.
#[derive(Clone, Debug, Default)]
pub struct VectorArt {
    shapes: Arc<Vec<VectorShape>>,
    raster: IndexedSurface,
}

impl PartialEq for VectorArt {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shapes, &other.shapes) || self.shapes == other.shapes
    }
}

impl VectorArt {
    /// Vector art of `shapes`, bottom first.
    pub fn new(shapes: Vec<VectorShape>) -> VectorArt {
        let raster = rasterize(&shapes);
        VectorArt { shapes: Arc::new(shapes), raster }
    }

    pub fn shapes(&self) -> &[VectorShape] {
        &self.shapes
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// The shapes as indexed pixels: a swatch and a coverage a pixel, each shape painted over
    /// those under it as a raster stroke is.
    pub fn raster(&self) -> &IndexedSurface {
        &self.raster
    }

    /// An id that no shape has yet.
    pub fn next_id(&self) -> ShapeId {
        ShapeId(self.shapes.iter().map(|shape| shape.id.0 + 1).max().unwrap_or(1))
    }

    /// The same art with its shapes changed by `edit`.
    pub fn edited(&self, edit: impl FnOnce(&mut Vec<VectorShape>)) -> VectorArt {
        let mut shapes = (*self.shapes).clone();
        edit(&mut shapes);
        VectorArt::new(shapes)
    }

    /// The shapes moved, scaled or turned by `transform`.
    pub fn transformed(&self, transform: &crate::Affine) -> VectorArt {
        let [a, b, c, d, e, f] = transform.m;
        let transform = kurbo::Affine::new([a, b, c, d, e, f]);
        self.edited(|shapes| shapes.iter_mut().for_each(|shape| shape.kind = shape.kind.transformed(transform)))
    }

    /// The shapes with every point moved by `bend`, as a deformer moves them (plan §4.5): their
    /// paths cut first into pieces no longer than `step`, so that a long segment bends along
    /// its length and not only at its ends. A stroke keeps its nib, and its width at each point
    /// is as much wider as `bend` stretches the drawing across the line there: a part turned or
    /// moved keeps its width, a part stretched along the line too.
    pub fn bent(&self, step: f64, bend: impl Fn(kurbo::Point) -> kurbo::Point) -> VectorArt {
        self.edited(|shapes| shapes.iter_mut().for_each(|shape| shape.kind.bend(step, &bend)))
    }

    /// The art as it is drawn on `frame` where any of its shapes boils, each of those the way
    /// its turn there says ([`Boil`]); `None` where none does, or each is drawn as it is kept.
    pub fn boiled(&self, frame: u32) -> Option<VectorArt> {
        let turning = |shape: &VectorShape| shape.kind.boil().filter(|boil| boil.amount > 0.0).map(|boil| boil.turn(frame)).filter(|turn| *turn > 0);
        if !self.shapes.iter().any(|shape| turning(shape).is_some()) {
            return None;
        }
        Some(self.edited(|shapes| {
            for shape in shapes.iter_mut() {
                let (Some(turn), Some(boil)) = (turning(shape), shape.kind.boil()) else { continue };
                let seed = u64::from(shape.id.0).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(turn).wrapping_mul(0xD1B5_4A32_D192_ED03);
                // Moved by a smooth field a few times as wide as it moves, its edge staying
                // smooth; each turn its own field, and its grain laid anew.
                let wide = (boil.amount * 6.0).max(8.0);
                let wobble =
                    |at: kurbo::Point| at + Vec2::new(noise2(at.x / wide, at.y / wide, seed), noise2(at.x / wide, at.y / wide, seed ^ 0xB011)) * boil.amount;
                shape.kind.bend((wide / 4.0).max(1.0), &wobble);
                let regrain = |stroke: &mut VectorStroke| {
                    if let Some(grain) = &mut stroke.grain {
                        grain.seed ^= seed;
                    }
                };
                match &mut shape.kind {
                    ShapeKind::Stroke(stroke) => regrain(stroke),
                    ShapeKind::Fill(fill) => fill.tracks.iter_mut().for_each(regrain),
                }
            }
        }))
    }

    /// `other`'s shapes put over these, with ids of their own here.
    pub fn stamped(&self, other: &VectorArt) -> VectorArt {
        let first = self.next_id().0;
        let renumbered = |shape: &VectorShape| {
            let id = |of: ShapeId| ShapeId(first + of.0);
            let mut shape = shape.clone();
            shape.id = id(shape.id);
            // What bounds it is of the art it came from: there it follows no lines.
            if let ShapeKind::Fill(fill) = &mut shape.kind {
                fill.bound_by.clear();
            }
            shape
        };
        self.edited(|shapes| shapes.extend(other.shapes.iter().map(renumbered)))
    }

    /// Which shapes a selection takes, whole: those of its swatch, if it names one, and of no
    /// protected one, whose middle line, or a fill's outline, passes inside its rectangle and
    /// its lasso anywhere.
    pub(crate) fn picks(&self, picked: &crate::Picked) -> Vec<bool> {
        let inside = |at: kurbo::Point| {
            let (x, y) = (at.x.floor() as i32, at.y.floor() as i32);
            picked.rect.contains(x, y) && crate::LassoPart::inside(picked.parts, x, y)
        };
        let of_swatch = |shape: &VectorShape| picked.swatch.is_none_or(|swatch| shape.swatch == swatch) && !picked.protected.contains(&shape.swatch);
        self.shapes.iter().map(|shape| of_swatch(shape) && samples(shape).into_iter().any(inside)).collect()
    }

    /// The topmost shape at `at`: a fill or a stroke painted there, or a stroke whose middle line
    /// passes within `reach` of it.
    pub fn shape_at(&self, at: kurbo::Point, reach: f64) -> Option<usize> {
        let near = |path: &BezPath| path.segments().any(|segment| kurbo::ParamCurveNearest::nearest(&segment, at, 0.01).distance_sq <= reach * reach);
        self.shapes.iter().rposition(|shape| {
            shape.kind.area().contains(at)
                || match &shape.kind {
                    ShapeKind::Stroke(stroke) => near(&stroke.path),
                    ShapeKind::Fill(_) => false,
                }
        })
    }

    /// A pixel that the shape at `index` passes through and no other shape of this art or of
    /// `others` does: a selection of it alone takes that shape, and only it. The one farthest
    /// from the others, to be sure of it; `None` where every pixel of it is shared.
    pub fn own_pixel(&self, index: usize, others: &[&VectorArt]) -> Option<(i32, i32)> {
        let pixel = |at: kurbo::Point| (at.x.floor() as i32, at.y.floor() as i32);
        let theirs: BTreeSet<(i32, i32)> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, shape)| shape)
            .chain(others.iter().flat_map(|art| art.shapes.iter()))
            .flat_map(samples)
            .map(pixel)
            .collect();
        let own: BTreeSet<(i32, i32)> = samples(self.shapes.get(index)?).into_iter().map(pixel).filter(|at| !theirs.contains(at)).collect();
        let apart = |(x, y): (i32, i32)| theirs.iter().map(|(ox, oy)| (ox - x).pow(2) + (oy - y).pow(2)).min().unwrap_or(i32::MAX);
        own.into_iter().max_by_key(|at| apart(*at))
    }

    /// The shapes a selection takes, as vector art of their own.
    pub fn extract_picked(&self, picked: &crate::Picked) -> VectorArt {
        let picks = self.picks(picked);
        VectorArt::new(self.shapes.iter().zip(picks).filter(|(_, taken)| *taken).map(|(shape, _)| shape.clone()).collect())
    }

    /// The art without the shapes a selection takes.
    pub fn clear_picked(&self, picked: &crate::Picked) -> VectorArt {
        let picks = self.picks(picked);
        let mut taken = picks.into_iter();
        self.edited(|shapes| shapes.retain(|_| !taken.next().unwrap_or(false)))
    }

    /// The shapes a selection takes moved, scaled or turned by `transform`, each keeping its
    /// place among the others and its id; and the bounds of where they landed.
    pub fn transform_picked(&self, picked: &crate::Picked, transform: &crate::Affine) -> (VectorArt, Rect) {
        let picks = self.picks(picked);
        let [a, b, c, d, e, f] = transform.m;
        let transform = kurbo::Affine::new([a, b, c, d, e, f]);
        let moved = self.edited(|shapes| {
            for (shape, taken) in shapes.iter_mut().zip(&picks) {
                if *taken {
                    shape.kind = shape.kind.transformed(transform);
                }
            }
        });
        let landed = moved.shapes.iter().zip(&picks).filter(|(_, taken)| **taken).fold(Rect::EMPTY, |landed, (shape, _)| {
            let reach = shape.kind.area().bounding_box();
            landed.union(&Rect::new(reach.x0.floor() as i32, reach.y0.floor() as i32, reach.x1.ceil() as i32, reach.y1.ceil() as i32))
        });
        (moved, landed)
    }

    /// The shapes a selection takes given the swatch `to`, or `None` if it takes none that
    /// isn't of it already.
    pub fn recolour_picked(&self, picked: &crate::Picked, to: SwatchId) -> Option<VectorArt> {
        let picks = self.picks(picked);
        let changes = self.shapes.iter().zip(&picks).any(|(shape, taken)| *taken && shape.swatch != to);
        changes.then(|| {
            self.edited(|shapes| {
                for (shape, taken) in shapes.iter_mut().zip(&picks) {
                    if *taken {
                        shape.swatch = to;
                    }
                }
            })
        })
    }

    /// The swatches the shapes are painted with.
    pub fn swatches(&self) -> BTreeSet<SwatchId> {
        self.shapes.iter().map(|shape| shape.swatch).collect()
    }

    /// The shapes as a project store keeps them.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&*self.shapes).unwrap_or_default()
    }

    /// Vector art of shapes as [`VectorArt::to_json`] wrote them.
    pub fn from_json(json: &str) -> Result<VectorArt, serde_json::Error> {
        Ok(VectorArt::new(serde_json::from_str(json)?))
    }
}

/// `path` with each segment cut into pieces no longer than `step`, and each of `values` (one for
/// each of its points, as a stroke's widths are) with those of the points that cutting adds,
/// going evenly between the two they lie between; an empty one stays empty. Closing a subpath
/// stays as it is.
fn cut_up(path: &BezPath, values: [&[f64]; 3], step: f64) -> (BezPath, [Vec<f64>; 3]) {
    let value = |of: &[f64], at: usize| of.get(at).or(of.last()).copied().unwrap_or(0.0);
    let (mut out, mut cut) = (BezPath::new(), [Vec::new(), Vec::new(), Vec::new()]);
    let mut point = 0;
    let mut last = kurbo::Point::ZERO;
    for element in path.elements() {
        let segment = match *element {
            PathEl::MoveTo(at) => {
                out.move_to(at);
                for (cut, of) in cut.iter_mut().zip(values).filter(|(_, of)| !of.is_empty()) {
                    cut.push(value(of, point));
                }
                (point, last) = (point + 1, at);
                continue;
            }
            PathEl::ClosePath => {
                out.close_path();
                continue;
            }
            PathEl::LineTo(to) => PathSeg::Line(kurbo::Line::new(last, to)),
            PathEl::QuadTo(c, to) => PathSeg::Quad(kurbo::QuadBez::new(last, c, to)),
            PathEl::CurveTo(c1, c2, to) => PathSeg::Cubic(kurbo::CubicBez::new(last, c1, c2, to)),
        };
        let pieces = (segment.arclen(0.1) / step).ceil().clamp(1.0, 4096.0) as usize;
        for piece in 0..pieces {
            let (t0, t1) = (piece as f64 / pieces as f64, (piece + 1) as f64 / pieces as f64);
            match segment.subsegment(t0..t1) {
                PathSeg::Line(line) => out.line_to(line.p1),
                PathSeg::Quad(quad) => out.quad_to(quad.p1, quad.p2),
                PathSeg::Cubic(cubic) => out.curve_to(cubic.p1, cubic.p2, cubic.p3),
            }
            for (cut, of) in cut.iter_mut().zip(values).filter(|(_, of)| !of.is_empty()) {
                let (from, to) = (value(of, point - 1), value(of, point));
                cut.push(from + (to - from) * t1);
            }
        }
        (point, last) = (point + 1, segment.end());
    }
    (out, cut)
}

/// `path` with every point, handles too, moved by `bend`.
fn moved(path: &BezPath, bend: impl Fn(kurbo::Point) -> kurbo::Point) -> BezPath {
    path.elements()
        .iter()
        .map(|element| match *element {
            PathEl::MoveTo(at) => PathEl::MoveTo(bend(at)),
            PathEl::LineTo(at) => PathEl::LineTo(bend(at)),
            PathEl::QuadTo(c, at) => PathEl::QuadTo(bend(c), bend(at)),
            PathEl::CurveTo(c1, c2, at) => PathEl::CurveTo(bend(c1), bend(c2), bend(at)),
            PathEl::ClosePath => PathEl::ClosePath,
        })
        .collect()
}

/// How much `bend` stretches the drawing across a line through `ends` at its point `at`: the
/// distance between where it takes two points `apart` either side of the line, the stroke's
/// edges, over their distance before. Taken at the edges and not just beside the line, a bend
/// that is sharp at the line, as the crease of a joint is, does not make the stroke jump in
/// width there. 1 where the line has no way there, and within a twentieth and twenty times.
fn across(ends: &[kurbo::Point], at: usize, apart: f64, bend: impl Fn(kurbo::Point) -> kurbo::Point) -> f64 {
    let apart = apart.max(0.5);
    let (Some(&point), Some(&before), Some(&after)) = (ends.get(at), ends.get(at.saturating_sub(1)), ends.get((at + 1).min(ends.len().saturating_sub(1))))
    else {
        return 1.0;
    };
    let way = after - before;
    if way.hypot() < 1e-9 {
        return 1.0;
    }
    let way = way.normalize();
    let side = kurbo::Vec2::new(-way.y, way.x) * apart;
    let stretch = (bend(point + side) - bend(point - side)).hypot() / (2.0 * apart);
    if stretch.is_finite() { stretch.clamp(0.05, 20.0) } else { 1.0 }
}

/// The largest piece of a shape turned into pixels at once, a side.
const PIECE: u32 = 1024;

/// `shapes` as indexed pixels, each painted over those before it.
fn rasterize(shapes: &[VectorShape]) -> IndexedSurface {
    let mut surface = IndexedSurface::new();
    let mut rasterizer = Rasterizer::default();
    for shape in shapes {
        // A stroke with grain or tone is cut by them, as the pixel brush's paint is; a surface
        // by those of each stroke it was drawn from, the most any of them leaves. A surface's
        // rough edge goes out up to nine tenths of the stroke's half width past it.
        let (tracks, reach): (Vec<&VectorStroke>, f64) = match &shape.kind {
            ShapeKind::Stroke(stroke) => (vec![stroke], 1.0),
            ShapeKind::Fill(fill) => (fill.tracks.iter().collect(), 1.9),
        };
        let marks: Vec<Mark> = tracks.into_iter().filter(|track| track.marks()).map(|track| Mark::of(track, reach)).collect();
        rasterizer.fill(&shape.kind.area(), |rect, coverage| match marks.is_empty() {
            false => {
                // How much each leaves, and how far its colour drifts: the most of any.
                let (mut most, mut drift) = (vec![0.0f32; coverage.len()], None::<Vec<f32>>);
                for mark in &marks {
                    let (cut, drifted) = mark.in_rect(rect);
                    match cut {
                        Some(cut) => most.iter_mut().zip(cut).for_each(|(most, m)| *most = most.max(m)),
                        None => most.fill(1.0),
                    }
                    if let Some(drifted) = drifted {
                        let drift = drift.get_or_insert_with(|| vec![0.0; coverage.len()]);
                        drift.iter_mut().zip(drifted).for_each(|(most, d)| *most = most.max(d));
                    }
                }
                let cut: Vec<f32> = coverage.iter().zip(most).map(|(c, m)| c * m).collect();
                surface.paint_drifted(crate::PaintMode::Over, rect, &cut, drift.as_deref(), shape.swatch, &[]);
            }
            true => surface.paint(rect, coverage, shape.swatch),
        });
    }
    surface.prune();
    surface
}

/// What cuts the coverage of a shape along one stroke, its grain and its opacity, and how far
/// its colour drifts.
struct Mark<'a> {
    stroke: &'a VectorStroke,
    grain: Option<photocraft_paint::StrokeRenderer>,
    /// How much wider than the stroke its tone reaches.
    reach: f64,
}

impl<'a> Mark<'a> {
    fn of(stroke: &'a VectorStroke, reach: f64) -> Mark<'a> {
        let grain = stroke.grained().then(|| stroke.grain.as_ref().map(|grain| grain.mask(&stroke.samples()))).flatten();
        Mark { stroke, grain, reach }
    }

    /// How much of each pixel of `rect` it leaves, in rows, `None` for all of it; and how far
    /// its colour drifts at each, `None` for not at all.
    fn in_rect(&self, rect: Rect) -> (Option<Vec<f32>>, Option<Vec<f32>>) {
        let [opacity, drift] = self.stroke.tone_in(rect, self.reach);
        let cut = match (self.grain.as_ref().map(|grain| grain.coverage_in(rect)), opacity) {
            (Some(grain), Some(opacity)) => Some(grain.iter().zip(opacity).map(|(g, o)| g * o).collect()),
            (grain, opacity) => grain.or(opacity),
        };
        (cut, drift)
    }
}

/// Turns areas into coverage, keeping what `vello_cpu` needs from one area to the next.
#[derive(Default)]
pub(crate) struct Rasterizer {
    context: Option<vello_cpu::RenderContext>,
    resources: Option<vello_cpu::Resources>,
}

impl Rasterizer {
    /// How much of each pixel `area` covers, filled by the non-zero rule: given to `each` as
    /// pieces of the pixels it reaches, each with its coverage (0..1) in rows.
    pub(crate) fn fill(&mut self, area: &BezPath, mut each: impl FnMut(Rect, &[f32])) {
        let bounds = area.bounding_box();
        if !(bounds.is_finite() && bounds.area() > 0.0) {
            return;
        }
        let [x0, y0] = [bounds.x0.floor() as i32, bounds.y0.floor() as i32];
        let [x1, y1] = [bounds.x1.ceil() as i32, bounds.y1.ceil() as i32];
        let resources = self.resources.get_or_insert_with(vello_cpu::Resources::new);
        for top in (y0..y1).step_by(PIECE as usize) {
            for left in (x0..x1).step_by(PIECE as usize) {
                let rect = Rect::new(left, top, (left + PIECE as i32).min(x1), (top + PIECE as i32).min(y1));
                let (width, height) = (rect.width() as u16, rect.height() as u16);
                let context = match &mut self.context {
                    Some(context) => {
                        context.reset_and_resize(width, height);
                        context
                    }
                    None => self.context.insert(vello_cpu::RenderContext::new(width, height)),
                };
                context.set_transform(kurbo::Affine::translate((-f64::from(left), -f64::from(top))));
                context.set_fill_rule(vello_cpu::peniko::Fill::NonZero);
                context.set_paint(vello_cpu::color::palette::css::WHITE);
                context.fill_path(area);
                context.flush();
                let mut pixmap = vello_cpu::Pixmap::new(width, height);
                context.render(&mut pixmap, resources);
                let coverage: Vec<f32> = pixmap.data().iter().map(|pixel| f32::from(pixel.a) / 255.0).collect();
                each(rect, &coverage);
            }
        }
    }
}

/// A path kept as SVG path data, which is short and can be read in the file.
mod svg_path {
    use kurbo::BezPath;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(path: &BezPath, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&path.to_svg())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<BezPath, D::Error> {
        let svg = String::deserialize(deserializer)?;
        BezPath::from_svg(&svg).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(from: (f64, f64), to: (f64, f64)) -> BezPath {
        let mut path = BezPath::new();
        path.move_to(from);
        path.line_to(to);
        path
    }

    fn stroke(id: u32, swatch: u16, stroke: VectorStroke) -> VectorShape {
        VectorShape { id: ShapeId(id), swatch: SwatchId(swatch), kind: ShapeKind::Stroke(stroke) }
    }

    #[test]
    fn a_stroke_covers_its_width_along_its_line_with_round_ends() {
        let art = VectorArt::new(vec![stroke(1, 3, VectorStroke::even(line((10.0, 10.0), (50.0, 10.0)), 6.0))]);
        let raster = art.raster();
        assert_eq!(raster.bounds(), Rect::new(7, 7, 53, 13), "three pixels each side of the line, and past its ends");
        let at = |x, y| raster.pixel(x, y);
        assert_eq!((at(30, 8).swatch, at(30, 8).coverage), (SwatchId(3), u16::MAX), "inside");
        assert_eq!(at(30, 14).coverage, 0, "outside");
        assert!(at(7, 7).coverage < u16::MAX / 20, "the end is round, not square: the corner is barely touched");
    }

    #[test]
    fn its_width_goes_evenly_from_point_to_point() {
        let tapered = VectorStroke {
            path: line((0.0, 0.0), (100.0, 0.0)),
            widths: vec![2.0, 20.0],
            nib: Nib::default(),
            caps: Default::default(),
            grain: None,
            ..Default::default()
        };
        let raster = VectorArt::new(vec![stroke(1, 1, tapered)]).raster().clone();
        let thickness = |x| (-12..12).filter(|&y| raster.pixel(x, y).coverage > u16::MAX / 2).count();
        assert!((2..=4).contains(&thickness(5)), "thin at the start: {}", thickness(5));
        assert!((10..=12).contains(&thickness(50)), "half way: {}", thickness(50));
        assert!((18..=21).contains(&thickness(95)), "wide at the end: {}", thickness(95));
    }

    #[test]
    fn a_stroke_that_crosses_itself_has_no_hole_where_it_does() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((40.0, 40.0));
        path.line_to((40.0, 0.0));
        path.line_to((0.0, 40.0));
        let raster = VectorArt::new(vec![stroke(1, 1, VectorStroke::even(path, 8.0))]).raster().clone();
        assert_eq!(raster.pixel(20, 20).coverage, u16::MAX, "where it crosses");
    }

    #[test]
    fn later_shapes_paint_over_earlier_ones() {
        let mut square = BezPath::new();
        square.move_to((0.0, 0.0));
        square.line_to((20.0, 0.0));
        square.line_to((20.0, 20.0));
        square.line_to((0.0, 20.0));
        square.close_path();
        let fill = VectorShape { id: ShapeId(1), swatch: SwatchId(2), kind: ShapeKind::Fill(VectorFill::plain(square)) };
        let art = VectorArt::new(vec![fill, stroke(2, 1, VectorStroke::even(line((0.0, 10.0), (20.0, 10.0)), 4.0))]);
        assert_eq!(art.raster().pixel(10, 3).swatch, SwatchId(2), "the fill");
        assert_eq!(art.raster().pixel(10, 10).swatch, SwatchId(1), "the line over it");
        assert_eq!(art.swatches(), BTreeSet::from([SwatchId(1), SwatchId(2)]));
    }

    #[test]
    fn shapes_go_to_json_and_back() {
        let nib = Nib { angle: 30.0, roundness: 0.25 };
        let art = VectorArt::new(vec![stroke(
            4,
            1,
            VectorStroke { path: line((0.5, 1.0), (30.0, 2.25)), widths: vec![1.0, 3.5], nib, caps: Default::default(), grain: None, ..Default::default() },
        )]);
        let json = art.to_json();
        assert!(json.contains("M0.5,1 L30,2.25"), "{json}");
        assert_eq!(VectorArt::from_json(&json).unwrap(), art);
        assert_eq!(art.next_id(), ShapeId(5));
    }

    #[test]
    fn an_oval_nib_draws_thick_across_its_angle_and_thin_along_it() {
        // A flat nib at 0°: wide across a line going down, thin along a line going across.
        let nib = Nib { angle: 0.0, roundness: 0.2 };
        let thickness = |from: (f64, f64), to: (f64, f64), across: &dyn Fn(i32) -> (i32, i32)| {
            let raster = VectorArt::new(vec![stroke(
                1,
                1,
                VectorStroke { path: line(from, to), widths: vec![20.0, 20.0], nib, caps: Default::default(), grain: None, ..Default::default() },
            )])
            .raster()
            .clone();
            (-15..15).filter(|&i| raster.pixel(across(i).0, across(i).1).coverage > u16::MAX / 2).count()
        };
        let down = thickness((0.0, 0.0), (0.0, 100.0), &|i| (i, 50));
        let level = thickness((0.0, 0.0), (100.0, 0.0), &|i| (50, i));
        assert!((19..=21).contains(&down), "the nib's whole width across a line going down: {down}");
        assert!((3..=5).contains(&level), "its thickness along a line going across: {level}");
    }

    #[test]
    fn the_contour_for_booleans_covers_what_the_outline_draws() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.curve_to((60.0, -80.0), (120.0, 80.0), (180.0, 0.0));
        path.line_to((180.0, 60.0));
        let mut corner = BezPath::new();
        corner.move_to((0.0, 0.0));
        corner.line_to((100.0, 0.0));
        corner.line_to((100.0, 60.0));
        let drawn = |area: &BezPath| {
            let fill = VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind: ShapeKind::Fill(VectorFill::plain(area.clone())) };
            VectorArt::new(vec![fill]).raster().clone()
        };
        // What one covers whole and the other not at all: a hole, or a piece too many. The
        // edges' pixels differ a little, as where the outline's pieces overlap at an edge their
        // coverage adds up, so they are left out.
        let apart = |a: &IndexedSurface, b: &IndexedSurface| {
            let around = a.bounds().union(&b.bounds());
            let (solid, none) = (u16::MAX - u16::MAX / 10, u16::MAX / 10);
            a.pixels(around)
                .iter()
                .zip(b.pixels(around))
                .filter(|(a, b)| (a.coverage >= solid && b.coverage <= none) || (b.coverage >= solid && a.coverage <= none))
                .count()
        };
        // A loop tighter than the stroke is wide, where the inner edge folds back on itself.
        let mut tight = BezPath::new();
        tight.move_to((0.0, 0.0));
        tight.curve_to((40.0, -30.0), (40.0, 30.0), (0.0, 0.0));
        for (path, widths) in [(path, vec![4.0, 24.0, 10.0]), (corner, vec![14.0, 14.0, 14.0]), (tight, vec![30.0, 30.0])] {
            for nib in [Nib::default(), Nib { angle: 30.0, roundness: 0.3 }] {
                let stroke = VectorStroke { path: path.clone(), widths: widths.clone(), nib, caps: Default::default(), grain: None, ..Default::default() };
                let (contour, outline) = (drawn(&stroke.contour()), drawn(&stroke.outline()));
                assert_eq!(apart(&contour, &outline), 0, "{nib:?}, {widths:?}");
            }
        }
    }

    #[test]
    fn a_surface_keeps_the_grain_of_its_strokes_moved_merged_and_erased() {
        let texture = photocraft_paint::Texture { enabled: true, ..Default::default() };
        let grained = |from: (f64, f64), to: (f64, f64), seed: u64| VectorStroke {
            grain: Some(Box::new(Grain { texture: texture.clone(), seed, ..Grain::default() })),
            ..VectorStroke::even(line(from, to), 20.0)
        };
        let surface_of = |track: VectorStroke, plain: bool| {
            let outline = track.contour();
            let tracks = if plain { Vec::new() } else { vec![track] };
            ShapeKind::Fill(VectorFill { tracks, ..VectorFill::plain(outline) })
        };
        let total = |art: &VectorArt, x0: i32, x1: i32| {
            (-15..15).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| u64::from(art.raster().pixel(x, y).coverage)).sum::<u64>()
        };
        let shape = |id, kind| VectorShape { id: ShapeId(id), swatch: SwatchId(1), kind };
        // Its coverage cut by the grain as the line's is, and kept as it is written and read.
        let rough = VectorArt::new(vec![shape(1, surface_of(grained((0.0, 0.0), (300.0, 0.0), 1), false))]);
        let plain = VectorArt::new(vec![shape(1, surface_of(grained((0.0, 0.0), (300.0, 0.0), 1), true))]);
        assert!(total(&rough, 0, 300) < total(&plain, 0, 300) * 95 / 100);
        assert_eq!(VectorArt::from_json(&rough.to_json()).unwrap(), rough);
        // Moved, its grain goes with it, over the paper there, which stays where it is.
        let moved = rough.transformed(&crate::Affine { m: [1.0, 0.0, 0.0, 1.0, 400.0, 0.0] });
        let ShapeKind::Fill(fill) = &moved.shapes()[0].kind else { panic!("a fill") };
        assert_eq!(fill.tracks[0].path.bounding_box().x0, 400.0);
        assert!(total(&moved, 400, 700) < total(&plain, 0, 300) * 95 / 100);
        // Merged with a surface of its grain, both tracks kept; a plain one stays apart.
        let new = shape(2, surface_of(grained((280.0, 0.0), (500.0, 0.0), 2), false));
        let merged = crate::flattened(&rough, &new).unwrap();
        let ShapeKind::Fill(fill) = &merged.shapes()[0].kind else { panic!("a fill") };
        assert_eq!((merged.shapes().len(), fill.tracks.len()), (1, 2));
        assert!(crate::flattened(&plain, &new).is_none(), "a plain surface is not merged with a grained one");
        // Merge Down the same: those of one grain made one, keeping it, a plain one apart.
        let three = VectorArt::new(vec![
            shape(1, surface_of(grained((0.0, 0.0), (300.0, 0.0), 1), false)),
            shape(2, surface_of(grained((280.0, 0.0), (500.0, 0.0), 2), false)),
            shape(3, surface_of(grained((100.0, 0.0), (200.0, 0.0), 3), true)),
        ]);
        let down = crate::merge_picked(&three, &crate::Picked::all(crate::Rect::new(-20, -20, 520, 20))).unwrap();
        let kinds: Vec<(bool, usize)> = down
            .shapes()
            .iter()
            .map(|shape| match &shape.kind {
                ShapeKind::Fill(fill) => (fill.grained(), fill.tracks.len()),
                _ => (false, 0),
            })
            .collect();
        assert_eq!(kinds, [(true, 2), (false, 0)]);
        // Erased in the middle, the pieces keep it.
        let eraser = VectorStroke::even(line((150.0, -40.0), (150.0, 40.0)), 10.0);
        let erased = crate::erase_vector(&rough, &eraser, &[]).unwrap();
        assert!(erased.shapes().iter().all(|shape| matches!(&shape.kind, ShapeKind::Fill(fill) if fill.grained())));
    }

    #[test]
    fn a_strokes_grain_cuts_its_coverage_the_same_each_time() {
        let drawn = |grain: Option<Box<Grain>>| {
            let line = VectorStroke {
                path: line((0.0, 0.0), (300.0, 0.0)),
                widths: vec![20.0, 20.0],
                nib: Nib::default(),
                caps: Default::default(),
                grain,
                ..Default::default()
            };
            VectorArt::new(vec![stroke(1, 1, line)]).raster().clone()
        };
        let total = |raster: &crate::IndexedSurface| {
            (-15..15).flat_map(|y| (0..300).map(move |x| (x, y))).map(|(x, y)| u64::from(raster.pixel(x, y).coverage)).sum::<u64>()
        };
        let plain = total(&drawn(None));
        // A paper texture takes some of it everywhere; a small second tip far apart leaves gaps.
        let texture = photocraft_paint::Texture { enabled: true, ..Default::default() };
        let textured = Grain { texture, ..Grain::default() };
        assert!(total(&drawn(Some(Box::new(textured.clone())))) < plain * 95 / 100);
        let dual = photocraft_paint::DualBrush { enabled: true, size: 8.0, spacing: 3.0, ..Default::default() };
        let dotted = drawn(Some(Box::new(Grain { dual_brush: dual, ..Grain::default() })));
        assert!(total(&dotted) < plain / 2);
        assert_eq!(drawn(Some(Box::new(textured.clone()))), drawn(Some(Box::new(textured))), "the same each time");
        // Kept with the stroke as it is written and read.
        let art = VectorArt::new(vec![stroke(
            1,
            1,
            VectorStroke { grain: Some(Box::new(Grain::default())), ..VectorStroke::even(line((0.0, 0.0), (9.0, 0.0)), 2.0) },
        )]);
        assert_eq!(VectorArt::from_json(&art.to_json()).unwrap(), art);
    }

    #[test]
    fn a_strokes_end_is_round_straight_across_or_square_past_it() {
        let drawn = |caps| {
            let line =
                VectorStroke { path: line((0.0, 0.0), (100.0, 0.0)), widths: vec![20.0, 20.0], nib: Nib::default(), caps, grain: None, ..Default::default() };
            VectorArt::new(vec![stroke(1, 1, line)]).raster().clone()
        };
        let at = |raster: &crate::IndexedSurface, x, y| raster.pixel(x, y).coverage > u16::MAX / 2;
        let (round, straight, miter) = (drawn([Cap::Round; 2]), drawn([Cap::Round, Cap::Straight]), drawn([Cap::Round, Cap::Miter]));
        // Past the end: round reaches on in the middle only, straight not at all, the square
        // to its corners too.
        assert_eq!((at(&round, 105, 0), at(&round, 107, 8)), (true, false));
        assert_eq!((at(&straight, 95, 8), at(&straight, 102, 0)), (true, false));
        assert_eq!((at(&miter, 107, 8), at(&miter, 112, 0)), (true, false));
        // The start, left round, is as it was.
        assert!(at(&straight, -5, 0) && at(&miter, -5, 0) && !at(&miter, -7, 8));
    }

    #[test]
    fn a_strokes_edge_covers_its_pixels_as_much_as_it_lies_over_them() {
        // Straight, and along a curve with its width changing: across the stroke, the coverage
        // of a column adds up to its width there, as it would were the edge drawn exactly.
        let mut curve = BezPath::new();
        curve.move_to((0.0, 0.25));
        curve.curve_to((60.0, -20.0), (120.0, 20.0), (200.0, 0.25));
        for (path, widths, column, width) in [(line((0.0, 10.3), (100.0, 10.3)), vec![5.0, 5.0], 50, 5.0), (curve, vec![6.0, 6.0], 100, 6.0)] {
            let art = VectorArt::new(vec![stroke(
                1,
                1,
                VectorStroke { path, widths, nib: Nib::default(), caps: Default::default(), grain: None, ..Default::default() },
            )]);
            let raster = art.raster();
            let across: f64 = (-40..40).map(|y| f64::from(raster.pixel(column, y).coverage) / f64::from(u16::MAX)).sum();
            assert!((across - width).abs() < 0.08, "{across} across where it is {width} wide");
        }
    }

    #[test]
    fn bent_a_long_line_bends_along_its_length_and_keeps_its_widths() {
        let art = VectorArt::new(vec![stroke(
            1,
            1,
            VectorStroke {
                path: line((0.0, 0.0), (100.0, 0.0)),
                widths: vec![2.0, 10.0],
                nib: Nib::default(),
                caps: Default::default(),
                grain: None,
                ..Default::default()
            },
        )]);
        // Past x = 50 everything is turned down by 45° about (50, 0), as a bone would.
        let bent = art.bent(4.0, |at| {
            if at.x <= 50.0 {
                at
            } else {
                let (dx, half) = (at.x - 50.0, std::f64::consts::FRAC_1_SQRT_2);
                kurbo::Point::new(50.0 + dx * half - at.y * half, dx * half + at.y * half)
            }
        });
        let ShapeKind::Stroke(stroke) = &bent.shapes()[0].kind else { panic!("a stroke") };
        assert!(stroke.path.segments().count() >= 25, "cut into pieces of 4");
        assert_eq!(stroke.widths.len(), stroke.path.segments().count() + 1);
        assert!(
            (stroke.widths[stroke.widths.len() / 2] - 6.0).abs() < 0.5 && (stroke.widths[stroke.widths.len() - 1] - 10.0).abs() < 1e-6,
            "{:?}",
            stroke.widths
        );
        let end = stroke.path.segments().last().unwrap().end();
        assert!((end.x - 85.36).abs() < 0.01 && (end.y - 35.36).abs() < 0.01, "{end:?}");
        assert!(bent.raster().pixel(80, 30).coverage > 0, "drawn where it was bent to");
    }

    #[test]
    fn bent_a_stroke_is_as_much_wider_as_it_is_stretched_across() {
        let art = VectorArt::new(vec![stroke(
            1,
            1,
            VectorStroke {
                path: line((0.0, 0.0), (100.0, 0.0)),
                widths: vec![10.0, 10.0],
                nib: Nib::default(),
                caps: Default::default(),
                grain: None,
                ..Default::default()
            },
        )]);
        let widths = |bent: &VectorArt| {
            let ShapeKind::Stroke(stroke) = &bent.shapes()[0].kind else { panic!("a stroke") };
            stroke.widths.clone()
        };
        // Turned a quarter: as wide as it was.
        let turned = art.bent(4.0, |at| kurbo::Point::new(-at.y, at.x));
        assert!(widths(&turned).iter().all(|width| (width - 10.0).abs() < 1e-6), "{:?}", widths(&turned));
        // Stretched along its length twice: as wide; across it three times: three times as wide.
        let along = art.bent(4.0, |at| kurbo::Point::new(at.x * 2.0, at.y));
        assert!(widths(&along).iter().all(|width| (width - 10.0).abs() < 1e-6));
        let wide = art.bent(4.0, |at| kurbo::Point::new(at.x, at.y * 3.0));
        assert!(widths(&wide).iter().all(|width| (width - 30.0).abs() < 1e-6), "{:?}", widths(&wide));
        // Past the middle only: wider there alone.
        let half = art.bent(4.0, |at| if at.x > 50.0 { kurbo::Point::new(at.x, at.y * 2.0) } else { at });
        let half = widths(&half);
        assert!((half[0] - 10.0).abs() < 1e-6 && (half[half.len() - 1] - 20.0).abs() < 1e-6, "{half:?}");
    }

    #[test]
    fn a_selection_takes_whole_shapes_it_touches() {
        let art = VectorArt::new(vec![
            stroke(1, 1, VectorStroke::even(line((0.0, 0.0), (100.0, 0.0)), 4.0)),
            stroke(2, 2, VectorStroke::even(line((0.0, 50.0), (100.0, 50.0)), 4.0)),
            stroke(3, 1, VectorStroke::even(line((0.0, 90.0), (100.0, 90.0)), 4.0)),
        ]);
        // A box over the middle of the first two lines, not their ends.
        let picked = crate::Picked::all(Rect::new(40, -10, 60, 60));
        let ids = |art: &VectorArt| art.shapes().iter().map(|shape| shape.id.0).collect::<Vec<_>>();
        assert_eq!(ids(&art.extract_picked(&picked)), [1, 2], "whole, though the box holds only their middles");
        assert_eq!(ids(&art.clear_picked(&picked)), [3]);
        let of_one = crate::Picked { swatch: Some(SwatchId(2)), ..picked };
        assert_eq!(ids(&art.extract_picked(&of_one)), [2]);
        let protected = crate::Picked { protected: &[SwatchId(1)], ..picked };
        assert_eq!(ids(&art.extract_picked(&protected)), [2]);
        let recoloured = art.recolour_picked(&picked, SwatchId(5)).unwrap();
        assert_eq!(recoloured.shapes().iter().map(|shape| shape.swatch.0).collect::<Vec<_>>(), [5, 5, 1]);
        assert!(recoloured.recolour_picked(&picked, SwatchId(5)).is_none());
    }

    #[test]
    fn a_transform_moves_the_points_and_scales_the_width() {
        let art = VectorArt::new(vec![stroke(1, 1, VectorStroke::even(line((0.0, 0.0), (10.0, 0.0)), 2.0))]);
        let moved = art.transformed(&crate::Affine { m: [2.0, 0.0, 0.0, 2.0, 100.0, 0.0] });
        let ShapeKind::Stroke(stroke) = &moved.shapes()[0].kind else { panic!("a stroke") };
        assert_eq!(stroke.path.elements(), line((100.0, 0.0), (120.0, 0.0)).elements());
        assert_eq!(stroke.widths, [4.0, 4.0]);
    }

    #[test]
    fn stamped_shapes_get_ids_of_their_own() {
        let one = VectorArt::new(vec![stroke(1, 1, VectorStroke::even(line((0.0, 0.0), (10.0, 0.0)), 2.0))]);
        let both = one.stamped(&one);
        assert_eq!(both.shapes().iter().map(|s| s.id).collect::<Vec<_>>(), [ShapeId(1), ShapeId(3)]);
    }

    #[test]
    fn a_stroke_is_as_faint_as_its_points_say_and_keeps_it_when_moved_bent_cut_and_saved() {
        // From a fifth opaque to opaque, 20 wide.
        let faint = VectorStroke { opacities: vec![0.2, 1.0], ..VectorStroke::even(line((0.0, 50.0), (200.0, 50.0)), 20.0) };
        assert!(faint.toned() && faint.marks() && !VectorStroke::even(line((0.0, 0.0), (9.0, 0.0)), 2.0).toned());
        let art = VectorArt::new(vec![stroke(1, 1, faint.clone())]);
        let coverage = |art: &VectorArt, x: i32| f32::from(art.raster().pixel(x, 50).coverage) / 65535.0;
        assert!((coverage(&art, 5) - 0.205).abs() < 0.02, "at its start {}", coverage(&art, 5));
        assert!((coverage(&art, 100) - 0.6).abs() < 0.02, "halfway {}", coverage(&art, 100));
        assert!(coverage(&art, 195) > 0.97, "at its end {}", coverage(&art, 195));
        // Its edge as smooth as an opaque one's, as faint as it is there.
        let edge = f32::from(art.raster().pixel(100, 40).coverage) / 65535.0;
        let plain = VectorArt::new(vec![stroke(1, 1, VectorStroke::even(line((0.0, 50.0), (200.0, 50.0)), 20.0))]);
        let plain_edge = f32::from(plain.raster().pixel(100, 40).coverage) / 65535.0;
        assert!((edge - plain_edge * 0.6).abs() < 0.03, "{edge} against {plain_edge}");
        // Moved, it keeps them; bent, cut into more points that go evenly between them.
        let moved = art.transformed(&crate::Affine::translate(10.0, 0.0));
        assert!((coverage(&moved, 15) - 0.205).abs() < 0.02);
        let bent = art.bent(20.0, |at| at);
        let ShapeKind::Stroke(bent) = &bent.shapes()[0].kind else { panic!() };
        assert_eq!(bent.opacities.len(), bent.widths.len());
        assert!(bent.opacities.windows(2).all(|pair| pair[0] < pair[1]) && (bent.opacities[5] - 0.6).abs() < 1e-9, "{:?}", bent.opacities);
        // Erased in the middle, each piece keeps its part.
        let eraser = VectorStroke::even(line((100.0, 0.0), (100.0, 100.0)), 30.0);
        let erased = crate::erase_vector(&art, &eraser, &[]).unwrap();
        let ends: Vec<(f64, f64)> = erased
            .shapes()
            .iter()
            .filter_map(|shape| match &shape.kind {
                ShapeKind::Stroke(piece) => Some((piece.opacities[0], piece.opacities[piece.opacities.len() - 1])),
                ShapeKind::Fill(_) => None,
            })
            .collect();
        assert_eq!(ends.len(), 2);
        assert!((ends[0].0 - 0.2).abs() < 1e-9 && ends[0].1 < 0.6 && ends[1].0 > 0.6 && (ends[1].1 - 1.0).abs() < 1e-9, "{ends:?}");
        // Saved and read back.
        assert_eq!(VectorArt::from_json(&art.to_json()).unwrap(), art);
        assert!(!plain.to_json().contains("opacities"), "an opaque stroke says nothing of them");
    }

    #[test]
    fn faint_surfaces_build_up_where_they_overlap_and_are_never_merged() {
        let faint = |y: f64| VectorStroke { opacities: vec![0.4, 0.4], ..VectorStroke::even(line((0.0, y), (100.0, y)), 20.0) };
        let surface = |id: u32, stroke: VectorStroke| VectorShape {
            id: ShapeId(id),
            swatch: SwatchId(1),
            kind: ShapeKind::Fill(VectorFill { tracks: vec![stroke.clone()], ..VectorFill::plain(stroke.contour()) }),
        };
        let one = VectorArt::new(vec![surface(1, faint(50.0))]);
        let other = surface(2, faint(60.0));
        assert!(crate::flattened(&one, &other).is_none(), "laid on top, its own");
        let both = VectorArt::new(vec![surface(1, faint(50.0)), other]);
        let at = |art: &VectorArt, y: i32| f32::from(art.raster().pixel(50, y).coverage) / 65535.0;
        assert!((at(&both, 45) - 0.4).abs() < 0.02, "one alone: {}", at(&both, 45));
        assert!((at(&both, 55) - 0.64).abs() < 0.02, "both over each other: {}", at(&both, 55));
        // Nor merged by Merge Down.
        let all = crate::Picked::all(crate::Rect::new(-100, -100, 300, 300));
        assert!(crate::merge_picked(&both, &all).is_none());
    }

    #[test]
    fn a_drifting_stroke_paints_its_swatch_drifted_by_steps_and_stays_its_swatchs() {
        // Not drifted at its start, fully at its end.
        let drifting = VectorStroke { drifts: vec![0.0, 1.0], ..VectorStroke::even(line((0.0, 50.0), (150.0, 50.0)), 20.0) };
        assert!(drifting.toned());
        let art = VectorArt::new(vec![stroke(1, 3, drifting)]);
        let at = |x: i32| art.raster().pixel(x, 50).swatch;
        assert_eq!((at(2), at(75).drift(), at(148)), (SwatchId(3), 8, SwatchId(3).drifted(SwatchId::DRIFTS)));
        assert!((1..149).all(|x| at(x).base() == SwatchId(3) && at(x).drift() <= at(x + 1).drift()), "in steps along it");
        assert_eq!(art.raster().swatches(), [SwatchId(3)].into(), "its swatch's");
        // Recoloured, it drifts as far from the other.
        let mut pixels = art.raster().clone();
        assert!(pixels.recolour(SwatchId(3), SwatchId(5)));
        assert_eq!(pixels.pixel(148, 50).swatch, SwatchId(5).drifted(SwatchId::DRIFTS));
        // A swatch past those that drift keeps its colour.
        assert_eq!(SwatchId(3000).drifted(4), SwatchId(3000));
        assert!(!SwatchId(u16::MAX).is_drifted());
    }

    #[test]
    fn a_boiling_stroke_is_drawn_three_ways_in_turn_its_edge_moved_a_little() {
        let boil = Boil { amount: 2.0, every: 2 };
        let boiling = VectorStroke { boil: Some(boil), ..VectorStroke::even(line((0.0, 50.0), (300.0, 50.0)), 10.0) };
        let still = stroke(2, 1, VectorStroke::even(line((0.0, 90.0), (300.0, 90.0)), 10.0));
        let art = VectorArt::new(vec![stroke(1, 1, boiling), still.clone()]);
        // Two frames each way: as kept, then two ways of its own, then as kept again.
        assert_eq!((0..8).map(|frame| boil.turn(frame)).collect::<Vec<_>>(), [0, 0, 1, 1, 2, 2, 0, 0]);
        assert!(art.boiled(0).is_none() && art.boiled(1).is_none() && art.boiled(6).is_none());
        let (one, two) = (art.boiled(2).unwrap(), art.boiled(4).unwrap());
        assert_eq!(art.boiled(3).unwrap(), one, "the same way while it is held");
        assert!(one != two && one != art);
        // Its points no further than the boil from where they were, its width near what it was; the still
        // one as it was.
        let ShapeKind::Stroke(moved) = &one.shapes()[0].kind else { panic!() };
        let away = moved.path.elements().iter().filter_map(PathEl::end_point).map(|at| (at.y - 50.0).abs()).fold(0.0, f64::max);
        assert!(away > 0.3 && away <= 2.0 * std::f64::consts::SQRT_2, "{away}");
        assert!(moved.widths.iter().all(|width| (width - 10.0).abs() < 3.0), "{:?}", moved.widths);
        assert_eq!(one.shapes()[1], still);
        // Kept through JSON.
        assert_eq!(VectorArt::from_json(&art.to_json()).unwrap(), art);
    }
}
