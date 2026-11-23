//! Times the Particles node over a picture of 1920 by 1080, by the reference and by the GPU
//! compositor, which has the reference work the particles out and lays them on the card
//! (plan §8.4, *Particles*, step 6):
//!
//! ```text
//! cargo run --release -p iskra-gpu --example particles_card
//! ```

use std::sync::Arc;
use std::time::Instant;

use iskra_gpu::Compositor;
use iskra_model::{COLLIDER, Effect, EffectId, PARTICLES, SPRITE, Source, Value, WorkingSpace, effect_kind};
use iskra_render::View;

#[path = "../../render/tests/common/mod.rs"]
#[allow(dead_code)]
mod common;

const FRAMES: u32 = 12;
/// The scene is 320 by 180: six times that is 1920 by 1080.
const SCALE: f64 = 6.0;

fn main() {
    let Some(mut compositor) = Compositor::headless() else { return println!("no graphics card here") };
    let cases: [(&str, f64, f64, bool); 10] = [
        ("", 0.0, 0.0, false),
        ("Rain", 400.0, 0.0, false),
        ("Rain", 4000.0, 0.0, false),
        ("Rain", 4000.0, 300.0, false),
        ("Snow", 400.0, 0.0, false),
        ("Snow", 4000.0, 0.0, false),
        ("Wind", 400.0, 0.0, false),
        ("Speed Lines", 400.0, 0.0, false),
        ("Snow", 400.0, 0.0, true),
        ("Burst", 400.0, 0.0, false),
    ];
    for (kind, count, range, sprites) in cases {
        let (mut project, ball, _) = common::scene();
        let id = EffectId(project.new_id());
        let shot = Arc::make_mut(&mut project.shots[0]);
        if !kind.is_empty() {
            let mut node = Effect::new(id, kind, effect_kind(PARTICLES).expect("the kind is known"));
            for (param, value) in [("kind", Value::Choice(kind.to_string())), ("count", Value::Number(count)), ("depthRange", Value::Number(range))] {
                node.param_mut(param).expect("it has it").value = value;
            }
            // A burst bounces off the ball, stepped frame by frame.
            if kind == "Burst" {
                node.param_mut("repeat").expect("it has it").value = Value::Number(4.0);
                node.inputs.insert(COLLIDER.to_string(), Source::Layer(ball));
            }
            if sprites {
                node.inputs.insert(SPRITE.to_string(), Source::Layer(ball));
                node.param_mut("spriteScale").expect("it has it").value = Value::Number(20.0);
            }
            Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Effect(id));
            shot.effects.push(Arc::new(node));
        }
        let view = View::frame(&project.settings, SCALE);
        let (shot, palette) = (&project.shots[0], &project.palette);
        let start = Instant::now();
        for frame in 0..FRAMES {
            drop(iskra_render::render(shot, palette, WorkingSpace::Srgb, frame, &view));
        }
        let reference = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
        let first = compositor.render(shot, palette, WorkingSpace::Srgb, 0, &view);
        drop(compositor.read_shown(&compositor.shown(&first)));
        let start = Instant::now();
        for frame in 0..FRAMES {
            let made = compositor.render(shot, palette, WorkingSpace::Srgb, frame, &view);
            drop(compositor.read_shown(&compositor.shown(&made)));
        }
        let card = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
        let what = match (kind, sprites) {
            ("", _) => "no particles".to_string(),
            (_, true) => format!("{kind}, {count} sprites"),
            ("Burst", _) => format!("{kind}, {count} bouncing"),
            _ if range > 0.0 => format!("{kind}, {count} in depth"),
            _ => format!("{kind}, {count}"),
        };
        println!("{what:<24} {reference:>5.0} ms a frame by the reference, {card:>5.0} ms by the GPU compositor");
        // Stepped, a frame far on is stepped to from the start the first time it is wanted.
        if kind == "Burst" {
            let start = Instant::now();
            drop(compositor.render(shot, palette, WorkingSpace::Srgb, 200, &view));
            println!("{:<24} {:>5.0} ms to frame 200 the first time", "", start.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
