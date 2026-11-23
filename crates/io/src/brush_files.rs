//! Brush files in and out (plan §7.12).
//!
//! Two kinds of file hold brushes. The app's own (`.iskrabrushes`) is the presets as they are
//! kept, with everything. A Photoshop brush file (`.abr`) is read whole, old kind and new, by
//! [`crate::abr_map`]. It is written as the new kind (version 6), which holds a brush's tip
//! with its dynamics, scattering, texture, second tip and the rest, under the names the reader
//! reads them by; or, for programs that read only that, as the old kind (version 2), which
//! holds the tip of a brush, its spacing and its name.

use std::path::Path;

use iskra_drawing::{Brush, BrushPreset, BrushSettings, Brushes, Control, Dynamic, MaskMode, Pattern, TipShape, presets_to_json};
use photocraft_psd::abr::{AbrSample, LegacyBrush, LegacyTip, write_v6, write_v12};
use photocraft_psd::descriptor::{Descriptor, Id, UnicodeString, Value};
use photocraft_psd::patterns::PsdPattern;

/// The extension of the app's own brush files.
pub const EXTENSION: &str = "iskrabrushes";

/// The extension of Photoshop's brush files.
pub const ABR: &str = "abr";

/// The largest side of a sampled tip that is written to a Photoshop brush file.
const MAX_TIP: f32 = 2500.0;

/// The brushes of a file, and what of it could not be taken.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrushImport {
    pub presets: Vec<BrushPreset>,
    pub notes: Vec<String>,
}

fn is_abr(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case(ABR))
}

/// Whether a file is one that brushes are read from, by its ending.
pub fn is_brush_file(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case(ABR) || extension.eq_ignore_ascii_case(EXTENSION))
}

/// Read the brushes of the file at `path`: a Photoshop brush file if it is named `.abr`, else
/// one of the app's own. They are in the group the file's name gives, unless the file says.
pub fn read_brushes(path: &Path) -> Result<BrushImport, String> {
    read_brushes_with(path, &|| false, &|_| {})
}

/// [`read_brushes`] for a file that may be large, read apart from the window: `stop` is asked
/// before each brush of a Photoshop file and ends the reading once it says so, and `progress`
/// is told how far the reading is, from 0 to 1.
pub fn read_brushes_with(path: &Path, stop: &(dyn Fn() -> bool + Sync), progress: &(dyn Fn(f32) + Sync)) -> Result<BrushImport, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{} could not be read: {e}", path.display()))?;
    let group = path.file_stem().map_or("Imported".to_string(), |stem| stem.to_string_lossy().into_owned());
    let read = if is_abr(path) { brushes_of_abr_with(&bytes, &group, stop, progress) } else { brushes_of_own(&bytes, &group) };
    progress(1.0);
    read
}

/// The brushes of a Photoshop brush file, in `group`.
pub fn brushes_of_abr(bytes: &[u8], group: &str) -> Result<BrushImport, String> {
    brushes_of_abr_with(bytes, group, &|| false, &|_| {})
}

fn brushes_of_abr_with(bytes: &[u8], group: &str, stop: &(dyn Fn() -> bool + Sync), progress: &(dyn Fn(f32) + Sync)) -> Result<BrushImport, String> {
    let read = crate::abr_map::read_abr_with(bytes, group, &photocraft_raster::Interrupt::new(stop, progress))?;
    let presets =
        read.presets.into_iter().map(|preset| BrushPreset { name: preset.name, group: group.to_string(), brush: Brush::Raster(preset.brush) }).collect();
    // The reader is PhotoCraft's and says so; here its words are about this app.
    Ok(BrushImport { presets, notes: read.warnings.into_iter().map(|note| note.replace("PhotoCraft", "Iskra")).collect() })
}

