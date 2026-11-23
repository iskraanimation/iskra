//! The Brushes panel (plan §7.12): the brush presets by group, each with a stroke drawn by it
//! as it would look, and under them the settings of the brush in hand, folding away.
//!
//! A click takes a preset as the brush in hand. The engine's presets come first, in its groups,
//! then the user's. A change to the brush in hand is tried at once on a stroke in the panel.
//! The presets listed are of the kind the art layer in hand takes, raster or vector (plan
//! §4.16); the others are under a heading at the end, greyed, to be seen and not taken.
//!
//! The user's presets are looked after here: one is dragged to another place in the list or
//! onto a group's name, a group by its name before another; a right-click on a preset or on a
//! group's name has the rest. A delete asks first, and what was last done to the presets is
//! undone by the panel's own *Undo*, since presets are the user's and not the project's.

use std::collections::HashMap;

use egui::{Color32, Rect, Sense, TextureHandle, TextureOptions, vec2};
use iskra_drawing::{
    Brush, BrushKind, BrushPreset, BrushSettings, Brushes, Control, Dynamic, GrayTile, MaskMode, Pattern, PatternStyle, TipShape, VectorBrush, stroke_picture,
};

use crate::theme::Theme;

/// A stroke's picture in pixels. It is shown at half that in points, so it is sharp on a
/// display of two pixels a point.
const PICTURE: [usize; 2] = [360, 72];

/// How many pictures are made on one frame of the screen, so that a long list opens at once.
const MADE_A_FRAME: usize = 6;

/// The side of a sampled tip's picture, in pixels and in points.
const TIP_SIDE: usize = 48;

/// What a name is being typed for.
#[derive(Clone, Debug, PartialEq)]
enum Naming {
    /// A new preset of the brush in hand.
    Save,
    /// Another name for the preset of this group and name.
    Rename(String, String),
    /// A new group for the preset of this group and name.
    Move(String, String),
    /// Another name for this group.
    Group(String),
}

/// What was asked of a preset, by its group and name.
enum Act {
    Take(String, String),
    Name(Naming),
    MoveTo(String, String, String),
    Duplicate(String, String),
    Delete(String, String),
    DeleteGroup(String),
    /// A preset, put before another in a group, or at the group's end.
    Place(String, String, String, Option<String>),
    /// A group, put before another, or at the end.
    PlaceGroup(String, Option<String>),
    /// Write a group to a file, or one preset of it.
    Export(String, Option<String>),
    /// Give a preset a slot to be taken by, or take its slot away.
    Slot(String, String, Option<usize>),
    Restore,
    Undo,
    /// A delete that was asked about, and is to be done.
    Answered(Box<Act>),
}

/// What is dragged in the list: a preset by its group and name, or a group.
#[derive(Clone, Debug, PartialEq)]
enum Dragged {
    Preset(String, String),
    Group(String),
}

/// What showing the panel came to.
#[derive(Default)]
pub struct Shown {
    /// A preset was taken as the brush in hand.
    pub taken: bool,
    /// Presets to be written to a file, with a name for it: the app asks where.
    pub export: Option<(String, Vec<BrushPreset>)>,
}

/// How much the copies of the presets that are kept to undo by may hold together, in bytes,
/// and how many are kept at most.
const UNDO_WEIGHT: usize = 256 << 20;
const UNDO_STEPS: usize = 12;

/// The stroke that tries the brush in hand: the brush it was drawn by, its picture, and the
/// picture of its tip if that is a sampled one.
struct Trial {
    brush: Brush,
    stroke: TextureHandle,
    tip: Option<TextureHandle>,
}

pub struct BrushesPanel {
    /// The kind of brush the art layer in hand takes, as the panel was last shown.
    kind: BrushKind,
    search: String,
    /// The stroke of each preset, by its group and name.
    pictures: HashMap<(String, String), TextureHandle>,
    trial: Option<Trial>,
    /// A name being typed, what it is for, and whether its field has yet to take the keys.
    naming: Option<(Naming, String, bool)>,
    /// A delete that waits for its answer.
    asking: Option<Act>,
    /// Why a picture could not be made a tip or a pattern, until it is clicked away.
    problem: Option<String>,
    /// The vector brush's pressure curve, drawn as a curve of tone is.
    pub(crate) pressure: crate::tone::ToneCurves,
    /// What was done to the presets, the last last: what it was, and the presets before it.
    undo: Vec<(String, Brushes)>,
    /// Where each preset was drawn.
    #[cfg(test)]
    pub drawn: Vec<(String, Rect)>,
    /// Where each group's name was drawn.
    #[cfg(test)]
    pub headers: Vec<(String, Rect)>,
    /// How many times the stroke that tries the brush in hand was drawn.
    #[cfg(test)]
    pub trials: usize,
}

impl Default for BrushesPanel {
    fn default() -> Self {
        Self {
            kind: BrushKind::Raster,
            search: String::new(),
            pictures: HashMap::new(),
            trial: None,
            naming: None,
            asking: None,
            problem: None,
            pressure: crate::tone::ToneCurves::default(),
            undo: Vec::new(),
            #[cfg(test)]
            drawn: Vec::new(),
            #[cfg(test)]
            headers: Vec::new(),
            #[cfg(test)]
            trials: 0,
        }
    }
}

impl BrushesPanel {
    /// Ask for the name under which the brush in hand of `kind` is kept as a preset.
    pub fn ask_save(&mut self, brushes: &Brushes, kind: BrushKind) {
        self.kind = kind;
        let (name, group) = brushes.in_hand.named(kind);
        // One of the user's is offered its own name, to be changed by saving over it.
        let name = if brushes.is_builtin(group, name) || group.is_empty() { String::new() } else { name.to_string() };
        self.naming = Some((Naming::Save, name, true));
    }

