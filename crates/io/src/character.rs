//! Character import (plan §4.14): a design made in a paint program, read with every part where
//! it is in the document, ready to rig. The layers of a Photoshop file, or of an OpenRaster
//! one (`.ora`, which Krita, MyPaint and GIMP write), become layers, whatever
//! groups they are in: a group with no tag is looked through, as if what is in it lay where
//! the group does. Tags at the start of a layer's or a group's name say what else to do:
//!
//! - `[ignore] name`: the layer, or the group and all in it, is left out.
//! - `[merge] name`: the group is flattened into one drawing.
//! - `[slot] name`: the group is one layer, and each layer in it, and each group in it
//!   flattened, is a drawing of that layer: the mouths of a head, the hands of an arm, to swap
//!   between with the substitution picker.
//! - `[skin Name] name` (plan §4.8): a group's parts are not layers of their own but the
//!   variants in the skin `Name` of the parts of the same names, read as if the group had no
//!   tag; a layer tagged so alone is the variant of the part of its name.
//!
//! A name is taken without its tag. Whether a layer or a group is hidden in the file makes no
//! difference to what is brought in, and a layer's opacity and fill are not in its pixels:
//! every part comes in whole, to be seen.
//!
//! A layer clipped to the one under it (Photoshop's clipping mask, OpenRaster's `src-atop`)
//! shows only where that one, its base, is: in a flattened group it is cut to the base, and
//! brought in as a layer of its own its pixels are cut so. In a flattened group each layer is
//! laid over those under it by its blend mode, Multiply, Screen, Overlay and the others that
//! go channel by channel; a group that is not *Pass Through* is flattened apart first, then
//! laid so. A layer brought in on its own can't keep its blend mode, and the user is told.

use std::path::Path;

use photocraft_psd::{LayerNode, PsdFile};

/// A picture of a part of the design: its pixels, straight alpha, the top row first, and where
/// its top left corner is in the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A layer of the design as it is to be brought in, with its drawings: one, or for a slot one
/// for each of its parts, in the file's order, the bottom of the stack first. `shown` is the
/// drawing that is seen: the first of a slot's parts that is visible in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    pub name: String,
    pub drawings: Vec<Part>,
    pub shown: usize,
}

/// A design read from a layered file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Character {
    /// The document's size, in pixels.
    pub width: u32,
    pub height: u32,
    /// The layers to bring in, the bottom of the stack first.
    pub nodes: Vec<Imported>,
    /// What was left out and why, for the user to be told: by name.
    pub skipped: Vec<String>,
    /// The parts of each skin, by the skin's name, in the order the file has the skins: read
    /// as the layers are, to be matched to them by their names.
    pub skins: Vec<(String, Vec<Imported>)>,
}

/// What a name's tag asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tag {
    None,
    Ignore,
    Merge,
    Slot,
    /// Of the skin of this name.
    Skin(String),
}

/// A name's tag and the name without it: `[slot] mouth` is a slot called `mouth`. The tag is
/// read whatever its case, with or without a space after it; a tag that is not known is part
/// of the name.
fn tagged(name: &str) -> (Tag, String) {
    let trimmed = name.trim();
    let Some((tag, rest)) = trimmed.strip_prefix('[').and_then(|rest| rest.split_once(']')) else { return (Tag::None, trimmed.to_string()) };
    let tag = match tag.trim().to_lowercase().as_str() {
        "ignore" => Tag::Ignore,
        "merge" => Tag::Merge,
        "slot" => Tag::Slot,
        skin if skin.starts_with("skin ") && !tag.trim()[5..].trim().is_empty() => Tag::Skin(tag.trim()[5..].trim().to_string()),
        _ => return (Tag::None, trimmed.to_string()),
    };
    (tag, rest.trim().to_string())
}

/// How a layer is laid over what is under it, as the paint program has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Blend {
    /// Of a group: what is in it is laid over what is under the group, as if it were not there.
    PassThrough,
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColourDodge,
    ColourBurn,
    LinearBurn,
    LinearDodge,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    /// One that is not channel by channel, or not known: laid as Normal, by its name.
    Other(String),
}

