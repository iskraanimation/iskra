//! The Storyboard PDF window (plan §4.7): the storyboard's panels on pages to print, in one of
//! four modes, with its options down the left and a picture of a page on the right, made again
//! as the options change. *Export* asks where, writes the PDF on a thread of its own, and opens
//! it in the system's viewer, which prints it.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{TextureHandle, TextureOptions, vec2};
use iskra_export::pages::{self, Heading, Laid, Length, Mode, PageOptions, Paper, PrintPreset, Range};
use iskra_export::pdf::{self, Written};
use iskra_model::{Project, ShotId, Storyboard};

use crate::form;

/// What the window asks the app for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Write the PDF: the app asks where.
    Export,
}

/// A page's picture as it is shown, and what it was made of.
struct Preview {
    of: String,
    texture: TextureHandle,
    size: [u32; 2],
}

#[derive(Default)]
pub struct PrintWindow {
    pub open: bool,
    pub options: PageOptions,
    /// A PNG of each page is written beside the PDF too.
    pub pngs: bool,
    /// The presets of the user's own. The app keeps them, with the options, in its preferences.
    pub presets: Vec<PrintPreset>,
    /// The options and presets were taken from the preferences.
    pub loaded: bool,
    /// A preset's name being written, to save what is set under it.
    naming: Option<String>,
    /// The page shown, from 0, and how many there are.
    page: usize,
    pages: usize,
    preview: Option<Preview>,
    /// The PDF being written, and where.
    job: Option<(Receiver<Result<Written, String>>, PathBuf)>,
    /// How the last export went.
    pub said: Option<String>,
    /// How many times the page's picture was made: for tests.
    #[cfg(test)]
    pub made: usize,
}

/// How the window offers the paper: its sizes with names, then one of the user's own.
const PAPERS: [Paper; 7] = [Paper::A3, Paper::A4, Paper::A5, Paper::Letter, Paper::Legal, Paper::Tabloid, Paper::Own { width: 200.0, height: 300.0 }];

impl PrintWindow {
    /// The preset that is as the options are now, if one is.
    pub fn preset_in_use(&self) -> Option<usize> {
        self.presets.iter().position(|preset| preset.options == self.options && preset.pngs == self.pngs)
    }

    /// Keep what is set as a preset named `name`, in place of one of that name.
    pub fn save_preset(&mut self, name: &str) {
        let preset = PrintPreset { name: name.trim().to_string(), options: self.options.clone(), pngs: self.pngs };
        if preset.name.is_empty() {
            return;
        }
        match self.presets.iter_mut().find(|had| had.name == preset.name) {
            Some(had) => *had = preset,
            None => self.presets.push(preset),
        }
    }

    /// Set the options as preset `at` has them.
    pub fn apply_preset(&mut self, at: usize) {
        if let Some(preset) = self.presets.get(at) {
            (self.options, self.pngs) = (preset.options.clone(), preset.pngs);
        }
    }

