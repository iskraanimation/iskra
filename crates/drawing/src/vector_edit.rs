//! Editing the points of vector art one by one (plan §4.16, step 8): what the point editor does
//! to a shape's path, as functions of the path.
//!
//! A point is named by the index of the element of the path that ends at it: the `MoveTo` that
//! starts a subpath, or a line or a curve to it. A stroke's widths have one for each such
//! element, in the same order, so the index names its width too. A curve's handles belong to
//! the points at its ends: its first to the point it leaves (that point's *out* handle), its
//! second to the point it reaches (that point's *in* handle). Quadratic curves are made cubic
//! before they are edited.

use kurbo::{BezPath, CubicBez, ParamCurve, PathEl, PathSeg, Point, Vec2};

/// Which of a point's two handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Handle {
    /// The handle of the curve that reaches the point.
    In,
    /// The handle of the curve that leaves it.
    Out,
}

/// The point an element ends at, if it ends at one.
fn end_of(element: &PathEl) -> Option<Point> {
    match *element {
        PathEl::MoveTo(at) | PathEl::LineTo(at) | PathEl::QuadTo(_, at) | PathEl::CurveTo(_, _, at) => Some(at),
        PathEl::ClosePath => None,
    }
}

/// `path` with its quadratic curves made cubic, which is the same line.
pub fn cubic(path: &BezPath) -> BezPath {
    let mut last = Point::ZERO;
    let mut out = BezPath::new();
    for element in path.elements() {
        match *element {
            PathEl::QuadTo(c, to) => {
                let curve = kurbo::QuadBez::new(last, c, to).raise();
                out.curve_to(curve.p1, curve.p2, curve.p3);
            }
            other => out.push(other),
        }
        last = end_of(element).unwrap_or(last);
    }
    out
}

/// The path's points: for each element, where it ends, or `None` for one that closes a subpath.
pub fn points(path: &BezPath) -> Vec<Option<Point>> {
    path.elements().iter().map(end_of).collect()
}

/// Where the point `index` starts from: the end of the element before it, if it is not the start of a subpath.
fn before(elements: &[PathEl], index: usize) -> Option<Point> {
    match elements.get(index)? {
        PathEl::MoveTo(_) | PathEl::ClosePath => None,
        _ => elements[..index].iter().rev().find_map(end_of),
    }
}

/// The point's handles: its in handle and its out handle, where the curves on either side have them.
pub fn handles(path: &BezPath, index: usize) -> (Option<Point>, Option<Point>) {
    let elements = path.elements();
    let into = match elements.get(index) {
        Some(PathEl::CurveTo(_, c2, _)) => Some(*c2),
        _ => None,
    };
    let out = match elements.get(index + 1) {
        Some(PathEl::CurveTo(c1, _, _)) => Some(*c1),
        _ => None,
    };
    (into, out)
}

/// Whether the point's two handles lie on one line through it, either way: a smooth point.
pub fn is_smooth(path: &BezPath, index: usize) -> bool {
    let Some(Some(at)) = points(path).get(index).copied() else { return false };
    match handles(path, index) {
        (Some(into), Some(out)) => {
            let (a, b) = (at - into, out - at);
            a.hypot() > 1e-6 && b.hypot() > 1e-6 && a.normalize().dot(b.normalize()) > 0.9994
        }
        _ => false,
    }
}

/// `path` with the points `which` moved by `by`, each with its handles.
pub fn moved_points(path: &BezPath, which: &[usize], by: Vec2) -> BezPath {
    let mut elements = path.elements().to_vec();
    for &index in which {
        match elements.get_mut(index) {
            Some(PathEl::MoveTo(at) | PathEl::LineTo(at)) => *at += by,
            Some(PathEl::QuadTo(c, at)) => (*c, *at) = (*c + by, *at + by),
            Some(PathEl::CurveTo(_, c2, at)) => (*c2, *at) = (*c2 + by, *at + by),
            _ => continue,
        }
        if let Some(PathEl::CurveTo(c1, ..)) = elements.get_mut(index + 1) {
            *c1 += by;
        }
    }
    BezPath::from_vec(elements)
}

