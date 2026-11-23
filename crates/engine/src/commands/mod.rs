//! The command registry. Ids are stable: menus, shortcuts, the CLI, MCP and scripts all name
//! commands by id, and saved keymaps refer to them.

mod bin;
mod camera;
mod character;
mod deformer;
mod display;
mod drawing;
mod effect;
mod export;
mod file;
mod graph;
mod group;
mod guide;
mod handoff;
mod keys;
mod layer;
mod library;
mod media;
mod note;
mod palette;
mod panel;
mod peg;
mod rig;
mod select;
mod shot;
mod skin;
mod sound;
mod storyboard;
mod text;

use iskra_model::{FrameRate, Project, Shot, ShotId, WorkingSpace};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{EngineError, Result, Session};

pub struct Command {
    pub id: &'static str,
    /// The name shown in menus and in the history.
    pub title: &'static str,
    pub run: fn(&mut Session, Value) -> Result<Value>,
}

/// Every command, in menu order.
pub fn commands() -> &'static [Command] {
    COMMANDS
}

const COMMANDS: &[Command] = &[
    Command { id: "app.commands", title: "List Commands", run: app_commands },
    Command { id: "file.new", title: "New Project", run: file::new },
    Command { id: "file.open", title: "Open Project", run: file::open },
    Command { id: "file.save", title: "Save", run: file::save },
    Command { id: "file.saveAs", title: "Save As", run: file::save_as },
    Command { id: "file.autosave", title: "Autosave", run: file::autosave },
    Command { id: "file.recover", title: "Recover from Backup", run: file::recover },
    Command { id: "edit.undo", title: "Undo", run: edit_undo },
    Command { id: "edit.redo", title: "Redo", run: edit_redo },
    Command { id: "edit.history", title: "List History", run: edit_history },
    Command { id: "edit.gotoHistory", title: "Go to History State", run: edit_goto_history },
    Command { id: "edit.batch", title: "Several Changes", run: edit_batch },
    Command { id: "script.run", title: "Run Script", run: script_run },
    Command { id: "project.get", title: "Get Project", run: project_get },
    Command { id: "project.setResolution", title: "Set Resolution", run: project_set_resolution },
    Command { id: "project.setFrameRate", title: "Set Frame Rate", run: project_set_frame_rate },
    Command { id: "project.setWorkingSpace", title: "Set Colour Working Space", run: project_set_working_space },
    Command { id: "project.setDrawingDensity", title: "Set Drawing Resolution", run: project_set_drawing_density },
    Command { id: "project.setVolume", title: "Set Master Volume", run: project_set_volume },
    Command { id: "bin.new", title: "New Bin", run: bin::new },
    Command { id: "bin.rename", title: "Rename Bin", run: bin::rename },
    Command { id: "bin.delete", title: "Delete Bin", run: bin::delete },
    Command { id: "bin.setParent", title: "Move Bin", run: bin::set_parent },
    Command { id: "item.setBin", title: "Move to Bin", run: bin::set_bin },
    Command { id: "media.import", title: "Import Media", run: media::import },
    Command { id: "media.rename", title: "Rename Media", run: media::rename },
    Command { id: "media.delete", title: "Delete Media", run: media::delete },
    Command { id: "media.relink", title: "Relink Media", run: media::relink },
    Command { id: "media.setTranscript", title: "Set Transcript", run: media::set_transcript },
    Command { id: "media.place", title: "Place Media", run: media::place },
    Command { id: "character.import", title: "Import Character", run: character::import },
    Command { id: "rig.import", title: "Import Rig", run: rig::import },
    Command { id: "export.sequence", title: "Render Shots", run: export::sequence },
    Command { id: "export.storyboard", title: "Render Storyboard", run: export::storyboard },
    Command { id: "export.editList", title: "Write Edit List", run: export::edit_list },
    Command { id: "export.pdf", title: "Export Storyboard PDF", run: export::pdf },
    Command { id: "sound.place", title: "Place Sound", run: sound::place },
    Command { id: "sound.setClip", title: "Change Sound Clip", run: sound::set_clip },
    Command { id: "sound.delete", title: "Delete Sound Clip", run: sound::delete },
    Command { id: "sound.setTrack", title: "Change Sound Track", run: sound::set_track },
    Command { id: "layer.refreshMedia", title: "Read Pictures Again", run: media::refresh },
    Command { id: "layer.setTransparency", title: "Set Pictures' Transparency", run: media::set_transparency },
    Command { id: "storyboard.new", title: "New Storyboard", run: storyboard::new },
    Command { id: "storyboard.setStart", title: "Set Start Timecode", run: storyboard::set_start },
    Command { id: "storyboard.rename", title: "Rename Storyboard", run: storyboard::rename },
    Command { id: "storyboard.delete", title: "Delete Storyboard", run: storyboard::delete },
    Command { id: "storyboard.place", title: "Place Shot on Storyboard", run: storyboard::place },
    Command { id: "storyboard.remove", title: "Remove Shot from Storyboard", run: storyboard::remove },
    Command { id: "storyboard.moveClip", title: "Move Shot", run: storyboard::move_clip },
    Command { id: "storyboard.trimClip", title: "Trim Shot", run: storyboard::trim_clip },
    Command { id: "storyboard.setTransition", title: "Set Transition", run: storyboard::set_transition },
    Command { id: "storyboard.removeTransition", title: "Remove Transition", run: storyboard::remove_transition },
    Command { id: "storyboard.setMarker", title: "Set Marker", run: storyboard::set_marker },
    Command { id: "storyboard.removeMarker", title: "Remove Marker", run: storyboard::remove_marker },
    Command { id: "storyboard.setRange", title: "Set Play Range", run: storyboard::set_range },
    Command { id: "shot.new", title: "New Shot", run: shot::new },
    Command { id: "shot.rename", title: "Rename Shot", run: shot::rename },
    Command { id: "shot.renameNumbered", title: "Rename Shots", run: shot::rename_numbered },
    Command { id: "shot.setScene", title: "Set Scene and Sequence", run: shot::set_scene },
    Command { id: "shot.renameScene", title: "Rename Scene or Sequence", run: shot::rename_scene },
    Command { id: "shot.delete", title: "Delete Shot", run: shot::delete },
    Command { id: "shot.duplicate", title: "Duplicate Shot", run: shot::duplicate },
    Command { id: "shot.split", title: "Split Shot", run: shot::split },
    Command { id: "shot.merge", title: "Merge Shots", run: shot::merge },
    Command { id: "shot.setDuration", title: "Set Shot Duration", run: shot::set_duration },
    Command { id: "shot.setMarker", title: "Set Marker", run: shot::set_marker },
    Command { id: "shot.removeMarker", title: "Remove Marker", run: shot::remove_marker },
    Command { id: "note.add", title: "Add Retake Note", run: note::add },
    Command { id: "note.set", title: "Change Retake Note", run: note::set },
    Command { id: "note.remove", title: "Remove Retake Note", run: note::remove },
    Command { id: "shot.setRange", title: "Set In and Out Points", run: shot::set_range },
    Command { id: "shot.setHidden", title: "Hide or Show Row", run: shot::set_hidden },
    Command { id: "shot.setLocked", title: "Lock or Unlock Row", run: shot::set_locked },
    Command { id: "shot.setUnlocked", title: "Lock or Unlock Board", run: shot::set_unlocked },
    Command { id: "panel.add", title: "New Panel", run: panel::add },
    Command { id: "panel.set", title: "Write Panel Notes", run: panel::set },
    Command { id: "panel.move", title: "Move Panel", run: panel::move_to },
    Command { id: "panel.remove", title: "Remove Panel", run: panel::remove },
    Command { id: "panel.list", title: "List Panels", run: panel::list },
    Command { id: "storyboard.fillDialogue", title: "Fill Dialogue from Transcripts", run: panel::fill_dialogue },
    Command { id: "shot.setStatus", title: "Set Shot Status", run: handoff::set_status },
    Command { id: "shot.sendOut", title: "Send Shots Out", run: handoff::send_out },
    Command { id: "shot.bringBack", title: "Bring Shot Back", run: handoff::bring_back },
    Command { id: "shot.renderFrame", title: "Render Frame", run: shot::render_frame },
    Command { id: "shot.load", title: "Load Shot", run: shot::load },
    Command { id: "shot.unload", title: "Unload Shot", run: shot::unload },
    Command { id: "layer.new", title: "New Layer", run: layer::new },
    Command { id: "layer.rename", title: "Rename Layer", run: layer::rename },
    Command { id: "layer.useAsAnimation", title: "Use Board as Animation", run: layer::use_as_animation },
    Command { id: "layer.delete", title: "Delete Layer", run: layer::delete },
    Command { id: "layer.move", title: "Reorder Layer", run: layer::reorder },
    Command { id: "layer.setDensity", title: "Change Layer Resolution", run: layer::set_density },
    Command { id: "layer.setArtMode", title: "Change Art Layer Kind", run: layer::set_art_mode },
    Command { id: "layer.setArtShown", title: "Show Art Layer", run: layer::set_art_shown },
    Command { id: "layer.setArtPorts", title: "Show Art Layer Ports", run: layer::set_art_ports },
    Command { id: "layer.setParent", title: "Parent Layer", run: layer::set_parent },
    Command { id: "composite.new", title: "New Composite", run: graph::new_composite },
    Command { id: "composite.rename", title: "Rename Composite", run: graph::rename_composite },
    Command { id: "composite.delete", title: "Delete Composite", run: graph::delete_composite },
    Command { id: "composite.setOrdering", title: "Set Composite's Ordering", run: graph::set_composite_ordering },
    Command { id: "node.connect", title: "Connect Node", run: graph::connect },
    Command { id: "node.disconnect", title: "Disconnect Node", run: graph::disconnect },
    Command { id: "display.set", title: "Set Display", run: display::set },
    Command { id: "display.new", title: "New Display", run: display::new },
    Command { id: "display.setMain", title: "Make Main Display", run: display::set_main },
    Command { id: "display.rename", title: "Rename Display", run: display::rename },
    Command { id: "pass.rename", title: "Rename Pass", run: display::rename_pass },
    Command { id: "display.delete", title: "Delete Display", run: display::delete },
    Command { id: "node.place", title: "Move Node", run: graph::place },
    Command { id: "node.setColour", title: "Colour Node", run: graph::set_colour },
    Command { id: "node.copy", title: "Copy Nodes", run: library::copy_nodes },
    Command { id: "node.paste", title: "Paste Nodes", run: library::paste_nodes },
    Command { id: "keys.copy", title: "Copy Keyframes", run: keys::copy },
    Command { id: "keys.paste", title: "Paste Keyframes", run: keys::paste },
    Command { id: "keys.step", title: "Hold Keyframes in Steps", run: keys::step },
    Command { id: "keys.chart", title: "Space In-betweens", run: keys::chart },
    Command { id: "keys.cycle", title: "Cycle Keyframes", run: keys::cycle },
    Command { id: "node.duplicate", title: "Duplicate Nodes", run: library::duplicate_nodes },
    Command { id: "effect.kinds", title: "List Kinds of Effect", run: effect::kinds },
    Command { id: "effect.new", title: "Add Effect", run: effect::new },
    Command { id: "effect.rename", title: "Rename Effect", run: effect::rename },
    Command { id: "effect.delete", title: "Delete Effect", run: effect::delete },
    Command { id: "effect.connect", title: "Wire Effect", run: effect::connect },
    Command { id: "effect.putOn", title: "Put Effect on a Wire", run: effect::put_on },
    Command { id: "effect.takeOff", title: "Take Effect off Its Wire", run: effect::take_off },
    Command { id: "effect.set", title: "Set Effect", run: effect::set },
    Command { id: "effect.setKey", title: "Add Keyframe", run: effect::set_key },
    Command { id: "effect.removeKey", title: "Remove Keyframe", run: effect::remove_key },
    Command { id: "effect.moveKey", title: "Move Keyframe", run: effect::move_key },
    Command { id: "effect.setEase", title: "Set Keyframe Ease", run: effect::set_ease },
    Command { id: "effect.setEnabled", title: "Switch Effect On or Off", run: effect::set_enabled },
    Command { id: "effect.setParent", title: "Wire a Transform into a Port", run: effect::set_parent },
    Command { id: "library.list", title: "List Library Folder", run: library::list },
    Command { id: "library.newFolder", title: "New Library Folder", run: library::new_folder },
    Command { id: "library.save", title: "Save to Library", run: library::save },
    Command { id: "library.place", title: "Place Template", run: library::place },
    Command { id: "library.apply", title: "Apply Template", run: library::apply },
    Command { id: "library.rename", title: "Rename Template", run: library::rename },
    Command { id: "library.delete", title: "Delete Template", run: library::delete },
    Command { id: "deformer.new", title: "New Bones", run: deformer::new },
    Command { id: "deformer.rename", title: "Rename Deformer", run: deformer::rename },
    Command { id: "deformer.delete", title: "Delete Deformer", run: deformer::delete },
    Command { id: "deformer.setRest", title: "Set Bones' Rest", run: deformer::set_rest },
    Command { id: "deformer.removeBone", title: "Remove Bone", run: deformer::remove_bone },
    Command { id: "deformer.setPose", title: "Pose Bones", run: deformer::set_pose },
    Command { id: "deformer.setIk", title: "Set Bones' IK", run: deformer::set_ik },
    Command { id: "deformer.setPort", title: "Wire Peg to Deformer", run: deformer::set_port },
    Command { id: "deformer.setKey", title: "Add Keyframe", run: deformer::set_key },
    Command { id: "deformer.removeKey", title: "Remove Keyframe", run: deformer::remove_key },
    Command { id: "deformer.moveKey", title: "Move Keyframe", run: deformer::move_key },
    Command { id: "deformer.setEase", title: "Set Keyframe Ease", run: deformer::set_ease },
    Command { id: "deformer.setWeights", title: "Set Bones' Weights", run: deformer::set_weights },
    Command { id: "deformer.setBend", title: "Shape Curve at Point", run: deformer::set_bend },
    Command { id: "deformer.insertKnot", title: "Add Point", run: deformer::insert_knot },
    Command { id: "deformer.removeKnot", title: "Remove Point", run: deformer::remove_knot },
    Command { id: "group.new", title: "New Group", run: group::new },
    Command { id: "group.rename", title: "Rename Group", run: group::rename },
    Command { id: "group.delete", title: "Ungroup", run: group::delete },
    Command { id: "group.add", title: "Add to Group", run: group::add },
    Command { id: "group.remove", title: "Take Out of Group", run: group::remove },
    Command { id: "group.set", title: "Set Group", run: group::set },
    Command { id: "layer.setDeformer", title: "Set Layer's Deformer", run: deformer::set_layer },
    Command { id: "peg.new", title: "New Peg", run: peg::new },
    Command { id: "peg.rename", title: "Rename Peg", run: peg::rename },
    Command { id: "peg.delete", title: "Delete Peg", run: peg::delete },
    Command { id: "peg.setParent", title: "Parent Peg", run: peg::set_parent },
    Command { id: "peg.setFolded", title: "Fold Peg", run: peg::set_folded },
    Command { id: "peg.setFollow", title: "Follow Bones", run: peg::set_follow },
    Command { id: "peg.setInherit", title: "Set What Peg Inherits", run: peg::set_inherit },
    Command { id: "peg.setTransform", title: "Transform Peg", run: peg::set_transform },
    Command { id: "peg.setKey", title: "Add Keyframe", run: peg::set_key },
    Command { id: "peg.removeKey", title: "Remove Keyframe", run: peg::remove_key },
    Command { id: "peg.moveKey", title: "Move Keyframe", run: peg::move_key },
    Command { id: "peg.setEase", title: "Set Keyframe Ease", run: peg::set_ease },
    Command { id: "peg.setBend", title: "Bend Motion Path", run: peg::set_bend },
    Command { id: "camera.setParent", title: "Parent Camera", run: camera::set_parent },
    Command { id: "camera.setLens", title: "Set Camera Lens", run: camera::set_lens },
    Command { id: "camera.setKey", title: "Add Lens Keyframe", run: camera::set_key },
    Command { id: "camera.moveKey", title: "Move Camera Keyframe", run: camera::move_key },
    Command { id: "camera.setEase", title: "Set Camera Keyframe Ease", run: camera::set_ease },
    Command { id: "camera.removeKey", title: "Remove Lens Keyframe", run: camera::remove_key },
    Command { id: "guide.add", title: "New Drawing Guide", run: guide::add },
    Command { id: "guide.set", title: "Change Drawing Guide", run: guide::set },
    Command { id: "guide.remove", title: "Remove Drawing Guide", run: guide::remove },
    Command { id: "palette.add", title: "New Swatch", run: palette::add },
    Command { id: "palette.set", title: "Change Swatch", run: palette::set },
    Command { id: "palette.move", title: "Move Swatch", run: palette::move_to },
    Command { id: "palette.remove", title: "Remove Swatch", run: palette::remove },
    Command { id: "palette.usage", title: "Report Swatch Use", run: palette::usage },
    Command { id: "palette.removeUnused", title: "Remove Unused Swatches", run: palette::remove_unused },
    Command { id: "palette.new", title: "New Palette", run: palette::new_set },
    Command { id: "palette.rename", title: "Rename Palette", run: palette::rename_set },
    Command { id: "palette.delete", title: "Remove Palette", run: palette::delete_set },
    Command { id: "palette.reorder", title: "Move Palette", run: palette::move_set },
    Command { id: "palette.import", title: "Import Palette", run: palette::import },
    Command { id: "palette.export", title: "Export Palette", run: palette::export },
    Command { id: "palette.addStyle", title: "New Palette Style", run: palette::add_style },
    Command { id: "palette.renameStyle", title: "Rename Palette Style", run: palette::rename_style },
    Command { id: "palette.removeStyle", title: "Remove Palette Style", run: palette::remove_style },
    Command { id: "shot.setStyle", title: "Set Palette Style", run: palette::set_shot_style },
    Command { id: "skin.add", title: "New Skin", run: skin::add },
    Command { id: "skin.rename", title: "Rename Skin", run: skin::rename },
    Command { id: "skin.remove", title: "Remove Skin", run: skin::remove },
    Command { id: "skin.addVariant", title: "New Skin Variant", run: skin::add_variant },
    Command { id: "skin.removeVariant", title: "Remove Skin Variant", run: skin::remove_variant },
    Command { id: "shot.setSkin", title: "Set Skin", run: skin::set_shot_skin },
    Command { id: "skin.import", title: "Import Skins", run: character::import_skins },
    Command { id: "skin.fromFolder", title: "Add Skin From Folder", run: media::skin_from_folder },
    Command { id: "drawing.newDrawing", title: "New Drawing", run: drawing::new_drawing },
    Command { id: "drawing.duplicate", title: "Duplicate Drawing", run: drawing::duplicate },
    Command { id: "drawing.rename", title: "Rename Drawing", run: drawing::rename },
    Command { id: "drawing.delete", title: "Delete Drawing", run: drawing::delete },
    Command { id: "drawing.stroke", title: "Brush Stroke", run: drawing::stroke },
    Command { id: "drawing.mix", title: "Mixer Stroke", run: drawing::mix },
    Command { id: "drawing.vectorStroke", title: "Vector Stroke", run: drawing::vector_stroke },
    Command { id: "drawing.scatter", title: "Scatter", run: drawing::scatter },
    Command { id: "text.add", title: "New Text", run: text::add },
    Command { id: "text.type", title: "Type Text", run: text::type_text },
    Command { id: "text.setFormat", title: "Format Text", run: text::set_format },
    Command { id: "text.setBox", title: "Change Text Box", run: text::set_box },
    Command { id: "text.setKerning", title: "Kern Letters", run: text::set_kerning },
    Command { id: "text.delete", title: "Delete Text", run: text::delete },
    Command { id: "text.toShapes", title: "Convert Text to Shapes", run: text::to_shapes },
    Command { id: "drawing.vectorErase", title: "Vector Eraser Stroke", run: drawing::vector_erase },
    Command { id: "drawing.editShapes", title: "Edit Points", run: drawing::edit_shapes },
    Command { id: "drawing.layBrush", title: "Lay Brush", run: drawing::lay_brush },
    Command { id: "drawing.splitPiece", title: "Split Line", run: drawing::split_piece },
    Command { id: "drawing.vectorize", title: "Make Vector Art", run: drawing::vectorize },
    Command { id: "drawing.rasterize", title: "Make Pixels", run: drawing::rasterize },
    Command { id: "drawing.shape", title: "Draw Shape", run: drawing::shape },
    Command { id: "drawing.fill", title: "Fill", run: drawing::fill },
    Command { id: "drawing.recolour", title: "Recolour", run: drawing::recolour },
    Command { id: "drawing.place", title: "Place Gradient", run: drawing::place },
    Command { id: "drawing.copyPlacements", title: "Copy Gradient Placements", run: drawing::copy_placements },
    Command { id: "drawing.clear", title: "Clear Selection", run: select::clear },
    Command { id: "drawing.recolourSelection", title: "Recolour Selection", run: select::recolour },
    Command { id: "drawing.mergeSelection", title: "Merge Shapes", run: select::merge },
    Command { id: "drawing.transform", title: "Transform Selection", run: select::transform },
    Command { id: "drawing.cutAlong", title: "Cut", run: select::cut_along },
    Command { id: "drawing.cutFlick", title: "Cut", run: select::cut_flick },
    Command { id: "drawing.copy", title: "Copy", run: select::copy },
    Command { id: "drawing.cut", title: "Cut", run: select::cut },
    Command { id: "drawing.paste", title: "Paste", run: select::paste },
    Command { id: "exposure.set", title: "Set Exposure", run: drawing::set_exposure },
    Command { id: "exposure.insert", title: "Add Exposure", run: drawing::insert_exposure },
    Command { id: "exposure.remove", title: "Remove Exposure", run: drawing::remove_exposure },
    Command { id: "exposure.setHold", title: "Set Hold", run: drawing::set_hold },
    Command { id: "exposure.extend", title: "Extend Hold", run: drawing::extend_exposure },
    Command { id: "exposure.move", title: "Move Frames", run: drawing::move_exposure },
    Command { id: "exposure.copy", title: "Copy Frames", run: drawing::copy_exposure },
    Command { id: "exposure.paste", title: "Paste Frames", run: drawing::paste_exposure },
];

