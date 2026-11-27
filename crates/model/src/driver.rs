//! Driver nodes (plan §8.8): a peg's values worked out on every frame by expressions over
//! other pegs, the driver's own parameters and the frame, and added to what its own keys give.
//!
//! What a peg is on a frame is [`Shot::pose`]: its own values, with its driver's amounts
//! added. Everything that places what a peg carries reads that; what edits a peg's keys reads
//! its own values, [`crate::Peg::transform_at`], which the driver adds to.

use std::cell::Cell;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{Amounts, Driven, DriverId, DriverRead, DriverScope, Expressions, Peg, PegId, Point, Shot, Track, Transform};

/// A Driver node: expressions whose amounts are added to the values of the peg it drives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Driver {
    pub id: DriverId,
    /// Unique among the shot's nodes, as every node's name is.
    pub name: String,
    /// The peg whose values it adds to. A peg has one driver at most.
    pub drives: PegId,
    /// The pegs wired into it, each under the name the expressions call it by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<DriverInput>,
    #[serde(default)]
    pub expressions: Expressions,
    /// The preset its expressions were taken from, if they were, by its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<DriverParameter>,
    /// Switched off, it adds nothing (plan §8.2 line 5).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub off: bool,
    /// The library template it is a copy from, if it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

/// A peg wired into a driver, under a name of the driver's own: the port's, not the peg's, so
/// that expressions keep working on a rig whose pegs are named otherwise (plan §4.12).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverInput {
    pub name: String,
    /// `None` for a port with nothing wired into it: what is read of it is no number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peg: Option<PegId>,
}

/// A number of a driver's, which its expressions read by name and which can be keyed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverParameter {
    pub name: String,
    /// On every frame, where it has no keys.
    pub value: f64,
    #[serde(default, skip_serializing_if = "Track::is_empty")]
    pub track: Track<f64>,
}

impl DriverParameter {
    pub fn at(&self, frame: u32) -> f64 {
        self.track.at(frame, false).unwrap_or(self.value)
    }
}

impl Driver {
    /// A driver of `drives` with no expressions, which adds nothing yet.
    pub fn new(id: DriverId, name: &str, drives: PegId) -> Driver {
        Driver {
            id,
            name: name.to_string(),
            drives,
            inputs: Vec::new(),
            expressions: Expressions::default(),
            preset: None,
            parameters: Vec::new(),
            off: false,
            template: None,
        }
    }

    pub fn input(&self, name: &str) -> Option<&DriverInput> {
        self.inputs.iter().find(|input| input.name == name)
    }

    pub fn parameter(&self, name: &str) -> Option<&DriverParameter> {
        self.parameters.iter().find(|parameter| parameter.name == name)
    }

    /// The same driver with its parameters' keys `by` frames later.
    pub fn shifted(&self, by: u32) -> Driver {
        let mut later = self.clone();
        for parameter in &mut later.parameters {
            parameter.track = parameter.track.shifted(by);
        }
        later
    }

    /// The names its expressions read that it has no input or parameter for.
    pub fn missing(&self) -> Vec<String> {
        let inputs = self.expressions.inputs().iter().filter(|name| self.input(name).is_none());
        let parameters = self.expressions.parameters().iter().filter(|name| self.parameter(name).is_none());
        inputs.chain(parameters).cloned().collect()
    }
}

/// A driver's expressions with the inputs and parameters they read, to be put on a driver:
/// one of the presets that come with Iskra, or one the user saved, as a file in the *Drivers*
/// folder of their Scripts folder (plan §8.8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverPreset {
    pub name: String,
    /// What it does, in a line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub expressions: String,
    /// The inputs it reads, by name, which a driver it is put on is given, empty, where it has
    /// none of that name: a peg is wired into each in the node view or Properties.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    /// Its parameters, with the values they start at.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<PresetParameter>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresetParameter {
    pub name: String,
    pub value: f64,
}

