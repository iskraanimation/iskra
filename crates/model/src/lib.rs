//! The project document: plain serde data. The engine shares it as immutable `Arc` snapshots
//! (plan §3), so everything here is cheap to clone and never mutated in place: shots,
//! layers and drawings sit behind `Arc`s, and an edit copies only what it changes.

mod apply;
mod bin;
mod camera;
mod clip;
mod cut;
mod deform;
mod display;
mod driver;
mod effect;
mod exposure;
mod expr;
mod graph;
mod group;
mod guide;
mod handoff;
mod key;
mod media;
mod note;
mod panel;
mod pass;
mod peg;
mod render;
mod skin;
mod sound;
mod space;
mod storyboard;
mod template;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

pub use apply::{Applied, ApplyOptions, Found, Match, compared};
pub use bin::{Bin, Item};
pub use camera::{Camera, FIELD_OF_VIEW, LensAnimation};
pub use clip::{CopiedFrame, CopiedKey, KeyClip, KeyOwner};
pub use cut::NoMerge;
pub use deform::{Bone, Deformer, DeformerId, Ik, Knot, PIECES, PoseValue, Warp};
pub use display::{Display, DisplayId, MAIN_DISPLAY};
pub use driver::{Driver, DriverInput, DriverParameter, DriverPreset, PresetParameter, driver_presets};
pub use effect::{
    BLEND, BLEND_MODES, COLLIDER, COLOUR_CARD, Curves, DEPTH, Effect, EffectId, EffectKind, MATTE, PARTICLES, PICTURE, PORT_IN, PORT_OUT, Param, ParamKind,
    ParamSpec, Port, PortKind, RECEIVE, SEND, SPLASH_SPRITE, SPRITE, STRAIGHT, STYLE, STYLE_PALETTE, TAG, VISIBLE, Value, effect_kind, effect_kinds,
};
pub use effectcraft_time::{FrameRate, format_timecode_frames};
pub use exposure::{Exposure, Run};
pub use expr::{Amounts, At as ExprAt, Driven, ExprError, Expressions, Read as DriverRead, Scope as DriverScope, reserved as reserved_in_expressions};
pub use graph::{Composite, Source, Stacking};
pub use group::{Group, GroupId, Work};
pub use guide::{DrawGuide, GuideId, GuideKind, RULER_ARM, Snap};
pub use handoff::{HandOff, Status};
pub use iskra_drawing::Align as TextAlign;
pub use iskra_drawing::kurbo;
pub use iskra_drawing::{
    Affine, ArtContent, ArtLayer, ArtLayerKind, ArtReach, Boil, BrushSettings, Cap, Drawing, Form, IndexedPixel, IndexedSurface, Lasso, LassoPart,
    MixerSettings, MixerState, Nib, Picked, Point, Rect, Shape, ShapeId, ShapeKind, Stroke, StrokePoint, SwatchId, TextBox, TextFormat, VectorArt, VectorFill,
    VectorShape, VectorStroke,
};
pub use iskra_drawing::{Area, Areas, Frame, Gradient, GradientShape, GradientStop, Picture, Placement, Shade};
pub use key::{Animatable, Bend, Cycle, CycleKind, Interp, Key, Slope, Spacing, Track, squash};
pub use media::{Alpha, Media, MediaKind, Spoken};
pub use note::Note;
pub use panel::Panel;
pub use peg::{Animation, Channel, Follow, Inherit, Node, Peg, Transform};
pub use render::{RenderMark, RenderRecord, Rendered};
use serde::{Deserialize, Serialize};
pub use skin::{Skin, SkinId, Variant};
pub use sound::{SoundClip, SoundTrack};
pub use space::{NEAREST, Projection, Space};
pub use storyboard::{Clip, Edge, Lying, MARKER_COLOURS, Marker, Mixed, Overlap, Storyboard, Transition, TransitionKind};
pub use template::{Part, Placed, TemplateInfo};

macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);
    };
}

id!(
    /// A shot's permanent id, unique in the project.
    ShotId
);
id!(
    /// A storyboard's permanent id, unique in the project.
    StoryboardId
);
id!(
    /// A layer's permanent id, unique in the project.
    LayerId
);
id!(
    /// A drawing's permanent id, unique in the project.
    DrawingId
);
id!(
    /// A peg's permanent id, unique in the project.
    PegId
);
id!(
    /// A Driver node's permanent id, unique in the project (plan §8.8).
    DriverId
);
id!(
    /// A composite's permanent id, unique in the project.
    CompositeId
);
id!(
    /// A camera's permanent id, unique in the project.
    CameraId
);
id!(
    /// A bin's permanent id, unique in the project.
    BinId
);
id!(
    /// A media item's permanent id, unique in the project.
    MediaId
);
id!(
    /// A sound clip's permanent id, unique in the project.
    SoundClipId
);

/// A whole project (plan §4.1): a bin of items. Shots and storyboards are items; sound, images
/// and footage join them with later milestones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub settings: Settings,
    pub palette: Arc<Palette>,
    /// Every shot of the project, in the order they were made. A shot is here whether or not a
    /// storyboard uses it. Names are unique among them.
    pub shots: Vec<Arc<Shot>>,
    /// Names are unique among them. A shot has a clip in at most one storyboard.
    pub storyboards: Vec<Arc<Storyboard>>,
    /// The bins the user orders the items in (plan §7.11), beside Master, which is not listed.
    #[serde(default)]
    pub bins: Vec<Bin>,
    /// The pictures and sound that were brought in (plan §7.11), in the order they were. Each
    /// refers to a file on disk. Names are unique among them.
    #[serde(default)]
    pub media: Vec<Arc<Media>>,
    /// The renders that were made (plan §7.14), each with how its shot was then.
    #[serde(default)]
    pub renders: Vec<RenderRecord>,
    /// A number that is another whenever the palette or the settings are: with a shot's
    /// revision it says whether a render is still of the project as it is.
    #[serde(default)]
    pub look: u64,
    /// The next id [`Project::new_id`] hands out. Ids are never reused.
    next_id: u64,
}

/// How the compositor blends layers and runs effects (plan §4.11). Palettes and drawings
/// store sRGB colours either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkingSpace {
    /// Blending behaves like most 2D tools: colours look as picked after blending.
    #[default]
    Srgb,
    /// Blending in linear light: glows, blurs and depth of field look more natural.
    Linear,
}

/// Project-wide settings. Defaults are the plan's: HD at 24 fps, drawings at twice that.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Shot resolution in pixels.
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    /// The pixel density new layers get (see [`Layer::density`]).
    pub drawing_density: f64,
    #[serde(default)]
    pub working_space: WorkingSpace,
    /// The master volume: how loud all of the project's sound is, played and in a movie. 1
    /// is each clip as loud as it says.
    #[serde(default = "Settings::full_volume", skip_serializing_if = "Settings::is_full_volume")]
    pub volume: f32,
}

impl Settings {
    fn full_volume() -> f32 {
        1.0
    }

