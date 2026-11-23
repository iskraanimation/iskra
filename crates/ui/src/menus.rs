//! The app's menus as a description (plan §7.5): what each menu has, in its order, and how
//! each thing in it is now. The window draws its menus from it, and so does a menu bar that
//! is not the window's, the system's on macOS: both show the same, and what is chosen in
//! either is run the same way, by its id.
//!
//! A menu has the commands the command list gives it, and after them the settings that are
//! picked from a list: each of those is a menu within the menu.

use std::path::PathBuf;

use egui::KeyboardShortcut;
use serde_json::json;

use crate::commands::{self, Menu};
use crate::dock::Page;
use crate::keymap::Keymap;
use crate::{IskraApp, prefs};

/// What is in a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuNode {
    /// A line between two groups.
    Separator,
    /// Something to choose.
    Item(MenuEntry),
    /// Words that say something and can't be chosen.
    Note(String),
    /// A menu within the menu.
    Menu { title: String, enabled: bool, items: Vec<MenuNode> },
}

/// Something to choose in a menu, as it is now.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuEntry {
    /// What it is run by ([`IskraApp::run_menu_command`]): a command's id, or a setting's.
    pub id: String,
    /// Its name as it is now: *Undo* says what it undoes.
    pub title: String,
    /// Whether it can be chosen now.
    pub enabled: bool,
    /// For what is on or off, or one of several: whether it is on.
    pub checked: Option<bool>,
    /// A command's shortcut, the first the keymap has for it.
    pub shortcut: Option<KeyboardShortcut>,
    /// What the window says of it when the pointer rests on it.
    pub hint: Option<String>,
    /// One of several of which one is on, where the window draws a round mark.
    radio: bool,
    action: Chosen,
}

/// What choosing an entry does.
#[derive(Clone, Debug, PartialEq)]
enum Chosen {
    Command(&'static str),
    /// A command a script adds, by its id.
    Script(&'static str),
    OpenRecent(PathBuf),
    ForgetMissing,
    ClearRecent,
    /// How much memory the pictures of effects may take, in MB.
    Aspect(Option<f64>),
    /// What is outside the camera's frame shown or not.
    Clip,
    Guide(usize),
    PageShown(Page),
    Workspace(String),
    DeleteWorkspace(String),
    /// The shot shown in the styles of this name, or in the palettes' own colours.
    Style(Option<String>),
    /// The display the viewer looks through: another one, or the main one.
    Display(Option<iskra_model::DisplayId>),
}

/// What the menus list that is read from the disk, looked up now and then and not on every
/// frame: which of the recent files are still there, and the keymaps of the user's own.
#[derive(Default)]
pub(crate) struct Listed {
    /// When it was looked up, in the window's seconds.
    at: Option<f64>,
    recent: Vec<(PathBuf, bool)>,
    pub(crate) keymaps: Vec<String>,
}

/// How long what was read from the disk for the menus is taken to hold, in seconds.
const LISTED_FOR: f64 = 2.0;

impl Listed {
    /// Look up again what is older than [`LISTED_FOR`], or is of other recent files.
    pub(crate) fn refresh(&mut self, now: f64, recent: &[PathBuf], keymaps: Option<&std::path::Path>) {
        let same = self.recent.len() == recent.len() && self.recent.iter().zip(recent).all(|((had, _), has)| had == has);
        if same && self.at.is_some_and(|at| (now - at).abs() < LISTED_FOR) {
            return;
        }
        self.at = Some(now);
        self.recent = recent.iter().map(|file| (file.clone(), file.is_file())).collect();
        self.keymaps = keymaps.map(Keymap::files).unwrap_or_default();
    }
}

fn entry(id: impl Into<String>, title: impl Into<String>, action: Chosen) -> MenuEntry {
    MenuEntry { id: id.into(), title: title.into(), enabled: true, checked: None, shortcut: None, hint: None, radio: false, action }
}

/// One of several, of which the one that is `on` is marked.
fn one_of(id: impl Into<String>, title: impl Into<String>, on: bool, action: Chosen) -> MenuNode {
    MenuNode::Item(MenuEntry { checked: Some(on), radio: true, ..entry(id, title, action) })
}

/// Something that is on or off.
fn switch(id: impl Into<String>, title: impl Into<String>, on: bool, action: Chosen) -> MenuNode {
    MenuNode::Item(MenuEntry { checked: Some(on), ..entry(id, title, action) })
}

fn menu(title: &str, items: Vec<MenuNode>) -> MenuNode {
    MenuNode::Menu { title: title.to_string(), enabled: true, items }
}

/// The menu within its menu that a command is put in, where the menu would be too long with
/// all of them in a row: the File menu's imports, and the Drawing menu's tools, rulers,
/// palettes, brushes and drawings. None for one that stays in the menu itself.
fn folder(menu: Menu, id: &str) -> Option<&'static str> {
    if menu == Menu::File {
        if id == "storyboard.exportPdf" {
            return Some("Export");
        }
        return matches!(id, "media.import" | "character.import" | "skin.import" | "rig.import" | "layer.refreshMedia" | "skin.fromFolder").then_some("Import");
    }
    if menu != Menu::Drawing {
        return None;
    }
    match id.split_once('.')?.0 {
        "tool" => Some("Tools"),
        "guide" => Some("Ruler"),
        "palette" => Some("Palette"),
        "brush" => Some("Brush"),
        "drawing" if !id.starts_with("drawing.make") => Some("Drawing"),
        "exposure" | "substitution" => Some("Drawing"),
        _ => None,
    }
}

impl MenuEntry {
    fn hinted(mut self, hint: impl Into<String>) -> MenuEntry {
        self.hint = Some(hint.into());
        self
    }
}

impl IskraApp {
    /// The app's menus as they are now: each menu's title and what is in it, in its order.
    pub fn menu_bar(&self) -> Vec<(String, Vec<MenuNode>)> {
        Menu::ALL.into_iter().map(|menu| (menu.title().to_string(), self.menu_nodes(menu))).collect()
    }

