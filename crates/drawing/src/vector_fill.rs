//! The paint bucket on vector art (plan §4.16, step 5).
//!
//! An area is a face of the planar map the strokes' middle lines make, on every art layer of
//! vector art of the drawing, found by VectorCraft's Live Paint, which takes open lines as
//! edges. A fill reaches the middle of the lines round it, under the half of each line that
//! faces it, so no seam shows between them. Each line's open ends are lengthened along the way
//! they point by half the line's width there, so lines that touch where they are drawn close
//! though their middle lines stop short, and by the gap asked for, to close wider gaps.
//!
//! A fill keeps the strokes that bound it and a point inside it. When one of those strokes
//! changes, its face is found again at that point and the fill takes its outline
//! ([`refit_fills`]); a face that is no longer closed leaves the fill as it was, marked open.
//! A new stroke drawn across a fill cuts it: the fill keeps the face its point inside is in,
//! and each other face the stroke cut off of it is a fill of its own of the same swatch.
//!
//! **Surfaces bound areas too** (the user's of 2026-11-04): a fill of its own outline on the
//! line art or the overart, as the Brush set to leave surfaces makes, is an edge of the map by
//! its outline, and its inside is the line, which the bucket is not poured into. A fill a
//! surface bounds reaches a pixel under it, so no seam shows at its edge. Surfaces on the colour
//! art bound areas as well (the user's of 2026-11-23: "enable fill on any layer"), and a click on
//! a line or a surface of the art layer filled gives it the swatch; a fill the bucket made, which
//! is between lines, bounds nothing.

use std::collections::HashSet;

use kurbo::{BezPath, ParamCurve, ParamCurveNearest, PathSeg, Point, Shape as _};
use vectorcraft_geom::{FillRule, PathData};
use vectorcraft_pathops::Shape as Edge;

use crate::{ArtContent, ArtLayerKind, Bound, Drawing, FillError, ShapeKind, SwatchId, VectorArt, VectorFill, VectorShape};

/// How near a stroke's middle line comes to a face's edge to be one of the strokes that bound it.
const BOUNDING: f64 = 0.75;

/// How far the middle lines may stray, in pixels, as they are made straight pieces for the
/// planar map. Its sweep is not sure of finding the faces among curves as a hand draws them
/// (a square of four such lines, crossing at each corner, had none), and is among lines. A
/// fill reaches under its lines, so this little is never seen.
const FLAT: f64 = 0.1;

/// `path` as straight pieces, straying from it by no more than [`FLAT`].
fn flat(path: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    kurbo::flatten(path, FLAT, |element| out.push(element));
    out
}

/// A stroke's middle line, its width at its two ends, and half its widest; or a surface's
/// outline, which has no ends nor width, the line being all inside it.
struct Line {
    path: BezPath,
    ends: [f64; 2],
    half: f64,
    surface: bool,
}

/// Whether `shape`, on the art layer `art`, is a surface that bounds areas as a line does: a fill
/// of its own outline, on any art layer.
fn is_surface(_art: ArtLayerKind, shape: &VectorShape) -> bool {
    matches!(&shape.kind, ShapeKind::Fill(fill) if !fill.between_lines())
}

/// Whether `shape` on the art layer `art` is an edge of the planar map: a stroke or a surface.
fn is_edge(art: ArtLayerKind, shape: &VectorShape) -> bool {
    matches!(shape.kind, ShapeKind::Stroke(_)) || is_surface(art, shape)
}

/// The middle lines of the drawing's strokes, and the outlines of its surfaces, on every art
/// layer of vector art.
fn lines(drawing: &Drawing) -> Vec<(Bound, Line)> {
    let mut lines = Vec::new();
    for layer in &drawing.art_layers {
        let ArtContent::Vector(art) = &layer.content else { continue };
        for shape in art.shapes() {
            if let ShapeKind::Fill(fill) = &shape.kind
                && is_surface(layer.kind, shape)
                && fill.outline.segments().next().is_some()
            {
                lines.push((Bound { art: layer.kind, shape: shape.id }, Line { path: fill.outline.clone(), ends: [0.0; 2], half: 0.0, surface: true }));
            }
            if let ShapeKind::Stroke(stroke) = &shape.kind
                && stroke.path.segments().next().is_some()
            {
                let ends = [stroke.widths.first().copied().unwrap_or(0.0), stroke.widths.last().copied().unwrap_or(0.0)];
                let half = stroke.widths.iter().copied().fold(0.0, f64::max) / 2.0;
                lines.push((Bound { art: layer.kind, shape: shape.id }, Line { path: stroke.path.clone(), ends, half, surface: false }));
            }
        }
    }
    lines
}

/// `line`'s open ends lengthened along the way they point, by half its width there and `gap`,
/// as lines of their own.
fn reaches(line: &Line, gap: f64) -> Vec<BezPath> {
    let segments: Vec<PathSeg> = line.path.segments().collect();
    let (Some(first), Some(last)) = (segments.first(), segments.last()) else { return Vec::new() };
    let mut out = Vec::new();
    // The way an end points: from a little way in, out to the end.
    for ((end, inward), width) in [(first.eval(0.0), first.eval(0.01)), (last.eval(1.0), last.eval(0.99))].into_iter().zip(line.ends) {
        let way = end - inward;
        let length = width.max(0.0) / 2.0 + gap;
        if way.hypot() <= 1e-9 || length <= 0.0 {
            continue;
        }
        let mut reach = BezPath::new();
        reach.move_to(end);
        reach.line_to(end + way.normalize() * length);
        out.push(reach);
    }
    out
}