    fn is_full_volume(volume: &f32) -> bool {
        *volume == 1.0
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self { width: 1920, height: 1080, frame_rate: FrameRate::FPS_24, drawing_density: 2.0, working_space: WorkingSpace::Srgb, volume: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Swatch {
    pub id: SwatchId,
    pub name: String,
    /// sRGB with straight alpha, as picked.
    pub color: [u8; 4],
    /// The named palette the swatch is in (see [`SwatchSet`]).
    #[serde(default, skip_serializing_if = "SetId::is_default")]
    pub set: SetId,
    /// What it paints with (plan §4.20): its colour, or a gradient or a picture, of which
    /// `color` is then the first colour or the picture's own.
    #[serde(default, skip_serializing_if = "SwatchKind::is_solid")]
    pub kind: SwatchKind,
    /// How far its colour drifts at most, where a vector stroke's colour drifts (the user's of
    /// 2026-11-04): its hue turned so many degrees (-180 to 180), its saturation and value moved
    /// so many hundredths (-100 to 100), added to its colour, so a swatch given another colour
    /// drifts as far from that. `None`, or nothing to add, and it does not drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<[i16; 3]>,
}

/// `color` (sRGB, straight alpha) drifted by `amount` (0..1) of `drift` (see [`Swatch::drift`]):
/// its hue turned and its saturation and value moved in HSV, its alpha kept.
pub fn drifted(color: [u8; 4], drift: [i16; 3], amount: f32) -> [u8; 4] {
    let [r, g, b] = [color[0], color[1], color[2]].map(|c| f32::from(c) / 255.0);
    let (most, least) = (r.max(g).max(b), r.min(g).min(b));
    let range = most - least;
    let hue = match range {
        0.0 => 0.0,
        _ if most == r => 60.0 * ((g - b) / range).rem_euclid(6.0),
        _ if most == g => 60.0 * ((b - r) / range + 2.0),
        _ => 60.0 * ((r - g) / range + 4.0),
    };
    let saturation = if most > 0.0 { range / most } else { 0.0 };
    let amount = amount.clamp(0.0, 1.0);
    let hue = (hue + f32::from(drift[0]) * amount).rem_euclid(360.0);
    let saturation = (saturation + f32::from(drift[1]) / 100.0 * amount).clamp(0.0, 1.0);
    let value = (most + f32::from(drift[2]) / 100.0 * amount).clamp(0.0, 1.0);
    let chroma = value * saturation;
    let x = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    let [r, g, b] = [r, g, b].map(|c| ((c + m) * 255.0).round().clamp(0.0, 255.0) as u8);
    [r, g, b, color[3]]
}

/// What a swatch paints with (plan §4.20). Where a gradient or a picture lies on a filled
/// area is the drawing's ([`iskra_drawing::Placement`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SwatchKind {
    /// Its one colour.
    #[default]
    Solid,
    Gradient(Gradient),
    Texture(SwatchTexture),
}

impl SwatchKind {
    pub fn is_solid(&self) -> bool {
        matches!(self, SwatchKind::Solid)
    }
}

/// A texture swatch's picture: an image of the project's media, repeated when `tile`, one of
/// its pixels `scale` of a drawing's where it is fitted to an area. Not repeated, it is shown
/// `once`, the area past it in the swatch's colour, or else with its edges drawn out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwatchTexture {
    pub media: MediaId,
    #[serde(default = "SwatchTexture::tiled")]
    pub tile: bool,
    #[serde(default = "SwatchTexture::whole")]
    pub scale: f32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub once: bool,
}

// A scale is always finite.
impl Eq for SwatchTexture {}

impl SwatchTexture {
    fn tiled() -> bool {
        true
    }

    fn whole() -> f32 {
        1.0
    }
}

/// The id of a named palette of the project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SetId(pub u16);

impl SetId {
    /// The palette a project starts with, which is there for everything.
    pub const DEFAULT: SetId = SetId(0);

    fn is_default(&self) -> bool {
        *self == SetId::DEFAULT
    }
}

/// A named palette (plan §4.8): the swatches of one character, one location, or the
/// *Default* that is there for everything, with its own styles. A project has several, used
/// side by side. What is of a palette says so, the swatches and the styles, which lie in
/// the project's one table: their ids are one run, so a drawing names a swatch and needs no
/// palette.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwatchSet {
    pub id: SetId,
    pub name: String,
}

fn default_sets() -> Vec<SwatchSet> {
    vec![SwatchSet { id: SetId::DEFAULT, name: "Default".to_string() }]
}

/// A palette style's permanent id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StyleId(pub u16);

/// A style of a named palette (plan §4.8): its swatches with other colours, as *Night* is to
/// *Day*. A swatch the style gives no colour has the palette's own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Style {
    pub id: StyleId,
    pub name: String,
    /// sRGB with straight alpha, by swatch.
    pub colors: BTreeMap<SwatchId, [u8; 4]>,
    /// The named palette the style is of: it gives colours to that one's swatches only.
    #[serde(default, skip_serializing_if = "SetId::is_default")]
    pub set: SetId,
    /// Gradients and pictures it gives swatches in place of their own (plan §4.20). A swatch
    /// it gives a colour and no kind is solid in it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kinds: BTreeMap<SwatchId, SwatchKind>,
}

/// What [`Palette::merge`] did: how many swatches it added, how many it gave another colour,
/// and how many styles it added or changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Merged {
    pub added: usize,
    pub updated: usize,
    pub styles: usize,
}

/// The project's colours. Drawings refer to swatches by id, so renaming, reordering or
/// recolouring a swatch never touches a drawing (plan §4.8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Palette {
    /// Every swatch of every named palette, each in the order it has in its own.
    pub swatches: Vec<Swatch>,
    /// No swatch made from now on gets an id below this: the id of a swatch that was removed
    /// is not given out again, since drawings may still be painted with it.
    #[serde(default)]
    next_swatch: u16,
    /// The palette's styles, beside its own colours.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<Style>,
    /// The named palettes the swatches are in, in the order they are listed. There is always
    /// at least one.
    #[serde(default = "default_sets")]
    pub sets: Vec<SwatchSet>,
    /// The skins (plan §4.8): named sets of drawings shown in place of the day ones, as the
    /// styles are colours shown in place of the swatches' own, and found by the same names.
    /// Names are unique among them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skins: Vec<Skin>,
    /// Of a palette as styles show it, the palette it was made from, with its own colours and
    /// its styles: what a Style node shows a picture in another style from. Not saved.
    #[serde(skip)]
    own: Option<Arc<Palette>>,
    /// The pictures of its texture swatches, by their media, as read from their files. Not
    /// saved: the engine reads them when a project is opened and when a swatch is given one.
    #[serde(skip)]
    pub pictures: BTreeMap<MediaId, Arc<Picture>>,
}

impl Default for Palette {
    fn default() -> Self {
        Self::of(Vec::new())
    }
}

impl Palette {
    /// The palette a new project starts with.
    pub fn standard() -> Self {
        let swatch =
            |id, name: &str, color| Swatch { id: SwatchId(id), name: name.to_string(), color, set: SetId::DEFAULT, kind: SwatchKind::Solid, drift: None };
        Self::of(vec![
            swatch(1, "Black", [0, 0, 0, 255]),
            swatch(2, "White", [255, 255, 255, 255]),
            swatch(3, "Red", [220, 50, 47, 255]),
            swatch(4, "Green", [64, 160, 43, 255]),
            swatch(5, "Blue", [38, 110, 210, 255]),
        ])
    }

    /// A palette of `swatches`, which have their ids.
    pub fn of(swatches: Vec<Swatch>) -> Self {
        Self { swatches, next_swatch: 0, styles: Vec::new(), sets: default_sets(), skins: Vec::new(), own: None, pictures: BTreeMap::new() }
    }

    /// An id for a new swatch: above every id the palette has, and every one it has given
    /// out. `None` when the ids have run out.
    pub fn new_id(&mut self) -> Option<SwatchId> {
        let above = self.swatches.iter().map(|swatch| swatch.id.0).max().unwrap_or(0).checked_add(1)?;
        let id = above.max(self.next_swatch);
        // The ids above are drifted colours'.
        if SwatchId(id).is_drifted() || id == u16::MAX {
            return None;
        }
        self.next_swatch = id.checked_add(1)?;
        Some(SwatchId(id))
    }

    /// Take a swatch out. Its id is not given out again.
    pub fn remove(&mut self, id: SwatchId) -> Option<Swatch> {
        let at = self.swatches.iter().position(|swatch| swatch.id == id)?;
        self.next_swatch = self.next_swatch.max(id.0.saturating_add(1));
        for style in &mut self.styles {
            style.colors.remove(&id);
        }
        Some(self.swatches.remove(at))
    }

    pub fn set(&self, id: SetId) -> Option<&SwatchSet> {
        self.sets.iter().find(|set| set.id == id)
    }

    /// The named palette a swatch is in: its own, or the first where that one is gone.
    pub fn set_of(&self, swatch: &Swatch) -> SetId {
        self.set(swatch.set).or(self.sets.first()).map_or(SetId::DEFAULT, |set| set.id)
    }

    /// The swatches of one named palette, in their order.
    pub fn swatches_of(&self, set: SetId) -> impl Iterator<Item = &Swatch> {
        self.swatches.iter().filter(move |swatch| self.set_of(swatch) == set)
    }

    /// Add a named palette, with no swatches yet. `None` when the ids have run out.
    pub fn new_set(&mut self, name: &str) -> Option<SetId> {
        let id = SetId(self.sets.iter().map(|set| set.id.0).max().map_or(Some(0), |most| most.checked_add(1))?);
        self.sets.push(SwatchSet { id, name: name.to_string() });
        Some(id)
    }