/// The brushes of one of the app's own files. Those that name no group are in `group`.
pub fn brushes_of_own(bytes: &[u8], group: &str) -> Result<BrushImport, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not a file of Iskra brushes".to_string())?;
    let is_ours = serde_json::from_str::<serde_json::Value>(text).is_ok_and(|file| file["presets"].is_array());
    if !is_ours {
        return Err("not a file of Iskra brushes".to_string());
    }
    let read = Brushes::from_json(text);
    let mut notes = Vec::new();
    match read.unread() {
        0 => {}
        1 => notes.push("1 brush is of a kind this version of Iskra does not know; skipped".to_string()),
        unread => notes.push(format!("{unread} brushes are of a kind this version of Iskra does not know; skipped")),
    }
    let presets =
        read.presets.into_iter().map(|preset| if preset.group.is_empty() { BrushPreset { group: group.to_string(), ..preset } } else { preset }).collect();
    Ok(BrushImport { presets, notes })
}

/// Write `presets` to the file at `path`: as a Photoshop brush file if it is named `.abr`,
/// of the new kind unless `old` asks for the old one, else as one of the app's own. Returns
/// what the file could not hold.
pub fn write_brushes(path: &Path, presets: &[BrushPreset], old: bool) -> Result<Vec<String>, String> {
    let (bytes, notes) = match (is_abr(path), old) {
        (true, true) => abr_v2(presets)?,
        (true, false) => abr_v6(presets)?,
        (false, _) => (presets_to_json(presets).map_err(|e| e.to_string())?.into_bytes(), Vec::new()),
    };
    std::fs::write(path, bytes).map_err(|e| format!("{} could not be written: {e}", path.display()))?;
    Ok(notes)
}

/// `presets` as a Photoshop brush file of the old kind, version 2, and what it leaves behind.
/// It holds a brush's tip, computed or sampled, with its spacing; a sampled tip has its name.
pub fn abr_v2(presets: &[BrushPreset]) -> Result<(Vec<u8>, Vec<String>), String> {
    let (mut brushes, mut more, mut unnamed) = (Vec::new(), Vec::new(), 0);
    for preset in presets {
        // Vector brushes are not in Photoshop's files: they are left out, and said so.
        let Brush::Raster(brush) = &preset.brush else { continue };
        if has_more_than_a_tip(brush) {
            more.push(preset.name.as_str());
        }
        let spacing = if brush.spacing_enabled { (brush.spacing * 100.0).round().clamp(1.0, 999.0) as u16 } else { 0 };
        let tip = match &brush.tip {
            TipShape::Sampled(tile) if tile.is_valid() => LegacyTip::Sampled(sample(tile, brush.size)),
            _ => {
                unnamed += 1;
                LegacyTip::Computed {
                    diameter: brush.size.round().clamp(1.0, 999.0) as u16,
                    hardness: (brush.hardness * 100.0).round().clamp(0.0, 100.0) as u16,
                    angle: brush.angle.round().clamp(-180.0, 180.0) as i16,
                    roundness: (brush.roundness * 100.0).round().clamp(1.0, 100.0) as u16,
                }
            }
        };
        brushes.push(LegacyBrush { name: preset.name.clone(), spacing, anti_alias: !brush.aliased, tip });
    }
    let mut notes: Vec<String> = vector_left(presets).into_iter().collect();
    if !more.is_empty() {
        notes.push(format!(
            "This kind of file holds a brush's tip and spacing. Dynamics, scattering, texture, dual brush and transfer were left behind for: {}",
            more.join(", ")
        ));
    }
    if unnamed > 0 {
        notes.push(format!("This kind of file names only brushes with a sampled tip: {unnamed} with a round tip went without their names"));
    }
    let bytes = write_v12(2, &brushes, true).map_err(|e| format!("the brushes could not be written: {e}"))?;
    Ok((bytes, notes))
}

/// What a Photoshop brush file leaves out of `presets`: their vector brushes, which it can't hold.
fn vector_left(presets: &[BrushPreset]) -> Option<String> {
    let vector: Vec<&str> = presets.iter().filter(|preset| matches!(preset.brush, Brush::Vector(_))).map(|preset| preset.name.as_str()).collect();
    (!vector.is_empty()).then(|| format!("A Photoshop brush file holds no vector brushes. Left out: {}", vector.join(", ")))
}

fn text(said: &str) -> Value {
    Value::Text(UnicodeString::new_nul(said))
}