/// The gaps of no more than `gap` between a surface and what is near it, closed (the user's of
/// 2026-11-04): a surface has no ends to lengthen, so its edge is walked a step at a time, and
/// where another surface's edge, a line's side, or a part of its own edge far along it comes
/// within `gap`, at the nearest place of each such stretch, a short line is laid across, a
/// little into both, when it runs over the paper and not through a shape.
fn bridges(lines: &[(Bound, Line)], gap: f64) -> Vec<BezPath> {
    if gap <= 0.0 || !lines.iter().any(|(_, line)| line.surface) {
        return Vec::new();
    }
    let step = (gap / 3.0).max(1.0);
    // Every edge as straight pieces: a surface's outlines, a line's middle with its half
    // width. Each piece knows its outline, and how far along it it starts.
    struct Piece {
        from: Point,
        to: Point,
        outline: usize,
        along: f64,
        half: f64,
    }
    let mut pieces: Vec<Piece> = Vec::new();
    let mut outlines: Vec<(Vec<Point>, f64)> = Vec::new();
    for (_, line) in lines {
        let mut runs: Vec<Vec<Point>> = Vec::new();
        kurbo::flatten(&line.path, 0.25, |element| match element {
            kurbo::PathEl::MoveTo(at) => runs.push(vec![at]),
            kurbo::PathEl::LineTo(at) => runs.last_mut().into_iter().for_each(|run| run.push(at)),
            kurbo::PathEl::ClosePath => {
                if let Some(run) = runs.last_mut()
                    && let Some(&first) = run.first()
                {
                    run.push(first);
                }
            }
            _ => {}
        });
        for run in runs.into_iter().filter(|run| run.len() >= 2) {
            let outline = outlines.len();
            let mut along = 0.0;
            for pair in run.windows(2) {
                pieces.push(Piece { from: pair[0], to: pair[1], outline, along, half: if line.surface { 0.0 } else { line.half } });
                along += pair[0].distance(pair[1]);
            }
            // A line's middle is never walked: only a surface's edge has gaps to close here.
            outlines.push((if line.surface { run } else { Vec::new() }, along));
        }
    }
    // The pieces by the squares of a grid they pass near.
    let cell = gap.max(2.0);
    let key = |at: Point| ((at.x / cell).floor() as i64, (at.y / cell).floor() as i64);
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
    for (index, piece) in pieces.iter().enumerate() {
        let reach = kurbo::Rect::from_points(piece.from, piece.to).inflate(piece.half, piece.half);
        let ((x0, y0), (x1, y1)) = (key(Point::new(reach.x0, reach.y0)), key(Point::new(reach.x1, reach.y1)));
        for x in x0..=x1 {
            for y in y0..=y1 {
                grid.entry((x, y)).or_default().push(index);
            }
        }
    }
    let mut out = Vec::new();
    for (outline, (run, length)) in outlines.iter().enumerate() {
        if run.len() < 2 || *length < step * 2.0 {
            continue;
        }
        // Places a step apart along the edge, each with how far along it is.
        let mut places: Vec<(Point, f64)> = Vec::new();
        let mut along = 0.0;
        for pair in run.windows(2) {
            let span = pair[0].distance(pair[1]);
            let mut at = (step - along % step) % step;
            while at < span {
                places.push((pair[0].lerp(pair[1], at / span), along + at));
                at += step;
            }
            along += span;
        }
        // The nearest of anything else to each place, within the gap.
        let nearest: Vec<Option<(f64, Point)>> = places
            .iter()
            .map(|&(at, here)| {
                let (cx, cy) = key(at);
                let mut best: Option<(f64, Point)> = None;
                for x in cx - 1..=cx + 1 {
                    for y in cy - 1..=cy + 1 {
                        for &index in grid.get(&(x, y)).into_iter().flatten() {
                            let piece = &pieces[index];
                            let segment = kurbo::Line::new(piece.from, piece.to);
                            let t = segment.nearest(at, 1e-6).t;
                            let point = segment.eval(t);
                            let apart = point.distance(at) - piece.half;
                            // Of its own edge, only what is far along it, round its end.
                            if piece.outline == outline {
                                let there = piece.along + piece.from.distance(point);
                                let round = (there - here).abs().min(length - (there - here).abs());
                                if round < apart.max(0.0) * 3.0 + step * 2.0 {
                                    continue;
                                }
                            }
                            if apart > 0.05 && apart <= gap && best.is_none_or(|(was, _)| apart < was) {
                                best = Some((apart, point));
                            }
                        }
                    }
                }
                best
            })
            .collect();
        // A bridge at the nearest place of each stretch, over the paper alone.
        let far = |at: usize| nearest.get(at).copied().flatten().map_or(f64::INFINITY, |(apart, _)| apart);
        for (at, found) in nearest.iter().enumerate() {
            let Some((apart, target)) = *found else { continue };
            let (before, after) = (if at == 0 { f64::INFINITY } else { far(at - 1) }, far(at + 1));
            if apart > before || apart > after {
                continue;
            }
            let from = places[at].0;
            let way = target - from;
            if way.hypot2() <= 1e-12 {
                continue;
            }
            let way = way.normalize();
            // Up to the other's edge: a line's middle is past it by its half width.
            let paper = (apart / from.distance(target)).min(1.0);
            let over_paper = [0.25, 0.5, 0.75].iter().all(|t| !on_line(lines, from.lerp(target, t * paper)));
            if over_paper {
                let mut bridge = BezPath::new();
                bridge.move_to(from - way * 0.5);
                bridge.line_to(target + way * 0.5);
                out.push(bridge);
            }
        }
    }
    out
}

/// The faces `lines` close round, closing gaps of `gap`: the planar map, made once for as many
/// places as are looked up in it.
fn faces(lines: &[(Bound, Line)], gap: f64) -> Vec<BezPath> {
    let mut edges: Vec<Edge> = Vec::new();
    for (_, line) in lines {
        edges.push(Edge::new(PathData::from_bezpath(&flat(&line.path)), FillRule::NonZero, edges.len() as u64));
        // A surface is closed: it has no ends to reach out from.
        for reach in reaches(line, gap).into_iter().filter(|_| !line.surface) {
            edges.push(Edge::new(PathData::from_bezpath(&reach), FillRule::NonZero, edges.len() as u64));
        }
    }
    // Its gaps closed from its sides instead.
    for bridge in bridges(lines, gap) {
        edges.push(Edge::new(PathData::from_bezpath(&bridge), FillRule::NonZero, edges.len() as u64));
    }
    vectorcraft_pathops::live_paint(&edges).0.into_iter().map(|face| face.path.to_bezpath()).collect()
}

/// The strokes whose middle lines run along `face`'s edge.
fn bounds_of(face: &BezPath, lines: &[(Bound, Line)]) -> Vec<Bound> {
    let reach = face.bounding_box().inflate(BOUNDING, BOUNDING);
    let mut edge = Vec::new();
    kurbo::flatten(face, 0.25, |element| match element {
        kurbo::PathEl::MoveTo(at) | kurbo::PathEl::LineTo(at) => edge.push(at),
        _ => {}
    });
    let mut bounds = Vec::new();
    for (bound, Line { path, .. }) in lines {
        // Widened, so that a line level or upright, whose box has no area, is not passed by.
        if path.bounding_box().inflate(BOUNDING, BOUNDING).intersect(reach).area() <= 0.0 {
            continue;
        }
        let near = |at: &Point| path.segments().any(|segment| segment.nearest(*at, 1e-3).distance_sq <= BOUNDING * BOUNDING);
        if edge.iter().any(near) {
            bounds.push(*bound);
        }
    }
    bounds
}

/// Whether `at` is on `line`: inside a surface, or within half a stroke's widest of its middle
/// line. Measured, not drawn, so that a drag over many places looks quickly.
fn is_on(line: &Line, at: Point) -> bool {
    if line.surface {
        return line.path.bounding_box().contains(at) && line.path.winding(at) != 0;
    }
    let reach = line.path.bounding_box().inflate(line.half, line.half);
    reach.contains(at) && line.path.segments().any(|segment| segment.nearest(at, 1e-3).distance_sq <= line.half * line.half)
}

