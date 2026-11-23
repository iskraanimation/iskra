//! The CPU compositor (plan §3, §4.4): what a shot looks like on a frame, as pixels. It is the
//! reference: slow and plain, so that it can be read and trusted, and what the GPU draws is
//! held against it.
//!
//! It is the second of the plan's two passes. The first (`iskra-eval`) says which layers are
//! seen, in which order and where; this one samples their drawings into a picture.
//!
//! A drawing is flattened as it was painted, its art layers over each other in sRGB; the
//! layers of the shot are then put over each other in the project's working space
//! (plan §4.11).

mod cache;
mod effects;
mod particles;
mod simulation;

pub use cache::{Cache, Key, set_shared_budget};
pub use effects::{CURVE_STEPS, boxes, laid, paint, reach};
pub use particles::{Mark, Particle, Particles, Picture};

use std::sync::Arc;

use iskra_drawing::{Affine, Drawing, Point, Rect};
use iskra_model::{ArtLayerKind, NEAREST, Palette, Projection, Settings, Shot, Warp, WorkingSpace};

/// A picture: rows of straight-alpha sRGB pixels, the top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 4]>,
}

impl Image {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        self.pixels[(y * self.width + x) as usize]
    }
}

/// Which part of what the camera sees a picture is of, and how large the picture is. Places
/// are in the frame's units: pixels of the shot, from the middle of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub width: u32,
    pub height: u32,
    /// From the frame's units to the picture's pixels.
    pub from_frame: Affine,
    /// The shot's frame, in its own pixels, whatever part of it, or round it, the picture
    /// shows and at whatever size: what is laid out over the frame, as particles are, is laid
    /// out by it, so that it is the same at any zoom.
    pub frame: [f64; 2],
}

impl View {
    /// The shot's frame, at `scale` times the shot's resolution.
    pub fn frame(settings: &Settings, scale: f64) -> View {
        let side = |pixels: u32| (f64::from(pixels) * scale).round().max(1.0);
        let (width, height) = (side(settings.width), side(settings.height));
        View {
            width: width as u32,
            height: height as u32,
            from_frame: Affine::translate(width / 2.0, height / 2.0).mul(&Affine::scale(scale)),
            frame: [f64::from(settings.width), f64::from(settings.height)],
        }
    }

    /// How many pixels of the picture a unit of the frame, a pixel of the shot, is across.
    pub fn scale(&self) -> f64 {
        let (origin, along) = (self.from_frame.apply(Point::new(0.0, 0.0)), self.from_frame.apply(Point::new(1.0, 0.0)));
        (along.x - origin.x).hypot(along.y - origin.y)
    }

    /// The same view with `room` pixels more on every side.
    pub fn with_room(&self, room: usize) -> View {
        let (more, by) = (2 * room as u32, room as f64);
        View { width: self.width + more, height: self.height + more, from_frame: Affine::translate(by, by).mul(&self.from_frame), frame: self.frame }
    }

    /// The part between `low` and `high`, in a picture whose longer side is at
    /// most `side` pixels. What is small is shown larger, but no more than `enlarge` times.
    /// The part is taken for the frame.
    pub fn around(low: [f64; 2], high: [f64; 2], side: u32, enlarge: f64) -> Option<View> {
        let (wide, tall) = (high[0] - low[0], high[1] - low[1]);
        if !(wide > 0.0 && tall > 0.0 && wide.is_finite() && tall.is_finite()) {
            return None;
        }
        let scale = (f64::from(side) / wide.max(tall)).min(enlarge);
        let pixels = |length: f64| (length * scale).ceil().clamp(1.0, f64::from(side)) as u32;
        Some(View {
            width: pixels(wide),
            height: pixels(tall),
            from_frame: Affine::scale(scale).mul(&Affine::translate(-low[0], -low[1])),
            frame: [wide, tall],
        })
    }
}

/// A drawing that the camera sees on a frame.
pub struct Seen<'a> {
    /// From the drawing's pixels to the frame: flat for a drawing that faces the camera
    /// squarely, in perspective for one that a peg tilts.
    pub to_frame: Projection,
    /// The drawing; one whose vector art a deformer bends point by point is a bent copy.
    pub drawing: Arc<Drawing>,
    /// The bounds of what is painted on it.
    pub bounds: Rect,
    /// How a deformer bends it on the frame, in its own pixels, before `to_frame` places it
    /// (plan §4.5). `None` for a drawing that is seen as it was drawn.
    pub warp: Option<Warp>,
    /// Its art layers of these kinds are not drawn: its layer has them switched off, or it is
    /// the output of another of its art layers.
    pub off: std::borrow::Cow<'a, [ArtLayerKind]>,
}

impl Seen<'_> {
    /// The drawing as it is drawn: without the art layers that are switched off.
    pub fn shown(&self) -> std::borrow::Cow<'_, Arc<Drawing>> {
        if self.off.is_empty() {
            return std::borrow::Cow::Borrowed(&self.drawing);
        }
        let art_layers = self.drawing.art_layers.iter().filter(|art| !self.off.contains(&art.kind)).cloned().collect();
        std::borrow::Cow::Owned(Arc::new(Drawing { art_layers, texts: Vec::new(), placements: self.drawing.placements.clone() }))
    }
}

/// What a shot's camera sees on a frame that has something painted, in the order to draw it
/// in. Any compositor draws this, so that all draw the same.
pub fn seen(shot: &Shot, frame: u32) -> Vec<Seen<'_>> {
    let evaluated = iskra_eval::evaluate(shot, frame);
    evaluated.layers.iter().filter_map(|placed| seen_of(shot, frame, placed)).collect()
}

/// A layer as it was placed on a frame, as what the camera sees of it: `None` if it shows no
/// drawing there, or one with nothing painted, or is not seen.
pub fn seen_of<'a>(shot: &'a Shot, frame: u32, placed: &iskra_eval::Placed) -> Option<Seen<'a>> {
    let layer = shot.layer(placed.layer)?;
    let drawing = &layer.drawing(placed.drawing?)?.drawing;
    // Its boiling shapes as they are drawn on the frame.
    let drawing = &boiled(drawing, frame);
    // A deformer at its rest bends nothing, and the drawing is drawn the plain way.
    let warp = placed.deformer.and_then(|_| shot.warp_of(layer, frame)).filter(|warp| !warp.is_still());
    let (drawing, warp) = bent_points(drawing, warp);
    // Of one art layer's output, the others are not drawn either.
    let off = match placed.art {
        None => std::borrow::Cow::Borrowed(layer.art_off.as_slice()),
        Some(_) => std::borrow::Cow::Owned(placed.off(&layer.art_off)),
    };
    let seen = Seen { to_frame: placed.projection?, bounds: drawing.bounds(), drawing, warp, off };
    // What is seen of it is what its art layers that are on have painted.
    let bounds = if seen.off.is_empty() { seen.bounds } else { seen.shown().bounds() };
    Some(Seen { bounds, ..seen }).filter(|seen| !seen.bounds.is_empty())
}

/// How many drawings boiled are kept, to be found again when their turn comes round.
const BOILED_KEPT: usize = 256;

