//! Deformers (plan §4.5, R4): what bends a layer's drawings. The first kind is a chain of
//! bones: each bone holds the part of the drawing that is near it, and when a bone turns that
//! part turns with it, with the parts between two bones shared out between them.
//!
//! The second kind is a curve: a smooth line through a few points, laid along the drawing. What
//! is drawn keeps its place along the line and its distance from it, so when the points are
//! moved and the line bends, the drawing follows it, as a tail or a strand of hair does.
//!
//! The third kind is an envelope: a closed outline through points, laid round a shape. What is
//! drawn is held by all of the outline at once, each part of it counting for more the nearer
//! and the wider it is seen from there, so when its points are moved the shape inside is
//! squashed, stretched and bulged with it, as a belly or a cheek is.
//!
//! A deformer lives in the space of the layers it bends: scene units, before the layers' pegs.
//! It has a *rest* pose, which is where its bones lie on the drawing as it was drawn, and a
//! pose on each frame; where the two are the same the drawing is as it was drawn.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{Affine, Bend, Interp, PegId, Point, Shot, Track};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeformerId(pub u64);

/// One bone of a chain. It starts where the bone before it ends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bone {
    /// At rest: the angle it lies at, in degrees from the bone before it, or for the first
    /// from the x axis; and its length, in scene units.
    pub rest_angle: f64,
    pub length: f64,
    /// How far it is turned from its rest, in degrees: on every frame, unless it has keys.
    #[serde(default)]
    pub turn: f64,
    #[serde(default, skip_serializing_if = "Track::is_empty")]
    pub turns: Track<f64>,
    /// How long it is, as a part of its length at rest: on every frame, unless it has keys.
    /// What it holds is stretched along it.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub stretch: f64,
    #[serde(default, skip_serializing_if = "Track::is_empty")]
    pub stretches: Track<f64>,
    /// How strongly it holds the drawing against the other bones of its chain: 1 as they do,
    /// more and it takes more of what lies between them.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub hold: f64,
    /// How round the joint is that the bone starts at, as two circles about it at rest, in
    /// scene units. Within `round` of the joint what lies between the two bones turns about
    /// the joint by an angle between theirs, and keeps its distance from it; from there the
    /// roundness falls off, to nothing at `fade`, where it goes part of the way with each
    /// bone, which pinches a sharp bend. Both at 0, the joint is not round. The first bone
    /// starts at no joint.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub round: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade: f64,
    /// How much of what lies within that joint's circles is drawn towards the middle of its
    /// bend, from -1 to 1. The middle is half the angle between the two bones: where the
    /// joint is bent by 90°, the line at 45°. At 0.4, what is on either bone's side turns
    /// 40% of the way from where it would be to that line; at 1 all of it turns by half the
    /// angle, as a ball in the joint; below 0 it is pushed from the middle, a sharper bend.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pull: f64,
    /// How sharp the inside of that joint's bend is, from 0 to 1. A round joint leaves a
    /// small round bite in the crook of the bend, where the drawing turns away from the
    /// corner. At 1 what lies in the crook, about the middle of the bend, goes with the bone
    /// it is nearer instead, from either side: the two sides meet in a sharp corner and fold
    /// over each other, and the outside of the bend stays round. At 0 the crook is round too.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub crease: f64,
}

fn one() -> f64 {
    1.0
}

fn is_one(value: &f64) -> bool {
    *value == 1.0
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

/// How sharply a bone's hold on the drawing falls off with distance, unless a chain says
/// otherwise: the higher, the stiffer the parts along the bones and the tighter the bend at a
/// joint.
pub const STIFFNESS: f64 = 4.0;

fn stiffness() -> f64 {
    STIFFNESS
}

fn is_stiffness(value: &f64) -> bool {
    *value == STIFFNESS
}

impl Bone {
    pub fn at_rest(rest_angle: f64, length: f64) -> Bone {
        Bone {
            rest_angle,
            length,
            turn: 0.0,
            turns: Track::default(),
            stretch: 1.0,
            stretches: Track::default(),
            hold: 1.0,
            round: 0.0,
            fade: 0.0,
            pull: 0.0,
            crease: 1.0,
        }
    }

    /// How long the bone is on `frame`, as a part of its length at rest.
    pub fn stretch_at(&self, frame: u32) -> f64 {
        self.stretches.at(frame, false).unwrap_or(self.stretch)
    }

    /// How far the bone is turned from its rest on `frame`.
    pub fn turn_at(&self, frame: u32) -> f64 {
        self.turns.at(frame, false).unwrap_or(self.turn)
    }
}

/// A point of a curve deformer's line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Knot {
    /// Where it lies at rest.
    pub at: [f64; 2],
    /// How far it is moved from its rest: on every frame, unless it has keys.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub moved: [f64; 2],
    #[serde(default, skip_serializing_if = "Track::is_empty")]
    pub moves: Track<[f64; 2]>,
    /// How far to the side of the line the drawing is carried here, as two circles about the
    /// point at rest, in scene units. Within `reach` of the line what is drawn follows it
    /// wholly; from there it follows less, and at `fade` and beyond it stays as it was drawn.
    /// Between two points the reach is between theirs. Both at 0, there is no limit.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reach: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade: f64,
    /// How the line passes through the point at rest: its two handles, each from the point.
    /// The line arrives from the direction of `into` and leaves towards `out`, and the longer
    /// a handle, the wider the line swings on its side. Without them the line passes smoothly,
    /// in the direction from the point before to the point after. When the points are moved,
    /// the handles turn and grow as that direction does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bend: Option<Bend>,
    /// How far each handle is moved from where it is at rest, before it is turned and grown
    /// with the points: on every frame, unless the handles have keys.
    #[serde(default, skip_serializing_if = "is_unbent")]
    pub bent: Bend,
    /// The keys of the two handles, which are always on the same frames.
    #[serde(default, skip_serializing_if = "Track::is_empty")]
    pub bends_into: Track<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Track::is_empty")]
    pub bends_out: Track<[f64; 2]>,
}

fn is_unbent(value: &Bend) -> bool {
    *value == Bend::default()
}

fn is_origin(value: &[f64; 2]) -> bool {
    *value == [0.0; 2]
}

impl Knot {
    pub fn at_rest(at: [f64; 2]) -> Knot {
        Knot {
            at,
            moved: [0.0; 2],
            moves: Track::default(),
            reach: 0.0,
            fade: 0.0,
            bend: None,
            bent: Bend::default(),
            bends_into: Track::default(),
            bends_out: Track::default(),
        }
    }

    /// How far the point's handles are moved from their rest on `frame`.
    pub fn bent_at(&self, frame: u32) -> Bend {
        Bend { into: self.bends_into.at(frame, false).unwrap_or(self.bent.into), out: self.bends_out.at(frame, false).unwrap_or(self.bent.out) }
    }

    /// Whether the handles are posed at all: moved from their rest on any frame.
    fn is_bent(&self) -> bool {
        self.bent != Bend::default() || !self.bends_into.is_empty()
    }

    /// The tracks of the point's pose: of where it is, and of its two handles.
    fn tracks(&self) -> [&Track<[f64; 2]>; 3] {
        [&self.moves, &self.bends_into, &self.bends_out]
    }

    fn tracks_mut(&mut self) -> [&mut Track<[f64; 2]>; 3] {
        [&mut self.moves, &mut self.bends_into, &mut self.bends_out]
    }

    /// How far the point is moved from its rest on `frame`.
    pub fn moved_at(&self, frame: u32) -> [f64; 2] {
        self.moves.at(frame, true).unwrap_or(self.moved)
    }
}

/// How many straight pieces the line of a curve is taken as between two of its points.
pub const PIECES: usize = 12;

/// A chain of bones that bends the layers it is given to, or a curve that does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deformer {
    pub id: DeformerId,
    pub name: String,
    /// Where the first bone starts, at rest.
    pub origin: [f64; 2],
    /// How far the first bone's start is moved from its rest: on every frame, unless it has
    /// keys.
    #[serde(default)]
    pub offset: [f64; 2],
    #[serde(default, skip_serializing_if = "Track::is_empty")]
    pub offsets: Track<[f64; 2]>,
    pub bones: Vec<Bone>,
    /// How sharply the bones' hold falls off with distance: [`STIFFNESS`] unless it is set.
    #[serde(default = "stiffness", skip_serializing_if = "is_stiffness")]
    pub stiffness: f64,
    /// The points of a curve, in their order along it. A deformer that has them is a curve
    /// and has no bones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spine: Vec<Knot>,
    /// The points are an envelope: their line closes on itself, and holds what is within it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub closed: bool,
    /// Of a curve, how far from each of its ends, the first and the last, along its line at
    /// rest, what is drawn is only moved and turned with the line, never stretched: a curve
    /// made longer or shorter stretches the rest of its first and last stretch the more, so
    /// that a round end or a hand keeps its shape. In scene units; at 0 the stretch is even.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub ends: [f64; 2],
    /// What turns the bones by IK instead of their own turns, where it is on (plan §4.18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ik: Option<Ik>,
    /// The peg wired into the start port: the chain's start, or a curve's first point, lies in
    /// that peg's space and goes with it as with a parent, so that a limb's start goes with
    /// the body while its end stays on its goal. A chain's bones turn with the peg too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_peg: Option<PegId>,
    /// How that peg carried what is in its space when it was wired in, from the deformer's
    /// space: the start is carried by how the peg has moved since, so that wiring it in moves
    /// nothing. None, as if it had been wired in at the scene's origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_bind: Option<Affine>,
    /// Of a curve, the peg wired into the end port: its last point is where that peg's pivot
    /// is. A chain's end port is its IK's goal ([`Ik::peg`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_peg: Option<PegId>,
    /// Of a deformer as the shot poses it on a frame ([`Shot::chain`]), not kept: how the peg
    /// wired into its start carries the start, from the deformer's space to where it is; and
    /// where the peg wired into a curve's end puts its last point.
    #[serde(skip)]
    pub carried: Option<Affine>,
    #[serde(skip)]
    pub pinned: Option<[f64; 2]>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A joint of a chain of bones reaching for a goal (plan §4.18): where it is on, the bones
/// from the chain's start up to that joint are turned so that the joint lies on the goal, and
/// those after it go along as their own turns say.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ik {
    /// Whether it is on: on every frame, unless it has keys. Held from one key to the next.
    #[serde(default)]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Track::is_empty")]
    pub ons: Track<bool>,
    /// The joint that reaches, as for a peg that follows the chain: `None` is the chain's end,
    /// whichever that is as bones are added and taken away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<usize>,
    /// Where the goal is, in the layer's own space: on every frame, unless it has keys.
    #[serde(default)]
    pub goal: [f64; 2],
    #[serde(default, skip_serializing_if = "Track::is_empty")]
    pub goals: Track<[f64; 2]>,
    /// A peg wired into the chain's IK port: the goal is where its pivot is, on every frame,
    /// and `goal` is not used. See [`Shot::chain`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peg: Option<PegId>,
    /// Two bones lying straight bend the other way: the second turned back from the first, by
    /// less than nothing, where else it is turned on by more. Two that are bent bend the way
    /// their own turns bend them.
    #[serde(default, skip_serializing_if = "is_false")]
    pub flip: bool,
    /// Out of reach, the bones up to the joint grow along the line to the goal until it is
    /// reached, each by the same part of its length; else they point at it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stretch: bool,
}

impl Ik {
    /// Whether it is on, on `frame`: as the key on it or the last before it says, and before
    /// the first key as that one does, however the keys are eased; in a cycle, as the frame
    /// it repeats.
    pub fn on_at(&self, frame: u32) -> bool {
        let frame = self.ons.cycled(frame).unwrap_or(frame);
        let keys = self.ons.keys();
        keys.iter().rev().find(|key| key.frame <= frame).or(keys.first()).map_or(self.on, |key| key.value)
    }

    /// Where the goal is on `frame`.
    pub fn goal_at(&self, frame: u32) -> [f64; 2] {
        self.goals.at(frame, true).unwrap_or(self.goal)
    }

    /// Switch it on or off: on every frame if it has no keys, else as a key on `frame`, held
    /// till the next.
    pub fn pose_on(&mut self, frame: u32, on: bool) {
        if self.ons.is_empty() {
            self.on = on;
        } else {
            self.ons.set(frame, on);
            self.hold();
        }
    }

    /// Move the goal: on every frame if it has no keys, else as a key on `frame`.
    pub fn pose_goal(&mut self, frame: u32, goal: [f64; 2]) {
        if self.goals.is_empty() { self.goal = goal } else { self.goals.set(frame, goal) }
    }

    /// Have every key of the switch held out of it, as a switch is: what the timeline and the
    /// curves show of it.
    pub fn hold(&mut self) {
        let keys: Vec<u32> = self.ons.keys().iter().map(|key| key.frame).collect();
        for frame in keys {
            if let Some(key) = self.ons.key_mut(frame) {
                key.out = Interp::Hold;
            }
        }
    }
}

/// How many times at most a chain of more than two bones is drawn towards its goal, and how
/// near it it has to come to stop sooner, in scene units.
const IK_PASSES: usize = 64;
const IK_NEAR: f64 = 0.01;

// As for `Peg`: lets `Arc<Deformer>` compare by address first. Its numbers are always finite.
impl Eq for Deformer {}

/// A deformer as it is on one frame, ready to move points.
#[derive(Clone, Debug, PartialEq)]
pub struct Warp {
    bones: Vec<WarpBone>,
    stiffness: f64,
    spine: Option<Spine>,
    /// Where it bends a drawing of another space than its own: what takes that drawing's
    /// pixels into its own, at rest; the point is taken there, moved, and brought back.
    through: Option<Affine>,
    /// A warp the point goes through first: the drawing's own bones, before those of a chain
    /// above it bend it.
    first: Option<Box<Warp>>,
}

/// A curve in a [`Warp`]: its line at rest and as posed, as the same number of points along
/// each, and for each of its own points the two circles of its reach.
#[derive(Clone, Debug, PartialEq)]
struct Spine {
    rest: Vec<Point>,
    posed: Vec<Point>,
    reach: Vec<(f64, f64)>,
    /// The line closes on itself: an envelope.
    closed: bool,
    /// How far from each end what is drawn is not stretched ([`Deformer::ends`]).
    ends: [f64; 2],
}

/// One bone of a [`Warp`].
#[derive(Clone, Debug, PartialEq)]
struct WarpBone {
    /// The line it lies on at rest.
    start: Point,
    end: Point,
    /// How it carries what it holds from the rest to the pose.
    carry: Affine,
    hold: f64,
    /// For a round joint, `carry` in two steps: how what it holds is stretched along it where
    /// it lies at rest, as the rows of a matrix, and then by how much it is turned, in radians.
    stretched: [f64; 4],
    turned: f64,
    /// The circles of the joint it starts at: round within the first, not beyond the second.
    round: f64,
    fade: f64,
    /// How far what is within the circles is drawn towards the middle of the bend.
    pull: f64,
    /// How sharp the crook of the bend is, and the way across the chain at the joint at rest,
    /// which a bend that turns the bone further (clockwise on screen) closes towards.
    crease: f64,
    across: (f64, f64),
}

/// One value of a pose that is keyed apart from the others (plan §4.5): the chain's offset, a
/// bone's turn or length, a point of a curve's move from its rest or one of its handles; the
/// switch of a chain's IK and its goal (§4.18).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PoseValue {
    Offset,
    Turn(usize),
    Stretch(usize),
    Moved(usize),
    BentInto(usize),
    BentOut(usize),
    Ik,
    Goal,
}

impl PoseValue {
    /// What the timeline and the menus call it: bones and points counted from 1.
    pub fn label(self) -> String {
        match self {
            PoseValue::Offset => "Offset".to_string(),
            PoseValue::Turn(bone) => format!("Bone {} Turn", bone + 1),
            PoseValue::Stretch(bone) => format!("Bone {} Length", bone + 1),
            PoseValue::Moved(knot) => format!("Point {}", knot + 1),
            PoseValue::BentInto(knot) => format!("Point {} Handle In", knot + 1),
            PoseValue::BentOut(knot) => format!("Point {} Handle Out", knot + 1),
            PoseValue::Ik => "IK".to_string(),
            PoseValue::Goal => "IK Goal".to_string(),
        }
    }
}

/// Do the same to each of a pose's tracks, with the value it rests at where it has no keys:
/// to all of them, or to the one named.
macro_rules! each_pose_track {
    ($pose:expr, $value:expr, |$track:ident, $rest:ident| $body:expr) => {{
        let (pose, value): (&mut Deformer, Option<PoseValue>) = ($pose, $value);
        let wanted = |of: PoseValue| value.is_none_or(|value| value == of);
        if wanted(PoseValue::Offset) {
            let ($track, $rest) = (&mut pose.offsets, &mut pose.offset);
            $body;
        }
        for (at, bone) in pose.bones.iter_mut().enumerate() {
            if wanted(PoseValue::Turn(at)) {
                let ($track, $rest) = (&mut bone.turns, &mut bone.turn);
                $body;
            }
            if wanted(PoseValue::Stretch(at)) {
                let ($track, $rest) = (&mut bone.stretches, &mut bone.stretch);
                $body;
            }
        }
        for (at, knot) in pose.spine.iter_mut().enumerate() {
            if wanted(PoseValue::Moved(at)) {
                let ($track, $rest) = (&mut knot.moves, &mut knot.moved);
                $body;
            }
            if wanted(PoseValue::BentInto(at)) {
                let ($track, $rest) = (&mut knot.bends_into, &mut knot.bent.into);
                $body;
            }
            if wanted(PoseValue::BentOut(at)) {
                let ($track, $rest) = (&mut knot.bends_out, &mut knot.bent.out);
                $body;
            }
        }
        if let Some(ik) = pose.ik.as_mut() {
            if wanted(PoseValue::Ik) {
                let ($track, $rest) = (&mut ik.ons, &mut ik.on);
                $body;
            }
            if wanted(PoseValue::Goal) {
                let ($track, $rest) = (&mut ik.goals, &mut ik.goal);
                $body;
            }
        }
    }};
}

