//! Hand-off commands (plan §7.2a): a shot's status, sending shots out as files of their own,
//! and bringing them back to take the place of the shots they came from.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use iskra_model::{HandOff, Project, ShotId, Status};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{invalid, parse, shot_id, title};
use crate::{Result, Session};

/// Whom a shot is with, as typed: nobody if nothing was.
fn whom(to: &str) -> Option<String> {
    Some(to.trim().to_string()).filter(|to| !to.is_empty())
}

/// A shot's name as a file's: what is awkward in a path becomes `_`.
fn file_name(name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || " -_.".contains(c) { c } else { '_' }).collect();
    let safe = safe.trim_matches([' ', '.']).to_string();
    if safe.is_empty() { "shot".to_string() } else { safe }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || a.canonicalize().is_ok_and(|a| b.canonicalize().is_ok_and(|b| a == b))
}

/// Sets a shot's status: `board`, `out`, `animation`, `post` or `final`. A shot that is `out`
/// can say whom it is with, in `to`.
pub(super) fn set_status(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        status: Status,
        #[serde(default)]
        to: String,
    }
    let id = "shot.setStatus";
    let Params { shot, status, to } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        if let Some(shot) = p.shot_mut(shot) {
            (shot.status, shot.out_to) = (status, whom(&to).filter(|_| status == Status::Out));
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sends shots out: writes each as `<its name>.iskra` in `folder`, a project file of that one
/// shot with its drawings, the project's settings and palette, and the part that is in the
/// cut as its clip; then marks the shots as out, with `to` if someone is named. A file that
/// is there already is only replaced with `replace`. Marking them is one undo step; the files
/// stay when it is undone. Returns the `files`.
pub(super) fn send_out(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shots: Vec<u64>,
        folder: PathBuf,
        #[serde(default)]
        to: String,
        #[serde(default)]
        replace: bool,
    }
    let id = "shot.sendOut";
    let Params { shots, folder, to, replace } = parse(id, params)?;
    if shots.is_empty() {
        return invalid("name the shots to send out");
    }
    if !folder.is_dir() {
        return invalid(format!("{} is not a folder", folder.display()));
    }
    // Every file is known to be writable before the first is written.
    let mut files: Vec<(ShotId, PathBuf)> = Vec::new();
    for shot in &shots {
        let shot = shot_id(s.project(), Some(*shot))?;
        let name = s.project().shot(shot).map(|shot| file_name(&shot.name)).unwrap_or_default();
        let path = folder.join(format!("{name}.iskra"));
        if s.path().is_some_and(|own| same_file(own, &path)) {
            return invalid(format!("{} is the project's own file", path.display()));
        }
        if files.iter().any(|(other, taken)| *other == shot || *taken == path) {
            return invalid(format!("`{name}` is named twice"));
        }
        if path.exists() && !replace {
            return invalid(format!("the folder already has a file named `{name}.iskra`"));
        }
        files.push((shot, path));
    }
    let project_name = s.path().and_then(|path| path.file_stem()).map_or("Untitled".to_string(), |name| name.to_string_lossy().into_owned());
    let sent = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    for (shot, path) in &files {
        // The file takes the drawings, which are read for it and let go again.
        let was_loaded = s.project().shot(*shot).is_some_and(|shot| shot.loaded);
        s.load_shot(*shot)?;
        let project = s.project().clone();
        let Some((mut file, name)) = project.shot_file(*shot).zip(project.shot(*shot).map(|shot| shot.name.clone())) else { continue };
        // The shot's sound goes with it by name: its files are named whole, since the shot's
        // file lies elsewhere than this project's folder.
        let folder = s.path().and_then(|own| own.parent()).map(std::path::Path::to_path_buf);
        for media in &mut file.media {
            let whole = media.file(folder.as_deref());
            Arc::make_mut(media).path = whole;
        }
        let from = HandOff { project: project_name.clone(), shot: shot.0, name, sent, to: to.trim().to_string() };
        let written = iskra_store::write_shot_file(path, &Arc::new(file), &from);
        if !was_loaded {
            s.unload_shot(*shot);
        }
        written?;
    }
    s.edit(title(id), |p| {
        for (shot, _) in &files {
            if let Some(shot) = p.shot_mut(*shot) {
                (shot.status, shot.out_to) = (Status::Out, whom(&to));
            }
        }
        Ok(())
    })?;
    Ok(json!({ "files": files.iter().map(|(_, path)| path).collect::<Vec<_>>() }))
}

