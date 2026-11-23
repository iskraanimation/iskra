//! The Select tool: a selection on the current drawing, a rectangle or the inside of a lasso,
//! that can be dragged to move it, scaled by its corners and turned by the handle above it.
//!
//! Everything here is in the drawing's pixels. While a handle is dragged the viewport shows the
//! result; the engine gets one `drawing.transform` command when the drag ends.

use std::sync::Arc;

use iskra_drawing::{Affine, Lasso, LassoPart, Picked, Point, Rect};
use iskra_model::{ArtContent, ArtLayerKind, Drawing, LayerId, SwatchId};
use serde_json::{Value, json};

/// What a drag on the selection does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Move,
    /// A corner, clockwise from the top left: dragging it scales about the opposite corner.
    Corner(usize),
    /// The middle of a side, clockwise from the top: dragging it scales one way, about the
    /// opposite side.
    Side(usize),
    Rotate,
    /// A side, clockwise from the top, dragged between its handles: it slides along itself
    /// with the pointer, and the opposite side stays, which skews.
    Skew(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub layer: LayerId,
    pub frame: u32,
    /// The rectangle the handles are at: all of it is selected, or of it what the lasso and
    /// the swatch say.
    pub rect: Rect,
    /// The outlines that make what is selected together, each adding to those before it or
    /// taking from them; none, when all of the rectangle is.
    pub parts: Vec<LassoPart>,
    /// The one swatch whose paint is selected, if it is not all paint.
    pub swatch: Option<SwatchId>,
}

impl Selection {
    /// All of a rectangle of the drawing a layer shows on a frame.
    pub fn of(layer: LayerId, frame: u32, rect: Rect) -> Self {
        Self { layer, frame, rect, parts: Vec::new(), swatch: None }
    }

    /// The inside of an outline drawn on the drawing a layer shows on a frame.
    pub fn of_lasso(layer: LayerId, frame: u32, lasso: Lasso) -> Self {
        let around = lasso.bounds();
        Self { parts: vec![LassoPart { lasso: Arc::new(lasso), subtract: false }], ..Self::of(layer, frame, around) }
    }

    /// This selection with what `more` holds added to it, or with `subtract` taken from it.
    /// `None` when nothing can be left: something was taken from a selection of nothing.
    pub fn with(&self, more: Lasso, subtract: bool) -> Option<Selection> {
        let mut parts = self.parts.clone();
        // All of a rectangle is the outline of that rectangle, to add to and take from.
        if parts.is_empty() {
            parts.push(LassoPart { lasso: Arc::new(Lasso::of_rect(self.rect)?), subtract: false });
        }
        parts.push(LassoPart { lasso: Arc::new(more), subtract });
        let rect = LassoPart::around(&parts);
        (!rect.is_empty()).then(|| Selection { rect, parts, ..self.clone() })
    }

    /// The selection turned inside out within `whole`: what of it is not selected now is, and
    /// what is, is not. Each outline that added to it takes away, and each that took away adds,
    /// after all of `whole`. Of all paint, where it was of one swatch's. `None` where `whole`
    /// is nothing.
    pub fn inverted(&self, whole: Rect) -> Option<Selection> {
        let whole = whole.union(&self.rect);
        let mut parts = vec![LassoPart { lasso: Arc::new(Lasso::of_rect(whole)?), subtract: false }];
        match self.parts.is_empty() {
            true => parts.push(LassoPart { lasso: Arc::new(Lasso::of_rect(self.rect)?), subtract: true }),
            false => parts.extend(self.parts.iter().map(|part| LassoPart { lasso: part.lasso.clone(), subtract: !part.subtract })),
        }
        Some(Selection { rect: whole, parts, swatch: None, ..self.clone() })
    }

    /// The outlines that make it: those it has, or its rectangle's where it is all of it.
    pub fn outlines(&self) -> Vec<LassoPart> {
        match self.parts.is_empty() {
            true => Lasso::of_rect(self.rect).map(|lasso| vec![LassoPart { lasso: Arc::new(lasso), subtract: false }]).unwrap_or_default(),
            false => self.parts.clone(),
        }
    }

    /// What it takes of an art layer, leaving the `protected` swatches.
    pub fn picked<'a>(&'a self, protected: &'a [SwatchId]) -> Picked<'a> {
        Picked { rect: self.rect, parts: &self.parts, swatch: self.swatch, protected }
    }

