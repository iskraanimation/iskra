//! The timeline: one row per peg and layer, one cell per frame. What is parented to a peg is
//! listed under it, indented. A layer's row shows its exposures, each hold as one block named
//! after its drawing; a peg's row shows its keyframes, which drag to another frame. A layer
//! that bones bend has a row under it for them, with the keyframes of their pose.
//!
//! A drag along a layer's row picks its frames, from where it was pressed to where it is let
//! go; picked, they are dragged to other frames, and the Edit menu cuts, copies, pastes and
//! deletes them. The ruler is where the playhead is dragged.
//!
//! Every block has a handle on its end: dragging it holds the drawing for longer or shorter;
//! with the bar's Ripple on the later drawings move along, off they stay where they are, and
//! Alt does the other (plan §7.2b). A click on a cell picks that frame, Shift-click from the
//! frame picked to it. Ctrl and the wheel, or the bar's slider, zoom the frames.
//!
//! Frames are numbered from 1 on screen and from 0 in the model.

use std::collections::{BTreeSet, HashMap};

use egui::{Align, Align2, Color32, CursorIcon, FontId, Layout, Rect, Sense, Stroke, pos2, vec2};
use iskra_model::{Channel, DeformerId, EffectId, GroupId, Interp, LayerId, Node, Part, PegId, PoseValue, Shot, ShotId, Source};
use serde_json::{Value, json};

use crate::icons;
use crate::prefs::NewDrawingHold;
use crate::theme::{self, Role, Theme};

/// How wide a frame's cell is, as the timeline starts, and how narrow and wide it is zoomed.
pub(crate) const CELL_WIDTH: f32 = 16.0;
pub(crate) const CELL_RANGE: std::ops::RangeInclusive<f32> = 3.0..=64.0;
const ROW_HEIGHT: f32 = 22.0;
const RULER_HEIGHT: f32 = 18.0;
/// How wide the column of names is to begin with, and how narrow and wide it can be dragged.
const NAMES_WIDTH: f32 = 150.0;
const NAMES_RANGE: std::ops::RangeInclusive<f32> = 90.0..=600.0;
/// How much of a block's end is its hold handle.
const HANDLE_WIDTH: f32 = 6.0;
/// How far a row's name moves in for each peg above it.
const INDENT: f32 = 14.0;

/// What the user asked for in the timeline. The app carries it out.
pub enum Action {
    /// Make this the current frame, and the node the selected one, when one is given: an
    /// layer to draw on, or a peg to transform.
    Select {
        node: Option<Node>,
        frame: u32,
    },
    Run(&'static str, Value),
    /// Save a node to the library as a template. `at` is where in the window it was dragged
    /// to and let go, which is a save if the Library panel is there; without it, it was asked
    /// for by name.
    Save {
        parts: Vec<Part>,
        at: Option<egui::Pos2>,
    },
    /// Pick these nodes together, by their ids, and no others.
    Pick(Vec<u64>),
    /// Run one of the app's own commands, by its id: what a menu here shares with the menu bar.
    Command(&'static str),
    /// What a part of the bar shared with the other bars asked for (see `bar::Asked`).
    Bar(crate::bar::Asked),
}

/// Frames of one layer that are picked together: from `first` to `last`, both among them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frames {
    pub layer: LayerId,
    pub first: u32,
    pub last: u32,
}

impl Frames {
    pub fn length(&self) -> u32 {
        self.last - self.first + 1
    }
}

#[derive(Default)]
pub struct Timeline {
    /// Which parts of the bar are shown, and in what order: the app's preferences keep it,
    /// and hand it over before the timeline is shown.
    pub bar: crate::bar::BarLayout,
    /// How wide the column of names was dragged to, where that is not what it starts with:
    /// kept by the app's preferences, as the bar is.
    pub names_wide: Option<f32>,
    /// The layer or peg in hand was let go of: no row is shown selected.
    pub let_go: bool,
    /// Where the edge of the column of names is, by which it is dragged wider and narrower.
    #[cfg(test)]
    pub names_edge: Option<Rect>,
    /// Loop, Snap and Ripple, the app's: handed over before the timeline is shown, and taken
    /// back as it is left (see `bar::Switches`).
    pub bar_switches: crate::bar::Switches,
    /// The Animate switch, the preferences': handed over before the timeline is shown, and
    /// taken back as it is left.
    pub animate: bool,
    /// The frame the bar says it is on, where that is not the playhead's: while the shot plays
    /// or the playhead is dragged, the one it was on before.
    pub shown: Option<u32>,
    /// Where the Animate switch was drawn, for the tests.
    #[cfg(test)]
    pub animate_switch: Option<Rect>,
    /// The Adjust switch beside it: handed over and taken back as `animate` is.
    pub adjust: bool,
    /// How the timing charts' lines are shown and dragged: handed over and taken back as
    /// `animate` is, the same on the storyboard timeline.
    pub breakdowns: crate::breakdown::Breakdowns,
    /// A line of a timing chart being dragged.
    chart_drag: Option<crate::breakdown::ChartDrag>,
    /// Where each timing chart's line was drawn: whose row, the way's first frame, the line.
    #[cfg(test)]
    pub breakdown_lines: Vec<(Keyed, u32, Rect)>,
    /// How wide a frame's cell is: the preferences', handed over and taken back. Nothing is
    /// [`CELL_WIDTH`].
    pub zoom: f32,
    /// The bar's Fit was clicked: the frames are made as wide as fits the whole shot in.
    fitting: bool,
    /// Where the bar's Fit was drawn.
    #[cfg(test)]
    pub fit_button: Option<Rect>,
    /// Where the Adjust switch was drawn, for the tests.
    #[cfg(test)]
    pub adjust_switch: Option<Rect>,
    /// The boards have no rows. The app says so, as the preferences keep it.
    pub hide_boards: bool,
    /// A click on a row or a keyframe selects it and leaves the playhead where it is
    /// (Settings ▸ Interface). A click or a drag on the ruler, and a drag along the rows,
    /// still move it.
    pub hold_playhead: bool,
    /// The playhead's frame as the timeline is shown, where a click leaves it while it is held.
    now: u32,
    /// Where the locks of the boards' rows were drawn.
    #[cfg(test)]
    pub locks: Vec<(LayerId, egui::Rect)>,
    /// The icon each row that is not a board's was drawn with.
    #[cfg(test)]
    pub row_icons: Vec<(Node, &'static str)>,
    /// The row whose name is being edited, with the text so far.
    renaming: Option<(Row, String)>,
    /// How far down the rows are scrolled, and how high the part of the panel is that shows
    /// them, under the ruler: both as they were when the rows were last shown.
    scroll: f32,
    room: f32,
    /// The row that was selected when the rows were last shown, by its node's id: when it is
    /// another, the rows scroll to show it.
    pub(crate) shown_selected: Option<u64>,
    /// The part of the panel the rows are seen in, under the ruler: the names and the cells.
    #[cfg(test)]
    pub rows_seen: Option<Rect>,
    /// The row that was clicked last, by its node's id: where a click with Shift picks from.
    anchor: Option<u64>,
    hold_drag: Option<HoldDrag>,
    /// Where the Animate part's key button was drawn.
    #[cfg(test)]
    pub key_button: Option<Rect>,
    #[cfg(test)]
    pub key_all_button: Option<Rect>,
    key_drag: Option<KeyDrag>,
    /// The keyframes a drag has just let go of, by where they were, and how far they go: they
    /// are shown where they went until the project has them there, on the next frame, so that
    /// they do not show where they were for one frame between.
    key_landing: Option<(Vec<(Keyed, u32)>, i64)>,
    /// Whether what the keyframe commands work on has a keyframe on the frame showing, as the
    /// app says: `None` with nothing to key. The Animate part's key button shows it.
    pub keyed_here: Option<bool>,
    /// Whether Key All has something to key, as the app says.
    pub can_key_all: bool,
    /// What *Keyframe Cycle* shows of the keyframes picked, as the app says.
    pub cycled: crate::bar::Cycled,
    /// Whether Extend has a hold to take on to the playhead, as the app says.
    pub extendable: bool,
    /// The frames of a layer that are picked, by a drag along its row.
    pub frames: Option<Frames>,
    /// The picked frames being dragged to other frames: by how many, so far.
    frames_drag: Option<i64>,
    /// Where the picked frames were marked.
    #[cfg(test)]
    pub frames_mark: Option<Rect>,
    /// A marker being dragged along the ruler: the frame it is on, and the frame it would
    /// land on.
    marker_drag: Option<(u32, u32)>,
    /// How many frames the cells show: the shot's, or more where something lies past its end.
    reach: u32,
    /// Where what is past the end was darkened.
    #[cfg(test)]
    pub past_end: Option<Rect>,
    /// The end of the shot being dragged on the ruler: the length it would have.
    end_drag: Option<u32>,
    /// Where that end's handle was drawn.
    #[cfg(test)]
    pub end_grip: Option<Rect>,
    /// The marker whose name is being typed in its menu, with the text so far.
    marker_name: Option<(u32, String)>,
    /// Where the markers were drawn, by their frames; where the ends of the stretch that
    /// plays were, if one is set; and where the arrows that fold pegs' rows were.
    #[cfg(test)]
    pub markers: Vec<(u32, Rect)>,
    /// Where the retake notes' marks were drawn, by their ids.
    #[cfg(test)]
    pub note_marks: Vec<(u64, Rect)>,
    #[cfg(test)]
    pub range_ends: Option<(f32, f32)>,
    #[cfg(test)]
    pub peg_arrows: Vec<(PegId, Rect)>,
    /// The keyframes that are picked together: whose they are, and their frames. One of
    /// them that is dragged takes the others along.
    pub(crate) picked_keys: Vec<(Keyed, u32)>,
    /// The nodes opened by their `+`, by their ids: each of their values has a row of its own
    /// under theirs. The window's, not the project's.
    pub(crate) opened: BTreeSet<u64>,
    /// The box that is being dragged out over the rows of keyframes to pick them: where it
    /// was pressed, and where the pointer is.
    key_marquee: Option<(egui::Pos2, egui::Pos2)>,
    /// Where each keyframe was drawn, this time round.
    shown_keys: Vec<(Keyed, u32, Rect)>,
    /// Where each block's hold handle was drawn: layer, the hold's first frame, the handle.
    #[cfg(test)]
    pub hold_handles: Vec<(LayerId, u32, Rect)>,
    /// Where each row's name was drawn.
    #[cfg(test)]
    pub names: Vec<(Node, Rect)>,
    /// The shot's sound (plan §7.2): a row for each of the storyboard's sound tracks that has
    /// sound under the shot. The app says what they are before the timeline is shown.
    pub sound: Vec<SoundRow>,
    /// The frames of the shot that are made and kept on the graphics card, in their order:
    /// they play without being made first. The app says which before the timeline is shown.
    pub cached: Vec<u32>,
    /// The stretches of such frames that were marked on the ruler: first frame, and the one
    /// after the last.
    #[cfg(test)]
    pub cache_marks: Vec<(u32, u32)>,
    /// The project's master volume. The app says what it is before the timeline is shown.
    pub volume: f32,
    /// A volume that is being dragged: whose, a clip's by its id or the master's by 0, and
    /// what it is so far.
    volume_edit: Option<(u64, f32)>,
    /// The shot length being dragged in the bar, until it is let go.
    length_edit: Option<u32>,
    /// Where that length's field was drawn.
    #[cfg(test)]
    pub length_field: Option<egui::Rect>,
    /// The sound tracks whose clips are locked where they are, which are the storyboard
    /// timeline's locks: the app hands them over and takes them back. And those whose row
    /// here shows no waveform.
    pub sound_locked: BTreeSet<usize>,
    pub sound_hidden: BTreeSet<usize>,
    /// Where the switch that hides each row was drawn, by the id of the row's node, and the
    /// one that locks it; and where the switches of each sound row were, by its track and what
    /// they switch.
    #[cfg(test)]
    pub eyes: Vec<(u64, Rect)>,
    #[cfg(test)]
    pub row_locks: Vec<(u64, Rect)>,
    #[cfg(test)]
    pub sound_switches: Vec<(usize, &'static str, Rect)>,
    /// Where each sound row was drawn.
    #[cfg(test)]
    pub sound_rows: Vec<Rect>,
    /// The frame of the film the shot's first frame is, where the shot is in its storyboard's
    /// cut: the app says before the timeline is shown. And where sound from the media pool
    /// would be laid, as the timeline was last drawn.
    pub film_start: Option<(iskra_model::StoryboardId, i64)>,
    /// How many sound tracks that storyboard has. The app says before the timeline is shown.
    pub sound_tracks: usize,
    pub(crate) sound_drop: Option<crate::sound::SoundDrop>,
    /// The stretches of frames on which a layer is shown too large for its pixels (plan §4.10),
    /// which its row marks.
    coarse: HashMap<LayerId, Vec<(u32, u32)>>,
    /// Where each such stretch was marked: layer, first frame, the mark.
    #[cfg(test)]
    pub coarse_marks: Vec<(LayerId, u32, Rect)>,
    /// Where each keyframe was drawn: peg, frame, the mark.
    #[cfg(test)]
    pub key_marks: Vec<(PegId, u32, Rect)>,
    /// Where each row of bones was named, and where each keyframe of a pose was drawn.
    #[cfg(test)]
    pub bone_names: Vec<(DeformerId, Rect)>,
    #[cfg(test)]
    pub bone_key_marks: Vec<(DeformerId, u32, Rect)>,
    /// The lines between keyframes that were drawn: whose, by the id, and from which frame
    /// to which.
    #[cfg(test)]
    pub tweens: Vec<(u64, u32, u32)>,
    /// Where each effect's row was named, and where each of its keyframes was drawn.
    #[cfg(test)]
    pub effect_names: Vec<(EffectId, Rect)>,
    #[cfg(test)]
    pub effect_key_marks: Vec<(EffectId, u32, Rect)>,
    /// Where each key of a value's own row was drawn.
    #[cfg(test)]
    pub value_key_marks: Vec<(Keyed, u32, Rect)>,
    /// Each keyframe, by its frame in the project, with the frame it was drawn on.
    #[cfg(test)]
    pub keys_drawn: Vec<(Keyed, u32, u32)>,
    /// Each keyframe a cycle repeats that was drawn, by its row and frame.
    #[cfg(test)]
    pub repeated_marks: Vec<(Keyed, u32)>,
    /// Where each node's `+` was drawn, by the node's id, and each value's row was named.
    #[cfg(test)]
    pub openers: Vec<(u64, Rect)>,
    #[cfg(test)]
    pub value_names: Vec<(Keyed, Rect)>,
    /// The marks on folded rows for what they put away: the row, by its id, and the frame.
    #[cfg(test)]
    pub put_away_marks: Vec<(u64, u32)>,
    /// Where the arrow of each group's row was drawn.
    #[cfg(test)]
    pub group_names: Vec<(GroupId, Rect)>,
}

/// A row of the timeline: a node of the shot's tree, or the bones that bend a layer, which are
/// listed under the first layer they bend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Node(Node),
    Bones(DeformerId, LayerId),
    /// A group (plan §4.3), over the first row of what is in it; folded, in place of all of it.
    Group(GroupId),
    /// An effect node that has keyframes (plan §8.3), under the layer whose wire it is on or
    /// whose matte it cuts by (see [`effect_layer`]), or after the other rows if neither.
    Effect(EffectId, Option<LayerId>),
    /// One value of a peg, of a chain of bones or of an effect node, with its own keys: under
    /// the node's row, while the node is opened by its `+`.
    Value(Keyed),
}

impl Row {
    /// What a press on the row selects: the node, or the layer the bones are listed under.
    /// A group's row selects nothing.
    fn node(self) -> Option<Node> {
        match self {
            Row::Node(node) => Some(node),
            Row::Bones(_, layer) => Some(Node::Layer(layer)),
            Row::Group(_) => None,
            Row::Effect(_, layer) => layer.map(Node::Layer),
            Row::Value(Keyed::Peg(peg, _)) => Some(Node::Peg(peg)),
            Row::Value(Keyed::Bones(_, layer, _)) => Some(Node::Layer(layer)),
            Row::Value(Keyed::Effect(..) | Keyed::Lens(_)) => None,
        }
    }