/// The presets that come with Iskra (plan §8.8), in the order they are listed.
pub fn driver_presets() -> Vec<DriverPreset> {
    let preset = |name: &str, hint: &str, expressions: &str, inputs: &[&str], parameters: &[(&str, f64)]| DriverPreset {
        name: name.to_string(),
        hint: hint.to_string(),
        expressions: expressions.to_string(),
        inputs: inputs.iter().map(|input| input.to_string()).collect(),
        parameters: parameters.iter().map(|(name, value)| PresetParameter { name: name.to_string(), value: *value }).collect(),
    };
    vec![
        preset(
            "Roll with Parent",
            "A wheel under its car: it turns as far as the car goes, clockwise going right, back going left, up a slope too",
            "// A wheel under its car: how far the car has gone, round a wheel of this radius.\nrotation = parent.travelled / radius * 180 / PI",
            &[],
            &[("radius", 50.0)],
        ),
        preset(
            "Shadow from Height",
            "A shadow on the ground under a character: it follows it across and shrinks as it goes up",
            "// How high the character is over the ground, from none to all of `height`.\nlet up = clamp(abs(char.world.y - ground) / height, 0, 1)\n// Under it, across.\nx = char.world.x - own.x\nscaleX = up * shrink\nscaleY = up * shrink",
            &["char"],
            &[("ground", 0.0), ("height", 300.0), ("shrink", -0.6)],
        ),
        preset(
            "Follow with Lag",
            "Goes where another peg went some frames before, and turns as it turned",
            "x = at(target.x, frame - lag) - own.x\ny = at(target.y, frame - lag) - own.y\nrotation = at(target.rotation, frame - lag) - own.rotation",
            &["target"],
            &[("lag", 4.0)],
        ),
        preset(
            "Wiggle",
            "A smooth random move about the peg's own place and turn, the same on every play",
            "x = noise(frame * speed, seed) * amount\ny = noise(frame * speed, seed + 1) * amount\nrotation = noise(frame * speed, seed + 2) * amount / 4",
            &[],
            &[("amount", 10.0), ("speed", 0.1), ("seed", 1.0)],
        ),
        preset(
            "Bounce",
            "Goes after another peg as a spring would: it overshoots and settles",
            "x = spring(target.x, stiffness, damping) - own.x\ny = spring(target.y, stiffness, damping) - own.y",
            &["target"],
            &[("stiffness", 0.2), ("damping", 0.25)],
        ),
        preset(
            "Look At",
            "Turns the peg towards another, wherever its parent moves and turns it",
            "// Where the target is from the peg, in the scene.\nlet across = target.world.x - own.world.x\nlet down = target.world.y - own.world.y\n// That way, less the turn the parent and the peg's own keys give it.\nrotation = deg(atan2(down, across)) - parent.world.rotation - own.rotation",
            &["target"],
            &[],
        ),
    ]
}

/// A value of a transform, as expressions name it.
fn value_of(transform: &Transform, driven: Driven) -> f64 {
    match driven {
        Driven::X => transform.position[0],
        Driven::Y => transform.position[1],
        Driven::Rotation => transform.rotation,
        Driven::ScaleX => transform.scale[0],
        Driven::ScaleY => transform.scale[1],
        Driven::Skew => transform.skew,
        Driven::Depth => transform.depth,
    }
}

fn value_mut(transform: &mut Transform, driven: Driven) -> &mut f64 {
    match driven {
        Driven::X => &mut transform.position[0],
        Driven::Y => &mut transform.position[1],
        Driven::Rotation => &mut transform.rotation,
        Driven::ScaleX => &mut transform.scale[0],
        Driven::ScaleY => &mut transform.scale[1],
        Driven::Skew => &mut transform.skew,
        Driven::Depth => &mut transform.depth,
    }
}

thread_local! {
    /// How many drivers are being worked out inside one another on this thread.
    static DRIVING: Cell<u32> = const { Cell::new(0) };
}

/// How deep drivers may read driven pegs that read driven pegs. The rule against loops keeps a
/// project from going round; this keeps bad data from going round for ever.
const DEEPEST: u32 = 16;

/// One driver more is being worked out, until this is dropped.
struct Driving;

impl Driving {
    fn enter() -> Option<Driving> {
        DRIVING.with(|driving| {
            let depth = driving.get();
            (depth < DEEPEST).then(|| {
                driving.set(depth + 1);
                Driving
            })
        })
    }
}

impl Drop for Driving {
    fn drop(&mut self) {
        DRIVING.with(|driving| driving.set(driving.get().saturating_sub(1)));
    }
}

