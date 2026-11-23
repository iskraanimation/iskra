//! The backup (plan §4.9): one copy of the project as it was last saved, `<name>.backup.iskra`
//! beside the project file. Every save replaces it, so whatever happens to the project file
//! afterwards, the last saved version can be returned to.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::{Result, StoreError};

fn stem(path: &Path) -> String {
    path.file_stem().map_or_else(|| "project".to_string(), |s| s.to_string_lossy().into_owned())
}

/// Where the backup of the project at `path` is kept.
pub fn backup_path(path: &Path) -> PathBuf {
    path.with_file_name(format!("{}.backup.iskra", stem(path)))
}

/// The backup of the project at `path`, if it has one.
pub fn backup_of(path: &Path) -> Option<PathBuf> {
    Some(backup_path(path)).filter(|backup| backup.is_file())
}

/// Copy the backup of the project at `path` to `<name> (recovered).iskra` beside it and return
/// that copy's path. The damaged file and the backup are left as they are.
pub fn recover(path: &Path) -> Result<PathBuf> {
    let Some(backup) = backup_of(path) else {
        return Err(StoreError::NotAProject { path: path.to_path_buf(), why: "there is no backup to recover from".to_string() });
    };
    let target = path.with_file_name(format!("{} (recovered).iskra", stem(path)));
    std::fs::copy(&backup, &target).map_err(|source| StoreError::Io { path: target.clone(), source })?;
    Ok(target)
}

/// Write a copy of the project file as it is on disk now to `target`, replacing what is there.
/// The copy is written beside `target` first and moved into place whole, so a failed copy
/// leaves the earlier one. Must not be called inside a transaction.
pub fn copy_to(conn: &Connection, target: &Path) -> Result<()> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| StoreError::Io { path, source }
    };
    let partial = target.with_extension("partial");
    match std::fs::remove_file(&partial) {
        Err(source) if source.kind() != std::io::ErrorKind::NotFound => return Err(io(&partial)(source)),
        _ => {}
    }
    // SQLite writes a consistent, compacted copy.
    conn.execute("VACUUM INTO ?1", [partial.to_string_lossy()])?;
    std::fs::rename(&partial, target).map_err(io(target))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_model::Project;

    use super::*;
    use crate::{ProjectStore, SqliteStore};

    #[test]
    fn every_save_replaces_the_one_backup_with_what_was_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("My Film.iskra");
        let first = Arc::new(Project::default());
        let mut store = SqliteStore::create(&path, &first).unwrap();
        let backup = dir.path().join("My Film.backup.iskra");
        assert_eq!(backup_of(&path), Some(backup.clone()), "the first save already has its copy");
        assert_eq!(SqliteStore::open_whole(&backup).unwrap().1, first, "a backup opens like any project");

        let mut renamed = Project::clone(&first);
        Arc::make_mut(&mut renamed.shots[0]).name = "Opening".to_string();
        let renamed = Arc::new(renamed);
        store.save(&renamed).unwrap();
        assert_eq!(SqliteStore::open_whole(&backup).unwrap().1, renamed, "the copy is of the last save");
        let files = std::fs::read_dir(dir.path()).unwrap().count();
        store.save(&renamed).unwrap();
        assert_eq!(files, 2, "one project file and one backup, nothing left over");

        // An autosave writes the project file and leaves the backup at the user's last save.
        let mut again = Project::clone(&renamed);
        Arc::make_mut(&mut again.shots[0]).name = "Closing".to_string();
        let again = Arc::new(again);
        assert!(store.autosave(&again).unwrap().structure_written);
        assert_eq!(SqliteStore::open_whole(&path).unwrap().1, again);
        assert_eq!(SqliteStore::open_whole(&backup).unwrap().1, renamed);
        // Saving what was already autosaved writes nothing more, and brings the backup up to it.
        assert_eq!(store.save(&again).unwrap(), crate::SaveReport::default());
        assert_eq!(SqliteStore::open_whole(&backup).unwrap().1, again);
        let renamed = again;

        drop(store);
        let recovered = recover(&path).unwrap();
        assert_eq!(recovered, dir.path().join("My Film (recovered).iskra"));
        assert_eq!(SqliteStore::open_whole(&recovered).unwrap().1, renamed);
        assert!(backup.exists() && path.exists(), "recovering changes neither the backup nor the original");
        assert!(recover(&dir.path().join("Other.iskra")).is_err());
    }
}
