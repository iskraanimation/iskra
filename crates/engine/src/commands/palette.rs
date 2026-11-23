//! The palette's swatches (plan §4.8). Drawings name swatches by id, so everything here but
//! removing one leaves every drawing as it is.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iskra_model::{Media, MediaId, MediaKind, Palette, Picture, Project, SetId, Style, StyleId, Swatch, SwatchId, SwatchKind, unique_name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{invalid, parse, shot_id, title};
use crate::{Result, Session};

/// A swatch's or a style's name as typed: trimmed, and not empty.
fn checked_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return invalid("a name can't be empty");
    }
    Ok(name)
}

/// The named palette a command names, which the project has to have.
fn set_id(palette: &Palette, set: u16) -> Result<SetId> {
    match palette.set(SetId(set)) {
        Some(set) => Ok(set.id),
        None => invalid(format!("the project has no palette {set}")),
    }
}

/// A named palette's name has to be free among them, or be the one the palette `own` has.
fn set_name_is_free(palette: &Palette, name: &str, own: Option<SetId>) -> Result<()> {
    if palette.sets.iter().any(|set| set.name == name && Some(set.id) != own) {
        return invalid(format!("the project already has a palette named `{name}`"));
    }
    Ok(())
}

fn style_at(palette: &Palette, style: u16) -> Result<usize> {
    match palette.styles.iter().position(|s| s.id == StyleId(style)) {
        Some(at) => Ok(at),
        None => invalid(format!("the palette has no style {style}")),
    }
}

/// A style's name has to be free among the styles of its named palette, or be the one the
/// style `own` has.
fn style_name_is_free(palette: &Palette, set: SetId, name: &str, own: Option<StyleId>) -> Result<()> {
    if palette.styles_of(set).any(|style| style.name == name && Some(style.id) != own) {
        return invalid(format!("the palette already has a style named `{name}`"));
    }
    Ok(())
}

/// Take from every shot the styles that are no longer there, or that `gone` says are not
/// to be shown any more.
fn forget_styles(project: &mut Project, gone: impl Fn(StyleId) -> bool) {
    let palette = project.palette.clone();
    for shot in &mut project.shots {
        if shot.styles.iter().any(|style| gone(*style) || palette.style(*style).is_none()) {
            Arc::make_mut(shot).styles.retain(|style| !gone(*style) && palette.style(*style).is_some());
        }
    }
}

/// Change the project's palette with `f`.
fn edit<T>(s: &mut Session, id: &str, f: impl FnOnce(&mut Palette) -> Result<T>) -> Result<T> {
    s.edit(title(id), |p: &mut Project| f(Arc::make_mut(&mut p.palette)))
}

fn at(palette: &Palette, swatch: u16) -> Result<usize> {
    match palette.swatches.iter().position(|s| s.id == SwatchId(swatch)) {
        Some(at) => Ok(at),
        None => invalid(format!("the palette has no swatch {swatch}")),
    }
}

/// Adds a swatch, after the swatch `after` and in its palette, or at the end of the named
/// `palette`, or of the first. Without a colour it takes that of `after`, or a middle grey;
/// without a name it is "Colour" with the first number that is free.
pub(super) fn add(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        name: Option<String>,
        color: Option<[u8; 4]>,
        after: Option<u16>,
        palette: Option<u16>,
    }
    let id = "palette.add";
    let Params { name, color, after, palette: set } = parse(id, params)?;
    let swatch = edit(s, id, |palette| {
        let after = after.map(|after| at(palette, after)).transpose()?;
        let set = match (set, after) {
            (Some(set), _) => set_id(palette, set)?,
            (None, Some(after)) => palette.set_of(&palette.swatches[after]),
            (None, None) => palette.sets.first().map_or(SetId::DEFAULT, |set| set.id),
        };
        // After the swatch named, if it is in that palette, or else after its last.
        let after = after.filter(|after| palette.set_of(&palette.swatches[*after]) == set);
        let last = palette.swatches.iter().rposition(|swatch| palette.set_of(swatch) == set);
        let name = match &name {
            Some(name) => checked_name(name)?.to_string(),
            None => {
                let free = |name: &String| palette.swatches.iter().all(|swatch| swatch.name != *name);
                (1..).map(|n| format!("Colour {n}")).find(free).expect("a free number")
            }
        };
        let color = color.or(after.map(|after| palette.swatches[after].color)).unwrap_or([128, 128, 128, 255]);
        let Some(id) = palette.new_id() else { return invalid("the palette can hold no more swatches") };
        palette
            .swatches
            .insert(after.or(last).map_or(palette.swatches.len(), |after| after + 1), Swatch { id, name, color, set, kind: SwatchKind::Solid, drift: None });
        Ok(id)
    })?;
    Ok(json!({ "swatch": swatch }))
}

