//! Shots in Properties (the user's, 2026-10-28): the shots picked on the storyboard timeline,
//! or the one picked in the project bin, with their name, status, length, scene and sequence.
//! Several picked are set together, as one step, a value they differ in shown as a dash, as
//! several pegs are.

use iskra_model::{Item, Numbering, Shot, ShotId, Status};
use serde_json::{Value, json};

use crate::IskraApp;
use crate::dock::Page;
use crate::form;

/// A text of the shots that is typed in Properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Text {
    Name,
    Scene,
    Sequence,
}

/// What a value of the shots is: the one they all have, or none where they differ.
fn shared<T: PartialEq>(shots: &[&Shot], of: impl Fn(&Shot) -> T) -> Option<T> {
    let first = of(shots.first()?);
    shots.iter().all(|shot| of(shot) == first).then_some(first)
}

impl IskraApp {
    /// The shots Properties shows: on the Storyboard page those picked on its timeline, on the
    /// Media page the one picked in the bin.
    pub(crate) fn shots_in_properties(&self) -> Vec<ShotId> {
        let project = self.session.project();
        let picked = match self.page {
            Page::Storyboard => self.storyboard_view.picked_shots.clone(),
            Page::Media => match self.media.picked {
                Some(Item::Shot(shot)) => vec![shot],
                _ => Vec::new(),
            },
            Page::Animation | Page::Export => Vec::new(),
        };
        picked.into_iter().filter(|shot| project.shot(*shot).is_some()).collect()
    }

