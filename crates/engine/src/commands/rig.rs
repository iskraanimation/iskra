//! Rig import (plan §4.17): a rig from the scene of another animation program brought into a
//! shot, as near as it goes: its pegs, its drawing nodes as layers with their drawings, its
//! composites and cutters, the mattes of its drawing nodes as cutters, wired as in the scene with its groups looked through, its chains of
//! deformers as deformers that the layers and pegs wired from them are bent by and follow, its
//! backdrops as groups, and its palettes. Only the rig: every value is as it is on the scene's first frame, and nothing is
//! keyed. What is not brought is said in a report, by the node's name.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use iskra_drawing::{ArtLayerKind, Drawing};
use iskra_io::{RigNode, RigScene, RigWired};
use iskra_model::{
    Animation, Composite, CompositeId, Deformer, DeformerId, DrawingId, Effect, EffectId, Group, GroupId, LayerId, Param, Peg, PegId, PoolDrawing, Project,
    Role, ShotId, Source, Stacking, Transform, effect_kind, unique_name,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{invalid, loaded, parse, shot_id, title};
use crate::{Result, Session};

/// What a node of the scene is made into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Peg,
    /// A drawing node: a layer.
    Drawings,
    /// A node that takes one art layer out of a drawing node's: that layer's output of it, or
    /// for underlay art a layer of its own, beside the drawing node's.
    Art(Part),
    Composite,
    Cutter,
    /// Lets the picture on its first port through.
    PassPicture,
    /// A node of a chain of deformers: where it starts, a curve, a bone. The chain is a
    /// deformer of the drawing nodes it is wired into, and what it carries goes on with what is
    /// on its first port.
    Deform,
    /// Takes the movement of the end of a chain of deformers, or of a peg that follows one, to
    /// the pegs it is wired into: they follow the chain's end, and carry what is under them
    /// without bending it.
    Kinematic,
    /// Moves what it carries in a way not brought: what it carries goes on with what is on its
    /// first port.
    PassMove(&'static str),
    /// What the scene is seen through: the picture on its first port is the rig's.
    Display,
    /// Not brought, and said so.
    Left,
}

/// An art layer that a node takes out of a drawing node's drawings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Line,
    Colour,
    Under,
    Over,
}

impl Part {
    fn title(self) -> &'static str {
        match self {
            Part::Line => "Line Art",
            Part::Colour => "Colour Art",
            Part::Under => "Underlay",
            Part::Over => "Overlay",
        }
    }
}

/// The pixels of a cutter's Alpha Size a matte resize's radius of 1 is brought as. Its unit is
/// not known: the user's rig, with a radius of 0.6 on its hair's mattes, matched Harmony at
/// 2 px (2026-10-30).
const MATTE_RESIZE_PIXELS: f64 = 2.0 / 0.6;

fn kind(node: &RigNode) -> Kind {
    match node.kind.as_str() {
        "PEG" => Kind::Peg,
        "READ" => Kind::Drawings,
        "LINE_ART" => Kind::Art(Part::Line),
        "COLOR_ART" => Kind::Art(Part::Colour),
        "UNDERLAY" => Kind::Art(Part::Under),
        "OVERLAY" => Kind::Art(Part::Over),
        "COMPOSITE" | "MATTE_COMPOSITE" => Kind::Composite,
        "CUTTER" => Kind::Cutter,
        "VISIBILITY" | "MATTE_RESIZE" => Kind::PassPicture,
        "DISPLAY" => Kind::Display,
        "WRITE" => Kind::Left,
        "OffsetModule" | "CurveModule" | "BendyBoneModule" => Kind::Deform,
        "ArticulationModule" => Kind::PassMove("a deformer of a kind not brought"),
        "KinematicOutputModule" => Kind::Kinematic,
        kind if kind.contains("Constraint") => Kind::PassMove("a constraint, not brought"),
        _ => Kind::Left,
    }
}

/// A node's transform on the scene's first frame, in the shot's units: its values under
/// `at` (`position` for a peg, `offset` for a drawing node). The scene's y is up and its
/// angles turn anticlockwise; the shot's y is down and its angles turn clockwise.
fn transform(scene: &RigScene, node: &RigNode, at: &str, units: [f64; 3]) -> Transform {
    let number = |path: &str, or: f64| scene.number(node, path).unwrap_or(or);
    let [x, y, z] = if at == "position" { scene.position(node) } else { ["x", "y", "z"].map(|axis| number(&format!("{at}.{axis}"), 0.0)) };
    let scale = match node.attrs.switch("scale.separate") {
        Some(false) => [number("scale.xy", 1.0); 2],
        _ => [number("scale.x", 1.0), number("scale.y", 1.0)],
    };
    let angle = scene.number(node, "rotation.anglez").or_else(|| scene.number(node, "angle")).unwrap_or(0.0);
    Transform {
        position: [x * units[0], -y * units[1]],
        rotation: -angle,
        scale,
        skew: number("skew", 0.0),
        pivot: [number("pivot.x", 0.0) * units[0], -number("pivot.y", 0.0) * units[1]],
        depth: z * units[2],
        ..Transform::default()
    }
}

