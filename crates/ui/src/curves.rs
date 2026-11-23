//! The function curve editor: one value of a peg, one number of an effect node, or one value
//! of a pose of bones or of a curve's points, over the frames of the shot, as a curve. A pose
//! eases as a whole: a handle shaped on one of its values shapes all of them on that frame. Its
//! keyframes are squares; dragging one up or down changes the value there. Beside each
//! keyframe are the handles of the way to the next and from the last: dragging one shapes how
//! the value gets from key to key, slow or fast at the key, and for how much of the way.
//!
//! A way timed by a chart (the timeline's breakdown lines) has no handles: its in-betweens are
//! points on the curve, in the breakdowns' colour, dragged up or down to say how far along
//! the way the value is on their frames; the others follow by halves or thirds, as the
//! timeline's Breakdown menu has it.
//!
//! A position is a place on a path, shaped in the viewport. Its curve here is how far along
//! the path the peg has come, so its handles are the same slow in and slow out, and its
//! keyframes are moved in the viewport and the timeline, not here.

use egui::{Align2, CursorIcon, Pos2, Rect, Sense, Stroke, pos2, vec2};
use iskra_model::{Channel, Deformer, Effect, Interp, ParamKind, Peg, Slope, Track};
use serde_json::json;

use crate::theme::{self, Theme};
use crate::timeline::Action;

/// What is plotted: a value that has a curve of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Curve {
    /// How far along its path the peg has come.
    #[default]
    Position,
    Rotation,
    ScaleX,
    ScaleY,
    Skew,
    Depth,
}

impl Curve {
    pub const ALL: [Curve; 6] = [Curve::Position, Curve::Rotation, Curve::ScaleX, Curve::ScaleY, Curve::Skew, Curve::Depth];

    fn title(self) -> &'static str {
        match self {
            Curve::Position => "Along the Path",
            Curve::Rotation => "Rotation",
            Curve::ScaleX => "Scale Across",
            Curve::ScaleY => "Scale Up",
            Curve::Skew => "Skew",
            Curve::Depth => "Depth",
        }
    }

    /// The track its keyframes are on. The two scales share theirs.
    pub fn channel(self) -> Channel {
        match self {
            Curve::Position => Channel::Position,
            Curve::Rotation => Channel::Rotation,
            Curve::ScaleX | Curve::ScaleY => Channel::Scale,
            Curve::Skew => Channel::Skew,
            Curve::Depth => Channel::Depth,
        }
    }

    /// The value on every frame of a shot of `frames` frames.
    fn samples(self, peg: &Peg, frames: u32) -> Vec<f64> {
        let poses = (0..frames).map(|frame| peg.transform_at(frame));
        match self {
            Curve::Rotation => poses.map(|pose| pose.rotation).collect(),
            Curve::ScaleX => poses.map(|pose| pose.scale[0]).collect(),
            Curve::ScaleY => poses.map(|pose| pose.scale[1]).collect(),
            Curve::Skew => poses.map(|pose| pose.skew).collect(),
            Curve::Depth => poses.map(|pose| pose.depth).collect(),
            // The way come so far, a frame's step at a time.
            Curve::Position => {
                let places: Vec<[f64; 2]> = poses.map(|pose| pose.position).collect();
                let steps = places.windows(2).map(|pair| (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]));
                std::iter::once(0.0)
                    .chain(steps.scan(0.0, |sum, step| {
                        *sum += step;
                        Some(*sum)
                    }))
                    .collect()
            }
        }
    }
}

/// The most frames that are plotted.
const MAX_FRAMES: u32 = 5000;
/// The width of the list of values beside the plot.
const LIST_WIDTH: f32 = 118.0;
/// The room left above and below the curve, for handles that reach past it.
const MARGIN: f32 = 22.0;
/// How near the pointer has to be to a keyframe or a handle to take it, in points.
const REACH: f32 = 7.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// A keyframe's square: its value.
    Value,
    /// A handle: the slope out of the keyframe, or into it.
    Slope { out: bool },
    /// A point of the chart of the way from the keyframe: which in-between.
    Chart { line: usize },
}

/// A value of a pose of bones that is plotted: how far the chain's start is moved from its
/// rest, each way, and of the bone at a place how far it is turned and how long it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    MoveX,
    MoveY,
    Turn(usize),
    Length(usize),
    /// Where a point of a curve or an envelope is, across and down, in the layer's own space.
    PointX(usize),
    PointY(usize),
}

impl Pose {
    fn title(self) -> String {
        match self {
            Pose::MoveX => "Move X".to_string(),
            Pose::MoveY => "Move Y".to_string(),
            Pose::Turn(bone) => format!("Bone {} turn", bone + 1),
            Pose::Length(bone) => format!("Bone {} length", bone + 1),
            Pose::PointX(knot) => format!("Point {} across", knot + 1),
            Pose::PointY(knot) => format!("Point {} down", knot + 1),
        }
    }
}

