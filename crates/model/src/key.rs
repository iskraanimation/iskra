//! Keyframes (plan §4.3, R8): a track holds a value on some frames, and between them the value
//! is worked out. How it gets from one key to the next is each key's own: straight, eased, or
//! held until the next key; or, by a timing chart, through points of its own between them.
//!
//! The interpolation is `effectcraft-keyframe`'s. Its clock is in seconds; here one frame is
//! one of its seconds, so keys stay on their frames whatever the project's frame rate.

use effectcraft_keyframe as kf;
use effectcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};

/// How a value moves on one side of a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Interp {
    /// At a steady rate.
    #[default]
    Linear,
    /// Slowing into the key, or starting slowly out of it: at rest at the key.
    Ease,
    /// On the way out of a key: the value stays until the next key, then jumps.
    Hold,
    /// As its slope says: what the function curve editor's handles set.
    Curve(Slope),
}

/// The shape of the way between two keys on one key's side, as a handle of the curve of the
/// value over time. It is in the measure of that way, so it fits whatever the value is: a
/// number, a pair, or a place moving along a path.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Slope {
    /// How far the handle reaches towards the other key, as a part of the time between them:
    /// above 0, up to 1.
    pub reach: f64,
    /// How fast the value moves at the key, where 1 is the steady speed that would take it to
    /// the other key in the time there is. 0 is at rest; above 1 it runs ahead, below 0 it
    /// first backs away.
    pub speed: f64,
}

impl Interp {
    /// The side as a slope. A held side has none.
    pub fn slope(self) -> Option<Slope> {
        match self {
            Interp::Linear => Some(Slope { reach: 1.0 / 3.0, speed: 1.0 }),
            Interp::Ease => Some(Slope { reach: 1.0 / 3.0, speed: 0.0 }),
            Interp::Curve(slope) => Some(slope),
            Interp::Hold => None,
        }
    }
}

/// A value that can be keyed: a number, or a pair of them.
pub trait Animatable: Copy + PartialEq {
    fn to_value(self) -> kf::Value;
    fn from_value(value: &kf::Value) -> Option<Self>;
    /// The value moved by as much as takes `from` to `to`.
    fn moved(self, from: Self, to: Self) -> Self;
}

impl Animatable for f64 {
    fn to_value(self) -> kf::Value {
        kf::Value::Scalar(self)
    }

    fn from_value(value: &kf::Value) -> Option<Self> {
        if let kf::Value::Scalar(v) = value { Some(*v) } else { None }
    }

    fn moved(self, from: Self, to: Self) -> Self {
        self + to - from
    }
}

impl Animatable for [f64; 2] {
    fn to_value(self) -> kf::Value {
        kf::Value::Vec2(self)
    }

    fn from_value(value: &kf::Value) -> Option<Self> {
        if let kf::Value::Vec2(v) = value { Some(*v) } else { None }
    }

    fn moved(self, from: Self, to: Self) -> Self {
        [self[0] + to[0] - from[0], self[1] + to[1] - from[1]]
    }
}

/// A switch: on or off, held from key to key.
impl Animatable for bool {
    fn to_value(self) -> kf::Value {
        kf::Value::Scalar(if self { 1.0 } else { 0.0 })
    }

    fn from_value(value: &kf::Value) -> Option<Self> {
        if let kf::Value::Scalar(v) = value { Some(*v >= 0.5) } else { None }
    }

    /// A switch is not moved by as much: it is set as `to`.
    fn moved(self, _from: Self, to: Self) -> Self {
        to
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Key<V> {
    pub frame: u32,
    pub value: V,
    /// How the value arrives at the key.
    #[serde(default)]
    pub into: Interp,
    /// How it leaves.
    #[serde(default)]
    pub out: Interp,
    /// For a place, which moves along a path: how the path bends through this key. Without one
    /// the path passes smoothly, turning as the keys before and after ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bend: Option<Bend>,
    /// The timing chart of the way to the next key: points the timing passes through, each a
    /// part of the time between the two keys and how far along the way the value is then, both
    /// from 0 to 1, in order. With points, the eases of the two keys' sides are not used; with
    /// none, the way is as they shape it. See [`Track::spacing`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chart: Vec<[f64; 2]>,
    /// The key ends a cycle: what the value does from the cycle's first frame to this key is
    /// done again after it, until the next key or for ever. See [`Track::cycled`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<Cycle>,
}

/// How a cycle repeats: from its start again each time, or back and forth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CycleKind {
    Loop,
    PingPong,
}

/// A cycle, held by the key it ends on: the frame it starts on, and how it repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cycle {
    pub from: u32,
    pub kind: CycleKind,
}

impl Cycle {
    /// The frame of the cycle that `frame`, after the cycle's last frame `to`, repeats. With a
    /// loop the frame after `to` is the one after `from`, `to` and `from` being one moment;
    /// back and forth, it is the one before `to`.
    pub fn repeats(self, to: u32, frame: u32) -> u32 {
        let span = to.saturating_sub(self.from);
        if span == 0 || frame <= to {
            return frame;
        }
        let past = frame - self.from;
        match self.kind {
            CycleKind::Loop => self.from + past % span,
            CycleKind::PingPong => {
                let part = past % (2 * span);
                if part <= span { self.from + part } else { self.from + 2 * span - part }
            }
        }
    }
}

impl<V> Key<V> {
    /// A key of `value` on `frame`, straight in and out, with no bend nor chart.
    pub fn new(frame: u32, value: V) -> Self {
        Key { frame, value, into: Interp::Linear, out: Interp::Linear, bend: None, chart: Vec::new(), cycle: None }
    }
}

/// How the lines of a timing chart are spaced again when one is dragged: each halfway between
/// the key and the line after it, bunched towards the key as an ease; evenly in the room; or
/// not at all, the dragged one moving alone between the ones beside it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Spacing {
    #[default]
    Halves,
    Thirds,
    Manual,
}