impl Deformer {
    /// The deformer with every key of its pose `by` frames later.
    pub fn shifted(&self, by: u32) -> Deformer {
        let mut later = self.clone();
        later.offsets = self.offsets.shifted(by);
        for bone in &mut later.bones {
            (bone.turns, bone.stretches) = (bone.turns.shifted(by), bone.stretches.shifted(by));
        }
        for knot in &mut later.spine {
            for track in knot.tracks_mut() {
                *track = track.shifted(by);
            }
        }
        if let Some(ik) = &mut later.ik {
            (ik.ons, ik.goals) = (ik.ons.shifted(by), ik.goals.shifted(by));
        }
        later
    }

    /// A chain at rest, with its bones laid from `origin`.
    pub fn new(id: DeformerId, name: impl Into<String>, origin: [f64; 2], bones: Vec<Bone>) -> Deformer {
        Deformer {
            id,
            name: name.into(),
            origin,
            offset: [0.0; 2],
            offsets: Track::default(),
            bones,
            stiffness: STIFFNESS,
            spine: Vec::new(),
            closed: false,
            ends: [0.0; 2],
            ik: None,
            start_peg: None,
            start_bind: None,
            end_peg: None,
            carried: None,
            pinned: None,
        }
    }

    /// A curve at rest, through `points` in their order.
    pub fn curve(id: DeformerId, name: impl Into<String>, points: &[[f64; 2]]) -> Deformer {
        let spine = points.iter().map(|at| Knot::at_rest(*at)).collect();
        Deformer { spine, origin: points.first().copied().unwrap_or_default(), ..Deformer::new(id, name, [0.0; 2], Vec::new()) }
    }

    /// An envelope at rest, through `points` in their order and back to the first.
    pub fn envelope(id: DeformerId, name: impl Into<String>, points: &[[f64; 2]]) -> Deformer {
        Deformer { closed: true, ..Deformer::curve(id, name, points) }
    }

    /// Whether it is of points, a curve or an envelope, and not a chain of bones.
    pub fn is_curve(&self) -> bool {
        !self.spine.is_empty() || self.closed
    }

    /// Whether it is an envelope: its points' line closes on itself.
    pub fn is_envelope(&self) -> bool {
        self.closed
    }

    /// What kind it is, in a word, and what its rest is called in its commands.
    pub fn kind(&self) -> (&'static str, &'static str) {
        match (self.is_envelope(), self.is_curve()) {
            (true, _) => ("an envelope", "envelope"),
            (false, true) => ("a curve", "curve"),
            (false, false) => ("a chain of bones", "bones"),
        }
    }

    /// The joints of the chain: where each bone starts, and after the last where it ends; of
    /// a curve, its points. With `frame` as they are posed there; without, at rest.
    pub fn joints(&self, frame: Option<u32>) -> Vec<Point> {
        if self.is_curve() {
            let offset = frame.map_or([0.0; 2], |frame| self.offset_at(frame));
            let moved = |knot: &Knot| frame.map_or([0.0; 2], |frame| knot.moved_at(frame));
            let place = |knot: &Knot| Point::new(knot.at[0] + moved(knot)[0] + offset[0], knot.at[1] + moved(knot)[1] + offset[1]);
            let mut points: Vec<Point> = self.spine.iter().map(place).collect();
            // As the pegs wired into its ports have its first and last points.
            if frame.is_some() && !self.closed {
                if let (Some(carried), Some(first)) = (self.carried, points.first_mut()) {
                    *first = carried.apply(*first);
                }
                if let (Some([x, y]), Some(last)) = (self.pinned, points.last_mut()) {
                    *last = Point::new(x, y);
                }
            }
            return points;
        }
        self.frames(frame).into_iter().map(|(start, _)| start).collect()
    }

    /// The line of a curve as points along it, a fixed number between each two of its own:
    /// smooth through them, each passed in the direction from the one before to the one
    /// after. An envelope's line goes on from its last point to its first, and is given
    /// without the first again at its end. With `frame` as it is posed there; without, at rest.
    pub fn line(&self, frame: Option<u32>) -> Vec<Point> {
        let knots = self.joints(frame);
        let count = knots.len();
        if count < 2 || (self.closed && count < 3) {
            return knots;
        }
        let ways = self.ways(frame);
        let stretches = if self.closed { count } else { count - 1 };
        let mut line = Vec::with_capacity(stretches * PIECES + 1);
        for at in 0..stretches {
            let next = (at + 1) % count;
            let (from, to, leaves, arrives) = (knots[at], knots[next], ways[at].1, ways[next].0);
            for piece in 0..PIECES {
                let t = piece as f64 / PIECES as f64;
                let (t2, t3) = (t * t, t * t * t);
                let (a, b, c, d) = (2.0 * t3 - 3.0 * t2 + 1.0, t3 - 2.0 * t2 + t, -2.0 * t3 + 3.0 * t2, t3 - t2);
                line.push(Point::new(a * from.x + b * leaves.0 + c * to.x + d * arrives.0, a * from.y + b * leaves.1 + c * to.y + d * arrives.1));
            }
        }
        if !self.closed {
            line.push(knots[count - 1]);
        }
        line
    }

    /// The way the line passes through each of its points, as two speeds over one stretch:
    /// how it arrives and how it leaves. A point without handles is passed in the direction
    /// from the point before to the one after; one with handles as they say, turned and
    /// grown, where the point itself is moved, as that direction is. A point that is not moved
    /// keeps its handles as they are, whatever the others do (the user's word, 2026-10-15). On
    /// a frame the handles are first moved from their rest by as much as they are posed there.
    fn ways(&self, frame: Option<u32>) -> Vec<((f64, f64), (f64, f64))> {
        let (rest, posed) = (self.smooth(None), self.smooth(frame));
        let moved = self.moved(frame);
        let way = |(at, knot): (usize, &Knot)| {
            let bent = frame.filter(|_| knot.is_bent()).map(|frame| knot.bent_at(frame));
            // Without handles of its own it is passed smoothly: where it is moved, as the
            // points before and after it now lie; where it is not, as at rest.
            if knot.bend.is_none() && bent.is_none() {
                let way = if moved[at] { posed[at] } else { rest[at] };
                return (way, way);
            }
            // The handles at rest, its own or those of the smooth way, and how they are posed.
            let (bend, bent) = (knot.bend.unwrap_or_else(|| smooth_bend(rest[at])), bent.unwrap_or_default());
            let (into, out) = ([bend.into[0] + bent.into[0], bend.into[1] + bent.into[1]], [bend.out[0] + bent.out[0], bend.out[1] + bent.out[1]]);
            let (grown, (sin, cos)) = if moved[at] { carried(rest[at], posed[at]) } else { (1.0, (0.0, 1.0)) };
            let carry = |[x, y]: [f64; 2], by: f64| ((x * cos - y * sin) * grown * by, (x * sin + y * cos) * grown * by);
            // A handle is a third of the speed, as a curve's control point is.
            (carry(into, -3.0), carry(out, 3.0))
        };
        self.spine.iter().enumerate().map(way).collect()
    }

    /// Whether each point is somewhere else on `frame` than at rest.
    fn moved(&self, frame: Option<u32>) -> Vec<bool> {
        let (rest, posed) = (self.joints(None), self.joints(frame));
        rest.iter().zip(&posed).map(|(was, is)| (was.x - is.x).abs() > 1e-9 || (was.y - is.y).abs() > 1e-9).collect()
    }

    /// The smooth way through each point, as a speed over one stretch: in the direction from
    /// the point before to the one after.
    fn smooth(&self, frame: Option<u32>) -> Vec<(f64, f64)> {
        let knots = self.joints(frame);
        let (count, last) = (knots.len(), knots.len().saturating_sub(1));
        let way = |at: usize| {
            let (before, after) = if self.closed { ((at + last) % count, (at + 1) % count) } else { (at.saturating_sub(1), (at + 1).min(last)) };
            let stretches = if !self.closed && (at == 0 || at == last) { 1.0 } else { 2.0 };
            ((knots[after].x - knots[before].x) / stretches, (knots[after].y - knots[before].y) / stretches)
        };
        (0..count).map(way).collect()
    }

    /// The handles of a curve's point at rest, as they are shown: its own, or where it has
    /// none, those of the smooth way through it.
    pub fn handles(&self, knot: usize) -> Option<Bend> {
        let (into, out) = *self.ways(None).get(knot)?;
        Some(self.spine[knot].bend.unwrap_or(Bend { into: [-into.0 / 3.0, -into.1 / 3.0], out: [out.0 / 3.0, out.1 / 3.0] }))
    }

    /// The handles of a curve's point as they are shown on `frame`, each from the point where
    /// it is posed: turned and grown with the points, and moved as they are posed there.
    pub fn handles_at(&self, knot: usize, frame: u32) -> Option<Bend> {
        let (into, out) = *self.ways(Some(frame)).get(knot)?;
        Some(Bend { into: [-into.0 / 3.0, -into.1 / 3.0], out: [out.0 / 3.0, out.1 / 3.0] })
    }

    /// The handles a curve's point would have on `frame` if the line passed it smoothly there,
    /// in the direction from the point before to the one after as they are posed.
    pub fn smooth_handles_at(&self, knot: usize, frame: u32) -> Option<Bend> {
        self.smooth(Some(frame)).get(knot).copied().map(smooth_bend)
    }

    /// Pose the handles of a curve's point, to be shown as `shown` on `frame`: on every frame
    /// if neither the point nor its handles have keys, else as a key there. Where the point
    /// has keys and its handles had none, they get one on each of the point's, as they were,
    /// so that only this frame changes. A point the curve has not got is left alone.
    pub fn pose_bend(&mut self, knot: usize, frame: u32, shown: Bend) {
        if let Some(bent) = self.bend_for(knot, frame, shown) {
            self.set_bent(knot, frame, bent);
        }
    }

    /// How far the handles of a curve's point are to be moved from their rest to be shown as
    /// `shown` on `frame`: what [`Deformer::pose_bend`] keeps. `None` for a point the curve
    /// has not got.
    pub fn bend_for(&self, knot: usize, frame: u32, shown: Bend) -> Option<Bend> {
        let (rest, posed, at_rest) = (self.smooth(None).get(knot).copied()?, self.smooth(Some(frame)).get(knot).copied()?, self.handles(knot)?);
        // Back from how the handles are turned and grown with the point, to the rest.
        let moved = self.moved(Some(frame)).get(knot).copied()?;
        let (grown, (sin, cos)) = if moved { carried(rest, posed) } else { (1.0, (0.0, 1.0)) };
        let back = |[x, y]: [f64; 2], rest: [f64; 2]| [(x * cos + y * sin) / grown - rest[0], (y * cos - x * sin) / grown - rest[1]];
        Some(Bend { into: back(shown.into, at_rest.into), out: back(shown.out, at_rest.out) })
    }

    /// Put the handles of a curve's point back to those of its rest on `frame`, keyed or
    /// not as [`Deformer::pose_bend`] has it.
    pub fn rest_bend(&mut self, knot: usize, frame: u32) {
        if knot < self.spine.len() {
            self.set_bent(knot, frame, Bend::default());
        }
    }

    fn set_bent(&mut self, knot: usize, frame: u32, bent: Bend) {
        // The handles as they are shown on each of the point's keys, held there as they are:
        // a point that is not moved keeps them, whatever the others do since.
        let first = self.spine[knot].bends_into.is_empty() && !self.spine[knot].moves.is_empty();
        let shown: Vec<Bend> = match first {
            true => {
                let frames: Vec<u32> = self.spine[knot].moves.keys().iter().map(|key| key.frame).collect();
                let held = |frame: u32| self.handles_at(knot, frame).and_then(|shown| self.bend_for(knot, frame, shown));
                frames.into_iter().map(|frame| held(frame).unwrap_or(self.spine[knot].bent)).collect()
            }
            false => Vec::new(),
        };
        let knot = &mut self.spine[knot];
        if knot.moves.is_empty() && knot.bends_into.is_empty() {
            knot.bent = bent;
            return;
        }
        if knot.bends_into.is_empty() {
            for (key, was) in knot.moves.keys().to_vec().into_iter().zip(shown) {
                for (track, value) in [(&mut knot.bends_into, was.into), (&mut knot.bends_out, was.out)] {
                    track.set(key.frame, value);
                    if let Some(made) = track.key_mut(key.frame) {
                        (made.into, made.out) = (key.into, key.out);
                    }
                }
            }
        }
        knot.bends_into.set(frame, bent.into);
        knot.bends_out.set(frame, bent.out);
    }

