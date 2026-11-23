//! Runs the built `iskra-extract` on real project files.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use iskra_drawing::Rect;
use iskra_model::{ArtContent, ArtLayerKind, DrawingId, PoolDrawing, Project, SwatchId};
use iskra_store::SqliteStore;

/// Save a project with two painted drawings at `path`.
fn save_project(path: &Path) {
    let mut project = Project::default();
    for name in ["1", "2"] {
        let id = DrawingId(project.new_id());
        let layer = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).layers[0]);
        let mut drawing = iskra_model::Drawing::default();
        let Some(ArtContent::Indexed(line)) = drawing.layer_mut(ArtLayerKind::LineArt).map(|l| &mut l.content) else { panic!("indexed") };
        line.paint(Rect::new(0, 0, 8, 8), &[1.0; 64], SwatchId(1));
        layer.drawings.push(PoolDrawing { id, name: name.to_string(), drawing: Arc::new(drawing) });
    }
    SqliteStore::create(path, &Arc::new(project)).unwrap();
}

fn extract(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_iskra-extract")).args(args).output().unwrap()
}

#[test]
fn a_sound_project_extracts_beside_itself_and_is_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("Film.iskra");
    save_project(&project);
    let before = std::fs::read(&project).unwrap();

    let output = extract(&[&project]);
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("2 drawings written"));
    let out = dir.path().join("Film extracted");
    assert!(out.join("drawings/Shot_01/Drawing/1.png").exists() && out.join("drawings/Shot_01/Drawing/2.png").exists());
    assert!(out.join("timing.json").exists() && out.join("palette.json").exists());
    assert_eq!(std::fs::read(&project).unwrap(), before, "the project file is only read");
}

#[test]
fn a_damaged_project_still_gives_up_its_drawings_and_says_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("Film.iskra");
    save_project(&project);
    rusqlite::Connection::open(&project).unwrap().execute("UPDATE document SET json = 'gone'", []).unwrap();
    assert!(SqliteStore::open(&project).is_err(), "the app refuses this file");

    let out = dir.path().join("out");
    let output = extract(&[&project, &out]);
    assert_eq!(output.status.code(), Some(1), "recovered, but not everything: {output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("structure can't be read"));
    let images = std::fs::read_dir(out.join("drawings/Shot_01/recovered")).unwrap().count();
    assert_eq!(images, 2, "both drawings, named by id");
}

#[test]
fn bad_arguments_and_missing_files_fail_with_a_message() {
    let usage = extract(&[]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&usage.stderr).contains("usage:"));
    let missing = extract(&[Path::new("/nowhere/Film.iskra")]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Film.iskra"));
}
