//! The point editor (plan §4.16, step 8): the points and handles of shapes of vector art, shown
//! over them and dragged.
//!
//! A click on a shape of the art layer in hand shows its points, with Shift beside those shown.
//! A box or a lasso, as the bar says, picks the strokes it touches, their points shown, or the
//! points inside it, of the shapes shown or, with none shown, of any. A click on a point picks
//! it, with Shift adds it to those picked or takes it away; with Alt takes it out of its line.
//! The points picked are dragged together, of all the shapes they are of; a handle of one is
//! dragged alone, its twin turning with it while the point is smooth, unless Alt is held. A
//! click on a shown shape's line, between its points, puts a point there. Delete takes the
//! picked points away; the tool's bar makes them smooth or corners, sets a stroke's width at
//! them, and lays a brush on the strokes shown. Each edit is one `drawing.editShapes`.
//!
//! The handles of every point are shown, those of the points not picked fainter, and can be
//! dragged, unless the bar says the picked points' alone. On a stroke, each picked point has a
//! diamond on the stroke's edge: dragged, it sets the width at the stroke's picked points.

use egui::{Color32, Pos2, Rect, Response, Stroke as Outline};
use iskra_drawing::points::{self, Handle};
use iskra_model::kurbo::{BezPath, PathEl, Point, Shape as _, Vec2};
use iskra_model::{ArtContent, ArtLayerKind, Layer, LayerId, ShapeId, ShapeKind, VectorArt, VectorShape};
use serde_json::{Value, json};

/// How near on screen, in points, a press is to a point or a handle to take it, or to a line to
/// put a point on it.
const REACH: f32 = 7.0;

/// The shapes whose points are shown: of which layer, on which frame, which art layer, which
/// shapes, the first shown first.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub layer: LayerId,
    pub frame: u32,
    pub art: ArtLayerKind,
    pub shapes: Vec<ShapeId>,
}

/// A point of a shape: the shape, and the point's index in its path.
pub type Pick = (ShapeId, usize);

/// What a drag in progress does.
#[derive(Clone, Debug)]
enum Drag {
    /// The picked points, from where the press was, in the drawing's pixels; the press was on
    /// `point`, which is picked once the pointer moves, if it isn't. The paths of the shapes
    /// shown, as they were and as the drag makes them.
    Points { point: Pick, from: Point, was: Vec<(ShapeId, BezPath)>, now: Vec<(ShapeId, BezPath)> },
    /// One handle of a point.
    Handle { point: Pick, handle: Handle, was: BezPath, now: BezPath },
    /// A box being dragged out, on screen, to pick what is inside it.
    Box { from: Pos2, to: Pos2 },
    /// A lasso being drawn, on screen, to pick what is inside it.
    Lasso { way: Vec<Pos2> },
    /// The width at the picked points of a stroke, by the diamond of `point`: `from` is how far
    /// from the point the press was, in the drawing's pixels.
    /// What it changes, as they were and as the drag makes them, is the stroke's per-point lists:
    /// of them the one `which` is (0 the width, 1 the opacity, 2 the drift). The opacity and the
    /// drift go with how far across the screen the pointer goes from `origin`.
    Width { point: Pick, from: f64, origin: Pos2, which: usize, was: Lists, now: Lists },
}

/// From the drawing's pixels to the screen and back, through the pegs and the bend.
pub struct Map<'a> {
    pub to_screen: &'a dyn Fn(Point) -> Pos2,
    pub to_drawing: &'a dyn Fn(Pos2) -> Point,
}

/// A stroke's per-point lists: its widths, opacities and drifts, the last two maybe empty.
type Lists = [Vec<f64>; 3];

/// What a drag changes of a shape: its path, and a stroke's per-point lists.
type Change = (ShapeId, BezPath, Option<Lists>);

#[derive(Default)]
pub struct PointEditor {
    pub shown: Option<Shown>,
    /// The picked points.
    pub picked: Vec<Pick>,
    drag: Option<Drag>,
    /// The width the bar's field is being dragged to, until it is let go.
    pub width_edit: Option<f32>,
    /// The opacity and the drift the bar's fields are being dragged to, until they are let go.
    pub tone_edit: [Option<f32>; 2],
    /// While a point or a handle is dragged, and on the frame it is let go until the edit is in
    /// the project, the art layer as the drag makes it: of which layer and frame, what it
    /// changes, and the art.
    live: Option<(LayerId, u32, ArtLayerKind, Vec<Change>, VectorArt)>,
    /// Only the picked points' handles are shown, and taken hold of.
    pub picked_handles_only: bool,
    /// A box or a lasso picks the strokes it touches, not the points inside it.
    pub picks_strokes: bool,
    /// What is picked is drawn round with a lasso, not a box.
    pub lasso: bool,
    /// Where on screen each point's handles and each width diamond were drawn, for the tests.
    #[cfg(test)]
    pub drawn_handles: Vec<(Pick, Pos2)>,
    #[cfg(test)]
    pub drawn_diamonds: Vec<(Pick, Pos2)>,
}