    /// A command's name in a menu, as it is now: *Undo* and *Redo* say what of.
    fn command_title(&self, command: &commands::AppCommand) -> String {
        let history = self.session.history();
        match command.id {
            "edit.undo" => history.undo_label().map_or("Undo".to_string(), |l| format!("Undo {l}")),
            "edit.redo" => history.redo_label().map_or("Redo".to_string(), |l| format!("Redo {l}")),
            _ => command.title().to_string(),
        }
    }

    /// One menu: its commands from the command list, a line between two groups of them, and
    /// then its settings.
    fn menu_nodes(&self, menu: Menu) -> Vec<MenuNode> {
        let mut nodes = Vec::new();
        for command in commands::commands().iter().filter(|command| command.menu == menu) {
            // In *Import*, *Import Rig…* is *Rig…*, and so in *Export*; the command keeps its
            // whole name elsewhere.
            let title = self.command_title(command);
            let title = match folder(menu, command.id) {
                Some(within @ ("Import" | "Export")) => title.strip_prefix(&format!("{within} ")).map_or_else(|| title.clone(), str::to_string),
                _ => title,
            };
            let item = MenuNode::Item(MenuEntry {
                enabled: (command.enabled)(self),
                checked: command.checked.map(|checked| checked(self)),
                shortcut: self.keys.chords(command.id).first().map(|chord| chord.shortcut()),
                ..entry(command.id, title, Chosen::Command(command.id))
            });
            // Into its menu within the menu, made where the first of its commands comes.
            let into = match folder(menu, command.id) {
                Some(title) => {
                    let at = nodes.iter().position(|node| matches!(node, MenuNode::Menu { title: had, .. } if had == title));
                    let at = at.unwrap_or_else(|| {
                        if command.group && !nodes.is_empty() {
                            nodes.push(MenuNode::Separator);
                        }
                        nodes.push(self::menu(title, Vec::new()));
                        nodes.len() - 1
                    });
                    let MenuNode::Menu { items, .. } = &mut nodes[at] else { unreachable!("found as a menu") };
                    items
                }
                None => &mut nodes,
            };
            if command.group && !into.is_empty() {
                into.push(MenuNode::Separator);
            }
            into.push(item);
            if command.id == "file.open" {
                nodes.push(self.recent_menu());
            }
        }
        // The commands scripts add to it, after its own.
        let scripts: Vec<MenuNode> = self
            .scripts
            .iter()
            .filter(|script| script.menu == menu)
            .map(|script| {
                let shortcut = self.keys.chords(script.id).first().map(|chord| chord.shortcut());
                MenuNode::Item(MenuEntry { shortcut, ..entry(script.id, script.title, Chosen::Script(script.id)) })
            })
            .collect();
        if !scripts.is_empty() && !nodes.is_empty() {
            nodes.push(MenuNode::Separator);
        }
        nodes.extend(scripts);
        nodes.extend(self.menu_settings(menu));
        nodes
    }

