//! Times how long a drawing takes to be bent by a deformer (plan §4.5): a square that is all
//! painted, of several sizes, under a chain of bones, a curve and an envelope, each posed.
//! The mesh is the deformer's own work, the same for any size; the rest is the pixels.
//!
//! ```text
//! cargo run --release -p iskra-render --example bend_timing
//! ```

use std::time::Instant;

use iskra_drawing::Rect;
use iskra_model::{ArtContent, Bone, Deformer, DeformerId, Drawing, Project, SwatchId};
use iskra_render::{Mesh, bend};

/// A drawing that is painted all over a square of `side` pixels about its middle.
fn painted(side: i32) -> (Drawing, Rect) {
    let (mut drawing, rect) = (Drawing::default(), Rect::new(-side / 2, -side / 2, side / 2, side / 2));
    if let ArtContent::Indexed(surface) = &mut drawing.art_layers[1].content {
        // A band at a time, so that the coverage that is handed over stays small.
        for top in (rect.y0..rect.y1).step_by(256) {
            let band = Rect::new(rect.x0, top, rect.x1, (top + 256).min(rect.y1));
            surface.paint(band, &vec![1.0; band.width() as usize * band.height() as usize], SwatchId(3));
        }
    }
    (drawing, rect)
}

/// `count` points round a circle of `radius`, or along a line of that half length.
fn points(count: usize, radius: f64, round: bool) -> Vec<[f64; 2]> {
    let at = |i: usize| {
        let part = i as f64 / if round { count as f64 } else { (count - 1) as f64 };
        if round { [radius * (part * std::f64::consts::TAU).cos(), radius * (part * std::f64::consts::TAU).sin()] } else { [radius * (2.0 * part - 1.0), 0.0] }
    };
    (0..count).map(at).collect()
}

fn main() {
    let project = Project::default();
    println!("{:<28}{:>8}{:>12}{:>12}", "", "side", "mesh, ms", "all, ms");
    for side in [1000, 2000, 4000, 7000] {
        let (drawing, rect) = painted(side);
        let reach = f64::from(side) * 0.4;
        let mut deformers = Vec::new();
        let mut arm = Deformer::new(DeformerId(1), "", [-reach, 0.0], (0..3).map(|_| Bone::at_rest(0.0, reach * 2.0 / 3.0)).collect());
        (arm.bones[1].turn, arm.bones[2].turn) = (30.0, -20.0);
        deformers.push(("a chain of 3 bones".to_string(), arm));
        for count in [4, 16] {
            let mut tail = Deformer::curve(DeformerId(1), "", &points(count, reach, false));
            tail.pose_knot(count - 1, 0, [reach * 0.8, reach * 0.5]);
            deformers.push((format!("a curve of {count} points"), tail));
        }
        for count in [8, 32] {
            let rest = points(count, reach, true);
            let mut belly = Deformer::envelope(DeformerId(1), "", &rest);
            belly.pose_knot(0, 0, [rest[0][0] * 1.3, rest[0][1]]);
            deformers.push((format!("an envelope of {count} points"), belly));
        }
        for (name, deformer) in deformers {
            let warp = deformer.warp(0, 1.0);
            let start = Instant::now();
            let mesh = Mesh::of(rect.inflate(1), &warp);
            let meshed = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let bent = bend(&drawing, rect, &project.palette, &warp);
            let all = start.elapsed().as_secs_f64() * 1000.0;
            assert!(mesh.corners.len() > 4 && !bent.pixels.is_empty());
            println!("{name:<28}{side:>8}{meshed:>12.1}{all:>12.0}");
        }
    }
}
