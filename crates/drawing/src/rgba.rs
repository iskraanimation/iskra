//! Brush strokes on free-colour art (plan §4.8): an art layer of real RGBA pixels, for
//! painterly work. The brush engine paints it as it paints a picture, so what mixes colour
//! works here; what is painted does not follow the palette.

use photocraft_color::PixelFormat;
use photocraft_geom::Rect;
pub use photocraft_paint::mixer::{MixerSettings, MixerState};
use photocraft_paint::{BrushSettings, Stroke, StrokePoint, StrokeRenderer};
use photocraft_raster::Surface;

use crate::PaintMode;

/// `brush` with a swatch's colour (sRGB, straight alpha, as the palette has it) as its own.
pub fn brush_in(brush: &BrushSettings, color: [u8; 4]) -> BrushSettings {
    BrushSettings { color: color.map(|channel| f32::from(channel) / 255.0), ..brush.clone() }
}

/// A stroke in progress on a free-colour layer. Feed it points as they arrive;
/// [`LiveRgba::layer`] is the layer with the stroke so far.
pub struct LiveRgba {
    mode: PaintMode,
    /// Behind: the stroke alone, which the layer as it was goes over.
    alone: Option<Surface>,
    renderer: StrokeRenderer,
    /// The brush it was started with, and the points it was given.
    brush: BrushSettings,
    points: Vec<StrokePoint>,
    erase: bool,
    /// The layer as it was before the stroke.
    before: Surface,
    layer: Surface,
}

impl LiveRgba {
    /// Start a stroke of `brush`, in the brush's colour, on `layer`.
    pub fn new(layer: &Surface, brush: &BrushSettings) -> Self {
        Self::new_as(layer, brush, PaintMode::Over)
    }

    /// [`LiveRgba::new`] in `mode`: over what the layer has, under it, as on a layer below, or
    /// only on it, its alpha kept (an eraser erases in any mode).
    pub fn new_as(layer: &Surface, brush: &BrushSettings, mode: PaintMode) -> Self {
        let mode = if brush.erase { PaintMode::Over } else { mode };
        Self {
            mode,
            alone: (mode == PaintMode::Behind).then(|| Surface::new(PixelFormat::RGBA8)),
            renderer: StrokeRenderer::new(brush, Some(PixelFormat::RGBA8), 1.0),
            brush: brush.clone(),
            points: Vec::new(),
            erase: brush.erase,
            before: layer.clone(),
            layer: layer.clone(),
        }
    }

    /// The brush the stroke is painted with.
    pub fn brush(&self) -> &BrushSettings {
        &self.brush
    }

    /// The points the stroke was given, in order.
    pub fn points(&self) -> &[StrokePoint] {
        &self.points
    }

    /// The layer as it was before the stroke.
    pub fn before(&self) -> &Surface {
        &self.before
    }

    /// The layer with the stroke so far.
    pub fn layer(&self) -> &Surface {
        &self.layer
    }

    /// Add input points, in the layer's pixels. Returns the rectangle that changed.
    pub fn push(&mut self, points: &[StrokePoint]) -> Rect {
        self.points.extend_from_slice(points);
        self.renderer.push(points);
        self.composite(false)
    }

    fn composite(&mut self, all: bool) -> Rect {
        let Some(alone) = &mut self.alone else {
            return self.renderer.composite(&self.before, &mut self.layer, None, self.mode == PaintMode::Repaint, all);
        };
        let rect = self.renderer.composite(&Surface::new(PixelFormat::RGBA8), alone, None, false, all);
        if !rect.is_empty() {
            let (stroke, mut pixels) = (pixels_of(alone, rect), pixels_of(&self.before, rect));
            for (pixel, under) in pixels.iter_mut().zip(stroke) {
                *pixel = crate::over_straight(*pixel, under);
            }
            self.layer.write_interleaved(rect, pixels.as_flattened());
        }
        rect
    }

    /// End the stroke and return the finished layer.
    pub fn finish(mut self) -> Surface {
        self.renderer.finish();
        self.composite(true);
        if self.erase {
            forget_erased(&mut self.layer);
        }
        self.layer
    }
}

