//! What a selection takes of a drawing (plan §4.2): everything in a rectangle, or of that
//! only what lies inside a lasso, only what one swatch painted, and never what is painted
//! with a protected swatch. A selection is a mask in that sense: it says of each pixel
//! whether it is taken.

use std::sync::Arc;

use photocraft_geom::{Affine, Point, Rect};

use crate::{IndexedPixel, SwatchId};

/// The most pixels a lasso's bounds may cover: more than any drawing has.
const MOST: i64 = 1 << 30;

/// A closed outline drawn by hand, in the drawing's pixels. A pixel is inside it when its
/// middle is, with holes where the outline crosses itself an even number of times around.
#[derive(Clone, Debug, PartialEq)]
pub struct Lasso {
    points: Vec<Point>,
    rect: Rect,
    /// For each row of `rect`, the stretches of pixels that are inside: first, and after last.
    rows: Vec<Vec<[i32; 2]>>,
}

impl Lasso {
    /// The lasso through `points`, closed from the last to the first. `None` when it holds
    /// no pixel: fewer than three points, or all on a line.
    pub fn new(points: &[Point]) -> Option<Lasso> {
        if points.len() < 3 || points.iter().any(|at| !at.x.is_finite() || !at.y.is_finite()) {
            return None;
        }
        let (x0, x1) = points.iter().fold((f64::MAX, f64::MIN), |(low, high), at| (low.min(at.x), high.max(at.x)));
        let (y0, y1) = points.iter().fold((f64::MAX, f64::MIN), |(low, high), at| (low.min(at.y), high.max(at.y)));
        if [x0, x1, y0, y1].iter().any(|side| side.abs() > f64::from(i32::MAX / 4)) {
            return None;
        }
        let around = Rect::new(x0.floor() as i32, y0.floor() as i32, x1.ceil() as i32, y1.ceil() as i32);
        if around.is_empty() || i64::from(around.width()) * i64::from(around.height()) > MOST {
            return None;
        }
        let mut rows = Vec::with_capacity(around.height() as usize);
        let mut crossings: Vec<f64> = Vec::new();
        let mut rect = Rect::EMPTY;
        for y in around.y0..around.y1 {
            // Where a level line through the middles of this row's pixels crosses the outline.
            let level = f64::from(y) + 0.5;
            crossings.clear();
            for (i, a) in points.iter().enumerate() {
                let b = points[(i + 1) % points.len()];
                if (a.y <= level) != (b.y <= level) {
                    crossings.push(a.x + (level - a.y) / (b.y - a.y) * (b.x - a.x));
                }
            }
            crossings.sort_by(f64::total_cmp);
            // A pixel is inside when its middle is between two crossings.
            let inside = |[enters, leaves]: &[f64; 2]| [(enters - 0.5).ceil() as i32, (leaves - 0.5).ceil() as i32];
            let row: Vec<[i32; 2]> = crossings.as_chunks::<2>().0.iter().map(inside).filter(|[first, after]| first < after).collect();
            for [first, after] in &row {
                rect = rect.union(&Rect::new(*first, y, *after, y + 1));
            }
            rows.push(row);
        }
        if rect.is_empty() {
            return None;
        }
        // The rows of the rectangle that has something in it.
        let rows = rows.drain(..).skip((rect.y0 - around.y0) as usize).take(rect.height() as usize).collect();
        Some(Lasso { points: points.to_vec(), rect, rows })
    }

    /// The lasso that holds just the pixels of `rect`.
    pub fn of_rect(rect: Rect) -> Option<Lasso> {
        let (x0, y0, x1, y1) = (f64::from(rect.x0), f64::from(rect.y0), f64::from(rect.x1), f64::from(rect.y1));
        Lasso::new(&[Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)])
    }

    /// The outline's points, as it was drawn.
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// The smallest rectangle around the pixels that are inside.
    pub fn bounds(&self) -> Rect {
        self.rect
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        let row = usize::try_from(y - self.rect.y0).ok().and_then(|row| self.rows.get(row));
        row.is_some_and(|row| row.iter().any(|[first, after]| (*first..*after).contains(&x)))
    }

    /// The lasso moved, scaled or turned as what it holds was. `None` when nothing is left
    /// inside it.
    pub fn transformed(&self, transform: &Affine) -> Option<Lasso> {
        let moved: Vec<Point> = self.points.iter().map(|at| transform.apply(*at)).collect();
        Lasso::new(&moved)
    }
}