/// Whether `at` is on one of `lines`.
fn on_line(lines: &[(Bound, Line)], at: Point) -> bool {
    lines.iter().any(|(_, line)| is_on(line, at))
}

/// The outline of a fill of `face`, which `bounds` bound: the face, reaching a pixel further
/// where a surface is among them, under its edge, so that the two leave no seam. Under a line
/// it reaches the middle already.
fn outline_of(face: &BezPath, bounds: &[Bound], lines: &[(Bound, Line)]) -> BezPath {
    if !lines.iter().any(|(bound, line)| line.surface && bounds.contains(bound)) {
        return face.clone();
    }
    let edge = kurbo::stroke(face.iter(), &kurbo::Stroke::new(2.0), &kurbo::StrokeOpts::default(), 0.1);
    let grown = vectorcraft_pathops::boolean(
        &PathData::from_bezpath(face),
        FillRule::NonZero,
        &PathData::from_bezpath(&edge),
        FillRule::NonZero,
        vectorcraft_pathops::BoolOp::Union,
    );
    grown.to_bezpath()
}

/// Where a fill is found again: its point inside, or else a point inside its outline.
fn inside_of(fill: &VectorFill) -> Option<Point> {
    fill.inside.map(|[x, y]| Point::new(x, y)).or_else(|| vectorcraft_pathops::interior_point(&PathData::from_bezpath(&fill.outline)))
}

/// Fill the area round `at` on the art layer `art` of vector art with `swatch`, closing gaps
/// of `gap` pixels in its lines, or with [`SwatchId::NONE`] take the fill there away. A fill
/// that is there already is given the swatch; a new one goes under everything on its art
/// layer. A fill of one of the `protected` swatches is refused. Says whether anything changed.
pub fn fill_vector(drawing: &mut Drawing, art: ArtLayerKind, at: Point, swatch: SwatchId, gap: f64, protected: &[SwatchId]) -> Result<bool, FillError> {
    match fill_vector_at(drawing, art, &[at], swatch, gap, protected) {
        (_, Some(why)) => Err(why),
        (changed, None) => Ok(changed > 0),
    }
}

/// [`fill_vector`] at each of `places`, as a drag of the bucket gives them: the lines' planar
/// map is made once, an area is filled once however many places are in it, and the art is
/// drawn once at the end. Returns how many places took the swatch or had it already, and why
/// the first that didn't, didn't.
pub fn fill_vector_at(
    drawing: &mut Drawing,
    art: ArtLayerKind,
    places: &[Point],
    swatch: SwatchId,
    gap: f64,
    protected: &[SwatchId],
) -> (usize, Option<FillError>) {
    let Some(mut bucket) = VectorBucket::new(drawing, art, swatch, gap, protected) else { return (0, None) };
    for &at in places {
        bucket.fill(at);
    }
    bucket.finish(drawing)
}

/// The bucket dragged over vector art: the places it goes over taken one at a time, each area
/// filled once, as [`fill_vector_at`] fills them all at once. What it has filled so far can
/// be drawn while it is dragged ([`VectorBucket::art`]).
pub struct VectorBucket {
    art: ArtLayerKind,
    swatch: SwatchId,
    gap: f64,
    protected: Vec<SwatchId>,
    lines: Vec<(Bound, Line)>,
    /// The areas between the lines, found when first wanted.
    faces: Option<Vec<BezPath>>,
    shapes: Vec<VectorShape>,
    next: u32,
    /// The faces filled so far: a place in one of them is filled already.
    done: Vec<usize>,
    filled: usize,
    changed: bool,
    refused: Option<FillError>,
}

impl VectorBucket {
    /// A bucket of `swatch`, or [`SwatchId::NONE`] to take fills away, on the art layer `art`
    /// of `drawing`. `None` where that is not vector art.
    pub fn new(drawing: &Drawing, art: ArtLayerKind, swatch: SwatchId, gap: f64, protected: &[SwatchId]) -> Option<VectorBucket> {
        let Some(ArtContent::Vector(vector)) = drawing.layer(art).map(|layer| &layer.content) else { return None };
        Some(VectorBucket {
            art,
            swatch,
            gap,
            protected: protected.to_vec(),
            lines: lines(drawing),
            faces: None,
            shapes: vector.shapes().to_vec(),
            next: vector.next_id().0,
            done: Vec::new(),
            filled: 0,
            changed: false,
            refused: None,
        })
    }

    /// Fill the area at `at`. Says whether the art changed.
    pub fn fill(&mut self, at: Point) -> bool {
        let (swatch, gap) = (self.swatch, self.gap);
        if on_line(&self.lines, at) {
            // On a line or a surface of the art layer filled, the topmost there takes the swatch.
            let own = self.lines.iter().rev().find(|(bound, line)| bound.art == self.art && is_on(line, at)).map(|(bound, _)| bound.shape);
            let index = own.and_then(|own| self.shapes.iter().position(|shape| shape.id == own));
            match index {
                Some(index) if swatch != SwatchId::NONE && !self.protected.contains(&self.shapes[index].swatch) => {
                    let changed = self.shapes[index].swatch != swatch;
                    self.shapes[index].swatch = swatch;
                    (self.filled, self.changed) = (self.filled + 1, self.changed | changed);
                    return changed;
                }
                Some(index) if self.protected.contains(&self.shapes[index].swatch) => _ = self.refused.get_or_insert(FillError::Protected),
                _ => _ = self.refused.get_or_insert(FillError::OnLine),
            }
            return false;
        }
        let lines = &self.lines;
        let faces = self.faces.get_or_insert_with(|| self::faces(lines, gap));
        let face = faces.iter().position(|face| face.winding(at) != 0);
        if face.is_some_and(|face| self.done.contains(&face)) {
            self.filled += 1;
            return false;
        }
        let shapes = &mut self.shapes;
        // The fill of this area, if it has one: the topmost whose point inside is in the face,
        // or with no face, whose outline holds the place.
        let found = shapes.iter().rposition(|shape| match (&shape.kind, face) {
            (ShapeKind::Fill(fill), Some(face)) => inside_of(fill).is_some_and(|inside| faces[face].winding(inside) != 0),
            (ShapeKind::Fill(fill), None) => fill.outline.winding(at) != 0,
            _ => false,
        });
        let mut changed = false;
        match (found, face) {
            (Some(index), _) if self.protected.contains(&shapes[index].swatch) => _ = self.refused.get_or_insert(FillError::Protected),
            (Some(index), _) if shapes[index].swatch == swatch => self.filled += 1,
            (Some(index), _) if swatch == SwatchId::NONE => {
                shapes.remove(index);
                (self.filled, changed) = (self.filled + 1, true);
            }
            (Some(index), face) => {
                shapes[index].swatch = swatch;
                // Filled again, it takes the area as the lines now have it.
                if let (Some(face), ShapeKind::Fill(fill)) = (face, &mut shapes[index].kind) {
                    fill.bound_by = bounds_of(&faces[face], lines);
                    let outline = outline_of(&faces[face], &fill.bound_by, lines);
                    (fill.outline, fill.inside, fill.gap, fill.open) = (outline, Some([at.x, at.y]), gap, false);
                }
                (self.filled, changed) = (self.filled + 1, true);
            }
            (None, None) if swatch == SwatchId::NONE => {}
            (None, None) => _ = self.refused.get_or_insert(FillError::Open),
            (None, Some(_)) if swatch == SwatchId::NONE => {}
            (None, Some(face)) => {
                let bound_by = bounds_of(&faces[face], lines);
                let outline = outline_of(&faces[face], &bound_by, lines);
                let fill = VectorFill { bound_by, inside: Some([at.x, at.y]), gap, ..VectorFill::plain(outline) };
                shapes.insert(0, VectorShape { id: crate::ShapeId(self.next), swatch, kind: ShapeKind::Fill(fill) });
                (self.next, self.filled, changed) = (self.next + 1, self.filled + 1, true);
            }
        }
        if let Some(face) = face {
            self.done.push(face);
        }
        self.changed |= changed;
        changed
    }