/// Make every pixel that is erased to nothing nothing at all, and free the tiles left empty:
/// a pixel with no alpha still has its colour, which would count as painted.
fn forget_erased(layer: &mut Surface) {
    let erased = |tile: &photocraft_raster::Tile| tile.bytes().as_chunks::<4>().0.iter().any(|px| px[3] == 0 && px[..3] != [0; 3]);
    let with: Vec<_> = layer.tiles().filter(|(_, tile)| erased(tile)).map(|(coord, _)| *coord).collect();
    for coord in with {
        for px in layer.tile_mut(coord).bytes_mut().as_chunks_mut::<4>().0.iter_mut().filter(|px| px[3] == 0) {
            *px = [0; 4];
        }
    }
    layer.prune();
}

/// Paint `color` (sRGB, straight alpha) over a free-colour `layer` with a `coverage` from 0
/// to 1 for each pixel of `rect`, in rows: the inside of a shape. Tiles the coverage does
/// not touch are left as they are.
pub fn fill_rgba(layer: &mut Surface, rect: Rect, coverage: &[f32], color: [u8; 4]) {
    let width = rect.width() as usize;
    assert_eq!(coverage.len(), width * rect.height() as usize, "one coverage value per pixel of the rect");
    for coord in rect.tiles() {
        let origin = coord.rect();
        let part = rect.intersect(&origin);
        let row = |y: i32| {
            let start = (y - rect.y0) as usize * width + (part.x0 - rect.x0) as usize;
            &coverage[start..start + part.width() as usize]
        };
        if !(part.y0..part.y1).any(|y| row(y).iter().any(|&c| c > 0.0)) {
            continue;
        }
        let pixels = layer.tile_mut(coord).bytes_mut().as_chunks_mut::<4>().0;
        for y in part.y0..part.y1 {
            let start = ((y - origin.y0) * photocraft_geom::TILE_SIZE + (part.x0 - origin.x0)) as usize;
            for (&c, px) in row(y).iter().zip(&mut pixels[start..]) {
                let over = [color[0], color[1], color[2], (f32::from(color[3]) * c.clamp(0.0, 1.0) + 0.5) as u8];
                *px = crate::over_straight(over, *px);
            }
        }
    }
}

/// Paint a whole stroke, in its brush's colour, onto a free-colour `layer`. The result depends
/// only on the stroke.
pub fn paint_rgba(layer: &mut Surface, stroke: &Stroke) {
    paint_rgba_as(layer, stroke, PaintMode::Over);
}

/// [`paint_rgba`] in `mode`, as [`LiveRgba::new_as`] paints it.
pub fn paint_rgba_as(layer: &mut Surface, stroke: &Stroke, mode: PaintMode) {
    if stroke.points.is_empty() {
        return;
    }
    let mut live = LiveRgba::new_as(layer, &stroke.brush, mode);
    live.push(&stroke.points);
    *layer = live.finish();
}

/// The pixels of `rect` of a free-colour layer, in rows.
fn pixels_of(layer: &Surface, rect: Rect) -> Vec<[u8; 4]> {
    let mut pixels = vec![[0u8; 4]; rect.width() as usize * rect.height() as usize];
    layer.read_rgba8_into(rect, &mut pixels);
    pixels
}

/// A free-colour layer of the `pixels` of `rect`, in rows, and nothing else.
fn layer_of(rect: Rect, pixels: &[[u8; 4]]) -> Surface {
    let mut layer = Surface::from_interleaved(PixelFormat::RGBA8, rect, pixels.as_flattened());
    layer.prune();
    layer
}

/// Whether a selection takes the pixel at (`x`, `y`) of free colour, which has no swatches:
/// one that is of a swatch's paint takes none of it, and protection does not reach it.
fn taken(picked: &crate::Picked, x: i32, y: i32) -> bool {
    picked.swatch.is_none() && picked.rect.contains(x, y) && crate::LassoPart::inside(picked.parts, x, y)
}

/// What a selection takes of a free-colour layer, as a layer of its own.
pub fn extract_rgba(layer: &Surface, picked: &crate::Picked) -> Surface {
    let rect = picked.rect.intersect(&layer.content_bounds());
    if rect.is_empty() || picked.swatch.is_some() {
        return Surface::new(PixelFormat::RGBA8);
    }
    let mut pixels = pixels_of(layer, rect);
    let places = (rect.y0..rect.y1).flat_map(|y| (rect.x0..rect.x1).map(move |x| (x, y)));
    for (pixel, (x, y)) in pixels.iter_mut().zip(places) {
        if !taken(picked, x, y) {
            *pixel = [0; 4];
        }
    }
    layer_of(rect, &pixels)
}