    /// The selection as the engine's commands on one take it.
    pub fn params(&self) -> Value {
        let rect = self.rect;
        let outline = |part: &LassoPart| json!({ "lasso": part.lasso.points().iter().map(|at| [at.x, at.y]).collect::<Vec<_>>(), "subtract": part.subtract });
        let parts: Vec<Value> = self.parts.iter().map(outline).collect();
        json!({ "layer": self.layer.0, "frame": self.frame, "rect": [rect.x0, rect.y0, rect.x1, rect.y1], "parts": parts, "swatch": self.swatch })
    }
}

/// The corners of a rectangle, clockwise from the top left.
pub fn corners(rect: Rect) -> [Point; 4] {
    let (x0, y0, x1, y1) = (f64::from(rect.x0), f64::from(rect.y0), f64::from(rect.x1), f64::from(rect.y1));
    [Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)]
}

pub fn center(rect: Rect) -> Point {
    Point::new(f64::from(rect.x0 + rect.x1) / 2.0, f64::from(rect.y0 + rect.y1) / 2.0)
}

/// The rectangle between two points, as whole pixels. `None` if it is under two pixels a side:
/// a click, not a drag.
pub fn marquee(a: Point, b: Point) -> Option<Rect> {
    let rect = Rect::new(a.x.min(b.x).floor() as i32, a.y.min(b.y).floor() as i32, a.x.max(b.x).ceil() as i32, a.y.max(b.y).ceil() as i32);
    (rect.width() >= 2 && rect.height() >= 2).then_some(rect)
}

/// The transform a drag of `handle` from `start` to `now` asks for.
pub fn drag_matrix(handle: Handle, rect: Rect, start: Point, now: Point) -> Affine {
    drag_matrix_in(handle, [f64::from(rect.x0), f64::from(rect.y0), f64::from(rect.x1), f64::from(rect.y1)], start, now)
}

