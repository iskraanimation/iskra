//! Export commands (plan §7.14, R6): shots rendered to files. None of them is an undo step.

use std::path::PathBuf;

use iskra_export::{IfExists, Settings};
use iskra_model::{ShotId, StoryboardId};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{invalid, parse, shot_id};
use crate::{Result, Session};

/// Renders shots to image sequences, each in a folder of its own in `folder`, named for the
/// shot: `folder/Shot_01/Shot_01.0001.png`, the number being the frame's in the shot, from 1.
///
/// `shots` are the shots; without them, `shot`, or else the project's first. `settings` say
/// how: `format` (`png`, `jpeg`, `tiff`, `exr`), `bits` a channel (8, 16, or 32 for floats),
/// `alpha` for transparency or else the `matte` colour under the picture, `scale` of the
/// shot's size, and `range`: `{ "cut": { "handles": n } }` for the part in the cut with that
/// many frames of handles, or `"whole"`. What is not given is a PNG of 8 bits with
/// transparency, of the cut, at the shot's size. `ifExists` says what to do where a shot's
/// folder has frames already: `fail` (the default), `replace`, `version` (beside it, as
/// `Shot_01_v002`) or `skip`.
///
/// `pick` renders some of the shots' passes: `passes` are the names of those in the picture,
/// `rest` whether what is in no pass is in it too, and `named` makes the render an output of
/// its own under that name, a folder down: `folder/Shot_01/Background/Shot_01_Background.0001.png`,
/// or for a movie `folder/Background/Shot_01_Background.mov`. A shot without a pass of a
/// name has nothing of it in the picture.
///
/// `work` in the settings says which of a shot is in the picture (plan §7.2c): `animation`,
/// which is what a render is of unless it is said, `boards` for an animatic, or `whole`. A
/// shot that shows nothing of it is rendered empty, and a note says so.
///
/// `naming` says what the files are called and where under `folder` they go: `name` in place
/// of the shot's name, `suffix` after it, and `subfolder`. With several shots a `name` would
/// make them one file: give it for one shot.
///
/// For a movie the settings also take `codec` (`proRes` or `h264`; an MP4 is H.264), `prores`
/// (`proxy`, `lt`, `standard`, `hq`, `p4444`, `p4444Xq`; the last two hold transparency with
/// `alpha`), `quality` of H.264 (`best`, `high`, `medium`, `low`) or `videoKbps` to hold it to a data rate instead, `soundCodec` (`pcm` or
/// `aac`), `soundKbps` and `soundRate` (44100, 48000 or 96000). `size`, `[width, height]` in
/// pixels, fits the shot whole into a picture of that size, for a sequence as for a movie.
///
/// A shot whose drawings are not loaded is loaded for the render and let go after it. The
/// project keeps a record of each render of a shot's whole picture, with how the shot was
/// then, which is no undo step.
/// With the `format` `mov` or `mp4` a shot is rendered to a movie instead: one file for the
/// shot, all shots' movies together in `folder` (`folder/Shot_01.mov`), ProRes 422 HQ with
/// uncompressed sound or H.264 with AAC, with the shot's stretch of the storyboard's sound in
/// it unless `sound` is false in the settings. A movie has no transparency.
///
/// Returns, as `sequences`, for each shot its `folder`, the `frames` rendered (the first, and
/// one past the last), how many `files` were written, for a movie its `file`, and as `notes`
/// what had to be left out.
pub(super) fn sequence(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        shot: Option<u64>,
        shots: Option<Vec<u64>>,
        folder: PathBuf,
        #[serde(default)]
        settings: Settings,
        #[serde(default)]
        if_exists: IfExists,
        pick: Option<iskra_export::Pick>,
        #[serde(default)]
        naming: iskra_export::Naming,
    }
    let Params { shot, shots, folder, settings, if_exists, pick, naming } = parse("export.sequence", params)?;
    let shots: Vec<ShotId> = match shots {
        Some(shots) => shots.into_iter().map(|shot| shot_id(s.project(), Some(shot))).collect::<Result<_>>()?,
        None => vec![shot_id(s.project(), shot)?],
    };
    let mut sequences = Vec::new();
    for shot in shots {
        let was_loaded = s.project().shot(shot).is_some_and(|shot| shot.loaded);
        s.load_shot(shot)?;
        let project = s.project().clone();
        // A pass's render is recorded under its name, beside the shot's own.
        let mark = iskra_export::mark_of(&project, iskra_model::Rendered::Shot(shot), &settings);
        // A movie is one file for the shot, with its sound; the others are a file a frame.
        let rendered = if settings.format.is_movie() {
            let own = s.path().and_then(|file| file.parent()).map(std::path::Path::to_path_buf);
            iskra_export::export_movie_picked(&project, shot, &settings, pick.as_ref(), &naming, &folder, if_exists, own.as_deref(), &|_, _| true)
        } else {
            let own = s.path().and_then(|file| file.parent()).map(std::path::Path::to_path_buf);
            iskra_export::export_sequence_picked(&project, shot, &settings, pick.as_ref(), &naming, &folder, if_exists, own.as_deref(), &|_, _| true)
        };
        if !was_loaded {
            s.unload_shot(shot);
        }
        // The project keeps a record of what was rendered, to say later what changed since.
        if let (Ok(report), Some(mark)) = (&rendered, mark)
            && let Some(record) = iskra_export::record_of(iskra_model::Rendered::Shot(shot), &settings, mark, report, pick.as_ref())
        {
            s.note(|p| p.record_render(record.clone()));
        }
        match rendered {
            Ok(report) => sequences.push(json!({
                "shot": shot.0,
                "folder": report.folder,
                "frames": [report.frames.0, report.frames.1],
                "files": report.files.len(),
                "file": report.files.first().filter(|_| settings.format.is_movie()),
                "notes": report.notes,
            })),
            Err(problem) => return invalid(problem.to_string()),
        }
    }
    Ok(json!({ "sequences": sequences }))
}

