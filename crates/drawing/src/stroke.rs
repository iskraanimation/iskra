//! Brush strokes on indexed art (plan §4.8).
//!
//! The brush engine (`photocraft-paint`) turns a stroke into a coverage value per pixel, with
//! all of its tip, dynamics, texture and smoothing. An indexed layer takes that coverage and
//! writes the stroke's swatch where it wins, so every brush works; what is lost is blending
//! between two colours inside one pixel.

use photocraft_geom::Rect;
use photocraft_paint::{BrushSettings, Stroke, StrokePoint, StrokeRenderer};

use crate::{IndexedSurface, PaintMode, SwatchId};

/// A stroke in progress on an indexed layer. Feed it points as they arrive; [`LiveStroke::layer`]
/// is the layer with the stroke so far, redone only where the new points changed it.
pub struct LiveStroke {
    renderer: StrokeRenderer,
    /// The brush it was started with, and the points it was given.
    brush: BrushSettings,
    points: Vec<StrokePoint>,
    opacity: f32,
    erase: bool,
    swatch: SwatchId,
    /// What is painted with these swatches is left as it is.
    protected: Vec<SwatchId>,
    /// Over what is painted, under it, or on it alone.
    mode: PaintMode,
    /// The layer as it was before the stroke.
    before: IndexedSurface,
    layer: IndexedSurface,
}

impl LiveStroke {
    /// Start a stroke of `brush` on `layer`. A brush set to erase lowers coverage; any other
    /// paints `swatch`. The brush's own colour is not used.
    pub fn new(layer: &IndexedSurface, brush: &BrushSettings, swatch: SwatchId) -> Self {
        Self {
            renderer: StrokeRenderer::new(brush, None, 1.0),
            brush: brush.clone(),
            points: Vec::new(),
            opacity: brush.opacity.clamp(0.0, 1.0),
            erase: brush.erase,
            swatch,
            protected: Vec::new(),
            mode: PaintMode::Over,
            before: layer.clone(),
            layer: layer.clone(),
        }
    }

    /// The stroke, leaving what is painted with the `protected` swatches as it is.
    pub fn protecting(self, protected: Vec<SwatchId>) -> Self {
        Self { protected, ..self }
    }

    /// The stroke laid in `mode`: over what is painted, under it, or on it alone.
    pub fn in_mode(self, mode: PaintMode) -> Self {
        Self { mode, ..self }
    }

