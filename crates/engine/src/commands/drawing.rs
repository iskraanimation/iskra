//! Drawing and exposure commands: a layer's drawing pool and which drawing shows when.

use std::sync::Arc;

use iskra_drawing::{PaintMode, paint_stroke_as};
use iskra_model::kurbo::Shape as _;
use iskra_model::{ArtContent, ArtLayerKind, Drawing, DrawingId, Layer, LayerId, PoolDrawing, Project, Stroke, StrokePoint, SwatchId};
use serde::Deserialize;
use serde_json::{Value, json};

use super::layer::layer_mut;
use super::skin::{drawn_on, variant_for};
use super::{checked_frames, checked_name, invalid, parse, title};
use crate::{Result, Session};

/// The name for a drawing added to `layer`: `name` if given and free, else the first free
/// number ("1", "2"…).
fn pool_name(layer: &Layer, name: Option<&str>) -> Result<String> {
    let taken = |name: &str| layer.drawings.iter().any(|d| d.name == name);
    match name {
        Some(name) => {
            let name = checked_name(name)?;
            if taken(name) {
                return invalid(format!("`{}` already has a drawing named `{name}`", layer.name));
            }
            Ok(name.to_string())
        }
        None => Ok((1u32..).map(|n| n.to_string()).find(|n| !taken(n)).unwrap_or_default()),
    }
}

/// How long a new drawing holds (plan §7.2b). It is the user's setting, so every command that
/// can make a drawing takes it as `hold`: a number of frames, or `"toNext"`. A command given
/// none holds the drawing for one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(super) enum Hold {
    /// This many frames: 1 for frame-by-frame work, 2 for animating on twos.
    Frames(u32),
    /// Until the next drawing, or the end of the shot.
    ToNext(ToNext),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ToNext {
    ToNext,
}

impl Default for Hold {
    fn default() -> Self {
        Hold::Frames(1)
    }
}

/// The cells a drawing added on a frame takes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Cells {
    /// A new drawing: its hold, and never over a later drawing.
    Hold(Hold),
    /// A copy of the drawing that shows there: the rest of that drawing's hold.
    RestOfHold,
}

/// Add `drawing` to the pool and expose it from `frame` on the cells `cells` says.
pub(super) fn add_exposed(project: &mut Project, layer: u64, frame: u32, name: Option<&str>, drawing: Arc<Drawing>, cells: Cells) -> Result<DrawingId> {
    checked_frames(frame, 1)?;
    if cells == Cells::Hold(Hold::Frames(0)) {
        return invalid("a drawing holds for at least one frame");
    }
    let name = pool_name(layer_mut(project, layer)?, name)?;
    let id = DrawingId(project.new_id());
    let shot_end = project.shot_of(LayerId(layer)).map_or(0, |s| s.duration);
    let layer = layer_mut(project, layer)?;
    layer.drawings.push(PoolDrawing { id, name, drawing });
    let next = layer.exposure.next_start(frame);
    let len = match cells {
        Cells::RestOfHold => layer.exposure.run_at(frame).map_or(1, |run| run.end() - frame),
        Cells::Hold(Hold::Frames(frames)) => frames.min(next.map_or(u32::MAX, |next| next - frame)),
        // Past the shot's last frame there is nothing to hold until.
        Cells::Hold(Hold::ToNext(_)) => next.unwrap_or(shot_end).saturating_sub(frame).max(1),
    };
    layer.exposure.set(frame, len, Some(id));
    Ok(id)
}

/// The drawing a stroke on `frame` of `layer` is drawn on ([`drawn_on`]: the skin's variant
/// where the shot is shown in one), with a blank one exposed first in an empty cell, held as
/// `hold` says.
pub(super) fn drawn_on_or_blank(p: &mut Project, layer: u64, frame: u32, hold: Hold) -> Result<DrawingId> {
    if let Some(id) = drawn_on(p, layer, frame)? {
        return Ok(id);
    }
    let blank = layer_mut(p, layer)?.blank_drawing();
    let day = add_exposed(p, layer, frame, None, blank, Cells::Hold(hold))?;
    Ok(drawn_on(p, layer, frame)?.unwrap_or(day))
}

/// Adds an empty drawing and exposes it from a frame, for as long as `hold` says.
pub(super) fn new_drawing(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        name: Option<String>,
        #[serde(default)]
        hold: Hold,
    }
    let id = "drawing.newDrawing";
    let Params { layer, frame, name, hold } = parse(id, params)?;
    let drawing = s.edit(title(id), |p| {
        let blank = layer_mut(p, layer)?.blank_drawing();
        add_exposed(p, layer, frame, name.as_deref(), blank, Cells::Hold(hold))
    })?;
    Ok(json!({ "id": drawing }))
}

/// Copies the drawing exposed at a frame and exposes the copy from there to the end of its
/// hold, so it can be changed without changing the earlier frames that show the original.
pub(super) fn duplicate(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        name: Option<String>,
    }
    let id = "drawing.duplicate";
    let Params { layer, frame, name } = parse(id, params)?;
    let drawing = s.edit(title(id), |p| {
        let Some(source) = layer_mut(p, layer)?.drawing_at(frame).map(|d| d.drawing.clone()) else {
            return invalid(format!("no drawing is exposed at frame {frame}"));
        };
        add_exposed(p, layer, frame, name.as_deref(), source, Cells::RestOfHold)
    })?;
    Ok(json!({ "id": drawing }))
}

pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        drawing: u64,
        name: String,
    }
    let id = "drawing.rename";
    let Params { layer, drawing, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        let layer = layer_mut(p, layer)?;
        if layer.drawings.iter().any(|d| d.id != DrawingId(drawing) && d.name == name) {
            return invalid(format!("`{}` already has a drawing named `{name}`", layer.name));
        }
        match layer.drawings.iter_mut().find(|d| d.id == DrawingId(drawing)) {
            Some(entry) => entry.name = name.to_string(),
            None => return invalid(format!("no drawing {drawing}")),
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Deletes a drawing from the pool and empties the cells that showed it.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        drawing: u64,
    }
    let id = "drawing.delete";
    let Params { layer, drawing } = parse(id, params)?;
    s.edit(title(id), |p| {
        let layer = layer_mut(p, layer)?;
        if !layer.drawings.iter().any(|d| d.id == DrawingId(drawing)) {
            return invalid(format!("no drawing {drawing}"));
        }
        layer.drawings.retain(|d| d.id != DrawingId(drawing));
        // Its variants in skins go with it.
        layer.variants.retain(|variant| variant.of != DrawingId(drawing));
        layer.exposure.remove_drawing(DrawingId(drawing));
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Exposes a drawing of the pool on a range of frames, or empties those cells (`drawing: null`).
pub(super) fn set_exposure(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "one")]
        length: u32,
        drawing: Option<u64>,
    }
    let id = "exposure.set";
    let Params { layer, frame, length, drawing } = parse(id, params)?;
    checked_frames(frame, length)?;
    s.edit(title(id), |p| {
        let layer = layer_mut(p, layer)?;
        let drawing = drawing.map(DrawingId);
        if let Some(id) = drawing.filter(|&id| layer.drawing(id).is_none()) {
            return invalid(format!("`{}` has no drawing {}", layer.name, id.0));
        }
        layer.exposure.set(frame, length, drawing);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Moves the `length` frames of a layer's track from `frame` so that they begin on `to`:
/// where they were is emptied, and where they land they cover what was there.
pub(super) fn move_exposure(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "one")]
        length: u32,
        to: u32,
    }
    let id = "exposure.move";
    let Params { layer, frame, length, to } = parse(id, params)?;
    checked_frames(frame, length)?;
    checked_frames(to, length)?;
    s.edit(title(id), |p| {
        layer_mut(p, layer)?.exposure.shift(frame, length, to);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Lists what the `length` frames of a layer's track from `frame` show, not an edit: `cells`,
/// the holds one after the other, each `[frames, drawing]` with `null` for empty cells. It is
/// what `exposure.paste` takes.
pub(super) fn copy_exposure(s: &mut Session, params: Value) -> Result<Value> {
    let id = "exposure.copy";
    let Frames { layer, frame, length } = parse(id, params)?;
    checked_frames(frame, length)?;
    match s.project().layer(LayerId(layer)) {
        Some(layer) => Ok(
            json!({ "cells": layer.exposure.cells(frame, length).into_iter().map(|(len, drawing)| json!([len, drawing.map(|id| id.0)])).collect::<Vec<_>>() }),
        ),
        None => invalid(format!("no layer {layer}")),
    }
}

/// Lays `cells` on a layer's track from `frame` on, over what is there: holds one after the
/// other, each `[frames, drawing]`, with `null` for empty cells, as `exposure.copy` gives
/// them. The drawings are the layer's own: frames are pasted on the layer they were copied
/// from. Returns the `length` of what was laid.
pub(super) fn paste_exposure(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        cells: Vec<(u32, Option<u64>)>,
    }
    let id = "exposure.paste";
    let Params { layer, frame, cells } = parse(id, params)?;
    let length = cells.iter().try_fold(0u32, |sum, (len, _)| sum.checked_add(*len).filter(|_| *len > 0));
    let Some(length) = length.filter(|length| *length > 0) else { return invalid("give the `cells` to lay, each of a frame or more") };
    checked_frames(frame, length)?;
    s.edit(title(id), |p| {
        let layer = layer_mut(p, layer)?;
        if let Some((_, Some(drawing))) = cells.iter().find(|(_, drawing)| drawing.is_some_and(|id| layer.drawing(DrawingId(id)).is_none())) {
            return invalid(format!("`{}` has no drawing {drawing}: frames are pasted on the layer they were copied from", layer.name));
        }
        let cells: Vec<_> = cells.iter().map(|(len, drawing)| (*len, drawing.map(DrawingId))).collect();
        layer.exposure.lay(frame, &cells);
        Ok(())
    })?;
    Ok(json!({ "length": length }))
}

#[derive(Deserialize)]
struct Frames {
    layer: u64,
    frame: u32,
    #[serde(default = "one")]
    length: u32,
}

fn one() -> u32 {
    1
}

/// Holds the drawing at a frame for longer, moving the later cells along. In an empty cell it
/// adds empty cells.
pub(super) fn insert_exposure(s: &mut Session, params: Value) -> Result<Value> {
    let id = "exposure.insert";
    let Frames { layer, frame, length } = parse(id, params)?;
    checked_frames(frame, length)?;
    s.edit(title(id), |p| {
        layer_mut(p, layer)?.exposure.insert(frame, length);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets how long the drawing on a frame holds: the whole hold, in frames. With `ripple` (the
/// default) the later cells move along or back; without it they stay, so a longer hold covers
/// what follows and a shorter one leaves empty cells.
pub(super) fn set_hold(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        length: u32,
        #[serde(default = "yes")]
        ripple: bool,
    }
    fn yes() -> bool {
        true
    }
    let id = "exposure.setHold";
    let Params { layer, frame, length, ripple } = parse(id, params)?;
    checked_frames(frame, length)?;
    s.edit(title(id), |p| {
        if !layer_mut(p, layer)?.exposure.set_hold(frame, length, ripple) {
            return invalid(format!("no drawing is exposed at frame {frame}"));
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Holds the last drawing before an empty cell on to it, over the empty cells between; the
/// later cells stay where they are.
pub(super) fn extend_exposure(s: &mut Session, params: Value) -> Result<Value> {
    let id = "exposure.extend";
    let Frames { layer, frame, .. } = parse(id, params)?;
    checked_frames(frame, 1)?;
    s.edit(title(id), |p| {
        if !layer_mut(p, layer)?.exposure.extend_to(frame) {
            return invalid(format!("frame {frame} is not an empty cell after a drawing"));
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Takes frames out of a layer's track, moving the later cells back.
pub(super) fn remove_exposure(s: &mut Session, params: Value) -> Result<Value> {
    let id = "exposure.remove";
    let Frames { layer, frame, length } = parse(id, params)?;
    checked_frames(frame, length)?;
    s.edit(title(id), |p| {
        layer_mut(p, layer)?.exposure.remove(frame, length);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Paints a brush stroke on the drawing exposed at a frame, or erases with it when the brush
/// is set to erase. Painting in an empty cell first makes a drawing there, held as `hold` says,
/// in the same undo step. The stroke's points are in the drawing's pixels. What is painted with
/// one of the swatches in `protect` is left as it is. On an art layer that is free colour,
/// `color` (sRGB, straight alpha) is painted in place of the swatch's colour, and then no
/// swatch is needed.
pub(super) fn stroke(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        /// Which of the drawing's art layers to paint on.
        #[serde(default = "line_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        /// The swatch to paint with. An eraser needs none.
        #[serde(default)]
        swatch: u16,
        /// On free colour: the colour to paint, where it is not the swatch's.
        color: Option<[u8; 4]>,
        stroke: Stroke,
        #[serde(default)]
        hold: Hold,
        #[serde(default)]
        protect: Vec<SwatchId>,
        /// Laid over what is painted, under it (`behind`), or on it alone (`repaint`).
        #[serde(default)]
        mode: PaintMode,
    }
    fn line_art() -> ArtLayerKind {
        ArtLayerKind::LineArt
    }
    let Params { layer, frame, art_layer, swatch, color, stroke, hold, protect, mode } = parse("drawing.stroke", params)?;
    let label = if stroke.brush.erase { "Eraser Stroke" } else { "Brush Stroke" };
    paint(s, label, Cell { layer, frame, hold }, art_layer, Ink { swatch, color, mode }, &stroke, &protect, None)
}

/// Draws a vector stroke on an art layer of vector art, of the drawing exposed at a frame (plan
/// §4.16): `points` (in the drawing's pixels, with their pressure) are smoothed and fitted with
/// Béziers as `brush` says, and kept as one shape of `swatch` with its width along it. In an
/// empty cell it first makes a drawing, as `drawing.stroke` does. Where it crosses a shape of
/// one of the swatches in `protect`, it goes under the lowest such shape, so they stay whole.
/// Returns the drawing and the new shape's id.
pub(super) fn vector_stroke(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "line_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        swatch: u16,
        #[serde(default)]
        brush: iskra_drawing::VectorBrush,
        points: Vec<StrokePoint>,
        #[serde(default)]
        hold: Hold,
        #[serde(default)]
        protect: Vec<SwatchId>,
        /// Laid over the shapes, under them all (`behind`), or (`repaint`) not laid at all,
        /// the shapes it touches given its swatch.
        #[serde(default)]
        mode: PaintMode,
        /// Merged with the shapes of its swatch it overlaps into one shape.
        #[serde(default)]
        flatten: bool,
        /// Left as a surface, the area it covers with no middle line, and merged as with
        /// `flatten`.
        #[serde(default)]
        surface: bool,
    }
    fn line_art() -> ArtLayerKind {
        ArtLayerKind::LineArt
    }
    let id = "drawing.vectorStroke";
    let Params { layer, frame, art_layer, swatch, brush, points, hold, protect, mode, flatten, surface } = parse(id, params)?;
    let stamp = &brush.stamp;
    let numbers = [
        brush.thinnest,
        brush.smoothing,
        brush.simplify,
        brush.faintest,
        brush.fade_start,
        brush.fade_end,
        brush.drift,
        brush.boil,
        stamp.spacing,
        stamp.jitter,
        stamp.drift,
    ];
    if !(brush.size.is_finite() && brush.size > 0.0 && numbers.iter().all(|n| n.is_finite())) {
        return invalid("the brush's size is a number above zero, and its other settings numbers");
    }
    let curve = &brush.pressure_curve;
    if curve.len() > 16 || curve.iter().flatten().any(|part| !(0.0..=1.0).contains(part)) || curve.windows(2).any(|pair| pair[0][0] >= pair[1][0]) {
        return invalid("the pressure curve has at most 16 points, each two numbers from 0 to 1, in order of the pressure");
    }
    let Some(stroke) = iskra_drawing::vector_stroke(&points, &brush) else {
        return invalid("a stroke needs at least one point, and finite coordinates");
    };
    // A line drawn by a stamped tip stays a line: a fill of its outline would lose its stamps.
    let flatten = flatten && stroke.stamp.is_none();
    let done = s.edit(title(id), |p| {
        if p.palette.swatch(SwatchId(swatch)).is_none() {
            return invalid(format!("the palette has no swatch {swatch}"));
        }
        let of = layer_mut(p, layer)?;
        if !of.vector.contains(&art_layer) && of.drawing_at(frame).is_none_or(|d| d.drawing.layer(art_layer).is_none_or(|art| !art.content.is_vector())) {
            return invalid("the art layer is not vector art: make it so first (layer.setArtMode)");
        }
        if mode == PaintMode::Repaint && of.exposure.at(frame).is_none() {
            return invalid("nothing is drawn here to repaint");
        }
        let drawing = drawn_on_or_blank(p, layer, frame, hold)?;
        let Some(entry) = layer_mut(p, layer)?.drawing_mut(drawing) else { return invalid(format!("no drawing {}", drawing.0)) };
        let before = entry.clone();
        let Some(ArtContent::Vector(art)) = entry.layer_mut(art_layer).map(|l| &mut l.content) else {
            return invalid("the drawing's art layer is not vector art");
        };
        let kind = match surface {
            true => iskra_drawing::surface_shape(&stroke, &brush),
            false => iskra_model::ShapeKind::Stroke(stroke),
        };
        let shape = iskra_model::VectorShape { id: art.next_id(), swatch: SwatchId(swatch), kind };
        // Repainting, the shapes it touches take its swatch, and it is not kept.
        if mode == PaintMode::Repaint {
            if let Some(recoloured) = iskra_drawing::repaint_touched(art, &shape.kind.area(), SwatchId(swatch), &protect) {
                *art = recoloured;
            }
            return Ok((drawing, None));
        }
        let reach = shape.kind.area().bounding_box();
        let under = art.shapes().iter().position(|other| protect.contains(&other.swatch) && other.kind.area().bounding_box().overlaps(reach));
        let new = shape.id;
        // Behind, it goes under them all.
        let at = if mode == PaintMode::Behind { Some(0) } else { under };
        // Flattened, it is one shape with those of its swatch it overlaps, where it is drawn:
        // not where it goes under a protected one, nor a line behind. A surface drawn behind
        // still merges with its swatch's, and the one shape goes under the rest (the user's of
        // 2026-11-23, as Moho's Blob Brush draws behind strokes).
        let behind = mode == PaintMode::Behind;
        let merges = (flatten || surface) && (at.is_none() || (surface && behind && under.is_none()));
        match merges.then(|| iskra_drawing::flattened(art, &shape)).flatten() {
            Some(flat) if behind => {
                *art = flat.edited(|shapes| {
                    if let Some(top) = shapes.iter().position(|other| other.id == new) {
                        let merged = shapes.remove(top);
                        shapes.insert(0, merged);
                    }
                })
            }
            Some(flat) => *art = flat,
            None => *art = art.edited(|shapes| shapes.insert(at.unwrap_or(shapes.len()), shape)),
        }
        // A fill it is drawn right across is cut in two along it.
        iskra_drawing::refit_fills(entry, &before);
        Ok((drawing, Some(new)))
    })?;
    Ok(json!({ "drawing": done.0, "shape": done.1.map(|shape| shape.0) }))
}

/// Sprays copies of a vector shape along `points` on an art layer of vector art, of the drawing
/// exposed at a frame (Moho's Scatter Brush, the user's of 2026-11-23): `scatter` says which
/// (a preset, in `swatch`, or the vector art last cut or copied, in its own swatches), how large
/// across they are, how far apart, and how much chance turns and flips them. The copies are laid
/// on top, or under the lowest shape of a protected swatch they overlap, in one step.
pub(super) fn scatter(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "line_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        #[serde(default)]
        swatch: u16,
        points: Vec<[f64; 2]>,
        #[serde(default)]
        scatter: iskra_drawing::Scatter,
        #[serde(default)]
        hold: Hold,
        #[serde(default)]
        protect: Vec<SwatchId>,
    }
    fn line_art() -> ArtLayerKind {
        ArtLayerKind::LineArt
    }
    let id = "drawing.scatter";
    let Params { layer, frame, art_layer, swatch, points, scatter, hold, protect } = parse(id, params)?;
    let sizes = [scatter.min_size, scatter.max_size, scatter.spacing, scatter.angle_jitter];
    if points.is_empty() || points.iter().flatten().any(|v| !v.is_finite()) || sizes.iter().any(|n| !n.is_finite() || *n < 0.0) {
        return invalid("a scatter needs points with finite coordinates, and sizes, a spacing and an angle of 0 or more");
    }
    let source = match scatter.preset {
        iskra_drawing::ScatterPreset::Clipboard => s.clipboard_shapes(),
        preset => preset.shapes(SwatchId(swatch)),
    };
    if source.is_empty() {
        return invalid("nothing is on the clipboard to scatter: cut or copy vector art first");
    }
    let path: Vec<iskra_model::Point> = points.iter().map(|[x, y]| iskra_model::Point::new(*x, *y)).collect();
    let done = s.edit(title(id), |p| {
        if scatter.preset != iskra_drawing::ScatterPreset::Clipboard && p.palette.swatch(SwatchId(swatch)).is_none() {
            return invalid(format!("the palette has no swatch {swatch}"));
        }
        let of = layer_mut(p, layer)?;
        if !of.vector.contains(&art_layer) && of.drawing_at(frame).is_none_or(|d| d.drawing.layer(art_layer).is_none_or(|art| !art.content.is_vector())) {
            return invalid("the art layer is not vector art: make it so first (layer.setArtMode)");
        }
        let drawing = drawn_on_or_blank(p, layer, frame, hold)?;
        let Some(entry) = layer_mut(p, layer)?.drawing_mut(drawing) else { return invalid(format!("no drawing {}", drawing.0)) };
        let before = entry.clone();
        let Some(ArtContent::Vector(art)) = entry.layer_mut(art_layer).map(|l| &mut l.content) else {
            return invalid("the drawing's art layer is not vector art");
        };
        let copies = iskra_drawing::scattered(&source, &path, &scatter, art.next_id().0);
        let count = copies.len();
        let reach = copies.iter().map(|one| one.kind.area().bounding_box()).reduce(|all, more| all.union(more)).unwrap_or_default();
        let under = art.shapes().iter().position(|other| protect.contains(&other.swatch) && other.kind.area().bounding_box().overlaps(reach));
        *art = art.edited(|all| {
            let at = under.unwrap_or(all.len());
            all.splice(at..at, copies);
        });
        iskra_drawing::refit_fills(entry, &before);
        Ok((drawing, count))
    })?;
    Ok(json!({ "drawing": done.0, "copies": done.1 }))
}

/// Erases along a path on an art layer of vector art, of the drawing exposed at a frame (plan
/// §4.16): the eraser is as wide as `brush` draws along `points`. Where it goes across a
/// stroke's whole width the stroke is cut, its pieces strokes still; a stroke it only bites
/// into becomes the outline of what is left, and a fill loses what it covers. Shapes of the
/// swatches in `protect` are left whole. An empty cell, or nothing under the eraser, is left as
/// it is, and makes no undo step.
pub(super) fn vector_erase(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "line_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        #[serde(default)]
        brush: iskra_drawing::VectorBrush,
        points: Vec<StrokePoint>,
        #[serde(default)]
        protect: Vec<SwatchId>,
    }
    fn line_art() -> ArtLayerKind {
        ArtLayerKind::LineArt
    }
    let id = "drawing.vectorErase";
    let Params { layer, frame, art_layer, brush, points, protect } = parse(id, params)?;
    if !(brush.size.is_finite() && brush.size > 0.0) {
        return invalid("the eraser's size is a number above zero");
    }
    let Some(eraser) = iskra_drawing::vector_stroke(&points, &brush) else {
        return invalid("a stroke needs at least one point, and finite coordinates");
    };
    let erased = {
        let Some(of) = s.project().layer(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
        let skin = s.project().shot_of(LayerId(layer)).and_then(|shot| shot.skin);
        let Some(entry) = of.shown_at(skin, frame) else { return Ok(json!({ "erased": false })) };
        match entry.drawing.layer(art_layer).map(|l| &l.content) {
            Some(ArtContent::Vector(art)) => iskra_drawing::erase_vector(art, &eraser, &protect),
            _ => return invalid("the drawing's art layer is not vector art"),
        }
    };
    let Some(art) = erased else { return Ok(json!({ "erased": false })) };
    s.edit("Eraser Stroke", |p| {
        let Some(drawing) = drawn_on(p, layer, frame)? else { return invalid("nothing is drawn here") };
        let Some(entry) = layer_mut(p, layer)?.drawing_mut(drawing) else { return invalid(format!("no drawing {}", drawing.0)) };
        let before = entry.clone();
        if let Some(art_layer) = entry.layer_mut(art_layer) {
            art_layer.content = ArtContent::Vector(art);
        }
        // The fills its cut lines bounded follow them.
        iskra_drawing::refit_fills(entry, &before);
        Ok(())
    })?;
    Ok(json!({ "erased": true }))
}

/// Gives shapes of an art layer of vector art, of the drawing exposed at a frame, new paths, as
/// the point editor moves, adds and deletes their points (plan §4.16, step 8): each of `shapes`
/// names a shape by its `shape` id and gives its `path` (SVG path data, in the drawing's
/// pixels) and, for a stroke, its `widths`, one for each point; a stroke given no widths keeps
/// the ones it has, which must then be as many as its new points. So too its `opacities` and
/// `drifts` (each 0 to 1), which may also be none, for opaque and not drifted. One undo step.
/// The fills its edited lines bound follow them.
pub(super) fn edit_shapes(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Edit {
        shape: u32,
        path: String,
        widths: Option<Vec<f64>>,
        opacities: Option<Vec<f64>>,
        drifts: Option<Vec<f64>>,
    }
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "line_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        shapes: Vec<Edit>,
    }
    fn line_art() -> ArtLayerKind {
        ArtLayerKind::LineArt
    }
    let id = "drawing.editShapes";
    let Params { layer, frame, art_layer, shapes } = parse(id, params)?;
    let mut edits = Vec::new();
    for edit in shapes {
        let path = iskra_model::kurbo::BezPath::from_svg(&edit.path).map_err(|why| crate::EngineError::Invalid(format!("shape {}: {why}", edit.shape)))?;
        let finite = path.elements().iter().all(|element| element.end_point().is_none_or(|at| at.is_finite()));
        if !finite || path.elements().is_empty() || edit.widths.iter().flatten().any(|width| !width.is_finite() || *width < 0.0) {
            return invalid(format!("shape {}: a path of finite points, and widths of finite numbers not below zero", edit.shape));
        }
        if edit.opacities.iter().chain(&edit.drifts).flatten().any(|value| !(0.0..=1.0).contains(value)) {
            return invalid(format!("shape {}: opacities and drifts are numbers from 0 to 1", edit.shape));
        }
        edits.push((iskra_model::ShapeId(edit.shape), path, [edit.widths, edit.opacities, edit.drifts]));
    }
    s.edit("Edit Points", |p| {
        let Some(shown) = drawn_on(p, layer, frame)? else { return invalid("nothing is drawn here to edit") };
        let Some(drawing) = layer_mut(p, layer)?.drawing_mut(shown) else { return invalid(format!("no drawing {}", shown.0)) };
        let before = drawing.clone();
        let Some(ArtContent::Vector(art)) = drawing.layer_mut(art_layer).map(|l| &mut l.content) else {
            return invalid("the drawing's art layer is not vector art");
        };
        let mut shapes = art.shapes().to_vec();
        for (shape, path, lists) in edits {
            let Some(found) = shapes.iter_mut().find(|found| found.id == shape) else { return invalid(format!("no shape {}", shape.0)) };
            match &mut found.kind {
                iskra_model::ShapeKind::Stroke(stroke) => {
                    let points = path.elements().iter().filter(|element| element.end_point().is_some()).count();
                    let names = ["widths", "opacities", "drifts"];
                    let lists: Vec<Vec<f64>> = lists.into_iter().zip(stroke.per_point_mut()).map(|(given, had)| given.unwrap_or_else(|| had.clone())).collect();
                    // Widths, one a point; the others one a point too, or none.
                    for (at, (list, name)) in lists.iter().zip(names).enumerate() {
                        if list.len() != points && (at == 0 || !list.is_empty()) {
                            return invalid(format!("shape {}: {} {name} for {points} points", shape.0, list.len()));
                        }
                    }
                    stroke.path = path;
                    for (had, list) in stroke.per_point_mut().into_iter().zip(lists) {
                        *had = list;
                    }
                }
                iskra_model::ShapeKind::Fill(fill) => fill.outline = path,
            }
        }
        *art = iskra_model::VectorArt::new(shapes);
        iskra_drawing::refit_fills(drawing, &before);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Lays a vector brush on strokes already drawn, as the point editor's list does: each of
/// `shapes` keeps its line, and takes the brush's widths, tapers, nib and grain, its thick and
/// thin kept ([`iskra_drawing::rebrushed`]). Fills are passed by. One undo step.
pub(super) fn lay_brush(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "line_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        shapes: Vec<u32>,
        brush: iskra_drawing::VectorBrush,
    }
    fn line_art() -> ArtLayerKind {
        ArtLayerKind::LineArt
    }
    let id = "drawing.layBrush";
    let Params { layer, frame, art_layer, shapes, brush } = parse(id, params)?;
    if !brush.size.is_finite() || brush.size <= 0.0 {
        return invalid("the brush's size must be a number above zero");
    }
    let laid = s.edit(title(id), |p| {
        let Some(shown) = drawn_on(p, layer, frame)? else { return invalid("nothing is drawn here") };
        let Some(drawing) = layer_mut(p, layer)?.drawing_mut(shown) else { return invalid(format!("no drawing {}", shown.0)) };
        let before = drawing.clone();
        let Some(ArtContent::Vector(art)) = drawing.layer_mut(art_layer).map(|l| &mut l.content) else {
            return invalid("the drawing's art layer is not vector art");
        };
        let mut laid = 0;
        let mut all = art.shapes().to_vec();
        for shape in &mut all {
            if let (true, iskra_model::ShapeKind::Stroke(stroke)) = (shapes.contains(&shape.id.0), &mut shape.kind) {
                *stroke = iskra_drawing::rebrushed(stroke, &brush);
                laid += 1;
            }
        }
        *art = iskra_model::VectorArt::new(all);
        iskra_drawing::refit_fills(drawing, &before);
        Ok(laid)
    })?;
    Ok(json!({ "strokes": laid }))
}

/// Splits the vector stroke near (`x`, `y`) of the drawing exposed at `frame` at the crossings
/// either side of that point, so that the piece between them is a stroke of its own, as the
/// Select tool's *Pieces* takes it ([`iskra_drawing::split_piece`]); `reach` is how near, in
/// the drawing's pixels. Says the piece's shape and its middle line, as points a pixel or so
/// apart; `null` where no stroke is near. A stroke that crosses none is its own piece, and
/// nothing changes.
pub(super) fn split_piece(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "line_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        x: f64,
        y: f64,
        reach: f64,
        #[serde(default)]
        protect: Vec<SwatchId>,
    }
    fn line_art() -> ArtLayerKind {
        ArtLayerKind::LineArt
    }
    let id = "drawing.splitPiece";
    let Params { layer, frame, art_layer, x, y, reach, protect } = parse(id, params)?;
    if !(x.is_finite() && y.is_finite() && reach.is_finite() && reach >= 0.0) {
        return invalid("the point and the reach must be finite");
    }
    let found = {
        let of = s.project().layer(LayerId(layer)).ok_or_else(|| crate::EngineError::Invalid(format!("no layer {layer}")))?;
        let skin = s.project().shot_of(LayerId(layer)).and_then(|shot| shot.skin);
        let art = of.shown_at(skin, frame).and_then(|entry| entry.drawing.layer(art_layer)).map(|art| &art.content);
        let Some(ArtContent::Vector(art)) = art else { return invalid("the drawing's art layer is not vector art") };
        iskra_drawing::split_piece(art, iskra_model::kurbo::Point::new(x, y), reach, &protect).map(|(split, shape, line)| (split != *art, split, shape, line))
    };
    let Some((changed, split, shape, line)) = found else { return Ok(Value::Null) };
    // A surface's piece is an area, which is taken whole; a line's, along its middle line.
    let surface = split.shapes().iter().any(|one| one.id == shape && matches!(one.kind, iskra_model::ShapeKind::Fill(_)));
    if changed {
        s.edit(title(id), |p| {
            let Some(shown) = drawn_on(p, layer, frame)? else { return invalid("nothing is drawn here") };
            let Some(drawing) = layer_mut(p, layer)?.drawing_mut(shown) else { return invalid(format!("no drawing {}", shown.0)) };
            if let Some(art) = drawing.layer_mut(art_layer) {
                art.content = ArtContent::Vector(split);
            }
            Ok(())
        })?;
    }
    let mut points = Vec::new();
    iskra_model::kurbo::flatten(&line, 0.5, |element| {
        if let iskra_model::kurbo::PathEl::MoveTo(at) | iskra_model::kurbo::PathEl::LineTo(at) = element {
            points.push([at.x, at.y]);
        }
    });
    Ok(json!({ "shape": shape.0, "line": points, "surface": surface }))
}

/// The colours free colour is traced with: the palette's swatches as `styles` show them.
pub(super) fn traced_with(palette: &iskra_model::Palette, styles: &[iskra_model::StyleId]) -> Vec<([u8; 3], SwatchId)> {
    palette
        .swatches
        .iter()
        .map(|swatch| {
            let [r, g, b, _] = palette.color_in(styles, swatch.id);
            ([r, g, b], swatch.id)
        })
        .collect()
}

/// Turns an art layer of the drawing exposed at a frame into vector art by tracing its pixels
/// as `trace` says (plan §4.16, step 9): indexed pixels swatch by swatch, free colour by the
/// palette's colours. The layer's other drawings are left as they are. One undo step.
pub(super) fn vectorize(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "line_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        #[serde(default)]
        trace: iskra_drawing::Trace,
    }
    fn line_art() -> ArtLayerKind {
        ArtLayerKind::LineArt
    }
    let id = "drawing.vectorize";
    let Params { layer, frame, art_layer, trace } = parse(id, params)?;
    let shapes = s.edit("Make Vector Art", |p| {
        let styles = p.shot_of(LayerId(layer)).map_or(Vec::new(), |shot| shot.styles.clone());
        let colours = traced_with(&p.palette, &styles);
        let of = layer_mut(p, layer)?;
        if of.media.is_some() {
            return invalid("a layer of pictures that were brought in keeps them as they are");
        }
        let Some(shown) = drawn_on(p, layer, frame)? else { return invalid("nothing is drawn here to trace") };
        let Some(drawing) = layer_mut(p, layer)?.drawing_mut(shown) else { return invalid(format!("no drawing {}", shown.0)) };
        let Some(art) = drawing.layer_mut(art_layer) else { return invalid("the drawing has no such art layer") };
        let Some(traced) = iskra_drawing::vectorize(&art.content, &trace, &colours) else {
            return invalid("too much detail to trace: trace it with less detail");
        };
        let shapes = traced.shapes().len();
        art.content = ArtContent::Vector(traced);
        Ok(shapes)
    })?;
    Ok(json!({ "shapes": shapes }))
}

/// Turns an art layer of vector art, of the drawing exposed at a frame, into the indexed pixels
/// it is drawn as. One undo step.
pub(super) fn rasterize(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "line_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
    }
    fn line_art() -> ArtLayerKind {
        ArtLayerKind::LineArt
    }
    let id = "drawing.rasterize";
    let Params { layer, frame, art_layer } = parse(id, params)?;
    s.edit("Make Pixels", |p| {
        let Some(shown) = drawn_on(p, layer, frame)? else { return invalid("nothing is drawn here") };
        let Some(drawing) = layer_mut(p, layer)?.drawing_mut(shown) else { return invalid(format!("no drawing {}", shown.0)) };
        let Some(art) = drawing.layer_mut(art_layer) else { return invalid("the drawing has no such art layer") };
        let ArtContent::Vector(vector) = &art.content else { return invalid("the art layer is pixels already") };
        art.content = ArtContent::Indexed(vector.raster().clone());
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Paints a stroke of the mixer brush on an art layer that is free colour, of the drawing
/// exposed at a frame (plan §4.8): the brush lays down the paint it is loaded with, picks up
/// what is painted under it and mixes the two, as `mixer` says (`wet`, `load`, `mix` and
/// `flow`, each from 0 to 1). `state` is what the brush carries when the stroke begins, the
/// paint it is loaded with and what it has picked up; the result's `state` is what it carries
/// after, for the next stroke to be given. The project keeps none of it: a stroke is the same
/// whenever it is run with the same `state`, and undoing one does not put paint back on the
/// brush. In an empty cell it first makes a drawing, as `drawing.stroke` does.
pub(super) fn mix(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default = "colour_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        stroke: Stroke,
        #[serde(default)]
        mixer: iskra_model::MixerSettings,
        #[serde(default)]
        state: iskra_model::MixerState,
        #[serde(default)]
        hold: Hold,
    }
    fn colour_art() -> ArtLayerKind {
        ArtLayerKind::ColourArt
    }
    let id = "drawing.mix";
    let Params { layer, frame, art_layer, stroke, mixer, mut state, hold } = parse(id, params)?;
    if stroke.points.is_empty() || stroke.points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return invalid("a stroke needs at least one point, and finite coordinates");
    }
    let carried = [state.reservoir, state.pickup].into_iter().flatten().flatten().chain([state.amount, mixer.wet, mixer.load, mixer.mix, mixer.flow]);
    if carried.into_iter().any(|value| !value.is_finite()) {
        return invalid("the mixer's settings and what the brush carries are finite numbers");
    }
    let drawing = s.edit(super::title(id), |p| {
        let of = layer_mut(p, layer)?;
        if !of.free_colour.contains(&art_layer) {
            return invalid("the mixer brush mixes real colour: make the art layer free colour first (layer.setArtMode)");
        }
        if of.is_picture(art_layer) {
            return invalid("this is a picture that was brought in: it is read again from its file, and paint on it would be lost");
        }
        let id = drawn_on_or_blank(p, layer, frame, hold)?;
        let Some(drawing) = layer_mut(p, layer)?.drawing_mut(id) else { return invalid(format!("no drawing {}", id.0)) };
        match drawing.layer_mut(art_layer).map(|l| &mut l.content) {
            Some(ArtContent::Rgba(surface)) => _ = iskra_drawing::mix_rgba(surface, &stroke, &mixer, &mut state),
            _ => return invalid("the drawing's art layer is not free colour"),
        }
        Ok(id)
    })?;
    Ok(json!({ "drawing": drawing, "state": state }))
}

/// The cell a stroke is painted in, and how a drawing made there is held.
struct Cell {
    layer: u64,
    frame: u32,
    hold: Hold,
}

/// What is painted with: a swatch, and on free colour a colour of its own in its place.
struct Ink {
    swatch: u16,
    color: Option<[u8; 4]>,
    /// Over what is painted, under it, or on it alone. Free colour is painted over.
    mode: PaintMode,
}

/// The pixels a shape's inside covers, and how much of each, in rows.
type Inside = (iskra_model::Rect, Vec<f32>);

/// Paint `stroke` on an art layer of the drawing a cell shows, making the drawing if the cell
/// is empty, as the undo step `label`. With `inside`, that is painted first, under the
/// stroke: the inside of a shape that is filled.
#[allow(clippy::too_many_arguments)]
fn paint(
    s: &mut Session,
    label: &str,
    cell: Cell,
    art_layer: ArtLayerKind,
    ink: Ink,
    stroke: &Stroke,
    protect: &[SwatchId],
    inside: Option<&Inside>,
) -> Result<Value> {
    let Cell { layer, frame, hold } = cell;
    let Ink { swatch, color: free, mode } = ink;
    let erase = stroke.brush.erase;
    // A shape with no line paints its inside alone.
    if (stroke.points.is_empty() && inside.is_none()) || stroke.points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return invalid("a stroke needs at least one point, and finite coordinates");
    }
    // The stroke as the client painted it, if it is this one: then only the brush (on free
    // colour, with its colour) and the pixels it was painted on are left to compare.
    let offered = s.painted.take().filter(|o| {
        inside.is_none()
            && (o.layer, o.frame, o.art_layer, o.swatch, o.mode) == (layer, frame, art_layer, SwatchId(swatch), mode)
            && o.protect == protect
            && o.points == stroke.points
    });
    let drawing = s.edit(label, |p| {
        // A colour of its own is painted on free colour, and needs no swatch there.
        let on_free = layer_mut(p, layer)?.free_colour.contains(&art_layer);
        if !erase && !(free.is_some() && on_free) && p.palette.swatch(SwatchId(swatch)).is_none() {
            return invalid(format!("the palette has no swatch {swatch}"));
        }
        // Nothing is there to take away, nor to repaint.
        if (erase || mode == PaintMode::Repaint) && layer_mut(p, layer)?.exposure.at(frame).is_none() {
            return Ok(None);
        }
        let id = drawn_on_or_blank(p, layer, frame, hold)?;
        // On free colour, the swatch gives the brush its colour as the shot shows it now; a
        // gradient's or a picture's, across what the stroke covers (plan §4.20).
        let shown = p.palette.styled(p.shot_of(LayerId(layer)).map_or(&[], |shot| &shot.styles));
        let color = free.unwrap_or_else(|| shown.color(SwatchId(swatch)));
        let shade = shown.shade(SwatchId(swatch));
        let shaded = free.is_none() && !erase && mode == PaintMode::Over && !shade.is_solid();
        let linear = p.settings.working_space == iskra_model::WorkingSpace::Linear;
        let of = layer_mut(p, layer)?;
        let placed = of.is_picture(art_layer);
        let Some(drawing) = of.drawing_mut(id) else { return invalid(format!("no drawing {}", id.0)) };
        match drawing.layer_mut(art_layer).map(|l| &mut l.content) {
            Some(ArtContent::Indexed(surface)) => match offered {
                Some(crate::Painted { brush, before: ArtContent::Indexed(before), after: ArtContent::Indexed(after), .. })
                    if brush == stroke.brush && before == *surface =>
                {
                    *surface = after;
                }
                _ => {
                    if let Some((rect, coverage)) = inside {
                        surface.paint_protecting(*rect, coverage, SwatchId(swatch), protect);
                    }
                    if !stroke.points.is_empty() {
                        paint_stroke_as(surface, stroke, SwatchId(swatch), protect, mode);
                    }
                }
            },
            // A picture that was placed is read again when its file changes: paint would be lost.
            Some(ArtContent::Rgba(_)) if placed => return invalid("this is a picture that was brought in: draw on the line art over it"),
            // Painted as on indexed art, and laid on in the colours it has there.
            Some(ArtContent::Rgba(surface)) if shaded => {
                let mut marks = iskra_model::IndexedSurface::new();
                if let Some((rect, coverage)) = inside {
                    marks.paint(*rect, coverage, SwatchId(swatch));
                }
                if !stroke.points.is_empty() {
                    paint_stroke_as(&mut marks, stroke, SwatchId(swatch), &[], PaintMode::Over);
                }
                iskra_drawing::paint_rgba_shaded(surface, &marks, &shade, linear);
            }
            Some(ArtContent::Rgba(surface)) => {
                let brush = iskra_drawing::brush_in(&stroke.brush, color);
                match offered {
                    Some(crate::Painted { brush: painted, before: ArtContent::Rgba(before), after: ArtContent::Rgba(after), .. })
                        if painted == brush && before == *surface =>
                    {
                        *surface = after;
                    }
                    _ => {
                        if let Some((rect, coverage)) = inside {
                            iskra_drawing::fill_rgba(surface, *rect, coverage, color);
                        }
                        if !stroke.points.is_empty() {
                            iskra_drawing::paint_rgba_as(surface, &Stroke { brush, points: stroke.points.clone() }, mode);
                        }
                    }
                }
            }
            Some(ArtContent::Vector(_)) => return invalid("the art layer is vector art, which the brush does not paint pixels on"),
            None => return invalid("the drawing has no such art layer"),
        }
        Ok(Some(id))
    })?;
    Ok(json!({ "drawing": drawing }))
}

/// Draws a line, a rectangle or an ellipse with a brush on the drawing exposed at a frame:
/// a stroke that goes along the shape at full pressure, without the brush's smoothing. `from`
/// and `to` are the line's ends, or opposite corners of the rectangle, or of the one the
/// ellipse lies in, in the drawing's pixels; `even` makes it a line at a step of 45°, a
/// square or a circle. With `centred`, `from` is the shape's middle; `round` rounds a
/// rectangle's corners with that radius, in pixels; `filled` paints the inside of a rectangle
/// or an ellipse with the swatch too. A `polyline` goes through `points`, two or more, from
/// each to the next, and with `closed` back to the first; `filled`, it is closed and its
/// inside is painted. The rest, `color` on free colour too, is as for `drawing.stroke`.
///
/// Moho's other shapes too (the user's of 2026-11-23): a `triangle` and a `star` (of `tips`
/// points) in the rectangle between the two points, an `arrow` from its tail to its head, a
/// `spiral` out from its middle, `from`, to `to`, its turns `spacing` pixels apart and with
/// `counter` the other way, and a `grid` of `rows` and `columns`. With `noLine`, a filled
/// shape's inside alone is painted, with no line round it.
///
/// On vector art the shape is vector shapes, its lines strokes along its own corners and
/// curves drawn with `vectorBrush` at full pressure, and its inside a fill under them.
pub(super) fn shape(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        shape: iskra_drawing::Shape,
        from: Option<[f64; 2]>,
        to: Option<[f64; 2]>,
        /// The corners of a polyline, in place of `from` and `to`.
        #[serde(default)]
        points: Vec<[f64; 2]>,
        #[serde(flatten)]
        form: iskra_drawing::Form,
        #[serde(default)]
        brush: iskra_model::BrushSettings,
        #[serde(default = "line_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        #[serde(default)]
        swatch: u16,
        color: Option<[u8; 4]>,
        #[serde(default)]
        hold: Hold,
        #[serde(default)]
        protect: Vec<SwatchId>,
        /// What its lines are drawn with on vector art.
        #[serde(default, rename = "vectorBrush")]
        vector_brush: iskra_drawing::VectorBrush,
    }
    fn line_art() -> ArtLayerKind {
        ArtLayerKind::LineArt
    }
    let Params { layer, frame, shape, from, to, points, form, brush, art_layer, swatch, color, hold, protect, vector_brush } = parse("drawing.shape", params)?;
    // A polyline has its corners; any other shape its two points.
    let corners: Vec<[f64; 2]> = match (shape, from, to) {
        (iskra_drawing::Shape::Polyline, ..) if points.len() >= 2 => points,
        (iskra_drawing::Shape::Polyline, ..) => return invalid("a polyline needs two points or more, as `points`"),
        (_, Some(from), Some(to)) => vec![from, to],
        _ => return invalid("a shape needs two points, `from` and `to`"),
    };
    if corners.iter().flatten().any(|v| !v.is_finite()) {
        return invalid("a shape needs points with finite coordinates");
    }
    if !form.round.is_finite() || form.round < 0.0 {
        return invalid("a rectangle's corners are rounded with a radius of 0 or more");
    }
    if !form.spacing.is_finite() || form.spacing < 1.0 || form.tips < 3 || form.rows < 1 || form.columns < 1 {
        return invalid("a spiral's turns are 1 pixel apart or more, a star has 3 points or more, and a grid a row and a column");
    }
    let corners: Vec<iskra_model::Point> = corners.iter().map(|[x, y]| iskra_model::Point::new(*x, *y)).collect();
    // With no line, a filled shape's inside alone; one with nothing inside keeps its line.
    let bare = !shape.lined(form) && !brush.erase;
    let on_vector = {
        let of = s.project().layer(LayerId(layer)).ok_or_else(|| crate::EngineError::Invalid(format!("no layer {layer}")))?;
        let content = of.drawing_at(frame).and_then(|d| d.drawing.layer(art_layer)).map(|art| &art.content);
        content.map_or(of.vector.contains(&art_layer), ArtContent::is_vector)
    };
    if on_vector {
        if brush.erase {
            return invalid("on vector art a shape is drawn, not erased with");
        }
        return vector_shape(s, Cell { layer, frame, hold }, art_layer, swatch, shape, &corners, form, &vector_brush, &protect);
    }
    let mut stroke = shape.stroke_through(&brush, &corners, form);
    if bare {
        stroke.points.clear();
    }
    // An eraser takes the shape's line away, and has nothing to fill.
    let inside = shape.inside_through(&corners, form).filter(|_| !brush.erase);
    paint(s, shape.title(), Cell { layer, frame, hold }, art_layer, Ink { swatch, color, mode: PaintMode::Over }, &stroke, &protect, inside.as_ref())
}

/// [`shape`] on vector art: its lines strokes of `brush` at full pressure along the shape's own
/// corners and curves, and a filled shape's inside one fill under them, laid on top of what is
/// there in one step, or under the lowest shape of a protected swatch they overlap.
#[allow(clippy::too_many_arguments)]
fn vector_shape(
    s: &mut Session,
    cell: Cell,
    art_layer: ArtLayerKind,
    swatch: u16,
    shape: iskra_drawing::Shape,
    corners: &[iskra_model::Point],
    form: iskra_drawing::Form,
    brush: &iskra_drawing::VectorBrush,
    protect: &[SwatchId],
) -> Result<Value> {
    let Cell { layer, frame, hold } = cell;
    if !(brush.size.is_finite() && brush.size > 0.0) {
        return invalid("the vector brush's size is a number above zero");
    }
    let mut kinds = Vec::new();
    if let Some(inside) = shape.vector_inside_through(corners, form) {
        kinds.push(iskra_model::ShapeKind::Fill(iskra_model::VectorFill::plain(inside)));
    }
    if shape.lined(form) {
        for line in shape.vector_lines_through(corners, form) {
            let points = line.elements().iter().filter(|element| !matches!(element, iskra_model::kurbo::PathEl::ClosePath)).count();
            let even = iskra_model::VectorStroke { path: line, widths: vec![1.0; points], ..Default::default() };
            kinds.push(iskra_model::ShapeKind::Stroke(iskra_drawing::rebrushed(&even, brush)));
        }
    }
    if kinds.is_empty() {
        return invalid("the shape has nothing to draw");
    }
    let done = s.edit(shape.title(), |p| {
        if p.palette.swatch(SwatchId(swatch)).is_none() {
            return invalid(format!("the palette has no swatch {swatch}"));
        }
        let drawing = drawn_on_or_blank(p, layer, frame, hold)?;
        let Some(entry) = layer_mut(p, layer)?.drawing_mut(drawing) else { return invalid(format!("no drawing {}", drawing.0)) };
        let before = entry.clone();
        let Some(ArtContent::Vector(art)) = entry.layer_mut(art_layer).map(|l| &mut l.content) else {
            return invalid("the drawing's art layer is not vector art");
        };
        let first = art.next_id().0;
        let shapes: Vec<iskra_model::VectorShape> = kinds
            .into_iter()
            .enumerate()
            .map(|(n, kind)| iskra_model::VectorShape { id: iskra_model::ShapeId(first + n as u32), swatch: SwatchId(swatch), kind })
            .collect();
        let ids: Vec<u32> = shapes.iter().map(|one| one.id.0).collect();
        let reach = shapes.iter().map(|one| one.kind.area().bounding_box()).reduce(|all, more| all.union(more)).unwrap_or_default();
        let under = art.shapes().iter().position(|other| protect.contains(&other.swatch) && other.kind.area().bounding_box().overlaps(reach));
        *art = art.edited(|all| {
            let at = under.unwrap_or(all.len());
            all.splice(at..at, shapes);
        });
        // A fill it is drawn right across is cut in two along it.
        iskra_drawing::refit_fills(entry, &before);
        Ok((drawing, ids))
    })?;
    Ok(json!({ "drawing": done.0, "shapes": done.1 }))
}

/// Fills an area of the colour art of the drawing exposed at a frame with a swatch, out to
/// the lines of its line art (the paint bucket, plan §4.8). `x` and `y` are in the drawing's
/// pixels. Gaps in the lines up to `gap` pixels wide are closed, and the fill runs `under`
/// pixels under the lines. Swatch 0 takes the paint of the area away. An area painted with one
/// of the swatches in `protect` is refused.
///
/// With `points` in place of `x` and `y`, every place of them is filled, in one step: the
/// bucket dragged over several areas. With `artLayer` the line art, the line under each place
/// gets the swatch, and keeps its soft edge, where on the colour art an area is filled. With
/// `everyDrawing`, the same is done on every drawing of the layer and not only the one the
/// frame shows; with `frames`, `[first, last]`, on every drawing exposed on those frames,
/// each once however long it is held. Of several places or drawings, those that can't be filled are left, and it
/// is refused only when none could be.
///
/// Where the colour art is free colour, the area is what is of one colour around the place,
/// inside the lines, and it is painted in the swatch's colour as the shot shows it, or in
/// `color` (sRGB, straight alpha) where that is given, which then needs no swatch.
pub(super) fn fill(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        x: Option<f64>,
        y: Option<f64>,
        #[serde(default)]
        points: Vec<[f64; 2]>,
        #[serde(default)]
        swatch: u16,
        color: Option<[u8; 4]>,
        #[serde(default)]
        gap: u32,
        #[serde(default = "under")]
        under: u32,
        #[serde(default)]
        protect: Vec<SwatchId>,
        #[serde(default = "colour_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
        #[serde(default, rename = "everyDrawing")]
        every_drawing: bool,
        frames: Option<[u32; 2]>,
    }
    fn under() -> u32 {
        iskra_drawing::Fill::default().under
    }
    fn colour_art() -> ArtLayerKind {
        ArtLayerKind::ColourArt
    }
    let Params { layer, frame, x, y, mut points, swatch, color, gap, under, protect, art_layer, every_drawing, frames } = parse("drawing.fill", params)?;
    if frames.is_some_and(|[first, last]| first > last) {
        return invalid("the frames to fill on run from the first to the last");
    }
    points.extend(x.zip(y).map(|(x, y)| [x, y]));
    if points.is_empty() || points.iter().flatten().any(|v| !v.is_finite()) {
        return invalid("a fill needs a place");
    }
    if gap > MAX_GAP || under > MAX_GAP {
        return invalid(format!("gaps are closed, and lines run under, up to {MAX_GAP} pixels"));
    }
    let places: Vec<(i32, i32)> = points.iter().map(|[x, y]| (x.floor() as i32, y.floor() as i32)).collect();
    // The overart's marks are recoloured as lines are: it has no areas of its own to fill.
    let on_lines = art_layer != ArtLayerKind::ColourArt;
    let label = match (swatch == 0 && color.is_none(), on_lines) {
        (true, _) => "Unpaint",
        (false, true) => "Recolour Line",
        (false, false) => "Fill",
    };
    let (drawing, changed, filled) = s.edit(label, |p| {
        // On free colour a colour of its own is painted, and needs no swatch.
        let on_free = !on_lines && layer_mut(p, layer)?.free_colour.contains(&art_layer);
        if swatch != 0 && !(on_free && color.is_some()) && p.palette.swatch(SwatchId(swatch)).is_none() {
            return invalid(format!("the palette has no swatch {swatch}"));
        }
        // What free colour is filled with: nothing, the colour given, or the swatch's as the shot
        // shows it; a gradient's or a picture's across the area (plan §4.20).
        let shown = p.palette.styled(p.shot_of(LayerId(layer)).map_or(&[], |shot| &shot.styles));
        let ink = if swatch == 0 && color.is_none() { [0; 4] } else { color.unwrap_or(shown.color(SwatchId(swatch))) };
        let shade = shown.shade(SwatchId(swatch));
        let shaded = (swatch != 0 && color.is_none() && !shade.is_solid()).then_some(&shade);
        let linear = p.settings.working_space == iskra_model::WorkingSpace::Linear;
        let of = layer_mut(p, layer)?;
        let placed = of.is_picture(art_layer);
        let Some(shown) = of.exposure.at(frame) else { return invalid("nothing is drawn here to fill") };
        let ids: Vec<DrawingId> = match (every_drawing, frames) {
            (true, _) => of.drawings.iter().map(|entry| entry.id).collect(),
            (false, Some([first, last])) => {
                let mut ids: Vec<DrawingId> = Vec::new();
                let runs = of.exposure.runs().iter().filter(|run| run.start <= last && run.end() > first);
                for id in runs.map(|run| run.drawing) {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                ids
            }
            (false, None) => vec![shown],
        };
        // Where the shot is shown in a skin, its variants are filled.
        let ids = ids.into_iter().map(|day| variant_for(p, layer, day)).collect::<Result<Vec<_>>>()?;
        let shown = variant_for(p, layer, shown)?;
        let of = layer_mut(p, layer)?;
        let options = iskra_drawing::Fill { gap, under };
        // What changed on the drawing that is shown, how many places took the swatch, and
        // why the first that did not, didn't.
        let (mut changed, mut filled, mut refused) = (iskra_model::Rect::EMPTY, 0, None);
        for id in ids {
            let Some(drawing) = of.drawing_mut(id) else { return invalid(format!("no drawing {}", id.0)) };
            // Vector art is filled with shapes, among the strokes' middle lines (plan §4.16).
            if drawing.layer(art_layer).is_some_and(|art| art.content.is_vector()) {
                let places: Vec<iskra_model::kurbo::Point> = points.iter().map(|[x, y]| iskra_model::kurbo::Point::new(*x, *y)).collect();
                let (done, why) = match on_lines {
                    true => {
                        let lines = places.iter().map(|at| iskra_drawing::recolour_vector_line(drawing, art_layer, *at, SwatchId(swatch), &protect));
                        lines.fold((0, None), |(done, first), line| match line {
                            Ok(_) => (done + 1, first),
                            Err(why) => (done, first.or(Some(why))),
                        })
                    }
                    // A drag's places all at once: its areas are found and filled once each.
                    false => iskra_drawing::fill_vector_at(drawing, art_layer, &places, SwatchId(swatch), f64::from(gap), &protect),
                };
                filled += done;
                if let Some(why) = why {
                    refused.get_or_insert(why.to_string());
                }
                if done > 0 && id == shown {
                    changed = changed.union(&drawing.layer(art_layer).map_or(iskra_model::Rect::EMPTY, |art| art.content.bounds()));
                }
                continue;
            }
            // Line art that is vector art bounds the fill as the pixels it is drawn as, and
            // line art of free colour by its alpha.
            if on_lines && drawing.layer(art_layer).is_some_and(|art| art.content.is_rgba()) {
                let art = if art_layer == ArtLayerKind::LineArt { "line art" } else { "overart" };
                refused.get_or_insert(format!("the {art} here is free colour, which has no swatches to change"));
                continue;
            }
            let lines = drawing.layer(ArtLayerKind::LineArt).map_or_else(Default::default, |l| l.content.lines().into_owned());
            let art = match drawing.layer_mut(art_layer).map(|l| &mut l.content) {
                Some(ArtContent::Indexed(art)) => art,
                // Free colour: what is of one colour around each place, inside the lines.
                Some(ArtContent::Rgba(art)) if !placed && !on_lines => {
                    for at in &places {
                        let filled_free = match shaded {
                            Some(shade) => {
                                iskra_drawing::fill_free_shaded(art, &lines, *at, ink, &options, &|x, y, bounds| shade.in_area(x, y, bounds, linear))
                            }
                            None => iskra_drawing::fill_free(art, &lines, *at, ink, &options),
                        };
                        match filled_free {
                            Ok(rect) => {
                                filled += 1;
                                if id == shown {
                                    changed = changed.union(&rect);
                                }
                            }
                            Err(why) => _ = refused.get_or_insert(why.to_string()),
                        }
                    }
                    continue;
                }
                _ => {
                    refused.get_or_insert("the colour art here is a picture that was brought in, which can't be filled".to_string());
                    continue;
                }
            };
            // The last place the fill ran out from: what lies open beside it does too.
            let mut open: Option<(i32, i32)> = None;
            for at in &places {
                // A drag gives many places of one area: once it is filled, or where the
                // place is on a line, the rest are told at a look.
                if !on_lines {
                    let (paint, line) = (art.pixel(at.0, at.1), lines.pixel(at.0, at.1));
                    let painted = if paint.coverage >= HALF { paint.swatch } else { SwatchId::NONE };
                    if line.coverage >= HALF {
                        refused.get_or_insert(iskra_drawing::FillError::OnLine.to_string());
                        continue;
                    }
                    if painted == SwatchId(swatch) {
                        filled += 1;
                        continue;
                    }
                    if open.is_some_and(|from| open_between(art, &lines, from, *at, gap)) {
                        open = Some(*at);
                        continue;
                    }
                }
                let done = match on_lines {
                    true => iskra_drawing::recolour_line(art, *at, SwatchId(swatch), &protect),
                    false => iskra_drawing::fill_protecting(art, &lines, *at, SwatchId(swatch), &options, &protect),
                };
                match done {
                    Ok(rect) => {
                        filled += 1;
                        if id == shown {
                            changed = changed.union(&rect);
                        }
                    }
                    Err(why) => {
                        open = (why == iskra_drawing::FillError::Open).then_some(*at);
                        refused.get_or_insert(why.to_string());
                    }
                }
            }
        }
        match (filled, refused) {
            (0, Some(why)) => invalid(why),
            _ => Ok((shown, changed, filled)),
        }
    })?;
    Ok(json!({ "drawing": drawing, "changed": [changed.x0, changed.y0, changed.x1, changed.y1], "filled": filled }))
}

/// Whether the way straight from `from` to `to` is open to the paint bucket: no line on it
/// and one paint all along, and room beside it for the square that closes gaps of `gap`
/// pixels. A fill from `to` then runs out as one from `from` did. For wide gaps it is not
/// looked into, and says no.
fn open_between(colour: &iskra_model::IndexedSurface, lines: &iskra_model::IndexedSurface, from: (i32, i32), to: (i32, i32), gap: u32) -> bool {
    let reach = gap.div_ceil(2) as i32;
    if reach > 16 {
        return false;
    }
    let painted = |x: i32, y: i32| Some(colour.pixel(x, y)).filter(|pixel| pixel.coverage >= HALF).map(|pixel| pixel.swatch);
    let before = painted(from.0, from.1);
    let free = |x: i32, y: i32| lines.pixel(x, y).coverage < HALF && painted(x, y) == before;
    let steps = (to.0 - from.0).abs().max((to.1 - from.1).abs()).max(1);
    (0..=steps).all(|step| {
        let (x, y) = (from.0 + (to.0 - from.0) * step / steps, from.1 + (to.1 - from.1) * step / steps);
        (-reach..=reach).all(|dy| (-reach..=reach).all(|dx| free(x + dx, y + dy)))
    })
}

/// A pixel covered at least this much is a line, or is painted, to the paint bucket: half.
const HALF: u16 = 32768;

/// The widest gap the paint bucket closes, in pixels.
const MAX_GAP: u32 = 200;

/// Gives what is painted with the swatch `from` the swatch `to` (plan §4.8), on both art
/// layers: on the drawing a layer shows on a frame, on all of a layer's drawings, on all of a
/// shot's, or with `all` on every shot's. With `frames`, a first and a last frame, only the
/// drawings a layer, or every layer of a shot, shows on a frame of those. `from` need not be
/// in the palette, which is how a colour that went missing is given a swatch again. A shot
/// that is not open is not looked at: `unread` names those.
pub(super) fn recolour(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        from: u16,
        to: u16,
        layer: Option<u64>,
        frame: Option<u32>,
        shot: Option<u64>,
        #[serde(default)]
        all: bool,
        frames: Option<[u32; 2]>,
    }
    let id = "drawing.recolour";
    let Params { from, to, layer, frame, shot, all, frames } = parse(id, params)?;
    if from == 0 || from == to {
        return invalid("recolouring needs two different swatches");
    }
    if frame.is_some() && layer.is_none() {
        return invalid("a frame is a layer's: name the layer too");
    }
    if frames.is_some() && (frame.is_some() || all) {
        return invalid("`frames` are a layer's or a shot's, and take no `frame` and no `all`");
    }
    if frames.is_some_and(|[first, last]| first > last) {
        return invalid("`frames` are a first frame and a last, in that order");
    }
    let (changed, unread) = s.edit(super::title(id), |p| {
        if p.palette.swatch(SwatchId(to)).is_none() {
            return invalid(format!("the palette has no swatch {to}"));
        }
        // The one drawing a frame shows, those some frames show, or every drawing.
        if let (Some(layer), Some(frame)) = (layer, frame)
            && layer_mut(p, layer)?.exposure.at(frame).is_none()
        {
            return invalid("nothing is drawn there");
        }
        let shown = |of: &Layer, entry: &PoolDrawing| match (frame, frames) {
            (Some(frame), _) => of.exposure.at(frame) == Some(entry.id),
            (None, Some([first, last])) => (first..=last).any(|frame| of.exposure.at(frame) == Some(entry.id)),
            (None, None) => true,
        };
        let shots: Vec<_> = match (layer, all) {
            (Some(layer), _) => match p.shot_of(LayerId(layer)) {
                Some(shot) => vec![shot.id],
                None => return invalid(format!("no layer {layer}")),
            },
            (None, true) => p.shots.iter().map(|shot| shot.id).collect(),
            (None, false) => vec![super::shot_id(p, shot)?],
        };
        let (mut changed, mut unread) = (0, Vec::new());
        for id in shots {
            let Some(shot) = p.shot_mut(id) else { continue };
            if !shot.loaded {
                unread.push(shot.name.clone());
                continue;
            }
            for of in shot.layers.iter_mut().filter(|of| layer.is_none_or(|layer| of.id == LayerId(layer))) {
                // Only the drawings that have the swatch are copied to be changed.
                let with: Vec<DrawingId> =
                    of.drawings.iter().filter(|entry| shown(of, entry) && entry.drawing.swatches().contains(&SwatchId(from))).map(|entry| entry.id).collect();
                for entry in Arc::make_mut(of).drawings.iter_mut().filter(|entry| with.contains(&entry.id)) {
                    for art in &mut Arc::make_mut(&mut entry.drawing).art_layers {
                        if let ArtContent::Indexed(surface) = &mut art.content {
                            surface.recolour(SwatchId(from), SwatchId(to));
                        }
                    }
                    changed += 1;
                }
            }
        }
        Ok((changed, unread))
    })?;
    Ok(json!({ "drawings": changed, "unread": unread }))
}

/// Places a gradient or a picture on the filled area of art layer `artLayer` that the point
/// (`x`, `y`) of the drawing on `frame` is in (plan §4.20): from `start` to `end`, in the
/// drawing's pixels, its other side from `start` to `across` (square to the first without
/// it), a radial gradient's focal point at `focus` (its middle without it). With `reset`, the area's placement is let go, and the swatch is fitted
/// to the area again. The area has to be of a swatch that is a gradient or a picture as the
/// shot shows it.
pub(super) fn place(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        x: f64,
        y: f64,
        start: Option<[f32; 2]>,
        end: Option<[f32; 2]>,
        across: Option<[f32; 2]>,
        focus: Option<[f32; 2]>,
        #[serde(default)]
        reset: bool,
        #[serde(default = "colour_art", rename = "artLayer")]
        art_layer: ArtLayerKind,
    }
    fn colour_art() -> ArtLayerKind {
        ArtLayerKind::ColourArt
    }
    let id = "drawing.place";
    let Params { layer, frame, x, y, start, end, across, focus, reset, art_layer } = parse(id, params)?;
    let ends = start.zip(end).filter(|(start, end)| start.iter().chain(end).all(|v| v.is_finite()));
    if across.iter().chain(&focus).flatten().any(|v| !v.is_finite()) {
        return invalid("a placement's `across` and `focus` are points");
    }
    match (reset, ends) {
        (true, None) if start.is_none() && end.is_none() => {}
        (false, Some(_)) => {}
        _ => return invalid("a placement needs a `start` and an `end`, or `reset` and neither"),
    }
    if !(x.is_finite() && y.is_finite()) {
        return invalid("a placement needs a point inside its area");
    }
    let at = [x.floor() as i32, y.floor() as i32];
    s.edit(if reset { "Reset Placement" } else { title(id) }, |p| {
        let Some(shot) = p.shot_of(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
        let palette = p.palette.styled(&shot.styles);
        let Some(drawing) = drawn_on(p, layer, frame)? else { return invalid("nothing is drawn here") };
        let Some(entry) = layer_mut(p, layer)?.drawing_mut(drawing) else { return invalid(format!("no drawing {}", drawing.0)) };
        let areas = entry.areas(|swatch| palette.shade(swatch));
        let Some(area) = areas.at(art_layer, at[0], at[1]).copied() else {
            return invalid("there is no area of a gradient or a picture there");
        };
        match ends {
            Some((start, end)) => {
                let focus = iskra_model::Frame::inside_ring(focus.unwrap_or_default());
                entry.place(&areas, iskra_model::Placement { art: art_layer, swatch: area.swatch, at, start, end, across, focus })
            }
            None => entry.unplace(&areas, art_layer, at),
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Gives every other drawing of the layer the placements of gradients and pictures (plan
/// §4.20) of the drawing on `frame`, in place of their own: for a fill that is to stay where
/// it is while the drawings under it change. A placement whose point lies in no area of a
/// drawing of that swatch does nothing there.
pub(super) fn copy_placements(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
    }
    let id = "drawing.copyPlacements";
    let Params { layer, frame } = parse(id, params)?;
    s.edit(title(id), |p| {
        let Some(from) = drawn_on(p, layer, frame)? else { return invalid("nothing is drawn here") };
        let of = layer_mut(p, layer)?;
        let Some(placements) = of.drawing_mut(from).map(|drawing| drawing.placements.clone()) else { return invalid(format!("no drawing {}", from.0)) };
        let others = of.drawings.iter_mut().chain(of.variants.iter_mut().map(|variant| &mut variant.drawing)).filter(|entry| entry.id != from);
        for entry in others.filter(|entry| entry.drawing.placements != placements) {
            Arc::make_mut(&mut entry.drawing).placements = placements.clone();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