/// One of the outlines a selection is made of: what it holds is added to what the outlines
/// before it gave, or taken from it.
#[derive(Clone, Debug, PartialEq)]
pub struct LassoPart {
    pub lasso: Arc<Lasso>,
    pub subtract: bool,
}

impl LassoPart {
    /// The part moved, scaled or turned. `None` when nothing is left inside it.
    pub fn transformed(&self, transform: &Affine) -> Option<LassoPart> {
        Some(LassoPart { lasso: Arc::new(self.lasso.transformed(transform)?), subtract: self.subtract })
    }

    /// Whether the pixel at (`x`, `y`) is inside what `parts` make together, each in its turn
    /// adding what it holds or taking it away. No parts hold everything.
    pub fn inside(parts: &[LassoPart], x: i32, y: i32) -> bool {
        parts.is_empty() || parts.iter().fold(false, |inside, part| if part.lasso.contains(x, y) { !part.subtract } else { inside })
    }

    /// The smallest rectangle around what the parts that add hold: no smaller than what `parts`
    /// make together.
    pub fn around(parts: &[LassoPart]) -> Rect {
        parts.iter().filter(|part| !part.subtract).fold(Rect::EMPTY, |around, part| around.union(&part.lasso.bounds()))
    }
}

/// What a selection takes of an art layer.
#[derive(Clone, Copy, Debug)]
pub struct Picked<'a> {
    /// Nothing outside this is taken.
    pub rect: Rect,
    /// Only what is inside what these outlines make together, if there are any.
    pub parts: &'a [LassoPart],
    /// Only what is painted with this swatch, if it names one.
    pub swatch: Option<SwatchId>,
    /// What is painted with one of these is left where it is.
    pub protected: &'a [SwatchId],
}

