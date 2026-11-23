//! Bitmap art turned into vector art, by tracing it with VectorCraft's tracer (plan §4.16,
//! step 9). The way back is the pixels vector art is drawn as.
//!
//! Indexed pixels are traced swatch by swatch: each swatch's paint alone, as black on white, so
//! every shape gets its swatch back and no colour is guessed. Free colour is traced in colour,
//! with the palette's colours alone, and each colour is matched to its swatch.

use kurbo::{BezPath, PathEl};
use serde::{Deserialize, Serialize};
use vectorcraft_trace::{Method, Mode, Palette, Raster, TraceParams, TracedPath};

use crate::{ArtContent, IndexedSurface, Rect, ShapeId, ShapeKind, SwatchId, VectorArt, VectorFill, VectorShape, VectorStroke};

/// How pixels are traced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Trace {
    /// How closely the shapes follow the pixels, 0 to 100: more is closer, with more points.
    pub detail: f64,
    /// How many corners stay sharp, 0 to 100: less is rounder.
    pub corners: f64,
    /// Lines no wider than `line_width` become strokes along their middle; wider areas, and
    /// all of it without this, filled outlines.
    pub lines: bool,
    pub line_width: f64,
    /// Specks of fewer pixels than this are left out.
    pub specks: u32,
}

impl Default for Trace {
    fn default() -> Self {
        Self { detail: 70.0, corners: 60.0, lines: false, line_width: 12.0, specks: 4 }
    }
}

impl Trace {
    fn params(&self) -> TraceParams {
        TraceParams {
            paths: self.detail.clamp(0.0, 100.0),
            corners: self.corners.clamp(0.0, 100.0),
            noise: self.specks,
            fills: true,
            strokes: self.lines,
            stroke_width: self.line_width.max(1.0),
            ignore_white: true,
            method: Method::Abutting,
            ..TraceParams::default()
        }
    }
}

/// The most anchors a trace may make before it is given up: a picture of noise traced closely
/// makes millions.
const MOST_ANCHORS: usize = 2_000_000;

/// The art layer's pixels traced into vector art, or `None` where a trace would be too large.
/// `swatch_of` gives the swatch of a colour of free colour (the palette's nearest), and
/// `palette` is the colours free colour is traced with. Vector art is given back as it is.
pub fn vectorize(content: &ArtContent, trace: &Trace, palette: &[([u8; 3], SwatchId)]) -> Option<VectorArt> {
    match content {
        ArtContent::Vector(art) => Some(art.clone()),
        ArtContent::Indexed(surface) => indexed(surface, trace),
        ArtContent::Rgba(surface) => {
            let bounds = surface.content_bounds();
            if bounds.is_empty() {
                return Some(VectorArt::default());
            }
            let mut pixels = vec![[0u8; 4]; bounds.width() as usize * bounds.height() as usize];
            surface.read_rgba8_into(bounds, &mut pixels);
            let raster = Raster::new(bounds.width(), bounds.height(), pixels.as_flattened().to_vec()).ok()?;
            let params = TraceParams {
                mode: Mode::Color,
                palette: Palette::DocumentLibrary,
                colors: palette.len().max(1) as u32,
                swatches: palette.iter().map(|(color, _)| *color).collect(),
                ignore_white: false,
                ..trace.params()
            };
            let traced = vectorcraft_trace::trace_within(&raster, &params, MOST_ANCHORS).ok()?;
            let nearest = |color: [u8; 3]| {
                let distance = |other: [u8; 3]| other.iter().zip(color).map(|(a, b)| (i32::from(*a) - i32::from(b)).pow(2)).sum::<i32>();
                palette.iter().min_by_key(|(other, _)| distance(*other)).map_or(SwatchId(1), |(_, swatch)| *swatch)
            };
            let mut shapes = Vec::new();
            for path in &traced.paths {
                shapes.extend(shapes_of(path, nearest(path.color), bounds, shapes.len()));
            }
            Some(VectorArt::new(shapes))
        }
    }
}

/// Indexed pixels traced swatch by swatch, the swatch covering most first, at the bottom.
fn indexed(surface: &IndexedSurface, trace: &Trace) -> Option<VectorArt> {
    let bounds = surface.bounds();
    if bounds.is_empty() {
        return Some(VectorArt::default());
    }
    let pixels = surface.pixels(bounds);
    let mut swatches: Vec<(SwatchId, usize)> = surface.swatches().into_iter().map(|swatch| (swatch, 0)).collect();
    for pixel in &pixels {
        if let Some(count) = swatches.iter_mut().find(|(swatch, _)| *swatch == pixel.swatch && pixel.coverage >= u16::MAX / 2) {
            count.1 += 1;
        }
    }
    swatches.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let params = trace.params();
    let mut shapes = Vec::new();
    let mut anchors = 0;
    for (swatch, _) in swatches {
        // This swatch's paint, at least half covered, black on white.
        let rgba: Vec<u8> =
            pixels.iter().flat_map(|pixel| if pixel.swatch == swatch && pixel.coverage >= u16::MAX / 2 { [0, 0, 0, 255] } else { [255; 4] }).collect();
        let raster = Raster::new(bounds.width(), bounds.height(), rgba).ok()?;
        let traced = vectorcraft_trace::trace_within(&raster, &params, MOST_ANCHORS - anchors).ok()?;
        anchors += traced.anchor_count();
        for path in &traced.paths {
            shapes.extend(shapes_of(path, swatch, bounds, shapes.len()));
        }
    }
    Some(VectorArt::new(shapes))
}