/// Frames are numbered below this, which keeps frame arithmetic far from overflow (about
/// eleven hours at 24 fps).
const MAX_FRAMES: u32 = 1_000_000;

/// A range of `length` frames from `frame`, as a command gives it.
fn checked_frames(frame: u32, length: u32) -> Result<()> {
    if length == 0 {
        return invalid("the length is at least one frame");
    }
    if frame >= MAX_FRAMES || length > MAX_FRAMES - frame {
        return invalid(format!("frames are numbered below {MAX_FRAMES}"));
    }
    Ok(())
}

/// A drawing pixel density as a command gives it: drawing pixels per shot pixel.
fn checked_density(density: f64) -> Result<f64> {
    if !(0.25..=8.0).contains(&density) {
        return invalid(format!("a drawing resolution of {density}× is outside 0.25× to 8×"));
    }
    Ok(density)
}

/// The largest shot side in pixels.
const MAX_SIDE: u32 = 32_768;

fn title(id: &str) -> &'static str {
    COMMANDS.iter().find(|c| c.id == id).map_or("", |c| c.title)
}

fn invalid<T>(why: impl Into<String>) -> Result<T> {
    Err(EngineError::Invalid(why.into()))
}

/// The shot a command names, or the project's first shot when it names none.
fn shot_id(project: &Project, shot: Option<u64>) -> Result<ShotId> {
    let id = shot.map(ShotId).or_else(|| project.shots.first().map(|s| s.id));
    match id.filter(|&id| project.shot(id).is_some()) {
        Some(id) => Ok(id),
        None => invalid(shot.map_or("the project has no shot".to_string(), |id| format!("no shot {id}"))),
    }
}

