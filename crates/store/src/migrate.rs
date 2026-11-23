//! Older project files: their structure is brought to the current shape when they are read.
//! The file itself is rewritten by the next save, which first keeps a copy of the old file.

use serde_json::{Value, json};

/// The structure of a version `version` file in the shape this build reads. Anything unexpected
/// is left as it is, for the reader to report.
pub(crate) fn to_current(version: i64, mut document: Value) -> Value {
    if version < 2 {
        storyboard_from_shot_list(&mut document);
    }
    if version < 3 {
        shots_without_pegs(&mut document);
    }
    if version < 5 {
        elements_to_layers(&mut document);
    }
    if version < 8 {
        composites_from_layer_order(&mut document);
    }
    if version < 13 {
        shots_get_cameras(&mut document);
    }
    if version < 23 {
        palette_keeps_its_ids(&mut document);
    }
    if version < 28 {
        shots_have_a_style_of_each_palette(&mut document);
    }
    if version < 39 {
        follow_by_to_inherit(&mut document);
    }
    if version < 45 {
        pass_composites_to_pass_nodes(&mut document);
    }
    if version < 48 {
        drawings_get_overart(&mut document);
    }
    if version < 51 {
        skins_into_the_palette(&mut document);
    }
    if version < 54 {
        groups_fold_their_rows_too(&mut document);
    }
    // Version 62 has a shot's retake notes (`notes`, plan §9.6). A shot without them has none.
    // Version 61 lets a key end a cycle of keyframes (`cycle`), repeated after it. A key
    // without one ends none.
    // Version 60 lets a placement of a gradient or a picture be skewed (`across`) and a
    // radial gradient's focal point be off its middle (`focus`), and a picture be shown
    // `once`. A placement without them is square to its line, its focus in the middle; a
    // picture without it is tiled or has its edges drawn out, as `tile` says.
    // Version 59 has gradient and texture swatches (plan §4.20): a swatch's and a style's
    // `kind`, and a drawing's placements. A swatch without one is solid, a drawing without
    // them has its gradients fitted to its areas.
    // Version 58 has text layers (plan §4.19): a layer's `text`, and a drawing's boxes of
    // text. A layer without it is a drawing layer, a drawing without them holds no text.
    // Version 57 has the pegs wired into the start and end ports of a chain or a curve. A
    // deformer without them has none.
    // Version 56 gives a chain of bones IK. A chain without it has none.
    // Version 55 has the rows of a shot's timeline that are locked. A shot without them has
    // every row unlocked.
    // Version 54 folds a group on the timeline apart from the node view (above).
    // Version 53 gives a shot a scene and a sequence, names to organise the shots by. A shot
    // without them is in none.
    // Version 52 lets a key have a timing chart of the way to the next (`chart`, points of
    // time and how far along). A key without one is timed by its eases, as before.
    // Version 51 keeps the skins in the palette, beside the styles (above).
    // Version 50 has skins (plan §4.8): the project's list of them, a shot's skin shown, and
    // a layer's variants of its drawings in them. Without them there are none.
    // Version 49 lets a peg that follows bones bend the drawings right under it (`bends` on
    // its `follows`). A peg without it carries them, as before.
    // Version 48 has a third art layer, the overart (above).
    // Version 47 has vector art (plan §4.16): a layer can make its art layers vector, and the
    // shapes are in a table of their own. A layer without the list has none.
    // Version 46 lets a render record say which pass it was. A record without it is of the
    // shot as itself.
    // Version 45 has passes as nodes of their own (above).
    // Version 44 lets a layer of pictures brought in leave out their alpha, and say how their
    // files keep their colours with it. A layer without either has the alpha, straight.
    // Version 43 lets a layer switch art layers of its drawings off. A layer without them has
    // all of them seen.
    // Version 42 keeps the boards that are opened to be drawn on on the Animation page. A shot
    // without them has every board locked there.
    // Version 41 lets a shot have displays beside its main one, and name the main one. A shot
    // without them has its one display, the main one, named "Display".
    // Version 40 lets a peg keep the names of templates' nodes matched by hand to its rig's. A
    // peg without them has none.
    // Version 38 lets a peg follow a point of a chain of bones or a curve. A peg without it
    // follows nothing.
    // Version 37 lets a parameter of an effect node be curves of tone. A colour correction
    // without them has straight ones, which change nothing.
    // Version 36 has the rows of a shot's timeline that are hidden, and the project's master
    // volume. A shot without them shows every row, and a project without one is as loud as
    // its clips say.
    // Version 35 lets a parameter of an effect node be a name, and a port or a Send take a
    // peg's transform (plan §8.6). A node without either has neither.
    // Version 34 lets a composite say how it orders what it stacks (plan §8.6). One that
    // doesn't say orders by depth, as all did.
    // Version 33 has effect nodes in shots (plan §8), and lets a composite and the display
    // take one. A shot without them has none.
    // Version 32 has markers on a shot, the stretch of it that plays, and the pegs whose rows
    // are folded. A shot without them has no marks, plays whole and has every row open.
    // Version 31 has markers on a storyboard, and the stretch of it that plays. A storyboard
    // without them has no marks and plays whole.
    // Version 30 lets a sound have a transcript: what is spoken in it, with when. A sound
    // without one has none.
    // Version 29 has panels in shots, with what is written of them. A shot without them has
    // the one that every shot has, on its first frame, with nothing written.
    // Version 27 has several named palettes, and says which one a swatch is in. A file
    // without them has the one, *Default*, with every swatch in it.
    // Version 26 lets a layer say which art layers of its drawings are free colour. A layer
    // that doesn't say has none but what its drawings have already.
    // Version 25 has drawing guides in shots: rulers and perspective. A shot without them
    // has none.
    // Version 24 has palette styles, and the one a shot is shown in. A file without them has
    // the palette's own colours everywhere.
    // Version 20 has passes: a list of their names in the project, and a mark on composites.
    // A file without them has the standard list and no composite marked.
    // Version 22 has deformers that are curves, and bones that stretch, with joints that are
    // round. A deformer without them is a chain of bones as version 21 had them.
    // Version 21 has deformers: chains of bones in a shot, and the one a layer is given to.
    // A file without them has none.
    // Version 19 keeps a record of the renders that were made, and a revision on each shot;
    // a file without them has nothing rendered.
    // Version 18 has sound tracks on storyboards; a storyboard without them is silent.
    // Version 17 has media items: pictures and sound that refer to files. A file without them
    // has none.
    // Version 16 lets a camera's lens have keyframes; a camera without them has the one lens.
    // Version 15 lets a peg tilt; a peg without a `tilt` faces the viewer squarely, as all did.
    // Version 14 has bins, and a start timecode on storyboards. A file without them has every
    // item in Master and every storyboard starting at 0.
    // Version 12 gave pegs a depth; a peg without one is at depth 0, where all were.
    // Version 11 gave shots a status; a shot without one reads as a board.
    // Version 10 lets a node say which library template it is a copy from; nothing older does.
    // Version 9 saves where nodes were put in the node view; a shot without `places` has all
    // its nodes laid out by the view.
    // Version 7 let a side of a key have a curve's slope of its own, beside the three kinds it
    // could be; nothing older has one.
    // Version 6 let a position key bend its path; a key without a `bend` is a smooth one, so
    // a version 5 file needs nothing done either.
    // Version 4 gave pegs keyframes. A peg without an `animation` reads as one with no keys, so
    // a version 3 file needs nothing done; the number says who can read the file.
    document
}

