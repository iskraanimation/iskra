//! Keymaps (plan §7.6): which keys run which commands. A keymap is data, command id to
//! shortcuts, so shortcuts are not written into the code. There are three presets, after the
//! key layouts of Harmony (the default), Animate and TVPaint, and a keymap can also be a JSON
//! file in the `keymaps` folder beside the preferences, which is what a preset becomes once
//! its keys are changed in the Settings window. A command's keys may be other on one page.
//!
//! Only key layouts are followed. Commands with no equivalent in a layout stay unassigned.

use std::collections::BTreeMap;
use std::path::Path;

use egui::{Key, KeyboardShortcut, Modifiers};
use serde::{Deserialize, Serialize};

use crate::dock::Page;

/// A key with its modifiers, written `Cmd+Shift+Z`. `Cmd` is Command on macOS and Ctrl elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Chord {
    /// Bit 1 is Cmd, 2 Ctrl, 4 Alt, 8 Shift.
    modifiers: u8,
    pub key: Key,
}

const MODIFIERS: [(u8, &str); 4] = [(1, "Cmd"), (2, "Ctrl"), (4, "Alt"), (8, "Shift")];

impl Chord {
    /// Read `Alt+B`, `Cmd+Shift+Z`, `Period`. Keys are named as egui names them.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let key = Key::from_name(parts.pop()?)?;
        let mut modifiers = 0;
        for part in parts {
            modifiers |= MODIFIERS.iter().find(|(_, name)| name.eq_ignore_ascii_case(part))?.0;
        }
        Some(Self { modifiers, key })
    }

    pub fn shortcut(self) -> KeyboardShortcut {
        let has = |bit| self.modifiers & bit != 0;
        let modifiers = Modifiers { command: has(1), ctrl: has(2), alt: has(4), shift: has(8), mac_cmd: false };
        KeyboardShortcut::new(modifiers, self.key)
    }

    /// Take this chord's key press out of `input`, if it is there. The modifiers have to be
    /// exactly the chord's, so `Alt+E` doesn't also run what is on `E`. Punctuation is the
    /// exception: which modifiers type a `+` depends on the keyboard, so extra Shift or Alt is
    /// let through there.
    pub fn consume(self, input: &mut egui::InputState) -> bool {
        let wanted = self.shortcut().modifiers;
        let punctuation = self.key.symbol_or_name() != self.key.name();
        let mut found = false;
        input.events.retain(|event| {
            let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { return true };
            let held = if punctuation { modifiers.matches_logically(wanted) } else { modifiers.matches_exact(wanted) };
            let hit = !found && *key == self.key && held;
            found |= hit;
            !hit
        });
        found
    }

    /// The key alone, or with Shift only: what typing into a text field would also press.
    /// The chord a key press is: the key with the modifiers held. Ctrl is Cmd where Cmd is
    /// Ctrl, off macOS, as a chord written `Cmd` is pressed there.
    pub fn pressed(key: Key, held: Modifiers) -> Self {
        let ctrl = held.ctrl && !(held.command && !held.mac_cmd);
        let modifiers = [held.command, ctrl, held.alt, held.shift].iter().zip(MODIFIERS).filter(|(on, _)| **on).fold(0, |bits, (_, (bit, _))| bits | bit);
        Self { modifiers, key }
    }

    pub fn is_typing(self) -> bool {
        self.modifiers & !8 == 0
    }

    fn modifier_count(self) -> u32 {
        self.modifiers.count_ones()
    }
}

impl From<Chord> for String {
    fn from(chord: Chord) -> String {
        let held = MODIFIERS.iter().filter(|(bit, _)| chord.modifiers & bit != 0).map(|(_, name)| *name);
        held.chain([chord.key.name()]).collect::<Vec<_>>().join("+")
    }
}

