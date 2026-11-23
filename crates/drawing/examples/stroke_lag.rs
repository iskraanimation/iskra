//! How long a brush takes to draw, frame by frame, as the viewport draws it: a stroke fed to
//! [`LiveStroke`] a few points at a time, with the time of each part of a frame.
//!
//! ```text
//! cargo run --release -p iskra-drawing --example stroke_lag -- [BRUSHES.json] [PRESET] [OFF...]
//! ```
//!
//! `BRUSHES.json` defaults to the app's own file, `PRESET` to the last preset in it. Each `OFF`
//! switches a part of the brush off, to see what it costs: `scatter`, `dual`, `texture`,
//! `colour`, `shape`, `transfer`, `buildup`.

use std::time::{Duration, Instant};

use iskra_drawing::{Brush, Brushes, IndexedSurface, Rect, StrokePoint, SwatchId};
use photocraft_paint::StrokeRenderer;

/// Points a frame: a pen reports about twice for each frame the screen shows.
const PER_FRAME: usize = 2;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().map(std::path::PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var("HOME").expect("HOME");
        std::path::PathBuf::from(home).join("Library/Application Support/Iskra/brushes.json")
    });
    let brushes = Brushes::load(&path);
    let name = args.next();
    let preset = match &name {
        Some(name) => brushes.presets.iter().find(|preset| &preset.name == name),
        None => brushes.presets.last(),
    }
    .unwrap_or_else(|| panic!("no preset {name:?} in {}", path.display()));
    let Brush::Raster(mut brush) = preset.brush.clone() else { panic!("{} is a vector brush", preset.name) };
    let off: Vec<String> = args.collect();
    for part in &off {
        match part.as_str() {
            "scatter" => brush.scattering.enabled = false,
            "dual" => brush.dual_brush.enabled = false,
            "texture" => brush.texture.enabled = false,
            "colour" => brush.color_dynamics.enabled = false,
            "shape" => brush.shape_dynamics.enabled = false,
            "transfer" => brush.transfer.enabled = false,
            "buildup" => brush.build_up = false,
            other => panic!("no part {other}"),
        }
    }
    println!("{} ({}), size {}, spacing {:.0} %, off: {off:?}", preset.name, preset.group, brush.size, brush.spacing * 100.0);

    // A wave across 2000 pixels, pressed harder in the middle, at 240 points a second.
    let points: Vec<StrokePoint> = (0..=480)
        .map(|i| {
            let along = f64::from(i) / 480.0;
            let pressure = (0.3 + 0.7 * (std::f64::consts::PI * along).sin()) as f32;
            StrokePoint { time: along * 2000.0, ..StrokePoint::new(200.0 + 2000.0 * along, 800.0 + 300.0 * (6.0 * along).sin(), pressure) }
        })
        .collect();

    // The parts of a frame, as LiveStroke does them.
    let before = IndexedSurface::new();
    let mut layer = before.clone();
    let mut renderer = StrokeRenderer::new(&brush, None, 1.0);
    let (mut engine, mut copy, mut coverage, mut paint) = (Duration::ZERO, Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let mut frames = Vec::new();
    let mut pixels = 0u64;
    for chunk in points.chunks(PER_FRAME) {
        let start = Instant::now();
        renderer.push(chunk);
        let pushed = Instant::now();
        let dirty = renderer.take_dirty_rect();
        let tiles = dirty.tiles().fold(Rect::EMPTY, |all, tile| all.union(&tile.rect()));
        let area = tiles.intersect(&renderer.bounds());
        let (mut copied, mut covered) = (pushed, pushed);
        if !area.is_empty() {
            layer.copy_tiles_from(&before, tiles);
            copied = Instant::now();
            let values = renderer.coverage_in(area);
            covered = Instant::now();
            layer.paint(area, &values, SwatchId(1));
            pixels += u64::from(area.width()) * u64::from(area.height());
        }
        let end = Instant::now();
        engine += pushed - start;
        copy += copied.saturating_duration_since(pushed);
        coverage += covered.saturating_duration_since(copied);
        paint += end.saturating_duration_since(covered);
        frames.push(end - start);
    }
    let total: Duration = frames.iter().sum();
    let mut sorted = frames.clone();
    sorted.sort();
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "{} frames: {:.1} ms in all; a frame {:.1} ms at the median, {:.1} at 95 %, {:.1} at most",
        frames.len(),
        ms(total),
        ms(at(0.5)),
        ms(at(0.95)),
        ms(sorted[sorted.len() - 1])
    );
    println!(
        "engine {:.0} ms, tiles put back {:.0} ms, coverage {:.0} ms, painting {:.0} ms; {:.1} Mpx redone",
        ms(engine),
        ms(copy),
        ms(coverage),
        ms(paint),
        pixels as f64 / 1e6
    );

    // When the pen lifts: the viewport sends the stroke to the engine as JSON, and the engine
    // paints it anew on the drawing as it was.
    let start = Instant::now();
    let stroke = iskra_drawing::Stroke { brush: brush.clone(), points: points.clone() };
    let json = serde_json::to_value(&stroke).unwrap();
    let text_len = json.to_string().len();
    let back: iskra_drawing::Stroke = serde_json::from_value(json).unwrap();
    let sent = Instant::now();
    let mut painted = IndexedSurface::new();
    iskra_drawing::paint_stroke(&mut painted, &back, SwatchId(1));
    let end = Instant::now();
    println!(
        "pen lifted: to JSON and back {:.0} ms ({:.1} MB of it), the stroke painted anew {:.0} ms",
        ms(sent - start),
        text_len as f64 / 1e6,
        ms(end - sent)
    );
}