    /// Show the presets, the one in hand marked, and under them the brush in hand.
    /// `strokes_only` lists the presets smaller and without their names; `settings` is whether
    /// the brush's settings are unfolded. Returns whether a preset was taken, and what is to
    /// be written to a file.
    /// A part of the bar over the Brushes panel: how the presets are shown, the presets' own
    /// undo, or their search.
    pub fn bar_part(&mut self, ui: &mut egui::Ui, part: &str, brushes: &mut Brushes, strokes_only: &mut bool) {
        match part {
            // Each name over its stroke, or the strokes alone.
            "brushview" => {
                ui.selectable_value(strokes_only, false, "Names").on_hover_text("Show each brush's name over a stroke drawn by it");
                ui.selectable_value(strokes_only, true, "Strokes")
                    .on_hover_text("Show the strokes alone, smaller, to fit more; a brush's name is said under the pointer");
            }
            "brushundo" => {
                let last = self.undo.last().map(|(what, _)| what.clone());
                let undo = ui.add_enabled(last.is_some(), egui::Button::new("Undo"));
                let undo = match &last {
                    Some(what) => undo.on_hover_text(format!("Undo: {what}. The presets are yours, not the project's, so they have an undo of their own")),
                    None => undo.on_disabled_hover_text("Nothing was done to the presets yet"),
                };
                if undo.clicked() {
                    self.act(brushes, Act::Undo, false, &mut Shown::default());
                }
            }
            "brushsearch" => drop(ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search brushes").desired_width(140.0))),
            _ => {}
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, brushes: &mut Brushes, kind: BrushKind, strokes_only: &mut bool, settings: &mut bool) -> Shown {
        self.kind = kind;
        let mut acts = Vec::new();
        self.name_field(ui, brushes);
        self.question(ui, &mut acts);
        if let Some(problem) = &self.problem
            && ui
                .add(egui::Label::new(egui::RichText::new(problem).color(ui.visuals().error_fg_color)).wrap().sense(Sense::click()))
                .on_hover_text("Click to put this away")
                .clicked()
        {
            self.problem = None;
        }
        // The brush in hand, at the foot of the panel. Folded and unfolded are two panels, so
        // that the height it was dragged to is kept for when it is unfolded.
        let foot = egui::Panel::bottom(ui.id().with(("brush in hand", *settings))).resizable(*settings);
        let foot = if *settings { foot.default_size(ui.available_height() * 0.6).min_size(80.0) } else { foot };
        foot.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.toggle_value(settings, "Settings").on_hover_text("The settings of the brush in hand");
                if ui.button("Save as Preset…").on_hover_text("Keep the brush in hand under a name").clicked() {
                    acts.push(Act::Name(Naming::Save));
                }
                ui.add(egui::Label::new(egui::RichText::new(brushes.in_hand.named(kind).0).weak()).truncate());
            });
            if *settings {
                match kind {
                    BrushKind::Raster => self.brush_in_hand(ui, &mut brushes.in_hand.raster.brush),
                    BrushKind::Vector => self.vector_in_hand(ui, &mut brushes.in_hand.vector.brush),
                }
            }
        });
        self.list(ui, brushes, *strokes_only, &mut acts);

        let mut shown = Shown::default();
        for act in acts {
            self.act(brushes, act, false, &mut shown);
        }
        shown
    }

    /// Do what was asked of the presets. A delete is only asked about, until it comes again
    /// as `answered`.
    fn act(&mut self, brushes: &mut Brushes, act: Act, answered: bool, shown: &mut Shown) {
        if let Act::Answered(asked) = act {
            return self.act(brushes, *asked, true, shown);
        }
        if matches!(act, Act::Delete(..) | Act::DeleteGroup(_)) && !answered {
            self.asking = Some(act);
            return;
        }
        // The pictures are by group and name, which every one of these but taking changes.
        if !matches!(act, Act::Take(..) | Act::Name(_) | Act::Export(..) | Act::Slot(..)) {
            self.pictures.clear();
        }
        let before = brushes.clone();
        // What it is called in Undo, if it changes the presets.
        let done = match act {
            Act::Take(group, name) => {
                let preset = brushes.listed().find(|preset| preset.listed_group() == group && preset.name == name).cloned();
                if let Some(preset) = preset {
                    brushes.take(&preset);
                    shown.taken = true;
                }
                return;
            }
            Act::Name(Naming::Save) => return self.ask_save(brushes, self.kind),
            Act::Name(naming) => {
                let text = match &naming {
                    Naming::Rename(_, name) => name.clone(),
                    Naming::Group(group) => group.clone(),
                    _ => String::new(),
                };
                self.naming = Some((naming, text, true));
                return;
            }
            Act::Export(group, name) => {
                let of = |preset: &&BrushPreset| preset.listed_group() == group && name.as_ref().is_none_or(|name| preset.name == *name);
                let presets: Vec<BrushPreset> = brushes.listed().filter(of).cloned().collect();
                shown.export = (!presets.is_empty()).then(|| (name.unwrap_or(group), presets));
                return;
            }
            Act::Undo => {
                if let Some((_, before)) = self.undo.pop() {
                    brushes.restore(&before);
                }
                return;
            }
            Act::Slot(group, name, slot) => return brushes.set_slot(&group, &name, slot),
            Act::MoveTo(group, name, to) => brushes.move_to(&group, &name, &to).map(|_| format!("Move {name} to {to}")),
            Act::Duplicate(group, name) => brushes.duplicate(&group, &name).map(|_| format!("Duplicate {name}")),
            Act::Delete(group, name) => brushes.delete(&group, &name).then(|| format!("Delete {name}")),
            Act::DeleteGroup(group) => (brushes.delete_group(&group) > 0).then(|| format!("Delete the group {group}")),
            Act::Place(group, name, to, before) => brushes.place(&group, &name, &to, before.as_deref()).map(|_| format!("Move {name}")),
            Act::PlaceGroup(group, before) => {
                brushes.place_group(&group, before.as_deref());
                Some(format!("Move the group {group}"))
            }
            Act::Restore => {
                brushes.restore_builtin();
                Some("Restore the engine's brushes".to_string())
            }
            Act::Answered(_) => None,
        };
        if let Some(done) = done {
            self.done(done, before, brushes);
        }
    }

    /// Keep the presets as they were `before` what was `done`, to undo it by: as many steps as
    /// are not too heavy together, a preset with a sampled tip being large.
    fn done(&mut self, done: String, before: Brushes, now: &Brushes) {
        if before.presets == now.presets && before.hidden == now.hidden {
            return;
        }
        self.undo.push((done, before));
        let steps = (UNDO_WEIGHT / now.weight().max(1)).clamp(1, UNDO_STEPS);
        let over = self.undo.len().saturating_sub(steps);
        self.undo.drain(..over);
    }

    /// The question before a delete: what would go, and the two answers. Enter deletes and
    /// Escape leaves it.
    fn question(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let said = match &self.asking {
            Some(Act::Delete(_, name)) => format!("Delete “{name}”?"),
            Some(Act::DeleteGroup(group)) => format!("Delete the group “{group}” with all its brushes?"),
            _ => return,
        };
        let (mut yes, mut no) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        ui.horizontal_wrapped(|ui| {
            ui.label(said);
            yes |= ui.button("Delete").clicked();
            no |= ui.button("Cancel").clicked();
        });
        if no {
            self.asking = None;
        } else if yes {
            acts.extend(self.asking.take().map(|act| Act::Answered(Box::new(act))));
        }
    }

    /// The field a name is typed into. Enter takes the name, Escape gives it up.
    fn name_field(&mut self, ui: &mut egui::Ui, brushes: &mut Brushes) {
        let Some((naming, text, focus)) = self.naming.as_mut() else { return };
        let hint = match naming {
            Naming::Save => "The new preset's name",
            Naming::Rename(..) => "The preset's new name",
            Naming::Move(..) => "The group to move it to",
            Naming::Group(_) => "The group's new name",
        };
        let field = ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(f32::INFINITY));
        if std::mem::take(focus) {
            field.request_focus();
        }
        if !field.lost_focus() {
            return;
        }
        let keep = !ui.input(|i| i.key_pressed(egui::Key::Escape));
        if let Some((naming, text, _)) = self.naming.take().filter(|_| keep) {
            let before = brushes.clone();
            let done = match naming {
                // A preset of the user's that is in hand is saved where it is.
                Naming::Save => {
                    let (name, group) = brushes.in_hand.named(self.kind);
                    let group = if brushes.is_builtin(group, name) { String::new() } else { group.to_string() };
                    brushes.save_in_hand(&text, &group, self.kind).map(|name| format!("Save {name}"))
                }
                Naming::Rename(group, name) => brushes.rename(&group, &name, &text).map(|to| format!("Rename {name} to {to}")),
                Naming::Move(group, name) => brushes.move_to(&group, &name, &text).map(|_| format!("Move {name} to {}", text.trim())),
                Naming::Group(group) => (brushes.rename_group(&group, &text) > 0).then(|| format!("Rename the group {group}")),
            };
            if let Some(done) = done {
                self.done(done, before, brushes);
            }
            self.pictures.clear();
        }
    }

    /// The presets, group by group.
    fn list(&mut self, ui: &mut egui::Ui, brushes: &Brushes, strokes_only: bool, acts: &mut Vec<Act>) {
        let words: Vec<String> = self.search.split_whitespace().map(str::to_lowercase).collect();
        let found = |preset: &&BrushPreset| {
            let name = preset.name.to_lowercase();
            words.iter().all(|word| name.contains(word))
        };
        let kind = self.kind;
        let presets: Vec<&BrushPreset> = brushes.listed_of(kind).filter(found).collect();
        // The presets of the other kind, which can't draw here.
        let others: Vec<&BrushPreset> = brushes.listed().filter(|preset| preset.brush.kind() != kind).filter(found).collect();
        let mut groups: Vec<&str> = Vec::new();
        for preset in &presets {
            if !groups.contains(&preset.listed_group()) {
                groups.push(preset.listed_group());
            }
        }
        // The groups a preset can be moved to: the user's own.
        let own_groups = brushes.own_groups();
        #[cfg(test)]
        {
            self.drawn.clear();
            self.headers.clear();
        }
        if presets.is_empty() && others.is_empty() {
            ui.weak(if words.is_empty() { "Every brush was deleted." } else { "No brush has that name." });
            if !brushes.hidden.is_empty() && ui.button("Restore the Engine's Brushes").clicked() {
                acts.push(Act::Restore);
            }
            return;
        }
        let (held_name, held_group) = brushes.in_hand.named(kind);
        let mut made = 0;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for group in groups {
                // A search opens every group that has a match.
                let open = (!words.is_empty()).then_some(true);
                let header = egui::CollapsingHeader::new(group).default_open(true).open(open).show(ui, |ui| {
                    let of_group = presets.iter().filter(|preset| preset.listed_group() == group);
                    let list = |ui: &mut egui::Ui| {
                        for preset in of_group {
                            let in_hand = preset.name == held_name && (preset.group == held_group || preset.listed_group() == held_group);
                            let response = self.row(ui, preset, in_hand, strokes_only, &mut made);
                            // The slot a brush has is written in its corner.
                            if let Some(slot) = brushes.slot_of(group, &preset.name) {
                                let theme = Theme::of(ui.ctx());
                                let at = response.rect.right_top() + vec2(-4.0, 2.0);
                                ui.painter().text(at, egui::Align2::RIGHT_TOP, format!("{}", slot + 1), egui::FontId::proportional(10.0), theme.accent);
                            }
                            if response.clicked() {
                                acts.push(Act::Take(group.to_string(), preset.name.clone()));
                            }
                            let (at, own) = ((group.to_string(), preset.name.clone()), !brushes.is_builtin(group, &preset.name));
                            // One of the user's is dragged to another place; what is let go on
                            // one of the user's goes before it, into its group.
                            if own {
                                response.dnd_set_drag_payload(Dragged::Preset(at.0.clone(), at.1.clone()));
                            }
                            if own && response.dnd_hover_payload::<Dragged>().is_some_and(|dragged| matches!(&*dragged, Dragged::Preset(..))) {
                                let line = [response.rect.left_top(), if strokes_only { response.rect.left_bottom() } else { response.rect.right_top() }];
                                ui.painter().line_segment(line, egui::Stroke::new(2.0, Theme::of(ui.ctx()).accent));
                            }
                            if let Some(dragged) = response.dnd_release_payload::<Dragged>()
                                && let (true, Dragged::Preset(from, name)) = (own, &*dragged)
                            {
                                acts.push(Act::Place(from.clone(), name.clone(), at.0.clone(), Some(at.1.clone())));
                            }
                            response.context_menu(|ui| {
                                let named = |naming: fn(String, String) -> Naming| Act::Name(naming(at.0.clone(), at.1.clone()));
                                if ui
                                    .add_enabled(own, egui::Button::new("Rename…"))
                                    .on_disabled_hover_text("The engine's brushes keep their names. Duplicate it to have one of your own")
                                    .clicked()
                                {
                                    acts.push(named(Naming::Rename));
                                }
                                if ui.button("Duplicate").clicked() {
                                    acts.push(Act::Duplicate(at.0.clone(), at.1.clone()));
                                }
                                ui.add_enabled_ui(own, |ui| {
                                    ui.menu_button("Move to", |ui| {
                                        for to in own_groups.iter().filter(|to| **to != group) {
                                            if ui.button(*to).clicked() {
                                                acts.push(Act::MoveTo(at.0.clone(), at.1.clone(), to.to_string()));
                                            }
                                        }
                                        if ui.button("A New Group…").clicked() {
                                            acts.push(named(Naming::Move));
                                        }
                                    });
                                });
                                // A slot, by which a command takes the brush: Drawing ▸ Brush in
                                // Slot 1 and so on, each with a shortcut of its own in the keymap.
                                let slot = brushes.slot_of(&at.0, &at.1);
                                ui.menu_button("Slot", |ui| {
                                    ui.weak("A brush in a slot is taken by its command,\nDrawing › Brush in Slot…, which can have a shortcut.");
                                    for number in 0..iskra_drawing::BRUSH_SLOTS {
                                        let held = brushes.in_slot(number).map(|preset| preset.name.as_str());
                                        let title = match held {
                                            Some(held) if slot != Some(number) => format!("Slot {} (now {held})", number + 1),
                                            _ => format!("Slot {}", number + 1),
                                        };
                                        if ui.radio(slot == Some(number), title).clicked() {
                                            acts.push(Act::Slot(at.0.clone(), at.1.clone(), Some(number)));
                                            ui.close();
                                        }
                                    }
                                    if ui.radio(slot.is_none(), "None").clicked() {
                                        acts.push(Act::Slot(at.0.clone(), at.1.clone(), None));
                                        ui.close();
                                    }
                                });
                                if ui.button("Export…").on_hover_text("Writes this brush to a file of its own").clicked() {
                                    acts.push(Act::Export(at.0.clone(), Some(at.1.clone())));
                                }
                                if ui.button("Delete…").clicked() {
                                    acts.push(Act::Delete(at.0.clone(), at.1.clone()));
                                }
                                ui.separator();
                                if ui.add_enabled(!brushes.hidden.is_empty(), egui::Button::new("Restore the Engine's Brushes")).clicked() {
                                    acts.push(Act::Restore);
                                }
                            });
                        }
                    };
                    if strokes_only {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                            list(ui);
                        });
                    } else {
                        list(ui);
                    }
                });
                // A group's name: one of the user's is dragged before another, a preset let go
                // on it goes to the end of the group, and a right-click has the rest.
                let (name, owned) = (header.header_response, own_groups.contains(&group));
                #[cfg(test)]
                self.headers.push((group.to_string(), name.rect));
                // Laid over the name, it has its clicks too: a click folds the group.
                let grip = ui.interact(name.rect, name.id.with("grip"), Sense::click_and_drag());
                if grip.clicked()
                    && let Some(mut state) = egui::collapsing_header::CollapsingState::load(ui.ctx(), name.id)
                {
                    state.toggle(ui);
                    state.store(ui.ctx());
                }
                if owned {
                    grip.dnd_set_drag_payload(Dragged::Group(group.to_string()));
                }
                if let Some(dragged) = grip.dnd_hover_payload::<Dragged>()
                    && (owned || matches!(&*dragged, Dragged::Preset(..)))
                {
                    ui.painter().rect_stroke(name.rect, 3.0, egui::Stroke::new(1.5, Theme::of(ui.ctx()).accent), egui::StrokeKind::Inside);
                }
                if let Some(dragged) = grip.dnd_release_payload::<Dragged>() {
                    match &*dragged {
                        Dragged::Preset(from, preset) => acts.push(Act::Place(from.clone(), preset.clone(), group.to_string(), None)),
                        Dragged::Group(other) if owned => acts.push(Act::PlaceGroup(other.clone(), Some(group.to_string()))),
                        Dragged::Group(_) => {}
                    }
                }
                grip.context_menu(|ui| {
                    if ui
                        .add_enabled(owned, egui::Button::new("Rename Group…"))
                        .on_hover_text("Your brushes in it go to the group of the new name")
                        .on_disabled_hover_text("The engine's groups keep their names")
                        .clicked()
                    {
                        acts.push(Act::Name(Naming::Group(group.to_string())));
                    }
                    if ui.button("Export Group…").on_hover_text("Writes the brushes of this group to a file").clicked() {
                        acts.push(Act::Export(group.to_string(), None));
                    }
                    if ui.button("Delete Group…").on_hover_text("Deletes every brush in it. The engine's come back with Restore the Engine's Brushes").clicked()
                    {
                        acts.push(Act::DeleteGroup(group.to_string()));
                    }
                });
            }
            // The other kind's presets: seen, greyed, not taken.
            if !others.is_empty() {
                let title = match kind {
                    BrushKind::Vector => "Raster Brushes, for Pixels",
                    BrushKind::Raster => "Vector Brushes, for Vector Art",
                };
                let header = egui::CollapsingHeader::new(title).id_salt("other kind").default_open(false).open((!words.is_empty()).then_some(true));
                let header = header.show(ui, |ui| {
                    ui.add_enabled_ui(false, |ui| {
                        for preset in others {
                            self.row(ui, preset, false, strokes_only, &mut made);
                        }
                    });
                });
                let hint = match kind {
                    BrushKind::Vector => "The art layer in hand is vector art: these paint pixels, and are taken on an art layer of pixels",
                    BrushKind::Raster => "These draw vector strokes, and are taken on an art layer that is vector art",
                };
                header.header_response.on_hover_text(hint);
            }
        });
        if made == MADE_A_FRAME {
            ui.ctx().request_repaint();
        }
    }

    /// One preset: its name over its stroke, or the stroke alone.
    fn row(&mut self, ui: &mut egui::Ui, preset: &BrushPreset, in_hand: bool, strokes_only: bool, made: &mut usize) -> egui::Response {
        let theme = Theme::of(ui.ctx());
        let aspect = PICTURE[1] as f32 / PICTURE[0] as f32;
        let (size, picture_at) = if strokes_only {
            (vec2(96.0, 96.0 * aspect + 6.0), vec2(3.0, 3.0))
        } else {
            let width = ui.available_width().max(60.0);
            (vec2(width, 16.0 + (width - 8.0).min(PICTURE[0] as f32 / 2.0) * aspect + 4.0), vec2(4.0, 16.0))
        };
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        #[cfg(test)]
        self.drawn.push((preset.name.clone(), rect));
        if in_hand || response.hovered() {
            ui.painter().rect_filled(rect, 4.0, if in_hand { theme.accent.gamma_multiply(0.35) } else { theme.surface_raised });
        }
        if !strokes_only {
            let name = egui::Align2::LEFT_TOP;
            ui.painter().with_clip_rect(rect).text(rect.min + vec2(6.0, 2.0), name, &preset.name, egui::FontId::proportional(11.0), theme.text);
        }
        if ui.is_rect_visible(rect) {
            let key = (preset.listed_group().to_string(), preset.name.clone());
            if !self.pictures.contains_key(&key) && *made < MADE_A_FRAME {
                *made += 1;
                self.pictures.insert(key.clone(), ui.ctx().load_texture("brush stroke", picture(&preset.brush), TextureOptions::LINEAR));
            }
            if let Some(texture) = self.pictures.get(&key) {
                let width = (rect.width() - 2.0 * picture_at.x).min(PICTURE[0] as f32 / 2.0);
                let area = Rect::from_min_size(rect.min + picture_at, vec2(width, width * aspect));
                ui.painter().image(texture.id(), area, WHOLE, theme.text);
            }
        }
        if strokes_only { response.on_hover_text(&preset.name) } else { response }
    }

    /// The brush in hand: a stroke that tries it, and its settings in the engine's sections.
    fn brush_in_hand(&mut self, ui: &mut egui::Ui, brush: &mut BrushSettings) {
        let theme = Theme::of(ui.ctx());
        let wanted = Brush::Raster(brush.clone());
        if self.trial.as_ref().is_none_or(|trial| trial.brush != wanted) {
            let stroke = ui.ctx().load_texture("brush trial", picture(&wanted), TextureOptions::LINEAR);
            let tip = match &brush.tip {
                TipShape::Sampled(tile) if tile.width > 0 && tile.height > 0 => {
                    let mut image = egui::ColorImage::filled([TIP_SIDE, TIP_SIDE], Color32::TRANSPARENT);
                    // The tip keeps its proportions, in the middle of the square.
                    let side = tile.width.max(tile.height) as usize;
                    let (left, top) = ((side - tile.width as usize) / 2, (side - tile.height as usize) / 2);
                    for (index, pixel) in image.pixels.iter_mut().enumerate() {
                        let (x, y) = ((index % TIP_SIDE * side / TIP_SIDE).checked_sub(left), (index / TIP_SIDE * side / TIP_SIDE).checked_sub(top));
                        if let (Some(x), Some(y)) = (x.filter(|x| *x < tile.width as usize), y.filter(|y| *y < tile.height as usize)) {
                            *pixel = Color32::from_white_alpha((tile.get(x as u32, y as u32) * 255.0) as u8);
                        }
                    }
                    Some(ui.ctx().load_texture("brush tip", image, TextureOptions::LINEAR))
                }
                _ => None,
            };
            self.trial = Some(Trial { brush: wanted, stroke, tip });
            #[cfg(test)]
            {
                self.trials += 1;
            }
        }
        let Some(trial) = &self.trial else { return };
        let width = ui.available_width().min(PICTURE[0] as f32 / 2.0);
        let (area, _) = ui.allocate_exact_size(vec2(width, width * PICTURE[1] as f32 / PICTURE[0] as f32), Sense::hover());
        ui.painter().image(trial.stroke.id(), area, WHOLE, theme.text);
        let tip = trial.tip.as_ref().map(TextureHandle::id);

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::CollapsingHeader::new("Tip").default_open(true).show(ui, |ui| {
                ui.add(egui::Slider::new(&mut brush.size, 1.0..=MAX_SIZE).logarithmic(true).text("Size"));
                match (&brush.tip, tip) {
                    (TipShape::Sampled(tile), Some(tip)) => {
                        ui.horizontal(|ui| {
                            let (area, _) = ui.allocate_exact_size(vec2(TIP_SIDE as f32, TIP_SIDE as f32), Sense::hover());
                            ui.painter().image(tip, area, WHOLE, theme.text);
                            ui.weak(format!("A sampled tip,\n{} × {} pixels", tile.width, tile.height));
                        });
                    }
                    _ => drop(percent(ui, &mut brush.hardness, 0.0, 1.0, "Hardness")),
                }
                // The tip's shape: a picture's, or round again.
                ui.horizontal(|ui| {
                    let hint = "The tip takes the shape of a picture: what is dark and solid in it paints, white and what is see-through does not";
                    if ui.button("Tip from a Picture…").on_hover_text(hint).clicked()
                        && let Some(tile) = self.tile_from_picture(true)
                    {
                        brush.tip = TipShape::Sampled(tile);
                    }
                    let round = matches!(brush.tip, TipShape::Round);
                    if ui.add_enabled(!round, egui::Button::new("Round Tip")).on_hover_text("A round tip again, with a hardness").clicked() {
                        brush.tip = TipShape::Round;
                    }
                });
                percent(ui, &mut brush.roundness, 0.01, 1.0, "Roundness");
                ui.add(egui::Slider::new(&mut brush.angle, -180.0..=180.0).suffix("°").text("Angle"));
                percent(ui, &mut brush.spacing, 0.01, 2.0, "Spacing");
                ui.checkbox(&mut brush.spacing_enabled, "Even spacing").on_hover_text("Off: the pen's speed sets how far apart the tip is stamped");
                ui.checkbox(&mut brush.pressure_size, "Pen pressure sets the size");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut brush.flip_x, "Flip across");
                    ui.checkbox(&mut brush.flip_y, "Flip down");
                });
                ui.checkbox(&mut brush.aliased, "Hard pixels").on_hover_text("A pencil's edge: a pixel is painted or it is not");
                ui.checkbox(&mut brush.noise, "Grain in the soft parts");
            });
            let dynamics = &mut brush.shape_dynamics;
            section(ui, "Shape Dynamics", &mut dynamics.enabled, |ui| {
                dynamic(ui, "Size", &mut dynamics.size, Some("Smallest size"), false, 1.0);
                // How flat a tilted pen makes the tip, where the size follows the pen's tilt.
                let tilted = dynamics.size.control == Control::PenTilt;
                ui.add_enabled_ui(tilted, |ui| {
                    percent(ui, &mut dynamics.tilt_scale, 0.0, 2.0, "Tilt scale")
                        .on_hover_text("How flat the tip is made by a pen held at a slant: none at 0%, flat at 200%")
                        .on_disabled_hover_text("For a size that follows the pen's tilt");
                });
                dynamic(ui, "Angle", &mut dynamics.angle, None, true, 1.0);
                dynamic(ui, "Roundness", &mut dynamics.roundness, Some("Least roundness"), false, 1.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut dynamics.flip_x_jitter, "Flips across by chance");
                    ui.checkbox(&mut dynamics.flip_y_jitter, "Flips down by chance");
                });
                ui.checkbox(&mut dynamics.brush_projection, "The pen's slant shapes the tip")
                    .on_hover_text("Brush projection: the tip is made shorter the way the pen leans, and turns as the pen is turned in the hand");
            });
            let scattering = &mut brush.scattering;
            section(ui, "Scattering", &mut scattering.enabled, |ui| {
                dynamic(ui, "Scatter", &mut scattering.scatter, None, false, 10.0);
                ui.checkbox(&mut scattering.both_axes, "Along the stroke too");
                ui.add(egui::Slider::new(&mut scattering.count, 1..=16).text("Count"));
                dynamic(ui, "Count", &mut scattering.count_jitter, None, false, 1.0);
            });
            self.texture_section(ui, &mut brush.texture, true);
            dual_section(ui, &mut brush.dual_brush);
            egui::CollapsingHeader::new("Transfer").show(ui, |ui| {
                percent(ui, &mut brush.opacity, 0.0, 1.0, "Opacity").on_hover_text("The most a whole stroke covers");
                percent(ui, &mut brush.flow, 0.0, 1.0, "Flow").on_hover_text("How much each stamp of the tip covers");
                ui.checkbox(&mut brush.pressure_opacity, "Pen pressure sets the opacity");
                ui.checkbox(&mut brush.build_up, "Builds up while the pen rests").on_hover_text("An airbrush: paint keeps coming while the pen is held still");
                ui.add_enabled(brush.build_up, egui::Slider::new(&mut brush.build_up_rate, 1.0..=100.0).text("Stamps a second"));
                let transfer = &mut brush.transfer;
                ui.checkbox(&mut transfer.enabled, "Opacity and flow vary");
                ui.add_enabled_ui(transfer.enabled, |ui| {
                    dynamic(ui, "Opacity", &mut transfer.opacity, Some("Least opacity"), false, 1.0);
                    dynamic(ui, "Flow", &mut transfer.flow, Some("Least flow"), false, 1.0);
                });
            });
            egui::CollapsingHeader::new("Smoothing").show(ui, |ui| {
                let smoothing = &mut brush.smoothing;
                percent(ui, &mut smoothing.amount, 0.0, 1.0, "Smoothing")
                    .on_hover_text("Stabiliser: the line follows the pen more slowly and evens out jitter");
                ui.checkbox(&mut smoothing.pulled_string, "Pulled string").on_hover_text("The line is drawn only when the pen pulls a string taut");
                ui.checkbox(&mut smoothing.catch_up, "Catches up while the pen rests");
                ui.checkbox(&mut smoothing.catch_up_on_end, "Ends where the pen lifts");
            });
            let pose = &mut brush.pose;
            section(ui, "Brush Pose", &mut pose.enabled, |ui| {
                ui.weak("How the pen is taken to be held, where the pen itself does not say, a mouse say; or in place of what it says.");
                ui.add(egui::Slider::new(&mut pose.tilt_x, -90.0..=90.0).suffix("°").text("Slant across"));
                ui.checkbox(&mut pose.override_tilt_x, "In place of the pen's own slant across");
                ui.add(egui::Slider::new(&mut pose.tilt_y, -90.0..=90.0).suffix("°").text("Slant down"));
                ui.checkbox(&mut pose.override_tilt_y, "In place of the pen's own slant down");
                ui.add(egui::Slider::new(&mut pose.rotation, 0.0..=360.0).suffix("°").text("Turned"));
                ui.checkbox(&mut pose.override_rotation, "In place of the pen's own turn");
                percent(ui, &mut pose.pressure, 0.0, 1.0, "Pressure");
                ui.checkbox(&mut pose.override_pressure, "In place of the pen's own pressure");
            });
            egui::CollapsingHeader::new("Colour").show(ui, |ui| {
                ui.weak(
                    "For free-colour art layers (Free Colour in the brush's bar), where colours mix inside a pixel. \
                     A line or colour art layer that follows the palette holds one swatch a pixel: there a brush paints with the rest of what it is.",
                );
                ui.checkbox(&mut brush.wet_edges, "Wet edges").on_hover_text("Paint gathers at the edge of the stroke, as watercolour does");
                let colour = &mut brush.color_dynamics;
                section(ui, "Colour varies", &mut colour.enabled, |ui| {
                    ui.checkbox(&mut colour.per_tip, "With every stamp of the tip").on_hover_text("Off: once for the whole stroke");
                    percent(ui, &mut colour.hue_jitter, 0.0, 1.0, "Hue by chance");
                    percent(ui, &mut colour.saturation_jitter, 0.0, 1.0, "Saturation by chance");
                    percent(ui, &mut colour.brightness_jitter, 0.0, 1.0, "Brightness by chance");
                    percent(ui, &mut colour.purity, -1.0, 1.0, "Purity").on_hover_text("Less or more saturated than the swatch");
                });
            });
            // What of the brush in hand stays when another preset is taken.
            let locks = &mut brush.locks;
            let kept = [
                locks.shape_dynamics,
                locks.scattering,
                locks.texture,
                locks.dual_brush,
                locks.color_dynamics,
                locks.transfer,
                locks.pose,
                locks.noise,
                locks.wet_edges,
                locks.build_up,
            ];
            let count = kept.iter().filter(|kept| **kept).count() + usize::from(brush.protect_texture);
            let heading = if count > 0 { format!("Kept from Brush to Brush ({count})") } else { "Kept from Brush to Brush".to_string() };
            egui::CollapsingHeader::new(heading).id_salt("locks").show(ui, |ui| {
                ui.weak("What is ticked stays as it is here when another brush is taken, where it would be that brush's. Smoothing always stays.");
                for (lock, name) in [
                    (&mut locks.shape_dynamics, "Shape dynamics"),
                    (&mut locks.scattering, "Scattering"),
                    (&mut locks.texture, "Texture"),
                    (&mut locks.dual_brush, "Dual brush"),
                    (&mut locks.transfer, "Transfer"),
                    (&mut locks.pose, "Brush pose"),
                    (&mut locks.color_dynamics, "Colour varies"),
                    (&mut locks.noise, "Grain in the soft parts"),
                    (&mut locks.wet_edges, "Wet edges"),
                    (&mut locks.build_up, "Builds up while the pen rests"),
                ] {
                    ui.checkbox(lock, name);
                }
                ui.checkbox(&mut brush.protect_texture, "The texture's pattern and scale").on_hover_text(
                    "Another brush with a texture then paints with this one's pattern, at this one's scale, and its own depth and way of cutting",
                );
            });
        });
    }

    /// The vector brush in hand: a stroke that tries it, and its settings (plan §4.16).
    fn vector_in_hand(&mut self, ui: &mut egui::Ui, brush: &mut VectorBrush) {
        let theme = Theme::of(ui.ctx());
        let wanted = Brush::Vector(brush.clone());
        if self.trial.as_ref().is_none_or(|trial| trial.brush != wanted) {
            let stroke = ui.ctx().load_texture("brush trial", picture(&wanted), TextureOptions::LINEAR);
            self.trial = Some(Trial { brush: wanted, stroke, tip: None });
            #[cfg(test)]
            {
                self.trials += 1;
            }
        }
        let Some(trial) = &self.trial else { return };
        let width = ui.available_width().min(PICTURE[0] as f32 / 2.0);
        let (area, _) = ui.allocate_exact_size(vec2(width, width * PICTURE[1] as f32 / PICTURE[0] as f32), Sense::hover());
        ui.painter().image(trial.stroke.id(), area, WHOLE, theme.text);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::CollapsingHeader::new("Width").default_open(true).show(ui, |ui| {
                ui.add(egui::Slider::new(&mut brush.size, 1.0..=MAX_SIZE).logarithmic(true).text("Size"));
                ui.checkbox(&mut brush.pressure_size, "Pressure").on_hover_text("The width follows the pen's pressure");
                ui.add_enabled_ui(brush.pressure_size, |ui| {
                    percent(ui, &mut brush.thinnest, 0.0, 1.0, "Thinnest").on_hover_text("The width with the lightest touch, of the size");
                    // The pressure going in from left to right, the width coming out from the
                    // thinnest at the bottom to the size at the top.
                    ui.label("Pressure Curve").on_hover_text("How the width answers the pen: above the line a light touch is wider, below it narrower");
                    let curves = iskra_model::Curves { master: brush.pressure_curve.clone(), ..iskra_model::Curves::default() };
                    if let (Some(changed), _) = self.pressure.show(ui, &curves) {
                        brush.pressure_curve = if changed.master == iskra_model::STRAIGHT { Vec::new() } else { changed.master };
                    }
                    if !brush.pressure_curve.is_empty() && ui.small_button("Straight").on_hover_text("The width as the pressure").clicked() {
                        brush.pressure_curve.clear();
                        self.pressure.picked = None;
                    }
                });
                let hint = "How long the stroke takes to grow to its width at its start, in widths. 0 is a round end";
                ui.add(egui::Slider::new(&mut brush.taper_start, 0.0..=20.0).text("Taper at Start")).on_hover_text(hint);
                let hint = "How long the stroke takes to narrow to a point at its end, in widths. 0 is a round end";
                ui.add(egui::Slider::new(&mut brush.taper_end, 0.0..=20.0).text("Taper at End")).on_hover_text(hint);
            });
            egui::CollapsingHeader::new("Opacity").default_open(true).show(ui, |ui| {
                let hint = "The opacity follows the pen's pressure, through the pressure curve above: a lighter touch is fainter. \
                            Faint strokes build up where they overlap";
                ui.checkbox(&mut brush.pressure_opacity, "Pressure").on_hover_text(hint);
                ui.add_enabled_ui(brush.pressure_opacity, |ui| {
                    percent(ui, &mut brush.faintest, 0.0, 1.0, "Faintest").on_hover_text("The opacity with the lightest touch");
                });
                let hint = "How long the stroke takes to come up to its opacity at its start, in widths. 0 is none";
                ui.add(egui::Slider::new(&mut brush.fade_start, 0.0..=20.0).text("Fade at Start")).on_hover_text(hint);
                let hint = "How long the stroke takes to fade out at its end, in widths. 0 is none";
                ui.add(egui::Slider::new(&mut brush.fade_end, 0.0..=20.0).text("Fade at End")).on_hover_text(hint);
            });
            egui::CollapsingHeader::new("Colour Drift").default_open(true).show(ui, |ui| {
                let hint = "How far the stroke's colour drifts towards its swatch's drift colour, which the swatch's window sets. \
                            A swatch with none keeps its colour";
                percent(ui, &mut brush.drift, 0.0, 1.0, "Drift").on_hover_text(hint);
                ui.add_enabled_ui(brush.drift > 0.0, |ui| {
                    let hint = "The drift follows the pen's pressure, through the pressure curve: the harder it presses, the further the colour drifts";
                    ui.checkbox(&mut brush.pressure_drift, "Pressure").on_hover_text(hint);
                });
            });
            egui::CollapsingHeader::new("Boil").default_open(true).show(ui, |ui| {
                let hint = "How far the stroke's edge moves as it boils, in pixels: drawn anew three ways in turn as the frames go, \
                            as hand-drawn lines shimmer. Its grain is laid anew too. 0 is still";
                ui.add(egui::Slider::new(&mut brush.boil, 0.0..=8.0).suffix(" px").text("Boil")).on_hover_text(hint);
                ui.add_enabled_ui(brush.boil > 0.0, |ui| {
                    let hint = "How many frames each way is held before the next";
                    ui.add(egui::Slider::new(&mut brush.boil_every, 1..=4).text("Every")).on_hover_text(hint);
                });
            });
            egui::CollapsingHeader::new("Nib").default_open(true).show(ui, |ui| {
                let hint = "Round, or an oval: a calligraphic pen, thick where the stroke goes across its angle and thin along it";
                percent(ui, &mut brush.roundness, 0.05, 1.0, "Roundness").on_hover_text(hint);
                ui.add_enabled_ui(brush.roundness < 1.0, |ui| {
                    ui.add(egui::Slider::new(&mut brush.angle, -90.0..=90.0).suffix("°").text("Angle"));
                });
            });
            egui::CollapsingHeader::new("Line").default_open(true).show(ui, |ui| {
                percent(ui, &mut brush.smoothing, 0.0, 1.0, "Smoothing").on_hover_text("The line follows the pen more slowly and evens out jitter");
                let hint = "How far the kept line may stray from the pen, in pixels of the screen: more keeps fewer points, and smoother curves";
                ui.add(egui::Slider::new(&mut brush.simplify, 0.05..=5.0).logarithmic(true).text("Simplify")).on_hover_text(hint);
            });
            egui::CollapsingHeader::new("Edge").default_open(true).show(ui, |ui| {
                let hint = "How far the edge of a surface goes in and out, of half its width: a rough edge, as of ink on paper or a crayon. \
                            The Brush set to leave surfaces makes it; lines keep a smooth edge";
                percent(ui, &mut brush.roughness, 0.0, 0.9, "Roughness").on_hover_text(hint);
                ui.add_enabled_ui(brush.roughness > 0.0, |ui| {
                    let hint = "How long a bump of the rough edge is, in widths of the brush: less is a finer grain";
                    ui.add(egui::Slider::new(&mut brush.bumps, 0.05..=4.0).logarithmic(true).text("Bumps")).on_hover_text(hint);
                });
            });
            egui::CollapsingHeader::new("Bristles").default_open(true).show(ui, |ui| {
                let hint = "A dry brush: a surface made of this many strands side by side, which break up where the paint runs out. \
                            Fewer than 2 is a brush full of paint. The Brush set to leave surfaces makes it; lines are one";
                ui.add(egui::Slider::new(&mut brush.bristles, 0..=16).text("Bristles")).on_hover_text(hint);
                ui.add_enabled_ui(brush.bristles >= 2, |ui| {
                    let hint = "How soon the strands break up: as the stroke goes on, and where it is thinner, as with a lighter touch";
                    percent(ui, &mut brush.dryness, 0.0, 1.0, "Dryness").on_hover_text(hint);
                });
            });
            egui::CollapsingHeader::new("Holes").default_open(true).show(ui, |ui| {
                let hint = "How much of a surface is small holes, as chalk or a sponge leaves on paper: more where the stroke is thinner. \
                            The Brush set to leave surfaces makes them; lines have none";
                percent(ui, &mut brush.holes, 0.0, 1.0, "Holes").on_hover_text(hint);
                ui.add_enabled_ui(brush.holes > 0.0, |ui| {
                    let hint = "How wide a hole is, in widths of the brush";
                    ui.add(egui::Slider::new(&mut brush.hole_size, 0.02..=0.5).logarithmic(true).text("Hole Size")).on_hover_text(hint);
                });
            });
            self.texture_section(ui, &mut brush.texture, false);
            dual_section(ui, &mut brush.dual_brush);
        });
    }

    /// The settings of a brush's texture. With `pictures`, its pattern can be taken from a
    /// picture; else only the patterns that are made are offered.
    fn texture_section(&mut self, ui: &mut egui::Ui, texture: &mut iskra_drawing::Texture, pictures: bool) {
        section(ui, "Texture", &mut texture.enabled, |ui| {
            match &mut texture.pattern {
                Pattern::Procedural { style, size, .. } => {
                    egui::ComboBox::from_label("Pattern").selected_text(pattern_name(*style)).show_ui(ui, |ui| {
                        for choice in [PatternStyle::Paper, PatternStyle::Canvas, PatternStyle::Noise, PatternStyle::Dots] {
                            ui.selectable_value(style, choice, pattern_name(choice));
                        }
                    });
                    ui.add(egui::Slider::new(size, 16..=512).logarithmic(true).suffix(" px").text("Pattern size"))
                        .on_hover_text("The side of the square the pattern is made in, before it is scaled: it repeats that far apart");
                }
                Pattern::Tile(tile) => {
                    ui.weak(format!("A pattern of its own, {} × {} pixels", tile.width, tile.height));
                }
                Pattern::Stored(stored) => {
                    ui.weak(format!("A pattern of its own, {} × {} pixels", stored.width, stored.height));
                }
            }
            // The pattern: a picture's, or one of those that are made. A vector stroke keeps
            // its pattern with it, so it is one of those that are made.
            ui.horizontal(|ui| {
                if !pictures {
                    return;
                }
                let hint = "The texture takes its grain from a picture, laid side by side: where the picture is light the brush paints most";
                if ui.button("Pattern from a Picture…").on_hover_text(hint).clicked()
                    && let Some(tile) = self.tile_from_picture(false)
                {
                    texture.pattern = Pattern::Tile(tile);
                }
                let made = matches!(texture.pattern, Pattern::Procedural { .. });
                if ui
                    .add_enabled(!made, egui::Button::new("A Made Pattern"))
                    .on_hover_text("One of the patterns that are made: paper, canvas, noise, dots")
                    .clicked()
                {
                    texture.pattern = Pattern::default();
                }
            });
            percent(ui, &mut texture.scale, 0.1, 10.0, "Scale");
            percent(ui, &mut texture.depth, 0.0, 1.0, "Depth");
            percent(ui, &mut texture.brightness, -1.0, 1.0, "Brightness");
            percent(ui, &mut texture.contrast, -1.0, 1.0, "Contrast");
            ui.checkbox(&mut texture.invert, "Inverted");
            mask_mode(ui, "Texture cuts by", &mut texture.mode);
            ui.checkbox(&mut texture.each_tip, "Textured at each stamp of the tip").on_hover_text("Off: once, over the whole stroke");
            ui.add_enabled_ui(texture.each_tip, |ui| dynamic(ui, "Depth", &mut texture.depth_jitter, Some("Least depth"), false, 1.0));
        });
    }

    /// Ask for a picture and make a tip of it, or a pattern. What went wrong, if the picture
    /// could not be read, is kept to be said under the search field.
    fn tile_from_picture(&mut self, tip: bool) -> Option<GrayTile> {
        let path = crate::dialogs::pick_picture(if tip { "A Picture for the Tip" } else { "A Picture for the Pattern" })?;
        match iskra_io::read_picture(&path) {
            Ok((width, height, rgba)) => iskra_drawing::tile_of_picture(width, height, &rgba, tip),
            Err(why) => {
                self.problem = Some(why);
                None
            }
        }
    }
}