    /// The named palette of this one that is called as `set` of `other` is, made if there is
    /// none: where swatches that come from another project belong here.
    fn set_like(&mut self, other: &Palette, set: SetId) -> SetId {
        let name = other.set(set).or(other.sets.first()).map_or("Default", |set| set.name.as_str());
        match self.sets.iter().find(|own| own.name == name) {
            Some(own) => own.id,
            None => self.new_set(name).unwrap_or(SetId::DEFAULT),
        }
    }

    /// Take in the swatches of `other` that this palette has none of, by their ids: those of
    /// a shot or a template that comes from another project and was painted with them. Each
    /// goes into the named palette called as its own was. The other's styles come with them:
    /// one of a name its palette has none of here is added, and one it has gets the colours
    /// it has none for, so that nothing here is changed and a character that comes has its
    /// *Night* colours with it.
    pub fn adopt(&mut self, other: &Palette) {
        let lacking: Vec<Swatch> = other.swatches.iter().filter(|swatch| self.swatch(swatch.id).is_none()).cloned().collect();
        for swatch in lacking {
            let set = self.set_like(other, swatch.set);
            self.swatches.push(Swatch { set, ..swatch });
        }
        for style in other.styles.iter().filter(|style| !style.colors.is_empty()) {
            // Of the colours it gives, those for swatches that are of its palette here.
            let set = self.set_like(other, style.set);
            let colors = style.colors.iter().filter(|(swatch, _)| self.swatch(**swatch).is_some_and(|swatch| self.set_of(swatch) == set));
            let colors: BTreeMap<SwatchId, [u8; 4]> = colors.map(|(swatch, color)| (*swatch, *color)).collect();
            match self.styles.iter_mut().find(|own| own.name == style.name && own.set == set) {
                Some(own) => colors.into_iter().for_each(|(swatch, color)| _ = own.colors.entry(swatch).or_insert(color)),
                None => {
                    let Some(id) = self.new_style_id().filter(|_| !colors.is_empty()) else { continue };
                    self.styles.push(Style { id, name: style.name.clone(), colors, set, kinds: BTreeMap::new() });
                }
            }
        }
    }

    /// Bring in the swatches and the styles of another palette, by their names (plan §4.8),
    /// into the named palette `into`: what that has no swatch of the name for is added with
    /// a new id, and with `update` a swatch of the same name gets the other's colour. The
    /// other's styles come the same way: a new one whole, one of a name this palette has
    /// only with `update`.
    pub fn merge(&mut self, other: &Palette, update: bool, into: SetId) -> Merged {
        let mut merged = Merged::default();
        // The other's swatches as they are here.
        let mut here: BTreeMap<SwatchId, SwatchId> = BTreeMap::new();
        for swatch in &other.swatches {
            match self.swatches.iter_mut().find(|own| own.name == swatch.name && own.set == into) {
                Some(own) => {
                    here.insert(swatch.id, own.id);
                    if update && (own.color != swatch.color || own.kind != swatch.kind) {
                        (own.color, own.kind) = (swatch.color, swatch.kind.clone());
                        merged.updated += 1;
                    }
                }
                None => {
                    let Some(id) = self.new_id() else { break };
                    self.swatches.push(Swatch { id, set: into, ..swatch.clone() });
                    here.insert(swatch.id, id);
                    merged.added += 1;
                }
            }
        }
        for style in &other.styles {
            let colors: BTreeMap<SwatchId, [u8; 4]> = style.colors.iter().filter_map(|(swatch, color)| Some((*here.get(swatch)?, *color))).collect();
            match self.styles.iter_mut().find(|own| own.name == style.name && own.set == into) {
                Some(own) if update && own.colors != colors => {
                    own.colors.extend(colors);
                    merged.styles += 1;
                }
                Some(_) => {}
                None => {
                    let Some(id) = self.new_style_id() else { break };
                    self.styles.push(Style { id, name: style.name.clone(), colors, set: into, kinds: BTreeMap::new() });
                    merged.styles += 1;
                }
            }
        }
        merged
    }

    /// The swatch whose own colour is nearest to `color`, by its red, green and blue.
    pub fn nearest(&self, color: [u8; 4]) -> Option<SwatchId> {
        let apart = |swatch: &&Swatch| (0..3).map(|c| (i32::from(swatch.color[c]) - i32::from(color[c])).pow(2)).sum::<i32>();
        self.swatches.iter().min_by_key(apart).map(|swatch| swatch.id)
    }

    pub fn style(&self, id: StyleId) -> Option<&Style> {
        self.styles.iter().find(|style| style.id == id)
    }

    /// An id for a new style, above every id the palette's styles have.
    pub fn new_style_id(&self) -> Option<StyleId> {
        self.styles.iter().map(|style| style.id.0).max().unwrap_or(0).checked_add(1).map(StyleId)
    }

    /// A swatch's colour under `styles`, of which each named palette has at most one: what
    /// the style of the swatch's palette gives it, or the palette's own where that gives
    /// none, or no style of its palette is among them.
    pub fn color_in(&self, styles: &[StyleId], id: SwatchId) -> [u8; 4] {
        let styled = styles.iter().filter_map(|style| self.style(*style)).find_map(|style| style.colors.get(&id));
        styled.copied().unwrap_or_else(|| self.color(id))
    }

    /// What a swatch paints with under `styles`, as [`Palette::color_in`] gives its colour: the
    /// kind the style of its palette gives it, solid where that gives it a colour alone, or
    /// its own.
    pub fn kind_in(&self, styles: &[StyleId], id: SwatchId) -> SwatchKind {
        let style = styles.iter().filter_map(|style| self.style(*style)).find(|style| style.kinds.contains_key(&id) || style.colors.contains_key(&id));
        match style {
            Some(style) => style.kinds.get(&id).cloned().unwrap_or_default(),
            None => self.swatch(id).map(|swatch| swatch.kind.clone()).unwrap_or_default(),
        }
    }

    /// The styles of one named palette, in their order.
    pub fn styles_of(&self, set: SetId) -> impl Iterator<Item = &Style> {
        self.styles.iter().filter(move |style| style.set == set)
    }

    /// The one of `styles` that is of the named palette `set`.
    pub fn style_among(&self, styles: &[StyleId], set: SetId) -> Option<&Style> {
        styles.iter().filter_map(|style| self.style(*style)).find(|style| style.set == set)
    }

    /// The swatches as `styles` show them: each with the colour it has there, and no styles
    /// left to apply.
    pub fn styled(&self, styles: &[StyleId]) -> Palette {
        let styled = |swatch: &Swatch| Swatch { color: self.color_in(styles, swatch.id), kind: self.kind_in(styles, swatch.id), ..swatch.clone() };
        let swatches = self.swatches.iter().map(styled).collect();
        Palette {
            swatches,
            next_swatch: self.next_swatch,
            styles: Vec::new(),
            sets: self.sets.clone(),
            skins: self.skins.clone(),
            own: Some(self.own()),
            pictures: self.pictures.clone(),
        }
    }

    /// The palette this one was made from by [`Palette::styled`], or itself.
    fn own(&self) -> Arc<Palette> {
        self.own.clone().unwrap_or_else(|| Arc::new(self.clone()))
    }

    /// This palette with the swatches of every named palette that has a style called `name`
    /// in that style's colours, and the rest as they are here: what a Style node shows its
    /// picture in. With no name, those swatches in the palette's own colours. With the name of
    /// a `palette`, only the swatches of the named palette called so; with none, of every one.
    pub fn restyled(&self, palette: &str, name: &str) -> Palette {
        let own = self.own();
        let picked = |swatch: &Swatch| palette.is_empty() || own.set(own.set_of(swatch)).is_some_and(|set| set.name == palette);
        let colour = |swatch: &Swatch| match name.is_empty() {
            _ if !picked(swatch) => None,
            true => Some((own.color(swatch.id), own.kind_in(&[], swatch.id))),
            false => own
                .styles_of(swatch.set)
                .find(|style| style.name == name)
                .map(|style| (own.color_in(&[style.id], swatch.id), own.kind_in(&[style.id], swatch.id))),
        };
        let restyled = |swatch: &Swatch| match colour(swatch) {
            Some((color, kind)) => Swatch { color, kind, ..swatch.clone() },
            None => swatch.clone(),
        };
        let swatches = self.swatches.iter().map(restyled).collect();
        Palette {
            swatches,
            next_swatch: self.next_swatch,
            styles: Vec::new(),
            sets: self.sets.clone(),
            skins: self.skins.clone(),
            own: Some(own),
            pictures: self.pictures.clone(),
        }
    }

