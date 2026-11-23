//! The app's commands (plan §7.6): everything the user can ask for, each with a stable id, a
//! title and its place in the menus. The menu bar, the shortcuts and the command palette are
//! all made from this one list, so a new command needs no other UI code to be reachable.
//!
//! Where a command is an engine command given what is current (the layer, the frame, the
//! selection), it has the engine command's id and title.

use egui::{Context, Pos2};
use iskra_model::{ArtLayerKind, Shape};
use serde_json::{Value, json};

use crate::dock::{DockNode, Page, Panel};
use crate::prefs::Guide;
use crate::view::View;
use crate::viewport::Tool;
use crate::{IskraApp, Turn};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    File,
    Edit,
    Drawing,
    Animation,
    View,
    Play,
    Storyboard,
    Scripts,
    Workspace,
}

impl Menu {
    pub const ALL: [Menu; 9] =
        [Menu::File, Menu::Edit, Menu::Drawing, Menu::Animation, Menu::View, Menu::Play, Menu::Storyboard, Menu::Scripts, Menu::Workspace];

    pub fn title(self) -> &'static str {
        match self {
            Menu::File => "File",
            Menu::Edit => "Edit",
            Menu::Drawing => "Drawing",
            Menu::Animation => "Animation",
            Menu::View => "View",
            Menu::Play => "Play",
            Menu::Storyboard => "Storyboard",
            Menu::Scripts => "Scripts",
            Menu::Workspace => "Workspace",
        }
    }
}

pub struct AppCommand {
    pub id: &'static str,
    /// The name in menus and in the palette. Empty for the engine command's own title.
    title: &'static str,
    pub menu: Menu,
    /// Starts a new group in its menu: a line is drawn above it.
    pub group: bool,
    pub run: fn(&mut IskraApp, &Context),
    /// Whether it can run now. A command that can't is greyed out and its shortcut does nothing.
    pub enabled: fn(&IskraApp) -> bool,
    /// For a command that switches something on and off: whether it is on.
    pub checked: Option<fn(&IskraApp) -> bool>,
}

impl AppCommand {
    pub fn title(&self) -> &'static str {
        if !self.title.is_empty() {
            return self.title;
        }
        iskra_engine::commands().iter().find(|c| c.id == self.id).map_or(self.id, |c| c.title)
    }
}

fn always(_: &IskraApp) -> bool {
    true
}

fn has_shot(app: &IskraApp) -> bool {
    app.shot.is_some()
}

fn has_layer(app: &IskraApp) -> bool {
    app.layer.is_some()
}

fn has_drawings(app: &IskraApp) -> bool {
    app.layer.and_then(|id| app.session.project().layer(id)).is_some_and(|layer| layer.drawings.len() > 1)
}

/// The art layer in hand is swatch-linked or vector art, which what works on swatches alone
/// needs: not plain colour.
fn swatch_linked(app: &IskraApp) -> bool {
    app.swatch_only().is_none()
}

fn exposed(app: &IskraApp) -> bool {
    app.layer.and_then(|id| app.session.project().layer(id)?.exposure.at(app.tools.frame)).is_some()
}

/// The playhead is on an empty cell of the layer in hand, with a drawing before it.
fn extendable(app: &IskraApp) -> bool {
    !app.layers_to_hold().is_empty()
}

/// Hold to Here on each layer it takes, as one undo step.
fn extend_holds(app: &mut IskraApp) {
    let frame = app.tools.frame;
    let commands: Vec<Value> =
        app.layers_to_hold().iter().map(|layer| json!({ "id": "exposure.extend", "params": { "layer": layer.0, "frame": frame } })).collect();
    match commands.as_slice() {
        [one] => app.run("exposure.extend", one["params"].clone()),
        _ => app.run("edit.batch", json!({ "commands": commands, "title": "Extend Holds" })),
    }
}

/// Run an engine command on the current cell: the current layer and frame, plus `extra`.
fn on_cell(app: &mut IskraApp, id: &str, extra: Value) {
    let Some(layer) = app.layer else { return };
    let mut params = json!({ "layer": layer.0, "frame": app.tools.frame });
    if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
        params.extend(extra.clone());
    }
    app.run(id, params);
}

fn step(app: &mut IskraApp, by: i64) {
    app.set_playhead((i64::from(app.playhead()) + by).max(0) as u32);
    app.playing = None;
}

macro_rules! command {
    ($id:literal, $title:literal, $menu:ident, $run:expr) => {
        command!($id, $title, $menu, $run, always)
    };
    ($id:literal, $title:literal, $menu:ident, $run:expr, $enabled:expr) => {
        AppCommand { id: $id, title: $title, menu: Menu::$menu, group: false, run: $run, enabled: $enabled, checked: None }
    };
}

/// The command that shows or hides a panel; its title is the panel's.
macro_rules! panel {
    ($id:literal, $panel:ident) => {
        AppCommand {
            id: $id,
            title: Panel::$panel.title_const(),
            menu: Menu::Workspace,
            group: false,
            run: |app, _| app.toggle_panel(Panel::$panel),
            enabled: always,
            checked: Some(|app| app.prefs.layouts.of(app.page).shows(Panel::$panel) || app.prefs.detached.contains(&Panel::$panel)),
        }
    };
}

/// The command that shows or hides a guide over the camera's frame.
macro_rules! guide {
    ($id:literal, $title:literal, $guide:ident, $group:literal) => {
        AppCommand {
            id: $id,
            title: $title,
            menu: Menu::View,
            group: $group,
            run: |app, _| match app.prefs.guides.iter().position(|guide| *guide == Guide::$guide) {
                Some(at) => drop(app.prefs.guides.remove(at)),
                None => app.prefs.guides.push(Guide::$guide),
            },
            enabled: always,
            checked: Some(|app| app.prefs.guides.contains(&Guide::$guide)),
        }
    };
}

