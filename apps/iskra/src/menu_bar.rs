//! The system's menu bar on macOS, at the top of the screen: Iskra's own, with the app's
//! menu first and then the menus the window has, made from the same list of commands.
//!
//! winit would give the app a menu of its own, but that one's Quit ends the process at once,
//! with no chance to ask about unsaved work, so it is switched off (see `main`). Without any
//! menu the bar shows that of whichever app was in front before, which is what this replaces.
//! Here Quit is the app's own command, which asks.
//!
//! The bar is made from the app's own description of its menus (`IskraApp::menu_bar`), the
//! one the window would draw its menus from: the commands, and the settings in menus within
//! the menus. So the window has no menus of its own here. The bar follows the app: after each
//! frame an entry is greyed where it can't be chosen, ticked where it is on, and named as it
//! is now, so that Undo says what it undoes; and where the menus have something more or less
//! in them, a recent file say, the bar is made anew.
//!
//! A command's shortcut is on its item when it has Cmd or Ctrl in it. The system then takes
//! the key before the window sees it and the item runs the command. A shortcut without
//! either, a tool's letter or Space, stays the window's alone: the system would take it from
//! a key that is held, and it is not shown in the bar. While a text is being typed the bar
//! has no shortcuts at all but Quit's, so that Undo and Paste are the text's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui;
use iskra_ui::{MenuEntry, MenuNode};
use muda::accelerator::{CMD_OR_CTRL, Key, KeyAccelerator, Modifiers, NamedKey};
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

/// The app's menus as it describes them: each menu's title, and what is in it.
pub type Menus = [(String, Vec<MenuNode>)];

/// What was chosen in the bar and is not run yet, by its id.
pub type Chosen = Arc<Mutex<Vec<String>>>;

/// An entry's item: a plain one, or one with a tick for what is on or off.
enum Item {
    Plain(MenuItem),
    Ticked(CheckMenuItem),
}

/// The menu bar, kept while the window lives.
pub struct MenuBar {
    _menu: Menu,
    chosen: Chosen,
    items: HashMap<String, Item>,
    /// What the bar is made of: its menus and what is in each, without how each is now. When
    /// the app's menus are made of something else, a file more under Open Recent say, the
    /// bar is made anew.
    made_of: Vec<String>,
    /// How each entry was last shown, and whether a text was being typed then.
    shown: HashMap<String, MenuEntry>,
    typing: bool,
}

/// Everything that can be chosen in `nodes`, in the menus within them too.
fn entries(nodes: &[MenuNode]) -> Vec<&MenuEntry> {
    let mut all = Vec::new();
    for node in nodes {
        match node {
            MenuNode::Item(entry) => all.push(entry),
            MenuNode::Menu { items, .. } => all.extend(entries(items)),
            MenuNode::Separator | MenuNode::Note(_) => {}
        }
    }
    all
}

/// The entry of the app's menus that has `id`.
pub fn entry<'a>(menus: &'a Menus, id: &str) -> Option<&'a MenuEntry> {
    menus.iter().flat_map(|(_, nodes)| entries(nodes)).find(|entry| entry.id == id)
}

/// What a bar of `menus` is made of (see [`MenuBar::made_of`]).
fn made_of(menus: &Menus) -> Vec<String> {
    fn of(nodes: &[MenuNode], into: &mut Vec<String>) {
        for node in nodes {
            match node {
                MenuNode::Separator => into.push("line".to_string()),
                MenuNode::Note(said) => into.push(format!("note {said}")),
                MenuNode::Item(entry) => into.push(format!("entry {} {}", entry.checked.is_some(), entry.id)),
                MenuNode::Menu { title, enabled, items } => {
                    into.push(format!("menu {enabled} {title}"));
                    of(items, into);
                    into.push("end".to_string());
                }
            }
        }
    }
    let mut made = Vec::new();
    for (title, nodes) in menus {
        made.push(format!("menu true {title}"));
        of(nodes, &mut made);
        made.push("end".to_string());
    }
    made
}

/// A title as the bar writes it: an `&` in it is no mark of a letter to press.
fn written(title: &str) -> String {
    title.replace('&', "&&")
}