    /// Put a point into a curve after the one at `after`, at `at` or else on the line half
    /// way to the next. It is moved, on each frame, by as much as the two beside it are
    /// between them, so the posed line keeps its shape.
    pub fn insert_knot(&mut self, after: usize, at: Option<[f64; 2]>) {
        let count = self.spine.len();
        if after >= count || (!self.closed && after + 1 >= count) {
            return;
        }
        let next = (after + 1) % count;
        let at = at.unwrap_or_else(|| self.line(None).get(after * PIECES + PIECES / 2).map_or(self.spine[after].at, |at| [at.x, at.y]));
        let (before, beyond) = (&self.spine[after], &self.spine[next]);
        let between = |frame: u32| {
            let (a, b) = (before.moved_at(frame), beyond.moved_at(frame));
            [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
        };
        let mut knot = Knot { moved: [(before.moved[0] + beyond.moved[0]) / 2.0, (before.moved[1] + beyond.moved[1]) / 2.0], ..Knot::at_rest(at) };
        let frames: BTreeSet<u32> = before.moves.keys().iter().chain(beyond.moves.keys()).map(|key| key.frame).collect();
        for frame in frames {
            knot.moves.set(frame, between(frame));
        }
        (knot.reach, knot.fade) = ((before.reach + beyond.reach) / 2.0, (before.fade + beyond.fade) / 2.0);
        self.spine.insert(after + 1, knot);
    }

    /// Take a point out of a curve. A curve keeps two points and an envelope three.
    pub fn remove_knot(&mut self, knot: usize) {
        if knot < self.spine.len() && self.spine.len() > if self.closed { 3 } else { 2 } {
            self.spine.remove(knot);
            self.origin = self.spine.first().map_or(self.origin, |first| first.at);
        }
    }

    /// The places that have two circles: of a chain the joints between two bones, by the
    /// place of the bone that starts there; of a curve every point.
    pub fn circled(&self) -> std::ops::Range<usize> {
        if self.is_curve() { 0..self.spine.len() } else { 1..self.bones.len().max(1) }
    }

    /// The radii of the two circles at a place: a joint's `round` and `fade`, or a curve's
    /// point's `reach` and `fade`.
    pub fn circles(&self, at: usize) -> [f64; 2] {
        match (self.spine.get(at), self.bones.get(at)) {
            (Some(knot), _) => [knot.reach, knot.fade],
            (None, Some(bone)) => [bone.round, bone.fade],
            (None, None) => [0.0; 2],
        }
    }

    /// Set the radii of the two circles at a place. The second is never less than the first.
    pub fn set_circles(&mut self, at: usize, [inner, outer]: [f64; 2]) {
        let outer = outer.max(inner);
        if let Some(knot) = self.spine.get_mut(at) {
            (knot.reach, knot.fade) = (inner, outer);
        } else if let Some(bone) = self.bones.get_mut(at) {
            (bone.round, bone.fade) = (inner, outer);
        }
    }

    /// Move a point of a curve to `to` on `frame`: on every frame if it has no keys, else as
    /// a key there. A point the curve has not got is left alone.
    pub fn pose_knot(&mut self, knot: usize, frame: u32, to: [f64; 2]) {
        let offset = self.offset_at(frame);
        if let Some(knot) = self.spine.get_mut(knot) {
            let moved = [to[0] - knot.at[0] - offset[0], to[1] - knot.at[1] - offset[1]];
            if knot.moves.is_empty() { knot.moved = moved } else { knot.moves.set(frame, moved) }
        }
    }

    /// How a point of the deformer is carried from its rest to its pose on `frame`, as a
    /// transform of its space: what lies at the point at rest goes where the pose has it,
    /// turned as the bone that ends there (or the first, at the chain's start) or the point's
    /// handle is turned, and grown as that bone is stretched or the line between the points
    /// has grown; a handle's length grows nothing (the user's word, 2026-10-30). A handle is
    /// the one the line arrives by, or at an open curve's start the one it leaves by; turned
    /// from its rest, by a pose or with its point, so what follows it starts where it was
    /// set up. `point` `None` is the last. `None` where there is no such point.
    pub fn carry(&self, point: Option<usize>, frame: u32) -> Option<Affine> {
        let (rest, posed) = (self.joints(None), self.joints(Some(frame)));
        let point = point.unwrap_or(rest.len().checked_sub(1)?);
        let (from, to) = (*rest.get(point)?, *posed.get(point)?);
        let (turned, grown) = if self.is_curve() {
            let (grown, _) = carried(self.smooth(None)[point], self.smooth(Some(frame))[point]);
            let handle = |(into, out): ((f64, f64), (f64, f64))| if point == 0 && !self.closed { out } else { into };
            let (_, (sin, cos)) = carried(handle(self.ways(None)[point]), handle(self.ways(Some(frame))[point]));
            (sin.atan2(cos), grown)
        } else {
            // The bone that ends at the joint, or the first where the chain starts.
            let bone = point.saturating_sub(1);
            let (was, is) = (self.frames(None), self.frames(Some(frame)));
            let grown = if point == 0 { 1.0 } else { self.posed(frame).get(bone).map_or(1.0, |(_, stretch)| *stretch) };
            (is.get(bone).map_or(0.0, |is| is.1) - was.get(bone).map_or(0.0, |was| was.1), grown)
        };
        Some(Affine::translate(to.x, to.y).mul(&Affine::rotate(turned)).mul(&Affine::scale(grown)).mul(&Affine::translate(-from.x, -from.y)))
    }

    /// For each bone, where it starts and the angle it lies at, in radians; and last the end
    /// of the chain.
    fn frames(&self, frame: Option<u32>) -> Vec<(Point, f64)> {
        let (at, angle) = frame.map_or((Point::new(self.origin[0], self.origin[1]), 0.0), |frame| self.start_at(frame));
        let posed = frame.map_or_else(|| vec![(0.0, 1.0); self.bones.len()], |frame| self.posed(frame));
        walk(at, angle, &self.bones, &posed)
    }

    /// Where the chain starts on `frame`, moved from its rest by its offset and carried by the
    /// peg wired into its start; and the angle, in radians, its first bone is laid from.
    fn start_at(&self, frame: u32) -> (Point, f64) {
        let offset = self.offset_at(frame);
        let at = Point::new(self.origin[0] + offset[0], self.origin[1] + offset[1]);
        self.carried.map_or((at, 0.0), |carried| (carried.apply(at), carried.m[1].atan2(carried.m[0])))
    }

    /// Each bone's turn from its rest, in degrees, and its stretch, on `frame`: as they are keyed,
    /// or where IK is on, as it turns and grows the bones up to its joint.
    pub fn posed(&self, frame: u32) -> Vec<(f64, f64)> {
        let mut posed: Vec<(f64, f64)> = self.bones.iter().map(|bone| (bone.turn_at(frame), bone.stretch_at(frame))).collect();
        let Some(ik) = self.ik.as_ref().filter(|ik| ik.on_at(frame) && !self.is_curve()) else { return posed };
        let reaching = ik.point.unwrap_or(self.bones.len()).min(self.bones.len());
        if reaching == 0 {
            return posed;
        }
        let (start, laid) = self.start_at(frame);
        let goal = ik.goal_at(frame);
        let goal = Point::new(goal[0], goal[1]);
        let mut joints: Vec<Point> = walk(start, laid, &self.bones, &posed).into_iter().map(|(at, _)| at).take(reaching + 1).collect();
        let mut lengths: Vec<f64> = self.bones[..reaching].iter().zip(&posed).map(|(bone, (_, stretch))| bone.length * stretch).collect();
        let (far, reach) = ((goal.x - start.x).hypot(goal.y - start.y), lengths.iter().sum::<f64>());
        if ik.stretch && far > reach && reach > 1e-9 {
            let grown = far / reach;
            lengths.iter_mut().for_each(|length| *length *= grown);
            posed[..reaching].iter_mut().for_each(|(_, stretch)| *stretch *= grown);
        }
        solve(&mut joints, &lengths, goal, ik.flip);
        // The turns back from where the joints are: each bone's angle from the one before, less
        // its rest, the nearest way round to its own turn, so a turn keyed past a full circle
        // stays as near it as it can.
        let mut before = laid.to_degrees();
        for (at, bone) in self.bones[..reaching].iter().enumerate() {
            let (from, to) = (joints[at], joints[at + 1]);
            let angle = if (to.x - from.x).hypot(to.y - from.y) > 1e-12 {
                (to.y - from.y).atan2(to.x - from.x).to_degrees()
            } else {
                before + bone.rest_angle + posed[at].0
            };
            let turn = angle - before - bone.rest_angle;
            let own = posed[at].0;
            posed[at].0 = own + (turn - own + 180.0).rem_euclid(360.0) - 180.0;
            before = angle;
        }
        posed
    }

    /// The deformer on `frame`, in the units of a layer that has `density` drawing pixels to
    /// a scene unit: what moves the pixels of that layer's drawings.
    pub fn warp(&self, frame: u32, density: f64) -> Warp {
        if self.is_curve() {
            let scaled = |line: Vec<Point>| line.into_iter().map(|at| Point::new(at.x * density, at.y * density)).collect();
            let reach = self.spine.iter().map(|knot| (knot.reach * density, knot.fade * density)).collect();
            let ends = self.ends.map(|end| end * density);
            let spine = Some(Spine { rest: scaled(self.line(None)), posed: scaled(self.line(Some(frame))), reach, closed: self.closed, ends });
            return Warp { bones: Vec::new(), stiffness: self.stiffness, spine, through: None, first: None };
        }
        let (rest, posed, stretches) = (self.frames(None), self.frames(Some(frame)), self.posed(frame));
        let place = |(at, angle): (Point, f64)| Affine::translate(at.x * density, at.y * density).mul(&Affine::rotate(angle));
        let bones = self.bones.iter().enumerate().map(|(at, bone)| {
            let (start, end) = (rest[at].0, rest[at + 1].0);
            // Stretched along the bone, in the bone's own space.
            let stretch = stretches[at].1;
            let along = Affine { m: [stretch, 0.0, 0.0, 1.0, 0.0, 0.0] };
            // From the rest to the bone's own space, and from there to where it is posed.
            let carry = place(rest[at]).inverse().map_or(Affine::IDENTITY, |back| place(posed[at]).mul(&along).mul(&back));
            // The same stretch where the bone lies at rest: c²s + s², cs(s − 1), s²s + c².
            let (sin, cos) = rest[at].1.sin_cos();
            let across = cos * sin * (stretch - 1.0);
            WarpBone {
                start: Point::new(start.x * density, start.y * density),
                end: Point::new(end.x * density, end.y * density),
                carry,
                hold: bone.hold,
                stretched: [cos * cos * stretch + sin * sin, across, across, sin * sin * stretch + cos * cos],
                turned: posed[at].1 - rest[at].1,
                round: bone.round * density,
                fade: bone.fade * density,
                pull: bone.pull,
                crease: bone.crease,
                across: {
                    // Square to the way the chain runs through the joint: between the two bones.
                    let through = (rest[at.saturating_sub(1)].1 + rest[at].1) / 2.0;
                    (-through.sin(), through.cos())
                },
            }
        });
        Warp { bones: bones.collect(), stiffness: self.stiffness, spine: None, through: None, first: None }
    }

    /// How far the chain's start is moved from its rest on `frame`.
    pub fn offset_at(&self, frame: u32) -> [f64; 2] {
        self.offsets.at(frame, true).unwrap_or(self.offset)
    }

    /// Move the chain's start from its rest: on every frame if the offset has no keys, else
    /// as a key on `frame`.
    pub fn pose_offset(&mut self, frame: u32, offset: [f64; 2]) {
        if self.offsets.is_empty() { self.offset = offset } else { self.offsets.set(frame, offset) }
    }

    /// Turn a bone from its rest: on every frame if its turn has no keys, else as a key on
    /// `frame`. A bone the chain has not got is left alone.
    pub fn pose_turn(&mut self, bone: usize, frame: u32, turn: f64) {
        if let Some(bone) = self.bones.get_mut(bone) {
            if bone.turns.is_empty() { bone.turn = turn } else { bone.turns.set(frame, turn) }
        }
    }

    /// Stretch a bone, to a part of its length at rest: on every frame if its length has no
    /// keys, else as a key on `frame`.
    pub fn pose_stretch(&mut self, bone: usize, frame: u32, stretch: f64) {
        if let Some(bone) = self.bones.get_mut(bone) {
            if bone.stretches.is_empty() { bone.stretch = stretch } else { bone.stretches.set(frame, stretch) }
        }
    }

    /// The chain with its rest laid through `joints`: where it starts, then where each bone
    /// ends. The bones it has keep their poses; those it gains are at rest, and no bone is
    /// shorter than `shortest`.
    pub fn with_rest_joints(&self, joints: &[Point], shortest: f64) -> Deformer {
        let mut laid = self.clone();
        let Some(first) = joints.first() else { return laid };
        laid.origin = [first.x, first.y];
        // A curve is laid through the points themselves; those it has keep how they are moved.
        if self.is_curve() {
            let mut was = std::mem::take(&mut laid.spine).into_iter();
            laid.spine = joints.iter().map(|at| was.next().map_or(Knot::at_rest([at.x, at.y]), |knot| Knot { at: [at.x, at.y], ..knot })).collect();
            return laid;
        }
        let (mut was, mut before) = (std::mem::take(&mut laid.bones).into_iter(), 0.0f64);
        for ends in joints.windows(2) {
            let (along, up) = (ends[1].x - ends[0].x, ends[1].y - ends[0].y);
            let (angle, length) = (up.atan2(along).to_degrees(), along.hypot(up).max(shortest));
            // The angle from the bone before, the short way round.
            let rest_angle = (angle - before + 180.0).rem_euclid(360.0) - 180.0;
            laid.bones.push(match was.next() {
                Some(bone) => Bone { rest_angle, length, ..bone },
                None => Bone::at_rest(rest_angle, length),
            });
            before = angle;
        }
        laid
    }

    /// The chain with one joint less: `joint` 0 is where it starts, and each one after is
    /// where a bone ends. The first joint taken away, the chain starts at the first bone's
    /// end; the last, it ends at the last bone's start; one between, the bone that ends there
    /// reaches to the end of the bone that started there, which goes, with its pose and what
    /// was set of the joint. The other bones lie as they did and keep their poses. `None` for
    /// a curve, for a chain of one bone, and for a joint the chain has not.
    pub fn without_joint(&self, joint: usize) -> Option<Deformer> {
        if self.is_curve() || self.bones.len() < 2 || joint > self.bones.len() {
            return None;
        }
        let mut joints = self.joints(None);
        joints.remove(joint);
        let mut less = self.clone();
        // The bone that goes: the one that started at the joint, or the last for the last joint.
        less.bones.remove(joint.min(self.bones.len() - 1));
        Some(less.with_rest_joints(&joints, 0.0))
    }

    /// The frames on which the pose has a key, of any of its values.
    pub fn key_frames(&self) -> BTreeSet<u32> {
        self.key_frames_of(None)
    }

    /// The frames on which the pose has a key: of any of its values, or of the one named.
    pub fn key_frames_of(&self, value: Option<PoseValue>) -> BTreeSet<u32> {
        let mut frames = BTreeSet::new();
        let mut pose = self.clone();
        each_pose_track!(&mut pose, value, |track, _rest| frames.extend(track.keys().iter().map(|key| key.frame)));
        frames
    }

    /// Key the pose on `frame`, as it is there: every bone's turn and length, and the chain's
    /// offset; of a curve every point, and the handles of those whose handles are posed; the
    /// IK's switch and goal, where the chain has IK.
    pub fn key_all(&mut self, frame: u32) {
        let offset = self.offsets.at(frame, true).unwrap_or(self.offset);
        self.offsets.set(frame, offset);
        for bone in &mut self.bones {
            let (turn, stretch) = (bone.turn_at(frame), bone.stretch_at(frame));
            bone.turns.set(frame, turn);
            bone.stretches.set(frame, stretch);
        }
        for knot in &mut self.spine {
            let (moved, bent) = (knot.moved_at(frame), knot.bent_at(frame));
            knot.moves.set(frame, moved);
            if knot.is_bent() {
                knot.bends_into.set(frame, bent.into);
                knot.bends_out.set(frame, bent.out);
            }
        }
        if let Some(ik) = &mut self.ik {
            let (on, goal) = (ik.on_at(frame), ik.goal_at(frame));
            ik.ons.set(frame, on);
            ik.hold();
            ik.goals.set(frame, goal);
        }
    }

    /// Take the keys on `frame` away. A value that loses its last key stays as that key had
    /// it.
    pub fn remove_keys(&mut self, frame: u32) {
        self.remove_keys_of(frame, None);
    }

    /// [`Deformer::remove_keys`], of every value or of the one named.
    pub fn remove_keys_of(&mut self, frame: u32, value: Option<PoseValue>) {
        each_pose_track!(self, value, |track, rest| if let (Some(key), true) = (track.remove(frame), track.is_empty()) {
            *rest = key.value;
        });
    }

    /// Move the keys on `from` to `to`, over any that are there.
    pub fn move_keys(&mut self, from: u32, to: u32) {
        self.move_keys_of(from, to, None);
    }

    /// [`Deformer::move_keys`], of every value or of the one named.
    pub fn move_keys_of(&mut self, from: u32, to: u32, value: Option<PoseValue>) {
        each_pose_track!(self, value, |track, _rest| track.move_key(from, to));
    }

    /// Whether any value of the pose moves on after `frame`, towards a later key of its own.
    pub fn moves_after(&self, frame: u32) -> bool {
        self.moves_after_of(frame, None)
    }

    /// [`Deformer::moves_after`], of any value or of the one named.
    pub fn moves_after_of(&self, frame: u32, value: Option<PoseValue>) -> bool {
        let mut moves = false;
        let mut pose = self.clone();
        each_pose_track!(&mut pose, value, |track, _rest| moves |= track.moves_after(frame));
        moves
    }

    /// How the keys on `frame` ease, in and out: those of the first value that has a key there.
    pub fn interp(&self, frame: u32) -> Option<(Interp, Interp)> {
        self.interp_of(frame, None)
    }

    /// [`Deformer::interp`], of the first value that has a key there or of the one named.
    pub fn interp_of(&self, frame: u32, value: Option<PoseValue>) -> Option<(Interp, Interp)> {
        let mut found = None;
        let mut pose = self.clone();
        each_pose_track!(&mut pose, value, |track, _rest| found = found.or(track.key(frame).map(|key| (key.into, key.out))));
        found
    }

    /// Set how the keys on `frame` ease; a side that is `None` stays as it is.
    pub fn set_interp(&mut self, frame: u32, into: Option<Interp>, out: Option<Interp>) {
        self.set_interp_of(frame, None, into, out);
    }

    /// [`Deformer::set_interp`], of every value or of the one named.
    pub fn set_interp_of(&mut self, frame: u32, value: Option<PoseValue>, into: Option<Interp>, out: Option<Interp>) {
        each_pose_track!(self, value, |track, _rest| if let Some(key) = track.key_mut(frame) {
            (key.into, key.out) = (into.unwrap_or(key.into), out.unwrap_or(key.out));
        });
    }

    /// The values of the pose that can be keyed, in order: the chain's offset, each bone's
    /// turn and length; each point of a curve, and the handles of those that have them; the IK's
    /// switch and goal.
    pub fn pose_values(&self) -> Vec<PoseValue> {
        let mut values = Vec::new();
        if !self.bones.is_empty() {
            values.push(PoseValue::Offset);
        }
        for bone in 0..self.bones.len() {
            values.extend([PoseValue::Turn(bone), PoseValue::Stretch(bone)]);
        }
        for (at, knot) in self.spine.iter().enumerate() {
            values.push(PoseValue::Moved(at));
            if knot.is_bent() || !knot.bends_into.is_empty() {
                values.extend([PoseValue::BentInto(at), PoseValue::BentOut(at)]);
            }
        }
        if self.ik.is_some() {
            values.extend([PoseValue::Ik, PoseValue::Goal]);
        }
        values
    }

    /// Whether anything of the deformer has keys.
    pub fn is_animated(&self) -> bool {
        let bones = self.bones.iter().any(|bone| !bone.turns.is_empty() || !bone.stretches.is_empty());
        let ik = self.ik.as_ref().is_some_and(|ik| !ik.ons.is_empty() || !ik.goals.is_empty());
        !self.offsets.is_empty() || bones || ik || self.spine.iter().flat_map(|knot| knot.tracks()).any(|track| !track.is_empty())
    }
}

/// For each bone, from `at`, where it starts and the angle it lies at, in radians, each turned
/// and stretched as `posed` says, the first from `angle`; and last the end of the chain.
fn walk(mut at: Point, mut angle: f64, bones: &[Bone], posed: &[(f64, f64)]) -> Vec<(Point, f64)> {
    let mut frames = Vec::with_capacity(bones.len() + 1);
    for (bone, (turn, stretch)) in bones.iter().zip(posed) {
        angle += (bone.rest_angle + turn).to_radians();
        frames.push((at, angle));
        let length = bone.length * stretch;
        at = Point::new(at.x + length * angle.cos(), at.y + length * angle.sin());
    }
    frames.push((at, angle));
    frames
}

/// Move `joints`, a chain's start and the ends of the bones of `lengths` as they are posed,
/// so that the last lies on `goal`, or as near it as the bones reach, the start staying. One
/// bone points at the goal. Two are turned exactly, the joint between them staying on the side
/// it is on, or where they lie straight on the side `flip` says. More are drawn to the goal and
/// back to the start in turn (FABRIK) from where they are, so they bend as they were bent.
fn solve(joints: &mut [Point], lengths: &[f64], goal: Point, flip: bool) {
    let start = joints[0];
    let way = |from: Point, to: Point, fallback: (f64, f64)| {
        let (x, y) = (to.x - from.x, to.y - from.y);
        let length = x.hypot(y);
        if length > 1e-12 { (x / length, y / length) } else { fallback }
    };
    let along = |from: Point, (x, y): (f64, f64), length: f64| Point::new(from.x + x * length, from.y + y * length);
    let far = (goal.x - start.x).hypot(goal.y - start.y);
    let reach: f64 = lengths.iter().sum();
    // Of the first bone as it lies, for a goal on the start.
    let first = way(start, joints[1], (1.0, 0.0));
    let towards = way(start, goal, first);
    if far >= reach || lengths.len() == 1 {
        let mut at = start;
        for (joint, length) in joints[1..].iter_mut().zip(lengths) {
            at = along(at, towards, *length);
            *joint = at;
        }
        return;
    }
    if let [a, b] = *lengths {
        // The joint's side: as the two bones bend, or as `flip` says where they lie straight.
        let (bent_x, bent_y) = (joints[1].x - start.x, joints[1].y - start.y);
        let (end_x, end_y) = (joints[2].x - start.x, joints[2].y - start.y);
        let cross = bent_x * end_y - bent_y * end_x;
        let side = if cross.abs() > 1e-9 * a * b {
            -cross.signum()
        } else if flip {
            1.0
        } else {
            -1.0
        };
        // The law of cosines: the angle at the start between the goal and the first bone.
        let cos = ((a * a + far * far - b * b) / (2.0 * a * far.max(1e-12))).clamp(-1.0, 1.0);
        let turned = side * cos.acos();
        let (sin, cos) = turned.sin_cos();
        let elbow = (towards.0 * cos - towards.1 * sin, towards.0 * sin + towards.1 * cos);
        joints[1] = along(start, elbow, a);
        joints[2] = along(joints[1], way(joints[1], goal, elbow), b);
        return;
    }
    let last = joints.len() - 1;
    for _ in 0..IK_PASSES {
        joints[last] = goal;
        for at in (0..last).rev() {
            let back = way(joints[at + 1], joints[at], (-towards.0, -towards.1));
            joints[at] = along(joints[at + 1], back, lengths[at]);
        }
        joints[0] = start;
        for at in 0..last {
            let on = way(joints[at], joints[at + 1], towards);
            joints[at + 1] = along(joints[at], on, lengths[at]);
        }
        if (joints[last].x - goal.x).hypot(joints[last].y - goal.y) < IK_NEAR {
            break;
        }
    }
}

/// The handles of the smooth `way` through a point: a third of the speed, each way.
fn smooth_bend(way: (f64, f64)) -> Bend {
    Bend { into: [-way.0 / 3.0, -way.1 / 3.0], out: [way.0 / 3.0, way.1 / 3.0] }
}

/// How the smooth way through a point has grown and turned from the rest to the pose, as the
/// handles do with it: by how much, and the sine and cosine of the angle.
fn carried(was: (f64, f64), is: (f64, f64)) -> (f64, (f64, f64)) {
    let (length, turned) = (was.0.hypot(was.1), is.1.atan2(is.0) - was.1.atan2(was.0));
    let grown = if length > 1e-9 { is.0.hypot(is.1) / length } else { 1.0 };
    if length > 1e-9 && grown > 1e-9 { (grown, turned.sin_cos()) } else { (1.0, (0.0, 1.0)) }
}

impl Warp {
    /// A number that is the same for the same warp, and with near certainty another for
    /// another: what a compositor keeps a bent drawing under.
    pub fn key(&self) -> u64 {
        let of = |bone: &WarpBone| {
            [bone.start.x, bone.start.y, bone.end.x, bone.end.y, bone.hold, bone.turned, bone.round, bone.fade, bone.pull, bone.crease]
                .into_iter()
                .chain(bone.carry.m)
        };
        let spine = self.spine.iter().flat_map(|spine| spine.rest.iter().chain(&spine.posed)).flat_map(|at| [at.x, at.y]);
        let reach =
            self.spine.iter().flat_map(|spine| spine.reach.iter().copied().chain([(spine.ends[0], spine.ends[1])])).flat_map(|(reach, fade)| [reach, fade]);
        let through = self.through.iter().flat_map(|to| to.m);
        let first = self.first.iter().map(|first| f64::from_bits(first.key()));
        let numbers = self.bones.iter().flat_map(of).chain([self.stiffness]).chain(spine).chain(reach).chain(through).chain(first);
        numbers.fold(0xcbf2_9ce4_8422_2325u64, |sum, number| {
            number.to_bits().to_le_bytes().iter().fold(sum, |sum, byte| (sum ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3))
        })
    }