/// What a curve is of: a value of a peg, a number of an effect node (plan §8.3), or a value
/// of a pose of bones. The plot is the same for all; this says how each is read, changed,
/// and set in the project.
#[derive(Clone, PartialEq)]
enum Subject {
    Peg(Box<Peg>, Curve),
    Effect(Effect, &'static str),
    Bones(Box<Deformer>, Pose),
}

/// The keys of one value of a pose, each with its frame and how it eases.
fn pose_keys(bones: &Deformer, pose: Pose) -> Vec<(u32, Interp, Interp)> {
    let of = |keys: Vec<(u32, Interp, Interp)>| keys;
    match pose {
        Pose::MoveX | Pose::MoveY => of(bones.offsets.keys().iter().map(|key| (key.frame, key.into, key.out)).collect()),
        Pose::Turn(bone) => of(bones.bones.get(bone).into_iter().flat_map(|bone| bone.turns.keys()).map(|key| (key.frame, key.into, key.out)).collect()),
        Pose::Length(bone) => of(bones.bones.get(bone).into_iter().flat_map(|bone| bone.stretches.keys()).map(|key| (key.frame, key.into, key.out)).collect()),
        Pose::PointX(knot) | Pose::PointY(knot) => {
            of(bones.spine.get(knot).into_iter().flat_map(|knot| knot.moves.keys()).map(|key| (key.frame, key.into, key.out)).collect())
        }
    }
}

/// One value of a pose on a frame.
fn pose_value(bones: &Deformer, pose: Pose, frame: u32) -> f64 {
    match pose {
        Pose::MoveX => bones.offset_at(frame)[0],
        Pose::MoveY => bones.offset_at(frame)[1],
        Pose::Turn(bone) => bones.bones.get(bone).map_or(0.0, |bone| bone.turn_at(frame)),
        Pose::Length(bone) => bones.bones.get(bone).map_or(1.0, |bone| bone.stretch_at(frame)),
        Pose::PointX(knot) => bones.joints(Some(frame)).get(knot).map_or(0.0, |at| at.x),
        Pose::PointY(knot) => bones.joints(Some(frame)).get(knot).map_or(0.0, |at| at.y),
    }
}

impl Subject {
    /// The value on every frame of a shot of `frames` frames.
    fn samples(&self, frames: u32) -> Vec<f64> {
        match self {
            Subject::Peg(peg, curve) => curve.samples(peg, frames),
            Subject::Effect(effect, param) => (0..frames).map(|frame| effect.number(param, frame)).collect(),
            Subject::Bones(bones, pose) => (0..frames).map(|frame| pose_value(bones, *pose, frame)).collect(),
        }
    }

    /// The frames that have a keyframe of it, in their order.
    fn key_frames(&self) -> Vec<u32> {
        match self {
            Subject::Peg(peg, curve) => peg.animation.frames(Some(curve.channel())).into_iter().collect(),
            Subject::Effect(effect, param) => effect.key_frames(Some(param)).into_iter().collect(),
            Subject::Bones(bones, pose) => pose_keys(bones, *pose).into_iter().map(|(frame, ..)| frame).collect(),
        }
    }

    /// How its keyframe on `frame` eases, in and out.
    fn interp(&self, frame: u32) -> Option<(Interp, Interp)> {
        match self {
            Subject::Peg(peg, curve) => peg.animation.interp_of(frame, Some(curve.channel())),
            Subject::Effect(effect, param) => effect.interp(frame, Some(param)),
            Subject::Bones(bones, pose) => pose_keys(bones, *pose).into_iter().find(|(at, ..)| *at == frame).map(|(_, into, out)| (into, out)),
        }
    }

    /// Whether a keyframe's value is changed here. A place is moved on its path, in the
    /// viewport; here its keyframes only show.
    fn valued(&self) -> bool {
        !matches!(self, Subject::Peg(_, Curve::Position))
    }