    /// The art layer as it is filled so far.
    pub fn art(&self) -> VectorArt {
        VectorArt::new(self.shapes.clone())
    }

    /// Put what was filled into `drawing`. Returns how many places took the swatch or had it
    /// already, and why the first that didn't, didn't: a refusal is told only if none did.
    pub fn finish(self, drawing: &mut Drawing) -> (usize, Option<FillError>) {
        if self.changed
            && let Some(layer) = drawing.layer_mut(self.art)
        {
            layer.content = ArtContent::Vector(VectorArt::new(self.shapes));
        }
        (self.filled, if self.filled == 0 { self.refused } else { None })
    }
}

/// Give the stroke or the surface under `at` on the art layer `art` of vector art the swatch
/// `swatch`: the topmost, unless it is of one of the `protected` swatches. Says whether anything changed.
pub fn recolour_vector_line(drawing: &mut Drawing, art: ArtLayerKind, at: Point, swatch: SwatchId, protected: &[SwatchId]) -> Result<bool, FillError> {
    let Some(ArtContent::Vector(vector)) = drawing.layer(art).map(|layer| &layer.content) else { return Ok(false) };
    // A line, or a surface, which is a line too.
    let under = vector.shapes().iter().rposition(|shape| match &shape.kind {
        ShapeKind::Stroke(stroke) => stroke.outline().winding(at) != 0,
        ShapeKind::Fill(fill) => is_surface(art, shape) && fill.outline.winding(at) != 0,
    });
    let Some(index) = under else { return Err(FillError::NoLine) };
    let was = vector.shapes()[index].swatch;
    if protected.contains(&was) {
        return Err(FillError::Protected);
    }
    if was == swatch || swatch == SwatchId::NONE {
        return Ok(false);
    }
    let edited = vector.edited(|shapes| shapes[index].swatch = swatch);
    if let Some(layer) = drawing.layer_mut(art) {
        layer.content = ArtContent::Vector(edited);
    }
    Ok(true)
}

/// Find again the fills of `drawing` that a stroke it changed since `before` bounds: each takes
/// the face round its point inside as the lines now are, or where they no longer close round
/// it, keeps its outline and is marked open. A fill that a stroke new since `before` crosses
/// is cut along it: it keeps the face round its point inside, and each other face inside its
/// outline as it was becomes a fill of the same swatch, just over it. Fills nothing changed
/// round are left as they are, and so are fills of their own outline, which no lines bound
/// (merged, flattened, a shape's).
pub fn refit_fills(drawing: &mut Drawing, before: &Drawing) {
    let shapes_of = |drawing: &Drawing, art: ArtLayerKind| match drawing.layer(art).map(|layer| &layer.content) {
        Some(ArtContent::Vector(vector)) => vector.shapes().to_vec(),
        _ => Vec::new(),
    };
    // The strokes that are gone or not as they were, and those that are new.
    let mut changed: HashSet<Bound> = HashSet::new();
    let mut added: Vec<kurbo::Rect> = Vec::new();
    for layer in &drawing.art_layers {
        let (now, was) = (shapes_of(drawing, layer.kind), shapes_of(before, layer.kind));
        for shape in was.iter().filter(|shape| is_edge(layer.kind, shape)) {
            if !now.contains(shape) {
                changed.insert(Bound { art: layer.kind, shape: shape.id });
            }
        }
        for shape in now.iter().filter(|shape| is_edge(layer.kind, shape)) {
            if was.iter().all(|other| other.id != shape.id) {
                added.push(shape.kind.area().bounding_box());
            }
        }
    }
    if changed.is_empty() && added.is_empty() {
        return;
    }
    let lines = lines(drawing);
    // The faces at each width of gap, found once.
    let mut found: Vec<(f64, Vec<BezPath>)> = Vec::new();
    let mut faces_at = |gap: f64| -> Vec<BezPath> {
        if let Some((_, faces)) = found.iter().find(|(at, _)| *at == gap) {
            return faces.clone();
        }
        let made = faces(&lines, gap);
        found.push((gap, made.clone()));
        made
    };
    for index in 0..drawing.art_layers.len() {
        let ArtContent::Vector(vector) = &drawing.art_layers[index].content else { continue };
        let follows = |fill: &VectorFill| fill.bound_by.iter().any(|bound| changed.contains(bound));
        let crossed = |fill: &VectorFill| {
            let reach = fill.outline.bounding_box();
            fill.between_lines() && added.iter().any(|stroke| stroke.overlaps(reach))
        };
        if !vector.shapes().iter().any(|shape| matches!(&shape.kind, ShapeKind::Fill(fill) if follows(fill) || crossed(fill))) {
            continue;
        }
        let mut next = vector.next_id().0;
        let mut shapes = vector.shapes().to_vec();
        // The fills cut off, each to go just over the fill it was cut from.
        let mut cut: Vec<(usize, VectorShape)> = Vec::new();
        for place in 0..shapes.len() {
            let swatch = shapes[place].swatch;
            let ShapeKind::Fill(fill) = &shapes[place].kind else { continue };
            let (follows, crossed) = (follows(fill), crossed(fill));
            if !follows && !crossed {
                continue;
            }
            let faces = faces_at(fill.gap);
            // Its own face: round its point inside, unless a new line was drawn over that point.
            let inside = inside_of(fill).filter(|inside| !(crossed && on_line(&lines, *inside)));
            let own = inside.and_then(|inside| faces.iter().position(|face| face.winding(inside) != 0));
            // Cut, the faces inside the outline it had, each with a point inside it, which is
            // in no other fill's face already.
            let pieces: Vec<(usize, Point)> = match crossed {
                false => Vec::new(),
                true => {
                    let reach = fill.outline.bounding_box();
                    let others: Vec<Point> = shapes
                        .iter()
                        .enumerate()
                        .filter(|(other, _)| *other != place)
                        .filter_map(|(_, shape)| match &shape.kind {
                            ShapeKind::Fill(other) => inside_of(other),
                            ShapeKind::Stroke(_) => None,
                        })
                        .collect();
                    let taken = |face: &BezPath| others.iter().any(|at| face.winding(*at) != 0);
                    faces
                        .iter()
                        .enumerate()
                        .filter(|(at, face)| Some(*at) != own && face.bounding_box().overlaps(reach) && !taken(face))
                        .filter_map(|(at, face)| point_within(face, &fill.outline, &lines).map(|point| (at, point)))
                        .collect()
                }
            };
            // A fill whose point inside is now under the line takes the largest of its pieces.
            let (own, pieces) = match own {
                Some(own) => (Some((own, None)), pieces),
                None if !pieces.is_empty() => {
                    let largest = (0..pieces.len()).max_by(|a, b| faces[pieces[*a].0].area().abs().total_cmp(&faces[pieces[*b].0].area().abs())).unwrap_or(0);
                    let mut pieces = pieces;
                    let (face, point) = pieces.remove(largest);
                    (Some((face, Some(point))), pieces)
                }
                None => (None, pieces),
            };
            let gap = fill.gap;
            let ShapeKind::Fill(fill) = &mut shapes[place].kind else { continue };
            match own {
                Some((face, point)) => {
                    fill.bound_by = bounds_of(&faces[face], &lines);
                    (fill.outline, fill.open) = (outline_of(&faces[face], &fill.bound_by, &lines), false);
                    if let Some(point) = point {
                        fill.inside = Some([point.x, point.y]);
                    }
                }
                None => fill.open = true,
            }
            for (face, point) in pieces {
                let bound_by = bounds_of(&faces[face], &lines);
                let outline = outline_of(&faces[face], &bound_by, &lines);
                let piece = VectorFill { bound_by, inside: Some([point.x, point.y]), gap, ..VectorFill::plain(outline) };
                cut.push((place, VectorShape { id: crate::ShapeId(next), swatch, kind: ShapeKind::Fill(piece) }));
                next += 1;
            }
        }
        // From the last, so that the places before stay as they were.
        for (place, piece) in cut.into_iter().rev() {
            shapes.insert(place + 1, piece);
        }
        drawing.art_layers[index].content = ArtContent::Vector(VectorArt::new(shapes));
    }
}

