//! The drawing viewport: shows the current frame of the shot and turns pen input into strokes.
//!
//! What is shown is what the shot's graph evaluates to on the frame (`iskra-eval`): the layers
//! that reach the display, each where its pegs put it.
//!
//! Drawings are shown tile by tile as egui textures, coloured from the palette on the CPU. A
//! texture is made once per tile and palette, so a stroke re-uploads only the tiles it touches.
//! Textures are kept after the frame they were drawn on, up to a budget, so that playing a
//! shot round doesn't make them again each time; and while it plays, those of the frames just
//! ahead are made a few at a time before the playhead gets there.
//! The GPU compositor (plan §4.8: id + coverage textures with the palette lookup in the
//! shader) replaces this later.
//!
//! A layer under pegs is shown where they put it, and the pen is taken back through them, so
//! a stroke lands on the drawing where it is seen. With several pegs in hand, the others go
//! along with the one that is dragged: moved as far, and turned and scaled about its pivot.
//! The Transform tool moves and turns a peg,
//! scales it by the handles of a box round what it carries, and shows the path an animated peg takes: a dot a frame, a square a keyframe. A square drags
//! to move that keyframe, and the handles of the keyframe under the playhead bend the path.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use egui::epaint::Vertex;
use egui::{Color32, Key, PointerButton, Pos2, Sense, Shape, Stroke as Outline, TextureHandle, TextureOptions};
use iskra_drawing::{Affine, BrushSettings, IndexedSurface, LiveRgba, LiveStroke, Point, Rect as PixelRect, Stroke, StrokePoint, Surface};
use iskra_engine::Session;
use iskra_model::{
    ArtContent, ArtLayerKind, Bend, GuideId, GuideKind, Layer, LayerId, Palette, Peg, PegId, Projection, Shape as Drawn, Shot, StyleId, SwatchId, Transform,
    Warp, Work,
};
use serde_json::{Value, json};

use crate::onion::{Display, LIGHT_TABLE_OPACITY, Motion, Shift, ghosts};
use crate::select::{self, Drag, Handle, Selection};
use crate::stylus::PenSample;
use crate::theme::{Role, Theme};
use crate::view::View;

/// A drawing tile's side in pixels.
const TILE: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Brush,
    Eraser,
    /// The paint bucket: fills an area of the colour art, out to the lines (plan §4.8).
    Fill,
    /// Takes the swatch that the drawing is painted with under the pointer.
    Eyedropper,
    /// Draws a line, a rectangle or an ellipse with the brush in hand.
    Shape,
    Select,
    /// Cuts lines along the edge of an outline or a box, and takes away what is inside.
    Cutter,
    /// Moves and turns a peg, with what is under it.
    Transform,
    /// Moves and turns the peg that moves the shot's camera: the Transform tool, on the camera.
    Camera,
    /// Lays a chain of bones on a layer's drawing, and poses it (plan §4.5).
    Bones,
    /// Drags the points and handles of vector art's shapes (plan §4.16).
    Points,
    /// Puts boxes of text on a text layer and types in them (plan §4.19).
    Text,
    /// Shows where a gradient or a picture lies on a filled area, and moves it (plan §4.20).
    Gradient,
    /// Zooms the view: a drag right or down zooms in, a click steps in and with Alt out, and a
    /// box drawn with Shift is zoomed to fill the viewport.
    Zoom,
}

/// How much a notch of a mouse wheel zooms.
const WHEEL_ZOOM: f32 = 1.2;

/// How much a drag that zooms zooms a point of the screen it goes right or down: left or up,
/// it zooms out as much.
const DRAG_ZOOM: f32 = 0.01;

/// What a drag that zooms by `delta` on screen zooms by: in to the right and down.
fn drag_zoom(delta: egui::Vec2) -> f32 {
    (DRAG_ZOOM * (delta.x + delta.y)).exp()
}

/// How far a trackpad scrolled the view this frame, in screen points. A mouse wheel's scroll,
/// which is in lines, is not here: it zooms. Nor is one with Cmd held, which egui makes a zoom.
fn scrolled(input: &egui::InputState) -> egui::Vec2 {
    let mut by = egui::Vec2::ZERO;
    for event in &input.events {
        if let egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, modifiers, .. } = event
            && !modifiers.command
        {
            // With Shift, a scroll goes across, as egui has it elsewhere.
            by += if modifiers.shift { egui::vec2(delta.x + delta.y, 0.0) } else { *delta };
        }
    }
    by
}

/// How many notches a mouse wheel turned this frame: forward, away from the hand, more than 0.
fn wheeled(input: &egui::InputState) -> f32 {
    let mut notches = 0.0;
    for event in &input.events {
        if let egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, modifiers, .. } = event
            && !modifiers.command
        {
            notches += delta.x + delta.y;
        }
    }
    notches
}

/// The Zoom tool's press: where it began and where the pointer is now, on screen, whether
/// it has moved far enough to be a drag, and whether it draws a box.
#[derive(Clone, Copy, Debug)]
struct Zooming {
    from: Pos2,
    to: Pos2,
    moved: bool,
    boxing: bool,
}

/// How far from a peg's pivot its rotate handle sits, in screen points.
const PEG_HANDLE_OFFSET: f32 = 56.0;

/// How close to a selection handle a press counts as on it, in screen points.
pub(crate) const HANDLE_REACH: f32 = 9.0;

/// A press that moves less than this on screen, in points, is a click, not a drag.
pub(crate) const CLICK_SLOP: f32 = 3.0;

/// How far above the selection its rotate handle sits, in screen points.
pub(crate) const ROTATE_HANDLE_OFFSET: f32 = 24.0;
/// How far out from a corner of a selection the pointer turns it, in screen points.
const TURN_REACH: f32 = 26.0;
/// How near a side of a selection, between its handles, the pointer skews it.
const SKEW_REACH: f32 = 4.0;

/// What the pen does and where, as chosen in the toolbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tools {
    pub tool: Tool,
    pub swatch: SwatchId,
    /// A colour of its own to paint free colour with, where it is not the swatch's (sRGB,
    /// straight alpha).
    pub free: Option<[u8; 4]>,
    /// Which art layer of a drawing the brush and the eraser work on (plan §4.2).
    pub art: ArtLayerKind,
    /// The paint bucket closes gaps in the lines up to this wide, in scene units.
    pub gap: f32,
    /// The paint bucket takes paint away.
    pub unpaint: bool,
    /// The Select tool takes what it selects from the art layer in hand alone, and not from
    /// those under it too.
    pub select_one: bool,
    /// The Select tool draws a lasso around what it selects, and drags out no rectangle.
    pub lasso: bool,
    /// What the Select tool does to a selection, and the Cutter's cut, is done to every drawing
    /// of the layer.
    pub all_drawings: bool,
    /// The Cutter tool cuts along a box it drags out, and draws no outline.
    pub cut_box: bool,
    /// How the Cutter tool finishes the ends of vector lines it cuts.
    pub cut_cap: iskra_model::Cap,
    /// How the Brush lays its strokes: over what is drawn, behind it, or on it alone.
    pub paint_mode: iskra_drawing::PaintMode,
    /// On vector art, a stroke of the Brush is merged with the shapes of its swatch it overlaps.
    pub auto_flatten: bool,
    /// On vector art, the Brush leaves surfaces, areas with no middle line merged with those of
    /// their swatch, as paint does, rather than lines with a width.
    pub surface: bool,
    /// The Select tool's box or outline, and the Cutter's, takes what is under it on every
    /// layer that is seen and can be drawn on, not only the one in hand.
    pub all_layers: bool,
    /// The Select tool's click on vector art takes the piece of a line between the crossings
    /// either side of it, not the whole drawing.
    pub pieces: bool,
    /// The Select tool's click on vector art takes the one line or fill under it whole, not the
    /// whole drawing. Not with `pieces`.
    pub shapes: bool,
    /// The brush is the mixer brush, on free colour, and how it mixes.
    pub mixing: Mixing,
    /// The paint bucket gives the line under it the swatch, on the line art, and fills no area.
    pub fill_lines: bool,
    /// Which drawings of the layer the paint bucket fills the same place on.
    pub fill_on: FillOn,
    /// The frames picked on the timeline row of the layer in hand, first and last: what the
    /// paint bucket fills on with [`FillOn::Picked`].
    pub picked_frames: Option<[u32; 2]>,
    /// What the Shape tool draws.
    pub shape: Drawn,
    /// How it draws it: from the middle, filled, a rectangle's corners rounded (in scene
    /// units). Whether it is evened is Shift's, while it is dragged out.
    pub form: iskra_model::Form,
    pub frame: u32,
    /// What the Bones tool lays on a layer that has no deformer.
    pub lays: Lays,
    /// The Bones tool adds to a chain or a curve the layer has already: a drag off it pulls a
    /// new bone, or point, out of its end. Off, a press off it is a click, which selects the
    /// drawing under it. Until it is switched off.
    pub add_bones: bool,
    /// The Transform tool's box scales both ways alike, whichever handle is dragged. Shift,
    /// while one is dragged, is the other way about.
    pub uniform: bool,
    /// The Transform tool moves the peg's pivot, and leaves what the peg carries where it is.
    pub pivot: bool,
    /// The Transform tool turns and scales about a pivot of its own, dragged where it is
    /// wanted, and leaves the peg's pivot as it is.
    pub temp_pivot: bool,
    /// A click of the Transform tool on a drawing selects the nearest peg above it, and not
    /// its layer; a second click there goes one peg up.
    pub peg_select: bool,
    /// The Transform tool's side handles keep the area: one way grows as the other shrinks.
    pub squash: bool,
}

/// The mixer brush (plan §4.8): whether the brush is one, on free colour, and its settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mixing {
    pub on: bool,
    /// How much of what is painted under the brush it picks up, 0 to 1.
    pub wet: f32,
    /// How much paint a stroke has to lay down before the brush runs dry, 0 to 1: 1 never does.
    pub load: f32,
    /// How much of what it lays is what it picked up and not what it was loaded with, 0 to 1.
    pub mix: f32,
    pub flow: f32,
    /// Each stroke begins with the brush loaded anew with the colour in hand.
    pub reload: bool,
    /// Each stroke begins with nothing picked up.
    pub clean: bool,
}

impl Default for Mixing {
    fn default() -> Self {
        Self { on: false, wet: 0.5, load: 0.5, mix: 0.5, flow: 1.0, reload: true, clean: true }
    }
}

impl Mixing {
    fn settings(&self) -> iskra_model::MixerSettings {
        iskra_model::MixerSettings { wet: self.wet, load: self.load, mix: self.mix, flow: self.flow, ..Default::default() }
    }
}

impl Tools {
    /// Whether bones are laid and edited at rest, the drawings shown as drawn: with the Bones
    /// tool. The Transform tool poses them.
    pub fn rest(&self) -> bool {
        self.tool == Tool::Bones
    }

    /// The art layers the Select tool works on: the one in hand, alone or with those under it,
    /// or all of them where it takes what is on every layer seen.
    pub fn selected_art(&self) -> iskra_model::ArtReach {
        use iskra_model::ArtReach;
        match (self.all_layers, self.select_one) {
            (true, _) => ArtReach::All,
            (false, true) => ArtReach::Only(self.art),
            (false, false) => ArtReach::Down(self.art),
        }
    }
}

/// Which drawings of the layer the paint bucket fills the same place on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillOn {
    /// The drawing that is shown.
    #[default]
    Drawing,
    /// The drawings exposed on the frames picked on the layer's timeline row.
    Picked,
    /// Every drawing of the layer.
    Every,
}

impl FillOn {
    pub const ALL: [FillOn; 3] = [FillOn::Drawing, FillOn::Picked, FillOn::Every];

    pub fn name(self) -> &'static str {
        match self {
            FillOn::Drawing => "This Drawing",
            FillOn::Picked => "Picked Frames",
            FillOn::Every => "Every Drawing",
        }
    }
}

/// The kinds of deformer the Bones tool lays (plan §4.5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lays {
    #[default]
    Bones,
    Curve,
    Envelope,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            // The app starts with the Select tool, so that a first click picks and paints nothing.
            tool: Tool::Select,
            swatch: SwatchId(1),
            free: None,
            art: ArtLayerKind::LineArt,
            gap: 0.0,
            unpaint: false,
            select_one: true,
            lasso: false,
            all_drawings: false,
            cut_box: false,
            cut_cap: iskra_model::Cap::Round,
            paint_mode: iskra_drawing::PaintMode::Over,
            auto_flatten: false,
            surface: false,
            all_layers: false,
            pieces: false,
            shapes: false,
            mixing: Mixing::default(),
            fill_lines: false,
            fill_on: FillOn::Drawing,
            picked_frames: None,
            shape: Drawn::Line,
            form: iskra_model::Form::default(),
            frame: 0,
            lays: Lays::Bones,
            add_bones: false,
            uniform: true,
            pivot: false,
            temp_pivot: false,
            peg_select: false,
            squash: false,
        }
    }
}

/// The palette in the colours of one of its styles (plan §4.8). It is kept for as long as the
/// palette and the style stay the same, so that what is made from it is made once.
#[derive(Default)]
pub(crate) struct Styled {
    of: Option<(Arc<Palette>, Vec<StyleId>, Arc<Palette>)>,
    /// A palette with one swatch in a colour that is being picked: which and what colour,
    /// the palette it was made from, and the one made.
    picking: Option<((SwatchId, [u8; 4]), Arc<Palette>, Arc<Palette>)>,
}

impl Styled {
    /// `palette` as `styles` show it; the palette itself where it has none of them.
    pub(crate) fn of(&mut self, palette: &Arc<Palette>, styles: &[StyleId]) -> Arc<Palette> {
        if !styles.iter().any(|style| palette.style(*style).is_some()) {
            return palette.clone();
        }
        match &self.of {
            Some((of, shown, made)) if Arc::ptr_eq(of, palette) && shown == styles => made.clone(),
            _ => {
                let made = Arc::new(palette.styled(styles));
                self.of = Some((palette.clone(), styles.to_vec(), made.clone()));
                made
            }
        }
    }

    /// `shown`, the palette as a shot shows it, with the swatch that is being `picked` in
    /// the colour it has so far; made anew only when that colour changes.
    pub(crate) fn picking(&mut self, shown: Arc<Palette>, picked: Option<(SwatchId, [u8; 4])>) -> Arc<Palette> {
        let Some(picked) = picked.filter(|(id, color)| shown.swatch(*id).is_some_and(|swatch| swatch.color != *color)) else {
            self.picking = None;
            return shown;
        };
        match &self.picking {
            Some((was, of, made)) if *was == picked && Arc::ptr_eq(of, &shown) => made.clone(),
            _ => {
                let mut made = Palette::clone(&shown);
                for swatch in made.swatches.iter_mut().filter(|swatch| swatch.id == picked.0) {
                    swatch.color = picked.1;
                }
                let made = Arc::new(made);
                self.picking = Some((picked, shown, made.clone()));
                made
            }
        }
    }
}

/// The art layer a stroke is drawn on, as it was or as the stroke has it so far: indexed, or
/// free colour.
#[derive(Clone)]
enum Art {
    Indexed(IndexedSurface),
    Rgba(Surface),
}

impl Art {
    fn pixels(&self) -> Pixels<'_> {
        match self {
            Art::Indexed(surface) => Pixels::Indexed(surface),
            Art::Rgba(surface) => Pixels::Rgba(surface),
        }
    }

    /// The art layer with `coverage` (0 to 1 for each pixel of `rect`, in rows) painted on
    /// it: with `swatch` on indexed art, leaving the `protected` swatches; in the colour
    /// `ink` on free colour. The inside of a shape that is filled.
    fn painted(&self, rect: iskra_model::Rect, coverage: &[f32], swatch: SwatchId, ink: [u8; 4], protected: &[SwatchId]) -> Art {
        let mut art = self.clone();
        match &mut art {
            Art::Indexed(surface) => surface.paint_protecting(rect, coverage, swatch, protected),
            Art::Rgba(surface) => iskra_drawing::fill_rgba(surface, rect, coverage, ink),
        }
        art
    }
}

/// A stroke in progress on an art layer of either kind.
enum Live {
    Indexed(LiveStroke),
    Rgba(LiveRgba),
    Mixer(LiveMixer),
    /// A vector stroke, seen over the pixels its vector art is drawn as (plan §4.16).
    Vector(iskra_drawing::LiveVector),
}

/// A stroke of the mixer brush in progress on free colour. What the brush lays depends on
/// all it has gone over, so the stroke so far is painted anew as it grows.
struct LiveMixer {
    before: Surface,
    layer: Surface,
    stroke: Stroke,
    settings: iskra_model::MixerSettings,
    /// What the brush carried when the stroke began.
    carried: iskra_model::MixerState,
}

impl Live {
    /// Start a stroke of `brush` on `before`: with `swatch` on indexed art, leaving the
    /// `protected` swatches; in the colour `ink` on free colour.
    fn start(before: &Art, brush: &BrushSettings, swatch: SwatchId, ink: [u8; 4], protected: &[SwatchId]) -> Live {
        Self::start_as(before, brush, swatch, ink, protected, iskra_drawing::PaintMode::Over)
    }

    /// [`Live::start`], the stroke laid in `mode`: over what is painted, under it or on it alone.
    fn start_as(before: &Art, brush: &BrushSettings, swatch: SwatchId, ink: [u8; 4], protected: &[SwatchId], mode: iskra_drawing::PaintMode) -> Live {
        match before {
            Art::Indexed(surface) => Live::Indexed(LiveStroke::new(surface, brush, swatch).protecting(protected.to_vec()).in_mode(mode)),
            Art::Rgba(surface) => Live::Rgba(LiveRgba::new_as(surface, &iskra_drawing::brush_in(brush, ink), mode)),
        }
    }

    /// Start a stroke of the mixer brush on free colour: with these `settings`, and the
    /// brush carrying what `carried` says.
    fn mixing(before: &Surface, brush: &BrushSettings, settings: iskra_model::MixerSettings, carried: iskra_model::MixerState) -> Live {
        let stroke = Stroke { brush: brush.clone(), points: Vec::new() };
        Live::Mixer(LiveMixer { before: before.clone(), layer: before.clone(), stroke, settings, carried })
    }

    fn push(&mut self, points: &[StrokePoint]) {
        let _timer = crate::timings::timer("viewport: stroke painted so far");
        match self {
            Live::Indexed(live) => _ = live.push(points),
            Live::Rgba(live) => _ = live.push(points),
            Live::Mixer(live) => {
                live.stroke.points.extend_from_slice(points);
                live.layer = live.before.clone();
                iskra_drawing::mix_rgba(&mut live.layer, &live.stroke, &live.settings, &mut live.carried.clone());
            }
            Live::Vector(live) => _ = live.push(points),
        }
    }

    /// End the stroke as painted, for the engine to take in place of painting it again (see
    /// [`iskra_engine::Painted`]): with the art layer before and after it. The mixer brush and
    /// vector art are painted by the engine.
    fn finish(
        self,
        layer: LayerId,
        frame: u32,
        art_layer: ArtLayerKind,
        protect: &[SwatchId],
        mode: iskra_drawing::PaintMode,
        swatch: SwatchId,
    ) -> Option<iskra_engine::Painted> {
        let _timer = crate::timings::timer("viewport: stroke finished");
        let painted = |brush: &BrushSettings, points: &[StrokePoint], before, after| iskra_engine::Painted {
            layer: layer.0,
            frame,
            art_layer,
            brush: brush.clone(),
            points: points.to_vec(),
            swatch,
            protect: protect.to_vec(),
            mode,
            before,
            after,
        };
        match self {
            Live::Indexed(live) => {
                let (swatch, mode) = live.ink();
                let before = ArtContent::Indexed(live.before().clone());
                let (brush, points, protect) = (live.brush().clone(), live.points().to_vec(), live.protected().to_vec());
                Some(iskra_engine::Painted { swatch, mode, protect, ..painted(&brush, &points, before, ArtContent::Indexed(live.finish())) })
            }
            Live::Rgba(live) => {
                let before = ArtContent::Rgba(live.before().clone());
                let (brush, points) = (live.brush().clone(), live.points().to_vec());
                Some(painted(&brush, &points, before, ArtContent::Rgba(live.finish())))
            }
            Live::Mixer(_) | Live::Vector(_) => None,
        }
    }

    /// The art layer with the stroke so far.
    fn art(&self) -> Art {
        match self {
            Live::Indexed(live) => Art::Indexed(live.layer().clone()),
            Live::Rgba(live) => Art::Rgba(live.layer().clone()),
            Live::Mixer(live) => Art::Rgba(live.layer.clone()),
            Live::Vector(live) => Art::Indexed(live.layer().clone()),
        }
    }
}

/// A polyline being clicked out, corner by corner. The engine gets it as one `drawing.shape`
/// command when it is finished.
struct Polyline {
    layer: LayerId,
    frame: u32,
    art: ArtLayerKind,
    swatch: SwatchId,
    brush: BrushSettings,
    /// The art layer as it was, to draw the polyline so far on anew.
    before: Art,
    /// The corners that were clicked, in the drawing's pixels.
    corners: Vec<Point>,
    ink: [u8; 4],
    free: Option<[u8; 4]>,
}

/// From a layer's drawing to the screen and back: through the pegs that place it, the bend its
/// deformer gives it, and the view.
pub(crate) struct DrawingSpace {
    bend: Option<Warp>,
    to_scene: Affine,
    to_pixels: Affine,
    view: View,
    center: Pos2,
}

impl DrawingSpace {
    pub(crate) fn to_screen(&self, at: iskra_model::kurbo::Point) -> Pos2 {
        let at = Point::new(at.x, at.y);
        let bent = self.bend.as_ref().map_or(at, |warp| warp.apply(at));
        let seen = self.to_scene.apply(bent);
        self.view.to_screen(self.center, [seen.x, seen.y])
    }

    pub(crate) fn to_drawing(&self, pos: Pos2) -> iskra_model::kurbo::Point {
        let [x, y] = self.view.to_scene(self.center, pos);
        let at = self.to_pixels.apply(Point::new(x, y));
        let at = self.bend.as_ref().map_or(at, |warp| warp.undo(at));
        iskra_model::kurbo::Point::new(at.x, at.y)
    }
}

/// Whether what is drawn on `layer` with the tools in hand is vector art: the art layer in hand
/// of the drawing on the frame is, or with no drawing there, the layer makes it so.
/// What bends `layer`'s drawings on `frame`: its own bones, with `own`, and a chain above that
/// bends it through its peg. `None` where nothing moves them.
fn warp_of(shot: &Shot, layer: &Layer, frame: u32, own: bool) -> Option<Warp> {
    let warp = match (own, layer.deformer) {
        (false, Some(_)) => shot.warp_of(&Layer { deformer: None, ..layer.clone() }, frame),
        _ => shot.warp_of(layer, frame),
    };
    warp.filter(|warp| !warp.is_still())
}

/// How near a line, on screen, in points, a click of the Select tool with *Pieces* takes it.
const PIECE_REACH: f32 = 6.0;

/// How far from either end of a piece, in the drawing's pixels, its selection stops, short of
/// the lines that cross there; and how far out from its middle line it reaches.
const PIECE_TRIM: f64 = 4.0;
const PIECE_WIDTH: f64 = 2.0;

/// How large a tool's icon is drawn as the pointer, in points.
const POINTER_SIZE: f32 = 22.0;

/// The icon a tool's pointer is, and its point that acts, in the icon's 24 units: the
/// eyedropper's tip, the drop the bucket pours, the middle of the others.
fn tool_pointer(tool: Tool) -> Option<(&'static str, [f32; 2])> {
    match tool {
        Tool::Cutter => Some(("scissors", [12.0, 12.0])),
        Tool::Fill => Some(("paint-bucket", [20.0, 20.0])),
        Tool::Eyedropper => Some(("eyedropper", [4.0, 20.0])),
        Tool::Camera => Some(("camera", [12.0, 12.0])),
        Tool::Bones => Some(("bones", [12.0, 12.0])),
        _ => None,
    }
}

/// One cut, or a cut of several layers' drawings as one step.
fn cut_all(id: &'static str, mut cuts: Vec<Value>) -> (&'static str, Value) {
    if cuts.len() == 1 {
        return (id, cuts.remove(0));
    }
    let commands: Vec<Value> = cuts.into_iter().map(|params| json!({ "id": id, "params": params })).collect();
    ("edit.batch", json!({ "commands": commands, "title": "Cut" }))
}

pub(crate) fn draws_vector(layer: &Layer, tools: &Tools) -> bool {
    let content = layer.drawing_at(tools.frame).and_then(|d| d.drawing.layer(tools.art)).map(|l| &l.content);
    content.map_or(layer.vector.contains(&tools.art), ArtContent::is_vector)
}

/// A stroke being drawn. The engine gets it as one `drawing.stroke` command when the pen lifts.
struct ActiveStroke {
    layer: LayerId,
    frame: u32,
    art: ArtLayerKind,
    swatch: SwatchId,
    brush: BrushSettings,
    points: Vec<StrokePoint>,
    live: Live,
    /// The swatch's colour as the shot shows it, which a stroke on free colour is painted in.
    ink: [u8; 4],
    /// The colour of its own that the stroke is painted in on free colour, if it has one.
    free: Option<[u8; 4]>,
    /// For a stroke of the mixer brush: its settings, and what the brush carried at its start.
    mixer: Option<(iskra_model::MixerSettings, iskra_model::MixerState)>,
    /// For a stroke on vector art: the vector brush it is drawn with.
    vector: Option<iskra_drawing::VectorBrush>,
    /// The guide the stroke is held to, if one is in hand.
    snap: Option<iskra_model::Snap>,
    /// For a shape: which, its two points so far in the drawing's pixels, how it is drawn
    /// between them, and the art layer as it was, to draw the shape on anew as it is dragged.
    shape: Option<(Drawn, [Point; 2], iskra_model::Form, Art)>,
    /// It is drawn with the right button, and erases.
    rubbing: bool,
    /// Laid over what is drawn, behind it, or on it alone.
    mode: iskra_drawing::PaintMode,
    /// On vector art, merged with the shapes of its swatch it overlaps.
    flatten: bool,
    /// On vector art, left as a surface.
    surface: bool,
}

/// How many frames ahead of the playhead are made for the cache on each frame of the screen,
/// at most.
const CACHED_A_FRAME: usize = 2;
/// How long making frames for the cache may take of one frame of the screen, in seconds:
/// once one that is made has taken this long, no other is made until the next. The window is
/// drawn on the same thread, and waits while a frame is made.
const CACHE_TIME: f64 = 0.008;
/// How long the shot and the window have to be left alone, in seconds, before the shot's
/// frames are made ahead of playing: not while it is drawn on, when every stroke would have
/// them all made again, and not while the pointer moves or a key is pressed, when the window
/// would answer late.
const SETTLE: f64 = 0.5;
/// How long after the playhead last moved the frames that are kept are still shown, in
/// seconds: while it is dragged along, and not once it rests.
const SCRUB: f64 = 0.15;

/// The place on screen of a place in a frame whose corners on screen are `frame`: top left,
/// top right, bottom right, bottom left. Across and down, each from 0 to 1.
fn in_frame(frame: [Pos2; 4], [across, down]: [f32; 2]) -> Pos2 {
    let (top, bottom) = (frame[0] + (frame[1] - frame[0]) * across, frame[3] + (frame[2] - frame[3]) * across);
    top + (bottom - top) * down
}

/// The lines of a guide over a frame whose corners on screen are `frame` (see [`in_frame`]).
fn guide_lines(guide: crate::prefs::Guide, frame: [Pos2; 4]) -> Vec<Vec<Pos2>> {
    let lines = iskra_export::burn::guide_lines(guide);
    lines.into_iter().map(|line| line.into_iter().map(|at| in_frame(frame, at)).collect()).collect()
}

/// The lines of a guide of the user's own over a frame (see [`guide_lines`]) whose aspect
/// ratio is `frame_aspect`.
fn own_guide_lines(kind: crate::prefs::GuideKind, frame: [Pos2; 4], frame_aspect: f64) -> Vec<Vec<Pos2>> {
    let lines = iskra_export::burn::own_guide_lines(kind, frame_aspect);
    lines.into_iter().map(|line| line.into_iter().map(|at| in_frame(frame, at)).collect()).collect()
}

/// What lies outside `frame` and inside `seen`, as one shape of `colour`: a ring between the
/// frame and the frame made as large as covers all that is seen.
fn outside(frame: [Pos2; 4], seen: egui::Rect, colour: Color32) -> Shape {
    let middle = frame.iter().fold(egui::Vec2::ZERO, |sum, at| sum + at.to_vec2()) / 4.0;
    let nearest = frame.iter().map(|at| (at.to_vec2() - middle).length()).fold(f32::INFINITY, f32::min).max(1.0);
    let far = (seen.min.to_vec2() - middle).length().max((seen.max.to_vec2() - middle).length()) + seen.size().length();
    let by = 1.0 + 3.0 * far / nearest;
    let mut mesh = egui::Mesh::default();
    for at in frame {
        mesh.colored_vertex(at, colour);
    }
    for at in frame {
        mesh.colored_vertex((middle + (at.to_vec2() - middle) * by).to_pos2(), colour);
    }
    for i in 0..4u32 {
        let next = (i + 1) % 4;
        mesh.add_triangle(i, next, 4 + next);
        mesh.add_triangle(i, 4 + next, 4 + i);
    }
    Shape::mesh(mesh)
}

/// The parts of a frame that an aspect mask darkens: the two bands outside the largest frame
/// of ratio `aspect` that fits in the middle. None if that is the frame itself.
fn mask_bands(aspect: f64, frame: [Pos2; 4], frame_aspect: f64) -> Vec<[Pos2; 4]> {
    let band = |[[x0, y0], [x1, y1]]: [[f32; 2]; 2]| [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|at| in_frame(frame, at));
    iskra_export::burn::mask_bands(aspect, frame_aspect).into_iter().map(band).collect()
}

/// From the space that `parent` gives what it carries, to what the viewport shows: the
/// scene, or with `flat` unset what the shot's camera sees of it on `frame`. `at` says at
/// which depth that space is seen: the one of the peg `at` and the pegs above it.
///
/// What moves the camera is shown flat whatever the view: seen through the camera it moves,
/// it would run from the pointer that drags it. So is what is behind the camera, which
/// the camera sees nowhere.
/// The onion skin of `layer` on `frame`, its ghosts placed and bent as the viewport has them:
/// what the Onion Skin panel lists.
pub(crate) fn ghosts_of(shot: &Shot, layer: &Layer, frame: u32, display: &Display) -> Vec<crate::onion::Ghost> {
    let placed = |frame: u32| space(shot, false, layer.parent, layer.parent, frame).m;
    let bent = |frame: u32| warp_of(shot, layer, frame, true);
    ghosts(layer, frame, display, &Motion { keys: Motion::keys_of(shot, layer), placed: &placed, bent: &bent })
}

pub(crate) fn space(shot: &Shot, flat: bool, parent: Option<PegId>, at: Option<PegId>, frame: u32) -> Affine {
    let world = shot.world(parent, frame);
    let carries_camera = at.is_some_and(|at| shot.ancestors(shot.camera.parent).any(|peg| peg.id == at));
    if flat || carries_camera {
        return world;
    }
    shot.seen(shot.depth(at, frame), frame).map_or(world, |seen| seen.mul(&world))
}

impl Viewport {
    /// `project` as its shots' skins show it ([`iskra_model::Project::as_skins_show`]), kept
    /// until the project changes.
    pub fn skins_show(&mut self, project: &Arc<iskra_model::Project>) -> Arc<iskra_model::Project> {
        match &self.skins_show {
            Some((of, shown)) if Arc::ptr_eq(of, project) => shown.clone(),
            _ => {
                let shown = project.as_skins_show();
                self.skins_show = Some((project.clone(), shown.clone()));
                shown
            }
        }
    }

    /// The emitter of the Particles node picked, which its rain, snow or wind come out of: a
    /// dashed orange outline, as the drawing being rigged has, where the camera sees it at the
    /// node's depth. An area is a box, a line a line and a point a cross. With a number of it
    /// being dragged in Properties, as it is dragged.
    fn paint_emitter(&mut self, painter: &egui::Painter, shot: &Shot, frame: u32) {
        #[cfg(test)]
        self.emitter_outline.clear();
        let Some(effect) = self.emitter.and_then(|id| shot.effect(id)).filter(|effect| effect.kind == iskra_model::PARTICLES) else { return };
        let number = |id: &str| match &self.live_effect {
            Some((of, param, iskra_model::Value::Number(value))) if *of == effect.id && param == id => *value,
            _ => effect.number(id, frame),
        };
        if !matches!(effect.choice("kind", frame).as_str(), "Rain" | "Snow" | "Wind") {
            return;
        }
        let (half_width, half_height) = match effect.choice("emitter", frame).as_str() {
            "Area" => (number("emitterWidth") / 2.0, number("emitterHeight") / 2.0),
            "Line" => (number("emitterWidth") / 2.0, 0.0),
            "Point" => (0.0, 0.0),
            _ => return,
        };
        // Placed on the frame at the node's depth as the camera saw it on the first frame, and
        // seen now as the viewport shows that depth.
        let depth = number(iskra_model::DEPTH);
        let Some(home) = shot.seen(depth, 0).and_then(|seen| seen.inverse()) else { return };
        let now = if self.flat { Some(Affine::IDENTITY) } else { shot.seen(depth, frame) };
        let Some(now) = now else { return };
        let center = painter.clip_rect().center();
        let (x, y) = (number("emitterX"), number("emitterY"));
        let screen = |dx: f64, dy: f64| {
            let seen = now.apply(home.apply(Point::new(x + dx, y + dy)));
            self.view.to_screen(center, [seen.x, seen.y])
        };
        let colour = crate::theme::Theme::of(painter.ctx()).colour(crate::theme::Role::Rigged);
        // Mutable for the tests, which keep where it was drawn.
        #[cfg_attr(not(test), allow(unused_mut))]
        let mut draw = |line: Vec<Pos2>| {
            painter.extend(Shape::dashed_line(&line, Outline::new(1.8, colour), 8.0, 5.0));
            #[cfg(test)]
            self.emitter_outline.extend(line);
        };
        match (half_width > 0.0, half_height > 0.0) {
            (_, true) => {
                let corners =
                    [screen(-half_width, -half_height), screen(half_width, -half_height), screen(half_width, half_height), screen(-half_width, half_height)];
                draw(vec![corners[0], corners[1], corners[2], corners[3], corners[0]]);
            }
            (true, false) => draw(vec![screen(-half_width, 0.0), screen(half_width, 0.0)]),
            (false, false) => {
                let at = screen(0.0, 0.0);
                draw(vec![at - egui::vec2(8.0, 0.0), at + egui::vec2(8.0, 0.0)]);
                draw(vec![at - egui::vec2(0.0, 8.0), at + egui::vec2(0.0, 8.0)]);
            }
        }
    }
}

