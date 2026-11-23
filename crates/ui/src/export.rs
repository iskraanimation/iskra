//! The Export page (plan §7.14): render settings on the left, the render queue on the right,
//! and between them the viewer over the strip of the storyboard's shots.
//!
//! This module is what the page shows and the queue it keeps. Rendering itself is the
//! headless `iskra-export`'s, on a thread of its own that the app starts and listens to.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;

use iskra_export::{Codec, ExportError, Format, IfExists, Naming, Pick, ProRes, Quality, Range, Report, Settings, SoundCodec};
use iskra_model::{Project, ShotId, Status, Storyboard, StoryboardId};

use crate::form;
use crate::items::status_color;
use crate::theme::Theme;

/// A way of rendering that the app comes with: its name, what it is for, the settings, and
/// whether it makes the storyboard one file.
pub struct Preset {
    pub name: &'static str,
    pub hint: &'static str,
    pub settings: Settings,
    pub film: bool,
}

/// The app's own presets (plan §7.14).
pub fn presets() -> [Preset; 6] {
    let cut = Settings::default();
    [
        Preset {
            name: "PNG Sequence",
            hint: "A PNG a frame, 8 bits, with transparency: for compositing",
            settings: Settings { format: Format::Png, ..cut },
            film: false,
        },
        Preset {
            name: "EXR Sequence",
            hint: "An EXR a frame, floats in linear light, with transparency: for finishing",
            settings: Settings { format: Format::Exr, bits: 32, ..cut },
            film: false,
        },
        Preset {
            name: "ProRes MOV",
            hint: "A QuickTime movie a shot, ProRes 422 HQ with its sound: for the edit",
            settings: Settings { format: Format::Mov, alpha: false, ..cut },
            film: false,
        },
        Preset {
            name: "ProRes 4444 MOV",
            hint: "A QuickTime movie a shot, ProRes 4444 with transparency: for compositing from one file",
            settings: Settings { format: Format::Mov, prores: ProRes::P4444, alpha: true, ..cut },
            film: false,
        },
        Preset {
            name: "H.264 MP4",
            hint: "An MP4 a shot, H.264 with its sound: to send and to watch",
            settings: Settings { format: Format::Mp4, alpha: false, ..cut },
            film: false,
        },
        Preset {
            name: "Animatic",
            hint: "The storyboard's boards as one small MP4 with its sound: the film so far, to watch",
            settings: Settings { format: Format::Mp4, alpha: false, scale: 0.5, work: iskra_model::Work::Boards, ..cut },
            film: true,
        },
    ]
}

/// What a part of a job renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Shot(ShotId),
    /// A storyboard as one movie.
    Film(StoryboardId),
}

/// Where a part of a job stands.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Waiting,
    Rendering,
    /// Rendered: how many files, and where.
    Done {
        files: usize,
        at: PathBuf,
    },
    /// It was there already, and left.
    Skipped,
    Failed(String),
    Stopped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub what: What,
    pub name: String,
    pub state: State,
    /// The pass of the shot that is rendered, or its whole picture as an output beside its
    /// passes; without, its whole picture as it always was.
    pub pick: Option<Pick>,
    /// What its files are called, and the folder under the job's place they go to.
    pub naming: Naming,
}

/// What a job is made of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Take {
    /// The shot that is in the viewer.
    Current,
    /// The shots that are selected under the viewer, or all of them: a file each.
    #[default]
    Shots,
    /// The storyboard as one movie.
    Film,
}

/// The tab of the render settings that is open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Video,
    Audio,
    File,
}

pub use iskra_export::{FileNaming, NameFrom, ShotNames};

/// A box of the render queue: a job, or a render the project knows of, by its record's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boxed {
    Job(u64),
    Record(usize),
}

/// A row of the passes under the shots: a Pass by its name, or `None` for *Final*, the shot's
/// whole picture.
pub type Row = Option<String>;

/// What the shot's whole picture is called, where it is rendered beside its passes.
pub const FINAL: &str = "Final";

/// A job of the render queue: what was selected when it was added, with the settings and the
/// place as they were then. Changing the settings afterwards changes no job.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub settings: Settings,
    pub dest: PathBuf,
    /// What to do where a part's files are there already. Asked once, for the job.
    pub if_exists: IfExists,
    pub parts: Vec<Part>,
}

/// The part of a job that is being rendered, on its thread.
pub struct Running {
    pub job: u64,
    pub part: usize,
    /// How many frames are done, of how many.
    pub progress: Arc<(AtomicUsize, AtomicUsize)>,
    pub stop: Arc<AtomicBool>,
    pub result: Receiver<Result<Report, ExportError>>,
    /// The shots that were loaded for it, to be let go after.
    pub loaded: Vec<ShotId>,
    /// How what it renders was when it was started, for the record of the render.
    pub mark: Option<iskra_model::RenderMark>,
}

/// What the page asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Rename the pass of that name in every shot.
    RenamePass(String, String),
    /// Make a job of what is selected, with the settings as they are.
    Add,
    RenderAll,
    /// Render these boxes of the queue alone: a job's parts that wait, or all of them again
    /// where none do; a render the project knows of, again as it was rendered.
    Render(Vec<Boxed>),
    Stop,
    Remove(u64),
    /// Forget the render that the project's record at this place is of. Its files stay.
    Forget(usize),
    /// Show a rendered part's files in the system's file browser.
    Reveal(PathBuf),
    /// Ask for the folder renders go into.
    ChooseFolder,
    /// A shot of the strip was clicked: it is shown in the viewer.
    Show(ShotId),
    /// Keep the settings as they are as a preset of the user's, under this name. A preset of
    /// that name is replaced.
    SavePreset(String),
    /// Give the user's preset at this place another name.
    RenamePreset(usize, String),
    /// Ask whether the user's preset at this place is to be deleted, and delete it if so.
    ClearPreset(usize),
    /// Ask for a preset's file, and bring it in.
    ImportPreset,
    /// Ask where to write the user's preset at this place as a file.
    ExportPreset(usize),
    /// Render again what the project's render record at this place is of, as it was rendered.
    Again(usize),
    /// Write the storyboard's cut as an edit list of this kind, beside the renders.
    EditList(iskra_export::ListFormat),
}

pub struct ExportPage {
    pub settings: Settings,
    /// What a job is made of: the shot in the viewer, shots a file each, or the storyboard
    /// as one movie.
    pub take: Take,
    pub tab: Tab,
    /// What the files are named for, the name of the user's own, what comes after the name,
    /// and the folder under the place renders go to.
    /// How the files are named, as the File tab says.
    pub file_naming: FileNaming,
    /// The project's name, for a file's name made of parts: the app says it before the page
    /// is shown.
    pub project_name: String,
    /// The picture's size is one the user types, though it may be a named one's just now.
    own_size: bool,
    /// Where renders go, if the user chose a place; else the project's `Renders` folder.
    pub dest: Option<PathBuf>,
    /// The shots that *Add to Render Queue* takes. None selected is all of the storyboard.
    pub selected: Vec<ShotId>,
    pub jobs: Vec<Job>,
    next_job: u64,
    /// The queue is being worked through.
    pub rendering: bool,
    /// The jobs that are rendered while the queue is, where only some were asked for.
    pub only: Option<Vec<u64>>,
    /// The boxes of the queue that are picked: the Render button renders them alone.
    pub picked: Vec<Boxed>,
    /// Where each box of the queue was drawn, and its cross; what the Render button said.
    #[cfg(test)]
    pub queue_boxes: Vec<(Boxed, egui::Rect, egui::Rect)>,
    #[cfg(test)]
    pub render_said: String,
    pub running: Option<Running>,
    /// A pass is switched on under the shots, for some shot: a job renders passes, each an
    /// output of its own. Said again each time the strip is drawn.
    pub apart: bool,
    /// The rows that are not as they start, for every shot: *Final* starts on, and the passes
    /// off, so that a render is of the whole picture until a pass is asked for.
    pub rows_switched: Vec<Row>,
    /// The shots that are not as their rows are: a pass that is on in a row that is off, or
    /// off in one that is on.
    pub cells_other: Vec<(ShotId, Row)>,
    /// A pass being renamed in every shot: its name, the new one so far, and whether the
    /// field is to take the keys.
    renaming: Option<(String, String, bool)>,
    /// Where each cell of the passes was drawn, and what the records said of each pass's.
    #[cfg(test)]
    pub cells: Vec<(ShotId, Row, egui::Rect)>,
    #[cfg(test)]
    pub cell_states: Vec<(ShotId, Row, Option<bool>)>,
    /// A preset's name being typed: for the user's preset at this place, or for a new one.
    naming: Option<(Option<usize>, String, bool)>,
    /// What the page last did that left no other trace: an edit list that was written.
    pub said: Option<String>,
}