/// A command that starts a new group in its menu.
const fn group(command: AppCommand) -> AppCommand {
    AppCommand { group: true, ..command }
}

/// A command that switches something on and off.
const fn toggle(command: AppCommand, checked: fn(&IskraApp) -> bool) -> AppCommand {
    AppCommand { checked: Some(checked), ..command }
}

/// Every command, in menu order.
pub fn commands() -> &'static [AppCommand] {
    COMMANDS
}

pub fn command(id: &str) -> Option<&'static AppCommand> {
    COMMANDS.iter().find(|c| c.id == id)
}

const COMMANDS: &[AppCommand] = &[
    command!("file.new", "New", File, |app, _| app.new_project()),
    command!("file.open", "Open…", File, |app, _| app.open_project()),
    group(command!("file.save", "", File, |app, _| app.save_project(false))),
    command!("file.saveAs", "Save As…", File, |app, _| app.save_project(true)),
    group(command!("media.import", "Import Media…", File, |app, _| app.ask_import_media())),
    command!("character.import", "Import Character…", File, |app, _| app.ask_import_character(), |app| app.shot.is_some()),
    command!("skin.import", "Import Skins…", File, |app, _| app.ask_import_skins(), |app| app.shot.is_some()),
    command!("rig.import", "Import Rig…", File, |app, _| app.ask_import_rig(), |app| app.shot.is_some()),
    group(command!(
        "layer.refreshMedia",
        "Read the Layer's Pictures Again",
        File,
        |app, _| {
            if let Some(layer) = app.layer {
                app.run("layer.refreshMedia", json!({ "layer": layer.0 }));
            }
        },
        |app| app.shot().zip(app.layer).is_some_and(|(shot, layer)| shot.layer(layer).is_some_and(|layer| layer.media.is_some()))
    )),
    command!(
        "skin.fromFolder",
        "Add Skin From Folder…",
        File,
        |app, _| {
            if let (Some(layer), Some(folder)) = (app.layer, crate::dialogs::skin_folder()) {
                app.skin_from_folder(layer, &folder);
            }
        },
        |app| app.shot().zip(app.layer).is_some_and(|(shot, layer)| shot.layer(layer).is_some_and(|layer| layer.media.is_some()))
    ),
    group(command!("storyboard.exportPdf", "Export Storyboard PDF…", File, |app, _| app.print.open = true, |app| app.storyboard.is_some())),
    group(command!("app.commandPalette", "Command Palette…", File, |app, _| app.palette.open())),
    group(command!("app.settings", "Settings…", File, |app, _| app.settings = Some(app.settings.unwrap_or(crate::settings::Section::General)))),
    command!("app.about", "About Iskra", File, |app, _| app.about_open = true),
    command!("app.quit", "Quit", File, |app, ctx| app.quit(ctx)),
    command!("edit.undo", "", Edit, |app, _| app.run("edit.undo", Value::Null), |app| app.session.history().can_undo()),
    command!("edit.redo", "", Edit, |app, _| app.run("edit.redo", Value::Null), |app| app.session.history().can_redo()),
    group(command!("edit.cut", "Cut", Edit, |app, ctx| app.copy(ctx, "drawing.cut"), IskraApp::can_cut)),
    command!("edit.copy", "Copy", Edit, |app, ctx| app.copy(ctx, "drawing.copy"), IskraApp::can_copy),
    command!("edit.paste", "Paste", Edit, |app, _| app.paste(), IskraApp::can_paste),
    command!("edit.duplicate", "Duplicate", Edit, |app, _| app.duplicate(), |app| !app.nodes_to_copy().is_empty()),
    command!("edit.delete", "Delete", Edit, |app, _| app.delete_selection(), IskraApp::can_delete),
    group(command!("edit.selectAll", "Select All", Edit, |app, _| app.select_all(), exposed)),
    command!("edit.deselect", "Deselect", Edit, |app, _| app.deselect(), IskraApp::can_deselect),
    command!("edit.invertSelection", "Invert Selection", Edit, |app, _| app.invert_selection(), |app| app.viewport.selection.is_some()),
    group(command!("edit.flipHorizontally", "Flip Horizontally", Edit, |app, _| app.turn_selection(Turn::Across), |app| app.viewport.selection.is_some())),
    command!("edit.flipVertically", "Flip Vertically", Edit, |app, _| app.turn_selection(Turn::Up), |app| app.viewport.selection.is_some()),
    command!("edit.rotateLeft", "Rotate 90° Left", Edit, |app, _| app.turn_selection(Turn::Left), |app| app.viewport.selection.is_some()),
    command!("edit.rotateRight", "Rotate 90° Right", Edit, |app, _| app.turn_selection(Turn::Right), |app| app.viewport.selection.is_some()),
    command!("edit.mergeShapes", "Merge Down", Edit, |app, _| app.on_selection("drawing.mergeSelection"), IskraApp::can_merge),
    command!("tool.brush", "Brush", Drawing, |app, _| app.tools.tool = Tool::Brush),
    command!("tool.eraser", "Eraser", Drawing, |app, _| app.tools.tool = Tool::Eraser),
    command!("tool.fill", "Paint Bucket", Drawing, |app, _| app.tools.tool = Tool::Fill),
    command!("tool.eyedropper", "Eyedropper", Drawing, |app, _| app.take_tool(Tool::Eyedropper)),
    command!("tool.gradient", "Gradient/Texture", Drawing, |app, _| app.tools.tool = Tool::Gradient),
    command!("tool.line", "Line", Drawing, |app, _| (app.tools.tool, app.tools.shape) = (Tool::Shape, Shape::Line)),
    command!("tool.rectangle", "Rectangle", Drawing, |app, _| (app.tools.tool, app.tools.shape) = (Tool::Shape, Shape::Rectangle)),
    command!("tool.ellipse", "Ellipse", Drawing, |app, _| (app.tools.tool, app.tools.shape) = (Tool::Shape, Shape::Ellipse)),
    command!("tool.polyline", "Polyline", Drawing, |app, _| (app.tools.tool, app.tools.shape) = (Tool::Shape, Shape::Polyline)),
    command!("tool.triangle", "Triangle", Drawing, |app, _| (app.tools.tool, app.tools.shape) = (Tool::Shape, Shape::Triangle)),
    command!("tool.star", "Star", Drawing, |app, _| (app.tools.tool, app.tools.shape) = (Tool::Shape, Shape::Star)),
    command!("tool.arrow", "Arrow", Drawing, |app, _| (app.tools.tool, app.tools.shape) = (Tool::Shape, Shape::Arrow)),
    command!("tool.spiral", "Spiral", Drawing, |app, _| (app.tools.tool, app.tools.shape) = (Tool::Shape, Shape::Spiral)),
    command!("tool.grid", "Grid", Drawing, |app, _| (app.tools.tool, app.tools.shape) = (Tool::Shape, Shape::Grid)),
    command!("tool.select", "Select", Drawing, |app, _| (app.tools.tool, app.tools.lasso) = (Tool::Select, false)),
    command!("tool.lasso", "Lasso", Drawing, |app, _| (app.tools.tool, app.tools.lasso) = (Tool::Select, true)),
    command!("tool.cutter", "Cutter", Drawing, |app, _| app.tools.tool = Tool::Cutter),
    command!("tool.transform", "Transform", Drawing, |app, _| app.tools.tool = Tool::Transform),
    command!("tool.camera", "Camera", Drawing, |app, _| app.take_tool(Tool::Camera)),
    command!("tool.bones", "Bones", Drawing, |app, _| app.tools.tool = Tool::Bones),
    command!("tool.points", "Point Editor", Drawing, |app, _| app.tools.tool = Tool::Points),
    command!("tool.text", "Text", Drawing, |app, _| app.take_tool(Tool::Text)),
    command!("text.convertToShapes", "Convert Text to Shapes", Drawing, |app, _| app.text_to_shapes(), IskraApp::has_text),
    command!("tool.zoom", "Zoom", Drawing, |app, _| app.tools.tool = Tool::Zoom),
    command!("drawing.makeVector", "Make Vector Layer…", Drawing, |app, _| app.set_vector_art(true), |app| has_layer(app) && !app.vector_art()),
    command!("drawing.makePixels", "Make Bitmap Layer", Drawing, |app, _| app.set_vector_art(false), IskraApp::has_vector_art),
    toggle(group(command!("art.lineArt", "Draw on Line Art", Drawing, |app, _| app.draw_on(ArtLayerKind::LineArt))), |app| {
        app.tools.art == ArtLayerKind::LineArt
    }),
    toggle(command!("art.colourArt", "Draw on Colour Art", Drawing, |app, _| app.draw_on(ArtLayerKind::ColourArt)), |app| {
        app.tools.art == ArtLayerKind::ColourArt
    }),
    toggle(command!("art.overart", "Draw on Overart", Drawing, |app, _| app.draw_on(ArtLayerKind::Overart)), |app| app.tools.art == ArtLayerKind::Overart),
    toggle(command!("art.vector", "Vector Layer", Drawing, |app, _| app.set_vector_art(!app.vector_art()), has_layer), |app| app.vector_art()),
    toggle(command!("art.mixer", "Mixer Brush, on Free Colour", Drawing, |app, _| app.tools.mixing.on = !app.tools.mixing.on), |app| app.tools.mixing.on),
    command!("art.toggle", "Switch Between Line Art and Colour Art", Drawing, |app, _| {
        app.tools.art = if app.tools.art == ArtLayerKind::LineArt { ArtLayerKind::ColourArt } else { ArtLayerKind::LineArt };
    }),
    group(command!("guide.addRuler", "New Ruler", Drawing, |app, _| app.add_guide(json!({ "kind": "ruler" })), has_shot)),
    command!("guide.addPerspective1", "New 1-Point Perspective", Drawing, |app, _| app.add_guide(json!({ "kind": "perspective", "points": 1 })), has_shot),
    command!("guide.addPerspective2", "New 2-Point Perspective", Drawing, |app, _| app.add_guide(json!({ "kind": "perspective", "points": 2 })), has_shot),
    command!("guide.addPerspective3", "New 3-Point Perspective", Drawing, |app, _| app.add_guide(json!({ "kind": "perspective", "points": 3 })), has_shot),
    command!("guide.putAway", "Put the Drawing Guide Away", Drawing, |app, _| app.viewport.guide = None, |app| app.guide_in_hand().is_some()),
    group(command!("palette.add", "", Drawing, |app, _| app.add_swatch())),
    command!("palette.remove", "", Drawing, |app, _| app.remove_swatch(), |app| app.session.project().palette.swatches.len() > 1),
    command!("palette.removeUnused", "", Drawing, |app, _| app.remove_unused_swatches()),
    command!("palette.nextSwatch", "Next Swatch", Drawing, |app, _| app.step_swatch(1)),
    command!("palette.previousSwatch", "Previous Swatch", Drawing, |app, _| app.step_swatch(-1)),
    toggle(command!("palette.protect", "Protect the Swatch in Hand", Drawing, |app, _| app.protect_swatch(), swatch_linked), |app| {
        app.viewport.protect.contains(&app.tools.swatch)
    }),
    command!("palette.select", "Select What the Swatch in Hand Painted", Drawing, |app, _| app.select_swatch(), |app| exposed(app) && swatch_linked(app)),
    command!("palette.addStyle", "", Drawing, |app, _| app.add_style(), has_shot),
    command!("palette.new", "", Drawing, |app, _| app.add_palette()),
    command!("palette.import", "Import Palette…", Drawing, |app, _| {
        if let Some(paths) = crate::dialogs::import_palettes(app.palettes_folder().as_deref(), "Import Palette") {
            app.import_palettes(&paths, crate::PaletteImport::New);
        }
    }),
    command!("palette.importInto", "Import Swatches into the Palette in Hand…", Drawing, |app, _| {
        if let Some(paths) = crate::dialogs::import_palettes(app.palettes_folder().as_deref(), "Import Swatches") {
            app.import_palettes(&paths, crate::PaletteImport::Into);
        }
    }),
    command!("palette.update", "Update the Palette in Hand from a File…", Drawing, |app, _| {
        if let Some(paths) = crate::dialogs::import_palettes(app.palettes_folder().as_deref(), "Update the Palette") {
            app.import_palettes(&paths, crate::PaletteImport::Update);
        }
    }),
    command!("palette.export", "Export the Palette in Hand…", Drawing, |app, _| {
        let name = app.session.project().palette.set(app.palette_in_hand()).map_or(app.name(), |set| set.name.clone());
        if let Some(path) = crate::dialogs::export_palette(app.palettes_folder().as_deref(), &name) {
            app.run("palette.export", json!({ "path": path, "palette": app.palette_in_hand().0 }));
        }
    }),
    command!("palette.exportAll", "Export Every Palette as One File…", Drawing, |app, _| {
        if let Some(path) = crate::dialogs::export_palette(app.palettes_folder().as_deref(), &app.name()) {
            app.run("palette.export", json!({ "path": path }));
        }
    }),
    // A brush preset is taken by going through the list, or by the slot it was given in the
    // Brushes panel: each of these can have a shortcut.
    group(command!("brush.next", "Next Brush", Drawing, |app, _| app.step_brush(1))),
    command!("brush.previous", "Previous Brush", Drawing, |app, _| app.step_brush(-1)),
    command!("brush.slot1", "Brush in Slot 1", Drawing, |app, _| app.take_brush_slot(0), |app| app.brushes.in_slot(0).is_some()),
    command!("brush.slot2", "Brush in Slot 2", Drawing, |app, _| app.take_brush_slot(1), |app| app.brushes.in_slot(1).is_some()),
    command!("brush.slot3", "Brush in Slot 3", Drawing, |app, _| app.take_brush_slot(2), |app| app.brushes.in_slot(2).is_some()),
    command!("brush.slot4", "Brush in Slot 4", Drawing, |app, _| app.take_brush_slot(3), |app| app.brushes.in_slot(3).is_some()),
    command!("brush.slot5", "Brush in Slot 5", Drawing, |app, _| app.take_brush_slot(4), |app| app.brushes.in_slot(4).is_some()),
    group(command!("brush.saveAsPreset", "Save Brush as Preset…", Drawing, |app, _| app.save_brush_as_preset())),
    command!("brush.restore", "Restore the Engine's Brushes", Drawing, |app, _| app.brushes.restore_builtin(), |app| !app.brushes.hidden.is_empty()),
    command!("brush.import", "Import Brushes…", Drawing, |app, _| {
        if let Some(path) = crate::dialogs::import_brushes() {
            app.import_brushes(&path);
        }
    }),
    command!(
        "brush.export",
        "Export Brushes…",
        Drawing,
        |app, _| {
            if let Some(path) = crate::dialogs::export_brushes("My Brushes") {
                let own = app.brushes.presets.clone();
                app.export_brushes(&path, &own, false);
            }
        },
        |app| !app.brushes.presets.is_empty()
    ),
    command!(
        "brush.exportOld",
        "Export Brushes for Older Programs…",
        Drawing,
        |app, _| {
            if let Some(path) = crate::dialogs::export_old_brushes("My Brushes") {
                let own = app.brushes.presets.clone();
                app.export_brushes(&path, &own, true);
            }
        },
        |app| !app.brushes.presets.is_empty()
    ),
    group(command!("drawing.newDrawing", "", Drawing, |app, _| on_cell(app, "drawing.newDrawing", json!({})), has_layer)),
    command!("drawing.duplicate", "", Drawing, |app, _| on_cell(app, "drawing.duplicate", json!({})), exposed),
    group(command!("exposure.insert", "Hold One Frame Longer", Drawing, |app, _| on_cell(app, "exposure.insert", json!({})), has_layer)),
    command!("exposure.remove", "Take This Frame Out", Drawing, |app, _| on_cell(app, "exposure.remove", json!({})), has_layer),
    command!("exposure.extend", "Extend the Hold to the Playhead", Drawing, |app, _| extend_holds(app), extendable),
    command!("exposure.clear", "Clear Cell", Drawing, |app, _| on_cell(app, "exposure.set", json!({ "drawing": null })), exposed),
    group(command!("substitution.next", "Swap to the Next Drawing", Drawing, |app, _| app.cycle_drawing(true), has_drawings)),
    command!("substitution.previous", "Swap to the Previous Drawing", Drawing, |app, _| app.cycle_drawing(false), has_drawings),
    group(command!("layer.new", "", Drawing, |app, _| app.new_layer(None), has_shot)),
    command!("layer.newBitmap", "New Bitmap Layer", Drawing, |app, _| app.new_layer(Some("bitmap")), has_shot),
    command!("layer.newVector", "New Vector Layer", Drawing, |app, _| app.new_layer(Some("vector")), has_shot),
    command!("layer.newText", "New Text Layer", Drawing, |app, _| app.new_layer(Some("text")), has_shot),
    command!("peg.addParent", "Add Parent Peg", Animation, |app, _| app.add_parent_peg(), |app| app.peg.is_some() || app.layer.is_some()),
    command!("peg.new", "", Animation, |app, _| app.new_peg(), has_shot),
    command!("peg.delete", "", Animation, |app, _| app.delete_peg(), |app| app.peg.is_some()),
    command!("node.deletePicked", "Delete Picked Nodes", Animation, |app, _| app.run_several(app.picked_deletes_ungrouping()), |app| !app.picked.is_empty()),
    command!("node.deletePickedWithAll", "Delete Picked Nodes with All in Them", Animation, |app, _| app.run_several(app.picked_deletes()), |app| !app
        .picked_deletes()
        .is_empty()),
    group(command!("key.add", "Add Keyframe", Animation, |app, _| app.add_key(false), |app| app.keyed_frames().is_some() && !app.on_key())),
    command!("key.remove", "Remove Keyframe", Animation, |app, _| app.add_key(true), IskraApp::on_key),
    command!("key.all", "Key All", Animation, |app, _| app.key_all(), |app| !app.key_all_commands().is_empty()),
    command!("key.next", "Go to Next Keyframe", Animation, |app, _| app.go_to_key(true), |app| app.keyed_frames().is_some()),
    command!("key.previous", "Go to Previous Keyframe", Animation, |app, _| app.go_to_key(false), |app| app.keyed_frames().is_some()),
    group(command!("key.linear", "Keyframe: Linear", Animation, |app, _| app.ease(json!({ "into": "linear", "out": "linear" })), IskraApp::can_ease)),
    command!("key.slowIn", "Keyframe: Slow In", Animation, |app, _| app.ease(json!({ "into": "ease" })), IskraApp::can_ease),
    command!("key.slowOut", "Keyframe: Slow Out", Animation, |app, _| app.ease(json!({ "out": "ease" })), IskraApp::can_ease),
    command!("key.slowInOut", "Keyframe: Slow In and Out", Animation, |app, _| app.ease(json!({ "into": "ease", "out": "ease" })), IskraApp::can_ease),
    command!("key.hold", "Keyframe: Hold", Animation, |app, _| app.ease(json!({ "out": "hold" })), IskraApp::can_ease),
    group(command!("key.stepOnes", "Hold Keyframes on Ones", Animation, |app, _| app.step_keys(1), IskraApp::can_step)),
    command!("key.stepTwos", "Hold Keyframes on Twos", Animation, |app, _| app.step_keys(2), IskraApp::can_step),
    command!("key.stepThrees", "Hold Keyframes on Threes", Animation, |app, _| app.step_keys(3), IskraApp::can_step),
    command!("timeline.showSelected", "Show Selected", Animation, |app, _| app.show_selected(), |app| app.peg.is_some() || app.layer.is_some()),
    command!("key.switchHeld", "Keyframe: Switch Between Held and Animated", Animation, |app, _| app.switch_held(), |app| !app.keys_to_switch().is_empty()),
    group(command!(
        "key.smooth",
        "Path: Smooth Through Keyframe",
        Animation,
        |app, _| app.key("peg.setBend", json!({ "smooth": true })),
        IskraApp::on_path_key
    )),
    command!(
        "key.corner",
        "Path: Corner at Keyframe",
        Animation,
        |app, _| app.key("peg.setBend", json!({ "into": [0.0, 0.0], "out": [0.0, 0.0] })),
        IskraApp::on_path_key
    ),
    toggle(group(command!("animate.toggle", "Animate: Key What Is Moved", Animation, |app, _| app.toggle_animate())), |app| app.animate()),
    group(command!("node.switchOn", "Switch the Picked Nodes On", Animation, |app, _| app.switch_picked(true), |app| app.picked_switch(true).is_some())),
    command!("node.switchOff", "Switch the Picked Nodes Off", Animation, |app, _| app.switch_picked(false), |app| app.picked_switch(false).is_some()),
    group(command!("note.write", "Write a Retake Note…", Animation, |app, _| app.write_note(), has_shot)),
    group(command!("camera.addPeg", "Add Camera Peg", Animation, |app, _| app.add_camera_peg(), has_shot)),
    group(command!("library.save", "Save to Library…", Animation, |app, _| app.save_selected_to_library(), |app| app.shot.is_some()
        && (app.peg.is_some() || app.layer.is_some()))),
    command!("library.addFolder", "Add Library Folder…", Animation, |app, _| app.add_library_folder()),
    command!("view.zoomIn", "Zoom In", View, |app, _| app.viewport.view.step_about(Pos2::ZERO, Pos2::ZERO, true)),
    command!("view.zoomOut", "Zoom Out", View, |app, _| app.viewport.view.step_about(Pos2::ZERO, Pos2::ZERO, false)),
    command!("view.resetZoom", "Reset Zoom to 100%", View, |app, _| {
        let view = &mut app.viewport.view;
        view.zoom_about(Pos2::ZERO, Pos2::ZERO, 1.0 / view.zoom);
    }),
    command!("view.fit", "Fit Frame in View", View, |app, _| app.viewport.fit()),
    command!("view.reset", "Reset Zoom and Rotation", View, |app, _| app.viewport.view = View::default()),
    command!("view.rotateClockwise", "Rotate View 15° Clockwise", View, |app, _| app.viewport.view.rotation += 15f32.to_radians()),
    command!("view.rotateCounterclockwise", "Rotate View 15° Counterclockwise", View, |app, _| app.viewport.view.rotation -= 15f32.to_radians()),
    command!("view.resetRotation", "Reset View Rotation", View, |app, _| app.viewport.view.rotation = 0.0),
    toggle(group(command!("view.flat", "Flat View, Not Through the Camera", View, |app, _| app.viewport.flat = !app.viewport.flat)), |app| app.viewport.flat),
    toggle(command!("view.onCard", "Draw the Viewport on the Graphics Card", View, |app, _| app.prefs.viewport_on_card = !app.prefs.viewport_on_card), |app| {
        app.prefs.viewport_on_card
    }),
    toggle(command!("view.mixAtRest", "Show Transitions Mixed While Standing Still", View, |app, _| app.prefs.mix_at_rest = !app.prefs.mix_at_rest), |app| {
        app.prefs.mix_at_rest
    }),
    guide!("guide.actionSafe", "Guide: Action Safe", ActionSafe, true),
    guide!("guide.titleSafe", "Guide: Title Safe", TitleSafe, false),
    guide!("guide.thirds", "Guide: Rule of Thirds", Thirds, false),
    guide!("guide.centre", "Guide: Centre Cross", Centre, false),
    guide!("guide.fields", "Guide: 12 Fields", Fields, false),
    command!("guide.edit", "My Guides…", View, |app, _| app.editing_guides = true),
    toggle(group(command!("view.onionSkin", "Onion Skin", View, |app, _| app.display.onion = !app.display.onion)), |app| app.display.onion),
    toggle(command!("view.lightTable", "Light Table", View, |app, _| app.display.light_table = !app.display.light_table), |app| app.display.light_table),
    toggle(command!("view.follow", "Follow the Layer", View, |app, _| app.display.follow = !app.display.follow), |app| app.display.follow),
    toggle(
        command!("view.oneArtLayer", "Show the Art Layer in Hand Alone", View, |app, _| {
            app.display.shown_art = if app.display.shown_art.is_some() { None } else { Some(app.tools.art) }
        }),
        |app| app.display.shown_art.is_some(),
    ),
    toggle(command!("view.allArt", "Show All Art Layers", View, |app, _| app.display.shown_art = None), |app| app.display.shown_art.is_none()),
    toggle(command!("view.lineArtAlone", "Show the Line Art Alone", View, |app, _| app.alone(ArtLayerKind::LineArt)), |app| {
        app.display.shown_art == Some(ArtLayerKind::LineArt)
    }),
    toggle(command!("view.colourArtAlone", "Show the Colour Art Alone", View, |app, _| app.alone(ArtLayerKind::ColourArt)), |app| {
        app.display.shown_art == Some(ArtLayerKind::ColourArt)
    }),
    toggle(command!("view.overartAlone", "Show the Overart Alone", View, |app, _| app.alone(ArtLayerKind::Overart)), |app| {
        app.display.shown_art == Some(ArtLayerKind::Overart)
    }),
    toggle(
        command!("view.artAloneEverywhere", "Art Layer Alone in Every Drawing", View, |app, _| app.display.shown_art_all = !app.display.shown_art_all),
        |app| app.display.shown_art_all,
    ),
    toggle(command!("view.boards", "Boards as Reference", View, |app, _| app.prefs.hide_boards = !app.prefs.hide_boards), |app| !app.prefs.hide_boards),
    command!("playback.toggle", "Play or Stop", Play, |app, ctx| app.toggle_play(ctx)),
    toggle(
        command!("playback.loop", "Loop", Play, |app, _| match app.page {
            Page::Export => app.export_looping = !app.export_looping,
            _ => app.switches.looping = !app.switches.looping,
        }),
        IskraApp::looping,
    ),
    toggle(command!("playback.everyFrame", "Play Every Frame", Play, |app, _| app.prefs.play_every_frame = !app.prefs.play_every_frame), |app| {
        app.prefs.play_every_frame
    }),
    toggle(command!("playback.mute", "Mute the Export Page", Play, |app, _| app.muted = !app.muted), |app| app.muted),
    group(command!("shot.previousCut", "Go to the Previous Shot", Play, |app, ctx| app.step_shot(ctx, false), IskraApp::on_film)),
    command!("shot.nextCut", "Go to the Next Shot", Play, |app, ctx| app.step_shot(ctx, true), IskraApp::on_film),
    group(command!("frame.next", "Next Frame", Play, |app, _| step(app, 1))),
    command!("frame.previous", "Previous Frame", Play, |app, _| step(app, -1)),
    command!("storyboard.new", "", Storyboard, |app, _| app.new_storyboard()),
    group(command!("shot.new", "", Storyboard, |app, _| app.new_shot(), |app| app.storyboard.is_some())),
    command!("shot.duplicate", "", Storyboard, |app, _| app.duplicate_shot(), has_shot),
    command!("shot.delete", "", Storyboard, |app, _| app.delete_shot(), has_shot),
    group(command!("shot.split", "Split Shot at the Playhead", Storyboard, |app, _| app.split_shot(), has_shot)),
    command!("shot.merge", "Merge with the Next Shot", Storyboard, |app, _| app.merge_shot(), has_shot),
    command!("shot.moveEarlier", "Move Shot Earlier", Storyboard, |app, _| app.move_shot(false), has_shot),
    command!("shot.moveLater", "Move Shot Later", Storyboard, |app, _| app.move_shot(true), has_shot),
    group(command!("panel.new", "New Panel at the Playhead", Storyboard, |app, _| app.new_panel(), has_shot)),
    group(command!("marker.add", "Marker at the Playhead", Storyboard, |app, _| app.at_playhead("setMarker", "frame"), IskraApp::has_timeline)),
    command!("marker.next", "Go to the Next Marker", Storyboard, |app, _| app.go_to_marker(true), IskraApp::has_timeline),
    command!("marker.previous", "Go to the Previous Marker", Storyboard, |app, _| app.go_to_marker(false), IskraApp::has_timeline),
    command!("range.in", "Play from the Playhead: In Point", Storyboard, |app, _| app.at_playhead("setRange", "in"), IskraApp::has_timeline),
    command!("range.out", "Play up to the Playhead: Out Point", Storyboard, |app, _| app.at_playhead("setRange", "out"), IskraApp::has_timeline),
    command!("range.clear", "Clear In and Out Points", Storyboard, |app, _| app.on_timeline("setRange", json!({ "clear": true })), IskraApp::has_timeline),
    command!("storyboard.fillDialogue", "Dialogue from Transcripts", Storyboard, |app, _| app.fill_dialogue(), |app| app.storyboard.is_some()),
    group(command!("shot.sendOut", "Send Shot Out…", Storyboard, |app, _| app.ask_send_out(app.shot.into_iter().collect()), has_shot)),
    command!("shot.bringBack", "Bring Shot Back…", Storyboard, |app, _| app.ask_bring_back(None)),
    group(command!("shot.open", "Open Shot for Animation", Storyboard, |app, _| app.page = Page::Animation, has_shot)),
    toggle(command!("page.media", "Media Page", Workspace, |app, _| app.page = Page::Media), |app| app.page == Page::Media),
    toggle(command!("page.storyboard", "Storyboard Page", Workspace, |app, _| app.page = Page::Storyboard), |app| app.page == Page::Storyboard),
    toggle(command!("page.animation", "Animation Page", Workspace, |app, _| app.page = Page::Animation), |app| app.page == Page::Animation),
    toggle(command!("page.export", "Export Page", Workspace, |app, _| app.page = Page::Export), |app| app.page == Page::Export),
    command!("script.runFile", "Run Script File…", Scripts, |app, _| app.ask_run_script_file()),
    command!("script.console", "Script Console", Scripts, |app, _| app.show_panel(Panel::ScriptConsole)),
    group(command!("script.reload", "Reload Scripts", Scripts, |app, _| app.reload_scripts())),
    command!("script.showFolder", "Show Scripts Folder", Scripts, |app, _| app.show_scripts_folder(), |app| app.scripts_folder().is_some()),
    group(panel!("panel.viewport", Viewport)),
    panel!("panel.timeline", Timeline),
    panel!("panel.palette", Palette),
    panel!("panel.brushes", Brushes),
    panel!("panel.onionSkin", OnionSkin),
    panel!("panel.history", History),
    panel!("panel.bin", Bin),
    panel!("panel.storyboard", Storyboard),
    panel!("panel.layers", Layers),
    panel!("panel.properties", Properties),
    panel!("panel.toolProperties", ToolProperties),
    panel!("panel.curves", Curves),
    panel!("panel.substitution", Substitution),
    panel!("panel.nodeView", NodeView),
    panel!("panel.topView", TopView),
    panel!("panel.sideView", SideView),
    panel!("panel.library", Library),
    panel!("panel.pool", Pool),
    panel!("panel.storage", Storage),
    panel!("panel.panels", Panels),
    panel!("panel.log", Log),
    panel!("panel.scriptConsole", ScriptConsole),
    panel!("panel.notes", Notes),
    toggle(group(command!("workspace.viewOnly", "Show Only the Viewport", Workspace, |app, _| app.view_only = !app.view_only)), |app| app.view_only),
    command!("workspace.reset", "Reset Layout", Workspace, |app, _| {
        *app.prefs.layouts.of_mut(app.page) = DockNode::for_page(app.page);
        app.view_only = false;
    }),
    command!("workspace.save", "Save Workspace As…", Workspace, |app, _| app.naming_workspace = Some(String::new())),
];

