//! Sound commands (plan §7.2, R21): the clips of sound media on a storyboard's sound tracks.

use iskra_model::{MediaId, MediaKind, Project, SoundClip, SoundClipId, StoryboardId};
use serde::Deserialize;
use serde_json::{Value, json};

use super::storyboard::storyboard_id;
use super::{checked_frames, checked_name, invalid, parse, title};
use crate::{Result, Session};

/// The storyboard a command works on: the one it names, else the project's first.
fn board(project: &Project, storyboard: Option<u64>) -> Result<StoryboardId> {
    match (storyboard, project.storyboards.first()) {
        (Some(id), _) => storyboard_id(project, id),
        (None, Some(first)) => Ok(first.id),
        (None, None) => invalid("the project has no storyboard"),
    }
}

/// The storyboard a sound clip is on.
fn home(project: &Project, clip: u64) -> Result<(StoryboardId, usize, SoundClip)> {
    let found = project.storyboards.iter().find_map(|board| board.sound_clip(SoundClipId(clip)).map(|(track, clip)| (board.id, track, *clip)));
    match found {
        Some(found) => Ok(found),
        None => invalid(format!("no sound clip {clip}")),
    }
}

/// How many frames of the film a sound item lasts.
fn sound_length(project: &Project, media: MediaId) -> Result<u32> {
    match project.media(media) {
        Some(found) if matches!(found.kind, MediaKind::Sound { .. }) => Ok(found.kind.length(project.settings.frame_rate).max(1)),
        Some(found) => invalid(format!("`{}` is {}, not a sound", found.name, found.kind.name().to_lowercase())),
        None => invalid(format!("no media item {}", media.0)),
    }
}

/// Lays a sound item on a storyboard's sound track as a clip of all of it, starting on the
/// frame `start` of the film: on `track` (counted from 0) if that has room there, else on the
/// first track that has, else on a new one. Returns the clip's `id` and the `track` it is on.
pub(super) fn place(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: Option<u64>,
        media: u64,
        #[serde(default)]
        start: u32,
        track: Option<usize>,
    }
    let id = "sound.place";
    let Params { storyboard, media, start, track } = parse(id, params)?;
    let made = s.edit(title(id), |p| {
        let (storyboard, length) = (board(p, storyboard)?, sound_length(p, MediaId(media))?);
        checked_frames(start, length)?;
        let clip = SoundClip { id: SoundClipId(p.new_id()), media: MediaId(media), start, offset: 0, length, gain: 1.0 };
        let track = p.storyboard_mut(storyboard).map_or(0, |board| board.put_sound(clip, track));
        Ok((clip.id, track))
    })?;
    Ok(json!({ "id": made.0, "track": made.1 }))
}

/// Changes a sound clip: where it `start`s, how far into its sound it starts (`offset`), its
/// `length`, its `gain`, and the `track` it is on. What is not given stays. A clip can't be
/// longer than what is left of its sound, and where it would run into another clip on its
/// track it goes to the first track that has room, as when it is placed. To trim a clip's
/// start, give the new `start`, `offset` and `length` together.
pub(super) fn set_clip(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        clip: u64,
        start: Option<u32>,
        offset: Option<u32>,
        length: Option<u32>,
        gain: Option<f32>,
        track: Option<usize>,
    }
    let id = "sound.setClip";
    let Params { clip, start, offset, length, gain, track } = parse(id, params)?;
    if gain.is_some_and(|gain| !(0.0..=8.0).contains(&gain)) {
        return invalid("the gain is outside 0 to 8");
    }
    // The history says what was done, where it was one thing.
    let label = match (start, offset.or(length), gain, track) {
        (Some(_), None, None, _) | (None, None, None, Some(_)) => "Move Sound",
        (_, Some(_), None, None) => "Trim Sound",
        (None, None, Some(_), None) => "Set Sound Gain",
        _ => title(id),
    };
    let on = s.edit(label, |p| {
        let (storyboard, was_on, was) = home(p, clip)?;
        let changed = SoundClip {
            start: start.unwrap_or(was.start),
            offset: offset.unwrap_or(was.offset),
            length: length.unwrap_or(was.length),
            gain: gain.unwrap_or(was.gain),
            ..was
        };
        let whole = sound_length(p, was.media)?;
        if changed.length == 0 || u64::from(changed.offset) + u64::from(changed.length) > u64::from(whole) {
            return invalid(format!("the clip would be outside its sound, which is {whole} frames long"));
        }
        checked_frames(changed.start, changed.length)?;
        Ok(p.storyboard_mut(storyboard).map_or(was_on, |board| board.put_sound(changed, Some(track.unwrap_or(was_on)))))
    })?;
    Ok(json!({ "track": on }))
}

/// Takes a sound clip off its track. The sound item stays in the project.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        clip: u64,
    }
    let id = "sound.delete";
    let Params { clip } = parse(id, params)?;
    s.edit(title(id), |p| {
        let (storyboard, ..) = home(p, clip)?;
        if let Some(board) = p.storyboard_mut(storyboard) {
            board.take_sound(SoundClipId(clip));
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Changes a sound track, counted from 0: its `name`, and whether it is `mute`. With `remove`
/// the track goes, with the clips on it.
pub(super) fn set_track(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: Option<u64>,
        track: usize,
        name: Option<String>,
        mute: Option<bool>,
        #[serde(default)]
        remove: bool,
    }
    let id = "sound.setTrack";
    let Params { storyboard, track, name, mute, remove } = parse(id, params)?;
    let name = name.as_deref().map(checked_name).transpose()?;
    s.edit(if remove { "Remove Sound Track" } else { title(id) }, |p| {
        let storyboard = board(p, storyboard)?;
        let Some(board) = p.storyboard_mut(storyboard) else { return Ok(()) };
        if track >= board.sound.len() {
            return invalid(format!("the storyboard has no sound track {track}"));
        }
        if remove {
            board.sound.remove(track);
            return Ok(());
        }
        let of = &mut board.sound[track];
        (of.name, of.mute) = (name.map_or(of.name.clone(), str::to_string), mute.unwrap_or(of.mute));
        Ok(())
    })?;
    Ok(Value::Null)
}