/// A drawing as it is drawn on `frame` where shapes of its vector art boil (the user's of
/// 2026-11-04): a copy with each of them the way its turn there says, or the drawing itself
/// where none does or each is in its first turn. The copies are kept by drawing and by the
/// turns of its shapes, so a turn that comes round again is the same copy, which a
/// compositor's cache knows.
pub fn boiled(drawing: &Arc<Drawing>, frame: u32) -> Arc<Drawing> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    type Kept = HashMap<(usize, Vec<(u32, u32)>), (Arc<Drawing>, Arc<Drawing>)>;
    static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
    // Each way its shapes boil, and which turn that is on the frame.
    let mut turns: Vec<(u32, u32)> = Vec::new();
    for art in &drawing.art_layers {
        if let iskra_model::ArtContent::Vector(vector) = &art.content {
            for boil in vector.shapes().iter().filter_map(|shape| shape.kind.boil()).filter(|boil| boil.amount > 0.0) {
                turns.push((boil.every, boil.turn(frame)));
            }
        }
    }
    turns.sort_unstable();
    turns.dedup();
    if turns.iter().all(|(_, turn)| *turn == 0) {
        return drawing.clone();
    }
    let key = (Arc::as_ptr(drawing) as usize, turns);
    let mut kept = KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let kept = kept.get_or_insert_with(HashMap::new);
    if let Some((_, boiled)) = kept.get(&key) {
        return boiled.clone();
    }
    let mut boiled = Drawing::clone(drawing);
    for art in &mut boiled.art_layers {
        if let iskra_model::ArtContent::Vector(vector) = &art.content
            && let Some(turned) = vector.boiled(frame)
        {
            art.content = iskra_model::ArtContent::Vector(turned);
        }
    }
    let boiled = Arc::new(boiled);
    if kept.len() >= BOILED_KEPT {
        kept.clear();
    }
    // The drawing is kept too, so that no other is made at its address while its copy is.
    kept.insert(key, (drawing.clone(), boiled.clone()));
    boiled
}

/// How long the pieces are, in the drawing's pixels, that vector art's paths are cut into before
/// a deformer moves their points: short enough that a line bends smoothly across a joint.
const BENT_STEP: f64 = 4.0;

/// How many drawings bent point by point are kept, to be found again on the next frame.
const BENT_KEPT: usize = 256;

/// A drawing as a deformer's `warp` bends it, if that is by its points (plan §4.16, step 6):
/// a drawing whose art is all vector art, or empty, is given back bent point by point, as a
/// copy, with no warp left to do; any other is given back as it is, with the warp, which bends
/// its pixels through the mesh, vector art with them. The copies are kept by drawing and warp,
/// so the same drawing bent the same way is the same copy, which a compositor's cache knows.
pub fn bent_points(drawing: &Arc<Drawing>, warp: Option<Warp>) -> (Arc<Drawing>, Option<Warp>) {
    use std::collections::HashMap;
    use std::sync::Mutex;
    type Kept = HashMap<(usize, u64), (Arc<Drawing>, Arc<Drawing>)>;
    static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
    let by_points = |drawing: &Drawing| {
        drawing.art_layers.iter().any(|art| art.content.is_vector()) && drawing.art_layers.iter().all(|art| art.content.is_vector() || art.content.is_empty())
    };
    let warp = match warp {
        Some(warp) if by_points(drawing) => warp,
        other => return (drawing.clone(), other),
    };
    let key = (Arc::as_ptr(drawing) as usize, warp.key());
    let mut kept = KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let kept = kept.get_or_insert_with(HashMap::new);
    if let Some((_, bent)) = kept.get(&key) {
        return (bent.clone(), None);
    }
    let move_point = |at: iskra_model::kurbo::Point| {
        let to = warp.apply(Point::new(at.x, at.y));
        iskra_model::kurbo::Point::new(to.x, to.y)
    };
    let mut bent = Drawing::clone(drawing);
    for art in &mut bent.art_layers {
        if let iskra_model::ArtContent::Vector(vector) = &art.content {
            art.content = iskra_model::ArtContent::Vector(vector.bent(BENT_STEP, move_point));
        }
    }
    let bent = Arc::new(bent);
    if kept.len() >= BENT_KEPT {
        kept.clear();
    }
    // The drawing is kept too, so that no other is made at its address while its copy is.
    kept.insert(key, (drawing.clone(), bent.clone()));
    (bent, None)
}

/// How many pixels the copies of vector art made larger may have, each, at most: past it,
/// they are made as large as this allows.
const MOST_SHARPENED: f64 = 16_000_000.0;

/// How many larger copies of vector art are kept, to be found again on the next frame.
const SHARPENED_KEPT: usize = 128;

/// A drawing seen larger than its pixels, when it is all vector art (or empty), drawn at the
/// size it is seen instead (plan §4.16, step 10): its shapes made larger by the power of two at
/// or above how many of the picture's pixels one of its pixels covers, and placed smaller by
/// as much, so that it lies where it did with as many pixels as it is seen with. `from_frame`
/// is the picture's, from the frame. A drawing seen at its size or smaller, one in
/// perspective, one with pixels, or one a deformer bends through its mesh is as it was. The
/// copies are kept by drawing and size, so that the next frame finds the same one.
pub fn sharp<'a>(seen: Seen<'a>, from_frame: &Affine) -> Seen<'a> {
    match sharpened(&seen.drawing, seen.warp.is_some(), &seen.to_frame, from_frame) {
        Some((drawing, to_frame)) => Seen { to_frame, bounds: drawing.bounds(), drawing, ..seen },
        None => seen,
    }
}

/// [`sharp`] of a drawing placed by `to_frame` and seen through `from_frame`: the larger copy
/// and its placing, or `None` where it is drawn as it is. `bent` says a deformer bends it
/// through its mesh.
pub fn sharpened(drawing: &Arc<Drawing>, bent: bool, to_frame: &Projection, from_frame: &Affine) -> Option<(Arc<Drawing>, Projection)> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    type Kept = HashMap<(usize, u32), (Arc<Drawing>, Arc<Drawing>)>;
    static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
    let all_vector = |drawing: &Drawing| {
        drawing.art_layers.iter().any(|art| art.content.is_vector()) && drawing.art_layers.iter().all(|art| art.content.is_vector() || art.content.is_empty())
    };
    let flat = to_frame.then(from_frame).as_flat().filter(|_| !bent && all_vector(drawing))?;
    // How many of the picture's pixels a side one of the drawing's covers.
    let seen_as = flat.determinant().abs().sqrt();
    if seen_as <= 1.25 {
        return None;
    }
    let bounds = drawing.bounds();
    let area = f64::from(bounds.width()) * f64::from(bounds.height());
    let most = (MOST_SHARPENED / area.max(1.0)).sqrt();
    let larger = (seen_as.min(most).log2().ceil().max(0.0) as u32).min(4);
    if larger == 0 {
        return None;
    }
    let times = f64::from(1u32 << larger);
    let key = (Arc::as_ptr(drawing) as usize, larger);
    let mut kept = KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let kept = kept.get_or_insert_with(HashMap::new);
    let copy = match kept.get(&key) {
        Some((_, made)) => made.clone(),
        None => {
            let mut made = Drawing::clone(drawing);
            for art in &mut made.art_layers {
                if let iskra_model::ArtContent::Vector(vector) = &art.content {
                    art.content = iskra_model::ArtContent::Vector(vector.transformed(&Affine::scale(times)));
                }
            }
            let made = Arc::new(made);
            if kept.len() >= SHARPENED_KEPT {
                kept.clear();
            }
            // The drawing is kept too, so that no other is made at its address while this is.
            kept.insert(key, (drawing.clone(), made.clone()));
            made
        }
    };
    Some((copy, to_frame.of(&Affine::scale(1.0 / times))))
}

/// How many points a side of each pixel of the picture a drawing is sampled at, when one
/// pixel of the picture is `shrunk` drawing pixels across.
pub fn points(shrunk: f64) -> u32 {
    (shrunk - 0.01).ceil().clamp(1.0, 8.0) as u32
}

