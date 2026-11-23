//! Commands on a rectangular selection of the drawing exposed at a frame: clear, transform,
//! and cut, copy and paste through the session's clipboard. Rectangles and transforms are in
//! the drawing's pixels. Every art layer of the drawing takes part, or with `artLayer` that
//! one alone, or with `{ "down": artLayer }` it and those under it, and what is painted with one of the swatches in `protect` is left where it is.
//! With `lasso`, the points of an outline, only what is inside it is selected, and with
//! `swatch` only what that swatch painted: of what is in the rectangle, either way. `parts`
//! are several outlines, each `{ "lasso": [...], "subtract": false }`: in their order, each
//! adds what it holds to the selection or takes it away.

use std::sync::Arc;

use iskra_drawing::{Affine, Lasso, LassoPart, Picked, Point, Rect};
use iskra_model::{ArtContent, ArtLayerKind, ArtReach, Cap, Drawing, Project, SwatchId};
use serde::Deserialize;
use serde_json::{Value, json};

use super::drawing::{Cells, Hold, add_exposed};
use super::layer::layer_mut;
use super::skin::{drawn_on, variant_for};
use super::{invalid, loaded, parse, title};
use crate::{Result, Session};

/// The selection a command names: `[x0, y0, x1, y1]` on the drawing exposed at `frame`.
#[derive(Deserialize)]
struct Selection {
    layer: u64,
    frame: u32,
    rect: [i32; 4],
    /// The art layers the selection is of; without it, all of them.
    #[serde(default, rename = "artLayer")]
    art_layer: ArtReach,
    /// The swatches whose paint the selection leaves.
    #[serde(default)]
    protect: Vec<SwatchId>,
    /// The outline that the selection is the inside of, where it is not all of `rect`.
    lasso: Option<Vec<[f64; 2]>>,
    /// The outlines that make the selection together, after `lasso` if both are given.
    #[serde(default)]
    parts: Vec<Outline>,
    /// The one swatch whose paint is selected, where it is not all paint.
    swatch: Option<SwatchId>,
}

/// One of a selection's outlines, as a command names it.
#[derive(Deserialize)]
struct Outline {
    lasso: Vec<[f64; 2]>,
    #[serde(default)]
    subtract: bool,
}

impl Selection {
    fn rect(&self) -> Result<Rect> {
        let [x0, y0, x1, y1] = self.rect;
        if x1 <= x0 || y1 <= y0 {
            return invalid("the selection is empty");
        }
        Ok(Rect::new(x0, y0, x1, y1))
    }

    /// The selection's outlines, each made of its points: one that holds no pixel is refused.
    fn parts(&self) -> Result<Vec<LassoPart>> {
        let first = self.lasso.iter().map(|points| (points, false));
        let outlines = first.chain(self.parts.iter().map(|part| (&part.lasso, part.subtract)));
        outlines
            .map(|(points, subtract)| {
                let points: Vec<Point> = points.iter().map(|[x, y]| Point::new(*x, *y)).collect();
                match Lasso::new(&points) {
                    Some(lasso) => Ok(LassoPart { lasso: Arc::new(lasso), subtract }),
                    None => invalid("a lasso holds nothing: it needs three points or more that are not on one line"),
                }
            })
            .collect()
    }

    /// What the selection takes of an art layer: `rect` and `parts` are its own, as
    /// [`Selection::rect`] and [`Selection::parts`] gave them.
    fn picked<'a>(&'a self, rect: Rect, parts: &'a [LassoPart]) -> Picked<'a> {
        Picked { rect, parts, swatch: self.swatch, protected: &self.protect }
    }
}

/// The drawing exposed at a frame, to change: where the shot is shown in a skin, its variant.
fn exposed_mut(project: &mut Project, layer: u64, frame: u32) -> Result<&mut Drawing> {
    let Some(id) = drawn_on(project, layer, frame)? else { return invalid(format!("no drawing is exposed at frame {frame}")) };
    match layer_mut(project, layer)?.drawing_mut(id) {
        Some(drawing) => Ok(drawing),
        None => invalid(format!("no drawing {}", id.0)),
    }
}