    /// The row of presets, over the options: pick one, keep what is set as one, or let one go.
    fn presets_ui(&mut self, ui: &mut egui::Ui) {
        let in_use = self.preset_in_use();
        form::row(ui, "Preset", |ui| {
            let shown = in_use.map_or("Own settings", |at| self.presets[at].name.as_str()).to_string();
            let mut chosen = None;
            egui::ComboBox::from_id_salt("print preset").selected_text(shown).width(form::FIELD_WIDTH - 40.0).show_ui(ui, |ui| {
                if self.presets.is_empty() {
                    ui.weak("None kept yet");
                }
                for (at, preset) in self.presets.iter().enumerate() {
                    if ui.selectable_label(in_use == Some(at), &preset.name).clicked() {
                        chosen = Some(at);
                    }
                }
            });
            if let Some(at) = chosen {
                self.apply_preset(at);
            }
            let menu = ui.menu_button("…", |ui| {
                if ui.button("Keep as a Preset…").clicked() {
                    self.naming = Some(in_use.map_or_else(|| self.options.mode.name().to_string(), |at| self.presets[at].name.clone()));
                    ui.close();
                }
                if let Some(at) = in_use
                    && ui.button(format!("Delete “{}”", self.presets[at].name)).clicked()
                {
                    self.presets.remove(at);
                    ui.close();
                }
            });
            menu.response.on_hover_text("Keep what is set as a preset, or delete the one in use");
        });
        if let Some(mut name) = self.naming.take() {
            let (keep, cancel) = form::row(ui, "", |ui| {
                let field = ui.add(egui::TextEdit::singleline(&mut name).hint_text("The preset's name").desired_width(form::FIELD_WIDTH - 50.0));
                let keep = ui.button("Keep").clicked() || (field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                (keep, ui.button("Cancel").clicked())
            });
            if keep {
                self.save_preset(&name);
            } else if !cancel {
                self.naming = Some(name);
            }
        }
        ui.separator();
    }

    /// A PDF is being written.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Write the PDF of `project`'s storyboard `board` to `path`, on a thread of its own.
    /// `project` has the storyboard's shots in memory.
    pub fn start(&mut self, project: Project, board: iskra_model::StoryboardId, heading: Heading, path: PathBuf) {
        let (options, pngs) = (self.options.clone(), self.pngs);
        let (send, receive) = std::sync::mpsc::channel();
        let to = path.clone();
        std::thread::spawn(move || {
            let written = pdf::export_pages(&project, board, &options, &heading, &to, pngs).map_err(|why| why.to_string());
            let _ = send.send(written);
        });
        (self.job, self.said) = (Some((receive, path)), None);
    }

    /// The export that was running, if it has ended: what it wrote, or why it didn't.
    pub fn finished(&mut self) -> Option<Result<Written, String>> {
        let (receive, path) = self.job.as_ref()?;
        let ended = match receive.try_recv() {
            Ok(written) => written,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(format!("{} could not be written", path.display())),
        };
        self.job = None;
        self.said = Some(match &ended {
            Ok(written) if written.pages.is_empty() => format!("Written: {}", written.pdf.display()),
            Ok(written) => format!("Written: {}, and {} pictures of its pages", written.pdf.display(), written.pages.len()),
            Err(why) => why.clone(),
        });
        Some(ended)
    }

    /// The window: `project` and its storyboard `board`, the shots that are picked on the
    /// storyboard, and what the header says. `printable` gives the project with these shots'
    /// drawings in memory, read from the file where they are not, for the page's picture.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        project: &Project,
        board: &Storyboard,
        picked: &[ShotId],
        heading: &Heading,
        printable: &mut dyn FnMut(&[ShotId]) -> Project,
    ) -> Option<Asked> {
        if !self.open {
            return None;
        }
        let mut asked = None;
        let mut open = !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let window = egui::Window::new("Storyboard PDF").collapsible(false).resizable(true).default_size(vec2(860.0, 560.0)).min_size(vec2(640.0, 420.0));
        window.open(&mut open).show(ctx, |ui| {
            egui::Panel::left("print options").resizable(false).exact_size(330.0).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    self.presets_ui(ui);
                    self.options_ui(ui, board, picked);
                });
            });
            egui::Panel::bottom("print export").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.pngs, "A PNG of each page too")
                        .on_hover_text("Beside the PDF, at the pictures' resolution, named for it and the page");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let export = ui.add_enabled(!self.busy() && self.pages > 0, egui::Button::new("Export…"));
                        if export.on_hover_text("Write the PDF, and open it to print").clicked() {
                            asked = Some(Asked::Export);
                        }
                        if self.busy() {
                            ui.spinner();
                            ui.weak("Writing…");
                        } else if let Some(said) = &self.said {
                            ui.weak(said);
                        }
                    });
                });
            });
            egui::CentralPanel::default().show(ui, |ui| self.preview_ui(ui, project, board, heading, printable));
        });
        self.open = open;
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        asked
    }

    fn options_ui(&mut self, ui: &mut egui::Ui, board: &Storyboard, picked: &[ShotId]) {
        let o = &mut self.options;
        form::row(ui, "Mode", |ui| {
            ui.vertical(|ui| {
                for mode in Mode::ALL {
                    if ui.radio(o.mode == mode, mode.name()).on_hover_text(mode_hint(mode)).clicked() && o.mode != mode {
                        // The mode's own layout and texts; what is of the paper, the page and the
                        // pictures stays as it was set.
                        let kept = o.clone();
                        *o = PageOptions {
                            paper: kept.paper,
                            margin: kept.margin,
                            header: kept.header,
                            version: kept.version,
                            footer: kept.footer,
                            range: kept.range,
                            dpi: kept.dpi,
                            grey: kept.grey,
                            ..PageOptions::of(mode)
                        };
                    }
                }
            });
        });
        ui.separator();
        form::row(ui, "Paper", |ui| {
            ui.vertical(|ui| {
                egui::ComboBox::from_id_salt("print paper").selected_text(o.paper.name()).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                    for paper in PAPERS {
                        let on = std::mem::discriminant(&o.paper) == std::mem::discriminant(&paper);
                        if ui.selectable_label(on, paper.name()).clicked() && !on {
                            o.paper = paper;
                        }
                    }
                });
                if let Paper::Own { width, height } = &mut o.paper {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(width).range(50.0..=2000.0).suffix(" mm"));
                        ui.label("×");
                        ui.add(egui::DragValue::new(height).range(50.0..=2000.0).suffix(" mm"));
                    });
                }
                ui.checkbox(&mut o.landscape, "Landscape");
            });
        });
        form::row(ui, "Margins", |ui| ui.add(egui::DragValue::new(&mut o.margin).range(0.0..=50.0).speed(0.5).suffix(" mm")));
        ui.separator();
        match o.mode {
            Mode::Sheets | Mode::Thumbnails => {
                form::row(ui, "Panels", |ui| {
                    ui.add(egui::DragValue::new(&mut o.columns).range(1..=12)).on_hover_text("Across the page");
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut o.rows).range(1..=16)).on_hover_text("Down the page");
                });
            }
            Mode::Script => {
                form::row(ui, "Rows", |ui| ui.add(egui::DragValue::new(&mut o.rows).range(1..=10)));
            }
            Mode::Presentation => {}
        }
        form::row(ui, "", |ui| ui.checkbox(&mut o.page_per_shot, "A new page at each shot"));
        ui.separator();
        let texts = o.mode != Mode::Thumbnails;
        form::row(ui, "Texts", |ui| {
            ui.vertical(|ui| {
                ui.add_enabled_ui(texts, |ui| {
                    ui.checkbox(&mut o.action, "Action");
                    ui.checkbox(&mut o.dialogue, "Dialogue");
                    ui.checkbox(&mut o.notes, "Notes");
                })
                .response
                .on_disabled_hover_text("Thumbnails have the shot and panel number alone");
            });
        });
        form::row(ui, "Text size", |ui| ui.add(egui::DragValue::new(&mut o.text_size).range(5.0..=24.0).speed(0.25).suffix(" pt")));
        form::row(ui, "To write on", |ui| ui.add(egui::DragValue::new(&mut o.writing_lines).range(0..=10).suffix(" lines")))
            .on_hover_text("Empty lines under each panel's texts, to write on by hand");
        ui.separator();
        form::row(ui, "Each panel", |ui| {
            ui.vertical(|ui| {
                ui.checkbox(&mut o.numbers, "Shot and panel number");
                ui.horizontal(|ui| {
                    let mut on = o.length.is_some();
                    if ui.checkbox(&mut on, "Length").changed() {
                        o.length = on.then_some(Length::Seconds);
                    }
                    if let Some(length) = &mut o.length {
                        ui.selectable_value(length, Length::Seconds, "seconds");
                        ui.selectable_value(length, Length::Frames, "frames");
                    }
                });
                ui.checkbox(&mut o.timecode, "Timecode").on_hover_text("Where the panel starts in the film");
                ui.checkbox(&mut o.framed, "A frame round the picture");
                ui.checkbox(&mut o.transitions, "Transitions").on_hover_text("A mark on the panel where a transition leads into its shot, or out of it");
            });
        });
        ui.separator();
        form::row(ui, "Page", |ui| {
            ui.vertical(|ui| {
                ui.checkbox(&mut o.header, "Header").on_hover_text("The project's and storyboard's names, the date and the version");
                ui.add_enabled(o.header, egui::TextEdit::singleline(&mut o.version).hint_text("Version").desired_width(form::FIELD_WIDTH));
                ui.checkbox(&mut o.footer, "Page numbers");
            });
        });
        ui.separator();
        form::row(ui, "Print", |ui| {
            ui.vertical(|ui| {
                if ui.radio(o.range == Range::All, "The whole storyboard").clicked() {
                    o.range = Range::All;
                }
                let picking = matches!(o.range, Range::Shots(_));
                let said = format!("The shots picked ({})", picked.len());
                let shots = ui.add_enabled(!picked.is_empty(), egui::RadioButton::new(picking, said));
                if shots.on_disabled_hover_text("No shots are picked on the storyboard").clicked() {
                    o.range = Range::Shots(picked.to_vec());
                }
                let ranged = matches!(o.range, Range::Film { .. });
                let range = ui.add_enabled(board.range.is_some(), egui::RadioButton::new(ranged, "The range that plays"));
                if let (true, Some((first, after))) = (range.on_disabled_hover_text("The storyboard has no range set").clicked(), board.range) {
                    o.range = Range::Film { first, after };
                }
            });
        });
        // The shots picked follow the storyboard while that is chosen.
        if let Range::Shots(shots) = &mut o.range
            && shots.as_slice() != picked
            && !picked.is_empty()
        {
            *shots = picked.to_vec();
        }
        ui.separator();
        form::row(ui, "Pictures", |ui| {
            ui.vertical(|ui| {
                ui.add(egui::DragValue::new(&mut o.dpi).range(50..=600).suffix(" pixels an inch"));
                ui.checkbox(&mut o.grey, "In grey");
            });
        });
    }

    /// The page shown, made again when what it is of changes, with arrows to the others.
    fn preview_ui(&mut self, ui: &mut egui::Ui, project: &Project, board: &Storyboard, heading: &Heading, printable: &mut dyn FnMut(&[ShotId]) -> Project) {
        let laid = pages::lay_out(project, board, &self.options, heading);
        self.pages = laid.pages.len();
        self.page = self.page.min(self.pages.saturating_sub(1));
        ui.horizontal(|ui| {
            if ui.add_enabled(self.page > 0, egui::Button::new("◀")).clicked() {
                self.page -= 1;
            }
            ui.label(if self.pages == 0 { "No panels to print".to_string() } else { format!("Page {} of {}", self.page + 1, self.pages) });
            if ui.add_enabled(self.page + 1 < self.pages, egui::Button::new("▶")).clicked() {
                self.page += 1;
            }
        });
        if self.pages == 0 {
            self.preview = None;
            return;
        }
        // As large as the room, at the screen's pixels.
        let room = ui.available_size() - vec2(8.0, 8.0);
        let scale = (room.x / laid.size[0]).min(room.y / laid.size[1]).max(0.05);
        let dpi = (72.0 * scale * ui.ctx().pixels_per_point()).round().max(8.0) as u32;
        let shots: Vec<ShotId> = laid.pages[self.page].pictures.iter().map(|picture| picture.shot).collect();
        let revisions: Vec<u64> = shots.iter().filter_map(|shot| project.shot(*shot)).map(|shot| shot.revision).collect();
        let of = format!("{:?} {} {dpi} {} {revisions:?} {heading:?}", self.options, self.page, project.look);
        if self.preview.as_ref().is_none_or(|preview| preview.of != of) {
            let small = PageOptions { dpi, ..self.options.clone() };
            let printable = printable(&shots);
            let laid = Laid { pages: vec![pages::lay_out(&printable, board, &small, heading).pages.swap_remove(self.page)], ..laid };
            if let Ok(pictures) = pdf::make_pictures(&printable, &laid, small.grey)
                && let Some(made) = pdf::page_image(&laid, 0, &pictures, dpi)
            {
                let image = egui::ColorImage::from_rgb([made.width as usize, made.height as usize], &made.rgb);
                let texture = ui.ctx().load_texture("print page", image, TextureOptions::LINEAR);
                self.preview = Some(Preview { of, texture, size: [made.width, made.height] });
                #[cfg(test)]
                {
                    self.made += 1;
                }
            }
        }
        if let Some(preview) = &self.preview {
            let size = vec2(preview.size[0] as f32, preview.size[1] as f32) / ui.ctx().pixels_per_point();
            ui.vertical_centered(|ui| {
                let shown = ui.add(egui::Image::new((preview.texture.id(), size)));
                ui.painter().rect_stroke(
                    shown.rect,
                    0.0,
                    egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                    egui::StrokeKind::Outside,
                );
            });
        }
    }
}

fn mode_hint(mode: Mode) -> &'static str {
    match mode {
        Mode::Sheets => "The storyboard page: a grid of panels, the texts under each",
        Mode::Script => "A panel a row, its texts beside its picture: to read with the dialogue",
        Mode::Presentation => "One panel a page, large, its texts under it: for a pitch",
        Mode::Thumbnails => "Many small panels a page with their numbers alone: for a wall or a beat board",
    }
}
