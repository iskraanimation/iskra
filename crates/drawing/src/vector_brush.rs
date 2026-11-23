//! Vector strokes drawn with the pen (plan §4.16, step 2): the pen's samples smoothed, fitted
//! with Béziers, and the pressure kept as the stroke's width at the fitted points.

use std::collections::HashMap;

use kurbo::{BezPath, ParamCurve, ParamCurveArclen, PathSeg, Point};
use photocraft_geom::{TILE_SIZE, TileCoord};
use photocraft_paint::StrokePoint;
use serde::{Deserialize, Serialize};
use vectorcraft_geom::{FillRule, PathData};
use vectorcraft_pathops::{BoolOp, SimplifyOptions};

use crate::stamp::{Laid, Place, Stamper};
use crate::vector::Rasterizer;
use crate::{IndexedSurface, Nib, Rect, StampBrush, SwatchId, VectorStroke};

/// What a vector stroke is drawn with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VectorBrush {
    /// The width at full pressure, in the drawing's pixels.
    pub size: f32,
    /// The width follows the pressure.
    pub pressure_size: bool,
    /// The width at no pressure, as a part of the size (0..1).
    pub thinnest: f32,
    /// How the width answers the pressure: the pressure going in, from 0 to 1, and how far
    /// from the thinnest to the full width it comes out, through the smooth line by these
    /// points in order. None is the straight line, the width as the pressure.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pressure_curve: Vec<[f64; 2]>,
    /// How much the pen's path is smoothed before it is fitted (0..1).
    pub smoothing: f32,
    /// How far the fitted line may stray from the pen's path, in the drawing's pixels: more
    /// gives fewer points.
    pub simplify: f32,
    /// How long the stroke takes to grow to its width from its start, and to narrow to a point
    /// at its end, in multiples of the size. 0 is a round end.
    pub taper_start: f32,
    pub taper_end: f32,
    /// The opacity follows the pressure (the user's of 2026-11-04): a lighter touch is fainter,
    /// through the same pressure curve as the width.
    pub pressure_opacity: bool,
    /// The opacity at no pressure (0..1).
    pub faintest: f32,
    /// How long the stroke takes to come up to its opacity from its start, and to fade out at
    /// its end, in multiples of the size. 0 is none.
    pub fade_start: f32,
    pub fade_end: f32,
    /// How far its strokes' colour drifts towards the swatch's drift colour (0..1), at the
    /// hardest press where it follows the pressure, else all along (the user's of 2026-11-04).
    pub drift: f32,
    /// The drift follows the pressure, through the same pressure curve: the harder the pen
    /// presses, the further the colour drifts.
    pub pressure_drift: bool,
    /// How far its strokes' edges move as they boil, in the drawing's pixels (the user's of
    /// 2026-11-04): drawn anew every `boil_every` frames, three ways in turn. 0 is still.
    pub boil: f32,
    pub boil_every: u32,
    /// The nib's angle in degrees, and how round it is (1 round, less an oval: a calligraphic
    /// pen, thick or thin as the stroke goes across or along its angle).
    pub angle: f32,
    pub roundness: f32,
    /// A paper texture over the stroke, as the pixel brush's.
    pub texture: photocraft_paint::Texture,
    /// A second tip along the stroke, which cuts it, as the pixel brush's.
    pub dual_brush: photocraft_paint::DualBrush,
    /// What chance does to the second tip: each stroke has its own.
    #[serde(skip_serializing_if = "is_zero")]
    pub seed: u64,
    /// A picture's tip stamped along the line, which draws it in place of its outline (the
    /// user's of 2026-11-23, as Moho's brushes do). With it on, the texture and the second tip
    /// are not used.
    #[serde(skip_serializing_if = "StampBrush::is_off")]
    pub stamp: StampBrush,
}

impl StampBrush {
    fn is_off(&self) -> bool {
        !self.enabled && *self == StampBrush { enabled: false, ..StampBrush::default() }
    }
}

fn is_zero(seed: &u64) -> bool {
    *seed == 0
}

impl Default for VectorBrush {
    /// A round pen whose width follows the pressure.
    fn default() -> Self {
        Self {
            size: 8.0,
            pressure_size: true,
            thinnest: 0.1,
            pressure_curve: Vec::new(),
            smoothing: 0.3,
            simplify: 0.5,
            taper_start: 0.0,
            taper_end: 0.0,
            pressure_opacity: false,
            faintest: 0.0,
            fade_start: 0.0,
            fade_end: 0.0,
            drift: 0.0,
            pressure_drift: true,
            boil: 0.0,
            boil_every: 2,
            angle: 0.0,
            roundness: 1.0,
            texture: Default::default(),
            dual_brush: Default::default(),
            seed: 0,
            stamp: StampBrush::default(),
        }
    }
}

impl VectorBrush {
    /// What of the brush's texture and second tip its strokes keep, if either is on.
    /// None where it stamps a tip, which takes their place.
    pub fn grain(&self) -> Option<Box<crate::Grain>> {
        (!self.stamp.stamps() && (self.texture.enabled || self.dual_brush.enabled))
            .then(|| Box::new(crate::Grain { texture: self.texture.clone(), dual_brush: self.dual_brush.clone(), seed: self.seed }))
    }

    /// The tip its strokes are stamped with, with this brush's chance, if it stamps one.
    pub fn stamping(&self) -> Option<Box<crate::Stamp>> {
        self.stamp.stamp(self.seed)
    }

    /// The nib the brush's strokes are drawn with.
    pub fn nib(&self) -> Nib {
        Nib { angle: f64::from(self.angle), roundness: f64::from(self.roundness.clamp(0.01, 1.0)) }
    }

    /// How much of its width a stroke has `from` its start, a length of `length` in all.
    fn tapered(&self, from: f64, length: f64) -> f64 {
        self.ramped([self.taper_start, self.taper_end], from, length)
    }

    /// How much of its opacity a stroke has `from` its start, a length of `length` in all.
    fn faded(&self, from: f64, length: f64) -> f64 {
        self.ramped([self.fade_start, self.fade_end], from, length)
    }