    /// The id of the shot's node the row is of.
    fn key(self) -> u64 {
        match self {
            Row::Node(Node::Layer(id)) => id.0,
            Row::Node(Node::Peg(id)) => id.0,
            Row::Bones(id, _) => id.0,
            Row::Group(id) => id.0,
            Row::Effect(id, _) => id.0,
            Row::Value(of) => of.id(),
        }
    }
}

/// Whether an effect can be keyed: it has a number among its parameters, or keys already.
fn keyable(effect: &iskra_model::Effect) -> bool {
    let numbers = effect.described().is_some_and(|kind| kind.params.iter().any(|param| matches!(param.kind, iskra_model::ParamKind::Number { .. })));
    numbers || !effect.key_frames(None).is_empty()
}

/// The layer a wire starts at, back through the effects on it: `None` where it starts at a
/// composite, say the picture a group's port gives out.
fn wire_layer(shot: &Shot, source: Source) -> Option<LayerId> {
    let mut at = source;
    // A wire has no more effects on it than the shot has; the limit keeps bad data from looping.
    for _ in 0..=shot.effects.len() {
        match at {
            Source::Layer(layer) | Source::Art(layer, _) => return Some(layer),
            Source::Effect(effect) => at = shot.effect(effect)?.picture()?,
            Source::Composite(_) => return None,
        }
    }
    None
}

/// The layer an effect's row is listed under: the one its wire starts at, or where that is
/// no layer, the one that the matte of it or of an effect before it on the wire comes from,
/// as a cutter that cuts a group's picture by a body. `None` for an effect on neither.
fn effect_layer(shot: &Shot, effect: EffectId) -> Option<LayerId> {
    wire_layer(shot, Source::Effect(effect)).or_else(|| {
        let mut at = Some(effect);
        for _ in 0..=shot.effects.len() {
            let node = shot.effect(at?)?;
            if let Some(layer) = node.inputs.get(iskra_model::MATTE).and_then(|matte| wire_layer(shot, *matte)) {
                return Some(layer);
            }
            at = match node.picture()? {
                Source::Effect(before) => Some(before),
                _ => None,
            };
        }
        None
    })
}

/// The effects listed under a layer: those on its wires, its flat output's and then its art
/// layers', the first each goes through first, then those that [`effect_layer`] puts there
/// from no layer's wire.
pub(crate) fn effects_of(shot: &Shot, layer: LayerId) -> Vec<&iskra_model::Effect> {
    let mut of: Vec<&iskra_model::Effect> = Source::outputs(layer).flat_map(|output| shot.effects_on(output)).collect();
    let matted = shot.effects.iter().map(|effect| &**effect);
    of.extend(matted.filter(|effect| wire_layer(shot, Source::Effect(effect.id)).is_none() && effect_layer(shot, effect.id) == Some(layer)));
    of
}

/// The shot's rows: its tree, with each chain of bones under the first layer it bends, and a
/// row for each group over the first row of what is in it. What is in a folded group has no
/// rows: the group's row stands for it.
fn rows(shot: &Shot, hide_boards: bool, opened: &BTreeSet<u64>) -> Vec<(Row, usize)> {
    // The boards are together at the top (plan §7.2c): each row that hangs under nothing, with
    // what hangs under it, goes there if it is a board or a board's peg. A board under another
    // peg stays with that peg.
    let mut nodes = shot.rows();
    let is_board = |node: Node| match node {
        Node::Layer(layer) => shot.is_board(layer),
        Node::Peg(peg) => shot.is_board_peg(peg),
    };
    nodes.retain(|(node, _)| !hide_boards || !is_board(*node));
    // What a folded peg carries has no rows: the peg's row stands for it.
    let mut folded_at: Option<usize> = None;
    nodes.retain(|(node, depth)| {
        if folded_at.is_some_and(|above| *depth > above) {
            return false;
        }
        folded_at = matches!(node, Node::Peg(peg) if shot.folded.contains(peg)).then_some(*depth);
        true
    });
    let mut board = false;
    let (mut boards, mut rest) = (Vec::new(), Vec::new());
    for (node, depth) in nodes {
        if depth == 0 {
            board = is_board(node);
        }
        if board { boards.push((node, depth)) } else { rest.push((node, depth)) }
    }
    let mut listed = Vec::new();
    // The camera's lens, once it has keys, has a row under the peg that moves the camera, or
    // at the end where that peg has no row.
    let mut lens = (!shot.camera.animation.is_empty()).then_some(Row::Value(Keyed::Lens(shot.id)));
    // A node opened by its `+` has a row for each of its values under it.
    let values = |listed: &mut Vec<(Row, usize)>, of: Keyed, depth: usize| {
        if opened.contains(&of.id()) {
            listed.extend(values_of(shot, of).into_iter().map(|value| (Row::Value(value), depth + 1)));
        }
    };
    for (node, depth) in boards.into_iter().chain(rest) {
        listed.push((Row::Node(node), depth));
        let layer = match node {
            Node::Peg(peg) => {
                values(&mut listed, Keyed::Peg(peg, None), depth);
                if lens.is_some() && shot.camera.parent == Some(peg) {
                    listed.extend(lens.take().map(|lens| (lens, depth + 1)));
                }
                continue;
            }
            Node::Layer(layer) => layer,
        };
        let bones = shot.layer(layer).and_then(|layer| layer.deformer).filter(|id| shot.deformer(*id).is_some());
        if let Some(bones) = bones.filter(|bones| !listed.iter().any(|(row, _)| matches!(row, Row::Bones(listed, _) if listed == bones))) {
            listed.push((Row::Bones(bones, layer), depth + 1));
            values(&mut listed, Keyed::Bones(bones, layer, None), depth + 1);
        }
        // The effects of the layer that can be keyed, whether they have keys yet or not.
        let keyed = effects_of(shot, layer).into_iter().filter(|effect| keyable(effect));
        for effect in keyed.map(|effect| effect.id).collect::<Vec<_>>() {
            listed.push((Row::Effect(effect, Some(layer)), depth + 1));
            values(&mut listed, Keyed::Effect(effect, None), depth + 1);
        }
    }
    // And those of no layer at the end. Those of a layer that a folded peg puts away, or that
    // is a hidden board, are put away with it.
    let apart = shot.effects.iter().filter(|effect| keyable(effect) && effect_layer(shot, effect.id).and_then(|layer| shot.layer(layer)).is_none());
    let apart: Vec<_> = apart.map(|effect| effect.id).collect();
    for effect in apart {
        if !listed.iter().any(|(row, _)| matches!(row, Row::Effect(listed, _) if *listed == effect)) {
            listed.push((Row::Effect(effect, None), 0));
            values(&mut listed, Keyed::Effect(effect, None), 0);
        }
    }
    listed.extend(lens.map(|lens| (lens, 0)));
    let mut rows: Vec<(Row, usize)> = Vec::new();
    for (row, depth) in listed {
        // The groups the row is in, the outermost first, as far in as the first folded one:
        // each has its row before the first row of what is in it.
        let mut above: Vec<_> = shot.groups_above(row.key()).collect();
        above.reverse();
        let mut put_away = false;
        for group in above {
            if !rows.iter().any(|(row, _)| *row == Row::Group(group.id)) {
                rows.push((Row::Group(group.id), depth));
            }
            if group.rows_folded {
                put_away = true;
                break;
            }
        }
        if !put_away {
            rows.push((row, depth));
        }
    }
    rows
}

/// The frames on which a row that a folded row puts away has a keyframe: a peg's, those of a
/// layer's bones, and those of a layer's effects ([`effects_of`]). Not where a layer's drawings
/// start: those are no keyframes, and a layer drawn on every frame would mark them all.
/// `inside` says which nodes are put away.
fn put_away(shot: &Shot, inside: impl Fn(Node) -> bool) -> BTreeSet<u32> {
    let mut frames = BTreeSet::new();
    for peg in shot.pegs.iter().filter(|peg| inside(Node::Peg(peg.id))) {
        frames.extend(peg.animation.key_frames());
    }
    for layer in shot.layers.iter().filter(|layer| inside(Node::Layer(layer.id))) {
        frames.extend(layer.deformer.and_then(|bones| shot.deformer(bones)).into_iter().flat_map(|bones| bones.key_frames()));
        frames.extend(effects_of(shot, layer.id).into_iter().flat_map(|effect| effect.key_frames(None)));
    }
    frames
}

/// What is asked of the pegs over the layers and pegs that are picked together.
#[derive(Clone, Copy)]
pub(crate) enum Parenting {
    /// A new peg above each, between it and what it is parented to.
    Above,
    /// Each taken off its peg.
    Off,
    /// Each put under this peg, or port, but the peg itself and those that carry it.
    Under(u64),
}

/// The parenting of all the layers and pegs among `picked`, as `edit.batch` takes it, in one
/// step: `None` where it would change fewer than two of them.
pub(crate) fn parenting(shot: &Shot, picked: &BTreeSet<u64>, ask: Parenting) -> Option<Value> {
    let one = |node: &u64| {
        let (kind, parent) = match (shot.layer(LayerId(*node)), shot.peg(PegId(*node))) {
            (Some(layer), _) => ("layer", layer.parent),
            (None, Some(peg)) => ("peg", peg.parent),
            (None, None) => return None,
        };
        let set_parent = format!("{kind}.setParent");
        match ask {
            Parenting::Above if kind == "layer" => Some(json!({ "id": "peg.new", "params": { "aboveLayer": node } })),
            Parenting::Above => Some(json!({ "id": "peg.new", "params": { "abovePeg": node } })),
            Parenting::Off => parent.map(|_| json!({ "id": set_parent, "params": { kind: node } })),
            // A peg goes under no peg that it carries, nor under itself.
            Parenting::Under(under) => {
                let fits = parent != Some(PegId(under)) && (kind == "layer" || !shot.carried_from(Some(PegId(under)), *node));
                fits.then(|| json!({ "id": set_parent, "params": { kind: node, "parent": under } }))
            }
        }
    };
    let commands: Vec<Value> = picked.iter().filter_map(one).collect();
    let title = match ask {
        Parenting::Above => "New Pegs",
        Parenting::Off => "Take Off Their Pegs",
        Parenting::Under(_) => "Parent to Peg",
    };
    (commands.len() > 1).then(|| json!({ "commands": commands, "title": title }))
}

/// The nodes that are picked together as what the library saves: the layers, pegs,
/// composites and groups among them.
pub(crate) fn parts_of(shot: &Shot, picked: &BTreeSet<u64>) -> Vec<Part> {
    let kinds = |id: &u64| {
        [
            Part::Layer(LayerId(*id)),
            Part::Peg(PegId(*id)),
            Part::Composite(iskra_model::CompositeId(*id)),
            Part::Group(GroupId(*id)),
            Part::Effect(iskra_model::EffectId(*id)),
            Part::Display(iskra_model::DisplayId(*id)),
        ]
    };
    picked.iter().flat_map(kinds).filter(|part| shot.has(*part)).collect()
}

/// How a pose's keyframe eases, as its menu offers it: into the key, where that is set too,
/// and out of it.
pub(crate) const EASES: [(&str, Option<Interp>, Interp); 5] = [
    ("Linear", Some(Interp::Linear), Interp::Linear),
    ("Slow In", Some(Interp::Ease), Interp::Linear),
    ("Slow Out", Some(Interp::Linear), Interp::Ease),
    ("Slow In and Out", Some(Interp::Ease), Interp::Ease),
    ("Hold", None, Interp::Hold),
];

/// One sound track as a shot hears it: the track's name, and for each frame of the shot what a
/// waveform draws there, the lowest and the highest value, or `None` where the track is silent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundRow {
    pub name: String,
    /// The track's clips that lie under the shot: the id, the name of its sound, how loud.
    pub clips: Vec<(u64, String, f32)>,
    /// Which track of which storyboard it is, and whether the track is muted.
    pub storyboard: u64,
    pub track: usize,
    pub mute: bool,
    pub peaks: Vec<Option<[f32; 2]>>,
}

/// How high a sound row is.
const SOUND_HEIGHT: f32 = 28.0;

/// Whose keyframes a row shows: a peg's, or those of the pose of a chain of bones, which is
/// listed under a layer, or an effect node's; of all its values, or on a row of its own of
/// the one named: a peg's channel, a value of the pose, an effect's parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keyed {
    Peg(PegId, Option<Channel>),
    Bones(DeformerId, LayerId, Option<PoseValue>),
    Effect(EffectId, Option<&'static str>),
    /// The lens of a shot's camera: its zoom and its field of view, keyed together.
    Lens(ShotId),
}

impl Keyed {
    /// The kind its commands are of: `peg`, `deformer` or `effect`, as in `peg.moveKey`.
    pub(crate) fn kind(self) -> &'static str {
        match self {
            Keyed::Peg(..) => "peg",
            Keyed::Bones(..) => "deformer",
            Keyed::Effect(..) => "effect",
            Keyed::Lens(_) => "camera",
        }
    }

    /// What its key commands take, with `more`: the node, and the value if it is of one.
    pub(crate) fn params(self, more: Value) -> Value {
        let mut params = match self {
            Keyed::Peg(peg, channel) => json!({ "peg": peg.0, "channel": channel }),
            Keyed::Bones(bones, _, value) => json!({ "deformer": bones.0, "value": value }),
            Keyed::Effect(effect, param) => json!({ "effect": effect.0, "param": param }),
            // The lens alone: the peg that moves the camera has its own row.
            Keyed::Lens(shot) => json!({ "shot": shot.0, "lensOnly": true }),
        };
        if let (Some(params), Value::Object(more)) = (params.as_object_mut(), more) {
            params.extend(more);
            params.retain(|_, value| !value.is_null());
        }
        params
    }

    /// The value it is of as copy and paste name it (`keys.copy`), if it is of one.
    pub(crate) fn clip_name(self) -> Option<String> {
        match self {
            Keyed::Peg(_, channel) => channel.map(|channel| channel_title(channel).to_lowercase()),
            Keyed::Bones(_, _, value) => value.map(|value| match value {
                PoseValue::Offset => "offset".to_string(),
                PoseValue::Turn(bone) => format!("bone{bone}.turn"),
                PoseValue::Stretch(bone) => format!("bone{bone}.stretch"),
                PoseValue::Moved(point) => format!("point{point}.move"),
                PoseValue::BentInto(point) => format!("point{point}.into"),
                PoseValue::BentOut(point) => format!("point{point}.out"),
                PoseValue::Ik => "ik".to_string(),
                PoseValue::Goal => "ik.goal".to_string(),
            }),
            Keyed::Effect(_, param) => param.map(str::to_string),
            Keyed::Lens(_) => None,
        }
    }

    /// The node's id.
    pub(crate) fn id(self) -> u64 {
        match self {
            Keyed::Peg(peg, _) => peg.0,
            Keyed::Bones(bones, ..) => bones.0,
            Keyed::Effect(effect, _) => effect.0,
            Keyed::Lens(shot) => shot.0,
        }
    }

    /// The frames it has keys on, how they ease, and whether a value moves on after one.
    fn frames(self, shot: &Shot) -> Vec<u32> {
        match self {
            Keyed::Peg(peg, channel) => shot.peg(peg).map(|peg| peg.animation.frames(channel).into_iter().collect()),
            Keyed::Bones(bones, _, value) => shot.deformer(bones).map(|bones| bones.key_frames_of(value).into_iter().collect()),
            Keyed::Effect(effect, param) => shot.effect(effect).map(|effect| effect.key_frames(param).into_iter().collect()),
            Keyed::Lens(_) => Some(shot.camera.animation.frames().into_iter().collect()),
        }
        .unwrap_or_default()
    }

    pub(crate) fn eases(self, shot: &Shot, frame: u32) -> Option<(Interp, Interp)> {
        match self {
            Keyed::Peg(peg, channel) => shot.peg(peg)?.animation.interp_of(frame, channel),
            Keyed::Bones(bones, _, value) => shot.deformer(bones)?.interp_of(frame, value),
            Keyed::Effect(effect, param) => shot.effect(effect)?.interp(frame, param),
            Keyed::Lens(_) => {
                let lens = &shot.camera.animation;
                lens.zoom.key(frame).or(lens.distance.key(frame)).map(|key| (key.into, key.out))
            }
        }
    }

    fn moves_after(self, shot: &Shot, frame: u32) -> bool {
        match self {
            Keyed::Peg(peg, channel) => shot.peg(peg).is_some_and(|peg| peg.animation.moves_after_of(frame, channel)),
            Keyed::Bones(bones, _, value) => shot.deformer(bones).is_some_and(|bones| bones.moves_after_of(frame, value)),
            Keyed::Effect(effect, param) => shot.effect(effect).is_some_and(|effect| effect.moves_after_of(frame, param)),
            Keyed::Lens(_) => shot.camera.animation.zoom.moves_after(frame) || shot.camera.animation.distance.moves_after(frame),
        }
    }

    /// Whose keys it is as `keys.chart` names it, and of which values: all of them, or the
    /// one or two its row is of.
    fn chart_owner(self) -> (iskra_model::KeyOwner, Option<Vec<String>>) {
        let owner = match self {
            Keyed::Peg(peg, _) => iskra_model::KeyOwner::Peg(peg),
            Keyed::Bones(bones, ..) => iskra_model::KeyOwner::Deformer(bones),
            Keyed::Effect(effect, _) => iskra_model::KeyOwner::Effect(effect),
            Keyed::Lens(shot) => iskra_model::KeyOwner::Camera(shot),
        };
        let values = match self {
            Keyed::Lens(_) => Some(vec!["zoom".to_string(), "distance".to_string()]),
            other => other.clip_name().map(|name| vec![name]),
        };
        (owner, values)
    }

