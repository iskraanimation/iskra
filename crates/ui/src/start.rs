//! The start window (the user's of 2026-10-26): what the app shows when it is opened from its
//! icon, before the project's window. On the left a new project, its name, where it is kept,
//! its resolution and frame rate, picked from the presets Settings keeps, and the buttons
//! under it; on the right, in a darker column of 30 % of the width, the projects opened
//! lately, and at their foot *Browse…* for a project file that is not among them. *Open*
//! makes the new project, or opens the recent one picked; a double click on one opens it
//! too. *Cancel* quits. It is the app's window, as large as
//! the start window and not resizable, until a project is opened (the user's of 2026-10-28).

use std::path::PathBuf;

use egui::{Align, Layout, RichText, vec2};

use crate::prefs::NewProject;

/// The room between the window's edges and what is in it.
const MARGIN: f32 = 20.0;
/// How wide the new project's name and folder are.
const FIELD_WIDTH: f32 = 260.0;
/// How much of the window's width the recent projects take.
const RECENT_SHARE: f32 = 0.3;
/// How much darker than the window the recent projects' column is.
const RECENT_DARKER: f32 = 0.12;
/// How tall the row of buttons is.
const BUTTONS_HEIGHT: f32 = 24.0;

/// What the start window asks for.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// A new project, made with these, kept in a folder of its name in `folder`.
    Create { name: String, folder: PathBuf, width: u32, height: u32, frame_rate: u32 },
    /// The project file at this path.
    Open(PathBuf),
    /// A project file the user picks in the system's dialog, to open.
    Browse,
    /// Leave the app.
    Quit,
}

pub struct StartWindow {
    pub name: String,
    pub folder: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    /// The recent project picked, by its place in the list.
    pub picked: Option<usize>,
    /// Why the last *Open* was not done, said over the buttons.
    pub problem: Option<String>,
    /// Where its buttons and the recent projects were drawn, for the tests.
    #[cfg(test)]
    pub drawn: Vec<(String, egui::Rect)>,
    /// What a test picks in place of the dialog *Browse…* would show.
    #[cfg(test)]
    pub browsed: Option<PathBuf>,
}

impl StartWindow {
    /// The window as the last new project was made: the same folder, resolution and rate.
    pub fn new(from: &NewProject) -> Self {
        // At first, a folder Iskra in the user's documents' place: home, or Windows' profile.
        let home = || ["HOME", "USERPROFILE"].iter().find_map(|name| std::env::var_os(name).filter(|value| !value.is_empty())).map(PathBuf::from);
        let folder = from.folder.clone().or_else(|| home().map(|home| home.join("Iskra")));
        StartWindow {
            name: "Untitled".to_string(),
            folder,
            width: from.width,
            height: from.height,
            frame_rate: from.frame_rate,
            picked: None,
            problem: None,
            #[cfg(test)]
            drawn: Vec::new(),
            #[cfg(test)]
            browsed: None,
        }
    }

