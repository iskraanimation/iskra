//! Library commands (plan §4.12). The library is folders of template files on disk, outside any
//! project: these commands list a folder, save some of a shot's nodes into it as a template,
//! and put a template into a shot as copies. Only the last is an undo step; the others act on
//! files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use iskra_model::{ApplyOptions, GroupId, LayerId, Node, Part, PegId, Project, ShotId, TemplateInfo};
use iskra_render::View;
use iskra_store::{TEMPLATE_EXTENSION, Template};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{checked_name, invalid, loaded, parse, shot_id, title};
use crate::{Result, Session};

/// What went wrong with a file or folder, with its path.
fn failed<T>(path: &Path, why: std::io::Error) -> Result<T> {
    invalid(format!("{}: {why}", path.display()))
}

/// A name for a template or a folder as typed, which is to be a file's name on any system.
fn file_name(name: &str) -> Result<&str> {
    let name = checked_name(name)?;
    if name.starts_with('.') || name.contains(['\\', ':', '*', '?', '"', '<', '>', '|']) {
        return invalid(format!("the name `{name}` can't be a file's name"));
    }
    Ok(name)
}

fn is_template(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == TEMPLATE_EXTENSION)
}

/// A template's name: its file's, without the extension.
fn name_of(path: &Path) -> String {
    path.file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The template file a command names, which has to be there.
fn template_file(path: &Path) -> Result<()> {
    if !is_template(path) || !path.is_file() {
        return invalid(format!("{} is not a template file (.{TEMPLATE_EXTENSION})", path.display()));
    }
    Ok(())
}

/// Lists a folder of the library: its folders, and its templates with what each says about
/// itself. A template that can't be read is listed with the `problem`. Both are in the order
/// of their names.
pub(super) fn list(_: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        folder: PathBuf,
    }
    let Params { folder } = parse("library.list", params)?;
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(why) => return failed(&folder, why),
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort_by_key(|path| name_of(path).to_lowercase());
    let (mut folders, mut items) = (Vec::new(), Vec::new());
    for path in paths {
        let name = name_of(&path);
        if path.is_dir() && !name.starts_with('.') {
            folders.push(json!({ "name": name, "path": path }));
        } else if is_template(&path) && path.is_file() {
            items.push(match iskra_store::read_template_info(&path) {
                Ok(info) => json!({ "name": name, "path": path, "source": info.source, "saved": info.saved, "tags": info.tags, "notes": info.notes }),
                Err(why) => json!({ "name": name, "path": path, "problem": why.to_string() }),
            });
        }
    }
    Ok(json!({ "folders": folders, "items": items }))
}

/// Makes a folder in a folder of the library.
pub(super) fn new_folder(_: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        folder: PathBuf,
        name: String,
    }
    let Params { folder, name } = parse("library.newFolder", params)?;
    let path = folder.join(file_name(&name)?);
    match std::fs::create_dir(&path) {
        Ok(()) => Ok(json!({ "path": path })),
        Err(why) => failed(&path, why),
    }
}

/// Saves nodes of a shot (each `{ "layer": id }`, `{ "peg": id }` or `{ "composite": id }`)
/// as the template `<name>.iskratpl` in `folder`. A layer goes alone, a peg with all it
/// carries, a composite with all it stacks and the pegs over that. The template holds their
/// drawings and animation over the whole shot, and the swatches the drawings use. A template
/// of that name is only replaced with `replace`. The project is not changed.
pub(super) fn save(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        folder: PathBuf,
        name: String,
        nodes: Vec<Part>,
        shot: Option<u64>,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        notes: String,
        #[serde(default)]
        replace: bool,
    }
    let Params { folder, name, nodes, shot, tags, notes, replace } = parse("library.save", params)?;
    let name = file_name(&name)?;
    let project = s.project().clone();
    let shot = shot_id(&project, shot)?;
    let Some(from) = project.shot(shot) else { return Ok(Value::Null) };
    // The template takes the drawings as they are in memory.
    loaded(from)?;
    if nodes.is_empty() {
        return invalid("a template is of at least one node");
    }
    if nodes.iter().any(|node| !from.has(*node)) {
        return invalid(format!("the shot `{}` has no such node to save", from.name));
    }
    if !folder.is_dir() {
        return invalid(format!("{} is not a folder", folder.display()));
    }
    let path = folder.join(format!("{name}.{TEMPLATE_EXTENSION}"));
    if path.exists() && !replace {
        return invalid(format!("the folder already has a template named `{name}`"));
    }
    let Some(template) = project.template(shot, &nodes, name) else { return invalid("there is nothing to save") };
    let tags = tags.iter().map(|tag| tag.trim().to_string()).filter(|tag| !tag.is_empty()).collect();
    let saved = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let source = s.path().map_or("Untitled".to_string(), name_of);
    let info = TemplateInfo { source, saved, tags, notes: notes.trim().to_string() };
    let preview = preview(&template);
    iskra_store::write_template(&path, &Template { info, project: Arc::new(template), preview })?;
    Ok(json!({ "path": path }))
}

