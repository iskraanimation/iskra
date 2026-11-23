//! The eraser on vector art (plan §4.16, step 4).
//!
//! Where the eraser goes across a stroke's whole width, the stroke is cut there, and each piece
//! stays a stroke with its middle line, its width and its nib. A piece the eraser only bites
//! into at an edge becomes the filled outline of what is left of it, as does a piece it goes
//! through the middle of without reaching both edges. A fill loses what the eraser covers.

use kurbo::{BezPath, ParamCurve, ParamCurveArclen, PathSeg, Point, Rect as Bounds, Shape as _};
use vectorcraft_geom::{FillRule, PathData};
use vectorcraft_pathops::BoolOp;

use crate::{Cap, ShapeKind, SwatchId, VectorArt, VectorFill, VectorShape, VectorStroke};

/// The eraser's area: the points along its middle line, each with its radius.
struct Eraser {
    samples: Vec<(Point, f64)>,
    outline: BezPath,
    bounds: Bounds,
}

impl Eraser {
    fn of(stroke: &VectorStroke) -> Eraser {
        let outline = stroke.contour();
        Eraser { samples: stroke.samples(), bounds: outline.bounding_box(), outline }
    }

    /// How far `at` is outside the eraser: below zero, how far inside.
    fn outside(&self, at: Point) -> f64 {
        let disc = |(centre, radius): (Point, f64)| centre.distance(at) - radius;
        let mut nearest = self.samples.iter().copied().map(disc).fold(f64::INFINITY, f64::min);
        for pair in self.samples.windows(2) {
            let [(a, ra), (b, rb)] = [pair[0], pair[1]];
            let along = b - a;
            let length = along.hypot2();
            if length <= 1e-12 {
                continue;
            }
            let t = ((at - a).dot(along) / length).clamp(0.0, 1.0);
            nearest = nearest.min(at.distance(a + along * t) - (ra + (rb - ra) * t));
        }
        nearest
    }
}

/// A place on a stroke's middle line: its segment, and how far along it (0..1).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    segment: usize,
    t: f64,
}

/// A point along a stroke where the eraser is looked at.
struct Sample {
    place: Place,
    at: Point,
    radius: f64,
    /// How far along the stroke from its start.
    along: f64,
}

/// `art` with what `eraser` covers taken away, leaving the shapes of the `protected` swatches
/// as they are. `None` if it covers nothing.
pub fn erase_vector(art: &VectorArt, eraser: &VectorStroke, protected: &[SwatchId]) -> Option<VectorArt> {
    let eraser = Eraser::of(eraser);
    let mut next = art.next_id().0;
    let mut changed = false;
    let mut shapes = Vec::with_capacity(art.shapes().len());
    for shape in art.shapes() {
        let reach = shape.kind.area().bounding_box();
        if protected.contains(&shape.swatch) || reach.intersect(eraser.bounds).area() <= 0.0 {
            shapes.push(shape.clone());
            continue;
        }
        let left = match &shape.kind {
            ShapeKind::Stroke(stroke) => erase_stroke(stroke, &eraser),
            ShapeKind::Fill(fill) => erase_area(&fill.outline, &eraser)
                .map(|outline| vec![ShapeKind::Fill(VectorFill { tracks: fill.tracks.clone(), boil: fill.boil, ..VectorFill::plain(outline) })]),
        };
        let Some(left) = left else {
            shapes.push(shape.clone());
            continue;
        };
        changed = true;
        // The first piece keeps the shape's id; the others are new shapes.
        for (index, kind) in left.into_iter().enumerate() {
            let id = if index == 0 {
                shape.id
            } else {
                next += 1;
                crate::ShapeId(next - 1)
            };
            shapes.push(VectorShape { id, swatch: shape.swatch, kind });
        }
    }
    changed.then(|| VectorArt::new(shapes))
}

/// `art` cut along the edge of `region`, a closed outline (the Cutter tool's box or lasso):
/// each stroke whose middle line crosses it is cut where it does, and each fill along it. What
/// is inside is taken away, or with `keep` kept as shapes of its own, apart from what is
/// outside by a pixel or so, so that a selection of `region` takes it alone. The shapes of the
/// `protected` swatches stay as they are. `None` if nothing changes.
/// The new ends are finished with `cap`.
pub fn cut_vector(art: &VectorArt, region: &BezPath, protected: &[SwatchId], keep: bool, cap: Cap) -> Option<VectorArt> {
    let bounds = region.bounding_box();
    let inside = |at: Point| region.winding(at) != 0;
    let mut next = art.next_id().0;
    let mut changed = false;
    let mut shapes = Vec::with_capacity(art.shapes().len());
    for shape in art.shapes() {
        let reach = shape.kind.area().bounding_box();
        if protected.contains(&shape.swatch) || reach.intersect(bounds).area() <= 0.0 {
            shapes.push(shape.clone());
            continue;
        }
        let left = match &shape.kind {
            ShapeKind::Stroke(stroke) => cut_stroke(stroke, &inside, keep, cap),
            // The pieces keep the surface's grain.
            ShapeKind::Fill(fill) => cut_area(&fill.outline, region, keep).map(|pieces| {
                pieces
                    .into_iter()
                    .map(|piece| match piece {
                        ShapeKind::Fill(piece) => ShapeKind::Fill(VectorFill { tracks: fill.tracks.clone(), boil: fill.boil, ..piece }),
                        other => other,
                    })
                    .collect()
            }),
        };
        let Some(left) = left else {
            shapes.push(shape.clone());
            continue;
        };
        changed = true;
        for (index, kind) in left.into_iter().enumerate() {
            let id = if index == 0 {
                shape.id
            } else {
                next += 1;
                crate::ShapeId(next - 1)
            };
            shapes.push(VectorShape { id, swatch: shape.swatch, kind });
        }
    }
    changed.then(|| VectorArt::new(shapes))
}

