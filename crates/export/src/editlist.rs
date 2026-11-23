//! The edit list (plan §7.2a, §7.14): a storyboard's cut written for an editing program, so
//! that the rendered shots open there already assembled, each where it lies, with the
//! dissolves between them and the sound under them. Transitions are therefore not baked into
//! the shots' renders.
//!
//! Three public formats are written: an EDL (CMX 3600), which every editor reads and which
//! holds the least; OpenTimelineIO, which holds all of it; and FCPXML, for the picture.

use std::path::{Path, PathBuf};

use iskra_model::{Clip, Edge, MediaKind, Project, ShotId, Storyboard, StoryboardId, TransitionKind, timecode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{ExportError, Format, file_name, movie_file};

/// The kind of edit list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ListFormat {
    Edl,
    Otio,
    Fcpxml,
}

impl ListFormat {
    pub const ALL: [ListFormat; 3] = [ListFormat::Edl, ListFormat::Otio, ListFormat::Fcpxml];

    pub fn extension(self) -> &'static str {
        match self {
            ListFormat::Edl => "edl",
            ListFormat::Otio => "otio",
            ListFormat::Fcpxml => "fcpxml",
        }
    }
}

/// One shot of the cut, as the list has it.
struct Event {
    name: String,
    clip: Clip,
    /// The shot's movie, and the frame of the shot that the movie starts with.
    file: PathBuf,
    first: u32,
    /// The shot's whole length.
    duration: u32,
    /// The transition into the shot at its start, and out of it at its end: its kind, and
    /// how many of its frames lie on this shot's side of the cut and on the other.
    into: Option<(TransitionKind, u32, u32)>,
    out: Option<(TransitionKind, u32, u32)>,
}

/// The movie a shot's clip refers to: the newest that the project's records know of the
/// shot, with the frame it starts on, or else the one a render of the cut into `movies` would
/// write, `Shot_01.mov`.
fn movie_of(project: &Project, shot: ShotId, name: &str, clip: &Clip, movies: &Path, format: Format) -> (PathBuf, u32) {
    let known = project
        .renders
        .iter()
        .rev()
        .find(|record| record.what == iskra_model::Rendered::Shot(shot) && crate::settings_of(record).is_some_and(|settings| settings.format.is_movie()));
    match known {
        Some(record) => (record.at.clone(), record.mark.frames.0),
        None => (movie_file(movies, name, 1, format), clip.in_point),
    }
}

fn events(project: &Project, board: &Storyboard, movies: &Path, format: Format) -> Vec<Event> {
    let clips = board.clips();
    let mut events = Vec::new();
    for (at, clip) in clips.iter().enumerate() {
        let Some(shot) = project.shot(clip.shot) else { continue };
        let (file, first) = movie_of(project, shot.id, &shot.name, clip, movies, format);
        // A transition lies across the cut: half of it on each side, the odd frame after.
        let halves = |length: u32| (length / 2, length - length / 2);
        // The transition at the join with the clip before or after, whichever of the two
        // shots it was put on; on an edge that joins nothing it is a fade, all on this shot.
        let before = at.checked_sub(1).map(|at| &clips[at]).filter(|before| before.end() == clip.start);
        let after = clips.get(at + 1).filter(|after| after.start == clip.end());
        let own_start = board.transition(clip.shot, Edge::Start);
        let own_end = board.transition(clip.shot, Edge::End);
        let into = match (own_start.or(before.and_then(|before| board.transition(before.shot, Edge::End))), before) {
            (Some(transition), Some(_)) => Some((transition.kind, halves(transition.length).1, halves(transition.length).0)),
            (Some(transition), None) => Some((transition.kind, transition.length, 0)),
            _ => None,
        };
        let out = match (own_end.or(after.and_then(|after| board.transition(after.shot, Edge::Start))), after) {
            (Some(transition), Some(_)) => Some((transition.kind, halves(transition.length).0, halves(transition.length).1)),
            (Some(transition), None) => Some((transition.kind, transition.length, 0)),
            _ => None,
        };
        events.push(Event { name: shot.name.clone(), clip: *clip, file, first, duration: shot.duration, into, out });
    }
    events
}

