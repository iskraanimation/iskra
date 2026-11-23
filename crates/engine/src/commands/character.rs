//! Character import (plan §4.14, R18): a design from a layered file brought into a shot with
//! every part where it is in the document, ready to rig.

use std::path::PathBuf;
use std::sync::Arc;

use iskra_io::{Character, Imported};
use iskra_model::{
    Animation, Drawing, DrawingId, LayerId, Media, MediaId, Peg, PegId, PoolDrawing, Project, Role, ShotId, Skin, SkinId, Transform, Variant, unique_name,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{invalid, loaded, parse, shot_id, title};
use crate::{Result, Session};

/// What the import made, by its ids.
#[derive(Default)]
struct Made {
    pegs: Vec<u64>,
    layers: Vec<u64>,
}

/// Bring the layers of a design into a shot, the bottom of the stack first, so that each is
/// connected over the ones before it: each under a peg of its own, and those under `top`. A
/// pixel of the document is a scene unit, and the document's middle is the frame's.
fn bring(p: &mut Project, shot: ShotId, design: &Character, top: PegId, names: &mut Vec<String>, made: &mut Made) {
    let named = |name: &str, names: &mut Vec<String>| {
        let name = unique_name(if name.is_empty() { "Layer" } else { name }, names.iter().map(String::as_str));
        names.push(name.clone());
        name
    };
    let (half_width, half_height) = (design.width as i32 / 2, design.height as i32 / 2);
    for Imported { name, drawings, shown } in &design.nodes {
        let mut layer = p.new_layer(&named(name, names), Role::Animation);
        // A peg over every layer, to move, turn and key the part by.
        let peg = PegId(p.new_id());
        let over = Peg {
            id: peg,
            name: named(&format!("{}-P", layer.name), names),
            parent: Some(top),
            transform: Transform::default(),
            animation: Animation::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        };
        (layer.density, layer.parent) = (1.0, Some(peg));
        let whole = p.shot(shot).map_or(1, |shot| shot.duration.max(1));
        let mut taken: Vec<String> = Vec::new();
        for (index, part) in drawings.iter().enumerate() {
            let id = DrawingId(p.new_id());
            // A drawing is named after its part of the file; two of one name get a number.
            let name = unique_name(if part.name.is_empty() { "1" } else { &part.name }, taken.iter().map(String::as_str));
            taken.push(name.clone());
            // Its line art and overart are drawn on in free colour, as bitmap art is.
            let drawing = Drawing::of_picture_at(part.left - half_width, part.top - half_height, part.width, part.height, &part.rgba);
            layer.drawings.push(PoolDrawing { id, name, drawing: Arc::new(drawing) });
            if index == *shown {
                layer.exposure.set(0, whole, Some(id));
            }
        }
        let id = layer.id;
        made.layers.push(id.0);
        made.pegs.push(peg.0);
        if let Some(shot) = p.shot_mut(shot) {
            shot.pegs.push(Arc::new(over));
            shot.layers.insert(0, Arc::new(layer));
            shot.connect_on_top(id);
        }
    }
}

/// The skin of the project called `name`, made if it has none.
pub(super) fn skin_called(p: &mut Project, name: &str) -> SkinId {
    if let Some(skin) = p.skin_named(name) {
        return skin.id;
    }
    let id = SkinId(p.new_id());
    Arc::make_mut(&mut p.palette).skins.push(Skin { id, name: name.to_string() });
    id
}

/// The parts of `skin`, as the file has them, made the variants in it of the drawings of
/// `layers`, each of the part of the file it was brought in from (`from`, by its name): a
/// part by the layer of its name, and in a slot each of its drawings by the drawing of its
/// name. A layer of one drawing takes a part of one, whatever its name. Brought again over a
/// variant there is, it takes its place. What matches nothing is said in `said`.
pub(super) fn bring_skin(
    p: &mut Project,
    skin: SkinId,
    name: &str,
    parts: &[Imported],
    layers: &[(String, LayerId)],
    origin: (i32, i32),
    said: &mut Vec<String>,
) {
    for part in parts {
        let Some((_, layer)) = layers.iter().find(|(from, _)| *from == part.name) else {
            said.push(format!("{} in {name}: no part of that name to be its variant", part.name));
            continue;
        };
        let Some(days) = p.layer(*layer).map(|layer| layer.drawings.iter().map(|day| (day.id, day.name.clone())).collect::<Vec<_>>()) else { continue };
        for drawing in &part.drawings {
            let day = match days.iter().find(|(_, named)| *named == drawing.name) {
                Some((id, _)) => *id,
                None if days.len() == 1 && part.drawings.len() == 1 => days[0].0,
                None => {
                    said.push(format!("{} in {name}: {} has no drawing {}", part.name, part.name, drawing.name));
                    continue;
                }
            };
            let pixels = Drawing::of_picture_at(drawing.left - origin.0, drawing.top - origin.1, drawing.width, drawing.height, &drawing.rgba);
            let id = DrawingId(p.new_id());
            let Some(layer) = p.layer_mut(*layer) else { continue };
            let day_name = days.iter().find(|(id, _)| *id == day).map_or(String::new(), |(_, named)| named.clone());
            layer.variants.retain(|variant| !(variant.skin == skin && variant.of == day));
            layer.variants.push(Variant { skin, of: day, drawing: PoolDrawing { id, name: day_name, drawing: Arc::new(pixels) } });
        }
    }
}

/// Brings skins in from the layered file at `path` onto a character that is in the shot
/// already (plan §4.8): the layers under `peg`, or every layer of the shot without one. A
/// part of the file is not made a layer but the variant of the drawing of its name, as
/// [`bring_skin`] matches them; brought again over a variant there is, it takes its place,
/// so a design changed in the paint program comes in over the rig, its pegs, bones and
/// animation kept. What is in a `[skin Name]` group, or is a layer tagged so, is of that
/// skin; what has no such tag is of the skin `name`, which a file with untagged parts needs.
/// A skin is made if the palette has none of that name.
///
/// Returns the `skins` brought, by their ids, and in `said` what matched nothing and which
/// drawings of the layers given variants each skin is still without.
pub(super) fn import_skins(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        path: PathBuf,
        shot: Option<u64>,
        peg: Option<u64>,
        name: Option<String>,
    }
    let id = "skin.import";
    let Params { path, shot, peg, name } = parse(id, params)?;
    if !iskra_io::is_character(&path) {
        return invalid(format!("{} is not a Photoshop file (.psd, .psb) nor an OpenRaster one (.ora)", path.display()));
    }
    let shot = shot_id(s.project(), shot)?;
    let Some(found) = s.project().shot(shot) else { return invalid("no such shot") };
    loaded(found)?;
    let peg = peg.map(PegId);
    if peg.is_some_and(|peg| found.peg(peg).is_none()) {
        return invalid("the shot has no such peg");
    }
    let under = |layer: &iskra_model::Layer| peg.is_none_or(|peg| found.ancestors(layer.parent).any(|above| above.id == peg));
    let layers: Vec<(String, LayerId)> = found.layers.iter().filter(|layer| under(layer)).map(|layer| (layer.name.clone(), layer.id)).collect();
    let design = match iskra_io::read_character(&path) {
        Ok(design) => design,
        Err(why) => return invalid(why),
    };
    let mut skins = design.skins.clone();
    if !design.nodes.is_empty() {
        let Some(name) = name.as_deref().map(str::trim).filter(|name| !name.is_empty()) else {
            return invalid("parts of the file have no [skin …] tag: say which skin they are (`name`)");
        };
        skins.insert(0, (name.to_string(), design.nodes.clone()));
    }
    let origin = (design.width as i32 / 2, design.height as i32 / 2);
    let mut said = design.skipped.clone();
    let brought = s.edit("Import Skins", |p| {
        let mut brought = Vec::new();
        for (name, parts) in &skins {
            let skin = skin_called(p, name);
            bring_skin(p, skin, name, parts, &layers, origin, &mut said);
            // Of the layers it was brought onto, the drawings it is without.
            let mut reached: Vec<LayerId> = Vec::new();
            for found in parts.iter().filter_map(|part| layers.iter().find(|(from, _)| *from == part.name).map(|(_, id)| *id)) {
                if !reached.contains(&found) {
                    reached.push(found);
                }
            }
            for layer in reached.iter().filter_map(|id| p.layer(*id)) {
                let lacking: Vec<&str> = layer.drawings.iter().filter(|day| layer.variant(skin, day.id).is_none()).map(|day| day.name.as_str()).collect();
                if !lacking.is_empty() {
                    said.push(format!("{name} has no variant of {} {}", layer.name, lacking.join(", ")));
                }
            }
            brought.push(skin.0);
        }
        Ok(brought)
    })?;
    Ok(json!({ "skins": brought, "said": said }))
}

