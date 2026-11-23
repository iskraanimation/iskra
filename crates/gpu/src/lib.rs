//! The GPU compositor (plan §3, §4.4): what a shot looks like on a frame, made on the graphics
//! card. It is the accelerator to `iskra-render`'s reference: it draws the same layers, in the
//! same order, sampled the same way, and its pictures are held against the reference's.
//!
//! A frame is drawn into a texture of 16-bit floats a channel, colour times alpha, in the
//! project's working space. That texture is the picture: it can be shown, kept, or read back.
//!
//! A drawing is kept on the card as it was painted (plan §4.8): each art layer a texture, of
//! swatch ids and coverage where it is indexed, and the palette another small texture that
//! the shader looks the colours up in. So a swatch that changes sends no drawing again, and a
//! drawing that was painted on sends only the tiles that are others than they were. A drawing
//! that a deformer bends is bent on the card from what is kept there, into a picture of its
//! own that is kept for as long as the drawing, the palette and the bend are the same. One too
//! large for one texture is kept in parts. A drawing with more art layers than the shader
//! reads, and one bent past what a texture holds, go flattened as the reference flattens them,
//! their art layers over each other with the palette's colours.
//!
//! What goes through an effect node (plan §8) is drawn into a picture of its own first. A blur
//! and a glow are made of it on the card, by the same passes the reference makes, and so is a
//! cutter that neither grows nor softens its matte, round what goes through it, and a Colour
//! Card, a Colour Correction, and a Tone, a Highlight and a Cutter that grows or softens its
//! matte, a Drop Shadow, and a Directional and a Radial Blur, within the widths the card
//! takes. A Particles
//! node's particles are worked out by the reference and laid on the card, each mark and sprite
//! as the reference lays it. Every other kind is made by the reference itself, on the
//! processor, and sent to the card as a picture.

use std::collections::HashMap;
use std::sync::Arc;

use iskra_drawing::{Affine, ArtLayerKind, Drawing, Point, Rect, TILE_SIZE, Tile, TileCoord};
use iskra_eval::Card;
use iskra_model::{LayerId, Palette, Projection, Shot, Warp, WorkingSpace};
use iskra_render::{Image, View};
use wgpu::util::DeviceExt;

/// How many pictures of effects made on the card are kept at most, and for how many frames
/// after they were last used: enough for a rig's while the viewport and the playback cache
/// take turns, and few while a drag makes new ones on every frame.
const EFFECTS_KEPT: usize = 128;
const EFFECTS_KEPT_FRAMES: u64 = 8;

/// The widest a plane is made wider or narrower on the card, in pixels each way: past it the
/// reference makes it, by a running greatest that costs the same whatever the width.
const WIDEST: u32 = 128;

/// The format of a frame: colour times alpha, in the working space.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The format of a frame made ready to be shown: sRGB colour times alpha, 8 bits a channel,
/// which is what a window's interface draws its pictures from.
pub const SHOWN: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The format of a picture that the reference made and the card is sent: its numbers as
/// they are.
const SENT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// The format of a drawing bent on the card: straight colour in sRGB, 8 bits a channel, as
/// the reference bends it; and of what keeps the triangles of its mesh in their order.
const BENT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const BENT_ORDER: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// How many readings to each side of a pixel a blur on the card takes at most
/// (`blur.wgsl`'s `taps`). The card reads more the wider the blur is, where the processor
/// keeps a running sum, so a very wide blur is the processor's.
const TAPS: usize = 256;

/// The blur the reference makes by three passes of a mean over 2r + 1 pixels, for the three
/// r of `passes`, as the one kernel they come to: how much of the pixel itself, and then of
/// the pixels to each side, the nearest first.
fn kernel(passes: [usize; 3]) -> Vec<f64> {
    let mut whole = vec![1.0f64];
    for r in passes {
        let (wide, share) = (2 * r + 1, 1.0 / (2 * r + 1) as f64);
        let mut next = vec![0.0; whole.len() + wide - 1];
        for (at, value) in whole.iter().enumerate() {
            next[at..at + wide].iter_mut().for_each(|sum| *sum += value * share);
        }
        whole = next;
    }
    // It is the same to both sides: from its middle on.
    whole.split_off(whole.len() / 2)
}

/// A kernel read two pixels at a time: how much of the pixel itself, and for each reading to
/// a side how far from the pixel it is taken and how much it counts. Between two pixels the
/// card gives their mix, in the proportion of where between them it reads.
fn readings(kernel: &[f64]) -> (f32, Vec<[f32; 2]>) {
    let pairs = kernel[1..].chunks(2).enumerate().map(|(pair, weights)| {
        let (near, far) = (weights[0], weights.get(1).copied().unwrap_or(0.0));
        let (first, both) = ((2 * pair + 1) as f64, near + far);
        [if both > 0.0 { first + far / both } else { first } as f32, both as f32]
    });
    (kernel[0] as f32, pairs.collect())
}

/// How much of a drawing that is larger than a texture can be is kept around each piece of it,
/// in pixels, so that what is read near a piece's edge is the neighbouring piece's.
const APRON: i32 = 16;

/// A drawing on the card: one texture for each piece of it. A drawing is in pieces only if it
/// is larger than a texture can be.
struct Uploaded {
    pieces: Vec<Piece>,
    /// What the pieces are of, in drawing pixels: what is painted, or what that is bent to.
    bounds: Rect,
    /// The count of the frame it was last drawn for.
    used: u64,
    /// Held so that their addresses, which are the key, can't be used again while this lives.
    _drawing: Arc<Drawing>,
    _palette: Arc<Palette>,
}

struct Piece {
    /// The part of the drawing this piece is drawn for, in drawing pixels.
    part: Rect,
    /// The drawing pixel that the texture's first texel is.
    origin: [i32; 2],
    view: wgpu::TextureView,
    bytes: usize,
}

/// A drawing on the card as it was painted: a texture for each of its art layers, all as
/// large as the tiles that anything is painted on. A drawing larger than a texture can be is
/// in parts, each with textures of its own.
struct Sheet {
    /// The tiles that anything is painted on, in drawing pixels.
    held: Rect,
    parts: Vec<Part>,
    /// The count of the frame it was last drawn for.
    used: u64,
    /// Held so that its address, which is the key, can't be used again while this lives.
    _drawing: Arc<Drawing>,
}

/// How a drawing is read: which of its kinds of art layer are not drawn, and whether it is an
/// onion skin's ghost, which is what it covers, whatever its swatches' alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Off {
    colour: bool,
    lines: bool,
    over: bool,
    ghost: bool,
}

impl Off {
    /// Whether an art layer of `kind` is drawn.
    fn shows(self, kind: ArtLayerKind) -> bool {
        match kind {
            ArtLayerKind::ColourArt => !self.colour,
            ArtLayerKind::LineArt => !self.lines,
            ArtLayerKind::Overart => !self.over,
        }
    }

    /// Whether any art layer is left out.
    fn any(self) -> bool {
        self.colour || self.lines || self.over
    }

    /// As a part of the key a drawing is kept on the card by: below 16.
    fn key(self) -> usize {
        usize::from(self.colour) + 2 * usize::from(self.lines) + 4 * usize::from(self.over) + 8 * usize::from(self.ghost)
    }
}

/// One art layer of a drawing on the card.
struct Art {
    /// Swatch ids and coverage, or free colour.
    indexed: bool,
    /// Which art layer it is.
    kind: ArtLayerKind,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// The tiles that were sent, held so that one is told from another by its address.
    tiles: HashMap<TileCoord, Arc<Tile>>,
}

/// One part of a drawing on the card as it was painted.
struct Part {
    /// What it is drawn for, in drawing pixels: whole tiles.
    part: Rect,
    /// What its textures hold: the part, with a tile more all round where the drawing goes on,
    /// so that what is read near its edge is the neighbouring part's.
    held: Rect,
    arts: Vec<Art>,
}

impl Sheet {
    fn bytes(&self) -> usize {
        self.parts.iter().map(|part| part.arts.len() * part.held.width() as usize * part.held.height() as usize * 4).sum()
    }
}

/// How many art layers a drawing kept as it was painted can have: `sheet.wgsl`'s places.
const PLACES: usize = 4;

/// How the window draws a drawing where that is not as the shot shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// How much of it is there: 1 all of it, less for a layer that is faded, as the others
    /// are on the light table and a board is under the animation.
    pub opacity: f32,
    /// A colour, sRGB, that it is drawn in alone wherever anything is painted: an onion
    /// skin's ghost.
    pub tint: Option<[f32; 3]>,
}

impl Look {
    pub const PLAIN: Look = Look { opacity: 1.0, tint: None };
}

/// A drawing that only the window shows: what a layer looked like on another frame, drawn
/// under the layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Ghost {
    /// The layer it is drawn under.
    pub under: LayerId,
    pub drawing: Arc<Drawing>,
    /// From the drawing's pixels to the frame, as it was placed on its own frame.
    pub to_frame: Projection,
    /// How a deformer bent it there.
    pub warp: Option<Warp>,
    pub look: Look,
    /// Only its line art.
    pub lines_only: bool,
}

/// What only the window shows of a shot (plan §4.2): layers faded, and the onion skin's
/// ghosts. A render has none of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Looks {
    /// The layers that are not drawn as they are, each with how it is.
    pub layers: HashMap<LayerId, Look>,
    pub ghosts: Vec<Ghost>,
}

/// A frame as the compositor made it.
pub struct Frame {
    pub texture: wgpu::Texture,
    pub width: u32,
    pub height: u32,
    /// The working space its colours are in.
    pub space: WorkingSpace,
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// What draws a drawing kept as it was painted, with a texture that holds nothing for
    /// the places a drawing has no art layer in.
    sheet: wgpu::RenderPipeline,
    sheet_layout: wgpu::BindGroupLayout,
    nothing: wgpu::TextureView,
    /// What bends a drawing kept as it was painted (`bend.wgsl`).
    bend: wgpu::RenderPipeline,
    /// What makes a frame ready to be shown.
    show: wgpu::RenderPipeline,
    /// A blur along a row or a column, and what puts the picture of an effect into the
    /// picture it is of.
    blur: wgpu::RenderPipeline,
    blur_layout: wgpu::BindGroupLayout,
    /// Reads a picture between its pixels.
    between: wgpu::Sampler,
    /// A Colour Correction of a picture.
    correct: wgpu::RenderPipeline,
    correct_layout: wgpu::BindGroupLayout,
    /// A plane made wider or narrower, along a row or a column; and a Directional or a
    /// Radial Blur: by the same layout.
    widen: wgpu::RenderPipeline,
    motion: wgpu::RenderPipeline,
    paste: wgpu::RenderPipeline,
    /// The same, laying what an effect made on by a Blend's mode where it is not *Normal*:
    /// in the order of [`iskra_model::BLEND_MODES`] after the first.
    blends: Vec<wgpu::RenderPipeline>,
    paste_layout: wgpu::BindGroupLayout,
    /// What lays a Particles node's marks and sprites (`particles.wgsl`).
    scatter: wgpu::RenderPipeline,
    scatter_layout: wgpu::BindGroupLayout,
    /// The reference, for the effects that are not made on the card, with what it made kept.
    reference: iskra_render::Cache,
    /// The longest side a texture can have on this card.
    longest: u32,
    drawings: HashMap<(usize, usize, usize, u64), Uploaded>,
    /// The drawings kept as they were painted, by their addresses; which of them each tile
    /// that was sent is in, by the tile's address, to find the drawing a new one was made
    /// from; and the palettes, by their addresses and the styles they are shown in.
    sheets: HashMap<usize, Sheet>,
    homes: HashMap<usize, usize>,
    palettes: HashMap<(usize, usize), (wgpu::TextureView, Arc<Palette>, u64)>,
    /// The palettes Style nodes showed their pictures in lately: of which palette, shown in
    /// which styles, in the style of which name, of which named palette alone. Kept, so that
    /// the colours on the card and the drawings sent in them are found again on the next frame.
    restyled: Vec<(Arc<Palette>, usize, (String, String), Arc<Palette>)>,
    /// Whether a drawing is painted with a gradient or a picture (plan §4.20) under a palette
    /// shown in a style, by their addresses and the style: such a drawing is sent flattened,
    /// as the reference paints it, and not as swatches. Held with both, and the frame it was
    /// last asked of.
    shaded: HashMap<(usize, usize, usize), (Arc<Drawing>, Arc<Palette>, bool, u64)>,
    /// How many tiles of drawings have been sent to the card: for tests and for tuning.
    pub sent: u64,
    /// The count of frames rendered so far.
    frames: u64,
    /// How many bytes of drawings may stay on the card between frames.
    pub budget: usize,
    /// How many effects were made on the card, and how many by the reference: for tests and
    /// for tuning.
    pub effects: (u64, u64),
    /// How many drawings were bent on the card, and how many by the reference: for tests and
    /// for tuning.
    pub bends: (u64, u64),
    /// The cutters and colour corrections made on the card lately: by what each is made of
    /// and how the window looked at it, its picture ready to put in, and the frame it was
    /// last used on.
    kept_effects: Vec<(iskra_render::Key, Option<Looks>, wgpu::BindGroup, u64)>,
    /// How many times one of those was found made: for tests and for tuning.
    pub effects_kept: u64,
}

