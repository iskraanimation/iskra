//! Group commands (plan §4.3): some of a shot's nodes handled as one, folded into one box and
//! one row, saved to the library as a rig, or marked as the storyboard's.

use std::collections::BTreeSet;
use std::sync::Arc;

use iskra_model::{Composite, CompositeId, Effect, EffectId, Group, GroupId, PegId, Project, Shot, ShotId, Source, unique_name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{checked_name, invalid, name_is_free, parse, shot_id, title};
use crate::{Result, Session};

/// The shot that holds the group a command names.
fn shot_of(project: &Project, group: u64) -> Result<&Shot> {
    match project.shots.iter().find(|shot| shot.group(GroupId(group)).is_some()) {
        Some(shot) => Ok(shot),
        None => invalid(format!("no group {group}")),
    }
}

/// The group of the node `by`, as the id that `put_into` takes: a node a command puts by
/// another, on its wire or above it, goes where that one is, unless it is told otherwise.
pub(super) fn group_by(shot: &Shot, by: u64) -> Option<u64> {
    shot.group_of(by).map(|group| group.id.0)
}

/// Puts a node that a command has just made where the node view asked for it: at `at` in the
/// view, when a place is given, and into `group` of its shot, when one is, which a node made
/// over an open group asks for.
pub(super) fn put_into(p: &mut Project, shot: ShotId, group: Option<u64>, at: Option<[f64; 2]>, node: u64) -> Result<()> {
    if let Some(at) = at {
        if at.iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
            return invalid("a place in the node view is within ±1000000");
        }
        if let Some(shot) = p.shot_mut(shot) {
            shot.places.insert(node, at);
        }
    }
    let Some(group) = group else { return Ok(()) };
    match p.shot_mut(shot).and_then(|shot| shot.group_mut(GroupId(group))) {
        Some(found) => {
            found.members.insert(node);
            Ok(())
        }
        None => invalid(format!("no group {group}")),
    }
}

/// The nodes a command names, which all have to be nodes of `shot` that can be in a group: its
/// layers, pegs, composites, deformers and groups.
fn checked_nodes(shot: &Shot, nodes: &[u64]) -> Result<BTreeSet<u64>> {
    if nodes.is_empty() {
        return invalid("give the `nodes`, by their ids");
    }
    match nodes.iter().find(|node| **node == shot.camera.id.0 || !shot.node_ids().any(|id| id == **node)) {
        Some(node) => invalid(format!("the shot `{}` has no node {node} that can be in a group", shot.name)),
        None => Ok(nodes.iter().copied().collect()),
    }
}

/// Takes nodes out of whatever groups of the shot they are in.
fn take_out(shot: &mut Shot, nodes: &BTreeSet<u64>) {
    for group in &mut shot.groups {
        if group.members.iter().any(|member| nodes.contains(member)) {
            Arc::make_mut(group).members.retain(|member| !nodes.contains(member));
        }
    }
}

/// A node's id, as the picture it makes.
pub(super) fn key(source: &Source) -> u64 {
    match source {
        Source::Layer(id) | Source::Art(id, _) => id.0,
        Source::Composite(id) => id.0,
        Source::Effect(id) => id.0,
    }
}

/// The pictures of the nodes `members` that are wired into something that is not one of
/// them, a composite, an effect or the display: in the order they are stacked where they go.
fn leaving(shot: &Shot, members: &BTreeSet<u64>) -> Vec<Source> {
    let stacked = shot.composites.iter().filter(|composite| !members.contains(&composite.id.0)).flat_map(|composite| composite.inputs.iter());
    let through = shot.effects.iter().filter(|effect| !members.contains(&effect.id.0)).flat_map(|effect| effect.inputs.values());
    let mut leaving: Vec<Source> = Vec::new();
    let shown = shot.display.iter().chain(shot.displays.iter().filter_map(|display| display.shows.as_ref()));
    for source in stacked.chain(through).chain(shown).filter(|source| members.contains(&key(source))) {
        if !leaving.contains(source) {
            leaving.push(*source);
        }
    }
    leaving
}

/// The pegs, and what hands a peg on, that are not among `members` and carry nodes that are:
/// each once, in the order of the shot's nodes.
fn carriers(shot: &Shot, members: &BTreeSet<u64>) -> Vec<PegId> {
    let pegs = shot.pegs.iter().map(|peg| (peg.id.0, peg.parent));
    let carried = pegs.chain(shot.layers.iter().map(|layer| (layer.id.0, layer.parent))).chain(shot.effects.iter().map(|effect| (effect.id.0, effect.parent)));
    let mut carriers: Vec<PegId> = Vec::new();
    for (_, parent) in carried.filter(|(node, parent)| members.contains(node) && parent.is_some_and(|parent| !members.contains(&parent.0))) {
        carriers.extend(parent.filter(|parent| !carriers.contains(parent)));
    }
    carriers
}

