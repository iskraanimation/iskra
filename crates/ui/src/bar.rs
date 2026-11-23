//! The bars over the panels (plan §7): a bar is made of parts, each a few controls that
//! belong together. Every part is listed once, in `PARTS`, and a panel's bar names the parts
//! it starts with (`of`). A part is dragged by the grip at its left to another place in the bar,
//! and the menu at the bar's right end, `…`, hides and shows the parts and puts the bar back
//! as it started. Which parts are shown, and in what order, is kept with the preferences.
//! A bar that is too long for its panel scrolls sideways; the menu stays at the right end.
//! A panel may have a second bar at its foot: a part let go in the panel's lower half goes
//! there, and one let go in its upper half comes back to the top.

use std::collections::BTreeSet;

use egui::{CursorIcon, Id, Rect, Sense, vec2};
use serde::{Deserialize, Serialize};

use crate::dock::Panel;
use crate::theme::Theme;

/// A part of a bar: what it is kept by, and what the menu calls it.
#[derive(Clone, Copy)]
pub struct Part {
    pub id: &'static str,
    pub title: &'static str,
}

/// Every part a bar can have, each once: a part of the same id in two bars is the same part.
pub const PARTS: [Part; 55] = [
    Part { id: "play", title: "Playback" },
    Part { id: "animate", title: "Animate" },
    Part { id: "volume", title: "Volume" },
    Part { id: "frame", title: "Frame and Length" },
    Part { id: "layer", title: "Add Layer" },
    Part { id: "marks", title: "Markers" },
    Part { id: "range", title: "Range" },
    Part { id: "convert", title: "Convert to Hold" },
    Part { id: "breakdown", title: "Breakdowns" },
    Part { id: "cycle", title: "Keyframe Cycle" },
    Part { id: "drawings", title: "Drawings" },
    Part { id: "zoom", title: "Zoom" },
    Part { id: "shots", title: "Shots" },
    Part { id: "sound", title: "Sound and Recording" },
    Part { id: "edit", title: "Snap, Ripple and Razor" },
    Part { id: "name", title: "Storyboard's Name" },
    Part { id: "display", title: "Display" },
    Part { id: "follow", title: "Follow" },
    Part { id: "onion", title: "Onion Skin and Light Table" },
    Part { id: "guides", title: "Guides" },
    Part { id: "clip", title: "Clip" },
    Part { id: "mask", title: "Aspect Mask" },
    Part { id: "pass", title: "Pass" },
    Part { id: "burn", title: "Burn-in" },
    Part { id: "duration", title: "Duration" },
    Part { id: "art", title: "Art Layers" },
    Part { id: "arrange", title: "Arrange" },
    Part { id: "map", title: "Mini Map" },
    Part { id: "keep", title: "Keep Size" },
    Part { id: "logged", title: "What Is Logged" },
    Part { id: "logcopy", title: "Copy and Clear" },
    Part { id: "logfile", title: "Log File" },
    Part { id: "fontsearch", title: "Search" },
    Part { id: "previews", title: "Previews" },
    Part { id: "scriptrun", title: "Run" },
    Part { id: "scriptclear", title: "Clear" },
    Part { id: "brushview", title: "Names and Strokes" },
    Part { id: "brushundo", title: "Undo" },
    Part { id: "brushsearch", title: "Search" },
    Part { id: "libfolders", title: "Folders" },
    Part { id: "libnav", title: "Up, New Folder, Refresh" },
    Part { id: "libsearch", title: "Search" },
    Part { id: "poolnew", title: "New and Bring Back" },
    Part { id: "poolshow", title: "Show" },
    Part { id: "poolcolumns", title: "Columns" },
    Part { id: "poolbin", title: "Bin" },
    Part { id: "inspector", title: "Inspector" },
    Part { id: "disknav", title: "Up and Refresh" },
    Part { id: "diskfavourite", title: "Add to Favourites" },
    Part { id: "diskfolder", title: "Folder" },
    Part { id: "advance", title: "Then On" },
    Part { id: "newbin", title: "New Bin" },
    Part { id: "scripts", title: "Script Buttons" },
    Part { id: "notewrite", title: "Write a Note" },
    Part { id: "notedone", title: "Show Done" },
];

/// The parts that may be put in any bar, from its `…` menu, as well as where they start: those
/// that act on the app, not on the panel under the bar. The script buttons start in none: they
/// are put where the user wants them (plan §9.4).
pub const ANYWHERE: [&str; 15] =
    ["play", "frame", "animate", "edit", "breakdown", "cycle", "convert", "layer", "drawings", "marks", "range", "volume", "onion", "art", "scripts"];

