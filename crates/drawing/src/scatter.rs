//! The scatter brush (Moho's Scatter Brush, the user's of 2026-11-23): copies of a vector shape
//! sprayed along the pen's path on vector art, each its own size, turned and flipped by chance,
//! no closer together than a spacing. The copies are ordinary vector shapes once laid.

use kurbo::Shape as _;
use serde::{Deserialize, Serialize};

use crate::{Point, ShapeId, ShapeKind, SwatchId, VectorFill, VectorShape, VectorStroke};

/// What the scatter brush sprays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScatterPreset {
    /// Round dots.
    #[default]
    Dots,
    /// Rings, as bubbles.
    Bubbles,
    /// Five-pointed stars.
    Stars,
    /// Leaves, pointed at both ends.
    Leaves,
    /// Tufts of three blades of grass.
    Grass,
    /// The vector art last cut or copied, in its own swatches.
    Clipboard,
}

impl ScatterPreset {
    pub const ALL: [ScatterPreset; 6] =
        [ScatterPreset::Dots, ScatterPreset::Bubbles, ScatterPreset::Stars, ScatterPreset::Leaves, ScatterPreset::Grass, ScatterPreset::Clipboard];

    pub fn title(self) -> &'static str {
        match self {
            ScatterPreset::Dots => "Dots",
            ScatterPreset::Bubbles => "Bubbles",
            ScatterPreset::Stars => "Stars",
            ScatterPreset::Leaves => "Leaves",
            ScatterPreset::Grass => "Grass",
            ScatterPreset::Clipboard => "Clipboard",
        }
    }

    /// The preset's shapes in `swatch`, about 100 pixels across round the origin. Empty for the
    /// clipboard, whose shapes are given.
    pub fn shapes(self, swatch: SwatchId) -> Vec<VectorShape> {
        let fill = |path: kurbo::BezPath| ShapeKind::Fill(VectorFill::plain(path));
        let kinds: Vec<ShapeKind> = match self {
            ScatterPreset::Dots => vec![fill(kurbo::Circle::new((0.0, 0.0), 50.0).to_path(0.1))],
            ScatterPreset::Bubbles => {
                let ring = kurbo::BezPath::from_vec(
                    kurbo::Circle::new((0.0, 0.0), 45.0).to_path(0.1).into_iter().filter(|el| !matches!(el, kurbo::PathEl::ClosePath)).collect(),
                );
                let points = ring.elements().len();
                vec![ShapeKind::Stroke(VectorStroke { path: ring, widths: vec![8.0; points], ..Default::default() })]
            }
            ScatterPreset::Stars => {
                let mut star = kurbo::BezPath::new();
                for corner in 0..10 {
                    let angle = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * f64::from(corner) / 5.0;
                    let reach = if corner % 2 == 0 { 50.0 } else { 19.1 };
                    let at = kurbo::Point::new(reach * angle.cos(), reach * angle.sin());
                    if corner == 0 { star.move_to(at) } else { star.line_to(at) }
                }
                star.close_path();
                vec![fill(star)]
            }
            ScatterPreset::Leaves => {
                let mut leaf = kurbo::BezPath::new();
                leaf.move_to((-50.0, 0.0));
                leaf.curve_to((-20.0, -28.0), (20.0, -28.0), (50.0, 0.0));
                leaf.curve_to((20.0, 28.0), (-20.0, 28.0), (-50.0, 0.0));
                leaf.close_path();
                vec![fill(leaf)]
            }
            ScatterPreset::Grass => [(-22.0, -40.0), (0.0, -50.0), (24.0, -36.0)]
                .into_iter()
                .map(|(x, top)| {
                    let mut blade = kurbo::BezPath::new();
                    blade.move_to((x * 0.3, 50.0));
                    blade.quad_to((x * 0.5, 0.0), (x, top));
                    ShapeKind::Stroke(VectorStroke { path: blade, widths: vec![10.0, 0.5], ..Default::default() })
                })
                .collect(),
            ScatterPreset::Clipboard => Vec::new(),
        };
        kinds.into_iter().enumerate().map(|(at, kind)| VectorShape { id: ShapeId(at as u32 + 1), swatch, kind }).collect()
    }
}

/// How the scatter brush sprays.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Scatter {
    pub preset: ScatterPreset,
    /// The smallest and the largest a copy is across, in the drawing's pixels: each is between,
    /// by chance; the same for all where they are the same.
    pub min_size: f64,
    pub max_size: f64,
    /// How far apart the copies are along the path at least, in the drawing's pixels.
    pub spacing: f64,
    /// How far a copy may be turned either way, in degrees.
    pub angle_jitter: f64,
    /// Copies flipped left to right, and upside down, by chance: half of them each way.
    pub flip_x: bool,
    pub flip_y: bool,
    /// What chance does: each stroke has its own.
    pub seed: u64,
}

impl Default for Scatter {
    fn default() -> Self {
        Scatter { preset: ScatterPreset::Dots, min_size: 16.0, max_size: 32.0, spacing: 24.0, angle_jitter: 0.0, flip_x: false, flip_y: false, seed: 0 }
    }
}