impl Default for ExportPage {
    fn default() -> Self {
        let first = &presets()[0];
        Self {
            settings: first.settings,
            take: Take::Shots,
            tab: Tab::Video,
            file_naming: FileNaming::default(),
            project_name: String::new(),
            own_size: false,
            dest: None,
            selected: Vec::new(),
            jobs: Vec::new(),
            next_job: 1,
            rendering: false,
            only: None,
            picked: Vec::new(),
            #[cfg(test)]
            queue_boxes: Vec::new(),
            #[cfg(test)]
            render_said: String::new(),
            running: None,
            apart: false,
            rows_switched: Vec::new(),
            renaming: None,
            cells_other: Vec::new(),
            #[cfg(test)]
            cells: Vec::new(),
            #[cfg(test)]
            cell_states: Vec::new(),
            naming: None,
            said: None,
        }
    }
}

/// How high a row of passes is under the shots, and how wide the rows' names are.
const PASS_ROW: f32 = 14.0;
const PASS_NAMES: f32 = 104.0;

/// The colour of what was changed since it was rendered: the warnings' colour.
fn stale_colour(ctx: &egui::Context) -> egui::Color32 {
    Theme::of(ctx).colour(crate::theme::Role::Warning)
}

/// What the project's records say of a shot: `Some(true)` if a render of it is still as the
/// shot is, `Some(false)` if it was rendered and changed since, `None` if it never was. Only a
/// render of its animation counts: one of its boards, an animatic, is no render of the shot.
pub fn rendered(project: &Project, shot: ShotId) -> Option<bool> {
    rendered_as(project, shot, &None)
}

/// What the records say of one output of a shot (plan §7.14): its whole picture, rendered as
/// itself or as *Final* beside its passes, or one of its passes by its name.
pub fn rendered_as(project: &Project, shot: ShotId, row: &Row) -> Option<bool> {
    let animation = |record: &&iskra_model::RenderRecord| iskra_export::settings_of(record).is_none_or(|settings| settings.work != iskra_model::Work::Boards);
    let output = |record: &&iskra_model::RenderRecord| match row {
        Some(pass) => record.pass.as_ref() == Some(pass),
        None => record.pass.as_deref().is_none_or(|pass| pass == FINAL),
    };
    let mut of_shot = project.renders.iter().filter(|record| record.what == iskra_model::Rendered::Shot(shot)).filter(animation).filter(output).peekable();
    of_shot.peek()?;
    Some(of_shot.any(|record| !iskra_export::is_stale(project, record)))
}

/// The settings in a few words, as a job is named.
fn said(settings: &Settings, film: bool) -> String {
    let kind = match settings.format {
        Format::Png => "PNG",
        Format::Jpeg => "JPEG",
        Format::Tiff => "TIFF",
        Format::Exr => "EXR",
        Format::Mov => "ProRes MOV",
        Format::Mp4 => "H.264 MP4",
    };
    let size = if settings.scale == 1.0 { String::new() } else { format!(", {:.0}%", settings.scale * 100.0) };
    // A rate that is not the project's is said.
    let size = match settings.frame_rate {
        Some(rate) => format!("{size}, {} fps", (rate.as_f64() * 1000.0).round() / 1000.0),
        None => size,
    };
    // A render of the boards says so: it is an animatic, and no render of the animation.
    let boards = if settings.work == iskra_model::Work::Boards { ", boards" } else { "" };
    if film { format!("{kind}, the film{boards}{size}") } else { format!("{kind}{boards}{size}") }
}

impl ExportPage {
    /// Where renders go: the place the user chose, or the `Renders` folder beside the
    /// project's file. `None` for a project that has no file yet and no place chosen.
    pub fn dest(&self, project_file: Option<&Path>) -> Option<PathBuf> {
        self.dest.clone().or_else(|| Some(project_file?.parent()?.join("Renders")))
    }

    /// The rows under the shots: *Final*, then each name of a Pass the board's shots have, in
    /// the order the shots first have them.
    pub fn pass_rows(project: &Project, board: &Storyboard) -> Vec<Row> {
        let mut names: Vec<String> = Vec::new();
        for shot in board.clips().iter().filter_map(|clip| project.shot(clip.shot)) {
            for pass in shot.passes() {
                if !names.contains(&pass.name) {
                    names.push(pass.name.clone());
                }
            }
        }
        std::iter::once(None).chain(names.into_iter().map(Some)).collect()
    }

    /// Whether a row is switched on for every shot: *Final* until it is switched off, a pass
    /// once it is switched on.
    pub fn row_on(&self, row: &Row) -> bool {
        row.is_none() != self.rows_switched.contains(row)
    }

    /// Whether a row is switched on for a shot: as the row is, unless the shot is set apart.
    pub fn is_on(&self, shot: ShotId, row: &Row) -> bool {
        let other = self.cells_other.iter().any(|(of, pass)| *of == shot && pass == row);
        self.row_on(row) != other
    }

    /// Switch a row of passes, for every shot: those that were set apart are as the row again.
    pub fn switch_row(&mut self, row: &Row) {
        match self.rows_switched.iter().position(|switched| switched == row) {
            Some(at) => drop(self.rows_switched.remove(at)),
            None => self.rows_switched.push(row.clone()),
        }
        self.cells_other.retain(|(_, pass)| pass != row);
    }

    /// A pass was renamed: its switches go with it.
    pub fn pass_renamed(&mut self, from: &str, to: &str) {
        let rename = |row: &mut Row| {
            if row.as_deref() == Some(from) {
                *row = Some(to.to_string());
            }
        };
        self.rows_switched.iter_mut().for_each(rename);
        self.cells_other.iter_mut().for_each(|(_, row)| rename(row));
    }

