//! Keyframes copied and pasted: a peg's, a pose of bones', an effect node's, the camera's.

use iskra_model::{CycleKind, KeyClip, KeyOwner};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{checked_frames, invalid, parse, shot_id, title};
use crate::{Result, Session};

/// A keyframe as copy and paste name it: what has it, its frame, and for one value's key
/// alone that value's name.
#[derive(Deserialize, Serialize)]
struct Picked {
    owner: KeyOwner,
    frame: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

/// Copies the keyframes `keys` of a shot, each an `owner` (`{ "peg": id }`, `{ "deformer":
/// id }`, `{ "effect": id }` or `{ "camera": shotId }`) and a `frame`: all of its values that
/// have a key there, or with `value` that one alone (`position`, `rotation`, `scale`, `skew`,
/// `depth`, `tilt`; `offset`, `bone0.turn`, `bone0.stretch`, `point0.move`, `point0.into`,
/// `point0.out`; an effect's parameter). Changes nothing; gives the `clip`, for `keys.paste`.
pub(super) fn copy(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        keys: Vec<Picked>,
    }
    let Params { shot, keys } = parse("keys.copy", params)?;
    let p = s.project();
    let Some(shot) = p.shot(shot_id(p, shot)?) else { return invalid("no such shot") };
    let picked: Vec<(KeyOwner, u32, Option<String>)> = keys.iter().map(|key| (key.owner, key.frame, key.value.clone())).collect();
    let clip = shot.copy_keys_of(&picked);
    if clip.frames.is_empty() {
        return invalid("none of those has a keyframe on its frame");
    }
    Ok(json!({ "clip": clip }))
}

/// Pastes a `clip` that `keys.copy` gave, its first frame on `frame` and the rest as far
/// after as they were: each key on the value it was of, with its eases and handles, whatever
/// the Animate switch says. With `onto`, an owner, the keys of the one thing of its kind in
/// the clip go onto it; a value it has not got is left out. Gives the keyframes pasted.
pub(super) fn paste(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        clip: KeyClip,
        frame: u32,
        onto: Option<KeyOwner>,
    }
    let id = "keys.paste";
    let Params { shot, clip, frame, onto } = parse(id, params)?;
    let last = clip.frames.iter().map(|copied| copied.frame).max().unwrap_or(0);
    checked_frames(frame, last.saturating_add(1))?;
    let pasted = s.edit(title(id), |p| {
        let within = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(within) else { return invalid("no such shot") };
        let pasted = shot.paste_keys(&clip, frame, onto);
        if pasted.is_empty() {
            return invalid("none of the keyframes has a value to go on there");
        }
        Ok(pasted)
    })?;
    let pasted: Vec<Picked> = pasted.into_iter().map(|(owner, frame)| Picked { owner, frame, value: None }).collect();
    Ok(json!({ "keys": pasted }))
}

/// Makes the motion between keyframes held steps of `every` frames (1, 2 or 3 and more), as
/// drawn animation on ones, twos or threes moves: of `keys`, picked as `keys.copy` takes them,
/// each owner's from each of its picked keys to the next of them, or from its one picked key to
/// its next key. A key goes on every `every` frames of the way, of the value its curve had
/// there, and each is held until the next; the key that ends the way is kept, so that what
/// comes after goes on as it went. Every value with a key on the way is stepped, whatever its
/// values; one with none there is left as it is.
/// Gives how many ways were stepped.
pub(super) fn step(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        keys: Vec<Picked>,
        every: u32,
    }
    let id = "keys.step";
    let Params { shot, keys, every } = parse(id, params)?;
    if every == 0 {
        return invalid("`every` is a number of frames, 1 or more");
    }
    // Each owner's picked frames, of all its values or of one named.
    let mut ways: Vec<((KeyOwner, Option<String>), Vec<u32>)> = Vec::new();
    for key in keys {
        let of = (key.owner, key.value);
        match ways.iter_mut().find(|(owner, _)| *owner == of) {
            Some((_, frames)) => frames.push(key.frame),
            None => ways.push((of, vec![key.frame])),
        }
    }
    let stepped = s.edit(title(id), |p| {
        let within = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(within) else { return invalid("no such shot") };
        let mut stepped = 0;
        for ((owner, value), mut frames) in ways {
            frames.sort_unstable();
            frames.dedup();
            if let [only] = frames[..]
                && let Some(next) = shot.next_key_of(owner, value.as_deref(), only)
            {
                frames.push(next);
            }
            for pair in frames.windows(2) {
                stepped += usize::from(shot.step_keys(owner, value.as_deref(), pair[0], pair[1], every));
            }
        }
        if stepped == 0 {
            return invalid("there is no other keyframe to step towards");
        }
        Ok(stepped)
    })?;
    Ok(json!({ "stepped": stepped }))
}