/// A sound clip of the storyboard, as the list has it: its track, its file, where it starts
/// in the film and in its sound, and how long it is, in frames.
struct Heard {
    track: usize,
    name: String,
    file: PathBuf,
    start: u32,
    offset: u32,
    length: u32,
}

fn sounds(project: &Project, board: &Storyboard, folder: Option<&Path>) -> Vec<Heard> {
    let mut heard = Vec::new();
    for (track, of) in board.sound.iter().enumerate().filter(|(_, of)| !of.mute) {
        for clip in &of.clips {
            let Some(media) = project.media(clip.media).filter(|media| matches!(media.kind, MediaKind::Sound { .. })) else { continue };
            heard.push(Heard { track, name: media.name.clone(), file: media.file(folder), start: clip.start, offset: clip.offset, length: clip.length });
        }
    }
    heard
}

/// A reel name as an EDL has it: eight characters of capitals, digits and underscores.
fn reel(name: &str) -> String {
    let plain: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).take(8).collect();
    format!("{plain:<8}")
}

fn edl(project: &Project, board: &Storyboard, events: &[Event], heard: &[Heard]) -> String {
    let rate = project.settings.frame_rate;
    let tc = |frame: i64| timecode(frame, rate);
    // Where a frame of the film is in the record's timecode, and a frame of a shot in its movie's.
    let record = |frame: i64| tc(i64::from(board.start) + frame);
    let mut out = format!("TITLE: {}\nFCM: NON-DROP FRAME\n\n", board.name);
    let mut number = 0;
    let line = |out: &mut String, number: usize, reel: &str, track: &str, kind: &str, source: (i64, i64), at: (i64, i64)| {
        out.push_str(&format!("{number:03}  {reel} {track:<5} {kind:<8} {} {} {} {}\n", tc(source.0), tc(source.1), record(at.0), record(at.1)));
    };
    for event in events {
        let clip = &event.clip;
        let source = |frame: u32| i64::from(frame) - i64::from(event.first);
        let (start, end) = (i64::from(clip.start), i64::from(clip.end()));
        // A dissolve into the shot begins before the cut, on the frames before its in point;
        // the shot before it is cut short by as much.
        let lead = match event.into {
            Some((TransitionKind::CrossDissolve, _, before)) => i64::from(before),
            _ => 0,
        };
        let tail = match event.out {
            Some((TransitionKind::CrossDissolve, before, after)) if after > 0 => i64::from(before),
            _ => 0,
        };
        number += 1;
        match event.into {
            Some((TransitionKind::CrossDissolve, after, before)) if before > 0 || after > 0 => {
                // The outgoing picture at the point the dissolve begins, then this shot dissolved in.
                let from = events.iter().find(|other| other.clip.end() == clip.start);
                let (from_reel, from_at) =
                    from.map_or(("BL      ".to_string(), 0), |from| (reel(&from.name), i64::from(from.clip.out_point) - i64::from(from.first) - lead));
                line(&mut out, number, &from_reel, "V", "C", (from_at, from_at), (start - lead, start - lead));
                let kind = format!("D    {:03}", before + after);
                // The frames before its in point are its handle, if its movie has them.
                let from_frame = source(clip.in_point) - lead;
                line(&mut out, number, &reel(&event.name), "V", &kind, (from_frame.max(0), source(clip.out_point) - tail), (start - lead, end - tail));
                if from_frame < 0 {
                    out.push_str(&format!(
                        "* NOTE: THE MOVIE OF {} HAS NO FRAMES BEFORE ITS IN POINT FOR THIS DISSOLVE. RENDER IT WITH HANDLES.\n",
                        event.name.to_uppercase()
                    ));
                }
            }
            _ => line(&mut out, number, &reel(&event.name), "V", "C", (source(clip.in_point), source(clip.out_point) - tail), (start, end - tail)),
        }
        out.push_str(&format!("* FROM CLIP NAME: {}\n", event.file.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned())));
        out.push_str(&format!("* SOURCE FILE: {}\n", event.file.display()));
        // What an EDL has no word for is said in a line an editor shows as a note.
        for (transition, edge) in [(event.into, "IN"), (event.out, "OUT")] {
            match transition {
                Some((TransitionKind::DipToBlack, mine, other)) => out.push_str(&format!("* DIP TO BLACK {edge}: {} FRAMES\n", mine + other)),
                Some((TransitionKind::DipToWhite, mine, other)) => out.push_str(&format!("* DIP TO WHITE {edge}: {} FRAMES\n", mine + other)),
                _ => {}
            }
        }
    }
    for sound in heard {
        number += 1;
        let track = if sound.track == 0 { "A".to_string() } else { format!("A{}", sound.track + 1) };
        let (offset, start, length) = (i64::from(sound.offset), i64::from(sound.start), i64::from(sound.length));
        line(&mut out, number, &reel(&sound.name), &track, "C", (offset, offset + length), (start, start + length));
        out.push_str(&format!("* FROM CLIP NAME: {}\n* SOURCE FILE: {}\n", sound.name, sound.file.display()));
    }
    out
}