/// Whether a layer is of pictures that were brought in: its colour art is read again from its
/// file, so a selection passes it by.
fn placed(project: &mut Project, layer: u64) -> Result<bool> {
    Ok(layer_mut(project, layer)?.media.is_some())
}

/// The art layers of a drawing that a selection is of, to change: all of them or the `only`
/// one, indexed and free colour alike, but for the colour art that is a `placed` picture.
/// Does `each` to every drawing the layer exposes on its timeline but the one at `frame`, each
/// once, and adds up what it says.
fn others_exposed(project: &mut Project, layer: u64, frame: u32, mut each: impl FnMut(&mut Drawing) -> usize) -> Result<usize> {
    let of = layer_mut(project, layer)?;
    let at = of.exposure.at(frame);
    let mut others: Vec<_> = of.exposure.runs().iter().map(|run| run.drawing).filter(|drawing| Some(*drawing) != at).collect();
    others.sort_unstable();
    others.dedup();
    // Where the shot is shown in a skin, their variants.
    let others = others.into_iter().map(|day| variant_for(project, layer, day)).collect::<Result<Vec<_>>>()?;
    let layer = layer_mut(project, layer)?;
    let mut said = 0;
    for other in others {
        if let Some(drawing) = layer.drawing_mut(other) {
            said += each(drawing);
        }
    }
    Ok(said)
}

fn art_layers(drawing: &mut Drawing, reach: ArtReach, placed: bool) -> impl Iterator<Item = &mut ArtContent> {
    let of = drawing.art_layers.iter_mut().filter(move |layer| reach.takes(layer.kind));
    of.filter(move |layer| !(placed && layer.kind == ArtLayerKind::ColourArt)).map(|layer| &mut layer.content)
}