/// How a drawing is sampled into a picture: where in the picture it can be, how a point of
/// the picture goes back to the drawing, and at how many points a side each pixel is read.
/// Every compositor samples by this, so that all make the same picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sampling {
    /// The picture's pixels the drawing can reach: x0, y0, x1, y1.
    pub reach: [usize; 4],
    /// From the picture back to the drawing (see [`Projection::back`]). For a flat drawing the
    /// last row is (0, 0, 1).
    pub back: [[f64; 3]; 3],
    pub points: u32,
    /// The drawing is seen in perspective.
    pub tilted: bool,
}

impl Sampling {
    /// How `bounds` of a drawing that `to_picture` puts into a picture of `width` by `height`
    /// pixels is sampled. `None` if none of it can be seen there.
    pub fn of(to_picture: &Projection, bounds: Rect, width: usize, height: usize) -> Option<Sampling> {
        let within = |value: f64, most: usize| (value.max(0.0) as usize).min(most);
        let reach_of = |low: [f64; 2], high: [f64; 2]| {
            let reach = [
                within(low[0].floor() - 1.0, width),
                within(low[1].floor() - 1.0, height),
                within(high[0].ceil() + 1.0, width),
                within(high[1].ceil() + 1.0, height),
            ];
            (reach[0] < reach[2] && reach[1] < reach[3]).then_some(reach)
        };
        if let Some(flat) = to_picture.as_flat() {
            let to_drawing = flat.inverse()?;
            let [a, b, c, d, e, f] = to_drawing.m;
            let (low, high) = spread(&flat, bounds);
            // How many drawing pixels one pixel of the picture is across.
            let points = points(to_drawing.determinant().abs().sqrt());
            return Some(Sampling { reach: reach_of(low, high)?, back: [[a, c, e], [b, d, f], [0.0, 0.0, 1.0]], points, tilted: false });
        }
        let back = to_picture.back()?;
        let corners = [(bounds.x0, bounds.y0), (bounds.x1, bounds.y0), (bounds.x0, bounds.y1), (bounds.x1, bounds.y1)]
            .map(|(x, y)| Point::new(f64::from(x), f64::from(y)));
        let seen: Vec<(Point, f64)> = corners.iter().filter_map(|corner| Some((to_picture.apply(*corner)?, to_picture.away(*corner)))).collect();
        // A card that reaches past the camera can be anywhere in the picture.
        let reach = if seen.len() == corners.len() {
            let low = seen.iter().fold([f64::INFINITY; 2], |low, (at, _)| [low[0].min(at.x), low[1].min(at.y)]);
            let high = seen.iter().fold([f64::NEG_INFINITY; 2], |high, (at, _)| [high[0].max(at.x), high[1].max(at.y)]);
            reach_of(low, high)?
        } else if seen.is_empty() {
            return None;
        } else {
            [0, 0, width, height]
        };
        // It is shrunk most where it is farthest: there one pixel of the picture is the most
        // drawing pixels across. Of the corners that are seen, the farthest says how many points.
        let det = back[0][0] * (back[1][1] * back[2][2] - back[1][2] * back[2][1]) - back[0][1] * (back[1][0] * back[2][2] - back[1][2] * back[2][0])
            + back[0][2] * (back[1][0] * back[2][1] - back[1][1] * back[2][0]);
        let shrunk = seen.iter().map(|(_, away)| (det.abs() * away.powi(3)).sqrt()).fold(0.0, f64::max);
        Some(Sampling { reach, back, points: points(shrunk), tilted: true })
    }

    /// The point of the drawing that is seen at a point of the picture. `None` where the
    /// drawing's plane is at the camera or behind it.
    pub fn drawing_at(&self, x: f64, y: f64) -> Option<Point> {
        let [u, v, s] = self.back.map(|row| row[0] * x + row[1] * y + row[2]);
        if !self.tilted {
            return Some(Point::new(u, v));
        }
        // `s` is one over how far in front of the camera the point is.
        (s > 0.0 && s * NEAREST < 1.0).then(|| Point::new(u / s, v / s))
    }
}

fn pictures(shot: &Shot, frame: u32) -> Vec<(Projection, Arc<Drawing>, Rect, Option<Warp>)> {
    seen(shot, frame).into_iter().map(|seen| (seen.to_frame, seen.drawing, seen.bounds, seen.warp)).collect()
}

/// How many drawing pixels a side the cells of a deformed drawing's mesh are, at least.
const CELL: u32 = 4;
/// How many cells a side the mesh of a deformed drawing has, at most.
const CELLS: u32 = 96;

/// The mesh a deformed drawing is bent through (plan §4.5): a grid over what is painted,
/// each cell two triangles, with every corner where the drawing has it and where the deformer
/// puts it. The triangles are given by their three corners, as indexes into the corners.
pub struct Mesh {
    /// For each corner: where it is in the drawing, and where it is bent to, both in the
    /// drawing's pixels.
    pub corners: Vec<(Point, Point)>,
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    pub fn of(bounds: Rect, warp: &Warp) -> Mesh {
        let cell = CELL.max(bounds.width().max(bounds.height()).div_ceil(CELLS)).max(1);
        let (across, down) = (bounds.width().div_ceil(cell).max(1), bounds.height().div_ceil(cell).max(1));
        let mut corners = Vec::with_capacity(((across + 1) * (down + 1)) as usize);
        for row in 0..=down {
            for column in 0..=across {
                let side = |low: i32, high: i32, cells: u32| f64::from(low) + f64::from(cells * cell).min(f64::from(high) - f64::from(low));
                let at = Point::new(side(bounds.x0, bounds.x1, column), side(bounds.y0, bounds.y1, row));
                corners.push((at, warp.apply(at)));
            }
        }
        let mut triangles = Vec::with_capacity((across * down * 2) as usize);
        for row in 0..down {
            for column in 0..across {
                let first = row * (across + 1) + column;
                let (right, below) = (first + 1, first + across + 1);
                triangles.extend([[first, right, below], [right, below + 1, below]]);
            }
        }
        Mesh { corners, triangles }
    }

    /// The pixels the mesh is bent over, in the drawing's: `None` where it can't be bent, for
    /// a bend that throws it to no place or over more than the most pixels a bent drawing is
    /// made of. Then the drawing is drawn as it was at rest.
    pub fn bent(&self) -> Option<Rect> {
        let low = self.corners.iter().fold([f64::INFINITY; 2], |low, (_, at)| [low[0].min(at.x), low[1].min(at.y)]);
        let high = self.corners.iter().fold([f64::NEG_INFINITY; 2], |high, (_, at)| [high[0].max(at.x), high[1].max(at.y)]);
        let fits = low.iter().chain(&high).all(|side| side.is_finite() && side.abs() < 1e9);
        let (x0, y0, x1, y1) = (low[0].floor(), low[1].floor(), high[0].ceil(), high[1].ceil());
        (fits && (x1 - x0) * (y1 - y0) <= MOST_BENT as f64 && x1 > x0 && y1 > y0).then(|| Rect::new(x0 as i32, y0 as i32, x1 as i32, y1 as i32))
    }
}

/// A drawing as a deformer has bent it: its pixels, straight alpha in sRGB, in rows from the
/// top of `bounds`, which are in the drawing's own pixels. It is drawn in place of the
/// drawing, by every compositor, so that all draw the same.
pub struct Bent {
    pub bounds: Rect,
    pub pixels: Vec<[u8; 4]>,
}

/// How far outside a triangle of the mesh a point still counts as on its edge, as a part of
/// the triangle.
const EDGE: f64 = 1e-7;

/// Fewer pixels than this are bent on one thread: more threads would take longer to start.
const BENT_ALONE: usize = 1 << 16;

/// The most pixels a bent drawing is made of: beyond that it is drawn as it was at rest.
const MOST_BENT: u64 = 1 << 26;

