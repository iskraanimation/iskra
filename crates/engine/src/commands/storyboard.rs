//! Storyboard commands: the timelines of shots (plan §7.2b). A storyboard has one shot track
//! with free placement; `ripple` on a command says whether the later shots move along to make
//! room or stay, in which case what would run into the next shot is refused.

use std::sync::Arc;

use iskra_model::{Clip, Edge, Project, ShotId, Storyboard, StoryboardId, Transition, TransitionKind, unique_name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{checked_frames, checked_name, invalid, parse, shot_id, title};
use crate::{Result, Session};

/// The storyboard a command names.
pub(super) fn storyboard_id(project: &Project, id: u64) -> Result<StoryboardId> {
    match project.storyboard(StoryboardId(id)) {
        Some(storyboard) => Ok(storyboard.id),
        None => invalid(format!("no storyboard {id}")),
    }
}

/// The storyboard a shot is on, and its clip there.
fn home(project: &Project, shot: ShotId) -> Result<(StoryboardId, Clip)> {
    match project.storyboard_of(shot).and_then(|s| Some((s.id, *s.clip(shot)?))) {
        Some(home) => Ok(home),
        None => invalid(format!("shot {} is not on a storyboard", shot.0)),
    }
}

/// Place a clip on a storyboard, or place it again after a change.
pub(super) fn put(project: &mut Project, storyboard: StoryboardId, clip: Clip, ripple: bool) -> Result<()> {
    checked_frames(clip.start, clip.length())?;
    let Some(board) = project.storyboard_mut(storyboard) else { return invalid(format!("no storyboard {}", storyboard.0)) };
    let Err(overlap) = board.put(clip, ripple) else { return Ok(()) };
    let name = |id| project.shot(id).map_or("?", |s| s.name.as_str());
    invalid(format!("`{}` would run into `{}`", name(clip.shot), name(overlap.0)))
}

/// Adds an empty storyboard. Without a name it gets a free "Storyboard" one.
pub(super) fn new(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        name: Option<String>,
        bin: Option<u64>,
    }
    let id = "storyboard.new";
    let Params { name, bin } = parse(id, params)?;
    let storyboard = s.edit(title(id), |p| {
        let bin = super::bin::bin_in(p, bin)?;
        let taken = p.storyboards.iter().map(|s| s.name.as_str());
        let name = match &name {
            Some(name) => {
                let name = checked_name(name)?;
                if taken.clone().any(|t| t == name) {
                    return invalid(format!("the project already has a storyboard named `{name}`"));
                }
                name.to_string()
            }
            None => unique_name("Storyboard", taken),
        };
        let id = StoryboardId(p.new_id());
        let mut storyboard = Storyboard::new(id, &name);
        storyboard.bin = bin;
        p.storyboards.push(Arc::new(storyboard));
        Ok(id)
    })?;
    Ok(json!({ "id": storyboard }))
}