/// The nodes of a shot that `nodes` names, as a template of them in memory.
fn copied(project: &Project, shot: ShotId, nodes: &[Part]) -> Result<Project> {
    let Some(from) = project.shot(shot) else { return invalid("no such shot") };
    // The copies take the drawings as they are in memory.
    loaded(from)?;
    if nodes.is_empty() || nodes.iter().any(|node| !from.has(*node)) {
        return invalid(format!("the shot `{}` has no such nodes to copy", from.name));
    }
    match project.copy_of(shot, nodes) {
        Some(template) => Ok(template),
        None => invalid("there is nothing to copy"),
    }
}

/// Put copies into `shot` as [`Project::place_copies`] does. With `connect` they are
/// connected on top of what the shot shows, and where that is the shot they were copied
/// from, under the pegs the nodes they are copies of are under: a layer that was copied
/// without its peg is carried by that peg still, beside the first.
fn place_copies(p: &mut Project, shot: ShotId, template: &Project, at: Option<[f64; 2]>, connect: bool) -> Option<iskra_model::Placed> {
    let made = p.place_copies(shot, template, at, connect)?;
    if !connect {
        return Some(made);
    }
    let (of, into) = (template.shots.first()?, p.shot_mut(shot)?);
    let above = |parent: Option<PegId>, into: &iskra_model::Shot| parent.filter(|peg| into.peg(*peg).is_some());
    for (original, copy) in of.layers.iter().zip(&made.layers) {
        let parent = above(into.layer(original.id).and_then(|layer| layer.parent), into).filter(|_| original.parent.is_none());
        if let (Some(parent), Some(copy)) = (parent, into.layers.iter_mut().find(|layer| layer.id == *copy)) {
            Arc::make_mut(copy).parent = Some(parent);
        }
    }
    for (original, copy) in of.pegs.iter().zip(&made.pegs) {
        let parent = above(into.peg(original.id).and_then(|peg| peg.parent), into).filter(|_| original.parent.is_none());
        if let (Some(parent), Some(copy)) = (parent, into.pegs.iter_mut().find(|peg| peg.id == *copy)) {
            Arc::make_mut(copy).parent = Some(parent);
        }
    }
    Some(made)
}

fn placed(placed: &iskra_model::Placed) -> Value {
    json!({ "layers": placed.layers, "pegs": placed.pegs, "composites": placed.composites, "groups": placed.groups, "effects": placed.effects, "displays": placed.displays, "drivers": placed.drivers })
}

#[derive(Deserialize)]
struct Nodes {
    nodes: Vec<Part>,
    shot: Option<u64>,
}

/// Copies nodes of a shot (each `{ "layer": id }`, `{ "peg": id }`, `{ "composite": id }` or
/// `{ "group": id }`) for `node.paste`: those nodes alone, with their drawings and animation,
/// as [`Project::copy_of`] takes them: a peg without what it carries, a composite without
/// what it stacks, a group with all in it. The project is not changed. Returns how many
/// nodes were `copied`.
pub(super) fn copy_nodes(s: &mut Session, params: Value) -> Result<Value> {
    let Nodes { nodes, shot } = parse("node.copy", params)?;
    let project = s.project().clone();
    let template = copied(&project, shot_id(&project, shot)?, &nodes)?;
    let count = template.shots.first().map_or(0, |shot| {
        shot.layers.len() + shot.pegs.len() + shot.composites.len().saturating_sub(1) + shot.groups.len() + shot.effects.len() + shot.displays.len()
    });
    s.copied_nodes = Some(Arc::new(template));
    Ok(json!({ "copied": count }))
}