    /// The names of the styles of the named palette called `palette`, or with none of every
    /// one, each once, in the order they first come.
    pub fn style_names(&self, palette: &str) -> Vec<String> {
        let own = self.own.as_deref().unwrap_or(self);
        let mut names: Vec<String> = Vec::new();
        for style in &own.styles {
            let of = palette.is_empty() || own.set(style.set).is_some_and(|set| set.name == palette);
            if of && !names.contains(&style.name) {
                names.push(style.name.clone());
            }
        }
        names
    }

    pub fn swatch(&self, id: SwatchId) -> Option<&Swatch> {
        self.swatches.iter().find(|s| s.id == id)
    }

    /// A swatch's colour. A swatch the palette doesn't have shows as opaque magenta, so missing
    /// colours are easy to spot. A drifted colour's id ([`SwatchId::drifted`]) is its swatch's
    /// colour drifted that far, or its colour where it has no drift.
    pub fn color(&self, id: SwatchId) -> [u8; 4] {
        if let Some(swatch) = self.swatch(id) {
            return swatch.color;
        }
        match self.swatch(id.base()) {
            Some(swatch) if id.is_drifted() => match swatch.drift {
                Some(drift) => drifted(swatch.color, drift, f32::from(id.drift()) / f32::from(SwatchId::DRIFTS)),
                None => swatch.color,
            },
            _ => [255, 0, 255, 255],
        }
    }

    /// The colours of the swatches' drifted ids, each with its id, at every step: a swatch's
    /// own colour where it does not drift.
    pub fn drifted_colors(&self) -> impl Iterator<Item = (SwatchId, [u8; 4])> + '_ {
        self.swatches
            .iter()
            .filter(|swatch| swatch.id.drifted(1) != swatch.id)
            .flat_map(move |swatch| (1..=SwatchId::DRIFTS).map(move |level| (swatch.id.drifted(level), self.color(swatch.id.drifted(level)))))
    }

    /// How a swatch paints (plan §4.20), as its own kind says: a palette shown in styles has
    /// its kinds in them already ([`Palette::styled`]). A texture whose picture was not read
    /// is its colour.
    pub fn shade(&self, id: SwatchId) -> Shade<'_> {
        let Some(swatch) = self.swatch(id) else { return Shade::Solid(self.color(id)) };
        match &swatch.kind {
            SwatchKind::Solid => Shade::Solid(swatch.color),
            SwatchKind::Gradient(gradient) => Shade::Gradient(gradient),
            SwatchKind::Texture(texture) => {
                let picture = self.pictures.get(&texture.media).map(Arc::as_ref);
                Shade::Texture { picture, tile: texture.tile, once: texture.once, scale: texture.scale, color: swatch.color }
            }
        }
    }

    /// Whether any swatch paints with more than one colour.
    pub fn is_shaded(&self) -> bool {
        self.swatches.iter().any(|swatch| !self.shade(swatch.id).is_solid())
    }

    /// The filled areas of a drawing that its gradient and texture swatches paint, for
    /// [`Palette::paint_in`]: none where every swatch is solid.
    pub fn areas_of(&self, drawing: &Drawing) -> Areas {
        if self.is_shaded() { drawing.areas(|swatch| self.shade(swatch)) } else { Areas::default() }
    }

    /// The drawing's pixels of `rect` in this palette's colours, its gradients and pictures
    /// where `areas` (of [`Palette::areas_of`]) has them; gradients blended in `space`.
    pub fn paint_in(&self, drawing: &Drawing, rect: Rect, areas: &Areas, space: WorkingSpace) -> Vec<[u8; 4]> {
        if areas.is_empty() {
            return drawing.to_rgba8(rect, |swatch| self.color(swatch));
        }
        drawing.to_rgba8_shaded(rect, areas, |swatch| self.shade(swatch), space == WorkingSpace::Linear)
    }

    /// The drawing's pixels of `rect` in this palette's colours, gradients and pictures.
    pub fn paint(&self, drawing: &Drawing, rect: Rect, space: WorkingSpace) -> Vec<[u8; 4]> {
        self.paint_in(drawing, rect, &self.areas_of(drawing), space)
    }
}

/// How shots renamed together are named: a text and a number after it, the first shot's
/// number, the step from one shot's to the next, and how many digits it is padded to with
/// zeros, as `SH010`, `SH020`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Numbering {
    pub text: String,
    pub first: u32,
    pub step: u32,
    pub digits: u32,
}