/// The settings of a brush's second tip.
fn dual_section(ui: &mut egui::Ui, dual: &mut iskra_drawing::DualBrush) {
    section(ui, "Dual Brush", &mut dual.enabled, |ui| {
        ui.weak("A second tip along the same stroke, which cuts the first.");
        mask_mode(ui, "Second tip cuts by", &mut dual.mode);
        ui.add(egui::Slider::new(&mut dual.size, 1.0..=MAX_SIZE).logarithmic(true).text("Size"));
        if matches!(dual.tip, TipShape::Round) {
            percent(ui, &mut dual.hardness, 0.0, 1.0, "Hardness");
        }
        percent(ui, &mut dual.roundness, 0.01, 1.0, "Roundness");
        ui.add(egui::Slider::new(&mut dual.angle, -180.0..=180.0).suffix("°").text("Angle"));
        percent(ui, &mut dual.spacing, 0.01, 2.0, "Spacing");
        percent(ui, &mut dual.scatter, 0.0, 10.0, "Scatter");
        ui.checkbox(&mut dual.both_axes, "Along the stroke too");
        ui.add(egui::Slider::new(&mut dual.count, 1..=16).text("Count"));
    });
}

/// The largest a brush is, in pixels across: the engine's own limit.
const MAX_SIZE: f32 = 5000.0;