/// The vector art of the art layer `art` of the drawing `layer` shows on `frame`.
pub fn art_of(layer: &Layer, frame: u32, art: ArtLayerKind) -> Option<&VectorArt> {
    match &layer.drawing_at(frame)?.drawing.layer(art)?.content {
        ArtContent::Vector(vector) => Some(vector),
        _ => None,
    }
}

fn path_of(shape: &VectorShape) -> &BezPath {
    match &shape.kind {
        ShapeKind::Stroke(stroke) => &stroke.path,
        ShapeKind::Fill(fill) => &fill.outline,
    }
}

fn widths_of(shape: &VectorShape) -> Option<&Vec<f64>> {
    match &shape.kind {
        ShapeKind::Stroke(stroke) => Some(&stroke.widths),
        ShapeKind::Fill(_) => None,
    }
}

/// A stroke's per-point lists; `None` for a fill.
fn lists_of(shape: &VectorShape) -> Option<Lists> {
    match &shape.kind {
        ShapeKind::Stroke(stroke) => Some([stroke.widths.clone(), stroke.opacities.clone(), stroke.drifts.clone()]),
        ShapeKind::Fill(_) => None,
    }
}

/// What a per-point list of `which` has at `index`: where it is empty, what none means (full
/// opacity, no drift).
fn value_at(list: &[f64], index: usize, which: usize) -> f64 {
    list.get(index).copied().unwrap_or(if which == 1 { 1.0 } else { 0.0 })
}

/// `list`, of a stroke of `points` points, with `value` at `index`: made whole first where it is
/// empty, and empty again where an opacity or a drift is none all along.
fn set_value(list: &mut Vec<f64>, points: usize, index: usize, which: usize, value: f64) {
    let none = if which == 1 { 1.0 } else { 0.0 };
    if list.is_empty() {
        *list = vec![none; points];
    }
    if let Some(slot) = list.get_mut(index) {
        *slot = if which == 0 { value.max(0.0) } else { value.clamp(0.0, 1.0) };
    }
    if which > 0 && list.iter().all(|v| (*v - none).abs() < 1e-9) {
        list.clear();
    }
}

/// `path` with a point put on its element `index`, `t` of the way along it, and each of `lists`
/// with its value there, as [`points::with_point`] does the widths.
fn with_point(path: &BezPath, lists: &Lists, index: usize, t: f64) -> Option<(BezPath, Lists)> {
    let (with, _) = points::with_point(path, &[], index, t)?;
    let lists = lists.clone().map(|list| points::with_point(path, &list, index, t).map_or(list, |(_, list)| list));
    Some((with, lists))
}

/// `path` without its point `index`, and each of `lists` without its value, as
/// [`points::without_point`] does the widths.
fn without_point(path: &BezPath, lists: &Lists, index: usize) -> Option<(BezPath, Lists)> {
    let (without, _) = points::without_point(path, &[], index)?;
    let lists = lists.clone().map(|list| points::without_point(path, &list, index).map_or(list, |(_, list)| list));
    Some((without, lists))
}

/// The command that gives each shape its path, and a stroke its per-point lists.
fn edit(layer: LayerId, frame: u32, art: ArtLayerKind, changes: &[Change]) -> (&'static str, Value) {
    let shapes: Vec<Value> = changes
        .iter()
        .map(|(shape, path, lists)| {
            let mut one = json!({ "shape": shape.0, "path": path.to_svg() });
            if let Some([widths, opacities, drifts]) = lists {
                (one["widths"], one["opacities"], one["drifts"]) = (json!(widths), json!(opacities), json!(drifts));
            }
            one
        })
        .collect();
    ("drawing.editShapes", json!({ "layer": layer.0, "frame": frame, "artLayer": art, "shapes": shapes }))
}