    /// *File ▸ Open Recent*: the project files last opened or saved, the newest first. A file
    /// that is gone is greyed, and those can be taken off the list.
    fn recent_menu(&self) -> MenuNode {
        let mut items = Vec::new();
        let there = |file: &PathBuf| self.listed.recent.iter().find(|(listed, _)| listed == file).is_none_or(|(_, there)| *there);
        for (index, file) in self.prefs.recent_files.iter().enumerate() {
            let name = file.file_stem().map_or_else(|| file.display().to_string(), |name| name.to_string_lossy().into_owned());
            let folder = file.parent().map_or(String::new(), |folder| folder.display().to_string());
            let found = there(file);
            let hint = if found { folder } else { format!("It is no longer in {folder}") };
            items.push(MenuNode::Item(MenuEntry { enabled: found, ..entry(format!("recent.{index}"), name, Chosen::OpenRecent(file.clone())).hinted(hint) }));
        }
        items.push(MenuNode::Separator);
        if self.prefs.recent_files.iter().any(|file| !there(file)) {
            items.push(MenuNode::Item(
                entry("recent.forget", "Take Off What Is Gone", Chosen::ForgetMissing).hinted("Takes the files that are no longer there off the list"),
            ));
        }
        items.push(MenuNode::Item(entry("recent.clear", "Clear", Chosen::ClearRecent)));
        MenuNode::Menu { title: "Open Recent".to_string(), enabled: !self.prefs.recent_files.is_empty(), items }
    }

