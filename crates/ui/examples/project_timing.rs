//! Times how a project file plays: opening it, and for every frame of each shot working out
//! where everything is, bending what bones bend, and drawing the frame on the processor and
//! on the graphics card. The file is only read.
//!
//! ```text
//! cargo run --release -p iskra-ui --example project_timing -- <project.iskra>
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use iskra_model::{ArtContent, WorkingSpace};
use iskra_render::View;

/// The least, the middle and the most of some timings, and all of them added up, in ms.
fn summary(times: &[Duration]) -> String {
    if times.is_empty() {
        return "-".to_string();
    }
    let mut ms: Vec<f64> = times.iter().map(|time| time.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let total: f64 = ms.iter().sum();
    format!("min {:7.2}  median {:7.2}  max {:7.2}  total {:8.1} ms", ms[0], ms[ms.len() / 2], ms[ms.len() - 1], total)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: project_timing <project.iskra>");
    let start = Instant::now();
    let (_store, project) = iskra_store::SqliteStore::open(path.as_ref()).expect("the project opens");
    let structure = start.elapsed();
    let start = Instant::now();
    let (_store, project_whole, report) = iskra_store::SqliteStore::open_whole(path.as_ref()).expect("the project loads");
    let whole = start.elapsed();
    println!(
        "open, structure only: {:.1} ms; with every drawing's pixels: {:.1} ms ({report:?})",
        structure.as_secs_f64() * 1000.0,
        whole.as_secs_f64() * 1000.0
    );
    drop(project);
    let project = project_whole;
    let settings = &project.settings;
    println!("frame {}x{}, {} fps, working space {:?}", settings.width, settings.height, settings.frame_rate.as_f64(), settings.working_space);

    let mut gpu = iskra_gpu::Compositor::headless();
    if gpu.is_none() {
        println!("no graphics card to time");
    }
    let palette = project.palette.clone();
    let space: WorkingSpace = settings.working_space;
    for shot in &project.shots {
        println!(
            "\n== shot `{}`: {} frames, {} layers, {} pegs, {} deformers, {} effects, {} composites",
            shot.name,
            shot.duration,
            shot.layers.len(),
            shot.pegs.len(),
            shot.deformers.len(),
            shot.effects.len(),
            shot.composites.len()
        );
        for layer in &shot.layers {
            let biggest = layer.drawings.iter().map(|pool| pool.drawing.bounds()).max_by_key(|rect| rect.width() as i64 * rect.height() as i64);
            let kinds: Vec<&str> = layer
                .drawings
                .iter()
                .flat_map(|pool| pool.drawing.art_layers.iter())
                .map(|art| match art.content {
                    ArtContent::Indexed(_) => "indexed",
                    ArtContent::Rgba(_) => "rgba",
                    ArtContent::Vector(_) => "vector",
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let bones = layer
                .deformer
                .and_then(|id| shot.deformer(id))
                .map(|bones| format!(", bent by `{}` ({}, {} bones, {} points)", bones.name, bones.kind().1, bones.bones.len(), bones.spine.len()));
            println!(
                "  layer `{}`: {} drawings, density {}, largest painted {:?}, art {:?}{}",
                layer.name,
                layer.drawings.len(),
                layer.density,
                biggest.map(|rect| (rect.width(), rect.height())),
                kinds,
                bones.unwrap_or_default()
            );
        }
        for peg in &shot.pegs {
            if peg.follows.is_some() || !peg.inherit.is_all() || !peg.animation.key_frames().is_empty() {
                println!("  peg `{}`: {} keyed frames, follows {:?}, inherits {:?}", peg.name, peg.animation.key_frames().len(), peg.follows, peg.inherit);
            }
        }
        for effect in &shot.effects {
            println!("  effect `{}` ({}), enabled {}", effect.name, effect.kind, effect.enabled);
        }
        let shot = Arc::new(shot.as_ref().clone());
        let frames = shot.duration.max(1);
        let (mut evaluated, mut bent, mut cpu, mut cached, mut card) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let view = View::frame(settings, 1.0);
        let cache = iskra_render::Cache::new(1 << 30);
        for frame in 0..frames {
            let start = Instant::now();
            let placed = iskra_eval::evaluate(&shot, frame);
            evaluated.push(start.elapsed());
            drop(placed);
            // Bending, alone: every bent layer's drawing on this frame.
            let start = Instant::now();
            for layer in shot.layers.iter() {
                let (Some(bones), Some(shown)) = (layer.deformer.and_then(|id| shot.deformer(id)), layer.drawing_at(frame)) else { continue };
                let warp = bones.warp(frame, layer.density);
                if !warp.is_still() {
                    let bounds = shown.drawing.bounds();
                    std::hint::black_box(iskra_render::bend(&shown.drawing, bounds, &palette, &warp));
                }
            }
            bent.push(start.elapsed());
            let start = Instant::now();
            std::hint::black_box(iskra_render::render(&shot, &palette, space, frame, &view));
            cpu.push(start.elapsed());
            let start = Instant::now();
            std::hint::black_box(cache.render(&shot, &palette, space, frame, &view));
            cached.push(start.elapsed());
            if let Some(gpu) = gpu.as_mut() {
                let start = Instant::now();
                let made = gpu.render(&shot, &palette, space, frame, &view);
                // Read back, so that the card has finished it.
                std::hint::black_box(gpu.read(&made));
                card.push(start.elapsed());
            }
        }
        // The cache a second time round, as when the shot plays again.
        let mut again = Vec::new();
        for frame in 0..frames {
            let start = Instant::now();
            std::hint::black_box(cache.render(&shot, &palette, space, frame, &view));
            again.push(start.elapsed());
        }
        let mut card_again = Vec::new();
        if let Some(gpu) = gpu.as_mut() {
            for frame in 0..frames {
                let start = Instant::now();
                let made = gpu.render(&shot, &palette, space, frame, &view);
                std::hint::black_box(gpu.read(&made));
                card_again.push(start.elapsed());
            }
        }
        println!("  a frame, over {frames} frames:");
        println!("    where everything is (eval) {}", summary(&evaluated));
        println!("    bending, alone             {}", summary(&bent));
        println!("    processor, whole render    {}", summary(&cpu));
        println!("    processor, with its cache  {}", summary(&cached));
        println!("      and played again         {}", summary(&again));
        println!("    graphics card, read back   {}", summary(&card));
        println!("      and played again         {}", summary(&card_again));
        println!("  ({} fps is {:.1} ms a frame)", settings.frame_rate.as_f64(), 1000.0 / settings.frame_rate.as_f64());
        // Where the time goes: the same frames with the effects switched off.
        if !shot.effects.is_empty() {
            let mut plain = shot.as_ref().clone();
            for effect in &mut plain.effects {
                Arc::make_mut(effect).enabled = false;
            }
            let plain = Arc::new(plain);
            let mut times = Vec::new();
            for frame in 0..frames {
                let start = Instant::now();
                std::hint::black_box(iskra_render::render(&plain, &palette, space, frame, &view));
                times.push(start.elapsed());
            }
            println!("    processor, effects off     {}", summary(&times));
            if let Some(gpu) = gpu.as_mut() {
                let mut times = Vec::new();
                for frame in 0..frames {
                    let start = Instant::now();
                    let made = gpu.render(&plain, &palette, space, frame, &view);
                    std::hint::black_box(gpu.read(&made));
                    times.push(start.elapsed());
                }
                println!("    graphics card, effects off {}", summary(&times));
            }
        }
        // On the card as a drag sees it, a step at a time on the first frame, by a part of a
        // pixel as a drag moves: the pegs at the top moved, which moves everything; one peg
        // moved, the first arm's; and the view made wider, as when the window is resized.
        if let Some(gpu) = gpu.as_mut() {
            let moved = |shot: &iskra_model::Shot, step: usize, which: &dyn Fn(&iskra_model::Peg) -> bool| {
                let mut moved = shot.clone();
                for peg in moved.pegs.iter_mut().filter(|peg| which(peg)) {
                    Arc::make_mut(peg).transform.position[0] += step as f64 * 0.37;
                }
                Arc::new(moved)
            };
            let arm = shot.pegs.iter().find(|peg| peg.name.contains("ARM")).map(|peg| peg.id);
            let cases: [(&str, &dyn Fn(&iskra_model::Peg) -> bool); 2] =
                [("the whole moved", &|peg| peg.parent.is_none()), ("one arm moved", &|peg| Some(peg.id) == arm)];
            for (title, which) in cases {
                let mut times = Vec::new();
                for step in 1..=20 {
                    let shot = moved(&shot, step, which);
                    let start = Instant::now();
                    let made = gpu.render(&shot, &palette, space, 0, &view);
                    std::hint::black_box(gpu.read(&made));
                    times.push(start.elapsed());
                }
                println!("    card, {title:<20} {}", summary(&times));
            }
            let mut times = Vec::new();
            for step in 1..=20 {
                let wider = View::frame(&iskra_model::Settings { width: settings.width + 8 * step, ..settings.clone() }, 1.0);
                let start = Instant::now();
                let made = gpu.render(&shot, &palette, space, 0, &wider);
                std::hint::black_box(gpu.read(&made));
                times.push(start.elapsed());
            }
            println!("    card, the view resized     {}", summary(&times));
        }
        // And one layer at a time, the costliest first.
        let mut each: Vec<(f64, String)> = Vec::new();
        for layer in &shot.layers {
            let mut alone = shot.as_ref().clone();
            alone.hidden.extend(shot.layers.iter().filter(|other| other.id != layer.id).map(|other| other.id.0));
            let start = Instant::now();
            for frame in (0..frames).step_by(4) {
                std::hint::black_box(iskra_render::render(&alone, &palette, space, frame, &view));
            }
            each.push((start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames.div_ceil(4)), layer.name.clone()));
        }
        each.sort_by(|a, b| b.0.total_cmp(&a.0));
        let listed: Vec<String> = each.iter().take(8).map(|(ms, name)| format!("{name} {ms:.1}")).collect();
        println!("    processor, a layer alone, ms: {}", listed.join(", "));
    }
}