/// Whether `at` is inside the polygon `way`, closed from its end to its start.
fn inside_way(way: &[Pos2], at: Pos2) -> bool {
    let mut inside = false;
    for (index, a) in way.iter().enumerate() {
        let b = way[(index + 1) % way.len()];
        if (a.y > at.y) != (b.y > at.y) && at.x < a.x + (at.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

impl PointEditor {
    /// The shapes shown that are still there on the layer and frame in hand, in their order.
    fn shapes<'a>(&self, layer: &'a Layer, frame: u32, art: ArtLayerKind) -> Vec<&'a VectorShape> {
        let Some(shown) = self.shown.as_ref().filter(|shown| shown.layer == layer.id && shown.frame == frame && shown.art == art) else {
            return Vec::new();
        };
        let Some(vector) = art_of(layer, frame, art) else { return Vec::new() };
        shown.shapes.iter().filter_map(|id| vector.shapes().iter().find(|shape| shape.id == *id)).collect()
    }

    /// The picked points of `shape`.
    fn picked_of(&self, shape: ShapeId) -> Vec<usize> {
        self.picked.iter().filter(|(of, _)| *of == shape).map(|(_, point)| *point).collect()
    }

    /// Show `shapes` of the art layer in hand, beside those shown with `more`.
    fn show(&mut self, layer: LayerId, frame: u32, art: ArtLayerKind, shapes: Vec<ShapeId>, more: bool) {
        let mut all = if more { self.shown.as_ref().map_or(Vec::new(), |shown| shown.shapes.clone()) } else { Vec::new() };
        all.extend(shapes.into_iter().filter(|shape| !all.contains(shape)).collect::<Vec<_>>());
        self.shown = (!all.is_empty()).then_some(Shown { layer, frame, art, shapes: all });
    }

    /// The point of the shapes under `at` on screen, if one is near enough.
    fn point_at(shapes: &[(ShapeId, BezPath)], map: &Map, at: Pos2) -> Option<Pick> {
        let near = shapes.iter().flat_map(|(shape, path)| {
            points::points(path).into_iter().enumerate().filter_map(move |(index, point)| Some(((*shape, index), (map.to_screen)(point?).distance(at))))
        });
        near.filter(|(_, distance)| *distance <= REACH).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(pick, _)| pick)
    }

    /// The handle under `at` on screen: of a picked point first, else of any point, unless only
    /// the picked points' handles are shown.
    fn handle_at(&self, shapes: &[(ShapeId, BezPath)], map: &Map, at: Pos2) -> Option<(Pick, Handle)> {
        let of = |picked: bool| {
            shapes
                .iter()
                .flat_map(move |(shape, path)| (0..path.elements().len()).map(move |point| (*shape, point, path)))
                .filter(move |(shape, point, _)| self.picked.contains(&(*shape, *point)) == picked)
        };
        let picked = Self::nearest_handle(map, at, of(true));
        picked.or_else(|| (!self.picked_handles_only).then(|| Self::nearest_handle(map, at, of(false))).flatten())
    }

    /// The handle of one of `of` points under `at` on screen, the nearest.
    fn nearest_handle<'p>(map: &Map, at: Pos2, of: impl Iterator<Item = (ShapeId, usize, &'p BezPath)>) -> Option<(Pick, Handle)> {
        let mut best: Option<(Pick, Handle, f32)> = None;
        for (shape, point, path) in of {
            let (into, out) = points::handles(path, point);
            for (handle, place) in [(Handle::In, into), (Handle::Out, out)] {
                let Some(place) = place else { continue };
                let distance = (map.to_screen)(place).distance(at);
                if distance <= REACH && best.is_none_or(|(.., nearest)| distance < nearest) {
                    best = Some(((shape, point), handle, distance));
                }
            }
        }
        best.map(|(pick, handle, _)| (pick, handle))
    }

    /// The picked point of a stroke whose width diamond is under `at` on screen.
    fn width_at(&self, shapes: &[&VectorShape], map: &Map, at: Pos2) -> Option<Pick> {
        let near = self.picked.iter().filter_map(|(id, point)| {
            let shape = shapes.iter().find(|shape| shape.id == *id)?;
            Some(((*id, *point), diamond_on_screen(path_of(shape), widths_of(shape)?, *point, map)?.distance(at)))
        });
        near.filter(|(_, distance)| *distance <= REACH).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(pick, _)| pick)
    }

    /// Handle the pointer over the viewport: its drags, and `click`, where a press was let go
    /// without moving. Returns the command an edit asks for.
    pub fn input(
        &mut self,
        ui: &egui::Ui,
        response: &Response,
        click: Option<Pos2>,
        layer: &Layer,
        frame: u32,
        art: ArtLayerKind,
        map: &Map,
    ) -> Option<(&'static str, Value)> {
        let primary = egui::PointerButton::Primary;
        // What the last drag made is in the project by now.
        if !matches!(self.drag, Some(Drag::Points { .. } | Drag::Handle { .. } | Drag::Width { .. })) {
            self.live = None;
        }
        let shapes: Vec<VectorShape> = self.shapes(layer, frame, art).into_iter().cloned().collect();
        // Those gone are no longer shown, nor their points picked.
        if shapes.is_empty() {
            (self.shown, self.picked) = (None, Vec::new());
        } else if let Some(shown) = &mut self.shown {
            shown.shapes.retain(|id| shapes.iter().any(|shape| shape.id == *id));
            self.picked.retain(|(id, point)| shapes.iter().any(|shape| shape.id == *id && *point < path_of(shape).elements().len()));
        }
        let paths: Vec<(ShapeId, BezPath)> = shapes.iter().map(|shape| (shape.id, path_of(shape).clone())).collect();
        let path = |id: ShapeId| paths.iter().find(|(shape, _)| *shape == id).map(|(_, path)| path.clone());
        let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
        if response.drag_started_by(primary)
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            let shown: Vec<&VectorShape> = shapes.iter().collect();
            self.drag = if let Some(point) = self.width_at(&shown, map, origin) {
                let shape = shapes.iter().find(|shape| shape.id == point.0).expect("found among them");
                let lists = lists_of(shape).unwrap_or_default();
                let centre = points::points(path_of(shape)).get(point.1).copied().flatten().unwrap_or_default();
                let from = ((map.to_drawing)(origin) - centre).hypot();
                // With Shift the opacity, with Alt and Shift the drift (the user's of 2026-11-04).
                let which = match (shift, alt) {
                    (true, true) => 2,
                    (true, false) => 1,
                    _ => 0,
                };
                Some(Drag::Width { point, from, origin, which, was: lists.clone(), now: lists })
            } else if let Some((point, handle)) = self.handle_at(&paths, map, origin) {
                let was = path(point.0).unwrap_or_default();
                Some(Drag::Handle { point, handle, was: was.clone(), now: was })
            } else if let Some(point) = Self::point_at(&paths, map, origin) {
                Some(Drag::Points { point, from: (map.to_drawing)(origin), was: paths.clone(), now: paths.clone() })
            } else if self.lasso {
                Some(Drag::Lasso { way: vec![origin] })
            } else {
                Some(Drag::Box { from: origin, to: origin })
            };
        }
        if response.dragged_by(primary)
            && let Some(at) = response.interact_pointer_pos()
        {
            match &mut self.drag {
                Some(Drag::Points { point, from, was, now }) => {
                    // A point that isn't picked is, once it moves: alone, or with Shift with the others.
                    if !self.picked.contains(point) {
                        if !shift {
                            self.picked.clear();
                        }
                        self.picked.push(*point);
                    }
                    let by = (map.to_drawing)(at) - *from;
                    let picked = &self.picked;
                    *now = was
                        .iter()
                        .map(|(shape, path)| {
                            let which: Vec<usize> = picked.iter().filter(|(of, _)| of == shape).map(|(_, point)| *point).collect();
                            (*shape, if which.is_empty() { path.clone() } else { points::moved_points(path, &which, by) })
                        })
                        .collect();
                }
                Some(Drag::Handle { point, handle, was, now }) => {
                    let smooth = points::is_smooth(was, point.1) && !alt;
                    if let Some(moved) = points::moved_handle(was, point.1, *handle, (map.to_drawing)(at), smooth) {
                        *now = moved;
                    }
                }
                Some(Drag::Box { to, .. }) => *to = at,
                Some(Drag::Lasso { way }) => {
                    if way.last().is_none_or(|last| last.distance(at) >= 2.0) {
                        way.push(at);
                    }
                }
                Some(Drag::Width { point, origin, which, was, now, .. }) if *which > 0 => {
                    // From none to all across 200 points of the screen.
                    let points = was[0].len();
                    let value = (value_at(&was[*which], point.1, *which) + f64::from((at.x - origin.x) / 200.0)).clamp(0.0, 1.0);
                    *now = was.clone();
                    let of_it = self.picked.iter().filter(|(of, _)| *of == point.0).map(|(_, index)| *index);
                    for index in of_it.chain(std::iter::once(point.1)) {
                        set_value(&mut now[*which], points, index, *which, value);
                    }
                }
                Some(Drag::Width { point, from, was, now, .. }) => {
                    // Wider by twice as much as the pointer comes away from the point: its edge
                    // goes with the pointer.
                    if let Some(centre) = path(point.0).and_then(|path| points::points(&path).get(point.1).copied().flatten()) {
                        let by = 2.0 * (((map.to_drawing)(at) - centre).hypot() - *from);
                        let width = (was[0].get(point.1).copied().unwrap_or(0.0) + by).max(0.0);
                        *now = was.clone();
                        let of_it = self.picked.iter().filter(|(of, _)| *of == point.0).map(|(_, index)| *index);
                        for index in of_it.chain(std::iter::once(point.1)) {
                            if let Some(slot) = now[0].get_mut(index) {
                                *slot = width;
                            }
                        }
                    }
                }
                None => {}
            }
            // The drawing as the drag makes it, made anew only when it changes.
            let changes: Option<Vec<Change>> = match &self.drag {
                Some(Drag::Points { was, now, .. }) => {
                    Some(now.iter().zip(was).filter(|(now, was)| now.1 != was.1).map(|((shape, path), _)| (*shape, path.clone(), None)).collect())
                }
                Some(Drag::Handle { point, now, .. }) => Some(vec![(point.0, now.clone(), None)]),
                Some(Drag::Width { point, now, .. }) => path(point.0).map(|path| vec![(point.0, path, Some(now.clone()))]),
                _ => None,
            };
            if let Some(changes) = changes
                && self.live.as_ref().is_none_or(|(.., was, _)| *was != changes)
                && let Some(vector) = art_of(layer, frame, art)
            {
                let made = vector.edited(|all| {
                    for (id, path, lists) in &changes {
                        let Some(shape) = all.iter_mut().find(|shape| shape.id == *id) else { continue };
                        match &mut shape.kind {
                            ShapeKind::Stroke(stroke) => {
                                stroke.path = path.clone();
                                if let Some(lists) = lists {
                                    stroke.per_point_mut().into_iter().zip(lists).for_each(|(had, list)| had.clone_from(list));
                                }
                            }
                            ShapeKind::Fill(fill) => fill.outline = path.clone(),
                        }
                    }
                });
                self.live = Some((layer.id, frame, art, changes, made));
            }
        }
        if response.drag_stopped_by(primary) {
            match self.drag.take() {
                Some(Drag::Points { was, now, .. }) => {
                    let changes: Vec<Change> =
                        now.into_iter().zip(was).filter(|(now, was)| now.1 != was.1).map(|((shape, path), _)| (shape, path, None)).collect();
                    if !changes.is_empty() {
                        return Some(edit(layer.id, frame, art, &changes));
                    }
                }
                Some(Drag::Handle { point, was, now, .. }) if now != was => return Some(edit(layer.id, frame, art, &[(point.0, now, None)])),
                Some(Drag::Width { point, was, now, .. }) => {
                    // A press on a diamond that does not move picks its point, as on a point.
                    if !self.picked.contains(&point) {
                        self.picked = vec![point];
                    }
                    if let (true, Some(path)) = (now != was, path(point.0)) {
                        return Some(edit(layer.id, frame, art, &[(point.0, path, Some(now))]));
                    }
                }
                Some(Drag::Box { from, to }) if from.distance(to) >= crate::viewport::CLICK_SLOP => {
                    let area = Rect::from_two_pos(from, to);
                    self.pick_in(layer, frame, art, map, shift, |at| area.contains(at));
                }
                Some(Drag::Lasso { way }) if way.len() > 2 => self.pick_in(layer, frame, art, map, shift, |at| inside_way(&way, at)),
                _ => {}
            }
        }
        if let Some(at) = click {
            if let Some(point) = Self::point_at(&paths, map, at) {
                // With Alt, the point is taken out of its line.
                if alt {
                    let shape = shapes.iter().find(|shape| shape.id == point.0)?;
                    let (path, lists) = without_point(path_of(shape), &lists_of(shape).unwrap_or_default(), point.1)?;
                    self.picked.retain(|picked| picked.0 != point.0);
                    return Some(edit(layer.id, frame, art, &[(point.0, path, lists_of(shape).map(|_| lists))]));
                }
                // Picked, or with Shift added or taken away.
                match (shift, self.picked.iter().position(|picked| *picked == point)) {
                    (true, Some(found)) => _ = self.picked.remove(found),
                    (true, None) => self.picked.push(point),
                    (false, _) => self.picked = vec![point],
                }
                return None;
            }
            // A shown shape's line: a point put there.
            for shape in &shapes {
                let path = path_of(shape);
                let Some((index, t, _)) = points::nearest(path, (map.to_drawing)(at)) else { continue };
                let on = points::with_point(path, &[], index, t).map(|(with, _)| points::points(&with)[index]);
                if on.flatten().is_some_and(|put| (map.to_screen)(put).distance(at) <= REACH) {
                    let (with, lists) = with_point(path, &lists_of(shape).unwrap_or_default(), index, t)?;
                    self.picked = vec![(shape.id, index)];
                    return Some(edit(layer.id, frame, art, &[(shape.id, with, lists_of(shape).map(|_| lists))]));
                }
            }
            // Another shape: its points shown, with Shift beside those shown. The topmost that
            // is drawn there.
            let under = (map.to_drawing)(at);
            let vector = art_of(layer, frame, art)?;
            let found = vector.shapes().iter().rev().find(|shape| shape.kind.area().winding(under) != 0).map(|shape| shape.id);
            if !shift {
                self.picked.clear();
            }
            self.show(layer.id, frame, art, found.into_iter().collect(), shift);
        }
        None
    }

    /// Pick by a box or a lasso, `inside` saying what of the screen it holds: the strokes it
    /// touches, or the points inside it of the shapes shown, or with none of them there of any
    /// shape. With `more`, beside what was picked.
    fn pick_in(&mut self, layer: &Layer, frame: u32, art: ArtLayerKind, map: &Map, more: bool, inside: impl Fn(Pos2) -> bool) {
        let Some(vector) = art_of(layer, frame, art) else { return };
        if !more {
            self.picked.clear();
        }
        if self.picks_strokes {
            // Along the line, a few points apart on screen.
            let touches = |shape: &VectorShape| {
                let (mut touched, mut last) = (false, None::<Pos2>);
                iskra_model::kurbo::flatten(path_of(shape), 0.5, |element| match element {
                    PathEl::MoveTo(at) => {
                        let at = (map.to_screen)(at);
                        touched |= inside(at);
                        last = Some(at);
                    }
                    PathEl::LineTo(at) => {
                        let at = (map.to_screen)(at);
                        if let Some(from) = last {
                            let steps = (from.distance(at) / 3.0).ceil().max(1.0) as usize;
                            touched |= (1..=steps).any(|step| inside(from + (at - from) * (step as f32 / steps as f32)));
                        }
                        last = Some(at);
                    }
                    _ => {}
                });
                touched
            };
            let touched = vector.shapes().iter().filter(|shape| touches(shape)).map(|shape| shape.id).collect();
            self.show(layer.id, frame, art, touched, more);
            return;
        }
        let points_in = |shape: &VectorShape| -> Vec<Pick> {
            let all = points::points(path_of(shape)).into_iter().enumerate();
            all.filter(|(_, point)| point.is_some_and(|point| inside((map.to_screen)(point)))).map(|(index, _)| (shape.id, index)).collect()
        };
        let shown = self.shapes(layer, frame, art);
        let mut found: Vec<Pick> = shown.iter().flat_map(|shape| points_in(shape)).collect();
        if found.is_empty() {
            found = vector.shapes().iter().flat_map(points_in).collect();
        }
        let of: Vec<ShapeId> = found.iter().map(|(shape, _)| *shape).collect();
        self.show(layer.id, frame, art, of, true);
        self.picked.extend(found.into_iter().filter(|pick| !self.picked.contains(pick)).collect::<Vec<_>>());
    }

    /// The art layer of `layer` on `frame` as a drag of a point or a handle makes it, to be drawn
    /// in place of the drawing's while it is dragged.
    pub fn live(&self, layer: LayerId, frame: u32) -> Option<(ArtLayerKind, &VectorArt)> {
        let (_, _, art, _, made) = self.live.as_ref().filter(|(of, at, ..)| *of == layer && *at == frame)?;
        Some((*art, made))
    }

    /// The points and handles of the shapes shown, over the drawing, and a box or a lasso being
    /// drawn.
    pub fn paint(&mut self, painter: &egui::Painter, layer: &Layer, frame: u32, art: ArtLayerKind, map: &Map, accent: Color32) {
        match &self.drag {
            Some(Drag::Box { from, to }) => {
                painter.rect_stroke(Rect::from_two_pos(*from, *to), 0.0, Outline::new(1.0, accent), egui::StrokeKind::Inside);
            }
            Some(Drag::Lasso { way }) if way.len() > 1 => {
                painter.add(egui::Shape::closed_line(way.clone(), Outline::new(1.0, accent)));
            }
            _ => {}
        }
        #[cfg(test)]
        {
            self.drawn_handles.clear();
            self.drawn_diamonds.clear();
        }
        let shapes: Vec<VectorShape> = self.shapes(layer, frame, art).into_iter().cloned().collect();
        for shape in &shapes {
            self.paint_shape(painter, shape, map, accent);
        }
    }

    /// The points and handles of one shape shown.
    fn paint_shape(&mut self, painter: &egui::Painter, shape: &VectorShape, map: &Map, accent: Color32) {
        // While a point or a handle is dragged, the path as it will be.
        let path = match &self.drag {
            Some(Drag::Points { now, .. }) => now.iter().find(|(id, _)| *id == shape.id).map_or(path_of(shape), |(_, path)| path),
            Some(Drag::Handle { point, now, .. }) if point.0 == shape.id => now,
            _ => path_of(shape),
        };
        let widths = match (&self.drag, widths_of(shape)) {
            (Some(Drag::Width { point, now, .. }), _) if point.0 == shape.id => Some(&now[0]),
            (_, widths) => widths,
        };
        let mut line = Vec::new();
        iskra_model::kurbo::flatten(path, 0.5, |element| match element {
            PathEl::MoveTo(at) => {
                if line.len() > 1 {
                    painter.add(egui::Shape::line(std::mem::take(&mut line), Outline::new(1.0, accent)));
                }
                line = vec![(map.to_screen)(at)];
            }
            PathEl::LineTo(at) => line.push((map.to_screen)(at)),
            PathEl::ClosePath => {
                if let Some(first) = line.first().copied() {
                    line.push(first);
                }
            }
            _ => {}
        });
        if line.len() > 1 {
            painter.add(egui::Shape::line(line, Outline::new(1.0, accent)));
        }
        for (index, point) in points::points(path).into_iter().enumerate() {
            let Some(point) = point else { continue };
            let at = (map.to_screen)(point);
            let picked = self.picked.contains(&(shape.id, index));
            // A point's handles, each on a line from it: a picked point's strong, the others'
            // faint.
            if picked || !self.picked_handles_only {
                let (into, out) = points::handles(path, index);
                let strength = if picked { 0.7 } else { 0.35 };
                for handle in [into, out].into_iter().flatten() {
                    let end = (map.to_screen)(handle);
                    painter.line_segment([at, end], Outline::new(1.0, accent.gamma_multiply(strength)));
                    let fill = if picked { Color32::WHITE } else { Color32::WHITE.gamma_multiply(0.6) };
                    painter.circle(end, if picked { 3.0 } else { 2.5 }, fill, Outline::new(1.0, accent.gamma_multiply(strength + 0.3)));
                    #[cfg(test)]
                    self.drawn_handles.push(((shape.id, index), end));
                }
            }
            // A picked point of a stroke: its width's diamond on the stroke's edge.
            if let (true, Some(end)) = (picked, widths.and_then(|widths| diamond_on_screen(path, widths, index, map))) {
                painter.line_segment([at, end], Outline::new(1.0, accent.gamma_multiply(0.4)));
                let corners = [end + egui::vec2(0.0, -4.5), end + egui::vec2(4.5, 0.0), end + egui::vec2(0.0, 4.5), end + egui::vec2(-4.5, 0.0)];
                painter.add(egui::Shape::convex_polygon(corners.to_vec(), Color32::WHITE, Outline::new(1.0, accent)));
                #[cfg(test)]
                self.drawn_diamonds.push(((shape.id, index), end));
            }
            let square = Rect::from_center_size(at, egui::vec2(7.0, 7.0));
            painter.rect(square, 0.0, if picked { accent } else { Color32::WHITE }, Outline::new(1.0, accent), egui::StrokeKind::Inside);
        }
    }

    /// The shapes shown that have picked points, each with those points.
    fn picked_shapes<'a>(&self, layer: &'a Layer, frame: u32, art: ArtLayerKind) -> Vec<(&'a VectorShape, Vec<usize>)> {
        let shapes = self.shapes(layer, frame, art);
        shapes.into_iter().map(|shape| (shape, self.picked_of(shape.id))).filter(|(_, picked)| !picked.is_empty()).collect()
    }

    /// The command that takes the picked points away, as Delete does; `None` if none are picked,
    /// or a line would be left with fewer than two points.
    pub fn delete(&mut self, layer: &Layer, frame: u32, art: ArtLayerKind) -> Option<(&'static str, Value)> {
        let mut changes = Vec::new();
        for (shape, mut picked) in self.picked_shapes(layer, frame, art) {
            picked.sort_unstable_by(|a, b| b.cmp(a));
            let (mut path, mut lists) = (path_of(shape).clone(), lists_of(shape).unwrap_or_default());
            for point in picked {
                (path, lists) = without_point(&path, &lists, point)?;
            }
            changes.push((shape.id, path, lists_of(shape).map(|_| lists)));
        }
        (!changes.is_empty()).then(|| {
            self.picked.clear();
            edit(layer.id, frame, art, &changes)
        })
    }

    /// The command that puts a point halfway along the line after each picked point, or before
    /// it at a line's end; the new points are picked. `None` if none are picked.
    pub fn add_points(&mut self, layer: &Layer, frame: u32, art: ArtLayerKind) -> Option<(&'static str, Value)> {
        let mut changes = Vec::new();
        let mut picked = Vec::new();
        for (shape, mut points) in self.picked_shapes(layer, frame, art) {
            // From the last, so that the points before keep their places.
            points.sort_unstable_by(|a, b| b.cmp(a));
            let (mut path, mut lists) = (path_of(shape).clone(), lists_of(shape).unwrap_or_default());
            let mut put = Vec::new();
            for point in points {
                let after = matches!(path.elements().get(point + 1), Some(PathEl::LineTo(_) | PathEl::CurveTo(..) | PathEl::QuadTo(..)));
                let at = if after { point + 1 } else { point };
                let Some((with, now)) = with_point(&path, &lists, at, 0.5) else { continue };
                (path, lists) = (with, now);
                put.iter_mut().for_each(|index: &mut usize| *index += 1);
                put.push(at);
            }
            if !put.is_empty() {
                picked.extend(put.into_iter().map(|index| (shape.id, index)));
                changes.push((shape.id, path, lists_of(shape).map(|_| lists)));
            }
        }
        (!changes.is_empty()).then(|| {
            self.picked = picked;
            edit(layer.id, frame, art, &changes)
        })
    }

    /// The command that makes the picked points smooth, or corners.
    pub fn smooth(&self, layer: &Layer, frame: u32, art: ArtLayerKind, smooth: bool) -> Option<(&'static str, Value)> {
        let mut changes = Vec::new();
        for (shape, picked) in self.picked_shapes(layer, frame, art) {
            let mut path = points::cubic(path_of(shape));
            for point in picked {
                path = points::smoothed(&path, point, smooth)?;
            }
            changes.push((shape.id, path, None));
        }
        (!changes.is_empty()).then(|| edit(layer.id, frame, art, &changes))
    }

    /// The width of a stroke at its first picked point, if a stroke's point is picked.
    pub fn width(&self, layer: &Layer, frame: u32, art: ArtLayerKind) -> Option<f64> {
        self.value(layer, frame, art, 0)
    }

    /// The command that gives the strokes the width `width` at every picked point.
    pub fn set_width(&self, layer: &Layer, frame: u32, art: ArtLayerKind, width: f64) -> Option<(&'static str, Value)> {
        self.set_value(layer, frame, art, 0, width.max(0.0))
    }

    /// One of a stroke's per-point values at its first picked point (0 the width, 1 the
    /// opacity, 2 the drift), if a stroke's point is picked.
    pub fn value(&self, layer: &Layer, frame: u32, art: ArtLayerKind, which: usize) -> Option<f64> {
        let shapes = self.shapes(layer, frame, art);
        self.picked.iter().find_map(|(id, point)| {
            let lists = lists_of(shapes.iter().find(|shape| shape.id == *id)?)?;
            (*point < lists[0].len()).then(|| value_at(&lists[which], *point, which))
        })
    }

    /// The command that gives the strokes the value `value` of `which` at every picked point.
    pub fn set_value(&self, layer: &Layer, frame: u32, art: ArtLayerKind, which: usize, value: f64) -> Option<(&'static str, Value)> {
        let mut changes = Vec::new();
        for (shape, picked) in self.picked_shapes(layer, frame, art) {
            let ShapeKind::Stroke(stroke) = &shape.kind else { continue };
            let mut lists = lists_of(shape)?;
            let points = lists[0].len();
            for point in picked {
                if point >= points {
                    return None;
                }
                set_value(&mut lists[which], points, point, which, value);
            }
            changes.push((shape.id, stroke.path.clone(), Some(lists)));
        }
        (!changes.is_empty()).then(|| edit(layer.id, frame, art, &changes))
    }

    /// Whether every picked point is smooth.
    pub fn all_smooth(&self, layer: &Layer, frame: u32, art: ArtLayerKind) -> bool {
        let picked = self.picked_shapes(layer, frame, art);
        !picked.is_empty() && picked.iter().all(|(shape, points)| points.iter().all(|point| points::is_smooth(path_of(shape), *point)))
    }
}