/// A chain of deformer nodes, from its start, as a deformer at rest: a curve through the points
/// of its curve nodes, with their handles, or a chain of bones. The scene lays a chain out in
/// its grid's units as if they were square, and the shot's are not: the points and handles are
/// worked out there and then brought to the shot's units, as a position is.
fn deformer_of(chain: &[&RigNode], units: [f64; 3], id: DeformerId, name: String) -> std::result::Result<Deformer, String> {
    let number = |node: &RigNode, path: &str| node.attrs.number(path).unwrap_or(0.0);
    let point = |node: &RigNode, at: &str| [number(node, &format!("{at}.x")), number(node, &format!("{at}.y"))];
    let toward = |degrees: f64, length: f64| [length * degrees.to_radians().cos(), length * degrees.to_radians().sin()];
    let add = |a: [f64; 2], b: [f64; 2]| [a[0] + b[0], a[1] + b[1]];
    let shot = |[x, y]: [f64; 2]| [x * units[0], -y * units[1]];
    let bones = chain.iter().any(|node| node.kind == "BendyBoneModule");
    if bones {
        // From where the chain starts, each bone at an angle from the one before, the first
        // from the start's direction, or with no start from the grid's across.
        let (mut joints, mut radii): (Vec<[f64; 2]>, Vec<f64>) = (Vec::new(), Vec::new());
        let (mut end, mut angle, mut started) = ([0.0; 2], 0.0, false);
        for node in chain {
            match node.kind.as_str() {
                "OffsetModule" => {
                    (end, angle, started) = (point(node, "restingoffset"), number(node, "restingorientation"), true);
                }
                "BendyBoneModule" => {
                    let off = point(node, "restoffset");
                    let start = if joints.is_empty() && !started { off } else { add(end, off) };
                    angle = if joints.is_empty() && !started { number(node, "restorientation") } else { angle + number(node, "restorientation") };
                    if joints.is_empty() {
                        joints.push(start);
                    }
                    end = add(start, toward(angle, number(node, "restlength")));
                    joints.push(end);
                    radii.push(number(node, "restradius"));
                }
                _ => return Err("a chain of bones with a curve in it, which is not brought".to_string()),
            }
        }
        let joints: Vec<iskra_model::Point> = joints.into_iter().map(shot).map(|[x, y]| iskra_model::Point::new(x, y)).collect();
        let mut deformer = Deformer::new(id, name, [0.0; 2], Vec::new()).with_rest_joints(&joints, 0.0);
        // How round each joint is: the radius of the bone that starts there.
        for (bone, radius) in deformer.bones.iter_mut().zip(radii).skip(1) {
            bone.round = radius * units[0];
            bone.fade = bone.round * 2.0;
        }
        return Ok(deformer);
    }
    // A curve: a point where it starts, and one where each curve node ends; each curve node's
    // first handle leaves the point before, its second arrives at its own.
    let (mut points, mut outs, mut intos): (Vec<[f64; 2]>, Vec<Option<[f64; 2]>>, Vec<Option<[f64; 2]>>) = (Vec::new(), Vec::new(), Vec::new());
    let mut reach = 0.0;
    for node in chain {
        match node.kind.as_str() {
            "OffsetModule" => {
                points.push(point(node, "restingoffset"));
                (outs, intos) = (vec![None], vec![None]);
            }
            "CurveModule" => {
                if points.is_empty() {
                    (points, outs, intos) = (vec![[0.0; 2]], vec![None], vec![None]);
                }
                let before = *points.last().expect("a point");
                *outs.last_mut().expect("a point") = Some(toward(number(node, "restingorientation0"), number(node, "restlength0")));
                let at = point(node, "restingoffset");
                points.push(if node.attrs.switch("localreferential") == Some(true) { add(before, at) } else { at });
                let [x, y] = toward(number(node, "restingorientation1"), number(node, "restlength1"));
                (intos, outs) = ([intos, vec![Some([-x, -y])]].concat(), [outs, vec![None]].concat());
                // How far to the side it carries the drawing, where its reach is not endless.
                if node.attrs.get("influencetype").and_then(|kind| kind.value.as_deref()).is_some_and(|kind| kind != "INFINITE_INFLUENCE") {
                    reach = number(node, "transversalradius") * units[0];
                }
            }
            _ => return Err("a curve with a bone in it, which is not brought".to_string()),
        }
    }
    if points.len() < 2 {
        return Err("a chain with no curve in it".to_string());
    }
    let mut deformer = Deformer::curve(id, name, &points.iter().copied().map(shot).collect::<Vec<_>>());
    let neg = |[x, y]: [f64; 2]| [-x, -y];
    for (knot, (into, out)) in deformer.spine.iter_mut().zip(intos.into_iter().zip(outs)) {
        // A point with a handle on one side passes smoothly: the other is as long, the other way.
        let (into, out) = (into.or(out.map(neg)), out.or(into.map(neg)));
        if let (Some(into), Some(out)) = (into, out) {
            knot.bend = Some(iskra_model::Bend { into: shot(into), out: shot(out) });
        }
        (knot.reach, knot.fade) = (reach, reach * 1.5);
    }
    Ok(deformer)
}