/// Bend a drawing (plan §4.5): each triangle of its mesh is drawn where the deformer puts
/// it, showing the part of the drawing the triangle has at rest, read between its pixels. A
/// point that two triangles cover, where the drawing folds over itself, shows the first.
pub fn bend(drawing: &Drawing, bounds: Rect, palette: &Palette, warp: &Warp) -> Bent {
    bend_on(drawing, bounds, palette, warp, std::thread::available_parallelism().map_or(1, usize::from))
}

/// [`bend`] on as many threads as `cores` says, at most.
fn bend_on(drawing: &Drawing, bounds: Rect, palette: &Palette, warp: &Warp, cores: usize) -> Bent {
    // Colour times alpha, as painted: between pixels that is what is mixed.
    let flat = Flat::of(drawing, bounds, palette, WorkingSpace::Srgb);
    // A pixel more all round, for the half pixel that the edge of what is painted fades over.
    let mesh = Mesh::of(bounds.inflate(1), warp);
    let Some(bent) = mesh.bent() else {
        return Bent { bounds, pixels: palette.paint(drawing, bounds, WorkingSpace::Srgb) };
    };
    let (x0, y0) = (f64::from(bent.x0), f64::from(bent.y0));
    let (wide, high) = (bent.width() as usize, bent.height() as usize);
    let mut pixels = vec![[0u8; 4]; wide * high];
    // The rows `from` on, of which `band` holds as many as it is long: each triangle in turn,
    // so that where the drawing folds over itself the first is shown in every band alike.
    let fill = |from: usize, band: &mut [[u8; 4]]| {
        let rows = band.len() / wide.max(1);
        let mut taken = vec![false; band.len()];
        for [a, b, c] in &mesh.triangles {
            let [(ra, pa), (rb, pb), (rc, pc)] = [mesh.corners[*a as usize], mesh.corners[*b as usize], mesh.corners[*c as usize]];
            let area = (pb.x - pa.x) * (pc.y - pa.y) - (pb.y - pa.y) * (pc.x - pa.x);
            if area.abs() < 1e-9 {
                continue;
            }
            // The pixels whose middles the triangle can cover, of this band's rows.
            let within = |value: f64, low: f64, most: usize| ((value - low).max(0.0) as usize).min(most);
            let (tx0, tx1) = (within(pa.x.min(pb.x).min(pc.x).floor(), x0, wide), within(pa.x.max(pb.x).max(pc.x).ceil(), x0, wide));
            let (ty0, ty1) = (within(pa.y.min(pb.y).min(pc.y).floor(), y0, high), within(pa.y.max(pb.y).max(pc.y).ceil(), y0, high));
            for y in ty0.max(from)..ty1.min(from + rows) {
                for x in tx0..tx1 {
                    let at = (y - from) * wide + x;
                    if taken[at] {
                        continue;
                    }
                    let (px, py) = (x0 + x as f64 + 0.5, y0 + y as f64 + 0.5);
                    // How much of each corner the point is: all three at least nothing inside.
                    let wb = ((px - pa.x) * (pc.y - pa.y) - (py - pa.y) * (pc.x - pa.x)) / area;
                    let wc = ((pb.x - pa.x) * (py - pa.y) - (pb.y - pa.y) * (px - pa.x)) / area;
                    let wa = 1.0 - wb - wc;
                    // A point on the line between two triangles is in the first, whichever way
                    // the last digit of the sums falls: never in neither.
                    if wa < -EDGE || wb < -EDGE || wc < -EDGE {
                        continue;
                    }
                    taken[at] = true;
                    let [r, g, b, alpha] = flat.sample(Point::new(wa * ra.x + wb * rb.x + wc * rc.x, wa * ra.y + wb * rb.y + wc * rc.y));
                    if alpha > 0.0 {
                        let straight = |value: f32| ((value / alpha).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        band[at] = [straight(r), straight(g), straight(b), (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8];
                    }
                }
            }
        }
    };
    // A large drawing is bent in bands of rows, one to each of the processor's cores: the
    // pixels are where the time goes (item 29 of the milestones). A small one is not worth it.
    let rows = if wide * high < BENT_ALONE || cores < 2 { high } else { high.div_ceil(cores) };
    if rows >= high {
        fill(0, &mut pixels);
    } else {
        std::thread::scope(|scope| {
            for (band, rows_of) in pixels.chunks_mut(rows * wide).enumerate() {
                let fill = &fill;
                scope.spawn(move || fill(band * rows, rows_of));
            }
        });
    }
    Bent { bounds: bent, pixels }
}

/// The corners of `bounds` as `to` puts them: the least and the greatest of each coordinate.
fn spread(to: &Affine, bounds: Rect) -> ([f64; 2], [f64; 2]) {
    let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for (x, y) in [(bounds.x0, bounds.y0), (bounds.x1, bounds.y0), (bounds.x0, bounds.y1), (bounds.x1, bounds.y1)] {
        let corner = to.apply(Point::new(f64::from(x), f64::from(y)));
        (low, high) = ([low[0].min(corner.x), low[1].min(corner.y)], [high[0].max(corner.x), high[1].max(corner.y)]);
    }
    (low, high)
}

/// The part of what the camera sees on `frame` that has something painted: its least and
/// greatest coordinates, in the frame's units from the frame's middle. `None` if nothing
/// painted is seen there.
pub fn painted(shot: &Shot, frame: u32) -> Option<([f64; 2], [f64; 2])> {
    // A tilted drawing is reckoned by its corners that are seen.
    let spreads = pictures(shot, frame).into_iter().filter_map(|(to_frame, _, bounds, _)| match to_frame.as_flat() {
        Some(flat) => Some(spread(&flat, bounds)),
        None => {
            let corners = [(bounds.x0, bounds.y0), (bounds.x1, bounds.y0), (bounds.x0, bounds.y1), (bounds.x1, bounds.y1)];
            let seen = corners.into_iter().filter_map(|(x, y)| to_frame.apply(Point::new(f64::from(x), f64::from(y))));
            seen.map(|at| ([at.x, at.y], [at.x, at.y]))
                .reduce(|(low, high), (l, h)| ([low[0].min(l[0]), low[1].min(l[1])], [high[0].max(h[0]), high[1].max(h[1])]))
        }
    });
    spreads.reduce(|(low, high), (l, h)| ([low[0].min(l[0]), low[1].min(l[1])], [high[0].max(h[0]), high[1].max(h[1])]))
}

/// An sRGB value, 0 to 1, in linear light.
fn to_linear(value: f32) -> f32 {
    if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
}

/// A value in linear light, 0 to 1, as sRGB.
fn to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 { value * 12.92 } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 }
}

/// A drawing's painted pixels, flattened, ready to be sampled: colour times alpha, and alpha,
/// in the working space.
pub(crate) struct Flat {
    pub(crate) bounds: Rect,
    pixels: Vec<[f32; 4]>,
}

impl Flat {
    fn of(drawing: &Drawing, bounds: Rect, palette: &Palette, space: WorkingSpace) -> Self {
        // A large drawing in bands of rows, one to each of the processor's cores.
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        let (wide, high) = (bounds.width() as usize, bounds.height() as usize);
        if wide * high < BENT_ALONE || cores < 2 {
            return Self::of_pixels(bounds, palette.paint(drawing, bounds, space), space);
        }
        // Its gradients' and pictures' areas are found once, for every band.
        let areas = &palette.areas_of(drawing);
        let rows = high.div_ceil(cores) as i32;
        let bands: Vec<Rect> =
            (bounds.y0..bounds.y1).step_by(rows as usize).map(|top| Rect::new(bounds.x0, top, bounds.x1, (top + rows).min(bounds.y1))).collect();
        let made: Vec<Vec<[f32; 4]>> = std::thread::scope(|scope| {
            let threads: Vec<_> =
                bands.iter().map(|band| scope.spawn(move || Self::of_pixels(*band, palette.paint_in(drawing, *band, areas, space), space).pixels)).collect();
            threads.into_iter().map(|thread| thread.join().unwrap_or_default()).collect()
        });
        Self { bounds, pixels: made.concat() }
    }

