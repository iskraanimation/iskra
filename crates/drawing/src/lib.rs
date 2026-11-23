//! Drawings and their art layers (plan §4.2, §4.8).
//!
//! A [`Drawing`] is an ordered stack of [`ArtLayer`]s, by default colour art under line art.
//! Each layer's pixels are either *indexed* ([`IndexedSurface`]: a swatch id and a coverage per
//! pixel, coloured from the palette when rendered) or free RGBA; or the layer is *vector* art
//! ([`VectorArt`]: shapes, each of one swatch, drawn as indexed pixels).

mod brushes;
mod curve;
mod fill;
pub mod fonts;
mod indexed;
mod lasso;
mod rgba;
mod scatter;
mod shade;
mod shape;
mod stamp;
mod stroke;
mod text;
mod vector;
mod vector_brush;
mod vector_edit;
mod vector_erase;
mod vector_fill;
mod vector_trace;

use std::collections::BTreeSet;

pub use brushes::{
    Brush, BrushKind, BrushPreset, Brushes, HeldRaster, HeldVector, InHand, MAX_TILE, OWN_GROUP, SLOTS as BRUSH_SLOTS, STAMP_GROUP, SURFACE_GROUP,
    VECTOR_GROUP, builtin as builtin_brushes, presets_to_json, stroke_picture, tile_of_picture,
};
pub use curve::monotone_through;
pub use fill::{Fill, FillError, fill, fill_free, fill_free_shaded, fill_protecting, recolour_line};
pub use indexed::{IndexedPixel, IndexedSurface, PaintMode, SwatchId};
pub use kurbo;
pub use lasso::{Lasso, LassoPart, Picked};
pub use photocraft_geom::{Affine, Point, Rect, TILE_SIZE, TileCoord};
pub use photocraft_paint::{BrushSettings, Stroke, StrokePoint};
pub use photocraft_paint::{Control, DualBrush, Dynamic, GrayTile, MaskMode, Pattern, PatternStyle, Texture, TipShape, tip_reach};
pub use photocraft_raster::{Surface, Tile};
pub use rgba::{LiveRgba, MixerSettings, MixerState, brush_in, fill_rgba, mix_rgba, paint_rgba, paint_rgba_as, paint_rgba_shaded};
pub use scatter::{Scatter, ScatterPreset, scattered};
use serde::{Deserialize, Serialize};
pub use shade::{Area, Areas, Frame, Gradient, GradientShape, GradientStop, Picture, Placement, Shade, shaded};
pub use shape::{Form, Shape};
pub use stamp::{MAX_STAMP_TIP, Stamp, StampBrush, remember_tip, remember_tip_as, stamp_tip, tip, tip_key, tip_of_png};
pub use stroke::{LiveStroke, paint_stroke, paint_stroke_as, paint_stroke_protecting};
pub use text::{Align, Laid, Line, TextBox, TextFormat, lay_out, lay_out_with, preview_text};
pub use vector::{Boil, Bound, Cap, Grain, Nib, ShapeId, ShapeKind, VectorArt, VectorFill, VectorShape, VectorStroke};
pub use vector_brush::{LiveVector, VectorBrush, rebrushed, surface, surface_shape, vector_stroke};
pub use vector_erase::{cut_overshoot, cut_vector, erase_vector, flattened, merge_picked, repaint_touched, split_piece};
pub use vector_trace::{Trace, vectorize};
pub mod points {
    //! The point editor's edits of a path (plan §4.16, step 8).
    pub use crate::vector_edit::*;
}
pub use vector_fill::{VectorBucket, fill_vector, fill_vector_at, recolour_vector_line, refit_fills};

/// What an art layer is for. The order of a drawing's layers is their stacking order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtLayerKind {
    /// Fills, painted under the lines.
    ColourArt,
    /// The lines.
    LineArt,
    /// Over the lines: highlights, shines and marks drawn over the inking without touching it.
    Overart,
}