    /// Whether it leaves every point where it is.
    pub fn is_still(&self) -> bool {
        let near = |(a, b): (&Point, &Point)| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9;
        self.spine.as_ref().is_none_or(|spine| spine.rest.iter().zip(&spine.posed).all(near))
            && self.bones.iter().all(|bone| bone.carry.m.iter().zip(Affine::IDENTITY.m).all(|(a, b)| (a - b).abs() < 1e-12))
            && self.first.as_ref().is_none_or(|first| first.is_still())
    }

    /// This warp bending a drawing of another space: `to` takes that drawing's pixels into
    /// the pixels this warp is made in, at rest (a chain above the drawing's peg, plan §4.5).
    pub fn through(mut self, to: Affine) -> Warp {
        self.through = Some(to);
        self
    }

    /// This warp after `first`: a point goes through `first`, the drawing's own bones, and then
    /// through this one.
    pub fn after(mut self, first: Warp) -> Warp {
        self.first = Some(Box::new(first));
        self
    }

    /// How much of a point of the drawing each bone has, as parts of one, in the bones' order:
    /// its reach. It is what [`Warp::apply`] shares the point out by, before the joints are
    /// made round. `None` for a curve or an envelope, which has no bones.
    pub fn shares(&self, at: Point) -> Option<Vec<f64>> {
        if self.spine.is_some() || self.bones.is_empty() {
            return None;
        }
        let weights: Vec<f64> = self
            .bones
            .iter()
            .map(|bone| {
                let (along, to) = ((bone.end.x - bone.start.x, bone.end.y - bone.start.y), (at.x - bone.start.x, at.y - bone.start.y));
                let length = along.0 * along.0 + along.1 * along.1;
                let part = if length > 0.0 { ((to.0 * along.0 + to.1 * along.1) / length).clamp(0.0, 1.0) } else { 0.0 };
                let away = (to.0 - along.0 * part).hypot(to.1 - along.1 * part).max(1e-3);
                bone.hold * away.powf(-self.stiffness)
            })
            .collect();
        let whole: f64 = weights.iter().sum();
        (whole > 0.0).then(|| weights.iter().map(|weight| weight / whole).collect())
    }

    /// The point of the drawing that goes to `to`: [`Warp::apply`] taken back, which is how a
    /// pen on a bent drawing finds the pixel it is over. Found by steps, each as far as the
    /// warp is straight near the point; where several points go to one place it is one of
    /// them, the one nearest to what a single bone carries there.
    pub fn undo(&self, to: Point) -> Point {
        // Away from the bones several points can go to the same place. The one that is looked
        // for is the one a bone carries there, or nearly: so the search starts from where
        // each bone alone would have brought the point from, at the one that comes nearest.
        let off = |at: Point| {
            let now = self.apply(at);
            (now.x - to.x).hypot(now.y - to.y)
        };
        let alone = self.bones.iter().filter_map(|bone| Some(bone.carry.inverse()?.apply(to)));
        let mut at = alone.fold(to, |best, from| if off(from) < off(best) { from } else { best });
        for _ in 0..16 {
            let now = self.apply(at);
            let (off_x, off_y) = (now.x - to.x, now.y - to.y);
            if off_x.hypot(off_y) < 1e-3 {
                break;
            }
            // How the warp moves a point near here: by what a step each way does.
            let (right, down) = (self.apply(Point::new(at.x + 0.5, at.y)), self.apply(Point::new(at.x, at.y + 0.5)));
            let (a, b, c, d) = ((right.x - now.x) * 2.0, (down.x - now.x) * 2.0, (right.y - now.y) * 2.0, (down.y - now.y) * 2.0);
            let turn = a * d - b * c;
            if turn.abs() < 1e-9 {
                break;
            }
            at = Point::new(at.x - (d * off_x - b * off_y) / turn, at.y - (a * off_y - c * off_x) / turn);
        }
        at
    }

    /// Where a point of the drawing goes: through the warp it comes after, and through this
    /// one, in this one's space where it is of another.
    pub fn apply(&self, at: Point) -> Point {
        let at = self.first.as_ref().map_or(at, |first| first.apply(at));
        match self.through.and_then(|to| Some((to, to.inverse()?))) {
            Some((to, back)) => back.apply(self.bent(to.apply(at))),
            None => self.bent(at),
        }
    }

    /// Where a point goes, in this warp's own space. Each bone would carry it rigidly; it goes
    /// where the bones together put it, each counting for more the nearer the point is to it
    /// at rest.
    fn bent(&self, at: Point) -> Point {
        if let Some(spine) = &self.spine {
            return if spine.closed { within(spine, at) } else { along(spine, at) };
        }
        // Where the point goes if it goes part of the way with each bone; and which bone it
        // is of, as a place along the chain: 1.5 is half the second bone's and half the third's.
        let (mut x, mut y, mut whole, mut along_chain) = (0.0, 0.0, 0.0, 0.0);
        for (index, bone) in self.bones.iter().enumerate() {
            // How far the point is from the bone's line at rest, never quite nothing.
            let (along, to) = ((bone.end.x - bone.start.x, bone.end.y - bone.start.y), (at.x - bone.start.x, at.y - bone.start.y));
            let length = along.0 * along.0 + along.1 * along.1;
            let part = if length > 0.0 { ((to.0 * along.0 + to.1 * along.1) / length).clamp(0.0, 1.0) } else { 0.0 };
            let away = (to.0 - along.0 * part).hypot(to.1 - along.1 * part).max(1e-3);
            let weight = bone.hold * away.powf(-self.stiffness);
            let carried = bone.carry.apply(at);
            (x, y, whole) = (x + carried.x * weight, y + carried.y * weight, whole + weight);
            along_chain += weight * index as f64;
        }
        if whole <= 0.0 {
            return at;
        }
        let shared = Point::new(x / whole, y / whole);
        if self.bones.len() < 2 {
            return shared;
        }
        // The joint between the two bones it is of, and how round the joint is where the
        // point lies at rest: wholly within its first circle, not at all beyond its second.
        let place = along_chain / whole;
        let before = (place.floor() as usize).min(self.bones.len() - 2);
        let (first, second, part) = (&self.bones[before], &self.bones[before + 1], place - before as f64);
        let joint = first.end;
        let from = (at.x - joint.x, at.y - joint.y);
        let away = from.0.hypot(from.1);
        let roundness = if away <= second.round {
            1.0
        } else if away < second.fade {
            (second.fade - away) / (second.fade - second.round)
        } else {
            return shared;
        };
        // A point that is nearly all one bone's goes with that bone either way; there the
        // roundness of this joint gives way, so that it meets the next joint's without a step.
        let roundness = roundness * (4.0 * part.min(1.0 - part)).min(1.0);
        // The joint's crease: in the crook of the bend, the nearer the middle of it the more,
        // a point goes with the bone it is nearer, so that the two sides meet in a corner.
        let bend = second.turned - first.turned;
        let crook = if away > 0.0 { (from.0 * second.across.0 + from.1 * second.across.1) / away * (bend / 2.0).sin().signum() } else { 0.0 };
        let sharp = if bend != 0.0 { (second.crease * crook).clamp(0.0, 1.0) } else { 0.0 };
        // The joint's pull: towards the middle of the bend, which is half way between the
        // two bones, by as much of the way as the pull says; less where the crease is.
        let part = (part + second.pull * (1.0 - sharp) * (0.5 - part)).clamp(0.0, 1.0);
        let part = if sharp > 0.0 { (0.5 + (part - 0.5) / (1.0 - sharp).max(1e-9)).clamp(0.0, 1.0) } else { part };
        let mix = |a: f64, b: f64| a + (b - a) * part;
        // Round: about the joint, turned and stretched by as much as lies between the two
        // bones' turns and stretches.
        let [a, b, c, d] = [0, 1, 2, 3].map(|at| mix(first.stretched[at], second.stretched[at]));
        let (sin, cos) = mix(first.turned, second.turned).sin_cos();
        let stretched = (a * from.0 + b * from.1, c * from.0 + d * from.1);
        let to = first.carry.apply(joint);
        let round = Point::new(to.x + cos * stretched.0 - sin * stretched.1, to.y + sin * stretched.0 + cos * stretched.1);
        Point::new(shared.x + (round.x - shared.x) * roundness, shared.y + (round.y - shared.y) * roundness)
    }
}

/// Where a curve carries a point: the point keeps its place along the line and its distance
/// from it, as far to the side as the curve reaches.
fn along(spine: &Spine, at: Point) -> Point {
    let (rest, posed) = (&spine.rest, &spine.posed);
    let pieces = rest.len().saturating_sub(1);
    if pieces == 0 || posed.len() != rest.len() {
        return at;
    }
    // The piece of the line at rest the point is nearest to, how far along it, and how far
    // off it to the side. Past either end the line is taken to run straight on.
    let mut nearest = (f64::INFINITY, 0, 0.0, 0.0);
    for piece in 0..pieces {
        let (from, to) = (rest[piece], rest[piece + 1]);
        let (way, off) = ((to.x - from.x, to.y - from.y), (at.x - from.x, at.y - from.y));
        let length = way.0.hypot(way.1);
        if length <= 0.0 {
            continue;
        }
        let (low, high) = (if piece == 0 { f64::NEG_INFINITY } else { 0.0 }, if piece + 1 == pieces { f64::INFINITY } else { 1.0 });
        let part = ((off.0 * way.0 + off.1 * way.1) / (length * length)).clamp(low, high);
        let away = (off.0 - way.0 * part).hypot(off.1 - way.1 * part);
        if away < nearest.0 {
            nearest = (away, piece, part, (way.0 * off.1 - way.1 * off.0) / length);
        }
    }
    let (_, piece, part, side) = nearest;
    // Across the posed line at each of its points: square to the way it runs through them.
    let across = |point: usize| {
        let (before, after) = (posed[point.saturating_sub(1)], posed[(point + 1).min(pieces)]);
        let (x, y) = (after.x - before.x, after.y - before.y);
        let length = x.hypot(y).max(1e-12);
        (-y / length, x / length)
    };
    // Past an end of the line, how far, at rest: there the point is only moved and turned
    // with the end, not stretched as the piece it is past is.
    let mix = part.clamp(0.0, 1.0);
    let past = (part - mix) * (rest[piece + 1].x - rest[piece].x).hypot(rest[piece + 1].y - rest[piece].y);
    let (to_piece, to_mix) = unstretched(spine, piece, mix);
    let (from, to) = (posed[to_piece], posed[to_piece + 1]);
    let way = (to.x - from.x, to.y - from.y);
    let ahead = past / way.0.hypot(way.1).max(1e-12);
    let ((ax, ay), (bx, by)) = (across(to_piece), across(to_piece + 1));
    let (nx, ny) = (ax + (bx - ax) * to_mix, ay + (by - ay) * to_mix);
    let length = nx.hypot(ny).max(1e-12);
    let along = to_mix + ahead;
    let carried = Point::new(from.x + way.0 * along + nx / length * side, from.y + way.1 * along + ny / length * side);
    let taken = taken(spine, piece, mix, side.hypot(past));
    Point::new(at.x + (carried.x - at.x) * taken, at.y + (carried.y - at.y) * taken)
}

/// Where on the posed line a place on the line at rest goes, `mix` of the way along its piece
/// `piece`, as a piece of the posed line and how far along it. Without stiff ends each piece
/// goes to its own posed piece. In the curve's first and last stretch, with stiff ends, the
/// place keeps its distance along the line from the stiff end it is within, and the rest of
/// the stretch takes the whole stretch, easing into it from the stiff ends so that no step
/// shows. A stretch squashed to nearly nothing in its middle is stretched evenly instead.
fn unstretched(spine: &Spine, piece: usize, mix: f64) -> (usize, f64) {
    let pieces = spine.rest.len() - 1;
    let (stretch, stretches) = (piece / PIECES, pieces / PIECES);
    let start = if stretch == 0 { spine.ends[0].max(0.0) } else { 0.0 };
    let end = if stretch + 1 == stretches { spine.ends[1].max(0.0) } else { 0.0 };
    if start + end <= 0.0 || !pieces.is_multiple_of(PIECES) {
        return (piece, mix);
    }
    let first = stretch * PIECES;
    let length = |line: &[Point], at: usize| (line[at + 1].x - line[at].x).hypot(line[at + 1].y - line[at].y);
    let (rest, posed): (Vec<f64>, Vec<f64>) = (first..first + PIECES).map(|at| (length(&spine.rest, at), length(&spine.posed, at))).unzip();
    let (whole, posed_whole) = (rest.iter().sum::<f64>(), posed.iter().sum::<f64>());
    if whole <= 0.0 {
        return (piece, mix);
    }
    let along = rest[..piece - first].iter().sum::<f64>() + mix * rest[piece - first];
    // The stiff ends leave at least a tenth of the stretch to be stretched, and ease into it
    // over as much as each is long, or as half of what they leave.
    let most = (0.9 * whole / (start + end)).min(1.0);
    let (start, end) = (start * most, end * most);
    let middle = whole - start - end;
    let (ease_in, ease_out) = (start.min(middle / 2.0), end.min(middle / 2.0));
    // How much of the stretch is stretched: all of the middle, half of each easing.
    let stretched = middle - (ease_in + ease_out) / 2.0;
    let even = along * posed_whole / whole;
    let to = if stretched <= 1e-9 {
        even
    } else {
        // Of the way from 0 to `at`, how much is stretched: within an easing, by as much as
        // a smooth step from none to all says.
        let eased = |at: f64, over: f64| {
            if at <= 0.0 {
                0.0
            } else if at < over {
                let x = at / over;
                over * (x * x * x - x * x * x * x / 2.0)
            } else {
                at - over / 2.0
            }
        };
        let by = 1.0 + (posed_whole - whole) / stretched;
        let stiff = along + (by - 1.0) * (eased(along - start, ease_in) - eased(along - (whole - end - ease_out), ease_out));
        // Squashed so far that the middle would fold back: towards even, all of it at none.
        let x = (by / 0.25).clamp(0.0, 1.0);
        even + (stiff - even) * x * x * (3.0 - 2.0 * x)
    };
    let mut left = to.max(0.0);
    for (at, length) in posed.iter().enumerate() {
        if left <= *length || at + 1 == PIECES {
            return (first + at, if *length > 0.0 { (left / length).clamp(0.0, 1.0) } else { 0.0 });
        }
        left -= length;
    }
    (piece, mix)
}

/// How much of what a curve or an envelope does to it a point takes, `away` from the line by
/// its piece `piece`, `mix` of the way along that piece: all of it within the reach there,
/// less from there on, none beyond where it has faded. There is between the two points the
/// piece lies between; a point with no circles reaches without limit.
fn taken(spine: &Spine, piece: usize, mix: f64, away: f64) -> f64 {
    let of = |knot: usize| match spine.reach.get(knot) {
        Some(&(reach, fade)) if fade > 0.0 => {
            if away <= reach {
                1.0
            } else if away < fade {
                (fade - away) / (fade - reach)
            } else {
                0.0
            }
        }
        _ => 1.0,
    };
    let (before, share) = (piece / PIECES, ((piece % PIECES) as f64 + mix) / PIECES as f64);
    let after = if spine.closed { (before + 1) % spine.reach.len().max(1) } else { before + 1 };
    of(before) + (of(after) - of(before)) * share
}

/// Where an envelope carries a point. The point is held by every corner of the outline at
/// rest, by its mean value coordinates: a corner counts for more the nearer it is and the
/// wider the outline is seen about it from the point. It goes where the same shares of the
/// posed corners put it, so an outline that is moved, turned or scaled as a whole takes what
/// it holds along exactly, and one point moved bulges what is near it.
fn within(spine: &Spine, at: Point) -> Point {
    let (rest, posed) = (&spine.rest, &spine.posed);
    let count = rest.len();
    if count < 3 || posed.len() != count {
        return at;
    }
    let to = |corner: usize| (rest[corner].x - at.x, rest[corner].y - at.y);
    // The tangent of half the angle a side of the outline is seen under from the point.
    let half = |corner: usize| {
        let (a, b) = (to(corner), to((corner + 1) % count));
        let (cross, dot) = (a.0 * b.1 - a.1 * b.0, a.0 * b.0 + a.1 * b.1);
        (cross, dot, a.0.hypot(a.1) * b.0.hypot(b.1))
    };
    let (mut x, mut y, mut whole) = (0.0, 0.0, 0.0);
    let mut before = {
        let (cross, dot, lengths) = half(count - 1);
        if cross.abs() > 1e-12 * lengths.max(1.0) { (lengths - dot) / cross } else { 0.0 }
    };
    for corner in 0..count {
        let (away, next) = (to(corner).0.hypot(to(corner).1), (corner + 1) % count);
        // On a corner, it is the corner's; on a side, it is between the side's two ends.
        if away < 1e-9 {
            return posed[corner];
        }
        let (cross, dot, lengths) = half(corner);
        let after = if cross.abs() > 1e-12 * lengths.max(1.0) {
            (lengths - dot) / cross
        } else if dot < 0.0 {
            let part = away / (away + to(next).0.hypot(to(next).1));
            return Point::new(posed[corner].x + (posed[next].x - posed[corner].x) * part, posed[corner].y + (posed[next].y - posed[corner].y) * part);
        } else {
            0.0
        };
        let weight = (before + after) / away;
        (x, y, whole) = (x + posed[corner].x * weight, y + posed[corner].y * weight, whole + weight);
        before = after;
    }
    if whole.abs() < 1e-12 {
        return at;
    }
    let carried = Point::new(x / whole, y / whole);
    if spine.reach.iter().all(|(_, fade)| *fade <= 0.0) {
        return carried;
    }
    // With circles, what is inside the outline is held wholly, and outside it as far as the
    // outline reaches there: by the side it is nearest to.
    let mut inside = false;
    let mut nearest = (f64::INFINITY, 0, 0.0);
    for piece in 0..count {
        let (from, to) = (rest[piece], rest[(piece + 1) % count]);
        if (from.y > at.y) != (to.y > at.y) && at.x < from.x + (at.y - from.y) / (to.y - from.y) * (to.x - from.x) {
            inside = !inside;
        }
        let (way, off) = ((to.x - from.x, to.y - from.y), (at.x - from.x, at.y - from.y));
        let length = way.0 * way.0 + way.1 * way.1;
        let part = if length > 0.0 { ((off.0 * way.0 + off.1 * way.1) / length).clamp(0.0, 1.0) } else { 0.0 };
        let away = (off.0 - way.0 * part).hypot(off.1 - way.1 * part);
        if away < nearest.0 {
            nearest = (away, piece, part);
        }
    }
    if inside {
        return carried;
    }
    let taken = taken(spine, nearest.1, nearest.2, nearest.0);
    Point::new(at.x + (carried.x - at.x) * taken, at.y + (carried.y - at.y) * taken)
}

impl Shot {
    pub fn deformer(&self, id: DeformerId) -> Option<&Deformer> {
        self.deformers.iter().map(|deformer| &**deformer).find(|deformer| deformer.id == id)
    }