/// The whole of a texture.
const WHOLE: Rect = Rect { min: egui::pos2(0.0, 0.0), max: egui::pos2(1.0, 1.0) };

/// A brush's stroke as a white picture whose alpha is the stroke's coverage, to be tinted.
fn picture(brush: &Brush) -> egui::ColorImage {
    let coverage = stroke_picture(brush, PICTURE[0] as u32, PICTURE[1] as u32);
    let mut image = egui::ColorImage::filled(PICTURE, Color32::TRANSPARENT);
    for (pixel, cover) in image.pixels.iter_mut().zip(coverage) {
        *pixel = Color32::from_white_alpha(cover);
    }
    image
}

/// A slider for a fraction, shown and typed in percent.
fn percent(ui: &mut egui::Ui, value: &mut f32, least: f32, most: f32, label: &str) -> egui::Response {
    let slider = egui::Slider::new(value, least..=most)
        .custom_formatter(|fraction, _| format!("{:.0}%", fraction * 100.0))
        .custom_parser(|typed| typed.trim().trim_end_matches('%').trim().parse::<f64>().ok().map(|percent| percent / 100.0));
    ui.add(slider.text(label))
}

/// A section of the brush that is switched on and off as a whole. Its heading says which.
fn section(ui: &mut egui::Ui, title: &str, enabled: &mut bool, body: impl FnOnce(&mut egui::Ui)) {
    let heading = if *enabled { format!("{title} (on)") } else { title.to_string() };
    egui::CollapsingHeader::new(heading).id_salt(title).show(ui, |ui| {
        ui.checkbox(enabled, "On");
        ui.add_enabled_ui(*enabled, body);
    });
}

