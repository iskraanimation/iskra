//! Media commands (plan §7.11, R7): the pictures and sound that are brought into the project.
//! A media item refers to its file; nothing of the file is copied into the project.

use std::path::PathBuf;
use std::sync::Arc;

use iskra_model::{Drawing, DrawingId, LayerId, Media, MediaId, MediaKind, PoolDrawing, Role, unique_name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::bin::bin_in;
use super::{checked_frames, checked_name, invalid, loaded, parse, shot_id, title};
use crate::{Result, Session};

/// The folder the project's file is in, which media in it are named from.
pub(super) fn folder(s: &Session) -> Option<PathBuf> {
    s.path().and_then(|file| file.parent()).map(PathBuf::from)
}

fn media_of(project: &iskra_model::Project, media: u64) -> Result<MediaId> {
    match project.media(MediaId(media)) {
        Some(found) => Ok(found.id),
        None => invalid(format!("no media item {media}")),
    }
}

/// Gives a sound its transcript (plan §4.7): what is spoken in it, with when. Either `path`,
/// a subtitle file (`.srt`, `.vtt`) or a list of words as JSON, or `words`, each with its
/// `text` and its `start` and `end` in seconds from the start of the sound. It takes the place
/// of the transcript the sound had; no words take it away. Returns how many `words` it has.
pub(super) fn set_transcript(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Word {
        text: String,
        start: f64,
        end: Option<f64>,
    }
    #[derive(Deserialize)]
    struct Params {
        media: u64,
        path: Option<PathBuf>,
        words: Option<Vec<Word>>,
    }
    let id = "media.setTranscript";
    let Params { media, path, words } = parse(id, params)?;
    let ms = |seconds: f64| (seconds.max(0.0) * 1000.0).round().min(f64::from(u32::MAX)) as u32;
    let mut spoken: Vec<iskra_model::Spoken> = match (path, words) {
        (Some(path), None) => iskra_io::read_transcript(&path).map_err(crate::EngineError::Invalid)?,
        (None, Some(words)) => {
            if words.iter().any(|word| !word.start.is_finite() || word.end.is_some_and(|end| !end.is_finite())) {
                return invalid("a word's `start` and `end` are numbers of seconds");
            }
            let spoken = words.iter().map(|word| {
                let text = word.text.split_whitespace().collect::<Vec<_>>().join(" ");
                iskra_model::Spoken { text, start: ms(word.start), end: ms(word.end.unwrap_or(word.start)).max(ms(word.start)) }
            });
            spoken.filter(|spoken| !spoken.text.is_empty()).collect()
        }
        _ => return invalid("give the transcript as `path`, a subtitle file, or as `words`"),
    };
    spoken.sort_by_key(|spoken| spoken.start);
    let count = spoken.len();
    s.edit(title(id), |p| {
        let media = media_of(p, media)?;
        let Some(item) = p.media.iter_mut().find(|item| item.id == media) else { return Ok(()) };
        if !matches!(item.kind, MediaKind::Sound { .. }) {
            return invalid(format!("`{}` is not a sound: only what is heard has a transcript", item.name));
        }
        Arc::make_mut(item).transcript = spoken;
        Ok(())
    })?;
    Ok(json!({ "words": count }))
}

/// Brings files in as media items: each of `paths` a picture or a sound, into `bin` or else
/// Master. With `sequence`, a picture that is the first of numbered pictures comes in as one
/// image sequence of them all. Each item is named after its file. Returns the ids of the
/// items made, as `media`, and as `problems` a line for each file that could not be read; if
/// none could, nothing is done and the first problem is the error.
pub(super) fn import(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        paths: Vec<PathBuf>,
        #[serde(default)]
        sequence: bool,
        bin: Option<u64>,
    }
    let id = "media.import";
    let Params { paths, sequence, bin } = parse(id, params)?;
    let (mut read, mut problems) = (Vec::new(), Vec::new());
    for path in paths {
        let kind = if sequence && iskra_io::sequence_from(&path).is_some() { iskra_io::probe_sequence(&path) } else { iskra_io::probe(&path) };
        match kind {
            Ok(kind) => read.push((path, kind)),
            Err(why) => problems.push(why),
        }
    }
    if read.is_empty() {
        return invalid(problems.into_iter().next().unwrap_or_else(|| "no files were given to bring in".to_string()));
    }
    let folder = folder(s);
    let made = s.edit(title(id), |p| {
        let bin = bin_in(p, bin)?;
        let mut made = Vec::new();
        for (path, kind) in &read {
            let stem = path.file_stem().map_or("Media".to_string(), |stem| stem.to_string_lossy().into_owned());
            let name = unique_name(&stem, p.media.iter().map(|media| media.name.as_str()));
            let id = MediaId(p.new_id());
            p.media.push(Arc::new(Media { id, name, bin, path: Media::path_from(folder.as_deref(), path), kind: kind.clone(), transcript: Vec::new() }));
            made.push(id);
        }
        Ok(made)
    })?;
    Ok(json!({ "media": made, "problems": problems }))
}