/// Sets the number a storyboard's first frame has when its times are told as timecode.
pub(super) fn set_start(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: u64,
        frame: u32,
    }
    let id = "storyboard.setStart";
    let Params { storyboard, frame } = parse(id, params)?;
    super::checked_frames(frame, 1)?;
    s.edit(title(id), |p| {
        let storyboard = storyboard_id(p, storyboard)?;
        if let Some(storyboard) = p.storyboard_mut(storyboard) {
            storyboard.start = frame;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: u64,
        name: String,
    }
    let id = "storyboard.rename";
    let Params { storyboard, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        let storyboard = storyboard_id(p, storyboard)?;
        if p.storyboards.iter().any(|s| s.id != storyboard && s.name == name) {
            return invalid(format!("the project already has a storyboard named `{name}`"));
        }
        if let Some(storyboard) = p.storyboard_mut(storyboard) {
            storyboard.name = name.to_string();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Deletes a storyboard. Its shots stay in the project.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: u64,
    }
    let id = "storyboard.delete";
    let Params { storyboard } = parse(id, params)?;
    s.edit(title(id), |p| {
        let storyboard = storyboard_id(p, storyboard)?;
        p.storyboards.retain(|s| s.id != storyboard);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Puts a shot of the project on a storyboard, whole, at `start` or after the last shot. A shot
/// sits on at most one storyboard, once (plan §7.2).
pub(super) fn place(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: u64,
        shot: u64,
        start: Option<u32>,
        #[serde(default)]
        ripple: bool,
    }
    let id = "storyboard.place";
    let Params { storyboard, shot, start, ripple } = parse(id, params)?;
    s.edit(title(id), |p| {
        let storyboard = storyboard_id(p, storyboard)?;
        let shot = shot_id(p, Some(shot))?;
        if let Some(other) = p.storyboard_of(shot) {
            return invalid(format!("the shot is already on `{}`; duplicate it to use it twice", other.name));
        }
        let duration = p.shot(shot).map_or(0, |s| s.duration);
        let start = start.unwrap_or_else(|| p.storyboard(storyboard).map_or(0, |s| s.end()));
        put(p, storyboard, Clip { shot, start, in_point: 0, out_point: duration }, ripple)
    })?;
    Ok(Value::Null)
}

/// Takes a shot off its storyboard. The shot stays in the project.
pub(super) fn remove(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: u64,
    }
    let id = "storyboard.remove";
    let Params { shot } = parse(id, params)?;
    s.edit(title(id), |p| {
        let (storyboard, clip) = home(p, ShotId(shot))?;
        if let Some(storyboard) = p.storyboard_mut(storyboard) {
            storyboard.take(clip.shot);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Moves a shot's clip to start on another frame of its storyboard.
pub(super) fn move_clip(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: u64,
        start: u32,
        #[serde(default)]
        ripple: bool,
    }
    let id = "storyboard.moveClip";
    let Params { shot, start, ripple } = parse(id, params)?;
    s.edit(title(id), |p| {
        let (storyboard, clip) = home(p, ShotId(shot))?;
        put(p, storyboard, Clip { start, ..clip }, ripple)
    })?;
    Ok(Value::Null)
}

/// Sets a clip's in point, out point or both: which part of the shot is in the cut. What stays
/// in the cut keeps its place on the storyboard, so a new in point moves the clip's start with
/// it. An out point past the shot's last frame makes the shot longer (plan §7.2b, provisional).
/// Nothing is ever deleted by trimming.
pub(super) fn trim_clip(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: u64,
        #[serde(rename = "in")]
        in_point: Option<u32>,
        #[serde(rename = "out")]
        out_point: Option<u32>,
        #[serde(default)]
        ripple: bool,
    }
    let id = "storyboard.trimClip";
    let Params { shot, in_point, out_point, ripple } = parse(id, params)?;
    s.edit(title(id), |p| {
        let (storyboard, clip) = home(p, ShotId(shot))?;
        let (in_point, out_point) = (in_point.unwrap_or(clip.in_point), out_point.unwrap_or(clip.out_point));
        if in_point >= out_point {
            return invalid("the out point comes after the in point");
        }
        checked_frames(in_point, out_point - in_point)?;
        // The clip's start moves by as much as its in point, and can't move before frame 0.
        let Some(start) = (i64::from(clip.start) + i64::from(in_point) - i64::from(clip.in_point)).try_into().ok() else {
            return invalid("the storyboard has no room before its first frame; move the shot later first");
        };
        if let Some(shot) = p.shot_mut(clip.shot).filter(|s| s.duration < out_point) {
            shot.duration = out_point;
        }
        put(p, storyboard, Clip { shot: clip.shot, start, in_point, out_point }, ripple)
    })?;
    Ok(Value::Null)
}

#[derive(Deserialize)]
struct OnEdge {
    shot: u64,
    edge: Edge,
}

/// The edge a transition asked for on `clip`'s `edge` is kept on, and the longest it can be
/// there. Where another clip meets the edge the transition lies across the cut and uses the
/// handles of both shots; on a free edge it is a fade inside the clip.
fn transition_place(project: &Project, storyboard: &Storyboard, clip: Clip, edge: Edge) -> (ShotId, Edge, u32) {
    let (outgoing, incoming) = match edge {
        Edge::Start => (storyboard.clips().iter().find(|c| c.end() == clip.start).copied(), clip),
        Edge::End => match storyboard.clips().iter().find(|c| c.start == clip.end()) {
            Some(next) => (Some(clip), *next),
            None => return (clip.shot, edge, clip.length()),
        },
    };
    let Some(outgoing) = outgoing else { return (clip.shot, edge, clip.length()) };
    let tail = project.shot(outgoing.shot).map_or(0, |s| s.duration.saturating_sub(outgoing.out_point));
    (outgoing.shot, Edge::End, 2 * tail.min(incoming.in_point))
}

/// Puts a transition on a cut or on a shot's free edge, or changes the one there. A transition
/// across a cut uses the handles: if the shots have too few frames beyond their out and in
/// points it is shortened to what there is (plan §7.2b, provisional). Returns the length it got.
pub(super) fn set_transition(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        #[serde(flatten)]
        on: OnEdge,
        kind: TransitionKind,
        length: u32,
    }
    let id = "storyboard.setTransition";
    let Params { on, kind, length } = parse(id, params)?;
    if length == 0 {
        return invalid("a transition is at least one frame long");
    }
    let length = s.edit(title(id), |p| {
        let (storyboard, clip) = home(p, ShotId(on.shot))?;
        let Some(board) = p.storyboard(storyboard) else { return Ok(0) };
        let (shot, edge, room) = transition_place(p, board, clip, on.edge);
        if room == 0 {
            return invalid("the shots have no handle frames at this cut: trim their in and out points, or lengthen them");
        }
        let transition = Transition { shot, edge, kind, length: length.min(room) };
        if let Some(board) = p.storyboard_mut(storyboard) {
            board.transitions.retain(|t| (t.shot, t.edge) != (shot, edge));
            board.transitions.push(transition);
        }
        Ok(transition.length)
    })?;
    Ok(json!({ "length": length }))
}

pub(super) fn remove_transition(s: &mut Session, params: Value) -> Result<Value> {
    let id = "storyboard.removeTransition";
    let on: OnEdge = parse(id, params)?;
    s.edit(title(id), |p| {
        let (storyboard, clip) = home(p, ShotId(on.shot))?;
        let Some(board) = p.storyboard(storyboard) else { return Ok(()) };
        let (shot, edge, _) = transition_place(p, board, clip, on.edge);
        if board.transition(shot, edge).is_none() {
            return invalid("there is no transition there");
        }
        if let Some(board) = p.storyboard_mut(storyboard) {
            board.transitions.retain(|t| (t.shot, t.edge) != (shot, edge));
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Puts a marker on `frame` of a storyboard's film, or changes the one there: its `name`, its
/// `colour` as `[red, green, blue]`, and with `to` the frame it is on. A new marker is blue
/// and has no name unless they are given.
pub(super) fn set_marker(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: u64,
        frame: u32,
        name: Option<String>,
        colour: Option<[u8; 3]>,
        to: Option<u32>,
    }
    let id = "storyboard.setMarker";
    let Params { storyboard, frame, name, colour, to } = parse(id, params)?;
    checked_frames(to.unwrap_or(frame), 1)?;
    s.edit(title(id), |p| {
        let storyboard = storyboard_id(p, storyboard)?;
        let Some(board) = p.storyboard_mut(storyboard) else { return Ok(()) };
        let was = board.markers.iter().position(|marker| marker.frame == frame).map(|at| board.markers.remove(at));
        if was.is_none() && to.is_some() {
            return invalid(format!("no marker is on frame {frame}"));
        }
        let mut marker = was.unwrap_or(iskra_model::Marker { frame, name: String::new(), colour: iskra_model::MARKER_COLOURS[0].1 });
        marker.frame = to.unwrap_or(frame);
        marker.name = name.map_or(marker.name, |name| name.trim().to_string());
        marker.colour = colour.unwrap_or(marker.colour);
        board.put_marker(marker);
        Ok(())
    })?;
    Ok(Value::Null)
}

pub(super) fn remove_marker(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: u64,
        frame: u32,
    }
    let id = "storyboard.removeMarker";
    let Params { storyboard, frame } = parse(id, params)?;
    s.edit(title(id), |p| {
        let storyboard = storyboard_id(p, storyboard)?;
        let Some(board) = p.storyboard_mut(storyboard) else { return Ok(()) };
        if !board.markers.iter().any(|marker| marker.frame == frame) {
            return invalid(format!("no marker is on frame {frame}"));
        }
        board.markers.retain(|marker| marker.frame != frame);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets the stretch of a storyboard's film that is played, and gone round when playback
/// loops: `in`, its first frame, `out`, its last, or both; the other stays, or is the film's
/// own end. With `clear` there is none, and all of the film plays.
pub(super) fn set_range(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: u64,
        #[serde(rename = "in")]
        first: Option<u32>,
        #[serde(rename = "out")]
        last: Option<u32>,
        #[serde(default)]
        clear: bool,
    }
    let id = "storyboard.setRange";
    let Params { storyboard, first, last, clear } = parse(id, params)?;
    if !clear && first.is_none() && last.is_none() {
        return invalid("say what to set: `in`, `out` or `clear`");
    }
    s.edit(title(id), |p| {
        let storyboard = storyboard_id(p, storyboard)?;
        let Some(board) = p.storyboard_mut(storyboard) else { return Ok(()) };
        if clear {
            board.range = None;
            return Ok(());
        }
        let (was_first, was_after) = board.range.unwrap_or((0, board.end().max(1)));
        let (first, after) = (first.unwrap_or(was_first), last.map_or(was_after, |last| last.saturating_add(1)));
        checked_frames(first, 1)?;
        // An in point after the out point takes the out point along to the film's end, and
        // an out point before the in point takes the in point back to the start.
        board.range = Some(match (first < after, last.is_some()) {
            (true, _) => (first, after),
            (false, true) => (0, after),
            (false, false) => (first, board.end().max(first + 1)),
        });
        Ok(())
    })?;
    Ok(Value::Null)
}