/// A shot whose drawings a command is about to read, change or delete must be loaded: one that
/// is not has no pixels in memory, only in the project file.
fn loaded(shot: &Shot) -> Result<()> {
    if !shot.loaded {
        return invalid(format!("the drawings of `{}` are not loaded; load the shot first (shot.load)", shot.name));
    }
    Ok(())
}

/// A name for a node or drawing as typed: trimmed, not empty, and without `/`, which separates
/// the names in a path (plan §4.12).
fn checked_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return invalid("a name can't be empty");
    }
    if name.contains('/') {
        return invalid(format!("the name `{name}` can't contain `/`"));
    }
    Ok(name)
}

/// No two of a shot's nodes share a name (plan §4.12): `name` has to be free in `shot`, or be
/// the name that the node with the id `own` has already.
fn name_is_free(shot: &Shot, name: &str, own: Option<u64>) -> Result<()> {
    let layers = shot.layers.iter().map(|l| ("a layer", l.id.0, &l.name));
    let pegs = shot.pegs.iter().map(|p| ("a peg", p.id.0, &p.name));
    let composites = shot.composites.iter().map(|c| ("a composite", c.id.0, &c.name));
    let composites = composites.chain(shot.deformers.iter().map(|d| ("a deformer", d.id.0, &d.name)));
    let composites = composites.chain(shot.groups.iter().map(|g| ("a group", g.id.0, &g.name)));
    let composites = composites.chain(shot.effects.iter().map(|e| ("an effect", e.id.0, &e.name)));
    let composites = composites.chain(shot.displays.iter().map(|d| ("a display", d.id.0, &d.name)));
    match layers.chain(pegs).chain(composites).find(|(_, id, taken)| Some(*id) != own && *taken == name) {
        Some((kind, ..)) => invalid(format!("the shot already has {kind} named `{name}`")),
        None => Ok(()),
    }
}