/// The space a peg's own values are in, as the viewport shows it: its parent's, carried by
/// the bones it follows, and less what it does not inherit.
fn peg_space(shot: &Shot, flat: bool, peg: &Peg, frame: u32) -> Affine {
    let above = shot.above(peg, frame);
    let carries_camera = shot.ancestors(shot.camera.parent).any(|above| above.id == peg.id);
    if flat || carries_camera {
        return above;
    }
    shot.seen(shot.depth(Some(peg.id), frame), frame).map_or(above, |seen| seen.mul(&above))
}

/// Where a layer's drawings are in what the viewport shows: at the layer's resolution, under
/// its pegs, and unless the view is flat as the camera sees them. "Scene" below is that.
#[derive(Clone, Copy)]
struct Placement {
    /// From drawing pixels to scene units.
    to_scene: Affine,
    /// From scene units to drawing pixels. `None` where the pegs flatten the drawing to a line
    /// or a point (a scale of nothing): it can be seen, barely, and not drawn on.
    to_pixels: Option<Affine>,
}

impl Placement {
    fn of(shot: &Shot, layer: &Layer, frame: u32, flat: bool) -> Self {
        // Under a peg that bends it, a chain above carries it by bending it, not the peg.
        let world = match shot.bending(layer) {
            Some(_) => {
                let world = shot.layer_world(layer, frame);
                let seen = (!flat).then(|| shot.seen(shot.depth(layer.parent, frame), frame)).flatten();
                seen.map_or(world, |seen| seen.mul(&world))
            }
            None => space(shot, flat, layer.parent, layer.parent, frame),
        };
        let to_scene = world.mul(&Affine::scale(1.0 / layer.density));
        // Through the camera a tilted layer is seen in perspective, which no flat transform
        // takes the pen back through: it is drawn on in the flat view.
        let leant = !flat && shot.tilted(layer.parent, frame);
        Self { to_scene, to_pixels: to_scene.inverse().filter(|_| !leant) }
    }

    /// Scene units per drawing pixel, evened out over the directions.
    fn scale(&self) -> f64 {
        self.to_scene.determinant().abs().sqrt()
    }
}

/// What of a peg the Transform tool has hold of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// The peg itself, on the frame that is showing.
    Move,
    Rotate,
    /// The peg's pivot, which is moved under what the peg carries: that stays where it is.
    Pivot,
    /// The tool's temporary pivot, which only the tool turns and scales about.
    Spot,
    /// A handle of the box round what the peg carries: where the handle is, in the peg's own
    /// space, and which of the two ways it scales, a corner both and a side one.
    Scale {
        handle: [f64; 2],
        ways: [bool; 2],
    },
    /// A side of the box, clockwise from the top, dragged between its handles: it slides
    /// along itself, about the pivot, which skews what the peg carries. The box is in the
    /// peg's own space.
    Skew {
        side: usize,
        bounds: [f64; 4],
    },
    /// The position keyframe on this frame, by its square on the path.
    Key(u32),
    /// A handle of the path at the keyframe on this frame: the one the path leaves by, or the
    /// one it arrives by.
    Bend {
        frame: u32,
        out: bool,
    },
    /// An in-between's dot on the path, slid along it: the position keyframe the way starts
    /// on, and the in-between's frame. Only the timing changes, by the way's chart.
    Tween {
        key: u32,
        frame: u32,
    },
}

/// A corner of the camera's frame dragged with the Camera tool: the frame is made smaller or
/// larger, which zooms the camera in or out, and with it goes to another place.
#[derive(Clone, Copy)]
struct FrameDrag {
    /// Which corner: 0 the top left, then clockwise.
    corner: usize,
    /// How large the frame is against what it was, and where its middle is, in the units of
    /// the frame as it was.
    scale: f64,
    centre: [f64; 2],
}

/// A drag with the Transform tool. The engine gets one command when it ends.
#[derive(Clone, Copy)]
struct PegDrag {
    peg: PegId,
    grab: Grab,
    /// Where the press was, on screen, and where the pointer was a frame ago.
    start: Pos2,
    last: Pos2,
    /// The pointer has gone far enough from the press for this to be a drag, not a click.
    moved: bool,
    /// The peg's transform before the drag, and with the drag so far. For a keyframe's square,
    /// the position is that keyframe's.
    from: Transform,
    now: Transform,
    /// The path's handles at the keyframe, before the drag and with it so far.
    bend_from: Bend,
    bend: Bend,
    /// The keyframe is a corner: a handle pulled out of it leaves the other side straight.
    corner: bool,
    /// The temporary pivot that the peg is turned and scaled about in place of its own, in the
    /// peg's own space, and where it was as the drag began.
    about: Option<[f64; 2]>,
}

/// A transform with its pivot moved by `by`, in the peg's own space, and its position moved
/// back by what that would move what it carries, which is how far its turn and its scale
/// take the pivot's step from where the step itself goes.
fn pivot_moved(from: Transform, by: [f64; 2]) -> Transform {
    let turned = Transform { position: [0.0, 0.0], pivot: [0.0, 0.0], ..from }.matrix().apply(Point::new(by[0], by[1]));
    let pivot = [from.pivot[0] + by[0], from.pivot[1] + by[1]];
    Transform { pivot, position: [from.position[0] - by[0] + turned.x, from.position[1] - by[1] + turned.y], ..from }
}

/// A step as a transform takes it: what it does to the step from nowhere to `step`, without
/// what it moves everything by.
fn linear(by: &Affine, step: [f64; 2]) -> [f64; 2] {
    let (to, from) = (by.apply(Point::new(step[0], step[1])), by.apply(Point::new(0.0, 0.0)));
    [to.x - from.x, to.y - from.y]
}

/// The handles of the Transform tool's box, of its left, top, right and bottom: where each is,
/// and which of the two ways it scales. The corners, clockwise from the top left, then the
/// middles of the sides, from the top's.
/// A peg's turn, skew and scale that give it `linear`, the part of its matrix with no move in
/// it, kept as close to `was` as they can be: mirrored the way it was, and turned by the
/// nearest number of degrees to its turn, so that a key doesn't go a whole turn round.
fn turn_skew_scale(linear: &Affine, was: &Transform) -> (f64, f64, [f64; 2]) {
    let [a, b, c, d, ..] = linear.m;
    // The peg's matrix is its turn, then its skew, then its scale: its own x axis is only
    // turned and scaled.
    let mirrored = if was.scale[0] < 0.0 { -1.0 } else { 1.0 };
    let turn = (mirrored * b).atan2(mirrored * a);
    let (sin, cos) = turn.sin_cos();
    let across = mirrored * a.hypot(b);
    let down = cos * d - sin * c;
    let lean = if down.abs() > 1e-12 { (cos * c + sin * d) / down } else { 0.0 };
    let degrees = turn.to_degrees();
    let rotation = degrees + 360.0 * ((was.rotation - degrees) / 360.0).round();
    // As the box's handles keep it: never to nothing, nor laid flat on a line.
    let kept = |scale: f64| scale.abs().clamp(0.01, 100.0).copysign(scale);
    (rotation, lean.atan().to_degrees().clamp(-MAX_SKEW, MAX_SKEW), [kept(across), kept(down)])
}

/// The most a peg is skewed by the Transform tool's sides, in degrees, as the engine allows.
const MAX_SKEW: f64 = 85.0;

fn box_handles([x0, y0, x1, y1]: [f64; 4]) -> [([f64; 2], [bool; 2]); 8] {
    let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (corner, across, along) = ([true, true], [true, false], [false, true]);
    [
        ([x0, y0], corner),
        ([x1, y0], corner),
        ([x1, y1], corner),
        ([x0, y1], corner),
        ([mx, y0], along),
        ([x1, my], across),
        ([mx, y1], along),
        ([x0, my], across),
    ]
}

/// How many squares a side a tile of a layer seen in perspective is drawn in.
const TILTED_CELLS: usize = 8;

/// How many squares a side a tile of a layer that a deformer bends is drawn in.
const BENT_CELLS: usize = 16;

/// How many tile textures are kept: about 200 MB of them at four bytes a pixel. Past it, those
/// that were drawn longest ago go first.
const TEXTURE_BUDGET: usize = 768;
/// How many textures of the frames ahead are made on one frame of the screen while playing:
/// few enough not to hold up the frame that is showing.
const WARMED_A_FRAME: usize = 6;

/// The longest path that is drawn, in frames.
const PATH_FRAMES: u32 = 2000;

/// How much of a board shows where it is reference under the animation (plan §7.2c).
const BOARD_OPACITY: f32 = 0.4;

/// How a layer is drawn.
#[derive(Clone, Copy)]
struct Look {
    /// What the tile's texture is multiplied by: white for the drawing as it is, white with
    /// less alpha to fade it, or an onion skin tint.
    color: Color32,
    /// Draw the layer's coverage in `color` alone, ignoring its swatches (onion skin).
    silhouette: bool,
    /// Of a silhouette, only its edges: what the layer covers as an outline.
    outline: bool,
    shift: Shift,
}

impl Look {
    const PLAIN: Look = Look { color: Color32::WHITE, silhouette: false, outline: false, shift: Shift { offset: [0.0, 0.0], rotation: 0.0 } };

    fn faded(opacity: f32) -> Look {
        Look { color: Color32::from_white_alpha((opacity * 255.0) as u8), ..Look::PLAIN }
    }
}

/// The paint bucket's drag so far, filled on a copy of the art layer it is on.
struct FillLive {
    layer: LayerId,
    frame: u32,
    kind: ArtLayerKind,
    art: Art,
    /// The line art the fills stop at.
    lines: IndexedSurface,
    swatch: SwatchId,
    /// The colour free colour is filled with: none to take paint away.
    ink: [u8; 4],
    fill: iskra_drawing::Fill,
    /// On vector art, what fills it as the engine will, whose pixels `art` then is.
    vector: Option<VectorLive>,
}

/// The bucket dragged over vector art.
enum VectorLive {
    /// Filling the areas between the lines.
    Areas(iskra_drawing::VectorBucket),
    /// Giving the lines it goes over the swatch, on a copy of the drawing.
    Lines(Box<iskra_model::Drawing>),
}

impl FillLive {
    /// Fill the area at `at`, in the drawing's pixels, unless it has the fill's colour there.
    fn fill(&mut self, place: Point, protected: &[SwatchId]) {
        let at @ (x, y) = (place.x.floor() as i32, place.y.floor() as i32);
        let place = iskra_model::kurbo::Point::new(place.x, place.y);
        match &mut self.vector {
            Some(VectorLive::Areas(bucket)) => {
                if bucket.fill(place) {
                    self.art = Art::Indexed(bucket.art().raster().clone());
                }
                return;
            }
            Some(VectorLive::Lines(drawing)) => {
                if iskra_drawing::recolour_vector_line(drawing, self.kind, place, self.swatch, protected).unwrap_or(false)
                    && let Some(ArtContent::Vector(vector)) = drawing.layer(self.kind).map(|layer| &layer.content)
                {
                    self.art = Art::Indexed(vector.raster().clone());
                }
                return;
            }
            None => {}
        }
        match &mut self.art {
            Art::Indexed(art) if self.kind != ArtLayerKind::ColourArt => {
                if art.pixel(x, y).swatch != self.swatch {
                    _ = iskra_drawing::recolour_line(art, at, self.swatch, protected);
                }
            }
            Art::Indexed(art) => {
                let pixel = art.pixel(x, y);
                let has = if pixel.coverage >= 32768 { pixel.swatch } else { SwatchId::NONE };
                if has != self.swatch {
                    _ = iskra_drawing::fill_protecting(art, &self.lines, at, self.swatch, &self.fill, protected);
                }
            }
            Art::Rgba(art) => {
                let mut here = [[0u8; 4]];
                art.read_rgba8_into(PixelRect::new(x, y, x + 1, y + 1), &mut here);
                if here[0] != self.ink && !(here[0][3] == 0 && self.ink[3] == 0) {
                    _ = iskra_drawing::fill_free(art, &self.lines, at, self.ink, &self.fill);
                }
            }
        }
    }
}

/// A picture of what goes through an effect node: made on the graphics card and kept there
/// by the frame cache, or made by the reference compositor and sent as a picture of the
/// window's, where there is no card to make it on.
enum Made {
    Card(egui::TextureId),
    Window(TextureHandle),
}

impl Made {
    fn id(&self) -> egui::TextureId {
        match self {
            Made::Card(id) => *id,
            Made::Window(texture) => texture.id(),
        }
    }
}

/// The picture of what goes through one effect node (plan §8).
struct ThroughPicture {
    /// What it is a picture of. `None` for one of a shot that was being posed, which is made
    /// again every time.
    of: Option<ThroughOf>,
    texture: Made,
    /// The count of the frame of the screen it was last drawn on.
    used: u64,
}

/// What a picture of an effect is of: the shot and the palette as they were, held so that
/// their addresses can't be reused, the frame, and the size it was made at.
struct ThroughOf {
    shot: Arc<Shot>,
    palette: Arc<Palette>,
    frame: u32,
    size: (u32, u32),
    space: iskra_model::WorkingSpace,
}

impl PartialEq for ThroughOf {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shot, &other.shot)
            && Arc::ptr_eq(&self.palette, &other.palette)
            && (self.frame, self.size, self.space) == (other.frame, other.size, other.space)
    }
}

/// The texture of one tile under one palette. It holds the tile and the palette so their
/// addresses, which are the cache key, can't be reused while the entry lives.
struct TileTexture {
    texture: TextureHandle,
    /// The count of the frame of the screen it was last drawn on.
    used: u64,
    _tile: Arc<photocraft_raster::Tile>,
    _palette: Arc<Palette>,
    _areas: Option<Arc<iskra_model::Areas>>,
}

#[derive(Default)]
pub struct Viewport {
    pub view: View,
    /// The point editor's shape and picked points.
    pub points: crate::points::PointEditor,
    /// The Text tool's box being typed in, and the press it is making.
    pub text: crate::text_tool::TextTool,
    /// The Gradient/Texture tool's area shown, and the handle being dragged.
    pub placing: crate::placing::Placing,
    /// The shot to show. The app says which each frame.
    pub shot: Option<iskra_model::ShotId>,
    /// What the viewer said last of the display it looks through.
    #[cfg(test)]
    pub viewing_said: Option<String>,
    /// The display of the shot the viewer looks through, where it is not the main one (plan
    /// §8.6): the window's own, not the project's, and let go when another shot is shown. The
    /// Storyboard page shows the main one whatever this says.
    pub viewing: Option<iskra_model::DisplayId>,
    /// The shot as the display it looks through shows it: of which shot of the project's,
    /// through which display, and what was made.
    seen: Option<(Arc<Shot>, iskra_model::DisplayId, Arc<Shot>)>,
    /// Show the camera frame black: the storyboard's playhead is between shots.
    pub black: bool,
    /// Layers that are not shown: hidden on their rows of the storyboard timeline. The app
    /// says which.
    pub hidden: std::collections::BTreeSet<LayerId>,
    /// The layers that can't be drawn on here, locked on their rows: the Select tool's
    /// *Select All Drawings* passes them by.
    pub fenced: std::collections::BTreeSet<LayerId>,
    /// The layers whose onion skin shows: the layer in hand, those picked with it and those
    /// under a peg in hand or picked. The app says which (`onion::onion_layers`).
    pub onioned: std::collections::BTreeSet<LayerId>,
    /// The project as its shots' skins show it, made from the one it is of, which it is kept
    /// with until that changes.
    skins_show: Option<(Arc<iskra_model::Project>, Arc<iskra_model::Project>)>,
    /// With *Select All Drawings*, what the selection takes of the drawings of the other
    /// layers: each the same box or outline, in that drawing's pixels, with the drawing's
    /// pixels' place in the scene when it was made.
    pub(crate) also: Vec<(Selection, Affine)>,
    /// A picture to show over the camera's frame in place of the shot: the film inside a
    /// transition, two shots mixed (plan §7.2b). The app makes it and says so each frame.
    pub mixed: Option<egui::TextureId>,
    /// Whether the mixed picture was what was drawn.
    #[cfg(test)]
    pub mixed_shown: bool,
    /// Which of the shot is shown (plan §7.2c): the boards alone on the Storyboard page, all
    /// of it on the Animation page. The app says which.
    pub work: Work,
    /// Where all of the shot is shown, the boards are faded, as on a light table: they are
    /// what the animation is made after. The one that is drawn on is shown plain.
    pub fade_boards: bool,
    /// The layer in hand is not to be drawn on, and why: a board that is locked on the
    /// Animation page. The pen changes nothing of it.
    pub locked: Option<&'static str>,
    /// The shot as only `work` of it, kept while the shot is the same: of which shot, which
    /// work, and what was made.
    worked: Option<(Arc<Shot>, Work, Arc<Shot>)>,
    /// Show the scene flat, as if nothing had a depth and the camera were at home, with the
    /// frame the camera sees drawn over it. Unset, the viewport looks through the camera.
    pub flat: bool,
    /// Standing still, draw the shot on the graphics card, as one picture the compositor
    /// makes of the part that is seen, where the viewport otherwise draws each drawing's
    /// tiles itself (M13). Only where there is a card, and not in the flat view.
    pub on_card: bool,
    /// The shot was last drawn so. And while it is, nothing is painted from tiles.
    pub from_card: bool,
    /// Where in the window the viewport was last shown.
    #[cfg(test)]
    pub shown_in: Option<egui::Rect>,
    /// The art layers of the drawings that were painted, in the order they were.
    #[cfg(test)]
    pub painted_art: Vec<ArtLayerKind>,
    /// The view has been fitted to the camera frame once.
    fitted: bool,
    /// The palette in the style the shot is shown in.
    styled: Styled,
    /// The colour of the swatch in hand, as the shot shows it.
    ink: [u8; 4],
    /// The swatches that are protected (plan §4.8): what is painted with them is not painted
    /// over, erased or filled.
    pub protect: Vec<SwatchId>,
    /// What the mixer brush carries from one stroke to the next: the paint it is loaded with,
    /// and what it picked up. It is the window's, as the brush in hand is, and no undo
    /// changes it.
    pub carries: iskra_model::MixerState,
    /// The swatch the Eyedropper found under the pointer, for the app to take.
    pub picked: Option<SwatchId>,
    /// The colour the Eyedropper found on free colour, which has no swatch.
    pub picked_colour: Option<[u8; 4]>,
    /// A swatch whose colour is being picked in the Palette panel, and the colour so far, as
    /// the shot would show it: the drawings follow it.
    pub recolouring: Option<(SwatchId, [u8; 4])>,
    /// Where the paint bucket has been dragged since it came down: on the screen, and in the
    /// drawing's pixels. Every place of it is filled when it is let go.
    fill_drag: Vec<(Pos2, [f64; 2])>,
    /// The bucket's drag is by the eraser end of the pen, and takes paint away.
    fill_erasing: bool,
    /// What the bucket's drag has filled so far, as the engine will when it is let go: shown
    /// in place of the art layer while it is dragged.
    fill_live: Option<FillLive>,
    stroke: Option<ActiveStroke>,
    /// How a deformer bends the layer in hand on the frame that is showing, if one does: the
    /// pen is taken back through it, so that a stroke lands on the drawing where it is seen.
    bend: Option<Warp>,
    /// The stroke that was finished on this frame of the screen: its layer, frame and art
    /// layer, with the art layer as the stroke left it. The project has the stroke only once
    /// the frame is drawn, so for that one frame it is shown from here, and does not go and
    /// come back.
    landed: Option<(LayerId, u32, ArtLayerKind, Art)>,
    /// The stroke that was finished on this frame, as painted while it was drawn: offered to
    /// the engine with the command, which then need not paint it again.
    pub(crate) painted: Option<iskra_engine::Painted>,
    /// The selection's drag that was let go on this frame of the screen: the project has its
    /// move only once the frame is drawn, so for that one frame the drawing is shown from its
    /// preview, and does not go back to where it was and come again.
    dropped: Option<Drag>,
    /// A flip or a turn of the selection asked for by its buttons, in the drawing's pixels: done
    /// as a drag of it is, on the next frame of the screen.
    pub(crate) operation: Option<Affine>,
    /// The Select tool's selection, on the current layer and frame.
    pub selection: Option<Selection>,
    /// The drawing guide in hand (plan §5, M7): strokes are held to it.
    pub guide: Option<GuideId>,
    /// The guide in hand as the shot has it, or as a drag of one of its handles has it so far.
    held_guide: Option<GuideKind>,
    /// A handle of the guide in hand being dragged: which, and the guide as it is so far.
    guide_drag: Option<(GuideId, usize, GuideKind)>,
    /// A selection rectangle being dragged out, from and to, in drawing pixels.
    marquee: Option<(Point, Point)>,
    /// Where on screen a drag that zooms began: it zooms about there.
    zoom_from: Option<Pos2>,
    /// The Zoom tool's press, while the pointer is down.
    zooming: Option<Zooming>,
    /// A polyline whose corners are being clicked, until it is finished.
    polyline: Option<Polyline>,
    /// The outline of a lasso so far, while it is drawn, in the drawing's pixels.
    lassoing: Option<Vec<Point>>,
    /// Where the pointer is over the viewport, if it is.
    pub(crate) pointer: Option<Pos2>,
    /// The handle of the selection under the pointer, or the one being dragged: a turn shows
    /// a curved arrow for the pointer.
    pub(crate) hover_handle: Option<Handle>,
    /// Which way the curved arrow of a turn bulges, as an angle on screen: out from the box
    /// at the corner it was taken by.
    pub(crate) turn_facing: f32,
    /// The Cutter tool's cut was with Alt: what it kept apart is selected, for the Select tool.
    pub cut_kept: bool,
    /// The icon drawn as the pointer in the last frame.
    #[cfg(test)]
    pub(crate) pointer_icon: Option<&'static str>,
    /// The lasso and the swatch of a selection that a command is moving: they are the
    /// selection's again where it lands, the lasso moved as what it holds was.
    carried: Option<(Vec<iskra_model::LassoPart>, Option<SwatchId>)>,
    /// What is being dragged out or drawn is added to the selection (`false`) or taken from
    /// it (`true`), where it does not take its place: Shift or Alt was held at the press.
    combining: Option<bool>,
    /// A selection handle being dragged.
    drag: Option<Drag>,
    /// A peg's transform as it is being edited somewhere else (the properties panel), to show
    /// before it is in the project.
    pub live: Option<(PegId, Transform)>,
    peg_drag: Option<PegDrag>,
    /// The other pegs the Transform tool has in hand with the one it works on: they are moved,
    /// turned and scaled along with it. The app says which before the viewport is shown.
    pub with_pegs: Vec<PegId>,
    /// Those pegs while the one is dragged: each with its transform before the drag, and with
    /// the drag so far.
    peg_along: Vec<(PegId, Transform, Transform)>,
    /// The Transform tool's drag that has just ended: shown for the one frame until the
    /// project has what it asked for.
    peg_held: Option<PegDrag>,
    /// The Transform tool's click on a drawing was with Shift held: its layer is to be picked
    /// beside what is picked, and not in its place.
    pub clicked_more: bool,
    /// The layer the Select tool's click took in hand, whose drawing it selects whole once
    /// the layer is the one in hand.
    whole_pending: Option<LayerId>,
    /// Where the selection's handles on its sides were drawn, the top's first.
    #[cfg(test)]
    pub side_handles: std::cell::RefCell<Vec<Pos2>>,
    /// The box round what the Transform tool's peg carries on the frame that is showing, in
    /// the peg's own space: its left, top, right and bottom. And the bounds of the drawings
    /// it was worked out from, kept for as long as they are the same drawings.
    scale_box: Option<(PegId, [f64; 4])>,
    box_bounds: Vec<(Arc<iskra_drawing::Drawing>, PixelRect)>,
    /// And where bones bend those bounds to, in the drawing's pixels, kept under the warp's
    /// key and the bounds.
    bent_bounds: Vec<((u64, PixelRect), [f64; 4])>,
    /// Where the handles of the box were: its corners, clockwise from the top left, then the
    /// middles of its sides, from the top's.
    #[cfg(test)]
    pub scale_handles: Vec<Pos2>,
    /// Where the pivot of the peg in hand was drawn.
    #[cfg(test)]
    pub pivot_on_screen: Option<Pos2>,
    /// The Transform tool's temporary pivot, in the peg's own space: which peg's, and where.
    /// It is kept across frames while the same layer and peg are in hand.
    pub(crate) temp_pivot: Option<(PegId, [f64; 2])>,
    /// The layer and the peg in hand that the temporary pivot was set with.
    temp_for: (Option<LayerId>, Option<PegId>),
    /// A corner of the camera's frame being dragged with the Camera tool, which zooms the
    /// camera and moves it at once.
    frame_drag: Option<FrameDrag>,
    /// Where the corners of the camera's frame were, as the Camera tool's handles; and the
    /// frame as the drag of one has it so far.
    #[cfg(test)]
    pub frame_handles: Vec<Pos2>,
    #[cfg(test)]
    pub frame_preview: Option<[Pos2; 4]>,
    /// A drag of the Bones tool.
    pub(crate) bone_drag: Option<crate::bones::BoneDrag>,
    /// The Bones tool's press is on something of the chain, or has become a drag: it is not
    /// a click that selects the drawing under it.
    pub(crate) bone_hit: bool,
    /// The deformer whose rest the Bones tool lays: the layer in hand's. Its drawing is shown
    /// as drawn, and what follows those bones where it is at rest; other layers stay posed.
    resting: Option<iskra_model::DeformerId>,
    /// The painted bounds of the drawing the Bones tool lays a rest on, kept for as long as
    /// it is the same drawing; and, for the tests, where its outline was drawn.
    pub(crate) rigged_bounds: Option<(Arc<iskra_drawing::Drawing>, PixelRect)>,
    #[cfg(test)]
    pub rigged_outline: Vec<Pos2>,
    /// The Particles node picked alone, whose emitter is drawn; and, for the tests, where it was.
    pub(crate) emitter: Option<iskra_model::EffectId>,
    #[cfg(test)]
    pub emitter_outline: Vec<Pos2>,
    /// The drag of the Bones tool that has just ended: shown for the one frame until the
    /// project has what it asked for, so that the chain does not spring back for a moment.
    pub(crate) bone_held: Option<crate::bones::BoneDrag>,
    /// The joint of the chain that is selected with the Bones tool, by the place of the bone
    /// that starts at it: its circles are dragged in the viewport.
    pub(crate) bone_joint: Option<usize>,
    /// With the Bones tool, show each bone's reach: which bone has most of each place of the
    /// drawing, as dots in the bones' colours.
    pub show_reach: bool,
    /// The chains of the shot's rigged layers are seen faint whatever is in hand, and not
    /// only that of the layer the Bones tool works on.
    pub bones_always: bool,
    /// The Animate switch is on: a thin outline of the keyframes' red runs round the view, so
    /// that what is changed in it is seen to be keyed.
    pub animating: bool,
    /// Where that outline was drawn.
    #[cfg(test)]
    pub animating_outline: Option<egui::Rect>,
    /// The peg and the frame the view was last panned to follow, with the follow mode on.
    followed: Option<(PegId, u32)>,
    /// The bone that was last taken hold of to pose it, filled in the accent's orange until a
    /// press on nothing lets it go.
    pub bone_picked: Option<usize>,
    /// For *Reach*'s dots, which of the cells they stand for have anything painted: kept by
    /// the drawing's address and the cells' size, as it takes a look at every pixel.
    pub(crate) reach_cells: Option<(usize, u64, Vec<bool>)>,
    /// The dots of the reach that were drawn: whose, by the bone's place, and where.
    #[cfg(test)]
    pub reach_dots: Vec<(usize, Pos2)>,
    /// The chains drawn faint with *Always Visible*: whose, and their joints on screen. A click
    /// with the Transform tool on one selects its layer.
    pub faint_chains: Vec<(LayerId, Vec<Pos2>)>,
    /// A chain of bones as Properties has it while one of its values is dragged there.
    pub(crate) live_bones: Option<iskra_model::Deformer>,
    /// A parameter of an effect node as it is being edited in Properties, a number dragged or
    /// a curve's point: shown before the project has it, held on every frame.
    pub(crate) live_effect: Option<(iskra_model::EffectId, String, iskra_model::Value)>,
    /// Where the joints of the chain the Bones tool works on were drawn.
    #[cfg(test)]
    pub bone_joints: Vec<Pos2>,
    /// The bone that was drawn in the accent's orange, as the selected one.
    #[cfg(test)]
    pub bone_lit: Option<usize>,
    /// The ends of the handles of the selected point of a curve as they were drawn: whether
    /// it is the one the line leaves by, and where.
    #[cfg(test)]
    pub bone_handles: Vec<(bool, Pos2)>,
    /// The circles of the chain's joints as they were drawn: the joint, and the two radii.
    #[cfg(test)]
    pub bone_circles: Vec<(usize, f32, f32)>,
    /// Where the goal of the chain's IK was drawn, where it is on.
    #[cfg(test)]
    pub bone_goal: Option<Pos2>,
    /// A keyframe's square on the path was clicked: the frame to go to. The app takes it.
    pub seek: Option<u32>,
    /// The layer whose drawing the Transform tool clicked on, for the app to select; and that
    /// it clicked where nothing is drawn, for the app to let go of what is selected.
    pub clicked: Option<LayerId>,
    pub clicked_nothing: bool,
    /// Where the Transform tool was pressed, and whether the pointer has gone far enough from
    /// there since for it to be a drag, not a click.
    press: Option<(Pos2, bool)>,
    /// The guides to draw over the camera's frame. The app says which.
    /// Only look: the pen and the pointer change nothing. For a viewer that is no place to
    /// draw, as on the Export page.
    pub look_only: bool,
    pub guides: Vec<crate::prefs::Guide>,
    /// The aspect ratio outside which the frame is darkened, and the user's own guides.
    pub aspect_mask: Option<f64>,
    /// Whether only what is in the camera's frame is seen: what is outside it is covered by
    /// this, the grey round the frame. Not in the flat view.
    pub clip: Option<Color32>,
    /// The frame that was cut to, for the tests.
    #[cfg(test)]
    pub clipped: Option<[Pos2; 4]>,
    /// The texts the Export page burns into its renders, with the shot's name and the frame's
    /// count under the playhead: drawn over the frame as the render will have them.
    pub burned: Option<(iskra_export::BurnIn, Option<String>, String)>,
    /// Where each was drawn, and what it said.
    #[cfg(test)]
    pub burned_drawn: Vec<(String, egui::Rect)>,
    pub user_guides: Vec<crate::prefs::UserGuide>,
    /// The bands the mask darkened when the viewport was last drawn.
    #[cfg(test)]
    pub masked: Vec<[Pos2; 4]>,
    /// The layers that the camera shows larger than their pixels allow on the frame that is
    /// showing, each with how many pixels of the picture one of its pixels is across.
    pub coarse: Vec<(LayerId, f64)>,
    /// In the flat view: the corners on screen of what the camera sees at depth 0, to draw
    /// over the drawings.
    camera_outline: Option<[Pos2; 4]>,
    #[cfg(test)]
    pub camera_frame: Option<[Pos2; 4]>,
    /// Where the path's keyframes were drawn, and the handles of the one under the playhead
    /// (arriving, leaving).
    #[cfg(test)]
    pub path_keys: Vec<(u32, Pos2)>,
    #[cfg(test)]
    pub bend_dots: Option<(Pos2, Pos2)>,
    /// Where the path's in-betweens were drawn, by their frames.
    #[cfg(test)]
    pub path_dots: Vec<(u32, Pos2)>,
    /// How the in-betweens a dot slid along the path pushes are spaced again: the timelines',
    /// handed over before the viewport is shown.
    pub breakdowns: crate::breakdown::Breakdowns,
    /// An in-between's dot being slid along the path: the way's spacing before the drag, and
    /// as the drag has it now.
    tween: Option<(Vec<f64>, Vec<f64>)>,
    textures: HashMap<(usize, usize, usize), TileTexture>,
    /// The filled areas of drawings that a palette paints with gradients and pictures (plan
    /// §4.20), by the addresses of the drawing and the palette: what their tiles are painted
    /// by. Held with both, and the frame of the screen they were last drawn on.
    areas: HashMap<(usize, usize), (Arc<iskra_model::Drawing>, Arc<Palette>, Arc<iskra_model::Areas>, u64)>,
    /// What goes through each effect node that is seen (plan §8), as a picture made by the
    /// reference compositor: kept while what it is of stays the same.
    through: HashMap<crate::playback::CardKey, ThroughPicture>,
    /// What the compositor made of what went through effect nodes, kept by what it was made
    /// of: a frame that is come back to, or an edit elsewhere in the shot, finds it again.
    made: iskra_render::Cache,
    /// How many of those the last frame of the screen showed, and how many it had to make.
    #[cfg(test)]
    pub through_shown: (usize, usize),
    /// The count of frames of the screen drawn so far.
    drawn: u64,
    /// What the shot shows on each frame, kept while the shot is the same.
    frames: iskra_eval::Cache,
    /// The frames of the shot that playback will show next, nearest first. Empty when it is
    /// not playing. The app says which.
    pub ahead: Vec<u32>,
    /// The shot is playing. The app says so.
    pub playing: bool,
    /// The frames of the shot that are made and kept on the card, in their order, for the
    /// timeline to mark: those that will play without being made first.
    pub cached_frames: Vec<u32>,
    /// How long the slowest frame took that was made for the cache since this was last
    /// taken, in seconds: the window stood still for that long.
    pub cache_took: Option<f64>,
    /// What the frames kept are of, as three numbers, and since when it has been that and
    /// been left alone.
    settled: Option<((usize, usize, u64), f64)>,
    /// The frame that was last shown, and when it became that: while the playhead is moved,
    /// the frames that are kept are shown, as when the shot plays.
    scrubbed: Option<(u32, f64)>,
    /// Whole frames made on the graphics card, which the shot is shown from while it plays.
    /// `None` where there is no card to make them on: the shot then plays from its tiles.
    pub cache: Option<crate::playback::FrameCache>,
    /// The last frame of the screen showed the shot from the cache.
    #[cfg(test)]
    pub from_cache: bool,
    /// How many tiles were last drawn through a deformer's mesh.
    #[cfg(test)]
    pub bent_tiles: usize,
    /// How many onion skin ghosts were last drawn from tiles.
    #[cfg(test)]
    pub ghosts_drawn: usize,
}

