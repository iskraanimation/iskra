use std::collections::HashSet;
use std::path::PathBuf;

use iskra_model::{BinId, Edge, FrameRate, Item, MediaId, MediaKind, Role, ShotId, Status, SwatchId};
use serde_json::{Value, json};

use crate::{EngineError, Session, commands};

fn size(s: &Session) -> (u32, u32) {
    (s.project().settings.width, s.project().settings.height)
}

fn resize(s: &mut Session, width: u32, height: u32) {
    s.execute("project.setResolution", json!({ "width": width, "height": height })).unwrap();
}

#[test]
fn command_ids_are_unique_and_namespaced() {
    let mut seen = HashSet::new();
    for c in commands() {
        assert!(seen.insert(c.id), "duplicate id {}", c.id);
        assert!(c.id.split_once('.').is_some_and(|(group, name)| !group.is_empty() && !name.is_empty()), "{} is not group.name", c.id);
        assert!(!c.title.is_empty(), "{} has no title", c.id);
    }
    let listed = Session::new().execute("app.commands", Value::Null).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), commands().len());
}

#[test]
fn a_command_edits_the_project_and_undo_takes_it_back() {
    let mut s = Session::new();
    assert_eq!(size(&s), (1920, 1080));
    let before = s.project().clone();
    resize(&mut s, 3840, 2160);
    assert_eq!(size(&s), (3840, 2160));
    assert_eq!(before.settings.width, 1920, "a snapshot taken earlier does not change");
    assert_eq!(s.history().undo_label(), Some("Set Resolution"));

    assert_eq!(s.execute("edit.undo", Value::Null).unwrap(), json!(true));
    assert_eq!(size(&s), (1920, 1080));
    assert_eq!(s.execute("edit.undo", Value::Null).unwrap(), json!(false), "nothing left to undo");
    assert_eq!(s.execute("edit.redo", Value::Null).unwrap(), json!(true));
    assert_eq!(size(&s), (3840, 2160));
    assert_eq!(s.execute("edit.redo", Value::Null).unwrap(), json!(false));
}

#[test]
fn a_step_after_undo_starts_a_branch_and_keeps_the_old_one() {
    let mut s = Session::new();
    resize(&mut s, 2048, 1080);
    resize(&mut s, 4096, 2160);
    s.undo();
    s.execute("project.setFrameRate", json!({ "num": 25 })).unwrap();
    assert!(!s.history().can_redo());

    // 0 Original → 1 (2048) → 2 (4096), and 3 (25 fps) also grows from 1.
    let entries = s.history().entries();
    assert_eq!(entries.iter().map(|e| e.parent).collect::<Vec<_>>(), [None, Some(0), Some(1), Some(1)]);
    assert!(entries[3].current);

    s.execute("edit.gotoHistory", json!({ "index": 2 })).unwrap();
    assert_eq!(size(&s), (4096, 2160));
    assert_eq!(s.project().settings.frame_rate, FrameRate::FPS_24);
    // Undo and Redo now walk the branch that was jumped to.
    s.undo();
    s.undo();
    assert_eq!(size(&s), (1920, 1080));
    s.redo();
    s.redo();
    assert_eq!(size(&s), (4096, 2160));
    assert!(matches!(s.execute("edit.gotoHistory", json!({ "index": 9 })), Err(EngineError::Invalid(_))));
}

#[test]
fn a_command_that_changes_nothing_adds_no_step() {
    let mut s = Session::new();
    resize(&mut s, 1920, 1080);
    assert!(!s.history().can_undo());
}

#[test]
fn bad_input_is_an_error_and_leaves_the_project_alone() {
    let mut s = Session::new();
    assert!(matches!(s.execute("project.explode", Value::Null), Err(EngineError::UnknownCommand(id)) if id == "project.explode"));
    assert!(matches!(s.execute("project.setResolution", json!({ "width": "wide" })), Err(EngineError::BadParams { .. })));
    assert!(matches!(s.execute("project.setResolution", json!({ "width": 0, "height": 1080 })), Err(EngineError::Invalid(_))));
    assert!(matches!(s.execute("project.setFrameRate", json!({ "num": 24, "den": 0 })), Err(EngineError::Invalid(_))));
    assert_eq!(size(&s), (1920, 1080));
    assert!(!s.history().can_undo());
}

#[test]
fn frame_rates_are_exact_fractions() {
    let mut s = Session::new();
    s.execute("project.setFrameRate", json!({ "num": 24000, "den": 1001 })).unwrap();
    assert_eq!(s.project().settings.frame_rate, FrameRate::FPS_23_976);
    let project = s.execute("project.get", Value::Null).unwrap();
    assert_eq!(project["settings"]["frameRate"], json!({ "num": 24000, "den": 1001 }));
}

/// The id of the new project's one layer.
fn layer(s: &Session) -> u64 {
    s.project().shots[0].layers[0].id.0
}

/// The first layer, made swatch-linked bitmap as layers were before free colour was the
/// default: for what is of swatch-linked pixels alone.
fn swatch_layer(s: &mut Session) -> u64 {
    let e = layer(s);
    s.edit("Swatches", |p| {
        p.layer_mut(iskra_model::LayerId(e)).unwrap().free_colour.clear();
        Ok(())
    })
    .unwrap();
    e
}

/// The names of the drawings exposed on frames `0..frames` of a layer ("-" = empty cell).
fn cells(s: &Session, layer: u64, frames: u32) -> String {
    let layer = s.project().layer(iskra_model::LayerId(layer)).unwrap();
    (0..frames).map(|f| layer.drawing_at(f).map_or("-", |d| d.name.as_str())).collect::<Vec<_>>().join(" ")
}

fn id(v: Value) -> u64 {
    v["id"].as_u64().unwrap()
}

#[test]
fn new_drawings_are_numbered_and_exposed_where_they_are_made() {
    let mut s = Session::new();
    let e = layer(&s);
    let first = id(s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 0 })).unwrap());
    assert_eq!(cells(&s, e, 4), "1 - - -", "one frame in an empty cell");
    s.execute("exposure.set", json!({ "layer": e, "frame": 0, "length": 4, "drawing": first })).unwrap();
    assert_eq!(cells(&s, e, 5), "1 1 1 1 -");

    // Inside a hold, a new drawing takes its frame and the hold carries on after it.
    s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 2 })).unwrap();
    assert_eq!(cells(&s, e, 5), "1 1 2 1 -");
    s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 3, "name": "blink" })).unwrap();
    assert_eq!(cells(&s, e, 5), "1 1 2 blink -");

    s.undo();
    assert_eq!(cells(&s, e, 5), "1 1 2 1 -", "one command is one undo step");
}

#[test]
fn how_long_a_new_drawing_holds_is_the_callers_setting() {
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("shot.setDuration", json!({ "frames": 12 })).unwrap();
    let all = |s: &Session| cells(s, e, 14);

    // Until the next drawing or the end of the shot: boards drawn on a few frames fill it.
    s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 0, "hold": "toNext" })).unwrap();
    assert_eq!(all(&s), "1 1 1 1 1 1 1 1 1 1 1 1 - -", "to the end of the shot, not beyond");
    s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 5, "hold": "toNext" })).unwrap();
    s.execute("drawing.stroke", stroke_params(e, 10, json!({ "size": 20 }))).unwrap();
    assert_eq!(all(&s), "1 1 1 1 1 2 2 2 2 2 2 2 - -", "a stroke on a drawing that shows there makes none");
    s.execute("exposure.set", json!({ "layer": e, "frame": 8, "length": 4, "drawing": null })).unwrap();

    // A fixed number of frames, on twos here, never runs over a later drawing.
    let mut on_twos = stroke_params(e, 10, json!({ "size": 20 }));
    on_twos["hold"] = json!(2);
    s.execute("drawing.stroke", on_twos).unwrap();
    assert_eq!(all(&s), "1 1 1 1 1 2 2 2 - - 3 3 - -", "a stroke in an empty cell holds its new drawing too");
    s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 9, "hold": 4 })).unwrap();
    assert_eq!(all(&s), "1 1 1 1 1 2 2 2 - 4 3 3 - -", "it stops at the next drawing");
    s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 8, "hold": "toNext" })).unwrap();
    assert_eq!(all(&s), "1 1 1 1 1 2 2 2 5 4 3 3 - -");
    // Past the shot's end there is nothing to hold until: one frame.
    s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 13, "hold": "toNext" })).unwrap();
    assert_eq!(all(&s), "1 1 1 1 1 2 2 2 5 4 3 3 - 6");

    // A duplicate takes the rest of the hold it copies, whatever the setting.
    s.execute("drawing.duplicate", json!({ "layer": e, "frame": 2 })).unwrap();
    assert_eq!(all(&s), "1 1 7 7 7 2 2 2 5 4 3 3 - 6");
    for hold in [json!(0), json!("forever"), json!(-1)] {
        assert!(s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 12, "hold": hold })).is_err(), "{hold}");
    }
}

#[test]
fn a_hold_can_be_set_to_a_length_with_or_without_moving_what_follows() {
    let mut s = Session::new();
    let e = layer(&s);
    for frame in [0, 1, 2] {
        s.execute("drawing.newDrawing", json!({ "layer": e, "frame": frame })).unwrap();
    }
    s.execute("exposure.setHold", json!({ "layer": e, "frame": 0, "length": 3 })).unwrap();
    assert_eq!(cells(&s, e, 7), "1 1 1 2 3 - -", "the later drawings move along");
    s.execute("exposure.setHold", json!({ "layer": e, "frame": 3, "length": 3, "ripple": false })).unwrap();
    assert_eq!(cells(&s, e, 7), "1 1 1 2 2 2 -", "or stay, and are covered");
    s.execute("exposure.setHold", json!({ "layer": e, "frame": 1, "length": 1, "ripple": false })).unwrap();
    assert_eq!(cells(&s, e, 7), "1 - - 2 2 2 -", "a shorter hold leaves empty cells");
    s.execute("exposure.setHold", json!({ "layer": e, "frame": 5, "length": 1 })).unwrap();
    assert_eq!((cells(&s, e, 7), s.history().undo_label()), ("1 - - 2 - - -".to_string(), Some("Set Hold")));
    for params in [json!({ "layer": e, "frame": 1, "length": 2 }), json!({ "layer": e, "frame": 0, "length": 0 })] {
        assert!(matches!(s.execute("exposure.setHold", params), Err(EngineError::Invalid(_))));
    }
}

#[test]
fn a_hold_is_extended_to_an_empty_cell() {
    let mut s = Session::new();
    let e = layer(&s);
    for frame in [0, 5] {
        s.execute("drawing.newDrawing", json!({ "layer": e, "frame": frame, "hold": 1 })).unwrap();
    }
    assert_eq!(cells(&s, e, 7), "1 - - - - 2 -");
    s.execute("exposure.extend", json!({ "layer": e, "frame": 3 })).unwrap();
    assert_eq!((cells(&s, e, 7), s.history().undo_label()), ("1 1 1 1 - 2 -".to_string(), Some("Extend Hold")));
    for frame in [2, 5] {
        assert!(matches!(s.execute("exposure.extend", json!({ "layer": e, "frame": frame })), Err(EngineError::Invalid(_))));
    }
}

#[test]
fn a_duplicate_can_change_without_changing_the_original() {
    let mut s = Session::new();
    let e = layer(&s);
    let first = id(s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 0 })).unwrap());
    s.execute("exposure.set", json!({ "layer": e, "frame": 0, "length": 3, "drawing": first })).unwrap();
    let copy = id(s.execute("drawing.duplicate", json!({ "layer": e, "frame": 1 })).unwrap());
    assert_ne!(copy, first);
    assert_eq!(cells(&s, e, 3), "1 2 2");
    let pool = &s.project().shots[0].layers[0].drawings;
    assert!(std::sync::Arc::ptr_eq(&pool[0].drawing, &pool[1].drawing), "the copy shares its pixels until one of them is drawn on");
    assert!(matches!(s.execute("drawing.duplicate", json!({ "layer": e, "frame": 9 })), Err(EngineError::Invalid(_))), "nothing to copy in an empty cell");
}

#[test]
fn deleting_a_drawing_empties_its_cells() {
    let mut s = Session::new();
    let e = layer(&s);
    let first = id(s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 0 })).unwrap());
    let second = id(s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 1 })).unwrap());
    s.execute("exposure.set", json!({ "layer": e, "frame": 2, "drawing": first })).unwrap();
    assert_eq!(cells(&s, e, 3), "1 2 1");
    s.execute("drawing.delete", json!({ "layer": e, "drawing": first })).unwrap();
    assert_eq!(cells(&s, e, 3), "- 2 -");
    // The freed name is given out again, the id is not.
    let third = id(s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 0 })).unwrap());
    assert_eq!(cells(&s, e, 3), "1 2 -");
    assert!(third != first && third != second);
    assert!(matches!(s.execute("exposure.set", json!({ "layer": e, "frame": 0, "drawing": first })), Err(EngineError::Invalid(_))), "not in the pool any more");
    s.execute("exposure.set", json!({ "layer": e, "frame": 0, "length": 2, "drawing": null })).unwrap();
    assert_eq!(cells(&s, e, 3), "- - -");
}

#[test]
fn names_are_unique_among_siblings() {
    let mut s = Session::new();
    let first = layer(&s);
    let second = id(s.execute("layer.new", Value::Null).unwrap());
    let names = |s: &Session| s.project().shots[0].layers.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&s), ["Drawing 2", "Drawing"], "a new layer goes on top and gets a free name");

    let clash = |r: crate::Result<Value>| matches!(r, Err(EngineError::Invalid(why)) if why.contains("already has"));
    assert!(clash(s.execute("layer.new", json!({ "name": "Drawing" }))));
    assert!(clash(s.execute("layer.rename", json!({ "layer": second, "name": " Drawing " }))), "names are trimmed before they are compared");
    s.execute("layer.rename", json!({ "layer": second, "name": "Drawing 2" })).unwrap();
    s.execute("layer.rename", json!({ "layer": first, "name": "Ball" })).unwrap();
    assert_eq!(names(&s), ["Drawing 2", "Ball"]);
    assert!(matches!(s.execute("layer.rename", json!({ "layer": first, "name": "arm/hand" })), Err(EngineError::Invalid(_))));
    assert!(matches!(s.execute("layer.rename", json!({ "layer": first, "name": "  " })), Err(EngineError::Invalid(_))));

    // Drawing names are unique in their pool, and two layers can both have a drawing "1".
    s.execute("drawing.newDrawing", json!({ "layer": first, "frame": 0 })).unwrap();
    let other = id(s.execute("drawing.newDrawing", json!({ "layer": second, "frame": 0 })).unwrap());
    assert_eq!((cells(&s, first, 1), cells(&s, second, 1)), ("1".to_string(), "1".to_string()));
    assert!(clash(s.execute("drawing.newDrawing", json!({ "layer": first, "frame": 1, "name": "1" }))));
    s.execute("drawing.newDrawing", json!({ "layer": second, "frame": 1 })).unwrap();
    assert!(clash(s.execute("drawing.rename", json!({ "layer": second, "drawing": other, "name": "2" }))));
    s.execute("drawing.rename", json!({ "layer": second, "drawing": other, "name": "mouth-A" })).unwrap();
    assert_eq!(cells(&s, second, 2), "mouth-A 2");

    s.execute("layer.delete", json!({ "layer": second })).unwrap();
    assert_eq!(names(&s), ["Ball"]);
    assert!(matches!(s.execute("layer.delete", json!({ "layer": second })), Err(EngineError::Invalid(_))));
}

#[test]
fn a_failed_command_uses_no_id_and_adds_no_step() {
    let mut s = Session::new();
    let e = layer(&s);
    assert!(s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 0, "name": "a/b" })).is_err());
    assert!(s.execute("drawing.newDrawing", json!({ "layer": 999, "frame": 0 })).is_err());
    assert!(s.execute("shot.setDuration", json!({ "frames": 0 })).is_err());
    assert!(s.execute("shot.setDuration", json!({ "shot": 999, "frames": 10 })).is_err());
    assert!(!s.history().can_undo());
    s.execute("shot.setDuration", json!({ "frames": 120 })).unwrap();
    assert_eq!(s.project().shots[0].duration, 120);
}

#[test]
fn a_shot_is_shown_and_rendered_in_a_skin_its_variants_in_place_of_the_day_drawings() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 40, "pressureSize": false }))).unwrap();
    let day = s.project().shots[0].layers[0].drawings[0].id;
    let pixel = |s: &mut Session, name: &str| {
        let path = dir.path().join(name);
        s.execute("shot.renderFrame", json!({ "frame": 0, "path": path })).unwrap();
        let picture = image::open(&path).unwrap().to_rgba8();
        picture.get_pixel(picture.width() / 2 + 125, picture.height() / 2 + 50).0
    };
    assert_eq!(pixel(&mut s, "day.png"), [220, 50, 47, 255]);

    // A skin with a variant of the drawing, painted green where the day one is red.
    let night = s.execute("skin.add", json!({ "name": "Night" })).unwrap()["skin"].as_u64().unwrap();
    assert!(matches!(s.execute("skin.add", json!({ "name": "Night" })), Err(EngineError::Invalid(_))), "names are not given twice");
    let made = s.execute("skin.addVariant", json!({ "layer": e, "drawing": day.0, "skin": night })).unwrap()["id"].as_u64().unwrap();
    assert!(s.execute("skin.addVariant", json!({ "layer": e, "drawing": day.0, "skin": night })).is_err(), "one a drawing a skin");
    s.edit("Paint the variant", |p| {
        let layer = p.layer_mut(iskra_model::LayerId(e)).unwrap();
        let variant = &mut layer.variants[0].drawing;
        assert_eq!(variant.id.0, made);
        let line = &mut std::sync::Arc::make_mut(&mut variant.drawing).layer_mut(iskra_model::ArtLayerKind::LineArt).unwrap().content;
        let iskra_model::ArtContent::Indexed(line) = line else { panic!("swatch art") };
        assert!(line.recolour(SwatchId(3), SwatchId(4)));
        Ok(())
    })
    .unwrap();
    assert_eq!(pixel(&mut s, "still-day.png"), [220, 50, 47, 255], "a skin no shot is shown in changes nothing");
    s.execute("shot.setSkin", json!({ "skin": night })).unwrap();
    assert_eq!(pixel(&mut s, "night.png"), [64, 160, 43, 255]);
    assert_eq!(cells(&s, e, 2), "1 -", "the exposure still names the day drawing");
    assert_eq!(s.project().shots[0].layers[0].drawing_at(0).unwrap().id, day);

    // Saved and opened again, the variant is there with its pixels.
    let path = dir.path().join("skins.iskra");
    s.execute("file.saveAs", json!({ "path": path })).unwrap();
    let mut s = Session::new();
    s.execute("file.open", json!({ "path": path })).unwrap();
    let shot = s.project().shots[0].id.0;
    s.execute("shot.load", json!({ "shot": shot })).unwrap();
    assert_eq!(pixel(&mut s, "reopened.png"), [64, 160, 43, 255]);

    // A copy of the shot has variants of its own, under ids of their own.
    let copy = s.execute("shot.duplicate", json!({ "shot": shot })).unwrap()["id"].as_u64().unwrap();
    let ids = |s: &Session, shot: u64| s.project().shot(iskra_model::ShotId(shot)).unwrap().drawings().map(|d| d.id).collect::<Vec<_>>();
    let (one, two) = (ids(&s, shot), ids(&s, copy));
    assert!(one.len() == 2 && two.len() == 2 && one.iter().all(|id| !two.contains(id)), "{one:?} {two:?}");
    let copied = &s.project().shot(iskra_model::ShotId(copy)).unwrap().layers[0];
    assert_eq!(copied.variants[0].of, copied.drawings[0].id, "the copy's variant is of the copy's drawing");

    // By name, as a style is: Night shows the skin; the palettes' own shows the day drawings.
    s.execute("shot.setStyle", json!({ "shot": shot })).unwrap();
    assert_eq!(pixel(&mut s, "own.png"), [220, 50, 47, 255]);
    s.execute("shot.setStyle", json!({ "shot": shot, "name": "Night" })).unwrap();
    assert_eq!(pixel(&mut s, "by-name.png"), [64, 160, 43, 255]);

    // The variant taken away, the day drawing shows; the skin removed, the shot shows the day.
    let e = s.project().shots[0].layers[0].id.0;
    let day = s.project().shots[0].layers[0].drawings[0].id.0;
    s.execute("skin.removeVariant", json!({ "layer": e, "drawing": day, "skin": night })).unwrap();
    assert_eq!(pixel(&mut s, "removed.png"), [220, 50, 47, 255]);
    s.undo();
    s.execute("skin.remove", json!({ "skin": night })).unwrap();
    let left = s.project().shots.iter().map(|shot| (shot.skin, shot.layers[0].variants.len())).collect::<Vec<_>>();
    assert_eq!((left, s.project().skins().len()), (vec![(None, 0), (None, 0)], 0));
}

#[test]
fn with_a_skin_shown_strokes_are_drawn_on_its_variants() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20 }))).unwrap();
    let day = s.project().shots[0].layers[0].drawings[0].id;
    s.execute("skin.add", json!({ "name": "Night", "shot": s.project().shots[0].id.0 })).unwrap();
    let layer = |s: &Session| s.project().shots[0].layers[0].clone();
    let line = |drawing: &iskra_model::PoolDrawing, x: i32| {
        let iskra_model::ArtContent::Indexed(art) = &drawing.drawing.layer(iskra_model::ArtLayerKind::LineArt).unwrap().content else { panic!() };
        art.pixel(x, 100).swatch.0
    };

    // The first stroke makes the variant, as a copy of the day drawing, and is drawn on it.
    let mut params = stroke_params(e, 0, json!({ "size": 20 }));
    params["swatch"] = json!(4);
    let drawn = s.execute("drawing.stroke", params).unwrap()["drawing"].as_u64().unwrap();
    let now = layer(&s);
    assert_eq!((now.variants.len(), now.variants[0].of, now.variants[0].drawing.id.0), (1, day, drawn));
    assert_eq!((line(&now.drawings[0], 250), line(&now.variants[0].drawing, 250)), (3, 4), "the day drawing is as it was");
    assert_eq!(s.history().undo_label(), Some("Brush Stroke"));
    s.undo();
    assert!(layer(&s).variants.is_empty(), "the variant is made in the stroke's step");
    s.redo();

    // In an empty cell, the day drawing is made empty and the variant drawn.
    let drawn = s.execute("drawing.stroke", stroke_params(e, 3, json!({ "size": 20 }))).unwrap()["drawing"].as_u64().unwrap();
    let now = layer(&s);
    let new_day = now.exposure.at(3).unwrap();
    let variant = now.variant(now.variants[1].skin, new_day).unwrap();
    assert_eq!(variant.drawing.id.0, drawn);
    assert!(now.drawing(new_day).unwrap().drawing.bounds().is_empty() && !variant.drawing.drawing.bounds().is_empty());

    // With the day drawings shown, a stroke is the day drawing's again.
    s.execute("shot.setSkin", json!({})).unwrap();
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20 }))).unwrap();
    assert_eq!(layer(&s).variants.len(), 2, "no variant is made with the day drawings shown");
    assert_eq!(line(&layer(&s).drawings[0], 250), 3);
    // Saved to the library and placed in another project, its variant comes in that
    // project's Night, made there by its name.
    let dir = tempfile::tempdir().unwrap();
    let saved = s.execute("library.save", json!({ "folder": dir.path(), "name": "Hero", "nodes": [{ "layer": e }] })).unwrap();
    let mut other = Session::new();
    other.execute("library.place", json!({ "path": saved["path"] })).unwrap();
    let night = other.project().skin_named("Night").expect("the skin comes with it").id;
    let placed = other.project().shots[0].layers.iter().find(|placed| !placed.variants.is_empty()).unwrap().clone();
    assert!(placed.variants.iter().all(|variant| variant.skin == night && placed.drawings.iter().any(|day| day.id == variant.of)));
    assert_eq!(placed.variants.len(), 2);
    assert!(placed.variants.iter().all(|variant| !variant.drawing.drawing.bounds().is_empty()), "with their pixels");
    // A drawing deleted takes its variants with it.
    s.execute("drawing.delete", json!({ "layer": e, "drawing": day.0 })).unwrap();
    assert!(layer(&s).variants.iter().all(|variant| variant.of != day) && layer(&s).variants.len() == 1);
}

/// A stroke from (100, 100) to (400, 100) in drawing pixels, as command parameters.
fn stroke_params(layer: u64, frame: u32, brush: Value) -> Value {
    let points: Vec<Value> = (0..=30).map(|i| json!({ "x": 100 + i * 10, "y": 100, "pressure": 1.0 })).collect();
    json!({ "layer": layer, "frame": frame, "swatch": 3, "stroke": { "brush": brush, "points": points } })
}

/// The line art pixel at (x, y) of the drawing exposed at `frame`, as (swatch, coverage).
fn line_pixel(s: &Session, layer: u64, frame: u32, x: i32, y: i32) -> Option<(u16, u16)> {
    let drawing = &s.project().layer(iskra_model::LayerId(layer))?.drawing_at(frame)?.drawing;
    let surface = drawing.layer(iskra_model::ArtLayerKind::LineArt)?.content.indexed()?;
    Some((surface.pixel(x, y).swatch.0, surface.pixel(x, y).coverage))
}

#[test]
fn a_stroke_in_an_empty_cell_makes_the_drawing_in_the_same_undo_step() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    let result = s.execute("drawing.stroke", stroke_params(e, 3, json!({ "size": 20 }))).unwrap();
    assert_eq!(cells(&s, e, 5), "- - - 1 -");
    assert_eq!(result["drawing"].as_u64(), Some(s.project().shots[0].layers[0].drawings[0].id.0));
    assert_eq!(line_pixel(&s, e, 3, 250, 100), Some((3, 65535)), "painted with swatch 3");
    assert_eq!(s.history().undo_label(), Some("Brush Stroke"));

    // A second stroke draws on the same drawing.
    s.execute("drawing.stroke", stroke_params(e, 3, json!({ "size": 20 }))).unwrap();
    assert_eq!(s.project().shots[0].layers[0].drawings.len(), 1);
    s.undo();
    s.undo();
    assert_eq!(cells(&s, e, 5), "- - - - -", "undoing the first stroke also takes back its drawing");
    assert!(s.project().shots[0].layers[0].drawings.is_empty());
}

/// Lines from corner to corner of a square, (100, 100) to (400, 400) in drawing pixels, left
/// open between the last corner and the first when `open`.
fn draw_square(s: &mut Session, layer: u64, open: bool) {
    let corners = [(100, 100), (400, 100), (400, 400), (100, 400), (100, 100)];
    for side in corners.windows(2).take(if open { 3 } else { 4 }) {
        let points: Vec<Value> = side.iter().map(|(x, y)| json!({ "x": x, "y": y, "pressure": 1.0 })).collect();
        let stroke = json!({ "brush": { "size": 12, "pressureSize": false }, "points": points });
        s.execute("drawing.stroke", json!({ "layer": layer, "frame": 0, "swatch": 1, "stroke": stroke })).unwrap();
    }
}

/// The colour art pixel at (x, y) of the drawing exposed at frame 0, as (swatch, coverage).
fn colour_pixel(s: &Session, layer: u64, x: i32, y: i32) -> (u16, u16) {
    let drawing = &s.project().layer(iskra_model::LayerId(layer)).unwrap().drawing_at(0).unwrap().drawing;
    let iskra_model::ArtContent::Indexed(surface) = &drawing.layer(iskra_model::ArtLayerKind::ColourArt).unwrap().content else { panic!("indexed") };
    (surface.pixel(x, y).swatch.0, surface.pixel(x, y).coverage)
}

#[test]
fn the_bucket_fills_several_places_recolours_a_line_and_goes_through_every_drawing() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    draw_square(&mut s, e, false);
    let line = |frame: u32, from: (i32, i32), to: (i32, i32)| {
        let points = json!([{ "x": from.0, "y": from.1, "pressure": 1.0 }, { "x": to.0, "y": to.1, "pressure": 1.0 }]);
        json!({ "layer": e, "frame": frame, "swatch": 1, "stroke": { "brush": { "size": 12, "pressureSize": false }, "points": points } })
    };
    // The square cut in two by a line down its middle, and a second drawing of a square alone.
    s.execute("drawing.stroke", line(0, (250, 100), (250, 400))).unwrap();
    for side in [(100, 100), (400, 100), (400, 400), (100, 400), (100, 100)].windows(2) {
        s.execute("drawing.stroke", line(4, side[0], side[1])).unwrap();
    }
    let colour = |s: &Session, frame: u32, x: i32, y: i32| {
        let drawing = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawing_at(frame).unwrap().drawing;
        let iskra_model::ArtContent::Indexed(surface) = &drawing.layer(iskra_model::ArtLayerKind::ColourArt).unwrap().content else { panic!("indexed") };
        surface.pixel(x, y).swatch.0
    };

    // Dragged across both halves and the line between: both are filled, in one step.
    let steps = s.history().entries().len();
    let dragged = json!({ "layer": e, "frame": 0, "swatch": 3, "points": [[150, 250], [250, 250], [350, 250], [351, 250]] });
    let result = s.execute("drawing.fill", dragged).unwrap();
    assert_eq!((result["filled"].clone(), s.history().entries().len(), s.history().undo_label()), (json!(3), steps + 1, Some("Fill")));
    assert_eq!((colour(&s, 0, 150, 250), colour(&s, 0, 350, 250), colour(&s, 4, 150, 250)), (3, 3, 0));
    assert_eq!(result["changed"], json!([104, 104, 396, 396]));

    // On the line art, the line under the place gets the swatch: all of it that touches.
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "swatch": 4, "x": 100, "y": 250, "artLayer": "lineArt" })).unwrap();
    assert_eq!(
        (line_pixel(&s, e, 0, 250, 250), line_pixel(&s, e, 0, 400, 400), s.history().undo_label()),
        (Some((4, 65535)), Some((4, 65535)), Some("Recolour Line"))
    );
    assert_eq!((colour(&s, 0, 150, 250), line_pixel(&s, e, 4, 100, 250)), (3, Some((1, 65535))), "the fills and the other drawing are as they were");

    // On every drawing of the layer: the same place of each, in one step.
    let steps = s.history().entries().len();
    let every = s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "swatch": 5, "x": 150, "y": 250, "everyDrawing": true })).unwrap();
    assert_eq!(
        (every["filled"].clone(), colour(&s, 0, 150, 250), colour(&s, 0, 350, 250), colour(&s, 4, 150, 250), colour(&s, 4, 350, 250)),
        (json!(2), 5, 3, 5, 5)
    );
    assert_eq!(s.history().entries().len(), steps + 1);
    s.undo();
    assert_eq!((colour(&s, 0, 150, 250), colour(&s, 4, 150, 250)), (3, 0));
    // On the drawings exposed on a range of frames: here only the one on frame 4, though
    // the frame given shows the other.
    let ranged = s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "swatch": 5, "x": 150, "y": 250, "frames": [4, 6] })).unwrap();
    assert_eq!((ranged["filled"].clone(), colour(&s, 0, 150, 250), colour(&s, 4, 150, 250)), (json!(1), 3, 5));
    s.undo();
    let both = s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "swatch": 5, "x": 150, "y": 250, "frames": [0, 4] })).unwrap();
    assert_eq!(both["filled"], json!(2));
    s.undo();
    assert!(s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "swatch": 5, "x": 150, "y": 250, "frames": [4, 0] })).is_err());

    // Where no place can be filled it is refused, and says why of the first.
    let steps = s.history().entries().len();
    for refused in [
        json!({ "layer": e, "frame": 0, "swatch": 3, "points": [[250, 250], [50, 50]] }),
        json!({ "layer": e, "frame": 0, "swatch": 3, "points": [] }),
        json!({ "layer": e, "frame": 0, "swatch": 3, "x": 150, "y": 250, "artLayer": "lineArt" }),
        json!({ "layer": e, "frame": 0, "swatch": 3, "x": 100, "y": 250, "artLayer": "lineArt", "protect": [4] }),
    ] {
        assert!(matches!(s.execute("drawing.fill", refused.clone()), Err(EngineError::Invalid(_))), "{refused}");
    }
    assert_eq!(s.history().entries().len(), steps);
}

#[test]
fn the_paint_bucket_fills_the_colour_art_inside_the_lines() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    draw_square(&mut s, e, false);
    let result = s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 250.5, "y": 250.5, "swatch": 3 })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Fill"));
    assert_eq!(result["changed"], json!([104, 104, 396, 396]), "the inside of lines twelve pixels wide, and two pixels under them");
    assert_eq!((colour_pixel(&s, e, 250, 250), colour_pixel(&s, e, 110, 390)), ((3, 65535), (3, 65535)));
    assert_eq!(colour_pixel(&s, e, 50, 250), (0, 0), "nothing outside");
    assert_eq!(line_pixel(&s, e, 0, 100, 250), Some((1, 65535)), "and the lines are as they were");

    // The same fill again is no step; swatch 0 takes the paint away.
    let steps = s.history().entries().len();
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 250, "y": 250, "swatch": 3 })).unwrap();
    assert_eq!(s.history().entries().len(), steps);
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 250, "y": 250 })).unwrap();
    assert_eq!((s.history().undo_label(), colour_pixel(&s, e, 250, 250)), (Some("Unpaint"), (0, 0)));
    s.undo();
    assert_eq!(colour_pixel(&s, e, 250, 250), (3, 65535));

    // What can't be filled is refused, and is no step.
    let steps = s.history().entries().len();
    for refused in [
        json!({ "layer": e, "frame": 0, "x": 100, "y": 250, "swatch": 3 }),
        json!({ "layer": e, "frame": 0, "x": 50, "y": 250, "swatch": 3 }),
        json!({ "layer": e, "frame": 0, "x": 250, "y": 250, "swatch": 77 }),
        json!({ "layer": e, "frame": 9, "x": 250, "y": 250, "swatch": 3 }),
        json!({ "layer": e, "frame": 0, "x": 250, "y": 250, "swatch": 3, "gap": 100_000 }),
    ] {
        assert!(matches!(s.execute("drawing.fill", refused.clone()), Err(EngineError::Invalid(_))), "{refused}");
    }
    assert_eq!(s.history().entries().len(), steps);
}

#[test]
fn the_paint_bucket_stops_at_line_art_of_free_colour_by_its_alpha() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "rgba" })).unwrap();
    draw_square(&mut s, e, false);
    let result = s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 250, "y": 250, "swatch": 3 })).unwrap();
    assert_eq!(result["filled"].as_u64(), Some(1));
    assert_eq!(
        (colour_pixel(&s, e, 250, 250), colour_pixel(&s, e, 104, 250), colour_pixel(&s, e, 50, 250)),
        ((3, 65535), (3, 65535), (0, 0)),
        "inside, under the hard line to where it is whole, and not outside"
    );
    assert!(
        matches!(s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 50, "y": 250, "swatch": 3 })), Err(EngineError::Invalid(_))),
        "outside is open"
    );
}

#[test]
fn the_paint_bucket_closes_gaps_when_asked() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    draw_square(&mut s, e, true);
    // Open on the left for the whole height of the square: too wide to close.
    let fill = |gap: u32| json!({ "layer": e, "frame": 0, "x": 250, "y": 250, "swatch": 3, "gap": gap });
    assert!(matches!(s.execute("drawing.fill", fill(40)), Err(EngineError::Invalid(_))));
    // A line drawn most of the way up leaves a gap of some thirty pixels.
    let points = json!([{ "x": 100, "y": 400, "pressure": 1.0 }, { "x": 100, "y": 142, "pressure": 1.0 }]);
    let stroke = json!({ "brush": { "size": 12, "pressureSize": false }, "points": points });
    s.execute("drawing.stroke", json!({ "layer": e, "frame": 0, "swatch": 1, "stroke": stroke })).unwrap();
    assert!(matches!(s.execute("drawing.fill", fill(0)), Err(EngineError::Invalid(_))), "it runs out through the gap");
    s.execute("drawing.fill", fill(40)).unwrap();
    assert_eq!((colour_pixel(&s, e, 250, 250), colour_pixel(&s, e, 110, 390), colour_pixel(&s, e, 390, 110)), ((3, 65535), (3, 65535), (3, 65535)));
    assert_eq!(colour_pixel(&s, e, 60, 120), (0, 0), "and not out of it");
}

#[test]
fn a_stroke_changes_only_its_own_drawing() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20 }))).unwrap();
    s.execute("exposure.set", json!({ "layer": e, "frame": 0, "length": 2, "drawing": s.project().shots[0].layers[0].drawings[0].id.0 })).unwrap();
    s.execute("drawing.duplicate", json!({ "layer": e, "frame": 1 })).unwrap();
    let eraser = json!({ "size": 60, "erase": true, "pressureSize": false });
    s.execute("drawing.stroke", stroke_params(e, 1, eraser.clone())).unwrap();
    assert_eq!(s.history().undo_label(), Some("Eraser Stroke"));
    assert_eq!(line_pixel(&s, e, 1, 250, 100), Some((0, 0)), "erased on the copy");
    assert_eq!(line_pixel(&s, e, 0, 250, 100), Some((3, 65535)), "the original keeps its line");

    // Erasing where nothing is exposed does nothing, and makes no drawing.
    let steps = s.history().entries().len();
    assert_eq!(s.execute("drawing.stroke", stroke_params(e, 9, eraser)).unwrap()["drawing"], Value::Null);
    assert_eq!((s.history().entries().len(), cells(&s, e, 10).matches('-').count()), (steps, 8));
}

#[test]
fn a_stroke_is_plain_data_that_replays_to_the_same_pixels() {
    let run = || {
        let mut s = Session::new();
        let e = layer(&s);
        s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 14, "hardness": 0.5, "smoothing": { "amount": 40 } }))).unwrap();
        s.project().clone()
    };
    assert_eq!(run(), run());
}

#[test]
fn a_stroke_painted_already_is_taken_as_painted_when_it_is_the_same_stroke() {
    use iskra_drawing::{BrushSettings, IndexedSurface, LiveStroke, Rect, Stroke, StrokePoint, SwatchId};
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    // Painted as the viewport paints it, and sent as the viewport sends it: through JSON.
    let brush = BrushSettings { size: 20.0, hardness: 0.5, ..Default::default() };
    let points: Vec<StrokePoint> = (0..=30).map(|i| StrokePoint::new(100.0 + 10.0 * f64::from(i), 100.0, 0.7)).collect();
    let params = json!({ "layer": e, "frame": 0, "swatch": 3, "stroke": Stroke { brush: brush.clone(), points: points.clone() } });
    let mut live = LiveStroke::new(&IndexedSurface::new(), &brush, SwatchId(3));
    live.push(&points);
    let before = iskra_model::ArtContent::Indexed(live.before().clone());
    // A mark the stroke doesn't make tells whether what was offered was taken.
    let mut after = live.finish();
    after.paint(Rect::new(900, 900, 901, 901), &[1.0], SwatchId(7));
    let offer = crate::Painted {
        layer: e,
        frame: 0,
        art_layer: iskra_model::ArtLayerKind::LineArt,
        brush,
        points,
        swatch: SwatchId(3),
        protect: Vec::new(),
        mode: iskra_drawing::PaintMode::Over,
        before,
        after: iskra_model::ArtContent::Indexed(after),
    };
    s.offer_painted(offer.clone());
    s.execute("drawing.stroke", params.clone()).unwrap();
    assert_eq!(line_pixel(&s, e, 0, 900, 900), Some((7, 65535)), "taken");
    assert_eq!(line_pixel(&s, e, 0, 250, 100).map(|(swatch, _)| swatch), Some(3));

    // Of another swatch, or on pixels that are no longer as they were, the stroke is painted anew.
    s.undo();
    s.offer_painted(crate::Painted { swatch: SwatchId(4), ..offer.clone() });
    s.execute("drawing.stroke", params.clone()).unwrap();
    assert_eq!(line_pixel(&s, e, 0, 900, 900).map(|(_, coverage)| coverage), Some(0));
    s.offer_painted(offer);
    s.execute("drawing.stroke", params.clone()).unwrap();
    assert_eq!(line_pixel(&s, e, 0, 900, 900).map(|(_, coverage)| coverage), Some(0), "the first stroke is there already");
    // And what is offered is taken once, by the command after it.
    s.undo();
    s.execute("drawing.stroke", params).unwrap();
    assert_eq!(line_pixel(&s, e, 0, 900, 900).map(|(_, coverage)| coverage), Some(0));
}

#[test]
fn a_stroke_with_bad_input_is_refused() {
    let mut s = Session::new();
    let e = layer(&s);
    let mut unknown_swatch = stroke_params(e, 0, json!({}));
    unknown_swatch["swatch"] = json!(77);
    assert!(matches!(s.execute("drawing.stroke", unknown_swatch), Err(EngineError::Invalid(_))));
    let mut no_points = stroke_params(e, 0, json!({}));
    no_points["stroke"]["points"] = json!([]);
    assert!(matches!(s.execute("drawing.stroke", no_points), Err(EngineError::Invalid(_))));
    assert!(matches!(s.execute("drawing.stroke", json!({ "layer": e, "frame": 0 })), Err(EngineError::BadParams { .. })));
    assert!(!s.history().can_undo());
}

#[test]
fn exposures_can_be_lengthened_and_shortened_in_place() {
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 0 })).unwrap();
    s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 1 })).unwrap();
    s.execute("exposure.insert", json!({ "layer": e, "frame": 0, "length": 2 })).unwrap();
    assert_eq!(cells(&s, e, 5), "1 1 1 2 -", "drawing 1 is held two frames longer and drawing 2 moves along");
    s.execute("exposure.remove", json!({ "layer": e, "frame": 1 })).unwrap();
    assert_eq!(cells(&s, e, 5), "1 1 2 - -");
    assert_eq!(s.history().undo_label(), Some("Remove Exposure"));
    for (id, params) in
        [("exposure.insert", json!({ "layer": e, "frame": 0, "length": 0 })), ("exposure.set", json!({ "layer": e, "frame": 4_000_000_000u32 }))]
    {
        assert!(matches!(s.execute(id, params), Err(EngineError::Invalid(_))), "{id}");
    }
}

#[test]
fn layers_can_be_reordered() {
    let mut s = Session::new();
    let bottom = layer(&s);
    s.execute("layer.new", json!({ "name": "Middle" })).unwrap();
    s.execute("layer.new", json!({ "name": "Top" })).unwrap();
    // Which layer is over which is the order the composite stacks them in.
    let names = |s: &Session| {
        let shot = &s.project().shots[0];
        shot.stack().iter().map(|id| shot.layer(*id).unwrap().name.clone()).collect::<Vec<_>>().join(" ")
    };
    assert_eq!(names(&s), "Top Middle Drawing");
    s.execute("layer.move", json!({ "layer": bottom, "index": 0 })).unwrap();
    assert_eq!(names(&s), "Drawing Top Middle");
    s.execute("layer.move", json!({ "layer": bottom, "index": 2 })).unwrap();
    assert_eq!(names(&s), "Top Middle Drawing");
    assert!(s.execute("layer.move", json!({ "layer": bottom, "index": 3 })).is_err());
}

#[test]
fn a_project_is_saved_and_opened_again_with_its_drawings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Ball.iskra");
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    assert!(s.is_dirty() && s.path().is_none());
    assert!(matches!(s.execute("file.save", Value::Null), Err(EngineError::Invalid(_))), "no file yet");

    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20 }))).unwrap();
    s.execute("file.saveAs", json!({ "path": path })).unwrap();
    assert!(!s.is_dirty());
    assert_eq!(s.path(), Some(path.as_path()));
    assert!(s.history().can_undo(), "saving keeps the undo history");

    // A second stroke is one more tile-sized save; an unchanged project writes nothing.
    s.execute("drawing.stroke", stroke_params(e, 1, json!({ "size": 20 }))).unwrap();
    assert!(s.is_dirty());
    let written = s.execute("file.save", Value::Null).unwrap();
    assert!(written["tilesWritten"].as_u64().unwrap() >= 1 && written["structureWritten"] == json!(true));
    assert_eq!(s.execute("file.save", Value::Null).unwrap()["tilesWritten"], json!(0));
    let saved = s.project().clone();

    s.execute("file.new", Value::Null).unwrap();
    assert!(s.path().is_none() && !s.history().can_undo());
    assert_eq!(**s.project(), iskra_model::Project::fresh());
    {
        // A new project's shot: the peg that moves its camera, and under its layer the
        // storyboard's two, Character over Background, in the group for storyboard drawings,
        // wired in through the group's own composite.
        let shot = &s.project().shots[0];
        let names: Vec<&str> = shot.layers.iter().map(|layer| layer.name.as_str()).collect();
        assert_eq!((names, shot.stack().len(), shot.camera.parent.is_some()), (vec!["Drawing", "Background", "Character"], 3, true));
        assert!(
            !shot.is_board(shot.layers[0].id) && shot.is_board(shot.layers[1].id) && shot.is_board(shot.layers[2].id) && shot.stack()[0] == shot.layers[0].id
        );
        assert_eq!(shot.only(iskra_model::Work::Boards).stack(), [shot.layers[2].id, shot.layers[1].id]);
    }

    s.execute("file.open", json!({ "path": path })).unwrap();
    assert!(!s.project().shots[0].loaded, "opening reads the structure only");
    // Read to look at, as a card's picture is made, it has its pixels; the shot stays unloaded.
    let id = s.project().shots[0].id;
    let peeked = s.peeked_shot(id).unwrap().unwrap();
    assert_eq!(&peeked, &*saved.shots[0], "as it was saved, pixels and all");
    assert!(!s.project().shots[0].loaded);
    assert_eq!(s.execute("shot.load", Value::Null).unwrap()["skipped"], json!([]));
    assert_eq!(s.project(), &saved);
    assert!(!s.is_dirty() && !s.history().can_undo(), "loading is neither a change nor an undo step");
    assert_eq!(line_pixel(&s, e, 1, 250, 100), Some((3, 65535)));

    // Undoing back to the saved state is clean again only if it is that very snapshot.
    s.execute("drawing.stroke", stroke_params(e, 2, json!({ "size": 20 }))).unwrap();
    assert!(s.is_dirty());
    s.undo();
    assert!(!s.is_dirty());
}

#[test]
fn a_damaged_project_is_refused_and_can_be_recovered_from_its_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Ball.iskra");
    let mut s = Session::new();
    assert!(matches!(s.execute("file.open", json!({ "path": path })), Err(EngineError::Store(_))), "a missing file");

    let e = layer(&s);
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20 }))).unwrap();
    s.execute("file.saveAs", json!({ "path": path })).unwrap();
    let good = s.project().clone();
    assert!(dir.path().join("Ball.backup.iskra").exists(), "saving keeps a copy of what was saved");
    s.execute("file.new", Value::Null).unwrap();
    std::fs::write(&path, b"overwritten by something that is certainly not a project file").unwrap();

    let Err(EngineError::Store(iskra_store::StoreError::Damaged { backup, .. })) = s.execute("file.open", json!({ "path": path })) else { panic!("damaged") };
    assert!(backup.is_some(), "the error names the backup to offer");
    assert_eq!(**s.project(), iskra_model::Project::fresh(), "a failed open leaves the open project alone");

    let recovered = s.execute("file.recover", json!({ "path": path })).unwrap();
    assert_eq!(recovered["path"], json!(dir.path().join("Ball (recovered).iskra")));
    s.execute("shot.load", Value::Null).unwrap();
    assert_eq!(s.project(), &good);
    assert_eq!(s.path(), Some(dir.path().join("Ball (recovered).iskra").as_path()));
}

#[test]
fn changing_a_layers_resolution_resamples_its_drawings_in_place() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20 }))).unwrap();
    let bounds = |s: &Session| {
        let drawing = &s.project().shots[0].layers[0].drawings[0].drawing;
        drawing.bounds()
    };
    let before = bounds(&s);
    s.execute("layer.setDensity", json!({ "layer": e, "density": 4 })).unwrap();
    assert_eq!(s.project().shots[0].layers[0].density, 4.0);
    let after = bounds(&s);
    // From 2× to 4×: twice the pixels each way, so the drawing is the same size in the shot.
    assert!((after.width() as i32 - 2 * before.width() as i32).abs() <= 2, "{before:?} → {after:?}");
    assert!((after.x0 - 2 * before.x0).abs() <= 1 && (after.y0 - 2 * before.y0).abs() <= 1);
    assert_eq!(line_pixel(&s, e, 0, 500, 200), Some((3, 65535)), "the line is where it was, at twice the scale");
    s.undo();
    assert_eq!((s.project().shots[0].layers[0].density, bounds(&s)), (2.0, before));
    assert!(matches!(s.execute("layer.setDensity", json!({ "layer": e, "density": 100 })), Err(EngineError::Invalid(_))));

    // The project default only applies to layers made after it is set.
    s.execute("project.setDrawingDensity", json!({ "density": 1 })).unwrap();
    let new = id(s.execute("layer.new", Value::Null).unwrap());
    assert_eq!(s.project().layer(iskra_model::LayerId(new)).unwrap().density, 1.0);
    assert_eq!(s.project().layer(iskra_model::LayerId(e)).unwrap().density, 2.0);
}

#[test]
fn the_working_space_is_a_project_setting() {
    let mut s = Session::new();
    assert_eq!(s.execute("project.get", Value::Null).unwrap()["settings"]["workingSpace"], json!("srgb"));
    s.execute("project.setWorkingSpace", json!({ "space": "linear" })).unwrap();
    assert_eq!(s.project().settings.working_space, iskra_model::WorkingSpace::Linear);
    assert!(matches!(s.execute("project.setWorkingSpace", json!({ "space": "cmyk" })), Err(EngineError::BadParams { .. })));
    // A project saved before the setting existed opens as sRGB.
    let mut old = serde_json::to_value(&**s.project()).unwrap();
    old["settings"].as_object_mut().unwrap().remove("workingSpace");
    assert_eq!(serde_json::from_value::<iskra_model::Project>(old).unwrap().settings.working_space, iskra_model::WorkingSpace::Srgb);
}

#[test]
fn a_selection_can_be_moved_cleared_and_pasted_on_another_frame() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    // A 20 pixel thick line from x = 100 to 400 at y = 100.
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
    let left_half = json!([80, 80, 250, 120]);

    // Move the left half down by 200 pixels.
    let moved = s.execute("drawing.transform", json!({ "layer": e, "frame": 0, "rect": left_half, "matrix": [1, 0, 0, 1, 0, 200] })).unwrap();
    assert_eq!(line_pixel(&s, e, 0, 200, 100), Some((0, 0)), "gone from where it was");
    assert_eq!(line_pixel(&s, e, 0, 200, 300), Some((3, 65535)), "and now 200 lower");
    assert_eq!(line_pixel(&s, e, 0, 300, 100), Some((3, 65535)), "the right half stayed");
    let landed = moved["rect"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect::<Vec<_>>();
    assert!(landed[1] >= 280 && landed[3] <= 320 && landed[2] <= 250, "{landed:?}");
    assert_eq!(s.history().undo_label(), Some("Transform Selection"));

    // Copy the moved piece, paste it on an empty frame: a drawing is made for it.
    assert_eq!(s.execute("drawing.copy", json!({ "layer": e, "frame": 0, "rect": moved["rect"] })).unwrap()["copied"], json!(true));
    let steps = s.history().entries().len();
    let pasted = s.execute("drawing.paste", json!({ "layer": e, "frame": 5 })).unwrap();
    assert_eq!(pasted["rect"], moved["rect"], "pasted where it was copied from");
    assert_eq!(line_pixel(&s, e, 5, 200, 300), Some((3, 65535)));
    assert_eq!(s.history().entries().len(), steps + 1, "copying is not an undo step; pasting is one");

    // Cut takes it away and keeps it on the clipboard; clear only takes it away.
    s.execute("drawing.cut", json!({ "layer": e, "frame": 5, "rect": moved["rect"] })).unwrap();
    assert_eq!(line_pixel(&s, e, 5, 200, 300), Some((0, 0)));
    s.execute("drawing.clear", json!({ "layer": e, "frame": 0, "rect": [0, 0, 1000, 1000] })).unwrap();
    assert_eq!(line_pixel(&s, e, 0, 300, 100), Some((0, 0)));
    s.execute("drawing.paste", json!({ "layer": e, "frame": 0 })).unwrap();
    assert_eq!(line_pixel(&s, e, 0, 200, 300), Some((3, 65535)), "the cut piece is still on the clipboard");
}

#[test]
fn a_transform_on_every_drawing_moves_each_of_the_layers_drawings_once() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    // The same line on frame 0 and on frame 5, in two drawings, the first held on frame 9 too.
    for frame in [0, 5] {
        s.execute("drawing.stroke", stroke_params(e, frame, json!({ "size": 20, "pressureSize": false }))).unwrap();
    }
    let shown = |s: &Session, frame| s.project().layer(iskra_model::LayerId(e)).unwrap().exposure.at(frame);
    assert_ne!(shown(&s, 0), shown(&s, 5));
    let params = json!({ "layer": e, "frame": 0, "rect": [0, 0, 1000, 1000], "matrix": [1, 0, 0, 1, 0, 200], "everyDrawing": true });
    let steps = s.history().entries().len();
    s.execute("drawing.transform", params).unwrap();
    // Both moved down by 200, once each, in one step.
    for frame in [0, 5] {
        assert_eq!((line_pixel(&s, e, frame, 200, 100), line_pixel(&s, e, frame, 200, 300)), (Some((0, 0)), Some((3, 65535))), "frame {frame}");
    }
    assert_eq!((s.history().entries().len(), s.history().undo_label()), (steps + 1, Some("Transform Selection")));
}

#[test]
fn a_selection_is_of_one_art_layer_alone_and_leaves_protected_paint() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    // A black square with a red fill, and a blue line across the fill on the line art.
    draw_square(&mut s, e, false);
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 250, "y": 250, "swatch": 3 })).unwrap();
    let across =
        json!({ "brush": { "size": 12, "pressureSize": false }, "points": [{ "x": 150, "y": 250, "pressure": 1.0 }, { "x": 350, "y": 250, "pressure": 1.0 }] });
    s.execute("drawing.stroke", json!({ "layer": e, "frame": 0, "swatch": 5, "stroke": across })).unwrap();
    let line = |s: &Session, x, y| line_pixel(s, e, 0, x, y).map_or(0, |(swatch, _)| swatch);
    let fill = |s: &Session, x, y| colour_pixel(s, e, x, y).0;
    let all = json!([0, 0, 500, 500]);

    // The colour art alone is moved: the fill goes, and the lines stay where they are.
    s.execute("drawing.transform", json!({ "layer": e, "frame": 0, "rect": all, "matrix": [1, 0, 0, 1, 1000, 0], "artLayer": "colourArt" })).unwrap();
    assert_eq!((fill(&s, 250, 200), fill(&s, 1250, 200), line(&s, 100, 250), line(&s, 1100, 250)), (0, 3, 1, 0));
    s.undo();

    // With blue protected, everything else is moved and the blue line is left.
    s.execute("drawing.transform", json!({ "layer": e, "frame": 0, "rect": all, "matrix": [1, 0, 0, 1, 1000, 0], "protect": [5] })).unwrap();
    assert_eq!((line(&s, 250, 250), line(&s, 1250, 250), line(&s, 1100, 250), fill(&s, 1250, 200), line(&s, 100, 250)), (5, 0, 1, 3, 0));
    s.undo();

    // Cut and clear leave it too, and only the line art's black is on the clipboard and gone.
    s.execute("drawing.cut", json!({ "layer": e, "frame": 0, "rect": all, "artLayer": "lineArt", "protect": [5] })).unwrap();
    assert_eq!((line(&s, 100, 250), line(&s, 250, 250), fill(&s, 250, 200)), (0, 5, 3));
    s.execute("drawing.paste", json!({ "layer": e, "frame": 6 })).unwrap();
    assert_eq!((line_pixel(&s, e, 6, 100, 250).map(|(swatch, _)| swatch), line_pixel(&s, e, 6, 250, 250).map(|(swatch, _)| swatch)), (Some(1), Some(0)));
    // Pasted onto the colour art, the line art's black lands among the fills.
    s.execute("drawing.paste", json!({ "layer": e, "frame": 0, "artLayer": "colourArt" })).unwrap();
    assert_eq!((fill(&s, 100, 250), line(&s, 100, 250)), (1, 0));
    s.undo();
    s.execute("drawing.clear", json!({ "layer": e, "frame": 0, "rect": all, "protect": [5] })).unwrap();
    assert_eq!((line(&s, 250, 250), fill(&s, 250, 200)), (5, 0));
    // A lasso takes what is inside its outline, and a swatch what it painted: the left half
    // of the blue line is moved down by the one, and what is left of it up by the other.
    for _ in 0..3 {
        s.undo();
    }
    let left = json!([[0, 0], [250, 0], [250, 500], [0, 500]]);
    s.execute("drawing.transform", json!({ "layer": e, "frame": 0, "rect": all, "lasso": left, "swatch": 5, "matrix": [1, 0, 0, 1, 0, 100] })).unwrap();
    assert_eq!((line(&s, 200, 250), line(&s, 200, 350), line(&s, 300, 250), line(&s, 100, 250), fill(&s, 200, 200)), (0, 5, 5, 1, 3));
    let moved =
        s.execute("drawing.transform", json!({ "layer": e, "frame": 0, "rect": [0, 0, 500, 300], "swatch": 5, "matrix": [1, 0, 0, 1, 0, -100] })).unwrap();
    assert_eq!((line(&s, 300, 250), line(&s, 300, 150), line(&s, 200, 350), line(&s, 400, 250)), (0, 5, 5, 1));
    assert_eq!(moved["rect"], json!([250, 142, 358, 158]), "where what was taken landed");
    assert_eq!(s.execute("drawing.copy", json!({ "layer": e, "frame": 0, "rect": all, "swatch": 4 })).unwrap(), json!({ "copied": false }));
    // Outlines add and take away in their turn: the fill's left half without its top, which
    // is given blue, and nothing else is.
    let square = |x0: i32, y0: i32, x1: i32, y1: i32, subtract: bool| json!({ "lasso": [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], "subtract": subtract });
    let parts = json!([square(0, 0, 250, 500, false), square(0, 0, 500, 200, true)]);
    let done = s.execute("drawing.recolourSelection", json!({ "layer": e, "frame": 0, "rect": all, "parts": parts, "swatch": 3, "to": 5 })).unwrap();
    assert_eq!((done, s.history().undo_label()), (json!({ "artLayers": 1 }), Some("Recolour Selection")));
    assert_eq!((fill(&s, 200, 300), fill(&s, 200, 150), fill(&s, 300, 300), line(&s, 100, 300)), (5, 3, 3, 1));
    assert!(matches!(s.execute("drawing.recolourSelection", json!({ "layer": e, "frame": 0, "rect": all, "to": 77 })), Err(EngineError::Invalid(_))));
    for refused in [json!([[0, 0], [10, 10]]), json!([[0, 0], [5, 5], [10, 10]])] {
        let cleared = s.execute("drawing.clear", json!({ "layer": e, "frame": 0, "rect": all, "lasso": refused }));
        assert!(matches!(cleared, Err(EngineError::Invalid(_))));
    }
}

#[test]
fn selection_commands_refuse_bad_input() {
    let mut s = Session::new();
    let e = layer(&s);
    assert!(matches!(s.execute("drawing.paste", json!({ "layer": e, "frame": 0 })), Err(EngineError::Invalid(why)) if why.contains("nothing to paste")));
    assert!(s.execute("drawing.clear", json!({ "layer": e, "frame": 0, "rect": [0, 0, 10, 10] })).is_err(), "an empty cell");
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20 }))).unwrap();
    let steps = s.history().entries().len();
    for (id, params) in [
        ("drawing.clear", json!({ "layer": e, "frame": 0, "rect": [10, 10, 10, 20] })),
        ("drawing.transform", json!({ "layer": e, "frame": 0, "rect": [0, 0, 500, 500], "matrix": [0, 0, 0, 0, 0, 0] })),
    ] {
        assert!(s.execute(id, params).is_err(), "{id}");
    }
    assert_eq!(s.execute("drawing.copy", json!({ "layer": e, "frame": 0, "rect": [900, 900, 910, 910] })).unwrap()["copied"], json!(false), "nothing there");
    assert_eq!(s.history().entries().len(), steps);
}

/// The clips of the project's first storyboard as "name start-end", end exclusive.
fn cut(s: &Session) -> String {
    let project = s.project();
    let clip = |c: &iskra_model::Clip| format!("{} {}-{}", project.shot(c.shot).map_or("?", |s| s.name.as_str()), c.start, c.end());
    project.storyboards[0].clips().iter().map(clip).collect::<Vec<_>>().join(", ")
}

/// The id of the new project's one storyboard.
fn storyboard(s: &Session) -> u64 {
    s.project().storyboards[0].id.0
}

#[test]
fn shots_are_items_of_the_project_whether_or_not_a_storyboard_uses_them() {
    let mut s = Session::new();
    let board = storyboard(&s);
    assert_eq!(cut(&s), "Shot_01 0-48");

    // A shot made on a storyboard goes after the last one; one made without stays in the bin.
    let second = id(s.execute("shot.new", json!({ "storyboard": board })).unwrap());
    let loose = id(s.execute("shot.new", json!({ "name": "Titles", "frames": 10 })).unwrap());
    assert_eq!(cut(&s), "Shot_01 0-48, Shot_02 48-72", "one second long, named in sequence");
    assert_eq!(s.project().shots.len(), 3);
    assert!(s.project().shot(ShotId(second)).unwrap().layers.is_empty(), "a new shot has no layers yet");

    s.execute("storyboard.place", json!({ "storyboard": board, "shot": loose, "start": 100 })).unwrap();
    assert_eq!(cut(&s), "Shot_01 0-48, Shot_02 48-72, Titles 100-110", "placed freely: the gap plays as black");
    let other = id(s.execute("storyboard.new", Value::Null).unwrap());
    assert_eq!(s.project().storyboards[1].name, "Storyboard 2");
    let twice = s.execute("storyboard.place", json!({ "storyboard": other, "shot": loose }));
    assert!(matches!(twice, Err(EngineError::Invalid(why)) if why.contains("already on `Storyboard`")), "a shot has one home");

    // Off the storyboard it is still a shot; deleting a storyboard keeps its shots too.
    s.execute("storyboard.remove", json!({ "shot": loose })).unwrap();
    s.execute("storyboard.place", json!({ "storyboard": other, "shot": loose })).unwrap();
    s.execute("storyboard.delete", json!({ "storyboard": other })).unwrap();
    assert_eq!((cut(&s), s.project().shots.len()), ("Shot_01 0-48, Shot_02 48-72".to_string(), 3));
    assert!(s.execute("storyboard.remove", json!({ "shot": loose })).is_err(), "it is on no storyboard now");

    // Deleting a shot takes it off its storyboard.
    s.execute("shot.delete", json!({ "shot": second })).unwrap();
    assert_eq!((cut(&s), s.project().shots.len()), ("Shot_01 0-48".to_string(), 2));
    s.undo();
    assert_eq!(cut(&s), "Shot_01 0-48, Shot_02 48-72");
}

#[test]
fn shot_and_storyboard_names_are_unique_in_the_project() {
    let mut s = Session::new();
    let first = s.project().shots[0].id.0;
    let clash = |r: crate::Result<Value>| matches!(r, Err(EngineError::Invalid(why)) if why.contains("already has"));
    assert!(clash(s.execute("shot.new", json!({ "name": "Shot_01" }))));
    let second = id(s.execute("shot.new", Value::Null).unwrap());
    assert!(clash(s.execute("shot.rename", json!({ "shot": second, "name": "Shot_01" }))));
    s.execute("shot.rename", json!({ "shot": first, "name": "Opening" })).unwrap();
    s.execute("shot.rename", json!({ "shot": first, "name": "Opening" })).unwrap();
    assert_eq!(
        id(s.execute("shot.new", Value::Null).unwrap()),
        second + 5,
        "a failed rename took no id, nor did a rename; the second shot's composite, its camera, its camera's peg and its first revision took one each"
    );
    assert_eq!(s.project().shots.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Opening", "Shot_02", "Shot_01"], "the freed name is given out again");

    assert!(clash(s.execute("storyboard.new", json!({ "name": "Storyboard" }))));
    let other = id(s.execute("storyboard.new", json!({ "name": "Trailer" })).unwrap());
    assert!(clash(s.execute("storyboard.rename", json!({ "storyboard": other, "name": "Storyboard" }))));
    assert!(s.execute("storyboard.rename", json!({ "storyboard": 999, "name": "x" })).is_err());
}

#[test]
fn clips_move_freely_but_never_overlap_and_ripple_makes_room() {
    let mut s = Session::new();
    let board = storyboard(&s);
    let first = s.project().shots[0].id.0;
    let second = id(s.execute("shot.new", json!({ "storyboard": board })).unwrap());
    let third = id(s.execute("shot.new", json!({ "storyboard": board })).unwrap());
    assert_eq!(cut(&s), "Shot_01 0-48, Shot_02 48-72, Shot_03 72-96");

    let refused = s.execute("storyboard.moveClip", json!({ "shot": third, "start": 40 }));
    assert!(matches!(refused, Err(EngineError::Invalid(why)) if why == "`Shot_03` would run into `Shot_01`"), "it would start inside a clip");
    s.execute("storyboard.moveClip", json!({ "shot": third, "start": 200 })).unwrap();
    s.execute("storyboard.moveClip", json!({ "shot": second, "start": 60 })).unwrap();
    assert_eq!(cut(&s), "Shot_01 0-48, Shot_02 60-84, Shot_03 200-224");

    // A new shot between two others: refused where it doesn't fit, unless the later shots ripple.
    assert!(s.execute("shot.new", json!({ "storyboard": board, "start": 48, "frames": 20 })).is_err());
    assert_eq!(s.project().shots.len(), 3, "a refused command leaves nothing behind");
    s.execute("shot.new", json!({ "storyboard": board, "start": 48, "frames": 20, "ripple": true })).unwrap();
    assert_eq!(cut(&s), "Shot_01 0-48, Shot_04 48-68, Shot_02 68-92, Shot_03 208-232", "every later shot moves by the same eight frames");
    assert_eq!(s.history().undo_label(), Some("New Shot"));
    assert!(s.execute("storyboard.moveClip", json!({ "shot": first, "start": 4_000_000_000u32 })).is_err());
}

#[test]
fn in_and_out_points_pick_the_part_of_a_shot_that_is_in_the_cut() {
    let mut s = Session::new();
    let board = storyboard(&s);
    let first = s.project().shots[0].id.0;
    let second = id(s.execute("shot.new", json!({ "storyboard": board })).unwrap());
    let clip = |s: &Session, shot: u64| {
        let c = *s.project().storyboards[0].clip(ShotId(shot)).unwrap();
        (c.start, c.in_point, c.out_point)
    };

    // Trimming the end leaves a gap; nothing is deleted, so the shot keeps its two seconds.
    s.execute("storyboard.trimClip", json!({ "shot": first, "out": 36 })).unwrap();
    assert_eq!((cut(&s), s.project().shots[0].duration), ("Shot_01 0-36, Shot_02 48-72".to_string(), 48));
    // Trimming the start: what stays in the cut keeps its place on the storyboard.
    s.execute("storyboard.trimClip", json!({ "shot": first, "in": 12 })).unwrap();
    assert_eq!((clip(&s, first), cut(&s)), ((12, 12, 36), "Shot_01 12-36, Shot_02 48-72".to_string()));
    // Dragging the edge back out uncovers the handle frames, up to the next shot.
    s.execute("storyboard.trimClip", json!({ "shot": first, "out": 48 })).unwrap();
    assert_eq!(clip(&s, first), (12, 12, 48));

    // Past the shot's last frame the shot gets longer, if the cut has room or ripples.
    assert!(s.execute("storyboard.trimClip", json!({ "shot": first, "out": 60 })).is_err());
    assert_eq!(s.project().shots[0].duration, 48);
    s.execute("storyboard.trimClip", json!({ "shot": first, "out": 60, "ripple": true })).unwrap();
    assert_eq!((s.project().shots[0].duration, cut(&s)), (60, "Shot_01 12-60, Shot_02 60-84".to_string()));

    for params in [json!({ "shot": first, "in": 60 }), json!({ "shot": first, "in": 30, "out": 20 }), json!({ "shot": 999, "out": 5 })] {
        assert!(matches!(s.execute("storyboard.trimClip", params), Err(EngineError::Invalid(_))));
    }
    s.execute("storyboard.moveClip", json!({ "shot": first, "start": 0 })).unwrap();
    let before_zero = s.execute("storyboard.trimClip", json!({ "shot": first, "in": 0 }));
    assert!(matches!(before_zero, Err(EngineError::Invalid(why)) if why.contains("no room before")), "the head handle would start before frame 0");

    // A shot made shorter than its out point takes the clip back with it; longer adds handles.
    s.execute("shot.setDuration", json!({ "shot": second, "frames": 10 })).unwrap();
    assert_eq!(clip(&s, second), (60, 0, 10));
    s.execute("shot.setDuration", json!({ "shot": second, "frames": 100 })).unwrap();
    assert_eq!(clip(&s, second), (60, 0, 10));
    s.execute("shot.setDuration", json!({ "shot": first, "frames": 5 })).unwrap();
    assert_eq!(clip(&s, first), (0, 4, 5), "at least one frame stays in the cut");
}

#[test]
fn a_duplicated_shot_goes_after_the_original_where_there_is_room() {
    let mut s = Session::new();
    swatch_layer(&mut s);
    let first = s.project().shots[0].id.0;
    s.execute("drawing.stroke", stroke_params(layer(&s), 0, json!({ "size": 20 }))).unwrap();
    s.execute("storyboard.trimClip", json!({ "shot": first, "in": 8, "out": 32 })).unwrap();

    let copy = id(s.execute("shot.duplicate", json!({ "shot": first })).unwrap());
    assert_eq!(cut(&s), "Shot_01 8-32, Shot_02 32-56", "with the same in and out points");
    let copied = s.project().shot(ShotId(copy)).unwrap().layers[0].clone();
    assert_ne!(copied.id.0, layer(&s));
    assert_eq!(line_pixel(&s, copied.id.0, 0, 250, 100), Some((3, 65535)), "with its drawings");
    // Drawing on the copy leaves the original alone.
    s.execute("drawing.stroke", stroke_params(copied.id.0, 0, json!({ "size": 60, "erase": true, "pressureSize": false }))).unwrap();
    assert_eq!((line_pixel(&s, copied.id.0, 0, 250, 100), line_pixel(&s, layer(&s), 0, 250, 100)), (Some((0, 0)), Some((3, 65535))));

    // No room after the original: after the last shot, or right after it with ripple.
    s.execute("shot.duplicate", json!({ "shot": first })).unwrap();
    assert_eq!(cut(&s), "Shot_01 8-32, Shot_02 32-56, Shot_03 56-80");
    s.execute("shot.duplicate", json!({ "shot": first, "ripple": true, "name": "Again" })).unwrap();
    assert_eq!(cut(&s), "Shot_01 8-32, Again 32-56, Shot_02 56-80, Shot_03 80-104");
    // A shot that is on no storyboard is copied into the bin only.
    let loose = id(s.execute("shot.new", Value::Null).unwrap());
    s.execute("shot.duplicate", json!({ "shot": loose })).unwrap();
    assert_eq!((s.project().shots.len(), s.project().storyboards[0].clips().len()), (6, 4));
}

#[test]
fn a_transition_lies_across_a_cut_and_uses_the_handles() {
    let mut s = Session::new();
    let board = storyboard(&s);
    let first = s.project().shots[0].id.0;
    let second = id(s.execute("shot.new", json!({ "storyboard": board, "frames": 48 })).unwrap());
    let set = |s: &mut Session, shot: u64, edge: &str, length: u32| {
        s.execute("storyboard.setTransition", json!({ "shot": shot, "edge": edge, "kind": "crossDissolve", "length": length }))
    };
    let transitions = |s: &Session| s.project().storyboards[0].transitions.iter().map(|t| (t.shot.0, t.edge, t.length)).collect::<Vec<_>>();

    // Two whole shots have no frames beyond the cut to dissolve with.
    assert!(matches!(set(&mut s, first, "end", 12), Err(EngineError::Invalid(why)) if why.contains("no handle frames")));
    // Six frames of tail on the first shot and eight of head on the second.
    s.execute("storyboard.trimClip", json!({ "shot": first, "out": 42 })).unwrap();
    s.execute("storyboard.trimClip", json!({ "shot": second, "in": 8 })).unwrap();
    s.execute("storyboard.moveClip", json!({ "shot": second, "start": 42 })).unwrap();
    assert_eq!(cut(&s), "Shot_01 0-42, Shot_02 42-82");
    assert_eq!(set(&mut s, first, "end", 24).unwrap(), json!({ "length": 12 }), "shortened to the handles there are: six each side");
    // Asked for on the incoming shot's start, it is the same cut's transition.
    assert_eq!(set(&mut s, second, "start", 4).unwrap(), json!({ "length": 4 }));
    assert_eq!(transitions(&s), [(first, Edge::End, 4)]);

    // On a free edge it is a fade inside the clip, as long as the clip at most.
    assert_eq!(set(&mut s, first, "start", 12).unwrap(), json!({ "length": 12 }));
    assert_eq!(set(&mut s, second, "end", 500).unwrap(), json!({ "length": 40 }));
    assert_eq!(transitions(&s), [(first, Edge::End, 4), (first, Edge::Start, 12), (second, Edge::End, 40)]);

    s.execute("storyboard.removeTransition", json!({ "shot": second, "edge": "start" })).unwrap();
    assert!(s.execute("storyboard.removeTransition", json!({ "shot": second, "edge": "start" })).is_err(), "nothing there any more");
    assert!(set(&mut s, first, "start", 0).is_err());
    // A shot taken off the storyboard takes its transitions with it.
    s.execute("storyboard.remove", json!({ "shot": first })).unwrap();
    assert_eq!(transitions(&s), [(second, Edge::End, 40)]);
}

#[test]
fn a_layer_is_a_board_layer_or_an_animation_layer() {
    let mut s = Session::new();
    let board = id(s.execute("layer.new", json!({ "name": "Roughs", "role": "board" })).unwrap());
    let animation = id(s.execute("layer.new", Value::Null).unwrap());
    let role = |s: &Session, layer: u64| s.project().layer(iskra_model::LayerId(layer)).unwrap().role;
    assert_eq!((role(&s, board), role(&s, animation)), (Role::Board, Role::Animation));
    assert!(matches!(s.execute("layer.new", json!({ "role": "background" })), Err(EngineError::BadParams { .. })));
    assert_eq!(s.execute("project.get", Value::Null).unwrap()["shots"][0]["layers"][1]["role"], json!("board"));
}

/// A session on a saved film of two shots, each with a stroke on frame 0, opened from its
/// file: no shot is loaded. Returns the shots' ids and their layers' ids.
fn opened_film(dir: &std::path::Path) -> (Session, [u64; 2], [u64; 2]) {
    let mut s = Session::new();
    swatch_layer(&mut s);
    let first = s.project().shots[0].id.0;
    s.execute("drawing.stroke", stroke_params(layer(&s), 0, json!({ "size": 20 }))).unwrap();
    let second = id(s.execute("shot.duplicate", json!({ "shot": first })).unwrap());
    let layers = [layer(&s), s.project().shot(ShotId(second)).unwrap().layers[0].id.0];
    s.execute("file.saveAs", json!({ "path": dir.join("Film.iskra") })).unwrap();
    s.execute("file.open", json!({ "path": dir.join("Film.iskra") })).unwrap();
    (s, [first, second], layers)
}

fn is_loaded(s: &Session, shot: u64) -> bool {
    s.project().shot(ShotId(shot)).unwrap().loaded
}

#[test]
fn a_shot_is_loaded_to_work_on_it_and_released_after() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, [first, second], [e1, e2]) = opened_film(dir.path());
    assert!(!is_loaded(&s, first) && !is_loaded(&s, second));
    assert_eq!(line_pixel(&s, e2, 0, 250, 100), Some((0, 0)), "no pixels are in memory yet");

    // Drawing on a shot that is not loaded is refused, not painted on a blank.
    let refused = s.execute("drawing.stroke", stroke_params(e2, 1, json!({ "size": 20 })));
    assert!(matches!(refused, Err(EngineError::Invalid(why)) if why.contains("`Shot_02` are not loaded")));
    for (id, params) in [
        ("shot.delete", json!({ "shot": second })),
        ("shot.duplicate", json!({ "shot": second })),
        ("layer.delete", json!({ "layer": e2 })),
        ("drawing.copy", json!({ "layer": e2, "frame": 0, "rect": [0, 0, 500, 500] })),
    ] {
        assert!(s.execute(id, params).is_err(), "{id}");
    }
    // What needs no pixels works: the cut can be edited without loading a single shot.
    s.execute("storyboard.trimClip", json!({ "shot": second, "out": 24 })).unwrap();
    s.execute("shot.rename", json!({ "shot": second, "name": "Landing" })).unwrap();

    s.execute("shot.load", json!({ "shot": second })).unwrap();
    assert!(is_loaded(&s, second) && !is_loaded(&s, first), "only the shot asked for");
    assert_eq!(line_pixel(&s, e2, 0, 250, 100), Some((3, 65535)));
    assert_eq!(s.history().entries().len(), 3, "loading adds no undo step");
    s.execute("drawing.stroke", stroke_params(e2, 1, json!({ "size": 20 }))).unwrap();

    // With unsaved changes the shot stays in memory; once saved it is released.
    assert_eq!(s.execute("shot.unload", json!({ "shot": second })).unwrap(), json!({ "released": false }));
    assert_eq!(line_pixel(&s, e2, 1, 250, 100), Some((3, 65535)));
    s.execute("file.save", Value::Null).unwrap();
    assert_eq!(s.execute("shot.unload", json!({ "shot": second })).unwrap(), json!({ "released": true }));
    assert!(!is_loaded(&s, second) && !s.is_dirty(), "releasing is not a change");
    assert_eq!(s.execute("file.save", Value::Null).unwrap()["tilesWritten"], json!(0));

    // Loaded again, both strokes are there, and so is the first shot, which was never loaded.
    s.execute("shot.load", json!({ "shot": second })).unwrap();
    s.execute("shot.load", json!({ "shot": first })).unwrap();
    assert_eq!((line_pixel(&s, e2, 0, 250, 100), line_pixel(&s, e2, 1, 250, 100)), (Some((3, 65535)), Some((3, 65535))));
    assert_eq!(line_pixel(&s, e1, 0, 250, 100), Some((3, 65535)));
    assert!(s.execute("shot.load", json!({ "shot": 999 })).is_err());
}

#[test]
fn the_use_of_swatches_is_of_the_shots_that_are_not_open_too() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, [first, second], [e1, _]) = opened_film(dir.path());
    // Both shots are painted red; the first, opened, is made blue.
    s.execute("shot.load", json!({ "shot": first })).unwrap();
    s.execute("drawing.stroke", json!({ "layer": e1, "frame": 0, "swatch": 5, "stroke": { "brush": { "size": 60 }, "points": [{ "x": 0, "y": 100, "pressure": 1.0 }, { "x": 500, "y": 100, "pressure": 1.0 }] } })).unwrap();
    s.execute("drawing.clear", json!({ "layer": e1, "frame": 0, "rect": [0, 0, 1000, 1000], "protect": [5] })).unwrap();
    assert!(!is_loaded(&s, second));
    // Red is still used, by the shot that is not open, and nothing of it is loaded.
    let said = s.execute("palette.usage", Value::Null).unwrap();
    assert_eq!(said, json!({ "unused": [1, 2, 4], "missing": [], "unread": [] }));
    assert_eq!(s.execute("palette.removeUnused", json!({})).unwrap(), json!({ "removed": [1, 2, 4] }));
    assert!(s.project().palette.swatch(iskra_model::SwatchId(3)).is_some() && !is_loaded(&s, second));
}

#[test]
fn undo_still_works_across_a_shot_being_released() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, [_, second], [_, e2]) = opened_film(dir.path());
    s.execute("shot.load", json!({ "shot": second })).unwrap();
    let eraser = json!({ "size": 60, "erase": true, "pressureSize": false });
    s.execute("drawing.stroke", stroke_params(e2, 0, eraser)).unwrap();
    s.execute("file.save", Value::Null).unwrap();
    assert_eq!(s.execute("shot.unload", json!({ "shot": second })).unwrap(), json!({ "released": true }));

    // The state before the eraser differs from the file, so it kept its pixels.
    s.undo();
    assert!(is_loaded(&s, second) && s.is_dirty());
    assert_eq!(line_pixel(&s, e2, 0, 250, 100), Some((3, 65535)), "the line is back");
    // Saving it writes the line back into the file. The released state stood for the file as
    // it was, so it is given its pixels back before the file changes under it.
    assert!(s.execute("file.save", Value::Null).unwrap()["tilesWritten"].as_u64().unwrap() >= 1);
    s.redo();
    assert!(is_loaded(&s, second) && s.is_dirty());
    assert_eq!(line_pixel(&s, e2, 0, 250, 100), Some((0, 0)), "redo is the erased drawing, not what the file has now");
    s.execute("file.save", Value::Null).unwrap();

    let mut other = Session::new();
    other.execute("file.open", json!({ "path": dir.path().join("Film.iskra") })).unwrap();
    other.execute("shot.load", json!({ "shot": second })).unwrap();
    assert_eq!(other.project().shot(ShotId(second)), s.project().shot(ShotId(second)));
}

#[test]
fn deleting_a_shot_keeps_its_drawings_for_undo_even_after_the_save() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, [_, second], [_, e2]) = opened_film(dir.path());
    // Renamed while not loaded: the state before the rename has the shot not loaded either.
    s.execute("shot.rename", json!({ "shot": second, "name": "Landing" })).unwrap();
    s.execute("shot.load", json!({ "shot": second })).unwrap();
    s.execute("file.save", Value::Null).unwrap();
    s.execute("shot.unload", json!({ "shot": second })).unwrap();
    assert!(!is_loaded(&s, second));
    s.execute("shot.load", json!({ "shot": second })).unwrap();
    s.execute("shot.delete", json!({ "shot": second })).unwrap();
    // The undone states of the shot are released here, while the file still has it…
    assert!(!s.unload_shot(ShotId(second)), "the current state has no such shot");
    s.undo();
    assert!(!is_loaded(&s, second));
    s.redo();
    s.execute("file.save", Value::Null).unwrap();
    // …but the save that deletes its tiles leaves every state with its pixels.
    s.undo();
    assert_eq!(line_pixel(&s, e2, 0, 250, 100), Some((3, 65535)));
    s.undo();
    assert_eq!(line_pixel(&s, e2, 0, 250, 100), Some((3, 65535)), "also the state from before it was first loaded");
}

#[test]
fn a_released_shot_loads_as_the_file_has_it() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, [_, second], [_, e2]) = opened_film(dir.path());
    s.execute("shot.load", json!({ "shot": second })).unwrap();
    s.execute("file.save", Value::Null).unwrap();
    s.execute("shot.unload", json!({ "shot": second })).unwrap();
    let mut other = Session::new();
    other.execute("file.open", json!({ "path": dir.path().join("Film.iskra") })).unwrap();
    other.execute("shot.load", json!({ "shot": second })).unwrap();
    s.execute("shot.load", json!({ "shot": second })).unwrap();
    assert_eq!(s.project().shot(ShotId(second)), other.project().shot(ShotId(second)));
    assert_eq!(line_pixel(&s, e2, 0, 250, 100), Some((3, 65535)));
}

#[test]
fn pictures_and_sound_are_brought_in_as_items_that_refer_to_their_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    s.execute("file.saveAs", json!({ "path": dir.path().join("Ball.iskra"), "projectFolder": true })).unwrap();
    let (images, audio, elsewhere) = (dir.path().join("Ball/Images"), dir.path().join("Ball/Audio"), dir.path().join("elsewhere"));
    std::fs::create_dir_all(&elsewhere).unwrap();
    let png = |path: &std::path::Path, width: u32| image::RgbaImage::from_pixel(width, 18, image::Rgba([40, 90, 200, 255])).save(path).unwrap();
    png(&images.join("sky.png"), 32);
    for frame in 1..=4 {
        png(&elsewhere.join(format!("walk_{frame:03}.png")), 16);
    }
    std::fs::write(audio.join("line.wav"), iskra_audio::write_wav16(&vec![0.1; 96_000], 1, 48_000)).unwrap();
    std::fs::write(elsewhere.join("notes.txt"), "not media").unwrap();

    // Each file is an item; what can't be read is said, and the rest comes in.
    let paths = json!([images.join("sky.png"), audio.join("line.wav"), elsewhere.join("notes.txt"), elsewhere.join("walk_001.png")]);
    let made = s.execute("media.import", json!({ "paths": paths, "sequence": true })).unwrap();
    assert_eq!((made["media"].as_array().unwrap().len(), made["problems"].as_array().unwrap().len()), (3, 1));
    assert_eq!(s.history().undo_label(), Some("Import Media"));
    let media = |s: &Session| s.project().media.iter().map(|media| (media.name.clone(), media.kind.clone(), media.path.clone())).collect::<Vec<_>>();
    let sound = MediaKind::Sound { sample_rate: 48_000, channels: 1, frames: 96_000 };
    assert_eq!(
        media(&s),
        [
            ("sky".to_string(), MediaKind::Image { width: 32, height: 18 }, PathBuf::from("Images/sky.png")),
            ("line".to_string(), sound.clone(), PathBuf::from("Audio/line.wav")),
            ("walk_001".to_string(), MediaKind::ImageSequence { width: 16, height: 18, frames: 4 }, elsewhere.join("walk_001.png")),
        ],
        "a file in the project's folder is named from it; one outside, whole"
    );
    assert_eq!(sound.length(s.project().settings.frame_rate), 48, "two seconds of sound at 24 frames a second");
    // Without `sequence`, a numbered picture is one picture, and a name that is taken gets a number.
    s.execute("media.import", json!({ "paths": [elsewhere.join("walk_001.png")] })).unwrap();
    assert_eq!(media(&s)[3].0, "walk_001 2");
    assert!(matches!(media(&s)[3].1, MediaKind::Image { .. }));
    assert!(turned_down(s.execute("media.import", json!({ "paths": [elsewhere.join("notes.txt")] })), "not a picture or a sound"));

    // Items are in bins, renamed, pointed at another file of their kind, and deleted.
    let ids: Vec<u64> = s.project().media.iter().map(|media| media.id.0).collect();
    let bin = s.execute("bin.new", json!({ "name": "Sound" })).unwrap()["id"].as_u64().unwrap();
    s.execute("item.setBin", json!({ "items": [{ "media": ids[1] }], "bin": bin })).unwrap();
    assert_eq!(s.project().items_in(Some(BinId(bin))), [Item::Media(MediaId(ids[1]))]);
    s.execute("media.rename", json!({ "media": ids[1], "name": "Line 1" })).unwrap();
    assert!(turned_down(s.execute("media.rename", json!({ "media": ids[0], "name": "Line 1" })), "already"));
    png(&elsewhere.join("sky wide.png"), 64);
    s.execute("media.relink", json!({ "media": ids[0], "path": elsewhere.join("sky wide.png") })).unwrap();
    assert_eq!(media(&s)[0].1, MediaKind::Image { width: 64, height: 18 });
    assert!(turned_down(s.execute("media.relink", json!({ "media": ids[0], "path": audio.join("line.wav") })), "is sound, and the item is image"));
    s.execute("media.delete", json!({ "media": ids[3] })).unwrap();
    assert_eq!(s.project().media.len(), 3);
    assert!(elsewhere.join("walk_001.png").is_file(), "the file is left as it is");

    // Saved and opened again, the items are as they were.
    let before = s.project().media.clone();
    s.execute("file.save", Value::Null).unwrap();
    let mut again = Session::new();
    again.execute("file.open", json!({ "path": dir.path().join("Ball/Ball.iskra") })).unwrap();
    assert_eq!(again.project().media, before);
    assert_eq!(again.project().media[1].file(again.path().and_then(|file| file.parent())), audio.join("line.wav"));
}

#[test]
fn a_folder_of_the_same_file_names_is_a_skin_of_a_layer_of_pictures() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    s.execute("project.setResolution", json!({ "width": 64, "height": 36 })).unwrap();
    let png = |path: std::path::PathBuf, colour: [u8; 4]| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbaImage::from_pixel(10, 20, image::Rgba(colour)).save(&path).unwrap();
        path
    };
    // A sequence of three days, and a night folder with the first and the third.
    let day = dir.path().join("day");
    let first = png(day.join("walk_1.png"), [200, 40, 40, 255]);
    png(day.join("walk_2.png"), [200, 40, 40, 255]);
    png(day.join("walk_3.png"), [200, 40, 40, 255]);
    let night = dir.path().join("night");
    png(night.join("walk_1.png"), [20, 20, 90, 255]);
    png(night.join("walk_3.png"), [20, 20, 90, 255]);
    let made = s.execute("media.import", json!({ "paths": [first], "sequence": true })).unwrap();
    let media = made["media"][0].as_u64().unwrap();
    let layer = s.execute("media.place", json!({ "media": media })).unwrap()["id"].as_u64().unwrap();
    let skin = s.execute("skin.addVariant", json!({ "layer": layer, "drawing": 1, "skin": 1 }));
    assert!(skin.is_err(), "no skin yet");

    // A skin called Night is the folder `night`'s.
    s.execute("skin.add", json!({ "name": "Night" })).unwrap();
    let said = s.execute("skin.fromFolder", json!({ "layer": layer, "folder": night })).unwrap();
    assert_eq!((said["made"].as_u64(), said["lacks"].clone()), (Some(2), json!(["walk_2.png"])));
    assert_eq!(s.project().skins().len(), 1);
    let placed = s.project().layer(iskra_model::LayerId(layer)).unwrap().clone();
    let skin = s.project().skin_named("Night").unwrap().id;
    let of = |name: &str| placed.drawings.iter().find(|drawing| drawing.name == name).unwrap().id;
    assert!(placed.variant(skin, of("1")).is_some() && placed.variant(skin, of("2")).is_none() && placed.variant(skin, of("3")).is_some());
    // Renamed, a drawing keeps its variant: they are matched by id after.
    let first = of("1");
    s.execute("drawing.rename", json!({ "layer": layer, "drawing": first.0, "name": "first" })).unwrap();
    let placed = s.project().layer(iskra_model::LayerId(layer)).unwrap().clone();
    assert!(placed.drawing(first).is_some_and(|day| day.name == "first") && placed.variant(skin, first).is_some());
    assert!(turned_down(s.execute("skin.fromFolder", json!({ "layer": layer, "folder": dir.path().join("none") })), "no folder"));
}

#[test]
fn a_picture_or_a_sequence_is_placed_into_a_shot_as_a_layer_of_its_own_pixels() {
    use iskra_model::LayerId;
    fn shot(s: &Session) -> &iskra_model::Shot {
        &s.project().shots[0]
    }
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    s.execute("project.setResolution", json!({ "width": 64, "height": 36 })).unwrap();
    s.execute("shot.setDuration", json!({ "frames": 6 })).unwrap();
    let png = |name: &str, width: u32, colour: [u8; 4]| {
        let path = dir.path().join(name);
        image::RgbaImage::from_pixel(width, 20, image::Rgba(colour)).save(&path).unwrap();
        path
    };
    let (blue, red, green) = ([40, 90, 200, 255], [200, 40, 40, 255], [40, 160, 60, 255]);
    let sky = png("sky.png", 40, blue);
    let first = png("blink_1.png", 10, red);
    png("blink_3.png", 10, green);
    std::fs::write(dir.path().join("line.wav"), iskra_audio::write_wav16(&[0.0; 800], 1, 8000)).unwrap();
    let made = s.execute("media.import", json!({ "paths": [sky, first, dir.path().join("line.wav")], "sequence": true })).unwrap();
    let ids: Vec<u64> = made["media"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();

    // A picture is a layer on top, held for the shot, a pixel of it a pixel of the shot, in the middle.
    let layer = s.execute("media.place", json!({ "media": ids[0] })).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.history().undo_label(), Some("Place Media"));
    let placed = shot(&s).layer(LayerId(layer)).unwrap().clone();
    assert_eq!((placed.name.as_str(), placed.density, placed.media, placed.drawings.len()), ("sky", 1.0, Some(MediaId(ids[0])), 1));
    assert_eq!(placed.exposure.runs().iter().map(|run| (run.start, run.len)).collect::<Vec<_>>(), [(0, 6)]);
    let out = dir.path().join("frame.png");
    let look = |s: &mut Session, frame: u32| {
        s.execute("shot.renderFrame", json!({ "path": out, "frame": frame })).unwrap();
        image::open(&out).unwrap().into_rgba8()
    };
    let picture = look(&mut s, 0);
    assert_eq!(
        (picture.get_pixel(32, 18).0, picture.get_pixel(13, 9).0, picture.get_pixel(11, 18).0, picture.get_pixel(32, 7).0),
        (blue, blue, [0; 4], [0; 4]),
        "40 by 20 in the middle of 64 by 36"
    );

    // A sequence shows a frame a file from the frame it is placed on; a number without a file is an empty cell.
    let blink = s.execute("media.place", json!({ "media": ids[1], "frame": 2 })).unwrap()["id"].as_u64().unwrap();
    let cells: Vec<Option<String>> = (0..6).map(|frame| shot(&s).layer(LayerId(blink)).unwrap().drawing_at(frame).map(|d| d.name.clone())).collect();
    assert_eq!(cells, [None, None, Some("1".to_string()), None, Some("3".to_string()), None]);
    assert_eq!((look(&mut s, 2).get_pixel(32, 18).0, look(&mut s, 3).get_pixel(32, 18).0, look(&mut s, 4).get_pixel(32, 18).0), (red, blue, green));
    assert!(turned_down(s.execute("media.place", json!({ "media": ids[2] })), "is a sound"));

    // The pixels are the project's: saved and opened again, the layers are as they were, without the files.
    let file = dir.path().join("Film.iskra");
    s.execute("file.saveAs", json!({ "path": file })).unwrap();
    let before = shot(&s).clone();
    std::fs::remove_file(dir.path().join("blink_3.png")).unwrap();
    let mut again = Session::new();
    again.execute("file.open", json!({ "path": file })).unwrap();
    again.execute("shot.load", json!({})).unwrap();
    assert_eq!(shot(&again).layers, before.layers);

    // The file painted over, the layer reads it again; what was drawn over the picture, on its
    // line art and overart of plain colour, stays. The picture itself is not painted on.
    s.execute("drawing.stroke", stroke_params(layer, 0, json!({ "size": 6.0 }))).unwrap();
    let mut over = stroke_params(layer, 0, json!({ "size": 6.0 }));
    over["artLayer"] = json!("overart");
    s.execute("drawing.stroke", over.clone()).unwrap();
    over["artLayer"] = json!("colourArt");
    assert!(turned_down(s.execute("drawing.stroke", over), "a picture that was brought in"));
    png("sky.png", 40, green);
    s.execute("layer.refreshMedia", json!({ "layer": layer })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Read Pictures Again"));
    let drawing = &shot(&s).layer(LayerId(layer)).unwrap().drawings[0].drawing;
    assert_eq!(drawing.to_rgba8(iskra_model::Rect::new(-19, -9, -18, -8), |_| [0; 4]), [green]);
    for kind in [iskra_model::ArtLayerKind::LineArt, iskra_model::ArtLayerKind::Overart] {
        let art = &drawing.layer(kind).unwrap().content;
        assert!(art.is_rgba() && !art.bounds().is_empty(), "the stroke over it is kept, in plain colour: {kind:?}");
    }
    // A frame whose file is gone keeps its picture; a layer that was drawn has nothing to read.
    s.execute("layer.refreshMedia", json!({ "layer": blink })).unwrap();
    assert_eq!(shot(&s).layer(LayerId(blink)).unwrap().drawings.len(), 2);
    let drawn = shot(&s).layers.last().unwrap().id.0;
    assert!(turned_down(s.execute("layer.refreshMedia", json!({ "layer": drawn })), "not placed from a media item"));
}

#[test]
fn sound_is_laid_on_a_storyboards_tracks_moved_trimmed_and_taken_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    // Two seconds and one second of sound, and a picture.
    for (name, seconds) in [("line.wav", 2), ("door.wav", 1)] {
        std::fs::write(dir.path().join(name), iskra_audio::write_wav16(&vec![0.2; 8000 * seconds], 1, 8000)).unwrap();
    }
    image::RgbaImage::new(4, 4).save(dir.path().join("sky.png")).unwrap();
    let paths = json!([dir.path().join("line.wav"), dir.path().join("door.wav"), dir.path().join("sky.png")]);
    let made = s.execute("media.import", json!({ "paths": paths })).unwrap();
    let ids: Vec<u64> = made["media"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    let tracks = |s: &Session| {
        let clips = |track: &iskra_model::SoundTrack| track.clips.iter().map(|clip| (clip.start, clip.offset, clip.length)).collect::<Vec<_>>();
        s.project().storyboards[0].sound.iter().map(clips).collect::<Vec<_>>()
    };

    // A sound is a clip of all of it; one that would run into it goes a track down.
    let line = s.execute("sound.place", json!({ "media": ids[0], "start": 12 })).unwrap();
    assert_eq!((line["track"].as_u64(), s.history().undo_label()), (Some(0), Some("Place Sound")));
    let door = s.execute("sound.place", json!({ "media": ids[1], "start": 40 })).unwrap();
    assert_eq!((door["track"].as_u64(), tracks(&s)), (Some(1), vec![vec![(12, 0, 48)], vec![(40, 0, 24)]]));
    assert!(turned_down(s.execute("sound.place", json!({ "media": ids[2] })), "is image, not a sound"));
    let (line, door) = (line["id"].as_u64().unwrap(), door["id"].as_u64().unwrap());

    // Moved to where the first track has room, it lies there.
    let moved = s.execute("sound.setClip", json!({ "clip": door, "start": 60, "track": 0 })).unwrap();
    assert_eq!((moved["track"].as_u64(), s.history().undo_label(), tracks(&s)), (Some(0), Some("Move Sound"), vec![vec![(12, 0, 48), (60, 0, 24)], vec![]]));
    // Trimmed at its start and its end; never past its sound.
    s.execute("sound.setClip", json!({ "clip": line, "start": 20, "offset": 8, "length": 30 })).unwrap();
    assert_eq!((s.history().undo_label(), tracks(&s)[0][0]), (Some("Trim Sound"), (20, 8, 30)));
    assert!(turned_down(s.execute("sound.setClip", json!({ "clip": line, "length": 41 })), "outside its sound"));
    assert!(turned_down(s.execute("sound.setClip", json!({ "clip": line, "length": 0 })), "outside its sound"));
    s.execute("sound.setClip", json!({ "clip": line, "gain": 0.5 })).unwrap();
    assert_eq!(s.project().storyboards[0].sound_clip(iskra_model::SoundClipId(line)).unwrap().1.gain, 0.5);
    assert!(turned_down(s.execute("sound.setClip", json!({ "clip": line, "gain": 20.0 })), "gain is outside"));
    // The master volume is the project's, and a film rendered with its sound is another with it.
    let film = iskra_model::Rendered::Film(s.project().storyboards[0].id);
    let mark = |s: &Session| s.project().render_mark(film, (0, 0), true).unwrap().sound;
    let before = mark(&s);
    s.execute("project.setVolume", json!({ "volume": 0.5 })).unwrap();
    assert!(s.project().settings.volume == 0.5 && mark(&s) != before && s.history().undo_label() == Some("Set Master Volume"));
    s.execute("project.setVolume", json!({ "volume": 1.0 })).unwrap();
    assert_eq!(mark(&s), before);
    assert!(turned_down(s.execute("project.setVolume", json!({ "volume": -1.0 })), "volume is outside"));

    // Tracks are named and muted; a track that goes takes its clips.
    s.execute("sound.setTrack", json!({ "track": 0, "name": "Dialogue", "mute": true })).unwrap();
    let first = &s.project().storyboards[0].sound[0];
    assert_eq!((first.name.as_str(), first.mute), ("Dialogue", true));
    s.execute("sound.setTrack", json!({ "track": 1, "remove": true })).unwrap();
    assert!(turned_down(s.execute("sound.setTrack", json!({ "track": 1, "mute": true })), "no sound track 1"));

    // A shot that is sent out takes the stretch under it: the shot is the film's frames 0 to
    // 48, the first clip lies from 20 to 50, so 28 frames of it go, from its start, and the
    // second clip, which starts at 60, stays.
    let shot_id = s.project().shots[0].id.0;
    s.execute("sound.setTrack", json!({ "track": 0, "mute": false })).unwrap();
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    s.execute("shot.sendOut", json!({ "shots": [shot_id], "folder": dir.path().join("out"), "to": "Ana" })).unwrap();
    let (_, sent, _) = iskra_store::read_shot_file(&dir.path().join("out").join("Shot_01.iskra")).unwrap();
    let theirs = &sent.storyboards[0].sound;
    assert_eq!((theirs.len(), theirs[0].name.as_str()), (1, "Dialogue"));
    assert_eq!(theirs[0].clips.iter().map(|clip| (clip.start, clip.offset, clip.length, clip.gain)).collect::<Vec<_>>(), [(20, 8, 28, 0.5)]);
    assert_eq!(sent.media.iter().map(|media| (media.name.as_str(), media.path.clone())).collect::<Vec<_>>(), [("line", dir.path().join("line.wav"))]);

    // Saved and opened again, the sound is where it was.
    let file = dir.path().join("Film.iskra");
    s.execute("file.saveAs", json!({ "path": file })).unwrap();
    let mut again = Session::new();
    again.execute("file.open", json!({ "path": file })).unwrap();
    assert_eq!(again.project().storyboards[0].sound, s.project().storyboards[0].sound);

    // A clip is taken off; a sound item that goes takes its clips with it.
    s.execute("sound.delete", json!({ "clip": door })).unwrap();
    assert!(turned_down(s.execute("sound.delete", json!({ "clip": door })), "no sound clip"));
    s.execute("media.delete", json!({ "media": ids[0] })).unwrap();
    assert_eq!(tracks(&s), [vec![]]);
}

#[test]
fn what_is_spoken_under_the_shots_is_written_into_their_panels_dialogue() {
    let dir = tempfile::tempdir().unwrap();
    // Three seconds of sound, and a picture, which has nothing to say.
    let line = dir.path().join("line.wav");
    std::fs::write(&line, iskra_audio::write_wav16(&vec![0.25; 3 * 8000], 1, 8000)).unwrap();
    let still = dir.path().join("still.png");
    std::fs::write(&still, iskra_io::encode_png(2, 2, &[[0, 0, 0, 255]; 4]).unwrap()).unwrap();
    let mut s = Session::new();
    let made = s.execute("media.import", json!({ "paths": [line, still] })).unwrap();
    let (sound, picture) = (made["media"][0].as_u64().unwrap(), made["media"][1].as_u64().unwrap());
    let board = s.project().storyboards[0].id.0;
    let shot = s.project().shots[0].id.0;
    // The shot is two seconds, with a panel from its frame 24 on; the sound lies under it
    // from the film's frame 12, starting half a second into itself.
    s.execute("panel.add", json!({ "frame": 24, "dialogue": "(as written)" })).unwrap();
    let clip = id(s.execute("sound.place", json!({ "storyboard": board, "media": sound, "start": 12 })).unwrap());
    s.execute("sound.setClip", json!({ "clip": clip, "offset": 12, "length": 30 })).unwrap();
    assert!(turned_down(s.execute("storyboard.fillDialogue", json!({ "storyboard": board })), "nothing is spoken"));

    // A transcript is given as words with their seconds, or read from a subtitle file.
    assert!(turned_down(s.execute("media.setTranscript", json!({ "media": picture, "words": [] })), "not a sound"));
    assert!(turned_down(s.execute("media.setTranscript", json!({ "media": sound })), "as `path`"));
    let words = json!([
        { "text": "Early", "start": 0.25, "end": 0.4 },
        { "text": "Hello?", "start": 0.5, "end": 0.9 },
        { "text": "Is", "start": 0.75 },
        { "text": "anyone", "start": 1.0 },
        { "text": "there?", "start": 1.5 },
        { "text": "Late", "start": 2.5 },
    ]);
    assert_eq!(s.execute("media.setTranscript", json!({ "media": sound, "words": words })).unwrap(), json!({ "words": 6 }));
    assert_eq!(s.history().undo_label(), Some("Set Transcript"));
    // The clip plays its sound from 0.5 s to 1.75 s, over the film's frames 12 to 42: `Hello?`
    // and `Is` start in the first panel, `anyone` on the film's frame 24, where the second
    // panel starts, and `there?` on 36. What the clip does not play is not heard.
    let dialogue = |s: &Session| s.project().shots[0].panels().iter().map(|panel| panel.dialogue.clone()).collect::<Vec<_>>();
    assert_eq!(s.execute("storyboard.fillDialogue", json!({ "storyboard": board })).unwrap(), json!({ "filled": 1, "kept": 1 }));
    assert_eq!(dialogue(&s), ["Hello? Is", "(as written)"], "what was written by hand is kept");
    assert_eq!(s.execute("storyboard.fillDialogue", json!({ "storyboard": board, "overwrite": true })).unwrap(), json!({ "filled": 1, "kept": 0 }));
    assert_eq!((dialogue(&s), s.history().undo_label()), (vec!["Hello? Is".to_string(), "anyone there?".to_string()], Some("Fill Dialogue from Transcripts")));
    s.undo();
    assert_eq!(dialogue(&s)[1], "(as written)");
    // A muted track says nothing.
    s.execute("sound.setTrack", json!({ "track": 0, "mute": true })).unwrap();
    assert!(turned_down(s.execute("storyboard.fillDialogue", json!({ "storyboard": board })), "nothing is spoken"));
    s.execute("sound.setTrack", json!({ "track": 0, "mute": false })).unwrap();

    // From a subtitle file, a cue is a few words together; and the transcript is the project's.
    let subtitles = dir.path().join("line.srt");
    std::fs::write(&subtitles, "1\n00:00:01,000 --> 00:00:01,400\nWho is it?\n").unwrap();
    assert_eq!(s.execute("media.setTranscript", json!({ "media": sound, "path": subtitles })).unwrap(), json!({ "words": 1 }));
    s.execute("storyboard.fillDialogue", json!({ "storyboard": board, "overwrite": true })).unwrap();
    assert_eq!(dialogue(&s), ["Hello? Is", "Who is it?"], "a panel that nothing is spoken over any more is left as it is");
    let file = dir.path().join("Film.iskra");
    s.execute("file.saveAs", json!({ "path": file })).unwrap();
    let mut again = Session::new();
    again.execute("file.open", json!({ "path": file })).unwrap();
    assert_eq!(again.project().media[0].transcript, s.project().media[0].transcript);
    let _ = shot;
}

#[test]
fn boards_are_in_a_group_of_the_shots_own_for_its_storyboard_drawings() {
    let mut s = Session::new();
    let groups = |s: &Session, shot: usize| {
        let shot = &s.project().shots[shot];
        shot.groups.iter().map(|group| (group.name.clone(), group.board, group.members.len())).collect::<Vec<_>>()
    };
    // An animation layer is in no group; the first board makes the group, the next joins it.
    s.execute("layer.new", json!({ "name": "Clean" })).unwrap();
    assert_eq!(groups(&s, 0), []);
    let roughs = id(s.execute("layer.new", json!({ "name": "Roughs", "role": "board" })).unwrap());
    let background = id(s.execute("layer.new", json!({ "name": "Background", "role": "board" })).unwrap());
    // With the two boards, each with its peg above it, and the group's own composite: they are
    // wired into it, the newest on top, and it is wired in where they went, so the shot is
    // stacked as it was.
    assert_eq!(groups(&s, 0), [("Storyboard".to_string(), true, 5)]);
    {
        let shot = &s.project().shots[0];
        let peg = shot.layer(iskra_model::LayerId(roughs)).and_then(|layer| layer.parent).and_then(|peg| shot.peg(peg)).expect("a board has a peg");
        assert_eq!((peg.name.as_str(), peg.parent, shot.group_of(peg.id.0).is_some_and(|group| group.board)), ("Roughs-P", None, true));
    }
    assert_eq!(s.project().shots[0].group_of(roughs).map(|group| group.name.as_str()), Some("Storyboard"));
    {
        use iskra_model::{LayerId, Source};
        let shot = &s.project().shots[0];
        let own = shot.composites.last().unwrap();
        assert_eq!((own.name.as_str(), &own.inputs[..]), ("Storyboard Composite", &[Source::Layer(LayerId(background)), Source::Layer(LayerId(roughs))][..]));
        assert!(shot.group_of(own.id.0).is_some_and(|group| group.board) && shot.composites.len() == 2);
        let main = shot.composite(shot.main_composite().unwrap()).unwrap();
        assert_eq!(main.inputs.iter().filter(|input| **input == Source::Composite(own.id)).count(), 1);
        assert!(!main.inputs.contains(&Source::Layer(LayerId(roughs))));
        assert_eq!(shot.stack()[..2], [LayerId(background), LayerId(roughs)]);
    }
    // One step with the layer: undone, the first board takes its group with it.
    s.undo();
    s.undo();
    assert_eq!(groups(&s, 0), []);
    // A shot made with boards has the group from the start, with its two layers and composite.
    s.execute("shot.new", json!({ "boards": true })).unwrap();
    assert_eq!(groups(&s, 1), [("Storyboard".to_string(), true, 5)]);
    let shot = &s.project().shots[1];
    assert!(shot.layers.iter().all(|layer| layer.parent.and_then(|peg| shot.peg(peg)).is_some_and(|peg| peg.name == format!("{}-P", layer.name))));
}

#[test]
fn a_storyboard_has_markers_with_colours_and_a_stretch_that_plays() {
    let mut s = Session::new();
    let board = s.project().storyboards[0].id.0;
    let markers = |s: &Session| s.project().storyboards[0].markers.iter().map(|marker| (marker.frame, marker.name.clone(), marker.colour)).collect::<Vec<_>>();
    let blue = iskra_model::MARKER_COLOURS[0].1;
    // A marker is put on a frame, named, coloured and moved; they are kept in the film's order.
    s.execute("storyboard.setMarker", json!({ "storyboard": board, "frame": 30 })).unwrap();
    s.execute("storyboard.setMarker", json!({ "storyboard": board, "frame": 12, "name": " Door ", "colour": [219, 68, 55] })).unwrap();
    assert_eq!(markers(&s), [(12, "Door".to_string(), [219, 68, 55]), (30, String::new(), blue)]);
    s.execute("storyboard.setMarker", json!({ "storyboard": board, "frame": 30, "colour": [15, 157, 88] })).unwrap();
    s.execute("storyboard.setMarker", json!({ "storyboard": board, "frame": 12, "to": 40 })).unwrap();
    assert_eq!(markers(&s), [(30, String::new(), [15, 157, 88]), (40, "Door".to_string(), [219, 68, 55])]);
    assert!(turned_down(s.execute("storyboard.setMarker", json!({ "storyboard": board, "frame": 5, "to": 6 })), "no marker"));
    s.execute("storyboard.removeMarker", json!({ "storyboard": board, "frame": 30 })).unwrap();
    assert!(turned_down(s.execute("storyboard.removeMarker", json!({ "storyboard": board, "frame": 30 })), "no marker"));
    assert_eq!((markers(&s).len(), s.history().undo_label()), (1, Some("Remove Marker")));

    // The film plays whole until it has an in or an out point; each is set by itself.
    let played = |s: &Session| s.project().storyboards[0].played();
    assert_eq!(played(&s), (0, 48));
    s.execute("storyboard.setRange", json!({ "storyboard": board, "in": 10 })).unwrap();
    assert_eq!(played(&s), (10, 48));
    s.execute("storyboard.setRange", json!({ "storyboard": board, "out": 29 })).unwrap();
    assert_eq!(played(&s), (10, 30), "the out point's frame plays too");
    // An out point before the in point takes the in point back to the start, and the other way.
    s.execute("storyboard.setRange", json!({ "storyboard": board, "out": 5 })).unwrap();
    assert_eq!(played(&s), (0, 6));
    s.execute("storyboard.setRange", json!({ "storyboard": board, "in": 20 })).unwrap();
    assert_eq!(played(&s), (20, 48));
    assert!(turned_down(s.execute("storyboard.setRange", json!({ "storyboard": board })), "say what to set"));
    s.execute("storyboard.setRange", json!({ "storyboard": board, "clear": true })).unwrap();
    assert_eq!((played(&s), s.project().storyboards[0].range), ((0, 48), None));
}

#[test]
fn a_colour_card_is_stacked_under_everything_and_takes_no_picture() {
    use iskra_model::{EffectId, Source};
    let mut s = Session::new();
    let shot = |s: &Session| s.project().shots[0].clone();
    let layer = shot(&s).layers[0].id;
    let made = s.execute("effect.new", json!({ "kind": "colourCard", "stack": "under" })).unwrap()["id"].as_u64().unwrap();
    // At the bottom of the main composite, which stacks the top first, with nothing wired in.
    let card = Source::Effect(EffectId(made));
    assert_eq!((shot(&s).composites[0].inputs.clone(), shot(&s).effect(EffectId(made)).unwrap().picture()), (vec![Source::Layer(layer), card], None));
    let seen = iskra_eval::evaluate(&shot(&s), 0);
    assert!(matches!(seen.cards.first(), Some(iskra_eval::Card::Through { inside, .. }) if inside.is_empty()), "drawn first, under the layer: {seen:?}");
    assert_eq!(s.history().undo_label(), Some("Add Effect"));
    s.undo();
    assert_eq!(shot(&s).composites[0].inputs, vec![Source::Layer(layer)], "one step");
    // It takes no picture: not put on a wire, nor wired from a node; and only it goes `under`.
    let made = s.execute("effect.new", json!({ "kind": "colourCard" })).unwrap()["id"].as_u64().unwrap();
    assert!(turned_down(s.execute("effect.putOn", json!({ "effect": made, "on": { "layer": layer.0 } })), "takes no picture"));
    assert!(turned_down(s.execute("effect.new", json!({ "kind": "colourCard", "on": { "layer": layer.0 } })), "takes no picture"));
    assert!(turned_down(s.execute("effect.new", json!({ "kind": "blur", "stack": "under" })), "only a node that makes its own picture"));
    assert!(turned_down(s.execute("effect.new", json!({ "kind": "colourCard", "stack": "sideways" })), "\"under\" or \"over\""));
    // Rain is stacked over everything.
    let rain = s.execute("effect.new", json!({ "kind": "particles", "stack": "over" })).unwrap()["id"].as_u64().unwrap();
    assert_eq!(shot(&s).composites[0].inputs.first(), Some(&Source::Effect(EffectId(rain))));
}

#[test]
fn an_effect_node_taken_off_its_wire_leaves_the_wire_joined() {
    use iskra_model::{EffectId, Source};
    let mut s = Session::new();
    let main = s.project().shots[0].composites[0].id;
    let made = s.execute("effect.new", json!({ "kind": "transparency", "on": { "composite": main.0 } })).unwrap()["id"].as_u64().unwrap();
    let shot = |s: &Session| s.project().shots[0].clone();
    // The composite's picture goes through the node to the display.
    assert_eq!((shot(&s).display, shot(&s).effect(EffectId(made)).unwrap().picture()), (Some(Source::Effect(EffectId(made))), Some(Source::Composite(main))));
    // Taken off, the composite goes straight to the display, and the node stays, wired to
    // nothing, where it was put.
    s.execute("effect.takeOff", json!({ "effect": made, "at": [10.0, 20.0] })).unwrap();
    let after = shot(&s);
    assert_eq!(
        (after.display, after.effect(EffectId(made)).unwrap().picture(), after.places.get(&made)),
        (Some(Source::Composite(main)), None, Some(&[10.0, 20.0]))
    );
    assert_eq!(s.history().undo_label(), Some("Take Effect off Its Wire"));
    s.undo();
    assert_eq!(shot(&s).display, Some(Source::Effect(EffectId(made))));
    // A port that a transform goes through: taken off, what it carried is carried by the peg.
    let layer = shot(&s).layers[0].id.0;
    let peg = s.execute("peg.new", json!({})).unwrap()["id"].as_u64().unwrap();
    let port = s.execute("effect.new", json!({ "kind": "portIn", "parent": peg })).unwrap()["id"].as_u64().unwrap();
    s.execute("layer.setParent", json!({ "layer": layer, "parent": port })).unwrap();
    s.execute("effect.takeOff", json!({ "effect": port })).unwrap();
    let after = shot(&s);
    assert_eq!((after.layers[0].parent.map(|parent| parent.0), after.effect(EffectId(port)).unwrap().parent), (Some(peg), None));
    assert!(turned_down(s.execute("effect.new", json!({ "kind": "portIn", "parent": peg, "from": { "layer": layer } })), "one of"));
}

#[test]
fn an_effect_node_is_put_on_a_wire_set_keyed_switched_and_taken_off() {
    let mut s = Session::new();
    let layer = s.project().shots[0].layers[0].id.0;
    let main = s.project().shots[0].composites[0].id;
    let shot = |s: &Session| s.project().shots[0].clone();
    let id = |made: Value| made["id"].as_u64().unwrap();
    // The kinds say what there is to make: each with its ports and its parameters.
    let kinds = s.execute("effect.kinds", json!({})).unwrap();
    let fade = kinds["kinds"].as_array().unwrap().iter().find(|kind| kind["id"] == "transparency").expect("the first kind");
    assert_eq!(
        (&fade["category"], &fade["ports"][0]["name"], &fade["params"][0]["id"], &fade["params"][0]["max"]),
        (&json!("Colour"), &json!("picture"), &json!("amount"), &json!(100.0))
    );
    assert!(turned_down(s.execute("effect.new", json!({ "kind": "hologram" })), "no kind of effect"));

    // Put on the layer's wire, the node takes the layer's place in the composite, and the
    // layer goes through it: it is still what is seen, and still found in its composite.
    let node = id(s.execute("effect.new", json!({ "kind": "transparency", "on": { "layer": layer } })).unwrap());
    let (effect, source) = (iskra_model::EffectId(node), iskra_model::Source::Effect(iskra_model::EffectId(node)));
    assert_eq!(
        (shot(&s).composites[0].inputs.clone(), shot(&s).effect(effect).unwrap().picture()),
        (vec![source], Some(iskra_model::Source::Layer(iskra_model::LayerId(layer))))
    );
    assert_eq!(
        (shot(&s).stack(), shot(&s).place(iskra_model::LayerId(layer)), shot(&s).effect(effect).unwrap().name.as_str()),
        (vec![iskra_model::LayerId(layer)], Some((0, 1)), "Transparency")
    );
    assert_eq!(s.history().undo_label(), Some("Add Effect"));
    // A second on the same wire comes after the first; names are the shot's, and free.
    let second = id(s.execute("effect.new", json!({ "kind": "transparency", "on": { "effect": node } })).unwrap());
    assert_eq!(
        (shot(&s).effect(iskra_model::EffectId(second)).unwrap().name.as_str(), shot(&s).tip(iskra_model::Source::Layer(iskra_model::LayerId(layer)))),
        ("Transparency 2", iskra_model::Source::Effect(iskra_model::EffectId(second)))
    );
    assert!(turned_down(s.execute("effect.rename", json!({ "effect": second, "name": "Transparency" })), "already has an effect named"));
    s.execute("effect.rename", json!({ "effect": second, "name": "Fade Out" })).unwrap();
    // Nothing is wired into what it gets its picture from.
    assert!(turned_down(s.execute("effect.connect", json!({ "effect": node, "node": { "effect": second } })), "gets its picture from"));
    assert!(turned_down(s.execute("effect.connect", json!({ "effect": node, "node": { "composite": main.0 } })), "gets its picture from"));
    assert!(turned_down(s.execute("effect.connect", json!({ "effect": node, "port": "matte", "node": { "layer": layer } })), "no port"));
    // Deleted, the wire is joined again.
    s.execute("effect.delete", json!({ "effect": second })).unwrap();
    assert_eq!((shot(&s).composites[0].inputs.clone(), shot(&s).effects.len()), (vec![source], 1));

    // A parameter is set, within its range, and what is no parameter or no number is refused.
    let amount = |s: &Session, frame: u32| shot(s).effect(effect).unwrap().number("amount", frame);
    assert_eq!(amount(&s, 0), 50.0);
    s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": 250 })).unwrap();
    assert_eq!((amount(&s, 0), s.history().undo_label()), (100.0, Some("Set Effect")));
    assert!(turned_down(s.execute("effect.set", json!({ "effect": node, "param": "radius", "value": 1 })), "has no `radius`"));
    assert!(turned_down(s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": true })), "is a number"));
    // Keyed, it is what the keys say; then it is set by key, on a frame.
    s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": 0 })).unwrap();
    s.execute("effect.setKey", json!({ "effect": node, "frame": 0 })).unwrap();
    assert!(turned_down(s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": 100 })), "has keyframes"));
    s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": 100, "frame": 10 })).unwrap();
    assert_eq!((amount(&s, 0), amount(&s, 5), amount(&s, 10)), (0.0, 50.0, 100.0));
    s.execute("effect.setEase", json!({ "effect": node, "frame": 0, "out": "hold" })).unwrap();
    assert_eq!(amount(&s, 5), 0.0, "held until the next key");
    s.execute("effect.moveKey", json!({ "effect": node, "param": "amount", "from": 10, "to": 4 })).unwrap();
    assert_eq!((amount(&s, 3), amount(&s, 4)), (0.0, 100.0));
    assert!(turned_down(s.execute("effect.moveKey", json!({ "effect": node, "from": 10, "to": 12 })), "no keyframe on frame 10"));
    assert!(turned_down(s.execute("effect.setEase", json!({ "effect": node, "frame": 0, "into": "hold" })), "not into one"));
    // The last key taken away leaves the parameter as that key had it.
    s.execute("effect.removeKey", json!({ "effect": node, "frame": 0 })).unwrap();
    s.execute("effect.removeKey", json!({ "effect": node, "frame": 4 })).unwrap();
    assert!(shot(&s).effect(effect).unwrap().key_frames(None).is_empty() && amount(&s, 0) == 100.0);

    // What a shot renders as changes with its nodes, and with them being off.
    let revision = shot(&s).revision;
    s.execute("effect.setEnabled", json!({ "effect": node, "enabled": false })).unwrap();
    assert!(shot(&s).revision != revision && !shot(&s).effect(effect).unwrap().enabled);
    // A copy of the shot has its own node, wired to its own layer.
    let first = shot(&s).id.0;
    let copy = id(s.execute("shot.duplicate", json!({ "shot": first })).unwrap());
    let copy = s.project().shot(ShotId(copy)).unwrap().clone();
    let (copied, own) = (&copy.effects[0], iskra_model::Source::Layer(copy.layers[0].id));
    assert!(copied.id != effect && copied.picture() == Some(own) && !copied.enabled && copied.number("amount", 0) == 100.0);
    assert_eq!(copy.tip(own), iskra_model::Source::Effect(copied.id));

    // Saved and opened again, the node is there as it was.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Fade.iskra");
    s.execute("file.saveAs", json!({ "path": path })).unwrap();
    let before = shot(&s).effects.clone();
    let mut opened = Session::new();
    opened.execute("file.open", json!({ "path": path })).unwrap();
    assert_eq!(opened.project().shots[0].effects, before);
    // The layer deleted, the node stays with nothing going through it, and is deleted itself.
    s.execute("layer.delete", json!({ "layer": layer })).unwrap();
    assert_eq!(shot(&s).effect(effect).unwrap().picture(), None);
    s.execute("effect.delete", json!({ "effect": node })).unwrap();
    assert!(shot(&s).effects.is_empty() && turned_down(s.execute("effect.delete", json!({ "effect": node })), "no effect node"));
}

#[test]
fn an_effect_node_goes_with_its_layer_into_a_template_and_through_a_split_and_a_merge() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let layer = s.project().shots[0].layers[0].id.0;
    s.execute("drawing.stroke", stroke_params(layer, 0, json!({ "size": 20 }))).unwrap();
    let node = s.execute("effect.new", json!({ "kind": "transparency", "name": "Fade", "on": { "layer": layer } })).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": 30 })).unwrap();
    // A layer saved to the library takes what is on its wire.
    let saved = s.execute("library.save", json!({ "folder": dir.path(), "name": "Faded", "nodes": [{ "layer": layer }] })).unwrap();
    let mut other = Session::new();
    other.execute("library.place", json!({ "path": saved["path"] })).unwrap();
    let shot = other.project().shots[0].clone();
    let [copied] = &shot.effects[..] else { panic!("one effect came along: {:?}", shot.effects) };
    let through = iskra_model::Source::Layer(shot.layers[0].id);
    assert_eq!((copied.name.as_str(), copied.picture(), copied.number("amount", 0), copied.template.as_deref()), ("Fade", Some(through), 30.0, Some("Faded")));
    assert_eq!(shot.composites[0].inputs[0], iskra_model::Source::Effect(copied.id), "the end of the wire is what is stacked, on top");

    // Split, each half has the node; merged again, the one shot has both layers with theirs.
    let first = s.project().shots[0].id.0;
    let second = s.execute("shot.split", json!({ "shot": first, "frame": 24 })).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.project().shot(ShotId(second)).unwrap().effects.len(), 1);
    s.execute("shot.merge", json!({ "shot": first })).unwrap();
    let merged = s.project().shots[0].clone();
    assert_eq!((merged.effects.len(), merged.layers.len(), merged.stack().len()), (2, 2, 2));
    for effect in &merged.effects {
        let Some(iskra_model::Source::Layer(under)) = effect.picture() else { panic!("each is wired to a layer: {effect:?}") };
        assert!(merged.layer(under).is_some() && merged.composite_of(iskra_model::Source::Layer(under)).is_some());
    }
}

#[test]
fn the_storyboards_group_hidden_on_the_animation_page_is_seen_on_the_storyboards() {
    let mut s = Session::new();
    let board = iskra_model::LayerId(id(s.execute("layer.new", json!({ "role": "board" })).unwrap()));
    s.execute("drawing.stroke", stroke_params(board.0, 0, json!({ "size": 20 }))).unwrap();
    let shot = s.project().shots[0].id.0;
    let group = s.project().shots[0].groups.iter().find(|group| group.board).map(|group| group.id.0);
    let group = group.unwrap_or_else(|| id(s.execute("group.new", json!({ "nodes": [board.0] })).unwrap()));
    s.execute("group.set", json!({ "group": group, "board": true })).unwrap();
    s.execute("shot.setHidden", json!({ "node": group, "hidden": true })).unwrap();
    let shot = s.project().shot(iskra_model::ShotId(shot)).unwrap().clone();
    // Hidden on the Animation page, and in what the shot shows of its animation; seen as the
    // storyboard, whose boards it is.
    assert!(shot.hides(board));
    assert!(!shot.only(iskra_model::Work::Boards).hides(board));
    assert!(iskra_eval::evaluate(&shot.only(iskra_model::Work::Boards), 0).layer(board).is_some());
    assert!(iskra_eval::evaluate(&shot, 0).layer(board).is_none());
}

#[test]
fn a_panel_goes_with_the_board_drawing_it_starts_with() {
    let mut s = Session::fresh();
    let character = s.project().shots[0].layers.iter().find(|layer| layer.name == "Character").unwrap().id.0;
    let background = s.project().shots[0].layers.iter().find(|layer| layer.name == "Background").unwrap().id.0;
    s.execute("drawing.newDrawing", json!({ "layer": character, "frame": 0, "hold": 12 })).unwrap();
    s.execute("drawing.newDrawing", json!({ "layer": character, "frame": 12, "hold": 12 })).unwrap();
    s.execute("drawing.newDrawing", json!({ "layer": background, "frame": 0, "hold": 24 })).unwrap();
    s.execute("panel.add", json!({ "frame": 12, "action": "He turns" })).unwrap();
    let panels = |s: &Session| s.project().shots[0].panels().iter().map(|panel| panel.frame).collect::<Vec<_>>();
    // Frames held longer before it: the character's second drawing starts later, and the
    // panel with it, in the same step.
    s.execute("exposure.insert", json!({ "layer": character, "frame": 5, "length": 3 })).unwrap();
    assert_eq!((panels(&s), s.history().undo_label()), (vec![0, 15], Some("Add Exposure")));
    assert_eq!(s.project().shots[0].panel_on(15).unwrap().action, "He turns");
    s.undo();
    assert_eq!(panels(&s), [0, 12]);
    // A change to the background, which starts no drawing there, moves nothing.
    s.execute("exposure.insert", json!({ "layer": background, "frame": 5, "length": 3 })).unwrap();
    assert_eq!(panels(&s), [0, 12]);
    // A panel moved by hand stays where it is put.
    s.execute("panel.move", json!({ "frame": 12, "to": 20 })).unwrap();
    assert_eq!(panels(&s), [0, 20]);
}

#[test]
fn an_effect_put_on_a_drawing_leaves_the_cutters_matte_wire_from_the_drawing() {
    use iskra_model::{EffectId, LayerId, Source};
    let mut s = Session::new();
    let drawing = layer(&s);
    let other = id(s.execute("layer.new", json!({ "name": "Cut" })).unwrap());
    // A cutter cuts the other layer by the drawing, which is also stacked by the composite.
    let cutter = id(s.execute("effect.new", json!({ "kind": "cutter", "on": { "layer": other } })).unwrap());
    s.execute("effect.connect", json!({ "effect": cutter, "port": iskra_model::MATTE, "node": { "layer": drawing } })).unwrap();
    let shot = |s: &Session| s.project().shots[0].clone();
    assert!(shot(&s).composites[0].inputs.contains(&Source::Layer(LayerId(drawing))));
    // An effect put on the drawing's picture: the composite takes it through the effect, and
    // the cutter's matte is still the drawing itself.
    let blur = id(s.execute("effect.new", json!({ "kind": "blur", "on": { "layer": drawing } })).unwrap());
    let now = shot(&s);
    assert!(now.composites[0].inputs.contains(&Source::Effect(EffectId(blur))) && !now.composites[0].inputs.contains(&Source::Layer(LayerId(drawing))));
    assert_eq!(now.effect(EffectId(cutter)).unwrap().inputs.get(iskra_model::MATTE), Some(&Source::Layer(LayerId(drawing))));
}

#[test]
fn new_bones_go_above_their_drawing_in_the_node_view_and_its_peg_above_them() {
    let mut s = Session::new();
    let arm = layer(&s);
    let peg = id(s.execute("peg.new", json!({ "aboveLayer": arm })).unwrap());
    // The drawing put by hand, and its peg just above it.
    s.execute("node.place", json!({ "moves": [{ "node": arm, "at": [100.0, 300.0] }, { "node": peg, "at": [100.0, 280.0] }] })).unwrap();
    let bones = json!([{ "angle": 0, "length": 75 }]);
    let chain = id(s.execute("deformer.new", json!({ "layer": arm, "origin": [0, 0], "bones": bones })).unwrap());
    let places = &s.project().shots[0].places;
    let (bones_at, peg_at) = (places[&chain], places[&peg]);
    assert!((bones_at[0] - 100.0).abs() < 1e-9 && bones_at[1] < 300.0 && peg_at[1] < bones_at[1], "{bones_at:?} {peg_at:?}");
}

#[test]
fn a_board_opened_to_be_drawn_on_stays_open_with_the_shot() {
    let mut s = Session::new();
    let board = id(s.execute("layer.new", json!({ "role": "board" })).unwrap());
    let drawn = layer(&s);
    let unlocked = |s: &Session| s.project().shots[0].unlocked.iter().map(|layer| layer.0).collect::<Vec<_>>();
    s.execute("shot.setUnlocked", json!({ "layer": board, "unlocked": true })).unwrap();
    assert_eq!((unlocked(&s), s.history().undo_label()), (vec![board], Some("Lock or Unlock Board")));
    // Kept in the file: a project read back has it open.
    let read: iskra_model::Project = serde_json::from_value(serde_json::to_value(s.project()).unwrap()).unwrap();
    assert!(read.shots[0].unlocked.contains(&iskra_model::LayerId(board)));
    s.undo();
    assert!(unlocked(&s).is_empty());
    let animation = s.project().shots[0].layers.iter().find(|layer| layer.id.0 != board && layer.id.0 != drawn).map_or(drawn, |layer| layer.id.0);
    assert!(turned_down(s.execute("shot.setUnlocked", json!({ "layer": animation, "unlocked": true })), "only a board"));
}

#[test]
fn displays_go_through_a_split_and_a_merge() {
    use iskra_model::Source;
    let mut s = Session::new();
    let layer = s.project().shots[0].layers[0].id;
    let all = s.project().shots[0].composites[0].id;
    s.execute("drawing.stroke", stroke_params(layer.0, 0, json!({ "size": 20 }))).unwrap();
    // A matte of the layer alone, and a display of all that the shot stacks.
    s.execute("display.new", json!({ "name": "Matte", "node": { "layer": layer.0 } })).unwrap();
    s.execute("display.new", json!({ "name": "All", "node": { "composite": all.0 } })).unwrap();
    let first = s.project().shots[0].id.0;
    let second = s.execute("shot.split", json!({ "shot": first, "frame": 24 })).unwrap()["id"].as_u64().unwrap();
    let half = s.project().shot(ShotId(second)).unwrap().clone();
    let names = |shot: &iskra_model::Shot| shot.displays.iter().map(|display| display.name.clone()).collect::<Vec<_>>();
    assert_eq!((names(&half), half.displays[0].shows), (vec!["Matte".to_string(), "All".to_string()], Some(Source::Layer(half.layers[0].id))));
    // Merged, the second half's come along under names of their own: its matte shows its own
    // layer, and what showed all that it stacked shows all that the merged shot stacks.
    s.execute("shot.merge", json!({ "shot": first })).unwrap();
    let merged = s.project().shots[0].clone();
    assert_eq!(names(&merged), ["Matte", "All", "Matte 2", "All 2"]);
    let later = merged.layers.iter().find(|own| own.id != layer).unwrap().id;
    assert_eq!((merged.displays[2].shows, merged.displays[3].shows), (Some(Source::Layer(later)), Some(Source::Composite(all))));
    let ids: std::collections::BTreeSet<u64> = merged.node_ids().collect();
    assert_eq!(ids.len(), merged.node_ids().count(), "no two nodes share an id");
}

#[test]
fn ports_sends_and_receives_go_through_a_split_and_a_merge() {
    use iskra_model::PegId;
    let mut s = Session::new();
    let layer = s.project().shots[0].layers[0].id.0;
    s.execute("drawing.stroke", stroke_params(layer, 0, json!({ "size": 20 }))).unwrap();
    // The layer in a group, carried by a peg outside it through a Port In; and a second layer
    // carried by the same peg through a Send and a Receive.
    let peg = s.execute("peg.new", json!({})).unwrap()["id"].as_u64().unwrap();
    s.execute("layer.setParent", json!({ "layer": layer, "parent": peg })).unwrap();
    s.execute("group.new", json!({ "nodes": [layer], "ports": true })).unwrap();
    let arm = s.execute("layer.new", json!({ "name": "Arm" })).unwrap()["id"].as_u64().unwrap();
    s.execute("drawing.stroke", stroke_params(arm, 0, json!({ "size": 20 }))).unwrap();
    let send = s.execute("effect.new", json!({ "kind": "send", "name": "Shoulder" })).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.setParent", json!({ "effect": send, "parent": peg })).unwrap();
    let receive = s.execute("effect.new", json!({ "kind": "receive" })).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.set", json!({ "effect": receive, "param": "tag", "value": "Shoulder" })).unwrap();
    s.execute("layer.setParent", json!({ "layer": arm, "parent": receive })).unwrap();
    // Each layer is carried by a peg of its own shot, by way of what is in that shot.
    let carried = |shot: &iskra_model::Shot| {
        let mut pegs: Vec<PegId> = shot.layers.iter().map(|layer| shot.peg_above(layer.parent).expect("carried by a peg").id).collect();
        pegs.sort();
        pegs.dedup();
        pegs
    };
    assert_eq!(carried(&s.project().shots[0]), [PegId(peg)]);
    let first = s.project().shots[0].id.0;
    let second = s.execute("shot.split", json!({ "shot": first, "frame": 24 })).unwrap()["id"].as_u64().unwrap();
    let half = s.project().shot(ShotId(second)).unwrap().clone();
    assert_eq!((carried(&half).len(), half.effects.len(), half.groups.len()), (1, 4, 1));
    s.execute("shot.merge", json!({ "shot": first })).unwrap();
    let merged = s.project().shots[0].clone();
    assert_eq!((merged.layers.len(), merged.effects.len(), merged.groups.len()), (4, 8, 2));
    // Two pegs, each carrying the layers of its half: the Receive of the first half reads its
    // Send, and the second half's reads the second half's, under a tag of its own.
    assert_eq!(carried(&merged).len(), 2);
    let receives: Vec<_> = merged.effects.iter().filter(|effect| effect.kind == iskra_model::RECEIVE).collect();
    let sends: Vec<_> = receives.iter().map(|receive| merged.sender(receive).map(|send| send.id.0)).collect();
    let later = sends.iter().flatten().find(|id| **id != send).copied();
    assert_eq!((receives.len(), sends[0], later.is_some()), (2, Some(send), true), "{sends:?}");
    let peg_of = |name_of: u64| merged.peg_above(Some(PegId(name_of))).map(|peg| peg.id);
    assert!(peg_of(send) == Some(PegId(peg)) && peg_of(later.unwrap()) != Some(PegId(peg)));
    assert_eq!(merged.tags(), ["Shoulder", "Shoulder 2"]);
    // Both groups have their ports, and the Sends' tags are told apart.
    for group in &merged.groups {
        let ports = merged.group_nodes(group.id).into_iter().filter(|node| merged.effect(iskra_model::EffectId(*node)).is_some()).count();
        assert_eq!(ports, 2, "{group:?}");
    }
    assert!(merged.effects.iter().filter_map(|send| merged.tag_shared(send)).next().is_none(), "{:?}", merged.tags());
}

#[test]
fn a_template_takes_the_transform_that_goes_through_ports_sends_and_receives() {
    use iskra_model::{PegId, RECEIVE, SEND};
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let layer = s.project().shots[0].layers[0].id.0;
    s.execute("drawing.stroke", stroke_params(layer, 0, json!({ "size": 20 }))).unwrap();
    // A peg moves a Send; a Receive of it carries a peg, which carries the arm.
    let peg = s.execute("peg.new", json!({ "name": "Root" })).unwrap()["id"].as_u64().unwrap();
    s.execute("peg.setTransform", json!({ "peg": peg, "position": [30.0, 0.0] })).unwrap();
    let send = s.execute("effect.new", json!({ "kind": "send", "name": "Shoulder" })).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.setParent", json!({ "effect": send, "parent": peg })).unwrap();
    let receive = s.execute("effect.new", json!({ "kind": "receive" })).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.set", json!({ "effect": receive, "param": "tag", "value": "Shoulder" })).unwrap();
    let elbow = s.execute("peg.new", json!({ "name": "Elbow", "parent": receive })).unwrap()["id"].as_u64().unwrap();
    s.execute("layer.setParent", json!({ "layer": layer, "parent": elbow })).unwrap();
    // Saved from the Root, all of it comes along: the arm is carried by the Root in the copy.
    let saved = s.execute("library.save", json!({ "folder": dir.path(), "name": "Arm", "nodes": [{ "peg": peg }] })).unwrap();
    let mut other = Session::new();
    other.execute("library.place", json!({ "path": saved["path"] })).unwrap();
    let shot = other.project().shots[0].clone();
    let kinds: Vec<&str> = shot.effects.iter().map(|effect| effect.kind.as_str()).collect();
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    assert!(kinds.contains(&SEND) && kinds.contains(&RECEIVE));
    let arm = shot.layers.iter().find(|layer| layer.parent.is_some()).expect("the arm keeps its parent");
    assert_eq!(shot.ancestors(arm.parent).map(|peg| peg.name.as_str()).collect::<Vec<_>>(), ["Elbow", "Root"]);
    let moved = iskra_eval::evaluate(&shot, 0).layer(arm.id).unwrap().to_scene.m[4];
    assert_eq!(moved, 30.0);
    // Saved from the Elbow alone, the Receive comes with it, and the Send: what moved them
    // stays behind, so the Send is carried by nothing.
    let saved = s.execute("library.save", json!({ "folder": dir.path(), "name": "Forearm", "nodes": [{ "peg": elbow }] })).unwrap();
    let mut other = Session::new();
    other.execute("library.place", json!({ "path": saved["path"] })).unwrap();
    let shot = other.project().shots[0].clone();
    assert_eq!(shot.effects.len(), 2);
    assert!(
        shot.effects
            .iter()
            .all(|effect| effect.parent.is_none_or(|parent| shot.peg(parent).is_some() || shot.effect(iskra_model::EffectId(parent.0)).is_some()))
    );
    let elbow = shot.pegs.iter().find(|peg| peg.name == "Elbow").unwrap();
    assert!(elbow.parent.is_some_and(|parent| shot.effect(iskra_model::EffectId(parent.0)).is_some()), "the Elbow is still under the Receive");
    assert_eq!(shot.peg_above(elbow.parent).map(|peg| peg.id), None::<PegId>);
}

#[test]
fn an_effect_nodes_colour_switch_and_choice_are_set_as_its_kind_says() {
    let mut s = Session::new();
    let layer = s.project().shots[0].layers[0].id.0;
    let kinds = s.execute("effect.kinds", json!({})).unwrap();
    let ids: Vec<&str> = kinds["kinds"].as_array().unwrap().iter().filter_map(|kind| kind["id"].as_str()).collect();
    assert_eq!(
        ids,
        [
            "colourCard",
            "cutter",
            "blend",
            "glow",
            "dropShadow",
            "tone",
            "highlight",
            "blur",
            "directionalBlur",
            "radialBlur",
            "colourCorrection",
            "transparency",
            "style",
            "particles",
            "portIn",
            "portOut",
            "send",
            "receive",
            "visible"
        ]
    );
    let node = |s: &mut Session, kind: &str| s.execute("effect.new", json!({ "kind": kind, "on": { "layer": layer } })).unwrap()["id"].as_u64().unwrap();
    let effect = |s: &Session, id: u64| s.project().shots[0].effect(iskra_model::EffectId(id)).unwrap().clone();
    // A colour is red, green and blue, with how solid if it is said, each held to 0 to 1.
    let glow = node(&mut s, "glow");
    assert_eq!(effect(&s, glow).colour("colour", 0), [1.0, 0.95, 0.6, 1.0]);
    s.execute("effect.set", json!({ "effect": glow, "param": "colour", "value": [0.2, 0.4, 2.0] })).unwrap();
    assert_eq!(effect(&s, glow).colour("colour", 0), [0.2, 0.4, 1.0, 1.0]);
    s.execute("effect.set", json!({ "effect": glow, "param": "colour", "value": [0.0, 0.0, 0.0, 0.5] })).unwrap();
    assert_eq!(effect(&s, glow).colour("colour", 0), [0.0, 0.0, 0.0, 0.5]);
    assert!(turned_down(s.execute("effect.set", json!({ "effect": glow, "param": "colour", "value": 3 })), "is a colour"));
    assert!(turned_down(s.execute("effect.set", json!({ "effect": glow, "param": "radius", "value": [1, 2, 3] })), "is a number"));
    // Only numbers have keyframes.
    assert!(turned_down(s.execute("effect.setKey", json!({ "effect": glow, "param": "colour", "frame": 0 })), "only those have keyframes"));
    s.execute("effect.setKey", json!({ "effect": glow, "frame": 0 })).unwrap();
    assert_eq!(effect(&s, glow).params.iter().filter(|(_, held)| !held.keys.is_empty()).count(), 2, "its two numbers, and not its colour");
    // A Visible node's switch is keyed, and stays as a keyframe has it until the next.
    let visible = node(&mut s, "visible");
    let shown = |s: &Session, frame: u32| effect(s, visible).switch("shown", frame);
    s.execute("effect.setKey", json!({ "effect": visible, "frame": 0 })).unwrap();
    s.execute("effect.set", json!({ "effect": visible, "param": "shown", "frame": 6, "value": false })).unwrap();
    s.execute("effect.set", json!({ "effect": visible, "param": "shown", "frame": 9, "value": true })).unwrap();
    assert_eq!([0, 5, 6, 8, 9, 30].map(|frame| shown(&s, frame)), [true, true, false, false, true, true]);
    assert!(turned_down(s.execute("effect.set", json!({ "effect": visible, "param": "shown", "value": false })), "has keyframes"));
    s.execute("effect.moveKey", json!({ "effect": visible, "from": 6, "to": 3 })).unwrap();
    assert_eq!([2, 3, 8].map(|frame| shown(&s, frame)), [true, false, false]);
    // Its last keyframe taken away, it stays as that one had it.
    for frame in [0, 9] {
        s.execute("effect.removeKey", json!({ "effect": visible, "param": "shown", "frame": frame })).unwrap();
    }
    s.execute("effect.removeKey", json!({ "effect": visible, "frame": 3 })).unwrap();
    assert_eq!((shown(&s, 0), effect(&s, visible).key_frames(None).len()), (false, 0));
    // Hidden on a frame, the layer is not among what the shot shows there.
    s.execute("effect.set", json!({ "effect": visible, "param": "shown", "value": true })).unwrap();
    s.execute("effect.setKey", json!({ "effect": visible, "frame": 0 })).unwrap();
    s.execute("effect.set", json!({ "effect": visible, "param": "shown", "frame": 4, "value": false })).unwrap();
    let seen = |s: &Session, frame: u32| iskra_eval::evaluate(&s.project().shots[0], frame).layers.len();
    assert_eq!((seen(&s, 0), seen(&s, 4)), (1, 0));
    s.execute("effect.setEnabled", json!({ "effect": visible, "enabled": false })).unwrap();
    assert_eq!(seen(&s, 4), 1, "switched off, the node hides nothing");
    // A switch and a choice.
    let cutter = node(&mut s, "cutter");
    s.execute("effect.set", json!({ "effect": cutter, "param": "inverted", "value": true })).unwrap();
    assert!(effect(&s, cutter).switch("inverted", 0));
    assert!(turned_down(s.execute("effect.set", json!({ "effect": cutter, "param": "inverted", "value": 1 })), "true or false"));
    // A matte is wired into the port a cutter has for it.
    let other = s.execute("layer.new", json!({})).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.connect", json!({ "effect": cutter, "port": "matte", "node": { "layer": other } })).unwrap();
    assert_eq!(effect(&s, cutter).inputs.get("matte"), Some(&iskra_model::Source::Layer(iskra_model::LayerId(other))));
    // A colour correction's numbers are held to their ranges and keyed as any; its tint is a colour.
    let grade = node(&mut s, "colourCorrection");
    s.execute("effect.set", json!({ "effect": grade, "param": "saturation", "value": -250 })).unwrap();
    s.execute("effect.set", json!({ "effect": grade, "param": "tint", "value": [0.0, 0.5, 1.0] })).unwrap();
    s.execute("effect.setKey", json!({ "effect": grade, "param": "hue", "frame": 0 })).unwrap();
    s.execute("effect.set", json!({ "effect": grade, "param": "hue", "frame": 4, "value": 90 })).unwrap();
    let graded = effect(&s, grade);
    assert_eq!(
        (graded.number("saturation", 0), graded.number("hue", 2), graded.number("hue", 4), graded.colour("tint", 0)),
        (-100.0, 45.0, 90.0, [0.0, 0.5, 1.0, 1.0])
    );
    // Its curves are points from dark to light, put in order and held to 0 to 1; one that is
    // left out, or is the straight line, has no points.
    assert!(graded.curves("curves").is_straight());
    let points = json!({ "master": [[1.0, 0.9], [0.0, 0.1], [0.5, 2.0]], "blue": [[0.0, 0.0], [1.0, 1.0]] });
    s.execute("effect.set", json!({ "effect": grade, "param": "curves", "value": points })).unwrap();
    let curves = effect(&s, grade).curves("curves");
    assert_eq!((curves.master.clone(), curves.blue.len(), curves.red.len()), (vec![[0.0, 0.1], [0.5, 1.0], [1.0, 0.9]], 0, 0));
    assert_eq!((curves.at(0, 0.5), curves.at(0, 0.0), curves.at(1, 0.3), s.history().undo_label()), (1.0, 0.1, 0.3, Some("Set Effect")));
    assert!((0.1..=1.0).contains(&curves.at(0, 0.25)) && (0.9..=1.0).contains(&curves.at(0, 0.75)), "between its points it stays between them");
    for (wrong, why) in [
        (json!({ "master": [[0.5, 0.5]] }), "from 2 to 16 points"),
        (json!({ "red": [[0.5, 0.2], [0.5, 0.8]] }), "on the same tone"),
        (json!({ "green": "steep" }), "as curves is"),
    ] {
        assert!(turned_down(s.execute("effect.set", json!({ "effect": grade, "param": "curves", "value": wrong })), why), "{why}");
    }
    assert!(turned_down(s.execute("effect.setKey", json!({ "effect": grade, "param": "curves", "frame": 0 })), "neither a number nor a switch"));
    let spin = node(&mut s, "radialBlur");
    s.execute("effect.set", json!({ "effect": spin, "param": "kind", "value": "Zoom" })).unwrap();
    assert_eq!(effect(&s, spin).choice("kind", 0), "Zoom");
    assert!(turned_down(s.execute("effect.set", json!({ "effect": spin, "param": "kind", "value": "Twist" })), "is one of: Spin, Zoom"));
}

#[test]
fn a_send_is_read_by_the_receives_that_name_it_and_a_port_hands_on_what_it_is_given() {
    let mut s = Session::new();
    let composite = s.project().shots[0].composites[0].id.0;
    let arm = s.execute("layer.new", json!({ "name": "Arm" })).unwrap()["id"].as_u64().unwrap();
    let seen = |s: &Session| iskra_eval::evaluate(&s.project().shots[0], 0).layers.iter().map(|placed| placed.layer.0).collect::<Vec<_>>();
    let first = s.project().shots[0].layers.iter().map(|layer| layer.id.0).find(|id| *id != arm).unwrap();
    assert_eq!(seen(&s), [first, arm]);
    // The arm goes into a Send and to nothing else: it is not seen.
    s.execute("node.disconnect", json!({ "node": { "layer": arm } })).unwrap();
    let send = s.execute("effect.new", json!({ "kind": "send", "name": "Arm Out", "on": { "layer": arm } })).unwrap()["id"].as_u64().unwrap();
    assert_eq!(seen(&s), [first]);
    // A Receive that names the Send gives the arm where it is wired, with no wire between.
    let receive = s.execute("effect.new", json!({ "kind": "receive" })).unwrap()["id"].as_u64().unwrap();
    s.execute("node.connect", json!({ "node": { "effect": receive }, "to": composite })).unwrap();
    assert_eq!(seen(&s), [first], "it names nothing yet");
    s.execute("effect.set", json!({ "effect": receive, "param": "tag", "value": " Arm Out " })).unwrap();
    assert_eq!(seen(&s), [first, arm]);
    // It goes on as it came: the arm is a card of its own, not a picture made of it.
    let cards = iskra_eval::evaluate(&s.project().shots[0], 0).cards;
    assert!(cards.len() == 2 && cards.iter().all(|card| matches!(card, iskra_eval::Card::Layer(_))));
    // They go together by the tag, which a new Send has its name for; the node's name is
    // another thing, and a rename changes nothing.
    assert_eq!(s.project().shots[0].tags(), ["Arm Out"]);
    s.execute("effect.rename", json!({ "effect": send, "name": "Hand" })).unwrap();
    assert_eq!(seen(&s), [first, arm]);
    // Another tag on the Send, and the Receive that read it follows, in the same step.
    let tag = |s: &Session, node: u64| s.project().shots[0].effect(iskra_model::EffectId(node)).unwrap().text(iskra_model::TAG);
    s.execute("effect.set", json!({ "effect": send, "param": "tag", "value": "hand" })).unwrap();
    assert_eq!((seen(&s), tag(&s, receive)), (vec![first, arm], "hand".to_string()));
    s.undo();
    assert_eq!((seen(&s), tag(&s, receive)), (vec![first, arm], "Arm Out".to_string()));
    s.redo();
    // A Receive that names another tag is left as it is.
    s.execute("effect.set", json!({ "effect": receive, "param": "tag", "value": "elsewhere" })).unwrap();
    assert_eq!(seen(&s), [first], "the Receive has no Send to go with");
    s.execute("effect.set", json!({ "effect": send, "param": "tag", "value": "Hand" })).unwrap();
    assert_eq!(tag(&s, receive), "elsewhere");
    s.execute("effect.set", json!({ "effect": receive, "param": "tag", "value": "Hand" })).unwrap();
    assert_eq!(seen(&s), [first, arm]);
    // Of two Sends with one tag the Receive reads the first: the second's tag changed, it stays.
    let twin = s.execute("effect.new", json!({ "kind": "send", "name": "Twin" })).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.set", json!({ "effect": twin, "param": "tag", "value": "Hand" })).unwrap();
    s.execute("effect.set", json!({ "effect": twin, "param": "tag", "value": "Other" })).unwrap();
    assert_eq!((seen(&s), tag(&s, receive)), (vec![first, arm], "Hand".to_string()));
    s.execute("effect.delete", json!({ "effect": twin })).unwrap();
    s.execute("effect.setEnabled", json!({ "effect": receive, "enabled": false })).unwrap();
    assert_eq!(seen(&s), [first], "switched off, a Receive gives nothing");
    s.execute("effect.setEnabled", json!({ "effect": receive, "enabled": true })).unwrap();
    // A Receive takes no wire, and what it gives can't be wired back into its Send.
    assert!(turned_down(s.execute("effect.new", json!({ "kind": "receive", "on": { "layer": arm } })), "takes no picture"));
    assert!(s.execute("effect.connect", json!({ "effect": receive, "port": "picture", "node": { "layer": arm } })).is_err());
    assert!(s.execute("effect.connect", json!({ "effect": send, "port": "picture", "node": { "effect": receive } })).is_err());
    assert_eq!(seen(&s), [first, arm]);

    // A port of a group is in the group, and hands on what is wired into it as it came.
    let group = s.execute("group.new", json!({ "nodes": [first], "name": "Body" })).unwrap()["id"].as_u64().unwrap();
    let port = s.execute("effect.new", json!({ "kind": "portOut", "on": { "layer": first }, "group": group })).unwrap()["id"].as_u64().unwrap();
    let shot = s.project().shots[0].clone();
    assert!(shot.group(iskra_model::GroupId(group)).unwrap().members.contains(&port) && shot.effect(iskra_model::EffectId(port)).unwrap().name == "Port Out");
    assert_eq!((seen(&s), iskra_eval::evaluate(&shot, 0).cards.len()), (vec![first, arm], 2));
    assert!(turned_down(s.execute("effect.new", json!({ "kind": "portIn", "group": 9999 })), "no group"));
    // A transform goes through them as a picture does. A peg is wired into a Send, and the arm
    // is parented to a Receive that names it: the peg carries the arm.
    let peg = s.execute("peg.new", json!({})).unwrap()["id"].as_u64().unwrap();
    s.execute("peg.setTransform", json!({ "peg": peg, "position": [30.0, 0.0] })).unwrap();
    let moved = |s: &Session, layer: u64| iskra_eval::evaluate(&s.project().shots[0], 0).layer(iskra_model::LayerId(layer)).unwrap().to_scene.m[4];
    let carry = s.execute("effect.new", json!({ "kind": "send", "name": "Shoulder" })).unwrap()["id"].as_u64().unwrap();
    let down = s.execute("effect.new", json!({ "kind": "receive" })).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.set", json!({ "effect": down, "param": "tag", "value": "Shoulder" })).unwrap();
    s.execute("layer.setParent", json!({ "layer": arm, "parent": down })).unwrap();
    assert_eq!(moved(&s, arm), 0.0, "nothing is wired into the Send yet");
    s.execute("effect.setParent", json!({ "effect": carry, "parent": peg })).unwrap();
    assert_eq!((moved(&s, arm), moved(&s, first), s.history().undo_label()), (30.0, 0.0, Some("Wire a Transform into a Port")));
    // A port in between hands it on too, and a peg under a port is moved by what is above.
    let way_in = s.execute("effect.new", json!({ "kind": "portIn", "group": group })).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.setParent", json!({ "effect": way_in, "parent": down })).unwrap();
    let inner = s.execute("peg.new", json!({ "parent": way_in })).unwrap()["id"].as_u64().unwrap();
    s.execute("peg.setTransform", json!({ "peg": inner, "position": [5.0, 0.0] })).unwrap();
    s.execute("layer.setParent", json!({ "layer": first, "parent": inner })).unwrap();
    assert_eq!(moved(&s, first), 35.0);
    // In the tree of rows, what is under a port or a Receive is under the peg it hands down.
    use iskra_model::{LayerId, Node, PegId};
    let rows = s.project().shots[0].rows();
    let depth = |node: Node| rows.iter().find(|(row, _)| *row == node).map(|(_, depth)| *depth);
    let top = depth(Node::Peg(PegId(peg))).unwrap();
    assert_eq!(
        (depth(Node::Layer(LayerId(arm))), depth(Node::Peg(PegId(inner))), depth(Node::Layer(LayerId(first)))),
        (Some(top + 1), Some(top + 1), Some(top + 2))
    );
    // A picture or a transform, not both; and no ring of transforms, by wire or by name.
    assert!(turned_down(s.execute("effect.connect", json!({ "effect": carry, "node": { "layer": arm } })), "not both"));
    assert!(turned_down(s.execute("effect.setParent", json!({ "effect": port, "parent": peg })), "not both"));
    assert!(turned_down(s.execute("effect.setParent", json!({ "effect": down, "parent": peg })), "no port and no Send"));
    assert!(turned_down(s.execute("peg.setParent", json!({ "peg": peg, "parent": way_in })), "under it"));
    assert!(turned_down(s.execute("effect.setParent", json!({ "effect": carry, "parent": inner })), "under it"));
    // A port that is deleted leaves what it carried to what came into it.
    s.execute("effect.delete", json!({ "effect": way_in })).unwrap();
    assert_eq!((moved(&s, first), s.project().shots[0].peg(iskra_model::PegId(inner)).unwrap().parent), (35.0, Some(iskra_model::PegId(down))));
    // A peg that is deleted leaves the Send with nothing wired in.
    s.execute("peg.delete", json!({ "peg": peg })).unwrap();
    assert_eq!((moved(&s, arm), moved(&s, first), s.project().shots[0].effect(iskra_model::EffectId(carry)).unwrap().parent), (0.0, 5.0, None));
    s.undo();
    assert_eq!(moved(&s, first), 35.0);
    // A copy of the shot has its own Send and Receive, which find each other by the name.
    let original = s.project().shots[0].id.0;
    let copy = s.execute("shot.duplicate", json!({ "shot": original })).unwrap()["id"].as_u64().unwrap();
    let copied = s.project().shot(iskra_model::ShotId(copy)).unwrap().clone();
    assert_eq!(iskra_eval::evaluate(&copied, 0).layers.len(), 2);
    // And its own ports, which hand down the copy's own peg.
    let carried: Vec<f64> = iskra_eval::evaluate(&copied, 0).layers.iter().map(|placed| placed.to_scene.m[4]).collect();
    assert_eq!(carried, [35.0, 30.0]);
    assert!(copied.layers.iter().all(|layer| layer.parent.is_some_and(|parent| copied.peg_above(Some(parent)).is_some())));
}

#[test]
fn a_composite_is_told_how_to_order_what_it_stacks() {
    use iskra_model::Stacking;
    let mut s = Session::new();
    let composite = s.project().shots[0].composites[0].id.0;
    let has = |s: &Session| (s.project().shots[0].composites[0].stacking, s.project().shots[0].composites[0].right_first);
    assert_eq!(has(&s), (Stacking::Depth, false));
    let revision = s.project().shots[0].revision;
    s.execute("composite.setOrdering", json!({ "composite": composite, "ordering": "2d", "rightFirst": true })).unwrap();
    assert_eq!((has(&s), s.history().undo_label()), ((Stacking::Ports, true), Some("Set Composite's Ordering")));
    assert_ne!(s.project().shots[0].revision, revision, "the shot renders otherwise");
    // What is left out stays as it is.
    s.execute("composite.setOrdering", json!({ "composite": composite, "ordering": "3dFlat" })).unwrap();
    assert_eq!(has(&s), (Stacking::DepthFlat, true));
    s.execute("composite.setOrdering", json!({ "composite": composite, "rightFirst": false })).unwrap();
    assert_eq!(has(&s), (Stacking::DepthFlat, false));
    assert!(turned_down(s.execute("composite.setOrdering", json!({ "composite": composite, "ordering": "4d" })), "is one of: 3d, 3dFlat, 2d"));
    assert!(turned_down(s.execute("composite.setOrdering", json!({ "composite": 9999, "ordering": "2d" })), "no composite"));
    s.undo();
    s.undo();
    s.undo();
    assert_eq!(has(&s), (Stacking::Depth, false));
}

#[test]
fn a_stretch_of_frames_is_moved_copied_and_pasted_on_its_layer() {
    let mut s = Session::new();
    let layer = s.project().shots[0].layers[0].id.0;
    for frame in [0, 2, 4] {
        s.execute("drawing.newDrawing", json!({ "layer": layer, "frame": frame, "hold": 2 })).unwrap();
    }
    let shown = |s: &Session| {
        let layer = &s.project().shots[0].layers[0];
        (0..12).map(|f| layer.drawing_at(f).map_or("-", |d| d.name.as_str())).collect::<Vec<_>>().join("")
    };
    assert_eq!(shown(&s), "112233------");
    // Moved, the frames leave empty cells and cover what they land on, as one step.
    s.execute("exposure.move", json!({ "layer": layer, "frame": 1, "length": 2, "to": 4 })).unwrap();
    assert_eq!((shown(&s).as_str(), s.history().undo_label()), ("1--212------", Some("Move Frames")));
    s.execute("edit.undo", json!({})).unwrap();
    // Copied, which is no edit, and pasted further on: holds and empty cells alike.
    let steps = s.history().entries().len();
    let copied = s.execute("exposure.copy", json!({ "layer": layer, "frame": 3, "length": 5 })).unwrap();
    assert_eq!(s.history().entries().len(), steps);
    assert_eq!(copied["cells"].as_array().map(Vec::len), Some(3), "{copied}");
    let pasted = s.execute("exposure.paste", json!({ "layer": layer, "frame": 7, "cells": copied["cells"] })).unwrap();
    assert_eq!((shown(&s).as_str(), pasted["length"].as_u64(), s.history().undo_label()), ("112233-233--", Some(5), Some("Paste Frames")));
    // Another layer has none of these drawings; and cells of no length are no cells.
    let other = s.execute("layer.new", json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(turned_down(s.execute("exposure.paste", json!({ "layer": other, "frame": 0, "cells": copied["cells"] })), "the layer they were copied from"));
    assert!(turned_down(s.execute("exposure.paste", json!({ "layer": layer, "frame": 0, "cells": [] })), "give the `cells`"));
    assert!(turned_down(s.execute("exposure.paste", json!({ "layer": layer, "frame": 0, "cells": [[0, null]] })), "give the `cells`"));
    assert!(turned_down(s.execute("exposure.move", json!({ "layer": layer, "frame": 0, "length": 2, "to": 999_999 })), "numbered below"));
}

#[test]
fn a_shot_has_markers_a_stretch_that_plays_and_pegs_that_fold_and_renders_as_before() {
    let mut s = Session::new();
    let shot = s.project().shots[0].id.0;
    let markers = |s: &Session| s.project().shots[0].markers.iter().map(|marker| (marker.frame, marker.name.clone(), marker.colour)).collect::<Vec<_>>();
    let blue = iskra_model::MARKER_COLOURS[0].1;
    // A marker is put on a frame of the shot, named, coloured and moved, as on a storyboard.
    s.execute("shot.setMarker", json!({ "shot": shot, "frame": 30 })).unwrap();
    s.execute("shot.setMarker", json!({ "frame": 12, "name": " Blink ", "colour": [219, 68, 55] })).unwrap();
    assert_eq!(markers(&s), [(12, "Blink".to_string(), [219, 68, 55]), (30, String::new(), blue)]);
    s.execute("shot.setMarker", json!({ "shot": shot, "frame": 12, "to": 40 })).unwrap();
    assert_eq!(markers(&s), [(30, String::new(), blue), (40, "Blink".to_string(), [219, 68, 55])]);
    assert!(turned_down(s.execute("shot.setMarker", json!({ "shot": shot, "frame": 5, "to": 6 })), "no marker"));
    assert!(turned_down(s.execute("shot.setMarker", json!({ "shot": shot, "frame": 48 })), "has 48 frames"));
    s.execute("shot.removeMarker", json!({ "shot": shot, "frame": 30 })).unwrap();
    assert!(turned_down(s.execute("shot.removeMarker", json!({ "shot": shot, "frame": 30 })), "no marker"));
    assert_eq!((markers(&s).len(), s.history().undo_label()), (1, Some("Remove Marker")));

    // The shot plays whole until it has an in or an out point; each is set by itself.
    let played = |s: &Session| s.project().shots[0].played();
    assert_eq!(played(&s), (0, 48));
    s.execute("shot.setRange", json!({ "shot": shot, "in": 10 })).unwrap();
    s.execute("shot.setRange", json!({ "shot": shot, "out": 29 })).unwrap();
    assert_eq!(played(&s), (10, 30), "the out point's frame plays too");
    s.execute("shot.setRange", json!({ "shot": shot, "out": 5 })).unwrap();
    assert_eq!(played(&s), (0, 6), "an out point before the in point takes it back to the start");
    s.execute("shot.setRange", json!({ "shot": shot, "in": 20 })).unwrap();
    assert_eq!(played(&s), (20, 48));
    // A shot made shorter than its stretch plays what is left of it, or whole.
    s.execute("shot.setDuration", json!({ "shot": shot, "frames": 24 })).unwrap();
    assert_eq!(played(&s), (20, 24));
    s.execute("shot.setDuration", json!({ "shot": shot, "frames": 12 })).unwrap();
    assert_eq!(played(&s), (0, 12));
    assert!(turned_down(s.execute("shot.setRange", json!({ "shot": shot })), "say what to set"));
    s.execute("shot.setRange", json!({ "shot": shot, "clear": true })).unwrap();
    assert_eq!(s.project().shots[0].range, None);

    // A peg's row is folded and opened; a copy of the shot has its own pegs folded.
    let peg = s.execute("peg.new", json!({ "shot": shot })).unwrap()["id"].as_u64().unwrap();
    let after_peg = s.project().shots[0].revision;
    s.execute("peg.setFolded", json!({ "peg": peg, "folded": true })).unwrap();
    s.execute("shot.setMarker", json!({ "shot": shot, "frame": 3 })).unwrap();
    s.execute("shot.setRange", json!({ "shot": shot, "in": 2 })).unwrap();
    assert!(s.project().shots[0].folded.contains(&iskra_model::PegId(peg)));
    assert_eq!(s.project().shots[0].revision, after_peg, "none of this changes what is rendered");
    let copy = s.execute("shot.duplicate", json!({ "shot": shot })).unwrap()["id"].as_u64().unwrap();
    let copy = s.project().shot(ShotId(copy)).unwrap().clone();
    assert_eq!(copy.folded.iter().copied().collect::<Vec<_>>(), [copy.pegs[1].id]);
    assert_eq!((copy.markers.len(), copy.range), (2, Some((2, 12))));
    s.execute("peg.setFolded", json!({ "peg": peg, "folded": false })).unwrap();
    assert!(s.project().shots[0].folded.is_empty());
    assert!(turned_down(s.execute("peg.setFolded", json!({ "peg": 999_999, "folded": true })), "no peg"));

    // A row is hidden and shown: a peg's hides what it carries. It changes what is rendered,
    // and a copy of the shot has its own nodes hidden.
    let layer = s.project().shots[0].layers[0].id;
    s.execute("layer.setParent", json!({ "layer": layer.0, "parent": peg })).unwrap();
    let before = s.project().shots[0].revision;
    s.execute("shot.setHidden", json!({ "node": peg, "hidden": true })).unwrap();
    assert!(s.project().shots[0].hides(layer) && s.project().shots[0].revision != before);
    let copy = s.execute("shot.duplicate", json!({ "shot": shot })).unwrap()["id"].as_u64().unwrap();
    let copy = s.project().shot(ShotId(copy)).unwrap().clone();
    assert_eq!((copy.hidden.iter().copied().collect::<Vec<_>>(), copy.hides(copy.layers[0].id)), (vec![copy.pegs[1].id.0], true));
    s.execute("shot.setHidden", json!({ "node": peg, "hidden": false })).unwrap();
    assert!(!s.project().shots[0].hides(layer) && s.project().shots[0].hidden.is_empty());
    assert!(turned_down(s.execute("shot.setHidden", json!({ "node": 999_999, "hidden": true })), "no layer, peg or group"));
}

#[test]
fn shots_are_rendered_to_image_sequences_each_in_a_folder_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    s.execute("project.setResolution", json!({ "width": 640, "height": 360 })).unwrap();
    s.execute("shot.setDuration", json!({ "frames": 4 })).unwrap();
    let layer = s.project().shots[0].layers[0].id.0;
    s.execute("drawing.stroke", stroke_params(layer, 0, json!({ "size": 6.0 }))).unwrap();
    let second = s.execute("shot.new", json!({})).unwrap()["id"].as_u64().unwrap();
    s.execute("shot.setDuration", json!({ "shot": second, "frames": 2 })).unwrap();
    let first = s.project().shots[0].id.0;

    // Both shots, as they are: PNG, the cut, a folder each.
    let out = dir.path().join("Renders");
    let made = s.execute("export.sequence", json!({ "shots": [first, second], "folder": out })).unwrap();
    let said: Vec<(u64, u64)> =
        made["sequences"].as_array().unwrap().iter().map(|sequence| (sequence["shot"].as_u64().unwrap(), sequence["files"].as_u64().unwrap())).collect();
    assert_eq!(said, [(first, 4), (second, 2)]);
    assert!(out.join("Shot_01").join("Shot_01.0001.png").is_file() && out.join("Shot_01").join("Shot_01.0004.png").is_file());
    assert!(out.join("Shot_02").join("Shot_02.0002.png").is_file() && !out.join("Shot_02").join("Shot_02.0003.png").exists());
    let frame = image::open(out.join("Shot_01").join("Shot_01.0001.png")).unwrap().into_rgba8();
    assert!((frame.width(), frame.height()) == (640, 360) && frame.pixels().any(|pixel| pixel.0[3] > 0), "the stroke is in the picture");
    assert!(!s.history().can_undo() || s.history().undo_label() != Some("Render Shots"), "rendering is no step to undo");

    // The project knows what was rendered, and that a shot was changed since: the first shot
    // drawn on is stale, the second still fresh; what is only said about a shot changes nothing.
    let stale = |s: &Session| s.project().renders.iter().map(|record| (record.what, iskra_export::is_stale(s.project(), record))).collect::<Vec<_>>();
    let (one, two) = (iskra_model::Rendered::Shot(ShotId(first)), iskra_model::Rendered::Shot(ShotId(second)));
    assert_eq!(stale(&s), [(one, false), (two, false)]);
    assert!(s.is_dirty(), "a record is something to save");
    s.execute("shot.setStatus", json!({ "shot": first, "status": "final" })).unwrap();
    assert_eq!(stale(&s), [(one, false), (two, false)]);
    s.execute("drawing.stroke", stroke_params(layer, 2, json!({ "size": 6.0 }))).unwrap();
    assert_eq!(stale(&s), [(one, true), (two, false)]);
    // Undone, the shot is as it was rendered, and the record is still there.
    s.undo();
    assert_eq!(stale(&s), [(one, false), (two, false)]);
    s.redo();
    // Another size of the picture, and every render is of the project as it was.
    s.execute("project.setResolution", json!({ "width": 320, "height": 180 })).unwrap();
    assert_eq!(stale(&s), [(one, true), (two, true)]);
    s.execute("project.setResolution", json!({ "width": 640, "height": 360 })).unwrap();

    // Again, it is refused unless it is said what to do with what is there.
    assert!(turned_down(s.execute("export.sequence", json!({ "shot": first, "folder": out })), "has rendered frames in it already"));
    let next = s.execute("export.sequence", json!({ "shot": first, "folder": out, "ifExists": "version" })).unwrap();
    assert_eq!(next["sequences"][0]["folder"].as_str(), out.join("Shot_01_v002").to_str());
    // Other settings: half the size, as JPEG, all of the shot.
    let settings = json!({ "format": "jpeg", "scale": 0.5, "range": "whole" });
    s.execute("export.sequence", json!({ "shot": first, "folder": dir.path().join("small"), "settings": settings })).unwrap();
    let small = image::open(dir.path().join("small").join("Shot_01").join("Shot_01.0001.jpg")).unwrap();
    assert_eq!((small.width(), small.height(), small.color()), (320, 180, image::ColorType::Rgb8));
    assert!(turned_down(s.execute("export.sequence", json!({ "shot": 999, "folder": out })), "no shot 999"));

    // Movies lie together in the folder, a file a shot; the storyboard is one file of both shots.
    let small = json!({ "format": "mp4", "scale": 0.25 });
    let movies = s.execute("export.sequence", json!({ "shots": [first, second], "folder": out, "settings": small })).unwrap();
    assert_eq!(movies["sequences"][1]["file"].as_str(), out.join("Shot_02.mp4").to_str());
    assert!(out.join("Shot_01.mp4").is_file() && out.join("Shot_02.mp4").is_file());
    let film = s.execute("export.storyboard", json!({ "folder": out, "settings": { "format": "mov", "scale": 0.25 } })).unwrap();
    let length = s.project().storyboards[0].end();
    assert_eq!((film["file"].as_str(), &film["frames"]), (out.join("Storyboard.mov").to_str(), &json!([0, length])), "as long as the cut is");
    assert!(turned_down(s.execute("export.storyboard", json!({ "folder": out, "settings": { "format": "png" } })), "a movie is a MOV or an MP4"));

    // The cut as an edit list beside the movies, which it names: those that were rendered.
    for (format, has) in [("edl", "* SOURCE FILE: "), ("otio", "Timeline.1"), ("fcpxml", "<asset-clip ")] {
        let list = s.execute("export.editList", json!({ "folder": out, "format": format })).unwrap();
        let text = std::fs::read_to_string(list["file"].as_str().unwrap()).unwrap();
        assert!(
            list["file"].as_str() == out.join(format!("Storyboard.{format}")).to_str() && text.contains(has) && text.contains("Shot_01.mp4"),
            "{format}: {text}"
        );
    }
    assert!(turned_down(s.execute("export.editList", json!({ "folder": out, "format": "aaf" })), "invalid parameters"));

    // From a file: a shot that is not loaded is loaded for its render, and let go again.
    let file = dir.path().join("Film.iskra");
    s.execute("file.saveAs", json!({ "path": file })).unwrap();
    let mut again = Session::new();
    again.execute("file.open", json!({ "path": file })).unwrap();
    again.execute("export.sequence", json!({ "shot": first, "folder": dir.path().join("fromfile") })).unwrap();
    assert!(dir.path().join("fromfile").join("Shot_01").join("Shot_01.0001.png").is_file());
    assert!(!again.project().shots[0].loaded);
}

#[test]
fn a_project_is_saved_into_a_folder_of_its_own_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let saved = s.execute("file.saveAs", json!({ "path": dir.path().join("Ball.iskra"), "projectFolder": true })).unwrap();
    let file = dir.path().join("Ball").join("Ball.iskra");
    assert_eq!((saved["path"].as_str(), s.path()), (file.to_str(), Some(file.as_path())));
    assert!(file.is_file() && !dir.path().join("Ball.iskra").exists());
    for folder in ["Images", "Audio", "Palettes", "Library", "Shots", "Renders"] {
        assert!(dir.path().join("Ball").join(folder).is_dir(), "{folder}");
    }
    // Saved again, and saved as itself, it stays where it is.
    s.execute("layer.new", json!({})).unwrap();
    s.execute("file.save", Value::Null).unwrap();
    s.execute("file.saveAs", json!({ "path": file, "projectFolder": true })).unwrap();
    assert_eq!(s.path(), Some(file.as_path()));
    assert!(!dir.path().join("Ball").join("Ball").exists());
    // Under another name it is another folder; without being asked, a file where it is told.
    s.execute("file.saveAs", json!({ "path": dir.path().join("Ball 2.iskra"), "projectFolder": true })).unwrap();
    assert_eq!(s.path(), Some(dir.path().join("Ball 2").join("Ball 2.iskra").as_path()));
    s.execute("file.saveAs", json!({ "path": dir.path().join("Loose.iskra") })).unwrap();
    assert!(dir.path().join("Loose.iskra").is_file() && !dir.path().join("Loose").exists());
}

#[test]
fn save_as_keeps_the_shots_that_were_never_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, [first, second], [e1, e2]) = opened_film(dir.path());
    s.execute("shot.load", json!({ "shot": second })).unwrap();
    s.execute("drawing.stroke", stroke_params(e2, 1, json!({ "size": 20 }))).unwrap();
    let copy = dir.path().join("Copy.iskra");
    s.execute("file.saveAs", json!({ "path": copy })).unwrap();
    assert!(!s.is_dirty() && s.path() == Some(copy.as_path()));

    let mut other = Session::new();
    other.execute("file.open", json!({ "path": copy })).unwrap();
    other.execute("shot.load", json!({ "shot": first })).unwrap();
    other.execute("shot.load", json!({ "shot": second })).unwrap();
    assert_eq!(line_pixel(&other, e1, 0, 250, 100), Some((3, 65535)), "the shot that was only in the old file");
    assert_eq!(line_pixel(&other, e2, 1, 250, 100), Some((3, 65535)));
    // The old file doesn't have the new stroke.
    other.execute("file.open", json!({ "path": dir.path().join("Film.iskra") })).unwrap();
    other.execute("shot.load", json!({ "shot": second })).unwrap();
    assert_eq!(cells(&other, e2, 2), "1 -");
}

#[test]
fn a_project_without_a_file_keeps_every_shot_in_memory() {
    let mut s = Session::new();
    let board = storyboard(&s);
    // A shot with no drawings yet has nothing that differs from a file, but there is no file.
    let empty = id(s.execute("shot.new", json!({ "storyboard": board })).unwrap());
    assert_eq!(s.execute("shot.unload", json!({ "shot": empty })).unwrap(), json!({ "released": false }));
    assert!(is_loaded(&s, empty));
    let e = id(s.execute("layer.new", json!({ "shot": empty })).unwrap());
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20 }))).unwrap();
}

/// The first shot's tree as the timeline lists it: each name after a `>` for every peg above it.
fn tree(s: &Session) -> String {
    let shot = &s.project().shots[0];
    let name = |node| match node {
        iskra_model::Node::Peg(id) => shot.peg(id).unwrap().name.clone(),
        iskra_model::Node::Layer(id) => shot.layer(id).unwrap().name.clone(),
    };
    // Without the peg that moves the camera, which every shot has and these trees are not about.
    let rows = shot.rows().into_iter().filter(|(node, _)| !matches!(node, iskra_model::Node::Peg(id) if shot.camera.parent == Some(*id)));
    rows.map(|(node, depth)| format!("{}{}", ">".repeat(depth), name(node))).collect::<Vec<_>>().join(" ")
}

#[test]
fn pegs_carry_what_is_parented_to_them() {
    let mut s = Session::new();
    let ball = layer(&s);
    s.execute("layer.rename", json!({ "layer": ball, "name": "Ball" })).unwrap();
    let shadow = id(s.execute("layer.new", json!({ "name": "Shadow" })).unwrap());

    // A peg put above a node takes its place and is named after it.
    let ball_peg = id(s.execute("peg.new", json!({ "aboveLayer": ball })).unwrap());
    assert_eq!((tree(&s).as_str(), s.history().undo_label()), ("Shadow Ball-P >Ball", Some("New Peg")));
    let top = id(s.execute("peg.new", json!({ "name": "Bounce" })).unwrap());
    let between = id(s.execute("peg.new", json!({ "abovePeg": ball_peg })).unwrap());
    assert_eq!(tree(&s), "Shadow Ball-P-P >Ball-P >>Ball Bounce");
    s.execute("peg.rename", json!({ "peg": between, "name": "Squash" })).unwrap();
    s.execute("peg.setParent", json!({ "peg": between, "parent": top })).unwrap();
    s.execute("layer.setParent", json!({ "layer": shadow, "parent": top })).unwrap();
    assert_eq!(tree(&s), "Bounce >Shadow >Squash >>Ball-P >>>Ball");

    // The ball is moved by every peg above it; the shadow only by the top one.
    s.execute("peg.setTransform", json!({ "peg": top, "position": [100.0, 0.0] })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Move Peg"));
    s.execute("peg.setTransform", json!({ "peg": ball_peg, "position": [0.0, -40.0], "scale": [2.0, 2.0] })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Transform Peg"));
    let at = |s: &Session, layer: u64| {
        let shot = &s.project().shots[0];
        // Where its origin is in the scene.
        let [.., x, y] = shot.world(shot.layer(iskra_model::LayerId(layer)).unwrap().parent, 0).m;
        (x, y)
    };
    assert_eq!((at(&s, ball), at(&s, shadow)), ((100.0, -40.0), (100.0, 0.0)));
    let steps = s.history().entries().len();
    s.execute("peg.setTransform", json!({ "peg": top, "position": [100.0, 0.0] })).unwrap();
    assert_eq!(s.history().entries().len(), steps, "the same values again change nothing");

    // Deleting a peg hands what it carried to its parent.
    s.execute("peg.delete", json!({ "peg": between })).unwrap();
    assert_eq!(tree(&s), "Bounce >Shadow >Ball-P >>Ball");
    s.execute("layer.setParent", json!({ "layer": shadow })).unwrap();
    s.execute("peg.delete", json!({ "peg": top })).unwrap();
    assert_eq!((tree(&s).as_str(), at(&s, ball)), ("Shadow Ball-P >Ball", (0.0, -40.0)));
    s.undo();
    assert_eq!(tree(&s), "Shadow Bounce >Ball-P >>Ball");
}

#[test]
fn peg_commands_refuse_what_would_break_the_tree() {
    let mut s = Session::new();
    let ball = layer(&s);
    let arm = id(s.execute("peg.new", json!({ "name": "Arm" })).unwrap());
    let hand = id(s.execute("peg.new", json!({ "name": "Hand", "parent": arm })).unwrap());
    let refused = |r: crate::Result<Value>, why: &str| matches!(r, Err(EngineError::Invalid(said)) if said.contains(why));
    let steps = s.history().entries().len();

    assert!(refused(s.execute("peg.setParent", json!({ "peg": arm, "parent": hand })), "under it"), "no rings");
    assert!(refused(s.execute("peg.setParent", json!({ "peg": arm, "parent": arm })), "itself"));
    assert!(refused(s.execute("peg.setParent", json!({ "peg": arm, "parent": 999 })), "no peg 999"));
    assert!(refused(s.execute("layer.setParent", json!({ "layer": ball, "parent": ball })), "no peg"), "a layer is no parent");
    assert!(refused(s.execute("peg.new", json!({ "aboveLayer": ball, "abovePeg": arm })), "not both"));
    assert!(refused(s.execute("peg.new", json!({ "aboveLayer": ball, "parent": arm })), "takes that node's parent"));
    assert!(refused(s.execute("peg.delete", json!({ "peg": 999 })), "no peg 999"));
    // Names are unique among the shot's pegs and layers together.
    assert!(refused(s.execute("peg.new", json!({ "name": "Drawing" })), "already has a layer"));
    assert!(refused(s.execute("peg.rename", json!({ "peg": hand, "name": "Arm" })), "already has a peg"));
    assert!(refused(s.execute("layer.rename", json!({ "layer": ball, "name": "Hand" })), "already has a peg"));
    s.execute("peg.rename", json!({ "peg": hand, "name": "Hand" })).unwrap();
    for bad in [json!({ "scale": [1e9, 1.0] }), json!({ "skew": 90.0 }), json!({ "position": [0.0, f64::MAX] })] {
        let mut params = bad.clone();
        params["peg"] = json!(hand);
        assert!(refused(s.execute("peg.setTransform", params), "outside"), "{bad}");
    }
    assert_eq!(s.history().entries().len(), steps, "and none of it changed the project");

    // A peg of another shot is no parent here.
    let other = id(s.execute("shot.new", Value::Null).unwrap());
    let far = id(s.execute("peg.new", json!({ "shot": other })).unwrap());
    assert!(refused(s.execute("peg.setParent", json!({ "peg": hand, "parent": far })), "has no peg"));
    assert_eq!(s.project().shot(ShotId(other)).unwrap().pegs[1].name, "Peg");
}

#[test]
fn keyframes_animate_a_pegs_transform() {
    let mut s = Session::new();
    let peg = id(s.execute("peg.new", json!({ "aboveLayer": layer(&s) })).unwrap());
    let at = |s: &Session, frame: u32| s.project().shots[0].pegs[1].transform_at(frame);
    let keys = |s: &Session| s.project().shots[0].pegs[1].animation.key_frames().into_iter().collect::<Vec<_>>();
    let refused = |r: crate::Result<Value>, why: &str| matches!(r, Err(EngineError::Invalid(said)) if said.contains(why));

    // Without keys a value is the same on every frame.
    s.execute("peg.setTransform", json!({ "peg": peg, "frame": 10, "position": [50.0, 0.0] })).unwrap();
    assert_eq!((at(&s, 0).position, at(&s, 40).position, keys(&s)), ([50.0, 0.0], [50.0, 0.0], vec![]));

    // A keyframe keys the pose. From then on, setting a value on a frame keys it there.
    s.execute("peg.setKey", json!({ "peg": peg, "frame": 0 })).unwrap();
    assert_eq!((s.history().undo_label(), keys(&s), at(&s, 30).position), (Some("Add Keyframe"), vec![0], [50.0, 0.0]));
    s.execute("peg.setTransform", json!({ "peg": peg, "frame": 20, "position": [250.0, 0.0] })).unwrap();
    assert_eq!((keys(&s), at(&s, 10).position, at(&s, 20).position), (vec![0, 20], [150.0, 0.0], [250.0, 0.0]));
    assert_eq!(at(&s, 10).rotation, 0.0, "the rotation has its one key and is still");
    assert!(refused(s.execute("peg.setTransform", json!({ "peg": peg, "position": [0.0, 0.0] })), "which `frame`"));
    // The pivot is never animated.
    s.execute("peg.setTransform", json!({ "peg": peg, "pivot": [5.0, 5.0] })).unwrap();
    assert_eq!((at(&s, 0).pivot, keys(&s)), ([5.0, 5.0], vec![0, 20]));

    // Eased out of the first key, the peg starts slowly; held, it waits and jumps.
    s.execute("peg.setEase", json!({ "peg": peg, "frame": 0, "out": "ease" })).unwrap();
    assert!(at(&s, 4).position[0] < 75.0, "{:?}", at(&s, 4).position);
    s.execute("peg.setEase", json!({ "peg": peg, "frame": 0, "out": "hold" })).unwrap();
    assert_eq!((at(&s, 19).position, at(&s, 20).position), ([50.0, 0.0], [250.0, 0.0]));
    assert!(refused(s.execute("peg.setEase", json!({ "peg": peg, "frame": 0, "into": "hold" })), "not into"));
    assert!(refused(s.execute("peg.setEase", json!({ "peg": peg, "frame": 7, "out": "ease" })), "no keyframe on frame 7"));
    s.execute("peg.setEase", json!({ "peg": peg, "frame": 0, "out": "linear" })).unwrap();

    // A curve gives a side its own slope; a channel's key can be eased, moved and removed alone.
    s.execute("peg.setEase", json!({ "peg": peg, "frame": 0, "channel": "position", "out": { "curve": { "reach": 0.5, "speed": 2.0 } } })).unwrap();
    assert!(at(&s, 6).position[0] > 125.0, "leaving fast, it is ahead of the steady 110: {:?}", at(&s, 6).position);
    assert!(refused(s.execute("peg.setEase", json!({ "peg": peg, "frame": 0, "out": { "curve": { "reach": 0.0, "speed": 1.0 } } })), "reaches 0.01 to 1"));
    assert!(
        refused(s.execute("peg.setEase", json!({ "peg": peg, "frame": 20, "channel": "rotation", "out": "ease" })), "no keyframe"),
        "only the position is keyed on 20"
    );
    s.execute("peg.setEase", json!({ "peg": peg, "frame": 0, "out": "linear" })).unwrap();
    s.execute("peg.moveKey", json!({ "peg": peg, "from": 0, "to": 3, "channel": "rotation" })).unwrap();
    assert_eq!(keys(&s), [0, 3, 20]);
    s.execute("peg.removeKey", json!({ "peg": peg, "frame": 3, "channel": "rotation" })).unwrap();
    s.execute("peg.setKey", json!({ "peg": peg, "frame": 0 })).unwrap();
    assert_eq!(keys(&s), [0, 20]);

    // The position goes along a path, which can be bent at a key or made a corner there.
    s.execute("peg.setTransform", json!({ "peg": peg, "frame": 10, "position": [150.0, 100.0] })).unwrap();
    let off_line = |s: &Session| (at(s, 5).position[1] - at(s, 5).position[0] + 50.0).abs();
    assert!(off_line(&s) > 3.0, "smooth through the middle key, the path leaves the straight line: {:?}", at(&s, 5).position);
    s.execute("peg.setBend", json!({ "peg": peg, "frame": 10, "into": [0.0, 0.0], "out": [0.0, 0.0] })).unwrap();
    assert!(off_line(&s) < 1e-6 && s.history().undo_label() == Some("Bend Motion Path"));
    s.execute("peg.setBend", json!({ "peg": peg, "frame": 10, "out": [0.0, 60.0] })).unwrap();
    assert!(off_line(&s) < 1e-6 && at(&s, 11).position[1] > 100.0, "only the way out is bent");
    s.execute("peg.setBend", json!({ "peg": peg, "frame": 10, "smooth": true })).unwrap();
    assert!(off_line(&s) > 3.0);
    assert!(refused(s.execute("peg.setBend", json!({ "peg": peg, "frame": 11, "smooth": true })), "no keyframe on frame 11"));
    assert!(refused(s.execute("peg.setBend", json!({ "peg": peg, "frame": 10, "smooth": true, "out": [1.0, 1.0] })), "no handles of its own"));
    s.execute("peg.removeKey", json!({ "peg": peg, "frame": 10 })).unwrap();

    // A key moves in time, and comes off; the last one off leaves the pose it held.
    s.execute("peg.moveKey", json!({ "peg": peg, "from": 20, "to": 40 })).unwrap();
    assert_eq!((keys(&s), at(&s, 20).position), (vec![0, 40], [150.0, 0.0]));
    assert!(refused(s.execute("peg.moveKey", json!({ "peg": peg, "from": 20, "to": 30 })), "no keyframe"));
    s.execute("peg.removeKey", json!({ "peg": peg, "frame": 0 })).unwrap();
    assert_eq!((keys(&s), at(&s, 0).position), (vec![40], [250.0, 0.0]));
    s.execute("peg.removeKey", json!({ "peg": peg, "frame": 40 })).unwrap();
    assert_eq!((keys(&s), at(&s, 0).position, at(&s, 0).rotation), (vec![], [250.0, 0.0], 0.0));
    assert!(refused(s.execute("peg.removeKey", json!({ "peg": peg, "frame": 40 })), "no keyframe"));
    s.undo();
    assert_eq!(keys(&s), [40]);
}

/// The first shot's layers as they are seen, top first.
fn stacked(s: &Session) -> String {
    let shot = &s.project().shots[0];
    shot.stack().iter().map(|id| shot.layer(*id).unwrap().name.clone()).collect::<Vec<_>>().join(" ")
}

#[test]
fn composites_stack_what_is_connected_and_the_display_shows_one_node() {
    let mut s = Session::new();
    let sky = layer(&s);
    s.execute("layer.rename", json!({ "layer": sky, "name": "Sky" })).unwrap();
    let body = id(s.execute("layer.new", json!({ "name": "Body" })).unwrap());
    let hat = id(s.execute("layer.new", json!({ "name": "Hat" })).unwrap());
    let all = s.project().shots[0].composites[0].id.0;
    assert_eq!((stacked(&s).as_str(), s.project().shots[0].display), ("Hat Body Sky", Some(iskra_model::Source::Composite(iskra_model::CompositeId(all)))));

    // The character gets a composite of its own, which goes where its layers were.
    let character = id(s.execute("composite.new", json!({ "name": "Character" })).unwrap());
    for layer in [body, hat] {
        s.execute("node.disconnect", json!({ "node": { "layer": layer }, "from": all })).unwrap();
        s.execute("node.connect", json!({ "node": { "layer": layer }, "to": character })).unwrap();
    }
    assert_eq!(stacked(&s), "Sky", "what is connected to nothing the display shows is not seen");
    s.execute("node.connect", json!({ "node": { "composite": character }, "to": all })).unwrap();
    assert_eq!((stacked(&s).as_str(), s.history().undo_label()), ("Hat Body Sky", Some("Connect Node")));
    // Connecting again moves it: under the sky. And a layer moves among its own composite's.
    s.execute("node.connect", json!({ "node": { "composite": character }, "to": all, "index": 1 })).unwrap();
    assert_eq!(stacked(&s), "Sky Hat Body");
    s.execute("layer.move", json!({ "layer": hat, "index": 1 })).unwrap();
    assert_eq!(stacked(&s), "Sky Body Hat");

    // The display can show the character alone, a single layer, or nothing.
    s.execute("display.set", json!({ "node": { "composite": character } })).unwrap();
    assert_eq!(stacked(&s), "Body Hat");
    s.execute("display.set", json!({ "node": { "layer": sky } })).unwrap();
    assert_eq!(stacked(&s), "Sky");
    s.execute("display.set", Value::Null).unwrap();
    assert_eq!(stacked(&s), "");
    s.execute("display.set", json!({ "node": { "composite": all } })).unwrap();

    // A new layer goes on top of what is on display; a deleted one leaves the graph.
    let prop = id(s.execute("layer.new", json!({ "name": "Prop" })).unwrap());
    assert_eq!(stacked(&s), "Prop Sky Body Hat");
    s.execute("layer.delete", json!({ "layer": prop })).unwrap();
    // Deleting a composite leaves what it stacked in the shot, seen no more.
    s.execute("composite.delete", json!({ "composite": character })).unwrap();
    assert_eq!((stacked(&s).as_str(), s.project().shots[0].layers.len()), ("Sky", 3));
    s.undo();
    assert_eq!(stacked(&s), "Sky Body Hat");
}

#[test]
fn a_shot_has_a_main_display_and_others_beside_it_for_the_viewer() {
    use iskra_model::Source;
    let mut s = Session::new();
    let sky = layer(&s);
    s.execute("layer.rename", json!({ "layer": sky, "name": "Sky" })).unwrap();
    let body = id(s.execute("layer.new", json!({ "name": "Body" })).unwrap());
    let shot = |s: &Session| (*s.project().shots[0]).clone();
    let main = s.project().shots[0].main_display();
    assert_eq!((shot(&s).display_name.as_str(), shot(&s).displays.len()), ("Display", 0), "a shot is made with its main display");
    // Another display shows the body alone; what the shot shows is the main one's still.
    let matte = id(s.execute("display.new", json!({ "node": { "layer": body }, "at": [10.0, 20.0] })).unwrap());
    let other = iskra_model::DisplayId(matte);
    assert_eq!(
        (shot(&s).displays[0].name.as_str(), shot(&s).shown_by(other), shot(&s).places.get(&matte)),
        ("Display 2", Some(Some(Source::Layer(iskra_model::LayerId(body)))), Some(&[10.0, 20.0]))
    );
    assert_eq!((stacked(&s).as_str(), s.history().undo_label()), ("Body Sky", Some("New Display")));
    assert_eq!(shot(&s).seen_through(other).unwrap().stack(), [iskra_model::LayerId(body)]);
    s.execute("display.rename", json!({ "display": matte, "name": "Matte" })).unwrap();
    s.execute("display.set", json!({ "display": matte, "node": { "layer": sky } })).unwrap();
    assert_eq!(shot(&s).displays[0].shows, Some(Source::Layer(iskra_model::LayerId(sky))));
    // Made the main one, the shot shows the sky; the main one before it is the other now.
    s.execute("display.setMain", json!({ "display": matte })).unwrap();
    assert_eq!((stacked(&s).as_str(), shot(&s).display_name.as_str(), shot(&s).displays[0].name.as_str()), ("Sky", "Matte", "Display"));
    s.undo();
    assert_eq!(stacked(&s), "Body Sky");
    // A layer it shows that is deleted leaves it showing nothing; the main one can't be deleted.
    s.execute("layer.delete", json!({ "layer": sky })).unwrap();
    assert_eq!(shot(&s).displays[0].shows, None);
    assert!(turned_down(s.execute("display.delete", json!({ "display": main.0 })), "make another one the main one"));
    assert!(turned_down(s.execute("display.rename", json!({ "display": matte, "name": "Display" })), "main display is named"));
    assert!(turned_down(s.execute("display.new", json!({ "name": "Body" })), "already has a layer"));
    s.execute("display.delete", json!({ "display": matte })).unwrap();
    assert!(shot(&s).displays.is_empty() && !shot(&s).places.contains_key(&matte));
    // The main one is renamed, and shows what the old command makes it show.
    s.execute("display.rename", json!({ "display": main.0, "name": "Final" })).unwrap();
    s.execute("display.set", Value::Null).unwrap();
    assert_eq!((shot(&s).display_name.as_str(), shot(&s).display), ("Final", None));
}

#[test]
fn graph_commands_refuse_rings_and_strangers() {
    let mut s = Session::new();
    let all = s.project().shots[0].composites[0].id.0;
    let inner = id(s.execute("composite.new", Value::Null).unwrap());
    assert_eq!(s.project().shots[0].composites[1].name, "Composite 2");
    let refused = |r: crate::Result<Value>, why: &str| matches!(r, Err(EngineError::Invalid(said)) if said.contains(why));
    s.execute("node.connect", json!({ "node": { "composite": inner }, "to": all })).unwrap();
    let steps = s.history().entries().len();

    assert!(refused(s.execute("node.connect", json!({ "node": { "composite": all }, "to": inner })), "gets its picture from"), "no rings");
    assert!(refused(s.execute("node.connect", json!({ "node": { "composite": all }, "to": all })), "to itself"));
    assert!(refused(s.execute("node.connect", json!({ "node": { "layer": 999 }, "to": all })), "no such node"));
    assert!(refused(s.execute("node.connect", json!({ "node": { "composite": inner }, "to": 999 })), "no composite 999"));
    assert!(refused(s.execute("node.connect", json!({ "node": { "layer": layer(&s) }, "to": inner, "index": 5 })), "nodes connected"));
    assert!(refused(s.execute("display.set", json!({ "node": { "composite": 999 } })), "no such node"));
    assert!(refused(s.execute("composite.rename", json!({ "composite": inner, "name": "Drawing" })), "already has a layer"));
    assert!(refused(s.execute("layer.rename", json!({ "layer": layer(&s), "name": "Composite" })), "already has a composite"));
    assert!(refused(s.execute("composite.delete", json!({ "composite": 999 })), "no composite"));
    assert_eq!(s.history().entries().len(), steps, "and none of it changed the project");

    // A node's place in the node view is kept by its id, the display's by the shot's, and goes
    // with the node when it is deleted.
    let shot = s.project().shots[0].id.0;
    s.execute("node.place", json!({ "node": inner, "at": [40.0, -120.0] })).unwrap();
    s.execute("node.place", json!({ "node": shot, "at": [0.0, 200.0] })).unwrap();
    assert_eq!((s.project().shots[0].places.get(&inner), s.history().undo_label()), (Some(&[40.0, -120.0]), Some("Move Node")));
    assert!(refused(s.execute("node.place", json!({ "node": 999, "at": [0.0, 0.0] })), "no node 999"));
    // Several are put in one step.
    let steps = s.history().entries().len();
    s.execute("node.place", json!({ "moves": [{ "node": inner, "at": [10.0, 20.0] }, { "node": shot, "at": [30.0, 40.0] }] })).unwrap();
    assert_eq!((s.project().shots[0].places.get(&inner), s.project().shots[0].places.get(&shot)), (Some(&[10.0, 20.0]), Some(&[30.0, 40.0])));
    assert_eq!(s.history().entries().len(), steps + 1);
    assert!(refused(s.execute("node.place", json!({ "node": inner })), "or several as `moves`"));
    s.undo();
    let spare = id(s.execute("composite.new", Value::Null).unwrap());
    s.execute("node.place", json!({ "node": spare, "at": [1.0, 1.0] })).unwrap();
    s.execute("composite.delete", json!({ "composite": spare })).unwrap();
    assert_eq!(s.project().shots[0].places.keys().copied().collect::<Vec<_>>(), [shot, inner]);

    // A node of another shot is a stranger here; a new shot has a composite on display.
    let other = id(s.execute("shot.new", Value::Null).unwrap());
    let theirs = s.project().shot(ShotId(other)).unwrap().composites[0].id.0;
    assert!(refused(s.execute("node.connect", json!({ "node": { "composite": theirs }, "to": all })), "no such node"));
    let far = id(s.execute("layer.new", json!({ "shot": other })).unwrap());
    assert_eq!(s.project().shot(ShotId(other)).unwrap().stack(), [iskra_model::LayerId(far)]);
    s.execute("composite.rename", json!({ "composite": inner, "name": "Character" })).unwrap();
    // A layer connected to no composite is neither over nor under another.
    s.execute("node.disconnect", json!({ "node": { "layer": far } })).unwrap();
    assert!(refused(s.execute("layer.move", json!({ "layer": far, "index": 0 })), "not connected"));
}

/// Whether a command was refused, with `why` in what it said.
#[test]
fn a_bone_is_taken_out_of_a_chain_and_the_others_keep_their_poses() {
    let mut s = Session::new();
    let layer = s.project().shots[0].layers[0].id.0;
    let bones = json!([{ "angle": 0, "length": 100 }, { "angle": 0, "length": 100 }, { "angle": 90, "length": 50 }]);
    let made = s.execute("deformer.new", json!({ "layer": layer, "origin": [10, 20], "bones": bones })).unwrap()["id"].as_u64().unwrap();
    s.execute("deformer.setPose", json!({ "deformer": made, "frame": 0, "bone": 2, "turn": 25 })).unwrap();
    s.execute("deformer.setKey", json!({ "deformer": made, "frame": 4 })).unwrap();
    let chain = |s: &Session| s.project().shots[0].deformers[0].clone();
    let ends = chain(&s).joints(None);
    // The joint between the first two: two bones, from the start to where the second ended,
    // and the last as it was, with its pose and its key.
    s.execute("deformer.removeBone", json!({ "deformer": made, "joint": 1 })).unwrap();
    let (less, now) = (chain(&s), chain(&s).joints(None));
    assert_eq!((less.bones.len(), less.bones[1].turns.keys().len(), s.history().undo_label()), (2, 1, Some("Remove Bone")));
    assert!((now[1].x - ends[2].x).abs() < 1e-6 && (now[2].y - ends[3].y).abs() < 1e-6 && (less.bones[0].length - 200.0).abs() < 1e-6, "{now:?}");
    s.execute("deformer.removeBone", json!({ "deformer": made, "joint": 2 })).unwrap();
    assert_eq!(chain(&s).bones.len(), 1);
    assert!(turned_down(s.execute("deformer.removeBone", json!({ "deformer": made, "joint": 0 })), "has one bone"));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(turned_down(s.execute("deformer.removeBone", json!({ "deformer": made, "joint": 9 })), "no joint 9"));
}

fn turned_down(result: crate::Result<Value>, why: &str) -> bool {
    matches!(result, Err(e) if e.to_string().contains(why))
}

/// A session whose one layer is `Arm`, with a stroke on frame 0, under the keyed peg
/// `Shoulder`: the layer's id and the peg's.
fn arm() -> (Session, u64, u64) {
    let mut s = Session::new();
    let arm = swatch_layer(&mut s);
    s.execute("layer.rename", json!({ "layer": arm, "name": "Arm" })).unwrap();
    s.execute("drawing.stroke", stroke_params(arm, 0, json!({ "size": 20 }))).unwrap();
    let shoulder = id(s.execute("peg.new", json!({ "aboveLayer": arm, "name": "Shoulder" })).unwrap());
    s.execute("peg.setTransform", json!({ "peg": shoulder, "position": [30.0, 0.0] })).unwrap();
    s.execute("peg.setKey", json!({ "peg": shoulder, "frame": 4 })).unwrap();
    (s, arm, shoulder)
}

#[test]
fn nodes_are_copied_pasted_and_duplicated_as_things_of_their_own() {
    let (mut s, arm, shoulder) = arm();
    assert!(turned_down(s.execute("node.paste", json!({})), "nothing to paste"));
    assert!(turned_down(s.execute("node.copy", json!({ "nodes": [{ "peg": 999_999 }] })), "no such nodes"));
    let steps = s.history().entries().len();
    // A peg is copied alone, without what it carries.
    assert_eq!(s.execute("node.copy", json!({ "nodes": [{ "peg": shoulder }] })).unwrap()["copied"].as_u64(), Some(1));
    assert_eq!(s.history().entries().len(), steps, "copying changes nothing of the project");
    s.execute("node.place", json!({ "moves": [{ "node": shoulder, "at": [100.0, -200.0] }, { "node": arm, "at": [100.0, -100.0] }] })).unwrap();
    s.execute("node.copy", json!({ "nodes": [{ "peg": shoulder }, { "layer": arm }] })).unwrap();

    // Pasted: the peg and the layer it carries, again, under new ids and names, wired to
    // each other and to nothing else, with the drawing and the animation, and beside where
    // the first are.
    let pasted = s.execute("node.paste", json!({})).unwrap();
    let (layer_id, peg_id) = (pasted["layers"][0].as_u64().unwrap(), pasted["pegs"][0].as_u64().unwrap());
    assert_eq!(s.history().undo_label(), Some("Paste Nodes"));
    let shot = s.project().shots[0].clone();
    let (copy, peg) = (shot.layer(iskra_model::LayerId(layer_id)).unwrap(), shot.peg(iskra_model::PegId(peg_id)).unwrap());
    assert_eq!((copy.name.as_str(), peg.name.as_str(), copy.parent, &copy.template), ("Arm 2", "Shoulder 2", Some(peg.id), &None));
    assert_eq!((shot.stack().contains(&copy.id), shot.places[&peg_id]), (false, [140.0, -160.0]));
    assert_eq!(peg.animation, shot.peg(iskra_model::PegId(shoulder)).unwrap().animation);
    assert_eq!(line_pixel(&s, layer_id, 0, 250, 100), line_pixel(&s, arm, 0, 250, 100));
    assert!(line_pixel(&s, layer_id, 0, 250, 100).is_some_and(|(_, coverage)| coverage > 0));
    // A thing of its own: drawn on, the one it was copied from is as it was.
    let lower: Vec<Value> = (0..=30).map(|i| json!({ "x": 100 + i * 10, "y": 300, "pressure": 1.0 })).collect();
    s.execute("drawing.stroke", json!({ "layer": layer_id, "frame": 0, "swatch": 3, "stroke": { "brush": { "size": 20 }, "points": lower } })).unwrap();
    let painted = |layer: u64| line_pixel(&s, layer, 0, 250, 300).is_some_and(|(_, coverage)| coverage > 0);
    assert!(painted(layer_id) && !painted(arm));
    s.undo();
    s.undo();
    assert_eq!(s.project().shots[0].layers.len(), shot.layers.len() - 1, "one step");

    // What was copied can be pasted again, and at a place of the node view.
    let again = s.execute("node.paste", json!({ "at": [500.0, 0.0] })).unwrap();
    let places = s.project().shots[0].places.clone();
    let (a, b) = (places[&again["pegs"][0].as_u64().unwrap()], places[&again["layers"][0].as_u64().unwrap()]);
    assert_eq!(((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0), (500.0, 0.0));
    s.undo();

    // Duplicated: the same in one step, and what was copied last is still what is pasted.
    // The peg pasted alone carries nothing.
    let twin = s.execute("node.duplicate", json!({ "nodes": [{ "layer": arm }] })).unwrap();
    assert_eq!((twin["layers"].as_array().unwrap().len(), twin["pegs"].as_array().unwrap().len(), s.history().undo_label()), (1, 0, Some("Duplicate Nodes")));
    // A layer goes alone, and its copy is under the peg that carries the first.
    let copy = iskra_model::LayerId(twin["layers"][0].as_u64().unwrap());
    assert_eq!(s.project().shots[0].layer(copy).unwrap().parent, Some(iskra_model::PegId(shoulder)));
    let pasted = s.execute("node.paste", json!({})).unwrap();
    assert_eq!((pasted["pegs"].as_array().unwrap().len(), pasted["layers"].as_array().unwrap().len()), (1, 1));
    s.execute("node.copy", json!({ "nodes": [{ "peg": shoulder }] })).unwrap();
    assert_eq!(s.execute("node.paste", json!({})).unwrap()["layers"].as_array().unwrap().len(), 0);
    // A composite copied alone stacks nothing: what it stacked stayed behind.
    let character = id(s.execute("composite.new", json!({ "name": "Character" })).unwrap());
    s.execute("node.connect", json!({ "node": { "layer": arm }, "to": character })).unwrap();
    s.execute("node.copy", json!({ "nodes": [{ "composite": character }] })).unwrap();
    let pasted = s.execute("node.paste", json!({})).unwrap();
    let copy = iskra_model::CompositeId(pasted["composites"][0].as_u64().unwrap());
    assert!(pasted["layers"].as_array().unwrap().is_empty() && s.project().shots[0].composite(copy).unwrap().inputs.is_empty());
}

#[test]
fn nodes_are_pasted_into_the_group_asked_for_or_beside_their_own() {
    use iskra_model::{GroupId, LayerId, PegId};
    let (mut s, arm, shoulder) = arm();
    let rig = id(s.execute("group.new", json!({ "nodes": [shoulder, arm], "name": "Rig" })).unwrap());
    let other = s.execute("layer.new", json!({})).unwrap()["id"].as_u64().unwrap();
    let apart = id(s.execute("group.new", json!({ "nodes": [other], "name": "Apart" })).unwrap());
    s.execute("node.copy", json!({ "nodes": [{ "peg": shoulder }, { "layer": arm }] })).unwrap();
    let group_of = |s: &Session, node: u64| s.project().shots[0].group_of(node).map(|group| group.id.0);
    // Without a place, beside the first and in their group.
    let pasted = s.execute("node.paste", json!({})).unwrap();
    let (layer, peg) = (pasted["layers"][0].as_u64().unwrap(), pasted["pegs"][0].as_u64().unwrap());
    assert_eq!((group_of(&s, layer), group_of(&s, peg)), (Some(rig), Some(rig)));
    s.undo();
    // At a place, in the group asked for, and wired only to each other.
    let pasted = s.execute("node.paste", json!({ "at": [300.0, 0.0], "group": apart })).unwrap();
    let (layer, peg) = (pasted["layers"][0].as_u64().unwrap(), pasted["pegs"][0].as_u64().unwrap());
    assert_eq!((group_of(&s, layer), group_of(&s, peg)), (Some(apart), Some(apart)));
    let shot = s.project().shots[0].clone();
    assert_eq!(shot.layer(LayerId(layer)).unwrap().parent, Some(PegId(peg)));
    assert!(!shot.stack().contains(&LayerId(layer)));
    s.undo();
    // At a place outside every group, in none.
    let pasted = s.execute("node.paste", json!({ "at": [300.0, 0.0] })).unwrap();
    assert_eq!(group_of(&s, pasted["pegs"][0].as_u64().unwrap()), None);
    s.undo();
    // A group copied comes whole, in the group asked for.
    s.execute("node.copy", json!({ "nodes": [{ "group": rig }] })).unwrap();
    let pasted = s.execute("node.paste", json!({ "at": [0.0, 300.0], "group": apart })).unwrap();
    let copy = pasted["groups"][0].as_u64().unwrap();
    assert_eq!(group_of(&s, copy), Some(apart));
    assert!(s.project().shots[0].group(GroupId(copy)).unwrap().members.contains(&pasted["pegs"][0].as_u64().unwrap()));
    assert!(turned_down(s.execute("node.paste", json!({ "at": [0.0, 0.0], "group": 99_999 })), "no group"));
}

#[test]
fn nodes_are_saved_as_a_template_file_and_put_into_a_shot_as_copies() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, arm, shoulder) = arm();
    let steps = s.history().entries().len();
    let saved =
        s.execute("library.save", json!({ "folder": dir.path(), "name": "Arm", "nodes": [{ "peg": shoulder }], "tags": ["limb", " "], "notes": " Left. " }));
    let path = saved.unwrap()["path"].clone();
    assert_eq!(path, json!(dir.path().join("Arm.iskratpl")));
    assert_eq!(s.history().entries().len(), steps, "saving a template changes nothing of the project");

    // The folder lists it with what it says about itself, and it has a picture of itself.
    let listed = s.execute("library.list", json!({ "folder": dir.path() })).unwrap();
    let [item] = &listed["items"].as_array().unwrap()[..] else { panic!("one template: {listed}") };
    assert_eq!(
        (&item["name"], &item["path"], &item["source"], &item["tags"], &item["notes"]),
        (&json!("Arm"), &path, &json!("Untitled"), &json!(["limb"]), &json!("Left."))
    );
    assert!(item["saved"].as_u64().unwrap() > 1_700_000_000);
    let preview = iskra_store::read_template_preview(std::path::Path::new(path.as_str().unwrap())).unwrap().expect("a picture");
    let (width, height, pixels) = iskra_io::decode_png(&preview).unwrap();
    // The stroke is about 320 by 20 drawing pixels: 160 by 10 of the scene, framed to its width.
    assert!(width == 192 && (8..=20).contains(&height), "{width} × {height}");
    assert_eq!(pixels[(height / 2 * width + width / 2) as usize], [220, 50, 47, 255], "the stroke, in its swatch's colour");
    assert_eq!(pixels[0][3], 0, "and nothing around its round end");

    // Into another project: copies with ids of that project, on top of what its shot shows.
    let mut other = Session::new();
    let theirs = layer(&other);
    let placed = other.execute("library.place", json!({ "path": path })).unwrap();
    let (layer_id, peg_id) = (placed["layers"][0].as_u64().unwrap(), placed["pegs"][0].as_u64().unwrap());
    assert_eq!(
        (placed["layers"].as_array().unwrap().len(), placed["pegs"].as_array().unwrap().len(), &placed["composites"], &placed["skipped"]),
        (1, 1, &json!([]), &json!([]))
    );
    let shot = other.project().shots[0].clone();
    let (copy, peg) = (shot.layer(iskra_model::LayerId(layer_id)).unwrap(), shot.peg(iskra_model::PegId(peg_id)).unwrap());
    assert_eq!((copy.name.as_str(), peg.name.as_str(), copy.parent, copy.template.as_deref()), ("Arm", "Shoulder", Some(peg.id), Some("Arm")));
    assert_eq!(shot.stack(), [copy.id, iskra_model::LayerId(theirs)]);
    assert_eq!(line_pixel(&other, layer_id, 0, 250, 100), Some((3, 65535)), "with its drawings");
    assert_eq!((peg.animation.key_frames().into_iter().collect::<Vec<_>>(), peg.transform_at(4).position), (vec![4], [30.0, 0.0]), "and its animation");
    assert!(shot.places.is_empty() && shot.composites.len() == 1);
    // It is one step.
    assert_eq!(other.history().undo_label(), Some("Place Template"));
    other.undo();
    assert_eq!((other.project().shots[0].layers.len(), other.project().shots[0].pegs.len()), (1, 1), "the camera's peg alone");
    assert!(!other.history().can_undo());

    // Into the shot it came from, at a place of the node view: the names are taken.
    s.execute("node.place", json!({ "node": arm, "at": [0.0, -90.0] })).unwrap();
    s.execute("library.save", json!({ "folder": dir.path(), "name": "Arm", "nodes": [{ "peg": shoulder }], "replace": true })).unwrap();
    let placed = s.execute("library.place", json!({ "path": path, "at": [400.0, -300.0] })).unwrap();
    let shot = s.project().shots[0].clone();
    let copy = shot.layer(iskra_model::LayerId(placed["layers"][0].as_u64().unwrap())).unwrap();
    assert_eq!(shot.node_names().collect::<Vec<_>>(), ["Arm 2", "Arm", "Camera-P", "Shoulder", "Shoulder 2", "Composite"]);
    assert_eq!(shot.places.get(&copy.id.0), Some(&[400.0, -300.0]));
    // The copy's drawing is its own from the first stroke on it.
    s.execute("drawing.stroke", stroke_params(copy.id.0, 0, json!({ "size": 60, "erase": true, "pressureSize": false }))).unwrap();
    assert_eq!((line_pixel(&s, copy.id.0, 0, 250, 100), line_pixel(&s, arm, 0, 250, 100)), (Some((0, 0)), Some((3, 65535))));
}

#[test]
fn the_library_is_folders_of_files_that_are_listed_renamed_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, arm, shoulder) = arm();
    let save = |s: &mut Session, name: &str, extra: Value| {
        let mut params = json!({ "folder": dir.path(), "name": name, "nodes": [{ "layer": arm }] });
        params.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        s.execute("library.save", params)
    };
    let props = s.execute("library.newFolder", json!({ "folder": dir.path(), "name": "Props" })).unwrap()["path"].clone();
    save(&mut s, "hand", json!({})).unwrap();
    save(&mut s, "Arm", json!({})).unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not a template").unwrap();
    std::fs::write(dir.path().join("Broken.iskratpl"), "not a template either, though it says so").unwrap();
    let names = |s: &mut Session| {
        let listed = s.execute("library.list", json!({ "folder": dir.path() })).unwrap();
        let of = |kind: &str| listed[kind].as_array().unwrap().iter().map(|entry| entry["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        (of("folders"), of("items"))
    };
    assert_eq!(
        names(&mut s),
        (vec!["Props".to_string()], vec!["Arm".to_string(), "Broken".to_string(), "hand".to_string()]),
        "by name; other files are not listed"
    );
    let listed = s.execute("library.list", json!({ "folder": dir.path() })).unwrap();
    assert!(listed["items"][1]["problem"].as_str().is_some_and(|why| why.contains("damaged")), "{listed}");
    assert_eq!(s.execute("library.list", json!({ "folder": props })).unwrap(), json!({ "folders": [], "items": [] }));

    // A template is not written over unless that is asked for, and a layer alone has no peg.
    assert!(turned_down(save(&mut s, "Arm", json!({})), "already has a template named `Arm`"));
    save(&mut s, "Arm", json!({ "replace": true })).unwrap();
    let (template, _) = iskra_store::read_template(&dir.path().join("Arm.iskratpl")).unwrap();
    assert!(template.project.shots[0].pegs.is_empty() && template.project.shots[0].layers[0].parent.is_none());

    let hand = dir.path().join("hand.iskratpl");
    assert!(turned_down(s.execute("library.rename", json!({ "path": hand, "name": "Arm" })), "already has a template"));
    let renamed = s.execute("library.rename", json!({ "path": hand, "name": "Hand" })).unwrap()["path"].clone();
    assert_eq!(renamed, json!(dir.path().join("Hand.iskratpl")));
    s.execute("library.delete", json!({ "path": dir.path().join("Broken.iskratpl") })).unwrap();
    assert_eq!(names(&mut s).1, ["Arm", "Hand"]);

    // What is refused: names that are no file names, nodes the shot lacks, files that are no
    // templates. None of it is a step.
    let steps = s.history().entries().len();
    for name in ["", "a/b", "a:b", ".hidden"] {
        assert!(matches!(save(&mut s, name, json!({})), Err(EngineError::Invalid(_))), "{name}");
    }
    assert!(turned_down(s.execute("library.save", json!({ "folder": dir.path(), "name": "x", "nodes": [] })), "at least one node"));
    assert!(turned_down(s.execute("library.save", json!({ "folder": dir.path(), "name": "x", "nodes": [{ "peg": 999 }] })), "no such node"));
    let nowhere = dir.path().join("nowhere");
    assert!(turned_down(s.execute("library.save", json!({ "folder": nowhere, "name": "x", "nodes": [{ "peg": shoulder }] })), "not a folder"));
    assert!(s.execute("library.list", json!({ "folder": nowhere })).is_err());
    assert!(turned_down(s.execute("library.delete", json!({ "path": dir.path().join("notes.txt") })), "not a template file"));
    assert!(turned_down(s.execute("library.place", json!({ "path": dir.path().join("Gone.iskratpl") })), "not a template file"));
    let project = dir.path().join("Film.iskra");
    s.execute("file.saveAs", json!({ "path": project })).unwrap();
    std::fs::copy(&project, dir.path().join("Film.iskratpl")).unwrap();
    assert!(turned_down(s.execute("library.place", json!({ "path": dir.path().join("Film.iskratpl") })), "not a library template"));
    assert!(turned_down(s.execute("library.place", json!({ "path": renamed, "shot": 999 })), "no shot 999"));
    assert_eq!(s.history().entries().len(), steps);
    // A template saved from a project with a file says which.
    save(&mut s, "Named", json!({})).unwrap();
    assert_eq!(iskra_store::read_template_info(&dir.path().join("Named.iskratpl")).unwrap().source, "Film");
}

#[test]
fn shots_are_renamed_together_as_a_text_and_a_number_in_their_order() {
    let mut s = Session::new();
    let board = s.project().storyboards[0].id.0;
    let first = s.project().shots[0].id.0;
    let second = id(s.execute("shot.new", json!({ "storyboard": board })).unwrap());
    let spare = id(s.execute("shot.new", json!({ "name": "Spare" })).unwrap());
    // The first goes after the second on the storyboard: the order is the cut's, not the ids'.
    s.execute("storyboard.moveClip", json!({ "shot": first, "start": 500 })).unwrap();
    let names = |s: &Session| s.project().shots.iter().map(|shot| (shot.id.0, shot.name.clone())).collect::<std::collections::BTreeMap<_, _>>();
    let order = s.project().in_cut_order(&[ShotId(spare), ShotId(first), ShotId(second)]);
    assert_eq!(order, [ShotId(second), ShotId(first), ShotId(spare)]);
    let said = s.execute("shot.renameNumbered", json!({ "shots": [spare, first, second], "text": "SH", "first": 10, "step": 10, "digits": 3 })).unwrap();
    assert_eq!(said["names"].as_array().unwrap().len(), 3);
    assert_eq!(s.history().undo_label(), Some("Rename Shots"));
    let named = names(&s);
    assert_eq!(
        (named[&order[0].0].as_str(), named[&order[1].0].as_str(), named[&spare].as_str()),
        ("SH010", "SH020", "SH030"),
        "the one on no storyboard last"
    );

    // Renamed again they may take each other's names; another shot's is refused.
    s.execute("shot.renameNumbered", json!({ "shots": [first, second], "text": "SH", "first": 0, "step": 10, "digits": 3 })).unwrap();
    assert_eq!((names(&s)[&second].as_str(), names(&s)[&first].as_str()), ("SH000", "SH010"));
    assert!(turned_down(
        s.execute("shot.renameNumbered", json!({ "shots": [first, second], "text": "SH", "first": 30, "step": 10, "digits": 3 })),
        "already has a shot named `SH030`"
    ));
    assert!(turned_down(s.execute("shot.renameNumbered", json!({ "shots": [first], "text": "A/", "first": 1, "step": 1, "digits": 1 })), "can't contain `/`"));
    assert!(turned_down(s.execute("shot.renameNumbered", json!({ "shots": [] })), "name the shots"));
    // Without a text or padding, the names are numbers.
    s.execute("shot.renameNumbered", json!({ "shots": [first], "text": "", "first": 7, "step": 1, "digits": 0 })).unwrap();
    assert_eq!(names(&s)[&first], "7");
    s.execute("edit.undo", Value::Null).unwrap();
    assert!(names(&s)[&first].starts_with("SH0"));
}

#[test]
fn shots_are_given_scenes_and_sequences_together_and_renamed_with_them() {
    let dir = tempfile::tempdir().unwrap();
    let film = dir.path().join("Film.iskra");
    let mut s = Session::new();
    let storyboard = |s: &Session| s.project().storyboards[0].id.0;
    let first = s.project().shots[0].id.0;
    let second = id(s.execute("shot.new", json!({ "storyboard": storyboard(&s) })).unwrap());
    let third = id(s.execute("shot.new", json!({ "storyboard": storyboard(&s) })).unwrap());
    let of = |s: &Session, shot: u64| s.project().shot(ShotId(shot)).map(|shot| (shot.scene.clone(), shot.sequence.clone())).unwrap();
    let named = |scene: &str, sequence: &str| (scene.to_string(), sequence.to_string());
    assert_eq!(of(&s, first), named("", ""), "a new shot is in none");

    // Two shots are given both at once, as one step; the third only a sequence.
    s.execute("shot.setScene", json!({ "shots": [first, second], "scene": " SC010 ", "sequence": "SQ01" })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Set Scene and Sequence"));
    s.execute("shot.setScene", json!({ "shots": [third], "sequence": "SQ01" })).unwrap();
    assert_eq!((of(&s, first), of(&s, second), of(&s, third)), (named("SC010", "SQ01"), named("SC010", "SQ01"), named("", "SQ01")));
    assert_eq!((s.project().scenes(), s.project().sequences()), (vec!["SC010".to_string()], vec!["SQ01".to_string()]));
    s.execute("edit.undo", Value::Null).unwrap();
    assert_eq!(of(&s, third), named("", ""));
    s.execute("shot.setScene", json!({ "shots": [third], "scene": "SC020", "sequence": "SQ01" })).unwrap();
    // A scene is let go by giving none; the sequence is left as it is.
    s.execute("shot.setScene", json!({ "shots": [second], "scene": "" })).unwrap();
    assert_eq!(of(&s, second), named("", "SQ01"));
    assert!(turned_down(s.execute("shot.setScene", json!({ "shots": [first] })), "say the scene or the sequence"));
    assert!(turned_down(s.execute("shot.setScene", json!({ "shots": [], "scene": "A" })), "name the shots"));
    assert!(turned_down(s.execute("shot.setScene", json!({ "shots": [first], "scene": "A/B" })), "can't contain `/`"));
    assert!(turned_down(s.execute("shot.setScene", json!({ "shots": [999], "scene": "A" })), "no shot 999"));
    assert_eq!(of(&s, first), named("SC010", "SQ01"), "a step refused changes nothing");
    let revision = s.project().shot(ShotId(first)).unwrap().revision;
    s.execute("shot.setScene", json!({ "shots": [first], "scene": "SC030" })).unwrap();
    s.execute("shot.setScene", json!({ "shots": [first], "scene": "SC010" })).unwrap();
    assert_eq!(s.project().shot(ShotId(first)).unwrap().revision, revision, "what a shot is given to does not change its render");

    // A sequence renamed is renamed on all its shots; a scene renamed to one in use joins it.
    s.execute("shot.renameScene", json!({ "from": "SQ01", "name": "Chase", "sequence": true })).unwrap();
    assert_eq!(s.project().sequences(), ["Chase"]);
    s.execute("shot.renameScene", json!({ "from": "SC020", "name": "SC010" })).unwrap();
    assert_eq!((of(&s, first), of(&s, third)), (named("SC010", "Chase"), named("SC010", "Chase")));
    assert!(turned_down(s.execute("shot.renameScene", json!({ "from": "SC020", "name": "X" })), "no shot is in the scene `SC020`"));
    assert!(turned_down(s.execute("shot.renameScene", json!({ "from": "SC010", "name": " " })), "can't be empty"));

    // They are kept in the file.
    s.execute("file.saveAs", json!({ "path": film })).unwrap();
    let mut again = Session::new();
    again.execute("file.open", json!({ "path": film })).unwrap();
    assert_eq!((of(&again, first), of(&again, second)), (named("SC010", "Chase"), named("", "Chase")));
}

#[test]
fn shots_go_out_as_files_of_their_own_and_come_back_in_one_step() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let film = dir.path().join("Film.iskra");
    let mut s = Session::new();
    let ours = swatch_layer(&mut s);
    s.execute("drawing.stroke", stroke_params(ours, 0, json!({ "size": 20 }))).unwrap();
    let second = id(s.execute("shot.new", json!({ "storyboard": storyboard(&s) })).unwrap());
    s.execute("file.saveAs", json!({ "path": film })).unwrap();
    let first = s.project().shots[0].id.0;

    // Both shots go out, each as a project file of that one shot, and are marked.
    let sent = s.execute("shot.sendOut", json!({ "shots": [first, second], "folder": out, "to": " Ana " })).unwrap();
    assert_eq!(sent["files"], json!([out.join("Shot_01.iskra"), out.join("Shot_02.iskra")]));
    let status = |s: &Session, shot: u64| s.project().shot(ShotId(shot)).map(|shot| (shot.status, shot.out_to.clone())).unwrap();
    assert_eq!((status(&s, first), status(&s, second)), ((Status::Out, Some("Ana".to_string())), (Status::Out, Some("Ana".to_string()))));
    assert_eq!(s.history().undo_label(), Some("Send Shots Out"));
    assert!(turned_down(s.execute("shot.sendOut", json!({ "shots": [first], "folder": out })), "already has a file named `Shot_01.iskra`"));

    // Whoever has the first shot opens its file as a project: one shot, with its drawing.
    let mut theirs = Session::new();
    theirs.execute("file.open", json!({ "path": out.join("Shot_01.iskra") })).unwrap();
    theirs.execute("shot.load", Value::Null).unwrap();
    let project = theirs.project().clone();
    assert_eq!((project.shots.len(), project.shots[0].id.0, project.shots[0].status, project.storyboards[0].clips().len()), (1, first, Status::Animation, 1));
    assert_eq!(line_pixel(&theirs, ours, 0, 250, 100), Some((3, 65535)));
    // They draw on another frame, add a layer, make the shot longer, and save.
    theirs.execute("drawing.stroke", stroke_params(ours, 6, json!({ "size": 20 }))).unwrap();
    let added = id(theirs.execute("layer.new", json!({ "name": "Shadow" })).unwrap());
    theirs.execute("shot.setDuration", json!({ "frames": 60 })).unwrap();
    theirs.execute("file.save", Value::Null).unwrap();
    drop(theirs);

    // Meanwhile the project goes on, and gives the same ids to other things.
    let meanwhile = id(s.execute("layer.new", json!({ "shot": second, "name": "Notes" })).unwrap());
    assert!(meanwhile < added, "an id the file has given to something of its own");
    s.execute("file.save", Value::Null).unwrap();
    let steps = s.history().entries().len();

    // A look at what would change, which changes nothing.
    let back = out.join("Shot_01.iskra");
    let preview = s.execute("shot.bringBack", json!({ "path": back, "preview": true })).unwrap();
    assert_eq!((&preview["shot"], &preview["name"], &preview["to"], &preview["skipped"]), (&json!(first), &json!("Shot_01"), &json!("Ana"), &json!([])));
    assert_eq!(
        preview["differences"],
        json!([
            "It is 60 frames long, where it was 48 frames.",
            "A new layer, “Shadow”, with 0 drawings.",
            "The layer “Drawing”: 1 drawing new, the timing changed.",
            "What is seen, or which layer is over which, has changed.",
        ])
    );
    assert_eq!((s.history().entries().len(), status(&s, first).0), (steps, Status::Out));

    // Brought back, it takes its shot's place under new ids; the cut is as it was.
    let before = cut(&s);
    s.execute("shot.bringBack", json!({ "path": back })).unwrap();
    let shot = s.project().shot(ShotId(first)).unwrap().clone();
    assert_eq!((status(&s, first), shot.duration, s.history().undo_label()), ((Status::Animation, None), 60, Some("Bring Shot Back")));
    assert_eq!(shot.layers.iter().map(|layer| layer.name.as_str()).collect::<Vec<_>>(), ["Shadow", "Drawing"]);
    assert!(shot.layers.iter().all(|layer| layer.id.0 > meanwhile) && cut(&s) == before, "ids the project has not given out");
    let drawn = shot.layers[1].id.0;
    assert_eq!((line_pixel(&s, drawn, 0, 250, 100), line_pixel(&s, drawn, 6, 250, 100)), (Some((3, 65535)), Some((3, 65535))));
    assert_eq!(
        s.project().layer(iskra_model::LayerId(meanwhile)).map(|layer| layer.name.as_str()),
        Some("Notes"),
        "what the project made meanwhile is its own"
    );
    // It is saved and read again like any shot, and undo brings the version it replaced.
    s.execute("file.save", Value::Null).unwrap();
    let mut again = Session::new();
    again.execute("file.open", json!({ "path": film })).unwrap();
    again.execute("shot.load", json!({ "shot": first })).unwrap();
    assert_eq!(again.project().shot(ShotId(first)), s.project().shot(ShotId(first)));
    s.undo();
    assert_eq!(
        (status(&s, first).0, s.project().shot(ShotId(first)).unwrap().duration, line_pixel(&s, ours, 0, 250, 100)),
        (Status::Out, 48, Some((3, 65535)))
    );
}

#[test]
fn a_returned_file_finds_its_shot_and_a_status_is_set_by_hand() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let first = s.project().shots[0].id.0;
    let second = id(s.execute("shot.new", json!({ "name": "Chase: 2" })).unwrap());
    let status = |s: &Session, shot: u64| s.project().shot(ShotId(shot)).map(|shot| (shot.status, shot.out_to.clone())).unwrap();
    assert_eq!(status(&s, first), (Status::Board, None), "a new shot is a board");
    s.execute("shot.setStatus", json!({ "shot": second, "status": "out", "to": "Ben" })).unwrap();
    assert_eq!((status(&s, second), s.history().undo_label()), ((Status::Out, Some("Ben".to_string())), Some("Set Shot Status")));
    s.execute("shot.setStatus", json!({ "shot": second, "status": "post", "to": "Ben" })).unwrap();
    assert_eq!(status(&s, second), (Status::Post, None), "only a shot that is out is with someone");
    assert!(matches!(s.execute("shot.setStatus", json!({ "status": "lost" })), Err(EngineError::BadParams { .. })));

    // A name that is no file's name is written as one.
    let sent = s.execute("shot.sendOut", json!({ "shots": [second], "folder": dir.path() })).unwrap();
    let file = dir.path().join("Chase_ 2.iskra");
    assert_eq!((&sent["files"], status(&s, second)), (&json!([file]), (Status::Out, None)));
    s.execute("shot.sendOut", json!({ "shots": [second], "folder": dir.path(), "replace": true })).unwrap();
    // Renamed here meanwhile, the shot is still the one the file came from.
    s.execute("shot.rename", json!({ "shot": second, "name": "Chase B" })).unwrap();
    let found = s.execute("shot.bringBack", json!({ "path": file, "preview": true })).unwrap();
    assert_eq!((&found["shot"], &found["name"], &found["differences"]), (&json!(second), &json!("Chase B"), &json!([])));
    // It can be told to replace another shot, which keeps its own name.
    s.execute("shot.bringBack", json!({ "path": file, "shot": first })).unwrap();
    assert_eq!((s.project().shots[0].name.as_str(), status(&s, first).0, s.project().shots[0].layers.len()), ("Shot_01", Status::Animation, 0));

    // A file saved under another name no longer says where it is from: its shot's name does.
    let mut theirs = Session::new();
    theirs.execute("file.open", json!({ "path": file })).unwrap();
    let copy = dir.path().join("returned.iskra");
    theirs.execute("file.saveAs", json!({ "path": copy })).unwrap();
    drop(theirs);
    assert!(turned_down(s.execute("shot.bringBack", json!({ "path": copy })), "no shot of this project is `Chase: 2`"));
    s.execute("shot.rename", json!({ "shot": second, "name": "Chase: 2" })).unwrap();
    assert_eq!(s.execute("shot.bringBack", json!({ "path": copy, "preview": true })).unwrap()["shot"], json!(second));

    let steps = s.history().entries().len();
    assert!(turned_down(s.execute("shot.sendOut", json!({ "shots": [], "folder": dir.path() })), "name the shots"));
    assert!(turned_down(s.execute("shot.sendOut", json!({ "shots": [999], "folder": dir.path() })), "no shot 999"));
    assert!(turned_down(s.execute("shot.sendOut", json!({ "shots": [first, first], "folder": dir.path() })), "named twice"));
    assert!(turned_down(s.execute("shot.sendOut", json!({ "shots": [first], "folder": dir.path().join("nowhere") })), "not a folder"));
    assert!(turned_down(s.execute("shot.bringBack", json!({ "path": copy, "shot": 999 })), "no shot 999"));
    assert!(s.execute("shot.bringBack", json!({ "path": dir.path().join("none.iskra") })).is_err());
    // The whole film is no returned shot: it holds more than one.
    let film = dir.path().join("Film.iskra");
    s.execute("file.saveAs", json!({ "path": film })).unwrap();
    assert!(turned_down(s.execute("shot.bringBack", json!({ "path": film })), "the project's own file"));
    std::fs::copy(&film, dir.path().join("Other.iskra")).unwrap();
    assert!(turned_down(s.execute("shot.bringBack", json!({ "path": dir.path().join("Other.iskra") })), "holds 2 shots"));
    assert_eq!(s.history().entries().len(), steps, "and none of it changed the project");
}

#[test]
fn a_pegs_depth_puts_what_it_carries_over_or_under_the_rest_of_the_shot() {
    let mut s = Session::new();
    let back = layer(&s);
    let front = id(s.execute("layer.new", json!({ "name": "Front" })).unwrap());
    let peg = id(s.execute("peg.new", json!({ "aboveLayer": front })).unwrap());
    let drawn = |s: &Session, frame: u32| iskra_eval::evaluate(&s.project().shots[0], frame).layers.iter().map(|placed| placed.layer.0).collect::<Vec<_>>();
    assert_eq!(drawn(&s, 0), [back, front], "as the composite stacks them");
    // Moved away from the viewer, the top layer is drawn first: under the other.
    s.execute("peg.setTransform", json!({ "peg": peg, "depth": -0.5 })).unwrap();
    assert_eq!((drawn(&s, 0), s.history().undo_label()), (vec![front, back], Some("Move Peg in Depth")));
    assert_eq!(s.project().shots[0].stack(), [iskra_model::LayerId(front), iskra_model::LayerId(back)], "the graph is as it was");
    // Depth is keyed like the other values, on its own or with them.
    s.execute("peg.setKey", json!({ "peg": peg, "frame": 0 })).unwrap();
    s.execute("peg.setTransform", json!({ "peg": peg, "frame": 10, "depth": 0.5 })).unwrap();
    assert_eq!((drawn(&s, 2), drawn(&s, 9)), (vec![front, back], vec![back, front]), "it comes forward past the other layer");
    s.execute("peg.removeKey", json!({ "peg": peg, "frame": 10, "channel": "depth" })).unwrap();
    assert_eq!(s.project().shots[0].pegs[1].transform_at(10).depth, -0.5);
    assert!(turned_down(s.execute("peg.setTransform", json!({ "peg": peg, "frame": 0, "depth": 1e9 })), "depth is outside"));
}

#[test]
fn a_peg_moved_with_animate_on_is_keyed_and_with_it_off_takes_its_keys_along() {
    let mut s = Session::new();
    let layer = layer(&s);
    let peg = id(s.execute("peg.new", json!({ "aboveLayer": layer })).unwrap());
    let own = |s: &Session| s.project().shots[0].peg(iskra_model::PegId(peg)).unwrap().clone();
    let frames = |s: &Session| own(s).animation.frames(None).into_iter().collect::<Vec<_>>();
    let at = |s: &Session, frame: u32| own(s).transform_at(frame);
    // Off, a value with no keys is set for every frame, as it is where nothing is said.
    s.execute("peg.setTransform", json!({ "peg": peg, "frame": 5, "position": [10.0, 0.0], "animate": false })).unwrap();
    assert_eq!((frames(&s), at(&s, 0).position), (vec![], [10.0, 0.0]));
    // On, it is keyed on the frame, the first key of it; the next move, elsewhere, animates it.
    s.execute("peg.setTransform", json!({ "peg": peg, "frame": 5, "position": [20.0, 0.0], "animate": true })).unwrap();
    assert_eq!((frames(&s), at(&s, 0).position, at(&s, 0).rotation), (vec![5], [20.0, 0.0], 0.0), "the position alone is keyed");
    s.execute("peg.setTransform", json!({ "peg": peg, "frame": 15, "position": [40.0, 0.0], "animate": true })).unwrap();
    assert_eq!((frames(&s), at(&s, 10).position), (vec![5, 15], [30.0, 0.0]));
    // Off again, between the keys: no key there, and both keys go as far as the value did.
    s.execute("peg.setTransform", json!({ "peg": peg, "frame": 10, "position": [30.0, 50.0], "rotation": 45.0, "animate": false })).unwrap();
    assert_eq!((frames(&s), at(&s, 5).position, at(&s, 15).position), (vec![5, 15], [20.0, 50.0], [40.0, 50.0]));
    assert_eq!((at(&s, 10).position, at(&s, 0).rotation), ([30.0, 50.0], 45.0), "what had no keys is set for every frame");
    assert!(turned_down(s.execute("peg.setTransform", json!({ "peg": peg, "position": [0.0, 0.0], "animate": true })), "which `frame`"));
    // The lens goes by the same switch.
    s.execute("camera.setLens", json!({ "frame": 5, "zoom": 2.0, "animate": true })).unwrap();
    s.execute("camera.setLens", json!({ "frame": 15, "zoom": 4.0, "animate": true })).unwrap();
    s.execute("camera.setLens", json!({ "frame": 10, "zoom": 2.0, "animate": false })).unwrap();
    let camera = &s.project().shots[0].camera;
    assert_eq!((camera.zoom_at(5), camera.zoom_at(15), camera.animation.frames().len()), (1.0, 3.0, 2));
}

#[test]
fn a_change_keyed_with_adjust_moves_the_keys_on_its_frames_alike() {
    let mut s = Session::new();
    let layer = layer(&s);
    let peg = id(s.execute("peg.new", json!({ "aboveLayer": layer })).unwrap());
    let at = |s: &Session, frame: u32| s.project().shots[0].peg(iskra_model::PegId(peg)).unwrap().transform_at(frame);
    for (frame, position) in [(0, [100.0, 100.0]), (10, [0.0, 0.0]), (20, [50.0, 0.0])] {
        s.execute("peg.setTransform", json!({ "peg": peg, "frame": frame, "position": position, "rotation": 0.0, "animate": true })).unwrap();
    }
    // The key on 0 is moved 20 to the right: the one on 10 goes as far, the one on 20 stays.
    s.execute("peg.setTransform", json!({ "peg": peg, "frame": 0, "position": [120.0, 100.0], "rotation": 30.0, "animate": true, "adjust": [0, 10] })).unwrap();
    assert_eq!((at(&s, 0).position, at(&s, 10).position, at(&s, 20).position), ([120.0, 100.0], [20.0, 0.0], [50.0, 0.0]));
    assert_eq!((at(&s, 0).rotation, at(&s, 10).rotation, at(&s, 20).rotation), (30.0, 30.0, 0.0));
    // A frame with no key is keyed, and the keys of the others move as far as it did.
    let (before, was) = (at(&s, 15).position, [at(&s, 10).position, at(&s, 20).position]);
    s.execute("peg.setTransform", json!({ "peg": peg, "frame": 15, "position": [35.0, 10.0], "adjust": [10, 20] })).unwrap();
    let by = [35.0 - before[0], 10.0 - before[1]];
    assert_eq!(at(&s, 15).position, [35.0, 10.0]);
    for (frame, was) in [10, 20].into_iter().zip(was) {
        let now = at(&s, frame).position;
        assert!((now[0] - was[0] - by[0]).abs() < 1e-9 && (now[1] - was[1] - by[1]).abs() < 1e-9, "{frame}: {now:?}");
    }
    // The lens goes alike.
    for (frame, zoom) in [(0, 1.0), (10, 2.0)] {
        s.execute("camera.setLens", json!({ "frame": frame, "zoom": zoom, "animate": true })).unwrap();
    }
    s.execute("camera.setLens", json!({ "frame": 0, "zoom": 1.5, "animate": true, "adjust": [10] })).unwrap();
    assert_eq!(s.project().shots[0].camera.zoom_at(10), 2.5);
    // An effect node's number: the key on 0 lowered by 20 takes the one on 10 as far.
    let node = id(s.execute("effect.new", json!({ "kind": "transparency", "on": { "layer": layer } })).unwrap());
    let effect = iskra_model::EffectId(node);
    let amount = |s: &Session, frame: u32| s.project().shots[0].effect(effect).unwrap().number("amount", frame);
    s.execute("effect.setKey", json!({ "effect": node, "frame": 0 })).unwrap();
    s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": 80, "frame": 10 })).unwrap();
    let first = amount(&s, 0);
    s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": first - 20.0, "frame": 0, "adjust": [10] })).unwrap();
    assert_eq!((amount(&s, 0), amount(&s, 10)), (first - 20.0, 60.0));
}

#[test]
fn a_curve_points_handles_posed_with_adjust_move_the_keys_on_its_frames_alike() {
    let mut s = Session::new();
    let tail = layer(&s);
    s.execute("drawing.stroke", stroke_params(tail, 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
    let made = id(s.execute("deformer.new", json!({ "layer": tail, "curve": [[50, 50], [125, 50], [200, 50]] })).unwrap());
    let curve = |s: &Session| s.project().shots[0].deformers[0].clone();
    // The middle point's handles keyed on 0 and 10.
    s.execute("deformer.setKey", json!({ "deformer": made, "frame": 0 })).unwrap();
    s.execute("deformer.setPose", json!({ "deformer": made, "frame": 10, "knot": 1, "out": [30, 40] })).unwrap();
    let out = |s: &Session, frame: u32| curve(s).spine[1].bends_out.key(frame).expect("a key").value;
    let (on_0, on_10) = (out(&s, 0), out(&s, 10));
    // Posed again on 10 with the key on 0 picked: it moves by as much, and the into side too.
    let into = |s: &Session, frame: u32| curve(s).spine[1].bends_into.key(frame).expect("a key").value;
    let (into_0, into_10) = (into(&s, 0), into(&s, 10));
    s.execute("deformer.setPose", json!({ "deformer": made, "frame": 10, "knot": 1, "out": [50, 10], "adjust": [0] })).unwrap();
    let (by, by_into) = ([out(&s, 10)[0] - on_10[0], out(&s, 10)[1] - on_10[1]], [into(&s, 10)[0] - into_10[0], into(&s, 10)[1] - into_10[1]]);
    assert!(by[0].abs() > 1.0, "the handle was posed: {by:?}");
    let moved = [out(&s, 0)[0] - on_0[0], out(&s, 0)[1] - on_0[1]];
    let moved_into = [into(&s, 0)[0] - into_0[0], into(&s, 0)[1] - into_0[1]];
    assert!((moved[0] - by[0]).abs() < 1e-9 && (moved[1] - by[1]).abs() < 1e-9, "{moved:?} {by:?}");
    assert!((moved_into[0] - by_into[0]).abs() < 1e-9 && (moved_into[1] - by_into[1]).abs() < 1e-9, "{moved_into:?} {by_into:?}");
}

#[test]
fn a_pass_is_a_node_of_its_own_that_is_rendered_apart_by_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let sky = layer(&s);
    s.execute("layer.rename", json!({ "layer": sky, "name": "Sky" })).unwrap();
    let body = id(s.execute("layer.new", json!({ "name": "Body" })).unwrap());
    let before = stacked(&s);

    // A Pass shows what flows into it, and the shot's picture is as it was.
    let made = s.execute("display.new", json!({ "name": "Character", "node": { "layer": body }, "pass": true })).unwrap();
    let character = id(made);
    let back = id(s.execute("display.new", json!({ "node": { "layer": sky }, "pass": true })).unwrap());
    s.execute("display.rename", json!({ "display": back, "name": "Background" })).unwrap();
    let look = id(s.execute("display.new", json!({ "node": { "layer": sky } })).unwrap());
    let shot = &s.project().shots[0];
    assert_eq!(stacked(&s), before);
    assert_eq!(shot.passes().map(|pass| pass.name.as_str()).collect::<Vec<_>>(), ["Character", "Background"]);
    assert_eq!(shot.looks().map(|display| display.id.0).collect::<Vec<_>>(), [look], "a display is no pass");
    assert_eq!(shot.pass_named("Character").and_then(|pass| pass.shows), Some(iskra_model::Source::Layer(iskra_model::LayerId(body))));
    // Without a name it is a Pass; it is never the main display.
    assert_eq!(s.project().shots[0].display_name_of(iskra_model::DisplayId(back)), Some("Background"));
    assert!(turned_down(s.execute("display.setMain", json!({ "display": character })), "a pass can't be the main display"));
    let unnamed = id(s.execute("display.new", json!({ "pass": true })).unwrap());
    assert_eq!(s.project().shots[0].display_name_of(iskra_model::DisplayId(unnamed)), Some("Pass"));
    s.execute("display.delete", json!({ "display": unnamed })).unwrap();

    // Renamed in every shot that has it at once, as one step; refused where a node has the name.
    let second = id(s.execute("shot.duplicate", json!({ "shot": s.project().shots[0].id.0 })).unwrap());
    let renamed = s.execute("pass.rename", json!({ "from": "Character", "to": "Hero" })).unwrap();
    let names = |s: &Session| s.project().shots.iter().map(|shot| shot.pass_named("Hero").is_some()).collect::<Vec<_>>();
    assert_eq!((renamed["renamed"].clone(), names(&s), s.history().undo_label()), (json!(2), vec![true, true], Some("Rename Pass")));
    s.undo();
    assert!(s.project().shots.iter().all(|shot| shot.pass_named("Character").is_some()));
    assert!(turned_down(s.execute("pass.rename", json!({ "from": "Character", "to": "Sky" })), "already has a layer named `Sky`"));
    assert!(turned_down(s.execute("pass.rename", json!({ "from": "Smoke", "to": "Fog" })), "no shot has a pass named `Smoke`"));
    s.execute("shot.delete", json!({ "shot": second })).unwrap();

    // Rendered apart, each pass is in a folder of its name under the shot's.
    s.execute("drawing.stroke", stroke_params(body, 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
    let out = dir.path().join("out");
    let small = json!({ "scale": 0.25, "range": "whole" });
    let pick = json!({ "pass": "Character", "named": "Character" });
    let made = s.execute("export.sequence", json!({ "folder": out, "settings": small, "pick": pick })).unwrap();
    let folder = out.join("Shot_01").join("Character");
    assert_eq!(made["sequences"][0]["folder"], json!(folder));
    let (width, _, pixels) = iskra_io::decode_png(&std::fs::read(folder.join("Shot_01_Character.0001.png")).unwrap()).unwrap();
    assert_eq!(pixels[(147 * width + 270) as usize], [220, 50, 47, 255], "the body's stroke is in its pass");
    let back = json!({ "pass": "Background", "named": "Background" });
    s.execute("export.sequence", json!({ "folder": out, "settings": small, "pick": back })).unwrap();
    let (_, _, pixels) = iskra_io::decode_png(&std::fs::read(out.join("Shot_01/Background/Shot_01_Background.0001.png")).unwrap()).unwrap();
    assert!(pixels.iter().all(|pixel| pixel[3] == 0), "and in no other");
    // Each is recorded under its pass's name; the whole picture beside them, as Final.
    s.execute("export.sequence", json!({ "folder": out, "settings": small, "pick": { "named": "Final" } })).unwrap();
    assert!(out.join("Shot_01/Final/Shot_01_Final.0001.png").exists());
    let recorded: Vec<Option<&str>> = s.project().renders.iter().map(|record| record.pass.as_deref()).collect();
    assert_eq!(recorded, [Some("Character"), Some("Background"), Some("Final")]);
}

#[test]
fn the_way_between_keyframes_is_held_in_steps_of_one_two_or_three_frames() {
    let mut s = Session::new();
    let layer = layer(&s);
    let arm = id(s.execute("peg.new", json!({ "aboveLayer": layer, "name": "Arm" })).unwrap());
    // Every value keyed on both frames; the turn alone changes.
    s.execute("peg.setKey", json!({ "peg": arm, "frame": 0 })).unwrap();
    s.execute("peg.setKey", json!({ "peg": arm, "frame": 12 })).unwrap();
    s.execute("peg.setTransform", json!({ "peg": arm, "frame": 12, "rotation": 120.0, "animate": true })).unwrap();
    let skew = |s: &Session| s.project().shots[0].peg(iskra_model::PegId(arm)).unwrap().animation.skew.keys().iter().map(|key| key.frame).collect::<Vec<_>>();
    let turned = |s: &Session, frame: u32| s.project().shots[0].peg(iskra_model::PegId(arm)).unwrap().transform_at(frame).rotation;
    let rotation =
        |s: &Session| s.project().shots[0].peg(iskra_model::PegId(arm)).unwrap().animation.rotation.keys().iter().map(|key| key.frame).collect::<Vec<_>>();
    assert!((turned(&s, 3) - 30.0).abs() < 1e-9, "a steady turn to begin with");
    // On twos: a key every second frame, each as the turn was there, held to the next.
    let picked = json!([{ "owner": { "peg": arm }, "frame": 0 }, { "owner": { "peg": arm }, "frame": 12 }]);
    let stepped = s.execute("keys.step", json!({ "keys": picked, "every": 2 })).unwrap();
    assert_eq!((stepped["stepped"].as_u64(), s.history().undo_label()), (Some(1), Some("Hold Keyframes in Steps")));
    assert_eq!(rotation(&s), [0, 2, 4, 6, 8, 10, 12]);
    assert_eq!(skew(&s), [0, 2, 4, 6, 8, 10, 12], "a value that stays the same is held in steps too, as it has keys");
    assert_eq!([turned(&s, 2), turned(&s, 3), turned(&s, 11), turned(&s, 12)], [20.0, 20.0, 100.0, 120.0]);
    s.undo();
    assert_eq!(rotation(&s), [0, 12], "one step");
    // One key picked: from it to the next. On threes.
    s.execute("keys.step", json!({ "keys": [{ "owner": { "peg": arm }, "frame": 0 }], "every": 3 })).unwrap();
    assert_eq!((rotation(&s), turned(&s, 5)), (vec![0, 3, 6, 9, 12], 30.0));
    // The last key has none after it to step towards; a step of 0 is no step.
    assert!(turned_down(s.execute("keys.step", json!({ "keys": [{ "owner": { "peg": arm }, "frame": 12 }], "every": 2 })), "no other keyframe"));
    assert!(turned_down(s.execute("keys.step", json!({ "keys": picked, "every": 0 })), "1 or more"));
}

#[test]
fn the_way_between_two_keyframes_is_timed_by_a_chart_of_its_in_betweens() {
    let mut s = Session::new();
    let layer = layer(&s);
    let arm = id(s.execute("peg.new", json!({ "aboveLayer": layer, "name": "Arm" })).unwrap());
    // Every value keyed on 0 and 6; it turns 60 degrees and moves 60 across, steadily.
    s.execute("peg.setKey", json!({ "peg": arm, "frame": 0 })).unwrap();
    s.execute("peg.setKey", json!({ "peg": arm, "frame": 6 })).unwrap();
    s.execute("peg.setTransform", json!({ "peg": arm, "frame": 6, "rotation": 60.0, "position": [60.0, 0.0], "animate": true })).unwrap();
    let at = |s: &Session, frame: u32| s.project().shots[0].peg(iskra_model::PegId(arm)).unwrap().transform_at(frame);
    let owner = json!({ "peg": arm });
    // On the folded row: every value alike, as one undo step.
    let chart = [0.05, 0.1, 0.2, 0.6, 0.9];
    let timed = s.execute("keys.chart", json!({ "owner": owner, "from": 0, "to": 6, "spacing": chart })).unwrap();
    assert!(timed["timed"].as_u64().unwrap() >= 2);
    assert_eq!(s.history().undo_label(), Some("Space In-betweens"));
    let (turn, across) = (at(&s, 3).rotation, at(&s, 3).position[0]);
    assert!((turn - 12.0).abs() < 1e-6 && (across - 12.0).abs() < 1e-6, "{turn} {across}");
    // Even: the chart taken away, steady again.
    s.execute("keys.chart", json!({ "owner": owner, "from": 0, "to": 6, "spacing": [] })).unwrap();
    assert!((at(&s, 3).rotation - 30.0).abs() < 1e-6);
    // One value's row alone: the turn is charted, the place goes on steadily.
    let timed = s.execute("keys.chart", json!({ "owner": owner, "value": "rotation", "from": 0, "to": 6, "spacing": chart })).unwrap();
    assert_eq!(timed["timed"].as_u64(), Some(1));
    assert!((at(&s, 3).rotation - 12.0).abs() < 1e-6 && (at(&s, 3).position[0] - 30.0).abs() < 1e-6);
    // Not straight from one key to the next, or the frames the wrong way round.
    assert!(turned_down(s.execute("keys.chart", json!({ "owner": owner, "from": 0, "to": 5, "spacing": chart })), "straight to one on 5"));
    assert!(turned_down(s.execute("keys.chart", json!({ "owner": owner, "from": 6, "to": 0, "spacing": chart })), "later frame"));
}

#[test]
fn keyframes_are_cycled_as_a_loop_or_back_and_forth_and_the_cycle_taken_away() {
    let mut s = Session::new();
    let layer = layer(&s);
    let arm = id(s.execute("peg.new", json!({ "aboveLayer": layer, "name": "Arm" })).unwrap());
    s.execute("peg.setKey", json!({ "peg": arm, "frame": 0 })).unwrap();
    s.execute("peg.setKey", json!({ "peg": arm, "frame": 4 })).unwrap();
    s.execute("peg.setTransform", json!({ "peg": arm, "frame": 4, "rotation": 40.0, "animate": true })).unwrap();
    let turned = |s: &Session, frame: u32| s.project().shots[0].peg(iskra_model::PegId(arm)).unwrap().transform_at(frame).rotation;
    let picked = json!([{ "owner": { "peg": arm }, "frame": 0 }, { "owner": { "peg": arm }, "frame": 4 }]);
    let changed = s.execute("keys.cycle", json!({ "keys": picked, "kind": "loop" })).unwrap();
    assert!(changed["changed"].as_u64().unwrap() >= 2, "every value of the peg");
    assert_eq!(s.history().undo_label(), Some("Cycle Keyframes"));
    assert_eq!([turned(&s, 5), turned(&s, 8), turned(&s, 50)], [10.0, 0.0, 20.0]);
    s.execute("keys.cycle", json!({ "keys": picked, "kind": "pingPong" })).unwrap();
    assert_eq!([turned(&s, 5), turned(&s, 8), turned(&s, 12)], [30.0, 0.0, 40.0]);
    // One key of the cycle picked takes it away; one key alone is no cycle.
    let one = json!([{ "owner": { "peg": arm }, "frame": 4 }]);
    assert!(turned_down(s.execute("keys.cycle", json!({ "keys": one, "kind": "loop" })), "two keyframes or more"));
    s.execute("keys.cycle", json!({ "keys": one, "kind": null })).unwrap();
    assert_eq!(turned(&s, 8), 40.0);
    assert!(turned_down(s.execute("keys.cycle", json!({ "keys": one, "kind": null })), "in a cycle"));
}

#[test]
fn a_particles_node_and_a_pass_are_copied_and_pasted_by_themselves() {
    use iskra_model::{EffectId, Source};
    let mut s = Session::new();
    let shot = |s: &Session| s.project().shots[0].clone();
    let layer = shot(&s).layers[0].id;
    // Rain, set its own way, over everything; and a pass of the layer.
    let rain = s.execute("effect.new", json!({ "kind": "particles", "name": "Rain", "stack": "over" })).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.set", json!({ "effect": rain, "param": "count", "value": 77.0 })).unwrap();
    let pass = s.execute("display.new", json!({ "node": { "layer": layer.0 }, "pass": true, "name": "Character" })).unwrap()["id"].as_u64().unwrap();
    // The rain alone: a copy of it, set as it is, wired into nothing.
    assert_eq!(s.execute("node.copy", json!({ "nodes": [{ "effect": rain }] })).unwrap()["copied"].as_u64(), Some(1));
    let pasted = s.execute("node.paste", json!({})).unwrap();
    let copy = EffectId(pasted["effects"][0].as_u64().expect("an effect was pasted"));
    let made = shot(&s).effect(copy).unwrap().clone();
    assert!(copy.0 != rain && made.kind == "particles" && made.number("count", 0) == 77.0 && made.name != "Rain", "{made:?}");
    assert!(!shot(&s).composites[0].inputs.contains(&Source::Effect(copy)), "not wired");
    // The pass alone: a copy of it, showing nothing, as what it showed stayed behind.
    s.execute("node.copy", json!({ "nodes": [{ "display": pass }] })).unwrap();
    let pasted = s.execute("node.paste", json!({})).unwrap();
    let copy = pasted["displays"][0].as_u64().expect("a display was pasted");
    let copied = shot(&s).displays.iter().find(|display| display.id.0 == copy).unwrap().clone();
    assert!(copied.pass && copied.shows.is_none() && copied.name != "Character", "{copied:?}");
    // With the layer it shows: the copy shows the layer's copy.
    s.execute("node.copy", json!({ "nodes": [{ "display": pass }, { "layer": layer.0 }] })).unwrap();
    let pasted = s.execute("node.paste", json!({})).unwrap();
    let (copy, layer_copy) = (pasted["displays"][0].as_u64().unwrap(), pasted["layers"][0].as_u64().unwrap());
    let copied = shot(&s).displays.iter().find(|display| display.id.0 == copy).unwrap().clone();
    assert_eq!(copied.shows, Some(Source::Layer(iskra_model::LayerId(layer_copy))));
    assert_eq!(s.history().undo_label(), Some("Paste Nodes"));
}

#[test]
fn keyframes_are_copied_and_pasted_on_another_frame_or_peg() {
    let mut s = Session::new();
    let layer = layer(&s);
    let arm = id(s.execute("peg.new", json!({ "aboveLayer": layer, "name": "Arm" })).unwrap());
    let leg = id(s.execute("peg.new", json!({ "name": "Leg" })).unwrap());
    s.execute("peg.setTransform", json!({ "peg": arm, "frame": 2, "rotation": 10.0, "animate": true })).unwrap();
    s.execute("peg.setTransform", json!({ "peg": arm, "frame": 6, "rotation": 50.0, "animate": true })).unwrap();
    let picked = json!([{ "owner": { "peg": arm }, "frame": 2 }, { "owner": { "peg": arm }, "frame": 6 }]);
    let clip = s.execute("keys.copy", json!({ "keys": picked })).unwrap()["clip"].clone();
    assert!(turned_down(s.execute("keys.copy", json!({ "keys": [{ "owner": { "peg": leg }, "frame": 2 }] })), "none of those"));
    let rotation = |s: &Session, peg: u64| {
        let peg = s.project().shots[0].peg(iskra_model::PegId(peg)).unwrap().animation.rotation.clone();
        peg.keys().iter().map(|key| (key.frame, key.value)).collect::<Vec<_>>()
    };
    // From frame 10: on 10 and 14. Onto the other peg: there.
    let pasted = s.execute("keys.paste", json!({ "clip": clip, "frame": 10 })).unwrap();
    assert_eq!(pasted["keys"], json!([{ "owner": { "peg": arm }, "frame": 10 }, { "owner": { "peg": arm }, "frame": 14 }]));
    assert_eq!((rotation(&s, arm), s.history().undo_label()), (vec![(2, 10.0), (6, 50.0), (10, 10.0), (14, 50.0)], Some("Paste Keyframes")));
    s.execute("keys.paste", json!({ "clip": clip, "frame": 0, "onto": { "peg": leg } })).unwrap();
    assert_eq!(rotation(&s, leg), [(0, 10.0), (4, 50.0)]);
    s.undo();
    assert!(rotation(&s, leg).is_empty());
    // One value's key alone, as its row on the timeline has it: the turn on 2, not the place.
    s.execute("peg.setKey", json!({ "peg": arm, "frame": 2, "channel": "position" })).unwrap();
    let turn = json!([{ "owner": { "peg": arm }, "frame": 2, "value": "rotation" }]);
    let clip = s.execute("keys.copy", json!({ "keys": turn })).unwrap()["clip"].clone();
    s.execute("keys.paste", json!({ "clip": clip, "frame": 20 })).unwrap();
    let arm_animation = s.project().shots[0].peg(iskra_model::PegId(arm)).unwrap().animation.clone();
    assert!(arm_animation.rotation.key(20).is_some() && arm_animation.position.key(20).is_none() && arm_animation.position.key(2).is_some());
}

#[test]
fn a_peg_follows_the_end_of_a_chain_of_bones() {
    let mut s = Session::new();
    let arm = layer(&s);
    let bones = json!([{ "angle": 0, "length": 75 }, { "angle": 0, "length": 75 }]);
    let chain = id(s.execute("deformer.new", json!({ "layer": arm, "origin": [50, 50], "bones": bones })).unwrap());
    let arm_peg = id(s.execute("peg.new", json!({ "aboveLayer": arm, "name": "Arm-P" })).unwrap());
    let fist = id(s.execute("peg.new", json!({ "parent": arm_peg, "name": "Fist-P" })).unwrap());
    // The peg over the arm moves what the bones bend: it can't follow them.
    assert!(turned_down(s.execute("peg.setFollow", json!({ "peg": arm_peg, "deformer": chain })), "bends what the peg carries"));
    assert!(turned_down(s.execute("peg.setFollow", json!({ "peg": fist, "deformer": chain, "point": 3 })), "no point 3"));
    s.execute("peg.setFollow", json!({ "peg": fist, "deformer": chain })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Follow Bones"));
    // It takes the turn of the bones and not how they are stretched.
    s.execute("peg.setInherit", json!({ "peg": fist, "scale": false })).unwrap();
    let inherit = s.project().shots[0].peg(iskra_model::PegId(fist)).unwrap().inherit;
    assert_eq!(inherit, iskra_model::Inherit { scale: false, ..Default::default() });
    let follows = |s: &Session| s.project().shots[0].peg(iskra_model::PegId(fist)).unwrap().follows;
    // A wire made new bends what the peg carries; told, it doesn't; moved to another point of
    // the same bones, it stays as it was.
    let followed = iskra_model::Follow { deformer: iskra_model::DeformerId(chain), point: None, bends: true };
    assert_eq!(follows(&s), Some(followed));
    s.execute("peg.setFollow", json!({ "peg": fist, "deformer": chain, "bends": false })).unwrap();
    s.execute("peg.setFollow", json!({ "peg": fist, "deformer": chain, "point": 1 })).unwrap();
    assert_eq!(follows(&s).map(|follow| (follow.point, follow.bends)), Some((Some(1), false)));
    s.execute("peg.setFollow", json!({ "peg": fist, "deformer": chain })).unwrap();
    let followed = iskra_model::Follow { bends: false, ..followed };
    assert_eq!(follows(&s), Some(followed));
    // The forearm swung down a quarter round the elbow at (125, 50): the hand at (200, 50)
    // goes to (125, 125), and the fist with it.
    s.execute("deformer.setPose", json!({ "deformer": chain, "bone": 1, "turn": 90 })).unwrap();
    let hand = |s: &Session| s.project().shots[0].world(Some(iskra_model::PegId(fist)), 0).apply(iskra_model::Point::new(210.0, 50.0));
    let at = hand(&s);
    assert!((at.x - 125.0).abs() < 1e-6 && (at.y - 135.0).abs() < 1e-6, "{at:?}");
    // With the bones deleted it follows nothing, and stays where it was at rest.
    s.execute("deformer.delete", json!({ "deformer": chain })).unwrap();
    assert_eq!(follows(&s), None);
    s.undo();
    assert_eq!(follows(&s), Some(followed));
    s.execute("peg.setFollow", json!({ "peg": fist })).unwrap();
    let at = hand(&s);
    assert!(follows(&s).is_none() && (at.x - 210.0).abs() < 1e-6 && (at.y - 50.0).abs() < 1e-6, "{at:?}");
}

#[test]
fn pegs_are_wired_into_the_start_and_end_ports_of_bones_and_curves() {
    let mut s = Session::new();
    let leg = layer(&s);
    let bones = json!([{ "angle": 0, "length": 100 }, { "angle": 0, "length": 100 }]);
    let made = id(s.execute("deformer.new", json!({ "layer": leg, "origin": [0, 0], "bones": bones })).unwrap());
    let body = id(s.execute("peg.new", json!({})).unwrap());
    let floor = id(s.execute("peg.new", json!({})).unwrap());
    s.execute("peg.setTransform", json!({ "peg": floor, "position": [150, 0] })).unwrap();
    s.execute("deformer.setPort", json!({ "deformer": made, "port": "end", "peg": floor })).unwrap();
    s.execute("deformer.setPort", json!({ "deformer": made, "port": "start", "peg": body })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Wire Peg to Deformer"));
    let chain = |s: &Session| iskra_model::Deformer::clone(&s.project().shots[0].deformers[0]);
    let posed = |s: &Session| s.project().shots[0].chain(iskra_model::DeformerId(made), 0).unwrap().joints(Some(0));
    let near = |at: iskra_model::Point, x: f64, y: f64| (at.x - x).abs() < 1e-6 && (at.y - y).abs() < 1e-6;
    // The end port is the IK's goal, which the chain is given, on.
    let ik = chain(&s).ik.expect("given IK");
    assert!(ik.on && ik.peg == Some(iskra_model::PegId(floor)) && chain(&s).start_peg == Some(iskra_model::PegId(body)));
    // The body moved up: the hip goes with it, the foot stays.
    s.execute("peg.setTransform", json!({ "peg": body, "position": [0, -40] })).unwrap();
    let joints = posed(&s);
    assert!(near(joints[0], 0.0, -40.0) && near(joints[2], 150.0, 0.0), "{joints:?}");
    // Unwired, the hip is back where the chain's own pose has it.
    s.execute("deformer.setPort", json!({ "deformer": made, "port": "start", "peg": null })).unwrap();
    assert!(chain(&s).start_peg.is_none() && near(posed(&s)[0], 0.0, 0.0));
    s.undo();
    // A peg wired in anywhere, turned, takes hold of the start where it is: nothing jumps,
    // and from there the start goes as the peg goes.
    s.execute("deformer.setPort", json!({ "deformer": made, "port": "start", "peg": null })).unwrap();
    let away = id(s.execute("peg.new", json!({})).unwrap());
    s.execute("peg.setTransform", json!({ "peg": away, "position": [300, 200], "rotation": 30 })).unwrap();
    let before = posed(&s);
    s.execute("deformer.setPort", json!({ "deformer": made, "port": "start", "peg": away, "frame": 0 })).unwrap();
    let after = posed(&s);
    assert!(before.iter().zip(&after).all(|(a, b)| near(*a, b.x, b.y)), "{before:?} {after:?}");
    s.execute("peg.setTransform", json!({ "peg": away, "position": [300, 160] })).unwrap();
    assert!(near(posed(&s)[0], 0.0, -40.0) && near(posed(&s)[2], 150.0, 0.0), "{:?}", posed(&s));
    for _ in 0..5 {
        s.undo();
    }
    assert_eq!(chain(&s).start_peg, Some(iskra_model::PegId(body)));
    // A peg that follows the chain can't be wired in; one deleted is let go.
    let rider = id(s.execute("peg.new", json!({})).unwrap());
    s.execute("peg.setFollow", json!({ "peg": rider, "deformer": made })).unwrap();
    assert!(turned_down(s.execute("deformer.setPort", json!({ "deformer": made, "port": "start", "peg": rider })), "round in a circle"));
    assert!(turned_down(s.execute("deformer.setPort", json!({ "deformer": made, "port": "middle", "peg": body })), "\"start\" or \"end\""));
    s.execute("peg.delete", json!({ "peg": body })).unwrap();
    assert!(chain(&s).start_peg.is_none());

    // A curve's end port holds its last point; an envelope has no ports.
    let tail = layer(&s);
    let curve = id(s.execute("deformer.new", json!({ "layer": tail, "curve": [[0, 0], [100, 0], [200, 0]] })).unwrap());
    s.execute("deformer.setPort", json!({ "deformer": curve, "port": "end", "peg": floor })).unwrap();
    let points = s.project().shots[0].chain(iskra_model::DeformerId(curve), 0).unwrap().joints(Some(0));
    assert!(near(points[2], 150.0, 0.0) && near(points[1], 100.0, 0.0), "{points:?}");
    let blob = layer(&s);
    let envelope = id(s.execute("deformer.new", json!({ "layer": blob, "envelope": [[0, 0], [100, 0], [50, 80]] })).unwrap());
    assert!(turned_down(s.execute("deformer.setPort", json!({ "deformer": envelope, "port": "start", "peg": floor })), "no ports"));
}

#[test]
fn a_curves_handles_are_posed_as_they_are_shown_where_a_peg_carries_its_start() {
    let mut s = Session::new();
    let arm = layer(&s);
    let curve = id(s.execute("deformer.new", json!({ "layer": arm, "curve": [[0, 0], [100, 0], [200, 0]] })).unwrap());
    let body = id(s.execute("peg.new", json!({})).unwrap());
    s.execute("deformer.setPort", json!({ "deformer": curve, "port": "start", "peg": body })).unwrap();
    // The body moved and turned: the first point goes with it, and the line turns with it.
    s.execute("peg.setTransform", json!({ "peg": body, "position": [-60, 80], "rotation": 40 })).unwrap();
    let shown = |s: &Session, knot: usize| s.project().shots[0].chain(iskra_model::DeformerId(curve), 0).unwrap().handles_at(knot, 0).unwrap();
    let near = |[x, y]: [f64; 2], [a, b]: [f64; 2]| (x - a).abs() < 1e-6 && (y - b).abs() < 1e-6;
    // Each handle posed is where it was put, as the shot shows it, the other side swung in
    // line; at the first point, which the peg carries, too.
    for knot in [0, 1] {
        s.execute("deformer.setPose", json!({ "deformer": curve, "frame": 0, "knot": knot, "into": [-12, -9], "out": [30, 40] })).unwrap();
        let handles = shown(&s, knot);
        assert!(near(handles.out, [30.0, 40.0]), "point {knot}: {handles:?}");
        assert!(knot == 0 || near(handles.into, [-12.0, -9.0]), "point {knot}: {handles:?}");
    }
}

#[test]
fn a_receive_is_wired_into_the_ports_of_bones_as_its_peg() {
    let mut s = Session::new();
    let leg = layer(&s);
    let bones = json!([{ "angle": 0, "length": 100 }, { "angle": 0, "length": 100 }]);
    let made = id(s.execute("deformer.new", json!({ "layer": leg, "origin": [0, 0], "bones": bones })).unwrap());
    let posed = |s: &Session| s.project().shots[0].chain(iskra_model::DeformerId(made), 0).unwrap().joints(Some(0));
    let near = |at: iskra_model::Point, x: f64, y: f64| (at.x - x).abs() < 1e-6 && (at.y - y).abs() < 1e-6;
    // The body and the floor sent by tag, each received by the leg's bones.
    let received = |s: &mut Session, peg: u64, tag: &str| {
        let send = id(s.execute("effect.new", json!({ "kind": "send", "name": tag })).unwrap());
        s.execute("effect.setParent", json!({ "effect": send, "parent": peg })).unwrap();
        let receive = id(s.execute("effect.new", json!({ "kind": "receive" })).unwrap());
        s.execute("effect.set", json!({ "effect": receive, "param": "tag", "value": tag })).unwrap();
        receive
    };
    let body = id(s.execute("peg.new", json!({})).unwrap());
    let floor = id(s.execute("peg.new", json!({})).unwrap());
    s.execute("peg.setTransform", json!({ "peg": floor, "position": [150, 0] })).unwrap();
    let (hip, foot) = (received(&mut s, body, "Body"), received(&mut s, floor, "Floor"));
    s.execute("deformer.setPort", json!({ "deformer": made, "port": "end", "peg": foot })).unwrap();
    s.execute("deformer.setPort", json!({ "deformer": made, "port": "start", "peg": hip })).unwrap();
    s.execute("peg.setTransform", json!({ "peg": body, "position": [0, -40] })).unwrap();
    let joints = posed(&s);
    assert!(near(joints[0], 0.0, -40.0) && near(joints[2], 150.0, 0.0), "{joints:?}");
    // A Receive that nothing is sent to carries no peg, and is turned down.
    let empty = id(s.execute("effect.new", json!({ "kind": "receive" })).unwrap());
    assert!(turned_down(s.execute("deformer.setPort", json!({ "deformer": made, "port": "start", "peg": empty })), "no peg"));
    // Its Send deleted, the Receive carries nothing: the hip is back where the pose has it,
    // and the start stays wired to it.
    let send =
        s.project().shots[0].effects.iter().find(|effect| effect.kind == iskra_model::SEND && effect.parent == Some(iskra_model::PegId(body))).unwrap().id.0;
    s.execute("effect.delete", json!({ "effect": send })).unwrap();
    assert!(near(posed(&s)[0], 0.0, 0.0), "{:?}", posed(&s));
    s.undo();
    // The Receive deleted, the start is wired to its Send, as what is parented to it is; the
    // Send deleted then, to the peg that came into it.
    s.execute("effect.delete", json!({ "effect": hip })).unwrap();
    assert_eq!(s.project().shots[0].deformers[0].start_peg, Some(iskra_model::PegId(send)));
    s.execute("effect.delete", json!({ "effect": send })).unwrap();
    assert_eq!(s.project().shots[0].deformers[0].start_peg, Some(iskra_model::PegId(body)));
    assert!(near(posed(&s)[0], 0.0, -40.0), "{:?}", posed(&s));
}

#[test]
fn ik_reaches_for_its_goal_and_is_switched_without_a_jump() {
    let mut s = Session::new();
    let arm = layer(&s);
    let bones = json!([{ "angle": 0, "length": 100 }, { "angle": 0, "length": 100 }]);
    let made = id(s.execute("deformer.new", json!({ "layer": arm, "origin": [0, 0], "bones": bones })).unwrap());
    let chain = |s: &Session| iskra_model::Deformer::clone(&s.project().shots[0].deformers[0]);
    let hand = |s: &Session, frame: u32| chain(s).joints(Some(frame))[2];
    let near = |at: iskra_model::Point, x: f64, y: f64| (at.x - x).abs() < 1e-6 && (at.y - y).abs() < 1e-6;
    // Given IK, the goal is where the hand is, so nothing moves; moved, the hand goes with it.
    s.execute("deformer.setIk", json!({ "deformer": made, "on": true })).unwrap();
    assert!(near(hand(&s, 0), 200.0, 0.0) && chain(&s).ik.unwrap().goal == [200.0, 0.0]);
    assert_eq!(s.history().undo_label(), Some("Set Bones' IK"));
    s.execute("deformer.setPose", json!({ "deformer": made, "goal": [100, 100] })).unwrap();
    assert!(near(hand(&s, 0), 100.0, 100.0), "{:?}", hand(&s, 0));

    // Keyed: on up to frame 10 and off from there, the bones keep the pose IK gave them.
    s.execute("deformer.setKey", json!({ "deformer": made, "frame": 0 })).unwrap();
    s.execute("deformer.setIk", json!({ "deformer": made, "frame": 10, "on": false })).unwrap();
    assert!(near(hand(&s, 9), 100.0, 100.0) && near(hand(&s, 10), 100.0, 100.0) && near(hand(&s, 20), 100.0, 100.0));
    let ik = chain(&s).ik.unwrap();
    assert!(ik.on_at(9) && !ik.on_at(10) && ik.ons.keys().iter().all(|key| key.out == iskra_model::Interp::Hold));
    // Posed by hand from there, and switched on again, the goal is put where the hand is.
    s.execute("deformer.setPose", json!({ "deformer": made, "frame": 20, "bone": 0, "turn": 0 })).unwrap();
    s.execute("deformer.setPose", json!({ "deformer": made, "frame": 20, "bone": 1, "turn": 0 })).unwrap();
    s.execute("deformer.setIk", json!({ "deformer": made, "frame": 20, "on": true })).unwrap();
    assert!(near(hand(&s, 20), 200.0, 0.0) && near(hand(&s, 0), 100.0, 100.0), "{:?} {:?}", hand(&s, 20), hand(&s, 0));
    s.undo();
    assert!(!chain(&s).ik.unwrap().on_at(20), "the switch and the goal are one step");

    // A peg wired in is the goal: the hand goes where its pivot is.
    s.execute("deformer.setIk", json!({ "deformer": made, "frame": 20, "on": true })).unwrap();
    let target = id(s.execute("peg.new", json!({})).unwrap());
    s.execute("peg.setTransform", json!({ "peg": target, "position": [0, 150] })).unwrap();
    s.execute("deformer.setIk", json!({ "deformer": made, "goalPeg": target })).unwrap();
    let posed = |s: &Session, frame: u32| s.project().shots[0].chain(iskra_model::DeformerId(made), frame).unwrap().joints(Some(frame))[2];
    assert!(near(posed(&s, 20), 0.0, 150.0), "{:?}", posed(&s, 20));
    s.execute("deformer.setIk", json!({ "deformer": made, "goalPeg": null })).unwrap();
    assert_eq!(chain(&s).ik.unwrap().peg, None, "null unwires it");
    s.undo();
    // A peg that follows the chain can't be its goal; one deleted is let go.
    let rider = id(s.execute("peg.new", json!({})).unwrap());
    s.execute("peg.setFollow", json!({ "peg": rider, "deformer": made })).unwrap();
    assert!(turned_down(s.execute("deformer.setIk", json!({ "deformer": made, "goalPeg": rider })), "round in a circle"));
    s.execute("peg.delete", json!({ "peg": target })).unwrap();
    assert_eq!(chain(&s).ik.unwrap().peg, None);

    // A joint the chain has not got, a curve, and a goal without IK are turned down.
    assert!(turned_down(s.execute("deformer.setIk", json!({ "deformer": made, "joint": 3 })), "no joint 3"));
    s.execute("deformer.setIk", json!({ "deformer": made, "remove": true })).unwrap();
    assert!(chain(&s).ik.is_none());
    assert!(turned_down(s.execute("deformer.setPose", json!({ "deformer": made, "goal": [1, 1] })), "no IK"));
}

#[test]
fn bones_bend_a_layers_drawings_and_their_pose_is_keyed() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let arm = layer(&s);
    s.execute("layer.rename", json!({ "layer": arm, "name": "Arm" })).unwrap();
    // A stroke from (100, 100) to (400, 100) of the layer's pixels, which are two to a scene
    // unit: from (50, 50) to (200, 50) of the scene, with two bones along it.
    s.execute("drawing.stroke", stroke_params(arm, 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
    let bones = json!([{ "angle": 0, "length": 75 }, { "angle": 0, "length": 75 }]);
    let made = id(s.execute("deformer.new", json!({ "layer": arm, "origin": [50, 50], "bones": bones })).unwrap());
    let deformer = |s: &Session| s.project().shots[0].deformers[0].clone();
    assert_eq!((deformer(&s).name.as_str(), s.project().layer(iskra_model::LayerId(arm)).unwrap().deformer.map(|id| id.0)), ("Arm-Bones", Some(made)));
    assert_eq!(s.history().undo_label(), Some("New Bones"));
    let shoot = |s: &mut Session, name: &str| {
        let path = dir.path().join(name);
        s.execute("shot.renderFrame", json!({ "path": path })).unwrap();
        let (width, _, pixels) = iskra_io::decode_png(&std::fs::read(&path).unwrap()).unwrap();
        move |x: u32, y: u32| pixels[(y * width + x) as usize][3] > 0
    };
    // The frame is 1920 by 1080 with the scene's origin in its middle.
    let (hand, bent) = ((960 + 190, 540 + 50), (960 + 125, 540 + 115));
    let rest = shoot(&mut s, "rest.png");
    assert!(rest(hand.0, hand.1) && !rest(bent.0, bent.1));

    // The forearm turned a quarter: the hand end swings down round the elbow at (125, 50).
    s.execute("deformer.setPose", json!({ "deformer": made, "bone": 1, "turn": 90 })).unwrap();
    assert_eq!((deformer(&s).bones[1].turn, s.history().undo_label()), (90.0, Some("Pose Bones")));
    let posed = shoot(&mut s, "posed.png");
    assert!(!posed(hand.0, hand.1) && posed(bent.0, bent.1) && posed(960 + 80, 540 + 50), "the forearm hangs, the upper arm lies");
    s.undo();
    // Keyed, a pose is of its frame: at rest on frame 0, bent on 10, and between the two between.
    s.execute("deformer.setKey", json!({ "deformer": made, "frame": 0 })).unwrap();
    assert!(turned_down(s.execute("deformer.setPose", json!({ "deformer": made, "bone": 1, "turn": 90 })), "is animated"));
    s.execute("deformer.setPose", json!({ "deformer": made, "frame": 10, "bone": 1, "turn": 90, "offset": [0, 20] })).unwrap();
    let chain = deformer(&s);
    assert_eq!((chain.bones[1].turn_at(0), chain.bones[1].turn_at(10), chain.key_frames().into_iter().collect::<Vec<_>>()), (0.0, 90.0, vec![0, 10]));
    assert!(chain.bones[1].turn_at(5) > 10.0 && chain.bones[1].turn_at(5) < 80.0 && chain.bones[0].turns.keys().len() == 1);
    // The keys of a frame are moved and eased together, as a peg's are.
    s.execute("deformer.moveKey", json!({ "deformer": made, "from": 10, "to": 16 })).unwrap();
    s.execute("deformer.setEase", json!({ "deformer": made, "frame": 0, "out": "hold" })).unwrap();
    let chain = deformer(&s);
    assert_eq!(
        (chain.key_frames().into_iter().collect::<Vec<_>>(), chain.interp(0)),
        (vec![0, 16], Some((iskra_model::Interp::Linear, iskra_model::Interp::Hold)))
    );
    assert!(chain.bones[1].turn_at(15) == 0.0 && chain.bones[1].turn_at(16) == 90.0 && chain.offset_at(15) == [0.0, 0.0]);
    assert!(turned_down(s.execute("deformer.moveKey", json!({ "deformer": made, "from": 10, "to": 12 })), "no keyframe on frame 10"));
    assert!(turned_down(s.execute("deformer.setEase", json!({ "deformer": made, "frame": 16, "into": "hold" })), "not into"));
    // Shaped by a handle, as a peg's are: out of frame 0 slowly, all the pose's values alike,
    // so the turn and the move are as far along their ways on each frame.
    s.execute("deformer.setEase", json!({ "deformer": made, "frame": 0, "out": "linear" })).unwrap();
    let linear = deformer(&s).bones[1].turn_at(4);
    let slow = json!({ "curve": { "reach": 0.6, "speed": 0.0 } });
    s.execute("deformer.setEase", json!({ "deformer": made, "frame": 0, "out": slow })).unwrap();
    let chain = deformer(&s);
    let (turned, moved) = (chain.bones[1].turn_at(4) / 90.0, chain.offset_at(4)[1] / 20.0);
    assert!(chain.bones[1].turn_at(4) < linear - 1.0 && (turned - moved).abs() < 1e-3, "{turned} {moved} against {}", linear / 90.0);
    assert!(turned_down(
        s.execute("deformer.setEase", json!({ "deformer": made, "frame": 16, "into": { "curve": { "reach": 2.0, "speed": 1.0 } } })),
        "0.01 to 1"
    ));
    s.execute("deformer.setEase", json!({ "deformer": made, "frame": 0, "out": "linear" })).unwrap();
    // One value alone, as its row on the timeline has it: the forearm's turn moves to 20, and
    // eases there, while the rest of the pose stays on 16.
    let forearm = json!({ "turn": 1 });
    s.execute("deformer.moveKey", json!({ "deformer": made, "from": 16, "to": 20, "value": forearm })).unwrap();
    s.execute("deformer.setEase", json!({ "deformer": made, "frame": 20, "into": "ease", "value": forearm })).unwrap();
    let chain = deformer(&s);
    let frames = |value| chain.key_frames_of(Some(value)).into_iter().collect::<Vec<_>>();
    assert_eq!((frames(iskra_model::PoseValue::Turn(1)), frames(iskra_model::PoseValue::Offset)), (vec![0, 20], vec![0, 16]));
    assert_eq!(
        (chain.interp_of(20, Some(iskra_model::PoseValue::Turn(1))), chain.interp(16)),
        (Some((iskra_model::Interp::Ease, iskra_model::Interp::Linear)), Some((iskra_model::Interp::Linear, iskra_model::Interp::Linear)))
    );
    assert!(turned_down(s.execute("deformer.removeKey", json!({ "deformer": made, "frame": 16, "value": forearm })), "no keyframe of Bone 2 Turn on frame 16"));
    s.execute("deformer.moveKey", json!({ "deformer": made, "from": 20, "to": 16, "value": forearm })).unwrap();
    s.execute("deformer.setEase", json!({ "deformer": made, "frame": 16, "into": "linear", "value": forearm })).unwrap();
    s.execute("deformer.moveKey", json!({ "deformer": made, "from": 16, "to": 10 })).unwrap();
    s.execute("deformer.removeKey", json!({ "deformer": made, "frame": 10 })).unwrap();
    assert!(turned_down(s.execute("deformer.removeKey", json!({ "deformer": made, "frame": 10 })), "no keyframe on frame 10"));
    s.execute("deformer.removeKey", json!({ "deformer": made, "frame": 0 })).unwrap();
    assert!(!deformer(&s).is_animated() && deformer(&s).bones[1].turn == 0.0);

    // The rest is set anew with the pose kept; a third bone is at rest.
    s.execute("deformer.setPose", json!({ "deformer": made, "bone": 1, "turn": 30 })).unwrap();
    let longer = json!([{ "angle": 10, "length": 60 }, { "angle": -10, "length": 60 }, { "angle": 0, "length": 30 }]);
    s.execute("deformer.setRest", json!({ "deformer": made, "origin": [40, 50], "bones": longer })).unwrap();
    let chain = deformer(&s);
    assert_eq!((chain.origin, chain.bones.len(), chain.bones[1].rest_angle, chain.bones[1].turn, chain.bones[2].turn), ([40.0, 50.0], 3, -10.0, 30.0, 0.0));

    // A second layer is given to the same bones; names are the shot's, and what is asked is checked.
    let sleeve = id(s.execute("layer.new", json!({ "name": "Sleeve" })).unwrap());
    s.execute("layer.setDeformer", json!({ "layer": sleeve, "deformer": made })).unwrap();
    assert!(turned_down(s.execute("layer.setDeformer", json!({ "layer": sleeve, "deformer": 999 })), "has no deformer 999"));
    assert!(turned_down(s.execute("deformer.rename", json!({ "deformer": made, "name": "Sleeve" })), "already has a layer named"));
    assert!(turned_down(s.execute("layer.rename", json!({ "layer": sleeve, "name": "Arm-Bones" })), "already has a deformer named"));
    s.execute("deformer.rename", json!({ "deformer": made, "name": "Arm Bones" })).unwrap();
    for (params, why) in [
        (json!({ "layer": arm, "origin": [0, 0], "bones": [] }), "from 1 to 64 bones"),
        (json!({ "layer": arm, "origin": [0, 0], "bones": [{ "angle": 0, "length": 0 }] }), "has a length"),
        (json!({ "layer": 999, "origin": [0, 0], "bones": [{ "angle": 0, "length": 5 }] }), "no layer 999"),
    ] {
        assert!(turned_down(s.execute("deformer.new", params), why), "{why}");
    }
    assert!(turned_down(s.execute("deformer.setPose", json!({ "deformer": made, "bone": 7, "turn": 5 })), "no bone 7"));
    assert!(turned_down(s.execute("deformer.setPose", json!({ "deformer": made, "turn": 5 })), "give both"));
    // A bone is stretched, which is keyed like its turn; and how the bones share the drawing
    // out is set once, for every frame.
    s.execute("deformer.setPose", json!({ "deformer": made, "bone": 0, "stretch": 1.5 })).unwrap();
    let joints = deformer(&s).joints(Some(0));
    assert!(((joints[1].x - joints[0].x).hypot(joints[1].y - joints[0].y) - deformer(&s).bones[0].length * 1.5).abs() < 1e-9);
    s.execute("deformer.setKey", json!({ "deformer": made, "frame": 0 })).unwrap();
    s.execute("deformer.setPose", json!({ "deformer": made, "frame": 8, "bone": 0, "stretch": 2 })).unwrap();
    assert_eq!((deformer(&s).bones[0].stretch_at(0), deformer(&s).bones[0].stretch_at(8), deformer(&s).bones[0].stretch_at(4)), (1.5, 2.0, 1.75));
    assert!(turned_down(s.execute("deformer.setPose", json!({ "deformer": made, "bone": 0, "stretch": 2 })), "is animated"));
    assert!(turned_down(s.execute("deformer.setPose", json!({ "deformer": made, "bone": 0, "stretch": 0 })), "stretched to between"));
    s.execute("deformer.removeKey", json!({ "deformer": made, "frame": 8 })).unwrap();
    s.execute("deformer.removeKey", json!({ "deformer": made, "frame": 0 })).unwrap();
    s.execute("deformer.setPose", json!({ "deformer": made, "bone": 0, "stretch": 1 })).unwrap();
    s.execute("deformer.setWeights", json!({ "deformer": made, "stiffness": 6, "bone": 1, "hold": 2, "joint": 1, "round": 20, "fade": 35 })).unwrap();
    let chain = deformer(&s);
    assert_eq!((chain.stiffness, chain.bones[1].hold, s.history().undo_label()), (6.0, 2.0, Some("Set Bones' Weights")));
    assert_eq!((chain.bones[1].round, chain.bones[1].fade, chain.bones[0].round), (20.0, 35.0, 0.0));
    // The second circle is never inside the first.
    s.execute("deformer.setWeights", json!({ "deformer": made, "joint": 1, "round": 50 })).unwrap();
    assert_eq!((deformer(&s).bones[1].round, deformer(&s).bones[1].fade), (50.0, 50.0));
    s.execute("deformer.setWeights", json!({ "deformer": made, "joint": 1, "pull": 0.5 })).unwrap();
    assert_eq!((deformer(&s).bones[1].pull, deformer(&s).bones[1].round, deformer(&s).bones[1].crease), (0.5, 50.0, 1.0));
    s.execute("deformer.setWeights", json!({ "deformer": made, "joint": 1, "crease": 0.25 })).unwrap();
    assert_eq!(deformer(&s).bones[1].crease, 0.25);
    for (params, why) in [
        (json!({ "stiffness": 0.5 }), "stiffness is from"),
        (json!({ "round": 2 }), "say which"),
        (json!({ "joint": 0, "round": 2 }), "no joint 0"),
        (json!({ "joint": 1, "fade": -1 }), "radius from 0"),
        (json!({ "reach": 5 }), "say which"),
        (json!({ "knot": 0, "reach": 5 }), "no point 0"),
        (json!({ "joint": 1, "pull": 2 }), "pull is from"),
        (json!({ "joint": 1, "crease": -0.5 }), "crease is from"),
        (json!({ "pull": 0.5 }), "say which"),
        (json!({ "hold": 2 }), "give both"),
        (json!({ "bone": 0, "hold": 100 }), "hold is from"),
        (json!({ "bone": 9, "hold": 2 }), "no bone 9"),
        (json!({ "ends": [5, 5] }), "has no ends"),
    ] {
        let mut params = params;
        params["deformer"] = json!(made);
        assert!(turned_down(s.execute("deformer.setWeights", params), why), "{why}");
    }
    // Deleted, it bends nothing: its layers are as they were drawn. And all of it is in the file.
    let file = dir.path().join("bones.iskra");
    s.execute("file.saveAs", json!({ "path": file })).unwrap();
    let mut again = Session::new();
    again.execute("file.open", json!({ "path": file })).unwrap();
    assert_eq!(again.project().shots[0].deformers, s.project().shots[0].deformers);
    s.execute("deformer.delete", json!({ "deformer": made })).unwrap();
    assert!(s.project().shots[0].deformers.is_empty() && s.project().shots[0].layers.iter().all(|layer| layer.deformer.is_none()));
    s.undo();
    assert_eq!(s.project().layer(iskra_model::LayerId(sleeve)).unwrap().deformer.map(|id| id.0), Some(made));
}

#[test]
fn a_curve_bends_a_layers_drawings_along_its_line() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let tail = layer(&s);
    s.execute("layer.rename", json!({ "layer": tail, "name": "Tail" })).unwrap();
    // A stroke from (50, 50) to (200, 50) of the scene, with a curve of three points along it.
    s.execute("drawing.stroke", stroke_params(tail, 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
    let made = id(s.execute("deformer.new", json!({ "layer": tail, "curve": [[50, 50], [125, 50], [200, 50]] })).unwrap());
    let curve = |s: &Session| s.project().shots[0].deformers[0].clone();
    assert_eq!((curve(&s).name.as_str(), curve(&s).is_curve(), s.history().undo_label()), ("Tail-Curve", true, Some("New Bones")));
    let shoot = |s: &mut Session, name: &str| {
        let path = dir.path().join(name);
        s.execute("shot.renderFrame", json!({ "frame": 0, "path": path })).unwrap();
        let image = image::open(&path).unwrap().to_rgba8();
        move |x: u32, y: u32| image.get_pixel(x, y)[3] > 128
    };
    let at_rest = shoot(&mut s, "rest.png");
    assert!(at_rest(960 + 190, 540 + 50) && !at_rest(960 + 200, 540 + 140));
    // Its end moved down by 90: the stroke's end goes there, and its start stays.
    s.execute("deformer.setPose", json!({ "deformer": made, "knot": 2, "to": [200, 140] })).unwrap();
    assert_eq!((curve(&s).spine[2].moved, s.history().undo_label()), ([0.0, 90.0], Some("Pose Bones")));
    let posed = shoot(&mut s, "posed.png");
    assert!(posed(960 + 198, 540 + 132) && !posed(960 + 190, 540 + 50) && posed(960 + 60, 540 + 50));
    // Keyed, a point moved on another frame is a key there; and its rest is laid anew.
    s.execute("deformer.setKey", json!({ "deformer": made, "frame": 0 })).unwrap();
    assert!(turned_down(s.execute("deformer.setPose", json!({ "deformer": made, "knot": 2, "to": [200, 50] })), "is animated"));
    s.execute("deformer.setPose", json!({ "deformer": made, "frame": 10, "knot": 2, "to": [200, 50] })).unwrap();
    assert_eq!(
        (curve(&s).key_frames().into_iter().collect::<Vec<_>>(), curve(&s).joints(Some(10))[2].y, curve(&s).joints(Some(0))[2].y),
        (vec![0, 10], 50.0, 140.0)
    );
    // A point's handles are posed too: as a key on the frame, since the point has keys, with
    // the frame that was keyed left as it was, and the rest too.
    let before = curve(&s).handles_at(1, 0).unwrap();
    assert!(turned_down(s.execute("deformer.setPose", json!({ "deformer": made, "knot": 1, "out": [30, 40] })), "is animated"));
    s.execute("deformer.setPose", json!({ "deformer": made, "frame": 10, "knot": 1, "out": [30, 40] })).unwrap();
    let (swung, kept) = (curve(&s).handles_at(1, 10).unwrap(), curve(&s).handles_at(1, 0).unwrap());
    assert!((swung.out[0] - 30.0).abs() < 1e-6 && (swung.out[1] - 40.0).abs() < 1e-6, "{swung:?}");
    assert!((kept.out[0] - before.out[0]).abs() < 1e-6 && (kept.out[1] - before.out[1]).abs() < 1e-6, "{kept:?}");
    assert_eq!((curve(&s).spine[1].bend, curve(&s).spine[1].bends_out.keys().len(), s.history().undo_label()), (None, 2, Some("Pose Bones")));
    // And put back to those of the rest, on that frame.
    assert!(turned_down(s.execute("deformer.setPose", json!({ "deformer": made, "frame": 10, "knot": 1, "atRest": true, "out": [1, 1] })), "not both"));
    s.execute("deformer.setPose", json!({ "deformer": made, "frame": 10, "knot": 1, "atRest": true })).unwrap();
    assert_eq!(curve(&s).spine[1].bent_at(10), iskra_model::Bend::default());
    s.undo();
    s.undo();
    s.execute("deformer.setRest", json!({ "deformer": made, "curve": [[50, 50], [100, 50], [150, 50], [200, 50]] })).unwrap();
    assert_eq!((curve(&s).spine.len(), curve(&s).spine[1].at, curve(&s).spine[3].moves.keys().len()), (4, [100.0, 50.0], 0));
    // A point's two circles say how far to the side the curve reaches there.
    s.execute("deformer.setWeights", json!({ "deformer": made, "knot": 1, "reach": 30, "fade": 20 })).unwrap();
    assert_eq!((curve(&s).circles(1), curve(&s).circles(0), s.history().undo_label()), ([30.0, 30.0], [0.0, 0.0], Some("Set Bones' Weights")));
    s.execute("deformer.setWeights", json!({ "deformer": made, "knot": 1, "fade": 45 })).unwrap();
    assert_eq!(curve(&s).circles(1), [30.0, 45.0]);
    assert!(turned_down(s.execute("deformer.setWeights", json!({ "deformer": made, "knot": 7, "reach": 1 })), "no point 7"));
    // How far from each end it is moved and not stretched.
    s.execute("deformer.setWeights", json!({ "deformer": made, "ends": [20, 35] })).unwrap();
    assert_eq!(curve(&s).ends, [20.0, 35.0]);
    assert!(turned_down(s.execute("deformer.setWeights", json!({ "deformer": made, "ends": [-1, 0] })), "from 0 to"));
    assert!(turned_down(s.execute("deformer.insertKnot", json!({ "deformer": made, "after": 3 })), "is the last of"));
    s.execute("deformer.removeKnot", json!({ "deformer": made, "knot": 1 })).unwrap();
    s.execute("deformer.removeKnot", json!({ "deformer": made, "knot": 1 })).unwrap();
    assert!(turned_down(s.execute("deformer.removeKnot", json!({ "deformer": made, "knot": 0 })), "keeps 2 points"));
    s.undo();
    s.undo();
    for (id, params, why) in [
        ("deformer.new", json!({ "layer": tail, "curve": [[0, 0]] }), "from 2 to"),
        ("deformer.new", json!({ "layer": tail, "curve": [[0, 0], [1, 1]], "origin": [0, 0] }), "one of the three"),
        ("deformer.new", json!({ "layer": tail }), "one of the three"),
        ("deformer.setRest", json!({ "deformer": made, "origin": [0, 0] }), "is a curve"),
        ("deformer.setPose", json!({ "deformer": made, "knot": 9, "frame": 0, "to": [0, 0] }), "no point 9"),
        ("deformer.setPose", json!({ "deformer": made, "knot": 1 }), "one of them"),
    ] {
        assert!(turned_down(s.execute(id, params), why), "{why}");
    }
    // It is in the file, and a file with a curve says so by its number.
    let file = dir.path().join("curve.iskra");
    s.execute("file.saveAs", json!({ "path": file })).unwrap();
    let mut again = Session::new();
    again.execute("file.open", json!({ "path": file })).unwrap();
    assert_eq!(again.project().shots[0].deformers, s.project().shots[0].deformers);
}

#[test]
fn an_envelope_holds_a_shape_of_a_layers_drawing() {
    let mut s = Session::new();
    let belly = layer(&s);
    s.execute("layer.rename", json!({ "layer": belly, "name": "Belly" })).unwrap();
    let outline = json!([[100, 0], [200, 100], [100, 200], [0, 100]]);
    let made = id(s.execute("deformer.new", json!({ "layer": belly, "envelope": outline })).unwrap());
    let envelope = |s: &Session| s.project().shots[0].deformers[0].clone();
    assert_eq!((envelope(&s).name.as_str(), envelope(&s).is_envelope(), envelope(&s).spine.len()), ("Belly-Envelope", true, 4));
    // A point pulled out is a pose, keyed like a curve's; laid anew, it keeps how it is moved.
    s.execute("deformer.setPose", json!({ "deformer": made, "knot": 1, "to": [260, 100] })).unwrap();
    assert_eq!(envelope(&s).warp(0, 1.0).apply(iskra_model::Point::new(200.0, 100.0)), iskra_model::Point::new(260.0, 100.0));
    s.execute("deformer.setRest", json!({ "deformer": made, "envelope": [[100, 0], [210, 100], [100, 200], [0, 100], [20, 30]] })).unwrap();
    assert_eq!((envelope(&s).spine.len(), envelope(&s).spine[1].at, envelope(&s).spine[1].moved), (5, [210.0, 100.0], [60.0, 0.0]));
    s.execute("deformer.setWeights", json!({ "deformer": made, "knot": 4, "reach": 10, "fade": 30 })).unwrap();
    assert_eq!(envelope(&s).circles(4), [10.0, 30.0]);
    // A point's handles shape how the outline passes it; a point is put in and taken out.
    s.execute("deformer.setBend", json!({ "deformer": made, "knot": 0, "out": [40, 0] })).unwrap();
    let bend = envelope(&s).spine[0].bend.unwrap();
    assert!(bend.out == [40.0, 0.0] && bend.into[0] < 0.0 && s.history().undo_label() == Some("Shape Curve at Point"), "{bend:?}");
    s.execute("deformer.setBend", json!({ "deformer": made, "knot": 0, "smooth": true })).unwrap();
    assert_eq!(envelope(&s).spine[0].bend, None);
    s.execute("deformer.insertKnot", json!({ "deformer": made, "after": 4 })).unwrap();
    s.execute("deformer.insertKnot", json!({ "deformer": made, "after": 0, "at": [160, 40] })).unwrap();
    assert_eq!((envelope(&s).spine.len(), envelope(&s).spine[1].at, s.history().undo_label()), (7, [160.0, 40.0], Some("Add Point")));
    s.execute("deformer.removeKnot", json!({ "deformer": made, "knot": 1 })).unwrap();
    s.execute("deformer.removeKnot", json!({ "deformer": made, "knot": 5 })).unwrap();
    assert_eq!((envelope(&s).spine.len(), envelope(&s).spine[1].at, envelope(&s).circles(4)), (5, [210.0, 100.0], [10.0, 30.0]));
    for (id, params, why) in [
        ("deformer.setBend", json!({ "deformer": made, "knot": 9, "smooth": true }), "no point 9"),
        ("deformer.setBend", json!({ "deformer": made, "knot": 0 }), "give a handle"),
        ("deformer.insertKnot", json!({ "deformer": made, "after": 12 }), "no point 12"),
        ("deformer.new", json!({ "layer": belly, "envelope": [[0, 0], [1, 1]] }), "from 3 to"),
        ("deformer.new", json!({ "layer": belly, "envelope": [[0, 0], [1, 1], [2, 0]], "curve": [[0, 0], [1, 1]] }), "one of the three"),
        ("deformer.setRest", json!({ "deformer": made, "curve": [[0, 0], [1, 1]] }), "is an envelope"),
        ("deformer.setRest", json!({ "deformer": made, "envelope": [[0, 0], [1, 1]] }), "from 3 to"),
    ] {
        assert!(turned_down(s.execute(id, params), why), "{why}");
    }
}

#[test]
fn nodes_are_grouped_and_a_group_is_folded_nested_and_taken_apart() {
    let mut s = Session::new();
    let arm = layer(&s);
    let hand = id(s.execute("layer.new", json!({})).unwrap());
    let peg = id(s.execute("peg.new", json!({ "aboveLayer": arm })).unwrap());
    let shot = |s: &Session| s.project().shots[0].clone();
    let members = |s: &Session, group: u64| shot(s).group(iskra_model::GroupId(group)).unwrap().members.iter().copied().collect::<Vec<_>>();
    // A group of a peg and its layer; the picture is as it was.
    let stack = shot(&s).stack();
    let rig = id(s.execute("group.new", json!({ "nodes": [peg, arm] })).unwrap());
    assert_eq!(
        (shot(&s).groups[0].name.as_str(), members(&s, rig), s.history().undo_label(), shot(&s).stack()),
        ("Group", vec![arm, peg], Some("New Group"), stack)
    );
    // A group of what is all in one group is in that group; folded, the outer puts all away.
    let inner = id(s.execute("group.new", json!({ "nodes": [arm], "name": "Arm" })).unwrap());
    assert_eq!((members(&s, rig), members(&s, inner)), (vec![peg, inner], vec![arm]));
    s.execute("group.set", json!({ "group": rig, "folded": true, "board": true })).unwrap();
    assert_eq!(shot(&s).folded_into(arm).map(|group| group.0), Some(rig));
    assert!(shot(&s).is_board(iskra_model::LayerId(arm)) && !shot(&s).is_board(iskra_model::LayerId(hand)));
    // Nodes are put in and taken out; a group can't go into itself or into what is in it.
    s.execute("group.add", json!({ "group": inner, "nodes": [hand, peg] })).unwrap();
    assert_eq!((members(&s, rig), members(&s, inner)), (vec![inner], vec![arm, hand, peg]));
    s.execute("group.remove", json!({ "nodes": [hand] })).unwrap();
    assert_eq!(members(&s, inner), vec![arm, peg]);
    s.execute("group.rename", json!({ "group": inner, "name": "Arm Rig" })).unwrap();
    for (id, params, why) in [
        ("group.add", json!({ "group": inner, "nodes": [rig] }), "nor into a group that is in it"),
        ("group.add", json!({ "group": inner, "nodes": [inner] }), "can't be put into itself"),
        ("group.new", json!({ "nodes": [] }), "give the `nodes`"),
        ("group.new", json!({ "nodes": [99999] }), "has no node 99999"),
        ("group.new", json!({ "nodes": [hand], "name": "Arm Rig" }), "already has a group named"),
        ("group.rename", json!({ "group": rig, "name": "Arm Rig" }), "already has a group named"),
        ("layer.rename", json!({ "layer": hand, "name": "Arm Rig" }), "already has a group named"),
        ("group.set", json!({ "group": rig }), "say what to set"),
        ("group.delete", json!({ "group": 4242 }), "no group 4242"),
    ] {
        assert!(turned_down(s.execute(id, params), why), "{why}");
    }
    // Taken apart, the inner group's nodes are in the outer; a deleted node's id names nothing.
    s.execute("group.delete", json!({ "group": inner })).unwrap();
    assert_eq!((members(&s, rig), shot(&s).groups.len(), s.history().undo_label()), (vec![arm, peg], 1, Some("Ungroup")));
    s.execute("layer.delete", json!({ "layer": arm })).unwrap();
    assert_eq!(shot(&s).group_nodes(iskra_model::GroupId(rig)).len(), 2);
    s.undo();
    s.undo();
    assert_eq!(members(&s, inner), vec![arm, peg]);
}

#[test]
fn a_template_applied_onto_a_rig_brings_its_variants_in_skins() {
    let dir = tempfile::tempdir().unwrap();
    // A shoulder over an arm whose drawing has a Night variant, saved to the library.
    let rigged = |s: &mut Session| {
        let arm = layer(s);
        s.execute("layer.rename", json!({ "layer": arm, "name": "armL" })).unwrap();
        s.execute("drawing.stroke", stroke_params(arm, 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
        let shoulder = id(s.execute("peg.new", json!({ "aboveLayer": arm, "name": "shoulder" })).unwrap());
        (arm, shoulder)
    };
    let mut s = Session::new();
    let (arm, shoulder) = rigged(&mut s);
    let shot = s.project().shots[0].id.0;
    s.execute("skin.add", json!({ "name": "Night", "shot": shot })).unwrap();
    s.execute("drawing.stroke", stroke_params(arm, 0, json!({ "size": 40, "pressureSize": false }))).unwrap();
    let saved = s.execute("library.save", json!({ "folder": dir.path(), "name": "Arm", "nodes": [{ "peg": shoulder }] })).unwrap();

    // Applied onto the same rig in another project, which has no skins: the arm's drawing of
    // the same name gets the variant, in a Night made there.
    let mut other = Session::new();
    let (arm, shoulder) = rigged(&mut other);
    other.execute("library.apply", json!({ "path": saved["path"], "peg": shoulder })).unwrap();
    let night = other.project().skin_named("Night").expect("made by its name").id;
    let found = other.project().layer(iskra_model::LayerId(arm)).unwrap().clone();
    let variant = found.variant(night, found.drawings[0].id).expect("the drawing's variant came with the template");
    assert!(!variant.drawing.drawing.bounds().is_empty() && found.variants.len() == 1);
}

#[test]
fn a_template_is_applied_onto_a_rig_the_shot_has_by_its_names() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let arm = layer(&s);
    s.execute("layer.rename", json!({ "layer": arm, "name": "armL" })).unwrap();
    s.execute("drawing.stroke", stroke_params(arm, 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
    let shoulder = id(s.execute("peg.new", json!({ "aboveLayer": arm, "name": "shoulder" })).unwrap());
    // A wave is keyed on the shoulder, saved to the library, and then undone in the shot.
    s.execute("peg.setKey", json!({ "peg": shoulder, "frame": 0 })).unwrap();
    s.execute("peg.setTransform", json!({ "peg": shoulder, "frame": 6, "rotation": 40 })).unwrap();
    s.execute("exposure.setHold", json!({ "layer": arm, "frame": 0, "length": 4, "ripple": false })).unwrap();
    let saved = s.execute("library.save", json!({ "folder": dir.path(), "name": "Wave", "nodes": [{ "peg": shoulder }] })).unwrap();
    let path = saved["path"].clone();
    for _ in 0..3 {
        s.undo();
    }
    let peg = |s: &Session| s.project().shots[0].peg(iskra_model::PegId(shoulder)).unwrap().clone();
    let hold = |s: &Session| s.project().layer(iskra_model::LayerId(arm)).unwrap().exposure.runs()[0].len;
    assert!(peg(&s).animation.key_frames().is_empty() && hold(&s) != 4);

    // A look says what would go where, and changes nothing.
    let steps = s.history().entries().len();
    let would = s.execute("library.apply", json!({ "path": path, "peg": shoulder, "preview": true })).unwrap();
    assert_eq!((&would["applied"], &would["skipped"], &would["untouched"]), (&json!(2), &json!(0), &json!([])));
    assert_eq!(
        (&would["matches"][0]["rig"], &would["matches"][1]["item"], &would["matches"][1]["found"]),
        (&json!("shoulder"), &json!("armL"), &json!("matched"))
    );
    assert_eq!(s.history().entries().len(), steps);
    // Applied, the shot's own peg waves and its own layer has the timing: one step, no copies.
    let nodes = (s.project().shots[0].pegs.len(), s.project().shots[0].layers.len());
    s.execute("library.apply", json!({ "path": path, "peg": shoulder })).unwrap();
    assert_eq!((peg(&s).animation.key_frames().into_iter().collect::<Vec<_>>(), peg(&s).transform_at(6).rotation, hold(&s)), (vec![0, 6], 40.0, 4));
    assert_eq!(
        ((s.project().shots[0].pegs.len(), s.project().shots[0].layers.len()), s.history().undo_label(), s.history().entries().len()),
        (nodes, Some("Apply Template"), steps + 1)
    );
    s.undo();
    assert!(peg(&s).animation.key_frames().is_empty());
    // At the playhead, the wave starts there. A layer renamed is missing, until it is matched by hand.
    s.execute("layer.rename", json!({ "layer": arm, "name": "limb" })).unwrap();
    let would = s.execute("library.apply", json!({ "path": path, "peg": shoulder, "frame": 10, "preview": true })).unwrap();
    assert_eq!((&would["skipped"], &would["matches"][1]["found"], &would["untouched"]), (&json!(1), &json!("missing in the shot"), &json!(["limb"])));
    let applied = s.execute("library.apply", json!({ "path": path, "peg": shoulder, "frame": 10, "map": { "armL": "limb" } })).unwrap();
    assert_eq!((&applied["skipped"], &applied["matches"][1]["found"]), (&json!(0), &json!("matched by hand")));
    assert_eq!(peg(&s).animation.key_frames().into_iter().collect::<Vec<_>>(), [10, 16]);
    assert!(peg(&s).matches.is_empty(), "not kept unless asked");
    // Kept with the rig's peg, in the same step, when asked; they are in the file, and an
    // undo takes them back with the rest.
    s.undo();
    s.execute("library.apply", json!({ "path": path, "peg": shoulder, "map": { "armL": "limb" }, "keepMatches": true })).unwrap();
    assert_eq!(peg(&s).matches, [("armL".to_string(), "limb".to_string())].into());
    s.undo();
    assert!(peg(&s).matches.is_empty() && peg(&s).animation.key_frames().is_empty());
    s.redo();
    // Made new, when asked: an armL under the shoulder, with the template's drawing and its
    // timing, in the same step; the renamed limb is left as it is.
    s.undo();
    let made = s.execute("library.apply", json!({ "path": path, "peg": shoulder, "makeMissing": true })).unwrap();
    assert_eq!((&made["skipped"], &made["matches"][1]["found"], &made["matches"][1]["rig"]), (&json!(0), &json!("made new"), &json!("armL")));
    let arm_l = |s: &Session| s.project().shots[0].layers.iter().find(|layer| layer.name == "armL").cloned();
    let new = arm_l(&s).unwrap();
    assert_eq!((new.parent, new.drawings.len(), new.exposure.runs()[0].len), (Some(iskra_model::PegId(shoulder)), 1, 4));
    assert!(*new.drawings[0].drawing != iskra_drawing::Drawing::default() && new.id.0 != arm);
    s.undo();
    assert!(arm_l(&s).is_none());
    assert!(turned_down(s.execute("library.apply", json!({ "path": path, "layer": arm, "keepMatches": true })), "with the `peg`"));
    for (params, why) in [
        (json!({ "path": path, "peg": shoulder, "layer": arm }), "not both"),
        (json!({ "path": path, "peg": 987_654 }), "no such node"),
        (json!({ "path": dir.path().join("Gone.iskratpl") }), "not a template file"),
    ] {
        assert!(turned_down(s.execute("library.apply", params), why), "{why}");
    }
}

#[test]
fn several_commands_are_run_as_one_step_or_not_at_all() {
    let mut s = Session::new();
    let arm = layer(&s);
    let (first, second) = (id(s.execute("peg.new", json!({})).unwrap()), id(s.execute("peg.new", json!({})).unwrap()));
    let turned = |s: &Session, peg: u64| s.project().shots[0].peg(iskra_model::PegId(peg)).unwrap().transform.rotation;
    let steps = s.history().entries().len();
    let both = json!([
        { "id": "peg.setTransform", "params": { "peg": first, "rotation": 10 } },
        { "id": "peg.setTransform", "params": { "peg": second, "rotation": 20 } },
    ]);
    let done = s.execute("edit.batch", json!({ "commands": both })).unwrap();
    assert_eq!((turned(&s, first), turned(&s, second), done["results"].as_array().unwrap().len()), (10.0, 20.0, 2));
    // One step, named after the commands since they are of one kind; undone and redone whole.
    assert_eq!((s.history().entries().len(), s.history().undo_label()), (steps + 1, Some("Rotate Peg")));
    s.undo();
    assert_eq!((turned(&s, first), turned(&s, second)), (0.0, 0.0));
    s.redo();
    assert_eq!((turned(&s, first), turned(&s, second)), (10.0, 20.0));
    // If one is turned down, none is done, and the history is as it was.
    let steps = s.history().entries().len();
    let one_bad = json!([
        { "id": "peg.setTransform", "params": { "peg": first, "rotation": 45 } },
        { "id": "peg.setTransform", "params": { "peg": 424_242, "rotation": 20 } },
    ]);
    assert!(turned_down(s.execute("edit.batch", json!({ "commands": one_bad })), "no peg 424242"));
    assert_eq!((turned(&s, first), s.history().entries().len(), s.history().undo_label()), (10.0, steps, Some("Rotate Peg")));
    // Of several kinds it has a name of its own, or the one it is given.
    let mixed = json!([
        { "id": "peg.setTransform", "params": { "peg": first, "rotation": 5 } },
        { "id": "layer.rename", "params": { "layer": arm, "name": "Arm" } },
    ]);
    s.execute("edit.batch", json!({ "commands": mixed, "title": "Tidy Up" })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Tidy Up"));
    assert!(turned_down(s.execute("edit.batch", json!({ "commands": [] })), "give the `commands`"));
    assert!(turned_down(s.execute("edit.batch", json!({ "commands": [{ "id": "edit.undo" }] })), "is no edit of the project"));
}

#[test]
fn skins_are_brought_onto_a_character_in_the_shot_as_variants_of_its_drawings() {
    use photocraft_psd::{GroupSpec, LayerSpec, PixelData, PsdBuilder};
    let dir = tempfile::tempdir().unwrap();
    let part = |name: &str, color: [u8; 4]| LayerSpec::new(name, 90, 40, 20, 20, PixelData::Rgba8(color.repeat(400)));
    let write = |name: &str, builder: PsdBuilder| {
        let path = dir.path().join(name);
        std::fs::write(&path, builder.to_bytes().unwrap()).unwrap();
        path
    };
    // The character: a body and a slot of two mouths, under its peg.
    let mut builder = PsdBuilder::new(200, 100);
    builder.push_layer(part("body", [255, 0, 0, 255]));
    builder.begin_group(GroupSpec::new("[slot] mouth"));
    builder.push_layer(part("A", [0, 0, 255, 255]));
    builder.push_layer(part("O", [255, 255, 0, 255]));
    builder.end_group().unwrap();
    let hero = write("hero.psd", builder);
    let mut s = Session::new();
    let top = s.execute("character.import", json!({ "path": hero })).unwrap()["peg"].as_u64().unwrap();
    let layers = s.project().shots[0].layers.len();

    // A file of the night parts, with no tags: the skin is named in the import.
    let mut builder = PsdBuilder::new(200, 100);
    builder.push_layer(part("body", [0, 0, 90, 255]));
    builder.begin_group(GroupSpec::new("[slot] mouth"));
    builder.push_layer(part("O", [90, 90, 0, 255]));
    builder.end_group().unwrap();
    let night = write("night.psd", builder);
    assert!(turned_down(s.execute("skin.import", json!({ "path": night, "peg": top })), "say which skin"));
    let made = s.execute("skin.import", json!({ "path": night, "peg": top, "name": "Night" })).unwrap();
    assert_eq!((s.project().shots[0].layers.len(), s.history().undo_label()), (layers, Some("Import Skins")), "no layer is made");
    let skin = s.project().skin_named("Night").unwrap().id;
    let shot = s.project().shots[0].clone();
    let named = |shot: &iskra_model::Shot, name: &str| shot.layers.iter().find(|layer| layer.name == name).unwrap().clone();
    let (body, mouth) = (named(&shot, "body"), named(&shot, "mouth"));
    assert!(body.variant(skin, body.drawings[0].id).is_some());
    assert_eq!(mouth.variants.len(), 1);
    assert!(made["said"].to_string().contains("Night has no variant of mouth A"), "{}", made["said"]);

    // Brought again, a part takes the place of the variant it has.
    let before = body.variants[0].drawing.id;
    s.execute("skin.import", json!({ "path": night, "peg": top, "name": "Night" })).unwrap();
    let body = named(&s.project().shots[0], "body");
    assert!(body.variants.len() == 1 && body.variants[0].drawing.id != before);
}

#[test]
fn a_characters_skins_come_in_as_variants_of_its_parts() {
    use photocraft_psd::{GroupSpec, LayerSpec, PixelData, PsdBuilder};
    let dir = tempfile::tempdir().unwrap();
    let part = |name: &str, color: [u8; 4]| LayerSpec::new(name, 90, 40, 20, 20, PixelData::Rgba8(color.repeat(400)));
    // A body and a slot of two mouths; Night's body and O mouth, and a hat no day part has; a
    // body tagged Dusk alone.
    let mut builder = PsdBuilder::new(200, 100);
    builder.push_layer(part("body", [255, 0, 0, 255]));
    builder.begin_group(GroupSpec::new("[slot] mouth"));
    builder.push_layer(part("A", [0, 0, 255, 255]));
    builder.push_layer(part("O", [255, 255, 0, 255]));
    builder.end_group().unwrap();
    builder.begin_group(GroupSpec::new("[skin Night]"));
    builder.push_layer(part("body", [0, 0, 90, 255]));
    builder.push_layer(part("hat", [9, 9, 9, 255]));
    builder.begin_group(GroupSpec::new("[slot] mouth"));
    builder.push_layer(part("O", [90, 90, 0, 255]));
    builder.end_group().unwrap();
    builder.end_group().unwrap();
    builder.push_layer(part("[skin Dusk] body", [120, 0, 60, 255]));
    let path = dir.path().join("hero.psd");
    std::fs::write(&path, builder.to_bytes().unwrap()).unwrap();

    let mut s = Session::new();
    let made = s.execute("character.import", json!({ "path": path })).unwrap();
    let shot = s.project().shots[0].clone();
    let named = |name: &str| shot.layers.iter().find(|layer| layer.name == name).unwrap().clone();
    assert!(shot.layers.iter().all(|layer| layer.name != "hat"), "a skin's part is no layer of its own");
    let names: Vec<&str> = s.project().skins().iter().map(|skin| skin.name.as_str()).collect();
    assert_eq!(names, ["Night", "Dusk"]);
    let (night, dusk) = (s.project().skin_named("Night").unwrap().id, s.project().skin_named("Dusk").unwrap().id);
    let (body, mouth) = (named("body"), named("mouth"));
    assert!(body.variant(night, body.drawings[0].id).is_some() && body.variant(dusk, body.drawings[0].id).is_some());
    let o = mouth.drawings.iter().find(|drawing| drawing.name == "O").unwrap().id;
    assert_eq!(mouth.variants.iter().map(|variant| (variant.skin, variant.of)).collect::<Vec<_>>(), [(night, o)], "Night has an O mouth alone");
    let said = made["skipped"].to_string();
    assert!(said.contains("hat in Night"), "{said}");

    // A file whose parts are all in skins is brought in once it is said which is the day one.
    let mut builder = PsdBuilder::new(200, 100);
    builder.push_layer(part("[skin Day] body", [255, 0, 0, 255]));
    builder.push_layer(part("[skin Night] body", [0, 0, 90, 255]));
    let path = dir.path().join("skins.psd");
    std::fs::write(&path, builder.to_bytes().unwrap()).unwrap();
    let mut s = Session::new();
    assert!(turned_down(s.execute("character.import", json!({ "path": path })), "Day, Night"));
    s.execute("character.import", json!({ "path": path, "day": "Day" })).unwrap();
    let body = s.project().shots[0].layers.iter().find(|layer| layer.name == "body").unwrap().clone();
    let night = s.project().skin_named("Night").unwrap().id;
    assert!(body.variant(night, body.drawings[0].id).is_some() && s.project().skin_named("Day").is_none());
}

#[test]
fn a_character_is_brought_in_from_a_layered_file_by_the_tags_in_its_names() {
    use photocraft_psd::{GroupSpec, LayerSpec, PixelData, PsdBuilder};
    let dir = tempfile::tempdir().unwrap();
    let part = |name: &str, left: i32, top: i32, color: [u8; 4]| LayerSpec::new(name, left, top, 20, 20, PixelData::Rgba8(color.repeat(400)));
    // A document of 200 by 100: a body, an arm in a group, a slot of two mouths, the second
    // of which is seen, a hidden prop, and notes to leave out.
    let mut builder = PsdBuilder::new(200, 100);
    builder.push_layer(part("body", 90, 40, [255, 0, 0, 255]));
    builder.begin_group(GroupSpec::new("arm"));
    builder.push_layer(part("hand", 120, 40, [0, 255, 0, 255]));
    builder.end_group().unwrap();
    builder.begin_group(GroupSpec::new("[slot] mouth"));
    builder.push_layer(LayerSpec { visible: false, ..part("A", 90, 70, [0, 0, 255, 255]) });
    builder.push_layer(part("O", 90, 70, [255, 255, 0, 255]));
    builder.end_group().unwrap();
    builder.push_layer(LayerSpec { visible: false, ..part("prop", 0, 0, [9, 9, 9, 255]) });
    builder.push_layer(part("[ignore] notes", 0, 0, [255, 255, 255, 255]));
    let path = dir.path().join("hero.psd");
    std::fs::write(&path, builder.to_bytes().unwrap()).unwrap();

    let mut s = Session::new();
    let own = layer(&s);
    let made = s.execute("character.import", json!({ "path": path })).unwrap();
    let shot = s.project().shots[0].clone();
    let named = |name: &str| shot.layers.iter().find(|layer| layer.name == name).unwrap_or_else(|| panic!("a layer named {name}")).clone();
    let peg = |name: &str| shot.pegs.iter().find(|peg| peg.name == name).unwrap_or_else(|| panic!("a peg named {name}")).clone();
    // One peg over all of it, named after the file, and a peg over every layer. The arm's
    // group has no tag and is looked through: its hand lies beside the body. The notes are
    // left out.
    assert_eq!((made["layers"].as_array().unwrap().len(), made["pegs"].as_array().unwrap().len(), &made["skipped"]), (4, 4, &json!([])));
    assert_eq!((made["peg"].as_u64(), s.history().undo_label()), (Some(peg("hero").id.0), Some("Import Character")));
    for name in ["body", "hand", "mouth", "prop"] {
        let over = peg(&format!("{name}-P"));
        assert_eq!((named(name).parent, over.parent), (Some(over.id), Some(peg("hero").id)), "{name}");
    }
    assert!(shot.layers.iter().all(|layer| !layer.name.contains("notes")) && shot.layers.len() == 5 && shot.pegs.len() == 6);
    // The slot is one layer with a drawing a mouth, named as the mouths are, showing the one
    // that is seen in the file.
    let mouth = named("mouth");
    assert_eq!(mouth.drawings.iter().map(|drawing| drawing.name.as_str()).collect::<Vec<_>>(), ["A", "O"]);
    assert_eq!((mouth.drawing_at(0).map(|drawing| drawing.name.as_str()), named("body").drawings[0].name.as_str()), (Some("O"), "body"));
    // Its pictures' line art and overart are drawn on in free colour, as bitmap art is.
    assert!(named("body").drawings[0].drawing.art_layers.iter().all(|art| art.content.is_rgba()));
    // Stacked as in the file, over what the shot had; the prop, though it is hidden in the
    // file, is in the picture like the rest.
    assert_eq!(shot.stack(), [named("prop").id, mouth.id, named("hand").id, named("body").id, iskra_model::LayerId(own)]);
    // Where it is in the document: the document's middle is the frame's, so the body, 90 to
    // 110 across and 40 to 60 down in a document of 200 by 100, lies about the frame's middle.
    let picture = dir.path().join("frame.png");
    s.execute("shot.renderFrame", json!({ "frame": 0, "path": picture })).unwrap();
    let image = image::open(&picture).unwrap().to_rgba8();
    let at = |x: i64, y: i64| image.get_pixel((960 + x) as u32, (540 + y) as u32).0;
    assert_eq!((at(0, 0), at(-9, -9), at(-12, 0)[3]), ([255, 0, 0, 255], [255, 0, 0, 255], 0), "the body");
    assert_eq!((at(30, 0), at(0, 30), at(-95, -45)), ([0, 255, 0, 255], [255, 255, 0, 255], [9, 9, 9, 255]), "the hand, the mouth that is seen, and the prop");
    // One step; and what is no such file is said.
    s.undo();
    assert_eq!((s.project().shots[0].layers.len(), s.project().shots[0].pegs.len()), (1, 1), "the camera's peg alone");
    assert!(turned_down(s.execute("character.import", json!({ "path": dir.path().join("hero.png") })), "not a Photoshop file"));
    assert!(turned_down(s.execute("character.import", json!({ "path": dir.path().join("gone.psd") })), "could not be read"));
}

#[test]
fn a_frame_is_rendered_to_a_picture_file_without_a_window() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    s.execute("drawing.stroke", stroke_params(layer(&s), 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
    let path = dir.path().join("frame.png");
    let made = s.execute("shot.renderFrame", json!({ "path": path, "scale": 0.5 })).unwrap();
    assert_eq!(made, json!({ "width": 960, "height": 540 }));
    let (width, _, pixels) = iskra_io::decode_png(&std::fs::read(&path).unwrap()).unwrap();
    // The stroke is from (50, 50) to (200, 50) of the scene, whose origin is the frame's middle:
    // at half size, around (480 + 60, 270 + 25).
    assert_eq!((pixels[(295 * width + 540) as usize], pixels[(200 * width + 540) as usize]), ([220, 50, 47, 255], [0, 0, 0, 0]));
    assert!(s.execute("shot.renderFrame", json!({ "path": path, "frame": 9 })).is_ok(), "an empty frame is an empty picture");
    assert!(turned_down(s.execute("shot.renderFrame", json!({ "path": path, "scale": 0.0 })), "can't be made"));
    assert!(turned_down(s.execute("shot.renderFrame", json!({ "path": path, "scale": 100.0 })), "can't be made"));
    assert!(!s.history().can_undo() || s.history().undo_label() == Some("Brush Stroke"));
}

#[test]
fn a_render_is_of_the_animation_and_an_animatic_of_the_boards() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    s.execute("project.setResolution", json!({ "width": 640, "height": 360 })).unwrap();
    s.execute("shot.setDuration", json!({ "frames": 2 })).unwrap();
    let animation = layer(&s);
    let board = id(s.execute("layer.new", json!({ "name": "Roughs", "role": "board" })).unwrap());
    s.execute("drawing.stroke", stroke_params(board, 0, json!({ "size": 6.0 }))).unwrap();
    let painted = |file: &std::path::Path| image::open(file).unwrap().into_rgba8().pixels().any(|pixel| pixel.0[3] > 0);
    let frame = |s: &mut Session, work: Option<&str>| {
        let path = dir.path().join("frame.png");
        s.execute("shot.renderFrame", json!({ "path": path, "work": work })).unwrap();
        painted(&path)
    };
    // Only the boards are drawn on: a frame of the shot has them, one of its animation has not.
    assert_eq!((frame(&mut s, None), frame(&mut s, Some("boards")), frame(&mut s, Some("animation")), frame(&mut s, Some("whole"))), (true, true, false, true));

    // A render is of the animation, and says so when the shot has only boards to show.
    let out = dir.path().join("Renders");
    let first = out.join("Shot_01").join("Shot_01.0001.png");
    let made = s.execute("export.sequence", json!({ "folder": out })).unwrap();
    let notes = |made: &Value| made["sequences"][0]["notes"].as_array().unwrap().iter().map(|note| note.as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert!(!painted(&first) && notes(&made).len() == 1 && notes(&made)[0].contains("only boards"), "{:?}", notes(&made));
    // An animatic is of the boards.
    let made = s.execute("export.sequence", json!({ "folder": out, "settings": { "work": "boards" }, "ifExists": "replace" })).unwrap();
    assert!(painted(&first) && notes(&made).is_empty());

    // Drawn on, the animation is what a render shows, and the boards are left out of it.
    s.execute("drawing.stroke", stroke_params(animation, 1, json!({ "size": 6.0 }))).unwrap();
    let made = s.execute("export.sequence", json!({ "folder": out, "ifExists": "replace" })).unwrap();
    assert!(!painted(&first) && painted(&out.join("Shot_01").join("Shot_01.0002.png")) && notes(&made).is_empty());
    // A layer in a group that is the storyboard's is a board, whatever it says of itself.
    let group = id(s.execute("group.new", json!({ "nodes": [animation] })).unwrap());
    s.execute("group.set", json!({ "group": group, "board": true })).unwrap();
    let made = s.execute("export.sequence", json!({ "folder": out, "ifExists": "replace" })).unwrap();
    assert!(!painted(&out.join("Shot_01").join("Shot_01.0002.png")) && notes(&made).len() == 1);
}

#[test]
fn a_board_is_copied_into_an_animation_layer_that_shares_nothing_with_it() {
    let mut s = Session::new();
    let animation = layer(&s);
    let board = id(s.execute("layer.new", json!({ "name": "Roughs", "role": "board" })).unwrap());
    s.execute("drawing.stroke", stroke_params(board, 0, json!({ "size": 6.0 }))).unwrap();
    s.execute("drawing.stroke", stroke_params(board, 5, json!({ "size": 6.0 }))).unwrap();
    let copy = id(s.execute("layer.useAsAnimation", json!({ "layer": board })).unwrap());
    let of = |s: &Session, layer: u64| s.project().layer(iskra_model::LayerId(layer)).unwrap().clone();
    let (from, made) = (of(&s, board), of(&s, copy));
    assert_eq!((made.name.as_str(), made.role, from.role), ("Roughs Animation", Role::Animation, Role::Board));
    assert_eq!((cells(&s, copy, 8), s.project().shots[0].stack()[0].0), (cells(&s, board, 8), copy), "the same timing, on top");
    assert!(made.drawings.iter().zip(&from.drawings).all(|(new, old)| new.id != old.id && new.drawing == old.drawing));
    // Drawn on, the copy changes and the board stays as it was.
    s.execute("drawing.stroke", stroke_params(copy, 0, json!({ "size": 30.0 }))).unwrap();
    assert!(of(&s, board) == from && of(&s, copy).drawings[0].drawing != from.drawings[0].drawing);
    // Only a board is copied so, and a name that is taken is refused.
    assert!(turned_down(s.execute("layer.useAsAnimation", json!({ "layer": animation })), "an animation layer already"));
    assert!(turned_down(s.execute("layer.useAsAnimation", json!({ "layer": board, "name": "Roughs" })), "already has a layer named"));
    let again = id(s.execute("layer.useAsAnimation", json!({ "layer": board })).unwrap());
    assert_eq!(of(&s, again).name, "Roughs Animation 2");
}

#[test]
fn a_shot_has_panels_that_are_written_of_moved_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let shot = s.project().shots[0].id.0;
    let panels = |s: &mut Session| {
        let listed = s.execute("panel.list", json!({ "shot": shot })).unwrap();
        let of = |panel: &Value| (panel["frame"].as_u64().unwrap(), panel["end"].as_u64().unwrap(), panel["action"].as_str().unwrap().to_string());
        listed["panels"].as_array().unwrap().iter().map(of).collect::<Vec<_>>()
    };
    let text = |action: &str| action.to_string();
    // A shot is one panel to begin with, the whole of it, with nothing written.
    assert_eq!(panels(&mut s), [(0, 48, text(""))]);
    let revision = s.project().shots[0].revision;

    // Panels are added on frames of the shot, and each lasts until the next.
    s.execute("panel.add", json!({ "shot": shot, "frame": 24, "action": "She turns" })).unwrap();
    s.execute("panel.add", json!({ "frame": 12 })).unwrap();
    assert_eq!(panels(&mut s), [(0, 12, text("")), (12, 24, text("")), (24, 48, text("She turns"))]);
    assert!(turned_down(s.execute("panel.add", json!({ "frame": 12 })), "already"));
    assert!(turned_down(s.execute("panel.add", json!({ "frame": 0 })), "already"), "the first is always there");
    assert!(turned_down(s.execute("panel.add", json!({ "frame": 48 })), "has no frame 48"));

    // What is said of one is written, each text by itself; the first panel's as any other's.
    s.execute("panel.set", json!({ "frame": 12, "action": "The door opens", "dialogue": "Hello?  " })).unwrap();
    s.execute("panel.set", json!({ "frame": 12, "notes": "creak" })).unwrap();
    s.execute("panel.set", json!({ "action": "A dark hall" })).unwrap();
    let second = s.project().shots[0].panel_on(12).unwrap();
    assert_eq!((second.action.as_str(), second.dialogue.as_str(), second.notes.as_str()), ("The door opens", "Hello?", "creak"));
    assert_eq!((panels(&mut s)[0].2.as_str(), s.history().undo_label()), ("A dark hall", Some("Write Panel Notes")));
    assert!(turned_down(s.execute("panel.set", json!({ "frame": 13, "action": "x" })), "no panel"));
    assert!(turned_down(s.execute("panel.set", json!({ "frame": 12 })), "say what to write"));
    // What is said of a shot is no change to what it shows: nothing rendered is stale by it.
    assert_eq!(s.project().shots[0].revision, revision);

    // A panel is moved between its neighbours, and removed; not the first.
    s.execute("panel.move", json!({ "frame": 12, "to": 18 })).unwrap();
    assert_eq!(panels(&mut s).iter().map(|panel| (panel.0, panel.1)).collect::<Vec<_>>(), [(0, 18), (18, 24), (24, 48)]);
    assert!(turned_down(s.execute("panel.move", json!({ "frame": 18, "to": 24 })), "between its neighbours"));
    assert!(turned_down(s.execute("panel.move", json!({ "frame": 18, "to": 0 })), "between its neighbours"));
    assert!(turned_down(s.execute("panel.move", json!({ "frame": 0, "to": 4 })), "the shot's own"));
    assert!(turned_down(s.execute("panel.remove", json!({ "frame": 0 })), "the shot's own"));
    s.execute("panel.remove", json!({ "frame": 24 })).unwrap();
    assert_eq!(panels(&mut s), [(0, 18, text("A dark hall")), (18, 48, text("The door opens"))]);
    s.undo();
    assert_eq!(panels(&mut s).len(), 3);
    s.redo();

    // They are kept in the project file, and a copy of the shot has them too.
    let path = dir.path().join("Film.iskra");
    s.execute("file.saveAs", json!({ "path": path })).unwrap();
    let mut opened = Session::new();
    opened.execute("file.open", json!({ "path": path })).unwrap();
    assert_eq!(panels(&mut opened), panels(&mut s));
    let copy = id(s.execute("shot.duplicate", json!({ "shot": shot })).unwrap());
    assert_eq!(s.project().shot(ShotId(copy)).unwrap().panels, s.project().shots[0].panels);
}

#[test]
fn a_shot_is_split_in_two_and_two_that_touch_are_merged_into_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let first = s.project().shots[0].id.0;
    let board = id(s.execute("layer.new", json!({ "name": "Roughs", "role": "board" })).unwrap());
    for frame in [0, 30] {
        s.execute("drawing.stroke", stroke_params(board, frame, json!({ "size": 6.0 }))).unwrap();
    }
    s.execute("panel.add", json!({ "frame": 30, "action": "She turns" })).unwrap();
    let cut = |s: &Session| {
        let project = s.project();
        let clips = project.storyboards[0].clips().iter();
        clips
            .map(|clip| format!("{} {}-{} from {}", project.shot(clip.shot).unwrap().name, clip.start, clip.end(), clip.in_point))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let painted = |s: &mut Session, shot: u64, frame: u32| {
        let path = dir.path().join("frame.png");
        s.execute("shot.renderFrame", json!({ "shot": shot, "frame": frame, "path": path, "scale": 0.25 })).unwrap();
        image::open(&path).unwrap().into_rgba8().pixels().any(|pixel| pixel.0[3] > 0)
    };

    // Split, the shot is two: each the whole of it, the second's cut starting where the first's ends.
    assert!(turned_down(s.execute("shot.split", json!({ "shot": first, "frame": 0 })), "inside its cut"));
    assert!(turned_down(s.execute("shot.split", json!({ "shot": first, "frame": 48 })), "inside its cut"));
    let second = id(s.execute("shot.split", json!({ "shot": first, "frame": 24 })).unwrap());
    assert_eq!((cut(&s).as_str(), s.history().undo_label()), ("Shot_01 0-24 from 0, Shot_02 24-48 from 24", Some("Split Shot")));
    let (a, b) = (s.project().shot(ShotId(first)).unwrap().clone(), s.project().shot(ShotId(second)).unwrap().clone());
    assert!(a.duration == 48 && b.duration == 48 && a.panels == b.panels && a.layers[0].id != b.layers[0].id, "nothing is deleted, and nothing shared");
    assert!(painted(&mut s, second, 30));
    s.undo();
    assert_eq!((cut(&s).as_str(), s.project().shots.len()), ("Shot_01 0-48 from 0", 1));
    s.redo();

    // A new shot goes right after another, as long as that one is in the cut, with a board to
    // draw on; where there is no room it goes after the last.
    let third = id(s.execute("shot.new", json!({ "after": second, "boards": true })).unwrap());
    assert_eq!(cut(&s), "Shot_01 0-24 from 0, Shot_02 24-48 from 24, Shot_03 48-72 from 0");
    let made = s.project().shot(ShotId(third)).unwrap().clone();
    let names: Vec<&str> = made.layers.iter().map(|layer| layer.name.as_str()).collect();
    assert!(names == ["Background", "Character"] && made.layers.iter().all(|layer| made.is_board(layer.id)));
    assert_eq!(made.stack(), [made.layers[1].id, made.layers[0].id], "Character over Background");
    let fourth = id(s.execute("shot.new", json!({ "after": first })).unwrap());
    assert!(cut(&s).ends_with("Shot_04 72-96 from 0"), "{}", cut(&s));
    s.execute("shot.delete", json!({ "shot": fourth })).unwrap();
    let fourth = id(s.execute("shot.new", json!({ "after": first, "ripple": true, "frames": 6 })).unwrap());
    assert_eq!(cut(&s), "Shot_01 0-24 from 0, Shot_04 24-30 from 0, Shot_02 30-54 from 24, Shot_03 54-78 from 0");
    s.execute("shot.delete", json!({ "shot": fourth })).unwrap();
    s.execute("storyboard.moveClip", json!({ "shot": second, "start": 24 })).unwrap();

    // Merged, the two are one shot again, as long in the cut as both: the second's layers are
    // in the first, showing from the join on, and the first's show up to it.
    assert!(turned_down(s.execute("shot.merge", json!({ "shot": third })), "no shot follows"));
    s.execute("shot.merge", json!({ "shot": first })).unwrap();
    assert_eq!((cut(&s).as_str(), s.project().shots.len()), ("Shot_01 0-48 from 0, Shot_03 54-78 from 0", 2));
    let merged = s.project().shot(ShotId(first)).unwrap().clone();
    let names: Vec<&str> = merged.layers.iter().map(|layer| layer.name.as_str()).collect();
    assert_eq!((names, merged.duration, merged.stack().len()), (vec!["Roughs 2", "Drawing 2", "Roughs", "Drawing"], 48, 4));
    let shown = |name: &str, frame: u32| merged.layers.iter().find(|layer| layer.name == name).unwrap().exposure.at(frame).is_some();
    assert_eq!((shown("Roughs", 0), shown("Roughs", 30), shown("Roughs 2", 0), shown("Roughs 2", 30)), (true, false, false, true));
    assert!(painted(&mut s, first, 0) && painted(&mut s, first, 30) && merged.is_board(merged.layers[0].id));
    // Each half's drawing that shows nothing any more is gone from its pool: the first's on 30,
    // past the join, and the second's on 0, before it.
    let pooled = |name: &str| merged.layers.iter().find(|layer| layer.name == name).unwrap().drawings.len();
    assert_eq!((pooled("Roughs"), pooled("Roughs 2")), (1, 1));
    // Its panels are the first's up to the join, one at the join, and the second's after it.
    let panels: Vec<(u32, &str)> = merged.panels.iter().map(|panel| (panel.frame, panel.action.as_str())).collect();
    assert_eq!(panels, [(24, ""), (30, "She turns")]);
    // Shots with a gap between them are not merged.
    assert!(turned_down(s.execute("shot.merge", json!({ "shot": first })), "no shot follows"));
    s.undo();
    assert_eq!(s.project().shots.len(), 3);

    // The first cut short, at 12, and the second moved up to it: the second has 24 frames
    // before its in point, more than the first's 12 up to its out point. Merged, the first
    // goes 12 frames later, with its in point; the join is on the second's in point.
    s.execute("storyboard.trimClip", json!({ "shot": first, "out": 12 })).unwrap();
    s.execute("storyboard.moveClip", json!({ "shot": second, "start": 12 })).unwrap();
    assert!(cut(&s).starts_with("Shot_01 0-12 from 0, Shot_02 12-36 from 24"), "{}", cut(&s));
    s.execute("shot.merge", json!({ "shot": first })).unwrap();
    assert!(cut(&s).starts_with("Shot_01 0-36 from 12"), "{}", cut(&s));
    let merged = s.project().shot(ShotId(first)).unwrap().clone();
    let shown = |name: &str, frame: u32| merged.layers.iter().find(|layer| layer.name == name).unwrap().exposure.at(frame).is_some();
    // The first's stroke on 0 is on 12 now; the second's on 30 is where it was.
    assert_eq!((shown("Roughs", 12), shown("Roughs", 0), shown("Roughs 2", 30), shown("Roughs 2", 23)), (true, false, true, false));
    assert_eq!(merged.duration, 60, "the first's 48 and the 12 it went later");
}

#[test]
fn the_camera_is_moved_by_a_peg_and_sees_what_is_near_larger() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    s.execute("project.setResolution", json!({ "width": 320, "height": 180 })).unwrap();
    let shot_id = id(s.execute("shot.new", json!({ "name": "Wide" })).unwrap());
    // Two layers, each with the same stroke: one stays at depth 0, one is brought near.
    let far = id(s.execute("layer.new", json!({ "shot": shot_id, "name": "Far" })).unwrap());
    let near = id(s.execute("layer.new", json!({ "shot": shot_id, "name": "Near" })).unwrap());
    for layer in [far, near] {
        s.execute("drawing.stroke", stroke_params(layer, 0, json!({ "size": 8, "pressureSize": false }))).unwrap();
    }
    let lift = id(s.execute("peg.new", json!({ "aboveLayer": near })).unwrap());
    let shot = |s: &Session| s.project().shot(ShotId(shot_id)).unwrap().clone();
    let distance = shot(&s).camera.distance;
    assert!((distance - 240.0).abs() < 1.0, "a new shot's camera has the usual lens over the shot's height: {distance}");
    s.execute("peg.setTransform", json!({ "peg": lift, "depth": distance / 2.0 })).unwrap();
    // Where the stroke's left end is seen: the first painted pixel of row `y` of the frame.
    let left_end = |s: &mut Session, y: u32| {
        let path = dir.path().join("frame.png");
        s.execute("shot.renderFrame", json!({ "shot": shot_id, "path": path })).unwrap();
        let (width, _, pixels) = iskra_io::decode_png(&std::fs::read(&path).unwrap()).unwrap();
        (0..width).find(|x| pixels[(y * width + x) as usize][3] > 128)
    };
    // The stroke runs from (50, 50) of the scene, which is (210, 140) of the frame. Half way
    // to the camera it is twice as far from the middle: off the frame, with its row.
    assert_eq!((left_end(&mut s, 140).map(|x| x.abs_diff(208) <= 2), left_end(&mut s, 170)), (Some(true), None));
    // Pulled back by its peg, the camera sees both: the near one at its drawn size again.
    let rig = id(s.execute("peg.new", json!({ "shot": shot_id, "aboveCamera": true })).unwrap());
    assert_eq!(
        (shot(&s).camera.parent.map(|peg| peg.0), shot(&s).peg(iskra_model::PegId(rig)).map(|peg| peg.name.clone())),
        (Some(rig), Some("Camera-P 2".to_string()))
    );
    s.execute("peg.setTransform", json!({ "peg": rig, "depth": distance / 2.0 })).unwrap();
    let seen = [left_end(&mut s, 140), left_end(&mut s, 123)];
    assert!(seen[0].is_some_and(|x| x.abs_diff(208) <= 2) && seen[1].is_some_and(|x| x.abs_diff(192) <= 2), "{seen:?}: near as drawn, far at two thirds");
    // Panned right, what is near goes farther left than what is far: parallax.
    s.execute("peg.setTransform", json!({ "peg": rig, "position": [30.0, 0.0] })).unwrap();
    let panned = [left_end(&mut s, 140), left_end(&mut s, 123)];
    assert!(panned[0].is_some_and(|x| x.abs_diff(178) <= 2) && panned[1].is_some_and(|x| x.abs_diff(172) <= 2), "{panned:?}");

    // The lens: zoomed in, the frame's middle stays and the rest moves out; a wider field of
    // view stands the camera nearer, and leaves the plane at depth 0 filling the frame.
    s.execute("camera.setParent", json!({ "shot": shot_id })).unwrap();
    s.execute("peg.setTransform", json!({ "peg": lift, "depth": 0.0 })).unwrap();
    s.execute("camera.setLens", json!({ "shot": shot_id, "fieldOfView": 90.0 })).unwrap();
    assert!((shot(&s).camera.distance - 90.0).abs() < 1e-9 && (shot(&s).camera.field_of_view(180, 0) - 90.0).abs() < 1e-9);
    assert!(left_end(&mut s, 140).is_some_and(|x| x.abs_diff(208) <= 2), "at depth 0 nothing changes");
    s.execute("camera.setLens", json!({ "shot": shot_id, "zoom": 0.5 })).unwrap();
    assert!(left_end(&mut s, 115).is_some_and(|x| x.abs_diff(184) <= 2));
    assert_eq!((shot(&s).camera.zoom, s.history().undo_label()), (0.5, Some("Set Camera Lens")));
    // The lens is keyed like a peg: once it has keys, setting it on another frame keys it
    // there, and between the keys it goes from one to the other.
    s.execute("camera.setKey", json!({ "shot": shot_id, "frame": 0 })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Add Lens Keyframe"));
    assert!(turned_down(s.execute("camera.setLens", json!({ "shot": shot_id, "zoom": 2.0 })), "say which `frame`"));
    s.execute("camera.setLens", json!({ "shot": shot_id, "frame": 10, "zoom": 2.0 })).unwrap();
    let zoom = |s: &Session, frame: u32| shot(s).camera.zoom_at(frame);
    assert_eq!((zoom(&s, 0), zoom(&s, 10), zoom(&s, 30)), (0.5, 2.0, 2.0));
    assert!(zoom(&s, 5) > 0.5 && zoom(&s, 5) < 2.0, "{}", zoom(&s, 5));
    assert_eq!(shot(&s).camera.field_of_view(180, 5), 90.0, "the field of view has the one key, and stays");
    // Its last key taken away, a value stays as that key had it.
    s.execute("camera.setKey", json!({ "shot": shot_id, "frame": 0, "remove": true })).unwrap();
    s.execute("camera.setKey", json!({ "shot": shot_id, "frame": 10, "remove": true })).unwrap();
    assert_eq!((shot(&s).camera.zoom, shot(&s).camera.animation.is_empty(), s.history().undo_label()), (2.0, true, Some("Remove Lens Keyframe")));
    assert!(turned_down(s.execute("camera.setKey", json!({ "shot": shot_id, "frame": 3, "remove": true })), "no keyframe on frame 3"));
    s.execute("camera.setLens", json!({ "shot": shot_id, "zoom": 0.5 })).unwrap();
    // The whole camera is keyed in one step: its peg and its lens together, and so moved,
    // eased and removed.
    let pan = s.execute("peg.new", json!({ "shot": shot_id, "aboveCamera": true })).unwrap()["id"].as_u64().unwrap();
    s.execute("camera.setKey", json!({ "shot": shot_id, "frame": 2, "withPeg": true })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Add Camera Keyframe"));
    s.execute("peg.setTransform", json!({ "peg": pan, "frame": 8, "position": [40.0, 0.0] })).unwrap();
    s.execute("camera.setLens", json!({ "shot": shot_id, "frame": 12, "zoom": 2.0 })).unwrap();
    let keys = |s: &Session| shot(s).camera_keys().into_iter().collect::<Vec<_>>();
    assert_eq!(keys(&s), [2, 8, 12], "the peg's keys and the lens's, as the camera's");
    s.execute("camera.moveKey", json!({ "shot": shot_id, "from": 2, "to": 4 })).unwrap();
    let of_peg = shot(&s).peg(iskra_model::PegId(pan)).unwrap().animation.key_frames().into_iter().collect::<Vec<_>>();
    assert_eq!((keys(&s), of_peg), (vec![4, 8, 12], vec![4, 8]));
    assert_eq!(shot(&s).camera.animation.frames().into_iter().collect::<Vec<_>>(), [4, 12]);
    s.execute("camera.setEase", json!({ "shot": shot_id, "frame": 4, "out": "hold" })).unwrap();
    assert_eq!(shot(&s).camera_interp(4).map(|(_, out)| out), Some(iskra_model::Interp::Hold));
    assert_eq!((shot(&s).camera.zoom_at(11), shot(&s).camera.zoom_at(12)), (0.5, 2.0), "the lens holds too");
    // The lens alone, as its row on the Animation timeline moves and eases it: the peg's keys stay.
    s.execute("camera.moveKey", json!({ "shot": shot_id, "from": 4, "to": 6, "lensOnly": true })).unwrap();
    let of_peg = shot(&s).peg(iskra_model::PegId(pan)).unwrap().animation.key_frames().into_iter().collect::<Vec<_>>();
    assert_eq!((shot(&s).camera.animation.frames().into_iter().collect::<Vec<_>>(), of_peg), (vec![6, 12], vec![4, 8]));
    s.execute("camera.setEase", json!({ "shot": shot_id, "frame": 6, "out": "linear", "lensOnly": true })).unwrap();
    assert_eq!(shot(&s).peg(iskra_model::PegId(pan)).unwrap().animation.interp(4).map(|(_, out)| out), Some(iskra_model::Interp::Hold));
    assert!(turned_down(s.execute("camera.moveKey", json!({ "shot": shot_id, "from": 8, "to": 9, "lensOnly": true })), "the lens has no keyframe on frame 8"));
    s.execute("camera.moveKey", json!({ "shot": shot_id, "from": 6, "to": 4, "lensOnly": true })).unwrap();
    s.execute("camera.setKey", json!({ "shot": shot_id, "frame": 4, "remove": true, "withPeg": true })).unwrap();
    assert_eq!((keys(&s), s.history().undo_label()), (vec![8, 12], Some("Remove Camera Keyframe")));
    assert!(turned_down(s.execute("camera.moveKey", json!({ "shot": shot_id, "from": 3, "to": 5 })), "no keyframe on frame 3"));
    assert!(turned_down(s.execute("camera.setKey", json!({ "shot": shot_id, "frame": 3, "remove": true, "withPeg": true })), "the camera has no keyframe"));
    s.execute("peg.delete", json!({ "peg": pan })).unwrap();
    s.execute("camera.setKey", json!({ "shot": shot_id, "frame": 12, "remove": true })).unwrap();
    // Deleting the camera's peg leaves the camera where the peg's own parent has it.
    s.execute("camera.setParent", json!({ "shot": shot_id, "parent": rig })).unwrap();
    s.execute("peg.delete", json!({ "peg": rig })).unwrap();
    assert_eq!(shot(&s).camera.parent, Some(shot(&s).pegs[0].id), "the peg the shot gave its camera");
    assert!(turned_down(s.execute("camera.setParent", json!({ "shot": shot_id, "parent": 999 })), "no peg 999"));
    assert!(turned_down(s.execute("camera.setLens", json!({ "zoom": 0.0 })), "zoom is outside"));
    assert!(turned_down(s.execute("camera.setLens", json!({ "fieldOfView": 179.0 })), "field of view is outside"));
    assert!(turned_down(s.execute("peg.new", json!({ "aboveCamera": true, "aboveLayer": near })), "goes above nothing else"));
}

#[test]
fn moved_back_with_its_size_kept_a_layer_looks_the_same_until_the_camera_moves() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    s.execute("project.setResolution", json!({ "width": 320, "height": 180 })).unwrap();
    let shot_id = id(s.execute("shot.new", json!({ "name": "Wide" })).unwrap());
    let hills = id(s.execute("layer.new", json!({ "shot": shot_id })).unwrap());
    s.execute("drawing.stroke", stroke_params(hills, 0, json!({ "size": 8, "pressureSize": false }))).unwrap();
    let peg = id(s.execute("peg.new", json!({ "aboveLayer": hills })).unwrap());
    let picture = |s: &mut Session| {
        let path = dir.path().join("frame.png");
        s.execute("shot.renderFrame", json!({ "shot": shot_id, "path": path })).unwrap();
        iskra_io::decode_png(&std::fs::read(&path).unwrap()).unwrap().2
    };
    let before = picture(&mut s);
    let distance = s.project().shot(ShotId(shot_id)).unwrap().camera.distance;
    s.execute("peg.setTransform", json!({ "peg": peg, "depth": -distance, "keepSize": true })).unwrap();
    let pose = s.project().shot(ShotId(shot_id)).unwrap().pegs[1].transform;
    assert_eq!((pose.depth, pose.scale, s.history().undo_label()), (-distance, [2.0, 2.0], Some("Move Peg in Depth")));
    let after = picture(&mut s);
    let off = before.iter().zip(&after).filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > 40)).count();
    assert!(off < 40, "the picture is the same but for the resampled edge: {off} pixels differ");
    assert!(turned_down(s.execute("peg.setTransform", json!({ "peg": peg, "depth": distance * 3.0, "keepSize": true })), "can't see it"));
    assert!(turned_down(
        s.execute("peg.setTransform", json!({ "peg": peg, "depth": 1.0, "scale": [1.0, 1.0], "keepSize": true })),
        "sets the position and the scale itself"
    ));
}

#[test]
fn items_are_ordered_in_bins_and_a_storyboard_can_start_at_a_time() {
    let mut s = Session::new();
    let (first, board) = (s.project().shots[0].id.0, storyboard(&s));
    let scenes = id(s.execute("bin.new", json!({ "name": "Scenes" })).unwrap());
    let one = id(s.execute("bin.new", json!({ "name": "Act One", "parent": scenes })).unwrap());
    let unnamed = id(s.execute("bin.new", Value::Null).unwrap());
    assert_eq!(s.project().bin(iskra_model::BinId(unnamed)).map(|bin| bin.name.as_str()), Some("Bin"));
    let in_bin = |s: &Session, bin: Option<u64>| s.project().items_in(bin.map(iskra_model::BinId)).len();
    assert_eq!((in_bin(&s, None), in_bin(&s, Some(one))), (2, 0), "everything starts in Master");

    // Items are moved into a bin, and new ones can be made in one.
    s.execute("item.setBin", json!({ "items": [{ "shot": first }, { "storyboard": board }], "bin": one })).unwrap();
    let second = id(s.execute("shot.new", json!({ "bin": one })).unwrap());
    let other = id(s.execute("storyboard.new", json!({ "bin": scenes })).unwrap());
    assert_eq!((in_bin(&s, None), in_bin(&s, Some(one)), in_bin(&s, Some(scenes)), s.history().undo_label()), (0, 3, 1, Some("New Storyboard")));
    // A copy of a shot is in the bin its original is in.
    let copy = id(s.execute("shot.duplicate", json!({ "shot": first })).unwrap());
    assert_eq!(s.project().shot(ShotId(copy)).unwrap().bin, Some(iskra_model::BinId(one)));

    // Bins are renamed and nested; names are unique in a bin, and no bin goes into itself.
    s.execute("bin.rename", json!({ "bin": unnamed, "name": "Sound" })).unwrap();
    assert!(turned_down(s.execute("bin.rename", json!({ "bin": unnamed, "name": "Scenes" })), "a bin named `Scenes` there already"));
    assert!(turned_down(s.execute("bin.new", json!({ "name": "Act One", "parent": scenes })), "there already"));
    s.execute("bin.new", json!({ "name": "Act One" })).unwrap();
    assert!(turned_down(s.execute("bin.setParent", json!({ "bin": scenes, "parent": one })), "into itself"));
    assert!(turned_down(s.execute("bin.setParent", json!({ "bin": one })), "there already"), "Master has an Act One by now");
    s.execute("bin.setParent", json!({ "bin": unnamed, "parent": one })).unwrap();
    assert_eq!(s.project().bin(iskra_model::BinId(unnamed)).unwrap().parent, Some(iskra_model::BinId(one)));

    // A deleted bin leaves what it held in the bin it was in, in one step that undo takes back.
    s.execute("bin.delete", json!({ "bin": one })).unwrap();
    assert_eq!((in_bin(&s, Some(scenes)), s.project().bin(iskra_model::BinId(unnamed)).unwrap().parent), (5, Some(iskra_model::BinId(scenes))));
    s.undo();
    assert_eq!((in_bin(&s, Some(one)), in_bin(&s, Some(scenes))), (4, 1));
    let _ = (second, other);

    // A storyboard's times can start elsewhere than at nothing.
    s.execute("storyboard.setStart", json!({ "storyboard": board, "frame": 86400 })).unwrap();
    assert_eq!((s.project().storyboards[0].start, s.history().undo_label()), (86400, Some("Set Start Timecode")));
    for (command, params, why) in [
        ("bin.new", json!({ "parent": 999 }), "no bin 999"),
        ("bin.delete", json!({ "bin": 999 }), "no bin 999"),
        ("item.setBin", json!({ "items": [{ "shot": 999 }] }), "no such item"),
        ("item.setBin", json!({ "items": [{ "shot": first }], "bin": 999 }), "no bin 999"),
        ("shot.new", json!({ "bin": 999 }), "no bin 999"),
        ("bin.rename", json!({ "bin": scenes, "name": "a/b" }), "can't contain"),
    ] {
        assert!(turned_down(s.execute(command, params), why), "{command}: {why}");
    }
}

#[test]
fn swatches_are_added_changed_moved_and_removed_and_ids_are_not_given_twice() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    let names = |s: &Session| s.project().palette.swatches.iter().map(|swatch| format!("{}:{}", swatch.id.0, swatch.name)).collect::<Vec<_>>().join(" ");
    assert_eq!(names(&s), "1:Black 2:White 3:Red 4:Green 5:Blue");

    // A new swatch takes the colour of the one it is put after.
    let skin = s.execute("palette.add", json!({ "after": 3 })).unwrap()["swatch"].as_u64().unwrap();
    assert_eq!((skin, names(&s).as_str()), (6, "1:Black 2:White 3:Red 6:Colour 1 4:Green 5:Blue"));
    assert_eq!(s.project().palette.color(SwatchId(6)), [220, 50, 47, 255]);
    s.execute("palette.set", json!({ "swatch": 6, "name": " Skin ", "color": [240, 200, 170, 255] })).unwrap();
    assert_eq!((s.history().undo_label(), s.project().palette.color(SwatchId(6))), (Some("Change Swatch"), [240, 200, 170, 255]));
    s.execute("palette.move", json!({ "swatch": 6, "to": 0 })).unwrap();
    s.execute("palette.move", json!({ "swatch": 1, "to": 99 })).unwrap();
    assert_eq!(names(&s), "6:Skin 2:White 3:Red 4:Green 5:Blue 1:Black");

    // A drawing painted with a swatch that is then removed is reported, and no later swatch
    // gets the id it is painted with.
    let mut painted = stroke_params(e, 0, json!({ "size": 20 }));
    painted["swatch"] = json!(6);
    s.execute("drawing.stroke", painted).unwrap();
    assert_eq!(s.execute("palette.usage", Value::Null).unwrap(), json!({ "unused": [2, 3, 4, 5, 1], "missing": [], "unread": [] }));
    s.execute("palette.remove", json!({ "swatch": 6 })).unwrap();
    assert_eq!(s.execute("palette.usage", Value::Null).unwrap()["missing"], json!([6]));
    assert_eq!(s.execute("palette.add", json!({ "name": "Hair", "color": [90, 60, 30, 255] })).unwrap()["swatch"], json!(7));
    assert_eq!(names(&s), "2:White 3:Red 4:Green 5:Blue 1:Black 7:Hair");
    s.undo();
    s.undo();
    assert_eq!(s.execute("palette.usage", Value::Null).unwrap()["missing"], json!([]), "undo brings the swatch back");

    let steps = s.history().entries().len();
    for (id, refused) in [
        ("palette.set", json!({ "swatch": 77, "name": "No" })),
        ("palette.set", json!({ "swatch": 2, "name": "  " })),
        ("palette.add", json!({ "after": 77 })),
        ("palette.move", json!({ "swatch": 77, "to": 0 })),
        ("palette.remove", json!({ "swatch": 77 })),
    ] {
        assert!(matches!(s.execute(id, refused.clone()), Err(EngineError::Invalid(_))), "{id} {refused}");
    }
    assert_eq!(s.history().entries().len(), steps);
    for swatch in [2, 3, 4, 5, 1] {
        s.execute("palette.remove", json!({ "swatch": swatch })).unwrap();
    }
    assert!(matches!(s.execute("palette.remove", json!({ "swatch": 6 })), Err(EngineError::Invalid(_))), "the last swatch stays");
}

#[test]
fn a_style_node_shows_what_goes_through_it_in_the_skin_of_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 40, "pressureSize": false }))).unwrap();
    let pixel = |s: &mut Session, name: &str| {
        let path = dir.path().join(name);
        s.execute("shot.renderFrame", json!({ "frame": 0, "path": path })).unwrap();
        let picture = image::open(&path).unwrap().to_rgba8();
        picture.get_pixel(picture.width() / 2 + 125, picture.height() / 2 + 50).0
    };
    // A Night skin whose variant is drawn in green over the day drawing's red, and no style of
    // that name: the shot shows the day drawings.
    let shot = s.project().shots[0].id.0;
    s.execute("skin.add", json!({ "name": "Night", "shot": shot })).unwrap();
    let mut green = stroke_params(e, 0, json!({ "size": 40, "pressureSize": false }));
    green["swatch"] = json!(4);
    s.execute("drawing.stroke", green).unwrap();
    s.execute("shot.setSkin", json!({ "shot": shot })).unwrap();
    assert_eq!(pixel(&mut s, "day.png"), [220, 50, 47, 255]);
    // A Style node named Night shows the Night drawings, by its amount.
    let node = s.execute("effect.new", json!({ "kind": "style", "on": { "layer": e } })).unwrap()["id"].as_u64().unwrap();
    s.execute("effect.set", json!({ "effect": node, "param": "style", "value": "Night" })).unwrap();
    assert_eq!(pixel(&mut s, "night.png"), [64, 160, 43, 255]);
    s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": 0.0 })).unwrap();
    assert_eq!(pixel(&mut s, "none.png"), [220, 50, 47, 255]);
    // With the shot shown in Night, a Style node of no name shows the day drawings.
    s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": 100.0 })).unwrap();
    s.execute("effect.set", json!({ "effect": node, "param": "style", "value": "" })).unwrap();
    s.execute("shot.setStyle", json!({ "shot": shot, "name": "Night" })).unwrap();
    assert_eq!(pixel(&mut s, "own.png"), [220, 50, 47, 255]);
}

#[test]
fn a_style_node_goes_over_to_a_style_by_its_keyed_amount() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 40, "pressureSize": false }))).unwrap();
    let pixel = |s: &mut Session, frame: u32| {
        let path = dir.path().join(format!("{frame}.png"));
        s.execute("shot.renderFrame", json!({ "frame": frame, "path": path })).unwrap();
        let picture = image::open(&path).unwrap().to_rgba8();
        picture.get_pixel(picture.width() / 2 + 125, picture.height() / 2 + 50).0
    };
    let night = s.execute("palette.addStyle", json!({ "name": "Night" })).unwrap()["style"].as_u64().unwrap();
    s.execute("palette.set", json!({ "swatch": 3, "style": night, "color": [60, 20, 90, 255] })).unwrap();
    let node = s.execute("effect.new", json!({ "kind": "style", "on": { "layer": e } })).unwrap()["id"].as_u64().unwrap();
    // A new one shows the palette's own colours, which the shot is shown in too.
    assert_eq!(pixel(&mut s, 0), [220, 50, 47, 255]);
    s.execute("effect.set", json!({ "effect": node, "param": "style", "value": "Night" })).unwrap();
    assert_eq!(pixel(&mut s, 0), [60, 20, 90, 255]);
    // From the shot's colours on frame 0 to Night's on frame 10, the drawing held over them.
    s.execute("exposure.setHold", json!({ "layer": e, "frame": 0, "length": 11 })).unwrap();
    s.execute("effect.set", json!({ "effect": node, "param": "amount", "value": 0.0 })).unwrap();
    s.execute("effect.setKey", json!({ "effect": node, "param": "amount", "frame": 0 })).unwrap();
    s.execute("effect.set", json!({ "effect": node, "param": "amount", "frame": 10, "value": 100.0 })).unwrap();
    let (start, middle, end) = (pixel(&mut s, 0), pixel(&mut s, 5), pixel(&mut s, 10));
    assert_eq!((start, end), ([220, 50, 47, 255], [60, 20, 90, 255]));
    assert!(middle[0] < 220 && middle[0] > 60 && middle[2] > 47 && middle[2] < 90, "{middle:?}");
    // Of a palette the stroke's swatch is not in, it is as the shot shows it; of its own, in Night.
    s.execute("palette.new", json!({ "name": "Hero" })).unwrap();
    s.execute("effect.set", json!({ "effect": node, "param": "palette", "value": "Hero" })).unwrap();
    assert_eq!(pixel(&mut s, 10), [220, 50, 47, 255]);
    s.execute("effect.set", json!({ "effect": node, "param": "palette", "value": "Default" })).unwrap();
    assert_eq!(pixel(&mut s, 10), [60, 20, 90, 255]);
}

#[test]
fn a_shot_is_shown_and_rendered_in_a_style_of_the_palette() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 40, "pressureSize": false }))).unwrap();
    // The stroke is in swatch 3, red, around (250, 100) of a drawing at two pixels a unit.
    let pixel = |s: &mut Session, name: &str| {
        let path = dir.path().join(name);
        s.execute("shot.renderFrame", json!({ "frame": 0, "path": path })).unwrap();
        let picture = image::open(&path).unwrap().to_rgba8();
        picture.get_pixel(picture.width() / 2 + 125, picture.height() / 2 + 50).0
    };
    assert_eq!(pixel(&mut s, "day.png"), [220, 50, 47, 255]);

    let night = s.execute("palette.addStyle", json!({ "name": "Night" })).unwrap()["style"].as_u64().unwrap();
    s.execute("palette.set", json!({ "swatch": 3, "style": night, "color": [60, 20, 90, 255] })).unwrap();
    assert_eq!(pixel(&mut s, "still-day.png"), [220, 50, 47, 255], "a style that no shot is shown in changes nothing");
    s.execute("shot.setStyle", json!({ "style": night })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Set Palette Style"));
    assert_eq!(pixel(&mut s, "night.png"), [60, 20, 90, 255]);
    assert_eq!(s.project().palette.color(SwatchId(3)), [220, 50, 47, 255], "the palette's own colour is as it was");

    // A swatch the style gives no colour has the palette's own, and follows it.
    s.execute("palette.set", json!({ "swatch": 3, "style": night, "reset": true })).unwrap();
    s.execute("palette.set", json!({ "swatch": 3, "color": [1, 2, 3, 255] })).unwrap();
    assert_eq!(pixel(&mut s, "reset.png"), [1, 2, 3, 255]);

    // A second style starts as a copy; names are not given twice; a removed style's shots
    // go back to the palette's own colours, in the same step.
    let dusk = s.execute("palette.addStyle", json!({ "from": night })).unwrap()["style"].as_u64().unwrap();
    assert_eq!((dusk, s.project().palette.styles[1].name.as_str()), (2, "Style 1"));
    assert!(matches!(s.execute("palette.renameStyle", json!({ "style": dusk, "name": "Night" })), Err(EngineError::Invalid(_))));
    s.execute("palette.renameStyle", json!({ "style": dusk, "name": "Dusk" })).unwrap();
    s.execute("palette.removeStyle", json!({ "style": night })).unwrap();
    assert_eq!((s.project().shots[0].styles.len(), s.project().palette.styles.len()), (0, 1));
    s.undo();
    assert_eq!(s.project().shots[0].styles, [iskra_model::StyleId(night as u16)]);
    for (id, refused) in [
        ("shot.setStyle", json!({ "style": 77 })),
        ("palette.set", json!({ "swatch": 3, "style": 77, "color": [0, 0, 0, 255] })),
        ("palette.set", json!({ "swatch": 3, "reset": true })),
        ("palette.addStyle", json!({ "name": "Dusk" })),
        ("palette.removeStyle", json!({ "style": 77 })),
    ] {
        assert!(matches!(s.execute(id, refused.clone()), Err(EngineError::Invalid(_))), "{id} {refused}");
    }
}

#[test]
fn a_swatch_is_recoloured_on_a_drawing_a_layer_or_a_shot_and_protected_ones_are_left() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    // The same red line on frames 0 and 5.
    for frame in [0, 5] {
        s.execute("drawing.stroke", stroke_params(e, frame, json!({ "size": 20, "pressureSize": false }))).unwrap();
    }
    let at = |s: &Session, frame| line_pixel(s, e, frame, 250, 100).map(|(swatch, _)| swatch);
    let done = s.execute("drawing.recolour", json!({ "from": 3, "to": 5, "layer": e, "frame": 0 })).unwrap();
    assert_eq!((done["drawings"].as_u64(), at(&s, 0), at(&s, 5)), (Some(1), Some(5), Some(3)), "the drawing on the frame alone");
    assert_eq!(s.history().undo_label(), Some("Recolour"));
    assert_eq!(s.execute("drawing.recolour", json!({ "from": 3, "to": 4 })).unwrap()["drawings"], json!(1));
    assert_eq!((at(&s, 0), at(&s, 5)), (Some(5), Some(4)), "every drawing of the shot that had it");

    // Between two frames: only the drawings shown on one of them, on the layer or the shot.
    assert_eq!(s.execute("drawing.recolour", json!({ "from": 4, "to": 2, "layer": e, "frames": [1, 4] })).unwrap()["drawings"], json!(0));
    assert_eq!(s.execute("drawing.recolour", json!({ "from": 4, "to": 2, "frames": [3, 9] })).unwrap()["drawings"], json!(1));
    assert_eq!((at(&s, 0), at(&s, 5)), (Some(5), Some(2)));
    s.execute("drawing.recolour", json!({ "from": 2, "to": 4, "layer": e, "frames": [5, 5] })).unwrap();
    assert_eq!(at(&s, 5), Some(4));

    // A colour that went missing is given a swatch again.
    s.execute("palette.remove", json!({ "swatch": 4 })).unwrap();
    assert_eq!(s.execute("palette.usage", Value::Null).unwrap()["missing"], json!([4]));
    assert_eq!(s.execute("drawing.recolour", json!({ "from": 4, "to": 5, "all": true })).unwrap(), json!({ "drawings": 1, "unread": [] }));
    assert_eq!(s.execute("palette.usage", Value::Null).unwrap()["missing"], json!([]));

    // Nothing to recolour is no step; what makes no sense is refused.
    let steps = s.history().entries().len();
    assert_eq!(s.execute("drawing.recolour", json!({ "from": 2, "to": 1, "layer": e })).unwrap()["drawings"], json!(0));
    for refused in [
        json!({ "from": 5, "to": 5 }),
        json!({ "from": 5, "to": 77 }),
        json!({ "from": 5, "to": 1, "frame": 0 }),
        json!({ "from": 5, "to": 1, "layer": e, "frame": 9 }),
        json!({ "from": 5, "to": 1, "layer": e, "frame": 0, "frames": [0, 3] }),
        json!({ "from": 5, "to": 1, "all": true, "frames": [0, 3] }),
        json!({ "from": 5, "to": 1, "frames": [3, 0] }),
    ] {
        assert!(matches!(s.execute("drawing.recolour", refused.clone()), Err(EngineError::Invalid(_))), "{refused}");
    }
    assert_eq!(s.history().entries().len(), steps);

    // A stroke over a protected swatch leaves it, and so does the eraser.
    let mut over = stroke_params(e, 0, json!({ "size": 40, "pressureSize": false }));
    (over["swatch"], over["protect"]) = (json!(1), json!([5]));
    s.execute("drawing.stroke", over).unwrap();
    assert_eq!((at(&s, 0), line_pixel(&s, e, 0, 250, 115).map(|(swatch, _)| swatch)), (Some(5), Some(1)), "painted around the line, not on it");
    let mut eraser = stroke_params(e, 0, json!({ "size": 60, "erase": true, "pressureSize": false }));
    eraser["protect"] = json!([5]);
    s.execute("drawing.stroke", eraser).unwrap();
    assert_eq!((at(&s, 0), line_pixel(&s, e, 0, 250, 115)), (Some(5), Some((0, 0))));
}

#[test]
fn the_paint_bucket_refuses_an_area_of_a_protected_swatch() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    draw_square(&mut s, e, false);
    let fill = |swatch: u16, protect: Value| json!({ "layer": e, "frame": 0, "x": 250, "y": 250, "swatch": swatch, "protect": protect });
    s.execute("drawing.fill", fill(3, json!([]))).unwrap();
    assert!(matches!(s.execute("drawing.fill", fill(4, json!([3]))), Err(EngineError::Invalid(_))));
    assert_eq!(colour_pixel(&s, e, 250, 250), (3, 65535));
    s.execute("drawing.fill", fill(4, json!([1, 2]))).unwrap();
    assert_eq!(colour_pixel(&s, e, 250, 250), (4, 65535));
}

#[test]
fn a_palette_goes_to_a_file_and_into_another_project_by_its_names() {
    let dir = tempfile::tempdir().unwrap();
    let (own, gimp) = (dir.path().join("Hero.iskrapalette"), dir.path().join("Hero.gpl"));
    let mut s = Session::new();
    s.execute("palette.add", json!({ "name": "Skin", "color": [240, 200, 170, 255] })).unwrap();
    s.execute("palette.set", json!({ "swatch": 3, "color": [200, 0, 0, 255] })).unwrap();
    let night = s.execute("palette.addStyle", json!({ "name": "Night" })).unwrap()["style"].clone();
    s.execute("palette.set", json!({ "swatch": 6, "style": night, "color": [90, 80, 120, 255] })).unwrap();
    let steps = s.history().entries().len();
    assert_eq!(s.execute("palette.export", json!({ "path": own })).unwrap(), json!({ "swatches": 6 }));
    s.execute("palette.export", json!({ "path": gimp })).unwrap();
    assert_eq!(s.history().entries().len(), steps, "writing a file is no edit");

    // Into a new project: Skin is new there and gets a free id; Red is there, and stays as
    // it is unless the file's colour is asked for; the style comes with the ids as they are
    // there.
    let mut other = Session::new();
    other.execute("palette.remove", json!({ "swatch": 5 })).unwrap();
    assert_eq!(other.execute("palette.import", json!({ "path": own })).unwrap(), json!({ "added": 2, "updated": 0, "styles": 1, "palette": 0 }));
    let palette = other.project().palette.clone();
    let skin = palette.swatches.iter().find(|swatch| swatch.name == "Skin").expect("Skin came in");
    assert_eq!((skin.id, palette.color(SwatchId(3))), (SwatchId(7), [220, 50, 47, 255]), "Blue's id is not given to Skin");
    assert_eq!(palette.color_in(&[palette.styles[0].id], skin.id), [90, 80, 120, 255]);
    assert_eq!(other.history().undo_label(), Some("Import Palette"));
    assert_eq!(
        other.execute("palette.import", json!({ "path": gimp, "update": true })).unwrap(),
        json!({ "added": 0, "updated": 1, "styles": 0, "palette": 0 })
    );
    assert_eq!(other.project().palette.color(SwatchId(3)), [200, 0, 0, 255]);

    // The same file again changes nothing, and is no step.
    let steps = other.history().entries().len();
    other.execute("palette.import", json!({ "path": own, "update": true })).unwrap();
    assert_eq!(other.history().entries().len(), steps);
    assert!(matches!(other.execute("palette.import", json!({ "path": dir.path().join("None.gpl") })), Err(EngineError::Invalid(_))));
}

#[test]
fn unused_swatches_go_at_once_palettes_are_reordered_and_a_shot_is_switched_by_a_styles_name() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    let hero = s.execute("palette.new", json!({ "name": "Hero" })).unwrap()["palette"].as_u64().unwrap();
    let skin = s.execute("palette.add", json!({ "name": "Skin", "palette": hero })).unwrap()["swatch"].as_u64().unwrap();
    s.execute("palette.add", json!({ "name": "Hair", "palette": hero })).unwrap();
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20 }))).unwrap();
    let sets = |s: &Session| {
        let palette = s.project().palette.clone();
        let of = |set: &iskra_model::SwatchSet| palette.swatches_of(set.id).map(|swatch| swatch.id.0.to_string()).collect::<Vec<_>>().join(" ");
        palette.sets.iter().map(|set| format!("{} [{}]", set.name, of(set))).collect::<Vec<_>>().join("; ")
    };
    assert_eq!(sets(&s), "Default [1 2 3 4 5]; Hero [6 7]");

    // The palettes change places, and the swatches stay with their own.
    s.execute("palette.reorder", json!({ "palette": hero, "to": 0 })).unwrap();
    assert_eq!((sets(&s).as_str(), s.history().undo_label()), ("Hero [6 7]; Default [1 2 3 4 5]", Some("Move Palette")));
    s.execute("palette.reorder", json!({ "palette": hero, "to": 99 })).unwrap();
    assert_eq!(sets(&s), "Default [1 2 3 4 5]; Hero [6 7]");

    // A style of one name in each palette, and the shot is shown in both by the name; a
    // style made with `shot` is shown in the step that makes it.
    let night = s.execute("palette.addStyle", json!({ "name": "Night" })).unwrap()["style"].as_u64().unwrap();
    let steps = s.history().entries().len();
    let hero_night = s.execute("palette.addStyle", json!({ "name": "Night", "palette": hero, "shot": s.project().shots[0].id.0 })).unwrap()["style"].clone();
    assert_eq!((s.history().entries().len(), serde_json::to_value(&s.project().shots[0].styles).unwrap()), (steps + 1, json!([hero_night])));
    s.undo();
    assert_eq!((s.project().palette.styles.len(), s.project().shots[0].styles.len()), (1, 0), "one step back takes both");
    s.redo();
    let dusk = s.execute("palette.addStyle", json!({ "name": "Dusk", "palette": hero })).unwrap()["style"].clone();
    s.execute("shot.setStyle", json!({ "style": dusk })).unwrap();
    s.execute("shot.setStyle", json!({ "name": "Night" })).unwrap();
    assert_eq!(serde_json::to_value(&s.project().shots[0].styles).unwrap(), json!([night, hero_night]));
    s.execute("shot.setStyle", json!({ "name": "Dusk" })).unwrap();
    assert_eq!(serde_json::to_value(&s.project().shots[0].styles).unwrap(), json!([dusk]), "the default palette has no Dusk and is its own");

    // Every swatch nothing is painted with goes in one step; red stays, and so does the
    // first of a palette that would have none.
    let steps = s.history().entries().len();
    assert_eq!(s.execute("palette.removeUnused", json!({ "palette": hero })).unwrap(), json!({ "removed": [7] }));
    assert_eq!(sets(&s), format!("Default [1 2 3 4 5]; Hero [{skin}]"));
    assert_eq!(s.execute("palette.removeUnused", json!({})).unwrap(), json!({ "removed": [1, 2, 4, 5] }));
    assert_eq!((sets(&s).as_str(), s.history().undo_label()), ("Default [3]; Hero [6]", Some("Remove Unused Swatches")));
    assert_eq!(s.history().entries().len(), steps + 2);
    assert_eq!(s.execute("palette.add", json!({})).unwrap()["swatch"], json!(8), "an id that went is not given again");

    let steps = s.history().entries().len();
    s.execute("palette.remove", json!({ "swatch": 8 })).unwrap();
    for (id, refused) in [
        ("palette.removeUnused", json!({})),
        ("palette.removeUnused", json!({ "palette": 77 })),
        ("palette.reorder", json!({ "palette": 77, "to": 0 })),
        ("shot.setStyle", json!({ "name": "Noon" })),
        ("shot.setStyle", json!({ "name": "Night", "style": night })),
        ("palette.addStyle", json!({ "name": "Day", "shot": 9999 })),
    ] {
        assert!(matches!(s.execute(id, refused.clone()), Err(EngineError::Invalid(_))), "{id} {refused}");
    }
    assert_eq!(s.history().entries().len(), steps + 1);
}

#[test]
fn a_shape_on_vector_art_is_vector_strokes_and_a_fill() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let shape = |shape: &str| json!({ "layer": e, "frame": 0, "shape": shape, "from": [100, 100], "to": [300, 200], "swatch": 3, "vectorBrush": { "size": 6, "pressureSize": false } });
    let kinds = |s: &Session| {
        let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector line art") };
        art.shapes().iter().map(|shape| if matches!(shape.kind, ShapeKind::Fill(_)) { 'f' } else { 's' }).collect::<String>()
    };
    // A star filled: its fill under its line, in one step, as wide as the brush.
    let mut star = shape("star");
    star["filled"] = json!(true);
    let steps = s.history().entries().len();
    s.execute("drawing.shape", star.clone()).unwrap();
    assert_eq!((kinds(&s).as_str(), s.history().entries().len(), s.history().undo_label()), ("fs", steps + 1, Some("Star")));
    let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
    let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!() };
    let ShapeKind::Stroke(line) = &art.shapes()[1].kind else { panic!("a stroke") };
    assert!(line.widths.iter().all(|width| (width - 6.0).abs() < 1e-6) && line.path.segments().count() == 10, "{:?}", line.widths);
    // With no line, the fill alone; a shape with no inside and no line is refused.
    star["noLine"] = json!(true);
    s.execute("drawing.shape", star).unwrap();
    assert_eq!(kinds(&s), "fsf");
    // A spiral has no inside, so it keeps its line.
    let mut bare = shape("spiral");
    (bare["filled"], bare["noLine"]) = (json!(true), json!(true));
    s.execute("drawing.shape", bare).unwrap();
    assert_eq!(kinds(&s), "fsfs");
    // A grid is its outside and a line for each row and column inside, apart.
    let mut grid = shape("grid");
    (grid["rows"], grid["columns"]) = (json!(2), json!(3));
    s.execute("drawing.shape", grid).unwrap();
    assert_eq!(kinds(&s), "fsfsssss");
    // A polyline through its corners, and an arrow; the eraser draws no shape on vector art.
    let roof = json!({ "layer": e, "frame": 0, "shape": "polyline", "points": [[100, 200], [200, 100], [300, 200]], "swatch": 3, "closed": true });
    s.execute("drawing.shape", roof).unwrap();
    s.execute("drawing.shape", shape("arrow")).unwrap();
    assert_eq!(kinds(&s), "fsfsssssss");
    let mut erasing = shape("line");
    erasing["brush"] = json!({ "erase": true });
    assert!(matches!(s.execute("drawing.shape", erasing), Err(EngineError::Invalid(_))));
}

#[test]
fn a_shape_with_no_line_paints_its_inside_alone() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    let box_ = json!({ "layer": e, "frame": 0, "shape": "triangle", "from": [100, 100], "to": [300, 200], "swatch": 3, "filled": true, "noLine": true, "brush": { "size": 6, "pressureSize": false } });
    s.execute("drawing.shape", box_).unwrap();
    let swatch = |x: i32, y: i32| line_pixel(&s, e, 0, x, y).map(|(swatch, coverage)| (swatch, coverage > 32768));
    assert_eq!((swatch(200, 170), swatch(200, 203), swatch(300, 202)), (Some((3, true)), Some((0, false)), Some((0, false))), "inside, and no line round it");
}

#[test]
fn a_shape_is_drawn_with_a_brush_as_one_stroke() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    let shape = |shape: &str, even: bool| json!({ "layer": e, "frame": 0, "shape": shape, "from": [100, 100], "to": [300, 200], "even": even, "swatch": 3, "brush": { "size": 6, "pressureSize": false, "smoothing": { "amount": 1.0 } } });
    let on = |s: &Session, x: i32, y: i32| line_pixel(s, e, 0, x, y).is_some_and(|(swatch, coverage)| swatch == 3 && coverage > 32768);
    s.execute("drawing.shape", shape("rectangle", false)).unwrap();
    assert_eq!((cells(&s, e, 2).as_str(), s.history().undo_label()), ("1 -", Some("Rectangle")), "the drawing is made with it, in one step");
    assert!(on(&s, 100, 100) && on(&s, 300, 200) && on(&s, 200, 100) && on(&s, 100, 150), "the corners are sharp whatever the brush's smoothing");
    assert!(!on(&s, 200, 150));
    s.execute("drawing.shape", shape("ellipse", true)).unwrap();
    assert_eq!(
        (on(&s, 200, 100), on(&s, 200, 300), on(&s, 300, 200), on(&s, 100, 200), on(&s, 200, 250)),
        (true, true, true, true, false),
        "a circle, as wide as the drag is long"
    );
    s.execute("drawing.shape", shape("line", false)).unwrap();
    assert!(on(&s, 200, 150) && s.history().undo_label() == Some("Line"));
    s.undo();
    assert!(!on(&s, 200, 150));

    // From its middle, filled and with its corners rounded: the inside is painted in the
    // same step, and the corner is cut off.
    let mut pill = shape("rectangle", false);
    (pill["from"], pill["to"], pill["centred"], pill["filled"], pill["round"]) = (json!([200, 150]), json!([300, 200]), json!(true), json!(true), json!(30));
    pill["frame"] = json!(10);
    let steps = s.history().entries().len();
    s.execute("drawing.shape", pill.clone()).unwrap();
    assert_eq!(s.history().entries().len(), steps + 1);
    let on = |s: &Session, x: i32, y: i32| line_pixel(s, e, 10, x, y).is_some_and(|(swatch, coverage)| swatch == 3 && coverage > 32768);
    assert!(on(&s, 150, 150) && on(&s, 100, 150) && on(&s, 200, 100) && on(&s, 110, 110) && !on(&s, 100, 100) && !on(&s, 94, 150));
    pill["round"] = json!(-1);
    assert!(matches!(s.execute("drawing.shape", pill), Err(EngineError::Invalid(_))));

    // A polyline goes through its corners; filled, it is closed and painted inside.
    let roof = json!({ "layer": e, "frame": 14, "shape": "polyline", "points": [[100, 200], [200, 100], [300, 200]], "swatch": 3, "filled": true, "brush": { "size": 6, "pressureSize": false } });
    s.execute("drawing.shape", roof.clone()).unwrap();
    let on = |s: &Session, x: i32, y: i32| line_pixel(s, e, 14, x, y).is_some_and(|(swatch, coverage)| swatch == 3 && coverage > 32768);
    assert!(on(&s, 150, 150) && on(&s, 200, 200) && on(&s, 200, 150) && !on(&s, 120, 120) && s.history().undo_label() == Some("Polyline"));
    let mut short = roof;
    short["points"] = json!([[100, 200]]);
    assert!(matches!(s.execute("drawing.shape", short), Err(EngineError::Invalid(_))));
    assert!(matches!(s.execute("drawing.shape", json!({ "layer": e, "frame": 0, "shape": "line", "from": [0, 0] })), Err(EngineError::Invalid(_))));

    let mut nowhere = shape("line", false);
    nowhere["to"] = json!([f64::NAN, 0]);
    assert!(s.execute("drawing.shape", nowhere).is_err());
    assert!(matches!(
        s.execute("drawing.shape", json!({ "layer": e, "frame": 0, "shape": "heart", "from": [0, 0], "to": [1, 1] })),
        Err(EngineError::BadParams { .. })
    ));
}

#[test]
fn a_shot_keeps_drawing_guides_that_are_added_changed_and_removed() {
    let mut s = Session::new();
    let guides = |s: &Session| serde_json::to_value(&s.project().shots[0].guides).unwrap();
    let ruler = s.execute("guide.add", json!({ "kind": "ruler" })).unwrap()["guide"].as_u64().unwrap();
    let street = s.execute("guide.add", json!({ "kind": "perspective", "points": 2, "name": "Street" })).unwrap()["guide"].as_u64().unwrap();
    let second = s.execute("guide.add", json!({ "kind": "ruler" })).unwrap()["guide"].as_u64().unwrap();
    assert_eq!(
        guides(&s),
        json!([
            { "id": ruler, "name": "Ruler", "kind": "ruler", "at": [0.0, 0.0], "angle": 0.0 },
            { "id": street, "name": "Street", "kind": "perspective", "points": [[-1920.0, 0.0], [1920.0, 0.0]] },
            { "id": second, "name": "Ruler 2", "kind": "ruler", "at": [0.0, 0.0], "angle": 0.0 },
        ])
    );
    s.execute("guide.set", json!({ "guide": ruler, "angle": 30, "name": "Roof" })).unwrap();
    s.execute("guide.set", json!({ "guide": street, "points": [[-900, 40], [2500, 40]] })).unwrap();
    assert_eq!(
        (guides(&s)[0]["angle"].clone(), guides(&s)[0]["name"].clone(), guides(&s)[1]["points"][1].clone()),
        (json!(30.0), json!("Roof"), json!([2500.0, 40.0]))
    );
    assert_eq!(s.history().undo_label(), Some("Change Drawing Guide"));
    s.execute("guide.remove", json!({ "guide": second })).unwrap();
    assert_eq!(guides(&s).as_array().map(Vec::len), Some(2));

    let steps = s.history().entries().len();
    for (id, refused) in [
        ("guide.add", json!({ "kind": "perspective", "points": 4 })),
        ("guide.add", json!({ "kind": "ruler", "name": " " })),
        ("guide.set", json!({ "guide": ruler, "points": [[0, 0]] })),
        ("guide.set", json!({ "guide": street, "angle": 10 })),
        ("guide.set", json!({ "guide": street, "points": [[0, 0]] })),
        ("guide.set", json!({ "guide": second, "angle": 10 })),
        ("guide.remove", json!({ "guide": 9999 })),
    ] {
        assert!(matches!(s.execute(id, refused.clone()), Err(EngineError::Invalid(_))), "{id} {refused}");
    }
    assert_eq!(s.history().entries().len(), steps);
}

/// The colour art pixel at (x, y) of the free-colour drawing exposed at `frame`.
fn free_pixel(s: &Session, layer: u64, frame: u32, x: i32, y: i32) -> Option<[u8; 4]> {
    let drawing = &s.project().layer(iskra_model::LayerId(layer))?.drawing_at(frame)?.drawing;
    let iskra_model::ArtContent::Rgba(surface) = &drawing.layer(iskra_model::ArtLayerKind::ColourArt)?.content else { return None };
    let mut out = [[0u8; 4]];
    surface.read_rgba8_into(iskra_model::Rect::new(x, y, x + 1, y + 1), &mut out);
    Some(out[0])
}

#[test]
fn an_art_layer_made_free_colour_is_painted_in_real_colour_and_leaves_the_palette() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    let on_colour = |frame: u32, swatch: u16, brush: Value| {
        let mut stroke = stroke_params(e, frame, brush);
        (stroke["artLayer"], stroke["swatch"]) = (json!("colourArt"), json!(swatch));
        stroke
    };
    let wide = json!({ "size": 40, "pressureSize": false });
    s.execute("drawing.stroke", on_colour(0, 3, wide.clone())).unwrap();
    assert_eq!(colour_pixel(&s, e, 250, 100), (3, 65535));

    // Made free colour, what was painted keeps its colour as pixels, and new drawings are so.
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "colourArt", "mode": "rgba" })).unwrap();
    assert_eq!((s.history().undo_label(), free_pixel(&s, e, 0, 250, 100)), (Some("Change Art Layer Kind"), Some([220, 50, 47, 255])));
    s.execute("drawing.stroke", on_colour(5, 5, wide.clone())).unwrap();
    assert_eq!(free_pixel(&s, e, 5, 250, 100), Some([38, 110, 210, 255]), "a stroke in an empty cell makes a free-colour drawing");
    assert_eq!(line_pixel(&s, e, 5, 250, 100), Some((0, 0)), "and its line art is indexed as ever");

    // Real colour: a half-opaque blue over the red is a mix of the two, which no swatch is,
    // and a later change to the swatch leaves what is painted.
    s.execute("drawing.stroke", on_colour(0, 5, json!({ "size": 40, "pressureSize": false, "opacity": 0.5 }))).unwrap();
    let mixed = free_pixel(&s, e, 0, 250, 100).unwrap();
    assert!(mixed[0] > 100 && mixed[0] < 160 && mixed[2] > 100 && mixed[2] < 160 && mixed[3] == 255, "{mixed:?}");
    s.execute("palette.set", json!({ "swatch": 5, "color": [0, 0, 0, 255] })).unwrap();
    assert_eq!(free_pixel(&s, e, 5, 250, 100), Some([38, 110, 210, 255]));
    s.execute("drawing.stroke", on_colour(5, 0, json!({ "size": 80, "erase": true, "pressureSize": false }))).unwrap();
    assert_eq!(free_pixel(&s, e, 5, 250, 100), Some([0, 0, 0, 0]), "the eraser erases it");

    // A colour of its own, that is no swatch's, is painted on free colour and needs no swatch;
    // on the indexed line art it is passed by, and the swatch is what is painted.
    let mut own = on_colour(8, 0, wide.clone());
    own["color"] = json!([10, 200, 150, 255]);
    s.execute("drawing.stroke", own.clone()).unwrap();
    assert_eq!(free_pixel(&s, e, 8, 250, 100), Some([10, 200, 150, 255]));
    let mut disc = json!({ "layer": e, "frame": 9, "artLayer": "colourArt", "shape": "ellipse", "from": [100, 100], "to": [300, 300], "filled": true });
    (disc["color"], disc["brush"]) = (json!([200, 10, 150, 255]), json!({ "size": 6, "pressureSize": false }));
    s.execute("drawing.shape", disc).unwrap();
    assert_eq!((free_pixel(&s, e, 9, 200, 200), free_pixel(&s, e, 9, 105, 105)), (Some([200, 10, 150, 255]), Some([0, 0, 0, 0])), "filled in the colour");
    (own["artLayer"], own["swatch"]) = (json!("lineArt"), json!(4));
    s.execute("drawing.stroke", own.clone()).unwrap();
    assert_eq!(line_pixel(&s, e, 8, 250, 100), Some((4, 65535)));
    own["swatch"] = json!(0);
    assert!(matches!(s.execute("drawing.stroke", own), Err(EngineError::Invalid(_))), "indexed art needs its swatch");

    // A selection takes free colour as it takes indexed art: the disc's left half is moved,
    // copied and pasted on another frame, and cleared; of a swatch's paint it has none.
    let half = json!({ "layer": e, "frame": 9, "rect": [0, 0, 200, 400] });
    let mut moved = half.clone();
    moved["matrix"] = json!([1, 0, 0, 1, 0, 500]);
    assert_eq!(s.execute("drawing.transform", moved).unwrap()["rect"], json!([95, 595, 200, 805]));
    assert_eq!(
        (free_pixel(&s, e, 9, 150, 200), free_pixel(&s, e, 9, 150, 700), free_pixel(&s, e, 9, 250, 200)),
        (Some([0, 0, 0, 0]), Some([200, 10, 150, 255]), Some([200, 10, 150, 255]))
    );
    assert_eq!(s.execute("drawing.copy", json!({ "layer": e, "frame": 9, "rect": [0, 0, 400, 400], "swatch": 3 })).unwrap(), json!({ "copied": false }));
    assert_eq!(s.execute("drawing.copy", json!({ "layer": e, "frame": 9, "rect": [0, 0, 400, 400] })).unwrap(), json!({ "copied": true }));
    s.execute("drawing.paste", json!({ "layer": e, "frame": 12 })).unwrap();
    assert_eq!((free_pixel(&s, e, 12, 250, 200), free_pixel(&s, e, 12, 150, 200)), (Some([200, 10, 150, 255]), Some([0, 0, 0, 0])));
    s.execute("drawing.clear", json!({ "layer": e, "frame": 12, "rect": [0, 0, 400, 400], "lasso": [[200, 0], [400, 0], [400, 200], [200, 200]] })).unwrap();
    assert_eq!((free_pixel(&s, e, 12, 250, 150), free_pixel(&s, e, 12, 250, 250)), (Some([0, 0, 0, 0]), Some([200, 10, 150, 255])));

    // Back to indexed, each pixel gets the nearest swatch: the mix becomes one of them.
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "colourArt", "mode": "indexed" })).unwrap();
    let (swatch, coverage) = colour_pixel(&s, e, 250, 100);
    assert!(coverage == 65535 && swatch != 0 && s.project().layer(iskra_model::LayerId(e)).unwrap().free_colour.is_empty(), "swatch {swatch}");
    s.undo();
    assert_eq!(free_pixel(&s, e, 0, 250, 100), Some(mixed));
    // The bucket fills free colour too: what is of one colour is one area, which gets the
    // swatch's colour, or a colour of its own that needs no swatch.
    assert_eq!(s.execute("drawing.fill", json!({ "layer": e, "frame": 9, "x": 250, "y": 200, "swatch": 4 })).unwrap()["filled"], json!(1));
    assert_eq!(
        (free_pixel(&s, e, 9, 250, 200), free_pixel(&s, e, 9, 150, 700)),
        (Some([64, 160, 43, 255]), Some([200, 10, 150, 255])),
        "the half that was moved is another area"
    );
    s.execute("drawing.fill", json!({ "layer": e, "frame": 9, "x": 250, "y": 200, "color": [9, 90, 190, 255] })).unwrap();
    assert_eq!(
        (free_pixel(&s, e, 9, 250, 200), free_pixel(&s, e, 9, 250, 250), s.history().undo_label()),
        (Some([9, 90, 190, 255]), Some([9, 90, 190, 255]), Some("Fill"))
    );
    s.execute("drawing.fill", json!({ "layer": e, "frame": 9, "x": 250, "y": 200 })).unwrap();
    assert_eq!((free_pixel(&s, e, 9, 250, 200), s.history().undo_label()), (Some([0, 0, 0, 0]), Some("Unpaint")));
    assert!(
        matches!(s.execute("drawing.fill", json!({ "layer": e, "frame": 9, "x": 250, "y": 450, "swatch": 3 })), Err(EngineError::Invalid(_))),
        "outside it runs out"
    );
}

#[test]
fn the_mixer_brush_is_a_command_that_is_given_what_the_brush_carries_and_gives_it_back() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    let points = |y: i32| (0..=20).map(|i| json!({ "x": 100 + i * 10, "y": y, "pressure": 1.0 })).collect::<Vec<_>>();
    let stroke = |y: i32| json!({ "brush": { "size": 20, "pressureSize": false }, "points": points(y) });
    let red = json!({ "reservoir": [1.0, 0.0, 0.0, 1.0], "amount": 1.0 });
    let dry = json!({ "wet": 0.0, "load": 1.0, "mix": 0.0, "flow": 1.0 });
    let mix = |y: i32, state: &Value, mixer: &Value| json!({ "layer": e, "frame": 0, "stroke": stroke(y), "state": state, "mixer": mixer });

    // On indexed art there is no colour to mix; on free colour it makes the drawing it paints.
    assert!(matches!(s.execute("drawing.mix", mix(100, &red, &dry)), Err(EngineError::Invalid(_))));
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "colourArt", "mode": "rgba" })).unwrap();
    let done = s.execute("drawing.mix", mix(100, &red, &dry)).unwrap();
    assert_eq!((free_pixel(&s, e, 0, 200, 100), s.history().undo_label(), cells(&s, e, 2).as_str()), (Some([255, 0, 0, 255]), Some("Mixer Stroke"), "1 -"));
    assert_eq!((done["state"]["reservoir"].clone(), done["state"]["pickup"].clone()), (json!([1.0, 0.0, 0.0, 1.0]), Value::Null), "dry, it picked nothing up");

    // Wet and loaded with blue, down through the red: it carries red on, and says so; given
    // what it gave back, the next stroke lays red where there was none.
    let blue = json!({ "reservoir": [0.0, 0.0, 1.0, 1.0], "amount": 1.0 });
    let wet = json!({ "wet": 1.0, "load": 1.0, "mix": 1.0, "flow": 1.0 });
    let mut down = mix(0, &blue, &wet);
    down["stroke"]["points"] = json!((0..=20).map(|i| json!({ "x": 200, "y": 60 + i * 5, "pressure": 1.0 })).collect::<Vec<_>>());
    let carried = s.execute("drawing.mix", down.clone()).unwrap()["state"].clone();
    assert!(carried["pickup"][0].as_f64().is_some_and(|red| red > 0.05), "{carried}");
    let after = free_pixel(&s, e, 0, 200, 100);
    s.execute("drawing.mix", mix(300, &carried, &wet)).unwrap();
    assert!(free_pixel(&s, e, 0, 200, 300).is_some_and(|laid| laid[0] > 10), "{:?}", free_pixel(&s, e, 0, 200, 300));

    // Undone and run again with what it was given, a stroke is what it was.
    s.undo();
    s.undo();
    assert_eq!(free_pixel(&s, e, 0, 200, 100), Some([255, 0, 0, 255]));
    assert_eq!((s.execute("drawing.mix", down).unwrap()["state"].clone(), free_pixel(&s, e, 0, 200, 100)), (carried, after));

    let steps = s.history().entries().len();
    let mut nowhere = mix(100, &red, &dry);
    nowhere["stroke"]["points"] = json!([]);
    let mut lines = mix(100, &red, &dry);
    lines["artLayer"] = json!("lineArt");
    for refused in [nowhere, lines] {
        assert!(matches!(s.execute("drawing.mix", refused.clone()), Err(EngineError::Invalid(_))), "{refused}");
    }
    assert_eq!(s.history().entries().len(), steps);
}

#[test]
fn a_project_has_several_palettes_whose_swatches_share_their_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let listed = |s: &Session| {
        let palette = s.project().palette.clone();
        let of =
            |set: &iskra_model::SwatchSet| palette.swatches_of(set.id).map(|swatch| format!("{}:{}", swatch.id.0, swatch.name)).collect::<Vec<_>>().join(" ");
        palette.sets.iter().map(|set| format!("{} [{}]", set.name, of(set))).collect::<Vec<_>>().join("; ")
    };
    assert_eq!(listed(&s), "Default [1:Black 2:White 3:Red 4:Green 5:Blue]");

    // A character's palette beside the default one: its swatches get the next ids, and two
    // palettes may each have a swatch of the same name.
    let hero = s.execute("palette.new", json!({ "name": "Hero" })).unwrap()["palette"].as_u64().unwrap();
    let skin = s.execute("palette.add", json!({ "palette": hero, "name": "Skin", "color": [240, 200, 170, 255] })).unwrap()["swatch"].as_u64().unwrap();
    s.execute("palette.add", json!({ "after": skin, "name": "Black" })).unwrap();
    s.execute("palette.add", json!({ "after": 2, "name": "Grey" })).unwrap();
    assert_eq!(listed(&s), "Default [1:Black 2:White 8:Grey 3:Red 4:Green 5:Blue]; Hero [6:Skin 7:Black]");
    // A drawing is painted with a swatch of either, by its id alone.
    let e = swatch_layer(&mut s);
    let mut stroke = stroke_params(e, 0, json!({ "size": 20 }));
    stroke["swatch"] = json!(skin);
    s.execute("drawing.stroke", stroke).unwrap();
    assert_eq!(line_pixel(&s, e, 0, 250, 100), Some((6, 65535)));

    // A swatch goes to another palette and keeps its id, so what it painted is as it was.
    s.execute("palette.set", json!({ "swatch": 3, "palette": hero })).unwrap();
    s.execute("palette.rename", json!({ "palette": hero, "name": "Knight" })).unwrap();
    assert_eq!(listed(&s), "Default [1:Black 2:White 8:Grey 4:Green 5:Blue]; Knight [6:Skin 7:Black 3:Red]");

    // One palette to a file, and into the project again as a palette of its own.
    let path = dir.path().join("Squire.iskrapalette");
    assert_eq!(s.execute("palette.export", json!({ "path": path, "palette": hero })).unwrap(), json!({ "swatches": 3 }));
    let came = s.execute("palette.import", json!({ "path": path, "asNew": true })).unwrap();
    assert_eq!((came["added"].as_u64(), came["palette"].as_u64()), (Some(3), Some(2)));
    assert_eq!(listed(&s), "Default [1:Black 2:White 8:Grey 4:Green 5:Blue]; Knight [6:Skin 7:Black 3:Red]; Squire [9:Skin 10:Black 11:Red]");
    // Into a palette that is there, only what it lacks by name comes.
    assert_eq!(s.execute("palette.import", json!({ "path": path, "palette": 0 })).unwrap()["added"], json!(2));

    // Removed, a palette takes its swatches with it, and what they painted is missing.
    s.execute("palette.delete", json!({ "palette": hero })).unwrap();
    assert_eq!(s.execute("palette.usage", Value::Null).unwrap()["missing"], json!([6]));
    s.undo();
    let steps = s.history().entries().len();
    for (id, refused) in [
        ("palette.new", json!({ "name": "Knight" })),
        ("palette.rename", json!({ "palette": hero, "name": "Default" })),
        ("palette.rename", json!({ "palette": 77, "name": "No" })),
        ("palette.add", json!({ "palette": 77 })),
        ("palette.set", json!({ "swatch": 1, "palette": 77 })),
        ("palette.delete", json!({ "palette": 77 })),
        ("palette.export", json!({ "path": path, "palette": 77 })),
    ] {
        assert!(matches!(s.execute(id, refused.clone()), Err(EngineError::Invalid(_))), "{id} {refused}");
    }
    assert_eq!(s.history().entries().len(), steps);
    s.execute("palette.delete", json!({ "palette": 2 })).unwrap();
    s.execute("palette.delete", json!({ "palette": hero })).unwrap();
    assert!(matches!(s.execute("palette.delete", json!({ "palette": 0 })), Err(EngineError::Invalid(_))), "the last palette stays");
}

#[test]
fn each_palette_has_styles_of_its_own_and_a_shot_is_shown_in_one_of_each() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let hero = s.execute("palette.new", json!({ "name": "Hero" })).unwrap()["palette"].as_u64().unwrap();
    let skin = s.execute("palette.add", json!({ "palette": hero, "name": "Skin", "color": [240, 200, 170, 255] })).unwrap()["swatch"].as_u64().unwrap();
    // Night of the default palette, and Night of the hero's: two styles, of the same name.
    let night = s.execute("palette.addStyle", json!({ "name": "Night" })).unwrap()["style"].as_u64().unwrap();
    let hero_night = s.execute("palette.addStyle", json!({ "name": "Night", "palette": hero })).unwrap()["style"].as_u64().unwrap();
    assert!(matches!(s.execute("palette.addStyle", json!({ "name": "Night", "palette": hero })), Err(EngineError::Invalid(_))));
    s.execute("palette.set", json!({ "swatch": 3, "style": night, "color": [60, 20, 90, 255] })).unwrap();
    s.execute("palette.set", json!({ "swatch": skin, "style": hero_night, "color": [90, 80, 120, 255] })).unwrap();
    let crossed = json!({ "swatch": skin, "style": night, "color": [0, 0, 0, 255] });
    assert!(matches!(s.execute("palette.set", crossed), Err(EngineError::Invalid(_))), "a style gives colours to its own palette's swatches");

    // The shot in the hero's Night alone: the default palette keeps its own colours.
    let seen = |s: &Session| {
        let (palette, styles) = (s.project().palette.clone(), s.project().shots[0].styles.clone());
        (palette.color_in(&styles, SwatchId(3)), palette.color_in(&styles, SwatchId(skin as u16)))
    };
    s.execute("shot.setStyle", json!({ "style": hero_night })).unwrap();
    assert_eq!(seen(&s), ([220, 50, 47, 255], [90, 80, 120, 255]));
    s.execute("shot.setStyle", json!({ "style": night })).unwrap();
    assert_eq!((seen(&s), s.project().shots[0].styles.len()), (([60, 20, 90, 255], [90, 80, 120, 255]), 2));
    // Another style of a palette takes the place of the one shown; none of a palette clears it.
    let dusk = s.execute("palette.addStyle", json!({ "from": hero_night, "name": "Dusk" })).unwrap()["style"].as_u64().unwrap();
    s.execute("shot.setStyle", json!({ "style": dusk })).unwrap();
    assert_eq!(s.project().shots[0].styles, [iskra_model::StyleId(night as u16), iskra_model::StyleId(dusk as u16)]);
    s.execute("shot.setStyle", json!({ "palette": 0 })).unwrap();
    assert_eq!(seen(&s), ([220, 50, 47, 255], [90, 80, 120, 255]));

    // A palette goes to a file with its own styles alone, and comes into another project so.
    let path = dir.path().join("Hero.iskrapalette");
    s.execute("palette.export", json!({ "path": path, "palette": hero })).unwrap();
    let mut other = Session::new();
    assert_eq!(
        other.execute("palette.import", json!({ "path": path, "asNew": true })).unwrap(),
        json!({ "added": 1, "updated": 0, "styles": 2, "palette": 1 })
    );
    let names: Vec<_> = other.project().palette.styles_of(iskra_model::SetId(1)).map(|style| style.name.clone()).collect();
    assert_eq!((names, other.project().palette.styles_of(iskra_model::SetId(0)).count()), (vec!["Night".to_string(), "Dusk".to_string()], 0));

    // A swatch that leaves a palette leaves its styles; a palette removed takes its styles,
    // and the shots are shown without them, in one step.
    s.execute("palette.set", json!({ "swatch": skin, "palette": 0 })).unwrap();
    assert!(s.project().palette.styles.iter().all(|style| !style.colors.contains_key(&SwatchId(skin as u16))));
    s.execute("palette.delete", json!({ "palette": hero })).unwrap();
    assert_eq!((s.project().palette.styles.len(), s.project().shots[0].styles.len()), (1, 0));
    s.undo();
    assert_eq!((s.project().palette.styles.len(), s.project().shots[0].styles.len()), (3, 1));
}

#[test]
fn a_new_layer_peg_or_composite_is_put_into_the_group_that_is_named() {
    let mut s = Session::new();
    let first = s.project().shots[0].layers[0].id.0;
    let group = s.execute("group.new", json!({ "nodes": [first] })).unwrap()["id"].as_u64().unwrap();
    let made: Vec<u64> =
        ["layer.new", "peg.new", "composite.new"].into_iter().map(|id| s.execute(id, json!({ "group": group })).unwrap()["id"].as_u64().unwrap()).collect();
    let shot = s.project().shots[0].clone();
    let members = &shot.group(iskra_model::GroupId(group)).unwrap().members;
    assert!(made.iter().all(|node| members.contains(node)) && members.len() == 4, "{members:?}");
    // One step each: undone, the node is gone and out of the group.
    s.undo();
    assert_eq!(s.project().shots[0].group(iskra_model::GroupId(group)).unwrap().members.len(), 3);
    // With a place, each is put there in the node view; a place that is none is refused.
    for (id, at) in [("layer.new", [10.0, -20.0]), ("peg.new", [30.0, -40.0]), ("composite.new", [50.0, -60.0])] {
        let made = s.execute(id, json!({ "at": at })).unwrap()["id"].as_u64().unwrap();
        assert_eq!(s.project().shots[0].places.get(&made), Some(&at), "{id}");
    }
    assert!(s.execute("peg.new", json!({ "at": [1e9, 0.0] })).is_err());
    // A group that is not the shot's is refused, and nothing is made.
    let layers = s.project().shots[0].layers.len();
    assert!(s.execute("layer.new", json!({ "group": 999_999 })).is_err());
    assert_eq!(s.project().shots[0].layers.len(), layers);
}

#[test]
fn a_group_made_with_a_composite_sends_its_pictures_out_through_it() {
    use iskra_model::{CompositeId, GroupId, LayerId, Source};
    let mut s = Session::new();
    let ids = |s: &mut Session, id: &str| s.execute(id, json!({})).unwrap()["id"].as_u64().unwrap();
    let first = s.project().shots[0].layers[0].id;
    let (second, third) = (LayerId(ids(&mut s, "layer.new")), LayerId(ids(&mut s, "layer.new")));
    let main = s.project().shots[0].main_composite().unwrap();
    let stacked = |s: &Session, composite: CompositeId| s.project().shots[0].composite(composite).unwrap().inputs.clone();
    assert_eq!(stacked(&s, main), [Source::Layer(third), Source::Layer(second), Source::Layer(first)]);
    // The top and the bottom layer grouped: they go into the group's composite, in their
    // order, and it takes the place of the top one where they were stacked.
    let made = s.execute("group.new", json!({ "nodes": [third.0, first.0], "composite": true })).unwrap();
    let (group, own) = (GroupId(made["id"].as_u64().unwrap()), CompositeId(made["composite"].as_u64().unwrap()));
    assert_eq!(stacked(&s, main), [Source::Composite(own), Source::Layer(second)]);
    assert_eq!(stacked(&s, own), [Source::Layer(third), Source::Layer(first)]);
    let members = s.project().shots[0].group(group).unwrap().members.clone();
    assert!(members.contains(&own.0) && members.len() == 3, "{members:?}");
    // One step: undone, all is wired as it was.
    s.undo();
    assert_eq!((stacked(&s, main).len(), s.project().shots[0].groups.len(), s.project().shots[0].composite(own).is_none()), (3, 0, true));
    // Nothing of a peg leaves as a picture, so a group of it gets no composite; nor does a
    // group that was not asked for one.
    let peg = ids(&mut s, "peg.new");
    assert!(s.execute("group.new", json!({ "nodes": [peg], "composite": true })).unwrap()["composite"].is_null());
    assert!(s.execute("group.new", json!({ "nodes": [second.0] })).unwrap()["composite"].is_null());
    assert_eq!(stacked(&s, main).len(), 3);
}

#[test]
fn every_shot_has_a_peg_that_moves_its_camera_from_the_start() {
    let mut s = Session::new();
    let moved_by = |s: &Session, shot: usize| {
        let shot = &s.project().shots[shot];
        shot.camera.parent.and_then(|peg| shot.peg(peg)).map(|peg| (peg.name.clone(), peg.parent, peg.transform == iskra_model::Transform::default()))
    };
    // The shot a session starts with, and one that is made: at home, so the picture is as drawn.
    assert_eq!(moved_by(&s, 0), Some(("Camera-P".to_string(), None, true)));
    assert!(!s.history().can_undo(), "it is there from the start, not as a step to undo");
    s.execute("shot.new", Value::Null).unwrap();
    assert_eq!(moved_by(&s, 1), Some(("Camera-P".to_string(), None, true)));
    // It is a peg like any other: deleted, the camera is at home with nothing to move it.
    let peg = s.project().shots[0].camera.parent.unwrap();
    s.execute("peg.delete", json!({ "peg": peg.0 })).unwrap();
    assert_eq!((moved_by(&s, 0), s.project().shots[0].pegs.len()), (None, 0));
}

#[test]
fn a_group_grouped_with_all_in_it_keeps_its_groups() {
    use iskra_model::GroupId;
    use std::collections::BTreeSet;
    let mut s = Session::new();
    let ids = |s: &mut Session, id: &str, params: Value| s.execute(id, params).unwrap()["id"].as_u64().unwrap();
    let first = s.project().shots[0].layers[0].id.0;
    let [second, third, fourth] = [(); 3].map(|_| ids(&mut s, "layer.new", json!({})));
    let arm = ids(&mut s, "group.new", json!({ "nodes": [first, second], "name": "Arm" }));
    let leg = ids(&mut s, "group.new", json!({ "nodes": [third, fourth], "name": "Leg" }));
    let body = ids(&mut s, "group.new", json!({ "nodes": [arm, leg], "name": "Body" }));
    // A box pulled over the open Body names it with all in it, its groups and their layers.
    let all = json!({ "nodes": [body, arm, leg, first, second, third, fourth], "ports": true });
    let rig = ids(&mut s, "group.new", all);
    let shot = s.project().shots[0].clone();
    let members =
        |group: u64| shot.group(GroupId(group)).map(|group| group.members.iter().copied().filter(|node| !shot.is_port(*node)).collect::<BTreeSet<_>>());
    assert_eq!(members(rig), Some([body].into()));
    assert_eq!(members(body), Some([arm, leg].into()));
    assert_eq!(members(arm), Some([first, second].into()));
    assert_eq!(members(leg), Some([third, fourth].into()));
    // A box pulled over all of it names the layers alone, not the groups: those all of
    // whose nodes are named go in whole, and are not emptied.
    s.undo();
    let rig = ids(&mut s, "group.new", json!({ "nodes": [first, second, third, fourth], "ports": true }));
    let shot = s.project().shots[0].clone();
    let members =
        |group: u64| shot.group(GroupId(group)).map(|group| group.members.iter().copied().filter(|node| !shot.is_port(*node)).collect::<BTreeSet<_>>());
    assert_eq!((members(rig), members(body), members(arm)), (Some([body].into()), Some([arm, leg].into()), Some([first, second].into())));
    // Some of the layers named with the group that holds them, beside one outside: the
    // layers stay where they are.
    s.undo();
    let rig = ids(&mut s, "group.new", json!({ "nodes": [arm, first, third] }));
    let shot = s.project().shots[0].clone();
    assert_eq!(shot.group_of(first).map(|group| group.id.0), Some(arm));
    assert_eq!(shot.group_of(third).map(|group| group.id.0), Some(rig));
    assert_eq!(shot.group_of(arm).map(|group| group.id.0), Some(rig));
}

#[test]
fn a_group_whose_last_node_is_taken_is_gone_with_its_ports() {
    use iskra_model::{GroupId, LayerId};
    let mut s = Session::new();
    let ids = |s: &mut Session, id: &str, params: Value| s.execute(id, params).unwrap()["id"].as_u64().unwrap();
    let first = s.project().shots[0].layers[0].id.0;
    let second = ids(&mut s, "layer.new", json!({}));
    let inner = ids(&mut s, "group.new", json!({ "nodes": [first, second] }));
    let outer = ids(&mut s, "group.new", json!({ "nodes": [inner] }));
    // A port of the inner group, with the first layer's picture going out through it.
    let port = ids(&mut s, "effect.new", json!({ "kind": "portOut", "group": inner, "on": { "layer": first } }));
    let groups = |s: &Session| s.project().shots[0].groups.iter().map(|group| group.id.0).collect::<Vec<_>>();
    let main = s.project().shots[0].main_composite().unwrap();
    let stacked = |s: &Session| s.project().shots[0].composite(main).unwrap().inputs.clone();
    assert!(stacked(&s).contains(&iskra_model::Source::Effect(iskra_model::EffectId(port))));
    // One layer deleted, the group still holds the other.
    s.execute("layer.delete", json!({ "layer": second })).unwrap();
    assert_eq!(groups(&s), [inner, outer]);
    // The last one taken out of it, in the same step the group is gone, and the group that
    // held only it; the port is gone, and the layer's picture goes where it went.
    s.execute("group.remove", json!({ "nodes": [first] })).unwrap();
    let shot = s.project().shots[0].clone();
    assert!(shot.groups.is_empty() && shot.effects.is_empty(), "{:?} {:?}", shot.groups, shot.effects);
    assert_eq!(stacked(&s), [iskra_model::Source::Layer(LayerId(first))]);
    // One step: undone, the groups and the port are there again.
    s.undo();
    assert_eq!((groups(&s), s.project().shots[0].effects.len()), (vec![inner, outer], 1));
    assert!(s.project().shots[0].group(GroupId(inner)).unwrap().members.contains(&port));
    // Deleting the last node does the same.
    s.execute("layer.delete", json!({ "layer": first })).unwrap();
    assert!(s.project().shots[0].groups.is_empty() && s.project().shots[0].effects.is_empty());
}

#[test]
fn a_group_gets_its_ports_by_itself_and_ungrouping_takes_them_away() {
    use iskra_model::{EffectId, GroupId, LayerId, PegId, Source};
    let mut s = Session::new();
    let ids = |s: &mut Session, id: &str, params: Value| s.execute(id, params).unwrap()["id"].as_u64().unwrap();
    let first = s.project().shots[0].layers[0].id.0;
    let second = ids(&mut s, "layer.new", json!({}));
    let outside = ids(&mut s, "layer.new", json!({}));
    // One peg carries both layers that are to be grouped, and the third.
    let peg = ids(&mut s, "peg.new", json!({}));
    for layer in [first, second, outside] {
        s.execute("layer.setParent", json!({ "layer": layer, "parent": peg })).unwrap();
    }
    let shot = |s: &Session| s.project().shots[0].clone();
    let main = shot(&s).main_composite().unwrap();
    let stacked = |s: &Session| shot(s).composite(main).unwrap().inputs.clone();
    let parents = |s: &Session| [first, second, outside].map(|layer| shot(s).layer(LayerId(layer)).unwrap().parent);
    let (was_stacked, was_carried) = (stacked(&s), parents(&s));

    let made = s.execute("group.new", json!({ "nodes": [first, second], "ports": true })).unwrap();
    let group = GroupId(made["id"].as_u64().unwrap());
    let ports: Vec<u64> = made["ports"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    let kinds: Vec<String> = ports.iter().map(|port| shot(&s).effect(EffectId(*port)).unwrap().kind.clone()).collect();
    assert_eq!(kinds, ["portIn", "portOut", "portOut"], "one way in for the one peg, and a way out a picture");
    // The peg goes into the port, and the port carries the two layers; the third is the peg's.
    let port_in = PegId(ports[0]);
    assert_eq!(shot(&s).effect(EffectId(ports[0])).unwrap().parent, Some(PegId(peg)));
    assert_eq!(parents(&s), [Some(port_in), Some(port_in), Some(PegId(peg))]);
    // Where the two pictures were stacked, their ports are; and all of them are in the group.
    let outs: Vec<Source> = was_stacked
        .iter()
        .map(|input| shot(&s).effects.iter().find(|port| port.picture() == Some(*input)).map_or(*input, |port| Source::Effect(port.id)))
        .collect();
    assert_eq!(stacked(&s), outs);
    assert!(ports.iter().all(|port| shot(&s).group(group).unwrap().members.contains(port)));
    assert_eq!(s.history().undo_label(), Some("New Group"));

    // Ungrouped, the ports are gone and all is wired as it was.
    s.execute("group.delete", json!({ "group": group.0 })).unwrap();
    assert!(shot(&s).effects.is_empty() && shot(&s).groups.is_empty());
    assert_eq!((stacked(&s), parents(&s)), (was_stacked, was_carried));
    // With a composite of its own the pictures leave through that, and only the way in is made.
    let made = s.execute("group.new", json!({ "nodes": [first, second], "ports": true, "composite": true })).unwrap();
    let ports: Vec<u64> = made["ports"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    assert_eq!((ports.len(), shot(&s).effect(EffectId(ports[0])).unwrap().kind.as_str(), made["composite"].is_u64()), (1, "portIn", true));
}

#[test]
fn an_art_layer_switched_off_is_not_rendered_and_is_kept() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    // Lines across the top, and colour across lower down.
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
    let mut colour = stroke_params(e, 0, json!({ "size": 40, "pressureSize": false }));
    colour["artLayer"] = json!("colourArt");
    colour["stroke"]["points"] = json!((0..=30).map(|i| json!({ "x": 100 + i * 10, "y": 300, "pressure": 1.0 })).collect::<Vec<_>>());
    s.execute("drawing.stroke", colour).unwrap();
    let painted = |s: &Session| {
        let project = s.project();
        let view = iskra_render::View::frame(&project.settings, 0.25);
        let picture = iskra_render::render(&project.shots[0], &project.palette, project.settings.working_space, 0, &view);
        picture.pixels.iter().filter(|pixel| pixel[3] > 0).count()
    };
    let whole = painted(&s);
    let shown = |s: &mut Session, art: &str, shown: bool| s.execute("layer.setArtShown", json!({ "layer": e, "artLayer": art, "shown": shown })).unwrap();

    shown(&mut s, "colourArt", false);
    let lines = painted(&s);
    assert!(lines > 0 && lines < whole, "the lines alone: {lines} of {whole}");
    assert_eq!(s.history().undo_label(), Some("Show Art Layer"));
    shown(&mut s, "lineArt", false);
    assert_eq!(painted(&s), 0, "nothing is seen");
    // What is switched off is kept, and is seen again when it is switched on.
    assert_eq!(colour_pixel(&s, e, 250, 300), (3, 65535));
    shown(&mut s, "lineArt", true);
    assert_eq!(painted(&s), lines);
    for _ in 0..3 {
        s.undo();
    }
    assert_eq!((painted(&s), s.project().layer(iskra_model::LayerId(e)).unwrap().art_off.as_slice()), (whole, &[][..]));
}

#[test]
fn a_pictures_alpha_is_read_as_its_file_keeps_it_or_left_out() {
    use iskra_model::LayerId;
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let path = dir.path().join("glow.png");
    std::fs::write(&path, iskra_io::encode_png(2, 1, &[[60, 30, 0, 128], [200, 150, 128, 128]]).unwrap()).unwrap();
    let made = s.execute("media.import", json!({ "paths": [path] })).unwrap();
    let media = made["media"][0].as_u64().unwrap();
    let layer = s.execute("media.place", json!({ "media": media })).unwrap()["id"].as_u64().unwrap();
    let pixels = |s: &Session| {
        let layer = s.project().layer(LayerId(layer)).unwrap();
        layer.drawings[0].drawing.to_rgba8(iskra_model::Rect::new(-1, 0, 1, 1), |_| [0; 4])
    };
    let set = |s: &mut Session, params: Value| {
        let mut params = params;
        params["layer"] = json!(layer);
        s.execute("layer.setTransparency", params).unwrap();
    };
    assert_eq!(pixels(&s), [[60, 30, 0, 128], [200, 150, 128, 128]], "straight, as it is");
    // Kept times their alpha, over black or over white, the colours are taken back out.
    set(&mut s, json!({ "alpha": "premultipliedBlack" }));
    assert_eq!((pixels(&s), s.history().undo_label()), (vec![[120, 60, 0, 128], [255, 255, 255, 128]], Some("Set Pictures' Transparency")));
    set(&mut s, json!({ "alpha": "premultipliedWhite" }));
    assert_eq!(pixels(&s), [[0, 0, 0, 128], [145, 46, 2, 128]]);
    // Without its transparency, each pixel is whole and as the file has it.
    set(&mut s, json!({ "transparent": false }));
    assert_eq!(pixels(&s), [[60, 30, 0, 255], [200, 150, 128, 255]]);
    let placed = s.project().layer(LayerId(layer)).unwrap();
    assert_eq!((placed.opaque, placed.alpha), (true, iskra_model::Alpha::PremultipliedWhite), "the mode is kept for when it is transparent again");
    // Read again, it stays so, and that changes nothing; undone, it is as it was.
    s.execute("layer.refreshMedia", json!({ "layer": layer })).unwrap();
    assert_eq!(pixels(&s), [[60, 30, 0, 255], [200, 150, 128, 255]]);
    s.undo();
    assert_eq!(pixels(&s), [[0, 0, 0, 128], [145, 46, 2, 128]]);
    // With the file gone, it is changed from the pixels kept: as the file had them, and read
    // again so.
    std::fs::remove_file(&path).unwrap();
    // The second pixel is had back; the first, darker than the white under it, had lost its
    // colour when it was read so.
    set(&mut s, json!({ "alpha": "straight" }));
    assert_eq!(pixels(&s), [[127, 127, 127, 128], [200, 150, 128, 128]]);
    set(&mut s, json!({ "alpha": "premultipliedBlack" }));
    assert_eq!(pixels(&s), [[253, 253, 253, 128], [255, 255, 255, 128]]);
    set(&mut s, json!({ "transparent": false }));
    assert_eq!(pixels(&s), [[127, 127, 127, 255], [128, 128, 128, 255]]);
    // But an alpha left out can't be given back without the file.
    assert!(turned_down(s.execute("layer.setTransparency", json!({ "layer": layer, "transparent": true })), "find the file"));
    assert!(s.project().layer(LayerId(layer)).unwrap().opaque);
    let drawn = s.project().shots[0].layers.last().unwrap().id.0;
    assert!(turned_down(s.execute("layer.setTransparency", json!({ "layer": drawn, "transparent": false })), "not placed from a media item"));
}

#[test]
fn a_node_put_by_one_in_a_group_goes_into_that_group() {
    let mut s = Session::new();
    let e = layer(&s);
    let group = s.execute("group.new", json!({ "nodes": [e] })).unwrap()["id"].as_u64().unwrap();
    let members = |s: &Session| s.project().shots[0].group(iskra_model::GroupId(group)).unwrap().members.clone();
    // A peg above the layer, an effect on its wire, bones that bend it and a display of it.
    let peg = s.execute("peg.new", json!({ "aboveLayer": e })).unwrap()["id"].as_u64().unwrap();
    let effect = s.execute("effect.new", json!({ "kind": "blur", "on": { "layer": e } })).unwrap()["id"].as_u64().unwrap();
    let chain = json!([{ "angle": 0.0, "length": 50.0 }]);
    let bones = s.execute("deformer.new", json!({ "layer": e, "origin": [0, 0], "bones": chain })).unwrap()["id"].as_u64().unwrap();
    let display = s.execute("display.new", json!({ "node": { "layer": e } })).unwrap()["id"].as_u64().unwrap();
    for node in [peg, effect, bones, display] {
        assert!(members(&s).contains(&node), "{node} is in the group: {:?}", members(&s));
    }
    // A node made apart from it, or told another group, is not.
    let apart = s.execute("peg.new", json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(!members(&s).contains(&apart));
    let other = s.execute("group.new", json!({ "nodes": [apart] })).unwrap()["id"].as_u64().unwrap();
    let told = s.execute("effect.new", json!({ "kind": "blur", "on": { "layer": e }, "group": other })).unwrap()["id"].as_u64().unwrap();
    assert!(!members(&s).contains(&told) && s.project().shots[0].group(iskra_model::GroupId(other)).unwrap().members.contains(&told));
}

#[test]
fn an_art_layer_made_vector_art_is_drawn_from_its_shapes_and_leaves_as_their_pixels() {
    use iskra_model::{ArtContent, ArtLayerKind, ShapeId, ShapeKind, VectorArt, VectorShape, VectorStroke};
    let mut s = Session::new();
    let e = layer(&s);
    let vector = json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" });
    let line = || stroke_params(e, 0, json!({ "size": 10, "pressureSize": false }));

    // Pixels painted on it are traced into shapes of their swatch (plan §4.16, step 9).
    s.execute("drawing.stroke", line()).unwrap();
    s.execute("layer.setArtMode", vector.clone()).unwrap();
    let traced = s.project().layer(iskra_model::LayerId(e)).unwrap().drawings[0].drawing.clone();
    let ArtContent::Vector(traced) = &traced.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector art") };
    assert!(!traced.is_empty() && traced.swatches() == [iskra_model::SwatchId(3)].into_iter().collect());
    assert_eq!(line_pixel(&s, e, 0, 250, 100).map(|(swatch, _)| swatch), Some(3), "where the stroke was");
    s.undo();
    s.undo();
    s.execute("layer.setArtMode", vector.clone()).unwrap();
    assert_eq!(s.project().layer(iskra_model::LayerId(e)).unwrap().vector, ArtLayerKind::ALL, "the whole layer");

    // The brush paints no pixels on it, and makes no drawing trying.
    assert!(matches!(s.execute("drawing.stroke", line()), Err(EngineError::Invalid(_))));
    assert!(s.project().layer(iskra_model::LayerId(e)).unwrap().drawings.is_empty());

    // A new drawing has vector art on both art layers; its shapes are seen as pixels.
    let id = s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 0 })).unwrap()["id"].as_u64().unwrap();
    s.edit("Draw", |p| {
        let drawing = p.layer_mut(iskra_model::LayerId(e)).unwrap().drawing_mut(iskra_model::DrawingId(id)).unwrap();
        assert!(drawing.layer(ArtLayerKind::ColourArt).unwrap().content.is_vector());
        let art = &mut drawing.layer_mut(ArtLayerKind::LineArt).unwrap().content;
        assert!(art.is_vector());
        let mut path = iskra_model::kurbo::BezPath::new();
        path.move_to((100.0, 100.0));
        path.line_to((400.0, 100.0));
        let stroke = VectorShape { id: ShapeId(1), swatch: iskra_model::SwatchId(3), kind: ShapeKind::Stroke(VectorStroke::even(path, 10.0)) };
        *art = ArtContent::Vector(VectorArt::new(vec![stroke]));
        Ok(())
    })
    .unwrap();
    assert_eq!((line_pixel(&s, e, 0, 250, 100), line_pixel(&s, e, 0, 250, 120)), (Some((3, 65535)), Some((0, 0))));

    // Made indexed again, the shapes become the pixels they were drawn as.
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "indexed" })).unwrap();
    let drawing = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawings[0].drawing;
    assert!(matches!(drawing.layer(ArtLayerKind::LineArt).unwrap().content, ArtContent::Indexed(_)));
    assert_eq!(line_pixel(&s, e, 0, 250, 100), Some((3, 65535)));
    assert!(s.project().layer(iskra_model::LayerId(e)).unwrap().vector.is_empty());
}

#[test]
fn the_cutter_cuts_vector_lines_along_its_box_and_clears_pixels_inside() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId, ShapeKind};
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    // Pixels: what is inside goes, and kept, nothing changes.
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
    let cut = json!({ "layer": e, "frame": 0, "rect": [180, 50, 220, 150] });
    let mut kept = cut.clone();
    kept["keep"] = json!(true);
    assert_eq!(s.execute("drawing.cutAlong", kept).unwrap()["artLayers"], json!(0));
    s.execute("drawing.cutAlong", cut.clone()).unwrap();
    assert_eq!((line_pixel(&s, e, 0, 200, 100), line_pixel(&s, e, 0, 300, 100)), (Some((0, 0)), Some((3, 65535))));
    assert_eq!(s.history().undo_label(), Some("Cut"));

    // Vector art: the line is cut along the box's edge, in two strokes, with nothing between.
    s.execute("drawing.clear", json!({ "layer": e, "frame": 0, "rect": [0, 0, 1000, 1000] })).unwrap();
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let points: Vec<Value> = (0..=20).map(|i| json!({ "x": 100 + i * 15, "y": 100 })).collect();
    let even = json!({ "size": 4, "pressureSize": false, "smoothing": 0 });
    s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": points, "brush": even })).unwrap();
    s.execute("drawing.cutAlong", cut).unwrap();
    let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
    let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector line art") };
    assert_eq!(art.shapes().iter().filter(|shape| matches!(shape.kind, ShapeKind::Stroke(_))).count(), 2);
    assert_eq!((line_pixel(&s, e, 0, 200, 100), line_pixel(&s, e, 0, 300, 100).map(|(swatch, _)| swatch)), (Some((0, 0)), Some(1)));
}

#[test]
fn the_cutter_cuts_every_drawing_of_the_layer_when_asked() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 20, "pressureSize": false }))).unwrap();
    s.execute("drawing.newDrawing", json!({ "layer": e, "frame": 1 })).unwrap();
    s.execute("drawing.stroke", stroke_params(e, 1, json!({ "size": 20, "pressureSize": false }))).unwrap();
    // Of the drawing in hand alone, the other keeps its line.
    let cut = json!({ "layer": e, "frame": 0, "rect": [180, 50, 220, 150] });
    s.execute("drawing.cutAlong", cut.clone()).unwrap();
    assert_eq!((line_pixel(&s, e, 0, 200, 100), line_pixel(&s, e, 1, 200, 100)), (Some((0, 0)), Some((3, 65535))));
    assert!(s.undo());
    // Of every drawing, in one step.
    let mut every = cut;
    every["everyDrawing"] = json!(true);
    assert_eq!(s.execute("drawing.cutAlong", every).unwrap()["artLayers"], json!(2));
    assert_eq!((line_pixel(&s, e, 0, 200, 100), line_pixel(&s, e, 1, 200, 100)), (Some((0, 0)), Some((0, 0))));
    assert_eq!((line_pixel(&s, e, 1, 300, 100), s.history().undo_label()), (Some((3, 65535)), Some("Cut")));
}

#[test]
fn a_vector_stroke_goes_behind_or_repaints_what_it_touches() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let even = json!({ "size": 4, "pressureSize": false, "smoothing": 0 });
    let line = |from: [i32; 2], to: [i32; 2]| -> Vec<Value> {
        (0..=20).map(|i| json!({ "x": from[0] + (to[0] - from[0]) * i / 20, "y": from[1] + (to[1] - from[1]) * i / 20 })).collect()
    };
    let draw = |s: &mut Session, swatch: u16, from, to, mode: &str| {
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": swatch, "points": line(from, to), "brush": even, "mode": mode }))
    };
    let swatches = |s: &Session| {
        let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector line art") };
        art.shapes().iter().map(|shape| shape.swatch.0).collect::<Vec<_>>()
    };
    draw(&mut s, 1, [100, 100], [300, 100], "over").unwrap();
    // Behind: under the line already there, whose colour shows where they cross.
    draw(&mut s, 4, [200, 50], [200, 150], "behind").unwrap();
    assert_eq!(swatches(&s), vec![4, 1]);
    assert_eq!(line_pixel(&s, e, 0, 200, 100).map(|(swatch, _)| swatch), Some(1));
    // Repainting across the line's left: it takes the swatch, and nothing new is laid.
    let repainted = draw(&mut s, 5, [130, 60], [130, 140], "repaint").unwrap();
    assert_eq!((swatches(&s), repainted["shape"].clone()), (vec![4, 5], Value::Null));
    assert_eq!(line_pixel(&s, e, 0, 130, 120), Some((0, 0)), "the canvas stays empty");
    // Flattened across the repainted line, of its swatch: one shape with it, a fill now.
    let points = line([250, 60], [250, 140]);
    s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 5, "points": points, "brush": even, "flatten": true })).unwrap();
    assert_eq!(swatches(&s), vec![4, 5]);
    let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
    let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector line art") };
    assert!(matches!(art.shapes()[1].kind, iskra_model::ShapeKind::Fill(_)));
}

#[test]
fn a_surface_stroke_leaves_an_area_merged_with_those_of_its_swatch() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let even = json!({ "size": 12, "pressureSize": false, "smoothing": 0 });
    let draw = |s: &mut Session, swatch: u16, from: [i32; 2], to: [i32; 2]| {
        let points: Vec<Value> = (0..=20).map(|i| json!({ "x": from[0] + (to[0] - from[0]) * i / 20, "y": from[1] + (to[1] - from[1]) * i / 20 })).collect();
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": swatch, "points": points, "brush": even, "surface": true })).unwrap();
    };
    let shapes = |s: &Session| {
        let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector line art") };
        art.shapes().iter().map(|shape| (shape.swatch.0, matches!(&shape.kind, ShapeKind::Fill(fill) if !fill.between_lines()))).collect::<Vec<_>>()
    };
    // Alone, an area with no middle line, as wide as the brush.
    draw(&mut s, 1, [100, 100], [300, 100]);
    assert_eq!(shapes(&s), vec![(1, true)]);
    assert_eq!(line_pixel(&s, e, 0, 200, 104).map(|(swatch, _)| swatch), Some(1));
    assert_eq!(line_pixel(&s, e, 0, 200, 110), Some((0, 0)));
    // Across it, of its swatch: one area with it, in one step.
    let steps = s.history().entries().len();
    draw(&mut s, 1, [200, 50], [200, 150]);
    assert_eq!((shapes(&s), s.history().entries().len()), (vec![(1, true)], steps + 1));
    assert_eq!(line_pixel(&s, e, 0, 200, 140).map(|(swatch, _)| swatch), Some(1));
    // Of another swatch it lies on top, apart.
    draw(&mut s, 4, [250, 50], [250, 150]);
    assert_eq!(shapes(&s), vec![(1, true), (4, true)]);
    assert_eq!(line_pixel(&s, e, 0, 250, 100).map(|(swatch, _)| swatch), Some(4));
    assert!(s.undo());
    assert!(s.undo());
    assert_eq!(shapes(&s), vec![(1, true)]);
    assert_eq!(line_pixel(&s, e, 0, 200, 140), Some((0, 0)));
}

#[test]
fn the_scatter_brush_sprays_copies_of_a_preset_or_of_the_clipboard() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let path: Vec<[f64; 2]> = (0..=20).map(|i| [100.0 + f64::from(i) * 10.0, 100.0]).collect();
    let swatches = |s: &Session| {
        let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector line art") };
        art.shapes().iter().map(|shape| shape.swatch.0).collect::<Vec<_>>()
    };
    // Dots 20 across, 50 apart, along 200 pixels: five, in the swatch, in one step.
    let dots = json!({ "layer": e, "frame": 0, "swatch": 3, "points": path, "scatter": { "preset": "dots", "minSize": 20, "maxSize": 20, "spacing": 50 } });
    let steps = s.history().entries().len();
    assert_eq!(s.execute("drawing.scatter", dots.clone()).unwrap()["copies"], json!(5));
    assert_eq!((swatches(&s), s.history().entries().len(), s.history().undo_label()), (vec![3; 5], steps + 1, Some("Scatter")));
    assert_eq!(line_pixel(&s, e, 0, 150, 100).map(|(swatch, _)| swatch), Some(3));
    assert_eq!(line_pixel(&s, e, 0, 175, 100), Some((0, 0)), "nothing between them");
    // From the clipboard: refused while it is empty; once a dot is copied, copies of it in its
    // own swatch, whatever swatch is in hand.
    let mut clipped = dots.clone();
    clipped["scatter"]["preset"] = json!("clipboard");
    assert!(matches!(s.execute("drawing.scatter", clipped.clone()), Err(EngineError::Invalid(_))));
    assert_eq!(s.execute("drawing.copy", json!({ "layer": e, "frame": 0, "rect": [130, 80, 170, 120] })).unwrap()["copied"], json!(true));
    clipped["swatch"] = json!(4);
    s.execute("drawing.scatter", clipped).unwrap();
    assert_eq!(swatches(&s), vec![3; 10]);
    // Not on pixels, nor along nowhere.
    let mut nowhere = dots;
    nowhere["points"] = json!([]);
    assert!(matches!(s.execute("drawing.scatter", nowhere), Err(EngineError::Invalid(_))));
}

#[test]
fn a_surface_drawn_behind_merges_with_its_swatch_under_the_rest() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let even = json!({ "size": 12, "pressureSize": false, "smoothing": 0 });
    let draw = |s: &mut Session, swatch: u16, from: [i32; 2], to: [i32; 2], mode: &str| {
        let points: Vec<Value> = (0..=20).map(|i| json!({ "x": from[0] + (to[0] - from[0]) * i / 20, "y": from[1] + (to[1] - from[1]) * i / 20 })).collect();
        let params = json!({ "layer": e, "frame": 0, "swatch": swatch, "points": points, "brush": even, "surface": true, "mode": mode });
        s.execute("drawing.vectorStroke", params).unwrap();
    };
    let swatches = |s: &Session| {
        let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector line art") };
        art.shapes().iter().map(|shape| shape.swatch.0).collect::<Vec<_>>()
    };
    draw(&mut s, 1, [100, 100], [300, 100], "over");
    draw(&mut s, 4, [200, 50], [200, 150], "over");
    assert_eq!(swatches(&s), vec![1, 4]);
    // Behind, across both: one area with the first, of its swatch, and under the other.
    draw(&mut s, 1, [150, 50], [150, 150], "behind");
    assert_eq!(swatches(&s), vec![1, 4]);
    assert_eq!(line_pixel(&s, e, 0, 150, 140).map(|(swatch, _)| swatch), Some(1));
    assert_eq!(line_pixel(&s, e, 0, 200, 100).map(|(swatch, _)| swatch), Some(4), "the other swatch stays on top");
    // Behind, of a swatch no shape has: under them all, alone.
    draw(&mut s, 5, [250, 50], [250, 150], "behind");
    assert_eq!(swatches(&s), vec![5, 1, 4]);
    assert_eq!(line_pixel(&s, e, 0, 250, 100).map(|(swatch, _)| swatch), Some(1));
}

#[test]
fn the_bucket_fills_between_surfaces_and_a_surface_across_cuts_the_fill() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let even = json!({ "size": 6, "pressureSize": false, "smoothing": 0 });
    let side = |from: [i32; 2], to: [i32; 2]| -> Vec<Value> {
        (0..=20).map(|i| json!({ "x": from[0] + (to[0] - from[0]) * i / 20, "y": from[1] + (to[1] - from[1]) * i / 20 })).collect()
    };
    let surface = |s: &mut Session, from, to| {
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": side(from, to), "brush": even, "surface": true })).unwrap();
    };
    // Four sides drawn as surfaces: one ring, merged.
    for (from, to) in [([100, 100], [300, 100]), ([300, 100], [300, 300]), ([300, 300], [100, 300]), ([100, 300], [100, 100])] {
        surface(&mut s, from, to);
    }
    let art_of = |s: &Session, art: ArtLayerKind| {
        let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
        let ArtContent::Vector(vector) = &drawing.layer(art).unwrap().content else { panic!("vector art") };
        vector.shapes().iter().map(|shape| (shape.swatch.0, matches!(&shape.kind, ShapeKind::Fill(fill) if fill.between_lines()))).collect::<Vec<_>>()
    };
    assert_eq!(art_of(&s, ArtLayerKind::LineArt), [(1, false)]);
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 150, "y": 200, "swatch": 4 })).unwrap();
    assert_eq!(art_of(&s, ArtLayerKind::ColourArt), [(4, true)]);
    // A surface across it, joined to the ring: two areas, each filled.
    surface(&mut s, [200, 80], [200, 320]);
    assert_eq!((art_of(&s, ArtLayerKind::ColourArt), art_of(&s, ArtLayerKind::LineArt)), (vec![(4, true), (4, true)], vec![(1, false)]));
    // The bucket on the line art gives the ring the swatch.
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 200, "y": 101, "swatch": 5, "artLayer": "lineArt" })).unwrap();
    assert_eq!(art_of(&s, ArtLayerKind::LineArt), [(5, false)]);
}

#[test]
fn a_vector_stroke_across_a_fill_cuts_it_in_one_step() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let even = json!({ "size": 4, "pressureSize": false, "smoothing": 0 });
    let side = |from: [i32; 2], to: [i32; 2]| -> Vec<Value> {
        (0..=20).map(|i| json!({ "x": from[0] + (to[0] - from[0]) * i / 20, "y": from[1] + (to[1] - from[1]) * i / 20 })).collect()
    };
    for (from, to) in [([100, 100], [300, 100]), ([300, 100], [300, 300]), ([300, 300], [100, 300]), ([100, 300], [100, 100])] {
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": side(from, to), "brush": even })).unwrap();
    }
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 150, "y": 200, "swatch": 4 })).unwrap();
    let fills = |s: &Session| {
        let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::ColourArt).unwrap().content else { panic!("vector colour art") };
        art.shapes().iter().filter(|shape| matches!(shape.kind, ShapeKind::Fill(_))).map(|shape| shape.swatch.0).collect::<Vec<_>>()
    };
    assert_eq!(fills(&s), [4]);
    s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": side([200, 80], [200, 320]), "brush": even })).unwrap();
    assert_eq!((fills(&s), s.history().undo_label()), (vec![4, 4], Some("Vector Stroke")));
    s.undo();
    assert_eq!(fills(&s), [4], "one step");
}

#[test]
fn an_art_layers_output_is_wired_as_a_node_kept_in_the_file_and_goes_with_its_layer() {
    use iskra_model::{ArtLayerKind, LayerId, Source};
    let mut s = Session::new();
    let e = layer(&s);
    let all = s.project().shots[0].composites[0].id.0;
    // The colour art into the composite beside the flat output, then the line art too.
    s.execute("node.disconnect", json!({ "node": { "layer": e }, "from": all })).unwrap();
    s.execute("node.connect", json!({ "node": { "art": [e, "colourArt"] }, "to": all })).unwrap();
    s.execute("node.connect", json!({ "node": { "art": [e, "lineArt"] }, "to": all, "index": 0 })).unwrap();
    let wired = |s: &Session| s.project().shots[0].composites[0].inputs.clone();
    let (line, colour) = (Source::Art(LayerId(e), ArtLayerKind::LineArt), Source::Art(LayerId(e), ArtLayerKind::ColourArt));
    assert_eq!(wired(&s), [line, colour]);
    assert_eq!(s.project().shots[0].stack(), [LayerId(e)], "the layer is seen");
    // Kept in the file as which output the wire leaves from; a wire from before is the flat one.
    let json = serde_json::to_value(&s.project().shots[0].composites[0].inputs).unwrap();
    assert_eq!(json, json!([{ "art": [e, "lineArt"] }, { "art": [e, "colourArt"] }]));
    let read: Vec<Source> = serde_json::from_value(json!([{ "layer": e }])).unwrap();
    assert_eq!(read, [Source::Layer(LayerId(e))]);
    // Its outputs go with the layer.
    s.execute("layer.new", json!({})).unwrap();
    s.execute("layer.delete", json!({ "layer": e })).unwrap();
    assert!(!wired(&s).iter().any(|input| input.layer() == Some(LayerId(e))), "{:?}", wired(&s));
}

#[test]
fn a_layer_holds_one_kind_of_art() {
    use iskra_model::{ArtLayerKind, LayerId};
    let mut s = Session::new();
    let e = layer(&s);
    let of = |s: &Session| {
        let layer = s.project().layer(LayerId(e)).unwrap();
        (layer.vector.clone(), layer.free_colour.clone(), layer.is_vector(), layer.is_mixed())
    };
    // Any art layer made vector makes the whole layer vector.
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "colourArt", "mode": "vector" })).unwrap();
    assert_eq!(of(&s), (ArtLayerKind::ALL.to_vec(), vec![], true, false));
    // Any made pixels makes it all pixels: the one as asked, the others free colour, as
    // bitmap art is.
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "colourArt", "mode": "indexed" })).unwrap();
    assert_eq!(of(&s), (vec![], vec![ArtLayerKind::LineArt, ArtLayerKind::Overart], false, false));
    // On pixels, each art layer keeps its own: indexed or free colour.
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "indexed" })).unwrap();
    assert_eq!(of(&s).1, vec![ArtLayerKind::Overart]);
    // A new layer is free colour pixels, or vector as asked; so is a new project's.
    let made = s.execute("layer.new", json!({ "kind": "vector" })).unwrap()["id"].as_u64().unwrap();
    assert!(s.project().layer(LayerId(made)).unwrap().is_vector() && s.project().layer(LayerId(made)).unwrap().free_colour.is_empty());
    let made = s.execute("layer.new", json!({})).unwrap()["id"].as_u64().unwrap();
    let bitmap = |s: &Session, layer: LayerId| {
        let layer = s.project().layer(layer).unwrap();
        layer.vector.is_empty() && layer.free_colour == ArtLayerKind::ALL && layer.blank_drawing().art_layers.iter().all(|art| art.content.is_rgba())
    };
    assert!(bitmap(&s, LayerId(made)));
    let fresh = Session::new();
    assert!(bitmap(&fresh, LayerId(layer(&fresh))));
    // A mixed layer from before is kept so until its kind is set.
    s.edit("Mixed", |p| {
        p.layer_mut(LayerId(e)).unwrap().vector = vec![ArtLayerKind::LineArt];
        Ok(())
    })
    .unwrap();
    assert!(of(&s).3);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    assert!(of(&s).2);
}

#[test]
fn a_vector_stroke_is_kept_as_a_fitted_line_with_its_pressure_as_its_width() {
    use iskra_model::{ArtContent, ArtLayerKind, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    let points: Vec<Value> = (0..=60).map(|i| json!({ "x": 100 + i * 5, "y": 200, "pressure": f64::from(i) / 60.0 })).collect();
    let params = |swatch: u16, points: &[Value]| json!({ "layer": e, "frame": 0, "swatch": swatch, "points": points, "brush": { "size": 20, "thinnest": 0.0, "smoothing": 0 } });
    // On pixels it is refused; the layer's line art is made vector art first.
    assert!(matches!(s.execute("drawing.vectorStroke", params(3, &points)), Err(EngineError::Invalid(_))));
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let done = s.execute("drawing.vectorStroke", params(3, &points)).unwrap();
    assert_eq!((done["shape"].as_u64(), s.history().undo_label()), (Some(1), Some("Vector Stroke")));
    assert_eq!(cells(&s, e, 2), "1 -", "a drawing made in the empty cell");
    let shapes = |s: &Session| {
        let drawing = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawings[0].drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector art") };
        art.shapes().to_vec()
    };
    let ShapeKind::Stroke(stroke) = &shapes(&s)[0].kind else { panic!("a stroke") };
    assert_eq!(stroke.path.segments().count(), 1, "a straight line is one segment");
    assert!(stroke.widths[0] < 1.0 && (stroke.widths[1] - 20.0).abs() < 0.5, "thin to thick: {:?}", stroke.widths);
    // Seen as pixels: thick at the end, thin at the start.
    assert_eq!(line_pixel(&s, e, 0, 395, 208).map(|(swatch, _)| swatch), Some(3));
    assert_eq!(line_pixel(&s, e, 0, 110, 208), Some((0, 0)));

    // A stroke across one of a protected swatch goes under it; another goes on top.
    let across: Vec<Value> = (0..=20).map(|i| json!({ "x": 300, "y": 150 + i * 5, "pressure": 1.0 })).collect();
    let mut protected = params(4, &across);
    protected["protect"] = json!([3]);
    s.execute("drawing.vectorStroke", protected).unwrap();
    s.execute("drawing.vectorStroke", params(5, &across)).unwrap();
    assert_eq!(shapes(&s).iter().map(|shape| (shape.id.0, shape.swatch.0)).collect::<Vec<_>>(), [(2, 4), (1, 3), (3, 5)]);
    s.undo();
    s.undo();
    assert_eq!(shapes(&s).len(), 1);
    assert!(matches!(s.execute("drawing.vectorStroke", params(99, &points)), Err(EngineError::Invalid(_))), "no such swatch");
    assert!(matches!(s.execute("drawing.vectorStroke", params(3, &[])), Err(EngineError::Invalid(_))), "no points");
}

#[test]
fn a_vector_stroke_is_faint_where_pressed_lightly_and_drifts_where_pressed_hard() {
    use iskra_model::SwatchId;
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    // Red drifts a third of the way round its hue, to green.
    assert!(matches!(s.execute("palette.set", json!({ "swatch": 3, "drift": [200, 0, 0] })), Err(EngineError::Invalid(_))));
    s.execute("palette.set", json!({ "swatch": 3, "drift": [120, 0, 0] })).unwrap();
    assert_eq!((s.project().palette.swatch(SwatchId(3)).unwrap().drift, s.history().undo_label()), (Some([120, 0, 0]), Some("Change Swatch")));
    let points: Vec<Value> = (0..=60).map(|i| json!({ "x": 100 + i * 5, "y": 200, "pressure": 0.05 + 0.95 * f64::from(i) / 60.0 })).collect();
    let brush = json!({ "size": 20, "pressureSize": false, "smoothing": 0, "pressureOpacity": true, "faintest": 0.0, "drift": 1.0 });
    s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 3, "points": points, "brush": brush })).unwrap();
    // Faint and red where light, opaque and green where hard.
    let (light, hard) = (line_pixel(&s, e, 0, 105, 200).unwrap(), line_pixel(&s, e, 0, 395, 200).unwrap());
    assert!(light.1 < 65535 / 8 && SwatchId(light.0).drift() <= 1, "{light:?}");
    assert!(hard.1 > 60000 && SwatchId(hard.0) == SwatchId(3).drifted(SwatchId::DRIFTS), "{hard:?}");
    let palette = &s.project().palette;
    assert_eq!(palette.color(SwatchId(hard.0)), iskra_model::drifted([220, 50, 47, 255], [120, 0, 0], 1.0));
    // No drift any more: the stroke shows its swatch's colour all along.
    s.execute("palette.set", json!({ "swatch": 3, "drift": [0, 0, 0] })).unwrap();
    assert_eq!(s.project().palette.color(SwatchId(hard.0)), [220, 50, 47, 255]);
    // Its point lists are given with its points, or none.
    let shape = json!({ "shape": 1, "path": "M100 200 L400 200", "widths": [20, 20], "opacities": [0.5, 1], "drifts": [] });
    s.execute("drawing.editShapes", json!({ "layer": e, "frame": 0, "shapes": [shape] })).unwrap();
    let bad = json!({ "shape": 1, "path": "M100 200 L400 200", "widths": [20, 20], "opacities": [0.5] });
    assert!(matches!(s.execute("drawing.editShapes", json!({ "layer": e, "frame": 0, "shapes": [bad] })), Err(EngineError::Invalid(_))));
}

#[test]
fn the_vector_eraser_cuts_strokes_and_leaves_protected_ones_and_empty_cells_alone() {
    use iskra_model::{ArtContent, ArtLayerKind, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let across: Vec<Value> = (0..=40).map(|i| json!({ "x": 100 + i * 5, "y": 200, "pressure": 1.0 })).collect();
    let even = json!({ "size": 6, "pressureSize": false, "smoothing": 0 });
    s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 3, "points": across, "brush": even })).unwrap();
    let down: Vec<Value> = (0..=10).map(|i| json!({ "x": 200, "y": 170 + i * 6, "pressure": 1.0 })).collect();
    let eraser = json!({ "size": 20, "pressureSize": false, "smoothing": 0 });
    let erase = |frame: u32, protect: Value| json!({ "layer": e, "frame": frame, "points": down, "brush": eraser, "protect": protect });
    let shapes = |s: &Session| {
        let drawing = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawings[0].drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector art") };
        art.shapes().to_vec()
    };

    // Protected, the stroke stays whole, and nothing is done; nor in an empty cell.
    let steps = s.history().entries().len();
    assert_eq!(s.execute("drawing.vectorErase", erase(0, json!([3]))).unwrap(), json!({ "erased": false }));
    assert_eq!(s.execute("drawing.vectorErase", erase(5, json!([]))).unwrap(), json!({ "erased": false }));
    assert_eq!(s.history().entries().len(), steps);

    // Across it, the stroke is cut in two strokes, in one undo step.
    assert_eq!(s.execute("drawing.vectorErase", erase(0, json!([]))).unwrap(), json!({ "erased": true }));
    assert_eq!(s.history().undo_label(), Some("Eraser Stroke"));
    let cut = shapes(&s);
    assert_eq!(cut.len(), 2);
    assert!(cut.iter().all(|shape| matches!(shape.kind, ShapeKind::Stroke(_)) && shape.swatch.0 == 3));
    assert_eq!(line_pixel(&s, e, 0, 200, 200), Some((0, 0)));
    assert_eq!(line_pixel(&s, e, 0, 120, 200).map(|(swatch, _)| swatch), Some(3));
    s.undo();
    assert_eq!(shapes(&s).len(), 1);
}

#[test]
fn the_bucket_fills_vector_art_between_vector_lines_and_the_fill_follows_an_erase() {
    use iskra_model::{ArtContent, ArtLayerKind, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    for art in ["lineArt", "colourArt"] {
        s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": art, "mode": "vector" })).unwrap();
    }
    let even = json!({ "size": 4, "pressureSize": false, "smoothing": 0 });
    let side = |from: [i32; 2], to: [i32; 2]| -> Vec<Value> {
        (0..=20).map(|i| json!({ "x": from[0] + (to[0] - from[0]) * i / 20, "y": from[1] + (to[1] - from[1]) * i / 20 })).collect()
    };
    for (from, to) in [([100, 100], [300, 100]), ([300, 100], [300, 300]), ([300, 300], [100, 300]), ([100, 300], [100, 100])] {
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": side(from, to), "brush": even })).unwrap();
    }
    let fills = |s: &Session| {
        let drawing = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawings[0].drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::ColourArt).unwrap().content else { panic!("vector colour art") };
        art.shapes()
            .iter()
            .filter_map(|shape| if let ShapeKind::Fill(fill) = &shape.kind { Some((shape.swatch.0, fill.open)) } else { None })
            .collect::<Vec<_>>()
    };
    let done = s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 200, "y": 200, "swatch": 4 })).unwrap();
    assert_eq!((done["filled"].as_u64(), s.history().undo_label(), fills(&s)), (Some(1), Some("Fill"), vec![(4, false)]));
    // Outside the lines there is no area to fill.
    assert!(matches!(s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 500, "y": 500, "swatch": 4 })), Err(EngineError::Invalid(_))));
    // The bucket on the lines gives a line the swatch.
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 200, "y": 101, "swatch": 5, "artLayer": "lineArt" })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Recolour Line"));
    // The left side erased: the fill's lines no longer close round it, and it stays, marked open.
    let down: Vec<Value> = (0..=20).map(|i| json!({ "x": 100, "y": 90 + i * 11 })).collect();
    s.execute("drawing.vectorErase", json!({ "layer": e, "frame": 0, "points": down, "brush": { "size": 20, "pressureSize": false, "smoothing": 0 } }))
        .unwrap();
    assert_eq!(fills(&s), [(4, true)]);
    s.undo();
    assert_eq!(fills(&s), [(4, false)]);
}

#[test]
fn the_bucket_on_pixels_is_bounded_by_vector_line_art() {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    // A layer from before a layer held one kind: vector lines over colour art of pixels.
    s.edit("Mixed", |p| {
        p.layer_mut(iskra_model::LayerId(e)).unwrap().vector = vec![iskra_model::ArtLayerKind::LineArt];
        Ok(())
    })
    .unwrap();
    let even = json!({ "size": 4, "pressureSize": false, "smoothing": 0 });
    let side = |from: [i32; 2], to: [i32; 2]| -> Vec<Value> {
        (0..=20).map(|i| json!({ "x": from[0] + (to[0] - from[0]) * i / 20, "y": from[1] + (to[1] - from[1]) * i / 20 })).collect()
    };
    for (from, to) in [([100, 100], [300, 100]), ([300, 100], [300, 300]), ([300, 300], [100, 300]), ([100, 300], [100, 100])] {
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": side(from, to), "brush": even })).unwrap();
    }
    // The colour art is pixels: the fill stops at the lines as they are drawn.
    let done = s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 200, "y": 200, "swatch": 4 })).unwrap();
    assert_eq!(done["filled"].as_u64(), Some(1));
    assert_eq!((colour_pixel(&s, e, 200, 200), colour_pixel(&s, e, 400, 200)), ((4, 65535), (0, 0)));
}

#[test]
fn a_selection_moves_copies_recolours_and_clears_whole_vector_shapes_and_fills_follow() {
    use iskra_model::{ArtContent, ArtLayerKind, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    for art in ["lineArt", "colourArt"] {
        s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": art, "mode": "vector" })).unwrap();
    }
    let even = json!({ "size": 4, "pressureSize": false, "smoothing": 0 });
    let side = |from: [i32; 2], to: [i32; 2]| -> Vec<Value> {
        (0..=20).map(|i| json!({ "x": from[0] + (to[0] - from[0]) * i / 20, "y": from[1] + (to[1] - from[1]) * i / 20 })).collect()
    };
    // The top and the bottom run on past the right side, which can then move along them.
    for (from, to) in [([100, 100], [500, 100]), ([300, 100], [300, 300]), ([500, 300], [100, 300]), ([100, 300], [100, 100])] {
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": side(from, to), "brush": even })).unwrap();
    }
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 200, "y": 200, "swatch": 4 })).unwrap();
    let art = |s: &Session, kind: ArtLayerKind| {
        let drawing = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawings[0].drawing;
        let ArtContent::Vector(art) = &drawing.layer(kind).unwrap().content else { panic!("vector art") };
        art.clone()
    };
    let fill_at = |s: &Session, x: i32| art(s, ArtLayerKind::ColourArt).raster().pixel(x, 200).swatch.0;

    // A box round the right side moves it out by 60: the side keeps its id and place, and the
    // fill follows it.
    let right = json!({ "layer": e, "frame": 0, "artLayer": "lineArt", "rect": [290, 150, 310, 250] });
    let mut moved = right.clone();
    moved["matrix"] = json!([1, 0, 0, 1, 60, 0]);
    let landed = s.execute("drawing.transform", moved).unwrap()["rect"].clone();
    assert!(landed[0].as_i64().unwrap() >= 355 && landed[2].as_i64().unwrap() <= 365, "{landed}");
    let lines = art(&s, ArtLayerKind::LineArt);
    assert_eq!(lines.shapes().iter().map(|shape| shape.id.0).collect::<Vec<_>>(), [1, 2, 3, 4]);
    assert_eq!((fill_at(&s, 330), fill_at(&s, 380)), (4, 0), "the fill reaches the moved side, and no further");

    // Copied and pasted, recoloured, and cleared, as shapes.
    assert_eq!(s.execute("drawing.copy", right.clone()).unwrap(), json!({ "copied": false }), "nothing there now");
    let at_new = json!({ "layer": e, "frame": 0, "artLayer": "lineArt", "rect": [350, 150, 370, 250] });
    assert_eq!(s.execute("drawing.copy", at_new.clone()).unwrap(), json!({ "copied": true }));
    let mut recolour = at_new.clone();
    recolour["to"] = json!(5);
    assert_eq!(s.execute("drawing.recolourSelection", recolour).unwrap(), json!({ "artLayers": 1 }));
    assert_eq!(art(&s, ArtLayerKind::LineArt).shapes()[1].swatch.0, 5);
    s.execute("drawing.clear", at_new).unwrap();
    let lines = art(&s, ArtLayerKind::LineArt);
    assert_eq!(lines.shapes().len(), 3);
    let colour = art(&s, ArtLayerKind::ColourArt);
    let ShapeKind::Fill(fill) = &colour.shapes()[0].kind else { panic!("a fill") };
    assert!(fill.open, "its side went: the fill keeps its outline, open");
}

#[test]
fn edited_points_give_a_shape_its_new_path_and_the_fill_follows() {
    use iskra_model::{ArtContent, ArtLayerKind, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    for art in ["lineArt", "colourArt"] {
        s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": art, "mode": "vector" })).unwrap();
    }
    let even = json!({ "size": 4, "pressureSize": false, "smoothing": 0 });
    let side = |from: [i32; 2], to: [i32; 2]| -> Vec<Value> {
        (0..=20).map(|i| json!({ "x": from[0] + (to[0] - from[0]) * i / 20, "y": from[1] + (to[1] - from[1]) * i / 20 })).collect()
    };
    for (from, to) in [([100, 100], [500, 100]), ([300, 100], [300, 300]), ([500, 300], [100, 300]), ([100, 300], [100, 100])] {
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": side(from, to), "brush": even })).unwrap();
    }
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 200, "y": 200, "swatch": 4 })).unwrap();
    let art = |s: &Session, kind: ArtLayerKind| {
        let drawing = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawings[0].drawing;
        let ArtContent::Vector(art) = &drawing.layer(kind).unwrap().content else { panic!("vector art") };
        art.clone()
    };
    // The right side's two points dragged out to 380, its width there made 10.
    let edit = json!({ "layer": e, "frame": 0, "shapes": [{ "shape": 2, "path": "M380,100 L380,300", "widths": [4.0, 10.0] }] });
    s.execute("drawing.editShapes", edit).unwrap();
    assert_eq!(s.history().undo_label(), Some("Edit Points"));
    let lines = art(&s, ArtLayerKind::LineArt);
    let ShapeKind::Stroke(stroke) = &lines.shapes()[1].kind else { panic!("a stroke") };
    assert_eq!((stroke.path.to_svg(), stroke.widths.clone()), ("M380,100 L380,300".to_string(), vec![4.0, 10.0]));
    assert_eq!(art(&s, ArtLayerKind::ColourArt).raster().pixel(350, 200).swatch.0, 4, "the fill followed the side");
    // Widths for another count of points, or a shape that isn't there, are refused.
    let wrong = json!({ "layer": e, "frame": 0, "shapes": [{ "shape": 2, "path": "M380,100 L380,200 L380,300" }] });
    assert!(matches!(s.execute("drawing.editShapes", wrong), Err(EngineError::Invalid(_))));
    let missing = json!({ "layer": e, "frame": 0, "shapes": [{ "shape": 9, "path": "M0,0 L1,1" }] });
    assert!(matches!(s.execute("drawing.editShapes", missing), Err(EngineError::Invalid(_))));
    s.undo();
    assert_eq!(art(&s, ArtLayerKind::ColourArt).raster().pixel(350, 200).coverage, 0);
}

#[test]
fn a_brush_is_laid_on_strokes_already_drawn() {
    use iskra_model::{ArtContent, ArtLayerKind, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let even = json!({ "size": 4, "pressureSize": false, "smoothing": 0 });
    for y in [100, 200] {
        let points: Vec<Value> = (0..=20).map(|i| json!({ "x": 100 + i * 15, "y": y })).collect();
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": points, "brush": even })).unwrap();
    }
    let strokes = |s: &Session| {
        let drawing = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawings[0].drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector art") };
        art.shapes().iter().map(|shape| if let ShapeKind::Stroke(stroke) = &shape.kind { stroke.clone() } else { panic!("strokes") }).collect::<Vec<_>>()
    };
    let before = strokes(&s);
    // The first alone, with a brush of 12 and a calligraphic nib: its line stays, its width is the brush's.
    let brush = json!({ "size": 12, "pressureSize": false, "angle": 45, "roundness": 0.3 });
    let laid = s.execute("drawing.layBrush", json!({ "layer": e, "frame": 0, "shapes": [1], "brush": brush })).unwrap();
    let after = strokes(&s);
    assert_eq!((laid["strokes"].clone(), s.history().undo_label()), (json!(1), Some("Lay Brush")));
    assert_eq!(after[0].path, before[0].path);
    assert!(after[0].widths.iter().all(|width| (width - 12.0).abs() < 1e-6) && (after[0].nib.roundness - 0.3).abs() < 1e-6, "{:?}", after[0]);
    assert_eq!(after[1], before[1], "the other is left");
    // A brush of no size is refused.
    let none = json!({ "layer": e, "frame": 0, "shapes": [2], "brush": { "size": 0 } });
    assert!(matches!(s.execute("drawing.layBrush", none), Err(EngineError::Invalid(_))));
}

#[test]
fn a_line_is_split_at_the_crossings_around_a_point_for_the_select_tool() {
    use iskra_model::{ArtContent, ArtLayerKind};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    let even = json!({ "size": 4, "pressureSize": false, "smoothing": 0 });
    let line = |from: [i32; 2], to: [i32; 2]| -> Vec<Value> {
        (0..=20).map(|i| json!({ "x": from[0] + (to[0] - from[0]) * i / 20, "y": from[1] + (to[1] - from[1]) * i / 20 })).collect()
    };
    for (from, to) in [([100, 200], [400, 200]), ([200, 100], [200, 300]), ([300, 100], [300, 300])] {
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": line(from, to), "brush": even })).unwrap();
    }
    let count = |s: &Session| {
        let drawing = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawings[0].drawing;
        let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector art") };
        art.shapes().len()
    };
    let split = s.execute("drawing.splitPiece", json!({ "layer": e, "frame": 0, "x": 250, "y": 201, "reach": 5 })).unwrap();
    assert_eq!((split["shape"].clone(), count(&s), s.history().undo_label()), (json!(1), 5, Some("Split Line")));
    let xs: Vec<f64> = split["line"].as_array().unwrap().iter().map(|at| at[0].as_f64().unwrap()).collect();
    assert!(xs.iter().all(|x| (199.0..=301.0).contains(x)), "the piece between the crossings: {xs:?}");
    // Nowhere near a line, nothing.
    let steps = s.history().entries().len();
    assert_eq!(s.execute("drawing.splitPiece", json!({ "layer": e, "frame": 0, "x": 250, "y": 250, "reach": 5 })).unwrap(), Value::Null);
    assert_eq!(s.history().entries().len(), steps);
}

#[test]
fn a_drawing_is_traced_into_vector_art_and_made_pixels_again() {
    use iskra_model::{ArtContent, ArtLayerKind};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("drawing.stroke", stroke_params(e, 0, json!({ "size": 10, "pressureSize": false }))).unwrap();
    // Traced as lines, it is one stroke along the line's middle.
    let lines = json!({ "layer": e, "frame": 0, "trace": { "lines": true, "lineWidth": 14 } });
    assert_eq!(s.execute("drawing.vectorize", lines).unwrap(), json!({ "shapes": 1 }));
    assert_eq!(s.history().undo_label(), Some("Make Vector Art"));
    let drawing = |s: &Session| s.project().layer(iskra_model::LayerId(e)).unwrap().drawings[0].drawing.clone();
    let line = drawing(&s);
    let ArtContent::Vector(art) = &line.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector art") };
    assert!(matches!(art.shapes()[0].kind, iskra_model::ShapeKind::Stroke(_)));
    assert_eq!(line_pixel(&s, e, 0, 250, 100).map(|(swatch, _)| swatch), Some(3));
    // And back to the pixels it is drawn as.
    s.execute("drawing.rasterize", json!({ "layer": e, "frame": 0 })).unwrap();
    assert!(matches!(drawing(&s).layer(ArtLayerKind::LineArt).unwrap().content, ArtContent::Indexed(_)));
    assert_eq!(line_pixel(&s, e, 0, 250, 100).map(|(swatch, _)| swatch), Some(3));
    assert!(matches!(s.execute("drawing.rasterize", json!({ "layer": e, "frame": 0 })), Err(EngineError::Invalid(_))), "pixels already");
}

/// A rig's scene written for the tests, with a palette: under a peg in a group, an arm whose
/// drawing node has a small offset of its own, a deformer between them, and its underlay art
/// cutting a hand; a node of a kind not brought; and the group into a composite the display
/// shows.
fn rig_scene(dir: &std::path::Path) -> PathBuf {
    let scene = r#"<?xml version="1.0" encoding="UTF-8"?>
<project>
 <elements>
  <element id="2" elementName="ARM" elementFolder="ARM" rootFolder="elements"><drawings><dwg name="1"/><dwg name="bent"/></drawings></element>
  <element id="3" elementName="HAND" elementFolder="HAND" rootFolder="elements"><drawings><dwg name="1"/></drawings></element>
 </elements>
 <options>
  <metrics unitAspectRatioX="4" unitAspectRatioY="3" numberOfUnitsX="24" numberOfUnitsY="24" numberOfUnitsZ="12"/>
  <resolution size="1920,1080"/>
  <framerate val="25"/>
 </options>
 <scenes><scene name="Top" startFrame="1">
  <columns>
   <column type="0" name="ARM-COL"><elementSeq exposures="1-10" val="bent" id="2"/></column>
   <column type="0" name="HAND-COL"><elementSeq exposures="1-10" val="1" id="3"/></column>
   <column type="3" name="X"><points><pt constSeg="true" x="1" y="2"/></points></column>
  </columns>
  <rootgroup name="Top">
   <nodeslist>
    <module type="COMPOSITE" name="Composite"><attrs><compositeMode val="compositePassthrough"/></attrs></module>
    <module type="DISPLAY" name="Display"/>
    <module type="WRITE" name="Write"/>
    <group name="HERO"><nodeslist>
     <module type="MULTIPORT_IN" name="Multi-Port-In"/>
     <module type="PEG" name="HERO-P"><attrs><position><separate val="true"/><x val="0" col="X"/><y val="-1"/><z val="0.5"/></position><angle val="90"/></attrs></module>
     <module type="PEG" name="ARM-P" pos="100,100,1"><attrs><position><separate val="true"/><x val="1"/><y val="0"/></position><scale><separate val="false"/><xy val="2"/></scale></attrs></module>
     <group name="Deformation-ARM" pos="100,150,2"><nodeslist>
      <module type="MULTIPORT_IN" name="Multi-Port-In"/>
      <module type="OffsetModule" name="Offset"><attrs><localreferential val="false"/><restingoffset><x val="0"/><y val="0"/></restingoffset><restingorientation val="0"/></attrs></module>
      <module type="CurveModule" name="Curve"><attrs><localreferential val="false"/><influencetype val="INFINITE_INFLUENCE"/>
       <restingoffset><x val="3"/><y val="0"/></restingoffset><restlength0 val="1"/><restingorientation0 val="0"/><restlength1 val="1"/><restingorientation1 val="90"/></attrs></module>
      <module type="MULTIPORT_OUT" name="Multi-Port-Out"/>
     </nodeslist><linkedlist>
      <link out="Multi-Port-In" in="Offset"/><link out="Offset" in="Curve"/><link out="Curve" in="Multi-Port-Out"/>
     </linkedlist></group>
     <group name="Deformation-HAND"><nodeslist>
      <module type="MULTIPORT_IN" name="Multi-Port-In"/>
      <module type="BendyBoneModule" name="Bone"><attrs><restoffset><x val="1"/><y val="2"/></restoffset><restorientation val="90"/><restlength val="2"/><restradius val="0.5"/></attrs></module>
      <module type="BendyBoneModule" name="Bone_1"><attrs><restoffset><x val="0"/><y val="0"/></restoffset><restorientation val="-90"/><restlength val="1"/><restradius val="0.25"/></attrs></module>
      <module type="MULTIPORT_OUT" name="Multi-Port-Out"/>
     </nodeslist><linkedlist>
      <link out="Multi-Port-In" in="Bone"/><link out="Bone" in="Bone_1"/><link out="Bone_1" in="Multi-Port-Out"/>
     </linkedlist></group>
     <module type="READ" name="ARM" pos="100,200,3"><attrs><offset><separate val="true"/><x val="0.5"/></offset><drawing><element col="ARM-COL"/></drawing></attrs></module>
     <module type="READ" name="HAND"><attrs><drawing><element col="HAND-COL"/></drawing></attrs></module>
     <module type="UNDERLAY" name="UL"/>
     <module type="CUTTER" name="Cutter"><attrs><inverted val="true"/></attrs></module>
     <module type="MasterController" name="mc_HAND"/>
     <module type="PEG" name="FOREARM-P" pos="300,150,4"/>
     <module type="KinematicOutputModule" name="KinematicOutput"/>
     <module type="PEG" name="FINGER-P"/>
     <module type="KinematicOutputModule" name="KinematicOutput_1"/>
     <module type="PEG" name="NAIL-P"/>
     <module type="COMPOSITE" name="Composite_HERO"><attrs><compositeMode val="compositeBitmap"/></attrs></module>
     <module type="MULTIPORT_OUT" name="Multi-Port-Out"/>
    </nodeslist><linkedlist>
     <link out="HERO-P" in="ARM-P"/>
     <link out="ARM-P" in="Deformation-ARM"/>
     <link out="Deformation-ARM" in="ARM"/>
     <link out="HERO-P" in="Deformation-HAND"/>
     <link out="Deformation-HAND" in="HAND"/>
     <link out="Deformation-ARM" in="FOREARM-P"/>
     <link out="Deformation-HAND" in="KinematicOutput"/>
     <link out="KinematicOutput" in="FINGER-P"/>
     <link out="FOREARM-P" in="KinematicOutput_1"/>
     <link out="KinematicOutput_1" in="NAIL-P"/>
     <link out="ARM" in="UL"/>
     <link out="HAND" in="Cutter" inport="0"/>
     <link out="UL" in="Cutter" inport="1"/>
     <link out="Cutter" in="Composite_HERO" inport="0"/>
     <link out="ARM" in="Composite_HERO" inport="1"/>
     <link out="Composite_HERO" in="Multi-Port-Out"/>
    </linkedlist>
    <backdrops>
     <backdrop topLeft="50,50" bottomRight="400,250"><title text="ARM"/></backdrop>
     <backdrop topLeft="250,100" bottomRight="350,200"><title text="FOREARM"/></backdrop>
    </backdrops></group>
   </nodeslist>
   <linkedlist>
    <link out="HERO" in="Composite"/>
    <link out="Composite" in="Display"/>
    <link out="Composite" in="Write"/>
   </linkedlist>
   <backdrops><backdrop topLeft="5000,5000" bottomRight="5100,5100"><title text="NOTHING"/></backdrop></backdrops>
  </rootgroup>
 </scene></scenes>
</project>"#;
    std::fs::create_dir_all(dir.join("palette-library")).unwrap();
    std::fs::write(
        dir.join("palette-library/HERO.plt"),
        "ToonBoomAnimationInc PaletteFile 2\nSolid    Skin   0x01   255 200 183 255\nTexture  Bumpy  0x02 Tile\nTexture  Grain  0x03 Tile\n",
    )
    .unwrap();
    // The picture of one of its textures, as a TGA named by its id; the other's is not there.
    std::fs::create_dir_all(dir.join("palette-library/HERO_textures")).unwrap();
    let grain = image::RgbaImage::from_fn(2, 1, |x, _| image::Rgba(if x == 0 { [200, 180, 150, 255] } else { [100, 80, 50, 255] }));
    grain.save_with_format(dir.join("palette-library/HERO_textures/03.tga"), image::ImageFormat::Tga).unwrap();
    let path = dir.join("hero.xstage");
    std::fs::write(&path, scene).unwrap();
    path
}

#[test]
fn a_rig_is_brought_from_a_scene_with_its_pegs_drawings_composites_and_palettes() {
    use iskra_model::{PegId, Source, Stacking};
    let dir = tempfile::tempdir().unwrap();
    let path = rig_scene(dir.path());
    let mut s = Session::new();
    let made = s.execute("rig.import", json!({ "path": path })).unwrap();
    let shot = s.project().shots[0].clone();
    let peg = |name: &str| shot.pegs.iter().find(|peg| peg.name == name).unwrap_or_else(|| panic!("a peg named {name}")).clone();
    let layer = |name: &str| shot.layers.iter().find(|layer| layer.name == name).unwrap_or_else(|| panic!("a layer named {name}")).clone();
    let composite = |name: &str| shot.composites.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("a composite named {name}")).clone();
    // A grid unit up is the shot's height over 24; across, 4/3 of that.
    let height = f64::from(s.project().settings.height);
    let (up, across) = (height / 24.0, height / 24.0 * 4.0 / 3.0);
    assert_eq!(made["units"], json!([across, up, across]));

    // Pegs in the tree the scene has, through the group and past the deformer, their values on
    // the first frame: y and the angle turned over, as the shot's y is down.
    let (hero, arm_p) = (peg("HERO-P"), peg("ARM-P"));
    assert_eq!(hero.parent, None);
    assert_eq!(arm_p.parent, Some(hero.id), "carried through the group, past the deformer");
    assert_eq!((hero.transform.position, hero.transform.rotation, hero.transform.depth), ([2.0 * across, up], -90.0, 0.5 * across));
    assert_eq!((arm_p.transform.position, arm_p.transform.scale), ([across, 0.0], [2.0, 2.0]));
    assert!(shot.pegs.iter().all(|peg| peg.animation.key_frames().is_empty()), "nothing keyed");

    // A drawing node is a layer with every drawing of its element, empty, the one it shows on
    // the first frame exposed; its own offset is a peg of its own over it.
    let arm = layer("ARM");
    let names: Vec<&str> = arm.drawings.iter().map(|drawing| drawing.name.as_str()).collect();
    assert_eq!(names, ["1", "bent"]);
    assert_eq!(arm.exposure.at(0), Some(arm.drawings[1].id));
    let own = peg("ARM-Drawing");
    assert_eq!((arm.parent, own.parent, own.transform.position), (Some(own.id), Some(arm_p.id), [0.5 * across, 0.0]));
    assert_eq!(layer("HAND").parent, Some(hero.id));
    // Its underlay art is a layer of its own beside it, with the same drawings.
    let under = layer("ARM Underlay");
    assert_eq!((under.parent, under.drawings.len()), (arm.parent, 2));

    // The cutter cuts the hand by the arm's underlay, inverted as it was; the group's composite
    // stacks by its ports, the rightmost on top: the arm over the cutter; what the display
    // shows is on top of the shot.
    let cutter = shot.effects.iter().find(|effect| effect.name == "Cutter").expect("the cutter").clone();
    assert_eq!(cutter.kind, "cutter");
    assert_eq!((cutter.inputs.get("picture"), cutter.inputs.get("matte")), (Some(&Source::Layer(layer("HAND").id)), Some(&Source::Layer(under.id))));
    assert_eq!(cutter.params.get("inverted").map(|param| param.value.clone()), Some(iskra_model::Value::Switch(true)));
    let group = composite("Composite_HERO");
    assert_eq!((group.inputs.clone(), group.stacking), (vec![Source::Layer(arm.id), Source::Effect(cutter.id)], Stacking::Ports));
    // The scene's top composite is named apart from the shot's own, which has its name.
    let top = shot.composites.iter().find(|c| c.inputs == [Source::Composite(group.id)]).expect("the scene's top composite").clone();
    assert_eq!((top.name.as_str(), top.stacking), ("Composite 2", Stacking::Depth));
    let main = shot.main_composite().unwrap();
    assert_eq!(shot.composite(main).unwrap().inputs.first(), Some(&Source::Composite(top.id)));

    // The chains of deformers are deformers of the layers they bend, at rest, in the shot's
    // units: the arm's a curve with its handles, out along it and into its end from below;
    // the hand's two bones, up and then turned back across. Its underlay's layer is bent too.
    let deformer = |name: &str| shot.deformers.iter().find(|deformer| deformer.name == name).unwrap_or_else(|| panic!("a deformer named {name}")).clone();
    let curve = deformer("Deformation-ARM");
    assert_eq!((arm.deformer, under.deformer), (Some(curve.id), Some(curve.id)));
    let at: Vec<[f64; 2]> = curve.spine.iter().map(|knot| knot.at).collect();
    assert_eq!(at, [[0.0, 0.0], [3.0 * across, 0.0]]);
    let bends: Vec<iskra_model::Bend> = curve.spine.iter().map(|knot| knot.bend.unwrap()).collect();
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
    assert!(near(bends[0].out, [across, 0.0]) && near(bends[1].into, [0.0, up]), "{bends:?}");
    let bones = deformer("Deformation-HAND");
    assert_eq!(layer("HAND").deformer, Some(bones.id));
    assert!(near(bones.origin, [across, -2.0 * up]), "{:?}", bones.origin);
    let laid: Vec<(f64, f64)> = bones.bones.iter().map(|bone| (bone.rest_angle.round(), (bone.length * 1e6).round() / 1e6)).collect();
    assert_eq!(laid, [(-90.0, 2.0 * up), (90.0, across)], "up the screen, then turned back to the right");
    assert_eq!((bones.bones[1].round, bones.bones[1].fade), (0.25 * across, 0.5 * across));
    assert_eq!(made["deformers"].as_array().map(Vec::len), Some(2));
    // A peg wired from a chain follows its end, bending the drawings right under it; one wired
    // through a kinematic output follows it and only carries them; one after a following peg
    // through a kinematic output is under that peg, which carries it by the chain's end.
    let follow = |name: &str| peg(name).follows.map(|follow| (follow.deformer, follow.point, follow.bends));
    assert_eq!((follow("FOREARM-P"), peg("FOREARM-P").parent), (Some((curve.id, None, true)), Some(peg("ARM-P").id)));
    assert_eq!((follow("FINGER-P"), peg("FINGER-P").parent), (Some((bones.id, None, false)), Some(peg("HERO-P").id)));
    assert_eq!((follow("NAIL-P"), peg("NAIL-P").parent), (None, Some(peg("FOREARM-P").id)));

    // A backdrop is a group of what was made of the nodes inside it, a chain's deformer by the
    // group of the chain; one inside it is a group in its group. One round nothing is none.
    let group = |name: &str| shot.groups.iter().find(|group| group.name == name).unwrap_or_else(|| panic!("a group named {name}")).clone();
    let (arm_group, forearm) = (group("ARM 2"), group("FOREARM"));
    let ids = |of: &[u64]| of.iter().copied().collect::<std::collections::BTreeSet<u64>>();
    assert_eq!(arm_group.members, ids(&[peg("ARM-P").id.0, peg("ARM-Drawing").id.0, arm.id.0, curve.id.0, forearm.id.0]));
    assert_eq!((forearm.members.clone(), shot.groups.len(), made["groups"].as_array().map(Vec::len)), (ids(&[peg("FOREARM-P").id.0]), 2, Some(2)));

    // Its palette is a named palette of the project; what wasn't brought is in the report.
    assert_eq!(made["palettes"], json!(["HERO"]));
    let palette = &s.project().palette;
    let set = palette.sets.iter().find(|set| set.name == "HERO").unwrap().id;
    let read: Vec<(&str, [u8; 4])> = palette.swatches_of(set).map(|swatch| (swatch.name.as_str(), swatch.color)).collect();
    assert_eq!(read, [("Skin", [255, 200, 183, 255]), ("Grain", [150, 130, 100, 255])], "a texture's colour is its picture's");
    // The texture paints with its picture, brought in as media and read.
    let grain = palette.swatches_of(set).find(|swatch| swatch.name == "Grain").unwrap();
    let iskra_model::SwatchKind::Texture(texture) = &grain.kind else { panic!("{:?}", grain.kind) };
    assert!(texture.tile && s.project().media(texture.media).is_some() && palette.pictures.contains_key(&texture.media));
    let report: Vec<&str> = made["report"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(report[0], "HERO/mc_HAND: not brought, a node of a kind this import doesn't know (MasterController)");
    assert!(report[1].starts_with("Palette HERO: Bumpy (Texture, its picture ") && report[1].ends_with("02.tga is not there) left out"), "{report:?}");
    assert_eq!(report.len(), 2);
    // One step to undo.
    s.undo();
    assert!(s.project().shots[0].pegs.iter().all(|peg| peg.id != PegId(made["pegs"][0].as_u64().unwrap())));
    assert!(turned_down(s.execute("rig.import", json!({ "path": dir.path().join("gone.xstage") })), "could not be read"));
}

#[test]
fn a_drawing_nodes_matte_is_a_cutter_after_its_layer() {
    use iskra_model::Source;
    let scene = r#"<?xml version="1.0" encoding="UTF-8"?>
<project>
 <elements>
  <element id="2" elementName="EYE" elementFolder="EYE" rootFolder="elements"><drawings><dwg name="1"/></drawings></element>
  <element id="3" elementName="PUPIL" elementFolder="PUPIL" rootFolder="elements"><drawings><dwg name="1"/></drawings></element>
  <element id="4" elementName="HAIR" elementFolder="HAIR" rootFolder="elements"><drawings><dwg name="1"/></drawings></element>
 </elements>
 <options>
  <metrics unitAspectRatioX="4" unitAspectRatioY="3" numberOfUnitsX="24" numberOfUnitsY="24" numberOfUnitsZ="12"/>
  <resolution size="1920,1080"/>
 </options>
 <scenes><scene name="Top" startFrame="1">
  <columns>
   <column type="0" name="EYE-COL"><elementSeq exposures="1-10" val="1" id="2"/></column>
   <column type="0" name="PUPIL-COL"><elementSeq exposures="1-10" val="1" id="3"/></column>
   <column type="0" name="HAIR-COL"><elementSeq exposures="1-10" val="1" id="4"/></column>
  </columns>
  <rootgroup name="Top">
   <nodeslist>
    <module type="PEG" name="PUPIL-P"/>
    <module type="READ" name="EYE"><attrs><drawing><element col="EYE-COL"/></drawing></attrs></module>
    <module type="READ" name="PUPIL"><attrs><drawing><element col="PUPIL-COL"/></drawing></attrs></module>
    <module type="READ" name="HAIR"><attrs><drawing><element col="HAIR-COL"/></drawing></attrs></module>
    <module type="MATTE_RESIZE" name="Matte-Resize"><attrs><radius val="0.5"/></attrs></module>
    <module type="COMPOSITE" name="Composite"><attrs><compositeMode val="compositeBitmap"/></attrs></module>
    <module type="DISPLAY" name="Display"/>
   </nodeslist>
   <linkedlist>
    <link out="PUPIL-P" in="PUPIL" inport="0"/>
    <link out="EYE" in="PUPIL" inport="1"/>
    <link out="EYE" in="Matte-Resize"/>
    <link out="Matte-Resize" in="HAIR" inport="1"/>
    <link out="PUPIL" in="Composite" inport="0"/>
    <link out="HAIR" in="Composite" inport="1"/>
    <link out="EYE" in="Composite" inport="2"/>
    <link out="Composite" in="Display"/>
   </linkedlist>
  </rootgroup>
 </scene></scenes>
</project>"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("eye.xstage");
    std::fs::write(&path, scene).unwrap();
    let mut s = Session::new();
    let made = s.execute("rig.import", json!({ "path": path })).unwrap();
    let shot = s.project().shots[0].clone();
    let layer = |name: &str| shot.layers.iter().find(|layer| layer.name == name).unwrap_or_else(|| panic!("a layer named {name}")).id;
    let cutter = |name: &str| shot.effects.iter().find(|effect| effect.name == name).unwrap_or_else(|| panic!("a cutter named {name}")).clone();

    // The pupil's layer, still on its peg, cut by the eye, keeping what is under it; the
    // composite takes the cutter where it took the pupil, the rightmost port on top. The
    // hair's matte goes through the matte resize, which grows it by its radius.
    let pupil = cutter("PUPIL-Matte");
    assert_eq!(pupil.kind, "cutter");
    assert_eq!((pupil.inputs.get("picture"), pupil.inputs.get("matte")), (Some(&Source::Layer(layer("PUPIL"))), Some(&Source::Layer(layer("EYE")))));
    assert_eq!(pupil.params.get("inverted").map(|param| param.value.clone()), Some(iskra_model::Value::Switch(true)));
    let pupil_peg = shot.pegs.iter().find(|peg| peg.name == "PUPIL-P").unwrap().id;
    assert_eq!(shot.layers.iter().find(|l| l.id == layer("PUPIL")).unwrap().parent, Some(pupil_peg));
    let hair = cutter("HAIR-Matte");
    assert_eq!(hair.inputs.get("matte"), Some(&Source::Layer(layer("EYE"))));
    let grow = |effect: &iskra_model::Effect| effect.params.get("grow").map(|param| param.value.clone());
    assert_eq!(grow(&hair), Some(iskra_model::Value::Number(0.5 * 2.0 / 0.6)));
    assert_eq!(grow(&pupil), None);
    let composite = shot.composites.iter().find(|c| c.name == "Composite 2").expect("the scene's composite");
    assert_eq!(composite.inputs, [Source::Layer(layer("EYE")), Source::Effect(hair.id), Source::Effect(pupil.id)]);
    let report: Vec<&str> = made["report"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(report, ["Matte-Resize: a matte resize, its radius (0.5) brought as 1.7 px of Alpha Size on the cutters it feeds, a guess"]);
}

/// Brings a real scene into a shot, given by its path in `ISKRA_RIG_SCENE`, and says what was
/// made and the report: `ISKRA_RIG_SCENE=/path/to/scene.xstage cargo test -p iskra-engine
/// a_real_rig -- --ignored --nocapture`.
#[test]
#[ignore = "needs a scene of the user's, by its path in ISKRA_RIG_SCENE"]
fn a_real_rig_is_brought_whole() {
    let path = std::env::var("ISKRA_RIG_SCENE").expect("ISKRA_RIG_SCENE names a scene file");
    let mut s = Session::new();
    let made = s.execute("rig.import", json!({ "path": path })).unwrap();
    let count = |what: &str| made[what].as_array().map_or(0, Vec::len);
    println!(
        "{} pegs, {} layers, {} composites, {} cutters, palettes {}, a unit {}",
        count("pegs"),
        count("layers"),
        count("composites"),
        count("effects"),
        made["palettes"],
        made["units"]
    );
    for line in made["report"].as_array().unwrap() {
        println!("  {}", line.as_str().unwrap());
    }
    let shot = &s.project().shots[0];
    let unparented = shot.layers.iter().filter(|layer| layer.parent.is_none()).map(|layer| layer.name.as_str()).collect::<Vec<_>>();
    println!("layers on no peg: {unparented:?}");
    for deformer in &shot.deformers {
        let bent: Vec<&str> = shot.layers.iter().filter(|layer| layer.deformer == Some(deformer.id)).map(|layer| layer.name.as_str()).collect();
        let (kind, count) = if deformer.is_curve() { ("curve", deformer.spine.len()) } else { ("bones", deformer.bones.len()) };
        let following: Vec<String> = shot
            .pegs
            .iter()
            .filter_map(|peg| peg.follows.filter(|follow| follow.deformer == deformer.id).map(|follow| (peg, follow)))
            .map(|(peg, follow)| format!("{}{}", peg.name, if follow.bends { " (bending)" } else { "" }))
            .collect();
        println!("deformer {}: {kind} of {count}, bends {bent:?}, followed by {following:?}", deformer.name);
    }
    let names: std::collections::HashMap<u64, &str> = shot.node_ids().zip(shot.node_names()).collect();
    let name_of = |id: u64| names.get(&id).map_or_else(|| format!("#{id}"), |name| name.to_string());
    for group in &shot.groups {
        let held: Vec<String> = group.members.iter().map(|id| name_of(*id)).collect();
        println!("group {}: {held:?}", group.name);
    }
    use iskra_model::Source;
    let source = |source: &Source| match source {
        Source::Layer(layer) => name_of(layer.0),
        Source::Art(layer, kind) => format!("{} {kind:?}", name_of(layer.0)),
        Source::Composite(composite) => name_of(composite.0),
        Source::Effect(effect) => name_of(effect.0),
    };
    for composite in &shot.composites {
        let inputs: Vec<String> = composite.inputs.iter().map(source).collect();
        println!("composite {} ({:?}): {inputs:?}", composite.name, composite.stacking);
    }
    for effect in &shot.effects {
        let inputs: Vec<String> = effect.inputs.iter().map(|(port, input)| format!("{port}: {}", source(input))).collect();
        println!("effect {}: {inputs:?}", effect.name);
    }
    // The layers as they are drawn, the farthest first, with how near each is.
    for placed in &iskra_eval::evaluate(shot, 0).layers {
        println!("depth {:>8.3}: {}", placed.depth, name_of(placed.layer.0));
    }
}

#[test]
fn a_rigs_drawings_are_the_pictures_the_converter_made_of_them() {
    use iskra_drawing::{ArtContent, ArtLayerKind};
    let dir = tempfile::tempdir().unwrap();
    let path = rig_scene(dir.path());
    let scene = iskra_io::RigScene::read(&path).unwrap();
    // Pictures as the converter makes them: 9 by 5, the drawing's origin in the middle. The
    // arm's line art a pixel near the skin's colour in the middle, its colour art half covered
    // at the left; its underlay a pixel at the right.
    let pictures = dir.path().join("pictures");
    let arm = scene.elements.iter().find(|element| element.name == "ARM").unwrap();
    let write = |pass: iskra_io::RigPass, at: usize, colour: [u8; 4]| {
        let mut pixels = vec![[0u8; 4]; 45];
        pixels[at] = colour;
        let file = iskra_io::picture_path(&pictures, arm, "bent", pass);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, iskra_io::encode_png(9, 5, &pixels).unwrap()).unwrap();
    };
    write(iskra_io::RigPass::Line, 22, [250, 205, 180, 255]);
    write(iskra_io::RigPass::Colour, 18, [255, 200, 183, 128]);
    write(iskra_io::RigPass::Under, 26, [255, 200, 183, 255]);
    write(iskra_io::RigPass::Over, 13, [255, 200, 183, 255]);
    let mut s = Session::new();
    let made = s.execute("rig.import", json!({ "path": path, "pictures": pictures })).unwrap();
    let shot = s.project().shots[0].clone();
    let layer = |name: &str| shot.layers.iter().find(|layer| layer.name == name).unwrap().clone();
    let skin = s.project().palette.swatches.iter().find(|swatch| swatch.name == "Skin").unwrap().id;
    let shown = |name: &str| layer(name).drawing_at(0).unwrap().drawing.clone();
    let pixel = |drawing: &iskra_drawing::Drawing, kind, x, y| match &drawing.layer(kind).unwrap().content {
        ArtContent::Indexed(surface) => surface.pixel(x, y),
        _ => panic!("indexed art"),
    };
    let arm_drawing = shown("ARM");
    let line = pixel(&arm_drawing, ArtLayerKind::LineArt, 0, 0);
    assert_eq!((line.swatch, line.coverage), (skin, 65535), "the nearest swatch of the rig's palette, at the origin");
    let colour = pixel(&arm_drawing, ArtLayerKind::ColourArt, -4, 0);
    assert_eq!((colour.swatch, colour.coverage), (skin, 128 * 257), "half covered as it was");
    // The overlay art is the overart, though no Overlay node takes it out.
    assert_eq!(pixel(&arm_drawing, ArtLayerKind::Overart, 0, -1).swatch, skin);
    // The underlay is the line art of the layer that takes it out.
    let under = shown("ARM Underlay");
    assert_eq!(pixel(&under, ArtLayerKind::LineArt, 4, 0).swatch, skin);
    assert_eq!(pixel(&under, ArtLayerKind::LineArt, 0, 0).coverage, 0, "not the arm's line art");
    // The drawings with no pictures are empty, and said so.
    assert!(
        matches!(&layer("ARM").drawings[0].drawing.layer(ArtLayerKind::LineArt).unwrap().content, ArtContent::Indexed(surface) if surface.pixel(0, 0).coverage == 0)
    );
    let report = made["report"].to_string();
    assert!(report.contains("5 pictures of drawings were not made"), "not the overlay art the other drawings have none of: {report}");
    // And of each art layer, how many pictures had art: to tell drawings without it from a
    // converter that did not make it.
    assert!(report.contains("Overlay art: 1 pictures with art, 0 empty, 2 not made, of 3"), "{report}");
    assert!(report.contains("Underlay art: 1 pictures with art, 0 empty, 1 not made, of 2"), "{report}");
}

#[test]
fn a_rigs_line_and_colour_art_nodes_are_outputs_of_the_drawing_nodes_layer() {
    use iskra_drawing::ArtLayerKind;
    use iskra_model::Source;
    for (node, title, output) in [("LINE_ART", "Line Art", ArtLayerKind::LineArt), ("COLOR_ART", "Colour Art", ArtLayerKind::ColourArt)] {
        let dir = tempfile::tempdir().unwrap();
        let path = rig_scene(dir.path());
        // The arm's art taken out by the node, which is the cutter's matte.
        let scene = std::fs::read_to_string(&path).unwrap().replace(r#"type="UNDERLAY" name="UL""#, &format!(r#"type="{node}" name="UL""#));
        std::fs::write(&path, scene).unwrap();
        let mut s = Session::new();
        s.execute("rig.import", json!({ "path": path })).unwrap();
        let shot = s.project().shots[0].clone();
        let arm = shot.layers.iter().find(|layer| layer.name == "ARM").expect("the arm's layer");
        assert!(shot.layers.iter().all(|layer| layer.name != format!("ARM {title}")), "no layer of its own for {node}");
        let cutter = shot.effects.iter().find(|effect| effect.name == "Cutter").expect("the cutter");
        assert_eq!(cutter.inputs.get("matte"), Some(&Source::Art(arm.id, output)), "{node}");
    }
}

#[test]
fn a_rigs_overlay_art_is_the_overart_of_its_drawings_and_its_node_their_overart_output() {
    use iskra_drawing::{ArtContent, ArtLayerKind};
    use iskra_model::Source;
    let dir = tempfile::tempdir().unwrap();
    let path = rig_scene(dir.path());
    // The arm's art taken out by an Overlay node, which is the cutter's matte.
    let scene = std::fs::read_to_string(&path).unwrap().replace(r#"type="UNDERLAY" name="UL""#, r#"type="OVERLAY" name="UL""#);
    std::fs::write(&path, scene).unwrap();
    let rig = iskra_io::RigScene::read(&path).unwrap();
    let arm = rig.elements.iter().find(|element| element.name == "ARM").unwrap();
    let pictures = dir.path().join("pictures");
    let mut pixels = vec![[0u8; 4]; 45];
    pixels[26] = [255, 200, 183, 255];
    let file = iskra_io::picture_path(&pictures, arm, "bent", iskra_io::RigPass::Over);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, iskra_io::encode_png(9, 5, &pixels).unwrap()).unwrap();
    let mut s = Session::new();
    s.execute("rig.import", json!({ "path": path, "pictures": pictures })).unwrap();
    let shot = s.project().shots[0].clone();
    let layer = |name: &str| shot.layers.iter().find(|layer| layer.name == name).cloned();
    let arm = layer("ARM").expect("the arm's layer");
    assert!(layer("ARM Overlay").is_none(), "no layer of its own");
    // The overlay picture is the overart of the arm's drawing.
    let drawing = arm.drawing_at(0).unwrap().drawing.clone();
    let ArtContent::Indexed(over) = &drawing.layer(ArtLayerKind::Overart).unwrap().content else { panic!("indexed overart") };
    assert_eq!(over.pixel(4, 0).coverage, 65535);
    // The cutter's matte is the arm's overart output.
    let cutter = shot.effects.iter().find(|effect| effect.name == "Cutter").expect("the cutter");
    assert_eq!(cutter.inputs.get("matte"), Some(&Source::Art(arm.id, ArtLayerKind::Overart)));
}

#[test]
fn a_text_layer_holds_texts_laid_out_as_vector_fills() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId};
    let mut s = Session::new();
    let drawing_layer = layer(&s);
    let text = id(s.execute("layer.new", json!({ "kind": "text" })).unwrap());
    {
        let made = s.project().layer(LayerId(text)).unwrap();
        assert!(made.text && made.is_vector() && made.name == "Text", "{}", made.name);
    }
    // Only a text layer takes a text.
    let at = |layer: u64, frame: u32| json!({ "layer": layer, "frame": frame, "at": [100, 50], "text": "Hello" });
    assert!(matches!(s.execute("text.add", at(drawing_layer, 0)), Err(EngineError::Invalid(_))));
    let added = s.execute("text.add", at(text, 0)).unwrap();
    assert_eq!((added["box"].as_u64(), &added["standIn"], s.history().undo_label()), (Some(0), &json!([]), Some("New Text")));
    let drawing = added["drawing"].as_u64().unwrap();
    assert_eq!(cells(&s, text, 1), "1", "a text drawing made in the empty cell, as a stroke makes one");
    let glyphs = |s: &Session| {
        let entry = &s.project().layer(LayerId(text)).unwrap().drawings[0].drawing;
        let ArtContent::Vector(art) = &entry.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector art") };
        (art.shapes().len(), entry.texts.iter().map(|t| t.text.clone()).collect::<Vec<_>>())
    };
    assert_eq!(glyphs(&s), (5, vec!["Hello".to_string()]));

    // Typing is a splice of the box's letters; each lays the drawing out again.
    let typed = |from: u64, to: u64, insert: &str| json!({ "layer": text, "drawing": drawing, "box": 0, "from": from, "to": to, "insert": insert });
    s.execute("text.type", typed(5, 5, " you")).unwrap();
    s.execute("text.type", typed(0, 1, "J")).unwrap();
    assert_eq!(glyphs(&s), (8, vec!["Jello you".to_string()]), "the space has no glyph");
    // Typing on is one step: undone, it all goes, back to the box as it was added.
    s.undo();
    assert_eq!(glyphs(&s).1, vec!["Hello".to_string()]);
    s.redo();
    assert_eq!(glyphs(&s).1, vec!["Jello you".to_string()]);
    // A new step is asked for, or something else was done between: two steps.
    s.execute("text.type", typed(9, 9, "!")).unwrap();
    s.execute("text.type", json!({ "layer": text, "drawing": drawing, "box": 0, "from": 10, "to": 10, "insert": "!", "newStep": true })).unwrap();
    s.undo();
    assert_eq!(glyphs(&s).1, vec!["Jello you!".to_string()]);
    s.undo();
    assert_eq!(glyphs(&s).1, vec!["Jello you".to_string()]);
    assert!(matches!(s.execute("text.type", typed(3, 2, "")), Err(EngineError::Invalid(_))));
    // A second box in the same drawing.
    s.execute("text.add", json!({ "layer": text, "frame": 0, "at": [100, 200], "text": "Hi" })).unwrap();
    assert_eq!(glyphs(&s).0, 10);
    s.undo();
    assert_eq!(glyphs(&s).0, 8);

    // The format: only what is given changes; a swatch the palette lacks is refused.
    let format = |format: Value| json!({ "layer": text, "drawing": drawing, "box": 0, "format": format });
    s.execute("text.setFormat", format(json!({ "size": 96, "swatch": 3 }))).unwrap();
    let shown = s.project().layer(LayerId(text)).unwrap().drawings[0].drawing.clone();
    assert_eq!((shown.texts[0].format.size, shown.texts[0].format.swatch, shown.texts[0].format.font.as_str()), (96.0, SwatchId(3), "Inter"));
    assert!(shown.swatches().contains(&SwatchId(3)));
    assert!(matches!(s.execute("text.setFormat", format(json!({ "swatch": 999 }))), Err(EngineError::Invalid(_))));
    assert!(matches!(s.execute("text.setFormat", format(json!({ "size": -1 }))), Err(EngineError::Invalid(_))));
    // A font the machine lacks is laid out in Inter, keeps its name, and is said.
    let missing = s.execute("text.setFormat", format(json!({ "font": "Nowhere Sans Pro" }))).unwrap();
    assert_eq!(missing["standIn"], json!(["Nowhere Sans Pro"]));
    assert_eq!(glyphs(&s).0, 8);

    // A fixed width wraps it; kerning sits before a letter; a box moves.
    let boxed = |more: Value| {
        let mut params = json!({ "layer": text, "drawing": drawing, "box": 0 });
        params.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
        params
    };
    s.execute("text.setBox", boxed(json!({ "width": 200 }))).unwrap();
    assert_eq!(s.project().layer(LayerId(text)).unwrap().drawings[0].drawing.texts[0].width, Some(200.0));
    s.execute("text.setBox", boxed(json!({ "at": [0, 0] }))).unwrap();
    assert_eq!(s.project().layer(LayerId(text)).unwrap().drawings[0].drawing.texts[0].width, Some(200.0), "a width not given is kept");
    s.execute("text.setBox", boxed(json!({ "width": null }))).unwrap();
    assert_eq!(s.project().layer(LayerId(text)).unwrap().drawings[0].drawing.texts[0].width, None);
    s.execute("text.setKerning", boxed(json!({ "at": 2, "kerning": 50 }))).unwrap();
    assert!(matches!(s.execute("text.setKerning", boxed(json!({ "at": 99, "kerning": 50 }))), Err(EngineError::Invalid(_))));

    // Saved and opened again: the boxes and their glyphs.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Titles.iskra");
    s.execute("file.saveAs", json!({ "path": path })).unwrap();
    let before = glyphs(&s);
    let kept = s.project().layer(LayerId(text)).unwrap().drawings[0].drawing.texts.clone();
    let mut opened = Session::new();
    opened.execute("file.open", json!({ "path": path })).unwrap();
    let shot = opened.project().shots[0].id;
    opened.execute("shot.load", json!({ "shot": shot.0 })).ok();
    assert!(opened.project().layer(LayerId(text)).unwrap().text);
    assert_eq!(opened.project().layer(LayerId(text)).unwrap().drawings[0].drawing.texts, kept);
    assert_eq!(glyphs(&opened), before);

    // The last box deleted leaves the drawing empty.
    s.execute("text.delete", boxed(json!({}))).unwrap();
    assert_eq!(glyphs(&s), (0, vec![]));
}

#[test]
fn a_text_has_a_background_and_is_turned_into_shapes_it_keeps() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId};
    let mut s = Session::new();
    let text = id(s.execute("layer.new", json!({ "kind": "text" })).unwrap());
    let added = s.execute("text.add", json!({ "layer": text, "frame": 0, "at": [0, 0], "text": "Ab" })).unwrap();
    let drawing = added["drawing"].as_u64().unwrap();
    s.execute("text.add", json!({ "layer": text, "frame": 0, "at": [0, 300], "text": "Cd" })).unwrap();
    let shapes = |s: &Session, kind: ArtLayerKind| {
        let entry = &s.project().layer(LayerId(text)).unwrap().drawings[0].drawing;
        let ArtContent::Vector(art) = &entry.layer(kind).unwrap().content else { panic!("vector art") };
        let own = art.shapes().iter().filter(|shape| shape.id.0 < iskra_drawing::TEXT_SHAPES).count();
        (own, art.shapes().len() - own, entry.texts.len())
    };
    assert_eq!(shapes(&s, ArtLayerKind::LineArt), (0, 4, 2), "the glyphs are the text's");
    // A background in a swatch of its own, behind the first box, on the colour art.
    let format = json!({ "layer": text, "drawing": drawing, "box": 0, "format": { "background": 2, "padding": 50 } });
    s.execute("text.setFormat", format).unwrap();
    assert_eq!(shapes(&s, ArtLayerKind::ColourArt), (0, 1, 2));
    let background = s.project().layer(LayerId(text)).unwrap().drawings[0].drawing.layer(ArtLayerKind::ColourArt).unwrap().content.bounds();
    assert!(background.x0 <= -23 && background.y0 <= -23, "half the size past the letters: {background:?}");

    // The first box made shapes: kept as the drawing's own, and no longer text.
    s.execute("text.toShapes", json!({ "layer": text, "drawing": drawing, "box": 0 })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Convert Text to Shapes"));
    assert_eq!(shapes(&s, ArtLayerKind::LineArt), (2, 2, 1));
    assert_eq!(shapes(&s, ArtLayerKind::ColourArt), (1, 0, 1));
    // Typing in the other box lays it out again and leaves them be.
    s.execute("text.type", json!({ "layer": text, "drawing": drawing, "box": 0, "from": 2, "to": 2, "insert": "e" })).unwrap();
    assert_eq!(shapes(&s, ArtLayerKind::LineArt), (2, 3, 1));
    // And all that is left, the same.
    s.execute("text.toShapes", json!({ "layer": text, "drawing": drawing })).unwrap();
    assert_eq!(shapes(&s, ArtLayerKind::LineArt), (5, 0, 0));
    assert!(matches!(s.execute("text.toShapes", json!({ "layer": text, "drawing": drawing })), Err(EngineError::Invalid(_))));
    s.undo();
    assert_eq!(shapes(&s, ArtLayerKind::LineArt), (2, 3, 1));
}

/// A session whose layer shows on frame 0 a drawing with two squares of swatch 3 on its
/// colour art, apart, at 0..10 and 20..30 across and 0..10 down, saved in `dir`.
fn two_squares(dir: &std::path::Path) -> (Session, u64) {
    let mut s = Session::new();
    let e = swatch_layer(&mut s);
    s.execute("file.saveAs", json!({ "path": dir.join("squares.iskra") })).unwrap();
    s.edit("Squares", |p| {
        let id = iskra_model::DrawingId(p.new_id());
        let mut drawing = iskra_model::Drawing::default();
        if let iskra_model::ArtContent::Indexed(surface) = &mut drawing.art_layers[0].content {
            for x0 in [0, 20] {
                surface.paint(iskra_model::Rect::new(x0, 0, x0 + 10, 10), &[1.0; 100], SwatchId(3));
            }
        }
        let layer = p.layer_mut(iskra_model::LayerId(e)).unwrap();
        layer.drawings.push(iskra_model::PoolDrawing { id, name: "1".to_string(), drawing: std::sync::Arc::new(drawing) });
        layer.exposure.set(0, 1, Some(id));
        Ok(())
    })
    .unwrap();
    (s, e)
}

/// The colours of the pixels (0, 0) to (29, 0) of the drawing on frame 0.
fn first_row(s: &Session, layer: u64) -> Vec<[u8; 4]> {
    let drawing = &s.project().layer(iskra_model::LayerId(layer)).unwrap().drawing_at(0).unwrap().drawing;
    s.project().palette.paint(drawing, iskra_model::Rect::new(0, 0, 30, 1), iskra_model::WorkingSpace::Srgb)
}

#[test]
fn a_placement_is_skewed_and_a_radial_gradients_focus_moved() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, e) = two_squares(dir.path());
    let ring = json!({ "kind": "gradient", "shape": "radial", "stops": [{ "at": 0.0, "color": [255, 0, 0, 255] }, { "at": 1.0, "color": [0, 0, 255, 255] }] });
    s.execute("palette.set", json!({ "swatch": 3, "kind": ring })).unwrap();
    let place = json!({ "layer": e, "frame": 0, "x": 25, "y": 5, "start": [25.0, 5.0], "end": [30.0, 5.0], "across": [27.0, 10.0], "focus": [3.0, 0.0] });
    s.execute("drawing.place", place).unwrap();
    let placed = |s: &Session| s.project().layer(iskra_model::LayerId(e)).unwrap().drawing_at(0).unwrap().drawing.placements.clone();
    // Its focus is kept inside its ring.
    assert_eq!((placed(&s)[0].across, placed(&s)[0].focus), (Some([27.0, 10.0]), [0.95, 0.0]));
    assert!(s.execute("drawing.place", json!({ "layer": e, "frame": 0, "x": 25, "y": 5, "start": [0.0, 0.0], "end": [1.0, 0.0], "across": "up" })).is_err());
    // Saved and opened again, it is there as it was.
    s.execute("file.save", Value::Null).unwrap();
    let mut s = Session::new();
    s.execute("file.open", json!({ "path": dir.path().join("squares.iskra") })).unwrap();
    let shot = s.project().shots[0].id.0;
    s.execute("shot.load", json!({ "shot": shot })).unwrap();
    assert_eq!((placed(&s)[0].across, placed(&s)[0].focus), (Some([27.0, 10.0]), [0.95, 0.0]));
}

#[test]
fn a_swatch_made_a_gradient_paints_each_of_its_areas_across_itself() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, e) = two_squares(dir.path());
    let ramp = json!({ "kind": "gradient", "stops": [{ "at": 1.0, "color": [0, 0, 255, 255] }, { "at": 0.0, "color": [255, 0, 0, 255] }] });
    s.execute("palette.set", json!({ "swatch": 3, "kind": ramp })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Change Swatch"));
    // Its stops are put in order, and its colour is its first.
    assert_eq!(s.project().palette.color(SwatchId(3)), [255, 0, 0, 255]);
    let row = first_row(&s, e);
    assert_eq!((row[0], row[9], row[20], row[29]), ([242, 0, 13, 255], [13, 0, 242, 255], [242, 0, 13, 255], [13, 0, 242, 255]));

    // The right square's turned round; the left's stays fitted.
    s.execute("drawing.place", json!({ "layer": e, "frame": 0, "x": 25, "y": 5, "start": [30.0, 0.0], "end": [20.0, 0.0] })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Place Gradient"));
    let row = first_row(&s, e);
    assert_eq!((row[0], row[20]), ([242, 0, 13, 255], [13, 0, 242, 255]));
    assert!(turned_down(
        s.execute("drawing.place", json!({ "layer": e, "frame": 0, "x": 15, "y": 5, "start": [0.0, 0.0], "end": [1.0, 0.0] })),
        "no area of a gradient"
    ));
    assert!(turned_down(s.execute("drawing.place", json!({ "layer": e, "frame": 0, "x": 25, "y": 5, "start": [0.0, 0.0] })), "`start` and an `end`"));

    // Saved and opened again, the placement is there.
    s.execute("file.save", Value::Null).unwrap();
    let mut s = Session::new();
    s.execute("file.open", json!({ "path": dir.path().join("squares.iskra") })).unwrap();
    let shot = s.project().shots[0].id.0;
    s.execute("shot.load", json!({ "shot": shot })).unwrap();
    assert_eq!(first_row(&s, e)[20], [13, 0, 242, 255]);
    // Reset, it is fitted again.
    s.execute("drawing.place", json!({ "layer": e, "frame": 0, "x": 22, "y": 2, "reset": true })).unwrap();
    assert_eq!(first_row(&s, e)[20], [242, 0, 13, 255]);

    // Copied to the layer's other drawings, a placement turns their area too.
    s.execute("drawing.place", json!({ "layer": e, "frame": 0, "x": 25, "y": 5, "start": [30.0, 0.0], "end": [20.0, 0.0] })).unwrap();
    s.execute("drawing.duplicate", json!({ "layer": e, "frame": 0 })).unwrap();
    s.execute("drawing.place", json!({ "layer": e, "frame": 0, "x": 25, "y": 5, "reset": true })).unwrap();
    s.execute("drawing.copyPlacements", json!({ "layer": e, "frame": 0 })).unwrap();
    let pool = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawings;
    assert!(pool.len() == 2 && pool.iter().all(|entry| entry.drawing.placements.is_empty()), "the reset one's none, copied");
    assert_eq!(s.history().undo_label(), Some("Copy Gradient Placements"));
    s.execute("edit.undo", Value::Null).unwrap();
    let pool = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawings;
    assert_eq!(pool.iter().map(|entry| entry.drawing.placements.len()).collect::<Vec<_>>(), [1, 0], "the first kept its own");

    // A gradient needs two colours; made solid, the swatch is its colour again.
    let one = json!({ "kind": "gradient", "stops": [{ "at": 0.0, "color": [0, 0, 0, 255] }] });
    assert!(turned_down(s.execute("palette.set", json!({ "swatch": 3, "kind": one })), "two colours or more"));
    s.execute("palette.set", json!({ "swatch": 3, "kind": { "kind": "solid" } })).unwrap();
    assert_eq!(first_row(&s, e)[20], [255, 0, 0, 255]);
}

#[test]
fn a_texture_swatch_paints_its_picture_and_has_it_read_again_when_opened() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, e) = two_squares(dir.path());
    let (red, yellow) = (image::Rgba([220, 50, 47, 255]), image::Rgba([250, 220, 60, 255]));
    image::RgbaImage::from_fn(2, 1, |x, _| if x == 0 { red } else { yellow }).save(dir.path().join("check.png")).unwrap();
    std::fs::write(dir.path().join("line.wav"), iskra_audio::write_wav16(&[0.1; 480], 1, 48_000)).unwrap();
    let made = s.execute("media.import", json!({ "paths": [dir.path().join("check.png"), dir.path().join("line.wav")] })).unwrap();
    let (picture, sound) = (made["media"][0].as_u64().unwrap(), made["media"][1].as_u64().unwrap());
    assert!(turned_down(s.execute("palette.set", json!({ "swatch": 3, "kind": { "kind": "texture", "media": sound } })), "not a picture"));
    assert!(turned_down(s.execute("palette.set", json!({ "swatch": 3, "kind": { "kind": "texture", "media": picture, "scale": 0.0 } })), "above 0"));
    s.execute("palette.set", json!({ "swatch": 3, "kind": { "kind": "texture", "media": picture } })).unwrap();
    // Tiled from each square's corner, a pixel of the picture to one of the drawing's; its
    // colour is the picture's own.
    let row = first_row(&s, e);
    assert_eq!(row[0..4], [red.0, yellow.0, red.0, yellow.0]);
    assert_eq!(s.project().palette.color(SwatchId(3)), [235, 135, 53, 255]);

    // The picture is not in the file: it is read again from its media when the project is opened.
    s.execute("file.save", Value::Null).unwrap();
    let mut s = Session::new();
    s.execute("file.open", json!({ "path": dir.path().join("squares.iskra") })).unwrap();
    assert_eq!(s.project().palette.pictures.len(), 1);
    let shot = s.project().shots[0].id.0;
    s.execute("shot.load", json!({ "shot": shot })).unwrap();
    assert_eq!(first_row(&s, e)[0..2], [red.0, yellow.0]);
    // Made solid, nothing paints with the picture, and it is let go.
    s.execute("palette.set", json!({ "swatch": 3, "kind": { "kind": "solid" } })).unwrap();
    assert!(s.project().palette.pictures.is_empty());
}

#[test]
fn a_palette_file_of_the_apps_own_carries_gradients_and_pictures_to_another_project() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, _) = two_squares(dir.path());
    image::RgbaImage::from_pixel(2, 2, image::Rgba([40, 160, 90, 255])).save(dir.path().join("moss.png")).unwrap();
    let made = s.execute("media.import", json!({ "paths": [dir.path().join("moss.png")] })).unwrap();
    let moss = made["media"][0].as_u64().unwrap();
    let ramp = json!({ "kind": "gradient", "shape": "radial", "stops": [{ "at": 0.0, "color": [255, 0, 0, 255] }, { "at": 1.0, "color": [0, 0, 255, 255] }] });
    s.execute("palette.set", json!({ "swatch": 3, "kind": ramp })).unwrap();
    s.execute("palette.set", json!({ "swatch": 4, "kind": { "kind": "texture", "media": moss, "tile": false, "scale": 2.0 } })).unwrap();
    let file = dir.path().join("looks.iskrapalette");
    s.execute("palette.export", json!({ "path": file })).unwrap();

    // Another project, with media of its own: the picture comes in as one more, under its id.
    let mut other = Session::new();
    image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255])).save(dir.path().join("dark.png")).unwrap();
    other.execute("media.import", json!({ "paths": [dir.path().join("dark.png")] })).unwrap();
    let made = other.execute("palette.import", json!({ "path": file, "asNew": true })).unwrap();
    let palette = &other.project().palette;
    let set = iskra_model::SetId(made["palette"].as_u64().unwrap() as u16);
    let kind = |name: &str| palette.swatches_of(set).find(|swatch| swatch.name == name).unwrap().kind.clone();
    let red = s.project().palette.swatch(SwatchId(3)).unwrap().name.clone();
    let green = s.project().palette.swatch(SwatchId(4)).unwrap().name.clone();
    assert!(matches!(kind(&red), iskra_model::SwatchKind::Gradient(gradient) if gradient.shape == iskra_model::GradientShape::Radial));
    let iskra_model::SwatchKind::Texture(texture) = kind(&green) else { panic!("{:?}", kind(&green)) };
    assert_eq!((texture.tile, texture.scale, other.project().media.len()), (false, 2.0, 2));
    assert_eq!(other.project().media(texture.media).unwrap().name, "moss");
    assert_eq!(palette.pictures.get(&texture.media).map(|picture| picture.pixels[0]), Some([40, 160, 90, 255]));
    // Brought in again, the picture is found among the media, not brought twice.
    other.execute("palette.import", json!({ "path": file, "asNew": true })).unwrap();
    assert_eq!(other.project().media.len(), 2);
    // With its picture gone, the texture is solid.
    std::fs::remove_file(dir.path().join("moss.png")).unwrap();
    let mut third = Session::new();
    let made = third.execute("palette.import", json!({ "path": file, "asNew": true })).unwrap();
    let set = iskra_model::SetId(made["palette"].as_u64().unwrap() as u16);
    assert!(third.project().palette.swatches_of(set).find(|swatch| swatch.name == green).unwrap().kind.is_solid());
}

#[test]
fn free_colour_is_painted_with_a_gradients_colours_and_keeps_them() {
    let mut s = Session::new();
    let e = layer(&s);
    assert!(!s.project().shots[0].layers[0].free_colour.is_empty(), "a new layer is free colour");
    draw_square(&mut s, e, false);
    let ramp = json!({ "kind": "gradient", "stops": [{ "at": 0.0, "color": [255, 0, 0, 255] }, { "at": 1.0, "color": [0, 0, 255, 255] }] });
    s.execute("palette.set", json!({ "swatch": 3, "kind": ramp })).unwrap();
    let free = |s: &Session, art: iskra_model::ArtLayerKind, x: i32, y: i32| {
        let drawing = &s.project().layer(iskra_model::LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
        drawing.free_colour_at(x, y, |kind| kind == art)
    };
    // The bucket fills the square's inside from red at its left to blue at its right.
    s.execute("drawing.fill", json!({ "layer": e, "frame": 0, "x": 250, "y": 250, "swatch": 3 })).unwrap();
    let colour = iskra_model::ArtLayerKind::ColourArt;
    let (left, right) = (free(&s, colour, 108, 250).unwrap(), free(&s, colour, 392, 250).unwrap());
    assert!(left[0] > 200 && left[2] < 50 && right[0] < 50 && right[2] > 200, "{left:?} {right:?}");
    // A stroke is laid on across what it covers, from its left end to its right.
    let points = json!([{ "x": 500, "y": 600, "pressure": 1.0 }, { "x": 800, "y": 600, "pressure": 1.0 }]);
    let stroke = json!({ "brush": { "size": 12, "pressureSize": false }, "points": points });
    s.execute("drawing.stroke", json!({ "layer": e, "frame": 0, "swatch": 3, "stroke": stroke })).unwrap();
    let lines = iskra_model::ArtLayerKind::LineArt;
    let (start, end) = (free(&s, lines, 500, 600).unwrap(), free(&s, lines, 800, 600).unwrap());
    assert!(start[0] > start[2] && end[2] > end[0], "{start:?} {end:?}");
    // Free colour keeps them: the swatch made solid changes nothing painted.
    s.execute("palette.set", json!({ "swatch": 3, "kind": { "kind": "solid" } })).unwrap();
    assert_eq!(free(&s, colour, 108, 250), Some(left));
}

#[test]
fn a_retake_note_on_frame_42_is_kept_in_the_file_and_goes_out_and_back_with_its_shot() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let shot = s.project().shots[0].id.0;
    s.execute("shot.setDuration", json!({ "shot": shot, "frames": 48 })).unwrap();
    let note = |s: &Session| s.project().shots[0].notes.clone();
    let made = s.execute("note.add", json!({ "frame": 42, "text": " The arm pops here ", "author": "Ana", "written": 100 })).unwrap();
    assert_eq!(made, json!({ "id": 1 }));
    s.execute("note.add", json!({ "shot": shot, "frame": 3, "length": 6, "text": "Too slow", "written": 200 })).unwrap();
    assert_eq!(s.history().undo_label(), Some("Add Retake Note"));
    let said: Vec<(u64, u32, u32, &str, &str)> =
        s.project().shots[0].notes.iter().map(|n| (n.id, n.frame, n.length, n.text.as_str(), n.author.as_str())).collect();
    assert_eq!(said, [(2, 3, 6, "Too slow", ""), (1, 42, 1, "The arm pops here", "Ana")], "in the order of their frames");
    assert!(turned_down(s.execute("note.add", json!({ "frame": 47, "length": 2, "text": "x" })), "has 48 frames"));
    assert!(turned_down(s.execute("note.add", json!({ "frame": 1, "text": "  " })), "some text"));
    assert!(turned_down(s.execute("note.set", json!({ "note": 9, "done": true })), "no note 9"));

    s.execute("note.set", json!({ "note": 2, "done": true, "text": "Too slow, still" })).unwrap();
    assert_eq!((note(&s)[0].done, note(&s)[0].text.as_str()), (true, "Too slow, still"));
    s.execute("note.remove", json!({ "note": 2 })).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(note(&s).len(), 2, "undone, it is back");
    let revision = s.project().shots[0].revision;
    s.execute("note.set", json!({ "note": 1, "frame": 41, "length": 2 })).unwrap();
    assert_eq!(s.project().shots[0].revision, revision, "a note changes nothing that renders");

    // Saved and opened again, the notes are there.
    let path = dir.path().join("Notes.iskra");
    s.execute("file.saveAs", json!({ "path": path })).unwrap();
    let mut again = Session::new();
    again.execute("file.open", json!({ "path": path })).unwrap();
    assert_eq!(note(&again), note(&s));

    // Sent out, the note on 41 ticked done there and one written; here, one written meanwhile.
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    s.execute("shot.sendOut", json!({ "shots": [shot], "folder": dir.path().join("out"), "to": "Ana" })).unwrap();
    let file = dir.path().join("out").join("Shot_01.iskra");
    let (from, sent, _) = iskra_store::read_shot_file(&file).unwrap();
    assert_eq!(sent.shots[0].notes, note(&s));
    let mut theirs = iskra_model::Project::clone(&sent);
    let out = std::sync::Arc::make_mut(&mut theirs.shots[0]);
    out.notes.iter_mut().filter(|n| n.frame == 41).for_each(|n| n.done = true);
    out.put_note(iskra_model::Note { id: 3, frame: 10, length: 1, text: "Fixed the feet too".into(), author: "Ana".into(), written: 300, done: false });
    iskra_store::write_shot_file(&file, &std::sync::Arc::new(theirs), &from.unwrap()).unwrap();
    s.execute("note.add", json!({ "frame": 20, "text": "Meanwhile", "written": 400 })).unwrap();
    let looked = s.execute("shot.bringBack", json!({ "path": file, "preview": true })).unwrap();
    let differences: Vec<&str> = looked["differences"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(differences.contains(&"1 retake note ticked done.") && differences.contains(&"1 new retake note written."), "{differences:?}");
    s.execute("shot.bringBack", json!({ "path": file })).unwrap();
    let back: Vec<(&str, bool)> = s.project().shots[0].notes.iter().map(|n| (n.text.as_str(), n.done)).collect();
    assert_eq!(back, [("Too slow, still", true), ("Fixed the feet too", false), ("Meanwhile", false), ("The arm pops here", true)]);
}

#[test]
fn a_brush_stamping_a_tip_draws_a_line_of_its_stamps_never_flattened() {
    use iskra_model::{ArtContent, ArtLayerKind, LayerId, ShapeKind};
    let mut s = Session::new();
    let e = layer(&s);
    s.execute("layer.setArtMode", json!({ "layer": e, "artLayer": "lineArt", "mode": "vector" })).unwrap();
    // The engine's Splatter, as the app sends a brush: its tip with it.
    let splatter = iskra_drawing::builtin_brushes().iter().find(|preset| preset.name == "Splatter").unwrap();
    let iskra_drawing::Brush::Vector(brush) = &splatter.brush else { panic!("a vector brush") };
    let brush = serde_json::to_value(iskra_drawing::VectorBrush { seed: 9, smoothing: 0.0, ..brush.clone() }).unwrap();
    let points: Vec<Value> = (0..=20).map(|i| json!({ "x": 100 + 10 * i, "y": 100, "pressure": 1.0 })).collect();
    for _ in 0..2 {
        s.execute("drawing.vectorStroke", json!({ "layer": e, "frame": 0, "swatch": 1, "points": points, "brush": brush, "flatten": true })).unwrap();
    }
    let drawing = &s.project().layer(LayerId(e)).unwrap().drawing_at(0).unwrap().drawing;
    let ArtContent::Vector(art) = &drawing.layer(ArtLayerKind::LineArt).unwrap().content else { panic!("vector line art") };
    assert_eq!(art.shapes().len(), 2, "two lines, not one fill");
    let ShapeKind::Stroke(stroke) = &art.shapes()[0].kind else { panic!("a stroke") };
    let stamp = stroke.stamp.as_ref().expect("stamped");
    assert!(iskra_drawing::tip(&stamp.tip).is_some() && art.tips().len() == 1);
    // Its stamps' grain: along the line much is partly covered.
    let row: Vec<u16> = (100..300).map(|x| line_pixel(&s, e, 0, x, 100).map_or(0, |(_, coverage)| coverage)).collect();
    assert!(row.iter().filter(|c| **c > 3000 && **c < 62000).count() > 20, "{row:?}");
    // A spacing that is not a number is refused.
    let refused = s.execute(
        "drawing.vectorStroke",
        json!({ "layer": e, "frame": 0, "swatch": 1, "points": points, "brush": { "size": 4, "stamp": { "enabled": true, "spacing": "wide" } } }),
    );
    assert!(refused.is_err());
}

#[test]
fn a_peg_deleted_takes_its_driver_with_it_and_empties_the_ports_it_was_wired_into() {
    use iskra_model::{Driver, DriverId, DriverInput, Expressions, PegId};
    let mut s = Session::new();
    let car = s.execute("peg.new", json!({})).unwrap()["id"].as_u64().unwrap();
    let wheel = s.execute("peg.new", json!({})).unwrap()["id"].as_u64().unwrap();
    let shadow = s.execute("peg.new", json!({})).unwrap()["id"].as_u64().unwrap();
    // Until the driver's commands (plan §8.8 step 3), wired in by hand.
    s.edit("Drivers", |p| {
        let (first, second) = (DriverId(p.new_id()), DriverId(p.new_id()));
        let shot = std::sync::Arc::make_mut(&mut p.shots[0]);
        let mut wheel_driver = Driver::new(first, "Wheel Driver", PegId(wheel));
        wheel_driver.expressions = Expressions::parse("rotation = car.x").unwrap();
        wheel_driver.inputs = vec![DriverInput { name: "car".into(), peg: Some(PegId(car)) }];
        let mut shadow_driver = Driver::new(second, "Shadow Driver", PegId(shadow));
        shadow_driver.inputs = vec![DriverInput { name: "wheel".into(), peg: Some(PegId(wheel)) }];
        shot.drivers = vec![std::sync::Arc::new(wheel_driver), std::sync::Arc::new(shadow_driver)];
        Ok(())
    })
    .unwrap();
    s.execute("peg.delete", json!({ "peg": car })).unwrap();
    let shot = &s.project().shots[0];
    assert_eq!(shot.driver_of(PegId(wheel)).unwrap().inputs[0].peg, None);
    s.execute("peg.delete", json!({ "peg": wheel })).unwrap();
    let shot = &s.project().shots[0];
    assert!(shot.driver_of(PegId(wheel)).is_none());
    assert_eq!(shot.driver_of(PegId(shadow)).unwrap().inputs[0].peg, None);
    s.undo();
    s.undo();
    let shot = &s.project().shots[0];
    assert_eq!(shot.driver_of(PegId(wheel)).unwrap().inputs[0].peg, Some(PegId(car)));
}

/// A car keyed from 0 to 100 across over ten frames, a wheel under it and a shadow beside it.
fn car_wheel_and_shadow(s: &mut Session) -> (u64, u64, u64) {
    let new =
        |s: &mut Session, name: &str, parent: Option<u64>| s.execute("peg.new", json!({ "name": name, "parent": parent })).unwrap()["id"].as_u64().unwrap();
    let car = new(s, "Car-P", None);
    let wheel = new(s, "Wheel-P", Some(car));
    let shadow = new(s, "Shadow-P", None);
    s.execute("peg.setTransform", json!({ "peg": car, "frame": 0, "position": [0.0, 0.0], "animate": true })).unwrap();
    s.execute("peg.setTransform", json!({ "peg": car, "frame": 10, "position": [100.0, 0.0], "animate": true })).unwrap();
    (car, wheel, shadow)
}

fn posed(s: &Session, peg: u64, frame: u32) -> iskra_model::Transform {
    let shot = &s.project().shots[0];
    shot.pose(shot.peg(iskra_model::PegId(peg)).unwrap(), frame)
}

#[test]
fn a_driver_is_made_set_and_wired_by_its_commands_each_one_undo_step() {
    let mut s = Session::new();
    let (car, wheel, shadow) = car_wheel_and_shadow(&mut s);
    let revision = s.project().shots[0].revision;
    let driver = s
        .execute(
            "driver.new",
            json!({ "peg": wheel, "expressions": "rotation = parent.travelled / radius * 180 / PI", "parameters": [{ "name": "radius", "value": 50.0 }] }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    assert!((posed(&s, wheel, 10).rotation - 2f64.to_degrees()).abs() < 1e-9);
    assert_ne!(s.project().shots[0].revision, revision, "the shot renders otherwise");
    let held = |s: &Session| s.project().shots[0].driver(iskra_model::DriverId(driver)).unwrap().clone();
    assert_eq!(held(&s).name, "Wheel-P Driver");
    // One driver a peg, a name of its own, and expressions that read what it has.
    assert!(turned_down(s.execute("driver.new", json!({ "peg": wheel })), "a driver already"));
    assert!(turned_down(s.execute("driver.new", json!({ "peg": shadow, "name": "Car-P" })), "already has a peg named `Car-P`"));
    assert!(turned_down(s.execute("driver.new", json!({ "peg": shadow, "expressions": "x = car.x" })), "read `car`"));
    assert!(turned_down(s.execute("driver.new", json!({ "peg": shadow, "expressions": "x = (" })), "line 1, column"));
    assert!(turned_down(s.execute("driver.setExpressions", json!({ "driver": driver, "expressions": "rotation = speed" })), "read `speed`"));
    let why = s.execute("driver.setExpressions", json!({ "driver": driver, "expressions": "rotation = 1 +" })).unwrap_err().to_string();
    assert!(why.contains("line 1, column"), "{why}");

    // A parameter set, keyed, and its key moved, eased and taken away.
    s.execute("driver.set", json!({ "driver": driver, "parameter": "radius", "value": 25.0 })).unwrap();
    assert!((posed(&s, wheel, 10).rotation - 4f64.to_degrees()).abs() < 1e-9);
    s.execute("driver.setKey", json!({ "driver": driver, "parameter": "radius", "frame": 0 })).unwrap();
    s.execute("driver.set", json!({ "driver": driver, "parameter": "radius", "value": 100.0, "frame": 10 })).unwrap();
    assert!(turned_down(s.execute("driver.set", json!({ "driver": driver, "parameter": "radius", "value": 5.0 })), "say on which `frame`"));
    assert_eq!(held(&s).parameters[0].at(5), 62.5);
    s.execute("driver.moveKey", json!({ "driver": driver, "parameter": "radius", "from": 10, "to": 20 })).unwrap();
    s.execute("driver.setEase", json!({ "driver": driver, "frame": 20, "into": "ease" })).unwrap();
    assert_eq!(held(&s).parameters[0].track.keys().iter().find(|key| key.frame == 20).map(|key| key.into), Some(iskra_model::Interp::Ease));
    s.execute("driver.removeKey", json!({ "driver": driver, "frame": 0 })).unwrap();
    s.execute("driver.removeKey", json!({ "driver": driver, "frame": 20 })).unwrap();
    assert_eq!((held(&s).parameters[0].track.is_empty(), held(&s).parameters[0].value), (true, 100.0));
    assert!(turned_down(s.execute("driver.removeKey", json!({ "driver": driver, "frame": 3 })), "no keyframe on frame 3"));

    // Inputs and parameters added, renamed where the expressions read them, and taken away.
    s.execute("driver.addInput", json!({ "driver": driver, "name": "car", "peg": car })).unwrap();
    s.execute("driver.setExpressions", json!({ "driver": driver, "expressions": "rotation = car.x / radius" })).unwrap();
    s.execute("driver.renameInput", json!({ "driver": driver, "input": "car", "name": "truck" })).unwrap();
    s.execute("driver.renameParameter", json!({ "driver": driver, "parameter": "radius", "name": "size" })).unwrap();
    assert_eq!(held(&s).expressions.text(), "rotation = truck.x / size");
    assert!(turned_down(s.execute("driver.removeInput", json!({ "driver": driver, "input": "truck" })), "read `truck`"));
    assert!(turned_down(s.execute("driver.addParameter", json!({ "driver": driver, "name": "truck" })), "an input named `truck` already"));
    assert!(turned_down(s.execute("driver.addParameter", json!({ "driver": driver, "name": "frame" })), "a name the expressions have already"));
    assert!(turned_down(s.execute("driver.addParameter", json!({ "driver": driver, "name": "two words" })), "can't be read by expressions"));
    s.execute("driver.addParameter", json!({ "driver": driver, "name": "spare", "value": 3.0 })).unwrap();
    s.execute("driver.removeParameter", json!({ "driver": driver, "parameter": "spare" })).unwrap();
    s.execute("driver.connect", json!({ "driver": driver, "input": "truck" })).unwrap();
    assert!(s.project().shots[0].amounts(&held(&s), 3).failed, "an empty port reads nothing");
    s.execute("driver.connect", json!({ "driver": driver, "input": "truck", "peg": car })).unwrap();

    // Off, on, renamed; and each command one undo step.
    s.execute("driver.setEnabled", json!({ "driver": driver, "enabled": false })).unwrap();
    assert_eq!(posed(&s, wheel, 10).rotation, 0.0);
    s.undo();
    assert_eq!(posed(&s, wheel, 10).rotation, 1.0);
    s.execute("driver.rename", json!({ "driver": driver, "name": "Roll" })).unwrap();
    assert!(turned_down(s.execute("driver.rename", json!({ "driver": driver, "name": "Shadow-P" })), "already has a peg named `Shadow-P`"));
    assert_eq!(held(&s).name, "Roll");
    s.execute("driver.delete", json!({ "driver": driver })).unwrap();
    assert!(s.project().shots[0].drivers.is_empty());
    s.undo();
    assert_eq!(held(&s).name, "Roll");
    let _ = shadow;
}

#[test]
fn a_driver_is_not_wired_round_in_a_loop_by_any_command() {
    let mut s = Session::new();
    let (car, wheel, shadow) = car_wheel_and_shadow(&mut s);
    let on_car = s.execute("driver.new", json!({ "peg": car })).unwrap()["id"].as_u64().unwrap();
    assert!(turned_down(s.execute("driver.addInput", json!({ "driver": on_car, "name": "me", "peg": car })), "can't drive it"));
    assert!(turned_down(s.execute("driver.addInput", json!({ "driver": on_car, "name": "wheel", "peg": wheel })), "can't drive it"));
    assert!(turned_down(s.execute("driver.new", json!({ "peg": car, "inputs": [{ "name": "w", "peg": wheel }] })), "a driver already"));
    s.execute("driver.addInput", json!({ "driver": on_car, "name": "shadow", "peg": shadow })).unwrap();
    // The shadow can't now read the car, which reads it; nor go under it.
    assert!(turned_down(s.execute("driver.new", json!({ "peg": shadow, "inputs": [{ "name": "car", "peg": car }] })), "can't drive it"));
    let on_shadow = s.execute("driver.new", json!({ "peg": shadow })).unwrap()["id"].as_u64().unwrap();
    assert!(turned_down(s.execute("driver.addInput", json!({ "driver": on_shadow, "name": "wheel", "peg": wheel })), "can't drive it"));
    assert!(turned_down(s.execute("peg.setParent", json!({ "peg": shadow, "parent": car })), "move itself"));
    s.execute("driver.connect", json!({ "driver": on_car, "input": "shadow" })).unwrap();
    s.execute("peg.setParent", json!({ "peg": shadow, "parent": car })).unwrap();
}

#[test]
fn a_driver_baked_leaves_its_peg_moving_as_it_did_with_keys() {
    let mut s = Session::new();
    let (car, wheel, shadow) = car_wheel_and_shadow(&mut s);
    s.execute("peg.setTransform", json!({ "peg": shadow, "position": [0.0, 20.0] })).unwrap();
    let driver = s
        .execute("driver.new", json!({ "peg": shadow, "expressions": "x = car.x\nscaleX = -car.x / 200", "inputs": [{ "name": "car", "peg": car }] }))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let before: Vec<_> = (0..=12).map(|frame| posed(&s, shadow, frame)).collect();
    let baked = s.execute("driver.bake", json!({ "driver": driver, "from": 2, "to": 12 })).unwrap();
    assert_eq!(baked["frames"], 11);
    let shot = &s.project().shots[0];
    assert!(shot.drivers.is_empty());
    let peg = shot.peg(iskra_model::PegId(shadow)).unwrap();
    for frame in 2..=12 {
        let (now, was) = (peg.transform_at(frame), before[frame as usize]);
        assert!((now.position[0] - was.position[0]).abs() < 1e-9 && (now.scale[0] - was.scale[0]).abs() < 1e-9, "frame {frame}");
        assert_eq!(now.position[1], 20.0, "what it did not give stays as it was");
    }
    assert!(peg.animation.rotation.is_empty(), "nor is it keyed");
    // Before the frames baked, it holds the first.
    assert_eq!(peg.transform_at(0).position[0], before[2].position[0]);
    s.undo();
    assert_eq!(s.project().shots[0].drivers.len(), 1);
    assert!(turned_down(s.execute("driver.bake", json!({ "driver": driver, "from": 5, "to": 2 })), "is after frame"));
    let _ = wheel;
}

#[test]
fn a_preset_put_on_a_driver_brings_its_inputs_and_parameters() {
    let mut s = Session::new();
    let (car, _, shadow) = car_wheel_and_shadow(&mut s);
    let driver = s.execute("driver.new", json!({ "peg": shadow, "parameters": [{ "name": "ground", "value": 20.0 }] })).unwrap()["id"].as_u64().unwrap();
    s.execute("driver.setPreset", json!({ "driver": driver, "preset": "Shadow from Height" })).unwrap();
    let held = s.project().shots[0].driver(iskra_model::DriverId(driver)).unwrap().clone();
    assert_eq!(held.preset.as_deref(), Some("Shadow from Height"));
    assert_eq!(held.inputs.iter().map(|input| (input.name.as_str(), input.peg)).collect::<Vec<_>>(), [("char", None)]);
    let values: Vec<(&str, f64)> = held.parameters.iter().map(|p| (p.name.as_str(), p.value)).collect();
    assert_eq!(values, [("ground", 0.0), ("height", 300.0), ("shrink", -0.6)], "the preset's values, the one it had among them");
    s.execute("driver.connect", json!({ "driver": driver, "input": "char", "peg": car })).unwrap();
    assert!((posed(&s, shadow, 10).position[0] - 100.0).abs() < 1e-9);
    // One of the user's, whole; changed by hand it is no preset's any more.
    let own = json!({ "name": "Mine", "expressions": "rotation = k", "parameters": [{ "name": "k", "value": 7.0 }] });
    s.execute("driver.setPreset", json!({ "driver": driver, "preset": own })).unwrap();
    assert_eq!(posed(&s, shadow, 3).rotation, 7.0);
    s.execute("driver.setExpressions", json!({ "driver": driver, "expressions": "rotation = k * 2" })).unwrap();
    assert_eq!(s.project().shots[0].driver(iskra_model::DriverId(driver)).unwrap().preset, None);
    assert!(turned_down(s.execute("driver.setPreset", json!({ "driver": driver, "preset": "Nope" })), "no preset `Nope`: there are `Roll with Parent`"));
}

#[test]
fn a_drivers_input_or_parameter_has_a_name_of_32_letters_at_most() {
    let mut s = Session::new();
    let (_, wheel, _) = car_wheel_and_shadow(&mut s);
    let driver = s.execute("driver.new", json!({ "peg": wheel })).unwrap()["id"].as_u64().unwrap();
    let long = "a".repeat(33);
    assert!(turned_down(s.execute("driver.addParameter", json!({ "driver": driver, "name": long })), "longer than 32 letters"));
    assert!(turned_down(s.execute("driver.addInput", json!({ "driver": driver, "name": long })), "longer than 32 letters"));
    s.execute("driver.addParameter", json!({ "driver": driver, "name": "a".repeat(32) })).unwrap();
}