fn control_name(control: Control) -> &'static str {
    match control {
        Control::Off => "Nothing",
        Control::Fade => "Fading out",
        Control::PenPressure => "Pen pressure",
        Control::PenTilt => "Pen tilt",
        Control::StylusWheel => "Stylus wheel",
        Control::Rotation => "Pen rotation",
        Control::InitialDirection => "The stroke's first direction",
        Control::Direction => "The stroke's direction",
    }
}

fn pattern_name(style: PatternStyle) -> &'static str {
    match style {
        PatternStyle::Noise => "Noise",
        PatternStyle::Canvas => "Canvas",
        PatternStyle::Paper => "Paper",
        PatternStyle::Dots => "Dots",
    }
}

/// How a texture or a second tip cuts the stroke.
fn mask_mode(ui: &mut egui::Ui, label: &str, mode: &mut MaskMode) {
    const MODES: [(MaskMode, &str); 10] = [
        (MaskMode::Multiply, "Multiply"),
        (MaskMode::Subtract, "Subtract"),
        (MaskMode::Darken, "Darken"),
        (MaskMode::Overlay, "Overlay"),
        (MaskMode::ColorDodge, "Colour Dodge"),
        (MaskMode::ColorBurn, "Colour Burn"),
        (MaskMode::LinearBurn, "Linear Burn"),
        (MaskMode::HardMix, "Hard Mix"),
        (MaskMode::LinearHeight, "Linear Height"),
        (MaskMode::Height, "Height"),
    ];
    let name = MODES.iter().find(|(known, _)| known == mode).map_or("", |(_, name)| name);
    egui::ComboBox::from_label(label).selected_text(name).show_ui(ui, |ui| {
        for (choice, name) in MODES {
            ui.selectable_value(mode, choice, name);
        }
    });
}

