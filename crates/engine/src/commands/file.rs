//! File commands: the project file the session is saved in. None of them is an undo step.

use std::path::PathBuf;
use std::sync::Arc;

use iskra_store::{SaveReport, SqliteStore};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{invalid, parse};
use crate::{Result, Session};

#[derive(Deserialize)]
struct File {
    path: PathBuf,
}

fn report(report: SaveReport) -> Value {
    json!({ "tilesWritten": report.tiles_written, "tilesDeleted": report.tiles_deleted, "vectorsWritten": report.vectors_written, "structureWritten": report.structure_written })
}

/// Replaces the open project with a new, empty one that has no file yet: the one the app
/// opens with, its shot with the peg that moves its camera, an animation layer and a board.
pub(super) fn new(s: &mut Session, _: Value) -> Result<Value> {
    s.replace(Arc::new(iskra_model::Project::fresh()), None);
    Ok(Value::Null)
}

/// Opens a project file: its structure, with no shot loaded yet. A shot's drawings are read
/// when it is loaded (`shot.load`).
pub(super) fn open(s: &mut Session, params: Value) -> Result<Value> {
    let File { path } = parse("file.open", params)?;
    let (store, mut project) = SqliteStore::open(&path)?;
    // The pictures of texture swatches, which are not in the file but in their media's.
    let media = project.media.clone();
    let palette = Arc::make_mut(&mut Arc::make_mut(&mut project).palette);
    super::palette::read_pictures(palette, &media, path.parent(), None);
    s.replace(project, Some(Box::new(store)));
    Ok(Value::Null)
}

/// Saves the project into its file, writing only what changed since the last save, and replaces
/// the backup beside it with a copy of what was saved.
pub(super) fn save(s: &mut Session, _: Value) -> Result<Value> {
    s.keep_history_whole()?;
    let project = s.project().clone();
    let Some(store) = s.store.as_mut() else { return invalid("the project has no file yet; save it with file.saveAs") };
    let written = store.save(&project)?;
    s.saved = Some(project);
    Ok(report(written))
}

/// Saves the project into its file as `file.save` does, and leaves the backup alone: for the
/// app's own saves between the user's, so the backup stays the version the user last saved.
pub(super) fn autosave(s: &mut Session, _: Value) -> Result<Value> {
    s.keep_history_whole()?;
    let project = s.project().clone();
    let Some(store) = s.store.as_mut() else { return invalid("the project has no file yet; save it with file.saveAs") };
    let written = store.autosave(&project)?;
    s.saved = Some(project);
    Ok(report(written))
}

/// Saves the project into a new file, which becomes the project's file. A file already at
/// `path` is replaced. With `projectFolder`, the file goes into a folder named after it beside
/// where `path` points (`Films/Ball.iskra` is saved as `Films/Ball/Ball.iskra`), and the folder
/// gets the folders a project starts with: Images, Audio, Palettes, Library, Shots, Renders.
/// Returns the `path` the project was saved at.
pub(super) fn save_as(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        path: PathBuf,
        #[serde(default)]
        project_folder: bool,
    }
    let Params { path, project_folder } = parse("file.saveAs", params)?;
    let path = if project_folder { iskra_store::in_project_folder(&path) } else { path };
    if project_folder {
        iskra_store::make_project_folder(&path)?;
    }
    if s.path().is_some_and(|current| current == path) {
        save(s, Value::Null)?;
        return Ok(json!({ "path": path }));
    }
    s.keep_history_whole()?;
    let project = s.project().clone();
    // The shots that are not loaded have their drawings in the old file only.
    s.store = Some(match s.store.as_mut() {
        Some(old) => old.save_as(&path, &project)?,
        None => Box::new(SqliteStore::create(&path, &project)?),
    });
    s.saved = Some(project);
    Ok(json!({ "path": path }))
}

/// For a damaged project file: copies its backup, the last saved version, to `<name> (recovered).iskra` beside it
/// and opens the copy. The damaged file and the backup are left as they are.
pub(super) fn recover(s: &mut Session, params: Value) -> Result<Value> {
    let File { path } = parse("file.recover", params)?;
    let recovered = iskra_store::recover(&path)?;
    open(s, json!({ "path": recovered }))?;
    Ok(json!({ "path": recovered }))
}