fn unit(of: &[u8; 4], value: f64) -> Value {
    Value::UnitFloat { unit: *of, value }
}

/// A fraction as the file has it: in percent.
fn percent(fraction: f32) -> Value {
    unit(b"#Prc", f64::from(fraction) * 100.0)
}

/// How a value varies, as the file has it: what it follows, by how much chance moves it, and
/// how low it may go.
fn varies(value: &Dynamic) -> Value {
    let follows = match value.control {
        Control::Off => 0,
        Control::Fade => 1,
        Control::PenPressure => 2,
        Control::PenTilt => 3,
        Control::StylusWheel => 4,
        Control::InitialDirection => 5,
        Control::Direction => 6,
        Control::Rotation => 8,
    };
    let said = Descriptor::new("brVr")
        .with("bVTy", Value::Integer(follows))
        .with("fStp", Value::Integer(value.fade_steps.min(9999) as i32))
        .with("jitter", percent(value.jitter))
        .with("Mnm ", percent(value.minimum));
    Value::Descriptor(said)
}

/// The way a texture or a second tip cuts the stroke, by the file's name for it.
fn cuts_by(mode: MaskMode) -> Value {
    let name = match mode {
        MaskMode::Multiply => "Mltp",
        MaskMode::Subtract => "Sbtr",
        MaskMode::Darken => "Drkn",
        MaskMode::Overlay => "Ovrl",
        MaskMode::ColorDodge => "CDdg",
        MaskMode::ColorBurn => "CBrn",
        MaskMode::LinearBurn => "linearBurn",
        MaskMode::HardMix => "hardMix",
        MaskMode::LinearHeight => "linearHeight",
        MaskMode::Height => "height",
    };
    Value::Enumerated { type_id: Id::new("BlnM"), value: Id::new(name) }
}

/// A tip's pixels as the new kind of file holds them: 16 bits a pixel, as they are.
fn sample_v6(tile: &photocraft_paint::GrayTile, id: &str) -> AbrSample {
    let data = tile.data.iter().flat_map(|value| value.to_be_bytes()).collect();
    AbrSample { id: id.to_string(), width: tile.width, height: tile.height, depth: 16, data }
}

/// The makers of a new-kind file's parts, which hand out the ids that the brushes name their
/// tips and patterns by.
#[derive(Default)]
struct Parts {
    samples: Vec<AbrSample>,
    patterns: Vec<PsdPattern>,
}

impl Parts {
    /// A tip's shape as the file describes it; a sampled tip's pixels go into the file under
    /// an id of their own. The same tip is in the file once.
    fn tip(&mut self, tip: &TipShape, size: f32, hardness: f32, angle: f32, roundness: f32, spacing: f32) -> Descriptor {
        let shape = match tip {
            TipShape::Sampled(tile) if tile.is_valid() => {
                let made = sample_v6(tile, "");
                let known = self.samples.iter().find(|sample| (sample.width, sample.height) == (made.width, made.height) && sample.data == made.data);
                let id = match known {
                    Some(known) => known.id.clone(),
                    None => {
                        let id = format!("$iskra-tip-{}", self.samples.len() + 1);
                        self.samples.push(AbrSample { id: id.clone(), ..made });
                        id
                    }
                };
                Descriptor::new("sampledBrush").with("sampledData", text(&id))
            }
            _ => Descriptor::new("computedBrush").with("Hrdn", percent(hardness)),
        };
        shape
            .with("Dmtr", unit(b"#Pxl", f64::from(size)))
            .with("Angl", unit(b"#Ang", f64::from(angle)))
            .with("Rndn", percent(roundness))
            .with("Spcn", percent(spacing))
    }