/// A file's place as a link, as the lists that are read by machines want it.
fn link(file: &Path) -> String {
    let path = file.to_string_lossy().replace('\\', "/");
    let escaped: String = path.chars().map(|c| if c == ' ' { "%20".to_string() } else { c.to_string() }).collect();
    if escaped.starts_with('/') { format!("file://{escaped}") } else { format!("file:///{escaped}") }
}

fn otio(project: &Project, board: &Storyboard, events: &[Event], heard: &[Heard]) -> String {
    let rate = project.settings.frame_rate.as_f64();
    let time = |frames: i64| json!({ "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": frames as f64 });
    let range = |start: i64, length: i64| json!({ "OTIO_SCHEMA": "TimeRange.1", "start_time": time(start), "duration": time(length) });
    let gap = |length: i64| json!({ "OTIO_SCHEMA": "Gap.1", "name": "", "source_range": range(0, length), "effects": [], "markers": [], "metadata": {} });
    let transition = |kind: TransitionKind, before: u32, after: u32| {
        let (name, what) = match kind {
            TransitionKind::CrossDissolve => ("Cross Dissolve", "SMPTE_Dissolve"),
            TransitionKind::DipToBlack => ("Dip to Black", "Custom_Transition"),
            TransitionKind::DipToWhite => ("Dip to White", "Custom_Transition"),
        };
        json!({ "OTIO_SCHEMA": "Transition.1", "name": name, "transition_type": what, "in_offset": time(i64::from(before)), "out_offset": time(i64::from(after)), "metadata": {} })
    };
    let reference = |file: &Path, length: i64| json!({ "OTIO_SCHEMA": "ExternalReference.1", "target_url": link(file), "available_range": range(0, length), "metadata": {} });
    let item = |name: &str, file: &Path, whole: i64, start: i64, length: i64| json!({ "OTIO_SCHEMA": "Clip.1", "name": name, "source_range": range(start, length), "media_reference": reference(file, whole), "effects": [], "markers": [], "metadata": {} });
    let track = |name: &str, kind: &str, children: Vec<Value>| json!({ "OTIO_SCHEMA": "Track.1", "name": name, "kind": kind, "children": children, "source_range": null, "effects": [], "markers": [], "metadata": {} });
    // The picture: the shots as they lie, a gap where there is none, a transition at a join.
    let (mut picture, mut at) = (Vec::new(), 0i64);
    for event in events {
        let clip = &event.clip;
        if i64::from(clip.start) > at {
            picture.push(gap(i64::from(clip.start) - at));
        }
        // At a join the transition is written once, by the shot that comes after it.
        if let Some((kind, after, before)) = event.into {
            picture.push(transition(kind, before, after));
        }
        picture.push(item(&event.name, &event.file, i64::from(event.duration), i64::from(clip.in_point) - i64::from(event.first), i64::from(clip.length())));
        // A fade out, on an end that joins nothing.
        if let Some((kind, before, 0)) = event.out {
            picture.push(transition(kind, before, 0));
        }
        at = i64::from(clip.end());
    }
    let mut tracks = vec![track("Shots", "Video", picture)];
    for (index, of) in board.sound.iter().enumerate().filter(|(_, of)| !of.mute) {
        let (mut children, mut at) = (Vec::new(), 0i64);
        for sound in heard.iter().filter(|sound| sound.track == index) {
            if i64::from(sound.start) > at {
                children.push(gap(i64::from(sound.start) - at));
            }
            let whole = project
                .media
                .iter()
                .find(|media| media.name == sound.name)
                .map_or(i64::from(sound.length), |media| i64::from(media.kind.length(project.settings.frame_rate)));
            children.push(item(&sound.name, &sound.file, whole, i64::from(sound.offset), i64::from(sound.length)));
            at = i64::from(sound.start + sound.length);
        }
        tracks.push(track(&of.name, "Audio", children));
    }
    let timeline = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": board.name,
        "global_start_time": time(i64::from(board.start)),
        "tracks": { "OTIO_SCHEMA": "Stack.1", "name": "tracks", "children": tracks, "source_range": null, "effects": [], "markers": [], "metadata": {} },
        "metadata": { "iskra": { "project_frame_rate": project.settings.frame_rate.label() } },
    });
    serde_json::to_string_pretty(&timeline).unwrap_or_default()
}

