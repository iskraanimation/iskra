//! Layer commands: the shot's drawing layers.

use std::sync::Arc;

use iskra_model::{ArtContent, ArtLayerKind, Layer, LayerId, Project, Role, Source, unique_name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::peg::parent_in;
use super::{checked_density, checked_name, invalid, loaded, name_is_free, parse, shot_id, title};
use crate::{Result, Session};

/// The layer a command names, to change. Its shot must be loaded.
pub(super) fn layer_mut(project: &mut Project, id: u64) -> Result<&mut Layer> {
    if let Some(shot) = project.shot_of(LayerId(id)) {
        loaded(shot)?;
    }
    match project.layer_mut(LayerId(id)) {
        Some(layer) => Ok(layer),
        None => invalid(format!("no layer {id}")),
    }
}

/// Adds a layer on top of the shot's others. Without a name it gets a free "Drawing" one.
/// It is an animation layer unless `role` says it is a board layer (plan §7.2c); a board
/// layer goes into the shot's group for its storyboard drawings, which is made if there is none,
/// with a peg of its own above it to animate it by.
/// With `group` it is put into that group of the shot, and with `at` at that place in the
/// node view, where otherwise the view lays it out.
pub(super) fn new(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        name: Option<String>,
        #[serde(default = "animation")]
        role: Role,
        group: Option<u64>,
        at: Option<[f64; 2]>,
        /// What it holds: pixels, vector art (plan §4.16), or texts (§4.19). A layer holds
        /// one kind.
        #[serde(default)]
        kind: Kind,
    }
    #[derive(Deserialize, Default, PartialEq)]
    #[serde(rename_all = "camelCase")]
    enum Kind {
        #[default]
        Bitmap,
        Vector,
        Text,
    }
    fn animation() -> Role {
        Role::Animation
    }
    let id = "layer.new";
    let Params { shot, name, role, group, at, kind } = parse(id, params)?;
    let layer = s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let name = match &name {
            Some(name) => {
                let name = checked_name(name)?;
                p.shot(shot).map_or(Ok(()), |shot| name_is_free(shot, name, None))?;
                name.to_string()
            }
            None => unique_name(if kind == Kind::Text { "Text" } else { "Drawing" }, p.shot(shot).into_iter().flat_map(|s| s.node_names())),
        };
        let mut layer = p.new_layer(&name, role);
        // A text's glyphs are vector art, and so is what may be drawn round them.
        if kind != Kind::Bitmap {
            (layer.vector, layer.free_colour) = (ArtLayerKind::ALL.to_vec(), Vec::new());
        }
        layer.text = kind == Kind::Text;
        let id = layer.id;
        if let Some(shot) = p.shot_mut(shot) {
            shot.layers.insert(0, Arc::new(layer));
            // Seen at once: on top of what the display shows.
            shot.connect_on_top(id);
        }
        // A board is with the shot's other storyboard drawings, in their group.
        if role == Role::Board {
            p.into_board_group(shot, id.0);
            p.peg_the_board(shot, id);
        }
        super::group::put_into(p, shot, group, at, id.0)?;
        Ok(id)
    })?;
    Ok(json!({ "id": layer }))
}

/// Copies a board layer into an animation layer of the same shot (plan §7.2c), on top of the
/// others: its drawings and their timing, under the pegs the board is under. Nothing is
/// shared afterwards, so cleaning up the copy leaves the board as it was. Without a `name`
/// the copy is named after the board.
pub(super) fn use_as_animation(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        name: Option<String>,
    }
    let id = "layer.useAsAnimation";
    let Params { layer, name } = parse(id, params)?;
    let copy = s.edit(title(id), |p| {
        let from = LayerId(layer);
        let Some(shot) = p.shot_of(from) else { return invalid(format!("no layer {layer}")) };
        loaded(shot)?;
        if !shot.is_board(from) {
            return invalid("the layer is an animation layer already");
        }
        let Some(board) = shot.layer(from) else { return invalid(format!("no layer {layer}")) };
        let name = match &name {
            Some(name) => {
                let name = checked_name(name)?;
                name_is_free(shot, name, None)?;
                name.to_string()
            }
            None => unique_name(&format!("{} Animation", board.name), shot.node_names()),
        };
        let (within, mut copy) = (shot.id, board.clone());
        (copy.id, copy.name, copy.role) = (iskra_model::LayerId(p.new_id()), name, Role::Animation);
        copy.renumber_drawings(|| p.new_id());
        let id = copy.id;
        if let Some(shot) = p.shot_mut(within) {
            shot.layers.insert(0, Arc::new(copy));
            shot.connect_on_top(id);
        }
        Ok(id)
    })?;
    Ok(json!({ "id": copy }))
}

pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        name: String,
    }
    let id = "layer.rename";
    let Params { layer, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        if let Some(shot) = p.shot_of(LayerId(layer)) {
            name_is_free(shot, name, Some(layer))?;
        }
        layer_mut(p, layer)?.name = name.to_string();
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Deletes a layer with its drawings.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
    }
    let id = "layer.delete";
    let Params { layer } = parse(id, params)?;
    s.edit(title(id), |p| {
        let Some(shot) = p.shot_of(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
        // Its drawings go with it, and undoing must bring them back.
        loaded(shot)?;
        let shot = shot.id;
        if let Some(shot) = p.shot_mut(shot) {
            shot.layers.retain(|e| e.id != LayerId(layer));
            shot.disconnect(Source::Layer(LayerId(layer)));
            shot.places.remove(&layer);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Parents a layer to a peg of its shot, or to nothing (`parent` null or left out). The
/// layer keeps its place in the stack.
pub(super) fn set_parent(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        parent: Option<u64>,
    }
    let id = "layer.setParent";
    let Params { layer, parent } = parse(id, params)?;
    s.edit(title(id), |p| {
        let Some(shot) = p.shot_of(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
        let parent = parent_in(shot, parent)?;
        // Not `layer_mut`: no drawing is touched, so the shot need not be loaded.
        if let Some(layer) = p.layer_mut(LayerId(layer)) {
            layer.parent = parent;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Moves a layer to `index` among the inputs of the composite it is connected to (0 is the
/// top): over or under the others that composite stacks.
pub(super) fn reorder(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        index: usize,
    }
    let id = "layer.move";
    let Params { layer, index } = parse(id, params)?;
    s.edit(title(id), |p| {
        let Some(shot) = p.shot_of(LayerId(layer)).map(|s| s.id) else { return invalid(format!("no layer {layer}")) };
        let Some(shot) = p.shot_mut(shot) else { return Ok(()) };
        // With effects on its wire, it is the end of the wire that a composite stacks.
        let source = shot.tip(Source::Layer(LayerId(layer)));
        let Some(composite) = shot.composites.iter_mut().find(|c| c.inputs.contains(&source)) else {
            return invalid("the layer is not connected to a composite, so it is neither over nor under another");
        };
        let composite = Arc::make_mut(composite);
        if index >= composite.inputs.len() {
            return invalid(format!("`{}` stacks {} nodes", composite.name, composite.inputs.len()));
        }
        let inputs = &mut composite.inputs;
        inputs.retain(|input| *input != source);
        inputs.insert(index, source);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Changes a layer's resolution and resamples its drawings to it, so they keep their size
/// in the shot. Enlarging adds pixels, never detail.
pub(super) fn set_density(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        density: f64,
    }
    let id = "layer.setDensity";
    let Params { layer, density } = parse(id, params)?;
    let density = checked_density(density)?;
    s.edit(title(id), |p| {
        let layer = layer_mut(p, layer)?;
        let scale = density / layer.density;
        if scale == 1.0 {
            return Ok(());
        }
        let has_rgba = |e: &Layer| e.drawings.iter().flat_map(|d| &d.drawing.art_layers).any(|l| matches!(l.content, ArtContent::Rgba(_)));
        if has_rgba(layer) {
            return invalid("RGBA art layers can't be resampled yet");
        }
        for entry in &mut layer.drawings {
            for art in &mut Arc::make_mut(&mut entry.drawing).art_layers {
                if let ArtContent::Indexed(surface) = &mut art.content {
                    *surface = surface.resampled(scale);
                }
            }
        }
        layer.density = density;
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Makes one art layer of a layer's drawings free colour, or indexed again (plan §4.8).
/// Free colour (`rgba`) is real pixels: the brushes that mix colour work on it, and it does
/// not follow the palette; what is drawn already keeps the colours the shot shows it in.
/// Back to `indexed`, every pixel gets the swatch whose colour is nearest, and what lay
/// between the palette's colours is lost. Every drawing of the layer is changed, and new
/// ones are made so.
/// Switch an art layer of a layer's drawings off, or on again: one that is off is not seen, in
/// the window or in a render, and is kept as it is.
pub(super) fn set_art_shown(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        #[serde(rename = "artLayer")]
        art_layer: iskra_model::ArtLayerKind,
        shown: bool,
    }
    let id = "layer.setArtShown";
    let Params { layer, art_layer, shown } = parse(id, params)?;
    s.edit(title(id), |p| {
        let layer = layer_mut(p, layer)?;
        layer.art_off.retain(|kind| *kind != art_layer);
        if !shown {
            layer.art_off.push(art_layer);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Shows or hides the outputs of a layer's art layers on its node, which the node view wires
/// one art layer alone from.
pub(super) fn set_art_ports(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        shown: bool,
    }
    let id = "layer.setArtPorts";
    let Params { layer, shown } = parse(id, params)?;
    s.edit(title(id), |p| {
        layer_mut(p, layer)?.art_ports = shown;
        Ok(())
    })?;
    Ok(Value::Null)
}

pub(super) fn set_art_mode(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize, Clone, Copy, PartialEq)]
    #[serde(rename_all = "camelCase")]
    enum Mode {
        Indexed,
        Rgba,
        Vector,
    }
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        #[serde(rename = "artLayer")]
        art_layer: iskra_model::ArtLayerKind,
        mode: Mode,
        /// How pixels made vector art are traced.
        #[serde(default)]
        trace: iskra_drawing::Trace,
    }
    let id = "layer.setArtMode";
    let Params { layer, art_layer, mode, trace } = parse(id, params)?;
    s.edit(title(id), |p| {
        let palette = p.palette.clone();
        let styles = p.shot_of(LayerId(layer)).map_or(Vec::new(), |shot| shot.styles.clone());
        let layer = layer_mut(p, layer)?;
        if layer.media.is_some() {
            return invalid("a layer of pictures that were brought in keeps them as they are");
        }
        // Pixels made vector art are traced, with the palette's colours as the shot shows them.
        let palette_colours = super::drawing::traced_with(&palette, &styles);
        // A layer holds one kind of art (plan §4.16): made vector, both its art layers are;
        // an art layer made pixels makes the whole layer pixels, the other free colour, as
        // bitmap art is, if it was vector, else as it was. Indexed or free colour is each art
        // layer's own.
        let modes = ArtLayerKind::ALL.map(|kind| match mode {
            Mode::Vector => (kind, Mode::Vector),
            _ if kind == art_layer => (kind, mode),
            _ if layer.vector.contains(&kind) => (kind, Mode::Rgba),
            _ if layer.free_colour.contains(&kind) => (kind, Mode::Rgba),
            _ => (kind, Mode::Indexed),
        });
        layer.free_colour = modes.iter().filter(|(_, mode)| *mode == Mode::Rgba).map(|(kind, _)| *kind).collect();
        layer.vector = modes.iter().filter(|(_, mode)| *mode == Mode::Vector).map(|(kind, _)| *kind).collect();
        for entry in &mut layer.drawings {
            for (art_layer, mode) in modes {
                let (free, vector) = (mode == Mode::Rgba, mode == Mode::Vector);
                let Some(art) = entry.drawing.layer(art_layer) else { continue };
                // What the art layer becomes. Vector art leaves as the pixels it is drawn as.
                let content = match mode {
                    _ if art.content.is_rgba() == free && art.content.is_vector() == vector => continue,
                    Mode::Vector => match iskra_drawing::vectorize(&art.content, &trace, &palette_colours) {
                        Some(traced) => iskra_model::ArtContent::Vector(traced),
                        None => return invalid(format!("drawing {} has too much detail to trace: trace it with less detail", entry.name)),
                    },
                    Mode::Rgba => art.content.to_rgba(|swatch| palette.color_in(&styles, swatch)),
                    Mode::Indexed => art.content.to_indexed(|color| palette.nearest(color).unwrap_or_default()),
                };
                if let Some(art) = Arc::make_mut(&mut entry.drawing).layer_mut(art_layer) {
                    art.content = content;
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
