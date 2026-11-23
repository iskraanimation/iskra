//! Graph commands: the composites that stack a shot's layers, what is connected to them, and
//! what the display shows (plan §4.3).

use std::sync::Arc;

use iskra_model::{Composite, CompositeId, Project, Shot, Source, unique_name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{checked_name, invalid, name_is_free, parse, shot_id, title};
use crate::{Result, Session};

/// The shot that holds the composite a command names.
fn shot_of(project: &Project, composite: u64) -> Result<&Shot> {
    match project.shots.iter().find(|s| s.composite(CompositeId(composite)).is_some()) {
        Some(shot) => Ok(shot),
        None => invalid(format!("no composite {composite}")),
    }
}

/// A node a command names as something to connect, which has to be in `shot`.
pub(super) fn source_in(shot: &Shot, source: Source) -> Result<Source> {
    let there = match source {
        Source::Layer(id) | Source::Art(id, _) => shot.layer(id).is_some(),
        Source::Composite(id) => shot.composite(id).is_some(),
        Source::Effect(id) => shot.effect(id).is_some(),
    };
    if !there {
        return invalid(format!("the shot `{}` has no such node to connect", shot.name));
    }
    Ok(source)
}

fn composite_mut(shot: &mut Shot, id: CompositeId) -> Option<&mut Composite> {
    shot.composites.iter_mut().find(|c| c.id == id).map(Arc::make_mut)
}

/// Adds a composite to a shot, connected to nothing. Without a name it gets a free "Composite"
/// one. With `group` it is put into that group of the shot, and with `at` at that place in
/// the node view.
pub(super) fn new_composite(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        name: Option<String>,
        group: Option<u64>,
        at: Option<[f64; 2]>,
    }
    let id = "composite.new";
    let Params { shot, name, group, at } = parse(id, params)?;
    let composite = s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let name = match (&name, p.shot(shot)) {
            (Some(name), Some(shot)) => {
                let name = checked_name(name)?;
                name_is_free(shot, name, None)?;
                name.to_string()
            }
            (_, shot) => unique_name("Composite", shot.into_iter().flat_map(|s| s.node_names())),
        };
        let composite = CompositeId(p.new_id());
        if let Some(shot) = p.shot_mut(shot) {
            shot.composites.push(Arc::new(Composite::new(composite, &name, Vec::new())));
        }
        super::group::put_into(p, shot, group, at, composite.0)?;
        Ok(composite)
    })?;
    Ok(json!({ "id": composite }))
}

pub(super) fn rename_composite(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        composite: u64,
        name: String,
    }
    let id = "composite.rename";
    let Params { composite, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, composite)?;
        name_is_free(shot, name, Some(composite))?;
        let shot = shot.id;
        if let Some(composite) = p.shot_mut(shot).and_then(|shot| composite_mut(shot, CompositeId(composite))) {
            composite.name = name.to_string();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Deletes a composite. What was connected to it stays in the shot, connected to one thing
/// fewer, and what it was connected to loses it.
pub(super) fn delete_composite(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        composite: u64,
    }
    let id = "composite.delete";
    let Params { composite } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, composite)?.id;
        if let Some(shot) = p.shot_mut(shot) {
            shot.disconnect(Source::Composite(CompositeId(composite)));
            shot.composites.retain(|c| c.id != CompositeId(composite));
            shot.places.remove(&composite);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Connects a node (`{ "layer": id }`, `{ "composite": id }` or `{ "effect": id }`) to the composite `to`, at
/// `index` among its inputs, or on top without one (0 is the top). A node that is connected
/// to it already is moved to that place. A composite can't be connected to one that feeds it.
pub(super) fn connect(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        node: Source,
        to: u64,
        index: Option<usize>,
    }
    let id = "node.connect";
    let Params { node, to, index } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, to)?;
        let (node, to) = (source_in(shot, node)?, CompositeId(to));
        if shot.flows_into(Source::Composite(to), node) {
            return invalid("a composite can't be connected to itself or to one that it gets its picture from");
        }
        let shot = shot.id;
        let Some(composite) = p.shot_mut(shot).and_then(|shot| composite_mut(shot, to)) else { return Ok(()) };
        composite.inputs.retain(|input| *input != node);
        let index = index.unwrap_or(0);
        if index > composite.inputs.len() {
            return invalid(format!("`{}` has {} nodes connected", composite.name, composite.inputs.len()));
        }
        composite.inputs.insert(index, node);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Disconnects a node from the composite `from`, or without one from everything it is