impl Default for Numbering {
    fn default() -> Self {
        Numbering { text: "SH".to_string(), first: 10, step: 10, digits: 3 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shot {
    pub id: ShotId,
    pub name: String,
    /// Length in frames: the whole shot, handles included.
    pub duration: u32,
    /// The shot's drawing layers, the newest first. No two of the shot's nodes (layers, pegs
    /// and composites) share a name.
    pub layers: Vec<Arc<Layer>>,
    /// The shot's pegs, in the order they were made. Their parents make them a tree, with the
    /// layers as its leaves.
    pub pegs: Vec<Arc<Peg>>,
    /// The shot's composites, in the order they were made. Which layer is over which is the
    /// order of a composite's inputs, not the order of `layers`.
    pub composites: Vec<Arc<Composite>>,
    /// The shot's deformers (plan §4.5), in the order they were made: what bends the drawings
    /// of the layers that are given to them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deformers: Vec<Arc<Deformer>>,
    /// The shot's groups (plan §4.3), in the order they were made: some of its nodes handled
    /// as one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Arc<Group>>,
    /// The shot's effect nodes (plan §8), in the order they were made: what a picture goes
    /// through on its way from a layer or a composite to what is seen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Arc<Effect>>,
    /// The shot's Driver nodes (plan §8.8), in the order they were made: what adds to its
    /// pegs' values, worked out on every frame from other pegs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drivers: Vec<Arc<Driver>>,
    /// What the shot's main display shows (see `display`): usually a composite. Nothing is
    /// seen of a shot without one.
    pub display: Option<Source>,
    /// The main display's name.
    #[serde(default = "display::main_display_name", skip_serializing_if = "display::is_main_display_name")]
    pub display_name: String,
    /// The shot's other displays, in the order they were made: other pictures of it, for the
    /// viewer alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub displays: Vec<Display>,
    /// The shot's one camera: what the display shows is seen through it.
    pub camera: Camera,
    /// Where nodes have been put in the node view, by their ids; the display's is under the
    /// shot's own id. A node that was never moved has no place here and is laid out by the
    /// view.
    #[serde(default)]
    pub places: BTreeMap<u64, [f64; 2]>,
    /// The colours the user gave nodes in the node view (plan §8.3), by their ids, as sRGB. A
    /// node with none here has the colour of its kind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colours: BTreeMap<u64, [u8; 3]>,
    /// The shot's drawing guides: rulers and perspective, which a stroke can be held to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guides: Vec<DrawGuide>,
    /// The styles the shot is shown and rendered in (plan §4.8): at most one of each named
    /// palette. A palette with none here shows its own colours.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<StyleId>,
    /// The skin the shot is shown and rendered in (plan §4.8): its layers' variants in it in
    /// place of their day drawings. None shows the day drawings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin: Option<SkinId>,
    /// The shot's panels after its first (plan §4.7), in the order of their frames, one a
    /// frame; and the first too, on frame 0, once something is written of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panels: Vec<Panel>,
    /// The pegs whose rows on the timeline are folded: the rows of what they carry are put
    /// away. The id of a peg that was deleted may stay here, and names nothing.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub folded: BTreeSet<PegId>,
    /// The boards opened to be drawn on on the Animation page, where a board is otherwise
    /// locked as reference (plan §7.2c). The id of a layer that was deleted may stay here.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub unlocked: BTreeSet<LayerId>,
    /// The rows of the timeline that are hidden, by the ids of their nodes: layers, pegs and
    /// groups. A hidden layer is not in what the shot shows, nor is one that a hidden peg
    /// carries or that is in a hidden group. The id of a node that was deleted may stay here,
    /// and names nothing.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub hidden: BTreeSet<u64>,
    /// The rows of the timeline that are locked, by the ids of their nodes: layers, pegs and
    /// groups. A locked layer is not drawn on nor picked in the viewport, nor is one that a
    /// locked peg carries or that is in a locked group; it is shown and rendered as ever. The
    /// id of a node that was deleted may stay here, and names nothing.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub locked: BTreeSet<u64>,
    /// Marks on the shot's frames, in their order, one a frame, as a storyboard has them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// The retake notes on the shot's frames, in their order (plan §9.6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// The stretch of the shot that is played on the Animation page, and gone round when it
    /// loops: its first frame and the one after its last. Without one, all of the shot is.
    /// It is for playing only: a render is of the cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<(u32, u32)>,
    /// Where the shot is in its life (plan §7.2a).
    #[serde(default)]
    pub status: Status,
    /// Who has the shot, while it is out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_to: Option<String>,
    /// The bin the shot is in. `None` is Master.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<BinId>,
    /// The scene the shot is given to, to organise the shots by: a name, empty for none. It
    /// holds nothing the shots of a scene share.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scene: String,
    /// The sequence the shot is given to, as its scene is; several lie on one storyboard.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sequence: String,
    /// A number that is another after every change to what the shot looks like (see
    /// [`RenderRecord`]). Not a count: only whether it is the same matters.
    #[serde(default)]
    pub revision: u64,
    /// Whether this snapshot holds the pixels of the shot's drawings. A project read from a file
    /// starts with no shot loaded: the drawings have their layers and nothing painted, until
    /// the store loads the shot (plan §6, "One project file at film size"). A shot that is not
    /// loaded has its pixels in the file exactly as they were last saved. Not part of the file.
    #[serde(skip)]
    pub loaded: bool,
}

// As for `Layer`: the places of nodes are always finite.
impl Eq for Shot {}

/// What a layer is for (plan §7.2c). The Storyboard page shows and plays board layers
/// only; renders use animation layers only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Board,
    Animation,
}

/// One drawing layer: a pool of drawings and the track that says which one shows on each frame
/// (plan §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub role: Role,
    /// The peg the layer is parented to, in the same shot.
    pub parent: Option<PegId>,
    /// Drawing pixels per scene unit. A scene unit is one shot pixel at z = 0, so 2 means the
    /// drawings have twice the shot's resolution (plan §4.10).
    pub density: f64,
    /// The drawing pool, in the order the drawings were made. Names are unique in the pool.
    pub drawings: Vec<PoolDrawing>,
    /// The pool's drawings in skins (plan §4.8), each in place of a day drawing where its skin
    /// is shown. At most one a drawing a skin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
    pub exposure: Exposure,
    /// The library template the layer is a copy from, if it is one (plan §4.12). It is a note
    /// for the user: nothing follows from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// The media item the layer's pictures were read from, for a layer that was made by
    /// placing one (R7). The pictures are the layer's own, kept in the project like drawings;
    /// the item is remembered so that they can be read again when its file changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaId>,
    /// The deformer of the shot that bends the layer's drawings, if one does (plan §4.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deformer: Option<DeformerId>,
    /// The art layers of the layer's drawings that are free colour (plan §4.8): painted with
    /// real colours that mix, and not following the palette. A new drawing is made so.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub free_colour: Vec<ArtLayerKind>,
    /// The art layers of the layer's drawings that are vector art (plan §4.16): shapes, not
    /// pixels. A new drawing is made so.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vector: Vec<ArtLayerKind>,
    /// The art layers of the layer's drawings that are switched off: not seen, in the window
    /// or in a render, and kept as they are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub art_off: Vec<ArtLayerKind>,
    /// Its node in the node view shows an output for each art layer beside its flat one, to
    /// wire one art layer alone from. Off, only those that carry a wire are shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub art_ports: bool,
    /// Of a layer of pictures brought in: their alpha left out, each pixel whole.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opaque: bool,
    /// Of a layer of pictures brought in: how their files keep their colours with the alpha.
    #[serde(default, skip_serializing_if = "Alpha::is_straight")]
    pub alpha: Alpha,
    /// A text layer (plan §4.19): its drawings are texts, each a drawing's boxes of text, laid
    /// out into its vector line art. They are exposed, held and substituted as drawings are.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub text: bool,
}

// As for `Drawing`: lets `Arc<Layer>` compare by address first. `density` is always finite.
impl Eq for Layer {}

/// A drawing in a layer's pool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolDrawing {
    pub id: DrawingId,
    /// "1", "2", "mouth-A"…
    pub name: String,
    pub drawing: Arc<Drawing>,
}

impl Default for Project {
    /// A project with one storyboard that holds one shot of two seconds, and in the shot one
    /// empty layer, ready to draw in.
    fn default() -> Self {
        let mut project = Self {
            settings: Settings::default(),
            palette: Arc::new(Palette::standard()),
            shots: Vec::new(),
            storyboards: Vec::new(),
            bins: Vec::new(),
            media: Vec::new(),
            renders: Vec::new(),
            look: 0,
            next_id: 1,
        };
        let mut first = project.new_shot("Shot_01", 48);
        let (shot, layer) = (first.id, project.new_layer("Drawing", Role::Animation));
        first.connect_on_top(layer.id);
        first.layers.push(Arc::new(layer));
        project.shots.push(Arc::new(first));
        let mut storyboard = Storyboard::new(StoryboardId(project.new_id()), "Storyboard");
        // An empty storyboard has room for any clip.
        let _ = storyboard.put(Clip { shot, start: 0, in_point: 0, out_point: 48 }, false);
        project.storyboards.push(Arc::new(storyboard));
        project
    }
}

/// What the peg that moves a shot's camera is called when the shot is made.
pub const CAMERA_PEG: &str = "Camera-P";

