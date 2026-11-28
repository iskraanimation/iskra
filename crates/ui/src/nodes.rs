//! The node view (plan §4.3): a shot's graph as boxes and wires. A layer, a peg and a composite
//! are boxes, and so are the display and the camera, which has the wire of the peg that moves it. A wire runs from the bottom of one box to the top of
//! another, and there are two kinds: a picture goes from a layer or a composite into a
//! composite or the display, and a transform goes from a peg down to what it carries. A chain
//! of bones is a box too, with a wire of a third kind to each layer it bends.
//!
//! A group (plan §4.3) is drawn as a frame round what is in it, with its name on the frame's
//! top, by which all of it is dragged. An open group floats over the nodes that are in none:
//! those and their wires are drawn first, then the frames, which all but hide what is under them,
//! then what is in the groups with its wires; folded, it is one box, what is in it is put away, and
//! the wires that went in and out of it go to the box. A group's ports (plan §8.6) are on its
//! frame: those in side by side on its top edge, those out on its bottom edge, so that a wire
//! from outside ends on the frame and runs on inside from there. A folded group's box has them
//! the same way: a wire is dragged out of it from under a Port Out, and one let go on it goes
//! into the free Port In nearest to where it was let go, or into a new one.
//!
//! Several nodes are picked together by dragging a box round them on the background, or by
//! clicking them with Shift held; what is then done to one of them, dragging it, grouping it,
//! saving it to the library, is done to all. A plain click on a box leaves that one alone
//! outlined, whatever kind it and the one before it are, and a click on the background none. The background is moved with the middle button or
//! with Space held, and by scrolling, over a box as over the background. While a box, a wire
//! or the box that picks is dragged to an edge of the view, the view moves along under it.
//!
//! A node stays where it is: one that was never put anywhere is laid out when it is first
//! seen, and is not moved again when the graph changes, unless the view is set to arrange the
//! nodes by itself, which lays all of those out anew whenever it does (its menu, *Arrange*).
//!
//! Dragging a box moves it; dragging from the port under a box to another box connects them;
//! Alt-clicking a wire takes it away; Alt-clicking a box wires all that is picked into it; and
//! the end of a wire is dragged off the box it arrives at onto another, which then has it, or
//! let go on nothing, which takes the wire away. A node wired to nothing that is let go on a
//! wire with Alt held goes onto that wire, if it can sit there (an effect or a composite on a
//! picture's, a peg or bones on a transform's), and the wire is lit while it is over it with Alt
//! down; let go without Alt, or wired already, it is only moved. An effect or a peg dragged
//! with Alt held from the start comes off the wire or the chain it is on: what went through
//! it is joined again behind it, drawn in red until it is let go, and it is then wired to
//! nothing, or on the wire it is let go on. It is the same graph the timeline shows, so what
//! is done here is seen there.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use egui::epaint::CubicBezierShape;
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use iskra_model::{ArtLayerKind, CompositeId, DeformerId, DisplayId, DriverId, EffectId, GroupId, LayerId, Node, Part, PegId, Shot, Source, effect_kinds};
use serde_json::{Value, json};

use crate::theme::{Role, Theme};
use crate::timeline::{Action, Parenting, parenting, parts_of};

/// A box of the node view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Layer(LayerId),
    Peg(PegId),
    Composite(CompositeId),
    /// A chain of bones (plan §4.5).
    Deformer(DeformerId),
    /// A group: the box of a folded one, or the name on an open one's frame.
    Group(GroupId),
    /// An effect node (plan §8).
    Effect(EffectId),
    /// A display: the shot's main one under the shot's own id, or another (plan §8.6).
    Display(DisplayId),
    /// The shot's camera (plan §4.4): the peg it is parented to is wired into it.
    Camera,
    /// A Driver node (plan §8.8): the pegs it reads are wired into its top, and it into the
    /// side of the peg it drives.
    Driver(DriverId),
}

impl Item {
    /// What the node's place in the view is kept under: its id, and the main display's the shot's.
    fn key(self, shot: &Shot) -> u64 {
        match self {
            Item::Layer(id) => id.0,
            Item::Peg(id) => id.0,
            Item::Composite(id) => id.0,
            Item::Deformer(id) => id.0,
            Item::Group(id) => id.0,
            Item::Effect(id) => id.0,
            Item::Display(id) => id.0,
            Item::Camera => shot.camera.id.0,
            Item::Driver(id) => id.0,
        }
    }

    /// As something that makes a picture, if it does.
    fn source(self) -> Option<Source> {
        match self {
            Item::Layer(id) => Some(Source::Layer(id)),
            Item::Composite(id) => Some(Source::Composite(id)),
            Item::Effect(id) => Some(Source::Effect(id)),
            Item::Peg(_) | Item::Deformer(_) | Item::Group(_) | Item::Display(_) | Item::Camera | Item::Driver(_) => None,
        }
    }

    /// As a node that can be saved to the library, which all but the display and, until groups
    /// carry them, bones are.
    fn part(self) -> Option<Part> {
        match self {
            Item::Layer(id) => Some(Part::Layer(id)),
            Item::Peg(id) => Some(Part::Peg(id)),
            Item::Composite(id) => Some(Part::Composite(id)),
            Item::Group(id) => Some(Part::Group(id)),
            Item::Deformer(_) | Item::Effect(_) | Item::Display(_) | Item::Camera | Item::Driver(_) => None,
        }
    }

    /// The library template the node is a copy from, if it is one.
    fn template(self, shot: &Shot) -> Option<&str> {
        match self {
            Item::Layer(id) => shot.layer(id)?.template.as_deref(),
            Item::Peg(id) => shot.peg(id)?.template.as_deref(),
            Item::Composite(id) => shot.composite(id)?.template.as_deref(),
            Item::Group(id) => shot.group(id)?.template.as_deref(),
            Item::Effect(id) => shot.effect(id)?.template.as_deref(),
            Item::Driver(id) => shot.driver(id)?.template.as_deref(),
            Item::Deformer(_) | Item::Display(_) | Item::Camera => None,
        }
    }

    fn of(source: Source) -> Item {
        match source {
            Source::Layer(id) | Source::Art(id, _) => Item::Layer(id),
            Source::Composite(id) => Item::Composite(id),
            Source::Effect(id) => Item::Effect(id),
        }
    }

    fn name(self, shot: &Shot) -> &str {
        match self {
            Item::Layer(id) => shot.layer(id).map_or("?", |layer| &layer.name),
            Item::Peg(id) => shot.peg(id).map_or("?", |peg| &peg.name),
            Item::Composite(id) => shot.composite(id).map_or("?", |composite| &composite.name),
            Item::Deformer(id) => shot.deformer(id).map_or("?", |deformer| &deformer.name),
            Item::Group(id) => shot.group(id).map_or("?", |group| &group.name),
            Item::Effect(id) => shot.effect(id).map_or("?", |effect| &effect.name),
            Item::Display(id) => shot.display_name_of(id).unwrap_or("?"),
            Item::Camera => "Camera",
            Item::Driver(id) => shot.driver(id).map_or("?", |driver| &driver.name),
        }
    }

    /// The colour of the kind of node, the same in both themes: as the user set it, or its own.
    fn color(self, theme: &Theme) -> Color32 {
        theme.colour(match self {
            Item::Layer(_) => Role::NodeLayer,
            Item::Peg(_) => Role::NodePeg,
            Item::Composite(_) => Role::NodeComposite,
            Item::Deformer(_) => Role::NodeDeformer,
            Item::Group(_) => Role::NodeGroup,
            Item::Effect(_) => Role::NodeEffect,
            Item::Display(_) => Role::NodeDisplay,
            Item::Camera => Role::NodeCamera,
            // Of the Move category, as a peg is (plan §8.8).
            Item::Driver(_) => Role::NodePeg,
        })
    }
}

/// The colour of a node's kind as the shot has it: a Pass (plan §7.14), which is kept among
/// the displays and is not one to the eye, is not a display's grey.
fn kind_colour(shot: &Shot, item: Item, theme: &Theme) -> Color32 {
    match item {
        Item::Display(id) if shot.passes().any(|pass| pass.id == id) => theme.colour(Role::Pass),
        _ => item.color(theme),
    }
}

/// The colour of a node as it is drawn: the one the user gave it, or else its kind's.
fn node_colour(shot: &Shot, item: Item, theme: &Theme) -> Color32 {
    shot.colours.get(&item.key(shot)).map_or(kind_colour(shot, item, theme), |[r, g, b]| Color32::from_rgb(*r, *g, *b))
}

/// The size of a box, and how far apart the layout puts them, in the graph's own units.
const SIZE: Vec2 = vec2(132.0, 30.0);
const ACROSS: f32 = 152.0;
const DOWN: f32 = 96.0;
/// The size of a port on an open group's frame, and how far it laps over the frame's edge.
const TAB: Vec2 = vec2(72.0, 18.0);
const LAP: f32 = 4.0;
/// How much room an open group's frame leaves round what is in it: at its sides and under
/// it, and above, where its name is.
const ROOM: Vec2 = vec2(30.0, 34.0);
const ROOM_ABOVE: f32 = 52.0;
/// How near a wire a click has to be to mean it, in points.
const WIRE_REACH: f32 = 6.0;
/// How far from the box it arrives at the end of a wire has to be dragged, in points, for
/// letting go on nothing to take the wire away: nearer, it was hardly moved, and stays.
const CLEAR: f32 = 12.0;

/// A wire, from the bottom of one box to a place on the top of another.
struct Wire {
    from: Item,
    to: Item,
    /// Where on `to` it arrives, in the graph's units.
    end: Pos2,
    kind: Kind,
    /// How far along the bottom of `from` it leaves, from 0 at the left to 1 at the right,
    /// where that is not the middle: from a port of a folded group.
    leaves: Option<f32>,
    /// A wire that is not in the shot yet: the one that joins what a node went through to
    /// where it went, while the node is dragged off its wire.
    fresh: bool,
    /// Of a wire drawn to or from a folded group's box, the nodes it joins in the shot.
    was: Option<(Item, Item)>,
    /// The art layer of a layer it leaves from the output of, where it is not the flat one.
    art: Option<ArtLayerKind>,
}

impl Wire {
    /// The nodes the wire joins in the shot, whatever box it is drawn to or from.
    fn ends(&self) -> (Item, Item) {
        self.was.unwrap_or((self.from, self.to))
    }

    /// What it carries the picture of: the node it leaves, or the art layer of it.
    fn source(&self) -> Option<Source> {
        source_of(self.ends().0, self.art)
    }

    /// The box and the output it leaves by.
    fn out(&self) -> (Item, Option<ArtLayerKind>) {
        (self.from, self.art)
    }

    /// The node it leaves in the shot and the output it leaves by: of two wires out of one
    /// folded group's box, each its own.
    fn leaves_from(&self) -> (Item, Option<ArtLayerKind>) {
        (self.ends().0, self.art)
    }
}

/// The outputs of a layer's box for its art layers, beside its flat one in the middle: each
/// with how far along the bottom of the box it is, its name and what it gives.
const ART_PORTS: [(ArtLayerKind, f32, &str); 3] = [
    (ArtLayerKind::Overart, 0.12, "Overart: a wire from here gives the overart of the drawings alone"),
    (ArtLayerKind::LineArt, 0.24, "Line Art: a wire from here gives the line art of the drawings alone"),
    (ArtLayerKind::ColourArt, 0.36, "Colour Art: a wire from here gives the colour art of the drawings alone"),
];

/// How far along the bottom of a layer's box the output of an art layer is.
fn art_port(kind: ArtLayerKind) -> f32 {
    ART_PORTS.iter().find(|(of, ..)| *of == kind).map_or(0.5, |(_, part, _)| *part)
}

/// What a node gives by one of its outputs: its picture, or for a layer and `art` that art
/// layer of it alone.
fn source_of(item: Item, art: Option<ArtLayerKind>) -> Option<Source> {
    match (item, art) {
        (Item::Layer(id), Some(kind)) => Some(Source::Art(id, kind)),
        _ => item.source(),
    }
}

/// The art layer a wire from `source` leaves by the output of.
fn art_of(source: Source) -> Option<ArtLayerKind> {
    match source {
        Source::Art(_, kind) => Some(kind),
        _ => None,
    }
}

/// What a wire carries: a picture into a composite or the display, a peg's transform down to
/// what it carries, or a chain of bones' bend to a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Picture,
    Transform,
    Bend,
    /// A peg a Driver reads, or the Driver into the peg it drives: drawn dashed, as it
    /// carries nothing.
    Read,
}

enum Drag {
    /// A box, with where its centre is now.
    Node(Item, Pos2),
    /// A new wire out of a box, by one of its outputs, to the pointer.
    Wire(Item, Option<ArtLayerKind>),
    /// The end of the wire from a node's output, taken along the ports of the composite it
    /// arrives at, to another place among them.
    End((Item, Option<ArtLayerKind>), CompositeId),
    /// The end of the wire from the first box's output to the second, taken off the second to
    /// be let go on another.
    Rewire((Item, Option<ArtLayerKind>), Item),
}

pub struct NodeView {
    /// The boards that are locked as reference where the view is shown (plan §7.2c): not
    /// changed from their menus. The app says which.
    pub locked: BTreeSet<LayerId>,
    /// Where the graph's origin is, in points from the centre of the panel, and points per unit
    /// (see [`NodeView::ZOOM`]).
    pan: Vec2,
    zoom: f32,
    drag: Option<Drag>,
    /// The other boxes that are picked with the one that is dragged, with where their centres
    /// are now: they go along.
    along: Vec<(Item, Pos2)>,
    /// The node that is dragged comes off the wire it is on: Alt was held when the drag began.
    lifting: bool,
    /// The box that is being dragged out on the background to pick nodes: where it was
    /// pressed, and where the pointer is.
    marquee: Option<(Pos2, Pos2)>,
    /// The view arranges the nodes by itself: one that was never put anywhere is where the
    /// layout has it now, and moves when the graph changes. Off, it stays where it was first
    /// laid out.
    pub arrange: bool,
    /// The view is to be panned and zoomed so that every node is seen: asked for by *Reset
    /// View*, and done once the places are known.
    pub fit: bool,
    /// A node the middle of the view is to go to: asked for by *Show Selected*, and done once
    /// the places are known.
    pub centre: Option<Item>,
    /// The whole graph is shown small in a corner, with the part of it that is seen.
    pub map: bool,
    /// Where the mini map was drawn.
    #[cfg(test)]
    pub map_drawn: Option<Rect>,
    /// Where the nodes that were never put anywhere were first laid out, in the shot they
    /// are of: there they stay while the view does not arrange them itself.
    kept: (Option<iskra_model::ShotId>, HashMap<Item, Pos2>),
    /// The groups in the order they were last taken hold of, the last on top of those beside
    /// it: for as long as the window is open, not kept with the shot.
    stack: Vec<GroupId>,
    /// Where on the background its menu was opened: a node made from the menu is put there.
    menu_at: Option<Pos2>,
    /// Where the background was last clicked, in the graph, of which shot, and the open group
    /// there: nodes pasted next go there, once. The app lets it go when nodes are copied.
    pub paste_at: Option<(iskra_model::ShotId, [f64; 2], Option<u64>)>,
    /// There are nodes copied, for *Paste* on the background's menu. The app says.
    pub copied: bool,
    /// How far the view has moved under that box since it was pressed: where it was pressed
    /// has moved as far.
    marquee_shift: Vec2,
    /// What is dragged has been clear of the view's edges since it was pressed: only then
    /// does it move the view at an edge, so that a drag begun beside an edge moves nothing.
    edge_armed: bool,
    /// The composite or the chain of bones whose name is being edited, with the text so far.
    renaming: Option<(Item, String)>,
    /// The search that adds a node (plan §8.3), while it is open: what was typed, where in
    /// the window it was opened, and which of what it found Enter would add.
    search: Option<(String, Pos2, usize)>,
    /// Where the view was last drawn, and where the graph's origin was in it.
    drawn: Option<(Rect, Pos2)>,
    /// Where each box was drawn, the topmost last.
    pub boxes: Vec<(Item, Rect)>,
    /// Where each wire was drawn: what it leaves and arrives at, and where.
    #[cfg(test)]
    pub wire_ends: Vec<(Item, Item, Pos2, Pos2)>,
    /// Where each output a wire is dragged out of was drawn: of which box, of which art layer
    /// for a layer's that are not its flat one.
    #[cfg(test)]
    pub outputs: Vec<(Item, Option<ArtLayerKind>, Pos2)>,
    /// What each Send and Receive said of its tag under its box, and each composite of its
    /// ordering.
    #[cfg(test)]
    pub tags_said: Vec<(Item, String)>,
    /// The wires that were drawn in another colour: the one a dragged node is about to go
    /// onto, and with `true` the one joined behind a node that comes off its wire.
    #[cfg(test)]
    pub lit: Vec<(Item, Item, bool)>,
    /// The boxes that were drawn darker: of nodes that are hidden or switched off.
    #[cfg(test)]
    pub dimmed: Vec<Item>,
    /// The boxes that were drawn outlined: of what is selected or picked.
    #[cfg(test)]
    pub outlined: Vec<Item>,
    /// Where the frame of each open group was drawn, and the name on it.
    #[cfg(test)]
    pub frames: Vec<(GroupId, Rect, Rect)>,
    /// The colour each open group's frame was drawn in.
    #[cfg(test)]
    pub frame_colours: Vec<(GroupId, Color32)>,
    /// The boxes that were drawn over the frames: of what is in an open group.
    #[cfg(test)]
    pub floating: Vec<Item>,
    /// The wires that were lit because they leave a node that is selected or picked.
    #[cfg(test)]
    pub leaving_chosen: Vec<(Item, Item)>,
}

impl Default for NodeView {
    fn default() -> Self {
        Self {
            locked: BTreeSet::new(),
            pan: vec2(0.0, 120.0),
            zoom: 1.0,
            drag: None,
            along: Vec::new(),
            lifting: false,
            marquee: None,
            arrange: false,
            fit: false,
            centre: None,
            map: false,
            #[cfg(test)]
            map_drawn: None,
            kept: (None, HashMap::new()),
            stack: Vec::new(),
            menu_at: None,
            paste_at: None,
            copied: false,
            marquee_shift: Vec2::ZERO,
            edge_armed: false,
            renaming: None,
            search: None,
            drawn: None,
            boxes: Vec::new(),
            #[cfg(test)]
            wire_ends: Vec::new(),
            #[cfg(test)]
            outputs: Vec::new(),
            #[cfg(test)]
            tags_said: Vec::new(),
            #[cfg(test)]
            lit: Vec::new(),
            #[cfg(test)]
            dimmed: Vec::new(),
            #[cfg(test)]
            outlined: Vec::new(),
            #[cfg(test)]
            frames: Vec::new(),
            #[cfg(test)]
            frame_colours: Vec::new(),
            #[cfg(test)]
            floating: Vec::new(),
            #[cfg(test)]
            leaving_chosen: Vec::new(),
        }
    }
}

/// Where each node's centre is: where it was put, or else where the layout puts it. The display
/// is at the bottom, what it shows above it, each composite's inputs in a row above the
/// composite, the top of the stack on the left, and each peg above what it carries.
pub fn layout(shot: &Shot) -> HashMap<Item, Pos2> {
    // The main display, with the others to its left in the order they were made.
    let mut places: HashMap<Item, Pos2> = HashMap::from([(Item::Display(shot.main_display()), pos2(0.0, 0.0))]);
    for (at, display) in shot.displays.iter().enumerate() {
        places.insert(Item::Display(display.id), pos2(-ACROSS * (at + 1) as f32, 0.0));
    }
    // How many columns a node's pictures take: a layer one, a composite those of its inputs,
    // an effect those of what is wired into it.
    fn columns(shot: &Shot, source: Source, depth: usize) -> f32 {
        match source {
            Source::Composite(_) | Source::Effect(_) if depth < 32 => {
                shot.inputs_of(source).into_iter().map(|input| columns(shot, input, depth + 1)).sum::<f32>().max(1.0)
            }
            _ => 1.0,
        }
    }
    fn put(shot: &Shot, places: &mut HashMap<Item, Pos2>, source: Source, center: Pos2, depth: usize) {
        if places.contains_key(&Item::of(source)) || depth >= 32 {
            return;
        }
        places.insert(Item::of(source), center);
        // An effect is under what goes through it, as a composite is under what it stacks.
        let inputs = shot.inputs_of(source);
        let mut left = center.x - columns(shot, source, depth) * ACROSS / 2.0;
        for input in inputs {
            let wide = columns(shot, input, depth + 1) * ACROSS;
            put(shot, places, input, pos2(left + wide / 2.0, center.y - DOWN), depth + 1);
            left += wide;
        }
    }
    if let Some(shown) = shot.display {
        put(shot, &mut places, shown, pos2(0.0, -DOWN), 0);
    }
    // What is connected to nothing the display shows: to the right, each with its own inputs.
    let rest = shot.composites.iter().map(|c| Source::Composite(c.id)).chain(shot.effects.iter().map(|e| Source::Effect(e.id)));
    let rest = rest.chain(shot.layers.iter().map(|l| Source::Layer(l.id)));
    for source in rest {
        if !places.contains_key(&Item::of(source)) {
            let right = places.values().map(|at| at.x).fold(0.0, f32::max);
            put(shot, &mut places, source, pos2(right + ACROSS * (columns(shot, source, 0) + 1.0) / 2.0, -DOWN), 0);
        }
    }
    // A peg is over the middle of what it carries, a step above the highest of it.
    fn put_peg(shot: &Shot, places: &mut HashMap<Item, Pos2>, peg: PegId, depth: usize) -> Option<Pos2> {
        if let Some(at) = places.get(&Item::Peg(peg)) {
            return Some(*at);
        }
        // A layer that bones bend is where its bones are, which the peg is above.
        let at_top = |layer: &iskra_model::Layer| {
            layer.deformer.and_then(|bones| places.get(&Item::Deformer(bones)).copied()).or(places.get(&Item::Layer(layer.id)).copied())
        };
        let layers = shot.layers.iter().filter(|l| l.parent == Some(peg)).filter_map(|l| at_top(l));
        let mut carried: Vec<Pos2> = layers.collect();
        if depth < 32 {
            let pegs: Vec<PegId> = shot.pegs.iter().filter(|p| p.parent == Some(peg)).map(|p| p.id).collect();
            carried.extend(pegs.into_iter().filter_map(|child| put_peg(shot, places, child, depth + 1)));
        }
        let at = match carried.len() {
            // A peg that carries nothing yet waits at the side.
            0 => pos2(places.values().map(|at| at.x).fold(0.0, f32::max) + ACROSS, -DOWN * 2.0),
            count => pos2(carried.iter().map(|at| at.x).sum::<f32>() / count as f32, carried.iter().map(|at| at.y).fold(f32::INFINITY, f32::min) - DOWN * 0.7),
        };
        places.insert(Item::Peg(peg), at);
        Some(at)
    }
    // A chain of bones is in the chain of what it bends: a step above the first layer it
    // bends, under the peg, which is above it as above what it carries.
    for deformer in &shot.deformers {
        // Where the layer is: where it was put, if it was, else where the layout has it.
        let at = |layer: &std::sync::Arc<iskra_model::Layer>| {
            shot.places.get(&layer.id.0).map(|[x, y]| pos2(*x as f32, *y as f32)).or(places.get(&Item::Layer(layer.id)).copied())
        };
        let bent = shot.layers.iter().filter(|layer| layer.deformer == Some(deformer.id)).find_map(at);
        if let Some(layer) = bent {
            places.insert(Item::Deformer(deformer.id), layer - vec2(0.0, DOWN * 0.7));
        }
    }
    for peg in &shot.pegs {
        put_peg(shot, &mut places, peg.id, 0);
    }
    // The camera is under the peg that moves it, where that carries nothing else; or else, and
    // with no peg, at the side.
    let alone = |peg: PegId| !shot.layers.iter().any(|layer| layer.parent == Some(peg)) && !shot.pegs.iter().any(|other| other.parent == Some(peg));
    let under = shot.camera.parent.filter(|peg| alone(*peg)).and_then(|peg| places.get(&Item::Peg(peg)).copied());
    let camera = under.map_or_else(|| pos2(places.values().map(|at| at.x).fold(0.0, f32::max) + ACROSS, -DOWN), |peg| peg + vec2(0.0, DOWN * 0.7));
    places.insert(Item::Camera, camera);
    // One that bends nothing waits at the side.
    for deformer in &shot.deformers {
        if !places.contains_key(&Item::Deformer(deformer.id)) {
            places.insert(Item::Deformer(deformer.id), pos2(places.values().map(|at| at.x).fold(0.0, f32::max) + ACROSS, -DOWN * 2.0));
        }
    }
    // A driver is at the left of the peg it drives, half a step up, where the peg is put.
    for driver in &shot.drivers {
        let peg = shot.places.get(&driver.drives.0).map(|[x, y]| pos2(*x as f32, *y as f32)).or(places.get(&Item::Peg(driver.drives)).copied());
        let at = peg.map_or_else(|| pos2(places.values().map(|at| at.x).fold(0.0, f32::max) + ACROSS, -DOWN * 2.0), |peg| peg - vec2(ACROSS, DOWN * 0.5));
        places.insert(Item::Driver(driver.id), at);
    }
    for (item, at) in &mut places {
        if let Some([x, y]) = shot.places.get(&item.key(shot)) {
            *at = pos2(*x as f32, *y as f32);
        }
    }
    // A group is in the middle of what is in it: where its box is when it is folded.
    for group in &shot.groups {
        let kept = shot.places.get(&group.id.0).map(|[x, y]| pos2(*x as f32, *y as f32));
        let middle = middle_of(shot, group.id, &places).or(kept).unwrap_or(Pos2::ZERO);
        places.insert(Item::Group(group.id), middle);
    }
    places
}