/// Version 54 folds a group on the timeline apart from its box in the node view; before, one
/// fold was both. A group that was folded has its rows folded too, so it looks as it did.
/// Groups are wherever there are `members`, in a shot or elsewhere.
fn groups_fold_their_rows_too(document: &mut Value) {
    match document {
        Value::Object(object) => {
            if object.contains_key("members") && object.get("folded") == Some(&Value::Bool(true)) {
                object.insert("rowsFolded".to_string(), Value::Bool(true));
            }
            object.values_mut().for_each(groups_fold_their_rows_too);
        }
        Value::Array(items) => items.iter_mut().for_each(groups_fold_their_rows_too),
        _ => {}
    }
}

/// Version 51 keeps the list of skins in the palette, where the Style node finds them by name
/// as it does the styles; version 50 kept it on the project.
fn skins_into_the_palette(document: &mut Value) {
    let Some(skins) = document.as_object_mut().and_then(|project| project.remove("skins")) else { return };
    if let Some(palette) = document.get_mut("palette").and_then(Value::as_object_mut) {
        palette.insert("skins".to_string(), skins);
    }
}

/// Version 48 gave drawings a third art layer, the overart, over the line art. A drawing from
/// before gets it empty, as vector art where its other art layers are all vector and indexed
/// otherwise; a layer whose art layers were all vector has the overart vector too. Drawings are
/// wherever there are `artLayers`, in a shot's layers or elsewhere.
fn drawings_get_overart(document: &mut Value) {
    match document {
        Value::Object(object) => {
            let kinds = |list: &Value| list.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
            let a_layer = object.contains_key("drawings");
            if let Some(vector) = object.get_mut("vector").filter(|_| a_layer) {
                let had = kinds(vector);
                if had.iter().any(|kind| kind == "colourArt") && had.iter().any(|kind| kind == "lineArt") && !had.iter().any(|kind| kind == "overart") {
                    vector.as_array_mut().into_iter().for_each(|list| list.push(json!("overart")));
                }
            }
            if let Some(layers) = object.get_mut("artLayers").and_then(Value::as_array_mut) {
                let has = layers.iter().any(|layer| layer["kind"] == "overart");
                let all_vector = !layers.is_empty() && layers.iter().all(|layer| layer["content"] == "vector");
                if !has {
                    layers.push(json!({ "kind": "overart", "content": if all_vector { "vector" } else { "indexed" } }));
                }
            }
            object.values_mut().for_each(drawings_get_overart);
        }
        Value::Array(items) => items.iter_mut().for_each(drawings_get_overart),
        _ => {}
    }
}