/// Where `pivot` of a peg's own space is in the scene, by `world`, the peg's own space to the
/// scene's, and how its space is turned and scaled there.
fn in_scene(world: crate::Affine, pivot: [f64; 2], read: DriverRead) -> f64 {
    let [a, b, ..] = world.m;
    match read {
        DriverRead::WorldX | DriverRead::WorldY => {
            let at = world.apply(Point::new(pivot[0], pivot[1]));
            if read == DriverRead::WorldX { at.x } else { at.y }
        }
        DriverRead::WorldRotation => b.atan2(a).to_degrees(),
        DriverRead::WorldScale => a.hypot(b),
        // Its own values are read without this.
        DriverRead::Own(_) => f64::NAN,
    }
}

/// What a driver's expressions read of the shot.
struct Reading<'a> {
    shot: &'a Shot,
    driver: &'a Driver,
    peg: &'a Peg,
}

impl Reading<'_> {
    fn peg(&self, input: &str) -> Option<&Peg> {
        match input {
            "parent" => self.shot.peg_above(self.peg.parent),
            name => self.shot.peg(self.driver.input(name)?.peg?),
        }
    }
}

impl DriverScope for Reading<'_> {
    fn read(&self, input: &str, read: DriverRead, frame: u32) -> Option<f64> {
        // Of the driven peg, its own values, and where they put it in the scene: as it would be
        // without its driver, which can't read what it adds itself.
        if input == "own" {
            let own = self.peg.transform_at(frame);
            let world = self.shot.world(self.peg.parent, frame).mul(&own.matrix());
            return Some(match read {
                DriverRead::Own(driven) => value_of(&own, driven),
                read => in_scene(world, own.pivot, read),
            });
        }
        // A peg with no parent is in the scene itself, which is nowhere moved, turned or scaled.
        if input == "parent" && self.peg.parent.is_none() {
            return Some(match read {
                DriverRead::Own(Driven::ScaleX | Driven::ScaleY) | DriverRead::WorldScale => 1.0,
                _ => 0.0,
            });
        }
        let peg = self.peg(input)?;
        if let DriverRead::Own(driven) = read {
            return Some(value_of(&self.shot.pose(peg, frame), driven));
        }
        Some(in_scene(self.shot.world(Some(peg.id), frame), self.shot.pose(peg, frame).pivot, read))
    }

    fn parameter(&self, name: &str, frame: u32) -> Option<f64> {
        self.driver.parameter(name).map(|parameter| parameter.at(frame))
    }

    /// Of everything the expressions read: the driver, its peg, and every peg that moves its
    /// parent or an input, with their drivers and the bones they follow.
    fn fingerprint(&self) -> Option<u64> {
        use std::fmt::Write;
        use std::hash::Hasher;
        /// What is written into it is hashed, with nothing kept.
        struct Hashing(std::collections::hash_map::DefaultHasher);
        impl Write for Hashing {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                self.0.write(text.as_bytes());
                Ok(())
            }
        }
        let mut hashing = Hashing(Default::default());
        let driver = |hashing: &mut Hashing, driver: &Driver| {
            let _ = write!(hashing, "{:?}{}{:?}{:?}{}", driver.id, driver.expressions.text(), driver.inputs, driver.parameters, driver.off);
        };
        driver(&mut hashing, self.driver);
        let _ = write!(hashing, "{:?}", self.peg);
        let read = std::iter::once(self.peg.parent).chain(self.driver.inputs.iter().map(|input| input.peg)).flatten();
        let mut moving: Vec<PegId> = read.flat_map(|peg| self.shot.moved_by(peg)).collect();
        moving.sort();
        moving.dedup();
        for peg in moving.iter().filter_map(|id| self.shot.peg(*id)) {
            let _ = write!(hashing, "{peg:?}");
            if let Some(other) = self.shot.driver_of(peg.id) {
                driver(&mut hashing, other);
            }
            if let Some(bones) = peg.follows.and_then(|follow| self.shot.deformer(follow.deformer)) {
                let _ = write!(hashing, "{bones:?}");
            }
        }
        Some(hashing.0.finish())
    }
}

impl Shot {
    pub fn driver(&self, id: DriverId) -> Option<&Driver> {
        self.drivers.iter().find(|driver| driver.id == id).map(|driver| &**driver)
    }

    /// The driver of `peg`, switched on or off.
    pub fn driver_of(&self, peg: PegId) -> Option<&Driver> {
        self.drivers.iter().find(|driver| driver.drives == peg).map(|driver| &**driver)
    }