/// Parse a command's parameters. A command called without parameters gets `{}`.
fn parse<T: DeserializeOwned>(id: &'static str, params: Value) -> Result<T> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).map_err(|source| EngineError::BadParams { id, source })
}

/// Every command, and after them those scripts add, each with its script's file.
fn app_commands(s: &mut Session, _: Value) -> Result<Value> {
    let built_in = COMMANDS.iter().map(|c| json!({ "id": c.id, "title": c.title }));
    let scripts = s.script_commands().iter().map(|c| json!({ "id": c.id, "title": c.title, "script": c.file }));
    Ok(built_in.chain(scripts).collect())
}

fn edit_undo(s: &mut Session, _: Value) -> Result<Value> {
    Ok(s.undo().into())
}

fn edit_redo(s: &mut Session, _: Value) -> Result<Value> {
    Ok(s.redo().into())
}

fn edit_history(s: &mut Session, _: Value) -> Result<Value> {
    Ok(json!(s.history().entries()))
}

/// Runs several commands as one undo step: `commands`, each an `id` with its `params`, in
/// their order. If one of them is turned down, none of them is done. The step is named
/// `title`, or as the steps of the commands were if they were all named alike. Commands that are no edits of
/// the project (`file.`, `edit.`) can't be among them. Returns what each returned.
fn edit_batch(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct One {
        id: String,
        #[serde(default)]
        params: Value,
    }
    #[derive(Deserialize)]
    struct Params {
        commands: Vec<One>,
        title: Option<String>,
    }
    let Params { commands: asked, title } = parse("edit.batch", params)?;
    if asked.is_empty() {
        return invalid("give the `commands` to run, each an `id` with its `params`");
    }
    if let Some(one) = asked.iter().find(|one| one.id.starts_with("file.") || one.id.starts_with("edit.")) {
        return invalid(format!("`{}` is no edit of the project: it can't be one of several", one.id));
    }
    let results = s.batch(|s| {
        let (mut results, mut labels) = (Vec::new(), Vec::new());
        for one in asked {
            let steps = s.history().entries().len();
            results.push(s.execute(&one.id, one.params)?);
            // What a command that changed something called its step.
            labels.extend(s.history().undo_label().filter(|_| s.history().entries().len() > steps).map(str::to_string));
        }
        let alike = labels.first().filter(|first| labels.iter().all(|label| label == *first)).cloned();
        Ok((results, title.or(alike).unwrap_or_else(|| "Several Changes".to_string())))
    })?;
    Ok(json!({ "results": results }))
}