    /// A texture's pattern as the file names it, with its pixels in the file: a pattern that
    /// is made is written as the square it is made in, since the file has pictures only.
    fn pattern(&mut self, pattern: &Pattern) -> Descriptor {
        let (width, height, values) = match pattern {
            Pattern::Tile(tile) => (tile.width, tile.height, tile.to_f32()),
            Pattern::Stored(stored) => (stored.bitmap().width, stored.bitmap().height, stored.bitmap().to_f32()),
            Pattern::Procedural { style, size, seed } => {
                let made = photocraft_paint::procedural::pattern(*style, *size, *seed);
                (made.width as u32, made.height as u32, made.data)
            }
        };
        let plane: Vec<u8> = values.iter().map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
        let known = self.patterns.iter().find(|known| (known.width, known.height) == (width, height) && known.channels.first() == Some(&plane));
        let (id, name) = match known {
            Some(known) => (known.id.clone(), known.name.clone()),
            None => {
                let (id, name) = (format!("iskra-pattern-{}", self.patterns.len() + 1), format!("Pattern {}", self.patterns.len() + 1));
                let made =
                    PsdPattern { mode: 1, width, height, name: name.clone(), id: id.clone(), palette: None, depth: 8, channels: vec![plane], alpha: None };
                self.patterns.push(made);
                (id, name)
            }
        };
        Descriptor::new("Ptrn").with("Nm  ", text(&name)).with("Idnt", text(&id))
    }
}

