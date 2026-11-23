//! Display commands (plan §8.6): what a shot's displays show, the displays beside the main
//! one, and which is the main one. A Pass (plan §7.14) is made and changed by these too: it is
//! kept among the displays, marked as a pass.

use iskra_model::{Display, DisplayId, Project, Source};
use serde::Deserialize;
use serde_json::{Value, json};

use super::graph::source_in;
use super::{checked_name, invalid, name_is_free, parse, shot_id, title};
use crate::{Result, Session};

/// The shot a display is of, and the display: the one named, or the shot's main one.
fn display_in(project: &Project, shot: Option<u64>, display: Option<u64>) -> Result<(iskra_model::ShotId, DisplayId)> {
    match display {
        Some(display) => match project.shots.iter().find(|shot| shot.has_display(DisplayId(display))) {
            Some(shot) => Ok((shot.id, DisplayId(display))),
            None => invalid(format!("no display {display}")),
        },
        None => {
            let shot = shot_id(project, shot)?;
            Ok((shot, DisplayId(shot.0)))
        }
    }
}

/// A display's name has to be free among the shot's nodes and its displays.
fn display_name_is_free(shot: &iskra_model::Shot, name: &str, own: DisplayId) -> Result<()> {
    name_is_free(shot, name, Some(own.0))?;
    if own != shot.main_display() && shot.display_name == name {
        return invalid(format!("the shot's main display is named `{name}`"));
    }
    Ok(())
}

/// Sets what a display shows: a node (`{ "layer": id }`, `{ "composite": id }` or
/// `{ "effect": id }`), or nothing (`node` null or left out). `display` is the display, by its
/// id; without it, the main display of `shot`.
pub(super) fn set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        display: Option<u64>,
        node: Option<Source>,
    }
    let id = "display.set";
    let Params { shot, display, node } = parse(id, params)?;
    s.edit(title(id), |p| {
        let (shot, display) = display_in(p, shot, display)?;
        let node = match (node, p.shot(shot)) {
            (Some(node), Some(shot)) => Some(source_in(shot, node)?),
            _ => None,
        };
        if let Some(shot) = p.shot_mut(shot) {
            shot.show_on(display, node);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Adds a display to a shot beside its main one, showing `node` or nothing; with `pass`, a
/// Pass, which the Export page renders apart under its name. Without a name it gets a free
/// "Display" one, or "Pass". With `at`, it is put there in the node view. Returns its `id`.
pub(super) fn new(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        name: Option<String>,
        node: Option<Source>,
        at: Option<[f64; 2]>,
        #[serde(default)]
        pass: bool,
    }
    let id = "display.new";
    let Params { shot, name, node, at, pass } = parse(id, params)?;
    let display = s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let display = DisplayId(p.new_id());
        let Some(within) = p.shot(shot) else { return invalid("no such shot") };
        let name = match &name {
            Some(name) => {
                let name = checked_name(name)?;
                display_name_is_free(within, name, display)?;
                name.to_string()
            }
            None => within.free_display_name(if pass { "Pass" } else { "Display" }),
        };
        let shows = node.map(|node| source_in(within, node)).transpose()?;
        // It goes into the group of the node it shows.
        let group = node.and_then(|node| super::group::group_by(within, super::group::key(&node)));
        if let Some(shot) = p.shot_mut(shot) {
            shot.displays.push(Display { id: display, name, shows, pass });
            if let Some(group) = group.and_then(|group| shot.group_mut(iskra_model::GroupId(group))) {
                group.members.insert(display.0);
            }
            if let Some(at) = at {
                shot.places.insert(display.0, at);
            }
        }
        Ok(display)
    })?;
    Ok(json!({ "id": display.0 }))
}

/// Makes a display the shot's main one: what the Export page renders, the Storyboard page
/// shows and the playback cache keeps. The main one before it becomes one of the others.
pub(super) fn set_main(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        display: u64,
    }
    let id = "display.setMain";
    let Params { display } = parse(id, params)?;
    s.edit(title(id), |p| {
        let (shot, display) = display_in(p, None, Some(display))?;
        if p.shot(shot).is_some_and(|shot| shot.passes().any(|pass| pass.id == display)) {
            return invalid("a pass can't be the main display");
        }
        if let Some(shot) = p.shot_mut(shot) {
            shot.make_main(display);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Renames a display, the main one or another.
pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        display: u64,
        name: String,
    }
    let id = "display.rename";
    let Params { display, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        let (shot, display) = display_in(p, None, Some(display))?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        display_name_is_free(shot, name, display)?;
        match shot.displays.iter_mut().find(|own| own.id == display) {
            Some(own) => own.name = name.to_string(),
            None => shot.display_name = name.to_string(),
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Renames a pass in every shot that has one of that name, `from`, to `to`, as one step: the
/// passes are matched across shots by their names (plan §7.14), so they are renamed together.
/// Refused, with the shot named, where a shot has another node named `to`. Returns how many
/// were renamed.
pub(super) fn rename_pass(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        from: String,
        to: String,
    }
    let id = "pass.rename";
    let Params { from, to } = parse(id, params)?;
    let to = checked_name(&to)?.to_string();
    let count = s.edit(title(id), |p| {
        let having: Vec<(iskra_model::ShotId, DisplayId)> = p.shots.iter().filter_map(|shot| Some((shot.id, shot.pass_named(&from)?.id))).collect();
        if having.is_empty() {
            return invalid(format!("no shot has a pass named `{from}`"));
        }
        for (shot, pass) in &having {
            let Some(shot) = p.shot(*shot) else { continue };
            if let Err(why) = display_name_is_free(shot, &to, *pass) {
                return invalid(format!("{}: {why}", shot.name));
            }
        }
        for (shot, pass) in &having {
            if let Some(own) = p.shot_mut(*shot).and_then(|shot| shot.displays.iter_mut().find(|own| own.id == *pass)) {
                own.name.clone_from(&to);
            }
        }
        Ok(having.len())
    })?;
    Ok(json!({ "renamed": count }))
}

/// Deletes a display that is not the main one. What it showed stays as it is.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        display: u64,
    }
    let id = "display.delete";
    let Params { display } = parse(id, params)?;
    s.edit(title(id), |p| {
        let (shot, display) = display_in(p, None, Some(display))?;
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        if display == shot.main_display() {
            return invalid("the main display can't be deleted: make another one the main one first");
        }
        shot.displays.retain(|own| own.id != display);
        shot.places.remove(&display.0);
        shot.colours.remove(&display.0);
        Ok(())
    })?;
    Ok(Value::Null)
}
