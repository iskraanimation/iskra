//! Measures what a stroke costs per pen sample: the CPU part of pen-to-pixel latency (plan §6).
//!
//!     cargo run --release -p iskra-drawing --example stroke_latency
//!
//! One sample is pushed at a time, as the viewport does while drawing, along a wavy line across
//! a 4K-wide drawing. A 120 Hz tablet leaves about 8 ms per sample.

use std::time::Instant;

use iskra_drawing::{BrushSettings, IndexedSurface, LiveStroke, StrokePoint, SwatchId};

/// A stroke of 1500 samples by `brush`, a sample at a time, on a drawing that has `strokes`
/// strokes on it already; `step` is how far apart the samples are. Prints how long a sample
/// took: the mean, the slowest in a hundred, and the slowest.
fn timed(name: &str, brush: &BrushSettings, step: f64, strokes: u32) {
    // A fine drawing is not an empty one: what is on it already is under the new stroke.
    let mut surface = IndexedSurface::new();
    for line in 0..strokes {
        let points =
            (0..400).map(|i| StrokePoint::new(100.0 + f64::from(i) * 9.0, 400.0 + f64::from(line) * 60.0 + 80.0 * (f64::from(i) / 30.0).sin(), 1.0)).collect();
        let inked = iskra_drawing::Stroke { brush: BrushSettings { size: 30.0, ..Default::default() }, points };
        iskra_drawing::paint_stroke(&mut surface, &inked, SwatchId(2));
    }
    let mut live = LiveStroke::new(&surface, brush, SwatchId(1));
    let mut times: Vec<f64> = (0..1500)
        .map(|i| {
            let t = f64::from(i);
            let point = StrokePoint::new(100.0 + t * step, 1000.0 + 300.0 * (t / 60.0).sin(), 0.3 + 0.7 * ((t / 40.0).sin() * 0.5 + 0.5) as f32);
            let start = Instant::now();
            live.push(std::slice::from_ref(&point));
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(f64::total_cmp);
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    println!("{name:>34} {mean:>10.3} {:>10.3} {:>10.3}", times[times.len() * 99 / 100], times[times.len() - 1]);
}

fn main() {
    println!("{:>34} {:>10} {:>10} {:>10}", "brush", "mean (ms)", "p99 (ms)", "max (ms)");
    for size in [8.0, 16.0, 40.0, 100.0, 200.0, 400.0] {
        timed(&format!("round, {size} px"), &BrushSettings { size, ..Default::default() }, 2.4, 0);
    }
    // Very large brushes, to the engine's limit, on a drawing that has strokes on it; the
    // pen moves faster with them, as a hand does with a broad brush.
    for size in [1000.0, 2500.0, 5000.0] {
        timed(&format!("round, {size} px, on a drawing"), &BrushSettings { size, ..Default::default() }, 12.0, 24);
    }
    // Scattered: many stamps of the tip for each step along the stroke.
    for (size, count) in [(100.0, 4), (100.0, 16), (400.0, 16)] {
        let mut brush = BrushSettings { size, ..Default::default() };
        (brush.scattering.enabled, brush.scattering.count, brush.scattering.both_axes, brush.scattering.scatter.jitter) = (true, count, true, 3.0);
        timed(&format!("scattered {count}, {size} px, on a drawing"), &brush, 2.4, 24);
    }
    // The engine's own brushes that have the most to them, at the size they come with and large.
    for preset in iskra_drawing::builtin_brushes() {
        let iskra_drawing::Brush::Raster(brush) = &preset.brush else { continue };
        if brush.texture.enabled && (brush.dual_brush.enabled || brush.scattering.enabled) {
            timed(&format!("{}, {} px", preset.name, brush.size), brush, 2.4, 24);
            timed(&format!("{}, 800 px", preset.name), &BrushSettings { size: 800.0, ..brush.clone() }, 6.0, 24);
        }
    }
}
