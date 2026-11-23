//! The Gradient/Texture tool (plan §4.20, step 4): where a gradient or a picture lies on the
//! filled areas of the drawing in hand.
//!
//! A click on an area painted with a gradient or a picture shows where it lies, as the Select
//! tool shows a selection: a box with a square at each corner and side, which scale it, a
//! handle above, which turns it, as does a drag just outside a corner, and sides that skew it
//! when dragged between their squares. A drag inside the box, or of the pivot at its middle,
//! moves it. A linear gradient shows its line, from its start to its end, each dragged on its
//! own; a radial one its ring and its focal point. Areas of the same swatch are taken together
//! by dragging a marquee across them, or by Shift and a click, and lie as one. Each drag is
//! kept when it is let go, found again by the points of the areas. A click elsewhere lets them
//! go.

use egui::{Color32, Pos2, Response, Shape, Stroke};
use iskra_drawing::{Affine, Point};
use iskra_model::{ArtLayerKind, Frame, GradientShape, Layer, LayerId, Palette, Rect, Shade, SwatchId};
use serde_json::{Value, json};

use crate::points::Map;
use crate::select::{self, Handle};
use crate::viewport::{HANDLE_REACH, ROTATE_HANDLE_OFFSET};

/// What a gradient or a picture is, for its handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Linear,
    Radial,
    Picture,
}

impl Kind {
    fn of(shade: &Shade) -> Option<Kind> {
        match shade {
            Shade::Gradient(gradient) if gradient.shape == GradientShape::Radial => Some(Kind::Radial),
            Shade::Gradient(_) => Some(Kind::Linear),
            Shade::Texture { picture: Some(_), .. } => Some(Kind::Picture),
            _ => None,
        }
    }

    /// Its box, in the frame's own measure: a linear gradient's about its line, a radial
    /// one's about its ring, a picture's all of it.
    fn rect(self) -> [f64; 4] {
        match self {
            Kind::Linear => [0.0, -0.25, 1.0, 0.25],
            Kind::Radial => [-1.0, -1.0, 1.0, 1.0],
            Kind::Picture => [0.0, 0.0, 1.0, 1.0],
        }
    }
}

/// What a press takes of the areas shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grab {
    /// One of the box's, as the Select tool's.
    Box(Handle),
    /// A linear gradient's start or end, moved alone.
    Start,
    End,
    /// A radial gradient's focal point.
    Focus,
}

/// The areas shown: of which layer's drawing on which frame, of which art layer, a pixel of
/// each, their swatch, and where its gradient or picture lies on them, in the drawing's pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub layer: LayerId,
    pub frame: u32,
    pub art: ArtLayerKind,
    pub at: Vec<[i32; 2]>,
    pub swatch: SwatchId,
    pub kind: Kind,
    pub place: Frame,
    /// Whether the drawing holds a placement for any of them, or they are fitted.
    pub placed: bool,
}

impl Shown {
    /// The frame as the transform from its own measure to the drawing's.
    fn affine(&self) -> Affine {
        let Frame { start, end, across, .. } = self.place;
        let [s, e, a] = [start, end, across].map(|p| [f64::from(p[0]), f64::from(p[1])]);
        Affine { m: [e[0] - s[0], e[1] - s[1], a[0] - s[0], a[1] - s[1], s[0], s[1]] }
    }

    /// The frame `affine` makes, its focal point kept.
    fn set(&mut self, affine: &Affine) {
        let at = |x: f64, y: f64| {
            let p = affine.apply(Point::new(x, y));
            [p.x as f32, p.y as f32]
        };
        (self.place.start, self.place.end, self.place.across) = (at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0));
    }

    /// The box's corners, clockwise from the top left, and its middle, in the drawing.
    fn corners(&self) -> ([Point; 4], Point) {
        let ([x0, y0, x1, y1], affine) = (self.kind.rect(), self.affine());
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| affine.apply(Point::new(x, y)));
        (corners, affine.apply(Point::new((x0 + x1) / 2.0, (y0 + y1) / 2.0)))
    }

    /// What lays its gradient or picture where it lies now, on each of its areas: one step.
    fn command(&self) -> Option<(&'static str, Value)> {
        let art = serde_json::to_value(self.art).ok()?;
        let Frame { start, end, across, focus } = self.place;
        let each = |[x, y]: [i32; 2]| json!({ "layer": self.layer.0, "frame": self.frame, "x": x, "y": y, "artLayer": art, "start": start, "end": end, "across": across, "focus": focus });
        Some(one_step(self.at.iter().map(|at| each(*at)).collect(), "Place Gradient"))
    }
}

