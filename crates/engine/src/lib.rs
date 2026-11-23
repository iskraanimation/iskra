//! The headless engine (plan §3). A [`Session`] holds the project snapshot and its undo
//! history, and every action is a command: a stable id plus JSON parameters, dispatched by
//! [`Session::execute`]. The GUI, tests, and later the CLI and MCP server all go through it.

mod commands;
mod history;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use iskra_model::{ArtLayerKind, DrawingId, PoolDrawing, Project, Shot, ShotId};
use iskra_store::{LoadReport, ProjectStore};
use serde_json::Value;

pub use commands::{Command, commands};
pub use history::{History, HistoryEntry};

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("`{id}`: invalid parameters: {source}")]
    BadParams { id: &'static str, source: serde_json::Error },
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Store(#[from] iskra_store::StoreError),
}

/// One open project, its undo history and the file it is saved in.
#[derive(Default)]
pub struct Session {
    history: History,
    /// The project file, once the project has been saved or opened.
    store: Option<Box<dyn ProjectStore>>,
    /// The snapshot that is in the file.
    saved: Option<Arc<Project>>,
    /// What was last cut or copied from a drawing: each art layer's lifted pixels.
    clipboard: Vec<(ArtLayerKind, iskra_model::ArtContent)>,
    /// The nodes that were last copied, as a template of them: what `node.paste` puts into a shot.
    copied_nodes: Option<Arc<Project>>,
    /// A stroke a client has painted already, which `drawing.stroke` takes in place of painting
    /// it again when it is the very same stroke on the very same pixels.
    painted: Option<Painted>,
    /// The run of `text.type`, or of `text.setFormat`, that the next of the same into the same
    /// box joins: the command and the box (its layer, drawing and place), where the history was
    /// before the run, and where it was after its last step. A step of anything else between
    /// ends the run.
    typing: Option<((&'static str, u64, u64, usize), history::Mark, history::Mark)>,
    /// What runs `script.run`: the script engine, which is a crate of its own above this one
    /// (`iskra-script`) and gives itself to a session with [`Session::set_script_runner`].
    script: Option<ScriptRunner>,
    /// The commands scripts add (plan §9.4), run by id as any command is.
    script_commands: Vec<ScriptCommand>,
}

/// Runs a script against a session and gives back what came of it, as `script.run` returns it.
pub type ScriptRunner = fn(&mut Session, &ScriptRequest) -> Result<Value>;

/// A script to run (plan §9).
#[derive(Clone, Debug, Default)]
pub struct ScriptRequest {
    pub code: String,
    /// The script's name, a file's for one: what its undo step and its errors are called.
    pub name: String,
    /// What the script is given as in hand, where a window would have it: the shot, the
    /// layer, the frame, as an object of JSON. A window gives its own.
    pub in_hand: Value,
    /// The script may write files, and read them outside the project's folder.
    pub write_files: bool,
    /// The command of the script's to run, where the script adds commands: its file is run to
    /// add them, and then that one's `run`.
    pub command: Option<String>,
    /// The answers to the script's dialogs, in the order it asks them: each the values of its
    /// fields by name, or null for one cancelled. A dialog the run has no answer for stops it,
    /// and is the outcome's to ask; answered, the script is run again from its start.
    pub answers: Vec<Value>,
}

/// A command a script adds (plan §9.4), with the script that has it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptCommand {
    pub id: String,
    pub title: String,
    /// The menu it is in, by its title; none for the Scripts menu.
    pub menu: Option<String>,
    /// The script, and its file's name.
    pub code: String,
    pub file: String,
    /// The script adds it with `iskra.command`, and runs it by its `run`; else the command is
    /// the whole script.
    pub added: bool,
}

impl ScriptCommand {
    /// What runs it.
    pub fn request(&self) -> ScriptRequest {
        ScriptRequest { code: self.code.clone(), name: self.title.clone(), command: self.added.then(|| self.id.clone()), ..Default::default() }
    }
}

/// A place in the history to fold what came after it into one step, with
/// [`Session::fold`].
#[derive(Clone, Copy, Debug)]
pub struct HistoryMark(history::Mark);

/// A stroke as a client painted it while it was drawn, offered to the next `drawing.stroke`
/// ([`Session::offer_painted`]). Painting a large brush's stroke anew takes long enough to be
/// felt when the pen lifts. The command takes `after` only if each of these is what it would
/// paint with and `before` is what is on the art layer, so the result is the same either way;
/// otherwise it paints the stroke itself.
#[derive(Clone, Debug)]
pub struct Painted {
    pub layer: u64,
    pub frame: u32,
    pub art_layer: ArtLayerKind,
    /// The brush the stroke was painted with: on free colour, with the colour it was painted in.
    pub brush: iskra_drawing::BrushSettings,
    pub points: Vec<iskra_drawing::StrokePoint>,
    pub swatch: iskra_drawing::SwatchId,
    pub protect: Vec<iskra_drawing::SwatchId>,
    pub mode: iskra_drawing::PaintMode,
    /// The art layer before the stroke, and with it.
    pub before: iskra_model::ArtContent,
    pub after: iskra_model::ArtContent,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// A session with the project the app opens with ([`Project::fresh`]): as `new`, and
    /// with a board in its shot.
    pub fn fresh() -> Self {
        let mut session = Self::default();
        session.replace(Arc::new(Project::fresh()), None);
        session
    }

    /// The file the project is saved in, if it has one yet.
    pub fn path(&self) -> Option<&Path> {
        self.store.as_deref().map(ProjectStore::path)
    }

    /// The project has changes that are not in its file, or has no file yet.
    pub fn is_dirty(&self) -> bool {
        !self.saved.as_ref().is_some_and(|saved| Arc::ptr_eq(saved, self.project()))
    }

    /// Make `project` the open project, with a fresh history. `store` is its file, which holds
    /// exactly this snapshot.
    fn replace(&mut self, project: Arc<Project>, store: Option<Box<dyn ProjectStore>>) {
        self.saved = store.is_some().then(|| project.clone());
        self.history = History::new(project);
        self.store = store;
    }

    /// The current project snapshot. Cloning the `Arc` gives a stable view for rendering or
    /// saving while editing continues.
    pub fn project(&self) -> &Arc<Project> {
        self.history.state()
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// Run the command `id` with `params` and return its result.
    pub fn execute(&mut self, id: &str, params: Value) -> Result<Value> {
        if let Some(command) = commands().iter().find(|c| c.id == id) {
            return (command.run)(self, params);
        }
        // A script's command, run by its script.
        let request = self.script_commands.iter().find(|c| c.id == id).map(ScriptCommand::request);
        match (request, self.script) {
            (Some(request), Some(runner)) => runner(self, &request),
            _ => Err(EngineError::UnknownCommand(id.to_string())),
        }
    }

    /// Let this session run scripts with `runner` (see [`ScriptRunner`]).
    pub fn set_script_runner(&mut self, runner: ScriptRunner) {
        self.script = Some(runner);
    }

    /// The commands scripts add, in place of those there were.
    pub fn set_script_commands(&mut self, commands: Vec<ScriptCommand>) {
        self.script_commands = commands;
    }

    pub fn script_commands(&self) -> &[ScriptCommand] {
        &self.script_commands
    }

    /// Offer the next `drawing.stroke` the stroke as it was painted already (see [`Painted`]).
    pub fn offer_painted(&mut self, painted: Painted) {
        self.painted = Some(painted);
    }

    /// Let go of a stroke that was offered and not taken.
    pub fn forget_painted(&mut self) {
        self.painted = None;
    }

    /// Apply `f` to a copy of the project and make the result the next undo step, named `label`.
    /// Nothing is recorded when `f` fails or changes nothing.
    pub fn edit<T>(&mut self, label: &str, f: impl FnOnce(&mut Project) -> Result<T>) -> Result<T> {
        let mut next = Project::clone(self.project());
        let result = f(&mut next)?;
        // There are no empty groups: one whose last node an edit took goes with it.
        for shot in next.shots.iter_mut().filter(|shot| shot.has_empty_group()) {
            Arc::make_mut(shot).drop_empty_groups();
        }
        // A panel goes with the board drawing it is tied to, wherever an edit took it.
        for at in 0..next.shots.len() {
            let Some(was) = self.project().shot(next.shots[at].id) else { continue };
            if !std::ptr::eq(was, &*next.shots[at]) && !was.panels.is_empty() {
                Arc::make_mut(&mut next.shots[at]).panels_follow(was);
            }
        }
        if next != **self.project() {
            // What would now render otherwise is marked, so that a render made before is
            // known to be of an earlier state.
            next.stamp(self.project());
            self.history.push(label, Arc::new(next));
        }
        Ok(result)
    }

    /// Run `f`, which may make several undo steps, and leave them as one, named as `f` says
    /// beside what it returns. If `f` fails, none of what it did stays.
    pub fn batch<T>(&mut self, f: impl FnOnce(&mut Session) -> Result<(T, String)>) -> Result<T> {
        let mark = self.history.mark();
        match f(self) {
            Ok((result, label)) => {
                self.history.fold(mark, &label, true);
                Ok(result)
            }
            Err(why) => {
                self.history.fold(mark, "", false);
                Err(why)
            }
        }
    }

    /// Where the history is now: [`Session::fold`] makes one step of what is done after it.
    /// [`Session::batch`] for what is not one closure, as a script's run is.
    pub fn mark(&self) -> HistoryMark {
        HistoryMark(self.history.mark())
    }

    /// Make one step of all that was done since `mark`, named `label`; with `keep` false, take
    /// it all back, leaving the history as it was at the mark.
    pub fn fold(&mut self, mark: HistoryMark, label: &str, keep: bool) {
        self.history.fold(mark.0, label, keep);
    }

    /// Write something down in the project that is no edit: it is not a step to undo, it is
    /// in every state of the history alike, and the project then has it to save.
    pub fn note(&mut self, f: impl Fn(&mut Project)) {
        self.history.replace_states(|state| {
            let mut next = Project::clone(state);
            f(&mut next);
            Some(Arc::new(next))
        });
        // The file no longer holds what the project does.
        self.saved = None;
    }

    /// Replace a shot, in every state of the history, by what `f` gives for that state's
    /// version of it. This is not an edit: nothing is added to the history, and a project that
    /// was saved still is.
    fn swap_shot(&mut self, shot: ShotId, f: impl Fn(&Shot) -> Option<Shot>) {
        // States share the versions they didn't change, and go on sharing them.
        let mut swapped: HashMap<*const Shot, (Arc<Shot>, Option<Arc<Shot>>)> = HashMap::new();
        let mut swap = |state: &Arc<Project>| {
            let at = state.shots.iter().position(|s| s.id == shot)?;
            let old = &state.shots[at];
            let (_, new) = swapped.entry(Arc::as_ptr(old)).or_insert_with(|| (old.clone(), f(old).map(Arc::new)));
            let mut next = Project::clone(state);
            next.shots[at] = new.clone()?;
            Some(Arc::new(next))
        };
        // The saved snapshot is one of the states, and stays that one.
        let saved_at = self.saved.as_ref().and_then(|saved| self.history.states().position(|state| Arc::ptr_eq(state, saved)));
        self.history.replace_states(&mut swap);
        match saved_at {
            Some(at) => self.saved = self.history.states().nth(at).cloned(),
            None => self.saved = self.saved.take().map(|saved| swap(&saved).unwrap_or(saved)),
        }
    }

    /// Load a shot: read the pixels of its drawings from the project file, for every state of
    /// the history. A project opened from a file starts with no shot loaded, and only a loaded
    /// shot can be drawn on. Loading a loaded shot does nothing.
    pub fn load_shot(&mut self, shot: ShotId) -> Result<LoadReport> {
        if self.project().shot(shot).is_none() {
            return Err(EngineError::Invalid(format!("no shot {}", shot.0)));
        }
        self.fill_shot(shot)
    }

    /// Load a shot in every state of the history that has it not loaded.
    fn fill_shot(&mut self, shot: ShotId) -> Result<LoadReport> {
        let mut waiting = self.history.states().filter_map(|state| state.shot(shot)).filter(|s| !s.loaded).peekable();
        // Without a file every shot is in memory.
        let (Some(_), Some(store)) = (waiting.peek(), self.store.as_mut()) else { return Ok(LoadReport::default()) };
        let mut drawings: HashMap<DrawingId, PoolDrawing> = HashMap::new();
        for entry in waiting.flat_map(Shot::drawings) {
            drawings.entry(entry.id).or_insert_with(|| entry.clone());
        }
        let (pixels, report) = store.load(&drawings.into_values().collect::<Vec<_>>())?;
        self.swap_shot(shot, |version| (!version.loaded).then(|| version.with_pixels(&pixels)));
        Ok(report)
    }

    /// The swatches the drawings of the shots that are not loaded are painted with, as the
    /// project file has them: read a shot at a time and let go, so nothing is loaded.
    pub(crate) fn swatches_in_file(&mut self) -> Result<BTreeSet<iskra_model::SwatchId>> {
        let project = self.project().clone();
        let mut used = BTreeSet::new();
        let Some(store) = self.store.as_mut() else { return Ok(used) };
        for shot in project.shots.iter().filter(|shot| !shot.loaded) {
            let drawings: Vec<PoolDrawing> = shot.drawings().cloned().collect();
            used.extend(store.peek(&drawings)?.values().flat_map(|drawing| drawing.swatches()));
        }
        Ok(used)
    }

    /// To be called before the current state is saved. A state in which a shot is not loaded
    /// stands for that shot as the file has it, so before a save changes or deletes a shot's
    /// drawings in the file, the states that still stand for the old ones are given them.
    fn keep_history_whole(&mut self) -> Result<()> {
        let project = self.project().clone();
        let in_file_only = |shot: ShotId| project.shot(shot).is_some_and(|s| !s.loaded);
        let waiting: BTreeSet<ShotId> = self.history.states().flat_map(|state| &state.shots).filter(|s| !s.loaded).map(|s| s.id).collect();
        for shot in waiting.into_iter().filter(|&shot| !in_file_only(shot)) {
            self.fill_shot(shot)?;
        }
        Ok(())
    }

    /// A shot with the pixels of its drawings, to look at: as it is if it is loaded, else read
    /// from the project file as it was saved, and not loaded. A picture of it is made; the
    /// shot is let go with what holds it.
    pub fn peeked_shot(&mut self, shot: ShotId) -> Result<Option<Shot>> {
        let Some(own) = self.project().shot(shot).cloned() else { return Ok(None) };
        let Some(store) = self.store.as_mut().filter(|_| !own.loaded) else { return Ok(Some(own)) };
        let drawings: Vec<PoolDrawing> = own.drawings().cloned().collect();
        let pixels = store.peek(&drawings)?;
        Ok(Some(own.with_pixels(&pixels)))
    }

    /// Release a shot's pixels from memory: the opposite of [`Session::load_shot`]. Only what
    /// is in the project file is released, so a shot with unsaved changes stays loaded, and so
    /// do the states of the history in which its drawings differ from the saved ones: undo
    /// still works. Returns whether the shot is released in the current state.
    pub fn unload_shot(&mut self, shot: ShotId) -> bool {
        // Without a file there is nowhere else the pixels are.
        if self.store.is_none() {
            return false;
        }
        let saved: HashMap<DrawingId, PoolDrawing> = match (&self.store, self.saved.as_ref().and_then(|saved| saved.shot(shot))) {
            (Some(_), Some(saved)) if saved.loaded => saved.drawings().map(|d| (d.id, d.clone())).collect(),
            _ => HashMap::new(),
        };
        let as_saved = |entry: &PoolDrawing| saved.get(&entry.id).is_some_and(|saved| Arc::ptr_eq(&saved.drawing, &entry.drawing));
        self.swap_shot(shot, |version| (version.loaded && version.drawings().all(as_saved)).then(|| version.without_pixels()));
        if let Some(store) = self.store.as_mut() {
            store.release(&saved.into_keys().collect::<Vec<_>>());
        }
        self.project().shot(shot).is_some_and(|s| !s.loaded)
    }

    /// Undo and redo end a run of typing (see `text.type`): what is typed next is a step of
    /// its own.
    pub fn undo(&mut self) -> bool {
        self.typing = None;
        self.history.undo()
    }

    pub fn redo(&mut self) -> bool {
        self.typing = None;
        self.history.redo()
    }
}
