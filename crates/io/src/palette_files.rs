//! Palettes as files (plan §4.8): the app's own, which holds a palette whole, with its styles,
//! and GIMP's (`.gpl`), a list of names and colours that many programs read and write.
//! Adobe's two kinds of swatch file are read and not written: Photoshop's colour swatches
//! (`.aco`) and the swatch exchange file (`.ase`), from the descriptions of them that are
//! public.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iskra_model::{MediaId, Palette, Swatch, SwatchId};
use serde_json::{Value, json};

/// The extension of the app's own palette files.
pub const PALETTE_EXTENSION: &str = "iskrapalette";

/// The extension of GIMP's palette files.
pub const GPL: &str = "gpl";

/// The first line of a GIMP palette.
const GPL_HEAD: &str = "GIMP Palette";

/// The extension of Photoshop's colour swatch files, which are read only.
pub const ACO: &str = "aco";

/// The extension of Adobe's swatch exchange files, which are read only.
pub const ASE: &str = "ase";

/// The extension of the palettes of a rig's scene (plan §4.17), which are read only.
pub const PLT: &str = "plt";

/// The first words of a palette of a rig's scene.
const PLT_HEAD: &str = "ToonBoomAnimationInc PaletteFile";

/// The extensions of the files a palette is read from.
pub const PALETTE_EXTENSIONS: [&str; 5] = [PALETTE_EXTENSION, GPL, ACO, ASE, PLT];

fn ends_in(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|ending| ending.eq_ignore_ascii_case(extension))
}

fn is_gpl(path: &Path) -> bool {
    ends_in(path, GPL)
}

/// Whether a file is one a palette is read from, by its extension.
pub fn is_palette_file(path: &Path) -> bool {
    PALETTE_EXTENSIONS.iter().any(|extension| ends_in(path, extension))
}

/// Read a palette from a file: GIMP's or one of Adobe's by its extension, or else the app's
/// own.
pub fn read_palette(path: &Path) -> Result<Palette, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{} can't be read: {e}", path.display()))?;
    let text = || String::from_utf8(bytes.clone()).map_err(|_| "it is not text".to_string());
    let read = if ends_in(path, ACO) {
        palette_of_aco(&bytes)
    } else if ends_in(path, ASE) {
        palette_of_ase(&bytes)
    } else if is_gpl(path) {
        text().and_then(|text| palette_of_gpl(&text))
    } else if ends_in(path, PLT) {
        text().and_then(|text| palette_of_plt(&text).map(|plt| plt.without_textures()))
    } else {
        text().and_then(|text| palette_of_own(&text))
    };
    read.map_err(|why| format!("{} is no palette: {why}", path.display()))
}

/// Write a palette to a file: GIMP's by its extension, which holds the palette's own colours
/// without their transparency and no styles, or else the app's own.
pub fn write_palette(path: &Path, palette: &Palette) -> Result<(), String> {
    write_palette_with(path, palette, &BTreeMap::new())
}

/// [`write_palette`], with the files of the pictures its texture swatches paint with, by the
/// ids of their media, for the app's own file (plan §4.20).
pub fn write_palette_with(path: &Path, palette: &Palette, pictures: &BTreeMap<MediaId, PathBuf>) -> Result<(), String> {
    let name = path.file_stem().map_or(String::new(), |name| name.to_string_lossy().into_owned());
    let text = if is_gpl(path) { gpl_of_palette(palette, &name) } else { own_of_palette_with(palette, pictures) };
    std::fs::write(path, text).map_err(|e| format!("{} can't be written: {e}", path.display()))
}

/// The app's own file: the palette as the project file has it, under a mark that says what
/// the file is.
pub fn own_of_palette(palette: &Palette) -> String {
    own_of_palette_with(palette, &BTreeMap::new())
}

/// [`own_of_palette`], with the files of the pictures of its texture swatches, which a project
/// it is brought into takes in as media.
pub fn own_of_palette_with(palette: &Palette, pictures: &BTreeMap<MediaId, PathBuf>) -> String {
    let mut written = json!({ "iskraPalette": 1, "palette": palette });
    if !pictures.is_empty() {
        let pictures: BTreeMap<String, &PathBuf> = pictures.iter().map(|(media, file)| (media.0.to_string(), file)).collect();
        written["pictures"] = json!(pictures);
    }
    serde_json::to_string_pretty(&written).expect("a palette is plain data")
}