/// Runs a script (plan §9) with the session's script engine, which the app and the tools that
/// run scripts give it.
fn script_run(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        code: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        in_hand: Value,
        #[serde(default)]
        write_files: bool,
        #[serde(default)]
        answers: Vec<Value>,
    }
    let Params { code, name, in_hand, write_files, answers } = parse("script.run", params)?;
    let Some(runner) = s.script else { return invalid("this session has no script engine") };
    runner(s, &crate::ScriptRequest { code, name: name.unwrap_or_else(|| "Script".to_string()), in_hand, write_files, command: None, answers })
}

fn edit_goto_history(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        index: usize,
    }
    let Params { index } = parse("edit.gotoHistory", params)?;
    s.typing = None;
    if !s.history.goto(index) {
        return Err(EngineError::Invalid(format!("no history state {index}")));
    }
    Ok(Value::Null)
}

fn project_get(s: &mut Session, _: Value) -> Result<Value> {
    Ok(json!(**s.project()))
}

fn project_set_resolution(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        width: u32,
        height: u32,
    }
    let id = "project.setResolution";
    let Params { width, height } = parse(id, params)?;
    if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
        return Err(EngineError::Invalid(format!("resolution {width}×{height} is outside 1–{MAX_SIDE} pixels a side")));
    }
    s.edit(title(id), |p| {
        p.settings.width = width;
        p.settings.height = height;
        Ok(())
    })?;
    Ok(Value::Null)
}