/// Gives the nodes `members` their ports: for each of `ins`, a peg outside them with the id
/// of its port, a Port In that the peg is wired into and that carries what the peg carried of
/// them; for each of `outs`, a picture of theirs with the id of its port, a Port Out that the
/// picture goes into and that is taken, outside, where the picture was. Returns the ports.
fn port(shot: &mut Shot, members: &BTreeSet<u64>, ins: &[(PegId, u64)], outs: &[(Source, u64)]) -> Vec<u64> {
    let mut names: Vec<String> = shot.node_names().map(str::to_string).collect();
    let mut named = |kind: &str| {
        let kind = iskra_model::effect_kind(kind)?;
        let name = unique_name(kind.title, names.iter().map(String::as_str));
        names.push(name.clone());
        Some((kind, name))
    };
    let mut made = Vec::new();
    for (peg, id) in ins {
        let Some((kind, name)) = named(iskra_model::PORT_IN) else { continue };
        let own = PegId(*id);
        for carried in shot.pegs.iter_mut().filter(|carried| members.contains(&carried.id.0) && carried.parent == Some(*peg)) {
            Arc::make_mut(carried).parent = Some(own);
        }
        for carried in shot.layers.iter_mut().filter(|carried| members.contains(&carried.id.0) && carried.parent == Some(*peg)) {
            Arc::make_mut(carried).parent = Some(own);
        }
        for carried in shot.effects.iter_mut().filter(|carried| members.contains(&carried.id.0) && carried.parent == Some(*peg)) {
            Arc::make_mut(carried).parent = Some(own);
        }
        let mut port = Effect::new(EffectId(*id), &name, kind);
        port.parent = Some(*peg);
        shot.effects.push(Arc::new(port));
        made.push(*id);
    }
    for (source, id) in outs {
        let Some((kind, name)) = named(iskra_model::PORT_OUT) else { continue };
        let out = Source::Effect(EffectId(*id));
        for composite in shot.composites.iter_mut().filter(|composite| !members.contains(&composite.id.0) && composite.inputs.contains(source)) {
            Arc::make_mut(composite).inputs.iter_mut().filter(|input| *input == source).for_each(|input| *input = out);
        }
        for effect in shot.effects.iter_mut().filter(|effect| !members.contains(&effect.id.0) && effect.inputs.values().any(|input| input == source)) {
            Arc::make_mut(effect).inputs.values_mut().filter(|input| *input == source).for_each(|input| *input = out);
        }
        if shot.display == Some(*source) {
            shot.display = Some(out);
        }
        for display in shot.displays.iter_mut().filter(|display| display.shows == Some(*source)) {
            display.shows = Some(out);
        }
        let mut port = Effect::new(EffectId(*id), &name, kind);
        port.inputs.insert(iskra_model::PICTURE.to_string(), *source);
        shot.effects.push(Arc::new(port));
        made.push(*id);
    }
    made
}

/// Gives the nodes `members` of a shot a composite of their own, `own`, for the pictures that
/// leave them: every picture of theirs that is wired into something that is not one of them
/// is wired into the new composite, and the composite takes its place there. Whether any was:
/// if none, no composite is made.
fn gather(shot: &mut Shot, members: &BTreeSet<u64>, own: CompositeId) -> bool {
    let inside = |source: &Source| members.contains(&key(source));
    // What leaves, in the order it is stacked where it goes: the composites first.
    let leaving = leaving(shot, members);
    if leaving.is_empty() {
        return false;
    }
    let gathered = Source::Composite(own);
    for composite in shot.composites.iter_mut().filter(|composite| !members.contains(&composite.id.0) && composite.inputs.iter().any(inside)) {
        // The first of them there is where the new composite goes; the others only leave.
        let composite = Arc::make_mut(composite);
        if let Some(first) = composite.inputs.iter_mut().find(|input| inside(input)) {
            *first = gathered;
        }
        composite.inputs.retain(|input| !inside(input));
    }
    for effect in shot.effects.iter_mut().filter(|effect| !members.contains(&effect.id.0) && effect.inputs.values().any(inside)) {
        Arc::make_mut(effect).inputs.values_mut().filter(|input| inside(input)).for_each(|input| *input = gathered);
    }
    if shot.display.as_ref().is_some_and(inside) {
        shot.display = Some(gathered);
    }
    for display in shot.displays.iter_mut().filter(|display| display.shows.as_ref().is_some_and(inside)) {
        display.shows = Some(gathered);
    }
    let name = unique_name("Composite", shot.node_names());
    shot.composites.push(Arc::new(Composite::new(own, &name, leaving)));
    true
}

