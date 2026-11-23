//! Panel commands (plan §4.7): the frame markers a storyboard is read by, and what is written
//! of each. A panel is named by its shot and the frame it starts on. A shot's first panel, on
//! frame 0, is always there: it is written of like any other, and neither moved nor removed.

use iskra_model::{Panel, Project, Shot, ShotId};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{invalid, parse, shot_id, title};
use crate::{Result, Session};

/// What is written of a panel, as a command gives it: what is not given stays as it is.
#[derive(Deserialize, Default)]
struct Texts {
    action: Option<String>,
    dialogue: Option<String>,
    notes: Option<String>,
}

impl Texts {
    fn write(self, panel: &mut Panel) {
        for (text, to) in [(self.action, &mut panel.action), (self.dialogue, &mut panel.dialogue), (self.notes, &mut panel.notes)] {
            if let Some(text) = text {
                *to = text.trim_end().to_string();
            }
        }
    }
}

/// A frame of `shot` that a panel may start on.
fn checked_frame(shot: &Shot, frame: u32) -> Result<u32> {
    if frame >= shot.duration {
        return invalid(format!("`{}` has no frame {frame}: it is {} frames long", shot.name, shot.duration));
    }
    Ok(frame)
}

fn shot_mut(project: &mut Project, shot: ShotId) -> Result<&mut Shot> {
    match project.shot_mut(shot) {
        Some(shot) => Ok(shot),
        None => invalid(format!("no shot {}", shot.0)),
    }
}

/// Adds a panel to a shot, starting on `frame`: from there on the shot shows another picture
/// of the storyboard. `action`, `dialogue` and `notes` are what is written of it.
pub(super) fn add(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        frame: u32,
        #[serde(flatten)]
        texts: Texts,
    }
    let id = "panel.add";
    let Params { shot, frame, texts } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_mut(p, shot_id(p, shot)?)?;
        checked_frame(shot, frame)?;
        if shot.panel_on(frame).is_some() {
            return invalid(format!("`{}` has a panel that starts on frame {frame} already", shot.name));
        }
        let mut panel = Panel { frame, ..Panel::default() };
        texts.write(&mut panel);
        shot.put_panel(panel);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Writes what is said of the panel that starts on `frame` of a shot: `action`, `dialogue`,
/// `notes`, those that are given.
pub(super) fn set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        #[serde(default)]
        frame: u32,
        #[serde(flatten)]
        texts: Texts,
    }
    let id = "panel.set";
    let Params { shot, frame, texts } = parse(id, params)?;
    if texts.action.is_none() && texts.dialogue.is_none() && texts.notes.is_none() {
        return invalid("say what to write: `action`, `dialogue` or `notes`");
    }
    s.edit(title(id), |p| {
        let shot = shot_mut(p, shot_id(p, shot)?)?;
        let Some(mut panel) = shot.panel_on(frame) else { return invalid(format!("no panel of `{}` starts on frame {frame}", shot.name)) };
        texts.write(&mut panel);
        // The first panel is kept in the shot only while something is written of it.
        if panel.frame == 0 && panel.is_blank() {
            shot.take_panel(0);
        } else {
            shot.put_panel(panel);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// The panel that starts on `frame` of a shot, which is not the shot's first.
fn later_panel(shot: &Shot, frame: u32, what: &str) -> Result<Panel> {
    if frame == 0 {
        return invalid(format!("a shot's first panel is the shot's own: it is not {what}"));
    }
    match shot.panel_on(frame) {
        Some(panel) => Ok(panel),
        None => invalid(format!("no panel of `{}` starts on frame {frame}", shot.name)),
    }
}

/// Moves the panel that starts on `frame` of a shot so that it starts on the frame `to`:
/// the panel before it lasts longer or shorter by as much. It stays between its neighbours.
pub(super) fn move_to(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        frame: u32,
        to: u32,
    }
    let id = "panel.move";
    let Params { shot, frame, to } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_mut(p, shot_id(p, shot)?)?;
        let panel = later_panel(shot, frame, "moved")?;
        checked_frame(shot, to)?;
        let before = shot.panels().into_iter().rfind(|other| other.frame < frame).map_or(0, |other| other.frame);
        let after = shot.panels().into_iter().find(|other| other.frame > frame).map_or(shot.duration, |other| other.frame);
        if to <= before || to >= after {
            return invalid(format!("the panel stays between its neighbours: after frame {before} and before frame {after}"));
        }
        shot.take_panel(frame);
        shot.put_panel(Panel { frame: to, ..panel });
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Removes the panel that starts on `frame` of a shot, with what is written of it. Nothing
/// that is drawn changes: its frames are the panel's before it.
pub(super) fn remove(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        frame: u32,
    }
    let id = "panel.remove";
    let Params { shot, frame } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_mut(p, shot_id(p, shot)?)?;
        later_panel(shot, frame, "removed")?;
        shot.take_panel(frame);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Lists a shot's panels, not an edit: for each its `frame`, the frame its stretch `end`s
/// before, and its `action`, `dialogue` and `notes`.
pub(super) fn list(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
    }
    let Params { shot } = parse("panel.list", params)?;
    let project = s.project().clone();
    let Some(shot) = project.shot(shot_id(&project, shot)?) else { return Ok(Value::Null) };
    let panels = shot.panels().into_iter().map(
        |panel| json!({ "frame": panel.frame, "end": shot.panel_end(panel.frame), "action": panel.action, "dialogue": panel.dialogue, "notes": panel.notes }),
    );
    Ok(json!({ "panels": panels.collect::<Vec<_>>() }))
}

/// Writes what is spoken over a storyboard's panels into their dialogue (plan §4.7): for each
/// panel in the cut, the words of the transcripts of the sounds that lie under it, on tracks
/// that are not muted. A panel that has dialogue already keeps it, unless `overwrite` is set;
/// one that nothing is spoken over is left as it is. One undo step. Returns how many panels
/// were `filled` and how many `kept` what they had.
pub(super) fn fill_dialogue(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: u64,
        #[serde(default)]
        overwrite: bool,
    }
    let id = "storyboard.fillDialogue";
    let Params { storyboard, overwrite } = parse(id, params)?;
    let (mut filled, mut kept) = (0, 0);
    s.edit(title(id), |p| {
        let storyboard = super::storyboard::storyboard_id(p, storyboard)?;
        let spoken = p.spoken_over(storyboard);
        if spoken.is_empty() {
            return invalid("nothing is spoken over the shots: give a sound under them a transcript first (media.setTranscript)");
        }
        for (shot, frame, words) in spoken {
            let Some(mut panel) = p.shot(shot).and_then(|shot| shot.panel_on(frame)) else { continue };
            if panel.dialogue == words {
                continue;
            }
            if !panel.dialogue.is_empty() && !overwrite {
                kept += 1;
                continue;
            }
            panel.dialogue = words;
            shot_mut(p, shot)?.put_panel(panel);
            filled += 1;
        }
        Ok(())
    })?;
    Ok(json!({ "filled": filled, "kept": kept }))
}