    /// Up from 0 over the first of `lengths` (in sizes) and down to 0 over the second, `from`
    /// the start of a stroke `length` long.
    fn ramped(&self, lengths: [f32; 2], from: f64, length: f64) -> f64 {
        let size = f64::from(self.size.max(0.0));
        let ramp = |over: f32, distance: f64| {
            let over = f64::from(over.max(0.0)) * size;
            if over <= 0.0 { 1.0 } else { (distance / over).clamp(0.0, 1.0) }
        };
        ramp(lengths[0], from) * ramp(lengths[1], length - from)
    }

    /// Whether its strokes' opacity changes along them: with the pressure, or fading at an end.
    pub fn toned(&self) -> bool {
        (self.pressure_opacity && self.faintest < 1.0) || self.fade_start > 0.0 || self.fade_end > 0.0
    }

    /// How its strokes boil: `None` where they are still.
    pub fn boiling(&self) -> Option<crate::Boil> {
        (self.boil > 0.0).then(|| crate::Boil { amount: f64::from(self.boil), every: self.boil_every.clamp(1, 12) })
    }

    /// Whether its strokes' colour drifts.
    pub fn drifts(&self) -> bool {
        self.drift > 0.0
    }

    /// How far the colour drifts at `pressure` (0..1).
    pub fn drift_at(&self, pressure: f32) -> f64 {
        let most = f64::from(self.drift.clamp(0.0, 1.0));
        if self.pressure_drift { most * self.felt(pressure) } else { most }
    }

    /// The opacity at `pressure` (0..1), before the fades at the ends.
    pub fn opacity(&self, pressure: f32) -> f64 {
        if !self.pressure_opacity {
            return 1.0;
        }
        let faintest = f64::from(self.faintest.clamp(0.0, 1.0));
        faintest + (1.0 - faintest) * self.felt(pressure)
    }

    /// How hard the pen is felt to press at `pressure` (0..1), through the pressure curve.
    fn felt(&self, pressure: f32) -> f64 {
        let pressure = f64::from(pressure.clamp(0.0, 1.0));
        // A curve whose points are not numbers in order is taken as the straight line.
        let curve = &self.pressure_curve;
        let usable = curve.iter().flatten().all(|part| part.is_finite()) && curve.windows(2).all(|pair| pair[0][0] < pair[1][0]);
        if usable { crate::monotone_through(curve, pressure).clamp(0.0, 1.0) } else { pressure }
    }

    /// The width at `pressure` (0..1).
    pub fn width(&self, pressure: f32) -> f64 {
        let size = f64::from(self.size.max(0.0));
        if !self.pressure_size {
            return size;
        }
        let thinnest = f64::from(self.thinnest.clamp(0.0, 1.0));
        size * (thinnest + (1.0 - thinnest) * self.felt(pressure))
    }
}

/// `stroke` as if drawn with `brush`, along the same line: its thick and thin, as the pen
/// pressed, kept and scaled so that its thickest is the brush's size, held above the brush's
/// thinnest where the width follows the pressure, and even where it does not; the brush's
/// tapers at its ends, its nib and its grain. Its caps stay.
pub fn rebrushed(stroke: &VectorStroke, brush: &VectorBrush) -> VectorStroke {
    let thickest = stroke.widths.iter().copied().fold(0.0_f64, f64::max);
    // How far along the line each point is, the points being the ends of its elements.
    let mut along = Vec::with_capacity(stroke.widths.len());
    let (mut length, mut last) = (0.0, None::<Point>);
    for element in stroke.path.elements() {
        let segment = match (*element, last) {
            (kurbo::PathEl::LineTo(to), Some(from)) => Some(PathSeg::Line(kurbo::Line::new(from, to))),
            (kurbo::PathEl::QuadTo(c, to), Some(from)) => Some(PathSeg::Quad(kurbo::QuadBez::new(from, c, to))),
            (kurbo::PathEl::CurveTo(c1, c2, to), Some(from)) => Some(PathSeg::Cubic(kurbo::CubicBez::new(from, c1, c2, to))),
            _ => None,
        };
        length += segment.map_or(0.0, |segment| segment.arclen(0.1));
        if let Some(end) = element.end_point() {
            along.push(length);
            last = Some(end);
        }
    }
    let pressed = |width: &f64| if thickest > 0.0 { (width / thickest) as f32 } else { 1.0 };
    let widths = stroke.widths.iter().zip(&along).map(|(width, from)| brush.width(pressed(width)) * brush.tapered(*from, length)).collect();
    // Its opacity as the brush gives it, from the same pressure.
    let opacities = match brush.toned() {
        true => stroke.widths.iter().zip(&along).map(|(width, from)| brush.opacity(pressed(width)) * brush.faded(*from, length)).collect(),
        false => Vec::new(),
    };
    let drifts = match brush.drifts() {
        true => stroke.widths.iter().map(|width| brush.drift_at(pressed(width))).collect(),
        false => Vec::new(),
    };
    // Each line its own chance, from where it lies.
    let stamp = brush.stamp.stamp(brush.seed ^ (length.to_bits().wrapping_mul(0x9E37_79B9_7F4A_7C15)));
    VectorStroke { widths, opacities, drifts, nib: brush.nib(), grain: brush.grain(), boil: brush.boiling(), stamp, ..stroke.clone() }
}

/// Anchors where the pen turns more sharply than this stay corners. Smaller turns, which a
/// hand's tremor makes between two samples, are smoothed over.
const CORNER_DEGREES: f64 = 70.0;

/// The surface a brush set to leave surfaces makes of `stroke` (the user's of 2026-11-04): the
/// area it covers as one plain outline, with no middle line, fitted with Béziers that stray
/// from it by no more than `brush`'s `simplify` in pixels. Where the stroke crosses itself it is
/// one area. Plain, as Moho's Blob Brush leaves it (the user's of 2026-11-23): no rough edge,
/// strands, holes or paper texture, which made it slow and looked cut out.
pub fn surface(stroke: &VectorStroke, brush: &VectorBrush) -> BezPath {
    let contour = PathData::from_bezpath(&stroke.smooth_contour());
    // Its union with itself takes the crossings out, leaving the edge alone.
    let whole = vectorcraft_pathops::boolean(&contour, FillRule::NonZero, &contour, FillRule::NonZero, BoolOp::Union);
    let options = SimplifyOptions { tolerance: f64::from(brush.simplify).max(0.05), corner_angle_deg: CORNER_DEGREES, straight_lines: false };
    vectorcraft_pathops::simplify_with(&whole, &options).to_bezpath()
}