impl ArtLayerKind {
    /// The art layers of a drawing, bottom first: the colour art, the lines over it, and the
    /// overart over them.
    pub const ALL: [ArtLayerKind; 3] = [ArtLayerKind::ColourArt, ArtLayerKind::LineArt, ArtLayerKind::Overart];
}

/// Which art layers of a drawing a selection takes: all of them, one alone, or one and those
/// under it (from the overart, all three; from the line art, it and the colour art). Written
/// as an art layer's name, or `{ "down": name }` for it and those under it; none is all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Option<ReachForm>", into = "Option<ReachForm>")]
pub enum ArtReach {
    #[default]
    All,
    Only(ArtLayerKind),
    Down(ArtLayerKind),
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
enum ReachForm {
    One(ArtLayerKind),
    Down { down: ArtLayerKind },
}

impl From<Option<ReachForm>> for ArtReach {
    fn from(form: Option<ReachForm>) -> Self {
        match form {
            None => ArtReach::All,
            Some(ReachForm::One(kind)) => ArtReach::Only(kind),
            Some(ReachForm::Down { down }) => ArtReach::Down(down),
        }
    }
}

impl From<ArtReach> for Option<ReachForm> {
    fn from(reach: ArtReach) -> Self {
        match reach {
            ArtReach::All => None,
            ArtReach::Only(kind) => Some(ReachForm::One(kind)),
            ArtReach::Down(down) => Some(ReachForm::Down { down }),
        }
    }
}

impl ArtReach {
    /// Whether it takes the art layer `kind`.
    pub fn takes(self, kind: ArtLayerKind) -> bool {
        let at = |kind: ArtLayerKind| ArtLayerKind::ALL.iter().position(|of| *of == kind);
        match self {
            ArtReach::All => true,
            ArtReach::Only(only) => kind == only,
            ArtReach::Down(top) => at(kind) <= at(top),
        }
    }

    /// The one art layer it takes, where it takes one alone.
    pub fn only(self) -> Option<ArtLayerKind> {
        match self {
            ArtReach::Only(kind) | ArtReach::Down(kind @ ArtLayerKind::ColourArt) => Some(kind),
            _ => None,
        }
    }
}

/// An art layer's pixels, or its vector art.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "ContentMode", into = "ContentMode")]
pub enum ArtContent {
    /// Palette-bound pixels: changing a swatch recolours them.
    Indexed(IndexedSurface),
    /// Free painterly pixels (RGBA8, straight alpha). They don't follow the palette.
    Rgba(Surface),
    /// Shapes, each of one swatch (plan §4.16). They are drawn as the indexed pixels
    /// [`VectorArt::raster`] makes of them.
    Vector(VectorArt),
}

/// How [`ArtContent`] appears in the project's JSON: the mode only. The pixels are stored
/// separately as tiles (plan §4.9), and vector art's shapes beside them, so content read from
/// JSON starts empty.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ContentMode {
    Indexed,
    Rgba,
    Vector,
}

impl From<ArtContent> for ContentMode {
    fn from(content: ArtContent) -> Self {
        match content {
            ArtContent::Indexed(_) => ContentMode::Indexed,
            ArtContent::Rgba(_) => ContentMode::Rgba,
            ArtContent::Vector(_) => ContentMode::Vector,
        }
    }
}

impl From<ContentMode> for ArtContent {
    fn from(mode: ContentMode) -> Self {
        match mode {
            ContentMode::Indexed => ArtContent::Indexed(IndexedSurface::new()),
            ContentMode::Rgba => ArtContent::Rgba(Surface::new(photocraft_color::PixelFormat::RGBA8)),
            ContentMode::Vector => ArtContent::Vector(VectorArt::default()),
        }
    }
}

impl ArtContent {
    /// Whether the pixels are free colour, which does not follow the palette.
    pub fn is_rgba(&self) -> bool {
        matches!(self, ArtContent::Rgba(_))
    }

    /// Whether the art layer is vector art.
    pub fn is_vector(&self) -> bool {
        matches!(self, ArtContent::Vector(_))
    }

