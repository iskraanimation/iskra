//! The Bones tool (plan §4.5, R4): a chain of bones laid on the layer that is drawn on, by
//! dragging in the viewport; the Transform tool poses it.
//!
//! Both work on the selected layer's deformer. With the Bones tool the drawing is shown as it
//! was drawn and the drags lay the chain along it, at rest: a drag on a joint moves it, a drag
//! from anywhere else pulls a new bone out of the chain's end while *Add Bones* is on (else it
//! is a click, which selects the drawing under it), and on a layer with no bones yet pulls out
//! the first. With the Transform tool a drag on the chain poses it on the frame
//! that is showing: a bone turns about its start, and the chain's first joint moves all of it;
//! a drag off it is the peg's. With Alt held, a bone dragged by its end is made longer or
//! shorter too, and what it holds is stretched.
//!
//! Where the chain has IK on the frame (plan §4.18), its goal is a ring with a cross, and a
//! drag on it, or on the joint that reaches for it, moves the goal: the bones up to that joint
//! follow. A bone before it, dragged, turns as its own turn says, which only shapes how they
//! bend.
//!
//! A layer's deformer can be a curve instead (`Tools::curve` says which a bare layer gets): a
//! smooth line through points, which are laid and moved the same way, a drag pulling a new
//! point out of the line's end while the rest is edited, and a drag on a point moving it,
//! at rest or as a pose. Its points have two circles as well, selected and dragged as a
//! joint's are: they say how far to the side of the line the drawing is carried.
//!
//! The selected point of a curve shows its two handles, which shape how the line passes it:
//! one dragged swings the other round to keep the line smooth, or with Alt held leaves it.
//! While the rest is edited that is the rest's shape; otherwise it is the pose on the frame,
//! keyed where the point is. A drag that starts on the line between two points of the rest
//! puts a new point in there.
//!
//! Or an envelope: a closed outline through points round a shape. A first drag lays an oval of
//! four points in the box that is dragged out; from there its points are a curve's.
//!
//! A joint between two bones has two circles, which say how round it is: within the first
//! the drawing goes round the joint, and from there the roundness falls off to nothing at the
//! second. A click on a joint selects it, and the circles of the selected joint are dragged
//! by their edges, while the rest is edited or the chain is posed; one that has no circles
//! yet shows a small ring to pull them out by. While the rest is edited every joint's circles
//! are dragged, and a drag out of a joint with Alt held pulls them out too.

use egui::{Color32, Pos2, Stroke as Outline};
use iskra_model::{Affine, Deformer, DeformerId, Layer, LayerId, Point, Shot};
use serde_json::{Value, json};

use crate::viewport::{CLICK_SLOP, HANDLE_REACH, Lays, Tools, Viewport, space};

/// What of a chain a drag has hold of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// A bone, by its place in the chain, to turn about its start.
    Turn(usize),
    /// A bone, by its end, to turn about its start and to make as long as the pointer is far.
    Pull(usize),
    /// The chain's first joint, which moves all of it.
    Shift,
    /// A joint at rest: 0 is where the chain starts.
    Joint(usize),
    /// A point of a curve, to move it on the frame.
    Knot(usize),
    /// A handle of a curve's point: the one the line leaves by, or the one it arrives by.
    Handle(usize, bool),
    /// The same handle while the curve is posed: it is moved on the frame.
    Swing(usize, bool),
    /// A new point of a curve, put in after the one at this place.
    Insert(usize),
    /// The circle of a joint within which it is round, and the one where that has faded out:
    /// the joint is the one the bone at this place starts at.
    Round(usize),
    Fade(usize),
    /// A bone that is being pulled out of the chain's end, or the first of a new chain.
    New,
    /// The goal of the chain's IK, to move it on the frame.
    Goal,
}

/// A drag with the Bones tool. The engine gets one command when it ends.
pub(crate) struct BoneDrag {
    layer: LayerId,
    /// The chain that is dragged, if the layer has one already.
    pub(crate) deformer: Option<DeformerId>,
    grab: Grab,
    /// The joint the press was on, if it was on one: a click there selects it.
    near: Option<usize>,
    /// Where the press was, on screen, and where the pointer was a frame ago.
    start: Pos2,
    last: Pos2,
    /// The pointer has gone far enough from the press for this to be a drag, not a click.
    moved: bool,
    /// The chain before the drag, and with the drag so far.
    from: Deformer,
    pub(crate) now: Deformer,
}

/// The ring a selected joint with no circles shows, to pull them out by: its radius on screen.
const RING: f32 = 14.0;
/// The ring round the point where a curve starts: its radius on screen. Its colour, and that of
/// the outline round the drawing whose bones are being laid, are the theme's `ChainStart` and
/// `Rigged`.
const START_RING: f32 = 11.0;
/// The ring of an IK goal: its radius on screen.
const GOAL_RING: f32 = 9.0;
/// The shortest a bone is made by a drag, in scene units.
const SHORTEST: f64 = 1.0;
/// How short and how long a drag stretches a bone, as a part of its length at rest.
const STRETCH: std::ops::RangeInclusive<f64> = 0.05..=20.0;

/// A deformer's rest as its commands take it: a chain's `origin` and `bones`, or the points of
/// a `curve`.
pub(crate) fn rest_of(chain: &Deformer) -> Value {
    if chain.is_curve() {
        return json!({ chain.kind().1: chain.spine.iter().map(|knot| knot.at).collect::<Vec<_>>() });
    }
    let bones: Vec<Value> = chain.bones.iter().map(|bone| json!({ "angle": bone.rest_angle, "length": bone.length })).collect();
    json!({ "origin": chain.origin, "bones": bones })
}