/// Where the diamond of a stroke's width at its point `index` is, in the drawing's pixels: on
/// the stroke's edge, half the width out from the point, square to the way the line goes.
pub fn width_handle(path: &BezPath, widths: &[f64], index: usize) -> Option<Point> {
    let all = points::points(path);
    let at = (*all.get(index)?)?;
    let elements = path.elements();
    let (into, out) = points::handles(path, index);
    let starts = matches!(elements.get(index), Some(PathEl::MoveTo(_)));
    let before = into.or_else(|| if starts { None } else { all[..index].iter().rev().flatten().next().copied() });
    let next_starts = matches!(elements.get(index + 1), Some(PathEl::MoveTo(_)) | None);
    let after = out.or_else(|| if next_starts { None } else { all.get(index + 1).copied().flatten() });
    let way = match (before, after) {
        (Some(before), Some(after)) => after - before,
        (Some(before), None) => at - before,
        (None, Some(after)) => after - at,
        (None, None) => return None,
    };
    if way.hypot() < 1e-9 {
        return None;
    }
    let way = way.normalize();
    Some(at + Vec2::new(-way.y, way.x) * (widths.get(index).copied().unwrap_or(0.0) / 2.0).max(1.0))
}

/// How far from its point a width's diamond is at least, on screen, in points: clear of the
/// point, which is taken hold of before it.
const DIAMOND_CLEAR: f32 = 12.0;