/// `path` with the point's `handle` dragged to `to`. While `smooth`, its other handle turns
/// with it to stay on one line through the point, keeping its length. `None` if the point has
/// no such handle.
pub fn moved_handle(path: &BezPath, index: usize, handle: Handle, to: Point, smooth: bool) -> Option<BezPath> {
    let at = points(path).get(index).copied().flatten()?;
    let mut elements = path.elements().to_vec();
    let (this, twin) = match handle {
        Handle::In => (index, index + 1),
        Handle::Out => (index + 1, index),
    };
    // The twin is the other handle: the first point of the curve after, or the second of the one before.
    let other = match handle {
        Handle::In => Handle::Out,
        Handle::Out => Handle::In,
    };
    match (elements.get_mut(this)?, handle) {
        (PathEl::CurveTo(_, c2, _), Handle::In) => *c2 = to,
        (PathEl::CurveTo(c1, _, _), Handle::Out) => *c1 = to,
        _ => return None,
    }
    if smooth && (to - at).hypot() > 1e-9 {
        let away = (at - to).normalize();
        match (elements.get_mut(twin), other) {
            (Some(PathEl::CurveTo(_, c2, _)), Handle::In) => *c2 = at + away * (*c2 - at).hypot(),
            (Some(PathEl::CurveTo(c1, _, _)), Handle::Out) => *c1 = at + away * (*c1 - at).hypot(),
            _ => {}
        }
    }
    Some(BezPath::from_vec(elements))
}

/// The point after `index` (the end of the next element that has one), if the subpath goes on.
fn after(elements: &[PathEl], index: usize) -> Option<Point> {
    match elements.get(index + 1)? {
        PathEl::MoveTo(_) | PathEl::ClosePath => None,
        element => end_of(element),
    }
}

/// `path` with the point made smooth, its handles on one line through it, along the way from
/// the point before it to the point after it and a third of the way to each; or made a corner,
/// its handles drawn in onto it. Lines on either side become curves to have the handles.
pub fn smoothed(path: &BezPath, index: usize, smooth: bool) -> Option<BezPath> {
    let path = cubic(path);
    let mut elements = path.elements().to_vec();
    let at = elements.get(index).and_then(end_of)?;
    let (from, to) = (before(&elements, index), after(&elements, index));
    let way = match (from, to) {
        (Some(from), Some(to)) => to - from,
        (Some(from), None) => at - from,
        (None, Some(to)) => to - at,
        (None, None) => return None,
    };
    let way = if way.hypot() > 1e-9 { way.normalize() } else { Vec2::new(1.0, 0.0) };
    if let Some(from) = from {
        let handle = if smooth { at - way * ((at - from).hypot() / 3.0) } else { at };
        elements[index] = match elements[index] {
            PathEl::CurveTo(c1, _, _) => PathEl::CurveTo(c1, handle, at),
            _ => PathEl::CurveTo(from + (at - from) / 3.0, handle, at),
        };
    }
    if let Some(to) = to {
        let handle = if smooth { at + way * ((to - at).hypot() / 3.0) } else { at };
        elements[index + 1] = match elements[index + 1] {
            PathEl::CurveTo(_, c2, _) => PathEl::CurveTo(handle, c2, to),
            _ => PathEl::CurveTo(handle, to + (at - to) / 3.0, to),
        };
    }
    Some(BezPath::from_vec(elements))
}

/// `path` with a point put on its element `index` (a line or curve), `t` of the way along it,
/// the curve cut there so the line stays as it was; and `widths` with the width there, going
/// evenly between the two on either side. `None` if the element is no line or curve.
pub fn with_point(path: &BezPath, widths: &[f64], index: usize, t: f64) -> Option<(BezPath, Vec<f64>)> {
    let path = cubic(path);
    let elements = path.elements();
    let from = before(elements, index)?;
    let segment = match elements[index] {
        PathEl::LineTo(to) => PathSeg::Line(kurbo::Line::new(from, to)),
        PathEl::CurveTo(c1, c2, to) => PathSeg::Cubic(CubicBez::new(from, c1, c2, to)),
        _ => return None,
    };
    let t = t.clamp(0.0, 1.0);
    let piece = |range: std::ops::Range<f64>| match segment.subsegment(range) {
        PathSeg::Line(line) => PathEl::LineTo(line.p1),
        PathSeg::Cubic(cubic) => PathEl::CurveTo(cubic.p1, cubic.p2, cubic.p3),
        PathSeg::Quad(quad) => PathEl::QuadTo(quad.p1, quad.p2),
    };
    let mut out = elements.to_vec();
    out.splice(index..=index, [piece(0.0..t), piece(t..1.0)]);
    let mut widths = widths.to_vec();
    if index < widths.len() {
        let width = widths[index - 1] + (widths[index] - widths[index - 1]) * t;
        widths.insert(index, width);
    }
    Some((BezPath::from_vec(out), widths))
}