    /// Of pixels with straight alpha, in sRGB, in rows from the top of `bounds`.
    fn of_pixels(bounds: Rect, straight: Vec<[u8; 4]>, space: WorkingSpace) -> Self {
        let pixels = straight.into_iter().map(|[r, g, b, a]| {
            let alpha = f32::from(a) / 255.0;
            let light = |value: u8| match space {
                WorkingSpace::Srgb => f32::from(value) / 255.0,
                WorkingSpace::Linear => to_linear(f32::from(value) / 255.0),
            };
            [light(r) * alpha, light(g) * alpha, light(b) * alpha, alpha]
        });
        Self { bounds, pixels: pixels.collect() }
    }

    /// The pixel at (x, y) of the drawing; clear outside what is painted.
    fn at(&self, x: i64, y: i64) -> [f32; 4] {
        let (x, y) = (x - i64::from(self.bounds.x0), y - i64::from(self.bounds.y0));
        let (width, height) = (i64::from(self.bounds.width()), i64::from(self.bounds.height()));
        if x < 0 || y < 0 || x >= width || y >= height { [0.0; 4] } else { self.pixels[(y * width + x) as usize] }
    }

    /// The colour at a point of the drawing, between the four pixels around it. A point in the
    /// middle of a pixel has that pixel's colour exactly.
    pub(crate) fn sample(&self, at: Point) -> [f32; 4] {
        let (u, v) = (at.x - 0.5, at.y - 0.5);
        let (x, y) = (u.floor(), v.floor());
        let (fx, fy) = ((u - x) as f32, (v - y) as f32);
        let (x, y) = (x as i64, y as i64);
        if fx == 0.0 && fy == 0.0 {
            return self.at(x, y);
        }
        let (a, b, c, d) = (self.at(x, y), self.at(x + 1, y), self.at(x, y + 1), self.at(x + 1, y + 1));
        std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    }
}

/// Render what `shot` shows on `frame`, as `view` sees it, over transparency. `palette` gives
/// the swatches their colours, and the layers are put over each other in `space`.
///
/// Each pixel of the picture is the mean of a few points in it, more of them the more the
/// drawing is shrunk, so that thin lines neither vanish nor flicker; a drawing that is
/// enlarged is smoothed between its pixels. A drawing shown pixel for pixel comes out as it
/// was painted.
pub fn render(shot: &Shot, palette: &Palette, space: WorkingSpace, frame: u32, view: &View) -> Image {
    finish(view.width, view.height, &composite(shot, palette, space, frame, view), space)
}

/// What goes through a Style node as it draws it in its style (plan §4.8): each layer's
/// drawing in the skin of the style's name, or with no name the day drawing. Where no skin has
/// that name, as the shot shows it.
pub fn in_style_skin<'a>(
    shot: &Shot,
    palette: &Palette,
    effect: &iskra_model::Effect,
    inside: &'a [iskra_eval::Card],
) -> std::borrow::Cow<'a, [iskra_eval::Card]> {
    let name = effect.text(iskra_model::STYLE);
    let skin = match name.trim() {
        "" => None,
        name => match palette.skin_named(name) {
            Some(skin) => Some(skin.id),
            None => return std::borrow::Cow::Borrowed(inside),
        },
    };
    if skin == shot.skin {
        return std::borrow::Cow::Borrowed(inside);
    }
    std::borrow::Cow::Owned(inside.iter().map(|card| card.in_skin(shot, skin)).collect())
}

/// [`render`], as the compositor has the picture before it is handed over at 8 bits: for each
/// pixel, in rows from the top, its colour times its alpha, and its alpha, in `space`. This is
/// what an export at a higher depth is made from (R24).
pub fn composite(shot: &Shot, palette: &Palette, space: WorkingSpace, frame: u32, view: &View) -> Vec<[f32; 4]> {
    // In the colours of the palette style the shot is shown in.
    let palette = &palette.styled(&shot.styles);
    let mut picture = vec![[0.0f32; 4]; view.width as usize * view.height as usize];
    let evaluated = iskra_eval::evaluate(shot, frame);
    draw(shot, palette, space, frame, view, &evaluated.cards, &mut picture, None);
    picture
}

/// One card of an evaluated frame as a picture of its own, over transparency, as `view` sees
/// it: what goes through an effect together, changed by it. This is what a window shows of an
/// effect while the rest of the shot is drawn from its tiles.
pub fn card(shot: &Shot, palette: &Palette, space: WorkingSpace, frame: u32, view: &View, card: &iskra_eval::Card) -> Image {
    let palette = &palette.styled(&shot.styles);
    let mut picture = vec![[0.0f32; 4]; view.width as usize * view.height as usize];
    draw(shot, palette, space, frame, view, std::slice::from_ref(card), &mut picture, None);
    finish(view.width, view.height, &picture, space)
}

/// What an effect made: the part of the view where it changes anything, as a picture of its
/// own, colour times alpha, in rows from the top. Outside it, it leaves what is under it.
pub struct Made {
    pub left: usize,
    pub top: usize,
    pub width: usize,
    pub pixels: Vec<[f32; 4]>,
}

impl Made {
    /// How many rows it is.
    pub fn height(&self) -> usize {
        self.pixels.len().checked_div(self.width).unwrap_or(0)
    }

    /// Lay it onto `picture`, a picture `across` pixels wide, by a Blend's `mode`: over it,
    /// for any other mode (see [`effects::laid`]).
    fn lay(&self, picture: &mut [[f32; 4]], across: usize, mode: &str) {
        if self.width == 0 {
            return;
        }
        for (row, made) in self.pixels.chunks(self.width).enumerate() {
            let at = (self.top + row) * across + self.left;
            for (under, over) in picture[at..at + self.width].iter_mut().zip(made) {
                *under = effects::laid(mode, *over, *under);
            }
        }
    }
}

/// The least box round what is painted of a picture `width` pixels wide: its left, top,
/// right and bottom, the right and bottom past it. `None` where nothing is.
fn painted_box(picture: &[[f32; 4]], width: usize) -> Option<[usize; 4]> {
    let rows: Vec<usize> = picture.chunks(width).enumerate().filter(|(_, row)| row.iter().any(|pixel| pixel[3] != 0.0)).map(|(y, _)| y).collect();
    let (&top, &bottom) = (rows.first()?, rows.last()?);
    let (mut left, mut right) = (width, 0);
    for row in picture.chunks(width).skip(top).take(bottom + 1 - top) {
        if let Some(first) = row.iter().position(|pixel| pixel[3] != 0.0) {
            left = left.min(first);
        }
        if let Some(last) = row.iter().rposition(|pixel| pixel[3] != 0.0) {
            right = right.max(last + 1);
        }
    }
    Some([left, top, right, bottom + 1])
}

/// The rows from `top` to `bottom` and columns from `left` to `right` of a picture `width`
/// pixels wide, as a picture of their own.
fn cropped(picture: &[[f32; 4]], width: usize, [left, top, right, bottom]: [usize; 4]) -> Vec<[f32; 4]> {
    (top..bottom).flat_map(|y| &picture[y * width + left..y * width + right]).copied().collect()
}

