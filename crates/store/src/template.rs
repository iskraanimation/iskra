//! Library template files (plan §4.12): `<name>.iskratpl`.
//!
//! A template file is a project file: its project has one shot, which holds the template's
//! nodes (see `iskra_model`'s `template`), so everything that reads a project reads a template.
//! It has two things more:
//!
//! - a `template` row in `document`: what the template says about itself, as JSON;
//! - a `preview` table: pictures of it as PNG, by frame. For now one, of frame 0.
//!
//! A template is written whole and read whole (see `whole`), and so is a shot that is handed
//! out to be worked on elsewhere (plan §7.2a): a project file of that one shot, with a `handOff`
//! row in `document` that says which shot of which project it is.

use std::path::Path;
use std::sync::Arc;

use iskra_model::{HandOff, Project, TemplateInfo};
use rusqlite::{Connection, OptionalExtension};

use crate::{LoadReport, Result, StoreError, whole};

/// The extension of a template file, without the dot.
pub const TEMPLATE_EXTENSION: &str = "iskratpl";

const PREVIEW_SCHEMA: &str = "CREATE TABLE preview (frame INTEGER PRIMARY KEY, png BLOB NOT NULL);";

pub struct Template {
    pub info: TemplateInfo,
    /// One shot, loaded, with the template's nodes.
    pub project: Arc<Project>,
    /// A small picture of it, as a PNG file's bytes.
    pub preview: Option<Vec<u8>>,
}

/// Write a template file at `path`, replacing any file there. Every shot of its project must be
/// loaded. A failed write leaves what was there.
pub fn write_template(path: &Path, template: &Template) -> Result<()> {
    whole::write(path, &template.project, |conn| {
        conn.execute_batch(PREVIEW_SCHEMA)?;
        whole::put_row(conn, "template", serde_json::to_string(&template.info)?)?;
        if let Some(png) = &template.preview {
            conn.execute("INSERT INTO preview (frame, png) VALUES (0, ?1)", [png])?;
        }
        Ok(())
    })
}

/// Open the template file at `path` for reading, once it is known to be one this build reads.
fn open(path: &Path) -> Result<(Connection, i64, TemplateInfo)> {
    let (conn, version) = whole::open(path)?;
    let Some(info) = whole::row(&conn, path, "template")? else {
        return Err(StoreError::NotAProject { path: path.to_path_buf(), why: "an Iskra project, not a library template".to_string() });
    };
    let unreadable = |e: serde_json::Error| StoreError::Damaged {
        path: path.to_path_buf(),
        why: format!("what the template says about itself can't be read: {e}"),
        backup: None,
    };
    Ok((conn, version, serde_json::from_str(&info).map_err(unreadable)?))
}

/// What the template file at `path` says about itself. Its nodes and drawings are not read.
pub fn read_template_info(path: &Path) -> Result<TemplateInfo> {
    Ok(open(path)?.2)
}

/// The picture the template file at `path` has of itself, as a PNG file's bytes.
pub fn read_template_preview(path: &Path) -> Result<Option<Vec<u8>>> {
    let (conn, ..) = open(path)?;
    Ok(conn.query_row("SELECT png FROM preview WHERE frame = 0", [], |row| row.get(0)).optional()?)
}

/// Read the template file at `path` whole, after checking that it is sound. As with a project,
/// a tile that can't be read is skipped and reported.
pub fn read_template(path: &Path) -> Result<(Template, LoadReport)> {
    let (conn, version, info) = open(path)?;
    let (project, report) = whole::read(&conn, version, path)?;
    let preview = conn.query_row("SELECT png FROM preview WHERE frame = 0", [], |row| row.get(0)).optional()?;
    Ok((Template { info, project: Arc::new(project), preview }, report))
}

/// Write a shot that is handed out as a file at `path`, replacing any file there: `project`,
/// which holds the one shot, loaded, and what the file says about where it came from. It is a
/// project file like any other, and is opened and saved as one.
pub fn write_shot_file(path: &Path, project: &Arc<Project>, from: &HandOff) -> Result<()> {
    whole::write(path, project, |conn| whole::put_row(conn, "handOff", serde_json::to_string(from)?))
}

/// Read a shot file that has come back, whole: its project, and where it says it came from.
/// A file that doesn't say, since it was saved as another file meanwhile, is read all the same.
pub fn read_shot_file(path: &Path) -> Result<(Option<HandOff>, Arc<Project>, LoadReport)> {
    let (conn, version) = whole::open(path)?;
    // What it says that can't be read is as good as not said: the shot is found by its name.
    let from = whole::row(&conn, path, "handOff")?.and_then(|json| serde_json::from_str(&json).ok());
    let (project, report) = whole::read(&conn, version, path)?;
    Ok((from, Arc::new(project), report))
}

