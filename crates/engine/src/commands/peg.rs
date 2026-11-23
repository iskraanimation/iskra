//! Peg commands: the transforms that carry a shot's layers, what is parented to what, and
//! the keyframes that animate a peg (plan §4.3).

use std::sync::Arc;

use iskra_model::{Animatable, Bend, Channel, DeformerId, Follow, Interp, LayerId, Node, Peg, PegId, Project, Shot, ShotId, Track, Transform, unique_name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{checked_frames, checked_name, invalid, name_is_free, parse, shot_id, title};
use crate::{Result, Session};

/// The shot that holds the peg a command names.
fn shot_of(project: &Project, peg: u64) -> Result<&Shot> {
    match project.shot_of_peg(PegId(peg)) {
        Some(shot) => Ok(shot),
        None => invalid(format!("no peg {peg}")),
    }
}

/// The peg a command names as a parent, which has to be in `shot`. `None` is no parent.
pub(super) fn parent_in(shot: &Shot, parent: Option<u64>) -> Result<Option<PegId>> {
    match parent.map(PegId) {
        // A port or a named port stands in a peg's place: it hands a peg's transform down.
        Some(id) if shot.peg(id).is_none() && !shot.hands_down(id) => invalid(format!("the shot `{}` has no peg {}", shot.name, id.0)),
        parent => Ok(parent),
    }
}

/// Adds a peg to a shot, under `parent` if one is given. With `aboveLayer` or `abovePeg` the
/// new peg goes between that node and its parent instead: it takes the node's place and the
/// node is parented to it. With `aboveCamera` it goes above the shot's camera in the same way.
/// With `group` it is put into that group of the shot, and with `at` at that place in the
/// node view. Without a name it gets a free one: "Peg", or the node's name and
/// "-P".
pub(super) fn new(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        shot: Option<u64>,
        name: Option<String>,
        parent: Option<u64>,
        above_layer: Option<u64>,
        above_peg: Option<u64>,
        #[serde(default)]
        above_camera: bool,
        group: Option<u64>,
        at: Option<[f64; 2]>,
    }
    let id = "peg.new";
    let Params { shot, name, parent, above_layer, above_peg, above_camera, group, at } = parse(id, params)?;
    if above_camera {
        if parent.is_some() || above_layer.is_some() || above_peg.is_some() {
            return invalid("a peg put above the camera takes the camera's parent, and goes above nothing else");
        }
        return above_the_camera(s, shot, name);
    }
    let peg = s.edit(title(id), |p| {
        let below = match (above_layer, above_peg) {
            (Some(_), Some(_)) => return invalid("a new peg goes above one node: a layer or a peg, not both"),
            (Some(layer), None) => Some(Node::Layer(LayerId(layer))),
            (None, Some(peg)) => Some(Node::Peg(PegId(peg))),
            (None, None) => None,
        };
        let (shot, parent, base): (ShotId, _, _) = match below {
            Some(_) if parent.is_some() => return invalid("a peg put above a node takes that node's parent"),
            Some(Node::Layer(layer)) => match p.shot_of(layer) {
                Some(shot) => (shot.id, shot.parent_of(Node::Layer(layer)), shot.layer(layer).map(|e| format!("{}-P", e.name))),
                None => return invalid(format!("no layer {}", layer.0)),
            },
            Some(Node::Peg(peg)) => {
                let shot = shot_of(p, peg.0)?;
                (shot.id, shot.parent_of(Node::Peg(peg)), shot.peg(peg).map(|peg| format!("{}-P", peg.name)))
            }
            None => {
                let shot = shot_id(p, shot)?;
                (shot, p.shot(shot).map_or(Ok(None), |shot| parent_in(shot, parent))?, None)
            }
        };
        let name = match &name {
            Some(name) => {
                let name = checked_name(name)?;
                p.shot(shot).map_or(Ok(()), |shot| name_is_free(shot, name, None))?;
                name.to_string()
            }
            None => unique_name(base.as_deref().unwrap_or("Peg"), p.shot(shot).into_iter().flat_map(|s| s.node_names())),
        };
        let peg = PegId(p.new_id());
        if let Some(shot) = p.shot_mut(shot) {
            shot.pegs.push(Arc::new(Peg {
                id: peg,
                name,
                parent,
                transform: Transform::default(),
                animation: Default::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            }));
        }
        match below {
            Some(Node::Layer(layer)) => p.layer_mut(layer).into_iter().for_each(|e| e.parent = Some(peg)),
            Some(Node::Peg(child)) => p.peg_mut(child).into_iter().for_each(|c| c.parent = Some(peg)),
            None => {}
        }
        // Put above a node, it goes into that node's group, unless it is told another.
        let by = below.map(|node| match node {
            Node::Layer(layer) => layer.0,
            Node::Peg(peg) => peg.0,
        });
        let group = group.or_else(|| super::group::group_by(p.shot(shot)?, by?));
        super::group::put_into(p, shot, group, at, peg.0)?;
        Ok(peg)
    })?;
    Ok(json!({ "id": peg }))
}