/// What goes through an effect node, as `card` has it, changed by the effect, as `view`
/// sees it: found in the cache where it was made before, else made now and kept there.
/// `None` for a card that is no effect node's, or of one the shot has not got.
fn through(
    shot: &Shot,
    palette: &Palette,
    space: WorkingSpace,
    frame: u32,
    view: &View,
    card: &iskra_eval::Card,
    cache: Option<(&Cache, u64)>,
) -> Option<Arc<Made>> {
    let iskra_eval::Card::Through { effect, inside, mattes, plane, .. } = card else { return None };
    let effect = shot.effect(*effect)?;
    let (width, height) = (view.width as usize, view.height as usize);
    let key = cache.and_then(|(_, colours)| cache::Key::of(colours, palette, space, shot, frame, view, card));
    let splash_sprites = sprites(shot, palette, space, frame, view, effect, mattes, cache, iskra_model::SPLASH_SPRITE);
    let splash_play = splash_play(shot, effect, mattes);
    let sprites = sprites(shot, palette, space, frame, view, effect, mattes, cache, iskra_model::SPRITE);
    if let Some(found) = cache.zip(key.as_ref()).and_then(|((cache, _), key)| cache.find(key)) {
        return Some(found);
    }
    // What goes through is drawn with room round it, as much as the effect reaches: a blur at
    // the frame's edge is of what lies past the edge too.
    let scale = view.scale();
    let room = effects::reach(effect, frame, scale, width.max(height));
    let wide = view.with_room(room);
    let (across, down) = (wide.width as usize, wide.height as usize);
    let mut through = vec![[0.0f32; 4]; across * down];
    draw(shot, palette, space, frame, &wide, inside, &mut through, cache);
    // A Style node's picture is what goes through drawn again in the style it names, with
    // colours of its own in the cache.
    let styled = (effect.kind == iskra_model::STYLE).then(|| {
        let restyled = palette.restyled(&effect.text(iskra_model::STYLE_PALETTE), &effect.text(iskra_model::STYLE));
        let mut styled = vec![[0.0f32; 4]; across * down];
        let cache = cache.map(|(cache, _)| (cache, cache.colours(&restyled, space)));
        // And in the skin of its name.
        let inside = in_style_skin(shot, palette, effect, inside);
        draw(shot, &restyled, space, frame, &wide, &inside, &mut styled, cache);
        styled
    });
    // An effect that changes only what goes through it, by what lies as far round it as it
    // reaches, is worked out there alone: the box round what is painted, and its reach round
    // that. The rest of the frame it leaves as it is, and with nothing painted it makes nothing.
    let painted = |picture: &[[f32; 4]]| painted_box(picture, across);
    let painted = match &styled {
        // A style may make solid what was not, or the other way round.
        Some(styled) => match (painted(&through), painted(styled)) {
            (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]),
            (a, b) => a.or(b),
        },
        None => painted(&through),
    };
    let region = match effects::confined(effect) {
        true => painted.map(|[x0, y0, x1, y1]| [x0.saturating_sub(room), y0.saturating_sub(room), (x1 + room).min(across), (y1 + room).min(down)]),
        false => Some([0, 0, across, down]),
    };
    // And only what falls inside the view, without its room again.
    let shown = region.map(|[x0, y0, x1, y1]| [x0.max(room), y0.max(room), x1.min(room + width), y1.min(room + height)]);
    let made = match (region, shown) {
        (Some(region @ [x0, y0, x1, y1]), Some([sx0, sy0, sx1, sy1])) if sx0 < sx1 && sy0 < sy1 => {
            // The region's own view: the wide one, moved so that the region's corner is its
            // origin.
            let part = View {
                width: (x1 - x0) as u32,
                height: (y1 - y0) as u32,
                from_frame: Affine::translate(-(x0 as f64), -(y0 as f64)).mul(&wide.from_frame),
                frame: wide.frame,
            };
            // What a sprite port takes is drawn as particles, not as a matte of the frame.
            let mut mattes: Vec<(&str, Vec<[f32; 4]>)> = mattes
                .iter()
                .filter(|(port, _)| ![iskra_model::SPRITE, iskra_model::SPLASH_SPRITE, iskra_model::COLLIDER].contains(&port.as_str()))
                .map(|(port, cards)| {
                    let mut matte = vec![[0.0f32; 4]; (x1 - x0) * (y1 - y0)];
                    draw(shot, palette, space, frame, &part, cards, &mut matte, cache);
                    (port.as_str(), matte)
                })
                .collect();
            let whole = region == [0, 0, across, down];
            if let Some(styled) = styled {
                mattes.push((effects::STYLED, if whole { styled } else { cropped(&styled, across, region) }));
            }
            let through = if whole { through } else { cropped(&through, across, region) };
            let camera = |depth: f64, at: u32| shot.seen(depth, at);
            let stepped = simulation::stepped(shot, palette, space, effect, particles::held(effect, frame), view.frame);
            let given = effects::Given {
                width: part.width,
                height: part.height,
                scale,
                from_frame: &part.from_frame,
                mattes: &mattes,
                space,
                frame: view.frame,
                sprites: &sprites,
                splash_sprites: &splash_sprites,
                splash_play: &splash_play,
                camera: &camera,
                plane: *plane,
                stepped: stepped.as_ref().map(|live| live.as_slice()),
            };
            let made = effects::apply(effect, frame, &given, through);
            let kept = cropped(&made, x1 - x0, [sx0 - x0, sy0 - y0, sx1 - x0, sy1 - y0]);
            Made { left: sx0 - room, top: sy0 - room, width: sx1 - sx0, pixels: kept }
        }
        _ => Made { left: 0, top: 0, width: 0, pixels: Vec::new() },
    };
    let made = Arc::new(made);
    if let Some(((cache, _), key)) = cache.zip(key) {
        cache.keep(key, made.clone());
    }
    Some(made)
}

/// What a Particles node's `card` draws on `frame`, as `view` sees it, worked out and not laid
/// onto a picture: what [`through`] lays. `None` for a card of another kind of node.
fn particles_of(
    shot: &Shot,
    palette: &Palette,
    space: WorkingSpace,
    frame: u32,
    view: &View,
    card: &iskra_eval::Card,
    cache: Option<(&Cache, u64)>,
) -> Option<Particles> {
    let iskra_eval::Card::Through { effect, mattes, plane, .. } = card else { return None };
    let effect = shot.effect(*effect).filter(|effect| effect.kind == iskra_model::PARTICLES)?;
    let splash_sprites = sprites(shot, palette, space, frame, view, effect, mattes, cache, iskra_model::SPLASH_SPRITE);
    let splash_play = splash_play(shot, effect, mattes);
    let sprites = sprites(shot, palette, space, frame, view, effect, mattes, cache, iskra_model::SPRITE);
    let camera = |depth: f64, at: u32| shot.seen(depth, at);
    let stepped = simulation::stepped(shot, palette, space, effect, particles::held(effect, frame), view.frame);
    let place = particles::Place {
        size: view.frame,
        scale: view.scale(),
        from_frame: &view.from_frame,
        camera: &camera,
        plane: *plane,
        stepped: stepped.as_ref().map(|live| live.as_slice()),
    };
    Some(particles::particles(effect, frame, &place, space, sprites, (splash_sprites, &splash_play)))
}

/// The layers whose drawings a Particles node draws its particles as, wired into its `port`
/// (its sprite port, or its splash's), where all that is wired there is layers. None for
/// another kind of node.
pub(crate) fn sprite_layers<'a>(
    shot: &'a Shot,
    effect: &iskra_model::Effect,
    mattes: &[(String, Vec<iskra_eval::Card>)],
    port: &str,
) -> Vec<&'a iskra_model::Layer> {
    let Some((_, cards)) = mattes.iter().find(|(wired, _)| wired.as_str() == port).filter(|_| effect.kind == iskra_model::PARTICLES) else {
        return Vec::new();
    };
    let mut layers: Vec<&iskra_model::Layer> = Vec::new();
    for card in cards.iter().flat_map(iskra_eval::Card::leaves) {
        let iskra_eval::Card::Layer(placed) = card else { return Vec::new() };
        if let Some(layer) = shot.layer(placed.layer).filter(|layer| !layers.iter().any(|seen| seen.id == layer.id)) {
            layers.push(layer);
        }
    }
    layers
}