/// What a frame is made of, beside the view.
struct Of<'a> {
    shot: &'a Shot,
    palette: &'a Arc<Palette>,
    /// The colours of the palette style the shot is shown in.
    colours: &'a Palette,
    /// The styles as one number, for telling drawings sent under others apart.
    style: usize,
    space: WorkingSpace,
    frame: u32,
    /// What only the window shows, if it is the window's picture.
    looks: Option<&'a Looks>,
}

/// How what an effect made is put into the picture, where it is not put in as it is.
enum Pasted {
    /// Over a light of this colour, as wide as its shape is spread, this strong.
    Glow([f32; 4], f32),
    /// Fainter: this much of it left.
    Faded(f32),
    /// A Style node's: from `made` to the picture in its style, this much of the way, and
    /// where a matte is wired by how solid it is.
    Styled(f32, Option<wgpu::TextureView>),
    /// A cutter's: cut by how solid its matte, the second picture, is; inverted, it keeps
    /// what is under the matte.
    Cut(bool),
    /// A Colour Card's: this colour times how solid, all over.
    Filled([f32; 4]),
    /// A Tone's or a Highlight's: this colour times how solid laid over the picture, as much
    /// as the second picture is solid times this much, where the picture is.
    Toned([f32; 4], f32),
}

/// One thing drawn into a picture: a piece of a drawing, a drawing kept as it was painted,
/// or what an effect made.
enum Step {
    Piece(wgpu::BindGroup),
    Sheet(wgpu::BindGroup),
    /// With the place of a Blend's mode among the modes, 0 for over.
    Paste(wgpu::BindGroup, usize),
}

/// What the shader is told about a piece: `layer.wgsl`'s `Piece`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Told {
    back: [[f32; 4]; 3],
    to_picture: [f32; 4],
    moves: [f32; 4],
    part: [f32; 4],
    origin: [f32; 2],
    picture: [f32; 2],
    points: f32,
    linear: f32,
    tilted: f32,
    nearest: f32,
    tint: [f32; 4],
    fade: [f32; 4],
    own: [f32; 4],
}

impl Told {
    fn bytes(&self) -> Vec<u8> {
        let [x, y, s] = &self.back;
        let numbers = [
            &x[..],
            y,
            s,
            &self.to_picture,
            &self.moves,
            &self.part,
            &self.origin,
            &self.picture,
            &[self.points, self.linear, self.tilted, self.nearest],
            &self.tint,
            &self.fade,
            &self.own,
        ];
        numbers.into_iter().flatten().flat_map(|number| number.to_le_bytes()).collect()
    }
}