impl Viewport {
    /// How many drawing tiles the last frame drew, and how many textures are kept.
    #[cfg(test)]
    pub fn tiles_shown(&self) -> usize {
        self.textures.values().filter(|texture| texture.used == self.drawn).count()
    }

    /// What the paint bucket's drag has filled so far, on the art layer it is on.
    #[cfg(test)]
    pub fn fill_shown(&self) -> Option<IndexedSurface> {
        match &self.fill_live.as_ref()?.art {
            Art::Indexed(surface) => Some(surface.clone()),
            Art::Rgba(_) => None,
        }
    }

    /// The widths, in the drawing's pixels, of the onion skin's outlines on screen.
    #[cfg(test)]
    pub fn outlines_shown(&self) -> std::collections::BTreeSet<usize> {
        // Of what is shown, the silhouettes' keys below the palettes' addresses are outlines.
        self.textures.iter().filter(|(key, texture)| texture.used == self.drawn && (1..=64).contains(&key.1)).map(|(key, _)| key.1).collect()
    }

    /// How many drawing tiles the last frame drew painted with gradients or pictures.
    #[cfg(test)]
    pub fn shaded_tiles_shown(&self) -> usize {
        self.textures.iter().filter(|(key, texture)| texture.used == self.drawn && key.2 != 0).count()
    }

    pub fn tiles_kept(&self) -> usize {
        self.textures.len()
    }

    /// How many frames have been evaluated, as against found already evaluated.
    #[cfg(test)]
    pub fn frames_evaluated(&self) -> u64 {
        self.frames.evaluated
    }

    /// Fit the camera frame in the viewport the next time it is shown.
    pub fn fit(&mut self) {
        self.fitted = false;
    }

    /// The shot with the peg that is being edited as the edit has it so far.
    /// `shot` as only `work` of it, made once for as long as the shot is the same, so that
    /// what keeps frames of a shot by its address keeps them.
    fn worked(&mut self, shot: &Arc<Shot>, work: Work) -> Arc<Shot> {
        if work == Work::Whole {
            return shot.clone();
        }
        if let Some((_, _, made)) = self.worked.as_ref().filter(|(of, kept, _)| Arc::ptr_eq(of, shot) && *kept == work) {
            return made.clone();
        }
        let made = Arc::new(shot.only(work));
        self.worked = Some((shot.clone(), work, made.clone()));
        made
    }

    /// The point of the drawing in hand that is seen at `at`, which is in the drawing's
    /// pixels as its pegs place it: taken back through the bend of its bones, if they bend it.
    fn drawn(&self, at: Point) -> Point {
        self.bend.as_ref().map_or(at, |warp| warp.undo(at))
    }

    /// The art layer of `layer` on `frame` that a stroke is being drawn on, as the stroke has
    /// it so far, or as the stroke that was just finished left it.
    fn live_art(&self, layer: LayerId, frame: u32) -> Option<(ArtLayerKind, Art)> {
        let drawn = self.stroke.as_ref().filter(|stroke| stroke.layer == layer && stroke.frame == frame).map(|stroke| (stroke.art, stroke.live.art()));
        let filled = || self.fill_live.as_ref().filter(|live| live.layer == layer && live.frame == frame).map(|live| (live.kind, live.art.clone()));
        // A point of vector art being dragged in the point editor.
        let pointed = || self.points.live(layer, frame).map(|(kind, art)| (kind, Art::Indexed(art.raster().clone())));
        let landed = || self.landed.as_ref().filter(|(of, on, ..)| *of == layer && *on == frame).map(|(_, _, kind, art)| (*kind, art.clone()));
        drawn.or_else(filled).or_else(pointed).or_else(landed)
    }

    /// The shot as the window shows it standing still, for the compositor to draw: with the
    /// stroke that is being drawn and the selection that is being dragged in place of the
    /// drawing they are on, without the layers that are hidden or not of the work in hand,
    /// and unbent while the Bones tool lays a chain at rest. With it, what no shot says: the
    /// layers that are faded, and the onion skin's ghosts.
    fn windowed(&self, shot: &Shot, current: Option<LayerId>, tools: &Tools, display: &Display, onion: [Color32; 2]) -> (Shot, iskra_gpu::Looks) {
        use iskra_gpu::{Ghost, Look, Looks};
        let frame = tools.frame;
        let fading = self.work == Work::Whole && self.fade_boards;
        let mut own = if self.work == Work::Whole { shot.clone() } else { shot.only(self.work) };
        own.hidden.extend(self.hidden.iter().map(|layer| layer.0));
        let at_rest = tools.rest();
        let mut looks = Looks::default();
        for at in 0..own.layers.len() {
            let is_current = current == Some(own.layers[at].id);
            // One art layer shown alone, of the drawing in hand or of all: the others are off,
            // for the card as for a layer that has them switched off.
            if let Some(shown) = display.shown_art.filter(|_| is_current || display.shown_art_all) {
                let off = &mut Arc::make_mut(&mut own.layers[at]).art_off;
                for other in ArtLayerKind::ALL.into_iter().filter(|kind| *kind != shown) {
                    if !off.contains(&other) {
                        off.push(other);
                    }
                }
            }
            let layer = own.layers[at].clone();
            // A board under the animation is reference, unless it is the layer in hand and
            // may be drawn on; on the light table the layers that are not in hand are faint.
            let reference = fading && shot.is_board(layer.id) && (!is_current || self.locked.is_some());
            if reference {
                looks.layers.insert(layer.id, Look { opacity: BOARD_OPACITY, tint: None });
            } else if display.light_table && current.is_some() && !is_current {
                looks.layers.insert(layer.id, Look { opacity: LIGHT_TABLE_OPACITY, tint: None });
            }
            let bent = |frame: u32| warp_of(shot, &layer, frame, !(at_rest && is_current));
            // The onion skin of the layer in hand and of those picked with it: each ghost where
            // the pegs had the layer on its own frame. Not of a layer seen in perspective.
            if self.onioned.contains(&layer.id) && !shot.tilted(layer.parent, frame) {
                let placed = |frame: u32| space(shot, false, layer.parent, layer.parent, frame).m;
                let keys = Motion::keys_of(shot, &layer);
                for ghost in ghosts(&layer, frame, display, &Motion { keys, placed: &placed, bent: &bent }) {
                    let Some(pool) = layer.drawing(shot.shown(&layer, ghost.drawing)) else { continue };
                    // Each is moved by its own move and then by the move of all.
                    let shift = display.shift_of(ghost.drawing);
                    let ([dx, dy], [ax, ay], [bx, by]) = (shift.apply([0.0, 0.0]), shift.apply([1.0, 0.0]), shift.apply([0.0, 1.0]));
                    let shifted = Affine { m: [ax - dx, ay - dy, bx - dx, by - dy, dx, dy] };
                    let [r, g, b, a] = ghost.color(display.opacity, onion).to_srgba_unmultiplied().map(|channel| f32::from(channel) / 255.0);
                    looks.ghosts.push(Ghost {
                        under: layer.id,
                        drawing: pool.drawing.clone(),
                        to_frame: Projection::flat(&shifted.mul(&Placement::of(shot, &layer, ghost.frame, false).to_scene)),
                        warp: bent(ghost.frame),
                        look: Look { opacity: a, tint: Some([r, g, b]) },
                        lines_only: display.line_art_only,
                    });
                }
            }
            let live = self.live_art(layer.id, frame);
            let dragged = self.dragging().filter(|_| is_current);
            let unbend = at_rest && is_current && layer.deformer.is_some();
            if live.is_none() && dragged.is_none() && !unbend {
                continue;
            }
            let mut changed = Layer::clone(&layer);
            if unbend {
                changed.deformer = None;
            }
            if live.is_some() || dragged.is_some() {
                // The drawing on the frame, with what is being done to it. A stroke in an
                // empty cell has a drawing only once it is committed: here, one of its own.
                let exposed = changed.exposure.at(frame);
                let mut drawing = exposed.and_then(|id| changed.drawing(id)).map(|pool| iskra_drawing::Drawing::clone(&pool.drawing)).unwrap_or_default();
                for (art, preview) in drawing.art_layers.iter_mut().zip(dragged.iter().flat_map(|drag| &drag.preview)) {
                    if let Some(preview) = preview {
                        art.content = preview.clone();
                    }
                }
                if let Some((pixels, art)) = live.and_then(|(kind, pixels)| Some((pixels, drawing.art_layers.iter_mut().find(|art| art.kind == kind)?))) {
                    art.content = match pixels {
                        Art::Indexed(surface) => ArtContent::Indexed(surface),
                        Art::Rgba(surface) => ArtContent::Rgba(surface),
                    };
                }
                let drawing = Arc::new(drawing);
                match exposed.and_then(|id| changed.drawings.iter_mut().find(|pool| pool.id == id)) {
                    Some(pool) => pool.drawing = drawing,
                    None => {
                        let id = iskra_model::DrawingId(u64::MAX);
                        changed.drawings.push(iskra_model::PoolDrawing { id, name: String::new(), drawing });
                        changed.exposure.set(frame, 1, Some(id));
                    }
                }
            }
            own.layers[at] = Arc::new(changed);
        }
        (own, looks)
    }