/// The shape a brush set to leave surfaces leaves of `stroke`: a fill of its [`surface`], which
/// keeps the stroke as its track where its tone changes along it, so it fades and drifts as the
/// line would, but never its grain, and boils as it does.
pub fn surface_shape(stroke: &VectorStroke, brush: &VectorBrush) -> crate::ShapeKind {
    let tracks = if stroke.toned() { vec![VectorStroke { grain: None, stamp: None, ..stroke.clone() }] } else { Vec::new() };
    crate::ShapeKind::Fill(crate::VectorFill { tracks, boil: stroke.boil, ..crate::VectorFill::plain(surface(stroke, brush)) })
}

/// The stroke the pen drew along `points` with `brush`, or `None` if it drew nothing.
pub fn vector_stroke(points: &[StrokePoint], brush: &VectorBrush) -> Option<VectorStroke> {
    if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return None;
    }
    let smoothed = photocraft_paint::smooth(points, brush.smoothing);
    // Samples closer than a tenth of a pixel say nothing more of the line; a later one at
    // the same place keeps the greater pressure.
    // With each, its opacity as the pressure gives it, where the brush's opacity changes.
    let (toned, drifting) = (brush.toned(), brush.drifts());
    let mut samples: Vec<(Point, f64)> = Vec::new();
    let (mut opacities, mut drifts): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());
    for p in &smoothed {
        let (at, width, opacity, drift) = (Point::new(p.x, p.y), brush.width(p.pressure), brush.opacity(p.pressure), brush.drift_at(p.pressure));
        match samples.last_mut() {
            Some((last, last_width)) if last.distance(at) < 0.1 => {
                *last_width = last_width.max(width);
                if let (Some(last_opacity), Some(last_drift)) = (opacities.last_mut(), drifts.last_mut()) {
                    (*last_opacity, *last_drift) = (last_opacity.max(opacity), last_drift.max(drift));
                }
            }
            _ => {
                samples.push((at, width));
                opacities.push(opacity);
                drifts.push(drift);
            }
        }
    }
    if !toned {
        opacities.clear();
    }
    if !drifting {
        drifts.clear();
    }
    let all_opaque = |opacities: &[f64]| opacities.iter().all(|opacity| *opacity >= 1.0 - 1e-6);
    let (first, first_width) = *samples.first()?;
    if samples.len() == 1 {
        let mut dot = BezPath::new();
        dot.move_to(first);
        // A dot is as faint as the touch, and has no length to fade along.
        return Some(VectorStroke {
            path: dot,
            widths: vec![first_width],
            nib: brush.nib(),
            caps: Default::default(),
            grain: brush.grain(),
            opacities: opacities.into_iter().filter(|opacity| *opacity < 1.0).collect(),
            drifts: drifts.into_iter().filter(|drift| *drift > 0.0).collect(),
            boil: brush.boiling(),
            stamp: brush.stamping(),
        });
    }
    // Along the pen's path: how far each sample is from the start.
    let mut along = vec![0.0];
    for pair in samples.windows(2) {
        along.push(along[along.len() - 1] + pair[0].0.distance(pair[1].0));
    }
    let length = along[along.len() - 1];
    // The ends narrow as the brush's tapers say; the fitting keeps the points they need.
    for ((_, width), from) in samples.iter_mut().zip(&along) {
        *width *= brush.tapered(*from, length);
    }
    // And fade as its fades say.
    for (opacity, from) in opacities.iter_mut().zip(&along) {
        *opacity *= brush.faded(*from, length);
    }
    let mut polyline = BezPath::new();
    polyline.move_to(first);
    samples[1..].iter().for_each(|(at, _)| polyline.line_to(*at));
    let options = SimplifyOptions { tolerance: f64::from(brush.simplify).max(0.01), corner_angle_deg: CORNER_DEGREES, straight_lines: false };
    let fitted = vectorcraft_pathops::simplify_with(&PathData::from_bezpath(&polyline), &options).to_bezpath();
    let segments: Vec<PathSeg> = fitted.segments().collect();
    if segments.is_empty() {
        return None;
    }
    // The fitted line is about as long as the pen's path: a place on one is taken for the
    // place as far along the other, in proportion.
    let lengths: Vec<f64> = segments.iter().map(|segment| segment.arclen(0.01)).collect();
    let fitted_length: f64 = lengths.iter().sum();
    let scale = if fitted_length > 0.0 { length / fitted_length } else { 1.0 };
    let width_at = |distance: f64| width_along(&along, &samples, distance);
    let opacity_at = |distance: f64| value_along(&along, &opacities, distance);
    let drift_at = |distance: f64| value_along(&along, &drifts, distance);
    // How far the width may differ from what goes evenly between two points before another
    // point is put between them.
    let close_enough = (f64::from(brush.size) * 0.04).max(0.25);
    let mut path = BezPath::new();
    path.move_to(first);
    let mut widths = vec![width_at(0.0)];
    let mut kept_opacities = if toned { vec![opacity_at(0.0)] } else { Vec::new() };
    let mut kept_drifts = if drifting { vec![drift_at(0.0)] } else { Vec::new() };
    let mut start = 0.0;
    for (segment, segment_length) in segments.into_iter().zip(lengths) {
        let end = start + segment_length;
        let mut pieces = Vec::new();
        refine(segment, (start, end), scale, &along, (&samples, &opacities, &drifts), close_enough, 0, &mut pieces);
        for (piece, to) in pieces {
            match piece {
                PathSeg::Line(line) => path.line_to(line.p1),
                PathSeg::Quad(quad) => path.quad_to(quad.p1, quad.p2),
                PathSeg::Cubic(cubic) => path.curve_to(cubic.p1, cubic.p2, cubic.p3),
            }
            widths.push(width_at(to * scale));
            if toned {
                kept_opacities.push(opacity_at(to * scale));
            }
            if drifting {
                kept_drifts.push(drift_at(to * scale));
            }
        }
        start = end;
    }
    if all_opaque(&kept_opacities) {
        kept_opacities.clear();
    }
    if kept_drifts.iter().all(|drift| *drift <= 1e-6) {
        kept_drifts.clear();
    }
    Some(VectorStroke {
        path,
        widths,
        nib: brush.nib(),
        caps: Default::default(),
        grain: brush.grain(),
        opacities: kept_opacities,
        drifts: kept_drifts,
        boil: brush.boiling(),
        stamp: brush.stamping(),
    })
}