/// Version 28 gives every named palette styles of its own, and a shot one style of each. A
/// shot from before was shown in one style at most, which is now the first of its list; a
/// style from before is of the first palette.
fn shots_have_a_style_of_each_palette(document: &mut Value) {
    for shot in document["shots"].as_array_mut().into_iter().flatten() {
        if let Some(style) = shot.as_object_mut().and_then(|shot| shot.remove("style")).filter(|style| !style.is_null()) {
            shot["styles"] = json!([style]);
        }
    }
}

/// Version 23 made the palette more than a list of swatches: it also knows which ids it has
/// given out, so that a removed swatch's id is not used again.
fn palette_keeps_its_ids(document: &mut Value) {
    if let Some(swatches) = document.get("palette").filter(|palette| palette.is_array()).cloned() {
        document["palette"] = json!({ "swatches": swatches });
    }
}

/// Version 13 gave every shot a camera. A shot from before gets one at home, with the lens new
/// shots have, which sees the shot as it was: nothing had a size that depended on its depth.
/// In version 38 a peg that followed bones said what of their pose it took, as `by`. From
/// version 39 every peg says what it inherits from above it, which takes that over: a peg
/// that took the position alone inherits no rotation or scale, and one that took the
/// position and rotation no scale.
fn follow_by_to_inherit(document: &mut Value) {
    for shot in document["shots"].as_array_mut().into_iter().flatten() {
        for peg in shot["pegs"].as_array_mut().into_iter().flatten() {
            let Some(by) = peg.get_mut("follows").and_then(Value::as_object_mut).and_then(|follows| follows.remove("by")) else { continue };
            match by.as_str() {
                Some("position") => peg["inherit"] = json!({ "rotation": false, "scale": false }),
                Some("positionRotation") => peg["inherit"] = json!({ "scale": false }),
                _ => {}
            }
        }
    }
}