impl Shot {
    /// Give the shot the peg that moves its camera, as a shot of a project has from the
    /// start: above the camera, between it and what it was parented to.
    pub fn add_camera_peg(&mut self, id: PegId) {
        let (name, parent) = (unique_name(CAMERA_PEG, self.node_names()), self.camera.parent);
        self.pegs.push(Arc::new(Peg {
            id,
            name,
            parent,
            transform: Transform::default(),
            animation: Animation::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        self.camera.parent = Some(id);
    }
}

impl Project {
    /// The project a session starts with: the default one, its shot with the peg that moves
    /// its camera, as every shot that is made in the app has.
    pub fn starting() -> Self {
        let mut project = Self::default();
        let peg = PegId(project.new_id());
        if let Some(shot) = project.shots.first_mut() {
            Arc::make_mut(shot).add_camera_peg(peg);
        }
        project
    }

    /// The project the app opens with, and makes for *New Project*: the starting one, with a
    /// board under its shot's layer, in the shot's group for storyboard drawings, so that
    /// the Storyboard page has something to draw on from the start (plan §7.2c).
    pub fn fresh() -> Self {
        let mut project = Self::starting();
        // The storyboard's layers, under the animation layer, which is what is seen over them.
        if let Some(shot) = project.shots.first().map(|shot| shot.id) {
            project.add_storyboard_layers(shot, true);
        }
        project
    }

    /// A fresh id, unique in the project.
    pub fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// An empty shot with a fresh id, its display showing a composite that stacks nothing yet.
    /// The caller adds it to the project, and gives a shot of a project its camera's peg
    /// ([`Shot::add_camera_peg`]), which a template's shot has none of.
    pub fn new_shot(&mut self, name: &str, duration: u32) -> Shot {
        let (id, composite, camera) = (ShotId(self.new_id()), CompositeId(self.new_id()), CameraId(self.new_id()));
        Shot {
            id,
            name: name.to_string(),
            duration,
            layers: Vec::new(),
            pegs: Vec::new(),
            composites: vec![Arc::new(Composite::new(composite, "Composite", Vec::new()))],
            deformers: Vec::new(),
            groups: Vec::new(),
            effects: Vec::new(),
            drivers: Vec::new(),
            display: Some(Source::Composite(composite)),
            display_name: MAIN_DISPLAY.to_string(),
            displays: Vec::new(),
            camera: Camera::new(camera, self.settings.height),
            places: BTreeMap::new(),
            colours: BTreeMap::new(),
            guides: Vec::new(),
            styles: Vec::new(),
            skin: None,
            panels: Vec::new(),
            folded: BTreeSet::new(),
            unlocked: BTreeSet::new(),
            hidden: BTreeSet::new(),
            locked: BTreeSet::new(),
            markers: Vec::new(),
            notes: Vec::new(),
            range: None,
            status: Status::default(),
            out_to: None,
            bin: None,
            scene: String::new(),
            sequence: String::new(),
            revision: 0,
            loaded: true,
        }
    }

    /// An empty bitmap layer of free colour, with a fresh id and the project's drawing density.
    /// The caller adds it to a shot.
    pub fn new_layer(&mut self, name: &str, role: Role) -> Layer {
        Layer {
            id: LayerId(self.new_id()),
            name: name.to_string(),
            role,
            parent: None,
            density: self.settings.drawing_density,
            drawings: Vec::new(),
            variants: Vec::new(),
            exposure: Exposure::default(),
            template: None,
            media: None,
            deformer: None,
            // Bitmap art is plain colour: swatch-linked pixels are vector behaviour.
            free_colour: ArtLayerKind::ALL.to_vec(),
            vector: Vec::new(),
            art_off: Vec::new(),
            art_ports: false,
            opaque: false,
            alpha: Alpha::Straight,
            text: false,
        }
    }

    pub fn shot(&self, id: ShotId) -> Option<&Shot> {
        self.shots.iter().find(|s| s.id == id).map(|s| &**s)
    }

    pub fn shot_mut(&mut self, id: ShotId) -> Option<&mut Shot> {
        self.shots.iter_mut().find(|s| s.id == id).map(Arc::make_mut)
    }

    /// The names of the scenes the shots are given to, each once, in the order of the alphabet.
    pub fn scenes(&self) -> Vec<String> {
        self.shots.iter().map(|s| s.scene.clone()).filter(|n| !n.is_empty()).collect::<BTreeSet<_>>().into_iter().collect()
    }

    /// The names of the sequences the shots are given to, as `scenes`.
    pub fn sequences(&self) -> Vec<String> {
        self.shots.iter().map(|s| s.sequence.clone()).filter(|n| !n.is_empty()).collect::<BTreeSet<_>>().into_iter().collect()
    }

    /// `shots` in the order they lie in: by their storyboards, in the project's order, and
    /// their places on them; those on no storyboard after, in the order given.
    pub fn in_cut_order(&self, shots: &[ShotId]) -> Vec<ShotId> {
        let place = |shot: ShotId| {
            let (board, clip) = self.storyboards.iter().enumerate().find_map(|(at, board)| Some((at, board.clip(shot)?.start)))?;
            Some((board, clip))
        };
        let mut placed: Vec<(usize, u32, ShotId)> = shots.iter().filter_map(|&shot| place(shot).map(|(board, start)| (board, start, shot))).collect();
        placed.sort_by_key(|(board, start, _)| (*board, *start));
        let unplaced = shots.iter().copied().filter(|&shot| place(shot).is_none());
        let mut ordered: Vec<ShotId> = placed.into_iter().map(|(_, _, shot)| shot).chain(unplaced).collect();
        let mut seen = BTreeSet::new();
        ordered.retain(|shot| seen.insert(*shot));
        ordered
    }

    /// The names `numbering` gives `shots`, in the order they lie in, each with the shot not
    /// among them that has that name already, if one has.
    pub fn numbered(&self, shots: &[ShotId], numbering: &Numbering) -> Vec<(ShotId, String, Option<ShotId>)> {
        let renamed: BTreeSet<ShotId> = shots.iter().copied().collect();
        let Numbering { text, first, step, digits } = numbering;
        let digits = *digits as usize;
        self.in_cut_order(shots)
            .into_iter()
            .enumerate()
            .map(|(at, shot)| {
                let name = format!("{text}{:0digits$}", u64::from(*first) + at as u64 * u64::from(*step));
                let taken = self.shots.iter().find(|other| !renamed.contains(&other.id) && other.name == name).map(|other| other.id);
                (shot, name, taken)
            })
            .collect()
    }

    pub fn storyboard(&self, id: StoryboardId) -> Option<&Storyboard> {
        self.storyboards.iter().find(|s| s.id == id).map(|s| &**s)
    }

    pub fn storyboard_mut(&mut self, id: StoryboardId) -> Option<&mut Storyboard> {
        self.storyboards.iter_mut().find(|s| s.id == id).map(Arc::make_mut)
    }

    /// The storyboard that has a clip of `shot`.
    pub fn storyboard_of(&self, shot: ShotId) -> Option<&Storyboard> {
        self.storyboards.iter().find(|s| s.clip(shot).is_some()).map(|s| &**s)
    }

    /// A copy of a shot under a new name, with fresh ids throughout. The copy's drawings share
    /// their pixels with the originals until one of them is drawn on. The caller adds it to the
    /// project.
    pub fn duplicate_shot(&mut self, id: ShotId, name: &str) -> Option<Shot> {
        let mut copy = self.shot(id)?.clone();
        let id = ShotId(self.new_id());
        copy.reidentify(id, || self.new_id());
        copy.name = name.to_string();
        Some(copy)
    }

    /// The shot that holds `layer`.
    pub fn shot_of(&self, layer: LayerId) -> Option<&Shot> {
        self.shots.iter().find(|s| s.layer(layer).is_some()).map(|s| &**s)
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.shots.iter().find_map(|s| s.layer(id))
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        let shot = self.shots.iter_mut().find(|s| s.layer(id).is_some())?;
        Arc::make_mut(shot).layers.iter_mut().find(|e| e.id == id).map(Arc::make_mut)
    }

    /// The shot that holds `peg`.
    pub fn shot_of_peg(&self, peg: PegId) -> Option<&Shot> {
        self.shots.iter().find(|s| s.peg(peg).is_some()).map(|s| &**s)
    }

    pub fn peg_mut(&mut self, id: PegId) -> Option<&mut Peg> {
        let shot = self.shots.iter_mut().find(|s| s.peg(id).is_some())?;
        Arc::make_mut(shot).pegs.iter_mut().find(|p| p.id == id).map(Arc::make_mut)
    }
}

impl Shot {
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|e| e.id == id).map(|e| &**e)
    }

    /// Put a marker on the shot, in place of the one on its frame, if one is there.
    pub fn put_marker(&mut self, marker: Marker) {
        self.markers.retain(|other| other.frame != marker.frame);
        let at = self.markers.partition_point(|other| other.frame < marker.frame);
        self.markers.insert(at, marker);
    }

    /// The stretch that plays: the range that was set, as far as the shot reaches, or all of
    /// the shot. Its first frame and the one after its last.
    pub fn played(&self) -> (u32, u32) {
        let end = self.duration.max(1);
        match self.range {
            Some((first, after)) if first < end => (first, after.min(end).max(first + 1)),
            _ => (0, end),
        }
    }

    /// Give the shot the id `id` and every node and drawing of it a fresh one from `new_id`,
    /// with all that names them by id following: parents, the graph, exposures and the places
    /// in the node view.
    pub(crate) fn reidentify(&mut self, id: ShotId, mut new_id: impl FnMut() -> u64) {
        // Every node's new id, to carry over where it was put in the node view.
        let mut renamed: HashMap<u64, u64> = HashMap::from([(self.id.0, id.0)]);
        self.id = id;
        let pegs: HashMap<PegId, PegId> = self.pegs.iter().map(|peg| (peg.id, PegId(new_id()))).collect();
        // The effects' new ids; they are wired again below, with the rest of the graph. A port
        // stands where a peg would for what is parented to it, under its new id too.
        let effects: HashMap<EffectId, EffectId> = self.effects.iter().map(|effect| (effect.id, EffectId(new_id()))).collect();
        let copied = |peg: Option<PegId>| peg.and_then(|id| pegs.get(&id).copied().or_else(|| effects.get(&EffectId(id.0)).map(|port| PegId(port.0))));
        renamed.extend(pegs.iter().map(|(old, new)| (old.0, new.0)));
        for peg in &mut self.pegs {
            let peg = Arc::make_mut(peg);
            (peg.id, peg.parent) = (pegs[&peg.id], copied(peg.parent));
        }
        let composites: HashMap<CompositeId, CompositeId> = self.composites.iter().map(|c| (c.id, CompositeId(new_id()))).collect();
        let camera = CameraId(new_id());
        renamed.insert(self.camera.id.0, camera.0);
        (self.camera.id, self.camera.parent) = (camera, copied(self.camera.parent));
        let deformers: HashMap<DeformerId, DeformerId> = self.deformers.iter().map(|deformer| (deformer.id, DeformerId(new_id()))).collect();
        renamed.extend(deformers.iter().map(|(old, new)| (old.0, new.0)));
        for deformer in &mut self.deformers {
            let deformer = Arc::make_mut(deformer);
            deformer.id = deformers[&deformer.id];
            // The pegs and ports wired into its ports, under their new ids; one not copied with
            // it is let go.
            if let Some(ik) = &mut deformer.ik {
                ik.peg = copied(ik.peg);
            }
            deformer.start_peg = copied(deformer.start_peg);
            deformer.start_bind = deformer.start_bind.filter(|_| deformer.start_peg.is_some());
            deformer.end_peg = copied(deformer.end_peg);
        }
        for peg in &mut self.pegs {
            if let Some(follow) = peg.follows {
                Arc::make_mut(peg).follows = deformers.get(&follow.deformer).map(|deformer| Follow { deformer: *deformer, ..follow });
            }
        }
        let mut layers: HashMap<LayerId, LayerId> = HashMap::new();
        for layer in &mut self.layers {
            let layer = Arc::make_mut(layer);
            let old = std::mem::replace(&mut layer.id, LayerId(new_id()));
            layers.insert(old, layer.id);
            layer.parent = copied(layer.parent);
            layer.deformer = layer.deformer.and_then(|id| deformers.get(&id).copied());
            layer.renumber_drawings(&mut new_id);
        }
        renamed.extend(layers.iter().map(|(old, new)| (old.0, new.0)));
        renamed.extend(composites.iter().map(|(old, new)| (old.0, new.0)));
        renamed.extend(effects.iter().map(|(old, new)| (old.0, new.0)));
        // The groups, and what is in them under its new ids; what names nothing is let go.
        let groups: HashMap<u64, u64> = self.groups.iter().map(|group| (group.id.0, new_id())).collect();
        renamed.extend(groups.iter().map(|(old, new)| (*old, *new)));
        for group in &mut self.groups {
            let group = Arc::make_mut(group);
            group.id = GroupId(groups[&group.id.0]);
            group.members = group.members.iter().filter_map(|member| renamed.get(member).copied()).collect();
        }
        let displays: HashMap<u64, u64> = self.displays.iter().map(|display| (display.id.0, new_id())).collect();
        renamed.extend(displays.iter().map(|(old, new)| (*old, *new)));
        // A driver drives the copy of its peg and reads the copies of its inputs; one whose peg
        // was not copied goes, and an input whose peg was not is empty.
        self.drivers.retain(|driver| pegs.contains_key(&driver.drives));
        for driver in &mut self.drivers {
            let driver = Arc::make_mut(driver);
            let new = DriverId(new_id());
            renamed.insert(driver.id.0, new.0);
            (driver.id, driver.drives) = (new, pegs[&driver.drives]);
            for input in &mut driver.inputs {
                input.peg = input.peg.and_then(|peg| pegs.get(&peg).copied());
            }
        }
        self.places = self.places.iter().filter_map(|(node, place)| Some((*renamed.get(node)?, *place))).collect();
        self.colours = self.colours.iter().filter_map(|(node, colour)| Some((*renamed.get(node)?, *colour))).collect();
        self.folded = self.folded.iter().filter_map(|peg| pegs.get(peg).copied()).collect();
        self.hidden = self.hidden.iter().filter_map(|node| renamed.get(node).copied()).collect();
        self.unlocked = self.unlocked.iter().filter_map(|layer| renamed.get(&layer.0).map(|id| LayerId(*id))).collect();
        // The graph joins the nodes under their new ids.
        let joined = |source: Source| match source {
            Source::Layer(id) => layers.get(&id).copied().map(Source::Layer),
            Source::Art(id, kind) => layers.get(&id).map(|id| Source::Art(*id, kind)),
            Source::Composite(id) => composites.get(&id).copied().map(Source::Composite),
            Source::Effect(id) => effects.get(&id).copied().map(Source::Effect),
        };
        for effect in &mut self.effects {
            let effect = Arc::make_mut(effect);
            effect.id = effects[&effect.id];
            effect.parent = copied(effect.parent);
            effect.inputs = effect.inputs.iter().filter_map(|(port, input)| Some((port.clone(), joined(*input)?))).collect();
        }
        for composite in &mut self.composites {
            let composite = Arc::make_mut(composite);
            composite.id = composites[&composite.id];
            composite.inputs = composite.inputs.iter().filter_map(|input| joined(*input)).collect();
        }
        self.display = self.display.and_then(joined);
        for display in &mut self.displays {
            (display.id, display.shows) = (DisplayId(displays[&display.id.0]), display.shows.and_then(joined));
        }
    }

    /// Every drawing of the shot's layers, their variants in skins too.
    pub fn drawings(&self) -> impl Iterator<Item = &PoolDrawing> {
        self.layers.iter().flat_map(|e| e.drawings.iter().chain(e.variants.iter().map(|variant| &variant.drawing)))
    }

    /// The shot with each of its drawings replaced by what `f` gives for it.
    fn with_drawings(&self, loaded: bool, f: impl Fn(&PoolDrawing) -> Option<Arc<Drawing>>) -> Shot {
        let mut shot = Shot { loaded, ..self.clone() };
        for layer in &mut shot.layers {
            let layer = Arc::make_mut(layer);
            for entry in layer.drawings.iter_mut().chain(layer.variants.iter_mut().map(|variant| &mut variant.drawing)) {
                if let Some(drawing) = f(entry) {
                    entry.drawing = drawing;
                }
            }
        }
        shot
    }

    /// The shot, loaded: its drawings are those of `pixels`, as a store read them.
    pub fn with_pixels(&self, pixels: &HashMap<DrawingId, Arc<Drawing>>) -> Shot {
        self.with_drawings(true, |entry| pixels.get(&entry.id).cloned())
    }

    /// The shot, not loaded: its drawings keep their layers and hold no pixels.
    pub fn without_pixels(&self) -> Shot {
        self.with_drawings(false, |entry| Some(Arc::new(entry.drawing.without_pixels())))
    }
}

impl Layer {
    /// Whether the layer is a vector layer: all its art layers are vector art (plan §4.16).
    /// A layer holds one kind of art; one made before that was so may be mixed.
    pub fn is_vector(&self) -> bool {
        ArtLayerKind::ALL.iter().all(|kind| self.vector.contains(kind))
    }