impl Compositor {
    /// A compositor on a device that is already there: the window's, so that its frames can
    /// be shown without leaving the card.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("iskra layer"),
            source: wgpu::ShaderSource::Wgsl(include_str!("layer.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iskra layer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("iskra layer"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("iskra layer"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            // Both faces: a mirrored drawing is seen from behind.
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // A drawing kept as it was painted: the same shader, reading its art layers and the
        // palette where the other reads flattened colours.
        let source = include_str!("layer.wgsl");
        let (before, rest) = source.split_once("// <held>").expect("layer.wgsl marks what it reads from");
        let (_, after) = rest.split_once("// </held>").expect("layer.wgsl marks what it reads from");
        let sheet_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("iskra sheet"),
            source: wgpu::ShaderSource::Wgsl(format!("{before}{}{after}", include_str!("sheet.wgsl")).into()),
        });
        let whole_numbers = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let told = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let sheet_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iskra sheet"),
            entries: &[
                told(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                whole_numbers(1),
                whole_numbers(2),
                whole_numbers(3),
                whole_numbers(4),
                whole_numbers(5),
                told(6, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let sheet_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("iskra sheet"),
            bind_group_layouts: &[Some(&sheet_layout)],
            immediate_size: 0,
        });
        let sheet = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("iskra sheet"),
            layout: Some(&sheet_pipeline_layout),
            vertex: wgpu::VertexState { module: &sheet_shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &sheet_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // A drawing kept as it was painted, bent: its mesh drawn into a picture of its own, each
        // triangle in front of those after it.
        let bend_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("iskra bend"),
            source: wgpu::ShaderSource::Wgsl(format!("{}{}", include_str!("sheet.wgsl"), include_str!("bend.wgsl")).into()),
        });
        let bend = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("iskra bend"),
            layout: Some(&sheet_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &bend_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 5 * 4,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: BENT_ORDER,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &bend_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: BENT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let nothing = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("iskra nothing"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rg16Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        // A frame is shown through the same kind of bindings: what it is told, and a texture.
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("iskra show"),
            source: wgpu::ShaderSource::Wgsl(include_str!("show.wgsl").into()),
        });
        let show = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("iskra show"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: SHOWN, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // A pass of a blur is told and reads as a layer does; it writes the numbers as they are.
        let whole = |label: &str, source: &str, layout: &wgpu::PipelineLayout, target: wgpu::ColorTargetState| {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(target)],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // A blur reads its picture between the pixels, and writes the numbers as they are.
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iskra blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blurred = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("iskra blur"),
            bind_group_layouts: &[Some(&blur_layout)],
            immediate_size: 0,
        });
        let plain = wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL };
        let blur = whole("iskra blur", include_str!("blur.wgsl"), &blurred, plain.clone());
        let between = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("iskra between"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // What is pasted reads three pictures: what was made; for a glow its shape spread, for a
        // style the picture in the style; and a style's matte.
        let read = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let paste_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iskra paste"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                read(1),
                read(2),
                read(3),
            ],
        });
        let pasted = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("iskra paste"),
            bind_group_layouts: &[Some(&paste_layout)],
            immediate_size: 0,
        });
        let over = wgpu::ColorTargetState { format: FORMAT, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL };
        let paste = whole("iskra paste", include_str!("paste.wgsl"), &pasted, over);
        // A Colour Correction reads the picture and the curves' tables, and writes the numbers
        // as they are.
        let correct_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iskra correct"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                read(1),
                read(2),
            ],
        });
        let corrected = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("iskra correct"),
            bind_group_layouts: &[Some(&correct_layout)],
            immediate_size: 0,
        });
        let correct = whole("iskra correct", include_str!("correct.wgsl"), &corrected, plain.clone());
        // A plane made wider or narrower reads it, and for a rim the original, as a Colour
        // Correction reads its two.
        let widen = whole("iskra widen", include_str!("widen.wgsl"), &corrected, plain.clone());
        // A Directional or a Radial Blur reads the picture, by the same layout.
        let motion = whole("iskra motion", include_str!("motion.wgsl"), &corrected, plain);
        // A Blend's modes, as `iskra_render::laid` has them: the alpha always one over the other.
        let alpha =
            wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
        let colour = |src_factor, dst_factor, operation| wgpu::BlendComponent { src_factor, dst_factor, operation };
        let (one, add) = (wgpu::BlendFactor::One, wgpu::BlendOperation::Add);
        let modes = [
            colour(one, one, add),
            colour(one, wgpu::BlendFactor::OneMinusSrc, add),
            colour(wgpu::BlendFactor::Dst, wgpu::BlendFactor::OneMinusSrcAlpha, add),
            colour(one, one, wgpu::BlendOperation::ReverseSubtract),
        ];
        let blends = modes
            .into_iter()
            .map(|color| {
                let target = wgpu::ColorTargetState { format: FORMAT, blend: Some(wgpu::BlendState { color, alpha }), write_mask: wgpu::ColorWrites::ALL };
                whole("iskra blend", include_str!("paste.wgsl"), &pasted, target)
            })
            .collect();
        // A Particles node's particles: what it is told, the particles, and its sprites in one
        // picture.
        let scatter_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("iskra particles"),
            source: wgpu::ShaderSource::Wgsl(include_str!("particles.wgsl").into()),
        });
        let scatter_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iskra particles"),
            entries: &[
                told(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                read(2),
            ],
        });
        let scatter = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("iskra particles"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("iskra particles"),
                bind_group_layouts: &[Some(&scatter_layout)],
                immediate_size: 0,
            })),
            vertex: wgpu::VertexState { module: &scatter_shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &scatter_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let longest = device.limits().max_texture_dimension_2d;
        let reference = iskra_render::Cache::default();
        Self {
            device,
            queue,
            pipeline,
            layout,
            show,
            blur,
            blur_layout,
            correct,
            correct_layout,
            widen,
            motion,
            between,
            paste,
            blends,
            paste_layout,
            scatter,
            scatter_layout,
            reference,
            longest,
            drawings: HashMap::new(),
            sheet,
            sheet_layout,
            nothing,
            bend,
            sheets: HashMap::new(),
            homes: HashMap::new(),
            palettes: HashMap::new(),
            restyled: Vec::new(),
            shaded: HashMap::new(),
            sent: 0,
            frames: 0,
            budget: 512 << 20,
            effects: (0, 0),
            bends: (0, 0),
            kept_effects: Vec::new(),
            effects_kept: 0,
        }
    }

    /// A compositor on a device of its own, for rendering without a window: the command line,
    /// tests. `None` where there is no graphics card to use, nor a stand-in for one.
    pub fn headless() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let limits = adapter.limits();
        let required_limits = wgpu::Limits { max_texture_dimension_2d: limits.max_texture_dimension_2d.min(16384), ..wgpu::Limits::default() };
        let asked = wgpu::DeviceDescriptor { label: Some("iskra compositor"), required_limits, ..Default::default() };
        let (device, queue) = pollster::block_on(adapter.request_device(&asked)).ok()?;
        Some(Self::new(device, queue))
    }

    /// Take no texture to be longer than `side` a side, as on a lesser card: a drawing larger
    /// than that goes up in pieces. What is on the card already is dropped.
    pub fn limit_textures(&mut self, side: u32) {
        self.longest = side.clamp(2 * APRON as u32 + 1, self.device.limits().max_texture_dimension_2d);
        self.drawings.clear();
        self.sheets.clear();
        self.homes.clear();
    }

    /// How many bytes of drawings are on the card.
    pub fn kept(&self) -> usize {
        let flattened: usize = self.drawings.values().flat_map(|uploaded| &uploaded.pieces).map(|piece| piece.bytes).sum();
        flattened + self.sheets.values().map(Sheet::bytes).sum::<usize>()
    }

    /// The tiles that anything of `drawing` is painted on, as one rectangle; `None` for a
    /// drawing that can't be kept as it was painted: one with more art layers than the
    /// shader has places for, or larger than a texture can be where a texture is too small
    /// to hold a part of it.
    fn sheet_of(&self, drawing: &Drawing) -> Option<Rect> {
        let coords = || drawing.art_layers.iter().flat_map(|layer| layer.content.tiles().tiles().map(|(coord, _)| *coord));
        let (left, top) = (coords().map(|coord| coord.tx).min()?, coords().map(|coord| coord.ty).min()?);
        let (right, bottom) = (coords().map(|coord| coord.tx).max()? + 1, coords().map(|coord| coord.ty).max()? + 1);
        let held = Rect::new(left * TILE_SIZE, top * TILE_SIZE, right * TILE_SIZE, bottom * TILE_SIZE);
        (drawing.art_layers.len() <= PLACES && !self.parts_of(held).is_empty()).then_some(held)
    }

    /// How the tiles of `held` are kept in parts: what each is drawn for, and what its
    /// textures hold. One part where a texture holds them all; none where a texture can't
    /// hold even one tile with a tile all round it.
    fn parts_of(&self, held: Rect) -> Vec<(Rect, Rect)> {
        if held.width() <= self.longest && held.height() <= self.longest {
            return vec![(held, held)];
        }
        let step = (self.longest as i32 / TILE_SIZE - 2) * TILE_SIZE;
        if step < TILE_SIZE {
            return Vec::new();
        }
        let mut parts = Vec::new();
        for top in (held.y0..held.y1).step_by(step as usize) {
            for left in (held.x0..held.x1).step_by(step as usize) {
                let part = Rect::new(left, top, (left + step).min(held.x1), (top + step).min(held.y1));
                parts.push((part, part.inflate(TILE_SIZE).intersect(&held)));
            }
        }
        parts
    }

    /// Have `drawing` on the card as it was painted, under its address. If it is not there, a
    /// drawing it was made from may be, one painted on since: then only the tiles that are
    /// others are sent, into the textures that are there. `false` for a drawing that is not
    /// kept this way.
    fn keep(&mut self, drawing: &Arc<Drawing>) -> bool {
        let key = Arc::as_ptr(drawing) as usize;
        if let Some(sheet) = self.sheets.get_mut(&key) {
            sheet.used = self.frames;
            return true;
        }
        let Some(held) = self.sheet_of(drawing) else { return false };
        // The drawing most of its tiles were sent for, unless it is drawn on this very frame
        // or is of another shape.
        let mut shared: HashMap<usize, usize> = HashMap::new();
        for (_, tile) in drawing.art_layers.iter().flat_map(|layer| layer.content.tiles().tiles()) {
            if let Some(home) = self.homes.get(&(Arc::as_ptr(tile) as usize)) {
                *shared.entry(*home).or_default() += 1;
            }
        }
        let alike = |sheet: &Sheet| {
            let now = drawing.art_layers.iter().map(|layer| (!layer.content.is_rgba(), layer.kind));
            let kinds = sheet.parts.iter().all(|part| part.arts.iter().map(|art| (art.indexed, art.kind)).eq(now.clone()));
            sheet.held == held && sheet.used != self.frames && kinds
        };
        let from = shared.into_iter().filter(|(home, _)| self.sheets.get(home).is_some_and(alike)).max_by_key(|(_, count)| *count).map(|(home, _)| home);
        let mut sheet = match from.and_then(|home| self.sheets.remove(&home)) {
            Some(sheet) => sheet,
            None => {
                let device = &self.device;
                let arts = |held: Rect| {
                    drawing.art_layers.iter().map(move |layer| {
                        let indexed = !layer.content.is_rgba();
                        let texture = device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("iskra art layer"),
                            size: wgpu::Extent3d { width: held.width(), height: held.height(), depth_or_array_layers: 1 },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            // A swatch's id and its coverage, 16 bits each, as a tile holds them;
                            // or a colour, 8 bits a channel.
                            format: if indexed { wgpu::TextureFormat::Rg16Uint } else { wgpu::TextureFormat::Rgba8Uint },
                            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        });
                        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                        Art { indexed, kind: layer.kind, texture, view, tiles: HashMap::new() }
                    })
                };
                let parts = self.parts_of(held).into_iter().map(|(part, held)| Part { part, held, arts: arts(held).collect() }).collect();
                Sheet { held, parts, used: 0, _drawing: drawing.clone() }
            }
        };
        // Each tile that is another than was sent goes up; one that is gone is cleared.
        let side = TILE_SIZE as u32;
        let cleared = vec![0u8; (side * side * 4) as usize];
        for (held, art, layer) in
            sheet.parts.iter_mut().flat_map(|part| part.arts.iter_mut().zip(&drawing.art_layers).map(|(art, layer)| (part.held, art, layer)))
        {
            let mut now = HashMap::new();
            for coord in held.tiles() {
                let (tile, was) = (layer.content.tiles().tile(coord), art.tiles.get(&coord));
                if let Some(tile) = tile {
                    now.insert(coord, tile.clone());
                    self.homes.insert(Arc::as_ptr(tile) as usize, key);
                }
                let same = match (tile, was) {
                    (Some(tile), Some(was)) => Arc::ptr_eq(tile, was),
                    (None, None) => true,
                    _ => false,
                };
                if same {
                    continue;
                }
                self.sent += 1;
                let at = coord.rect();
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &art.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: (at.x0 - held.x0) as u32, y: (at.y0 - held.y0) as u32, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    tile.map_or(&cleared[..], |tile| tile.bytes()),
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(side * 4), rows_per_image: Some(side) },
                    wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
                );
            }
            art.tiles = now;
        }
        (sheet.used, sheet._drawing) = (self.frames, drawing.clone());
        self.sheets.insert(key, sheet);
        true
    }

    /// The palette's colours as the shot is shown in them, on the card: a texture of 256 by
    /// 256 colours, a swatch's at its id. A swatch that is gone is the reference's magenta.
    fn colours(&mut self, of: &Of) -> wgpu::TextureView {
        let key = (Arc::as_ptr(of.palette) as usize, of.style);
        if let Some((view, _, used)) = self.palettes.get_mut(&key) {
            *used = self.frames;
            return view.clone();
        }
        let mut colours = vec![of.colours.color(iskra_drawing::SwatchId(u16::MAX)); 1 << 16];
        for swatch in &of.colours.swatches {
            colours[usize::from(swatch.id.0)] = swatch.color;
        }
        // Their colours drifted, where a vector stroke's colour drifts.
        for (id, color) in of.colours.drifted_colors() {
            colours[usize::from(id.0)] = color;
        }
        // A swatch id that the palette never gave out is what the reference makes of it.
        colours[usize::from(u16::MAX)] = of.colours.color(iskra_drawing::SwatchId(u16::MAX));
        let described = wgpu::TextureDescriptor {
            label: Some("iskra palette"),
            size: wgpu::Extent3d { width: 256, height: 256, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        let texture = self.device.create_texture_with_data(&self.queue, &described, wgpu::util::TextureDataOrder::LayerMajor, colours.as_flattened());
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // A few palettes are kept: the ones drawn with longest ago go.
        if self.palettes.len() >= 16 {
            let frames = self.frames;
            self.palettes.retain(|_, (_, _, used)| *used + 2 > frames);
        }
        self.palettes.insert(key, (view.clone(), of.palette.clone(), self.frames));
        view
    }

    /// The colours `of` shows in, with those of every named palette that has a style called
    /// `name` in that style's, or of the one called `set` alone: what a Style node shows its
    /// picture in. The same one while it is asked for of the same palette and styles, kept for
    /// a few.
    fn restyled(&mut self, of: &Of, set: &str, name: &str) -> Arc<Palette> {
        let named = (set.to_string(), name.to_string());
        let found = self.restyled.iter().position(|(palette, style, of_named, _)| Arc::ptr_eq(palette, of.palette) && *style == of.style && *of_named == named);
        let kept = match found {
            Some(at) => self.restyled.remove(at),
            None => (of.palette.clone(), of.style, named, Arc::new(of.colours.restyled(set, name))),
        };
        let restyled = kept.3.clone();
        self.restyled.push(kept);
        if self.restyled.len() > 8 {
            self.restyled.remove(0);
        }
        restyled
    }

    /// Whether `drawing` has a swatch that `of`'s colours paint as a gradient or a picture.
    fn is_shaded(&mut self, of: &Of, drawing: &Arc<Drawing>) -> bool {
        if !of.colours.is_shaded() {
            return false;
        }
        let key = (Arc::as_ptr(drawing) as usize, Arc::as_ptr(of.palette) as usize, of.style);
        let frames = self.frames;
        let found = self.shaded.entry(key).or_insert_with(|| {
            let swatches = drawing.art_layers.iter().filter_map(|layer| layer.content.indexed()).flat_map(|surface| surface.swatches());
            let shaded = swatches.into_iter().any(|swatch| !of.colours.shade(swatch).is_solid());
            (drawing.clone(), of.palette.clone(), shaded, frames)
        });
        found.3 = frames;
        let shaded = found.2;
        if self.shaded.len() > 256 {
            self.shaded.retain(|_, (.., used)| *used + 2 > frames);
        }
        shaded
    }

    /// Put a drawing on the card, flattened under `palette`, in as many pieces as its size
    /// needs.
    fn upload(
        &self,
        drawing: &Arc<Drawing>,
        bounds: Rect,
        palette: &Arc<Palette>,
        colours: &Palette,
        space: WorkingSpace,
        bent: Option<&iskra_render::Bent>,
    ) -> Uploaded {
        let most = (self.longest as i32 - 2 * APRON).max(1);
        let mut pieces = Vec::new();
        for top in (bounds.y0..bounds.y1).step_by(most as usize) {
            for left in (bounds.x0..bounds.x1).step_by(most as usize) {
                let part = Rect::new(left, top, (left + most).min(bounds.x1), (top + most).min(bounds.y1));
                // With what is around it, as far as the drawing goes.
                let held = part.inflate(APRON).intersect(&bounds);
                let pixels = match bent {
                    // The part of the bent drawing, row by row.
                    Some(bent) => {
                        let (wide, left) = (bent.bounds.width() as usize, (held.x0 - bent.bounds.x0) as usize);
                        let rows = (held.y0 - bent.bounds.y0) as usize..(held.y1 - bent.bounds.y0) as usize;
                        rows.flat_map(|row| bent.pixels[row * wide + left..row * wide + left + held.width() as usize].iter().copied()).collect()
                    }
                    None => colours.paint(drawing, held, space),
                };
                let size = wgpu::Extent3d { width: held.width(), height: held.height(), depth_or_array_layers: 1 };
                let described = wgpu::TextureDescriptor {
                    label: Some("iskra drawing"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    // The numbers as they are, not taken for sRGB: the shader says what they mean.
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                };
                let texture = self.device.create_texture_with_data(&self.queue, &described, wgpu::util::TextureDataOrder::LayerMajor, pixels.as_flattened());
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                pieces.push(Piece { part, origin: [held.x0, held.y0], view, bytes: pixels.len() * 4 });
            }
        }
        Uploaded { pieces, bounds, used: self.frames, _drawing: drawing.clone(), _palette: palette.clone() }
    }

    /// A picture on the card to draw into, and to read from after.
    fn target(&self, label: &str, width: u32, height: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    /// Render what `shot` shows on `frame`, as `view` sees it, over transparency: what
    /// `iskra_render::render` makes, made on the card.
    pub fn render(&mut self, shot: &Shot, palette: &Arc<Palette>, space: WorkingSpace, frame: u32, view: &View) -> Frame {
        self.rendered(shot, palette, space, frame, view, None, None)
    }

    /// [`Compositor::render`], with what only the window shows: the layers that `looks`
    /// names drawn as it says, and its ghosts under their layers. What goes through an
    /// effect node that the reference makes is as the shot shows it.
    pub fn render_looked(&mut self, shot: &Shot, palette: &Arc<Palette>, space: WorkingSpace, frame: u32, view: &View, looks: &Looks) -> Frame {
        self.rendered(shot, palette, space, frame, view, None, Some(looks))
    }

    /// Render one card of what `shot` shows on `frame`, alone, as `view` sees it: what
    /// `iskra_render::card` makes, made on the card. It is how the window shows what goes
    /// through an effect node while the shot stands still.
    pub fn card(&mut self, shot: &Shot, palette: &Arc<Palette>, space: WorkingSpace, frame: u32, view: &View, card: &Card) -> Frame {
        self.rendered(shot, palette, space, frame, view, Some(card), None)
    }

    #[allow(clippy::too_many_arguments)]
    fn rendered(
        &mut self,
        shot: &Shot,
        palette: &Arc<Palette>,
        space: WorkingSpace,
        frame: u32,
        view: &View,
        card: Option<&Card>,
        looks: Option<&Looks>,
    ) -> Frame {
        self.frames += 1;
        let texture = self.target("iskra frame", view.width, view.height, FORMAT);
        let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // The colours of the palette style the shot is shown in.
        let colours = palette.styled(&shot.styles);
        // The styles as one number, for telling drawings sent under others apart.
        let style = shot.styles.iter().fold(0usize, |all, style| all.wrapping_mul(65537).wrapping_add(usize::from(style.0) + 1));
        let of = Of { shot, palette, colours: &colours, style, space, frame, looks };
        // All that the shot shows, or the one card.
        match card {
            Some(card) => self.draw(&of, view, std::slice::from_ref(card), &target),
            None => self.draw(&of, view, &iskra_eval::evaluate(shot, frame).cards, &target),
        }
        self.forget();
        Frame { texture, width: view.width.max(1), height: view.height.max(1), space }
    }

    /// Draw `cards` into `target`, which is as large as `view`, over transparency, in their
    /// order: a layer's drawing by its pieces, and what goes through an effect as the picture
    /// that was made of it.
    fn draw(&mut self, of: &Of, view: &View, cards: &[Card], target: &wgpu::TextureView) {
        let mut steps = Vec::new();
        for card in cards.iter().flat_map(Card::leaves) {
            match card {
                // A flat card is drawn as the cards in it, which are what `leaves` gave.
                Card::Flat { .. } => {}
                Card::Layer(placed) => {
                    // The onion skin's ghosts of the layer are under it.
                    for ghost in of.looks.iter().flat_map(|looks| &looks.ghosts).filter(|ghost| ghost.under == placed.layer) {
                        // Its vector art bent point by point, as the drawing's own is.
                        let (drawing, warp) = iskra_render::bent_points(&ghost.drawing, ghost.warp.clone());
                        let bounds = drawing.bounds();
                        if !bounds.is_empty() {
                            // Without the art layers its layer has switched off, as the layer itself.
                            let off = of.shot.layer(placed.layer).map_or(Vec::new(), |layer| placed.off(&layer.art_off)).into();
                            let seen = iskra_render::Seen { to_frame: ghost.to_frame, drawing, bounds, warp, off };
                            let seen = iskra_render::sharp(seen, &view.from_frame);
                            steps.extend(self.pieces(of, view, seen, ghost.look, ghost.lines_only));
                        }
                    }
                    // Vector art seen larger than its pixels is drawn at the size it is seen.
                    if let Some(seen) = iskra_render::seen_of(of.shot, of.frame, placed).map(|seen| iskra_render::sharp(seen, &view.from_frame)) {
                        let look = of.looks.and_then(|looks| looks.layers.get(&placed.layer)).copied().unwrap_or(Look::PLAIN);
                        steps.extend(self.pieces(of, view, seen, look, false));
                    }
                }
                Card::Through { effect, .. } => {
                    // Laid on by a Blend's mode: its place among the modes, 0 for over.
                    let mode = iskra_render::blend_mode(of.shot, *effect, of.frame);
                    let mode = iskra_model::BLEND_MODES.iter().position(|one| *one == mode).unwrap_or(0);
                    steps.extend(self.through(of, view, card).map(|made| Step::Paste(made, mode)));
                }
            }
        }
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("iskra frame") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("iskra layers"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            for step in &steps {
                match step {
                    Step::Piece(piece) => {
                        pass.set_pipeline(&self.pipeline);
                        pass.set_bind_group(0, piece, &[]);
                        pass.draw(0..6, 0..1);
                    }
                    Step::Sheet(sheet) => {
                        pass.set_pipeline(&self.sheet);
                        pass.set_bind_group(0, sheet, &[]);
                        pass.draw(0..6, 0..1);
                    }
                    Step::Paste(made, mode) => {
                        pass.set_pipeline(mode.checked_sub(1).and_then(|at| self.blends.get(at)).unwrap_or(&self.paste));
                        pass.set_bind_group(0, made, &[]);
                        pass.draw(0..3, 0..1);
                    }
                }
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    /// The part of `view` that `cards` can paint, as its corners in pixels: round every
    /// drawing in them where it is placed. `None` where that is not known so: a drawing bent
    /// through a mesh, an effect in them that may paint past what goes through it, or ghosts
    /// of the onion skin, which are drawn elsewhere.
    fn painted_part(of: &Of, view: &View, cards: &[Card]) -> Option<[u32; 4]> {
        if of.looks.is_some_and(|looks| !looks.ghosts.is_empty()) {
            return None;
        }
        // The effects in them, however deep.
        fn effects_in(cards: &[Card], into: &mut Vec<iskra_model::EffectId>) {
            for card in cards {
                match card {
                    Card::Through { effect, inside, mattes, .. } => {
                        into.push(*effect);
                        effects_in(inside, into);
                        mattes.iter().for_each(|(_, matte)| effects_in(matte, into));
                    }
                    Card::Flat { inside, .. } => effects_in(inside, into),
                    Card::Layer(_) => {}
                }
            }
        }
        let mut effects = Vec::new();
        effects_in(cards, &mut effects);
        if effects.into_iter().any(|effect| of.shot.effect(effect).is_none_or(|effect| !matches!(effect.kind.as_str(), "cutter" | "transparency"))) {
            return None;
        }
        let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for placed in cards.iter().flat_map(Card::placed) {
            let Some(seen) = iskra_render::seen_of(of.shot, of.frame, &placed) else { continue };
            if seen.warp.is_some() {
                return None;
            }
            let projected = seen.to_frame.then(&view.from_frame);
            let Rect { x0, y0, x1, y1 } = seen.bounds;
            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                let at = projected.apply(Point::new(f64::from(x), f64::from(y)))?;
                (low, high) = ([low[0].min(at.x), low[1].min(at.y)], [high[0].max(at.x), high[1].max(at.y)]);
            }
        }
        // A pixel's worth round it, for what sampling reaches; within the view.
        let side = |at: f64, most: u32| (at.max(0.0) as u32).min(most);
        let [x0, y0] = [side((low[0] - 2.0).floor(), view.width), side((low[1] - 2.0).floor(), view.height)];
        let [x1, y1] = [side((high[0] + 2.0).ceil(), view.width), side((high[1] + 2.0).ceil(), view.height)];
        Some(if x0 < x1 && y0 < y1 { [x0, y0, x1, y1] } else { [0; 4] })
    }

    /// How much there is of what goes through an effect, where the window fades all of it
    /// alike and none of it is a ghost's tint: `None` where it is all there, or not alike.
    fn faded_alike(of: &Of, card: &Card) -> Option<f32> {
        let looks = of.looks?;
        let mut seen = card.placed().into_iter().map(|placed| looks.layers.get(&placed.layer).copied().unwrap_or(Look::PLAIN));
        let first = seen.next()?;
        (first.tint.is_none() && first.opacity < 1.0 && seen.all(|look| look == first)).then_some(first.opacity)
    }

    /// A Particles node's `particles` laid onto a picture as large as `view`, as the reference
    /// lays them. `None` where its sprites can't be held in one picture on this card, for the
    /// reference to lay them.
    fn scattered(&self, particles: &iskra_render::Particles, view: &View) -> Option<wgpu::TextureView> {
        // The sprites side by side in rows, each row as tall as its tallest.
        let (mut places, mut row, mut across, mut wide, mut tall) = (Vec::new(), 0u32, 0u32, 1u32, 1u32);
        for picture in &particles.pictures {
            let [x0, y0, x1, y1] = picture.bounds;
            let (width, height) = ((x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32);
            if across + width > self.longest {
                (across, row) = (0, tall);
            }
            places.push([across, row]);
            across += width;
            wide = wide.max(across);
            tall = tall.max(row + height);
        }
        if wide > self.longest || tall > self.longest {
            return None;
        }
        let mut atlas = vec![[0.0f32; 4]; wide as usize * tall as usize];
        for (picture, [left, top]) in particles.pictures.iter().zip(&places) {
            let width = (picture.bounds[2] - picture.bounds[0]).max(0) as usize;
            for (y, line) in picture.pixels.chunks(width.max(1)).enumerate() {
                let at = (*top as usize + y) * wide as usize + *left as usize;
                atlas[at..at + line.len()].copy_from_slice(line);
            }
        }
        let described = wgpu::TextureDescriptor {
            label: Some("iskra sprites"),
            size: wgpu::Extent3d { width: wide, height: tall, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SENT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        let bytes: Vec<u8> = atlas.iter().flatten().flat_map(|number| number.to_le_bytes()).collect();
        let atlas = self.device.create_texture_with_data(&self.queue, &described, wgpu::util::TextureDataOrder::LayerMajor, &bytes);
        // Each particle as `particles.wgsl`'s `Item`: a quad round it, the pixels it may touch,
        // and what it is.
        let (width, height) = (f64::from(view.width), f64::from(view.height));
        let clip = |low: Point, high: Point| [low.x.floor().max(0.0), low.y.floor().max(0.0), high.x.ceil().min(width), high.y.ceil().min(height)];
        let mut items: Vec<f32> = Vec::with_capacity(particles.drawn.len() * 36);
        for particle in &particles.drawn {
            let mut item = [0.0f64; 36];
            let (corners, kept) = match particle {
                iskra_render::Particle::Mark(mark) => {
                    let reach = mark.reach();
                    let (along_x, along_y) = (mark.to.x - mark.from.x, mark.to.y - mark.from.y);
                    let long = along_x.hypot(along_y);
                    let (dx, dy) = if long > 0.0 { (along_x / long, along_y / long) } else { (1.0, 0.0) };
                    // A pixel more than it reaches, all round.
                    let (ex, ey, ax, ay) = (dx * (reach + 1.0), dy * (reach + 1.0), -dy * (reach + 1.0), dx * (reach + 1.0));
                    let (from, to) = (mark.from, mark.to);
                    let corners = [
                        Point::new(from.x - ex - ax, from.y - ey - ay),
                        Point::new(to.x + ex - ax, to.y + ey - ay),
                        Point::new(to.x + ex + ax, to.y + ey + ay),
                        Point::new(from.x - ex + ax, from.y - ey + ay),
                    ];
                    let low = Point::new(from.x.min(to.x) - reach, from.y.min(to.y) - reach);
                    let high = Point::new(from.x.max(to.x) + reach, from.y.max(to.y) + reach);
                    item[12..16].copy_from_slice(&[0.0, f64::from(mark.alpha), 0.0, 0.0]);
                    item[16..24].copy_from_slice(&[from.x, from.y, to.x, to.y, mark.radius, mark.taper, mark.soft, 0.0]);
                    (corners, clip(low, high))
                }
                iskra_render::Particle::Sprite { picture, to_picture, alpha } => {
                    let Some(to_sprite) = to_picture.inverse() else { continue };
                    let [x0, y0, x1, y1] = particles.pictures[*picture].bounds.map(f64::from);
                    let placed = |x: f64, y: f64| to_picture.apply(Point::new(x, y));
                    // Where it is drawn, as the reference finds it: the box round its corners.
                    let ends = [placed(x0, y0), placed(x1, y0), placed(x1, y1), placed(x0, y1)];
                    let low = Point::new(ends.iter().map(|at| at.x).fold(f64::INFINITY, f64::min), ends.iter().map(|at| at.y).fold(f64::INFINITY, f64::min));
                    let high =
                        Point::new(ends.iter().map(|at| at.x).fold(f64::NEG_INFINITY, f64::max), ends.iter().map(|at| at.y).fold(f64::NEG_INFINITY, f64::max));
                    // Its edge pixels are read between them and nothing, half a pixel past it.
                    let corners = [placed(x0 - 1.0, y0 - 1.0), placed(x1 + 1.0, y0 - 1.0), placed(x1 + 1.0, y1 + 1.0), placed(x0 - 1.0, y1 + 1.0)];
                    let [left, top] = places[*picture].map(f64::from);
                    item[12..16].copy_from_slice(&[1.0, f64::from(*alpha), 0.0, 0.0]);
                    item[24..36].copy_from_slice(&[
                        to_sprite.m[0],
                        to_sprite.m[1],
                        to_sprite.m[2],
                        to_sprite.m[3],
                        to_sprite.m[4],
                        to_sprite.m[5],
                        x0,
                        y0,
                        left,
                        top,
                        x1 - x0,
                        y1 - y0,
                    ]);
                    (corners, clip(low, high))
                }
            };
            if kept[0] >= kept[2] || kept[1] >= kept[3] {
                continue;
            }
            for (at, corner) in corners.iter().enumerate() {
                item[2 * at] = corner.x;
                item[2 * at + 1] = corner.y;
            }
            item[8..12].copy_from_slice(&kept);
            items.extend(item.map(|number| number as f32));
        }
        let count = (items.len() / 36) as u32;
        let [red, green, blue, _] = particles.tint.0;
        let told = [width as f32, height as f32, 0.0, 0.0]
            .into_iter()
            .chain(particles.colour)
            .chain([red, green, blue, particles.tint.1])
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<u8>>();
        let told = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("iskra particles"),
            contents: &told,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // A storage buffer may not be empty.
        items.resize(items.len().max(36), 0.0);
        let items = items.into_iter().flat_map(f32::to_le_bytes).collect::<Vec<u8>>();
        let items = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("iskra particles"),
            contents: &items,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let atlas = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("iskra particles"),
            layout: &self.scatter_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: told.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: items.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&atlas) },
            ],
        });
        let laid = self.target("iskra particles", view.width, view.height, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("iskra particles") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("iskra particles"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &laid,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.scatter);
            pass.set_bind_group(0, &bindings, &[]);
            pass.draw(0..6, 0..count);
        }
        self.queue.submit([encoder.finish()]);
        Some(laid)
    }

    /// What goes through an effect, made into a picture and ready to be put into `view`'s.
    /// `None` for a card that is no effect's.
    fn through(&mut self, of: &Of, view: &View, card: &Card) -> Option<wgpu::BindGroup> {
        let Card::Through { effect, inside, mattes, .. } = card else { return None };
        let effect = of.shot.effect(*effect)?;
        let (scale, frame) = (view.scale(), of.frame);
        // What is made less solid is drawn as it is and put in fainter; what a Blend lays on
        // is put in as much as its amount says, by its mode.
        if effect.kind == "transparency" || effect.kind == iskra_model::BLEND {
            self.effects.0 += 1;
            let inner = self.target("iskra through", view.width, view.height, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
            self.draw(of, view, inside, &inner);
            let amount = (effect.number("amount", frame) / 100.0).clamp(0.0, 1.0) as f32;
            let kept = if effect.kind == iskra_model::BLEND { amount } else { 1.0 - amount };
            return Some(self.pasted(&inner, &inner, [0.0, 0.0], Some(Pasted::Faded(kept))));
        }
        // What goes through a Style node is drawn twice, in the shot's colours and in the
        // style's, and the two are mixed as it says.
        if effect.kind == iskra_model::STYLE {
            self.effects.0 += 1;
            let picture =
                |compositor: &Self| compositor.target("iskra through", view.width, view.height, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
            let shown = picture(self);
            self.draw(of, view, inside, &shown);
            let restyled = self.restyled(of, &effect.text(iskra_model::STYLE_PALETTE), &effect.text(iskra_model::STYLE));
            let in_style = Of { palette: &restyled, colours: &restyled, ..*of };
            let styled = picture(self);
            // And in the skin of its name.
            let skinned = iskra_render::in_style_skin(of.shot, of.colours, effect, inside);
            self.draw(&in_style, view, &skinned, &styled);
            let matte = mattes.iter().find(|(port, _)| port == iskra_model::MATTE).map(|(_, cards)| {
                let matte = picture(self);
                self.draw(of, view, cards, &matte);
                matte
            });
            let share = (effect.number("amount", frame) / 100.0).clamp(0.0, 1.0) as f32;
            return Some(self.pasted(&shown, &styled, [0.0, 0.0], Some(Pasted::Styled(share, matte))));
        }
        // A Colour Card is its colour all over.
        if effect.kind == iskra_model::COLOUR_CARD {
            self.effects.0 += 1;
            let colour = iskra_render::paint(effect.colour("colour", frame), of.space);
            // Its picture is told, not read: one pixel stands for it.
            let none = self.target("iskra colour card", 1, 1, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
            return Some(self.pasted(&none, &none, [0.0, 0.0], Some(Pasted::Filled(colour))));
        }
        // A cutter that neither grows nor softens its matte cuts by it as it is, on the card:
        // the matte drawn as the reference draws it, with nothing of how the window looks.
        // It is worked out where what goes through can paint, and only there.
        if effect.kind == "cutter" && effect.number("grow", frame) == 0.0 && effect.number("blur", frame) == 0.0 {
            let (key, looks) = (self.reference.key(of.shot, of.palette, of.space, frame, view, card), of.looks.cloned());
            if let Some(kept) = self.kept_effect(key.as_ref(), &looks) {
                return Some(kept);
            }
            self.effects.0 += 1;
            let [x0, y0, x1, y1] = Self::painted_part(of, view, inside).unwrap_or([0, 0, view.width, view.height]);
            if x0 >= x1 || y0 >= y1 {
                return None;
            }
            let part = View {
                width: x1 - x0,
                height: y1 - y0,
                from_frame: Affine::translate(-f64::from(x0), -f64::from(y0)).mul(&view.from_frame),
                frame: view.frame,
            };
            let picture =
                |compositor: &Self| compositor.target("iskra through", part.width, part.height, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
            let (inner, at) = (picture(self), [-(x0 as f32), -(y0 as f32)]);
            self.draw(of, &part, inside, &inner);
            let made = match mattes.iter().find(|(port, _)| port == iskra_model::MATTE) {
                Some((_, cards)) => {
                    let matte = picture(self);
                    self.draw(&Of { looks: None, ..*of }, &part, cards, &matte);
                    self.pasted(&inner, &matte, at, Some(Pasted::Cut(effect.switch("inverted", frame))))
                }
                // With no matte wired there is nothing to cut by.
                None => self.pasted(&inner, &inner, at, None),
            };
            self.keep_effect(key, looks, &made);
            return Some(made);
        }
        // A Colour Correction changes each pixel alone, so it is worked out round what goes
        // through it.
        if effect.kind == "colourCorrection" {
            let (key, looks) = (self.reference.key(of.shot, of.palette, of.space, frame, view, card), of.looks.cloned());
            if let Some(kept) = self.kept_effect(key.as_ref(), &looks) {
                return Some(kept);
            }
            self.effects.0 += 1;
            let [x0, y0, x1, y1] = Self::painted_part(of, view, inside).unwrap_or([0, 0, view.width, view.height]);
            if x0 >= x1 || y0 >= y1 {
                return None;
            }
            let part = View {
                width: x1 - x0,
                height: y1 - y0,
                from_frame: Affine::translate(-f64::from(x0), -f64::from(y0)).mul(&view.from_frame),
                frame: view.frame,
            };
            let picture =
                |compositor: &Self| compositor.target("iskra through", part.width, part.height, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
            let inner = picture(self);
            self.draw(of, &part, inside, &inner);
            let corrected = picture(self);
            self.corrected(effect, frame, of.space, &inner, &corrected);
            let made = self.pasted(&corrected, &corrected, [-(x0 as f32), -(y0 as f32)], None);
            self.keep_effect(key, looks, &made);
            return Some(made);
        }
        // A Tone, a Highlight, and a Cutter that grows or softens its matte: a plane of how
        // much, made wider or narrower and softened, by which the picture is laid over or cut.
        // Worked out round what goes through, with the room the effect reaches into round that;
        // a radius wider than the card takes is the reference's.
        let toned = effect.kind == "tone" || effect.kind == "highlight";
        let (soft, by) = match toned {
            true => (effect.number("softness", frame), effect.number("width", frame)),
            false => (effect.number("blur", frame), effect.number("grow", frame)),
        };
        let sigma = soft * scale / 2.0;
        let (middle, taps) = if sigma <= 0.05 { (1.0, Vec::new()) } else { readings(&kernel(iskra_render::boxes(sigma))) };
        let widening = |by: f64| if (by * scale).abs() < 0.5 { 0 } else { (by * scale).abs().round() as u32 };
        if (toned || effect.kind == "cutter") && taps.len() <= TAPS && widening(by) <= WIDEST {
            let (key, looks) = (self.reference.key(of.shot, of.palette, of.space, frame, view, card), of.looks.cloned());
            if let Some(kept) = self.kept_effect(key.as_ref(), &looks) {
                return Some(kept);
            }
            self.effects.0 += 1;
            let room = iskra_render::reach(effect, frame, scale, view.width.max(view.height) as usize) as u32;
            let wide = view.with_room(room as usize);
            let [x0, y0, x1, y1] = match Self::painted_part(of, &wide, inside) {
                Some([x0, y0, x1, y1]) if x0 < x1 && y0 < y1 => {
                    [x0.saturating_sub(room), y0.saturating_sub(room), (x1 + room).min(wide.width), (y1 + room).min(wide.height)]
                }
                Some(_) => return None,
                None => [0, 0, wide.width, wide.height],
            };
            let part = View {
                width: x1 - x0,
                height: y1 - y0,
                from_frame: Affine::translate(-f64::from(x0), -f64::from(y0)).mul(&wide.from_frame),
                frame: wide.frame,
            };
            let size = (part.width, part.height);
            let picture = |compositor: &Self| compositor.target("iskra through", size.0, size.1, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
            let (inner, at) = (picture(self), [room as f32 - x0 as f32, room as f32 - y0 as f32]);
            self.draw(of, &part, inside, &inner);
            let matte = mattes.iter().find(|(port, _)| port == iskra_model::MATTE).map(|(_, cards)| {
                let matte = picture(self);
                self.draw(&Of { looks: None, ..*of }, &part, cards, &matte);
                matte
            });
            let grown = |compositor: &Self, plane: &wgpu::TextureView, by: f64| match widening(by) {
                0 => plane.clone(),
                radius => compositor.widened(plane, size, radius, by > 0.0, None),
            };
            let made = match (toned, matte) {
                // Where the matte is, a little wider; or with none, a rim inside the picture's
                // edge: the shape less the shape made narrower.
                (true, matte) => {
                    let region = match matte {
                        Some(matte) => grown(self, &matte, by),
                        None => match widening(-by) {
                            0 => self.target("iskra rim", size.0, size.1, FORMAT).create_view(&wgpu::TextureViewDescriptor::default()),
                            radius => self.widened(&inner, size, radius, -by > 0.0, Some(&inner)),
                        },
                    };
                    let region = self.blurred(&region, size, middle, &taps);
                    let shade = iskra_render::paint(effect.colour("colour", frame), of.space);
                    self.pasted(&inner, &region, at, Some(Pasted::Toned(shade, (effect.number("amount", frame) / 100.0) as f32)))
                }
                (false, Some(matte)) => {
                    let cut = self.blurred(&grown(self, &matte, by), size, middle, &taps);
                    self.pasted(&inner, &cut, at, Some(Pasted::Cut(effect.switch("inverted", frame))))
                }
                // With no matte wired there is nothing to cut by.
                (false, None) => self.pasted(&inner, &inner, at, None),
            };
            self.keep_effect(key, looks, &made);
            return Some(made);
        }
        // A Drop Shadow: the picture's shape moved aside, softened, and laid under it as a glow
        // is, round what goes through with the room it reaches into.
        let (middle, taps) = match effect.kind == "dropShadow" {
            true => match effect.number("softness", frame) * scale / 2.0 {
                sigma if sigma <= 0.05 => (1.0, Vec::new()),
                sigma => readings(&kernel(iskra_render::boxes(sigma))),
            },
            false => (1.0, Vec::new()),
        };
        if effect.kind == "dropShadow" && taps.len() <= TAPS {
            let (key, looks) = (self.reference.key(of.shot, of.palette, of.space, frame, view, card), of.looks.cloned());
            if let Some(kept) = self.kept_effect(key.as_ref(), &looks) {
                return Some(kept);
            }
            self.effects.0 += 1;
            let room = iskra_render::reach(effect, frame, scale, view.width.max(view.height) as usize) as u32;
            let wide = view.with_room(room as usize);
            let [x0, y0, x1, y1] = match Self::painted_part(of, &wide, inside) {
                Some([x0, y0, x1, y1]) if x0 < x1 && y0 < y1 => {
                    [x0.saturating_sub(room), y0.saturating_sub(room), (x1 + room).min(wide.width), (y1 + room).min(wide.height)]
                }
                Some(_) => return None,
                None => [0, 0, wide.width, wide.height],
            };
            let part = View {
                width: x1 - x0,
                height: y1 - y0,
                from_frame: Affine::translate(-f64::from(x0), -f64::from(y0)).mul(&wide.from_frame),
                frame: wide.frame,
            };
            let size = (part.width, part.height);
            let picture = |compositor: &Self| compositor.target("iskra through", size.0, size.1, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
            let inner = picture(self);
            self.draw(of, &part, inside, &inner);
            // Moved aside by whole pixels: each pixel what the picture has that far up and to
            // the left of it.
            let (across, down) = ((effect.number("offsetX", frame) * scale).round() as f32, (effect.number("offsetY", frame) * scale).round() as f32);
            let moved = picture(self);
            self.pass("iskra moved", &self.paste, &self.pasted(&inner, &inner, [-across, -down], None), &moved);
            let shadow = self.blurred(&moved, size, middle, &taps);
            let dark = iskra_render::paint(effect.colour("colour", frame), of.space);
            let opacity = (effect.number("opacity", frame) / 100.0) as f32;
            let made = self.pasted(&inner, &shadow, [room as f32 - x0 as f32, room as f32 - y0 as f32], Some(Pasted::Glow(dark, opacity)));
            self.keep_effect(key, looks, &made);
            return Some(made);
        }
        // A Directional Blur along a line through each pixel, worked out round what goes
        // through with its room; a Radial Blur, which reads from as far as it turns or pulls,
        // over the whole picture with its room.
        let radial = effect.kind == "radialBlur";
        if radial || effect.kind == "directionalBlur" {
            let (key, looks) = (self.reference.key(of.shot, of.palette, of.space, frame, view, card), of.looks.cloned());
            if let Some(kept) = self.kept_effect(key.as_ref(), &looks) {
                return Some(kept);
            }
            self.effects.0 += 1;
            let room = iskra_render::reach(effect, frame, scale, view.width.max(view.height) as usize) as u32;
            let wide = view.with_room(room as usize);
            let [x0, y0, x1, y1] = match Self::painted_part(of, &wide, inside).filter(|_| !radial) {
                Some([x0, y0, x1, y1]) if x0 < x1 && y0 < y1 => {
                    [x0.saturating_sub(room), y0.saturating_sub(room), (x1 + room).min(wide.width), (y1 + room).min(wide.height)]
                }
                Some(_) => return None,
                None => [0, 0, wide.width, wide.height],
            };
            let part = View {
                width: x1 - x0,
                height: y1 - y0,
                from_frame: Affine::translate(-f64::from(x0), -f64::from(y0)).mul(&wide.from_frame),
                frame: wide.frame,
            };
            let picture =
                |compositor: &Self| compositor.target("iskra through", part.width, part.height, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
            let inner = picture(self);
            self.draw(of, &part, inside, &inner);
            let number = |id: &str| effect.number(id, frame);
            // What it is told: which, how many readings, along what, how long, how much, round
            // where. One that moves nothing leaves the picture as it is.
            let told = match radial {
                false => {
                    // Its own angle is from straight up, clockwise.
                    let (angle, length) = ((90.0 - number("angle")).to_radians(), number("radius") * scale * 2.0);
                    let count = (length.ceil() as usize * 2 + 1).clamp(3, 257);
                    (length >= 0.5).then_some([0.0, count as f32, angle.sin() as f32, -angle.cos() as f32, length as f32, 0.0, 0.0, 0.0])
                }
                true => {
                    let zoom = effect.choice("kind", frame) == "Zoom";
                    let amount = if zoom { number("amount") / 100.0 } else { number("amount") };
                    let centre = part.from_frame.apply(Point::new(number("centreX"), number("centreY")));
                    (amount.abs() >= 1e-6).then_some([if zoom { 2.0 } else { 1.0 }, 32.0, 0.0, 0.0, 0.0, amount as f32, centre.x as f32, centre.y as f32])
                }
            };
            let blurred = match told {
                None => inner,
                Some(told) => {
                    let told: Vec<u8> = told.into_iter().flat_map(f32::to_le_bytes).collect();
                    let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("iskra motion"),
                        contents: &told,
                        usage: wgpu::BufferUsages::UNIFORM,
                    });
                    let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("iskra motion"),
                        layout: &self.correct_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&inner) },
                            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&inner) },
                        ],
                    });
                    let blurred = picture(self);
                    self.pass("iskra motion", &self.motion, &bindings, &blurred);
                    blurred
                }
            };
            let made = self.pasted(&blurred, &blurred, [room as f32 - x0 as f32, room as f32 - y0 as f32], None);
            self.keep_effect(key, looks, &made);
            return Some(made);
        }
        // Particles are worked out by the reference and laid on the card.
        if effect.kind == iskra_model::PARTICLES
            && let Some(particles) = self.reference.particles(of.shot, of.palette, of.space, frame, view, card)
            && let Some(laid) = self.scattered(&particles, view)
        {
            self.effects.0 += 1;
            let faded = Self::faded_alike(of, card).map(Pasted::Faded);
            return Some(self.pasted(&laid, &laid, [0.0, 0.0], faded));
        }
        let glow = effect.kind == "glow";
        // The blur, or the spread of a glow, and the room the effect reaches into.
        let (middle, taps) = readings(&kernel(iskra_render::boxes(effect.number("radius", frame) * scale / 2.0)));
        let room = iskra_render::reach(effect, frame, scale, view.width.max(view.height) as usize);
        // Within its room a blur never reads past the picture's edges, where the reference has
        // nothing and the card would have the edge again.
        let on_card = (glow || effect.kind == "blur") && taps.len() <= TAPS && 2 * taps.len() <= room;
        if !on_card {
            self.effects.1 += 1;
            // The reference makes it, and the card is sent the part of the picture where it
            // changes anything, to be put there: none where it changes nothing.
            let made = self.reference.through_part(of.shot, of.palette, of.space, frame, view, card)?;
            if made.width == 0 {
                return None;
            }
            let described = wgpu::TextureDescriptor {
                label: Some("iskra effect"),
                size: wgpu::Extent3d { width: made.width as u32, height: made.height() as u32, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SENT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            };
            let bytes: Vec<u8> = made.pixels.iter().flatten().flat_map(|number| number.to_le_bytes()).collect();
            let texture = self.device.create_texture_with_data(&self.queue, &described, wgpu::util::TextureDataOrder::LayerMajor, &bytes);
            let at = [-(made.left as f32), -(made.top as f32)];
            let made = texture.create_view(&wgpu::TextureViewDescriptor::default());
            // The reference knows nothing of how the window looks at the layers: where all that
            // goes through is faded alike, by the light table or as a board under the
            // animation, the picture is put in as faded.
            let faded = Self::faded_alike(of, card).map(Pasted::Faded);
            return Some(self.pasted(&made, &made, at, faded));
        }
        self.effects.0 += 1;
        // What goes through, with that room round it.
        let wide = view.with_room(room);
        let inner = self.target("iskra through", wide.width, wide.height, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
        self.draw(of, &wide, inside, &inner);
        let blurred = self.blurred(&inner, (wide.width, wide.height), middle, &taps);
        if !glow {
            return Some(self.pasted(&blurred, &blurred, [room as f32; 2], None));
        }
        // The picture over a light as wide as its shape is spread.
        let light = iskra_render::paint(effect.colour("colour", frame), of.space);
        Some(self.pasted(&inner, &blurred, [room as f32; 2], Some(Pasted::Glow(light, (effect.number("strength", frame) / 100.0) as f32))))
    }

    /// Draw one triangle over the whole of `into` by `pipeline`, told and given what
    /// `bindings` hold.
    fn pass(&self, label: &str, pipeline: &wgpu::RenderPipeline, bindings: &wgpu::BindGroup, into: &wgpu::TextureView) {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: into,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bindings, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
    }

    /// `picture`, of `size`, blurred as the reference blurs: by the kernel `middle` and `taps`
    /// say ([`readings`]), along the rows and then along the columns of what that gave. As it
    /// is where there are no taps.
    fn blurred(&self, picture: &wgpu::TextureView, size: (u32, u32), middle: f32, taps: &[[f32; 2]]) -> wgpu::TextureView {
        let mut blurred = picture.clone();
        for step in [[1.0f32, 0.0], [0.0, 1.0]].into_iter().filter(|_| !taps.is_empty()) {
            let mut told = vec![step[0], step[1], middle, taps.len() as f32];
            told.extend((0..TAPS).flat_map(|at| taps.get(at).map_or([0.0; 4], |tap| [tap[0], tap[1], 0.0, 0.0])));
            let told: Vec<u8> = told.into_iter().flat_map(f32::to_le_bytes).collect();
            let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("iskra blur"),
                contents: &told,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("iskra blur"),
                layout: &self.blur_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&blurred) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.between) },
                ],
            });
            let next = self.target("iskra blur", size.0, size.1, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
            self.pass("iskra blur", &self.blur, &bindings, &next);
            blurred = next;
        }
        blurred
    }

    /// The alpha of `plane`, of `size`, made wider by `radius` pixels all round, or with `grow`
    /// false narrower, as the reference's `grown` makes it; with `rim`, what is left of `rim`'s
    /// alpha less that, the band inside its edge.
    fn widened(&self, plane: &wgpu::TextureView, size: (u32, u32), radius: u32, grow: bool, rim: Option<&wgpu::TextureView>) -> wgpu::TextureView {
        let mut widened = plane.clone();
        for (step, last) in [([1.0f32, 0.0], false), ([0.0, 1.0], true)] {
            let rim = rim.filter(|_| last);
            let told = [step[0], step[1], radius as f32, if grow { 1.0 } else { 0.0 }, if rim.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
            let told: Vec<u8> = told.into_iter().flat_map(f32::to_le_bytes).collect();
            let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("iskra widen"),
                contents: &told,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("iskra widen"),
                layout: &self.correct_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&widened) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(rim.unwrap_or(&widened)) },
                ],
            });
            let next = self.target("iskra widen", size.0, size.1, FORMAT).create_view(&wgpu::TextureViewDescriptor::default());
            self.pass("iskra widen", &self.widen, &bindings, &next);
            widened = next;
        }
        widened
    }

    /// `picture` as the Colour Correction `effect` changes it on `frame`, into `into`, which is
    /// as large.
    fn corrected(&self, effect: &iskra_model::Effect, frame: u32, space: WorkingSpace, picture: &wgpu::TextureView, into: &wgpu::TextureView) {
        let number = |id: &str| effect.number(id, frame) as f32;
        let black = number("black") / 100.0;
        let span = (number("white") / 100.0 - black).max(0.001);
        let middle = 1.0 / number("middle").max(0.01);
        // A turn round the grey axis, which takes red to green at a third of the way round.
        let (sin, cos) = number("hue").to_radians().sin_cos();
        let (same, part, side) = (cos + (1.0 - cos) / 3.0, (1.0 - cos) / 3.0, sin / 3.0f32.sqrt());
        let tint = effect.colour("tint", frame).map(|value| value as f32);
        let curves = effect.curves("curves");
        let told = [
            black,
            span,
            middle,
            1.0 + number("saturation") / 100.0,
            number("lightness") / 100.0,
            tint[3] * number("tintAmount") / 100.0,
            if space == WorkingSpace::Linear { 1.0 } else { 0.0 },
            if curves.is_straight() { 0.0 } else { 1.0 },
            same,
            part + side,
            part - side,
            0.0,
            tint[0],
            tint[1],
            tint[2],
            tint[3],
        ];
        let told: Vec<u8> = told.into_iter().flat_map(f32::to_le_bytes).collect();
        let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("iskra correct"),
            contents: &told,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // The four curves side by side in each step's channels: the master's, red's, green's and
        // blue's.
        let steps = iskra_render::CURVE_STEPS;
        let tables: [Vec<f32>; 4] = std::array::from_fn(|at| curves.table(at, steps));
        let texels: Vec<u8> = (0..steps).flat_map(|step| tables.iter().map(move |table| table[step])).flat_map(f32::to_le_bytes).collect();
        let described = wgpu::TextureDescriptor {
            label: Some("iskra curves"),
            size: wgpu::Extent3d { width: steps as u32, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SENT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        let tables = self.device.create_texture_with_data(&self.queue, &described, wgpu::util::TextureDataOrder::LayerMajor, &texels);
        let tables = tables.create_view(&wgpu::TextureViewDescriptor::default());
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("iskra correct"),
            layout: &self.correct_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(picture) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&tables) },
            ],
        });
        self.pass("iskra correct", &self.correct, &bindings, into);
    }

    /// An effect's picture made on the card lately, ready to put in, where all it is made of
    /// (`key`) and how the window looks at it are the same: moving what is outside it, or
    /// playing, makes it no more.
    fn kept_effect(&mut self, key: Option<&iskra_render::Key>, looks: &Option<Looks>) -> Option<wgpu::BindGroup> {
        let kept = self.kept_effects.iter_mut().find(|kept| Some(&kept.0) == key && kept.1 == *looks)?;
        kept.3 = self.frames;
        self.effects_kept += 1;
        Some(kept.2.clone())
    }

    /// Keep an effect's picture `made` on the card, by what it is made of and how the window
    /// looks at it, for [`Compositor::kept_effect`] to find.
    fn keep_effect(&mut self, key: Option<iskra_render::Key>, looks: Option<Looks>, made: &wgpu::BindGroup) {
        let Some(key) = key else { return };
        // Those not used for a few frames go, and the longest unused past the most kept.
        let frames = self.frames;
        self.kept_effects.retain(|kept| kept.3 + EFFECTS_KEPT_FRAMES > frames);
        if self.kept_effects.len() >= EFFECTS_KEPT {
            self.kept_effects.sort_by_key(|kept| std::cmp::Reverse(kept.3));
            self.kept_effects.truncate(EFFECTS_KEPT - 1);
        }
        self.kept_effects.push((key, looks, made.clone(), frames));
    }

    /// What puts `made` into another picture: as it is, or for a glow over its light, which
    /// is as wide as `spread` is solid. `offset` is the pixel of `made` that the picture's
    /// first pixel is: its room, for one made with room round what is wanted of it, or less
    /// than nothing for a part of the picture that lies away from its corner. Outside `made`
    /// the picture is left as it is.
    fn pasted(&self, made: &wgpu::TextureView, spread: &wgpu::TextureView, offset: [f32; 2], how: Option<Pasted>) -> wgpu::BindGroup {
        let (kind, light, strength, matte) = match how {
            None => (0.0, [0.0; 4], 0.0, None),
            Some(Pasted::Glow(light, strength)) => (1.0, light, strength, None),
            Some(Pasted::Faded(kept)) => (2.0, [0.0; 4], kept, None),
            Some(Pasted::Styled(share, matte)) => (3.0, [if matte.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0], share, matte),
            Some(Pasted::Cut(inverted)) => (4.0, [0.0; 4], if inverted { 1.0 } else { 0.0 }, None),
            Some(Pasted::Filled(colour)) => (5.0, colour, 0.0, None),
            Some(Pasted::Toned(shade, amount)) => (6.0, shade, amount, None),
        };
        let told = [offset[0], offset[1], kind, strength, light[0], light[1], light[2], light[3]];
        let told: Vec<u8> = told.into_iter().flat_map(f32::to_le_bytes).collect();
        let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("iskra paste"),
            contents: &told,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("iskra paste"),
            layout: &self.paste_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(made) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(spread) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(matte.as_ref().unwrap_or(made)) },
            ],
        })
    }

    /// What draws a drawing that the camera sees into a picture as large as `view`: one for
    /// each of its pieces on the card, where it is sent first if it is not there.
    /// `look` is how the window looks at it, and with `lines_only` only its line art is drawn.
    fn pieces(&mut self, of: &Of, view: &View, seen: iskra_render::Seen, look: Look, lines_only: bool) -> Vec<Step> {
        let (palette, space) = (of.palette, of.space);
        // The art layers that are not drawn: all but the line art of a ghost of the lines alone,
        // and those its layer has switched off.
        let off = Off {
            colour: lines_only || seen.off.contains(&ArtLayerKind::ColourArt),
            lines: seen.off.contains(&ArtLayerKind::LineArt),
            over: lines_only || seen.off.contains(&ArtLayerKind::Overart),
            ghost: look.tint.is_some(),
        };
        let mut draws = Vec::new();
        // Sampled as the reference samples it.
        let projected = seen.to_frame.then(&view.from_frame);
        // One painted with a gradient or a picture is flattened as the reference paints it.
        let shaded = self.is_shaded(of, &seen.drawing);
        // A drawing that is seen as it was drawn is kept on the card as it was painted, and
        // is a piece for each part of it: what is painted there, read from textures that start
        // at a tile's corner.
        if seen.warp.is_none() && !shaded && self.keep(&seen.drawing) {
            let Some(sampling) = iskra_render::Sampling::of(&projected, seen.bounds, view.width as usize, view.height as usize) else { return draws };
            let colours = self.colours(of);
            let Some(sheet) = self.sheets.get(&(Arc::as_ptr(&seen.drawing) as usize)) else { return draws };
            for part in &sheet.parts {
                let drawn = part.part.intersect(&seen.bounds);
                if drawn.is_empty() {
                    continue;
                }
                let piece = Piece { part: drawn, origin: [part.held.x0, part.held.y0], view: self.nothing.clone(), bytes: 0 };
                let told = self.told(&sampling, &projected, view, seen.bounds, &piece, space, look);
                draws.push(Step::Sheet(self.read_sheet(&part.arts, &told, &colours, off)));
            }
            return draws;
        }
        // A bent drawing is kept as it is bent: by its bend too, and with its bounds then.
        let bend = seen.warp.as_ref().map_or(0, |warp| warp.key());
        // And by which of its art layers are drawn, and the space its gradients are blended in.
        let drawn_as = of.style.wrapping_mul(16).wrapping_add(off.key()).wrapping_mul(2).wrapping_add(usize::from(space == WorkingSpace::Linear));
        let key = (Arc::as_ptr(&seen.drawing) as usize, Arc::as_ptr(palette) as usize, drawn_as, bend);
        if let Some(sheet) = self.sheets.get_mut(&(Arc::as_ptr(&seen.drawing) as usize)) {
            // What it was bent from stays as long as it does, for the next bend of it.
            sheet.used = self.frames;
        }
        // Bent on the card from the drawing kept as it was painted, where it can be.
        if !self.drawings.contains_key(&key)
            && !shaded
            && let Some(warp) = &seen.warp
            && self.keep(&seen.drawing)
            && let Some(uploaded) = self.bent_on_card(of, &seen, warp, off)
        {
            self.bends.0 += 1;
            self.drawings.insert(key, uploaded);
        }
        if !self.drawings.contains_key(&key) {
            self.bends.1 += u64::from(seen.warp.is_some());
            // Bent as the reference bends it, on the processor, and drawn from there on
            // as any drawing is.
            let lines = off.any().then(|| {
                let art_layers = seen.drawing.art_layers.iter().filter(|layer| off.shows(layer.kind)).cloned().collect();
                Arc::new(Drawing { art_layers, texts: Vec::new(), placements: seen.drawing.placements.clone() })
            });
            let drawn = lines.as_ref().unwrap_or(&seen.drawing);
            // A ghost is flattened with every swatch whole: what it covers.
            let whole = off.ghost.then(|| {
                let mut whole = of.colours.clone();
                whole.swatches.iter_mut().for_each(|swatch| (swatch.color[3], swatch.kind) = (255, iskra_model::SwatchKind::Solid));
                whole
            });
            let colours = whole.as_ref().unwrap_or(of.colours);
            let bent = seen.warp.as_ref().map(|warp| iskra_render::bend(drawn, seen.bounds, colours, warp));
            let bounds = bent.as_ref().map_or(seen.bounds, |bent| bent.bounds);
            let mut uploaded = self.upload(drawn, bounds, palette, colours, space, bent.as_ref());
            // The address that is the key is the whole drawing's.
            uploaded._drawing = seen.drawing.clone();
            self.drawings.insert(key, uploaded);
        }
        let Some(bounds) = self.drawings.get(&key).map(|uploaded| uploaded.bounds) else { return draws };
        let seen = iskra_render::Seen { bounds, ..seen };
        let Some(sampling) = iskra_render::Sampling::of(&projected, seen.bounds, view.width as usize, view.height as usize) else { return draws };
        let Some(uploaded) = self.drawings.get_mut(&key) else { return draws };
        uploaded.used = self.frames;
        let Some(uploaded) = self.drawings.get(&key) else { return draws };
        for piece in &uploaded.pieces {
            let buffer = self.told(&sampling, &projected, view, seen.bounds, piece, space, look);
            draws.push(Step::Piece(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("iskra piece"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&piece.view) },
                ],
            })));
        }
        draws
    }

    /// What reads a part of a drawing kept as it was painted, its art layers `arts` but those
    /// `off`, with `told` for what draws it and `colours` for the palette.
    fn read_sheet(&self, arts: &[Art], told: &wgpu::Buffer, colours: &wgpu::TextureView, off: Off) -> wgpu::BindGroup {
        let mut kinds = [0u32; PLACES];
        for (kind, art) in kinds.iter_mut().zip(arts).filter(|(_, art)| off.shows(art.kind)) {
            *kind = if art.indexed { 1 } else { 2 };
        }
        let kinds = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("iskra sheet"),
            contents: &kinds.iter().flat_map(|kind| kind.to_le_bytes()).collect::<Vec<u8>>(),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let art = |place: usize| wgpu::BindingResource::TextureView(arts.get(place).map_or(&self.nothing, |art| &art.view));
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("iskra sheet"),
            layout: &self.sheet_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: told.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: art(0) },
                wgpu::BindGroupEntry { binding: 2, resource: art(1) },
                wgpu::BindGroupEntry { binding: 3, resource: art(2) },
                wgpu::BindGroupEntry { binding: 4, resource: art(3) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(colours) },
                wgpu::BindGroupEntry { binding: 6, resource: kinds.as_entire_binding() },
            ],
        })
    }

    /// `seen`'s drawing, which is kept as it was painted, bent by `warp` on the card into a
    /// picture of its own, as the reference bends it: without the art layers `off`.
    /// `None` where the reference would not bend it, or where what it is bent over is larger
    /// than a texture can be.
    fn bent_on_card(&mut self, of: &Of, seen: &iskra_render::Seen, warp: &Warp, off: Off) -> Option<Uploaded> {
        // A pixel more all round, as the reference has it.
        let mesh = iskra_render::Mesh::of(seen.bounds.inflate(1), warp);
        let bent = mesh.bent().filter(|bent| bent.width() <= self.longest && bent.height() <= self.longest)?;
        let colours = self.colours(of);
        // Bent from one texture of each art layer.
        let [sheet] = &self.sheets.get(&(Arc::as_ptr(&seen.drawing) as usize))?.parts[..] else { return None };
        // Each corner of each triangle: where it is bent to, where it is at rest, and how far
        // back the triangle is, the first the nearest.
        let count = mesh.triangles.len() as f32 + 1.0;
        let corners: Vec<f32> = (mesh.triangles.iter().enumerate())
            .flat_map(|(order, triangle)| triangle.map(|corner| (mesh.corners[corner as usize], order as f32 / count)))
            .flat_map(|((rest, posed), order)| [posed.x as f32, posed.y as f32, rest.x as f32, rest.y as f32, order])
            .collect();
        let corners = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("iskra bend"),
            contents: &corners.iter().flat_map(|number| number.to_le_bytes()).collect::<Vec<u8>>(),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ghost = if off.ghost { 1.0 } else { 0.0 };
        let told = [bent.x0 as f32, bent.y0 as f32, bent.width() as f32, bent.height() as f32, sheet.held.x0 as f32, sheet.held.y0 as f32, ghost, 0.0];
        let told = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("iskra bend"),
            contents: &told.iter().flat_map(|number| number.to_le_bytes()).collect::<Vec<u8>>(),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bindings = self.read_sheet(&sheet.arts, &told, &colours, off);
        let texture = self.target("iskra bent drawing", bent.width(), bent.height(), BENT);
        let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let order = self.target("iskra bend order", bent.width(), bent.height(), BENT_ORDER).create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("iskra bend") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("iskra bend"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &order,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.bend);
            pass.set_bind_group(0, &bindings, &[]);
            pass.set_vertex_buffer(0, corners.slice(..));
            pass.draw(0..3 * mesh.triangles.len() as u32, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bytes = bent.width() as usize * bent.height() as usize * 4;
        let piece = Piece { part: bent, origin: [bent.x0, bent.y0], view, bytes };
        Some(Uploaded { pieces: vec![piece], bounds: bent, used: self.frames, _drawing: seen.drawing.clone(), _palette: of.palette.clone() })
    }

    /// What the shader is told to draw one piece of a drawing whose painted part is `bounds`
    /// into a picture as large as `view`, as a buffer on the card.
    #[allow(clippy::too_many_arguments)]
    fn told(
        &self,
        sampling: &iskra_render::Sampling,
        projected: &Projection,
        view: &View,
        bounds: Rect,
        piece: &Piece,
        space: WorkingSpace,
        look: Look,
    ) -> wgpu::Buffer {
        // A tint is given in the working space, as the colours it stands in for are.
        let lit = |value: f32| if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) };
        let tint = look.tint.map_or([0.0; 4], |[r, g, b]| if space == WorkingSpace::Linear { [lit(r), lit(g), lit(b), 1.0] } else { [r, g, b, 1.0] });
        let to_picture = projected.as_flat().unwrap_or(Affine::IDENTITY);
        let back = sampling.back;
        // How many drawing pixels one pixel of the picture is across, for a flat drawing.
        let shrunk = (back[0][0] * back[1][1] - back[0][1] * back[1][0]).abs().sqrt();
        // Past the drawing's own edges a pixel of the picture still takes a share of it.
        let reach = (shrunk * 1.5 + 1.0).ceil() as i32;
        let (picture, picture_moves) = ([0, 1, 2, 3].map(|i| to_picture.m[i] as f32), [to_picture.m[4] as f32, to_picture.m[5] as f32]);
        let back = back.map(|row| [row[0] as f32, row[1] as f32, row[2] as f32, 0.0]);
        let part = piece.part;
        let out = |side: i32, edge: i32, by: i32| if side == edge { side + by } else { side };
        // Of a tilted drawing, the points that are this piece's: to its sides, and past them
        // where they are the drawing's edges.
        let far = 1 << 30;
        let own = [out(part.x0, bounds.x0, -far), out(part.y0, bounds.y0, -far), out(part.x1, bounds.x1, far), out(part.y1, bounds.y1, far)];
        let part = [out(part.x0, bounds.x0, -reach), out(part.y0, bounds.y0, -reach), out(part.x1, bounds.x1, reach), out(part.y1, bounds.y1, reach)];
        // A tilted drawing is drawn over all of the picture it can reach, and each
        // piece of it puts in what it holds.
        let part = if sampling.tilted { sampling.reach.map(|side| side as f32) } else { part.map(|side| side as f32) };
        let told = Told {
            back,
            to_picture: picture,
            moves: [0.0, 0.0, picture_moves[0], picture_moves[1]],
            part,
            origin: piece.origin.map(|side| side as f32),
            picture: [view.width as f32, view.height as f32],
            points: sampling.points as f32,
            linear: if space == WorkingSpace::Linear { 1.0 } else { 0.0 },
            tilted: if sampling.tilted { 1.0 } else { 0.0 },
            nearest: iskra_model::NEAREST as f32,
            tint,
            fade: [look.opacity.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
            own: own.map(|side| side as f32),
        };
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("iskra piece"),
            contents: &told.bytes(),
            usage: wgpu::BufferUsages::UNIFORM,
        })
    }

    /// Past the budget, the drawings that were drawn longest ago leave the card; never those
    /// of the frame just made.
    fn forget(&mut self) {
        let mut kept = self.kept();
        if kept <= self.budget {
            return;
        }
        // Flattened or as they were painted, they are counted alike.
        enum Kept {
            Flat((usize, usize, usize, u64)),
            Sheet(usize),
        }
        let flat = self.drawings.iter().map(|(key, uploaded)| (Kept::Flat(*key), uploaded.used, uploaded.pieces.iter().map(|piece| piece.bytes).sum()));
        let sheets = self.sheets.iter().map(|(key, sheet)| (Kept::Sheet(*key), sheet.used, sheet.bytes()));
        let mut oldest: Vec<(Kept, u64, usize)> = flat.chain(sheets).collect();
        oldest.sort_by_key(|(_, used, _)| *used);
        let before = self.sheets.len();
        for (key, used, bytes) in oldest {
            if kept <= self.budget || used == self.frames {
                break;
            }
            match key {
                Kept::Flat(key) => drop(self.drawings.remove(&key)),
                Kept::Sheet(key) => drop(self.sheets.remove(&key)),
            }
            kept -= bytes;
        }
        // The tiles of the drawings that left are in none.
        if self.sheets.len() != before {
            self.homes.retain(|_, home| self.sheets.contains_key(home));
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// A frame as a texture to show ([`SHOWN`]): what a window puts on screen as it is. The
    /// frame itself stays as it was made.
    pub fn shown(&self, frame: &Frame) -> wgpu::Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("iskra shown frame"),
            size: wgpu::Extent3d { width: frame.width, height: frame.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHOWN,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let linear: f32 = if frame.space == WorkingSpace::Linear { 1.0 } else { 0.0 };
        // Padded to the size the card wants of what it is told.
        let told: Vec<u8> = [linear, 0.0, 0.0, 0.0].into_iter().flat_map(f32::to_le_bytes).collect();
        let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("iskra show"),
            contents: &told,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let from = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("iskra show"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&from) },
            ],
        });
        let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("iskra show") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("iskra show"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.show);
            pass.set_bind_group(0, &bindings, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        texture
    }

    /// Read a shown frame back from the card: rows of sRGB colour times alpha. `None` if the
    /// card could not be read.
    pub fn read_shown(&self, shown: &wgpu::Texture) -> Option<Vec<[u8; 4]>> {
        let (width, height) = (shown.width(), shown.height());
        let bytes = self.read_rows(shown, width, height, 4)?;
        Some(bytes.as_chunks::<4>().0.to_vec())
    }

    /// The rows of a texture, read back from the card, `size` bytes a pixel.
    fn read_rows(&self, texture: &wgpu::Texture, width: u32, height: u32, size: u32) -> Option<Vec<u8>> {
        // A row in a buffer is a multiple of 256 bytes long.
        let row = (width * size).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("iskra readback"),
            size: u64::from(row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("iskra readback") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (done, mapped) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = done.send(result);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        mapped.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut rows = Vec::with_capacity((width * height * size) as usize);
        for y in 0..height as usize {
            rows.extend_from_slice(data.get(y * row as usize..y * row as usize + (width * size) as usize)?);
        }
        drop(data);
        buffer.unmap();
        Some(rows)
    }

    /// Read a frame back from the card as the picture the reference would hand over: straight
    /// alpha, sRGB, 8 bits a channel. `None` if the card could not be read.
    pub fn read(&self, frame: &Frame) -> Option<Image> {
        let (width, height) = (frame.width, frame.height);
        let bytes = self.read_rows(&frame.texture, width, height, 8)?;
        let number = |pixel: &[u8; 8], channel: usize| half(u16::from_le_bytes([pixel[channel * 2], pixel[channel * 2 + 1]]));
        let pixels: Vec<[f32; 4]> = bytes.as_chunks::<8>().0.iter().map(|pixel| std::array::from_fn(|channel| number(pixel, channel))).collect();
        Some(iskra_render::finish(width, height, &pixels, frame.space))
    }
}