/// The files of the pictures that the texture swatches of the app's own palette file at
/// `path` paint with, by the ids of their media where it was written. None for another kind
/// of file, or one that has none.
pub fn palette_pictures(path: &Path) -> BTreeMap<MediaId, PathBuf> {
    let read = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let pictures = read.and_then(|mut read| serde_json::from_value::<BTreeMap<String, PathBuf>>(read["pictures"].take()).ok()).unwrap_or_default();
    pictures.into_iter().filter_map(|(media, file)| Some((MediaId(media.parse().ok()?), file))).collect()
}

pub fn palette_of_own(text: &str) -> Result<Palette, String> {
    let mut read: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    match read["iskraPalette"].as_u64() {
        Some(1) => serde_json::from_value(read["palette"].take()).map_err(|e| e.to_string()),
        Some(later) => Err(format!("it is of a later kind ({later}) than this version reads")),
        None => Err("it does not say that it is one".to_string()),
    }
}

/// A GIMP palette: a line for each colour, its red, green and blue from 0 to 255 and then
/// its name.
pub fn gpl_of_palette(palette: &Palette, name: &str) -> String {
    let mut text = format!("{GPL_HEAD}\nName: {name}\nColumns: 0\n#\n");
    for swatch in &palette.swatches {
        let [r, g, b, _] = swatch.color;
        // A name is the rest of its line.
        text.push_str(&format!("{r:3} {g:3} {b:3}\t{}\n", swatch.name.replace(['\n', '\r'], " ")));
    }
    text
}

pub fn palette_of_gpl(text: &str) -> Result<Palette, String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some(GPL_HEAD) {
        return Err(format!("it does not start with `{GPL_HEAD}`"));
    }
    let mut swatches = Vec::new();
    for line in lines.map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
        // `Name:` and `Columns:` are about the palette; anything else that is no colour is
        // left out, as GIMP leaves it.
        let mut parts = line.split_whitespace();
        let mut channel = || parts.next().and_then(|part| part.parse::<u8>().ok());
        let (Some(r), Some(g), Some(b)) = (channel(), channel(), channel()) else { continue };
        let Ok(id) = u16::try_from(swatches.len() + 1) else { break };
        // Three numbers, and the rest of the line is the name.
        let rest = parts.collect::<Vec<_>>().join(" ");
        let name = if rest.is_empty() || rest == "Untitled" { format!("Colour {id}") } else { rest };
        swatches.push(Swatch { id: SwatchId(id), name, color: [r, g, b, 255], set: Default::default(), kind: Default::default(), drift: None });
    }
    if swatches.is_empty() {
        return Err("it has no colours".to_string());
    }
    Ok(Palette::of(swatches))
}

/// A palette of a rig's scene, as read: its swatches, the texture swatches among them, and
/// the colours of a kind it has no colour for, by name and kind.
#[derive(Clone, Debug)]
pub struct Plt {
    pub palette: Palette,
    pub textures: Vec<PltTexture>,
    pub left: Vec<String>,
}

/// A texture swatch of a rig's palette: the swatch, which is grey until its picture is read,
/// the picture's file name in the folder of the palette's textures, and whether it is tiled.
#[derive(Clone, Debug, PartialEq)]
pub struct PltTexture {
    pub swatch: SwatchId,
    pub file: String,
    pub tile: bool,
}

impl Plt {
    /// The palette with its texture swatches left out: for a reader of colours alone.
    pub fn without_textures(mut self) -> Palette {
        self.palette.swatches.retain(|swatch| self.textures.iter().all(|texture| texture.swatch != swatch.id));
        self.palette
    }
}

/// Where the picture of a texture of the palette file `plt` is: in the folder named after
/// the palette with `_textures`, beside it.
pub fn plt_texture_path(plt: &Path, texture: &PltTexture) -> std::path::PathBuf {
    let stem = plt.file_stem().map_or(String::new(), |stem| stem.to_string_lossy().into_owned());
    plt.with_file_name(format!("{stem}_textures")).join(&texture.file)
}