    /// Whether some of the layer's art layers are vector art and some pixels: a layer made
    /// before a layer held one kind, kept as it is until its kind is set.
    pub fn is_mixed(&self) -> bool {
        !self.vector.is_empty() && !self.is_vector()
    }

    /// A drawing of the pool, or a variant of one in a skin, by its id.
    pub fn drawing(&self, id: DrawingId) -> Option<&PoolDrawing> {
        self.drawings.iter().find(|d| d.id == id).or_else(|| self.variants.iter().map(|variant| &variant.drawing).find(|d| d.id == id))
    }

    /// A drawing of the pool, or a variant of one in a skin, by its id, to change.
    pub fn drawing_mut(&mut self, id: DrawingId) -> Option<&mut Drawing> {
        let variants = self.variants.iter_mut().map(|variant| &mut variant.drawing);
        self.drawings.iter_mut().chain(variants).find(|d| d.id == id).map(|d| Arc::make_mut(&mut d.drawing))
    }

    /// An empty drawing as this layer's are: with the art layers that are free colour here,
    /// and those that are vector art.
    pub fn blank_drawing(&self) -> Arc<Drawing> {
        Arc::new(Drawing::blank(&self.free_colour, &self.vector))
    }

    /// The drawing exposed at `frame`.
    pub fn drawing_at(&self, frame: u32) -> Option<&PoolDrawing> {
        self.drawing(self.exposure.at(frame)?)
    }