/// `params` for `drawing.place` as one command, or as one step of several.
fn one_step(mut params: Vec<Value>, title: &str) -> (&'static str, Value) {
    if params.len() == 1 {
        return ("drawing.place", params.remove(0));
    }
    let commands: Vec<Value> = params.into_iter().map(|params| json!({ "id": "drawing.place", "params": params })).collect();
    ("edit.batch", json!({ "commands": commands, "title": title }))
}

/// A press being dragged: what it took, where it was in the drawing, and the frame then.
struct Drag {
    grab: Grab,
    from: Point,
    was: Shown,
}

#[derive(Default)]
pub struct Placing {
    pub shown: Option<Shown>,
    drag: Option<Drag>,
    /// A marquee being dragged out, in the drawing: where it started and where it is.
    marquee: Option<(Point, Point)>,
    /// What a press where the pointer is would take, and which way a turn's arrow bulges.
    hover: Option<(Pos2, Grab)>,
    facing: f32,
    /// The drawing and the palette the shown areas were found in, by their addresses: they
    /// are found again when either is another.
    of: (usize, usize),
    /// Where the handles were drawn last, on screen: a linear gradient's start and end, and
    /// the box's corners.
    #[cfg(test)]
    pub handles: Option<(Pos2, Pos2)>,
    #[cfg(test)]
    pub corners: Option<[Pos2; 4]>,
}

/// A point of the drawing, as `map` takes it.
fn kurbo(p: Point) -> iskra_model::kurbo::Point {
    iskra_model::kurbo::Point::new(p.x, p.y)
}

fn drawn(at: [f32; 2]) -> Point {
    Point::new(f64::from(at[0]), f64::from(at[1]))
}

impl Placing {
    /// The areas of `layer`'s drawing on `frame` that the pixels `at` are in, where `palette`
    /// paints them with a gradient or a picture: of the top art layer that has one at the
    /// first, and of the first's swatch.
    fn areas_at(layer: &Layer, frame: u32, palette: &Palette, at: &[[i32; 2]]) -> Option<Shown> {
        let drawing = &layer.drawing_at(frame)?.drawing;
        let areas = drawing.areas(|swatch| palette.shade(swatch));
        let first = *at.first()?;
        let (art, area) = ArtLayerKind::ALL.into_iter().rev().find_map(|art| Some((art, *areas.at(art, first[0], first[1])?)))?;
        let kind = Kind::of(&palette.shade(area.swatch))?;
        let mut found = vec![area];
        for at in &at[1..] {
            if let Some(more) = areas.at(art, at[0], at[1]).filter(|more| more.swatch == area.swatch && !found.contains(more)) {
                found.push(*more);
            }
        }
        // Taken together, they lie as the first of them that has a placement does, or are
        // fitted as one, across them all.
        let place = match found.iter().find(|area| area.placed) {
            _ if found.len() == 1 => area.frame,
            Some(placed) => placed.frame,
            None => {
                palette.shade(area.swatch).fitted(found.iter().fold(Rect::new(i32::MAX, i32::MAX, i32::MIN, i32::MIN), |all, area| all.union(&area.bounds)))
            }
        };
        let placed = found.iter().any(|area| area.placed);
        let at = found.iter().map(|area| area.inside).collect();
        Some(Shown { layer: layer.id, frame, art, at, swatch: area.swatch, kind, place, placed })
    }

    /// The areas of the swatch first found in `rect` of `layer`'s drawing on `frame` that
    /// reach into it, of the top art layer that has one there.
    fn areas_in(layer: &Layer, frame: u32, palette: &Palette, rect: Rect) -> Option<Shown> {
        let drawing = &layer.drawing_at(frame)?.drawing;
        let areas = drawing.areas(|swatch| palette.shade(swatch));
        let inside = |area: &iskra_model::Area| !area.bounds.intersect(&rect).is_empty();
        let art = ArtLayerKind::ALL.into_iter().rev().find(|art| areas.all().any(|(of, area)| of == *art && inside(area)))?;
        let first = areas.all().find(|(of, area)| *of == art && inside(area))?.1.swatch;
        let at: Vec<[i32; 2]> = areas.all().filter(|(of, area)| *of == art && area.swatch == first && inside(area)).map(|(_, area)| area.inside).collect();
        Self::areas_at(layer, frame, palette, &at)
    }