/// A point inside `face` and inside `outline`, off the lines: the one nearest the middle of
/// those found on a grid over where the two meet. `None` where they hardly meet.
fn point_within(face: &BezPath, outline: &BezPath, lines: &[(Bound, Line)]) -> Option<Point> {
    const STEPS: usize = 24;
    let reach = face.bounding_box().intersect(outline.bounding_box());
    if reach.area() <= 0.0 {
        return None;
    }
    let mut found = Vec::new();
    for row in 0..STEPS {
        for column in 0..STEPS {
            let at = Point::new(reach.x0 + reach.width() * (column as f64 + 0.5) / STEPS as f64, reach.y0 + reach.height() * (row as f64 + 0.5) / STEPS as f64);
            if face.winding(at) != 0 && outline.winding(at) != 0 && !on_line(lines, at) {
                found.push(at);
            }
        }
    }
    // Too few to be more than a sliver along a line.
    if found.len() < 4 {
        return None;
    }
    let middle = found.iter().fold(kurbo::Vec2::ZERO, |sum, at| sum + at.to_vec2()) / found.len() as f64;
    found.into_iter().min_by(|a, b| (a.to_vec2() - middle).hypot2().total_cmp(&(b.to_vec2() - middle).hypot2()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArtLayer, ShapeId, VectorStroke};

    fn line(from: (f64, f64), to: (f64, f64)) -> BezPath {
        let mut path = BezPath::new();
        path.move_to(from);
        path.line_to(to);
        path
    }

    /// A drawing with colour art under line art, both vector art, the line art these lines.
    fn drawing_of(lines: &[BezPath]) -> Drawing {
        let strokes = lines
            .iter()
            .enumerate()
            .map(|(i, path)| VectorShape { id: ShapeId(i as u32 + 1), swatch: SwatchId(1), kind: ShapeKind::Stroke(VectorStroke::even(path.clone(), 4.0)) })
            .collect();
        Drawing {
            art_layers: vec![
                ArtLayer { kind: ArtLayerKind::ColourArt, content: ArtContent::Vector(VectorArt::default()) },
                ArtLayer { kind: ArtLayerKind::LineArt, content: ArtContent::Vector(VectorArt::new(strokes)) },
            ],
            texts: Vec::new(),
            placements: Vec::new(),
        }
    }

    /// [`drawing_of`] with the lines drawn as surfaces, each the area a line 6 wide covers.
    fn surfaces_of(lines: &[BezPath]) -> Drawing {
        let mut drawing = drawing_of(lines);
        let Some(ArtContent::Vector(art)) = drawing.layer(ArtLayerKind::LineArt).map(|l| l.content.clone()) else { panic!() };
        let surfaces = art.edited(|shapes| {
            for shape in shapes.iter_mut() {
                let ShapeKind::Stroke(stroke) = &shape.kind else { continue };
                let wide = VectorStroke::even(stroke.path.clone(), 6.0);
                shape.kind = ShapeKind::Fill(VectorFill::plain(crate::surface(&wide, &crate::VectorBrush { simplify: 0.1, ..Default::default() })));
            }
        });
        drawing.layer_mut(ArtLayerKind::LineArt).unwrap().content = ArtContent::Vector(surfaces);
        drawing
    }

    #[test]
    fn surfaces_bound_the_fill_they_close_which_follows_them() {
        let mut drawing = surfaces_of(&square(0.0));
        // Inside a surface is on the line; inside the four, the area between them.
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(50.0, 1.0), SwatchId(2), 0.0, &[]), Err(FillError::OnLine));
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 0.0, &[]), Ok(true));
        let ShapeKind::Fill(fill) = &colour(&drawing).shapes()[0].kind else { panic!("a fill") };
        assert_eq!(fill.bound_by.len(), 4, "{:?}", fill.bound_by);
        // To a pixel under the surfaces' inner edge, at 3, and not past them.
        assert_eq!((painted(&drawing, 50, 50), painted(&drawing, 2, 50), painted(&drawing, 50, 97), painted(&drawing, -2, 50)), (2, 2, 2, 0));
        // The right side moved out to 150: the fill goes with it.
        let before = drawing.clone();
        let moved =
            vec![line((0.0, 0.0), (150.0, 0.0)), line((150.0, 0.0), (150.0, 100.0)), line((150.0, 100.0), (0.0, 100.0)), line((0.0, 100.0), (0.0, 0.0))];
        let lines = surfaces_of(&moved).layer(ArtLayerKind::LineArt).unwrap().clone();
        *drawing.layer_mut(ArtLayerKind::LineArt).unwrap() = lines;
        refit_fills(&mut drawing, &before);
        let ShapeKind::Fill(fill) = &colour(&drawing).shapes()[0].kind else { panic!("a fill") };
        assert!(!fill.open && painted(&drawing, 140, 50) == 2, "the fill went with the surface");
        // The bucket on the line art gives a surface the swatch, as it does a line.
        assert_eq!(recolour_vector_line(&mut drawing, ArtLayerKind::LineArt, Point::new(50.0, 1.0), SwatchId(5), &[]), Ok(true));
        // A ring drawn as a surface on the colour art bounds a fill too (the user's of
        // 2026-11-23), and a click on the ring gives it the swatch.
        let mut colour_only = drawing_of(&[]);
        let ring = kurbo::Circle::new((50.0, 50.0), 30.0).to_path(0.1);
        let mut hole = kurbo::Circle::new((50.0, 50.0), 20.0).to_path(0.1);
        hole.apply_affine(kurbo::Affine::FLIP_X.then_translate((100.0, 0.0).into()));
        let mut outline = ring;
        outline.extend(hole);
        let blob = VectorShape { id: ShapeId(9), swatch: SwatchId(3), kind: ShapeKind::Fill(VectorFill::plain(outline)) };
        colour_only.layer_mut(ArtLayerKind::ColourArt).unwrap().content = ArtContent::Vector(VectorArt::new(vec![blob]));
        assert_eq!(super::lines(&colour_only).len(), 1);
        assert_eq!(fill_vector(&mut colour_only, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 0.0, &[]), Ok(true));
        assert_eq!(colour(&colour_only).shapes().len(), 2, "a fill inside the ring");
        assert_eq!(fill_vector(&mut colour_only, ArtLayerKind::ColourArt, Point::new(50.0, 25.0), SwatchId(4), 0.0, &[]), Ok(true));
        assert!(colour(&colour_only).shapes().iter().any(|shape| shape.id == ShapeId(9) && shape.swatch == SwatchId(4)), "the ring recoloured");
        assert_eq!(fill_vector(&mut colour_only, ArtLayerKind::ColourArt, Point::new(50.0, 25.0), SwatchId(4), 0.0, &[SwatchId(4)]), Err(FillError::Protected));
    }

    #[test]
    fn gaps_from_surfaces_are_closed_to_another_a_line_and_their_own_far_end() {
        // The left side stops 16 short of the top: a gap of 10 between their edges.
        let mut drawing = surfaces_of(&square(16.0));
        assert_eq!(fill_vector(&mut drawing.clone(), ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 0.0, &[]), Err(FillError::Open));
        assert_eq!(fill_vector(&mut drawing.clone(), ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 8.0, &[]), Err(FillError::Open));
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 12.0, &[]), Ok(true));
        assert_eq!((painted(&drawing, 50, 50), painted(&drawing, -10, 50), painted(&drawing, -10, 8)), (2, 0, 0), "inside, and not out through the gap");
        // A surface short of a line: the top a line, the rest surfaces.
        let mut mixed = surfaces_of(&square(16.0));
        let top = VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind: ShapeKind::Stroke(VectorStroke::even(line((-3.0, 0.0), (103.0, 0.0)), 6.0)) };
        let Some(ArtContent::Vector(art)) = mixed.layer(ArtLayerKind::LineArt).map(|l| l.content.clone()) else { panic!() };
        mixed.layer_mut(ArtLayerKind::LineArt).unwrap().content = ArtContent::Vector(art.edited(|shapes| shapes[0] = top));
        assert_eq!(fill_vector(&mut mixed, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 12.0, &[]), Ok(true));
        // A ring drawn as one surface, its ends 10 apart: closed on itself.
        let mut ring = BezPath::new();
        for step in 0..=60 {
            let angle = 0.2 + step as f64 / 60.0 * (std::f64::consts::TAU - 0.4);
            let at = Point::new(50.0 + 40.0 * angle.cos(), 50.0 + 40.0 * angle.sin());
            if step == 0 { ring.move_to(at) } else { ring.line_to(at) }
        }
        let mut round = surfaces_of(&[ring]);
        assert_eq!(fill_vector(&mut round.clone(), ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 0.0, &[]), Err(FillError::Open));
        assert_eq!(fill_vector(&mut round, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 12.0, &[]), Ok(true));
        assert_eq!((painted(&round, 50, 50), painted(&round, 95, 50)), (2, 0));
    }

    #[test]
    fn surfaces_drawn_with_a_brush_bound_the_fill_they_close() {
        let mut drawing = surfaces_of(&square(0.0));
        let brush = crate::VectorBrush { size: 10.0, simplify: 0.1, ..Default::default() };
        let Some(ArtContent::Vector(art)) = drawing.layer(ArtLayerKind::LineArt).map(|l| l.content.clone()) else { panic!() };
        let surfaced = art.edited(|shapes| {
            for (shape, side) in shapes.iter_mut().zip(square(0.0)) {
                shape.kind = ShapeKind::Fill(VectorFill::plain(crate::surface(&VectorStroke::even(side, 10.0), &brush)));
            }
        });
        drawing.layer_mut(ArtLayerKind::LineArt).unwrap().content = ArtContent::Vector(surfaced);
        let started = std::time::Instant::now();
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 0.0, &[]), Ok(true));
        assert_eq!(fill_vector(&mut drawing.clone(), ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 6.0, &[]), Ok(true));
        assert!(started.elapsed().as_secs_f64() < 2.0, "{:?}", started.elapsed());
        assert_eq!(painted(&drawing, 50, 50), 2);
        assert_eq!(painted(&drawing, -8, 50), 0, "not out past the lines");
    }

    #[test]
    fn lines_level_and_upright_bound_the_fill_they_close() {
        // Lines exactly level and upright, running on past each other at the corners.
        let crossing = vec![
            line((-30.0, 0.0), (130.0, 0.0)),
            line((100.0, -30.0), (100.0, 130.0)),
            line((130.0, 100.0), (-30.0, 100.0)),
            line((0.0, 130.0), (0.0, -30.0)),
        ];
        let mut drawing = drawing_of(&crossing);
        fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 0.0, &[]).unwrap();
        let ShapeKind::Fill(fill) = &colour(&drawing).shapes()[0].kind else { panic!("a fill") };
        assert_eq!(fill.bound_by.len(), 4, "{:?}", fill.bound_by);
    }

    /// A square of four lines that meet at its corners, `open` short of closing on its left.
    fn square(open: f64) -> Vec<BezPath> {
        vec![line((0.0, 0.0), (100.0, 0.0)), line((100.0, 0.0), (100.0, 100.0)), line((100.0, 100.0), (0.0, 100.0)), line((0.0, 100.0), (0.0, open))]
    }

    fn colour(drawing: &Drawing) -> &VectorArt {
        let Some(ArtContent::Vector(art)) = drawing.layer(ArtLayerKind::ColourArt).map(|l| &l.content) else { panic!("vector colour art") };
        art
    }

    fn painted(drawing: &Drawing, x: i32, y: i32) -> u16 {
        let colour = colour(drawing).raster().pixel(x, y);
        if colour.coverage > u16::MAX / 2 { colour.swatch.0 } else { 0 }
    }

    /// `drawing` with a stroke of `path` drawn on its line art, new.
    fn with_line(drawing: &Drawing, path: BezPath) -> Drawing {
        let mut drawn = drawing.clone();
        let Some(ArtContent::Vector(lines)) = drawn.layer(ArtLayerKind::LineArt).map(|layer| &layer.content) else { panic!("vector line art") };
        let stroke = VectorShape { id: lines.next_id(), swatch: SwatchId(1), kind: ShapeKind::Stroke(VectorStroke::even(path, 4.0)) };
        let lines = lines.edited(|shapes| shapes.push(stroke));
        drawn.layer_mut(ArtLayerKind::LineArt).unwrap().content = ArtContent::Vector(lines);
        drawn
    }

    #[test]
    fn a_new_line_across_a_fill_cuts_it_in_two_of_its_swatch() {
        let mut drawing = drawing_of(&square(0.0));
        fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(30.0, 50.0), SwatchId(2), 0.0, &[]).unwrap();
        // A line that stops inside it cuts nothing.
        let mut stopped = with_line(&drawing, line((50.0, -10.0), (50.0, 60.0)));
        refit_fills(&mut stopped, &drawing);
        assert_eq!(colour(&stopped).shapes().len(), 1);
        // One right across: the fill keeps the left, where its point inside is; the right is a
        // fill of its own, of the same swatch, each to the middle of the new line.
        let before = drawing.clone();
        let mut cut = with_line(&drawing, line((50.0, -10.0), (50.0, 110.0)));
        refit_fills(&mut cut, &before);
        let fills: Vec<&VectorShape> = colour(&cut).shapes().iter().collect();
        assert_eq!(fills.len(), 2, "{fills:?}");
        assert!(fills.iter().all(|shape| shape.swatch == SwatchId(2)));
        let reach = |shape: &VectorShape| shape.kind.area().bounding_box();
        let (left, right) = (reach(fills[0]), reach(fills[1]));
        assert!((left.x1 - 50.0).abs() < 1.0 && left.x0 < 1.0 && (right.x0 - 50.0).abs() < 1.0 && right.x1 > 99.0, "{left:?} {right:?}");
        assert!(painted(&cut, 25, 50) == 2 && painted(&cut, 75, 50) == 2);
        // The piece follows its lines as the fill does.
        let ShapeKind::Fill(piece) = &fills[1].kind else { panic!("a fill") };
        assert!(!piece.bound_by.is_empty() && piece.inside.is_some_and(|[x, _]| x > 50.0));
    }

    #[test]
    fn a_fill_of_its_own_outline_is_left_as_it_is_by_a_line_drawn_over_it() {
        // A merged fill inside lines, as Merge Down leaves it: no point inside, no lines bound it.
        let mut drawing = drawing_of(&square(0.0));
        let merged = VectorShape {
            id: crate::ShapeId(1),
            swatch: SwatchId(2),
            kind: ShapeKind::Fill(VectorFill::plain(kurbo::Rect::new(20.0, 20.0, 80.0, 80.0).to_path(0.1))),
        };
        drawing.layer_mut(ArtLayerKind::ColourArt).unwrap().content = ArtContent::Vector(crate::VectorArt::new(vec![merged.clone()]));
        // A line right across it, and the lines round it moved: it stays as it was.
        let before = drawing.clone();
        let mut drawn = with_line(&drawing, line((50.0, -10.0), (50.0, 110.0)));
        refit_fills(&mut drawn, &before);
        assert_eq!(colour(&drawn).shapes(), &[merged]);
    }

    #[test]
    fn a_click_inside_lines_fills_to_their_middle_under_them() {
        let mut drawing = drawing_of(&square(0.0));
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 0.0, &[]), Ok(true));
        let fill = &colour(&drawing).shapes()[0];
        let ShapeKind::Fill(kind) = &fill.kind else { panic!("a fill") };
        assert_eq!(fill.swatch, SwatchId(2));
        assert_eq!(kind.bound_by.len(), 4, "the four lines bound it: {:?}", kind.bound_by);
        assert!(kind.bound_by.iter().all(|bound| bound.art == ArtLayerKind::LineArt));
        assert_eq!((painted(&drawing, 50, 50), painted(&drawing, 1, 50), painted(&drawing, -2, 50)), (2, 2, 0), "to the lines' middle");
    }

    /// Four lines as a hand draws them with the vector brush: wavering, a little wider and
    /// narrower, crossing at each corner; `right` is how far the right side is moved out.
    fn drawn_by_hand(right: f64) -> Drawing {
        use crate::{StrokePoint, VectorBrush, vector_stroke};
        let side = |from: [f64; 2], to: [f64; 2]| -> Vec<StrokePoint> {
            let wavering = |t: f64| ((t * 31.0).sin() * 1.5, (t * 17.0).cos() * 1.5);
            (0..=40)
                .map(|i| {
                    let t = f64::from(i) / 40.0;
                    StrokePoint::new(from[0] + (to[0] - from[0]) * t + wavering(t).0, from[1] + (to[1] - from[1]) * t + wavering(t).1, 0.6)
                })
                .collect()
        };
        let brush = VectorBrush { size: 8.0, ..VectorBrush::default() };
        let corners = [
            ([95.0, 100.0], [305.0 + right, 102.0]),
            ([300.0 + right, 95.0], [298.0 + right, 305.0]),
            ([305.0 + right, 300.0], [95.0, 298.0]),
            ([100.0, 305.0], [102.0, 95.0]),
        ];
        let strokes = corners
            .iter()
            .enumerate()
            .map(|(i, (from, to))| VectorShape {
                id: ShapeId(i as u32 + 1),
                swatch: SwatchId(1),
                kind: ShapeKind::Stroke(vector_stroke(&side(*from, *to), &brush).unwrap()),
            })
            .collect();
        Drawing {
            art_layers: vec![
                ArtLayer { kind: ArtLayerKind::ColourArt, content: ArtContent::Vector(VectorArt::default()) },
                ArtLayer { kind: ArtLayerKind::LineArt, content: ArtContent::Vector(VectorArt::new(strokes)) },
            ],
            texts: Vec::new(),
            placements: Vec::new(),
        }
    }

    #[test]
    fn lines_drawn_by_hand_are_filled_and_the_fill_follows_one_moved() {
        let mut drawing = drawn_by_hand(0.0);
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(200.0, 200.0), SwatchId(2), 0.0, &[]), Ok(true));
        let ShapeKind::Fill(fill) = &colour(&drawing).shapes()[0].kind else { panic!("a fill") };
        assert_eq!(fill.bound_by.len(), 4, "{:?}", fill.bound_by);
        // The three sides that touch the right one are not the same lines once it moves; the
        // right side moved out by 40, as the point editor or a selection would.
        let before = drawing.clone();
        let moved = drawn_by_hand(40.0);
        *drawing.layer_mut(ArtLayerKind::LineArt).unwrap() = moved.layer(ArtLayerKind::LineArt).unwrap().clone();
        refit_fills(&mut drawing, &before);
        let ShapeKind::Fill(fill) = &colour(&drawing).shapes()[0].kind else { panic!("a fill") };
        assert!(!fill.open && painted(&drawing, 330, 200) == 2, "the fill followed the right side out");
    }

    #[test]
    fn a_drag_over_two_areas_fills_each_once_and_quickly() {
        let mut two = square(0.0);
        two.extend([line((100.0, 0.0), (200.0, 0.0)), line((200.0, 0.0), (200.0, 100.0)), line((200.0, 100.0), (100.0, 100.0))]);
        let places: Vec<Point> = (0..100).map(|i| Point::new(10.0 + f64::from(i) * 1.8, 50.0)).filter(|at| (at.x - 100.0).abs() > 4.0).collect();
        let mut drawing = drawing_of(&two);
        let one_by_one = std::time::Instant::now();
        for at in &places {
            let _ = fill_vector(&mut drawing.clone(), ArtLayerKind::ColourArt, *at, SwatchId(2), 0.0, &[]);
        }
        let one_by_one = one_by_one.elapsed();
        let at_once = std::time::Instant::now();
        let (filled, refused) = fill_vector_at(&mut drawing, ArtLayerKind::ColourArt, &places, SwatchId(2), 0.0, &[]);
        let at_once = at_once.elapsed();
        assert_eq!((filled, refused, colour(&drawing).shapes().len()), (places.len(), None, 2), "two fills, one an area");
        assert_eq!((painted(&drawing, 50, 50), painted(&drawing, 150, 50)), (2, 2));
        assert!(at_once * 10 < one_by_one, "at once {at_once:?}, one by one {one_by_one:?}");
    }

    #[test]
    fn a_gap_wider_than_asked_for_is_refused_and_a_narrower_one_is_closed() {
        let mut drawing = drawing_of(&square(10.0));
        let at = Point::new(50.0, 50.0);
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, at, SwatchId(2), 4.0, &[]), Err(FillError::Open));
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, at, SwatchId(2), 12.0, &[]), Ok(true));
        assert_eq!(painted(&drawing, 50, 50), 2);
    }

    #[test]
    fn lines_that_touch_where_they_are_drawn_close_though_their_middles_stop_short() {
        // Each side stops 1.5 pixels short of the next, inside the 4 pixel width of the lines.
        let short = vec![line((1.5, 0.0), (100.0, 0.0)), line((100.0, 1.5), (100.0, 100.0)), line((98.5, 100.0), (0.0, 100.0)), line((0.0, 98.5), (0.0, 0.0))];
        let mut drawing = drawing_of(&short);
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 0.0, &[]), Ok(true));
        // A gap wider than the lines still needs the gap closed.
        let mut drawing = drawing_of(&square(10.0));
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 0.0, &[]), Err(FillError::Open));
    }

    #[test]
    fn filled_again_it_takes_the_new_swatch_and_unpaint_takes_it_away() {
        let mut drawing = drawing_of(&square(0.0));
        let at = Point::new(50.0, 50.0);
        fill_vector(&mut drawing, ArtLayerKind::ColourArt, at, SwatchId(2), 0.0, &[]).unwrap();
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(20.0, 70.0), SwatchId(3), 0.0, &[]), Ok(true));
        assert_eq!((colour(&drawing).shapes().len(), painted(&drawing, 50, 50)), (1, 3), "one fill, recoloured");
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, at, SwatchId(4), 0.0, &[SwatchId(3)]), Err(FillError::Protected));
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, at, SwatchId::NONE, 0.0, &[]), Ok(true));
        assert!(colour(&drawing).is_empty());
        assert_eq!(fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(1.0, 50.0), SwatchId(2), 0.0, &[]), Err(FillError::OnLine));
    }

    #[test]
    fn a_fill_follows_its_lines_and_is_left_open_when_they_no_longer_close() {
        let mut drawing = drawing_of(&square(0.0));
        fill_vector(&mut drawing, ArtLayerKind::ColourArt, Point::new(50.0, 50.0), SwatchId(2), 0.0, &[]).unwrap();
        // The right side moved out to 150: the fill reaches it.
        let before = drawing.clone();
        let moved =
            vec![line((0.0, 0.0), (150.0, 0.0)), line((150.0, 0.0), (150.0, 100.0)), line((150.0, 100.0), (0.0, 100.0)), line((0.0, 100.0), (0.0, 0.0))];
        let lines = drawing_of(&moved).layer(ArtLayerKind::LineArt).unwrap().clone();
        *drawing.layer_mut(ArtLayerKind::LineArt).unwrap() = lines;
        refit_fills(&mut drawing, &before);
        assert_eq!(painted(&drawing, 140, 50), 2, "the fill went with the line");
        // The left side taken away: the lines no longer close, and the fill stays as it was.
        let before = drawing.clone();
        let Some(ArtContent::Vector(art)) = drawing.layer(ArtLayerKind::LineArt).map(|l| l.content.clone()) else { panic!() };
        drawing.layer_mut(ArtLayerKind::LineArt).unwrap().content = ArtContent::Vector(art.edited(|shapes| _ = shapes.pop()));
        refit_fills(&mut drawing, &before);
        let ShapeKind::Fill(fill) = &colour(&drawing).shapes()[0].kind else { panic!("a fill") };
        assert!(fill.open && painted(&drawing, 140, 50) == 2);
    }

    #[test]
    fn the_bucket_on_lines_recolours_the_stroke_under_it() {
        let mut drawing = drawing_of(&square(0.0));
        assert_eq!(recolour_vector_line(&mut drawing, ArtLayerKind::LineArt, Point::new(50.0, 1.0), SwatchId(5), &[]), Ok(true));
        let Some(ArtContent::Vector(lines)) = drawing.layer(ArtLayerKind::LineArt).map(|l| &l.content) else { panic!() };
        assert_eq!(lines.shapes().iter().map(|shape| shape.swatch.0).collect::<Vec<_>>(), [5, 1, 1, 1]);
        assert_eq!(recolour_vector_line(&mut drawing, ArtLayerKind::LineArt, Point::new(50.0, 50.0), SwatchId(5), &[]), Err(FillError::NoLine));
    }
}