/// `segment`, which lies from `range.0` to `range.1` along the fitted line, cut where the
/// pen's width strays from going evenly between its ends by more than `close_enough`, or its
/// opacity or drift (of `samples.1` and `samples.2`, where there are any) by more than a
/// thirtieth: each piece with how far along the fitted line it ends.
#[allow(clippy::too_many_arguments)]
fn refine(
    segment: PathSeg,
    range: (f64, f64),
    scale: f64,
    along: &[f64],
    samples: (&[(Point, f64)], &[f64], &[f64]),
    close_enough: f64,
    depth: u32,
    pieces: &mut Vec<(PathSeg, f64)>,
) {
    let (start, end) = range;
    let (widths, opacities, drifts) = samples;
    let width = |distance: f64| width_along(along, widths, distance);
    let (from, to) = (width(start * scale), width(end * scale));
    // How far one of the other lists strays at sample `i`, `t` of the way along, in thirtieths,
    // from going evenly between its values at the ends.
    let ends = |values: &[f64]| (value_along(along, values, start * scale), value_along(along, values, end * scale));
    let (faint, drift) = (ends(opacities), ends(drifts));
    let off_of = |values: &[f64], (a, b): (f64, f64), i: usize, t: f64| values.get(i).map_or(0.0, |v| (v - (a + (b - a) * t)).abs() * 30.0);
    // The sample within the segment furthest from the width, or the opacity, going evenly, in
    // parts of what is close enough.
    let within = along.iter().enumerate().filter(|(_, d)| **d > start * scale && **d < end * scale);
    let stray = |(i, d): (usize, &f64)| {
        let t = (d / scale - start) / (end - start);
        let off = (widths[i].1 - (from + (to - from) * t)).abs() / close_enough;
        (i, off.max(off_of(opacities, faint, i, t)).max(off_of(drifts, drift, i, t)), d / scale)
    };
    let furthest = within.map(stray).max_by(|a, b| a.1.total_cmp(&b.1));
    match furthest {
        Some((_, off, at)) if off > 1.0 && end - start > 1.0 && depth < 16 => {
            let t = segment.inv_arclen(at - start, 0.01).clamp(0.0, 1.0);
            refine(segment.subsegment(0.0..t), (start, at), scale, along, samples, close_enough, depth + 1, pieces);
            refine(segment.subsegment(t..1.0), (at, end), scale, along, samples, close_enough, depth + 1, pieces);
        }
        _ => pieces.push((segment, end)),
    }
}

/// A vector stroke being drawn, seen on its art layer as it goes. It is drawn along the pen's
/// smoothed path piece by piece, which is quick; the stroke that is kept is fitted once the
/// pen lifts ([`vector_stroke`]), and strays from this by no more than the brush's `simplify`.
pub struct LiveVector {
    brush: VectorBrush,
    swatch: SwatchId,
    /// What is painted with these swatches is left as it is.
    protected: Vec<SwatchId>,
    /// How the stroke is seen laid on what is there: over it, under it, or on it alone.
    mode: crate::PaintMode,
    /// The art layer's pixels as they were before the stroke, and with the stroke so far.
    before: IndexedSurface,
    layer: IndexedSurface,
    /// How much the stroke so far covers each pixel, by tile: the most any piece of it covers.
    coverage: HashMap<TileCoord, Vec<f32>>,
    /// How far its colour has drifted at each pixel, by tile, where it drifts: the most of any
    /// piece over it.
    drift: HashMap<TileCoord, Vec<f32>>,
    /// The smoothed path's last point and its width.
    last: Option<(Point, f64)>,
    /// Its opacity and drift there.
    last_opacity: f64,
    last_drift: f64,
    /// How far the smoothed path has gone, for the taper at the start. The end's taper is not
    /// seen until the pen lifts, when the end is known.
    gone: f64,
    rasterizer: Rasterizer,
    /// Where the brush stamps a tip: what lays its stamps, and the place along the pen's path
    /// it stamped to last.
    stamper: Option<(Stamper, Option<Place>)>,
}

impl LiveVector {
    /// Start a stroke of `brush` with `swatch` over `layer`, the pixels its art layer is drawn as.
    pub fn new(layer: &IndexedSurface, brush: &VectorBrush, swatch: SwatchId) -> LiveVector {
        LiveVector {
            brush: brush.clone(),
            swatch,
            protected: Vec::new(),
            mode: crate::PaintMode::Over,
            before: layer.clone(),
            layer: layer.clone(),
            coverage: HashMap::new(),
            drift: HashMap::new(),
            last: None,
            last_opacity: 1.0,
            last_drift: 0.0,
            gone: 0.0,
            rasterizer: Rasterizer::default(),
            stamper: brush.stamping().and_then(|stamp| Stamper::new(&stamp, brush.nib())).map(|stamper| (stamper, None)),
        }
    }

    /// The same, leaving what is painted with the `protected` swatches as it is.
    pub fn protecting(self, protected: Vec<SwatchId>) -> LiveVector {
        LiveVector { protected, ..self }
    }

    /// The same, seen laid in `mode`: over what is painted, under it, or on it alone.
    pub fn in_mode(self, mode: crate::PaintMode) -> LiveVector {
        LiveVector { mode, ..self }
    }

    /// The art layer with the stroke so far.
    pub fn layer(&self) -> &IndexedSurface {
        &self.layer
    }

