//! The Library panel (plan §4.12): the folders of templates the user has pointed the app at.
//! It shows one folder at a time, as a file browser does: its folders, then its templates as
//! pictures with their names.
//!
//! A template is dragged out of the panel into a shot, where it is put as copies; nodes are
//! dragged onto the panel from the node view or the timeline to save them as a template. The
//! app carries both out: this panel says what was asked for.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, vec2};
use iskra_engine::Session;
use serde_json::{Value, json};

use crate::icons;
use crate::theme::Theme;

/// What the user asked for in the panel. The app carries it out.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Run(&'static str, Value),
    /// A template is being dragged out of the panel.
    Drag(PathBuf),
    /// Put a template into the shot that is open.
    Place(PathBuf),
    /// Apply a template onto the nodes the shot has, by their names: the app asks how.
    Apply(PathBuf),
    /// Delete a template's file, once the user has said so again.
    Delete(PathBuf),
    /// Ask for a folder to add to the library.
    AddFolder,
    /// Take a folder out of the library. It stays on the disk.
    RemoveFolder(PathBuf),
}

/// The side of a template's picture, in points.
const SIDE: f32 = 80.0;
/// The size of a tile: a picture or a folder, with its name under it.
const TILE: egui::Vec2 = vec2(SIDE + 8.0, SIDE + 24.0);

struct Item {
    name: String,
    path: PathBuf,
    /// What the template says about itself, for its tooltip: or why it can't be read.
    about: String,
    readable: bool,
    /// The name and the tags in small letters, which the search looks through.
    words: String,
    /// The template's picture, once it has been asked for: `Some(None)` if it has none.
    picture: Option<Option<TextureHandle>>,
}

/// What a folder holds, as it was last read.
struct Listing {
    folder: PathBuf,
    /// When the folder last changed, as its file system says.
    stamp: Option<SystemTime>,
    folders: Vec<(String, PathBuf)>,
    items: Vec<Item>,
    /// Why the folder could not be read.
    problem: Option<String>,
}

#[derive(Default)]
pub struct Library {
    /// The folder that is showing: one of the library's, or one inside it.
    folder: Option<PathBuf>,
    listing: Option<Listing>,
    /// When the folder was last looked at for changes (egui's clock, in seconds).
    checked: f64,
    search: String,
    /// The template whose name is being edited, with the text so far.
    renaming: Option<(PathBuf, String)>,
    /// The name being typed for a new folder.
    new_folder: Option<String>,
    /// Where each folder's tile was drawn, to know which a drop is on.
    pub(crate) folder_tiles: Vec<(PathBuf, Rect)>,
    /// Where each template's tile was drawn.
    #[cfg(test)]
    pub tiles: Vec<(PathBuf, Rect)>,
}

