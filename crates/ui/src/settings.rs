//! The Settings window (plan §7.5): a list of sections on the left and a section's settings on
//! the right, after Resolve's. The list says where each is kept: under *Iskra* the app's, with
//! the preferences on this machine, and under *This project* the project's, in its file.

use egui::{RichText, vec2};
use iskra_model::WorkingSpace;

use crate::IskraApp;
use crate::commands;
use crate::dock::Page;
use crate::keymap::{Chord, Preset};
use crate::prefs::{GraphicsBackend, NewDrawingHold, TabletInput};
use crate::theme::{Appearance, Role, Theme};

/// A section of the Settings window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Interface,
    Memory,
    Shortcuts,
    NewProjects,
    Scripting,
    Format,
}

/// The graphics cards found when the app started, and what it was started with.
#[derive(Debug, Default)]
pub struct Graphics {
    /// The cards' names, as the system gives them.
    pub cards: Vec<String>,
    /// The card the window is drawn with, and its API.
    pub in_use: Option<String>,
    /// The API and the card the preferences asked for when the app started.
    pub started: (GraphicsBackend, Option<String>),
}

impl Section {
    /// The app's sections, in the list's order.
    const APP: [Section; 6] = [Section::General, Section::Interface, Section::Memory, Section::Shortcuts, Section::NewProjects, Section::Scripting];
    /// The project's.
    const PROJECT: [Section; 1] = [Section::Format];

    fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Interface => "Interface",
            Section::Memory => "Memory and GPU",
            Section::Shortcuts => "Shortcuts",
            Section::NewProjects => "New Projects",
            Section::Scripting => "Scripting",
            Section::Format => "Format",
        }
    }
}

/// How long changes wait before they are autosaved, in minutes.
const AUTOSAVE_MINUTES: [u32; 5] = [1, 2, 5, 10, 15];
/// The interface's sizes, in percent.
const SIZES: [u32; 6] = [80, 90, 100, 110, 125, 150];
/// The playback cache's limits, in MB, and the effects'.
const PLAYBACK_MEMORY: std::ops::RangeInclusive<u32> = 256..=16384;
const EFFECT_MEMORY: std::ops::RangeInclusive<u32> = 256..=8192;

/// Megabytes as they are read: in GB from 1024 on.
fn megabytes(megabytes: u64) -> String {
    if megabytes < 1024 { format!("{megabytes} MB") } else { format!("{:.1} GB", megabytes as f64 / 1024.0) }
}

/// The greys round the camera's frame.
const SURROUNDS: [(u8, &str); 4] = [(60, "Dark Grey"), (128, "Mid Grey"), (190, "Light Grey"), (255, "White")];