    /// Draw the stroke on to `points`. Returns the pixels that changed.
    pub fn push(&mut self, points: &[StrokePoint]) -> Rect {
        let keep = 1.0 - f64::from(self.brush.smoothing.clamp(0.0, 0.95));
        let mut changed = Rect::EMPTY;
        for p in points.iter().filter(|p| p.x.is_finite() && p.y.is_finite()) {
            let (from, at) = match self.last {
                None => (None, Point::new(p.x, p.y)),
                Some((last, _)) => (self.last, last + (Point::new(p.x, p.y) - last) * keep),
            };
            if from.is_some_and(|(last, _)| last.distance(at) < 0.1) {
                continue;
            }
            self.gone += from.map_or(0.0, |(last, _)| last.distance(at));
            let width = self.brush.width(p.pressure) * self.brush.tapered(self.gone, f64::INFINITY);
            // A piece is as faint as its ends are, between them; the end's fade comes when the
            // pen lifts.
            let opacity = self.brush.opacity(p.pressure) * self.brush.faded(self.gone, f64::INFINITY);
            let drift = self.brush.drift_at(p.pressure);
            let (faint, drifted) = match from {
                Some(_) => (((self.last_opacity + opacity) / 2.0) as f32, ((self.last_drift + drift) / 2.0) as f32),
                None => (opacity as f32, drift as f32),
            };
            (self.last_opacity, self.last_drift) = (opacity, drift);
            let mut path = BezPath::new();
            let widths = match from {
                Some((last, last_width)) => {
                    path.move_to(last);
                    path.line_to(at);
                    vec![last_width, width]
                }
                None => {
                    path.move_to(at);
                    vec![width]
                }
            };
            self.last = Some((at, width));
            // Stamped, the stamps that fall on the piece, each as faint and drifted as the pen
            // was there.
            if let Some((stamper, last)) = &mut self.stamper {
                let place = Place { at, along: self.gone, radius: width / 2.0, opacity, drift };
                let mut laid = Laid { coverage: std::mem::take(&mut self.coverage), drift: std::mem::take(&mut self.drift) };
                changed = changed.union(&stamper.between(last.as_ref().unwrap_or(&place), &place, &mut laid));
                (self.coverage, self.drift) = (laid.coverage, laid.drift);
                *last = Some(place);
                continue;
            }
            let piece = VectorStroke { path, widths, nib: self.brush.nib(), caps: Default::default(), grain: None, ..Default::default() }.contour();
            let (coverage, drifts) = (&mut self.coverage, &mut self.drift);
            self.rasterizer.fill(&piece, |rect, cover| {
                changed = changed.union(&rect);
                if drifted > 0.0 {
                    most(drifts, rect, &cover.iter().map(|c| if *c > 0.0 { drifted } else { 0.0 }).collect::<Vec<f32>>());
                }
                match faint < 1.0 {
                    true => most(coverage, rect, &cover.iter().map(|c| c * faint).collect::<Vec<f32>>()),
                    false => most(coverage, rect, cover),
                }
            });
        }
        self.refresh(changed)
    }

    /// Paint the tiles that `changed` touches anew: as they were before the stroke, with the
    /// stroke's coverage so far painted on once.
    fn refresh(&mut self, changed: Rect) -> Rect {
        if changed.is_empty() {
            return changed;
        }
        let tiles = changed.tiles().fold(Rect::EMPTY, |all, tile| all.union(&tile.rect()));
        self.layer.copy_tiles_from(&self.before, tiles);
        for tile in changed.tiles() {
            if let Some(coverage) = self.coverage.get(&tile) {
                let drift = self.drift.get(&tile).map(Vec::as_slice);
                self.layer.paint_drifted(self.mode, tile.rect(), coverage, drift, self.swatch, &self.protected);
            }
        }
        tiles
    }
}

/// Keep in `tiles` the most of what they held and of `cover`, the coverage of `rect` in rows.
fn most(tiles: &mut HashMap<TileCoord, Vec<f32>>, rect: Rect, cover: &[f32]) {
    let side = TILE_SIZE;
    let width = rect.width() as usize;
    for (row, y) in (rect.y0..rect.y1).enumerate() {
        for (column, x) in (rect.x0..rect.x1).enumerate() {
            let value = cover[row * width + column];
            if value <= 0.0 {
                continue;
            }
            let coord = TileCoord::new(x.div_euclid(side), y.div_euclid(side));
            let tile = tiles.entry(coord).or_insert_with(|| vec![0.0; (TILE_SIZE * TILE_SIZE) as usize]);
            let at = (y.rem_euclid(side) * side + x.rem_euclid(side)) as usize;
            tile[at] = tile[at].max(value);
        }
    }
}

/// One of `values`, kept by sample, `distance` along the pen's path, between the samples on
/// either side: 1 where there are none.
fn value_along(along: &[f64], values: &[f64], distance: f64) -> f64 {
    if values.is_empty() {
        return 1.0;
    }
    let after = along.partition_point(|d| *d < distance).min(values.len() - 1);
    if after == 0 {
        return values[0];
    }
    let (d0, d1) = (along[after - 1], along[after]);
    let t = if d1 > d0 { (distance - d0) / (d1 - d0) } else { 1.0 };
    values[after - 1] + (values[after] - values[after - 1]) * t.clamp(0.0, 1.0)
}

