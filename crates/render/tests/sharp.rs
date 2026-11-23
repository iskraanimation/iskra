//! Vector art drawn at the size it is seen (plan §4.16, step 10): seen larger than its pixels,
//! its edges stay a pixel wide in the picture, where pixels made larger would blur them.

use std::sync::Arc;

use iskra_model::kurbo::BezPath;
use iskra_model::{ArtContent, Project, ShapeId, ShapeKind, SwatchId, VectorArt, VectorShape, VectorStroke, WorkingSpace};
use iskra_render::View;

#[path = "common/mod.rs"]
#[allow(dead_code)]
mod common;

/// The scene with the ball's drawing made a level line of vector art, or of the pixels it is
/// drawn as, or with `None` nothing.
fn with_a_line(vector: Option<bool>) -> Project {
    let (mut project, ball, _) = common::scene();
    let shot = Arc::make_mut(&mut project.shots[0]);
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
    let drawing = Arc::make_mut(&mut layer.drawings[0].drawing);
    let mut path = BezPath::new();
    path.move_to((-60.0, 0.0));
    path.line_to((60.0, 0.0));
    let line = VectorArt::new(vec![VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind: ShapeKind::Stroke(VectorStroke::even(path, 6.0)) }]);
    drawing.art_layers[0].content = ArtContent::Vector(VectorArt::default());
    drawing.art_layers[1].content = match vector {
        Some(true) => ArtContent::Vector(line),
        Some(false) => ArtContent::Indexed(line.raster().clone()),
        None => ArtContent::Vector(VectorArt::default()),
    };
    project
}

/// How many pixels of the picture are part covered by the drawing's line, neither as they are
/// without it nor the line's own colour: where its edges blur.
fn soft(project: &Project, scale: f64) -> usize {
    let view = View::frame(&project.settings, scale);
    let render = |project: &Project| iskra_render::render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view).pixels;
    let without = with_a_line(None);
    let (with, without) = (render(project), render(&without));
    let apart = |a: &[u8; 4], b: &[u8; 4]| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    // The line's own colour: what it is drawn as where most of it is.
    let changed: Vec<(&[u8; 4], &[u8; 4])> = with.iter().zip(&without).filter(|(a, b)| apart(a, b) > 8).collect();
    let mut counts = std::collections::HashMap::new();
    for (pixel, _) in &changed {
        *counts.entry(**pixel).or_insert(0) += 1;
    }
    let line = counts.into_iter().max_by_key(|(_, count)| *count).map(|(pixel, _)| pixel).unwrap_or_default();
    changed.iter().filter(|(pixel, _)| apart(pixel, &line) > 8).count()
}

#[test]
fn vector_art_seen_larger_keeps_its_edges_sharp() {
    let (vector, pixels) = (with_a_line(Some(true)), with_a_line(Some(false)));
    // At the frame's size the two are alike.
    assert_eq!(soft(&vector, 1.0), soft(&pixels, 1.0));
    // Seen eight times larger, a drawing pixel (of which the layer has two to a unit) covers
    // four of the picture's, and the pixels blur over them; the shapes don't.
    let (sharp, blurred) = (soft(&vector, 8.0), soft(&pixels, 8.0));
    assert!(sharp * 2 < blurred, "{sharp} part covered pixels drawn at size, {blurred} enlarged");
}