/// Where the diamond of the width at the point `index` is drawn, on screen: on the stroke's edge
/// ([`width_handle`]), or where that is too near the point, further out the same way.
fn diamond_on_screen(path: &BezPath, widths: &[f64], index: usize, map: &Map) -> Option<Pos2> {
    let point = (map.to_screen)((*points::points(path).get(index)?)?);
    let edge = (map.to_screen)(width_handle(path, widths, index)?);
    let out = edge - point;
    if out.length() >= DIAMOND_CLEAR {
        return Some(edge);
    }
    (out.length() > 1e-3).then(|| point + out.normalized() * DIAMOND_CLEAR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_is_inside_a_lasso_by_its_crossings() {
        let way = [Pos2::new(0.0, 0.0), Pos2::new(100.0, 0.0), Pos2::new(100.0, 100.0), Pos2::new(0.0, 100.0)];
        assert!(inside_way(&way, Pos2::new(50.0, 50.0)) && !inside_way(&way, Pos2::new(150.0, 50.0)));
        // A C: its mouth is not inside.
        let c = [
            Pos2::new(0.0, 0.0),
            Pos2::new(100.0, 0.0),
            Pos2::new(100.0, 30.0),
            Pos2::new(30.0, 30.0),
            Pos2::new(30.0, 70.0),
            Pos2::new(100.0, 70.0),
            Pos2::new(100.0, 100.0),
            Pos2::new(0.0, 100.0),
        ];
        assert!(inside_way(&c, Pos2::new(15.0, 50.0)) && !inside_way(&c, Pos2::new(60.0, 50.0)));
    }
}