/// Puts what `node.copy` copied into a shot, the one it came from or another, of this
/// project or of one opened since: copies with new ids and drawings of their own, which
/// share nothing with what they were copied from. A name the shot has already gets a number
/// after it. The copies are wired to each other as the copied nodes were, and to nothing
/// else: not to what the shot shows, nor under a peg that was not copied.
/// With `at`, a place in the node view, they are put around it, and into `group` where one
/// is given, else into none; without, a little beside where the copied nodes are, and into
/// the group they were all in, if they were in one of this shot. Returns the ids of the
/// copies.
pub(super) fn paste_nodes(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        at: Option<[f64; 2]>,
        group: Option<u64>,
    }
    let id = "node.paste";
    let Params { shot, at, group } = parse(id, params)?;
    if at.is_some_and(|at| at.iter().any(|v| !v.is_finite() || v.abs() > 1e6)) {
        return invalid("a place in the node view is within ±1000000");
    }
    let Some(template) = s.copied_nodes.clone() else { return invalid("no nodes were copied: there is nothing to paste") };
    let made = s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        p.shot(shot).map_or(Ok(()), loaded)?;
        let group = match at {
            Some(_) => group,
            None => template.shots.first().zip(p.shot(shot)).and_then(|(copied, into)| shared_group(copied, into)),
        };
        if group.is_some_and(|group| p.shot(shot).and_then(|into| into.group(GroupId(group))).is_none()) {
            return invalid(format!("no group {} in the shot", group.unwrap_or_default()));
        }
        let Some(made) = place_copies(p, shot, &template, at, false) else { return invalid("there is nothing to paste") };
        if let (Some(group), Some(into)) = (group, p.shot_mut(shot)) {
            // What is in a group that was pasted stays in it; the rest goes into `group`.
            let held: BTreeSet<u64> = made.groups.iter().filter_map(|id| into.group(*id)).flat_map(|group| group.members.iter().copied()).collect();
            let ids = made.layers.iter().map(|id| id.0).chain(made.pegs.iter().map(|id| id.0)).chain(made.composites.iter().map(|id| id.0));
            let ids = ids.chain(made.groups.iter().map(|id| id.0)).chain(made.effects.iter().map(|id| id.0));
            // And the bones that bend a pasted layer, which come with it.
            let bends: Vec<u64> = made.layers.iter().filter_map(|id| into.layer(*id)?.deformer).map(|id| id.0).collect();
            let ids = ids.chain(bends);
            let loose: Vec<u64> = ids.filter(|id| !held.contains(id)).collect();
            if let Some(found) = into.group_mut(GroupId(group)) {
                found.members.extend(loose);
            }
        }
        Ok(made)
    })?;
    Ok(placed(&made))
}

/// The group of `into` that the nodes of `copied` that are in no group of their own were
/// all in, by their ids, if they were all in one: a copy pasted beside them goes in it too.
fn shared_group(copied: &iskra_model::Shot, into: &iskra_model::Shot) -> Option<u64> {
    let held: BTreeSet<u64> = copied.groups.iter().flat_map(|group| group.members.iter().copied()).collect();
    let main = copied.main_composite();
    let nodes = copied.layers.iter().map(|layer| layer.id.0).chain(copied.pegs.iter().map(|peg| peg.id.0)).chain(copied.groups.iter().map(|group| group.id.0));
    let nodes = nodes.chain(copied.composites.iter().filter(|composite| Some(composite.id) != main).map(|composite| composite.id.0));
    let mut loose = nodes.chain(copied.effects.iter().map(|effect| effect.id.0)).filter(|id| !held.contains(id));
    let first = into.group_of(loose.next()?)?.id;
    loose.all(|node| into.group_of(node).is_some_and(|group| group.id == first)).then_some(first.0)
}

/// Copies nodes of a shot and puts the copies into the same shot, in one step: `node.copy`
/// and `node.paste` together, without changing what was copied last. Returns the ids of the
/// copies.
pub(super) fn duplicate_nodes(s: &mut Session, params: Value) -> Result<Value> {
    let id = "node.duplicate";
    let Nodes { nodes, shot } = parse(id, params)?;
    let made = s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let template = copied(p, shot, &nodes)?;
        match place_copies(p, shot, &template, None, true) {
            Some(made) => Ok(made),
            None => invalid("there is nothing to duplicate"),
        }
    })?;
    Ok(placed(&made))
}

