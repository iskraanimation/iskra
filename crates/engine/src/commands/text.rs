//! Text commands (plan §4.19): the boxes of text of a text layer's drawings. Each edit lays the
//! drawing's boxes out again into its line art.

use iskra_drawing::fonts::Fonts;
use iskra_model::{Drawing, DrawingId, Project, TextBox, TextFormat};
use serde::Deserialize;
use serde_json::{Value, json};

use super::drawing::{Hold, drawn_on_or_blank};
use super::layer::layer_mut;
use super::{invalid, parse, title};
use crate::{Result, Session};

/// The fonts texts are laid out in: those of the machine, and Inter.
fn fonts() -> &'static Fonts {
    Fonts::system()
}

/// The drawing `drawing` of the text layer `layer`, to change.
fn text_drawing(p: &mut Project, layer: u64, drawing: u64) -> Result<&mut Drawing> {
    let of = layer_mut(p, layer)?;
    if !of.text {
        return invalid("the layer is not a text layer");
    }
    match of.drawing_mut(DrawingId(drawing)) {
        Some(drawing) => Ok(drawing),
        None => invalid(format!("the layer has no drawing {drawing}")),
    }
}

/// The box `index` of a drawing, to change.
fn text_box(drawing: &mut Drawing, index: usize) -> Result<&mut TextBox> {
    match drawing.texts.get_mut(index) {
        Some(text) => Ok(text),
        None => invalid(format!("the drawing has no box {index}")),
    }
}

fn checked_format(p: &Project, format: &TextFormat) -> Result<()> {
    if !format.is_valid() {
        return invalid("a text's size and leading are numbers above zero, and its tracking a number");
    }
    if p.palette.swatch(format.swatch).is_none() {
        return invalid(format!("the palette has no swatch {}", format.swatch.0));
    }
    Ok(())
}

fn checked_place(at: [f64; 2], width: Option<f64>) -> Result<()> {
    if !at.iter().all(|v| v.is_finite()) || width.is_some_and(|width| !(width.is_finite() && width > 0.0)) {
        return invalid("a box's place is two numbers, and its width a number above zero");
    }
    Ok(())
}

/// Lays out a drawing's boxes again, and says which fonts it lacked.
fn laid_out(drawing: &mut Drawing) -> Value {
    json!(drawing.lay_out_texts(fonts()))
}

/// Puts a box of text on a text layer at a frame: into the drawing exposed there, or a new one
/// exposed from there for as long as `hold` says. `at` is its top left corner in the drawing's
/// pixels; with `width` its lines wrap in that width. Without a `format` it is in Inter, 48
/// pixels, black.
pub(super) fn add(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        frame: u32,
        at: [f64; 2],
        width: Option<f64>,
        #[serde(default)]
        text: String,
        #[serde(default)]
        format: TextFormat,
        #[serde(default)]
        hold: Hold,
    }
    let id = "text.add";
    let Params { layer, frame, at, width, text, format, hold } = parse(id, params)?;
    checked_place(at, width)?;
    let (drawing, index, missing) = s.edit(title(id), |p| {
        checked_format(p, &format)?;
        if !layer_mut(p, layer)?.text {
            return invalid("the layer is not a text layer: make one with layer.new, kind text");
        }
        let drawing = drawn_on_or_blank(p, layer, frame, hold)?;
        let entry = text_drawing(p, layer, drawing.0)?;
        entry.texts.push(TextBox { text, ..TextBox::new(at, width, format) });
        let missing = laid_out(entry);
        Ok((drawing, entry.texts.len() - 1, missing))
    })?;
    Ok(json!({ "drawing": drawing, "box": index, "standIn": missing }))
}

/// Types into a box: its letters from `from` up to `to` (places between letters, from 0)
/// replaced by `insert`. Both at the same place, it is put in there; `insert` empty, the
/// letters are deleted. Typed on into the same box, with nothing else done between, it joins
/// the step of the history before it, unless `newStep` says otherwise.
pub(super) fn type_text(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        drawing: u64,
        #[serde(rename = "box")]
        index: usize,
        from: usize,
        to: usize,
        #[serde(default)]
        insert: String,
        /// A step of the undo history of its own, not joined to the typing before it.
        #[serde(default, rename = "newStep")]
        new_step: bool,
    }
    let id = "text.type";
    let Params { layer, drawing, index, from, to, insert, new_step } = parse(id, params)?;
    if to < from {
        return invalid("`to` is not before `from`");
    }
    let missing = in_one_step(s, (id, layer, drawing, index), new_step, |p| {
        let entry = text_drawing(p, layer, drawing)?;
        text_box(entry, index)?.splice(from, to, &insert);
        Ok(laid_out(entry))
    })?;
    Ok(json!({ "standIn": missing }))
}

