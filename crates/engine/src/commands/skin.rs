//! Skins (plan §4.8): named sets of drawings shown in place of the day ones, as palette styles
//! are colours shown in place of the swatches' own. A variant is a drawing of its own; the
//! exposure keeps naming the day drawing, so nothing here retimes a layer.

use std::sync::Arc;

use iskra_model::{DrawingId, LayerId, PoolDrawing, Project, Skin, SkinId, Variant};
use serde::Deserialize;
use serde_json::{Value, json};

use super::layer::layer_mut;
use super::{checked_name, invalid, loaded, parse, shot_id, title};
use crate::{Result, Session};

/// The drawing a change to `frame` of `layer` is made on: the one exposed there, or where the
/// shot is shown in a skin, its variant in that skin, made as a copy of it the first time.
/// None in an empty cell.
pub(super) fn drawn_on(p: &mut Project, layer: u64, frame: u32) -> Result<Option<DrawingId>> {
    let Some(day) = layer_mut(p, layer)?.exposure.at(frame) else { return Ok(None) };
    variant_for(p, layer, day).map(Some)
}

/// The drawing a change to the day drawing `day` of `layer` is made on: itself, or where the
/// shot is shown in a skin, its variant in that skin, made as a copy of it the first time.
pub(super) fn variant_for(p: &mut Project, layer: u64, day: DrawingId) -> Result<DrawingId> {
    let id = LayerId(layer);
    let Some(shot) = p.shot_of(id) else { return invalid(format!("no layer {layer}")) };
    let skin = shot.skin.filter(|skin| p.skin(*skin).is_some());
    let Some(found) = shot.layer(id) else { return invalid(format!("no layer {layer}")) };
    let Some(skin) = skin else { return Ok(day) };
    if let Some(variant) = found.variant(skin, day) {
        return Ok(variant.drawing.id);
    }
    let Some(entry) = found.drawings.iter().find(|entry| entry.id == day).cloned() else { return Ok(day) };
    let made = DrawingId(p.new_id());
    layer_mut(p, layer)?.variants.push(Variant { skin, of: day, drawing: PoolDrawing { id: made, ..entry } });
    Ok(made)
}

/// The skin a command names, which the project has to have.
fn skin_id(p: &Project, skin: u64) -> Result<SkinId> {
    match p.skin(SkinId(skin)) {
        Some(skin) => Ok(skin.id),
        None => invalid(format!("the project has no skin {skin}")),
    }
}

/// No two skins share a name: `name` has to be free, or be the name of `own` already.
fn skin_name_is_free(p: &Project, name: &str, own: Option<SkinId>) -> Result<()> {
    match p.skins().iter().any(|skin| Some(skin.id) != own && skin.name == name) {
        true => invalid(format!("the project already has a skin named `{name}`")),
        false => Ok(()),
    }
}

/// Makes a skin, with no variants yet: every drawing shows as it is in it. Without a `name`,
/// "Skin 1" or the next that is free. With a `shot`, that shot is shown in it.
pub(super) fn add(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        name: Option<String>,
        shot: Option<u64>,
    }
    let id = "skin.add";
    let Params { name, shot } = parse(id, params)?;
    let skin = s.edit(title(id), |p: &mut Project| {
        let shot = shot.map(|shot| shot_id(p, Some(shot))).transpose()?;
        let name = match &name {
            Some(name) => checked_name(name)?.to_string(),
            None => (1..).map(|n| format!("Skin {n}")).find(|name| p.skin_named(name).is_none()).expect("a free number"),
        };
        skin_name_is_free(p, &name, None)?;
        let skin = SkinId(p.new_id());
        Arc::make_mut(&mut p.palette).skins.push(Skin { id: skin, name });
        if let Some(shot) = shot.and_then(|shot| p.shot_mut(shot)) {
            shot.skin = Some(skin);
        }
        Ok(skin)
    })?;
    Ok(json!({ "skin": skin.0 }))
}