/// What goes along when `item` is dragged with `others`: those, and what is in each group among
/// them all, groups in groups too, folded or open. A group is where what is in it is, so one
/// whose nodes stayed behind would go back there when let go.
fn carried(shot: &Shot, item: Item, others: impl Iterator<Item = Item>) -> Vec<Item> {
    let mut along: Vec<Item> = others.collect();
    let groups: Vec<GroupId> =
        std::iter::once(item).chain(along.iter().copied()).filter_map(|item| if let Item::Group(group) = item { Some(group) } else { None }).collect();
    for group in groups {
        for within in shot.group_nodes(group).into_iter().filter_map(|key| item_of(shot, key)) {
            if within != item && !along.contains(&within) {
                along.push(within);
            }
        }
    }
    along
}

/// Where a folded group's box is: in the middle of the frame it has when it is open, round
/// what is in it where that is now, so that it folds and opens in place. Its place of its own
/// is not looked at, since what is in it moves without it while it is open. Ports are not in
/// a frame but on its edge, wherever their own places are. None for a group with nothing else.
fn middle_of(shot: &Shot, group: GroupId, places: &HashMap<Item, Pos2>) -> Option<Pos2> {
    let nodes = shot.group_nodes(group);
    let framed = |item: &Item| nodes.contains(&item.key(shot)) && !matches!(item, Item::Group(_) | Item::Display(_)) && !shot.is_port(item.key(shot));
    let around = places.iter().filter(|(item, _)| framed(item)).map(|(_, at)| Rect::from_center_size(*at, SIZE)).reduce(|all, rect| all.union(rect))?;
    Some(around.center())
}

/// Where each node is in a view that does not arrange the nodes by itself: where it was put;
/// else where it was first laid out, kept in `kept`. A node that is new, never put anywhere nor
/// laid out yet, goes by the node wired to it that is nearest in the layout and has a place:
/// as far from where that one is as the layout has it from it, so that it comes on top of
/// what it carries or beside what it is wired to, wherever those were dragged; and aside, where
/// another box is there already. A group's box is in the middle of what is in it, wherever that is.
fn arranged(shot: &Shot, kept: &mut HashMap<Item, Pos2>) -> HashMap<Item, Pos2> {
    let mut places = layout(shot);
    kept.retain(|item, _| places.contains_key(item));
    let put = |item: &Item| matches!(item, Item::Group(_)) || shot.places.contains_key(&item.key(shot));
    let mut new: Vec<Item> = places.keys().copied().filter(|item| !put(item) && !kept.contains_key(item)).collect();
    new.sort_by_key(|item| item.key(shot));
    // A new node first laid out on a frame where nothing was yet is where the layout has it.
    let first = kept.is_empty();
    for (item, at) in places.iter_mut().filter(|(item, _)| !put(item)) {
        if let Some(was) = kept.get(item) {
            *at = *was;
        }
    }
    // The layout as it would be with nothing put anywhere: how far apart it has two nodes.
    let free = match first || new.is_empty() {
        true => HashMap::new(),
        false => {
            let mut free = shot.clone();
            free.places.clear();
            layout(&free)
        }
    };
    let pairs: Vec<(Item, Item)> = if free.is_empty() { Vec::new() } else { wires(shot, &free).into_iter().map(|wire| (wire.from, wire.to)).collect() };
    // Those wired to one that has a place, one by one, so that the next can go by the last.
    while !first && !new.is_empty() {
        let has_place = |other: &Item| put(other) && !matches!(other, Item::Group(_)) || kept.contains_key(other);
        let found = new.iter().enumerate().find_map(|(at, item)| {
            let near = pairs.iter().filter_map(|(a, b)| {
                if a == item {
                    Some(*b)
                } else if b == item {
                    Some(*a)
                } else {
                    None
                }
            });
            let distance = |other: &Item| free.get(other).zip(free.get(item)).map_or(f32::INFINITY, |(a, b)| a.distance(*b));
            near.filter(has_place).min_by(|a, b| distance(a).total_cmp(&distance(b))).map(|anchor| (at, *item, anchor))
        });
        let Some((index, item, anchor)) = found else { break };
        new.remove(index);
        let (Some(anchor_at), Some(free_at), Some(free_anchor)) = (places.get(&anchor).copied(), free.get(&item), free.get(&anchor)) else { continue };
        let mut at = anchor_at + (*free_at - *free_anchor);
        // Aside, a step at a time, from a box that is there already: of what is seen, not put
        // away in a folded group. One that is put away itself needs no room.
        let seen = |other: &Item| shot.folded_into(other.key(shot)).is_none();
        let taken = |at: Pos2, places: &HashMap<Item, Pos2>| {
            let near = |there: &Pos2| (at - *there).abs().x < SIZE.x && (at - *there).abs().y < SIZE.y;
            places.iter().any(|(other, there)| *other != item && !matches!(other, Item::Group(_)) && seen(other) && near(there))
        };
        for _ in 0..24 {
            if !seen(&item) || !taken(at, &places) {
                break;
            }
            at.x += ACROSS;
        }
        places.insert(item, at);
        kept.insert(item, at);
    }
    for (item, at) in places.iter_mut().filter(|(item, _)| !put(item)) {
        *kept.entry(*item).or_insert(*at) = *at;
    }
    // A group's box is in the middle of what is in it as it is now.
    for group in &shot.groups {
        if let Some(middle) = middle_of(shot, group.id, &places) {
            places.insert(Item::Group(group.id), middle);
        }
    }
    places
}

/// The wires of the graph, given where the boxes are.
/// The port of an effect that its box has on top: its picture's, or for a kind that takes no
/// picture, as Particles, its first port. Its others are at its left side, in the kind's order.
/// `None` for a kind with no port at all, as a Colour Card or a Receive.
fn top_port(effect: &iskra_model::Effect) -> Option<&str> {
    match effect.described() {
        Some(kind) if kind.port(iskra_model::PICTURE).is_none() => kind.ports.first().map(|port| port.name),
        Some(_) | None => Some(iskra_model::PICTURE),
    }
}

/// The inputs of an effect along the top of its box, at its left, as a layer's art outputs are
/// along its bottom: each its port, how far along the top it is, and its hint. A Particles
/// node's: its sprites', for rain, or where something is wired there, its splash's, and for a
/// burst, or where something is wired there, its collider's. None for another kind, whose
/// inputs are on top in the middle and at the left side.
fn input_tabs(effect: &iskra_model::Effect) -> Vec<(&'static str, f32, &'static str)> {
    if effect.kind != iskra_model::PARTICLES {
        return Vec::new();
    }
    let mut tabs = vec![(iskra_model::SPRITE, 0.12, "Sprites: a layer wired here has its drawings drawn as the particles, each one of them at random")];
    if effect.choice("kind", 0) == "Rain" || effect.inputs.contains_key(iskra_model::SPLASH_SPRITE) {
        tabs.push((
            iskra_model::SPLASH_SPRITE,
            0.24,
            "Splash: a layer wired here is what a drop makes where it lands, its drawings played as the layer times them; with one drawing, a few droplets of it thrown up",
        ));
    }
    if effect.choice("kind", 0) == "Burst" || effect.inputs.contains_key(iskra_model::COLLIDER) {
        tabs.push((
            iskra_model::COLLIDER,
            0.12 * (tabs.len() + 1) as f32,
            "Collider: the burst's particles bounce off where a layer wired here is painted, as it moves; it is not seen for being wired here",
        ));
    }
    tabs
}

/// The ports of an effect at its box's left side, in its kind's order.
fn side_ports(effect: &iskra_model::Effect) -> Vec<&'static str> {
    let top = top_port(effect);
    effect.described().map_or_else(Vec::new, |kind| kind.ports.iter().map(|port| port.name).filter(|name| Some(*name) != top).collect())
}

fn wires(shot: &Shot, places: &HashMap<Item, Pos2>) -> Vec<Wire> {
    let top = |item: Item| places.get(&item).map(|at| *at - vec2(0.0, SIZE.y / 2.0));
    let mut wires = Vec::new();
    for composite in &shot.composites {
        let Some(top) = top(Item::Composite(composite.id)) else { continue };
        for (index, input) in composite.inputs.iter().enumerate() {
            // The inputs arrive side by side along the top, the top of the stack on the left.
            let x = top.x - SIZE.x / 2.0 + SIZE.x * (index + 1) as f32 / (composite.inputs.len() + 1) as f32;
            wires.push(Wire {
                from: Item::of(*input),
                to: Item::Composite(composite.id),
                end: pos2(x, top.y),
                kind: Kind::Picture,
                leaves: art_of(*input).map(art_port),
                fresh: false,
                was: None,
                art: art_of(*input),
            });
        }
    }
    // What is wired into an effect: the picture arrives at its top, and a matte at its left
    // side, so that it is never mistaken for the picture.
    for effect in &shot.effects {
        let Some(top) = top(Item::Effect(effect.id)) else { continue };
        // Inputs along the top at its left, each at its own.
        let tabs = input_tabs(effect);
        if !tabs.is_empty() {
            for (port, input) in &effect.inputs {
                let part = tabs.iter().find(|(tab, ..)| tab == port).map_or(0.5, |(_, part, _)| *part);
                let end = pos2(top.x - SIZE.x / 2.0 + SIZE.x * part, top.y);
                let (leaves, art) = (art_of(*input).map(art_port), art_of(*input));
                wires.push(Wire { from: Item::of(*input), to: Item::Effect(effect.id), end, kind: Kind::Picture, leaves, fresh: false, was: None, art });
            }
            continue;
        }
        let on_top = top_port(effect);
        // The side ports in the kind's order, then any its kind does not name.
        let sides = side_ports(effect);
        let mut others: Vec<(&String, &Source)> = effect.inputs.iter().filter(|(port, _)| Some(port.as_str()) != on_top).collect();
        others.sort_by_key(|(port, _)| sides.iter().position(|side| side == port).unwrap_or(sides.len()));
        let others = others.into_iter().enumerate();
        wires.extend(on_top.and_then(|port| effect.inputs.get(port)).copied().map(|input| Wire {
            from: Item::of(input),
            to: Item::Effect(effect.id),
            end: top,
            kind: Kind::Picture,
            leaves: art_of(input).map(art_port),
            fresh: false,
            was: None,
            art: art_of(input),
        }));
        for (index, (_, input)) in others {
            let end = top + vec2(-SIZE.x / 2.0, SIZE.y / 2.0 + 8.0 * index as f32);
            let (leaves, art) = (art_of(*input).map(art_port), art_of(*input));
            wires.push(Wire { from: Item::of(*input), to: Item::Effect(effect.id), end, kind: Kind::Picture, leaves, fresh: false, was: None, art });
        }
    }
    let shown = std::iter::once((shot.main_display(), shot.display)).chain(shot.displays.iter().map(|display| (display.id, display.shows)));
    for (display, shown) in shown {
        if let (Some(shown), Some(end)) = (shown, top(Item::Display(display))) {
            let (leaves, art) = (art_of(shown).map(art_port), art_of(shown));
            wires.push(Wire { from: Item::of(shown), to: Item::Display(display), end, kind: Kind::Picture, leaves, fresh: false, was: None, art });
        }
    }
    // A transform goes down from a peg, or from a port or a named port that hands one on
    // (plan §8.6), to what is parented to it; and from a peg into such a port.
    // A chain of bones is in the chain of what it bends: the transform goes into it, and on
    // from it to those of its layers that are under the same peg, and to a peg that follows
    // it there. What is under another peg has that peg's wire, and the bend beside it.
    let inline = |bones: Option<DeformerId>, parent: Option<PegId>| bones.is_some_and(|bones| bones_space(shot, bones) == Some(parent));
    let carried = shot.layers.iter().filter(|l| !inline(l.deformer, l.parent)).map(|l| (l.parent, Item::Layer(l.id)));
    let carried = carried.chain(shot.pegs.iter().filter(|p| !inline(p.follows.map(|follow| follow.deformer), p.parent)).map(|p| (p.parent, Item::Peg(p.id))));
    let carried = carried.chain(shot.effects.iter().map(|e| (e.parent, Item::Effect(e.id)))).chain([(shot.camera.parent, Item::Camera)]);
    let carried = carried.chain(shot.deformers.iter().filter_map(|d| Some((bones_space(shot, d.id)?, Item::Deformer(d.id)))));
    for (parent, child) in carried {
        if let (Some(parent), Some(end)) = (parent, top(child)) {
            let from = if shot.hands_down(parent) { Item::Effect(iskra_model::EffectId(parent.0)) } else { Item::Peg(parent) };
            wires.push(Wire { from, to: child, end, kind: Kind::Transform, leaves: None, fresh: false, was: None, art: None });
        }
    }
    for layer in &shot.layers {
        if let (Some(deformer), Some(end)) = (layer.deformer, top(Item::Layer(layer.id))) {
            let end = if inline(layer.deformer, layer.parent) { end } else { end - vec2(SIZE.x / 4.0, 0.0) };
            wires.push(Wire {
                from: Item::Deformer(deformer),
                to: Item::Layer(layer.id),
                end,
                kind: Kind::Bend,
                leaves: None,
                fresh: false,
                was: None,
                art: None,
            });
        }
    }
    // A peg that follows bones has its wire from them.
    for peg in &shot.pegs {
        if let (Some(follow), Some(end)) = (peg.follows, top(Item::Peg(peg.id))) {
            let end = if inline(Some(follow.deformer), peg.parent) { end } else { end - vec2(SIZE.x / 4.0, 0.0) };
            wires.push(Wire {
                from: Item::Deformer(follow.deformer),
                to: Item::Peg(peg.id),
                end,
                kind: Kind::Bend,
                leaves: None,
                fresh: false,
                was: None,
                art: None,
            });
        }
    }
    // The pegs a driver reads come into its top side by side, in the order of its inputs; it
    // goes into the left side of the peg it drives.
    for driver in &shot.drivers {
        if let (Some(at), Some(peg)) = (places.get(&Item::Driver(driver.id)), places.get(&Item::Peg(driver.drives))) {
            let end = *peg - vec2(SIZE.x / 2.0, 0.0);
            wires.push(Wire {
                from: Item::Driver(driver.id),
                to: Item::Peg(driver.drives),
                end,
                kind: Kind::Read,
                leaves: None,
                fresh: false,
                was: None,
                art: None,
            });
            let wired: Vec<PegId> = driver.inputs.iter().filter_map(|input| input.peg).collect();
            for (index, read) in wired.iter().enumerate() {
                let x = at.x - SIZE.x / 2.0 + SIZE.x * (index + 1) as f32 / (wired.len() + 1) as f32;
                let end = pos2(x, at.y - SIZE.y / 2.0);
                wires.push(Wire {
                    from: Item::Peg(*read),
                    to: Item::Driver(driver.id),
                    end,
                    kind: Kind::Read,
                    leaves: None,
                    fresh: false,
                    was: None,
                    art: None,
                });
            }
        }
    }
    // Pegs wired into the ports at the left of a chain's or a curve's top: the start's, which
    // carries its start, and the end's, its IK's goal or a curve's last point. A port, a Send
    // or a Receive can stand there for a peg.
    for chain in &shot.deformers {
        let Some(top) = top(Item::Deformer(chain.id)) else { continue };
        for (peg, part) in [(chain.start_peg, START_PORT), (end_peg(chain), END_PORT)] {
            if let Some(peg) = peg {
                let end = top + vec2(SIZE.x * (part - 0.5), 0.0);
                wires.push(Wire {
                    from: if shot.hands_down(peg) { Item::Effect(iskra_model::EffectId(peg.0)) } else { Item::Peg(peg) },
                    to: Item::Deformer(chain.id),
                    end,
                    kind: Kind::Bend,
                    leaves: None,
                    fresh: false,
                    was: None,
                    art: None,
                });
            }
        }
    }
    wires
}

/// The peg in a deformer's end port: a chain's IK goal, or what a curve's last point goes to.
fn end_peg(deformer: &iskra_model::Deformer) -> Option<PegId> {
    if deformer.is_curve() { deformer.end_peg } else { deformer.ik.as_ref().and_then(|ik| ik.peg) }
}

/// Where the start and end ports of a chain or a curve are along the top of its box, as parts
/// of its width from the left; a peg let go nearer the left than `PORTS` goes into one of them.
const START_PORT: f32 = 0.12;
const END_PORT: f32 = 0.3;
const PORTS: f32 = 0.4;

/// Bring the groups `key` is in to the top of `stack`, the outermost first so that each is on
/// top of those beside it, with `key` itself where it is a group.
fn raise(stack: &mut Vec<GroupId>, shot: &Shot, key: u64) {
    let own = shot.group(GroupId(key)).map(|group| group.id);
    let mut groups: Vec<GroupId> = own.into_iter().chain(shot.groups_above(key).map(|group| group.id)).collect();
    groups.reverse();
    stack.retain(|group| !groups.contains(group) && shot.group(*group).is_some());
    stack.extend(groups);
}

/// Whether `bones` has a start and an end port: a chain of bones or a curve, not an envelope.
fn has_ports(shot: &Shot, bones: DeformerId) -> bool {
    shot.deformer(bones).is_some_and(|chain| !chain.is_envelope())
}

/// The space a chain of bones lies in: that of the first layer it bends, which is under
/// that layer's peg (`Some(None)` under none). `None` for bones that bend nothing.
fn bones_space(shot: &Shot, bones: DeformerId) -> Option<Option<PegId>> {
    shot.layers.iter().find(|layer| layer.deformer == Some(bones)).map(|layer| layer.parent)
}

/// The commands that put every layer a chain of bones bends under `peg`, as one step.
fn bones_under(shot: &Shot, bones: DeformerId, peg: Option<PegId>, title: &str) -> Action {
    let commands: Vec<Value> = shot
        .layers
        .iter()
        .filter(|layer| layer.deformer == Some(bones))
        .map(|layer| json!({ "id": "layer.setParent", "params": { "layer": layer.id.0, "parent": peg.map(|peg| peg.0) } }))
        .collect();
    Action::Run("edit.batch", json!({ "commands": commands, "title": title }))
}

/// The command that takes a wire away.
fn cut(shot: &Shot, wire: &Wire) -> Option<Action> {
    // A wire into or out of a folded group is cut where it goes in the shot.
    Some(match wire.ends() {
        (Item::Deformer(_), Item::Layer(layer)) => Action::Run("layer.setDeformer", json!({ "layer": layer.0 })),
        (Item::Deformer(_), Item::Peg(peg)) => Action::Run("peg.setFollow", json!({ "peg": peg.0 })),
        (Item::Group(_), _) | (_, Item::Group(_)) => return None,
        // A peg a driver reads is let go: its input stays, empty. The driver's own wire into
        // its peg goes only with the driver.
        (Item::Peg(peg), Item::Driver(driver)) => {
            let input = shot.driver(driver)?.inputs.iter().find(|input| input.peg == Some(peg))?.name.clone();
            Action::Run("driver.connect", json!({ "driver": driver.0, "input": input }))
        }
        (Item::Driver(_), _) => return None,
        // A peg in a port: the start's, where it is wired there, else the end's. Or a port, a
        // Send or a Receive in its place.
        (Item::Peg(_) | Item::Effect(_), Item::Deformer(bones)) if wire.kind == Kind::Bend => {
            let peg = PegId(wire.ends().0.key(shot));
            let start = shot.deformer(bones).is_some_and(|chain| chain.start_peg == Some(peg));
            Action::Run("deformer.setPort", json!({ "deformer": bones.0, "port": if start { "start" } else { "end" }, "peg": null }))
        }
        // The transform into bones is that of the layers they bend.
        (_, Item::Deformer(bones)) => bones_under(shot, bones, None, "Disconnect Nodes"),
        (_, Item::Effect(effect)) if wire.kind == Kind::Transform => Action::Run("effect.setParent", json!({ "effect": effect.0 })),
        (_, Item::Effect(effect)) => {
            let port = shot.effect(effect)?.inputs.iter().find(|(_, input)| Some(**input) == wire.source())?.0.clone();
            Action::Run("effect.connect", json!({ "effect": effect.0, "port": port }))
        }
        (_, Item::Layer(layer)) => Action::Run("layer.setParent", json!({ "layer": layer.0 })),
        (_, Item::Peg(peg)) => Action::Run("peg.setParent", json!({ "peg": peg.0 })),
        (_, Item::Composite(composite)) => Action::Run("node.disconnect", json!({ "node": wire.source()?, "from": composite.0 })),
        (_, Item::Display(display)) => Action::Run("display.set", json!({ "display": display.0 })),
        (_, Item::Camera) => Action::Run("camera.setParent", json!({ "shot": shot.id.0 })),
        (_, Item::Driver(_)) => return None,
    })
}