/// A folder's or a file's name, without the extension.
pub fn name_of(path: &Path) -> String {
    path.file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// The day a time is on, as the storyboard's pages print it.
pub use iskra_export::pages::date;

impl Listing {
    /// Read `folder` through the engine, as anything else that lists the library does.
    fn read(session: &mut Session, folder: &Path) -> Self {
        let stamp = std::fs::metadata(folder).and_then(|folder| folder.modified()).ok();
        let mut listing = Self { folder: folder.to_path_buf(), stamp, folders: Vec::new(), items: Vec::new(), problem: None };
        let listed = match session.execute("library.list", json!({ "folder": folder })) {
            Ok(listed) => listed,
            Err(why) => {
                listing.problem = Some(why.to_string());
                return listing;
            }
        };
        let entries = |kind: &str| listed[kind].as_array().cloned().unwrap_or_default();
        let named = |entry: &Value| Some((entry["name"].as_str()?.to_string(), PathBuf::from(entry["path"].as_str()?)));
        listing.folders = entries("folders").iter().filter_map(named).collect();
        for entry in entries("items") {
            let Some((name, path)) = named(&entry) else { continue };
            let tags: Vec<&str> = entry["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let about = match entry["problem"].as_str() {
                Some(why) => format!("{name}\nThis template can't be read: {why}"),
                None => {
                    let mut about = format!("{name}\nFrom {}, saved {}", entry["source"].as_str().unwrap_or("?"), date(entry["saved"].as_u64().unwrap_or(0)));
                    if !tags.is_empty() {
                        about += &format!("\nTags: {}", tags.join(", "));
                    }
                    if let Some(notes) = entry["notes"].as_str().filter(|notes| !notes.is_empty()) {
                        about += &format!("\n{notes}");
                    }
                    about
                }
            };
            let words = format!("{name} {}", tags.join(" ")).to_lowercase();
            listing.items.push(Item { name, path, about, readable: entry["problem"].is_null(), words, picture: None });
        }
        listing
    }
}

/// A template's picture as a texture, if it has one that can be read.
fn picture(ctx: &egui::Context, path: &Path) -> Option<TextureHandle> {
    let png = iskra_store::read_template_preview(path).ok()??;
    let (width, height, pixels) = iskra_io::decode_png(&png).ok()?;
    let mut image = egui::ColorImage::filled([width as usize, height as usize], Color32::TRANSPARENT);
    for (to, [r, g, b, a]) in image.pixels.iter_mut().zip(pixels) {
        *to = Color32::from_rgba_unmultiplied(r, g, b, a);
    }
    Some(ctx.load_texture("template", image, TextureOptions::LINEAR))
}

impl Library {
    /// Read the folder again before it is next shown: something in it has changed.
    pub fn refresh(&mut self) {
        self.listing = None;
    }

    /// The folder that is showing.
    pub fn folder(&self) -> Option<&Path> {
        self.folder.as_deref()
    }

    /// Show `folder`, which is one of the library's or inside one.
    pub fn go(&mut self, folder: PathBuf) {
        (self.folder, self.listing, self.renaming, self.new_folder) = (Some(folder), None, None, None);
    }

    /// The folder that something dropped on the panel at `pos` goes into: the folder whose
    /// tile is there, or else the one that is showing.
    pub fn folder_at(&self, pos: Pos2) -> Option<PathBuf> {
        let on_tile = self.folder_tiles.iter().find(|(_, tile)| tile.contains(pos)).map(|(folder, _)| folder.clone());
        on_tile.or_else(|| self.folder.clone())
    }

    /// Show the library, whose folders are `roots`.
    pub fn show(&mut self, ui: &mut egui::Ui, session: &mut Session, roots: &[PathBuf]) -> Vec<Action> {
        let mut actions = Vec::new();
        self.folder_tiles.clear();
        #[cfg(test)]
        self.tiles.clear();
        if roots.is_empty() {
            (self.folder, self.listing) = (None, None);
            ui.weak("The library is folders of templates on your disk: rigs, drawings and animation to use again, in this project or another.");
            if ui.button("Add a Folder…").clicked() {
                actions.push(Action::AddFolder);
            }
            return actions;
        }
        let Some((folder, root)) = self.showing(roots) else { return actions };
        // Where in its library folder it is.
        if let Ok(inside) = folder.strip_prefix(&root)
            && !inside.as_os_str().is_empty()
        {
            ui.weak(inside.iter().map(|part| part.to_string_lossy()).collect::<Vec<_>>().join(" › "));
        }
        if let Some(name) = self.new_folder.as_mut() {
            let field = ui.add(egui::TextEdit::singleline(name).hint_text("The new folder's name").desired_width(f32::INFINITY));
            if field.lost_focus() {
                let keep = !ui.input(|i| i.key_pressed(egui::Key::Escape)) && !name.trim().is_empty();
                if let Some(name) = self.new_folder.take().filter(|_| keep) {
                    actions.push(Action::Run("library.newFolder", json!({ "folder": folder, "name": name })));
                }
            } else {
                field.request_focus();
            }
        }

        // The folder is read again when it has changed on the disk, which is looked for now
        // and then, not every frame.
        let now = ui.input(|i| i.time);
        if let Some(listing) = self.listing.as_ref().filter(|listing| listing.folder == folder) {
            if now - self.checked > 1.0 {
                self.checked = now;
                if std::fs::metadata(&folder).and_then(|folder| folder.modified()).ok() != listing.stamp {
                    self.listing = None;
                }
            }
        } else {
            self.listing = None;
        }
        let listing = self.listing.get_or_insert_with(|| Listing::read(session, &folder));
        if let Some(problem) = &listing.problem {
            ui.colored_label(ui.visuals().error_fg_color, format!("This folder can't be read: {problem}"));
            return actions;
        }
        let theme = Theme::of(ui.ctx());
        let wanted = self.search.trim().to_lowercase();
        let mut entered = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if listing.folders.is_empty() && listing.items.is_empty() {
                ui.weak("Nothing here yet. Drag a node from the node view, or a row from the timeline, onto this panel to save it as a template.");
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for (name, path) in &listing.folders {
                    let (tile, response) = ui.allocate_exact_size(TILE, Sense::click());
                    self.folder_tiles.push((path.clone(), tile));
                    if response.hovered() {
                        ui.painter().rect_filled(tile, 4.0, theme.surface_raised);
                    }
                    let at = Rect::from_center_size(tile.center_top() + vec2(0.0, 4.0 + SIDE / 2.0), vec2(40.0, 40.0));
                    icons::image("folder-open", 40.0, theme.text_muted).paint_at(ui, at);
                    caption(ui, &theme, tile, name);
                    if response.on_hover_text(format!("{name}\nDouble-click to open; drop nodes here to save them into it")).double_clicked() {
                        entered = Some(path.clone());
                    }
                }
                for item in listing.items.iter_mut().filter(|item| wanted.split_whitespace().all(|word| item.words.contains(word))) {
                    let (tile, response) = ui.allocate_exact_size(TILE, Sense::click_and_drag());
                    #[cfg(test)]
                    self.tiles.push((item.path.clone(), tile));
                    if response.hovered() {
                        ui.painter().rect_filled(tile, 4.0, theme.surface_raised);
                    }
                    // Paper, so that a template looks as it does in the viewport in either theme.
                    let paper = Rect::from_min_size(tile.min + vec2(4.0, 4.0), vec2(SIDE, SIDE));
                    ui.painter().rect(paper, 0.0, Color32::WHITE, Stroke::new(1.0, theme.border), egui::StrokeKind::Outside);
                    if ui.is_rect_visible(tile) {
                        let texture = item.picture.get_or_insert_with(|| picture(ui.ctx(), &item.path).filter(|_| item.readable));
                        match texture {
                            Some(texture) => {
                                // The whole picture, as large as the paper lets it be.
                                let size = texture.size_vec2();
                                let fitted = size * ((SIDE - 6.0) / size.max_elem().max(1.0)).min(2.0);
                                let whole = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                                ui.painter().image(texture.id(), Rect::from_center_size(paper.center(), fitted), whole, Color32::WHITE);
                            }
                            None => {
                                let mark = if item.readable { "nothing painted" } else { "can't be read" };
                                ui.painter().text(paper.center(), Align2::CENTER_CENTER, mark, FontId::proportional(10.0), Color32::from_gray(120));
                            }
                        }
                    }
                    if let Some((_, text)) = self.renaming.as_mut().filter(|(renamed, _)| *renamed == item.path) {
                        let label = Rect::from_min_max(egui::pos2(tile.min.x + 2.0, paper.max.y + 1.0), tile.max - vec2(2.0, 0.0));
                        let mut inside = ui.new_child(egui::UiBuilder::new().max_rect(label));
                        let edit = inside.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
                        if edit.lost_focus() {
                            // Enter keeps the new name; Escape, or the same name, changes nothing.
                            let keep = !inside.input(|i| i.key_pressed(egui::Key::Escape)) && *text != item.name;
                            if let Some((path, name)) = self.renaming.take().filter(|_| keep) {
                                actions.push(Action::Run("library.rename", json!({ "path": path, "name": name })));
                            }
                        } else {
                            edit.request_focus();
                        }
                    } else {
                        caption(ui, &theme, tile, &item.name);
                    }
                    if response.drag_started() && item.readable {
                        actions.push(Action::Drag(item.path.clone()));
                    }
                    if response.double_clicked() && item.readable {
                        actions.push(Action::Place(item.path.clone()));
                    }
                    response.context_menu(|ui| {
                        if ui.add_enabled(item.readable, egui::Button::new("Put into the Shot")).on_hover_text("As copies: new nodes beside what the shot has").clicked() {
                            actions.push(Action::Place(item.path.clone()));
                            ui.close();
                        }
                        let hint = "Onto the nodes the shot already has, found by their names: their poses, animation, timing and drawings become the template's. \
                                    Nothing is copied. A window shows what would go where first";
                        if ui.add_enabled(item.readable, egui::Button::new("Apply onto the Shot's Own…")).on_hover_text(hint).clicked() {
                            actions.push(Action::Apply(item.path.clone()));
                            ui.close();
                        }
                        if ui.button("Rename").clicked() {
                            self.renaming = Some((item.path.clone(), item.name.clone()));
                            ui.close();
                        }
                        if ui.button("Delete…").on_hover_text("Deletes the template's file. Shots it was put into keep their copies").clicked() {
                            actions.push(Action::Delete(item.path.clone()));
                            ui.close();
                        }
                    });
                    response.on_hover_text(format!("{}\nDrag into the viewport, the node view or the timeline to put it into the shot", item.about));
                }
            });
        });
        if let Some(folder) = entered {
            self.go(folder);
        }
        actions
    }

    /// The folder showing and the library's folder it is in: the library's first folder, where
    /// the one that was showing is no longer in the library. None where the library has none.
    fn showing(&mut self, roots: &[PathBuf]) -> Option<(PathBuf, PathBuf)> {
        let first = roots.first()?.clone();
        let root_of = |folder: &Path| roots.iter().filter(|root| folder.starts_with(root)).max_by_key(|root| root.as_os_str().len()).cloned();
        match self.folder.clone().and_then(|folder| Some((folder.clone(), root_of(&folder)?))) {
            Some(showing) => Some(showing),
            None => {
                self.go(first.clone());
                Some((first.clone(), first))
            }
        }
    }

    /// A part of the bar over the folder: which of the library's folders it is in, the way back
    /// up, a new folder and reading it again, or the search.
    pub fn bar_part(&mut self, ui: &mut egui::Ui, part: &str, roots: &[PathBuf]) -> Vec<Action> {
        let mut actions = Vec::new();
        let Some((folder, root)) = self.showing(roots) else { return actions };
        match part {
            "libfolders" => {
                ui.menu_button(format!("{}  ⌄", name_of(&root)), |ui| {
                    for other in roots {
                        if ui.radio(*other == root, name_of(other)).on_hover_text(other.display().to_string()).clicked() {
                            self.go(other.clone());
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Add a Folder…").clicked() {
                        actions.push(Action::AddFolder);
                        ui.close();
                    }
                    if ui.button("Take This Folder Out of the Library").on_hover_text("The folder and its templates stay on the disk").clicked() {
                        actions.push(Action::RemoveFolder(root.clone()));
                        ui.close();
                    }
                })
                .response
                .on_hover_text("The folders of the library");
            }
            "libnav" => {
                let up = folder.parent().filter(|_| folder != root);
                if ui.add_enabled(up.is_some(), egui::Button::new("Up")).on_hover_text("To the folder this one is in").clicked()
                    && let Some(up) = up
                {
                    self.go(up.to_path_buf());
                }
                if ui.button("New Folder").clicked() {
                    self.new_folder = Some(String::new());
                }
                if ui.button("Refresh").on_hover_text("Read the folder again").clicked() {
                    self.refresh();
                }
            }
            "libsearch" => drop(ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search names and tags").desired_width(160.0))),
            _ => {}
        }
        actions
    }
}

/// A tile's name, under its picture.
fn caption(ui: &egui::Ui, theme: &Theme, tile: Rect, name: &str) {
    ui.painter().with_clip_rect(tile).text(tile.center_bottom() - vec2(0.0, 3.0), Align2::CENTER_BOTTOM, name, FontId::proportional(11.0), theme.text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_shown_as_its_day() {
        assert_eq!((date(0), date(86_399), date(86_400)), ("1970-01-01".to_string(), "1970-01-01".to_string(), "1970-01-02".to_string()));
        assert_eq!((date(951_782_400), date(1_760_000_000)), ("2000-02-29".to_string(), "2025-10-09".to_string()), "leap days too");
    }
}