/// Brings a character design in from the layered file at `path`, a Photoshop file (`.psd`,
/// `.psb`) or an OpenRaster one (`.ora`, which Krita, MyPaint and GIMP write), into a
/// shot: every layer of the file as a layer here, where it is in the document, a pixel of the
/// document a unit of the scene and its middle the frame's, stacked as in the file; a peg
/// over every layer, named after it with "-P"; and one peg over all of those, named after the
/// file, to move the whole character by. The pixels are kept in the project, as drawings in
/// full colour, as they are painted: a layer's opacity and fill are not applied.
///
/// A tag at the start of a layer's or a group's name says what else to do with it, and the
/// name is taken without it: `[ignore]` leaves the layer, or the group and all in it, out;
/// `[merge]` flattens a group into one drawing; `[slot]` makes a group one layer, with each
/// layer in it, and each group in it flattened, a drawing of that layer to swap between, the
/// one that is seen in the file being the one that is shown. A group with no tag is looked
/// through: what is in it is brought in as if it lay where the group does. Whether a layer or
/// a group is hidden in the file makes no difference: it is brought in and seen.
///
/// `[skin Name]` makes what a group holds, or a layer alone, the variants in the skin `Name`
/// (plan §4.8) of the parts of the same names, made if the project has no skin so called. A
/// file all of whose parts are in skins needs `day`, the name of the skin whose parts are the
/// day drawings.
///
/// The file is listed with the project's media too, in Master, as the picture it is when it
/// is read flat, unless it is there already or can't be read so (an OpenRaster file): `media`
/// is that item's id.
/// Returns the `peg` over all of it, the `pegs` and `layers` that were made, and what was
/// `skipped` and why.
pub(super) fn import(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        path: PathBuf,
        shot: Option<u64>,
        day: Option<String>,
    }
    let id = "character.import";
    let Params { path, shot, day } = parse(id, params)?;
    if !iskra_io::is_character(&path) {
        return invalid(format!("{} is not a Photoshop file (.psd, .psb) nor an OpenRaster one (.ora)", path.display()));
    }
    let shot = shot_id(s.project(), shot)?;
    if let Some(shot) = s.project().shot(shot) {
        loaded(shot)?;
    }
    let mut design = match iskra_io::read_character(&path) {
        Ok(design) => design,
        Err(why) => return invalid(why),
    };
    // Its parts all in skins: the one that is said to be the day one is the day drawings.
    if design.nodes.is_empty() {
        let names: Vec<&str> = design.skins.iter().map(|(name, _)| name.as_str()).collect();
        let Some(at) = day.as_deref().and_then(|day| names.iter().position(|name| *name == day.trim())) else {
            return invalid(format!("every part of the file is in a skin: say which is the day one (`day`), of {}", names.join(", ")));
        };
        design.nodes = design.skins.remove(at).1;
    }
    let name = path.file_stem().map_or("Character".to_string(), |stem| stem.to_string_lossy().into_owned());
    // The file as the project's media would name it, and what it is as a picture.
    let folder = s.path().and_then(|file| file.parent()).map(PathBuf::from);
    let listed = iskra_io::probe(&path).ok().map(|kind| (Media::path_from(folder.as_deref(), &path), kind));
    let mut said = design.skipped.clone();
    let (top, made, media) = s.edit(title(id), |p| {
        let mut names: Vec<String> = p.shot(shot).into_iter().flat_map(|shot| shot.node_names()).map(str::to_string).collect();
        let top = PegId(p.new_id());
        let named = unique_name(&name, names.iter().map(String::as_str));
        names.push(named.clone());
        if let Some(shot) = p.shot_mut(shot) {
            shot.pegs.push(Arc::new(Peg {
                id: top,
                name: named,
                parent: None,
                transform: Transform::default(),
                animation: Animation::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            }));
        }
        let mut made = Made::default();
        bring(p, shot, &design, top, &mut names, &mut made);
        // The skins' parts, as variants of the drawings of the layers just made.
        let layers: Vec<(String, LayerId)> = design.nodes.iter().map(|node| node.name.clone()).zip(made.layers.iter().map(|id| LayerId(*id))).collect();
        let origin = (design.width as i32 / 2, design.height as i32 / 2);
        for (name, parts) in &design.skins {
            let skin = skin_called(p, name);
            bring_skin(p, skin, name, parts, &layers, origin, &mut said);
        }
        let media = listed.map(|(file, kind)| match p.media.iter().find(|media| media.path == file) {
            Some(there) => there.id,
            None => {
                let id = MediaId(p.new_id());
                let name = unique_name(&name, p.media.iter().map(|media| media.name.as_str()));
                p.media.push(Arc::new(Media { id, name, bin: None, path: file, kind, transcript: Vec::new() }));
                id
            }
        });
        Ok((top, made, media))
    })?;
    let layers: Vec<LayerId> = made.layers.iter().map(|id| LayerId(*id)).collect();
    Ok(
        json!({ "peg": top.0, "media": media.map(|media| media.0), "pegs": made.pegs, "layers": layers.iter().map(|id| id.0).collect::<Vec<_>>(), "skipped": said }),
    )
}