    /// Give the keyframe on `frame` a new value.
    fn set_value(&mut self, frame: u32, value: f64) {
        match self {
            Subject::Peg(peg, curve) => set_value(peg, *curve, frame, value),
            Subject::Effect(effect, param) => {
                // Held to the parameter's range, as the engine will hold it.
                let within = match effect.described().and_then(|kind| kind.param(param)).map(|spec| spec.kind) {
                    Some(ParamKind::Number { min, max, .. }) => value.clamp(min, max),
                    _ => value,
                };
                if let Some(held) = effect.param_mut(param) {
                    held.keys.set(frame, within);
                }
            }
            Subject::Bones(bones, pose) => match *pose {
                Pose::MoveX | Pose::MoveY => {
                    let mut moved = bones.offset_at(frame);
                    moved[usize::from(*pose == Pose::MoveY)] = value;
                    bones.offsets.set(frame, moved);
                }
                Pose::Turn(bone) => bones.bones.get_mut(bone).into_iter().for_each(|bone| bone.turns.set(frame, value)),
                // A bone is never of no length.
                Pose::Length(bone) => bones.bones.get_mut(bone).into_iter().for_each(|bone| bone.stretches.set(frame, value.max(0.01))),
                Pose::PointX(knot) | Pose::PointY(knot) => {
                    if let Some(at) = bones.joints(Some(frame)).get(knot).copied() {
                        let to = if matches!(pose, Pose::PointX(_)) { [value, at.y] } else { [at.x, value] };
                        bones.pose_knot(knot, frame, to);
                    }
                }
            },
        }
    }

    /// Set how the keyframe on `frame` eases; a side that is `None` stays as it is.
    fn set_interp(&mut self, frame: u32, into: Option<Interp>, out: Option<Interp>) {
        match self {
            Subject::Peg(peg, curve) => peg.animation.set_interp(frame, Some(curve.channel()), into, out),
            Subject::Effect(effect, param) => {
                if let Some(key) = effect.params.get_mut(*param).and_then(|held| held.keys.key_mut(frame)) {
                    (key.into, key.out) = (into.unwrap_or(key.into), out.unwrap_or(key.out));
                }
            }
            // A pose eases as a whole, all of its values on the frame together.
            Subject::Bones(bones, _) => bones.set_interp(frame, into, out),
        }
    }

    /// The command that gives the project the value this has on `frame`.
    fn value_command(&self, frame: u32) -> (&'static str, serde_json::Value) {
        match self {
            Subject::Peg(peg, curve) => {
                let (mut params, now) = (json!({ "peg": peg.id.0, "frame": frame }), peg.transform_at(frame));
                match curve {
                    Curve::Rotation => params["rotation"] = json!(now.rotation),
                    Curve::ScaleX | Curve::ScaleY => params["scale"] = json!(now.scale),
                    Curve::Skew => params["skew"] = json!(now.skew),
                    Curve::Depth => params["depth"] = json!(now.depth),
                    Curve::Position => {}
                }
                ("peg.setTransform", params)
            }
            Subject::Effect(effect, param) => {
                ("effect.set", json!({ "effect": effect.id.0, "param": param, "value": effect.number(param, frame), "frame": frame }))
            }
            Subject::Bones(bones, pose) => {
                let mut params = json!({ "deformer": bones.id.0, "frame": frame });
                match *pose {
                    Pose::MoveX | Pose::MoveY => params["offset"] = json!(bones.offset_at(frame)),
                    Pose::Turn(bone) => (params["bone"], params["turn"]) = (json!(bone), json!(pose_value(bones, *pose, frame))),
                    Pose::Length(bone) => (params["bone"], params["stretch"]) = (json!(bone), json!(pose_value(bones, *pose, frame))),
                    Pose::PointX(knot) | Pose::PointY(knot) => {
                        let at = bones.joints(Some(frame)).get(knot).map_or([0.0; 2], |at| [at.x, at.y]);
                        (params["knot"], params["to"]) = (json!(knot), json!(at));
                    }
                }
                ("deformer.setPose", params)
            }
        }
    }

    /// The command that gives the project the ease this has on one side of `frame`.
    fn ease_command(&self, frame: u32, out: bool) -> (&'static str, serde_json::Value) {
        let side = self.interp(frame).map(|(into, leaving)| if out { leaving } else { into });
        let (id, mut params) = match self {
            Subject::Peg(peg, curve) => ("peg.setEase", json!({ "peg": peg.id.0, "frame": frame, "channel": curve.channel() })),
            Subject::Effect(effect, param) => ("effect.setEase", json!({ "effect": effect.id.0, "frame": frame, "param": param })),
            Subject::Bones(bones, _) => ("deformer.setEase", json!({ "deformer": bones.id.0, "frame": frame })),
        };
        params[if out { "out" } else { "into" }] = json!(side);
        (id, params)
    }