    /// What a press at `pos` on screen takes of the areas shown: a gradient's ends or focal
    /// point first, then the box's handles as the Select tool's, then inside it, a move.
    fn grab_at(shown: &Shown, map: &Map, pos: Pos2) -> Option<Grab> {
        let screen = |p: Point| (map.to_screen)(kurbo(p));
        let near = |at: Point| (screen(at) - pos).length() <= HANDLE_REACH;
        let affine = shown.affine();
        match shown.kind {
            Kind::Linear if near(drawn(shown.place.end)) => return Some(Grab::End),
            Kind::Linear if near(drawn(shown.place.start)) => return Some(Grab::Start),
            Kind::Radial if near(affine.apply(drawn(Frame::inside_ring(shown.place.focus)))) => return Some(Grab::Focus),
            _ => {}
        }
        let (corners, middle) = shown.corners();
        let corners = corners.map(screen);
        let (top, outward) = Self::top(&corners, screen(middle));
        let handle = corners
            .iter()
            .position(|c| (*c - pos).length() <= HANDLE_REACH)
            .map(Handle::Corner)
            .or_else(|| crate::viewport::sides(&corners).iter().position(|c| (*c - pos).length() <= HANDLE_REACH).map(Handle::Side))
            .or(((top + outward * ROTATE_HANDLE_OFFSET - pos).length() <= HANDLE_REACH).then_some(Handle::Rotate))
            .or_else(|| crate::viewport::turn_or_skew(&corners, pos, true));
        if let Some(handle) = handle {
            return Some(Grab::Box(handle));
        }
        let [x0, y0, x1, y1] = shown.kind.rect();
        let at = (map.to_drawing)(pos);
        let local = affine.inverse()?.apply(Point::new(at.x, at.y));
        (local.x >= x0.min(x1) && local.x <= x0.max(x1) && local.y >= y0.min(y1) && local.y <= y0.max(y1)).then_some(Grab::Box(Handle::Move))
    }

    /// The middle of the box's top on screen, and the way out of the box from there.
    fn top(corners: &[Pos2; 4], middle: Pos2) -> (Pos2, egui::Vec2) {
        let top = corners[0] + (corners[1] - corners[0]) / 2.0;
        (top, (top - middle).normalized())
    }

    /// The areas shown with `grab` dragged from `from` to `now`, in the drawing.
    fn dragged(was: &Shown, grab: Grab, from: Point, now: Point) -> Shown {
        let mut shown = was.clone();
        let affine = was.affine();
        match grab {
            Grab::Box(handle @ (Handle::Move | Handle::Rotate)) => {
                let (_, c) = was.corners();
                shown.set(&affine.then(&select::drag_matrix_in(handle, [c.x, c.y, c.x, c.y], from, now)));
            }
            Grab::Box(handle) => {
                let Some(back) = affine.inverse() else { return shown };
                let local = select::drag_matrix_in(handle, was.kind.rect(), back.apply(from), back.apply(now));
                shown.set(&local.then(&affine));
            }
            // An end moved alone: the other side turns and grows with the line, so that the
            // box keeps its shape.
            Grab::Start | Grab::End => {
                let Frame { start, end, across, .. } = was.place;
                let at = [now.x as f32, now.y as f32];
                let (new_start, new_end) = if grab == Grab::Start { (at, end) } else { (start, at) };
                let (x, nx, y) = (
                    [end[0] - start[0], end[1] - start[1]],
                    [new_end[0] - new_start[0], new_end[1] - new_start[1]],
                    [across[0] - start[0], across[1] - start[1]],
                );
                let length = (x[0] * x[0] + x[1] * x[1]).max(f32::EPSILON);
                // The turn and the growth from the line to the new, as a complex number.
                let k = [(nx[0] * x[0] + nx[1] * x[1]) / length, (nx[1] * x[0] - nx[0] * x[1]) / length];
                let y = [k[0] * y[0] - k[1] * y[1], k[0] * y[1] + k[1] * y[0]];
                (shown.place.start, shown.place.end, shown.place.across) = (new_start, new_end, [new_start[0] + y[0], new_start[1] + y[1]]);
            }
            Grab::Focus => {
                let local = was.place.local([now.x as f32, now.y as f32]);
                shown.place.focus = Frame::inside_ring(local);
            }
        }
        shown
    }