    pub(crate) fn posed<'a>(&self, shot: &'a Shot) -> Cow<'a, Shot> {
        let posed = self.posed_as_edited(shot);
        // Seen through another display than the main one: as if that one were.
        match self.looking_through(&posed).and_then(|display| posed.seen_through(display)) {
            Some(seen) => Cow::Owned(seen),
            None => posed,
        }
    }

    /// How many frames the viewport has evaluated, as against found kept.
    #[cfg(test)]
    pub(crate) fn evaluated(&self) -> u64 {
        self.frames.evaluated
    }

    /// The display other than the main one that the viewer looks through, if it does and the
    /// shot has it.
    pub(crate) fn looking_through(&self, shot: &Shot) -> Option<iskra_model::DisplayId> {
        self.viewing.filter(|display| self.work != Work::Boards && *display != shot.main_display() && shot.has_display(*display))
    }

    /// The shot with what is being edited as the edit has it.
    fn posed_as_edited<'a>(&self, shot: &'a Shot) -> Cow<'a, Shot> {
        let mut posed = self.posed_pegs(shot);
        // While the Bones tool lays a rest its drawing is as it was drawn, and a peg that
        // follows those bones is where it is at rest too: as if it followed none.
        let rests = |peg: &Arc<Peg>| peg.follows.is_some_and(|follow| Some(follow.deformer) == self.resting);
        if posed.pegs.iter().any(rests) {
            for peg in posed.to_mut().pegs.iter_mut().filter(|peg| rests(peg)) {
                Arc::make_mut(peg).follows = None;
            }
        }
        // With an effect's parameter as it is being edited.
        let posed = match &self.live_effect {
            Some((id, param, value)) if posed.effect(*id).is_some() => {
                let mut changed = posed.into_owned();
                if let Some(held) = changed.effects.iter_mut().find(|effect| effect.id == *id).and_then(|effect| Arc::make_mut(effect).param_mut(param)) {
                    (held.value, held.keys) = (value.clone(), Default::default());
                }
                Cow::Owned(changed)
            }
            _ => posed,
        };
        // And with the chain the Bones tool has hold of as its drag has it.
        // Or, while a value of a chain is dragged in Properties, with the chain as it is there.
        let dragged = self.bone_drag.as_ref().or(self.bone_held.as_ref()).and_then(|drag| Some((drag.deformer?, &drag.now)));
        let Some((id, now)) = dragged.or(self.live_bones.as_ref().map(|bones| (bones.id, bones))) else { return posed };
        let mut posed = posed.into_owned();
        if let Some(deformer) = posed.deformer_mut(id) {
            *deformer = now.clone();
        }
        Cow::Owned(posed)
    }

    fn posed_pegs<'a>(&self, shot: &'a Shot) -> Cow<'a, Shot> {
        // The drag, or the one that has just ended until the project has what it asked for.
        let dragging = self.peg_drag.or(self.peg_held);
        let editing = dragging.map(|drag| drag.peg).or(self.live.map(|(peg, _)| peg));
        let Some(at) = editing.and_then(|peg| shot.pegs.iter().position(|p| p.id == peg)) else { return Cow::Borrowed(shot) };
        let mut posed = shot.clone();
        // The pegs that go along with the one that is dragged, as the drag has them.
        for (id, _, now) in self.peg_along.iter().filter(|_| dragging.is_some()) {
            if let Some(along) = posed.pegs.iter_mut().find(|peg| peg.id == *id) {
                let along = Arc::make_mut(along);
                (along.transform, along.animation) = (*now, Default::default());
            }
        }
        let tween = self.tween.as_ref().map(|(_, now)| now.clone());
        let peg = Arc::make_mut(&mut posed.pegs[at]);
        // A pose with no keys is the same on whichever frame is showing.
        let mut still = |transform| (peg.transform, peg.animation) = (transform, Default::default());
        match (dragging, self.live) {
            (Some(PegDrag { grab: Grab::Move | Grab::Rotate | Grab::Pivot | Grab::Scale { .. } | Grab::Skew { .. }, now, .. }), _) => still(now),
            (Some(PegDrag { grab: Grab::Spot, .. }), _) => {}
            (Some(PegDrag { grab: Grab::Key(frame), now, .. }), _) => peg.animation.position.set(frame, now.position),
            (Some(PegDrag { grab: Grab::Bend { frame, .. }, bend, .. }), _) => {
                if let Some(key) = peg.animation.position.key_mut(frame) {
                    key.bend = Some(bend);
                }
            }
            (Some(PegDrag { grab: Grab::Tween { key, .. }, .. }), _) => {
                if let Some(spacing) = tween {
                    peg.animation.position.set_chart(key, &spacing);
                }
            }
            (None, Some((_, transform))) => still(transform),
            (None, None) => {}
        }
        Cow::Owned(posed)
    }

    /// Draw the viewport and handle its input. Returns the command a finished stroke or drag
    /// asks for. `peg` is the peg the Transform tool works on.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        session: &Session,
        layer: Option<LayerId>,
        peg: Option<PegId>,
        tools: &Tools,
        brush: &iskra_drawing::InHand,
        display: &mut Display,
        pen: Option<PenSample>,
    ) -> Option<(&'static str, Value)> {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::drag());
        // What the last frame's stroke or drag left is in the project by now.
        (self.landed, self.dropped) = (None, None);
        let rect = response.rect;
        #[cfg(test)]
        {
            self.shown_in = Some(rect);
        }
        let center = rect.center();
        // As the shots' skins show them: what is drawn on is what is seen.
        let project = self.skins_show(session.project());
        let settings = &project.settings;
        if !self.fitted {
            self.view.fit(rect, settings.width as f32, settings.height as f32);
            self.fitted = true;
        }

        // Navigation: a trackpad's scroll pans and a mouse wheel zooms, pinch zooms, a
        // two-finger twist rotates, and dragging with Space held or with the middle button pans;
        // with Space and Cmd held, a drag zooms.
        let (spacebar, command) = ui.input(|i| (i.key_down(Key::Space), i.modifiers.command));
        let panning = spacebar || ui.input(|i| i.pointer.button_down(PointerButton::Middle));
        self.pointer = response.hover_pos();
        if let Some(pointer) = response.hover_pos() {
            let (scroll, wheel, zoom, twist) = ui.input(|i| (scrolled(i), wheeled(i), i.zoom_delta(), i.multi_touch().map_or(0.0, |t| t.rotation_delta)));
            self.view.pan += scroll;
            let zoom = zoom * WHEEL_ZOOM.powf(wheel);
            if zoom != 1.0 {
                self.view.zoom_about(center, pointer, zoom);
            }
            if twist != 0.0 {
                self.view.rotate_about(center, pointer, twist);
            }
            if spacebar && !response.is_pointer_button_down_on() {
                ui.ctx().set_cursor_icon(if command { egui::CursorIcon::ZoomIn } else { egui::CursorIcon::Grab });
            }
        }
        if panning && response.is_pointer_button_down_on() {
            let delta = ui.input(|i| i.pointer.delta());
            if spacebar && command {
                // About where the drag began.
                let from = *self.zoom_from.get_or_insert(response.interact_pointer_pos().unwrap_or(center));
                self.view.zoom_about(center, from, drag_zoom(delta));
                ui.ctx().set_cursor_icon(egui::CursorIcon::ZoomIn);
            } else {
                self.view.pan += delta;
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        }
        if !response.is_pointer_button_down_on() {
            self.zoom_from = None;
        }

        // Following the layer in hand, or the peg: on another frame, or another layer, the view
        // is panned to bring the pivot of the peg that carries it to the middle; never while
        // something is drawn or dragged, until it is let go.
        let followed_shot = self.shot.and_then(|id| project.shots.iter().find(|shot| shot.id == id));
        let followed = followed_shot.and_then(|shot| layer.and_then(|layer| shot.layer(layer)?.parent).or(peg)).map(|peg| (peg, tools.frame));
        if !display.follow {
            self.followed = None;
        } else if followed != self.followed && self.stroke.is_none() && !response.is_pointer_button_down_on() {
            if let Some(at) = followed_shot.zip(followed).and_then(|(shot, (peg, frame))| self.pivot_in_scene(shot, peg, frame)) {
                self.view.centre_on(at);
            }
            self.followed = followed;
        }

        // Shift-and-trace: with the onion skin on, Alt-drag moves the onion-skinned drawings and
        // Alt-Shift-drag turns them: all together, or the one that is chosen in the panel.
        let (alt, shift_key) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        // Not with the Bones tool, where Alt stretches the bone that is dragged.
        let shifting = display.onion && alt && !panning && tools.tool != Tool::Bones;
        if shifting && response.is_pointer_button_down_on() {
            let delta = ui.input(|i| i.pointer.delta());
            // One drawing's own move is made before the move of all, which may have turned it.
            let turned = if display.shifting.is_some() { display.shift.rotation as f32 } else { 0.0 };
            let moves = display.shift_in_hand();
            if shift_key {
                moves.rotation += f64::from(delta.x) * 0.005;
            } else {
                let moved = egui::emath::Rot2::from_angle(-self.view.rotation - turned) * delta / self.view.zoom;
                moves.offset[0] += f64::from(moved.x);
                moves.offset[1] += f64::from(moved.y);
            }
        }
        let panning = panning || shifting;

        // A selection belongs to one drawing: it goes when the tool, layer or frame changes.
        // What a command of the last frame was to carry to where it landed is there by now.
        self.carried = None;
        self.hover_handle = None;
        if self.selection.as_ref().is_some_and(|s| tools.tool != Tool::Select || Some(s.layer) != layer || s.frame != tools.frame) {
            (self.selection, self.drag) = (None, None);
        }
        if self.selection.is_none() || !tools.all_layers {
            self.also.clear();
        }
        if !matches!(tools.tool, Tool::Transform | Tool::Camera) {
            self.peg_drag = None;
        }
        // The temporary pivot goes with the switch, and when another layer or peg is taken.
        if !tools.temp_pivot || (tools.tool == Tool::Transform && self.temp_for != (layer, peg)) {
            self.temp_pivot = None;
        }
        if tools.tool == Tool::Transform {
            self.temp_for = (layer, peg);
        }
        if tools.tool != Tool::Fill {
            self.fill_live = None;
        }
        if tools.tool != Tool::Camera {
            self.frame_drag = None;
        }
        if !matches!(tools.tool, Tool::Bones | Tool::Transform) {
            self.bone_drag = None;
        }
        // What the last frame's drag asked for is in the project by now.
        self.bone_held = None;
        if self.peg_held.take().is_some() {
            self.peg_along.clear();
        }
        // The shot as it is in the project, and as it shows: with the peg that is being edited
        // where the edit has it.
        let shot = self.shot.and_then(|id| project.shot(id));
        // Drawings are coloured as the palette style the shot is shown in has them.
        let colours = self.styled.of(&project.palette, shot.map_or(&[], |shot| &shot.styles));
        // A colour being picked for a swatch is seen on the drawings before it is let go.
        let colours = self.styled.picking(colours, self.recolouring);
        self.ink = tools.free.unwrap_or_else(|| colours.color(tools.swatch));
        self.resting = shot.zip(layer).and_then(|(shot, layer)| shot.layer(layer)?.deformer).filter(|_| tools.rest());
        // The layer in hand as its bones bend it, for the pen to be taken back through. Not
        // while the Bones tool lays a chain at rest, when the drawing is shown as it was drawn.
        self.bend = shot.and_then(|shot| {
            let (layer, posed) = (shot.layer(layer?)?, self.posed(shot));
            warp_of(&posed, layer, tools.frame, !tools.rest())
        });
        // The Transform tool's box, round what its peg carries.
        self.scale_box = match (tools.tool, shot, peg) {
            (Tool::Transform, Some(shot), Some(peg)) => self.carried_box(shot, peg, tools.frame).map(|bounds| (peg, bounds)),
            _ => None,
        };
        // The guide in hand has the pen while one of its handles is dragged.
        let guiding = if self.look_only { None } else { self.guide_input(&response, shot, tools, panning) };
        let command = match (tools.tool, shot) {
            _ if self.look_only => {
                (self.stroke, self.marquee, self.peg_drag, self.bone_drag) = (None, None, None, None);
                None
            }
            _ if guiding.is_some() => {
                (self.stroke, self.marquee) = (None, None);
                guiding.flatten()
            }
            (Tool::Bones, Some(shot)) => {
                (self.stroke, self.marquee) = (None, None);
                let asked = self.bone_input(ui, &response, shot, layer, tools, panning);
                // A click on no part of the chain selects the layer whose drawing is under
                // it, as the Transform tool's does; on nothing it keeps the layer in hand.
                if let Some(at) = self.click(&response, panning)
                    && !self.bone_hit
                {
                    self.clicked = self.layer_under(shot, center, at, tools.frame).filter(|under| Some(*under) != layer);
                    self.clicked_more = shift_key;
                }
                asked
            }
            (Tool::Bones, None) => None,
            (Tool::Zoom, _) => {
                (self.stroke, self.marquee) = (None, None);
                self.zoom_input(ui, &response, panning);
                None
            }
            (Tool::Transform | Tool::Camera, Some(shot)) => {
                (self.stroke, self.marquee) = (None, None);
                // The Transform tool poses the bones of the layer in hand: a press on a bone, a
                // joint or a handle of them takes it, and one anywhere else is the peg's. It is
                // tried only as the press begins, so that the peg dragged over a joint stays.
                let pressed = ui.input(|i| i.pointer.primary_pressed());
                let transforming = tools.tool == Tool::Transform;
                if transforming && pressed {
                    self.bone_hit = false;
                }
                let posing = transforming && self.peg_drag.is_none() && (self.bone_drag.is_some() || pressed || !response.is_pointer_button_down_on());
                let posed = if posing { self.bone_input(ui, &response, shot, layer, tools, panning) } else { None };
                let bones_have_it = self.bone_drag.is_some() || posed.is_some();
                // A click with the Transform tool selects the layer of the chain drawn faint under
                // it, else the layer whose drawing is under it: the topmost that is painted
                // there. On the box round what the selected peg carries, its sides and corners
                // and their handles too, which only a drag takes: a small part that lies on a
                // large peg's box is selected there.
                let boxed = |grab: Grab| matches!(grab, Grab::Move | Grab::Rotate | Grab::Scale { .. } | Grab::Skew { .. });
                if transforming
                    && let Some(at) = self.click(&response, panning)
                    && self.peg_drag.is_none_or(|drag| boxed(drag.grab))
                    && !self.bone_hit
                {
                    self.clicked = self.faint_under(shot, layer, at).or_else(|| self.layer_under(shot, center, at, tools.frame));
                    // With Shift, a click on nothing lets nothing go.
                    (self.clicked_more, self.clicked_nothing) = (shift_key, self.clicked.is_none() && !shift_key);
                }
                if tools.tool != Tool::Transform {
                    self.press = None;
                }
                // The Camera tool's frame has the pen while one of its corners is dragged.
                let half = [f64::from(settings.width) / 2.0, f64::from(settings.height) / 2.0];
                let framing = if tools.tool == Tool::Camera { self.frame_input(ui, &response, shot, peg, tools.frame, half, panning) } else { None };
                match framing {
                    Some(done) => done,
                    None if bones_have_it => posed,
                    None => {
                        let (even, pivoting, spot) = (tools.uniform != shift_key, tools.pivot && transforming, tools.temp_pivot && transforming);
                        self.peg_input(ui, &response, shot, peg, tools.frame, panning, even, pivoting, spot, tools.squash && transforming)
                    }
                }
            }
            (Tool::Transform | Tool::Camera, None) => None,
            // A locked layer is looked at, not drawn on.
            _ if self.locked.is_some() => {
                (self.stroke, self.marquee, self.selection, self.drag, self.lassoing) = (None, None, None, None, None);
                // The Select tool's click still picks a drawing that is seen, on another layer,
                // as the Transform tool's does: it is selected whole once its layer is in hand.
                if tools.tool == Tool::Select
                    && let (Some(shot), Some(at)) = (shot, self.click(&response, panning))
                    && let Some(under) = self.layer_under(shot, center, at, tools.frame).filter(|under| Some(*under) != layer)
                {
                    (self.clicked, self.whole_pending) = (Some(under), Some(under));
                }
                if let Some(why) = self.locked.filter(|_| response.hovered()) {
                    response.clone().on_hover_text_at_pointer(why);
                }
                None
            }
            (tool, _) => {
                let current = shot.and_then(|shot| {
                    let layer = shot.layer(layer?)?;
                    Some((layer, Placement::of(&self.posed(shot), layer, tools.frame, self.flat)))
                });
                if tool == Tool::Select {
                    self.stroke = None;
                    // A drawing whose layer a click took in hand is selected whole once it is.
                    if let (Some(pending), Some(current)) = (self.whole_pending, current.as_ref())
                        && pending == current.0.id
                    {
                        (self.selection, self.whole_pending) = (whole(current.0, tools.frame, tools.selected_art()), None);
                    }
                    // A click, not a corner of a lasso nor with Shift or Alt, selects the
                    // drawing under it whole: the topmost that is painted there.
                    let combining = ui.input(|i| i.modifiers.shift || i.modifiers.alt);
                    let click = self.click(&response, panning).filter(|_| !combining);
                    let had = self.selection.clone();
                    let mut return_piece = None;
                    let asked = self.select_input(ui, &response, current, tools, panning);
                    // With *Pieces*, a click on a vector line of the layer in hand takes the piece of
                    // it between the crossings either side, split from the rest.
                    let piece = click.filter(|_| tools.pieces && asked.is_none()).and_then(|at| {
                        let (drawn, place) = current.as_ref()?;
                        let space = self.drawing_space(place, center).filter(|_| draws_vector(drawn, tools))?;
                        let (on, beside) = (space.to_drawing(at), space.to_drawing(at + egui::vec2(PIECE_REACH, 0.0)));
                        let reach = (beside.x - on.x).hypot(beside.y - on.y);
                        let params = json!({
                            "layer": drawn.id.0, "frame": tools.frame, "artLayer": tools.art, "x": on.x, "y": on.y, "reach": reach, "protect": self.protect,
                        });
                        Some(("drawing.splitPiece", params))
                    });
                    // With *Shapes*, a click on vector art of the layer in hand takes the one shape
                    // under it whole, whatever crosses it.
                    let shape = click.filter(|_| tools.shapes && !tools.pieces && asked.is_none()).and_then(|at| {
                        let (drawn, place) = current.as_ref()?;
                        let space = self.drawing_space(place, center).filter(|_| draws_vector(drawn, tools))?;
                        let drawing = &drawn.drawing_at(tools.frame)?.drawing;
                        let (on, beside) = (space.to_drawing(at), space.to_drawing(at + egui::vec2(PIECE_REACH, 0.0)));
                        let reach = (beside.x - on.x).hypot(beside.y - on.y);
                        // The art layers the selection takes, the top first.
                        let arts: Vec<&iskra_model::VectorArt> = ArtLayerKind::ALL
                            .iter()
                            .rev()
                            .filter(|kind| tools.selected_art().takes(**kind))
                            .filter_map(|kind| match drawing.layer(*kind).map(|art| &art.content) {
                                Some(ArtContent::Vector(vector)) => Some(vector),
                                _ => None,
                            })
                            .collect();
                        let (at, index) = arts.iter().enumerate().find_map(|(at, art)| Some((at, art.shape_at(on, reach)?)))?;
                        let others: Vec<&iskra_model::VectorArt> = arts.iter().enumerate().filter(|(other, _)| *other != at).map(|(_, art)| *art).collect();
                        let (x, y) = arts[at].own_pixel(index, &others)?;
                        let lasso = iskra_model::Lasso::of_rect(PixelRect::new(x, y, x + 1, y + 1))?;
                        Some((drawn.id, Selection::of_lasso(drawn.id, tools.frame, lasso)))
                    });
                    if piece.is_some() {
                        return_piece = piece;
                    } else if let Some((drawn, picked)) = shape {
                        self.selection = Some(picked);
                        if let Some(layer) = shot.and_then(|shot| shot.layer(drawn)) {
                            self.fit_to_shapes(layer, tools.frame, tools.selected_art());
                        }
                    } else if let (Some(shot), Some(at)) = (shot, click) {
                        match self.layer_under(shot, center, at, tools.frame) {
                            Some(under) if Some(under) == layer => {
                                self.selection = shot.layer(under).and_then(|layer| whole(layer, tools.frame, tools.selected_art()));
                            }
                            Some(under) => (self.clicked, self.whole_pending) = (Some(under), Some(under)),
                            None => {}
                        }
                    }
                    // A new selection, with *Select All Drawings*, is made on the other layers too.
                    if let Some(shot) = shot.filter(|_| tools.all_layers && asked.is_none() && self.selection != had) {
                        self.also = self.others_of(shot, tools.frame);
                    }
                    match (asked, shot) {
                        (Some(("drawing.transform", params)), Some(shot)) if !self.also.is_empty() => Some(self.with_others(shot, params, tools.frame)),
                        (asked, _) => asked.or(return_piece),
                    }
                } else if tool == Tool::Cutter {
                    self.stroke = None;
                    self.cutter_input(ui, &response, shot, current, tools, panning)
                } else if tool == Tool::Fill {
                    (self.stroke, self.marquee) = (None, None);
                    self.fill_input(&response, current, tools, pen, panning)
                } else if tool == Tool::Points {
                    (self.stroke, self.marquee) = (None, None);
                    let click = self.click(&response, panning);
                    match (current, panning) {
                        (Some((layer, place)), false) => self.drawing_space(&place, center).and_then(|space| {
                            let (to_screen, to_drawing) = (|at| space.to_screen(at), |pos| space.to_drawing(pos));
                            let map = crate::points::Map { to_screen: &to_screen, to_drawing: &to_drawing };
                            self.points.input(ui, &response, click, layer, tools.frame, tools.art, &map)
                        }),
                        _ => None,
                    }
                } else if tool == Tool::Text {
                    (self.stroke, self.marquee) = (None, None);
                    match current {
                        Some((layer, place)) if layer.text => self.drawing_space(&place, center).and_then(|space| {
                            let (to_screen, to_drawing) = (|at| space.to_screen(at), |pos| space.to_drawing(pos));
                            let map = crate::points::Map { to_screen: &to_screen, to_drawing: &to_drawing };
                            self.text.input(ui, &response, layer, tools.frame, &map, tools.swatch, panning)
                        }),
                        // A box typed in on a layer let go of, or on another tool, is let go.
                        _ => {
                            if response.hovered() {
                                response.clone().on_hover_text_at_pointer("The Text tool types on a text layer: make one with Drawing ▸ New Text Layer");
                            }
                            None
                        }
                    }
                } else if tool == Tool::Gradient {
                    (self.stroke, self.marquee) = (None, None);
                    let click = self.click(&response, panning);
                    match current {
                        Some((layer, place)) => self.drawing_space(&place, center).and_then(|space| {
                            let (to_screen, to_drawing) = (|at| space.to_screen(at), |pos| space.to_drawing(pos));
                            let map = crate::points::Map { to_screen: &to_screen, to_drawing: &to_drawing };
                            self.placing.input(&response, click, layer, tools.frame, &colours, &map, panning)
                        }),
                        None => None,
                    }
                } else if tool == Tool::Eyedropper {
                    (self.stroke, self.marquee) = (None, None);
                    match self.pick_input(&response, current, tools, display, panning) {
                        Some(Ok(swatch)) => self.picked = Some(swatch),
                        Some(Err(colour)) => self.picked_colour = Some(colour),
                        None => {}
                    }
                    None
                } else if tool == Tool::Shape && tools.shape == Drawn::Polyline && !pen.is_some_and(|p| p.eraser) {
                    (self.stroke, self.marquee) = (None, None);
                    self.polyline_input(ui, &response, current, tools, &brush.raster.brush, panning)
                } else {
                    (self.marquee, self.polyline) = (None, None);
                    let _timer = crate::timings::timer("viewport: pen input");
                    self.draw_input(ui, &response, current, tools, brush, pen, panning)
                }
            }
        };
        // Seen through another display than the main one, with nothing being edited: the shot
        // as that display shows it, made once for as long as the shot and the display are the
        // same, so that its frames are kept as the main one's are.
        let project_shot = self.shot.and_then(|id| project.shots.iter().find(|shot| shot.id == id));
        let unedited = shot.is_some_and(|shot| matches!(self.posed_as_edited(shot), Cow::Borrowed(_)));
        let seen: Option<Arc<Shot>> = match (project_shot, unedited) {
            (Some(own), true) => self.looking_through(own).and_then(|display| {
                if let Some((of, by, seen)) = &self.seen
                    && Arc::ptr_eq(of, own)
                    && *by == display
                {
                    return Some(seen.clone());
                }
                let seen = Arc::new(own.seen_through(display)?);
                self.seen = Some((own.clone(), display, seen.clone()));
                Some(seen)
            }),
            _ => None,
        };
        let shot = match &seen {
            Some(seen) => Some(Cow::Borrowed(&**seen)),
            None => shot.map(|shot| self.posed(shot)),
        };
        // What the frame shows. The shot as the project has it is evaluated once a frame for as
        // long as it stays the same; one with a peg held by a drag is worked out as it goes.
        self.drawn += 1;
        let kept = seen.as_ref().or(project_shot).filter(|_| matches!(shot, Some(Cow::Borrowed(_))));
        // The boards alone are evaluated as the boards are seen: with the storyboard's group,
        // which its eye on the Animation page does not put away here.
        let timer = crate::timings::timer("viewport: evaluate");
        let shown = match (&shot, kept) {
            (_, Some(kept)) if self.work == Work::Boards => {
                let boards = self.worked(kept, Work::Boards);
                self.frames.frame(&boards, tools.frame)
            }
            (_, Some(kept)) => self.frames.frame(kept, tools.frame),
            (Some(posed), None) if self.work == Work::Boards => Arc::new(iskra_eval::evaluate(&posed.only(Work::Boards), tools.frame)),
            (Some(posed), None) => Arc::new(iskra_eval::evaluate(posed, tools.frame)),
            (None, None) => Arc::default(),
        };
        drop(timer);
        let shot = shot.as_deref();

        // The camera frame as paper.
        let (w, h) = (f64::from(settings.width) / 2.0, f64::from(settings.height) / 2.0);
        let corners = [[-w, -h], [w, -h], [w, h], [-w, h]].map(|p| self.view.to_screen(center, p));
        let paper = if self.black { Color32::BLACK } else { Color32::WHITE };
        painter.add(Shape::convex_polygon(corners.to_vec(), paper, Outline::new(1.0, Color32::from_gray(90))));
        // In the flat view, what the camera sees of the plane at depth 0 is outlined on it.
        #[cfg(test)]
        {
            self.camera_frame = None;
        }
        if let Some(back) = shot.filter(|_| self.flat).and_then(|shot| shot.seen(0.0, tools.frame)?.inverse()) {
            let seen = [[-w, -h], [w, -h], [w, h], [-w, h]].map(|[x, y]| {
                let at = back.apply(Point::new(x, y));
                self.view.to_screen(center, [at.x, at.y])
            });
            #[cfg(test)]
            {
                self.camera_frame = Some(seen);
            }
            self.camera_outline = Some(seen);
        } else {
            self.camera_outline = None;
        }

        // What the camera enlarges past what its pixels allow, to say so and to outline.
        self.coarse = shot.map_or(Vec::new(), |shot| iskra_eval::enlarged(shot, &shown));
        // While it plays, the shot is shown from whole frames made on the card, if there is
        // one: the frame under the playhead, with a few of those ahead made meanwhile. Not in
        // the flat view, which is no picture the camera takes.
        let mut cached = false;
        // The cache has the boards alone, the animation alone, or both with the boards faded.
        let fading = self.work == Work::Whole && self.fade_boards;
        // Where the boards are seen with the animation, the frames are of all of it, the boards
        // faded in their place among the layers: over what they are stacked over, under the rest.
        let played = kept.map(|kept| self.worked(kept, self.work));
        if let Some(cache) = self.cache.as_mut() {
            cache.boards = fading.then_some(BOARD_OPACITY);
        }
        // The playhead moved a moment ago, with nothing that only the window shows wanted
        // under it: no stroke, no onion skin, no light table.
        let now = ui.input(|i| i.time);
        if self.scrubbed.is_none_or(|(frame, _)| frame != tools.frame) {
            // The first frame that is shown is no move.
            self.scrubbed = Some((tools.frame, if self.scrubbed.is_some() { now } else { f64::NEG_INFINITY }));
        }
        let moved = self.scrubbed.is_some_and(|(_, since)| now - since < SCRUB);
        // Nor while a rest is edited, when the drawings are shown as they were drawn: a frame
        // that is kept has them posed.
        let at_rest = tools.rest();
        let scrubbing =
            moved && !at_rest && self.stroke.is_none() && self.landed.is_none() && self.dragging().is_none() && !display.onion && !display.light_table;
        // A whole picture, laid over the camera's frame.
        let on_frame = |picture: egui::TextureId| {
            let mut mesh = egui::Mesh::with_texture(picture);
            let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
            mesh.vertices.extend(corners.iter().zip(uvs).map(|(&pos, [u, v])| Vertex { pos, uv: Pos2::new(u, v), color: Color32::WHITE }));
            mesh.indices.extend([0, 1, 2, 0, 2, 3]);
            Shape::mesh(mesh)
        };
        // Not while something is drawn or dragged with the film standing still: what the pen
        // does is seen on the shot in hand, alone.
        let working = self.stroke.is_some() || self.landed.is_some() || self.dragging().is_some();
        #[cfg(test)]
        {
            self.mixed_shown = false;
        }
        if let Some(picture) = self.mixed.filter(|_| !self.flat && (self.playing || !working)) {
            // Inside a transition the film is two shots mixed, which the app has made.
            painter.add(on_frame(picture));
            cached = true;
            #[cfg(test)]
            {
                self.mixed_shown = true;
            }
        } else if let (true, false, false, Some(cache), Some(kept)) = (scrubbing && !self.playing, self.flat, self.black, self.cache.as_mut(), played.as_ref())
        {
            // The playhead is being moved: a frame that is kept is shown as it is, at once.
            // One that is not is drawn as when the shot stands still. A kept frame is of the
            // camera's frame alone, so a drawing larger than the frame is cut on those and
            // whole on the others: the aspect mask hides what is outside on both.
            let scale = crate::playback::FrameCache::scale_for(self.view.zoom * ui.ctx().pixels_per_point());
            if cache.made_of(kept, &project.palette, settings, scale).binary_search(&tools.frame).is_ok() {
                painter.add(on_frame(cache.frame(kept, &project.palette, settings, scale, tools.frame)));
                cached = true;
            }
            // Once it rests, the frame is drawn as it stands, with what only the window shows.
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(SCRUB));
        } else if let (true, false, false, Some(cache), Some(kept)) = (self.playing, self.flat, self.black, self.cache.as_mut(), played.as_ref()) {
            let scale = crate::playback::FrameCache::scale_for(self.view.zoom * ui.ctx().pixels_per_point());
            let picture = cache.frame(kept, &project.palette, settings, scale, tools.frame);
            painter.add(on_frame(picture));
            let coming: Vec<u32> = self.ahead.iter().copied().filter(|frame| !cache.has(*frame)).take(CACHED_A_FRAME).collect();
            for frame in coming {
                cache.frame(kept, &project.palette, settings, scale, frame);
            }
            cached = true;
        }
        // Standing still and left alone for a moment, the frames are made ahead of playing, a
        // few on each frame of the screen, from the playhead on and round the stretch that
        // plays, for as many as the budget has room: so that the first time round is smooth.
        let timer = crate::timings::timer("viewport: playback cache");
        self.cached_frames.clear();
        if let (false, false, Some(cache), Some(played)) = (self.flat, self.black, self.cache.as_mut(), played.as_ref()) {
            let scale = crate::playback::FrameCache::scale_for(self.view.zoom * ui.ctx().pixels_per_point());
            // The window is in use: a button is down, the pointer moves, or something came
            // in, a key or a turn of the wheel.
            let (now, held) = ui.input(|i| (i.time, i.pointer.any_down() || i.pointer.is_moving() || !i.events.is_empty()));
            let of = (Arc::as_ptr(played) as usize, Arc::as_ptr(&project.palette) as usize, scale.to_bits());
            if held || self.stroke.is_some() || self.settled.is_none_or(|(was, _)| was != of) {
                self.settled = Some((of, now));
            }
            let waited = self.settled.is_some_and(|(_, since)| now - since >= SETTLE);
            if !self.playing && self.mixed.is_none() {
                let (first, after) = played.played();
                let (span, from) = (after.saturating_sub(first).max(1), tools.frame.clamp(first, after.saturating_sub(1).max(first)));
                let made = cache.made_of(played, &project.palette, settings, scale);
                let mut missing = (0..span).map(|step| first + (from - first + step) % span).filter(|frame| made.binary_search(frame).is_err());
                if waited {
                    let began = std::time::Instant::now();
                    for frame in missing.by_ref().take(CACHED_A_FRAME) {
                        if cache.full(settings, scale) {
                            break;
                        }
                        let from = began.elapsed().as_secs_f64();
                        cache.frame(played, &project.palette, settings, scale, frame);
                        // How long the slowest took, for the app to say if it is long.
                        let took = began.elapsed().as_secs_f64();
                        self.cache_took = Some(self.cache_took.map_or(took - from, |slowest| slowest.max(took - from)));
                        if took >= CACHE_TIME {
                            break;
                        }
                    }
                }
                // More to make: on the next frame of the screen, or once it has been left alone.
                if missing.next().is_some() && !cache.full(settings, scale) {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(if waited { 0.0 } else { SETTLE }));
                }
            }
            self.cached_frames = cache.made_of(played, &project.palette, settings, scale);
        }
        drop(timer);
        #[cfg(test)]
        {
            self.from_cache = cached;
            self.bent_tiles = 0;
            self.ghosts_drawn = 0;
            self.painted_art.clear();
        }
        // Standing still, on the card if that is asked for: the compositor's picture of the
        // part that is seen, at the screen's own pixels, with what only the window shows.
        self.from_card = false;
        // A Blend that is not laid on over what is under it can't be shown over tiles, which
        // have its picture alone: a shot with one is drawn on the card whatever the switch says.
        let blends = shot.as_ref().is_some_and(|shot| {
            shot.effects.iter().any(|effect| effect.enabled && effect.kind == iskra_model::BLEND && effect.choice("mode", tools.frame) != "Normal")
        });
        let timer = crate::timings::timer("viewport: card");
        if let (true, false, false, false, Some(shot)) = ((self.on_card || blends) && self.cache.is_some(), cached, self.flat, self.black, shot) {
            let points = ui.ctx().pixels_per_point();
            let (wide, high) = ((rect.width() * points).round(), (rect.height() * points).round());
            if (1.0..=8192.0).contains(&wide) && (1.0..=8192.0).contains(&high) {
                // From the frame's units to the picture's pixels, as the view has them.
                let (zoom, (sin, cos)) = (f64::from(self.view.zoom * points), f64::from(self.view.rotation).sin_cos());
                let origin = (center + self.view.pan - rect.min) * points;
                let from_frame = Affine { m: [zoom * cos, zoom * sin, -zoom * sin, zoom * cos, f64::from(origin.x), f64::from(origin.y)] };
                let frame = [f64::from(settings.width), f64::from(settings.height)];
                let view = iskra_render::View { width: wide as u32, height: high as u32, from_frame, frame };
                let theme = Theme::of(ui.ctx());
                let onion = [theme.colour(Role::OnionBefore), theme.colour(Role::OnionAfter)];
                let (mut own, looks) = self.windowed(shot, layer, tools, display, onion);
                // A colour being picked for a swatch: the palette as the shot shows it, with
                // that colour, and no style for the card to show it in again.
                let palette = match self.recolouring {
                    Some(_) if !Arc::ptr_eq(&colours, &project.palette) => {
                        own.styles.clear();
                        colours.clone()
                    }
                    _ => project.palette.clone(),
                };
                if let Some(cache) = self.cache.as_mut() {
                    let picture = cache.window(&own, &palette, settings.working_space, tools.frame, &view, &looks);
                    let mut mesh = egui::Mesh::with_texture(picture);
                    let places = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
                    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
                    mesh.vertices.extend(places.iter().zip(uvs).map(|(&pos, [u, v])| Vertex { pos, uv: Pos2::new(u, v), color: Color32::WHITE }));
                    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
                    painter.add(Shape::mesh(mesh));
                    self.from_card = true;
                }
            }
        }
        drop(timer);
        let timer = crate::timings::timer("viewport: layers from tiles");
        let current = layer;
        // Only what the display shows is seen, bottom first.
        let flat = self.flat;
        // Through the camera, what is behind it is not seen.
        // The flat view shows every layer as it lies, tilted or not.
        let place = move |placed: &iskra_eval::Placed| if flat { Some(Projection::flat(&placed.to_scene)) } else { placed.projection };
        // Only the work that is shown; and what the cache has shown is not drawn again from tiles.
        let (work, hidden) = (self.work, self.hidden.clone());
        let wanted = |board: bool| {
            let shown = match work {
                Work::Boards => board,
                Work::Animation => !board,
                Work::Whole => true,
            };
            shown && !cached
        };
        // What is drawn, in its order: a layer from its tiles, or what goes through an effect
        // node as the one picture the node makes of it. The layers of such a picture follow
        // it, for what is only in the window: the onion skin and the stroke being drawn.
        // With each layer, the one art layer it is of where it comes from that art layer's output.
        let mut seen: Vec<(Option<&iskra_eval::Card>, Option<(&Shot, &iskra_model::Layer, Projection, Option<ArtLayerKind>)>)> = Vec::new();
        // A composite's flat card is drawn as the cards in it.
        for (s, card) in shot.into_iter().flat_map(|s| shown.cards.iter().flat_map(iskra_eval::Card::leaves).map(move |card| (s, card))) {
            let layers = card.placed();
            let layers = layers
                .iter()
                .filter_map(|placed| Some((s, s.layer(placed.layer)?, place(placed)?, placed.art)))
                .filter(|(_, layer, ..)| !hidden.contains(&layer.id));
            match card {
                // A picture the camera takes: the flat view shows the layers as they lie.
                iskra_eval::Card::Through { .. } if !flat => {
                    // It is a board's if all that goes through is boards.
                    // A picture an effect makes itself, with nothing going through, is animation.
                    let placed = card.placed();
                    let board = !placed.is_empty() && placed.iter().all(|placed| s.is_board(placed.layer));
                    if wanted(board) {
                        seen.push((Some(card), None));
                        seen.extend(layers.map(|layer| (Some(card), Some(layer))));
                    }
                }
                _ => seen.extend(layers.filter(|(shot, layer, ..)| wanted(shot.is_board(layer.id))).map(|layer| (None, Some(layer)))),
            }
        }
        #[cfg(test)]
        {
            self.through_shown = (0, 0);
        }
        // While it plays, only what is in the camera's frame is seen, as on the frames that
        // are kept: a frame that is drawn from its tiles for once is cut to the frame too.
        let framed = egui::Rect::from_points(&corners).intersect(painter.clip_rect());
        let layers_painter = if self.playing && !flat { painter.with_clip_rect(framed) } else { painter.clone() };
        for (card, layer) in seen {
            let painter = &layers_painter;
            // The picture of what goes through an effect node, laid over the camera's frame.
            let (Some((shot, layer, place, only)), through) = (layer, card.is_some()) else {
                // On the card, it is in the picture already.
                if self.from_card {
                    continue;
                }
                let (Some(iskra_eval::Card::Through { effect, plane, .. }), Some(shot)) = (card, shot) else { continue };
                // A Particles node in sheets has a picture for each.
                let key = (*effect, *plane);
                let scale = crate::playback::FrameCache::scale_for(self.view.zoom * ui.ctx().pixels_per_point());
                let view = iskra_render::View::frame(settings, scale);
                // Of the shot as the project has it, the picture is kept; of one that is being
                // posed it is made as the pose goes.
                let of = kept.map(|kept| ThroughOf {
                    shot: kept.clone(),
                    palette: project.palette.clone(),
                    frame: tools.frame,
                    size: (view.width, view.height),
                    space: settings.working_space,
                });
                if !self.through.get(&key).is_some_and(|made| of.is_some() && made.of == of) {
                    let Some(card) = card else { continue };
                    // On the card where there is one, as while the shot plays; else by the
                    // reference, on the processor.
                    let texture = match self.cache.as_mut() {
                        Some(cache) => Made::Card(cache.card(key, shot, &project.palette, settings.working_space, tools.frame, &view, card)),
                        None => {
                            let made = self.made.card(shot, &project.palette, settings.working_space, tools.frame, &view, card);
                            let bytes: Vec<u8> = made.pixels.iter().flatten().copied().collect();
                            let image = egui::ColorImage::from_rgba_unmultiplied([made.width as usize, made.height as usize], &bytes);
                            Made::Window(ui.ctx().load_texture("through", image, egui::TextureOptions::LINEAR))
                        }
                    };
                    self.through.insert(key, ThroughPicture { of, texture, used: self.drawn });
                    #[cfg(test)]
                    {
                        self.through_shown.1 += 1;
                    }
                }
                if let Some(made) = self.through.get_mut(&key) {
                    made.used = self.drawn;
                    painter.add(on_frame(made.texture.id()));
                    #[cfg(test)]
                    {
                        self.through_shown.0 += 1;
                    }
                }
                continue;
            };
            let is_current = current == Some(layer.id);
            // How the layer's deformer bends its drawings on a frame, if it has one that does.
            // While the Bones tool lays a chain at rest, its drawing is shown as it was drawn;
            // the other layers' bones stay posed.
            let at_rest = tools.rest() && is_current;
            let bent = |frame: u32| warp_of(shot, layer, frame, !at_rest);
            // The onion skin of the layer being drawn on, and of those picked with it, under its
            // drawing. Each ghost is where the pegs had the layer on its own frame.
            let placed = |frame: u32| space(shot, flat, layer.parent, layer.parent, frame).m;
            let keys = Motion::keys_of(shot, layer);
            // Not of a layer seen in perspective: its ghosts would lie flat.
            let ghosted = self.onioned.contains(&layer.id) && place.as_flat().is_some();
            let found = if ghosted { ghosts(layer, tools.frame, display, &Motion { keys, placed: &placed, bent: &bent }) } else { Vec::new() };
            for ghost in found {
                #[cfg(test)]
                {
                    self.ghosts_drawn += 1;
                }
                let theme = Theme::of(painter.ctx());
                let onion = [theme.colour(Role::OnionBefore), theme.colour(Role::OnionAfter)];
                let look =
                    Look { color: ghost.color(display.opacity, onion), silhouette: true, outline: display.outline, shift: display.shift_of(ghost.drawing) };
                let place = Projection::flat(&Placement::of(shot, layer, ghost.frame, flat).to_scene);
                // Vector art is bent point by point, where the drawing is all vector art.
                let ghost_bent = layer.drawing(shot.shown(layer, ghost.drawing)).map(|d| iskra_render::bent_points(&d.drawing, bent(ghost.frame)));
                let ghost_warp = ghost_bent.as_ref().and_then(|(_, warp)| warp.clone());
                let art_layers = ghost_bent.iter().flat_map(|(drawing, _)| &drawing.art_layers);
                // Nor the one that is not shown alone, as on the card.
                let alone = display.shown_art.filter(|_| display.shown_art_all || is_current);
                let art_layers = art_layers
                    .filter(|art| !layer.art_off.contains(&art.kind) && alone.is_none_or(|kind| art.kind == kind) && only.is_none_or(|kind| art.kind == kind));
                for art in art_layers.filter(|l| l.kind == ArtLayerKind::LineArt || !display.line_art_only) {
                    self.paint_layer(ui, painter, Pixels::of(&art.content), &colours, &place, ghost_warp.as_ref(), look);
                }
            }
            let warp = bent(tools.frame);
            let warp = warp.as_ref();
            // A board under the animation is reference, unless it is the layer in hand and may be drawn on.
            let reference = fading && shot.is_board(layer.id) && (!is_current || self.locked.is_some());
            let look = match (reference, display.light_table && current.is_some() && !is_current) {
                (true, _) => Look::faded(BOARD_OPACITY),
                (false, true) => Look::faded(LIGHT_TABLE_OPACITY),
                (false, false) => Look::PLAIN,
            };
            let live = self.live_art(layer.id, tools.frame);
            // As the skin the shot is shown in has it.
            let exposed = layer.shown_at(shot.skin, tools.frame);
            // Its art layers as the deformer bends them: vector art point by point where the
            // drawing is all vector art, with nothing left to bend; else through the mesh.
            let bent_exposed = exposed.map(|d| iskra_render::bent_points(&d.drawing, warp.cloned()));
            let art_warp = bent_exposed.as_ref().map_or(warp, |(_, bent)| bent.as_ref());
            // Its gradients' and pictures' areas, where the palette has any it is painted with.
            let areas = bent_exposed.as_ref().and_then(|(drawing, _)| self.areas_of(drawing, &colours));
            // Vector art seen larger than its pixels, drawn at the size it is seen on screen;
            // not one with gradients, which lie where its pixels are.
            let on_screen = Affine::scale(f64::from(self.view.zoom * ui.ctx().pixels_per_point()));
            let sharpened = bent_exposed
                .as_ref()
                .filter(|_| areas.is_none())
                .and_then(|(drawing, bent)| iskra_render::sharpened(drawing, bent.is_some(), &place, &on_screen));
            let art_place = sharpened.as_ref().map_or(place, |(_, sharp)| *sharp);
            let shown_drawing = sharpened.as_ref().map(|(drawing, _)| drawing).or(bent_exposed.as_ref().map(|(drawing, _)| drawing));
            for (index, art) in shown_drawing.into_iter().flat_map(|drawing| &drawing.art_layers).enumerate() {
                // One art layer alone, if that is asked for: of the drawing in hand, or of all.
                if display.shown_art.is_some_and(|kind| art.kind != kind) && (is_current || display.shown_art_all) {
                    continue;
                }
                // Switched off on its layer, or another art layer than the output it comes from.
                if layer.art_off.contains(&art.kind) || only.is_some_and(|kind| art.kind != kind) {
                    continue;
                }
                #[cfg(test)]
                self.painted_art.push(art.kind);
                // While a selection handle is dragged, the art layer shows the result so far.
                if let Some(preview) = self.dragging().filter(|_| is_current).and_then(|d| d.preview.get(index).cloned().flatten()) {
                    self.paint_layer(ui, painter, Pixels::of(&preview), &colours, &place, warp, look);
                    continue;
                }
                match (&art.content, &live) {
                    (_, Some((kind, live))) if art.kind == *kind => self.paint_layer(ui, painter, live.pixels(), &colours, &place, warp, look),
                    // What goes through an effect node is in the node's picture already.
                    _ if through => {}
                    // A picture that was brought in is drawn as it is, with the rest.
                    (content, _) => {
                        let pixels = match (content.indexed(), &areas) {
                            (Some(surface), Some(areas)) => {
                                Pixels::Shaded { surface, art: art.kind, areas, linear: settings.working_space == iskra_model::WorkingSpace::Linear }
                            }
                            _ => Pixels::of(content),
                        };
                        self.paint_layer(ui, painter, pixels, &colours, &art_place, art_warp, look)
                    }
                }
            }
            // A stroke in an empty cell: its drawing exists only once the stroke is committed.
            if let (None, Some((_, live))) = (exposed, &live) {
                self.paint_layer(ui, painter, live.pixels(), &colours, &place, warp, look);
            }
            // Shown too large for its pixels: what is painted is outlined.
            if let (false, Some(entry), true) = (flat, exposed, self.coarse.iter().any(|(coarse, _)| *coarse == layer.id)) {
                let bounds = entry.drawing.bounds();
                let corner = |x: i32, y: i32| {
                    let at = place.apply(Point::new(f64::from(x), f64::from(y)))?;
                    Some(self.view.to_screen(center, [at.x, at.y]))
                };
                let around: Option<Vec<Pos2>> = [(bounds.x0, bounds.y0), (bounds.x1, bounds.y0), (bounds.x1, bounds.y1), (bounds.x0, bounds.y1)]
                    .into_iter()
                    .map(|(x, y)| corner(x, y))
                    .collect();
                if let Some(around) = around {
                    painter.add(Shape::closed_line(around, Outline::new(1.5, Theme::of(painter.ctx()).colour(Role::Coarse))));
                }
            }
        }
        drop(timer);
        if let Some(kept) = kept.filter(|_| !cached && !self.from_card) {
            let _timer = crate::timings::timer("viewport: textures ahead");
            self.warm(ui, kept, &colours);
        }
        // The pictures of effects that are no longer seen go.
        let drawn = self.drawn;
        self.through.retain(|_, made| made.used == drawn);
        if let Some(cache) = self.cache.as_mut() {
            cache.keep_cards(|key| self.through.contains_key(&key));
        }
        // Past the budget, the textures drawn longest ago go.
        if self.textures.len() > TEXTURE_BUDGET {
            let mut used: Vec<u64> = self.textures.values().map(|texture| texture.used).collect();
            used.sort_unstable_by(|a, b| b.cmp(a));
            let oldest_kept = used[TEXTURE_BUDGET - 1];
            self.textures.retain(|_, texture| texture.used >= oldest_kept);
        }

        // Cut to the camera's frame: what is outside it covered, whichever way the frame lies.
        #[cfg(test)]
        {
            self.clipped = None;
        }
        if let (Some(surround), false) = (self.clip, flat) {
            painter.add(outside(corners, painter.clip_rect(), surround));
            #[cfg(test)]
            {
                self.clipped = Some(corners);
            }
        }
        if let Some(seen) = self.camera_outline {
            painter.add(Shape::closed_line(seen.to_vec(), Outline::new(1.5, Color32::from_rgb(230, 120, 40))));
        }
        // The Camera tool's handles: a square on each corner of the camera's frame, and while
        // one is dragged the frame as it would be.
        #[cfg(test)]
        {
            self.frame_handles.clear();
            self.frame_preview = None;
        }
        // While the film plays, what a tool draws over it is put away: the handles, the bones,
        // the selection and the motion path. The guides and the mask are the view's, and stay.
        // Where the view is only looked at, the Export page's player, they are never drawn.
        let tooling = !self.playing && !self.look_only;
        if let (Tool::Camera, false, true, Some(shot)) = (tools.tool, self.black, tooling, shot) {
            let dark = Color32::from_gray(30);
            let square = |at: Pos2, fill: Color32| {
                painter.rect(egui::Rect::from_center_size(at, egui::vec2(8.0, 8.0)), 0.0, fill, Outline::new(1.0, dark), egui::StrokeKind::Middle)
            };
            let unit = [[-w, -h], [w, -h], [w, h], [-w, h]];
            let on_screen = |scale: f64, centre: [f64; 2]| -> Option<Vec<Pos2>> {
                unit.iter().map(|[x, y]| self.frame_point(center, shot, tools.frame, [centre[0] + x * scale, centre[1] + y * scale])).collect()
            };
            let (handles, preview) = (on_screen(1.0, [0.0, 0.0]), self.frame_drag.and_then(|drag| on_screen(drag.scale, drag.centre)));
            for at in handles.iter().flatten() {
                square(*at, Color32::WHITE);
            }
            if let Some(preview) = &preview {
                painter.add(Shape::closed_line(preview.clone(), Outline::new(1.5, Color32::from_rgb(230, 120, 40))));
                for at in preview {
                    square(*at, Color32::from_rgb(230, 120, 40));
                }
            }
            #[cfg(test)]
            {
                self.frame_handles = handles.unwrap_or_default();
                self.frame_preview = preview.and_then(|preview| preview.try_into().ok());
            }
        }
        // The guides lie on the camera's frame: the paper, or in the flat view its outline.
        if !self.black && (shot.is_some() || !self.flat) {
            let frame = self.camera_outline.filter(|_| self.flat).unwrap_or(corners);
            let frame_aspect = f64::from(settings.width) / f64::from(settings.height.max(1));
            // The mask first, under the lines: what is outside the picture's shape is dimmed.
            let bands = self.aspect_mask.map_or(Vec::new(), |aspect| mask_bands(aspect, frame, frame_aspect));
            let theme = Theme::of(painter.ctx());
            for band in &bands {
                painter.add(Shape::convex_polygon(band.to_vec(), theme.colour(Role::Mask), Outline::NONE));
            }
            #[cfg(test)]
            {
                self.masked = bands;
            }
            let [r, g, b, _] = theme.colour(Role::Guides).to_srgba_unmultiplied();
            for line in self.guides.iter().flat_map(|guide| guide_lines(*guide, frame)) {
                painter.add(Shape::line(line, Outline::new(1.0, Color32::from_rgba_unmultiplied(r, g, b, iskra_export::burn::GUIDE_ALPHA))));
            }
            for guide in self.user_guides.iter().filter(|guide| guide.shown) {
                let [r, g, b] = guide.colour;
                for line in own_guide_lines(guide.kind, frame, frame_aspect) {
                    painter.add(Shape::line(line, Outline::new(1.0, Color32::from_rgba_unmultiplied(r, g, b, iskra_export::burn::OWN_GUIDE_ALPHA))));
                }
            }
            #[cfg(test)]
            self.burned_drawn.clear();
            if let Some((burn, name, counter)) = self.burned.clone() {
                self.paint_burned(&painter, frame, &burn, name.as_deref(), &counter);
            }
        }
        #[cfg(test)]
        {
            // The display it looks through, by its name: the posed shot still has its displays.
            self.viewing_said = shot.and_then(|shot| self.looking_through(shot).and_then(|display| shot.display_name_of(display)).map(str::to_string));
        }
        if let (Tool::Bones | Tool::Transform, true, Some(shot)) = (tools.tool, tooling, shot) {
            self.paint_bones(&painter, shot, current, tools);
        }
        if let Some(shot) = shot {
            self.paint_emitter(&painter, shot, tools.frame);
        }
        if let (true, true, Some(shot)) = (self.bones_always, tooling, shot) {
            let whole = current.filter(|_| tools.tool == Tool::Bones);
            self.paint_faint_bones(&painter, shot, whole, tools.frame);
        } else {
            self.faint_chains.clear();
        }
        let current = shot.and_then(|shot| Some(Placement::of(shot, shot.layer(current?)?, tools.frame, flat)));
        if tooling {
            self.paint_selection(&painter, current);
        }
        #[cfg(test)]
        {
            self.animating_outline = None;
        }
        if self.animating {
            let outline = rect.shrink(1.0);
            painter.rect_stroke(outline, 0.0, Outline::new(2.0, Theme::of(painter.ctx()).colour(Role::Animate)), egui::StrokeKind::Inside);
            #[cfg(test)]
            {
                self.animating_outline = Some(outline);
            }
        }
        if let Some(Zooming { from, to, boxing: true, .. }) = self.zooming {
            // Dark, as the selection's.
            painter.rect_stroke(egui::Rect::from_two_pos(from, to), 0.0, Outline::new(1.0, Color32::from_gray(30)), egui::StrokeKind::Middle);
        }
        #[cfg(test)]
        {
            self.pivot_on_screen = None;
        }
        let handled = shot.filter(|_| tooling).and_then(|shot| self.peg_handles(center, shot, peg?, tools.frame));
        if let (Tool::Transform | Tool::Camera, Some((pivot, handle))) = (tools.tool, handled) {
            #[cfg(test)]
            {
                self.pivot_on_screen = Some(pivot);
            }
            // Elsewhere than where a press turns or skews, the pointer moves the peg.
            if response.hovered() && !panning && (tools.tool != Tool::Transform || self.hover_handle.is_none()) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
            }
            // Dark, like the selection's outline; a temporary pivot is orange.
            let temporary = tools.tool == Tool::Transform && self.temp_pivot.is_some_and(|(of, _)| Some(of) == peg);
            let dark = if temporary { Color32::from_rgb(230, 120, 20) } else { Color32::from_gray(30) };
            let pen = Outline::new(1.0, dark);
            painter.line_segment([pivot, handle], pen);
            painter.circle_stroke(pivot, 6.0, pen);
            painter.line_segment([pivot - egui::vec2(9.0, 0.0), pivot + egui::vec2(9.0, 0.0)], pen);
            painter.line_segment([pivot - egui::vec2(0.0, 9.0), pivot + egui::vec2(0.0, 9.0)], pen);
            painter.circle(handle, 4.5, Color32::WHITE, Outline::new(1.0, Color32::from_gray(30)));
        }
        // The box round what the peg carries: a square on each corner and each side scales it.
        #[cfg(test)]
        self.scale_handles.clear();
        let boxed = shot.filter(|_| tooling).zip(self.scale_box);
        if let Some(handles) = boxed.and_then(|(shot, (peg, bounds))| self.box_on_screen(center, shot, peg, tools.frame, bounds)) {
            let dark = Color32::from_gray(30);
            let outline: Vec<Pos2> = handles[..4].iter().chain(&handles[..1]).copied().collect();
            painter.add(Shape::line(outline, Outline::new(1.0, dark)));
            for at in handles {
                painter.rect(egui::Rect::from_center_size(at, egui::vec2(7.0, 7.0)), 0.0, Color32::WHITE, Outline::new(1.0, dark), egui::StrokeKind::Middle);
            }
            #[cfg(test)]
            self.scale_handles.extend(handles);
            // Where a press turns the peg, the pointer is a curved arrow.
            if let (Tool::Transform, Some(Handle::Rotate), Some(at)) = (tools.tool, self.hover_handle, self.pointer) {
                paint_turn_cursor(&painter, at, self.turn_facing);
            }
        }
        #[cfg(test)]
        {
            (self.path_keys, self.bend_dots, self.path_dots) = (Vec::new(), None, Vec::new());
        }
        let pathed = shot.filter(|_| tooling).and_then(|shot| Some((shot, shot.peg(peg?)?)));
        if let (Tool::Transform | Tool::Camera, Some((shot, peg))) = (tools.tool, pathed) {
            let dark = Color32::from_gray(30);
            // While an in-between is slid along it, the path's dots are as the drag has them.
            let sliding = self.peg_drag.or(self.peg_held).filter(|drag| drag.peg == peg.id).and_then(|drag| match drag.grab {
                Grab::Tween { key, .. } => Some(key),
                _ => None,
            });
            let charted = sliding.zip(self.tween.as_ref()).map(|(key, (_, now))| {
                let mut charted = peg.clone();
                charted.animation.position.set_chart(key, now);
                charted
            });
            let path = self.peg_path(center, shot, charted.as_ref().unwrap_or(peg), tools.frame);
            let line: Vec<Pos2> = path.iter().map(|(_, at, _)| *at).collect();
            painter.add(Shape::line(line, Outline::new(1.0, dark)));
            // A dot a frame shows the spacing; a square is a keyframe, filled on the playhead.
            for (frame, at, key) in &path {
                if *key {
                    let fill = if *frame == tools.frame { dark } else { Color32::WHITE };
                    painter.rect(egui::Rect::from_center_size(*at, egui::vec2(7.0, 7.0)), 0.0, fill, Outline::new(1.0, dark), egui::StrokeKind::Middle);
                } else {
                    painter.circle_filled(*at, 2.0, dark);
                }
            }
            let handles = self.bend_handles(center, shot, peg, tools.frame);
            if let Some((key, into, out)) = handles {
                for end in [into, out].into_iter().filter(|end| (*end - key).length() > 1.0) {
                    painter.line_segment([key, end], Outline::new(1.0, dark));
                    painter.circle(end, 3.5, Color32::WHITE, Outline::new(1.0, dark));
                }
            }
            #[cfg(test)]
            {
                self.path_keys = path.iter().filter(|(.., key)| *key).map(|(frame, at, _)| (*frame, *at)).collect();
                self.path_dots = path.iter().filter(|(.., key)| !*key).map(|(frame, at, _)| (*frame, *at)).collect();
                self.bend_dots = handles.map(|(_, into, out)| (into, out));
            }
        }

        if let Some(guide) = self.held_guide.clone().filter(|_| tooling) {
            self.paint_guide(&painter, rect, &guide);
        }

        // The point editor's points and handles over the drawing.
        if tools.tool == Tool::Points
            && tooling
            && let Some(shot) = shot
            && let Some(layer) = layer.and_then(|id| shot.layer(id))
            && let Some(space) = self.drawing_space(&Placement::of(&self.posed(shot), layer, tools.frame, self.flat), center)
        {
            let (to_screen, to_drawing) = (|at| space.to_screen(at), |pos| space.to_drawing(pos));
            let map = crate::points::Map { to_screen: &to_screen, to_drawing: &to_drawing };
            self.points.paint(&painter, layer, tools.frame, tools.art, &map, crate::theme::Theme::of(ui.ctx()).accent);
        }
        // The Gradient/Texture tool's handles on the area shown.
        if tools.tool == Tool::Gradient
            && tooling
            && let Some(shot) = shot
            && let Some(layer) = layer.and_then(|id| shot.layer(id))
            && let Some(space) = self.drawing_space(&Placement::of(&self.posed(shot), layer, tools.frame, self.flat), center)
        {
            let (to_screen, to_drawing) = (|at| space.to_screen(at), |pos| space.to_drawing(pos));
            let map = crate::points::Map { to_screen: &to_screen, to_drawing: &to_drawing };
            self.placing.paint(&painter, &map, crate::theme::Theme::of(ui.ctx()).accent);
        }
        // The Text tool's boxes, the one typed in with its cursor.
        if tools.tool == Tool::Text
            && tooling
            && let Some(shot) = shot
            && let Some(layer) = layer.and_then(|id| shot.layer(id)).filter(|layer| layer.text)
            && let Some(space) = self.drawing_space(&Placement::of(&self.posed(shot), layer, tools.frame, self.flat), center)
        {
            let (to_screen, to_drawing) = (|at| space.to_screen(at), |pos| space.to_drawing(pos));
            let map = crate::points::Map { to_screen: &to_screen, to_drawing: &to_drawing };
            self.text.paint(ui, &painter, layer, tools.frame, &map, crate::theme::Theme::of(ui.ctx()).accent);
        }
        // The brush outline under the pointer.
        // Only where there is something to draw on: not between shots, nor with a peg selected.
        let brushing = |_: &Pos2| tooling && !panning && layer.is_some() && matches!(tools.tool, Tool::Brush | Tool::Eraser | Tool::Shape);
        if let Some(pointer) = response.hover_pos().filter(brushing) {
            let vector = shot.zip(layer).and_then(|(shot, layer)| shot.layer(layer)).is_some_and(|layer| draws_vector(layer, tools));
            let size = if vector { brush.vector.brush.size } else { brush.raster.brush.size };
            painter.circle_stroke(pointer, size * self.view.zoom / 2.0, Outline::new(1.0, Color32::from_gray(128)));
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        // Where the bucket has been dragged, until it is let go and fills.
        if tools.tool == Tool::Fill && tooling && self.fill_drag.len() > 1 {
            let trail: Vec<Pos2> = self.fill_drag.iter().map(|(pos, _)| *pos).collect();
            painter.add(Shape::line(trail.clone(), Outline::new(3.0, Color32::from_black_alpha(120))));
            painter.add(Shape::line(trail, Outline::new(1.0, Color32::WHITE)));
        }
        if matches!(tools.tool, Tool::Fill | Tool::Eyedropper) && tooling && layer.is_some() && !panning && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        // The pointer is the tool's icon where the tool works on what is under it, unless a
        // handle has a pointer of its own there: a turn, a scale.
        let needs_layer = tools.tool != Tool::Camera;
        #[cfg(test)]
        {
            self.pointer_icon = None;
        }
        if let Some((icon, hot)) = tool_pointer(tools.tool).filter(|_| tooling && !panning && (layer.is_some() || !needs_layer))
            && let Some(pointer) = response.hover_pos()
            && ui.ctx().output(|o| match o.cursor_icon {
                egui::CursorIcon::Default | egui::CursorIcon::Crosshair => true,
                // The camera is moved by a drag anywhere: its icon in place of the Move pointer.
                egui::CursorIcon::Move => tools.tool == Tool::Camera,
                _ => false,
            })
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            let accent = crate::theme::Theme::of(ui.ctx()).accent;
            let at = pointer - egui::vec2(hot[0], hot[1]) * (POINTER_SIZE / 24.0);
            egui::Area::new(egui::Id::new("tool pointer")).order(egui::Order::Tooltip).fixed_pos(at).interactable(false).show(ui.ctx(), |ui| {
                ui.add(crate::icons::image(icon, POINTER_SIZE, accent));
            });
            // The point that acts, a dot.
            ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("tool pointer dot"))).circle_filled(pointer, 1.5, accent);
            #[cfg(test)]
            {
                self.pointer_icon = Some(icon);
            }
        }
        command
    }

    /// The texts `burn` puts on the frame whose corners on screen are `frame`, as the export's
    /// [`iskra_export::burn::burn`] lays them: the same places and sizes, in the same typeface.
    fn paint_burned(&mut self, painter: &egui::Painter, frame: [Pos2; 4], burn: &iskra_export::BurnIn, name: Option<&str>, counter: &str) {
        let burn = burn.fitted();
        let (origin, width, height) = (frame[0], (frame[1] - frame[0]).length(), (frame[3] - frame[0]).length());
        let px = burn.size * height;
        if px < 2.0 {
            return;
        }
        let font = egui::FontId::monospace(px);
        for (text, spot) in burn.texts(name, counter) {
            let pad = iskra_export::burn::PADDING * px;
            let light =
                painter.layout_no_wrap(text.clone(), font.clone(), Color32::from_white_alpha(if burn.boxed { 255 } else { (burn.opacity * 255.0) as u8 }));
            let size = light.size() + egui::vec2(2.0 * pad, 2.0 * pad);
            let [x, y] = burn.place(spot, size.x, size.y, width, height);
            let around = egui::Rect::from_min_size(origin + egui::vec2(x, y), size);
            let alpha = (burn.opacity * 255.0).round() as u8;
            if burn.boxed {
                painter.rect_filled(around, 0.0, Color32::from_black_alpha(alpha));
            } else {
                // An outline, as the render strokes the letters dark under them.
                let dark = painter.layout_no_wrap(text.clone(), font.clone(), Color32::from_black_alpha(alpha));
                let by = px / 16.0;
                for (dx, dy) in [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)] {
                    painter.galley(around.min + egui::vec2(pad + dx * by, pad + dy * by), dark.clone(), Color32::BLACK);
                }
            }
            painter.galley(around.min + egui::vec2(pad, pad), light, Color32::WHITE);
            #[cfg(test)]
            self.burned_drawn.push((text, around));
        }
    }

    /// The guide in hand, and a drag of one of its handles. `None` when the guide has nothing
    /// to do with the pen; `Some` while a handle is dragged, with the command that sets the
    /// guide when the handle is let go.
    fn guide_input(&mut self, response: &egui::Response, shot: Option<&Shot>, tools: &Tools, panning: bool) -> Option<Option<(&'static str, Value)>> {
        let drawing = matches!(tools.tool, Tool::Brush | Tool::Eraser | Tool::Shape);
        let found = self.guide.filter(|_| drawing).and_then(|id| shot?.guides.iter().find(|guide| guide.id == id));
        let Some(found) = found else {
            (self.held_guide, self.guide_drag) = (None, None);
            return None;
        };
        let center = response.rect.center();
        if let (None, false, true, Some(pos)) = (&self.guide_drag, panning, response.drag_started_by(PointerButton::Primary), response.interact_pointer_pos()) {
            let near = |handle: &[f64; 2]| (self.view.to_screen(center, *handle) - pos).length() <= HANDLE_REACH;
            let index = found.kind.handles().iter().position(near);
            self.guide_drag = index.map(|index| (found.id, index, found.kind.clone()));
        }
        let Some((id, index, kind)) = self.guide_drag.take().filter(|(id, ..)| *id == found.id) else {
            self.held_guide = Some(found.kind.clone());
            return None;
        };
        if response.dragged_by(PointerButton::Primary) || response.drag_started_by(PointerButton::Primary) {
            let kind = response.interact_pointer_pos().map_or(kind.clone(), |pos| found.kind.with_handle(index, self.view.to_scene(center, pos)));
            self.held_guide = Some(kind.clone());
            self.guide_drag = Some((id, index, kind));
            return Some(None);
        }
        // Let go: the guide is where the handle was taken, shown so until the project has it.
        self.held_guide = Some(kind.clone());
        let params = match kind {
            GuideKind::Ruler { at, angle } => json!({ "guide": id.0, "at": at, "angle": angle }),
            GuideKind::Perspective { points } => json!({ "guide": id.0, "points": points }),
        };
        Some(Some(("guide.set", params)))
    }

    /// Draw the guide in hand over the picture: a ruler's edge, or the lines that run to the
    /// vanishing points, with the handles that place it.
    fn paint_guide(&self, painter: &egui::Painter, rect: egui::Rect, guide: &GuideKind) {
        let center = rect.center();
        let (line, faint) = (Color32::from_rgb(40, 150, 230), Color32::from_rgba_unmultiplied(40, 150, 230, 70));
        let screen = |at: [f64; 2]| self.view.to_screen(center, at);
        // Far enough to leave the window whatever the zoom.
        let far = f64::from(rect.width() + rect.height()) / f64::from(self.view.zoom.max(1e-3)) * 4.0;
        let through = |at: [f64; 2], angle: f64, color: Color32| {
            let (sin, cos) = angle.sin_cos();
            painter.line_segment([screen([at[0] - far * cos, at[1] - far * sin]), screen([at[0] + far * cos, at[1] + far * sin])], Outline::new(1.0, color));
        };
        match guide {
            GuideKind::Ruler { at, angle } => through(*at, angle.to_radians(), line),
            GuideKind::Perspective { points } => {
                for point in points {
                    for ray in 0..12 {
                        through(*point, std::f64::consts::PI * f64::from(ray) / 12.0, faint);
                    }
                }
                // The horizon, where there are two points to lay it through.
                if let [left, right, ..] = points[..] {
                    painter.line_segment([screen(left), screen(right)], Outline::new(1.0, line));
                }
            }
        }
        for (index, handle) in guide.handles().into_iter().enumerate() {
            // A ruler's second handle turns it, and is a square; the others are moved.
            if matches!(guide, GuideKind::Ruler { .. }) && index == 1 {
                painter.rect(
                    egui::Rect::from_center_size(screen(handle), egui::vec2(8.0, 8.0)),
                    1.0,
                    Color32::WHITE,
                    Outline::new(1.5, line),
                    egui::StrokeKind::Inside,
                );
            } else {
                painter.circle(screen(handle), 5.0, Color32::WHITE, Outline::new(1.5, line));
            }
        }
    }

    /// The Eyedropper: a press, or a drag, finds what the drawing in hand is painted with
    /// under the pointer, the line's or the fill's where there is no line: the swatch, or on
    /// free colour, which has none, the colour seen there, its art layers over each other and
    /// whole however thin the paint is. An art layer that is not seen, switched off or with
    /// the other shown alone, is not looked at.
    fn pick_input(
        &self,
        response: &egui::Response,
        current: Option<(&Layer, Placement)>,
        tools: &Tools,
        display: &Display,
        panning: bool,
    ) -> Option<std::result::Result<SwatchId, [u8; 4]>> {
        let (layer, place) = current?;
        let to_pixels = place.to_pixels?;
        if panning || !(response.drag_started_by(egui::PointerButton::Primary) || response.dragged_by(egui::PointerButton::Primary)) {
            return None;
        }
        let [x, y] = self.view.to_scene(response.rect.center(), response.interact_pointer_pos()?);
        let at = self.drawn(to_pixels.apply(Point::new(x, y)));
        let (x, y) = (at.x.floor() as i32, at.y.floor() as i32);
        let drawing = &layer.drawing_at(tools.frame)?.drawing;
        let seen = |kind: &ArtLayerKind| !layer.art_off.contains(kind) && display.shown_art.is_none_or(|shown| shown == *kind);
        // The top art layer first: what is seen there.
        ArtLayerKind::ALL.into_iter().rev().filter(seen).find_map(|kind| match &drawing.layer(kind)?.content {
            ArtContent::Rgba(surface) => {
                let mut found = [[0u8; 4]];
                surface.read_rgba8_into(iskra_model::Rect::new(x, y, x + 1, y + 1), &mut found);
                // The colour seen: of this art layer over the free colour under it.
                let under = |other: ArtLayerKind| {
                    seen(&other) && ArtLayerKind::ALL.iter().position(|k| *k == other) <= ArtLayerKind::ALL.iter().position(|k| *k == kind)
                };
                (found[0][3] > 0).then(|| drawing.free_colour_at(x, y, under)).flatten().map(Err)
            }
            // Indexed pixels, or those vector art is drawn as.
            content => content.indexed().map(|surface| surface.pixel(x, y)).filter(|pixel| pixel.coverage > 0).map(|pixel| Ok(pixel.swatch)),
        })
    }

    /// The Shape tool's polyline: each click is a corner, and the line so far is seen with
    /// the stretch to the pointer. A double click or Enter finishes it where it is; a click on
    /// its first corner finishes it closed; Backspace or Delete takes its last corner back;
    /// Escape gives it up.
    fn polyline_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        current: Option<(&Layer, Placement)>,
        tools: &Tools,
        in_hand: &BrushSettings,
        panning: bool,
    ) -> Option<(&'static str, Value)> {
        let Some((layer, place, to_pixels)) = current.and_then(|(layer, place)| Some((layer, place, place.to_pixels?))) else {
            self.polyline = None;
            return None;
        };
        // One begun on another drawing or art layer, or given up, is no more.
        let elsewhere = self.polyline.as_ref().is_some_and(|line| line.layer != layer.id || line.frame != tools.frame || line.art != tools.art);
        if elsewhere || ui.input(|i| i.key_pressed(Key::Escape)) {
            self.polyline = None;
        }
        let center = response.rect.center();
        // A place on the screen in the drawing's pixels, taken back through what bends the layer.
        let (view, bend) = (self.view, self.bend.clone());
        let in_pixels = |pos: Pos2| {
            let [x, y] = view.to_scene(center, pos);
            let at = to_pixels.apply(Point::new(x, y));
            bend.as_ref().map_or(at, |warp| warp.undo(at))
        };
        let pressed = (!panning && response.drag_started_by(PointerButton::Primary)).then(|| response.interact_pointer_pos()).flatten();
        let mut finished = response.double_clicked() && self.polyline.is_some();
        if let Some(line) = self.polyline.as_mut() {
            finished |= ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
            // The last corner taken back; with none left the polyline is given up.
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Backspace) | i.consume_key(egui::Modifiers::NONE, Key::Delete)) {
                line.corners.pop();
                if line.corners.is_empty() {
                    self.polyline = None;
                }
            }
        }
        let mut closed = tools.form.closed;
        if let Some(pos) = pressed {
            let at = in_pixels(pos);
            if self.polyline.is_none() {
                // As a stroke begins: the brush as wide on screen as its outline, and the art as it is.
                let finer = (1.0 / place.scale()) as f32;
                let mut brush = BrushSettings { size: in_hand.size * finer, erase: false, ..in_hand.clone() };
                brush.dual_brush.size *= finer;
                brush.seed = ui.input(|i| i.time).to_bits();
                let before = match layer.drawing_at(tools.frame).and_then(|d| d.drawing.layer(tools.art)).map(|l| &l.content) {
                    Some(ArtContent::Indexed(surface)) => Art::Indexed(surface.clone()),
                    Some(ArtContent::Rgba(_)) if layer.is_picture(tools.art) => return None,
                    Some(ArtContent::Rgba(surface)) => Art::Rgba(surface.clone()),
                    // Shapes are not drawn on vector art yet (plan §4.16).
                    Some(ArtContent::Vector(_)) => return None,
                    None if layer.vector.contains(&tools.art) => return None,
                    None if layer.free_colour.contains(&tools.art) => Art::Rgba(iskra_drawing::free_colour()),
                    None => Art::Indexed(IndexedSurface::new()),
                };
                let free = tools.free.filter(|_| matches!(before, Art::Rgba(_)));
                let (layer, frame, art, swatch, ink) = (layer.id, tools.frame, tools.art, tools.swatch, self.ink);
                self.polyline = Some(Polyline { layer, frame, art, swatch, brush, before, corners: Vec::new(), ink, free });
            }
            let line = self.polyline.as_mut()?;
            // A click on the first corner closes it; one where the last is adds nothing.
            let first = line.corners.first().map(|first| place.to_scene.apply(self.bend.as_ref().map_or(*first, |warp| warp.apply(*first))));
            let on_first = first.is_some_and(|first| (self.view.to_screen(center, [first.x, first.y]) - pos).length() <= HANDLE_REACH);
            if line.corners.len() >= 3 && on_first {
                (finished, closed) = (true, true);
            } else if line.corners.last().is_none_or(|last| (last.x - at.x).hypot(last.y - at.y) >= 1.0) {
                line.corners.push(at);
            }
        }
        let form = iskra_model::Form { closed, ..tools.form };
        let line = self.polyline.as_ref()?;
        if finished {
            let line = self.polyline.take()?;
            if line.corners.len() < 2 {
                return None;
            }
            self.landed = Some((line.layer, line.frame, line.art, Self::polyline_art(&line, &line.corners, form, &self.protect)));
            let corners: Vec<[f64; 2]> = line.corners.iter().map(|at| [at.x, at.y]).collect();
            let brush = serde_json::to_value(&line.brush).ok()?;
            let mut params = json!({ "layer": line.layer.0, "frame": line.frame, "artLayer": line.art, "swatch": line.swatch.0, "shape": Drawn::Polyline });
            if let (Some(params), Ok(Value::Object(form))) = (params.as_object_mut(), serde_json::to_value(form)) {
                params.extend(form);
            }
            (params["points"], params["color"], params["brush"], params["protect"]) = (json!(corners), json!(line.free), brush, json!(self.protect));
            return Some(("drawing.shape", params));
        }
        // The line so far, and on to where the pointer is.
        let mut shown = line.corners.clone();
        shown.extend(response.hover_pos().map(in_pixels));
        self.landed = Some((line.layer, line.frame, line.art, Self::polyline_art(line, &shown, form, &self.protect)));
        None
    }

    /// Whether a polyline is being clicked out: Enter, Backspace and Delete are its own then.
    pub fn polyline_open(&self) -> bool {
        self.polyline.is_some()
    }

    /// The art layer a polyline is clicked out on, with the polyline through `corners` on it.
    fn polyline_art(line: &Polyline, corners: &[Point], form: iskra_model::Form, protected: &[SwatchId]) -> Art {
        let drawn = Drawn::Polyline.stroke_through(&line.brush, corners, form);
        let inside = Drawn::Polyline.inside_through(corners, form);
        let under = inside.map(|(rect, coverage)| line.before.painted(rect, &coverage, line.swatch, line.ink, protected));
        let mut live = Live::start(under.as_ref().unwrap_or(&line.before), &drawn.brush, line.swatch, line.ink, protected);
        live.push(&drawn.points);
        live.art()
    }

    /// The paint bucket: a click fills the area under it, and a drag every area it goes
    /// over, in one step, when it is let go.
    fn fill_input(
        &mut self,
        response: &egui::Response,
        current: Option<(&Layer, Placement)>,
        tools: &Tools,
        pen: Option<PenSample>,
        panning: bool,
    ) -> Option<(&'static str, Value)> {
        let primary = egui::PointerButton::Primary;
        let (Some((layer, place)), false) = (current, panning) else {
            (self.fill_drag, self.fill_live) = (Vec::new(), None);
            return None;
        };
        let to_pixels = place.to_pixels?;
        // The gap is as wide as it is seen, whatever the pegs do; the drawing's pixels can be finer.
        let gap = (f64::from(tools.gap) / place.scale()).round().clamp(0.0, 200.0) as u32;
        // With the overart in hand, its marks are recoloured, as lines are. Plain-colour lines
        // have no swatch to give another: the bucket fills between them.
        let art = match (tools.art, tools.fill_lines && !layer.free_colour.contains(&ArtLayerKind::LineArt)) {
            (ArtLayerKind::Overart, _) => ArtLayerKind::Overart,
            (_, true) => ArtLayerKind::LineArt,
            (_, false) => ArtLayerKind::ColourArt,
        };
        if response.drag_started_by(primary) {
            // The eraser end of the pen takes paint away.
            (self.fill_erasing, self.fill_drag) = (pen.is_some_and(|p| p.eraser), Vec::new());
            // Filled as it goes on a copy of the art layer, to be seen while it is dragged.
            let unpaint = tools.unpaint || self.fill_erasing;
            let drawing = layer.drawing_at(tools.frame).filter(|_| !layer.is_picture(art)).map(|d| &d.drawing);
            // Line art that is vector art bounds a fill as the pixels it is drawn as, and line
            // art of free colour by its alpha.
            let lines = drawing.and_then(|d| d.layer(ArtLayerKind::LineArt)).map(|l| l.content.lines().into_owned()).unwrap_or_default();
            let swatch = if unpaint { SwatchId::NONE } else { tools.swatch };
            // Vector art is filled as the engine fills it, with shapes, and seen as its pixels.
            let (copy, vector) = match (drawing, drawing.and_then(|d| d.layer(art)).map(|l| &l.content)) {
                (_, Some(ArtContent::Indexed(surface))) => (Some(Art::Indexed(surface.clone())), None),
                (_, Some(ArtContent::Rgba(surface))) if art == ArtLayerKind::ColourArt => (Some(Art::Rgba(surface.clone())), None),
                (Some(drawing), Some(ArtContent::Vector(vector))) => {
                    let live = match art {
                        ArtLayerKind::LineArt | ArtLayerKind::Overart => Some(VectorLive::Lines(Box::new(drawing.as_ref().clone()))),
                        _ => iskra_drawing::VectorBucket::new(drawing, art, swatch, f64::from(gap), &self.protect).map(VectorLive::Areas),
                    };
                    (live.as_ref().map(|_| Art::Indexed(vector.raster().clone())), live)
                }
                _ => (None, None),
            };
            self.fill_live = copy.map(|art_copy| FillLive {
                layer: layer.id,
                frame: tools.frame,
                kind: art,
                art: art_copy,
                lines,
                swatch,
                ink: if unpaint { [0; 4] } else { self.ink },
                fill: iskra_drawing::Fill { gap, ..Default::default() },
                vector,
            });
        }
        if !response.dragged_by(primary) && !response.drag_started_by(primary) && !response.drag_stopped_by(primary) {
            self.fill_live = None;
        }
        // Only a press that began with the bucket: not one the Eyedropper handed over.
        let held = response.drag_started_by(primary) || (response.dragged_by(primary) && !self.fill_drag.is_empty());
        // A place every few points of the way: what lies between is of the same areas.
        if let Some(pos) = response.interact_pointer_pos().filter(|pos| held && self.fill_drag.last().is_none_or(|(last, _)| last.distance(*pos) >= 3.0)) {
            let [x, y] = self.view.to_scene(response.rect.center(), pos);
            let at = self.drawn(to_pixels.apply(Point::new(x, y)));
            self.fill_drag.push((pos, [at.x, at.y]));
            let protect = self.protect.clone();
            if let Some(live) = self.fill_live.as_mut() {
                live.fill(at, &protect);
            }
        }
        // Let go, it fills: a pen's tap that slides a little is a drag over one area.
        if !response.drag_stopped_by(primary) {
            return None;
        }
        // What was filled on the copy stays in sight until the engine's fill is in the project.
        self.landed = self.fill_live.take().map(|live| (live.layer, live.frame, live.kind, live.art));
        let places: Vec<[f64; 2]> = std::mem::take(&mut self.fill_drag).into_iter().map(|(_, at)| at).collect();
        if places.is_empty() {
            return None;
        }
        let swatch = if tools.unpaint || self.fill_erasing { 0 } else { tools.swatch.0 };
        let (layer, frame) = (layer.id.0, tools.frame);
        let mut params = json!({ "layer": layer, "frame": frame, "points": places, "swatch": swatch, "gap": gap, "protect": self.protect });
        (params["artLayer"], params["everyDrawing"]) = (json!(art), json!(tools.fill_on == FillOn::Every));
        if let Some(frames) = tools.picked_frames.filter(|_| tools.fill_on == FillOn::Picked) {
            params["frames"] = json!(frames);
        }
        // On free colour, the colour in hand that is no swatch's is what an area is filled with.
        params["color"] = json!(tools.free.filter(|_| swatch != 0));
        Some(("drawing.fill", params))
    }

    /// The part of the camera's frame of `width` by `height` that is in from each side by
    /// `inset` of it, in the pixels of `layer`'s drawing on `frame`, as the viewport sees it:
    /// its corner at the top left and the one at the bottom right, across the box of the four.
    /// `None` where the drawing can't be drawn on.
    pub(crate) fn frame_in_drawing(&self, shot: &Shot, layer: &iskra_model::Layer, frame: u32, [width, height]: [f64; 2], inset: f64) -> Option<[[f64; 2]; 2]> {
        let to_pixels = Placement::of(&self.posed(shot), layer, frame, self.flat).to_pixels?;
        let (x, y) = (width * (0.5 - inset), height * (0.5 - inset));
        let corners = [(-x, -y), (x, -y), (x, y), (-x, y)].map(|(x, y)| to_pixels.apply(Point::new(x, y)));
        let along = |of: fn(&Point) -> f64| corners.iter().map(of).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let ((x0, x1), (y0, y1)) = (along(|p| p.x), along(|p| p.y));
        Some([[x0, y0], [x1, y1]])
    }

    /// The way from a drawing placed as `place` to the screen and back, if it can be drawn on.
    fn drawing_space(&self, place: &Placement, center: Pos2) -> Option<DrawingSpace> {
        Some(DrawingSpace { bend: self.bend.clone(), to_scene: place.to_scene, to_pixels: place.to_pixels?, view: self.view, center })
    }

    /// Start, extend or finish the stroke under the pen.
    fn draw_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        current: Option<(&Layer, Placement)>,
        tools: &Tools,
        held: &iskra_drawing::InHand,
        pen: Option<PenSample>,
        panning: bool,
    ) -> Option<(&'static str, Value)> {
        // The right button, which a pen's side button is too, held with the brush erases with
        // the brush as it is.
        let (primary, secondary) = ui.input(|i| (i.pointer.primary_down(), i.pointer.secondary_down()));
        let rubbing = secondary && !primary && tools.tool == Tool::Brush;
        let down = !panning && response.is_pointer_button_down_on() && (primary || rubbing || self.stroke.as_ref().is_some_and(|s| s.rubbing && secondary));
        // A point of the drawing as it is seen, bent, is taken back to where it was drawn.
        let bend = self.bend.clone();
        let drawn = |at: Point| bend.as_ref().map_or(at, |warp| warp.undo(at));
        let Some((layer, place, to_pixels)) = current.and_then(|(layer, place)| Some((layer, place, place.to_pixels?))) else {
            self.stroke = None;
            return None;
        };
        if down && self.stroke.is_none() {
            // The eraser end of the pen erases whatever tool is chosen.
            let erase = tools.tool == Tool::Eraser || pen.is_some_and(|p| p.eraser) || rubbing;
            // The brush is as wide on screen as its outline, whatever the pegs do to the drawing.
            // Its size is in scene units (shot pixels); the drawing's pixels can be finer.
            let finer = (1.0 / place.scale()) as f32;
            let in_hand = &held.raster.brush;
            let mut brush = BrushSettings { size: in_hand.size * finer, erase, ..in_hand.clone() };
            brush.dual_brush.size *= finer;
            // What chance does to a brush differs from stroke to stroke, and is the same when
            // the engine paints the stroke again.
            brush.seed = ui.input(|i| i.time).to_bits();
            let content = layer.drawing_at(tools.frame).and_then(|d| d.drawing.layer(tools.art)).map(|l| &l.content);
            // Vector art is drawn with vector strokes and erased by cutting them (plan §4.16);
            // the shapes don't work on it yet.
            let on_vector = draws_vector(layer, tools);
            if on_vector && !erase && tools.tool == Tool::Shape {
                return None;
            }
            // The vector brush's size is in scene units, as the raster brush's is, and how far
            // its line may stray is in pixels of the screen.
            let vector = on_vector.then(|| {
                let in_hand = &held.vector.brush;
                let mut brush =
                    iskra_drawing::VectorBrush { size: in_hand.size * finer, simplify: in_hand.simplify * finer / self.view.zoom, ..in_hand.clone() };
                // The second tip's size is in scene units too, and its chance its own a stroke.
                (brush.dual_brush.size, brush.seed) = (in_hand.dual_brush.size * finer, ui.input(|i| i.time).to_bits());
                brush
            });
            let art = match content {
                Some(ArtContent::Indexed(surface)) => Art::Indexed(surface.clone()),
                Some(ArtContent::Vector(art)) => Art::Indexed(art.raster().clone()),
                None if on_vector => Art::Indexed(IndexedSurface::new()),
                // A picture that was placed is not painted on: it is read again from its file.
                Some(ArtContent::Rgba(_)) if layer.is_picture(tools.art) => return None,
                Some(ArtContent::Rgba(surface)) => Art::Rgba(surface.clone()),
                // A drawing the stroke will make is as the layer's drawings are.
                None if layer.free_colour.contains(&tools.art) => Art::Rgba(iskra_drawing::free_colour()),
                None => Art::Indexed(IndexedSurface::new()),
            };
            let ink = self.ink;
            // The mixer brush, on free colour: cleaned and loaded as its settings say. Cleaning
            // empties the brush, so it comes first.
            let mixer = match &art {
                Art::Rgba(_) if tools.mixing.on && tools.tool == Tool::Brush && !erase => {
                    let mut carried = self.carries.clone();
                    if tools.mixing.clean {
                        carried.clean();
                    }
                    if tools.mixing.reload {
                        carried.load(ink.map(|channel| f32::from(channel) / 255.0));
                    }
                    Some((tools.mixing.settings(), carried))
                }
                _ => None,
            };
            crate::timings::stroke_began();
            // The Brush draws behind or repaints, as its bar says; the shapes and erasing don't.
            let mode = if tools.tool == Tool::Brush && !erase { tools.paint_mode } else { iskra_drawing::PaintMode::Over };
            let live = match (&art, &mixer, &vector) {
                // The vector eraser is seen as pixels erased, until the engine cuts the shapes.
                (Art::Indexed(surface), _, Some(vector)) if erase => {
                    let eraser =
                        BrushSettings { size: vector.size, hardness: 1.0, pressure_size: vector.pressure_size, erase: true, ..BrushSettings::default() };
                    Live::start(&Art::Indexed(surface.clone()), &eraser, tools.swatch, ink, &self.protect)
                }
                (Art::Indexed(surface), _, Some(vector)) => {
                    let live = iskra_drawing::LiveVector::new(surface, vector, tools.swatch).protecting(self.protect.clone()).in_mode(mode);
                    Live::Vector(live.leaving_surface(tools.surface && tools.tool == Tool::Brush))
                }
                (Art::Rgba(surface), Some((settings, carried)), _) => Live::mixing(surface, &brush, settings.clone(), carried.clone()),
                _ => Live::start_as(&art, &brush, tools.swatch, ink, &self.protect, mode),
            };
            // A shape starts where the pen came down.
            let shape = response.interact_pointer_pos().filter(|_| tools.tool == Tool::Shape && !erase).map(|pos| {
                let [x, y] = self.view.to_scene(response.rect.center(), pos);
                let at = self.drawn(to_pixels.apply(Point::new(x, y)));
                // The corners are as round on screen as the bar says, as the brush is as wide.
                (tools.shape, [at, at], iskra_model::Form { round: tools.form.round * f64::from(finer), ..tools.form }, art)
            });
            // With a guide in hand, a stroke by hand goes along a line of it from where it began;
            // its heading is taken once the pen has gone a few points on the screen.
            let began = response.interact_pointer_pos().map(|pos| self.view.to_scene(response.rect.center(), pos));
            let snap = self
                .held_guide
                .clone()
                .zip(began)
                // Of the shapes, a line is held to it: it is a stroke along one line too.
                .filter(|_| shape.as_ref().is_none_or(|(shape, ..)| *shape == Drawn::Line))
                .map(|(guide, from)| iskra_model::Snap::new(guide, from, 4.0 / f64::from(self.view.zoom)));
            let (layer, frame) = (layer.id, tools.frame);
            // A colour of its own is for free colour alone.
            let free = tools.free.filter(|_| matches!(live, Live::Rgba(_)));
            let art = tools.art;
            self.stroke = Some(ActiveStroke {
                layer,
                frame,
                art,
                swatch: tools.swatch,
                brush,
                points: Vec::new(),
                live,
                ink,
                free,
                mixer,
                vector,
                snap,
                shape,
                rubbing,
                mode,
                flatten: tools.auto_flatten && tools.tool == Tool::Brush,
                surface: tools.surface && tools.tool == Tool::Brush,
            });
        }
        let stroke = self.stroke.as_mut()?;
        if let (true, Some((shape, [from, to], form, before))) = (down, &mut stroke.shape) {
            // The shape so far, drawn anew from where it began to where the pen is.
            if let Some(pos) = response.interact_pointer_pos() {
                let seen = self.view.to_scene(response.rect.center(), pos);
                // Held to a guide, a line ends on the guide's line through its start, once
                // the pen has gone far enough to say which.
                let held = match &mut stroke.snap {
                    Some(snap) => snap.at(seen),
                    None => Some(seen),
                };
                if let Some([x, y]) = held {
                    *to = drawn(to_pixels.apply(Point::new(x, y)));
                }
                form.even = ui.input(|i| i.modifiers.shift) && stroke.snap.is_none();
            }
            let drawn = shape.stroke(&stroke.brush, *from, *to, *form);
            // A filled shape's inside is painted first, and its line over that.
            let under = shape.inside(*from, *to, *form).map(|(rect, coverage)| before.painted(rect, &coverage, stroke.swatch, stroke.ink, &self.protect));
            stroke.live = Live::start(under.as_ref().unwrap_or(before), &drawn.brush, stroke.swatch, stroke.ink, &self.protect);
            stroke.live.push(&drawn.points);
            return None;
        }
        if down {
            // Every pointer position of this frame, not only the last, so fast strokes stay smooth.
            let center = response.rect.center();
            let mut positions: Vec<Pos2> =
                ui.input(|i| i.events.iter().filter_map(|e| if let egui::Event::PointerMoved(pos) = e { Some(*pos) } else { None }).collect());
            if stroke.points.is_empty() && positions.is_empty() {
                positions.extend(response.interact_pointer_pos());
            }
            let time = ui.input(|i| i.time) * 1000.0;
            let view = self.view;
            let snap = &mut stroke.snap;
            let new: Vec<StrokePoint> = positions
                .into_iter()
                .filter_map(|pos| {
                    let seen = view.to_scene(center, pos);
                    // Held to a guide, the point is on the stroke's line, once it has one.
                    let [x, y] = match snap {
                        Some(snap) => snap.at(seen)?,
                        None => seen,
                    };
                    let at = drawn(to_pixels.apply(Point::new(x, y)));
                    let point = StrokePoint { x: at.x, y: at.y, time, ..Default::default() };
                    Some(match pen {
                        Some(p) => StrokePoint { pressure: p.pressure, tilt_x: p.tilt_x, tilt_y: p.tilt_y, rotation: p.rotation, ..point },
                        None => point,
                    })
                })
                .collect();
            if !new.is_empty() {
                stroke.live.push(&new);
                stroke.points.extend(new);
            }
            return None;
        }
        // The pen lifted: hand the stroke to the engine.
        let stroke = self.stroke.take()?;
        self.landed = Some((stroke.layer, stroke.frame, stroke.art, stroke.live.art()));
        let (layer, frame, swatch) = (stroke.layer.0, stroke.frame, stroke.swatch.0);
        if let Some((shape, [from, to], form, _)) = stroke.shape {
            // A press that went nowhere is no shape.
            if (to.x - from.x).hypot(to.y - from.y) < 1.0 {
                return None;
            }
            let (from, to, brush) = ([from.x, from.y], [to.x, to.y], serde_json::to_value(&stroke.brush).ok()?);
            let mut params = json!({ "layer": layer, "frame": frame, "artLayer": stroke.art, "swatch": swatch, "shape": shape, "from": from, "to": to });
            params["color"] = json!(stroke.free);
            if let (Some(params), Ok(Value::Object(form))) = (params.as_object_mut(), serde_json::to_value(form)) {
                params.extend(form);
            }
            (params["brush"], params["protect"]) = (brush, json!(self.protect));
            return Some(("drawing.shape", params));
        }
        if stroke.points.is_empty() {
            return None;
        }
        if let Some(brush) = stroke.vector {
            if stroke.brush.erase {
                let params =
                    json!({ "layer": layer, "frame": frame, "artLayer": stroke.art, "brush": brush, "points": stroke.points, "protect": self.protect });
                return Some(("drawing.vectorErase", params));
            }
            let params = json!({
                "layer": layer, "frame": frame, "artLayer": stroke.art, "swatch": swatch, "brush": brush, "points": stroke.points, "protect": self.protect,
                "mode": stroke.mode, "flatten": stroke.flatten, "surface": stroke.surface,
            });
            return Some(("drawing.vectorStroke", params));
        }
        let data = serde_json::to_value(Stroke { brush: stroke.brush, points: stroke.points }).ok()?;
        if let Some((settings, carried)) = stroke.mixer {
            let params = json!({ "layer": layer, "frame": frame, "artLayer": stroke.art, "stroke": data, "mixer": settings, "state": carried });
            return Some(("drawing.mix", params));
        }
        self.painted = stroke.live.finish(stroke.layer, frame, stroke.art, &self.protect, stroke.mode, stroke.swatch);
        // Shown as it is finished, its end where the pen lifted too.
        if let (Some(painted), Some(landed)) = (&self.painted, &mut self.landed) {
            match &painted.after {
                ArtContent::Indexed(surface) => landed.3 = Art::Indexed(surface.clone()),
                ArtContent::Rgba(surface) => landed.3 = Art::Rgba(surface.clone()),
                ArtContent::Vector(_) => {}
            }
        }
        let mut params = json!({ "layer": layer, "frame": frame, "artLayer": stroke.art, "swatch": swatch, "stroke": data, "protect": self.protect });
        (params["color"], params["mode"]) = (json!(stroke.free), json!(stroke.mode));
        Some(("drawing.stroke", params))
    }

    /// Where a selection's corners and rotate handle are on screen. `matrix` is the transform of
    /// a drag in progress.
    fn handles(&self, center: Pos2, to_scene: &Affine, rect: PixelRect, matrix: &Affine) -> ([Pos2; 4], Pos2) {
        // On a layer that bones bend, the handles are where the bones carry the corners.
        let on_screen = |p: Point| {
            let p = matrix.apply(p);
            let p = to_scene.apply(self.bend.as_ref().map_or(p, |warp| warp.apply(p)));
            self.view.to_screen(center, [p.x, p.y])
        };
        let corners = select::corners(rect).map(on_screen);
        let top = corners[0] + (corners[1] - corners[0]) / 2.0;
        let outward = (top - on_screen(select::center(rect))).normalized();
        (corners, top + outward * ROTATE_HANDLE_OFFSET)
    }

    /// What the selection in hand takes of the drawings of the other layers on `frame`: each
    /// layer that is seen and can be drawn on, with a drawing painted where the box or the
    /// outline is, with the same box or outline in its own pixels.
    fn others_of(&self, shot: &Shot, frame: u32) -> Vec<(Selection, Affine)> {
        let Some(main) = self.selection.as_ref() else { return Vec::new() };
        let parts = main.outlines();
        self.others_across(shot, main.layer, frame)
            .into_iter()
            .filter_map(|(id, across, to_scene)| {
                let layer = shot.layer(id)?;
                let parts: Vec<_> = parts.iter().filter_map(|part| part.transformed(&across)).collect();
                let rect = iskra_model::LassoPart::around(&parts);
                let painted = layer.drawing_at(frame)?.drawing.bounds();
                (!rect.is_empty() && !rect.intersect(&painted).is_empty())
                    .then_some((Selection { layer: id, frame, rect, parts, swatch: main.swatch }, to_scene))
            })
            .collect()
    }

    /// The layers seen beside `main` that can be drawn on, each with what takes the pixels of
    /// `main`'s drawing to its own, and its own to the scene.
    fn others_across(&self, shot: &Shot, main: LayerId, frame: u32) -> Vec<(LayerId, Affine, Affine)> {
        let posed = self.posed(shot);
        let Some(of) = posed.layer(main) else { return Vec::new() };
        let from = Placement::of(&posed, of, frame, self.flat).to_scene;
        let work = self.work;
        let shown = |id: LayerId| match work {
            Work::Boards => shot.is_board(id),
            Work::Animation => !shot.is_board(id),
            Work::Whole => true,
        };
        let seen =
            shot.stack().into_iter().filter(|id| *id != main && !shot.hides(*id) && !self.hidden.contains(id) && !self.fenced.contains(id) && shown(*id));
        seen.filter_map(|id| {
            let layer = posed.layer(id)?;
            let place = Placement::of(&posed, layer, frame, self.flat);
            // From the pixels of the drawing in hand to this one's.
            let across = place.to_pixels?.mul(&from);
            Some((id, across, place.to_scene))
        })
        .collect()
    }

    /// Select the piece `drawing.splitPiece` split off, of `layer` on `frame`, by its middle
    /// line as `result` gives it: a thin outline along it, short of its ends, so that it takes
    /// the piece and not the lines crossing it there.
    pub fn select_piece(&mut self, layer: LayerId, frame: u32, result: &Value) {
        let line: Vec<Point> =
            result["line"].as_array().into_iter().flatten().filter_map(|at| Some(Point::new(at.get(0)?.as_f64()?, at.get(1)?.as_f64()?))).collect();
        // A point a pixel or so apart along it, as a straight line is flattened to its ends.
        let mut line: Vec<Point> = line
            .windows(2)
            .flat_map(|pair| {
                let steps = (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y).ceil().max(1.0) as usize;
                (0..steps).map(move |step| {
                    let t = step as f64 / steps as f64;
                    Point::new(pair[0].x + (pair[1].x - pair[0].x) * t, pair[0].y + (pair[1].y - pair[0].y) * t)
                })
            })
            .chain(line.last().copied())
            .collect::<Vec<_>>();
        line.dedup_by(|a, b| a.x == b.x && a.y == b.y);
        // How far along each point is, and what is kept of it: past the trim at either end.
        let mut along = vec![0.0];
        for pair in line.windows(2) {
            along.push(along.last().copied().unwrap_or(0.0) + (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y));
        }
        let length = along.last().copied().unwrap_or(0.0);
        let trim = PIECE_TRIM.min(length / 3.0);
        let kept: Vec<Point> = line.iter().zip(&along).filter(|(_, at)| **at >= trim && **at <= length - trim).map(|(at, _)| *at).collect();
        let middle = line.get(line.len() / 2).copied();
        let kept = if kept.len() >= 2 { kept } else { middle.into_iter().chain(middle).collect() };
        if kept.is_empty() {
            return;
        }
        // Each side, out from the line square to it; across a dot, as if it went to the right.
        let side = |at: usize, out: f64| {
            let (before, after) = (kept[at.saturating_sub(1)], kept[(at + 1).min(kept.len() - 1)]);
            let (dx, dy) = (after.x - before.x, after.y - before.y);
            let length = dx.hypot(dy);
            let (dx, dy) = if length > 1e-6 { (dx / length, dy / length) } else { (1.0, 0.0) };
            Point::new(kept[at].x - dy * out, kept[at].y + dx * out)
        };
        let mut outline: Vec<Point> = (0..kept.len()).map(|at| side(at, PIECE_WIDTH)).collect();
        outline.extend((0..kept.len()).rev().map(|at| side(at, -PIECE_WIDTH)));
        if let Some(lasso) = iskra_model::Lasso::new(&outline) {
            self.selection = Some(Selection::of_lasso(layer, frame, lasso));
        }
    }

    /// A transform of the selection in hand, `params`, as one step with the same done to what
    /// it takes of the other layers' drawings: the matrix taken to each one's pixels. Their
    /// selections go where it takes them.
    fn with_others(&mut self, shot: &Shot, params: Value, frame: u32) -> (&'static str, Value) {
        let posed = self.posed(shot);
        let main = posed.layer(LayerId(params["layer"].as_u64().unwrap_or_default())).map(|layer| Placement::of(&posed, layer, frame, self.flat).to_scene);
        let matrix = params["matrix"].as_array().map(|m| m.iter().filter_map(Value::as_f64).collect::<Vec<_>>());
        let (Some(main), Some(Ok(m))) = (main, matrix.map(<[f64; 6]>::try_from)) else { return ("drawing.transform", params) };
        // In the scene, what the matrix does in the drawing in hand.
        let Some(back) = main.inverse() else { return ("drawing.transform", params) };
        let moved = main.mul(&Affine { m }).mul(&back);
        let mut commands = vec![json!({ "id": "drawing.transform", "params": params.clone() })];
        // The selection in hand lands as its box or outline is moved, round what all of them
        // took, and not round what it moved of its own drawing alone.
        if let Some(main) = &self.selection {
            self.carried = Some((main.outlines().iter().filter_map(|part| part.transformed(&Affine { m })).collect(), main.swatch));
        }
        for (other, to_scene) in &mut self.also {
            let Some(to_pixels) = to_scene.inverse() else { continue };
            let mut one = other.params();
            let across = to_pixels.mul(&moved).mul(to_scene);
            (one["matrix"], one["artLayer"], one["protect"]) = (json!(across.m), params["artLayer"].clone(), params["protect"].clone());
            if let Some(every) = params.get("everyDrawing") {
                one["everyDrawing"] = every.clone();
            }
            commands.push(json!({ "id": "drawing.transform", "params": one }));
            other.parts = other.parts.iter().filter_map(|part| part.transformed(&across)).collect();
            other.rect = iskra_model::LassoPart::around(&other.parts);
        }
        self.also.retain(|(other, _)| !other.rect.is_empty());
        ("edit.batch", json!({ "commands": commands, "title": "Transform Selection" }))
    }

    /// Where the selection in hand lands after a transform of it with the other layers': round
    /// its box or outline, moved.
    pub fn landed_with_others(&self) -> Value {
        let rect = self.carried.as_ref().map_or(PixelRect::EMPTY, |(parts, _)| iskra_model::LassoPart::around(parts));
        json!([rect.x0, rect.y0, rect.x1, rect.y1])
    }

    /// The Cutter tool: an outline drawn by hand, or a box dragged out, across lines. Let go,
    /// the lines are cut along its edge and what is inside goes; with Alt it is kept apart,
    /// and selected for the Select tool to move.
    fn cutter_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        shot: Option<&Shot>,
        current: Option<(&Layer, Placement)>,
        tools: &Tools,
        panning: bool,
    ) -> Option<(&'static str, Value)> {
        let (layer, place) = current?;
        let pixels = place.to_pixels?;
        let (center, view, bend) = (response.rect.center(), self.view, self.bend.clone());
        let to_pixels = |pos: Pos2| {
            let [x, y] = view.to_scene(center, pos);
            let at = pixels.apply(Point::new(x, y));
            bend.as_ref().map_or(at, |warp| warp.undo(at))
        };
        if !panning && response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()) {
            let now = to_pixels(response.interact_pointer_pos()?);
            match (&mut self.marquee, &mut self.lassoing) {
                (Some(marquee), _) => marquee.1 = now,
                (_, Some(outline)) => {
                    // A point of the outline for every point or so the pen goes on the screen.
                    let apart = 1.5 / (place.scale() * f64::from(view.zoom)).max(1e-9);
                    if outline.last().is_none_or(|last| (last.x - now.x).hypot(last.y - now.y) >= apart) {
                        outline.push(now);
                    }
                }
                _ if tools.cut_box => self.marquee = Some((now, now)),
                _ => self.lassoing = Some(vec![now]),
            }
            return None;
        }
        // What is inside is kept, selected, to be changed or deleted; with Alt it is taken away.
        let keep = !ui.input(|i| i.modifiers.alt);
        // On vector art, an outline that encloses next to nothing is a flick across lines.
        if let Some(outline) = self.lassoing.as_ref().filter(|outline| draws_vector(layer, tools) && is_flick(outline)) {
            let (outline, frame) = (outline.clone(), tools.frame);
            self.lassoing = None;
            let flick = |layer: LayerId, across: &Affine| {
                let points: Vec<[f64; 2]> = outline.iter().map(|at| across.apply(*at)).map(|at| [at.x, at.y]).collect();
                json!({
                    "layer": layer.0, "frame": frame, "points": points, "artLayer": tools.selected_art(), "protect": self.protect, "cap": tools.cut_cap,
                    "everyDrawing": tools.all_drawings,
                })
            };
            let mut flicks = vec![flick(layer.id, &Affine::IDENTITY)];
            if let Some(shot) = shot.filter(|_| tools.all_layers) {
                let others = self.others_across(shot, layer.id, frame).into_iter();
                let drawn = others.filter(|(id, ..)| shot.layer(*id).is_some_and(|other| other.drawing_at(frame).is_some()));
                flicks.extend(drawn.map(|(id, across, _)| flick(id, &across)));
            }
            return (outline.len() > 1).then(|| cut_all("drawing.cutFlick", flicks));
        }
        let cut = match (self.marquee.take(), self.lassoing.take()) {
            (Some((from, to)), _) => select::marquee(from, to).map(|rect| Selection::of(layer.id, tools.frame, rect)),
            (_, Some(outline)) => iskra_model::Lasso::new(&outline).map(|lasso| Selection::of_lasso(layer.id, tools.frame, lasso)),
            _ => None,
        }?;
        let along = |cut: &Selection| {
            let mut params = cut.params();
            (params["artLayer"], params["protect"], params["keep"], params["cap"], params["everyDrawing"]) =
                (json!(tools.selected_art()), json!(self.protect), json!(keep), json!(tools.cut_cap), json!(tools.all_drawings));
            params
        };
        let mut cuts = vec![along(&cut)];
        // With *Select All Layers*, the same edge in each other layer's drawing that it crosses.
        if let Some(shot) = shot.filter(|_| tools.all_layers) {
            let outlines = cut.outlines();
            for (id, across, _) in self.others_across(shot, layer.id, tools.frame) {
                let Some(painted) = shot.layer(id).and_then(|other| other.drawing_at(tools.frame)).map(|entry| entry.drawing.bounds()) else { continue };
                let parts: Vec<_> = outlines.iter().filter_map(|part| part.transformed(&across)).collect();
                let rect = iskra_model::LassoPart::around(&parts);
                if !rect.is_empty() && !rect.intersect(&painted).is_empty() {
                    cuts.push(along(&Selection { layer: id, frame: tools.frame, rect, parts, swatch: None }));
                }
            }
        }
        // Kept apart, it is selected, to be moved: what was kept of the drawing in hand.
        if keep {
            (self.selection, self.cut_kept) = (Some(cut), true);
        }
        Some(cut_all("drawing.cutAlong", cuts))
    }

    /// Which handle of a selection round `rect` a press at `pos` on screen takes: a corner, the
    /// middle of a side, the handle above, then just outside a corner, which turns as the
    /// handle above does, and a side between its handles, which skews. `None` elsewhere.
    fn selection_hit(&self, center: Pos2, to_scene: &Affine, rect: PixelRect, pos: Pos2) -> Option<Handle> {
        let (corners, rotate) = self.handles(center, to_scene, rect, &Affine::IDENTITY);
        let near = |handle: Pos2| (handle - pos).length() <= HANDLE_REACH;
        let corner = corners.iter().position(|&c| near(c)).map(Handle::Corner);
        let side = || sides(&corners).iter().position(|&c| near(c)).map(Handle::Side);
        corner.or_else(side).or(near(rotate).then_some(Handle::Rotate)).or_else(|| turn_or_skew(&corners, pos, true))
    }

    /// The Select tool: drag out a rectangle, or drag the selection or one of its handles.
    fn select_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        current: Option<(&Layer, Placement)>,
        tools: &Tools,
        panning: bool,
    ) -> Option<(&'static str, Value)> {
        let (layer, place) = current?;
        let pixels = place.to_pixels?;
        let (center, view) = (response.rect.center(), self.view);
        // A flip or a turn from the buttons: one step, as a drag let go is.
        if let Some(matrix) = self.operation.take().filter(|_| self.drag.is_none()) {
            let selection = self.selection.clone().filter(|selection| selection.layer == layer.id)?;
            return Some(self.transformed(&selection, matrix, tools));
        }
        // On a layer that bones bend, the pointer is taken back through the bend: what is
        // selected is the part of the drawing that is seen under it.
        let bend = self.bend.clone();
        let to_pixels = |pos: Pos2| {
            let [x, y] = view.to_scene(center, pos);
            let at = pixels.apply(Point::new(x, y));
            bend.as_ref().map_or(at, |warp| warp.undo(at))
        };
        // The pointer says what a press would take: a turn, or a skew along a side.
        let pointed = response.hover_pos().filter(|_| !panning && self.combining.is_none());
        let rect = self.selection.as_ref().map(|selection| selection.rect);
        let hovered = pointed.zip(rect).and_then(|(pos, rect)| self.selection_hit(center, &place.to_scene, rect, pos));
        if let (None, Some(Handle::Rotate), Some(pos), Some(rect)) = (&self.drag, hovered, pointed, rect) {
            let (corners, rotate) = self.handles(center, &place.to_scene, rect, &Affine::IDENTITY);
            self.turn_facing = facing(&corners, pos, Some(rotate));
        }
        // Inside the selection, the pointer moves it.
        let inside = pointed.zip(rect).is_some_and(|(pos, rect)| {
            let at = to_pixels(pos);
            rect.contains(at.x.floor() as i32, at.y.floor() as i32)
        });
        self.hover_handle = self.drag.as_ref().map(|drag| drag.handle).or(hovered).or(inside.then_some(Handle::Move));
        set_handle_cursor(ui.ctx(), self.hover_handle);
        if !panning && response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()) {
            let pos = response.interact_pointer_pos()?;
            let now = to_pixels(pos);
            if self.drag.is_none() && self.marquee.is_none() && self.lassoing.is_none() {
                // With Shift what is selected next is added to the selection, with Alt it is
                // taken from it: the press then starts that, wherever it is.
                self.combining =
                    ui.input(|i| if i.modifiers.alt { Some(true) } else { i.modifiers.shift.then_some(false) }).filter(|_| self.selection.is_some());
                // The press: on a handle, inside the selection, or somewhere else.
                let hit = self.selection.as_ref().filter(|_| self.combining.is_none()).and_then(|s| {
                    let handle = self.selection_hit(center, &place.to_scene, s.rect, pos);
                    handle.or(s.rect.contains(now.x.floor() as i32, now.y.floor() as i32).then_some(Handle::Move))
                });
                match (hit, &self.selection, layer.drawing_at(tools.frame)) {
                    (Some(handle), Some(selection), Some(drawing)) => {
                        let placed = layer.media.is_some();
                        self.drag = Some(Drag::new(handle, now, &drawing.drawing, &selection.picked(&self.protect), tools.selected_art(), placed));
                    }
                    // A new selection, or more or less of the one there is: an outline drawn
                    // by hand, or a rectangle dragged out.
                    _ if tools.lasso => self.lassoing = Some(vec![now]),
                    _ => self.marquee = Some((now, now)),
                }
            }
            if let (Some(drag), Some(selection)) = (&mut self.drag, &self.selection) {
                drag.update(selection.rect, now);
            }
            if let Some(marquee) = &mut self.marquee {
                marquee.1 = now;
            }
            // A point of the outline for every point or so the pen goes on the screen.
            let apart = 1.5 / (place.scale() * f64::from(view.zoom)).max(1e-9);
            if let Some(outline) = self.lassoing.as_mut().filter(|outline| outline.last().is_none_or(|last| (last.x - now.x).hypot(last.y - now.y) >= apart)) {
                outline.push(now);
            }
            return None;
        }
        // The button is up: a dragged-out rectangle becomes the selection, a dragged handle a command.
        // What was there is added to or taken from, if Shift or Alt was held, or else replaced.
        let combining = self.combining.take().zip(self.selection.clone());
        let made = self.marquee.is_some();
        if let Some((from, to)) = self.marquee.take() {
            // A press that barely moved on screen is a click: it only drops the selection.
            let on_screen = ((to.x - from.x).hypot(to.y - from.y) * place.scale()) as f32 * view.zoom;
            let rect = select::marquee(from, to).filter(|_| on_screen >= CLICK_SLOP);
            self.selection = match (rect, &combining) {
                (Some(rect), Some((subtract, had))) => iskra_model::Lasso::of_rect(rect).and_then(|more| had.with(more, *subtract)),
                // With Shift or Alt, a click leaves the selection as it is.
                (None, Some((_, had))) => Some(had.clone()),
                (rect, None) => rect.map(|rect| Selection::of(layer.id, tools.frame, rect)),
            };
        }
        // And so does the inside of an outline, closed from where it ends to where it began.
        let outlined = self.lassoing.take();
        if let Some(outline) = &outlined {
            self.close_outline(outline, combining.clone(), layer.id, tools.frame);
        }
        if made || outlined.is_some() {
            self.fit_to_shapes(layer, tools.frame, tools.selected_art());
        }
        let (drag, selection) = (self.drag.take()?, self.selection.clone()?);
        if drag.matrix == Affine::IDENTITY {
            return None;
        }
        let command = self.transformed(&selection, drag.matrix, tools);
        self.dropped = Some(drag);
        Some(command)
    }

    /// The command that moves, scales or turns what `selection` takes by `matrix`: of every
    /// drawing of the layer with *Select All Drawings*.
    fn transformed(&mut self, selection: &Selection, matrix: Affine, tools: &Tools) -> (&'static str, Value) {
        let mut params = selection.params();
        (params["matrix"], params["artLayer"], params["protect"]) = (json!(matrix.m), json!(tools.selected_art()), json!(self.protect));
        if tools.all_drawings {
            params["everyDrawing"] = json!(true);
        }
        // Where it lands, the selection is of the same swatch, inside the same outlines, moved.
        let moved = selection.parts.iter().filter_map(|part| part.transformed(&matrix)).collect();
        self.carried = Some((moved, selection.swatch));
        ("drawing.transform", params)
    }

    /// On vector art, a selection's handles round the whole shapes it takes, not round the box
    /// or the outline that was drawn, which stays what decides which shapes those are: it is
    /// kept as the selection's outline. On pixels, or where it takes nothing, it is as it was.
    /// Select the surface `id` of the art layer `kind` of `layer`'s drawing on `frame`, whole, as
    /// *Shapes* takes a shape: by a pixel only it covers of the art layers `reach` takes, the
    /// handles fitted round it. The Select tool's *Pieces* on a surface.
    pub fn select_surface(&mut self, layer: &Layer, frame: u32, kind: ArtLayerKind, id: iskra_model::ShapeId, reach: iskra_model::ArtReach) {
        let Some(entry) = layer.drawing_at(frame) else { return };
        let vector = |of: ArtLayerKind| match entry.drawing.layer(of).map(|art| &art.content) {
            Some(ArtContent::Vector(vector)) => Some(vector),
            _ => None,
        };
        let Some(art) = vector(kind) else { return };
        let Some(index) = art.shapes().iter().position(|shape| shape.id == id) else { return };
        let others: Vec<&iskra_model::VectorArt> =
            ArtLayerKind::ALL.iter().filter(|other| **other != kind && reach.takes(**other)).filter_map(|other| vector(*other)).collect();
        let Some((x, y)) = art.own_pixel(index, &others) else { return };
        let Some(lasso) = iskra_model::Lasso::of_rect(PixelRect::new(x, y, x + 1, y + 1)) else { return };
        self.selection = Some(Selection::of_lasso(layer.id, frame, lasso));
        self.fit_to_shapes(layer, frame, reach);
    }

    fn fit_to_shapes(&mut self, layer: &Layer, frame: u32, art: iskra_model::ArtReach) {
        let Some(selection) = self.selection.clone().filter(|selection| selection.layer == layer.id && selection.frame == frame) else { return };
        let Some(entry) = layer.drawing_at(frame) else { return };
        let arts: Vec<&ArtContent> = entry.drawing.art_layers.iter().filter(|of| art.takes(of.kind)).map(|of| &of.content).collect();
        if arts.is_empty() || !arts.iter().all(|content| content.is_vector()) {
            return;
        }
        let picked = selection.picked(&self.protect);
        let taken = arts.iter().fold(PixelRect::EMPTY, |around, content| match content {
            ArtContent::Vector(vector) => around.union(&vector.extract_picked(&picked).raster().bounds()),
            _ => around,
        });
        if taken.is_empty() || taken == selection.rect {
            return;
        }
        let mut parts = selection.parts.clone();
        if parts.is_empty() {
            let Some(drawn) = iskra_model::Lasso::of_rect(selection.rect) else { return };
            parts.push(iskra_model::LassoPart { lasso: Arc::new(drawn), subtract: false });
        }
        self.selection = Some(Selection { rect: taken, parts, ..selection });
    }

    /// The selection's drag that shows: the one under way, or the one let go on this frame.
    fn dragging(&self) -> Option<&Drag> {
        self.drag.as_ref().or(self.dropped.as_ref())
    }

    /// The inside of an outline, closed from where it ends to where it began, is the
    /// selection: or with `combining` is added to the one there was, or taken from it.
    fn close_outline(&mut self, outline: &[Point], combining: Option<(bool, Selection)>, layer: LayerId, frame: u32) {
        let lasso = iskra_model::Lasso::new(outline).filter(|lasso| lasso.bounds().width() >= 2 && lasso.bounds().height() >= 2);
        self.selection = match (lasso, &combining) {
            (Some(lasso), Some((subtract, had))) => had.with(lasso, *subtract),
            (None, Some((_, had))) => Some(had.clone()),
            (lasso, None) => lasso.map(|lasso| Selection::of_lasso(layer, frame, lasso)),
        };
    }

    /// Select what `swatch` painted on the drawing `layer` shows on `frame`, on every art
    /// layer or the `only` one: the selection is around all of it, and of that swatch alone.
    /// Says whether anything is painted with it there.
    pub fn select_swatch(&mut self, layer: &Layer, frame: u32, swatch: SwatchId, only: iskra_model::ArtReach) -> bool {
        let Some(entry) = layer.drawing_at(frame) else { return false };
        let of = entry.drawing.art_layers.iter().filter(|art| only.takes(art.kind));
        let around = of.fold(PixelRect::EMPTY, |around, art| match art.content.indexed() {
            Some(surface) => {
                let picked = iskra_model::Picked { swatch: Some(swatch), ..iskra_model::Picked::all(surface.bounds()) };
                around.union(&surface.extract_picked(&picked).bounds())
            }
            None => around,
        });
        self.selection = (!around.is_empty()).then(|| Selection { swatch: Some(swatch), ..Selection::of(layer.id, frame, around) });
        self.selection.is_some()
    }

    /// The selection, with what it takes of the other layers' drawings: to be put back as it
    /// was when an undo or a redo goes back to where it was.
    pub(crate) fn selection_kept(&self) -> (Option<Selection>, Vec<(Selection, Affine)>) {
        (self.selection.clone(), self.also.clone())
    }

    pub(crate) fn put_selection_back(&mut self, (selection, also): (Option<Selection>, Vec<(Selection, Affine)>)) {
        (self.selection, self.also, self.drag, self.dropped) = (selection, also, None, None);
    }

    /// The selection is now `rect` (`[x0, y0, x1, y1]`, as a command returned it): where a
    /// transformed selection landed, or what was pasted.
    pub fn select(&mut self, layer: LayerId, frame: u32, rect: &Value) {
        let side = |i: usize| rect.get(i).and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok());
        let (Some(x0), Some(y0), Some(x1), Some(y1)) = (side(0), side(1), side(2), side(3)) else { return };
        let rect = PixelRect::new(x0, y0, x1, y1);
        let (parts, swatch) = self.carried.take().unwrap_or_default();
        self.selection = (!rect.is_empty()).then_some(Selection { layer, frame, rect, parts, swatch });
    }

    /// Where a peg's pivot and its rotate handle are on screen. The handle is out along the
    /// peg's own x axis, so it shows how the peg is turned.
    /// Where a peg's pivot is on `frame`, in the scene.
    pub(crate) fn pivot_in_scene(&self, shot: &Shot, peg: PegId, frame: u32) -> Option<[f64; 2]> {
        let peg = shot.peg(peg)?;
        let [x, y] = peg.transform.pivot;
        let at = space(shot, self.flat, Some(peg.id), Some(peg.id), frame).apply(Point::new(x, y));
        Some([at.x, at.y])
    }

    /// A peg's transform on `frame` with its pivot where `at` is in the scene, and its position
    /// moved by as much as keeps what it carries where it was.
    pub(crate) fn pivot_put(&self, shot: &Shot, peg: PegId, frame: u32, [x, y]: [f64; 2]) -> Option<Transform> {
        let from = shot.peg(peg)?.transform_at(frame);
        let to = space(shot, self.flat, Some(peg), Some(peg), frame).inverse()?.apply(Point::new(x, y));
        Some(pivot_moved(from, [to.x - from.pivot[0], to.y - from.pivot[1]]))
    }

    fn peg_handles(&self, center: Pos2, shot: &Shot, peg: PegId, frame: u32) -> Option<(Pos2, Pos2)> {
        let peg = shot.peg(peg)?;
        // The tool's temporary pivot, where it has one, in place of the peg's.
        let pivot = self.temp_pivot.filter(|(of, _)| *of == peg.id).map_or(peg.transform.pivot, |(_, at)| at);
        let (world, [px, py]) = (space(shot, self.flat, Some(peg.id), Some(peg.id), frame), pivot);
        let on_screen = |x: f64, y: f64| {
            let p = world.apply(Point::new(x, y));
            self.view.to_screen(center, [p.x, p.y])
        };
        let pivot = on_screen(px, py);
        let along = on_screen(px + 1.0, py) - pivot;
        let along = if along.length() > 1e-6 { along.normalized() } else { egui::Vec2::X };
        Some((pivot, pivot + along * PEG_HANDLE_OFFSET))
    }

    /// Where a click was, as it is let go: a press that did not go far enough to be a drag.
    /// The Zoom tool: a drag right or down zooms in about where it began, left or up out; a
    /// click steps in about where it is, with Alt out; with Shift, a drag draws a box that is
    /// zoomed to fill the viewport once it is let go.
    fn zoom_input(&mut self, ui: &egui::Ui, response: &egui::Response, panning: bool) {
        let (rect, alt) = (response.rect, ui.input(|i| i.modifiers.alt));
        if panning {
            self.zooming = None;
            return;
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(if alt { egui::CursorIcon::ZoomOut } else { egui::CursorIcon::ZoomIn });
        }
        if response.drag_started_by(PointerButton::Primary)
            && let Some(at) = response.interact_pointer_pos()
        {
            let boxing = ui.input(|i| i.modifiers.shift);
            self.zooming = Some(Zooming { from: at, to: at, moved: false, boxing });
        }
        let Some(mut zooming) = self.zooming else { return };
        if let Some(now) = response.interact_pointer_pos() {
            let delta = now - zooming.to;
            zooming.to = now;
            let was = zooming.moved;
            zooming.moved |= (now - zooming.from).length() >= CLICK_SLOP;
            // Once it is a drag, and from what it moves after; not the slop it took to be one.
            if was && !zooming.boxing {
                self.view.zoom_about(rect.center(), zooming.from, drag_zoom(delta));
            }
        }
        if response.is_pointer_button_down_on() {
            self.zooming = Some(zooming);
            return;
        }
        self.zooming = None;
        match zooming {
            Zooming { moved: false, from, .. } => self.view.step_about(rect.center(), from, !alt),
            Zooming { boxing: true, from, to, .. } => self.view.zoom_to(rect, egui::Rect::from_two_pos(from, to)),
            _ => {}
        }
    }

    fn click(&mut self, response: &egui::Response, panning: bool) -> Option<Pos2> {
        if panning {
            self.press = None;
            return None;
        }
        if response.drag_started_by(PointerButton::Primary) {
            self.press = response.interact_pointer_pos().map(|at| (at, false));
        }
        if let (Some((from, moved)), Some(now)) = (self.press.as_mut(), response.interact_pointer_pos()) {
            *moved |= (now - *from).length() >= CLICK_SLOP;
        }
        if response.is_pointer_button_down_on() {
            return None;
        }
        match self.press.take()? {
            (at, false) => Some(at),
            _ => None,
        }
    }

    /// The layer whose drawing is under a point of the screen on `frame`: the first of those
    /// that are seen, the top one first, that has paint there, as its bones bend it. One that
    /// is hidden or switched off is not seen, and is passed over, as is one locked on its row
    /// of the Animation timeline.
    /// The layer of the chain drawn faint with *Always Visible* at `at` on screen, on a joint or
    /// along a bone, the one drawn last where several are; not `current`'s, which is drawn whole,
    /// nor one that is locked.
    fn faint_under(&self, shot: &Shot, current: Option<LayerId>, at: Pos2) -> Option<LayerId> {
        let along = |a: Pos2, b: Pos2| {
            let way = b - a;
            let part = if way.length_sq() > 0.0 { ((at - a).dot(way) / way.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
            (a + way * part - at).length()
        };
        let locked = |id: LayerId| self.work != Work::Boards && shot.locks(id);
        let on = |joints: &[Pos2]| {
            joints.iter().any(|joint| (*joint - at).length() <= HANDLE_REACH) || joints.windows(2).any(|ends| along(ends[0], ends[1]) <= HANDLE_REACH * 0.6)
        };
        self.faint_chains.iter().rev().find(|(layer, joints)| Some(*layer) != current && !locked(*layer) && on(joints)).map(|(layer, _)| *layer)
    }

    fn layer_under(&self, shot: &Shot, center: Pos2, at: Pos2, frame: u32) -> Option<LayerId> {
        let [x, y] = self.view.to_scene(center, at);
        // Only what is seen: not a layer hidden on its row, nor one of the work that is not
        // shown here, the animation on the Storyboard page or the boards where they are hidden.
        let work = self.work;
        let shown = |id: LayerId| match work {
            Work::Boards => shot.is_board(id),
            Work::Animation => !shot.is_board(id),
            Work::Whole => true,
        };
        let locked = |id: LayerId| work != Work::Boards && shot.locks(id);
        let mut seen = shot.stack().into_iter().filter(|id| !shot.hides(*id) && !self.hidden.contains(id) && shown(*id) && !locked(*id));
        seen.find(|id| {
            let painted = || {
                let layer = shot.layer(*id)?;
                let pixel = Placement::of(shot, layer, frame, self.flat).to_pixels?.apply(Point::new(x, y));
                // On a layer that bones bend, the pixel that is seen there is where the bend
                // takes it from.
                let pixel = warp_of(shot, layer, frame, true).map_or(pixel, |warp| warp.undo(pixel));
                let (x, y) = (pixel.x.floor() as i32, pixel.y.floor() as i32);
                let drawing = &layer.drawing_at(frame)?.drawing;
                // Nor an art layer that is switched off.
                let there = drawing.art_layers.iter().filter(|art| !layer.art_off.contains(&art.kind)).any(|art| match &art.content {
                    ArtContent::Rgba(surface) => {
                        let mut found = [[0u8; 4]];
                        surface.read_rgba8_into(iskra_model::Rect::new(x, y, x + 1, y + 1), &mut found);
                        found[0][3] > 0
                    }
                    content => content.indexed().is_some_and(|surface| surface.pixel(x, y).coverage > 0),
                });
                Some(there)
            };
            painted() == Some(true)
        })
    }

    /// The box round what a peg carries on `frame`, in the peg's own space: its left, top,
    /// right and bottom. It is round the drawings as bones bend them, found through a grid
    /// over what is painted, as the bent drawing's own pixels are.
    /// With other pegs in hand beside it, it is round what they carry too, since all of it
    /// is moved, turned and scaled as one thing. `None` when none of it is painted there.
    fn carried_box(&mut self, shot: &Shot, peg: PegId, frame: u32) -> Option<[f64; 4]> {
        let back = shot.world(Some(peg), frame).inverse()?;
        let (mut kept, mut bent, mut bounds) = (Vec::new(), Vec::new(), None::<[f64; 4]>);
        let in_hand: Vec<PegId> = [peg].into_iter().chain(self.with_pegs.iter().copied()).collect();
        for layer in shot.layers.iter().filter(|layer| in_hand.iter().any(|peg| shot.carried_from(layer.parent, peg.0))) {
            let Some(shown) = layer.drawing_at(frame) else { continue };
            // A drawing's bounds are found by looking through its pixels: once for a drawing.
            let known = self.box_bounds.iter().find(|(of, _)| Arc::ptr_eq(of, &shown.drawing)).map(|(_, rect)| *rect);
            let rect = known.unwrap_or_else(|| shown.drawing.bounds());
            kept.push((shown.drawing.clone(), rect));
            if rect.is_empty() {
                continue;
            }
            let to_peg = back.mul(&shot.layer_world(layer, frame)).mul(&Affine::scale(1.0 / layer.density));
            // Bent, the box is round where the corners of the drawing's mesh go: once for a
            // drawing and a pose.
            let [x0, y0, x1, y1] = match warp_of(shot, layer, frame, true) {
                Some(warp) => {
                    let key = (warp.key(), rect);
                    let known = self.bent_bounds.iter().find(|(of, _)| *of == key).map(|(_, bounds)| *bounds);
                    let bounds = known.unwrap_or_else(|| {
                        let corners = iskra_render::Mesh::of(rect, &warp).corners;
                        corners.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |[x0, y0, x1, y1], (_, at)| {
                            [x0.min(at.x), y0.min(at.y), x1.max(at.x), y1.max(at.y)]
                        })
                    });
                    bent.push((key, bounds));
                    bounds
                }
                None => [rect.x0, rect.y0, rect.x1, rect.y1].map(f64::from),
            };
            for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
                let at = to_peg.apply(Point::new(x, y));
                let [x0, y0, x1, y1] = bounds.unwrap_or([at.x, at.y, at.x, at.y]);
                bounds = Some([x0.min(at.x), y0.min(at.y), x1.max(at.x), y1.max(at.y)]);
            }
        }
        (self.box_bounds, self.bent_bounds) = (kept, bent);
        bounds.filter(|[x0, y0, x1, y1]| x1 - x0 > 1e-6 && y1 - y0 > 1e-6)
    }

    /// Where the handles of that box are on screen: its corners, clockwise from the top left,
    /// then the middles of its sides, from the top's.
    /// What a press at `pos` on the Transform tool's box does that is on none of its handles:
    /// a turn just outside a corner, or a skew along a side, with the box in the peg's own space.
    fn box_edge(&self, center: Pos2, shot: &Shot, peg: PegId, frame: u32, pos: Pos2) -> Option<(Handle, [f64; 4])> {
        let bounds = self.scale_box.filter(|(of, _)| *of == peg)?.1;
        let on_screen = self.box_on_screen(center, shot, peg, frame, bounds)?;
        if on_screen.iter().any(|at| (*at - pos).length() <= HANDLE_REACH) {
            return None;
        }
        let corners = [on_screen[0], on_screen[1], on_screen[2], on_screen[3]];
        Some((turn_or_skew(&corners, pos, false)?, bounds))
    }

    fn box_on_screen(&self, center: Pos2, shot: &Shot, peg: PegId, frame: u32, bounds: [f64; 4]) -> Option<[Pos2; 8]> {
        let world = space(shot, self.flat, Some(shot.peg(peg)?.id), Some(peg), frame);
        Some(box_handles(bounds).map(|([x, y], _)| {
            let at = world.apply(Point::new(x, y));
            self.view.to_screen(center, [at.x, at.y])
        }))
    }

    /// The path of an animated peg on screen: where its pivot is on each frame from its first
    /// position keyframe to its last, and whether that frame has one. What is above the peg is
    /// taken as it is on `frame`.
    fn peg_path(&self, center: Pos2, shot: &Shot, peg: &Peg, frame: u32) -> Vec<(u32, Pos2, bool)> {
        let track = &peg.animation.position;
        let (Some(first), Some(last)) = (track.keys().first(), track.keys().last()) else { return Vec::new() };
        let (above, [px, py]) = (peg_space(shot, self.flat, peg, frame), peg.transform.pivot);
        let on_screen = |at: u32| {
            let [x, y] = track.at(at, true)?;
            let p = above.apply(Point::new(x + px, y + py));
            Some((at, self.view.to_screen(center, [p.x, p.y]), track.key(at).is_some()))
        };
        (first.frame..=last.frame.min(first.frame.saturating_add(PATH_FRAMES))).filter_map(on_screen).collect()
    }

    /// Where the position keyframe on `frame` and the two handles of the path there are on
    /// screen: the key, the handle the path arrives by, the one it leaves by.
    fn bend_handles(&self, center: Pos2, shot: &Shot, peg: &Peg, frame: u32) -> Option<(Pos2, Pos2, Pos2)> {
        let (key, bend) = (peg.animation.position.key(frame)?, peg.animation.position.handles(frame)?);
        let (above, [px, py]) = (peg_space(shot, self.flat, peg, frame), peg.transform.pivot);
        let on_screen = |[dx, dy]: [f64; 2]| {
            let p = above.apply(Point::new(key.value[0] + px + dx, key.value[1] + py + dy));
            self.view.to_screen(center, [p.x, p.y])
        };
        Some((on_screen([0.0, 0.0]), on_screen(bend.into), on_screen(bend.out)))
    }

    /// Where a pivot dragged from `from`, in the peg's own space, by `by` on screen, is caught
    /// on the Transform tool's box: on a corner, the middle of a side or the box's middle, when
    /// it is let go near one.
    fn caught_on_box(&self, center: Pos2, shot: &Shot, peg: PegId, frame: u32, from: [f64; 2], by: egui::Vec2) -> Option<[f64; 2]> {
        let (_, bounds) = self.scale_box.filter(|(of, _)| *of == peg)?;
        let world = space(shot, self.flat, Some(peg), Some(peg), frame);
        let on_screen = |[x, y]: [f64; 2]| {
            let at = world.apply(Point::new(x, y));
            self.view.to_screen(center, [at.x, at.y])
        };
        let moved = on_screen(from) + by;
        let middle = [(bounds[0] + bounds[2]) / 2.0, (bounds[1] + bounds[3]) / 2.0];
        let catches = box_handles(bounds).map(|(at, _)| at).into_iter().chain([middle]);
        let caught = catches.map(|at| (at, (on_screen(at) - moved).length())).filter(|(_, off)| *off <= HANDLE_REACH).min_by(|a, b| a.1.total_cmp(&b.1));
        caught.map(|(at, _)| at)
    }

    /// The Transform tool: dragging moves the peg, dragging its round handle turns it about its
    /// pivot, and dragging a handle of the box round what it carries scales it about its pivot:
    /// both ways alike if `even`, or else a corner each way by its own and a side one way.
    /// On an animated peg's path, dragging a keyframe's square moves that keyframe,
    /// clicking it goes to its frame, and dragging a handle of the keyframe under the playhead
    /// bends the path: the other handle swings round with it, unless Alt is held.
    /// The peg is changed on `frame`: where the value has keyframes, that makes a key there.
    /// With `spot`, the tool has a temporary pivot of its own: dragged from where the pivot is
    /// shown, or from anywhere while `pivoting`, and turned and scaled about in place of the
    /// peg's. With `squash`, a side handle keeps the area: the other way shrinks as it grows.
    fn peg_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        shot: &Shot,
        peg: Option<PegId>,
        frame: u32,
        panning: bool,
        even: bool,
        pivoting: bool,
        spot: bool,
        squash: bool,
    ) -> Option<(&'static str, Value)> {
        let center = response.rect.center();
        let Some(peg) = peg.and_then(|id| shot.peg(id)) else {
            (self.peg_drag, self.hover_handle) = (None, None);
            return None;
        };
        // The pointer says what a press would do: a turn just outside a corner of the box, a
        // skew along a side; and goes on saying it while that is dragged.
        self.hover_handle = match self.peg_drag.map(|drag| drag.grab) {
            Some(Grab::Rotate) => Some(Handle::Rotate),
            Some(Grab::Skew { side, .. }) => Some(Handle::Skew(side)),
            Some(_) => None,
            None => response.hover_pos().filter(|_| !panning && !pivoting).and_then(|pos| {
                let (handle, bounds) = self.box_edge(center, shot, peg.id, frame, pos)?;
                if let (Handle::Rotate, Some(on_screen)) = (handle, self.box_on_screen(center, shot, peg.id, frame, bounds)) {
                    self.turn_facing = facing(&[on_screen[0], on_screen[1], on_screen[2], on_screen[3]], pos, None);
                }
                Some(handle)
            }),
        };
        set_handle_cursor(ui.ctx(), self.hover_handle);
        if !panning && response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()) {
            let pos = response.interact_pointer_pos()?;
            let (pivot, handle) = self.peg_handles(center, &self.posed(shot), peg.id, frame)?;
            if self.peg_drag.is_none() {
                // The press: on a handle of the path, the round handle, a keyframe's square, or
                // anywhere else, which moves the peg.
                let near = |at: Pos2| (at - pos).length() <= HANDLE_REACH;
                let bends = self.bend_handles(center, shot, peg, frame).filter(|_| peg.animation.position.keys().len() > 1);
                let bend = bends.and_then(|(key, into, out)| {
                    let on = |end: Pos2| near(end) && (end - key).length() > 1.0;
                    on(out).then_some(true).or(on(into).then_some(false))
                });
                let key = || self.peg_path(center, shot, peg, frame).into_iter().find(|(_, at, key)| *key && near(*at)).map(|(frame, ..)| frame);
                // An in-between's dot, on a way that moves, where no keyframe's square is.
                let track = &peg.animation.position;
                let tween = || {
                    let (at, ..) = self.peg_path(center, shot, peg, frame).into_iter().find(|(_, at, key)| !*key && near(*at))?;
                    let from = track.keys().iter().rev().find(|key| key.frame < at)?;
                    (from.out != iskra_model::Interp::Hold && track.spacing(from.frame).is_some()).then_some(Grab::Tween { key: from.frame, frame: at })
                };
                // A handle of the box, where no keyframe's square is.
                let scale = || {
                    let bounds = self.scale_box.filter(|(of, _)| *of == peg.id)?.1;
                    // The nearest, where a narrow box has its handles close together.
                    let reach = |at: &Pos2| (*at - pos).length();
                    let on_screen = self.box_on_screen(center, shot, peg.id, frame, bounds)?;
                    let at = (0..8).filter(|at| near(on_screen[*at])).min_by(|a, b| reach(&on_screen[*a]).total_cmp(&reach(&on_screen[*b])))?;
                    let (handle, ways) = box_handles(bounds)[at];
                    Some(Grab::Scale { handle, ways })
                };
                // Just outside a corner of the box, a turn; on a side between its handles, a skew.
                let edge = || match self.box_edge(center, shot, peg.id, frame, pos)? {
                    (Handle::Rotate, _) => Some(Grab::Rotate),
                    (Handle::Skew(side), bounds) => Some(Grab::Skew { side, bounds }),
                    _ => None,
                };
                let grab = match bend {
                    // With a temporary pivot, that is what is moved, from where it is shown.
                    _ if spot && (pivoting || near(pivot)) => Grab::Spot,
                    // Set to move the pivot, the tool does that wherever it is pressed.
                    _ if pivoting => Grab::Pivot,
                    Some(out) => Grab::Bend { frame, out },
                    None if near(handle) => Grab::Rotate,
                    None => key().map(Grab::Key).or_else(tween).or_else(scale).or_else(edge).unwrap_or(Grab::Move),
                };
                if let Grab::Tween { key, .. } = grab {
                    let spacing = track.spacing(key).unwrap_or_default();
                    self.tween = Some((spacing.clone(), spacing));
                }
                // Taken by the round handle, the curved arrow bulges the way the handle is from the pivot.
                if grab == Grab::Rotate && near(handle) {
                    self.turn_facing = (handle - pivot).angle();
                }
                let mut pose = peg.transform_at(frame);
                if let Grab::Key(at) = grab {
                    pose.position = peg.animation.position.key(at).map_or(pose.position, |key| key.value);
                }
                let mut bend = peg.animation.position.handles(frame).unwrap_or_default();
                // A corner shows handles to pull it into a curve by, though it has none: the one
                // that is not pulled stays at nothing.
                let corner = peg.animation.position.key(frame).is_some_and(|key| key.bend == Some(Bend::default()));
                match grab {
                    Grab::Bend { out: true, .. } if corner => bend.into = [0.0, 0.0],
                    Grab::Bend { out: false, .. } if corner => bend.out = [0.0, 0.0],
                    _ => {}
                }
                // The other pegs in hand go along when the peg is moved, turned or scaled.
                let together = matches!(grab, Grab::Move | Grab::Rotate | Grab::Scale { .. });
                let others = self.with_pegs.iter().filter(|other| together && **other != peg.id).filter_map(|other| shot.peg(*other));
                self.peg_along = others.map(|other| (other.id, other.transform_at(frame), other.transform_at(frame))).collect();
                let (start, last, moved) = (pos, pos, false);
                let about = self.temp_pivot.filter(|(of, _)| spot && *of == peg.id).map(|(_, at)| at);
                self.peg_drag = Some(PegDrag { peg: peg.id, grab, start, last, moved, from: pose, now: pose, bend_from: bend, bend, corner, about });
            }
            let mut drag = self.peg_drag?;
            drag.moved |= (pos - drag.start).length() >= CLICK_SLOP;
            // What is above the peg says which way its own axes point in the scene.
            let above = peg_space(shot, self.flat, peg, frame);
            let in_peg_space = |at: Pos2| {
                let [x, y] = self.view.to_scene(center, at);
                above.inverse().map(|back| back.apply(Point::new(x, y)))
            };
            match drag.grab {
                Grab::Rotate => {
                    // How far the pointer went round the pivot since the last frame: added up, so
                    // a drag can go round more than once.
                    let (from, to) = (drag.last - pivot, pos - pivot);
                    let turned = f64::from(to.y.atan2(to.x) - from.y.atan2(from.x)).to_degrees();
                    let turned = (turned + 180.0).rem_euclid(360.0) - 180.0;
                    // Under a mirrored peg, clockwise on screen is the other way round for this one.
                    drag.now.rotation += turned * above.determinant().signum();
                }
                Grab::Pivot => {
                    // How far the pointer went in the peg's own space, as it was before the
                    // drag: the pivot goes as far, and the peg is moved back by what that
                    // would move what it carries, which is how far its turn and its scale
                    // take the pivot's step from where the step itself goes.
                    let own = space(shot, self.flat, Some(peg.id), Some(peg.id), frame).inverse();
                    let own = |at: Pos2| {
                        let [x, y] = self.view.to_scene(center, at);
                        own.map(|back| back.apply(Point::new(x, y)))
                    };
                    if let (Some(from), Some(to)) = (own(drag.start), own(pos)) {
                        let mut by = [to.x - from.x, to.y - from.y];
                        // Near a corner of the box, the middle of a side or the box's middle,
                        // the pivot is caught there.
                        if let Some(at) = self.caught_on_box(center, shot, peg.id, frame, drag.from.pivot, pos - drag.start) {
                            by = [at[0] - drag.from.pivot[0], at[1] - drag.from.pivot[1]];
                        }
                        drag.now = pivot_moved(drag.from, by);
                    }
                }
                Grab::Spot => {
                    // As the pivot is moved, and caught as it is, but nothing else changes.
                    let own = space(shot, self.flat, Some(peg.id), Some(peg.id), frame).inverse();
                    let own = |at: Pos2| {
                        let [x, y] = self.view.to_scene(center, at);
                        own.map(|back| back.apply(Point::new(x, y)))
                    };
                    if let (Some(from), Some(to)) = (own(drag.start), own(pos)) {
                        let was = drag.about.unwrap_or(drag.from.pivot);
                        let at = self.caught_on_box(center, shot, peg.id, frame, was, pos - drag.start);
                        self.temp_pivot = Some((peg.id, at.unwrap_or([was[0] + to.x - from.x, was[1] + to.y - from.y])));
                    }
                }
                Grab::Scale { handle, ways } => {
                    // Where the handle was pulled to, in the peg's own space as it was before
                    // the drag: the scale is by as much as that is further from the pivot.
                    let own = space(shot, self.flat, Some(peg.id), Some(peg.id), frame).inverse();
                    let own = |at: Pos2| {
                        let [x, y] = self.view.to_scene(center, at);
                        own.map(|back| back.apply(Point::new(x, y)))
                    };
                    if let (Some(from), Some(to)) = (own(drag.start), own(pos)) {
                        let pivot = drag.about.unwrap_or(drag.from.pivot);
                        let reach = [handle[0] - pivot[0], handle[1] - pivot[1]];
                        let pulled = [reach[0] + to.x - from.x, reach[1] + to.y - from.y];
                        // A way the handle is not out from the pivot in can't be scaled by it.
                        let by = |way: usize| if ways[way] && reach[way].abs() > 1e-6 { pulled[way] / reach[way] } else { 1.0 };
                        let by = match (even, ways) {
                            // Squashed and stretched, the other way goes by as much the other way.
                            _ if squash && ways == [true, false] => [by(0), 1.0 / by(0).abs().max(1e-6)],
                            _ if squash && ways == [false, true] => [1.0 / by(1).abs().max(1e-6), by(1)],
                            (false, _) => [by(0), by(1)],
                            // A corner goes out and in along the line from the pivot to it.
                            (true, [true, true]) => {
                                let along = (pulled[0] * reach[0] + pulled[1] * reach[1]) / (reach[0] * reach[0] + reach[1] * reach[1]).max(1e-12);
                                [along, along]
                            }
                            (true, [true, _]) => [by(0), by(0)],
                            (true, _) => [by(1), by(1)],
                        };
                        // Never to nothing, where there would be no drawing to take hold of.
                        let kept = |scale: f64| scale.abs().clamp(0.01, 100.0).copysign(scale);
                        drag.now.scale = [kept(drag.from.scale[0] * by[0]), kept(drag.from.scale[1] * by[1])];
                    }
                }
                Grab::Skew { side, bounds: [x0, y0, x1, y1] } => {
                    // How far the side was slid along itself, in the peg's own space as it was
                    // before the drag, against how far the side is from the pivot across itself.
                    let own = space(shot, self.flat, Some(peg.id), Some(peg.id), frame).inverse();
                    let own = |at: Pos2| {
                        let [x, y] = self.view.to_scene(center, at);
                        own.map(|back| back.apply(Point::new(x, y)))
                    };
                    if let (Some(from), Some(to)) = (own(drag.start), own(pos)) {
                        let pivot = drag.about.unwrap_or(drag.from.pivot);
                        // The top and the bottom slide across, the right and the left up and down.
                        let (reach, slid) = match side {
                            0 => (y0 - pivot[1], to.x - from.x),
                            2 => (y1 - pivot[1], to.x - from.x),
                            1 => (x1 - pivot[0], to.y - from.y),
                            _ => (x0 - pivot[0], to.y - from.y),
                        };
                        if reach.abs() > 1e-6 {
                            let k = slid / reach;
                            let shear = if side % 2 == 0 { Affine { m: [1.0, 0.0, k, 1.0, 0.0, 0.0] } } else { Affine { m: [1.0, k, 0.0, 1.0, 0.0, 0.0] } };
                            let linear = Transform { position: [0.0, 0.0], pivot: [0.0, 0.0], ..drag.from }.matrix().mul(&shear);
                            (drag.now.rotation, drag.now.skew, drag.now.scale) = turn_skew_scale(&linear, &drag.from);
                        }
                    }
                }
                Grab::Move | Grab::Key(_) => {
                    if let (Some(from), Some(to)) = (in_peg_space(drag.start), in_peg_space(pos)) {
                        drag.now.position = [drag.from.position[0] + to.x - from.x, drag.from.position[1] + to.y - from.y];
                    }
                }
                Grab::Tween { key, frame: at } => {
                    // How far along the way the point of it nearest the pointer is: the way
                    // as a steady one would go, every frame between the keys at the same part.
                    if let (Some(to), Some((was, now))) = (in_peg_space(pos), self.tween.as_mut()) {
                        let track = &peg.animation.position;
                        let [px, py] = peg.transform.pivot;
                        let place = |part: f64| {
                            let mut steady = track.clone();
                            steady.set_chart(key, &vec![part; was.len()]);
                            steady.at(at, true).map(|[x, y]| [x + px, y + py])
                        };
                        const STEPS: usize = 64;
                        let way: Vec<(f64, [f64; 2])> =
                            (0..=STEPS).filter_map(|step| Some((step as f64 / STEPS as f64, place(step as f64 / STEPS as f64)?))).collect();
                        let nearest = way
                            .windows(2)
                            .map(|pair| {
                                let ((p0, [x0, y0]), (p1, [x1, y1])) = (pair[0], pair[1]);
                                let (dx, dy) = (x1 - x0, y1 - y0);
                                let along = (((to.x - x0) * dx + (to.y - y0) * dy) / (dx * dx + dy * dy).max(1e-12)).clamp(0.0, 1.0);
                                let off = (x0 + along * dx - to.x).hypot(y0 + along * dy - to.y);
                                (off, p0 + along * (p1 - p0))
                            })
                            .min_by(|a, b| a.0.total_cmp(&b.0));
                        if let Some((_, part)) = nearest {
                            let line = (at - key - 1) as usize;
                            *now = iskra_model::squash(was, line, part, self.breakdowns.rule_now(ui), self.breakdowns.other_side);
                        }
                    }
                }
                Grab::Bend { out, .. } => {
                    if let (Some(from), Some(to)) = (in_peg_space(drag.start), in_peg_space(pos)) {
                        let (held, other) = if out { (drag.bend_from.out, drag.bend_from.into) } else { (drag.bend_from.into, drag.bend_from.out) };
                        let held = [held[0] + to.x - from.x, held[1] + to.y - from.y];
                        // The other handle keeps its length and points the opposite way, so the
                        // path stays smooth through the key. With Alt it stays where it is, which
                        // breaks the path there, and at a corner it stays at nothing.
                        let (length, reach) = (held[0].hypot(held[1]), other[0].hypot(other[1]));
                        let stays = drag.corner || ui.input(|i| i.modifiers.alt) || length < 1e-9;
                        let swung = if stays { other } else { [-held[0] / length * reach, -held[1] / length * reach] };
                        drag.bend = if out { Bend { into: swung, out: held } } else { Bend { into: held, out: swung } };
                    }
                }
            }
            // Turned or scaled about the temporary pivot, the peg is moved by as much as keeps
            // that point where it was.
            if let (Some(about), Grab::Rotate | Grab::Scale { .. } | Grab::Skew { .. }) = (drag.about, drag.grab) {
                let kept = Transform { position: drag.from.position, ..drag.now };
                let (was, is) = (drag.from.matrix().apply(Point::new(about[0], about[1])), kept.matrix().apply(Point::new(about[0], about[1])));
                drag.now.position = [drag.from.position[0] + was.x - is.x, drag.from.position[1] + was.y - is.y];
            }
            drag.last = pos;
            self.peg_drag = Some(drag);
            // The pegs that go along: moved as far in the scene, and turned and scaled about
            // the pivot of the one that is dragged, as one thing with it.
            let (from, now, grab) = (drag.from, drag.now, drag.grab);
            let [cx, cy] = drag.about.unwrap_or(from.pivot);
            let centre = above.apply(from.matrix().apply(Point::new(cx, cy)));
            // Of a step in the scene, what the drag makes: for a move the same step and the
            // move added, for a turn the step turned, for a scale the step scaled along the
            // peg's own axes.
            let turn = ((now.rotation - from.rotation) * above.determinant().signum()).to_radians();
            let by = [0, 1].map(|way| if from.scale[way].abs() > 1e-9 { now.scale[way] / from.scale[way] } else { 1.0 });
            let axes = above.mul(&Transform { position: [0.0, 0.0], pivot: [0.0, 0.0], scale: [1.0, 1.0], ..from }.matrix());
            // A turn or a scale moves the peg only to keep a temporary pivot still: the turn
            // about it takes the others along already.
            let moved = match grab {
                Grab::Rotate | Grab::Scale { .. } | Grab::Skew { .. } => [0.0, 0.0],
                _ => linear(&above, [now.position[0] - from.position[0], now.position[1] - from.position[1]]),
            };
            let carried = |step: [f64; 2]| -> [f64; 2] {
                match grab {
                    Grab::Rotate => [step[0] * turn.cos() - step[1] * turn.sin(), step[0] * turn.sin() + step[1] * turn.cos()],
                    Grab::Scale { .. } => match axes.inverse() {
                        Some(back) => {
                            let own = linear(&back, step);
                            linear(&axes, [own[0] * by[0], own[1] * by[1]])
                        }
                        None => step,
                    },
                    _ => step,
                }
            };
            for (id, was, is) in &mut self.peg_along {
                let Some(other) = shot.peg(*id) else { continue };
                let over = peg_space(shot, self.flat, other, frame);
                let Some(back) = over.inverse() else { continue };
                // Where its pivot is in the scene, and where the drag takes that.
                let pivot = over.apply(Point::new(was.position[0] + was.pivot[0], was.position[1] + was.pivot[1]));
                let step = carried([pivot.x - centre.x, pivot.y - centre.y]);
                let to = back.apply(Point::new(centre.x + step[0] + moved[0], centre.y + step[1] + moved[1]));
                is.position = [to.x - was.pivot[0], to.y - was.pivot[1]];
                match grab {
                    Grab::Rotate => is.rotation = was.rotation + turn.to_degrees() * over.determinant().signum(),
                    Grab::Scale { .. } => is.scale = [was.scale[0] * by[0], was.scale[1] * by[1]],
                    _ => {}
                }
            }
            return None;
        }
        let drag = self.peg_drag.take()?;
        // Shown where it was let go for the one frame until the command is run, with what
        // went along, so that nothing springs back for a moment.
        let along = self.peg_along.clone();
        if drag.grab == Grab::Spot {
            return None;
        }
        if drag.moved {
            self.peg_held = Some(drag);
        } else {
            self.peg_along.clear();
        }
        let at = |frame: u32| json!({ "peg": drag.peg.0, "frame": frame });
        let (command, mut params) = match drag.grab {
            // A click on a keyframe's square goes to its frame.
            Grab::Key(key) if !drag.moved => {
                self.seek = Some(key);
                return None;
            }
            _ if !drag.moved => return None,
            Grab::Rotate | Grab::Pivot | Grab::Scale { .. } | Grab::Skew { .. } | Grab::Move | Grab::Spot => ("peg.setTransform", at(frame)),
            // An in-between slid along the path times the way of the position from its key.
            Grab::Tween { key, .. } => {
                let to = peg.animation.position.keys().iter().map(|key| key.frame).find(|at| *at > key)?;
                let spacing = self.tween.as_ref().map(|(_, now)| now.clone())?;
                let owner = json!({ "peg": drag.peg.0 });
                return Some(("keys.chart", json!({ "owner": owner, "value": "position", "from": key, "to": to, "spacing": spacing })));
            }
            // A keyframe's square that is dragged is that key moved, whatever the Animate
            // switch says: saying nothing here keeps the app from adding its say.
            Grab::Key(key) => ("peg.setTransform", json!({ "peg": drag.peg.0, "frame": key, "animate": null })),
            Grab::Bend { frame, .. } => ("peg.setBend", at(frame)),
        };
        match drag.grab {
            Grab::Rotate => params["rotation"] = json!(drag.now.rotation),
            Grab::Pivot => (params["pivot"], params["position"]) = (json!(drag.now.pivot), json!(drag.now.position)),
            Grab::Scale { .. } => params["scale"] = json!(drag.now.scale),
            // A side slid up or down turns and scales the peg too, as its skew is across.
            Grab::Skew { .. } => {
                (params["rotation"], params["skew"], params["scale"]) = (json!(drag.now.rotation), json!(drag.now.skew), json!(drag.now.scale));
            }
            Grab::Spot | Grab::Tween { .. } => {}
            Grab::Move | Grab::Key(_) => params["position"] = json!(drag.now.position),
            Grab::Bend { .. } => (params["into"], params["out"]) = (json!(drag.bend.into), json!(drag.bend.out)),
        }
        // About a temporary pivot, the peg is moved as well as turned or scaled.
        if drag.about.is_some() && matches!(drag.grab, Grab::Rotate | Grab::Scale { .. } | Grab::Skew { .. }) {
            params["position"] = json!(drag.now.position);
        }
        // With pegs that went along, it is one step of them all.
        if !along.is_empty() {
            let mut commands = vec![json!({ "id": command, "params": params })];
            for (id, _, now) in along {
                let mut params = json!({ "peg": id.0, "frame": frame, "position": now.position });
                match drag.grab {
                    Grab::Rotate => params["rotation"] = json!(now.rotation),
                    Grab::Scale { .. } => params["scale"] = json!(now.scale),
                    _ => {}
                }
                commands.push(json!({ "id": "peg.setTransform", "params": params }));
            }
            let title = match drag.grab {
                Grab::Rotate => "Turn Pegs",
                Grab::Scale { .. } => "Scale Pegs",
                _ => "Move Pegs",
            };
            return Some(("edit.batch", json!({ "commands": commands, "title": title })));
        }
        Some((command, params))
    }

    /// Where a point of the camera's frame, in the frame's units from its middle, is on the
    /// screen: as the view has it through the camera, and in the flat view where the camera
    /// sees it on the plane at depth 0.
    fn frame_point(&self, center: Pos2, shot: &Shot, frame: u32, [x, y]: [f64; 2]) -> Option<Pos2> {
        if !self.flat {
            return Some(self.view.to_screen(center, [x, y]));
        }
        let at = shot.seen(0.0, frame)?.inverse()?.apply(Point::new(x, y));
        Some(self.view.to_screen(center, [at.x, at.y]))
    }

    /// The Camera tool's frame: a corner of the camera's frame dragged makes the frame
    /// smaller or larger, which zooms the camera in or out, and the corner across from it
    /// stays, so the camera moves too, to look at the middle of what the frame holds. With
    /// Alt, or while the camera has no peg to move it, the frame keeps its middle. `None`
    /// when the pen is not on a corner; `Some` while one is dragged, with the command that
    /// zooms and moves the camera, as one step, when it is let go.
    #[allow(clippy::too_many_arguments)]
    fn frame_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        shot: &Shot,
        peg: Option<PegId>,
        frame: u32,
        [w, h]: [f64; 2],
        panning: bool,
    ) -> Option<Option<(&'static str, Value)>> {
        let center = response.rect.center();
        let corners = [[-w, -h], [w, -h], [w, h], [-w, h]];
        let seen = shot.seen(0.0, frame)?;
        if !panning && response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()) {
            let pos = response.interact_pointer_pos()?;
            if self.frame_drag.is_none() {
                // Not while the peg itself is dragged, and only from a corner.
                if self.peg_drag.is_some() {
                    return None;
                }
                let near = |corner: &[f64; 2]| self.frame_point(center, shot, frame, *corner).is_some_and(|at| (at - pos).length() <= HANDLE_REACH);
                self.frame_drag = Some(FrameDrag { corner: corners.iter().position(near)?, scale: 1.0, centre: [0.0, 0.0] });
            }
            // The pointer in the frame's units.
            let [x, y] = self.view.to_scene(center, pos);
            let at = if self.flat { seen.apply(Point::new(x, y)) } else { Point::new(x, y) };
            let centred = peg.is_none() || ui.input(|i| i.modifiers.alt);
            let zoom = shot.camera.zoom_at(frame);
            let drag = self.frame_drag.as_mut()?;
            let grabbed = corners[drag.corner];
            let stays = if centred { [0.0, 0.0] } else { [-grabbed[0], -grabbed[1]] };
            // As far along the frame's diagonal as the pointer is, so the frame keeps its shape.
            let across = [grabbed[0] - stays[0], grabbed[1] - stays[1]];
            let along = ((at.x - stays[0]) * across[0] + (at.y - stays[1]) * across[1]) / (across[0] * across[0] + across[1] * across[1]);
            // Within what the zoom can be.
            drag.scale = along.clamp(zoom / 100.0, zoom / 0.01).clamp(0.02, 50.0);
            drag.centre = [stays[0] + across[0] * drag.scale - grabbed[0] * drag.scale, stays[1] + across[1] * drag.scale - grabbed[1] * drag.scale];
            return Some(None);
        }
        let drag = self.frame_drag.take()?;
        if (drag.scale - 1.0).abs() < 1e-3 {
            return Some(None);
        }
        let zoom = (shot.camera.zoom_at(frame) / drag.scale).clamp(0.01, 100.0);
        let mut commands = vec![json!({ "id": "camera.setLens", "params": { "shot": shot.id.0, "frame": frame, "zoom": zoom } })];
        // The camera goes to look at what the frame's new middle was on: by its peg, in the
        // space of what the peg is under.
        let moved = peg.and_then(|id| shot.peg(id)).filter(|_| drag.centre != [0.0, 0.0]).and_then(|peg| {
            let (back, above) = (seen.inverse()?, shot.above(peg, frame).inverse()?);
            let in_parent = |[x, y]: [f64; 2]| above.apply(back.apply(Point::new(x, y)));
            let (from, to, pose) = (in_parent([0.0, 0.0]), in_parent(drag.centre), peg.transform_at(frame));
            Some((peg.id, [pose.position[0] + to.x - from.x, pose.position[1] + to.y - from.y]))
        });
        if let Some((peg, position)) = moved {
            commands.push(json!({ "id": "peg.setTransform", "params": { "peg": peg.0, "frame": frame, "position": position } }));
        }
        Some(Some(("edit.batch", json!({ "commands": commands }))))
    }

    /// The outline of a rectangle of the drawing in hand, on screen: its four corners, or on a
    /// layer that bones bend its sides in short pieces, each where the bones carry it, so that
    /// the outline lies on what it selects.
    fn outlined(&self, center: Pos2, to_scene: &Affine, rect: PixelRect, matrix: &Affine) -> Vec<Pos2> {
        let corners = select::corners(rect);
        let Some(warp) = &self.bend else { return self.handles(center, to_scene, rect, matrix).0.to_vec() };
        const PIECES: usize = 12;
        let along = (0..4).flat_map(|side| {
            let (from, to) = (corners[side], corners[(side + 1) % 4]);
            (0..PIECES).map(move |piece| {
                let part = piece as f64 / PIECES as f64;
                Point::new(from.x + (to.x - from.x) * part, from.y + (to.y - from.y) * part)
            })
        });
        along
            .map(|p| {
                let p = to_scene.apply(warp.apply(matrix.apply(p)));
                self.view.to_screen(center, [p.x, p.y])
            })
            .collect()
    }

    /// Outline the selection, with its handles, or the rectangle being dragged out. `place` is
    /// where the layer the selection is on sits.
    fn paint_selection(&self, painter: &egui::Painter, place: Option<Placement>) {
        let Some(place) = place.map(|place| place.to_scene) else { return };
        let center = painter.clip_rect().center();
        let outline = |points: &[Pos2]| {
            painter.add(Shape::closed_line(points.to_vec(), Outline::new(1.0, Color32::from_gray(30))));
        };
        if let Some((from, to)) = self.marquee {
            let rect = PixelRect::new(from.x.min(to.x) as i32, from.y.min(to.y) as i32, from.x.max(to.x) as i32, from.y.max(to.y) as i32);
            outline(&self.outlined(center, &place, rect, &Affine::IDENTITY));
        }
        // A point of the drawing on screen, moved by `matrix` first and bent as the layer is.
        let seen = |at: Point, matrix: &Affine| {
            let moved = matrix.apply(at);
            let p = place.apply(self.bend.as_ref().map_or(moved, |warp| warp.apply(moved)));
            self.view.to_screen(center, [p.x, p.y])
        };
        if let Some(drawn) = self.lassoing.as_ref().filter(|drawn| drawn.len() > 1) {
            outline(&drawn.iter().map(|at| seen(*at, &Affine::IDENTITY)).collect::<Vec<_>>());
        }
        let Some(selection) = &self.selection else { return };
        let matrix = self.dragging().map_or(Affine::IDENTITY, |d| d.matrix);
        // What is selected of the other layers' drawings, fainter, moved as the drag has it.
        let moved = place.inverse().map_or(Affine::IDENTITY, |back| place.mul(&matrix).mul(&back));
        for (other, to_scene) in &self.also {
            let on_screen = |at: &Point| {
                let p = moved.apply(to_scene.apply(*at));
                self.view.to_screen(center, [p.x, p.y])
            };
            for part in &other.parts {
                let points: Vec<Pos2> = part.lasso.points().iter().map(on_screen).collect();
                painter.add(Shape::closed_line(points, Outline::new(1.0, Color32::from_gray(30))));
            }
        }
        // Once made, the selection is shown by its box and handles alone, not by the rectangle
        // or the outline it was drawn with (the user's of 2026-10-27).
        let (corners, rotate) = self.handles(center, &place, selection.rect, &matrix);
        outline(&self.outlined(center, &place, selection.rect, &matrix));
        let top = corners[0] + (corners[1] - corners[0]) / 2.0;
        painter.line_segment([top, rotate], Outline::new(1.0, Color32::from_gray(30)));
        painter.circle(rotate, 4.5, Color32::WHITE, Outline::new(1.0, Color32::from_gray(30)));
        #[cfg(test)]
        {
            *self.side_handles.borrow_mut() = sides(&corners).to_vec();
        }
        for corner in corners.into_iter().chain(sides(&corners)) {
            painter.rect(
                egui::Rect::from_center_size(corner, egui::vec2(7.0, 7.0)),
                0.0,
                Color32::WHITE,
                Outline::new(1.0, Color32::from_gray(30)),
                egui::StrokeKind::Middle,
            );
        }
        // Where a press turns the selection, the pointer is a curved arrow.
        if let (Some(Handle::Rotate), Some(at)) = (self.hover_handle, self.pointer) {
            paint_turn_cursor(painter, at, self.turn_facing);
        }
    }

    /// Draw an indexed layer's tiles through the view.
    #[allow(clippy::too_many_arguments)]
    /// The areas of `drawing` that `palette` paints with gradients and pictures, found once for
    /// each, or `None` where it paints none.
    fn areas_of(&mut self, drawing: &Arc<iskra_model::Drawing>, palette: &Arc<Palette>) -> Option<Arc<iskra_model::Areas>> {
        if !palette.is_shaded() {
            return None;
        }
        let now = self.drawn;
        let key = (Arc::as_ptr(drawing) as usize, Arc::as_ptr(palette) as usize);
        let found = self.areas.entry(key).or_insert_with(|| (drawing.clone(), palette.clone(), Arc::new(palette.areas_of(drawing)), now));
        found.3 = now;
        let areas = found.2.clone();
        if self.areas.len() > 64 {
            self.areas.retain(|_, (.., used)| *used + 2 > now);
        }
        (!areas.is_empty()).then_some(areas)
    }

    fn paint_layer(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        layer: Pixels,
        palette: &Arc<Palette>,
        to_scene: &Projection,
        warp: Option<&Warp>,
        look: Look,
    ) {
        // On the card, the drawings are in the compositor's picture.
        if self.from_card {
            return;
        }
        let center = painter.clip_rect().center();
        // A layer seen in perspective is drawn in small squares: across each, the picture is
        // stretched evenly, which over a small one is near enough to how it recedes. So is a
        // layer that a deformer bends (plan §4.5): the corners of the squares go where the
        // deformer puts them.
        let cells: usize = match (warp, to_scene.as_flat()) {
            (Some(_), _) => BENT_CELLS,
            (None, Some(_)) => 1,
            (None, None) => TILTED_CELLS,
        };
        for (coord, tile) in layer.tiles().tiles() {
            let bounds = coord.rect();
            // Where the corners of the tile's squares are on screen; `None` behind the camera.
            let at = |i: usize, j: usize| {
                let part = |from: i32, to: i32, step: usize| f64::from(from) + f64::from(to - from) * step as f64 / cells as f64;
                let drawn = Point::new(part(bounds.x0, bounds.x1, i), part(bounds.y0, bounds.y1, j));
                let p = to_scene.apply(warp.map_or(drawn, |warp| warp.apply(drawn)))?;
                Some(self.view.to_screen(center, look.shift.apply([p.x, p.y])))
            };
            let grid: Vec<Option<Pos2>> = (0..=cells).flat_map(|j| (0..=cells).map(move |i| (i, j))).map(|(i, j)| at(i, j)).collect();
            let seen: Vec<Pos2> = grid.iter().flatten().copied().collect();
            if seen.is_empty() || !egui::Rect::from_points(&seen).intersects(painter.clip_rect()) {
                continue;
            }
            // An outline is as wide on screen at every zoom: so many of the drawing's pixels
            // as the tile's top and left sides show it.
            let outline = (look.silhouette && look.outline).then(|| {
                let side = |a: Option<Pos2>, b: Option<Pos2>| a.zip(b).map_or(0.0, |(a, b)| a.distance(b));
                let across = side(grid[0], grid[cells]).max(side(grid[0], grid[cells * (cells + 1)]));
                crate::onion::outline_reach(across / TILE as f32)
            });
            // A silhouette doesn't depend on the palette, nor does its outline, which is kept
            // for each width; numbers this small are no palette's address.
            // A tile of a drawing painted with gradients is of its areas too.
            let areas = match layer {
                Pixels::Shaded { areas, .. } if !look.silhouette => Some(areas.clone()),
                _ => None,
            };
            let key = (
                Arc::as_ptr(tile) as usize,
                if look.silhouette { outline.unwrap_or(0) } else { Arc::as_ptr(palette) as usize },
                areas.as_ref().map_or(0, |areas| Arc::as_ptr(areas) as usize),
            );
            let now = self.drawn;
            let entry = self.textures.entry(key).or_insert_with(|| TileTexture {
                texture: {
                    crate::timings::count("tile textures made", 1);
                    ui.ctx().load_texture("tile", layer.tile_image(bounds, palette, look.silhouette, outline), TextureOptions::LINEAR)
                },
                used: now,
                _tile: tile.clone(),
                _palette: palette.clone(),
                _areas: areas,
            });
            entry.used = now;
            #[cfg(test)]
            {
                self.bent_tiles += usize::from(warp.is_some());
            }
            let mut mesh = egui::Mesh::with_texture(entry.texture.id());
            for j in 0..cells {
                for i in 0..cells {
                    // A square with a corner behind the camera is left out.
                    let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                    let Some(places) = corners.iter().map(|(i, j)| grid[j * (cells + 1) + i]).collect::<Option<Vec<Pos2>>>() else { continue };
                    let first = mesh.vertices.len() as u32;
                    let uv = |(i, j): (usize, usize)| Pos2::new(i as f32 / cells as f32, j as f32 / cells as f32);
                    mesh.vertices.extend(places.iter().zip(corners).map(|(&pos, corner)| Vertex { pos, uv: uv(corner), color: look.color }));
                    mesh.indices.extend([0, 1, 2, 0, 2, 3].map(|index| first + index));
                }
            }
            painter.add(Shape::mesh(mesh));
        }
    }
}

