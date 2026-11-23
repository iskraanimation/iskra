//! Files that are written whole and read whole: library templates and the shots that are
//! handed out. Each is a project file with a row more in `document` that says what it is. It
//! is written beside its place and moved into it, and has no backup: it is itself a copy.

use std::path::Path;
use std::sync::Arc;

use iskra_model::{PoolDrawing, Project};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::sqlite::{checked_version, read_drawings, read_structure};
use crate::{LoadReport, Result, SqliteStore, StoreError};

fn damaged(path: &Path) -> impl Fn(String) -> StoreError + '_ {
    |why| StoreError::Damaged { path: path.to_path_buf(), why, backup: None }
}

/// Write `project`, whose shots must all be loaded, as a file at `path`, replacing any file
/// there. `more` adds what the file has beside the project. A failed write leaves what was
/// there.
pub(crate) fn write(path: &Path, project: &Arc<Project>, more: impl FnOnce(&Connection) -> Result<()>) -> Result<()> {
    let partial = path.with_extension("partial");
    let write = || {
        let mut store = SqliteStore::new_file(&partial)?;
        store.write(project)?;
        more(&store.conn)?;
        drop(store);
        std::fs::rename(&partial, path).map_err(|source| StoreError::Io { path: path.to_path_buf(), source })
    };
    let written = write();
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    written
}

/// Put a row that says what the file is into `document`, as JSON.
pub(crate) fn put_row(conn: &Connection, name: &str, json: String) -> Result<()> {
    conn.execute("INSERT INTO document (name, json) VALUES (?1, ?2)", [name, &json])?;
    Ok(())
}

/// Open the file at `path` for reading only, once it is known to be an Iskra file that this
/// build reads. Returns its format version too.
pub(crate) fn open(path: &Path) -> Result<(Connection, i64)> {
    std::fs::metadata(path).map_err(|source| StoreError::Io { path: path.to_path_buf(), source })?;
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| damaged(path)(e.to_string()))?;
    let version = checked_version(&conn, path)?;
    Ok((conn, version))
}

/// The JSON of the row `name` of `document`, if the file has one.
pub(crate) fn row(conn: &Connection, path: &Path, name: &str) -> Result<Option<String>> {
    conn.query_row("SELECT json FROM document WHERE name = ?1", [name], |row| row.get(0)).optional().map_err(|e| damaged(path)(e.to_string()))
}

/// Read the project of an open file whole, after checking that the file is sound. As with a
/// project that is opened, a tile that can't be read is skipped and reported.
pub(crate) fn read(conn: &Connection, version: i64, path: &Path) -> Result<(Project, LoadReport)> {
    let check: String = conn.pragma_query_value(None, "integrity_check", |row| row.get(0)).map_err(|e| damaged(path)(e.to_string()))?;
    if check != "ok" {
        return Err(damaged(path)(check));
    }
    let (_, mut project) = read_structure(conn, version).map_err(|why| damaged(path)(format!("its structure can't be read: {why}")))?;
    let drawings: Vec<PoolDrawing> = project.shots.iter().flat_map(|shot| shot.drawings()).cloned().collect();
    let mut report = LoadReport::default();
    let pixels = read_drawings(conn, &drawings, &mut report)?;
    for shot in &mut project.shots {
        *shot = Arc::new(shot.with_pixels(&pixels));
    }
    Ok((project, report))
}