/// A shortcut as the system takes it, if it is one the bar may have: with Cmd or Ctrl, and of
/// a key the system has a name for.
fn accelerator(shortcut: &egui::KeyboardShortcut) -> Option<KeyAccelerator> {
    let held = shortcut.modifiers;
    if !(held.command || held.mac_cmd || held.ctrl) {
        return None;
    }
    let mut modifiers = Modifiers::empty();
    for (has, modifier) in
        [(held.command || held.mac_cmd, CMD_OR_CTRL), (held.ctrl, Modifiers::CONTROL), (held.alt, Modifiers::ALT), (held.shift, Modifiers::SHIFT)]
    {
        if has {
            modifiers |= modifier;
        }
    }
    use egui::Key as K;
    let named = match shortcut.logical_key {
        K::Enter => Some(NamedKey::Enter),
        K::Escape => Some(NamedKey::Escape),
        K::Tab => Some(NamedKey::Tab),
        K::Backspace => Some(NamedKey::Backspace),
        K::Delete => Some(NamedKey::Delete),
        K::ArrowLeft => Some(NamedKey::ArrowLeft),
        K::ArrowRight => Some(NamedKey::ArrowRight),
        K::ArrowUp => Some(NamedKey::ArrowUp),
        K::ArrowDown => Some(NamedKey::ArrowDown),
        K::Home => Some(NamedKey::Home),
        K::End => Some(NamedKey::End),
        K::PageUp => Some(NamedKey::PageUp),
        K::PageDown => Some(NamedKey::PageDown),
        _ => None,
    };
    let key = match (named, shortcut.logical_key) {
        (Some(named), _) => Key::Named(named),
        (None, K::Space) => Key::Character(" ".to_string()),
        // A letter, a digit or a mark: the one character the key is written as.
        (None, key) => {
            let written = key.symbol_or_name();
            let mut characters = written.chars();
            match (characters.next(), characters.next()) {
                (Some(one), None) if one.is_ascii_graphic() => Key::Character(one.to_ascii_lowercase().to_string()),
                _ => return None,
            }
        }
    };
    Some(KeyAccelerator::new(modifiers, key))
}

/// Put what `nodes` describe into a menu of the bar. About, Settings and Quit are left out:
/// they are in the app's menu, where the system has them. A line is not put first, last or after another.
fn fill(into: &Submenu, nodes: &[MenuNode], items: &mut HashMap<String, Item>) -> Result<(), muda::Error> {
    let kept: Vec<&MenuNode> = nodes
        .iter()
        .filter(|node| !matches!(node, MenuNode::Item(entry) if [iskra_ui::QUIT, iskra_ui::ABOUT, iskra_ui::SETTINGS].contains(&entry.id.as_str())))
        .collect();
    let mut line_wanted = false;
    let mut any = false;
    for node in kept {
        if *node == MenuNode::Separator {
            line_wanted = any;
            continue;
        }
        if std::mem::take(&mut line_wanted) {
            into.append(&PredefinedMenuItem::separator())?;
        }
        any = true;
        match node {
            MenuNode::Separator => {}
            MenuNode::Note(said) => into.append(&MenuItem::new(written(said), false, None))?,
            MenuNode::Item(entry) if entry.checked.is_some() => {
                let item = CheckMenuItem::with_id(entry.id.as_str(), written(&entry.title), true, false, None);
                into.append(&item)?;
                items.insert(entry.id.clone(), Item::Ticked(item));
            }
            MenuNode::Item(entry) => {
                let item = MenuItem::with_id(entry.id.as_str(), written(&entry.title), true, None);
                into.append(&item)?;
                items.insert(entry.id.clone(), Item::Plain(item));
            }
            MenuNode::Menu { title, enabled, items: within } => {
                let menu = Submenu::new(written(title), *enabled);
                fill(&menu, within, items)?;
                into.append(&menu)?;
            }
        }
    }
    Ok(())
}

/// The bar of `menus`, given to the system: the app's menu, the app's menus, and Window.
fn build(menus: &Menus) -> Result<(Menu, HashMap<String, Item>), muda::Error> {
    let menu = Menu::new();
    let mut items = HashMap::new();
    // The first menu is the app's, whatever it is called here: the system names it.
    let app = Submenu::new("Iskra", true);
    let quit = MenuItem::with_id(iskra_ui::QUIT, "Quit Iskra", true, None);
    let about = MenuItem::with_id(iskra_ui::ABOUT, "About Iskra", true, None);
    let settings = MenuItem::with_id(iskra_ui::SETTINGS, "Settings…", true, None);
    app.append_items(&[
        &about,
        &PredefinedMenuItem::separator(),
        &settings,
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(Some("Hide Iskra")),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::show_all(None),
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;
    items.insert(iskra_ui::QUIT.to_string(), Item::Plain(quit));
    items.insert(iskra_ui::ABOUT.to_string(), Item::Plain(about));
    items.insert(iskra_ui::SETTINGS.to_string(), Item::Plain(settings));
    menu.append(&app)?;
    for (title, nodes) in menus {
        let listed = Submenu::new(written(title), true);
        fill(&listed, nodes, &mut items)?;
        menu.append(&listed)?;
    }
    let window = Submenu::new("Window", true);
    window.append_items(&[&PredefinedMenuItem::minimize(None), &PredefinedMenuItem::maximize(Some("Zoom"))])?;
    menu.append(&window)?;
    menu.init_for_nsapp();
    window.set_as_windows_menu_for_nsapp();
    Ok((menu, items))
}

/// Make the bar, as `menus` has the app's menus now, and give it to the system. It is to be
/// called on the main thread, once the app is there: where the window's app is made.
pub fn install(ctx: &egui::Context, menus: &Menus) -> Result<MenuBar, muda::Error> {
    let (menu, items) = build(menus)?;
    // What is chosen is run by the window on its next frame, which is asked for.
    let chosen = Chosen::default();
    let (into, ctx) = (chosen.clone(), ctx.clone());
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Ok(mut chosen) = into.lock() {
            chosen.push(event.id.0);
        }
        ctx.request_repaint();
    }));
    let mut bar = MenuBar { _menu: menu, chosen, items, made_of: made_of(menus), shown: HashMap::new(), typing: false };
    bar.follow(menus, false)?;
    Ok(bar)
}