impl Viewport {
    /// While the shot plays: make some of the textures the frames just ahead will need, so
    /// that they are there when the playhead is.
    fn warm(&mut self, ui: &egui::Ui, shot: &Arc<Shot>, palette: &Arc<Palette>) {
        let mut made = 0;
        for frame in self.ahead.clone() {
            let coming = self.frames.frame(shot, frame);
            let drawings = coming.layers.iter().filter_map(|placed| shot.layer(placed.layer)?.drawing(placed.drawing?));
            for art in drawings.flat_map(|entry| &entry.drawing.art_layers) {
                let surface = Pixels::of(&art.content);
                for (coord, tile) in surface.tiles().tiles() {
                    let key = (Arc::as_ptr(tile) as usize, Arc::as_ptr(palette) as usize, 0);
                    if self.textures.contains_key(&key) {
                        continue;
                    }
                    if made == WARMED_A_FRAME {
                        return;
                    }
                    made += 1;
                    crate::timings::count("tile textures made ahead", 1);
                    let texture = ui.ctx().load_texture("tile", surface.tile_image(coord.rect(), palette, false, None), TextureOptions::LINEAR);
                    // As if drawn a frame ago: kept as long as what is on screen, but not
                    // counted as on it.
                    self.textures
                        .insert(key, TileTexture { texture, used: self.drawn.saturating_sub(1), _tile: tile.clone(), _palette: palette.clone(), _areas: None });
                }
            }
        }
    }
}