/// The pen's width `distance` along its path, between the samples on either side.
fn width_along(along: &[f64], samples: &[(Point, f64)], distance: f64) -> f64 {
    let after = along.partition_point(|d| *d < distance).min(samples.len() - 1);
    if after == 0 {
        return samples[0].1;
    }
    let (d0, d1) = (along[after - 1], along[after]);
    let t = if d1 > d0 { (distance - d0) / (d1 - d0) } else { 1.0 };
    samples[after - 1].1 + (samples[after].1 - samples[after - 1].1) * t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use kurbo::{ParamCurveNearest, Shape};

    use super::*;

    fn pen(points: &[(f64, f64, f32)]) -> Vec<StrokePoint> {
        points.iter().map(|&(x, y, pressure)| StrokePoint::new(x, y, pressure)).collect()
    }

    fn even(brush: &VectorBrush) -> VectorBrush {
        VectorBrush { pressure_size: false, smoothing: 0.0, ..brush.clone() }
    }

    #[test]
    fn the_pressure_curve_shapes_how_the_width_answers_the_pressure() {
        let straight = VectorBrush { size: 10.0, thinnest: 0.0, ..VectorBrush::default() };
        assert!((straight.width(0.5) - 5.0).abs() < 1e-9);
        // Soft: half the pressure gives most of the width; the ends stay where they were.
        let soft = VectorBrush { pressure_curve: vec![[0.0, 0.0], [0.5, 0.8], [1.0, 1.0]], ..straight.clone() };
        assert!((soft.width(0.5) - 8.0).abs() < 1e-9 && soft.width(0.0) == 0.0 && (soft.width(1.0) - 10.0).abs() < 1e-9);
        assert!(soft.width(0.25) > straight.width(0.25) && soft.width(0.25) < soft.width(0.5));
        // Above the thinnest, as the straight line is.
        let thin = VectorBrush { thinnest: 0.2, ..soft.clone() };
        assert!((thin.width(0.5) - 10.0 * (0.2 + 0.8 * 0.8)).abs() < 1e-6);
        // Points out of order are the straight line; without pressure the curve plays no part.
        assert_eq!(VectorBrush { pressure_curve: vec![[0.6, 0.1], [0.2, 0.9]], ..straight.clone() }.width(0.5), straight.width(0.5));
        assert_eq!(VectorBrush { pressure_size: false, ..soft }.width(0.3), 10.0);
    }

    #[test]
    fn a_lighter_touch_is_fainter_and_the_ends_fade() {
        // Pressed from lightly to fully along 400 px.
        let points: Vec<_> = (0..=100).map(|i| (f64::from(i) * 4.0, 50.0, 0.1 + 0.9 * i as f32 / 100.0)).collect();
        let soft = VectorBrush { size: 10.0, smoothing: 0.0, pressure_opacity: true, faintest: 0.0, ..VectorBrush::default() };
        let stroke = vector_stroke(&pen(&points), &soft).unwrap();
        assert_eq!(stroke.opacities.len(), stroke.widths.len());
        let (first, last) = (stroke.opacities[0], stroke.opacities[stroke.opacities.len() - 1]);
        assert!((first - 0.1).abs() < 0.02 && (last - 1.0).abs() < 0.02, "{first} to {last}");
        // Kept close to the pen's along the line, with more points where it changes.
        let art =
            crate::VectorArt::new(vec![crate::VectorShape { id: crate::ShapeId(1), swatch: SwatchId(1), kind: crate::ShapeKind::Stroke(stroke.clone()) }]);
        let at = |x: i32| f64::from(art.raster().pixel(x, 50).coverage) / 65535.0;
        assert!((at(200) - 0.55).abs() < 0.04, "halfway {}", at(200));
        // Fading over three widths at its end, and one at its start, it starts and ends at 0.
        let fading = VectorBrush { pressure_opacity: false, fade_start: 1.0, fade_end: 3.0, ..soft.clone() };
        let faded = vector_stroke(&pen(&points), &fading).unwrap();
        assert!(faded.opacities[0] < 1e-9 && faded.opacities[faded.opacities.len() - 1] < 1e-9, "{:?}", faded.opacities);
        let art = crate::VectorArt::new(vec![crate::VectorShape { id: crate::ShapeId(1), swatch: SwatchId(1), kind: crate::ShapeKind::Stroke(faded) }]);
        let at = |x: i32| f64::from(art.raster().pixel(x, 50).coverage) / 65535.0;
        assert!(at(200) > 0.98 && (at(385) - 0.5).abs() < 0.06, "{} and {}", at(200), at(385));
        // A brush whose opacity does not change keeps none.
        assert!(vector_stroke(&pen(&points), &VectorBrush { pressure_opacity: false, ..soft.clone() }).unwrap().opacities.is_empty());
        // Laid on a stroke, a soft brush makes it faint where it is thin.
        let laid = rebrushed(&VectorStroke::even(stroke.path.clone(), 10.0), &soft);
        assert!(laid.opacities.iter().all(|o| (o - 1.0).abs() < 1e-9));
        let thin_to_thick = rebrushed(&stroke, &VectorBrush { thinnest: 0.0, ..soft.clone() });
        assert!(thin_to_thick.opacities[0] < 0.2);
        // Seen while drawn: as faint as it will be kept, but for the end's fade.
        let mut live = LiveVector::new(&IndexedSurface::new(), &soft, SwatchId(1));
        live.push(&pen(&points));
        let seen = |x: i32| f64::from(live.layer().pixel(x, 50).coverage) / 65535.0;
        assert!((seen(20) - 0.18).abs() < 0.05 && seen(380) > 0.9, "{} and {}", seen(20), seen(380));
    }

    #[test]
    fn the_harder_the_pen_presses_the_further_the_colour_drifts() {
        let points: Vec<_> = (0..=100).map(|i| (f64::from(i) * 4.0, 50.0, 0.1 + 0.9 * i as f32 / 100.0)).collect();
        let drifting = VectorBrush { size: 10.0, smoothing: 0.0, drift: 0.8, ..VectorBrush::default() };
        let stroke = vector_stroke(&pen(&points), &drifting).unwrap();
        let (first, last) = (stroke.drifts[0], stroke.drifts[stroke.drifts.len() - 1]);
        assert!((first - 0.08).abs() < 0.02 && (last - 0.8).abs() < 0.02, "{first} to {last}");
        assert!(stroke.opacities.is_empty(), "opaque");
        // Not by pressure, as far all along; with none, none kept.
        let even = vector_stroke(&pen(&points), &VectorBrush { pressure_drift: false, ..drifting.clone() }).unwrap();
        assert!(even.drifts.iter().all(|d| (d - 0.8).abs() < 1e-6));
        assert!(vector_stroke(&pen(&points), &VectorBrush { drift: 0.0, ..drifting.clone() }).unwrap().drifts.is_empty());
        // A boiling brush's strokes boil, every so many frames; a still one's do not.
        let boiling = VectorBrush { boil: 1.5, boil_every: 3, ..drifting.clone() };
        assert_eq!(vector_stroke(&pen(&points), &boiling).unwrap().boil, Some(crate::Boil { amount: 1.5, every: 3 }));
        assert_eq!(rebrushed(&stroke, &boiling).boil, Some(crate::Boil { amount: 1.5, every: 3 }));
        assert!(stroke.boil.is_none());
        // Seen drifting while drawn.
        let mut live = LiveVector::new(&IndexedSurface::new(), &drifting, SwatchId(3));
        live.push(&pen(&points));
        let (start, end) = (live.layer().pixel(10, 50).swatch, live.layer().pixel(390, 50).swatch);
        assert!(start.drift() <= 2 && end.drift() >= 11 && end.base() == SwatchId(3), "{start:?} to {end:?}");
    }

    #[test]
    fn a_surface_is_the_area_the_stroke_covers_in_a_few_curves() {
        // A wavy line with the pressure rising and falling, and one that loops over itself.
        let wave: Vec<_> = (0..=200).map(|i| (f64::from(i) * 2.0, 20.0 * (f64::from(i) / 20.0).sin(), (0.3 + 0.7 * f64::from(i) / 200.0) as f32)).collect();
        let looped: Vec<_> =
            (0..=200).map(|i| f64::from(i) / 200.0 * 2.0 * std::f64::consts::TAU).map(|t| (60.0 * t.cos() + 10.0 * t, 60.0 * t.sin(), 1.0)).collect();
        for points in [wave, looped] {
            let stroke = vector_stroke(&pen(&points), &VectorBrush { size: 16.0, ..VectorBrush::default() }).unwrap();
            let area = surface(&stroke, &VectorBrush { simplify: 0.5, ..VectorBrush::default() });
            let (contour, fitted) = (stroke.contour(), area.segments().count());
            assert!(fitted * 4 < contour.segments().count(), "{fitted} curves for the contour's {}", contour.segments().count());
            // The same pixels as the stroke, within its edge.
            let covered = |path: &BezPath| {
                let (from, to) = (contour.bounding_box().x0 as i32, contour.bounding_box().x1 as i32);
                let (top, bottom) = (contour.bounding_box().y0 as i32, contour.bounding_box().y1 as i32);
                (from..to)
                    .flat_map(|x| (top..bottom).map(move |y| Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5)))
                    .filter(|p| path.winding(*p) != 0)
                    .collect::<Vec<_>>()
            };
            let (stroke_px, surface_px) = (covered(&contour), covered(&area));
            let apart = stroke_px.iter().filter(|p| area.winding(**p) == 0).count() + surface_px.iter().filter(|p| contour.winding(**p) == 0).count();
            // Pixels whose middles the fitted edge passes the other side of, no more than a few
            // in ten along it.
            let edge = contour.perimeter(0.1);
            assert!((apart as f64) < edge * 0.15 && apart * 50 < stroke_px.len(), "{apart} pixels apart of {}, along {edge:.0}", stroke_px.len());
        }
    }

    #[test]
    fn a_surface_keeps_its_strokes_tone_but_never_its_grain() {
        let points: Vec<_> = (0..=40).map(|i| (f64::from(i) * 5.0, 50.0, 1.0)).collect();
        let mut grained = VectorBrush { size: 20.0, ..VectorBrush::default() };
        grained.texture.enabled = true;
        let stroke = vector_stroke(&pen(&points), &grained).unwrap();
        assert!(stroke.grained(), "a line keeps its grain");
        let crate::ShapeKind::Fill(fill) = surface_shape(&stroke, &grained) else { panic!("a fill") };
        assert!(fill.tracks.is_empty() && !fill.grained(), "a surface is plain");
        // Fading, it keeps the stroke for its tone, with no grain.
        let fading = VectorBrush { fade_end: 3.0, ..grained };
        let stroke = vector_stroke(&pen(&points), &fading).unwrap();
        let crate::ShapeKind::Fill(fill) = surface_shape(&stroke, &fading) else { panic!("a fill") };
        assert_eq!(fill.tracks.len(), 1);
        assert!(fill.tracks[0].toned() && !fill.grained());
    }

    #[test]
    fn a_straight_line_drawn_with_many_samples_is_one_segment() {
        let points: Vec<_> = (0..=100).map(|i| (f64::from(i) * 3.0, 50.0, 1.0)).collect();
        let stroke = vector_stroke(&pen(&points), &even(&VectorBrush::default())).unwrap();
        assert_eq!(stroke.path.segments().count(), 1, "{:?}", stroke.path.to_svg());
        assert_eq!(stroke.widths, [8.0, 8.0]);
    }

    #[test]
    fn a_curve_is_fitted_with_few_points_close_to_the_pen() {
        let points: Vec<_> = (0..=200)
            .map(|i| (100.0 * (f64::from(i) / 200.0 * std::f64::consts::PI).cos(), 100.0 * (f64::from(i) / 200.0 * std::f64::consts::PI).sin(), 1.0))
            .collect();
        let stroke = vector_stroke(&pen(&points), &even(&VectorBrush::default())).unwrap();
        let count = stroke.path.segments().count();
        assert!((1..=6).contains(&count), "a half circle in {count} segments");
        assert_eq!(stroke.widths.len(), count + 1, "a width at every point");
        for &(x, y, _) in points.iter().step_by(10) {
            let near = stroke.path.segments().map(|s| s.nearest(Point::new(x, y), 1e-6).distance_sq.sqrt()).fold(f64::INFINITY, f64::min);
            assert!(near < 0.6, "({x}, {y}) is {near} from the fitted line");
        }
    }

    #[test]
    fn the_pressure_is_kept_as_the_width_along_the_line() {
        // A straight line pressed hard in its middle: the width swells there and nowhere else.
        let points: Vec<_> = (0..=100).map(|i| (f64::from(i) * 4.0, 0.0, 1.0 - (f64::from(i) - 50.0).abs() as f32 / 50.0)).collect();
        let brush = VectorBrush { size: 20.0, thinnest: 0.0, smoothing: 0.0, ..VectorBrush::default() };
        let stroke = vector_stroke(&pen(&points), &brush).unwrap();
        assert!(stroke.path.segments().count() >= 2, "a point where the width turns: {:?}", stroke.widths);
        let widest = stroke.widths.iter().copied().fold(0.0, f64::max);
        assert!((widest - 20.0).abs() < 1.0 && stroke.widths[0] < 1.0 && stroke.widths[stroke.widths.len() - 1] < 1.0, "{:?}", stroke.widths);
    }

    #[test]
    fn a_line_takes_a_new_brush_keeping_its_shape_and_its_thick_and_thin() {
        // A line of three points, thin, thick, half: as a pen pressed.
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((100.0, 0.0));
        path.line_to((200.0, 0.0));
        let stroke = VectorStroke { path: path.clone(), widths: vec![2.0, 8.0, 4.0], ..VectorStroke::even(path.clone(), 1.0) };
        // Even, of size 20: every point 20.
        let even = VectorBrush { size: 20.0, pressure_size: false, ..VectorBrush::default() };
        let laid = rebrushed(&stroke, &even);
        assert_eq!((laid.path.clone(), laid.widths.clone()), (path.clone(), vec![20.0, 20.0, 20.0]));
        // Following the pressure, from no width: the thickest the size, the others as they were to it.
        let pressed = VectorBrush { size: 20.0, pressure_size: true, thinnest: 0.0, ..VectorBrush::default() };
        let laid = rebrushed(&stroke, &pressed);
        assert!((laid.widths[1] - 20.0).abs() < 1e-9 && (laid.widths[0] - 5.0).abs() < 1e-9 && (laid.widths[2] - 10.0).abs() < 1e-9, "{:?}", laid.widths);
        // Tapered over half its size at the start: the first point is nothing, the others whole.
        let tapered = VectorBrush { taper_start: 0.5, ..even.clone() };
        assert_eq!(rebrushed(&stroke, &tapered).widths, vec![0.0, 20.0, 20.0]);
        // A calligraphic nib is taken with it.
        let calligraphy = VectorBrush { angle: 45.0, roundness: 0.2, ..even };
        assert_eq!(rebrushed(&stroke, &calligraphy).nib, calligraphy.nib());
    }

    #[test]
    fn a_tap_is_a_dot() {
        let stroke = vector_stroke(&pen(&[(10.0, 10.0, 0.5), (10.02, 10.0, 1.0)]), &VectorBrush::default()).unwrap();
        assert_eq!((stroke.path.segments().count(), stroke.widths.len()), (0, 1));
        assert!((stroke.widths[0] - 8.0).abs() < 1e-9, "the greater pressure");
        assert!(vector_stroke(&[], &VectorBrush::default()).is_none());
        assert!(vector_stroke(&pen(&[(f64::NAN, 0.0, 1.0)]), &VectorBrush::default()).is_none());
    }

    #[test]
    fn the_stroke_seen_as_it_is_drawn_is_close_to_the_one_kept() {
        let points: Vec<_> = (0..=120).map(|i| (f64::from(i) * 3.0, 40.0 * (f64::from(i) / 20.0).sin(), 0.3 + 0.7 * (f64::from(i) / 120.0) as f32)).collect();
        let brush = VectorBrush { size: 12.0, ..VectorBrush::default() };
        let mut live = LiveVector::new(&IndexedSurface::new(), &brush, SwatchId(2));
        // Fed a few points at a time, as the window does.
        for chunk in pen(&points).chunks(7) {
            live.push(chunk);
        }
        let kept = crate::VectorArt::new(vec![crate::VectorShape {
            id: crate::ShapeId(1),
            swatch: SwatchId(2),
            kind: crate::ShapeKind::Stroke(vector_stroke(&pen(&points), &brush).unwrap()),
        }]);
        let (seen, kept) = (live.layer().bounds(), kept.raster().bounds());
        let near = |a: i32, b: i32| (a - b).abs() <= 2;
        assert!(near(seen.x0, kept.x0) && near(seen.y0, kept.y0) && near(seen.y1, kept.y1), "seen {seen:?}, kept {kept:?}");
        // Pieces that meet don't build up coverage where they overlap: the inside is whole,
        // and an edge no more than one piece makes it.
        let middle = live.layer().pixel(180, (40.0 * 3.0_f64.sin()) as i32);
        assert_eq!((middle.swatch, middle.coverage), (SwatchId(2), u16::MAX));
    }

    #[test]
    fn the_ends_taper_over_so_many_widths() {
        let points: Vec<_> = (0..=100).map(|i| (f64::from(i) * 4.0, 0.0, 1.0)).collect();
        let brush = VectorBrush { size: 10.0, taper_start: 3.0, taper_end: 6.0, ..even(&VectorBrush::default()) };
        let stroke = vector_stroke(&pen(&points), &brush).unwrap();
        let ends: Vec<(f64, f64)> = stroke.path.segments().map(|s| s.end().x).zip(stroke.widths[1..].iter().copied()).collect();
        assert!(stroke.widths[0] < 0.5 && stroke.widths[stroke.widths.len() - 1] < 0.5, "pointed ends: {:?}", stroke.widths);
        assert!(ends.iter().any(|(x, w)| (*x - 30.0).abs() <= 2.5 && (*w - 10.0).abs() < 0.5), "whole 30 px in: {ends:?}");
        assert!(ends.iter().any(|(x, w)| (*x - 340.0).abs() <= 2.5 && (*w - 10.0).abs() < 0.5), "and to 60 px from the end: {ends:?}");
    }

    #[test]
    fn a_sharp_turn_stays_a_corner() {
        let mut points: Vec<_> = (0..=50).map(|i| (f64::from(i) * 2.0, 0.0, 1.0)).collect();
        points.extend((1..=50).map(|i| (100.0, f64::from(i) * 2.0, 1.0)));
        let stroke = vector_stroke(&pen(&points), &even(&VectorBrush::default())).unwrap();
        let ends: Vec<Point> = stroke.path.segments().map(|s| s.end()).collect();
        assert!(ends.iter().any(|at| at.distance(Point::new(100.0, 0.0)) < 0.5), "a point at the corner: {ends:?}");
    }
}