    pub fn deformer_mut(&mut self, id: DeformerId) -> Option<&mut Deformer> {
        self.deformers.iter_mut().find(|deformer| deformer.id == id).map(Arc::make_mut)
    }

    /// A deformer as it is posed on `frame` in the shot, with the pegs wired into its ports:
    /// its start carried by the peg in its start port; where the goal of its IK is a peg that
    /// is on, the goal put where that peg's pivot is; and a curve's last point where the peg
    /// in its end port has its pivot, all in the deformer's space. A peg that is not there,
    /// or that the deformer itself places ([`Shot::ik_loops`]), is not followed: what it
    /// would place is as it was set.
    pub fn chain(&self, id: DeformerId, frame: u32) -> Option<Cow<'_, Deformer>> {
        self.wired(self.deformer(id)?, frame)
    }

    /// `deformer`, one of the shot's or a copy of one as it is being edited, posed on `frame`
    /// with the pegs wired into its ports, as [`Shot::chain`] has it.
    pub fn wired<'a>(&self, deformer: &'a Deformer, frame: u32) -> Option<Cow<'a, Deformer>> {
        let id = deformer.id;
        let wired = |peg: Option<PegId>| peg.filter(|peg| !self.ik_loops(id, *peg));
        let carried = wired(deformer.start_peg).and_then(|peg| self.carry_of(id, peg, frame));
        let carried = carried.and_then(|carried| Some(carried.mul(&deformer.start_bind.map_or(Some(Affine::IDENTITY), |bind| bind.inverse())?)));
        let goal = wired(deformer.ik.as_ref().filter(|ik| ik.on_at(frame) && !deformer.is_curve()).and_then(|ik| ik.peg));
        let goal = goal.and_then(|peg| self.goal_of(id, peg, frame));
        let pinned = wired(deformer.end_peg.filter(|_| deformer.is_curve())).and_then(|peg| self.goal_of(id, peg, frame));
        if carried.is_none() && goal.is_none() && pinned.is_none() {
            return Some(Cow::Borrowed(deformer));
        }
        let mut posed = deformer.clone();
        (posed.carried, posed.pinned) = (carried, pinned);
        if let (Some(ik), Some(at)) = (&mut posed.ik, goal) {
            (ik.goal, ik.goals) = (at, Track::default());
        }
        Some(Cow::Owned(posed))
    }

    /// How the peg `peg` carries what lies in its space on `frame`, from the space of the
    /// deformer `id` to where it is in that space: as if what is there were under the peg.
    /// `peg` can be a port, a Send or a Receive, which carries as the peg that comes into it.
    pub fn carry_of(&self, id: DeformerId, peg: PegId, frame: u32) -> Option<Affine> {
        self.peg_above(Some(peg))?;
        Some(self.world(self.home_of(id), frame).inverse()?.mul(&self.world(Some(peg), frame)))
    }

    /// Where the pivot of the peg `goal` is on `frame`, in the space of the deformer `id`:
    /// that of the first layer it bends. Through a port, a Send or a Receive, the pivot of the
    /// peg that comes into it.
    pub fn goal_of(&self, id: DeformerId, goal: PegId, frame: u32) -> Option<[f64; 2]> {
        let goal = self.peg_above(Some(goal))?;
        let [x, y] = goal.transform_at(frame).pivot;
        let seen = self.world(Some(goal.id), frame).apply(Point::new(x, y));
        let at = self.world(self.home_of(id), frame).inverse()?.apply(seen);
        Some([at.x, at.y])
    }

    /// The deformers' ports that `from`, a port, a Send or a Receive, is wired into are wired
    /// to `to` instead, which hands on the same transform: what came into it. With nothing
    /// there, a start lets go and stays where its own pose has it.
    pub fn rewire_ports(&mut self, from: PegId, to: Option<PegId>) {
        for chain in &mut self.deformers {
            let goal = chain.ik.as_ref().is_some_and(|ik| ik.peg == Some(from));
            if goal || chain.start_peg == Some(from) || chain.end_peg == Some(from) {
                let chain = Arc::make_mut(chain);
                if let Some(ik) = chain.ik.as_mut().filter(|_| goal) {
                    ik.peg = to;
                }
                if chain.start_peg == Some(from) {
                    chain.start_peg = to;
                    chain.start_bind = chain.start_bind.filter(|_| to.is_some());
                }
                if chain.end_peg == Some(from) {
                    chain.end_peg = to;
                }
            }
        }
    }

    /// The peg the space of the deformer `id` is under: the parent of the first layer it bends.
    fn home_of(&self, id: DeformerId) -> Option<PegId> {
        self.layers.iter().find(|layer| layer.deformer == Some(id)).and_then(|layer| layer.parent)
    }

    /// Whether the peg `goal`, wired into a port of the deformer `id` (its IK's goal, its
    /// start or its end), would be placed by that deformer: it or a peg above it follows the
    /// deformer, or follows another whose own wired pegs or space are so placed, and so on; or
    /// the deformer's own space is. A peg that does is a circle, and is not wired in.
    pub fn ik_loops(&self, id: DeformerId, goal: PegId) -> bool {
        self.placed_by(Some(goal), id, 0) || self.placed_by(self.home_of(id), id, 0)
    }

    /// Whether what is under `parent` is placed by the deformer `id`, through the pegs it
    /// follows. Deeper than the shot has deformers it is taken to be, which a circle is.
    fn placed_by(&self, parent: Option<PegId>, id: DeformerId, depth: usize) -> bool {
        if depth > self.deformers.len() {
            return true;
        }
        self.ancestors(parent).filter_map(|peg| peg.follows).any(|follow| {
            let other = self.deformer(follow.deformer);
            let wired = other.into_iter().flat_map(|other| [other.ik.as_ref().and_then(|ik| ik.peg), other.start_peg, other.end_peg]).flatten();
            follow.deformer == id
                || wired.collect::<Vec<_>>().into_iter().any(|peg| self.placed_by(Some(peg), id, depth + 1))
                || self.placed_by(self.home_of(follow.deformer), id, depth + 1)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arm along the x axis: an upper arm and a forearm, each 100 long, from (10, 20).
    fn arm() -> Deformer {
        let bones = vec![Bone::at_rest(0.0, 100.0), Bone::at_rest(0.0, 100.0)];
        Deformer::new(DeformerId(1), "Arm", [10.0, 20.0], bones)
    }

    fn near(at: Point, x: f64, y: f64) -> bool {
        (at.x - x).abs() < 1e-6 && (at.y - y).abs() < 1e-6
    }

    #[test]
    fn at_rest_nothing_moves_and_a_turned_bone_carries_what_is_on_it() {
        let mut arm = arm();
        assert_eq!(arm.joints(None), [Point::new(10.0, 20.0), Point::new(110.0, 20.0), Point::new(210.0, 20.0)]);
        let still = arm.warp(0, 1.0);
        assert!(still.is_still() && near(still.apply(Point::new(33.0, 44.0)), 33.0, 44.0));

        // The forearm turns a quarter: its end swings round the elbow, and what lies on the
        // upper arm stays.
        arm.bones[1].turn = 90.0;
        let bent = arm.warp(0, 1.0);
        assert!(near(arm.joints(Some(0))[2], 110.0, 120.0) && !bent.is_still());
        assert!(near(bent.apply(Point::new(210.0, 20.0)), 110.0, 120.0), "the hand: {:?}", bent.apply(Point::new(210.0, 20.0)));
        assert!(near(bent.apply(Point::new(110.0, 20.0)), 110.0, 20.0), "the elbow is where both bones meet");
        let shoulder = bent.apply(Point::new(10.0, 20.0));
        assert!((shoulder.x - 10.0).abs() < 0.01 && (shoulder.y - 20.0).abs() < 0.01, "{shoulder:?}");
        // Off the bones a point goes with the nearer one, mostly: beside the middle of the
        // forearm it has turned with it, beside the upper arm it has hardly moved.
        let beside = bent.apply(Point::new(160.0, 30.0));
        assert!((beside.x - 100.0).abs() < 2.0 && (beside.y - 70.0).abs() < 2.0, "{beside:?}");
        let upper = bent.apply(Point::new(40.0, 30.0));
        assert!((upper.x - 40.0).abs() < 1.0 && (upper.y - 30.0).abs() < 1.0, "{upper:?}");
        // Each bone's reach: on a bone the point is all its own, and half way between two
        // that hold alike it is half of each.
        let reach = bent.shares(Point::new(60.0, 21.0)).unwrap();
        assert!(reach[0] > 0.99 && (reach[0] + reach[1] - 1.0).abs() < 1e-9, "{reach:?}");
        let between = bent.shares(Point::new(110.0, 60.0)).unwrap();
        assert!((between[0] - 0.5).abs() < 1e-6, "{between:?}");
        // A joint taken out of the middle: the bone before it reaches to the end, and keeps
        // its pose; the bone that started there is gone. The ends stay where they were.
        let mut three = self::arm();
        three.bones.push(Bone::at_rest(90.0, 50.0));
        (three.bones[0].turn, three.bones[2].turn) = (10.0, 25.0);
        let (was, merged) = (three.joints(None), three.without_joint(1).expect("a joint between"));
        assert_eq!((merged.bones.len(), merged.bones[0].turn, merged.bones[1].turn), (2, 10.0, 25.0));
        let now = merged.joints(None);
        assert!(near(now[0], was[0].x, was[0].y) && near(now[1], was[2].x, was[2].y) && near(now[2], was[3].x, was[3].y), "{now:?} of {was:?}");
        // The first and the last: the chain is shorter by that bone. One bone stays one.
        assert!(near(three.without_joint(0).unwrap().joints(None)[0], was[1].x, was[1].y));
        assert_eq!(three.without_joint(3).unwrap().joints(None).len(), 3);
        assert!(three.without_joint(4).is_none() && merged.without_joint(1).unwrap().without_joint(0).is_none());
        // Taken back, a point that was moved is where it was drawn: on a bone, beside one,
        // and between the two.
        for drawn in [Point::new(210.0, 20.0), Point::new(160.0, 30.0), Point::new(40.0, 30.0), Point::new(150.0, -40.0), Point::new(60.0, -30.0)] {
            let back = bent.undo(bent.apply(drawn));
            assert!((back.x - drawn.x).abs() < 0.01 && (back.y - drawn.y).abs() < 0.01, "{drawn:?} came back as {back:?}");
        }
        // On the inside of the bend the drawing folds over itself, and several points go to
        // one place: what comes back is one of them.
        let folded = bent.apply(Point::new(80.0, 60.0));
        let one = bent.apply(bent.undo(folded));
        assert!((one.x - folded.x).abs() < 0.01 && (one.y - folded.y).abs() < 0.01, "{one:?} for {folded:?}");
        assert!(near(still.undo(Point::new(33.0, 44.0)), 33.0, 44.0));

        // Turning the upper arm swings all of it; in a layer of twice the pixels the same,
        // in its pixels.
        arm.bones[0].turn = 90.0;
        arm.bones[1].turn = 0.0;
        assert!(near(arm.warp(0, 1.0).apply(Point::new(210.0, 20.0)), 10.0, 220.0));
        assert!(near(arm.warp(0, 2.0).apply(Point::new(420.0, 40.0)), 20.0, 440.0));
    }

    #[test]
    fn a_pose_is_keyed_and_the_chain_can_be_moved() {
        let mut arm = arm();
        arm.bones[1].turns.set(0, 0.0);
        arm.bones[1].turns.set(10, 90.0);
        assert!(arm.is_animated() && arm.bones[1].turn_at(10) == 90.0 && arm.bones[1].turn_at(5) > 0.0);
        assert!(arm.warp(0, 1.0).is_still() && !arm.warp(10, 1.0).is_still());
        // The rest is never animated.
        assert_eq!(arm.joints(None)[2], Point::new(210.0, 20.0));
        arm.offset = [5.0, -5.0];
        assert!(near(arm.warp(0, 1.0).apply(Point::new(10.0, 20.0)), 15.0, 15.0));
        // The keys of a frame are moved and eased together.
        arm.key_all(0);
        arm.move_keys(10, 20);
        arm.set_interp(0, None, Some(Interp::Hold));
        assert_eq!(arm.key_frames().into_iter().collect::<Vec<_>>(), [0, 20]);
        assert_eq!((arm.interp(0), arm.interp(20), arm.interp(10)), (Some((Interp::Linear, Interp::Hold)), Some((Interp::Linear, Interp::Linear)), None));
        assert!(arm.bones[1].turn_at(19) == 0.0 && arm.bones[1].turn_at(20) == 90.0, "held until the next key");
    }

    #[test]
    fn a_round_joint_keeps_its_distance_and_a_stretched_bone_takes_what_it_holds_along() {
        let from_elbow = |at: Point| (at.x - 110.0).hypot(at.y - 20.0);
        let mut bent = arm();
        (bent.bones[1].turn, bent.bones[1].crease) = (120.0, 0.0);
        // Inside a sharp bend, a point shared by both bones is drawn in towards the joint:
        // the pinch. With round joints it keeps its distance, and turns half as far.
        let inside = Point::new(110.0, 40.0);
        let pinched = bent.warp(0, 1.0).apply(inside);
        assert!(from_elbow(pinched) < 12.0, "{pinched:?}");
        (bent.bones[1].round, bent.bones[1].fade) = (30.0, 40.0);
        let round = bent.warp(0, 1.0).apply(inside);
        assert!((from_elbow(round) - 20.0).abs() < 0.01, "{round:?}");
        assert!(near(round, 110.0 - 20.0 * 60f64.to_radians().sin(), 20.0 + 20.0 * 60f64.to_radians().cos()), "{round:?}");
        // What is all one bone's goes where the bone carries it, round or not.
        let hand = bent.warp(0, 1.0).apply(Point::new(210.0, 20.0));
        assert!(near(hand, bent.joints(Some(0))[2].x, bent.joints(Some(0))[2].y), "{hand:?}");
        assert!(near(bent.warp(0, 1.0).apply(Point::new(40.0, 20.0)), 40.0, 20.0));
        // Between the joint's two circles it is between the two, and beyond them pinched.
        (bent.bones[1].round, bent.bones[1].fade) = (10.0, 30.0);
        let half = bent.warp(0, 1.0).apply(inside);
        assert!(near(half, (pinched.x + round.x) / 2.0, (pinched.y + round.y) / 2.0));
        (bent.bones[1].round, bent.bones[1].fade) = (5.0, 15.0);
        assert_eq!(bent.warp(0, 1.0).apply(inside), pinched);
        // In a layer of twice the pixels the circles are twice as large, in its pixels.
        (bent.bones[1].round, bent.bones[1].fade) = (30.0, 40.0);
        let twice = bent.warp(0, 2.0).apply(Point::new(220.0, 80.0));
        assert!(near(twice, round.x * 2.0, round.y * 2.0), "{twice:?}");

        // With a pull, what is on either bone's side within the circles is drawn towards the
        // middle of the bend, which at 120° is the line at 60°: by 40% of the way at 0.4, and
        // all the way at 1. What is half of each bone is on that line already.
        (bent.bones[1].round, bent.bones[1].fade) = (30.0, 40.0);
        let turned_by = |bent: &Deformer, at: Point| {
            let to = bent.warp(0, 1.0).apply(at);
            ((to.y - 20.0).atan2(to.x - 110.0) - (at.y - 20.0).atan2(at.x - 110.0)).to_degrees().rem_euclid(360.0)
        };
        // Beside the joint: more the upper arm's, more the forearm's, and between the two.
        let (upper, fore, between) = (Point::new(95.0, 35.0), Point::new(125.0, 35.0), Point::new(110.0, 40.0));
        let even = [upper, fore, between].map(|at| turned_by(&bent, at));
        assert!(even[0] > 1.0 && even[0] < 55.0 && even[1] > 65.0 && even[1] < 119.0 && (even[2] - 60.0).abs() < 1e-6, "{even:?}");
        bent.bones[1].pull = 0.4;
        let drawn = [upper, fore, between].map(|at| turned_by(&bent, at));
        assert!(drawn[0] > even[0] + 1.0 && drawn[0] < 60.0 && drawn[1] < even[1] - 1.0 && drawn[1] > 60.0 && (drawn[2] - 60.0).abs() < 1e-6, "{drawn:?}");
        bent.bones[1].pull = 1.0;
        let ball = [upper, fore].map(|at| turned_by(&bent, at));
        assert!(ball[0] > drawn[0] && ball[0] <= 60.0 + 1e-9 && ball[1] < drawn[1] && ball[1] >= 60.0 - 1e-9, "{ball:?}");
        let kept = bent.warp(0, 1.0).apply(between);
        assert!((from_elbow(kept) - 20.0).abs() < 0.01, "still round: {kept:?}");
        // Below 0 it is pushed from the middle: a sharper bend.
        bent.bones[1].pull = -0.5;
        let sharper = [upper, fore].map(|at| turned_by(&bent, at));
        assert!(sharper[0] < even[0] && sharper[1] > even[1], "{sharper:?}");

        // With its crease, the crook of the bend is sharp: what lies there goes with the bone
        // it is nearer, the upper arm's side staying and the forearm's turning all the way,
        // where without it both turn part of the way and leave a round bite. The outside of
        // the bend, above the arm, is as round as it was.
        (bent.bones[1].pull, bent.bones[1].crease) = (0.0, 0.0);
        let (near_upper, near_fore, outside) = (Point::new(107.0, 40.0), Point::new(113.0, 40.0), Point::new(107.0, 0.0));
        let round = [near_upper, near_fore, outside].map(|at| turned_by(&bent, at));
        assert!(round[0] > 30.0 && round[1] < 90.0, "{round:?}");
        bent.bones[1].crease = 1.0;
        let creased = [near_upper, near_fore, outside].map(|at| turned_by(&bent, at));
        assert!(creased[0] < 1.0 && (creased[1] - 120.0).abs() < 1.0 && creased[2] == round[2], "{creased:?}");
        assert!(near(bent.warp(0, 1.0).apply(near_upper), 107.0, 40.0));
        // Bent the other way, the crook is on the other side.
        bent.bones[1].turn = -120.0;
        let other = bent.warp(0, 1.0).apply(Point::new(107.0, 0.0));
        assert!(near(other, 107.0, 0.0), "{other:?}");

        // A bone twice as long: its end is twice as far, what lies on it is stretched along
        // it, and the bone after it starts where it ends. Round joints say the same.
        for roundness in [0.0, 1000.0] {
            let mut long = arm();
            (long.bones[0].stretch, long.bones[1].round) = (2.0, roundness);
            assert_eq!(long.joints(Some(0)), [Point::new(10.0, 20.0), Point::new(210.0, 20.0), Point::new(310.0, 20.0)]);
            let warp = long.warp(0, 1.0);
            assert!(near(warp.apply(Point::new(60.0, 20.0)), 110.0, 20.0) && near(warp.apply(Point::new(210.0, 20.0)), 310.0, 20.0), "round {roundness}");
            let beside = warp.apply(Point::new(60.0, 25.0));
            assert!((beside.x - 110.0).abs() < 0.5 && (beside.y - 25.0).abs() < 0.01, "not made thicker: {beside:?}");
        }
        // The length is keyed with the turn, and a bone that holds harder takes more of what
        // lies between it and the next.
        let mut keyed = arm();
        keyed.key_all(0);
        keyed.pose_stretch(1, 10, 1.5);
        assert_eq!((keyed.bones[1].stretch_at(0), keyed.bones[1].stretch_at(10), keyed.key_frames().len()), (1.0, 1.5, 2));
        keyed.remove_keys(0);
        keyed.remove_keys(10);
        assert!(!keyed.is_animated() && keyed.bones[1].stretch == 1.5);
        let mut held_arm = arm();
        held_arm.bones[1].turn = 90.0;
        let even = held_arm.warp(0, 1.0).apply(Point::new(110.0, 40.0));
        held_arm.bones[0].hold = 8.0;
        let held = held_arm.warp(0, 1.0).apply(Point::new(110.0, 40.0));
        assert!((held.x - 110.0).hypot(held.y - 40.0) < (even.x - 110.0).hypot(even.y - 40.0), "{even:?} {held:?}");
        assert_ne!(held_arm.warp(0, 1.0).key(), arm().warp(0, 1.0).key());
    }

    #[test]
    fn a_curve_carries_what_is_drawn_along_it_and_keeps_its_distance() {
        // A tail along the x axis, through three points.
        let mut tail = Deformer::curve(DeformerId(2), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]);
        assert!(tail.is_curve() && tail.joints(None).len() == 3 && tail.line(None).len() == 2 * PIECES + 1);
        assert!(tail.warp(0, 1.0).is_still());
        // Its end is moved up by 100: the line bends, and a point on its end goes with it.
        tail.pose_knot(2, 0, [200.0, -100.0]);
        let warp = tail.warp(0, 1.0);
        assert!(!warp.is_still() && near(warp.apply(Point::new(200.0, 0.0)), 200.0, -100.0));
        assert_ne!(warp.key(), Deformer::curve(DeformerId(2), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]).warp(0, 1.0).key());
        // What is beside the line stays as far from it: 10 from the middle point, and at the
        // start, where the line runs as it did, nearly where it was.
        let beside = warp.apply(Point::new(100.0, 10.0));
        assert!(((beside.x - 100.0).hypot(beside.y) - 10.0).abs() < 1e-6 && beside.x > 100.0, "{beside:?}");
        let start = warp.apply(Point::new(0.0, 10.0));
        assert!(start.x.abs() < 1.0 && (start.y - 10.0).abs() < 0.1, "{start:?}");
        // Past the end the line runs straight on, and takes what is there along.
        let past = warp.apply(Point::new(220.0, 0.0));
        assert!(past.x > 200.0 && past.y < -100.0, "{past:?}");
        // In a layer of twice the pixels, the same in its pixels.
        assert!(near(tail.warp(0, 2.0).apply(Point::new(400.0, 0.0)), 400.0, -200.0));

        // With circles, the curve reaches so far to the side and no further: within 20 of the
        // line all follows, at 40 and beyond nothing does, and at 30 half.
        let unbound = [10.0, 30.0, 60.0].map(|side| warp.apply(Point::new(200.0, side)));
        for knot in 0..3 {
            tail.set_circles(knot, [20.0, 40.0]);
        }
        let bound = tail.warp(0, 1.0);
        assert_ne!(bound.key(), warp.key());
        let reached = [10.0, 30.0, 60.0].map(|side| bound.apply(Point::new(200.0, side)));
        assert_eq!(reached[0], unbound[0]);
        assert!(near(reached[1], (200.0 + unbound[1].x) / 2.0, (30.0 + unbound[1].y) / 2.0), "{reached:?}");
        assert_eq!(reached[2], Point::new(200.0, 60.0));
        // Between a point with circles and one without, it is between the two; and past the
        // line's end the reach is from the end.
        tail.set_circles(2, [0.0, 0.0]);
        let part = tail.warp(0, 1.0).apply(Point::new(150.0, 60.0));
        let (free, held) = (warp.apply(Point::new(150.0, 60.0)), Point::new(150.0, 60.0));
        assert!(near(part, (free.x + held.x) / 2.0, (free.y + held.y) / 2.0), "{part:?}");
        tail.set_circles(2, [20.0, 40.0]);
        assert_eq!(tail.warp(0, 1.0).apply(Point::new(245.0, 0.0)), Point::new(245.0, 0.0));
        assert_eq!((tail.circled(), tail.circles(1), arm().circled(), arm().circles(1)), (0..3, [20.0, 40.0], 1..2, [0.0, 0.0]));
        for knot in 0..3 {
            tail.set_circles(knot, [0.0, 0.0]);
        }

        // A point with handles is passed as they say: with both at nothing it is a corner,
        // and the line runs straight to it and from it. Shown, a point without handles has
        // those of the smooth way through.
        let straight = Deformer::curve(DeformerId(2), "Tail", &[[0.0, 0.0], [100.0, 100.0], [200.0, 0.0]]);
        let shown = straight.handles(1).unwrap();
        assert!((shown.out[0] - 100.0 / 3.0).abs() < 1e-9 && shown.out[1].abs() < 1e-9 && shown.into == [-shown.out[0], 0.0], "{shown:?}");
        let mut cornered = straight.clone();
        cornered.spine[1].bend = Some(Bend::default());
        let by_corner = |line: &[Point]| line[PIECES + 1];
        // How steeply the line falls away just after the point: hardly, where it is smooth,
        // and as the straight way to the next point does, where it is a corner.
        let falls = |line: &[Point]| (100.0 - by_corner(line).y) / (by_corner(line).x - 100.0);
        assert!(
            falls(&straight.line(None)) < 0.25 && (falls(&cornered.line(None)) - 1.0).abs() < 0.01,
            "{} {}",
            falls(&straight.line(None)),
            falls(&cornered.line(None))
        );
        // A longer handle swings the line wider on its side.
        cornered.spine[1].bend = Some(Bend { into: [-60.0, 0.0], out: [60.0, 0.0] });
        assert!(by_corner(&cornered.line(None)).x > by_corner(&straight.line(None)).x + 3.0 && falls(&cornered.line(None)) < falls(&straight.line(None)));
        // Posed, the handles of a point that is moved turn as the smooth way through does: all
        // turned a quarter and moved aside, the line is the line at rest so.
        for (knot, to) in [[10.0, 10.0], [-90.0, 110.0], [10.0, 210.0]].into_iter().enumerate() {
            cornered.pose_knot(knot, 0, to);
        }
        let (rest, turned) = (cornered.line(None), cornered.line(Some(0)));
        assert!(rest.iter().zip(&turned).all(|(was, is)| near(*is, 10.0 - was.y, 10.0 + was.x)));
        // A point that is not moved keeps its handles, also without handles of its own: the
        // line leaves it as at rest whatever the others do.
        let mut kept = straight.clone();
        kept.pose_knot(2, 0, [200.0, 150.0]);
        assert!(near(kept.line(Some(0))[1], straight.line(None)[1].x, straight.line(None)[1].y));

        // A point put in between two lies on the line, and is moved as the two beside it
        // are between them; one taken out leaves the rest, down to two.
        let mut longer = straight.clone();
        longer.spine[2].moves.set(0, [0.0, 0.0]);
        longer.spine[2].moves.set(8, [0.0, 40.0]);
        longer.insert_knot(1, None);
        assert_eq!((longer.spine.len(), longer.spine[2].moves.keys().len(), longer.spine[2].moved_at(8)), (4, 2, [0.0, 20.0]));
        let on_line = straight.line(None)[PIECES + PIECES / 2];
        assert_eq!(longer.spine[2].at, [on_line.x, on_line.y]);
        longer.insert_knot(0, Some([40.0, 60.0]));
        assert_eq!((longer.spine.len(), longer.spine[1].at), (5, [40.0, 60.0]));
        longer.insert_knot(4, None);
        assert_eq!(longer.spine.len(), 5, "nothing is after the last point of a curve");
        for _ in 0..6 {
            longer.remove_knot(1);
        }
        assert_eq!((longer.spine.len(), longer.spine[0].at), (2, [0.0, 0.0]));
        let mut round = Deformer::envelope(DeformerId(3), "Belly", &[[100.0, 0.0], [200.0, 100.0], [100.0, 200.0], [0.0, 100.0]]);
        round.insert_knot(3, None);
        assert!(
            round.spine.len() == 5 && round.spine[4].at[0] < 60.0 && round.spine[4].at[1] < 60.0,
            "between the last and the first: {:?}",
            round.spine[4].at
        );
        for _ in 0..6 {
            round.remove_knot(0);
        }
        assert_eq!(round.spine.len(), 3);

        // Its pose is keyed a point at a time, and all on a frame together.
        tail.pose_knot(2, 0, [200.0, 0.0]);
        tail.key_all(0);
        tail.pose_knot(2, 10, [200.0, -100.0]);
        assert_eq!((tail.key_frames().into_iter().collect::<Vec<_>>(), tail.interp(10)), (vec![0, 10], Some((Interp::Linear, Interp::Linear))));
        assert!(tail.is_animated() && tail.warp(0, 1.0).is_still() && tail.joints(Some(5))[2].y < -20.0);
        tail.move_keys(10, 20);
        tail.remove_keys(0);
        tail.remove_keys(20);
        assert!(!tail.is_animated() && tail.spine[2].moved == [0.0, -100.0]);
        // Laid anew through other points, those it had keep how they are moved.
        let laid = tail.with_rest_joints(&[Point::new(0.0, 0.0), Point::new(90.0, 0.0), Point::new(180.0, 0.0), Point::new(260.0, 0.0)], 1.0);
        assert_eq!((laid.spine.len(), laid.spine[2].at, laid.spine[2].moved, laid.spine[3].moved), (4, [180.0, 0.0], [0.0, -100.0], [0.0, 0.0]));
    }

    #[test]
    fn a_curves_stiff_ends_are_moved_and_not_stretched() {
        // An arm along the x axis, its hand end pulled out to three times as long.
        let mut arm = Deformer::curve(DeformerId(2), "Arm", &[[0.0, 0.0], [100.0, 0.0]]);
        arm.pose_knot(1, 0, [300.0, 0.0]);
        let even = arm.warp(0, 1.0);
        // Past its ends nothing is stretched, only moved.
        assert!(near(even.apply(Point::new(110.0, 0.0)), 310.0, 0.0) && near(even.apply(Point::new(-10.0, 0.0)), -10.0, 0.0));
        // With 20 stiff at each end: what is within them keeps its size, the rest stretches.
        arm.ends = [20.0, 20.0];
        let stiff = arm.warp(0, 1.0);
        assert_ne!(stiff.key(), even.key());
        assert!(near(stiff.apply(Point::new(10.0, 5.0)), 10.0, 5.0) && near(stiff.apply(Point::new(85.0, -5.0)), 285.0, -5.0));
        assert!(near(stiff.apply(Point::new(110.0, 0.0)), 310.0, 0.0));
        // The middle goes on without a step or a fold, made longer or shorter. Both ends are
        // moved, so that the line keeps no handle of its rest and stays straight.
        arm.pose_knot(0, 0, [-10.0, 0.0]);
        for to in [300.0, 50.0, 20.0, 5.0] {
            arm.pose_knot(1, 0, [to - 10.0, 0.0]);
            let warp = arm.warp(0, 1.0);
            let placed: Vec<f64> = (0..=100).map(|at| warp.apply(Point::new(f64::from(at), 0.0)).x).collect();
            assert!(placed.windows(2).all(|pair| pair[1] >= pair[0] && pair[1] - pair[0] < 7.0), "{to}: {placed:?}");
            assert!(near(Point::new(placed[100], 0.0), to - 10.0, 0.0), "{to}");
        }
    }

    #[test]
    fn what_rides_a_curves_end_turns_with_its_handle_and_is_not_grown_by_it() {
        let turn = |carry: Affine| {
            let (at, along) = (carry.apply(Point::new(200.0, 0.0)), carry.apply(Point::new(201.0, 0.0)));
            ((along.y - at.y).atan2(along.x - at.x).to_degrees(), (along.x - at.x).hypot(along.y - at.y))
        };
        // The end's handle set up at 20° from the line's way: at rest, nothing is turned.
        let mut tail = Deformer::curve(DeformerId(2), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]);
        let (sin, cos) = 20f64.to_radians().sin_cos();
        tail.spine[2].bend = Some(Bend { into: [-60.0 * cos, -60.0 * sin], out: [0.0, 0.0] });
        let (angle, grown) = turn(tail.carry(None, 0).unwrap());
        assert!(angle.abs() < 1e-9 && (grown - 1.0).abs() < 1e-9, "{angle} {grown}");
        // The handle posed 20° further round and twice as long: turned by 20°, not grown.
        let (sin, cos) = 40f64.to_radians().sin_cos();
        tail.pose_bend(2, 0, Bend { into: [-120.0 * cos, -120.0 * sin], out: [0.0, 0.0] });
        let (angle, grown) = turn(tail.carry(None, 0).unwrap());
        assert!((angle - 20.0).abs() < 1e-6 && (grown - 1.0).abs() < 1e-9, "{angle} {grown}");
        // The end moved, its handle turns with it, and so does what rides it.
        let mut moved = Deformer::curve(DeformerId(2), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]);
        moved.pose_knot(2, 0, [100.0, 100.0]);
        let (angle, _) = turn(moved.carry(None, 0).unwrap());
        assert!((angle - 90.0).abs() < 1e-6, "{angle}");
    }

    #[test]
    fn a_points_handles_are_posed_and_keyed() {
        let close = |a: Bend, b: Bend| a.into.iter().chain(&a.out).zip(b.into.iter().chain(&b.out)).all(|(a, b)| (a - b).abs() < 1e-9);
        let mut tail = Deformer::curve(DeformerId(1), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]);
        let rest = tail.line(None);
        // Not posed, a point's handles are shown as they are at rest.
        assert_eq!(tail.handles_at(1, 0), tail.handles(1));
        // Posed, they are shown as asked on the frame, and the line passes as they say; the
        // rest is as it was, and with no keys it is so on every frame.
        let asked = Bend { into: [-20.0, 30.0], out: [40.0, -10.0] };
        tail.pose_bend(1, 0, asked);
        assert!(close(tail.handles_at(1, 0).unwrap(), asked) && close(tail.handles_at(1, 7).unwrap(), asked));
        assert!(tail.line(None) == rest && tail.spine[1].bend.is_none() && tail.line(Some(0)) != rest && !tail.is_animated());
        // Another point moved, this one's handles stay as they are; the point itself moved,
        // they turn and grow with it, as those at rest do; posed again there, they are shown
        // as asked again.
        tail.pose_knot(2, 0, [100.0, 100.0]);
        assert!(close(tail.handles_at(1, 0).unwrap(), asked), "{:?}", tail.handles_at(1, 0));
        let mut cornered = Deformer::curve(DeformerId(3), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]);
        cornered.spine[1].bend = Some(Bend { into: [-30.0, 10.0], out: [30.0, 10.0] });
        let was = cornered.handles_at(1, 0).unwrap();
        cornered.pose_knot(0, 0, [0.0, -80.0]);
        cornered.pose_knot(2, 0, [200.0, 90.0]);
        assert!(close(cornered.handles_at(1, 0).unwrap(), was), "handles of their own stay too");
        tail.pose_knot(1, 0, [100.0, 50.0]);
        assert!(!close(tail.handles_at(1, 0).unwrap(), asked));
        tail.pose_bend(1, 0, asked);
        assert!(close(tail.handles_at(1, 0).unwrap(), asked), "{:?}", tail.handles_at(1, 0));

        // Of a point that has keys, the handles are keyed: on the frame, and as they were on
        // the point's other keys, so that only this frame changes.
        let mut keyed = Deformer::curve(DeformerId(1), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]);
        keyed.spine[1].moves.set(0, [0.0, 0.0]);
        keyed.spine[1].moves.set(10, [0.0, 20.0]);
        let before = keyed.handles_at(1, 0).unwrap();
        keyed.pose_bend(1, 10, asked);
        let half = keyed.handles_at(1, 5).unwrap();
        assert!(close(keyed.handles_at(1, 0).unwrap(), before) && close(keyed.handles_at(1, 10).unwrap(), asked));
        assert!(half.out[0] > before.out[0] && half.out[0] < asked.out[0], "{half:?}");
        assert_eq!((keyed.spine[1].bends_into.keys().len(), keyed.spine[1].bends_out.keys().len(), keyed.key_frames().len()), (2, 2, 2));
        // The keys of a frame go together: keyed, moved and taken away.
        keyed.key_all(20);
        assert_eq!((keyed.spine[1].bends_out.keys().len(), keyed.spine[0].bends_out.keys().len()), (3, 0));
        keyed.move_keys(20, 30);
        assert!(keyed.spine[1].bends_into.key(30).is_some() && close(keyed.handles_at(1, 30).unwrap(), asked));
        for frame in [0, 10, 30] {
            keyed.remove_keys(frame);
        }
        assert!(!keyed.is_animated() && close(keyed.handles_at(1, 3).unwrap(), asked));
        // Posed as the smooth way through, a point passes as one without handles does.
        let mut cornered = Deformer::curve(DeformerId(1), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]);
        cornered.pose_knot(2, 0, [100.0, 100.0]);
        cornered.pose_knot(1, 0, [100.0, 10.0]);
        let smooth = cornered.line(Some(0));
        cornered.spine[1].bend = Some(Bend::default());
        assert!(cornered.line(Some(0)) != smooth);
        let way = cornered.smooth_handles_at(1, 0).unwrap();
        cornered.pose_bend(1, 0, way);
        let again = cornered.line(Some(0));
        assert!(smooth.iter().zip(&again).all(|(a, b)| near(*b, a.x, a.y)) && cornered.spine[1].bend == Some(Bend::default()));
        // Put back, the handles are those of the rest again.
        keyed.rest_bend(1, 3);
        assert_eq!((keyed.handles_at(1, 3), keyed.spine[1].bent), (keyed.handles(1), Bend::default()));
    }

    #[test]
    fn an_envelope_holds_what_is_within_it_and_takes_it_along() {
        // An outline round a shape, through four points: a square stood on its corner.
        let corners = [[100.0, 0.0], [200.0, 100.0], [100.0, 200.0], [0.0, 100.0]];
        let mut belly = Deformer::envelope(DeformerId(3), "Belly", &corners);
        assert!(belly.is_envelope() && belly.is_curve() && belly.kind().1 == "envelope" && belly.line(None).len() == 4 * PIECES);
        assert!(belly.warp(0, 1.0).is_still());
        let points = [Point::new(100.0, 100.0), Point::new(130.0, 60.0), Point::new(40.0, 110.0), Point::new(260.0, 30.0)];
        // Moved as a whole, it takes everything along exactly, within it and without.
        for (knot, at) in corners.iter().enumerate() {
            belly.pose_knot(knot, 0, [at[0] + 30.0, at[1] - 10.0]);
        }
        let moved = belly.warp(0, 1.0);
        for at in points {
            assert!(near(moved.apply(at), at.x + 30.0, at.y - 10.0), "{at:?} to {:?}", moved.apply(at));
        }
        // Made twice as large about its middle, what it holds is twice as large.
        for (knot, at) in corners.iter().enumerate() {
            belly.pose_knot(knot, 0, [100.0 + (at[0] - 100.0) * 2.0, 100.0 + (at[1] - 100.0) * 2.0]);
        }
        let grown = belly.warp(0, 1.0);
        for at in points {
            assert!(near(grown.apply(at), 100.0 + (at.x - 100.0) * 2.0, 100.0 + (at.y - 100.0) * 2.0), "{at:?} to {:?}", grown.apply(at));
        }
        // One point pulled out: what is on it goes all the way, what is near it most of the
        // way, and what is by the far side hardly moves.
        for (knot, at) in corners.iter().enumerate() {
            belly.pose_knot(knot, 0, *at);
        }
        belly.pose_knot(1, 0, [260.0, 100.0]);
        let bulged = belly.warp(0, 1.0);
        assert!(!bulged.is_still() && near(bulged.apply(Point::new(200.0, 100.0)), 260.0, 100.0));
        let (by_it, middle, far) = (bulged.apply(Point::new(180.0, 100.0)), bulged.apply(Point::new(100.0, 100.0)), bulged.apply(Point::new(15.0, 100.0)));
        assert!(by_it.x > 215.0 && middle.x > 105.0 && middle.x < 130.0 && far.x < 18.0 && (middle.y - 100.0).abs() < 1e-6, "{by_it:?} {middle:?} {far:?}");
        // With circles, what is outside the outline is taken along only as far as it reaches.
        let outside = Point::new(230.0, 100.0);
        let free = bulged.apply(outside);
        for knot in 0..4 {
            belly.set_circles(knot, [10.0, 20.0]);
        }
        let bound = belly.warp(0, 1.0);
        assert!(free.x > 260.0 && bound.apply(outside) == outside && bound.apply(Point::new(180.0, 100.0)) == by_it, "{free:?}");
        // Its pose is keyed like a curve's.
        belly.key_all(4);
        assert_eq!(belly.key_frames().into_iter().collect::<Vec<_>>(), [4]);
    }

    #[test]
    fn a_shot_given_new_ids_keeps_its_layers_on_their_bones() {
        let mut project = crate::Project::default();
        let mut next = 1000;
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.deformers.push(Arc::new(arm()));
        Arc::make_mut(&mut shot.layers[0]).deformer = Some(DeformerId(1));
        shot.places.insert(1, [30.0, 40.0]);
        shot.reidentify(crate::ShotId(999), || {
            next += 1;
            next
        });
        let bones = shot.deformers[0].id;
        assert!(bones.0 > 1000 && shot.layers[0].deformer == Some(bones) && shot.node_ids().any(|id| id == bones.0));
        assert_eq!(shot.places.get(&bones.0), Some(&[30.0, 40.0]), "where it was put in the node view goes with it");
    }

    #[test]
    fn a_peg_that_follows_a_point_is_carried_as_the_point_is_posed() {
        use crate::{Follow, Inherit, Peg, PegId, Transform};
        let mut project = crate::Project::default();
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.deformers.push(Arc::new(arm()));
        Arc::make_mut(&mut shot.layers[0]).deformer = Some(DeformerId(1));
        let end = Some(Follow { deformer: DeformerId(1), point: None, bends: false });
        let fist = |follows, inherit| Peg {
            id: PegId(50),
            name: "Fist-P".to_string(),
            parent: None,
            transform: Transform::default(),
            animation: Default::default(),
            template: None,
            follows,
            inherit,
            matches: Default::default(),
        };
        // The forearm swung down a quarter round the elbow at (110, 20), and made twice as
        // long: the hand at (210, 20) is at (110, 220), and what is 10 past it goes along.
        let posed = Arc::make_mut(&mut shot.deformers[0]);
        posed.pose_turn(1, 0, 90.0);
        posed.pose_stretch(1, 0, 2.0);
        let carried = |shot: &Shot| shot.world(Some(PegId(50)), 0).apply(Point::new(220.0, 20.0));
        let place = |shot: &mut Shot, follows, inherit| {
            shot.pegs = vec![Arc::new(fist(follows, inherit))];
            carried(shot)
        };
        let all = Inherit::default();
        assert!(near(place(shot, end, all), 110.0, 240.0), "{:?}", carried(shot));
        // Taking less of it: about the hand, where it was at rest.
        assert!(near(place(shot, end, Inherit { scale: false, ..all }), 110.0, 230.0), "{:?}", carried(shot));
        assert!(near(place(shot, end, Inherit { rotation: false, scale: false, ..all }), 120.0, 220.0), "{:?}", carried(shot));
        assert!(near(place(shot, end, Inherit { position: false, ..all }), 210.0, 40.0), "{:?}", carried(shot));
        // A peg that follows nothing, or a point the chain has not got, is not carried.
        assert!(near(place(shot, None, all), 220.0, 20.0));
        assert!(near(place(shot, Some(Follow { deformer: DeformerId(1), point: Some(7), bends: false }), all), 220.0, 20.0));
        // The elbow, the joint the upper arm ends at, which is not turned: there it is moved
        // not at all.
        assert!(near(place(shot, Some(Follow { deformer: DeformerId(1), point: Some(1), bends: false }), all), 220.0, 20.0));

        // Under a parent of its own, where the bones' layer is under another: the hand is
        // followed where it is in the scene. The arm is 50 lower; the fist's parent is not.
        let lift = Peg { id: PegId(60), name: "Lift".to_string(), transform: Transform { position: [0.0, 50.0], ..Transform::default() }, ..fist(None, all) };
        Arc::make_mut(&mut shot.layers[0]).parent = Some(PegId(60));
        let position = Inherit { rotation: false, scale: false, ..all };
        shot.pegs = vec![Arc::new(lift), Arc::new(fist(end, position))];
        assert!(near(shot.world(Some(PegId(50)), 0).apply(Point::new(210.0, 70.0)), 110.0, 270.0));

        // A curve's end, moved, is followed too; and copied with new ids, the peg follows the
        // copy's deformer.
        let mut tail = Deformer::curve(DeformerId(1), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]);
        tail.pose_knot(2, 0, [200.0, 100.0]);
        Arc::make_mut(&mut shot.layers[0]).parent = None;
        (shot.deformers, shot.pegs) = (vec![Arc::new(tail)], vec![Arc::new(fist(end, position))]);
        assert!(near(shot.world(Some(PegId(50)), 0).apply(Point::new(200.0, 0.0)), 200.0, 100.0));
        let mut next = 1000;
        shot.reidentify(crate::ShotId(999), || {
            next += 1;
            next
        });
        assert_eq!(shot.pegs[0].follows.map(|follow| follow.deformer), Some(shot.deformers[0].id));
    }

    #[test]
    fn a_peg_that_bends_what_it_carries_passes_the_bend_to_its_drawings_and_carries_its_pegs() {
        use crate::{Follow, Peg, PegId, Transform};
        let mut project = crate::Project::default();
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.deformers.push(Arc::new(arm()));
        Arc::make_mut(&mut shot.layers[0]).deformer = Some(DeformerId(1));
        // The forearm swung down a quarter round the elbow at (110, 20).
        Arc::make_mut(&mut shot.deformers[0]).pose_turn(1, 0, 90.0);
        let peg = |id, parent, follows| Peg {
            id: PegId(id),
            name: format!("P{id}"),
            parent,
            transform: Transform::default(),
            animation: Default::default(),
            template: None,
            follows,
            inherit: Default::default(),
            matches: Default::default(),
        };
        // The lower arm's peg follows the end and bends what it carries; the fist's peg is under it.
        let bends = Some(Follow { deformer: DeformerId(1), point: None, bends: true });
        shot.pegs = vec![Arc::new(peg(50, None, bends)), Arc::new(peg(80, Some(PegId(50)), None))];
        let up = shot.layers[0].clone();
        let down = crate::Layer { id: crate::LayerId(70), parent: Some(PegId(50)), deformer: None, ..(*up).clone() };
        shot.layers.push(Arc::new(down.clone()));
        assert_eq!(shot.bending(&down), Some(DeformerId(1)));
        // A point of the lower arm, by the forearm, is bent as the upper arm's own drawing is
        // there; and the peg does not carry it besides.
        let d = down.density;
        let at = Point::new(180.0 * d, 20.0 * d);
        let (bent, own) = (shot.warp_of(&down, 0).expect("bent").apply(at), shot.warp_of(&up, 0).unwrap().apply(at));
        assert!(near(bent, own.x, own.y) && near(Point::new(bent.x / d, bent.y / d), 110.0, 90.0), "{bent:?} {own:?}");
        assert!(near(shot.layer_world(&down, 0).apply(Point::new(5.0, 5.0)), 5.0, 5.0), "not carried");
        // The fist's peg is carried by the end, as a peg that follows is.
        let hand = shot.world(Some(PegId(80)), 0).apply(Point::new(210.0, 20.0));
        assert!(near(hand, 110.0, 120.0), "{hand:?}");
        // Not inheriting the rotation, with its pivot at the hand, it rides the end without
        // turning with it, as without the position it stays where it is: what the bones carry
        // the lower arm's peg by is its parent's for it to leave out.
        let fist = |shot: &mut Shot, inherit| {
            let fist = Arc::make_mut(&mut shot.pegs[1]);
            (fist.inherit, fist.transform.pivot) = (inherit, [210.0, 20.0]);
            shot.world(Some(PegId(80)), 0).apply(Point::new(220.0, 20.0))
        };
        let all = crate::Inherit::default();
        let unturned = fist(shot, crate::Inherit { rotation: false, ..all });
        assert!(near(unturned, 120.0, 120.0), "{unturned:?}");
        assert!(near(fist(shot, crate::Inherit { position: false, ..all }), 210.0, 30.0));
        assert!(near(fist(shot, all), 110.0, 130.0));
        Arc::make_mut(&mut shot.pegs[1]).inherit = all;
        // Not bending, the drawing is carried, not bent.
        Arc::make_mut(&mut shot.pegs[0]).follows = Some(Follow { deformer: DeformerId(1), point: None, bends: false });
        assert_eq!((shot.bending(&down), shot.warp_of(&down, 0).is_none()), (None, true));
        assert!(near(shot.layer_world(&down, 0).apply(Point::new(210.0, 20.0)), 110.0, 120.0));
    }

    #[test]
    fn a_peg_takes_from_its_parent_only_what_it_inherits() {
        use crate::{Inherit, Peg, PegId, Transform};
        let mut project = crate::Project::default();
        let shot = Arc::make_mut(&mut project.shots[0]);
        let peg = |id, parent, transform, inherit| {
            Arc::new(Peg {
                id: PegId(id),
                name: id.to_string(),
                parent,
                transform,
                animation: Default::default(),
                template: None,
                follows: None,
                inherit,
                matches: Default::default(),
            })
        };
        // A parent that moves 100 across, turns a quarter and scales twice; a child at (10, 0)
        // of it, with its pivot there.
        let above = Transform { position: [100.0, 0.0], rotation: 90.0, scale: [2.0, 2.0], ..Transform::default() };
        let child = Transform { position: [10.0, 0.0], ..Transform::default() };
        let world = |shot: &mut Shot, inherit| {
            shot.pegs = vec![peg(1, None, above, Inherit::default()), peg(2, Some(PegId(1)), child, inherit)];
            let at = shot.world(Some(PegId(2)), 0).apply(Point::new(1.0, 0.0));
            [at.x, at.y]
        };
        let all = Inherit::default();
        // All of it: (1, 0) of the child is (11, 0) of the parent, turned and doubled.
        let whole = world(shot, all);
        assert!(near(Point::new(whole[0], whole[1]), 100.0, 22.0), "{whole:?}");
        // Without the scale or the rotation, its pivot goes where the parent puts it, and
        // what is about the pivot is not scaled, or not turned.
        let unscaled = world(shot, Inherit { scale: false, ..all });
        assert!(near(Point::new(unscaled[0], unscaled[1]), 100.0, 21.0), "{unscaled:?}");
        let unturned = world(shot, Inherit { rotation: false, ..all });
        assert!(near(Point::new(unturned[0], unturned[1]), 102.0, 20.0), "{unturned:?}");
        // Without the position, the pivot stays where its own values put it.
        let unmoved = world(shot, Inherit { position: false, ..all });
        assert!(near(Point::new(unmoved[0], unmoved[1]), 10.0, 2.0), "{unmoved:?}");
        // Only the parent's own transform is left out: a peg above the parent, which turns
        // a quarter the other way and moves 1000 down, still counts, for the parent's
        // world and for the child's alike.
        let top = Transform { position: [0.0, 1000.0], rotation: -90.0, ..Transform::default() };
        let under_top = |shot: &mut Shot, inherit| {
            shot.pegs = vec![peg(0, None, top, all), peg(1, Some(PegId(0)), above, all), peg(2, Some(PegId(1)), child, inherit)];
            let at = shot.world(Some(PegId(2)), 0).apply(Point::new(1.0, 0.0));
            (at, shot.world(Some(PegId(0)), 0).apply(Point::new(10.0, 2.0)))
        };
        let (at, expected) = under_top(shot, Inherit { position: false, ..all });
        assert!(near(at, expected.x, expected.y), "{at:?} is the top peg's (10, 2): {expected:?}");
        let (at, _) = under_top(shot, all);
        let whole_under_top = shot.world(Some(PegId(0)), 0).apply(Point::new(whole[0], whole[1]));
        assert!(near(at, whole_under_top.x, whole_under_top.y), "{at:?}");
        // What is above a peg, as the viewport moves it by, is the same.
        let child_peg = shot.pegs[2].clone();
        let above_child = shot.above(&child_peg, 0).mul(&child_peg.transform_at(0).matrix()).apply(Point::new(1.0, 0.0));
        assert!(near(above_child, at.x, at.y), "{above_child:?}");
    }

    /// The arm with IK on, reaching for `goal`.
    fn reaching(goal: [f64; 2]) -> Deformer {
        Deformer { ik: Some(Ik { on: true, goal, ..Ik::default() }), ..arm() }
    }

    /// Which side of the line from the shoulder to the hand the elbow is on: above 0 turned on
    /// from it, below turned back.
    fn side(joints: &[Point]) -> f64 {
        let (elbow, hand) = ((joints[1].x - joints[0].x, joints[1].y - joints[0].y), (joints[2].x - joints[0].x, joints[2].y - joints[0].y));
        elbow.0 * hand.1 - elbow.1 * hand.0
    }

    #[test]
    fn ik_turns_two_bones_so_the_hand_is_on_its_goal_and_bent_as_they_were() {
        let mut arm = reaching([110.0, 120.0]);
        arm.bones[1].turn = 10.0;
        let joints = arm.joints(Some(0));
        assert!(near(joints[2], 110.0, 120.0), "{joints:?}");
        let upper = (joints[1].x - 10.0).hypot(joints[1].y - 20.0);
        assert!((upper - 100.0).abs() < 1e-6 && side(&joints) > 0.0, "the elbow stays on the side the turns bend it to: {joints:?}");
        // Bent the other way by their own turns, the other way it is.
        arm.bones[1].turn = -10.0;
        assert!(near(arm.joints(Some(0))[2], 110.0, 120.0) && side(&arm.joints(Some(0))) < 0.0);
        // The turns IK gives are what the drawing and a follower are carried by: the hand's
        // rest goes to the goal.
        let hand = arm.warp(0, 1.0).apply(Point::new(210.0, 20.0));
        assert!((hand.x - 110.0).abs() < 0.01 && (hand.y - 120.0).abs() < 0.01, "{hand:?}");
        let carried = arm.carry(None, 0).unwrap().apply(Point::new(210.0, 20.0));
        assert!(near(carried, 110.0, 120.0), "{carried:?}");
        // Off, the bones are as their own turns have them.
        arm.ik.as_mut().unwrap().on = false;
        assert!((arm.joints(Some(0))[2].y - 20.0 + 100.0 * 10f64.to_radians().sin()).abs() < 1e-6);
    }

    #[test]
    fn two_bones_lying_straight_bend_as_flip_says() {
        let mut arm = reaching([110.0, 120.0]);
        let joints = arm.joints(Some(0));
        assert!(near(joints[2], 110.0, 120.0) && side(&joints) > 0.0 && arm.posed(0)[1].0 > 0.0, "{joints:?}");
        arm.ik.as_mut().unwrap().flip = true;
        let joints = arm.joints(Some(0));
        assert!(near(joints[2], 110.0, 120.0) && side(&joints) < 0.0 && arm.posed(0)[1].0 < 0.0, "{joints:?}");
    }

    #[test]
    fn out_of_reach_the_bones_point_at_the_goal_or_grow_to_it() {
        let mut arm = reaching([10.0, 520.0]);
        let joints = arm.joints(Some(0));
        assert!(near(joints[1], 10.0, 120.0) && near(joints[2], 10.0, 220.0), "{joints:?}");
        arm.ik.as_mut().unwrap().stretch = true;
        let joints = arm.joints(Some(0));
        assert!(near(joints[1], 10.0, 270.0) && near(joints[2], 10.0, 520.0), "{joints:?}");
        assert!(arm.posed(0).iter().all(|(_, stretch)| (stretch - 2.5).abs() < 1e-9));
        // A squash keyed on a bone is kept, and the others bend to make up for it.
        arm.ik.as_mut().unwrap().goal = [110.0, 120.0];
        arm.bones[0].stretch = 0.8;
        let joints = arm.joints(Some(0));
        assert!(near(joints[2], 110.0, 120.0) && ((joints[1].x - 10.0).hypot(joints[1].y - 20.0) - 80.0).abs() < 1e-6, "{joints:?}");
    }

    #[test]
    fn more_bones_are_drawn_to_the_goal_and_the_joint_reaching_can_be_any() {
        let bones = vec![Bone::at_rest(0.0, 100.0), Bone::at_rest(10.0, 100.0), Bone::at_rest(10.0, 100.0)];
        let mut tail = Deformer { ik: Some(Ik { on: true, goal: [150.0, 150.0], ..Ik::default() }), ..Deformer::new(DeformerId(1), "Tail", [0.0, 0.0], bones) };
        let end = tail.joints(Some(0))[3];
        assert!((end.x - 150.0).hypot(end.y - 150.0) < IK_NEAR, "{end:?}");
        // The second bone's end reaching: the last bone goes along as its own turn says.
        let ik = tail.ik.as_mut().unwrap();
        (ik.point, ik.goal) = (Some(2), [100.0, 100.0]);
        let joints = tail.joints(Some(0));
        let last = (joints[3].y - joints[2].y).atan2(joints[3].x - joints[2].x) - (joints[2].y - joints[1].y).atan2(joints[2].x - joints[1].x);
        assert!(near(joints[2], 100.0, 100.0), "{joints:?}");
        assert!((last.to_degrees() - 10.0).abs() < 1e-6, "{}", last.to_degrees());
    }

    #[test]
    fn the_ik_switch_holds_from_key_to_key_and_keys_go_with_the_pose() {
        let mut arm = reaching([110.0, 120.0]);
        let ik = arm.ik.as_mut().unwrap();
        ik.ons.set(0, false);
        ik.ons.set(10, true);
        // Eased however, a switch is held.
        ik.ons.key_mut(0).unwrap().out = Interp::Linear;
        assert!(!ik.on_at(0) && !ik.on_at(9) && ik.on_at(10) && ik.on_at(30));
        assert!(near(arm.joints(Some(9))[2], 210.0, 20.0) && near(arm.joints(Some(10))[2], 110.0, 120.0));
        arm.key_all(20);
        let ik = arm.ik.as_ref().unwrap();
        assert!(ik.ons.key(20).is_some_and(|key| key.value && key.out == Interp::Hold) && ik.goals.key(20).is_some());
        assert_eq!(arm.key_frames_of(Some(PoseValue::Ik)).into_iter().collect::<Vec<_>>(), [0, 10, 20]);
        assert!(arm.shifted(5).ik.unwrap().ons.key(15).is_some() && arm.is_animated());
        arm.remove_keys_of(20, Some(PoseValue::Goal));
        assert!(arm.ik.as_ref().unwrap().goals.is_empty() && arm.ik.as_ref().unwrap().goal == [110.0, 120.0]);
    }

    #[test]
    fn a_peg_wired_into_the_ik_is_its_goal_and_a_circle_is_not_followed() {
        use crate::{Follow, Peg, PegId, Transform};
        let mut project = crate::Project::default();
        let shot = Arc::make_mut(&mut project.shots[0]);
        let mut chain = reaching([0.0, 0.0]);
        chain.ik.as_mut().unwrap().peg = Some(PegId(70));
        shot.deformers.push(Arc::new(chain));
        Arc::make_mut(&mut shot.layers[0]).deformer = Some(DeformerId(1));
        let peg = |id: u64, position: [f64; 2], follows: Option<Follow>| Peg {
            id: PegId(id),
            name: format!("P{id}"),
            parent: None,
            transform: Transform { position, ..Transform::default() },
            animation: Default::default(),
            template: None,
            follows,
            inherit: Default::default(),
            matches: Default::default(),
        };
        // The goal is the peg's pivot, and a peg that follows the hand goes there too.
        let hand = Some(Follow { deformer: DeformerId(1), point: None, bends: false });
        shot.pegs = vec![Arc::new(peg(70, [110.0, 120.0], None)), Arc::new(peg(80, [0.0, 0.0], hand))];
        let posed = shot.chain(DeformerId(1), 0).unwrap();
        assert!(near(posed.joints(Some(0))[2], 110.0, 120.0), "{:?}", posed.joints(Some(0)));
        assert!(near(shot.world(Some(PegId(80)), 0).apply(Point::new(210.0, 20.0)), 110.0, 120.0));
        // Where the chain's layer is under a peg of its own, the goal is taken into its space.
        shot.pegs.push(Arc::new(peg(60, [0.0, 50.0], None)));
        Arc::make_mut(&mut shot.layers[0]).parent = Some(PegId(60));
        assert_eq!(shot.goal_of(DeformerId(1), PegId(70), 0), Some([110.0, 70.0]));
        assert!(near(shot.chain(DeformerId(1), 0).unwrap().joints(Some(0))[2], 110.0, 70.0));
        // The goal following the hand itself, or under a peg that does, would go round in a
        // circle: it is not followed, and the goal is as it was set.
        assert!(!shot.ik_loops(DeformerId(1), PegId(70)));
        assert!(shot.ik_loops(DeformerId(1), PegId(80)));
        shot.pegs[0] = Arc::new(Peg { parent: Some(PegId(80)), ..peg(70, [110.0, 120.0], None) });
        assert!(shot.ik_loops(DeformerId(1), PegId(70)));
        assert!(matches!(shot.chain(DeformerId(1), 0), Some(Cow::Borrowed(_))));
        let _ = shot.world(Some(PegId(70)), 0);
        // Switched off, the peg is not looked at.
        shot.pegs[0] = Arc::new(peg(70, [110.0, 120.0], None));
        Arc::make_mut(&mut shot.deformers[0]).ik.as_mut().unwrap().on = false;
        assert!(matches!(shot.chain(DeformerId(1), 0), Some(Cow::Borrowed(_))));
    }

    #[test]
    fn a_peg_in_the_start_port_carries_a_limbs_start_while_its_end_keeps_its_goal() {
        use crate::{Follow, Peg, PegId, Transform};
        let mut project = crate::Project::default();
        let shot = Arc::make_mut(&mut project.shots[0]);
        let peg = |id: u64, transform: Transform, follows: Option<Follow>| Peg {
            id: PegId(id),
            name: format!("P{id}"),
            parent: None,
            transform,
            animation: Default::default(),
            template: None,
            follows,
            inherit: Default::default(),
            matches: Default::default(),
        };
        // A leg reaching for a goal on the floor, its start wired to the body, which is 30 lower.
        let mut leg = reaching([150.0, 20.0]);
        leg.start_peg = Some(PegId(90));
        shot.deformers.push(Arc::new(leg));
        Arc::make_mut(&mut shot.layers[0]).deformer = Some(DeformerId(1));
        shot.pegs = vec![Arc::new(peg(90, Transform { position: [0.0, 30.0], ..Transform::default() }, None))];
        let joints = shot.chain(DeformerId(1), 0).unwrap().joints(Some(0));
        assert!(near(joints[0], 10.0, 50.0) && near(joints[2], 150.0, 20.0), "the hip goes with the body, the foot stays: {joints:?}");
        assert!(((joints[1].x - 10.0).hypot(joints[1].y - 50.0) - 100.0).abs() < 1e-6);
        // Without IK the chain goes with the body whole, turned as it is turned.
        Arc::make_mut(&mut shot.deformers[0]).ik = None;
        shot.pegs = vec![Arc::new(peg(90, Transform { rotation: 90.0, ..Transform::default() }, None))];
        let joints = shot.chain(DeformerId(1), 0).unwrap().joints(Some(0));
        assert!(near(joints[0], -20.0, 10.0) && near(joints[2], -20.0, 210.0), "{joints:?}");
        // What it bends goes with it: the hand's rest is carried round.
        let hand = shot.chain(DeformerId(1), 0).unwrap().warp(0, 1.0).apply(Point::new(210.0, 20.0));
        assert!((hand.x + 20.0).abs() < 0.01 && (hand.y - 210.0).abs() < 0.01, "{hand:?}");
        // A curve: its first point goes with the peg in its start port, its last to the pivot of
        // the one in its end port, and the points between stay.
        let mut tail = Deformer::curve(DeformerId(1), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]);
        (tail.start_peg, tail.end_peg) = (Some(PegId(90)), Some(PegId(91)));
        shot.deformers = vec![Arc::new(tail)];
        let end = Transform { position: [250.0, 30.0], ..Transform::default() };
        shot.pegs = vec![Arc::new(peg(90, Transform { position: [0.0, 50.0], ..Transform::default() }, None)), Arc::new(peg(91, end, None))];
        let points = shot.chain(DeformerId(1), 0).unwrap().joints(Some(0));
        assert!(near(points[0], 0.0, 50.0) && near(points[1], 100.0, 0.0) && near(points[2], 250.0, 30.0), "{points:?}");
        assert!(near(shot.chain(DeformerId(1), 0).unwrap().joints(None)[0], 0.0, 0.0), "the rest stays");
        // A start peg that follows the curve would go round in a circle: it is not followed.
        shot.pegs[0] = Arc::new(peg(90, Transform::default(), Some(Follow { deformer: DeformerId(1), point: None, bends: false })));
        assert!(shot.ik_loops(DeformerId(1), PegId(90)));
        assert!(near(shot.chain(DeformerId(1), 0).unwrap().joints(Some(0))[0], 0.0, 0.0));
    }
}
