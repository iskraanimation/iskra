//! Driver node commands (plan §8.8): a driver is made on a peg, given expressions, inputs and
//! parameters, keyed, switched, and baked into the peg's keys.

use std::sync::Arc;

use iskra_model::{
    Driven, Driver, DriverId, DriverInput, DriverParameter, DriverPreset, Expressions, Interp, PegId, Project, Shot, ShotId, Track, driver_presets,
    reserved_in_expressions, unique_name,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{checked_frames, checked_name, invalid, name_is_free, parse, title};
use crate::{Result, Session};

/// The shot that holds the driver a command names.
fn shot_of(project: &Project, driver: u64) -> Result<&Shot> {
    match project.shots.iter().find(|shot| shot.driver(DriverId(driver)).is_some()) {
        Some(shot) => Ok(shot),
        None => invalid(format!("no driver {driver}")),
    }
}

/// The shot that holds a peg a command names.
fn shot_of_peg(project: &Project, peg: u64) -> Result<&Shot> {
    match project.shots.iter().find(|shot| shot.peg(PegId(peg)).is_some()) {
        Some(shot) => Ok(shot),
        None => invalid(format!("no peg {peg}")),
    }
}

fn driver_in(shot: &Shot, driver: u64) -> Result<&Driver> {
    shot.driver(DriverId(driver)).map_or_else(|| invalid(format!("no driver {driver}")), Ok)
}

/// Change the driver a command names, then check that what its expressions read it has.
fn change(p: &mut Project, driver: u64, f: impl FnOnce(&mut Driver) -> Result<()>) -> Result<()> {
    let within = shot_of(p, driver)?.id;
    let Some(shot) = p.shot_mut(within) else { return Ok(()) };
    let Some(held) = shot.drivers.iter_mut().find(|held| held.id == DriverId(driver)) else { return Ok(()) };
    let held = Arc::make_mut(held);
    f(held)?;
    reads_what_it_has(held)
}

/// A driver's expressions read only the inputs and parameters it has.
fn reads_what_it_has(driver: &Driver) -> Result<()> {
    match driver.missing().as_slice() {
        [] => Ok(()),
        [one] => invalid(format!("the expressions read `{one}`, which `{}` has no input or parameter for", driver.name)),
        many => invalid(format!("the expressions read {}, which `{}` has no inputs or parameters for", quoted(many), driver.name)),
    }
}

fn quoted(names: &[String]) -> String {
    names.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ")
}

/// No driver of `shot` moves its own peg (plan §8.2 line 8).
pub(super) fn no_loop_in(p: &Project, shot: ShotId) -> Result<()> {
    let Some(shot) = p.shot(shot) else { return Ok(()) };
    match shot.driver_in_a_loop() {
        Some(driver) => {
            let peg = shot.peg(driver.drives).map_or("its peg", |peg| peg.name.as_str());
            invalid(format!("that would have `{peg}` move itself, through its driver `{}`", driver.name))
        }
        None => Ok(()),
    }
}

/// The same for the shot that holds `peg`.
pub(super) fn no_loop(p: &Project, peg: PegId) -> Result<()> {
    match p.shots.iter().find(|shot| shot.peg(peg).is_some()) {
        Some(shot) => no_loop_in(p, shot.id),
        None => Ok(()),
    }
}

/// How long the name of an input or a parameter may be.
const NAME_MOST: usize = 32;

/// A name for an input or a parameter as typed: a name the expressions can read, which the
/// language does not have already.
fn checked_word(name: &str) -> Result<&str> {
    let name = name.trim();
    let mut chars = name.chars();
    let first = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$');
    if !first || !chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$') {
        return invalid(format!("`{name}` can't be read by expressions: a name begins with a letter and has letters, digits and `_` in it"));
    }
    if reserved_in_expressions(name) {
        return invalid(format!("`{name}` is a name the expressions have already; take another"));
    }
    // Held as short as a timeline row holds its value's name.
    if name.len() > NAME_MOST {
        return invalid(format!("`{name}` is longer than {NAME_MOST} letters: take a shorter name"));
    }
    Ok(name)
}

/// `name` is no other input's or parameter's of `driver`.
fn word_is_free(driver: &Driver, name: &str) -> Result<()> {
    if driver.input(name).is_some() {
        return invalid(format!("`{}` has an input named `{name}` already", driver.name));
    }
    if driver.parameter(name).is_some() {
        return invalid(format!("`{}` has a parameter named `{name}` already", driver.name));
    }
    Ok(())
}