/// `art` with what a flick of the Cutter tool goes across cut away: of each stroke whose
/// middle line the flick crosses, the stretch between the crossings with other strokes on
/// either side of it, or with the stroke's end where there is none on that side: the overshoot
/// of a line past a crossing, or a piece between two, however little of it the flick goes
/// along. A stroke that crosses no other goes whole.
/// `flick` is the points of the pointer's way; the shapes of the `protected` swatches stay.
/// `None` if nothing is cut. The new ends are finished with `cap`.
pub fn cut_overshoot(art: &VectorArt, flick: &[Point], protected: &[SwatchId], cap: Cap) -> Option<VectorArt> {
    let lines: Vec<Vec<Sample>> =
        art.shapes().iter().map(|shape| if let ShapeKind::Stroke(stroke) = &shape.kind { sample(stroke) } else { Vec::new() }).collect();
    let bounds = |points: &mut dyn Iterator<Item = Point>| {
        points.fold(Bounds::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |b, at| b.union_pt(at))
    };
    let boxes: Vec<Bounds> = lines.iter().map(|line| bounds(&mut line.iter().map(|sample| sample.at)).inflate(0.5, 0.5)).collect();
    let flicked = bounds(&mut flick.iter().copied());
    let mut next = art.next_id().0;
    let mut changed = false;
    let mut shapes = Vec::with_capacity(art.shapes().len());
    for (index, shape) in art.shapes().iter().enumerate() {
        let (ShapeKind::Stroke(stroke), line) = (&shape.kind, &lines[index]) else {
            shapes.push(shape.clone());
            continue;
        };
        let crossed =
            (!protected.contains(&shape.swatch) && boxes[index].intersect(flicked).area() >= 0.0).then(|| crossings(line, flick).into_iter().next()).flatten();
        // Where the other strokes cross it, by the sample there.
        let others = crossed.map(|_| {
            let near = (0..lines.len()).filter(|other| *other != index && boxes[*other].intersect(boxes[index]).area() >= 0.0);
            near.flat_map(|other| crossings(line, &lines[other].iter().map(|sample| sample.at).collect::<Vec<_>>())).collect::<Vec<usize>>()
        });
        let (Some(at), Some(others)) = (crossed, others) else {
            shapes.push(shape.clone());
            continue;
        };
        let (before, after) = (others.iter().copied().filter(|other| *other < at).max(), others.iter().copied().filter(|other| *other >= at).min());
        // With none on either side, nothing is kept of it.
        changed = true;
        let last = line.len() - 1;
        let kept = [before.map(|before| (0, before)), after.map(|after| (after, last))];
        let pieces = kept.into_iter().flatten().filter_map(|(from, to)| piece(stroke, line[from].place, line[to].place, cap));
        for (index, piece) in pieces.enumerate() {
            let id = if index == 0 {
                shape.id
            } else {
                next += 1;
                crate::ShapeId(next - 1)
            };
            shapes.push(VectorShape { id, swatch: shape.swatch, kind: ShapeKind::Stroke(piece) });
        }
    }
    changed.then(|| VectorArt::new(shapes))
}

/// `art` with the stroke whose middle line passes within `reach` of `at` split where other
/// strokes cross it either side of `at`, so that the piece between those crossings, or between
/// one and the stroke's end, is a stroke of its own; and that piece's id and its middle line.
/// The new ends are round, as they meet where they were. A stroke that crosses none is its own
/// piece, whole. `None` where no stroke is near, or it is of a `protected` swatch.
///
/// Where a surface is under `at` above the nearest stroke, it is the surface that is split
/// ([`split_surface`]), and the piece's outline is given in place of a middle line.
pub fn split_piece(art: &VectorArt, at: Point, reach: f64, protected: &[SwatchId]) -> Option<(VectorArt, crate::ShapeId, BezPath)> {
    let surface = art.shapes().iter().rposition(|shape| matches!(&shape.kind, ShapeKind::Fill(fill) if !fill.between_lines() && fill.outline.winding(at) != 0));
    let stroke = split_line(art, at, reach, protected);
    match (surface, &stroke) {
        (Some(surface), Some((_, id, _))) if art.shapes().iter().position(|shape| shape.id == *id).is_some_and(|line| line < surface) => {
            split_surface(art, surface, at, protected)
        }
        (Some(surface), None) => split_surface(art, surface, at, protected),
        _ => stroke,
    }
}

/// The surface at `index` of `art` split along the edges of the strokes and surfaces that cross
/// it (the user's of 2026-11-04): the part of it round `at` that none of them crosses, or the
/// part under one of them, as the Select tool's *Pieces* takes a line's piece between its
/// crossings. The piece keeps the surface's id and the rest is one surface of its own beside it,
/// both with its grain. A surface that nothing crosses is its own piece, whole; one of a
/// `protected` swatch is not split.
fn split_surface(art: &VectorArt, index: usize, at: Point, protected: &[SwatchId]) -> Option<(VectorArt, crate::ShapeId, BezPath)> {
    let shape = &art.shapes()[index];
    let ShapeKind::Fill(fill) = &shape.kind else { return None };
    if protected.contains(&shape.swatch) {
        return None;
    }
    let whole = PathData::from_bezpath(&fill.outline);
    let reach = fill.outline.bounding_box();
    let area = |path: &PathData| vectorcraft_pathops::area(path, FillRule::NonZero).abs();
    // What crosses it: a stroke or a surface over part of it and not all.
    let crossing: Vec<PathData> = art
        .shapes()
        .iter()
        .enumerate()
        .filter(|(other, _)| *other != index)
        .filter(|(_, other)| matches!(&other.kind, ShapeKind::Stroke(_)) || matches!(&other.kind, ShapeKind::Fill(fill) if !fill.between_lines()))
        .filter(|(_, other)| other.kind.area().bounding_box().intersect(reach).area() > 0.0)
        .map(|(_, other)| PathData::from_bezpath(&other.kind.area()))
        .filter(|other| {
            let over = vectorcraft_pathops::boolean(&whole, FillRule::NonZero, other, FillRule::NonZero, BoolOp::Intersect);
            let left = vectorcraft_pathops::boolean(&whole, FillRule::NonZero, other, FillRule::NonZero, BoolOp::Difference);
            area(&over) > 0.25 && area(&left) > 0.25
        })
        .collect();
    let alone = || Some((art.clone(), shape.id, fill.outline.clone()));
    if crossing.is_empty() {
        return alone();
    }
    let mut edges = vec![vectorcraft_pathops::Shape::new(whole.clone(), FillRule::NonZero, 0)];
    edges.extend(crossing.into_iter().enumerate().map(|(n, other)| vectorcraft_pathops::Shape::new(other, FillRule::NonZero, n as u64 + 1)));
    let Some(region) = vectorcraft_pathops::region_at(&edges, at).filter(|region| region.sources.contains(&0)) else { return alone() };
    let rest = vectorcraft_pathops::boolean(&whole, FillRule::NonZero, &region.path, FillRule::NonZero, BoolOp::Difference);
    if area(&rest) < 0.25 {
        return alone();
    }
    let piece = region.path.to_bezpath();
    let tracks = fill.tracks.clone();
    let rest = VectorShape {
        id: art.next_id(),
        swatch: shape.swatch,
        kind: ShapeKind::Fill(VectorFill { tracks: tracks.clone(), boil: fill.boil, ..VectorFill::plain(rest.to_bezpath()) }),
    };
    let kept =
        VectorShape { id: shape.id, swatch: shape.swatch, kind: ShapeKind::Fill(VectorFill { tracks, boil: fill.boil, ..VectorFill::plain(piece.clone()) }) };
    let split = art.edited(|shapes| {
        shapes.splice(index..=index, [rest, kept]);
    });
    Some((split, shape.id, piece))
}