/// `presets` as a Photoshop brush file of the new kind, version 6, and what it leaves behind.
/// It holds a brush as the reader of such files reads one: the tip with its spacing and
/// flips, shape dynamics, scattering, texture with its pattern, the second tip, how colour
/// and transfer vary, the pose, and the opacity, flow and smoothing as the tool's options.
pub fn abr_v6(presets: &[BrushPreset]) -> Result<(Vec<u8>, Vec<String>), String> {
    let (mut parts, mut said, mut made) = (Parts::default(), Vec::new(), 0);
    for preset in presets {
        // Vector brushes are not in Photoshop's files: they are left out, and said so.
        let Brush::Raster(brush) = &preset.brush else { continue };
        let flag = |on: bool| Value::Boolean(on);
        let tip = parts
            .tip(&brush.tip, brush.size, brush.hardness, brush.angle, brush.roundness, brush.spacing)
            .with("Intr", flag(brush.spacing_enabled))
            .with("flipX", flag(brush.flip_x))
            .with("flipY", flag(brush.flip_y));
        let mut one = Descriptor::new("brushPreset").with("Nm  ", text(&preset.name)).with("Brsh", Value::Descriptor(tip));
        let shape = &brush.shape_dynamics;
        one = one.with("useTipDynamics", flag(shape.enabled));
        if shape.enabled {
            one = one
                .with("szVr", varies(&shape.size))
                .with("minimumDiameter", percent(shape.size.minimum))
                .with("tiltScale", percent(shape.tilt_scale))
                .with("angleDynamics", varies(&shape.angle))
                .with("roundnessDynamics", varies(&shape.roundness))
                .with("minimumRoundness", percent(shape.roundness.minimum))
                .with("flipX", flag(shape.flip_x_jitter))
                .with("flipY", flag(shape.flip_y_jitter))
                .with("brushProjection", flag(shape.brush_projection));
        }
        let scatter = &brush.scattering;
        one = one.with("useScatter", flag(scatter.enabled));
        if scatter.enabled {
            one = one
                .with("scatterDynamics", varies(&scatter.scatter))
                .with("bothAxes", flag(scatter.both_axes))
                .with("Cnt ", Value::Integer(scatter.count as i32))
                .with("countDynamics", varies(&scatter.count_jitter));
        }
        let texture = &brush.texture;
        one = one.with("useTexture", flag(texture.enabled)).with("protectTexture", flag(brush.protect_texture));
        if texture.enabled {
            made += usize::from(matches!(texture.pattern, Pattern::Procedural { .. }));
            one = one
                .with("TxtC", flag(texture.each_tip))
                .with("InvT", flag(texture.invert))
                .with("textureScale", percent(texture.scale))
                .with("textureBrightness", Value::Integer((texture.brightness * 150.0).round() as i32))
                .with("textureContrast", Value::Integer((texture.contrast * 50.0).round() as i32))
                .with("textureBlendMode", cuts_by(texture.mode))
                .with("textureDepth", percent(texture.depth))
                .with("minimumDepth", percent(texture.depth_jitter.minimum))
                .with("textureDepthDynamics", varies(&texture.depth_jitter))
                .with("Txtr", Value::Descriptor(parts.pattern(&texture.pattern)));
        }
        let dual = &brush.dual_brush;
        if dual.enabled {
            let second = parts.tip(&dual.tip, dual.size, dual.hardness, dual.angle, dual.roundness, dual.spacing);
            let scattered = Dynamic { jitter: dual.scatter, ..Dynamic::default() };
            let two = Descriptor::new("dualBrush")
                .with("useDualBrush", flag(true))
                .with("Brsh", Value::Descriptor(second))
                .with("BlnM", cuts_by(dual.mode))
                .with("Flip", flag(dual.flip))
                .with("Spcn", percent(dual.spacing))
                .with("scatterDynamics", varies(&scattered))
                .with("bothAxes", flag(dual.both_axes))
                .with("Cnt ", Value::Integer(dual.count as i32));
            one = one.with("dualBrush", Value::Descriptor(two));
        }
        let colour = &brush.color_dynamics;
        one = one.with("useColorDynamics", flag(colour.enabled));
        if colour.enabled {
            one = one
                .with("colorDynamicsPerTip", flag(colour.per_tip))
                .with("clVr", varies(&colour.fg_bg))
                .with("H   ", percent(colour.hue_jitter))
                .with("Strt", percent(colour.saturation_jitter))
                .with("Brgh", percent(colour.brightness_jitter))
                .with("purity", percent(colour.purity));
        }
        let transfer = &brush.transfer;
        one = one.with("usePaintDynamics", flag(transfer.enabled));
        if transfer.enabled {
            one = one
                .with("opVr", varies(&transfer.opacity))
                .with("prVr", varies(&transfer.flow))
                .with("wtVr", varies(&transfer.wetness))
                .with("mxVr", varies(&transfer.mix));
        }
        let pose = &brush.pose;
        one = one.with("useBrushPose", flag(pose.enabled));
        if pose.enabled {
            // The file has a slant from -100 to 100 where the brush has degrees to 90.
            one = one
                .with("brushPoseTiltX", Value::Double(f64::from(pose.tilt_x) / 0.9))
                .with("brushPoseTiltY", Value::Double(f64::from(pose.tilt_y) / 0.9))
                .with("brushPoseAngle", Value::Double(f64::from(pose.rotation)))
                .with("brushPosePressure", percent(pose.pressure))
                .with("overridePoseTiltX", flag(pose.override_tilt_x))
                .with("overridePoseTiltY", flag(pose.override_tilt_y))
                .with("overridePoseAngle", flag(pose.override_rotation))
                .with("overridePosePressure", flag(pose.override_pressure));
        }
        let smoothing = &brush.smoothing;
        let tool = Descriptor::new("toolOptions")
            .with("Opct", percent(brush.opacity))
            .with("flow", percent(brush.flow))
            .with("usePressureOverridesSize", flag(brush.pressure_size))
            .with("usePressureOverridesOpacity", flag(brush.pressure_opacity))
            .with("smoothing", flag(smoothing.enabled))
            .with("smoothingValue", percent(smoothing.amount))
            .with("smoothingRadiusMode", flag(smoothing.pulled_string))
            .with("smoothingCatchup", flag(smoothing.catch_up))
            .with("smoothingCatchupAtEnd", flag(smoothing.catch_up_on_end))
            .with("smoothingZoomCompensation", flag(smoothing.adjust_for_zoom));
        one = one
            .with("Nose", flag(brush.noise))
            .with("Wtdg", flag(brush.wet_edges))
            .with("Rpt ", flag(brush.build_up))
            .with("toolOptions", Value::Descriptor(tool));
        said.push(one);
    }
    let mut notes: Vec<String> = vector_left(presets).into_iter().collect();
    if made > 0 {
        let patterns = if made == 1 { "1 made pattern was".to_string() } else { format!("{made} made patterns were") };
        notes.push(format!(
            "This kind of file holds a texture's pattern as a picture: {patterns} written as the square it is made in (paper, canvas, noise, dots)"
        ));
    }
    if presets.iter().any(|preset| matches!(&preset.brush, Brush::Raster(brush) if brush.aliased || brush.build_up)) {
        notes.push("Hard pixels and how fast a resting brush builds up are not in this kind of file".to_string());
    }
    let bytes = write_v6(2, &parts.samples, &parts.patterns, &said, true).map_err(|e| format!("the brushes could not be written: {e}"))?;
    Ok((bytes, notes))
}