/// Whether a transform does anything.
fn moves(transform: &Transform) -> bool {
    let plain = Transform::default();
    let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
    !(near(transform.position[0], 0.0)
        && near(transform.position[1], 0.0)
        && near(transform.rotation, 0.0)
        && near(transform.scale[0], plain.scale[0])
        && near(transform.scale[1], plain.scale[1])
        && near(transform.skew, 0.0)
        && near(transform.depth, 0.0))
}

fn peg(id: PegId, name: String, transform: Transform) -> Peg {
    Peg {
        id,
        name,
        parent: None,
        transform,
        animation: Animation::default(),
        template: None,
        follows: None,
        inherit: Default::default(),
        matches: Default::default(),
    }
}

/// Which pictures of a drawing a layer's drawing is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Which {
    /// Its line art, its colour art, and its overlay art as its overart.
    Drawing,
    /// Its underlay art, as line art.
    Under,
}

/// The pictures the drawing converter made of the scene's drawings (see
/// `iskra_io::convert_drawings`), made into drawings: each picture's middle at the drawing's
/// origin, its pixels given the nearest swatch of the rig's palettes, their alpha as how much
/// of the pixel is painted. A drawing made once is shared by the layers that show it.
struct Pictures {
    folder: Option<PathBuf>,
    swatches: Vec<(iskra_drawing::SwatchId, [u8; 3])>,
    made: std::cell::RefCell<HashMap<(u32, String, Which), Arc<Drawing>>>,
    /// The pictures that were looked for and not there, or not read.
    missing: std::cell::Cell<usize>,
    /// Of each art layer, how many pictures had art, were empty, and were not there: to tell
    /// drawings without the art from a converter that did not make it.
    read: std::cell::RefCell<HashMap<iskra_io::RigPass, [usize; 3]>>,
}

impl Pictures {
    fn new(p: &Project, folder: Option<PathBuf>, palettes: &[String]) -> Self {
        let palette = &p.palette;
        let sets: Vec<_> = palette.sets.iter().filter(|set| palettes.contains(&set.name)).map(|set| set.id).collect();
        let of = |swatch: &iskra_model::Swatch| (swatch.id, [swatch.color[0], swatch.color[1], swatch.color[2]]);
        let mut swatches: Vec<_> = sets.iter().flat_map(|set| palette.swatches_of(*set)).map(of).collect();
        if swatches.is_empty() {
            swatches = palette.swatches.iter().map(of).collect();
        }
        Self { folder, swatches, made: Default::default(), missing: Default::default(), read: Default::default() }
    }

    /// The swatch nearest a colour.
    fn swatch(&self, [r, g, b, _]: [u8; 4]) -> iskra_drawing::SwatchId {
        let distance = |[sr, sg, sb]: [u8; 3]| [(sr, r), (sg, g), (sb, b)].iter().map(|(a, b)| (i32::from(*a) - i32::from(*b)).pow(2)).sum::<i32>();
        self.swatches.iter().min_by_key(|(_, colour)| distance(*colour)).map_or(iskra_drawing::SwatchId(1), |(id, _)| *id)
    }

    /// One picture as indexed art, or none where it isn't there.
    fn art(&self, element: &iskra_io::RigElement, drawing: &str, pass: iskra_io::RigPass) -> Option<iskra_drawing::ArtContent> {
        let path = iskra_io::picture_path(self.folder.as_ref()?, element, drawing, pass);
        let count = |at: usize| self.read.borrow_mut().entry(pass).or_default()[at] += 1;
        let Ok((width, height, rgba)) = iskra_io::read_picture(&path) else {
            count(2);
            // A drawing with no overlay art may have no picture of it.
            if pass != iskra_io::RigPass::Over {
                self.missing.set(self.missing.get() + 1);
            }
            return None;
        };
        count(if rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0) { 0 } else { 1 });
        let (left, top) = (-(width as i32 / 2), -(height as i32 / 2));
        let bounds = iskra_drawing::Rect::new(left, top, left + width as i32, top + height as i32);
        let pixels: Vec<[u8; 4]> = rgba.as_chunks::<4>().0.to_vec();
        Some(iskra_drawing::ArtContent::Indexed(iskra_drawing::IndexedSurface::of_rgba8(bounds, &pixels, |colour| self.swatch(colour))))
    }

    /// The drawing a layer shows of a drawing of the scene: empty with no pictures.
    fn drawing(&self, element: &iskra_io::RigElement, name: &str, which: Which) -> Arc<Drawing> {
        let key = (element.id, name.to_string(), which);
        if let Some(made) = self.made.borrow().get(&key) {
            return made.clone();
        }
        let mut drawing = Drawing::default();
        if self.folder.is_some() {
            use iskra_io::RigPass;
            let arts = match which {
                Which::Drawing => {
                    vec![(RigPass::Line, ArtLayerKind::LineArt), (RigPass::Colour, ArtLayerKind::ColourArt), (RigPass::Over, ArtLayerKind::Overart)]
                }
                Which::Under => vec![(RigPass::Under, ArtLayerKind::LineArt)],
            };
            for (pass, kind) in arts {
                if let (Some(art), Some(layer)) = (self.art(element, name, pass), drawing.layer_mut(kind)) {
                    layer.content = art;
                }
            }
        }
        let drawing = Arc::new(drawing);
        self.made.borrow_mut().insert(key, drawing.clone());
        drawing
    }
}