/// One value of a brush that chance and the pen can vary: by how much chance does, what else
/// drives it, and how low it may go. `angle` adds what only an angle can follow.
fn dynamic(ui: &mut egui::Ui, label: &str, value: &mut Dynamic, least: Option<&str>, angle: bool, most: f32) {
    percent(ui, &mut value.jitter, 0.0, most, &format!("{label} by chance"));
    let mut controls = vec![Control::Off, Control::Fade, Control::PenPressure, Control::PenTilt, Control::StylusWheel, Control::Rotation];
    if angle {
        controls.extend([Control::InitialDirection, Control::Direction]);
    }
    egui::ComboBox::from_label(format!("{label} follows")).selected_text(control_name(value.control)).show_ui(ui, |ui| {
        for choice in controls {
            ui.selectable_value(&mut value.control, choice, control_name(choice));
        }
    });
    if value.control == Control::Fade {
        ui.add(egui::Slider::new(&mut value.fade_steps, 1..=500).text("Stamps to fade over"));
    }
    if let Some(least) = least {
        percent(ui, &mut value.minimum, 0.0, 1.0, least);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delete_asks_first_and_what_is_done_to_the_presets_is_undone() {
        let (mut panel, mut brushes, mut shown) = (BrushesPanel::default(), Brushes::default(), Shown::default());
        for (name, group) in [("One", "Inks"), ("Two", "Inks"), ("Three", "Paints")] {
            brushes.save_in_hand(name, group, iskra_drawing::BrushKind::Raster);
        }
        let own = |brushes: &Brushes| brushes.presets.iter().map(|preset| preset.name.clone()).collect::<Vec<_>>();
        // A delete only asks; answered, it is done, and can be undone.
        panel.act(&mut brushes, Act::Delete("Inks".to_string(), "One".to_string()), false, &mut shown);
        assert!(panel.asking.is_some() && own(&brushes) == ["One", "Two", "Three"] && panel.undo.is_empty());
        let asked = panel.asking.take().unwrap();
        panel.act(&mut brushes, Act::Answered(Box::new(asked)), false, &mut shown);
        assert_eq!((own(&brushes), panel.undo.last().map(|(what, _)| what.as_str())), (vec!["Two".to_string(), "Three".to_string()], Some("Delete One")));
        // A group goes whole, the engine's too, which are then out of the list.
        let listed = brushes.listed().count();
        panel.act(&mut brushes, Act::Answered(Box::new(Act::DeleteGroup("General".to_string()))), false, &mut shown);
        assert!(brushes.listed().count() < listed && !brushes.hidden.is_empty());
        panel.act(&mut brushes, Act::Place("Paints".to_string(), "Three".to_string(), "Inks".to_string(), Some("Two".to_string())), false, &mut shown);
        assert_eq!(own(&brushes), ["Three", "Two"]);
        // Undo goes back a step at a time: the move, the group, the delete.
        for (presets, hidden) in [(vec!["Two", "Three"], false), (vec!["Two", "Three"], true), (vec!["One", "Two", "Three"], true)] {
            panel.act(&mut brushes, Act::Undo, false, &mut shown);
            assert_eq!((own(&brushes), brushes.hidden.is_empty()), (presets.iter().map(|name| name.to_string()).collect(), hidden));
        }
        assert!(panel.undo.is_empty());
        // What changes nothing is no step: a preset put where it is.
        panel.act(&mut brushes, Act::Place("Inks".to_string(), "One".to_string(), "Inks".to_string(), Some("One".to_string())), false, &mut shown);
        assert!(panel.undo.is_empty());
        // A group, or one brush of it, is handed over to be written to a file, under its name.
        panel.act(&mut brushes, Act::Export("Inks".to_string(), None), false, &mut shown);
        let (name, presets) = shown.export.take().unwrap();
        assert_eq!((name.as_str(), presets.len()), ("Inks", 2));
        panel.act(&mut brushes, Act::Export("General".to_string(), Some("Hard Round".to_string())), false, &mut shown);
        assert_eq!(shown.export.take().map(|(name, presets)| (name, presets.len())), Some(("Hard Round".to_string(), 1)));
        assert!(!shown.taken && panel.undo.is_empty());
    }
}
