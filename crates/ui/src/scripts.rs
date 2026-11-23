//! The user's Scripts folder (plan §9.4): `scripts` beside the preferences, seen in every
//! project. Each script in it adds commands with `iskra.command`, which take their place in the
//! menus, the command palette and the Shortcuts table; a script that adds none is a command
//! itself, in the Scripts menu, named after its file.

use std::path::Path;

use iskra_engine::{ScriptCommand, Session};

use crate::commands::{self, Menu};

/// A script's command as the window lists it. Its id and title live as long as the app, as
/// the built-in commands' do, so that menus and shortcuts take them alike; they are made again
/// only when the scripts are read again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub id: &'static str,
    pub title: &'static str,
    pub menu: Menu,
}

/// What reading the folder came to: the commands, and what went wrong, a line a problem.
#[derive(Default)]
pub struct Read {
    pub commands: Vec<ScriptCommand>,
    pub problems: Vec<String>,
}

/// The menu named `title`, as a script asks for it; the Scripts menu for none, or a name that
/// is no menu.
pub fn menu_named(title: Option<&str>) -> Menu {
    title.and_then(|title| Menu::ALL.into_iter().find(|menu| menu.title().eq_ignore_ascii_case(title.trim()))).unwrap_or(Menu::Scripts)
}

/// An id for a script that adds no command: `script.` and its file's name, of letters, digits
/// and `_` alone.
fn id_of_file(stem: &str) -> String {
    let name: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("script.{name}")
}

/// Whether `id` is taken by a command of the engine's or the window's.
fn built_in(id: &str) -> bool {
    iskra_engine::commands().iter().any(|c| c.id == id) || commands::command(id).is_some()
}

/// Read the scripts in `folder`, in the order of their names, against `session`, which is not
/// changed. A folder that is not there holds none.
pub fn read(folder: &Path, session: &mut Session) -> Read {
    let mut read = Read::default();
    let Ok(listed) = std::fs::read_dir(folder) else { return read };
    let mut files: Vec<_> = listed.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "js")).collect();
    files.sort();
    for path in files {
        let file = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let code = match std::fs::read_to_string(&path) {
            Ok(code) => code,
            Err(why) => {
                read.problems.push(format!("{file}: can't be read: {why}"));
                continue;
            }
        };
        let added = if code.contains("iskra.command(") {
            match iskra_script::added_commands(session, &code, &file) {
                Ok(added) => added.into_iter().map(|a| (a.id, a.title, a.menu, true)).collect(),
                Err(why) => {
                    read.problems.push(why.to_string());
                    continue;
                }
            }
        } else {
            let stem = path.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            vec![(id_of_file(&stem), stem, None, false)]
        };
        for (id, title, menu, added) in added {
            if built_in(&id) || read.commands.iter().any(|c| c.id == id) {
                read.problems.push(format!("{file}: the command `{id}` is taken already, and is left out"));
                continue;
            }
            read.commands.push(ScriptCommand { id, title, menu, code: code.clone(), file: file.clone(), added });
        }
    }
    read
}

/// The window's entries for `commands`.
pub fn entries(commands: &[ScriptCommand]) -> Vec<Entry> {
    commands
        .iter()
        .map(|c| Entry {
            id: Box::leak(c.id.clone().into_boxed_str()),
            title: Box::leak(c.title.clone().into_boxed_str()),
            menu: menu_named(c.menu.as_deref()),
        })
        .collect()
}