    /// What a menu has besides commands: the settings that are picked from a list.
    fn menu_settings(&self, of: Menu) -> Vec<MenuNode> {
        let prefs = &self.prefs;
        match of {
            // Autosave is in the Settings window.
            Menu::File => Vec::new(),
            // The keymap is in the Settings window.
            Menu::Edit => Vec::new(),
            Menu::View => {
                let mut masks = vec![one_of("aspect.none", "None", prefs.aspect_mask.is_none(), Chosen::Aspect(None))];
                masks.extend(
                    prefs::ASPECTS
                        .iter()
                        .map(|(name, aspect)| one_of(format!("aspect.{name}"), *name, prefs.aspect_mask == Some(*aspect), Chosen::Aspect(Some(*aspect)))),
                );
                let mut nodes = Vec::new();
                // The displays the viewer can look through, where the shot has more than one:
                // the main one is what is rendered and the storyboard shows. Not on the
                // Storyboard page, which shows the main one alone, nor on the Export page, which
                // looks through passes. A Pass is no display to look through.
                let looks = |shot: &&iskra_model::Shot| shot.looks().next().is_some();
                if let Some(shot) = self.shot().filter(looks).filter(|_| !matches!(self.page, Page::Storyboard | Page::Export)) {
                    let viewing = self.viewport.looking_through(shot);
                    let hint = "What the Export page renders and the Storyboard page shows";
                    let main = one_of("display.main", format!("{} (Main)", shot.display_name), viewing.is_none(), Chosen::Display(None));
                    let mut items = vec![match main {
                        MenuNode::Item(entry) => MenuNode::Item(entry.hinted(hint)),
                        other => other,
                    }];
                    items.extend(shot.looks().map(|display| {
                        one_of(format!("display.{}", display.id.0), display.name.clone(), viewing == Some(display.id), Chosen::Display(Some(display.id)))
                    }));
                    nodes.push(menu("Display", items));
                }
                // In front of the mask; the Export page's player is cut to the frame always, as a
                // render is.
                if self.page != Page::Export {
                    nodes.push(match switch("view.clip", "Clip", prefs.clip, Chosen::Clip) {
                        MenuNode::Item(entry) => MenuNode::Item(entry.hinted("Show only what is in the camera's frame, as a render does")),
                        other => other,
                    });
                }
                nodes.push(menu("Aspect Mask", masks));
                if !prefs.user_guides.is_empty() {
                    let guides = prefs
                        .user_guides
                        .iter()
                        .enumerate()
                        .map(|(index, guide)| switch(format!("guide.{index}"), guide.name.clone(), guide.shown, Chosen::Guide(index)));
                    nodes.push(menu("Show My Guides", guides.collect()));
                }
                nodes
            }
            Menu::Workspace => {
                let pages = Page::ALL
                    .into_iter()
                    .map(|page| switch(format!("page.{}", page.title()), page.title(), !prefs.hidden_pages.contains(&page), Chosen::PageShown(page)));
                let mut nodes = vec![MenuNode::Separator, menu("Pages in the Bar", pages.collect())];
                // The layouts the user saved: one is taken, or deleted.
                for (name, layout) in &prefs.workspaces {
                    let on = layout == prefs.layouts.of(self.page);
                    let taken = MenuEntry { checked: Some(on), ..entry(format!("workspace.{name}"), name.clone(), Chosen::Workspace(name.clone())) };
                    nodes.push(MenuNode::Item(taken.hinted("Arrange the panels as saved")));
                }
                if !prefs.workspaces.is_empty() {
                    let gone = prefs
                        .workspaces
                        .keys()
                        .map(|name| MenuNode::Item(entry(format!("workspace.delete.{name}"), name.clone(), Chosen::DeleteWorkspace(name.clone()))));
                    nodes.push(menu("Delete Workspace", gone.collect()));
                }
                nodes
            }
            Menu::Drawing => {
                // The shot is shown in a style by its name: each palette in its style of that
                // name, and the others in their own colours; and the skin of that name.
                let Some(shot) = self.shot() else { return Vec::new() };
                let project = self.session.project();
                let palette = &project.palette;
                let skin = shot.skin.and_then(|skin| project.skin(skin)).map(|skin| skin.name.as_str());
                let shown: Vec<&str> = shot.styles.iter().filter_map(|id| palette.style(*id)).map(|style| style.name.as_str()).chain(skin).collect();
                let mut names: Vec<&str> = Vec::new();
                for name in palette.styles.iter().map(|style| style.name.as_str()).chain(project.skins().iter().map(|skin| skin.name.as_str())) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                if names.is_empty() {
                    return Vec::new();
                }
                let mut items = vec![one_of("style.own", "The Palettes' Own", shown.is_empty(), Chosen::Style(None))];
                items.extend(names.into_iter().map(|name| {
                    let on = !shown.is_empty() && shown.iter().all(|shown| *shown == name);
                    one_of(format!("style.{name}"), name, on, Chosen::Style(Some(name.to_string())))
                }));
                vec![MenuNode::Separator, menu("Show Shot In", items)]
            }
            Menu::Animation | Menu::Play | Menu::Storyboard | Menu::Scripts => Vec::new(),
        }
    }

    /// Run what a menu has by its id, as choosing it in the window's menus does: for a menu
    /// that is not the window's (the system's menu bar on macOS). What can't be chosen now,
    /// or is not known, does nothing.
    pub fn run_menu_command(&mut self, id: &str, ctx: &egui::Context) {
        fn find(nodes: &[MenuNode], id: &str) -> Option<MenuEntry> {
            nodes.iter().find_map(|node| match node {
                MenuNode::Item(entry) if entry.id == id => Some(entry.clone()),
                MenuNode::Menu { enabled: true, items, .. } => find(items, id),
                _ => None,
            })
        }
        let found = self.menu_bar().iter().find_map(|(_, nodes)| find(nodes, id));
        if let Some(entry) = found.filter(|entry| entry.enabled) {
            self.choose(entry.action, ctx);
        }
    }