#[cfg(test)]
mod tests {
    use iskra_drawing::Rect;
    use iskra_model::{ArtContent, DrawingId, Part, PoolDrawing, SwatchId};

    use super::*;
    use crate::SqliteStore;

    /// A template of a project's one layer, which has one painted drawing.
    fn template() -> Template {
        let mut project = Project::default();
        let id = DrawingId(project.new_id());
        let layer = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).layers[0]);
        let mut drawing = iskra_model::Drawing::default();
        let ArtContent::Indexed(surface) = &mut drawing.art_layers[1].content else { panic!("indexed") };
        surface.paint(Rect::new(300, 10, 302, 11), &[1.0, 0.5], SwatchId(4));
        layer.drawings.push(PoolDrawing { id, name: "1".to_string(), drawing: Arc::new(drawing) });
        layer.exposure.set(0, 6, Some(id));
        let project = project.template(project.shots[0].id, &[Part::Layer(project.shots[0].layers[0].id)], "Ball").unwrap();
        let info = TemplateInfo { source: "Film".to_string(), saved: 1_760_000_000, tags: vec!["prop".to_string()], notes: "Bounces.".to_string() };
        Template { info, project: Arc::new(project), preview: Some(b"\x89PNG not really".to_vec()) }
    }

    #[test]
    fn a_template_comes_back_as_it_was_written_and_says_what_it_is_without_being_read_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Ball.iskratpl");
        let written = template();
        write_template(&path, &written).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "one file: nothing half-written, no backup");

        let (read, report) = read_template(&path).unwrap();
        assert_eq!(report, LoadReport::default());
        assert_eq!((&read.info, &read.project, &read.preview), (&written.info, &written.project, &written.preview), "with every pixel");
        assert_eq!(read_template_info(&path).unwrap(), written.info);
        assert_eq!(read_template_preview(&path).unwrap(), written.preview);

        // Writing over it replaces it; a template may have no picture.
        let plain = Template { info: TemplateInfo::default(), preview: None, project: written.project.clone() };
        write_template(&path, &plain).unwrap();
        assert_eq!((read_template_info(&path).unwrap(), read_template_preview(&path).unwrap()), (TemplateInfo::default(), None));
    }

    #[test]
    fn a_template_file_is_a_project_file_and_a_project_file_is_not_a_template() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Ball.iskratpl");
        let written = template();
        write_template(&path, &written).unwrap();
        let (_, opened, _) = SqliteStore::open_whole(&path).unwrap();
        assert_eq!(opened, written.project, "what reads a project reads a template");
        assert_eq!(SqliteStore::salvage(&path).unwrap().0, *written.project);

        let project = dir.path().join("Film.iskra");
        drop(SqliteStore::create(&project, &written.project).unwrap());
        assert!(matches!(read_template(&project), Err(StoreError::NotAProject { why, .. }) if why.contains("not a library template")));
        assert!(matches!(read_template_info(&dir.path().join("none.iskratpl")), Err(StoreError::Io { .. })));
        std::fs::write(&path, b"this is not a database at all, just some text that is long enough").unwrap();
        assert!(matches!(read_template_info(&path), Err(StoreError::Damaged { .. })));
    }

    #[test]
    fn a_shot_file_is_a_project_that_says_where_it_came_from_as_long_as_it_is_saved_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Shot_01.iskra");
        let project = Arc::new(template().project.shot_file(template().project.shots[0].id).unwrap());
        let from =
            HandOff { project: "Film".to_string(), shot: project.shots[0].id.0, name: "Shot_01".to_string(), sent: 1_760_000_000, to: "Ana".to_string() };
        write_shot_file(&path, &project, &from).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let (read, back, report) = read_shot_file(&path).unwrap();
        assert_eq!((read.as_ref(), &back, report), (Some(&from), &project, LoadReport::default()), "with every pixel");

        // Whoever has it opens and saves it as a project; it still says where it is from.
        let (mut store, opened, _) = SqliteStore::open_whole(&path).unwrap();
        let mut next = Project::clone(&opened);
        Arc::make_mut(&mut next.shots[0]).duration = 60;
        let next = Arc::new(next);
        crate::ProjectStore::save(&mut store, &next).unwrap();
        let copy = dir.path().join("Copy.iskra");
        drop(crate::ProjectStore::save_as(&mut store, &copy, &next).unwrap());
        drop(store);
        let (read, back, _) = read_shot_file(&path).unwrap();
        assert_eq!((read, back.shots[0].duration), (Some(from), 60));
        // Saved as another file, it no longer says, and is read all the same.
        let (read, back, _) = read_shot_file(&copy).unwrap();
        assert_eq!((read, &back), (None, &next));
        assert!(matches!(read_shot_file(&dir.path().join("none.iskra")), Err(StoreError::Io { .. })));
    }
}
