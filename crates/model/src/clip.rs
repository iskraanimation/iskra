//! Keyframes copied, to be pasted on another frame or onto another node of the same kind: a
//! peg's, a pose of bones', an effect node's or the camera's. Each copied key is kept with
//! the name of the value it is of, so that it finds that value again wherever it is pasted.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CycleKind, DeformerId, EffectId, Key, PegId, Shot, ShotId, Track};

/// What has keyframes, as copy and paste name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyOwner {
    Peg(PegId),
    /// A chain of bones, a curve or an envelope: its pose.
    Deformer(DeformerId),
    Effect(EffectId),
    /// A shot's camera: its lens, and the peg that moves it, if it has one.
    Camera(ShotId),
    /// A Driver node (plan §8.8): its parameters, by their names.
    Driver(crate::DriverId),
}

impl KeyOwner {
    /// Whether the two are the same kind of thing, which keys can be pasted from one onto
    /// the other.
    pub fn same_kind(self, other: KeyOwner) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }
}

/// A key as it is copied: of a number, of a pair (a place, a scale, a handle), or of a switch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CopiedKey {
    One(Key<f64>),
    Two(Key<[f64; 2]>),
    Switch(Key<bool>),
}

/// The keys of one owner on one frame, by the names of their values. `frame` is from the
/// first frame that was copied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopiedFrame {
    pub owner: KeyOwner,
    pub frame: u32,
    pub keys: BTreeMap<String, CopiedKey>,
}

/// Keyframes copied, the earliest on frame 0.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyClip {
    pub frames: Vec<CopiedFrame>,
}

/// Whether the value of this name is a place, which moves along its path: the peg's, the
/// camera peg's, the bones' offset and IK goal, and a curve's points.
fn spatial(name: &str) -> bool {
    name.ends_with("position") || name == "offset" || name == "ik.goal" || name.ends_with(".move")
}