/// The command that joins `from`, by the port under it, to `to`: by its flat output, or a
/// layer's by that of its art layer `art`. `index` is where among a composite's inputs the
/// wire was dropped, and `side` whether it was dropped on the left end of the box, where an
/// effect that has a matte port takes its matte.
fn join(shot: &Shot, (from, art): (Item, Option<ArtLayerKind>), to: Item, index: usize, across: Option<f32>) -> Option<Action> {
    // Let go at the left side of a box: what goes to an effect's side port.
    let side = across.is_some_and(|across| across < 0.3);
    let source = || source_of(from, art);
    Some(match (from, to) {
        (Item::Peg(peg), Item::Layer(layer)) => Action::Run("layer.setParent", json!({ "layer": layer.0, "parent": peg.0 })),
        (Item::Peg(parent), Item::Peg(peg)) => Action::Run("peg.setParent", json!({ "peg": peg.0, "parent": parent.0 })),
        (Item::Deformer(deformer), Item::Layer(layer)) => Action::Run("layer.setDeformer", json!({ "layer": layer.0, "deformer": deformer.0 })),
        // A peg wired from bones follows the end of them, under the peg the bones are under.
        (Item::Deformer(bones), Item::Peg(peg)) => {
            let mut commands = Vec::new();
            if let Some(space) = bones_space(shot, bones).filter(|space| *space != shot.peg(peg).and_then(|peg| peg.parent)) {
                commands.push(json!({ "id": "peg.setParent", "params": { "peg": peg.0, "parent": space.map(|space| space.0) } }));
            }
            commands.push(json!({ "id": "peg.setFollow", "params": { "peg": peg.0, "deformer": bones.0 } }));
            Action::Run("edit.batch", json!({ "commands": commands, "title": "Follow Bones" }))
        }
        // Let go at the left of a chain's or a curve's box, into the port it is nearer: the
        // start's, or the end's (a chain's IK goal, which it is given, on, if it had none).
        (Item::Peg(peg), Item::Deformer(bones)) if across.is_some_and(|across| across < PORTS) && has_ports(shot, bones) => {
            let start = across.is_some_and(|across| across < (START_PORT + END_PORT) / 2.0);
            Action::Run("deformer.setPort", json!({ "deformer": bones.0, "port": if start { "start" } else { "end" }, "peg": peg.0 }))
        }
        (Item::Peg(peg), Item::Deformer(bones)) => bones_under(shot, bones, Some(peg), "Connect Nodes"),
        // A port, a Send or a Receive that a peg comes into, as a peg into a port of bones.
        (Item::Effect(port), Item::Deformer(bones))
            if across.is_some_and(|across| across < PORTS) && has_ports(shot, bones) && shot.peg_above(Some(PegId(port.0))).is_some() =>
        {
            let start = across.is_some_and(|across| across < (START_PORT + END_PORT) / 2.0);
            Action::Run("deformer.setPort", json!({ "deformer": bones.0, "port": if start { "start" } else { "end" }, "peg": port.0 }))
        }
        // A transform into a port or a Send, and from one, or from a Receive, down to what
        // it then carries.
        (Item::Peg(peg), Item::Effect(effect)) => Action::Run("effect.setParent", json!({ "effect": effect.0, "parent": peg.0 })),
        (Item::Effect(port), Item::Layer(layer)) if shot.hands_down(PegId(port.0)) => {
            Action::Run("layer.setParent", json!({ "layer": layer.0, "parent": port.0 }))
        }
        (Item::Effect(port), Item::Peg(peg)) if shot.hands_down(PegId(port.0)) => Action::Run("peg.setParent", json!({ "peg": peg.0, "parent": port.0 })),
        // A peg wired into a driver is read by it, under a name made from the peg's.
        (Item::Peg(peg), Item::Driver(driver)) => {
            let driver = shot.driver(driver)?;
            if driver.inputs.iter().any(|input| input.peg == Some(peg)) {
                return None;
            }
            let name = input_name(driver, &shot.peg(peg)?.name);
            Action::Run("driver.addInput", json!({ "driver": driver.id.0, "name": name, "peg": peg.0 }))
        }
        // The camera is moved by the peg that is wired into it.
        (Item::Peg(peg), Item::Camera) => Action::Run("camera.setParent", json!({ "shot": shot.id.0, "parent": peg.0 })),
        (_, Item::Composite(composite)) => Action::Run("node.connect", json!({ "node": source()?, "to": composite.0, "index": index })),
        (_, Item::Display(display)) => Action::Run("display.set", json!({ "display": display.0, "node": source()? })),
        (_, Item::Effect(effect)) => {
            let node = shot.effect(effect)?;
            // Into the input along the top that it was let go nearest, among them; else into
            // the first with nothing wired, or the first.
            let tabs = input_tabs(node);
            if !tabs.is_empty() {
                let nearest = across
                    .filter(|across| *across < 0.4)
                    .and_then(|across| tabs.iter().min_by(|a, b| (a.1 - across).abs().total_cmp(&(b.1 - across).abs())).map(|(port, ..)| *port));
                let free = tabs.iter().find(|(port, ..)| !node.inputs.contains_key(*port)).or(tabs.first()).map(|(port, ..)| *port);
                let port = nearest.or(free)?;
                return Some(Action::Run("effect.connect", json!({ "effect": effect.0, "port": port, "node": source()? })));
            }
            // Let go at the left side, into its first side port; else into the one on top.
            let on_top = top_port(node);
            let port = side_ports(node).first().copied().filter(|_| side || on_top.is_none()).or(on_top)?;
            Action::Run("effect.connect", json!({ "effect": effect.0, "port": port, "node": source()? }))
        }
        _ => return None,
    })
}

/// The name a peg wired into `driver` is read by: the peg's, short of the `-P` pegs are
/// often named with, in the letters and digits a name of the expressions has, its first letter
/// small; with a number after it where the driver has it already or the language does.
fn input_name(driver: &iskra_model::Driver, peg: &str) -> String {
    let short = ["-P", "_P", " P", "-p", "_p"].iter().find_map(|end| peg.strip_suffix(end)).unwrap_or(peg);
    let mut name: String = short.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
    if let Some(first) = name.chars().next() {
        name.replace_range(..first.len_utf8(), &first.to_lowercase().to_string());
    }
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        name = format!("peg{name}");
    }
    let taken = |name: &str| iskra_model::reserved_in_expressions(name) || driver.input(name).is_some() || driver.parameter(name).is_some();
    if !taken(&name) {
        return name;
    }
    (2..).map(|n| format!("{name}{n}")).find(|name| !taken(name)).unwrap_or(name)
}

/// The commands, as one step, that take `wire` off the box it arrives at and wire what it
/// leaves into `onto`, as the input after `index` others. `None` where `onto` takes no such
/// wire.
fn rewired(shot: &Shot, wire: &Wire, onto: Item, index: usize) -> Option<Action> {
    moved(shot, wire, join(shot, (wire.ends().0, wire.art), onto, index, None)?)
}

/// Whether `node` can sit on a wire of `kind`: it takes in what such a wire carries and gives
/// it out again. A picture goes through an effect that takes one and through a composite; a
/// transform through a peg and through bones. A layer, a display and the camera are only
/// ever at an end, and a driver only reads.
fn sits_on(shot: &Shot, node: Item, kind: Kind) -> bool {
    match (node, kind) {
        (Item::Effect(id), Kind::Picture) => {
            shot.effect(id).and_then(iskra_model::Effect::described).is_none_or(|described| described.port(iskra_model::PICTURE).is_some())
        }
        (Item::Composite(_), Kind::Picture) | (Item::Peg(_) | Item::Deformer(_), Kind::Transform) => true,
        _ => false,
    }
}

/// The commands, as one step, that put `node`, which is wired to nothing, on `wire`: the wire
/// is cut, `node` goes where it arrived, and what it left is wired into `node`. `None` where
/// that can't be done.
fn put_on(shot: &Shot, wire: &Wire, node: Item) -> Option<Action> {
    let (from, to) = wire.ends();
    if matches!(from, Item::Group(_)) || matches!(to, Item::Group(_)) || !sits_on(shot, node, wire.kind) {
        return None;
    }
    let as_command = |action: Action| match action {
        Action::Run(id, params) => Some(json!({ "id": id, "params": params })),
        _ => None,
    };
    // Where it arrived: a composite's input of the same place, an effect's same port.
    let into = match to {
        Item::Composite(id) => {
            let index = shot.composite(id)?.inputs.iter().position(|input| Some(*input) == wire.source())?;
            Action::Run("node.connect", json!({ "node": node.source()?, "to": id.0, "index": index }))
        }
        Item::Effect(id) if wire.kind == Kind::Picture => {
            let port = shot.effect(id)?.inputs.iter().find(|(_, input)| Some(**input) == wire.source())?.0.clone();
            Action::Run("effect.connect", json!({ "effect": id.0, "port": port, "node": node.source()? }))
        }
        _ => join(shot, (node, None), to, 0, None)?,
    };
    let commands = [cut(shot, wire)?, into, join(shot, (from, wire.art), node, 0, None)?];
    let commands: Vec<Value> = commands.into_iter().map(as_command).collect::<Option<_>>()?;
    Some(Action::Run("edit.batch", json!({ "commands": commands, "title": "Put Node on a Wire" })))
}

/// The commands, as one step, that take `wire` off the box it arrives at and wire what it
/// leaves elsewhere by `joined`.
fn moved(shot: &Shot, wire: &Wire, joined: Action) -> Option<Action> {
    let as_command = |action: Action| match action {
        Action::Run(id, params) => Some(json!({ "id": id, "params": params })),
        _ => None,
    };
    let joined = as_command(joined)?;
    // A peg's or a chain of bones' wire to another layer is the layer's own: what the first
    // had is taken away by its own command. Others are cut where they arrived.
    let commands: Vec<Value> = cut(shot, wire).and_then(as_command).into_iter().chain([joined]).collect();
    Some(Action::Run("edit.batch", json!({ "commands": commands, "title": "Rewire" })))
}

/// The ways a composite orders what it stacks (plan §8.6), as the menus offer them: the
/// title, a hint, the name `composite.setOrdering` takes, and which port is on top in 2D.
pub(crate) const ORDERINGS: [(&str, &str, &str, iskra_model::Stacking, Option<bool>); 4] = [
    ("3D", "By depth, among everything else in the shot: something from elsewhere can stand between what it stacks", "3d", iskra_model::Stacking::Depth, None),
    ("3D Flat", "By depth among what it stacks, and then one flat picture that nothing comes between", "3dFlat", iskra_model::Stacking::DepthFlat, None),
    ("2D, Left on Top", "By the order of its ports alone, whatever the depths: the left port is on top", "2d", iskra_model::Stacking::Ports, Some(false)),
    ("2D, Right on Top", "By the order of its ports alone, whatever the depths: the right port is on top", "2d", iskra_model::Stacking::Ports, Some(true)),
];

/// The title of the way a composite orders what it stacks.
pub(crate) fn ordering_title(composite: &iskra_model::Composite) -> &'static str {
    let way = ORDERINGS.iter().find(|(.., stacking, right)| *stacking == composite.stacking && right.is_none_or(|right| right == composite.right_first));
    way.map_or("3D", |(title, ..)| title)
}

/// A new port of `group`, of `kind`, at `at` in the node view, with a wire from `from` going
/// into it: its picture, or for a peg, or a port that hands one down, its transform.
fn new_port(shot: &Shot, (from, art): (Item, Option<ArtLayerKind>), group: GroupId, kind: &str, at: [f64; 2]) -> Option<Action> {
    let mut params = json!({ "shot": shot.id.0, "kind": kind, "group": group.0, "at": at });
    match from {
        Item::Peg(peg) => params["parent"] = json!(peg.0),
        Item::Effect(port) if shot.hands_down(PegId(port.0)) && shot.effect(port).is_some_and(|port| port.parent.is_some()) => {
            params["parent"] = json!(port.0);
        }
        other => params["from"] = json!(source_of(other, art)?),
    }
    Some(Action::Run("effect.new", params))
}

/// Whether a node is out of what the shot shows, as its row on the timeline says: it is
/// hidden there, or a peg that carries it is, or a group that it or one of those is in; or it
/// is an effect that is switched off. Such a node's box is drawn darker.
fn is_off(shot: &Shot, item: Item) -> bool {
    let gone = |node: u64| shot.hidden.contains(&node) || shot.groups_above(node).any(|group| shot.hidden.contains(&group.id.0));
    match item {
        Item::Display(_) | Item::Camera => false,
        Item::Layer(id) => shot.hides(id),
        Item::Peg(id) => gone(id.0) || shot.ancestors(shot.peg(id).and_then(|peg| peg.parent)).any(|peg| gone(peg.id.0)),
        Item::Effect(id) => gone(id.0) || shot.effect(id).is_some_and(|effect| !effect.enabled),
        Item::Driver(id) => shot.driver(id).is_some_and(|driver| driver.off),
        Item::Composite(_) | Item::Deformer(_) | Item::Group(_) => gone(item.key(shot)),
    }
}

/// The commands that switch nodes of the shot on or off, by their ids, as one step: an
/// effect is switched, and a layer, a peg or a group has its row shown or hidden, which is
/// what being off is for them. Those that are so already, and the kinds that have no switch
/// (a composite, bones, the camera), are left out; `None` where that leaves nothing.
pub(crate) fn switching(shot: &Shot, nodes: impl IntoIterator<Item = u64>, on: bool) -> Option<Value> {
    let one = |node: u64| match item_of(shot, node)? {
        Item::Effect(id) => (shot.effect(id)?.enabled != on).then(|| json!({ "id": "effect.setEnabled", "params": { "effect": node, "enabled": on } })),
        Item::Layer(_) | Item::Peg(_) | Item::Group(_) => {
            (shot.hidden.contains(&node) == on).then(|| json!({ "id": "shot.setHidden", "params": { "node": node, "hidden": !on } }))
        }
        Item::Driver(id) => (shot.driver(id)?.off == on).then(|| json!({ "id": "driver.setEnabled", "params": { "driver": node, "enabled": on } })),
        Item::Composite(_) | Item::Deformer(_) | Item::Display(_) | Item::Camera => None,
    };
    let commands: Vec<Value> = nodes.into_iter().filter_map(one).collect();
    let title = if on { "Switch On" } else { "Switch Off" };
    (!commands.is_empty()).then(|| json!({ "commands": commands, "title": title }))
}

/// The box a node of the shot is, by the node's id.
fn item_of(shot: &Shot, key: u64) -> Option<Item> {
    let kinds = [
        Item::Layer(LayerId(key)),
        Item::Peg(PegId(key)),
        Item::Composite(CompositeId(key)),
        Item::Deformer(DeformerId(key)),
        Item::Group(GroupId(key)),
        Item::Effect(EffectId(key)),
        Item::Display(DisplayId(key)),
        Item::Camera,
        Item::Driver(DriverId(key)),
    ];
    kinds.into_iter().find(|item| match item {
        Item::Layer(id) => shot.layer(*id).is_some(),
        Item::Peg(id) => shot.peg(*id).is_some(),
        Item::Composite(id) => shot.composite(*id).is_some(),
        Item::Deformer(id) => shot.deformer(*id).is_some(),
        Item::Group(id) => shot.group(*id).is_some(),
        Item::Effect(id) => shot.effect(*id).is_some(),
        Item::Camera => shot.camera.id.0 == key,
        Item::Driver(id) => shot.driver(*id).is_some(),
        // The main display is under the shot's own id, which is no node's.
        Item::Display(id) => *id != shot.main_display() && shot.has_display(*id),
    })
}

/// A node with what goes with it, by their ids: what a group of it is made of. A layer goes
/// with the bones that bend it; a peg with all it carries; a composite with all it stacks and
/// the pegs over that. What of it is in a group that the node is not in goes as that group,
/// the outermost such, whole: a group is not broken up to make another.
fn rig_of(shot: &Shot, item: Item) -> Vec<u64> {
    let mut layers: Vec<LayerId> = Vec::new();
    let mut nodes = vec![item.key(shot)];
    match item {
        Item::Layer(id) => layers.push(id),
        Item::Peg(id) => {
            let under = |parent: Option<PegId>| shot.ancestors(parent).any(|peg| peg.id == id);
            nodes.extend(shot.pegs.iter().filter(|peg| under(peg.parent)).map(|peg| peg.id.0));
            layers.extend(shot.layers.iter().filter(|layer| under(layer.parent)).map(|layer| layer.id));
        }
        Item::Composite(id) => {
            let mut open = vec![Source::Composite(id)];
            while let Some(source) = open.pop() {
                match source {
                    Source::Layer(layer) | Source::Art(layer, _) => layers.push(layer),
                    Source::Composite(composite) if composite == id || !nodes.contains(&composite.0) => {
                        nodes.push(composite.0);
                        open.extend(shot.composite(composite).into_iter().flat_map(|composite| &composite.inputs));
                    }
                    Source::Composite(_) => {}
                    // What goes through an effect is of the rig, and so is the effect.
                    Source::Effect(effect) if !nodes.contains(&effect.0) => {
                        nodes.push(effect.0);
                        open.extend(shot.effect(effect).into_iter().flat_map(|effect| effect.inputs.values()));
                    }
                    Source::Effect(_) => {}
                }
            }
            let above = layers.iter().filter_map(|layer| shot.layer(*layer)).flat_map(|layer| shot.ancestors(layer.parent)).map(|peg| peg.id.0);
            nodes.extend(above);
        }
        Item::Deformer(id) => layers.extend(shot.layers.iter().filter(|layer| layer.deformer == Some(id)).map(|layer| layer.id)),
        Item::Group(_) | Item::Effect(_) | Item::Display(_) | Item::Camera | Item::Driver(_) => {}
    }
    nodes.extend(layers.iter().map(|layer| layer.0));
    nodes.extend(layers.iter().filter_map(|layer| shot.layer(*layer)?.deformer).map(|deformer| deformer.0));
    let key = item.key(shot);
    let whole = |node: u64| shot.groups_above(node).filter(|group| !shot.group_nodes(group.id).contains(&key)).last().map_or(node, |group| group.id.0);
    let mut nodes: Vec<u64> = nodes.into_iter().map(|node| if node == key { node } else { whole(node) }).collect();
    nodes.sort_unstable();
    nodes.dedup();
    nodes
}

/// The categories the kinds of effect node are in (plan §8.4), in the order they are listed.
fn categories() -> Vec<&'static str> {
    let mut categories: Vec<&'static str> = Vec::new();
    for kind in effect_kinds() {
        if !categories.contains(&kind.category) {
            categories.push(kind.category);
        }
    }
    categories
}

/// Where an effect node that is added goes, with one node picked that gives a picture: on
/// the wire that leaves that node, just below its box, in its group. Its name says which.
#[derive(Clone)]
struct Below {
    on: Source,
    at: [f64; 2],
    group: Option<u64>,
    name: String,
    /// Where what the node's picture goes on to is moved, to make room for the new one: as
    /// `node.place` takes them, none where there is room already.
    room: Vec<Value>,
}

impl Below {
    /// Below `item`, as the view has it at `places`, if it gives a picture.
    fn of(shot: &Shot, item: Item, places: &HashMap<Item, Pos2>) -> Option<Below> {
        let (on, place) = (item.source()?, *places.get(&item)?);
        let group = shot.group_of(item.key(shot)).map(|group| group.id.0);
        // What the picture goes on to and is under it moves down, all by as much, so that
        // the nearest is a row under the node again, with the new one between.
        let under: Vec<(Item, Pos2)> =
            downstream(shot, on).into_iter().filter_map(|item| Some((item, *places.get(&item)?))).filter(|(_, at)| at.y > place.y).collect();
        let nearest = under.iter().map(|(_, at)| at.y).fold(f32::INFINITY, f32::min);
        let push = (place.y + DOWN - nearest).max(0.0);
        let room = if push > 0.0 { under.iter().map(|(item, at)| json!({ "node": item.key(shot), "at": [at.x, at.y + push] })).collect() } else { Vec::new() };
        Some(Below { on, at: [f64::from(place.x), f64::from(place.y + DOWN / 2.0)], group, name: item.name(shot).to_string(), room })
    }
}

/// What `source` gives its picture to, straight or through others: the composites, the
/// effects and the display, as boxes of the view, the nearest first.
fn downstream(shot: &Shot, source: Source) -> Vec<Item> {
    let (mut seen, mut open) = (Vec::new(), std::collections::VecDeque::from([source]));
    while let Some(at) = open.pop_front() {
        let composites = shot.composites.iter().filter(|c| c.inputs.contains(&at)).map(|c| Source::Composite(c.id));
        let takers: Vec<Source> =
            composites.chain(shot.effects.iter().filter(|e| e.inputs.values().any(|input| *input == at)).map(|e| Source::Effect(e.id))).collect();
        for taker in takers {
            if !seen.contains(&Item::of(taker)) {
                seen.push(Item::of(taker));
                open.push_back(taker);
            }
        }
        let shown = std::iter::once((shot.main_display(), shot.display)).chain(shot.displays.iter().map(|display| (display.id, display.shows)));
        for (display, _) in shown.filter(|(_, shown)| *shown == Some(at)) {
            if !seen.contains(&Item::Display(display)) {
                seen.push(Item::Display(display));
            }
        }
    }
    seen
}

/// The command that adds an effect of `kind`: below the picked node, on the wire that leaves
/// it, where there is one and the kind takes a picture; else at `at` in the graph, in the group
/// `group`.
/// Below it, what the picture goes on to is moved down to make room, as the same step.
fn added(shot: &Shot, kind: &iskra_model::EffectKind, below: Option<&Below>, at: Option<[f64; 2]>, group: Option<u64>) -> Action {
    let Some(below) = below.filter(|_| kind.port(iskra_model::PICTURE).is_some()) else {
        // A node that makes its own picture is seen at once, where it is not put into a group:
        // a Colour Card under everything else, as a background, and rain or snow over it.
        let stack = (kind.makes_picture() && group.is_none()).then_some(if kind.id == iskra_model::COLOUR_CARD { "under" } else { "over" });
        return Action::Run("effect.new", json!({ "shot": shot.id.0, "kind": kind.id, "at": at, "group": group, "stack": stack }));
    };
    let made = json!({ "shot": shot.id.0, "kind": kind.id, "on": below.on, "at": below.at, "group": below.group });
    if below.room.is_empty() {
        return Action::Run("effect.new", made);
    }
    let commands = json!([{ "id": "effect.new", "params": made }, { "id": "node.place", "params": { "moves": below.room } }]);
    Action::Run("edit.batch", json!({ "commands": commands, "title": format!("Add {}", kind.title) }))
}

/// The nodes the search and *Add Node* add that are no effects: their titles, the command that
/// makes each, what each is, and the category of plan §8.4 it is listed under.
const STRUCTURE: [(&str, &str, &str, &str); 6] = [
    ("Layer", "layer.new", "A drawing layer, on top of what the composite it goes to stacks", "Sources"),
    ("Text Layer", TEXT_LAYER_NEW, "A text layer (plan §4.19): its drawings are texts, typed with the Text tool", "Sources"),
    ("Peg", "peg.new", "A peg: it moves, turns and scales what is parented to it", "Move"),
    ("Composite", "composite.new", "A composite: it stacks what is wired into it into one picture", "Combine"),
    ("Display", "display.new", "Another display: another picture of the shot, for the viewer", "Structure"),
    ("Pass", PASS_NEW, "A pass: what is wired into it is rendered apart on the Export page, under its name", "Structure"),
];

/// The command and its parameters that make the node of `STRUCTURE` that `command` names, at
/// `at` in the node view and in the open group `group` there; a display, which is in no group.
fn structure_made(shot: &Shot, command: &'static str, at: Option<[f64; 2]>, group: Option<u64>) -> Action {
    let (command, params) = match command {
        "display.new" => (command, json!({ "shot": shot.id.0, "at": at })),
        PASS_NEW => ("display.new", json!({ "shot": shot.id.0, "at": at, "pass": true })),
        TEXT_LAYER_NEW => ("layer.new", json!({ "shot": shot.id.0, "group": group, "at": at, "kind": "text" })),
        _ => (command, json!({ "shot": shot.id.0, "group": group, "at": at })),
    };
    Action::Run(command, params)
}

/// What the search says to make a Pass: `display.new` with `pass`.
const PASS_NEW: &str = "display.newPass";
/// What the search says to make a text layer: `layer.new` of the kind `text`.
const TEXT_LAYER_NEW: &str = "layer.newText";

/// The kinds of effect node whose category and title have every word of `query`.
pub(crate) fn found(query: &str) -> Vec<&'static iskra_model::EffectKind> {
    let query = query.to_lowercase();
    let has_all = |kind: &&iskra_model::EffectKind| {
        let said = format!("{} {}", kind.category, kind.title).to_lowercase();
        query.split_whitespace().all(|word| said.contains(word))
    };
    // A group's ports are no nodes to add: grouping makes them, and a wire let go on a frame.
    effect_kinds().iter().filter(|kind| !is_port_kind(kind.id)).filter(has_all).collect()
}

/// Whether a kind of effect node is a port of a group.
/// Whether a port of a group was given a name of its own: not its kind's, as a new one has,
/// with or without a number after it.
fn named_port(shot: &Shot, item: Item) -> bool {
    let Item::Effect(id) = item else { return false };
    let Some(port) = shot.effect(id) else { return false };
    let Some(kind) = port.described() else { return true };
    let rest = port.name.strip_prefix(kind.title);
    !rest.is_some_and(|rest| rest.is_empty() || rest.strip_prefix(' ').is_some_and(|number| number.chars().all(|c| c.is_ascii_digit())))
}

fn is_port_kind(kind: &str) -> bool {
    kind == iskra_model::PORT_IN || kind == iskra_model::PORT_OUT
}

/// The command that takes a wire away with Alt: the wire's own, or, where it was the last
/// wire of a port of a group, the one that takes the port away with it.
fn cut_last(shot: &Shot, wire: &Wire, all: &[Wire], inside: &[(Item, Item)]) -> Option<Action> {
    let lone = |item: &Item| {
        let ends = all.iter().map(Wire::ends).chain(inside.iter().copied());
        matches!(item, Item::Effect(id) if shot.is_port(id.0)) && ends.filter(|(from, to)| from == item || to == item).count() == 1
    };
    let (from, to) = wire.ends();
    match [from, to].iter().find(|item| lone(item)) {
        Some(Item::Effect(port)) => Some(Action::Run("effect.delete", json!({ "effect": port.0 }))),
        _ => cut(shot, wire),
    }
}