impl Blend {
    fn of_psd(mode: photocraft_psd::BlendMode) -> Self {
        use photocraft_psd::BlendMode as Mode;
        match mode {
            Mode::PassThrough => Blend::PassThrough,
            Mode::Normal => Blend::Normal,
            Mode::Multiply => Blend::Multiply,
            Mode::Screen => Blend::Screen,
            Mode::Overlay => Blend::Overlay,
            Mode::Darken => Blend::Darken,
            Mode::Lighten => Blend::Lighten,
            Mode::ColorDodge => Blend::ColourDodge,
            Mode::ColorBurn => Blend::ColourBurn,
            Mode::LinearBurn => Blend::LinearBurn,
            Mode::LinearDodge => Blend::LinearDodge,
            Mode::HardLight => Blend::HardLight,
            Mode::SoftLight => Blend::SoftLight,
            Mode::Difference => Blend::Difference,
            Mode::Exclusion => Blend::Exclusion,
            Mode::Subtract => Blend::Subtract,
            Mode::Divide => Blend::Divide,
            other => Blend::Other(format!("{other:?}")),
        }
    }

    /// OpenRaster's `composite-op`, and whether it clips: `src-atop` is a layer laid only where
    /// what is under it is.
    pub(crate) fn of_ora(op: Option<&str>, group: bool) -> (Self, bool) {
        let blend = match op.map(|op| op.trim_start_matches("svg:")) {
            None if group => Blend::PassThrough,
            None | Some("src-over" | "src-atop") => Blend::Normal,
            Some("multiply") => Blend::Multiply,
            Some("screen") => Blend::Screen,
            Some("overlay") => Blend::Overlay,
            Some("darken") => Blend::Darken,
            Some("lighten") => Blend::Lighten,
            Some("color-dodge") => Blend::ColourDodge,
            Some("color-burn") => Blend::ColourBurn,
            Some("hard-light") => Blend::HardLight,
            Some("soft-light") => Blend::SoftLight,
            Some("difference") => Blend::Difference,
            Some("exclusion") => Blend::Exclusion,
            Some("plus") => Blend::LinearDodge,
            Some(other) => Blend::Other(other.to_string()),
        };
        (blend, op.is_some_and(|op| op.ends_with("src-atop")))
    }

    /// What a channel of `over` makes of the same channel of `under`, both from 0 to 1.
    fn mix(&self, under: f32, over: f32) -> f32 {
        let screen = |b: f32, s: f32| b + s - b * s;
        let hard = |b: f32, s: f32| if s <= 0.5 { b * 2.0 * s } else { screen(b, 2.0 * s - 1.0) };
        match self {
            Blend::PassThrough | Blend::Normal | Blend::Other(_) => over,
            Blend::Multiply => under * over,
            Blend::Screen => screen(under, over),
            Blend::Overlay => hard(over, under),
            Blend::Darken => under.min(over),
            Blend::Lighten => under.max(over),
            Blend::ColourDodge if under <= 0.0 => 0.0,
            Blend::ColourDodge if over >= 1.0 => 1.0,
            Blend::ColourDodge => (under / (1.0 - over)).min(1.0),
            Blend::ColourBurn if under >= 1.0 => 1.0,
            Blend::ColourBurn if over <= 0.0 => 0.0,
            Blend::ColourBurn => 1.0 - ((1.0 - under) / over).min(1.0),
            Blend::LinearBurn => (under + over - 1.0).max(0.0),
            Blend::LinearDodge => (under + over).min(1.0),
            Blend::HardLight => hard(under, over),
            Blend::SoftLight if over <= 0.5 => under - (1.0 - 2.0 * over) * under * (1.0 - under),
            Blend::SoftLight => {
                let deep = if under <= 0.25 { ((16.0 * under - 12.0) * under + 4.0) * under } else { under.sqrt() };
                under + (2.0 * over - 1.0) * (deep - under)
            }
            Blend::Difference => (under - over).abs(),
            Blend::Exclusion => under + over - 2.0 * under * over,
            Blend::Subtract => (under - over).max(0.0),
            Blend::Divide if over <= 0.0 => 1.0,
            Blend::Divide => (under / over).min(1.0),
        }
    }
}

/// A layer or a group of a layered file, as the reader walks it, whatever kind the file is.
pub(crate) struct Node {
    pub(crate) name: String,
    /// Whether it is seen in the file.
    pub(crate) visible: bool,
    /// How it is laid over what is under it.
    pub(crate) blend: Blend,
    /// It is clipped to the layer or the group under it: seen only where that one is.
    pub(crate) clipped: bool,
    /// Which of the file's layers its pixels are, for the file to be asked for them.
    pub(crate) layer: usize,
    /// What is in it, the bottom of the stack first: `None` for a layer.
    pub(crate) children: Option<Vec<Node>>,
}