/// `path` without the point `index`, and `widths` without its width. A point between two others
/// joins the curves on either side into one, from the handle that leaves the point before it to
/// the one that reaches the point after; an end point takes its line or curve with it. `None`
/// where that would leave a subpath of fewer than two points.
pub fn without_point(path: &BezPath, widths: &[f64], index: usize) -> Option<(BezPath, Vec<f64>)> {
    let path = cubic(path);
    let mut elements = path.elements().to_vec();
    end_of(elements.get(index)?)?;
    // The points of the subpath the point is on.
    let start = elements[..=index].iter().rposition(|element| matches!(element, PathEl::MoveTo(_)))?;
    let end = elements[start + 1..].iter().position(|element| matches!(element, PathEl::MoveTo(_))).map_or(elements.len(), |at| start + 1 + at);
    let points = elements[start..end].iter().filter(|element| end_of(element).is_some()).count();
    if points <= 2 {
        return None;
    }
    let (from, to) = (before(&elements, index), after(&elements, index));
    match (from, to) {
        // The start: the next point starts the subpath.
        (None, Some(to)) => elements.splice(index..=index + 1, [PathEl::MoveTo(to)]).for_each(drop),
        // The end: its line or curve goes.
        (Some(_), None) => _ = elements.remove(index),
        (Some(from), Some(to)) => {
            let leave = match elements[index] {
                PathEl::CurveTo(c1, ..) => Some(c1),
                _ => None,
            };
            let reach = match elements[index + 1] {
                PathEl::CurveTo(_, c2, _) => Some(c2),
                _ => None,
            };
            let joined = match (leave, reach) {
                (None, None) => PathEl::LineTo(to),
                (leave, reach) => PathEl::CurveTo(leave.unwrap_or(from), reach.unwrap_or(to), to),
            };
            elements.splice(index..=index + 1, [joined]).for_each(drop);
        }
        (None, None) => return None,
    }
    let mut widths = widths.to_vec();
    if index < widths.len() {
        widths.remove(index);
    }
    Some((BezPath::from_vec(elements), widths))
}