/// A palette of a rig's scene (plan §4.17): a line a colour, of its kind, its name (in quotes
/// when it has spaces), its id in the scene, and for a solid colour its red, green, blue and
/// alpha from 0 to 255. A texture's line has `Tile` after its id where it is tiled, and its
/// picture is a TGA named by its id (plan §4.20). A colour of another kind is a solid swatch
/// of the first colour its line gives; one that gives none is left out, and named in the
/// list that comes back with the palette.
pub fn palette_of_plt(text: &str) -> Result<Plt, String> {
    let mut lines = text.lines();
    if !lines.next().is_some_and(|head| head.trim_start().starts_with(PLT_HEAD)) {
        return Err("it does not start as such a palette does".to_string());
    }
    let (mut swatches, mut textures, mut left) = (Vec::new(), Vec::new(), Vec::new());
    for line in lines {
        let words = quoted_words(line);
        // A line of one word names a group of colours, and has none of its own.
        let [kind, name, scene_id, rest @ ..] = &words[..] else { continue };
        let Ok(id) = u16::try_from(swatches.len() + 1) else { break };
        let channels: Vec<u8> = rest.iter().map_while(|word| word.parse().ok()).collect();
        let color = match channels[..] {
            [r, g, b, a, ..] => [r, g, b, a],
            _ if kind == "Texture" => {
                let file = format!("{}.tga", scene_id.trim_start_matches("0x"));
                textures.push(PltTexture { swatch: SwatchId(id), file, tile: rest.iter().any(|word| word == "Tile") });
                [128, 128, 128, 255]
            }
            _ => {
                left.push(format!("{name} ({kind})"));
                continue;
            }
        };
        swatches.push(Swatch { id: SwatchId(id), name: name.clone(), color, set: Default::default(), kind: Default::default(), drift: None });
    }
    if swatches.is_empty() {
        return Err("it has no colours".to_string());
    }
    Ok(Plt { palette: Palette::of(swatches), textures, left })
}