/// One tile of an indexed layer as an image: coloured from the palette, or as a white
/// silhouette of its coverage to be tinted.
/// An art layer's pixels, of either kind, for the tiles the viewport draws them from.
#[derive(Clone, Copy)]
enum Pixels<'a> {
    /// Swatches and coverage, coloured from the palette.
    Indexed(&'a IndexedSurface),
    /// The same, of art layer `art` of a drawing whose `areas` some of its swatches paint with
    /// gradients and pictures, blended in linear light when `linear`.
    Shaded { surface: &'a IndexedSurface, art: ArtLayerKind, areas: &'a Arc<iskra_model::Areas>, linear: bool },
    /// Colours of their own: a picture that was brought in.
    Rgba(&'a Surface),
}

impl<'a> Pixels<'a> {
    fn of(content: &'a ArtContent) -> Self {
        match content {
            ArtContent::Indexed(surface) => Pixels::Indexed(surface),
            ArtContent::Vector(art) => Pixels::Indexed(art.raster()),
            ArtContent::Rgba(surface) => Pixels::Rgba(surface),
        }
    }

    fn tiles(self) -> &'a Surface {
        match self {
            Pixels::Indexed(surface) | Pixels::Shaded { surface, .. } => surface.tiles(),
            Pixels::Rgba(surface) => surface,
        }
    }

    /// One tile as an image: in its colours, or as a white silhouette of what it covers, to
    /// be tinted; or with `outline` the edges of that silhouette alone, so many pixels wide.
    fn tile_image(self, bounds: PixelRect, palette: &Palette, silhouette: bool, outline: Option<usize>) -> egui::ColorImage {
        if let Some(reach) = outline.filter(|_| silhouette) {
            // What is covered a little way round the tile says where the tile's own edges are.
            let around = bounds.inflate(reach as i32);
            let covered: Vec<u8> = match self {
                Pixels::Indexed(surface) | Pixels::Shaded { surface, .. } => surface.to_rgba8(around, |_| [255; 4]).iter().map(|pixel| pixel[3]).collect(),
                Pixels::Rgba(surface) => {
                    let mut pixels = vec![[0u8; 4]; around.width() as usize * around.height() as usize];
                    surface.read_rgba8_into(around, &mut pixels);
                    pixels.iter().map(|pixel| pixel[3]).collect()
                }
            };
            let side = TILE + 2 * reach;
            let edge = crate::onion::edges(&covered, side, side, reach);
            let pixels: Vec<[u8; 4]> = (0..TILE * TILE).map(|at| [255, 255, 255, edge[(at / TILE + reach) * side + at % TILE + reach]]).collect();
            return egui::ColorImage::from_rgba_unmultiplied([TILE, TILE], pixels.as_flattened());
        }
        let pixels = match self {
            Pixels::Indexed(surface) | Pixels::Shaded { surface, .. } if silhouette => surface.to_rgba8(bounds, |_| [255; 4]),
            Pixels::Indexed(surface) => surface.to_rgba8(bounds, |swatch| palette.color(swatch)),
            Pixels::Shaded { surface, art, areas, linear } => iskra_drawing::shaded(surface, art, bounds, areas, |swatch| palette.shade(swatch), linear),
            Pixels::Rgba(surface) => {
                let mut pixels = vec![[0u8; 4]; TILE * TILE];
                surface.read_rgba8_into(bounds, &mut pixels);
                if silhouette {
                    pixels.iter_mut().for_each(|pixel| *pixel = [255, 255, 255, pixel[3]]);
                }
                pixels
            }
        };
        egui::ColorImage::from_rgba_unmultiplied([TILE, TILE], pixels.as_flattened())
    }
}