    /// Take the pointer: a `click` shows the areas under it (with Shift, adds to them), a drag
    /// of a handle changes where they lie, a drag elsewhere is a marquee that takes the areas
    /// it reaches; what a drag changed is asked for when it is let go.
    #[allow(clippy::too_many_arguments)]
    pub fn input(
        &mut self,
        response: &Response,
        click: Option<Pos2>,
        layer: &Layer,
        frame: u32,
        palette: &std::sync::Arc<Palette>,
        map: &Map,
        panning: bool,
    ) -> Option<(&'static str, Value)> {
        // The areas shown are of the drawing in hand as it is now: after a change, an undo or
        // a step of the playhead they are found again, or let go.
        let drawing = layer.drawing_at(frame).map_or(0, |entry| std::sync::Arc::as_ptr(&entry.drawing) as usize);
        let of = (drawing, std::sync::Arc::as_ptr(palette) as usize);
        if self.drag.is_none() && (self.of != of || self.shown.as_ref().is_some_and(|shown| shown.layer != layer.id || shown.frame != frame)) {
            self.of = of;
            self.shown = self.shown.as_ref().filter(|shown| shown.layer == layer.id).and_then(|shown| Self::areas_at(layer, frame, palette, &shown.at));
        }
        if panning {
            (self.drag, self.marquee, self.hover) = (None, None, None);
            return None;
        }
        let in_drawing = |pos: Pos2| {
            let p = (map.to_drawing)(pos);
            Point::new(p.x, p.y)
        };
        // The pointer says what a press would take: a turn, a skew, a move.
        let pointed = response.hover_pos().filter(|_| self.marquee.is_none());
        let hovered =
            self.drag.as_ref().map(|drag| drag.grab).or_else(|| pointed.zip(self.shown.as_ref()).and_then(|(pos, shown)| Self::grab_at(shown, map, pos)));
        self.hover = pointed.zip(hovered);
        if let (Some(pos), Some(Grab::Box(Handle::Rotate)), Some(shown)) = (pointed, hovered, &self.shown) {
            let screen = |p: Point| (map.to_screen)(kurbo(p));
            let (corners, _) = shown.corners();
            self.facing = crate::viewport::facing(&corners.map(screen), pos, None);
        }
        crate::viewport::set_handle_cursor(&response.ctx, hovered.and_then(|grab| if let Grab::Box(handle) = grab { Some(handle) } else { None }));
        let pointer = response.interact_pointer_pos();
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pressed) = response.interact_pointer_pos()
        {
            let from = in_drawing(pressed);
            match self.shown.as_ref().and_then(|shown| Self::grab_at(shown, map, pressed).map(|grab| (grab, shown))) {
                Some((grab, shown)) => self.drag = Some(Drag { grab, from, was: shown.clone() }),
                None => self.marquee = Some((from, from)),
            }
        }
        if let (Some(drag), Some(pointer)) = (&self.drag, pointer) {
            self.shown = Some(Self::dragged(&drag.was, drag.grab, drag.from, in_drawing(pointer)));
        }
        if let (Some((_, to)), Some(pointer)) = (&mut self.marquee, pointer) {
            *to = in_drawing(pointer);
        }
        if !response.dragged() {
            // A marquee of under two pixels a side is a click.
            if let Some(rect) = self.marquee.take().and_then(|(from, to)| select::marquee(from, to)) {
                self.shown = Self::areas_in(layer, frame, palette, rect);
                return None;
            }
            if let Some(drag) = self.drag.take() {
                let shown = self.shown.as_ref().filter(|shown| **shown != drag.was)?;
                return shown.command();
            }
        }
        if let Some(click) = click.filter(|_| self.drag.is_none()) {
            let p = in_drawing(click);
            let at = [p.x.floor() as i32, p.y.floor() as i32];
            // With Shift, the area clicked is added to those shown, if it is of their swatch.
            let adding = response.ctx.input(|i| i.modifiers.shift);
            self.shown = match self.shown.as_ref().filter(|_| adding) {
                Some(shown) => {
                    let more: Vec<[i32; 2]> = shown.at.iter().copied().chain([at]).collect();
                    Self::areas_at(layer, frame, palette, &more).or(self.shown.clone())
                }
                None => Self::areas_at(layer, frame, palette, &[at]),
            };
        }
        None
    }

    /// The areas shown, with `matrix` (a flip or a quarter turn, `[a, b, c, d]`) done about the
    /// middle of their box; what keeps it.
    pub fn turned(&mut self, matrix: [f64; 4]) -> Option<(&'static str, Value)> {
        let shown = self.shown.as_mut()?;
        let (_, c) = shown.corners();
        let [a, b, cc, d] = matrix;
        let about = Affine::translate(-c.x, -c.y).then(&Affine { m: [a, b, cc, d, 0.0, 0.0] }).then(&Affine::translate(c.x, c.y));
        let affine = shown.affine().then(&about);
        shown.set(&affine);
        shown.command()
    }

    /// The areas shown, their box and handles, as the Select tool draws a selection's; a
    /// linear gradient's line and ends in `accent`, a radial one's ring and focal point.
    pub fn paint(&mut self, painter: &egui::Painter, map: &Map, accent: Color32) {
        let screen = |p: Point| (map.to_screen)(kurbo(p));
        let line = Stroke::new(1.0, Color32::from_gray(30));
        if let Some((from, to)) = self.marquee {
            let corners = [from, Point::new(to.x, from.y), to, Point::new(from.x, to.y)].map(screen);
            painter.add(Shape::closed_line(corners.to_vec(), line));
        }
        let Some(shown) = &self.shown else { return };
        let affine = shown.affine();
        let (corners, middle) = shown.corners();
        let (corners, middle) = (corners.map(screen), screen(middle));
        painter.add(Shape::closed_line(corners.to_vec(), line));
        let (top, outward) = Self::top(&corners, middle);
        let rotate = top + outward * ROTATE_HANDLE_OFFSET;
        painter.line_segment([top, rotate], line);
        painter.circle(rotate, 4.5, Color32::WHITE, line);
        for at in corners.into_iter().chain(crate::viewport::sides(&corners)) {
            painter.rect(egui::Rect::from_center_size(at, egui::vec2(7.0, 7.0)), 0.0, Color32::WHITE, line, egui::StrokeKind::Middle);
        }
        // The pivot in the middle, which moves it.
        let under = Stroke::new(3.0, Color32::from_black_alpha(140));
        painter.circle(middle, 4.0, Color32::TRANSPARENT, under);
        painter.circle(middle, 4.0, Color32::TRANSPARENT, Stroke::new(1.5, accent));
        let dot =
            |at: Pos2, filled: bool| painter.circle(at, 5.5, if filled { accent } else { Color32::WHITE }, Stroke::new(1.5, Color32::from_black_alpha(180)));
        #[cfg(test)]
        {
            self.corners = Some(corners);
        }
        match shown.kind {
            Kind::Linear => {
                let (start, end) = (screen(drawn(shown.place.start)), screen(drawn(shown.place.end)));
                #[cfg(test)]
                {
                    self.handles = Some((start, end));
                }
                painter.line_segment([start, end], under);
                painter.line_segment([start, end], Stroke::new(1.5, accent));
                dot(start, true);
                dot(end, false);
            }
            Kind::Radial => {
                let ring: Vec<Pos2> = (0..48)
                    .map(|i| {
                        let angle = i as f64 / 48.0 * std::f64::consts::TAU;
                        screen(affine.apply(Point::new(angle.cos(), angle.sin())))
                    })
                    .collect();
                painter.add(Shape::closed_line(ring.clone(), under));
                painter.add(Shape::closed_line(ring, Stroke::new(1.5, accent)));
                let focus = screen(affine.apply(drawn(Frame::inside_ring(shown.place.focus))));
                // The focal point as a diamond, apart from the round handles.
                let r = 5.5;
                let diamond = vec![focus + egui::vec2(0.0, -r), focus + egui::vec2(r, 0.0), focus + egui::vec2(0.0, r), focus + egui::vec2(-r, 0.0)];
                painter.add(Shape::convex_polygon(diamond, accent, Stroke::new(1.5, Color32::from_black_alpha(180))));
            }
            Kind::Picture => {}
        }
        // Where a press turns it, the pointer is a curved arrow.
        if let Some((at, Grab::Box(Handle::Rotate))) = self.hover {
            crate::viewport::paint_turn_cursor(painter, at, self.facing);
        }
    }

    /// What lets the shown areas' placements go, so that their swatch is fitted to each
    /// again: one step.
    pub fn reset(&self) -> Option<(&'static str, Value)> {
        let shown = self.shown.as_ref().filter(|shown| shown.placed)?;
        let art = serde_json::to_value(shown.art).ok()?;
        let each = |[x, y]: [i32; 2]| json!({ "layer": shown.layer.0, "frame": shown.frame, "x": x, "y": y, "artLayer": art, "reset": true });
        Some(one_step(shown.at.iter().map(|at| each(*at)).collect(), "Reset Placement"))
    }
}
