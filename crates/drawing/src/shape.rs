//! Shapes drawn with a brush: a line, a rectangle, an ellipse, a polyline, and Moho's others (the
//! user's of 2026-11-23): a triangle, a star, an arrow, a spiral and a grid. A shape is a stroke
//! that goes along it at full pressure, so it is drawn with all the brush has, as a line by hand
//! is. A shape that is filled has its inside painted too, under that stroke, and one with
//! `no_line` its inside alone. On vector art its lines are vector strokes along the shape's own
//! curves ([`Shape::vector_lines`]) and its inside a fill ([`Shape::vector_inside`]).

use photocraft_paint::{BrushSettings, Stroke, StrokePoint};
use serde::{Deserialize, Serialize};

use crate::{Point, Rect};

/// How far apart the points of a shape's stroke are, in pixels.
const STEP: f64 = 4.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Shape {
    /// From one point to the other.
    #[default]
    Line,
    /// With the two points at opposite corners.
    Rectangle,
    /// Inside the rectangle with the two points at opposite corners.
    Ellipse,
    /// Straight from each of several points to the next.
    Polyline,
    /// Inside the rectangle with the two points at opposite corners, its point at the top
    /// middle.
    Triangle,
    /// Inside the rectangle with the two points at opposite corners, a point at the top.
    Star,
    /// From its tail to its head, with a shaft and a head.
    Arrow,
    /// Out from its middle, the first point, winding round to the second: more turns the further
    /// it goes.
    Spiral,
    /// Inside the rectangle with the two points at opposite corners, cut into rows and columns.
    Grid,
}

/// How a shape is drawn between its two points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Form {
    /// A line or an arrow at a step of 45°, a square, a circle, an even triangle or star.
    pub even: bool,
    /// The first point is the shape's middle, not its end or its corner: the shape is drawn
    /// out from it, as far the other way as towards the second point.
    pub centred: bool,
    /// A rectangle's corners are rounded with this radius, in pixels, as far as its sides
    /// allow.
    pub round: f64,
    /// The inside of a rectangle, an ellipse or a polyline is painted too.
    pub filled: bool,
    /// A polyline goes back from its last point to its first. One that is filled does anyway.
    pub closed: bool,
    /// Its inside alone, with no line round it (Moho's *Auto-stroke* off): a shape that is not
    /// filled is then nothing.
    pub no_line: bool,
    /// How many points a star has.
    pub tips: u32,
    /// How far apart a spiral's turns are, in pixels.
    pub spacing: f64,
    /// A spiral winds the other way: against the clock, where it goes with it.
    pub counter: bool,
    /// How many rows and columns a grid is cut into.
    pub rows: u32,
    pub columns: u32,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            even: false,
            centred: false,
            round: 0.0,
            filled: false,
            closed: false,
            no_line: false,
            tips: 5,
            spacing: 24.0,
            counter: false,
            rows: 4,
            columns: 4,
        }
    }
}

/// How far in a star's inner corners are, as a part of its outer radius: a five-pointed star's
/// sides then lie on straight lines from point to point.
const STAR_INNER: f64 = 0.382;

/// How many rows a pixel is cut into where a shape's inside is worked out.
const ROWS: i32 = 4;

impl Shape {
    pub const ALL: [Shape; 9] =
        [Shape::Line, Shape::Rectangle, Shape::Ellipse, Shape::Polyline, Shape::Triangle, Shape::Star, Shape::Arrow, Shape::Spiral, Shape::Grid];