/// [`split_piece`] for a stroke alone.
fn split_line(art: &VectorArt, at: Point, reach: f64, protected: &[SwatchId]) -> Option<(VectorArt, crate::ShapeId, BezPath)> {
    let lines: Vec<Vec<Sample>> =
        art.shapes().iter().map(|shape| if let ShapeKind::Stroke(stroke) = &shape.kind { sample(stroke) } else { Vec::new() }).collect();
    // The nearest stroke's nearest sample, from the top down.
    let mut nearest: Option<(usize, usize, f64)> = None;
    for (index, line) in lines.iter().enumerate().rev() {
        for (sample, point) in line.iter().enumerate() {
            let distance = (point.at - at).hypot();
            if distance <= reach && nearest.is_none_or(|(.., best)| distance < best) {
                nearest = Some((index, sample, distance));
            }
        }
    }
    let (index, near, _) = nearest?;
    let shape = &art.shapes()[index];
    let ShapeKind::Stroke(stroke) = &shape.kind else { return None };
    if protected.contains(&shape.swatch) {
        return None;
    }
    let line = &lines[index];
    let others: Vec<usize> = (0..lines.len())
        .filter(|other| *other != index)
        .flat_map(|other| crossings(line, &lines[other].iter().map(|sample| sample.at).collect::<Vec<_>>()))
        .collect();
    let (before, after) = (others.iter().copied().filter(|other| *other < near).max(), others.iter().copied().filter(|other| *other > near).min());
    let last = line.len() - 1;
    let (from, to) = (before.unwrap_or(0), after.unwrap_or(last));
    if before.is_none() && after.is_none() {
        return Some((art.clone(), shape.id, stroke.path.clone()));
    }
    let mut next = art.next_id().0;
    let mut new_id = || {
        next += 1;
        crate::ShapeId(next - 1)
    };
    let mut kept = Vec::new();
    let mut shapes = Vec::with_capacity(art.shapes().len() + 2);
    let cut = Cap::Round;
    for (other, one) in art.shapes().iter().enumerate() {
        if other != index {
            shapes.push(one.clone());
            continue;
        }
        let parts = [(0, from), (from, to), (to, last)];
        for (part, (start, end)) in parts.into_iter().enumerate() {
            if start >= end {
                continue;
            }
            let Some(piece) = piece(stroke, line[start].place, line[end].place, cut) else { continue };
            // The piece keeps the stroke's id, the rest take new ones.
            let id = if part == 1 { shape.id } else { new_id() };
            if part == 1 {
                kept.push(piece.path.clone());
            }
            shapes.push(VectorShape { id, swatch: shape.swatch, kind: ShapeKind::Stroke(piece) });
        }
    }
    Some((VectorArt::new(shapes), shape.id, kept.pop()?))
}