fn project_set_frame_rate(s: &mut Session, params: Value) -> Result<Value> {
    /// Frames per second as the exact fraction `num / den` (23.976 is 24000 / 1001).
    #[derive(Deserialize)]
    struct Params {
        num: i64,
        #[serde(default = "one")]
        den: i64,
    }
    fn one() -> i64 {
        1
    }
    let id = "project.setFrameRate";
    let Params { num, den } = parse(id, params)?;
    if num < 1 || den < 1 {
        return Err(EngineError::Invalid(format!("frame rate {num}/{den} must be positive")));
    }
    s.edit(title(id), |p| {
        p.settings.frame_rate = FrameRate::new(num, den);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn project_set_working_space(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        space: WorkingSpace,
    }
    let id = "project.setWorkingSpace";
    let Params { space } = parse(id, params)?;
    s.edit(title(id), |p| {
        p.settings.working_space = space;
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets the master volume: how loud all of the project's sound is, played and in a movie. 1
/// is each clip as loud as it says, 0 silent.
fn project_set_volume(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        volume: f32,
    }
    let id = "project.setVolume";
    let Params { volume } = parse(id, params)?;
    if !(0.0..=8.0).contains(&volume) {
        return invalid("the volume is outside 0 to 8");
    }
    s.edit(title(id), |p| {
        p.settings.volume = volume;
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Sets the resolution new layers get. Existing layers keep theirs.
fn project_set_drawing_density(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        density: f64,
    }
    let id = "project.setDrawingDensity";
    let Params { density } = parse(id, params)?;
    let density = checked_density(density)?;
    s.edit(title(id), |p| {
        p.settings.drawing_density = density;
        Ok(())
    })?;
    Ok(Value::Null)
}