/// A chart's in-betweens after the one at `line` is dragged to `to`, a part of the way. The
/// lines between it and the key it moved towards are spaced again by `rule`, and with
/// `other_side` those between it and the other key too; the rest stay. The lines keep their
/// order, so the value never goes back. `spacing` is where each in-between is now, in order.
pub fn squash(spacing: &[f64], line: usize, to: f64, rule: Spacing, other_side: bool) -> Vec<f64> {
    let mut made = spacing.to_vec();
    let Some(&was) = spacing.get(line) else { return made };
    if rule == Spacing::Manual {
        let low = line.checked_sub(1).map_or(0.0, |before| spacing[before]);
        made[line] = to.clamp(low, spacing.get(line + 1).copied().unwrap_or(1.0));
        return made;
    }
    let to = to.clamp(0.0, 1.0);
    made[line] = to;
    let (before, after) = (line, spacing.len() - line - 1);
    // The `count` lines from the dragged one towards a key, the nearest it first, as parts of
    // the way from the line (at 0) to the key (at 1).
    let towards_key = |count: usize| -> Vec<f64> {
        (1..=count)
            .map(|step| match rule {
                Spacing::Halves => 1.0 - 0.5f64.powi(step as i32),
                Spacing::Thirds | Spacing::Manual => step as f64 / (count + 1) as f64,
            })
            .collect()
    };
    let towards_start = to < was;
    if towards_start || other_side {
        for (step, part) in towards_key(before).into_iter().enumerate() {
            made[line - 1 - step] = to * (1.0 - part);
        }
    }
    if !towards_start || other_side {
        for (step, part) in towards_key(after).into_iter().enumerate() {
            made[line + 1 + step] = to + (1.0 - to) * part;
        }
    }
    made
}

/// How far along the way the timing is at `time`, both from 0 to 1, by the chart's points: a
/// smooth curve through them that never goes back (monotone cubic, Fritsch and Carlson).
fn charted(points: &[[f64; 2]], time: f64) -> f64 {
    let mut all = Vec::with_capacity(points.len() + 2);
    all.push([0.0, 0.0]);
    all.extend(points.iter().copied().filter(|[t, _]| *t > 0.0 && *t < 1.0));
    all.push([1.0, 1.0]);
    let time = time.clamp(0.0, 1.0);
    let at = all.partition_point(|[t, _]| *t <= time).clamp(1, all.len() - 1) - 1;
    let width = |i: usize| (all[i + 1][0] - all[i][0]).max(1e-12);
    let slope = |i: usize| (all[i + 1][1] - all[i][1]) / width(i);
    // The curve's slope at a point: none where it stays, else a mean of its sides' weighted by
    // their widths, which keeps the curve from overshooting.
    let tangent = |i: usize| -> f64 {
        if i == 0 {
            return slope(0);
        }
        if i == all.len() - 1 {
            return slope(i - 1);
        }
        let (a, b) = (slope(i - 1), slope(i));
        if a * b <= 0.0 {
            return 0.0;
        }
        let (before, after) = (2.0 * width(i) + width(i - 1), width(i) + 2.0 * width(i - 1));
        (before + after) / (before / a + after / b)
    };
    let ([t0, p0], [t1, p1]) = (all[at], all[at + 1]);
    let h = (t1 - t0).max(1e-12);
    let s = (time - t0) / h;
    let (m0, m1) = (tangent(at), tangent(at + 1));
    let (s2, s3) = (s * s, s * s * s);
    let p = (2.0 * s3 - 3.0 * s2 + 1.0) * p0 + (s3 - 2.0 * s2 + s) * h * m0 + (-2.0 * s3 + 3.0 * s2) * p1 + (s3 - s2) * h * m1;
    p.clamp(p0.min(p1), p0.max(p1))
}

/// The handles of a path at a key, each from the key: the path arrives from the direction of
/// `into` and leaves towards `out`, and the longer a handle, the wider the curve on its side.
/// Both at nothing make a corner with straight ways in and out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bend {
    pub into: [f64; 2],
    pub out: [f64; 2],
}

/// The keys of one value, in frame order, at most one a frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Track<V> {
    keys: Vec<Key<V>>,
}

impl<V> Default for Track<V> {
    fn default() -> Self {
        Self { keys: Vec::new() }
    }
}

fn tick(frame: u32) -> Tick {
    Tick(i64::from(frame) * TICKS_PER_SECOND)
}

