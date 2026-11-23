//! Project files (plan §4.9).
//!
//! The engine saves through the [`ProjectStore`] trait and never sees SQLite, so another backend
//! (a studio server, say) can be added later. [`SqliteStore`] is the backend: one `.iskra` file
//! that holds the project's structure as JSON and its drawings as compressed tiles, with
//! a backup of the last save beside it. `docs/file-format.md` documents the file. Files of earlier
//! format versions are converted as they are read. A library template (`.iskratpl`) is such a
//! file too, written and read whole (see `template`).

mod backup;
mod folder;
mod migrate;
mod sqlite;
mod template;
mod whole;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use backup::{backup_of, backup_path, recover};
pub use folder::{PROJECT_FOLDERS, in_project_folder, make_project_folder};
use iskra_model::{Drawing, DrawingId, PoolDrawing, Project};
pub use sqlite::SqliteStore;
pub use template::{TEMPLATE_EXTENSION, Template, read_shot_file, read_template, read_template_info, read_template_preview, write_shot_file, write_template};

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The file is not an Iskra project, or was written by a newer version of the app.
    #[error("{path}: {why}")]
    NotAProject { path: PathBuf, why: String },
    /// The file failed its integrity check. `backup` is its backup, if it has one.
    #[error("{path} is damaged: {why}")]
    Damaged { path: PathBuf, why: String, backup: Option<PathBuf> },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("project file: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("project structure: {0}")]
    Json(#[from] serde_json::Error),
}

/// What a save wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveReport {
    pub tiles_written: usize,
    pub tiles_deleted: usize,
    /// Art layers of vector art written whole, as they had changed (plan §4.16).
    pub vectors_written: usize,
    /// The project's structure had changed and was written.
    pub structure_written: bool,
}

/// What a load could not read. The rest of the project is loaded all the same.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// One line per tile that was skipped, saying which and why.
    pub skipped: Vec<String>,
}

/// Where an open project is saved, and where the pixels of its shots are until they are loaded.
///
/// A shot that is not loaded ([`iskra_model::Shot::loaded`]) stands for its drawings as the
/// store has them, so whoever holds snapshots keeps to two rules: a shot's drawings are only
/// changed or deleted while the shot is loaded, and a shot is only made not loaded again while
/// its drawings are the ones last saved or loaded.
pub trait ProjectStore {
    /// Save the project. A save is all or nothing: a crash leaves the previous save intact.
    /// Each save also replaces the store's backup with a copy of what was saved. The drawings of shots that are not loaded are left as the store has them.
    fn save(&mut self, project: &Arc<Project>) -> Result<SaveReport>;

    /// Save the project as [`ProjectStore::save`] does, and leave the backup as it is: the
    /// backup stays the version the user last saved, however often the app saves in between.
    fn autosave(&mut self, project: &Arc<Project>) -> Result<SaveReport>;

    /// Read the pixels of `drawings`, which are from shots that are not loaded, as they were
    /// last saved: each drawing by id, filled.
    fn load(&mut self, drawings: &[PoolDrawing]) -> Result<(HashMap<DrawingId, Arc<Drawing>>, LoadReport)>;

    /// Read the pixels of `drawings` as [`ProjectStore::load`] does, to look at and let go:
    /// the store does not count them as held.
    fn peek(&mut self, drawings: &[PoolDrawing]) -> Result<HashMap<DrawingId, Arc<Drawing>>>;

    /// The caller no longer holds these drawings as they were last saved or loaded, so the
    /// store need not either.
    fn release(&mut self, drawings: &[DrawingId]);

    /// Save the project into a new store at `path`, replacing any file there, with the pixels
    /// of the shots that are not loaded copied from this one. This store is left as it is.
    fn save_as(&mut self, path: &Path, project: &Arc<Project>) -> Result<Box<dyn ProjectStore>>;

    /// The file the project is saved in.
    fn path(&self) -> &Path;
}