    /// The shots' values, which are given to all of them when one is changed.
    pub(crate) fn shots_properties(&mut self, ui: &mut egui::Ui, ids: &[ShotId]) {
        let project = self.session.project().clone();
        let shots: Vec<&Shot> = ids.iter().filter_map(|id| project.shot(*id)).collect();
        let [first, ..] = shots[..] else { return };
        let several = shots.len() > 1;
        if several {
            ui.strong(format!("{} shots", shots.len()));
            ui.weak("A value they share is shown; one they don't is a dash. Changed here, it is changed on all of them");
        } else {
            ui.strong(&first.name);
            ui.weak(match project.storyboard_of(first.id) {
                Some(board) => format!("On {}", board.name),
                None => "On no storyboard".to_string(),
            });
        }
        ui.add_space(4.0);
        ui.separator();
        #[cfg(test)]
        let mut fields = Vec::new();
        egui::Grid::new("shots properties").num_columns(2).show(ui, |ui| {
            // A name is a shot's own: several are renamed one by one.
            form::name(ui, "Name");
            if several {
                let hint = "Each shot has a name of its own: renamed together, as a text and a number in their order on the storyboard";
                if ui.button("Rename Together…").on_hover_text(hint).clicked() {
                    self.renaming_shots = Some(ids.to_vec());
                }
            } else {
                let typed = self.shot_text_field(ui, Text::Name, Some(first.name.clone()), &[]);
                #[cfg(test)]
                fields.push(typed.1);
                if let Some(name) = typed.0.filter(|name| !name.trim().is_empty() && *name != first.name) {
                    self.run("shot.rename", json!({ "shot": first.id.0, "name": name }));
                }
            }
            ui.end_row();

            form::name(ui, "Status");
            let status = shared(&shots, |shot| shot.status);
            let mut chosen = status;
            let said = status.map_or("—", Status::label);
            #[cfg_attr(not(test), allow(unused_variables))]
            let combo = egui::ComboBox::from_id_salt("shots status").selected_text(said).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                for status in Status::ALL {
                    ui.selectable_value(&mut chosen, Some(status), status.label());
                }
            });
            #[cfg(test)]
            fields.push(combo.response.rect);
            if let Some(to) = chosen.filter(|to| Some(*to) != status) {
                // A shot that is out stays with whom it is with.
                let set = |shot: &&Shot| json!({ "shot": shot.id.0, "status": to, "to": shot.out_to.clone().unwrap_or_default() });
                self.run_all("shot.setStatus", "Set Shot Status", shots.iter().map(set).collect());
            }
            ui.end_row();
            if let (false, Status::Out, Some(who)) = (several, first.status, &first.out_to) {
                form::name(ui, "");
                ui.weak(format!("With {who}"));
                ui.end_row();
            }

            form::name(ui, "Length");
            let length = shared(&shots, |shot| shot.duration);
            let mut now = self.shots_length.unwrap_or(length.unwrap_or(first.duration));
            let mut field = egui::DragValue::new(&mut now).speed(0.25).range(1..=u32::MAX).suffix(" frames");
            if length.is_none() && self.shots_length.is_none() {
                field = field.custom_formatter(|_, _| "—".to_string());
            }
            let dragged = ui.add(field).on_hover_text("The whole shot, handles included");
            #[cfg(test)]
            fields.push(dragged.rect);
            if dragged.changed() {
                self.shots_length = Some(now);
            }
            if let Some(to) = self.shots_length.filter(|_| !dragged.dragged() && !dragged.has_focus()) {
                self.shots_length = None;
                let set = |shot: &&Shot| json!({ "shot": shot.id.0, "frames": to });
                self.run_all("shot.setDuration", "Set Shot Duration", shots.iter().filter(|shot| shot.duration != to).map(set).collect());
            }
            ui.end_row();

            let groups = [(Text::Scene, "Scene", project.scenes()), (Text::Sequence, "Sequence", project.sequences())];
            for (text, title, in_use) in groups {
                form::name(ui, title);
                let of = |shot: &Shot| if text == Text::Scene { shot.scene.clone() } else { shot.sequence.clone() };
                let typed = ui.horizontal(|ui| self.shot_text_field(ui, text, shared(&shots, of), &in_use)).inner;
                #[cfg(test)]
                fields.push(typed.1);
                if let Some(name) = typed.0.filter(|name| shots.iter().any(|shot| of(shot) != name.trim())) {
                    let key = if text == Text::Scene { "scene" } else { "sequence" };
                    self.run("shot.setScene", json!({ "shots": ids.iter().map(|id| id.0).collect::<Vec<_>>(), key: name }));
                }
                ui.end_row();
            }
        });
        #[cfg(test)]
        {
            self.property_fields = fields;
        } // The clips' colours, on the storyboard that is open, or else the shot's.
        let board = self.storyboard.and_then(|id| project.storyboard(id)).or_else(|| project.storyboard_of(first.id));
        if let Some(board) = board {
            ui.add_space(4.0);
            ui.separator();
            match crate::scenes::colours(ui, &mut self.prefs.clip_colours, &project, board) {
                Some(crate::scenes::Asked::Pick(shots)) => {
                    let view = &mut self.storyboard_view;
                    (view.picked_shots, view.picked_sounds, view.picked_keys) = (shots, Vec::new(), Vec::new());
                }
                Some(crate::scenes::Asked::SendOut(shots)) => self.ask_send_out(shots),
                None => {}
            }
        }
    }

    /// A text of the shots, typed and kept when it is left or Enter is pressed; Escape keeps
    /// what was. `shown` is what they all have, or none where they differ, shown as a dash.
    /// With names in use, a menu of them beside it, and of none, to pick one from. Returns
    /// what is to be given them, and where the field is.
    fn shot_text_field(&mut self, ui: &mut egui::Ui, which: Text, shown: Option<String>, in_use: &[String]) -> (Option<String>, egui::Rect) {
        let typing = self.shot_text.as_ref().filter(|(at, _)| *at == which).map(|(_, text)| text.clone());
        let mut text = typing.clone().unwrap_or_else(|| shown.clone().unwrap_or_default());
        let width = if which == Text::Name { form::FIELD_WIDTH } else { form::FIELD_WIDTH - 24.0 };
        let mut edit = egui::TextEdit::singleline(&mut text).id_salt(("shot text", which)).desired_width(width);
        if shown.is_none() {
            edit = edit.hint_text("—");
        }
        let field = ui.add(edit);
        if field.changed() {
            self.shot_text = Some((which, text.clone()));
        }
        let mut given = None;
        if field.lost_focus() {
            let kept = self.shot_text.take_if(|(at, _)| *at == which).map(|(_, text)| text);
            given = kept.filter(|_| !ui.input(|i| i.key_pressed(egui::Key::Escape)));
        }
        if which != Text::Name {
            let hint = if in_use.is_empty() { "None is in use yet: a name typed is one" } else { "The names in use, to pick one" };
            ui.menu_button("⌄", |ui| {
                for name in in_use {
                    if ui.radio(shown.as_deref() == Some(name.as_str()), name).clicked() {
                        given = Some(name.clone());
                        ui.close();
                    }
                }
                if !in_use.is_empty() {
                    ui.separator();
                }
                if ui.radio(shown.as_deref() == Some(""), "None").clicked() {
                    given = Some(String::new());
                    ui.close();
                }
            })
            .response
            .on_hover_text(hint);
        }
        (given, field.rect)
    }

    /// The Rename Shots window (the user's, 2026-10-28): the shots being renamed get a text
    /// and a number after it, in their order on the storyboard, with the names it would give
    /// shown before it is done, and a name another shot has already said and refused.
    pub(crate) fn rename_shots_window(&mut self, ctx: &egui::Context) {
        let Some(shots) = self.renaming_shots.clone() else { return };
        let mut open = !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let (mut go, mut cancel) = (false, false);
        let mut numbering: Numbering = self.prefs.shot_numbering.clone();
        let project = self.session.project().clone();
        let named = project.numbered(&shots, &numbering);
        let clash = named.iter().any(|(_, _, taken)| taken.is_some());
        let window = egui::Window::new("Rename Shots").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_TOP, [0.0, 60.0]);
        window.open(&mut open).show(ctx, |ui| {
            ui.weak(format!("{} shots, named in their order on the storyboard", shots.len()));
            egui::Grid::new("rename shots").num_columns(2).show(ui, |ui| {
                form::name(ui, "Text");
                ui.add(egui::TextEdit::singleline(&mut numbering.text).desired_width(form::FIELD_WIDTH))
                    .on_hover_text("What each name starts with, as SH or Shot_");
                ui.end_row();
                form::name(ui, "First number");
                ui.add(egui::DragValue::new(&mut numbering.first).speed(0.25));
                ui.end_row();
                form::name(ui, "Step");
                ui.add(egui::DragValue::new(&mut numbering.step).speed(0.1).range(1..=1000))
                    .on_hover_text("From one shot's number to the next: 10 leaves room for shots put between, as SH010, SH020");
                ui.end_row();
                form::name(ui, "Digits");
                ui.add(egui::DragValue::new(&mut numbering.digits).speed(0.05).range(0..=9)).on_hover_text("How many digits a number is padded to with zeros");
                ui.end_row();
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                egui::Grid::new("renamed shots").num_columns(2).striped(true).show(ui, |ui| {
                    for (shot, name, taken) in &named {
                        ui.weak(project.shot(*shot).map_or("", |shot| shot.name.as_str()));
                        match taken {
                            Some(_) => ui.colored_label(crate::theme::Theme::of(ui.ctx()).danger, format!("→ {name}, which another shot has")),
                            None => ui.label(format!("→ {name}")),
                        };
                        ui.end_row();
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                let hint = "Another shot already has one of these names";
                let button = ui.add_enabled(!clash, egui::Button::new("Rename")).on_disabled_hover_text(hint);
                #[cfg(test)]
                {
                    self.rename_shots_button = Some((button.rect, !clash));
                }
                go = button.clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        self.prefs.shot_numbering = numbering.clone();
        if go {
            let mut params = serde_json::to_value(&numbering).unwrap_or_default();
            params["shots"] = json!(shots.iter().map(|shot| shot.0).collect::<Vec<_>>());
            self.run("shot.renameNumbered", params);
        }
        if go || cancel || !open {
            self.renaming_shots = None;
        }
    }

    /// One command for each of `params`, as one step called `title`; none for none.
    fn run_all(&mut self, id: &str, title: &str, params: Vec<Value>) {
        match &params[..] {
            [] => {}
            [one] => self.run(id, one.clone()),
            _ => {
                let commands: Vec<Value> = params.into_iter().map(|params| json!({ "id": id, "params": params })).collect();
                self.run("edit.batch", json!({ "commands": commands, "title": title }));
            }
        }
    }
}