/// The shapes of one traced path, placed where its picture lies on the drawing: a fill of its
/// outline with its holes, or a stroke along each of its lines. `count` is how many shapes were
/// made before, which the new ones' ids follow.
fn shapes_of(traced: &TracedPath, swatch: SwatchId, bounds: Rect, count: usize) -> Vec<VectorShape> {
    let place = kurbo::Affine::translate((f64::from(bounds.x0), f64::from(bounds.y0)));
    let path = place * traced.path.to_bezpath();
    let id = |more: usize| ShapeId((count + more) as u32 + 1);
    match traced.stroke {
        None => vec![VectorShape { id: id(0), swatch, kind: ShapeKind::Fill(VectorFill::plain(path)) }],
        // A stroke is one line from a start: each subpath is a stroke of its own.
        Some(width) => subpaths(&path)
            .into_iter()
            .enumerate()
            .map(|(more, line)| VectorShape { id: id(more), swatch, kind: ShapeKind::Stroke(VectorStroke::even(line, width)) })
            .collect(),
    }
}

/// A path's subpaths, each a path of its own; one that closes ends back at its start.
fn subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    let mut start = kurbo::Point::ZERO;
    for element in path.elements() {
        match *element {
            PathEl::MoveTo(at) => {
                start = at;
                let mut line = BezPath::new();
                line.move_to(at);
                out.push(line);
            }
            PathEl::ClosePath => {
                if let Some(line) = out.last_mut() {
                    line.line_to(start);
                }
            }
            other => {
                if let Some(line) = out.last_mut() {
                    line.push(other);
                }
            }
        }
    }
    out.retain(|line| line.elements().len() > 1);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Indexed pixels: a filled disc of swatch 2 with a ring of swatch 1 round it.
    fn disc_in_a_ring() -> IndexedSurface {
        let mut surface = IndexedSurface::new();
        let rect = Rect::new(-60, -60, 60, 60);
        let (mut fill, mut ring) = (Vec::new(), Vec::new());
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let distance = f64::from(x * x + y * y).sqrt();
                fill.push(if distance < 40.0 { 1.0 } else { 0.0 });
                ring.push(if (40.0..48.0).contains(&distance) { 1.0 } else { 0.0 });
            }
        }
        surface.paint(rect, &fill, SwatchId(2));
        surface.paint(rect, &ring, SwatchId(1));
        surface
    }

    #[test]
    fn indexed_pixels_are_traced_swatch_by_swatch_where_they_lie() {
        let art = vectorize(&ArtContent::Indexed(disc_in_a_ring()), &Trace::default(), &[]).unwrap();
        assert_eq!(art.swatches(), [SwatchId(1), SwatchId(2)].into_iter().collect());
        assert!(art.shapes().iter().all(|shape| matches!(shape.kind, ShapeKind::Fill(_))));
        // The pixels it is drawn as are the pixels it was traced from, but at the edges.
        let raster = art.raster();
        assert_eq!((raster.pixel(0, 0).swatch, raster.pixel(44, 0).swatch, raster.pixel(55, 0).coverage), (SwatchId(2), SwatchId(1), 0));
        assert_eq!((raster.pixel(-20, 20).swatch, raster.pixel(-31, -31).swatch), (SwatchId(2), SwatchId(1)));
    }

    #[test]
    fn thin_lines_are_traced_as_strokes_along_their_middle() {
        let mut surface = IndexedSurface::new();
        let rect = Rect::new(0, 0, 200, 40);
        let line: Vec<f32> = (0..40).flat_map(|y| (0..200).map(move |x| if (18..23).contains(&y) && (10..190).contains(&x) { 1.0 } else { 0.0 })).collect();
        surface.paint(rect, &line, SwatchId(3));
        let lines = Trace { lines: true, line_width: 8.0, ..Trace::default() };
        let art = vectorize(&ArtContent::Indexed(surface), &lines, &[]).unwrap();
        let strokes: Vec<&VectorStroke> =
            art.shapes().iter().filter_map(|shape| if let ShapeKind::Stroke(stroke) = &shape.kind { Some(stroke) } else { None }).collect();
        assert_eq!(strokes.len(), 1, "{:?}", art.shapes());
        let ends = kurbo::Shape::bounding_box(&strokes[0].path);
        assert!(ends.x0 < 20.0 && ends.x1 > 180.0 && (ends.y0 - 20.0).abs() < 2.0 && (ends.y1 - 20.0).abs() < 2.0, "along the middle: {ends:?}");
    }

    #[test]
    fn free_colour_is_traced_with_the_palettes_colours_and_given_their_swatches() {
        let rgba: Vec<u8> = (0..80).flat_map(|_| (0..80).flat_map(|x| if x < 40 { [200, 30, 30, 255] } else { [30, 30, 200, 255] })).collect();
        let surface = photocraft_raster::Surface::from_interleaved(photocraft_color::PixelFormat::RGBA8, Rect::new(0, 0, 80, 80), &rgba);
        let palette = [([220, 40, 40], SwatchId(7)), ([40, 40, 220], SwatchId(9)), ([255, 255, 255], SwatchId(1))];
        let art = vectorize(&ArtContent::Rgba(surface), &Trace::default(), &palette).unwrap();
        assert_eq!(art.swatches(), [SwatchId(7), SwatchId(9)].into_iter().collect());
        assert_eq!((art.raster().pixel(20, 40).swatch, art.raster().pixel(60, 40).swatch), (SwatchId(7), SwatchId(9)));
    }
}