/// Renders a storyboard to one movie in `folder`, named for the storyboard: its shots in the
/// cut as they lie, black between them, with all of its sound. `storyboard` is the
/// storyboard, or else the project's first; `settings` and `ifExists` are as for
/// `export.sequence`, with the `format` `mov` or `mp4`, and `naming` too; `pick` as there, a
/// pass of every shot, or a shot whole that has none. Transitions are not in it. Returns the
/// `file`, the `frames` of the film, and `notes` of what was left out.
pub(super) fn storyboard(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        storyboard: Option<u64>,
        folder: PathBuf,
        #[serde(default)]
        settings: Settings,
        #[serde(default)]
        if_exists: IfExists,
        #[serde(default)]
        naming: iskra_export::Naming,
        pick: Option<iskra_export::Pick>,
    }
    let Params { storyboard, folder, settings, if_exists, naming, pick } = parse("export.storyboard", params)?;
    let board = match (storyboard, s.project().storyboards.first()) {
        (Some(id), _) => super::storyboard::storyboard_id(s.project(), id)?,
        (None, Some(first)) => first.id,
        (None, None) => return invalid("the project has no storyboard"),
    };
    // Every shot of the film is needed at once; those that were not loaded are let go after.
    let shots: Vec<ShotId> = s.project().storyboard(StoryboardId(board.0)).map_or(Vec::new(), |board| board.clips().iter().map(|clip| clip.shot).collect());
    let unloaded: Vec<ShotId> = shots.iter().copied().filter(|shot| s.project().shot(*shot).is_some_and(|shot| !shot.loaded)).collect();
    for shot in &shots {
        s.load_shot(*shot)?;
    }
    let project = s.project().clone();
    let own = s.path().and_then(|file| file.parent()).map(std::path::Path::to_path_buf);
    let mark = iskra_export::mark_of(&project, iskra_model::Rendered::Film(board), &settings);
    let rendered = iskra_export::export_storyboard_named(&project, board, &settings, pick.as_ref(), &naming, &folder, if_exists, own.as_deref(), &|_, _| true);
    if let (Ok(report), Some(mark)) = (&rendered, mark)
        && let Some(record) = iskra_export::record_of(iskra_model::Rendered::Film(board), &settings, mark, report, pick.as_ref())
    {
        s.note(|p| p.record_render(record.clone()));
    }
    for shot in unloaded {
        s.unload_shot(shot);
    }
    match rendered {
        Ok(report) => Ok(json!({ "file": report.files.first(), "frames": [report.frames.0, report.frames.1], "notes": report.notes })),
        Err(problem) => invalid(problem.to_string()),
    }
}