/// connected to, the display too.
pub(super) fn disconnect(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        node: Source,
        from: Option<u64>,
    }
    let id = "node.disconnect";
    let Params { node, from } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = match (from, node) {
            (Some(from), _) => shot_of(p, from)?.id,
            (None, Source::Composite(id)) => shot_of(p, id.0)?.id,
            (None, Source::Effect(id)) => match p.shots.iter().find(|shot| shot.effect(id).is_some()) {
                Some(shot) => shot.id,
                None => return invalid(format!("no effect node {}", id.0)),
            },
            (None, Source::Layer(id) | Source::Art(id, _)) => match p.shot_of(id) {
                Some(shot) => shot.id,
                None => return invalid(format!("no layer {}", id.0)),
            },
        };
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        match from.map(CompositeId) {
            Some(from) => composite_mut(shot, from).into_iter().for_each(|composite| composite.inputs.retain(|input| *input != node)),
            None => shot.disconnect(node),
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Puts a node at a place in the node view: a layer, a peg, a composite, a deformer or a
/// group by its id, or a shot's display by the shot's id. A node that was never placed is
/// laid out by the view. Several nodes are put in one step with `moves`, each a `node` and
/// where it goes, `at`.
pub(super) fn place(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Move {
        node: u64,
        at: [f64; 2],
    }
    #[derive(Deserialize)]
    struct Params {
        node: Option<u64>,
        at: Option<[f64; 2]>,
        #[serde(default)]
        moves: Vec<Move>,
    }
    let id = "node.place";
    let Params { node, at, mut moves } = parse(id, params)?;
    match (node, at) {
        (Some(node), Some(at)) => moves.push(Move { node, at }),
        (None, None) if !moves.is_empty() => {}
        _ => return invalid("give a `node` and where it goes, `at`, or several as `moves`"),
    }
    if moves.iter().any(|to| to.at.iter().any(|v| !v.is_finite() || v.abs() > 1e6)) {
        return invalid("a place in the node view is within ±1000000");
    }
    s.edit(title(id), |p| {
        for Move { node, at } in &moves {
            let has = |shot: &Shot| shot.id.0 == *node || shot.node_ids().any(|id| id == *node);
            let Some(shot) = p.shots.iter().find(|shot| has(shot)).map(|shot| shot.id) else { return invalid(format!("no node {node}")) };
            if let Some(shot) = p.shot_mut(shot) {
                shot.places.insert(*node, *at);
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Gives a node of the node view a colour of the user's own: a layer, a peg, a composite, a
/// deformer, a group or an effect by its id, or a shot's display by the shot's id; `colour`
/// as `[red, green, blue]`. With `colour` null or left out the node has its kind's colour
/// again. Several nodes are coloured in one step with `nodes`. Nothing of the picture changes.
pub(super) fn set_colour(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        node: Option<u64>,
        #[serde(default)]
        nodes: Vec<u64>,
        colour: Option<[u8; 3]>,
    }
    let id = "node.setColour";
    let Params { node, mut nodes, colour } = parse(id, params)?;
    nodes.extend(node);
    if nodes.is_empty() {
        return invalid("give the `node` to colour, or several as `nodes`");
    }
    s.edit(title(id), |p| {
        for node in &nodes {
            let has = |shot: &Shot| shot.id.0 == *node || shot.node_ids().any(|id| id == *node);
            let Some(shot) = p.shots.iter().find(|shot| has(shot)).map(|shot| shot.id) else { return invalid(format!("no node {node}")) };
            if let Some(shot) = p.shot_mut(shot) {
                match colour {
                    Some(colour) => drop(shot.colours.insert(*node, colour)),
                    None => drop(shot.colours.remove(node)),
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets how a composite orders what it stacks: `ordering` is `3d`, by depth among all else
/// the shot shows; `3dFlat`, by depth among themselves and then as one flat card; or `2d`, by
/// the order of its ports alone, as one flat card. `rightFirst` says which end of the ports is
/// on top where they are stacked by their order: the right with `true`, the left with `false`.
/// What is left out stays as it is.
pub(super) fn set_composite_ordering(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        composite: u64,
        ordering: Option<String>,
        right_first: Option<bool>,
    }
    let id = "composite.setOrdering";
    let Params { composite, ordering, right_first } = parse(id, params)?;
    let stacking = match &ordering {
        Some(said) => match iskra_model::Stacking::NAMES.iter().find(|(name, _)| name == said) {
            Some((_, stacking)) => Some(*stacking),
            None => return invalid(format!("`ordering` is one of: 3d, 3dFlat, 2d, not `{said}`")),
        },
        None => None,
    };
    s.edit(title(id), |p| {
        let shot = shot_of(p, composite)?.id;
        if let Some(composite) = p.shot_mut(shot).and_then(|shot| composite_mut(shot, CompositeId(composite))) {
            composite.stacking = stacking.unwrap_or(composite.stacking);
            composite.right_first = right_first.unwrap_or(composite.right_first);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