/// The parts a panel's bar starts with, in its order: none for a panel without a bar.
pub fn of(panel: Panel) -> Vec<Part> {
    let ids: &[&str] = match panel {
        Panel::Timeline => &["play", "animate", "volume", "frame", "layer", "marks", "range", "convert", "breakdown", "cycle", "drawings", "edit", "zoom"],
        Panel::Storyboard => &[
            "play",
            "frame",
            "animate",
            "convert",
            "breakdown",
            "cycle",
            "layer",
            "drawings",
            "shots",
            "colours",
            "sound",
            "edit",
            "marks",
            "range",
            "zoom",
            "volume",
            "name",
        ],
        Panel::Viewport => &["display", "zoom", "follow", "onion", "guides", "clip", "mask", "pass", "burn", "duration", "art"],
        Panel::NodeView => &["arrange", "map", "zoom"],
        Panel::TopView | Panel::SideView => &["keep"],
        Panel::Log => &["logged", "logcopy", "logfile"],
        Panel::Text => &["fontsearch", "previews"],
        Panel::ScriptConsole => &["scriptrun", "scriptclear"],
        Panel::Notes => &["notewrite", "notedone"],
        Panel::Brushes => &["brushview", "brushundo", "brushsearch"],
        Panel::Library => &["libfolders", "libnav", "libsearch"],
        Panel::Pool => &["poolnew", "poolshow", "poolcolumns", "poolbin", "inspector"],
        Panel::Storage => &["disknav", "diskfavourite", "diskfolder"],
        Panel::Substitution => &["advance"],
        Panel::Bin => &["newbin"],
        _ => &[],
    };
    ids.iter().filter_map(|id| PARTS.iter().find(|part| part.id == *id)).copied().collect()
}

/// How a bar is laid out: the order its parts were put in, where that is not their own,
/// those that are hidden, those in the bar at the panel's foot, and those added to it that
/// it does not start with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BarLayout {
    pub order: Vec<String>,
    pub hidden: BTreeSet<String>,
    pub below: BTreeSet<String>,
    pub added: Vec<String>,
}

/// Which of a panel's two bars is drawn, and the panel's rect, whose halves say where a part
/// let go goes.
#[derive(Clone, Copy)]
pub struct Rows {
    pub below: bool,
    pub panel: Rect,
}

/// The switches of the parts every bar may have, other than Animate and Adjust, which the
/// preferences keep: one set for the app, so that a part switched in one bar is so in all. The
/// app hands it to a timeline before it is shown and takes it back after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switches {
    /// Playing goes round again at the end.
    pub looping: bool,
    /// The playhead, dragged, is caught by what it comes next to; and a dragged shot's ends.
    pub snap: bool,
    /// What is made longer or shorter moves what comes after it along: the drawings after a
    /// hold, the shots after a shot.
    pub ripple: bool,
    /// A click on a shot's clip splits it there, where it otherwise picks it.
    pub razor: bool,
}

impl Default for Switches {
    fn default() -> Self {
        Switches { looping: false, snap: true, ripple: false, razor: false }
    }
}

/// *Playback*: Play or Stop, Loop, and where the playhead runs along the film, to the start of
/// the shot before and of the next. Returns the command asked for, if one is.
pub fn playback(ui: &mut egui::Ui, switches: &mut Switches, playing: bool, on_film: bool) -> Option<&'static str> {
    let mut asked = None;
    let (icon, hint) = if playing { ("square", "Stop (Enter)") } else { ("play", "Play (Enter)") };
    if crate::icons::button(ui, icon, false, hint).clicked() {
        asked = Some("playback.toggle");
    }
    if crate::icons::button(ui, "loop", switches.looping, "Loop: play again from the first frame after the last").clicked() {
        switches.looping = !switches.looping;
    }
    if on_film {
        if crate::icons::button(ui, "shot-previous", false, "To the start of this shot, or of the one before").clicked() {
            asked = Some("shot.previousCut");
        }
        if crate::icons::button(ui, "shot-next", false, "To the start of the next shot").clicked() {
            asked = Some("shot.nextCut");
        }
    }
    asked
}

/// The command *Animate* asked for, if one; and for the tests, where it drew its buttons.
pub struct Animated {
    pub asked: Option<&'static str>,
    #[cfg(test)]
    pub switch: Rect,
    #[cfg(test)]
    pub adjust: Rect,
    #[cfg(test)]
    pub key: Rect,
    #[cfg(test)]
    pub all: Rect,
}

/// *Animate*: the Animate switch, dark red while it is on, as the keyframes it makes are;
/// Adjust beside it; and the keyframe on the frame showing, added or taken away, and Key All.
/// `keyed` is whether the frame has a keyframe of what is in hand, none where nothing that
/// can be keyed is.
pub fn animate(ui: &mut egui::Ui, animate: &mut bool, adjust: &mut bool, keyed: Option<bool>, can_key_all: bool) -> Animated {
    let text = egui::RichText::new("Animate");
    let red = Theme::of(ui.ctx()).colour(crate::theme::Role::Animate);
    let button = if *animate { egui::Button::new(text.color(egui::Color32::WHITE)).fill(red) } else { egui::Button::new(text) };
    let switch = ui.add(button).on_hover_text(crate::ANIMATE_HINT);
    if switch.clicked() {
        *animate = !*animate;
    }
    let hint = "Adjust: a change keyed on one keyframe is made alike on the other keyframes picked on the timeline, of the same peg, bones or lens. \
                Moved 20 to the right on one, they all move 20 to the right";
    let adjusted = toggle(ui, *adjust, "Adjust").on_hover_text(hint);
    if adjusted.clicked() {
        *adjust = !*adjust;
    }
    let on = keyed == Some(true);
    let (label, hint, command) = match on {
        true => ("Remove Key", "Take away the keyframe on this frame of the peg, the bones or the camera in hand", "key.remove"),
        false => {
            ("Add Key", "Key the position, rotation, scale and skew on this frame, of the peg or the bones in hand, or of the camera with its zoom", "key.add")
        }
    };
    let key = ui.add_enabled(keyed.is_some(), egui::Button::new(label).selected(on)).on_hover_text(hint);
    let mut asked = key.clicked().then_some(command);
    let hint = "Key all that can be keyed on this frame, in the tree of what is selected: the pegs, the bones and the effects \
                already animated that it carries or has in it. With nothing selected, all of the shot";
    let all = ui.add_enabled(can_key_all, egui::Button::new("Key All")).on_hover_text(hint);
    if all.clicked() {
        asked = Some("key.all");
    }
    Animated {
        asked,
        #[cfg(test)]
        switch: switch.rect,
        #[cfg(test)]
        adjust: adjusted.rect,
        #[cfg(test)]
        key: key.rect,
        #[cfg(test)]
        all: all.rect,
    }
}