    /// Whether nothing is painted or drawn on it.
    pub fn is_empty(&self) -> bool {
        match self {
            ArtContent::Vector(art) => art.is_empty(),
            _ => self.tiles().tiles().next().is_none(),
        }
    }

    /// The indexed pixels: those of an indexed layer, or those vector art is drawn as. `None`
    /// for free colour.
    pub fn indexed(&self) -> Option<&IndexedSurface> {
        match self {
            ArtContent::Indexed(surface) => Some(surface),
            ArtContent::Vector(art) => Some(art.raster()),
            ArtContent::Rgba(_) => None,
        }
    }

    /// The art as lines that bound the paint bucket: indexed pixels as they are, vector art as
    /// the pixels it is drawn as, and free colour by its alpha, as the coverage of pixels of
    /// one swatch.
    pub fn lines(&self) -> std::borrow::Cow<'_, IndexedSurface> {
        match self {
            ArtContent::Rgba(_) => match self.to_indexed(|_| SwatchId(1)) {
                ArtContent::Indexed(surface) => std::borrow::Cow::Owned(surface),
                _ => unreachable!("free colour made indexed is indexed"),
            },
            content => std::borrow::Cow::Borrowed(content.indexed().expect("indexed or vector art has indexed pixels")),
        }
    }

    /// The same picture as free colour: indexed pixels, and those vector art is drawn as, get
    /// the colours `color` gives their swatches, and from then on don't follow the palette.
    pub fn to_rgba(&self, color: impl Fn(SwatchId) -> [u8; 4]) -> ArtContent {
        let Some(surface) = self.indexed() else { return self.clone() };
        let bounds = surface.bounds();
        let pixels = surface.to_rgba8(bounds, color);
        let mut made = Surface::from_interleaved(photocraft_color::PixelFormat::RGBA8, bounds, pixels.as_flattened());
        made.prune();
        ArtContent::Rgba(made)
    }

    /// The same picture as indexed pixels: each pixel of free colour gets the swatch `swatch`
    /// gives for its colour, and its alpha as its coverage. Colours between the palette's are
    /// lost.
    /// Vector art becomes the pixels it is drawn as.
    pub fn to_indexed(&self, swatch: impl Fn([u8; 4]) -> SwatchId) -> ArtContent {
        match self {
            ArtContent::Indexed(_) => self.clone(),
            ArtContent::Vector(art) => ArtContent::Indexed(art.raster().clone()),
            ArtContent::Rgba(surface) => {
                let bounds = surface.content_bounds();
                let mut pixels = vec![[0u8; 4]; bounds.width() as usize * bounds.height() as usize];
                surface.read_rgba8_into(bounds, &mut pixels);
                ArtContent::Indexed(IndexedSurface::of_rgba8(bounds, &pixels, swatch))
            }
        }
    }

    /// The tiles as stored: indexed pixels (see [`IndexedSurface::tiles`]) or RGBA8. Of vector
    /// art, the tiles it is drawn as, which are not stored: its shapes are.
    pub fn tiles(&self) -> &Surface {
        match self {
            ArtContent::Indexed(surface) => surface.tiles(),
            ArtContent::Rgba(surface) => surface,
            ArtContent::Vector(art) => art.raster().tiles(),
        }
    }

    /// The tiles for a project store to fill when it loads a drawing. `None` for vector art,
    /// which is loaded as its shapes.
    pub fn raw_tiles_mut(&mut self) -> Option<&mut Surface> {
        match self {
            ArtContent::Indexed(surface) => Some(surface.raw_tiles_mut()),
            ArtContent::Rgba(surface) => Some(surface),
            ArtContent::Vector(_) => None,
        }
    }

    /// Exact bounds of the painted pixels.
    pub fn bounds(&self) -> Rect {
        match self {
            ArtContent::Indexed(surface) => surface.bounds(),
            ArtContent::Rgba(surface) => surface.content_bounds(),
            ArtContent::Vector(art) => art.raster().bounds(),
        }
    }

    /// What a selection takes of the pixels, as pixels of the same kind; of vector art, the
    /// whole shapes it touches (plan §4.16, step 7).
    pub fn extract_picked(&self, picked: &Picked) -> ArtContent {
        match self {
            ArtContent::Indexed(surface) => ArtContent::Indexed(surface.extract_picked(picked)),
            ArtContent::Rgba(surface) => ArtContent::Rgba(rgba::extract_rgba(surface, picked)),
            ArtContent::Vector(art) => ArtContent::Vector(art.extract_picked(picked)),
        }
    }

    /// Erase what a selection takes of the pixels, or of vector art the shapes it takes.
    pub fn clear_picked(&mut self, picked: &Picked) {
        match self {
            ArtContent::Indexed(surface) => surface.clear_picked(picked),
            ArtContent::Rgba(surface) => rgba::clear_rgba(surface, picked),
            ArtContent::Vector(art) => *art = art.clear_picked(picked),
        }
    }

    /// Give what a selection takes the swatch `to`: indexed pixels with the coverage they have,
    /// or whole shapes. Free colour has no swatches. Says whether it changed anything.
    pub fn recolour_picked(&mut self, picked: &Picked, to: SwatchId) -> bool {
        match self {
            ArtContent::Indexed(surface) => surface.recolour_picked(picked, to),
            ArtContent::Vector(art) => art.recolour_picked(picked, to).map(|recoloured| *art = recoloured).is_some(),
            ArtContent::Rgba(_) => false,
        }
    }

    /// The pixels, or the shapes, moved, scaled or turned by `transform`.
    pub fn transformed(&self, transform: &Affine) -> ArtContent {
        match self {
            ArtContent::Indexed(surface) => ArtContent::Indexed(surface.transformed(transform)),
            ArtContent::Rgba(surface) => ArtContent::Rgba(rgba::transformed_rgba(surface, transform)),
            ArtContent::Vector(art) => ArtContent::Vector(art.transformed(transform)),
        }
    }

    /// Paint `other` over these pixels, where it is of their kind: swatches over swatches,
    /// colour over colour, shapes over shapes. Says whether it was.
    pub fn stamp(&mut self, other: &ArtContent) -> bool {
        match (self, other) {
            (ArtContent::Indexed(surface), ArtContent::Indexed(other)) => surface.stamp(other),
            (ArtContent::Rgba(surface), ArtContent::Rgba(other)) => rgba::stamp_rgba(surface, other),
            (ArtContent::Vector(art), ArtContent::Vector(other)) => *art = art.stamped(other),
            _ => return false,
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtLayer {
    pub kind: ArtLayerKind,
    pub content: ArtContent,
}

/// The first id of the shapes a drawing's boxes of text are laid out into (plan §4.19): made
/// anew with the boxes, over the drawing's own shapes, whose ids are below it.
pub const TEXT_SHAPES: u32 = 1 << 24;

/// The shapes `texts` are drawn as, each in its swatch: their glyphs, for the line art, and
/// their backgrounds, for the colour art; and the fonts the machine lacked.
fn text_shapes(texts: &[TextBox], fonts: &fonts::Fonts) -> (Vec<(SwatchId, kurbo::BezPath)>, Vec<(SwatchId, kurbo::BezPath)>, Vec<String>) {
    let (mut glyphs, mut backgrounds, mut missing) = (Vec::new(), Vec::new(), Vec::new());
    for text in texts {
        let laid = lay_out(text, fonts);
        if let Some((swatch, outline)) = text.format.background.zip(laid.background(text)) {
            backgrounds.push((swatch, outline));
        }
        missing.extend(laid.stand_in.filter(|font| !missing.contains(font)));
        glyphs.extend(laid.glyphs.into_iter().map(|outline| (text.format.swatch, outline)));
    }
    (glyphs, backgrounds, missing)
}

/// One drawing: its art layers, bottom first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Drawing {
    pub art_layers: Vec<ArtLayer>,
    /// The boxes of text of a text layer's drawing (plan §4.19). Their glyphs are the vector
    /// art of its line art, made anew by [`Drawing::lay_out_texts`] when a box changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<TextBox>,
    /// Where gradients and pictures lie on its filled areas (plan §4.20), one at most an area.
    /// An area with none has its swatch fitted to it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placements: Vec<Placement>,
}

// Lets `Arc<Drawing>` compare by address first, so comparing two project snapshots only looks
// inside the drawings that differ.
impl Eq for Drawing {}

impl Default for Drawing {
    /// An empty drawing with indexed colour art under indexed line art, and indexed overart
    /// over that.
    fn default() -> Self {
        let layer = |kind| ArtLayer { kind, content: ArtContent::Indexed(IndexedSurface::new()) };
        Self { art_layers: ArtLayerKind::ALL.map(layer).to_vec(), texts: Vec::new(), placements: Vec::new() }
    }
}

impl Drawing {
    /// An empty drawing whose art layers of the kinds in `free` are free colour, those in
    /// `vector` vector art, and the others indexed.
    pub fn blank(free: &[ArtLayerKind], vector: &[ArtLayerKind]) -> Drawing {
        let mut drawing = Drawing::default();
        for layer in &mut drawing.art_layers {
            if vector.contains(&layer.kind) {
                layer.content = ContentMode::Vector.into();
            } else if free.contains(&layer.kind) {
                layer.content = ContentMode::Rgba.into();
            }
        }
        drawing
    }

    /// A drawing of a picture that was brought in: `rgba` is its `width` × `height` pixels,
    /// straight alpha, the top row first. The picture is the drawing's colour art, with its
    /// middle at the drawing's origin, and the line art and overart over it are empty, of free
    /// colour as bitmap art is, to be drawn on.
    pub fn of_picture(width: u32, height: u32, rgba: &[u8]) -> Drawing {
        Drawing::of_picture_at(-(width as i32 / 2), -(height as i32 / 2), width, height, rgba)
    }

    /// [`Drawing::of_picture`], with the picture's top left corner at `left`, `top` of the
    /// drawing: a part of a larger design, where it is in the design.
    pub fn of_picture_at(left: i32, top: i32, width: u32, height: u32, rgba: &[u8]) -> Drawing {
        let mut pixels = Surface::from_interleaved(photocraft_color::PixelFormat::RGBA8, Rect::new(left, top, left + width as i32, top + height as i32), rgba);
        // Tiles with nothing in them cost nothing.
        pixels.prune();
        Drawing {
            art_layers: vec![
                ArtLayer { kind: ArtLayerKind::ColourArt, content: ArtContent::Rgba(pixels) },
                ArtLayer { kind: ArtLayerKind::LineArt, content: ContentMode::Rgba.into() },
                ArtLayer { kind: ArtLayerKind::Overart, content: ContentMode::Rgba.into() },
            ],
            texts: Vec::new(),
            placements: Vec::new(),
        }
    }

    /// Its line art made anew from its boxes of text (plan §4.19): their glyphs as vector fills,
    /// each in its box's swatch. A drawing with no boxes is left as it is. Returns the fonts the
    /// machine lacks, which were laid out in Inter.
    pub fn lay_out_texts(&mut self, fonts: &fonts::Fonts) -> Vec<String> {
        let (glyphs, backgrounds, missing) = text_shapes(&self.texts, fonts);
        self.put_text_shapes(ArtLayerKind::LineArt, glyphs);
        self.put_text_shapes(ArtLayerKind::ColourArt, backgrounds);
        missing
    }

    /// The boxes of text `which` (all, without one) made plain vector art (plan §4.19): what
    /// they are drawn as is kept as the drawing's own shapes, no longer text, and the boxes go.
    /// Returns the fonts the machine lacks, as [`Drawing::lay_out_texts`] does.
    pub fn texts_to_shapes(&mut self, which: Option<usize>, fonts: &fonts::Fonts) -> Vec<String> {
        let picked: Vec<TextBox> = self.texts.iter().enumerate().filter(|(i, _)| which.is_none_or(|which| which == *i)).map(|(_, text)| text.clone()).collect();
        let (glyphs, backgrounds, mut missing) = text_shapes(&picked, fonts);
        for (kind, shapes) in [(ArtLayerKind::LineArt, glyphs), (ArtLayerKind::ColourArt, backgrounds)] {
            if let Some(ArtContent::Vector(art)) = self.layer_mut(kind).map(|layer| &mut layer.content) {
                let next = art.shapes().iter().map(|shape| shape.id.0).filter(|id| *id < TEXT_SHAPES).max().unwrap_or(0) + 1;
                let own = shapes.into_iter().enumerate().map(|(i, (swatch, outline))| VectorShape {
                    id: ShapeId(next + i as u32),
                    swatch,
                    kind: ShapeKind::Fill(VectorFill::plain(outline)),
                });
                *art = art.edited(|shapes| {
                    // Under the text's shapes, which are made anew below.
                    let at = shapes.iter().position(|shape| shape.id.0 >= TEXT_SHAPES).unwrap_or(shapes.len());
                    shapes.splice(at..at, own);
                });
            }
        }
        let mut index = 0;
        self.texts.retain(|_| {
            index += 1;
            which.is_some_and(|which| which != index - 1)
        });
        for font in self.lay_out_texts(fonts) {
            if !missing.contains(&font) {
                missing.push(font);
            }
        }
        missing
    }

    /// The shapes of the drawing's text on the art layer `kind`, its vector art, in place of
    /// those it had: the shapes of the drawing's own, below [`TEXT_SHAPES`], are kept, under
    /// them.
    fn put_text_shapes(&mut self, kind: ArtLayerKind, shapes: Vec<(SwatchId, kurbo::BezPath)>) {
        let text = shapes.into_iter().enumerate().map(|(i, (swatch, outline))| VectorShape {
            id: ShapeId(TEXT_SHAPES + i as u32),
            swatch,
            kind: ShapeKind::Fill(VectorFill::plain(outline)),
        });
        let text: Vec<VectorShape> = text.collect();
        match self.layer_mut(kind).map(|layer| &mut layer.content) {
            Some(ArtContent::Vector(art)) => {
                let own = art.shapes().iter().filter(|shape| shape.id.0 < TEXT_SHAPES).cloned();
                let shapes: Vec<VectorShape> = own.chain(text).collect();
                if shapes != art.shapes() {
                    *art = VectorArt::new(shapes);
                }
            }
            // Text is put only on vector art: a drawing whose art layer is pixels takes none.
            Some(_) => {}
            None if text.is_empty() => {}
            None => self.art_layers.push(ArtLayer { kind, content: ArtContent::Vector(VectorArt::new(text)) }),
        }
    }

    /// The shapes of its own vector art: those below the text's.
    pub fn own_shapes(&self, kind: ArtLayerKind) -> Vec<&VectorShape> {
        match self.layer(kind).map(|layer| &layer.content) {
            Some(ArtContent::Vector(art)) => art.shapes().iter().filter(|shape| shape.id.0 < TEXT_SHAPES).collect(),
            _ => Vec::new(),
        }
    }

    pub fn layer(&self, kind: ArtLayerKind) -> Option<&ArtLayer> {
        self.art_layers.iter().find(|l| l.kind == kind)
    }

    pub fn layer_mut(&mut self, kind: ArtLayerKind) -> Option<&mut ArtLayer> {
        self.art_layers.iter_mut().find(|l| l.kind == kind)
    }

    /// The drawing's art layers composited over transparency: the pixels of `rect` as
    /// straight-alpha RGBA8 in rows. `color` gives a swatch's straight-alpha sRGB colour.
    pub fn to_rgba8(&self, rect: Rect, color: impl Fn(SwatchId) -> [u8; 4]) -> Vec<[u8; 4]> {
        let mut out = vec![[0u8; 4]; rect.width() as usize * rect.height() as usize];
        for layer in &self.art_layers {
            let pixels = match &layer.content {
                ArtContent::Indexed(surface) => surface.to_rgba8(rect, &color),
                ArtContent::Vector(art) => art.raster().to_rgba8(rect, &color),
                ArtContent::Rgba(surface) => {
                    let mut pixels = vec![[0u8; 4]; out.len()];
                    surface.read_rgba8_into(rect, &mut pixels);
                    pixels
                }
            };
            for (under, over) in out.iter_mut().zip(pixels) {
                *under = over_straight(over, *under);
            }
        }
        out
    }

    /// The colour its free colour shows at the pixel (`x`, `y`): its art layers of free colour
    /// that `seen` lets in, each over those under it, made whole, as the colour that was
    /// painted there and not how thinly. `None` where none of them has paint there.
    pub fn free_colour_at(&self, x: i32, y: i32, seen: impl Fn(ArtLayerKind) -> bool) -> Option<[u8; 4]> {
        let mut out = [0u8; 4];
        for layer in self.art_layers.iter().filter(|layer| seen(layer.kind)) {
            if let ArtContent::Rgba(surface) = &layer.content {
                let mut found = [[0u8; 4]];
                surface.read_rgba8_into(Rect::new(x, y, x + 1, y + 1), &mut found);
                out = over_straight(found[0], out);
            }
        }
        (out[3] > 0).then_some([out[0], out[1], out[2], 255])
    }

    /// The same layers with nothing painted on them: what a drawing is before a project store
    /// has loaded its pixels.
    pub fn without_pixels(&self) -> Drawing {
        let empty = |layer: &ArtLayer| ArtLayer { kind: layer.kind, content: ContentMode::from(layer.content.clone()).into() };
        Drawing { art_layers: self.art_layers.iter().map(empty).collect(), texts: self.texts.clone(), placements: self.placements.clone() }
    }

    /// The swatches that anything on the drawing's indexed layers is painted with, or any
    /// shape of its vector art.
    pub fn swatches(&self) -> BTreeSet<SwatchId> {
        let swatches = |layer: &ArtLayer| match &layer.content {
            ArtContent::Indexed(surface) => surface.swatches(),
            ArtContent::Vector(art) => art.swatches(),
            ArtContent::Rgba(_) => BTreeSet::new(),
        };
        self.art_layers.iter().flat_map(swatches).collect()
    }

    /// Exact bounds of everything painted, over all art layers.
    pub fn bounds(&self) -> Rect {
        self.art_layers.iter().fold(Rect::EMPTY, |bounds, layer| bounds.union(&layer.content.bounds()))
    }
}

/// An empty plane of free colour (RGBA8, straight alpha).
pub fn free_colour() -> Surface {
    Surface::new(photocraft_color::PixelFormat::RGBA8)
}

/// `over` composited on `under`, both straight-alpha RGBA8.
fn over_straight(over: [u8; 4], under: [u8; 4]) -> [u8; 4] {
    let (a, b) = (f32::from(over[3]) / 255.0, f32::from(under[3]) / 255.0);
    let alpha = a + b * (1.0 - a);
    if alpha <= 0.0 {
        return [0; 4];
    }
    let channel = |i: usize| ((f32::from(over[i]) * a + f32::from(under[i]) * b * (1.0 - a)) / alpha + 0.5) as u8;
    [channel(0), channel(1), channel(2), (alpha * 255.0 + 0.5) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_free_colour_seen_at_a_pixel_is_its_art_layers_over_each_other_made_whole() {
        let mut drawing = Drawing::blank(&ArtLayerKind::ALL, &[]);
        let paint = |drawing: &mut Drawing, kind: ArtLayerKind, colour: [u8; 4]| {
            let Some(ArtContent::Rgba(surface)) = drawing.layer_mut(kind).map(|l| &mut l.content) else { panic!("free colour") };
            *surface = Surface::from_interleaved(photocraft_color::PixelFormat::RGBA8, Rect::new(0, 0, 1, 1), &colour);
        };
        paint(&mut drawing, ArtLayerKind::ColourArt, [0, 0, 255, 255]);
        paint(&mut drawing, ArtLayerKind::LineArt, [255, 0, 0, 128]);
        assert_eq!(drawing.free_colour_at(0, 0, |_| true), Some([128, 0, 127, 255]), "half red over blue");
        assert_eq!(drawing.free_colour_at(0, 0, |kind| kind == ArtLayerKind::LineArt), Some([255, 0, 0, 255]), "the thin line alone, whole");
        assert_eq!(drawing.free_colour_at(5, 5, |_| true), None);
    }

    #[test]
    fn a_new_drawing_has_colour_art_under_line_art() {
        let drawing = Drawing::default();
        assert_eq!(drawing.art_layers.iter().map(|l| l.kind).collect::<Vec<_>>(), ArtLayerKind::ALL);
        assert!(drawing.bounds().is_empty());
    }

    #[test]
    fn layers_composite_with_line_art_over_colour_art() {
        let mut drawing = Drawing::default();
        let mut paint = |kind, rect, coverage: &[f32], swatch| {
            let Some(ArtLayer { content: ArtContent::Indexed(surface), .. }) = drawing.layer_mut(kind) else { panic!("indexed") };
            surface.paint(rect, coverage, swatch);
        };
        paint(ArtLayerKind::ColourArt, Rect::new(0, 0, 3, 1), &[1.0, 1.0, 1.0], SwatchId(2));
        paint(ArtLayerKind::LineArt, Rect::new(1, 0, 3, 1), &[1.0, 0.5], SwatchId(1));
        let color = |swatch: SwatchId| if swatch == SwatchId(1) { [0, 0, 0, 255] } else { [200, 100, 0, 255] };
        let pixels = drawing.to_rgba8(Rect::new(0, 0, 4, 1), color);
        assert_eq!(pixels, [[200, 100, 0, 255], [0, 0, 0, 255], [100, 50, 0, 255], [0, 0, 0, 0]], "fill, line, half a line over the fill, nothing");
    }

    #[test]
    fn json_keeps_the_layers_and_their_modes_but_not_the_pixels() {
        let mut drawing = Drawing::default();
        let Some(ArtLayer { content: ArtContent::Indexed(line), .. }) = drawing.layer_mut(ArtLayerKind::LineArt) else { panic!("no line art") };
        line.paint(Rect::new(10, 20, 12, 21), &[1.0, 1.0], SwatchId(1));
        assert_eq!(drawing.bounds(), Rect::new(10, 20, 12, 21));

        let json = serde_json::to_value(&drawing).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "artLayers": [{ "kind": "colourArt", "content": "indexed" }, { "kind": "lineArt", "content": "indexed" }, { "kind": "overart", "content": "indexed" }] })
        );
        let read: Drawing = serde_json::from_value(json).unwrap();
        assert_eq!(read, Drawing::default());
        assert_eq!(drawing.without_pixels(), read);
    }

    #[test]
    fn a_reach_takes_one_art_layer_or_it_and_those_under_it_and_is_written_as_the_engine_reads_it() {
        use ArtLayerKind::{ColourArt, LineArt, Overart};
        let takes = |reach: ArtReach| ArtLayerKind::ALL.map(|kind| reach.takes(kind));
        assert_eq!(takes(ArtReach::Down(LineArt)), [true, true, false], "the lines and the fills under them");
        assert_eq!((takes(ArtReach::Down(Overart)), takes(ArtReach::Down(ColourArt))), ([true; 3], [true, false, false]));
        assert_eq!((takes(ArtReach::Only(Overart)), takes(ArtReach::All)), ([false, false, true], [true; 3]));
        assert_eq!((ArtReach::Down(ColourArt).only(), ArtReach::Down(LineArt).only()), (Some(ColourArt), None));
        for (reach, json) in [
            (ArtReach::All, serde_json::json!(null)),
            (ArtReach::Only(LineArt), serde_json::json!("lineArt")),
            (ArtReach::Down(Overart), serde_json::json!({ "down": "overart" })),
        ] {
            assert_eq!((serde_json::to_value(reach).unwrap(), serde_json::from_value::<ArtReach>(json.clone()).unwrap()), (json, reach));
        }
    }
}
