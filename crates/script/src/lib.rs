//! Scripting (plan §9): JavaScript on the boa engine, which edits a session only by running its
//! commands, so what a script does is what the window does, undone the same way.
//!
//! ```text
//! var project = iskra.run("project.get");
//! project.shots[0].layers.forEach(function (layer) {
//!     layer.drawings.forEach(function (d, i) {
//!         iskra.run("drawing.rename", { layer: layer.id, drawing: d.id, name: "pose-" + (i + 1) });
//!     });
//! });
//! ```
//!
//! * **`iskra.run(id, params)`** runs any command (`app.commands` lists them) and gives back its
//!   result; a command that fails throws, with what the engine said.
//! * **`print(...)`**, `iskra.print` and `console.log` write lines to the run's output.
//! * **One undo step a run**, named after the script. A script that throws, part way or not, has
//!   all it did taken back.
//! * **`iskra.dialog({ title, fields })`** asks for values. A run is given the answers
//!   (`ScriptRequest::answers`); one that asks a dialog it has no answer to stops there, has all
//!   it did taken back, and gives the dialog in its outcome, for the window to show and run it
//!   again from its start with the answer. So a script asks its dialogs before it writes files.
//!
//! [`install`] gives a session this engine for `script.run`, which is how menus, other tools and
//! tests run a script; the window calls [`run`] to have the output as well.
//!
//! How an error is placed at its line and column, and the guard against brackets nested deeply
//! enough to overflow the parser's stack, are ported from EffectCraft's `script` crate (see
//! `ATTRIBUTION.md`); the rest is Iskra's own.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod runtime;
#[cfg(test)]
mod tests;

use iskra_engine::{EngineError, ScriptRequest, Session};
use serde_json::{Value, json};

pub use runtime::{MAX_NESTING, added_commands, check_nesting, run};

/// What a run of a script came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// The value of the script's last expression, as JSON where it is one.
    pub result: Value,
    /// What it printed, a line an entry: kept when it fails too.
    pub output: Vec<String>,
    pub error: Option<ScriptError>,
    /// The dialog the script asked for and was given no answer to, as `iskra.dialog` describes
    /// it: its `title`, `ok` and `fields`. The run is taken back, to be run again with the
    /// answer added to its request's.
    pub asks: Option<Value>,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

/// A command a script adds with `iskra.command` (plan §9.4).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct Added {
    pub id: String,
    pub title: String,
    /// The menu it asks to be in, by its title.
    #[serde(default)]
    pub menu: Option<String>,
}

/// What stopped a script: what was thrown, and where in the script, when that is known.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptError {
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// The script's name, as its run was given it.
    pub file: String,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(column)) => write!(f, "{}:{line}:{column}: {}", self.file, self.message),
            (Some(line), None) => write!(f, "{}:{line}: {}", self.file, self.message),
            _ => write!(f, "{}: {}", self.file, self.message),
        }
    }
}

/// Let `session` run scripts: `script.run` then runs them with this engine.
pub fn install(session: &mut Session) {
    session.set_script_runner(runner);
}

/// `script.run`: the script's result and output, or its error as the command's.
fn runner(session: &mut Session, request: &ScriptRequest) -> iskra_engine::Result<Value> {
    let outcome = run(session, request);
    if let Some(asks) = &outcome.asks {
        let title = asks["title"].as_str().unwrap_or_default();
        return Err(EngineError::Invalid(format!("{}: the script asks for a dialog, \"{title}\": give its answers in `answers`", request.name)));
    }
    match outcome.error {
        Some(error) => Err(EngineError::Invalid(error.to_string())),
        None => Ok(json!({ "result": outcome.result, "output": outcome.output })),
    }
}