/// The transform a drag of `handle` of a box from (`x0`, `y0`) to (`x1`, `y1`), from `start`
/// to `now`, asks for, all in one measure.
pub fn drag_matrix_in(handle: Handle, [x0, y0, x1, y1]: [f64; 4], start: Point, now: Point) -> Affine {
    match handle {
        Handle::Move => Affine::translate(now.x - start.x, now.y - start.y),
        Handle::Corner(index) => {
            let all = [Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)];
            let (corner, fixed) = (all[index % 4], all[(index + 2) % 4]);
            // The corner follows the pointer; it can't cross the fixed corner and flip the art.
            let scale = |now: f64, corner: f64, fixed: f64| ((now - fixed) / (corner - fixed)).max(0.02);
            let (sx, sy) = (scale(now.x, corner.x, fixed.x), scale(now.y, corner.y, fixed.y));
            Affine { m: [sx, 0.0, 0.0, sy, fixed.x * (1.0 - sx), fixed.y * (1.0 - sy)] }
        }
        Handle::Side(index) => {
            // The side follows the pointer one way; it can't cross the opposite side either.
            let scale = |now: f64, side: f64, fixed: f64| ((now - fixed) / (side - fixed)).max(0.02);
            match index % 4 {
                0 => Affine { m: [1.0, 0.0, 0.0, scale(now.y, y0, y1), 0.0, y1 * (1.0 - scale(now.y, y0, y1))] },
                1 => Affine { m: [scale(now.x, x1, x0), 0.0, 0.0, 1.0, x0 * (1.0 - scale(now.x, x1, x0)), 0.0] },
                2 => Affine { m: [1.0, 0.0, 0.0, scale(now.y, y1, y0), 0.0, y0 * (1.0 - scale(now.y, y1, y0))] },
                _ => Affine { m: [scale(now.x, x0, x1), 0.0, 0.0, 1.0, x1 * (1.0 - scale(now.x, x0, x1)), 0.0] },
            }
        }
        Handle::Skew(index) => {
            let (dx, dy) = (now.x - start.x, now.y - start.y);
            // How far the side goes along itself for each pixel out from the side that stays.
            match index % 4 {
                0 => shear_x(dx / (y0 - y1), y1),
                1 => shear_y(dy / (x1 - x0), x0),
                2 => shear_x(dx / (y1 - y0), y0),
                _ => shear_y(dy / (x0 - x1), x1),
            }
        }
        Handle::Rotate => {
            let c = Point::new((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            let angle = (now.y - c.y).atan2(now.x - c.x) - (start.y - c.y).atan2(start.x - c.x);
            Affine::translate(-c.x, -c.y).then(&Affine::rotate(angle)).then(&Affine::translate(c.x, c.y))
        }
    }
}

/// Across by `k` for each pixel down from `y`, which stays.
fn shear_x(k: f64, y: f64) -> Affine {
    let k = if k.is_finite() { k } else { 0.0 };
    Affine { m: [1.0, 0.0, k, 1.0, -k * y, 0.0] }
}

/// Down by `k` for each pixel across from `x`, which stays.
fn shear_y(k: f64, x: f64) -> Affine {
    let k = if k.is_finite() { k } else { 0.0 };
    Affine { m: [1.0, k, 0.0, 1.0, 0.0, -k * x] }
}

/// A handle being dragged, with what the viewport shows meanwhile.
pub struct Drag {
    pub handle: Handle,
    pub start: Point,
    pub matrix: Affine,
    /// Per art layer: the layer without the selection, and the selection lifted off it.
    layers: Vec<Option<(ArtContent, ArtContent)>>,
    /// Per art layer: what to show, the layer with the selection moved by `matrix`.
    pub preview: Vec<Option<ArtContent>>,
}

impl Drag {
    /// A drag of what a selection takes of `drawing`: of the art layers `only` takes,
    /// indexed or free colour, but for free colour that is a `placed` picture, which is its
    /// file's.
    pub fn new(handle: Handle, start: Point, drawing: &Drawing, picked: &Picked, only: iskra_model::ArtReach, placed: bool) -> Self {
        let layers: Vec<_> = drawing
            .art_layers
            .iter()
            .map(|layer| {
                let taken = only.takes(layer.kind) && !(placed && layer.kind == ArtLayerKind::ColourArt);
                taken.then(|| {
                    let mut rest = layer.content.clone();
                    rest.clear_picked(picked);
                    (rest, layer.content.extract_picked(picked))
                })
            })
            .collect();
        let preview = layers.iter().map(|_| None).collect();
        Self { handle, start, matrix: Affine::IDENTITY, layers, preview }
    }

    /// The pointer moved to `now`: update the transform and the preview.
    pub fn update(&mut self, rect: Rect, now: Point) {
        let matrix = drag_matrix(self.handle, rect, self.start, now);
        if matrix == self.matrix && self.preview.iter().any(Option::is_some) {
            return;
        }
        self.matrix = matrix;
        self.preview = self
            .layers
            .iter()
            .map(|layer| {
                layer.as_ref().map(|(rest, lifted)| match (rest, lifted) {
                    // Vector art moved, or with stamped lines, is shown as the pixels its shapes
                    // made, moved: making them anew at each move of the pointer stamps every
                    // stamped line again (the user's of 2026-11-23). A move is seen by whole
                    // pixels; the engine moves the shapes exactly when let go.
                    (ArtContent::Vector(rest), ArtContent::Vector(lifted)) if matrix.m[..4] == [1.0, 0.0, 0.0, 1.0] || !lifted.tips().is_empty() => {
                        let [.., e, f] = matrix.m;
                        let moved = match matrix.m[..4] == [1.0, 0.0, 0.0, 1.0] {
                            true => lifted.raster().translated(e.round() as i32, f.round() as i32),
                            false => lifted.raster().transformed(&matrix),
                        };
                        let mut shown = rest.raster().clone();
                        shown.stamp(&moved);
                        ArtContent::Indexed(shown)
                    }
                    _ => {
                        let mut shown = rest.clone();
                        shown.stamp(&lifted.transformed(&matrix));
                        shown
                    }
                })
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: Rect = Rect { x0: 100, y0: 50, x1: 300, y1: 150 };

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn a_marquee_is_whole_pixels_whichever_way_it_is_dragged() {
        let expected = Some(Rect::new(10, 20, 31, 41));
        assert_eq!(marquee(Point::new(10.4, 20.9), Point::new(30.2, 40.1)), expected);
        assert_eq!(marquee(Point::new(30.2, 40.1), Point::new(10.4, 20.9)), expected);
        assert_eq!(marquee(Point::new(10.0, 10.0), Point::new(10.5, 300.0)), None, "a click, or a sliver, selects nothing");
    }

    #[test]
    fn dragging_inside_moves() {
        let m = drag_matrix(Handle::Move, RECT, Point::new(150.0, 100.0), Point::new(170.0, 90.0));
        assert!(close(m.apply(Point::new(100.0, 50.0)), Point::new(120.0, 40.0)));
    }

    #[test]
    fn dragging_a_corner_scales_about_the_opposite_corner() {
        // The bottom right corner dragged from (300, 150) to (500, 100): twice as wide, half as high.
        let m = drag_matrix(Handle::Corner(2), RECT, Point::new(300.0, 150.0), Point::new(500.0, 100.0));
        assert!(close(m.apply(Point::new(100.0, 50.0)), Point::new(100.0, 50.0)), "the top left corner stays");
        assert!(close(m.apply(Point::new(300.0, 150.0)), Point::new(500.0, 100.0)), "the dragged corner follows the pointer");
        // Dragged past the fixed corner, the selection gets very small but never flips.
        let m = drag_matrix(Handle::Corner(2), RECT, Point::new(300.0, 150.0), Point::new(0.0, 0.0));
        assert!(m.m[0] > 0.0 && m.m[3] > 0.0 && m.inverse().is_some());
    }

    #[test]
    fn dragging_a_side_scales_one_way_about_the_opposite_side() {
        // The right side, at 300, dragged out to 400: half as wide again, as high as it was.
        let m = drag_matrix(Handle::Side(1), RECT, Point::new(300.0, 100.0), Point::new(400.0, 130.0));
        assert!(close(m.apply(Point::new(100.0, 50.0)), Point::new(100.0, 50.0)), "the left side stays");
        assert!(close(m.apply(Point::new(300.0, 150.0)), Point::new(400.0, 150.0)), "the right goes with the pointer, across only");
        // The top, at 50, dragged up to 0: twice as high, about the bottom.
        let m = drag_matrix(Handle::Side(0), RECT, Point::new(200.0, 50.0), Point::new(260.0, -50.0));
        assert!(close(m.apply(Point::new(100.0, 150.0)), Point::new(100.0, 150.0)) && close(m.apply(Point::new(100.0, 50.0)), Point::new(100.0, -50.0)));
        // Past the opposite side, very thin but never flipped.
        let m = drag_matrix(Handle::Side(3), RECT, Point::new(100.0, 100.0), Point::new(500.0, 100.0));
        assert!(m.m[0] > 0.0 && m.m[3] == 1.0);
    }

    #[test]
    fn dragging_the_rotate_handle_turns_about_the_centre() {
        // From straight above the centre (200, 100) to straight right of it: a quarter turn.
        let m = drag_matrix(Handle::Rotate, RECT, Point::new(200.0, 0.0), Point::new(400.0, 100.0));
        assert!(close(m.apply(Point::new(200.0, 100.0)), Point::new(200.0, 100.0)), "the centre stays");
        assert!(close(m.apply(Point::new(200.0, 50.0)), Point::new(250.0, 100.0)), "top goes to the right");
    }

    fn indexed(art: &ArtContent) -> &iskra_drawing::IndexedSurface {
        let ArtContent::Indexed(surface) = art else { panic!("indexed art") };
        surface
    }

    #[test]
    fn a_drag_previews_the_drawing_with_the_selection_moved() {
        let mut drawing = Drawing::default();
        let Some(ArtContent::Indexed(line)) = drawing.layer_mut(ArtLayerKind::LineArt).map(|l| &mut l.content) else { panic!("indexed") };
        line.paint(Rect::new(0, 0, 20, 2), &[1.0; 40], iskra_model::SwatchId(1));
        let rect = Rect::new(0, 0, 10, 2);
        let mut drag = Drag::new(Handle::Move, Point::new(5.0, 1.0), &drawing, &Picked::all(rect), iskra_model::ArtReach::All, false);
        drag.update(rect, Point::new(5.0, 31.0));
        let shown = drag.preview[1].as_ref().map(indexed).expect("the line art has a preview");
        assert_eq!((shown.pixel(5, 0).coverage, shown.pixel(5, 30).coverage, shown.pixel(15, 0).coverage), (0, 65535, 65535));
        assert!(drag.preview[0].as_ref().is_some_and(|art| art.bounds().is_empty()), "the empty colour art stays empty");

        // Of the colour art alone, the line art has no preview and is shown as it is; and
        // what is painted with a protected swatch stays where it was.
        let mut other =
            Drag::new(Handle::Move, Point::new(5.0, 1.0), &drawing, &Picked::all(rect), iskra_model::ArtReach::Only(ArtLayerKind::ColourArt), false);
        other.update(rect, Point::new(5.0, 31.0));
        assert!(other.preview[1].is_none() && other.preview[0].is_some());
        let selection = Selection::of(LayerId(1), 0, rect);
        let mut kept = Drag::new(Handle::Move, Point::new(5.0, 1.0), &drawing, &selection.picked(&[SwatchId(1)]), iskra_model::ArtReach::All, false);
        kept.update(rect, Point::new(5.0, 31.0));
        let shown = kept.preview[1].as_ref().map(indexed).expect("the line art has a preview");
        assert_eq!((shown.pixel(5, 0).coverage, shown.pixel(5, 30).coverage), (65535, 0));

        // Inside a lasso, only what the outline holds goes: the left of the selected half.
        let outline = [Point::new(0.0, 0.0), Point::new(4.0, 0.0), Point::new(4.0, 2.0), Point::new(0.0, 2.0)];
        let lassoed = Selection::of_lasso(LayerId(1), 0, Lasso::new(&outline).unwrap());
        let mut part = Drag::new(Handle::Move, Point::new(2.0, 1.0), &drawing, &lassoed.picked(&[]), iskra_model::ArtReach::All, false);
        part.update(rect, Point::new(2.0, 31.0));
        let shown = part.preview[1].as_ref().map(indexed).expect("the line art has a preview");
        assert_eq!((shown.pixel(2, 0).coverage, shown.pixel(2, 30).coverage, shown.pixel(5, 0).coverage, shown.pixel(5, 30).coverage), (0, 65535, 65535, 0));
        assert_eq!(lassoed.params()["parts"], json!([{ "lasso": [[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [0.0, 2.0]], "subtract": false }]));

        // More is added to a selection, and taken from it: the rectangle is around it all.
        let more = selection.with(Lasso::of_rect(Rect::new(14, 0, 18, 2)).unwrap(), false).expect("there is more");
        let less = more.with(Lasso::of_rect(Rect::new(0, 0, 2, 2)).unwrap(), true).expect("something is left");
        let held = |x| LassoPart::inside(&less.parts, x, 0);
        assert_eq!((more.rect, less.parts.len(), held(1), held(5), held(12), held(16)), (Rect::new(0, 0, 18, 2), 3, false, true, false, true));
    }

    #[test]
    fn a_drag_of_vector_art_previews_the_pixels_its_shapes_made_moved() {
        use iskra_drawing::{ShapeId, ShapeKind, VectorArt, VectorShape, VectorStroke, kurbo::BezPath};
        let mut drawing = Drawing::default();
        let line = |y: f64| {
            let mut path = BezPath::new();
            path.move_to((10.0, y));
            path.line_to((90.0, y));
            path
        };
        let shapes = [(1, 20.0), (2, 60.0)].map(|(id, y)| VectorShape {
            id: ShapeId(id),
            swatch: SwatchId(1),
            kind: ShapeKind::Stroke(VectorStroke::even(line(y), 6.0)),
        });
        let art = VectorArt::new(shapes.to_vec());
        drawing.layer_mut(ArtLayerKind::LineArt).unwrap().content = ArtContent::Vector(art.clone());
        // The upper line taken, moved down by 10.
        let rect = Rect::new(0, 10, 100, 30);
        let mut drag = Drag::new(Handle::Move, Point::new(50.0, 20.0), &drawing, &Picked::all(rect), iskra_model::ArtReach::All, false);
        drag.update(rect, Point::new(50.0, 30.0));
        let shown = drag.preview[1].as_ref().map(indexed).expect("shown as pixels");
        let full = art.raster().pixel(50, 20).coverage;
        assert_eq!((shown.pixel(50, 20).coverage, shown.pixel(50, 30).coverage, shown.pixel(50, 60).coverage), (0, full, full));
    }
}