impl<V: Animatable> Track<V> {
    pub fn keys(&self) -> &[Key<V>] {
        &self.keys
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn index(&self, frame: u32) -> Result<usize, usize> {
        self.keys.binary_search_by_key(&frame, |key| key.frame)
    }

    /// Whether the value moves on after `frame`, towards a later key: there is one, and the
    /// key on the frame, or the last before it, is not held.
    pub fn moves_after(&self, frame: u32) -> bool {
        let from = self.keys.iter().rev().find(|key| key.frame <= frame);
        from.is_some_and(|key| key.out != Interp::Hold) && self.keys.last().is_some_and(|key| key.frame > frame)
    }

    /// The key on `frame`, if there is one.
    pub fn key(&self, frame: u32) -> Option<&Key<V>> {
        self.index(frame).ok().map(|at| &self.keys[at])
    }

    pub fn key_mut(&mut self, frame: u32) -> Option<&mut Key<V>> {
        self.index(frame).ok().map(|at| &mut self.keys[at])
    }

    /// Put a key whole, its eases and handles too, over any on its frame. One put between two
    /// keys whose way has a chart splits it (see [`Track::insert`]).
    pub fn put(&mut self, key: Key<V>) {
        match self.index(key.frame) {
            Ok(at) => self.keys[at] = key,
            Err(at) => self.insert(at, key),
        }
    }

    /// Key `value` on `frame`. A key that is there already keeps how it eases.
    pub fn set(&mut self, frame: u32, value: V) {
        match self.index(frame) {
            Ok(at) => self.keys[at].value = value,
            Err(at) => self.insert(at, Key::new(frame, value)),
        }
    }

    /// Insert `key` at `at`, where no key has its frame. Where the way it falls in has a chart,
    /// the chart is split in two, each in-between measured anew as a part of its own way, so
    /// that the value on every frame stays as it was where the key is put on the way.
    fn insert(&mut self, at: usize, mut key: Key<V>) {
        if let (Some(before), Some(after)) = (at.checked_sub(1).map(|at| &self.keys[at]), self.keys.get(at))
            && !before.chart.is_empty()
        {
            let (from, to, cut) = (before.frame, after.frame, key.frame);
            let parts = self.spacing(from).unwrap_or_default();
            let split = parts[(cut - from - 1) as usize];
            let measured = |start: u32, end: u32, part: &dyn Fn(f64) -> f64| -> Vec<[f64; 2]> {
                let span = f64::from(end - start);
                (start + 1..end).map(|frame| [f64::from(frame - start) / span, part(parts[(frame - from - 1) as usize]).clamp(0.0, 1.0)]).collect()
            };
            key.chart = measured(cut, to, &|part| if split < 1.0 { (part - split) / (1.0 - split) } else { 1.0 });
            self.keys[at - 1].chart = measured(from, cut, &|part| if split > 0.0 { part / split } else { 0.0 });
        }
        self.keys.insert(at, key);
    }

    /// Where each in-between of the way from the key at `at` stands in time, in frames: its
    /// frame, and the time at which a steady way would be where the value is then.
    fn places(&self, at: usize) -> Vec<(u32, f64)> {
        let Some(after) = self.keys.get(at + 1) else { return Vec::new() };
        let from = self.keys[at].frame;
        let span = f64::from(after.frame - from);
        self.spacing(from).unwrap_or_default().into_iter().zip(from + 1..).map(|(part, frame)| (frame, f64::from(from) + part * span)).collect()
    }

    /// How far along the way from the key on `from` to the next the value is on each frame
    /// between them, from 0 to 1: what a timing chart's lines show. By the way's chart, or as
    /// its eases shape it; a held way stays at 0. None where there is no key on `from`, or no
    /// key after it.
    pub fn spacing(&self, from: u32) -> Option<Vec<f64>> {
        let at = self.index(from).ok()?;
        let (key, next) = (&self.keys[at], self.keys.get(at + 1)?);
        let span = f64::from(next.frame - key.frame);
        let frames = key.frame + 1..next.frame;
        if key.out == Interp::Hold {
            return Some(frames.map(|_| 0.0).collect());
        }
        if !key.chart.is_empty() {
            return Some(frames.map(|frame| charted(&key.chart, f64::from(frame - key.frame) / span)).collect());
        }
        // The eases are in the measure of the way, so a number from 0 to 1 eased alike is how
        // far along it the value is, whatever the value.
        let probe = Track {
            keys: vec![Key { into: Interp::Linear, ..Key { out: key.out, ..Key::new(key.frame, 0.0) } }, Key { into: next.into, ..Key::new(next.frame, 1.0) }],
        };
        Some(frames.map(|frame| probe.at(frame, false).unwrap_or(0.0)).collect())
    }

    /// Give the way from the key on `from` to the next the chart of `spacing`: how far along
    /// it the value is on each frame between them, in order. Empty takes the chart away. The
    /// parts are kept from 0 to 1 and never going back. Nothing where there is no such way.
    pub fn set_chart(&mut self, from: u32, spacing: &[f64]) -> bool {
        let Ok(at) = self.index(from) else { return false };
        let Some(next) = self.keys.get(at + 1).map(|key| key.frame) else { return false };
        let span = f64::from(next - from);
        let mut last = 0.0f64;
        let chart = spacing
            .iter()
            .take((next - from - 1) as usize)
            .zip(1..)
            .map(|(part, step)| {
                last = part.clamp(last, 1.0);
                [f64::from(step) / span, last]
            })
            .collect();
        self.keys[at].chart = chart;
        true
    }

    /// Make the value on `frame` be `value` by moving every key by as much: the whole of the
    /// animation goes along, as it is shaped, and no key is made. Nothing on a track with no
    /// keys.
    pub fn move_all(&mut self, frame: u32, value: V, spatial: bool) {
        if let Some(from) = self.at(frame, spatial) {
            self.keys.iter_mut().for_each(|key| key.value = key.value.moved(from, value));
        }
    }

    /// Key `value` on `frame`, and move the keys on `frames` by as much as the value on `frame`
    /// moved: what was changed on one key is changed alike on the others. The keys on other
    /// frames stay as they are.
    pub fn set_adjusting(&mut self, frame: u32, value: V, spatial: bool, frames: &[u32]) {
        let from = self.at(frame, spatial);
        self.set(frame, value);
        if let Some(from) = from {
            self.adjust(frame, from, value, frames);
        }
    }

    /// Move the keys on `frames`, but the one on `frame`, by as much as takes `from` to `to`:
    /// what was changed on `frame` is changed alike on them.
    pub fn adjust(&mut self, frame: u32, from: V, to: V, frames: &[u32]) {
        let others = self.keys.iter_mut().filter(|key| key.frame != frame && frames.contains(&key.frame));
        others.for_each(|key| key.value = key.value.moved(from, to));
    }

    /// Make the way from `from` to `to` held steps of `every` frames: a key on `from` and every
    /// `every` frames after it, before `to`, of the value the curve had there (along the path,
    /// for a place), each held until the next; and one on `to` as the curve had it, for what
    /// comes after to go on as it went. Keys already between them are held too. A track with a
    /// key from `from` to `to` is stepped whatever its values, the same all the way or not; one
    /// with none there is left: `false` then.
    pub fn step(&mut self, from: u32, to: u32, every: u32, spatial: bool) -> bool {
        if to <= from || !self.keys.iter().any(|key| (from..=to).contains(&key.frame)) {
            return false;
        }
        let steps: Vec<(u32, V)> = (from..to).step_by(every.max(1) as usize).filter_map(|frame| Some((frame, self.at(frame, spatial)?))).collect();
        let Some(end) = self.at(to, spatial) else { return false };
        if self.key(to).is_none() {
            self.put(Key::new(to, end));
        }
        for (frame, value) in steps {
            if self.key(frame).is_none() {
                self.put(Key { out: Interp::Hold, ..Key::new(frame, value) });
            }
        }
        for key in self.keys.iter_mut().filter(|key| (from..to).contains(&key.frame)) {
            (key.out, key.chart) = (Interp::Hold, Vec::new());
        }
        true
    }

    /// Take the key on `frame` away. Where a way on either side of it had a chart, the two ways
    /// are one with one chart: each in-between keeps its place in time, and the frame of the key
    /// that went is an in-between of its own.
    pub fn remove(&mut self, frame: u32) -> Option<Key<V>> {
        let at = self.index(frame).ok()?;
        let joined = match (at.checked_sub(1), self.keys.get(at + 1)) {
            (Some(before), Some(after)) if !self.keys[before].chart.is_empty() || !self.keys[at].chart.is_empty() => {
                let mut places = self.places(before);
                places.push((frame, f64::from(frame)));
                places.extend(self.places(at));
                let (start, span) = (self.keys[before].frame, f64::from(after.frame - self.keys[before].frame));
                Some((
                    before,
                    places.into_iter().map(|(frame, place)| [f64::from(frame - start) / span, ((place - f64::from(start)) / span).clamp(0.0, 1.0)]).collect(),
                ))
            }
            _ => None,
        };
        let key = self.keys.remove(at);
        if let Some((before, chart)) = joined {
            self.keys[before].chart = chart;
        }
        Some(key)
    }

    /// Move the key on `from` to `to`, over any key that is there. Moved between the keys on
    /// either side of it, the charts of its ways are stretched with it; past them, it is taken
    /// away and put again.
    /// A cycle that starts on the key's frame starts where it goes.
    pub fn move_key(&mut self, from: u32, to: u32) {
        let Ok(at) = self.index(from) else { return };
        for cycle in self.keys.iter_mut().filter_map(|key| key.cycle.as_mut()).filter(|cycle| cycle.from == from) {
            cycle.from = to;
        }
        let after_before = at.checked_sub(1).is_none_or(|before| self.keys[before].frame < to);
        let before_after = self.keys.get(at + 1).is_none_or(|after| to < after.frame);
        if after_before && before_after {
            self.keys[at].frame = to;
            return;
        }
        if let Some(key) = self.remove(from) {
            self.remove(to);
            let at = self.index(to).unwrap_or_else(|at| at);
            self.insert(at, Key { frame: to, ..key });
        }
    }

    /// The track with every key `by` frames later.
    pub fn shifted(&self, by: u32) -> Track<V> {
        Track { keys: self.keys.iter().map(|key| Key { frame: key.frame.saturating_add(by), ..key.clone() }).collect() }
    }

    /// The track with every key's value as `value` makes it, and a path's handles as `handle`
    /// does: the same keys on the same frames, eased as they were.
    pub fn mapped(&self, value: impl Fn(V) -> V, handle: impl Fn([f64; 2]) -> [f64; 2]) -> Track<V> {
        let bend = |bend: Bend| Bend { into: handle(bend.into), out: handle(bend.out) };
        Track { keys: self.keys.iter().map(|key| Key { value: value(key.value), bend: key.bend.map(bend), ..key.clone() }).collect() }
    }

    /// Put the keys of `other` into the track, over any that are on their frames.
    pub fn merge(&mut self, other: &Track<V>) {
        for key in &other.keys {
            match self.index(key.frame) {
                Ok(at) => self.keys[at] = key.clone(),
                Err(at) => self.keys.insert(at, key.clone()),
            }
        }
    }

    /// Go on, from `cut`, as `later` does, which is another track of the same value: the keys
    /// before `cut` stay, those from it on are `later`'s, and the value jumps on `cut` from
    /// `before`, what it was a frame earlier, to `after`, what `later` has there. The jump is
    /// a held key on the frame before and a key on `cut`. Where neither track has a key and
    /// the two values are the same, there is nothing to do, and no key is made.
    pub fn go_on_as(&mut self, cut: u32, before: V, later: &Track<V>, after: V) {
        if self.is_empty() && later.is_empty() && before == after {
            return;
        }
        self.keys.retain(|key| key.frame < cut);
        if let Some(last) = cut.checked_sub(1) {
            self.set(last, before);
            self.keys.iter_mut().filter(|key| key.frame == last).for_each(|key| key.out = Interp::Hold);
        }
        self.set(cut, after);
        self.merge(&Track { keys: later.keys.iter().filter(|key| key.frame >= cut).cloned().collect() });
    }

    /// The cycle `frame` is repeated by: the key that ends it, where `frame` is after that key
    /// and before the next.
    fn cycle_over(&self, frame: u32) -> Option<(&Key<V>, Cycle)> {
        let before = self.keys.partition_point(|key| key.frame <= frame).checked_sub(1)?;
        let key = &self.keys[before];
        key.cycle.filter(|cycle| cycle.from < key.frame && key.frame < frame).map(|cycle| (key, cycle))
    }

    /// The frame of a cycle that `frame` repeats, where a cycle repeats on it.
    pub fn cycled(&self, frame: u32) -> Option<u32> {
        self.cycle_over(frame).map(|(key, cycle)| cycle.repeats(key.frame, frame))
    }

    /// The cycle that `frame` is in, as its first and last frames and how it repeats: from its
    /// start to its last key, or repeated after it until the next key.
    pub fn cycle_of(&self, frame: u32) -> Option<(u32, u32, CycleKind)> {
        if let Some((key, cycle)) = self.cycle_over(frame) {
            return Some((cycle.from, key.frame, cycle.kind));
        }
        let ends = self.keys.iter().filter_map(|key| Some((key.frame, key.cycle.filter(|cycle| cycle.from < key.frame)?)));
        ends.map(|(to, cycle)| (cycle.from, to, cycle.kind)).find(|(from, to, _)| (*from..=*to).contains(&frame))
    }

    /// The frames before `until` that repeat a key of a cycle, and are not keys themselves:
    /// where the timeline shows a key as repeated.
    pub fn repeated_keys(&self, until: u32) -> Vec<u32> {
        let mut made = Vec::new();
        for (at, key) in self.keys.iter().enumerate() {
            if key.cycle.is_none_or(|cycle| cycle.from >= key.frame) {
                continue;
            }
            let end = self.keys.get(at + 1).map_or(until, |next| next.frame.min(until));
            made.extend((key.frame + 1..end).filter(|frame| self.cycled(*frame).is_some_and(|repeated| self.key(repeated).is_some())));
        }
        made
    }

    /// Repeat what the value does from `from` to `to` after `to`, as `kind` says, until the
    /// next key: the key on `to` ends the cycle, and is made of the value there if there is
    /// none, so that every value of a node cycles alike. Cycles ending between the two are
    /// taken away. Nothing where no key is from `from` to `to`: `false` then.
    pub fn set_cycle(&mut self, from: u32, to: u32, kind: CycleKind, spatial: bool) -> bool {
        if to <= from || !self.keys.iter().any(|key| (from..=to).contains(&key.frame)) {
            return false;
        }
        if self.key(to).is_none() {
            let Some(value) = self.at(to, spatial) else { return false };
            self.put(Key::new(to, value));
        }
        for key in self.keys.iter_mut().filter(|key| key.frame > from && key.frame < to) {
            key.cycle = None;
        }
        self.key_mut(to).expect("put above").cycle = Some(Cycle { from, kind });
        true
    }

    /// Take away every cycle that has a frame from `from` to `to` in it: whether there was one.
    pub fn clear_cycles(&mut self, from: u32, to: u32) -> bool {
        let ends: Vec<u32> = self.keys.iter().map(|key| key.frame).collect();
        let mut cleared = false;
        for at in 0..ends.len() {
            let Some(cycle) = self.keys[at].cycle else { continue };
            let last = ends.get(at + 1).map_or(u32::MAX, |next| next - 1);
            if cycle.from <= to && from <= last {
                self.keys[at].cycle = None;
                cleared = true;
            }
        }
        cleared
    }

    /// The value on `frame`: `None` for a track with no keys. Before the first key it is that
    /// key's value, after the last the last's. A `spatial` pair is a place: it moves along a
    /// smooth path through its keys, at a speed the eases shape, where a plain pair has each
    /// number go its own way.
    ///
    /// After a key that ends a cycle the value is the cycle's, again and again, until the
    /// next key.
    pub fn at(&self, frame: u32, spatial: bool) -> Option<V> {
        if let Some(repeated) = self.cycled(frame) {
            return self.at(repeated, spatial);
        }
        let (first, last) = (self.keys.first()?, self.keys.last()?);
        if frame <= first.frame {
            return Some(first.value);
        }
        if frame >= last.frame {
            return Some(last.value);
        }
        let before = self.keys.partition_point(|key| key.frame <= frame) - 1;
        if self.keys[before].frame == frame {
            return Some(self.keys[before].value);
        }
        // The two keys around the frame, and for the bend of a path their neighbours.
        let around = &self.keys[before.saturating_sub(1)..(before + 3).min(self.keys.len())];
        let (key, next) = (&self.keys[before], &self.keys[before + 1]);
        if key.chart.is_empty() || key.out == Interp::Hold {
            return V::from_value(&kf::evaluate(&keyframes(around, spatial), tick(frame), spatial)?);
        }
        // By a chart: the value of the steady way at the time it is as far along as the chart
        // says, which can fall between two frames.
        let mut steady = around.to_vec();
        let pair = before - before.saturating_sub(1);
        (steady[pair].out, steady[pair + 1].into) = (Interp::Linear, Interp::Linear);
        let span = f64::from(next.frame - key.frame);
        let part = charted(&key.chart, f64::from(frame - key.frame) / span);
        let time = Tick(((f64::from(key.frame) + part * span) * TICKS_PER_SECOND as f64).round() as i64);
        V::from_value(&kf::evaluate(&keyframes(&steady, spatial), time, spatial)?)
    }
}

impl Track<[f64; 2]> {
    /// [`Track::go_on_as`] for a place that moves along a path. A smooth path turns at each
    /// key as the keys beside it ask, so the jump would bend the way towards it and away
    /// from it. The two keys on each side of the jump therefore keep the handles they had,
    /// as bends of their own, and the keys the jump makes are corners.
    pub fn path_goes_on_as(&mut self, cut: u32, before: [f64; 2], later: &Track<[f64; 2]>, after: [f64; 2]) {
        if self.is_empty() && later.is_empty() && before == after {
            return;
        }
        let had = |track: &Track<[f64; 2]>, key: &Key<[f64; 2]>| (key.frame, key.bend.or_else(|| track.handles(key.frame)).unwrap_or_default());
        let mine = self.keys.iter().rev().filter(|key| key.frame < cut).take(2).map(|key| had(self, key));
        let theirs = later.keys.iter().filter(|key| key.frame >= cut).take(2).map(|key| had(later, key));
        let kept: Vec<(u32, Bend)> = mine.chain(theirs).collect();
        self.go_on_as(cut, before, later, after);
        for key in self.keys.iter_mut().filter(|key| (cut.saturating_sub(1)..=cut).contains(&key.frame)) {
            key.bend = Some(Bend::default());
        }
        for (frame, bend) in kept {
            if let Some(key) = self.key_mut(frame) {
                key.bend = Some(bend);
            }
        }
    }