/// What the import made, and what it said.
#[derive(Default)]
struct Made {
    pegs: Vec<PegId>,
    layers: Vec<LayerId>,
    composites: Vec<CompositeId>,
    effects: Vec<EffectId>,
    deformers: Vec<DeformerId>,
    groups: Vec<GroupId>,
    palettes: Vec<String>,
    report: Vec<String>,
}

/// Bring the rig into the shot.
fn bring(p: &mut Project, shot: ShotId, scene: &RigScene, pictures: &Pictures, made: &mut Made) {
    let nodes = scene.wired();
    let kinds: Vec<Kind> = nodes.iter().map(|wired| kind(wired.node)).collect();
    let units = scene.unit_size(p.settings.height);
    let mut names: Vec<String> = p.shot(shot).into_iter().flat_map(|shot| shot.node_names()).map(str::to_string).collect();
    let mut named = |name: &str| {
        let name = unique_name(if name.is_empty() { "Node" } else { name }, names.iter().map(String::as_str));
        names.push(name.clone());
        name
    };
    let whole = p.shot(shot).map_or(1, |shot| shot.duration.max(1));
    let where_is = |wired: &RigWired| if wired.path.is_empty() { wired.node.name.clone() } else { format!("{}/{}", wired.path.join("/"), wired.node.name) };

    // The pegs, the composites and the cutters first, so that what is wired can name them. A
    // drawing node with a matte on its second port is clipped to it: a cutter after its layer.
    let (mut pegs, mut layers, mut composites, mut effects) = (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new());
    let mut mattes: HashMap<usize, EffectId> = HashMap::new();
    let mut made_pegs: Vec<Peg> = Vec::new();
    for (at, (wired, kind)) in nodes.iter().zip(&kinds).enumerate() {
        match kind {
            Kind::Peg => {
                let id = PegId(p.new_id());
                made_pegs.push(peg(id, named(&wired.node.name), transform(scene, wired.node, "position", units)));
                pegs.insert(at, id);
            }
            Kind::Composite => _ = composites.insert(at, CompositeId(p.new_id())),
            Kind::Cutter => _ = effects.insert(at, EffectId(p.new_id())),
            Kind::Drawings if wired.inputs.iter().any(|(port, _)| *port == 1) => _ = mattes.insert(at, EffectId(p.new_id())),
            Kind::PassPicture if wired.node.kind == "MATTE_RESIZE" => {
                let radius = scene.number(wired.node, "radius").unwrap_or(0.0);
                let size = radius * MATTE_RESIZE_PIXELS;
                made.report.push(format!(
                    "{}: a matte resize, its radius ({radius}) brought as {size:.1} px of Alpha Size on the cutters it feeds, a guess",
                    where_is(wired)
                ));
            }
            Kind::Left if wired.node.kind != "WRITE" => {
                made.report.push(format!("{}: not brought, a node of a kind this import doesn't know ({})", where_is(wired), wired.node.kind));
            }
            Kind::PassMove(why) => made.report.push(format!("{}: {why}; what it moves goes on with what is on its first port", where_is(wired))),
            _ => {}
        }
    }
    // What carries a node: the peg on its first port, through what moves it in ways not brought.
    let carrier = |at: usize| -> Option<PegId> {
        let mut on = nodes[at].inputs.first().filter(|(port, _)| *port == 0).map(|(_, from)| *from);
        for _ in 0..64 {
            let from = on?;
            match kinds[from] {
                Kind::Peg => return pegs.get(&from).copied(),
                Kind::PassMove(_) | Kind::Kinematic | Kind::Deform => on = nodes[from].inputs.first().map(|(_, from)| *from),
                _ => return None,
            }
        }
        None
    };
    for (at, id) in &pegs {
        let parent = carrier(*at);
        if let Some(made) = made_pegs.iter_mut().find(|peg| peg.id == *id) {
            made.parent = parent;
        }
    }

    // A drawing node is a layer with all of its element's drawings, empty until their pictures
    // are brought, their overlay art as its overart, as the node shows it whether an Overlay
    // node takes it out or not; the one it shows on the first frame exposed for the whole shot. Its own
    // transform, where it has one, is a peg of its own over it.
    let drawings_of = |p: &mut Project, wired: &RigWired, layer: &mut iskra_model::Layer, which: Which, made: &mut Made| {
        let Some((element, shown)) = scene.drawing(wired.node) else {
            made.report.push(format!("{}: a drawing node that shows no drawings", where_is(wired)));
            return;
        };
        for name in &element.drawings {
            let id = DrawingId(p.new_id());
            layer.drawings.push(PoolDrawing { id, name: name.clone(), drawing: pictures.drawing(element, name, which) });
            if Some(name.as_str()) == shown {
                layer.exposure.set(0, whole, Some(id));
            }
        }
    };
    let mut made_layers = Vec::new();
    let mut own_pegs: HashMap<usize, PegId> = HashMap::new();
    for (at, (wired, kind)) in nodes.iter().zip(&kinds).enumerate() {
        if *kind != Kind::Drawings {
            continue;
        }
        let mut layer = p.new_layer(&named(&wired.node.name), Role::Animation);
        // Its pictures are given the rig's swatches, so it is swatch-linked bitmap.
        (layer.parent, layer.free_colour) = (carrier(at), Vec::new());
        let own = transform(scene, wired.node, "offset", units);
        if moves(&own) {
            let id = PegId(p.new_id());
            let mut over = peg(id, named(&format!("{}-Drawing", wired.node.name)), own);
            over.parent = layer.parent;
            made_pegs.push(over);
            own_pegs.insert(at, id);
            layer.parent = Some(id);
        }
        drawings_of(p, wired, &mut layer, Which::Drawing, made);
        layers.insert(at, layer.id);
        made_layers.push(layer);
    }
    // A node taking one art layer out of a drawing node's is that layer's output of it, and no
    // layer of its own: line art, colour art, or overlay art as overart. A drawing has no
    // underlay art, so an Underlay node is a layer beside it, carried as it is, with the same
    // drawings, their underlay art brought into their line art and shown alone.
    let mut arts: HashMap<usize, Source> = HashMap::new();
    for (at, (wired, kind)) in nodes.iter().zip(&kinds).enumerate() {
        let Kind::Art(part) = kind else { continue };
        let Some(of) = wired.inputs.first().map(|(_, from)| *from).filter(|from| kinds[*from] == Kind::Drawings) else {
            made.report.push(format!("{}: takes the {} of no drawing node", where_is(wired), part.title().to_lowercase()));
            continue;
        };
        let source = &nodes[of];
        let output = match part {
            Part::Line => ArtLayerKind::LineArt,
            Part::Colour => ArtLayerKind::ColourArt,
            Part::Over => ArtLayerKind::Overart,
            Part::Under => {
                let base = made_layers.iter().find(|layer| Some(&layer.id) == layers.get(&of)).map(|layer| (layer.name.clone(), layer.parent));
                let Some((base, parent)) = base else { continue };
                let mut layer = p.new_layer(&named(&format!("{base} {}", part.title())), Role::Animation);
                (layer.parent, layer.free_colour) = (parent, Vec::new());
                layer.art_off = vec![ArtLayerKind::ColourArt, ArtLayerKind::Overart];
                drawings_of(p, source, &mut layer, Which::Under, made);
                layers.insert(at, layer.id);
                made_layers.push(layer);
                continue;
            }
        };
        arts.extend(layers.get(&of).map(|layer| (at, Source::Art(*layer, output))));
    }

    // The picture a node gives: its layer's, its composite's, its cutter's, or what it lets
    // through.
    let picture = |at: usize| -> Option<Source> {
        let mut on = Some(at);
        for _ in 0..64 {
            let at = on?;
            match kinds[at] {
                Kind::Art(Part::Under) => return layers.get(&at).map(|id| Source::Layer(*id)),
                Kind::Art(_) => return arts.get(&at).copied(),
                Kind::Drawings => return mattes.get(&at).map(|id| Source::Effect(*id)).or_else(|| layers.get(&at).map(|id| Source::Layer(*id))),
                Kind::Composite => return composites.get(&at).map(|id| Source::Composite(*id)),
                Kind::Cutter => return effects.get(&at).map(|id| Source::Effect(*id)),
                Kind::PassPicture => on = nodes[at].inputs.first().map(|(_, from)| *from),
                _ => return None,
            }
        }
        None
    };
    // How much wider the matte from a node is made by the matte resizes it goes through on its
    // way, in pixels.
    let resized = |at: usize| -> f64 {
        let (mut on, mut size) = (Some(at), 0.0);
        for _ in 0..64 {
            let Some(at) = on.filter(|at| kinds[*at] == Kind::PassPicture) else { break };
            if nodes[at].node.kind == "MATTE_RESIZE" {
                size += scene.number(nodes[at].node, "radius").unwrap_or(0.0) * MATTE_RESIZE_PIXELS;
            }
            on = nodes[at].inputs.first().map(|(_, from)| *from);
        }
        size
    };
    let grown = |effect: &mut Effect, matte: usize| {
        let size = resized(matte);
        if size != 0.0 {
            effect.params.insert("grow".to_string(), Param::of(iskra_model::Value::Number(size)));
        }
    };
    let mut made_composites = Vec::new();
    for (at, id) in &composites {
        let wired = &nodes[*at];
        // The scene stacks a composite's rightmost port on top, and a composite's inputs are
        // listed the top first.
        let mut inputs: Vec<Source> = Vec::new();
        for source in wired.inputs.iter().rev().filter_map(|(_, from)| picture(*from)) {
            if !inputs.contains(&source) {
                inputs.push(source);
            }
        }
        let mut composite = Composite::new(*id, &named(&wired.node.name), inputs);
        // A composite that passes through keeps what it stacks apart, ordered by depth; one
        // that flattens orders by its ports, the rightmost on top.
        let through = wired.node.attrs.get("compositeMode").and_then(|mode| mode.value.as_deref()).is_none_or(|mode| mode == "compositePassthrough");
        composite.stacking = if through { Stacking::Depth } else { Stacking::Ports };
        made_composites.push((*at, composite));
    }
    made_composites.sort_by_key(|(at, _)| *at);
    let mut made_effects = Vec::new();
    let cutter = effect_kind("cutter").expect("the cutter is a kind of effect");
    for (at, id) in &effects {
        let wired = &nodes[*at];
        let mut effect = Effect::new(*id, &named(&wired.node.name), cutter);
        // The picture on its first port, cut by the matte on its second.
        for (port, name) in [(0, iskra_model::PICTURE), (1, iskra_model::MATTE)] {
            if let Some(source) = wired.inputs.iter().find(|(on, _)| *on == port).and_then(|(_, from)| picture(*from)) {
                effect.inputs.insert(name.to_string(), source);
            }
        }
        if let Some(&(_, matte)) = wired.inputs.iter().find(|(on, _)| *on == 1) {
            grown(&mut effect, matte);
        }
        if wired.node.attrs.switch("inverted") == Some(true) {
            effect.params.insert("inverted".to_string(), Param::of(iskra_model::Value::Switch(true)));
        }
        made_effects.push((*at, effect));
    }
    // A drawing node's matte port: its layer cut by the matte, keeping what is under it.
    for (at, id) in &mattes {
        let wired = &nodes[*at];
        let mut effect = Effect::new(*id, &named(&format!("{}-Matte", wired.node.name)), cutter);
        effect.inputs.extend(layers.get(at).map(|layer| (iskra_model::PICTURE.to_string(), Source::Layer(*layer))));
        let matte = wired.inputs.iter().find(|(on, _)| *on == 1).map(|(_, from)| *from);
        effect.inputs.extend(matte.and_then(picture).map(|source| (iskra_model::MATTE.to_string(), source)));
        if let Some(matte) = matte {
            grown(&mut effect, matte);
        }
        effect.params.insert("inverted".to_string(), Param::of(iskra_model::Value::Switch(true)));
        made_effects.push((*at, effect));
    }
    made_effects.sort_by_key(|(at, _)| *at);
    // What the scene is seen through, at its top: shown on top of the shot.
    let shown = nodes.iter().zip(&kinds).find(|(wired, kind)| **kind == Kind::Display && wired.path.is_empty()).and_then(|(wired, _)| {
        let from = wired.inputs.first()?.1;
        picture(from)
    });
    if shown.is_none() {
        made.report.push("Nothing of the rig is connected to the scene's display: it is in the node view, and not seen".to_string());
    }

    // The deformers: the chain of deformer nodes on a drawing node's first port, from where it
    // starts to the node the drawing node takes it from, one deformer of every layer it bends.
    // A chain wired into a peg is followed by it, to its end: straight, the peg bends the
    // drawings right under it as the chain does its own; through a kinematic output, the peg
    // only carries them. A kinematic output after a peg that follows a chain is the end of
    // the chain already, which carries the pegs under it.
    let chain_on = |at: usize| nodes[at].inputs.first().filter(|(port, _)| *port == 0).map(|(_, from)| *from).filter(|from| kinds[*from] == Kind::Deform);
    let mut follows: Vec<(PegId, usize, bool)> = Vec::new();
    for (at, id) in &pegs {
        let Some(&(_, from)) = nodes[*at].inputs.first() else { continue };
        match kinds[from] {
            Kind::Deform => follows.push((*id, from, true)),
            Kind::Kinematic => match (chain_on(from), nodes[from].inputs.first().map(|(_, before)| kinds[*before])) {
                (Some(last), _) => follows.push((*id, last, false)),
                (None, Some(Kind::Peg)) => {}
                _ => made.report.push(format!("{}: takes the movement of no chain of deformers nor peg", where_is(&nodes[from]))),
            },
            _ => {}
        }
    }
    follows.sort_by_key(|(id, ..)| id.0);
    let mut deformers: HashMap<usize, Option<DeformerId>> = HashMap::new();
    let mut chained: HashMap<usize, DeformerId> = HashMap::new();
    let mut made_deformers: Vec<Deformer> = Vec::new();
    let bent = kinds.iter().enumerate().filter_map(|(at, kind)| {
        let read = match kind {
            Kind::Drawings => at,
            Kind::Art(_) => match nodes[at].inputs.first() {
                Some(&(_, from)) if kinds[from] == Kind::Drawings => from,
                _ => return None,
            },
            _ => return None,
        };
        Some((Some(at), chain_on(read)?))
    });
    let followed: Vec<(Option<usize>, usize)> = follows.iter().map(|(_, last, _)| (None, *last)).collect();
    for (at, last) in bent.collect::<Vec<_>>().into_iter().chain(followed) {
        let id = *deformers.entry(last).or_insert_with(|| {
            let mut chain = vec![last];
            while let Some(&(_, before)) =
                nodes[*chain.last().expect("one")].inputs.first().filter(|(_, from)| kinds[*from] == Kind::Deform && chain.len() < 64)
            {
                chain.push(before);
            }
            chain.reverse();
            // Named after the group the chain is in, as the scene groups each chain.
            let name = nodes[last].path.last().map_or_else(|| nodes[last].node.name.clone(), |group| group.to_string());
            let links = chain.clone();
            let chain: Vec<&RigNode> = chain.iter().map(|at| nodes[*at].node).collect();
            match deformer_of(&chain, units, DeformerId(p.new_id()), named(&name)) {
                Ok(deformer) => {
                    let id = deformer.id;
                    chained.extend(links.into_iter().map(|at| (at, id)));
                    made_deformers.push(deformer);
                    Some(id)
                }
                Err(why) => {
                    made.report.push(format!("{}: {why}", where_is(&nodes[last])));
                    None
                }
            }
        });
        if let Some(layer) = at.and_then(|at| layers.get(&at)).and_then(|id| made_layers.iter_mut().find(|layer| layer.id == *id)) {
            layer.deformer = id;
        }
    }
    for (peg, last, bends) in follows {
        let (Some(Some(deformer)), Some(peg)) = (deformers.get(&last), made_pegs.iter_mut().find(|made| made.id == peg)) else { continue };
        peg.follows = Some(iskra_model::Follow { deformer: *deformer, point: None, bends });
    }

    // The backdrops: each a group of what was made of the nodes inside it, and of the groups
    // of the backdrops inside it, there or in a group of the scene that lies inside it. What
    // two hold is in the one further in, or else the smaller.
    let backdrops = scene.backdrops();
    let innermost = |of: &mut dyn Iterator<Item = usize>| {
        of.max_by(|a, b| {
            let (a, b) = (&backdrops[*a], &backdrops[*b]);
            a.0.len().cmp(&b.0.len()).then(b.1.area().total_cmp(&a.1.area()))
        })
    };
    let held_by = |inside: &[&str], at: Option<[f64; 2]>, not: Option<usize>| {
        let holding = (0..backdrops.len()).filter(|other| Some(*other) != not).filter(|other| {
            let (path, backdrop) = &backdrops[*other];
            scene.place(path, inside, at).is_some_and(|place| backdrop.contains(place))
                && not.is_none_or(|not| path.len() < inside.len() || backdrop.area() > backdrops[not].1.area())
        });
        innermost(&mut holding.into_iter())
    };
    let mut members: Vec<Vec<u64>> = vec![Vec::new(); backdrops.len()];
    for (at, wired) in nodes.iter().enumerate() {
        let made_of = [
            pegs.get(&at).map(|id| id.0),
            own_pegs.get(&at).map(|id| id.0),
            layers.get(&at).map(|id| id.0),
            composites.get(&at).map(|id| id.0),
            effects.get(&at).map(|id| id.0),
            mattes.get(&at).map(|id| id.0),
            chained.get(&at).map(|id| id.0),
        ];
        let Some(held) = held_by(&wired.path, wired.node.pos, None) else { continue };
        for id in made_of.into_iter().flatten() {
            if !members.iter().any(|ids| ids.contains(&id)) {
                members[held].push(id);
            }
        }
    }
    let within: Vec<Option<usize>> = backdrops.iter().enumerate().map(|(at, (path, backdrop))| held_by(path, Some(backdrop.middle()), Some(at))).collect();
    // A backdrop round nothing that was made is no group, nor one round only such backdrops.
    let mut kept: Vec<bool> = members.iter().map(|ids| !ids.is_empty()).collect();
    while let Some(at) = (0..backdrops.len()).find(|at| !kept[*at] && (0..backdrops.len()).any(|inner| kept[inner] && within[inner] == Some(*at))) {
        kept[at] = true;
    }
    let ids: Vec<GroupId> = backdrops.iter().map(|_| GroupId(p.new_id())).collect();
    let mut made_groups = Vec::new();
    for at in (0..backdrops.len()).filter(|at| kept[*at]) {
        let mut holds: std::collections::BTreeSet<u64> = members[at].iter().copied().collect();
        holds.extend((0..backdrops.len()).filter(|inner| kept[*inner] && within[*inner] == Some(at)).map(|inner| ids[inner].0));
        let title = backdrops[at].1.title.trim();
        let name = named(if title.is_empty() { "Backdrop" } else { title });
        made_groups.push(Group { id: ids[at], name, members: holds, folded: false, rows_folded: false, board: false, template: None });
    }

    made.pegs = made_pegs.iter().map(|peg| peg.id).collect();
    made.groups = made_groups.iter().map(|group| group.id).collect();
    made.deformers = made_deformers.iter().map(|deformer| deformer.id).collect();
    made.layers = made_layers.iter().map(|layer| layer.id).collect();
    made.composites = made_composites.iter().map(|(_, composite)| composite.id).collect();
    made.effects = made_effects.iter().map(|(_, effect)| effect.id).collect();
    let Some(shot) = p.shot_mut(shot) else { return };
    shot.pegs.extend(made_pegs.into_iter().map(Arc::new));
    for layer in made_layers {
        shot.layers.insert(0, Arc::new(layer));
    }
    shot.composites.extend(made_composites.into_iter().map(|(_, composite)| Arc::new(composite)));
    shot.effects.extend(made_effects.into_iter().map(|(_, effect)| Arc::new(effect)));
    shot.deformers.extend(made_deformers.into_iter().map(Arc::new));
    shot.groups.extend(made_groups.into_iter().map(Arc::new));
    if let Some(shown) = shown {
        match shot.main_composite() {
            Some(main) => {
                if let Some(composite) = shot.composites.iter_mut().find(|composite| composite.id == main) {
                    Arc::make_mut(composite).inputs.insert(0, shown);
                }
            }
            None => made.report.push("The shot has no composite to show the rig in: it is in the node view, and not seen".to_string()),
        }
    }
}