/// Gives a skin another name.
pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        skin: u64,
        name: String,
    }
    let id = "skin.rename";
    let Params { skin, name } = parse(id, params)?;
    s.edit(title(id), |p: &mut Project| {
        let skin = skin_id(p, skin)?;
        let name = checked_name(&name)?.to_string();
        skin_name_is_free(p, &name, Some(skin))?;
        if let Some(found) = Arc::make_mut(&mut p.palette).skins.iter_mut().find(|found| found.id == skin) {
            found.name = name;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Removes a skin with its variants, from every shot. The shots that were shown in it show
/// the day drawings.
pub(super) fn remove(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        skin: u64,
    }
    let id = "skin.remove";
    let Params { skin } = parse(id, params)?;
    s.edit(title(id), |p: &mut Project| {
        let skin = skin_id(p, skin)?;
        Arc::make_mut(&mut p.palette).skins.retain(|other| other.id != skin);
        for shot in &mut p.shots {
            let with = shot.skin == Some(skin) || shot.layers.iter().any(|layer| layer.variants.iter().any(|variant| variant.skin == skin));
            if !with {
                continue;
            }
            let shot = Arc::make_mut(shot);
            shot.skin = shot.skin.filter(|shown| *shown != skin);
            for layer in &mut shot.layers {
                if layer.variants.iter().any(|variant| variant.skin == skin) {
                    Arc::make_mut(layer).variants.retain(|variant| variant.skin != skin);
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Gives the day drawing `drawing` of `layer` a variant in `skin`: a copy of it, to be drawn
/// over. Its id is the variant's, which strokes are drawn on.
pub(super) fn add_variant(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        drawing: u64,
        skin: u64,
    }
    let id = "skin.addVariant";
    let Params { layer, drawing, skin } = parse(id, params)?;
    let made = s.edit(title(id), |p: &mut Project| {
        let (skin, of) = (skin_id(p, skin)?, DrawingId(drawing));
        let Some(shot) = p.shot_of(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
        loaded(shot)?;
        let Some(found) = shot.layer(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
        let Some(day) = found.drawings.iter().find(|entry| entry.id == of).cloned() else {
            return invalid(format!("layer {layer} has no drawing {drawing}"));
        };
        if found.variant(skin, of).is_some() {
            return invalid(format!("drawing {drawing} has a variant in that skin already"));
        }
        let made = DrawingId(p.new_id());
        let Some(found) = p.layer_mut(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
        found.variants.push(Variant { skin, of, drawing: PoolDrawing { id: made, ..day } });
        Ok(made)
    })?;
    Ok(json!({ "id": made.0 }))
}

/// Takes the variant of the day drawing `drawing` of `layer` in `skin` away: where the skin is
/// shown, the day drawing shows again.
pub(super) fn remove_variant(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        drawing: u64,
        skin: u64,
    }
    let id = "skin.removeVariant";
    let Params { layer, drawing, skin } = parse(id, params)?;
    s.edit(title(id), |p: &mut Project| {
        let (skin, of) = (skin_id(p, skin)?, DrawingId(drawing));
        let Some(found) = p.layer_mut(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
        let before = found.variants.len();
        found.variants.retain(|variant| !(variant.skin == skin && variant.of == of));
        if found.variants.len() == before {
            return invalid(format!("drawing {drawing} has no variant in that skin"));
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Shows `shot` in `skin`, or with none in the day drawings.
pub(super) fn set_shot_skin(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        skin: Option<u64>,
    }
    let id = "shot.setSkin";
    let Params { shot, skin } = parse(id, params)?;
    s.edit(title(id), |p: &mut Project| {
        let shot = shot_id(p, shot)?;
        let skin = skin.map(|skin| skin_id(p, skin)).transpose()?;
        if let Some(shot) = p.shot_mut(shot) {
            shot.skin = skin;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