    /// How its way from its key on `from` to its key on `to` is spaced, of each value.
    fn spacings(self, shot: &Shot, from: u32, to: u32) -> Vec<Vec<f64>> {
        let (owner, values) = self.chart_owner();
        let names: Option<Vec<&str>> = values.as_ref().map(|values| values.iter().map(String::as_str).collect());
        shot.spacings_of(owner, names.as_deref(), from, to)
    }

    /// The commands, for `edit.batch`, that time its way from its key on `from` to the next
    /// by `spacing`; empty takes the chart away. None where it has no later key.
    pub(crate) fn charted(self, shot: &Shot, from: u32, spacing: &[f64]) -> Vec<Value> {
        let Some(to) = self.frames(shot).into_iter().find(|frame| *frame > from) else { return Vec::new() };
        let (owner, values) = self.chart_owner();
        let params =
            |value: Option<&String>| json!({ "id": "keys.chart", "params": { "owner": owner, "value": value, "from": from, "to": to, "spacing": spacing } });
        match values {
            Some(values) => values.iter().map(|value| params(Some(value))).collect(),
            None => vec![params(None)],
        }
    }

    /// The frames before `until` on which a cycle of its repeats one of its keys, where it has
    /// none of its own.
    fn repeated(self, shot: &Shot, until: u32) -> BTreeSet<u32> {
        let (owner, values) = self.chart_owner();
        let names: Option<Vec<&str>> = values.as_ref().map(|values| values.iter().map(String::as_str).collect());
        let mut repeated = shot.repeated_keys_of(owner, names.as_deref(), until);
        for frame in self.frames(shot) {
            repeated.remove(&frame);
        }
        repeated
    }

    /// Whether a cycle of its ends on its key on `frame`, and repeats after it.
    fn ends_cycle(self, shot: &Shot, frame: u32) -> bool {
        let (owner, values) = self.chart_owner();
        let names: Option<Vec<&str>> = values.as_ref().map(|values| values.iter().map(String::as_str).collect());
        shot.cycle_of(owner, names.as_deref(), frame + 1).is_some_and(|(_, to, _)| to == frame)
    }

    /// What its row is called, if it is of one value.
    fn value_label(self, shot: &Shot) -> Option<String> {
        match self {
            Keyed::Peg(_, Some(channel)) => Some(channel_title(channel).to_string()),
            Keyed::Bones(_, _, Some(value)) => Some(value.label()),
            Keyed::Effect(effect, Some(param)) => {
                let title = shot.effect(effect).and_then(|effect| effect.described()?.param(param)).map(|spec| spec.title);
                Some(title.unwrap_or(param).to_string())
            }
            Keyed::Lens(_) => Some("Lens".to_string()),
            _ => None,
        }
    }
}

/// What a peg's channel's row is called.
fn channel_title(channel: Channel) -> &'static str {
    match channel {
        Channel::Position => "Position",
        Channel::Rotation => "Rotation",
        Channel::Scale => "Scale",
        Channel::Skew => "Skew",
        Channel::Depth => "Depth",
        Channel::Tilt => "Tilt",
    }
}

/// The values of a node that have rows of their own when it is opened by its `+`.
fn values_of(shot: &Shot, of: Keyed) -> Vec<Keyed> {
    match of {
        Keyed::Peg(peg, _) => Channel::ALL.into_iter().map(|channel| Keyed::Peg(peg, Some(channel))).collect(),
        Keyed::Bones(bones, layer, _) => {
            shot.deformer(bones).map_or(Vec::new(), |chain| chain.pose_values().into_iter().map(|value| Keyed::Bones(bones, layer, Some(value))).collect())
        }
        Keyed::Effect(id, _) => {
            let Some((effect, kind)) = shot.effect(id).and_then(|effect| Some((effect, effect.described()?))) else { return Vec::new() };
            // The numbers, which are keyed, and any other that has keys.
            let keyed = |param: &iskra_model::ParamSpec| {
                matches!(param.kind, iskra_model::ParamKind::Number { .. }) || effect.params.get(param.id).is_some_and(|held| !held.keys.is_empty())
            };
            kind.params.iter().filter(|param| keyed(param)).map(|param| Keyed::Effect(id, Some(param.id))).collect()
        }
        Keyed::Lens(_) => Vec::new(),
    }
}

/// The keyframes on one frame being dragged to another.
#[derive(Clone, Copy)]
struct KeyDrag {
    of: Keyed,
    from: u32,
    /// The frame under the pointer now.
    to: u32,
}

/// The id a command takes for a node, under the name its commands call it by.
fn node_param(node: Node) -> (&'static str, u64) {
    match node {
        Node::Layer(id) => ("layer", id.0),
        Node::Peg(id) => ("peg", id.0),
    }
}

/// The end of a hold being dragged.
#[derive(Clone, Copy)]
struct HoldDrag {
    layer: LayerId,
    /// The hold's first frame.
    start: u32,
    /// How long the hold is where the pointer is now.
    len: u32,
}

impl Timeline {
    /// How wide a frame's cell is.
    pub fn frame_width(&self) -> f32 {
        if self.zoom > 0.0 { self.zoom.clamp(*CELL_RANGE.start(), *CELL_RANGE.end()) } else { CELL_WIDTH }
    }

    /// How wide the column of names is.
    pub(crate) fn names_width(&self) -> f32 {
        self.names_wide.unwrap_or(NAMES_WIDTH).clamp(*NAMES_RANGE.start(), *NAMES_RANGE.end())
    }