/// The deformer as its own pose has it, without what the pegs wired into its ports do.
fn unheld(deformer: &Deformer) -> Deformer {
    Deformer { carried: None, pinned: None, ..deformer.clone() }
}

/// A move of `by` of the point at `point` where it is seen, as a move of its own pose: of the
/// start, which the peg in the start port carries, taken back through that peg's turn and
/// scale; of any other point, as it is.
pub(crate) fn held_back(deformer: &Deformer, point: usize, by: [f64; 2]) -> [f64; 2] {
    let Some(back) = deformer.carried.filter(|_| point == 0).and_then(|carried| carried.inverse()) else { return by };
    let (from, to) = (back.apply(Point::new(0.0, 0.0)), back.apply(Point::new(by[0], by[1])));
    [to.x - from.x, to.y - from.y]
}

/// The handles of a curve's point that are shown and dragged: the one the line arrives by
/// and the one it leaves by, each from the point, but not one at an end of a curve that no
/// line runs on from, nor one of no length. With each, whether it is the one it leaves by.
fn shown_handles(curve: &Deformer, point: usize, handles: iskra_model::Bend) -> Vec<(bool, [f64; 2])> {
    let last = curve.spine.len().saturating_sub(1);
    let sides = [(false, handles.into, curve.is_envelope() || point > 0), (true, handles.out, curve.is_envelope() || point < last)];
    sides.into_iter().filter(|(_, handle, runs)| *runs && handle[0].hypot(handle[1]) > 1e-9).map(|(out, handle, _)| (out, handle)).collect()
}

impl Viewport {
    /// From the space the layer's bones are in to what the viewport shows, and back.
    fn bone_space(&self, shot: &Shot, layer: &Layer, frame: u32) -> Option<(Affine, Affine)> {
        let to_view = space(shot, self.flat, layer.parent, layer.parent, frame);
        Some((to_view, to_view.inverse()?))
    }