/// How a splash plays the drawings of its layer, frame by frame from where a drop lands: the
/// splash sprite each frame shows, by its place among them, or none on a frame the layer
/// leaves empty. As the first layer wired into the splash port times its drawings, or with no
/// timing, each drawing for a frame. Empty where the splash has one drawing or none: then it
/// throws droplets of it up instead.
pub(crate) fn splash_play(shot: &Shot, effect: &iskra_model::Effect, mattes: &[(String, Vec<iskra_eval::Card>)]) -> Vec<Option<usize>> {
    let layers = sprite_layers(shot, effect, mattes, iskra_model::SPLASH_SPRITE);
    // The sprites' drawings, in the order `sprites` makes them.
    let ids: Vec<iskra_model::DrawingId> =
        layers.iter().flat_map(|layer| layer.drawings.iter()).filter(|entry| !entry.drawing.bounds().is_empty()).map(|entry| entry.id).collect();
    let Some(first) = layers.first().filter(|_| ids.len() > 1) else { return Vec::new() };
    let end = first.exposure.runs().last().map_or(0, |run| run.end()).min(1000);
    if end == 0 {
        return (0..ids.len()).map(Some).collect();
    }
    (0..end).map(|frame| first.exposure.at(frame).and_then(|drawing| ids.iter().position(|id| *id == drawing))).collect()
}

/// The pictures a Particles node draws its particles as, from its `port`: every drawing of the
/// layers wired there, as drawn, whatever their pegs do; or, where something else is wired
/// there, what it shows on the frame, about the frame's middle. None for another kind.
#[allow(clippy::too_many_arguments)]
fn sprites(
    shot: &Shot,
    palette: &Palette,
    space: WorkingSpace,
    frame: u32,
    view: &View,
    effect: &iskra_model::Effect,
    mattes: &[(String, Vec<iskra_eval::Card>)],
    cache: Option<(&Cache, u64)>,
    port: &str,
) -> Vec<particles::Sprite> {
    let Some((_, cards)) = mattes.iter().find(|(wired, _)| wired.as_str() == port).filter(|_| effect.kind == iskra_model::PARTICLES) else {
        return Vec::new();
    };
    let layers = sprite_layers(shot, effect, mattes, port);
    if !layers.is_empty() {
        let drawings = layers.iter().flat_map(|layer| layer.drawings.iter().map(move |entry| (&entry.drawing, layer.density)));
        let flat = drawings.filter_map(|(drawing, density)| {
            let bounds = drawing.bounds();
            (!bounds.is_empty()).then(|| particles::Sprite { flat: Flat::of(drawing, bounds, palette, space), per_unit: density })
        });
        return flat.collect();
    }
    // Anything else, drawn as the frame shows it, its middle the sprite's.
    let mut picture = vec![[0.0f32; 4]; view.width as usize * view.height as usize];
    draw(shot, palette, space, frame, view, cards, &mut picture, cache);
    let middle = view.from_frame.apply(Point::new(0.0, 0.0));
    let (left, top) = (-(middle.x.round() as i32), -(middle.y.round() as i32));
    let bounds = Rect::new(left, top, left + view.width as i32, top + view.height as i32);
    vec![particles::Sprite { flat: Flat { bounds, pixels: picture }, per_unit: view.scale() }]
}

/// Draw `cards` over `picture`, in their order. A layer's drawing goes straight onto it; what
/// goes through an effect is drawn on a picture of its own first, changed by the effect, and
/// then put over as one (plan §8.2). With a cache, the picture an effect made before of the
/// same things is found there, and one that is made is kept.
fn draw(
    shot: &Shot,
    palette: &Palette,
    space: WorkingSpace,
    frame: u32,
    view: &View,
    cards: &[iskra_eval::Card],
    picture: &mut [[f32; 4]],
    cache: Option<(&Cache, u64)>,
) {
    let (width, height) = (view.width as usize, view.height as usize);
    for card in cards.iter().flat_map(iskra_eval::Card::leaves) {
        let seen = match card {
            // A flat card is drawn as the cards in it, which are what `leaves` gave.
            iskra_eval::Card::Flat { .. } => None,
            // Vector art seen larger than its pixels is drawn at the size it is seen.
            iskra_eval::Card::Layer(placed) => seen_of(shot, frame, placed).map(|seen| sharp(seen, &view.from_frame)),
            iskra_eval::Card::Through { effect, .. } => {
                if let Some(made) = through(shot, palette, space, frame, view, card, cache) {
                    made.lay(picture, width, &blend_mode(shot, *effect, frame));
                }
                None
            }
        };
        let Some(seen) = seen else { continue };
        let shown = seen.shown().into_owned();
        let Seen { to_frame, bounds, warp, .. } = seen;
        let drawing: &Drawing = &shown;
        // A bent drawing is bent first, and then drawn as any other is.
        let bent = warp.map(|warp| bend(drawing, bounds, palette, &warp));
        let bounds = bent.as_ref().map_or(bounds, |bent| bent.bounds);
        let Some(sampling) = Sampling::of(&to_frame.then(&view.from_frame), bounds, width, height) else { continue };
        let ([x0, y0, x1, y1], points) = (sampling.reach, sampling.points);
        let flat = match bent {
            Some(bent) => Flat::of_pixels(bent.bounds, bent.pixels, space),
            None => Flat::of(drawing, bounds, palette, space),
        };
        let share = 1.0 / (points * points) as f32;
        for y in y0..y1 {
            for x in x0..x1 {
                let mut over = [0.0f32; 4];
                for (i, j) in (0..points).flat_map(|i| (0..points).map(move |j| (i, j))) {
                    let part = |at: usize, step: u32| at as f64 + (f64::from(step) + 0.5) / f64::from(points);
                    // A point at which the card is behind the camera adds nothing.
                    let Some(at) = sampling.drawing_at(part(x, i), part(y, j)) else { continue };
                    let sample = flat.sample(at);
                    over = std::array::from_fn(|c| over[c] + sample[c] * share);
                }
                let under = &mut picture[y * width + x];
                *under = std::array::from_fn(|c| over[c] + under[c] * (1.0 - over[3]));
            }
        }
    }
}

/// How what an effect node makes is laid onto what is under it on `frame`: a Blend's mode,
/// and *Normal*, over it, for every other kind.
pub fn blend_mode(shot: &Shot, effect: iskra_model::EffectId, frame: u32) -> String {
    shot.effect(effect).filter(|effect| effect.kind == iskra_model::BLEND).map_or_else(|| "Normal".to_string(), |effect| effect.choice("mode", frame))
}

/// Two composited pictures of one size as a transition shows them `at` of its way, from 0 to
/// 1 (plan §7.2b): a dissolve from `outgoing` to `incoming`, or a dip by way of black or
/// white. A side that is missing is black: a fade from or to nothing.
pub fn mix(kind: iskra_model::TransitionKind, at: f64, outgoing: Option<&[[f32; 4]]>, incoming: Option<&[[f32; 4]]>, pixels: usize) -> Vec<[f32; 4]> {
    let (out, into, flat, colour) = kind.mix(at);
    let side = |picture: Option<&[[f32; 4]]>, at: usize| picture.and_then(|picture| picture.get(at)).copied().unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let flat_colour = [colour[0] as f32, colour[1] as f32, colour[2] as f32, 1.0];
    let mixed = (0..pixels).map(|at| {
        let (a, b) = (side(outgoing, at), side(incoming, at));
        std::array::from_fn(|c| a[c] * out as f32 + b[c] * into as f32 + flat_colour[c] * flat as f32)
    });
    mixed.collect()
}