    /// The value's track: of a number, or of a pair, as `one` or `two` reads it.
    fn track<R>(&self, one: impl FnOnce(&Track<f64>) -> R, two: impl FnOnce(&Track<[f64; 2]>) -> R) -> Option<R> {
        Some(match self {
            Subject::Peg(peg, curve) => match curve.channel() {
                Channel::Position => two(&peg.animation.position),
                Channel::Rotation => one(&peg.animation.rotation),
                Channel::Scale => two(&peg.animation.scale),
                Channel::Skew => one(&peg.animation.skew),
                Channel::Depth => one(&peg.animation.depth),
                Channel::Tilt => two(&peg.animation.tilt),
            },
            Subject::Effect(effect, param) => one(&effect.params.get(*param)?.keys),
            Subject::Bones(bones, pose) => match *pose {
                Pose::MoveX | Pose::MoveY => two(&bones.offsets),
                Pose::Turn(bone) => one(&bones.bones.get(bone)?.turns),
                Pose::Length(bone) => one(&bones.bones.get(bone)?.stretches),
                Pose::PointX(knot) | Pose::PointY(knot) => two(&bones.spine.get(knot)?.moves),
            },
        })
    }

    /// [`Subject::track`], to be changed.
    fn track_mut<R>(&mut self, one: impl FnOnce(&mut Track<f64>) -> R, two: impl FnOnce(&mut Track<[f64; 2]>) -> R) -> Option<R> {
        Some(match self {
            Subject::Peg(peg, curve) => match curve.channel() {
                Channel::Position => two(&mut peg.animation.position),
                Channel::Rotation => one(&mut peg.animation.rotation),
                Channel::Scale => two(&mut peg.animation.scale),
                Channel::Skew => one(&mut peg.animation.skew),
                Channel::Depth => one(&mut peg.animation.depth),
                Channel::Tilt => two(&mut peg.animation.tilt),
            },
            Subject::Effect(effect, param) => one(&mut effect.params.get_mut(*param)?.keys),
            Subject::Bones(bones, pose) => match *pose {
                Pose::MoveX | Pose::MoveY => two(&mut bones.offsets),
                Pose::Turn(bone) => one(&mut bones.bones.get_mut(bone)?.turns),
                Pose::Length(bone) => one(&mut bones.bones.get_mut(bone)?.stretches),
                Pose::PointX(knot) | Pose::PointY(knot) => two(&mut bones.spine.get_mut(knot)?.moves),
            },
        })
    }

    /// How the way from its keyframe on `from` is spaced, where a chart times it.
    fn chart(&self, from: u32) -> Option<Vec<f64>> {
        let charted = |chart: bool, spacing: Option<Vec<f64>>| spacing.filter(|_| chart);
        self.track(
            |track| charted(track.key(from).is_some_and(|key| !key.chart.is_empty()), track.spacing(from)),
            |track| charted(track.key(from).is_some_and(|key| !key.chart.is_empty()), track.spacing(from)),
        )
        .flatten()
    }

    fn set_chart(&mut self, from: u32, spacing: &[f64]) {
        self.track_mut(|track| track.set_chart(from, spacing), |track| track.set_chart(from, spacing));
    }

    /// The command that gives the project the chart this has of the way from `from` to `to`.
    fn chart_command(&self, from: u32, to: u32) -> (&'static str, serde_json::Value) {
        let (owner, value) = match self {
            Subject::Peg(peg, curve) => (json!({ "peg": peg.id.0 }), format!("{:?}", curve.channel()).to_lowercase()),
            Subject::Effect(effect, param) => (json!({ "effect": effect.id.0 }), param.to_string()),
            Subject::Bones(bones, pose) => (
                json!({ "deformer": bones.id.0 }),
                match *pose {
                    Pose::MoveX | Pose::MoveY => "offset".to_string(),
                    Pose::Turn(bone) => format!("bone{bone}.turn"),
                    Pose::Length(bone) => format!("bone{bone}.stretch"),
                    Pose::PointX(knot) | Pose::PointY(knot) => format!("point{knot}.move"),
                },
            ),
        };
        let spacing = self.chart(from).unwrap_or_default();
        ("keys.chart", json!({ "owner": owner, "value": value, "from": from, "to": to, "spacing": spacing }))
    }

    /// What the plot says of a value with no keyframes.
    fn unkeyed(&self) -> &'static str {
        match self {
            Subject::Peg(..) => "This value has no keyframes. Animation › Add Keyframe keys the peg on the current frame.",
            Subject::Effect(..) => "This number has no keyframes. Add Keyframe beside it in Properties keys it on the current frame.",
            Subject::Bones(..) => "This value has no keyframes. Animation › Add Keyframe keys the pose on the current frame.",
        }
    }
}

/// A keyframe or a handle being dragged. The engine gets one command when it is let go.
struct Drag {
    frame: u32,
    grab: Grab,
    /// What is plotted as the drag has it so far, which is what the plot shows meanwhile.
    edited: Subject,
    /// The values at the bottom and the top of the plot, kept as they were when the drag began
    /// so that the curve doesn't slide about under the pointer.
    range: (f64, f64),
}