    /// Whether a job of these shots renders passes: one of their passes is switched on.
    pub fn renders_passes<'a>(&self, shots: impl IntoIterator<Item = &'a iskra_model::Shot>) -> bool {
        shots.into_iter().any(|shot| shot.passes().any(|pass| self.is_on(shot.id, &Some(pass.name.clone()))))
    }

    /// Switch a pass of one shot.
    pub fn switch_cell(&mut self, shot: ShotId, row: &Row) {
        match self.cells_other.iter().position(|(of, pass)| *of == shot && pass == row) {
            Some(at) => drop(self.cells_other.remove(at)),
            None => self.cells_other.push((shot, row.clone())),
        }
    }

    /// What a job renders of a shot, as the rows are switched. A job of no passes renders the
    /// shot's whole picture, as it always did, if *Final* is on for it. A job of passes renders
    /// each of the shot's passes that is on, an output of its own, and *Final* beside them if
    /// it is on; a shot that has none of the passes that are on is rendered whole, as *Final*.
    pub fn parts_of(&self, shot: &iskra_model::Shot, passes: bool) -> Vec<Part> {
        let part = |name: String, pick: Option<Pick>| Part { what: What::Shot(shot.id), name, state: State::Waiting, pick, naming: Naming::default() };
        let whole = self.is_on(shot.id, &None);
        if !passes {
            return if whole { vec![part(shot.name.clone(), None)] } else { Vec::new() };
        }
        let on: Vec<String> = shot.passes().map(|pass| pass.name.clone()).filter(|name| self.is_on(shot.id, &Some(name.clone()))).collect();
        let final_part = || part(format!("{} · {FINAL}", shot.name), Some(Pick { pass: None, named: Some(FINAL.to_string()) }));
        let mut parts: Vec<Part> =
            on.iter().map(|name| part(format!("{} · {name}", shot.name), Some(Pick { pass: Some(name.clone()), named: Some(name.clone()) }))).collect();
        if whole || on.is_empty() {
            parts.push(final_part());
        }
        parts
    }

    /// Make a job with the settings as they are, to be rendered into `dest`: of the shot
    /// `showing` in the viewer, of the shots that are selected or all of `board`'s, or of the
    /// storyboard as one movie, as the page says. Returns its id; `None` if there is nothing
    /// to render.
    pub fn add_job(&mut self, project: &Project, board: &Storyboard, dest: PathBuf, showing: Option<ShotId>) -> Option<u64> {
        let settings = self.settings.fitted();
        let parts: Vec<Part> = if self.film() {
            // A movie of each pass that is on, of a name the board's shots have, and the whole
            // picture beside them; without passes, the storyboard as it is.
            let naming = self.naming_of(board, None, false);
            let part = |name: String, pick: Option<Pick>| Part { what: What::Film(board.id), name, state: State::Waiting, pick, naming: naming.clone() };
            let passes: Vec<String> = Self::pass_rows(project, board).into_iter().flatten().filter(|name| self.row_on(&Some(name.clone()))).collect();
            match (passes.is_empty(), self.row_on(&None)) {
                (true, true) => vec![part(board.name.clone(), None)],
                (true, false) => Vec::new(),
                (false, whole) => {
                    let of = |name: &String| part(format!("{} · {name}", board.name), Some(Pick { pass: Some(name.clone()), named: Some(name.clone()) }));
                    let mut parts: Vec<Part> = passes.iter().map(of).collect();
                    if whole {
                        parts.push(part(format!("{} · {FINAL}", board.name), Some(Pick { pass: None, named: Some(FINAL.to_string()) })));
                    }
                    parts
                }
            }
        } else {
            let wanted = |shot: ShotId| match self.take {
                Take::Current => showing == Some(shot),
                _ => self.selected.is_empty() || self.selected.contains(&shot),
            };
            let shots: Vec<&iskra_model::Shot> =
                board.clips().iter().map(|clip| clip.shot).filter(|shot| wanted(*shot)).filter_map(|shot| project.shot(shot)).collect();
            let several = shots.len() > 1;
            let passes = self.renders_passes(shots.iter().copied());
            let named = |shot: &iskra_model::Shot| {
                let naming = self.naming_of(board, Some(ShotNames::from(shot)), several);
                self.parts_of(shot, passes).into_iter().map(move |part| Part { naming: naming.clone(), ..part })
            };
            shots.iter().flat_map(|shot| named(shot)).collect()
        };
        if parts.is_empty() {
            return None;
        }
        let id = self.next_job;
        self.next_job += 1;
        let passes = if parts.iter().any(|part| part.pick.is_some()) { ", a pass each" } else { "" };
        let name = format!("{}{passes}", said(&settings, self.film()));
        self.jobs.push(Job { id, name, settings, dest, if_exists: IfExists::Fail, parts });
        Some(id)
    }

    /// The next part that waits to be rendered: its job's place in the queue, and its own.
    pub fn next_waiting(&self) -> Option<(usize, usize)> {
        let asked = |job: &Job| self.only.as_ref().is_none_or(|only| only.contains(&job.id));
        self.jobs
            .iter()
            .enumerate()
            .filter(|(_, job)| asked(job))
            .find_map(|(at, job)| Some((at, job.parts.iter().position(|part| part.state == State::Waiting)?)))
    }

    /// Make a job that renders again what a record of the project is of: with the settings it
    /// was rendered with, to where it is, over what is there.
    pub fn job_again(&mut self, project: &Project, record: &iskra_model::RenderRecord) -> Option<u64> {
        let settings = iskra_export::settings_of(record)?;
        let (what, name, film) = match record.what {
            iskra_model::Rendered::Shot(shot) => (What::Shot(shot), project.shot(shot)?.name.clone(), false),
            iskra_model::Rendered::Film(board) => (What::Film(board), project.storyboard(board)?.name.clone(), true),
        };
        // A pass lies a folder down, under its name: the shot's own name and place are above.
        let (dest, stem, pick) = match &record.pass {
            Some(pass) => {
                let (shot_name, dest) = if settings.format.is_movie() {
                    let stem = record.at.file_stem()?.to_string_lossy().into_owned();
                    (stem.strip_suffix(&format!("_{pass}")).map_or(stem.clone(), str::to_string), record.at.parent()?.parent()?.to_path_buf())
                } else {
                    (record.at.parent()?.file_name()?.to_string_lossy().into_owned(), record.at.parent()?.parent()?.to_path_buf())
                };
                let picked = Pick { pass: (pass != FINAL).then(|| pass.clone()), named: Some(pass.clone()) };
                (dest, Some(shot_name), Some(picked))
            }
            None => (record.at.parent()?.to_path_buf(), record.at.file_stem().map(|stem| stem.to_string_lossy().into_owned()), None),
        };
        let name = match &record.pass {
            Some(pass) => format!("{name} · {pass}"),
            None => name,
        };
        let id = self.next_job;
        self.next_job += 1;
        let job = Job {
            id,
            name: format!("{}, again", said(&settings, film)),
            settings,
            dest,
            if_exists: IfExists::Replace,
            // Named as it was: what is there may have had a name of its own.
            parts: vec![Part { what, name, state: State::Waiting, pick, naming: Naming { name: stem, ..Naming::default() } }],
        };
        self.jobs.push(job);
        Some(id)
    }

    /// The storyboard is made one movie.
    pub fn film(&self) -> bool {
        self.take == Take::Film && self.settings.format.is_movie()
    }

    /// Take a preset's settings. One that is not of the storyboard as one movie leaves what
    /// is rendered as it is chosen.
    /// The passes as they are switched, for a preset to keep: those on, and Final off.
    pub fn passes_switched(&self) -> iskra_export::PresetPasses {
        let on = self.rows_switched.iter().flatten().cloned().collect();
        iskra_export::PresetPasses { on, without_final: self.rows_switched.contains(&None) }
    }

    /// Take a preset of the user's own: its settings, how much it renders, and where it keeps
    /// them, its files' naming and the passes it renders.
    pub fn take_own(&mut self, preset: &iskra_export::Preset) {
        self.take_preset(preset.settings, preset.film, Some(preset.this_shot));
        if let Some(naming) = &preset.naming {
            self.file_naming = naming.clone();
        }
        if let Some(passes) = &preset.passes {
            self.rows_switched = passes.on.iter().cloned().map(Some).chain(passes.without_final.then_some(None)).collect();
            self.cells_other.clear();
        }
    }

    /// Take a preset's settings, and how much it renders: one movie or not, and, for a
    /// preset of the user's own, the shot in the viewer alone or a file for each shot. The
    /// app's presets leave that as it was, and what is burned in.
    pub fn take_preset(&mut self, settings: Settings, film: bool, this_shot: Option<bool>) {
        // What is burned in is the user's: the app's presets leave it as it is.
        let burn_in = self.settings.burn_in;
        self.settings = settings;
        if this_shot.is_none() {
            self.settings.burn_in = burn_in;
        }
        self.take = match (film, this_shot, self.take) {
            (true, ..) => Take::Film,
            (false, Some(true), _) => Take::Current,
            (false, Some(false), _) | (false, None, Take::Film) => Take::Shots,
            (false, None, take) => take,
        };
    }

    /// How a part of a job is named: `own` is the shot's name, or nothing for the storyboard
    /// as one movie; `several` says that other shots are rendered beside it, which a name
    /// that is not the shot's own has to be told apart from.
    pub fn naming_of(&self, board: &Storyboard, own: Option<ShotNames>, several: bool) -> Naming {
        self.file_naming.naming(&self.project_name, &board.name, own, several)
    }

    /// The render settings (plan §7.14): the presets, where renders go and what is rendered,
    /// then the picture, the sound and the file's name, a tab each.
    pub fn settings_panel(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        project_file: Option<&Path>,
        board: Option<&Storyboard>,
        own: &[iskra_export::Preset],
        showing: Option<ShotId>,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        ui.strong("Render Settings");
        ui.add_space(4.0);
        // The presets: the app's, the user's own, and what is done with them.
        let film = self.take == Take::Film;
        let is = |settings: &Settings, of_film: bool| self.settings == *settings && (of_film == film || !settings.format.is_movie());
        let builtin = presets();
        let this_shot = self.take == Take::Current;
        let chosen_own = own.iter().position(|preset| is(&preset.settings, preset.film) && (preset.film || preset.this_shot == this_shot));
        let chosen = builtin.iter().find(|preset| is(&preset.settings, preset.film)).map(|preset| preset.name.to_string());
        let shown = chosen.clone().or(chosen_own.map(|at| own[at].name.clone())).unwrap_or("Custom settings".to_string());
        let (mut taken, mut taken_own) = (None, None);
        egui::ComboBox::from_id_salt("render presets").selected_text(shown.as_str()).width(ui.available_width() - 8.0).height(420.0).show_ui(ui, |ui| {
            for preset in &builtin {
                if ui.selectable_label(chosen.as_deref() == Some(preset.name), preset.name).on_hover_text(preset.hint).clicked() {
                    taken = Some((preset.settings, preset.film, None));
                }
            }
            if !own.is_empty() {
                ui.separator();
            }
            for (at, preset) in own.iter().enumerate() {
                if ui.selectable_label(chosen.is_none() && chosen_own == Some(at), egui::RichText::new(&preset.name).italics()).clicked() {
                    taken_own = Some(at);
                }
            }
            ui.separator();
            if ui.button("Save as New Preset…").on_hover_text("Keep these settings under a name").clicked() {
                self.naming = Some((None, String::new(), true));
            }
            if ui.button("Import Preset…").on_hover_text("Bring in a preset from a file").clicked() {
                actions.push(Action::ImportPreset);
            }
            ui.separator();
            // What is done with the preset of the user's own that is chosen.
            let mine = chosen_own.filter(|_| chosen.is_none());
            ui.add_enabled_ui(mine.is_some(), |ui| {
                let hint = "Choose a preset of your own first";
                if ui.button("Rename Preset…").on_disabled_hover_text(hint).clicked()
                    && let Some(at) = mine
                {
                    self.naming = Some((Some(at), own[at].name.clone(), true));
                }
                if ui.button("Export Preset…").on_hover_text("Write it as a file, to hand to someone else").on_disabled_hover_text(hint).clicked()
                    && let Some(at) = mine
                {
                    actions.push(Action::ExportPreset(at));
                }
                if ui.button("Clear Preset").on_hover_text("Delete the preset of your own that is chosen").on_disabled_hover_text(hint).clicked()
                    && let Some(at) = mine
                {
                    actions.push(Action::ClearPreset(at));
                }
            });
        });
        if let Some((settings, film, this_shot)) = taken {
            self.take_preset(settings, film, this_shot);
        }
        if let Some(preset) = taken_own.and_then(|at| own.get(at)) {
            self.take_own(preset);
        }
        // The field a preset's name is typed into. Enter takes it, Escape gives it up.
        if let Some((_, text, focus)) = self.naming.as_mut() {
            let field = ui.add(egui::TextEdit::singleline(text).hint_text("The preset's name").desired_width(f32::INFINITY));
            if std::mem::take(focus) {
                field.request_focus();
            }
            if field.lost_focus() {
                let keep = !ui.input(|i| i.key_pressed(egui::Key::Escape));
                if let Some((which, text, _)) = self.naming.take().filter(|(_, text, _)| keep && !text.trim().is_empty()) {
                    actions.push(match which {
                        Some(at) => Action::RenamePreset(at, text.trim().to_string()),
                        None => Action::SavePreset(text.trim().to_string()),
                    });
                }
            }
        }
        ui.add_space(4.0);
        ui.separator();

        // Where renders go: typed, or browsed for. Empty is the project's Renders folder.
        form::row(ui, "Location", |ui| {
            let browse = ui.available_width() - 70.0;
            let mut text = self.dest.as_ref().map_or(String::new(), |dest| dest.display().to_string());
            let hint = match self.dest(project_file) {
                Some(dest) => dest.display().to_string(),
                None => "Save the project, or choose a folder".to_string(),
            };
            let field = ui.add(egui::TextEdit::singleline(&mut text).hint_text(hint.as_str()).desired_width(browse.max(60.0)));
            if field.changed() {
                self.dest = Some(text.trim()).filter(|text| !text.is_empty()).map(PathBuf::from);
            }
            field.on_hover_text(format!("The folder renders go into. Empty: the project's Renders folder\n{hint}"));
            if ui.button("Browse").clicked() {
                actions.push(Action::ChooseFolder);
            }
        });
        ui.add_space(4.0);
        ui.separator();

        // What is exported, and as what.
        let movie = self.settings.format.is_movie();
        form::row(ui, "Export", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.radio_value(&mut self.settings.work, iskra_model::Work::Boards, "Storyboard")
                    .on_hover_text("The boards alone, as the Storyboard page plays them: an animatic");
                ui.radio_value(&mut self.settings.work, iskra_model::Work::Animation, "Animation")
                    .on_hover_text("What was animated: the final animation, with the boards left out");
            });
        });
        form::row(ui, "As", |ui| {
            ui.vertical(|ui| {
                ui.add_enabled(movie, egui::RadioButton::new(self.take == Take::Film, "One movie"))
                    .on_hover_text("The storyboard as one file: its shots in the cut, one after another, black between them, with all of its sound")
                    .on_disabled_hover_text("One movie needs a movie format: QuickTime or MP4")
                    .clicked()
                    .then(|| self.take = Take::Film);
                ui.radio_value(&mut self.take, Take::Shots, "A file per shot")
                    .on_hover_text("A file for each shot, or a folder of frames. With shots selected under the viewer, only those");
                let current = showing.and_then(|shot| project.shot(shot)).map_or("the shot in the viewer".to_string(), |shot| shot.name.clone());
                ui.radio_value(&mut self.take, Take::Current, "This shot only").on_hover_text(format!("Only the shot that is in the viewer: {current}"));
            });
        });
        if self.take == Take::Film && !self.settings.format.is_movie() {
            self.take = Take::Shots;
        }
        ui.add_space(4.0);
        ui.separator();

        // The tabs, side by side and as wide as each other, in the middle.
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let width = ((ui.available_width() - 8.0).min(300.0) / 3.0).floor();
            ui.add_space(((ui.available_width() - 3.0 * width) / 2.0).max(0.0));
            for (tab, name) in [(Tab::Video, "Video"), (Tab::Audio, "Audio"), (Tab::File, "File")] {
                if ui.add_sized([width, 24.0], egui::Button::selectable(self.tab == tab, name)).clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.add_space(6.0);

        let dest = self.dest(project_file);
        let ready = dest.is_some() && board.is_some_and(|board| !board.clips().is_empty());
        // The button stays at the foot, under whatever the tab holds.
        egui::Panel::bottom("add to queue").frame(egui::Frame::NONE).show(ui, |ui| {
            ui.add_space(6.0);
            let count = match board {
                None => "no storyboard".to_string(),
                Some(_) if self.film() => "the storyboard as one movie".to_string(),
                Some(_) if self.take == Take::Current => "the shot in the viewer".to_string(),
                Some(board) if self.selected.is_empty() => format!("all {} shots", board.clips().len()),
                Some(_) => format!("{} selected", if self.selected.len() == 1 { "1 shot".to_string() } else { format!("{} shots", self.selected.len()) }),
            };
            if ui.add_enabled(ready, egui::Button::new("Add to Render Queue")).on_hover_text(format!("Makes a job of {count}, with these settings")).clicked() {
                actions.push(Action::Add);
            }
            ui.weak(count);
        });
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match self.tab {
            Tab::Video => self.video_tab(ui, project),
            Tab::Audio => self.audio_tab(ui),
            Tab::File => self.file_tab(ui, project, dest.as_deref(), board, ready, &mut actions),
        });
        actions
    }

    /// The picture: the file's format, how it is encoded, its size, and which frames.
    fn video_tab(&mut self, ui: &mut egui::Ui, project: &Project) {
        let (settings, own_size) = (&mut self.settings, &mut self.own_size);
        let format_name = |format: Format| match format {
            Format::Mov => "QuickTime",
            Format::Mp4 => "MP4",
            Format::Png => "PNG sequence",
            Format::Jpeg => "JPEG sequence",
            Format::Tiff => "TIFF sequence",
            Format::Exr => "EXR sequence",
        };
        let (wide, tall) = (project.settings.width, project.settings.height);
        form::row(ui, "Format", |ui| {
            egui::ComboBox::from_id_salt("format").selected_text(format_name(settings.format)).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                for format in [Format::Mov, Format::Mp4, Format::Png, Format::Jpeg, Format::Tiff, Format::Exr] {
                    ui.selectable_value(&mut settings.format, format, format_name(format));
                }
            });
        });
        *settings = settings.fitted();

        form::row(ui, "Codec", |ui| {
            let codec_name = |codec: Codec| if codec == Codec::ProRes { "Apple ProRes" } else { "H.264" };
            match settings.format {
                Format::Mov => {
                    egui::ComboBox::from_id_salt("codec").selected_text(codec_name(settings.codec)).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                        for codec in [Codec::ProRes, Codec::H264] {
                            ui.selectable_value(&mut settings.codec, codec, codec_name(codec));
                        }
                    });
                }
                // One it can be: shown, with nothing to choose.
                format => {
                    let only = if format == Format::Mp4 { "H.264" } else { format.extension() };
                    ui.add_enabled_ui(false, |ui| {
                        egui::ComboBox::from_id_salt("codec").selected_text(only.to_uppercase()).width(form::FIELD_WIDTH).show_ui(ui, |_| {})
                    });
                }
            }
        });
        ui.add_space(4.0);
        ui.separator();

        form::row(ui, "Resolution", |ui| {
            let named = [("HD", [1280, 720]), ("Full HD", [1920, 1080]), ("UHD", [3840, 2160])];
            let percent = |scale: f64| {
                let side = |pixels: u32| (f64::from(pixels) * scale).round().max(1.0) as u32;
                if scale == 1.0 { format!("Shot size, {wide} × {tall}") } else { format!("{:.0}%, {} × {}", scale * 100.0, side(wide), side(tall)) }
            };
            // A size that is none of the named ones is one of the user's own.
            *own_size = settings.size.is_some_and(|size| *own_size || !named.iter().any(|(_, of)| *of == size));
            let shown = match settings.size {
                None => percent(settings.scale),
                Some(size) => match named.iter().find(|(_, of)| *of == size).filter(|_| !*own_size) {
                    Some((name, [width, height])) => format!("{name}, {width} × {height}"),
                    None => format!("Custom, {} × {}", size[0], size[1]),
                },
            };
            egui::ComboBox::from_id_salt("resolution").selected_text(shown).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                for scale in [1.0, 0.5, 0.25] {
                    if ui.selectable_label(settings.size.is_none() && settings.scale == scale, percent(scale)).clicked() {
                        (settings.size, settings.scale, *own_size) = (None, scale, false);
                    }
                }
                ui.separator();
                for (name, size) in named {
                    let is = !*own_size && settings.size == Some(size);
                    if ui
                        .selectable_label(is, format!("{name}, {} × {}", size[0], size[1]))
                        .on_hover_text("The shot is fitted into it whole, with the matte at the sides it does not reach")
                        .clicked()
                    {
                        (settings.size, *own_size) = (Some(size), false);
                    }
                }
                ui.separator();
                if ui.selectable_label(*own_size, "Custom…").on_hover_text("A size in pixels of your own: the shot is fitted into it whole").clicked() {
                    // It starts from the size the picture has now.
                    let side = |pixels: u32| ((f64::from(pixels) * settings.scale).round() as u32).clamp(2, 16_384);
                    (settings.size, *own_size) = (Some(settings.size.unwrap_or([side(wide), side(tall)])), true);
                }
            });
        });
        if let Some(size) = settings.size.as_mut().filter(|_| *own_size) {
            form::row(ui, "", |ui| {
                ui.add(egui::DragValue::new(&mut size[0]).range(2..=16_384).suffix(" px"));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut size[1]).range(2..=16_384).suffix(" px"));
            });
        }

        form::row(ui, "Frame rate", |ui| {
            // The project's, or another: then each frame is the project's on screen at its
            // moment, repeated or left out, and the sound is as long as the picture.
            let fps = |rate: iskra_model::FrameRate| {
                let fps = rate.as_f64();
                if fps.fract() == 0.0 { format!("{fps:.0} fps") } else { format!("{fps:.3} fps") }
            };
            let own = project.settings.frame_rate;
            let shown = settings.frame_rate.map_or_else(|| format!("{}, the project's", fps(own)), fps);
            egui::ComboBox::from_id_salt("frame rate").selected_text(shown).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                ui.selectable_value(&mut settings.frame_rate, None, format!("{}, the project's", fps(own)));
                ui.separator();
                for rate in iskra_model::FrameRate::COMMON.into_iter().filter(|rate| *rate != own) {
                    ui.selectable_value(&mut settings.frame_rate, Some(rate), fps(rate)).on_hover_text(
                        "Each frame shows the project's that is on screen at its moment: frames are repeated to go faster, left out to go slower",
                    );
                }
            });
        });
        ui.add_space(4.0);
        ui.separator();

        form::row(ui, "Type", |ui| {
            if !settings.format.is_movie() {
                let said = |bits: u8| if bits == 32 { "32 bits, floats".to_string() } else { format!("{bits} bits a channel") };
                let depths = settings.format.depths();
                ui.add_enabled_ui(depths.len() > 1, |ui| {
                    egui::ComboBox::from_id_salt("type").selected_text(said(settings.bits)).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                        for bits in depths {
                            ui.selectable_value(&mut settings.bits, *bits, said(*bits));
                        }
                    });
                });
            } else if settings.codec == Codec::ProRes {
                egui::ComboBox::from_id_salt("type").selected_text(settings.prores.label()).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                    for kind in ProRes::ALL {
                        let hint = if kind.has_alpha() { "Colour at every pixel. Holds transparency" } else { "No transparency: the picture is on the matte" };
                        ui.selectable_value(&mut settings.prores, kind, kind.label()).on_hover_text(hint);
                    }
                });
            } else {
                ui.add_enabled_ui(settings.video_kbps.is_none(), |ui| {
                    egui::ComboBox::from_id_salt("type")
                        .selected_text(format!("{} quality", settings.quality.label()))
                        .width(form::FIELD_WIDTH)
                        .show_ui(ui, |ui| {
                            for quality in Quality::ALL {
                                ui.selectable_value(&mut settings.quality, quality, format!("{} quality", quality.label()))
                                    .on_hover_text("The better the picture, the larger the file");
                            }
                        })
                        .response
                        .on_disabled_hover_text("Held to a data rate, the picture is as good as the rate lets it be");
                });
            }
        });
        // H.264 can be held to a data rate, in place of a quality.
        if settings.format.is_movie() && settings.codec == Codec::H264 {
            form::row(ui, "Data rate", |ui| {
                let mut held = settings.video_kbps.is_some();
                ui.checkbox(&mut held, "Restrict to").on_hover_text("Hold the picture to this many Kb/s, whatever it shows. Off: the quality above decides");
                let mut kbps = settings.video_kbps.unwrap_or(8000);
                ui.add_enabled(held, egui::DragValue::new(&mut kbps).range(100..=200_000).suffix(" Kb/s")).on_hover_text("Type a number, or drag");
                settings.video_kbps = held.then_some(kbps);
            });
        }
        ui.add_space(4.0);
        ui.separator();

        form::row(ui, "", |ui| {
            ui.add_enabled(settings.can_alpha(), egui::Checkbox::new(&mut settings.alpha, "With transparency"))
                .on_hover_text("Clear where nothing is painted")
                .on_disabled_hover_text(
                    "This format has no transparency: the picture is on the matte. ProRes 4444 and the PNG, TIFF and EXR sequences have it",
                );
        });
        if !settings.alpha {
            form::row(ui, "Matte", |ui| {
                ui.color_edit_button_srgb(&mut settings.matte).on_hover_text("The colour under the picture where nothing is painted: the paper");
            });
        }
        ui.add_space(4.0);
        ui.separator();

        form::row(ui, "Frames", |ui| {
            ui.vertical(|ui| {
                let mut cut = matches!(settings.range, Range::Cut { .. });
                ui.radio_value(&mut cut, true, "Each shot's part in the cut")
                    .on_hover_text("From its in point to its out point, with some of its handles if wanted");
                if let (true, Range::Cut { handles }) = (cut, &mut settings.range) {
                    ui.indent("handles", |ui| {
                        ui.add(egui::DragValue::new(handles).range(0..=240).prefix("with ").suffix(" frames of handles")).on_hover_text(
                            "Frames before the in point and after the out point, as far as the shot has them: room for the editor to slide a cut",
                        );
                    });
                }
                ui.radio_value(&mut cut, false, "Each shot whole").on_hover_text("All of its frames, handles and all");
                settings.range = match (cut, settings.range) {
                    (true, Range::Whole) => Range::Cut { handles: 0 },
                    (false, _) => Range::Whole,
                    (true, range) => range,
                };
            });
        });
    }

    /// The sound of a movie: whether it has any, and how it is encoded.
    fn audio_tab(&mut self, ui: &mut egui::Ui) {
        let settings = &mut self.settings;
        let depth = |bits: u16| if bits == 32 { "32-bit float".to_string() } else { format!("{bits}-bit") };
        if !settings.format.is_movie() {
            // A sequence's sound is a WAV file beside its frames, uncompressed.
            ui.checkbox(&mut settings.sound_beside, "A WAV file beside the frames")
                .on_hover_text("The stretch of the storyboard's sound tracks under what is rendered, in the sequence's folder, named as its frames are");
            ui.add_space(4.0);
            ui.separator();
            ui.add_enabled_ui(settings.sound_beside, |ui| {
                form::row(ui, "Bit depth", |ui| {
                    egui::ComboBox::from_id_salt("wav bits").selected_text(depth(settings.sound_bits)).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                        for bits in iskra_export::SOUND_BITS {
                            ui.selectable_value(&mut settings.sound_bits, bits, depth(bits));
                        }
                    });
                });
                form::row(ui, "Sample rate", |ui| {
                    egui::ComboBox::from_id_salt("wav rate").selected_text(format!("{} Hz", settings.sound_rate)).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                        for rate in iskra_export::SOUND_RATES {
                            ui.selectable_value(&mut settings.sound_rate, rate, format!("{rate} Hz"));
                        }
                    });
                });
            });
            *settings = settings.fitted();
            return;
        }
        ui.checkbox(&mut settings.sound, "With the sound under it").on_hover_text("The stretch of the storyboard's sound tracks under what is rendered");
        ui.add_space(4.0);
        ui.separator();
        ui.add_enabled_ui(settings.sound, |ui| {
            form::row(ui, "Codec", |ui| {
                let name = |codec: SoundCodec| if codec == SoundCodec::Pcm { "Linear PCM" } else { "AAC" };
                ui.add_enabled_ui(settings.format == Format::Mov, |ui| {
                    egui::ComboBox::from_id_salt("sound codec")
                        .selected_text(name(settings.sound_codec))
                        .width(form::FIELD_WIDTH)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut settings.sound_codec, SoundCodec::Pcm, name(SoundCodec::Pcm)).on_hover_text("Uncompressed: for post");
                            ui.selectable_value(&mut settings.sound_codec, SoundCodec::Aac, name(SoundCodec::Aac)).on_hover_text("Small: to send and to watch");
                        })
                        .response
                        .on_disabled_hover_text("An MP4 file has AAC sound");
                });
            });
            form::row(ui, "Bit depth", |ui| {
                ui.add_enabled_ui(settings.sound_codec == SoundCodec::Pcm, |ui| {
                    egui::ComboBox::from_id_salt("sound bits").selected_text(depth(settings.sound_bits)).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                        for bits in iskra_export::SOUND_BITS {
                            ui.selectable_value(&mut settings.sound_bits, bits, depth(bits));
                        }
                    })
                })
                .response
                .on_disabled_hover_text("AAC has no bit depth of its own: it is compressed");
            });
            form::row(ui, "Track data rate", |ui| {
                ui.add_enabled(settings.sound_codec == SoundCodec::Aac, egui::DragValue::new(&mut settings.sound_kbps).range(64..=512).suffix(" Kb/s"))
                    .on_hover_text("For both channels together. Type a number, or drag")
                    .on_disabled_hover_text("Uncompressed sound has the rate its samples make");
            });
            form::row(ui, "Sample rate", |ui| {
                egui::ComboBox::from_id_salt("sound rate").selected_text(format!("{} Hz", settings.sound_rate)).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                    for rate in iskra_export::SOUND_RATES {
                        ui.selectable_value(&mut settings.sound_rate, rate, format!("{rate} Hz"));
                    }
                });
            });
        });
        *settings = settings.fitted();
    }

    /// The file: what it is called, and what comes after its name and before it.
    fn file_tab(&mut self, ui: &mut egui::Ui, project: &Project, dest: Option<&Path>, board: Option<&Storyboard>, ready: bool, actions: &mut Vec<Action>) {
        let named = &mut self.file_naming;
        form::row(ui, "Filename uses", |ui| {
            ui.vertical(|ui| {
                ui.radio_value(&mut named.from, NameFrom::Own, "Shot name")
                    .on_hover_text("Each file is named for its shot: Shot_01. The storyboard as one movie is named for the storyboard");
                ui.radio_value(&mut named.from, NameFrom::Storyboard, "Storyboard name")
                    .on_hover_text("The storyboard's name, and with several shots each shot's after it");
                ui.radio_value(&mut named.from, NameFrom::Custom, "Custom name")
                    .on_hover_text("A name of your own, and with several shots each shot's after it");
                ui.radio_value(&mut named.from, NameFrom::Parts, "Parts").on_hover_text("Made of the names picked below, with _ between them");
                if named.from == NameFrom::Parts {
                    ui.indent("name parts", |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let [project, board, shot] = &mut named.parts;
                            let hint = "In the name, in this order";
                            ui.checkbox(project, "Project").on_hover_text(hint);
                            ui.checkbox(board, "Storyboard").on_hover_text(hint);
                            let hint = "The shot's, given in its Properties: left out of the name of a shot in none";
                            ui.checkbox(&mut named.sequence, "Sequence").on_hover_text(hint);
                            ui.checkbox(&mut named.scene, "Scene").on_hover_text(hint);
                            ui.checkbox(shot, "Shot").on_hover_text("Always there where several shots are rendered, so no two are named alike");
                        });
                        ui.weak("A pass's name comes after them, in its own outputs");
                    });
                }
            });
        });
        ui.add_space(4.0);
        ui.separator();
        form::row(ui, "Custom name", |ui| {
            ui.add_enabled(named.from == NameFrom::Custom, egui::TextEdit::singleline(&mut named.custom).hint_text("Film").desired_width(form::FIELD_WIDTH))
                .on_disabled_hover_text("Choose Custom name above to type one");
        });
        form::row(ui, "File suffix", |ui| {
            ui.add(egui::TextEdit::singleline(&mut named.suffix).hint_text("_v2").desired_width(form::FIELD_WIDTH))
                .on_hover_text("Comes straight after the name");
        });
        form::row(ui, "File subfolder", |ui| {
            ui.add(egui::TextEdit::singleline(&mut named.subfolder).hint_text("dailies").desired_width(form::FIELD_WIDTH))
                .on_hover_text("A folder under the location, made if it is not there");
        });
        form::row(ui, "Frame numbers", |ui| {
            ui.vertical(|ui| {
                ui.add(egui::DragValue::new(&mut named.digits).range(3..=8).suffix(" digits"))
                    .on_hover_text("How many digits a frame's number has in a sequence's files");
                ui.checkbox(&mut named.film_numbers, "As in the film").on_hover_text(
                    "A frame is numbered as it is on the storyboard's timeline, so that the sequences of the shots go on from one another; else each from 1",
                );
            });
        });
        ui.add_space(6.0);
        // What would be written, for the storyboard's first shot.
        match (dest, board) {
            (Some(dest), Some(board)) => {
                let first = board.clips().first().and_then(|clip| project.shot(clip.shot));
                let names = first.map_or(ShotNames { name: "Shot_01", ..ShotNames::default() }, ShotNames::from);
                let (settings, shot, pass) = (self.settings.fitted(), names.name, "Background");
                let several = self.take == Take::Shots && (board.clips().len() > 1 || self.selected.len() > 1);
                let naming = self.naming_of(board, (!self.film()).then_some(names), several);
                let (to, name) = (naming.dest(dest), naming.named(if self.film() { &board.name } else { shot }));
                let example = if self.film() {
                    iskra_export::movie_file(&to, &name, 1, settings.format)
                } else if self.apart && settings.format.is_movie() {
                    iskra_export::movie_file(&to.join(pass), &format!("{name}_{pass}"), 1, settings.format)
                } else if self.apart {
                    naming.frame_path(&to.join(&name).join(pass), &format!("{name}_{pass}"), 1, settings.format)
                } else if settings.format.is_movie() {
                    iskra_export::movie_file(&to, &name, 1, settings.format)
                } else {
                    naming.frame_path(&iskra_export::sequence_folder(&to, &name, 1), &name, 1, settings.format)
                };
                let shown = example.strip_prefix(dest).map_or_else(|_| example.display().to_string(), |inside| inside.display().to_string());
                form::row(ui, "", |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(shown).monospace().size(11.0)).wrap())
                        .on_hover_text("What a render writes under the location: a sequence in a folder of its own, movies together");
                });
            }
            _ => drop(ui.weak("The project has no file yet: choose a location, or save the project and its Renders folder is used.")),
        }
        ui.add_space(4.0);
        ui.separator();
        form::row(ui, "For the edit", |ui| {
            ui.add_enabled_ui(ready, |ui| {
                ui.menu_button("Write Edit List", |ui| {
                    let kinds = [
                        (iskra_export::ListFormat::Otio, "OpenTimelineIO (.otio)", "The cut, its transitions and its sound. Resolve and others open it"),
                        (iskra_export::ListFormat::Edl, "EDL (.edl)", "The oldest and plainest: cuts, dissolves and sound, read by every editor"),
                        (iskra_export::ListFormat::Fcpxml, "FCPXML (.fcpxml)", "The picture: cuts, gaps and transitions"),
                    ];
                    for (kind, name, hint) in kinds {
                        if ui.button(name).on_hover_text(hint).clicked() {
                            actions.push(Action::EditList(kind));
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("The storyboard's cut as a file an editing program opens: the rendered shots where they lie, with the dissolves and the sound");
            });
        });
        if let Some(said) = &self.said {
            ui.add(egui::Label::new(egui::RichText::new(said).weak().size(11.0)).wrap());
        }
    }

    /// How high the strip of shots is with that many rows of passes under it.
    pub fn strip_height(rows: usize) -> f32 {
        96.0 + rows as f32 * (PASS_ROW + 2.0)
    }

    /// The strip of the storyboard's shots, under the viewer: what is selected here is what a
    /// job is made of. Under the shots are the passes (plan §7.14), a row each, and what is in
    /// no pass: a cell where a shot has something of it, which a click switches for that
    /// shot, as a click on the row's name does for all of them.
    pub fn shots_strip(&mut self, ui: &mut egui::Ui, project: &Project, board: Option<&Storyboard>, showing: Option<ShotId>) -> Vec<Action> {
        let mut actions = Vec::new();
        let theme = Theme::of(ui.ctx());
        let Some(board) = board else {
            ui.weak("The project has no storyboard to render.");
            return actions;
        };
        ui.horizontal(|ui| {
            ui.strong(&board.name);
            if ui.small_button("All").on_hover_text("Render every shot: nothing selected is all of them").clicked() {
                self.selected.clear();
            }
            if ui.small_button("Changed").on_hover_text("Select the shots that were changed since they were rendered, and those never rendered").clicked() {
                self.selected = board.clips().iter().map(|clip| clip.shot).filter(|shot| rendered(project, *shot) != Some(true)).collect();
            }
            // The field a pass's new name is typed into. Enter takes it, Escape gives it up.
            if let Some((from, text, focus)) = self.renaming.as_mut() {
                ui.label(format!("Rename {from} in every shot:"));
                let field = ui.add(egui::TextEdit::singleline(text).desired_width(160.0));
                if std::mem::take(focus) {
                    field.request_focus();
                }
                if field.lost_focus() {
                    let keep = !ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if let Some((from, text, _)) = self.renaming.take().filter(|(from, text, _)| keep && !text.trim().is_empty() && text.trim() != from) {
                        actions.push(Action::RenamePass(from, text.trim().to_string()));
                    }
                }
            } else {
                ui.weak("Click a shot to select it for the next job; with none selected, all are taken.");
            }
        });
        let rows = Self::pass_rows(project, board);
        self.apart = self.renders_passes(board.clips().iter().filter_map(|clip| project.shot(clip.shot)));
        // The storyboard as one file is a movie of each row that is on: its shots are not
        // switched one by one.
        let film = self.film();
        let live = !film;
        #[cfg(test)]
        {
            self.cells.clear();
            self.cell_states.clear();
        }
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            // The rows' names, beside the cells: a click switches the row for every shot.
            ui.vertical(|ui| {
                ui.allocate_exact_size(egui::vec2(PASS_NAMES, 52.0), egui::Sense::hover());
                for row in &rows {
                    let (rect, response) = ui.allocate_exact_size(egui::vec2(PASS_NAMES, PASS_ROW), egui::Sense::click());
                    let on = self.row_on(row);
                    let dot = egui::pos2(rect.left() + 8.0, rect.center().y);
                    if on {
                        ui.painter().circle_filled(dot, 4.0, theme.accent);
                    } else {
                        ui.painter().circle_stroke(dot, 3.5, egui::Stroke::new(1.0, theme.text_muted));
                    }
                    ui.painter().with_clip_rect(rect).text(
                        egui::pos2(rect.left() + 18.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        row.as_deref().unwrap_or(FINAL),
                        egui::FontId::proportional(11.0),
                        if on { theme.text } else { theme.text_muted },
                    );
                    if response.clicked() {
                        self.switch_row(row);
                    }
                    if let Some(name) = row {
                        response.context_menu(|ui| {
                            if ui.button("Rename in Every Shot…").on_hover_text("Passes are matched by their names: all of this name are renamed").clicked() {
                                self.renaming = Some((name.clone(), name.clone(), true));
                                ui.close();
                            }
                        });
                    }
                    response.on_hover_text(match (film, row) {
                        (true, Some(_)) => "A Pass of the shots. Click to render the storyboard as one movie of it too: each shot through it, a shot without it whole",
                        (true, None) => "The storyboard's whole picture, as one movie. Beside passes, a movie of its own",
                        (false, Some(_)) => {
                            "A Pass of the shots, from the node view. Click to render it, as an output of its own, for every shot that has it. \
                             It is what is wired into it alone: what covers it in Final, or what its layers blend over, is not in it unless that is wired in too"
                        }
                        (false, None) => "Each shot's whole picture, its main display. Beside passes, an output of its own; a shot with none of the passes that are on is rendered whole",
                    });
                }
            });
            egui::ScrollArea::horizontal().auto_shrink(false).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for clip in board.clips() {
                        let Some(shot) = project.shot(clip.shot) else { continue };
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            actions.extend(self.shot_tile(ui, project, shot, clip.length(), showing));
                            for row in &rows {
                                let (rect, response) = ui.allocate_exact_size(egui::vec2(96.0, PASS_ROW), egui::Sense::click());
                                let has = match row {
                                    Some(name) => shot.pass_named(name).is_some(),
                                    None => true,
                                };
                                if !has {
                                    // Nothing of the shot is in it: a hairline, and no switch.
                                    ui.painter().hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, theme.border));
                                    continue;
                                }
                                #[cfg(test)]
                                self.cells.push((shot.id, row.clone(), rect));
                                let on = live && self.is_on(shot.id, row);
                                let bar = rect.shrink2(egui::vec2(0.0, 2.0));
                                if on {
                                    ui.painter().rect_filled(bar, 3.0, theme.accent.gamma_multiply(if row.is_some() { 0.85 } else { 0.5 }));
                                } else {
                                    ui.painter().rect_stroke(bar, 3.0, egui::Stroke::new(1.0, theme.border), egui::StrokeKind::Inside);
                                }
                                // What the records say of a pass: rendered and as it is, or
                                // changed since. The shot's own state is on its tile.
                                let state = row.as_ref().and_then(|_| rendered_as(project, shot.id, row));
                                #[cfg(test)]
                                if row.is_some() {
                                    self.cell_states.push((shot.id, row.clone(), state));
                                }
                                if let Some(fresh) = state {
                                    let dot = egui::pos2(bar.right() - 6.0, bar.center().y);
                                    ui.painter().circle_filled(dot, 3.0, if fresh { theme.text } else { stale_colour(ui.ctx()) });
                                }
                                if live && response.clicked() {
                                    self.switch_cell(shot.id, row);
                                }
                                let name = row.as_deref().unwrap_or("the whole picture");
                                let records = match state {
                                    Some(true) => " It was rendered, and is as it was.",
                                    Some(false) => " It was rendered, and changed since.",
                                    None if row.is_some() => " It was never rendered.",
                                    None => "",
                                };
                                response.on_hover_text(format!(
                                    "{}: {name} is {}.{records} Click to switch it for this shot",
                                    shot.name,
                                    if on { "rendered" } else { "left out" }
                                ));
                            }
                        });
                    }
                });
            });
        });
        // What a pass can't carry, said where the shots have passes: always, so that the strip
        // keeps its height as they are switched.
        if rows.len() > 1 {
            ui.label(
                egui::RichText::new(
                    "A pass is what is wired into its node alone: a tree of the background in front of a character, or what the character's layers blend over, \
                     is not in the Character pass unless it is wired in too. Put the pass after a composite that has both, or comp them in that order.",
                )
                .weak()
                .size(11.0),
            );
        }
        actions
    }

    /// A shot of the strip: its name, its status, and what the records say of its renders.
    fn shot_tile(&mut self, ui: &mut egui::Ui, project: &Project, shot: &iskra_model::Shot, length: u32, showing: Option<ShotId>) -> Vec<Action> {
        let mut actions = Vec::new();
        let theme = Theme::of(ui.ctx());
        let selected = self.selected.contains(&shot.id);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(96.0, 52.0), egui::Sense::click());
        let outline = if showing == Some(shot.id) { theme.accent } else { theme.border };
        ui.painter().rect(rect, 4.0, if selected { theme.selection } else { theme.surface_raised }, egui::Stroke::new(1.0, outline), egui::StrokeKind::Inside);
        let text = ui.painter().with_clip_rect(rect);
        text.text(rect.left_top() + egui::vec2(6.0, 4.0), egui::Align2::LEFT_TOP, &shot.name, egui::FontId::proportional(12.0), theme.text);
        text.circle_filled(rect.right_top() + egui::vec2(-9.0, 9.0), 4.0, status_color(ui.ctx(), shot.status));
        // What the project's records say of it: rendered, changed since, or never.
        let (state, colour) = match rendered(project, shot.id) {
            Some(true) => ("rendered", theme.text_muted),
            Some(false) => ("changed since", stale_colour(ui.ctx())),
            None => ("not rendered", theme.text_muted),
        };
        text.text(
            rect.left_bottom() + egui::vec2(6.0, -4.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{length} fr · {state}"),
            egui::FontId::proportional(10.0),
            colour,
        );
        if response.clicked() {
            match self.selected.iter().position(|id| *id == shot.id) {
                Some(at) => drop(self.selected.remove(at)),
                None => self.selected.push(shot.id),
            }
            actions.push(Action::Show(shot.id));
        }
        response.on_hover_text(format!("{}: {}", shot.name, Status::label(shot.status)));
        actions
    }

    /// The render queue: the jobs with their parts, and what starts and stops them.
    pub fn queue_panel(&mut self, ui: &mut egui::Ui, project: &Project) -> Vec<Action> {
        let mut actions = Vec::new();
        let theme = Theme::of(ui.ctx());
        // The boxes: the jobs, then the renders the project knows of that no job here made,
        // the newest first. They are numbered by their place.
        let made: Vec<&Path> = self
            .jobs
            .iter()
            .flat_map(|job| &job.parts)
            .filter_map(|part| if let State::Done { at, .. } = &part.state { Some(at.as_path()) } else { None })
            .collect();
        let records: Vec<usize> = (0..project.renders.len()).rev().filter(|at| !made.contains(&project.renders[*at].at.as_path())).collect();
        let boxes: Vec<Boxed> = self.jobs.iter().map(|job| Boxed::Job(job.id)).chain(records.iter().map(|at| Boxed::Record(*at))).collect();
        self.picked.retain(|picked| boxes.contains(picked));
        let number = |of: &Boxed| boxes.iter().position(|other| other == of).map_or(0, |at| at + 1);

        ui.strong("Render Queue");
        ui.add_space(4.0);
        #[cfg(test)]
        self.queue_boxes.clear();
        ui.horizontal(|ui| {
            // None picked, or all of them: the whole queue. Else the boxes picked, by name.
            if self.picked.is_empty() || self.picked.len() == boxes.len() {
                let waiting = self.next_waiting().is_some() || self.only.is_some();
                #[cfg(test)]
                {
                    self.render_said = "Render All".to_string();
                }
                if ui
                    .add_enabled(waiting && !self.rendering, egui::Button::new("Render All"))
                    .on_hover_text("Renders every part that waits, one after another, while you work on")
                    .clicked()
                {
                    actions.push(Action::RenderAll);
                }
            } else {
                let mut picked = self.picked.clone();
                picked.sort_by_key(number);
                let names: Vec<String> = picked.iter().map(|of| format!("Render {}", number(of))).collect();
                let hint = "Renders the boxes picked alone. One that is rendered already is rendered again, over its files";
                #[cfg(test)]
                {
                    self.render_said = names.join(", ");
                }
                if ui.add_enabled(!self.rendering, egui::Button::new(names.join(", ")).truncate()).on_hover_text(hint).clicked() {
                    actions.push(Action::Render(picked));
                }
            }
            if ui.add_enabled(self.rendering, egui::Button::new("Stop")).on_hover_text("Ends after the frames in hand. What waits stays in the queue").clicked()
            {
                actions.push(Action::Stop);
            }
        });
        ui.separator();
        if boxes.is_empty() {
            ui.weak("No jobs yet. Choose the settings on the left, select shots under the viewer, and add them to the queue.");
        }
        let stale_at = |at: &Path| project.renders.iter().find(|record| record.at == at).is_some_and(|record| iskra_export::is_stale(project, record));
        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for job in &self.jobs {
                let of = Boxed::Job(job.id);
                let running = self.running.as_ref().filter(|running| running.job == job.id);
                let states = || job.parts.iter().map(|part| &part.state);
                let state = if states().any(|state| *state == State::Rendering) {
                    ("Rendering".to_string(), theme.accent)
                } else if states().any(|state| *state == State::Waiting) {
                    ("Waiting".to_string(), theme.text_muted)
                } else if states().any(|state| matches!(state, State::Failed(_))) {
                    ("Failed".to_string(), theme.danger)
                } else if states().any(|state| *state == State::Stopped) {
                    ("Stopped".to_string(), theme.text_muted)
                } else if states().any(|state| matches!(state, State::Done { at, .. } if stale_at(at))) {
                    ("Changed since".to_string(), stale_colour(ui.ctx()))
                } else {
                    ("Rendered".to_string(), RENDERED)
                };
                let icon = if job.settings.format.is_movie() { "film" } else { "image" };
                let (boxed, cross) = queue_box(ui, number(&of), self.picked.contains(&of), state, running.is_none(), icon, |ui| {
                    for (at, part) in job.parts.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(egui::RichText::new(&part.name).strong()).truncate());
                            match &part.state {
                                State::Waiting => drop(ui.weak("waiting")),
                                State::Rendering => {
                                    let (done, total) = running
                                        .filter(|running| running.part == at)
                                        .map_or((0, 0), |running| (running.progress.0.load(Ordering::Relaxed), running.progress.1.load(Ordering::Relaxed)));
                                    let part_done = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                                    ui.add(egui::ProgressBar::new(part_done).desired_width(90.0).text(format!("{done} / {total}")));
                                }
                                State::Done { files, at } => {
                                    if stale_at(at) {
                                        ui.colored_label(stale_colour(ui.ctx()), "changed since")
                                            .on_hover_text("The shot, the palette, the cut or the sound under it is no longer as it was rendered");
                                    } else {
                                        ui.weak(if *files == 1 { "done".to_string() } else { format!("done, {files} files") });
                                    }
                                    if ui.small_button("Show").on_hover_text("Show the files in the system's file browser").clicked() {
                                        actions.push(Action::Reveal(at.clone()));
                                    }
                                }
                                State::Skipped => drop(ui.weak("there already, left")),
                                State::Stopped => drop(ui.weak("stopped")),
                                State::Failed(why) => drop(ui.colored_label(theme.danger, "failed").on_hover_text(why)),
                            }
                        });
                    }
                    ui.weak(&job.name);
                    ui.add(egui::Label::new(egui::RichText::new(job.dest.display().to_string()).weak().size(10.0)).truncate());
                });
                #[cfg(test)]
                self.queue_boxes.push((of, boxed.rect, cross.rect));
                if cross.clicked() {
                    actions.push(Action::Remove(job.id));
                } else if boxed.clicked() {
                    clicked = Some(of);
                }
            }
            // What the project knows was rendered, and whether it was changed since: kept
            // with the project, so it is here tomorrow too.
            for at in &records {
                let (of, record) = (Boxed::Record(*at), &project.renders[*at]);
                let (name, film) = match record.what {
                    iskra_model::Rendered::Shot(shot) => (project.shot(shot).map_or("a shot that is gone", |shot| shot.name.as_str()), false),
                    iskra_model::Rendered::Film(board) => (project.storyboard(board).map_or("a storyboard that is gone", |board| board.name.as_str()), true),
                };
                let settings = iskra_export::settings_of(record);
                let kind = settings.as_ref().map_or("?".to_string(), |settings| said(settings, film));
                let stale = iskra_export::is_stale(project, record);
                let state = if stale { ("Changed since".to_string(), stale_colour(ui.ctx())) } else { ("Rendered".to_string(), RENDERED) };
                let icon = if settings.is_some_and(|settings| !settings.format.is_movie()) { "image" } else { "film" };
                let (boxed, cross) = queue_box(ui, number(&of), self.picked.contains(&of), state, true, icon, |ui| {
                    let title = match &record.pass {
                        Some(pass) => format!("{name} · {pass}"),
                        None => name.to_string(),
                    };
                    ui.add(egui::Label::new(egui::RichText::new(title).strong()).truncate());
                    ui.weak(kind);
                    ui.add(egui::Label::new(egui::RichText::new(record.at.display().to_string()).weak().size(10.0)).truncate());
                    ui.horizontal(|ui| {
                        if ui.small_button("Show").on_hover_text(record.at.display().to_string()).clicked() {
                            actions.push(Action::Reveal(record.at.clone()));
                        }
                        if stale
                            && ui.small_button("Again").on_hover_text("Render it again as it was rendered, to the same place, over what is there").clicked()
                        {
                            actions.push(Action::Again(*at));
                        }
                    });
                });
                #[cfg(test)]
                self.queue_boxes.push((of, boxed.rect, cross.rect));
                if cross.clicked() {
                    actions.push(Action::Forget(*at));
                } else if boxed.clicked() {
                    clicked = Some(of);
                }
            }
        });
        // A click picks the box alone, or lets it go if it was; with Shift or the command
        // key it joins the boxes picked, or leaves them.
        if let Some(of) = clicked {
            let (shift, command) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
            let had = self.picked.contains(&of);
            if shift || command {
                if had {
                    self.picked.retain(|picked| *picked != of);
                } else {
                    self.picked.push(of);
                }
            } else {
                self.picked = if had && self.picked.len() == 1 { Vec::new() } else { vec![of] };
            }
        }
        actions
    }
}