/// Gives a swatch another name, another colour, or both. Every drawing painted with it shows
/// the new colour. With `style`, the colour is the swatch's in that style of the palette, and
/// `reset` takes it from the style, so that the swatch has the palette's own colour there.
/// `kind` makes it solid, a gradient or a picture (plan §4.20), in the style too; without a
/// `color`, a gradient's colour is its first and a picture's its own.
/// With `palette`, the swatch goes to that named palette, at its end. `drift` is how far a vector
/// stroke's colour drifts from it at most, `[hue, saturation, value]` (see [`iskra_model::Swatch`]);
/// `[0, 0, 0]` is none.
pub(super) fn set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        swatch: u16,
        name: Option<String>,
        color: Option<[u8; 4]>,
        style: Option<u16>,
        #[serde(default)]
        reset: bool,
        palette: Option<u16>,
        kind: Option<SwatchKind>,
        drift: Option<[i16; 3]>,
    }
    let id = "palette.set";
    let Params { swatch, name, color, style, reset, palette: set, kind, drift } = parse(id, params)?;
    if drift.is_some_and(|[hue, saturation, value]| !(-180..=180).contains(&hue) || [saturation, value].iter().any(|n| !(-100..=100).contains(n))) {
        return invalid("a drift is a turn of the hue from -180 to 180 degrees, and of saturation and value from -100 to 100");
    }
    if reset && (style.is_none() || color.is_some() || kind.is_some()) {
        return invalid("`reset` takes a swatch's colour from a style: it needs a `style`, and no `color` nor `kind`");
    }
    let kind = kind.map(|kind| checked_kind(s.project(), kind)).transpose()?;
    let folder = super::media::folder(s);
    s.edit(title(id), |p: &mut Project| {
        let media = p.media.clone();
        let palette = Arc::make_mut(&mut p.palette);
        let at = at(palette, swatch)?;
        let style = style.map(|style| style_at(palette, style)).transpose()?;
        if style.is_some_and(|style| palette.styles[style].set != palette.set_of(&palette.swatches[at])) {
            return invalid("that style is of another palette than the swatch");
        }
        if let Some(name) = &name {
            palette.swatches[at].name = checked_name(name)?.to_string();
        }
        // Nothing to add is no drift.
        if let Some(drift) = drift {
            palette.swatches[at].drift = (drift != [0; 3]).then_some(drift);
        }
        if let Some(kind) = &kind {
            read_pictures(palette, &media, folder.as_deref(), Some(kind));
        }
        let color = color.or_else(|| kind.as_ref().and_then(|kind| first_color(palette, kind)));
        match (&kind, style) {
            (Some(kind), Some(style)) if kind.is_solid() => _ = palette.styles[style].kinds.remove(&SwatchId(swatch)),
            (Some(kind), Some(style)) => _ = palette.styles[style].kinds.insert(SwatchId(swatch), kind.clone()),
            (Some(kind), None) => palette.swatches[at].kind = kind.clone(),
            (None, Some(style)) if reset => _ = palette.styles[style].kinds.remove(&SwatchId(swatch)),
            (None, _) => {}
        }
        match (color, style) {
            (Some(color), Some(style)) => _ = palette.styles[style].colors.insert(SwatchId(swatch), color),
            (Some(color), None) => palette.swatches[at].color = color,
            (None, Some(style)) if reset => _ = palette.styles[style].colors.remove(&SwatchId(swatch)),
            (None, _) => {}
        }
        if let Some(set) = set {
            let set = set_id(palette, set)?;
            if palette.set_of(&palette.swatches[at]) != set {
                let moved = Swatch { set, ..palette.swatches.remove(at) };
                // The styles of the palette it leaves have nothing to say of it any more.
                for style in &mut palette.styles {
                    style.colors.remove(&moved.id);
                }
                palette.swatches.push(moved);
            }
        }
        // Pictures no swatch paints with any more are let go.
        read_pictures(palette, &media, folder.as_deref(), None);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// A swatch's kind as a command gives it, checked: a gradient of two colours or more, put
/// in the order of their places; a picture that is an image of the project's media, at a
/// scale above nothing.
fn checked_kind(project: &Project, kind: SwatchKind) -> Result<SwatchKind> {
    match kind {
        SwatchKind::Solid => Ok(kind),
        SwatchKind::Gradient(gradient) if gradient.stops.len() < 2 => {
            invalid(format!("a gradient needs two colours or more, and has {}", gradient.stops.len()))
        }
        SwatchKind::Gradient(gradient) => Ok(SwatchKind::Gradient(gradient.sorted())),
        SwatchKind::Texture(texture) => match project.media(texture.media).map(|media| &media.kind) {
            _ if !(texture.scale.is_finite() && texture.scale > 0.0) => invalid("a texture's scale has to be above 0"),
            Some(MediaKind::Image { .. }) => Ok(SwatchKind::Texture(texture)),
            Some(_) => invalid(format!("media item {} is not a picture", texture.media.0)),
            None => invalid(format!("no media item {}", texture.media.0)),
        },
    }
}

/// The colour a swatch of `kind` has as one: a gradient's first, or the mean of a picture
/// that was read.
fn first_color(palette: &Palette, kind: &SwatchKind) -> Option<[u8; 4]> {
    match kind {
        SwatchKind::Solid => None,
        SwatchKind::Gradient(gradient) => gradient.stops.first().map(|stop| stop.color),
        SwatchKind::Texture(texture) => palette.pictures.get(&texture.media).map(|picture| picture.average()),
    }
}

/// The media of every picture the palette's swatches and styles paint with, and that of
/// `also`.
fn pictured(palette: &Palette, also: Option<&SwatchKind>) -> BTreeSet<MediaId> {
    let kinds = palette.swatches.iter().map(|swatch| &swatch.kind).chain(palette.styles.iter().flat_map(|style| style.kinds.values()));
    let media = |kind: &SwatchKind| match kind {
        SwatchKind::Texture(texture) => Some(texture.media),
        _ => None,
    };
    kinds.chain(also).filter_map(media).collect()
}

/// Read the pictures of the palette's texture swatches, and of `also`, that it has not read
/// yet, from the files of the media they name; and let go of those no swatch paints with. A
/// picture that can't be read is left out, and its swatch shows as its colour.
pub(crate) fn read_pictures(palette: &mut Palette, media: &[Arc<iskra_model::Media>], folder: Option<&std::path::Path>, also: Option<&SwatchKind>) {
    let wanted = pictured(palette, also);
    palette.pictures.retain(|id, _| wanted.contains(id));
    for id in wanted {
        let Some(item) = media.iter().find(|item| item.id == id).filter(|_| !palette.pictures.contains_key(&id)) else { continue };
        if let Ok((width, height, rgba)) = iskra_io::read_picture(&item.file(folder)) {
            palette.pictures.insert(id, Arc::new(Picture::of_rgba(width, height, &rgba)));
        }
    }
}

/// Adds a style to a named palette: its swatches, to be given other colours. It starts with
/// the colours of the style `from` and is of that one's palette, or with the palette's own,
/// of the named `palette` or the first. Without a name it is "Style" with the first number
/// that is free. With `shot`, that shot is shown in the new style, in the same step.
pub(super) fn add_style(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        name: Option<String>,
        from: Option<u16>,
        palette: Option<u16>,
        shot: Option<u64>,
    }
    let id = "palette.addStyle";
    let Params { name, from, palette: set, shot } = parse(id, params)?;
    let style = s.edit(title(id), |p: &mut Project| {
        let shot = shot.map(|shot| shot_id(p, Some(shot))).transpose()?;
        let palette = Arc::make_mut(&mut p.palette);
        let from = from.map(|from| style_at(palette, from)).transpose()?;
        let set = match (from, set) {
            (Some(from), _) => palette.styles[from].set,
            (None, Some(set)) => set_id(palette, set)?,
            (None, None) => palette.sets.first().map_or(SetId::DEFAULT, |set| set.id),
        };
        let colors = from.map(|from| palette.styles[from].colors.clone()).unwrap_or_default();
        let kinds = from.map(|from| palette.styles[from].kinds.clone()).unwrap_or_default();
        let name = match &name {
            Some(name) => checked_name(name)?.to_string(),
            None => (1..).map(|n| format!("Style {n}")).find(|name| palette.styles_of(set).all(|style| style.name != *name)).expect("a free number"),
        };
        style_name_is_free(palette, set, &name, None)?;
        let Some(id) = palette.new_style_id() else { return invalid("the palette can hold no more styles") };
        palette.styles.push(Style { id, name, colors, set, kinds });
        if let Some(shot) = shot {
            show_in(p, shot, Some(set), Some(id));
        }
        Ok(id)
    })?;
    Ok(json!({ "style": style }))
}

/// Gives a style of the palette another name.
pub(super) fn rename_style(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        style: u16,
        name: String,
    }
    let id = "palette.renameStyle";
    let Params { style, name } = parse(id, params)?;
    edit(s, id, |palette| {
        let at = style_at(palette, style)?;
        let name = checked_name(&name)?;
        style_name_is_free(palette, palette.styles[at].set, name, Some(StyleId(style)))?;
        palette.styles[at].name = name.to_string();
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Removes a style of the palette. The shots that were shown in it are shown in the palette's
/// own colours.
pub(super) fn remove_style(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        style: u16,
    }
    let id = "palette.removeStyle";
    let Params { style } = parse(id, params)?;
    s.edit(title(id), |p: &mut Project| {
        let at = style_at(&p.palette, style)?;
        Arc::make_mut(&mut p.palette).styles.remove(at);
        forget_styles(p, |_| false);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Show `shot` in `style` in place of the one of the named palette `set` it was shown in;
/// without a style, that palette is shown in its own colours, and without a palette either,
/// all of them are.
fn show_in(p: &mut Project, shot: iskra_model::ShotId, set: Option<SetId>, style: Option<StyleId>) {
    let palette = p.palette.clone();
    if let Some(shot) = p.shot_mut(shot) {
        shot.styles.retain(|shown| set.is_some_and(|set| palette.style(*shown).is_some_and(|shown| shown.set != set)));
        shot.styles.extend(style);
    }
}

/// Shows a shot in a style, in the window and in renders: the style takes the place of the
/// one of its named palette that the shot was shown in, and the other palettes' stay.
/// Without a `style`, the named `palette` is shown in its own colours, or without that
/// either, all of them are. With `name` in place of a style, every palette that has a style
/// of that name is shown in it, and the others in their own colours, and the layers in the
/// skin of that name, or the day drawings where there is none: a shot switched to *Night* at
/// once. With nothing, all of the palettes in their own colours and the day drawings.
pub(super) fn set_shot_style(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        style: Option<u16>,
        palette: Option<u16>,
        name: Option<String>,
    }
    let id = "shot.setStyle";
    let Params { shot, style, palette: set, name } = parse(id, params)?;
    if name.is_some() && (style.is_some() || set.is_some()) {
        return invalid("`name` is for every palette: it takes no `style` and no `palette`");
    }
    s.edit(title(id), |p: &mut Project| {
        let shot = shot_id(p, shot)?;
        let palette = p.palette.clone();
        if let Some(name) = &name {
            let named: Vec<StyleId> = palette.styles.iter().filter(|style| style.name == name.trim()).map(|style| style.id).collect();
            let skin = p.skin_named(name.trim()).map(|skin| skin.id);
            if named.is_empty() && skin.is_none() {
                return invalid(format!("no palette has a style named `{}`, and no skin is", name.trim()));
            }
            if let Some(shot) = p.shot_mut(shot) {
                (shot.styles, shot.skin) = (named, skin);
            }
            return Ok(());
        }
        if (style, set) == (None, None)
            && let Some(shot) = p.shot_mut(shot)
        {
            shot.skin = None;
        }
        // The named palette whose style is changed; none is all of them.
        let set = match (style, set) {
            (Some(style), _) => Some(palette.styles[style_at(&palette, style)?].set),
            (None, Some(set)) => Some(set_id(&palette, set)?),
            (None, None) => None,
        };
        show_in(p, shot, set, style.map(StyleId));
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Moves a swatch to the place `to` in the palette's order, counted from 0.
pub(super) fn move_to(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        swatch: u16,
        to: usize,
    }
    let id = "palette.move";
    let Params { swatch, to } = parse(id, params)?;
    edit(s, id, |palette| {
        let at = at(palette, swatch)?;
        let moved = palette.swatches.remove(at);
        palette.swatches.insert(to.min(palette.swatches.len()), moved);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Removes a swatch. What is painted with it stays painted and shows as a missing colour,
/// which `palette.usage` lists; the swatch's id is not given to another.
pub(super) fn remove(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        swatch: u16,
    }
    let id = "palette.remove";
    let Params { swatch } = parse(id, params)?;
    edit(s, id, |palette| {
        at(palette, swatch)?;
        if palette.swatches.len() == 1 {
            return invalid("a palette keeps at least one swatch");
        }
        palette.remove(SwatchId(swatch));
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Says which swatches no drawing is painted with (`unused`), and which ids drawings are
/// painted with that the palette lacks (`missing`). The shots that are not open are looked at
/// as the project file has them, so `unread`, which named them, is empty.
pub(super) fn usage(s: &mut Session, _: Value) -> Result<Value> {
    let used = used(s)?;
    let project = s.project();
    let unused: Vec<SwatchId> = project.palette.swatches.iter().map(|swatch| swatch.id).filter(|id| !used.contains(id)).collect();
    let missing: Vec<SwatchId> = used.iter().copied().filter(|id| project.palette.swatch(*id).is_none()).collect();
    // Every shot is looked at, open or not.
    Ok(json!({ "unused": unused, "missing": missing, "unread": [] }))
}

/// The swatches something is painted with: in the shots that are loaded, and as the project
/// file has them in those that are not.
fn used(s: &mut Session) -> Result<BTreeSet<SwatchId>> {
    let project = s.project();
    let mut used: BTreeSet<SwatchId> =
        project.shots.iter().filter(|shot| shot.loaded).flat_map(|shot| shot.drawings()).flat_map(|entry| entry.drawing.swatches()).collect();
    used.extend(s.swatches_in_file()?);
    Ok(used)
}

/// Removes every swatch that no drawing is painted with, of the named `palette` or of all
/// of them; each palette keeps its first swatch where it would be left with none. The shots
/// that are not open are looked at as the project file has them. Says which went.
pub(super) fn remove_unused(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        palette: Option<u16>,
    }
    let id = "palette.removeUnused";
    let Params { palette: set } = parse(id, params)?;
    let used = used(s)?;
    let removed = edit(s, id, |palette| {
        let set = set.map(|set| set_id(palette, set)).transpose()?;
        let mut gone = Vec::new();
        for of in palette.sets.iter().map(|of| of.id).filter(|of| set.is_none_or(|set| set == *of)).collect::<Vec<_>>() {
            let own: Vec<SwatchId> = palette.swatches_of(of).map(|swatch| swatch.id).collect();
            let unused: Vec<SwatchId> = own.iter().copied().filter(|id| !used.contains(id)).collect();
            // A palette is not left with no swatch at all.
            let kept = usize::from(unused.len() == own.len());
            gone.extend(unused.into_iter().skip(kept));
        }
        if gone.is_empty() {
            return invalid("every swatch is used, or is its palette's only one");
        }
        for swatch in &gone {
            palette.remove(*swatch);
        }
        Ok(gone)
    })?;
    Ok(json!({ "removed": removed }))
}

/// Moves a named palette to the place `to` among the palettes, counted from 0.
pub(super) fn move_set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        palette: u16,
        to: usize,
    }
    let id = "palette.reorder";
    let Params { palette: set, to } = parse(id, params)?;
    edit(s, id, |palette| {
        let set = set_id(palette, set)?;
        let at = palette.sets.iter().position(|found| found.id == set).expect("the palette was found");
        let moved = palette.sets.remove(at);
        palette.sets.insert(to.min(palette.sets.len()), moved);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Brings the swatches and the styles of a palette file into the project, by their names:
/// the app's own file, or GIMP's (`.gpl`). They go into the named `palette`, or the first,
/// where a swatch of a name that palette has is left as it is, or with `update` gets the
/// file's colour; or with `asNew` into a palette of their own, called `name` or after the
/// file.
pub(super) fn import(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        path: std::path::PathBuf,
        #[serde(default)]
        update: bool,
        palette: Option<u16>,
        #[serde(default, rename = "asNew")]
        as_new: bool,
        name: Option<String>,
    }
    let id = "palette.import";
    let Params { path, update, palette: into, as_new, name } = parse(id, params)?;
    // A rig's palette has its textures' pictures beside it, which come in as media.
    let rig = path.extension().is_some_and(|ending| ending.eq_ignore_ascii_case(iskra_io::PLT));
    let read = match rig {
        true => None,
        false => match iskra_io::read_palette(&path) {
            Ok(read) => Some(read),
            Err(why) => return invalid(why),
        },
    };
    let pictures = iskra_io::palette_pictures(&path);
    let folder = super::media::folder(s);
    let (merged, into, left) = s.edit(title(id), |p: &mut Project| {
        let (read, left) = match read {
            Some(mut read) => {
                textures_from(p, &mut read, &pictures, folder.as_deref());
                (read, Vec::new())
            }
            None => match plt_into(p, &path, folder.as_deref()) {
                Ok(read) => read,
                Err(why) => return invalid(format!("{} is no palette: {why}", path.display())),
            },
        };
        let media = p.media.clone();
        let palette = Arc::make_mut(&mut p.palette);
        let into = match (as_new, into) {
            (true, _) => {
                // Named as asked, or after the file, with a number where the name is taken.
                let stem = path.file_stem().map_or("Palette".to_string(), |stem| stem.to_string_lossy().into_owned());
                let wanted = name.as_deref().map(str::trim).filter(|name| !name.is_empty()).map_or(stem, str::to_string);
                let free = |name: &String| palette.sets.iter().all(|set| set.name != *name);
                let name = std::iter::once(wanted.clone()).chain((2..).map(|n| format!("{wanted} {n}"))).find(free).expect("a free name");
                match palette.new_set(&name) {
                    Some(set) => set,
                    None => return invalid("the project can hold no more palettes"),
                }
            }
            (false, Some(into)) => set_id(palette, into)?,
            (false, None) => palette.sets.first().map_or(SetId::DEFAULT, |set| set.id),
        };
        let merged = palette.merge(&read, update, into);
        read_pictures(palette, &media, folder.as_deref(), None);
        Ok((merged, into, left))
    })?;
    let mut result = json!({ "added": merged.added, "updated": merged.updated, "styles": merged.styles, "palette": into });
    // What of a rig's palette could not be brought, where there is any.
    if !left.is_empty() {
        result["left"] = json!(left);
    }
    Ok(result)
}

/// A rig's palette file (plan §4.17) read into a palette, with its texture swatches made
/// textures of their pictures (plan §4.20), each brought in as an image of the project's
/// media, or found among them by its file. A texture whose picture can't be read is left out,
/// and named with the colours of kinds that have no colour in what comes back.
pub(crate) fn plt_into(p: &mut Project, path: &std::path::Path, folder: Option<&std::path::Path>) -> std::result::Result<(Palette, Vec<String>), String> {
    let text = std::fs::read_to_string(path).map_err(|why| why.to_string())?;
    let iskra_io::Plt { mut palette, textures, mut left } = iskra_io::palette_of_plt(&text)?;
    for texture in &textures {
        let file = iskra_io::plt_texture_path(path, texture);
        let Some(at) = palette.swatches.iter().position(|swatch| swatch.id == texture.swatch) else { continue };
        let read = iskra_io::probe(&file).and_then(|kind| Ok((kind, iskra_io::read_picture(&file)?)));
        let (kind, (width, height, rgba)) = match read {
            Ok(read) => read,
            Err(_) => {
                left.push(format!("{} (Texture, its picture {} is not there)", palette.swatches[at].name, file.display()));
                palette.swatches.remove(at);
                continue;
            }
        };
        let own = Media::path_from(folder, &file);
        let media = match p.media.iter().find(|media| media.path == own) {
            Some(media) => media.id,
            None => {
                let name = unique_name(&palette.swatches[at].name, p.media.iter().map(|media| media.name.as_str()));
                let id = MediaId(p.new_id());
                p.media.push(Arc::new(Media { id, name, bin: None, path: own, kind, transcript: Vec::new() }));
                id
            }
        };
        let swatch = &mut palette.swatches[at];
        swatch.color = Picture::of_rgba(width, height, &rgba).average();
        swatch.kind = SwatchKind::Texture(iskra_model::SwatchTexture { media, tile: texture.tile, scale: 1.0, once: false });
    }
    if palette.swatches.is_empty() {
        return Err("it has no colours".to_string());
    }
    Ok((palette, left))
}

/// Writes swatches to a file: the app's own, with the styles, or GIMP's by the extension
/// `.gpl`, with the palette's own colours alone. With `palette`, the swatches of that named
/// palette; without, those of all of them.
pub(super) fn export(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        path: std::path::PathBuf,
        palette: Option<u16>,
    }
    let Params { path, palette: set } = parse("palette.export", params)?;
    let all = s.project().palette.clone();
    let written = match set {
        None => Palette::clone(&all),
        Some(set) => {
            let set = set_id(&all, set)?;
            // The one palette, as a file's only one, with what the styles say of its swatches.
            let mut one = Palette::of(all.swatches_of(set).map(|swatch| Swatch { set: SetId::DEFAULT, ..swatch.clone() }).collect());
            one.sets[0].name = all.set(set).map_or(String::new(), |set| set.name.clone());
            one.styles = all.styles_of(set).map(|style| Style { set: SetId::DEFAULT, ..style.clone() }).collect();
            one
        }
    };
    // The files of its textures' pictures, for the project it is brought into to take in.
    let folder = super::media::folder(s);
    let media = |id: MediaId| Some((id, s.project().media(id)?.file(folder.as_deref())));
    let pictures = pictured(&written, None).into_iter().filter_map(media).collect();
    match iskra_io::write_palette_with(&path, &written, &pictures) {
        Ok(()) => Ok(json!({ "swatches": written.swatches.len() })),
        Err(why) => invalid(why),
    }
}

/// A palette brought from a file of the app's own, with its texture swatches made textures of
/// this project's media: their pictures, by the files `pictures` names for the media they had
/// where they were written, brought in or found among the media by their files. A swatch or
/// a style whose picture is not there is solid.
fn textures_from(p: &mut Project, palette: &mut Palette, pictures: &BTreeMap<MediaId, PathBuf>, folder: Option<&Path>) {
    let mut here: BTreeMap<MediaId, Option<MediaId>> = BTreeMap::new();
    for media in pictured(palette, None) {
        let found = pictures.get(&media).and_then(|file| {
            let kind = iskra_io::probe(file).ok().filter(|kind| matches!(kind, MediaKind::Image { .. }))?;
            let own = Media::path_from(folder, file);
            if let Some(media) = p.media.iter().find(|media| media.path == own) {
                return Some(media.id);
            }
            let stem = file.file_stem().map_or("Texture".to_string(), |stem| stem.to_string_lossy().into_owned());
            let name = unique_name(&stem, p.media.iter().map(|media| media.name.as_str()));
            let id = MediaId(p.new_id());
            p.media.push(Arc::new(Media { id, name, bin: None, path: own, kind, transcript: Vec::new() }));
            Some(id)
        });
        here.insert(media, found);
    }
    let kinds = palette.swatches.iter_mut().map(|swatch| &mut swatch.kind).chain(palette.styles.iter_mut().flat_map(|style| style.kinds.values_mut()));
    for kind in kinds {
        if let SwatchKind::Texture(texture) = kind {
            match here.get(&texture.media).copied().flatten() {
                Some(media) => texture.media = media,
                None => *kind = SwatchKind::Solid,
            }
        }
    }
}

/// Adds a named palette to the project, with no swatches yet: a character's, a location's.
/// Without a name it is "Palette" with the first number that is free.
pub(super) fn new_set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        name: Option<String>,
    }
    let id = "palette.new";
    let Params { name } = parse(id, params)?;
    let set = edit(s, id, |palette| {
        let name = match &name {
            Some(name) => checked_name(name)?.to_string(),
            None => (2..).map(|n| format!("Palette {n}")).find(|name| palette.sets.iter().all(|set| set.name != *name)).expect("a free number"),
        };
        set_name_is_free(palette, &name, None)?;
        match palette.new_set(&name) {
            Some(set) => Ok(set),
            None => invalid("the project can hold no more palettes"),
        }
    })?;
    Ok(json!({ "palette": set }))
}

/// Gives a named palette another name.
pub(super) fn rename_set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        palette: u16,
        name: String,
    }
    let id = "palette.rename";
    let Params { palette: set, name } = parse(id, params)?;
    edit(s, id, |palette| {
        let set = set_id(palette, set)?;
        let name = checked_name(&name)?;
        set_name_is_free(palette, name, Some(set))?;
        if let Some(found) = palette.sets.iter_mut().find(|found| found.id == set) {
            found.name = name.to_string();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Removes a named palette with its swatches. What is painted with them stays painted and
/// shows as missing colours. The project keeps at least one palette.
pub(super) fn delete_set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        palette: u16,
    }
    let id = "palette.delete";
    let Params { palette: set } = parse(id, params)?;
    s.edit(title(id), |p: &mut Project| {
        let palette = Arc::make_mut(&mut p.palette);
        let set = set_id(palette, set)?;
        if palette.sets.len() == 1 {
            return invalid("a project keeps at least one palette");
        }
        let gone: Vec<SwatchId> = palette.swatches_of(set).map(|swatch| swatch.id).collect();
        for swatch in gone {
            palette.remove(swatch);
        }
        palette.styles.retain(|style| style.set != set);
        palette.sets.retain(|kept| kept.id != set);
        // The shots that were shown in a style of it are shown without.
        forget_styles(p, |_| false);
        Ok(())
    })?;
    Ok(Value::Null)
}