    /// Whether `art` is a picture that was brought in: the colour art of a layer of media, read
    /// again from its file, so not painted on. Its line art and overart are drawn on.
    pub fn is_picture(&self, art: ArtLayerKind) -> bool {
        self.media.is_some() && art == ArtLayerKind::ColourArt
    }
}

/// A count of frames as timecode at `rate`: hours, minutes, seconds and frames.
pub fn timecode(frame: i64, rate: FrameRate) -> String {
    format_timecode_frames(frame, rate, false)
}

/// `base` if no name in `taken` equals it, else `base` with the first free number after it
/// ("Drawing", "Drawing 2", "Drawing 3"…). Names are unique among siblings (plan §4.12).
pub fn unique_name<'a>(base: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    let free = |name: &str| taken.clone().all(|t| t != name);
    if free(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base} {n}")).find(|name| free(name)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_project_has_a_storyboard_with_a_shot_and_a_layer_to_draw_in() {
        let project = Project::default();
        let [shot] = &project.shots[..] else { panic!("one shot") };
        let [layer] = &shot.layers[..] else { panic!("one layer") };
        let [storyboard] = &project.storyboards[..] else { panic!("one storyboard") };
        assert_eq!(storyboard.clips(), [Clip { shot: shot.id, start: 0, in_point: 0, out_point: 48 }], "the whole shot is in the cut");
        assert_eq!(project.storyboard_of(shot.id).map(|s| s.id), Some(storyboard.id));
        assert_eq!((shot.name.as_str(), shot.duration, layer.density, layer.role), ("Shot_01", 48, 2.0, Role::Animation));
        assert!(layer.drawings.is_empty() && layer.exposure.runs().is_empty());
        assert_eq!(project.shot_of(layer.id).map(|s| s.id), Some(shot.id));
    }

    #[test]
    fn ids_are_unique_and_never_reused() {
        let mut project = Project::default();
        let used = [project.shots[0].id.0, project.shots[0].layers[0].id.0];
        let a = project.new_id();
        let b = project.new_id();
        assert!(a != b && !used.contains(&a) && !used.contains(&b));
        let mut read: Project = serde_json::from_value(serde_json::to_value(&project).unwrap()).unwrap();
        assert_eq!(read.new_id(), project.new_id(), "the id counter is saved with the project");
        assert!(project.shots[0].loaded && !read.shots[0].loaded, "a project that was read has its pixels still to load");
    }

    #[test]
    fn editing_one_layer_leaves_the_rest_of_the_snapshot_shared() {
        let mut before = Project::default();
        let extra = before.new_layer("Background", Role::Animation);
        let first = before.shots[0].layers[0].id;
        Arc::make_mut(&mut before.shots[0]).layers.push(Arc::new(extra));

        let mut after = before.clone();
        after.layer_mut(first).unwrap().name = "Ball".to_string();
        assert_eq!(before.layer(first).unwrap().name, "Drawing", "the earlier snapshot is untouched");
        assert!(Arc::ptr_eq(&before.shots[0].layers[1], &after.shots[0].layers[1]), "the other layer is shared, not copied");
        assert!(Arc::ptr_eq(&before.palette, &after.palette));
    }

    #[test]
    fn a_duplicated_shot_shares_pixels_but_no_ids() {
        let mut project = Project::default();
        let original = project.shots[0].id;
        let drawing = DrawingId(project.new_id());
        let layer = project.layer_mut(project.shots[0].layers[0].id).unwrap();
        layer.drawings.push(PoolDrawing { id: drawing, name: "1".to_string(), drawing: Arc::default() });
        layer.exposure.set(0, 5, Some(drawing));
        let (arm, hand) = (PegId(project.new_id()), PegId(project.new_id()));
        let peg = |id, parent| {
            Arc::new(Peg {
                id,
                name: id.0.to_string(),
                parent,
                transform: Transform::default(),
                animation: Animation::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            })
        };
        Arc::make_mut(&mut project.shots[0]).pegs = vec![peg(arm, None), peg(hand, Some(arm))];
        project.layer_mut(project.shots[0].layers[0].id).unwrap().parent = Some(hand);
        let layer = project.shots[0].layers[0].id.0;
        Arc::make_mut(&mut project.shots[0]).places = BTreeMap::from([(layer, [10.0, 20.0]), (original.0, [0.0, 90.0]), (999, [1.0, 1.0])]);

        let copy = project.duplicate_shot(original, "Shot_02").unwrap();
        let (from, to) = (&project.shots[0].layers[0], &copy.layers[0]);
        assert!(copy.id != original && to.id != from.id && to.drawings[0].id != drawing);
        assert!(Arc::ptr_eq(&to.drawings[0].drawing, &from.drawings[0].drawing));
        assert_eq!(to.drawing_at(4).map(|d| d.id), Some(to.drawings[0].id), "the copy's timing shows the copy's drawings");
        assert_eq!((copy.name.as_str(), copy.duration, to.name.as_str()), ("Shot_02", 48, "Drawing"));
        let [new_arm, new_hand] = &copy.pegs[..] else { panic!("two pegs") };
        assert!(new_arm.id != arm && new_hand.id != hand && project.shot_of_peg(new_arm.id).is_none());
        assert_eq!((new_hand.parent, to.parent), (Some(new_arm.id), Some(new_hand.id)), "the copy's tree is of the copy's pegs");
        let (old, new) = (&project.shots[0].composites[0], &copy.composites[0]);
        assert!(new.id != old.id && copy.display == Some(Source::Composite(new.id)) && new.inputs == [Source::Layer(to.id)]);
        assert_eq!((copy.stack(), project.shots[0].stack()), (vec![to.id], vec![from.id]), "each shot shows its own layer");
        assert_eq!(copy.places, BTreeMap::from([(to.id.0, [10.0, 20.0]), (copy.id.0, [0.0, 90.0])]), "nodes keep their places in the node view");
        assert!(project.duplicate_shot(ShotId(999), "x").is_none());
    }

    #[test]
    fn unique_names_get_the_first_free_number() {
        let taken = ["Drawing", "Drawing 2", "Ball"];
        assert_eq!(unique_name("Peg", taken.iter().copied()), "Peg");
        assert_eq!(unique_name("Drawing", taken.iter().copied()), "Drawing 3");
        assert_eq!(unique_name("Ball", taken.iter().copied()), "Ball 2");
    }

    #[test]
    fn a_swatch_drifted_is_its_colour_moved_in_hue_saturation_and_value() {
        // Red turned a third of the way round is green; darker by half, half as bright.
        assert_eq!(drifted([255, 0, 0, 200], [120, 0, 0], 1.0), [0, 255, 0, 200]);
        assert_eq!(drifted([255, 0, 0, 255], [0, 0, -50], 1.0), [128, 0, 0, 255]);
        assert_eq!(drifted([255, 0, 0, 255], [120, 0, 0], 0.0), [255, 0, 0, 255]);
        let mut palette = Palette::standard();
        let red = SwatchId(3);
        palette.swatches[2].drift = Some([60, 0, 0]);
        // Its drifted ids: its colour that far along; another swatch's its own colour.
        assert_eq!(palette.color(red.drifted(SwatchId::DRIFTS)), drifted([220, 50, 47, 255], [60, 0, 0], 1.0));
        assert_eq!(palette.color(SwatchId(1).drifted(7)), [0, 0, 0, 255]);
        assert_eq!(palette.color(SwatchId(999).drifted(7)), [255, 0, 255, 255]);
        let (id, halfway) = palette.drifted_colors().find(|(id, _)| id.base() == red && id.drift() == 8).unwrap();
        assert_eq!((id, halfway), (red.drifted(8), drifted([220, 50, 47, 255], [60, 0, 0], 8.0 / 15.0)));
        // A style's colour drifts as far.
        palette.styles.push(Style {
            id: StyleId(1),
            name: "Night".into(),
            colors: [(red, [0, 0, 200, 255])].into(),
            set: SetId::DEFAULT,
            kinds: Default::default(),
        });
        assert_eq!(palette.styled(&[StyleId(1)]).color(red.drifted(SwatchId::DRIFTS)), drifted([0, 0, 200, 255], [60, 0, 0], 1.0));
    }

    #[test]
    fn a_missing_swatch_shows_magenta() {
        let palette = Palette::standard();
        assert_eq!(palette.color(SwatchId(1)), [0, 0, 0, 255]);
        assert_eq!(palette.color(SwatchId(999)), [255, 0, 255, 255]);
    }

    #[test]
    fn the_style_names_are_of_one_named_palette_or_of_every_one() {
        let mut palette = Palette::standard();
        let hero = palette.new_set("Hero").unwrap();
        let style = |id, name: &str, set| Style { id: StyleId(id), name: name.to_string(), colors: BTreeMap::new(), set, kinds: BTreeMap::new() };
        palette.styles.extend([style(1, "Night", SetId::DEFAULT), style(2, "Dusk", hero), style(3, "Night", hero)]);
        assert_eq!(palette.style_names(""), ["Night", "Dusk"]);
        assert_eq!(palette.style_names("Hero"), ["Dusk", "Night"]);
        assert_eq!(palette.style_names("Default"), ["Night"]);
        assert!(palette.style_names("Sky").is_empty());
        // A palette as a style shows it still lists them.
        assert_eq!(palette.styled(&[StyleId(1)]).style_names("Hero"), ["Dusk", "Night"]);
    }
}
