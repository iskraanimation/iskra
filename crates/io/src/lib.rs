//! Import and export. For now: writing a project's drawings, timing and palette as ordinary
//! files, which is what the `iskra-extract` recovery tool does (plan §4.9).

pub mod abr_map;
mod brush_files;
mod character;
mod media;
mod ora;
mod palette_files;
mod rig_convert;
mod rig_import;
mod transcripts;
mod zip;

use std::collections::HashSet;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use iskra_drawing::Rect;
use iskra_model::{Palette, PoolDrawing, Project};
use serde_json::{Value, json};

pub use brush_files::{
    ABR, BrushImport, EXTENSION as BRUSHES_EXTENSION, abr_v2, abr_v6, brushes_of_abr, brushes_of_own, is_brush_file, read_brushes, read_brushes_with,
    write_brushes,
};
pub use character::{CHARACTER_EXTENSIONS, Character, Imported, Part as CharacterPart, is_character, read_character};
pub use media::{IMAGE_EXTENSIONS, SOUND_EXTENSIONS, is_media, probe, probe_sequence, read_picture, sequence_from};
pub use palette_files::{
    ACO, ASE, GPL, PALETTE_EXTENSION, PALETTE_EXTENSIONS, PLT, Plt, PltTexture, gpl_of_palette, is_palette_file, own_of_palette, palette_of_aco,
    palette_of_ase, palette_of_gpl, palette_of_own, palette_of_plt, palette_pictures, plt_texture_path, read_palette, write_palette, write_palette_with,
};
pub use rig_convert::{Converted, DrawingConverter, Pass as RigPass, Program as ConverterProgram, convert_drawings, drawing_count, picture_path};
pub use rig_import::{
    Attr as RigAttr, Backdrop as RigBackdrop, Element as RigElement, GROUP as RIG_GROUP, Group as RigGroup, Key as RigKey, Link as RigLink, RigNode, RigScene,
    Wired as RigWired,
};
pub use transcripts::{TRANSCRIPT_EXTENSIONS, read_transcript, transcript_of_json, transcript_of_subtitles};

/// What an export wrote and what it could not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportReport {
    pub drawings_written: usize,
    /// Drawings with nothing painted on them: listed in the timing, but they get no image.
    pub empty_drawings: usize,
    /// One line per file that could not be written, saying which and why.
    pub problems: Vec<String>,
}

/// A name as a file name: characters that are awkward in paths become `_`, and a name already
/// in `used` gets `-<id>` added so two drawings never share a file.
fn file_name(name: &str, id: u64, used: &mut HashSet<String>) -> String {
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || " -_.".contains(c) { c } else { '_' }).collect();
    let safe = safe.trim_matches([' ', '.']).to_string();
    let safe = if safe.is_empty() { id.to_string() } else { safe };
    let unique = if used.contains(&safe.to_lowercase()) { format!("{safe}-{id}") } else { safe };
    used.insert(unique.to_lowercase());
    unique
}

/// Straight-alpha RGBA8 pixels, in rows, as the bytes of a PNG file.
pub fn encode_png(width: u32, height: u32, pixels: &[[u8; 4]]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    write_png_to(&mut bytes, width, height, pixels)?;
    Ok(bytes)
}

/// The picture in a PNG file's bytes: its width and height, and its pixels as straight-alpha
/// RGBA8 in rows.
pub fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<[u8; 4]>), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // Whatever kind of PNG it is, it is read as 8 bits a channel.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut data = vec![0; reader.output_buffer_size().ok_or("the picture is too large")?];
    let frame = reader.next_frame(&mut data).map_err(|e| e.to_string())?;
    let data = &data[..frame.buffer_size()];
    let pixels = match frame.color_type {
        png::ColorType::Rgba => data.as_chunks::<4>().0.to_vec(),
        png::ColorType::Rgb => data.as_chunks::<3>().0.iter().map(|&[r, g, b]| [r, g, b, 255]).collect(),
        png::ColorType::GrayscaleAlpha => data.as_chunks::<2>().0.iter().map(|&[v, a]| [v, v, v, a]).collect(),
        png::ColorType::Grayscale => data.iter().map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::Indexed => return Err("the picture's colours could not be read".to_string()),
    };
    Ok((frame.width, frame.height, pixels))
}

