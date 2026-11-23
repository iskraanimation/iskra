//! Shot commands: the shots in the project bin. A shot exists whether or not a storyboard uses
//! it (plan §4.1).

use std::sync::Arc;

use iskra_model::{Clip, Numbering, Project, ShotId, StoryboardId};
use serde::Deserialize;
use serde_json::{Value, json};

use super::storyboard::{put, storyboard_id};
use super::{checked_frames, checked_name, invalid, loaded, parse, shot_id, title};
use crate::{Result, Session};

/// The name for a new shot: `name` if given and free, else the first free `Shot_01`, `Shot_02`…
/// Shot names are unique in the project (plan §7.2b).
fn shot_name(project: &Project, name: Option<&str>) -> Result<String> {
    let taken = |name: &str| project.shots.iter().any(|s| s.name == name);
    match name {
        Some(name) => {
            let name = checked_name(name)?;
            if taken(name) {
                return invalid(format!("the project already has a shot named `{name}`"));
            }
            Ok(name.to_string())
        }
        None => Ok((1u32..).map(|n| format!("Shot_{n:02}")).find(|n| !taken(n)).unwrap_or_default()),
    }
}

/// Adds an empty shot to the project, in `bin` or else in Master: one second long unless `frames` says otherwise. With a
/// `storyboard` it is also placed there, whole, at `start` or after the last shot.
///
/// With `after`, a shot on a storyboard, it goes right after that shot and is as long as that
/// one is in the cut, unless `frames` says otherwise (plan §7.2b): where there is room, or
/// with `ripple`; else after the last shot. With `boards` it has a board layer, to draw on at
/// once.
pub(super) fn new(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        name: Option<String>,
        frames: Option<u32>,
        storyboard: Option<u64>,
        start: Option<u32>,
        after: Option<u64>,
        #[serde(default)]
        boards: bool,
        #[serde(default)]
        ripple: bool,
        bin: Option<u64>,
    }
    let id = "shot.new";
    let Params { name, frames, storyboard, start, after, boards, ripple, bin } = parse(id, params)?;
    let shot = s.edit(title(id), |p| {
        // The shot it comes after, with where that is.
        let after = match after {
            Some(after) => {
                let Some(home) = p.storyboard_of(ShotId(after)).and_then(|s| Some((s.id, *s.clip(ShotId(after))?, s.end()))) else {
                    return invalid(format!("shot {after} is not on a storyboard"));
                };
                Some(home)
            }
            None => None,
        };
        let duration = frames.or(after.map(|(_, clip, _)| clip.length())).unwrap_or_else(|| (p.settings.frame_rate.as_f64().round() as u32).max(1));
        checked_frames(0, duration)?;
        let name = shot_name(p, name.as_deref())?;
        let bin = super::bin::bin_in(p, bin)?;
        let mut made = iskra_model::Shot { bin, ..p.new_shot(&name, duration) };
        // Its camera has the peg that moves it from the start.
        made.add_camera_peg(iskra_model::PegId(p.new_id()));
        let shot = made.id;
        p.shots.push(Arc::new(made));
        // Its storyboard's layers, in the shot's group for storyboard drawings.
        if boards {
            p.add_storyboard_layers(shot, false);
        }
        if let Some((storyboard, clip, end)) = after {
            let next = Clip { shot, start: clip.end(), in_point: 0, out_point: duration };
            if put(p, storyboard, next, ripple).is_err() {
                put(p, storyboard, Clip { start: end, ..next }, false)?;
            }
        } else if let Some(storyboard) = storyboard {
            let storyboard = storyboard_id(p, storyboard)?;
            let start = start.unwrap_or_else(|| p.storyboard(storyboard).map_or(0, |s| s.end()));
            put(p, storyboard, Clip { shot, start, in_point: 0, out_point: duration }, ripple)?;
        }
        Ok(shot)
    })?;
    Ok(json!({ "id": shot }))
}