impl TryFrom<String> for Chord {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        Chord::parse(&text).ok_or_else(|| format!("`{text}` is not a shortcut"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Harmony,
    Animate,
    TvPaint,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Harmony, Preset::Animate, Preset::TvPaint];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Harmony => "Harmony",
            Preset::Animate => "Animate",
            Preset::TvPaint => "TVPaint",
        }
    }

    /// The preset's own keys, on top of [`COMMON`].
    fn keys(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Preset::Harmony => &[
                ("tool.brush", "Alt+B"),
                ("tool.eraser", "Alt+E"),
                ("tool.fill", "Alt+I"),
                ("tool.line", "Alt+Backslash"),
                ("tool.rectangle", "Alt+7"),
                ("tool.ellipse", "Alt+Equals"),
                ("tool.select", "Alt+S"),
                ("tool.cutter", "Alt+T"),
                ("tool.transform", "Shift+T"),
                ("tool.bones", "Alt+J"),
                ("tool.points", "Alt+Q"),
                ("tool.text", "Alt+Y"),
                ("tool.zoom", "Alt+Z"),
                ("art.lineArt", "L"),
                ("art.colourArt", "K"),
                ("peg.addParent", "Cmd+P"),
                ("substitution.previous", "OpenBracket"),
                ("substitution.next", "CloseBracket"),
                ("key.add", "F6"),
                ("key.remove", "F7"),
                ("key.previous", "Semicolon"),
                ("key.next", "Quote"),
                ("key.switchHeld", "Cmd+L"),
                ("node.switchOn", "A"),
                ("node.switchOff", "D"),
                ("view.onionSkin", "Alt+O"),
                ("view.lightTable", "Shift+L"),
                ("view.zoomOut", "1"),
                ("view.zoomIn", "2"),
                ("view.resetZoom", "Shift+Z"),
                ("view.reset", "Shift+M"),
                ("view.rotateClockwise", "V"),
                ("view.rotateCounterclockwise", "C"),
                ("view.resetRotation", "Shift+X"),
                ("playback.toggle", "Shift+Enter"),
                ("frame.next", "Period"),
                ("frame.previous", "Comma"),
                ("drawing.newDrawing", "Alt+Shift+R"),
                ("drawing.duplicate", "Alt+Shift+D"),
                ("exposure.insert", "Plus"),
                ("exposure.remove", "Minus"),
                ("exposure.extend", "Alt+Shift+E"),
            ],
            Preset::Animate => &[
                ("tool.brush", "B"),
                ("tool.eraser", "E"),
                ("tool.fill", "K"),
                ("tool.eyedropper", "I"),
                ("tool.line", "N"),
                ("tool.rectangle", "R"),
                ("tool.ellipse", "O"),
                ("tool.select", "V"),
                ("tool.lasso", "L"),
                ("tool.transform", "Q"),
                ("tool.bones", "M"),
                ("tool.points", "A"),
                ("tool.text", "T"),
                ("tool.zoom", "Z"),
                ("view.onionSkin", "Alt+Shift+O"),
                ("playback.toggle", "Enter"),
                ("frame.next", "Period"),
                ("frame.previous", "Comma"),
                ("drawing.newDrawing", "F7"),
                ("drawing.duplicate", "F6"),
                ("exposure.insert", "F5"),
                ("exposure.remove", "Shift+F5"),
                ("exposure.extend", "Alt+Shift+E"),
                ("edit.redo", "Cmd+Y"),
            ],
            // A first draft: only the keys we are sure of. The rest stay unassigned until
            // someone who works in TVPaint fills them in.
            Preset::TvPaint => &[("frame.next", "ArrowRight"), ("frame.previous", "ArrowLeft")],
        }
    }
}

/// What every preset shares: the system's own conventions.
const COMMON: &[(&str, &str)] = &[
    ("file.new", "Cmd+N"),
    ("file.open", "Cmd+O"),
    ("file.save", "Cmd+S"),
    ("app.settings", "Cmd+Comma"),
    ("file.saveAs", "Cmd+Shift+S"),
    ("app.quit", "Cmd+Q"),
    ("app.commandPalette", "Cmd+Shift+P"),
    ("edit.undo", "Cmd+Z"),
    ("edit.redo", "Cmd+Shift+Z"),
    ("edit.selectAll", "Cmd+A"),
    ("edit.duplicate", "Cmd+D"),
    ("edit.deselect", "Escape"),
    ("edit.delete", "Delete"),
    ("edit.delete", "Backspace"),
    ("workspace.viewOnly", "Tab"),
    ("shot.new", "Cmd+Shift+N"),
    ("shot.duplicate", "Cmd+Shift+D"),
    ("shot.split", "Cmd+Shift+B"),
    ("marker.add", "Cmd+Shift+M"),
    ("marker.next", "Shift+ArrowDown"),
    ("marker.previous", "Shift+ArrowUp"),
    ("range.in", "Cmd+Shift+I"),
    ("range.out", "Cmd+Shift+O"),
    ("text.convertToShapes", "Cmd+Shift+T"),
    ("timeline.showSelected", "F"),
];

