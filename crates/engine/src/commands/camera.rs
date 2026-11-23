//! Camera commands (plan §4.4): the peg that moves a shot's camera, and its lens. Where the
//! camera is, and how it moves, is the transform of the peg it is parented to.

use iskra_model::{Camera, Interp};
use serde::Deserialize;
use serde_json::Value;

use super::peg::parent_in;
use super::{checked_frames, invalid, parse, shot_id, title};
use crate::{Result, Session};

/// Parents a shot's camera to a peg of the shot, which then moves it, or to nothing (`parent`
/// null or left out), which puts it back at home.
pub(super) fn set_parent(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        parent: Option<u64>,
    }
    let id = "camera.setParent";
    let Params { shot, parent } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let parent = p.shot(shot).map_or(Ok(None), |shot| parent_in(shot, parent))?;
        if let Some(shot) = p.shot_mut(shot) {
            shot.camera.parent = parent;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets a shot's camera's lens: its `zoom` (1 shows the plane at depth 0 as drawn), and its
/// `fieldOfView`, top to bottom in degrees, which says how strongly depth shows. Whatever
/// the field of view, the plane at depth 0 fills the frame. What is not given stays. A value
/// that has keyframes is set on `frame`, as a key there; one that has none is set for every
/// frame. With `animate` it is as the window's Animate switch has it, and `adjust` as its
/// Adjust, as for a peg (`peg.setTransform`).
pub(super) fn set_lens(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        shot: Option<u64>,
        frame: Option<u32>,
        zoom: Option<f64>,
        field_of_view: Option<f64>,
        animate: Option<bool>,
        #[serde(default)]
        adjust: Vec<u32>,
    }
    let id = "camera.setLens";
    let Params { shot, frame, zoom, field_of_view, animate, adjust } = parse(id, params)?;
    checked_frames(frame.unwrap_or(0), 1)?;
    if zoom.is_some_and(|zoom| !(0.01..=100.0).contains(&zoom)) {
        return invalid("the zoom is outside 0.01 to 100");
    }
    if field_of_view.is_some_and(|degrees| !(1.0..=150.0).contains(&degrees)) {
        return invalid("the field of view is outside 1° to 150°");
    }
    s.edit(title(id), |p| {
        let (shot, height) = (shot_id(p, shot)?, p.settings.height);
        if let Some(shot) = p.shot_mut(shot) {
            let camera = &mut shot.camera;
            let distance = field_of_view.map(|degrees| Camera::distance_for(degrees, height));
            super::peg::put("zoom", &mut camera.animation.zoom, &mut camera.zoom, (frame, animate, false, &adjust), zoom)?;
            super::peg::put("field of view", &mut camera.animation.distance, &mut camera.distance, (frame, animate, false, &adjust), distance)?;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Keys a shot's camera's lens on `frame`, its zoom and its field of view as they are there, or
/// with `remove` takes the lens's keys on that frame away. A value that loses its last key
/// stays as that key had it. With `withPeg` the peg that moves the camera is keyed with the
/// lens, or loses its keys with it: the whole of the camera, in one step.
pub(super) fn set_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        shot: Option<u64>,
        frame: u32,
        #[serde(default)]
        remove: bool,
        #[serde(default)]
        with_peg: bool,
    }
    let id = "camera.setKey";
    let Params { shot, frame, remove, with_peg } = parse(id, params)?;
    checked_frames(frame, 1)?;
    let label = match (remove, with_peg) {
        (false, false) => title(id),
        (true, false) => "Remove Lens Keyframe",
        (false, true) => "Add Camera Keyframe",
        (true, true) => "Remove Camera Keyframe",
    };
    s.edit(label, |p| {
        let shot = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        let keyed = if with_peg { shot.camera_keys() } else { shot.camera.animation.frames() };
        if remove && !keyed.contains(&frame) {
            return invalid(format!("the {} has no keyframe on frame {frame}", if with_peg { "camera" } else { "lens" }));
        }
        if let Some(peg) = shot.camera.parent.filter(|_| with_peg).and_then(|peg| shot.pegs.iter_mut().find(|other| other.id == peg)) {
            let peg = std::sync::Arc::make_mut(peg);
            if remove { peg.remove_keys(frame, None) } else { peg.key_all(frame) }
        }
        let camera = &mut shot.camera;
        if !remove {
            let (zoom, distance) = (camera.zoom_at(frame), camera.distance_at(frame));
            camera.animation.zoom.set(frame, zoom);
            camera.animation.distance.set(frame, distance);
            return Ok(());
        }
        for (track, still) in [(&mut camera.animation.zoom, &mut camera.zoom), (&mut camera.animation.distance, &mut camera.distance)] {
            if let (Some(key), true) = (track.remove(frame), track.is_empty()) {
                *still = key.value;
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Takes the camera's keyframes on `frame` away, as `camera.setKey` with `remove` does: those
/// of its lens and of the peg that moves it, or with `lensOnly` those of the lens alone.
pub(super) fn remove_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        shot: Option<u64>,
        frame: u32,
        #[serde(default)]
        lens_only: bool,
    }
    let Params { shot, frame, lens_only } = parse("camera.removeKey", params)?;
    set_key(s, serde_json::json!({ "shot": shot, "frame": frame, "remove": true, "withPeg": !lens_only }))
}

/// Moves the camera's keyframes on the frame `from` to the frame `to`: those of its lens and
/// those of the peg that moves it, together, or with `lensOnly` those of the lens alone. Keys
/// that are on `to` already are replaced.
pub(super) fn move_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        shot: Option<u64>,
        from: u32,
        to: u32,
        #[serde(default)]
        lens_only: bool,
    }
    let id = "camera.moveKey";
    let Params { shot, from, to, lens_only } = parse(id, params)?;
    checked_frames(to, 1)?;
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        let keyed = if lens_only { shot.camera.animation.frames() } else { shot.camera_keys() };
        if !keyed.contains(&from) {
            return invalid(format!("the {} has no keyframe on frame {from}", if lens_only { "lens" } else { "camera" }));
        }
        if let Some(peg) = shot.camera.parent.filter(|_| !lens_only).and_then(|peg| shot.pegs.iter_mut().find(|other| other.id == peg)) {
            std::sync::Arc::make_mut(peg).animation.move_keys(from, to, None);
        }
        shot.camera.animation.zoom.move_key(from, to);
        shot.camera.animation.distance.move_key(from, to);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets how the camera's keyframes on `frame` ease, of its lens and of the peg that moves it:
/// `into` the key and `out` of it, as for a peg (`peg.setEase`). A side that is not given
/// stays as it is. With `lensOnly`, those of the lens alone.
pub(super) fn set_ease(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        shot: Option<u64>,
        frame: u32,
        into: Option<Interp>,
        out: Option<Interp>,
        #[serde(default)]
        lens_only: bool,
    }
    let id = "camera.setEase";
    let Params { shot, frame, into, out, lens_only } = parse(id, params)?;
    if into == Some(Interp::Hold) {
        return invalid("a value is held out of a keyframe, not into one");
    }
    if [into, out].iter().any(|side| matches!(side, Some(Interp::Curve(_)))) {
        return invalid("a curve is set on a peg's value, in the function curves");
    }
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        let keyed = if lens_only { shot.camera.animation.frames() } else { shot.camera_keys() };
        if !keyed.contains(&frame) {
            return invalid(format!("the {} has no keyframe on frame {frame}", if lens_only { "lens" } else { "camera" }));
        }
        if let Some(peg) = shot.camera.parent.filter(|_| !lens_only).and_then(|peg| shot.pegs.iter_mut().find(|other| other.id == peg)) {
            std::sync::Arc::make_mut(peg).animation.set_interp(frame, None, into, out);
        }
        let lens = &mut shot.camera.animation;
        for key in [lens.zoom.key_mut(frame), lens.distance.key_mut(frame)].into_iter().flatten() {
            (key.into, key.out) = (into.unwrap_or(key.into), out.unwrap_or(key.out));
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