/// The pixels of a layer of a file, straight alpha, the top row first, and where their top
/// left corner is in the document.
pub(crate) struct Picture {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

/// A layered file, as far as the reader needs it: the pixels of one of its layers, as they
/// are painted.
pub(crate) trait Layered {
    fn pixels(&self, layer: usize) -> Result<Picture, String>;
}

impl Layered for PsdFile {
    fn pixels(&self, layer: usize) -> Result<Picture, String> {
        let image = self.layer(layer).ok_or("the file has no such layer")?.rgba8().map_err(|why| why.to_string())?;
        Ok(Picture { left: image.left, top: image.top, width: image.width, height: image.height, rgba: image.data })
    }
}

/// The tree of a Photoshop file's layers, as the reader walks it.
fn psd_nodes(file: &PsdFile, nodes: &[LayerNode]) -> Vec<Node> {
    let node = |node: &LayerNode| {
        let index = node.index();
        let record = file.layers().get(index);
        // A group's mode is in its section block; Pass Through is said only there.
        let mode = record.map(|record| record.section_divider().and_then(|section| section.blend_mode).unwrap_or(record.blend_mode));
        Node {
            name: file.layer(index).map_or(String::new(), |layer| layer.name()),
            visible: record.is_some_and(|record| !record.flags.hidden()),
            blend: mode.map_or(Blend::Normal, Blend::of_psd),
            clipped: record.is_some_and(|record| record.clipping != 0),
            layer: index,
            children: matches!(node, LayerNode::Group { .. }).then(|| psd_nodes(file, node.children())),
        }
    };
    nodes.iter().map(node).collect()
}

/// Reads a design from a layered file.
struct Reader<'a> {
    file: &'a dyn Layered,
    skipped: Vec<String>,
    /// The pictures of the file's layers that were read, by the layer: a flattened group asks
    /// for them more than once.
    read: std::collections::HashMap<usize, Option<Part>>,
    /// The parts of the skins read so far, by the skin's name.
    skins: Vec<(String, Vec<Imported>)>,
}

/// A picture as large as a flattened group, straight alpha, the top row first: where its top
/// left corner is in the document, and its size.
#[derive(Clone, Copy)]
struct Frame {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
}

impl Frame {
    fn blank(self) -> Vec<u8> {
        vec![0; self.width * self.height * 4]
    }

    /// `part` laid on a blank picture of the frame, as it is.
    fn placed(self, part: &Part) -> Vec<u8> {
        let mut rgba = self.blank();
        let (dx, dy) = ((part.left - self.left) as usize, (part.top - self.top) as usize);
        for row in 0..part.height as usize {
            let from = &part.rgba[row * part.width as usize * 4..(row + 1) * part.width as usize * 4];
            let at = ((dy + row) * self.width + dx) * 4;
            rgba[at..at + from.len()].copy_from_slice(from);
        }
        rgba
    }
}

/// `over` laid on `under`, both of one frame: by `blend`, and only as far as `clip`'s alpha
/// is, where it is given.
fn laid(over: &[u8], under: &mut [u8], blend: &Blend, clip: Option<&[u8]>) {
    for (at, (over, under)) in over.as_chunks::<4>().0.iter().zip(under.as_chunks_mut::<4>().0).enumerate() {
        let mut over = *over;
        if let Some(clip) = clip {
            over[3] = ((u32::from(over[3]) * u32::from(clip[at * 4 + 3]) + 127) / 255) as u8;
        }
        if matches!(blend, Blend::Normal | Blend::PassThrough | Blend::Other(_)) {
            over_straight(&over, under);
        } else {
            blended(&over, under, blend);
        }
    }
}