/// A text as it stands in XML.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn fcpxml(project: &Project, board: &Storyboard, events: &[Event]) -> String {
    let rate = project.settings.frame_rate;
    // A time in frames as FCPXML has it: a fraction of seconds.
    let time = |frames: i64| if frames == 0 { "0s".to_string() } else { format!("{}/{}s", frames * rate.den, rate.num) };
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n<fcpxml version=\"1.9\">\n  <resources>\n");
    out.push_str(&format!(
        "    <format id=\"r1\" name=\"FFVideoFormatRateUndefined\" frameDuration=\"{}/{}s\" width=\"{}\" height=\"{}\"/>\n",
        rate.den, rate.num, project.settings.width, project.settings.height
    ));
    for (at, event) in events.iter().enumerate() {
        out.push_str(&format!(
            "    <asset id=\"r{}\" name=\"{}\" start=\"0s\" duration=\"{}\" hasVideo=\"1\" hasAudio=\"1\" format=\"r1\">\n      <media-rep kind=\"original-media\" src=\"{}\"/>\n    </asset>\n",
            at + 2,
            xml(&event.name),
            time(i64::from(event.duration)),
            xml(&link(&event.file))
        ));
    }
    out.push_str(&format!(
        "  </resources>\n  <library>\n    <event name=\"{name}\">\n      <project name=\"{name}\">\n        <sequence format=\"r1\" duration=\"{}\" tcStart=\"{}\" tcFormat=\"NDF\">\n          <spine>\n",
        time(i64::from(board.end())),
        time(i64::from(board.start)),
        name = xml(&board.name)
    ));
    let mut at = 0i64;
    for (index, event) in events.iter().enumerate() {
        let clip = &event.clip;
        if i64::from(clip.start) > at {
            out.push_str(&format!("            <gap name=\"Gap\" offset=\"{}\" duration=\"{}\"/>\n", time(at), time(i64::from(clip.start) - at)));
        }
        if let Some((kind, after, before)) = event.into {
            let name = match kind {
                TransitionKind::CrossDissolve => "Cross Dissolve",
                TransitionKind::DipToBlack => "Fade to Color",
                TransitionKind::DipToWhite => "Fade to Color",
            };
            out.push_str(&format!(
                "            <transition name=\"{name}\" offset=\"{}\" duration=\"{}\"/>\n",
                time(i64::from(clip.start) - i64::from(before)),
                time(i64::from(before + after))
            ));
        }
        out.push_str(&format!(
            "            <asset-clip ref=\"r{}\" name=\"{}\" offset=\"{}\" start=\"{}\" duration=\"{}\" tcFormat=\"NDF\"/>\n",
            index + 2,
            xml(&event.name),
            time(i64::from(clip.start)),
            time(i64::from(clip.in_point) - i64::from(event.first)),
            time(i64::from(clip.length()))
        ));
        at = i64::from(clip.end());
    }
    out.push_str("          </spine>\n        </sequence>\n      </project>\n    </event>\n  </library>\n</fcpxml>\n");
    out
}