    /// `layer` is the layer that is drawn on, or `peg` the peg that is selected.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        shot: &Shot,
        layer: Option<LayerId>,
        peg: Option<PegId>,
        picked: &BTreeSet<u64>,
        frame: u32,
        playing: bool,
        hold: &mut NewDrawingHold,
        coarse: &HashMap<LayerId, Vec<(u32, u32)>>,
        elsewhere: &mut dyn FnMut(&mut egui::Ui, &'static str),
    ) -> Vec<Action> {
        self.coarse.clone_from(coarse);
        self.now = frame;
        // What was let go of last frame is where it went in the project now.
        self.key_landing = None;
        let mut actions = Vec::new();
        self.controls(ui, shot, layer, frame, playing, hold, &mut actions, elsewhere);
        ui.separator();
        let rows = rows(shot, self.hide_boards, &self.opened);
        let let_go = self.let_go;
        let selected = |node| match node {
            _ if let_go => false,
            Node::Peg(id) => peg == Some(id),
            Node::Layer(id) => layer == Some(id),
        };
        let height = RULER_HEIGHT + ROW_HEIGHT * rows.len() as f32 + SOUND_HEIGHT * self.sound.len() as f32;
        // A row that is newly selected, here or anywhere else, is scrolled to if it is out of
        // sight.
        let mut scroll_to = None;
        let at = rows.iter().position(|(row, _)| matches!(row, Row::Node(node) if selected(*node)));
        let now = at.map(|at| rows[at].0.key());
        if self.room > ROW_HEIGHT && now != self.shown_selected {
            self.shown_selected = now;
            let top = at.map(|at| ROW_HEIGHT * at as f32);
            scroll_to = top.filter(|top| *top < self.scroll).or(top.map(|top| top + ROW_HEIGHT - self.room).filter(|to| *to > self.scroll));
        }
        // The names stay on the left while the frames scroll sideways; up and down, both go
        // together, and the ruler stays at the top.
        ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let wide = self.names_width();
            let (column, _) = ui.allocate_exact_size(vec2(wide, ui.available_height()), Sense::hover());
            // The wheel scrolls the rows over the names as it does over the cells.
            let wheel = if ui.rect_contains_pointer(column) { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
            if wheel != 0.0 {
                scroll_to = Some(self.scroll - wheel);
            }
            let mut area = egui::ScrollArea::both().auto_shrink(false);
            if std::mem::take(&mut self.fitting) {
                self.zoom = (ui.available_width() / shot.duration.max(1) as f32).clamp(*CELL_RANGE.start(), *CELL_RANGE.end());
                area = area.horizontal_scroll_offset(0.0);
            }
            if let Some(to) = scroll_to {
                area = area.vertical_scroll_offset(to.clamp(0.0, (height - RULER_HEIGHT - self.room).max(0.0)));
            }
            let shown = area.show_viewport(ui, |ui, viewport| {
                self.cells(ui, shot, &rows, selected, frame, viewport, &mut actions);
                viewport.min.y
            });
            (self.scroll, self.room) = (shown.inner, shown.inner_rect.height() - RULER_HEIGHT);
            // The names are as far up as the cells are, and are seen under the ruler only.
            let seen = Rect::from_x_y_ranges(column.x_range(), shown.inner_rect.top() + RULER_HEIGHT..=shown.inner_rect.bottom());
            #[cfg(test)]
            {
                self.rows_seen = Some(seen.union(shown.inner_rect.intersect(Rect::everything_below(seen.top()))));
            }
            let place = Rect::from_min_size(pos2(column.left(), column.top() - self.scroll), vec2(wide, height));
            let mut names = ui.new_child(egui::UiBuilder::new().id_salt("names").max_rect(place).layout(Layout::top_down(Align::LEFT)));
            names.set_clip_rect(seen.intersect(ui.clip_rect()));
            self.names(&mut names, shot, &rows, selected, picked, frame, &mut actions);
            // The column's right edge is dragged to make it wider or narrower, for names that
            // are longer than it; a double click puts it back as it starts.
            let edge = Rect::from_x_y_ranges(column.right() - 3.0..=column.right() + 3.0, column.y_range());
            let held = ui.interact(edge, ui.id().with("names edge"), Sense::click_and_drag());
            let held = held
                .on_hover_cursor(CursorIcon::ResizeHorizontal)
                .on_hover_text("Drag to make the names wider or narrower. A double click puts the column back");
            if held.dragged() {
                self.names_wide = Some((wide + held.drag_delta().x).clamp(*NAMES_RANGE.start(), *NAMES_RANGE.end()));
            }
            if held.double_clicked() {
                self.names_wide = None;
            }
            let theme = Theme::of(ui.ctx());
            let line = if held.dragged() || held.hovered() { Stroke::new(2.0, theme.accent) } else { Stroke::new(1.0, theme.border) };
            ui.painter().vline(column.right(), column.y_range(), line);
            #[cfg(test)]
            {
                self.names_edge = Some(edge);
            }
        });
        actions
    }

    /// Playback and the commands that act on the current cell.
    fn controls(
        &mut self,
        ui: &mut egui::Ui,
        shot: &Shot,
        layer: Option<LayerId>,
        frame: u32,
        playing: bool,
        hold: &mut NewDrawingHold,
        actions: &mut Vec<Action>,
        elsewhere: &mut dyn FnMut(&mut egui::Ui, &'static str),
    ) {
        // The bar's parts, each shown, hidden and put elsewhere by the bar itself.
        let mut bar = std::mem::take(&mut self.bar);
        let parts = bar.parts(crate::dock::Panel::Timeline);
        bar.show(ui, "timeline bar", &parts, |ui, part| match part {
            "play" => actions.extend(crate::bar::playback(ui, &mut self.bar_switches, playing, false).map(Action::Command)),
            "animate" => {
                let drawn = crate::bar::animate(ui, &mut self.animate, &mut self.adjust, self.keyed_here, self.can_key_all);
                #[cfg(test)]
                {
                    (self.animate_switch, self.adjust_switch) = (Some(drawn.switch), Some(drawn.adjust));
                    (self.key_button, self.key_all_button) = (Some(drawn.key), Some(drawn.all));
                }
                actions.extend(drawn.asked.map(Action::Command));
            }
            "volume" => {
                // The master volume, the project's: all of its sound, played and in a movie.
                ui.scope(|ui| {
                    ui.spacing_mut().slider_width = 60.0;
                    ui.weak("Volume").on_hover_text("The master volume: how loud all of the project's sound is, played and in a movie");
                    if let Some(volume) = crate::sound::volume_slider(ui, &mut self.volume_edit, 0, self.volume) {
                        actions.push(Action::Run("project.setVolume", json!({ "volume": volume })));
                    }
                });
            }
            "frame" => {
                ui.label(format!("Frame {} of", self.shown.unwrap_or(frame) + 1));
                // Typed, or dragged over: what it is dragged to is held here and set when it is
                // let go, as one undo step and not one for every value it passes.
                let mut duration = self.length_edit.unwrap_or(shot.duration);
                let field = egui::DragValue::new(&mut duration).speed(0.2).range(1..=100_000).update_while_editing(false);
                let typed = ui.add(field).on_hover_text("Shot length in frames: type it, or drag over it");
                #[cfg(test)]
                {
                    self.length_field = Some(typed.rect);
                }
                if typed.changed() {
                    self.length_edit = Some(duration);
                }
                if let Some(to) = self.length_edit.filter(|_| !typed.dragged() && !typed.has_focus()) {
                    self.length_edit = None;
                    if to != shot.duration {
                        actions.push(Action::Run("shot.setDuration", json!({ "shot": shot.id.0, "frames": to })));
                    }
                }
            }
            "layer" => actions.extend(crate::bar::add_layer(ui, Some(shot.id.0)).map(Action::Bar)),
            "marks" | "range" => {
                let here = shot.markers.iter().find(|marker| marker.frame == frame).map(|marker| marker.colour);
                let marks = crate::bar::Marks { of: ("shot", shot.id.0), frame, here, ranged: shot.range.is_some() };
                let asked = if part == "marks" { crate::bar::markers(ui, Some(&marks)) } else { crate::bar::range(ui, Some(&marks)) };
                actions.extend(asked.map(Action::Bar));
            }
            "convert" => actions.extend(crate::bar::convert(ui, !self.picked_keys.is_empty()).map(Action::Bar)),
            "cycle" => actions.extend(crate::bar::cycle(ui, self.cycled).map(Action::Bar)),
            "breakdown" => {
                if crate::breakdown::menu(ui, &mut self.breakdowns, !self.picked_keys.is_empty()) {
                    let commands: Vec<Value> = self.picked_keys.iter().flat_map(|(of, frame)| of.charted(shot, *frame, &[])).collect();
                    if !commands.is_empty() {
                        actions.push(Action::Run("edit.batch", json!({ "commands": commands })));
                    }
                }
            }
            "edit" => crate::bar::edit(ui, &mut self.bar_switches, false),
            "zoom" => {
                // 100% is a frame as wide as it starts.
                let range = CELL_RANGE.start() / CELL_WIDTH * 100.0..=CELL_RANGE.end() / CELL_WIDTH * 100.0;
                let hint = "How wide a frame is: at 100%, as it starts. Ctrl and the wheel over the frames zoom too";
                let zoomed = crate::bar::zoom(ui, self.frame_width() / CELL_WIDTH * 100.0, range, hint, "Fit the whole shot in the timeline's width");
                if let Some(percent) = zoomed.to {
                    self.zoom = percent / 100.0 * CELL_WIDTH;
                }
                self.fitting |= zoomed.fit;
                #[cfg(test)]
                {
                    self.fit_button = Some(zoomed.fit_button);
                }
            }
            "drawings" => actions.extend(crate::bar::drawings(ui, hold, layer.is_some(), self.extendable).map(Action::Bar)),
            // A part added to the bar that acts on the app: the app draws it.
            other => elsewhere(ui, other),
        });
        self.bar = bar;
    }

    /// The column of names: the shot's pegs and layers as a tree. Click selects, double-click
    /// renames, right-click has more.
    fn names(
        &mut self,
        ui: &mut egui::Ui,
        shot: &Shot,
        rows: &[(Row, usize)],
        selected: impl Fn(Node) -> bool,
        picked: &BTreeSet<u64>,
        frame: u32,
        actions: &mut Vec<Action>,
    ) {
        let muted = Theme::of(ui.ctx()).text_muted;
        #[cfg(test)]
        {
            self.names.clear();
            self.locks.clear();
            self.row_icons.clear();
            self.bone_names.clear();
            self.group_names.clear();
            self.effect_names.clear();
            self.peg_arrows.clear();
            self.openers.clear();
            self.value_names.clear();
            self.eyes.clear();
            self.row_locks.clear();
            self.sound_switches.clear();
        }
        let height = RULER_HEIGHT + ROW_HEIGHT * rows.len() as f32 + SOUND_HEIGHT * self.sound.len() as f32;
        let wide = self.names_width();
        ui.allocate_ui_with_layout(vec2(wide, height), Layout::top_down(Align::LEFT), |ui| {
            ui.allocate_exact_size(vec2(wide, RULER_HEIGHT), Sense::hover());
            for &(row, depth) in rows {
                let (rect, _) = ui.allocate_exact_size(vec2(wide, ROW_HEIGHT), Sense::hover());
                let node = match row {
                    Row::Node(node) => node,
                    Row::Bones(bones, layer) => {
                        let rect = self.opener(ui, rect, bones.0);
                        self.bones_name(ui, shot, rect, depth, (bones, layer), frame, actions);
                        continue;
                    }
                    Row::Group(group) => {
                        self.group_name(ui, shot, rect, depth, group, actions);
                        continue;
                    }
                    Row::Effect(effect, _) => {
                        let rect = self.opener(ui, rect, effect.0);
                        self.effect_name(ui, shot, rect, depth, effect, picked, frame, actions);
                        continue;
                    }
                    Row::Value(of) => {
                        self.value_name(ui, shot, rect, depth, of, frame, actions);
                        continue;
                    }
                };
                let name = match node {
                    Node::Peg(id) => shot.peg(id).map(|peg| &peg.name),
                    Node::Layer(id) => shot.layer(id).map(|layer| &layer.name),
                };
                let Some(name) = name else { continue };
                let rect = self.eye(ui, shot, rect, node_param(node).1, actions);
                // A peg's values open under it by the `+` after its name, beside its eye.
                let rect = match node {
                    Node::Peg(peg) => self.opener(ui, rect, peg.0),
                    Node::Layer(_) => rect,
                };
                let mut inside = rect.shrink2(vec2(4.0, 1.0));
                inside.min.x += INDENT * depth as f32;
                let mut row = ui.new_child(egui::UiBuilder::new().max_rect(inside).layout(Layout::left_to_right(Align::Center)));
                // A name that is longer than its room is cut off there, clear of the switch beside it.
                row.set_clip_rect(inside.intersect(ui.clip_rect()));
                let (kind, id) = node_param(node);
                // What kind of row it is: a peg, or a layer with its drawings.
                let board = match node {
                    Node::Layer(layer) if shot.is_board(layer) => Some(layer),
                    _ => None,
                };
                // Every row keeps a place for the arrow that folds a peg, so that the names stay
                // in line. A peg that carries something has the arrow there: it puts the rows
                // of what the peg carries away, and brings them back.
                let (slot, _) = row.allocate_exact_size(vec2(12.0, ROW_HEIGHT - 2.0), Sense::hover());
                if let Node::Peg(peg) = node
                    && (shot.pegs.iter().any(|other| other.parent == Some(peg)) || shot.layers.iter().any(|layer| layer.parent == Some(peg)))
                {
                    let folded = shot.folded.contains(&peg);
                    let hint = if folded { "Open the peg: show the rows of what it carries" } else { "Fold the peg: put the rows of what it carries away" };
                    let arrow = row.put(slot, egui::Button::new(if folded { "›" } else { "⌄" }).frame(false).small()).on_hover_text(hint);
                    #[cfg(test)]
                    self.peg_arrows.push((peg, arrow.rect));
                    if arrow.clicked() {
                        actions.push(Action::Run("peg.setFolded", json!({ "peg": peg.0, "folded": !folded })));
                    }
                }
                // A layer says what it holds: pixels, vector art, or pictures brought in.
                let of = match node {
                    Node::Layer(id) => shot.layer(id),
                    Node::Peg(_) => None,
                };
                let (icon, hint) = match of {
                    None => ("peg", "A peg: it moves what it carries"),
                    Some(layer) if layer.media.is_some() => ("image", "Pictures that were brought in"),
                    Some(layer) if layer.text => ("text", "A text layer: its drawings are texts, typed with the Text tool"),
                    Some(layer) if layer.is_vector() => ("layer-vector", "A vector layer: its drawings are shapes"),
                    Some(layer) if layer.is_mixed() => ("layer-bitmap", "A layer from before a layer held one kind: some of its art is vector, some pixels"),
                    Some(_) => ("layer-bitmap", "A bitmap layer: its drawings are pixels"),
                };
                row.add(icons::image(icon, 12.0, muted)).on_hover_text(hint);
                #[cfg(test)]
                self.row_icons.push((node, icon));
                row.add_space(3.0);
                if let Some((_, text)) = self.renaming.as_mut().filter(|(renamed, _)| *renamed == Row::Node(node)) {
                    let edit = row.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
                    if edit.lost_focus() {
                        // Enter keeps the new name; Escape, or the same name, changes nothing.
                        let keep = !row.input(|i| i.key_pressed(egui::Key::Escape)) && text != name;
                        if let Some((_, text)) = self.renaming.take().filter(|_| keep) {
                            let rename = if kind == "peg" { "peg.rename" } else { "layer.rename" };
                            actions.push(Action::Run(rename, json!({ kind: id, "name": text })));
                        }
                    } else {
                        edit.request_focus();
                    }
                    continue;
                }
                // A board that is locked is written fainter, and so is a row that is hidden.
                let gone = shot.hidden.contains(&id) || matches!(node, Node::Layer(layer) if shot.hides(layer));
                let written = match board {
                    Some(layer) if !shot.unlocked.contains(&layer) => egui::RichText::new(name).color(muted),
                    _ if gone => egui::RichText::new(name).color(muted),
                    _ => egui::RichText::new(name),
                };
                // A name longer than its room ends in "…" there, clear of the eye beside it.
                let label = row.add(egui::Button::selectable(selected(node) || picked.contains(&id), written).truncate());
                #[cfg(test)]
                self.names.push((node, label.rect));
                if label.clicked() {
                    // With Shift, all the rows from the one clicked last to this one are
                    // picked together; with the command key this one joins them or leaves.
                    // A plain click selects it alone, to draw on or to pose.
                    let (shift, command) = row.input(|i| (i.modifiers.shift, i.modifiers.command));
                    let keys: Vec<u64> = rows.iter().filter_map(|(row, _)| if let Row::Node(_) = row { Some(row.key()) } else { None }).collect();
                    let place = |key: u64| keys.iter().position(|other| *other == key);
                    match (shift, command, self.anchor.and_then(place), place(id)) {
                        (true, _, Some(from), Some(to)) => {
                            let range = keys[from.min(to)..=from.max(to)].iter().copied();
                            actions.push(Action::Pick(picked.iter().copied().chain(range).collect()));
                        }
                        (false, true, ..) => {
                            let mut all = picked.clone();
                            if !all.remove(&id) {
                                all.insert(id);
                            }
                            actions.push(Action::Pick(all.into_iter().collect()));
                            self.anchor = Some(id);
                        }
                        _ => {
                            actions.push(Action::Select { node: Some(node), frame });
                            self.anchor = Some(id);
                        }
                    }
                }
                // A board that is locked here is reference: it is not renamed, deleted or
                // changed on its row until its lock is opened.
                let locked = board.is_some_and(|layer| !shot.unlocked.contains(&layer));
                if label.double_clicked() && !locked {
                    self.renaming = Some((Row::Node(node), name.clone()));
                }
                // A row dragged away by its name goes to the library, if that is where it is
                // let go.
                let part = match node {
                    Node::Peg(id) => Part::Peg(id),
                    Node::Layer(id) => Part::Layer(id),
                };
                let grip = label.interact(Sense::drag());
                if grip.dragged() {
                    crate::drag_label(row.ctx(), name);
                }
                if grip.drag_stopped()
                    && let Some(at) = grip.interact_pointer_pos()
                {
                    // With the others it is picked with, if it is one of several.
                    let parts = if picked.contains(&id) && picked.len() > 1 { parts_of(shot, picked) } else { vec![part] };
                    actions.push(Action::Save { parts, at: Some(at) });
                }
                label.context_menu(|ui| {
                    if locked {
                        ui.weak("A board, locked here: open its lock to change it");
                        if ui
                            .button("Use as Animation")
                            .on_hover_text("A copy of this board as an animation layer, to clean up: the board stays as it is")
                            .clicked()
                        {
                            actions.push(Action::Run("layer.useAsAnimation", json!({ "layer": id })));
                            ui.close();
                        }
                        return;
                    }
                    if ui.button("Rename").clicked() {
                        self.renaming = Some((Row::Node(node), name.clone()));
                        ui.close();
                    }
                    // Into a group, or out of the one it is in. A new group of a node with
                    // what goes with it is made in the node view.
                    let own = shot.group_of(id).map(|group| group.id);
                    if !shot.groups.is_empty() {
                        ui.menu_button("Group", |ui| {
                            if ui.radio(own.is_none(), "None").clicked() {
                                actions.push(Action::Run("group.remove", json!({ "nodes": [id] })));
                                ui.close();
                            }
                            for group in &shot.groups {
                                if ui.radio(own == Some(group.id), &group.name).clicked() {
                                    actions.push(Action::Run("group.add", json!({ "group": group.id.0, "nodes": [id] })));
                                    ui.close();
                                }
                            }
                        });
                    }
                    // Of all that is picked with it, if it is one of several.
                    let several = picked.contains(&id) && picked.len() > 1;
                    let (title, hint) = if several {
                        ("Group the Picked", "A group of the rows that are picked together")
                    } else {
                        ("New Group", "A group of this row alone, to put others into")
                    };
                    if ui.button(title).on_hover_text(hint).clicked() {
                        let nodes: Vec<u64> = if several { picked.iter().copied().collect() } else { vec![id] };
                        actions.push(Action::Run("group.new", json!({ "nodes": nodes, "ports": true })));
                        ui.close();
                    }
                    if several && ui.button("Group the Picked with a Composite").on_hover_text(crate::nodes::GATHER_HINT).clicked() {
                        actions
                            .push(Action::Run("group.new", json!({ "nodes": picked.iter().copied().collect::<Vec<_>>(), "composite": true, "ports": true })));
                        ui.close();
                    }
                    // What is asked of one row's peg is asked of all that are picked with it.
                    let for_all = |ask: Parenting| parenting(shot, picked, ask).filter(|_| several).map(|batch| Action::Run("edit.batch", batch));
                    let above = if kind == "peg" { "abovePeg" } else { "aboveLayer" };
                    let (title, hint) = match for_all(Parenting::Above) {
                        Some(_) => ("Add Parent Pegs", "A new peg above each that is picked, between it and what it is parented to"),
                        None => ("Add Parent Peg", "A new peg between this and what it is parented to"),
                    };
                    if ui.button(title).on_hover_text(hint).clicked() {
                        actions.push(for_all(Parenting::Above).unwrap_or(Action::Run("peg.new", json!({ above: id }))));
                        ui.close();
                    }
                    ui.menu_button("Parent To", |ui| {
                        let set_parent = if kind == "peg" { "peg.setParent" } else { "layer.setParent" };
                        let parent = shot.parent_of(node);
                        // A port, a Send or a Receive it is parented to is set in the node view.
                        let port = parent.and_then(|parent| shot.effect(iskra_model::EffectId(parent.0)));
                        if let Some(port) = port {
                            ui.add_enabled(false, egui::RadioButton::new(true, &port.name)).on_disabled_hover_text("Wired in the node view");
                        }
                        if ui.radio(parent.is_none(), "Nothing").clicked() {
                            actions.push(for_all(Parenting::Off).unwrap_or(Action::Run(set_parent, json!({ kind: id }))));
                            ui.close();
                        }
                        // A peg can go under any peg but itself and those it carries.
                        let carried = |peg: PegId| matches!(node, Node::Peg(this) if shot.ancestors(Some(peg)).any(|above| above.id == this));
                        for peg in shot.pegs.iter().filter(|peg| !carried(peg.id)) {
                            if ui.radio(parent == Some(peg.id), &peg.name).clicked() {
                                let alone = Action::Run(set_parent, json!({ kind: id, "parent": peg.id.0 }));
                                actions.push(for_all(Parenting::Under(peg.id.0)).unwrap_or(alone));
                                ui.close();
                            }
                        }
                    });
                    let Node::Layer(layer) = node else {
                        let folded = shot.folded.contains(&PegId(id));
                        if ui
                            .button(if folded { "Unfold" } else { "Fold" })
                            .on_hover_text("Put the rows of what the peg carries away, or bring them back")
                            .clicked()
                        {
                            actions.push(Action::Run("peg.setFolded", json!({ "peg": id, "folded": !folded })));
                            ui.close();
                        }
                        if ui.button("Delete").on_hover_text("What it carries stays, parented to what this peg was parented to").clicked() {
                            actions.push(Action::Run("peg.delete", json!({ "peg": id })));
                            ui.close();
                        }
                        return;
                    };
                    let Some(layer) = shot.layer(layer) else { return };
                    if board.is_some()
                        && ui
                            .button("Use as Animation")
                            .on_hover_text("A copy of this board as an animation layer, to clean up: the board stays as it is")
                            .clicked()
                    {
                        actions.push(Action::Run("layer.useAsAnimation", json!({ "layer": layer.id.0 })));
                        ui.close();
                    }
                    // Over or under the others its composite stacks. A layer that is connected
                    // to none is neither.
                    let (index, count) = shot.place(layer.id).unwrap_or((0, 0));
                    for (name, to) in [("Move Up", index.checked_sub(1)), ("Move Down", Some(index + 1).filter(|&i| i < count))] {
                        if ui.add_enabled(to.is_some(), egui::Button::new(name)).on_hover_text("Over or under the other layers its composite stacks").clicked()
                        {
                            actions.push(Action::Run("layer.move", json!({ "layer": layer.id.0, "index": to })));
                            ui.close();
                        }
                    }
                    ui.menu_button("Resolution", |ui| {
                        for scale in [1.0, 2.0, 4.0] {
                            let label = format!("{scale}× the shot");
                            if ui
                                .radio(layer.density == scale, label)
                                .on_hover_text("Resamples this layer's drawings. Enlarging adds pixels, not detail")
                                .clicked()
                            {
                                actions.push(Action::Run("layer.setDensity", json!({ "layer": layer.id.0, "density": scale })));
                                ui.close();
                            }
                        }
                    });
                    // The bones that bend it: a second layer is given to a chain here, a sleeve
                    // to the arm's.
                    if !shot.deformers.is_empty() {
                        ui.menu_button("Bones", |ui| {
                            if ui.radio(layer.deformer.is_none(), "None").on_hover_text("As it was drawn").clicked() {
                                actions.push(Action::Run("layer.setDeformer", json!({ "layer": layer.id.0 })));
                                ui.close();
                            }
                            for bones in &shot.deformers {
                                if ui.radio(layer.deformer == Some(bones.id), &bones.name).clicked() {
                                    actions.push(Action::Run("layer.setDeformer", json!({ "layer": layer.id.0, "deformer": bones.id.0 })));
                                    ui.close();
                                }
                            }
                        })
                        .response
                        .on_hover_text(
                            "The chain of bones that bends this layer's drawings. Several layers can have one chain; a new one is laid with the Bones tool",
                        );
                    }
                    // A Pass of the layer alone, rendered apart on the Export page.
                    let hint = "A Pass wired from this layer: the Export page can render it apart, under the pass's name";
                    if ui.button("Add a Pass of It").on_hover_text(hint).clicked() {
                        actions.push(Action::Run("display.new", json!({ "node": { "layer": layer.id.0 }, "pass": true })));
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        actions.push(Action::Run("layer.delete", json!({ "layer": layer.id.0 })));
                        ui.close();
                    }
                });
            }
            // The sound rows are named after their tracks.
            // Each has its switches at the right: the track muted, which is the project's;
            // its clips locked where they are on the storyboard; and its waveform put away.
            for at in 0..self.sound.len() {
                let (rect, _) = ui.allocate_exact_size(vec2(wide, SOUND_HEIGHT), Sense::hover());
                let rect = self.sound_switches(ui, rect, at, actions);
                let sound = &self.sound[at];
                // How loud each of its clips under the shot is, from the row's menu.
                let named = ui.interact(rect, ui.id().with(("sound row", sound.track)), Sense::click());
                named.on_hover_text("Right-click for the volume of its clips").context_menu(|ui| {
                    ui.weak("Volume");
                    for (clip, name, gain) in &sound.clips {
                        ui.horizontal(|ui| {
                            if let Some(gain) = crate::sound::volume_slider(ui, &mut self.volume_edit, *clip, *gain) {
                                actions.push(Action::Run("sound.setClip", json!({ "clip": clip, "gain": gain })));
                            }
                            ui.label(name);
                        });
                    }
                });
                ui.painter().with_clip_rect(rect.intersect(ui.clip_rect())).text(
                    rect.left_center() + vec2(8.0, 0.0),
                    Align2::LEFT_CENTER,
                    &sound.name,
                    FontId::proportional(11.0),
                    muted,
                );
            }
        });
    }

    /// The ruler and the grid of cells. Clicking or dragging in it picks the frame, and a press
    /// on a row also selects that row's node; dragging the end of a block sets its hold.
    /// `viewport` is the part of the cells that is seen: as far down as it is, the rows are
    /// scrolled, and the ruler stays where it is.
    fn cells(
        &mut self,
        ui: &mut egui::Ui,
        shot: &Shot,
        rows: &[(Row, usize)],
        selected: impl Fn(Node) -> bool,
        frame: u32,
        viewport: Rect,
        actions: &mut Vec<Action>,
    ) {
        let cw = self.frame_width();
        let scroll = viewport.min.y;
        // Keys, holds and markers past the shot's end are shown too, up to the last of them,
        // darkened, as what is not played. Past that there is room of half of what is seen, so
        // that the frames scroll sideways however short the shot is, and its end can be brought
        // to the middle.
        self.reach = past_the_end(shot).map_or(shot.duration, |last| last + 1);
        let reach = self.reach;
        let width = cw * reach as f32;
        let size = vec2(width + viewport.width() / 2.0, RULER_HEIGHT + ROW_HEIGHT * rows.len() as f32 + SOUND_HEIGHT * self.sound.len() as f32);
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let origin = response.rect.min;
        let visuals = ui.visuals().clone();
        let theme = Theme::of(ui.ctx());
        let (faint, text, accent) = (theme.text_muted, theme.text, theme.accent);
        let column = |f: u32| origin.x + cw * f as f32;
        let row_top = |row: usize| origin.y + RULER_HEIGHT + ROW_HEIGHT * row as f32;
        self.sound_drop = self.film_start.map(|(storyboard, first)| crate::sound::SoundDrop {
            storyboard,
            // Down to the bottom of the panel, where a new track is.
            area: response.rect.with_max_y(response.rect.max.y.max(ui.clip_rect().max.y)).intersect(ui.clip_rect()),
            zero: origin.x,
            frame_width: cw,
            first,
            last: Some(shot.duration.saturating_sub(1)),
            sound_top: row_top(rows.len()),
            row_height: SOUND_HEIGHT,
            tracks: self.sound.iter().map(|sound| sound.track).collect(),
            count: self.sound_tracks,
        });
        // The ruler stays at the top of what is seen while the rows scroll under it: the rows
        // are drawn and pressed only below it.
        let top = origin.y + scroll;
        let below = Rect::everything_below(top + RULER_HEIGHT);

        // A drag that starts on a row of keyframes, a peg's, a chain of bones', an effect's or
        // one value's of them, pulls out a box that picks the keyframes it touches; anywhere
        // else it moves the playhead.
        let row_at = |pos: egui::Pos2| {
            let row = Some(((pos.y - row_top(0)) / ROW_HEIGHT).floor()).filter(|row| *row >= 0.0 && below.contains(pos));
            row.and_then(|row| rows.get(row as usize)).map(|(row, _)| *row)
        };
        let pressed_on = ui.input(|i| i.pointer.press_origin()).and_then(row_at);
        let boxing = response.dragged() && matches!(pressed_on, Some(Row::Node(Node::Peg(_)) | Row::Bones(..) | Row::Effect(..) | Row::Value(_)));
        if let (true, Some(from), Some(to)) = (boxing, ui.input(|i| i.pointer.press_origin()), response.interact_pointer_pos()) {
            self.key_marquee = Some((from, to));
            // The press still selects the row it lands on; the playhead stays.
            if let (true, Some(node)) = (response.drag_started(), pressed_on.and_then(Row::node)) {
                actions.push(Action::Select { node: Some(node), frame });
            }
        }
        // A drag that starts on a layer's row picks its frames, from where it was pressed to
        // where the pointer is. One that starts on frames that are picked takes them along.
        // Not on a board that is locked here, whose frames are not changed until it is opened.
        let locked = |layer: LayerId| shot.is_board(layer) && !shot.unlocked.contains(&layer);
        let spanning = response.dragged() && matches!(pressed_on, Some(Row::Node(Node::Layer(layer))) if !locked(layer));
        if let (true, Some(Row::Node(Node::Layer(layer))), Some(from), Some(to)) =
            (spanning, pressed_on, ui.input(|i| i.pointer.press_origin()), response.interact_pointer_pos())
        {
            let (from, to) = (frame_at(from.x - origin.x, shot, cw), frame_at(to.x - origin.x, shot, cw));
            if response.drag_started() {
                let held = self.frames.is_some_and(|picked| picked.layer == layer && (picked.first..=picked.last).contains(&from));
                self.frames_drag = held.then_some(0);
                // The press selects the row and puts the playhead where it lands; there it stays.
                if !held {
                    actions.push(Action::Select { node: Some(Node::Layer(layer)), frame: if self.hold_playhead { frame } else { from } });
                }
            }
            match self.frames_drag.as_mut() {
                Some(by) => *by = i64::from(to) - i64::from(from),
                None => self.frames = Some(Frames { layer, first: from.min(to), last: from.max(to) }),
            }
        }
        // Let go, the frames that were dragged are moved, and stay picked where they went.
        if !response.dragged()
            && let Some(by) = self.frames_drag.take()
            && let Some(picked) = self.frames.as_mut()
        {
            let to = moved_to(*picked, by, shot);
            if to != picked.first {
                actions.push(Action::Run("exposure.move", json!({ "layer": picked.layer.0, "frame": picked.first, "length": picked.length(), "to": to })));
                (picked.first, picked.last) = (to, to + picked.length() - 1);
            }
        }
        // A click on a layer's cell picks that frame, with Shift from the frame picked on its
        // row to it; elsewhere it lets the picked frames go.
        if response.clicked() {
            self.picked_keys.clear();
            let clicked = response.interact_pointer_pos().and_then(|at| match row_at(at) {
                Some(Row::Node(Node::Layer(layer))) if !locked(layer) => Some((layer, frame_at(at.x - origin.x, shot, cw))),
                _ => None,
            });
            let shift = ui.input(|i| i.modifiers.shift);
            self.frames = clicked.map(|(layer, at)| match self.frames.filter(|picked| shift && picked.layer == layer) {
                Some(picked) => Frames { layer, first: picked.first.min(at), last: picked.last.max(at) },
                None => Frames { layer, first: at, last: at },
            });
        }
        // Ctrl and the wheel over the cells zoom the frames.
        let zoom = ui.input(|i| i.zoom_delta());
        if zoom != 1.0 && ui.rect_contains_pointer(response.rect.intersect(below)) {
            self.zoom = (cw * zoom).clamp(*CELL_RANGE.start(), *CELL_RANGE.end());
        }
        // The frames that are picked have the Edit menu's commands for them here too.
        response.context_menu(|ui| {
            let picked = self.frames.is_some();
            for (title, id, needs) in [
                ("Cut Frames", "edit.cut", true),
                ("Copy Frames", "edit.copy", true),
                ("Paste at the Playhead", "edit.paste", false),
                ("Delete Frames", "edit.delete", true),
            ] {
                if ui.add_enabled(picked || !needs, egui::Button::new(title)).clicked() {
                    actions.push(Action::Command(id));
                    ui.close();
                }
            }
        });
        // What a dragged playhead is caught by, with snapping: where a drawing starts, a
        // keyframe, a marker, an end of the stretch that plays.
        let catches = |keys: &[(Keyed, u32, Rect)]| -> Vec<u32> {
            let drawings = shot.layers.iter().flat_map(|layer| layer.exposure.runs().iter().map(|run| run.start));
            let marked = shot.markers.iter().map(|marker| marker.frame).chain(shot.range.into_iter().flat_map(|(first, after)| [first, after]));
            drawings.chain(keys.iter().map(|(_, frame, _)| *frame)).chain(marked).filter(|frame| *frame < shot.duration).collect()
        };
        let catches = (self.bar_switches.snap && response.dragged()).then(|| catches(&self.shown_keys));
        self.shown_keys.clear();
        if let Some(pos) = response.interact_pointer_pos().filter(|_| response.clicked() || (response.dragged() && !boxing && !spanning)) {
            // A row is selected by the press, on the row it lands on. After that a drag only
            // moves the playhead, whichever rows the pointer passes over.
            let pressed = if response.clicked() { Some(pos) } else { ui.input(|i| i.pointer.press_origin()).filter(|_| response.drag_started()) };
            let node = pressed.and_then(row_at).and_then(Row::node);
            let under = frame_at(pos.x - origin.x, shot, cw);
            let held = self.hold_playhead && response.clicked() && below.contains(pos);
            let to = if held { frame } else { catches.map_or(under, |catches| caught(under, catches, 1)) };
            actions.push(Action::Select { node, frame: to });
        }

        // The ruler: a tick per frame and a number on the first and every fifth.
        for f in 0..reach {
            let numbered = f == 0 || (f + 1) % 5 == 0;
            painter.vline(column(f), top + if numbered { 0.0 } else { RULER_HEIGHT * 0.6 }..=top + RULER_HEIGHT, Stroke::new(1.0, faint));
            if numbered {
                painter.text(pos2(column(f) + 3.0, top), Align2::LEFT_TOP, f + 1, theme::mono(10.0), faint);
            }
        }

        // A thin line along the foot of the ruler over the frames that are made and kept on
        // the card: those play at once, the others are made as the playhead comes to them.
        #[cfg(test)]
        self.cache_marks.clear();
        let mut cached = self.cached.iter().copied().filter(|frame| *frame < shot.duration).peekable();
        while let Some(first) = cached.next() {
            let mut after = first + 1;
            while cached.next_if_eq(&after).is_some() {
                after += 1;
            }
            painter.hline(column(first)..=column(after), top + RULER_HEIGHT - 1.5, Stroke::new(2.0, theme.colour(Role::Cached)));
            #[cfg(test)]
            self.cache_marks.push((first, after));
        }
        self.ruler_marks(ui, &painter, &response, shot, scroll, actions);
        let end_line = self.end_handle(ui, &painter, &response, shot, top, actions);
        let over = painter.clone();
        let painter = painter.with_clip_rect(painter.clip_rect().intersect(below));
        ui.set_clip_rect(ui.clip_rect().intersect(below));

        // A dragged hold moves the later drawings with Ripple on, and with Alt held it does
        // the other.
        let ripple = self.bar_switches.ripple != ui.input(|i| i.modifiers.alt);
        #[cfg(test)]
        {
            self.hold_handles.clear();
            self.key_marks.clear();
            self.bone_key_marks.clear();
            self.effect_key_marks.clear();
            self.value_key_marks.clear();
            self.keys_drawn.clear();
            self.repeated_marks.clear();
            self.coarse_marks.clear();
            self.tweens.clear();
            self.breakdown_lines.clear();
            self.put_away_marks.clear();
            self.frames_mark = None;
        }
        for (at, &(row, _)) in rows.iter().enumerate() {
            let lane = Rect::from_min_size(pos2(origin.x, row_top(at)), vec2(width, ROW_HEIGHT));
            let is_selected = row.node().is_some_and(&selected);
            if !matches!(row, Row::Node(_)) || is_selected {
                // The row of a layer's bones is of the layer: lit with it, or faintly; a
                // group's row is faint too.
                let lit = if is_selected { visuals.faint_bg_color } else { visuals.faint_bg_color.gamma_multiply(0.4) };
                painter.rect_filled(lane, 0.0, lit);
            }
            painter.hline(lane.x_range(), lane.bottom(), Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color));
            let layer = match row {
                Row::Node(Node::Layer(layer)) => shot.layer(layer),
                Row::Node(Node::Peg(peg)) => {
                    if let Some(peg) = shot.peg(peg) {
                        self.keys(ui, &painter, &response, shot, Keyed::Peg(peg.id, None), lane, actions);
                        // Folded, it shows where there is something on the rows it puts away.
                        if shot.folded.contains(&peg.id) {
                            let under = |parent| shot.ancestors(parent).any(|above| above.id == peg.id);
                            let frames = put_away(shot, |node| under(shot.parent_of(node)));
                            self.put_away_marks(&painter, peg.id.0, frames, lane);
                        }
                    }
                    None
                }
                Row::Group(group) => {
                    if shot.group(group).is_some_and(|group| group.rows_folded) {
                        let frames = put_away(shot, |node| shot.groups_above(node_param(node).1).any(|above| above.id == group));
                        self.put_away_marks(&painter, group.0, frames, lane);
                    }
                    None
                }
                Row::Effect(effect, _) => {
                    self.keys(ui, &painter, &response, shot, Keyed::Effect(effect, None), lane, actions);
                    None
                }
                Row::Bones(bones, layer) => {
                    self.keys(ui, &painter, &response, shot, Keyed::Bones(bones, layer, None), lane, actions);
                    None
                }
                Row::Value(of) => {
                    self.keys(ui, &painter, &response, shot, of, lane, actions);
                    None
                }
            };
            let Some(layer) = layer else { continue };
            // Where the camera shows the layer too large for its pixels: an amber line under
            // those frames.
            for &(start, end) in self.coarse.get(&layer.id).into_iter().flatten() {
                let mark = Rect::from_min_max(pos2(column(start) + 1.5, lane.bottom() - 3.0), pos2(column(end.min(shot.duration)) - 1.5, lane.bottom() - 1.0));
                painter.rect_filled(mark, 1.0, theme.colour(Role::Coarse));
                #[cfg(test)]
                self.coarse_marks.push((layer.id, start, mark));
                if response.hover_pos().is_some_and(|pos| below.contains(pos) && mark.expand(2.0).contains(pos)) {
                    response.clone().on_hover_text_at_pointer(
                        "On these frames the camera shows this layer larger than its pixels allow: it will look soft. Draw it at a higher resolution, or keep it smaller",
                    );
                }
            }
            // While a hold of this row is dragged, the row shows what letting go would give.
            let mut exposure = layer.exposure.clone();
            if let Some(drag) = self.hold_drag.filter(|d| d.layer == layer.id) {
                exposure.set_hold(drag.start, drag.len, ripple);
            }
            // And so while its picked frames are dragged: where they would land.
            let picked = self.frames.filter(|picked| picked.layer == layer.id && picked.last < shot.duration);
            let picked_at = picked.map(|picked| moved_to(picked, self.frames_drag.unwrap_or(0), shot));
            if let Some((picked, to)) = picked.zip(picked_at) {
                exposure.shift(picked.first, picked.length(), to);
            }
            for run in exposure.runs().iter().filter(|r| r.start < reach) {
                let end = run.end().min(reach);
                let block = Rect::from_min_max(pos2(column(run.start), lane.top()), pos2(column(end), lane.bottom())).shrink(1.5);
                painter.rect_filled(block, 3.0, theme.block_fill(is_selected));
                let name = layer.drawing(run.drawing).map_or("?", |d| d.name.as_str());
                painter.with_clip_rect(block.intersect(below)).text(
                    block.left_center() + vec2(3.0, 0.0),
                    Align2::LEFT_CENTER,
                    name,
                    FontId::proportional(11.0),
                    text,
                );

                // A board that is locked here has no handles to drag its holds by.
                if shot.is_board(layer.id) && !shot.unlocked.contains(&layer.id) {
                    continue;
                }
                let handle = Rect::from_min_max(pos2(block.right() - HANDLE_WIDTH, block.top()), block.max);
                #[cfg(test)]
                self.hold_handles.push((layer.id, run.start, handle));
                let grip = ui.interact(handle, response.id.with(("hold", layer.id.0, run.start)), Sense::drag()).on_hover_cursor(CursorIcon::ResizeHorizontal);
                if grip.drag_started() {
                    self.hold_drag = Some(HoldDrag { layer: layer.id, start: run.start, len: run.len });
                }
                let Some(drag) = self.hold_drag.as_mut().filter(|d| d.layer == layer.id && d.start == run.start) else {
                    if grip.hovered() {
                        painter.rect_filled(handle, 2.0, visuals.widgets.hovered.bg_fill);
                    }
                    continue;
                };
                painter.rect_stroke(block, 3.0, Stroke::new(1.5, accent), egui::StrokeKind::Inside);
                if let Some(pos) = grip.interact_pointer_pos() {
                    // The hold ends on the frame boundary nearest the pointer.
                    let end = ((pos.x - origin.x) / cw).round().max(0.0) as u32;
                    drag.len = end.saturating_sub(run.start).max(1);
                }
                if grip.drag_stopped() {
                    let (start, len) = (drag.start, drag.len);
                    self.hold_drag = None;
                    if layer.exposure.run_at(start).is_some_and(|before| before.len != len) {
                        actions.push(Action::Run("exposure.setHold", json!({ "layer": layer.id.0, "frame": start, "length": len, "ripple": ripple })));
                    }
                }
            }
            // The frames that are picked, marked over the blocks.
            if let Some((picked, at)) = picked.zip(picked_at) {
                let mark = Rect::from_min_max(pos2(column(at), lane.top()), pos2(column(at + picked.length()), lane.bottom())).shrink(0.5);
                painter.rect(mark, 2.0, accent.gamma_multiply(0.25), Stroke::new(1.5, accent), egui::StrokeKind::Inside);
                #[cfg(test)]
                {
                    self.frames_mark = Some(mark);
                }
            }
        }

        // The shot's sound, under its rows: for each frame a bar from the lowest to the highest
        // value in it, so a word's start and a beat are seen on the frame they fall on.
        #[cfg(test)]
        self.sound_rows.clear();
        for (index, sound) in self.sound.iter().enumerate() {
            let top = row_top(rows.len()) + SOUND_HEIGHT * index as f32;
            let lane = Rect::from_min_size(pos2(origin.x, top), vec2(width, SOUND_HEIGHT));
            #[cfg(test)]
            self.sound_rows.push(lane);
            painter.rect_filled(lane, 0.0, visuals.faint_bg_color);
            painter.hline(lane.x_range(), lane.bottom(), Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color));
            if self.sound_hidden.contains(&sound.track) {
                continue;
            }
            let (middle, half) = (lane.center().y, SOUND_HEIGHT / 2.0 - 2.0);
            for (at, peak) in sound.peaks.iter().enumerate().take(shot.duration as usize) {
                let Some([low, high]) = peak else { continue };
                let bar = Rect::from_min_max(
                    pos2(column(at as u32) + 1.0, middle - high.clamp(-1.0, 1.0) * half - 0.5),
                    pos2(column(at as u32 + 1) - 1.0, middle - low.clamp(-1.0, 1.0) * half + 0.5),
                );
                painter.rect_filled(bar, 1.0, faint);
            }
        }

        // The box that picks keyframes: drawn while it is pulled out, and when it is let go
        // the keyframes it touches are picked, beside the others with Shift held.
        if let Some((from, to)) = self.key_marquee {
            let pulled = Rect::from_two_pos(from, to).intersect(below);
            if boxing {
                painter.rect(pulled, 2.0, accent.gamma_multiply(0.12), Stroke::new(1.0, accent), egui::StrokeKind::Inside);
            } else {
                self.key_marquee = None;
                if !ui.input(|i| i.modifiers.shift) {
                    self.picked_keys.clear();
                }
                for (of, frame, mark) in &self.shown_keys {
                    if mark.intersects(pulled) && !self.picked_keys.contains(&(*of, *frame)) {
                        self.picked_keys.push((*of, *frame));
                    }
                }
            }
        }

        // What is outside the stretch that plays is dimmed.
        if shot.range.is_some() {
            let (first, after) = shot.played();
            let whole = Rect::from_min_size(origin, vec2(width, size.y));
            for outside in [Rect::from_min_max(whole.min, pos2(column(first), whole.max.y)), Rect::from_min_max(pos2(column(after), whole.min.y), whole.max)] {
                over.rect_filled(outside, 0.0, Color32::from_black_alpha(70));
            }
        }

        // What is past the end of the shot, not played, is darkened.
        #[cfg(test)]
        {
            self.past_end = None;
        }
        if reach > shot.duration {
            let past = Rect::from_min_max(pos2(column(shot.duration), top), pos2(column(reach), response.rect.bottom()));
            over.rect_filled(past, 0.0, Color32::from_black_alpha(110));
            if response.hover_pos().is_some_and(|pos| past.contains(pos)) {
                response
                    .clone()
                    .on_hover_text_at_pointer("Past the end of the shot: not played. Drag the end of the shot out to take it in, or the keys back into it");
            }
            #[cfg(test)]
            {
                self.past_end = Some(past);
            }
        }

        // The current frame, over everything.
        let playhead = Rect::from_min_max(pos2(column(frame), top), pos2(column(frame + 1), response.rect.bottom()));
        over.rect_filled(playhead, 0.0, accent.gamma_multiply(0.2));
        over.vline(playhead.left(), playhead.y_range(), Stroke::new(1.5, accent));
        let (x, lit) = end_line;
        over.vline(x, top + RULER_HEIGHT..=response.rect.bottom(), Stroke::new(1.0, lit.gamma_multiply(0.6)));
    }

    /// The handle at the end of the shot, on the ruler: dragged, it makes the shot longer or
    /// shorter, and the length is set once it is let go, as one undo step. While it is held
    /// the end it would have is drawn, with the length it would be. Where the end is, and its
    /// colour, for the line down the rows that is drawn over them.
    fn end_handle(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        cells: &egui::Response,
        shot: &Shot,
        top: f32,
        actions: &mut Vec<Action>,
    ) -> (f32, Color32) {
        let cw = self.frame_width();
        let origin = cells.rect.min;
        let end = self.end_drag.unwrap_or(shot.duration);
        let x = origin.x + cw * end as f32;
        let grip = Rect::from_min_max(pos2(x - 4.0, top), pos2(x + 4.0, top + RULER_HEIGHT));
        #[cfg(test)]
        {
            self.end_grip = Some(grip);
        }
        let held = ui.interact(grip, cells.id.with("end of the shot"), Sense::drag());
        let held = held.on_hover_cursor(egui::CursorIcon::ResizeHorizontal).on_hover_text("The end of the shot: drag to make it longer or shorter");
        if held.drag_started() {
            self.end_drag = Some(shot.duration);
        }
        if let (Some(drag), Some(now)) = (self.end_drag.as_mut(), held.interact_pointer_pos()) {
            *drag = ((now.x - origin.x) / cw).round().clamp(1.0, 100_000.0) as u32;
        }
        if held.drag_stopped()
            && let Some(to) = self.end_drag.take()
            && to != shot.duration
        {
            actions.push(Action::Run("shot.setDuration", json!({ "shot": shot.id.0, "frames": to })));
        }
        let theme = Theme::of(ui.ctx());
        let lit = if held.hovered() || held.dragged() { theme.accent } else { theme.text_muted };
        painter.rect_filled(grip.shrink2(vec2(1.0, 2.0)), 2.0, lit);
        if self.end_drag.is_some() {
            painter.text(pos2(x + 7.0, top + RULER_HEIGHT / 2.0), Align2::LEFT_CENTER, format!("{end} frames"), theme::mono(10.0), theme.text);
        }
        (x, lit)
    }
}