/// Up to version 44 a pass was a composite marked as one, and the project kept a list of the
/// passes' names. From version 45 a pass is a node of its own, a Pass, kept among the displays:
/// each marked composite gets one wired from it, named as it was, so that the pass's name is
/// the same in every shot, and the composite takes a name of its own beside it. The list goes.
fn pass_composites_to_pass_nodes(document: &mut Value) {
    let Some(mut next) = document["nextId"].as_u64() else { return };
    if let Some(project) = document.as_object_mut() {
        project.remove("passes");
    }
    for shot in document["shots"].as_array_mut().into_iter().flatten() {
        let marked: Vec<(u64, String)> = shot["composites"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .filter_map(|composite| {
                let pass = composite.as_object_mut()?.remove("pass")?.as_bool()?;
                pass.then(|| Some((composite["id"].as_u64()?, composite["name"].as_str()?.to_string()))).flatten()
            })
            .collect();
        for (composite, name) in marked {
            // Every name the shot's nodes have, for the composite's new one to be free of.
            let mut names: Vec<String> = Vec::new();
            for kind in ["layers", "pegs", "composites", "deformers", "groups", "effects", "displays"] {
                names.extend(shot[kind].as_array().into_iter().flatten().filter_map(|node| node["name"].as_str().map(str::to_string)));
            }
            names.extend(shot["displayName"].as_str().map(str::to_string));
            let free = (1..).map(|n| if n == 1 { format!("{name} Composite") } else { format!("{name} Composite {n}") }).find(|free| !names.contains(free));
            for own in shot["composites"].as_array_mut().into_iter().flatten().filter(|own| own["id"].as_u64() == Some(composite)) {
                own["name"] = json!(free);
            }
            let pass = next;
            next += 1;
            let displays = shot.as_object_mut().map(|shot| shot.entry("displays").or_insert_with(|| json!([])));
            if let Some(displays) = displays.and_then(Value::as_array_mut) {
                displays.push(json!({ "id": pass, "name": name, "shows": { "composite": composite }, "pass": true }));
            }
            // In the composite's group, if it is in one.
            for group in shot["groups"].as_array_mut().into_iter().flatten() {
                if let Some(members) = group["members"].as_array_mut().filter(|members| members.iter().any(|member| member.as_u64() == Some(composite))) {
                    members.push(json!(pass));
                }
            }
        }
    }
    document["nextId"] = json!(next);
}

fn shots_get_cameras(document: &mut Value) {
    let Some(mut next) = document["nextId"].as_u64() else { return };
    let height = document["settings"]["height"].as_u64().and_then(|height| u32::try_from(height).ok()).unwrap_or(1080);
    for shot in document["shots"].as_array_mut().into_iter().flatten().filter(|shot| shot.get("camera").is_none()) {
        shot["camera"] = json!(iskra_model::Camera::new(iskra_model::CameraId(next), height));
        next += 1;
    }
    document["nextId"] = json!(next);
}

/// Up to version 7 the order of a shot's layers was which is over which, and all were seen.
/// Version 8 has composites for that: each shot gets one that stacks its layers in that order,
/// and its display shows it.
fn composites_from_layer_order(document: &mut Value) {
    let Some(mut next) = document["nextId"].as_u64() else { return };
    for shot in document["shots"].as_array_mut().into_iter().flatten().filter(|shot| shot.get("composites").is_none()) {
        let inputs: Vec<Value> = shot["layers"].as_array().into_iter().flatten().map(|layer| json!({ "layer": layer["id"] })).collect();
        shot["composites"] = json!([{ "id": next, "name": "Composite", "inputs": inputs }]);
        shot["display"] = json!({ "composite": next });
        next += 1;
    }
    document["nextId"] = json!(next);
}

/// Up to version 4 a shot's layers were called its elements. Version 5 only changed the word.
fn elements_to_layers(document: &mut Value) {
    for shot in document["shots"].as_array_mut().into_iter().flatten() {
        if let Some(elements) = shot.as_object_mut().and_then(|shot| shot.remove("elements")) {
            shot["layers"] = elements;
        }
    }
}

/// Version 3 gave shots pegs, and elements a parent among them. Before it a shot has no pegs,
/// and an element without a parent reads as it is.
fn shots_without_pegs(document: &mut Value) {
    for shot in document["shots"].as_array_mut().into_iter().flatten() {
        shot["pegs"] = json!([]);
    }
}

/// Version 1 had an ordered list of shots and no storyboards. Version 2: the shots are items of
/// the project, laid end to end on one storyboard, and every element has a role.
fn storyboard_from_shot_list(document: &mut Value) {
    let mut clips = Vec::new();
    let mut start = 0;
    for shot in document["shots"].as_array_mut().into_iter().flatten() {
        let duration = shot["duration"].as_u64().unwrap_or(0);
        clips.push(json!({ "shot": shot["id"], "start": start, "in": 0, "out": duration }));
        start += duration;
        for element in shot["elements"].as_array_mut().into_iter().flatten() {
            element["role"] = json!("animation");
        }
    }
    let Some(id) = document["nextId"].as_u64() else { return };
    document["storyboards"] = json!([{ "id": id, "name": "Storyboard", "clips": clips, "transitions": [] }]);
    document["nextId"] = json!(id + 1);
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    #[test]
    fn a_version_53_group_folded_has_its_rows_folded_too() {
        let group = |id: u64, folded: bool| json!({ "id": id, "name": "G", "members": [1], "folded": folded });
        let document = json!({ "shots": [{ "groups": [group(5, true), group(6, false), { "id": 7, "name": "H", "members": [] }] }] });
        let now = super::to_current(53, document);
        let groups = &now["shots"][0]["groups"];
        assert_eq!((groups[0]["rowsFolded"].clone(), groups[1].get("rowsFolded"), groups[2].get("rowsFolded")), (json!(true), None, None));
        assert_eq!(super::to_current(54, now.clone()), now, "once is enough");
    }

    #[test]
    fn a_version_50_projects_skins_go_into_its_palette() {
        let skins = json!([{ "id": 9, "name": "Night" }]);
        let now = super::to_current(50, json!({ "palette": { "swatches": [] }, "skins": skins.clone() }));
        assert_eq!(now, json!({ "palette": { "swatches": [], "skins": skins } }));
        let none = json!({ "palette": { "swatches": [] } });
        assert_eq!(super::to_current(50, none.clone()), none, "a project without skins is as it was");
    }

    #[test]
    fn a_version_47_drawing_gets_empty_overart_of_its_kind() {
        let layer = |vector: Value, content: &str| {
            let art = json!([{ "kind": "colourArt", "content": content }, { "kind": "lineArt", "content": content }]);
            json!({ "vector": vector, "drawings": [{ "id": 1, "name": "1", "drawing": { "artLayers": art } }] })
        };
        let document = json!({ "shots": [{ "layers": [layer(json!([]), "indexed"), layer(json!(["colourArt", "lineArt"]), "vector")] }] });
        let now = super::to_current(47, document);
        let layers = &now["shots"][0]["layers"];
        assert_eq!(layers[0]["drawings"][0]["drawing"]["artLayers"][2], json!({ "kind": "overart", "content": "indexed" }));
        assert_eq!(layers[0]["vector"], json!([]));
        assert_eq!(layers[1]["drawings"][0]["drawing"]["artLayers"][2], json!({ "kind": "overart", "content": "vector" }));
        assert_eq!(layers[1]["vector"], json!(["colourArt", "lineArt", "overart"]));
        assert_eq!(super::to_current(48, now.clone()), now, "once is enough");
    }

    #[test]
    fn a_composite_marked_as_a_pass_gets_a_pass_node_of_its_name() {
        let mut old = json!({
            "nextId": 50,
            "passes": ["Background", "Characters"],
            "shots": [{
                "displayName": "Display",
                "layers": [{ "id": 1, "name": "Sky" }],
                "composites": [{ "id": 2, "name": "Background", "inputs": [{ "layer": 1 }], "pass": true }, { "id": 3, "name": "Composite", "inputs": [{ "composite": 2 }] }],
                "groups": [{ "id": 4, "name": "Set", "members": [2] }]
            }]
        });
        super::pass_composites_to_pass_nodes(&mut old);
        let shot = &old["shots"][0];
        assert_eq!(shot["displays"], json!([{ "id": 50, "name": "Background", "shows": { "composite": 2 }, "pass": true }]));
        assert_eq!((&shot["composites"][0]["name"], shot["composites"][0].get("pass")), (&json!("Background Composite"), None));
        assert_eq!((&shot["groups"][0]["members"], &old["nextId"], old.get("passes")), (&json!([2, 50]), &json!(51), None));
    }

    #[test]
    fn what_a_version_38_peg_took_of_the_bones_it_follows_is_what_it_inherits() {
        let peg = |by: &str| json!({ "id": 5, "follows": { "deformer": 7, "by": by } });
        let old = json!({ "shots": [{ "pegs": [peg("position"), peg("positionRotation"), peg("all"), { "id": 6 }] }] });
        let new = {
            let mut old = old;
            super::follow_by_to_inherit(&mut old);
            old
        };
        let pegs = &new["shots"][0]["pegs"];
        assert_eq!(pegs[0], json!({ "id": 5, "follows": { "deformer": 7 }, "inherit": { "rotation": false, "scale": false } }));
        assert_eq!(pegs[1], json!({ "id": 5, "follows": { "deformer": 7 }, "inherit": { "scale": false } }));
        assert_eq!((&pegs[2], &pegs[3]), (&json!({ "id": 5, "follows": { "deformer": 7 } }), &json!({ "id": 6 })));
    }
}