    /// The handles of the path at the key on `frame`, as they are shown: the key's own bend,
    /// or where it has none, the handles of the smooth path through it.
    pub fn handles(&self, frame: u32) -> Option<Bend> {
        let at = self.index(frame).ok()?;
        let ([ix, iy, _], [ox, oy, _]) = kf::spatial_handles(&keyframes(&self.keys, true), at);
        Some(Bend { into: [ix, iy], out: [ox, oy] })
    }
}

/// Keys as the interpolation takes them. Its eases are speeds in the value's own units; ours
/// are in the measure of the way between two keys, so they are worked out a pair at a time.
fn keyframes<V: Animatable>(keys: &[Key<V>], spatial: bool) -> Vec<kf::Keyframe> {
    let mut made: Vec<kf::Keyframe> = keys.iter().map(|key| key.keyframe()).collect();
    for (at, pair) in keys.windows(2).enumerate() {
        let frames = f64::from(pair[1].frame - pair[0].frame).max(1.0);
        // The steady speed over the way: of each number, or of a place along its path.
        let (from, to) = (made[at].value.components(), made[at + 1].value.components());
        let steady: Vec<f64> = match spatial && from.len() > 1 {
            true => vec![kf::spatial_segment_length(&made, at) / frames],
            false => from.iter().zip(&to).map(|(from, to)| (to - from) / frames).collect(),
        };
        let ease = |slope: Slope| steady.iter().map(|steady| kf::Ease { speed: steady * slope.speed, influence: slope.reach.clamp(0.001, 1.0) }).collect();
        if let (Some(slope), false) = (pair[0].out.slope(), pair[0].out == Interp::Linear) {
            (made[at].out_interp, made[at].out_ease) = (kf::Interp::Bezier, ease(slope));
        }
        if let (Some(slope), false) = (pair[1].into.slope(), pair[1].into == Interp::Linear) {
            (made[at + 1].in_interp, made[at + 1].in_ease) = (kf::Interp::Bezier, ease(slope));
        }
    }
    made
}

impl<V: Animatable> Key<V> {
    /// The key without its eases, which depend on the keys beside it.
    fn keyframe(&self) -> kf::Keyframe {
        let mut key = kf::Keyframe::new(tick(self.frame), self.value.to_value());
        if self.out == Interp::Hold {
            key.out_interp = kf::Interp::Hold;
        }
        if let Some(Bend { into, out }) = self.bend {
            key.spatial_auto = false;
            (key.spatial_in, key.spatial_out) = ([into[0], into[1], 0.0], [out[0], out[1], 0.0]);
        }
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(keys: &[(u32, f64)]) -> Track<f64> {
        let mut track = Track::default();
        for (frame, value) in keys {
            track.set(*frame, *value);
        }
        track
    }

    #[test]
    fn a_value_goes_from_key_to_key_and_holds_outside_them() {
        assert_eq!(Track::<f64>::default().at(5, false), None);
        let track = track(&[(20, 100.0), (10, 0.0)]);
        assert_eq!(track.keys().iter().map(|k| k.frame).collect::<Vec<_>>(), [10, 20], "keys are kept in frame order");
        let values: Vec<f64> = [0, 10, 15, 20, 99].iter().map(|f| track.at(*f, false).unwrap()).collect();
        assert_eq!(values, [0.0, 0.0, 50.0, 100.0, 100.0]);
    }

    #[test]
    fn a_value_moves_on_towards_a_later_key_unless_it_is_held() {
        let mut track = track(&[(10, 0.0), (20, 100.0), (30, 50.0)]);
        track.key_mut(20).unwrap().out = Interp::Hold;
        let moves: Vec<bool> = [0, 10, 15, 20, 25, 30].iter().map(|frame| track.moves_after(*frame)).collect();
        assert_eq!(moves, [false, true, true, false, false, false]);
    }

    #[test]
    fn an_eased_key_is_left_slowly_and_a_held_one_not_until_the_next() {
        let mut eased = track(&[(0, 0.0), (10, 100.0)]);
        (eased.key_mut(0).unwrap().out, eased.key_mut(10).unwrap().into) = (Interp::Ease, Interp::Ease);
        let (early, middle, late) = (eased.at(2, false).unwrap(), eased.at(5, false).unwrap(), eased.at(8, false).unwrap());
        assert!(early < 15.0 && (middle - 50.0).abs() < 1e-6 && late > 85.0, "slow out, slow in: {early} {middle} {late}");
        // Easing only out of the first key still arrives at speed.
        eased.key_mut(10).unwrap().into = Interp::Linear;
        assert!(eased.at(2, false).unwrap() < 15.0 && eased.at(8, false).unwrap() < 85.0);

        let mut held = track(&[(0, 0.0), (10, 100.0)]);
        held.key_mut(0).unwrap().out = Interp::Hold;
        assert_eq!((held.at(9, false), held.at(10, false)), (Some(0.0), Some(100.0)));
        // A slope of its own: leaving at twice the steady speed, the value is ahead of the line.
        let mut fast = track(&[(0, 0.0), (10, 100.0)]);
        fast.key_mut(0).unwrap().out = Interp::Curve(Slope { reach: 0.5, speed: 2.0 });
        assert!(fast.at(3, false).unwrap() > 40.0 && fast.at(10, false) == Some(100.0), "{:?}", fast.at(3, false));
        // The same slope shapes a pair of numbers alike, and the speed of a place on its path.
        let mut pair: Track<[f64; 2]> = Track::default();
        pair.set(0, [0.0, 50.0]);
        pair.set(10, [100.0, 0.0]);
        pair.key_mut(0).unwrap().out = Interp::Curve(Slope { reach: 0.5, speed: 2.0 });
        let ([x, y], [px, py]) = (pair.at(3, false).unwrap(), pair.at(3, true).unwrap());
        assert!(x > 40.0 && (y - (50.0 - x / 2.0)).abs() < 1e-6 && (px - x).abs() < 0.5 && (py - y).abs() < 0.5, "{x} {y} {px} {py}");
        // Setting a key that is there keeps how it eases.
        held.set(0, 7.0);
        assert_eq!(held.at(9, false), Some(7.0));
    }

    #[test]
    fn keys_can_be_moved_and_removed() {
        let mut track = track(&[(0, 0.0), (10, 10.0), (20, 20.0)]);
        track.move_key(0, 15);
        assert_eq!(track.keys().iter().map(|k| (k.frame, k.value)).collect::<Vec<_>>(), [(10, 10.0), (15, 0.0), (20, 20.0)]);
        track.move_key(15, 20);
        assert_eq!(track.keys().iter().map(|k| (k.frame, k.value)).collect::<Vec<_>>(), [(10, 10.0), (20, 0.0)], "onto a key replaces it");
        track.move_key(3, 4);
        assert_eq!(track.remove(10).map(|k| k.value), Some(10.0));
        assert!(track.remove(10).is_none() && track.key(20).is_some() && track.keys().len() == 1);
    }

    fn near(a: &[f64], b: &[f64]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-9)
    }