/// Where on `path` is nearest to `at`: the element (a line or a curve) and how far along it, and
/// how far away that is.
pub fn nearest(path: &BezPath, at: Point) -> Option<(usize, f64, f64)> {
    use kurbo::ParamCurveNearest;
    let path = cubic(path);
    let elements = path.elements();
    let mut best: Option<(usize, f64, f64)> = None;
    for index in 0..elements.len() {
        let Some(from) = before(elements, index) else { continue };
        let segment = match elements[index] {
            PathEl::LineTo(to) => PathSeg::Line(kurbo::Line::new(from, to)),
            PathEl::CurveTo(c1, c2, to) => PathSeg::Cubic(CubicBez::new(from, c1, c2, to)),
            _ => continue,
        };
        let found = segment.nearest(at, 1e-6);
        let distance = found.distance_sq.sqrt();
        if best.is_none_or(|(.., nearest)| distance < nearest) {
            best = Some((index, found.t, distance));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stroke's line: a line, then a curve, then a line.
    fn path() -> BezPath {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((100.0, 0.0));
        path.curve_to((130.0, 0.0), (150.0, 30.0), (150.0, 60.0));
        path.line_to((150.0, 120.0));
        path
    }

    #[test]
    fn a_point_moves_with_its_handles() {
        let moved = moved_points(&path(), &[2], Vec2::new(10.0, 5.0));
        assert_eq!(points(&moved)[2], Some(Point::new(160.0, 65.0)));
        assert_eq!(handles(&moved, 2), (Some(Point::new(160.0, 35.0)), None), "its in handle with it");
        assert_eq!(points(&moved)[3], Some(Point::new(150.0, 120.0)), "the others stay");
        let both = moved_points(&path(), &[0, 1], Vec2::new(0.0, -10.0));
        assert_eq!(handles(&both, 1).1, Some(Point::new(130.0, -10.0)), "the out handle of a point moved");
    }

    #[test]
    fn a_handle_of_a_smooth_point_turns_its_twin_and_keeps_its_length() {
        let smooth = smoothed(&path(), 2, true).unwrap();
        assert!(is_smooth(&smooth, 2) && !is_smooth(&path(), 2));
        let (_, out) = handles(&smooth, 2);
        let length = (out.unwrap() - Point::new(150.0, 60.0)).hypot();
        let turned = moved_handle(&smooth, 2, Handle::In, Point::new(120.0, 60.0), true).unwrap();
        let (into, out) = handles(&turned, 2);
        assert_eq!(into, Some(Point::new(120.0, 60.0)));
        let out = out.unwrap();
        assert!((out.x - (150.0 + length)).abs() < 1e-9 && (out.y - 60.0).abs() < 1e-9, "the twin straight across, as long as it was: {out:?}");
        assert!(is_smooth(&turned, 2));
        // Not smooth, the twin stays where it was.
        let alone = moved_handle(&smooth, 2, Handle::In, Point::new(120.0, 60.0), false).unwrap();
        assert_eq!(handles(&alone, 2).1, handles(&smooth, 2).1);
        assert!(moved_handle(&path(), 0, Handle::In, Point::ZERO, false).is_none(), "the start has no in handle");
    }

    #[test]
    fn a_corner_made_smooth_bends_its_lines_and_back_draws_the_handles_in() {
        let smooth = smoothed(&path(), 1, true).unwrap();
        assert!(matches!(smooth.elements()[1], PathEl::CurveTo(..)), "the line before became a curve");
        assert!(is_smooth(&smooth, 1));
        let corner = smoothed(&smooth, 1, false).unwrap();
        assert_eq!(handles(&corner, 1), (Some(Point::new(100.0, 0.0)), Some(Point::new(100.0, 0.0))));
        assert!(!is_smooth(&corner, 1));
    }

    #[test]
    fn a_point_is_put_on_a_curve_without_changing_it_and_taken_away_again() {
        let widths = [2.0, 4.0, 6.0, 8.0];
        let (more, wider) = with_point(&path(), &widths, 2, 0.5).unwrap();
        assert_eq!((more.elements().len(), wider.clone()), (5, vec![2.0, 4.0, 5.0, 6.0, 8.0]));
        let curve = CubicBez::new((100.0, 0.0), (130.0, 0.0), (150.0, 30.0), (150.0, 60.0));
        let put = points(&more)[2].unwrap();
        assert!((put - curve.eval(0.5)).hypot() < 1e-9, "on the curve as it was");
        let (back, widths_back) = without_point(&more, &wider, 2).unwrap();
        assert_eq!((back.elements().len(), widths_back), (4, widths.to_vec()));
        // The ends go with their lines; two points are the least a line has.
        let (no_start, _) = without_point(&path(), &widths, 0).unwrap();
        assert_eq!(no_start.elements()[0], PathEl::MoveTo(Point::new(100.0, 0.0)));
        let (no_end, _) = without_point(&path(), &widths, 3).unwrap();
        assert_eq!(points(&no_end).last(), Some(&Some(Point::new(150.0, 60.0))));
        let mut short = BezPath::new();
        short.move_to((0.0, 0.0));
        short.line_to((10.0, 0.0));
        assert!(without_point(&short, &[1.0, 1.0], 1).is_none());
    }

    #[test]
    fn the_nearest_place_on_the_line_is_found() {
        let (index, t, distance) = nearest(&path(), Point::new(50.0, 3.0)).unwrap();
        assert_eq!(index, 1);
        assert!((t - 0.5).abs() < 1e-9 && (distance - 3.0).abs() < 1e-9);
    }
}
