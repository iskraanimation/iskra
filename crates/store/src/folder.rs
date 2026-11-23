//! The project folder. A project saved from the app is a folder named after it: the project
//! file inside, with its backup beside it, and folders for what a film gathers around itself.
//! Nothing needs the folders to be there: they are made so that a project has one place where
//! its things go, the same in every project.

use std::path::{Path, PathBuf};

use crate::{Result, StoreError};

/// The folders a new project folder starts with: pictures and sound brought in, palettes,
/// the project's own library of templates, shots sent out and come back, and what is rendered.
pub const PROJECT_FOLDERS: [&str; 6] = ["Images", "Audio", "Palettes", "Library", "Shots", "Renders"];

/// Where the project file goes when `chosen` is what the user picked to save as: into a folder
/// named after the project, beside where they pointed. `Films/Ball.iskra` gives
/// `Films/Ball/Ball.iskra`. A file picked inside a folder that already has its name stays
/// where it is, so a project saved again into its own folder isn't nested.
pub fn in_project_folder(chosen: &Path) -> PathBuf {
    let (Some(name), Some(stem)) = (chosen.file_name(), chosen.file_stem()) else { return chosen.to_path_buf() };
    let folder = chosen.parent().unwrap_or(Path::new(""));
    if folder.file_name() == Some(stem) {
        return chosen.to_path_buf();
    }
    folder.join(stem).join(name)
}

/// Make the folder a project file at `file` is in, and in it the folders a project starts
/// with. Those that are there already are left as they are.
pub fn make_project_folder(file: &Path) -> Result<()> {
    let Some(folder) = file.parent() else { return Ok(()) };
    for path in std::iter::once(folder.to_path_buf()).chain(PROJECT_FOLDERS.iter().map(|name| folder.join(name))) {
        std::fs::create_dir_all(&path).map_err(|source| StoreError::Io { path, source })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_goes_into_a_folder_of_its_name_with_the_folders_it_starts_with() {
        let dir = tempfile::tempdir().unwrap();
        let file = in_project_folder(&dir.path().join("Ball.iskra"));
        assert_eq!(file, dir.path().join("Ball").join("Ball.iskra"));
        assert_eq!(in_project_folder(&file), file, "picked inside its own folder, it is not nested");
        assert_eq!(in_project_folder(&dir.path().join("Ball").join("Take 2.iskra")), dir.path().join("Ball").join("Take 2").join("Take 2.iskra"));

        make_project_folder(&file).unwrap();
        assert!(PROJECT_FOLDERS.iter().all(|name| dir.path().join("Ball").join(name).is_dir()));
        // What is in them stays when the folder is made again.
        std::fs::write(dir.path().join("Ball/Images/sky.png"), b"png").unwrap();
        make_project_folder(&file).unwrap();
        assert!(dir.path().join("Ball/Images/sky.png").is_file());
    }
}