impl Timeline {
    /// What lies on the ruler beside the frames' numbers, as on the storyboard's: the stretch
    /// that plays, a bar along the top with an end at its in point and one at its out point,
    /// and the markers, each a flag of its colour with its name. A click on a marker goes to
    /// it, a drag moves it, its menu names and colours it and takes it away.
    /// `scroll` is how far down the rows are scrolled, which is how far down the ruler is in
    /// the cells.
    fn ruler_marks(&mut self, ui: &egui::Ui, painter: &egui::Painter, cells: &egui::Response, shot: &Shot, scroll: f32, actions: &mut Vec<Action>) {
        let cw = self.frame_width();
        let (origin, accent) = (cells.rect.min + vec2(0.0, scroll), Theme::of(ui.ctx()).accent);
        let column = |f: u32| origin.x + cw * f as f32;
        #[cfg(test)]
        {
            self.range_ends = None;
            self.markers.clear();
        }
        if shot.range.is_some() {
            let (first, after) = shot.played();
            let (from, to) = (column(first), column(after));
            painter.rect_filled(Rect::from_min_max(pos2(from, origin.y), pos2(to, origin.y + 3.0)), 0.0, accent);
            for (x, inwards) in [(from, 5.0), (to, -5.0)] {
                painter.add(egui::Shape::convex_polygon(vec![pos2(x, origin.y), pos2(x + inwards, origin.y), pos2(x, origin.y + 9.0)], accent, Stroke::NONE));
                painter.vline(x, origin.y..=origin.y + RULER_HEIGHT, Stroke::new(1.0, accent));
            }
            #[cfg(test)]
            {
                self.range_ends = Some((from, to));
            }
        }
        let last = shot.duration.saturating_sub(1);
        for marker in shot.markers.iter().filter(|marker| marker.frame < self.reach) {
            let at = self.marker_drag.filter(|drag| drag.0 == marker.frame).map_or(marker.frame, |drag| drag.1);
            let x = column(at) + cw / 2.0;
            let flag = Rect::from_min_max(pos2(x - 4.0, origin.y), pos2(x + 5.0, origin.y + RULER_HEIGHT));
            #[cfg(test)]
            self.markers.push((marker.frame, flag));
            let [r, g, b] = marker.colour;
            let colour = Color32::from_rgb(r, g, b);
            painter.vline(x, origin.y..=cells.rect.bottom(), Stroke::new(1.0, colour.gamma_multiply(0.6)));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(x - 4.0, origin.y + 1.0),
                    pos2(x + 5.0, origin.y + 1.0),
                    pos2(x + 5.0, origin.y + 8.0),
                    pos2(x, origin.y + 12.0),
                    pos2(x - 4.0, origin.y + 8.0),
                ],
                colour,
                Stroke::NONE,
            ));
            if !marker.name.is_empty() {
                painter.text(pos2(x + 8.0, origin.y + RULER_HEIGHT - 1.0), Align2::LEFT_BOTTOM, &marker.name, FontId::proportional(10.0), colour);
            }
            let held = ui.interact(flag, cells.id.with(("marker", marker.frame)), Sense::click_and_drag());
            let said = if marker.name.is_empty() { "A marker".to_string() } else { marker.name.clone() };
            let held = held.on_hover_text(format!("{said}\nDrag to move it; right-click to name or colour it, or to remove it"));
            if held.clicked() {
                actions.push(Action::Select { node: None, frame: marker.frame });
            }
            if held.drag_started() {
                self.marker_drag = Some((marker.frame, marker.frame));
            }
            if let Some(drag) = self.marker_drag.as_mut().filter(|drag| drag.0 == marker.frame) {
                if let Some(now) = held.interact_pointer_pos() {
                    drag.1 = frame_at(now.x - origin.x, shot, cw).min(last);
                }
                if held.drag_stopped() {
                    let to = drag.1;
                    self.marker_drag = None;
                    if to != marker.frame {
                        actions.push(Action::Run("shot.setMarker", json!({ "shot": shot.id.0, "frame": marker.frame, "to": to })));
                    }
                }
            }
            held.context_menu(|ui| {
                // Its name, written when the field is left.
                if self.marker_name.as_ref().is_none_or(|(frame, _)| *frame != marker.frame) {
                    self.marker_name = Some((marker.frame, marker.name.clone()));
                }
                if let Some((_, name)) = self.marker_name.as_mut() {
                    let field = ui.add(egui::TextEdit::singleline(name).hint_text("Name").desired_width(120.0));
                    if field.lost_focus() && *name != marker.name {
                        actions.push(Action::Run("shot.setMarker", json!({ "shot": shot.id.0, "frame": marker.frame, "name": name })));
                    }
                }
                ui.separator();
                for (name, colour) in iskra_model::MARKER_COLOURS {
                    ui.horizontal(|ui| {
                        let (swatch, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                        ui.painter().rect_filled(swatch, 2.0, Color32::from_rgb(colour[0], colour[1], colour[2]));
                        if ui.radio(marker.colour == colour, name).clicked() {
                            actions.push(Action::Run("shot.setMarker", json!({ "shot": shot.id.0, "frame": marker.frame, "colour": colour })));
                            ui.close();
                        }
                    });
                }
                ui.separator();
                if ui.button("Remove Marker").clicked() {
                    actions.push(Action::Run("shot.removeMarker", json!({ "shot": shot.id.0, "frame": marker.frame })));
                    ui.close();
                }
            });
        }
        // The retake notes, each a band at the ruler's foot over its frames; a click goes to it.
        #[cfg(test)]
        self.note_marks.clear();
        for note in shot.notes.iter().filter(|note| note.frame < self.reach) {
            let band = Rect::from_min_max(
                pos2(column(note.frame) + 1.0, origin.y + RULER_HEIGHT - 4.0),
                pos2(column(note.end()) - 1.0, origin.y + RULER_HEIGHT - 1.0),
            );
            #[cfg(test)]
            self.note_marks.push((note.id, band));
            if crate::notes::mark(ui, painter, band, cells.id.with(("note", note.id)), note).clicked() {
                actions.push(Action::Select { node: None, frame: note.frame });
            }
        }
    }

    /// The switches at the right end of a row of a layer, a peg or a group: the lock, which
    /// keeps it from being drawn on or picked in the viewport (`shot.setLocked`), and the eye,
    /// which hides it (`shot.setHidden`); for a peg or a group, all it carries or has in it.
    /// A board's lock is also the one that keeps it reference on this page until it is opened
    /// (`shot.setUnlocked`, plan §7.2c): it shows locked while either is shut, and opens both.
    /// What is left of the row, for its name, is given back.
    fn eye(&mut self, ui: &mut egui::Ui, shot: &Shot, rect: Rect, id: u64, actions: &mut Vec<Action>) -> Rect {
        let (left, slot) = rect.split_left_right_at_x(rect.right() - 40.0);
        let (hidden, theme) = (shot.hidden.contains(&id), Theme::of(ui.ctx()));
        let board = Some(LayerId(id)).filter(|layer| shot.layer(*layer).is_some() && shot.is_board(*layer));
        let row_locked = shot.locked.contains(&id);
        let shut = board.is_some_and(|layer| !shot.unlocked.contains(&layer));
        let locked = row_locked || shut;
        let hint = match (board, locked) {
            (Some(_), true) => "A board: what the animation is made after. It is locked here; click to draw on it",
            (Some(_), false) => "A board, open to be drawn on here. Click to lock it again",
            (None, true) => "Locked: not drawn on nor picked in the viewport. Click to unlock it",
            (None, false) => "Lock it: a layer, or all that a peg carries or a group has in it, so that it is not drawn on nor picked in the viewport",
        };
        let (icon, tint) = if locked { ("lock", theme.accent) } else { ("lock-open", theme.text_muted) };
        let mut at = ui.new_child(egui::UiBuilder::new().id_salt(("row lock", id)).max_rect(slot).layout(Layout::left_to_right(Align::Center)));
        let lock = at.add(egui::Button::image(icons::image(icon, 12.0, tint)).frame(false)).on_hover_text(hint);
        #[cfg(test)]
        {
            self.row_locks.push((id, lock.rect));
            if let Some(layer) = board {
                self.locks.push((layer, lock.rect));
            }
        }
        if lock.clicked() {
            match board {
                Some(layer) if locked => {
                    if shut {
                        actions.push(Action::Run("shot.setUnlocked", json!({ "layer": layer.0, "unlocked": true })));
                    }
                    if row_locked {
                        actions.push(Action::Run("shot.setLocked", json!({ "node": id, "locked": false })));
                    }
                }
                Some(layer) => actions.push(Action::Run("shot.setUnlocked", json!({ "layer": layer.0, "unlocked": false }))),
                None => actions.push(Action::Run("shot.setLocked", json!({ "node": id, "locked": !locked }))),
            }
        }
        let slot = slot.split_left_right_at_x(slot.right() - 20.0).1;
        let hint = if hidden {
            "Hidden: not in what the shot shows, nor in a render. Click to show it again"
        } else {
            "Hide it: a layer, or all that a peg carries or a group has in it. What is hidden is not rendered either"
        };
        let (icon, tint) = if hidden { ("eye-off", theme.accent) } else { ("eye", theme.text_muted) };
        let mut at = ui.new_child(egui::UiBuilder::new().id_salt(("eye", id)).max_rect(slot).layout(Layout::left_to_right(Align::Center)));
        let eye = at.add(egui::Button::image(icons::image(icon, 12.0, tint)).frame(false)).on_hover_text(hint);
        #[cfg(test)]
        self.eyes.push((id, eye.rect));
        if eye.clicked() {
            actions.push(Action::Run("shot.setHidden", json!({ "node": id, "hidden": !hidden })));
        }
        left
    }

    /// The switches at the right end of a sound row: `M` mutes the track, which is the
    /// project's and the storyboard timeline's too; the lock keeps its clips where they are on
    /// the storyboard; the eye puts the waveform away here. What is left of the row, for its
    /// name, is given back.
    fn sound_switches(&mut self, ui: &mut egui::Ui, rect: Rect, at: usize, actions: &mut Vec<Action>) -> Rect {
        let (left, slot) = rect.split_left_right_at_x(rect.right() - 62.0);
        let (theme, sound) = (Theme::of(ui.ctx()), &self.sound[at]);
        let (storyboard, track, mute) = (sound.storyboard, sound.track, sound.mute);
        let mut row = ui.new_child(egui::UiBuilder::new().id_salt(("sound switches", track)).max_rect(slot).layout(Layout::left_to_right(Align::Center)));
        row.spacing_mut().item_spacing.x = 4.0;
        let hint = if mute { "Muted: not heard. Click to hear it again" } else { "Mute the track" };
        let muted = row.add(egui::Button::selectable(mute, egui::RichText::new("M").size(10.0))).on_hover_text(hint);
        if muted.clicked() {
            actions.push(Action::Run("sound.setTrack", json!({ "storyboard": storyboard, "track": track, "mute": !mute })));
        }
        let switch = |row: &mut egui::Ui, on: bool, icons: [&str; 2], hint: &str| {
            let tint = if on { theme.accent } else { theme.text_muted };
            row.add(egui::Button::image(icons::image(icons[usize::from(on)], 12.0, tint)).frame(false)).on_hover_text(hint)
        };
        let locked = self.sound_locked.contains(&track);
        let hint =
            if locked { "Locked: its clips are not moved or trimmed on the storyboard. Click to unlock" } else { "Lock the track's clips where they are" };
        let lock = switch(&mut row, locked, ["lock-open", "lock"], hint);
        if lock.clicked() && !self.sound_locked.remove(&track) {
            self.sound_locked.insert(track);
        }
        let hidden = self.sound_hidden.contains(&track);
        let hint = if hidden { "Its waveform is put away. Click to show it" } else { "Put the waveform away. The sound is still heard" };
        let eye = switch(&mut row, hidden, ["eye", "eye-off"], hint);
        if eye.clicked() && !self.sound_hidden.remove(&track) {
            self.sound_hidden.insert(track);
        }
        #[cfg(test)]
        self.sound_switches.extend([(track, "mute", muted.rect), (track, "lock", lock.rect), (track, "hide", eye.rect)]);
        left
    }

    /// A group's row: an arrow that folds it and opens it, and its name. A double click on the
    /// name renames it, and a right-click has the rest.
    fn group_name(&mut self, ui: &mut egui::Ui, shot: &Shot, rect: Rect, depth: usize, id: GroupId, actions: &mut Vec<Action>) {
        let (Some(group), this) = (shot.group(id), Row::Group(id)) else { return };
        let rect = self.eye(ui, shot, rect, id.0, actions);
        let mut inside = rect.shrink2(vec2(4.0, 1.0));
        inside.min.x += INDENT * depth as f32;
        let mut row = ui.new_child(egui::UiBuilder::new().max_rect(inside).layout(Layout::left_to_right(Align::Center)));
        // A name that is longer than its room is cut off there, clear of the switch beside it.
        row.set_clip_rect(inside.intersect(ui.clip_rect()));
        let fold = json!({ "group": id.0, "rowsFolded": !group.rows_folded });
        let hint = if group.rows_folded { "Open the group: show the rows of what is in it" } else { "Fold the group: put the rows of what is in it away" };
        let arrow = row.add(egui::Button::new(if group.rows_folded { "›" } else { "⌄" }).frame(false).small()).on_hover_text(hint);
        #[cfg(test)]
        self.group_names.push((id, arrow.rect));
        if arrow.clicked() {
            actions.push(Action::Run("group.set", fold.clone()));
        }
        if let Some((_, text)) = self.renaming.as_mut().filter(|(renamed, _)| *renamed == this) {
            let edit = row.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
            if edit.lost_focus() {
                let keep = !row.input(|i| i.key_pressed(egui::Key::Escape)) && *text != group.name;
                if let Some((_, text)) = self.renaming.take().filter(|_| keep) {
                    actions.push(Action::Run("group.rename", json!({ "group": id.0, "name": text })));
                }
            } else {
                edit.request_focus();
            }
            return;
        }
        let name = egui::RichText::new(&group.name).strong();
        let label = row.add(egui::Label::new(name).sense(Sense::click())).on_hover_text(if group.board {
            "A group, and the storyboard's: all that is in it is boards"
        } else {
            "A group: some of the shot's nodes handled as one"
        });
        if label.double_clicked() {
            self.renaming = Some((this, group.name.clone()));
        }
        label.context_menu(|ui| {
            if ui.button(if group.rows_folded { "Unfold" } else { "Fold" }).clicked() {
                actions.push(Action::Run("group.set", fold));
                ui.close();
            }
            if ui.button("Rename").clicked() {
                self.renaming = Some((this, group.name.clone()));
                ui.close();
            }
            let title = if group.board { "No Longer the Storyboard's" } else { "Make It the Storyboard's" };
            if ui.button(title).on_hover_text("All that is in the storyboard's group is boards, whatever a layer says of itself").clicked() {
                actions.push(Action::Run("group.set", json!({ "group": id.0, "board": !group.board })));
                ui.close();
            }
            if ui.button("Ungroup").on_hover_text("What is in it stays in the shot").clicked() {
                actions.push(Action::Run("group.delete", json!({ "group": id.0 })));
                ui.close();
            }
            // The app makes it the deleting of each thing in it, as one step.
            if ui.button("Delete with All in It").on_hover_text("The group and every layer, peg and node in it. Undo brings them back").clicked() {
                actions.push(Action::Run("group.delete", json!({ "group": id.0, "contents": true })));
                ui.close();
            }
            if ui.button("Save to Library…").on_hover_text("As a rig, to put into other shots").clicked() {
                actions.push(Action::Save { parts: vec![Part::Group(id)], at: None });
                ui.close();
            }
        });
    }

    /// The name of an effect node on its row. A click picks the node, so that Properties
    /// shows it; a right-click keys its numbers on the frame, switches it, or deletes it.
    fn effect_name(
        &mut self,
        ui: &mut egui::Ui,
        shot: &Shot,
        rect: Rect,
        depth: usize,
        id: EffectId,
        picked: &BTreeSet<u64>,
        frame: u32,
        actions: &mut Vec<Action>,
    ) {
        let Some(effect) = shot.effect(id) else { return };
        let muted = Theme::of(ui.ctx()).text_muted;
        let mut inside = rect.shrink2(vec2(4.0, 1.0));
        inside.min.x += INDENT * depth as f32;
        let mut row = ui.new_child(egui::UiBuilder::new().max_rect(inside).layout(Layout::left_to_right(Align::Center)));
        // A name that is longer than its room is cut off there, clear of the switch beside it.
        row.set_clip_rect(inside.intersect(ui.clip_rect()));
        row.add_space(12.0);
        row.add(icons::image("sparkles", 12.0, muted));
        row.add_space(3.0);
        let kind = effect.described().map_or(effect.kind.as_str(), |kind| kind.title);
        let written = if effect.enabled { egui::RichText::new(&effect.name) } else { egui::RichText::new(&effect.name).color(muted) };
        let hint = format!("{kind}: an effect node, with its keyframes. Its numbers are in Properties when it is picked");
        let label = row.selectable_label(picked.contains(&id.0), written).on_hover_text(hint);
        #[cfg(test)]
        self.effect_names.push((id, label.rect));
        if label.clicked() {
            actions.push(Action::Pick(vec![id.0]));
        }
        label.context_menu(|ui| {
            let keyed = effect.key_frames(None).contains(&frame);
            let (title, command) = if keyed { ("Remove Keyframe", "effect.removeKey") } else { ("Add Keyframe", "effect.setKey") };
            if ui.button(title).on_hover_text("Of its numbers, on the frame that is showing").clicked() {
                actions.push(Action::Run(command, json!({ "effect": id.0, "frame": frame })));
                ui.close();
            }
            if ui.button(if effect.enabled { "Switch Off" } else { "Switch On" }).clicked() {
                actions.push(Action::Run("effect.setEnabled", json!({ "effect": id.0, "enabled": !effect.enabled })));
                ui.close();
            }
            if ui.button("Delete").on_hover_text("The wire it was on is joined again").clicked() {
                actions.push(Action::Run("effect.delete", json!({ "effect": id.0 })));
                ui.close();
            }
        });
    }

    /// The name of a layer's bones on their row. A click selects the layer they bend, a double
    /// click renames them, and a right-click keys their pose on the frame, or deletes them.
    /// The `+` at the right end of a row's name that opens the node's values, each on a row
    /// of its own under it, and the `−` that puts them away. The room left for the name.
    fn opener(&mut self, ui: &mut egui::Ui, rect: Rect, node: u64) -> Rect {
        let open = self.opened.contains(&node);
        let at = Rect::from_min_max(pos2(rect.right() - 16.0, rect.top()), rect.max);
        let hint = if open { "Put the rows of its values away" } else { "Show a row for each of its values, with its own keyframes" };
        let button = ui.put(at, egui::Button::new(if open { "−" } else { "+" }).frame(false).small()).on_hover_text(hint);
        #[cfg(test)]
        self.openers.push((node, button.rect));
        if button.clicked() && !self.opened.remove(&node) {
            self.opened.insert(node);
        }
        Rect::from_min_max(rect.min, pos2(at.left(), rect.bottom()))
    }

    /// The name of a row of one value: under its node's, fainter. A click on it selects the
    /// node, or picks the effect node.
    fn value_name(&mut self, ui: &mut egui::Ui, shot: &Shot, rect: Rect, depth: usize, of: Keyed, frame: u32, actions: &mut Vec<Action>) {
        let Some(name) = of.value_label(shot) else { return };
        let mut inside = rect.shrink2(vec2(4.0, 1.0));
        inside.min.x += INDENT * depth as f32 + 15.0;
        let mut row = ui.new_child(egui::UiBuilder::new().max_rect(inside).layout(Layout::left_to_right(Align::Center)));
        row.set_clip_rect(inside.intersect(ui.clip_rect()));
        let keyed = of.frames(shot).contains(&frame);
        let written = egui::RichText::new(name).size(11.0).color(Theme::of(ui.ctx()).text_muted);
        let label = row.add(egui::Button::selectable(false, written).truncate());
        #[cfg(test)]
        self.value_names.push((of, label.rect));
        if label.clicked() {
            match of {
                Keyed::Effect(effect, _) => actions.push(Action::Pick(vec![effect.0])),
                _ => actions.push(Action::Select { node: Row::Value(of).node(), frame }),
            }
        }
        // Its own keyframe on the frame that is showing, added or taken away, of this value
        // alone: a peg's channel or an effect's parameter. A value of a pose is keyed with
        // the pose, and taken away alone.
        label.context_menu(|ui| {
            let (title, command) = match (keyed, of) {
                (true, Keyed::Peg(..)) => ("Remove Keyframe", "peg.removeKey"),
                (true, Keyed::Bones(..)) => ("Remove Keyframe", "deformer.removeKey"),
                (true, Keyed::Effect(..)) => ("Remove Keyframe", "effect.removeKey"),
                (true, Keyed::Lens(_)) => ("Remove Keyframe", "camera.removeKey"),
                (false, Keyed::Lens(_)) => ("Add Keyframe", "camera.setKey"),
                (false, Keyed::Peg(..)) => ("Add Keyframe", "peg.setKey"),
                (false, Keyed::Bones(..)) => ("Add Keyframe of the Pose", "deformer.setKey"),
                (false, Keyed::Effect(..)) => ("Add Keyframe", "effect.setKey"),
            };
            if ui.button(title).clicked() {
                let params = match (keyed, of) {
                    (false, Keyed::Bones(bones, ..)) => json!({ "deformer": bones.0, "frame": frame }),
                    _ => of.params(json!({ "frame": frame })),
                };
                actions.push(Action::Run(command, params));
                ui.close();
            }
        });
    }

    fn bones_name(&mut self, ui: &mut egui::Ui, shot: &Shot, rect: Rect, depth: usize, of: (DeformerId, LayerId), frame: u32, actions: &mut Vec<Action>) {
        let (Some(bones), this) = (shot.deformer(of.0), Row::Bones(of.0, of.1)) else { return };
        let mut inside = rect.shrink2(vec2(4.0, 1.0));
        inside.min.x += INDENT * depth as f32;
        let mut row = ui.new_child(egui::UiBuilder::new().max_rect(inside).layout(Layout::left_to_right(Align::Center)));
        // A name that is longer than its room is cut off there, clear of the switch beside it.
        row.set_clip_rect(inside.intersect(ui.clip_rect()));
        row.add(icons::image("bones", 12.0, Theme::of(ui.ctx()).text_muted));
        row.add_space(3.0);
        if let Some((_, text)) = self.renaming.as_mut().filter(|(renamed, _)| *renamed == this) {
            let edit = row.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
            if edit.lost_focus() {
                // Enter keeps the new name; Escape, or the same name, changes nothing.
                let keep = !row.input(|i| i.key_pressed(egui::Key::Escape)) && *text != bones.name;
                if let Some((_, text)) = self.renaming.take().filter(|_| keep) {
                    actions.push(Action::Run("deformer.rename", json!({ "deformer": bones.id.0, "name": text })));
                }
            } else {
                edit.request_focus();
            }
            return;
        }
        let bent = shot.layers.iter().filter(|layer| layer.deformer == Some(bones.id)).map(|layer| layer.name.as_str()).collect::<Vec<_>>().join(", ");
        let label = row.selectable_label(false, &bones.name).on_hover_text(format!("The bones that bend {bent}. Lay and pose them with the Bones tool"));
        #[cfg(test)]
        self.bone_names.push((bones.id, label.rect));
        if label.clicked() {
            actions.push(Action::Select { node: this.node(), frame });
        }
        if label.double_clicked() {
            self.renaming = Some((this, bones.name.clone()));
        }
        label.context_menu(|ui| {
            if ui.button("Rename").clicked() {
                self.renaming = Some((this, bones.name.clone()));
                ui.close();
            }
            let keyed = bones.key_frames().contains(&frame);
            let (title, id) = if keyed { ("Remove Keyframe", "deformer.removeKey") } else { ("Add Keyframe", "deformer.setKey") };
            if ui.button(title).on_hover_text("Of the pose, on the frame that is showing").clicked() {
                actions.push(Action::Run(id, json!({ "deformer": bones.id.0, "frame": frame })));
                ui.close();
            }
            if ui.button("Delete").on_hover_text("The layers it bends are as they were drawn again").clicked() {
                actions.push(Action::Run("deformer.delete", json!({ "deformer": bones.id.0 })));
                ui.close();
            }
        });
    }

    /// The marks on a folded row, a peg's or a group's, for what is on the rows it puts away:
    /// a small dark red bar at the foot of each frame that has a keyframe there. They only show it:
    /// they are not pressed, dragged or joined by lines.
    fn put_away_marks(&mut self, painter: &egui::Painter, id: u64, frames: BTreeSet<u32>, lane: Rect) {
        let cw = self.frame_width();
        let colour = Theme::of(painter.ctx()).colour(Role::PutAway);
        for frame in frames.into_iter().filter(|frame| *frame < self.reach) {
            let middle = lane.left() + cw * (frame as f32 + 0.5);
            painter.rect_filled(Rect::from_min_max(pos2(middle - 3.0, lane.bottom() - 5.0), pos2(middle + 3.0, lane.bottom() - 2.0)), 1.0, colour);
            #[cfg(test)]
            self.put_away_marks.push((id, frame));
        }
        #[cfg(not(test))]
        let _ = id;
    }

    /// The keyframes of a row, a peg's or those of a pose: a diamond where the value moves on
    /// at a steady rate, a circle where it slows in or out, a square where it is held.
    /// Clicking one goes to its frame; dragging one moves the keys to another frame. `frames`
    /// are the frames that have keys, `eases` says how the keys of one ease, in and out, and
    /// `moves` whether anything moves on from a frame towards a later key.
    fn keys(&mut self, ui: &egui::Ui, painter: &egui::Painter, cells: &egui::Response, shot: &Shot, of: Keyed, lane: Rect, actions: &mut Vec<Action>) {
        let cw = self.frame_width();
        let theme = Theme::of(ui.ctx());
        // What its commands call it by, and what a click on a key selects.
        let id = of.id();
        let node = match of {
            Keyed::Peg(peg, _) => Some(Node::Peg(peg)),
            Keyed::Bones(_, layer, _) => Some(Node::Layer(layer)),
            Keyed::Effect(..) | Keyed::Lens(_) => None,
        };
        let eases = |frame| of.eases(shot, frame);
        let moves = |frame| of.moves_after(shot, frame);
        let last = shot.duration.saturating_sub(1);
        // The command that moves one of the keyframes that are picked by `by` frames.
        let moved = |(of, frame): (Keyed, u32), by: i64| {
            let to = (i64::from(frame) + by).clamp(0, i64::from(last)) as u32;
            json!({ "id": format!("{}.moveKey", of.kind()), "params": of.params(json!({ "from": frame, "to": to })) })
        };
        let cell = |frame: u32| Rect::from_min_size(pos2(lane.left() + cw * frame as f32, lane.top()), vec2(cw, ROW_HEIGHT));
        let frames: Vec<u32> = of.frames(shot).into_iter().filter(|frame| *frame < self.reach).collect();
        // Where each key is shown, with whether anything moves on from it, and its frame.
        let mut placed: Vec<(u32, bool, u32)> = Vec::new();
        for frame in frames {
            let dragged = self.key_drag.filter(|drag| drag.of == of && drag.from == frame);
            let mark = cell(frame);
            self.shown_keys.push((of, frame, mark));
            let is_picked = self.picked_keys.contains(&(of, frame));
            #[cfg(test)]
            match of {
                Keyed::Peg(peg, None) => self.key_marks.push((peg, frame, mark)),
                Keyed::Bones(bones, _, None) => self.bone_key_marks.push((bones, frame, mark)),
                Keyed::Effect(effect, None) => self.effect_key_marks.push((effect, frame, mark)),
                _ => self.value_key_marks.push((of, frame, mark)),
            }
            let grip = ui.interact(mark, cells.id.with(("key", id, format!("{of:?}"), frame)), Sense::click_and_drag()).on_hover_cursor(CursorIcon::Grab);
            // With Shift held a click picks the keyframe beside the others, or lets it go; a
            // plain click picks it alone, and goes to it, unless the playhead is held.
            if grip.clicked() && ui.input(|i| i.modifiers.shift) {
                match self.picked_keys.iter().position(|key| *key == (of, frame)) {
                    Some(at) => drop(self.picked_keys.remove(at)),
                    None => self.picked_keys.push((of, frame)),
                }
            } else if grip.clicked() {
                self.picked_keys = vec![(of, frame)];
                actions.push(Action::Select { node, frame: if self.hold_playhead { self.now } else { frame } });
                // An effect is picked, as a click on its box in the node view picks it, so
                // that Properties shows it.
                if let Keyed::Effect(effect, _) = of {
                    actions.push(Action::Pick(vec![effect.0]));
                }
            }
            if grip.drag_started() {
                self.key_drag = Some(KeyDrag { of, from: frame, to: frame });
                // One that is not picked is dragged alone.
                if !is_picked {
                    self.picked_keys = vec![(of, frame)];
                }
            }
            if let (Some(drag), Some(pos)) = (self.key_drag.as_mut().filter(|_| dragged.is_some()), grip.interact_pointer_pos()) {
                drag.to = frame_at(pos.x - lane.left(), shot, cw);
            }
            if let (Some(drag), true) = (self.key_drag.filter(|_| dragged.is_some()), grip.drag_stopped()) {
                self.key_drag = None;
                let by = i64::from(drag.to) - i64::from(drag.from);
                if by != 0 && self.picked_keys.len() > 1 {
                    // All that are picked go by as many frames, as one step: those ahead
                    // first, so that none lands on one that has yet to move.
                    let mut keys = self.picked_keys.clone();
                    keys.sort_by_key(|(_, frame)| if by > 0 { std::cmp::Reverse(i64::from(*frame)) } else { std::cmp::Reverse(-i64::from(*frame)) });
                    let commands: Vec<Value> = keys.iter().map(|key| moved(*key, by)).collect();
                    actions.push(Action::Run("edit.batch", json!({ "commands": commands })));
                    self.key_landing = Some((self.picked_keys.clone(), by));
                    for (_, frame) in &mut self.picked_keys {
                        *frame = (i64::from(*frame) + by).clamp(0, i64::from(last)) as u32;
                    }
                } else if by != 0 {
                    let move_key = match of {
                        Keyed::Peg(..) => "peg.moveKey",
                        Keyed::Bones(..) => "deformer.moveKey",
                        Keyed::Effect(..) => "effect.moveKey",
                        Keyed::Lens(_) => "camera.moveKey",
                    };
                    actions.push(Action::Run(move_key, of.params(json!({ "from": drag.from, "to": drag.to }))));
                    self.picked_keys = vec![(of, drag.to)];
                    self.key_landing = Some((vec![(of, drag.from)], by));
                }
            }
            let (into, out) = eases(frame).unwrap_or_default();
            // A key is eased, held and removed from its own menu, a peg's as a pose's.
            let (set_ease, remove_key) = match of {
                Keyed::Peg(..) => ("peg.setEase", "peg.removeKey"),
                Keyed::Bones(..) => ("deformer.setEase", "deformer.removeKey"),
                Keyed::Effect(..) => ("effect.setEase", "effect.removeKey"),
                Keyed::Lens(_) => ("camera.setEase", "camera.removeKey"),
            };
            grip.context_menu(|ui| {
                for (title, to_into, to_out) in EASES {
                    if ui.radio(to_out == out && to_into.is_none_or(|to_into| to_into == into), title).clicked() {
                        let mut params = of.params(json!({ "frame": frame, "out": to_out }));
                        if let Some(to_into) = to_into {
                            params["into"] = json!(to_into);
                        }
                        actions.push(Action::Run(set_ease, params));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Remove Keyframe").clicked() {
                    actions.push(Action::Run(remove_key, of.params(json!({ "frame": frame }))));
                    ui.close();
                }
            });
            // While it is dragged, the key shows on the frame it would land on; and so do
            // the others that are picked with the one that is dragged.
            let along = self
                .key_drag
                .filter(|drag| is_picked && self.picked_keys.contains(&(drag.of, drag.from)))
                .map(|drag| i64::from(drag.to) - i64::from(drag.from));
            // Or, let go of just now, where it went.
            let landed = self.key_landing.as_ref().filter(|(moved, _)| moved.contains(&(of, frame))).map(|(_, by)| *by);
            let shown = match (self.key_drag.filter(|_| dragged.is_some()), along.or(landed)) {
                (Some(drag), _) => drag.to,
                (None, Some(by)) => (i64::from(frame) + by).clamp(0, i64::from(last)) as u32,
                (None, None) => frame,
            };
            placed.push((shown, moves(frame), frame));
            #[cfg(test)]
            self.keys_drawn.push((of, frame, shown));
            let (center, r) = (cell(shown).center(), 4.5);
            let color = if dragged.is_some() || grip.hovered() || is_picked { theme.accent } else { theme.text };
            match (into, out) {
                (_, Interp::Hold) => painter.rect_filled(Rect::from_center_size(center, vec2(2.0 * r - 1.0, 2.0 * r - 1.0)), 0.0, color),
                (Interp::Ease | Interp::Curve(_), _) | (_, Interp::Ease | Interp::Curve(_)) => painter.circle_filled(center, r - 0.5, color),
                _ => {
                    let corners = vec![center - vec2(0.0, r), center + vec2(r, 0.0), center + vec2(0.0, r), center - vec2(r, 0.0)];
                    painter.add(egui::Shape::convex_polygon(corners, color, Stroke::NONE))
                }
            };
        }
        // The keys a cycle repeats, after its last key: darker, and not picked nor dragged.
        let repeated = of.repeated(shot, self.reach);
        let shade = theme.repeated_key();
        for frame in &repeated {
            let (center, r) = (cell(*frame).center(), 4.0);
            let corners = vec![center - vec2(0.0, r), center + vec2(r, 0.0), center + vec2(0.0, r), center - vec2(r, 0.0)];
            painter.add(egui::Shape::convex_polygon(corners, shade, Stroke::NONE));
        }
        #[cfg(test)]
        self.repeated_marks.extend(repeated.iter().map(|frame| (of, *frame)));
        // A line from a keyframe to the next where anything moves between them. Where all is
        // held until the next there is none, so a pose that stays is told from one that moves.
        // The line is between the keys as they are shown, so it follows one that is dragged.
        // From the last key of a cycle, the line is darker, as far as the cycle repeats.
        placed.sort_by_key(|(shown, ..)| *shown);
        let still = self.key_drag.is_none() && self.key_landing.is_none();
        for (at, &(from, _, key)) in placed.iter().enumerate() {
            if !still || !of.ends_cycle(shot, key) {
                continue;
            }
            let next = placed.get(at + 1).map_or(self.reach, |(to, ..)| *to);
            if let Some(last) = repeated.range(from + 1..next).next_back() {
                let (start, end) = (cell(from).center(), cell(*last).center());
                painter.hline(start.x + 5.0..=end.x - 5.0, start.y, Stroke::new(1.5, shade));
            }
        }
        for pair in placed.windows(2) {
            let ((from, moving, key), (to, _, next)) = (pair[0], pair[1]);
            if moving && to > from && !(still && of.ends_cycle(shot, key)) {
                let (start, end) = (cell(from).center(), cell(to).center());
                painter.hline(start.x + 5.0..=end.x - 5.0, start.y, Stroke::new(1.5, theme.text_muted));
                #[cfg(test)]
                self.tweens.push((id, from, to));
                // The timing chart's lines, while no key is on its way elsewhere.
                if still {
                    let row = cells.id.with(("chart", format!("{of:?}")));
                    let spacings = of.spacings(shot, key, next);
                    let made =
                        crate::breakdown::way(ui, painter, self.breakdowns, &mut self.chart_drag, row, key, (start.x, end.x), start.y, ROW_HEIGHT, &spacings);
                    #[cfg(test)]
                    self.breakdown_lines.extend(made.lines.iter().map(|line| (of, key, *line)));
                    if let Some(spacing) = made.let_go {
                        actions.push(Action::Run("edit.batch", json!({ "commands": of.charted(shot, key, &spacing) })));
                    }
                }
            }
        }
    }
}