/// A storyboard's cut as an edit list of the kind `format`. The shots refer to their movies:
/// those the project's records know, or else the ones a render of each shot's cut into
/// `movies`, as `movie`, would write. `folder` is the folder the project's file is in, which
/// sound files in it are named from. An FCPXML list has the picture alone.
pub fn edit_list(
    project: &Project,
    storyboard: StoryboardId,
    format: ListFormat,
    movies: &Path,
    movie: Format,
    folder: Option<&Path>,
) -> Result<String, ExportError> {
    let Some(board) = project.storyboard(storyboard) else { return Err(ExportError::Invalid(format!("no storyboard {}", storyboard.0))) };
    if board.clips().is_empty() {
        return Err(ExportError::Invalid(format!("`{}` has no shots to list", board.name)));
    }
    let movie = if movie.is_movie() { movie } else { Format::Mov };
    let events = events(project, board, movies, movie);
    let heard = sounds(project, board, folder);
    Ok(match format {
        ListFormat::Edl => edl(project, board, &events, &heard),
        ListFormat::Otio => otio(project, board, &events, &heard),
        ListFormat::Fcpxml => fcpxml(project, board, &events),
    })
}

/// Write a storyboard's edit list into `dest`, named for the storyboard, beside the shots'
/// movies there (plan §7.14). A list that is there already is written over: it is the cut as
/// it is now. Returns the file.
pub fn export_edit_list(
    project: &Project,
    storyboard: StoryboardId,
    format: ListFormat,
    dest: &Path,
    movie: Format,
    folder: Option<&Path>,
) -> Result<PathBuf, ExportError> {
    let text = edit_list(project, storyboard, format, dest, movie, folder)?;
    let name = project.storyboard(storyboard).map_or("Storyboard".to_string(), |board| file_name(&board.name));
    let path = dest.join(format!("{name}.{}", format.extension()));
    let io = |at: &Path, why: std::io::Error| ExportError::Io { path: at.to_path_buf(), why: why.to_string() };
    std::fs::create_dir_all(dest).map_err(|e| io(dest, e))?;
    std::fs::write(&path, text).map_err(|e| io(&path, e))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_model::{Media, MediaId, SoundClip, SoundClipId, Transition};

    use super::*;

    /// A film at 24 frames a second of two shots end to end and a third after a gap: Shot_01
    /// of 48 frames whose cut is its frames 12 to 36, Shot_02 of 24 from the film's frame 24,
    /// a dissolve of 12 frames between them, and Shot_03 of 24 from frame 60. A line of sound
    /// lies from frame 6, two seconds of it, from half a second in.
    fn film() -> (Project, StoryboardId) {
        let mut project = Project::default();
        let first = project.shots[0].id;
        let (second, third) = (project.new_shot("Shot_02", 24), project.new_shot("Shot_03", 24));
        let (two, three) = (second.id, third.id);
        project.shots.extend([Arc::new(second), Arc::new(third)]);
        let media = MediaId(project.new_id());
        let kind = MediaKind::Sound { sample_rate: 48_000, channels: 1, frames: 48_000 * 4 };
        project.media.push(Arc::new(Media {
            id: media,
            name: "line".to_string(),
            bin: None,
            path: PathBuf::from("Audio/line 1.wav"),
            kind,
            transcript: Vec::new(),
        }));
        let sound = SoundClip { id: SoundClipId(project.new_id()), media, start: 6, offset: 12, length: 48, gain: 1.0 };
        let board = Arc::make_mut(&mut project.storyboards[0]);
        board.put(Clip { shot: first, start: 0, in_point: 12, out_point: 36 }, false).unwrap();
        board.put(Clip { shot: two, start: 24, in_point: 0, out_point: 24 }, false).unwrap();
        board.put(Clip { shot: three, start: 60, in_point: 0, out_point: 24 }, false).unwrap();
        board.transitions.push(Transition { shot: two, edge: Edge::Start, kind: TransitionKind::CrossDissolve, length: 12 });
        board.put_sound(sound, None);
        let id = board.id;
        (project, id)
    }

    #[test]
    fn an_edl_has_an_event_a_shot_with_its_dissolve_and_the_sound() {
        let (project, board) = film();
        let list = edit_list(&project, board, ListFormat::Edl, Path::new("/renders"), Format::Mov, Some(Path::new("/films/Ball"))).unwrap();
        let lines: Vec<&str> = list.lines().filter(|line| line.starts_with(|c: char| c.is_ascii_digit())).collect();
        assert!(list.starts_with("TITLE: Storyboard\nFCM: NON-DROP FRAME\n"), "{list}");
        // The first shot's cut, ended six frames early for the dissolve, from its movie's first frame.
        assert_eq!(lines[0], "001  SHOT_01  V     C        00:00:00:00 00:00:00:18 00:00:00:00 00:00:00:18");
        // The dissolve: the first shot at the point it begins, then the second, six frames before its in point.
        assert_eq!(lines[1], "002  SHOT_01  V     C        00:00:00:18 00:00:00:18 00:00:00:18 00:00:00:18");
        assert_eq!(lines[2], "002  SHOT_02  V     D    012 00:00:00:00 00:00:01:00 00:00:00:18 00:00:02:00");
        assert!(list.contains("* NOTE: THE MOVIE OF SHOT_02 HAS NO FRAMES BEFORE ITS IN POINT"), "a movie of the cut alone has no handle to dissolve from");
        // After the gap, the third; then the sound, from half a second into its file.
        assert_eq!(lines[3], "003  SHOT_03  V     C        00:00:00:00 00:00:01:00 00:00:02:12 00:00:03:12");
        assert_eq!(lines[4], "004  LINE     A     C        00:00:00:12 00:00:02:12 00:00:00:06 00:00:02:06");
        assert!(list.contains("* FROM CLIP NAME: Shot_01.mov") && list.contains("* SOURCE FILE: /films/Ball/Audio/line 1.wav"));
    }

    #[test]
    fn an_otio_list_has_the_tracks_with_gaps_transitions_and_files() {
        let (mut project, board) = film();
        // A movie of the first shot that the project knows of, with four frames of handles.
        let shot = project.shots[0].id;
        let settings = crate::Settings { format: Format::Mp4, range: crate::Range::Cut { handles: 4 }, ..Default::default() };
        let mark = crate::mark_of(&project, iskra_model::Rendered::Shot(shot), &settings).unwrap();
        let report = crate::Report { folder: PathBuf::from("/out"), frames: (8, 40), files: vec![PathBuf::from("/out/Shot_01.mp4")], notes: Vec::new() };
        project.record_render(crate::record_of(iskra_model::Rendered::Shot(shot), &settings, mark, &report, None).unwrap());

        let list = edit_list(&project, board, ListFormat::Otio, Path::new("/renders"), Format::Mov, Some(Path::new("/films/Ball"))).unwrap();
        let read: Value = serde_json::from_str(&list).unwrap();
        assert_eq!((read["OTIO_SCHEMA"].as_str(), read["name"].as_str()), (Some("Timeline.1"), Some("Storyboard")));
        let tracks = read["tracks"]["children"].as_array().unwrap();
        assert_eq!(tracks.iter().map(|track| track["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["Video", "Audio"]);
        let picture: Vec<&str> = tracks[0]["children"].as_array().unwrap().iter().map(|child| child["OTIO_SCHEMA"].as_str().unwrap()).collect();
        assert_eq!(picture, ["Clip.1", "Transition.1", "Clip.1", "Gap.1", "Clip.1"]);
        // The first shot is in the movie that was rendered: its cut starts four frames in.
        let first = &tracks[0]["children"][0];
        assert_eq!(first["media_reference"]["target_url"], "file:///out/Shot_01.mp4");
        assert_eq!((first["source_range"]["start_time"]["value"].as_f64(), first["source_range"]["duration"]["value"].as_f64()), (Some(4.0), Some(24.0)));
        // The second is in the movie a render would write; the dissolve is half before the cut and half after.
        assert_eq!(tracks[0]["children"][2]["media_reference"]["target_url"], "file:///renders/Shot_02.mov");
        let dissolve = &tracks[0]["children"][1];
        assert_eq!(
            (dissolve["transition_type"].as_str(), dissolve["in_offset"]["value"].as_f64(), dissolve["out_offset"]["value"].as_f64()),
            (Some("SMPTE_Dissolve"), Some(6.0), Some(6.0))
        );
        assert_eq!(tracks[0]["children"][3]["source_range"]["duration"]["value"].as_f64(), Some(12.0), "the gap before the third shot");
        // The sound: a gap, then the clip from half a second into its file, whose name has a space.
        let sound = tracks[1]["children"].as_array().unwrap();
        assert_eq!((sound[0]["OTIO_SCHEMA"].as_str(), sound[0]["source_range"]["duration"]["value"].as_f64()), (Some("Gap.1"), Some(6.0)));
        assert_eq!(sound[1]["media_reference"]["target_url"], "file:///films/Ball/Audio/line%201.wav");
        assert_eq!(
            (sound[1]["source_range"]["start_time"]["value"].as_f64(), sound[1]["source_range"]["duration"]["value"].as_f64()),
            (Some(12.0), Some(48.0))
        );
        assert_eq!(read["tracks"]["children"][0]["children"][0]["source_range"]["start_time"]["rate"].as_f64(), Some(24.0));
    }

    #[test]
    fn an_fcpxml_list_has_the_spine_and_a_list_is_written_beside_the_movies() {
        let (project, board) = film();
        let list = edit_list(&project, board, ListFormat::Fcpxml, Path::new("/renders"), Format::Mov, None).unwrap();
        assert!(list.contains("<fcpxml version=\"1.9\">") && list.contains("frameDuration=\"1/24s\""));
        assert!(list.contains("<media-rep kind=\"original-media\" src=\"file:///renders/Shot_01.mov\"/>"));
        // The first shot's cut, a dissolve of half a second across the cut at one second, the gap.
        assert!(list.contains("<asset-clip ref=\"r2\" name=\"Shot_01\" offset=\"0s\" start=\"0s\" duration=\"24/24s\""), "{list}");
        assert!(list.contains("<transition name=\"Cross Dissolve\" offset=\"18/24s\" duration=\"12/24s\"/>"));
        assert!(list.contains("<gap name=\"Gap\" offset=\"48/24s\" duration=\"12/24s\"/>"));
        assert!(list.contains("<asset-clip ref=\"r4\" name=\"Shot_03\" offset=\"60/24s\" start=\"0s\" duration=\"24/24s\""));

        let dir = tempfile::tempdir().unwrap();
        for format in ListFormat::ALL {
            let path = export_edit_list(&project, board, format, dir.path(), Format::Mov, None).unwrap();
            assert_eq!(path, dir.path().join(format!("Storyboard.{}", format.extension())));
            assert!(std::fs::read_to_string(&path).unwrap().len() > 200);
        }
        assert!(edit_list(&project, StoryboardId(999), ListFormat::Edl, dir.path(), Format::Mov, None).is_err());
        assert_eq!((reel("Shot 1: the long name"), xml("a<b & \"c\"")), ("SHOT_1__".to_string(), "a&lt;b &amp; &quot;c&quot;".to_string()));
    }
}