/// Whether a brush is more than the old kind of file holds.
fn has_more_than_a_tip(brush: &BrushSettings) -> bool {
    brush.shape_dynamics.enabled
        || brush.scattering.enabled
        || brush.texture.enabled
        || brush.dual_brush.enabled
        || brush.transfer.enabled
        || brush.pressure_size
        || brush.pressure_opacity
}

/// A sampled tip as the file holds it: 8 bits a pixel, at the size the brush paints it, since
/// the old kind of file has no size but the picture's.
fn sample(tile: &photocraft_paint::GrayTile, size: f32) -> AbrSample {
    let scale = f64::from(size.clamp(1.0, MAX_TIP)) / f64::from(tile.width.max(tile.height));
    let side = |pixels: u32| ((f64::from(pixels) * scale).round() as u32).max(1);
    let (width, height) = (side(tile.width), side(tile.height));
    // Each pixel is read between the four of the tip around its centre.
    let at = |x: f64, y: f64| {
        let (x, y) = ((x - 0.5).clamp(0.0, f64::from(tile.width - 1)), (y - 0.5).clamp(0.0, f64::from(tile.height - 1)));
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(tile.width - 1), (y0 + 1).min(tile.height - 1));
        let (fx, fy) = ((x - f64::from(x0)) as f32, (y - f64::from(y0)) as f32);
        let across = |y: u32| tile.get(x0, y) * (1.0 - fx) + tile.get(x1, y) * fx;
        across(y0) * (1.0 - fy) + across(y1) * fy
    };
    let mut data = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let value = at((f64::from(x) + 0.5) / scale, (f64::from(y) + 0.5) / scale);
            data.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    AbrSample { id: String::new(), width, height, depth: 8, data }
}

#[cfg(test)]
mod tests {
    use iskra_drawing::builtin_brushes;

    use super::*;

    /// Where two brushes differ, as their settings are written: every number within a
    /// thousandth, but for what `skip` names, which the file does not hold.
    fn differences(was: &serde_json::Value, is: &serde_json::Value, at: &str, skip: &[&str], found: &mut Vec<String>) {
        use serde_json::Value as Json;
        if skip.iter().any(|skipped| at.ends_with(skipped)) {
            return;
        }
        match (was, is) {
            (Json::Object(was), Json::Object(is)) => {
                for (key, value) in was {
                    differences(value, is.get(key).unwrap_or(&Json::Null), &format!("{at}.{key}"), skip, found);
                }
            }
            (Json::Array(was), Json::Array(is)) if was.len() == is.len() => {
                for (index, (was, is)) in was.iter().zip(is).enumerate() {
                    differences(was, is, &format!("{at}[{index}]"), skip, found);
                }
            }
            (Json::Number(was), Json::Number(is)) => {
                if (was.as_f64().unwrap_or(0.0) - is.as_f64().unwrap_or(0.0)).abs() > 1e-3 {
                    found.push(format!("{at}: {was} became {is}"));
                }
            }
            (was, is) if was != is => found.push(format!("{at}: {was} became {is}")),
            _ => {}
        }
    }