#[derive(Default)]
pub struct Curves {
    pub curve: Curve,
    /// The number of an effect node that is plotted, by its id, when a node's are shown.
    pub param: Option<&'static str>,
    /// The value of a pose of bones that is plotted, when a chain's are shown.
    pub pose: Option<Pose>,
    /// How a chart's points are spaced again when one is dragged: the timelines', handed over
    /// before the curves are shown.
    pub breakdowns: crate::breakdown::Breakdowns,
    drag: Option<Drag>,
    /// Where the keyframes and the handles were drawn: frame, and for a handle whether it is the
    /// one out of the keyframe.
    #[cfg(test)]
    pub keys: Vec<(u32, Pos2)>,
    #[cfg(test)]
    pub handles: Vec<(u32, bool, Pos2)>,
    /// And the points of the charts: the way's first frame, which in-between, where.
    #[cfg(test)]
    pub chart_points: Vec<(u32, usize, Pos2)>,
}

/// A handle of the curve between two keyframes, in the plot's own measures.
struct Handle {
    /// The keyframe it belongs to, and whether it is the one out of it.
    frame: u32,
    out: bool,
    /// Where it is: a frame, with parts of a frame, and a value.
    at: (f64, f64),
}

/// The handles of the way from the keyframe on `from` to the one on `to`, whose values are
/// `v0` and `v1`.
fn handles(of: &Subject, (from, v0): (u32, f64), (to, v1): (u32, f64)) -> Vec<Handle> {
    let (frames, rise) = (f64::from(to - from), v1 - v0);
    let (Some((_, out)), Some((into, _))) = (of.interp(from), of.interp(to)) else {
        return Vec::new();
    };
    // A chart times the way, not the eases: its points are shown in their place.
    if of.chart(from).is_some() {
        return Vec::new();
    }
    // Out of a held keyframe the value doesn't move, so there is nothing to shape.
    let (Some(out), Some(into)) = (out.slope(), into.slope().or(Interp::Linear.slope())) else { return Vec::new() };
    vec![
        Handle { frame: from, out: true, at: (f64::from(from) + out.reach * frames, v0 + out.speed * out.reach * rise) },
        Handle { frame: to, out: false, at: (f64::from(to) - into.reach * frames, v1 - into.speed * into.reach * rise) },
    ]
}