    pub fn title(self) -> &'static str {
        match self {
            Shape::Line => "Line",
            Shape::Rectangle => "Rectangle",
            Shape::Ellipse => "Ellipse",
            Shape::Polyline => "Polyline",
            Shape::Triangle => "Triangle",
            Shape::Star => "Star",
            Shape::Arrow => "Arrow",
            Shape::Spiral => "Spiral",
            Shape::Grid => "Grid",
        }
    }

    /// Whether it has an inside to fill: a line, a spiral and a grid have none.
    pub fn has_inside(self) -> bool {
        !matches!(self, Shape::Line | Shape::Spiral | Shape::Grid)
    }

    /// Whether it is drawn with its line: always, but for a filled shape with an inside and
    /// `no_line`, which is its inside alone. A shape with nothing inside it keeps its line.
    pub fn lined(self, form: Form) -> bool {
        !(form.no_line && form.filled && self.has_inside())
    }

    /// Where `to` is taken to be when the shape is to be `even`: a line at a step of 45°, a
    /// square, a circle.
    fn evened(self, from: Point, to: Point) -> Point {
        let (dx, dy) = (to.x - from.x, to.y - from.y);
        match self {
            Shape::Line | Shape::Polyline | Shape::Arrow | Shape::Spiral => {
                let step = std::f64::consts::FRAC_PI_4;
                let (angle, length) = ((dy.atan2(dx) / step).round() * step, dx.hypot(dy));
                Point::new(from.x + length * angle.cos(), from.y + length * angle.sin())
            }
            Shape::Rectangle | Shape::Ellipse | Shape::Star | Shape::Grid => {
                let side = dx.abs().max(dy.abs());
                Point::new(from.x + side.copysign(dx), from.y + side.copysign(dy))
            }
            // As high as an even triangle as wide as the drag.
            Shape::Triangle => Point::new(from.x + dx, from.y + (dx.abs() * 3f64.sqrt() / 2.0).copysign(dy)),
        }
    }

    /// The two points the shape is drawn between, once `form` has had its say: evened, and
    /// with the first moved as far back from the middle as the second is from it.
    fn ends(self, from: Point, to: Point, form: Form) -> (Point, Point) {
        let to = if form.even { self.evened(from, to) } else { to };
        // A spiral's first point is its middle anyway.
        let from = if form.centred && self != Shape::Spiral { Point::new(2.0 * from.x - to.x, 2.0 * from.y - to.y) } else { from };
        (from, to)
    }

    /// The corners of a shape of straight sides between `from` and `to`, once [`Shape::ends`]
    /// has had its say, in order round it, the first not given again at the end: a rectangle
    /// with sharp corners, a triangle, a star and an arrow. `None` for the others.
    fn corners(self, from: Point, to: Point, form: Form) -> Option<Vec<Point>> {
        let (x0, x1, y0, y1) = (from.x.min(to.x), from.x.max(to.x), from.y.min(to.y), from.y.max(to.y));
        let round = form.round.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
        Some(match self {
            Shape::Rectangle if round < 0.5 => vec![from, Point::new(to.x, from.y), to, Point::new(from.x, to.y)],
            Shape::Triangle => vec![Point::new((x0 + x1) / 2.0, y0), Point::new(x1, y1), Point::new(x0, y1)],
            Shape::Star => {
                let tips = form.tips.clamp(3, 64);
                let (cx, cy, rx, ry) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0);
                (0..tips * 2)
                    .map(|corner| {
                        let angle = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * f64::from(corner) / f64::from(tips);
                        let reach = if corner % 2 == 0 { 1.0 } else { STAR_INNER };
                        Point::new(cx + rx * reach * angle.cos(), cy + ry * reach * angle.sin())
                    })
                    .collect()
            }
            Shape::Arrow => {
                let (dx, dy) = (to.x - from.x, to.y - from.y);
                let length = dx.hypot(dy);
                if length <= 1e-9 {
                    return Some(vec![from]);
                }
                let (along, across) = ((dx / length, dy / length), (-dy / length, dx / length));
                let at = |forward: f64, side: f64| Point::new(from.x + along.0 * forward + across.0 * side, from.y + along.1 * forward + across.1 * side);
                // A shaft a seventh as wide as it is long, a head three tenths of it.
                let (shaft, head, head_wide) = (length * 0.07, length * 0.3, length * 0.17);
                let neck = length - head;
                vec![at(0.0, shaft), at(neck, shaft), at(neck, head_wide), to, at(neck, -head_wide), at(neck, -shaft), at(0.0, -shaft)]
            }
            _ => return None,
        })
    }

    /// A spiral out from `from` to `to`: the angle and the distance of each of its points from
    /// the middle, `count` of them, ending at `to`.
    fn spiral(from: Point, to: Point, form: Form) -> Vec<Point> {
        let (dx, dy) = (to.x - from.x, to.y - from.y);
        let reach = dx.hypot(dy);
        let turns = (reach / form.spacing.max(1.0)).max(0.25);
        let sweep = turns * std::f64::consts::TAU;
        let (end, way) = (dy.atan2(dx), if form.counter { -1.0 } else { 1.0 });
        // About a step apart along it: its length is about half the sweep times the reach.
        let count = ((sweep * reach / 2.0 / STEP).ceil() as usize).clamp(16, 20_000);
        (0..=count)
            .map(|i| {
                let t = i as f64 / count as f64;
                let angle = end - way * sweep * (1.0 - t);
                Point::new(from.x + reach * t * angle.cos(), from.y + reach * t * angle.sin())
            })
            .collect()
    }

    /// A grid's lines between `from` and `to`: its outside, closed, then the lines across and the
    /// lines down inside it.
    fn grid(from: Point, to: Point, form: Form) -> Vec<Vec<Point>> {
        let (x0, x1, y0, y1) = (from.x.min(to.x), from.x.max(to.x), from.y.min(to.y), from.y.max(to.y));
        let (rows, columns) = (form.rows.clamp(1, 200), form.columns.clamp(1, 200));
        let mut lines = vec![vec![Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1), Point::new(x0, y0)]];
        lines.extend((1..rows).map(|row| {
            let y = y0 + (y1 - y0) * f64::from(row) / f64::from(rows);
            vec![Point::new(x0, y), Point::new(x1, y)]
        }));
        lines.extend((1..columns).map(|column| {
            let x = x0 + (x1 - x0) * f64::from(column) / f64::from(columns);
            vec![Point::new(x, y0), Point::new(x, y1)]
        }));
        lines
    }

    /// The path of the shape between `from` and `to`, in the drawing's pixels: its points a
    /// step apart, and for a closed shape back at its start. A grid is one path along all its
    /// lines, going from one to the next along its outside, which is drawn anyway.
    pub fn outline(self, from: Point, to: Point, form: Form) -> Vec<Point> {
        let (from, to) = self.ends(from, to, form);
        if let Some(corners) = self.corners(from, to, form) {
            return along(&corners.iter().chain(corners.first()).copied().collect::<Vec<_>>());
        }
        if self == Shape::Spiral {
            return Shape::spiral(from, to, form);
        }
        if self == Shape::Grid {
            let lines = Shape::grid(from, to, form);
            let mut path: Vec<Point> = Vec::new();
            for line in lines {
                // From where the last ended to this one's nearer end, along the outside.
                let line = match path.last() {
                    Some(last) if apart(*last, line[line.len() - 1]) < apart(*last, line[0]) => line.into_iter().rev().collect(),
                    _ => line,
                };
                path.extend(line);
            }
            return along(&path);
        }
        // Around the middle `(cx, cy)` with the radii `(rx, ry)`, from `start` to `end`.
        let arc = |cx: f64, cy: f64, rx: f64, ry: f64, start: f64, end: f64| {
            // Enough points that the way between two is a step along the larger radius.
            let count = (((end - start).abs() * rx.max(ry) / STEP).ceil() as usize).clamp(4, 4096);
            (0..=count).map(move |i| start + (end - start) * i as f64 / count as f64).map(move |angle| Point::new(cx + rx * angle.cos(), cy + ry * angle.sin()))
        };
        let (x0, x1, y0, y1) = (from.x.min(to.x), from.x.max(to.x), from.y.min(to.y), from.y.max(to.y));
        let round = form.round.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
        match self {
            Shape::Line | Shape::Polyline => along(&[from, to]),
            Shape::Rectangle if round >= 0.5 => {
                // Each corner a quarter turn, from the top right around by the bottom.
                let quarter = std::f64::consts::FRAC_PI_2;
                let corners = [(x1 - round, y0 + round, -1.0), (x1 - round, y1 - round, 0.0), (x0 + round, y1 - round, 1.0), (x0 + round, y0 + round, 2.0)];
                let mut turns: Vec<Point> =
                    corners.into_iter().flat_map(|(cx, cy, turn)| arc(cx, cy, round, round, turn * quarter, (turn + 1.0) * quarter)).collect();
                turns.push(turns[0]);
                along(&turns)
            }
            Shape::Ellipse => arc((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0, 0.0, std::f64::consts::TAU).collect(),
            // Straight sides, a spiral and a grid are made above.
            _ => along(&[from, to]),
        }
    }

    /// The lines of the shape between `from` and `to` as vector paths, in the drawing's
    /// pixels: its own corners and curves, not points a step apart. A closed shape ends where it
    /// began; a grid is its outside and each line inside, apart.
    pub fn vector_lines(self, from: Point, to: Point, form: Form) -> Vec<kurbo::BezPath> {
        let (from, to) = self.ends(from, to, form);
        let open = |points: &[Point]| {
            let mut path = kurbo::BezPath::new();
            for (i, at) in points.iter().enumerate() {
                let at = kurbo::Point::new(at.x, at.y);
                if i == 0 { path.move_to(at) } else { path.line_to(at) }
            }
            path
        };
        let (x0, x1, y0, y1) = (from.x.min(to.x), from.x.max(to.x), from.y.min(to.y), from.y.max(to.y));
        if let Some(corners) = self.corners(from, to, form) {
            return vec![open(&corners.iter().chain(corners.first()).copied().collect::<Vec<_>>())];
        }
        // Its own curves, with the close left out: the last one ends where it began.
        let curve = |path: kurbo::BezPath| kurbo::BezPath::from_vec(path.into_iter().filter(|element| !matches!(element, kurbo::PathEl::ClosePath)).collect());
        match self {
            Shape::Ellipse => vec![curve(kurbo::Shape::to_path(&kurbo::Ellipse::from_rect(kurbo::Rect::new(x0, y0, x1, y1)), 0.1))],
            Shape::Rectangle => {
                let round = form.round.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
                vec![curve(kurbo::Shape::to_path(&kurbo::RoundedRect::new(x0, y0, x1, y1, round), 0.1))]
            }
            Shape::Spiral => {
                // Fitted with few curves, as a line drawn by hand is.
                let options = vectorcraft_pathops::SimplifyOptions { tolerance: 0.25, corner_angle_deg: 70.0, straight_lines: false };
                let points = open(&Shape::spiral(from, to, form));
                vec![vectorcraft_pathops::simplify_with(&vectorcraft_geom::PathData::from_bezpath(&points), &options).to_bezpath()]
            }
            Shape::Grid => Shape::grid(from, to, form).iter().map(|line| open(line)).collect(),
            _ => vec![open(&[from, to])],
        }
    }

    /// [`Shape::vector_lines`] through `corners`: a polyline goes from each to the next, and back
    /// to the first if it is closed or filled; any other shape is drawn between the first and the
    /// last.
    pub fn vector_lines_through(self, corners: &[Point], form: Form) -> Vec<kurbo::BezPath> {
        let (Some(first), Some(last)) = (corners.first(), corners.last()) else { return Vec::new() };
        match self {
            Shape::Polyline => {
                let mut path = kurbo::BezPath::new();
                let closing = (form.closed || form.filled) && corners.len() >= 3;
                for (i, at) in corners.iter().chain(closing.then_some(first)).enumerate() {
                    let at = kurbo::Point::new(at.x, at.y);
                    if i == 0 { path.move_to(at) } else { path.line_to(at) }
                }
                vec![path]
            }
            _ => self.vector_lines(*first, *last, form),
        }
    }

    /// The inside of a shape through `corners` that is `filled`, as a closed vector path for a
    /// fill. `None` where it has none.
    pub fn vector_inside_through(self, corners: &[Point], form: Form) -> Option<kurbo::BezPath> {
        if !form.filled || !self.has_inside() || (self == Shape::Polyline && corners.len() < 3) {
            return None;
        }
        let mut outline = self.vector_lines_through(corners, form).into_iter().next()?;
        outline.close_path();
        Some(outline)
    }

    /// The path of the shape through `corners`, in the drawing's pixels: a polyline goes
    /// from each to the next, and back to the first if it is closed or filled; any other
    /// shape is drawn between the first and the last, as [`Shape::outline`] draws it.
    pub fn outline_through(self, corners: &[Point], form: Form) -> Vec<Point> {
        let (Some(first), Some(last)) = (corners.first(), corners.last()) else { return Vec::new() };
        match self {
            Shape::Polyline if form.closed || form.filled => along(&corners.iter().chain([first]).copied().collect::<Vec<_>>()),
            Shape::Polyline => along(corners),
            _ => self.outline(*first, *last, form),
        }
    }

    /// The points a stroke goes through to draw the shape between `from` and `to`, in the
    /// drawing's pixels.
    pub fn points(self, from: Point, to: Point, form: Form) -> Vec<StrokePoint> {
        stroke_points(self.outline(from, to, form))
    }

    /// The stroke that draws the shape through `corners` with `brush`, as [`Shape::stroke`]
    /// draws one between two points.
    pub fn stroke_through(self, brush: &BrushSettings, corners: &[Point], form: Form) -> Stroke {
        let mut brush = brush.clone();
        brush.smoothing.amount = 0.0;
        Stroke { brush, points: stroke_points(self.outline_through(corners, form)) }
    }

    /// The inside of a shape through `corners` that is `filled`, as [`Shape::inside`] gives
    /// it for one between two points. A polyline of fewer than three corners has none.
    pub fn inside_through(self, corners: &[Point], form: Form) -> Option<(Rect, Vec<f32>)> {
        if !form.filled || !self.has_inside() || (self == Shape::Polyline && corners.len() < 3) {
            return None;
        }
        inside_of(&self.outline_through(corners, form))
    }

    /// The stroke that draws the shape with `brush`. The line goes where the shape is: the
    /// brush's smoothing, which would round the corners off, is not used.
    pub fn stroke(self, brush: &BrushSettings, from: Point, to: Point, form: Form) -> Stroke {
        let mut brush = brush.clone();
        brush.smoothing.amount = 0.0;
        Stroke { brush, points: self.points(from, to, form) }
    }

    /// The inside of a shape that is `filled`: a rectangle of the drawing's pixels, and how
    /// much of each is inside the shape's path, from 0 to 1, in rows. `None` for a shape
    /// that is not filled, and for a line, which has no inside.
    pub fn inside(self, from: Point, to: Point, form: Form) -> Option<(Rect, Vec<f32>)> {
        if !form.filled || !self.has_inside() || self == Shape::Polyline {
            return None;
        }
        inside_of(&self.outline(from, to, form))
    }
}