/// The shot of this project that a returned file is of: the one named, or the one the file
/// says it came from, or the one with the name its shot has.
fn target(project: &Project, named: Option<u64>, from: Option<&HandOff>, returned_name: &str) -> Result<ShotId> {
    if named.is_some() {
        return shot_id(project, named);
    }
    let by_id = from.and_then(|from| project.shot(ShotId(from.shot))).map(|shot| shot.id);
    let by_name = |name: &str| project.shots.iter().find(|shot| shot.name == name).map(|shot| shot.id);
    match by_id.or_else(|| by_name(returned_name)).or_else(|| from.and_then(|from| by_name(&from.name))) {
        Some(shot) => Ok(shot),
        None => invalid(format!("no shot of this project is `{returned_name}`; say which shot the file is to replace")),
    }
}

/// Brings a shot back from the file at `path`: it takes the place of the shot it came from,
/// or of `shot` if one is named. The shot keeps its id, its name and its clip, and is now in
/// animation; the project's settings stay, and its palette gets the swatches it lacks.
/// `differences` says, a line each, what the returned shot changes and what of it doesn't
/// match the project. With `preview` nothing is changed: the differences are all that is
/// returned. Bringing a shot back is one undo step, which brings the earlier version back.
pub(super) fn bring_back(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        path: PathBuf,
        shot: Option<u64>,
        #[serde(default)]
        preview: bool,
    }
    let id = "shot.bringBack";
    let Params { path, shot, preview } = parse(id, params)?;
    if s.path().is_some_and(|own| same_file(own, &path)) {
        return invalid(format!("{} is the project's own file", path.display()));
    }
    let (from, returned, report) = iskra_store::read_shot_file(&path)?;
    // The file's shot: the one it says it is of, or its only one.
    let said = from.as_ref().and_then(|from| returned.shot(ShotId(from.shot))).map(|shot| shot.id);
    let only = match &returned.shots[..] {
        [only] => Some(only.id),
        _ => None,
    };
    let Some(theirs) = said.or(only) else {
        return invalid(format!("{} holds {} shots, and doesn't say which is to come back", path.display(), returned.shots.len()));
    };
    let mut returned = Project::clone(&returned);
    returned.shots.retain(|shot| shot.id == theirs);
    let returned_name = returned.shots[0].name.clone();
    let shot = target(s.project(), shot, from.as_ref(), &returned_name)?;
    // Both versions with their pixels: to say what changed, and for undo to have the old one.
    let was_loaded = s.project().shot(shot).is_some_and(|shot| shot.loaded);
    s.load_shot(shot)?;
    let differences = s.project().differences(shot, &returned).unwrap_or_default();
    let name = s.project().shot(shot).map(|shot| shot.name.clone()).unwrap_or_default();
    if preview && !was_loaded {
        // A look leaves the shot's drawings where they were: in the file.
        s.unload_shot(shot);
    }
    if !preview {
        s.edit(title(id), |p| match p.bring_back(shot, &returned) {
            Some(()) => Ok(()),
            None => invalid(format!("no shot {}", shot.0)),
        })?;
    }
    let to = from.as_ref().map(|from| from.to.as_str()).unwrap_or_default();
    Ok(json!({ "shot": shot, "name": name, "to": to, "differences": differences, "skipped": report.skipped }))
}