/// Command id to the shortcuts that run it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keymap {
    /// The keys on every page. A file of before 2026-10-17 is this map alone.
    #[serde(flatten)]
    bindings: BTreeMap<String, Vec<Chord>>,
    /// Where a page's keys are other than every page's: the command's keys there, none for a
    /// command that has no key on that page.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pages: BTreeMap<Page, BTreeMap<String, Vec<Chord>>>,
    /// The preset a keymap of the user's own was made from, whose keys Reset puts back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preset: Option<String>,
}

impl Keymap {
    pub fn preset(preset: Preset) -> Self {
        let mut keymap = Self::default();
        for (id, chord) in COMMON.iter().chain(preset.keys()) {
            // The presets are checked by a test, so every chord here parses.
            keymap.bindings.entry(id.to_string()).or_default().extend(Chord::parse(chord));
        }
        keymap
    }

    /// The keymap called `name`: a preset, or the file `<name>.json` in `folder`. A name that
    /// is neither gives the default preset.
    pub fn named(name: &str, folder: Option<&Path>) -> Self {
        if let Some(preset) = Preset::ALL.into_iter().find(|p| p.name() == name) {
            return Self::preset(preset);
        }
        let file = folder.map(|folder| folder.join(format!("{name}.json")));
        let read: Option<Self> = file.and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| serde_json::from_str(&text).ok());
        let Some(read) = read else { return Self::preset(Preset::Harmony) };
        // The layer commands were `element.…` until 2026-10-09; a file from before still works.
        let renamed = |id: String| id.strip_prefix("element.").map(|rest| format!("layer.{rest}")).unwrap_or(id);
        Self { bindings: read.bindings.into_iter().map(|(id, chords)| (renamed(id), chords)).collect(), ..read }
    }

    /// The preset this keymap is, or was made from.
    pub fn based_on(&self, name: &str) -> Preset {
        let base = self.preset.as_deref().unwrap_or(name);
        Preset::ALL.into_iter().find(|preset| preset.name() == base).unwrap_or(Preset::Harmony)
    }

    /// The preset's keys again, still said to be made from it.
    pub fn reset(&mut self, preset: Preset) {
        *self = Self { preset: Some(preset.name().to_string()), ..Self::preset(preset) };
    }

    /// Say which preset this was made from, if nothing says so yet.
    pub fn made_from(&mut self, preset: Preset) {
        self.preset.get_or_insert_with(|| preset.name().to_string());
    }

    /// The keys as they are on `page`: every page's, but where the page has its own.
    pub fn on(&self, page: Page) -> Keymap {
        let mut bindings = self.bindings.clone();
        for (id, chords) in self.pages.get(&page).into_iter().flatten() {
            bindings.insert(id.clone(), chords.clone());
        }
        Keymap { bindings, pages: BTreeMap::new(), preset: None }
    }

    /// Whether `page` has keys of its own for a command.
    pub fn has_own(&self, page: Page, id: &str) -> bool {
        self.pages.get(&page).is_some_and(|keys| keys.contains_key(id))
    }

    /// Give a command `chord` as its one key, on every page or on one. The key is taken from
    /// the commands that had it there; their ids are returned.
    pub fn bind(&mut self, id: &str, chord: Chord, page: Option<Page>) -> Vec<String> {
        let mut taken = Vec::new();
        match page {
            None => {
                let keys = std::iter::once(&mut self.bindings).chain(self.pages.values_mut());
                for keys in keys {
                    for (other, chords) in keys.iter_mut().filter(|(other, _)| other.as_str() != id) {
                        if chords.contains(&chord) {
                            chords.retain(|c| *c != chord);
                            taken.push(other.clone());
                        }
                    }
                }
                self.bindings.insert(id.to_string(), vec![chord]);
                for keys in self.pages.values_mut() {
                    keys.remove(id);
                }
            }
            Some(page) => {
                let on = self.on(page);
                for (other, chords) in on.bindings.iter().filter(|(other, chords)| other.as_str() != id && chords.contains(&chord)) {
                    let left = chords.iter().copied().filter(|c| *c != chord).collect();
                    self.pages.entry(page).or_default().insert(other.clone(), left);
                    taken.push(other.clone());
                }
                self.pages.entry(page).or_default().insert(id.to_string(), vec![chord]);
            }
        }
        self.tidy();
        taken.sort();
        taken.dedup();
        taken
    }

    /// Take a command's keys away, on every page or on one.
    pub fn unbind(&mut self, id: &str, page: Option<Page>) {
        match page {
            None => {
                self.bindings.insert(id.to_string(), Vec::new());
                for keys in self.pages.values_mut() {
                    keys.remove(id);
                }
            }
            Some(page) => {
                self.pages.entry(page).or_default().insert(id.to_string(), Vec::new());
            }
        }
        self.tidy();
    }

    /// A command on `page` has every page's keys again.
    pub fn follow_all(&mut self, id: &str, page: Page) {
        if let Some(keys) = self.pages.get_mut(&page) {
            keys.remove(id);
        }
        self.tidy();
    }

    /// A page's keys that are every page's anyway are not kept as its own.
    fn tidy(&mut self) {
        let all = &self.bindings;
        for keys in self.pages.values_mut() {
            keys.retain(|id, chords| all.get(id).map_or(!chords.is_empty(), |every| every != chords));
        }
        self.pages.retain(|_, keys| !keys.is_empty());
    }

    /// Write the keymap as the file `<name>.json` in `folder`, to edit by hand.
    pub fn save(&self, name: &str, folder: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        std::fs::write(folder.join(format!("{name}.json")), serde_json::to_string_pretty(self)?)
    }

    /// The names of the keymap files in `folder`.
    pub fn files(folder: &Path) -> Vec<String> {
        let entries = std::fs::read_dir(folder).into_iter().flatten().flatten();
        let mut names: Vec<String> = entries.filter_map(|e| Some(e.file_name().to_string_lossy().strip_suffix(".json")?.to_string())).collect();
        names.sort();
        names
    }

    /// The shortcuts of a command; the first is the one menus show.
    pub fn chords(&self, id: &str) -> &[Chord] {
        self.bindings.get(id).map_or(&[], Vec::as_slice)
    }

    /// Every shortcut with its command, those with more modifiers first: `Shift+Enter` has to
    /// be tried before `Enter`, which a key press with Shift held would also match.
    pub fn in_order(&self) -> Vec<(Chord, &str)> {
        let mut all: Vec<(Chord, &str)> = self.bindings.iter().flat_map(|(id, chords)| chords.iter().map(|c| (*c, id.as_str()))).collect();
        all.sort_by_key(|(chord, _)| std::cmp::Reverse(chord.modifier_count()));
        all
    }

    /// The shortcuts that more than one command claims, with those commands.
    pub fn conflicts(&self) -> Vec<(Chord, Vec<&str>)> {
        let mut by_chord: BTreeMap<Chord, Vec<&str>> = BTreeMap::new();
        for (chord, id) in self.in_order() {
            by_chord.entry(chord).or_default().push(id);
        }
        by_chord.into_iter().filter(|(_, ids)| ids.len() > 1).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_are_written_as_people_write_them() {
        let chord = Chord::parse("Cmd+Shift+Z").unwrap();
        assert_eq!(chord.shortcut(), KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z));
        assert_eq!(String::from(chord), "Cmd+Shift+Z");
        assert_eq!(String::from(Chord::parse("shift + alt + r").unwrap()), "Alt+Shift+R", "any order and case, written one way");
        assert_eq!(Chord::parse("Period").map(|c| c.key), Some(Key::Period));
        assert!(Chord::parse("B").unwrap().is_typing() && Chord::parse("Shift+L").unwrap().is_typing() && !Chord::parse("Alt+B").unwrap().is_typing());
        for bad in ["", "Cmd+", "Hyper+B", "Cmd+NoSuchKey"] {
            assert_eq!(Chord::parse(bad), None, "{bad}");
        }
        assert_eq!(serde_json::to_string(&chord).unwrap(), "\"Cmd+Shift+Z\"");
        assert!(serde_json::from_str::<Chord>("\"Cmd+Nope\"").is_err());
    }

    #[test]
    fn every_preset_parses_and_no_shortcut_runs_two_commands() {
        for preset in Preset::ALL {
            for (id, chord) in COMMON.iter().chain(preset.keys()) {
                assert!(Chord::parse(chord).is_some(), "{}: `{chord}` for {id}", preset.name());
            }
            assert_eq!(Keymap::preset(preset).conflicts(), [], "{}", preset.name());
        }
        let harmony = Keymap::preset(Preset::Harmony);
        assert_eq!(harmony.chords("tool.brush"), [Chord::parse("Alt+B").unwrap()]);
        assert_eq!(harmony.chords("edit.delete").len(), 2, "a command can have several shortcuts");
        assert!(harmony.chords("no.such").is_empty());
        let order: Vec<&str> = harmony.in_order().into_iter().filter(|(c, _)| c.key == Key::Z).map(|(_, id)| id).collect();
        assert_eq!(order, ["edit.redo", "edit.undo", "tool.zoom", "view.resetZoom"], "more modifiers first");
    }

    #[test]
    fn a_keymap_can_be_a_file_and_conflicts_in_it_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("keymaps");
        assert!(Keymap::files(&folder).is_empty());
        Keymap::preset(Preset::Animate).save("Mine", &folder).unwrap();
        assert_eq!(Keymap::files(&folder), ["Mine"]);
        assert_eq!(Keymap::named("Mine", Some(&folder)), Keymap::preset(Preset::Animate));
        assert_eq!(Keymap::named("Animate", None), Keymap::preset(Preset::Animate));
        assert_eq!(Keymap::named("Gone", Some(&folder)), Keymap::preset(Preset::Harmony), "an unknown name is the default");

        std::fs::write(folder.join("Clash.json"), r#"{ "tool.brush": ["B"], "tool.eraser": ["E", "B"] }"#).unwrap();
        let clash = Keymap::named("Clash", Some(&folder));
        assert_eq!(clash.conflicts(), [(Chord::parse("B").unwrap(), vec!["tool.brush", "tool.eraser"])]);
    }

    #[test]
    fn a_key_is_bound_on_every_page_or_on_one_and_taken_from_who_had_it() {
        let key = |text| Chord::parse(text).unwrap();
        let mut keymap = Keymap::preset(Preset::Harmony);
        // Every page: one key, taken from the command that had it.
        assert_eq!(keymap.bind("tool.eraser", key("Alt+B"), None), ["tool.brush"]);
        assert_eq!((keymap.chords("tool.eraser"), keymap.chords("tool.brush")), (&[key("Alt+B")][..], &[][..]));
        assert_eq!(keymap.bind("edit.delete", key("Delete"), None), Vec::<String>::new());
        assert_eq!(keymap.chords("edit.delete"), [key("Delete")], "one key to a command");
        // One page: there alone, and taken from who has it there.
        assert_eq!(keymap.bind("tool.brush", key("Alt+B"), Some(Page::Storyboard)), ["tool.eraser"]);
        let (board, animation) = (keymap.on(Page::Storyboard), keymap.on(Page::Animation));
        assert_eq!((board.chords("tool.brush"), board.chords("tool.eraser")), (&[key("Alt+B")][..], &[][..]));
        assert_eq!((animation.chords("tool.brush"), animation.chords("tool.eraser")), (&[][..], &[key("Alt+B")][..]));
        assert!(keymap.has_own(Page::Storyboard, "tool.brush") && !keymap.has_own(Page::Animation, "tool.brush"));
        assert_eq!(board.conflicts(), []);
        // Kept in the file, and read back.
        let dir = tempfile::tempdir().unwrap();
        keymap.made_from(Preset::Harmony);
        keymap.save("Mine", dir.path()).unwrap();
        assert_eq!(Keymap::named("Mine", Some(dir.path())), keymap);
        // The page follows every page again; a key bound on every page is so on each.
        keymap.follow_all("tool.brush", Page::Storyboard);
        assert_eq!(keymap.on(Page::Storyboard).chords("tool.brush"), []);
        keymap.unbind("tool.eraser", Some(Page::Media));
        assert_eq!(keymap.on(Page::Media).chords("tool.eraser"), []);
        keymap.bind("tool.eraser", key("E"), None);
        assert!(!keymap.has_own(Page::Media, "tool.eraser") && !keymap.has_own(Page::Storyboard, "tool.eraser"));
        // Reset is the preset's keys.
        keymap.reset(keymap.based_on("Mine"));
        assert_eq!(keymap.on(Page::Animation), Keymap::preset(Preset::Harmony).on(Page::Animation));
    }

    #[test]
    fn a_press_is_the_chord_it_is_written_as() {
        let mac_cmd = Modifiers { command: true, mac_cmd: true, ..Modifiers::NONE };
        let pc_ctrl = Modifiers { command: true, ctrl: true, ..Modifiers::NONE };
        assert_eq!(String::from(Chord::pressed(Key::D, mac_cmd)), "Cmd+D");
        assert_eq!(String::from(Chord::pressed(Key::D, pc_ctrl)), "Cmd+D");
        assert_eq!(String::from(Chord::pressed(Key::D, Modifiers { ctrl: true, ..Modifiers::NONE })), "Ctrl+D");
        assert_eq!(String::from(Chord::pressed(Key::D, Modifiers::ALT | Modifiers::SHIFT)), "Alt+Shift+D");
    }
}