    /// What `driver`'s expressions give on `frame`: the amounts added to its peg's values.
    /// Nothing for one that is off, or whose peg is gone.
    pub fn amounts(&self, driver: &Driver, frame: u32) -> Amounts {
        let Some(peg) = self.peg(driver.drives).filter(|_| !driver.off) else { return Amounts::default() };
        let Some(_driving) = Driving::enter() else { return Amounts { failed: true, ..Amounts::default() } };
        driver.expressions.amounts(&Reading { shot: self, driver, peg }, frame)
    }

    /// The peg's values on `frame`: its own, keyed or still, with what its driver adds.
    pub fn pose(&self, peg: &Peg, frame: u32) -> Transform {
        let mut pose = peg.transform_at(frame);
        // A shot with no drivers, which is most, does no more than that.
        if self.drivers.is_empty() {
            return pose;
        }
        let Some(driver) = self.driver_of(peg.id).filter(|driver| !driver.off) else { return pose };
        let amounts = self.amounts(driver, frame);
        for driven in Driven::ALL {
            if let Some(amount) = amounts.of(driven) {
                *value_mut(&mut pose, driven) += amount;
            }
        }
        pose
    }

    /// The pegs whose values move where `peg` is: itself, the pegs above it, and what their
    /// drivers read, and so on.
    pub fn moved_by(&self, peg: PegId) -> HashSet<PegId> {
        let (mut moved, mut open) = (HashSet::new(), vec![peg]);
        while let Some(at) = open.pop() {
            if !moved.insert(at) {
                continue;
            }
            let Some(peg) = self.peg(at) else { continue };
            open.extend(self.peg_above(peg.parent).map(|parent| parent.id));
            if let Some(driver) = self.driver_of(at) {
                open.extend(driver.inputs.iter().filter_map(|input| input.peg));
            }
        }
        moved
    }

    /// Whether wiring `input` into the driver of `driven` would have `driven` move itself:
    /// `input` is it, is under it, or is moved by a driver that reads it (plan §8.2 line 8).
    pub fn driver_loops(&self, driven: PegId, input: PegId) -> bool {
        self.moved_by(input).contains(&driven)
    }

    /// A driver whose peg moves itself: through a peg above it or an input that it moves, or
    /// one moved by a driver that reads it. What a command that rewires pegs refuses to make.
    pub fn driver_in_a_loop(&self) -> Option<&Driver> {
        self.drivers.iter().map(|driver| &**driver).find(|driver| {
            let Some(peg) = self.peg(driver.drives) else { return false };
            let read = self.peg_above(peg.parent).map(|parent| parent.id).into_iter().chain(driver.inputs.iter().filter_map(|input| input.peg));
            read.into_iter().any(|read| self.moved_by(read).contains(&driver.drives))
        })
    }