/// What was chosen in the command palette: a command, or a brush preset by its group and name.
pub enum Picked {
    Command(&'static AppCommand),
    /// A command a script adds, by its id.
    Script(&'static str),
    Brush(String, String),
}

/// The command palette (plan §7.6): a search over every command, and over the brush presets,
/// which are taken by their names there.
#[derive(Default)]
pub struct Palette {
    /// What has been typed, while the palette is open.
    query: Option<String>,
    /// Which of the matching commands Enter would run.
    picked: usize,
}

impl Palette {
    pub fn open(&mut self) {
        (self.query, self.picked) = (Some(String::new()), 0);
    }

    /// The commands whose title has every word of `query`, in menu order.
    pub fn matches(query: &str) -> Vec<&'static AppCommand> {
        let query = query.to_lowercase();
        let has_all = |command: &&AppCommand| {
            let title = format!("{} {}", command.menu.title(), command.title()).to_lowercase();
            query.split_whitespace().all(|word| title.contains(word))
        };
        COMMANDS.iter().filter(has_all).collect()
    }

    /// The commands scripts add whose menu and title have every word of `query`.
    pub fn scripts(query: &str, scripts: &[crate::scripts::Entry]) -> Vec<crate::scripts::Entry> {
        let query = query.to_lowercase();
        let has_all = |script: &&crate::scripts::Entry| {
            let title = format!("{} {}", script.menu.title(), script.title).to_lowercase();
            query.split_whitespace().all(|word| title.contains(word))
        };
        scripts.iter().filter(has_all).copied().collect()
    }

    /// The brush presets whose name has every word of `query`, each by the group it is listed
    /// in and its name. The words *brush* and the group's count too. None for no words.
    pub fn brushes(query: &str, brushes: &iskra_drawing::Brushes) -> Vec<(String, String)> {
        let query = query.to_lowercase();
        if query.trim().is_empty() {
            return Vec::new();
        }
        let has_all = |preset: &&iskra_drawing::BrushPreset| {
            let said = format!("brush {} {}", preset.listed_group(), preset.name).to_lowercase();
            query.split_whitespace().all(|word| said.contains(word))
        };
        brushes.listed().filter(has_all).map(|preset| (preset.listed_group().to_string(), preset.name.clone())).collect()
    }

    /// Show the palette if it is open. Returns what was chosen, to run or to take.
    pub fn show(&mut self, ctx: &Context, app: &IskraApp) -> Option<Picked> {
        let query = self.query.as_mut()?;
        let mut chosen = None;
        let mut close = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let window = egui::Window::new("Command Palette").title_bar(false).resizable(false).collapsible(false);
        window.anchor(egui::Align2::CENTER_TOP, [0.0, 60.0]).fixed_size([420.0, 0.0]).show(ctx, |ui| {
            let (found, brushes) = (Self::matches(query), Self::brushes(query, &app.brushes));
            let scripts = Self::scripts(query, &app.scripts);
            let moved = ui.input_mut(|i| {
                i64::from(i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) - i64::from(i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp))
            });
            let count = found.len() + scripts.len() + brushes.len();
            self.picked = (self.picked as i64 + moved).clamp(0, count.saturating_sub(1) as i64) as usize;
            let entered = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            let field = ui.add(egui::TextEdit::singleline(query).hint_text("Search commands and brushes").desired_width(f32::INFINITY));
            if field.changed() {
                self.picked = 0;
            }
            field.request_focus();
            ui.separator();
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (index, command) in found.iter().enumerate() {
                    let enabled = (command.enabled)(app);
                    let keys = app.keys.chords(command.id).first().map(|c| ctx.format_shortcut(&c.shortcut())).unwrap_or_default();
                    let label = format!("{} › {}", command.menu.title(), command.title());
                    let row = ui.add_enabled(enabled, egui::Button::selectable(index == self.picked, label).shortcut_text(keys));
                    if enabled && (row.clicked() || (entered && index == self.picked)) {
                        chosen = Some(Picked::Command(command));
                    }
                }
                // The commands scripts add after the app's own.
                for (index, script) in scripts.iter().enumerate() {
                    let index = found.len() + index;
                    let keys = app.keys.chords(script.id).first().map(|c| ctx.format_shortcut(&c.shortcut())).unwrap_or_default();
                    let label = format!("{} › {}", script.menu.title(), script.title);
                    let row = ui.add(egui::Button::selectable(index == self.picked, label).shortcut_text(keys));
                    if row.clicked() || (entered && index == self.picked) {
                        chosen = Some(Picked::Script(script.id));
                    }
                }
                // The brush presets after the commands, each taken as the brush in hand.
                for (index, (group, name)) in brushes.iter().enumerate() {
                    let index = found.len() + scripts.len() + index;
                    let row = ui.add(egui::Button::selectable(index == self.picked, format!("Brush › {group} › {name}")));
                    if row.clicked() || (entered && index == self.picked) {
                        chosen = Some(Picked::Brush(group.clone(), name.clone()));
                    }
                }
                if count == 0 {
                    ui.weak("No command or brush matches");
                }
            });
            close |= entered;
        });
        if close || chosen.is_some() {
            self.query = None;
        }
        chosen
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::keymap::{Keymap, Preset};

    #[test]
    fn command_ids_are_unique_and_every_command_has_a_title() {
        let mut seen = HashSet::new();
        for command in commands() {
            assert!(seen.insert(command.id), "duplicate id {}", command.id);
            assert!(command.title() != command.id && !command.title().is_empty(), "{} has no title", command.id);
        }
        assert_eq!(command("file.save").map(AppCommand::title), Some("Save"), "an engine command keeps its title");
    }

    #[test]
    fn every_shortcut_of_every_preset_is_for_a_command_that_exists() {
        for preset in Preset::ALL {
            for (chord, id) in Keymap::preset(preset).in_order() {
                assert!(command(id).is_some(), "{}: {} is bound to `{id}`, which is no command", preset.name(), String::from(chord));
            }
        }
    }

    #[test]
    fn the_palette_finds_commands_by_any_words_of_their_menu_and_title() {
        let ids = |query| Palette::matches(query).iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids("onion"), ["view.onionSkin", "panel.onionSkin"], "the switch, and the panel of its settings");
        assert_eq!(ids("workspace onion"), ["panel.onionSkin"]);
        assert_eq!(ids("ROTATE counter"), ["view.rotateCounterclockwise"]);
        assert_eq!(ids("play loop"), ["playback.loop"], "the menu's name counts");
        assert_eq!(ids("animation page"), ["page.animation"]);
        assert_eq!(ids("").len(), commands().len());
        assert!(ids("zzz").is_empty());
    }
}