    /// Start, go on with or finish a drag of the Bones tool on `layer`.
    pub(crate) fn bone_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        shot: &Shot,
        layer: Option<LayerId>,
        tools: &Tools,
        panning: bool,
    ) -> Option<(&'static str, Value)> {
        let center = response.rect.center();
        let Some((layer, (to_view, to_local))) =
            layer.and_then(|id| shot.layer(id)).and_then(|layer| Some((layer, self.bone_space(shot, layer, tools.frame)?)))
        else {
            self.bone_drag = None;
            return None;
        };
        let frame = tools.frame;
        // On a layer that a peg tilts, seen through the camera, the chain is where it is drawn,
        // in perspective, and the pointer is taken onto the layer's plane there.
        let leant = (!self.flat && shot.tilted(layer.parent, frame)).then(|| shot.projected(layer.parent, frame)).flatten();
        let back = leant.and_then(|leant| leant.back());
        let screen = |at: Point| {
            let seen = leant.and_then(|leant| leant.apply(at)).unwrap_or_else(|| to_view.apply(at));
            self.view.to_screen(center, [seen.x, seen.y])
        };
        let local = |pos: Pos2| {
            let [x, y] = self.view.to_scene(center, pos);
            let on_plane = back.and_then(|back| {
                let [u, v, s] = back.map(|row| row[0] * x + row[1] * y + row[2]);
                (s > 0.0).then(|| Point::new(u / s, v / s))
            });
            on_plane.unwrap_or_else(|| to_local.apply(Point::new(x, y)))
        };
        // As posed in the shot: with the goal of its IK where the peg wired in has it.
        let resolved = layer.deformer.and_then(|id| shot.chain(id, frame));
        let chain = resolved.as_deref();
        let picked = self.bone_joint;
        if !panning && response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()) {
            let pos = response.interact_pointer_pos()?;
            if self.bone_drag.is_none() {
                self.bone_hit = false;
                let joints = chain.map_or(Vec::new(), |chain| chain.joints((!tools.rest()).then_some(frame)));
                let near = joints.iter().position(|joint| (screen(*joint) - pos).length() <= HANDLE_REACH);
                // The bone the press is on, between its joints.
                let on_bone = || {
                    joints.windows(2).position(|ends| {
                        let (a, b) = (screen(ends[0]), screen(ends[1]));
                        let along = b - a;
                        let part = if along.length_sq() > 0.0 { ((pos - a).dot(along) / along.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
                        (a + along * part - pos).length() <= HANDLE_REACH
                    })
                };
                let pulls = ui.input(|i| i.modifiers.alt);
                // The circle of a joint whose edge the press is on. Where a joint's two circles
                // are one, a press outside it means the outer and one inside it the inner.
                let on_circle = || {
                    let chain = chain?;
                    // While the chain is posed, only the selected joint's circles are in reach.
                    let within = |joint: &usize| tools.rest() || picked == Some(*joint);
                    let rings = chain.circled().filter(within).flat_map(|joint| [(joint, false), (joint, true)]).filter_map(|(joint, outer)| {
                        let radius = chain.circles(joint)[usize::from(outer)];
                        let (center, edge) = (screen(joints[joint]), screen(Point::new(joints[joint].x + radius, joints[joint].y)));
                        // A selected joint with no circles has a ring to pull them out by.
                        let ring = picked == Some(joint) && !outer && chain.circles(joint)[1] <= 0.0;
                        let shown = if radius > 0.0 { (edge - center).length() } else { RING };
                        let off = (center - pos).length() - shown;
                        let rather = if outer { off >= 0.0 } else { off < 0.0 };
                        ((radius > 0.0 || ring) && off.abs() <= HANDLE_REACH * 0.7).then_some((off.abs(), !rather, joint, outer))
                    });
                    let (_, _, joint, outer) = rings.min_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap_or(std::cmp::Ordering::Equal))?;
                    Some(if outer { Grab::Fade(joint) } else { Grab::Round(joint) })
                };
                // A handle of the selected point of a curve, at rest or as it is posed: where
                // the view is far out it lies close by its point, and the nearer of the two
                // is meant.
                let handle = chain.filter(|curve| curve.is_curve()).and_then(|curve| {
                    let point = picked.filter(|point| *point < curve.spine.len())?;
                    let (at, handles) = (joints[point], if tools.rest() { curve.handles(point) } else { curve.handles_at(point, frame) }?);
                    let ends =
                        shown_handles(curve, point, handles).into_iter().map(|(out, [x, y])| (out, (screen(Point::new(at.x + x, at.y + y)) - pos).length()));
                    let (out, away) = ends.min_by(|a, b| a.1.total_cmp(&b.1))?;
                    let nearer = near.is_none_or(|joint| away < (screen(joints[joint]) - pos).length());
                    (away <= HANDLE_REACH && nearer).then_some(if tools.rest() { Grab::Handle(point, out) } else { Grab::Swing(point, out) })
                });
                // The goal of the chain's IK, where it is on, or the joint that reaches for it.
                let on_goal = chain.filter(|_| !tools.rest()).and_then(|chain| {
                    // A goal that is a peg is moved by moving the peg.
                    let ik = chain.ik.as_ref().filter(|ik| ik.on_at(frame) && ik.peg.is_none())?;
                    let [x, y] = ik.goal_at(frame);
                    let reaching = ik.point.unwrap_or(chain.bones.len()).min(chain.bones.len());
                    ((screen(Point::new(x, y)) - pos).length() <= HANDLE_REACH || near == Some(reaching)).then_some(Grab::Goal)
                });
                let grab = match (chain, tools.rest(), near) {
                    // A layer with no bones is given them only while the rest is edited.
                    (None, true, _) => Grab::New,
                    (None, false, _) => return None,
                    (Some(_), false, _) if on_goal.is_some() => Grab::Goal,
                    (Some(_), ..) if handle.is_some() => handle.unwrap_or(Grab::New),
                    // A curve: its points are moved, at rest or posed, and a drag off them
                    // pulls a new one out of its end while the rest is edited.
                    // Its points have circles too, pulled out and dragged as a joint's are.
                    (Some(curve), true, Some(point)) if curve.is_curve() && pulls => Grab::Round(point),
                    (Some(curve), true, Some(point)) if curve.is_curve() => Grab::Joint(point),
                    (Some(curve), true, None) if curve.is_curve() => {
                        // The line itself, between two points: a new point is put in there.
                        let on_line = || {
                            let line: Vec<Pos2> = curve.line(None).into_iter().map(screen).collect();
                            let near = |at: &Pos2| (*at - pos).length() <= HANDLE_REACH;
                            line.iter()
                                .position(near)
                                .map(|piece| Grab::Insert((piece / iskra_model::PIECES).min(curve.spine.len() - if curve.is_envelope() { 1 } else { 2 })))
                        };
                        match on_circle().or_else(on_line) {
                            Some(grab) => grab,
                            None if tools.add_bones => Grab::New,
                            // A press on nothing lets the selected point go, and is a click.
                            None => {
                                self.bone_joint = None;
                                return None;
                            }
                        }
                    }
                    // A last point the peg in the curve's end port holds is moved by moving the peg.
                    (Some(curve), false, Some(point)) if curve.is_curve() && curve.pinned.is_some() && point + 1 == curve.spine.len() => return None,
                    (Some(curve), false, Some(point)) if curve.is_curve() => Grab::Knot(point),
                    (Some(curve), false, None) if curve.is_curve() => match on_circle() {
                        Some(grab) => grab,
                        // A press on nothing lets the selected point go.
                        None => {
                            self.bone_joint = None;
                            return None;
                        }
                    },
                    // With Alt, the circles are pulled out of a joint between two bones.
                    (Some(chain), true, Some(joint)) if pulls && joint > 0 && joint < chain.bones.len() => Grab::Round(joint),
                    (Some(_), true, Some(joint)) => Grab::Joint(joint),
                    (Some(_), true, None) => match on_circle() {
                        Some(grab) => grab,
                        None if tools.add_bones => Grab::New,
                        // A press on nothing lets the selected joint go, and is a click: on
                        // another rigged drawing it selects that one.
                        None => {
                            self.bone_joint = None;
                            return None;
                        }
                    },
                    (Some(_), false, Some(0)) => Grab::Shift,
                    // A joint is the end of the bone before it.
                    (Some(_), false, Some(joint)) if pulls => Grab::Pull(joint - 1),
                    (Some(_), false, Some(joint)) => Grab::Turn(joint - 1),
                    (Some(_), false, None) => match on_circle().or_else(|| on_bone().map(Grab::Turn)) {
                        Some(grab) => grab,
                        // A press on nothing lets the selected joint and bone go.
                        None => {
                            (self.bone_joint, self.bone_picked) = (None, None);
                            return None;
                        }
                    },
                };
                // A bone taken hold of to pose it is the selected one.
                if let Grab::Turn(bone) | Grab::Pull(bone) = grab {
                    self.bone_picked = Some(bone);
                }
                let from = chain.cloned().unwrap_or_else(|| {
                    let at = local(pos);
                    match tools.lays {
                        Lays::Bones => Deformer::new(DeformerId(0), "", [at.x, at.y], Vec::new()),
                        Lays::Curve => Deformer::curve(DeformerId(0), "", &[[at.x, at.y]]),
                        Lays::Envelope => Deformer::envelope(DeformerId(0), "", &[]),
                    }
                });
                let (deformer, now) = (chain.map(|chain| chain.id), from.clone());
                // The press is on something of the chain, or lays bones: not a click that
                // selects the drawing under it.
                self.bone_hit = grab != Grab::New || near.is_some();
                self.bone_drag = Some(BoneDrag { layer: layer.id, deformer, grab, near, start: pos, last: pos, moved: false, from, now });
            }
            let drag = self.bone_drag.as_mut()?;
            drag.moved |= (pos - drag.start).length() >= CLICK_SLOP;
            self.bone_hit |= drag.moved;
            let (from, to) = (local(drag.start), local(pos));
            match drag.grab {
                // A new envelope is an oval in the box that is dragged out: a point in the
                // middle of each of the box's sides, from the top round.
                Grab::New if drag.deformer.is_none() && drag.from.is_envelope() => {
                    let middle = Point::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
                    let (top, bottom, left, right) = (from.y.min(to.y), from.y.max(to.y), from.x.min(to.x), from.x.max(to.x));
                    let oval = [Point::new(middle.x, top), Point::new(right, middle.y), Point::new(middle.x, bottom), Point::new(left, middle.y)];
                    drag.now = drag.from.with_rest_joints(&oval, SHORTEST);
                }
                Grab::New => {
                    let mut joints = drag.from.joints(None);
                    joints.push(to);
                    drag.now = drag.from.with_rest_joints(&joints, SHORTEST);
                }
                Grab::Joint(joint) => {
                    let mut joints = drag.from.joints(None);
                    joints[joint] = Point::new(joints[joint].x + to.x - from.x, joints[joint].y + to.y - from.y);
                    drag.now = drag.from.with_rest_joints(&joints, SHORTEST);
                }
                Grab::Handle(point, out) | Grab::Swing(point, out) => {
                    let posed = matches!(drag.grab, Grab::Swing(..));
                    let at = drag.from.joints(posed.then_some(frame))[point];
                    let was = if posed { drag.from.handles_at(point, frame) } else { drag.from.handles(point) }.unwrap_or_default();
                    let dragged = [to.x - at.x, to.y - at.y];
                    // The other swings round to stay in line with it, as long as it was; with
                    // Alt held it stays, and the line has a corner.
                    let other = |was: [f64; 2]| {
                        let (length, reach) = (dragged[0].hypot(dragged[1]), was[0].hypot(was[1]));
                        if ui.input(|i| i.modifiers.alt) || length <= 0.0 { was } else { [-dragged[0] / length * reach, -dragged[1] / length * reach] }
                    };
                    let bend =
                        if out { iskra_model::Bend { into: other(was.into), out: dragged } } else { iskra_model::Bend { into: dragged, out: other(was.out) } };
                    if posed {
                        drag.now = drag.from.clone();
                        drag.now.pose_bend(point, frame, bend);
                    } else {
                        drag.now.spine[point].bend = Some(bend);
                    }
                }
                Grab::Insert(after) => {
                    drag.now = drag.from.clone();
                    drag.now.insert_knot(after, Some([to.x, to.y]));
                }
                Grab::Knot(point) => {
                    let was = unheld(&drag.from).joints(Some(frame))[point];
                    let [x, y] = held_back(&drag.from, point, [to.x - from.x, to.y - from.y]);
                    drag.now.pose_knot(point, frame, [was.x + x, was.y + y]);
                }
                Grab::Round(joint) | Grab::Fade(joint) => {
                    // About the joint where it is shown: at rest, or as posed.
                    let center = drag.from.joints((!tools.rest()).then_some(frame))[joint];
                    let radius = (to.x - center.x).hypot(to.y - center.y);
                    let [inner, outer] = drag.from.circles(joint);
                    drag.now.set_circles(
                        joint,
                        match drag.grab {
                            Grab::Fade(_) => [inner, radius.max(inner)],
                            // The outer circle goes along, as far out as it was; pulled out
                            // of a joint that had none, it is half as large again.
                            _ => [radius, if outer > 0.0 { radius + outer - inner } else { radius * 1.5 }],
                        },
                    );
                }
                Grab::Shift => {
                    let was = drag.from.offset_at(frame);
                    let [x, y] = held_back(&drag.from, 0, [to.x - from.x, to.y - from.y]);
                    drag.now.pose_offset(frame, [was[0] + x, was[1] + y]);
                }
                Grab::Goal => {
                    if let (Some(was), Some(ik)) = (drag.from.ik.as_ref().map(|ik| ik.goal_at(frame)), drag.now.ik.as_mut()) {
                        ik.pose_goal(frame, [was[0] + to.x - from.x, was[1] + to.y - from.y]);
                    }
                }
                Grab::Turn(bone) | Grab::Pull(bone) => {
                    // How far the pointer went round the bone's start since the last frame, on
                    // the layer's own plane, so that a mirrored peg or a tilt turns it as seen:
                    // added up, so a drag can go round more than once.
                    let pivot = drag.now.joints(Some(frame))[bone];
                    let (before, after) = (local(drag.last), local(pos));
                    let angle = |at: Point| (at.y - pivot.y).atan2(at.x - pivot.x);
                    let turned = (angle(after) - angle(before)).to_degrees();
                    let turned = (turned + 180.0).rem_euclid(360.0) - 180.0;
                    let turn = drag.now.bones[bone].turn_at(frame) + turned;
                    drag.now.pose_turn(bone, frame, turn);
                    if let Grab::Pull(_) = drag.grab {
                        let start = drag.now.joints(Some(frame))[bone];
                        let stretch = (to.x - start.x).hypot(to.y - start.y) / drag.now.bones[bone].length;
                        drag.now.pose_stretch(bone, frame, stretch.clamp(*STRETCH.start(), *STRETCH.end()));
                    }
                }
            }
            drag.last = pos;
            return None;
        }
        let drag = self.bone_drag.take()?;
        if !drag.moved {
            // A click: on a joint between two bones, or a curve's point, it selects it, elsewhere it lets
            // the selected one go; on a circle's edge it changes nothing.
            if !matches!(drag.grab, Grab::Round(_) | Grab::Fade(_) | Grab::Handle(..) | Grab::Swing(..)) {
                // Of a curve any point is selected; of a chain, a joint between two bones.
                let within = |joint: &usize| if drag.from.is_curve() { *joint < drag.from.spine.len() } else { *joint > 0 && *joint < drag.from.bones.len() };
                self.bone_joint = drag.near.filter(within);
            }
            return None;
        }
        let asked = match (drag.grab, drag.deformer) {
            (Grab::New, None) => {
                let mut params = rest_of(&drag.now);
                params["layer"] = json!(drag.layer.0);
                ("deformer.new", params)
            }
            (Grab::New | Grab::Joint(_), Some(id)) => {
                let mut params = rest_of(&drag.now);
                params["deformer"] = json!(id.0);
                ("deformer.setRest", params)
            }
            (Grab::Handle(point, _), Some(id)) => {
                let bend = drag.now.spine[point].bend.unwrap_or_default();
                ("deformer.setBend", json!({ "deformer": id.0, "knot": point, "into": bend.into, "out": bend.out }))
            }
            (Grab::Swing(point, _), Some(id)) => {
                let bend = drag.now.handles_at(point, frame).unwrap_or_default();
                ("deformer.setPose", json!({ "deformer": id.0, "frame": frame, "knot": point, "into": bend.into, "out": bend.out }))
            }
            (Grab::Insert(after), Some(id)) => {
                // The new point is the selected one.
                self.bone_joint = Some(after + 1);
                ("deformer.insertKnot", json!({ "deformer": id.0, "after": after, "at": drag.now.spine[after + 1].at }))
            }
            (Grab::Knot(point), Some(id)) => {
                let to = unheld(&drag.now).joints(Some(frame))[point];
                ("deformer.setPose", json!({ "deformer": id.0, "frame": frame, "knot": point, "to": [to.x, to.y] }))
            }
            (Grab::Turn(bone), Some(id)) => {
                ("deformer.setPose", json!({ "deformer": id.0, "frame": frame, "bone": bone, "turn": drag.now.bones[bone].turn_at(frame) }))
            }
            (Grab::Pull(bone), Some(id)) => {
                let (turn, stretch) = (drag.now.bones[bone].turn_at(frame), drag.now.bones[bone].stretch_at(frame));
                ("deformer.setPose", json!({ "deformer": id.0, "frame": frame, "bone": bone, "turn": turn, "stretch": stretch }))
            }
            (Grab::Round(joint) | Grab::Fade(joint), Some(id)) => {
                let [inner, outer] = drag.now.circles(joint);
                let circles = if drag.now.is_curve() {
                    json!({ "deformer": id.0, "knot": joint, "reach": inner, "fade": outer })
                } else {
                    json!({ "deformer": id.0, "joint": joint, "round": inner, "fade": outer })
                };
                ("deformer.setWeights", circles)
            }
            (Grab::Shift, Some(id)) => ("deformer.setPose", json!({ "deformer": id.0, "frame": frame, "offset": drag.now.offset_at(frame) })),
            (Grab::Goal, Some(id)) => {
                let goal = drag.now.ik.as_ref().map(|ik| ik.goal_at(frame));
                ("deformer.setPose", json!({ "deformer": id.0, "frame": frame, "goal": goal }))
            }
            (_, None) => return None,
        };
        // The command is run after this frame is drawn: until then the drag's end is shown.
        self.bone_held = Some(drag);
        Some(asked)
    }

    /// While the rest is edited, a dashed orange outline round what is painted of the drawing
    /// the bones are laid on, as it was drawn: which drawing is being rigged.
    fn paint_rigged(&mut self, painter: &egui::Painter, shot: &Shot, layer: &Layer, frame: u32) {
        #[cfg(test)]
        {
            self.rigged_outline.clear();
        }
        let Some(shown) = layer.drawing_at(frame) else { return };
        // A drawing's bounds are found by looking through its pixels: once for a drawing.
        let rect = match &self.rigged_bounds {
            Some((of, rect)) if std::sync::Arc::ptr_eq(of, &shown.drawing) => *rect,
            _ => {
                let rect = shown.drawing.bounds();
                self.rigged_bounds = Some((shown.drawing.clone(), rect));
                rect
            }
        };
        let Some((to_view, _)) = self.bone_space(shot, layer, frame) else { return };
        if rect.is_empty() {
            return;
        }
        let center = painter.clip_rect().center();
        let leant = (!self.flat && shot.tilted(layer.parent, frame)).then(|| shot.projected(layer.parent, frame)).flatten();
        let screen = |x: i32, y: i32| {
            let at = Point::new(f64::from(x) / layer.density, f64::from(y) / layer.density);
            let seen = leant.and_then(|leant| leant.apply(at)).unwrap_or_else(|| to_view.apply(at));
            self.view.to_screen(center, [seen.x, seen.y])
        };
        let corners = [screen(rect.x0, rect.y0), screen(rect.x1, rect.y0), screen(rect.x1, rect.y1), screen(rect.x0, rect.y1)];
        let mut around = corners.to_vec();
        around.push(corners[0]);
        let rigged = crate::theme::Theme::of(painter.ctx()).colour(crate::theme::Role::Rigged);
        painter.extend(egui::Shape::dashed_line(&around, Outline::new(1.8, rigged), 8.0, 5.0));
        #[cfg(test)]
        {
            self.rigged_outline = corners.to_vec();
        }
    }

    /// With *Always Visible* on: the chains of the shot's other rigged layers, as they are
    /// posed on the frame, at a fifth of their strength: a click with the Transform tool on one
    /// selects its layer.
    /// `skip` is the layer whose chain the Bones tool draws whole.
    pub(crate) fn paint_faint_bones(&mut self, painter: &egui::Painter, shot: &Shot, skip: Option<LayerId>, frame: u32) {
        self.faint_chains.clear();
        let mut painter = painter.clone();
        painter.multiply_opacity(FAINT_BONES);
        let center = painter.clip_rect().center();
        let (light, dark) = (Color32::from_white_alpha(210), Color32::from_gray(30));
        let rigged = shot.layers.iter().filter(|layer| Some(layer.id) != skip && !self.hidden.contains(&layer.id) && !shot.hides(layer.id));
        for layer in rigged {
            let Some(chain) = layer.deformer.and_then(|id| shot.chain(id, frame)) else { continue };
            let Some((to_view, _)) = self.bone_space(shot, layer, frame) else { continue };
            let leant = (!self.flat && shot.tilted(layer.parent, frame)).then(|| shot.projected(layer.parent, frame)).flatten();
            let screen = |at: Point| {
                let seen = leant.and_then(|leant| leant.apply(at)).unwrap_or_else(|| to_view.apply(at));
                self.view.to_screen(center, [seen.x, seen.y])
            };
            let on_screen: Vec<Pos2> = chain.joints(Some(frame)).into_iter().map(screen).collect();
            if chain.is_curve() {
                let mut line: Vec<Pos2> = chain.line(Some(frame)).into_iter().map(screen).collect();
                line.extend(line.first().copied().filter(|_| chain.is_envelope()));
                painter.add(egui::Shape::line(line, Outline::new(2.0, dark)));
            } else {
                for ends in on_screen.windows(2) {
                    bone_arrow(&painter, ends[0], ends[1], light.gamma_multiply(0.55), dark);
                }
            }
            for joint in &on_screen {
                painter.circle(*joint, 4.0, Color32::WHITE, Outline::new(1.5, dark));
            }
            self.faint_chains.push((layer.id, on_screen));
        }
    }

    /// Draw the chain of the layer the tool works on: each bone an arrow that is wide where
    /// the bone starts and comes to a point where it ends, so the way the chain runs is seen;
    /// a dot a joint, a square where the chain starts, and the two circles of a round joint.
    pub(crate) fn paint_bones(&mut self, painter: &egui::Painter, shot: &Shot, layer: Option<LayerId>, tools: &Tools) {
        #[cfg(test)]
        {
            self.bone_joints.clear();
            self.bone_circles.clear();
            self.bone_goal = None;
        }
        let center = painter.clip_rect().center();
        let Some(layer) = layer.and_then(|id| shot.layer(id)) else { return };
        if tools.rest() {
            self.paint_rigged(painter, shot, layer, tools.frame);
        }
        let dragged = self.bone_drag.as_ref().or(self.bone_held.as_ref()).filter(|drag| drag.layer == layer.id).map(|drag| &drag.now);
        // Or as Properties has it, while a number of it is dragged there: with the pegs wired
        // into its ports, as the shot has it, since what Properties holds is the chain as kept.
        let typed = self.live_bones.as_ref().filter(|bones| layer.deformer == Some(bones.id)).and_then(|bones| shot.wired(bones, tools.frame));
        let resolved = layer.deformer.and_then(|id| shot.chain(id, tools.frame));
        let Some(chain) = dragged.or(typed.as_deref()).or(resolved.as_deref()) else { return };
        let (Some((to_view, _)), joints) = (self.bone_space(shot, layer, tools.frame), chain.joints((!tools.rest()).then_some(tools.frame))) else { return };
        // On a layer that a peg tilts, seen through the camera, the chain is in perspective as
        // the drawing is: it lies on what it bends.
        let leant = (!self.flat && shot.tilted(layer.parent, tools.frame)).then(|| shot.projected(layer.parent, tools.frame)).flatten();
        let screen = |at: Point| {
            let seen = leant.and_then(|leant| leant.apply(at)).unwrap_or_else(|| to_view.apply(at));
            self.view.to_screen(center, [seen.x, seen.y])
        };
        let on_screen: Vec<Pos2> = joints.iter().map(|joint| screen(*joint)).collect();
        // Dark over light, like the selection's outline. At rest the chain is the orange of
        // what is being set up.
        let (light, dark) = (Color32::from_white_alpha(210), if tools.rest() { Color32::from_rgb(200, 90, 20) } else { Color32::from_gray(30) });
        // The joints' circles are the orange of what is lit in the window.
        let accent = crate::theme::Theme::of(painter.ctx()).accent;
        // The circles of the joints, or of a curve's points, under the rest: the inner one
        // whole, the outer fainter. Those of the selected one are stronger and have a dot each
        // to drag them by, though any of the edge will do; with no circles yet, it has a ring
        // to pull them out by.
        let picked = self.bone_joint.filter(|joint| chain.circled().contains(joint));
        #[cfg(test)]
        let mut circles = Vec::new();
        for joint in chain.circled() {
            let radius = |radius: f64| (screen(Point::new(joints[joint].x + radius, joints[joint].y)) - on_screen[joint]).length();
            let (round, fade) = (radius(chain.circles(joint)[0]), radius(chain.circles(joint)[1]));
            #[cfg(test)]
            circles.push((joint, round, fade));
            let is_picked = picked == Some(joint);
            for (radius, strength) in [(round, 1.0), (fade, 0.55)] {
                let strength = if is_picked { strength } else { strength * 0.55 };
                if radius > 0.0 {
                    painter.circle_stroke(on_screen[joint], radius, Outline::new(1.5, accent.gamma_multiply(strength)));
                    if is_picked {
                        painter.circle(on_screen[joint] - egui::vec2(0.0, radius), 3.5, Color32::WHITE, Outline::new(1.5, accent));
                    }
                }
            }
            if is_picked && fade <= 0.0 {
                painter.circle_stroke(on_screen[joint], RING, Outline::new(1.2, accent.gamma_multiply(0.7)));
                painter.circle(on_screen[joint] - egui::vec2(0.0, RING), 3.5, Color32::WHITE, Outline::new(1.5, accent));
            }
        }
        // A curve is its line, with a dot a point and a square where it starts; the point
        // that is selected is dark.
        if chain.is_curve() {
            let mut line: Vec<Pos2> = chain.line((!tools.rest()).then_some(tools.frame)).into_iter().map(screen).collect();
            // An envelope's line goes round to where it starts.
            line.extend(line.first().copied().filter(|_| chain.is_envelope()));
            painter.add(egui::Shape::line(line, Outline::new(2.0, dark)));
            // The selected point shows its handles, at rest or as they are posed: a line to
            // a small square at the end of each.
            #[cfg(test)]
            self.bone_handles.clear();
            if let Some(point) = picked
                && let Some(handles) = if tools.rest() { chain.handles(point) } else { chain.handles_at(point, tools.frame) }
            {
                for (_out, [x, y]) in shown_handles(chain, point, handles) {
                    let end = screen(Point::new(joints[point].x + x, joints[point].y + y));
                    painter.line_segment([on_screen[point], end], Outline::new(1.2, accent));
                    painter.rect(
                        egui::Rect::from_center_size(end, egui::vec2(7.0, 7.0)),
                        0.0,
                        Color32::WHITE,
                        Outline::new(1.5, accent),
                        egui::StrokeKind::Middle,
                    );
                    #[cfg(test)]
                    self.bone_handles.push((_out, end));
                }
            }
            for (at, point) in on_screen.iter().enumerate() {
                let (fill, edge) = if picked == Some(at) { (dark, Color32::WHITE) } else { (Color32::WHITE, dark) };
                if at == 0 {
                    // A red ring round where the line starts, to be seen and not dragged.
                    painter.circle_stroke(*point, START_RING, Outline::new(1.8, crate::theme::Theme::of(painter.ctx()).colour(crate::theme::Role::ChainStart)));
                    painter.rect(egui::Rect::from_center_size(*point, egui::vec2(9.0, 9.0)), 1.0, fill, Outline::new(1.5, edge), egui::StrokeKind::Middle);
                } else if picked == Some(at) {
                    painter.circle(*point, 5.5, fill, Outline::new(2.0, edge));
                } else {
                    painter.circle(*point, 4.5, fill, Outline::new(1.5, edge));
                }
            }
            #[cfg(test)]
            {
                (self.bone_joints, self.bone_circles) = (on_screen, circles);
            }
            return;
        }
        // Each bone's reach: a dot at places all over the drawing, in the colour of the bone
        // that has most of that place, the fainter the more the others have of it. Where the
        // chain is posed the dots are where the bones carry those places.
        #[cfg(test)]
        self.reach_dots.clear();
        if let (true, Some(drawing)) = (self.show_reach, layer.drawing_at(tools.frame)) {
            let (bounds, density) = (drawing.drawing.bounds(), layer.density);
            let warp = chain.warp(tools.frame, density);
            // About fourteen points of the screen apart, and no more than some thousands.
            let per_pixel = f64::from(self.view.zoom) * to_view.determinant().abs().sqrt() / density;
            let (wide, high) = (f64::from(bounds.width()), f64::from(bounds.height()));
            let step = (14.0 / per_pixel.max(1e-6)).max((wide * high / 3000.0).sqrt()).max(1.0);
            let (columns, rows) = ((wide / step).ceil() as u32, (high / step).ceil() as u32);
            // Only where something is painted: a cell with nothing in it has no dot.
            let key = (std::sync::Arc::as_ptr(&drawing.drawing) as usize, step.to_bits());
            if self.reach_cells.as_ref().is_none_or(|(of, size, _)| (*of, *size) != key) {
                let pixels = drawing.drawing.to_rgba8(bounds, |_| [0, 0, 0, 255]);
                let mut painted = vec![false; (columns * rows) as usize];
                for (index, _) in pixels.iter().enumerate().filter(|(_, pixel)| pixel[3] > 0) {
                    let (x, y) = (index % bounds.width() as usize, index / bounds.width() as usize);
                    let cell = ((y as f64 / step) as u32).min(rows - 1) * columns + ((x as f64 / step) as u32).min(columns - 1);
                    painted[cell as usize] = true;
                }
                self.reach_cells = Some((key.0, key.1, painted));
            }
            let painted = self.reach_cells.as_ref().map(|(.., painted)| painted.clone()).unwrap_or_default();
            for (column, row) in (0..rows).flat_map(|row| (0..columns).map(move |column| (column, row))) {
                if !painted.get((row * columns + column) as usize).copied().unwrap_or(false) {
                    continue;
                }
                let at = Point::new(f64::from(bounds.x0) + (f64::from(column) + 0.5) * step, f64::from(bounds.y0) + (f64::from(row) + 0.5) * step);
                let Some(shares) = warp.shares(at) else { break };
                let Some((bone, most)) = shares.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
                let seen = if tools.rest() { at } else { warp.apply(at) };
                let dot = screen(Point::new(seen.x / density, seen.y / density));
                if painter.clip_rect().contains(dot) {
                    painter.circle_filled(dot, 2.5, reach_colour(bone).gamma_multiply((most as f32 * 1.4 - 0.4).clamp(0.25, 1.0)));
                }
                #[cfg(test)]
                self.reach_dots.push((bone, dot));
            }
        }
        #[cfg(test)]
        {
            self.bone_lit = None;
        }
        for (bone, ends) in on_screen.windows(2).enumerate() {
            if (ends[1] - ends[0]).length() < 0.5 {
                continue;
            }
            // With the reach shown, each bone is in the colour of its dots; the selected one is
            // in the accent's orange, as what is selected is in the window.
            let fill = match (self.show_reach, self.bone_picked == Some(bone) && !chain.is_curve()) {
                (true, _) => reach_colour(bone),
                (false, true) => accent,
                (false, false) => light.gamma_multiply(0.55),
            };
            #[cfg(test)]
            if fill == accent {
                self.bone_lit = Some(bone);
            }
            bone_arrow(painter, ends[0], ends[1], fill, dark);
        }
        for (at, joint) in on_screen.iter().enumerate() {
            if at == 0 {
                painter.rect(
                    egui::Rect::from_center_size(*joint, egui::vec2(9.0, 9.0)),
                    1.0,
                    Color32::WHITE,
                    Outline::new(1.5, dark),
                    egui::StrokeKind::Middle,
                );
            } else if picked == Some(at) {
                painter.circle(*joint, 5.5, dark, Outline::new(2.0, Color32::WHITE));
            } else {
                painter.circle(*joint, 4.5, Color32::WHITE, Outline::new(1.5, dark));
            }
        }
        // The goal of the chain's IK where it is on: a ring with a cross, in the accent's orange.
        if let Some(ik) = chain.ik.as_ref().filter(|ik| !tools.rest() && ik.on_at(tools.frame)) {
            let [x, y] = ik.goal_at(tools.frame);
            let goal = screen(Point::new(x, y));
            painter.circle_stroke(goal, GOAL_RING, Outline::new(3.5, dark));
            painter.circle_stroke(goal, GOAL_RING, Outline::new(2.0, accent));
            for way in [egui::vec2(1.0, 0.0), egui::vec2(0.0, 1.0)] {
                painter.line_segment([goal - way * (GOAL_RING + 4.0), goal - way * 3.0], Outline::new(1.5, accent));
                painter.line_segment([goal + way * 3.0, goal + way * (GOAL_RING + 4.0)], Outline::new(1.5, accent));
            }
            #[cfg(test)]
            {
                self.bone_goal = Some(goal);
            }
        }
        #[cfg(test)]
        {
            (self.bone_joints, self.bone_circles) = (on_screen, circles);
        }
    }
}