    #[test]
    fn a_chart_times_the_way_between_two_keys_through_its_points() {
        // Two keys with five frames between them: five lines, evenly spaced on a steady way.
        let mut track = track(&[(0, 0.0), (6, 60.0)]);
        assert!(near(&track.spacing(0).unwrap(), &[1.0 / 6.0, 2.0 / 6.0, 0.5, 4.0 / 6.0, 5.0 / 6.0]));
        assert_eq!((track.spacing(6), track.spacing(3)), (None, None), "no way after the last key, and no key on 3");
        // An eased way is spaced as its eases shape it, whatever the value.
        track.key_mut(0).unwrap().out = Interp::Ease;
        let eased = track.spacing(0).unwrap();
        assert!(eased.iter().zip(1..).all(|(part, frame)| (part * 60.0 - track.at(frame, false).unwrap()).abs() < 1e-9), "{eased:?}");
        // A chart: the value on each frame is as far along as its line, and the eases are not used.
        let chart = [0.05, 0.1, 0.2, 0.6, 0.9];
        assert!(track.set_chart(0, &chart));
        assert!(near(&track.spacing(0).unwrap(), &chart));
        let values: Vec<f64> = (1..6).map(|frame| track.at(frame, false).unwrap()).collect();
        assert!(near(&values, &[3.0, 6.0, 12.0, 36.0, 54.0]), "{values:?}");
        // Kept in order and from 0 to 1: a line can't go back past the one before it.
        track.set_chart(0, &[0.3, 0.1, 1.5]);
        assert!(near(&track.spacing(0).unwrap()[..3], &[0.3, 0.3, 1.0]));
        // Taken away, the way is as its eases shape it again.
        track.set_chart(0, &[]);
        assert!(near(&track.spacing(0).unwrap(), &eased));
        assert!(!track.set_chart(6, &chart) && !track.set_chart(2, &chart));
    }