impl MenuBar {
    /// What was chosen in the bar since this was last asked. The system ticks an item that
    /// is chosen by itself, so those are shown anew on the next frame, as the app has them.
    pub fn take_chosen(&mut self) -> Vec<String> {
        let chosen = self.chosen.lock().map(|mut chosen| std::mem::take(&mut *chosen)).unwrap_or_default();
        for id in &chosen {
            self.shown.remove(id);
        }
        chosen
    }

    /// Show the app's menus as `menus` has them now. Where they are made of what the bar is
    /// made of, only what has changed since is told to the system; else the bar is made
    /// anew. While a text is `typing`, nothing but Quit has its shortcut. Says why, if the
    /// system would not take a bar that was made anew: the one before then stays.
    pub fn follow(&mut self, menus: &Menus, typing: bool) -> Result<(), muda::Error> {
        let made = made_of(menus);
        if made != self.made_of {
            let (menu, items) = build(menus)?;
            (self._menu, self.items, self.made_of) = (menu, items, made);
            self.shown.clear();
        }
        let all = typing != self.typing;
        self.typing = typing;
        for entry in menus.iter().flat_map(|(_, nodes)| entries(nodes)) {
            if !all && self.shown.get(&entry.id) == Some(entry) {
                continue;
            }
            let Some(item) = self.items.get(&entry.id) else { continue };
            let quit = entry.id == iskra_ui::QUIT;
            let title = if quit { "Quit Iskra".to_string() } else { written(&entry.title) };
            // Quit keeps its shortcut while a text is typed, and has Cmd+Q if the keymap has none.
            let keys = entry.shortcut.as_ref().filter(|_| quit || !typing).and_then(accelerator);
            let keys = keys.or_else(|| quit.then(|| KeyAccelerator::new(CMD_OR_CTRL, Key::Character("q".to_string()))));
            // A shortcut the system will not take is left off: the window still has it.
            match item {
                Item::Plain(item) => {
                    item.set_text(title);
                    item.set_enabled(entry.enabled);
                    let _ = item.set_key_accelerator(keys);
                }
                Item::Ticked(item) => {
                    item.set_text(title);
                    item.set_enabled(entry.enabled);
                    item.set_checked(entry.checked == Some(true));
                    let _ = item.set_key_accelerator(keys);
                }
            }
            self.shown.insert(entry.id.clone(), entry.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_goes_to_the_bar_only_with_cmd_or_ctrl() {
        let shortcut = |command: bool, shift: bool, key| egui::KeyboardShortcut::new(egui::Modifiers { command, shift, ..egui::Modifiers::NONE }, key);
        let save = accelerator(&shortcut(true, false, egui::Key::S)).expect("Cmd+S");
        assert_eq!(save, KeyAccelerator::new(CMD_OR_CTRL, Key::Character("s".to_string())));
        let redo = accelerator(&shortcut(true, true, egui::Key::Z)).expect("Cmd+Shift+Z");
        assert_eq!(redo, KeyAccelerator::new(CMD_OR_CTRL | Modifiers::SHIFT, Key::Character("z".to_string())));
        assert_eq!(accelerator(&shortcut(true, false, egui::Key::Enter)), Some(KeyAccelerator::new(CMD_OR_CTRL, Key::Named(NamedKey::Enter))));
        // A tool's letter, Space, or a key with Shift alone stays the window's.
        assert_eq!(accelerator(&shortcut(false, false, egui::Key::B)), None);
        assert_eq!(accelerator(&shortcut(false, false, egui::Key::Space)), None);
        assert_eq!(accelerator(&shortcut(false, true, egui::Key::F)), None);
        assert_eq!(written("Cut & Paste"), "Cut && Paste");
    }

    #[test]
    fn the_bar_is_made_anew_only_when_the_menus_are_made_of_something_else() {
        let app = iskra_ui::IskraApp::new(iskra_engine::Session::new());
        let menus = app.menu_bar();
        let titles: Vec<&str> = menus.iter().map(|(title, _)| title.as_str()).collect();
        assert!(titles.contains(&"File") && titles.contains(&"Workspace"));
        assert!(entry(&menus, iskra_ui::QUIT).is_some() && entry(&menus, "page.Media").is_some() && entry(&menus, "no.such").is_none());
        // How an entry is now is not what the bar is made of: its name, whether it can be
        // chosen, whether it is on.
        let mut other = menus.clone();
        let first = other.iter_mut().flat_map(|(_, nodes)| nodes.iter_mut()).find_map(|node| match node {
            MenuNode::Item(entry) => Some(entry),
            _ => None,
        });
        let first = first.expect("a command");
        (first.title, first.enabled) = ("Another Name".to_string(), !first.enabled);
        assert_eq!(made_of(&menus), made_of(&other));
        // Something more in a menu is.
        other[0].1.push(MenuNode::Note("more".to_string()));
        assert_ne!(made_of(&menus), made_of(&other));
    }
}