/// `peg.new` with `aboveCamera`: a peg between a shot's camera and what the camera is
/// parented to, to move the camera with. Without a name it gets a free "Camera-P" one.
fn above_the_camera(s: &mut Session, shot: Option<u64>, name: Option<String>) -> Result<Value> {
    let id = "peg.new";
    let peg = s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let name = match (&name, p.shot(shot)) {
            (Some(name), Some(shot)) => {
                let name = checked_name(name)?;
                name_is_free(shot, name, None)?;
                name.to_string()
            }
            (_, shot) => unique_name("Camera-P", shot.into_iter().flat_map(|s| s.node_names())),
        };
        let peg = PegId(p.new_id());
        if let Some(shot) = p.shot_mut(shot) {
            let parent = shot.camera.parent;
            shot.pegs.push(Arc::new(Peg {
                id: peg,
                name,
                parent,
                transform: Transform::default(),
                animation: Default::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            }));
            shot.camera.parent = Some(peg);
        }
        Ok(peg)
    })?;
    Ok(json!({ "id": peg }))
}

pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        name: String,
    }
    let id = "peg.rename";
    let Params { peg, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        name_is_free(shot_of(p, peg)?, name, Some(peg))?;
        if let Some(peg) = p.peg_mut(PegId(peg)) {
            peg.name = name.to_string();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Folds a peg's row on the timeline, or opens it: `folded` puts the rows of what the peg
/// carries away. Nothing of the picture changes.
pub(super) fn set_folded(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        folded: bool,
    }
    let id = "peg.setFolded";
    let Params { peg, folded } = parse(id, params)?;
    s.edit(title(id), |p| {
        let within = shot_of(p, peg)?.id;
        if let Some(shot) = p.shot_mut(within) {
            match folded {
                true => drop(shot.folded.insert(PegId(peg))),
                false => drop(shot.folded.remove(&PegId(peg))),
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Deletes a peg. What was parented to it is parented to its parent, and so loses only this
/// peg's transform.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
    }
    let id = "peg.delete";
    let Params { peg } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, peg)?;
        let (shot, peg) = (shot.id, PegId(peg));
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        let parent = shot.parent_of(Node::Peg(peg));
        shot.pegs.retain(|p| p.id != peg);
        shot.places.remove(&peg.0);
        shot.reparent(peg, parent);
        // A deformer it was wired into lets it go: an IK goal stays where it was set, and a
        // start or an end where its own pose has it.
        for chain in &mut shot.deformers {
            let goal = chain.ik.as_ref().is_some_and(|ik| ik.peg == Some(peg));
            if goal || chain.start_peg == Some(peg) || chain.end_peg == Some(peg) {
                let chain = std::sync::Arc::make_mut(chain);
                if let Some(ik) = chain.ik.as_mut().filter(|_| goal) {
                    ik.peg = None;
                }
                if chain.start_peg == Some(peg) {
                    (chain.start_peg, chain.start_bind) = (None, None);
                }
                chain.end_peg = chain.end_peg.filter(|wired| *wired != peg);
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Parents a peg to another peg of its shot, or to nothing (`parent` null or left out).
pub(super) fn set_parent(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        parent: Option<u64>,
    }
    let id = "peg.setParent";
    let Params { peg, parent } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, peg)?;
        let parent = parent_in(shot, parent)?;
        if shot.carried_from(parent, peg) {
            return invalid("a peg can't be parented to itself or to what is under it");
        }
        if let Some(peg) = p.peg_mut(PegId(peg)) {
            peg.parent = parent;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Makes a peg follow a point of a chain of bones or a curve of its shot (plan §4.5), as
/// Harmony's kinematic output: the peg is carried as the point is posed. With `bends`, as a
/// wire made new has unless told, the drawings right under it are bent by the bones as their
/// own drawings are, and the pegs under it are carried; without, nothing it carries is bent.
/// `point` is the joint of a chain or the point of a curve, 0 where it starts; left out it is
/// the end. What of the point's pose the peg takes is what it inherits (`peg.setInherit`).
/// With `deformer` null or left out the peg follows nothing.
pub(super) fn set_follow(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        deformer: Option<u64>,
        point: Option<usize>,
        /// The drawings right under the peg are bent by the bones, not carried: as said, or as
        /// it was for the bones it follows already, or on for a wire made new.
        bends: Option<bool>,
    }
    let id = "peg.setFollow";
    let Params { peg, deformer, point, bends } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, peg)?;
        let was = shot.peg(PegId(peg)).and_then(|peg| peg.follows);
        let follows = match deformer.map(DeformerId) {
            None => None,
            Some(deformer) => {
                let Some(found) = shot.deformer(deformer) else { return invalid(format!("the shot `{}` has no deformer {}", shot.name, deformer.0)) };
                if let Some(point) = point.filter(|point| *point >= found.joints(None).len()) {
                    return invalid(format!("`{}` has {} points: no point {point}", found.name, found.joints(None).len()));
                }
                // What the bones bend is moved by the peg: following them, it would go round.
                if shot.layers.iter().any(|layer| layer.deformer == Some(deformer) && shot.carried_from(layer.parent, peg)) {
                    return invalid(format!("`{}` bends what the peg carries: the peg can't follow it", found.name));
                }
                let bends = bends.unwrap_or_else(|| was.filter(|was| was.deformer == deformer).is_none_or(|was| was.bends));
                Some(Follow { deformer, point, bends })
            }
        };
        if let Some(peg) = p.peg_mut(PegId(peg)) {
            peg.follows = follows;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets what a peg takes from its parent and the bones it follows: `position`, `rotation` and
/// `scale`, each true or false, and those left out as they are. Without the position its
/// pivot stays where its own values put it under the parent's parent; without the rotation
/// or the scale it goes where the parent puts it, but is not turned or scaled by it. The pegs
/// above the parent always count.
pub(super) fn set_inherit(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        position: Option<bool>,
        rotation: Option<bool>,
        scale: Option<bool>,
    }
    let id = "peg.setInherit";
    let Params { peg, position, rotation, scale } = parse(id, params)?;
    s.edit(title(id), |p| {
        shot_of(p, peg)?;
        if let Some(peg) = p.peg_mut(PegId(peg)) {
            let inherit = &mut peg.inherit;
            (inherit.position, inherit.rotation, inherit.scale) =
                (position.unwrap_or(inherit.position), rotation.unwrap_or(inherit.rotation), scale.unwrap_or(inherit.scale));
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// How far a peg may move what is under it, in scene units, and how far it may turn it, in
/// degrees: far more than any shot needs, and far from where the arithmetic loses its footing.
const MAX_OFFSET: f64 = 1_000_000.0;
const MAX_SCALE: f64 = 10_000.0;
/// The fastest a value may move at a keyframe, in steady speeds of the way to the next.
pub(super) const MAX_SLOPE: f64 = 20.0;
/// A skew of 90° would lay the drawing flat on a line.
const MAX_SKEW: f64 = 85.0;
/// A tilt of more than a turn says nothing more.
const MAX_TILT: f64 = 360.0;

fn within(what: &str, values: &[f64], limit: f64) -> Result<()> {
    if values.iter().any(|v| !v.is_finite() || v.abs() > limit) {
        return invalid(format!("the {what} is outside ±{limit}"));
    }
    Ok(())
}

/// Give one value of a transform a new value: on every frame if it has no keys, else as a key
/// on `frame`. That is where `animate` is not said. Said, it is the Animate switch of the
/// window: on, the value is keyed on `frame` whether it had keys or not; off, no key is made,
/// and a value that has keys has them all moved by as much, so that the whole of its animation
/// goes along. A `spatial` value is a place that moves along a path. Where it is keyed on
/// `frame`, its keys on the frames of `adjust` are moved by as much as it moved there.
pub(super) fn put<V: Animatable>(
    what: &str,
    track: &mut Track<V>,
    still: &mut V,
    (frame, animate, spatial, adjust): (Option<u32>, Option<bool>, bool, &[u32]),
    value: Option<V>,
) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    match (animate, track.is_empty(), frame) {
        (Some(true), _, Some(frame)) => track.set_adjusting(frame, value, spatial, adjust),
        (Some(true), _, None) => return invalid(format!("say which `frame` the {what} is keyed on")),
        (_, true, _) => *still = value,
        (Some(false), false, Some(frame)) => track.move_all(frame, value, spatial),
        (None, false, Some(frame)) => track.set_adjusting(frame, value, spatial, adjust),
        (_, false, None) => return invalid(format!("the {what} is animated: say which `frame` to set it on")),
    }
    Ok(())
}

/// Sets the parts of a peg's transform that are given, and leaves the others as they are. A
/// value that has keyframes is set on `frame`, as a key there; one that has none is set for
/// every frame. A `depth` with `keepSize` also sets the position and the scale so that what
/// the peg carries looks the same to the camera on `frame` as before: only its parallax
/// changes (plan §4.4). A `tilt` leans what the peg carries out of the picture plane: its top
/// away from the viewer, and its right side.
/// With `animate` it is as the window's Animate switch has it: `true` keys every value that is
/// given on `frame`, the first key of it if need be; `false` makes no key, and moves all the
/// keys of a value that has them by as much. Where a value is keyed on `frame`, `adjust`, a
/// list of frames, has its keys on those frames moved by as much as it moved on `frame`: the
/// window's Adjust, which changes the keyframes picked on the timeline alike.
pub(super) fn set_transform(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        frame: Option<u32>,
        position: Option<[f64; 2]>,
        rotation: Option<f64>,
        scale: Option<[f64; 2]>,
        skew: Option<f64>,
        pivot: Option<[f64; 2]>,
        depth: Option<f64>,
        tilt: Option<[f64; 2]>,
        #[serde(default, rename = "keepSize")]
        keep_size: bool,
        animate: Option<bool>,
        #[serde(default)]
        adjust: Vec<u32>,
    }
    let id = "peg.setTransform";
    let Params { peg, frame, mut position, rotation, mut scale, skew, pivot, depth, tilt, keep_size, animate, adjust } = parse(id, params)?;
    if keep_size {
        // The position and the scale that make it look the same from the camera at the new depth.
        let (Some(depth), None, None) = (depth, position, scale) else {
            return invalid("`keepSize` goes with a `depth`, and sets the position and the scale itself");
        };
        let shot = shot_of(s.project(), peg)?;
        let Some(kept) = shot.kept_in_view(PegId(peg), depth, frame.unwrap_or(0)) else {
            return invalid("the camera can't see it at that depth, so nothing keeps how it looks");
        };
        (position, scale) = (Some(kept.0), Some(kept.1));
    }
    checked_frames(frame.unwrap_or(0), 1)?;
    within("position", &position.unwrap_or_default(), MAX_OFFSET)?;
    within("rotation", &[rotation.unwrap_or_default()], MAX_OFFSET)?;
    within("scale", &scale.unwrap_or_default(), MAX_SCALE)?;
    within("skew", &[skew.unwrap_or_default()], MAX_SKEW)?;
    within("pivot", &pivot.unwrap_or_default(), MAX_OFFSET)?;
    within("depth", &[depth.unwrap_or_default()], MAX_OFFSET)?;
    within("tilt", &tilt.unwrap_or_default(), MAX_TILT)?;
    // The history says what was done, where it was one thing.
    let label = match (position, rotation, scale, skew, pivot, depth) {
        _ if keep_size => "Move Peg in Depth",
        (None, None, None, None, None, None) if tilt.is_some() => "Tilt Peg",
        _ if tilt.is_some() => title(id),
        (Some(_), None, None, None, None, None) => "Move Peg",
        (None, Some(_), None, None, None, None) => "Rotate Peg",
        (None, None, Some(_), None, None, None) => "Scale Peg",
        (None, None, None, None, Some(_), None) => "Move Peg Pivot",
        (None, None, None, None, None, Some(_)) => "Move Peg in Depth",
        _ => title(id),
    };
    s.edit(label, |p| {
        let Some(peg) = p.peg_mut(PegId(peg)) else { return invalid(format!("no peg {peg}")) };
        let (still, animation) = (&mut peg.transform, &mut peg.animation);
        put("position", &mut animation.position, &mut still.position, (frame, animate, true, &adjust), position)?;
        put("rotation", &mut animation.rotation, &mut still.rotation, (frame, animate, false, &adjust), rotation)?;
        put("scale", &mut animation.scale, &mut still.scale, (frame, animate, false, &adjust), scale)?;
        put("skew", &mut animation.skew, &mut still.skew, (frame, animate, false, &adjust), skew)?;
        put("depth", &mut animation.depth, &mut still.depth, (frame, animate, false, &adjust), depth)?;
        put("tilt", &mut animation.tilt, &mut still.tilt, (frame, animate, false, &adjust), tilt)?;
        still.pivot = pivot.unwrap_or(still.pivot);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// The peg a command names, to change, with a frame the command gives checked.
fn peg_mut(project: &mut Project, peg: u64, frame: u32) -> Result<&mut Peg> {
    checked_frames(frame, 1)?;
    match project.peg_mut(PegId(peg)) {
        Some(peg) => Ok(peg),
        None => invalid(format!("no peg {peg}")),
    }
}

/// A peg that has a keyframe on `frame`: of any of its values, or of the one of `channel`.
fn keyed(project: &mut Project, peg: u64, frame: u32, channel: Option<Channel>) -> Result<&mut Peg> {
    let peg = peg_mut(project, peg, frame)?;
    if !peg.animation.frames(channel).contains(&frame) {
        return invalid(format!("`{}` has no keyframe on frame {frame}", peg.name));
    }
    Ok(peg)
}

/// Keys a peg's position, rotation, scale and skew on `frame`, as they are there; or the one
/// value of `channel` alone. From then on they are animated: setting one on another frame
/// makes a key there.
pub(super) fn set_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        frame: u32,
        channel: Option<Channel>,
    }
    let id = "peg.setKey";
    let Params { peg, frame, channel } = parse(id, params)?;
    s.edit(title(id), |p| {
        peg_mut(p, peg, frame)?.key_channel(frame, channel);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Removes a peg's keyframes on `frame`: of all its values, or of the one of `channel`
/// (`position`, `rotation`, `scale` or `skew`). A value that loses its last key stays as that
/// key had it, on every frame.
pub(super) fn remove_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        frame: u32,
        channel: Option<Channel>,
    }
    let id = "peg.removeKey";
    let Params { peg, frame, channel } = parse(id, params)?;
    s.edit(title(id), |p| {
        keyed(p, peg, frame, channel)?.remove_keys(frame, channel);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Moves a peg's keyframes from one frame to another, over any that are there: those of all
/// its values, or of the one of `channel`.
pub(super) fn move_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        from: u32,
        to: u32,
        channel: Option<Channel>,
    }
    let id = "peg.moveKey";
    let Params { peg, from, to, channel } = parse(id, params)?;
    checked_frames(to, 1)?;
    s.edit(title(id), |p| {
        keyed(p, peg, from, channel)?.animation.move_keys(from, to, channel);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets how a peg's keyframes on `frame` ease: `into` them, `out` of them, or both; those of
/// all its values, or of the one of `channel`. Each side is `linear`, `ease` (at rest at the
/// key), out of a key `hold`, or `{ "curve": { "reach", "speed" } }`: how far the slope
/// reaches towards the other key, as a part of the time between them, and how fast the value
/// moves at the key, where 1 is the steady speed that would get it to the other key.
pub(super) fn set_ease(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        frame: u32,
        channel: Option<Channel>,
        into: Option<Interp>,
        out: Option<Interp>,
    }
    let id = "peg.setEase";
    let Params { peg, frame, channel, into, out } = parse(id, params)?;
    if into == Some(Interp::Hold) {
        return invalid("a value is held out of a keyframe, not into one");
    }
    for side in [into, out] {
        if let Some(Interp::Curve(slope)) = side
            && !((0.01..=1.0).contains(&slope.reach) && slope.speed.is_finite() && slope.speed.abs() <= MAX_SLOPE)
        {
            return invalid(format!("a curve reaches 0.01 to 1 of the way, at a speed within ±{MAX_SLOPE}"));
        }
    }
    s.edit(title(id), |p| {
        keyed(p, peg, frame, channel)?.animation.set_interp(frame, channel, into, out);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Bends a peg's motion path at its position keyframe on `frame`. `into` and `out` are the
/// path's handles there, each from the key: the path arrives from the direction of `into` and
/// leaves towards `out`. A side that is not given stays as it is. Both at `[0, 0]` make a
/// corner. `smooth` instead gives the bend back to the keys around it.
pub(super) fn set_bend(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        frame: u32,
        into: Option<[f64; 2]>,
        out: Option<[f64; 2]>,
        #[serde(default)]
        smooth: bool,
    }
    let id = "peg.setBend";
    let Params { peg, frame, into, out, smooth } = parse(id, params)?;
    within("handle", &[into.unwrap_or_default(), out.unwrap_or_default()].concat(), MAX_OFFSET)?;
    if smooth && (into.is_some() || out.is_some()) {
        return invalid("a smooth bend has no handles of its own");
    }
    s.edit(title(id), |p| {
        let peg = peg_mut(p, peg, frame)?;
        let shown = peg.animation.position.handles(frame);
        let (Some(shown), Some(key)) = (shown, peg.animation.position.key_mut(frame)) else {
            return invalid(format!("the position of `{}` has no keyframe on frame {frame}", peg.name));
        };
        key.bend = (!smooth).then(|| Bend { into: into.unwrap_or(shown.into), out: out.unwrap_or(shown.out) });
        Ok(())
    })?;
    Ok(Value::Null)
}