    #[test]
    fn a_chart_moves_a_place_along_its_path_without_changing_the_path() {
        let mut path: Track<[f64; 2]> = Track::default();
        for (frame, at) in [(0, [0.0, 0.0]), (10, [100.0, 100.0]), (20, [200.0, 0.0])] {
            path.set(frame, at);
        }
        let steady: Vec<[f64; 2]> = (0..=10).map(|frame| path.at(frame, true).unwrap()).collect();
        // An even chart is the steady way.
        let even: Vec<f64> = (1..10).map(|frame| f64::from(frame) / 10.0).collect();
        path.set_chart(0, &even);
        for frame in 1..10 {
            let ([x, y], [sx, sy]) = (path.at(frame, true).unwrap(), steady[frame as usize]);
            assert!((x - sx).abs() < 1e-6 && (y - sy).abs() < 1e-6, "frame {frame}");
        }
        // Half way along on frame 1 is where the steady way is on frame 5, between the frames
        // too: a quarter of the way is at frame 2.5.
        let mut chart = even.clone();
        chart[0] = 0.5;
        path.set_chart(0, &chart);
        let ([x, y], [sx, sy]) = (path.at(1, true).unwrap(), steady[5]);
        assert!((x - sx).abs() < 1e-6 && (y - sy).abs() < 1e-6);
        chart[0] = 0.25;
        path.set_chart(0, &chart);
        let ([x, _], [before, _], [after, _]) = (path.at(1, true).unwrap(), steady[2], steady[3]);
        assert!(x > before && x < after, "{before} < {x} < {after}");
    }

