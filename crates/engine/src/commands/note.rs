//! Retake notes (plan §9.6): notes pinned to a stretch of a shot's frames, each open until it
//! is ticked done.

use std::time::{SystemTime, UNIX_EPOCH};

use iskra_model::{Note, Shot};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{checked_frames, invalid, parse, shot_id, title};
use crate::{Result, Session};

/// The note's frames must lie in the shot.
fn within(shot: &Shot, frame: u32, length: u32) -> Result<()> {
    checked_frames(frame, length)?;
    if frame + length > shot.duration {
        return invalid(format!("the shot `{}` has {} frames", shot.name, shot.duration));
    }
    Ok(())
}

fn text(text: &str) -> Result<String> {
    match text.trim() {
        "" => invalid("a note has some text"),
        text => Ok(text.to_string()),
    }
}

/// Writes a note on `length` frames (1 if not said) of a shot from `frame`, said by `author`,
/// at `written` (seconds since 1970; now if not said). Gives its `id` among the shot's notes.
pub(super) fn add(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        frame: u32,
        #[serde(default)]
        length: Option<u32>,
        text: String,
        #[serde(default)]
        author: String,
        written: Option<u64>,
    }
    let id = "note.add";
    let Params { shot, frame, length, text: said, author, written } = parse(id, params)?;
    let said = text(&said)?;
    let written = written.unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs()));
    let mut made = 0;
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        let length = length.unwrap_or(1);
        within(shot, frame, length)?;
        made = shot.new_note_id();
        shot.put_note(Note { id: made, frame, length, text: said, author: author.trim().to_string(), written, done: false });
        Ok(())
    })?;
    Ok(json!({ "id": made }))
}

/// Changes a shot's note `note`: its `text`, its `frame` and `length`, and whether it is
/// `done`.
pub(super) fn set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        note: u64,
        text: Option<String>,
        frame: Option<u32>,
        length: Option<u32>,
        done: Option<bool>,
    }
    let id = "note.set";
    let Params { shot, note, text: said, frame, length, done } = parse(id, params)?;
    let said = said.as_deref().map(text).transpose()?;
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        let Some(mut changed) = shot.note(note).cloned() else { return invalid(format!("the shot `{}` has no note {note}", shot.name)) };
        changed.frame = frame.unwrap_or(changed.frame);
        changed.length = length.unwrap_or(changed.length);
        within(shot, changed.frame, changed.length)?;
        changed.text = said.unwrap_or(changed.text);
        changed.done = done.unwrap_or(changed.done);
        shot.put_note(changed);
        Ok(())
    })?;
    Ok(Value::Null)
}

pub(super) fn remove(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        note: u64,
    }
    let id = "note.remove";
    let Params { shot, note } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        if shot.note(note).is_none() {
            return invalid(format!("the shot `{}` has no note {note}", shot.name));
        }
        shot.notes.retain(|other| other.id != note);
        Ok(())
    })?;
    Ok(Value::Null)
}