/// A path as the points of a stroke along it, at full pressure.
fn stroke_points(path: Vec<Point>) -> Vec<StrokePoint> {
    // A point a millisecond: time only has to go on, for what a brush does with speed.
    path.into_iter().enumerate().map(|(i, at)| StrokePoint { x: at.x, y: at.y, pressure: 1.0, time: i as f64, ..Default::default() }).collect()
}

/// The pixels a closed `path` holds: a rectangle of them, and how much of each is inside,
/// from 0 to 1, in rows. `None` for a path that holds nothing, or far too much.
fn inside_of(path: &[Point]) -> Option<(Rect, Vec<f32>)> {
    let (x0, x1) = path.iter().fold((f64::MAX, f64::MIN), |(low, high), at| (low.min(at.x), high.max(at.x)));
    let (y0, y1) = path.iter().fold((f64::MAX, f64::MIN), |(low, high), at| (low.min(at.y), high.max(at.y)));
    let rect = Rect::new(x0.floor() as i32, y0.floor() as i32, x1.ceil() as i32, y1.ceil() as i32);
    // A drag across the whole of a very large drawing and more is no shape to fill.
    if rect.is_empty() || i64::from(rect.width()) * i64::from(rect.height()) > 1 << 28 {
        return None;
    }
    let width = rect.width() as usize;
    let mut coverage = vec![0.0f32; width * rect.height() as usize];
    let mut crossings: Vec<f64> = Vec::new();
    for row in 0..rect.height() as i32 * ROWS {
        // Where a level line through the middle of this part of a pixel crosses the path.
        let y = f64::from(rect.y0) + (f64::from(row) + 0.5) / f64::from(ROWS);
        crossings.clear();
        for side in path.windows(2) {
            let (a, b) = (side[0], side[1]);
            if (a.y <= y) != (b.y <= y) {
                crossings.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
            }
        }
        crossings.sort_by(f64::total_cmp);
        let pixels = &mut coverage[(row / ROWS) as usize * width..][..width];
        for [enters, leaves] in crossings.as_chunks::<2>().0 {
            // From one crossing to the next is inside: each pixel by how much of it is.
            let (left, right) = (enters - f64::from(rect.x0), leaves - f64::from(rect.x0));
            let (first, last) = (left.floor().max(0.0) as usize, (right.ceil() as usize).min(width));
            for (x, pixel) in pixels.iter_mut().enumerate().take(last).skip(first) {
                let part = (right.min(x as f64 + 1.0) - left.max(x as f64)).clamp(0.0, 1.0);
                *pixel += part as f32 / ROWS as f32;
            }
        }
    }
    Some((rect, coverage))
}