    /// Let go of `peg` in the drivers: its own driver goes, and a port it was wired into is
    /// empty. For a peg that is deleted.
    pub fn drivers_forget(&mut self, peg: PegId) {
        self.drivers.retain(|driver| driver.drives != peg);
        for driver in &mut self.drivers {
            if driver.inputs.iter().any(|input| input.peg == Some(peg)) {
                let driver = std::sync::Arc::make_mut(driver);
                for input in driver.inputs.iter_mut().filter(|input| input.peg == Some(peg)) {
                    input.peg = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::Project;

    fn peg_of(id: PegId, name: &str, parent: Option<PegId>) -> Peg {
        Peg {
            id,
            name: name.to_string(),
            parent,
            transform: Transform::default(),
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }
    }

    /// A shot with a car peg and a wheel peg under it, the car keyed from 0 to `to` over ten
    /// frames.
    fn car_and_wheel(to: [f64; 2]) -> (Project, PegId, PegId) {
        let mut project = Project::default();
        let (car, wheel) = (PegId(project.new_id()), PegId(project.new_id()));
        let mut car_peg = peg_of(car, "Car-P", None);
        car_peg.animation.position.set(0, [0.0, 0.0]);
        car_peg.animation.position.set(10, to);
        let wheel_peg = peg_of(wheel, "Wheel-P", Some(car));
        Arc::make_mut(&mut project.shots[0]).pegs.extend([Arc::new(car_peg), Arc::new(wheel_peg)]);
        (project, car, wheel)
    }

    fn drive(project: &mut Project, peg: PegId, text: &str, inputs: &[(&str, PegId)], parameters: &[(&str, f64)]) -> DriverId {
        let id = DriverId(project.new_id());
        let mut driver = Driver::new(id, "Driver", peg);
        driver.expressions = Expressions::parse(text).unwrap();
        driver.inputs = inputs.iter().map(|(name, peg)| DriverInput { name: name.to_string(), peg: Some(*peg) }).collect();
        driver.parameters = parameters.iter().map(|(name, value)| DriverParameter { name: name.to_string(), value: *value, track: Track::default() }).collect();
        assert!(driver.missing().is_empty(), "{:?}", driver.missing());
        Arc::make_mut(&mut project.shots[0]).drivers.push(Arc::new(driver));
        id
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn a_wheel_turns_as_its_car_is_keyed() {
        let (mut project, _, wheel) = car_and_wheel([100.0, 0.0]);
        drive(&mut project, wheel, "rotation = parent.travelled / radius * 180 / PI", &[], &[("radius", 50.0)]);
        let shot = &project.shots[0];
        let turned = |frame| shot.pose(shot.peg(wheel).unwrap(), frame).rotation;
        assert!(close(turned(0), 0.0));
        assert!(close(turned(10), 2f64.to_degrees()), "{}", turned(10));
        // What the wheel carries is turned with it: a point on its rim has gone round by
        // two radians.
        let world = shot.world(Some(wheel), 10);
        let rim = world.apply(Point::new(50.0, 0.0));
        let turn = 2f64;
        assert!(close(rim.x, 100.0 + 50.0 * turn.cos()) && close(rim.y, 50.0 * turn.sin()), "{rim:?}");
    }

    #[test]
    fn up_a_slope_a_wheel_turns_by_the_way_gone() {
        let (mut project, _, wheel) = car_and_wheel([60.0, -80.0]);
        drive(&mut project, wheel, "rotation = parent.travelled / radius * 180 / PI", &[], &[("radius", 50.0)]);
        let shot = &project.shots[0];
        assert!(close(shot.pose(shot.peg(wheel).unwrap(), 10).rotation, 2f64.to_degrees()));
    }

    #[test]
    fn a_shadow_shrinks_as_the_character_jumps_and_follows_it_along_the_ground() {
        let mut project = Project::default();
        let (char, shadow) = (PegId(project.new_id()), PegId(project.new_id()));
        let mut char_peg = peg_of(char, "Char-P", None);
        char_peg.animation.position.set(0, [0.0, 0.0]);
        char_peg.animation.position.set(10, [200.0, -300.0]);
        let mut shadow_peg = peg_of(shadow, "Shadow-P", None);
        shadow_peg.transform.position = [0.0, 20.0];
        Arc::make_mut(&mut project.shots[0]).pegs.extend([Arc::new(char_peg), Arc::new(shadow_peg)]);
        let text =
            "let height = abs(char.world.y - ground)\nx = char.world.x - own.x\nscaleX = remap(height, 0, 300, 0, -0.6)\nscaleY = scaleX_of_height(height)";
        assert!(Expressions::parse(text).is_err(), "a function that is not there is refused");
        let text = "let height = abs(char.world.y - ground)\nx = char.world.x - own.x\nscaleX = remap(height, 0, 300, 0, -0.6)\nscaleY = remap(height, 0, 300, 0, -0.6)";
        drive(&mut project, shadow, text, &[("char", char)], &[("ground", 0.0)]);
        let shot = &project.shots[0];
        let pose = |frame| shot.pose(shot.peg(shadow).unwrap(), frame);
        assert_eq!(pose(0).scale, [1.0, 1.0]);
        assert!(close(pose(5).scale[0], 0.7) && close(pose(5).scale[1], 0.7), "{:?}", pose(5).scale);
        assert!(close(pose(10).scale[0], 0.4));
        // Along the ground under the character, at the shadow's own height.
        assert!(close(pose(5).position[0], 100.0) && close(pose(5).position[1], 20.0), "{:?}", pose(5).position);
    }

    #[test]
    fn what_a_driven_peg_carries_moves_with_it_everywhere_it_is_placed() {
        let (mut project, car, wheel) = car_and_wheel([100.0, 0.0]);
        // A peg under the wheel, and the world it gives what it carries.
        let hub = PegId(project.new_id());
        let hub_peg = peg_of(hub, "Hub-P", Some(wheel));
        Arc::make_mut(&mut project.shots[0]).pegs.push(Arc::new(hub_peg));
        drive(&mut project, wheel, "y = 7; depth = 3", &[], &[]);
        let shot = &project.shots[0];
        let at = shot.world(Some(hub), 10).apply(Point::new(0.0, 0.0));
        assert!(close(at.x, 100.0) && close(at.y, 7.0), "{at:?}");
        assert!(close(shot.depth(Some(hub), 10), 3.0));
        assert!(close(shot.space(Some(hub), 10).offset[2], 3.0));
        // The car's own values are not driven, and nor are the wheel's keys.
        assert_eq!(shot.pose(shot.peg(car).unwrap(), 10).position, [100.0, 0.0]);
        assert_eq!(shot.peg(wheel).unwrap().transform_at(10).position, [0.0, 0.0]);
    }

    #[test]
    fn a_driver_that_is_off_or_fails_adds_nothing() {
        let (mut project, _, wheel) = car_and_wheel([100.0, 0.0]);
        let id = drive(&mut project, wheel, "x = 5\ny = 1 / 0", &[], &[]);
        let shot = &project.shots[0];
        assert_eq!(shot.pose(shot.peg(wheel).unwrap(), 3).position, [5.0, 0.0]);
        assert!(shot.amounts(shot.driver(id).unwrap(), 3).failed);
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.drivers.iter_mut().for_each(|driver| Arc::make_mut(driver).off = true);
        assert_eq!(shot.pose(shot.peg(wheel).unwrap(), 3).position, [0.0, 0.0]);
    }

    #[test]
    fn a_keyed_parameter_changes_over_frames() {
        let (mut project, _, wheel) = car_and_wheel([0.0, 0.0]);
        let id = drive(&mut project, wheel, "rotation = amount", &[], &[("amount", 0.0)]);
        let shot = Arc::make_mut(&mut project.shots[0]);
        let driver = Arc::make_mut(shot.drivers.iter_mut().find(|driver| driver.id == id).unwrap());
        driver.parameters[0].track.set(0, 0.0);
        driver.parameters[0].track.set(10, 90.0);
        assert!(close(shot.pose(shot.peg(wheel).unwrap(), 5).rotation, 45.0));
    }

    #[test]
    fn a_driver_can_read_a_driven_peg() {
        let (mut project, car, wheel) = car_and_wheel([100.0, 0.0]);
        let other = PegId(project.new_id());
        let other_peg = peg_of(other, "Other-P", None);
        Arc::make_mut(&mut project.shots[0]).pegs.push(Arc::new(other_peg));
        drive(&mut project, car, "y = 10", &[], &[]);
        drive(&mut project, other, "y = car.y * 2; x = car.world.x", &[("car", car)], &[]);
        let _ = wheel;
        let shot = &project.shots[0];
        assert_eq!(shot.pose(shot.peg(other).unwrap(), 10).position, [100.0, 20.0]);
    }

    #[test]
    fn a_wire_that_would_loop_is_found() {
        let (mut project, car, wheel) = car_and_wheel([100.0, 0.0]);
        let other = PegId(project.new_id());
        let other_peg = peg_of(other, "Other-P", None);
        Arc::make_mut(&mut project.shots[0]).pegs.push(Arc::new(other_peg));
        let shot = &project.shots[0];
        assert!(shot.driver_loops(car, car), "itself");
        assert!(shot.driver_loops(car, wheel), "what is under it");
        assert!(!shot.driver_loops(wheel, car), "what is above it");
        assert!(!shot.driver_loops(car, other));
        drive(&mut project, other, "x = car.x", &[("car", car)], &[]);
        let shot = &project.shots[0];
        assert!(shot.driver_loops(car, other), "what reads it");
        assert!(!shot.driver_loops(wheel, other));
    }

    #[test]
    fn a_loop_in_bad_data_ends_rather_than_going_round() {
        let (mut project, car, _) = car_and_wheel([100.0, 0.0]);
        drive(&mut project, car, "x = me.x + 1", &[("me", car)], &[]);
        let shot = &project.shots[0];
        // It went round as deep as it may, each time adding the car's own 100 and one, and
        // stopped there with the car's own value.
        let x = shot.pose(shot.peg(car).unwrap(), 10).position[0];
        assert_eq!(x, 100.0 + 101.0 * f64::from(DEEPEST));
    }

    #[test]
    fn a_peg_deleted_is_let_go_by_the_drivers() {
        let (mut project, car, wheel) = car_and_wheel([100.0, 0.0]);
        let other = PegId(project.new_id());
        let other_peg = peg_of(other, "Other-P", None);
        Arc::make_mut(&mut project.shots[0]).pegs.push(Arc::new(other_peg));
        drive(&mut project, wheel, "x = 1", &[], &[]);
        drive(&mut project, other, "x = car.x", &[("car", car)], &[]);
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.drivers_forget(wheel);
        assert!(shot.driver_of(wheel).is_none());
        shot.drivers_forget(car);
        assert_eq!(shot.driver_of(other).unwrap().inputs[0].peg, None);
        assert!(shot.amounts(shot.driver_of(other).unwrap(), 3).failed);
    }

    #[test]
    fn a_driver_is_saved_and_read_back() {
        let (mut project, car, wheel) = car_and_wheel([100.0, 0.0]);
        drive(&mut project, wheel, "rotation = car.x * k", &[("car", car)], &[("k", 2.0)]);
        let shot = &project.shots[0];
        let json = serde_json::to_string(&**shot).unwrap();
        let back: Shot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.drivers, shot.drivers);
        assert_eq!(back.pose(back.peg(wheel).unwrap(), 10).rotation, 200.0);
    }

    #[test]
    fn a_copied_shot_has_its_own_drivers_of_its_own_pegs() {
        let (mut project, car, wheel) = car_and_wheel([100.0, 0.0]);
        drive(&mut project, wheel, "rotation = car.x", &[("car", car)], &[]);
        let first = project.shots[0].id;
        let copy = project.duplicate_shot(first, "Copy").unwrap();
        let driver = &copy.drivers[0];
        let (car_copy, wheel_copy) = (copy.pegs[0].id, copy.pegs[1].id);
        assert_ne!(driver.id, project.shots[0].drivers[0].id);
        assert_eq!((driver.drives, driver.inputs[0].peg), (wheel_copy, Some(car_copy)));
        assert_eq!(copy.pose(copy.peg(wheel_copy).unwrap(), 10).rotation, 100.0);
    }

    #[test]
    fn a_driver_goes_into_a_template_with_its_peg_and_comes_out_with_a_name_of_its_own() {
        let (mut project, car, wheel) = car_and_wheel([100.0, 0.0]);
        let outside = PegId(project.new_id());
        Arc::make_mut(&mut project.shots[0]).pegs.push(Arc::new(peg_of(outside, "Outside-P", None)));
        drive(&mut project, wheel, "rotation = car.x + far.x", &[("car", car), ("far", outside)], &[]);
        let shot = project.shots[0].id;
        let template = project.template(shot, &[crate::Part::Peg(car)], "Car").unwrap();
        let held = &template.shots[0].drivers;
        assert_eq!(held.len(), 1, "it goes with the wheel, which goes with the car");
        assert_eq!(held[0].input("car").unwrap().peg, Some(car));
        assert_eq!(held[0].input("far").unwrap().peg, None, "a peg that stays behind is let go");
        // Without its peg it stays behind.
        assert!(project.template(shot, &[crate::Part::Peg(outside)], "Far").unwrap().shots[0].drivers.is_empty());

        let placed = project.place_template(shot, &template, "Car", None).unwrap();
        let shot = &project.shots[0];
        let made = shot.driver(placed.drivers[0]).unwrap();
        assert_eq!(made.name, "Driver 2");
        assert_eq!(made.template.as_deref(), Some("Car"));
        assert!(placed.pegs.contains(&made.drives) && made.drives != wheel);
        assert!(placed.pegs.contains(&made.input("car").unwrap().peg.unwrap()));
        assert!(shot.node_ids().any(|id| id == made.id.0));
    }

    #[test]
    fn a_shot_made_later_has_its_parameters_keys_later() {
        let (mut project, _, wheel) = car_and_wheel([0.0, 0.0]);
        let id = drive(&mut project, wheel, "rotation = amount", &[], &[("amount", 0.0)]);
        let shot = Arc::make_mut(&mut project.shots[0]);
        let driver = Arc::make_mut(shot.drivers.iter_mut().find(|driver| driver.id == id).unwrap());
        driver.parameters[0].track.set(4, 30.0);
        shot.shift_later(6);
        assert_eq!(shot.driver(id).unwrap().parameters[0].track.at(10, false), Some(30.0));
    }

    #[test]
    fn every_preset_parses_and_reads_only_what_it_brings_with_it() {
        for preset in driver_presets() {
            let expressions = Expressions::parse(&preset.expressions).unwrap_or_else(|why| panic!("{}: {why}", preset.name));
            let mut driver = Driver::new(DriverId(1), "Driver", PegId(2));
            driver.expressions = expressions;
            driver.inputs = preset.inputs.iter().map(|name| DriverInput { name: name.clone(), peg: None }).collect();
            driver.parameters = preset
                .parameters
                .iter()
                .map(|parameter| DriverParameter { name: parameter.name.clone(), value: parameter.value, track: Track::default() })
                .collect();
            assert!(driver.missing().is_empty(), "{}: {:?}", preset.name, driver.missing());
            assert!(!driver.expressions.gives().is_empty() && !preset.hint.is_empty(), "{}", preset.name);
        }
    }

    #[test]
    fn follow_with_lag_goes_where_its_target_was_frames_before() {
        let (mut project, car, wheel) = car_and_wheel([100.0, 0.0]);
        let presets = driver_presets();
        let lagging = &presets[2];
        let id = drive(&mut project, wheel, &lagging.expressions, &[("target", car)], &[("lag", 4.0)]);
        let shot = &project.shots[0];
        // Under the car, the wheel goes as far again, four frames late: the car was at 30 then.
        let world = shot.world(Some(wheel), 7).apply(Point::new(0.0, 0.0));
        assert!(close(world.x, 70.0 + 30.0), "{world:?}");
        let _ = id;
    }

    #[test]
    fn look_at_turns_a_peg_towards_its_target_whatever_its_parent_does() {
        let (mut project, car, wheel) = car_and_wheel([100.0, 0.0]);
        let target = PegId(project.new_id());
        let mut target_peg = peg_of(target, "Target-P", None);
        target_peg.transform.position = [100.0, 300.0];
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.pegs.push(Arc::new(target_peg));
        // The car turned a quarter, and the wheel some way along it, and turned a little itself.
        let car_peg = Arc::make_mut(shot.pegs.iter_mut().find(|peg| peg.id == car).unwrap());
        car_peg.transform.rotation = 90.0;
        let wheel_peg = Arc::make_mut(shot.pegs.iter_mut().find(|peg| peg.id == wheel).unwrap());
        (wheel_peg.transform.position, wheel_peg.transform.rotation) = ([50.0, 0.0], 10.0);
        let look = driver_presets().into_iter().find(|preset| preset.name == "Look At").unwrap();
        drive(&mut project, wheel, &look.expressions, &[("target", target)], &[]);
        let shot = &project.shots[0];
        for frame in [0, 5, 10] {
            let world = shot.world(Some(wheel), frame);
            let (at, ahead) = (world.apply(Point::new(0.0, 0.0)), world.apply(Point::new(1.0, 0.0)));
            let (to, facing) = ((100.0 - at.x, 300.0 - at.y), (ahead.x - at.x, ahead.y - at.y));
            let off = (to.1.atan2(to.0) - facing.1.atan2(facing.0)).rem_euclid(std::f64::consts::TAU);
            assert!(off.min(std::f64::consts::TAU - off) < 1e-9, "frame {frame}: {off}");
        }
    }

    #[test]
    fn own_world_is_where_the_pegs_own_values_put_it_and_no_parent_is_the_scene() {
        let (mut project, _, wheel) = car_and_wheel([100.0, 0.0]);
        let id = drive(&mut project, wheel, "y = own.world.x + parent.world.rotation", &[], &[]);
        let shot = &project.shots[0];
        // Under the car, at 100 on frame 10, and the car is not turned.
        assert!(close(shot.pose(shot.peg(wheel).unwrap(), 10).position[1], 100.0));
        let alone = PegId(project.new_id());
        Arc::make_mut(&mut project.shots[0]).pegs.push(Arc::new(peg_of(alone, "Alone-P", None)));
        drive(&mut project, alone, "x = parent.x + parent.world.scale + parent.travelled", &[], &[]);
        let shot = &project.shots[0];
        assert_eq!((shot.pose(shot.peg(alone).unwrap(), 4).position[0], shot.amounts(shot.driver_of(alone).unwrap(), 4).failed), (1.0, false));
        let _ = id;
    }
}