/// The frame the first of some picked frames is on once they are dragged `by` frames: never
/// before the shot's first frame, nor so far that the last of them is past its end.
fn moved_to(picked: Frames, by: i64, shot: &Shot) -> u32 {
    let furthest = i64::from(shot.duration.saturating_sub(picked.length()));
    (i64::from(picked.first) + by).clamp(0, furthest.max(0)) as u32
}

/// The layers whose holds Hold to Here takes on to `frame`: those whose cell there is empty
/// after a drawing, of the layer in hand, or of all that are picked when it is one of them.
pub(crate) fn layers_to_hold(shot: &Shot, layer: Option<LayerId>, picked: &BTreeSet<u64>, frame: u32) -> Vec<LayerId> {
    let Some(layer) = layer else { return Vec::new() };
    let layers: Vec<LayerId> =
        if picked.contains(&layer.0) { shot.layers.iter().map(|l| l.id).filter(|l| picked.contains(&l.0)).collect() } else { vec![layer] };
    layers.into_iter().filter(|id| shot.layer(*id).is_some_and(|l| l.exposure.held_before(frame).is_some())).collect()
}

/// The last frame past the end of the shot that has something on it: a key, the lens's too, a
/// held cell, a marker. `None` where all of it is in the shot.
fn past_the_end(shot: &Shot) -> Option<u32> {
    let mut keys = put_away(shot, |_| true);
    keys.extend(shot.camera.animation.frames());
    let held = shot.layers.iter().flat_map(|layer| layer.exposure.runs().last().map(|run| run.end().saturating_sub(1)));
    let marked = shot.markers.iter().map(|marker| marker.frame);
    keys.into_iter().chain(held).chain(marked).filter(|frame| *frame >= shot.duration).max()
}