    #[test]
    fn the_new_kind_of_photoshop_file_holds_a_brush_with_all_that_shapes_its_stroke() {
        // What the file does not hold: the colour and the tool the brush is painted with,
        // what is kept from brush to brush, hard pixels, the rate of building up, and a
        // made pattern as such, which is written as a picture.
        let skip = [".color", ".background", ".erase", ".mode", ".seed", ".locks", ".mixer", ".aliased", ".buildUpRate", ".texture.pattern"];
        // Every brush of the engine comes back as it went, and so does one with everything on.
        let mut all: Vec<BrushPreset> = builtin_brushes().iter().filter(|preset| matches!(preset.brush, Brush::Raster(_))).cloned().collect();
        let mut full = raster(&all[0]).clone();
        (full.shape_dynamics.enabled, full.shape_dynamics.tilt_scale, full.shape_dynamics.brush_projection) = (true, 1.5, true);
        (full.shape_dynamics.size.control, full.shape_dynamics.size.minimum, full.shape_dynamics.flip_x_jitter) = (Control::PenTilt, 0.25, true);
        (full.scattering.enabled, full.scattering.count, full.scattering.both_axes, full.scattering.scatter.jitter) = (true, 4, true, 2.5);
        (full.texture.enabled, full.texture.each_tip, full.texture.mode, full.texture.depth, full.texture.scale) = (true, true, MaskMode::HardMix, 0.6, 1.5);
        full.texture.pattern = Pattern::Tile(photocraft_paint::GrayTile::from_fn(4, 2, |x, y| if (x + y) % 2 == 0 { 1.0 } else { 0.0 }));
        (full.dual_brush.enabled, full.dual_brush.size, full.dual_brush.mode, full.dual_brush.count, full.dual_brush.scatter) =
            (true, 12.0, MaskMode::Subtract, 2, 1.2);
        (full.color_dynamics.enabled, full.color_dynamics.hue_jitter, full.color_dynamics.purity, full.color_dynamics.per_tip) = (true, 0.1, -0.5, false);
        (full.transfer.enabled, full.transfer.opacity.control, full.transfer.flow.jitter) = (true, Control::PenPressure, 0.3);
        (full.pose.enabled, full.pose.tilt_x, full.pose.rotation, full.pose.pressure, full.pose.override_tilt_x) = (true, 45.0, 120.0, 0.7, true);
        (full.noise, full.wet_edges, full.build_up, full.protect_texture, full.flip_x, full.spacing_enabled) = (true, true, true, true, true, false);
        (full.opacity, full.flow, full.pressure_opacity, full.smoothing.amount, full.smoothing.pulled_string) = (0.8, 0.6, true, 0.4, true);
        all.push(BrushPreset { name: "Everything".to_string(), group: String::new(), brush: Brush::Raster(full) });

        let (bytes, _) = abr_v6(&all).unwrap();
        let read = brushes_of_abr(&bytes, "Kit").unwrap();
        assert_eq!(read.presets.len(), all.len(), "{:?}", read.notes);
        for (was, is) in all.iter().zip(&read.presets) {
            assert_eq!((is.name.as_str(), is.group.as_str()), (was.name.as_str(), "Kit"));
            let mut found = Vec::new();
            let (before, after) = (serde_json::to_value(raster(was)).unwrap(), serde_json::to_value(raster(is)).unwrap());
            differences(&before, &after, "", &skip, &mut found);
            assert!(found.is_empty(), "{}: {found:#?}", was.name);
        }
        // A pattern of its own is in the file as it was; a made one as a picture of it.
        let Pattern::Tile(pattern) = &raster(read.presets.last().unwrap()).texture.pattern else { panic!("the pattern is in the file") };
        assert_eq!((pattern.width, pattern.height, pattern.get(0, 0), pattern.get(1, 0)), (4, 2, 1.0, 0.0));
        let made = all.iter().position(|preset| raster(preset).texture.enabled && matches!(raster(preset).texture.pattern, Pattern::Procedural { .. }));
        let made = made.expect("the engine has brushes with made patterns");
        assert!(matches!(raster(&read.presets[made]).texture.pattern, Pattern::Tile(_)));
        let (_, notes) = abr_v6(&all[made..=made]).unwrap();
        assert!(notes.iter().any(|note| note.contains("made pattern")), "{notes:?}");
        // The same tip is in the file once, however many brushes have it.
        let sampled = all.iter().find(|preset| matches!(raster(preset).tip, TipShape::Sampled(_))).unwrap().clone();
        let twice = [sampled.clone(), BrushPreset { name: "Again".to_string(), ..sampled }];
        let file = photocraft_psd::abr::parse(&abr_v6(&twice).unwrap().0).unwrap();
        assert_eq!((file.version, file.samples.len(), file.presets.len()), (6, 1, 2));
    }