/// Erases everything inside the selection.
pub(super) fn clear(s: &mut Session, params: Value) -> Result<Value> {
    let id = "drawing.clear";
    let selection: Selection = parse(id, params)?;
    let (rect, parts) = (selection.rect()?, selection.parts()?);
    let picked = selection.picked(rect, &parts);
    s.edit(title(id), |p| {
        let placed = placed(p, selection.layer)?;
        let drawing = exposed_mut(p, selection.layer, selection.frame)?;
        let before = drawing.clone();
        art_layers(drawing, selection.art_layer, placed).for_each(|art| art.clear_picked(&picked));
        // The fills of vector art whose lines went are found again, or marked open.
        iskra_drawing::refit_fills(drawing, &before);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// The Cutter tool's cut along the edge of the selection, one rectangle or one outline: on
/// vector art, the lines and fills that cross the edge are cut there, and what is inside is
/// taken away, or with `keep` left as shapes of their own; on pixels what is inside is taken
/// away, and with `keep` nothing changes, the pixels being cut by a selection already.
/// Protected swatches are left. With `everyDrawing`, every drawing the layer exposes on its
/// timeline is cut the same, each once. Says how many art layers it changed.
pub(super) fn cut_along(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        #[serde(flatten)]
        selection: Selection,
        #[serde(default)]
        keep: bool,
        /// How the ends the cut makes are finished.
        #[serde(default)]
        cap: Cap,
        #[serde(default, rename = "everyDrawing")]
        every_drawing: bool,
    }
    let id = "drawing.cutAlong";
    let Params { selection, keep, cap, every_drawing } = parse(id, params)?;
    let (rect, parts) = (selection.rect()?, selection.parts()?);
    let picked = selection.picked(rect, &parts);
    // The edge: the outline, or the rectangle's.
    let mut region = iskra_drawing::kurbo::BezPath::new();
    match parts.iter().find(|part| !part.subtract) {
        Some(part) => {
            for (index, at) in part.lasso.points().iter().enumerate() {
                if index == 0 { region.move_to((at.x, at.y)) } else { region.line_to((at.x, at.y)) }
            }
            region.close_path();
        }
        None => {
            region = iskra_drawing::kurbo::Shape::to_path(&iskra_drawing::kurbo::Rect::new(rect.x0.into(), rect.y0.into(), rect.x1.into(), rect.y1.into()), 0.1)
        }
    }
    let cut = |drawing: &mut Drawing, placed: bool| {
        let before = drawing.clone();
        let mut changed = 0;
        for art in art_layers(drawing, selection.art_layer, placed) {
            match art {
                ArtContent::Vector(vector) => {
                    if let Some(cut) = iskra_drawing::cut_vector(vector, &region, &selection.protect, keep, cap) {
                        (*vector, changed) = (cut, changed + 1);
                    }
                }
                _ if keep => {}
                art => {
                    changed += usize::from(!art.extract_picked(&picked).bounds().is_empty());
                    art.clear_picked(&picked);
                }
            }
        }
        iskra_drawing::refit_fills(drawing, &before);
        changed
    };
    let changed = s.edit(title(id), |p| {
        let placed = placed(p, selection.layer)?;
        let mut changed = cut(exposed_mut(p, selection.layer, selection.frame)?, placed);
        if every_drawing {
            changed += others_exposed(p, selection.layer, selection.frame, |drawing| cut(drawing, placed))?;
        }
        Ok(changed)
    })?;
    Ok(json!({ "artLayers": changed }))
}

/// The Cutter tool's flick across vector lines, along `points`: of each stroke it crosses, on
/// the vector art of the drawing exposed at `frame` (of `artLayer` alone, if given), the
/// stretch between the crossings with other strokes either side of it goes. Protected
/// swatches are left. With `everyDrawing`, every drawing the layer exposes on its timeline is
/// cut the same, each once. Says how many art layers it changed.
pub(super) fn cut_flick(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        points: Vec<[f64; 2]>,
        #[serde(default, rename = "artLayer")]
        art_layer: ArtReach,
        #[serde(default)]
        protect: Vec<SwatchId>,
        #[serde(default)]
        cap: Cap,
        #[serde(default, rename = "everyDrawing")]
        every_drawing: bool,
    }
    let id = "drawing.cutFlick";
    let Params { layer, frame, points, art_layer, protect, cap, every_drawing } = parse(id, params)?;
    if points.len() < 2 || points.iter().flatten().any(|v| !v.is_finite()) {
        return invalid("a flick needs two points or more");
    }
    let flick: Vec<iskra_drawing::kurbo::Point> = points.iter().map(|[x, y]| iskra_drawing::kurbo::Point::new(*x, *y)).collect();
    let cut = |drawing: &mut Drawing| {
        let before = drawing.clone();
        let mut changed = 0;
        for art in art_layers(drawing, art_layer, false) {
            if let ArtContent::Vector(vector) = art
                && let Some(cut) = iskra_drawing::cut_overshoot(vector, &flick, &protect, cap)
            {
                (*vector, changed) = (cut, changed + 1);
            }
        }
        iskra_drawing::refit_fills(drawing, &before);
        changed
    };
    let changed = s.edit(title(id), |p| {
        let mut changed = cut(exposed_mut(p, layer, frame)?);
        if every_drawing {
            changed += others_exposed(p, layer, frame, cut)?;
        }
        Ok(changed)
    })?;
    Ok(json!({ "artLayers": changed }))
}

/// Moves, scales or turns what is inside the selection: `matrix` is `[a, b, c, d, e, f]` with
/// `x' = a·x + c·y + e` and `y' = b·x + d·y + f`. With `everyDrawing`, the same is done to
/// every drawing the layer exposes on its timeline, each once, as when a whole cycle is
/// shifted. Returns the bounds of where it landed on the drawing exposed at `frame`.
pub(super) fn transform(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        #[serde(flatten)]
        selection: Selection,
        matrix: [f64; 6],
        #[serde(default, rename = "everyDrawing")]
        every_drawing: bool,
    }
    let id = "drawing.transform";
    let Params { selection, matrix, every_drawing } = parse(id, params)?;
    let (rect, parts) = (selection.rect()?, selection.parts()?);
    let picked = selection.picked(rect, &parts);
    let matrix = Affine { m: matrix };
    if matrix.m.iter().any(|v| !v.is_finite()) || matrix.inverse().is_none() {
        return invalid("the transform must be finite and reversible");
    }
    let landed = s.edit(title(id), |p| {
        let placed = placed(p, selection.layer)?;
        let shown = exposed_mut(p, selection.layer, selection.frame)?;
        let landed = transform_drawing(shown, &picked, &matrix, selection.art_layer, placed);
        if every_drawing {
            others_exposed(p, selection.layer, selection.frame, |drawing| {
                transform_drawing(drawing, &picked, &matrix, selection.art_layer, placed);
                0
            })?;
        }
        Ok(landed)
    })?;
    Ok(json!({ "rect": [landed.x0, landed.y0, landed.x1, landed.y1] }))
}