/// A 16-bit float as the number it is.
fn half(bits: u16) -> f32 {
    let (sign, exponent, fraction) = (bits >> 15, (bits >> 10) & 0x1f, f32::from(bits & 0x3ff));
    let size = match exponent {
        0 => fraction * 2f32.powi(-24),
        31 if fraction == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        exponent => (1.0 + fraction / 1024.0) * 2f32.powi(i32::from(exponent) - 15),
    };
    if sign == 1 { -size } else { size }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_passes_of_a_mean_are_one_kernel_read_in_pairs() {
        // One pass over three pixels is a third of each; no pass at all is the pixel itself.
        assert_eq!((kernel([0, 0, 0]), kernel([1, 0, 0])), (vec![1.0], vec![1.0 / 3.0, 1.0 / 3.0]));
        // Three passes reach as far as their r together, and all of the picture is still there.
        let wide = kernel([2, 2, 3]);
        assert_eq!(wide.len(), 8);
        assert!((wide[0] + 2.0 * wide[1..].iter().sum::<f64>() - 1.0).abs() < 1e-12 && wide.windows(2).all(|pair| pair[0] >= pair[1]));
        // Read in pairs: each reading lies between its two pixels, nearer the one that counts
        // more, and counts for both; an odd one out is read where it is.
        let (middle, taps) = readings(&wide);
        assert_eq!((middle, taps.len()), (wide[0] as f32, 4));
        assert!(taps[0][0] > 1.0 && taps[0][0] < 1.5 && (f64::from(taps[0][1]) - (wide[1] + wide[2])).abs() < 1e-7);
        assert_eq!(taps[3], [7.0, wide[7] as f32]);
        assert_eq!(readings(&kernel([0, 0, 0])), (1.0, Vec::new()));
    }

    #[test]
    fn half_floats_are_read_as_the_numbers_they_are() {
        assert_eq!((half(0x0000), half(0x3c00), half(0xc000), half(0x3800), half(0x7bff)), (0.0, 1.0, -2.0, 0.5, 65504.0));
        assert_eq!((half(0x0001), half(0x3555)), (2f32.powi(-24), 0.333_251_95));
        assert!(half(0x7c00).is_infinite() && half(0x7e00).is_nan());
    }
}