/// Gives a media item another name. Names are unique among the project's media.
pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        media: u64,
        name: String,
    }
    let id = "media.rename";
    let Params { media, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        let media = media_of(p, media)?;
        if p.media.iter().any(|other| other.id != media && other.name == name) {
            return invalid(format!("there is a media item named `{name}` already"));
        }
        if let Some(media) = p.media_mut(media) {
            media.name = name.to_string();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Takes a media item out of the project, and with it the sound clips that are of it. Its
/// file is left as it is, and so are the layers that were placed from it.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        media: u64,
    }
    let id = "media.delete";
    let Params { media } = parse(id, params)?;
    s.edit(title(id), |p| {
        let media = media_of(p, media)?;
        p.media.retain(|other| other.id != media);
        for board in &mut p.storyboards {
            if board.sound.iter().flat_map(|track| &track.clips).any(|clip| clip.media == media) {
                for track in &mut Arc::make_mut(board).sound {
                    track.clips.retain(|clip| clip.media != media);
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Points a media item at another file, `path`: one that was moved or renamed, or a newer
/// take. The file has to be of the item's kind, a picture for a picture and a sound for a
/// sound; what is read of it replaces what the item knew.
pub(super) fn relink(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        media: u64,
        path: PathBuf,
    }
    let id = "media.relink";
    let Params { media, path } = parse(id, params)?;
    let was = match s.project().media(MediaId(media)) {
        Some(found) => found.kind.clone(),
        None => return invalid(format!("no media item {media}")),
    };
    let found = match was {
        MediaKind::ImageSequence { .. } => iskra_io::probe_sequence(&path),
        _ => iskra_io::probe(&path),
    };
    let kind = match found {
        Ok(kind) if std::mem::discriminant(&kind) == std::mem::discriminant(&was) => kind,
        Ok(kind) => return invalid(format!("{} is {}, and the item is {}", path.display(), kind.name().to_lowercase(), was.name().to_lowercase())),
        Err(why) => return invalid(why),
    };
    let folder = folder(s);
    s.edit(title(id), |p| {
        if let Some(media) = p.media_mut(MediaId(media)) {
            (media.path, media.kind) = (Media::path_from(folder.as_deref(), &path), kind);
        }
        // A texture swatch's picture is read again from where the item is now.
        if p.palette.pictures.contains_key(&MediaId(media)) {
            let palette = Arc::make_mut(&mut p.palette);
            palette.pictures.remove(&MediaId(media));
            super::palette::read_pictures(palette, &p.media, folder.as_deref(), None);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// The pictures of a media item, read from its files: for a picture, the one; for a sequence,
/// one for each of its frames, `None` where a number has no file. Their alpha is read as
/// `alpha` says, or left out where they are `opaque`.
fn pictures(s: &Session, media: &Media, opaque: bool, alpha: iskra_model::Alpha) -> Result<Vec<Option<Arc<Drawing>>>> {
    let file = media.file(folder(s).as_deref());
    let read = |path: &std::path::Path| match iskra_io::read_picture(path) {
        Ok((width, height, mut rgba)) => {
            alpha.read(opaque, &mut rgba);
            Ok(Arc::new(Drawing::of_picture(width, height, &rgba)))
        }
        Err(why) => invalid(why),
    };
    match media.kind {
        MediaKind::Image { .. } => Ok(vec![Some(read(&file)?)]),
        MediaKind::ImageSequence { .. } => {
            // With the files after it gone, the first is all there is of the sequence.
            let frames = iskra_io::sequence_from(&file).unwrap_or_else(|| vec![Some(file.clone())]);
            frames.iter().map(|frame| frame.as_deref().map(read).transpose()).collect()
        }
        MediaKind::Sound { .. } => invalid(format!("`{}` is a sound: it goes on a sound track, not into the picture", media.name)),
    }
}

/// Places a picture or an image sequence into a shot, as a new layer on top of what the shot
/// shows, named after the media item. A pixel of the picture is a pixel of the shot, and the
/// picture's middle is the frame's. A picture shows for the whole shot; a sequence shows a
/// frame a file from `frame` on (0 if not given), with an empty cell where a number has no
/// file. The pixels are read now and kept in the project like drawings; the layer remembers
/// the media item, so that `layer.refreshMedia` can read them again. Returns the layer's `id`.
pub(super) fn place(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        media: u64,
        shot: Option<u64>,
        #[serde(default)]
        frame: u32,
    }
    let id = "media.place";
    let Params { media, shot, frame } = parse(id, params)?;
    checked_frames(frame, 1)?;
    let Some(media) = s.project().media(MediaId(media)).cloned() else { return invalid(format!("no media item {media}")) };
    let shot = shot_id(s.project(), shot)?;
    if let Some(shot) = s.project().shot(shot) {
        loaded(shot)?;
    }
    let pictures = pictures(s, &media, false, iskra_model::Alpha::Straight)?;
    let layer = s.edit(title(id), |p| {
        let name = unique_name(&media.name, p.shot(shot).into_iter().flat_map(|shot| shot.node_names()));
        let mut layer = p.new_layer(&name, Role::Animation);
        (layer.density, layer.media) = (1.0, Some(media.id));
        let whole = p.shot(shot).map_or(1, |shot| shot.duration.max(1));
        for (index, picture) in pictures.iter().enumerate() {
            let Some(picture) = picture else { continue };
            let drawing = DrawingId(p.new_id());
            layer.drawings.push(PoolDrawing { id: drawing, name: (index + 1).to_string(), drawing: picture.clone() });
            // A picture is held for the shot; a frame of a sequence for its one frame.
            let (start, length) = if pictures.len() == 1 { (0, whole) } else { (frame + index as u32, 1) };
            layer.exposure.set(start, length, Some(drawing));
        }
        let id = layer.id;
        if let Some(shot) = p.shot_mut(shot) {
            shot.layers.insert(0, Arc::new(layer));
            shot.connect_on_top(id);
        }
        Ok(id)
    })?;
    Ok(json!({ "id": layer }))
}

/// Reads a placed layer's pictures again from its media item's files: after the file was
/// painted over, or the item was pointed at another. Each drawing gets the picture that has
/// its number; what was drawn over a picture, on its line art and overart, stays. A frame whose file is
/// gone keeps the picture it has.
pub(super) fn refresh(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
    }
    let id = "layer.refreshMedia";
    let Params { layer } = parse(id, params)?;
    read_again(s, id, LayerId(layer), None)
}

/// Says whether a layer of pictures brought in has their alpha, `transparent`, and how their
/// files keep their colours with it, `alpha`; and reads the pictures again so, as
/// `layer.refreshMedia` does. Either may be left out, to stay as it is.
pub(super) fn set_transparency(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        transparent: Option<bool>,
        alpha: Option<iskra_model::Alpha>,
    }
    let id = "layer.setTransparency";
    let Params { layer, transparent, alpha } = parse(id, params)?;
    let Some(now) = s.project().layer(LayerId(layer)) else { return invalid(format!("no layer {layer}")) };
    let (opaque, alpha) = (transparent.map_or(now.opaque, |transparent| !transparent), alpha.unwrap_or(now.alpha));
    read_again(s, id, LayerId(layer), Some((opaque, alpha)))
}

/// Read a layer's pictures again from its media item's files, as `layer.refreshMedia` says,
/// with their alpha as `to` says, or as the layer has it.
fn read_again(s: &mut Session, id: &str, layer: LayerId, to: Option<(bool, iskra_model::Alpha)>) -> Result<Value> {
    let found = s.project().shots.iter().find_map(|shot| Some((shot.id, shot.layer(layer)?)));
    let Some((shot, (opaque, alpha), placed)) = found.map(|(shot, layer)| (shot, to.unwrap_or((layer.opaque, layer.alpha)), layer.media)) else {
        return invalid(format!("no layer {}", layer.0));
    };
    let Some(media) = placed.and_then(|media| s.project().media(media)).cloned() else {
        return invalid("the layer was not placed from a media item, or the item is gone from the project");
    };
    if let Some(shot) = s.project().shot(shot) {
        loaded(shot)?;
    }
    // A picture whose file is gone has its transparency changed from the pixels it kept.
    let gone = to.is_some() && !media.file(folder(s).as_deref()).exists();
    let pictures = if gone { Vec::new() } else { pictures(s, &media, opaque, alpha)? };
    let [width, height] = media.kind.size().unwrap_or_default();
    let whole = iskra_model::Rect::new(-(width as i32 / 2), -(height as i32 / 2), width as i32 - width as i32 / 2, height as i32 - height as i32 / 2);
    s.edit(title(id), |p| {
        let Some(layer) = p.layer_mut(layer) else { return Ok(()) };
        let was = (layer.opaque, layer.alpha);
        (layer.opaque, layer.alpha) = (opaque, alpha);
        for entry in &mut layer.drawings {
            // A drawing is named by the number of its frame in the sequence, from 1.
            let fresh = entry.name.parse::<usize>().ok().and_then(|number| pictures.get(number.checked_sub(1)?)).cloned().flatten();
            let fresh = match fresh {
                Some(fresh) => fresh,
                None if was == (opaque, alpha) => continue,
                None if was.0 && !opaque => {
                    return invalid(format!(
                        "the file of `{}` is gone, and its pictures were read without their alpha: find the file to give it back",
                        media.name
                    ));
                }
                None => Arc::new(kept_again(&entry.drawing, whole, was, (opaque, alpha))),
            };
            let mut drawing = Drawing::clone(&fresh);
            for kind in [iskra_model::ArtLayerKind::LineArt, iskra_model::ArtLayerKind::Overart] {
                if let (Some(over), Some(kept)) = (drawing.layer_mut(kind), entry.drawing.layer(kind)) {
                    over.content = kept.content.clone();
                }
            }
            entry.drawing = Arc::new(drawing);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Makes a skin of a folder of pictures (plan §4.8): of each file of the media item a layer was
/// placed from, the file of the same name in `folder` is the variant of the drawing it is, in
/// the skin `name`, made if the palette has none so called; without a name, the folder's.
/// The line art and overart drawn over the day picture stay over the night one. Matched by
/// the files' names this once and by the drawings' ids after, so a drawing renamed keeps its
/// variant; a variant there is is taken over. Returns the `skin`, how many variants were
/// `made`, and the files the folder `lacks`.
pub(super) fn skin_from_folder(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        layer: u64,
        folder: PathBuf,
        name: Option<String>,
    }
    let id = "skin.fromFolder";
    let Params { layer, folder: from, name } = parse(id, params)?;
    let Some(found) = s.project().layer(LayerId(layer)).cloned() else { return invalid(format!("no layer {layer}")) };
    let Some(media) = found.media.and_then(|media| s.project().media(media)).cloned() else {
        return invalid("the layer was not placed from a media item, or the item is gone from the project");
    };
    if let Some(shot) = s.project().shot_of(LayerId(layer)) {
        loaded(shot)?;
    }
    if !from.is_dir() {
        return invalid(format!("{} is no folder", from.display()));
    }
    let name = match name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => from.file_name().map_or("Skin".to_string(), |name| name.to_string_lossy().into_owned()),
    };
    // The folder `night` is the skin Night, where there is one.
    let name = s.project().skins().iter().find(|skin| skin.name.eq_ignore_ascii_case(&name)).map_or(name, |skin| skin.name.clone());
    // The day files, a frame of the sequence each, numbered from 1 as the drawings are named.
    let file = media.file(folder(s).as_deref());
    let files: Vec<Option<PathBuf>> = match media.kind {
        MediaKind::ImageSequence { .. } => iskra_io::sequence_from(&file).unwrap_or_else(|| vec![Some(file.clone())]),
        _ => vec![Some(file)],
    };
    let mut lacks = Vec::new();
    let mut read = Vec::new();
    for (index, day) in files.iter().enumerate() {
        let Some(named) = day.as_ref().and_then(|day| day.file_name()) else { continue };
        let Some(entry) = found.drawings.iter().find(|entry| entry.name == (index + 1).to_string()) else { continue };
        let night = from.join(named);
        if !night.is_file() {
            lacks.push(named.to_string_lossy().into_owned());
            continue;
        }
        let (width, height, mut rgba) = match iskra_io::read_picture(&night) {
            Ok(read) => read,
            Err(why) => return invalid(why),
        };
        found.alpha.read(found.opaque, &mut rgba);
        let mut drawing = Drawing::of_picture(width, height, &rgba);
        for kind in [iskra_model::ArtLayerKind::LineArt, iskra_model::ArtLayerKind::Overart] {
            if let (Some(over), Some(kept)) = (drawing.layer_mut(kind), entry.drawing.layer(kind)) {
                over.content = kept.content.clone();
            }
        }
        read.push((entry.clone(), drawing));
    }
    if read.is_empty() {
        return invalid(format!("{} has none of the layer's files", from.display()));
    }
    let made = read.len();
    let skin = s.edit(title(id), |p| {
        let skin = super::character::skin_called(p, &name);
        for (entry, drawing) in read {
            let id = DrawingId(p.new_id());
            let Some(layer) = p.layer_mut(LayerId(layer)) else { continue };
            layer.variants.retain(|variant| !(variant.skin == skin && variant.of == entry.id));
            layer.variants.push(iskra_model::Variant { skin, of: entry.id, drawing: PoolDrawing { id, drawing: Arc::new(drawing), ..entry } });
        }
        Ok(skin)
    })?;
    Ok(json!({ "skin": skin.0, "made": made, "lacks": lacks }))
}

/// A picture's drawing with its transparency changed from `was` to `to`, from the pixels it
/// kept of the picture `whole`, where its file is gone.
fn kept_again(drawing: &Drawing, whole: iskra_model::Rect, was: (bool, iskra_model::Alpha), to: (bool, iskra_model::Alpha)) -> Drawing {
    let mut pixels = vec![[0u8; 4]; whole.width() as usize * whole.height() as usize];
    if let Some(iskra_model::ArtContent::Rgba(surface)) = drawing.layer(iskra_model::ArtLayerKind::ColourArt).map(|art| &art.content) {
        surface.read_rgba8_into(whole, &mut pixels);
    }
    // As the file had them, and read again as it is to be.
    if !was.0 {
        was.1.write(pixels.as_flattened_mut());
    }
    to.1.read(to.0, pixels.as_flattened_mut());
    Drawing::of_picture_at(whole.x0, whole.y0, whole.width(), whole.height(), pixels.as_flattened())
}