/// Puts the template at `path` into a shot, as copies: its nodes, drawings and animation
/// with new ids, and the swatches the project lacks. A name the shot has already gets a
/// number after it, and each copy notes the template's name. What the template shows is
/// connected on top of what the shot shows. With `at`, a place in the node view, the nodes
/// that were arranged in the template are put around it. Returns the ids of the copies, and
/// what of the file could not be read.
pub(super) fn place(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        path: PathBuf,
        shot: Option<u64>,
        at: Option<[f64; 2]>,
    }
    let id = "library.place";
    let Params { path, shot, at } = parse(id, params)?;
    template_file(&path)?;
    if at.is_some_and(|at| at.iter().any(|v| !v.is_finite() || v.abs() > 1e6)) {
        return invalid("a place in the node view is within ±1000000");
    }
    let (template, report) = iskra_store::read_template(&path)?;
    let name = name_of(&path);
    let placed = s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        // The copies bring drawings, which only a loaded shot holds.
        p.shot(shot).map_or(Ok(()), loaded)?;
        match p.place_template(shot, &template.project, &name, at) {
            Some(placed) => Ok(placed),
            None => invalid(format!("the template `{name}` holds nothing")),
        }
    })?;
    Ok(json!({ "layers": placed.layers, "pegs": placed.pegs, "composites": placed.composites, "groups": placed.groups, "skipped": report.skipped }))
}

/// Applies the template at `path` onto nodes a shot already has, found by their names: where
/// `library.place` makes copies, this gives the shot's own pegs the template's poses and
/// animation, its layers their timing and drawings, and what bends a layer the pose of what
/// bends the template's. Nothing is made but drawings, unless `makeMissing`, and what the
/// template does not name is left as it is.
///
/// `peg` or `layer` is the node it is applied to: a peg stands for the template's top peg,
/// if it has one, and the rest is matched under it by name, parents and all; without either
/// the whole shot is matched. Names are compared without case, separators and the ways a side
/// is written (`leg_L`, `legL`, `LegLeft`); a name that is not where it should be but is the
/// only one of its kind under there is taken too. `map` matches names by hand, the template's
/// to the shot's. `mirror` makes left and right change places. `frame` is where the template's
/// first frame goes; `merge` keeps the keys and timing the shot has under the template's;
/// `replaceDrawings` replaces a drawing the shot has under the same name, where otherwise the
/// shot's is kept; `drawings` chooses for one drawing at a time, by the template's layer as
/// its match's `item` names it and the drawing's name, `true` to replace it. `mirror` mirrors
/// the pose too. `makeMissing` makes what the shot has not got.
///
/// A group of the template's goes onto the shot's of its name, or of the name `map` gives
/// it, and what is in it is looked for in that group alone, where a copy's number is not
/// looked at: `armL` of `Hero` is `armL 2` of `Hero 2`. A composite of the template's stacks
/// what the shot's of its name has of the same in the template's order; the one that stood
/// for the shot it was saved from orders the shot's main composite so.
///
/// With `preview` nothing is changed. Either way it returns `matches`, each node of the
/// template with the shot's node it goes onto and how it was `found` (`matched`, `found
/// elsewhere`, `matched by hand`, `missing in the shot`, `another kind of node`, or with
/// `makeMissing`, `made new`, as a copy of the template's under where its parent went), for a
/// layer the drawings in `conflicts` that the shot's has under the same names drawn otherwise, the shot's
/// nodes that are `untouched`, and how many were `applied` and `skipped`.
pub(super) fn apply(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        path: PathBuf,
        shot: Option<u64>,
        peg: Option<u64>,
        layer: Option<u64>,
        #[serde(default)]
        preview: bool,
        #[serde(default)]
        mirror: bool,
        #[serde(default)]
        map: std::collections::BTreeMap<String, String>,
        frame: Option<u32>,
        #[serde(default)]
        merge: bool,
        #[serde(default)]
        replace_drawings: bool,
        #[serde(default)]
        make_missing: bool,
        #[serde(default)]
        drawings: std::collections::BTreeMap<String, std::collections::BTreeMap<String, bool>>,
        #[serde(default)]
        keep_matches: bool,
    }
    let id = "library.apply";
    let Params { path, shot, peg, layer, preview, mirror, map, frame, merge, replace_drawings, make_missing, drawings, keep_matches } = parse(id, params)?;
    if keep_matches && peg.is_none() {
        return invalid("hand matches are kept with the `peg` a template is applied onto: give it");
    }
    let kept = keep_matches.then(|| map.clone());
    template_file(&path)?;
    if let Some(frame) = frame {
        super::checked_frames(frame, 1)?;
    }
    let onto = match (peg, layer) {
        (Some(_), Some(_)) => return invalid("apply it onto a `peg` or onto a `layer`, not both"),
        (Some(peg), None) => Some(Node::Peg(PegId(peg))),
        (None, Some(layer)) => Some(Node::Layer(LayerId(layer))),
        (None, None) => None,
    };
    let (template, report) = iskra_store::read_template(&path)?;
    let options = ApplyOptions { mirror, map, frame, merge, replace_drawings, make_missing, drawings };
    // The shot the node is in, or the one that is named, or the open one.
    let within = |p: &Project| -> Result<iskra_model::ShotId> {
        let holds = |shot: &&Arc<iskra_model::Shot>| match onto {
            Some(Node::Peg(id)) => shot.peg(id).is_some(),
            Some(Node::Layer(id)) => shot.layer(id).is_some(),
            None => false,
        };
        match (shot, onto) {
            (None, Some(_)) => match p.shots.iter().find(holds) {
                Some(shot) => Ok(shot.id),
                None => invalid("no such node to apply the template onto"),
            },
            _ => {
                let shot = shot_id(p, shot)?;
                if onto.is_some() && !p.shot(shot).is_some_and(|shot| holds(&&Arc::new(shot.clone()))) {
                    return invalid("the shot has no such node to apply the template onto");
                }
                Ok(shot)
            }
        }
    };
    let nothing = format!("the template `{}` holds nothing", name_of(&path));
    let applied = if preview {
        let project = s.project();
        let shot = within(project)?;
        match project.match_template(shot, &template.project, onto, &options) {
            Some(applied) => applied,
            None => return invalid(nothing),
        }
    } else {
        s.edit(title(id), |p| {
            let shot = within(p)?;
            // It brings drawings, which only a loaded shot holds.
            p.shot(shot).map_or(Ok(()), loaded)?;
            let applied = match p.apply_template(shot, &template.project, onto, &options) {
                Some(applied) => applied,
                None => return invalid(nothing.clone()),
            };
            // The hand matches stay with the rig, for the next template applied onto it.
            if let (Some(matches), Some(Node::Peg(top))) = (&kept, onto)
                && let Some(rig) = p.shot_mut(shot).and_then(|shot| shot.pegs.iter_mut().find(|peg| peg.id == top))
            {
                Arc::make_mut(rig).matches.clone_from(matches);
            }
            Ok(applied)
        })?
    };
    let matches: Vec<Value> = applied
        .matches
        .iter()
        .map(|found| json!({ "item": found.item, "rig": found.rig, "kind": found.kind.label(), "found": found.found.label(), "applies": found.found.applies(), "note": found.note, "conflicts": found.conflicts }))
        .collect();
    Ok(json!({ "matches": matches, "untouched": applied.untouched, "applied": applied.applied(), "skipped": applied.skipped(), "unread": report.skipped }))
}