/// The words of a line, a word in double quotes kept whole without its quotes.
fn quoted_words(line: &str) -> Vec<String> {
    let (mut words, mut word, mut quoted) = (Vec::new(), String::new(), false);
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Bytes read from their start, big end first, as Adobe's files are written.
struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], String> {
        if self.0.len() < count {
            return Err("it ends too soon".to_string());
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(taken)
    }

    fn u16(&mut self) -> Result<u16, String> {
        self.take(2).map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        self.take(4).map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn f32(&mut self) -> Result<f32, String> {
        self.u32().map(f32::from_bits)
    }

    /// A name of `count` two-byte characters, up to the zero that ends it.
    fn name(&mut self, count: usize) -> Result<String, String> {
        let units: Vec<u16> = self.take(count * 2)?.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect();
        let named = units.split(|unit| *unit == 0).next().unwrap_or(&[]);
        Ok(String::from_utf16_lossy(named).trim().to_string())
    }
}

fn byte(part: f32) -> u8 {
    (part.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A colour from its hue (0 to 1, around the wheel), saturation and brightness.
fn rgb_of_hsb(hue: f32, saturation: f32, brightness: f32) -> [u8; 3] {
    let sixth = hue.rem_euclid(1.0) * 6.0;
    let channel = |shift: f32| {
        let k = (shift + sixth).rem_euclid(6.0);
        byte(brightness - brightness * saturation * k.min(4.0 - k).clamp(0.0, 1.0))
    };
    [channel(5.0), channel(3.0), channel(1.0)]
}

/// A colour from inks, each from 0 (none) to 1 (full), with no profile: as near as a
/// screen colour is to a printed one without knowing the press.
fn rgb_of_cmyk(inks: [f32; 4]) -> [u8; 3] {
    let [c, m, y, k] = inks;
    [c, m, y].map(|ink| byte((1.0 - ink) * (1.0 - k)))
}

/// A colour from CIE Lab (lightness 0 to 100) under the D50 light that Adobe's files mean.
fn rgb_of_lab(l: f32, a: f32, b: f32) -> [u8; 3] {
    let fy = (l + 16.0) / 116.0;
    let (fx, fz) = (fy + a / 500.0, fy - b / 200.0);
    let part = |f: f32| if f > 6.0 / 29.0 { f * f * f } else { 3.0 * (6.0f32 / 29.0).powi(2) * (f - 4.0 / 29.0) };
    let (x, y, z) = (0.96422 * part(fx), part(fy), 0.82521 * part(fz));
    // To sRGB's own light (D65) and its three colours, then its curve.
    let linear = [3.133_856 * x - 1.616_867 * y - 0.490_615 * z, -0.978_768 * x + 1.916_142 * y + 0.033_454 * z, 0.071_945 * x - 0.228_991 * y + 1.405_243 * z];
    linear.map(|light| byte(if light <= 0.003_130_8 { 12.92 * light } else { 1.055 * light.max(0.0).powf(1.0 / 2.4) - 0.055 }))
}

/// The palette of colours as they were read, numbered from 1, each with its name or
/// "Colour" and its number.
fn palette_of_colours(colours: Vec<(String, [u8; 3])>) -> Result<Palette, String> {
    if colours.is_empty() {
        return Err("it has no colours that can be read".to_string());
    }
    let numbered = colours.into_iter().zip(1u16..);
    let swatch = |((name, [r, g, b]), id): ((String, [u8; 3]), u16)| {
        let name = if name.is_empty() { format!("Colour {id}") } else { name };
        Swatch { id: SwatchId(id), name, color: [r, g, b, 255], set: Default::default(), kind: Default::default(), drift: None }
    };
    Ok(Palette::of(numbered.map(swatch).collect()))
}

/// Photoshop's colour swatches: a count of colours, each a colour space and four numbers,
/// and after those, in the files of the second kind, the same colours again with names.
/// Colours of a space that is not red-green-blue, hue-saturation-brightness, inks, Lab or
/// grey are left out.
pub fn palette_of_aco(bytes: &[u8]) -> Result<Palette, String> {
    let mut bytes = Bytes(bytes);
    let mut read = Vec::new();
    // The second kind follows the first in most files, and is the one with the names.
    while !bytes.0.is_empty() {
        let version = bytes.u16()?;
        if !(1..=2).contains(&version) {
            return Err(format!("it is of a kind ({version}) that is not known"));
        }
        let count = bytes.u16()?;
        let mut colours = Vec::new();
        for _ in 0..count {
            let space = bytes.u16()?;
            let [w, x, y, z] = [bytes.u16()?, bytes.u16()?, bytes.u16()?, bytes.u16()?].map(f32::from);
            let colour = match space {
                0 => Some([w, x, y].map(|part| byte(part / 65535.0))),
                1 => Some(rgb_of_hsb(w / 65536.0, x / 65535.0, y / 65535.0)),
                2 => Some(rgb_of_cmyk([w, x, y, z].map(|ink| 1.0 - ink / 65535.0))),
                // Lightness from 0 to 10000; a and b from -12800 to 12700, as signed numbers.
                7 => Some(rgb_of_lab(w / 100.0, f32::from(x as u16 as i16) / 100.0, f32::from(y as u16 as i16) / 100.0)),
                8 => Some([byte(1.0 - w / 10000.0); 3]),
                _ => None,
            };
            let name = if version == 2 {
                let length = bytes.u32()? as usize;
                bytes.name(length)?
            } else {
                String::new()
            };
            colours.extend(colour.map(|colour| (name, colour)));
        }
        read = colours;
    }
    palette_of_colours(read)
}

/// Adobe's swatch exchange file: blocks, of which the colours are read, each a name, a
/// colour model and its numbers from 0 to 1. Groups of colours are read as their colours.
pub fn palette_of_ase(bytes: &[u8]) -> Result<Palette, String> {
    let mut bytes = Bytes(bytes);
    if bytes.take(4)? != b"ASEF" {
        return Err("it does not start with `ASEF`".to_string());
    }
    let (_version, blocks) = (bytes.u32()?, bytes.u32()?);
    let mut colours = Vec::new();
    for _ in 0..blocks {
        let kind = bytes.u16()?;
        let length = bytes.u32()? as usize;
        let mut block = Bytes(bytes.take(length)?);
        // A colour; the start and the end of a group are passed by.
        if kind != 1 {
            continue;
        }
        let count = block.u16()? as usize;
        let name = block.name(count)?;
        let colour = match block.take(4)? {
            b"RGB " => Some([block.f32()?, block.f32()?, block.f32()?].map(byte)),
            b"CMYK" => Some(rgb_of_cmyk([block.f32()?, block.f32()?, block.f32()?, block.f32()?])),
            b"LAB " => Some(rgb_of_lab(block.f32()? * 100.0, block.f32()?, block.f32()?)),
            b"Gray" => Some([byte(block.f32()?); 3]),
            _ => None,
        };
        colours.extend(colour.map(|colour| (name, colour)));
    }
    palette_of_colours(colours)
}

#[cfg(test)]
mod tests {
    use iskra_model::{Style, StyleId};

    use super::*;

    #[test]
    fn a_palette_of_a_rigs_scene_is_read_with_its_solid_colours() {
        let text = "ToonBoomAnimationInc PaletteFile 2\n\
                    Solid    Black                      0x0d5395128fb0102a   0   0   0 255\n\
                    Solid    \"Vectorized Line\"          0x0000000000000003   0   0   0 255\n\
                    \"Pencil|\"\n\
                    Texture  Bumpy_Fat                  0x04fec228884827dc Tile\n\
                    LinearGradient Sky                  0x04fec22888482817 10 20 30 255 200 210 220 255\n\
                    Solid    CTRL1                      0x0d233a429994d489   0 225 255 132\n";
        let plt = palette_of_plt(text).unwrap();
        let read: Vec<(&str, [u8; 4])> = plt.palette.swatches.iter().map(|swatch| (swatch.name.as_str(), swatch.color)).collect();
        let grey = [128, 128, 128, 255];
        assert_eq!(
            read,
            [("Black", [0, 0, 0, 255]), ("Vectorized Line", [0, 0, 0, 255]), ("Bumpy_Fat", grey), ("Sky", [10, 20, 30, 255]), ("CTRL1", [0, 225, 255, 132])]
        );
        // The texture's picture is named by its id, in the folder of the palette's textures.
        assert_eq!(plt.textures, [PltTexture { swatch: SwatchId(3), file: "04fec228884827dc.tga".to_string(), tile: true }]);
        let path = plt_texture_path(Path::new("/scene/palette-library/Pencil.plt"), &plt.textures[0]);
        assert_eq!(path, Path::new("/scene/palette-library/Pencil_textures/04fec228884827dc.tga"));
        assert!(plt.left.is_empty());
        assert_eq!(plt.clone().without_textures().swatches.len(), 4);
        assert!(palette_of_plt("GIMP Palette\n").unwrap_err().contains("does not start"));
        assert!(!palette_of_plt("ToonBoomAnimationInc PaletteFile 2\nTexture A 0x1\n").unwrap().textures[0].tile);
        assert!(palette_of_plt("ToonBoomAnimationInc PaletteFile 2\nPencil A 0x1\n").unwrap_err().contains("no colours"));
        assert!(is_palette_file(Path::new("BOY.PLT")));
    }

    #[test]
    fn the_apps_own_file_holds_a_palette_whole() {
        let mut palette = Palette::standard();
        palette.styles.push(Style {
            id: StyleId(1),
            name: "Night".to_string(),
            colors: [(SwatchId(3), [60, 20, 90, 128])].into(),
            set: Default::default(),
            kinds: Default::default(),
        });
        assert_eq!(palette_of_own(&own_of_palette(&palette)), Ok(palette.clone()));
        assert!(palette_of_own("{}").is_err() && palette_of_own("nothing").is_err());
        assert!(palette_of_own(r#"{ "iskraPalette": 2, "palette": {} }"#).unwrap_err().contains("later"));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Hero.iskrapalette");
        write_palette(&path, &palette).unwrap();
        assert_eq!(read_palette(&path), Ok(palette));
        assert!(read_palette(&dir.path().join("None.iskrapalette")).is_err());
    }

    #[test]
    fn a_gimp_palette_is_names_and_colours() {
        let text = gpl_of_palette(&Palette::standard(), "Standard");
        assert!(text.starts_with("GIMP Palette\nName: Standard\nColumns: 0\n#\n  0   0   0\tBlack\n255 255 255\tWhite\n220  50  47\tRed\n"), "{text}");
        assert_eq!(palette_of_gpl(&text), Ok(Palette::standard()));

        // As other programs write them: spaces or tabs, comments, names of several words or
        // none, and lines that are no colour.
        let other = "GIMP Palette\nName: Skin tones\nColumns: 4\n# made elsewhere\n\n240 200 170 Skin light\n 90\t60\t30\n12 300 4 Wrong\n1 2 3\tUntitled\n";
        let read = palette_of_gpl(other).unwrap();
        let listed: Vec<_> = read.swatches.iter().map(|swatch| (swatch.id.0, swatch.name.as_str(), swatch.color)).collect();
        assert_eq!(listed, [(1, "Skin light", [240, 200, 170, 255]), (2, "Colour 2", [90, 60, 30, 255]), (3, "Colour 3", [1, 2, 3, 255])]);
        assert!(palette_of_gpl("JASC-PAL\n0100\n").is_err() && palette_of_gpl("GIMP Palette\n#\n").is_err());
    }

    fn wide(name: &str) -> Vec<u8> {
        name.encode_utf16().chain([0]).flat_map(u16::to_be_bytes).collect()
    }

    #[test]
    fn photoshops_colour_swatches_are_read_with_their_names() {
        // Red, a hue of a third around at full strength, no ink but full black, a mid grey.
        let colours: [(u16, [u16; 4], &str); 5] = [
            (0, [0xdcdc, 0x3232, 0x2f2f, 0], "Red"),
            (1, [21845, 65535, 65535, 0], "Green"),
            (2, [65535, 65535, 65535, 0], "Ink"),
            (8, [5000, 0, 0, 0], ""),
            (9, [1, 2, 3, 4], "Unknown"),
        ];
        let entry = |(space, parts, _): &(u16, [u16; 4], &str)| std::iter::once(*space).chain(*parts).flat_map(u16::to_be_bytes).collect::<Vec<u8>>();
        let mut file: Vec<u8> = [1u16, 5].into_iter().flat_map(u16::to_be_bytes).collect();
        file.extend(colours.iter().flat_map(entry));
        let first = palette_of_aco(&file).unwrap();
        assert_eq!(first.swatches.iter().map(|swatch| swatch.name.as_str()).collect::<Vec<_>>(), ["Colour 1", "Colour 2", "Colour 3", "Colour 4"]);

        file.extend([2u16, 5].into_iter().flat_map(u16::to_be_bytes));
        for colour in &colours {
            file.extend(entry(colour));
            file.extend((colour.2.encode_utf16().count() as u32 + 1).to_be_bytes());
            file.extend(wide(colour.2));
        }
        let read = palette_of_aco(&file).unwrap();
        let listed: Vec<_> = read.swatches.iter().map(|swatch| (swatch.id.0, swatch.name.as_str(), swatch.color)).collect();
        assert_eq!(
            listed,
            [(1, "Red", [220, 50, 47, 255]), (2, "Green", [0, 255, 0, 255]), (3, "Ink", [0, 0, 0, 255]), (4, "Colour 4", [128, 128, 128, 255])],
            "the colour of a space that is not known is left out"
        );
        assert!(palette_of_aco(&file[..file.len() - 3]).is_err() && palette_of_aco(&[0, 3, 0, 0]).is_err() && palette_of_aco(&[]).is_err());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Hero.ACO");
        std::fs::write(&path, &file).unwrap();
        assert_eq!((read_palette(&path), is_palette_file(&path), is_palette_file(&dir.path().join("Hero.png"))), (Ok(read), true, false));
    }

    #[test]
    fn a_swatch_exchange_file_is_read_through_its_groups() {
        let block = |kind: u16, body: Vec<u8>| kind.to_be_bytes().into_iter().chain((body.len() as u32).to_be_bytes()).chain(body).collect::<Vec<u8>>();
        let colour = |name: &str, model: &[u8; 4], parts: &[f32]| {
            let mut body: Vec<u8> = (name.encode_utf16().count() as u16 + 1).to_be_bytes().to_vec();
            body.extend(wide(name));
            body.extend(model);
            body.extend(parts.iter().flat_map(|part| part.to_be_bytes()));
            body.extend(0u16.to_be_bytes());
            block(1, body)
        };
        let mut file = b"ASEF".to_vec();
        file.extend([0u8, 1, 0, 0]);
        file.extend(6u32.to_be_bytes());
        file.extend(block(0xc001, [0u8, 5].into_iter().chain(wide("Hero")).collect()));
        file.extend(colour("Skin", b"RGB ", &[240.0 / 255.0, 200.0 / 255.0, 170.0 / 255.0]));
        file.extend(colour("White", b"LAB ", &[1.0, 0.0, 0.0]));
        file.extend(colour("Paper", b"CMYK", &[0.0, 0.0, 0.0, 0.0]));
        file.extend(colour("", b"Gray", &[0.5]));
        file.extend(block(0xc002, Vec::new()));
        let read = palette_of_ase(&file).unwrap();
        let listed: Vec<_> = read.swatches.iter().map(|swatch| (swatch.name.as_str(), swatch.color)).collect();
        assert_eq!(
            listed,
            [("Skin", [240, 200, 170, 255]), ("White", [255, 255, 255, 255]), ("Paper", [255, 255, 255, 255]), ("Colour 4", [128, 128, 128, 255])]
        );
        assert!(palette_of_ase(b"ASEX").is_err() && palette_of_ase(&file[..40]).is_err() && palette_of_ase(b"ASEF\0\x01\0\0\0\0\0\0").is_err());
        // Lab's middle grey and a strong red come out as the screen has them, near enough.
        assert_eq!(rgb_of_lab(53.585, 0.0, 0.0), [128, 128, 128]);
        let [r, g, b] = rgb_of_lab(54.29, 80.81, 69.89);
        assert!(r >= 253 && g <= 4 && b <= 4, "{r} {g} {b}");
    }
}