    fn choose(&mut self, chosen: Chosen, ctx: &egui::Context) {
        match chosen {
            Chosen::Command(id) => {
                if let Some(command) = commands::command(id).filter(|command| (command.enabled)(self)) {
                    (command.run)(self, ctx);
                }
            }
            Chosen::Script(id) => self.run_script_command(id),
            Chosen::OpenRecent(file) => self.open_recent(&file),
            Chosen::ForgetMissing => self.prefs.recent_files.retain(|file| file.is_file()),
            Chosen::ClearRecent => self.prefs.recent_files.clear(),
            Chosen::Aspect(aspect) => self.prefs.aspect_mask = aspect,
            Chosen::Clip => self.prefs.clip = !self.prefs.clip,
            Chosen::Guide(index) => {
                if let Some(guide) = self.prefs.user_guides.get_mut(index) {
                    guide.shown = !guide.shown;
                }
            }
            Chosen::PageShown(page) => {
                let shown = !self.prefs.hidden_pages.contains(&page);
                self.prefs.hidden_pages.retain(|hidden| *hidden != page);
                if shown {
                    self.prefs.hidden_pages.push(page);
                }
            }
            Chosen::Workspace(name) => {
                if let Some(layout) = self.prefs.workspaces.get(&name).cloned() {
                    *self.prefs.layouts.of_mut(self.page) = layout;
                    self.view_only = false;
                    // Taken, it is what a start opens where workspace changes are not kept.
                    if !self.prefs.keep_layouts {
                        self.prefs.fixed_layouts = Some(self.prefs.layouts.clone());
                    }
                }
            }
            Chosen::DeleteWorkspace(name) => drop(self.prefs.workspaces.remove(&name)),
            Chosen::Style(name) => {
                if let Some(shot) = self.shot().map(|shot| shot.id) {
                    self.run("shot.setStyle", json!({ "shot": shot.0, "name": name }));
                }
            }
            Chosen::Display(display) => self.viewport.viewing = display,
        }
    }

    /// The window's own menus, drawn from the description: one button a menu.
    pub(crate) fn menus(&mut self, ui: &mut egui::Ui) {
        let mut chosen = None;
        for (title, nodes) in self.menu_bar() {
            ui.menu_button(title, |ui| chosen = chosen.take().or(Self::menu_ui(ui, &nodes)));
        }
        if let Some(chosen) = chosen {
            self.choose(chosen, ui.ctx());
        }
    }

    /// What is in a menu, drawn; and what of it was clicked.
    fn menu_ui(ui: &mut egui::Ui, nodes: &[MenuNode]) -> Option<Chosen> {
        let mut chosen = None;
        for node in nodes {
            match node {
                MenuNode::Separator => drop(ui.separator()),
                MenuNode::Note(said) => drop(ui.colored_label(ui.visuals().error_fg_color, said)),
                MenuNode::Menu { title, enabled, items } => {
                    ui.add_enabled_ui(*enabled, |ui| {
                        ui.menu_button(title, |ui| chosen = chosen.take().or(Self::menu_ui(ui, items)));
                    });
                }
                MenuNode::Item(entry) => {
                    let keys = entry.shortcut.map(|shortcut| ui.ctx().format_shortcut(&shortcut)).unwrap_or_default();
                    let on = entry.checked == Some(true);
                    // A command that switches something is lit when it is on; a setting has
                    // a round mark where it is one of several, a square one where it is alone.
                    let drawn = match (&entry.action, entry.checked) {
                        (Chosen::Command(_), _) | (_, None) => ui.add_enabled(entry.enabled, egui::Button::new(&entry.title).shortcut_text(keys).selected(on)),
                        (_, Some(_)) if entry.radio => ui.add_enabled(entry.enabled, egui::RadioButton::new(on, &entry.title)),
                        (_, Some(_)) => {
                            let mut ticked = on;
                            ui.add_enabled(entry.enabled, egui::Checkbox::new(&mut ticked, &entry.title))
                        }
                    };
                    let drawn = match &entry.hint {
                        Some(hint) if entry.enabled => drawn.on_hover_text(hint),
                        Some(hint) => drawn.on_disabled_hover_text(hint),
                        None => drawn,
                    };
                    if drawn.clicked() {
                        chosen = Some(entry.action.clone());
                    }
                }
            }
        }
        chosen
    }
}
