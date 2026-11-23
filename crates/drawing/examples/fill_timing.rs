//! Measures what the paint bucket costs on a large drawing.
//!
//!     cargo run --release -p iskra-drawing --example fill_timing
//!
//! The drawing is 3840 by 2160 pixels, a shot of 1920 by 1080 at two pixels a unit: a frame
//! of lines round all of it, and inside it a grid of small boxes over its left half. A fill of
//! the large area round the boxes, and of one small box, is timed at each width of gap closed,
//! and so is recolouring the line of the frame.

use std::time::Instant;

use iskra_drawing::{BrushSettings, Fill, IndexedSurface, Point, Shape, SwatchId, fill, paint_stroke, recolour_line};

const LINE: SwatchId = SwatchId(1);
const SKIN: SwatchId = SwatchId(2);

fn drawing() -> IndexedSurface {
    let brush = BrushSettings { size: 6.0, pressure_size: false, ..Default::default() };
    let mut line = IndexedSurface::new();
    let rectangle = |line: &mut IndexedSurface, from: (f64, f64), to: (f64, f64)| {
        paint_stroke(line, &Shape::Rectangle.stroke(&brush, Point::new(from.0, from.1), Point::new(to.0, to.1), Default::default()), LINE);
    };
    rectangle(&mut line, (10.0, 10.0), (3830.0, 2150.0));
    for row in 0..20 {
        for column in 0..20 {
            let (x, y) = (100.0 + f64::from(column) * 90.0, 100.0 + f64::from(row) * 95.0);
            rectangle(&mut line, (x, y), (x + 60.0, y + 60.0));
        }
    }
    line
}

/// How long `run` takes, the least of three tries, in milliseconds.
fn timed(run: impl Fn()) -> f64 {
    (0..3)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::INFINITY, f64::min)
}

fn main() {
    let line = drawing();
    println!("{:>28} {:>12}", "fill", "time (ms)");
    for gap in [0, 8, 40] {
        let options = Fill { gap, under: 2 };
        let large = timed(|| _ = fill(&mut IndexedSurface::new(), &line, (3000, 1000), SKIN, &options).unwrap());
        let small = timed(|| _ = fill(&mut IndexedSurface::new(), &line, (130, 130), SKIN, &options).unwrap());
        println!("{:>28} {large:>12.1}", format!("the large area, gap {gap}"));
        println!("{:>28} {small:>12.1}", format!("a small box, gap {gap}"));
    }
    let recolour = timed(|| _ = recolour_line(&mut line.clone(), (10, 1000), SwatchId(3), &[]).unwrap());
    println!("{:>28} {recolour:>12.1}", "the frame's line recoloured");
}
