//! Effect node commands (plan §8.2): the same for every kind of effect, since a kind is a
//! description and not code of its own. A node is made, wired, set, keyed and switched here.

use std::sync::Arc;

use iskra_model::{Effect, EffectId, Interp, PICTURE, ParamKind, Project, Shot, Source, TAG, Value as Set, effect_kind, effect_kinds, unique_name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{checked_frames, checked_name, invalid, name_is_free, parse, shot_id, title};
use crate::{Result, Session};

/// The shot that holds the effect a command names.
fn shot_of(project: &Project, effect: u64) -> Result<&Shot> {
    match project.shots.iter().find(|shot| shot.effect(EffectId(effect)).is_some()) {
        Some(shot) => Ok(shot),
        None => invalid(format!("no effect node {effect}")),
    }
}

/// Change the effect a command names.
fn change(p: &mut Project, effect: u64, f: impl FnOnce(&mut Effect) -> Result<()>) -> Result<()> {
    let shot = shot_of(p, effect)?.id;
    match p.shot_mut(shot).and_then(|shot| shot.effect_mut(EffectId(effect))) {
        Some(effect) => f(effect),
        None => Ok(()),
    }
}

/// The parameters a command about keyframes is about: the one it names, which the node's
/// kind has to have and which has to be a number or a switch, or without a name all that are.
fn numbers(effect: &Effect, param: Option<&str>) -> Result<Vec<&'static str>> {
    let Some(kind) = effect.described() else { return invalid(format!("`{}` is of a kind this version does not know: {}", effect.name, effect.kind)) };
    let is_number = |spec: &iskra_model::ParamSpec| matches!(spec.kind, ParamKind::Number { .. } | ParamKind::Switch { .. });
    match param {
        Some(id) => match kind.param(id) {
            Some(spec) if is_number(spec) => Ok(vec![spec.id]),
            Some(_) => invalid(format!("`{id}` is neither a number nor a switch, and only those have keyframes")),
            None => invalid(format!("{} has no `{id}`", kind.title)),
        },
        None => Ok(kind.params.iter().filter(|spec| is_number(spec)).map(|spec| spec.id).collect()),
    }
}

/// Lists the kinds of effect node there are, not an edit: for each its `id`, `title`,
/// `category` and `hint`, its `ports` (each a `name`, whether it takes a `picture` or a
/// `matte`, and whether it is `needed`) and its `params` (each an `id`, `title`, `hint` and
/// `type`, with what it starts as and for a number its range, step and unit).
pub(super) fn kinds(_: &mut Session, _: Value) -> Result<Value> {
    let listed = effect_kinds().iter().map(|kind| {
        let ports = kind.ports.iter().map(|port| {
            let takes = if port.kind == iskra_model::PortKind::Matte { "matte" } else { "picture" };
            json!({ "name": port.name, "takes": takes, "needed": port.needed })
        });
        let params = kind.params.iter().map(|spec| {
            let mut said = json!({ "id": spec.id, "title": spec.title, "hint": spec.hint });
            match spec.kind {
                ParamKind::Number { default, min, max, step, unit } => {
                    said["type"] = json!("number");
                    (said["default"], said["min"], said["max"], said["step"], said["unit"]) = (json!(default), json!(min), json!(max), json!(step), json!(unit));
                }
                ParamKind::Choice { options } => (said["type"], said["options"], said["default"]) = (json!("choice"), json!(options), json!(options.first())),
                ParamKind::Switch { default } => (said["type"], said["default"]) = (json!("switch"), json!(default)),
                ParamKind::Colour { default } => (said["type"], said["default"]) = (json!("colour"), json!(default)),
                ParamKind::Text => (said["type"], said["default"]) = (json!("text"), json!("")),
                ParamKind::Curves => (said["type"], said["default"]) = (json!("curves"), json!({})),
            }
            said
        });
        json!({ "id": kind.id, "title": kind.title, "category": kind.category, "hint": kind.hint, "ports": ports.collect::<Vec<_>>(), "params": params.collect::<Vec<_>>() })
    });
    Ok(json!({ "kinds": listed.collect::<Vec<_>>() }))
}