impl Picked<'_> {
    /// Everything in `rect`.
    pub fn all(rect: Rect) -> Picked<'static> {
        Picked { rect, parts: &[], swatch: None, protected: &[] }
    }

    /// Whether the painted `pixel` at (`x`, `y`) is taken.
    pub fn takes(&self, x: i32, y: i32, pixel: IndexedPixel) -> bool {
        self.rect.contains(x, y)
            && LassoPart::inside(self.parts, x, y)
            && self.swatch.is_none_or(|swatch| pixel.swatch.base() == swatch)
            && !self.protected.contains(&pixel.swatch.base())
    }

    /// Whether it takes everything in its rectangle, whatever is painted there.
    pub fn is_all(&self) -> bool {
        self.parts.is_empty() && self.swatch.is_none() && self.protected.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IndexedSurface;

    fn lasso(points: &[(f64, f64)]) -> Option<Lasso> {
        Lasso::new(&points.iter().map(|(x, y)| Point::new(*x, *y)).collect::<Vec<_>>())
    }

    #[test]
    fn a_lasso_holds_the_pixels_whose_middles_are_inside_its_outline() {
        // A triangle: its right angle at the top left.
        let triangle = lasso(&[(10.0, 10.0), (30.0, 10.0), (10.0, 30.0)]).unwrap();
        assert_eq!(triangle.bounds(), Rect::new(10, 10, 29, 29), "the pixels the slanting side cuts through the middle of are out");
        assert!(triangle.contains(10, 10) && triangle.contains(28, 10) && triangle.contains(10, 28) && triangle.contains(15, 15));
        assert!(
            !triangle.contains(25, 25) && !triangle.contains(9, 10) && !triangle.contains(10, 9) && !triangle.contains(10, 30) && !triangle.contains(500, 500)
        );
        // Moved, it holds what it held, there.
        let moved = triangle.transformed(&Affine::translate(100.0, 0.0)).unwrap();
        assert!(moved.contains(115, 15) && !moved.contains(15, 15) && moved.bounds() == Rect::new(110, 10, 129, 29));
        // What holds no pixel is no lasso.
        assert_eq!((lasso(&[(0.0, 0.0), (10.0, 10.0)]), lasso(&[(0.0, 0.0), (10.0, 10.0), (20.0, 20.0)])), (None, None));
        assert_eq!((lasso(&[(0.1, 0.1), (0.3, 0.1), (0.2, 0.3)]), lasso(&[(0.0, 0.0), (f64::NAN, 1.0), (5.0, 5.0)])), (None, None));
    }

    #[test]
    fn a_selection_takes_what_is_inside_its_lasso_of_its_swatch_and_not_protected() {
        // Rows of red and of blue, twenty pixels wide.
        let mut layer = IndexedSurface::new();
        layer.paint(Rect::new(0, 0, 20, 10), &[1.0; 200], SwatchId(3));
        layer.paint(Rect::new(0, 10, 20, 20), &[1.0; 200], SwatchId(5));
        let left = [LassoPart { lasso: Arc::new(lasso(&[(0.0, 0.0), (10.0, 0.0), (10.0, 20.0), (0.0, 20.0)]).unwrap()), subtract: false }];
        let all = Rect::new(0, 0, 20, 20);
        let count = |layer: &IndexedSurface| layer.pixels(all).iter().filter(|pixel| pixel.coverage > 0).count();

        let halved = Picked { rect: all, parts: &left, swatch: None, protected: &[] };
        assert_eq!((count(&layer.extract_picked(&halved)), layer.extract_picked(&halved).bounds()), (200, Rect::new(0, 0, 10, 20)));
        let red = Picked { rect: all, parts: &left, swatch: Some(SwatchId(3)), protected: &[] };
        assert_eq!(layer.extract_picked(&red).bounds(), Rect::new(0, 0, 10, 10));
        let kept = Picked { rect: all, parts: &[], swatch: None, protected: &[SwatchId(5)] };
        assert_eq!(layer.extract_picked(&kept).bounds(), Rect::new(0, 0, 20, 10));
        assert_eq!(count(&layer.extract_picked(&Picked::all(Rect::new(5, 5, 8, 8)))), 9);

        // Cleared, only what was taken is gone.
        layer.clear_picked(&red);
        assert_eq!((count(&layer), layer.pixel(5, 5).coverage, layer.pixel(15, 5).swatch, layer.pixel(5, 15).swatch), (300, 0, SwatchId(3), SwatchId(5)));
        assert!(Picked::all(all).is_all() && !red.is_all());

        // Outlines add to what those before them hold, or take from it, each in its turn:
        // the left half, and the bottom right quarter, without a square in their middle.
        let part = |rect: Rect, subtract: bool| LassoPart { lasso: Arc::new(Lasso::of_rect(rect).unwrap()), subtract };
        let parts = [left[0].clone(), part(Rect::new(10, 10, 20, 20), false), part(Rect::new(5, 5, 15, 15), true)];
        let held = |x, y| LassoPart::inside(&parts, x, y);
        assert_eq!((held(2, 2), held(15, 18), held(15, 2), held(7, 7), held(12, 12), held(2, 18)), (true, true, false, false, false, true));
        assert_eq!((LassoPart::around(&parts), LassoPart::around(&parts[2..]), LassoPart::inside(&[], 99, 99)), (all, Rect::EMPTY, true));
        // What a selection takes is given another swatch, and keeps how much it covers.
        let mut layer = IndexedSurface::new();
        layer.paint(Rect::new(0, 0, 20, 20), &[0.5; 400], SwatchId(3));
        let soft = layer.pixel(2, 2).coverage;
        assert!(layer.recolour_picked(&Picked { rect: all, parts: &parts, swatch: None, protected: &[] }, SwatchId(4)));
        assert_eq!(
            (layer.pixel(2, 2), layer.pixel(7, 7).swatch, layer.pixel(15, 2).swatch),
            (IndexedPixel { swatch: SwatchId(4), coverage: soft }, SwatchId(3), SwatchId(3))
        );
        assert!(!layer.recolour_picked(&Picked { swatch: Some(SwatchId(9)), ..Picked::all(all) }, SwatchId(4)));
    }
}
