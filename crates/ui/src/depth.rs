//! The top and side views (plan §4.4): the scene seen from above, or from the side, to set up
//! depth. A drawing is a flat card, so it is a line here, as wide (from above) or as high
//! (from the side) as what is painted on it, at the depth its pegs give it. The camera is
//! shown with what it sees opening out in front of it, and the frame it sees at depth 0.
//!
//! From above the camera is at the foot, looking up the screen; from the side it is on the
//! left, looking right. Dragging a layer's line moves the peg it is on in depth; dragging the
//! camera moves the camera's peg. Each selected peg has a handle at its pivot, which moves it
//! in depth and across, even where many lines lie on one another.

use egui::{Pos2, Sense, Stroke, vec2};
use iskra_drawing::Point;
use iskra_model::{LayerId, PegId, Shot, Transform};
use serde_json::{Value, json};

use crate::theme::Theme;
use crate::view::View;

/// Where the scene is looked at from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    Above,
    TheSide,
}

/// How close to a line or to the camera a press counts as on it, in screen points.
const REACH: f32 = 7.0;

/// A press that moves less than this on screen, in points, is a click, not a drag.
const CLICK_SLOP: f32 = 3.0;

/// How long a peg handle's arrows are, in screen points, and how wide its square is.
const ARROW: f32 = 28.0;
const SQUARE: f32 = 5.0;

/// The part of a peg's handle that is held: the square moves the peg every way in the view,
/// an arrow along its line alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    Free,
    Depth,
    Across,
}

/// A selected peg's handle as the view draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Handle {
    pub peg: PegId,
    /// Its pivot, and the ends of its arrows in depth (away from the camera) and across.
    pub at: Pos2,
    pub depth: Pos2,
    pub across: Pos2,
}

/// What a view asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Select(LayerId),
    SelectPeg(PegId),
    /// Pick a Particles node, as a click on its box in the node view does.
    Pick(iskra_model::EffectId),
    Run(&'static str, Value),
}

/// A peg being moved in depth.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    peg: PegId,
    /// The layer whose line was pressed, if it was not the camera or a handle.
    layer: Option<LayerId>,
    /// The part of a peg's handle that was pressed, if it was one, and whether it has moved
    /// the peg across.
    grip: Option<Grip>,
    across: bool,
    /// Where the press was, and the peg as it was then.
    pressed: Pos2,
    from: Transform,
    now: Transform,
}

/// One layer as the view draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub layer: LayerId,
    pub depth: f64,
    /// The two ends of its line on screen.
    pub ends: [Pos2; 2],
}

/// Where a point of the scene, across (x from above, y from the side) and at a depth, is in
/// the view's own plane: nearer the viewer is down from above, and left from the side.
fn plane(from: From, across: f64, depth: f64) -> [f64; 2] {
    match from {
        From::Above => [across, depth],
        From::TheSide => [-depth, across],
    }
}

/// How far across (x from above, y from the side) a move on screen is, in scene units.
fn sideways(from: From, zoom: f32, moved: egui::Vec2) -> f64 {
    f64::from(match from {
        From::Above => moved.x,
        From::TheSide => moved.y,
    }) / f64::from(zoom)
}

/// How much nearer the viewer a move on screen is, in scene units, at `zoom` points a unit.
fn nearer(from: From, zoom: f32, moved: egui::Vec2) -> f64 {
    f64::from(match from {
        From::Above => moved.y,
        From::TheSide => -moved.x,
    }) / f64::from(zoom)
}

pub struct DepthView {
    /// The boards that are locked as reference where the view is shown (plan §7.2c), and the
    /// layers locked on their rows of the timeline: their lines select them, and do not move
    /// them. The app says which.
    pub locked: std::collections::BTreeSet<LayerId>,
    from: From,
    pub view: View,
    fitted: bool,
    drag: Option<Drag>,
    /// The layers as they were last drawn, and where the camera was.
    pub marks: Vec<Mark>,
    pub camera: Option<Pos2>,
    /// The selected pegs' handles as they were last drawn.
    pub handles: Vec<Handle>,
    /// Each sheet of a Particles node as it was last drawn: the node, its depth, and the ends
    /// of its line.
    pub sheets: Vec<(iskra_model::EffectId, f64, [Pos2; 2])>,
}