impl Curves {
    /// Show the curves of `peg` in a shot of `frames` frames, with the playhead on `frame`.
    pub fn show(&mut self, ui: &mut egui::Ui, frames: u32, peg: &Peg, frame: u32) -> Vec<Action> {
        let mut actions = Vec::new();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(LIST_WIDTH, ui.available_height()), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.strong(&peg.name);
                for curve in Curve::ALL {
                    let keyed = !peg.animation.frames(Some(curve.channel())).is_empty();
                    let label = if keyed { curve.title().to_string() } else { format!("{} ·", curve.title()) };
                    let hint = if keyed { "Show this value's curve" } else { "This value has no keyframes: it is the same on every frame" };
                    if ui.selectable_label(self.curve == curve, label).on_hover_text(hint).clicked() {
                        (self.curve, self.drag) = (curve, None);
                    }
                }
            });
            self.plot(ui, frames.clamp(1, MAX_FRAMES), &Subject::Peg(Box::new(peg.clone()), self.curve), frame, &mut actions);
        });
        actions
    }

    /// Show the curves of an effect node's numbers (plan §8.3), as a peg's values are shown:
    /// the list picks the number.
    pub fn show_effect(&mut self, ui: &mut egui::Ui, frames: u32, effect: &Effect, frame: u32) -> Vec<Action> {
        let mut actions = Vec::new();
        let numbers: Vec<_> = effect
            .described()
            .into_iter()
            .flat_map(|kind| kind.params)
            .filter(|spec| matches!(spec.kind, ParamKind::Number { .. }) && effect.applies(spec.id, frame))
            .collect();
        // The number that was picked, if this node has it, or else its first.
        let param = numbers.iter().map(|spec| spec.id).find(|id| self.param == Some(*id)).or(numbers.first().map(|spec| spec.id));
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(LIST_WIDTH, ui.available_height()), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.strong(&effect.name);
                for spec in &numbers {
                    let keyed = !effect.key_frames(Some(spec.id)).is_empty();
                    let label = if keyed { spec.title.to_string() } else { format!("{} ·", spec.title) };
                    let hint = if keyed { "Show this number's curve" } else { "This number has no keyframes: it is the same on every frame" };
                    if ui.selectable_label(param == Some(spec.id), label).on_hover_text(hint).clicked() {
                        (self.param, self.drag) = (Some(spec.id), None);
                    }
                }
            });
            match param {
                Some(param) => self.plot(ui, frames.clamp(1, MAX_FRAMES), &Subject::Effect(effect.clone(), param), frame, &mut actions),
                None => drop(ui.weak("This node has no numbers to plot.")),
            }
        });
        actions
    }

    /// Show the curves of a pose of bones, as a peg's values are shown: how the chain's start
    /// is moved, and each bone's turn and length. The list picks the value.
    pub fn show_bones(&mut self, ui: &mut egui::Ui, frames: u32, bones: &Deformer, frame: u32) -> Vec<Action> {
        let mut actions = Vec::new();
        // A chain's bones, or a curve's or an envelope's points.
        let each: Vec<Pose> = match bones.is_curve() {
            true => (0..bones.spine.len()).flat_map(|knot| [Pose::PointX(knot), Pose::PointY(knot)]).collect(),
            false => (0..bones.bones.len()).flat_map(|bone| [Pose::Turn(bone), Pose::Length(bone)]).collect(),
        };
        let values: Vec<Pose> = [Pose::MoveX, Pose::MoveY].into_iter().chain(each).collect();
        // The value that was picked, if this chain has it, or else the first that has keys, or
        // the first bone's turn, or the first point's place.
        let keyed = values.iter().copied().find(|pose| !pose_keys(bones, *pose).is_empty());
        let first = keyed.or_else(|| values.iter().copied().find(|pose| matches!(pose, Pose::Turn(_) | Pose::PointX(_)))).unwrap_or(Pose::MoveX);
        let pose = self.pose.filter(|pose| values.contains(pose)).unwrap_or(first);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(LIST_WIDTH, ui.available_height()), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.strong(&bones.name);
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    for value in &values {
                        let keyed = !pose_keys(bones, *value).is_empty();
                        let label = if keyed { value.title() } else { format!("{} ·", value.title()) };
                        let hint = if keyed { "Show this value's curve" } else { "This value has no keyframes: it is the same on every frame" };
                        if ui.selectable_label(pose == *value, label).on_hover_text(hint).clicked() {
                            (self.pose, self.drag) = (Some(*value), None);
                        }
                    }
                });
            });
            self.plot(ui, frames.clamp(1, MAX_FRAMES), &Subject::Bones(Box::new(bones.clone()), pose), frame, &mut actions);
        });
        actions
    }

    fn plot(&mut self, ui: &mut egui::Ui, frames: u32, subject: &Subject, frame: u32, actions: &mut Vec<Action>) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let (rect, theme) = (response.rect, Theme::of(ui.ctx()));
        // A drag of something else than what is shown now is none.
        if self.drag.as_ref().is_some_and(|drag| std::mem::discriminant(&drag.edited) != std::mem::discriminant(subject)) {
            self.drag = None;
        }
        // While something is dragged, the plot is of it as the drag has it.
        let shown = self.drag.as_ref().map_or(subject, |drag| &drag.edited);
        let samples = shown.samples(frames);
        let range = self.drag.as_ref().map_or_else(
            || {
                let (low, high) = samples.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), v| (low.min(*v), high.max(*v)));
                // A value that doesn't change is a level line in the middle.
                if high - low < 1e-9 { (low - 1.0, low + 1.0) } else { (low, high) }
            },
            |drag| drag.range,
        );
        let step = rect.width() / frames as f32;
        let x = |at: f64| rect.left() + (at as f32 + 0.5) * step;
        let y = |value: f64| rect.bottom() - MARGIN - ((value - range.0) / (range.1 - range.0)) as f32 * (rect.height() - 2.0 * MARGIN);
        let frame_at = |pos: Pos2| f64::from((pos.x - rect.left()) / step - 0.5);
        let value_at = |pos: Pos2| range.0 + f64::from((rect.bottom() - MARGIN - pos.y) / (rect.height() - 2.0 * MARGIN).max(1.0)) * (range.1 - range.0);

        // The frames, every fifth numbered, and the values at the top and the bottom of the curve.
        for at in (0..frames).filter(|at| at % 5 == 4 || *at == 0) {
            painter.vline(x(f64::from(at)), rect.y_range(), Stroke::new(1.0, theme.border));
            painter.text(pos2(x(f64::from(at)) + 3.0, rect.top()), Align2::LEFT_TOP, at + 1, theme::mono(10.0), theme.text_muted);
        }
        for value in [range.0, range.1] {
            painter.hline(rect.x_range(), y(value), Stroke::new(1.0, theme.border));
            painter.text(pos2(rect.right() - 4.0, y(value) - 1.0), Align2::RIGHT_BOTTOM, format!("{value:.1}"), theme::mono(10.0), theme.text_muted);
        }
        painter.vline(x(f64::from(frame)), rect.y_range(), Stroke::new(1.5, theme.accent));

        let line: Vec<Pos2> = samples.iter().enumerate().map(|(at, value)| pos2(x(at as f64), y(*value))).collect();
        painter.add(egui::Shape::line(line, Stroke::new(1.5, theme.text)));

        let keys: Vec<(u32, f64)> = shown.key_frames().into_iter().filter(|at| *at < frames).map(|at| (at, samples[at as usize])).collect();
        if keys.is_empty() {
            painter.text(rect.center_top() + vec2(0.0, 16.0), Align2::CENTER_TOP, shown.unkeyed(), egui::FontId::proportional(11.0), theme.text_muted);
        }
        let shown_handles: Vec<Handle> = keys.windows(2).flat_map(|pair| handles(shown, pair[0], pair[1])).collect();
        // The points of the ways a chart times, on the curve on their frames.
        let chart_points: Vec<(u32, usize, Pos2)> = keys
            .windows(2)
            .filter_map(|pair| Some((pair[0].0, shown.chart(pair[0].0)?)))
            .flat_map(|(from, spacing)| (0..spacing.len()).map(move |line| (from, line)))
            .filter_map(|(from, line)| {
                let at = from + 1 + line as u32;
                Some((from, line, pos2(x(f64::from(at)), y(*samples.get(at as usize)?))))
            })
            .collect();
        #[cfg(test)]
        {
            self.keys = keys.iter().map(|(at, value)| (*at, pos2(x(f64::from(*at)), y(*value)))).collect();
            self.handles = shown_handles.iter().map(|handle| (handle.frame, handle.out, pos2(x(handle.at.0), y(handle.at.1)))).collect();
            self.chart_points.clone_from(&chart_points);
        }

        // What is under the pointer is taken by a drag: a handle before the keyframe it is on.
        let grips = shown_handles.iter().map(|handle| (handle.frame, Grab::Slope { out: handle.out }, pos2(x(handle.at.0), y(handle.at.1))));
        let points = chart_points.iter().map(|(from, line, at)| (*from, Grab::Chart { line: *line }, *at));
        let values = keys.iter().filter(|_| shown.valued()).map(|(at, value)| (*at, Grab::Value, pos2(x(f64::from(*at)), y(*value))));
        let grips: Vec<(u32, Grab, Pos2)> = grips.chain(points).chain(values).collect();
        let near = |pos: Pos2| grips.iter().find(|(.., at)| (*at - pos).length() <= REACH).map(|(frame, grab, _)| (*frame, *grab));
        if response.hover_pos().and_then(near).is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        // A drag is known as one only once the pointer has moved: what it took is what was under
        // the press.
        if response.drag_started()
            && let Some((frame, grab)) = ui.input(|i| i.pointer.press_origin()).and_then(near)
        {
            self.drag = Some(Drag { frame, grab, edited: subject.clone(), range });
        }
        let pointer = response.interact_pointer_pos().filter(|_| response.dragged() || response.clicked());
        match (&mut self.drag, pointer) {
            (Some(drag), Some(pos)) => {
                drag.edited = subject.clone();
                let (at, value) = (frame_at(pos), value_at(pos));
                match drag.grab {
                    Grab::Value => drag.edited.set_value(drag.frame, value),
                    Grab::Slope { out } => {
                        // The handle's own way: from its keyframe to the next, or from the last.
                        let way = keys.windows(2).find(|pair| if out { pair[0].0 == drag.frame } else { pair[1].0 == drag.frame });
                        if let Some(&[(from, v0), (to, v1)]) = way.map(|pair| [pair[0], pair[1]]).as_ref() {
                            let (frames, rise) = (f64::from(to - from), v1 - v0);
                            let reach = (if out { at - f64::from(from) } else { f64::from(to) - at } / frames).clamp(0.01, 1.0);
                            let climbed = if out { value - v0 } else { v1 - value };
                            // On a level way there is no speed to give.
                            let speed = if rise.abs() > 1e-9 { (climbed / (reach * rise)).clamp(-20.0, 20.0) } else { 0.0 };
                            let side = Some(Interp::Curve(Slope { reach, speed }));
                            let (into, out) = if out { (None, side) } else { (side, None) };
                            drag.edited.set_interp(drag.frame, into, out);
                        }
                    }
                    Grab::Chart { line } => {
                        // How far along the way from its keyframe to the next; the others
                        // follow as on the timeline.
                        let way = keys.windows(2).find(|pair| pair[0].0 == drag.frame);
                        if let (Some(&[(_, v0), (_, v1)]), Some(spacing)) = (way.map(|pair| [pair[0], pair[1]]).as_ref(), subject.chart(drag.frame))
                            && (v1 - v0).abs() > 1e-9
                        {
                            let rule = self.breakdowns.rule_now(ui);
                            let spacing = iskra_model::squash(&spacing, line, (value - v0) / (v1 - v0), rule, self.breakdowns.other_side);
                            drag.edited.set_chart(drag.frame, &spacing);
                        }
                    }
                }
            }
            // Anywhere else, the pointer puts the playhead on its frame.
            (None, Some(pos)) => actions.push(Action::Select { node: None, frame: (frame_at(pos).round().max(0.0) as u32).min(frames - 1) }),
            _ => {}
        }
        if response.drag_stopped()
            && let Some(drag) = self.drag.take()
        {
            let command = match drag.grab {
                Grab::Value => (*subject != drag.edited).then(|| drag.edited.value_command(drag.frame)),
                Grab::Slope { out } => (subject.interp(drag.frame) != drag.edited.interp(drag.frame)).then(|| drag.edited.ease_command(drag.frame, out)),
                Grab::Chart { .. } => {
                    let next = keys.iter().map(|(at, _)| *at).find(|at| *at > drag.frame);
                    next.filter(|_| subject.chart(drag.frame) != drag.edited.chart(drag.frame)).map(|to| drag.edited.chart_command(drag.frame, to))
                }
            };
            actions.extend(command.map(|(id, params)| Action::Run(id, params)));
        }

        // Handles under the keyframes, so a keyframe is never hidden by its own handle.
        for handle in &shown_handles {
            let key = keys.iter().find(|(at, _)| *at == handle.frame).map_or(Pos2::ZERO, |(at, value)| pos2(x(f64::from(*at)), y(*value)));
            let end = pos2(x(handle.at.0), y(handle.at.1));
            painter.line_segment([key, end], Stroke::new(1.0, theme.text_muted));
            painter.circle(end, 3.5, theme.surface, Stroke::new(1.0, theme.text));
        }
        let red = theme.colour(theme::Role::Breakdowns);
        for (.., at) in &chart_points {
            painter.circle(*at, 3.0, red, Stroke::new(1.0, theme.surface));
        }
        for (at, value) in &keys {
            let fill = if *at == frame { theme.accent } else { theme.text };
            painter.rect_filled(Rect::from_center_size(pos2(x(f64::from(*at)), y(*value)), vec2(7.0, 7.0)), 0.0, fill);
        }
    }
}