/// Writes a storyboard's panels on pages, as a PDF at `file` (plan §4.7): `storyboard` is the
/// storyboard, or else the project's first; `options` are [`iskra_export::pages::PageOptions`],
/// what is not given being as its `mode` (Sheets if none) starts with; with `pngs`, a picture of
/// each page beside it too. The header has the project file's name and `date`, or today. The
/// shots that were not loaded are let go after. Returns the `file`, and the pages' `pictures`.
pub(super) fn pdf(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: Option<u64>,
        file: PathBuf,
        #[serde(default)]
        options: Value,
        #[serde(default)]
        pngs: bool,
        date: Option<String>,
    }
    let Params { storyboard, file, options: given, pngs, date } = parse("export.pdf", params)?;
    // What is given, over what its mode starts with.
    let mode = given.get("mode").cloned().map(serde_json::from_value).transpose().map_err(|why| crate::EngineError::Invalid(format!("export.pdf: {why}")))?;
    let mut options = serde_json::to_value(iskra_export::pages::PageOptions::of(mode.unwrap_or_default())).unwrap_or_default();
    if let (Some(options), Some(given)) = (options.as_object_mut(), given.as_object()) {
        options.extend(given.clone());
    }
    let options: iskra_export::pages::PageOptions = parse("export.pdf", options)?;
    let board = match (storyboard, s.project().storyboards.first()) {
        (Some(id), _) => super::storyboard::storyboard_id(s.project(), id)?,
        (None, Some(first)) => first.id,
        (None, None) => return invalid("the project has no storyboard"),
    };
    let shots: Vec<ShotId> = s.project().storyboard(board).map_or(Vec::new(), |board| board.clips().iter().map(|clip| clip.shot).collect());
    let unloaded: Vec<ShotId> = shots.iter().copied().filter(|shot| s.project().shot(*shot).is_some_and(|shot| !shot.loaded)).collect();
    for shot in &shots {
        s.load_shot(*shot)?;
    }
    let name = s.path().and_then(|path| path.file_stem()).map_or("Untitled".to_string(), |stem| stem.to_string_lossy().into_owned());
    let heading = iskra_export::pages::Heading { project: name, date: date.unwrap_or_else(iskra_export::pages::today) };
    let written = iskra_export::pdf::export_pages(&s.project().clone(), board, &options, &heading, &file, pngs);
    for shot in unloaded {
        s.unload_shot(shot);
    }
    match written {
        Ok(written) => Ok(json!({ "file": written.pdf, "pictures": written.pages })),
        Err(problem) => invalid(problem.to_string()),
    }
}

/// Writes a storyboard's cut as an edit list into `folder`, named for the storyboard, for an
/// editing program to open the rendered shots assembled: each where it lies, with the
/// dissolves between them and the sound under them. `format` is `edl`, `otio` or `fcpxml`
/// (which has the picture alone). The shots refer to the movies the project's render records
/// know, or else to those a render of each shot's cut into `folder` as `movies` (`mov` or
/// `mp4`, `mov` if not given) would write. Returns the `file`.
pub(super) fn edit_list(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        storyboard: Option<u64>,
        folder: PathBuf,
        format: iskra_export::ListFormat,
        movies: Option<iskra_export::Format>,
    }
    let Params { storyboard, folder, format, movies } = parse("export.editList", params)?;
    let board = match (storyboard, s.project().storyboards.first()) {
        (Some(id), _) => super::storyboard::storyboard_id(s.project(), id)?,
        (None, Some(first)) => first.id,
        (None, None) => return invalid("the project has no storyboard"),
    };
    let own = s.path().and_then(|file| file.parent()).map(std::path::Path::to_path_buf);
    match iskra_export::export_edit_list(s.project(), board, format, &folder, movies.unwrap_or(iskra_export::Format::Mov), own.as_deref()) {
        Ok(file) => Ok(json!({ "file": file })),
        Err(problem) => invalid(problem.to_string()),
    }
}