/// Where the polyline through `others` crosses the middle line sampled as `line`: the index of
/// the sample of `line` the crossing is at, first along the line first.
fn crossings(line: &[Sample], others: &[Point]) -> Vec<usize> {
    let mut found = Vec::new();
    for (index, pair) in line.windows(2).enumerate() {
        let (a, b) = (pair[0].at, pair[1].at);
        for other in others.windows(2) {
            let (c, d) = (other[0], other[1]);
            let (ab, cd, ac) = (b - a, d - c, c - a);
            let across = ab.cross(cd);
            if across.abs() <= 1e-12 {
                continue;
            }
            let (t, u) = (ac.cross(cd) / across, ac.cross(ab) / across);
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                found.push(if t < 0.5 { index } else { index + 1 });
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    found
}

/// `art` with the shape `new` merged with the shapes of its swatch it overlaps into one fill of
/// their outline, on top, where `new` is drawn, as a brush set to flatten leaves it; shapes of
/// other swatches stay apart. `None` if it overlaps none, and is to be laid as it is.
pub fn flattened(art: &VectorArt, new: &VectorShape) -> Option<VectorArt> {
    // One whose tone changes along it lies on top, its own, and is never merged: faint strokes
    // build up where they overlap, as paint does (decided on 2026-11-04, the user's to change).
    if toned(&new.kind) {
        return None;
    }
    let (area, reach) = (new.kind.area(), new.kind.area().bounding_box());
    let mut merged = PathData::from_bezpath(&area);
    let mut taken = vec![false; art.shapes().len()];
    // Of one grain only, that a surface of paper texture is not made plain, nor the other way.
    let own = grain_of(&new.kind);
    let mut tracks = tracks_of(&new.kind);
    for (index, shape) in art.shapes().iter().enumerate() {
        if shape.swatch != new.swatch
            || shape.kind.area().bounding_box().intersect(reach).area() <= 0.0
            || grain_of(&shape.kind) != own
            || toned(&shape.kind)
            || shape.kind.boil() != new.kind.boil()
        {
            continue;
        }
        let other = PathData::from_bezpath(&shape.kind.area());
        let both = vectorcraft_pathops::boolean(&other, FillRule::NonZero, &PathData::from_bezpath(&area), FillRule::NonZero, BoolOp::Intersect);
        if vectorcraft_pathops::area(&both, FillRule::NonZero).abs() > 0.01 {
            merged = vectorcraft_pathops::boolean(&merged, FillRule::NonZero, &other, FillRule::NonZero, BoolOp::Union);
            taken[index] = true;
            tracks.extend(tracks_of(&shape.kind));
        }
    }
    if !taken.contains(&true) {
        return None;
    }
    let mut kept = taken.iter();
    let mut shapes: Vec<VectorShape> = art.shapes().iter().filter(|_| !kept.next().copied().unwrap_or(false)).cloned().collect();
    let fill = VectorFill { tracks: if own.is_some() { tracks } else { Vec::new() }, boil: new.kind.boil(), ..VectorFill::plain(merged.to_bezpath()) };
    shapes.push(VectorShape { id: new.id, swatch: new.swatch, kind: ShapeKind::Fill(fill) });
    Some(VectorArt::new(shapes))
}

/// A shape's grain, the texture and second tip that cut it, which shapes merged into one share:
/// `None` where it has none.
fn grain_of(kind: &ShapeKind) -> Option<(photocraft_paint::Texture, photocraft_paint::DualBrush)> {
    let track = match kind {
        ShapeKind::Stroke(stroke) => Some(stroke),
        ShapeKind::Fill(fill) => fill.tracks.first(),
    };
    track.filter(|track| track.grained()).and_then(|track| track.grain.as_ref()).map(|grain| (grain.texture.clone(), grain.dual_brush.clone()))
}

/// Whether a shape's tone changes along it: a stroke's, or that of a stroke a surface was drawn
/// from.
fn toned(kind: &ShapeKind) -> bool {
    match kind {
        ShapeKind::Stroke(stroke) => stroke.toned(),
        ShapeKind::Fill(fill) => fill.tracks.iter().any(VectorStroke::toned),
    }
}

/// The strokes whose grain or tone a shape has: a stroke's own, a surface's tracks.
fn tracks_of(kind: &ShapeKind) -> Vec<VectorStroke> {
    match kind {
        ShapeKind::Stroke(stroke) if stroke.marks() => vec![stroke.clone()],
        ShapeKind::Stroke(_) => Vec::new(),
        ShapeKind::Fill(fill) => fill.tracks.clone(),
    }
}

/// `art` with the shapes a selection takes merged, swatch by swatch: those of one swatch and one
/// grain made one fill of their outline, where the topmost of them was, keeping the grain, and
/// those of other swatches or grains kept apart, as a brush set to flatten does. `None` if it
/// takes no two of one swatch and grain.
pub fn merge_picked(art: &VectorArt, picked: &crate::Picked) -> Option<VectorArt> {
    let (shapes, picks) = (art.shapes(), art.picks(picked));
    // By swatch, and within one by grain and boil, in the order they are first met.
    let mut by_swatch: Vec<(SwatchId, Option<(photocraft_paint::Texture, photocraft_paint::DualBrush)>, Vec<usize>)> = Vec::new();
    // Those whose tone changes along them stay apart, as the Brush leaves them.
    for (at, shape) in shapes.iter().enumerate().filter(|(at, shape)| picks[*at] && !toned(&shape.kind)) {
        let grain = grain_of(&shape.kind);
        let alike = |group: &[usize]| group.first().is_none_or(|first| shapes[*first].kind.boil() == shape.kind.boil());
        match by_swatch.iter_mut().find(|(swatch, of, group)| *swatch == shape.swatch && *of == grain && alike(group)) {
            Some((_, _, group)) => group.push(at),
            None => by_swatch.push((shape.swatch, grain, vec![at])),
        }
    }
    let mut merged: std::collections::HashMap<usize, VectorShape> = std::collections::HashMap::new();
    let mut gone = std::collections::HashSet::new();
    for (_, grain, group) in by_swatch.iter().filter(|(_, _, group)| group.len() >= 2) {
        let outline = group
            .iter()
            .map(|at| PathData::from_bezpath(&shapes[*at].kind.area()))
            .reduce(|all, more| vectorcraft_pathops::boolean(&all, FillRule::NonZero, &more, FillRule::NonZero, BoolOp::Union));
        let (Some(outline), Some(&top)) = (outline, group.last()) else { continue };
        let shape = &shapes[top];
        let tracks = if grain.is_some() { group.iter().flat_map(|at| tracks_of(&shapes[*at].kind)).collect() } else { Vec::new() };
        let fill = VectorFill { tracks, boil: shape.kind.boil(), ..VectorFill::plain(outline.to_bezpath()) };
        merged.insert(top, VectorShape { id: shape.id, swatch: shape.swatch, kind: ShapeKind::Fill(fill) });
        gone.extend(group.iter().copied().filter(|at| *at != top));
    }
    if merged.is_empty() {
        return None;
    }
    let kept = shapes.iter().enumerate().filter(|(at, _)| !gone.contains(at));
    Some(VectorArt::new(kept.map(|(at, shape)| merged.remove(&at).unwrap_or_else(|| shape.clone())).collect()))
}

/// `art` with the shapes that `area` touches given the swatch `to`, as a brush set to repaint
/// does: none is added, and none changes its shape. The shapes of the `protected` swatches stay
/// as they are. `None` if it touches none that isn't of `to` already.
pub fn repaint_touched(art: &VectorArt, area: &BezPath, to: SwatchId, protected: &[SwatchId]) -> Option<VectorArt> {
    let (reach, brush) = (area.bounding_box(), PathData::from_bezpath(area));
    let touched = |shape: &VectorShape| {
        if shape.swatch == to || protected.contains(&shape.swatch) || shape.kind.area().bounding_box().intersect(reach).area() <= 0.0 {
            return false;
        }
        let both = vectorcraft_pathops::boolean(&PathData::from_bezpath(&shape.kind.area()), FillRule::NonZero, &brush, FillRule::NonZero, BoolOp::Intersect);
        vectorcraft_pathops::area(&both, FillRule::NonZero).abs() > 0.01
    };
    let picks: Vec<bool> = art.shapes().iter().map(touched).collect();
    picks.contains(&true).then(|| {
        art.edited(|shapes| {
            for (shape, _) in shapes.iter_mut().zip(&picks).filter(|(_, touched)| **touched) {
                shape.swatch = to;
            }
        })
    })
}

/// The pieces of `stroke` outside the region, and with `keep` those inside too, or `None` if
/// its middle line is all outside, or all inside and kept.
fn cut_stroke(stroke: &VectorStroke, inside: &impl Fn(Point) -> bool, keep: bool, cap: Cap) -> Option<Vec<ShapeKind>> {
    let samples = sample(stroke);
    let within: Vec<bool> = samples.iter().map(|sample| inside(sample.at)).collect();
    if within.iter().all(|within| !within) || (keep && within.iter().all(|within| *within)) {
        return None;
    }
    // Runs of samples on one side, each a piece; those inside go unless they are kept.
    let mut left = Vec::new();
    let mut at = 0;
    while at < samples.len() {
        let start = at;
        while at < samples.len() && within[at] == within[start] {
            at += 1;
        }
        if within[start] && !keep {
            continue;
        }
        // A kept piece inside stops a sample short of the edge, where it would touch the
        // pixels outside.
        let (mut first, mut last) = (start, at - 1);
        if within[start] && last > first + 2 {
            (first, last) = (first + usize::from(start > 0), last - usize::from(at < samples.len()));
        }
        let (from, to) = (samples[first].place, samples[last].place);
        left.extend(piece(stroke, from, to, cap).map(ShapeKind::Stroke));
    }
    Some(left)
}

/// What of `outline` is outside the region, and with `keep` what is inside as a fill of its
/// own, or `None` if the region misses it, or holds it whole and it is kept.
fn cut_area(outline: &BezPath, region: &BezPath, keep: bool) -> Option<Vec<ShapeKind>> {
    let (before, region) = (PathData::from_bezpath(outline), PathData::from_bezpath(region));
    let part = |op| vectorcraft_pathops::boolean(&before, FillRule::NonZero, &region, FillRule::NonZero, op);
    let area = |path: &PathData| vectorcraft_pathops::area(path, FillRule::NonZero).abs();
    let (outside, within) = (part(BoolOp::Difference), part(BoolOp::Intersect));
    if area(&within) <= 0.01 || (keep && area(&outside) <= 0.01) {
        return None;
    }
    let mut left = Vec::new();
    for (path, taken) in [(outside, true), (within, keep)] {
        if taken && area(&path) > 0.01 {
            left.push(ShapeKind::Fill(VectorFill::plain(path.to_bezpath())));
        }
    }
    Some(left)
}

/// What is left of `outline` where the eraser has been, or `None` if it is untouched. An empty
/// path where nothing is left.
fn erase_area(outline: &BezPath, eraser: &Eraser) -> Option<BezPath> {
    let before = PathData::from_bezpath(outline);
    let after = vectorcraft_pathops::boolean(&before, FillRule::NonZero, &PathData::from_bezpath(&eraser.outline), FillRule::NonZero, BoolOp::Difference);
    let (was, is) = (vectorcraft_pathops::area(&before, FillRule::NonZero).abs(), vectorcraft_pathops::area(&after, FillRule::NonZero).abs());
    (was - is > 0.01).then(|| after.to_bezpath())
}

/// What is left of `stroke` where the eraser has been, as shapes, or `None` if it is untouched.
fn erase_stroke(stroke: &VectorStroke, eraser: &Eraser) -> Option<Vec<ShapeKind>> {
    let samples = sample(stroke);
    let first = samples.first()?;
    // A dot is all or nothing.
    if samples.len() == 1 {
        return (eraser.outside(first.at) <= first.radius).then(Vec::new);
    }
    let outside: Vec<f64> = samples.iter().map(|sample| eraser.outside(sample.at)).collect();
    if outside.iter().zip(&samples).all(|(outside, sample)| *outside > sample.radius) {
        return None;
    }
    // Runs of samples whose middle is under the eraser: a cut where it covers the whole width
    // somewhere in the run, else a bite.
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    let mut bitten = false;
    let mut at = 0;
    while at < samples.len() {
        if outside[at] > 0.0 {
            at += 1;
            continue;
        }
        let start = at;
        while at < samples.len() && outside[at] <= 0.0 {
            at += 1;
        }
        if (start..at).any(|i| outside[i] <= -samples[i].radius) {
            cuts.push((start, at - 1));
        } else {
            bitten = true;
        }
    }
    // The pieces between the cuts, by their first and last sample.
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut from = 0;
    for &(start, end) in &cuts {
        if start > from {
            pieces.push((from, start));
        }
        from = end;
    }
    if cuts.last().is_none_or(|&(_, end)| end < samples.len() - 1) {
        pieces.push((if cuts.is_empty() { 0 } else { from }, samples.len() - 1));
    }
    let largest = eraser.samples.iter().map(|(_, radius)| *radius).fold(0.0, f64::max);
    let mut left = Vec::new();
    for (start, end) in pieces {
        // Next to a cut, where the eraser leaves the line, it reaches the line's edges: that is
        // the end of the cut, not a bite.
        let cut_at_start = cuts.iter().any(|&(_, cut_end)| cut_end == start);
        let cut_at_end = cuts.iter().any(|&(cut_start, _)| cut_start == end);
        let near_cut = |i: usize| {
            let margin = samples[i].radius + largest;
            (cut_at_start && samples[i].along - samples[start].along <= margin) || (cut_at_end && samples[end].along - samples[i].along <= margin)
        };
        let touched = bitten || (start..=end).any(|i| outside[i] <= samples[i].radius && outside[i] > 0.0 && !near_cut(i));
        let Some(piece) = piece(stroke, samples[start].place, samples[end].place, Cap::Round) else { continue };
        if touched {
            if let Some(outline) = erase_area(&piece.contour(), eraser).or_else(|| Some(piece.contour()))
                && !outline.elements().is_empty()
            {
                left.push(ShapeKind::Fill(VectorFill::plain(outline)));
            }
        } else {
            left.push(ShapeKind::Stroke(piece));
        }
    }
    Some(left)
}

/// Points along the stroke's middle line, a pixel or so apart, each with its radius there.
fn sample(stroke: &VectorStroke) -> Vec<Sample> {
    let radius = |at: usize| stroke.widths.get(at).or(stroke.widths.last()).copied().unwrap_or(0.0).max(0.0) / 2.0;
    let mut samples = Vec::new();
    let mut along = 0.0;
    for (segment, curve) in stroke.path.segments().enumerate() {
        let (from, to) = (radius(segment), radius(segment + 1));
        let length = curve.arclen(0.05);
        let steps = length.ceil().clamp(1.0, 4096.0) as usize;
        let first = if segment == 0 { 0 } else { 1 };
        for step in first..=steps {
            let t = step as f64 / steps as f64;
            samples.push(Sample { place: Place { segment, t }, at: curve.eval(t), radius: from + (to - from) * t, along: along + length * t });
        }
        along += length;
    }
    if samples.is_empty()
        && let Some(kurbo::PathEl::MoveTo(at)) = stroke.path.elements().first()
    {
        samples.push(Sample { place: Place { segment: 0, t: 0.0 }, at: *at, radius: radius(0), along: 0.0 });
    }
    samples
}

/// The part of `stroke` from `from` to `to`, with its widths, opacities and drifts, or `None` if it
/// is next to nothing. An end of it that is not one of the stroke's is finished with `cut`.
fn piece(stroke: &VectorStroke, from: Place, to: Place, cut: Cap) -> Option<VectorStroke> {
    let segments: Vec<PathSeg> = stroke.path.segments().collect();
    let (starts, ends) = (from.segment == 0 && from.t <= 1e-9, to.segment + 1 >= segments.len() && to.t >= 1.0 - 1e-9);
    let caps = [if starts { stroke.caps[0] } else { cut }, if ends { stroke.caps[1] } else { cut }];
    let value = |of: &[f64], at: usize| of.get(at).or(of.last()).copied().unwrap_or(0.0);
    let value_at = |of: &[f64], place: Place| value(of, place.segment) + (value(of, place.segment + 1) - value(of, place.segment)) * place.t;
    let lists = [&stroke.widths, &stroke.opacities, &stroke.drifts];
    // Each list as the piece has it, an empty one staying empty.
    let mut values: [Vec<f64>; 3] = lists.map(|of| if of.is_empty() { Vec::new() } else { vec![value_at(of, from)] });
    let mut path = BezPath::new();
    let push = |curve: PathSeg, path: &mut BezPath| match curve {
        PathSeg::Line(line) => path.line_to(line.p1),
        PathSeg::Quad(quad) => path.quad_to(quad.p1, quad.p2),
        PathSeg::Cubic(cubic) => path.curve_to(cubic.p1, cubic.p2, cubic.p3),
    };
    path.move_to(segments.get(from.segment)?.eval(from.t));
    for segment in from.segment..=to.segment {
        let (t0, t1) = (if segment == from.segment { from.t } else { 0.0 }, if segment == to.segment { to.t } else { 1.0 });
        if t1 - t0 <= 1e-9 {
            continue;
        }
        push(segments[segment].subsegment(t0..t1), &mut path);
        for (out, of) in values.iter_mut().zip(lists).filter(|(_, of)| !of.is_empty()) {
            out.push(if segment == to.segment { value_at(of, to) } else { value(of, segment + 1) });
        }
    }
    let length: f64 = path.segments().map(|segment| segment.arclen(0.1)).sum();
    let [widths, opacities, drifts] = values;
    (length >= 0.5).then_some(VectorStroke {
        path,
        widths,
        nib: stroke.nib,
        caps,
        grain: stroke.grain.clone(),
        opacities,
        drifts,
        boil: stroke.boil,
        stamp: stroke.stamp.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShapeId;

    fn line(from: (f64, f64), to: (f64, f64)) -> BezPath {
        let mut path = BezPath::new();
        path.move_to(from);
        path.line_to(to);
        path
    }

    fn art_of(kind: ShapeKind) -> VectorArt {
        VectorArt::new(vec![VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind }])
    }

    fn stroke(from: (f64, f64), to: (f64, f64), width: f64) -> ShapeKind {
        ShapeKind::Stroke(VectorStroke::even(line(from, to), width))
    }

    #[test]
    fn an_eraser_across_a_line_cuts_it_in_two_strokes() {
        let art = art_of(stroke((0.0, 0.0), (200.0, 0.0), 6.0));
        let erased = erase_vector(&art, &VectorStroke::even(line((100.0, -40.0), (100.0, 40.0)), 20.0), &[]).unwrap();
        let pieces: Vec<(ShapeId, f64, f64)> = erased
            .shapes()
            .iter()
            .map(|shape| match &shape.kind {
                ShapeKind::Stroke(stroke) => (shape.id, stroke.path.segments().next().unwrap().start().x, stroke.path.segments().last().unwrap().end().x),
                ShapeKind::Fill(_) => panic!("strokes still"),
            })
            .collect();
        assert_eq!(pieces.len(), 2, "{pieces:?}");
        assert_eq!((pieces[0].0, pieces[1].0), (ShapeId(1), ShapeId(2)), "the first keeps its id");
        assert!(pieces[0].1 == 0.0 && (pieces[0].2 - 90.0).abs() < 1.5, "to where the eraser begins: {pieces:?}");
        assert!((pieces[1].1 - 110.0).abs() < 1.5 && pieces[1].2 == 200.0, "from where it ends: {pieces:?}");
        assert_eq!(erased.raster().pixel(100, 0).coverage, 0, "erased there");
    }

    fn ends(art: &VectorArt) -> Vec<(f64, f64)> {
        let x = |stroke: &VectorStroke| (stroke.path.segments().next().unwrap().start().x, stroke.path.segments().last().unwrap().end().x);
        art.shapes().iter().filter_map(|shape| if let ShapeKind::Stroke(stroke) = &shape.kind { Some(x(stroke)) } else { None }).collect()
    }

    #[test]
    fn a_cut_along_a_box_takes_what_is_inside_or_keeps_it_apart() {
        let art = art_of(stroke((0.0, 0.0), (200.0, 0.0), 6.0));
        let box_ = kurbo::Rect::new(80.0, -20.0, 120.0, 20.0).to_path(0.1);
        let cut = cut_vector(&art, &box_, &[], false, Cap::Round).unwrap();
        let pieces = ends(&cut);
        assert!(pieces.len() == 2 && (pieces[0].1 - 80.0).abs() < 1.5 && (pieces[1].0 - 120.0).abs() < 1.5, "{pieces:?}");
        assert_eq!(cut.raster().pixel(100, 0).coverage, 0, "the middle is gone");
        // Kept, the middle is a stroke of its own between the two, short of the edge.
        let kept = cut_vector(&art, &box_, &[], true, Cap::Round).unwrap();
        let pieces = ends(&kept);
        assert_eq!(pieces.len(), 3, "{pieces:?}");
        assert!(pieces[1].0 > 80.0 && pieces[1].1 < 120.0 && pieces[0].1 < 80.0 && pieces[2].0 >= 120.0, "{pieces:?}");
        // Missed, or held whole and kept, nothing changes; held whole, it goes.
        assert!(cut_vector(&art, &kurbo::Rect::new(0.0, 50.0, 10.0, 60.0).to_path(0.1), &[], false, Cap::Round).is_none());
        let all = kurbo::Rect::new(-10.0, -10.0, 210.0, 10.0).to_path(0.1);
        assert!(cut_vector(&art, &all, &[], true, Cap::Round).is_none() && cut_vector(&art, &all, &[], false, Cap::Round).unwrap().is_empty());
        assert!(cut_vector(&art, &box_, &[SwatchId(1)], false, Cap::Round).is_none(), "protected");
    }

    #[test]
    fn a_flick_across_an_overshoot_cuts_it_off_at_the_crossing() {
        // A line across from 0 to 200, and one down through it at 150.
        let shapes = vec![
            VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind: stroke((0.0, 0.0), (200.0, 0.0), 4.0) },
            VectorShape { id: ShapeId(2), swatch: SwatchId(1), kind: stroke((150.0, -50.0), (150.0, 50.0), 4.0) },
        ];
        let art = VectorArt::new(shapes);
        // A flick across the end past the crossing takes it off there.
        let cut = cut_overshoot(&art, &[Point::new(180.0, -10.0), Point::new(185.0, 10.0)], &[], Cap::Round).unwrap();
        let pieces = ends(&cut);
        assert_eq!(cut.shapes().len(), 2);
        assert!(pieces[0].0 == 0.0 && (pieces[0].1 - 150.0).abs() < 1.5, "{pieces:?}");
        // Across the other line's top, past the first: that goes.
        let cut = cut_overshoot(&art, &[Point::new(140.0, -30.0), Point::new(160.0, -30.0)], &[], Cap::Round).unwrap();
        let ShapeKind::Stroke(down) = &cut.shapes()[1].kind else { panic!("a stroke") };
        assert!((down.path.segments().next().unwrap().start().y).abs() < 1.5, "from the crossing down");
        // A short flick near the end takes all of the stretch from the crossing, not only what
        // it goes along; and so does one near the crossing.
        for flick in [[Point::new(197.0, -3.0), Point::new(198.0, 3.0)], [Point::new(152.0, -3.0), Point::new(153.0, 3.0)]] {
            let pieces = ends(&cut_overshoot(&art, &flick, &[], Cap::Round).unwrap());
            assert!(pieces[0].0 == 0.0 && (pieces[0].1 - 150.0).abs() < 1.5, "{pieces:?}");
        }
        // Across a line that crosses nothing else, all of it goes; where the flick misses, nothing.
        let alone = art_of(stroke((0.0, 0.0), (200.0, 0.0), 4.0));
        assert!(cut_overshoot(&alone, &[Point::new(100.0, -10.0), Point::new(100.0, 10.0)], &[], Cap::Round).unwrap().is_empty());
        assert!(cut_overshoot(&art, &[Point::new(100.0, 20.0), Point::new(120.0, 20.0)], &[], Cap::Round).is_none());
    }

    #[test]
    fn a_surface_is_split_along_what_crosses_it() {
        // A surface from 0 to 300, a line of another swatch across it at 100 over it.
        let block = kurbo::Rect::new(0.0, -20.0, 300.0, 20.0).to_path(0.1);
        let surface = VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind: ShapeKind::Fill(VectorFill::plain(block)) };
        let line = VectorShape { id: ShapeId(2), swatch: SwatchId(2), kind: stroke((100.0, -50.0), (100.0, 50.0), 4.0) };
        let art = VectorArt::new(vec![surface.clone(), line]);
        // Left of the line: the piece keeps the id, the rest is a surface of its own.
        let (split, id, piece) = split_piece(&art, Point::new(50.0, 0.0), 5.0, &[]).unwrap();
        let bounds = piece.bounding_box();
        assert!(id == ShapeId(1) && bounds.x0.abs() < 0.5 && (bounds.x1 - 98.0).abs() < 0.5, "{bounds:?}");
        assert_eq!(split.shapes().len(), 3);
        let kept = split.shapes().iter().find(|shape| shape.id == id).unwrap();
        assert!(
            matches!(&kept.kind, ShapeKind::Fill(fill) if fill.outline.winding(Point::new(50.0, 0.0)) != 0 && fill.outline.winding(Point::new(200.0, 0.0)) == 0)
        );
        // On the line over it, the line's piece; with nothing across it, the surface whole.
        let (_, on_line, _) = split_piece(&art, Point::new(100.0, 30.0), 5.0, &[]).unwrap();
        assert_eq!(on_line, ShapeId(2));
        let alone = VectorArt::new(vec![surface]);
        let (same, _, _) = split_piece(&alone, Point::new(50.0, 0.0), 5.0, &[]).unwrap();
        assert_eq!(same.shapes(), alone.shapes());
        assert!(split_piece(&art, Point::new(50.0, 0.0), 5.0, &[SwatchId(1)]).is_none());
    }

    #[test]
    fn a_line_is_split_at_the_crossings_either_side_of_a_point_on_it() {
        // A line across from 0 to 300, crossed at 100 and 200.
        let shapes = vec![
            VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind: stroke((0.0, 0.0), (300.0, 0.0), 4.0) },
            VectorShape { id: ShapeId(2), swatch: SwatchId(1), kind: stroke((100.0, -50.0), (100.0, 50.0), 4.0) },
            VectorShape { id: ShapeId(3), swatch: SwatchId(1), kind: stroke((200.0, -50.0), (200.0, 50.0), 4.0) },
        ];
        let art = VectorArt::new(shapes);
        // Between the crossings: the middle is a stroke of its own, keeping the line's id.
        let (split, id, middle) = split_piece(&art, Point::new(150.0, 1.0), 5.0, &[]).unwrap();
        assert_eq!((split.shapes().len(), id), (5, ShapeId(1)));
        let bounds = middle.bounding_box();
        assert!((bounds.x0 - 100.0).abs() < 1.5 && (bounds.x1 - 200.0).abs() < 1.5, "{bounds:?}");
        // Past the last crossing: from it to the end.
        let (_, _, end) = split_piece(&art, Point::new(250.0, 0.0), 5.0, &[]).unwrap();
        assert!((end.bounding_box().x0 - 200.0).abs() < 1.5 && (end.bounding_box().x1 - 300.0).abs() < 1.5);
        // A line that crosses none is its piece whole; nowhere near one, or protected, none.
        let alone = art_of(stroke((0.0, 0.0), (300.0, 0.0), 4.0));
        let (same, _, whole) = split_piece(&alone, Point::new(150.0, 0.0), 5.0, &[]).unwrap();
        assert!(same == alone && (whole.bounding_box().width() - 300.0).abs() < 1e-6);
        assert!(split_piece(&art, Point::new(150.0, 40.0), 5.0, &[]).is_none());
        assert!(split_piece(&art, Point::new(150.0, 0.0), 5.0, &[SwatchId(1)]).is_none());
    }

    #[test]
    fn a_stroke_flattened_is_one_shape_with_those_of_its_swatch_it_overlaps() {
        let shapes = vec![
            VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind: stroke((0.0, 0.0), (100.0, 0.0), 10.0) },
            VectorShape { id: ShapeId(2), swatch: SwatchId(2), kind: stroke((0.0, 30.0), (100.0, 30.0), 10.0) },
            VectorShape { id: ShapeId(3), swatch: SwatchId(1), kind: stroke((0.0, 200.0), (100.0, 200.0), 10.0) },
        ];
        let art = VectorArt::new(shapes);
        // Down through the first two: merged with the first, of its swatch, and not the other.
        let new = VectorShape { id: ShapeId(4), swatch: SwatchId(1), kind: stroke((50.0, -20.0), (50.0, 50.0), 10.0) };
        let flat = flattened(&art, &new).unwrap();
        let kinds: Vec<(u32, bool)> = flat.shapes().iter().map(|shape| (shape.id.0, matches!(shape.kind, ShapeKind::Fill(_)))).collect();
        assert_eq!(kinds, vec![(2, false), (3, false), (4, true)]);
        let raster = flat.raster();
        assert_eq!((raster.pixel(10, 0).swatch, raster.pixel(50, 45).swatch, raster.pixel(50, 30).swatch), (SwatchId(1), SwatchId(1), SwatchId(1)));
        // Overlapping none of its swatch, it is laid as it is.
        let apart = VectorShape { id: ShapeId(4), swatch: SwatchId(1), kind: stroke((300.0, 0.0), (400.0, 0.0), 10.0) };
        assert!(flattened(&art, &apart).is_none());
    }

    #[test]
    fn what_is_selected_is_merged_swatch_by_swatch_where_its_topmost_was() {
        let shapes = vec![
            VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind: stroke((0.0, 0.0), (100.0, 0.0), 10.0) },
            VectorShape { id: ShapeId(2), swatch: SwatchId(2), kind: stroke((0.0, 30.0), (100.0, 30.0), 10.0) },
            VectorShape { id: ShapeId(3), swatch: SwatchId(1), kind: stroke((0.0, 60.0), (100.0, 60.0), 10.0) },
            VectorShape { id: ShapeId(4), swatch: SwatchId(2), kind: stroke((0.0, 90.0), (100.0, 90.0), 10.0) },
            VectorShape { id: ShapeId(5), swatch: SwatchId(1), kind: stroke((0.0, 300.0), (100.0, 300.0), 10.0) },
        ];
        let art = VectorArt::new(shapes);
        // The first four selected: the two of each swatch one fill each, where the upper of
        // them was, apart or not; the one not selected stays a stroke.
        let merged = merge_picked(&art, &crate::Picked::all(crate::Rect::new(-10, -10, 110, 100))).unwrap();
        let kinds: Vec<(u32, u16, bool)> = merged.shapes().iter().map(|shape| (shape.id.0, shape.swatch.0, matches!(shape.kind, ShapeKind::Fill(_)))).collect();
        assert_eq!(kinds, vec![(3, 1, true), (4, 2, true), (5, 1, false)]);
        let raster = merged.raster();
        assert_eq!((raster.pixel(50, 0).swatch, raster.pixel(50, 60).swatch, raster.pixel(50, 30).swatch), (SwatchId(1), SwatchId(1), SwatchId(2)));
        // One of each swatch taken: nothing to merge.
        assert!(merge_picked(&art, &crate::Picked::all(crate::Rect::new(-10, -10, 110, 40))).is_none());
    }

    #[test]
    fn a_cut_along_a_box_splits_a_fill_too() {
        let art = art_of(ShapeKind::Fill(VectorFill::plain(kurbo::Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1))));
        let box_ = kurbo::Rect::new(50.0, -10.0, 150.0, 110.0).to_path(0.1);
        let raster = |art: &VectorArt, x, y| art.raster().pixel(x, y).coverage;
        let cut = cut_vector(&art, &box_, &[], false, Cap::Round).unwrap();
        assert_eq!((cut.shapes().len(), raster(&cut, 20, 50), raster(&cut, 80, 50)), (1, u16::MAX, 0));
        let kept = cut_vector(&art, &box_, &[], true, Cap::Round).unwrap();
        assert_eq!((kept.shapes().len(), raster(&kept, 20, 50), raster(&kept, 80, 50)), (2, u16::MAX, u16::MAX));
    }

    #[test]
    fn a_cut_at_a_slant_leaves_strokes_too() {
        let art = art_of(stroke((0.0, 0.0), (200.0, 0.0), 6.0));
        let slant = VectorStroke::even(line((70.0, -50.0), (130.0, 50.0)), 16.0);
        let erased = erase_vector(&art, &slant, &[]).unwrap();
        assert_eq!(erased.shapes().len(), 2);
        assert!(erased.shapes().iter().all(|shape| matches!(shape.kind, ShapeKind::Stroke(_))));
    }

    #[test]
    fn a_bite_out_of_a_long_wavy_stroke_takes_little_time() {
        let mut wave = BezPath::new();
        wave.move_to((0.0, 0.0));
        for i in 0..8 {
            let x = f64::from(i) * 100.0;
            wave.curve_to((x + 30.0, -60.0), (x + 70.0, 60.0), (x + 100.0, 0.0));
        }
        let art = art_of(ShapeKind::Stroke(VectorStroke::even(wave, 20.0)));
        let start = std::time::Instant::now();
        let erased = erase_vector(&art, &VectorStroke::even(line((380.0, -40.0), (420.0, -40.0)), 30.0), &[]);
        let took = start.elapsed();
        assert!(erased.is_some_and(|art| matches!(art.shapes()[0].kind, ShapeKind::Fill(_))));
        assert!(took.as_millis() < 500, "{took:?}");
    }

    #[test]
    fn a_bite_out_of_an_edge_leaves_the_outline_of_the_rest() {
        let art = art_of(stroke((0.0, 0.0), (200.0, 0.0), 40.0));
        // A small eraser along the top edge, not reaching the middle.
        let erased = erase_vector(&art, &VectorStroke::even(line((80.0, -24.0), (120.0, -24.0)), 16.0), &[]).unwrap();
        assert_eq!(erased.shapes().len(), 1);
        assert!(matches!(erased.shapes()[0].kind, ShapeKind::Fill(_)), "an outline now");
        let raster = erased.raster();
        assert_eq!((raster.pixel(100, -18).coverage, raster.pixel(100, 5).coverage), (0, u16::MAX), "bitten at the edge, whole below");
    }

    #[test]
    fn what_the_eraser_misses_or_covers_whole_and_protected_swatches_stay_or_go_whole() {
        let art = art_of(stroke((0.0, 0.0), (50.0, 0.0), 4.0));
        assert!(erase_vector(&art, &VectorStroke::even(line((0.0, 50.0), (50.0, 50.0)), 10.0), &[]).is_none(), "missed");
        let gone = erase_vector(&art, &VectorStroke::even(line((-20.0, 0.0), (70.0, 0.0)), 30.0), &[]).unwrap();
        assert!(gone.is_empty(), "covered whole");
        assert!(erase_vector(&art, &VectorStroke::even(line((-20.0, 0.0), (70.0, 0.0)), 30.0), &[SwatchId(1)]).is_none(), "protected");
    }

    #[test]
    fn a_fill_loses_what_the_eraser_covers() {
        let mut square = BezPath::new();
        square.move_to((0.0, 0.0));
        square.line_to((100.0, 0.0));
        square.line_to((100.0, 100.0));
        square.line_to((0.0, 100.0));
        square.close_path();
        let art = art_of(ShapeKind::Fill(VectorFill::plain(square)));
        let erased = erase_vector(&art, &VectorStroke::even(line((50.0, -10.0), (50.0, 110.0)), 20.0), &[]).unwrap();
        let raster = erased.raster();
        assert_eq!((raster.pixel(50, 50).coverage, raster.pixel(20, 50).coverage, raster.pixel(80, 50).coverage), (0, u16::MAX, u16::MAX));
    }
}