impl IskraApp {
    /// The Settings window, while it is open.
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(section) = self.settings else { return };
        // Escape closes it, unless a command is waiting for a key: then it leaves the key be.
        let escape = self.shortcut_capture.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut open = !escape;
        let mut chosen = section;
        let window = egui::Window::new("Settings").collapsible(false).resizable(true).default_size(vec2(600.0, 380.0)).min_size(vec2(520.0, 280.0));
        window.open(&mut open).show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                // The sections, under where they are kept.
                ui.vertical(|ui| {
                    ui.set_width(130.0);
                    ui.label(RichText::new("ISKRA").small().weak());
                    #[cfg(test)]
                    self.settings_sections.clear();
                    let mut list = |ui: &mut egui::Ui, sections: &[Section]| {
                        for &section in sections {
                            let picked = ui.selectable_label(chosen == section, section.title());
                            #[cfg(test)]
                            self.settings_sections.push((section, picked.rect));
                            if picked.clicked() {
                                chosen = section;
                            }
                        }
                    };
                    list(ui, &Section::APP);
                    ui.add_space(8.0);
                    ui.label(RichText::new("THIS PROJECT").small().weak());
                    list(ui, &Section::PROJECT);
                });
                ui.separator();
                ui.vertical(|ui| {
                    // As large as the window is made: the section fills it, and scrolls in it.
                    ui.set_min_size(ui.available_size().max(vec2(380.0, 0.0)));
                    ui.heading(chosen.title());
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match chosen {
                        Section::General => self.general_settings(ui),
                        Section::Interface => self.interface_settings(ui),
                        Section::Memory => self.memory_settings(ui),
                        Section::Shortcuts => self.shortcut_settings(ui),
                        Section::NewProjects => self.new_project_settings(ui),
                        Section::Scripting => self.scripting_settings(ui),
                        Section::Format => self.format_settings(ui),
                    });
                });
            });
        });
        self.settings = open.then_some(chosen);
        if self.settings != Some(Section::Shortcuts) {
            self.shortcut_capture = None;
        }
    }

    /// What scripts may do beyond the project (plan §9.3).
    fn scripting_settings(&mut self, ui: &mut egui::Ui) {
        let hint = "Off, a script reads files only in the project's folder and writes none. On, it reads and writes files anywhere \
                    the app may: only for scripts you trust";
        ui.checkbox(&mut self.prefs.scripts_write_files, "Let scripts write files").on_hover_text(hint);
    }

    fn general_settings(&mut self, ui: &mut egui::Ui) {
        let prefs = &mut self.prefs;
        egui::Grid::new("general settings").num_columns(2).spacing(vec2(16.0, 8.0)).show(ui, |ui| {
            ui.label("Your name");
            ui.add(egui::TextEdit::singleline(&mut prefs.user_name).hint_text("Signs your retake notes").desired_width(200.0))
                .on_hover_text("The retake notes you write are signed with it, and scripts read it as iskra.user");
            ui.end_row();
            ui.label("Autosave");
            ui.horizontal(|ui| {
                ui.checkbox(&mut prefs.autosave, "").on_hover_text("Save changes on their own. The backup stays the version you last saved yourself");
                ui.add_enabled_ui(prefs.autosave, |ui| {
                    let every = |minutes: u32| if minutes == 1 { "Every minute".to_string() } else { format!("Every {minutes} minutes") };
                    egui::ComboBox::from_id_salt("autosave minutes").selected_text(every(prefs.autosave_minutes)).show_ui(ui, |ui| {
                        for minutes in AUTOSAVE_MINUTES {
                            ui.selectable_value(&mut prefs.autosave_minutes, minutes, every(minutes));
                        }
                    });
                });
            });
            ui.end_row();
            ui.label("Keep workspace changes");
            let mut keep = prefs.keep_layouts;
            ui.checkbox(&mut keep, "").on_hover_text(
                "On, each page opens as it was left. Off, each start opens the pages as they were when this was switched off, \
                 or a workspace was last saved or taken",
            );
            if keep != prefs.keep_layouts {
                prefs.keep_layouts = keep;
                prefs.fixed_layouts = (!keep).then(|| prefs.layouts.clone());
            }
            ui.end_row();
            ui.label("Animate at start");
            ui.checkbox(&mut prefs.animate_at_start, "").on_hover_text(
                "The Animate switch on the Animation page at each start of the app, keying changes on the frame they are made on. \
                 The Storyboard page's starts off",
            );
            ui.end_row();
            ui.label("New drawings hold");
            let mut hold = prefs.new_drawing_hold;
            egui::ComboBox::from_id_salt("settings hold").selected_text(hold.label()).show_ui(ui, |ui| {
                for choice in
                    [NewDrawingHold::ToNext, NewDrawingHold::Frames(1), NewDrawingHold::Frames(2), NewDrawingHold::Frames(3), NewDrawingHold::Frames(4)]
                {
                    ui.selectable_value(&mut hold, choice, choice.label());
                }
            });
            prefs.new_drawing_hold = hold;
            ui.end_row();
            // Only Windows has a choice of where a pen is read from.
            if cfg!(any(windows, test)) {
                ui.label("Tablet input");
                ui.vertical(|ui| {
                    egui::ComboBox::from_id_salt("tablet input").selected_text(prefs.tablet_input.label()).show_ui(ui, |ui| {
                        for input in TabletInput::ALL {
                            ui.selectable_value(&mut prefs.tablet_input, input, input.label()).on_hover_text(input.hint());
                        }
                    });
                    ui.label(RichText::new(prefs.tablet_input.hint()).small().weak());
                });
                ui.end_row();
            }
        });
    }

    fn interface_settings(&mut self, ui: &mut egui::Ui) {
        let prefs = &mut self.prefs;
        egui::Grid::new("interface settings").num_columns(2).spacing(vec2(16.0, 8.0)).show(ui, |ui| {
            ui.label("Appearance");
            ui.horizontal(|ui| {
                for look in Appearance::ALL {
                    ui.radio_value(&mut prefs.appearance, look, look.label());
                }
            });
            ui.end_row();
            ui.label("Interface size");
            egui::ComboBox::from_id_salt("interface size").selected_text(format!("{}%", prefs.ui_scale)).show_ui(ui, |ui| {
                for scale in SIZES {
                    ui.selectable_value(&mut prefs.ui_scale, scale, format!("{scale}%"));
                }
            });
            ui.end_row();
            ui.label("Canvas surround");
            ui.horizontal(|ui| {
                for (grey, name) in SURROUNDS {
                    ui.radio_value(&mut prefs.canvas_surround, grey, name).on_hover_text(
                        "The grey round the camera's frame, with an aspect mask on. With none it is white, and with Clip on darker than dark grey",
                    );
                }
            });
            ui.end_row();
            ui.label("Hold playhead");
            ui.checkbox(&mut prefs.hold_playhead, "").on_hover_text(
                "A click on a timeline's row, drawing or keyframe selects it and leaves the playhead where it is. \
                 The ruler, a drag and playing still move it",
            );
            ui.end_row();
        });
        ui.add_space(12.0);
        self.colour_settings(ui);
    }

    /// The colours of the interface: what each is used for, its swatch, which opens a picker,
    /// and a way back to its default. One set for both appearances.
    fn colour_settings(&mut self, ui: &mut egui::Ui) {
        let theme = Theme::of(ui.ctx());
        let colours = &mut self.prefs.colours;
        ui.horizontal(|ui| {
            ui.strong("Colours");
            if ui.add_enabled(!colours.is_empty(), egui::Button::new("Reset All").small()).on_hover_text("Every colour as Iskra has it").clicked() {
                colours.clear();
            }
        });
        ui.label(RichText::new("The same in Dark and Light").small().weak());
        ui.add_space(4.0);
        #[cfg(test)]
        self.colour_rows.clear();
        egui::Grid::new("colour settings").num_columns(3).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
            // Under a heading for each group: the interface, the kinds of node, the statuses.
            let mut group = None;
            for role in Role::ALL {
                if group != Some(role.group()) {
                    group = Some(role.group());
                    ui.label(RichText::new(role.group()).strong());
                    ui.end_row();
                }
                ui.label(role.label());
                let mut colour = theme.colour(role);
                let alpha = if role.has_alpha() { egui::color_picker::Alpha::OnlyBlend } else { egui::color_picker::Alpha::Opaque };
                let swatch = egui::color_picker::color_edit_button_srgba(ui, &mut colour, alpha);
                #[cfg(test)]
                self.colour_rows.push((role, swatch.rect));
                if swatch.changed() {
                    colours.insert(role, colour.to_srgba_unmultiplied());
                }
                if colours.contains_key(&role) && ui.small_button("Default").on_hover_text("Back to the colour it started as").clicked() {
                    colours.remove(&role);
                }
                ui.end_row();
            }
        });
    }

    fn memory_settings(&mut self, ui: &mut egui::Ui) {
        // The machine's memory is read again each second while it is shown.
        let now = std::time::Instant::now();
        if self.system_memory.is_none_or(|(read, ..)| now.duration_since(read).as_secs_f32() >= 1.0) {
            let mut system = sysinfo::System::new();
            system.refresh_memory();
            self.system_memory = Some((now, system.total_memory(), system.available_memory()));
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        let (_, total, free) = self.system_memory.unwrap_or((now, 0, 0));
        let frame = {
            let settings = &self.session.project().settings;
            u64::from(settings.width) * u64::from(settings.height) * 4
        };
        let (started, graphics) = (&self.graphics.started, &self.graphics);
        let prefs = &mut self.prefs;
        egui::Grid::new("memory settings").num_columns(2).spacing(vec2(16.0, 8.0)).show(ui, |ui| {
            ui.label("System memory");
            if total == 0 {
                ui.label(RichText::new("Not known").weak());
            } else {
                ui.label(format!("{} free of {}", megabytes(free >> 20), megabytes(total >> 20)));
            }
            ui.end_row();
            ui.label("Playback cache");
            ui.vertical(|ui| {
                let slider = egui::Slider::new(&mut prefs.playback_memory, PLAYBACK_MEMORY).step_by(256.0).logarithmic(true).suffix(" MB");
                if ui.add(slider).on_hover_text("How much memory of the graphics card the frames made for playback may take").changed()
                    && let Some(cache) = &mut self.viewport.cache
                {
                    cache.budget = (prefs.playback_memory.max(1) as usize) << 20;
                }
                let frames = (u64::from(prefs.playback_memory) << 20) / frame.max(1);
                ui.label(RichText::new(format!("About {frames} frames of this project's size")).small().weak());
            });
            ui.end_row();
            ui.label("Memory for effects");
            let slider = egui::Slider::new(&mut prefs.effect_memory, EFFECT_MEMORY).step_by(256.0).logarithmic(true).suffix(" MB");
            let hint =
                "How much memory the pictures of effects may take, in the viewport and in each render: more keeps more frames of a shot whose effects change";
            if ui.add(slider).on_hover_text(hint).changed() {
                iskra_render::set_shared_budget((prefs.effect_memory.max(1) as usize) << 20);
            }
            ui.end_row();
            ui.label("Viewport on the card");
            ui.checkbox(&mut prefs.viewport_on_card, "Draw the viewport on the graphics card")
                .on_hover_text("Standing still, the viewport is drawn by the GPU compositor, as playback is. Also in the View menu");
            ui.end_row();
            ui.label("Processing mode");
            egui::ComboBox::from_id_salt("graphics backend").selected_text(prefs.graphics_backend.label()).show_ui(ui, |ui| {
                for &backend in GraphicsBackend::OFFERED {
                    ui.selectable_value(&mut prefs.graphics_backend, backend, backend.label());
                }
            });
            ui.end_row();
            ui.label("Graphics card");
            let shown = prefs.graphics_card.clone().unwrap_or_else(|| "Auto".to_string());
            egui::ComboBox::from_id_salt("graphics card").selected_text(shown).width(260.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut prefs.graphics_card, None, "Auto");
                for card in &graphics.cards {
                    ui.selectable_value(&mut prefs.graphics_card, Some(card.clone()), card);
                }
            });
            ui.end_row();
            ui.label("");
            ui.vertical(|ui| {
                if let Some(in_use) = &graphics.in_use {
                    ui.label(RichText::new(format!("Drawing with {in_use}")).small().weak());
                }
                if (prefs.graphics_backend, &prefs.graphics_card) != (started.0, &started.1) {
                    ui.label(RichText::new("Takes effect when Iskra is started again").small().color(ui.visuals().warn_fg_color));
                } else {
                    ui.label(RichText::new("The mode and the card take effect when Iskra starts").small().weak());
                }
            });
            ui.end_row();
        });
    }

    /// The keymap: which preset, and every command with its key, on every page or on one.
    fn shortcut_settings(&mut self, ui: &mut egui::Ui) {
        // The key pressed while a command waits for one is its key; Escape leaves it be.
        if let Some(id) = self.shortcut_capture {
            let pressed = ui.input_mut(|input| {
                let mut found = None;
                input.events.retain(|event| match event {
                    egui::Event::Key { key, pressed: true, modifiers, .. } if found.is_none() => {
                        found = Some(Chord::pressed(*key, *modifiers));
                        false
                    }
                    _ => true,
                });
                found
            });
            match pressed {
                Some(chord) if chord == Chord::pressed(egui::Key::Escape, egui::Modifiers::NONE) => self.shortcut_capture = None,
                Some(chord) => self.bind_key(id, chord),
                None => {}
            }
        }
        let base = self.keymap.based_on(&self.prefs.keymap);
        egui::Grid::new("shortcut settings").num_columns(2).spacing(vec2(16.0, 8.0)).show(ui, |ui| {
            ui.label("Keymap");
            ui.horizontal(|ui| {
                let mut names: Vec<String> = Preset::ALL.iter().map(|preset| preset.name().to_string()).collect();
                names.extend(self.listed.keymaps.iter().filter(|name| !names.contains(name)).cloned().collect::<Vec<_>>());
                if !names.contains(&self.prefs.keymap) {
                    names.push(self.prefs.keymap.clone());
                }
                egui::ComboBox::from_id_salt("keymap").selected_text(self.prefs.keymap.clone()).show_ui(ui, |ui| {
                    for name in names {
                        if ui.selectable_label(self.prefs.keymap == name, &name).clicked() {
                            self.prefs.keymap = name;
                            self.shortcut_note = None;
                        }
                    }
                });
                let reset = ui.button("Reset").on_hover_text(format!("Puts the keys of {} back, on every page", base.name()));
                if reset.clicked() {
                    self.keymap.reset(base);
                    self.shortcut_note = None;
                    if Preset::ALL.iter().all(|preset| preset.name() != self.prefs.keymap) {
                        self.keep_keymap();
                    }
                    self.keys_of = None;
                }
            });
            ui.end_row();
            ui.label("Keys for");
            let title = |page: Option<Page>| page.map_or("Every page", Page::title);
            egui::ComboBox::from_id_salt("shortcut page").selected_text(title(self.shortcut_page)).show_ui(ui, |ui| {
                for page in std::iter::once(None).chain(Page::ALL.map(Some)) {
                    ui.selectable_value(&mut self.shortcut_page, page, title(page));
                }
            });
            ui.end_row();
            ui.label("Search");
            ui.add(egui::TextEdit::singleline(&mut self.shortcut_search).hint_text("A command's name").desired_width(220.0));
            ui.end_row();
        });
        let page = self.shortcut_page;
        let shown = page.map_or_else(|| self.keymap.clone(), |page| self.keymap.on(page));
        let clashes = shown.conflicts().len();
        if clashes > 0 {
            ui.label(RichText::new(format!("{clashes} keys are given to more than one command here")).small().color(ui.visuals().warn_fg_color));
        }
        // The line is there with no note too, so that the table doesn't move when one comes.
        ui.label(RichText::new(self.shortcut_note.as_deref().unwrap_or(" ")).small().color(ui.visuals().warn_fg_color));
        ui.add_space(4.0);
        // What a click in the table does, done once it is drawn.
        enum Act {
            Capture(&'static str),
            Clear(&'static str),
            Follow(&'static str, Page),
        }
        let mut act = None;
        let query = self.shortcut_search.trim().to_lowercase();
        #[cfg(test)]
        self.shortcut_rows.clear();
        // The app's commands, and after them those scripts add.
        let own = commands::commands().iter().map(|c| (c.id, format!("{} › {}", c.menu.title(), c.title())));
        let rows: Vec<(&'static str, String)> = own.chain(self.scripts.iter().map(|s| (s.id, format!("{} › {}", s.menu.title(), s.title)))).collect();
        egui::Grid::new("shortcut table").num_columns(3).striped(true).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
            for (id, name) in rows {
                if !query.is_empty() && !name.to_lowercase().contains(&query) {
                    continue;
                }
                ui.label(name);
                let chords = shown.chords(id);
                let text = if self.shortcut_capture == Some(id) {
                    RichText::new("Press a key…").strong()
                } else if chords.is_empty() {
                    RichText::new("None").weak()
                } else {
                    RichText::new(chords.iter().map(|chord| String::from(*chord)).collect::<Vec<_>>().join(", "))
                };
                let key = ui.add(egui::Button::new(text).min_size(vec2(120.0, 0.0)));
                #[cfg(test)]
                self.shortcut_rows.push((id, key.rect));
                if key.on_hover_text("Click, then press the key it is to have. Escape leaves it as it was").clicked() {
                    act = Some(Act::Capture(id));
                }
                ui.horizontal(|ui| {
                    if !chords.is_empty() && ui.small_button("×").on_hover_text("No key").clicked() {
                        act = Some(Act::Clear(id));
                    }
                    if let Some(page) = page.filter(|page| self.keymap.has_own(*page, id)) {
                        let all =
                            ui.small_button("Every page's").on_hover_text(format!("Its key on the {} page is its own: give it every page's", page.title()));
                        if all.clicked() {
                            act = Some(Act::Follow(id, page));
                        }
                    }
                });
                ui.end_row();
            }
        });
        match act {
            Some(Act::Capture(id)) => {
                self.shortcut_capture = Some(id);
                self.shortcut_note = None;
            }
            Some(Act::Clear(id)) => {
                self.keymap.unbind(id, page);
                self.keep_keymap();
            }
            Some(Act::Follow(id, page)) => {
                self.keymap.follow_all(id, page);
                self.keep_keymap();
            }
            None => {}
        }
    }

    /// Give a command the key pressed for it, and say whose it was.
    fn bind_key(&mut self, id: &'static str, chord: Chord) {
        let taken = self.keymap.bind(id, chord, self.shortcut_page);
        self.shortcut_capture = None;
        let names: Vec<&str> = taken.iter().map(|other| commands::command(other).map_or(other.as_str(), |command| command.title())).collect();
        self.shortcut_note = (!names.is_empty()).then(|| format!("{} was taken from {}", String::from(chord), names.join(", ")));
        self.keep_keymap();
    }

    /// Keep the keymap as it was changed: in its file, or, for a preset, in a file of the
    /// user's own made from it, which is the keymap in use from then on.
    fn keep_keymap(&mut self) {
        self.keys_of = None;
        let base = self.keymap.based_on(&self.prefs.keymap);
        self.keymap.made_from(base);
        let Some(folder) = self.keymaps_folder() else { return };
        let mut name = self.prefs.keymap.clone();
        if Preset::ALL.iter().any(|preset| preset.name() == name) {
            name = format!("{} (mine)", base.name());
            let mut count = 2;
            while folder.join(format!("{name}.json")).exists() {
                name = format!("{} (mine {count})", base.name());
                count += 1;
            }
        }
        match self.keymap.save(&name, &folder) {
            Ok(()) => {
                self.prefs.keymap = name.clone();
                self.keymap_name = name;
            }
            Err(why) => self.error = Some(format!("the keymap could not be saved: {why}")),
        }
    }

    /// The presets the start window offers a new project: resolutions, each named, and frame
    /// rates. A preset is changed in place, taken away by its cross, and added at the foot.
    fn new_project_settings(&mut self, ui: &mut egui::Ui) {
        let new = &mut self.prefs.new_project;
        ui.label(RichText::new("Resolutions").strong());
        let mut removed = None;
        egui::Grid::new("new project resolutions").num_columns(4).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
            for (at, preset) in new.resolutions.iter_mut().enumerate() {
                ui.add(egui::TextEdit::singleline(&mut preset.name).desired_width(170.0));
                ui.add(egui::DragValue::new(&mut preset.width).range(16..=16384).suffix(" px"));
                ui.add(egui::DragValue::new(&mut preset.height).range(16..=16384).suffix(" px"));
                if ui.small_button("✕").on_hover_text("Take this preset away").clicked() {
                    removed = Some(at);
                }
                ui.end_row();
            }
        });
        if let Some(at) = removed.filter(|_| new.resolutions.len() > 1) {
            new.resolutions.remove(at);
        }
        if ui.button("Add a Resolution").clicked() {
            new.resolutions.push(crate::prefs::Resolution { name: "1280 × 720".to_string(), width: 1280, height: 720 });
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Frame rates").strong());
        let mut removed = None;
        ui.horizontal_wrapped(|ui| {
            for (at, fps) in new.frame_rates.iter_mut().enumerate() {
                ui.add(egui::DragValue::new(fps).range(1..=240).suffix(" fps"));
                if ui.small_button("✕").on_hover_text("Take this rate away").clicked() {
                    removed = Some(at);
                }
                ui.add_space(8.0);
            }
        });
        if let Some(at) = removed.filter(|_| new.frame_rates.len() > 1) {
            new.frame_rates.remove(at);
        }
        if ui.button("Add a Frame Rate").clicked() {
            new.frame_rates.push(12);
        }
        ui.add_space(10.0);
        let first = crate::prefs::NewProject::default();
        let changed = new.resolutions != first.resolutions || new.frame_rates != first.frame_rates;
        if ui.add_enabled(changed, egui::Button::new("Reset")).on_hover_text("The presets Iskra starts with").clicked() {
            (new.resolutions, new.frame_rates) = (first.resolutions, first.frame_rates);
        }
    }

    /// The project's format: each change is a step of its history.
    fn format_settings(&mut self, ui: &mut egui::Ui) {
        let settings = self.session.project().settings.clone();
        egui::Grid::new("format settings").num_columns(2).spacing(vec2(16.0, 8.0)).show(ui, |ui| {
            ui.label("Timeline resolution");
            let named = crate::RESOLUTIONS.iter().find(|(_, width, height)| (*width, *height) == (settings.width, settings.height));
            let shown = named.map_or_else(|| format!("{} × {}", settings.width, settings.height), |(name, ..)| name.to_string());
            egui::ComboBox::from_id_salt("resolution").selected_text(shown).show_ui(ui, |ui| {
                for (name, width, height) in crate::RESOLUTIONS {
                    if ui.selectable_label((settings.width, settings.height) == (width, height), name).clicked() {
                        self.run("project.setResolution", serde_json::json!({ "width": width, "height": height }));
                        self.viewport.fit();
                    }
                }
            });
            ui.end_row();
            ui.label("Frame rate");
            egui::ComboBox::from_id_salt("frame rate").selected_text(format!("{} fps", settings.frame_rate.as_f64())).show_ui(ui, |ui| {
                for fps in crate::FRAME_RATES {
                    if ui.selectable_label((settings.frame_rate.num, settings.frame_rate.den) == (fps, 1), format!("{fps} fps")).clicked() {
                        self.run("project.setFrameRate", serde_json::json!({ "num": fps }));
                    }
                }
            });
            ui.end_row();
            ui.label("Pixel aspect ratio");
            ui.add_enabled_ui(false, |ui| {
                let mut square = 0;
                ui.horizontal(|ui| {
                    for (at, name) in ["Square", "16:9 anamorphic", "4:3"].into_iter().enumerate() {
                        ui.radio_value(&mut square, at, name);
                    }
                });
            })
            .response
            .on_disabled_hover_text("Pixels are square for now: other aspects are not built yet");
            ui.end_row();
            ui.label("New layer resolution");
            ui.horizontal(|ui| {
                for scale in [1.0, 2.0, 4.0] {
                    if ui.radio(settings.drawing_density == scale, format!("{scale}× the shot")).clicked() && settings.drawing_density != scale {
                        self.run("project.setDrawingDensity", serde_json::json!({ "density": scale }));
                    }
                }
            });
            ui.end_row();
            ui.label("Colour working space");
            ui.horizontal(|ui| {
                for (space, id, name) in [(WorkingSpace::Srgb, "srgb", "sRGB"), (WorkingSpace::Linear, "linear", "Linear")] {
                    if ui.radio(settings.working_space == space, name).clicked() && settings.working_space != space {
                        self.run("project.setWorkingSpace", serde_json::json!({ "space": id }));
                    }
                }
            });
            ui.end_row();
            ui.label("Master volume");
            if let Some(volume) = crate::sound::volume_slider(ui, &mut self.settings_volume, 0, settings.volume) {
                self.run("project.setVolume", serde_json::json!({ "volume": volume }));
            }
            ui.end_row();
        });
    }
}