/// Give the keyframe of `curve` on `frame` a new value.
fn set_value(peg: &mut Peg, curve: Curve, frame: u32, value: f64) {
    let scale = peg.transform_at(frame).scale;
    let animation = &mut peg.animation;
    match curve {
        Curve::Rotation => animation.rotation.set(frame, value),
        Curve::ScaleX => animation.scale.set(frame, [value, scale[1]]),
        Curve::ScaleY => animation.scale.set(frame, [scale[0], value]),
        // A skew of 90° would lay the drawing flat on a line.
        Curve::Skew => animation.skew.set(frame, value.clamp(-85.0, 85.0)),
        Curve::Depth => animation.depth.set(frame, value),
        Curve::Position => {}
    }
}

#[cfg(test)]
mod tests {
    use iskra_model::DeformerId;

    use super::*;

    #[test]
    fn a_curves_points_are_values_of_its_pose() {
        let mut tail = Deformer::curve(DeformerId(1), "Tail", &[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]);
        tail.pose_knot(1, 0, [100.0, 0.0]);
        tail.key_all(0);
        tail.pose_knot(1, 10, [100.0, 40.0]);
        // Where the point lies on each frame, down, with its keys and theirs only.
        let down = Pose::PointY(1);
        assert_eq!((pose_value(&tail, down, 0), pose_value(&tail, down, 10), pose_value(&tail, Pose::PointX(1), 10)), (0.0, 40.0, 100.0));
        assert_eq!(pose_keys(&tail, down).iter().map(|(frame, ..)| *frame).collect::<Vec<_>>(), [0, 10]);
        assert!(pose_keys(&tail, Pose::PointY(2)).iter().all(|(frame, ..)| *frame == 0));
        // A key's value set here moves the point there, and the project is asked the same.
        let mut subject = Subject::Bones(Box::new(tail), down);
        subject.set_value(10, 25.0);
        let Subject::Bones(changed, _) = &subject else { unreachable!() };
        assert_eq!(pose_value(changed, down, 10), 25.0);
        let (command, params) = subject.value_command(10);
        assert_eq!((command, &params["knot"], &params["to"], &params["frame"]), ("deformer.setPose", &json!(1), &json!([100.0, 25.0]), &json!(10)));
    }
}