/// A switch of words, lit while it is on. It is framed while off too, as the icon switches
/// are: one framed only while hovered grew by its frame's line, and moved the bar after it.
fn toggle(ui: &mut egui::Ui, on: bool, text: &str) -> egui::Response {
    ui.add(egui::Button::new(text).selected(on))
}

/// What a part asks of the app, where it is more than one of the app's commands: the app works
/// out from what is picked what it is done to.
#[derive(Clone, Debug, PartialEq)]
pub enum Asked {
    /// One of the app's commands, or a script's, by its id.
    Command(&'static str),
    /// A command of the project, with its parameters.
    Run(&'static str, serde_json::Value),
    /// A held key every so many frames between the keyframes picked on the page's timeline.
    HoldEvery(u32),
    /// The keyframes picked on the page's timeline cycled so, or with none their cycles
    /// taken away.
    Cycle(Option<iskra_model::CycleKind>),
}

/// What the keyframes picked on the page's timeline are to *Keyframe Cycle*: whether they can
/// be cycled, two or more of one thing or a key of a cycle being picked, and how the cycle
/// they are in repeats, where they are all in one of the same kind.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cycled {
    pub can: bool,
    pub kind: Option<iskra_model::CycleKind>,
}

/// *Keyframe Cycle*: what the keyframes picked do repeated after the last of them, until the
/// next keyframe or the end: *None*, *Loop* or *Ping Pong*. Greyed until two keyframes of one
/// thing, or a keyframe of a cycle, are picked.
pub fn cycle(ui: &mut egui::Ui, cycled: Cycled) -> Option<Asked> {
    use iskra_model::CycleKind;
    let mut asked = None;
    ui.add_enabled_ui(cycled.can, |ui| {
        ui.weak("Cycle").on_hover_text("Repeat the keyframes picked after the last of them, until the next keyframe or the end of the shot");
        let kinds = [
            (None, "None", "No cycle: after the last keyframe picked the value goes on to the next keyframe"),
            (Some(CycleKind::Loop), "Loop", "From the first keyframe picked to the last, again and again"),
            (Some(CycleKind::PingPong), "Ping Pong", "From the first keyframe picked to the last and back, again and again"),
        ];
        for (kind, title, hint) in kinds {
            let radio = ui.radio(cycled.kind == kind, title).on_hover_text(hint).on_disabled_hover_text("Pick two keyframes or more of one thing first");
            if radio.clicked() && cycled.kind != kind {
                asked = Some(Asked::Cycle(kind));
            }
        }
    });
    asked
}

/// *Convert to Hold*: a held key every so many frames, typed, between the keyframes picked, as
/// the motion was there. The number is one for every bar.
pub fn convert(ui: &mut egui::Ui, picked: bool) -> Option<Asked> {
    let id = Id::new("convert to hold every");
    let mut every = ui.data(|d| d.get_temp::<u32>(id)).unwrap_or(2);
    let hint = if picked {
        "A held key every so many frames between the keyframes picked, as the motion was there: 1 on ones, 2 on twos"
    } else {
        "Pick keyframes on the timeline first: the motion between them is held in steps"
    };
    let hold = ui.add_enabled(picked, egui::Button::new("Hold")).on_hover_text(hint).on_disabled_hover_text(hint);
    ui.add(egui::DragValue::new(&mut every).range(1..=99).suffix(" fr")).on_hover_text("Every how many frames a held key is made");
    ui.data_mut(|d| d.insert_temp(id, every));
    hold.clicked().then_some(Asked::HoldEvery(every))
}

/// *Add Layer*: a new layer in `shot`, of one kind of art: pixels, vector art, or texts to type
/// on with the Text tool. Greyed where there is no shot to put it in.
pub fn add_layer(ui: &mut egui::Ui, shot: Option<u64>) -> Option<Asked> {
    let mut asked = None;
    ui.add_enabled_ui(shot.is_some(), |ui| {
        let hint = "A new layer in the shot: of pixels, of vector art, or of texts to type on with the Text tool";
        ui.menu_button("Add Layer", |ui| {
            for (title, kind) in [("Bitmap Layer", "bitmap"), ("Vector Layer", "vector"), ("Text Layer", "text")] {
                if let (true, Some(shot)) = (ui.button(title).clicked(), shot) {
                    asked = Some(Asked::Run("layer.new", serde_json::json!({ "shot": shot, "kind": kind })));
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text(hint);
    });
    asked
}

/// *Drawings*: a new drawing on the layer in hand from the frame showing, or a copy of the one
/// exposed there; and how long a new one holds, typed in frames, or with *Whole Shot* until the
/// next drawing or the end of the shot. The number is kept while Whole Shot is on. *Extend*
/// holds the drawing before the playhead on to it, on each layer picked: `extendable` where
/// there is one to hold.
pub fn drawings(ui: &mut egui::Ui, hold: &mut crate::prefs::NewDrawingHold, layer: bool, extendable: bool) -> Option<Asked> {
    use crate::prefs::NewDrawingHold;
    let mut asked = None;
    let hint = "A new drawing on the layer in hand, exposed from the frame showing, held as the box says";
    if ui.add_enabled(layer, egui::Button::new("New Drawing")).on_hover_text(hint).clicked() {
        asked = Some(Asked::Command("drawing.newDrawing"));
    }
    let hint = "Copy the drawing exposed on this frame, so it can change on its own";
    if ui.add_enabled(layer, egui::Button::new("Duplicate")).on_hover_text(hint).clicked() {
        asked = Some(Asked::Command("drawing.duplicate"));
    }
    let id = Id::new("new drawings hold frames");
    let mut frames = match *hold {
        NewDrawingHold::Frames(frames) => frames,
        NewDrawingHold::ToNext => ui.data(|d| d.get_temp::<u32>(id)).unwrap_or(1),
    };
    ui.weak("Hold").on_hover_text("How many frames a new drawing holds");
    if ui.add(egui::DragValue::new(&mut frames).range(1..=99).suffix(" fr")).on_hover_text("How many frames a new drawing holds").changed() {
        *hold = NewDrawingHold::Frames(frames);
    }
    ui.data_mut(|d| d.insert_temp(id, frames));
    let whole = *hold == NewDrawingHold::ToNext;
    let hint = "Whole Shot: a new drawing holds until the next drawing, or to the end of the shot. Off, as many frames as the box says";
    if toggle(ui, whole, "Whole Shot").on_hover_text(hint).clicked() {
        *hold = if whole { NewDrawingHold::Frames(frames) } else { NewDrawingHold::ToNext };
    }
    let hint = "Hold the drawing before the playhead on to it, over the empty cells between: on each of the layers that are picked";
    let disabled = "Put the playhead on an empty cell after a drawing, on the layer in hand or the layers picked";
    if ui.add_enabled(extendable, egui::Button::new("Extend")).on_hover_text(hint).on_disabled_hover_text(disabled).clicked() {
        asked = Some(Asked::Command("exposure.extend"));
    }
    asked
}

/// What *Markers* and *Range* set, and where: the shot's on the Animation page, the storyboard's
/// on the Storyboard page.
pub struct Marks {
    /// What they are of: `("shot", id)` or `("storyboard", id)`, the commands' own word.
    pub of: (&'static str, u64),
    /// The playhead's frame, where a marker or an end of the range is put.
    pub frame: u32,
    /// The colour of the marker on that frame, if there is one.
    pub here: Option<[u8; 3]>,
    /// There are an in and an out point to clear.
    pub ranged: bool,
}

impl Marks {
    pub(crate) fn asked(&self, what: &str, mut params: serde_json::Value) -> Asked {
        params[self.of.0] = serde_json::json!(self.of.1);
        let id = match (self.of.0, what) {
            ("shot", "marker") => "shot.setMarker",
            ("shot", _) => "shot.setRange",
            (_, "marker") => "storyboard.setMarker",
            _ => "storyboard.setRange",
        };
        Asked::Run(id, params)
    }
}

/// *Markers*: a marker on the playhead's frame, in the colour picked beside it; and the colour,
/// which a marker on the playhead's frame is given too. The colour is one for every bar.
pub fn markers(ui: &mut egui::Ui, marks: Option<&Marks>) -> Option<Asked> {
    let id = Id::new("new marker colour");
    let colour = ui.data(|d| d.get_temp::<[u8; 3]>(id)).unwrap_or(iskra_model::MARKER_COLOURS[0].1);
    let mut asked = None;
    ui.add_enabled_ui(marks.is_some(), |ui| {
        let hint = "A marker on the playhead's frame, in the colour beside it. Its menu on the ruler names it";
        if let (true, Some(marks)) = (ui.button("Marker").on_hover_text(hint).clicked(), marks) {
            asked = Some(marks.asked("marker", serde_json::json!({ "frame": marks.frame, "colour": colour })));
        }
        let dot = |rgb: [u8; 3]| egui::RichText::new("●").color(egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
        let hint = "The colour of a new marker, and of the one on the playhead's frame";
        ui.menu_button(dot(marks.and_then(|marks| marks.here).unwrap_or(colour)), |ui| {
            for (name, rgb) in iskra_model::MARKER_COLOURS {
                let mut text = egui::text::LayoutJob::default();
                dot(rgb).append_to(&mut text, ui.style(), egui::FontSelection::Default, egui::Align::Center);
                egui::RichText::new(format!("  {name}")).append_to(&mut text, ui.style(), egui::FontSelection::Default, egui::Align::Center);
                if ui.selectable_label(rgb == colour, text).clicked() {
                    ui.data_mut(|d| d.insert_temp(id, rgb));
                    if let Some(marks) = marks.filter(|marks| marks.here.is_some()) {
                        asked = Some(marks.asked("marker", serde_json::json!({ "frame": marks.frame, "colour": rgb })));
                    }
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text(hint);
    });
    asked
}

/// *Range*: the playhead's frame as the in or the out point of the stretch that plays and
/// loops, or no stretch: all of it plays.
pub fn range(ui: &mut egui::Ui, marks: Option<&Marks>) -> Option<Asked> {
    let on = marks.is_some();
    let mut asked = None;
    let hint = "Play from the playhead's frame on: the in point of the stretch that plays and loops";
    if ui.add_enabled(on, egui::Button::new("In")).on_hover_text(hint).clicked() {
        asked = marks.map(|marks| marks.asked("range", serde_json::json!({ "in": marks.frame })));
    }
    let hint = "Play up to the playhead's frame: the out point of the stretch that plays and loops";
    if ui.add_enabled(on, egui::Button::new("Out")).on_hover_text(hint).clicked() {
        asked = marks.map(|marks| marks.asked("range", serde_json::json!({ "out": marks.frame })));
    }
    let ranged = marks.is_some_and(|marks| marks.ranged);
    if ui.add_enabled(ranged, egui::Button::new("Clear")).on_hover_text("No in and out points: all of it plays").clicked() {
        asked = marks.map(|marks| marks.asked("range", serde_json::json!({ "clear": true })));
    }
    asked
}

/// What *Zoom* was asked: a zoom to go to, in percent, or to fit; and for the tests, where its
/// Fit was drawn.
pub struct Zoomed {
    pub to: Option<f32>,
    pub fit: bool,
    #[cfg(test)]
    pub fit_button: Rect,
}

/// *Zoom*: how near what the panel shows is seen, in percent, on a slider whose steps are each
/// as much again; and *Fit*, which shows all of it. The same in every panel that zooms: `hint`
/// says what its zoom is, and `fit` what fitting shows.
pub fn zoom(ui: &mut egui::Ui, percent: f32, range: std::ops::RangeInclusive<f32>, hint: &str, fit: &str) -> Zoomed {
    // The value is shown whole, and kept as it is: rounding it would change it.
    let mut value = percent;
    let slider = egui::Slider::new(&mut value, range).logarithmic(true).suffix("%").custom_formatter(|value, _| format!("{value:.0}")).text("Zoom");
    let to = ui.add(slider).on_hover_text(hint).changed().then_some(value);
    let button = ui.button("Fit").on_hover_text(fit);
    Zoomed {
        to,
        fit: button.clicked(),
        #[cfg(test)]
        fit_button: button.rect,
    }
}

/// *Edit*: Snap, Ripple, and where the bar is over the storyboard, the Razor.
pub fn edit(ui: &mut egui::Ui, switches: &mut Switches, razor: bool) {
    let hint = "Snap: the playhead, while it is dragged, is caught by the drawings, the keyframes, the markers and the shots' ends it comes next to; \
                and a dragged shot's ends by the ends of the other shots and by the playhead";
    if crate::icons::button(ui, "magnet", switches.snap, hint).clicked() {
        switches.snap = !switches.snap;
    }
    let hint = "Ripple: what is made longer or shorter moves what comes after it along: the drawings after a hold, the later shots after a shot. \
                Off, they stay where they are, and what would run into a shot is refused. Alt on a hold does the other";
    if crate::icons::switch(ui, "ripple", switches.ripple, hint).clicked() {
        switches.ripple = !switches.ripple;
    }
    if razor {
        let hint = "Razor: a click on a shot splits it there, into two shots. Off, a click picks the shot";
        if crate::icons::switch(ui, "razor", switches.razor, hint).clicked() {
            switches.razor = !switches.razor;
        }
    }
}

/// How wide the room for the menu at a bar's right end is, and a part's grip.
const MENU: f32 = 30.0;
const GRIP: f32 = 8.0;

impl BarLayout {
    /// The parts of the bar over `panel`: those it starts with, then those added to it.
    pub fn parts(&self, panel: Panel) -> Vec<Part> {
        let mut parts = of(panel);
        for id in &self.added {
            if let Some(part) = PARTS.iter().find(|part| part.id == id && ANYWHERE.contains(&part.id) && !parts.iter().any(|had| had.id == part.id)) {
                parts.push(*part);
            }
        }
        parts
    }

    /// Show the part `id` in the bar over `panel`: added at the end where it is not one of its
    /// parts, or shown again where it is hidden.
    pub fn add(&mut self, panel: Panel, id: &str) {
        self.hidden.remove(id);
        if ANYWHERE.contains(&id) && !self.parts(panel).iter().any(|part| part.id == id) {
            self.added.push(id.to_string());
        }
    }

    /// Take the part `id`, which was added, out of the bar.
    fn remove(&mut self, id: &str) {
        self.added.retain(|added| added != id);
        self.order.retain(|put| put != id);
        self.hidden.remove(id);
        self.below.remove(id);
    }

    /// All of a bar's parts in the order they are in: those that were put somewhere first, as
    /// they were put, then the others in their own order. A part the bar no longer has is
    /// left out, and one that is new comes at the end.
    fn ordered(&self, all: &[Part]) -> Vec<Part> {
        let put = self.order.iter().filter_map(|id| all.iter().find(|part| part.id == id));
        let rest = all.iter().filter(|part| !self.order.iter().any(|id| id == part.id));
        put.chain(rest).copied().collect()
    }

    /// The parts that are shown, in their order.
    pub fn shown(&self, all: &[Part]) -> Vec<Part> {
        self.ordered(all).into_iter().filter(|part| !self.hidden.contains(part.id)).collect()
    }

    /// The parts that are shown in the bar at the top, or in the one at the foot.
    pub fn shown_in(&self, all: &[Part], below: bool) -> Vec<Part> {
        self.shown(all).into_iter().filter(|part| self.below.contains(part.id) == below).collect()
    }

    /// Put the part `id` at `index` among the parts that are shown. Those that are hidden
    /// keep their places after them.
    pub fn put(&mut self, all: &[Part], id: &str, index: usize) {
        let below = self.below.contains(id);
        self.put_in(all, id, below, index);
    }

    /// Put the part `id` in the bar at the top or at the foot, at `index` among the parts shown
    /// there.
    pub fn put_in(&mut self, all: &[Part], id: &str, below: bool, index: usize) {
        let Some(part) = all.iter().find(|part| part.id == id) else { return };
        let mut row: Vec<&'static str> = self.shown_in(all, below).iter().map(|part| part.id).filter(|other| *other != id).collect();
        row.insert(index.min(row.len()), part.id);
        if below {
            self.below.insert(id.to_string());
        } else {
            self.below.remove(id);
        }
        let other = self.shown_in(all, !below).into_iter().map(|part| part.id);
        let hidden = self.ordered(all).into_iter().map(|part| part.id).filter(|part| self.hidden.contains(*part));
        self.order = row.iter().copied().chain(other).chain(hidden).map(str::to_string).collect();
    }

    /// Call the part `from` `to`, where it is laid out: a part whose controls went into another.
    pub fn rename(&mut self, from: &str, to: &str) {
        for id in self.order.iter_mut().filter(|id| *id == from) {
            *id = to.to_string();
        }
        for set in [&mut self.hidden, &mut self.below] {
            if set.remove(from) {
                set.insert(to.to_string());
            }
        }
    }

    /// Where the grip of the part `id` of the bar keyed by `salt` was last drawn.
    #[cfg(test)]
    pub fn grip(ctx: &egui::Context, salt: &str, id: &str) -> Option<Rect> {
        ctx.data(|d| d.get_temp(Id::new((salt, id, "grip"))))
    }

    /// Whether a part of the bar keyed by `salt` is being dragged by its grip.
    pub fn dragging(ctx: &egui::Context, salt: &str) -> bool {
        ctx.data(|d| d.get_temp::<bool>(Id::new((salt, "bar dragging")))).unwrap_or(false)
    }

    /// Draw the bar: each part that is shown by `part`, with its grip before it, and the menu
    /// at the right end.
    pub fn show(&mut self, ui: &mut egui::Ui, salt: &str, all: &[Part], part: impl FnMut(&mut egui::Ui, &'static str)) {
        self.show_row(ui, salt, all, None, part);
    }

    /// Draw one of a panel's two bars, or with no `rows` the one bar of a panel that has one.
    pub fn show_row(&mut self, ui: &mut egui::Ui, salt: &str, all: &[Part], rows: Option<Rows>, mut part: impl FnMut(&mut egui::Ui, &'static str)) {
        let theme = Theme::of(ui.ctx());
        let below = rows.is_some_and(|rows| rows.below);
        let row_parts = if rows.is_some() { self.shown_in(all, below) } else { self.shown(all) };
        ui.horizontal(|ui| {
            let room = (ui.available_width() - MENU).max(40.0);
            // Where each part was drawn, from its grip to its end; and the part that was let
            // go of, with where.
            let mut drawn: Vec<(&'static str, Rect)> = Vec::new();
            let mut let_go: Option<(&'static str, egui::Pos2)> = None;
            let parts = egui::ScrollArea::horizontal().id_salt((salt, below)).max_width(room).auto_shrink([false, true]);
            parts.scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if row_parts.is_empty() {
                        ui.weak("Drag a part of the bar here");
                    }
                    for shown in row_parts {
                        let (grip, held) = ui.allocate_exact_size(vec2(GRIP, ui.spacing().interact_size.y), Sense::drag());
                        let held = held
                            .on_hover_cursor(CursorIcon::Grab)
                            .on_hover_text("Drag to put this part of the bar elsewhere. The … at the right end hides and shows the parts");
                        let colour = if held.dragged() || held.hovered() { theme.accent } else { theme.text_muted.gamma_multiply(0.6) };
                        for row in 0..3 {
                            ui.painter().circle_filled(grip.center() + vec2(0.0, (row as f32 - 1.0) * 5.0), 1.2, colour);
                        }
                        #[cfg(test)]
                        ui.data_mut(|d| d.insert_temp(Id::new((salt, shown.id, "grip")), grip));
                        if held.dragged() {
                            ui.data_mut(|d| d.insert_temp(Id::new((salt, "bar dragging")), true));
                        }
                        if held.drag_stopped() {
                            ui.data_mut(|d| d.insert_temp(Id::new((salt, "bar dragging")), false));
                            let_go = ui.input(|i| i.pointer.latest_pos()).map(|at| (shown.id, at));
                        }
                        part(ui, shown.id);
                        drawn.push((shown.id, Rect::from_x_y_ranges(grip.left()..=ui.cursor().left(), grip.y_range())));
                        ui.separator();
                    }
                });
            });
            // Where each part of this bar was drawn, for a part let go in the other bar.
            let drawn_id = |below: bool| Id::new((salt, "bar drawn", below));
            ui.data_mut(|d| d.insert_temp(drawn_id(below), drawn.iter().map(|(id, rect)| (id.to_string(), *rect)).collect::<Vec<_>>()));
            // Let go, a part goes before the first other whose middle is to the right of there,
            // in the bar of the panel's half it is let go in.
            if let Some((id, at)) = let_go {
                let to = rows.map_or(below, |rows| at.y > rows.panel.center().y);
                let there: Vec<(String, Rect)> = if to == below {
                    drawn.iter().map(|(id, rect)| (id.to_string(), *rect)).collect()
                } else {
                    ui.data(|d| d.get_temp(drawn_id(to))).unwrap_or_default()
                };
                let index = there.iter().filter(|(other, rect)| other != id && rect.center().x < at.x).count();
                self.put_in(all, id, to, index);
            }
            ui.menu_button("…", |ui| {
                for listed in self.ordered(all) {
                    let added = self.added.iter().any(|id| id == listed.id);
                    let mut on = !self.hidden.contains(listed.id);
                    let hint = if added { "Added to this bar: untick to take it out again" } else { "Show or hide this part of the bar" };
                    if ui.checkbox(&mut on, listed.title).on_hover_text(hint).changed() {
                        match (on, added) {
                            (false, true) => self.remove(listed.id),
                            (true, _) => drop(self.hidden.remove(listed.id)),
                            (false, false) => drop(self.hidden.insert(listed.id.to_string())),
                        }
                    }
                }
                // The parts that may go anywhere and are not here yet.
                let more: Vec<Part> = ANYWHERE
                    .iter()
                    .filter(|id| !all.iter().any(|part| part.id == **id))
                    .filter_map(|id| PARTS.iter().find(|part| part.id == *id))
                    .copied()
                    .collect();
                if !more.is_empty() {
                    ui.menu_button("Add Part", |ui| {
                        for part in more {
                            if ui.button(part.title).on_hover_text("Put it at the end of this bar").clicked() {
                                self.added.push(part.id.to_string());
                                ui.close();
                            }
                        }
                    });
                }
                ui.separator();
                if ui
                    .add_enabled(*self != Self::default(), egui::Button::new("Reset"))
                    .on_hover_text("All the parts, in the order the bar starts with")
                    .clicked()
                {
                    *self = Self::default();
                    ui.close();
                }
            })
            .response
            .on_hover_text("The parts of this bar: hide and show them here, drag one by the dots at its left to put it elsewhere");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARTS: [Part; 4] = [
        Part { id: "play", title: "Playback" },
        Part { id: "marks", title: "Markers" },
        Part { id: "zoom", title: "Zoom" },
        Part { id: "cell", title: "Cell" },
    ];

    #[test]
    fn a_switch_of_words_is_as_wide_hovered_as_not() {
        let ctx = egui::Context::default();
        crate::theme::Theme::dark().apply(&ctx);
        let width = |ctx: &egui::Context, on: bool, over: Option<egui::Pos2>| {
            let mut rect = Rect::NOTHING;
            let input = egui::RawInput { events: over.map(egui::Event::PointerMoved).into_iter().collect(), ..Default::default() };
            // Twice, as egui frames a button by how it was hovered the frame before.
            for _ in 0..2 {
                ctx.run_ui(input.clone(), |ui| rect = toggle(ui, on, "Whole Shot").rect).textures_delta.clear();
            }
            rect
        };
        for on in [false, true] {
            let still = width(&ctx, on, None);
            assert_eq!(width(&ctx, on, Some(still.center())).width(), still.width(), "on: {on}");
        }
    }

    #[test]
    fn every_part_is_listed_once_and_every_bar_has_only_listed_parts() {
        let ids: BTreeSet<&str> = super::PARTS.iter().map(|part| part.id).collect();
        assert_eq!(ids.len(), super::PARTS.len(), "an id listed twice");
        let starts = |panel: Panel| match panel {
            Panel::Timeline => 13,
            Panel::Storyboard => 16,
            Panel::Viewport => 11,
            Panel::NodeView => 3,
            Panel::TopView | Panel::SideView | Panel::Substitution | Panel::Bin => 1,
            Panel::Log | Panel::Brushes | Panel::Library | Panel::Storage => 3,
            Panel::Pool => 5,
            Panel::Text | Panel::ScriptConsole | Panel::Notes => 2,
            _ => 0,
        };
        for panel in Panel::ALL {
            assert_eq!(of(panel).len(), starts(panel), "{panel:?}");
        }
        // The same part in two bars: the zoom is in four.
        assert_eq!(Panel::ALL.iter().filter(|panel| of(**panel).iter().any(|part| part.id == "zoom")).count(), 4);
    }

    #[test]
    fn the_timelines_have_every_part_that_may_go_anywhere_but_the_viewers_and_the_script_buttons() {
        // An added part is drawn by the app while the timeline is out of it, so it must not
        // need the timeline: only the viewer's parts and the script buttons, which act on the
        // app alone, are added.
        for panel in [Panel::Timeline, Panel::Storyboard] {
            let lacks: Vec<&str> = ANYWHERE.iter().copied().filter(|id| !of(panel).iter().any(|part| part.id == *id)).collect();
            assert_eq!(lacks, ["onion", "art", "scripts"], "{panel:?}");
        }
    }

    #[test]
    fn a_part_added_to_a_bar_comes_after_its_own_and_goes_when_taken_out() {
        let mut layout = BarLayout { added: ["onion", "keep", "play", "onion"].map(str::to_string).to_vec(), ..Default::default() };
        let ids = |layout: &BarLayout| layout.parts(Panel::NodeView).iter().map(|part| part.id).collect::<Vec<_>>();
        // A part of one panel, and one added twice, are not added.
        assert_eq!(ids(&layout), ["arrange", "map", "zoom", "onion", "play"]);
        assert_eq!(layout.parts(Panel::Timeline).iter().filter(|part| part.id == "play").count(), 1, "the timeline's own");
        layout.hidden.insert("onion".to_string());
        layout.remove("onion");
        assert_eq!((ids(&layout), layout.hidden.len()), (vec!["arrange", "map", "zoom", "play"], 0));
        // Added from a panel's tab: once, shown again where it was hidden, and only a part that
        // may go anywhere.
        layout.hidden.insert("zoom".to_string());
        for id in ["scripts", "scripts", "zoom", "keep"] {
            layout.add(Panel::NodeView, id);
        }
        assert_eq!((ids(&layout), layout.hidden.len()), (vec!["arrange", "map", "zoom", "play", "scripts"], 0));
    }

    fn ids(layout: &BarLayout) -> Vec<&'static str> {
        layout.shown(&PARTS).iter().map(|part| part.id).collect()
    }

    #[test]
    fn a_bars_parts_are_hidden_and_put_elsewhere_and_what_is_new_comes_last() {
        let mut layout = BarLayout::default();
        assert_eq!(ids(&layout), ["play", "marks", "zoom", "cell"]);
        // The last part dragged to the front, and the second hidden.
        layout.put(&PARTS, "cell", 0);
        layout.hidden.insert("marks".to_string());
        assert_eq!(ids(&layout), ["cell", "play", "zoom"]);
        // A part put among those that are shown: the hidden one is not counted.
        layout.put(&PARTS, "play", 2);
        assert_eq!(ids(&layout), ["cell", "zoom", "play"]);
        layout.hidden.clear();
        assert_eq!(ids(&layout), ["cell", "zoom", "play", "marks"], "shown again, after those that were put");
        // A layout that names a part the bar has no more, and lacks one it has now.
        let old = BarLayout { order: vec!["zoom".to_string(), "gone".to_string(), "play".to_string()], ..Default::default() };
        assert_eq!(ids(&old), ["zoom", "play", "marks", "cell"]);
    }

    #[test]
    fn a_part_goes_to_the_bar_at_the_foot_and_back() {
        let mut layout = BarLayout::default();
        let row = |layout: &BarLayout, below| layout.shown_in(&PARTS, below).iter().map(|part| part.id).collect::<Vec<_>>();
        layout.put_in(&PARTS, "zoom", true, 0);
        layout.put_in(&PARTS, "play", true, 0);
        assert_eq!((row(&layout, false), row(&layout, true)), (vec!["marks", "cell"], vec!["play", "zoom"]));
        // Put within its bar, it stays there.
        layout.put(&PARTS, "play", 5);
        assert_eq!(row(&layout, true), ["zoom", "play"]);
        // Hidden, it is in neither; shown again, in the bar it was in.
        layout.hidden.insert("zoom".to_string());
        assert_eq!(row(&layout, true), ["play"]);
        layout.hidden.clear();
        assert_eq!(row(&layout, true), ["zoom", "play"]);
        layout.put_in(&PARTS, "zoom", false, 1);
        assert_eq!((row(&layout, false), row(&layout, true)), (vec!["marks", "zoom", "cell"], vec!["play"]));
    }
}