/// The frame under a horizontal position in the grid.
fn frame_at(x: f32, shot: &Shot, width: f32) -> u32 {
    ((x / width).floor().max(0.0) as u32).min(shot.duration.saturating_sub(1))
}

/// The frame a dragged playhead is caught by, with snapping: the nearest of `catches` that is no
/// further than `reach` frames from `frame`, and the earlier of two that are as near. `frame`
/// itself when there is none.
pub(crate) fn caught(frame: u32, catches: impl IntoIterator<Item = u32>, reach: u32) -> u32 {
    catches.into_iter().filter(|catch| catch.abs_diff(frame) <= reach).min_by_key(|catch| (catch.abs_diff(frame), *catch)).unwrap_or(frame)
}

/// The frame showing `elapsed` seconds after playback started on frame `from`. `None` when
/// playback has run past the last frame and doesn't loop.
pub fn playback_frame(from: u32, elapsed: f64, fps: f64, duration: u32, looping: bool) -> Option<u32> {
    let frame = u64::from(from) + (elapsed.max(0.0) * fps).floor() as u64;
    match (frame < u64::from(duration), looping) {
        (true, _) => Some(frame as u32),
        (false, true) => Some((frame % u64::from(duration.max(1))) as u32),
        (false, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dragged_playhead_is_caught_by_what_is_nearest_within_reach() {
        assert_eq!(caught(10, [4, 12, 30], 2), 12);
        assert_eq!(caught(10, [4, 13, 30], 2), 10, "nothing is near enough");
        assert_eq!(caught(10, [11, 9], 1), 9, "the earlier of two as near");
        assert_eq!(caught(10, [10, 9], 1), 10);
        assert_eq!(caught(10, [], 3), 10);
    }

    #[test]
    fn playback_advances_at_the_frame_rate() {
        assert_eq!(playback_frame(0, 0.0, 24.0, 48, false), Some(0));
        assert_eq!(playback_frame(0, 0.999, 24.0, 48, false), Some(23));
        assert_eq!(playback_frame(10, 1.0, 24.0, 48, false), Some(34));
        assert_eq!(playback_frame(0, 1.0, 24000.0 / 1001.0, 48, false), Some(23), "23.976 fps is a little slower than 24");
    }

    #[test]
    fn playback_stops_after_the_last_frame_or_loops() {
        assert_eq!(playback_frame(40, 0.3, 24.0, 48, false), Some(47));
        assert_eq!(playback_frame(40, 0.34, 24.0, 48, false), None);
        assert_eq!(playback_frame(40, 0.34, 24.0, 48, true), Some(0));
        assert_eq!(playback_frame(0, 2.5, 24.0, 48, true), Some(12));
    }

    /// The rig of a character whose face is a folded group under the character's peg: the
    /// irises cut by the eyes inside it, and the face's picture cut by the body outside it.
    /// And a shadow, glowing, under a folded peg of its own.
    #[test]
    fn effects_stay_with_the_layers_they_are_of_when_a_peg_or_group_is_folded() {
        use std::sync::Arc;

        use iskra_model::{CompositeId, Effect, Group, Peg, Project, Role, effect_kind};

        let mut project = Project::default();
        let mut id = || project.new_id();
        let (chestnut, shadow_peg, face, glow_id, cut_eyes, cut_face) = (PegId(id()), PegId(id()), GroupId(id()), id(), id(), id());
        let composite = CompositeId(id());
        let mut layers = Vec::new();
        for (name, parent) in [("iris", chestnut), ("eyes", chestnut), ("body", chestnut), ("sdw", shadow_peg)] {
            let mut layer = project.new_layer(name, Role::Animation);
            layer.parent = Some(parent);
            layers.push(layer);
        }
        let [iris, eyes, body, sdw] = [0, 1, 2, 3].map(|at| layers[at].id);
        let peg = |id, name: &str| {
            let (transform, animation) = (Default::default(), Default::default());
            Arc::new(Peg {
                id,
                name: name.to_string(),
                parent: None,
                transform,
                animation,
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            })
        };
        let effect = |id, name: &str, kind: &str, picture: Source, matte: Option<Source>| {
            let mut effect = Effect::new(EffectId(id), name, effect_kind(kind).unwrap());
            effect.inputs.insert(iskra_model::PICTURE.to_string(), picture);
            effect.inputs.extend(matte.map(|matte| (iskra_model::MATTE.to_string(), matte)));
            Arc::new(effect)
        };
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.camera.parent = None;
        shot.layers = layers.into_iter().map(Arc::new).collect();
        shot.pegs = vec![peg(chestnut, "CHESTNUT-P"), peg(shadow_peg, "sdw-P")];
        shot.composites = vec![Arc::new(iskra_model::Composite::new(composite, "Face", vec![Source::Effect(EffectId(cut_eyes)), Source::Layer(eyes)]))];
        shot.effects = vec![
            effect(cut_face, "Cutter", "cutter", Source::Composite(composite), Some(Source::Layer(body))),
            effect(cut_eyes, "Cutter 2", "cutter", Source::Layer(iris), Some(Source::Layer(eyes))),
            effect(glow_id, "Glow", "glow", Source::Layer(sdw), None),
        ];
        let members = [iris.0, eyes.0, composite.0, cut_eyes].into();
        shot.groups = vec![Arc::new(Group { id: face, name: "FACE".to_string(), members, folded: true, rows_folded: true, board: false, template: None })];
        shot.folded = [shadow_peg].into();
        let listed = |shot: &Shot| {
            let name = |row: Row| match row {
                Row::Node(Node::Peg(id)) => shot.peg(id).unwrap().name.clone(),
                Row::Node(Node::Layer(id)) => shot.layer(id).unwrap().name.clone(),
                Row::Group(id) => shot.group(id).unwrap().name.clone(),
                Row::Effect(id, _) => shot.effect(id).unwrap().name.clone(),
                other => format!("{other:?}"),
            };
            rows(shot, false, &BTreeSet::new()).into_iter().map(|(row, depth)| format!("{}{}", ">".repeat(depth), name(row))).collect::<Vec<_>>().join(" ")
        };
        // The face's cutter is under the body it cuts by; the glow is put away with its shadow.
        assert_eq!(listed(shot), "CHESTNUT-P >FACE >body >>Cutter sdw-P");
        assert_eq!(effect_layer(shot, EffectId(cut_face)), Some(body));
        // Folded, the character's peg puts away all of it: no face and no cutter left after it.
        shot.folded.insert(chestnut);
        assert_eq!(listed(shot), "CHESTNUT-P sdw-P");
        // Opened, the face shows the irises' cutter under the irises.
        shot.folded.remove(&chestnut);
        Arc::make_mut(&mut shot.groups[0]).rows_folded = false;
        assert_eq!(listed(shot), "CHESTNUT-P >FACE >iris >>Cutter 2 >eyes >body >>Cutter sdw-P");
    }
}