/// The middles of the sides of a box whose `corners` are clockwise from the top left: the
/// top's first, clockwise.
/// Whether the way the pointer went, in pixels, is a flick and not an outline round
/// something: what it would enclose is small for how long it is, or it ends far from where it
/// began, as an outline round something does not, however it bends on the way.
fn is_flick(outline: &[Point]) -> bool {
    let length: f64 = outline.windows(2).map(|pair| (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y)).sum();
    let twice: f64 = outline.iter().zip(outline.iter().cycle().skip(1)).map(|(a, b)| a.x * b.y - b.x * a.y).sum();
    let open = match (outline.first(), outline.last()) {
        (Some(first), Some(last)) => (last.x - first.x).hypot(last.y - first.y) >= length * FLICK_OPEN,
        _ => false,
    };
    open || (twice / 2.0).abs() <= length * length * 0.02
}

/// How far apart a flick's ends are at the least, for how long it is: a lasso comes back near
/// where it began, a C round something to a third of its length or so.
const FLICK_OPEN: f64 = 0.45;

/// A curved arrow at `at`, a gentle arc with a head at each end, bulging the way `facing` (an
/// angle on screen) says: the pointer where a press turns a selection or a peg. Facing up and
/// to the right, it goes from the top left round to the bottom right.
pub(crate) fn paint_turn_cursor(painter: &egui::Painter, at: Pos2, facing: f32) {
    // A little more than a quarter of a ring, its chord's middle on the pointer.
    let turned = facing + std::f32::consts::FRAC_PI_4;
    let (radius, from, to) = (12.0_f32, -1.66_f32 + turned, 0.09_f32 + turned);
    let middle = (from + to) / 2.0;
    let centre = at - egui::vec2(middle.cos(), middle.sin()) * radius * ((to - from) / 2.0).cos();
    let on = |angle: f32| centre + egui::vec2(angle.cos(), angle.sin()) * radius;
    let arc: Vec<Pos2> = (0..=12).map(|step| on(from + (to - from) * step as f32 / 12.0)).collect();
    // A head at each end, pointing on along the ring.
    let head = |angle: f32, way: f32| {
        let tip = on(angle);
        let along = egui::vec2(-angle.sin(), angle.cos()) * way;
        let out = egui::vec2(angle.cos(), angle.sin());
        vec![tip + along * 4.0, tip - along * 1.0 + out * 3.5, tip - along * 1.0 - out * 3.5]
    };
    let colour = Color32::from_gray(20);
    painter.add(Shape::line(arc, Outline::new(1.5, colour)));
    for (angle, way) in [(to, 1.0), (from, -1.0)] {
        painter.add(Shape::convex_polygon(head(angle, way), colour, Outline::NONE));
    }
}