/// Moves what `picked` takes of a drawing by `matrix`, on its art layers or the `only` one,
/// and says where it landed.
fn transform_drawing(drawing: &mut Drawing, picked: &Picked, matrix: &Affine, only: ArtReach, placed: bool) -> Rect {
    let mut landed = Rect::EMPTY;
    let before = drawing.clone();
    for art in art_layers(drawing, only, placed) {
        // Vector art's shapes move where they are, keeping their places and ids.
        if let ArtContent::Vector(vector) = art {
            let (moved, at) = vector.transform_picked(picked, matrix);
            (*vector, landed) = (moved, landed.union(&at));
            continue;
        }
        let moved = art.extract_picked(picked).transformed(matrix);
        art.clear_picked(picked);
        art.stamp(&moved);
        landed = landed.union(&moved.bounds());
    }
    // The fills of vector art follow the lines that moved.
    iskra_drawing::refit_fills(drawing, &before);
    landed
}

/// Merges the vector shapes the selection takes, swatch by swatch: those of one swatch become
/// one fill of their outline, where the topmost of them was, and the swatches stay apart.
/// Refused where it takes no two shapes of one swatch. Says how many art layers it changed.
pub(super) fn merge(s: &mut Session, params: Value) -> Result<Value> {
    let id = "drawing.mergeSelection";
    let selection: Selection = parse(id, params)?;
    let (rect, parts) = (selection.rect()?, selection.parts()?);
    let picked = selection.picked(rect, &parts);
    let changed = s.edit(title(id), |p| {
        let mut changed = 0;
        for art in art_layers(exposed_mut(p, selection.layer, selection.frame)?, selection.art_layer, false) {
            if let ArtContent::Vector(vector) = art
                && let Some(merged) = iskra_drawing::merge_picked(vector, &picked)
            {
                (*vector, changed) = (merged, changed + 1);
            }
        }
        if changed == 0 {
            return invalid("the selection takes no two vector shapes of one swatch to merge");
        }
        Ok(changed)
    })?;
    Ok(json!({ "artLayers": changed }))
}

/// Gives what is selected the swatch `to`, with the coverage it has: all paint in the
/// selection, or with the selection's `swatch` what that one painted. Says how many art
/// layers it changed.
pub(super) fn recolour(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        #[serde(flatten)]
        selection: Selection,
        to: SwatchId,
    }
    let id = "drawing.recolourSelection";
    let Params { selection, to } = parse(id, params)?;
    let (rect, parts) = (selection.rect()?, selection.parts()?);
    let picked = selection.picked(rect, &parts);
    let changed = s.edit(title(id), |p| {
        if p.palette.swatch(to).is_none() {
            return invalid(format!("the palette has no swatch {}", to.0));
        }
        let layers = art_layers(exposed_mut(p, selection.layer, selection.frame)?, selection.art_layer, false);
        Ok(layers.map(|art| art.recolour_picked(&picked, to)).filter(|changed| *changed).count())
    })?;
    Ok(json!({ "artLayers": changed }))
}