/// The colour a box of the queue says it is rendered in.
const RENDERED: egui::Color32 = egui::Color32::from_rgb(96, 176, 96);

/// A box of the render queue: its name and state over what it renders, a picture of what kind
/// of files those are, and a cross that takes it away. The box, and its cross.
fn queue_box(
    ui: &mut egui::Ui,
    number: usize,
    picked: bool,
    (state, colour): (String, egui::Color32),
    removable: bool,
    icon: &str,
    body: impl FnOnce(&mut egui::Ui),
) -> (egui::Response, egui::Response) {
    let theme = Theme::of(ui.ctx());
    let mut remove = None;
    let edge = if picked { egui::Stroke::new(1.5, theme.accent) } else { egui::Stroke::new(1.0, theme.border) };
    let boxed = ui.scope_builder(egui::UiBuilder::new().id_salt(("queue box", number)).sense(egui::Sense::click()), |ui| {
        // Its words are not selected as text: a click on them picks the box.
        ui.style_mut().interaction.selectable_labels = false;
        egui::Frame::NONE.fill(theme.surface_raised).stroke(edge).corner_radius(4.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Its name, its state, and the cross at the right.
            egui::Frame::NONE.fill(if picked { theme.selection } else { theme.surface }).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(format!("Render {number}"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // The cross is drawn: the font has no glyph of one.
                        let sense = if removable { egui::Sense::click() } else { egui::Sense::hover() };
                        let (rect, cross) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), sense);
                        let ink = match (removable, cross.hovered()) {
                            (false, _) => theme.text_muted.gamma_multiply(0.5),
                            (true, true) => theme.text,
                            (true, false) => theme.text_muted,
                        };
                        let arm = rect.shrink(4.0);
                        for (from, to) in [(arm.left_top(), arm.right_bottom()), (arm.right_top(), arm.left_bottom())] {
                            ui.painter().line_segment([from, to], egui::Stroke::new(1.5, ink));
                        }
                        let cross = if removable {
                            cross.on_hover_text("Take it out of the queue. What it rendered stays")
                        } else {
                            cross.on_hover_text("It is being rendered: stop it first")
                        };
                        remove = Some(cross);
                        ui.add_space(6.0);
                        ui.colored_label(colour, state);
                    });
                });
            });
            // What it renders, beside the picture of its kind.
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.add(crate::icons::image(icon, 28.0, theme.text_muted));
                    ui.add_space(6.0);
                    ui.vertical(body);
                });
            });
        });
    });
    ui.add_space(6.0);
    let cross = remove.expect("the cross is drawn");
    (boxed.response, cross)
}