/// The pointer for a handle of a box: none for a turn, which the viewport draws as a curved
/// arrow, a two-way arrow along a side for a skew, and the cross for a move.
pub(crate) fn set_handle_cursor(ctx: &egui::Context, handle: Option<Handle>) {
    match handle {
        Some(Handle::Rotate) => ctx.set_cursor_icon(egui::CursorIcon::None),
        Some(Handle::Skew(side)) if side % 2 == 0 => ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal),
        Some(Handle::Skew(_)) => ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical),
        Some(Handle::Move) => ctx.set_cursor_icon(egui::CursorIcon::Move),
        _ => {}
    }
}

/// Which way the curved arrow of a turn at `pos` bulges: out from the middle of the box whose
/// `corners` are on screen, through the round `handle` if the pointer is on it, else through
/// the corner nearest the pointer.
pub(crate) fn facing(corners: &[Pos2; 4], pos: Pos2, handle: Option<Pos2>) -> f32 {
    let middle = corners.iter().fold(egui::Vec2::ZERO, |sum, corner| sum + corner.to_vec2()) / 4.0;
    let nearest = corners.iter().min_by(|a, b| (**a - pos).length().total_cmp(&(**b - pos).length())).copied().unwrap_or(pos);
    let through = handle.filter(|handle| (*handle - pos).length() <= HANDLE_REACH).unwrap_or(nearest);
    (through.to_vec2() - middle).angle()
}

/// Of a box whose `corners` are on screen in order round it, what a press at `pos` that is on
/// none of its handles does: just outside a corner it turns the box, and on a side it skews
/// along that side (clockwise from the top). `None` elsewhere. Unless `inside_skews`, only
/// the side's line and just outside it skew, so that a press anywhere inside a thin box moves.
pub(crate) fn turn_or_skew(corners: &[Pos2; 4], pos: Pos2, inside_skews: bool) -> Option<Handle> {
    // Inside, the pointer is on the same side of all four sides, whichever way round the box
    // is on screen.
    let crosses: Vec<f32> = (0..4)
        .map(|side| {
            let (a, b) = (corners[side], corners[(side + 1) % 4]);
            let (along, to) = (b - a, pos - a);
            along.x * to.y - along.y * to.x
        })
        .collect();
    let outside = !(crosses.iter().all(|c| *c >= 0.0) || crosses.iter().all(|c| *c <= 0.0));
    if outside && corners.iter().any(|&c| (c - pos).length() <= TURN_REACH) {
        return Some(Handle::Rotate);
    }
    // On a side's line itself, the pointer is inside as much as out.
    let on_a_line = crosses.iter().any(|c| c.abs() < 1e-3);
    if !(inside_skews || outside || on_a_line) {
        return None;
    }
    (0..4)
        .find(|side| {
            let (a, b) = (corners[*side], corners[(side + 1) % 4]);
            let along = b - a;
            let t = ((pos - a).dot(along) / along.length_sq().max(1e-6)).clamp(0.0, 1.0);
            (a + along * t - pos).length() <= SKEW_REACH
        })
        .map(Handle::Skew)
}

pub(crate) fn sides(corners: &[Pos2; 4]) -> [Pos2; 4] {
    [0, 1, 2, 3].map(|side| corners[side] + (corners[(side + 1) % 4] - corners[side]) / 2.0)
}

/// All of the drawing `layer` shows on `frame`, as a selection: what is painted on it, or on
/// its art layer `art` alone. `None` where nothing is.
fn whole(layer: &Layer, frame: u32, art: iskra_model::ArtReach) -> Option<Selection> {
    let drawing = &layer.drawing_at(frame)?.drawing;
    let taken = drawing.art_layers.iter().filter(|of| art.takes(of.kind));
    let rect = taken.fold(PixelRect::EMPTY, |around, of| around.union(&of.content.bounds()));
    (!rect.is_empty()).then(|| Selection::of(layer.id, frame, rect))
}

#[cfg(test)]
mod tests {
    use egui::pos2;

    use super::*;

    #[test]
    fn a_flick_is_told_from_an_outline_round_something() {
        let arc = |turn: f64, steps: usize| -> Vec<Point> {
            (0..=steps).map(|step| step as f64 / steps as f64 * turn).map(|angle| Point::new(100.0 * angle.cos(), 100.0 * angle.sin())).collect()
        };
        // Straight, and bent as a hand flicks, a quarter round: flicks.
        assert!(is_flick(&[Point::new(0.0, 0.0), Point::new(40.0, 5.0)]));
        assert!(is_flick(&arc(std::f64::consts::FRAC_PI_2, 12)));
        // Round something, closed or nearly, or a C round it: outlines.
        assert!(!is_flick(&arc(std::f64::consts::TAU, 24)));
        assert!(!is_flick(&arc(std::f64::consts::TAU * 0.85, 24)));
        assert!(!is_flick(&arc(std::f64::consts::PI * 1.5, 24)));
    }
    use crate::prefs::Guide;

    #[test]
    fn guides_are_laid_over_the_frame_wherever_its_corners_are() {
        // A frame of 200 by 100 from (100, 50), and the same frame turned a quarter.
        let frame = [pos2(100.0, 50.0), pos2(300.0, 50.0), pos2(300.0, 150.0), pos2(100.0, 150.0)];
        let safe = &guide_lines(Guide::ActionSafe, frame)[0];
        assert_eq!((safe[0], safe[2], safe.len()), (pos2(110.0, 55.0), pos2(290.0, 145.0), 5), "a closed line a twentieth in from each side");
        assert_eq!(guide_lines(Guide::TitleSafe, frame)[0][0], pos2(120.0, 60.0));
        let thirds = guide_lines(Guide::Thirds, frame);
        assert_eq!(thirds.len(), 4);
        assert!((thirds[0][0].x - 166.666).abs() < 0.01 && thirds[0][0].y == 50.0 && thirds[0][1].y == 150.0, "{:?}", thirds[0]);
        let cross = guide_lines(Guide::Centre, frame);
        assert_eq!((cross[0][0], cross[1][1]), (pos2(200.0, 95.0), pos2(210.0, 100.0)));
        let turned = [frame[1], frame[2], frame[3], frame[0]];
        assert_eq!(guide_lines(Guide::Centre, turned)[0][0], pos2(210.0, 100.0), "it turns with the frame");
        // Twelve fields about the middle, the twelfth the frame itself, and four lines through it.
        let fields = guide_lines(Guide::Fields, frame);
        assert_eq!(fields.len(), 16);
        assert_eq!((fields[11][0], fields[11][2]), (pos2(100.0, 50.0), pos2(300.0, 150.0)));
        assert!((fields[5][0].x - 150.0).abs() < 0.01 && (fields[5][0].y - 75.0).abs() < 0.01, "field 6 is half the frame: {:?}", fields[5][0]);
    }

    #[test]
    fn a_mask_dims_what_is_outside_its_shape_and_own_guides_are_frames_margins_and_grids() {
        use crate::prefs::GuideKind;
        // A frame of 200 by 100, which is 2:1.
        let frame = [pos2(100.0, 50.0), pos2(300.0, 150.0)];
        let frame = [frame[0], pos2(frame[1].x, frame[0].y), frame[1], pos2(frame[0].x, frame[1].y)];
        // Wider than the frame: a band above and one below. 4:1 is half as high.
        let wide = mask_bands(4.0, frame, 2.0);
        assert_eq!(
            wide,
            [
                [pos2(100.0, 50.0), pos2(300.0, 50.0), pos2(300.0, 75.0), pos2(100.0, 75.0)],
                [pos2(100.0, 125.0), pos2(300.0, 125.0), pos2(300.0, 150.0), pos2(100.0, 150.0)]
            ]
        );
        // Narrower: a band on each side. A square is half as wide.
        let square = mask_bands(1.0, frame, 2.0);
        assert_eq!((square.len(), square[0][1], square[1][0]), (2, pos2(150.0, 50.0), pos2(250.0, 50.0)));
        assert!(mask_bands(2.0, frame, 2.0).is_empty() && mask_bands(0.0, frame, 2.0).is_empty());

        let outline = &own_guide_lines(GuideKind::Aspect(1.0), frame, 2.0)[0];
        assert_eq!((outline[0], outline[2], outline.len()), (pos2(150.0, 50.0), pos2(250.0, 150.0), 5));
        assert_eq!(own_guide_lines(GuideKind::Margin(25.0), frame, 2.0)[0][0], pos2(150.0, 75.0));
        let grid = own_guide_lines(GuideKind::Grid { columns: 4, rows: 2 }, frame, 2.0);
        assert_eq!(grid.len(), 4, "three lines down and one across");
        assert_eq!((grid[0][0], grid[3][0], grid[3][1]), (pos2(150.0, 50.0), pos2(100.0, 100.0), pos2(300.0, 100.0)));
        assert!(own_guide_lines(GuideKind::Grid { columns: 0, rows: 1 }, frame, 2.0).is_empty());
    }
}