/// Add the scene's palettes to the project, each a named palette of its own, named after its
/// file. A colour of a kind with no colour of its own is left out, and said so.
fn palettes(p: &mut Project, scene: &RigScene, folder: Option<&std::path::Path>, made: &mut Made) {
    for path in scene.palettes() {
        let name = path.file_stem().map_or("Palette".to_string(), |stem| stem.to_string_lossy().into_owned());
        let read = super::palette::plt_into(p, &path, folder);
        let (palette, left) = match read {
            Ok(read) => read,
            Err(why) => {
                made.report.push(format!("Palette {name}: not brought, {why}"));
                continue;
            }
        };
        for left in left {
            made.report.push(format!("Palette {name}: {left} left out"));
        }
        let name = unique_name(&name, p.palette.sets.iter().map(|set| set.name.as_str()));
        let palette_of = Arc::make_mut(&mut p.palette);
        let Some(set) = palette_of.new_set(&name) else {
            made.report.push(format!("Palette {name}: not brought, the project can hold no more palettes"));
            continue;
        };
        palette_of.merge(&palette, false, set);
        let media = p.media.clone();
        super::palette::read_pictures(Arc::make_mut(&mut p.palette), &media, folder, None);
        made.palettes.push(name);
    }
}

/// Brings a rig into a shot from the scene file at `path` (plan §4.17): its pegs as pegs, its
/// drawing nodes as layers with every drawing of their elements, empty, the one each shows on
/// the scene's first frame exposed; a node taking line, colour or overlay art out of a
/// drawing node's as that layer's output of it, and one taking underlay art as a layer of its
/// own beside it; its composites and matte composites as composites, its cutters
/// as cutters, a drawing node with a matte on its second port as its layer cut by that matte,
/// keeping what is under it, wired as in the scene with its groups looked through; and its palettes as named
/// palettes. What the scene shows is put on top of the shot's main composite. Every value is
/// as it is on the scene's first frame: nothing is keyed.
///
/// With `pictures`, the folder `iskra_io::convert_drawings` made the pictures of the scene's
/// drawings in, the drawings are those pictures, as indexed art of the rig's palettes; without,
/// or where a picture is not there, they are empty.
///
/// Returns the ids of the `pegs`, `layers`, `composites` and `effects` that were made, the
/// names of the `palettes`, how many shot units one of the scene's units is (`units`, across,
/// up and in depth), and a `report` of what was not brought and why, a line each.
pub(super) fn import(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        path: PathBuf,
        shot: Option<u64>,
        pictures: Option<PathBuf>,
    }
    let id = "rig.import";
    let Params { path, shot, pictures } = parse(id, params)?;
    let shot = shot_id(s.project(), shot)?;
    if let Some(shot) = s.project().shot(shot) {
        loaded(shot)?;
    }
    let scene = match RigScene::read(&path) {
        Ok(scene) => scene,
        Err(why) => return invalid(why),
    };
    let units = scene.unit_size(s.project().settings.height);
    let folder = super::media::folder(s);
    let made = s.edit(title(id), |p| {
        let mut made = Made::default();
        // The palettes first, for the drawings' swatches; what is said of them after the nodes.
        palettes(p, &scene, folder.as_deref(), &mut made);
        let said_of_palettes = std::mem::take(&mut made.report);
        let pictures = Pictures::new(p, pictures.clone(), &made.palettes);
        bring(p, shot, &scene, &pictures, &mut made);
        made.report.extend(said_of_palettes);
        use iskra_io::RigPass;
        let read = pictures.read.borrow();
        for (pass, title) in [(RigPass::Line, "Line art"), (RigPass::Colour, "Colour art"), (RigPass::Under, "Underlay art"), (RigPass::Over, "Overlay art")] {
            if let Some([art, empty, gone]) = read.get(&pass) {
                made.report.push(format!("{title}: {art} pictures with art, {empty} empty, {gone} not made, of {}", art + empty + gone));
            }
        }
        drop(read);
        if pictures.missing.get() > 0 {
            made.report.push(format!("{} pictures of drawings were not made, or could not be read: those drawings are empty", pictures.missing.get()));
        }
        Ok(made)
    })?;
    let ids = |ids: Vec<u64>| json!(ids);
    Ok(json!({
        "pegs": ids(made.pegs.iter().map(|id| id.0).collect()),
        "layers": ids(made.layers.iter().map(|id| id.0).collect()),
        "composites": ids(made.composites.iter().map(|id| id.0).collect()),
        "effects": ids(made.effects.iter().map(|id| id.0).collect()),
        "deformers": ids(made.deformers.iter().map(|id| id.0).collect()),
        "groups": ids(made.groups.iter().map(|id| id.0).collect()),
        "palettes": made.palettes,
        "units": units,
        "report": made.report,
    }))
}