    fn raster(preset: &BrushPreset) -> &BrushSettings {
        let Brush::Raster(brush) = &preset.brush else { panic!("a raster brush") };
        brush
    }

    #[test]
    fn a_photoshop_file_leaves_out_vector_brushes_and_says_so() {
        let vector = iskra_drawing::builtin_brushes().iter().find(|preset| matches!(preset.brush, Brush::Vector(_))).unwrap().clone();
        let round = iskra_drawing::builtin_brushes().iter().find(|preset| matches!(preset.brush, Brush::Raster(_))).unwrap().clone();
        for (bytes, notes) in [abr_v2(&[round.clone(), vector.clone()]).unwrap(), abr_v6(&[round.clone(), vector.clone()]).unwrap()] {
            assert!(notes.iter().any(|note| note.contains(&vector.name)), "{notes:?}");
            assert_eq!(brushes_of_abr(&bytes, "Kit").unwrap().presets.len(), 1);
        }
    }

    #[test]
    fn the_apps_own_file_carries_presets_whole_and_says_what_it_cant_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Inks.iskrabrushes");
        let presets: Vec<BrushPreset> =
            builtin_brushes().iter().take(3).cloned().chain([BrushPreset { group: String::new(), ..builtin_brushes()[20].clone() }]).collect();
        assert_eq!(write_brushes(&path, &presets, false), Ok(Vec::new()));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"kind\": \"raster\"") && !text.contains("inHand"), "a file of presets has no brush in hand");
        let read = read_brushes(&path).unwrap();
        assert_eq!(read.presets[..3], presets[..3]);
        assert_eq!((read.presets[3].group.as_str(), &read.presets[3].brush), ("Inks", &presets[3].brush), "a preset without a group is in the file's");
        assert!(read.notes.is_empty());

        let later = text.replacen("\"kind\": \"raster\"", "\"kind\": \"vector\"", 1);
        let read = brushes_of_own(later.as_bytes(), "Inks").unwrap();
        assert_eq!((read.presets.len(), read.notes.len()), (3, 1));
        assert!(brushes_of_own(b"{ \"layers\": [] }", "x").is_err() && brushes_of_own(&[0xff, 0xfe], "x").is_err());
        assert!(read_brushes(&dir.path().join("none.abr")).is_err());
    }

    #[test]
    fn a_photoshop_file_of_the_old_kind_is_written_and_read_back_with_its_tips() {
        let find = |name: &str| builtin_brushes().iter().find(|preset| preset.name == name).expect(name).clone();
        let sampled = builtin_brushes().iter().find(|preset| matches!(&raster(preset).tip, TipShape::Sampled(_))).expect("the engine has sampled tips").clone();
        let presets = [find("Calligraphy Flat"), find("Hard Round Pressure Size"), sampled.clone()];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Set.ABR");
        let notes = write_brushes(&path, &presets, true).unwrap();
        assert!(notes.iter().any(|note| note.contains("left behind") && note.contains("Hard Round Pressure Size")), "{notes:?}");
        assert!(notes.iter().any(|note| note.contains("2 with a round tip")), "{notes:?}");

        let read = read_brushes(&path).unwrap();
        assert_eq!(read.presets.len(), 3);
        assert!(read.presets.iter().all(|preset| preset.group == "Set"));
        let (flat, was) = (raster(&read.presets[0]), raster(&presets[0]));
        assert_eq!((flat.size, flat.hardness, flat.roundness, flat.angle, flat.spacing), (was.size, was.hardness, was.roundness, was.angle, was.spacing));
        // A sampled tip keeps its name and the size it paints at.
        let (tip, was) = (raster(&read.presets[2]), raster(&sampled));
        assert_eq!(read.presets[2].name, sampled.name);
        assert!(matches!(&tip.tip, TipShape::Sampled(tile) if tile.width.max(tile.height) == was.size.round() as u32), "{} for {}", tip.size, was.size);
        assert!((tip.size - was.size.round()).abs() < 1.0);
        assert!(brushes_of_abr(b"not a brush file", "x").is_err());
    }
}
