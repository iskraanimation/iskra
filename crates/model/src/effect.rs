//! Effect nodes (plan §8): the nodes that take pictures in and give one out. A blur, a glow
//! and a cutter are all one thing in the shot, an [`Effect`], and differ by their kind, which
//! says what ports and what parameters the node has. The kinds are described here as data,
//! so that the commands, the file and the window are written once for all of them; how a
//! kind makes its picture is the compositor's.
//!
//! An effect sits on a wire: what is wired into its `picture` port goes through it, and it is
//! itself a [`Source`], connected to a composite, to another effect or to the display.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Interp, PegId, Shot, Source, Track};

/// An effect node's permanent id, unique in the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectId(pub u64);

/// The port every kind has: the picture that goes through the node.
pub const PICTURE: &str = "picture";

/// What a port takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortKind {
    /// The picture the node works on.
    Picture,
    /// A picture used for its shape: where it is, and how solid.
    Matte,
}

/// An input of a kind of effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Port {
    pub name: &'static str,
    pub kind: PortKind,
    /// Without something wired into it the node gives no picture.
    pub needed: bool,
}

/// What a parameter is, with what it starts as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    /// A number from `min` to `max`, changed by `step` at a time, said in `unit`.
    Number {
        default: f64,
        min: f64,
        max: f64,
        step: f64,
        unit: &'static str,
    },
    /// One of `options`, the first to begin with.
    Choice {
        options: &'static [&'static str],
    },
    Switch {
        default: bool,
    },
    /// A colour: red, green, blue and how solid, each from 0 to 1, as sRGB.
    Colour {
        default: [f64; 4],
    },
    /// A few words: a name. Empty to begin with.
    Text,
    /// Curves of tone, as a paint program has them: straight to begin with.
    Curves,
}

/// A parameter of a kind of effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub hint: &'static str,
    pub kind: ParamKind,
}

impl ParamSpec {
    /// What a new node has for the parameter.
    pub fn default(&self) -> Value {
        match self.kind {
            ParamKind::Number { default, .. } => Value::Number(default),
            ParamKind::Choice { options } => Value::Choice(options.first().copied().unwrap_or_default().to_string()),
            ParamKind::Switch { default } => Value::Switch(default),
            ParamKind::Colour { default } => Value::Colour(default),
            ParamKind::Text => Value::Text(String::new()),
            ParamKind::Curves => Value::Curves(Curves::default()),
        }
    }

    /// `value` as the parameter takes it: a number within its range, one of its options.
    /// `Err` says what is wrong with it.
    pub fn checked(&self, value: Value) -> Result<Value, String> {
        match (self.kind, value) {
            (ParamKind::Number { min, max, .. }, Value::Number(number)) if number.is_finite() => Ok(Value::Number(number.clamp(min, max))),
            (ParamKind::Number { .. }, _) => Err(format!("`{}` is a number", self.id)),
            (ParamKind::Choice { options }, Value::Choice(choice)) if options.contains(&choice.as_str()) => Ok(Value::Choice(choice)),
            (ParamKind::Choice { options }, _) => Err(format!("`{}` is one of: {}", self.id, options.join(", "))),
            (ParamKind::Switch { .. }, Value::Switch(on)) => Ok(Value::Switch(on)),
            (ParamKind::Switch { .. }, _) => Err(format!("`{}` is true or false", self.id)),
            (ParamKind::Colour { .. }, Value::Colour(colour)) if colour.iter().all(|part| part.is_finite()) => {
                Ok(Value::Colour(colour.map(|part| part.clamp(0.0, 1.0))))
            }
            (ParamKind::Colour { .. }, _) => Err(format!("`{}` is a colour: red, green, blue and how solid, each from 0 to 1", self.id)),
            // A name is said as a choice is: by a string.
            (ParamKind::Text, Value::Text(text) | Value::Choice(text)) if text.chars().count() <= 100 => Ok(Value::Text(text.trim().to_string())),
            (ParamKind::Text, _) => Err(format!("`{}` is a name, of 100 letters at most", self.id)),
            (ParamKind::Curves, Value::Curves(curves)) => curves.checked().map(Value::Curves).map_err(|why| format!("`{}`: {why}", self.id)),
            (ParamKind::Curves, _) => Err(format!("`{}` is curves: `master`, `red`, `green` and `blue`, each a list of points", self.id)),
        }
    }
}

/// A kind of effect: what a node of it is called, where it is found, and what it has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectKind {
    /// Stable: files, commands and scripts name the kind by it.
    pub id: &'static str,
    pub title: &'static str,
    /// Where the kind is found in the menu that adds nodes (plan §8.4).
    pub category: &'static str,
    pub hint: &'static str,
    pub ports: &'static [Port],
    pub params: &'static [ParamSpec],
}

impl EffectKind {
    /// Whether a node of the kind makes a picture of its own, with nothing wired into it: a
    /// card of the frame's size, at the depth its parameter says (plan §8.4, *Sources*).
    pub fn makes_picture(&self) -> bool {
        SOURCES.contains(&self.id)
    }

    pub fn port(&self, name: &str) -> Option<&'static Port> {
        self.ports.iter().find(|port| port.name == name)
    }

    pub fn param(&self, id: &str) -> Option<&'static ParamSpec> {
        self.params.iter().find(|param| param.id == id)
    }
}

/// The kind that lays a picture onto what is under it by a mode (plan §8.4), and its modes.
pub const BLEND: &str = "blend";
pub const BLEND_MODES: [&str; 5] = ["Normal", "Add", "Screen", "Multiply", "Subtract"];

/// The port a kind that cuts or shades by another picture has for it.
pub const MATTE: &str = "matte";

/// What a node that only hands a picture on takes: with nothing wired in it hands on nothing.
const PASSING: &[Port] = &[Port { name: PICTURE, kind: PortKind::Picture, needed: false }];
const ONE_PICTURE: &[Port] = &[Port { name: PICTURE, kind: PortKind::Picture, needed: true }];
const PICTURE_AND_MATTE: &[Port] = &[Port { name: PICTURE, kind: PortKind::Picture, needed: true }, Port { name: MATTE, kind: PortKind::Matte, needed: false }];

/// A length in pixels of the frame, from nothing up to `max`.
const fn pixels(default: f64, max: f64) -> ParamKind {
    ParamKind::Number { default, min: 0.0, max, step: 0.5, unit: " px" }
}

/// How far something is moved aside, either way.
const OFFSET: ParamKind = ParamKind::Number { default: 8.0, min: -2000.0, max: 2000.0, step: 0.5, unit: " px" };