/// Edits the project with `f`, as the command `run.0` of the box `run`: typed on into the same
/// box, or its format changed on, with nothing else done since, it joins the step before it,
/// unless `new_step` says otherwise.
fn in_one_step<T>(s: &mut Session, run: (&'static str, u64, u64, usize), new_step: bool, f: impl FnOnce(&mut Project) -> Result<T>) -> Result<T> {
    let before = s.history.mark();
    let done = s.edit(title(run.0), f)?;
    let start = match s.typing {
        Some((was, start, after)) if !new_step && was == run && after == before => {
            s.history.fold(start, title(run.0), true);
            start
        }
        _ => before,
    };
    s.typing = Some((run, start, s.history.mark()));
    Ok(done)
}

/// Changes a box's format: the fields of `format` that are given, the others kept. Changed on,
/// with nothing else done between, it joins the step before it, unless `newStep` says
/// otherwise: a field dragged is one step.
pub(super) fn set_format(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        drawing: u64,
        #[serde(rename = "box")]
        index: usize,
        format: serde_json::Map<String, Value>,
        #[serde(default, rename = "newStep")]
        new_step: bool,
    }
    let id = "text.setFormat";
    let Params { layer, drawing, index, format, new_step } = parse(id, params)?;
    let missing = in_one_step(s, (id, layer, drawing, index), new_step, |p| {
        let entry = text_drawing(p, layer, drawing)?;
        let Value::Object(mut merged) = json!(text_box(entry, index)?.format) else { unreachable!("a format is an object") };
        merged.extend(format);
        let new: TextFormat = match serde_json::from_value(Value::Object(merged)) {
            Ok(format) => format,
            Err(e) => return invalid(format!("the format can't be read: {e}")),
        };
        checked_format(p, &new)?;
        let entry = text_drawing(p, layer, drawing)?;
        text_box(entry, index)?.format = new;
        Ok(laid_out(entry))
    })?;
    Ok(json!({ "standIn": missing }))
}

/// Moves a box (`at`, its top left corner) or sets its width (`width`, a number, or `null`
/// for as wide as its longest line) or its height (`height`, the same, for as tall as its
/// lines). What is not given is kept.
pub(super) fn set_box(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        drawing: u64,
        #[serde(rename = "box")]
        index: usize,
        at: Option<[f64; 2]>,
        #[serde(default, deserialize_with = "given")]
        width: Option<Option<f64>>,
        #[serde(default, deserialize_with = "given")]
        height: Option<Option<f64>>,
    }
    fn given<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<Option<f64>>, D::Error> {
        Option::<f64>::deserialize(d).map(Some)
    }
    let id = "text.setBox";
    let Params { layer, drawing, index, at, width, height } = parse(id, params)?;
    let missing = s.edit(title(id), |p| {
        let entry = text_drawing(p, layer, drawing)?;
        let text = text_box(entry, index)?;
        let (at, width, height) = (at.unwrap_or(text.at), width.unwrap_or(text.width), height.unwrap_or(text.height));
        checked_place(at, width)?;
        checked_place(at, height)?;
        (text.at, text.width, text.height) = (at, width, height);
        Ok(laid_out(entry))
    })?;
    Ok(json!({ "standIn": missing }))
}

/// Kerning by hand: the space before the letter at the place `at` of a box, in thousandths of
/// its size. 0 takes it away.
pub(super) fn set_kerning(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        drawing: u64,
        #[serde(rename = "box")]
        index: usize,
        at: u32,
        kerning: f64,
    }
    let id = "text.setKerning";
    let Params { layer, drawing, index, at, kerning } = parse(id, params)?;
    if !kerning.is_finite() {
        return invalid("kerning is a number");
    }
    let missing = s.edit(title(id), |p| {
        let entry = text_drawing(p, layer, drawing)?;
        let text = text_box(entry, index)?;
        if at as usize >= text.letters() {
            return invalid(format!("the box has no letter at {at}"));
        }
        match kerning {
            0.0 => text.kerning.remove(&at),
            kerning => text.kerning.insert(at, kerning),
        };
        Ok(laid_out(entry))
    })?;
    Ok(json!({ "standIn": missing }))
}

/// Takes a box out of its drawing. A drawing left with no box is left empty.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        drawing: u64,
        #[serde(rename = "box")]
        index: usize,
    }
    let id = "text.delete";
    let Params { layer, drawing, index } = parse(id, params)?;
    s.edit(title(id), |p| {
        let entry = text_drawing(p, layer, drawing)?;
        text_box(entry, index)?;
        entry.texts.remove(index);
        laid_out(entry);
        Ok(())
    })?;
    Ok(json!({}))
}

/// Turns a box of text (`box`), or all of a drawing's, into plain vector shapes: what it is
/// drawn as is kept, its glyphs on the line art and its background on the colour art, and it
/// is no longer text, so that it looks the same on a machine without its font.
pub(super) fn to_shapes(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        drawing: u64,
        #[serde(rename = "box")]
        index: Option<usize>,
    }
    let id = "text.toShapes";
    let Params { layer, drawing, index } = parse(id, params)?;
    let missing = s.edit(title(id), |p| {
        let entry = text_drawing(p, layer, drawing)?;
        if let Some(index) = index {
            text_box(entry, index)?;
        } else if entry.texts.is_empty() {
            return invalid("the drawing has no text");
        }
        Ok(json!(entry.texts_to_shapes(index, fonts())))
    })?;
    Ok(json!({ "standIn": missing }))
}