/// Repeats the motion between keyframes after them, as a cycle: of `keys`, picked as
/// `keys.copy` takes them, each owner's from the first of its picked keys to the last, which
/// takes two or more of its keys. `kind` is `loop`, which starts the cycle again each time, or
/// `pingPong`, which goes back and forth; the cycle goes on until the owner's next key, or to
/// the end. Each value with a key on the way cycles, and a key is made on the last frame of
/// one that has none there. With `kind` null, the cycles that the picked keys are in are taken
/// away. Gives how many values were changed.
pub(super) fn cycle(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        keys: Vec<Picked>,
        kind: Option<CycleKind>,
    }
    let id = "keys.cycle";
    let Params { shot, keys, kind } = parse(id, params)?;
    // Each owner's picked frames, of all its values or of one named: the first and the last.
    let mut spans: Vec<((KeyOwner, Option<String>), (u32, u32))> = Vec::new();
    for key in keys {
        let of = (key.owner, key.value);
        match spans.iter_mut().find(|(owner, _)| *owner == of) {
            Some((_, (from, to))) => (*from, *to) = ((*from).min(key.frame), (*to).max(key.frame)),
            None => spans.push((of, (key.frame, key.frame))),
        }
    }
    let changed = s.edit(title(id), |p| {
        let within = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(within) else { return invalid("no such shot") };
        let mut changed = 0;
        for ((owner, value), (from, to)) in spans {
            if kind.is_some() && from == to {
                continue;
            }
            changed += shot.cycle_keys(owner, value.as_deref(), from, to, kind);
        }
        match (changed, kind) {
            (0, Some(_)) => invalid("a cycle is of two keyframes or more of one thing"),
            (0, None) => invalid("none of those keyframes is in a cycle"),
            (changed, _) => Ok(changed),
        }
    })?;
    Ok(json!({ "changed": changed }))
}

/// Times the way of `owner` (as `keys.copy` names one) from its keyframe on `from` to its
/// next, on `to`, by a timing chart: `spacing` says how far along the way the value is on each
/// frame between them, in order, each from 0 to 1, and the value on that frame is where the
/// steady way is when it is that far, which can fall between frames. Every value with a key on
/// both frames and none between is timed alike, or with `value` that one alone. Parts are kept
/// from 0 to 1 and never going back. An empty `spacing` takes the chart away, and the way is
/// as its eases shape it. Gives how many values were timed.
pub(super) fn chart(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        owner: KeyOwner,
        #[serde(default)]
        value: Option<String>,
        from: u32,
        to: u32,
        spacing: Vec<f64>,
    }
    let id = "keys.chart";
    let Params { shot, owner, value, from, to, spacing } = parse(id, params)?;
    if to <= from {
        return invalid("`to` is a later frame than `from`");
    }
    if spacing.iter().any(|part| !part.is_finite()) {
        return invalid("each part of the way is a number from 0 to 1");
    }
    let timed = s.edit(title(id), |p| {
        let within = shot_id(p, shot)?;
        let Some(shot) = p.shot_mut(within) else { return invalid("no such shot") };
        match shot.chart_keys(owner, value.as_deref(), from, to, &spacing) {
            0 => invalid(format!("nothing goes from a keyframe on {from} straight to one on {to}")),
            timed => Ok(timed),
        }
    })?;
    Ok(json!({ "timed": timed }))
}
