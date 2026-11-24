//! Times the window's picture while a stroke is drawn in a swatch that is a gradient, against
//! one of a plain colour: the drawing changes on every frame, as a spiral is drawn out to a
//! radius of 900 drawing pixels; and at the end, what painting the drawing takes, part by part.
//!
//! ```text
//! cargo run --release -p iskra-gpu --example shaded_stroke
//! ```

use std::sync::Arc;
use std::time::Instant;

use iskra_gpu::Compositor;
use iskra_model::{ArtContent, Gradient, GradientShape, GradientStop, Rect, SwatchId, SwatchKind, WorkingSpace};
use iskra_render::View;

#[path = "../../render/tests/common/mod.rs"]
#[allow(dead_code)]
mod common;

fn main() {
    let Some(mut compositor) = Compositor::headless() else { return println!("no graphics card here") };
    for shaded in [false, true] {
        let (mut project, ball, _) = common::scene();
        if shaded {
            let palette = Arc::make_mut(&mut project.palette);
            let swatch = palette.swatches.iter_mut().find(|swatch| swatch.id == SwatchId(1)).unwrap();
            let stop = |at, color| GradientStop { at, color };
            swatch.kind =
                SwatchKind::Gradient(Gradient { shape: GradientShape::Linear, stops: vec![stop(0.0, [250, 160, 40, 255]), stop(1.0, [38, 110, 210, 255])] });
        }
        let view = View::frame(&project.settings, 6.0);
        let mut t = 0.0f64;
        let mut times = Vec::new();
        for frame in 0..1200 {
            let shot = Arc::make_mut(&mut project.shots[0]);
            let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
            let drawing = Arc::make_mut(&mut layer.drawings[0].drawing);
            let ArtContent::Indexed(surface) = &mut drawing.art_layers[0].content else { unreachable!() };
            for _ in 0..4 {
                let r = 30.0 + t * 30.0;
                let (x, y) = ((r * t.cos()) as i32, (r * t.sin()) as i32);
                surface.paint(Rect::new(x - 6, y - 6, x + 6, y + 6), &[1.0; 144], SwatchId(1));
                t += 0.006;
            }
            let start = Instant::now();
            let made = compositor.render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view);
            drop(compositor.read_shown(&compositor.shown(&made)));
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            if frame % 200 == 199 {
                let last = &times[times.len() - 50..];
                if frame == 1199 && shaded {
                    parts(&project.shots[0].layers.iter().find(|layer| layer.id == ball).unwrap().drawings[0].drawing, &project.palette);
                }
                println!(
                    "{} frame {:>4}, radius {:>4.0}: {:>5.1} ms a frame",
                    if shaded { "gradient" } else { "plain   " },
                    frame + 1,
                    30.0 + t * 30.0,
                    last.iter().sum::<f64>() / 50.0
                );
            }
        }
    }
}

/// How long the parts of painting `drawing` with gradients take.
fn parts(drawing: &iskra_model::Drawing, palette: &iskra_model::Palette) {
    let bounds = drawing.bounds();
    let start = Instant::now();
    let swatches: usize = drawing.art_layers.iter().filter_map(|layer| layer.content.indexed()).map(|surface| surface.swatches().len()).sum();
    let a = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let areas = palette.areas_of(drawing);
    let b = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let pixels = palette.paint_in(drawing, bounds, &areas, WorkingSpace::Srgb);
    let c = start.elapsed().as_secs_f64() * 1000.0;
    println!("{}x{}: swatches {a:.1} ms ({swatches}), areas {b:.1} ms, paint {c:.1} ms ({})", bounds.width(), bounds.height(), pixels.len());
}