/// Erase what a selection takes of a free-colour layer.
pub fn clear_rgba(layer: &mut Surface, picked: &crate::Picked) {
    let rect = picked.rect.intersect(&layer.content_bounds());
    if rect.is_empty() || picked.swatch.is_some() {
        return;
    }
    let mut pixels = pixels_of(layer, rect);
    let places = (rect.y0..rect.y1).flat_map(|y| (rect.x0..rect.x1).map(move |x| (x, y)));
    for (pixel, (x, y)) in pixels.iter_mut().zip(places) {
        if taken(picked, x, y) {
            *pixel = [0; 4];
        }
    }
    layer.write_interleaved(rect, pixels.as_flattened());
    layer.prune();
}

/// Paint what `marks` covers over a free-colour layer in the colours of `shade` (plan §4.20),
/// as much as it covers each pixel: a stroke of a gradient or a picture, fitted to all it
/// covers, in its colours from then on.
pub fn paint_rgba_shaded(layer: &mut Surface, marks: &crate::IndexedSurface, shade: &crate::Shade, linear: bool) {
    let rect = marks.bounds();
    if rect.is_empty() {
        return;
    }
    let covered = marks.pixels(rect);
    let mut under = pixels_of(layer, rect);
    for (i, (under, pixel)) in under.iter_mut().zip(&covered).enumerate().filter(|(_, (_, pixel))| pixel.coverage > 0) {
        let (x, y) = (rect.x0 + (i % rect.width() as usize) as i32, rect.y0 + (i / rect.width() as usize) as i32);
        let [r, g, b, a] = shade.in_area(x, y, rect, linear);
        let over = [r, g, b, (f32::from(a) * f32::from(pixel.coverage) / 65535.0 + 0.5) as u8];
        *under = crate::over_straight(over, *under);
    }
    layer.write_interleaved(rect, under.as_flattened());
    layer.prune();
}

/// Paint `other` over a free-colour layer, each pixel in its own colour (pasting a selection).
pub fn stamp_rgba(layer: &mut Surface, other: &Surface) {
    let rect = other.content_bounds();
    if rect.is_empty() {
        return;
    }
    let (over, mut under) = (pixels_of(other, rect), pixels_of(layer, rect));
    for (under, over) in under.iter_mut().zip(&over) {
        *under = crate::over_straight(*over, *under);
    }
    layer.write_interleaved(rect, under.as_flattened());
}

/// A free-colour layer moved, scaled or turned by `transform`: each new pixel is read back
/// from where it came from, between the four pixels around that place. A whole-pixel move
/// is exact.
pub fn transformed_rgba(layer: &Surface, transform: &photocraft_geom::Affine) -> Surface {
    let empty = Surface::new(PixelFormat::RGBA8);
    let (bounds, Some(inverse)) = (layer.content_bounds(), transform.inverse()) else { return empty };
    if bounds.is_empty() {
        return empty;
    }
    let corners = [(bounds.x0, bounds.y0), (bounds.x1, bounds.y0), (bounds.x1, bounds.y1), (bounds.x0, bounds.y1)]
        .map(|(x, y)| transform.apply(photocraft_geom::Point::new(f64::from(x), f64::from(y))));
    let (xs, ys) = (corners.map(|p| p.x), corners.map(|p| p.y));
    let low = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min).floor() - 1.0;
    let high = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max).ceil() + 1.0;
    let sides = [low(xs), low(ys), high(xs), high(ys)];
    // Too far away or too large to be a picture: nothing lands.
    if sides.iter().any(|side| side.abs() > f64::from(i32::MAX / 4)) || (sides[2] - sides[0]) * (sides[3] - sides[1]) > f64::from(1u32 << 28) {
        return empty;
    }
    let rect = Rect::new(sides[0] as i32, sides[1] as i32, sides[2] as i32, sides[3] as i32);
    // The pixels it is read from, with a rim of nothing around them, and colour times alpha:
    // what is between a painted pixel and an empty one is then fainter, not darker.
    let from = Rect::new(bounds.x0 - 1, bounds.y0 - 1, bounds.x1 + 1, bounds.y1 + 1);
    let width = from.width() as usize;
    let source: Vec<[f32; 4]> = pixels_of(layer, from)
        .iter()
        .map(|[r, g, b, a]| {
            let alpha = f32::from(*a) / 255.0;
            [f32::from(*r) * alpha, f32::from(*g) * alpha, f32::from(*b) * alpha, alpha]
        })
        .collect();
    let at = |x: i64, y: i64| {
        let inside = (i64::from(from.x0)..i64::from(from.x1)).contains(&x) && (i64::from(from.y0)..i64::from(from.y1)).contains(&y);
        if inside { source[(y - i64::from(from.y0)) as usize * width + (x - i64::from(from.x0)) as usize] } else { [0.0; 4] }
    };
    let mut pixels = Vec::with_capacity(rect.width() as usize * rect.height() as usize);
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            // The middle of the new pixel, taken back; then as a place between pixel middles.
            let back = inverse.apply(photocraft_geom::Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5));
            let (sx, sy) = (back.x - 0.5, back.y - 0.5);
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = ((sx - x0) as f32, (sy - y0) as f32);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let mut mixed = [0.0f32; 4];
            for (dx, dy, weight) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)), (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
                let pixel = at(x0 + dx, y0 + dy);
                for (sum, part) in mixed.iter_mut().zip(pixel) {
                    *sum += part * weight;
                }
            }
            // A pixel with no alpha to speak of is nothing at all: its colour would count as painted.
            let (alpha, opacity) = (mixed[3], (mixed[3] * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
            let channel = |value: f32| (value / alpha + 0.5).clamp(0.0, 255.0) as u8;
            pixels.push(if opacity == 0 { [0; 4] } else { [channel(mixed[0]), channel(mixed[1]), channel(mixed[2]), opacity] });
        }
    }
    layer_of(rect, &pixels)
}