/// Splits a shot in two at `frame` of it, a frame inside its cut on a storyboard (plan §4.7):
/// the shot keeps what is before the frame, and a new shot, named `name` or the next free
/// `Shot_NN`, has the rest, right after it. Nothing is deleted: each is the whole shot, with
/// the other's part as its handles, so the cut between them can still be slid. A transition
/// on the shot's end goes with the new shot. The shot must be loaded.
pub(super) fn split(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: u64,
        frame: u32,
        name: Option<String>,
    }
    let id = "shot.split";
    let Params { shot, frame, name } = parse(id, params)?;
    let made = s.edit(title(id), |p| {
        let shot = shot_id(p, Some(shot))?;
        p.shot(shot).map_or(Ok(()), loaded)?;
        let Some(clip) = p.storyboard_of(shot).and_then(|home| home.clip(shot).copied()) else {
            return invalid("the shot is not on a storyboard: there is no cut to split it in");
        };
        if frame <= clip.in_point || frame >= clip.out_point {
            return invalid(format!("a shot is split inside its cut: after frame {} and before frame {}", clip.in_point, clip.out_point));
        }
        let name = shot_name(p, name.as_deref())?;
        match p.split_shot(shot, frame, &name) {
            Some(made) => Ok(made),
            None => invalid("the shot can't be split there"),
        }
    })?;
    Ok(json!({ "id": made }))
}

/// Merges a shot with the one that follows it on its storyboard, where the two touch (plan
/// §4.7): the shot is then as long in the cut as both were, and the other's layers, pegs and
/// deformers are in it, later by as much as the shot was long, with its panels. The other shot
/// is gone, and a transition on the cut between them with it. What the shot had past its out
/// point, and the other before its in point, shows nothing any more. The shot's camera goes
/// on from the join as the other's did: where it was, and moving as it moved, by keys on its
/// lens and on the peg that moves it, with a held key on the frame before the join. Only
/// where each camera has one peg at most, with none above it; else the other's camera is not
/// kept. Both must be loaded. Returns the `id` of the shot that is left.
pub(super) fn merge(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: u64,
    }
    let id = "shot.merge";
    let Params { shot } = parse(id, params)?;
    s.edit(title(id), |p| {
        let first = shot_id(p, Some(shot))?;
        let next = p.storyboard_of(first).and_then(|home| {
            let clip = home.clip(first)?;
            home.clips().iter().find(|other| other.start == clip.end()).map(|other| other.shot)
        });
        let Some(second) = next else { return invalid("no shot follows it on the storyboard without a gap: there is nothing to merge it with") };
        for shot in [first, second] {
            p.shot(shot).map_or(Ok(()), loaded)?;
        }
        match p.merge_shots(first, second) {
            Ok(()) => Ok(()),
            Err(_) => invalid("the two shots don't touch on a storyboard"),
        }
    })?;
    Ok(json!({ "id": shot }))
}

pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: u64,
        name: String,
    }
    let id = "shot.rename";
    let Params { shot, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        let shot = shot_id(p, Some(shot))?;
        if p.shots.iter().any(|s| s.id != shot && s.name == name) {
            return invalid(format!("the project already has a shot named `{name}`"));
        }
        if let Some(shot) = p.shot_mut(shot) {
            shot.name = name.to_string();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Renames `shots` together, in the order they lie in on the storyboards, as a `text` and a
/// number after it: from `first`, by `step`, padded with zeros to `digits` (`SH010`, `SH020`),
/// as one step. A name another shot has already is refused; the shots renamed may take each
/// other's. Returns the `names`, in the shots' order.
pub(super) fn rename_numbered(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        shots: Vec<u64>,
        #[serde(flatten)]
        numbering: Numbering,
    }
    let id = "shot.renameNumbered";
    let Params { shots, mut numbering } = parse(id, params)?;
    if shots.is_empty() {
        return invalid("name the shots to rename");
    }
    numbering.text = numbering.text.trim_start().to_string();
    if numbering.text.contains('/') {
        return invalid(format!("the name `{}` can't contain `/`", numbering.text));
    }
    if numbering.digits > 9 {
        return invalid("a number is padded to 9 digits at most");
    }
    let names = s.edit(title(id), |p| {
        let shots = shots.iter().map(|shot| shot_id(p, Some(*shot))).collect::<Result<Vec<_>>>()?;
        let named = p.numbered(&shots, &numbering);
        if let Some((_, name, _)) = named.iter().find(|(_, _, taken)| taken.is_some()) {
            return invalid(format!("the project already has a shot named `{name}`"));
        }
        for (shot, name, _) in &named {
            if let Some(shot) = p.shot_mut(*shot) {
                shot.name.clone_from(name);
            }
        }
        Ok(named.into_iter().map(|(shot, name, _)| json!({ "shot": shot, "name": name })).collect::<Vec<_>>())
    })?;
    Ok(json!({ "names": names }))
}

/// A scene's or a sequence's name as typed: empty for none.
fn group_name(name: &str) -> Result<String> {
    match name.trim() {
        "" => Ok(String::new()),
        name => checked_name(name).map(str::to_string),
    }
}

/// Gives `shots` a `scene` and a `sequence`, whichever is given, as one step: a name, or empty
/// for none.
pub(super) fn set_scene(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shots: Vec<u64>,
        scene: Option<String>,
        sequence: Option<String>,
    }
    let id = "shot.setScene";
    let Params { shots, scene, sequence } = parse(id, params)?;
    if shots.is_empty() {
        return invalid("name the shots to set");
    }
    if scene.is_none() && sequence.is_none() {
        return invalid("say the scene or the sequence to set");
    }
    let scene = scene.as_deref().map(group_name).transpose()?;
    let sequence = sequence.as_deref().map(group_name).transpose()?;
    s.edit(title(id), |p| {
        for shot in shots {
            let shot = shot_id(p, Some(shot))?;
            let Some(shot) = p.shot_mut(shot) else { continue };
            if let Some(scene) = &scene {
                shot.scene.clone_from(scene);
            }
            if let Some(sequence) = &sequence {
                shot.sequence.clone_from(sequence);
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Renames a scene, or a sequence with `sequence`, on all its shots: `name` may be one in use
/// already, and the two are then one.
pub(super) fn rename_scene(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        from: String,
        name: String,
        #[serde(default)]
        sequence: bool,
    }
    let id = "shot.renameScene";
    let Params { from, name, sequence } = parse(id, params)?;
    let name = checked_name(&name)?.to_string();
    let what = if sequence { "sequence" } else { "scene" };
    let of = |shot: &iskra_model::Shot| if sequence { shot.sequence.clone() } else { shot.scene.clone() };
    let from = from.trim();
    if from.is_empty() || !s.project().shots.iter().any(|shot| of(shot) == from) {
        return invalid(format!("no shot is in the {what} `{from}`"));
    }
    s.edit(title(id), |p| {
        for shot in p.shots.iter_mut().filter(|shot| of(shot) == from).map(Arc::make_mut) {
            *if sequence { &mut shot.sequence } else { &mut shot.scene } = name.clone();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Deletes a shot with its layers and drawings, and takes it off its storyboard. The shot
/// must be loaded, so that undoing brings its drawings back.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: u64,
    }
    let id = "shot.delete";
    let Params { shot } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_id(p, Some(shot))?;
        p.shot(shot).map_or(Ok(()), loaded)?;
        if let Some(storyboard) = p.storyboard_of(shot).map(|s| s.id).and_then(|id| p.storyboard_mut(id)) {
            storyboard.take(shot);
        }
        p.shots.retain(|s| s.id != shot);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Copies a shot with its layers and drawings, for a continuing action. If the original is on
/// a storyboard the copy goes right after it, with the same in and out points: where there is
/// room, or with `ripple`; else after the last shot.
pub(super) fn duplicate(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: u64,
        name: Option<String>,
        #[serde(default)]
        ripple: bool,
    }
    let id = "shot.duplicate";
    let Params { shot, name, ripple } = parse(id, params)?;
    let copy = s.edit(title(id), |p| {
        let original = shot_id(p, Some(shot))?;
        p.shot(original).map_or(Ok(()), loaded)?;
        let name = shot_name(p, name.as_deref())?;
        let Some(copy) = p.duplicate_shot(original, &name) else { return invalid(format!("no shot {shot}")) };
        let id = copy.id;
        p.shots.push(Arc::new(copy));
        let home: Option<(StoryboardId, Clip, u32)> = p.storyboard_of(original).and_then(|s| Some((s.id, *s.clip(original)?, s.end())));
        if let Some((storyboard, clip, end)) = home {
            let after = Clip { shot: id, start: clip.end(), ..clip };
            if put(p, storyboard, after, ripple).is_err() {
                put(p, storyboard, Clip { start: end, ..after }, false)?;
            }
        }
        Ok(id)
    })?;
    Ok(json!({ "id": copy }))
}

/// Sets a shot's whole length. A shot made shorter than its out point takes its clip's in and
/// out points back with it; a shot made longer keeps them, and the new frames are handles.
pub(super) fn set_duration(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        frames: u32,
    }
    let id = "shot.setDuration";
    let Params { shot, frames } = parse(id, params)?;
    checked_frames(0, frames)?;
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        if let Some(shot) = p.shot_mut(shot) {
            shot.duration = frames;
        }
        let home = p.storyboard_of(shot).and_then(|s| Some((s.id, *s.clip(shot)?)));
        if let Some((storyboard, clip)) = home.filter(|(_, clip)| clip.out_point > frames) {
            let in_point = clip.in_point.min(frames - 1);
            // A shorter clip in the same place always fits.
            put(p, storyboard, Clip { in_point, out_point: frames, ..clip }, false)?;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Puts a marker on `frame` of a shot, or changes the one there: its `name`, its `colour` as
/// `[red, green, blue]`, and with `to` the frame it is on. A new marker is blue and has no
/// name unless they are given. As a storyboard's markers are (`storyboard.setMarker`).
pub(super) fn set_marker(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        frame: u32,
        name: Option<String>,
        colour: Option<[u8; 3]>,
        to: Option<u32>,
    }
    let id = "shot.setMarker";
    let Params { shot, frame, name, colour, to } = parse(id, params)?;
    checked_frames(to.unwrap_or(frame), 1)?;
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        if to.unwrap_or(frame) >= shot.duration {
            return invalid(format!("the shot `{}` has {} frames", shot.name, shot.duration));
        }
        let was = shot.markers.iter().position(|marker| marker.frame == frame).map(|at| shot.markers.remove(at));
        if was.is_none() && to.is_some() {
            return invalid(format!("no marker is on frame {frame}"));
        }
        let mut marker = was.unwrap_or(iskra_model::Marker { frame, name: String::new(), colour: iskra_model::MARKER_COLOURS[0].1 });
        marker.frame = to.unwrap_or(frame);
        marker.name = name.map_or(marker.name, |name| name.trim().to_string());
        marker.colour = colour.unwrap_or(marker.colour);
        shot.put_marker(marker);
        Ok(())
    })?;
    Ok(Value::Null)
}

pub(super) fn remove_marker(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        frame: u32,
    }
    let id = "shot.removeMarker";
    let Params { shot, frame } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        if !shot.markers.iter().any(|marker| marker.frame == frame) {
            return invalid(format!("no marker is on frame {frame}"));
        }
        shot.markers.retain(|marker| marker.frame != frame);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Opens a board to be drawn on on the Animation page, where it is otherwise locked as
/// reference, or locks it again: `layer`, which has to be a board.
pub(super) fn set_unlocked(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        unlocked: bool,
    }
    let id = "shot.setUnlocked";
    let Params { layer, unlocked } = parse(id, params)?;
    let layer = iskra_model::LayerId(layer);
    s.edit(title(id), |p| {
        let Some(within) = p.shots.iter().find(|shot| shot.layer(layer).is_some()).map(|shot| shot.id) else { return invalid(format!("no layer {}", layer.0)) };
        let Some(shot) = p.shot_mut(within) else { return Ok(()) };
        if !shot.is_board(layer) {
            return invalid("only a board is locked: an animation layer is drawn on as it is");
        }
        match unlocked {
            true => drop(shot.unlocked.insert(layer)),
            false => drop(shot.unlocked.remove(&layer)),
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Hides a row of a shot's timeline, or shows it again: `node` is the id of a layer, a peg
/// or a group. A hidden layer is not in what the shot shows, in the window or in a render,
/// nor is one that a hidden peg carries or that is in a hidden group.
pub(super) fn set_hidden(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        node: u64,
        hidden: bool,
    }
    let id = "shot.setHidden";
    let Params { node, hidden } = parse(id, params)?;
    s.edit(title(id), |p| mark_row(p, node, hidden, |shot| &mut shot.hidden))?;
    Ok(Value::Null)
}

/// Locks the row of a layer, a peg or a group on the timeline, or unlocks it: a locked layer
/// is not drawn on nor picked in the viewport, nor is all that a locked peg carries or a
/// locked group has in it. It is seen and rendered as ever.
pub(super) fn set_locked(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        node: u64,
        locked: bool,
    }
    let id = "shot.setLocked";
    let Params { node, locked } = parse(id, params)?;
    s.edit(title(id), |p| mark_row(p, node, locked, |shot| &mut shot.locked))?;
    Ok(Value::Null)
}

/// Puts the row of a layer, a peg or a group into the shot's set of rows that `rows` gives,
/// or takes it out.
fn mark_row(p: &mut Project, node: u64, on: bool, rows: impl Fn(&mut iskra_model::Shot) -> &mut std::collections::BTreeSet<u64>) -> Result<()> {
    let has = |shot: &iskra_model::Shot| {
        shot.layers.iter().any(|layer| layer.id.0 == node) || shot.pegs.iter().any(|peg| peg.id.0 == node) || shot.groups.iter().any(|group| group.id.0 == node)
    };
    let Some(within) = p.shots.iter().find(|shot| has(shot)).map(|shot| shot.id) else { return invalid(format!("no layer, peg or group {node}")) };
    if let Some(shot) = p.shot_mut(within) {
        match on {
            true => drop(rows(shot).insert(node)),
            false => drop(rows(shot).remove(&node)),
        }
    }
    Ok(())
}

/// Sets the stretch of a shot that is played on the Animation page, and gone round when
/// playback loops: `in`, its first frame, `out`, its last, or both; the other stays, or is the
/// shot's own end. With `clear` there is none, and all of the shot plays. It is for playing
/// only: what is rendered is the cut.
pub(super) fn set_range(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        #[serde(rename = "in")]
        first: Option<u32>,
        #[serde(rename = "out")]
        last: Option<u32>,
        #[serde(default)]
        clear: bool,
    }
    let id = "shot.setRange";
    let Params { shot, first, last, clear } = parse(id, params)?;
    if !clear && first.is_none() && last.is_none() {
        return invalid("say what to set: `in`, `out` or `clear`");
    }
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        if clear {
            shot.range = None;
            return Ok(());
        }
        let end = shot.duration.max(1);
        let (was_first, was_after) = shot.range.unwrap_or((0, end));
        let (first, after) = (first.unwrap_or(was_first), last.map_or(was_after, |last| last.saturating_add(1)));
        checked_frames(first, 1)?;
        // An in point after the out point takes the out point along to the shot's end, and
        // an out point before the in point takes the in point back to the start.
        shot.range = Some(match (first < after, last.is_some()) {
            (true, _) => (first, after),
            (false, true) => (0, after),
            (false, false) => (first, end.max(first + 1)),
        });
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Renders one frame of a shot with the reference compositor and writes it as a PNG at
/// `path`, over transparency: the shot's frame, at `scale` times the shot's resolution (1
/// without one). `work` says which of the shot is in it (plan §7.2c): `boards`, `animation`,
/// or without one all of it. The shot must be loaded. Returns the picture's `width` and
/// `height`.
pub(super) fn render_frame(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        #[serde(default)]
        frame: u32,
        path: std::path::PathBuf,
        scale: Option<f64>,
        work: Option<iskra_model::Work>,
    }
    let Params { shot, frame, path, scale, work } = parse("shot.renderFrame", params)?;
    let project = s.project().clone();
    let Some(shot) = project.shot(shot_id(&project, shot)?) else { return Ok(Value::Null) };
    loaded(shot)?;
    checked_frames(frame, 1)?;
    let scale = scale.unwrap_or(1.0);
    let view = iskra_render::View::frame(&project.settings, scale);
    if !(scale > 0.0 && scale.is_finite()) || u64::from(view.width) * u64::from(view.height) > 1 << 28 {
        return invalid(format!("a picture at {scale} times the shot's size can't be made"));
    }
    let shot = &shot.only(work.unwrap_or(iskra_model::Work::Whole));
    let picture = iskra_render::render(shot, &project.palette, project.settings.working_space, frame, &view);
    let png = iskra_io::encode_png(picture.width, picture.height, &picture.pixels).map_err(crate::EngineError::Invalid)?;
    if let Err(why) = std::fs::write(&path, png) {
        return invalid(format!("{}: {why}", path.display()));
    }
    Ok(json!({ "width": picture.width, "height": picture.height }))
}

#[derive(Deserialize)]
struct Named {
    shot: Option<u64>,
}

/// Loads a shot's drawings from the project file, which a project opened from a file needs
/// before the shot can be shown or drawn on. `skipped` lists the tiles that could not be read,
/// if any. Not an undo step.
pub(super) fn load(s: &mut Session, params: Value) -> Result<Value> {
    let Named { shot } = parse("shot.load", params)?;
    let shot = shot_id(s.project(), shot)?;
    let report = s.load_shot(shot)?;
    Ok(json!({ "skipped": report.skipped }))
}

/// Releases a shot's drawings from memory; they stay in the project file. What is not saved
/// yet is kept in memory, and so is what undo still needs. `released` says whether the shot is
/// now not loaded. Not an undo step.
pub(super) fn unload(s: &mut Session, params: Value) -> Result<Value> {
    let Named { shot } = parse("shot.unload", params)?;
    let shot = shot_id(s.project(), shot)?;
    Ok(json!({ "released": s.unload_shot(shot) }))
}