/// Renames a template: its file, in the folder it is in.
pub(super) fn rename(_: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        path: PathBuf,
        name: String,
    }
    let Params { path, name } = parse("library.rename", params)?;
    template_file(&path)?;
    let name = file_name(&name)?;
    let to = path.with_file_name(format!("{name}.{TEMPLATE_EXTENSION}"));
    // Where capitals don't tell file names apart, the new name can be this file's own.
    let same = path.canonicalize().is_ok_and(|from| to.canonicalize().is_ok_and(|to| from == to));
    if to.exists() && !same {
        return invalid(format!("the folder already has a template named `{name}`"));
    }
    match std::fs::rename(&path, &to) {
        Ok(()) => Ok(json!({ "path": to })),
        Err(why) => failed(&path, why),
    }
}

/// Deletes a template: its file. Shots that it was put into keep their copies.
pub(super) fn delete(_: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        path: PathBuf,
    }
    let Params { path } = parse("library.delete", params)?;
    template_file(&path)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Value::Null),
        Err(why) => failed(&path, why),
    }
}

/// The longer side of a template's picture, in pixels.
const PREVIEW_SIDE: u32 = 192;

/// A small picture of a template, as a PNG file's bytes: what its shot shows on the first
/// frame that shows anything, framed to what is painted. Something small is shown larger, but
/// not so large that its pixels are all there is to see. `None` for a template with nothing
/// painted.
fn preview(template: &Project) -> Option<Vec<u8>> {
    let shot = template.shots.first()?;
    let starts: BTreeSet<u32> = shot.layers.iter().flat_map(|layer| layer.exposure.runs()).map(|run| run.start).collect();
    let (frame, (low, high)) = [0].into_iter().chain(starts).find_map(|frame| Some((frame, iskra_render::painted(shot, frame)?)))?;
    let view = View::around(low, high, PREVIEW_SIDE, 4.0)?;
    let picture = iskra_render::render(shot, &template.palette, template.settings.working_space, frame, &view);
    iskra_io::encode_png(picture.width, picture.height, &picture.pixels).ok()
}