/// The kind of wire that is dragged out of a box.
/// What a group made with a composite is, as its menus say it.
pub(crate) const GATHER_HINT: &str = "The group gets a composite of its own: the pictures that left the group go into it, and it is wired in where they went, \
                                      so that one wire leaves the group";

/// How wide the band along the view's edges is in which a drag moves the view, and how fast
/// at most, in points a second.
const EDGE: f32 = 28.0;
const EDGE_SPEED: f32 = 700.0;

/// How fast the view moves, in points a second, with a drag's pointer at `at`: nothing in the
/// middle of the view, towards the right with the pointer in the band along the left edge,
/// and so for each edge, faster the nearer the edge and fastest on it and past it.
fn edge_push(view: Rect, at: Pos2) -> Vec2 {
    let push = |from_low: f32, from_high: f32| {
        let into = |from: f32| ((EDGE - from) / EDGE).clamp(0.0, 1.0);
        (into(from_low) - into(from_high)) * EDGE_SPEED
    };
    vec2(push(at.x - view.left(), view.right() - at.x), push(at.y - view.top(), view.bottom() - at.y))
}

fn kind_from(item: Item) -> Kind {
    match item {
        Item::Peg(_) => Kind::Transform,
        Item::Deformer(_) => Kind::Bend,
        Item::Driver(_) => Kind::Read,
        Item::Layer(_) | Item::Composite(_) | Item::Group(_) | Item::Effect(_) | Item::Display(_) | Item::Camera => Kind::Picture,
    }
}