    /// The layer with the stroke so far.
    pub fn layer(&self) -> &IndexedSurface {
        &self.layer
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
    pub fn before(&self) -> &IndexedSurface {
        &self.before
    }

    /// What the stroke leaves what is painted with.
    pub fn protected(&self) -> &[SwatchId] {
        &self.protected
    }

    /// The swatch it paints, and how it lays it.
    pub fn ink(&self) -> (SwatchId, PaintMode) {
        (self.swatch, self.mode)
    }

    /// Add input points, in the layer's pixels. Returns the rectangle that changed.
    pub fn push(&mut self, points: &[StrokePoint]) -> Rect {
        self.points.extend_from_slice(points);
        self.renderer.push(points);
        self.refresh()
    }

    /// End the stroke and return the finished layer.
    pub fn finish(mut self) -> IndexedSurface {
        self.renderer.finish();
        self.refresh();
        if self.erase {
            self.layer.prune();
        }
        self.layer
    }

    /// Redo the tiles the stroke's coverage changed since the last call: painting is not
    /// repeatable, so each is put back as it was before the stroke and the stroke's coverage
    /// so far is applied to it once.
    fn refresh(&mut self) -> Rect {
        let dirty = self.renderer.take_dirty_rect();
        let tiles = dirty.tiles().fold(Rect::EMPTY, |all, tile| all.union(&tile.rect()));
        let area = tiles.intersect(&self.renderer.bounds());
        if area.is_empty() {
            return Rect::EMPTY;
        }
        self.layer.copy_tiles_from(&self.before, tiles);
        let mut coverage = self.renderer.coverage_in(area);
        for c in &mut coverage {
            *c *= self.opacity;
        }
        if self.erase {
            self.layer.erase_protecting(area, &coverage, &self.protected);
        } else {
            self.layer.paint_as(self.mode, area, &coverage, self.swatch, &self.protected);
        }
        area
    }
}

/// Paint a whole stroke onto `layer`. The result depends only on the stroke: the same stroke
/// gives the same pixels however its points were fed to a [`LiveStroke`].
pub fn paint_stroke(layer: &mut IndexedSurface, stroke: &Stroke, swatch: SwatchId) {
    paint_stroke_protecting(layer, stroke, swatch, &[]);
}

/// [`paint_stroke`], leaving what is painted with the `protected` swatches as it is.
pub fn paint_stroke_protecting(layer: &mut IndexedSurface, stroke: &Stroke, swatch: SwatchId, protected: &[SwatchId]) {
    paint_stroke_as(layer, stroke, swatch, protected, PaintMode::Over);
}

/// [`paint_stroke_protecting`], the stroke laid in `mode`.
pub fn paint_stroke_as(layer: &mut IndexedSurface, stroke: &Stroke, swatch: SwatchId, protected: &[SwatchId], mode: PaintMode) {
    let mut live = LiveStroke::new(layer, &stroke.brush, swatch).protecting(protected.to_vec()).in_mode(mode);
    live.push(&stroke.points);
    *layer = live.finish();
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: SwatchId = SwatchId(1);

    fn brush(size: f32) -> BrushSettings {
        BrushSettings { size, ..Default::default() }
    }

    /// A horizontal stroke from x = 20 to x = 600 at y = 300, light at the start and heavy at
    /// the end.
    fn points() -> Vec<StrokePoint> {
        (0..=58).map(|i| StrokePoint::new(20.0 + 10.0 * f64::from(i), 300.0, 0.1 + 0.9 * i as f32 / 58.0)).collect()
    }

    /// How many pixels of the column at `x` the stroke covers.
    fn width_at(layer: &IndexedSurface, x: i32) -> usize {
        (200..400).filter(|&y| layer.pixel(x, y).coverage > 0).count()
    }

    #[test]
    fn a_stroke_behind_goes_under_and_a_repaint_only_over_what_is_painted() {
        let across = Stroke { brush: brush(20.0), points: points() };
        let down = Stroke { brush: brush(20.0), points: (0..=20).map(|i| StrokePoint::new(300.0, 200.0 + 10.0 * f64::from(i), 1.0)).collect() };
        let mut first = IndexedSurface::new();
        paint_stroke(&mut first, &across, LINE);
        let other = SwatchId(2);
        // Behind: where they cross the first stays in front; elsewhere the new one shows.
        let mut behind = first.clone();
        paint_stroke_as(&mut behind, &down, other, &[], PaintMode::Behind);
        assert_eq!((behind.pixel(300, 300).swatch, behind.pixel(300, 250).swatch), (LINE, other));
        // Repaint: the crossing takes the new swatch, as thick as it was; the canvas stays empty.
        let mut repainted = first.clone();
        paint_stroke_as(&mut repainted, &down, other, &[], PaintMode::Repaint);
        assert_eq!((repainted.pixel(300, 300).swatch, repainted.pixel(300, 300).coverage), (other, first.pixel(300, 300).coverage));
        assert_eq!((repainted.pixel(300, 250).coverage, repainted.pixel(100, 300).swatch), (0, LINE));
    }

    #[test]
    fn a_stroke_paints_its_swatch_and_pressure_makes_it_wider() {
        let mut layer = IndexedSurface::new();
        paint_stroke(&mut layer, &Stroke { brush: brush(30.0), points: points() }, LINE);
        assert_eq!(layer.pixel(300, 300).swatch, LINE);
        assert_eq!(layer.pixel(300, 300).coverage, 65535, "solid in the middle of the line");
        let (light, heavy) = (width_at(&layer, 60), width_at(&layer, 560));
        assert!(light >= 2 && heavy > light * 2, "pressure drives the width: {light} → {heavy}");
        assert!(heavy <= 32);
        let edge = (285..300).map(|y| layer.pixel(560, y).coverage).find(|&c| c > 0);
        assert!(edge.is_some_and(|c| c < 65535), "the edge is antialiased: {edge:?}");
    }

    #[test]
    fn feeding_points_one_at_a_time_gives_the_same_pixels() {
        let stroke = Stroke { brush: brush(24.0), points: points() };
        let mut whole = IndexedSurface::new();
        paint_stroke(&mut whole, &stroke, LINE);

        let mut live = LiveStroke::new(&IndexedSurface::new(), &stroke.brush, LINE);
        let mut changed = Rect::EMPTY;
        for point in &stroke.points {
            changed = changed.union(&live.push(std::slice::from_ref(point)));
        }
        assert!(changed.contains_rect(&live.layer().bounds()), "every painted pixel was reported as changed");
        assert_eq!(live.finish(), whole);
    }

    #[test]
    fn opacity_caps_the_coverage_even_where_the_stroke_overlaps_itself() {
        let mut layer = IndexedSurface::new();
        let mut points = points();
        points.extend(points.clone().into_iter().rev());
        paint_stroke(&mut layer, &Stroke { brush: BrushSettings { opacity: 0.5, pressure_size: false, ..brush(30.0) }, points }, LINE);
        let coverage = f32::from(layer.pixel(300, 300).coverage) / 65535.0;
        assert!((coverage - 0.5).abs() < 0.01, "{coverage}");
    }

    #[test]
    fn an_eraser_stroke_removes_paint_and_frees_emptied_tiles() {
        let mut layer = IndexedSurface::new();
        paint_stroke(&mut layer, &Stroke { brush: brush(10.0), points: points() }, LINE);
        assert!(layer.tiles().tile_count() > 0);
        let eraser = BrushSettings { erase: true, pressure_size: false, ..brush(60.0) };
        // A soft eraser pass at half opacity halves the coverage; the swatch stays.
        let mut half = layer.clone();
        paint_stroke(&mut half, &Stroke { brush: BrushSettings { opacity: 0.5, ..eraser.clone() }, points: points() }, SwatchId::NONE);
        assert_eq!(half.pixel(300, 300).swatch, LINE);
        assert!((f32::from(half.pixel(300, 300).coverage) / 65535.0 - 0.5).abs() < 0.01);
        paint_stroke(&mut layer, &Stroke { brush: eraser, points: points() }, SwatchId::NONE);
        assert!(layer.is_empty());
        assert_eq!(layer.tiles().tile_count(), 0);
    }
}