/// A number from 0 to 1 for `at` and `seed`, the same each time.
fn chance(at: u64, seed: u64) -> f64 {
    let mut x = at.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed;
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 32;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// The places along `path` a copy goes, `spacing` apart, the first where it starts.
fn places(path: &[Point], spacing: f64) -> Vec<Point> {
    let spacing = spacing.max(1.0);
    let Some(&first) = path.first() else { return Vec::new() };
    let mut out = vec![first];
    let (mut gone, mut next) = (0.0, spacing);
    for pair in path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let length = (b.x - a.x).hypot(b.y - a.y);
        while gone + length >= next && length > 0.0 {
            let t = (next - gone) / length;
            out.push(Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
            next += spacing;
            if out.len() >= 20_000 {
                return out;
            }
        }
        gone += length;
    }
    out
}

/// Copies of `source` sprayed along `path` as `scatter` says, bottom first, their ids from
/// `first_id` on: each as large across as its size by chance, at its place, turned and flipped
/// by chance. The same path and seed give the same copies, and a longer path the same copies
/// and more, so a stroke can be shown as it grows. Empty where `source` is.
pub fn scattered(source: &[VectorShape], path: &[Point], scatter: &Scatter, first_id: u32) -> Vec<VectorShape> {
    let reach = source.iter().map(|shape| shape.kind.area().bounding_box()).reduce(|all, more| all.union(more));
    let Some(reach) = reach.filter(|reach| reach.width().max(reach.height()) > 1e-9) else { return Vec::new() };
    let (middle, across) = (reach.center(), reach.width().max(reach.height()));
    let (least, most) = (scatter.min_size.min(scatter.max_size).max(0.5), scatter.max_size.max(scatter.min_size).max(0.5));
    let jitter = scatter.angle_jitter.clamp(0.0, 180.0).to_radians();
    let mut out = Vec::new();
    let mut id = first_id;
    for (n, at) in places(path, scatter.spacing).into_iter().enumerate() {
        let n = n as u64;
        let size = least + (most - least) * chance(n, scatter.seed ^ 0x51);
        let turn = (chance(n, scatter.seed ^ 0xA7) * 2.0 - 1.0) * jitter;
        let flip = |on: bool, salt: u64| if on && chance(n, scatter.seed ^ salt) < 0.5 { -1.0 } else { 1.0 };
        let scale = size / across;
        let transform = kurbo::Affine::translate((at.x, at.y))
            * kurbo::Affine::rotate(turn)
            * kurbo::Affine::scale_non_uniform(scale * flip(scatter.flip_x, 0xF1), scale * flip(scatter.flip_y, 0xF2))
            * kurbo::Affine::translate(-middle.to_vec2());
        for shape in source {
            out.push(VectorShape { id: ShapeId(id), swatch: shape.swatch, kind: shape.kind.transformed(transform) });
            id += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(length: f64) -> Vec<Point> {
        (0..=100).map(|i| Point::new(f64::from(i) * length / 100.0, 50.0)).collect()
    }

    #[test]
    fn copies_are_sprayed_along_the_path_no_closer_than_the_spacing() {
        let dots = ScatterPreset::Dots.shapes(SwatchId(3));
        let scatter = Scatter { min_size: 10.0, max_size: 10.0, spacing: 25.0, seed: 7, ..Scatter::default() };
        let copies = scattered(&dots, &line(200.0), &scatter, 5);
        // At 0, 25, … 200: nine, each 10 across, in the swatch, numbered on.
        assert_eq!(copies.len(), 9);
        assert!(copies.iter().all(|copy| copy.swatch == SwatchId(3)));
        assert_eq!((copies[0].id, copies[8].id), (ShapeId(5), ShapeId(13)));
        let middles: Vec<f64> = copies.iter().map(|copy| copy.kind.area().bounding_box().center().x).collect();
        assert!(middles.windows(2).all(|pair| (pair[1] - pair[0] - 25.0).abs() < 1e-6), "{middles:?}");
        let wide = copies[3].kind.area().bounding_box().width();
        assert!((wide - 10.0).abs() < 0.2, "{wide}");
        // A longer path keeps the copies it had, and the same seed gives the same ones.
        let longer = scattered(&dots, &line(300.0), &scatter, 5);
        assert_eq!(&longer[..9], &copies[..]);
    }

    #[test]
    fn copies_differ_in_size_angle_and_flip_by_chance() {
        let leaves = ScatterPreset::Leaves.shapes(SwatchId(1));
        let scatter = Scatter { min_size: 10.0, max_size: 40.0, spacing: 30.0, angle_jitter: 90.0, flip_y: true, seed: 3, ..Scatter::default() };
        let copies = scattered(&leaves, &line(900.0), &scatter, 1);
        let sizes: Vec<f64> = copies.iter().map(|copy| copy.kind.area().bounding_box().width().max(copy.kind.area().bounding_box().height())).collect();
        let (least, most) = sizes.iter().fold((f64::MAX, 0.0_f64), |(l, m), s| (l.min(*s), m.max(*s)));
        assert!(least >= 3.0 && most <= 41.0 && most - least > 10.0, "{least} to {most}");
        // Turned, some leaves are taller than wide; with no jitter none is.
        assert!(copies.iter().any(|copy| copy.kind.area().bounding_box().height() > copy.kind.area().bounding_box().width()));
        let level = scattered(&leaves, &line(900.0), &Scatter { angle_jitter: 0.0, ..scatter }, 1);
        assert!(level.iter().all(|copy| copy.kind.area().bounding_box().height() < copy.kind.area().bounding_box().width()));
        // The clipboard preset has no shapes of its own, and nothing is sprayed of nothing.
        assert!(ScatterPreset::Clipboard.shapes(SwatchId(1)).is_empty());
        assert!(scattered(&[], &line(100.0), &scatter, 1).is_empty());
        assert!(ScatterPreset::ALL.iter().filter(|preset| **preset != ScatterPreset::Clipboard).all(|preset| !preset.shapes(SwatchId(1)).is_empty()));
    }
}