/// What a tone and a highlight have, which differ in the colour they start with.
const fn shade(colour: [f64; 4]) -> [ParamSpec; 4] {
    [
        ParamSpec {
            id: "width",
            title: "Width",
            hint: "With a matte, how much wider the shade is than the matte; without one, how far in from the picture's edge it reaches",
            kind: pixels(8.0, 500.0),
        },
        ParamSpec { id: "softness", title: "Softness", hint: "How soft the shade's edge is", kind: pixels(4.0, 500.0) },
        ParamSpec { id: "colour", title: "Colour", hint: "The colour laid over the picture", kind: ParamKind::Colour { default: colour } },
        ParamSpec {
            id: "amount",
            title: "Amount",
            hint: "How much of the colour is laid over: at 0 none",
            kind: ParamKind::Number { default: 60.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
        },
    ]
}
const SHADE_DARK: [ParamSpec; 4] = shade([0.1, 0.1, 0.3, 1.0]);
const SHADE_LIGHT: [ParamSpec; 4] = shade([1.0, 1.0, 1.0, 1.0]);

/// The kind that is a picture of one colour, the size of the frame.
pub const COLOUR_CARD: &str = "colourCard";
/// The kind that is rain or snow, made from the frame alone (plan §8.4, *Particles*).
pub const PARTICLES: &str = "particles";
/// The Particles node's port for a drawing to draw each particle as.
pub const SPRITE: &str = "sprite";
/// Its port for drawings to draw the droplets of rain's splashes as.
pub const SPLASH_SPRITE: &str = "splashSprite";
/// Its port for what a burst's particles bounce off, where it is painted.
pub const COLLIDER: &str = "collider";
/// The kinds that make a picture with nothing wired into them.
const SOURCES: [&str; 2] = [COLOUR_CARD, PARTICLES];
/// The parameter that says how near the viewer a picture a node makes stands.
pub const DEPTH: &str = "depth";
/// A place across or down from the frame's middle.
const CENTRE: ParamKind = ParamKind::Number { default: 0.0, min: -5000.0, max: 5000.0, step: 1.0, unit: " px" };
const PERCENT_30: ParamKind = ParamKind::Number { default: 30.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" };
const DEPTH_KIND: ParamKind = ParamKind::Number { default: 0.0, min: -100_000.0, max: 100_000.0, step: 1.0, unit: "" };

/// The kinds, by their categories in the order of plan §8.4. Sizes are in pixels of the frame,
/// whatever size the picture is made at (plan §8.2).
const KINDS: &[EffectKind] = &[
    EffectKind {
        id: COLOUR_CARD,
        title: "Colour Card",
        category: "Sources",
        hint: "A picture of one colour, filling the camera's frame wherever the camera looks: a background, or a wash over the shot",
        ports: &[],
        params: &[
            ParamSpec {
                id: "colour",
                title: "Colour",
                hint: "The card's colour, and how solid it is",
                kind: ParamKind::Colour { default: [1.0, 1.0, 1.0, 1.0] },
            },
            ParamSpec {
                id: DEPTH,
                title: "Depth",
                hint: "Where it stands among the layers: below 0 is farther from the viewer, behind what is at 0",
                kind: DEPTH_KIND,
            },
        ],
    },
    EffectKind {
        id: "cutter",
        title: "Cutter",
        category: "Combine",
        hint: "Cuts the picture away where the matte is; inverted, keeps only what is under the matte",
        ports: PICTURE_AND_MATTE,
        params: &[
            ParamSpec {
                id: "inverted",
                title: "Inverted",
                hint: "Keep what is under the matte, and cut the rest away",
                kind: ParamKind::Switch { default: false },
            },
            ParamSpec { id: "blur", title: "Alpha Blur", hint: "How soft the edge of the cut is", kind: pixels(0.0, 500.0) },
            ParamSpec {
                id: "grow",
                title: "Alpha Size",
                hint: "The matte made wider all round, or with a number below 0 narrower, before it cuts",
                kind: ParamKind::Number { default: 0.0, min: -200.0, max: 200.0, step: 0.5, unit: " px" },
            },
        ],
    },
    EffectKind {
        id: BLEND,
        title: "Blend",
        category: "Combine",
        hint: "Lays the picture onto what is under it by a mode: added to the light, screened, multiplied or taken away, rather than over it",
        ports: ONE_PICTURE,
        params: &[
            ParamSpec {
                id: "mode",
                title: "Mode",
                hint: "Normal lays it over; Add adds its light to what is under it; Screen brightens as Add does, never past white; \
                       Multiply darkens what is under it by its colour, and shows nothing over nothing; Subtract takes its light away",
                kind: ParamKind::Choice { options: &BLEND_MODES },
            },
            ParamSpec {
                id: "amount",
                title: "Amount",
                hint: "How much of it is laid on: at 0 none",
                kind: ParamKind::Number { default: 100.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
            },
        ],
    },
    EffectKind {
        id: "glow",
        title: "Glow",
        category: "Light and shade",
        hint: "A soft light round the picture, in a colour",
        ports: ONE_PICTURE,
        params: &[
            ParamSpec { id: "radius", title: "Radius", hint: "How far the light reaches out from the picture", kind: pixels(12.0, 500.0) },
            ParamSpec {
                id: "strength",
                title: "Strength",
                hint: "How bright the light is: 100 is as solid as the picture at its edge, more is brighter",
                kind: ParamKind::Number { default: 100.0, min: 0.0, max: 400.0, step: 1.0, unit: "%" },
            },
            ParamSpec { id: "colour", title: "Colour", hint: "The colour of the light", kind: ParamKind::Colour { default: [1.0, 0.95, 0.6, 1.0] } },
        ],
    },
    EffectKind {
        id: "dropShadow",
        title: "Drop Shadow",
        category: "Light and shade",
        hint: "The picture's shape, moved aside, softened and coloured, under the picture",
        ports: ONE_PICTURE,
        params: &[
            ParamSpec { id: "offsetX", title: "Offset Across", hint: "How far to the right the shadow falls", kind: OFFSET },
            ParamSpec { id: "offsetY", title: "Offset Down", hint: "How far down the shadow falls", kind: OFFSET },
            ParamSpec { id: "softness", title: "Softness", hint: "How soft the shadow's edge is", kind: pixels(6.0, 500.0) },
            ParamSpec { id: "colour", title: "Colour", hint: "The colour of the shadow", kind: ParamKind::Colour { default: [0.0, 0.0, 0.0, 1.0] } },
            ParamSpec {
                id: "opacity",
                title: "Opacity",
                hint: "How solid the shadow is: at 0 it is not seen",
                kind: ParamKind::Number { default: 50.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
            },
        ],
    },
    EffectKind {
        id: "tone",
        title: "Tone",
        category: "Light and shade",
        hint: "A shade inside the picture's shape: where the matte is, or without one along the picture's edge",
        ports: PICTURE_AND_MATTE,
        params: &SHADE_DARK,
    },
    EffectKind {
        id: "highlight",
        title: "Highlight",
        category: "Light and shade",
        hint: "A light inside the picture's shape: where the matte is, or without one along the picture's edge",
        ports: PICTURE_AND_MATTE,
        params: &SHADE_LIGHT,
    },
    EffectKind {
        id: "blur",
        title: "Blur",
        category: "Blur",
        hint: "Makes the picture soft all round",
        ports: ONE_PICTURE,
        params: &[ParamSpec { id: "radius", title: "Radius", hint: "How far the picture is spread, each way", kind: pixels(10.0, 500.0) }],
    },
    EffectKind {
        id: "directionalBlur",
        title: "Directional Blur",
        category: "Blur",
        hint: "Makes the picture soft along one direction: speed",
        ports: ONE_PICTURE,
        params: &[
            ParamSpec { id: "radius", title: "Radius", hint: "How far the picture is spread, each way along the direction", kind: pixels(20.0, 500.0) },
            ParamSpec {
                id: "angle",
                title: "Angle",
                hint: "The direction: 0 is level, and it turns anticlockwise",
                kind: ParamKind::Number { default: 0.0, min: -360.0, max: 360.0, step: 1.0, unit: "°" },
            },
        ],
    },
    EffectKind {
        id: "radialBlur",
        title: "Radial Blur",
        category: "Blur",
        hint: "Makes the picture soft round a centre, or out from it: a spin, a zoom",
        ports: ONE_PICTURE,
        params: &[
            ParamSpec { id: "kind", title: "Kind", hint: "Round the centre, or out from it", kind: ParamKind::Choice { options: &["Spin", "Zoom"] } },
            ParamSpec {
                id: "amount",
                title: "Amount",
                hint: "For a spin, the angle it is spread over in degrees; for a zoom, how far in percent of the way from the centre",
                kind: ParamKind::Number { default: 10.0, min: 0.0, max: 180.0, step: 0.5, unit: "" },
            },
            ParamSpec {
                id: "centreX",
                title: "Centre Across",
                hint: "Where the centre is, from the middle of the frame to the right",
                kind: ParamKind::Number { default: 0.0, min: -100_000.0, max: 100_000.0, step: 1.0, unit: " px" },
            },
            ParamSpec {
                id: "centreY",
                title: "Centre Down",
                hint: "Where the centre is, from the middle of the frame downwards",
                kind: ParamKind::Number { default: 0.0, min: -100_000.0, max: 100_000.0, step: 1.0, unit: " px" },
            },
        ],
    },
    EffectKind {
        id: "colourCorrection",
        title: "Colour Correction",
        category: "Colour",
        hint: "Changes the picture's colours: its darks and lights, its curves, its hue, how strong and how light its colours are, and a tint",
        ports: ONE_PICTURE,
        params: &[
            ParamSpec {
                id: "black",
                title: "Black",
                hint: "What is this light or darker becomes black",
                kind: ParamKind::Number { default: 0.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "white",
                title: "White",
                hint: "What is this light or lighter becomes white",
                kind: ParamKind::Number { default: 100.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "middle",
                title: "Middle",
                hint: "The tones between black and white: above 1 they are lighter, below it darker",
                kind: ParamKind::Number { default: 1.0, min: 0.1, max: 10.0, step: 0.01, unit: "" },
            },
            ParamSpec {
                id: "curves",
                title: "Curves",
                hint: "How light each tone comes out, after the levels: a curve for all three of red, green and blue, and one for each alone",
                kind: ParamKind::Curves,
            },
            ParamSpec {
                id: "hue",
                title: "Hue",
                hint: "Every colour turned round the colour wheel: at 120 red is green",
                kind: ParamKind::Number { default: 0.0, min: -180.0, max: 180.0, step: 1.0, unit: "°" },
            },
            ParamSpec {
                id: "saturation",
                title: "Saturation",
                hint: "How strong the colours are: at -100 the picture is grey",
                kind: ParamKind::Number { default: 0.0, min: -100.0, max: 100.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "lightness",
                title: "Lightness",
                hint: "Lighter above 0, up to white at 100; darker below it, down to black at -100",
                kind: ParamKind::Number { default: 0.0, min: -100.0, max: 100.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "tint",
                title: "Tint",
                hint: "A colour the picture is seen through, as through coloured glass: white changes nothing",
                kind: ParamKind::Colour { default: [1.0, 1.0, 1.0, 1.0] },
            },
            ParamSpec {
                id: "tintAmount",
                title: "Tint Amount",
                hint: "How much of the tint: at 0 none",
                kind: ParamKind::Number { default: 100.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
            },
        ],
    },
    EffectKind {
        id: "transparency",
        title: "Transparency",
        category: "Colour",
        hint: "Makes the picture more see-through, by an amount that can change over time: a fade",
        ports: ONE_PICTURE,
        params: &[ParamSpec {
            id: "amount",
            title: "Amount",
            hint: "How see-through: 0 leaves the picture as it is, 100 shows nothing of it",
            kind: ParamKind::Number { default: 50.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
        }],
    },
    EffectKind {
        id: STYLE,
        title: "Style",
        category: "Colour",
        hint: "Shows the picture in another palette style, as much as its amount says: a change of style over some frames, \
               or with a matte a part of it in another style, in shade where the rest is in daylight",
        ports: PICTURE_AND_MATTE,
        params: &[
            ParamSpec {
                id: STYLE_PALETTE,
                title: "Palette",
                hint: "The palette whose swatches are shown in the style; the others stay as the shot shows them. \
                       With none, every palette",
                kind: ParamKind::Text,
            },
            ParamSpec {
                id: STYLE,
                title: "Style",
                hint: "The style it is shown in, in the palette picked, or in every palette that has a style of this name; the others stay \
                       as the shot shows them. With none, in the palettes' own colours",
                kind: ParamKind::Text,
            },
            ParamSpec {
                id: "amount",
                title: "Amount",
                hint: "How much of the style is seen: at 0 none, the picture as the shot shows it; at 100 the style alone",
                kind: ParamKind::Number { default: 100.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
            },
        ],
    },
    EffectKind {
        id: PARTICLES,
        title: "Particles",
        category: "Particles",
        hint: "Rain or snow over the whole frame, falling as the frames go, or speed lines: the same on any frame however it is reached, and different for each seed",
        ports: &[
            Port { name: SPRITE, kind: PortKind::Picture, needed: false },
            Port { name: SPLASH_SPRITE, kind: PortKind::Picture, needed: false },
            Port { name: COLLIDER, kind: PortKind::Picture, needed: false },
        ],
        params: &[
            ParamSpec {
                id: "kind",
                title: "Kind",
                hint: "Rain: thin streaks falling fast. Snow: soft flakes drifting down. Wind: long faint streaks blown along the angle, fading in and out. Burst: sparks thrown out from the centre, falling and fading, and bouncing off a layer wired into its Collider. Speed Lines: lines thinning in from the frame's edge, drawn anew on every frame that is not held",
                kind: ParamKind::Choice { options: &["Rain", "Snow", "Wind", "Burst", "Speed Lines"] },
            },
            ParamSpec {
                id: "count",
                title: "Count",
                hint: "How many particles there are over the frame",
                kind: ParamKind::Number { default: 400.0, min: 0.0, max: 20_000.0, step: 10.0, unit: "" },
            },
            ParamSpec {
                id: "speed",
                title: "Speed",
                hint: "How fast they go: at 100% rain crosses the frame in some 18 frames, snow in some 150, a streak of wind in some 12, and a streaming speed line goes from the middle to the corner in some 12",
                kind: ParamKind::Number { default: 100.0, min: 0.0, max: 1000.0, step: 1.0, unit: "%" },
            },
            ParamSpec { id: "speedVariation", title: "Speed Variation", hint: "How much faster or slower each one is than the others", kind: PERCENT_30 },
            ParamSpec {
                id: "wind",
                title: "Wind",
                hint: "How far they are blown to the right each frame, or to the left below 0: rain slants with it",
                kind: ParamKind::Number { default: 0.0, min: -500.0, max: 500.0, step: 0.5, unit: " px" },
            },
            ParamSpec { id: "sway", title: "Sway", hint: "How far a flake of snow sways from side to side as it falls", kind: pixels(6.0, 500.0) },
            ParamSpec {
                id: "length",
                title: "Streak Length",
                hint: "How long a drop of rain is drawn: as far as it falls in this many frames. A streak of wind is ten times as long",
                kind: ParamKind::Number { default: 1.0, min: 0.0, max: 10.0, step: 0.1, unit: " frames" },
            },
            ParamSpec {
                id: "size",
                title: "Size",
                hint: "How wide a flake is, or a speed line at its thick end; a drop of rain is a third as wide",
                kind: pixels(4.0, 200.0),
            },
            ParamSpec { id: "sizeVariation", title: "Size Variation", hint: "How much larger or smaller each one is than the others", kind: PERCENT_30 },
            ParamSpec {
                id: "far",
                title: "Far Away",
                hint: "With no Depth Range: how much smaller, slower and fainter the farthest are than the nearest; 0 has them all alike",
                kind: ParamKind::Number { default: 60.0, min: 0.0, max: 95.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "planes",
                title: "Planes",
                hint: "Rain and snow: how many sheets they fall in, spread evenly from the Depth back across the Depth Range, each a picture of its own among the layers; a layer between two sheets has one in front of it and one behind",
                kind: ParamKind::Number { default: 1.0, min: 1.0, max: 4.0, step: 1.0, unit: "" },
            },
            ParamSpec {
                id: "depthRange",
                title: "Depth Range",
                hint: "Rain, snow and wind: how far behind the Depth the farthest are. Seen through the camera, the far ones are smaller and slower, and a camera move shows their parallax",
                kind: ParamKind::Number { default: 0.0, min: 0.0, max: 100_000.0, step: 1.0, unit: "" },
            },
            ParamSpec {
                id: "colour",
                title: "Colour",
                hint: "Their colour, and how solid the nearest are",
                kind: ParamKind::Colour { default: [0.85, 0.9, 1.0, 0.8] },
            },
            ParamSpec {
                id: "seed",
                title: "Seed",
                hint: "Another number gives other particles in other places, falling alike",
                kind: ParamKind::Number { default: 1.0, min: 0.0, max: 10_000.0, step: 1.0, unit: "" },
            },
            ParamSpec {
                id: "spriteScale",
                title: "Sprite Scale",
                hint: "Sprites only: how large, against their drawn size; a speed line is stretched along its length and as high as this says",
                kind: ParamKind::Number { default: 100.0, min: 1.0, max: 1000.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "tint",
                title: "Tint",
                hint: "Sprites only: how much they take of the Colour, as solid as each is; 0 keeps their own colours",
                kind: ParamKind::Number { default: 0.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "turn",
                title: "Turn",
                hint: "Sprites only: how far each is turned from upright, at random, up to a whole turn at 100%",
                kind: ParamKind::Number { default: 0.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "spin",
                title: "Spin",
                hint: "Sprites only: how far each turns every frame, some one way and some the other, as leaves tumble",
                kind: ParamKind::Number { default: 0.0, min: 0.0, max: 90.0, step: 0.5, unit: "°" },
            },
            ParamSpec {
                id: "emitter",
                title: "Emitter",
                hint: "Rain, snow and wind: where they come from. Whole Frame: they are everywhere on every frame. Area, Line or Point: they come out of it from the Start Frame on and go on into the scene",
                kind: ParamKind::Choice { options: &["Whole Frame", "Area", "Line", "Point"] },
            },
            ParamSpec {
                id: "emitterX",
                title: "Emitter Across",
                hint: "Where the emitter's middle is, right of the frame's middle, at the node's depth",
                kind: CENTRE,
            },
            ParamSpec {
                id: "emitterY",
                title: "Emitter Down",
                hint: "Where the emitter's middle is, below the frame's middle, at the node's depth: above the frame, it rains into it",
                kind: ParamKind::Number { default: -600.0, min: -5000.0, max: 5000.0, step: 1.0, unit: " px" },
            },
            ParamSpec {
                id: "emitterWidth",
                title: "Emitter Width",
                hint: "How wide the emitter's line or area is",
                kind: ParamKind::Number { default: 2000.0, min: 0.0, max: 20_000.0, step: 1.0, unit: " px" },
            },
            ParamSpec {
                id: "emitterHeight",
                title: "Emitter Height",
                hint: "How high the emitter's area is",
                kind: ParamKind::Number { default: 200.0, min: 0.0, max: 20_000.0, step: 1.0, unit: " px" },
            },
            ParamSpec {
                id: "preroll",
                title: "Pre-roll",
                hint: "Rain, snow and wind out of an emitter: how many frames they have already been coming when the Start Frame comes, so that it is not empty then",
                kind: ParamKind::Number { default: 0.0, min: 0.0, max: 100_000.0, step: 1.0, unit: " frames" },
            },
            ParamSpec {
                id: "start",
                title: "Start Frame",
                hint: "Burst, and rain, snow and wind out of an emitter: the frame of the shot they begin on; before it there are none",
                kind: ParamKind::Number { default: 0.0, min: 0.0, max: 100_000.0, step: 1.0, unit: "" },
            },
            ParamSpec {
                id: "over",
                title: "Let Out Over",
                hint: "Burst only: over how many frames its particles are thrown out, 1 for all at once; more makes a fountain",
                kind: ParamKind::Number { default: 1.0, min: 1.0, max: 100_000.0, step: 1.0, unit: " frames" },
            },
            ParamSpec {
                id: "repeat",
                title: "Repeat Every",
                hint: "Burst only: thrown again every this many frames, each time anew, as a fountain or sparks that keep coming; 0 throws it once",
                kind: ParamKind::Number { default: 0.0, min: 0.0, max: 100_000.0, step: 1.0, unit: " frames" },
            },
            ParamSpec {
                id: "spread",
                title: "Spread",
                hint: "Burst only: how wide round the Angle they are thrown, 360 all round",
                kind: ParamKind::Number { default: 360.0, min: 0.0, max: 360.0, step: 1.0, unit: "°" },
            },
            ParamSpec {
                id: "life",
                title: "Life",
                hint: "Burst only: about how many frames a particle lasts, fading out as it goes",
                kind: ParamKind::Number { default: 30.0, min: 1.0, max: 10_000.0, step: 1.0, unit: " frames" },
            },
            ParamSpec {
                id: "pull",
                title: "Gravity",
                hint: "Rain and snow: how much faster they fall than their Speed says, in pixels a frame; below 0 they rise. Rain's splashes fall back by it too",
                kind: ParamKind::Number { default: 0.0, min: -500.0, max: 500.0, step: 0.5, unit: " px" },
            },
            ParamSpec {
                id: "gravity",
                title: "Gravity",
                hint: "Burst only: how much faster its particles fall each frame, in pixels a frame; below 0 they rise",
                kind: ParamKind::Number { default: 0.5, min: -20.0, max: 20.0, step: 0.05, unit: " px" },
            },
            ParamSpec {
                id: "drag",
                title: "Drag",
                hint: "Burst only: how much of its speed a particle loses to the air each frame",
                kind: ParamKind::Number { default: 5.0, min: 0.0, max: 95.0, step: 0.5, unit: "%" },
            },
            ParamSpec {
                id: "bounce",
                title: "Bounce",
                hint: "Burst with a Collider only: how much of its speed a particle keeps as it bounces off where the collider is painted; 0 stops it there",
                kind: ParamKind::Number { default: 50.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "friction",
                title: "Friction",
                hint: "Burst with a Collider only: how much of its speed along the collider a particle loses each time it touches it, so that it slides or stops",
                kind: ParamKind::Number { default: 10.0, min: 0.0, max: 100.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "splash",
                title: "Splash",
                hint: "Rain only: drops end on the Ground, each throwing up a few droplets that fall back",
                kind: ParamKind::Switch { default: false },
            },
            ParamSpec {
                id: "splashScale",
                title: "Splash Size",
                hint: "Rain with Splash only: how large a splash is, against its drawn size where its drawings are wired in, or the droplets it throws",
                kind: ParamKind::Number { default: 100.0, min: 1.0, max: 1000.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "ground",
                title: "Ground",
                hint: "Rain with Splash only: how far below the frame's middle the drops land, at their depth",
                kind: ParamKind::Number { default: 300.0, min: -5000.0, max: 5000.0, step: 1.0, unit: " px" },
            },
            ParamSpec {
                id: "lines",
                title: "Speed Lines",
                hint: "Speed lines only. Radial: towards the centre, as a rush forward. Parallel: along the angle, as a move across",
                kind: ParamKind::Choice { options: &["Radial", "Parallel"] },
            },
            ParamSpec {
                id: "centreX",
                title: "Centre Across",
                hint: "Radial speed lines, and a burst: how far right of the frame's middle they point to, or it is thrown from",
                kind: CENTRE,
            },
            ParamSpec {
                id: "centreY",
                title: "Centre Down",
                hint: "Radial speed lines, and a burst: how far down from the frame's middle they point to, or it is thrown from",
                kind: CENTRE,
            },
            ParamSpec {
                id: "angle",
                title: "Angle",
                hint: "Wind, a burst and parallel speed lines: the way they go, 0 from left to right, 90 from the top down",
                kind: ParamKind::Number { default: 0.0, min: -360.0, max: 360.0, step: 1.0, unit: "°" },
            },
            ParamSpec {
                id: "flow",
                title: "Flow",
                hint: "Speed lines only. Boil: drawn anew on every frame that is not held, as drawn ones are. Stream: each goes on, moving at the Speed, out from the centre or along the angle",
                kind: ParamKind::Choice { options: &["Boil", "Stream"] },
            },
            ParamSpec {
                id: "clear",
                title: "Clear Middle",
                hint: "Speed lines only: how much is left clear of them, at the centre or across the frame",
                kind: ParamKind::Number { default: 40.0, min: 0.0, max: 95.0, step: 1.0, unit: "%" },
            },
            ParamSpec {
                id: "lengthVariation",
                title: "Length Variation",
                hint: "Speed lines only: how much longer or shorter each one is than the others",
                kind: PERCENT_30,
            },
            ParamSpec {
                id: "hold",
                title: "Hold",
                hint: "On twos or threes they move only every second or third frame, as drawings timed so do",
                kind: ParamKind::Choice { options: &["Every Frame", "On Twos", "On Threes"] },
            },
            ParamSpec {
                id: DEPTH,
                title: "Depth",
                hint: "Where it stands among the layers: below 0 is farther from the viewer, behind what is at 0",
                kind: DEPTH_KIND,
            },
        ],
    },
    EffectKind {
        id: PORT_IN,
        title: "Port In",
        category: "Structure",
        hint: "A way into a group: what is wired into it from outside is taken from it inside. Folded, the group's box has it as a port",
        ports: PASSING,
        params: &[],
    },
    EffectKind {
        id: PORT_OUT,
        title: "Port Out",
        category: "Structure",
        hint: "A way out of a group: what is wired into it inside is taken from it outside. Folded, the group's box has it as a port",
        ports: PASSING,
        params: &[],
    },
    EffectKind {
        id: SEND,
        title: "Send",
        category: "Structure",
        hint: "A named port: what is wired into it is read by every Receive that has the same tag, anywhere in the shot, with no wire between them",
        ports: PASSING,
        params: &[ParamSpec {
            id: TAG,
            title: "Tag",
            hint: "The name it sends under: a Receive with the same tag gives what is wired in here",
            kind: ParamKind::Text,
        }],
    },
    EffectKind {
        id: RECEIVE,
        title: "Receive",
        category: "Structure",
        hint: "Gives what is wired into the Send that has the same tag, with no wire from there to here",
        ports: &[],
        params: &[ParamSpec {
            id: TAG,
            title: "Tag",
            hint: "The name it receives under: it gives what is wired into the Send with the same tag",
            kind: ParamKind::Text,
        }],
    },
    EffectKind {
        id: VISIBLE,
        title: "Visible",
        category: "Structure",
        hint: "Shows what comes into it, or hides it: keyed, a part of a rig is there on some frames and not on others",
        ports: ONE_PICTURE,
        params: &[ParamSpec {
            id: "shown",
            title: "Shown",
            hint: "Off, nothing of what comes into the node is seen, here and in a render. It can be keyed, and stays as a keyframe has it until the next",
            kind: ParamKind::Switch { default: true },
        }],
    },
];

/// The kinds that are a group's ports (plan §8.6): what is wired into one from outside the
/// group is taken from it inside, and the other way round. A port is named by its node's
/// name, and a folded group's box shows the ports of what is in it.
pub const PORT_IN: &str = "portIn";
pub const PORT_OUT: &str = "portOut";
/// The kinds of the named port: what is wired into a Send is read by every Receive that has
/// the same tag, anywhere in the shot, with no wire between them.
pub const SEND: &str = "send";
pub const RECEIVE: &str = "receive";
/// The parameter of a Send and of a Receive that says which go together.
pub const TAG: &str = "tag";

/// The kind that shows what comes into it or hides it, and changes nothing of the picture: the
/// evaluator knows it, and no compositor does.
pub const VISIBLE: &str = "visible";

/// The kind that shows its picture in another palette style, where its matte is; and its
/// parameter that names the style.
pub const STYLE: &str = "style";

/// A Style node's parameter that names the palette it shows in its style: with none, every one.
pub const STYLE_PALETTE: &str = "palette";

/// Every kind of effect this build knows, in the order the menu lists them.
pub fn effect_kinds() -> &'static [EffectKind] {
    KINDS
}

pub fn effect_kind(id: &str) -> Option<&'static EffectKind> {
    KINDS.iter().find(|kind| kind.id == id)
}

/// What a parameter is set to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Value {
    Number(f64),
    Choice(String),
    Switch(bool),
    Colour([f64; 4]),
    Text(String),
    Curves(Curves),
}

/// Curves of tone (plan §8.4), as a paint program has them: one for all three of red, green
/// and blue, the master, and one for each of them alone, which comes after it. A curve says
/// how light each tone comes out, by points it goes through: each is a tone going in and the
/// tone it comes out as, both from 0, black, to 1, white. Between its points it bends
/// smoothly and never goes past them; before its first and after its last it is level. A
/// curve with no points is the straight line that changes nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Curves {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub master: Vec<[f64; 2]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub red: Vec<[f64; 2]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub green: Vec<[f64; 2]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blue: Vec<[f64; 2]>,
}

/// The straight line: the two points a curve that was never changed is shown with.
pub const STRAIGHT: [[f64; 2]; 2] = [[0.0, 0.0], [1.0, 1.0]];

impl Curves {
    /// The curves by their names, the master first: what a file and a command call them.
    pub const NAMES: [&'static str; 4] = ["master", "red", "green", "blue"];
    /// The most points one curve has.
    pub const MOST: usize = 16;

    /// The points of the curve at the place `at` of [`Curves::NAMES`].
    pub fn points(&self, at: usize) -> &[[f64; 2]] {
        [&self.master, &self.red, &self.green, &self.blue][at.min(3)]
    }

    pub fn points_mut(&mut self, at: usize) -> &mut Vec<[f64; 2]> {
        [&mut self.master, &mut self.red, &mut self.green, &mut self.blue].into_iter().nth(at.min(3)).expect("there are four")
    }

    /// Whether none of the four changes anything.
    pub fn is_straight(&self) -> bool {
        (0..4).all(|at| self.points(at).is_empty() || self.points(at) == STRAIGHT)
    }

    /// The curves as they are kept: each one's points in order from dark to light and held
    /// within 0 to 1, and a curve that is the straight line with no points at all. `Err` says
    /// what is wrong with them.
    pub fn checked(mut self) -> Result<Self, String> {
        for at in 0..4 {
            let points = self.points_mut(at);
            if points.iter().flatten().any(|part| !part.is_finite()) {
                return Err(format!("a point of the {} curve is no number", Self::NAMES[at]));
            }
            points.iter_mut().for_each(|point| *point = point.map(|part| part.clamp(0.0, 1.0)));
            points.sort_by(|a, b| a[0].total_cmp(&b[0]));
            if points.len() == 1 || points.len() > Self::MOST {
                return Err(format!("the {} curve has from 2 to {} points, or none", Self::NAMES[at], Self::MOST));
            }
            if points.windows(2).any(|pair| pair[1][0] - pair[0][0] < 1e-4) {
                return Err(format!("two points of the {} curve are on the same tone", Self::NAMES[at]));
            }
            if *points == STRAIGHT {
                points.clear();
            }
        }
        Ok(self)
    }

    /// What the tone `tone` comes out as through the curve at the place `at`.
    pub fn at(&self, at: usize, tone: f64) -> f64 {
        iskra_drawing::monotone_through(self.points(at), tone)
    }

    /// The curve at the place `at` as a table of `steps` tones from black to white, each as
    /// it comes out.
    pub fn table(&self, at: usize, steps: usize) -> Vec<f32> {
        let last = steps.max(2) - 1;
        (0..=last).map(|step| self.at(at, step as f64 / last as f64) as f32).collect()
    }
}

/// A parameter of a node: what it is on every frame, and for a number or a switch the keys
/// that change it over time. With keys, the value is what they say: a number goes from one
/// key to the next as they ease, and a switch stays as a key has it until the next (it is
/// keyed as 1 for on and 0 for off).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Param {
    pub value: Value,
    #[serde(default, skip_serializing_if = "Track::is_empty")]
    pub keys: Track<f64>,
}

impl Param {
    pub fn of(value: Value) -> Self {
        Self { value, keys: Track::default() }
    }

    /// The parameter on `frame`.
    pub fn at(&self, frame: u32) -> Value {
        match (&self.value, self.keys.at(frame, false)) {
            (Value::Number(_), Some(keyed)) => Value::Number(keyed),
            (Value::Switch(_), Some(first)) => {
                // Held: as the last key at the frame or before it has it, or else the first; in
                // a cycle, at the frame it repeats.
                let frame = self.keys.cycled(frame).unwrap_or(frame);
                let held = self.keys.keys().iter().rev().find(|key| key.frame <= frame).map_or(first, |key| key.value);
                Value::Switch(held >= 0.5)
            }
            (still, _) => still.clone(),
        }
    }
}

/// An effect node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Effect {
    pub id: EffectId,
    pub name: String,
    /// The id of its [`EffectKind`]. A kind this build does not know, of a file from a newer
    /// one, is kept as it is and lets its picture through unchanged.
    pub kind: String,
    /// What is wired into its ports, by their names. A port with nothing wired has no entry.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, Source>,
    /// Its parameters, by their ids. One that has no entry is as its kind starts it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Param>,
    /// Switched off, the node lets its picture through unchanged, in the window and in a
    /// render alike.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// The library template the node is a copy from, if it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// For a port or a Send that carries a transform and not a picture (plan §8.6): the peg
    /// whose transform comes into it. What is parented to the node, by the node's own id
    /// where a peg's would stand, is carried by that peg, as if it were parented to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<PegId>,
}

fn yes() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(on: &bool) -> bool {
    *on
}

// As for `Layer`: the numbers of a node's parameters are always finite.
impl Eq for Effect {}

impl Effect {
    /// A node of `kind` with nothing wired into it and every parameter as the kind starts it.
    pub fn new(id: EffectId, name: &str, kind: &EffectKind) -> Self {
        Self {
            id,
            name: name.to_string(),
            kind: kind.id.to_string(),
            inputs: BTreeMap::new(),
            params: BTreeMap::new(),
            enabled: true,
            template: None,
            parent: None,
        }
    }

    /// What the node is, if this build knows its kind.
    /// Whether the parameter `param` does anything on `frame`, as the node is set: a Particles
    /// node's own kind of particle has some of its parameters and not others, and some are only
    /// of use with another switched on or wired in. Properties and the curves list only these.
    /// Every parameter of every other kind does.
    pub fn applies(&self, param: &str, frame: u32) -> bool {
        if self.kind != PARTICLES {
            return true;
        }
        let kind = self.choice("kind", frame);
        let lines = kind == "Speed Lines";
        let wired = self.inputs.contains_key(SPRITE);
        let sprites = wired && !lines;
        match param {
            "kind" | "count" | "colour" | "seed" | "hold" | DEPTH | "size" | "sizeVariation" => true,
            "speed" => !lines || self.choice("flow", frame) == "Stream",
            "speedVariation" | "wind" | "depthRange" => !lines,
            "far" => !lines && self.number("depthRange", frame) == 0.0,
            "sway" => kind == "Snow",
            "planes" => matches!(kind.as_str(), "Rain" | "Snow"),
            "length" => matches!(kind.as_str(), "Rain" | "Wind" | "Burst") && !sprites,
            "angle" => matches!(kind.as_str(), "Wind" | "Burst") || (lines && self.choice("lines", frame) == "Parallel"),
            "centreX" | "centreY" => kind == "Burst" || (lines && self.choice("lines", frame) != "Parallel"),
            "spriteScale" | "tint" => wired,
            "turn" | "spin" => sprites,
            "over" | "repeat" | "spread" | "life" | "drag" => kind == "Burst",
            "emitter" => matches!(kind.as_str(), "Rain" | "Snow" | "Wind"),
            "start" => kind == "Burst" || (matches!(kind.as_str(), "Rain" | "Snow" | "Wind") && self.choice("emitter", frame) != "Whole Frame"),
            "preroll" => matches!(kind.as_str(), "Rain" | "Snow" | "Wind") && self.choice("emitter", frame) != "Whole Frame",
            "emitterX" | "emitterY" => matches!(kind.as_str(), "Rain" | "Snow" | "Wind") && self.choice("emitter", frame) != "Whole Frame",
            "emitterWidth" => matches!(kind.as_str(), "Rain" | "Snow" | "Wind") && matches!(self.choice("emitter", frame).as_str(), "Area" | "Line"),
            "emitterHeight" => matches!(kind.as_str(), "Rain" | "Snow" | "Wind") && self.choice("emitter", frame) == "Area",
            "gravity" => kind == "Burst",
            "bounce" | "friction" => kind == "Burst" && self.inputs.contains_key(COLLIDER),
            "pull" => matches!(kind.as_str(), "Rain" | "Snow"),
            "splash" => kind == "Rain",
            "ground" | "splashScale" => kind == "Rain" && self.switch("splash", frame),
            "lines" | "flow" | "clear" | "lengthVariation" => lines,
            _ => true,
        }
    }

    pub fn described(&self) -> Option<&'static EffectKind> {
        effect_kind(&self.kind)
    }

    /// What is wired into its picture port.
    pub fn picture(&self) -> Option<Source> {
        self.inputs.get(PICTURE).copied()
    }

    /// A parameter on `frame`: as it is set and keyed, or as the kind starts it. `None` for a
    /// parameter the kind has not got.
    pub fn value(&self, id: &str, frame: u32) -> Option<Value> {
        let spec = self.described()?.param(id)?;
        Some(self.params.get(id).map_or_else(|| spec.default(), |param| param.at(frame)))
    }

    /// A parameter that is a number, on `frame`; 0 for one that is none.
    pub fn number(&self, id: &str, frame: u32) -> f64 {
        match self.value(id, frame) {
            Some(Value::Number(number)) => number,
            _ => 0.0,
        }
    }

    /// A parameter that is a switch, on `frame`; off for one that is none.
    pub fn switch(&self, id: &str, frame: u32) -> bool {
        matches!(self.value(id, frame), Some(Value::Switch(true)))
    }

    /// A parameter that is a colour, on `frame`, as sRGB with how solid; clear for one that
    /// is none.
    pub fn colour(&self, id: &str, frame: u32) -> [f64; 4] {
        match self.value(id, frame) {
            Some(Value::Colour(colour)) => colour,
            _ => [0.0; 4],
        }
    }

    /// A parameter that is curves; straight ones for one that is none.
    pub fn curves(&self, id: &str) -> Curves {
        match self.value(id, 0) {
            Some(Value::Curves(curves)) => curves,
            _ => Curves::default(),
        }
    }

    /// A parameter that is a name; nothing for one that is none.
    pub fn text(&self, id: &str) -> String {
        match self.value(id, 0) {
            Some(Value::Text(text)) => text,
            _ => String::new(),
        }
    }

    /// The node only hands on what comes into it, as it came: a port of a group, or a Send.
    pub fn hands_on(&self) -> bool {
        [PORT_IN, PORT_OUT, SEND].contains(&self.kind.as_str())
    }

    /// A parameter that is a choice, on `frame`; nothing for one that is none.
    pub fn choice(&self, id: &str, frame: u32) -> String {
        match self.value(id, frame) {
            Some(Value::Choice(choice)) => choice,
            _ => String::new(),
        }
    }

    /// The parameter `id` to change, made as the kind starts it if the node has not got it
    /// yet. `None` for a parameter the kind has not got.
    pub fn param_mut(&mut self, id: &str) -> Option<&mut Param> {
        let spec = self.described()?.param(id)?;
        Some(self.params.entry(id.to_string()).or_insert_with(|| Param::of(spec.default())))
    }

    /// The frames on which a parameter has a key: any of them, or the one named.
    pub fn key_frames(&self, id: Option<&str>) -> BTreeSet<u32> {
        let wanted = self.params.iter().filter(|(name, _)| id.is_none_or(|id| id == name.as_str()));
        wanted.flat_map(|(_, param)| param.keys.keys().iter().map(|key| key.frame)).collect()
    }

    /// Whether any parameter moves on after `frame`, towards a later key of its own.
    pub fn moves_after(&self, frame: u32) -> bool {
        self.moves_after_of(frame, None)
    }

    /// [`Effect::moves_after`], of any parameter or of the one named.
    pub fn moves_after_of(&self, frame: u32, id: Option<&str>) -> bool {
        self.params.iter().filter(|(name, _)| id.is_none_or(|id| id == name.as_str())).any(|(_, param)| param.keys.moves_after(frame))
    }

    /// How the keys on `frame` ease, in and out: those of the first parameter that has a key
    /// there, or of the one named.
    pub fn interp(&self, frame: u32, id: Option<&str>) -> Option<(Interp, Interp)> {
        let wanted = self.params.iter().filter(|(name, _)| id.is_none_or(|id| id == name.as_str()));
        wanted.filter_map(|(_, param)| param.keys.key(frame)).map(|key| (key.into, key.out)).next()
    }

    /// The node with every key `by` frames later.
    pub fn shifted(&self, by: u32) -> Effect {
        let mut later = self.clone();
        for param in later.params.values_mut() {
            param.keys = param.keys.shifted(by);
        }
        later
    }
}

impl Shot {
    /// The Send that has a Receive's tag: the first, if several have it. One with no tag
    /// goes with none.
    pub fn sender(&self, receive: &Effect) -> Option<&Effect> {
        let tag = receive.text(TAG);
        if receive.kind != RECEIVE || tag.is_empty() {
            return None;
        }
        self.effects.iter().map(|effect| &**effect).find(|effect| effect.kind == SEND && effect.text(TAG) == tag)
    }

    /// Where another Send has a Send's tag too: the one that Receives of the tag read, the
    /// first of them, which may be `send` itself. `None` for a tag that is the one Send's.
    pub fn tag_shared(&self, send: &Effect) -> Option<&Effect> {
        let tag = send.text(TAG);
        if send.kind != SEND || tag.is_empty() {
            return None;
        }
        let mut alike = self.effects.iter().map(|effect| &**effect).filter(|effect| effect.kind == SEND && effect.text(TAG) == tag);
        let first = alike.next()?;
        alike.next().map(|_| first)
    }

    /// The tags that the shot's Sends send under, each once, in their order.
    pub fn tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = Vec::new();
        for tag in self.effects.iter().filter(|effect| effect.kind == SEND).map(|send| send.text(TAG)) {
            if !tag.is_empty() && !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        tags
    }

    /// What stands above `id` where `id` names no peg but a port, a Send or a Receive that
    /// something is parented to: the peg wired into the port, or for a Receive the Send it
    /// names. `None` for an id that names none of those, and for one with nothing above it.
    pub fn handed_down(&self, id: PegId) -> Option<PegId> {
        let node = self.effect(EffectId(id.0))?;
        if node.hands_on() { node.parent } else { self.sender(node).map(|send| PegId(send.id.0)) }
    }

    /// Whether `id` is a port, a Send or a Receive: a node that something can be parented to
    /// in a peg's place.
    pub fn hands_down(&self, id: PegId) -> bool {
        self.effect(EffectId(id.0)).is_some_and(|node| node.hands_on() || node.kind == RECEIVE)
    }

    /// Whether going up from `from`, by pegs and by the ports and named ports between them,
    /// comes to the node `target`: what `target` may then not be parented to.
    pub fn carried_from(&self, from: Option<PegId>, target: u64) -> bool {
        let mut at = from;
        for _ in 0..=self.pegs.len() + self.effects.len() {
            match at {
                Some(id) if id.0 == target => return true,
                Some(id) => at = self.peg(id).map_or_else(|| self.handed_down(id), |peg| peg.parent),
                None => return false,
            }
        }
        false
    }

    /// Take an effect node off the wire it is on. The wire is joined again: what went
    /// through the node goes straight to where the node went, and what was parented to a
    /// port is carried by what came into the port. The node stays, wired to nothing.
    pub fn take_off(&mut self, id: EffectId) {
        let this = crate::Source::Effect(id);
        let Some(node) = self.effect(id) else { return };
        let (through, handed) = (node.picture().filter(|through| *through != this), node.hands_on().then_some(node.parent));
        if let Some(above) = handed {
            self.reparent(PegId(id.0), above);
            self.rewire_ports(PegId(id.0), above);
        }
        if let Some(node) = self.effect_mut(id) {
            node.inputs.remove(PICTURE);
            node.parent = None;
        }
        self.rewire(this, through);
    }

    /// Everything that is parented to `from` is parented to `to` instead: pegs, layers, the
    /// camera, and the ports that a transform is wired into.
    pub fn reparent(&mut self, from: PegId, to: Option<PegId>) {
        for child in self.pegs.iter_mut().filter(|peg| peg.parent == Some(from)) {
            std::sync::Arc::make_mut(child).parent = to;
        }
        for child in self.layers.iter_mut().filter(|layer| layer.parent == Some(from)) {
            std::sync::Arc::make_mut(child).parent = to;
        }
        for child in self.effects.iter_mut().filter(|effect| effect.parent == Some(from)) {
            std::sync::Arc::make_mut(child).parent = to;
        }
        if self.camera.parent == Some(from) {
            self.camera.parent = to;
        }
    }

    pub fn effect(&self, id: EffectId) -> Option<&Effect> {
        self.effects.iter().find(|effect| effect.id == id).map(|effect| &**effect)
    }

    pub fn effect_mut(&mut self, id: EffectId) -> Option<&mut Effect> {
        self.effects.iter_mut().find(|effect| effect.id == id).map(std::sync::Arc::make_mut)
    }

    /// The effect that `source` is wired into, by any of its ports: the first, if several.
    pub fn taker(&self, source: Source) -> Option<&Effect> {
        self.effects.iter().map(|effect| &**effect).find(|effect| effect.inputs.values().any(|input| *input == source))
    }

    /// The end of the wire that leaves `source`: what the last of the effects on it gives, or
    /// `source` itself if none is on it. This is what a composite stacks when it stacks a
    /// layer that has effects.
    pub fn tip(&self, source: Source) -> Source {
        let mut at = source;
        // A wire has no more effects on it than the shot has; the limit keeps bad data from looping.
        for _ in 0..=self.effects.len() {
            match self.effects.iter().find(|effect| effect.picture() == Some(at)) {
                Some(effect) => at = Source::Effect(effect.id),
                None => break,
            }
        }
        at
    }

    /// The effects on the wire that leaves `source`, the first it goes through first.
    pub fn effects_on(&self, source: Source) -> Vec<&Effect> {
        let (mut on, mut at) = (Vec::new(), source);
        while on.len() <= self.effects.len() {
            let Some(effect) = self.effects.iter().map(|effect| &**effect).find(|effect| effect.picture() == Some(at)) else { break };
            on.push(effect);
            at = Source::Effect(effect.id);
        }
        on
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{LayerId, Project};

    #[test]
    fn a_particles_node_lists_what_its_kind_of_particle_uses() {
        let kind = effect_kind(PARTICLES).unwrap();
        let mut node = Effect::new(EffectId(1), "Rain", kind);
        let set = |node: &mut Effect, param: &str, value: Value| node.param_mut(param).unwrap().value = value;
        let listed = |node: &Effect| kind.params.iter().map(|spec| spec.id).filter(|id| node.applies(id, 0)).collect::<BTreeSet<_>>();
        // Rain: its streaks, but not snow's sway nor the lines'; the ground once it splashes.
        let rain = listed(&node);
        assert!(rain.contains("length") && rain.contains("far") && !rain.contains("sway") && !rain.contains("clear") && !rain.contains("ground"), "{rain:?}");
        set(&mut node, "splash", Value::Switch(true));
        assert!(listed(&node).contains("ground") && listed(&node).contains("pull") && !listed(&node).contains("gravity"));
        // Spread in depth, Far Away is of no use; with a sprite, the sprite's own are listed.
        set(&mut node, "depthRange", Value::Number(100.0));
        assert!(!listed(&node).contains("far"));
        assert!(!listed(&node).contains("spin"));
        node.inputs.insert(SPRITE.to_string(), crate::Source::Layer(LayerId(9)));
        assert!(listed(&node).contains("spin") && !listed(&node).contains("length"));
        // A burst bounces only with a collider.
        set(&mut node, "kind", Value::Choice("Burst".to_string()));
        assert!(!listed(&node).contains("bounce"));
        node.inputs.insert(COLLIDER.to_string(), crate::Source::Layer(LayerId(9)));
        assert!(listed(&node).contains("bounce") && listed(&node).contains("friction"));
        // Speed lines: radial has a centre, parallel an angle.
        set(&mut node, "kind", Value::Choice("Speed Lines".to_string()));
        let radial = listed(&node);
        assert!(radial.contains("centreX") && !radial.contains("angle") && !radial.contains("speed") && !radial.contains("spin"), "{radial:?}");
        set(&mut node, "lines", Value::Choice("Parallel".to_string()));
        assert!(listed(&node).contains("angle") && !listed(&node).contains("centreX"));
        assert!(!listed(&node).contains("speed"), "boiling lines have no speed");
        set(&mut node, "flow", Value::Choice("Stream".to_string()));
        assert!(listed(&node).contains("speed"));
        // Every parameter is listed for some kind set some way.
        let mut ever = BTreeSet::new();
        for particle in ["Rain", "Snow", "Wind", "Burst", "Speed Lines"] {
            for (lines, splash, sprite, range, emitter) in [("Radial", true, true, 0.0, "Area"), ("Parallel", false, false, 10.0, "Line")] {
                let mut node = Effect::new(EffectId(1), "One", kind);
                set(&mut node, "kind", Value::Choice(particle.to_string()));
                set(&mut node, "lines", Value::Choice(lines.to_string()));
                set(&mut node, "splash", Value::Switch(splash));
                set(&mut node, "depthRange", Value::Number(range));
                set(&mut node, "emitter", Value::Choice(emitter.to_string()));
                if sprite {
                    node.inputs.insert(SPRITE.to_string(), crate::Source::Layer(LayerId(9)));
                    node.inputs.insert(COLLIDER.to_string(), crate::Source::Layer(LayerId(9)));
                }
                ever.extend(listed(&node));
            }
        }
        let all: BTreeSet<_> = kind.params.iter().map(|spec| spec.id).collect();
        assert_eq!(ever, all);
        // Another kind lists all it has.
        let blur = Effect::new(EffectId(2), "Blur", effect_kind("blur").unwrap());
        assert!(blur.applies("radius", 0));
    }

    #[test]
    fn a_node_has_what_its_kind_says_and_its_numbers_can_be_keyed() {
        let kind = effect_kind("transparency").unwrap();
        assert_eq!((kind.category, kind.ports.len(), kind.param("amount").map(|param| param.title)), ("Colour", 1, Some("Amount")));
        // Every kind but a Receive and one that makes its own picture has the picture port,
        // and an id that no other has.
        for (at, kind) in effect_kinds().iter().enumerate() {
            assert!(
                (kind.port(PICTURE).is_some() || kind.id == RECEIVE || kind.makes_picture()) && effect_kinds()[..at].iter().all(|other| other.id != kind.id),
                "{}",
                kind.id
            );
        }
        // A name is said by a string, and kept without the blanks round it.
        let from = effect_kind(RECEIVE).unwrap().param(TAG).unwrap();
        assert_eq!(from.checked(Value::Choice(" Arm ".to_string())), Ok(Value::Text("Arm".to_string())));
        assert!(from.checked(Value::Number(1.0)).is_err() && from.default() == Value::Text(String::new()));
        let cutter = effect_kind("cutter").unwrap();
        assert!(cutter.port(MATTE).is_some_and(|port| port.kind == PortKind::Matte && !port.needed));
        assert!(!Effect::new(EffectId(1), "Cut", cutter).switch("inverted", 0));
        assert_eq!(Effect::new(EffectId(1), "Spin", effect_kind("radialBlur").unwrap()).choice("kind", 0), "Spin");
        let mut node = Effect::new(EffectId(7), "Fade", kind);
        // A parameter that was never set is as the kind starts it; one the kind has not got is none.
        assert_eq!((node.number("amount", 0), node.value("radius", 0), node.picture()), (50.0, None, None));
        assert!(node.param_mut("radius").is_none() && node.params.is_empty());
        node.param_mut("amount").unwrap().value = Value::Number(20.0);
        assert_eq!(node.number("amount", 9), 20.0);
        // With keys, the number is what they say on the frame.
        let keys = &mut node.param_mut("amount").unwrap().keys;
        keys.set(0, 0.0);
        keys.set(10, 100.0);
        assert_eq!((node.number("amount", 0), node.number("amount", 5), node.number("amount", 30)), (0.0, 50.0, 100.0));
        assert_eq!(node.key_frames(None).into_iter().collect::<Vec<_>>(), [0, 10]);
        assert_eq!(node.shifted(4).key_frames(Some("amount")).into_iter().collect::<Vec<_>>(), [4, 14]);
        // A switch that is keyed stays as a key has it until the next, and before the first
        // key is as that one.
        let mut shown = Effect::new(EffectId(8), "Shown", effect_kind(VISIBLE).unwrap());
        assert!(shown.switch("shown", 0));
        let keys = &mut shown.param_mut("shown").unwrap().keys;
        keys.set(4, 0.0);
        keys.set(8, 1.0);
        assert_eq!([0, 3, 4, 7, 8, 20].map(|frame| shown.switch("shown", frame)), [false, false, false, false, true, true]);
        assert!(node.key_frames(Some("radius")).is_empty());
        // What a parameter takes is checked against its kind.
        let amount = kind.param("amount").unwrap();
        assert_eq!(amount.checked(Value::Number(250.0)), Ok(Value::Number(100.0)), "held to its range");
        assert!(amount.checked(Value::Switch(true)).is_err() && amount.checked(Value::Number(f64::NAN)).is_err());
        // Written and read again, it is the same; what is as it started is not written.
        let written = serde_json::to_value(&node).unwrap();
        assert_eq!(serde_json::from_value::<Effect>(written.clone()).unwrap(), node);
        assert!(written.get("enabled").is_none() && written.get("inputs").is_none(), "{written}");
        // A kind this build does not know is kept, and has nothing to set.
        let strange: Effect =
            serde_json::from_value(serde_json::json!({ "id": 9, "name": "New", "kind": "hologram", "params": { "glint": { "value": { "number": 3.0 } } } }))
                .unwrap();
        assert!(strange.described().is_none() && strange.value("glint", 0).is_none() && strange.params.len() == 1);
    }

    #[test]
    fn the_end_of_a_wire_is_what_its_last_effect_gives() {
        let mut project = Project::default();
        let layer = Source::Layer(LayerId(project.new_id()));
        let kind = effect_kind("transparency").unwrap();
        let (fade, again) = (EffectId(project.new_id()), EffectId(project.new_id()));
        let shot = Arc::make_mut(&mut project.shots[0]);
        assert_eq!((shot.tip(layer), shot.taker(layer).map(|effect| effect.id)), (layer, None));
        let mut first = Effect::new(fade, "Fade", kind);
        first.inputs.insert(PICTURE.to_string(), layer);
        let mut second = Effect::new(again, "Fade Again", kind);
        second.inputs.insert(PICTURE.to_string(), Source::Effect(fade));
        shot.effects = vec![Arc::new(second), Arc::new(first)];
        assert_eq!(
            (shot.tip(layer), shot.tip(Source::Effect(fade)), shot.taker(layer).map(|effect| effect.id)),
            (Source::Effect(again), Source::Effect(again), Some(fade))
        );
        assert_eq!(shot.effects_on(layer).iter().map(|effect| effect.id).collect::<Vec<_>>(), [fade, again]);
        // Bad data: a ring of effects is walked once round.
        shot.effect_mut(fade).unwrap().inputs.insert(PICTURE.to_string(), Source::Effect(again));
        let _ = (shot.tip(Source::Effect(fade)), shot.effects_on(Source::Effect(fade)));
    }
}