/// Copies what is inside the selection to the session's clipboard.
pub(super) fn copy(s: &mut Session, params: Value) -> Result<Value> {
    let selection: Selection = parse("drawing.copy", params)?;
    let (rect, parts) = (selection.rect()?, selection.parts()?);
    let picked = selection.picked(rect, &parts);
    let project = s.project().clone();
    if let Some(shot) = project.shot_of(iskra_model::LayerId(selection.layer)) {
        loaded(shot)?;
    }
    let of = project.layer(iskra_model::LayerId(selection.layer));
    let skin = project.shot_of(iskra_model::LayerId(selection.layer)).and_then(|shot| shot.skin);
    let Some((of, drawing)) = of.and_then(|of| Some((of, of.shown_at(skin, selection.frame)?))) else {
        return invalid(format!("no drawing is exposed at frame {}", selection.frame));
    };
    // A picture that was brought in is not lifted: it is its file's.
    let taken = drawing.drawing.art_layers.iter().filter(|layer| selection.art_layer.takes(layer.kind));
    let lifted = taken.filter(|layer| !of.is_picture(layer.kind)).map(|layer| (layer.kind, layer.content.extract_picked(&picked)));
    s.clipboard = lifted.filter(|(_, art)| !art.bounds().is_empty()).collect();
    Ok(json!({ "copied": !s.clipboard.is_empty() }))
}

/// Copies what is inside the selection to the clipboard and erases it.
pub(super) fn cut(s: &mut Session, params: Value) -> Result<Value> {
    let selection: Selection = parse("drawing.cut", params.clone())?;
    let (rect, parts) = (selection.rect()?, selection.parts()?);
    let picked = selection.picked(rect, &parts);
    copy(s, params)?;
    s.edit(title("drawing.cut"), |p| {
        let placed = placed(p, selection.layer)?;
        let layers = art_layers(exposed_mut(p, selection.layer, selection.frame)?, selection.art_layer, placed);
        layers.for_each(|art| art.clear_picked(&picked));
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Pastes the clipboard onto the drawing exposed at a frame, where it was cut or copied from,
/// each art layer onto its own kind. What was copied from indexed art goes onto indexed art
/// and what was copied from free colour onto free colour: where the art layer here is of the
/// other kind, that part is not pasted, and it is refused when no part can be. In an empty
/// cell it first makes a drawing, held as `hold` says. With `artLayer`, all of it goes onto
/// that art layer instead, whichever it was copied from: lines pasted among the fills, say.
/// Returns the bounds of what was pasted, to select.
pub(super) fn paste(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        #[serde(default)]
        hold: Hold,
        #[serde(default, rename = "artLayer")]
        art_layer: ArtReach,
    }
    let id = "drawing.paste";
    let Params { layer, frame, hold, art_layer } = parse(id, params)?;
    let clipboard = s.clipboard.clone();
    if clipboard.is_empty() {
        return invalid("there is nothing to paste");
    }
    let pasted = s.edit(title(id), |p| {
        if layer_mut(p, layer)?.exposure.at(frame).is_none() {
            let blank = layer_mut(p, layer)?.blank_drawing();
            add_exposed(p, layer, frame, None, blank, Cells::Hold(hold))?;
        }
        let placed = placed(p, layer)?;
        let drawing = exposed_mut(p, layer, frame)?;
        let mut pasted = Rect::EMPTY;
        for (kind, lifted) in &clipboard {
            // Not onto a picture that was brought in, which is read again from its file.
            if drawing
                .layer_mut(art_layer.only().unwrap_or(*kind))
                .filter(|art| !(placed && art.kind == ArtLayerKind::ColourArt))
                .is_some_and(|art| art.content.stamp(lifted))
            {
                pasted = pasted.union(&lifted.bounds());
            }
        }
        if pasted.is_empty() {
            return invalid("what was copied is of another kind than the art here: indexed art is pasted on indexed art, free colour on free colour");
        }
        Ok(pasted)
    })?;
    Ok(json!({ "rect": [pasted.x0, pasted.y0, pasted.x1, pasted.y1] }))
}