/// One value's keys, of a number, of a pair or of a switch, to be read and written by name.
pub enum TrackMut<'a> {
    One(&'a mut Track<f64>),
    Two(&'a mut Track<[f64; 2]>),
    Switch(&'a mut Track<bool>),
}

impl TrackMut<'_> {
    /// [`Track::step`]: whether the value is a place along a path is said by `spatial`.
    fn step(&mut self, from: u32, to: u32, every: u32, spatial: bool) -> bool {
        match self {
            TrackMut::One(track) => track.step(from, to, every, spatial),
            TrackMut::Two(track) => track.step(from, to, every, spatial),
            // A switch has no in-betweens to step.
            TrackMut::Switch(_) => false,
        }
    }

    /// [`Track::set_chart`] of the way from `from`, where it goes straight to the key on `to`.
    fn chart(&mut self, from: u32, to: u32, spacing: &[f64]) -> bool {
        fn of<V: crate::Animatable>(track: &mut Track<V>, from: u32, to: u32, spacing: &[f64]) -> bool {
            let next = track.keys().iter().map(|key| key.frame).find(|at| *at > from);
            track.key(from).is_some() && next == Some(to) && track.set_chart(from, spacing)
        }
        match self {
            TrackMut::One(track) => of(track, from, to, spacing),
            TrackMut::Two(track) => of(track, from, to, spacing),
            TrackMut::Switch(_) => false,
        }
    }

    /// [`Track::set_cycle`], or with no `kind` [`Track::clear_cycles`].
    fn cycle(&mut self, from: u32, to: u32, kind: Option<CycleKind>, spatial: bool) -> bool {
        fn of<V: crate::Animatable>(track: &mut Track<V>, from: u32, to: u32, kind: Option<CycleKind>, spatial: bool) -> bool {
            match kind {
                Some(kind) => track.set_cycle(from, to, kind, spatial),
                None => track.clear_cycles(from, to),
            }
        }
        match self {
            TrackMut::One(track) => of(track, from, to, kind, spatial),
            TrackMut::Two(track) => of(track, from, to, kind, spatial),
            TrackMut::Switch(track) => of(track, from, to, kind, spatial),
        }
    }

    fn copied(&self, frame: u32) -> Option<CopiedKey> {
        match self {
            TrackMut::One(track) => track.key(frame).cloned().map(CopiedKey::One),
            TrackMut::Two(track) => track.key(frame).cloned().map(CopiedKey::Two),
            TrackMut::Switch(track) => track.key(frame).cloned().map(CopiedKey::Switch),
        }
    }

    /// Put `key` on `frame`, whole: its value, its eases and its handles. A key of the other
    /// kind of value is not put.
    fn put(&mut self, key: CopiedKey, frame: u32) -> bool {
        match (self, key) {
            (TrackMut::One(track), CopiedKey::One(key)) => track.put(Key { frame, ..key }),
            (TrackMut::Two(track), CopiedKey::Two(key)) => track.put(Key { frame, ..key }),
            (TrackMut::Switch(track), CopiedKey::Switch(key)) => track.put(Key { frame, ..key }),
            _ => return false,
        }
        true
    }
}

/// One value's keys, of a number, of a pair or of a switch, to be read by name.
#[derive(Clone, Copy)]
enum TrackRef<'a> {
    One(&'a Track<f64>),
    Two(&'a Track<[f64; 2]>),
    Switch(&'a Track<bool>),
}

impl TrackRef<'_> {
    /// [`Track::spacing`] of the way from `from`, where it goes straight to the key on `to`.
    fn spacing(self, from: u32, to: u32) -> Option<Vec<f64>> {
        fn of<V: crate::Animatable>(track: &Track<V>, from: u32, to: u32) -> Option<Vec<f64>> {
            // A held way has no in-betweens to time.
            let next = track.keys().iter().map(|key| key.frame).find(|at| *at > from);
            let held = track.key(from).is_some_and(|key| key.out == crate::Interp::Hold);
            (next == Some(to) && !held).then(|| track.spacing(from)).flatten()
        }
        match self {
            TrackRef::One(track) => of(track, from, to),
            TrackRef::Two(track) => of(track, from, to),
            // A switch has no way between its keys to read.
            TrackRef::Switch(_) => None,
        }
    }

    /// [`Track::cycle_of`].
    fn cycle_of(self, frame: u32) -> Option<(u32, u32, CycleKind)> {
        match self {
            TrackRef::One(track) => track.cycle_of(frame),
            TrackRef::Two(track) => track.cycle_of(frame),
            TrackRef::Switch(track) => track.cycle_of(frame),
        }
    }

    /// [`Track::repeated_keys`].
    fn repeated_keys(self, until: u32) -> Vec<u32> {
        match self {
            TrackRef::One(track) => track.repeated_keys(until),
            TrackRef::Two(track) => track.repeated_keys(until),
            TrackRef::Switch(track) => track.repeated_keys(until),
        }
    }
}

impl Shot {
    /// [`Shot::tracks_of`], to be read.
    fn tracks_ref(&self, owner: KeyOwner) -> Option<Vec<(String, TrackRef<'_>)>> {
        let named = |name: &str| name.to_string();
        Some(match owner {
            KeyOwner::Peg(id) => peg_tracks_ref(&self.peg(id)?.animation, ""),
            KeyOwner::Deformer(id) => {
                let deformer = self.deformer(id)?;
                let mut tracks = vec![(named("offset"), TrackRef::Two(&deformer.offsets))];
                for (at, bone) in deformer.bones.iter().enumerate() {
                    tracks.push((format!("bone{at}.turn"), TrackRef::One(&bone.turns)));
                    tracks.push((format!("bone{at}.stretch"), TrackRef::One(&bone.stretches)));
                }
                for (at, knot) in deformer.spine.iter().enumerate() {
                    tracks.push((format!("point{at}.move"), TrackRef::Two(&knot.moves)));
                    tracks.push((format!("point{at}.into"), TrackRef::Two(&knot.bends_into)));
                    tracks.push((format!("point{at}.out"), TrackRef::Two(&knot.bends_out)));
                }
                if let Some(ik) = &deformer.ik {
                    tracks.push((named("ik"), TrackRef::Switch(&ik.ons)));
                    tracks.push((named("ik.goal"), TrackRef::Two(&ik.goals)));
                }
                tracks
            }
            KeyOwner::Effect(id) => self.effect(id)?.params.iter().map(|(id, param)| (id.clone(), TrackRef::One(&param.keys))).collect(),
            KeyOwner::Driver(id) => self.driver(id)?.parameters.iter().map(|parameter| (parameter.name.clone(), TrackRef::One(&parameter.track))).collect(),
            KeyOwner::Camera(shot) if shot == self.id => {
                let lens = &self.camera.animation;
                let mut tracks = vec![(named("zoom"), TrackRef::One(&lens.zoom)), (named("distance"), TrackRef::One(&lens.distance))];
                if let Some(peg) = self.camera.parent.and_then(|peg| self.peg(peg)) {
                    tracks.extend(peg_tracks_ref(&peg.animation, "peg."));
                }
                tracks
            }
            KeyOwner::Camera(_) => return None,
        })
    }

    /// How the way of `owner` from its key on `from` to its key on `to` is spaced (see
    /// [`Track::spacing`]): of each value of it that goes straight from one to the other, or
    /// with `values` of those named alone. A timing chart's lines, which differ where the
    /// values were charted apart.
    pub fn spacings_of(&self, owner: KeyOwner, values: Option<&[&str]>, from: u32, to: u32) -> Vec<Vec<f64>> {
        let Some(tracks) = self.tracks_ref(owner) else { return Vec::new() };
        let wanted = |name: &String| values.is_none_or(|values| values.contains(&name.as_str()));
        tracks.into_iter().filter(|(name, _)| wanted(name)).filter_map(|(_, track)| track.spacing(from, to)).collect()
    }

    /// The values of what has keys, each by its name. `None` for what the shot has not got.
    fn tracks_of(&mut self, owner: KeyOwner) -> Option<Vec<(String, TrackMut<'_>)>> {
        let named = |name: &str| name.to_string();
        Some(match owner {
            KeyOwner::Peg(id) => {
                let peg = std::sync::Arc::make_mut(self.pegs.iter_mut().find(|peg| peg.id == id)?);
                peg_tracks(&mut peg.animation, "")
            }
            KeyOwner::Deformer(id) => {
                let deformer = std::sync::Arc::make_mut(self.deformers.iter_mut().find(|deformer| deformer.id == id)?);
                let mut tracks = vec![(named("offset"), TrackMut::Two(&mut deformer.offsets))];
                for (at, bone) in deformer.bones.iter_mut().enumerate() {
                    tracks.push((format!("bone{at}.turn"), TrackMut::One(&mut bone.turns)));
                    tracks.push((format!("bone{at}.stretch"), TrackMut::One(&mut bone.stretches)));
                }
                for (at, knot) in deformer.spine.iter_mut().enumerate() {
                    tracks.push((format!("point{at}.move"), TrackMut::Two(&mut knot.moves)));
                    tracks.push((format!("point{at}.into"), TrackMut::Two(&mut knot.bends_into)));
                    tracks.push((format!("point{at}.out"), TrackMut::Two(&mut knot.bends_out)));
                }
                if let Some(ik) = &mut deformer.ik {
                    tracks.push((named("ik"), TrackMut::Switch(&mut ik.ons)));
                    tracks.push((named("ik.goal"), TrackMut::Two(&mut ik.goals)));
                }
                tracks
            }
            KeyOwner::Effect(id) => {
                let effect = std::sync::Arc::make_mut(self.effects.iter_mut().find(|effect| effect.id == id)?);
                effect.params.iter_mut().map(|(id, param)| (id.clone(), TrackMut::One(&mut param.keys))).collect()
            }
            KeyOwner::Driver(id) => {
                let driver = std::sync::Arc::make_mut(self.drivers.iter_mut().find(|driver| driver.id == id)?);
                driver.parameters.iter_mut().map(|parameter| (parameter.name.clone(), TrackMut::One(&mut parameter.track))).collect()
            }
            KeyOwner::Camera(shot) if shot == self.id => {
                let (lens, peg) = (&mut self.camera.animation, self.camera.parent);
                let mut tracks = vec![(named("zoom"), TrackMut::One(&mut lens.zoom)), (named("distance"), TrackMut::One(&mut lens.distance))];
                if let Some(peg) = peg.and_then(|peg| self.pegs.iter_mut().find(|other| other.id == peg)) {
                    tracks.extend(peg_tracks(&mut std::sync::Arc::make_mut(peg).animation, "peg."));
                }
                tracks
            }
            KeyOwner::Camera(_) => return None,
        })
    }

    /// Make what `owner` does from `from` to `to` held steps of `every` frames, as drawn
    /// animation on ones, twos or threes is (see [`Track::step`]): every value of it that
    /// changes on the way, or with `value` that one alone. Whether anything changed.
    pub fn step_keys(&mut self, owner: KeyOwner, value: Option<&str>, from: u32, to: u32, every: u32) -> bool {
        let Some(tracks) = self.tracks_of(owner) else { return false };
        let mut stepped = false;
        for (name, mut track) in tracks.into_iter().filter(|(name, _)| value.is_none_or(|value| value == name)) {
            stepped |= track.step(from, to, every, spatial(&name));
        }
        stepped
    }

    /// Repeat what `owner` does from `from` to `to` after `to`, as `kind` says, until its next
    /// key (see [`Track::set_cycle`]); with no `kind`, take away its cycles that have a frame
    /// from `from` to `to` in them. Every value of it with a key on the way, or with `value`
    /// that one alone. How many were.
    pub fn cycle_keys(&mut self, owner: KeyOwner, value: Option<&str>, from: u32, to: u32, kind: Option<CycleKind>) -> usize {
        let Some(tracks) = self.tracks_of(owner) else { return 0 };
        let wanted = tracks.into_iter().filter(|(name, _)| value.is_none_or(|value| value == name));
        wanted.map(|(name, mut track)| track.cycle(from, to, kind, spatial(&name))).filter(|cycled| *cycled).count()
    }

    /// The cycle of `owner` that `frame` is in, of any of its values or of those of `values`:
    /// its first and last frames and how it repeats.
    pub fn cycle_of(&self, owner: KeyOwner, values: Option<&[&str]>, frame: u32) -> Option<(u32, u32, CycleKind)> {
        let tracks = self.tracks_ref(owner)?;
        let wanted = |name: &String| values.is_none_or(|values| values.contains(&name.as_str()));
        tracks.into_iter().filter(|(name, _)| wanted(name)).find_map(|(_, track)| track.cycle_of(frame))
    }

    /// The frames before `until` on which a key of `owner` is repeated by a cycle, of any of
    /// its values or of those of `values`, and none of those has a key of its own.
    pub fn repeated_keys_of(&self, owner: KeyOwner, values: Option<&[&str]>, until: u32) -> std::collections::BTreeSet<u32> {
        let Some(tracks) = self.tracks_ref(owner) else { return Default::default() };
        let wanted = |name: &String| values.is_none_or(|values| values.contains(&name.as_str()));
        tracks.into_iter().filter(|(name, _)| wanted(name)).flat_map(|(_, track)| track.repeated_keys(until)).collect()
    }

    /// Time the way of `owner` from its key on `from` to its key on `to` by the chart of
    /// `spacing` (see [`Track::set_chart`]; empty takes the chart away): every value of it that
    /// has a key on both and none between, or with `value` that one alone. How many were.
    pub fn chart_keys(&mut self, owner: KeyOwner, value: Option<&str>, from: u32, to: u32, spacing: &[f64]) -> usize {
        let Some(tracks) = self.tracks_of(owner) else { return 0 };
        let wanted = tracks.into_iter().filter(|(name, _)| value.is_none_or(|value| value == name));
        wanted.map(|(_, mut track)| track.chart(from, to, spacing)).filter(|charted| *charted).count()
    }

    /// The frame of the first key of `owner`, of any of its values or of `value` alone, after
    /// `frame`.
    pub fn next_key_of(&mut self, owner: KeyOwner, value: Option<&str>, frame: u32) -> Option<u32> {
        let tracks = self.tracks_of(owner)?;
        let later = |track: &TrackMut| match track {
            TrackMut::One(track) => track.keys().iter().map(|key| key.frame).find(|at| *at > frame),
            TrackMut::Two(track) => track.keys().iter().map(|key| key.frame).find(|at| *at > frame),
            TrackMut::Switch(track) => track.keys().iter().map(|key| key.frame).find(|at| *at > frame),
        };
        tracks.iter().filter(|(name, _)| value.is_none_or(|value| value == name)).filter_map(|(_, track)| later(track)).min()
    }

    /// Copy the keys of `picked`, each an owner and a frame: all its values that have a key
    /// there. What has none there, or is not in the shot, is left out.
    pub fn copy_keys(&self, picked: &[(KeyOwner, u32)]) -> KeyClip {
        let all: Vec<(KeyOwner, u32, Option<String>)> = picked.iter().map(|(owner, frame)| (*owner, *frame, None)).collect();
        self.copy_keys_of(&all)
    }

    /// [`Shot::copy_keys`], each of all the owner's values or of the one named, as the clip
    /// names it (`position`, `bone0.turn`, an effect's parameter). Picked twice on one frame,
    /// an owner's keys are copied once, of all that was named.
    pub fn copy_keys_of(&self, picked: &[(KeyOwner, u32, Option<String>)]) -> KeyClip {
        let first = picked.iter().map(|(_, frame, _)| *frame).min().unwrap_or(0);
        // The tracks are read through the same names as they are written by: of a copy.
        let mut shot = self.clone();
        let mut frames: Vec<CopiedFrame> = Vec::new();
        for (owner, frame, value) in picked {
            let Some(tracks) = shot.tracks_of(*owner) else { continue };
            let wanted = |name: &String| value.as_ref().is_none_or(|value| value == name);
            let keys: BTreeMap<String, CopiedKey> =
                tracks.iter().filter(|(name, _)| wanted(name)).filter_map(|(name, track)| Some((name.clone(), track.copied(*frame)?))).collect();
            if keys.is_empty() {
                continue;
            }
            match frames.iter_mut().find(|copied| copied.owner == *owner && copied.frame == frame - first) {
                Some(copied) => copied.keys.extend(keys),
                None => frames.push(CopiedFrame { owner: *owner, frame: frame - first, keys }),
            }
        }
        frames.sort_by_key(|copied| copied.frame);
        KeyClip { frames }
    }

    /// Paste `clip` with its first frame on `at`, each key as it was copied: on its value of
    /// its owner, or with `onto`, of that one where the clip has keys of one thing of its
    /// kind. A key whose value the owner has not got (a bone a chain has not got, a
    /// parameter of another kind of effect) is left out. What was pasted, as owners and frames.
    pub fn paste_keys(&mut self, clip: &KeyClip, at: u32, onto: Option<KeyOwner>) -> Vec<(KeyOwner, u32)> {
        let owners_of_kind = |kind: KeyOwner| {
            let mut owners: Vec<KeyOwner> = clip.frames.iter().map(|copied| copied.owner).filter(|owner| owner.same_kind(kind)).collect();
            owners.dedup();
            owners.len()
        };
        let mut pasted = Vec::new();
        for copied in &clip.frames {
            let owner = match onto {
                Some(onto) if onto.same_kind(copied.owner) && owners_of_kind(onto) == 1 => onto,
                _ => copied.owner,
            };
            let frame = at.saturating_add(copied.frame);
            // A parameter that another node of the kind has keys of, and this one has not
            // got yet, is made as the kind starts it, for the key to land on.
            if let KeyOwner::Effect(id) = owner
                && let Some(effect) = self.effects.iter_mut().find(|effect| effect.id == id)
            {
                let effect = std::sync::Arc::make_mut(effect);
                for name in copied.keys.keys() {
                    effect.param_mut(name);
                }
            }
            let Some(mut tracks) = self.tracks_of(owner) else { continue };
            let mut any = false;
            for (name, key) in &copied.keys {
                if let Some((_, track)) = tracks.iter_mut().find(|(other, _)| other == name) {
                    any |= track.put(key.clone(), frame);
                }
            }
            if any && !pasted.contains(&(owner, frame)) {
                pasted.push((owner, frame));
            }
        }
        pasted
    }
}

/// A peg's values by name, each name after `prefix`.
fn peg_tracks<'a>(animation: &'a mut crate::Animation, prefix: &str) -> Vec<(String, TrackMut<'a>)> {
    let crate::Animation { position, rotation, scale, skew, depth, tilt } = animation;
    let named = |name: &str| format!("{prefix}{name}");
    vec![
        (named("position"), TrackMut::Two(position)),
        (named("rotation"), TrackMut::One(rotation)),
        (named("scale"), TrackMut::Two(scale)),
        (named("skew"), TrackMut::One(skew)),
        (named("depth"), TrackMut::One(depth)),
        (named("tilt"), TrackMut::Two(tilt)),
    ]
}

/// [`peg_tracks`], to be read.
fn peg_tracks_ref<'a>(animation: &'a crate::Animation, prefix: &str) -> Vec<(String, TrackRef<'a>)> {
    let crate::Animation { position, rotation, scale, skew, depth, tilt } = animation;
    let named = |name: &str| format!("{prefix}{name}");
    vec![
        (named("position"), TrackRef::Two(position)),
        (named("rotation"), TrackRef::One(rotation)),
        (named("scale"), TrackRef::Two(scale)),
        (named("skew"), TrackRef::One(skew)),
        (named("depth"), TrackRef::One(depth)),
        (named("tilt"), TrackRef::Two(tilt)),
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{Bone, Deformer, Interp, Peg, Transform};

    fn peg(id: u64) -> Arc<Peg> {
        Arc::new(Peg {
            id: PegId(id),
            name: id.to_string(),
            parent: None,
            transform: Transform::default(),
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        })
    }

    #[test]
    fn keys_copied_are_pasted_on_another_frame_and_onto_another_of_their_kind() {
        let mut project = crate::Project::default();
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.pegs = vec![peg(1), peg(2)];
        let arm = Arc::make_mut(&mut shot.pegs[0]);
        arm.animation.position.set(4, [10.0, 0.0]);
        arm.animation.position.set(8, [30.0, 5.0]);
        arm.animation.rotation.set(8, 45.0);
        arm.animation.set_interp(8, None, Some(Interp::Ease), None);
        // Frames 4 and 8 of the first peg: the clip has them from 0, each with its values.
        let clip = shot.copy_keys(&[(KeyOwner::Peg(PegId(1)), 8), (KeyOwner::Peg(PegId(1)), 4), (KeyOwner::Peg(PegId(1)), 6)]);
        assert_eq!(clip.frames.iter().map(|copied| (copied.frame, copied.keys.len())).collect::<Vec<_>>(), [(0, 1), (4, 2)], "frame 6 has none");
        // Pasted from frame 20: on 20 and 24, the second as it was, with its ease.
        let pasted = shot.paste_keys(&clip, 20, None);
        assert_eq!(pasted, [(KeyOwner::Peg(PegId(1)), 20), (KeyOwner::Peg(PegId(1)), 24)]);
        let arm = &shot.pegs[0].animation;
        assert_eq!(
            (arm.position.key(24).map(|key| (key.value, key.into)), arm.rotation.key(24).map(|key| key.value)),
            (Some(([30.0, 5.0], Interp::Ease)), Some(45.0))
        );
        // Onto the other peg, which is of the same kind: there, and not on the first.
        let before = shot.pegs[0].clone();
        let pasted = shot.paste_keys(&clip, 0, Some(KeyOwner::Peg(PegId(2))));
        assert_eq!(pasted, [(KeyOwner::Peg(PegId(2)), 0), (KeyOwner::Peg(PegId(2)), 4)]);
        assert_eq!((shot.pegs[0].clone(), shot.pegs[1].animation.position.key(4).map(|key| key.value)), (before, Some([30.0, 5.0])));
        // Onto something of another kind, they go where they came from.
        let pasted = shot.paste_keys(&clip, 40, Some(KeyOwner::Deformer(DeformerId(9))));
        assert_eq!(pasted[0], (KeyOwner::Peg(PegId(1)), 40));
    }

    #[test]
    fn a_pose_is_pasted_onto_bones_by_the_bones_both_have() {
        let mut project = crate::Project::default();
        let shot = Arc::make_mut(&mut project.shots[0]);
        let mut arm = Deformer::new(DeformerId(1), "Arm", [0.0, 0.0], vec![Bone::at_rest(0.0, 10.0), Bone::at_rest(0.0, 10.0)]);
        arm.pose_turn(1, 0, 30.0);
        arm.key_all(3);
        let short = Deformer::new(DeformerId(2), "Short", [0.0, 0.0], vec![Bone::at_rest(0.0, 10.0)]);
        shot.deformers = vec![Arc::new(arm), Arc::new(short)];
        let clip = shot.copy_keys(&[(KeyOwner::Deformer(DeformerId(1)), 3)]);
        assert!(clip.frames[0].keys.contains_key("bone1.turn"));
        // The one bone the other chain has gets its key; the second bone's is left out.
        assert_eq!(shot.paste_keys(&clip, 7, Some(KeyOwner::Deformer(DeformerId(2)))), [(KeyOwner::Deformer(DeformerId(2)), 7)]);
        let short = &shot.deformers[1];
        assert_eq!((short.bones.len(), short.bones[0].turns.key(7).map(|key| key.value)), (1, Some(0.0)));
        // And the clip goes through text, as the system's clipboard has it.
        let text = serde_json::to_string(&clip).unwrap();
        assert_eq!(serde_json::from_str::<KeyClip>(&text).unwrap(), clip);
    }
}