    /// Draw the window over the whole of `ui`, which the app's window is as large as, with
    /// the `recent` projects; and what was asked.
    pub fn show(&mut self, ui: &mut egui::Ui, presets: &NewProject, recent: &[PathBuf]) -> Option<Action> {
        #[cfg(test)]
        self.drawn.clear();
        let mut asked = None;
        let whole = ui.available_rect_before_wrap();
        ui.painter().rect_filled(whole, 0.0, ui.visuals().panel_fill);
        ui.allocate_rect(whole, egui::Sense::hover());
        // The new project on the left, its buttons under it; the recent projects on the
        // right, in a column of their own, a little darker (the user's of 2026-10-28).
        let split = whole.left() + whole.width() * (1.0 - RECENT_SHARE);
        let (left, right) = (whole.with_max_x(split), whole.with_min_x(split));
        ui.painter().rect_filled(right, 0.0, ui.visuals().panel_fill.lerp_to_gamma(egui::Color32::BLACK, RECENT_DARKER));
        // The name Iskra is in the window's title bar.
        let column = |rect: egui::Rect| egui::UiBuilder::new().max_rect(rect.shrink(MARGIN)).layout(Layout::top_down(Align::Min));
        ui.scope_builder(column(left), |ui| {
            ui.set_clip_rect(left);
            self.new_project(ui, presets);
            // The buttons at the foot of the column, and why *Open* was not done over them.
            let foot = ui.available_rect_before_wrap();
            let buttons = foot.with_min_y(foot.max.y - BUTTONS_HEIGHT);
            if let Some(problem) = &self.problem {
                let over = foot.with_max_y(buttons.min.y - 8.0);
                ui.scope_builder(egui::UiBuilder::new().max_rect(over).layout(Layout::bottom_up(Align::Min)), |ui| {
                    ui.colored_label(ui.visuals().error_fg_color, problem);
                });
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(buttons).layout(Layout::right_to_left(Align::Center)), |ui| {
                let open = ui.add_enabled(self.can_open(recent), egui::Button::new("Open"));
                let open = open.on_hover_text("Make the new project, or open the recent one picked");
                let cancel = ui.button("Cancel").on_hover_text("Leave Iskra");
                #[cfg(test)]
                self.drawn.extend([("Open".to_string(), open.rect), ("Cancel".to_string(), cancel.rect)]);
                if cancel.clicked() {
                    asked = Some(Action::Quit);
                }
                if open.clicked() {
                    asked = self.opened(recent);
                }
            });
        });
        // The recent projects, and under them at the column's foot *Browse…*, level with the
        // buttons on the left (the user's of 2026-10-31).
        let inside = right.shrink(MARGIN);
        let browse_row = inside.with_min_y(inside.max.y - BUTTONS_HEIGHT);
        ui.scope_builder(column(right.with_max_y(browse_row.min.y - 8.0 + MARGIN)), |ui| {
            ui.set_clip_rect(right);
            asked = asked.take().or_else(|| self.recent(ui, recent));
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(browse_row).layout(Layout::right_to_left(Align::Center)), |ui| {
            let browse = ui.button("Browse…").on_hover_text("Open a project file that is not among the recent ones");
            #[cfg(test)]
            self.drawn.push(("Browse…".to_string(), browse.rect));
            if browse.clicked() {
                asked = Some(Action::Browse);
            }
        });
        asked
    }

    fn can_open(&self, recent: &[PathBuf]) -> bool {
        self.picked.is_some_and(|at| at < recent.len()) || (!self.name.trim().is_empty() && self.folder.is_some())
    }

    /// What *Open* does: the recent project picked, or the new one.
    fn opened(&self, recent: &[PathBuf]) -> Option<Action> {
        if let Some(path) = self.picked.and_then(|at| recent.get(at)) {
            return Some(Action::Open(path.clone()));
        }
        let name = self.name.trim().to_string();
        let folder = self.folder.clone().filter(|_| !name.is_empty())?;
        Some(Action::Create { name, folder, width: self.width, height: self.height, frame_rate: self.frame_rate })
    }

    fn new_project(&mut self, ui: &mut egui::Ui, presets: &NewProject) {
        ui.label(RichText::new("New Project").strong());
        ui.add_space(6.0);
        egui::Grid::new("start new project").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
            ui.label("Name");
            let name = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(FIELD_WIDTH));
            if name.gained_focus() || name.changed() {
                self.picked = None;
            }
            ui.end_row();
            ui.label("Location");
            ui.horizontal(|ui| {
                // As wide as the name at most: a long folder is cut short, not let widen
                // the window.
                ui.set_max_width(FIELD_WIDTH);
                let shown = self.folder.as_ref().map_or("Choose a folder".to_string(), |folder| folder.display().to_string());
                ui.add(egui::Label::new(RichText::new(shown).weak()).truncate()).on_hover_text("The project's folder is made in this one");
                if ui.button("Choose…").clicked()
                    && let Some(folder) = crate::dialogs::project_location()
                {
                    (self.folder, self.picked) = (Some(folder), None);
                }
            });
            ui.end_row();
            ui.label("Resolution");
            let named = presets.resolutions.iter().find(|preset| (preset.width, preset.height) == (self.width, self.height));
            let shown = named.map_or_else(|| format!("{} × {}", self.width, self.height), |preset| preset.name.clone());
            egui::ComboBox::from_id_salt("start resolution").selected_text(shown).show_ui(ui, |ui| {
                for preset in &presets.resolutions {
                    if ui.selectable_label((preset.width, preset.height) == (self.width, self.height), &preset.name).clicked() {
                        (self.width, self.height, self.picked) = (preset.width, preset.height, None);
                    }
                }
            });
            ui.end_row();
            ui.label("Frame rate");
            egui::ComboBox::from_id_salt("start frame rate").selected_text(format!("{} fps", self.frame_rate)).show_ui(ui, |ui| {
                for fps in &presets.frame_rates {
                    if ui.selectable_label(*fps == self.frame_rate, format!("{fps} fps")).clicked() {
                        (self.frame_rate, self.picked) = (*fps, None);
                    }
                }
            });
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.label(RichText::new("The presets are in Settings ▸ New Projects.").small().weak());
    }

    /// The projects opened lately: a click picks one, a double click opens it.
    fn recent(&mut self, ui: &mut egui::Ui, recent: &[PathBuf]) -> Option<Action> {
        ui.label(RichText::new("Recent Projects").strong());
        ui.add_space(6.0);
        if recent.is_empty() {
            ui.weak("None yet");
            return None;
        }
        let mut asked = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (at, path) in recent.iter().enumerate() {
                let name = path.file_stem().map_or_else(|| path.display().to_string(), |stem| stem.to_string_lossy().to_string());
                let row = ui.selectable_label(self.picked == Some(at), name).on_hover_text(path.display().to_string());
                #[cfg(test)]
                self.drawn.push((path.display().to_string(), row.rect));
                if row.clicked() {
                    self.picked = Some(at);
                }
                if row.double_clicked() {
                    asked = Some(Action::Open(path.clone()));
                }
            }
        });
        asked
    }
}