/// Makes a group of some of a shot's nodes, given by their ids in `nodes`: layers, pegs,
/// composites, deformers and other groups. A node that was in another group leaves it, unless
/// a group it is in, however deep, is named too: then it stays where it is, in that group, and
/// the group goes into the new one whole. A group all of whose nodes are named is taken as
/// named too. If all of them were in one group, the new group is
/// in that one. A group changes nothing of the
/// picture. Without a name it is "Group", with a number if that is taken. Returns its `id`.
///
/// With `composite`, the group gets a composite of its own for what leaves it: the pictures
/// of its nodes that were wired into something outside it, a composite, an effect or the
/// display, are wired into the new composite, in the order they were stacked, and the new
/// composite is wired in wherever they were. So one wire leaves the group for each place its
/// pictures went, and the picture is as it was. Nothing of it left the group, there is no
/// composite; where there is one, its id is returned as `composite`.
///
/// With `ports`, the group gets its ways in and out (plan §8.6) by itself: a Port In for each
/// peg outside it that carries nodes of it, wired from that peg and carrying those nodes in
/// its place; and, where it got no composite, a Port Out on each picture that leaves it, which
/// what took the picture outside then takes from the port. Their ids are returned as `ports`.
/// Ungrouping takes them away again, and all is wired as it was. With `compositeAt`, the
/// group's composite is put at that place in the node view.
pub(super) fn new(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        shot: Option<u64>,
        name: Option<String>,
        nodes: Vec<u64>,
        #[serde(default)]
        composite: bool,
        #[serde(default)]
        ports: bool,
        composite_at: Option<[f64; 2]>,
    }
    let id = "group.new";
    let Params { shot, name, nodes, composite, ports, composite_at } = parse(id, params)?;
    if composite_at.is_some_and(|at| at.iter().any(|v| !v.is_finite() || v.abs() > 1e6)) {
        return invalid("a place in the node view is within ±1000000");
    }
    let name = name.as_deref().map(checked_name).transpose()?;
    let (mut made, mut gathered, mut ported) = (0, None, Vec::new());
    s.edit(title(id), |p| {
        // The shot that is named, or else the one the first of the nodes is in.
        let holds = |shot: &&Arc<Shot>| nodes.first().is_some_and(|node| shot.node_ids().any(|id| id == *node));
        let within: ShotId = match shot {
            Some(_) => shot_id(p, shot)?,
            None => p.shots.iter().find(holds).map_or_else(|| shot_id(p, None), |shot| Ok(shot.id))?,
        };
        let Some(from) = p.shot(within) else { return invalid("no such shot") };
        let mut picked = checked_nodes(from, &nodes)?;
        // A group all of whose nodes are named is named too, as a box pulled over it names
        // its nodes and not it: it goes in whole and is not emptied.
        let nodes: BTreeSet<u64> = from.node_ids().collect();
        let held = |group: GroupId| {
            from.group_nodes(group).into_iter().filter(|node| nodes.contains(node) && from.group(GroupId(*node)).is_none() && !from.is_port(*node))
        };
        let whole: Vec<u64> = from
            .groups
            .iter()
            .filter(|group| held(group.id).next().is_some() && held(group.id).all(|node| picked.contains(&node)))
            .map(|group| group.id.0)
            .collect();
        picked.extend(whole);
        // A node named with a group it is in stays in that group, which a box pulled over an
        // open group names with all in it: only the outermost go into the new group.
        let members: BTreeSet<u64> = picked.iter().copied().filter(|node| !from.groups_above(*node).any(|group| picked.contains(&group.id.0))).collect();
        // All that will be in it, however deep: what its ports and its composite reckon with.
        let whole: BTreeSet<u64> = members.iter().flat_map(|node| from.group_nodes(GroupId(*node)).into_iter().chain([*node])).collect();
        let name = match name {
            Some(name) => {
                name_is_free(from, name, None)?;
                name.to_string()
            }
            None => unique_name("Group", from.node_names()),
        };
        // The group they were all in, if they were in one.
        let mut above = members.iter().map(|node| from.group_of(*node).map(|group| group.id));
        let first = above.next().flatten();
        let shared = first.filter(|_| above.all(|other| other == first));
        // The ports it is to get: the pegs that come in, and the pictures that go out, each
        // with the id its port will have.
        let (carriers, leaving) = if ports { (carriers(from, &whole), leaving(from, &whole)) } else { Default::default() };
        let leaving = if composite { Vec::new() } else { leaving };
        let fresh = GroupId(p.new_id());
        made = fresh.0;
        let own = CompositeId(p.new_id());
        let ins: Vec<(PegId, u64)> = carriers.into_iter().map(|peg| (peg, p.new_id())).collect();
        let outs: Vec<(Source, u64)> = leaving.into_iter().map(|source| (source, p.new_id())).collect();
        let Some(shot) = p.shot_mut(within) else { return Ok(()) };
        take_out(shot, &members);
        let (mut members, mut whole) = (members, whole);
        if composite && gather(shot, &whole, own) {
            members.insert(own.0);
            whole.insert(own.0);
            gathered = Some(own.0);
            if let Some(at) = composite_at {
                shot.places.insert(own.0, at);
            }
        }
        ported = port(shot, &whole, &ins, &outs);
        members.extend(ported.iter().copied());
        shot.groups.push(Arc::new(Group { id: fresh, name, members, folded: false, rows_folded: false, board: false, template: None }));
        if let Some(above) = shared.and_then(|id| shot.group_mut(id)) {
            above.members.insert(fresh.0);
        }
        Ok(())
    })?;
    Ok(json!({ "id": made, "composite": gathered, "ports": ported }))
}

pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        group: u64,
        name: String,
    }
    let id = "group.rename";
    let Params { group, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, group)?;
        name_is_free(shot, name, Some(group))?;
        let shot = shot.id;
        if let Some(group) = p.shot_mut(shot).and_then(|shot| shot.group_mut(GroupId(group))) {
            group.name = name.to_string();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Takes a group away. What was in it stays in the shot, in the group this one was in if it
/// was in one. Its ports go with it: each comes off its wire, which is joined again, so that
/// all is wired as it would be without them.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        group: u64,
    }
    let id = "group.delete";
    let Params { group } = parse(id, params)?;
    s.edit(title(id), |p| {
        let from = shot_of(p, group)?;
        let (within, above) = (from.id, from.group_of(group).map(|above| above.id));
        let Some(shot) = p.shot_mut(within) else { return Ok(()) };
        shot.remove_ports(GroupId(group));
        let members = shot.group(GroupId(group)).map(|group| group.members.clone()).unwrap_or_default();
        shot.groups.retain(|other| other.id != GroupId(group));
        shot.places.remove(&group);
        if let Some(above) = above.and_then(|id| shot.group_mut(id)) {
            above.members.remove(&group);
            above.members.extend(members);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Puts nodes of a group's shot into the group, out of whatever groups they were in. A group
/// can't be put into itself, nor into a group that is in it.
pub(super) fn add(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        group: u64,
        nodes: Vec<u64>,
    }
    let id = "group.add";
    let Params { group, nodes } = parse(id, params)?;
    s.edit(title(id), |p| {
        let from = shot_of(p, group)?;
        let members = checked_nodes(from, &nodes)?;
        let around = |node: &u64| *node == group || from.groups_above(group).any(|above| above.id.0 == *node);
        if let Some(node) = members.iter().find(|node| around(node)) {
            return invalid(format!("group {node} can't be put into itself, nor into a group that is in it"));
        }
        let within = from.id;
        let Some(shot) = p.shot_mut(within) else { return Ok(()) };
        take_out(shot, &members);
        if let Some(group) = shot.group_mut(GroupId(group)) {
            group.members.extend(members);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Takes nodes out of the groups they are in. They stay in the shot, in no group.
pub(super) fn remove(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        nodes: Vec<u64>,
    }
    let id = "group.remove";
    let Params { nodes } = parse(id, params)?;
    s.edit(title(id), |p| {
        let holds = |shot: &&Arc<Shot>| nodes.first().is_some_and(|node| shot.node_ids().any(|id| id == *node));
        let Some(from) = p.shots.iter().find(holds) else { return invalid("give the `nodes`, by their ids") };
        let (within, members) = (from.id, checked_nodes(from, &nodes)?);
        if let Some(shot) = p.shot_mut(within) {
            take_out(shot, &members);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets what a group is shown as and what it is for. `folded` shows it as one box in the node
/// view, and `rowsFolded` as one row on the timeline, with what is in it put away; each is
/// apart from the other. `board` marks it as the storyboard's: all that is in it is boards,
/// whatever a layer says of itself.
pub(super) fn set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        group: u64,
        folded: Option<bool>,
        rows_folded: Option<bool>,
        board: Option<bool>,
    }
    let id = "group.set";
    let Params { group, folded, rows_folded, board } = parse(id, params)?;
    if folded.is_none() && rows_folded.is_none() && board.is_none() {
        return invalid("say what to set: `folded`, `rowsFolded` or `board`");
    }
    s.edit(title(id), |p| {
        let within = shot_of(p, group)?.id;
        if let Some(group) = p.shot_mut(within).and_then(|shot| shot.group_mut(GroupId(group))) {
            group.folded = folded.unwrap_or(group.folded);
            group.rows_folded = rows_folded.unwrap_or(group.rows_folded);
            group.board = board.unwrap_or(group.board);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
