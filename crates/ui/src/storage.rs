//! The Media Storage panel (plan §7.11): the disk, as an editor's media page shows it. Places
//! and their folders are a tree on the left; the files of the folder that is open are listed
//! on the right. From here a project is opened, a shot that was sent out is brought back, and
//! a folder of templates is added to the library. Images and sound are brought in from here
//! when the project can hold them (M5); until then they are listed, greyed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{Align, Layout, Rect, Sense};
use egui_extras::{Column, TableBuilder};

use crate::library::{date, name_of};
use crate::theme::Theme;

/// What the user asked for in the panel. The app carries it out.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Open a project file in place of the open project.
    Open(PathBuf),
    /// Bring a shot back from a file, after showing what would change.
    BringBack(PathBuf),
    /// Add a folder to the library's.
    AddToLibrary(PathBuf),
    AddFavourite(PathBuf),
    RemoveFavourite(PathBuf),
    /// Bring a picture or a sound into the project, as a media item.
    Import(PathBuf),
}

/// What a file is to the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Folder,
    Project,
    Template,
    /// A picture or a sound: of use once the project can hold media (M5).
    Media,
    Other,
}

impl Kind {
    fn of(path: &Path, is_folder: bool) -> Kind {
        let extension = path.extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
        match extension.as_str() {
            _ if is_folder => Kind::Folder,
            "iskra" => Kind::Project,
            "iskratpl" => Kind::Template,
            "png" | "jpg" | "jpeg" | "tif" | "tiff" | "exr" | "psd" | "ora" | "kra" | "wav" | "aif" | "aiff" | "mp3" | "flac" | "ogg" | "m4a" | "mov"
            | "mp4" => Kind::Media,
            _ => Kind::Other,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Folder => "Folder",
            Kind::Project => "Iskra project",
            Kind::Template => "Library template",
            Kind::Media => "Media",
            Kind::Other => "",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub kind: Kind,
    /// In bytes; nothing for a folder.
    pub size: u64,
    /// When it was last changed, in seconds since the start of 1970.
    pub modified: u64,
}

/// What `folder` holds, the folders first, each by name. Files whose names begin with a dot
/// are left out. An error if the folder can't be read.
pub fn list(folder: &Path) -> std::io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = std::fs::read_dir(folder)?
        .flatten()
        .filter_map(|entry| {
            let (path, about) = (entry.path(), entry.metadata().ok()?);
            let name = entry.file_name().to_string_lossy().into_owned();
            let modified = about.modified().ok().and_then(|at| at.duration_since(UNIX_EPOCH).ok()).map_or(0, |since| since.as_secs());
            let kind = Kind::of(&path, about.is_dir());
            (!name.starts_with('.')).then_some(Entry { path, name, kind, size: if about.is_dir() { 0 } else { about.len() }, modified })
        })
        .collect();
    entries.sort_by_key(|entry| (entry.kind != Kind::Folder, entry.name.to_lowercase()));
    Ok(entries)
}

/// A size in bytes as people say it.
pub fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.0} KB", bytes as f64 / 1024.0),
        1_048_576..1_073_741_824 => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
        _ => format!("{:.2} GB", bytes as f64 / 1_073_741_824.0),
    }
}

/// The folder a user's own files are in, if the system says where.
pub fn home() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"].iter().find_map(std::env::var_os).filter(|value| !value.is_empty()).map(PathBuf::from)
}

#[derive(Default)]
pub struct Storage {
    /// The folder whose files are listed.
    pub folder: Option<PathBuf>,
    /// What it holds, as last read, or why it could not be read; and when it last changed.
    listing: Option<(PathBuf, Result<Vec<Entry>, String>, Option<SystemTime>)>,
    /// When the folder was last looked at for changes (egui's clock, in seconds).
    checked: f64,
    /// The folders in each folder of the tree that has been opened.
    folders: HashMap<PathBuf, Vec<PathBuf>>,
    /// Where each file's row was drawn.
    #[cfg(test)]
    pub rows: Vec<(PathBuf, Rect)>,
}

