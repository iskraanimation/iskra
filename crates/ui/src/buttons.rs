//! The script buttons (plan §9.4): one row of buttons the user fills, each running a command, a
//! script's or one of the app's, and shown by its title or by an icon of the user's own. The row
//! is a part of the bars, *Script Buttons*, put in any bar, and is the same in every bar it is in.
//! The **+** at its end adds a button; a button's right-click menu renames it, gives it an icon,
//! moves it or takes it out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{Color32, TextureHandle, TextureOptions, vec2};
use serde::{Deserialize, Serialize};

/// The side of a button's icon, as the bars' switches have theirs.
const ICON: f32 = 16.0;

/// A button of the row: the command it runs, by its id, and how it is shown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScriptButton {
    pub command: String,
    /// The user's own title for it, in place of the command's.
    pub title: Option<String>,
    /// An image file of the user's, PNG or SVG, shown in place of the title.
    pub icon: Option<PathBuf>,
}

/// What the app says of a button's command: none for a command that is not there, as one of a
/// script taken out of the Scripts folder.
pub struct Known {
    pub id: &'static str,
    pub title: &'static str,
    /// Its menu and title, and the key it has, for the button's tooltip.
    pub hint: String,
    pub enabled: bool,
}

/// What was done on the row that the app does.
#[derive(Default)]
pub struct Done {
    /// The command of the button that was clicked.
    pub run: Option<&'static str>,
    /// The button to ask an icon file for.
    pub pick_icon: Option<usize>,
}

/// An icon file as it is drawn, or `None` for one that can't be read.
type Loaded = Option<(egui::Image<'static>, Option<TextureHandle>)>;

/// The row's state that is not kept: the icon files read, and what is typed in the **+** list.
#[derive(Default)]
pub struct Row {
    icons: HashMap<PathBuf, Loaded>,
    query: String,
    /// Each button as it was last drawn, by its title, and whether it had its icon.
    #[cfg(test)]
    pub drawn: Vec<(String, egui::Rect, bool)>,
}

impl Row {
    /// Read the icon files again when they are next drawn, as they may have changed.
    pub fn forget_icons(&mut self) {
        self.icons.clear();
    }

    /// The icon in `path`, read once.
    fn icon(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::Image<'static>> {
        let loaded = self.icons.entry(path.to_path_buf()).or_insert_with(|| load(ctx, path));
        loaded.as_ref().map(|(image, _)| image.clone().fit_to_exact_size(vec2(ICON, ICON)))
    }

    /// Draw the row: `buttons`, each with what the app knows of its command in `known`, then the
    /// **+**, whose list is of the commands `choices` gives for what is typed, by id and label.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        buttons: &mut Vec<ScriptButton>,
        known: &[Option<Known>],
        choices: impl Fn(&str) -> Vec<(&'static str, String)>,
    ) -> Done {
        let mut done = Done::default();
        let (mut moved, mut removed) = (None, None);
        let count = buttons.len();
        #[cfg(test)]
        self.drawn.clear();
        for (index, (button, known)) in buttons.iter_mut().zip(known).enumerate() {
            let title = button.title.clone().or_else(|| known.as_ref().map(|known| known.title.to_string())).unwrap_or_else(|| button.command.clone());
            let icon = button.icon.as_deref().and_then(|path| self.icon(ui.ctx(), path));
            #[cfg(test)]
            let iconed = icon.is_some();
            let widget = match icon {
                Some(icon) => egui::Button::image(icon),
                None => egui::Button::new(&title),
            };
            let enabled = known.as_ref().is_some_and(|known| known.enabled);
            let hint = match known {
                Some(known) => known.hint.clone(),
                None => format!("`{}` is no command now: its script is gone, or was not read. Right-click to take the button out", button.command),
            };
            let unread = button.icon.as_ref().filter(|path| !self.icons.get(*path).is_some_and(Option::is_some));
            let hint = match unread {
                Some(path) => format!("{hint}\nIts icon, {}, can't be read", path.display()),
                None => hint,
            };
            let response = ui.add_enabled(enabled, widget).on_hover_text(hint).on_disabled_hover_text(format!("{title}: can't be run now"));
            #[cfg(test)]
            self.drawn.push((title.clone(), response.rect, iconed));
            if response.clicked() {
                done.run = known.as_ref().map(|known| known.id);
            }
            response.context_menu(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Title");
                    let mut typed = button.title.clone().unwrap_or_default();
                    let field = egui::TextEdit::singleline(&mut typed).hint_text(known.as_ref().map_or("", |known| known.title)).desired_width(160.0);
                    if ui.add(field).on_hover_text("The button's own title. Empty, it has its command's").changed() {
                        button.title = (!typed.trim().is_empty()).then_some(typed);
                    }
                });
                if ui.button("Choose Icon…").on_hover_text("A PNG or SVG file of your own, shown in place of the title").clicked() {
                    done.pick_icon = Some(index);
                    ui.close();
                }
                if button.icon.is_some() && ui.button("No Icon").on_hover_text("Show the title again").clicked() {
                    button.icon = None;
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(index > 0, egui::Button::new("Move Left")).clicked() {
                    moved = Some((index, index - 1));
                }
                if ui.add_enabled(index + 1 < count, egui::Button::new("Move Right")).clicked() {
                    moved = Some((index, index + 1));
                }
                ui.separator();
                if ui.button("Remove").clicked() {
                    removed = Some(index);
                    ui.close();
                }
            });
        }
        if let Some((from, to)) = moved {
            buttons.swap(from, to);
        }
        if let Some(index) = removed {
            buttons.remove(index);
        }
        let query = &mut self.query;
        ui.menu_button("+", |ui| {
            let field = ui.add(egui::TextEdit::singleline(query).hint_text("Search commands").desired_width(260.0));
            field.request_focus();
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                let found = choices(query);
                for (id, label) in &found {
                    if ui.button(label).clicked() {
                        buttons.push(ScriptButton { command: id.to_string(), ..Default::default() });
                        query.clear();
                        ui.close();
                    }
                }
                if found.is_empty() {
                    ui.weak("No command matches");
                }
            });
        })
        .response
        .on_hover_text("Add a button: any command, a script's or the app's");
        done
    }
}

/// Read the icon file at `path`: an SVG as it is, a PNG as a texture.
fn load(ctx: &egui::Context, path: &Path) -> Loaded {
    let bytes = std::fs::read(path).ok()?;
    if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("svg")) {
        return Some((egui::Image::from_bytes(format!("bytes://script-buttons/{}", path.display()), bytes), None));
    }
    let (width, height, pixels) = iskra_io::decode_png(&bytes).ok()?;
    let mut image = egui::ColorImage::filled([width as usize, height as usize], Color32::TRANSPARENT);
    for (to, [r, g, b, a]) in image.pixels.iter_mut().zip(pixels) {
        *to = Color32::from_rgba_unmultiplied(r, g, b, a);
    }
    let texture = ctx.load_texture(format!("script button {}", path.display()), image, TextureOptions::LINEAR);
    Some((egui::Image::from_texture(&texture), Some(texture)))
}