/// A picture that was composited, colour times alpha in `space`, as the picture that is
/// handed over: straight alpha, sRGB, 8 bits a channel.
pub fn finish(width: u32, height: u32, composited: &[[f32; 4]], space: WorkingSpace) -> Image {
    let pixels = composited.iter().map(|&[r, g, b, a]| {
        // Not above nothing: also what is not a number, which a card may hand back.
        if a.is_nan() || a <= 0.0 {
            return [0; 4];
        }
        let shown = |value: f32| {
            let straight = (value / a).clamp(0.0, 1.0);
            let encoded = if space == WorkingSpace::Linear { to_srgb(straight) } else { straight };
            (encoded * 255.0 + 0.5) as u8
        };
        // So little that it is none at 8 bits is nothing, and has no colour.
        match (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 {
            0 => [0; 4],
            alpha => [shown(r), shown(g), shown(b), alpha],
        }
    });
    Image { width, height, pixels: pixels.collect() }
}

// The tests' scene, which names this crate as the tests outside it do.
#[cfg(test)]
extern crate self as iskra_render;
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
#[allow(dead_code)]
mod test_scene;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_model::{MATTE, PICTURE, Source, Value, WorkingSpace};

    use super::*;
    use crate::test_scene as common;

    /// A drawing whose vector art boils is drawn as it is kept on the frames of its first turn,
    /// and as one copy for each of its other turns, the same while the turn is held.
    #[test]
    fn a_boiling_drawing_is_a_copy_of_its_own_for_each_turn() {
        use iskra_model::{ArtContent, Boil, ShapeId, ShapeKind, SwatchId, VectorArt, VectorShape, VectorStroke, kurbo::BezPath};
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((100.0, 0.0));
        let stroke = VectorStroke { boil: Some(Boil { amount: 2.0, every: 2 }), ..VectorStroke::even(path, 6.0) };
        let mut drawing = iskra_model::Drawing::default();
        drawing.art_layers[1].content =
            ArtContent::Vector(VectorArt::new(vec![VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind: ShapeKind::Stroke(stroke) }]));
        let drawing = Arc::new(drawing);
        assert!(Arc::ptr_eq(&boiled(&drawing, 0), &drawing) && Arc::ptr_eq(&boiled(&drawing, 7), &drawing));
        let (second, third) = (boiled(&drawing, 2), boiled(&drawing, 4));
        assert!(!Arc::ptr_eq(&second, &drawing) && second.art_layers[1] != drawing.art_layers[1] && second.art_layers[1] != third.art_layers[1]);
        assert!(Arc::ptr_eq(&boiled(&drawing, 3), &second) && Arc::ptr_eq(&boiled(&drawing, 8), &second), "found again");
        // A still drawing is itself.
        let still = Arc::new(iskra_model::Drawing::default());
        assert!(Arc::ptr_eq(&boiled(&still, 2), &still));
    }

    /// A drawing bent in bands of rows on several threads is the same as on one, also where
    /// it folds over itself and the first triangle is shown.
    #[test]
    fn a_drawing_bent_on_several_threads_is_the_same_as_on_one() {
        use iskra_model::{Bone, Deformer, DeformerId, SwatchId};
        let mut drawing = iskra_model::Drawing::default();
        if let iskra_model::ArtContent::Indexed(surface) = &mut drawing.art_layers[1].content {
            surface.paint(Rect::new(-300, -60, 300, 60), &vec![1.0; 600 * 120], SwatchId(3));
            surface.paint(Rect::new(-300, -10, 300, 10), &vec![1.0; 600 * 20], SwatchId(1));
        }
        // The second bone turned nearly back on the first: the drawing folds over itself.
        let bones = vec![Bone::at_rest(0.0, 300.0), Bone { turn: 160.0, ..Bone::at_rest(0.0, 300.0) }];
        let warp = Deformer::new(DeformerId(1), "Arm", [-300.0, 0.0], bones).warp(0, 1.0);
        let (bounds, palette) = (drawing.bounds(), iskra_model::Palette::standard());
        let one = bend_on(&drawing, bounds, &palette, &warp, 1);
        assert!(one.pixels.len() > BENT_ALONE, "large enough to go in bands");
        for cores in [2, 3, 7] {
            let several = bend_on(&drawing, bounds, &palette, &warp, cores);
            assert!(several.bounds == one.bounds && several.pixels == one.pixels, "on {cores} threads");
        }
    }

    /// An effect that is worked out round what is painted alone makes the same picture as
    /// it does over the whole frame: on the processor, with and without a cache, wherever
    /// the matte lies.
    #[test]
    fn an_effect_worked_out_round_what_is_painted_is_the_same_as_over_the_whole_frame() {
        let cases: &[(&str, &[(&str, Value)], bool)] = &[
            ("cutter", &[], true),
            ("cutter", &[("grow", Value::Number(-6.0)), ("blur", Value::Number(4.0))], true),
            ("cutter", &[("grow", Value::Number(9.0)), ("blur", Value::Number(7.0)), ("inverted", Value::Switch(true))], true),
            ("transparency", &[("amount", Value::Number(40.0))], false),
            ("tone", &[("width", Value::Number(5.0)), ("softness", Value::Number(3.0))], false),
            ("highlight", &[("width", Value::Number(8.0)), ("softness", Value::Number(6.0))], true),
        ];
        for (kind, set, with_matte) in cases {
            // The ball through the node, and as its matte the band of mist, which runs off
            // both sides of the frame.
            let (mut project, ball, _) = common::scene();
            let mist = Source::Layer(project.shots[0].layers[1].id);
            let id = iskra_model::EffectId(project.new_id());
            let shot = Arc::make_mut(&mut project.shots[0]);
            let mut node = iskra_model::Effect::new(id, kind, iskra_model::effect_kind(kind).unwrap());
            node.inputs.insert(PICTURE.to_string(), Source::Layer(ball));
            if *with_matte {
                node.inputs.insert(MATTE.to_string(), mist);
            }
            for (param, value) in *set {
                node.param_mut(param).unwrap().value = value.clone();
            }
            shot.rewire(Source::Layer(ball), Some(Source::Effect(id)));
            shot.effects.push(Arc::new(node));
            let shot = &project.shots[0];
            for scale in [1.0, 0.5] {
                let view = View::frame(&project.settings, scale);
                let made = |whole: bool| {
                    effects::WHOLE.set(whole);
                    let plain = composite(shot, &project.palette, WorkingSpace::Linear, 0, &view);
                    let cached = Cache::default().composite(shot, &project.palette, WorkingSpace::Linear, 0, &view);
                    effects::WHOLE.set(false);
                    (plain, cached)
                };
                let (confined, whole) = (made(false), made(true));
                assert!(whole.0.iter().any(|pixel| pixel[3] > 0.0));
                // A blur sums along its rows from where they start, so a row that starts
                // elsewhere comes to the last bit or so apart; no more, and nothing at 8 bits.
                for (confined, whole) in [(&confined.0, &whole.0), (&confined.1, &whole.1)] {
                    let apart = confined.iter().zip(whole).flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (a - b).abs())).fold(0.0f32, f32::max);
                    assert!(apart < 1e-6, "{kind} {set:?} at {scale}: {apart} apart");
                    let shown = |picture: &[[f32; 4]]| finish(view.width, view.height, picture, WorkingSpace::Linear);
                    assert!(shown(confined) == shown(whole), "{kind} {set:?} at {scale}: not the same at 8 bits");
                }
            }
        }
    }
}