fn write_png_to(to: impl std::io::Write, width: u32, height: u32, pixels: &[[u8; 4]]) -> Result<(), String> {
    let mut encoder = png::Encoder::new(to, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(pixels.as_flattened()).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

/// Write straight-alpha RGBA8 pixels as a PNG.
fn write_png(path: &Path, width: u32, height: u32, pixels: &[[u8; 4]]) -> Result<(), String> {
    write_png_to(BufWriter::new(File::create(path).map_err(|e| e.to_string())?), width, height, pixels)
}

/// Write one drawing as a PNG of its painted area, with the palette's colours baked in.
/// Returns the area, or `None` for a drawing with nothing painted.
fn export_drawing(entry: &PoolDrawing, palette: &Palette, path: &Path) -> Result<Option<Rect>, String> {
    let bounds = entry.drawing.bounds();
    if bounds.is_empty() {
        return Ok(None);
    }
    let pixels = palette.paint(&entry.drawing, bounds, iskra_model::WorkingSpace::Srgb);
    write_png(path, bounds.width(), bounds.height(), &pixels)?;
    Ok(Some(bounds))
}

/// Export a project into the folder `out`, which is created:
///
/// - `drawings/<shot>/<layer>/<drawing>.png`: every drawing, cropped to what is painted;
/// - `timing.json`: per shot and layer, the drawings (with each image's position in drawing
///   pixels), the exposure runs and the peg it is parented to; per shot its pegs with their
///   transforms and keyframes, and which layers are seen, top first; and per storyboard where each shot sits in the cut: enough to rebuild the
///   animation elsewhere;
/// - `palette.json`: the swatches, each with the named palette it is in;
/// - `styles.json`: the palette's styles, each with the named palette it is of and the
///   colours it gives its swatches, by swatch id. Each shot in `timing.json` names the styles
///   it is shown in, and its drawings are written in the colours those give.
///
/// A drawing's variants in skins (plan §4.8) are written beside it, each in a folder named
/// after its skin (`Night/1.png`), and listed with it under `skins`, by the skin's name; each
/// shot names the skin it is shown in. The timing names the day drawings.
///
/// A file that can't be written is reported and the export carries on.
pub fn export_project(project: &Project, out: &Path) -> std::io::Result<ExportReport> {
    let mut report = ExportReport::default();
    std::fs::create_dir_all(out)?;
    let mut shots = Vec::new();
    let mut shot_names = HashSet::new();
    for shot in &project.shots {
        // The colours of the palette style the shot is shown in.
        let colours = project.palette.styled(&shot.styles);
        let shot_dir = PathBuf::from("drawings").join(file_name(&shot.name, shot.id.0, &mut shot_names));
        let mut layers = Vec::new();
        let mut layer_names = HashSet::new();
        for layer in &shot.layers {
            let dir = shot_dir.join(file_name(&layer.name, layer.id.0, &mut layer_names));
            if let Err(e) = std::fs::create_dir_all(out.join(&dir)) {
                report.problems.push(format!("{}: {e}", dir.display()));
                continue;
            }
            let mut drawings = Vec::new();
            let mut names = HashSet::new();
            for entry in &layer.drawings {
                let file = dir.join(format!("{}.png", file_name(&entry.name, entry.id.0, &mut names)));
                let mut listed = json!({ "id": entry.id.0, "name": entry.name, "file": null });
                match export_drawing(entry, &colours, &out.join(&file)) {
                    Ok(Some(bounds)) => {
                        report.drawings_written += 1;
                        // Forward slashes, so the listing reads the same on every system.
                        listed["file"] = json!(file.iter().map(|part| part.to_string_lossy()).collect::<Vec<_>>().join("/"));
                        listed["x"] = json!(bounds.x0);
                        listed["y"] = json!(bounds.y0);
                        listed["width"] = json!(bounds.width());
                        listed["height"] = json!(bounds.height());
                    }
                    Ok(None) => report.empty_drawings += 1,
                    Err(e) => report.problems.push(format!("{}: {e}", file.display())),
                }
                // Its variants, each in its skin's folder.
                let mut skins = serde_json::Map::new();
                for variant in layer.variants.iter().filter(|variant| variant.of == entry.id) {
                    let Some(skin) = project.skin(variant.skin) else { continue };
                    let folder = dir.join(file_name(&skin.name, skin.id.0, &mut HashSet::new()));
                    let file = folder.join(format!("{}.png", file_name(&entry.name, entry.id.0, &mut HashSet::new())));
                    if let Err(e) = std::fs::create_dir_all(out.join(&folder)) {
                        report.problems.push(format!("{}: {e}", folder.display()));
                        continue;
                    }
                    let mut one = json!({ "id": variant.drawing.id.0, "file": null });
                    match export_drawing(&variant.drawing, &colours, &out.join(&file)) {
                        Ok(Some(bounds)) => {
                            report.drawings_written += 1;
                            one["file"] = json!(file.iter().map(|part| part.to_string_lossy()).collect::<Vec<_>>().join("/"));
                            one["x"] = json!(bounds.x0);
                            one["y"] = json!(bounds.y0);
                            one["width"] = json!(bounds.width());
                            one["height"] = json!(bounds.height());
                        }
                        Ok(None) => report.empty_drawings += 1,
                        Err(e) => report.problems.push(format!("{}: {e}", file.display())),
                    }
                    skins.insert(skin.name.clone(), one);
                }
                if !skins.is_empty() {
                    listed["skins"] = Value::Object(skins);
                }
                drawings.push(listed);
            }
            let exposure: Vec<Value> = layer.exposure.runs().iter().map(|r| json!({ "start": r.start, "length": r.len, "drawing": r.drawing.0 })).collect();
            let parent = layer.parent.and_then(|id| shot.peg(id)).map(|peg| &peg.name);
            layers.push(json!({
                "name": layer.name, "role": layer.role, "parent": parent, "pixelsPerUnit": layer.density, "drawings": drawings, "exposure": exposure,
            }));
        }
        let peg = |peg: &std::sync::Arc<iskra_model::Peg>| {
            let parent = peg.parent.and_then(|id| shot.peg(id)).map(|parent| &parent.name);
            json!({ "name": peg.name, "parent": parent, "transform": peg.transform, "keyframes": peg.animation })
        };
        let pegs: Vec<Value> = shot.pegs.iter().map(peg).collect();
        // Which layers are seen, top first: the display's node with its composites opened up.
        let stack: Vec<&str> = shot.stack().into_iter().filter_map(|id| shot.layer(id)).map(|layer| layer.name.as_str()).collect();
        let styles: Vec<&str> = shot.styles.iter().filter_map(|id| project.palette.style(*id)).map(|style| style.name.as_str()).collect();
        let skin = shot.skin.and_then(|skin| project.skin(skin)).map(|skin| skin.name.as_str());
        shots.push(
            json!({ "name": shot.name, "durationFrames": shot.duration, "stack": stack, "styles": styles, "skin": skin, "pegs": pegs, "layers": layers }),
        );
    }
    let shot_name = |id| project.shot(id).map(|s| s.name.as_str());
    let storyboards: Vec<Value> = project
        .storyboards
        .iter()
        .map(|storyboard| {
            let clips: Vec<Value> =
                storyboard.clips().iter().map(|c| json!({ "shot": shot_name(c.shot), "start": c.start, "in": c.in_point, "out": c.out_point })).collect();
            let transitions: Vec<Value> =
                storyboard.transitions.iter().map(|t| json!({ "shot": shot_name(t.shot), "edge": t.edge, "kind": t.kind, "length": t.length })).collect();
            json!({ "name": storyboard.name, "clips": clips, "transitions": transitions })
        })
        .collect();
    let settings = &project.settings;
    let timing = json!({
        "width": settings.width,
        "height": settings.height,
        "frameRate": { "num": settings.frame_rate.num, "den": settings.frame_rate.den },
        "storyboards": storyboards,
        "shots": shots,
    });
    let hex = |c: [u8; 4]| format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]);
    let swatches: Vec<Value> = project
        .palette
        .swatches
        .iter()
        .map(|s| {
            let color = hex(s.color);
            // The named palette the swatch is in: a character's, a location's, the default.
            let palette = project.palette.set(project.palette.set_of(s)).map_or("", |set| set.name.as_str());
            json!({ "id": s.id.0, "name": s.name, "color": color, "palette": palette })
        })
        .collect();
    let styles: Vec<Value> = project
        .palette
        .styles
        .iter()
        .map(|style| {
            let palette = project.palette.set(style.set).map_or("", |set| set.name.as_str());
            let colors: serde_json::Map<String, Value> = style.colors.iter().map(|(swatch, color)| (swatch.0.to_string(), json!(hex(*color)))).collect();
            json!({ "name": style.name, "palette": palette, "colors": colors })
        })
        .collect();
    for (name, value) in [("timing.json", timing), ("palette.json", json!(swatches)), ("styles.json", json!(styles))] {
        let text = serde_json::to_string_pretty(&value).unwrap_or_default();
        if let Err(e) = std::fs::write(out.join(name), text) {
            report.problems.push(format!("{name}: {e}"));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_model::{ArtContent, ArtLayerKind, DrawingId, SwatchId};

    use super::*;

    /// A project with a drawing "1" (a 3 × 2 red block at (10, 20)), an empty drawing "2", and
    /// a drawing whose name is not a file name.
    fn project() -> Project {
        let mut project = Project::default();
        let ids = [project.new_id(), project.new_id(), project.new_id()].map(DrawingId);
        let layer = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).layers[0]);
        for (id, name) in ids.iter().zip(["1", "2", "a:b"]) {
            layer.drawings.push(PoolDrawing { id: *id, name: name.to_string(), drawing: Arc::default() });
        }
        layer.exposure.set(0, 4, Some(ids[0]));
        layer.exposure.set(4, 2, Some(ids[1]));
        for index in [0, 2] {
            let drawing = Arc::make_mut(&mut layer.drawings[index].drawing);
            let Some(ArtContent::Indexed(line)) = drawing.layer_mut(ArtLayerKind::LineArt).map(|l| &mut l.content) else { panic!("indexed") };
            line.paint(Rect::new(10, 20, 13, 22), &[1.0; 6], SwatchId(3));
        }
        let peg = iskra_model::PegId(project.new_id());
        let transform = iskra_model::Transform { position: [40.0, 0.0], ..Default::default() };
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.pegs.push(Arc::new(iskra_model::Peg {
            id: peg,
            name: "Bounce".to_string(),
            parent: None,
            transform,
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        Arc::make_mut(&mut shot.layers[0]).parent = Some(peg);
        project
    }

    #[test]
    fn a_drawings_variants_are_written_in_their_skins_folders() {
        let mut project = project();
        let (skin, variant) = (iskra_model::SkinId(project.new_id()), DrawingId(project.new_id()));
        Arc::make_mut(&mut project.palette).skins.push(iskra_model::Skin { id: skin, name: "Night".to_string() });
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.skin = Some(skin);
        let layer = Arc::make_mut(&mut shot.layers[0]);
        let day = layer.drawings.iter().find(|day| day.name == "1").unwrap().clone();
        layer.variants.push(iskra_model::Variant { skin, of: day.id, drawing: PoolDrawing { id: variant, ..day } });
        let dir = tempfile::tempdir().unwrap();
        let report = export_project(&project, dir.path()).unwrap();
        assert_eq!(report.drawings_written, 3, "the variant too");
        assert!(dir.path().join("drawings").join("Shot_01").join("Drawing").join("Night").join("1.png").is_file());
        let timing: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("timing.json")).unwrap()).unwrap();
        let shot = &timing["shots"][0];
        assert_eq!(shot["skin"], json!("Night"));
        assert_eq!(shot["layers"][0]["drawings"][0]["skins"]["Night"]["file"], json!("drawings/Shot_01/Drawing/Night/1.png"));
    }

    #[test]
    fn a_project_exports_as_images_timing_and_palette() {
        let dir = tempfile::tempdir().unwrap();
        let project = project();
        let report = export_project(&project, dir.path()).unwrap();
        assert_eq!(report, ExportReport { drawings_written: 2, empty_drawings: 1, problems: vec![] });

        let folder = dir.path().join("drawings").join("Shot_01").join("Drawing");
        let decoder = png::Decoder::new(std::io::BufReader::new(File::open(folder.join("1.png")).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (3, 2), "cropped to what is painted");
        assert_eq!(&pixels[..4], [220, 50, 47, 255], "the palette's red, baked in");
        assert!(folder.join("a_b.png").exists(), "a name that is not a file name is made one");
        assert!(!folder.join("2.png").exists(), "an empty drawing has no image");

        let timing: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("timing.json")).unwrap()).unwrap();
        let layer = &timing["shots"][0]["layers"][0];
        assert_eq!(timing["frameRate"], json!({ "num": 24, "den": 1 }));
        assert_eq!(layer["drawings"][0]["file"], json!("drawings/Shot_01/Drawing/1.png"));
        assert_eq!((&layer["drawings"][0]["x"], &layer["drawings"][0]["y"]), (&json!(10), &json!(20)));
        assert_eq!(layer["drawings"][1]["file"], Value::Null);
        assert_eq!(layer["exposure"][0], json!({ "start": 0, "length": 4, "drawing": layer["drawings"][0]["id"] }));
        assert_eq!(layer["exposure"].as_array().unwrap().len(), 2);
        assert_eq!(timing["shots"][0]["stack"], json!(["Drawing"]));
        let peg = &timing["shots"][0]["pegs"][0];
        assert_eq!((&layer["parent"], &peg["name"], &peg["transform"]["position"]), (&json!("Bounce"), &json!("Bounce"), &json!([40.0, 0.0])));
        assert_eq!(layer["role"], json!("animation"));
        assert_eq!(timing["storyboards"][0]["clips"], json!([{ "shot": "Shot_01", "start": 0, "in": 0, "out": 48 }]));

        let palette: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("palette.json")).unwrap()).unwrap();
        assert_eq!(palette[2], json!({ "id": 3, "name": "Red", "color": "#dc322fff", "palette": "Default" }));
    }

    #[test]
    fn a_shot_shown_in_a_style_is_written_in_its_colours_and_the_styles_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = project();
        let night = iskra_model::Style {
            id: iskra_model::StyleId(1),
            name: "Night".into(),
            colors: [(iskra_model::SwatchId(3), [10, 20, 30, 255])].into(),
            set: Default::default(),
            kinds: Default::default(),
        };
        Arc::make_mut(&mut project.palette).styles.push(night);
        Arc::make_mut(&mut project.shots[0]).styles = vec![iskra_model::StyleId(1)];
        export_project(&project, dir.path()).unwrap();

        let folder = dir.path().join("drawings").join("Shot_01").join("Drawing");
        let mut reader = png::Decoder::new(std::io::BufReader::new(File::open(folder.join("1.png")).unwrap())).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        assert_eq!(&pixels[..4], [10, 20, 30, 255], "the style's red, baked in");
        let timing: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("timing.json")).unwrap()).unwrap();
        assert_eq!(timing["shots"][0]["styles"], json!(["Night"]));
        let styles: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("styles.json")).unwrap()).unwrap();
        assert_eq!(styles, json!([{ "name": "Night", "palette": "Default", "colors": { "3": "#0a141eff" } }]));
        let palette: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("palette.json")).unwrap()).unwrap();
        assert_eq!(palette[2]["color"], json!("#dc322fff"), "the swatch's own colour is as it was");
    }

    #[test]
    fn file_names_are_safe_and_never_shared() {
        let mut used = HashSet::new();
        assert_eq!(file_name("mouth-A", 7, &mut used), "mouth-A");
        assert_eq!(file_name("Mouth-A", 8, &mut used), "Mouth-A-8", "names that differ only in case would share a file on some systems");
        assert_eq!(file_name("a/b\\c:d", 9, &mut used), "a_b_c_d");
        assert_eq!(file_name("..", 10, &mut used), "10");
        assert_eq!(file_name("рука", 11, &mut used), "рука", "letters of any script are kept");
    }

    #[test]
    fn a_png_comes_back_as_it_was_encoded() {
        let pixels = vec![[255, 0, 0, 255], [0, 255, 0, 128], [0, 0, 0, 0], [1, 2, 3, 4], [9, 9, 9, 9], [255, 255, 255, 255]];
        let bytes = encode_png(3, 2, &pixels).unwrap();
        assert_eq!(decode_png(&bytes).unwrap(), (3, 2, pixels));
        assert!(decode_png(b"not a picture").is_err() && encode_png(3, 3, &[[0; 4]; 2]).is_err());
    }
}