/// How far `pos` is from the straight piece between `a` and `b`.
fn distance_to_piece(pos: Pos2, a: Pos2, b: Pos2) -> f32 {
    let (along, to) = (b - a, pos - a);
    let part = if along.length_sq() > 0.0 { (to.dot(along) / along.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (to - along * part).length()
}

impl NodeView {
    /// How far the view zooms out and in: as a pinch does, and the bar's slider. Far out, so
    /// that a large rig is seen whole.
    pub const ZOOM: std::ops::RangeInclusive<f32> = 0.02..=3.0;

    /// Points per unit of the graph.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Zoom to `to` points per unit about the middle of the view: what is there stays.
    pub fn zoom_to(&mut self, to: f32) {
        let to = to.clamp(*Self::ZOOM.start(), *Self::ZOOM.end());
        self.pan *= to / self.zoom;
        self.zoom = to;
    }

    /// The search that adds a node: a field, and under it the kinds that have what was
    /// typed, with their categories. Enter adds the one that is marked, the arrows mark
    /// another, a click adds any, and Escape puts the search away. The node is put where the
    /// search was opened.
    fn search_window(
        &mut self,
        ui: &egui::Ui,
        shot: &Shot,
        here: impl Fn(Option<Pos2>) -> Option<[f64; 2]>,
        into: impl Fn(Option<Pos2>) -> Option<u64>,
        below: Option<&Below>,
        driving: Option<PegId>,
        actions: &mut Vec<Action>,
    ) {
        let Some((query, at, marked)) = self.search.as_mut() else { return };
        let (at, mut chosen, mut made, mut drive) = (*at, None, None, false);
        let mut close = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let area = egui::Area::new(egui::Id::new("add node search")).order(egui::Order::Foreground).fixed_pos(at);
        let shown = area.show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(220.0);
                // The nodes that are no effects first, then the kinds of effect.
                let words = query.to_lowercase();
                let structure: Vec<&(&str, &str, &str, &str)> = STRUCTURE
                    .iter()
                    .filter(|(title, _, _, category)| {
                        words.split_whitespace().all(|word| format!("{} {}", category.to_lowercase(), title.to_lowercase()).contains(word))
                    })
                    .collect();
                // A Driver, of the Move category, after the nodes that are no effects.
                let driver = words.split_whitespace().all(|word| "move driver".contains(word));
                let kinds = found(query);
                let count = structure.len() + usize::from(driver) + kinds.len();
                let moved = ui.input_mut(|i| {
                    i64::from(i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) - i64::from(i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp))
                });
                *marked = (*marked as i64 + moved).clamp(0, count.saturating_sub(1) as i64) as usize;
                let entered = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                let hint = below.map_or("Add a node".to_string(), |below| format!("Add a node below {}", below.name));
                let field = ui.add(egui::TextEdit::singleline(query).hint_text(hint).desired_width(f32::INFINITY));
                if field.changed() {
                    *marked = 0;
                }
                field.request_focus();
                ui.separator();
                for (index, (title, command, hint, category)) in structure.iter().enumerate() {
                    let row = ui.add(egui::Button::selectable(index == *marked, *title).shortcut_text(*category)).on_hover_text(*hint);
                    if row.clicked() || (entered && index == *marked) {
                        made = Some(*command);
                    }
                }
                if driver {
                    let index = structure.len();
                    let hint = match driving.and_then(|peg| shot.peg(peg)) {
                        Some(peg) => format!("A Driver of {}: expressions over other pegs that add to its position, rotation and scale", peg.name),
                        None => "A Driver goes on a peg: select a peg that has none first".to_string(),
                    };
                    let button = egui::Button::selectable(index == *marked, "Driver").shortcut_text("Move");
                    let row = ui.add_enabled(driving.is_some(), button).on_hover_text(hint.clone()).on_disabled_hover_text(hint);
                    if row.clicked() || (entered && index == *marked && driving.is_some()) {
                        drive = true;
                    }
                }
                let after = structure.len() + usize::from(driver);
                for (index, kind) in kinds.iter().enumerate().map(|(at, kind)| (at + after, kind)) {
                    let row = ui.add(egui::Button::selectable(index == *marked, kind.title).shortcut_text(kind.category)).on_hover_text(kind.hint);
                    if row.clicked() || (entered && index == *marked) {
                        chosen = Some(kind.id);
                    }
                }
                if count == 0 {
                    ui.weak("No node of that name");
                }
                close |= entered;
            });
        });
        // A click anywhere else puts it away.
        close |= ui.input(|i| i.pointer.any_pressed()) && !shown.response.contains_pointer();
        if let Some(kind) = chosen.and_then(iskra_model::effect_kind) {
            actions.push(added(shot, kind, below, here(Some(at)), into(Some(at))));
        }
        // A layer, a peg or a composite where the search was opened, in the open group there; a
        // display there, which is in no group.
        if let Some(command) = made {
            actions.push(structure_made(shot, command, here(Some(at)), into(Some(at))));
        }
        if let (true, Some(peg)) = (drive, driving) {
            actions.push(Action::Run("driver.new", json!({ "peg": peg.0, "at": here(Some(at)) })));
        }
        if close || chosen.is_some() || made.is_some() || drive {
            self.search = None;
        }
    }

    /// Where a composite gathering `nodes` goes in the graph: under them, in the middle of
    /// them across, a row below the lowest. As the view has them now: where they were put,
    /// or where it laid them out.
    pub fn below(&self, shot: &Shot, nodes: &[u64]) -> Option<[f64; 2]> {
        let places = match self.kept.0 == Some(shot.id) && !self.arrange {
            true => arranged(shot, &mut self.kept.1.clone()),
            false => layout(shot),
        };
        let at: Vec<Pos2> = nodes.iter().filter_map(|key| item_of(shot, *key)).filter_map(|item| places.get(&item).copied()).collect();
        let lowest = at.iter().map(|at| at.y).fold(None, |most: Option<f32>, y| Some(most.map_or(y, |most| most.max(y))))?;
        let across = at.iter().map(|at| at.x).sum::<f32>() / at.len() as f32;
        Some([f64::from(across), f64::from(lowest + DOWN)])
    }

    /// Where the view was last drawn in the window: under its bar, in its panel.
    #[cfg(test)]
    pub fn shown_in(&self) -> Option<Rect> {
        self.drawn.map(|(rect, _)| rect)
    }

    /// Lay the nodes that were never dragged anywhere out anew, once.
    pub fn arrange_now(&mut self) {
        self.kept.1.clear();
    }

    /// The box at `pos` of the window, the topmost where several are, if the view is there.
    pub fn item_at(&self, pos: Pos2) -> Option<Item> {
        self.drawn.filter(|(rect, _)| rect.contains(pos))?;
        self.boxes.iter().rev().find(|(_, rect)| rect.contains(pos)).map(|(item, _)| *item)
    }

    /// The place in the graph that is at `pos` of the window, if the view is there.
    pub fn graph_pos(&self, pos: Pos2) -> Option<[f64; 2]> {
        let (_, origin) = self.drawn.filter(|(rect, _)| rect.contains(pos))?;
        let at = (pos - origin) / self.zoom;
        Some([f64::from(at.x), f64::from(at.y)])
    }

    /// Show the graph of `shot`. `selected` is the layer or peg that is selected in the
    /// timeline, `picked` the nodes that are picked together, by their ids, and `frame` the
    /// frame to stay on when one is selected here.
    pub fn show(&mut self, ui: &mut egui::Ui, shot: &Shot, selected: Option<Node>, picked: &BTreeSet<u64>, frame: u32) -> Vec<Action> {
        let mut actions = Vec::new();
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let (rect, theme) = (response.rect, Theme::of(ui.ctx()));
        // Scrolling pans, and so does dragging the background with the middle button or with
        // Space held; a pinch zooms about the pointer.
        // Over a box too: the pointer need only be in the view, with no window over it.
        if let Some(pointer) = ui.input(|i| i.pointer.hover_pos()).filter(|_| ui.rect_contains_pointer(rect)) {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta(), i.zoom_delta()));
            self.pan += scroll;
            if zoom != 1.0 {
                let to = (self.zoom * zoom).clamp(*Self::ZOOM.start(), *Self::ZOOM.end());
                let from_origin = pointer - rect.center() - self.pan;
                self.pan += from_origin - from_origin * (to / self.zoom);
                self.zoom = to;
            }
        }
        // What is dragged to an edge of the view takes the view along: the graph moves the
        // other way under it, faster the nearer the edge. A box that is dragged out of the
        // view is on its way to the library, and moves nothing.
        let leaving = matches!(self.drag, Some(Drag::Node(..)));
        let dragging = self.drag.is_some() || self.marquee.is_some();
        self.edge_armed &= dragging;
        if let Some(pointer) = ui.input(|i| i.pointer.latest_pos()).filter(|at| dragging && (rect.contains(*at) || !leaving)) {
            let push = edge_push(rect, pointer) * ui.input(|i| i.stable_dt).min(0.1);
            self.edge_armed |= push == Vec2::ZERO;
            if push != Vec2::ZERO && self.edge_armed {
                self.pan += push;
                self.marquee_shift += push;
                // The box that is dragged stays under the pointer.
                if let Some(Drag::Node(_, at)) = &mut self.drag {
                    *at -= push / self.zoom;
                    for (_, at) in &mut self.along {
                        *at -= push / self.zoom;
                    }
                }
                ui.ctx().request_repaint();
            }
        }
        let panning = response.dragged_by(egui::PointerButton::Middle) || (response.dragged() && ui.input(|i| i.key_down(egui::Key::Space)));
        if panning {
            self.pan += response.drag_delta();
        }
        let (origin, zoom) = (rect.center() + self.pan, self.zoom);
        self.drawn = Some((rect, origin));
        let on_screen = |at: Pos2| origin + at.to_vec2() * zoom;

        // Not arranged by the view, a node that was never put anywhere stays where it was
        // first laid out, and a new one comes beside what it is wired to (see `arranged`).
        if self.kept.0 != Some(shot.id) || self.arrange {
            self.kept = (Some(shot.id), HashMap::new());
        }
        let mut places = if self.arrange { layout(shot) } else { arranged(shot, &mut self.kept.1) };
        if let Some(Drag::Node(item, at)) = &self.drag {
            places.insert(*item, *at);
            places.extend(self.along.iter().copied());
        }
        // Every node seen, once: the view as wide as all of them with a box's room about them,
        // its middle on theirs, from the next frame on.
        if std::mem::take(&mut self.fit) {
            let mut around = Rect::NOTHING;
            for at in places.values() {
                around = around.union(Rect::from_center_size(*at, SIZE));
            }
            if around.is_positive() {
                let room = around.expand2(SIZE);
                self.zoom = (rect.width() / room.width()).min(rect.height() / room.height()).clamp(*Self::ZOOM.start(), 1.5);
                self.pan = -room.center().to_vec2() * self.zoom;
                ui.ctx().request_repaint();
            }
        }
        // A node to be shown, or the folded group it is put away in, in the middle of the view,
        // from the next frame on.
        if let Some(item) = self.centre.take() {
            let shown = shot.folded_into(item.key(shot)).map_or(item, Item::Group);
            if let Some(at) = places.get(&shown) {
                self.pan = -at.to_vec2() * self.zoom;
                ui.ctx().request_repaint();
            }
        }
        // One node picked that gives a picture: an effect that is added goes below it.
        let below = picked.iter().next().filter(|_| picked.len() == 1).and_then(|key| item_of(shot, *key)).and_then(|item| Below::of(shot, item, &places));
        // The peg a Driver that is added goes on: the one selected, or picked alone, that has none.
        let driving = match selected {
            Some(Node::Peg(peg)) => Some(peg),
            _ => picked.iter().next().filter(|_| picked.len() == 1).map(|key| PegId(*key)),
        };
        let driving = driving.filter(|peg| shot.peg(*peg).is_some() && shot.driver_of(*peg).is_none());
        let mut boxes: Vec<(Item, Rect)> = {
            // The display last, then composites, pegs, layers and bones: the order they are drawn in.
            let composites = shot.composites.iter().map(|c| Item::Composite(c.id));
            let all = composites.chain(shot.pegs.iter().map(|p| Item::Peg(p.id))).chain(shot.layers.iter().map(|l| Item::Layer(l.id)));
            let all = all.chain(shot.deformers.iter().map(|d| Item::Deformer(d.id))).chain(shot.effects.iter().map(|e| Item::Effect(e.id)));
            // What is in a folded group is put away, and the group is a box in its place.
            let folded = shot.groups.iter().filter(|group| group.folded).map(|group| Item::Group(group.id));
            let all = all.chain(folded).filter(|item| shot.folded_into(item.key(shot)).is_none());
            let placed = |item: Item| Some((item, Rect::from_center_size(on_screen(*places.get(&item)?), SIZE * zoom)));
            let displays = std::iter::once(shot.main_display()).chain(shot.displays.iter().map(|display| display.id)).map(Item::Display);
            // A driver is put away with the peg it drives.
            let drivers = shot.drivers.iter().filter(|driver| shot.folded_into(driver.drives.0).is_none()).map(|driver| Item::Driver(driver.id));
            all.chain(drivers).chain([Item::Camera]).chain(displays).filter_map(placed).collect()
        };
        // The frames of the open groups that are seen, the widest first so that a group in a
        // group is drawn over it: round the boxes of what is in each, with room for its name.
        // A group's own ports are not in its frame but on it (plan §8.6): the ports in along
        // the top edge and the ports out along the bottom, in the order they were put in
        // from the left. Each is a small box that laps over the edge.
        let mut open: Vec<&iskra_model::Group> = shot.groups.iter().map(|group| &**group).filter(|group| !group.folded).collect();
        open.retain(|group| shot.folded_into(group.id.0).is_none());
        // The innermost first, so that a group round another has room for the inner one's ports.
        open.sort_by_key(|group| shot.group_nodes(group.id).len());
        let ports_of = |group: &iskra_model::Group, kind: &str| -> Vec<Item> {
            let port = |node: &u64| {
                let port = shot.effect(EffectId(*node)).filter(|effect| effect.kind == kind)?;
                Some((places.get(&Item::Effect(port.id)).map_or(0.0, |at| at.x), port.id))
            };
            let mut ports: Vec<(f32, EffectId)> = group.members.iter().filter_map(port).collect();
            ports.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            ports.into_iter().map(|(_, id)| Item::Effect(id)).collect()
        };
        let mut tabs: HashMap<Item, Rect> = HashMap::new();
        // The number each port has on its frame: 1, 2, 3 from the left, in and out apart.
        let mut numbers: HashMap<Item, usize> = HashMap::new();
        // Each frame, and the room it takes with its ports.
        let mut frames: Vec<(GroupId, Rect)> = Vec::new();
        let mut rooms: Vec<(GroupId, Rect)> = Vec::new();
        for group in open {
            let nodes = shot.group_nodes(group.id);
            let (ins, outs) = (ports_of(group, iskra_model::PORT_IN), ports_of(group, iskra_model::PORT_OUT));
            let own = |item: &Item| ins.contains(item) || outs.contains(item);
            let within =
                boxes.iter().filter(|(item, _)| nodes.contains(&item.key(shot)) && !matches!(item, Item::Display(_)) && !own(item)).map(|(_, rect)| *rect);
            let inner = rooms.iter().filter(|(inner, _)| nodes.contains(&inner.0)).map(|(_, room)| *room);
            let Some(around) = within.chain(inner).reduce(|all, rect| all.union(rect)) else { continue };
            let mut frame = Rect::from_min_max(around.min - vec2(ROOM.x, ROOM_ABOVE) * zoom, around.max + ROOM * zoom);
            // Wide enough for its ports side by side.
            let needed = (ins.len().max(outs.len()) as f32 * (TAB.x + 8.0) + 8.0) * zoom;
            if frame.width() < needed {
                frame = frame.expand2(vec2((needed - frame.width()) / 2.0, 0.0));
            }
            let mut room = frame;
            for (ports, top) in [(ins, true), (outs, false)] {
                let count = ports.len();
                for (index, port) in ports.into_iter().enumerate() {
                    let x = frame.left() + frame.width() * (index as f32 + 0.5) / count as f32;
                    let off = (TAB.y / 2.0 - LAP) * zoom;
                    let tab = Rect::from_center_size(pos2(x, if top { frame.top() - off } else { frame.bottom() + off }), TAB * zoom);
                    if let Some(drawn) = boxes.iter_mut().find(|(item, _)| *item == port) {
                        (drawn.1, room) = (tab, room.union(tab));
                        tabs.insert(port, tab);
                        numbers.insert(port, index + 1);
                    }
                }
            }
            frames.push((group.id, frame));
            rooms.push((group.id, room));
        }
        frames.sort_by(|a, b| b.1.area().total_cmp(&a.1.area()));
        let titles: Vec<(GroupId, Rect, Rect)> = frames
            .iter()
            .map(|(group, frame)| {
                // The strip along the top, all of its width: below where the ports lap over it.
                (*group, *frame, Rect::from_min_size(frame.min + vec2(6.0, LAP + 1.0) * zoom, vec2(frame.width() - 12.0 * zoom, 18.0 * zoom)))
            })
            .collect();
        // Each open group is a layer of its own, over what is in no group, layer 0: its frame,
        // which is on the ground's colour and hides what is under it, then what is in it and
        // in none of its open groups, then each of those groups the same way, over it. Groups
        // side by side are stacked as they were last taken hold of, the last on top
        // (`NodeView::stack`), and those not yet taken hold of the larger under the smaller.
        // Worked out once a frame: the innermost open group of every node, and each group's
        // place in the stack.
        let mut owner: HashMap<u64, (usize, GroupId)> = HashMap::new();
        for (group, _) in &frames {
            let nodes = shot.group_nodes(*group);
            for key in &nodes {
                let held = owner.entry(*key).or_insert((nodes.len(), *group));
                if nodes.len() < held.0 {
                    *held = (nodes.len(), *group);
                }
            }
        }
        let parent_of = |group: GroupId| owner.get(&group.0).map(|(_, parent)| *parent);
        let raised = |group: GroupId| self.stack.iter().position(|other| *other == group).map_or(0, |at| at + 1);
        let children = |of: Option<GroupId>| {
            let mut inside: Vec<(usize, GroupId)> =
                frames.iter().enumerate().filter(|(_, (group, _))| parent_of(*group) == of).map(|(at, (group, _))| (at, *group)).collect();
            inside.sort_by_key(|(at, group)| (raised(*group), *at));
            inside.into_iter().map(|(_, group)| group).rev().collect::<Vec<_>>()
        };
        let mut ranks: HashMap<GroupId, usize> = HashMap::new();
        let mut next = children(None);
        while let Some(group) = next.pop() {
            ranks.insert(group, ranks.len() + 1);
            next.extend(children(Some(group)));
        }
        let rank = |group: GroupId| ranks.get(&group).copied().unwrap_or(0);
        let layer = |item: Item| match item {
            Item::Display(_) => 0,
            _ => owner.get(&item.key(shot)).map_or(0, |(_, group)| rank(*group)),
        };
        boxes.sort_by_key(|(item, _)| layer(*item));
        let renamed = match &self.renaming {
            Some((Item::Group(renamed), _)) => Some(*renamed),
            _ => None,
        };
        // Of the colour the user gave the group, or else of its kind's.
        let frame_colour = |group: GroupId| node_colour(shot, Item::Group(group), &theme);
        let paint_frames = |painter: &egui::Painter, at: usize| {
            for (group, frame, title) in titles.iter().filter(|(group, ..)| rank(*group) == at) {
                // The frame of a group that is hidden is fainter, as what is in it is darker.
                let color = frame_colour(*group).gamma_multiply(if is_off(shot, Item::Group(*group)) { 0.45 } else { 1.0 });
                // Three quarters of the ground's colour: what is under the frame shows through faintly.
                painter.rect_filled(*frame, 8.0 * zoom, theme.surface.gamma_multiply(0.75));
                painter.rect(*frame, 8.0 * zoom, color.gamma_multiply(0.08), Stroke::new(1.0, color.gamma_multiply(0.7)), egui::StrokeKind::Inside);
                if renamed != Some(*group) {
                    let (name, font) = (Item::Group(*group).name(shot), egui::FontId::proportional((11.0 * zoom).max(6.0)));
                    painter.with_clip_rect(*title).text(title.left_center(), Align2::LEFT_CENTER, name, font, color);
                }
            }
        };
        #[cfg(test)]
        {
            self.frames = titles.clone();
            self.frame_colours = titles.iter().map(|(group, ..)| (*group, frame_colour(*group))).collect();
        }
        self.boxes = boxes.clone();
        #[cfg(test)]
        {
            self.dimmed.clear();
            self.outputs.clear();
            self.outlined.clear();
            self.floating = boxes.iter().map(|(item, _)| *item).filter(|item| layer(*item) > 0).collect();
        }
        // What is drawn over the frames, once the nodes that are in no group are drawn: the
        // wires that touch a node of a group, and the ports on them.
        let mut over_frames: BTreeMap<usize, Vec<egui::Shape>> = BTreeMap::new();
        let rect_of = |item: Item| boxes.iter().find(|(other, _)| *other == item).map(|(_, rect)| *rect);
        // On an open group's frame, what is drawn under it is out of reach: a box, a port or a
        // wire there is not taken, cut, gone onto nor let go on. What is on `layer` is under
        // the frames of the layers over it.
        let covered = |pos: Pos2, layer: usize| frames.iter().any(|(group, frame)| rank(*group) > layer && frame.contains(pos));
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let ctx = ui.ctx().clone();
        // Where to take what is drawn at `rect` from: nowhere while the pointer is on a frame
        // over it, unless it is being dragged already.
        let reach = |rect: Rect, id: egui::Id, layer: usize| match pointer.is_some_and(|pos| covered(pos, layer)) && !ctx.is_being_dragged(id) {
            true => Rect::from_min_size(rect.min, Vec2::ZERO),
            false => rect,
        };
        let under = |pos: Pos2, but: Item| {
            boxes.iter().rev().find(|(item, rect)| *item != but && rect.expand(4.0).contains(pos) && !covered(pos, layer(*item))).map(|(item, _)| *item)
        };

        // The wires, under the boxes. Each leaves the bottom of a box and arrives at a top.
        // While a node is dragged off its wire, the wires are those the shot will have: the
        // one that is joined behind it is marked, and drawn in another colour.
        let lifted = match (&self.drag, self.lifting) {
            (Some(Drag::Node(Item::Effect(id), _)), true) => {
                let mut without = shot.clone();
                without.take_off(*id);
                Some(without).filter(|without| without != shot)
            }
            (Some(Drag::Node(Item::Peg(id), _)), true) => {
                let mut without = shot.clone();
                without.take_off_peg(*id);
                Some(without).filter(|without| without != shot)
            }
            _ => None,
        };
        // The shot as it is to be when what is dragged is let go: off its chain or its wire, if
        // it is being taken off.
        let now = lifted.as_ref().unwrap_or(shot);
        let mut all = wires(lifted.as_ref().unwrap_or(shot), &places);
        if lifted.is_some() {
            let were: Vec<(Item, Item)> = wires(shot, &places).iter().map(|wire| (wire.from, wire.to)).collect();
            for wire in &mut all {
                wire.fresh = !were.contains(&(wire.from, wire.to));
            }
        }
        // A wire to or from what is put away in a folded group goes to the group's box; one
        // that is all inside it is not seen.
        let seen = |item: Item| shot.folded_into(item.key(shot)).map_or(item, Item::Group);
        // A folded group's box has the ports of what is in it (plan §8.6): its Port In nodes
        // side by side along the top, and its Port Out nodes along the bottom, in the order
        // they lie in from the left. This is how far along the box a port is, from 0 to 1.
        let along = |item: Item, kind: &str| -> Option<f32> {
            let (Item::Effect(id), Item::Group(group)) = (item, seen(item)) else { return None };
            shot.effect(id).filter(|effect| effect.kind == kind)?;
            let ports = shot.group_nodes(group).into_iter().filter_map(|node| {
                let port = shot.effect(iskra_model::EffectId(node)).filter(|effect| effect.kind == kind)?;
                Some((places.get(&Item::Effect(port.id)).map_or(0.0, |at| at.x), node))
            });
            let mut ports: Vec<(f32, u64)> = ports.collect();
            ports.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            let at = ports.iter().position(|(_, node)| *node == id.0)?;
            Some((at + 1) as f32 / (ports.len() + 1) as f32)
        };
        // A folded group's ports of a kind, each with how far along the box it is.
        let folded_ports = |group: GroupId, kind: &str| -> Vec<(EffectId, f32)> {
            let ports = shot.group_nodes(group).into_iter().map(EffectId);
            ports.filter_map(|port| Some((port, along(Item::Effect(port), kind).filter(|_| seen(Item::Effect(port)) == Item::Group(group))?))).collect()
        };
        // A folded group, by the box it is.
        let folded = |item: Item| match item {
            Item::Group(group) if shot.group(group).is_some_and(|group| group.folded) => Some(group),
            _ => None,
        };
        // What wires `from` into a folded group, let go on its box at `pos`: into the free
        // Port In nearest to there, or into a new one beside it, or above what is in the group.
        let into_folded = |from: (Item, Option<ArtLayerKind>), group: GroupId, pos: Pos2| -> Option<Action> {
            let onto = boxes.iter().find(|(item, _)| *item == Item::Group(group))?.1;
            let part = (pos.x - onto.left()) / onto.width().max(1.0);
            let mut ins = folded_ports(group, iskra_model::PORT_IN);
            ins.sort_by(|a, b| (a.1 - part).abs().total_cmp(&(b.1 - part).abs()));
            let free = |port: &EffectId| shot.effect(*port).is_some_and(|port| port.picture().is_none() && port.parent.is_none());
            match (ins.iter().find(|(port, _)| free(port)), ins.first()) {
                (Some((free, _)), _) => join(shot, from, Item::Effect(*free), 0, None),
                (None, Some((taken, _))) => {
                    let at = places.get(&Item::Effect(*taken)).map_or([0.0; 2], |at| [f64::from(at.x) + 1.0, f64::from(at.y)]);
                    new_port(shot, from, group, iskra_model::PORT_IN, at)
                }
                (None, None) => {
                    let inside = shot.group_nodes(group).into_iter().filter_map(|node| places.iter().find(|(item, _)| item.key(shot) == node));
                    let inside: Vec<Pos2> = inside.map(|(_, at)| *at).collect();
                    let x = inside.iter().map(|at| at.x).sum::<f32>() / inside.len().max(1) as f32;
                    let top = inside.iter().map(|at| at.y).fold(f32::INFINITY, f32::min);
                    let top = if top.is_finite() { top } else { 0.0 };
                    new_port(shot, from, group, iskra_model::PORT_IN, [f64::from(x), f64::from(top - DOWN * 0.6)])
                }
            }
        };
        for wire in &mut all {
            if seen(wire.to) != wire.to {
                let top = places.get(&seen(wire.to)).map_or(wire.end, |at| *at - vec2(0.0, SIZE.y / 2.0));
                wire.end = top + vec2(along(wire.to, iskra_model::PORT_IN).map_or(0.0, |part| (part - 0.5) * SIZE.x), 0.0);
            }
            wire.leaves = along(wire.from, iskra_model::PORT_OUT).or(wire.leaves);
            if (seen(wire.from), seen(wire.to)) != (wire.from, wire.to) {
                wire.was = Some((wire.from, wire.to));
            }
            (wire.from, wire.to) = (seen(wire.from), seen(wire.to));
            // A wire into a port on an open group's frame ends where the port is drawn.
            if let Some(tab) = tabs.get(&wire.to) {
                wire.end = ((tab.center_top() - origin) / zoom).to_pos2();
            }
        }
        // A wire all inside a folded group is not seen, but it is still a port's wire.
        let inside: Vec<(Item, Item)> = all.iter().filter(|wire| wire.from == wire.to).map(Wire::ends).collect();
        all.retain(|wire| wire.from != wire.to);
        let curve = |from: Pos2, to: Pos2| {
            let bend = vec2(0.0, ((to.y - from.y).abs() * 0.5).max(18.0 * zoom));
            [from, from + bend, to - bend, to]
        };
        let ends = |wire: &Wire| {
            let from = rect_of(wire.from)?;
            Some((pos2(from.left() + from.width() * wire.leaves.unwrap_or(0.5), from.bottom()), on_screen(wire.end)))
        };
        #[cfg(test)]
        {
            self.wire_ends = all.iter().filter_map(|wire| ends(wire).map(|(from, to)| (wire.from, wire.to, from, to))).collect();
        }
        let wire_color = |kind: Kind| match kind {
            Kind::Picture => theme.text_muted,
            Kind::Transform => Item::Peg(PegId(0)).color(&theme),
            Kind::Bend => Item::Deformer(DeformerId(0)).color(&theme),
            Kind::Read => Item::Peg(PegId(0)).color(&theme),
        };
        // Whether `pos` is on a wire, or within `reach` of it.
        let on_wire = |wire: &Wire, pos: Pos2, reach: f32| {
            let Some((from, to)) = ends(wire) else { return false };
            if covered(pos, layer(wire.from).max(layer(wire.to))) {
                return false;
            }
            let shape = CubicBezierShape::from_points_stroke(curve(from, to), false, Color32::TRANSPARENT, Stroke::NONE);
            // The curve as straight pieces: near any of them is near the wire.
            shape.flatten(Some(1.0)).windows(2).any(|piece| distance_to_piece(pos, piece[0], piece[1]) <= reach)
        };
        // The wire an effect node that is dragged alone is over with Alt held: let go so, the
        // node goes onto it; without Alt it is only moved, so a node dragged past a wire is not
        // wired in by mistake. Not for a port on a group's frame, which is only moved along
        // its edge.
        let alt = ui.input(|i| i.modifiers.alt);
        // Only a node that is wired to nothing goes onto a wire; one that is wired is wired by
        // hand. An effect that is taken off its wire as it is dragged is wired to nothing.
        let free = |item: Item| !all.iter().any(|wire| [wire.from, wire.to, wire.ends().0, wire.ends().1].contains(&item));
        let goes_onto = |item: Item, pos: Pos2, alone: bool| {
            if !(alone && alt && !tabs.contains_key(&item) && free(item)) {
                return None;
            }
            let takes = |wire: &Wire| {
                sits_on(now, item, wire.kind)
                    && on_wire(wire, pos, WIRE_REACH * 2.0)
                    && (matches!(item, Item::Effect(_)) || put_on(now, wire, item).is_some())
            };
            all.iter().position(takes)
        };
        let over = match (&self.drag, ui.input(|i| i.pointer.interact_pos())) {
            (Some(Drag::Node(item, _)), Some(pos)) => goes_onto(*item, pos, self.along.is_empty()),
            _ => None,
        };
        // A wire is drawn in its kind's colour; the one a node is about to go onto is lit,
        // and the one that is joined behind a node that comes off its wire is red.
        // The wire that leaves a node that is selected, or picked, is lit too: where what it
        // makes goes is seen at a glance.
        let chosen = |item: Item| match item {
            Item::Layer(id) => selected == Some(Node::Layer(id)),
            Item::Peg(id) => selected == Some(Node::Peg(id)),
            _ => false,
        } || picked.contains(&item.key(shot));
        for (index, wire) in all.iter().enumerate() {
            if let Some((from, to)) = ends(wire) {
                let stroke = match (over == Some(index), wire.fresh) {
                    (true, _) => Stroke::new(2.5, theme.accent),
                    (false, true) => Stroke::new(2.5, theme.danger),
                    (false, false) if chosen(wire.from) => Stroke::new(2.5, theme.accent),
                    (false, false) => Stroke::new(1.5, wire_color(wire.kind)),
                };
                let shape = CubicBezierShape::from_points_stroke(curve(from, to), false, Color32::TRANSPARENT, stroke);
                // What a driver reads, and its wire into its peg, are dashed: they carry nothing.
                let shapes: Vec<egui::Shape> = match wire.kind {
                    Kind::Read => egui::Shape::dashed_line(&shape.flatten(Some(1.0)), stroke, 5.0 * zoom.max(0.5), 4.0 * zoom.max(0.5)),
                    _ => vec![shape.into()],
                };
                match layer(wire.from).max(layer(wire.to)) {
                    0 => painter.extend(shapes),
                    deep => over_frames.entry(deep).or_default().extend(shapes),
                }
            }
        }
        // A small circle of a port, under the frames or over them as what it is on is.
        let mut dot = |on: Item, at: Pos2, fill: Color32, edge: Color32| {
            let shape = egui::Shape::Circle(egui::epaint::CircleShape { center: at, radius: 3.0 * zoom.max(0.7), fill, stroke: Stroke::new(1.0, edge) });
            match layer(on) {
                0 => drop(painter.add(shape)),
                deep => over_frames.entry(deep).or_default().push(shape),
            }
        };
        #[cfg(test)]
        {
            self.lit =
                all.iter().enumerate().filter(|(index, wire)| over == Some(*index) || wire.fresh).map(|(_, wire)| (wire.from, wire.to, wire.fresh)).collect();
            self.leaving_chosen = all.iter().filter(|wire| chosen(wire.from)).map(|wire| (wire.from, wire.to)).collect();
        }
        // A port where each wire arrives, in the wire's colour; and an empty one on the top
        // of what takes a picture and has none yet.
        for wire in &all {
            if rect_of(wire.to).is_some() {
                dot(wire.to, on_screen(wire.end), wire_color(wire.kind), theme.surface);
            }
            // And where one leaves a port of a folded group.
            if let (Some(_), Some((from, _))) = (wire.leaves, ends(wire)) {
                dot(wire.from, from, wire_color(wire.kind), theme.surface);
            }
        }
        for (item, node) in &boxes {
            let empty = match item {
                // A Receive takes no wire: it reads the Send it names.
                Item::Effect(id) => shot
                    .effect(*id)
                    .is_some_and(|effect| input_tabs(effect).is_empty() && top_port(effect).is_some_and(|port| !effect.inputs.contains_key(port))),
                Item::Composite(id) => shot.composite(*id).is_some_and(|composite| composite.inputs.is_empty()),
                Item::Display(id) => shot.shown_by(*id).flatten().is_none(),
                Item::Camera => shot.camera.parent.is_none(),
                _ => false,
            };
            if empty {
                dot(*item, node.center_top(), theme.surface, theme.text_muted);
            }
            // A matte port with nothing wired into it, on the left side.
            let no_matte = matches!(item, Item::Effect(id) if shot.effect(*id).is_some_and(|effect| {
                input_tabs(effect).is_empty() && side_ports(effect).iter().any(|port| !effect.inputs.contains_key(*port))
            }));
            // Inputs along the top at its left: one with nothing wired shows so, and each says
            // what it takes.
            if let Item::Effect(id) = item
                && let Some(effect) = shot.effect(*id)
            {
                for (port, part, hint) in input_tabs(effect) {
                    let at = pos2(node.left() + node.width() * part, node.top());
                    if !effect.inputs.contains_key(port) {
                        dot(*item, at, theme.surface, theme.text_muted);
                    }
                    let near = Rect::from_center_size(at, vec2(12.0, 12.0) * zoom.max(0.7));
                    let input = response.id.with(("input", id.0, port));
                    ui.interact(reach(near, input, layer(*item)), input, Sense::hover()).on_hover_text(hint);
                }
            }
            if no_matte {
                dot(*item, node.left_center(), theme.surface, theme.text_muted);
            }
            // The start and end ports of a chain or a curve, at the left of its top, where
            // nothing is wired in.
            if let Item::Deformer(id) = item
                && has_ports(shot, *id)
                && let Some(chain) = shot.deformer(*id)
            {
                let (start, end) = if chain.is_curve() {
                    (
                        "Start: a peg wired in here carries the curve's first point, as the body carries a limb",
                        "End: the curve's last point goes where the pivot of a peg wired in here is",
                    )
                } else {
                    (
                        "Start: a peg wired in here carries the first bone, as the body carries a limb; the bones turn with it",
                        "End: a peg wired in here is the goal of the bones' IK, which their end reaches for",
                    )
                };
                for (peg, part, hint) in [(chain.start_peg, START_PORT, start), (end_peg(chain), END_PORT, end)] {
                    let at = pos2(node.left() + node.width() * part, node.top());
                    if peg.is_none() {
                        dot(*item, at, theme.surface, theme.text_muted);
                    }
                    let near = Rect::from_center_size(at, vec2(12.0, 12.0) * zoom.max(0.7));
                    let port = response.id.with(("port", id.0, part.to_bits()));
                    ui.interact(reach(near, port, layer(*item)), port, Sense::hover()).on_hover_text(hint);
                }
            }
        }
        // Alt-click on a wire takes it away.
        if response.clicked()
            && ui.input(|i| i.modifiers.alt)
            && let Some(pos) = response.interact_pointer_pos()
        {
            actions.extend(all.iter().find(|wire| on_wire(wire, pos, WIRE_REACH)).and_then(|wire| cut_last(shot, wire, &all, &inside)));
        } else if response.clicked() {
            // A click on the background lets all go: what is picked, and what is selected.
            actions.push(Action::Pick(Vec::new()));
        }
        // A drag on the background pulls out a box, and what it touches is picked: with
        // Shift held, beside what is picked already.
        if response.dragged_by(egui::PointerButton::Primary) && !panning {
            if let (Some(from), Some(to)) = (ui.input(|i| i.pointer.press_origin()), response.interact_pointer_pos()) {
                if self.marquee.is_none() {
                    self.marquee_shift = Vec2::ZERO;
                }
                self.marquee = Some((from + self.marquee_shift, to));
            }
        } else if let Some((from, to)) = self.marquee.take() {
            let pulled = Rect::from_two_pos(from, to);
            let touched = boxes.iter().filter(|(_, rect)| rect.intersects(pulled)).map(|(item, _)| item.key(shot));
            let kept = picked.iter().copied().filter(|_| ui.input(|i| i.modifiers.shift));
            actions.push(Action::Pick(kept.chain(touched).collect()));
        }
        // The layer that is selected, for the bones that bend it.
        let bones_selected = match selected {
            Some(Node::Layer(id)) => shot.layer(id).and_then(|layer| layer.deformer),
            _ => None,
        };
        // Where a node that is added here goes: where the menu was opened, in the graph.
        let here = |pos: Option<Pos2>| pos.map(|pos| (pos - origin) / zoom).map(|at| [f64::from(at.x), f64::from(at.y)]);
        // Where the menu was opened, which is not where the pointer is once it is on the menu.
        if response.secondary_clicked() {
            self.menu_at = response.interact_pointer_pos();
        }
        let opened = self.menu_at.or(response.interact_pointer_pos()).or(response.hover_pos());
        // And the group it goes into: the open one whose frame it is added over, the innermost.
        let into = |pos: Option<Pos2>| frames.iter().rev().find(|(_, frame)| pos.is_some_and(|pos| frame.contains(pos))).map(|(group, _)| group.0);
        // A click on the background, or its menu opened, is where nodes are pasted next.
        if response.clicked() || response.secondary_clicked() {
            let at = response.interact_pointer_pos();
            self.paste_at = here(at).map(|place| (shot.id, place, into(at)));
        }
        response.context_menu(|ui| {
            let hint = "The nodes that were copied, here and in the open group here, wired to nothing";
            if ui.add_enabled(self.copied, egui::Button::new("Paste")).on_hover_text(hint).clicked() {
                actions.push(Action::Command("edit.paste"));
                ui.close();
            }
            ui.separator();
            for (title, id) in [("New Layer", "layer.new"), ("New Peg", "peg.new"), ("New Composite", "composite.new")] {
                if ui.button(title).clicked() {
                    actions.push(Action::Run(id, json!({ "shot": shot.id.0, "group": into(opened), "at": here(opened) })));
                    ui.close();
                }
                if id == "layer.new" && ui.button("New Text Layer").clicked() {
                    actions.push(Action::Run(id, json!({ "shot": shot.id.0, "group": into(opened), "at": here(opened), "kind": "text" })));
                    ui.close();
                }
            }
            let hint = "Another picture of the shot, for the viewer: the View menu switches between the displays";
            if ui.button("New Display").on_hover_text(hint).clicked() {
                actions.push(Action::Run("display.new", json!({ "shot": shot.id.0, "at": here(opened) })));
                ui.close();
            }
            let hint = "What is wired into it is rendered apart on the Export page, under its name: name it as the same pass is named in the other shots";
            if ui.button("New Pass").on_hover_text(hint).clicked() {
                actions.push(Action::Run("display.new", json!({ "shot": shot.id.0, "at": here(opened), "pass": true })));
                ui.close();
            }
            ui.separator();
            // How the nodes that were never put anywhere are arranged.
            ui.menu_button("Arrange", |ui| {
                let hint = "A node stays where it is. One that is new is laid out once, where the layout would have it";
                if ui.radio(!self.arrange, "By Hand").on_hover_text(hint).clicked() {
                    self.arrange = false;
                    ui.close();
                }
                let hint = "The nodes that were never dragged anywhere are laid out anew whenever the graph changes";
                if ui.radio(self.arrange, "By the View").on_hover_text(hint).clicked() {
                    self.arrange = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Arrange Now").on_hover_text("Lay the nodes that were never dragged anywhere out anew, once").clicked() {
                    self.kept.1.clear();
                    ui.close();
                }
            });
            ui.separator();
            // Every kind of effect node, by its category (plan §8.3).
            let title = below.as_ref().map_or("Add Node".to_string(), |below| format!("Add Node below {}", below.name));
            ui.menu_button(title, |ui| {
                if ui.button("Search…").on_hover_text("Find a node by its name. Enter over the node view opens this too, when nothing is picked").clicked() {
                    self.search = opened.map(|at| (String::new(), at, 0));
                    ui.close();
                }
                ui.separator();
                // A Driver goes on a peg: the one selected, or picked alone, that has none.
                // The nodes that are no effects, in their categories, before the kinds of effect.
                let structure_in = |ui: &mut egui::Ui, category: &str, actions: &mut Vec<Action>| {
                    for (title, command, hint, _) in STRUCTURE.iter().filter(|(.., of)| *of == category) {
                        if ui.button(*title).on_hover_text(*hint).clicked() {
                            actions.push(structure_made(shot, command, here(opened), into(opened)));
                            ui.close();
                        }
                    }
                };
                ui.menu_button("Move", |ui| {
                    structure_in(ui, "Move", &mut actions);
                    let peg = driving;
                    let hint = match peg.and_then(|peg| shot.peg(peg)) {
                        Some(peg) => format!("A Driver of {}: expressions over other pegs that add to its position, rotation and scale", peg.name),
                        None => "A Driver goes on a peg: select a peg that has none first".to_string(),
                    };
                    let button = ui.add_enabled(peg.is_some(), egui::Button::new("Driver")).on_hover_text(hint.clone()).on_disabled_hover_text(hint);
                    if let (true, Some(peg)) = (button.clicked(), peg) {
                        actions.push(Action::Run("driver.new", json!({ "peg": peg.0, "at": here(opened) })));
                        ui.close();
                    }
                });
                for category in categories() {
                    ui.menu_button(category, |ui| {
                        structure_in(ui, category, &mut actions);
                        if STRUCTURE.iter().any(|(.., of)| *of == category) {
                            ui.separator();
                        }
                        for kind in effect_kinds().iter().filter(|kind| kind.category == category && !is_port_kind(kind.id)) {
                            if ui.button(kind.title).on_hover_text(kind.hint).clicked() {
                                actions.push(added(shot, kind, below.as_ref(), here(opened), into(opened)));
                                ui.close();
                            }
                        }
                    });
                }
            });
        });
        // Enter over the view opens the search that adds a node: with nothing picked, or with
        // the one node it goes below, or the peg a Driver would go on.
        if self.search.is_none()
            && (picked.is_empty() || below.is_some() || driving.is_some())
            && self.renaming.is_none()
            && response.hovered()
            && !ui.ctx().text_edit_focused()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            self.search = response.hover_pos().map(|at| (String::new(), at, 0));
        }
        self.search_window(ui, shot, here, into, below.as_ref(), driving, &mut actions);

        // The boxes: those in no group first, then layer by layer a frame with what goes over
        // it, and the boxes of its group.
        let deepest = ranks.len();
        let mut drawn = 0;
        let mut up_to = |at: usize, painter: &egui::Painter, over_frames: &mut BTreeMap<usize, Vec<egui::Shape>>| {
            while drawn < at {
                drawn += 1;
                paint_frames(painter, drawn);
                painter.extend(over_frames.remove(&drawn).unwrap_or_default());
            }
        };
        #[cfg(test)]
        self.tags_said.clear();
        for (item, node) in &boxes {
            let (item, node) = (*item, *node);
            up_to(layer(item), &painter, &mut over_frames);
            let id = response.id.with(("node", item));
            let body = ui.interact(reach(node, id, layer(item)), id, Sense::click_and_drag());
            // Taken hold of, a node's groups come to the top of those beside them.
            if body.clicked() || body.drag_started() || body.secondary_clicked() {
                raise(&mut self.stack, shot, item.key(shot));
            }
            let is_selected = match item {
                Item::Layer(id) => selected == Some(Node::Layer(id)),
                Item::Peg(id) => selected == Some(Node::Peg(id)),
                Item::Deformer(id) => bones_selected == Some(id),
                Item::Composite(_) | Item::Group(_) | Item::Effect(_) | Item::Display(_) | Item::Camera | Item::Driver(_) => false,
            } || picked.contains(&item.key(shot));
            #[cfg(test)]
            if is_selected {
                self.outlined.push(item);
            }
            // A node that is hidden on the timeline, or an effect that is switched off, is
            // darker: its box is of the ground's colour, and its band and its name are faint.
            let off = is_off(shot, item);
            #[cfg(test)]
            if off {
                self.dimmed.push(item);
            }
            painter.rect(
                node,
                5.0 * zoom,
                if off { theme.surface } else { theme.surface_raised },
                Stroke::new(if is_selected { 2.0 } else { 1.0 }, if is_selected { theme.accent } else { theme.border }),
                egui::StrokeKind::Inside,
            );
            // A band of the kind's colour down the left side.
            let band = Rect::from_min_max(node.min, pos2(node.min.x + 5.0 * zoom, node.max.y)).shrink(1.0);
            // Of the colour the user gave the node, or else of its kind's.
            let colour = node_colour(shot, item, &theme);
            painter.rect_filled(band, 3.0 * zoom, if off { colour.gamma_multiply(0.35) } else { colour });
            // An effect that is switched off says so.
            let switched_off = matches!(item, Item::Effect(id) if shot.effect(id).is_some_and(|effect| !effect.enabled));
            let failing = |item: Item| match item {
                Item::Driver(id) => shot.driver(id).is_some_and(|driver| !driver.off && shot.amounts(driver, frame).failed),
                _ => false,
            };
            if let Some((_, name)) = self.renaming.as_mut().filter(|(renamed, _)| *renamed == item) {
                let mut inside = ui.new_child(egui::UiBuilder::new().max_rect(node.shrink2(vec2(8.0, 3.0))));
                let edit = inside.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                if edit.lost_focus() {
                    // Enter keeps the new name; Escape, or the same name, changes nothing.
                    let keep = !inside.input(|i| i.key_pressed(egui::Key::Escape)) && name.as_str() != item.name(shot);
                    if let Some((_, name)) = self.renaming.take().filter(|_| keep) {
                        actions.extend(match item {
                            Item::Composite(id) => Some(Action::Run("composite.rename", json!({ "composite": id.0, "name": name }))),
                            Item::Deformer(id) => Some(Action::Run("deformer.rename", json!({ "deformer": id.0, "name": name }))),
                            Item::Group(id) => Some(Action::Run("group.rename", json!({ "group": id.0, "name": name }))),
                            Item::Effect(id) => Some(Action::Run("effect.rename", json!({ "effect": id.0, "name": name }))),
                            Item::Display(id) => Some(Action::Run("display.rename", json!({ "display": id.0, "name": name }))),
                            Item::Layer(id) => Some(Action::Run("layer.rename", json!({ "layer": id.0, "name": name }))),
                            Item::Peg(id) => Some(Action::Run("peg.rename", json!({ "peg": id.0, "name": name }))),
                            Item::Driver(id) => Some(Action::Run("driver.rename", json!({ "driver": id.0, "name": name }))),
                            Item::Camera => None,
                        });
                    }
                } else {
                    edit.request_focus();
                }
            } else {
                let font = egui::FontId::proportional((12.0 * zoom).max(6.0));
                let written = if off { theme.text_muted } else { theme.text };
                // A port on a group's frame is its number there, unless it was given a name of
                // its own.
                let number = numbers.get(&item).filter(|_| !named_port(shot, item)).map(|number| number.to_string());
                let said = number.as_deref().unwrap_or_else(|| item.name(shot));
                painter.with_clip_rect(node).text(node.center() + vec2(2.0 * zoom, 0.0), Align2::CENTER_CENTER, said, font, written);
                if switched_off {
                    let small = egui::FontId::proportional((9.0 * zoom).max(5.0));
                    painter.text(node.right_top() + vec2(-5.0, 3.0) * zoom, Align2::RIGHT_TOP, "off", small, theme.text_muted);
                }
                // A driver whose expressions come to no number on the frame that is showing.
                if failing(item) {
                    let small = egui::FontId::proportional((11.0 * zoom).max(6.0));
                    #[cfg(test)]
                    self.tags_said.push((item, "!".to_string()));
                    painter.text(node.right_top() + vec2(-5.0, 2.0) * zoom, Align2::RIGHT_TOP, "!", small, theme.danger);
                }
                // A Pass says so under its box.
                if matches!(item, Item::Display(id) if shot.passes().any(|pass| pass.id == id)) {
                    let small = egui::FontId::proportional((9.0 * zoom).max(5.0));
                    #[cfg(test)]
                    self.tags_said.push((item, "pass".to_string()));
                    painter.text(node.center_bottom() + vec2(0.0, 2.0) * zoom, Align2::CENTER_TOP, "pass", small, theme.colour(Role::Pass));
                }
                // The main display says so under its box, when the shot has others.
                if item == Item::Display(shot.main_display()) && shot.looks().next().is_some() {
                    let small = egui::FontId::proportional((9.0 * zoom).max(5.0));
                    #[cfg(test)]
                    self.tags_said.push((item, "main".to_string()));
                    painter.text(node.center_bottom() + vec2(0.0, 2.0) * zoom, Align2::CENTER_TOP, "★ main", small, theme.accent);
                }
                // A Send and a Receive say their tag under the box: the same tag goes together.
                let tag = match item {
                    Item::Effect(id) => shot.effect(id).filter(|effect| [iskra_model::SEND, iskra_model::RECEIVE].contains(&effect.kind.as_str())),
                    _ => None,
                };
                if let Some(effect) = tag {
                    let (tag, small) = (effect.text(iskra_model::TAG), egui::FontId::proportional((9.0 * zoom).max(5.0)));
                    // Of Sends that have one tag the first is read: each says so, in red.
                    let (said, colour) = match shot.tag_shared(effect) {
                        Some(first) if first.id == effect.id => (format!("tag: {tag}, read here; another Send has it too"), theme.danger),
                        Some(first) => (format!("tag: {tag}, not read: {} has it first", first.name), theme.danger),
                        None if tag.is_empty() => ("no tag".to_string(), theme.text_muted),
                        None => (format!("tag: {tag}"), theme.text_muted),
                    };
                    #[cfg(test)]
                    self.tags_said.push((item, said.clone()));
                    painter.text(node.left_bottom() + vec2(8.0, 2.0) * zoom, Align2::LEFT_TOP, said, small, colour);
                }
                // A composite says how it orders what it stacks.
                if let Some(composite) = match item {
                    Item::Composite(id) => shot.composite(id),
                    _ => None,
                } {
                    let small = egui::FontId::proportional((9.0 * zoom).max(5.0));
                    // Short, and to the right, clear of the port in the middle that it leaves by.
                    let said = ordering_title(composite).replace(", ", " ").replace(" on Top", "");
                    #[cfg(test)]
                    self.tags_said.push((item, said.clone()));
                    painter.text(node.right_bottom() + vec2(-8.0, 2.0) * zoom, Align2::RIGHT_TOP, said, small, theme.text_muted);
                }
            }

            // With Alt held, a click on a box that is not picked wires all that is picked into
            // it, as one step: from the left to the right, after what arrives there already.
            // A peg that is clicked, or a chain of bones, is wired the other way, into each of
            // the picked: it is what carries them, or bends them.
            if body.clicked() && ui.input(|i| i.modifiers.alt) && !picked.is_empty() && !picked.contains(&item.key(shot)) {
                let mut from: Vec<(Item, f32)> =
                    picked.iter().filter_map(|key| item_of(shot, *key)).filter_map(|from| Some((from, places.get(&from)?.x))).collect();
                from.sort_by(|a, b| a.1.total_cmp(&b.1));
                let arriving = all.iter().filter(|wire| wire.to == item).count();
                let down = matches!(item, Item::Peg(_) | Item::Deformer(_));
                let joined = |nth: usize, from: Item| {
                    if down { join(shot, (item, None), from, 0, None) } else { join(shot, (from, None), item, arriving + nth, None) }
                };
                let wired = from.iter().enumerate().filter_map(|(nth, (from, _))| match joined(nth, *from)? {
                    Action::Run(id, params) => Some(json!({ "id": id, "params": params })),
                    _ => None,
                });
                let commands: Vec<Value> = wired.collect();
                if !commands.is_empty() {
                    actions.push(Action::Run("edit.batch", json!({ "commands": commands, "title": "Connect Nodes" })));
                }
            } else if body.clicked() && ui.input(|i| i.modifiers.shift) {
                let mut all = picked.clone();
                if !all.remove(&item.key(shot)) {
                    all.insert(item.key(shot));
                }
                actions.push(Action::Pick(all.into_iter().collect()));
            } else if body.clicked() {
                // A node that is not a layer or a peg is picked alone; those are selected,
                // which picks them. Either way what was outlined before no longer is.
                if !matches!(item, Item::Layer(_) | Item::Peg(_)) {
                    actions.push(Action::Pick(vec![item.key(shot)]));
                }
                match item {
                    Item::Layer(id) => actions.push(Action::Select { node: Some(Node::Layer(id)), frame }),
                    Item::Peg(id) => actions.push(Action::Select { node: Some(Node::Peg(id)), frame }),
                    // Bones are of the layer they bend: the first of them is selected.
                    Item::Deformer(id) => {
                        let bent = shot.layers.iter().find(|layer| layer.deformer == Some(id));
                        actions.extend(bent.map(|layer| Action::Select { node: Some(Node::Layer(layer.id)), frame }));
                    }
                    // The camera is in hand with its tool, which Properties then shows it by.
                    Item::Camera => actions.push(Action::Command("tool.camera")),
                    Item::Composite(_) | Item::Group(_) | Item::Effect(_) | Item::Display(_) | Item::Driver(_) => {}
                }
            }
            // A double click edits the name: of any node but a group, which it opens, the camera,
            // whose name is its kind, and a board locked as reference.
            let named = match item {
                Item::Layer(id) => !self.locked.contains(&id),
                Item::Peg(_) | Item::Composite(_) | Item::Deformer(_) | Item::Effect(_) | Item::Display(_) | Item::Driver(_) => true,
                Item::Group(_) | Item::Camera => false,
            };
            if body.double_clicked() && named {
                self.renaming = Some((item, item.name(shot).to_string()));
            }
            // A double click on a folded group's box opens it.
            if let (true, Item::Group(id)) = (body.double_clicked(), item) {
                actions.push(Action::Run("group.set", json!({ "group": id.0, "folded": false })));
            }
            // Dragging the box moves it; letting go keeps the place with the shot.
            // One of several that are picked takes the others along.
            let several = picked.contains(&item.key(shot)) && picked.len() > 1;
            if body.drag_started() {
                self.drag = places.get(&item).map(|at| Drag::Node(item, *at));
                // With Alt held, an effect node or a peg that is dragged alone comes off its
                // wire or its chain.
                self.lifting = !several && matches!(item, Item::Effect(_) | Item::Peg(_)) && !tabs.contains_key(&item) && ui.input(|i| i.modifiers.alt);
                let others = boxes.iter().map(|(other, _)| *other).filter(|other| several && *other != item && picked.contains(&other.key(shot)));
                self.along = carried(shot, item, others).into_iter().filter_map(|other| Some((other, *places.get(&other)?))).collect();
            }
            if let (true, Some(Drag::Node(dragged, at))) = (body.dragged(), &mut self.drag)
                && *dragged == item
            {
                *at += body.drag_delta() / zoom;
                for (_, at) in &mut self.along {
                    *at += body.drag_delta() / zoom;
                }
            }
            // Dragged out of the view, a node is on its way to the library: it stays where it
            // was here, and the app saves it if the Library panel is where it is let go.
            let outside = body.interact_pointer_pos().filter(|pos| !rect.contains(*pos)).and_then(|pos| Some((pos, item.part()?)));
            if body.dragged() && outside.is_some() {
                crate::drag_label(ui.ctx(), item.name(shot));
            }
            if body.drag_stopped()
                && let Some(Drag::Node(dragged, at)) = self.drag.take_if(|drag| matches!(drag, Drag::Node(dragged, _) if *dragged == item))
            {
                let along = std::mem::take(&mut self.along);
                // A node wired to nothing, let go with Alt held on a wire it can sit on, goes
                // onto that wire.
                let dropped = body.interact_pointer_pos().filter(|_| !several);
                let onto = dropped.and_then(|pos| all.get(goes_onto(item, pos, true)?));
                let put = onto.and_then(|wire| match item {
                    Item::Effect(effect) => Some(Action::Run("effect.putOn", json!({ "effect": effect.0, "on": wire.source()? }))),
                    // A peg taken off its chain is taken off it first, in the same step.
                    Item::Peg(peg) if self.lifting && lifted.is_some() => match put_on(now, wire, item)? {
                        Action::Run(_, params) => {
                            let off = json!({ "id": "peg.takeOff", "params": { "peg": peg.0 } });
                            let commands: Vec<Value> = [off].into_iter().chain(params["commands"].as_array()?.iter().cloned()).collect();
                            Some(Action::Run("edit.batch", json!({ "commands": commands, "title": "Put Node on a Wire" })))
                        }
                        _ => None,
                    },
                    _ => put_on(now, wire, item),
                });
                let onto = put.is_some().then_some(());
                actions.extend(put);
                // One that was dragged off its wire, and let go on no other, is off it.
                let lifted = std::mem::take(&mut self.lifting) && onto.is_none() && lifted.is_some();
                actions.push(match outside {
                    Some((pos, _)) if several => Action::Save { parts: parts_of(shot, picked), at: Some(pos) },
                    Some((pos, part)) => Action::Save { parts: vec![part], at: Some(pos) },
                    None if lifted && matches!(item, Item::Peg(_)) => Action::Run("peg.takeOff", json!({ "peg": item.key(shot), "at": [at.x, at.y] })),
                    None if lifted => Action::Run("effect.takeOff", json!({ "effect": item.key(shot), "at": [at.x, at.y] })),
                    None => {
                        let moves: Vec<Value> =
                            [(dragged, at)].into_iter().chain(along).map(|(item, at)| json!({ "node": item.key(shot), "at": [at.x, at.y] })).collect();
                        Action::Run("node.place", json!({ "moves": moves }))
                    }
                });
            }
            body.context_menu(|ui| self.node_menu(ui, shot, item, picked, &places, &mut actions));
            if let Some(template) = item.template(shot) {
                body.on_hover_text(format!("A copy from the library template “{template}”"));
            } else if let Item::Driver(id) = item
                && let Some(driver) = shot.driver(id)
            {
                let peg = shot.peg(driver.drives).map_or("its peg", |peg| peg.name.as_str());
                let mut said = match driver.expressions.text().trim() {
                    "" => format!("A driver of {peg}, with no expressions yet"),
                    text => format!("A driver of {peg}: what it gives is added to the peg's own values\n\n{text}"),
                };
                if !driver.off && shot.amounts(driver, frame).failed {
                    said.push_str("\n\n! On this frame a value it gives comes to no number, and adds nothing: a division by zero, or an input with no peg");
                }
                body.on_hover_text(said);
            } else if let Item::Deformer(_) = item {
                body.on_hover_text(
                    "Bones: they bend the drawings of the layers they are wired to. Drag from the port under them to a layer to bend that one too, \
                     or to a peg to have it follow the bones' end without being bent. Of the two ports at the left of their top, a peg wired into the first carries their start, as the body carries a limb; one in the second is their end's goal",
                );
            }

            // The port under the box: a wire is dragged out of it. The display has none, nor
            // has the camera. A folded group's box has one for each of its Port Out nodes,
            // where their wires leave it, and the wire is dragged out of that node.
            // A layer's box has an output for each of its art layers too, to the left of its
            // flat one, where its Properties show them: a wire from one of them gives that art
            // layer of the drawings alone. One that carries a wire is always shown.
            let sources: Vec<(Item, Pos2, Option<ArtLayerKind>)> = match item {
                Item::Display(_) | Item::Camera | Item::Driver(_) => continue,
                Item::Group(group) => {
                    let out = folded_ports(group, iskra_model::PORT_OUT).into_iter();
                    out.map(|(port, part)| (Item::Effect(port), pos2(node.left() + node.width() * part, node.bottom()), None)).collect()
                }
                Item::Layer(id) => {
                    let ports = shot.layer(id).is_some_and(|layer| layer.art_ports);
                    let wired = |kind: ArtLayerKind| all.iter().any(|wire| wire.ends().0 == item && wire.art == Some(kind));
                    let arts = ART_PORTS.iter().filter(|(kind, ..)| ports || wired(*kind));
                    let arts = arts.map(|(kind, part, _)| (item, pos2(node.left() + node.width() * part, node.bottom()), Some(*kind)));
                    std::iter::once((item, node.center_bottom(), None)).chain(arts).collect()
                }
                _ => vec![(item, node.center_bottom(), None)],
            };
            let boxed = item;
            #[cfg(test)]
            self.outputs.extend(sources.iter().map(|(item, at, art)| (*item, *art, *at)));
            for (item, start, art) in sources {
                let port = Rect::from_center_size(start, vec2(if art.is_some() { 12.0 } else { 16.0 }, 12.0) * zoom.max(0.7));
                let grip = id.with(("port", item, art));
                let grip = ui.interact(reach(port, grip, layer(boxed)), grip, Sense::drag()).on_hover_cursor(egui::CursorIcon::Crosshair);
                // Under a folded group's box, which port it is; under a layer's, which art layer.
                let named = ART_PORTS.iter().find(|(kind, ..)| Some(*kind) == art).map(|(.., hint)| *hint);
                let grip = match (named, item != boxed) {
                    (Some(hint), _) => grip.on_hover_text(hint),
                    (None, true) => grip.on_hover_text(item.name(shot)),
                    (None, false) => grip,
                };
                let fill = if grip.hovered() { theme.accent } else { boxed.color(&theme) };
                // An art layer's output is a small square, the flat one a dot.
                let edge = Stroke::new(1.0, theme.surface);
                match art {
                    Some(_) => painter.rect(Rect::from_center_size(start, Vec2::splat(5.0 * zoom.max(0.7))), 1.0, fill, edge, egui::StrokeKind::Middle),
                    None => painter.circle(start, 3.5 * zoom.max(0.7), fill, edge),
                };
                if grip.drag_started() {
                    self.drag = Some(Drag::Wire(item, art));
                }
                let wiring = matches!(self.drag, Some(Drag::Wire(from, out)) if from == item && out == art);
                if let (true, Some(pos)) = (wiring, grip.interact_pointer_pos()) {
                    // Not into the folded group it leaves.
                    let target = under(pos, item).filter(|target| *target != boxed);
                    // The edge of an open group's frame the wire is over, where no box is: let go
                    // there, it makes a port of the group, in on the top edge and out on the bottom.
                    let on_edge = |(group, frame): &(GroupId, Rect)| {
                        let reach = 9.0 * zoom.max(0.7);
                        let within = pos.x >= frame.left() && pos.x <= frame.right();
                        let kind = if (pos.y - frame.top()).abs() <= reach {
                            iskra_model::PORT_IN
                        } else if (pos.y - frame.bottom()).abs() <= reach {
                            iskra_model::PORT_OUT
                        } else {
                            return None;
                        };
                        within.then_some((*group, kind))
                    };
                    let edge = frames.iter().rev().find_map(on_edge).filter(|_| target.is_none());
                    // A port that carries something already takes no second wire: another port is
                    // made beside it, as a composite makes room for another input.
                    let beside = target.filter(|port| tabs.contains_key(port)).and_then(|port| {
                        let Item::Effect(id) = port else { return None };
                        let taken = shot.effect(id).filter(|taken| taken.picture().is_some() || taken.parent.is_some())?;
                        let same =
                            taken.picture() == source_of(item, art) && item.source().is_some() || matches!(item, Item::Peg(peg) if taken.parent == Some(peg));
                        let at = places.get(&port)?;
                        (!same).then_some((shot.group_of(id.0)?.id, taken.kind.clone(), [f64::from(at.x) + 1.0, f64::from(at.y)]))
                    });
                    painter.add(CubicBezierShape::from_points_stroke(
                        curve(start, pos),
                        false,
                        Color32::TRANSPARENT,
                        Stroke::new(2.0, if target.is_some() || edge.is_some() { theme.accent } else { wire_color(kind_from(item)) }),
                    ));
                    if let (true, Some(group)) = (grip.drag_stopped(), target.and_then(folded)) {
                        self.drag = None;
                        actions.extend(into_folded((item, art), group, pos));
                    } else if let (true, Some((group, kind, at))) = (grip.drag_stopped(), &beside) {
                        self.drag = None;
                        actions.extend(new_port(shot, (item, art), *group, kind, *at));
                    } else if let (true, Some((group, kind))) = (grip.drag_stopped(), edge) {
                        self.drag = None;
                        let at = (pos - origin) / zoom;
                        actions.extend(new_port(shot, (item, art), group, kind, [f64::from(at.x), f64::from(at.y)]));
                    } else if grip.drag_stopped() {
                        self.drag = None;
                        // Among a composite's inputs it goes where it was dropped: before those
                        // that arrive to the right of it.
                        let index = all.iter().filter(|wire| Some(wire.to) == target && wire.out() != (item, art) && on_screen(wire.end).x < pos.x).count();
                        let across = target.and_then(rect_of).map(|onto| (pos.x - onto.left()) / onto.width());
                        actions.extend(target.and_then(|to| join(shot, (item, art), to, index, across)));
                    }
                } else if wiring && grip.drag_stopped() {
                    self.drag = None;
                }
            }
        }
        up_to(deepest, &painter, &mut over_frames);
        // The end of a wire on a composite is dragged along the composite's ports to another
        // place among them, which changes what is over what (plan §8.3).
        for wire in all.iter().filter(|wire| wire.kind == Kind::Picture) {
            // From a folded group's box, the wire is its Port Out's, which is what is stacked.
            let (Item::Composite(composite), Some(onto), Some(node)) = (wire.to, rect_of(wire.to), wire.source()) else { continue };
            let others: Vec<f32> =
                all.iter().filter(|other| other.to == wire.to && other.leaves_from() != wire.leaves_from()).map(|other| on_screen(other.end).x).collect();
            if others.is_empty() {
                continue;
            }
            let end = on_screen(wire.end);
            let grip = Rect::from_center_size(end, vec2(12.0, 12.0) * zoom.max(0.7));
            let id = response.id.with(("end", wire.leaves_from(), composite.0));
            let held = ui.interact(reach(grip, id, layer(wire.from).max(layer(wire.to))), id, Sense::drag());
            let hint = "Drag along the ports to change what is over what, onto another node to wire it there, or off onto nothing to take the wire away";
            let held = held.on_hover_cursor(egui::CursorIcon::ResizeHorizontal).on_hover_text(hint);
            if held.drag_started() {
                self.drag = Some(Drag::End(wire.leaves_from(), composite));
            }
            if !matches!(self.drag, Some(Drag::End(from, to)) if from == wire.leaves_from() && to == composite) {
                continue;
            }
            let Some(pos) = held.interact_pointer_pos() else { continue };
            painter.circle(pos2(pos.x.clamp(onto.left(), onto.right()), end.y), 4.5 * zoom.max(0.7), theme.accent, Stroke::new(1.0, theme.surface));
            // Let go on another box, the wire is that box's: off the composite it follows the
            // pointer, lit over a box that can take it. Over nothing it is red: let go
            // there, it is taken away.
            let other = under(pos, wire.from).filter(|other| *other != wire.to);
            let off = !onto.expand(8.0 * zoom).contains(pos);
            let gone = off && under(pos, wire.from).is_none();
            if let (true, Some((start, _))) = (off, ends(wire)) {
                let colour = if other.is_some() { theme.accent } else { theme.danger };
                painter.add(CubicBezierShape::from_points_stroke(curve(start, pos), false, Color32::TRANSPARENT, Stroke::new(2.0, colour)));
            }
            if let (true, Some(group)) = (held.drag_stopped(), other.and_then(folded)) {
                self.drag = None;
                actions.extend(into_folded((wire.ends().0, wire.art), group, pos).and_then(|joined| moved(shot, wire, joined)));
            } else if let (true, Some(other)) = (held.drag_stopped(), other) {
                self.drag = None;
                actions.extend(rewired(shot, wire, other, all.iter().filter(|arriving| arriving.to == other).count()));
            } else if held.drag_stopped() && gone {
                self.drag = None;
                actions.extend(cut_last(shot, wire, &all, &inside));
            } else if held.drag_stopped() {
                self.drag = None;
                // Before those that arrive to the right of where it was let go.
                let (index, was) = (others.iter().filter(|x| **x < pos.x).count(), others.iter().filter(|x| **x < end.x).count());
                if index != was {
                    actions.push(Action::Run("node.connect", json!({ "node": node, "to": composite.0, "index": index })));
                }
            }
        }
        // The end of any other wire is taken off the box it arrives at and let go on another,
        // which the wire then goes to; let go on nothing, clear of the box it was on, the wire
        // is taken away, as an Alt-click takes it; let go on its own box, it stays as it was.
        for wire in all.iter().filter(|wire| !wire.fresh) {
            let alone = !all.iter().any(|other| other.to == wire.to && other.leaves_from() != wire.leaves_from());
            if (wire.kind == Kind::Picture && matches!(wire.to, Item::Composite(_)) && !alone) || rect_of(wire.to).is_none() {
                continue;
            }
            let Some((start, end)) = ends(wire) else { continue };
            let grip = Rect::from_center_size(end, vec2(12.0, 12.0) * zoom.max(0.7));
            let (from, to) = wire.ends();
            let id = response.id.with(("rewire", from.key(shot), wire.art, to.key(shot)));
            let held = ui.interact(reach(grip, id, layer(wire.from).max(layer(wire.to))), id, Sense::drag());
            let held =
                held.on_hover_cursor(egui::CursorIcon::Grab).on_hover_text("Drag onto another node to wire it there, or onto nothing to take the wire away");
            if held.drag_started() {
                self.drag = Some(Drag::Rewire((from, wire.art), to));
            }
            if !matches!(self.drag, Some(Drag::Rewire(dragged, into)) if (dragged, into) == ((from, wire.art), to)) {
                continue;
            }
            let Some(pos) = held.interact_pointer_pos() else { continue };
            let other = under(pos, wire.from).filter(|other| *other != wire.to);
            // Red over nothing, where letting go takes it away.
            let gone = under(pos, wire.from).is_none() && !rect_of(wire.to).is_some_and(|onto| onto.expand(CLEAR).contains(pos));
            let colour = match (other, gone) {
                (Some(_), _) => theme.accent,
                (None, true) => theme.danger,
                (None, false) => wire_color(wire.kind),
            };
            painter.add(CubicBezierShape::from_points_stroke(curve(start, pos), false, Color32::TRANSPARENT, Stroke::new(2.0, colour)));
            if held.drag_stopped() {
                self.drag = None;
                actions.extend(match other {
                    Some(Item::Group(group)) if folded(Item::Group(group)).is_some() => {
                        into_folded((wire.ends().0, wire.art), group, pos).and_then(|joined| moved(shot, wire, joined))
                    }
                    Some(other) => rewired(shot, wire, other, all.iter().filter(|arriving| arriving.to == other).count()),
                    None if gone => cut_last(shot, wire, &all, &inside),
                    None => None,
                });
            }
        }
        if let Some((from, to)) = self.marquee {
            let pulled = Rect::from_two_pos(from, to);
            painter.rect(pulled, 2.0, theme.accent.gamma_multiply(0.12), Stroke::new(1.0, theme.accent), egui::StrokeKind::Inside);
        }
        // The name on an open group's frame: a drag moves the group with all that is in it, a
        // double click folds it, a right-click has the rest, and the name is edited there.
        for (group, _, title) in titles.iter().rev() {
            let (item, group) = (Item::Group(*group), *group);
            if let Some((_, name)) = self.renaming.as_mut().filter(|(renamed, _)| *renamed == item) {
                let mut inside = ui.new_child(egui::UiBuilder::new().max_rect(*title));
                let edit = inside.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                if edit.lost_focus() {
                    let keep = !inside.input(|i| i.key_pressed(egui::Key::Escape)) && name.as_str() != item.name(shot);
                    if let Some((_, name)) = self.renaming.take().filter(|_| keep) {
                        actions.push(Action::Run("group.rename", json!({ "group": group.0, "name": name })));
                    }
                } else {
                    edit.request_focus();
                }
                continue;
            }
            let name = ui.interact(*title, response.id.with(("group", group.0)), Sense::click_and_drag());
            if name.clicked() || name.drag_started() || name.secondary_clicked() {
                raise(&mut self.stack, shot, group.0);
            }
            if name.double_clicked() {
                actions.push(Action::Run("group.set", json!({ "group": group.0, "folded": true })));
            }
            if name.drag_started() {
                let several = picked.contains(&group.0) && picked.len() > 1;
                let others = picked.iter().filter(|_| several).filter_map(|key| item_of(shot, *key)).filter(|other| *other != item);
                self.along = carried(shot, item, others).into_iter().filter_map(|other| Some((other, *places.get(&other)?))).collect();
                self.drag = places.get(&item).map(|at| Drag::Node(item, *at));
            }
            if let (true, Some(Drag::Node(dragged, at))) = (name.dragged(), &mut self.drag)
                && *dragged == item
            {
                *at += name.drag_delta() / zoom;
                for (_, at) in &mut self.along {
                    *at += name.drag_delta() / zoom;
                }
            }
            if name.drag_stopped()
                && let Some(Drag::Node(dragged, at)) = self.drag.take_if(|drag| matches!(drag, Drag::Node(dragged, _) if *dragged == item))
            {
                let along = std::mem::take(&mut self.along);
                let moves: Vec<Value> =
                    [(dragged, at)].into_iter().chain(along).map(|(item, at)| json!({ "node": item.key(shot), "at": [at.x, at.y] })).collect();
                actions.push(Action::Run("node.place", json!({ "moves": moves })));
            }
            name.context_menu(|ui| self.node_menu(ui, shot, item, picked, &places, &mut actions));
            name.on_hover_cursor(egui::CursorIcon::Grab)
                .on_hover_text("A group. Drag to move it with all that is in it; double-click to fold it into one box; right-click for more");
        }
        if self.map {
            self.mini_map(ui, rect, origin, zoom, &boxes, &places);
        }
        actions
    }

    /// The whole graph small in the view's corner, with the part of it that is seen: a press or
    /// a drag there brings the view to that part.
    fn mini_map(&mut self, ui: &egui::Ui, rect: Rect, origin: Pos2, zoom: f32, boxes: &[(Item, Rect)], places: &HashMap<Item, Pos2>) {
        let theme = Theme::of(ui.ctx());
        // The graph's extent, and the part of it in sight, in its own units.
        let mut graph = Rect::NOTHING;
        for at in places.values() {
            graph = graph.union(Rect::from_center_size(*at, SIZE));
        }
        let seen = Rect::from_min_max(((rect.min - origin) / zoom).to_pos2(), ((rect.max - origin) / zoom).to_pos2());
        let whole = graph.union(seen).expand2(SIZE * 0.5);
        if !whole.is_positive() {
            return;
        }
        let room = vec2(180.0, 120.0).min(rect.size() * 0.4);
        let scale = (room.x / whole.width()).min(room.y / whole.height());
        let size = whole.size() * scale;
        let map = Rect::from_min_size(rect.right_bottom() - size - vec2(8.0, 8.0), size);
        let to_map = |at: Pos2| map.min + (at - whole.min) * scale;
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect(map.expand(3.0), 4.0, theme.surface.gamma_multiply(0.92), Stroke::new(1.0, theme.border), egui::StrokeKind::Outside);
        for (item, on_screen) in boxes {
            let at = ((on_screen.center() - origin) / zoom).to_pos2();
            painter.rect_filled(
                Rect::from_center_size(to_map(at), (SIZE * scale).max(vec2(2.0, 2.0))),
                1.0,
                item.color(&Theme::of(ui.ctx())).gamma_multiply(0.8),
            );
        }
        painter.rect_stroke(Rect::from_min_max(to_map(seen.min), to_map(seen.max)), 2.0, Stroke::new(1.5, theme.accent), egui::StrokeKind::Middle);
        #[cfg(test)]
        {
            self.map_drawn = Some(map);
        }
        // Pressed or dragged on, the middle of the view goes there.
        let held = ui.interact(map, ui.id().with("node mini map"), Sense::click_and_drag());
        if let Some(pos) = held.interact_pointer_pos().filter(|_| held.is_pointer_button_down_on() || held.clicked()) {
            let at = whole.min + (pos - map.min) / scale;
            self.pan = -at.to_vec2() * self.zoom;
            ui.ctx().request_repaint();
        }
    }

    /// What can be done to a node, on a right-click.
    fn node_menu(&mut self, ui: &mut egui::Ui, shot: &Shot, item: Item, picked: &BTreeSet<u64>, places: &HashMap<Item, Pos2>, actions: &mut Vec<Action>) {
        // What is done to one of several that are picked together is done to all of them.
        let several = picked.contains(&item.key(shot)) && picked.len() > 1;
        let mut run = |ui: &mut egui::Ui, title: &str, id: &'static str, params: Value| {
            if ui.button(title).clicked() {
                actions.push(Action::Run(id, params));
                ui.close();
            }
        };
        // A board that is locked here is reference: its menu changes nothing of it. Its wires
        // stay free, so that it is stacked over or under the animation as wanted.
        if let Item::Layer(id) = item
            && self.locked.contains(&id)
        {
            ui.weak("A board, locked here: open its lock on its row of the timeline to change it");
            run(ui, "Use as Animation", "layer.useAsAnimation", json!({ "layer": id.0 }));
            return;
        }
        if let Some(source) = item.source() {
            if shot.display != Some(source) {
                run(ui, "Show in the Display", "display.set", json!({ "shot": shot.id.0, "node": source }));
            }
            if shot.composite_of(source).is_some() || shot.display == Some(source) {
                run(ui, "Disconnect", "node.disconnect", json!({ "node": source }));
            }
            // An effect node, put on the wire that leaves this one: what this gives goes
            // through it, to wherever this went. It is just below, in this one's group, and
            // what was below makes room for it.
            let below = Below::of(shot, item, places);
            ui.menu_button("Add Effect", |ui| {
                for kind in effect_kinds().iter().filter(|kind| !is_port_kind(kind.id)) {
                    if let Action::Run(id, params) = added(shot, kind, below.as_ref(), None, None) {
                        run(ui, kind.title, id, params);
                    }
                }
            });
        }
        // Off, an effect lets the picture through as it is, and a layer, a peg or a group is
        // out of what the shot shows, as when its row is hidden: here and in a render. Of
        // several that are picked, whichever way would change any of them.
        let keys = || if several { picked.iter().copied().collect() } else { vec![item.key(shot)] };
        for on in [false, true] {
            if let Some(batch) = switching(shot, keys(), on) {
                run(ui, if on { "Switch On" } else { "Switch Off" }, "edit.batch", batch);
            }
        }
        // What is asked of a layer's or a peg's own peg is asked of all that are picked with it.
        let for_all = |ask: Parenting| parenting(shot, picked, ask).filter(|_| several);
        match item {
            Item::Layer(id) => {
                run(ui, "Add a Pass of It", "display.new", json!({ "node": { "layer": id.0 }, "pass": true }));
                if let Some(batch) = for_all(Parenting::Off) {
                    run(ui, "Take Off Their Pegs", "edit.batch", batch);
                } else if shot.layer(id).is_some_and(|layer| layer.parent.is_some()) {
                    run(ui, "Take Off Its Peg", "layer.setParent", json!({ "layer": id.0 }));
                }
                if shot.layer(id).is_some_and(|layer| layer.deformer.is_some()) {
                    run(ui, "Take Off Its Bones", "layer.setDeformer", json!({ "layer": id.0 }));
                }
                match for_all(Parenting::Above) {
                    Some(batch) => run(ui, "Add Parent Pegs", "edit.batch", batch),
                    None => run(ui, "Add Parent Peg", "peg.new", json!({ "aboveLayer": id.0 })),
                }
                run(ui, "Delete", "layer.delete", json!({ "layer": id.0 }));
            }
            Item::Peg(id) => {
                if let Some(batch) = for_all(Parenting::Off) {
                    run(ui, "Take Off Their Pegs", "edit.batch", batch);
                } else if shot.peg(id).is_some_and(|peg| peg.parent.is_some()) {
                    run(ui, "Take Off Its Peg", "peg.setParent", json!({ "peg": id.0 }));
                }
                match for_all(Parenting::Above) {
                    Some(batch) => run(ui, "Add Parent Pegs", "edit.batch", batch),
                    None => run(ui, "Add Parent Peg", "peg.new", json!({ "abovePeg": id.0 })),
                }
                // Pushed once `run` is done with `actions`.
                let mut driving = None;
                match shot.driver_of(id) {
                    Some(driver) => {
                        if ui.button("Pick Its Driver").clicked() {
                            driving = Some(Action::Pick(vec![driver.id.0]));
                            ui.close();
                        }
                    }
                    None => {
                        let at = places.get(&item).map(|at| [f64::from(at.x - ACROSS), f64::from(at.y - DOWN * 0.5)]);
                        let hint = "A Driver node: expressions over other pegs that add to this peg's position, rotation and scale on every frame";
                        if ui.button("Add Driver").on_hover_text(hint).clicked() {
                            driving = Some(Action::Run("driver.new", json!({ "peg": id.0, "at": at })));
                            ui.close();
                        }
                    }
                }
                run(ui, "Delete", "peg.delete", json!({ "peg": id.0 }));
                actions.extend(driving);
            }
            Item::Composite(id) => {
                if ui.button("Rename").clicked() {
                    self.renaming = Some((item, item.name(shot).to_string()));
                    ui.close();
                }
                // What it stacks, rendered apart on the Export page.
                run(ui, "Add a Pass of It", "display.new", json!({ "node": { "composite": id.0 }, "pass": true }));
                run(ui, "Delete", "composite.delete", json!({ "composite": id.0 }));
            }
            Item::Deformer(id) => {
                if ui.button("Rename").clicked() {
                    self.renaming = Some((item, item.name(shot).to_string()));
                    ui.close();
                }
                run(ui, "Delete", "deformer.delete", json!({ "deformer": id.0 }));
            }
            Item::Group(id) => {
                let Some(group) = shot.group(id) else { return };
                run(ui, if group.folded { "Unfold" } else { "Fold" }, "group.set", json!({ "group": id.0, "folded": !group.folded }));
                if ui.button("Rename").clicked() {
                    self.renaming = Some((item, item.name(shot).to_string()));
                    ui.close();
                }
                // Ways in and out of the group: ports on its frame, and on its box when it is folded.
                run(ui, "Add Port In", "effect.new", json!({ "shot": shot.id.0, "kind": iskra_model::PORT_IN, "group": id.0 }));
                run(ui, "Add Port Out", "effect.new", json!({ "shot": shot.id.0, "kind": iskra_model::PORT_OUT, "group": id.0 }));
                let title = if group.board { "No Longer the Storyboard's" } else { "Make It the Storyboard's" };
                run(ui, title, "group.set", json!({ "group": id.0, "board": !group.board }));
                run(ui, "Ungroup", "group.delete", json!({ "group": id.0 }));
                // The app makes it the deleting of each thing in it, as one step.
                run(ui, "Delete with All in It", "group.delete", json!({ "group": id.0, "contents": true }));
            }
            Item::Effect(id) => {
                if shot.effect(id).is_none() {
                    return;
                }
                run(ui, "Add a Pass of It", "display.new", json!({ "node": { "effect": id.0 }, "pass": true }));
                if ui.button("Rename").clicked() {
                    self.renaming = Some((item, item.name(shot).to_string()));
                    ui.close();
                }
                run(ui, "Delete", "effect.delete", json!({ "effect": id.0 }));
            }
            Item::Display(id) => {
                if !shot.has_display(id) {
                    return;
                }
                if shot.shown_by(id).flatten().is_some() {
                    run(ui, "Show Nothing", "display.set", json!({ "display": id.0 }));
                }
                if ui.button("Rename").clicked() {
                    self.renaming = Some((item, item.name(shot).to_string()));
                    ui.close();
                }
                let pass = shot.passes().any(|pass| pass.id == id);
                if id != shot.main_display() && !pass {
                    // What the Export page renders, the Storyboard page shows and playback keeps ready.
                    run(ui, "Make Main Display", "display.setMain", json!({ "display": id.0 }));
                }
                if id != shot.main_display() {
                    run(ui, "Delete", "display.delete", json!({ "display": id.0 }));
                }
            }
            Item::Driver(id) => {
                if shot.driver(id).is_none() {
                    return;
                }
                if ui.button("Rename").clicked() {
                    self.renaming = Some((item, item.name(shot).to_string()));
                    ui.close();
                }
                let hint = "Key the peg on every frame of the shot with what the driver makes it, and take the driver away";
                let baked = ui.button("Bake into Keys").on_hover_text(hint).clicked();
                if baked {
                    ui.close();
                }
                run(ui, "Delete", "driver.delete", json!({ "driver": id.0 }));
                if baked {
                    actions.push(Action::Run("driver.bake", json!({ "driver": id.0 })));
                }
            }
            Item::Camera => {
                if shot.camera.parent.is_some() {
                    run(ui, "Put It Back at Home", "camera.setParent", json!({ "shot": shot.id.0 }));
                }
                let title = if shot.camera.parent.is_some() { "Select Its Peg" } else { "Add Camera Peg" };
                if ui
                    .button(title)
                    .on_hover_text("The peg that moves the camera: its position pans it, its rotation rolls it, its depth moves it in and out")
                    .clicked()
                {
                    actions.push(Action::Command("camera.addPeg"));
                    ui.close();
                }
            }
        }
        if let Item::Composite(id) = item {
            // How it orders what it stacks (plan §8.6): the one it has now is marked.
            let now = shot.composite(id).map(|composite| (composite.stacking, composite.right_first));
            ui.menu_button("Ordering", |ui| {
                for (title, hint, said, stacking, right) in ORDERINGS {
                    let on = now.is_some_and(|(has, right_first)| has == stacking && right.is_none_or(|right| right == right_first));
                    if ui.radio(on, title).on_hover_text(hint).clicked() {
                        actions.push(Action::Run("composite.setOrdering", json!({ "composite": id.0, "ordering": said, "rightFirst": right })));
                        ui.close();
                    }
                }
            });
        }
        // A colour of the user's own for the node, or for all that are picked with it.
        ui.menu_button("Colour", |ui| {
            let keys: Vec<u64> = if several { picked.iter().copied().collect() } else { vec![item.key(shot)] };
            let own = shot.colours.get(&item.key(shot)).copied();
            if ui.radio(own.is_none(), "Its Kind's").clicked() {
                actions.push(Action::Run("node.setColour", json!({ "nodes": keys })));
                ui.close();
            }
            for (name, colour) in iskra_model::MARKER_COLOURS {
                ui.horizontal(|ui| {
                    let (swatch, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                    ui.painter().rect_filled(swatch, 2.0, Color32::from_rgb(colour[0], colour[1], colour[2]));
                    if ui.radio(own == Some(colour), name).clicked() {
                        actions.push(Action::Run("node.setColour", json!({ "nodes": keys, "colour": colour })));
                        ui.close();
                    }
                });
            }
        });
        // Into a group, out of one, or a new group of the node with what goes with it.
        if !matches!(item, Item::Display(_) | Item::Camera | Item::Driver(_)) {
            ui.separator();
            let key = item.key(shot);
            if several {
                // Those that are picked and no others: what goes with one of them is not taken
                // along, as it is when one node is grouped alone.
                let chosen = || picked.iter().copied().filter(|key| item_of(shot, *key).is_some_and(|item| item != Item::Camera)).collect::<Vec<u64>>();
                if ui.button("Group the Picked").on_hover_text("A group of the nodes that are picked together, and no others").clicked() {
                    let nodes = chosen();
                    actions.push(Action::Run("group.new", json!({ "nodes": nodes, "ports": true })));
                    ui.close();
                }
                if ui.button("Group the Picked with a Composite").on_hover_text(GATHER_HINT).clicked() {
                    let nodes = chosen();
                    actions.push(Action::Run("group.new", json!({ "nodes": nodes, "composite": true, "ports": true })));
                    ui.close();
                }
            } else if !matches!(item, Item::Group(_)) {
                let hint =
                    "A group of this and what goes with it: what a peg carries, what a composite stacks and the pegs over that, and the bones that bend them";
                if ui.button("Group").on_hover_text(hint).clicked() {
                    actions.push(Action::Run("group.new", json!({ "nodes": rig_of(shot, item), "ports": true })));
                    ui.close();
                }
                if ui.button("Group with a Composite").on_hover_text(GATHER_HINT).clicked() {
                    actions.push(Action::Run("group.new", json!({ "nodes": rig_of(shot, item), "composite": true, "ports": true })));
                    ui.close();
                }
            }
            let own = shot.group_of(key).map(|group| group.id);
            let others: Vec<_> = shot
                .groups
                .iter()
                .filter(|group| Some(group.id) != own && group.id.0 != key && !shot.groups_above(group.id.0).any(|above| above.id.0 == key))
                .collect();
            if !others.is_empty() {
                ui.menu_button("Add to Group", |ui| {
                    for group in others {
                        if ui.button(&group.name).clicked() {
                            let nodes: Vec<u64> = if several { picked.iter().copied().filter(|node| *node != group.id.0).collect() } else { vec![key] };
                            actions.push(Action::Run("group.add", json!({ "group": group.id.0, "nodes": nodes })));
                            ui.close();
                        }
                    }
                });
            }
            if own.is_some() && ui.button("Take Out of Its Group").clicked() {
                actions.push(Action::Run("group.remove", json!({ "nodes": [key] })));
                ui.close();
            }
        }
        if let Some(part) = item.part() {
            ui.separator();
            let hint = "As a template, to put into other shots. A peg goes with all it carries, a composite with all it stacks";
            let copies = "Copies beside these, with drawings of their own: what is drawn on a copy leaves the first as it is";
            if ui.button("Duplicate").on_hover_text(copies).clicked() {
                let nodes = if several { parts_of(shot, picked) } else { vec![part] };
                actions.push(Action::Run("node.duplicate", json!({ "shot": shot.id.0, "nodes": nodes })));
                ui.close();
            }
            if ui.button("Save to Library…").on_hover_text(hint).clicked() {
                let parts = if several { parts_of(shot, picked) } else { vec![part] };
                actions.push(Action::Save { parts, at: None });
                ui.close();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_model::{Composite, Peg, Project, Role, Transform};

    use super::*;

    #[test]
    fn a_node_grouped_with_what_goes_with_it_takes_the_groups_that_is_in_whole() {
        let mut s = iskra_engine::Session::new();
        let id = |made: Value| made["id"].as_u64().unwrap();
        let arm = s.project().shots[0].layers[0].id.0;
        let shoulder = id(s.execute("peg.new", json!({ "aboveLayer": arm, "name": "Shoulder" })).unwrap());
        let root = id(s.execute("peg.new", json!({ "abovePeg": shoulder, "name": "Root" })).unwrap());
        let hand = id(s.execute("layer.new", json!({ "name": "Hand" })).unwrap());
        let inner = id(s.execute("group.new", json!({ "nodes": [arm, hand], "name": "Inner" })).unwrap());
        let limb = id(s.execute("group.new", json!({ "nodes": [shoulder, inner], "name": "Limb" })).unwrap());
        // The root, in no group, takes the Limb whole, the Hand it does not carry with it.
        let shot = s.project().shots[0].clone();
        assert_eq!(rig_of(&shot, Item::Peg(PegId(root))), {
            let mut whole = vec![root, limb];
            whole.sort_unstable();
            whole
        });
        // The shoulder, in the Limb, takes the Inner group whole, and not the Limb.
        assert_eq!(rig_of(&shot, Item::Peg(PegId(shoulder))), {
            let mut whole = vec![shoulder, inner];
            whole.sort_unstable();
            whole
        });
        // Grouped so, every group is kept, in the new one.
        let made = id(s.execute("group.new", json!({ "nodes": rig_of(&shot, Item::Peg(PegId(root))), "ports": true })).unwrap());
        let shot = s.project().shots[0].clone();
        assert_eq!(shot.group_of(limb).map(|group| group.id.0), Some(made));
        assert_eq!((shot.group_of(inner).map(|group| group.id.0), shot.group_of(hand).map(|group| group.id.0)), (Some(limb), Some(inner)));
    }

    #[test]
    fn a_port_given_a_name_of_its_own_is_called_by_it() {
        let mut shot = Arc::clone(&Project::default().shots[0]);
        let kind = iskra_model::effect_kind(iskra_model::PORT_IN).expect("ports are a kind");
        for (id, name) in [(1, "Port In"), (2, "Port In 3"), (3, "Shoulder"), (4, "Port In Left")] {
            Arc::make_mut(&mut shot).effects.push(Arc::new(iskra_model::Effect::new(EffectId(id), name, kind)));
        }
        let named: Vec<bool> = (1..=4).map(|id| named_port(&shot, Item::Effect(EffectId(id)))).collect();
        assert_eq!(named, [false, false, true, true]);
    }

    #[test]
    fn the_bars_zoom_keeps_what_is_in_the_middle_where_it_is() {
        let mut view = NodeView { pan: vec2(40.0, -20.0), ..NodeView::default() };
        // The point of the graph in the middle of the view: where the origin is, the other way.
        let middle = |view: &NodeView| -view.pan / view.zoom;
        let before = middle(&view);
        view.zoom_to(2.0);
        assert_eq!((view.zoom(), middle(&view)), (2.0, before));
        view.zoom_to(10.0);
        assert_eq!(view.zoom(), *NodeView::ZOOM.end(), "no further in than a pinch goes");
        view.zoom_to(0.05);
        assert_eq!(view.zoom(), 0.05, "far below a fifth, for a large rig");
        view.zoom_to(0.001);
        assert_eq!(view.zoom(), *NodeView::ZOOM.start());
    }

    #[test]
    fn a_drag_at_the_edge_of_the_view_pushes_the_view_the_other_way() {
        let view = Rect::from_min_max(pos2(100.0, 100.0), pos2(500.0, 400.0));
        assert_eq!(edge_push(view, pos2(300.0, 250.0)), Vec2::ZERO);
        assert_eq!(edge_push(view, pos2(100.0, 250.0)), vec2(EDGE_SPEED, 0.0));
        assert_eq!(edge_push(view, pos2(40.0, 250.0)), vec2(EDGE_SPEED, 0.0), "past the edge as on it");
        assert_eq!(edge_push(view, pos2(500.0 - EDGE / 2.0, 400.0)), vec2(-EDGE_SPEED / 2.0, -EDGE_SPEED));
    }

    #[test]
    fn bones_sit_between_the_peg_and_the_drawing_and_a_peg_wired_from_them_follows_them() {
        let mut project = Project::default();
        let fist_layer = project.new_layer("Fist", Role::Animation);
        let (arm_peg, fist_peg, bones) = (PegId(project.new_id()), PegId(project.new_id()), DeformerId(project.new_id()));
        let shot = Arc::make_mut(&mut project.shots[0]);
        let (arm, fist) = (shot.layers[0].id, fist_layer.id);
        shot.layers.push(Arc::new(fist_layer));
        Arc::make_mut(&mut shot.composites[0]).inputs.push(Source::Layer(fist));
        let peg = |id, name: &str, parent| {
            Arc::new(Peg {
                id,
                name: name.to_string(),
                parent,
                transform: Transform::default(),
                animation: Default::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            })
        };
        shot.pegs.extend([peg(arm_peg, "Arm-P", None), peg(fist_peg, "Fist-P", Some(arm_peg))]);
        shot.deformers.push(Arc::new(iskra_model::Deformer::curve(bones, "Arm-Curve", &[[0.0, 0.0], [100.0, 0.0]])));
        Arc::make_mut(&mut shot.layers[0]).parent = Some(arm_peg);
        Arc::make_mut(&mut shot.layers[0]).deformer = Some(bones);
        Arc::make_mut(&mut shot.layers[1]).parent = Some(fist_peg);
        // Arm-P, then the bones, then the arm, in a column.
        let at = layout(shot);
        let (peg_at, bones_at, arm_at) = (at[&Item::Peg(arm_peg)], at[&Item::Deformer(bones)], at[&Item::Layer(arm)]);
        assert!(peg_at.y < bones_at.y && bones_at.y < arm_at.y && bones_at.x == arm_at.x, "{peg_at:?} {bones_at:?} {arm_at:?}");
        // The transform goes into the bones and the bend on to the arm, and not the peg's
        // own wire to the arm beside it.
        let into =
            |shot: &Shot, to: Item| wires(shot, &layout(shot)).into_iter().filter(|wire| wire.to == to).map(|wire| (wire.from, wire.kind)).collect::<Vec<_>>();
        assert_eq!(into(shot, Item::Deformer(bones)), [(Item::Peg(arm_peg), Kind::Transform)]);
        assert_eq!(into(shot, Item::Layer(arm)), [(Item::Deformer(bones), Kind::Bend)]);
        assert_eq!(into(shot, Item::Peg(fist_peg)), [(Item::Peg(arm_peg), Kind::Transform)]);
        // A wire from the bones to the fist's peg makes it follow them, under the arm's peg;
        // then its wire is the bones' alone, and cut, it follows nothing.
        let Some(Action::Run("edit.batch", params)) = join(shot, (Item::Deformer(bones), None), Item::Peg(fist_peg), 0, None) else { panic!() };
        assert_eq!(params["commands"], json!([{ "id": "peg.setFollow", "params": { "peg": fist_peg.0, "deformer": bones.0 } }]));
        Arc::make_mut(&mut shot.pegs[1]).follows = Some(iskra_model::Follow { deformer: bones, point: None, bends: false });
        assert_eq!(into(shot, Item::Peg(fist_peg)), [(Item::Deformer(bones), Kind::Bend)]);
        let wire = wires(shot, &layout(shot)).into_iter().find(|wire| wire.to == Item::Peg(fist_peg)).unwrap();
        assert!(matches!(cut(shot, &wire), Some(Action::Run("peg.setFollow", params)) if params == json!({ "peg": fist_peg.0 })));
        // A peg under another parent is put under the bones' peg as it is wired.
        Arc::make_mut(&mut shot.pegs[1]).parent = None;
        let Some(Action::Run(_, params)) = join(shot, (Item::Deformer(bones), None), Item::Peg(fist_peg), 0, None) else { panic!() };
        assert_eq!(params["commands"][0], json!({ "id": "peg.setParent", "params": { "peg": fist_peg.0, "parent": arm_peg.0 } }));
        // The transform into the bones is the arm's: wired from another peg, the arm goes
        // under that one.
        let Some(Action::Run(_, params)) = join(shot, (Item::Peg(fist_peg), None), Item::Deformer(bones), 0, None) else { panic!() };
        assert_eq!(params["commands"], json!([{ "id": "layer.setParent", "params": { "layer": arm.0, "parent": fist_peg.0 } }]));

        // A peg let go at the left of the top goes into the start port or the end port, the
        // nearer; further along, it is the transform into the top.
        let port = |shot: &Shot, across: f32| match join(shot, (Item::Peg(fist_peg), None), Item::Deformer(bones), 0, Some(across)) {
            Some(Action::Run("deformer.setPort", params)) => Some(params),
            _ => None,
        };
        assert_eq!(port(shot, 0.1), Some(json!({ "deformer": bones.0, "port": "start", "peg": fist_peg.0 })));
        assert_eq!(port(shot, 0.3), Some(json!({ "deformer": bones.0, "port": "end", "peg": fist_peg.0 })));
        assert_eq!(port(shot, 0.5), None);
        // Wired into both, each is a wire of its own, and each is cut from its own port.
        let chain = iskra_model::Deformer::new(bones, "Arm-Bones", [0.0, 0.0], vec![iskra_model::Bone::at_rest(0.0, 100.0)]);
        shot.deformers = vec![Arc::new(chain)];
        Arc::make_mut(&mut shot.deformers[0]).ik = Some(iskra_model::Ik { on: true, peg: Some(fist_peg), ..Default::default() });
        Arc::make_mut(&mut shot.deformers[0]).start_peg = Some(arm_peg);
        let ported = |shot: &Shot, from: PegId| {
            wires(shot, &layout(shot)).into_iter().find(|wire| (wire.from, wire.to, wire.kind) == (Item::Peg(from), Item::Deformer(bones), Kind::Bend))
        };
        let (start, end) = (ported(shot, arm_peg).expect("the start's wire"), ported(shot, fist_peg).expect("the end's wire"));
        assert!(start.end.x < end.end.x, "the start's port is the left one");
        assert!(matches!(cut(shot, &start), Some(Action::Run("deformer.setPort", params)) if params["port"] == "start" && params["peg"].is_null()));
        assert!(matches!(cut(shot, &end), Some(Action::Run("deformer.setPort", params)) if params["port"] == "end" && params["peg"].is_null()));
        // A port, a Send or a Receive stands for the peg that comes into it: wired into the
        // start, cut from there. One that no peg comes into is not wired.
        let port = EffectId(4242);
        let mut node = iskra_model::Effect::new(port, "In", iskra_model::effect_kind(iskra_model::PORT_IN).unwrap());
        node.parent = Some(arm_peg);
        shot.effects.push(Arc::new(node));
        let into_start = |shot: &Shot| join(shot, (Item::Effect(port), None), Item::Deformer(bones), 0, Some(0.1));
        assert!(
            matches!(into_start(shot), Some(Action::Run("deformer.setPort", params)) if params == json!({ "deformer": bones.0, "port": "start", "peg": port.0 }))
        );
        Arc::make_mut(&mut shot.deformers[0]).start_peg = Some(PegId(port.0));
        let start = wires(shot, &layout(shot)).into_iter().find(|wire| (wire.from, wire.kind) == (Item::Effect(port), Kind::Bend)).expect("the port's wire");
        assert!(matches!(cut(shot, &start), Some(Action::Run("deformer.setPort", params)) if params["port"] == "start" && params["peg"].is_null()));
        Arc::make_mut(shot.effects.last_mut().unwrap()).parent = None;
        assert!(into_start(shot).is_none());
    }

    #[test]
    fn an_effect_added_below_a_node_in_a_group_goes_into_the_group_and_what_is_below_makes_room() {
        let mut project = Project::default();
        let (group, blur) = (iskra_model::GroupId(project.new_id()), EffectId(project.new_id()));
        let shot = Arc::make_mut(&mut project.shots[0]);
        let (layer, composite) = (shot.layers[0].id, shot.composites[0].id);
        // The layer, in a group, through a blur into the composite.
        shot.effects.push(Arc::new(iskra_model::Effect::new(blur, "Blur", iskra_model::effect_kind("blur").unwrap())));
        shot.rewire(Source::Layer(layer), Some(Source::Effect(blur)));
        Arc::make_mut(&mut shot.effects[0]).inputs.insert(iskra_model::PICTURE.to_string(), Source::Layer(layer));
        shot.groups.push(Arc::new(iskra_model::Group {
            id: group,
            name: "Rig".to_string(),
            members: [layer.0].into(),
            folded: false,
            rows_folded: false,
            board: false,
            template: None,
        }));
        assert_eq!(downstream(shot, Source::Layer(layer)), [Item::Effect(blur), Item::Composite(composite), Item::Display(shot.main_display())]);
        // The blur half a row under the layer, the composite a row under that.
        let places = HashMap::from([
            (Item::Layer(layer), pos2(0.0, 0.0)),
            (Item::Effect(blur), pos2(0.0, DOWN / 2.0)),
            (Item::Composite(composite), pos2(0.0, DOWN * 1.5)),
            (Item::Display(shot.main_display()), pos2(0.0, DOWN * 2.5)),
        ]);
        let below = Below::of(shot, Item::Layer(layer), &places).unwrap();
        let Action::Run("edit.batch", params) = added(shot, iskra_model::effect_kind("glow").unwrap(), Some(&below), None, None) else { panic!() };
        let made = &params["commands"][0];
        assert_eq!((&made["id"], &made["params"]["group"], &made["params"]["on"]), (&json!("effect.new"), &json!(group.0), &json!({ "layer": layer.0 })));
        // All that is below goes down by half a row, so the blur is a row under the layer.
        let half = f64::from(DOWN / 2.0);
        let moves = json!([
            { "node": blur.0, "at": [0.0, half * 2.0] },
            { "node": composite.0, "at": [0.0, half * 4.0] },
            { "node": shot.id.0, "at": [0.0, half * 6.0] },
        ]);
        assert_eq!(params["commands"][1], json!({ "id": "node.place", "params": { "moves": moves } }));
        // Where there is room already, nothing moves.
        let roomy = HashMap::from([(Item::Layer(layer), pos2(0.0, 0.0)), (Item::Effect(blur), pos2(0.0, DOWN * 2.0))]);
        assert!(Below::of(shot, Item::Layer(layer), &roomy).unwrap().room.is_empty());
    }

    #[test]
    fn the_layout_puts_inputs_over_their_composite_and_pegs_over_what_they_carry() {
        let mut project = Project::default();
        let extra = project.new_layer("Sky", Role::Animation);
        let (peg, spare) = (PegId(project.new_id()), CompositeId(project.new_id()));
        let shot = Arc::make_mut(&mut project.shots[0]);
        let (drawing, sky, main) = (shot.layers[0].id, extra.id, shot.composites[0].id);
        shot.layers.push(Arc::new(extra));
        Arc::make_mut(&mut shot.composites[0]).inputs.push(Source::Layer(sky));
        shot.pegs.push(Arc::new(Peg {
            id: peg,
            name: "Peg".to_string(),
            parent: None,
            transform: Transform::default(),
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        Arc::make_mut(&mut shot.layers[0]).parent = Some(peg);
        shot.composites.push(Arc::new(Composite::new(spare, "Spare", Vec::new())));

        let at = layout(shot);
        let [display, composite, top, bottom] =
            [Item::Display(shot.main_display()), Item::Composite(main), Item::Layer(drawing), Item::Layer(sky)].map(|item| at[&item]);
        assert!(composite.y < display.y && top.y < composite.y && top.y == bottom.y, "what feeds a node is above it");
        assert!(top.x < bottom.x && (top.x + bottom.x) / 2.0 == composite.x, "the top of the stack on the left, centred over the composite");
        assert!(at[&Item::Peg(peg)].y < top.y && at[&Item::Peg(peg)].x == top.x, "a peg is over what it carries");
        assert!(at[&Item::Composite(spare)].x > bottom.x, "what is connected to nothing waits at the side");
        // A node that was put somewhere stays there.
        shot.places.insert(sky.0, [500.0, -40.0]);
        assert_eq!((layout(shot)[&Item::Layer(sky)], layout(shot)[&Item::Layer(drawing)]), (pos2(500.0, -40.0), top));
        // Every wire: two pictures into the composite, one into the display, one transform.
        let wires = wires(shot, &at);
        assert_eq!((wires.len(), wires.iter().filter(|wire| wire.kind == Kind::Transform).count()), (4, 1));
        assert!(wires[0].end.x < wires[1].end.x && wires[0].end.y == composite.y - SIZE.y / 2.0);
    }

    #[test]
    fn a_new_node_comes_beside_what_it_is_wired_to_wherever_that_was_put() {
        let mut project = Project::default();
        let peg = PegId(project.new_id());
        let shot = Arc::make_mut(&mut project.shots[0]);
        let drawing = shot.layers[0].id;
        // The view has laid the shot out once; then the layer is dragged far off.
        let mut kept = HashMap::new();
        arranged(shot, &mut kept);
        shot.places.insert(drawing.0, [900.0, -600.0]);
        // A new peg over the layer is over it there, as far above it as the layout has it.
        shot.pegs.push(Arc::new(Peg {
            id: peg,
            name: "Peg".to_string(),
            parent: None,
            transform: Transform::default(),
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        Arc::make_mut(&mut shot.layers[0]).parent = Some(peg);
        let at = arranged(shot, &mut kept);
        let (over, layer) = (at[&Item::Peg(peg)], at[&Item::Layer(drawing)]);
        assert!(over.x == layer.x && over.y < layer.y && layer.y - over.y < DOWN, "{over:?} over {layer:?}");
        // And stays there, as one laid out once does.
        assert_eq!(arranged(shot, &mut kept)[&Item::Peg(peg)], over);
        // A second peg wired to the same spot goes aside, clear of the first.
        let other = PegId(peg.0 + 1000);
        let mut second = (*shot.pegs[0]).clone();
        (second.id, second.name) = (other, "Peg 2".to_string());
        shot.pegs.push(Arc::new(second));
        Arc::make_mut(&mut shot.pegs[0]).parent = Some(other);
        let at = arranged(shot, &mut kept);
        let beside = at[&Item::Peg(other)];
        assert!((beside - over).abs().x >= SIZE.x || (beside - over).abs().y >= SIZE.y, "{beside:?} clear of {over:?}");
    }

    #[test]
    fn a_folded_groups_box_is_in_the_middle_of_its_nodes_where_they_were_put() {
        let mut project = Project::default();
        let group = GroupId(project.new_id());
        let shot = Arc::make_mut(&mut project.shots[0]);
        let drawing = shot.layers[0].id;
        shot.places.insert(drawing.0, [900.0, -600.0]);
        let members = [drawing.0].into();
        shot.groups.push(Arc::new(iskra_model::Group {
            id: group,
            name: "G".to_string(),
            members,
            folded: true,
            rows_folded: true,
            board: false,
            template: None,
        }));
        let at = arranged(shot, &mut HashMap::new());
        assert_eq!(at[&Item::Group(group)], pos2(900.0, -600.0));
    }

    #[test]
    fn groups_dragged_together_take_all_that_is_in_them_along_groups_in_groups_too() {
        // Two groups picked, the second holding a group of its own: dragging the first moves
        // what is in both, or the second would go back to where its nodes stayed.
        let mut project = Project::default();
        let (first, second, inner, peg) = (GroupId(project.new_id()), GroupId(project.new_id()), GroupId(project.new_id()), PegId(project.new_id()));
        let shot = Arc::make_mut(&mut project.shots[0]);
        let drawing = shot.layers[0].id;
        shot.pegs.push(Arc::new(Peg {
            id: peg,
            name: "Peg".to_string(),
            parent: None,
            transform: Transform::default(),
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        let group = |id: GroupId, name: &str, member: u64| {
            Arc::new(iskra_model::Group {
                id,
                name: name.to_string(),
                members: [member].into(),
                folded: false,
                rows_folded: false,
                board: false,
                template: None,
            })
        };
        shot.groups.extend([group(first, "A", drawing.0), group(second, "B", inner.0), group(inner, "C", peg.0)]);
        let along = carried(shot, Item::Group(first), [Item::Group(second)].into_iter());
        for item in [Item::Group(second), Item::Layer(drawing), Item::Group(inner), Item::Peg(peg)] {
            assert!(along.contains(&item), "{item:?} in {along:?}");
        }
        assert!(!along.contains(&Item::Group(first)));
    }

    #[test]
    fn a_folded_group_folds_where_its_frame_is_whatever_its_own_place_and_ports() {
        // As in a file where the nodes of an open group were moved and the group then folded:
        // its own place is where its box once was, and its port far off at the side.
        let mut project = Project::default();
        let (group, port, peg) = (GroupId(project.new_id()), EffectId(project.new_id()), PegId(project.new_id()));
        let shot = Arc::make_mut(&mut project.shots[0]);
        let drawing = shot.layers[0].id;
        shot.pegs.push(Arc::new(Peg {
            id: peg,
            name: "Peg".to_string(),
            parent: None,
            transform: Transform::default(),
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        let kind = iskra_model::effect_kind(iskra_model::PORT_IN).expect("ports are a kind");
        shot.effects.push(Arc::new(iskra_model::Effect::new(port, "Port In", kind)));
        shot.places.extend([(drawing.0, [100.0, 100.0]), (peg.0, [500.0, 300.0]), (port.0, [2700.0, 0.0]), (group.0, [-600.0, -600.0])]);
        let members = [drawing.0, peg.0, port.0].into();
        shot.groups.push(Arc::new(iskra_model::Group {
            id: group,
            name: "G".to_string(),
            members,
            folded: true,
            rows_folded: true,
            board: false,
            template: None,
        }));
        for at in [arranged(shot, &mut HashMap::new()), layout(shot)] {
            assert_eq!(at[&Item::Group(group)], pos2(300.0, 200.0));
        }
    }

    /// A shot with the pegs `Car-P`, `Wheel-P` under it and `Other-P`, and a driver of the
    /// wheel that reads the car.
    fn driven() -> (Project, [PegId; 3], DriverId) {
        let mut project = Project::default();
        let ids = [PegId(project.new_id()), PegId(project.new_id()), PegId(project.new_id())];
        let driver = DriverId(project.new_id());
        let shot = Arc::make_mut(&mut project.shots[0]);
        for (id, (name, parent)) in ids.iter().zip([("Car-P", None), ("Wheel-P", Some(ids[0])), ("Other-P", None)]) {
            shot.pegs.push(Arc::new(Peg {
                id: *id,
                name: name.to_string(),
                parent,
                transform: Transform::default(),
                animation: Default::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            }));
        }
        let mut made = iskra_model::Driver::new(driver, "Roll", ids[1]);
        made.inputs.push(iskra_model::DriverInput { name: "car".into(), peg: Some(ids[0]) });
        shot.drivers.push(Arc::new(made));
        (project, ids, driver)
    }

    #[test]
    fn a_driver_is_beside_its_peg_with_dashed_wires_from_what_it_reads_and_into_its_peg() {
        let (project, [car, wheel, _], driver) = driven();
        let shot = &project.shots[0];
        let at = layout(shot);
        let (box_at, peg_at) = (at[&Item::Driver(driver)], at[&Item::Peg(wheel)]);
        assert!(box_at.x < peg_at.x && box_at.y < peg_at.y, "at the left of its peg, a little up: {box_at:?} {peg_at:?}");
        let reads: Vec<(Item, Item, Pos2)> =
            wires(shot, &at).into_iter().filter(|wire| wire.kind == Kind::Read).map(|wire| (wire.from, wire.to, wire.end)).collect();
        assert_eq!(reads.len(), 2);
        let (_, _, into_peg) = reads.iter().find(|(from, ..)| *from == Item::Driver(driver)).expect("its wire into its peg");
        assert_eq!(*into_peg, peg_at - vec2(SIZE.x / 2.0, 0.0), "into the left side of the peg");
        let (_, _, into_top) = reads.iter().find(|(from, to, _)| (*from, *to) == (Item::Peg(car), Item::Driver(driver))).expect("the car's wire into it");
        assert_eq!(into_top.y, box_at.y - SIZE.y / 2.0);
        // Its name, its colour of the Move category, a peg's, and its box found by its id.
        assert_eq!(Item::Driver(driver).name(shot), "Roll");
        assert_eq!(item_of(shot, driver.0), Some(Item::Driver(driver)));
    }

    #[test]
    fn a_peg_wired_into_a_driver_is_read_by_a_name_made_from_its_own_and_cut_it_is_let_go() {
        let (project, [car, wheel, other], driver) = driven();
        let shot = &project.shots[0];
        let ran = |action: Option<Action>| match action {
            Some(Action::Run(id, params)) => Some((id, params)),
            _ => None,
        };
        let (id, params) = ran(join(shot, (Item::Peg(other), None), Item::Driver(driver), 0, None)).expect("it is wired");
        assert_eq!((id, params), ("driver.addInput", json!({ "driver": driver.0, "name": "other", "peg": other.0 })));
        assert_eq!(ran(join(shot, (Item::Peg(car), None), Item::Driver(driver), 0, None)), None, "the car is read already");
        let read = wires(shot, &layout(shot)).into_iter().find(|wire| wire.from == Item::Peg(car) && wire.kind == Kind::Read).unwrap();
        assert_eq!(ran(cut(shot, &read)), Some(("driver.connect", json!({ "driver": driver.0, "input": "car" }))));
        let own = wires(shot, &layout(shot)).into_iter().find(|wire| wire.from == Item::Driver(driver)).unwrap();
        assert_eq!(ran(cut(shot, &own)), None, "its own wire goes with it");
        // Names: short of `-P`, small, free, and none the language has.
        let held = shot.driver(driver).unwrap();
        assert_eq!(input_name(held, "Car-P"), "car2");
        assert_eq!(input_name(held, "Frame"), "frame2");
        assert_eq!(input_name(held, "Left Wheel-P"), "leftWheel");
        assert_eq!(input_name(held, "3D-P"), "peg3D");
        let _ = wheel;
    }

    #[test]
    fn a_driver_switched_off_is_drawn_off_and_switched_on_by_its_command() {
        let (mut project, _, driver) = driven();
        let shot = Arc::make_mut(&mut project.shots[0]);
        assert!(!is_off(shot, Item::Driver(driver)));
        let off = switching(shot, [driver.0], false).expect("it can be switched off");
        assert_eq!(off["commands"][0], json!({ "id": "driver.setEnabled", "params": { "driver": driver.0, "enabled": false } }));
        Arc::make_mut(&mut shot.drivers[0]).off = true;
        assert!(is_off(shot, Item::Driver(driver)) && switching(shot, [driver.0], false).is_none());
    }

    #[test]
    fn every_node_that_is_no_effect_is_in_a_category_add_node_has() {
        for (title, _, _, category) in STRUCTURE {
            assert!(category == "Move" || categories().contains(&category), "{title} is in {category}, which Add Node has not got");
        }
        let in_category = |category: &str| STRUCTURE.iter().filter(|(.., of)| *of == category).map(|(title, ..)| *title).collect::<Vec<_>>();
        assert_eq!(in_category("Sources"), ["Layer", "Text Layer"]);
        assert_eq!(in_category("Move"), ["Peg"]);
        assert_eq!(in_category("Combine"), ["Composite"]);
        assert_eq!(in_category("Structure"), ["Display", "Pass"]);
    }
}