impl DepthView {
    pub fn new(from: From) -> Self {
        Self {
            locked: Default::default(),
            from,
            view: View::default(),
            fitted: false,
            drag: None,
            marks: Vec::new(),
            camera: None,
            handles: Vec::new(),
            sheets: Vec::new(),
        }
    }

    /// The peg that is being dragged here, as the drag has it so far, for the other views to
    /// show.
    pub fn live(&self) -> Option<(PegId, Transform)> {
        self.drag.filter(|drag| drag.now != drag.from).map(|drag| (drag.peg, drag.now))
    }

    /// Show `shot` on `frame`, whose picture is `size` scene units. `layer` and `peg` are what
    /// is selected, and `pegs` those that have a handle. With `keep_size`, a peg moved in depth
    /// keeps the size and the place the camera sees what it carries at.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        shot: &Shot,
        frame: u32,
        size: [u32; 2],
        layer: Option<LayerId>,
        peg: Option<PegId>,
        pegs: &[PegId],
        keep_size: bool,
        particles: Option<iskra_model::EffectId>,
    ) -> Vec<Action> {
        let theme = Theme::of(ui.ctx());
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let (rect, center) = (response.rect, response.rect.center());
        let mut actions = Vec::new();

        // The shot as the drag has it so far.
        let mut posed = shot.clone();
        if let Some((id, transform)) = self.live()
            && let Some(dragged) = posed.pegs.iter_mut().find(|peg| peg.id == id)
        {
            let dragged = std::sync::Arc::make_mut(dragged);
            (dragged.transform, dragged.animation) = (transform, Default::default());
        }
        let seen = iskra_eval::evaluate(&posed, frame);
        let from = self.from;
        let across = |point: Point| match from {
            From::Above => point.x,
            From::TheSide => point.y,
        };

        // Where the camera stands, and how wide it sees at a depth.
        let camera = &posed.camera;
        let stands = camera.distance_at(frame) + posed.depth(camera.parent, frame);
        let eye = across(posed.world(camera.parent, frame).apply(Point::new(0.0, 0.0)));
        let (half_width, half_height) = (f64::from(size[0]) / 2.0, f64::from(size[1]) / 2.0);
        let sees = |depth: f64| {
            let back = posed.seen(depth, frame)?.inverse()?;
            let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y)| across(back.apply(Point::new(x * half_width, y * half_height))));
            Some((corners.into_iter().fold(f64::INFINITY, f64::min), corners.into_iter().fold(f64::NEG_INFINITY, f64::max)))
        };

        // Each layer: from one edge of what is painted on it to the other, at its depth.
        let mut lines: Vec<(LayerId, f64, f64, f64)> = Vec::new();
        for placed in &seen.layers {
            let Some(of) = posed.layer(placed.layer) else { continue };
            let bounds = placed
                .drawing
                .and_then(|id| of.drawings.iter().find(|entry| entry.id == id))
                .map(|entry| entry.drawing.bounds())
                .filter(|bounds| !bounds.is_empty());
            let (lo, hi) = match bounds {
                Some(b) => {
                    let corners = [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)]
                        .map(|(x, y)| across(placed.to_scene.apply(Point::new(f64::from(x), f64::from(y)))));
                    (corners.into_iter().fold(f64::INFINITY, f64::min), corners.into_iter().fold(f64::NEG_INFINITY, f64::max))
                }
                // Nothing painted on the frame: a tick where the layer's origin is.
                None => {
                    let at = across(placed.to_scene.apply(Point::new(0.0, 0.0)));
                    (at, at)
                }
            };
            lines.push((placed.layer, placed.depth, lo, hi));
        }

        // The first time, show the camera and everything in front of it.
        let farthest = lines.iter().map(|line| line.1).fold(0.0, f64::min) - camera.distance_at(frame) * 0.25;
        if !self.fitted && rect.width() > 1.0 {
            let reach = sees(farthest).map_or(half_width, |(lo, hi)| lo.abs().max(hi.abs())).max(half_width);
            let (near, far) = (plane(from, 0.0, stands), plane(from, 0.0, farthest));
            let (wide, high) = match from {
                From::Above => (2.0 * reach, stands - farthest),
                From::TheSide => (stands - farthest, 2.0 * reach),
            };
            self.view.fit(rect, wide as f32, high as f32);
            let middle = [(near[0] + far[0]) / 2.0, (near[1] + far[1]) / 2.0];
            self.view.pan = -vec2(middle[0] as f32, middle[1] as f32) * self.view.zoom;
            self.fitted = true;
        }

        // Navigation, as in the viewport: scroll pans, pinch zooms, Space or the middle button drags.
        let panning = ui.input(|i| i.key_down(egui::Key::Space) || i.pointer.button_down(egui::PointerButton::Middle));
        if let Some(pointer) = response.hover_pos() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta(), i.zoom_delta()));
            self.view.pan += scroll;
            if zoom != 1.0 {
                self.view.zoom_about(center, pointer, zoom);
            }
        }
        if panning && response.is_pointer_button_down_on() {
            self.view.pan += ui.input(|i| i.pointer.delta());
        }
        let view = self.view;
        let on_screen = |across: f64, depth: f64| view.to_screen(center, plane(from, across, depth));
        let painter = painter.with_clip_rect(rect);

        // The plane at depth 0, where a drawing is seen as large as it is drawn.
        let along = |depth: f64| match from {
            From::Above => [Pos2::new(rect.left(), on_screen(0.0, depth).y), Pos2::new(rect.right(), on_screen(0.0, depth).y)],
            From::TheSide => [Pos2::new(on_screen(0.0, depth).x, rect.top()), Pos2::new(on_screen(0.0, depth).x, rect.bottom())],
        };
        painter.line_segment(along(0.0), Stroke::new(1.0, theme.border));

        // The camera: what it sees opens out from it, and the frame it sees at depth 0. Its
        // colour, an orange unless the user set another, is that of what it sees too.
        let camera_colour = theme.colour(crate::theme::Role::Camera);
        let at = on_screen(eye, stands);
        if let Some((lo, hi)) = sees(farthest) {
            for edge in [lo, hi] {
                painter.line_segment([at, on_screen(edge, farthest)], Stroke::new(1.0, camera_colour.gamma_multiply(0.6)));
            }
        }
        if let Some((lo, hi)) = sees(0.0) {
            painter.line_segment([on_screen(lo, 0.0), on_screen(hi, 0.0)], Stroke::new(2.0, camera_colour));
        }
        let moved_by_peg = camera.parent.is_some();
        painter.circle(at, 5.0, if moved_by_peg { camera_colour } else { theme.surface }, Stroke::new(1.5, camera_colour));
        let beside = match from {
            From::Above => (at + vec2(9.0, 0.0), egui::Align2::LEFT_CENTER),
            From::TheSide => (at + vec2(0.0, -9.0), egui::Align2::CENTER_BOTTOM),
        };
        painter.text(beside.0, beside.1, "Camera", egui::FontId::proportional(11.0), camera_colour);
        self.camera = Some(at);

        // The layers, the farthest first, each with its name at its end.
        self.marks.clear();
        for (id, depth, lo, hi) in &lines {
            let mut ends = [on_screen(*lo, *depth), on_screen(*hi, *depth)];
            if (ends[1] - ends[0]).length() < 12.0 {
                let (middle, half) = (ends[0] + (ends[1] - ends[0]) / 2.0, if from == From::Above { vec2(6.0, 0.0) } else { vec2(0.0, 6.0) });
                ends = [middle - half, middle + half];
            }
            // A selected peg marks what it carries.
            let carried = peg.is_some_and(|peg| posed.ancestors(posed.layer(*id).and_then(|of| of.parent)).any(|above| above.id == peg));
            let selected = layer == Some(*id) || carried;
            let colour = if selected { theme.accent } else { theme.text };
            painter.line_segment(ends, Stroke::new(if selected { 3.0 } else { 2.0 }, colour));
            if let Some(of) = posed.layer(*id) {
                let (end, anchor) = match from {
                    From::Above => (ends[1] + vec2(6.0, 0.0), egui::Align2::LEFT_CENTER),
                    From::TheSide => (ends[1] + vec2(0.0, 4.0), egui::Align2::CENTER_TOP),
                };
                painter.text(end, anchor, &of.name, egui::FontId::proportional(11.0), if selected { theme.accent } else { theme.text_muted });
            }
            self.marks.push(Mark { layer: *id, depth: *depth, ends });
        }

        // Each Particles node as the cards it makes, a dashed line for each at its depth: as
        // wide as the camera sees there, or out of an emitter as wide as it is. Rain or snow
        // in planes is a line for each plane. The node picked is lit.
        self.sheets.clear();
        for card in seen.cards.iter().flat_map(iskra_eval::Card::leaves) {
            let iskra_eval::Card::Through { effect: id, depth, .. } = card else { continue };
            let Some(node) = posed.effect(*id).filter(|effect| effect.kind == iskra_model::PARTICLES) else { continue };
            let number = |param: &str| node.number(param, frame);
            let kind = node.choice("kind", frame);
            let emitter = node.choice("emitter", frame);
            let emitted = matches!(kind.as_str(), "Rain" | "Snow" | "Wind") && emitter != "Whole Frame";
            let extent = match emitted {
                true => {
                    // Placed on the frame at the node's depth on the first frame, as it is made.
                    let (half_width, half_height) = match emitter.as_str() {
                        "Area" => (number("emitterWidth") / 2.0, number("emitterHeight") / 2.0),
                        "Line" => (number("emitterWidth") / 2.0, 0.0),
                        _ => (0.0, 0.0),
                    };
                    posed.seen(number(iskra_model::DEPTH), 0).and_then(|seen| seen.inverse()).map(|home| {
                        let (x, y) = (number("emitterX"), number("emitterY"));
                        let ends = [home.apply(Point::new(x - half_width, y - half_height)), home.apply(Point::new(x + half_width, y + half_height))];
                        (across(ends[0]).min(across(ends[1])), across(ends[0]).max(across(ends[1])))
                    })
                }
                false => sees(*depth),
            };
            let Some((lo, hi)) = extent else { continue };
            let mut ends = [on_screen(lo, *depth), on_screen(hi, *depth)];
            if (ends[1] - ends[0]).length() < 12.0 {
                let (middle, half) = (ends[0] + (ends[1] - ends[0]) / 2.0, if from == From::Above { vec2(6.0, 0.0) } else { vec2(0.0, 6.0) });
                ends = [middle - half, middle + half];
            }
            let colour = if particles == Some(*id) { theme.accent } else { theme.text_muted };
            painter.extend(egui::Shape::dashed_line(&ends, Stroke::new(1.5, colour), 6.0, 4.0));
            let (end, anchor) = match from {
                From::Above => (ends[0] - vec2(6.0, 0.0), egui::Align2::RIGHT_CENTER),
                From::TheSide => (ends[0] - vec2(0.0, 4.0), egui::Align2::CENTER_BOTTOM),
            };
            painter.text(end, anchor, &node.name, egui::FontId::proportional(10.0), colour);
            self.sheets.push((*id, *depth, ends));
        }

        // A handle on each selected peg, at its pivot and its depth, over the lines.
        self.handles.clear();
        let (away, aside) = match from {
            From::Above => (vec2(0.0, -ARROW), vec2(ARROW, 0.0)),
            From::TheSide => (vec2(ARROW, 0.0), vec2(0.0, ARROW)),
        };
        for id in pegs {
            let Some(handled) = posed.peg(*id) else { continue };
            let pose = handled.transform_at(frame);
            let pivot = posed.above(handled, frame).apply(Point::new(pose.position[0] + pose.pivot[0], pose.position[1] + pose.pivot[1]));
            let at = on_screen(across(pivot), posed.depth(Some(*id), frame));
            let handle = Handle { peg: *id, at, depth: at + away, across: at + aside };
            let held = self.drag.and_then(|drag| drag.grip.filter(|_| drag.peg == *id));
            for (grip, end) in [(Grip::Depth, handle.depth), (Grip::Across, handle.across)] {
                let width = if held == Some(grip) { 3.0 } else { 2.0 };
                painter.arrow(at, end - at, Stroke::new(width, theme.accent));
            }
            let square = egui::Rect::from_center_size(at, vec2(2.0 * SQUARE, 2.0 * SQUARE));
            painter.rect_filled(square, 1.0, if held == Some(Grip::Free) { theme.text } else { theme.accent });
            self.handles.push(handle);
        }

        // What is under the pointer: the camera before a layer, the nearest layer before a farther one.
        let to_line = |pos: Pos2, [a, b]: [Pos2; 2]| {
            let (line, to) = (b - a, pos - a);
            let part = if line.length_sq() > 0.0 { (to.dot(line) / line.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
            (pos - (a + line * part)).length()
        };
        let under = |pos: Pos2| -> Option<Option<LayerId>> {
            if (pos - at).length() <= REACH + 2.0 {
                return Some(None);
            }
            self.marks.iter().rev().find(|mark| to_line(pos, mark.ends) <= REACH).map(|mark| Some(mark.layer))
        };
        // A handle comes before anything else: its square, then its arrows.
        let gripped = |pos: Pos2| -> Option<(PegId, Grip)> {
            let handles = self.handles.iter().rev();
            handles.clone().find(|handle| (pos - handle.at).abs().max_elem() <= SQUARE + 2.0).map(|handle| (handle.peg, Grip::Free)).or_else(|| {
                handles
                    .flat_map(|handle| {
                        [(handle.peg, Grip::Depth, handle.depth), (handle.peg, Grip::Across, handle.across)].map(|(peg, grip, end)| (peg, grip, handle.at, end))
                    })
                    .find(|(_, _, at, end)| to_line(pos, [*at, *end]) <= REACH - 2.0)
                    .map(|(peg, grip, _, _)| (peg, grip))
            })
        };
        let hovered_grip = response.hover_pos().filter(|_| !panning).and_then(gripped);
        if let Some((_, grip)) = hovered_grip {
            let hint = match grip {
                Grip::Free => "Drag to move the selected peg in depth and across",
                Grip::Depth => "Drag to move the selected peg in depth alone",
                Grip::Across => "Drag to move the selected peg across alone",
            };
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            response.clone().on_hover_text_at_pointer(hint);
        }
        let hovered = response.hover_pos().filter(|_| !panning && hovered_grip.is_none()).and_then(under);
        if let Some(what) = hovered {
            let carried = match what {
                None => camera.parent,
                Some(id) => posed.layer(id).and_then(|layer| layer.parent),
            };
            let hint = match (what, carried) {
                (None, Some(_)) => "Drag to move the camera in depth".to_string(),
                (None, None) => "The camera is moved by a peg: Animation › Add Camera Peg".to_string(),
                (Some(_), Some(_)) => "Drag to move it in depth".to_string(),
                (Some(_), None) => "A layer gets its depth from a peg: Animation › Add Parent Peg".to_string(),
            };
            ui.ctx().set_cursor_icon(if carried.is_some() { egui::CursorIcon::Grab } else { egui::CursorIcon::PointingHand });
            response.clone().on_hover_text_at_pointer(hint);
        }

        // A press on a line or on the camera takes the peg that carries it.
        let down = !panning && response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let pressed = down && self.drag.is_none() && ui.input(|i| i.pointer.primary_pressed());
        if pressed
            && let Some((pos, (id, grip))) = pointer.and_then(|pos| Some((pos, gripped(pos)?)))
            && let Some(held) = shot.peg(id)
        {
            let from = held.transform_at(frame);
            self.drag = Some(Drag { peg: id, layer: None, grip: Some(grip), across: false, pressed: pos, from, now: from });
        }
        if pressed
            && self.drag.is_none()
            && let Some((pos, what)) = pointer.and_then(|pos| Some((pos, under(pos)?)))
        {
            let carried = match what {
                None => shot.camera.parent,
                Some(id) => shot.layer(id).and_then(|layer| layer.parent),
            };
            let locked = what.is_some_and(|id| self.locked.contains(&id));
            match (carried.and_then(|id| shot.peg(id)), what) {
                (Some(_), Some(id)) if locked => actions.push(Action::Select(id)),
                (Some(peg), _) => {
                    let from = peg.transform_at(frame);
                    self.drag = Some(Drag { peg: peg.id, layer: what, grip: None, across: false, pressed: pos, from, now: from });
                }
                (None, Some(id)) => actions.push(Action::Select(id)),
                (None, None) => {}
            }
        }
        // A press on a sheet of particles, where nothing else is, picks its node.
        if pressed
            && self.drag.is_none()
            && let Some(pos) = pointer.filter(|pos| under(*pos).is_none() && gripped(*pos).is_none())
            && let Some((id, ..)) = self.sheets.iter().rev().find(|(_, _, ends)| to_line(pos, *ends) <= REACH)
        {
            actions.push(Action::Pick(*id));
        }
        if let (Some(drag), Some(pos)) = (self.drag.as_mut(), pointer)
            && down
            && (pos - drag.pressed).length() > CLICK_SLOP
        {
            let moved = pos - drag.pressed;
            let (deeper, aside) = match drag.grip {
                None | Some(Grip::Depth) => (nearer(from, view.zoom, moved), 0.0),
                Some(Grip::Free) => (nearer(from, view.zoom, moved), sideways(from, view.zoom, moved)),
                Some(Grip::Across) => (0.0, sideways(from, view.zoom, moved)),
            };
            let depth = drag.from.depth + deeper;
            let mut now = Transform { depth, ..drag.from };
            // The camera's own peg has no size to keep, and `kept_in_view` says so.
            if keep_size
                && deeper != 0.0
                && let Some((position, scale)) = shot.kept_in_view(drag.peg, depth, frame)
            {
                (now.position, now.scale) = (position, scale);
            }
            // Across the scene, brought into the space the peg's values are in.
            if aside != 0.0
                && let Some(moving) = shot.peg(drag.peg)
                && let Some(back) = shot.above(moving, frame).inverse()
            {
                let scene = match from {
                    From::Above => Point::new(aside, 0.0),
                    From::TheSide => Point::new(0.0, aside),
                };
                let (start, end) = (back.apply(Point::new(0.0, 0.0)), back.apply(scene));
                now.position = [now.position[0] + end.x - start.x, now.position[1] + end.y - start.y];
            }
            (drag.now, drag.across) = (now, aside != 0.0);
        }
        if !down && let Some(drag) = self.drag.take() {
            if drag.now == drag.from {
                // A click selects what was pressed.
                actions.push(match drag.layer {
                    Some(id) => Action::Select(id),
                    None => Action::SelectPeg(drag.peg),
                });
            } else if drag.across {
                // Moved across: what it has come to, said in full.
                let mut params = json!({ "peg": drag.peg.0, "frame": frame, "position": drag.now.position });
                if drag.now.depth != drag.from.depth {
                    params["depth"] = json!(drag.now.depth);
                }
                if drag.now.scale != drag.from.scale {
                    params["scale"] = json!(drag.now.scale);
                }
                actions.push(Action::Run("peg.setTransform", params));
            } else {
                let mut params = json!({ "peg": drag.peg.0, "frame": frame, "depth": drag.now.depth });
                if drag.now.position != drag.from.position || drag.now.scale != drag.from.scale {
                    params["keepSize"] = json!(true);
                }
                actions.push(Action::Run("peg.setTransform", params));
            }
        }
        // The depth of what is dragged, in numbers, beside the pointer.
        if let (Some(drag), Some(pos)) = (self.drag.filter(|drag| drag.now != drag.from), pointer) {
            painter.text(
                pos + vec2(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                match drag.grip {
                    Some(Grip::Free | Grip::Across) => {
                        let [x, y] = drag.now.position;
                        format!("depth {:.1}, {} {:.1}", drag.now.depth, if from == From::Above { "x" } else { "y" }, if from == From::Above { x } else { y })
                    }
                    _ => format!("depth {:.1}", drag.now.depth),
                },
                egui::FontId::proportional(11.0),
                theme.text,
            );
        }
        painter.text(
            rect.left_top() + vec2(8.0, 6.0),
            egui::Align2::LEFT_TOP,
            if from == From::Above { "From above" } else { "From the side" },
            egui::FontId::proportional(11.0),
            theme.text_muted,
        );
        actions
    }
}
