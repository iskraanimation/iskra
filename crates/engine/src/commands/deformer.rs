//! Deformer commands (plan §4.5, R4): the chains of bones that bend a layer's drawings, their
//! rest, and the keyframes of their poses.

use std::sync::Arc;

use iskra_model::{Bone, Deformer, DeformerId, EffectId, Interp, LayerId, PegId, PoseValue, Project, Shot, unique_name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::peg::put;
use super::{checked_frames, checked_name, invalid, name_is_free, parse, title};
use crate::{Result, Session};

/// The most bones a chain has: more than any arm, tail or strand of hair needs.
const MAX_BONES: usize = 64;
/// How far from the origin a chain may lie or be moved, and how long a bone may be, in scene
/// units; and how far a bone may be turned, in degrees: several times round.
const MAX_OFFSET: f64 = 1_000_000.0;
const MAX_TURN: f64 = 3600.0;
/// How short and how long a bone may be stretched, as a part of its length at rest; and how
/// weakly and how strongly it may hold the drawing against the others.
const STRETCH: std::ops::RangeInclusive<f64> = 0.05..=20.0;
const HOLD: std::ops::RangeInclusive<f64> = 0.05..=20.0;
/// How sharply the bones' hold may fall off with distance.
const STIFFNESS: std::ops::RangeInclusive<f64> = 1.0..=12.0;

/// A bone at rest, as a command gives it.
#[derive(Deserialize)]
struct RestBone {
    /// Degrees from the bone before it; for the first, from the x axis.
    angle: f64,
    length: f64,
}

fn checked_bones(bones: &[RestBone]) -> Result<()> {
    if bones.is_empty() || bones.len() > MAX_BONES {
        return invalid(format!("a chain has from 1 to {MAX_BONES} bones"));
    }
    if bones.iter().any(|bone| !bone.angle.is_finite() || bone.angle.abs() > MAX_TURN) {
        return invalid(format!("a bone's angle is outside ±{MAX_TURN}"));
    }
    if bones.iter().any(|bone| !(bone.length > 0.0 && bone.length <= MAX_OFFSET)) {
        return invalid("a bone has a length, above nothing");
    }
    Ok(())
}

/// The points of a curve, as a command gives them: enough for a line, and each within reach.
fn checked_curve(points: &[[f64; 2]], closed: bool) -> Result<()> {
    let (what, least) = if closed { ("an envelope", 3) } else { ("a curve", 2) };
    if points.len() < least || points.len() > MAX_BONES + 1 {
        return invalid(format!("{what} has from {least} to {} points", MAX_BONES + 1));
    }
    points.iter().try_for_each(|at| checked_point("point", *at).map(|_| ()))
}

fn checked_point(what: &str, at: [f64; 2]) -> Result<[f64; 2]> {
    if at.iter().any(|value| !value.is_finite() || value.abs() > MAX_OFFSET) {
        return invalid(format!("the {what} is outside ±{MAX_OFFSET}"));
    }
    Ok(at)
}

/// The shot that holds the deformer a command names.
fn shot_of(project: &Project, deformer: u64) -> Result<&Shot> {
    match project.shots.iter().find(|shot| shot.deformer(DeformerId(deformer)).is_some()) {
        Some(shot) => Ok(shot),
        None => invalid(format!("no deformer {deformer}")),
    }
}

/// How far apart, up and down, the node view puts a node and what it is over: as its layout
/// does, in the graph's units.
const ABOVE: f64 = 96.0 * 0.7;

/// In the node view, put new bones just above the layer they bend, where the layer was put by
/// hand, and the peg that carries it above them, if it is in their way.
fn place_above(shot: &mut Shot, layer: LayerId, bones: DeformerId) {
    let Some([x, y]) = shot.places.get(&layer.0).copied() else { return };
    let at = [x, y - ABOVE];
    shot.places.insert(bones.0, at);
    let peg = shot.layer(layer).and_then(|layer| layer.parent);
    if let Some(peg) = peg
        && let Some(place) = shot.places.get_mut(&peg.0)
        && place[1] > at[1] - ABOVE
    {
        place[1] = at[1] - ABOVE;
    }
}

/// The deformer a command names, to change.
fn deformer_mut(project: &mut Project, deformer: u64) -> Result<&mut Deformer> {
    let shot = shot_of(project, deformer)?.id;
    match project.shot_mut(shot).and_then(|shot| shot.deformer_mut(DeformerId(deformer))) {
        Some(deformer) => Ok(deformer),
        None => invalid(format!("no deformer {deformer}")),
    }
}

/// Adds a chain of bones to a layer's shot and gives the layer to it: from then on the bones
/// bend the layer's drawings. `origin` is where the first bone starts and `bones` are the
/// bones in their order, each with the `angle` it lies at, in degrees from the bone before it
/// (the first from the x axis, turning towards y), and its `length`. All of it is in the
/// layer's own space: scene units, before the layer's pegs. This is the chain's rest: lay it
/// along the drawing as it is drawn. Without a name it gets a free one, the layer's name and
/// "-Bones". Returns its `id`.
///
/// With `curve` in place of `origin` and `bones` it adds a curve: a smooth line through the
/// points given, two or more, in the layer's own space. What is drawn keeps its place along
/// the line and its distance from it, so it follows the line when its points are moved. Its
/// name is the layer's and "-Curve".
///
/// With `envelope` it adds an envelope: a closed outline through the points given, three or
/// more, round a shape of the drawing. What is drawn is held by all of the outline, so when
/// its points are moved the shape within is squashed, stretched and bulged with it. Its name
/// is the layer's and "-Envelope".
pub(super) fn new(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        name: Option<String>,
        origin: Option<[f64; 2]>,
        bones: Option<Vec<RestBone>>,
        curve: Option<Vec<[f64; 2]>>,
        envelope: Option<Vec<[f64; 2]>>,
    }
    let id = "deformer.new";
    let Params { layer, name, origin, bones, curve, envelope } = parse(id, params)?;
    let name = name.as_deref().map(checked_name).transpose()?;
    let closed = envelope.is_some();
    let chain = match (origin, bones, &curve, &envelope) {
        (Some(origin), Some(bones), None, None) => {
            checked_bones(&bones)?;
            Some((checked_point("origin", origin)?, bones))
        }
        (None, None, Some(points), None) | (None, None, None, Some(points)) => {
            checked_curve(points, closed)?;
            None
        }
        _ => return invalid("a deformer is a chain, with an `origin` and `bones`, or a `curve`, or an `envelope`: give one of the three"),
    };
    let curve = curve.or(envelope);
    let mut made = 0;
    s.edit(title(id), |p| {
        let Some(shot) = p.shot_of(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
        let name = match name {
            Some(name) => {
                name_is_free(shot, name, None)?;
                name.to_string()
            }
            None => {
                let of = shot.layer(LayerId(layer)).map_or("Layer", |layer| layer.name.as_str());
                let kind = if chain.is_some() {
                    "Bones"
                } else if closed {
                    "Envelope"
                } else {
                    "Curve"
                };
                unique_name(&format!("{of}-{kind}"), shot.node_names())
            }
        };
        let (shot, fresh) = (shot.id, DeformerId(p.new_id()));
        made = fresh.0;
        let deformer = match (&chain, &curve) {
            (Some((origin, bones)), _) => Deformer::new(fresh, name, *origin, bones.iter().map(|bone| Bone::at_rest(bone.angle, bone.length)).collect()),
            (None, points) if closed => Deformer::envelope(fresh, name, points.as_deref().unwrap_or_default()),
            (None, points) => Deformer::curve(fresh, name, points.as_deref().unwrap_or_default()),
        };
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        // The bones go into the group of the layer they bend.
        let group = super::group::group_by(shot, layer);
        shot.deformers.push(Arc::new(deformer));
        if let Some(group) = group.and_then(|group| shot.group_mut(iskra_model::GroupId(group))) {
            group.members.insert(fresh.0);
        }
        if let Some(layer) = shot.layers.iter_mut().find(|other| other.id == LayerId(layer)) {
            Arc::make_mut(layer).deformer = Some(fresh);
        }
        place_above(shot, LayerId(layer), fresh);
        Ok(())
    })?;
    Ok(json!({ "id": made }))
}

pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        name: String,
    }
    let id = "deformer.rename";
    let Params { deformer, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        name_is_free(shot_of(p, deformer)?, name, Some(deformer))?;
        deformer_mut(p, deformer)?.name = name.to_string();
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Deletes a deformer. The layers it bent are as they were drawn again, and the pegs that
/// followed it follow nothing.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
    }
    let id = "deformer.delete";
    let Params { deformer } = parse(id, params)?;
    s.edit(title(id), |p| {
        let (shot, gone) = (shot_of(p, deformer)?.id, DeformerId(deformer));
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        shot.deformers.retain(|other| other.id != gone);
        shot.places.remove(&gone.0);
        for layer in shot.layers.iter_mut().filter(|layer| layer.deformer == Some(gone)) {
            Arc::make_mut(layer).deformer = None;
        }
        // The pegs that followed it stay where they are at rest.
        for peg in shot.pegs.iter_mut().filter(|peg| peg.follows.is_some_and(|follow| follow.deformer == gone)) {
            Arc::make_mut(peg).follows = None;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Gives a layer to a deformer of its shot, which bends its drawings from then on, or with
/// `deformer` null or left out takes it from the one it has. Several layers can have one
/// deformer: a sleeve and the arm in it.
pub(super) fn set_layer(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        deformer: Option<u64>,
    }
    let id = "layer.setDeformer";
    let Params { layer, deformer } = parse(id, params)?;
    s.edit(title(id), |p| {
        let Some(shot) = p.shot_of(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
        if let Some(deformer) = deformer.filter(|id| shot.deformer(DeformerId(*id)).is_none()) {
            return invalid(format!("the shot `{}` has no deformer {deformer}", shot.name));
        }
        if let Some(layer) = p.layer_mut(LayerId(layer)) {
            layer.deformer = deformer.map(DeformerId);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Takes a joint out of a chain of bones, and with it a bone: `joint` 0 is where the chain
/// starts, and each one after is where a bone ends. Of a joint between two bones, the bone
/// that ends there reaches to the end of the one that started there, which goes with its pose;
/// of the first or the last, the chain is shorter by that bone. The other bones lie as they
/// did and keep their poses and keys.
pub(super) fn remove_bone(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        joint: usize,
    }
    let id = "deformer.removeBone";
    let Params { deformer, joint } = parse(id, params)?;
    s.edit(title(id), |p| {
        let chain = deformer_mut(p, deformer)?;
        if chain.is_curve() {
            return invalid(format!("`{}` is {}: take a point out of it with deformer.removeKnot", chain.name, chain.kind().0));
        }
        if chain.bones.len() < 2 {
            return invalid(format!("`{}` has one bone: delete it instead", chain.name));
        }
        let Some(less) = chain.without_joint(joint) else { return invalid(format!("`{}` has {} bones: no joint {joint}", chain.name, chain.bones.len())) };
        *chain = less;
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets a deformer's rest: where its bones lie on the drawing as it is drawn. `origin` and
/// `bones` are as for `deformer.new`; what is not given stays. The pose is kept: each bone is
/// turned from its new rest by as much as it was from its old, keys and all. With another
/// number of bones, those that are new are at rest and those that go take their keys along.
pub(super) fn set_rest(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        origin: Option<[f64; 2]>,
        bones: Option<Vec<RestBone>>,
        curve: Option<Vec<[f64; 2]>>,
        envelope: Option<Vec<[f64; 2]>>,
    }
    let id = "deformer.setRest";
    let Params { deformer, origin, bones, curve, envelope } = parse(id, params)?;
    let origin = origin.map(|origin| checked_point("origin", origin)).transpose()?;
    if let Some(bones) = &bones {
        checked_bones(bones)?;
    }
    if let Some(points) = &curve {
        checked_curve(points, false)?;
    }
    if let Some(points) = &envelope {
        checked_curve(points, true)?;
    }
    s.edit(title(id), |p| {
        let deformer = deformer_mut(p, deformer)?;
        // A curve or an envelope is laid through its points, and those it has keep how they
        // are moved. Each kind's rest is given as its own.
        let (what, rest) = deformer.kind();
        let given = [("bones", origin.is_some() || bones.is_some()), ("curve", curve.is_some()), ("envelope", envelope.is_some())];
        if given.iter().any(|(kind, given)| *given && *kind != rest) {
            let wanted = if rest == "bones" { "an `origin` and `bones`".to_string() } else { format!("{what}'s points, as `{rest}`") };
            return invalid(format!("`{}` is {what}: its rest is {wanted}", deformer.name));
        }
        let curve = curve.as_ref().or(envelope.as_ref());
        if let Some(points) = curve {
            let laid: Vec<iskra_model::Point> = points.iter().map(|at| iskra_model::Point::new(at[0], at[1])).collect();
            *deformer = deformer.with_rest_joints(&laid, 0.0);
            return Ok(());
        }
        if let Some(origin) = origin {
            deformer.origin = origin;
        }
        if let Some(bones) = &bones {
            let mut was = std::mem::take(&mut deformer.bones).into_iter();
            let rested = bones.iter().map(|rest| match was.next() {
                Some(bone) => Bone { rest_angle: rest.angle, length: rest.length, ..bone },
                None => Bone::at_rest(rest.angle, rest.length),
            });
            deformer.bones = rested.collect();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Poses a deformer: turns the bone at the place `bone` (0 is the first) by `turn` degrees
/// from its rest, makes it `stretch` times as long as at rest, which stretches what it holds
/// along it, moves the chain's start by `offset` from its rest, or any of these. A value that
/// has keyframes is set on `frame`, as a key there; one that has none is set for every frame.
/// Of a curve, it moves the point at the place `knot` to `to`, in the layer's own space, or
/// poses that point's handles: `into` and `out` are the handles as they are to be on the frame,
/// each from the point where it is posed, and a side that is not given stays as it is shown.
/// With `atRest` they are put back to those of the point's rest. The handles are keyed where
/// the point or they have keyframes. With `animate` it is as the window's Animate switch has
/// it, as for a peg (`peg.setTransform`): on, every value given is keyed on `frame`; off, no
/// key is made, and a value that has keys has them all moved by as much. `adjust` is as for a
/// peg: the keys on its frames are moved by as much as a value keyed on `frame`, a curve
/// point's handles too. Of a chain with IK, `goal` moves its goal, in the layer's own space
/// (see `deformer.setIk`).
pub(super) fn set_pose(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        frame: Option<u32>,
        bone: Option<usize>,
        turn: Option<f64>,
        stretch: Option<f64>,
        offset: Option<[f64; 2]>,
        knot: Option<usize>,
        to: Option<[f64; 2]>,
        into: Option<[f64; 2]>,
        out: Option<[f64; 2]>,
        #[serde(default, rename = "atRest")]
        at_rest: bool,
        goal: Option<[f64; 2]>,
        animate: Option<bool>,
        #[serde(default)]
        adjust: Vec<u32>,
    }
    let id = "deformer.setPose";
    let Params { deformer, frame, bone, turn, stretch, offset, knot, to, into, out, at_rest, goal, animate, adjust } = parse(id, params)?;
    let to = to.map(|to| checked_point("point", to)).transpose()?;
    let goal = goal.map(|goal| checked_point("goal", goal)).transpose()?;
    let into = into.map(|into| checked_point("handle", into)).transpose()?;
    let out = out.map(|out| checked_point("handle", out)).transpose()?;
    if at_rest && (into.is_some() || out.is_some()) {
        return invalid("`atRest` puts both handles back: give it, or `into` and `out`, not both");
    }
    let handles = into.is_some() || out.is_some() || at_rest;
    if knot.is_some() != (to.is_some() || handles) {
        return invalid("a `knot` is moved `to` a place, or its handles posed with `into`, `out` or `atRest`: give the `knot` and one of them");
    }
    if let Some(frame) = frame {
        checked_frames(frame, 1)?;
    }
    let offset = offset.map(|offset| checked_point("offset", offset)).transpose()?;
    if turn.is_some_and(|turn| !turn.is_finite() || turn.abs() > MAX_TURN) {
        return invalid(format!("the turn is outside ±{MAX_TURN}"));
    }
    if stretch.is_some_and(|stretch| !STRETCH.contains(&stretch)) {
        return invalid(format!("a bone is stretched to between {} and {} of its length", STRETCH.start(), STRETCH.end()));
    }
    if (turn.is_some() || stretch.is_some()) != bone.is_some() {
        return invalid("a `turn` or a `stretch` is of a `bone`: give both");
    }
    let animated = |what: &str| invalid::<()>(format!("the {what} is animated: say which `frame` to set it on"));
    s.edit(title(id), |p| {
        // How the pegs wired into its ports hold it on the frame, as the handles are shown.
        let held = shot_of(p, deformer)?.chain(DeformerId(deformer), frame.unwrap_or(0)).map(|posed| (posed.carried, posed.pinned));
        let deformer = deformer_mut(p, deformer)?;
        put("offset", &mut deformer.offsets, &mut deformer.offset, (frame, animate, true, &adjust), offset)?;
        if goal.is_some() {
            let name = deformer.name.clone();
            let Some(ik) = deformer.ik.as_mut() else { return invalid(format!("`{name}` has no IK to move the goal of")) };
            put("goal", &mut ik.goals, &mut ik.goal, (frame, animate, true, &adjust), goal)?;
        }
        if let Some(at) = knot {
            let Some(point) = deformer.spine.get(at) else {
                return invalid(format!("`{}` has {} points: no point {at}", deformer.name, deformer.spine.len()));
            };
            let keyed = !point.moves.is_empty() || (handles && !point.bends_into.is_empty());
            if (keyed || animate == Some(true)) && frame.is_none() {
                animated("point")?;
            }
            let frame = frame.unwrap_or(0);
            if let Some(to) = to {
                let (rest, offset) = (deformer.spine[at].at, deformer.offset_at(frame));
                let moved = [to[0] - rest[0] - offset[0], to[1] - rest[1] - offset[1]];
                let point = &mut deformer.spine[at];
                put("point", &mut point.moves, &mut point.moved, (Some(frame), animate, true, &adjust), Some(moved))?;
            }
            if handles {
                // The handles are as the shot shows them: the first point carried by the peg
                // in the start port, the last where the one in the end port is.
                (deformer.carried, deformer.pinned) = held.unwrap_or_default();
                let shown = deformer.handles_at(at, frame).unwrap_or_default();
                let shown = iskra_model::Bend { into: into.unwrap_or(shown.into), out: out.unwrap_or(shown.out) };
                let bent = if at_rest { Some(iskra_model::Bend::default()) } else { deformer.bend_for(at, frame, shown) };
                if let Some(bent) = bent {
                    let point = &mut deformer.spine[at];
                    let unkeyed = point.moves.is_empty() && point.bends_into.is_empty();
                    // The handles as they were here, for the keys picked with `adjust`.
                    let was = (point.bends_into.at(frame, false), point.bends_out.at(frame, false));
                    match animate {
                        // As the switch has it: keyed here, or the keys it has all moved.
                        Some(true) if unkeyed => {
                            point.bends_into.set(frame, bent.into);
                            point.bends_out.set(frame, bent.out);
                        }
                        Some(false) if !point.bends_into.is_empty() => {
                            point.bends_into.move_all(frame, bent.into, false);
                            point.bends_out.move_all(frame, bent.out, false);
                        }
                        Some(false) => point.bent = bent,
                        // Keyed where the point or its handles have keys.
                        _ if at_rest => deformer.rest_bend(at, frame),
                        _ => deformer.pose_bend(at, frame, shown),
                    }
                    // Keyed here, the handles' keys on the frames of `adjust` move alike.
                    let point = &mut deformer.spine[at];
                    let now = (point.bends_into.key(frame).map(|key| key.value), point.bends_out.key(frame).map(|key| key.value));
                    if let ((Some(into), Some(out)), (Some(now_into), Some(now_out)), false) = (was, now, animate == Some(false) || adjust.is_empty()) {
                        point.bends_into.adjust(frame, into, now_into, &adjust);
                        point.bends_out.adjust(frame, out, now_out, &adjust);
                    }
                }
                (deformer.carried, deformer.pinned) = (None, None);
            }
        }
        if let Some(at) = bone {
            let count = deformer.bones.len();
            let Some(bone) = deformer.bones.get_mut(at) else { return invalid(format!("`{}` has {count} bones: no bone {at}", deformer.name)) };
            put("turn", &mut bone.turns, &mut bone.turn, (frame, animate, false, &adjust), turn)?;
            put("stretch", &mut bone.stretches, &mut bone.stretch, (frame, animate, false, &adjust), stretch)?;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Gives a chain of bones IK, changes it, or with `remove` takes it away (plan §4.18). Where IK
/// is on, a joint of the chain reaches for a goal: the bones from the chain's start up to it
/// are turned so that it lies on the goal, and their own turns only say which way they bend.
/// `joint` is the joint that reaches, `"end"` (as it is unless said) or its place from 1, the
/// first bone's end. `goal` is where the goal is, in the layer's own space; a chain given IK
/// without one has it where the joint is. `goalPeg` wires a peg of the shot into the chain's
/// IK port: the goal is then where that peg's pivot is, on every frame, until it is unwired
/// with `goalPeg` null. A peg that the chain itself places, through a peg above it that
/// follows the chain, is turned down. `flip` bends two bones that lie straight the other way.
/// With `stretch`, a goal out of reach has the bones grow along the line to it, each by the
/// same part of its length, until it is reached; without, they point at it.
///
/// `on` switches IK on or off, held from key to key: on every frame where the switch has no
/// keyframes, else as a key on `frame`; `animate` is the Animate switch, as for
/// `deformer.setPose`. The pose on `frame` stays as it was: switched on, the goal is put
/// where the joint is (unless `goal` is given, or the goal is a peg); switched off, the
/// bones' turns, and their lengths with `stretch`, are set as IK had them there.
pub(super) fn set_ik(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        deformer: u64,
        frame: Option<u32>,
        on: Option<bool>,
        joint: Option<Value>,
        goal: Option<[f64; 2]>,
        goal_peg: Option<u64>,
        flip: Option<bool>,
        stretch: Option<bool>,
        #[serde(default)]
        remove: bool,
        animate: Option<bool>,
    }
    let id = "deformer.setIk";
    // A `goalPeg` of null unwires the peg, which serde would read as none given.
    let unwire = params.get("goalPeg").is_some_and(Value::is_null);
    let Params { deformer, frame, on, joint, goal, goal_peg, flip, stretch, remove, animate } = parse(id, params)?;
    if let Some(frame) = frame {
        checked_frames(frame, 1)?;
    }
    let goal = goal.map(|goal| checked_point("goal", goal)).transpose()?;
    let joint = match joint {
        None => None,
        Some(Value::String(end)) if end == "end" => Some(None),
        Some(Value::Number(at)) => match at.as_u64() {
            Some(at) if at >= 1 => Some(Some(at as usize)),
            _ => return invalid("the `joint` is counted from 1, the first bone's end"),
        },
        Some(_) => return invalid("the `joint` is \"end\" or a number"),
    };
    // Absent, the peg stays as it is; null unwires it.
    let goal_peg = if unwire { Some(None) } else { goal_peg.map(|peg| Some(PegId(peg))) };
    s.edit(title(id), |p| {
        let shot = shot_of(p, deformer)?;
        let chain = DeformerId(deformer);
        let Some(was) = shot.deformer(chain) else { return invalid(format!("no deformer {deformer}")) };
        if was.is_curve() {
            return invalid(format!("`{}` is {}: IK is for a chain of bones", was.name, was.kind().0));
        }
        if let Some(Some(peg)) = goal_peg {
            match shot.peg(peg) {
                None => return invalid(format!("no peg {} in the shot of `{}`", peg.0, was.name)),
                Some(found) if shot.ik_loops(chain, peg) => {
                    return invalid(format!("`{}` follows `{}`, or is under a peg that does: as its goal it would go round in a circle", found.name, was.name));
                }
                Some(_) => {}
            }
        }
        let count = was.bones.len();
        if let Some(Some(at)) = joint
            && at > count
        {
            return invalid(format!("`{}` has {count} bones: no joint {at}", was.name));
        }
        let at = frame.unwrap_or(0);
        let was_on = was.ik.as_ref().is_some_and(|ik| ik.on_at(at));
        let new = was.ik.is_none();
        // The pose as it is shown before the switch, with the IK changed as asked: where the
        // joint is, and how IK turns the bones.
        let mut shown = shot.chain(chain, at).map(std::borrow::Cow::into_owned).unwrap_or_else(|| was.clone());
        let ik = shown.ik.get_or_insert_with(Default::default);
        ik.point = joint.unwrap_or(ik.point);
        ik.flip = flip.unwrap_or(ik.flip);
        ik.stretch = stretch.unwrap_or(ik.stretch);
        let (reaching, by_peg) = (ik.point.unwrap_or(count).min(count), goal_peg.unwrap_or(ik.peg).is_some());
        let (joint_at, solved) = (shown.joints(Some(at))[reaching], shown.posed(at));
        let deformer = deformer_mut(p, deformer)?;
        if remove {
            deformer.ik = None;
            return Ok(());
        }
        let ik = deformer.ik.get_or_insert_with(Default::default);
        ik.point = joint.unwrap_or(ik.point);
        ik.flip = flip.unwrap_or(ik.flip);
        ik.stretch = stretch.unwrap_or(ik.stretch);
        ik.peg = goal_peg.unwrap_or(ik.peg);
        if new && goal.is_none() {
            ik.goal = [joint_at.x, joint_at.y];
        }
        put("goal", &mut ik.goals, &mut ik.goal, (frame, animate, true, &[]), goal)?;
        if let Some(on) = on {
            put("switch", &mut ik.ons, &mut ik.on, (frame, animate, false, &[]), Some(on))?;
            ik.hold();
            if on && !was_on && goal.is_none() && !by_peg {
                put("goal", &mut ik.goals, &mut ik.goal, (frame, animate, true, &[]), Some([joint_at.x, joint_at.y]))?;
            }
            if !on && was_on {
                let grows = ik.stretch;
                for (bone, (turn, stretch)) in deformer.bones[..reaching].iter_mut().zip(solved) {
                    put("turn", &mut bone.turns, &mut bone.turn, (frame, animate, false, &[]), Some(turn))?;
                    if grows {
                        put("stretch", &mut bone.stretches, &mut bone.stretch, (frame, animate, false, &[]), Some(stretch))?;
                    }
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Wires a peg of the shot into a port of a chain of bones or of a curve, or with `peg` null
/// unwires it. `port` is `"start"` or `"end"`. A peg in the start port carries the chain's
/// start, or the curve's first point, as if that were under it, and a chain's bones turn
/// with it: so a limb's start goes with the body, whatever peg the limb's layer is under,
/// while its end stays on its goal. A peg in a curve's end port has its last point where the
/// peg's pivot is. A chain's end port is the goal of its IK (`goalPeg` of `deformer.setIk`),
/// which a chain that has none is given, on. An envelope has no ports. A peg that the
/// deformer itself places, through a peg above it that follows the deformer, is turned down.
/// A peg wired into the start takes hold of the start where it is on `frame` (0 unless
/// given): from there the start goes as the peg moves, and wiring it in moves nothing.
pub(super) fn set_port(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        port: String,
        peg: Option<u64>,
        #[serde(default)]
        frame: u32,
    }
    let id = "deformer.setPort";
    let Params { deformer, port, peg, frame } = parse(id, params)?;
    checked_frames(frame, 1)?;
    let start = match port.as_str() {
        "start" => true,
        "end" => false,
        _ => return invalid("the `port` is \"start\" or \"end\""),
    };
    let peg = peg.map(PegId);
    s.edit(title(id), |p| {
        let shot = shot_of(p, deformer)?;
        let chain = DeformerId(deformer);
        let Some(was) = shot.deformer(chain) else { return invalid(format!("no deformer {deformer}")) };
        if was.is_envelope() {
            return invalid(format!("`{}` is an envelope: it has no ports", was.name));
        }
        if let Some(peg) = peg {
            // A port, a Send or a Receive stands where a peg would, by its own id, where a
            // peg comes into it.
            let found = shot
                .peg(peg)
                .map(|peg| &peg.name)
                .or_else(|| shot.effect(EffectId(peg.0)).filter(|_| shot.hands_down(peg) && shot.peg_above(Some(peg)).is_some()).map(|node| &node.name));
            match found {
                None => return invalid(format!("no peg, port, Send or Receive {} in the shot of `{}`", peg.0, was.name)),
                Some(found) if shot.ik_loops(chain, peg) => {
                    return invalid(format!("`{found}` follows `{}`, or is under a peg that does: wired in, it would go round in a circle", was.name));
                }
                Some(_) => {}
            }
        }
        // How the peg carries the start as it is wired in, which it goes on from.
        let bind = peg.filter(|_| start).and_then(|peg| shot.carry_of(chain, peg, frame));
        let deformer = deformer_mut(p, deformer)?;
        match (start, deformer.is_curve()) {
            (true, _) => (deformer.start_peg, deformer.start_bind) = (peg, bind),
            (false, true) => deformer.end_peg = peg,
            (false, false) => {
                let ik = deformer.ik.get_or_insert_with(|| iskra_model::Ik { on: true, ..Default::default() });
                ik.peg = peg;
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Keys a deformer's pose on `frame`, as it is there: every bone's turn and the chain's
/// offset. From then on the pose is animated: setting it on another frame makes a key there.
pub(super) fn set_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        frame: u32,
    }
    let id = "deformer.setKey";
    let Params { deformer, frame } = parse(id, params)?;
    checked_frames(frame, 1)?;
    s.edit(title(id), |p| {
        deformer_mut(p, deformer)?.key_all(frame);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Removes a deformer's keyframes on `frame`: of all its values, or of the one `value` names
/// (`"offset"`, `{ "turn": bone }`, `{ "stretch": bone }`, `{ "moved": point }`,
/// `{ "bentInto": point }` or `{ "bentOut": point }`, bones and points counted from 0; `"ik"`
/// or `"goal"` of a chain's IK). A
/// value that loses its last key stays as that key had it, on every frame.
pub(super) fn remove_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        frame: u32,
        value: Option<PoseValue>,
    }
    let id = "deformer.removeKey";
    let Params { deformer, frame, value } = parse(id, params)?;
    s.edit(title(id), |p| {
        keyed(p, deformer, frame, value)?.remove_keys_of(frame, value);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// The deformer a command names, which has to have a keyframe on `frame`: of any value, or of
/// the one named.
fn keyed(project: &mut Project, deformer: u64, frame: u32, value: Option<PoseValue>) -> Result<&mut Deformer> {
    let deformer = deformer_mut(project, deformer)?;
    if !deformer.key_frames_of(value).contains(&frame) {
        let of = value.map_or(String::new(), |value| format!(" of {}", value.label()));
        return invalid(format!("`{}` has no keyframe{of} on frame {frame}", deformer.name));
    }
    Ok(deformer)
}

/// Moves a deformer's keyframes from one frame to another, over any that are there: of all its
/// values, or of the one `value` names (see `deformer.removeKey`).
pub(super) fn move_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        from: u32,
        to: u32,
        value: Option<PoseValue>,
    }
    let id = "deformer.moveKey";
    let Params { deformer, from, to, value } = parse(id, params)?;
    checked_frames(to, 1)?;
    s.edit(title(id), |p| {
        keyed(p, deformer, from, value)?.move_keys_of(from, to, value);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets how a deformer's keyframes on `frame` ease: `into` them, `out` of them, or both, each
/// `linear`, `ease` (at rest at the key), out of a key `hold`, or `{ "curve": { "reach",
/// "speed" } }`, as a peg's do (`peg.setEase`). All the values of the pose on the frame ease
/// alike, or the one `value` names alone (see `deformer.removeKey`). A side that is not given
/// stays as it is.
pub(super) fn set_ease(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        frame: u32,
        into: Option<Interp>,
        out: Option<Interp>,
        value: Option<PoseValue>,
    }
    let id = "deformer.setEase";
    let Params { deformer, frame, into, out, value } = parse(id, params)?;
    if into == Some(Interp::Hold) {
        return invalid("a value is held out of a keyframe, not into one");
    }
    for side in [into, out] {
        if let Some(Interp::Curve(slope)) = side
            && !((0.01..=1.0).contains(&slope.reach) && slope.speed.is_finite() && slope.speed.abs() <= super::peg::MAX_SLOPE)
        {
            return invalid(format!("a curve reaches 0.01 to 1 of the way, at a speed within ±{}", super::peg::MAX_SLOPE));
        }
    }
    s.edit(title(id), |p| {
        keyed(p, deformer, frame, value)?.set_interp_of(frame, value, into, out);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets how a deformer's bones share the drawing out between them, which is the same on every
/// frame. `stiffness` is how sharply a bone's hold falls off with distance, from 1 to 12 (4
/// unless set): the higher, the stiffer the parts along the bones and the tighter the bend at
/// a joint. `hold`, of the bone at the place `bone`, is how strongly that bone holds against
/// the others: 1 as they do, more and it takes more of what lies between. `round` and `fade`
/// are the two circles of the joint at the place `joint`, where the bone at that place starts
/// (1 is the first joint: the chain's start is none): their radii about the joint at rest, in
/// scene units. Within `round` what lies between the two bones turns about the joint and
/// keeps its distance from it, as an elbow does; from there the roundness falls off to
/// nothing at `fade`, which is never less than `round`, and beyond it a sharp bend pinches.
/// `pull`, of a `joint` too and from -1 to 1, draws what is within the circles towards the
/// middle of the bend, the line at half the angle between the two bones: at 0.4 by 40% of the
/// way, at 1 all of it turns by half the angle, and below 0 it is pushed from the middle.
/// `crease`, of a `joint` and from 0 to 1 (1 unless set), is how sharp the inside of the bend
/// is: at 1 what lies in its crook goes with the bone it is nearer, so the two sides meet in a
/// sharp corner while the outside stays round; at 0 the crook is round and shows a small bite.
///
/// Of a curve, `reach` and `fade` are the two circles of the point at the place `knot`: how far
/// to the side of the line the drawing is carried there. Within `reach` of the line it follows
/// wholly, from there less, and at `fade` and beyond it stays as it was drawn; between two
/// points the reach is between theirs, and both at 0 set no limit. `ends`, of a curve, is how
/// far from its first and its last point, along its line at rest, what is drawn is only moved
/// and turned with the line, never stretched: the rest of the line stretches the more. In
/// scene units; at 0 the stretch is even.
pub(super) fn set_weights(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        stiffness: Option<f64>,
        bone: Option<usize>,
        hold: Option<f64>,
        joint: Option<usize>,
        round: Option<f64>,
        fade: Option<f64>,
        pull: Option<f64>,
        crease: Option<f64>,
        knot: Option<usize>,
        reach: Option<f64>,
        ends: Option<[f64; 2]>,
    }
    let id = "deformer.setWeights";
    let Params { deformer, stiffness, bone, hold, joint, round, fade, pull, crease, knot, reach, ends } = parse(id, params)?;
    if stiffness.is_some_and(|stiffness| !STIFFNESS.contains(&stiffness)) {
        return invalid(format!("the stiffness is from {} to {}", STIFFNESS.start(), STIFFNESS.end()));
    }
    if hold.is_some_and(|hold| !HOLD.contains(&hold)) {
        return invalid(format!("a bone's hold is from {} to {}", HOLD.start(), HOLD.end()));
    }
    if hold.is_some() != bone.is_some() {
        return invalid("a `hold` is of a `bone`: give both");
    }
    if [round, fade, reach].into_iter().flatten().any(|radius| !(0.0..=MAX_OFFSET).contains(&radius)) {
        return invalid(format!("a circle has a radius from 0 to {MAX_OFFSET}"));
    }
    if ends.is_some_and(|ends| ends.iter().any(|end| !(0.0..=MAX_OFFSET).contains(end))) {
        return invalid(format!("a curve's stiff ends are from 0 to {MAX_OFFSET} long"));
    }
    if reach.is_some() && knot.is_none() {
        return invalid("a `reach` is of a `knot`: say which");
    }
    if pull.is_some_and(|pull| !(-1.0..=1.0).contains(&pull)) {
        return invalid("a joint's pull is from -1 to 1");
    }
    if crease.is_some_and(|crease| !(0.0..=1.0).contains(&crease)) {
        return invalid("a joint's crease is from 0 to 1");
    }
    if (round.is_some() || (fade.is_some() && knot.is_none()) || pull.is_some() || crease.is_some()) != joint.is_some() {
        return invalid("`round`, `fade`, `pull` and `crease` are of a `joint`: say which");
    }
    s.edit(title(id), |p| {
        let deformer = deformer_mut(p, deformer)?;
        deformer.stiffness = stiffness.unwrap_or(deformer.stiffness);
        let (count, name) = (deformer.bones.len(), deformer.name.clone());
        if let Some(ends) = ends {
            if !deformer.is_curve() || deformer.is_envelope() {
                return invalid(format!("`{name}` is not a curve: it has no ends"));
            }
            deformer.ends = ends;
        }
        if let (Some(at), Some(hold)) = (bone, hold) {
            let Some(bone) = deformer.bones.get_mut(at) else { return invalid(format!("`{name}` has {count} bones: no bone {at}")) };
            bone.hold = hold;
        }
        if let Some(at) = knot {
            let Some(point) = deformer.spine.get(at) else {
                return invalid(format!("`{name}` has {} points: no point {at}", deformer.spine.len()));
            };
            let circles = [reach.unwrap_or(point.reach), fade.unwrap_or(point.fade)];
            deformer.set_circles(at, circles);
        }
        if let Some(at) = joint {
            let Some(bone) = deformer.bones.get_mut(at).filter(|_| at > 0) else {
                return invalid(format!("`{name}` has joints from 1 to {}: no joint {at}", count.saturating_sub(1)));
            };
            bone.round = round.unwrap_or(bone.round);
            bone.fade = fade.unwrap_or(bone.fade).max(bone.round);
            bone.pull = pull.unwrap_or(bone.pull);
            bone.crease = crease.unwrap_or(bone.crease);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// The curve or the envelope a command names, to change one of its points.
fn curve_mut(project: &mut Project, deformer: u64, knot: usize) -> Result<&mut Deformer> {
    let curve = deformer_mut(project, deformer)?;
    if !curve.is_curve() {
        return invalid(format!("`{}` is a chain of bones: it has no points", curve.name));
    }
    if knot >= curve.spine.len() {
        return invalid(format!("`{}` has {} points: no point {knot}", curve.name, curve.spine.len()));
    }
    Ok(curve)
}

/// Shapes how a curve's or an envelope's line passes through the point at the place `knot`, at
/// rest. `into` and `out` are its handles, each from the point: the line arrives from the
/// direction of `into` and leaves towards `out`, and the longer a handle the wider the line
/// swings on its side. A side that is not given stays as it is shown. Both at `[0, 0]` make a
/// corner. With `smooth` the handles are taken away, and the line passes smoothly again.
pub(super) fn set_bend(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        knot: usize,
        into: Option<[f64; 2]>,
        out: Option<[f64; 2]>,
        #[serde(default)]
        smooth: bool,
    }
    let id = "deformer.setBend";
    let Params { deformer, knot, into, out, smooth } = parse(id, params)?;
    let into = into.map(|into| checked_point("handle", into)).transpose()?;
    let out = out.map(|out| checked_point("handle", out)).transpose()?;
    if smooth == (into.is_some() || out.is_some()) {
        return invalid("give a handle, `into` or `out` or both, or `smooth` to take them away");
    }
    s.edit(title(id), |p| {
        let curve = curve_mut(p, deformer, knot)?;
        let shown = curve.handles(knot).unwrap_or_default();
        curve.spine[knot].bend = (!smooth).then_some(iskra_model::Bend { into: into.unwrap_or(shown.into), out: out.unwrap_or(shown.out) });
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Puts a point into a curve or an envelope after the one at the place `after`: at `at`, in the
/// layer's own space, or else on the line half way to the next point. It is moved, keys and
/// all, by as much as the two points beside it are between them. Nothing is after the last
/// point of a curve; of an envelope, the first is.
pub(super) fn insert_knot(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        after: usize,
        at: Option<[f64; 2]>,
    }
    let id = "deformer.insertKnot";
    let Params { deformer, after, at } = parse(id, params)?;
    let at = at.map(|at| checked_point("point", at)).transpose()?;
    s.edit(title(id), |p| {
        let curve = curve_mut(p, deformer, after)?;
        if curve.spine.len() > MAX_BONES {
            return invalid(format!("a curve has up to {} points", MAX_BONES + 1));
        }
        if !curve.is_envelope() && after + 1 >= curve.spine.len() {
            return invalid(format!("point {after} is the last of `{}`: lay a new end with `deformer.setRest`", curve.name));
        }
        curve.insert_knot(after, at);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Takes the point at the place `knot` out of a curve or an envelope. A curve keeps two points
/// and an envelope three.
pub(super) fn remove_knot(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        deformer: u64,
        knot: usize,
    }
    let id = "deformer.removeKnot";
    let Params { deformer, knot } = parse(id, params)?;
    s.edit(title(id), |p| {
        let curve = curve_mut(p, deformer, knot)?;
        let least = if curve.is_envelope() { 3 } else { 2 };
        if curve.spine.len() <= least {
            return invalid(format!("`{}` is {} and keeps {least} points: delete it instead", curve.name, curve.kind().0));
        }
        curve.remove_knot(knot);
        Ok(())
    })?;
    Ok(Value::Null)
}