impl Storage {
    /// Show `folder`'s files.
    pub fn go(&mut self, folder: PathBuf) {
        (self.folder, self.listing) = (Some(folder), None);
    }

    /// Read the disk again: the folder that is open, and the tree.
    pub fn refresh(&mut self) {
        self.listing = None;
        self.folders.clear();
    }

    /// The panel: places and their folders on the left, the open folder's files on the right.
    pub fn show(&mut self, ui: &mut egui::Ui, favourites: &[PathBuf]) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.folder.is_none() {
            self.folder = favourites.first().cloned().or_else(home);
        }
        let area = ui.available_rect_before_wrap();
        let places = Rect::from_min_max(area.min, egui::pos2((area.min.x + 200.0).min(area.max.x), area.max.y));
        let files = Rect::from_min_max(egui::pos2(places.max.x + 8.0, area.min.y), area.max);
        let mut left = ui.new_child(egui::UiBuilder::new().max_rect(places).id_salt("places"));
        left.set_clip_rect(places.intersect(ui.clip_rect()));
        egui::ScrollArea::vertical().id_salt("places").auto_shrink(false).show(&mut left, |ui| self.places(ui, favourites, &mut actions));
        ui.painter().vline(places.max.x + 4.0, area.y_range(), egui::Stroke::new(1.0, Theme::of(ui.ctx()).border));
        if files.width() > 40.0 {
            let mut right = ui.new_child(egui::UiBuilder::new().max_rect(files).id_salt("files"));
            right.set_clip_rect(files.intersect(ui.clip_rect()));
            self.files(&mut right, &mut actions);
        }
        ui.allocate_rect(area, Sense::hover());
        actions
    }

    /// The places to start from, each with its folders under it as a tree.
    fn places(&mut self, ui: &mut egui::Ui, favourites: &[PathBuf], actions: &mut Vec<Action>) {
        if !favourites.is_empty() {
            ui.weak("Favourites");
            for folder in favourites {
                self.branch(ui, folder, true, actions);
            }
            ui.add_space(6.0);
        }
        ui.weak("Places");
        if let Some(home) = home() {
            self.branch(ui, &home, false, actions);
        }
        // The whole disk; on macOS the other disks are in /Volumes.
        let roots: &[&str] = if cfg!(windows) {
            &["C:\\"]
        } else if cfg!(target_os = "macos") {
            &["/Volumes", "/"]
        } else {
            &["/"]
        };
        for root in roots.iter().map(Path::new).filter(|root| root.is_dir()) {
            self.branch(ui, root, false, actions);
        }
    }

    /// A folder of the tree: a click shows its files, and the arrow opens the folders in it.
    fn branch(&mut self, ui: &mut egui::Ui, folder: &Path, favourite: bool, actions: &mut Vec<Action>) {
        let id = ui.make_persistent_id(("storage", folder));
        let name = if folder.parent().is_none() { folder.display().to_string() } else { name_of(folder) };
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
        let open = state.is_open();
        let header = state.show_header(ui, |ui| {
            let label = ui.selectable_label(self.folder.as_deref() == Some(folder), name).on_hover_text(folder.display().to_string());
            if label.clicked() {
                self.go(folder.to_path_buf());
            }
            label.context_menu(|ui| {
                let (title, action) = if favourite {
                    ("Take Off the Favourites", Action::RemoveFavourite(folder.to_path_buf()))
                } else {
                    ("Add to Favourites", Action::AddFavourite(folder.to_path_buf()))
                };
                if ui.button(title).clicked() {
                    actions.push(action);
                    ui.close();
                }
                if ui.button("Add to the Library").on_hover_text("As a folder of templates the Library panel shows").clicked() {
                    actions.push(Action::AddToLibrary(folder.to_path_buf()));
                    ui.close();
                }
            });
        });
        // The folders in it are read when it is first opened, and again on Refresh.
        let inside: Vec<PathBuf> = if open {
            self.folders
                .entry(folder.to_path_buf())
                .or_insert_with(|| list(folder).unwrap_or_default().into_iter().filter(|entry| entry.kind == Kind::Folder).map(|entry| entry.path).collect())
                .clone()
        } else {
            Vec::new()
        };
        header.body(|ui| {
            for folder in &inside {
                self.branch(ui, folder, false, actions);
            }
        });
    }

    /// A part of the bar over the disk: the way up and reading it again, the folder open as a
    /// favourite, or where it is.
    pub fn bar_part(&mut self, ui: &mut egui::Ui, part: &str, favourites: &[PathBuf]) -> Vec<Action> {
        let mut actions = Vec::new();
        let Some(folder) = self.folder.clone() else { return actions };
        match part {
            "disknav" => {
                if ui.add_enabled(folder.parent().is_some(), egui::Button::new("Up")).on_hover_text("To the folder this one is in").clicked()
                    && let Some(up) = folder.parent()
                {
                    self.go(up.to_path_buf());
                }
                if ui.button("Refresh").on_hover_text("Read the disk again").clicked() {
                    self.refresh();
                }
            }
            "diskfavourite" => {
                let hint = "Keep this folder among the favourites, at the top of the folders";
                if ui.add_enabled(!favourites.contains(&folder), egui::Button::new("Add to Favourites")).on_hover_text(hint).clicked() {
                    actions.push(Action::AddFavourite(folder.clone()));
                }
            }
            "diskfolder" => drop(ui.weak(folder.display().to_string())),
            _ => {}
        }
        actions
    }

    /// The files of the folder that is open.
    fn files(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(folder) = self.folder.clone() else {
            ui.weak("Pick a folder on the left.");
            return;
        };
        // The folder is read again when it has changed on the disk, which is looked for now
        // and then, not every frame.
        let now = ui.input(|i| i.time);
        let stamp = |folder: &Path| std::fs::metadata(folder).and_then(|folder| folder.modified()).ok();
        if let Some((listed, _, at)) = &self.listing {
            if *listed != folder {
                self.listing = None;
            } else if now - self.checked > 1.0 {
                self.checked = now;
                if stamp(&folder) != *at {
                    self.listing = None;
                }
            }
        }
        let (_, listing, _) = self.listing.get_or_insert_with(|| (folder.clone(), list(&folder).map_err(|why| why.to_string()), stamp(&folder)));
        let entries = match listing {
            Ok(entries) => entries.clone(),
            Err(why) => {
                ui.colored_label(ui.visuals().error_fg_color, format!("This folder can't be read: {why}"));
                return;
            }
        };
        #[cfg(test)]
        self.rows.clear();
        let theme = Theme::of(ui.ctx());
        let mut entered = None;
        let table = TableBuilder::new(ui).striped(true).resizable(true).sense(Sense::click()).cell_layout(Layout::left_to_right(Align::Center));
        let table = table
            .column(Column::auto().at_least(180.0).clip(true))
            .column(Column::auto().at_least(90.0).clip(true))
            .column(Column::auto().at_least(60.0))
            .column(Column::remainder().at_least(70.0));
        table
            .header(20.0, |mut header| {
                for title in ["Name", "Type", "Size", "Changed"] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|mut body| {
                for entry in &entries {
                    body.row(20.0, |mut line| {
                        // What the app has no use for yet is there, and quiet.
                        let usable = matches!(entry.kind, Kind::Folder | Kind::Project | Kind::Template);
                        let color = if usable { theme.text } else { theme.text_muted.gamma_multiply(0.7) };
                        let cells = [
                            entry.name.clone(),
                            entry.kind.label().to_string(),
                            if entry.kind == Kind::Folder { String::new() } else { size(entry.size) },
                            date(entry.modified),
                        ];
                        for (index, cell) in cells.into_iter().enumerate() {
                            line.col(|ui| {
                                if index == 0 && entry.kind == Kind::Folder {
                                    ui.add(crate::icons::image("folder-open", 14.0, theme.text_muted));
                                }
                                let color = if index == 0 { color } else { theme.text_muted };
                                ui.add(egui::Label::new(egui::RichText::new(cell).color(color)).selectable(false).truncate());
                            });
                        }
                        let response = line.response();
                        #[cfg(test)]
                        self.rows.push((entry.path.clone(), response.rect));
                        if response.double_clicked() {
                            match entry.kind {
                                Kind::Folder => entered = Some(entry.path.clone()),
                                Kind::Project => actions.push(Action::Open(entry.path.clone())),
                                Kind::Media if iskra_io::is_media(&entry.path) => actions.push(Action::Import(entry.path.clone())),
                                _ => {}
                            }
                        }
                        let hint = match entry.kind {
                            Kind::Folder => "Double-click to open",
                            Kind::Project => "Double-click to open it as the project. Right-click to bring it back as a shot that was sent out",
                            Kind::Template => "A library template. Add its folder to the library to use it",
                            Kind::Media if iskra_io::is_media(&entry.path) => "Double-click to bring it into the project, into the bin that is open",
                            Kind::Media => "A kind of media that is not read yet",
                            Kind::Other => "",
                        };
                        response.context_menu(|ui| match entry.kind {
                            Kind::Project => {
                                if ui.button("Open as the Project").clicked() {
                                    actions.push(Action::Open(entry.path.clone()));
                                    ui.close();
                                }
                                if ui
                                    .button("Bring Back as a Shot…")
                                    .on_hover_text("For the file of a shot that was sent out: it takes the place of the shot it came from")
                                    .clicked()
                                {
                                    actions.push(Action::BringBack(entry.path.clone()));
                                    ui.close();
                                }
                            }
                            Kind::Folder => {
                                if ui.button("Add to Favourites").clicked() {
                                    actions.push(Action::AddFavourite(entry.path.clone()));
                                    ui.close();
                                }
                                if ui.button("Add to the Library").clicked() {
                                    actions.push(Action::AddToLibrary(entry.path.clone()));
                                    ui.close();
                                }
                            }
                            Kind::Template => {
                                if ui.button("Add Its Folder to the Library").clicked() {
                                    actions.extend(entry.path.parent().map(|folder| Action::AddToLibrary(folder.to_path_buf())));
                                    ui.close();
                                }
                            }
                            Kind::Media if iskra_io::is_media(&entry.path) => {
                                if ui.button("Import into the Project").on_hover_text("As a media item that refers to this file; nothing is copied").clicked() {
                                    actions.push(Action::Import(entry.path.clone()));
                                    ui.close();
                                }
                            }
                            Kind::Media | Kind::Other => {
                                ui.weak("Nothing to do with this file yet");
                            }
                        });
                        if !hint.is_empty() {
                            response.on_hover_text(hint);
                        }
                    });
                }
            });
        if let Some(folder) = entered {
            self.go(folder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_listed_folders_first_with_what_each_file_is() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("shots")).unwrap();
        for (name, bytes) in [("Film.iskra", 10), ("arm.iskratpl", 2000), ("voice.WAV", 3_000_000), ("notes.txt", 0), (".hidden", 1)] {
            std::fs::write(dir.path().join(name), vec![0u8; bytes]).unwrap();
        }
        let listed = list(dir.path()).unwrap();
        let said: Vec<(&str, Kind, u64)> = listed.iter().map(|entry| (entry.name.as_str(), entry.kind, entry.size)).collect();
        let expected = [
            ("shots", Kind::Folder, 0),
            ("arm.iskratpl", Kind::Template, 2000),
            ("Film.iskra", Kind::Project, 10),
            ("notes.txt", Kind::Other, 0),
            ("voice.WAV", Kind::Media, 3_000_000),
        ];
        assert_eq!(said, expected, "the folder first, the rest by name whatever its capitals; no hidden file");
        assert!(listed[1].modified > 1_700_000_000 && list(&dir.path().join("none")).is_err());
        assert_eq!(
            (size(0), size(2000), size(3_000_000), size(5_000_000_000)),
            ("0 B".to_string(), "2 KB".to_string(), "2.9 MB".to_string(), "4.66 GB".to_string())
        );
    }
}