/// Adds an effect node of `kind` to a shot. With `on`, a node (`{ "layer": id }`,
/// `{ "composite": id }` or `{ "effect": id }`), it is put on the wire that leaves that node:
/// the node's picture goes through it, and it goes wherever the node went. Without, it lies
/// in the shot wired to nothing. Without a name it gets a free one, its kind's. `at` is where
/// it is put in the node view; without it the view lays it out. `group` is the group it is
/// put into, which a port of a group wants. A Send gets its name as its tag. With `from`, a
/// node as for `on`, that node's picture is wired into the new one and goes on wherever else
/// it went too; with `parent`, a peg or what hands one down, that transform is wired into it,
/// which a port or a Send takes. One of `on`, `from` and `parent` at most. With `stack`, a
/// node that makes its picture itself (a Colour Card, Particles) is stacked in the shot's
/// main composite: `"under"` everything else, as a background is, or `"over"` it.
pub(super) fn new(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        kind: String,
        name: Option<String>,
        on: Option<Source>,
        at: Option<[f64; 2]>,
        group: Option<u64>,
        from: Option<Source>,
        parent: Option<u64>,
        stack: Option<String>,
    }
    let id = "effect.new";
    let Params { shot, kind, name, on, at, group, from, parent, stack } = parse(id, params)?;
    let over = match stack.as_deref() {
        None => None,
        Some("under") => Some(false),
        Some("over") => Some(true),
        Some(other) => return invalid(format!("`stack` is \"under\" or \"over\", not \"{other}\"")),
    };
    if usize::from(on.is_some()) + usize::from(from.is_some()) + usize::from(parent.is_some()) > 1 {
        return invalid("give one of `on`, `from` and `parent`: a node is wired one way as it is made");
    }
    if at.is_some_and(|at| at.iter().any(|v| !v.is_finite() || v.abs() > 1e6)) {
        return invalid("a place in the node view is within ±1000000");
    }
    let Some(kind) = effect_kind(&kind) else {
        return invalid(format!("no kind of effect `{kind}`: there are {}", effect_kinds().iter().map(|kind| kind.id).collect::<Vec<_>>().join(", ")));
    };
    let made = s.edit(title(id), |p| {
        let within = match on {
            Some(Source::Effect(on)) => shot_of(p, on.0)?.id,
            _ => shot_id(p, shot)?,
        };
        let Some(shot) = p.shot(within) else { return invalid("no such shot") };
        if let Some(on) = on {
            super::graph::source_in(shot, on)?;
            if kind.port(PICTURE).is_none() {
                return invalid(format!("a {} takes no picture, so it can't be put on a wire", kind.title));
            }
        }
        if let Some(from) = from {
            super::graph::source_in(shot, from)?;
            if kind.port(PICTURE).is_none() {
                return invalid(format!("a {} takes no picture", kind.title));
            }
        }
        if over.is_some() && !kind.makes_picture() {
            return invalid(format!("only a node that makes its own picture is stacked, and a {} does not", kind.title));
        }
        // Put on a wire, or taking a node's picture or a peg's transform, it goes into that
        // node's group, unless it is told another.
        let by = on.or(from).map(|source| super::group::key(&source)).or(parent);
        let group = group.or_else(|| super::group::group_by(shot, by?));
        let parent = super::peg::parent_in(shot, parent)?;
        if parent.is_some() && ![iskra_model::PORT_IN, iskra_model::PORT_OUT, iskra_model::SEND].contains(&kind.id) {
            return invalid(format!("a {} is no port and no Send: only those take a transform", kind.title));
        }
        if let Some(group) = group
            && shot.group(iskra_model::GroupId(group)).is_none()
        {
            return invalid(format!("no group {group}"));
        }
        let name = match &name {
            Some(name) => {
                let name = checked_name(name)?;
                name_is_free(shot, name, None)?;
                name.to_string()
            }
            None => unique_name(kind.title, shot.node_names()),
        };
        let made = EffectId(p.new_id());
        let Some(shot) = p.shot_mut(within) else { return Ok(made) };
        let mut effect = Effect::new(made, &name, kind);
        // A new Send sends under its own name, so that it can be received at once.
        if kind.id == iskra_model::SEND
            && let Some(tag) = effect.param_mut(iskra_model::TAG)
        {
            tag.value = Set::Text(name.clone());
        }
        if let Some(on) = on {
            // Everything that took the node's picture takes it through the new one; a matte, which
            // a cutter or a mask takes from the node to cut with, is still the node's own.
            shot.rewire_picture(on, Source::Effect(made));
            effect.inputs.insert(PICTURE.to_string(), on);
        }
        if let Some(from) = from {
            effect.inputs.insert(PICTURE.to_string(), from);
        }
        effect.parent = parent;
        shot.effects.push(Arc::new(effect));
        // At the top or the bottom of the main composite, which stacks the top first.
        if let Some(over) = over
            && let Some(main) = shot.main_composite()
            && let Some(composite) = shot.composites.iter_mut().find(|composite| composite.id == main)
        {
            let inputs = &mut Arc::make_mut(composite).inputs;
            inputs.insert(if over { 0 } else { inputs.len() }, Source::Effect(made));
        }
        if let Some(at) = at {
            shot.places.insert(made.0, at);
        }
        if let Some(group) = group.and_then(|group| shot.groups.iter_mut().find(|held| held.id.0 == group)) {
            Arc::make_mut(group).members.insert(made.0);
        }
        Ok(made)
    })?;
    Ok(json!({ "id": made }))
}

pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        name: String,
    }
    let id = "effect.rename";
    let Params { effect, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        name_is_free(shot_of(p, effect)?, name, Some(effect))?;
        change(p, effect, |effect| {
            effect.name = name.to_string();
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Deletes an effect node. The wire it was on is joined again: what went through it goes
/// straight to where the node went.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
    }
    let id = "effect.delete";
    let Params { effect } = parse(id, params)?;
    s.edit(title(id), |p| {
        let within = shot_of(p, effect)?.id;
        let Some(shot) = p.shot_mut(within) else { return Ok(()) };
        let through = shot.effect(EffectId(effect)).and_then(Effect::picture);
        // What was parented to a port is carried by what came into the port.
        let above = shot.handed_down(iskra_model::PegId(effect));
        shot.reparent(iskra_model::PegId(effect), above);
        shot.rewire_ports(iskra_model::PegId(effect), above);
        shot.effects.retain(|other| other.id != EffectId(effect));
        shot.rewire(Source::Effect(EffectId(effect)), through);
        shot.places.remove(&effect);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Puts an effect node that is in the shot already onto the wire that leaves `on`, a node
/// (`{ "layer": id }`, `{ "composite": id }` or `{ "effect": id }`): it is taken off the wire
/// it was on, which is joined again, and then the node's picture goes through it to wherever
/// the node went. It can't be put on a wire that it gives its own picture to.
pub(super) fn put_on(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        on: Source,
    }
    let id = "effect.putOn";
    let Params { effect, on } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, effect)?;
        let this = Source::Effect(EffectId(effect));
        super::graph::source_in(shot, on)?;
        if on == this {
            return invalid("a node can't be put on its own wire");
        }
        if let Some(kind) = shot.effect(EffectId(effect)).and_then(Effect::described)
            && kind.port(PICTURE).is_none()
        {
            return invalid(format!("a {} takes no picture, so it can't be put on a wire", kind.title));
        }
        let within = shot.id;
        let Some(shot) = p.shot_mut(within) else { return Ok(()) };
        // Off the wire it was on: what went through it goes straight on.
        let through = shot.effect(EffectId(effect)).and_then(Effect::picture);
        if let Some(effect) = shot.effect_mut(EffectId(effect)) {
            effect.inputs.remove(PICTURE);
        }
        shot.rewire(this, through.filter(|through| *through != this));
        // And onto the other: if that came from this one, it has just been joined past it. A
        // matte wired from the other stays its own.
        shot.rewire_picture(on, this);
        if let Some(effect) = shot.effect_mut(EffectId(effect)) {
            // Another of its ports that took that node would now take the node itself.
            effect.inputs.retain(|_, input| *input != this);
            effect.inputs.insert(PICTURE.to_string(), on);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Takes an effect node off the wire it is on, and leaves it in the shot wired to nothing. The
/// wire is joined again: what went through the node goes straight to where the node went, and
/// what was parented to a port is carried by what came into the port. `at` is where the node
/// is put in the node view, if it is to be moved too.
pub(super) fn take_off(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        at: Option<[f64; 2]>,
    }
    let id = "effect.takeOff";
    let Params { effect, at } = parse(id, params)?;
    if at.is_some_and(|at| at.iter().any(|v| !v.is_finite() || v.abs() > 1e6)) {
        return invalid("a place in the node view is within ±1000000");
    }
    s.edit(title(id), |p| {
        let within = shot_of(p, effect)?.id;
        let Some(shot) = p.shot_mut(within) else { return Ok(()) };
        shot.take_off(EffectId(effect));
        if let Some(at) = at {
            shot.places.insert(effect, at);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Wires a node (`{ "layer": id }`, `{ "composite": id }` or `{ "effect": id }`) into a port
/// of an effect node, in place of what was wired there; with `node` null or left out the port
/// is emptied. The port is `picture` unless `port` names another. A node can't be wired into
/// one that it gets its picture from.
pub(super) fn connect(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        port: Option<String>,
        node: Option<Source>,
    }
    let id = "effect.connect";
    let Params { effect, port, node } = parse(id, params)?;
    let port = port.unwrap_or_else(|| PICTURE.to_string());
    s.edit(title(id), |p| {
        let shot = shot_of(p, effect)?;
        let this = shot.effect(EffectId(effect)).expect("the shot was found by it");
        // A kind this build does not know has the port every kind has.
        if this.described().map_or(port != PICTURE, |kind| kind.port(&port).is_none()) {
            return invalid(format!("`{}` has no port `{port}`", this.name));
        }
        if let Some(node) = node {
            super::graph::source_in(shot, node)?;
            if this.parent.is_some() {
                return invalid(format!("`{}` has a transform wired into it: a port carries a picture or a transform, not both", this.name));
            }
            if shot.flows_into(Source::Effect(EffectId(effect)), node) {
                return invalid("a node can't be wired into itself or into one that it gets its picture from");
            }
        }
        change(p, effect, |effect| {
            match node {
                Some(node) => drop(effect.inputs.insert(port, node)),
                None => drop(effect.inputs.remove(&port)),
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Sets a parameter of an effect node: `param`, by its id, to `value`, a number, `true` or
/// `false`, the name of a choice, or a colour as `[red, green, blue]` or with how solid as a
/// fourth, each from 0 to 1, or curves as `{ master, red, green, blue }`, each a list of
/// points `[in, out]` from 0 to 1 that the curve goes through, as the parameter is. A number
/// is held to the parameter's range. A parameter that has keyframes is changed by key: `frame` says on which, and a key
/// is made there if there is none; with `adjust`, a list of frames, the number's keys on them
/// move by as much as it moved on `frame`. A Send's `tag` changed takes the Receives that read
/// it along, in the same step.
pub(super) fn set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        param: String,
        value: Value,
        frame: Option<u32>,
        #[serde(default)]
        adjust: Vec<u32>,
    }
    let id = "effect.set";
    let Params { effect, param, value, frame, adjust } = parse(id, params)?;
    let value = match value {
        Value::Number(number) => Set::Number(number.as_f64().unwrap_or(f64::NAN)),
        Value::Bool(on) => Set::Switch(on),
        Value::String(choice) => Set::Choice(choice),
        // A colour: red, green and blue, and how solid if it is said.
        Value::Array(parts) if (3..=4).contains(&parts.len()) && parts.iter().all(Value::is_number) => {
            let part = |at: usize| parts.get(at).and_then(Value::as_f64).unwrap_or(1.0);
            Set::Colour([part(0), part(1), part(2), part(3)])
        }
        // Curves: `master`, `red`, `green` and `blue`, each a list of points, a tone going
        // in and the tone it comes out as; one that is left out is straight.
        Value::Object(curves) => match serde_json::from_value(Value::Object(curves)) {
            Ok(curves) => Set::Curves(curves),
            Err(why) => return invalid(format!("`value` as curves is `master`, `red`, `green` and `blue`, each a list of `[in, out]`: {why}")),
        },
        other => return invalid(format!("`value` is a number, true or false, a name, a colour or curves, not {other}")),
    };
    if let Some(frame) = frame {
        checked_frames(frame, 1)?;
    }
    s.edit(title(id), |p| {
        // The Receives that read a Send whose tag is set go on reading it, under the new tag.
        let shot = shot_of(p, effect)?;
        let readers: Vec<u64> = match param == TAG {
            true => shot.effects.iter().filter(|other| shot.sender(other).is_some_and(|send| send.id.0 == effect)).map(|other| other.id.0).collect(),
            false => Vec::new(),
        };
        change(p, effect, |effect| {
            let Some(kind) = effect.described() else { return invalid(format!("`{}` is of a kind this version does not know: {}", effect.name, effect.kind)) };
            let Some(spec) = kind.param(&param) else { return invalid(format!("{} has no `{param}`", kind.title)) };
            let value = spec.checked(value).or_else(invalid)?;
            let Some(held) = effect.param_mut(&param) else { return Ok(()) };
            match (&value, held.keys.is_empty(), frame) {
                (Set::Number(number), false, Some(frame)) => held.keys.set_adjusting(frame, *number, false, &adjust),
                (Set::Switch(on), false, Some(frame)) => key_switch(held, frame, *on),
                (Set::Number(_) | Set::Switch(_), false, None) => return invalid(format!("`{param}` has keyframes: say on which `frame` it is set")),
                _ => held.value = value,
            }
            Ok(())
        })?;
        let tag = shot_of(p, effect)?.effect(EffectId(effect)).map(|send| send.text(TAG)).unwrap_or_default();
        for reader in readers {
            change(p, reader, |receive| {
                if let Some(held) = receive.param_mut(TAG) {
                    held.value = Set::Text(tag.clone());
                }
                Ok(())
            })?;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Keys a switch on `frame`: as 1 for on and 0 for off, held until the next key.
fn key_switch(held: &mut iskra_model::Param, frame: u32, on: bool) {
    held.keys.set(frame, if on { 1.0 } else { 0.0 });
    if let Some(key) = held.keys.key_mut(frame) {
        key.out = Interp::Hold;
    }
}

/// Keys a parameter of an effect node on `frame`, as it is there: `param`, which has to be a
/// number or a switch, or without one every number and switch the node has. A switch stays
/// as a keyframe has it until the next.
pub(super) fn set_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        param: Option<String>,
        frame: u32,
    }
    let id = "effect.setKey";
    let Params { effect, param, frame } = parse(id, params)?;
    checked_frames(frame, 1)?;
    s.edit(title(id), |p| {
        change(p, effect, |effect| {
            for id in numbers(effect, param.as_deref())? {
                let (now, on) = (effect.number(id, frame), effect.switch(id, frame));
                match effect.param_mut(id) {
                    Some(held) if matches!(held.value, Set::Switch(_)) => key_switch(held, frame, on),
                    Some(held) => held.keys.set(frame, now),
                    None => {}
                }
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Takes the keyframes on `frame` away: of `param`, or of every parameter. A parameter that
/// loses its last key stays as that key had it.
pub(super) fn remove_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        param: Option<String>,
        frame: u32,
    }
    let id = "effect.removeKey";
    let Params { effect, param, frame } = parse(id, params)?;
    s.edit(title(id), |p| {
        change(p, effect, |effect| {
            let wanted = numbers(effect, param.as_deref())?;
            if !wanted.iter().any(|id| effect.key_frames(Some(id)).contains(&frame)) {
                return invalid(format!("`{}` has no keyframe on frame {frame}", effect.name));
            }
            for id in wanted {
                let Some(held) = effect.params.get_mut(id) else { continue };
                if let (Some(key), true) = (held.keys.remove(frame), held.keys.is_empty()) {
                    held.value = if matches!(held.value, Set::Switch(_)) { Set::Switch(key.value >= 0.5) } else { Set::Number(key.value) };
                }
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Moves the keyframes on `from` to `to`: of `param`, or of every parameter. Keys that are
/// on `to` already give way.
pub(super) fn move_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        param: Option<String>,
        from: u32,
        to: u32,
    }
    let id = "effect.moveKey";
    let Params { effect, param, from, to } = parse(id, params)?;
    checked_frames(to, 1)?;
    s.edit(title(id), |p| {
        change(p, effect, |effect| {
            let wanted = numbers(effect, param.as_deref())?;
            if !wanted.iter().any(|id| effect.key_frames(Some(id)).contains(&from)) {
                return invalid(format!("`{}` has no keyframe on frame {from}", effect.name));
            }
            for id in wanted {
                if let Some(held) = effect.params.get_mut(id) {
                    held.keys.move_key(from, to);
                }
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Sets how the keyframes on `frame` ease, `into` them, `out` of them, or both, as a peg's
/// do (`peg.setEase`): of `param`, or of every parameter.
pub(super) fn set_ease(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        param: Option<String>,
        frame: u32,
        into: Option<Interp>,
        out: Option<Interp>,
    }
    let id = "effect.setEase";
    let Params { effect, param, frame, into, out } = parse(id, params)?;
    if into == Some(Interp::Hold) {
        return invalid("a value is held out of a keyframe, not into one");
    }
    s.edit(title(id), |p| {
        change(p, effect, |effect| {
            let wanted = numbers(effect, param.as_deref())?;
            if !wanted.iter().any(|id| effect.key_frames(Some(id)).contains(&frame)) {
                return invalid(format!("`{}` has no keyframe on frame {frame}", effect.name));
            }
            for id in wanted {
                if let Some(key) = effect.params.get_mut(id).and_then(|held| held.keys.key_mut(frame)) {
                    (key.into, key.out) = (into.unwrap_or(key.into), out.unwrap_or(key.out));
                }
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Wires a peg's transform into a port of a group or into a Send (plan §8.6): `parent`, a peg,
/// or a port or a named port that hands one down; without it the wire is taken away. What is
/// parented to the node, by the node's id where a peg's would be, is then carried by that
/// peg. A port carries a picture or a transform, not both: one that has a picture wired in
/// takes no transform.
pub(super) fn set_parent(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        parent: Option<u64>,
    }
    let id = "effect.setParent";
    let Params { effect, parent } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, effect)?;
        let parent = super::peg::parent_in(shot, parent)?;
        let Some(node) = shot.effect(EffectId(effect)) else { return Ok(()) };
        if !node.hands_on() {
            return invalid(format!("`{}` is no port and no Send: only those take a transform", node.name));
        }
        if parent.is_some() && node.picture().is_some() {
            return invalid(format!("`{}` has a picture wired into it: a port carries a picture or a transform, not both", node.name));
        }
        if shot.carried_from(parent, effect) {
            return invalid("a port can't take the transform of what is under it");
        }
        change(p, effect, |effect| {
            effect.parent = parent;
            Ok(())
        })?;
        super::driver::no_loop_in(p, shot_of(p, effect)?.id)
    })?;
    Ok(Value::Null)
}

/// Switches an effect node on or off. Off, it lets its picture through unchanged, in the
/// window and in a render alike.
pub(super) fn set_enabled(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        effect: u64,
        enabled: bool,
    }
    let id = "effect.setEnabled";
    let Params { effect, enabled } = parse(id, params)?;
    s.edit(title(id), |p| {
        change(p, effect, |effect| {
            effect.enabled = enabled;
            Ok(())
        })
    })?;
    Ok(Value::Null)
}