/// Put a pixel over another by a blend mode, both straight alpha: where there is nothing
/// under it the pixel is as it is, and where there is, its colour is what the mode makes.
fn blended(over: &[u8; 4], under: &mut [u8; 4], blend: &Blend) {
    let (a, b) = (f32::from(over[3]) / 255.0, f32::from(under[3]) / 255.0);
    if a <= 0.0 {
        return;
    }
    let all = a + b * (1.0 - a);
    for channel in 0..3 {
        let (s, d) = (f32::from(over[channel]) / 255.0, f32::from(under[channel]) / 255.0);
        let mixed = (1.0 - b) * s + b * blend.mix(d, s);
        let colour = (a * mixed + (1.0 - a) * b * d) / all;
        under[channel] = (colour.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    under[3] = (all * 255.0).round() as u8;
}

impl Reader<'_> {
    /// The pixels of one layer of the file, as they are painted: its opacity and its fill are
    /// not in them. `None` for a layer with no pixels: an adjustment, a fill, an empty layer.
    fn picture(&mut self, layer: usize, name: &str) -> Option<Part> {
        if let Some(read) = self.read.get(&layer) {
            return read.clone();
        }
        let read = self.read_picture(layer, name);
        self.read.insert(layer, read.clone());
        read
    }

    fn read_picture(&mut self, layer: usize, name: &str) -> Option<Part> {
        let image = match self.file.pixels(layer) {
            Ok(image) if image.width > 0 && image.height > 0 => image,
            Ok(_) => {
                self.skipped.push(format!("{name}: it has no pixels"));
                return None;
            }
            Err(why) => {
                self.skipped.push(format!("{name}: {why}"));
                return None;
            }
        };
        Some(Part { name: name.to_string(), left: image.left, top: image.top, width: image.width, height: image.height, rgba: image.rgba })
    }

    /// Every layer under a node, the bottom first: what a flattened group is made of. What
    /// is tagged to be ignored is left out; what is hidden is not.
    fn seen(&mut self, node: &Node, into: &mut Vec<Part>) {
        let (tag, name) = tagged(&node.name);
        if matches!(tag, Tag::Ignore | Tag::Skin(_)) {
            return;
        }
        match &node.children {
            None => into.extend(self.picture(node.layer, &name)),
            Some(children) => children.iter().for_each(|child| self.seen(child, into)),
        }
    }

    /// A group flattened into one picture, named `name`: its layers, each over those under
    /// it by its blend mode, and those that are clipped only where their base is.
    fn merged(&mut self, children: &[Node], name: &str) -> Option<Part> {
        let mut parts = Vec::new();
        for child in children {
            self.seen(child, &mut parts);
        }
        let (left, top) = (parts.iter().map(|part| part.left).min()?, parts.iter().map(|part| part.top).min()?);
        let right = parts.iter().map(|part| part.left + part.width as i32).max()?;
        let bottom = parts.iter().map(|part| part.top + part.height as i32).max()?;
        let frame = Frame { left, top, width: (right - left) as usize, height: (bottom - top) as usize };
        let mut rgba = frame.blank();
        self.stack(children, frame, &mut rgba);
        Some(Part { name: name.to_string(), left, top, width: frame.width as u32, height: frame.height as u32, rgba })
    }

    /// Lay `children`, the bottom first, on `canvas`, a picture of `frame`. A run of clipped
    /// layers is cut to the first one under them that is not clipped, their base; a clipped
    /// layer with no base is laid as any.
    fn stack(&mut self, children: &[Node], frame: Frame, canvas: &mut [u8]) {
        let mut base: Option<Vec<u8>> = None;
        for child in children {
            let (tag, name) = tagged(&child.name);
            // A skin's parts are no part of the day picture.
            if matches!(tag, Tag::Ignore | Tag::Skin(_)) {
                continue;
            }
            if let Blend::Other(mode) = &child.blend {
                self.skipped.push(format!("{name}: its blend mode, {mode}, is not one this reads, so it is laid as Normal"));
            }
            match (&child.children, &child.blend) {
                (_, blend) if child.clipped && base.is_some() => {
                    let picture = self.isolated(child, frame);
                    let blend = if *blend == Blend::PassThrough { Blend::Normal } else { blend.clone() };
                    laid(&picture, canvas, &blend, base.as_deref());
                }
                // What is in a Pass Through group is laid as if the group were not there; its
                // shape is still the base of what is clipped to it.
                (Some(within), Blend::PassThrough) => {
                    let shape = self.isolated(child, frame);
                    self.stack(within, frame, canvas);
                    base = Some(shape);
                }
                (_, blend) => {
                    let picture = self.isolated(child, frame);
                    laid(&picture, canvas, blend, None);
                    base = Some(picture);
                }
            }
        }
    }

    /// A layer or a group as a picture of `frame` alone: a group flattened apart.
    fn isolated(&mut self, node: &Node, frame: Frame) -> Vec<u8> {
        let (_, name) = tagged(&node.name);
        match &node.children {
            None => self.picture(node.layer, &name).map_or_else(|| frame.blank(), |part| frame.placed(&part)),
            Some(within) => {
                let mut rgba = frame.blank();
                self.stack(within, frame, &mut rgba);
                rgba
            }
        }
    }

    /// Bring in `nodes`, side by side in their stack: a layer clipped to the one under it,
    /// its base, has its pixels cut to that one's shape.
    fn nodes(&mut self, nodes: &[Node], into: &mut Vec<Imported>) {
        let mut base: Option<Part> = None;
        for node in nodes {
            let (tag, name) = tagged(&node.name);
            if tag == Tag::Ignore {
                continue;
            }
            let from = into.len();
            self.node(node, into);
            let followed = |nodes: &[Node]| nodes.iter().skip_while(|other| !std::ptr::eq(*other, node)).nth(1).is_some_and(|next| next.clipped);
            if !matches!(node.blend, Blend::Normal | Blend::PassThrough | Blend::Other(_)) && !matches!(tag, Tag::Merge | Tag::Skin(_)) {
                self.skipped
                    .push(format!("{name}: comes in laid as Normal, since a layer of Iskra has no blend mode; its own is kept in a group tagged [merge]"));
            }
            match (&base, node.clipped) {
                (Some(shape), true) => {
                    for drawing in into[from..].iter_mut().flat_map(|imported| imported.drawings.iter_mut()) {
                        cut(drawing, shape);
                    }
                }
                // The shape of what is clipped to it, where something is: a layer's pixels, or
                // all of a group.
                _ if followed(nodes) => {
                    base = match &node.children {
                        None => self.picture(node.layer, &name),
                        Some(within) => self.merged(within, &name),
                    };
                }
                _ => base = None,
            }
        }
    }

    /// What a node of the file is brought in as: nothing, a layer, or for a group with no
    /// tag the layers of what is in it, as if they lay where the group does.
    fn node(&mut self, node: &Node, into: &mut Vec<Imported>) {
        let (tag, name) = tagged(&node.name);
        match (tag, &node.children) {
            (Tag::Ignore, _) => {}
            // A skin's parts, read as if the group had no tag, apart from the day ones.
            (Tag::Skin(skin), children) => {
                let mut parts = Vec::new();
                match children {
                    Some(children) => self.nodes(children, &mut parts),
                    None => parts.extend(self.picture(node.layer, &name).map(|drawing| Imported { name, drawings: vec![drawing], shown: 0 })),
                }
                match self.skins.iter_mut().find(|(named, _)| *named == skin) {
                    Some((_, had)) => had.extend(parts),
                    None => self.skins.push((skin, parts)),
                }
            }
            (_, None) => {
                into.extend(self.picture(node.layer, &name).map(|drawing| Imported { name, drawings: vec![drawing], shown: 0 }));
            }
            (Tag::Merge, Some(children)) => match self.merged(children, &name) {
                Some(drawing) => into.push(Imported { name, drawings: vec![drawing], shown: 0 }),
                None => self.skipped.push(format!("{name}: there is nothing in it to merge")),
            },
            // Each part of a slot is a drawing, whether it is seen in the file or not: the
            // one that is seen there is the one that is shown.
            (Tag::Slot, Some(children)) => {
                let (mut drawings, mut shown) = (Vec::new(), None);
                for child in children {
                    let (tag, part) = tagged(&child.name);
                    if tag == Tag::Ignore {
                        continue;
                    }
                    let drawing = match &child.children {
                        None => self.picture(child.layer, &part),
                        Some(within) => self.merged(within, &part),
                    };
                    if let Some(drawing) = drawing {
                        if shown.is_none() && child.visible {
                            shown = Some(drawings.len());
                        }
                        drawings.push(drawing);
                    }
                }
                if drawings.is_empty() {
                    self.skipped.push(format!("{name}: a slot with nothing in it"));
                } else {
                    into.push(Imported { name, drawings, shown: shown.unwrap_or(0) });
                }
            }
            (Tag::None, Some(children)) => self.nodes(children, into),
        }
    }
}

/// `part` seen only where `shape` is: its alpha times the shape's where they overlap, and
/// nothing where the shape is not.
fn cut(part: &mut Part, shape: &Part) {
    for y in 0..part.height as i32 {
        for x in 0..part.width as i32 {
            let (sx, sy) = (part.left + x - shape.left, part.top + y - shape.top);
            let inside = sx >= 0 && sy >= 0 && sx < shape.width as i32 && sy < shape.height as i32;
            let solid = if inside { u32::from(shape.rgba[((sy as usize) * shape.width as usize + sx as usize) * 4 + 3]) } else { 0 };
            let at = ((y as usize) * part.width as usize + x as usize) * 4 + 3;
            part.rgba[at] = ((u32::from(part.rgba[at]) * solid + 127) / 255) as u8;
        }
    }
}

/// Put a pixel over another, both straight alpha.
fn over_straight(over: &[u8; 4], under: &mut [u8; 4]) {
    let (a, b) = (u32::from(over[3]), u32::from(under[3]));
    if a == 0 {
        return;
    }
    // What of the one under still shows, and how much there is in all, out of 255 × 255.
    let rest = b * (255 - a);
    let all = a * 255 + rest;
    for channel in 0..3 {
        under[channel] = ((u32::from(over[channel]) * a * 255 + u32::from(under[channel]) * rest + all / 2) / all) as u8;
    }
    under[3] = ((all + 127) / 255) as u8;
}

/// The file name endings of the layered files a character is read from: Photoshop's, and
/// OpenRaster's, which Krita, MyPaint and GIMP write.
pub const CHARACTER_EXTENSIONS: [&str; 3] = ["psd", "psb", "ora"];

fn ending(path: &Path) -> String {
    path.extension().and_then(|ending| ending.to_str()).unwrap_or_default().to_lowercase()
}

/// Whether a file is one a character can be read from, by its ending.
pub fn is_character(path: &Path) -> bool {
    CHARACTER_EXTENSIONS.contains(&ending(path).as_str())
}

/// Reads a character design from a layered file, a Photoshop or an OpenRaster one: its
/// layers and groups as they are to be brought in, by the tags in their names.
pub fn read_character(path: &Path) -> Result<Character, String> {
    let unreadable = |why: &dyn std::fmt::Display| format!("{} could not be read: {why}", path.display());
    let bytes = std::fs::read(path).map_err(|e| unreadable(&e))?;
    let (psd, ora);
    let (file, size, tree): (&dyn Layered, _, _) = if ending(path) == "ora" {
        ora = crate::ora::Ora::open(bytes).map_err(|e| unreadable(&e))?;
        (&ora, (ora.width, ora.height), ora.nodes())
    } else {
        psd = PsdFile::from_bytes(&bytes).map_err(|e| unreadable(&e))?;
        (&psd, (psd.header.width, psd.header.height), psd_nodes(&psd, &psd.layer_tree()))
    };
    let mut reader = Reader { file, skipped: Vec::new(), read: Default::default(), skins: Vec::new() };
    let mut nodes: Vec<Imported> = Vec::new();
    reader.nodes(&tree, &mut nodes);
    // A file all of whose parts are in skins has the user say which is the day one.
    if nodes.is_empty() && reader.skins.is_empty() {
        return Err(format!("{} has no layers to bring in. A flat picture comes in with Import Media", path.display()));
    }
    // A group flattened more than once says what it says once.
    let mut skipped = reader.skipped;
    let mut said = std::collections::HashSet::new();
    skipped.retain(|line| said.insert(line.clone()));
    Ok(Character { width: size.0, height: size.1, nodes, skipped, skins: reader.skins })
}

#[cfg(test)]
mod tests {
    use photocraft_psd::{GroupSpec, LayerSpec, PixelData, PsdBuilder};

    use super::*;

    fn layer(name: &str, left: i32, top: i32, color: [u8; 4]) -> LayerSpec {
        LayerSpec::new(name, left, top, 2, 2, PixelData::Rgba8(color.repeat(4)))
    }

    /// The pixel at `x`, `y` of the document in a part, or nothing outside it.
    fn at(part: &Part, x: i32, y: i32) -> [u8; 4] {
        let (px, py) = (x - part.left, y - part.top);
        if px < 0 || py < 0 || px >= part.width as i32 || py >= part.height as i32 {
            return [0; 4];
        }
        let i = (py as usize * part.width as usize + px as usize) * 4;
        part.rgba[i..i + 4].try_into().unwrap()
    }

    fn read(builder: PsdBuilder) -> Character {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("design.psd");
        std::fs::write(&path, builder.to_bytes().unwrap()).unwrap();
        read_character(&path).unwrap()
    }

    #[test]
    fn a_clipped_layer_is_cut_to_its_base_and_blend_modes_are_kept_in_a_merge() {
        use photocraft_psd::BlendMode;
        let (grey, red, white) = ([128, 128, 128, 255], [200, 0, 0, 255], [255, 255, 255, 255]);
        let wide = |name: &str, color: [u8; 4]| LayerSpec::new(name, 0, 0, 4, 4, PixelData::Rgba8(color.repeat(16)));
        let mut builder = PsdBuilder::new(4, 4);
        // A merged arm: a grey sleeve of 2 by 2, and over it, clipped to it, a red stripe as
        // wide as the document laid by Multiply; then a white layer clipped to nothing.
        builder.begin_group(GroupSpec::new("[merge] arm"));
        builder.push_layer(layer("sleeve", 0, 0, grey));
        builder.push_layer(LayerSpec { clipping: true, blend_mode: BlendMode::Multiply, ..wide("shade", red) });
        builder.end_group().unwrap();
        // Beside it, brought in on its own: a body, and a shadow clipped to it by Multiply.
        builder.push_layer(layer("body", 2, 2, white));
        builder.push_layer(LayerSpec { clipping: true, blend_mode: BlendMode::Multiply, ..wide("shadow", red) });
        let character = read(builder);
        let names: Vec<&str> = character.nodes.iter().map(|node| node.name.as_str()).collect();
        assert_eq!(names, ["arm", "body", "shadow"]);
        // The stripe is only on the sleeve, and multiplied with it: 128 × 200 / 255 is 100.
        let arm = &character.nodes[0].drawings[0];
        assert_eq!((at(arm, 0, 0), at(arm, 1, 1)), ([100, 0, 0, 255], [100, 0, 0, 255]));
        assert_eq!(at(arm, 3, 3)[3], 0, "nothing past the sleeve");
        // On its own the shadow is cut to the body's square, and the user is told its mode is not kept.
        let shadow = &character.nodes[2].drawings[0];
        assert_eq!((at(shadow, 3, 3), at(shadow, 0, 0)[3]), (red, 0));
        assert!(character.skipped.iter().any(|line| line.starts_with("shadow: comes in laid as Normal")), "{:?}", character.skipped);
    }

    #[test]
    fn a_group_that_is_not_pass_through_is_flattened_apart_first() {
        use photocraft_psd::BlendMode;
        let grey = [128, 128, 128, 255];
        let flattened = |inner: BlendMode| {
            let mut builder = PsdBuilder::new(2, 2);
            builder.begin_group(GroupSpec::new("[merge] all"));
            builder.push_layer(layer("ground", 0, 0, grey));
            builder.begin_group(GroupSpec { blend_mode: inner, ..GroupSpec::new("over") });
            // Multiply in the inner group: on what is under it only where the group passes through.
            builder.push_layer(LayerSpec { blend_mode: BlendMode::Multiply, ..layer("dark", 0, 0, grey) });
            builder.end_group().unwrap();
            builder.end_group().unwrap();
            let character = read(builder);
            at(&character.nodes[0].drawings[0], 0, 0)
        };
        assert_eq!(flattened(BlendMode::PassThrough), [64, 64, 64, 255], "multiplied with the ground");
        assert_eq!(flattened(BlendMode::Normal), grey, "apart, it has nothing under it to multiply, and is laid over as it is");
    }

    #[test]
    fn a_name_has_a_tag_or_none() {
        assert_eq!(tagged("[slot] mouth"), (Tag::Slot, "mouth".to_string()));
        assert_eq!(tagged(" [MERGE]arm L "), (Tag::Merge, "arm L".to_string()));
        assert_eq!(tagged("[Ignore]   notes"), (Tag::Ignore, "notes".to_string()));
        assert_eq!(tagged("head"), (Tag::None, "head".to_string()));
        assert_eq!(tagged("[skin Night] body"), (Tag::Skin("Night".to_string()), "body".to_string()));
        assert_eq!(tagged("[SKIN Late Night]"), (Tag::Skin("Late Night".to_string()), String::new()), "a skin's name keeps its case");
        assert_eq!(tagged("[skin] body"), (Tag::None, "[skin] body".to_string()), "a skin needs a name");
        assert_eq!(tagged("[wip] head"), (Tag::None, "[wip] head".to_string()), "a tag that is not known is part of the name");
    }

    #[test]
    fn a_design_is_read_with_its_groups_slots_and_merged_parts() {
        let dir = tempfile::tempdir().unwrap();
        let (red, green, blue, white) = ([255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]);
        let mut builder = PsdBuilder::new(20, 10);
        // From the bottom: a body; a group of an arm with a merged hand; a slot of mouths, the
        // second of which is the one that is seen; notes to leave out.
        builder.push_layer(layer("body", 4, 2, red));
        builder.begin_group(GroupSpec::new("arm"));
        builder.push_layer(layer("upper", 8, 2, green));
        builder.begin_group(GroupSpec::new("[merge] hand"));
        builder.push_layer(layer("palm", 10, 4, blue));
        builder.push_layer(LayerSpec { opacity: 128, ..layer("glove", 11, 4, white) });
        builder.push_layer(LayerSpec { visible: false, ..layer("ring", 0, 0, red) });
        builder.end_group().unwrap();
        builder.end_group().unwrap();
        builder.begin_group(GroupSpec::new("[slot] mouth"));
        builder.push_layer(LayerSpec { visible: false, ..layer("A", 6, 6, red) });
        builder.push_layer(layer("O", 6, 7, green));
        builder.push_layer(layer("[ignore] guide", 0, 0, white));
        builder.end_group().unwrap();
        builder.push_layer(layer("[ignore] notes", 0, 0, white));
        builder.begin_group(GroupSpec::new("[ignore] old"));
        builder.push_layer(layer("leg", 0, 0, white));
        builder.end_group().unwrap();
        let path = dir.path().join("hero.psd");
        std::fs::write(&path, builder.to_bytes().unwrap()).unwrap();

        let design = read_character(&path).unwrap();
        assert_eq!((design.width, design.height, design.skipped.len()), (20, 10, 0));
        // The arm's group has no tag: it is looked through, and its layers lie beside the body.
        let names: Vec<&str> = design.nodes.iter().map(|node| node.name.as_str()).collect();
        assert_eq!(names, ["body", "upper", "hand", "mouth"]);
        let [body, _, hand, mouth] = &design.nodes[..] else { panic!("{names:?}") };
        // A layer is where it is in the document, named as it is.
        assert_eq!((body.drawings[0].name.as_str(), (body.drawings[0].left, body.drawings[0].top)), ("body", (4, 2)));
        // A merged group is one picture of all that is in it: the glove whole over the palm,
        // though it is at half its strength in the file, and the ring though it is hidden.
        let merged = &hand.drawings[0];
        assert_eq!((hand.drawings.len(), merged.name.as_str(), merged.left, merged.top, merged.width, merged.height), (1, "hand", 0, 0, 13, 6));
        let pixel = |x: usize, y: usize| &merged.rgba[(y * 13 + x) * 4..(y * 13 + x) * 4 + 4];
        assert_eq!((pixel(10, 4), pixel(11, 4), pixel(12, 4), pixel(0, 0), pixel(5, 3)[3]), (&blue[..], &white[..], &white[..], &red[..], 0));
        // A slot is one layer with a drawing a part, the hidden one too, named as the parts
        // are; the one that is seen in the file is the one that is shown.
        assert_eq!((mouth.drawings.iter().map(|drawing| drawing.name.as_str()).collect::<Vec<_>>(), mouth.shown), (vec!["A", "O"], 1));
        assert_eq!(((mouth.drawings[0].left, mouth.drawings[0].top), (mouth.drawings[1].left, mouth.drawings[1].top)), ((6, 6), (6, 7)));
        // A hidden layer with no tag comes in like any other, at its full strength.
        let mut hidden = PsdBuilder::new(8, 8);
        hidden.push_layer(LayerSpec { visible: false, opacity: 64, ..layer("prop", 1, 1, green) });
        let path = dir.path().join("hidden.psd");
        std::fs::write(&path, hidden.to_bytes().unwrap()).unwrap();
        let design = read_character(&path).unwrap();
        assert_eq!((design.nodes.len(), design.nodes[0].name.as_str(), &design.nodes[0].drawings[0].rgba[..4]), (1, "prop", &green[..]));

        // A file with nothing to bring in says so.
        let mut flat = PsdBuilder::new(4, 4);
        flat.push_layer(layer("[ignore] all", 0, 0, red));
        let path = dir.path().join("flat.psd");
        std::fs::write(&path, flat.to_bytes().unwrap()).unwrap();
        assert!(read_character(&path).is_err_and(|why| why.contains("no layers to bring in")));
        assert!(is_character(&path) && is_character(Path::new("BIG.PSB")) && !is_character(Path::new("a.png")));
    }
}