/// How strong the chains of the layers not in hand are drawn with *Always Visible* on.
const FAINT_BONES: f32 = 0.2;

/// A bone from `start` to `end`: an arrow, widest a little way along from its start and no
/// wider than suits its length, filled with `fill` and edged `dark`.
fn bone_arrow(painter: &egui::Painter, start: Pos2, end: Pos2, fill: Color32, dark: Color32) {
    let along = end - start;
    let length = along.length();
    if length < 0.5 {
        return;
    }
    let (way, half) = (along / length, (length * 0.12).clamp(2.0, 7.0));
    let shoulder = start + way * (length * 0.18).min(14.0);
    let arrow = vec![start, shoulder + way.rot90() * half, end, shoulder - way.rot90() * half];
    painter.add(egui::Shape::convex_polygon(arrow, fill, Outline::new(1.5, dark)));
}

/// The colour a bone's reach is shown in, by its place in the chain: a few that are told
/// apart, taken round again for a long chain.
fn reach_colour(bone: usize) -> Color32 {
    const COLOURS: [Color32; 6] = [
        Color32::from_rgb(52, 132, 228),
        Color32::from_rgb(236, 148, 32),
        Color32::from_rgb(150, 84, 208),
        Color32::from_rgb(40, 168, 150),
        Color32::from_rgb(220, 72, 130),
        Color32::from_rgb(140, 150, 40),
    ];
    COLOURS[bone % COLOURS.len()]
}