/// Paint a whole stroke of the mixer brush onto a free-colour `layer` (plan §4.8): the brush
/// lays down the paint it is loaded with, picks up what is on the layer under it, and mixes
/// the two as its settings say. `carried` is what the brush carries, the paint it is loaded
/// with and what it has picked up: as it was before the stroke going in, and as the stroke
/// leaves it coming out. The result depends only on the stroke, the settings and that.
/// Returns the rectangle that changed.
pub fn mix_rgba(layer: &mut Surface, stroke: &Stroke, settings: &MixerSettings, carried: &mut MixerState) -> Rect {
    if stroke.points.is_empty() {
        return Rect::EMPTY;
    }
    photocraft_paint::mixer::apply_mixer_stroke(layer, None, stroke, settings, carried, None, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_lifts_clears_moves_and_stamps_free_colour() {
        let mut layer = Surface::new(PixelFormat::RGBA8);
        fill_rgba(&mut layer, Rect::new(0, 0, 20, 10), &[1.0; 200], [220, 50, 47, 255]);
        let left = crate::LassoPart { lasso: std::sync::Arc::new(crate::Lasso::of_rect(Rect::new(0, 0, 10, 10)).unwrap()), subtract: false };
        let picked = crate::Picked { rect: Rect::new(0, 0, 20, 10), parts: std::slice::from_ref(&left), swatch: None, protected: &[] };
        let lifted = extract_rgba(&layer, &picked);
        assert_eq!((lifted.content_bounds(), pixel(&lifted, 5, 5), pixel(&lifted, 15, 5)), (Rect::new(0, 0, 10, 10), [220, 50, 47, 255], [0, 0, 0, 0]));
        // Of a swatch's paint there is none on free colour.
        let of_swatch = crate::Picked { swatch: Some(crate::SwatchId(3)), ..picked };
        assert!(extract_rgba(&layer, &of_swatch).content_bounds().is_empty());
        clear_rgba(&mut layer, &of_swatch);
        assert_eq!(layer.content_bounds(), Rect::new(0, 0, 20, 10));

        // Moved by whole pixels it is as it was, there; turned a quarter it stands upright.
        let moved = transformed_rgba(&lifted, &photocraft_geom::Affine::translate(100.0, 30.0));
        assert_eq!((moved.content_bounds(), pixel(&moved, 105, 35)), (Rect::new(100, 30, 110, 40), [220, 50, 47, 255]));
        let wide = extract_rgba(&layer, &crate::Picked::all(Rect::new(0, 0, 20, 10)));
        let turned = transformed_rgba(&wide, &photocraft_geom::Affine::rotate(std::f64::consts::FRAC_PI_2));
        assert_eq!((turned.content_bounds().width(), turned.content_bounds().height(), pixel(&turned, -5, 10)), (10, 20, [220, 50, 47, 255]));
        // Scaled to twice its size, its edge is where it should be and its colour is not darkened.
        let twice = transformed_rgba(&lifted, &photocraft_geom::Affine { m: [2.0, 0.0, 0.0, 2.0, 0.0, 0.0] });
        assert_eq!((pixel(&twice, 10, 10), pixel(&twice, 19, 19)[..3].to_vec(), pixel(&twice, 21, 10)), ([220, 50, 47, 255], vec![220, 50, 47], [0, 0, 0, 0]));

        // Cleared where it was lifted, and stamped where it was moved to, over what is there.
        clear_rgba(&mut layer, &picked);
        assert_eq!((layer.content_bounds(), pixel(&layer, 5, 5), pixel(&layer, 15, 5)), (Rect::new(10, 0, 20, 10), [0, 0, 0, 0], [220, 50, 47, 255]));
        stamp_rgba(&mut layer, &moved);
        assert_eq!((pixel(&layer, 105, 35), pixel(&layer, 15, 5), layer.content_bounds()), ([220, 50, 47, 255], [220, 50, 47, 255], Rect::new(10, 0, 110, 40)));
    }

    #[test]
    fn the_mixer_brush_lays_its_paint_picks_up_what_is_there_and_carries_it_on() {
        let brush = BrushSettings { size: 20.0, pressure_size: false, ..Default::default() };
        let across = |y: f64| Stroke { brush: brush.clone(), points: (0..=20).map(|i| StrokePoint::new(100.0 + 10.0 * f64::from(i), y, 1.0)).collect() };
        // Dry and loaded with red, it paints red, and what it carries is as it was.
        let dry = MixerSettings { wet: 0.0, load: 1.0, mix: 0.0, flow: 1.0, ..Default::default() };
        let mut carried = MixerState::default();
        carried.load([1.0, 0.0, 0.0, 1.0]);
        let mut layer = Surface::new(PixelFormat::RGBA8);
        let changed = mix_rgba(&mut layer, &across(100.0), &dry, &mut carried);
        assert_eq!((pixel(&layer, 200, 100), pixel(&layer, 200, 130)), ([255, 0, 0, 255], [0, 0, 0, 0]));
        assert!(changed.contains(200, 100) && carried.reservoir == Some([1.0, 0.0, 0.0, 1.0]) && carried.pickup.is_none());

        // Wet and loaded with blue, drawn down through the red line: it picks the red up, so
        // under the line what it lays is neither, and it goes on carrying red.
        let wet = MixerSettings { wet: 1.0, load: 1.0, mix: 1.0, flow: 1.0, ..Default::default() };
        carried.load([0.0, 0.0, 1.0, 1.0]);
        let down = Stroke { brush: brush.clone(), points: (0..=20).map(|i| StrokePoint::new(200.0, 60.0 + 5.0 * f64::from(i), 1.0)).collect() };
        mix_rgba(&mut layer, &down, &wet, &mut carried);
        let (picked, below) = (carried.pickup.expect("it picked colour up"), pixel(&layer, 200, 150));
        assert!(picked[0] > 0.05 && below[0] > 10 && below[3] > 0, "red was carried below the line: {picked:?}, {below:?}");
        // The same stroke with the same paint on the brush is the same picture: a command of it can be run again.
        let (mut again, mut first) = (Surface::new(PixelFormat::RGBA8), Surface::new(PixelFormat::RGBA8));
        let (mut one, mut two) = (carried.clone(), carried.clone());
        mix_rgba(&mut first, &across(300.0), &wet, &mut one);
        mix_rgba(&mut again, &across(300.0), &wet, &mut two);
        assert_eq!((pixel(&first, 200, 300), one), (pixel(&again, 200, 300), two));
        assert_eq!(mix_rgba(&mut again, &Stroke { brush, points: Vec::new() }, &wet, &mut carried), Rect::EMPTY);
    }

    fn pixel(layer: &Surface, x: i32, y: i32) -> [u8; 4] {
        let mut out = [[0u8; 4]];
        layer.read_rgba8_into(Rect::new(x, y, x + 1, y + 1), &mut out);
        out[0]
    }

    fn line(brush: BrushSettings) -> Stroke {
        Stroke { brush, points: (0..=20).map(|i| StrokePoint::new(100.0 + 10.0 * f64::from(i), 100.0, 1.0)).collect() }
    }

    #[test]
    fn a_stroke_paints_its_colour_and_the_eraser_takes_it_away() {
        let mut layer = Surface::new(PixelFormat::RGBA8);
        let brush = BrushSettings { size: 20.0, pressure_size: false, ..Default::default() };
        paint_rgba(&mut layer, &line(brush_in(&brush, [220, 50, 47, 255])));
        assert_eq!(pixel(&layer, 200, 100), [220, 50, 47, 255]);
        assert_eq!(pixel(&layer, 200, 130), [0, 0, 0, 0]);
        // Another colour over half of it: real colour, mixed where the edge is soft.
        let over = Stroke { brush: brush_in(&brush, [0, 0, 255, 255]), points: vec![StrokePoint::new(100.0, 100.0, 1.0), StrokePoint::new(200.0, 100.0, 1.0)] };
        paint_rgba(&mut layer, &over);
        assert_eq!((pixel(&layer, 150, 100), pixel(&layer, 280, 100)), ([0, 0, 255, 255], [220, 50, 47, 255]));

        paint_rgba(&mut layer, &line(BrushSettings { size: 60.0, erase: true, ..brush }));
        assert!(layer.content_bounds().is_empty() && layer.tile_count() == 0, "erased, and its tiles freed");
    }

    #[test]
    fn a_stroke_behind_goes_under_what_is_painted_and_a_repaint_only_over_it_keeping_its_alpha() {
        let brush = BrushSettings { size: 20.0, pressure_size: false, ..Default::default() };
        let mut first = Surface::new(PixelFormat::RGBA8);
        paint_rgba(&mut first, &line(brush_in(&brush, [220, 50, 47, 255])));
        let edge = pixel(&first, 200, 109);
        assert!(edge[3] > 0 && edge[3] < 255, "the line's edge is soft: {edge:?}");
        let down =
            Stroke { brush: brush_in(&brush, [0, 0, 255, 255]), points: (0..=20).map(|i| StrokePoint::new(200.0, 50.0 + 5.0 * f64::from(i), 1.0)).collect() };

        let mut behind = first.clone();
        paint_rgba_as(&mut behind, &down, PaintMode::Behind);
        assert_eq!((pixel(&behind, 200, 100), pixel(&behind, 200, 140)), ([220, 50, 47, 255], [0, 0, 255, 255]));
        let under = pixel(&behind, 200, 109);
        assert!(under[3] == 255 && under[0] < 220 && under[2] > 0, "the soft edge shows the blue under it: {under:?}");

        let mut repainted = first.clone();
        paint_rgba_as(&mut repainted, &down, PaintMode::Repaint);
        assert_eq!((pixel(&repainted, 200, 100), pixel(&repainted, 200, 140), pixel(&repainted, 100, 100)), ([0, 0, 255, 255], [0; 4], [220, 50, 47, 255]));
        assert_eq!(pixel(&repainted, 200, 109)[3], edge[3], "the edge keeps its alpha");
        // A soft brush's edge blends what it repaints partly to its colour.
        let soft = Stroke { brush: brush_in(&BrushSettings { size: 40.0, hardness: 0.0, ..brush }, [0, 0, 255, 255]), points: down.points };
        let mut partly = first.clone();
        paint_rgba_as(&mut partly, &soft, PaintMode::Repaint);
        let blended = pixel(&partly, 215, 100);
        assert!(blended[3] == 255 && blended[0] > 0 && blended[0] < 220 && blended[2] > 0, "partly blue: {blended:?}");
    }

    #[test]
    fn a_stroke_fed_in_parts_is_the_stroke_painted_whole() {
        let brush = brush_in(&BrushSettings { size: 14.0, hardness: 0.5, ..Default::default() }, [10, 120, 60, 255]);
        let stroke = line(brush.clone());
        let mut whole = Surface::new(PixelFormat::RGBA8);
        paint_rgba(&mut whole, &stroke);
        let mut live = LiveRgba::new(&Surface::new(PixelFormat::RGBA8), &brush);
        for part in stroke.points.chunks(3) {
            live.push(part);
            assert!(!live.layer().content_bounds().is_empty(), "seen as it is drawn");
        }
        assert!(live.finish() == whole);
    }
}