/// How far apart two points are.
fn apart(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// The corners, with points a step apart on the way from each to the next.
fn along(corners: &[Point]) -> Vec<Point> {
    let mut path = vec![corners[0]];
    for side in corners.windows(2) {
        let (dx, dy) = (side[1].x - side[0].x, side[1].y - side[0].y);
        let steps = (dx.hypot(dy) / STEP).ceil().max(1.0) as usize;
        path.extend((1..=steps).map(|i| Point::new(side[0].x + dx * i as f64 / steps as f64, side[0].y + dy * i as f64 / steps as f64)));
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IndexedSurface, Rect, SwatchId, paint_stroke};

    /// Drawn with a 6 pixel brush, whose soft edge reaches 5 pixels out from the line.
    fn drawn(shape: Shape, from: (f64, f64), to: (f64, f64), even: bool) -> IndexedSurface {
        let mut layer = IndexedSurface::new();
        let brush = BrushSettings { size: 6.0, pressure_size: false, ..Default::default() };
        let form = Form { even, ..Default::default() };
        paint_stroke(&mut layer, &shape.stroke(&brush, Point::new(from.0, from.1), Point::new(to.0, to.1), form), SwatchId(1));
        layer
    }

    fn painted(layer: &IndexedSurface, x: i32, y: i32) -> bool {
        layer.pixel(x, y).coverage > 32768
    }

    #[test]
    fn a_line_goes_from_one_point_to_the_other() {
        let line = drawn(Shape::Line, (100.0, 100.0), (300.0, 200.0), false);
        assert!(painted(&line, 100, 100) && painted(&line, 200, 150) && painted(&line, 300, 200));
        assert!(!painted(&line, 200, 120) && !painted(&line, 320, 210));
        // Evened, it lies at the nearest step of 45°, as long as it was.
        let level = drawn(Shape::Line, (100.0, 100.0), (300.0, 130.0), true);
        assert_eq!((level.bounds().y0, level.bounds().y1), (95, 105), "{:?}", level.bounds());
        assert!(painted(&level, 300, 100) && !painted(&level, 300, 130));
    }

    #[test]
    fn a_rectangle_is_its_four_sides_and_an_ellipse_lies_inside_it() {
        let rectangle = drawn(Shape::Rectangle, (100.0, 100.0), (300.0, 200.0), false);
        assert_eq!(rectangle.bounds(), Rect::new(95, 95, 305, 205));
        assert!([(100, 150), (300, 150), (200, 100), (200, 200), (100, 100), (300, 200)].into_iter().all(|(x, y)| painted(&rectangle, x, y)));
        assert!(!painted(&rectangle, 200, 150), "it is not filled");
        // Dragged up and to the left, it is the same rectangle.
        assert_eq!(drawn(Shape::Rectangle, (300.0, 200.0), (100.0, 100.0), false).bounds(), rectangle.bounds());
        assert_eq!(drawn(Shape::Rectangle, (100.0, 100.0), (300.0, 160.0), true).bounds(), Rect::new(95, 95, 305, 305), "a square, of the longer side");

        let ellipse = drawn(Shape::Ellipse, (100.0, 100.0), (300.0, 200.0), false);
        assert_eq!(ellipse.bounds(), Rect::new(95, 95, 305, 205));
        assert!([(100, 150), (300, 150), (200, 100), (200, 200)].into_iter().all(|(x, y)| painted(&ellipse, x, y)), "it touches the middle of each side");
        assert!(!painted(&ellipse, 100, 100) && !painted(&ellipse, 200, 150), "and neither the corners nor the middle");
        let circle = drawn(Shape::Ellipse, (100.0, 100.0), (160.0, 300.0), true);
        assert_eq!((circle.bounds().width(), circle.bounds().height()), (210, 210));
    }

    #[test]
    fn a_shape_is_drawn_out_from_its_middle_rounded_and_filled() {
        let (middle, corner) = (Point::new(200.0, 150.0), Point::new(300.0, 200.0));
        let brush = BrushSettings { size: 6.0, pressure_size: false, ..Default::default() };
        let draw = |shape: Shape, form: Form| {
            let mut layer = IndexedSurface::new();
            if let Some((rect, coverage)) = shape.inside(middle, corner, form) {
                layer.paint(rect, &coverage, SwatchId(2));
            }
            paint_stroke(&mut layer, &shape.stroke(&brush, middle, corner, form), SwatchId(1));
            layer
        };
        // From its middle, it is as far the other way: the same rectangle as from a corner.
        let centred = draw(Shape::Rectangle, Form { centred: true, ..Default::default() });
        assert_eq!(centred.bounds(), Rect::new(95, 95, 305, 205));
        assert!(painted(&centred, 100, 100) && !painted(&centred, 200, 150));
        let line = draw(Shape::Line, Form { centred: true, filled: true, ..Default::default() });
        assert!(painted(&line, 100, 100) && painted(&line, 300, 200), "a line through its middle; it has no inside to fill");
        assert_eq!(Shape::Line.inside(middle, corner, Form { filled: true, ..Default::default() }), None);

        // Rounded, the corners are cut and the sides are where they were.
        let rounded = draw(Shape::Rectangle, Form { centred: true, round: 30.0, ..Default::default() });
        assert_eq!(rounded.bounds(), Rect::new(95, 95, 305, 205));
        assert!(!painted(&rounded, 100, 100) && !painted(&rounded, 300, 200), "no corner");
        assert!(painted(&rounded, 109, 109) && painted(&rounded, 200, 100) && painted(&rounded, 100, 150), "a quarter circle from side to side");
        // A radius larger than the sides allow makes the short sides half circles.
        let pill = draw(Shape::Rectangle, Form { centred: true, round: 500.0, ..Default::default() });
        assert!(painted(&pill, 100, 150) && !painted(&pill, 100, 120) && painted(&pill, 200, 100));

        // Filled, the inside is painted with what the fill is given, under the line.
        let swatch = |layer: &IndexedSurface, x, y| (layer.pixel(x, y).swatch.0, layer.pixel(x, y).coverage > 65000);
        let filled = draw(Shape::Rectangle, Form { centred: true, filled: true, ..Default::default() });
        assert_eq!((swatch(&filled, 200, 150), swatch(&filled, 110, 110), swatch(&filled, 100, 150)), ((2, true), (2, true), (1, true)));
        assert_eq!(filled.bounds(), Rect::new(95, 95, 305, 205), "nothing outside the line");
        let ball = draw(Shape::Ellipse, Form { centred: true, filled: true, ..Default::default() });
        assert_eq!((swatch(&ball, 200, 150), swatch(&ball, 140, 150), swatch(&ball, 104, 104).0), ((2, true), (2, true), 0), "not in the corners");
        // The inside's edge is soft: half a pixel is half covered.
        let (rect, coverage) = Shape::Rectangle.inside(Point::new(10.5, 10.0), Point::new(20.0, 12.5), Form { filled: true, ..Default::default() }).unwrap();
        assert_eq!((rect, coverage[0], coverage[1], coverage[20]), (Rect::new(10, 10, 20, 13), 0.5, 1.0, 0.25));

        // A polyline goes from corner to corner: open, closed back to its start, or filled.
        let corners = [Point::new(100.0, 100.0), Point::new(300.0, 100.0), Point::new(300.0, 200.0)];
        let through = |form: Form| {
            let mut layer = IndexedSurface::new();
            if let Some((rect, coverage)) = Shape::Polyline.inside_through(&corners, form) {
                layer.paint(rect, &coverage, SwatchId(2));
            }
            paint_stroke(&mut layer, &Shape::Polyline.stroke_through(&brush, &corners, form), SwatchId(1));
            layer
        };
        let open = through(Form::default());
        assert!(painted(&open, 200, 100) && painted(&open, 300, 150) && !painted(&open, 200, 150), "two sides, and not the way back");
        let closed = through(Form { closed: true, ..Default::default() });
        assert!(painted(&closed, 200, 150) && !painted(&closed, 260, 120), "and the way back; nothing inside");
        let solid = through(Form { filled: true, ..Default::default() });
        assert_eq!((swatch(&solid, 260, 120), swatch(&solid, 200, 150).0, solid.pixel(140, 180).coverage), ((2, true), 1, 0));
        assert_eq!(Shape::Polyline.inside_through(&corners[..2], Form { filled: true, ..Default::default() }), None);
        assert!(Shape::Polyline.outline_through(&[], Form::default()).is_empty());
        // Any other shape through corners is the shape between the first and the last.
        assert_eq!(Shape::Rectangle.outline_through(&corners, Form::default()), Shape::Rectangle.outline(corners[0], corners[2], Form::default()));
    }

    #[test]
    fn mohos_other_shapes_lie_where_they_are_dragged() {
        use kurbo::Shape as _;
        let (from, to) = (Point::new(100.0, 100.0), Point::new(300.0, 200.0));
        let form = Form::default();
        let ends = |path: &[kurbo::BezPath]| path.iter().map(|line| line.segments().count()).collect::<Vec<_>>();
        // A triangle in the box, its point at the top middle; even, as high as an even one.
        let triangle = Shape::Triangle.vector_lines(from, to, form);
        assert_eq!(ends(&triangle), [3]);
        let top = triangle[0].elements()[0].end_point().unwrap();
        assert_eq!((top.x, top.y), (200.0, 100.0));
        let even = Shape::Triangle.vector_lines(from, to, Form { even: true, ..form })[0].bounding_box();
        assert!((even.height() - 200.0 * 3f64.sqrt() / 2.0).abs() < 1e-6, "{even:?}");
        // A star of so many points, a point at the top, within the box.
        let star = Shape::Star.vector_lines(from, to, Form { tips: 6, ..form });
        assert_eq!(ends(&star), [12]);
        let box_ = star[0].bounding_box();
        assert!(box_.y0 >= 100.0 - 1e-6 && box_.y1 <= 200.0 + 1e-6 && (box_.y0 - 100.0).abs() < 1e-6, "{box_:?}");
        // An arrow from its tail to its head, at the head.
        let arrow = Shape::Arrow.outline(from, to, form);
        assert!(arrow.iter().any(|at| apart(*at, to) < 1e-6), "the head is where it was dragged to");
        assert_eq!(ends(&Shape::Arrow.vector_lines(from, to, form)), [7]);
        // A spiral out from its middle to the pointer, more turns the further, and the other way.
        let spiral = Shape::Spiral.outline(from, to, form);
        assert!(apart(spiral[0], from) < 1e-6 && apart(spiral[spiral.len() - 1], to) < 1e-6);
        let turned = |form: Form| {
            let points = Shape::Spiral.outline(from, to, form);
            points.windows(2).map(|pair| (pair[0].x - from.x) * (pair[1].y - from.y) - (pair[0].y - from.y) * (pair[1].x - from.x)).sum::<f64>()
        };
        assert!(turned(form) > 0.0 && turned(Form { counter: true, ..form }) < 0.0, "with the clock, and against it");
        let windings = |spacing: f64| {
            let points = Shape::Spiral.outline(from, to, Form { spacing, ..form });
            points
                .windows(2)
                .map(|pair| (pair[1].y - from.y).atan2(pair[1].x - from.x) - (pair[0].y - from.y).atan2(pair[0].x - from.x))
                .map(|turn| (turn + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI)
                .sum::<f64>()
                .abs()
                / std::f64::consts::TAU
        };
        assert!((windings(24.0) - 223.6 / 24.0).abs() < 0.3 && windings(48.0) < windings(24.0), "{}", windings(24.0));
        // A grid: its outside and the lines inside, apart on vector art, one path for pixels.
        let grid = Shape::Grid.vector_lines(from, to, Form { rows: 2, columns: 3, ..form });
        assert_eq!(ends(&grid), [4, 1, 1, 1]);
        let pixels = drawn_with(Shape::Grid, from, to, Form { rows: 2, columns: 3, ..form });
        assert!(painted(&pixels, 200, 150) && painted(&pixels, 166, 120) && painted(&pixels, 233, 180) && !painted(&pixels, 140, 125));
        // An ellipse on vector art is its own four curves, ending where it began; its inside closed.
        let ellipse = Shape::Ellipse.vector_lines(from, to, form);
        let ellipse_ends = ellipse[0].elements().iter().filter_map(|element| element.end_point()).collect::<Vec<_>>();
        assert!(ellipse[0].segments().count() == 4 && ellipse_ends[0].distance(ellipse_ends[ellipse_ends.len() - 1]) < 1e-6);
        assert!(Shape::Ellipse.vector_inside_through(&[from, to], Form { filled: true, ..form }).is_some());
        assert!(Shape::Spiral.vector_inside_through(&[from, to], Form { filled: true, ..form }).is_none(), "a spiral has no inside");
        assert!(Shape::Grid.inside(from, to, Form { filled: true, ..form }).is_none() && Shape::Star.inside(from, to, Form { filled: true, ..form }).is_some());
    }

    fn drawn_with(shape: Shape, from: Point, to: Point, form: Form) -> IndexedSurface {
        let mut layer = IndexedSurface::new();
        let brush = BrushSettings { size: 6.0, pressure_size: false, ..Default::default() };
        paint_stroke(&mut layer, &shape.stroke(&brush, from, to, form), SwatchId(1));
        layer
    }
}