    #[test]
    fn a_chart_is_split_joined_and_stretched_as_its_keys_change() {
        let mut track = track(&[(0, 0.0), (6, 60.0)]);
        track.set_chart(0, &[0.05, 0.1, 0.2, 0.6, 0.9]);
        let place = |track: &Track<f64>, frame: u32| track.at(frame, false).unwrap();
        let before: Vec<f64> = (1..6).map(|frame| place(&track, frame)).collect();
        // A key put on frame 3, on the way as it was: the value on every frame stays, each line
        // measured anew as a part of its own way.
        track.set(3, place(&track, 3));
        let after: Vec<f64> = (1..6).map(|frame| place(&track, frame)).collect();
        assert!(near(&before, &after), "{before:?} {after:?}");
        assert!(near(&track.spacing(0).unwrap(), &[0.25, 0.5]) && near(&track.spacing(3).unwrap(), &[0.5, 0.875]));
        // Taken away again, the two charts are one: each line keeps its place in time, and the
        // key's frame is a line of its own, at its frame.
        track.remove(3);
        let joined = track.spacing(0).unwrap();
        let places: Vec<f64> = [0.25, 0.5].iter().map(|part| part * 3.0).chain([3.0]).chain([0.5, 0.875].iter().map(|part| 3.0 + part * 3.0)).collect();
        assert!(near(&joined, &places.iter().map(|place| place / 6.0).collect::<Vec<_>>()), "{joined:?}");
        track.set_chart(0, &[0.05, 0.1, 0.2, 0.6, 0.9]);
        // The last key moved later stretches the chart: the same shape over more frames.
        track.move_key(6, 12);
        let stretched = track.spacing(0).unwrap();
        assert_eq!(stretched.len(), 11);
        assert!(near(&[stretched[1], stretched[5], stretched[9]], &[0.05, 0.2, 0.9]), "{stretched:?}");
        // A way with no chart is joined as it is, without one.
        let mut plain = self::track(&[(0, 0.0), (5, 50.0), (10, 0.0)]);
        plain.remove(5);
        assert!(plain.key(0).unwrap().chart.is_empty());
        // Held steps leave no chart.
        track.step(0, 12, 2, false);
        assert!(track.keys().iter().all(|key| key.chart.is_empty()));
    }