/// `peg` may be wired into a driver of `drives`: it is in the shot, and does not move it.
fn wirable(shot: &Shot, drives: PegId, peg: u64) -> Result<PegId> {
    let peg = PegId(peg);
    let Some(read) = shot.peg(peg) else { return invalid(format!("no peg {} in `{}`", peg.0, shot.name)) };
    if shot.driver_loops(drives, peg) {
        let driven = shot.peg(drives).map_or("the peg", |peg| peg.name.as_str());
        let how = if peg == drives { "is" } else { "moves with" };
        return invalid(format!("`{}` {how} `{driven}`, so it can't drive it", read.name));
    }
    Ok(peg)
}

fn expressions(text: &str) -> Result<Expressions> {
    Expressions::parse(text).or_else(|why| invalid(why.to_string()))
}

#[derive(Deserialize)]
struct Input {
    name: String,
    peg: Option<u64>,
}

#[derive(Deserialize)]
struct Parameter {
    name: String,
    #[serde(default)]
    value: f64,
}

/// Makes a driver on `peg`, which has none: with `expressions`, `inputs` (each a `name` and
/// the `peg` wired into it) and `parameters` (each a `name` and a `value`), all of which may be
/// left out, and named `name` or after the peg. Returns its `id`.
pub(super) fn new(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        peg: u64,
        name: Option<String>,
        #[serde(default)]
        expressions: String,
        #[serde(default)]
        inputs: Vec<Input>,
        #[serde(default)]
        parameters: Vec<Parameter>,
        at: Option<[f64; 2]>,
    }
    let id = "driver.new";
    let Params { peg, name, expressions: text, inputs, parameters, at } = parse(id, params)?;
    if at.is_some_and(|at| at.iter().any(|v| !v.is_finite() || v.abs() > 1e6)) {
        return invalid("a place in the node view is within ±1000000");
    }
    let parsed = expressions(&text)?;
    let made = s.edit(title(id), |p| {
        let within = shot_of_peg(p, peg)?.id;
        let made = DriverId(p.new_id());
        let Some(shot) = p.shot(within) else { return invalid("no such shot") };
        let driven = shot.peg(PegId(peg)).map(|peg| peg.name.clone()).unwrap_or_default();
        if let Some(other) = shot.driver_of(PegId(peg)) {
            return invalid(format!("`{driven}` has a driver already, `{}`", other.name));
        }
        let name = match &name {
            Some(name) => {
                let name = checked_name(name)?;
                name_is_free(shot, name, None)?;
                name.to_string()
            }
            None => unique_name(&format!("{driven} Driver"), shot.node_names()),
        };
        let mut driver = Driver::new(made, &name, PegId(peg));
        driver.expressions = parsed;
        for input in &inputs {
            let name = checked_word(&input.name)?;
            word_is_free(&driver, name)?;
            let peg = input.peg.map(|read| wirable(shot, PegId(peg), read)).transpose()?;
            driver.inputs.push(DriverInput { name: name.to_string(), peg });
        }
        for parameter in &parameters {
            let name = checked_word(&parameter.name)?;
            word_is_free(&driver, name)?;
            if !parameter.value.is_finite() {
                return invalid(format!("`{name}` is a number"));
            }
            driver.parameters.push(DriverParameter { name: name.to_string(), value: parameter.value, track: Track::default() });
        }
        reads_what_it_has(&driver)?;
        let Some(shot) = p.shot_mut(within) else { return Ok(made) };
        if let Some(at) = at {
            shot.places.insert(made.0, at);
        }
        shot.drivers.push(Arc::new(driver));
        Ok(made)
    })?;
    Ok(json!({ "id": made.0 }))
}

pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
    }
    let id = "driver.delete";
    let Params { driver } = parse(id, params)?;
    s.edit(title(id), |p| {
        let within = shot_of(p, driver)?.id;
        let Some(shot) = p.shot_mut(within) else { return Ok(()) };
        shot.drivers.retain(|held| held.id != DriverId(driver));
        shot.places.remove(&driver);
        shot.colours.remove(&driver);
        Ok(())
    })?;
    Ok(Value::Null)
}

pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        name: String,
    }
    let id = "driver.rename";
    let Params { driver, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        name_is_free(shot_of(p, driver)?, name, Some(driver))?;
        change(p, driver, |driver| {
            driver.name = name.to_string();
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Gives a driver new expressions, which read only the inputs and parameters it has. They are
/// its own now, not a preset's.
pub(super) fn set_expressions(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        expressions: String,
    }
    let id = "driver.setExpressions";
    let Params { driver, expressions: text } = parse(id, params)?;
    let parsed = expressions(&text)?;
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            if driver.expressions.text() != text {
                driver.preset = None;
            }
            driver.expressions = parsed;
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Puts a preset on a driver (plan §8.8): `preset`, the name of one that comes with Iskra,
/// or one whole, as a preset file holds it (`name`, `expressions`, `inputs`, `parameters`).
/// The driver takes its expressions; each input it reads that the driver has not got is added,
/// empty; each of its parameters is set to the preset's value, or added at it. What the driver
/// had besides stays.
pub(super) fn set_preset(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Preset {
        Named(String),
        Whole(DriverPreset),
    }
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        preset: Preset,
    }
    let id = "driver.setPreset";
    let Params { driver, preset } = parse(id, params)?;
    let preset = match preset {
        Preset::Whole(preset) => preset,
        Preset::Named(name) => match driver_presets().into_iter().find(|preset| preset.name == name) {
            Some(preset) => preset,
            None => {
                let names: Vec<String> = driver_presets().into_iter().map(|preset| preset.name).collect();
                return invalid(format!("no preset `{name}`: there are {}", quoted(&names)));
            }
        },
    };
    let parsed = expressions(&preset.expressions)?;
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            for input in &preset.inputs {
                let name = checked_word(input)?;
                if driver.input(name).is_none() {
                    word_is_free(driver, name)?;
                    driver.inputs.push(DriverInput { name: name.to_string(), peg: None });
                }
            }
            for parameter in &preset.parameters {
                let name = checked_word(&parameter.name)?;
                if !parameter.value.is_finite() {
                    return invalid(format!("`{name}` is a number"));
                }
                match driver.parameters.iter_mut().find(|held| held.name == name) {
                    Some(held) => (held.value, held.track) = (parameter.value, Track::default()),
                    None => {
                        word_is_free(driver, name)?;
                        driver.parameters.push(DriverParameter { name: name.to_string(), value: parameter.value, track: Track::default() });
                    }
                }
            }
            (driver.expressions, driver.preset) = (parsed, Some(preset.name.clone()));
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Adds an input to a driver, named `name`, with `peg` wired into it or nothing.
pub(super) fn add_input(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        name: String,
        peg: Option<u64>,
    }
    let id = "driver.addInput";
    let Params { driver, name, peg } = parse(id, params)?;
    let name = checked_word(&name)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, driver)?;
        let held = driver_in(shot, driver)?;
        word_is_free(held, name)?;
        let peg = peg.map(|peg| wirable(shot, held.drives, peg)).transpose()?;
        change(p, driver, |driver| {
            driver.inputs.push(DriverInput { name: name.to_string(), peg });
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Wires `peg` into a driver's input `input`, or with `peg` null or left out, nothing.
pub(super) fn connect(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        input: String,
        peg: Option<u64>,
    }
    let id = "driver.connect";
    let Params { driver, input, peg } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, driver)?;
        let held = driver_in(shot, driver)?;
        if held.input(&input).is_none() {
            return invalid(format!("`{}` has no input `{input}`", held.name));
        }
        let peg = peg.map(|peg| wirable(shot, held.drives, peg)).transpose()?;
        change(p, driver, |driver| {
            if let Some(held) = driver.inputs.iter_mut().find(|held| held.name == input) {
                held.peg = peg;
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Takes an input away from a driver, which its expressions must not read.
pub(super) fn remove_input(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        input: String,
    }
    let id = "driver.removeInput";
    let Params { driver, input } = parse(id, params)?;
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            if driver.input(&input).is_none() {
                return invalid(format!("`{}` has no input `{input}`", driver.name));
            }
            driver.inputs.retain(|held| held.name != input);
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Renames a driver's input, and the input wherever its expressions read it.
pub(super) fn rename_input(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        input: String,
        name: String,
    }
    let id = "driver.renameInput";
    let Params { driver, input, name } = parse(id, params)?;
    let name = checked_word(&name)?;
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            if driver.input(&input).is_none() {
                return invalid(format!("`{}` has no input `{input}`", driver.name));
            }
            if name == input {
                return Ok(());
            }
            word_is_free(driver, name)?;
            driver.expressions = driver.expressions.renamed(&input, name, true).or_else(|why| invalid(why.to_string()))?;
            if let Some(held) = driver.inputs.iter_mut().find(|held| held.name == input) {
                held.name = name.to_string();
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Adds a parameter to a driver, named `name`, at `value` (0 if it is left out).
pub(super) fn add_parameter(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        name: String,
        #[serde(default)]
        value: f64,
    }
    let id = "driver.addParameter";
    let Params { driver, name, value } = parse(id, params)?;
    let name = checked_word(&name)?;
    if !value.is_finite() {
        return invalid(format!("`{name}` is a number"));
    }
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            word_is_free(driver, name)?;
            driver.parameters.push(DriverParameter { name: name.to_string(), value, track: Track::default() });
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Takes a parameter away from a driver, which its expressions must not read.
pub(super) fn remove_parameter(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        parameter: String,
    }
    let id = "driver.removeParameter";
    let Params { driver, parameter } = parse(id, params)?;
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            if driver.parameter(&parameter).is_none() {
                return invalid(format!("`{}` has no parameter `{parameter}`", driver.name));
            }
            driver.parameters.retain(|held| held.name != parameter);
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Renames a driver's parameter, and the parameter wherever its expressions read it.
pub(super) fn rename_parameter(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        parameter: String,
        name: String,
    }
    let id = "driver.renameParameter";
    let Params { driver, parameter, name } = parse(id, params)?;
    let name = checked_word(&name)?;
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            if driver.parameter(&parameter).is_none() {
                return invalid(format!("`{}` has no parameter `{parameter}`", driver.name));
            }
            if name == parameter {
                return Ok(());
            }
            word_is_free(driver, name)?;
            driver.expressions = driver.expressions.renamed(&parameter, name, false).or_else(|why| invalid(why.to_string()))?;
            if let Some(held) = driver.parameters.iter_mut().find(|held| held.name == parameter) {
                held.name = name.to_string();
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// The parameters a command about keyframes is about: the one it names, or all of them.
fn named<'a>(driver: &'a mut Driver, parameter: Option<&str>) -> Result<Vec<&'a mut DriverParameter>> {
    match parameter {
        Some(name) => match driver.parameters.iter_mut().find(|held| held.name == name) {
            Some(held) => Ok(vec![held]),
            None => invalid(format!("`{}` has no parameter `{name}`", driver.name)),
        },
        None => Ok(driver.parameters.iter_mut().collect()),
    }
}

/// Sets a driver's parameter to `value`: on `frame`, which keys it there, for one that has
/// keys; else on every frame.
pub(super) fn set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        parameter: String,
        value: f64,
        frame: Option<u32>,
        #[serde(default)]
        adjust: Vec<u32>,
    }
    let id = "driver.set";
    let Params { driver, parameter, value, frame, adjust } = parse(id, params)?;
    if !value.is_finite() {
        return invalid(format!("`{parameter}` is a number"));
    }
    if let Some(frame) = frame {
        checked_frames(frame, 1)?;
    }
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            let name = driver.name.clone();
            let Some(held) = driver.parameters.iter_mut().find(|held| held.name == parameter) else {
                return invalid(format!("`{name}` has no parameter `{parameter}`"));
            };
            match (held.track.is_empty(), frame) {
                (false, Some(frame)) => held.track.set_adjusting(frame, value, false, &adjust),
                (false, None) => return invalid(format!("`{parameter}` has keyframes: say on which `frame` it is set")),
                (true, _) => held.value = value,
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Keys a driver's `parameter` on `frame` as it is there, or without one all its parameters.
pub(super) fn set_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        parameter: Option<String>,
        frame: u32,
    }
    let id = "driver.setKey";
    let Params { driver, parameter, frame } = parse(id, params)?;
    checked_frames(frame, 1)?;
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            for held in named(driver, parameter.as_deref())? {
                let now = held.at(frame);
                held.track.set(frame, now);
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Takes the keyframes on `frame` away, of `parameter` or of all. A parameter that loses its
/// last key stays as that key had it.
pub(super) fn remove_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        parameter: Option<String>,
        frame: u32,
    }
    let id = "driver.removeKey";
    let Params { driver, parameter, frame } = parse(id, params)?;
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            let name = driver.name.clone();
            let mut found = false;
            for held in named(driver, parameter.as_deref())? {
                if let Some(key) = held.track.remove(frame) {
                    found = true;
                    if held.track.is_empty() {
                        held.value = key.value;
                    }
                }
            }
            if !found {
                return invalid(format!("`{name}` has no keyframe on frame {frame}"));
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Moves the keyframes on `from` to `to`, of `parameter` or of all. Keys on `to` give way.
pub(super) fn move_key(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        parameter: Option<String>,
        from: u32,
        to: u32,
    }
    let id = "driver.moveKey";
    let Params { driver, parameter, from, to } = parse(id, params)?;
    checked_frames(to, 1)?;
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            let name = driver.name.clone();
            let mut found = false;
            for held in named(driver, parameter.as_deref())? {
                if held.track.key_mut(from).is_some() {
                    found = true;
                    held.track.move_key(from, to);
                }
            }
            if !found {
                return invalid(format!("`{name}` has no keyframe on frame {from}"));
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Sets how the keyframes on `frame` ease, `into` them, `out` of them or both, of
/// `parameter` or of all, as a peg's do.
pub(super) fn set_ease(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        parameter: Option<String>,
        frame: u32,
        into: Option<Interp>,
        out: Option<Interp>,
    }
    let id = "driver.setEase";
    let Params { driver, parameter, frame, into, out } = parse(id, params)?;
    if into == Some(Interp::Hold) {
        return invalid("a value is held out of a keyframe, not into one");
    }
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            let name = driver.name.clone();
            let mut found = false;
            for held in named(driver, parameter.as_deref())? {
                if let Some(key) = held.track.key_mut(frame) {
                    found = true;
                    (key.into, key.out) = (into.unwrap_or(key.into), out.unwrap_or(key.out));
                }
            }
            if !found {
                return invalid(format!("`{name}` has no keyframe on frame {frame}"));
            }
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Switches a driver on or off. Off, it adds nothing to its peg, in the window and in a
/// render alike.
pub(super) fn set_enabled(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        enabled: bool,
    }
    let id = "driver.setEnabled";
    let Params { driver, enabled } = parse(id, params)?;
    s.edit(title(id), |p| {
        change(p, driver, |driver| {
            driver.off = !enabled;
            Ok(())
        })
    })?;
    Ok(Value::Null)
}

/// Bakes a driver into its peg: on every frame from `from` to `to` (both counted, the whole
/// shot where they are left out) each value it gives becomes a key on the peg, at what the
/// peg comes to there with the driver, and the driver goes. Outside those frames the peg is
/// as its keys make it, the baked ones among them. Returns how many frames were keyed.
pub(super) fn bake(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        driver: u64,
        from: Option<u32>,
        to: Option<u32>,
    }
    let id = "driver.bake";
    let Params { driver, from, to } = parse(id, params)?;
    let keyed = s.edit(title(id), |p| {
        let shot = shot_of(p, driver)?;
        let held = driver_in(shot, driver)?;
        let Some(peg) = shot.peg(held.drives) else { return invalid(format!("the peg `{}` drives is gone", held.name)) };
        let (from, to) = (from.unwrap_or(0), to.unwrap_or(shot.duration.saturating_sub(1)));
        if from > to {
            return invalid(format!("frame {from} is after frame {to}"));
        }
        checked_frames(from, to - from + 1)?;
        let gives = held.expressions.gives();
        let poses: Vec<(u32, iskra_model::Transform)> = (from..=to).map(|frame| (frame, shot.pose(peg, frame))).collect();
        let (within, peg) = (shot.id, peg.id);
        let Some(shot) = p.shot_mut(within) else { return Ok(0) };
        shot.drivers.retain(|other| other.id != DriverId(driver));
        shot.places.remove(&driver);
        shot.colours.remove(&driver);
        let given = |values: &[Driven]| values.iter().any(|value| gives.contains(value));
        let Some(held) = shot.pegs.iter_mut().find(|held| held.id == peg) else { return Ok(0) };
        let animation = &mut Arc::make_mut(held).animation;
        for (frame, pose) in &poses {
            if given(&[Driven::X, Driven::Y]) {
                animation.position.set(*frame, pose.position);
            }
            if given(&[Driven::Rotation]) {
                animation.rotation.set(*frame, pose.rotation);
            }
            if given(&[Driven::ScaleX, Driven::ScaleY]) {
                animation.scale.set(*frame, pose.scale);
            }
            if given(&[Driven::Skew]) {
                animation.skew.set(*frame, pose.skew);
            }
            if given(&[Driven::Depth]) {
                animation.depth.set(*frame, pose.depth);
            }
        }
        Ok(poses.len())
    })?;
    Ok(json!({ "frames": keyed }))
}