    #[test]
    fn a_dragged_line_squashes_the_lines_towards_its_key_by_halves_or_thirds() {
        let even = [1.0 / 6.0, 2.0 / 6.0, 0.5, 4.0 / 6.0, 5.0 / 6.0];
        // The third line dragged towards the first key: the first two follow, the rest stay.
        assert!(near(&squash(&even, 2, 0.4, Spacing::Halves, false), &[0.1, 0.2, 0.4, 4.0 / 6.0, 5.0 / 6.0]));
        assert!(near(&squash(&even, 2, 0.3, Spacing::Thirds, false), &[0.1, 0.2, 0.3, 4.0 / 6.0, 5.0 / 6.0]));
        // Towards the last key: the two after it follow, bunched towards the key by halves.
        assert!(near(&squash(&even, 2, 0.6, Spacing::Halves, false), &[1.0 / 6.0, 2.0 / 6.0, 0.6, 0.8, 0.9]));
        // With the other side too, both are spaced again.
        assert!(near(&squash(&even, 2, 0.6, Spacing::Thirds, true), &[0.2, 0.4, 0.6, 0.6 + 0.4 / 3.0, 0.6 + 0.8 / 3.0]));
        // Halves from three lines away: each halfway between the key and the line after it.
        assert!(near(&squash(&[0.2, 0.4, 0.6, 0.8], 3, 0.4, Spacing::Halves, false), &[0.05, 0.1, 0.2, 0.4]));
        // By hand: the dragged line alone, held between the ones beside it, the other side too.
        assert!(near(&squash(&even, 2, 0.4, Spacing::Manual, true), &[1.0 / 6.0, 2.0 / 6.0, 0.4, 4.0 / 6.0, 5.0 / 6.0]));
        assert!(near(&squash(&even, 2, 0.1, Spacing::Manual, true), &[1.0 / 6.0, 2.0 / 6.0, 2.0 / 6.0, 4.0 / 6.0, 5.0 / 6.0]));
        // Never past a key, and never back past another line.
        let far = squash(&even, 4, 2.0, Spacing::Halves, true);
        assert!(far[4] == 1.0 && far.windows(2).all(|pair| pair[0] <= pair[1]), "{far:?}");
    }

    #[test]
    fn a_cycle_repeats_its_keys_until_the_next_key() {
        let mut track = track(&[(0, 0.0), (4, 40.0), (20, 100.0)]);
        assert!(track.set_cycle(0, 4, CycleKind::Loop, false));
        let values: Vec<f64> = (4..=10).map(|frame| track.at(frame, false).unwrap()).collect();
        assert_eq!(values, [40.0, 10.0, 20.0, 30.0, 0.0, 10.0, 20.0], "the frame after the last is the one after the first");
        assert_eq!((track.at(19, false), track.at(20, false)), (Some(30.0), Some(100.0)), "up to the next key");
        assert_eq!(track.repeated_keys(30), [8, 12, 16], "the keys repeated, not past the next");
        // Back and forth, and for ever after the last key.
        track.remove(20);
        track.set_cycle(0, 4, CycleKind::PingPong, false);
        let values: Vec<f64> = (4..=13).map(|frame| track.at(frame, false).unwrap()).collect();
        assert_eq!(values, [40.0, 30.0, 20.0, 10.0, 0.0, 10.0, 20.0, 30.0, 40.0, 30.0]);
        assert_eq!(track.repeated_keys(14), [8, 12]);
        assert_eq!((track.cycle_of(2), track.cycle_of(9)), (Some((0, 4, CycleKind::PingPong)), Some((0, 4, CycleKind::PingPong))));
        // Its first key moved, the cycle starts with it.
        track.move_key(0, 1);
        assert_eq!(track.key(4).unwrap().cycle, Some(Cycle { from: 1, kind: CycleKind::PingPong }));
        assert!(track.clear_cycles(9, 9) && track.cycle_of(2).is_none() && track.at(9, false) == Some(40.0));
        // A key is made to end a cycle where there is none, of the value there.
        let mut held = self::track(&[(0, 0.0), (2, 20.0)]);
        assert!(held.set_cycle(0, 6, CycleKind::Loop, false) && held.key(6).is_some_and(|key| key.value == 20.0));
        assert!(!held.set_cycle(8, 12, CycleKind::Loop, false), "no key there");
    }

    #[test]
    fn a_place_moves_along_a_smooth_path_through_its_keys() {
        let mut path: Track<[f64; 2]> = Track::default();
        for (frame, at) in [(0, [0.0, 0.0]), (10, [100.0, 100.0]), (20, [200.0, 0.0])] {
            path.set(frame, at);
        }
        assert_eq!(path.at(10, true), Some([100.0, 100.0]), "through every key");
        // Up to the middle key the straight way is the line y = x; the path swings off it, to
        // pass through the key level. As two plain numbers it would go straight.
        let [x, y] = path.at(5, true).unwrap();
        assert!((y - x).abs() > 5.0 && x > 30.0 && x < 70.0, "{x}, {y}");
        assert_eq!(path.at(5, false), Some([50.0, 50.0]));
        // A corner at the middle key: straight there and straight on.
        let smooth = path.handles(10).unwrap();
        assert!(smooth.out[0] > 10.0 && smooth.out[1].abs() < 1e-9 && smooth.into[0] < -10.0, "level through the key: {smooth:?}");
        path.key_mut(10).unwrap().bend = Some(Bend::default());
        let [x, y] = path.at(5, true).unwrap();
        assert!((y - x).abs() < 1e-6 && (x - 50.0).abs() < 1e-6, "{x}, {y}");
        // Its own handles: leaving straight down, the path dips below the key before it rises.
        path.key_mut(10).unwrap().bend = Some(Bend { into: [0.0, -40.0], out: [0.0, 40.0] });
        assert!(path.at(11, true).unwrap()[1] > 100.0);
        assert_eq!(path.handles(10), Some(Bend { into: [0.0, -40.0], out: [0.0, 40.0] }));
        path.key_mut(10).unwrap().bend = None;
        // Two keys alone make a straight path.
        path.remove(20);
        let [x, y] = path.at(5, true).unwrap();
        assert!((x - 50.0).abs() < 1e-6 && (y - 50.0).abs() < 1e-6);
    }
}
