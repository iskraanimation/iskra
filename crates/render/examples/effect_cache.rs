//! Times a shot with a still background under a blur, with a ball moving over it, rendered
//! at 1920 by 1080 with and without the cache of effect pictures (plan §8.5 step 8):
//!
//! ```text
//! cargo run --release -p iskra-render --example effect_cache
//! ```

use std::sync::Arc;
use std::time::Instant;

use iskra_model::{Effect, EffectId, PICTURE, Source, Value, WorkingSpace, effect_kind};
use iskra_render::{Cache, View, render};

#[path = "../tests/common/mod.rs"]
#[allow(dead_code)]
mod common;

const FRAMES: u32 = 24;

fn main() {
    let (mut project, _, peg) = common::scene();
    let field = Source::Layer(project.shots[0].layers[2].id);
    let blur = EffectId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let mut node = Effect::new(blur, "Blur", effect_kind("blur").expect("the kind is known"));
    node.inputs.insert(PICTURE.to_string(), field);
    node.param_mut("radius").expect("a blur has a radius").value = Value::Number(20.0);
    shot.rewire(field, Some(Source::Effect(blur)));
    shot.effects.push(Arc::new(node));
    let bounce = Arc::make_mut(shot.pegs.iter_mut().find(|held| held.id == peg).expect("the scene has the peg"));
    bounce.animation.position.set(0, [40.0, 10.0]);
    bounce.animation.position.set(FRAMES, [-60.0, 10.0]);
    // The scene is 320 by 180: six times that is 1920 by 1080.
    let view = View::frame(&project.settings, 6.0);
    let timed = |what: &str, frame: &dyn Fn(u32)| {
        let start = Instant::now();
        (0..FRAMES).for_each(frame);
        let took = start.elapsed().as_secs_f64();
        println!("{what}: {took:.2} s for {FRAMES} frames, {:.0} ms a frame", took * 1000.0 / f64::from(FRAMES));
    };
    let shot = &project.shots[0];
    timed("a still background under a blur, without the cache", &|frame| drop(render(shot, &project.palette, WorkingSpace::Srgb, frame, &view)));
    let cache = Cache::default();
    timed("the same, with the cache", &|frame| drop(cache.render(shot, &project.palette, WorkingSpace::Srgb, frame, &view)));
    timed("the same, a second time round", &|frame| drop(cache.render(shot, &project.palette, WorkingSpace::Srgb, frame, &view)));
    println!("made {} and found {}", cache.counts().0, cache.counts().1);

    // A blur whose radius changes on every frame is never found: what the cache costs.
    let mut keyed = project.clone();
    let radius = Arc::make_mut(&mut keyed.shots[0]).effect_mut(blur).and_then(|node| node.param_mut("radius")).expect("a blur has a radius");
    radius.keys.set(0, 10.0);
    radius.keys.set(FRAMES, 30.0);
    let shot = &keyed.shots[0];
    timed("a blur that changes on every frame, without the cache", &|frame| drop(render(shot, &keyed.palette, WorkingSpace::Srgb, frame, &view)));
    let cache = Cache::default();
    timed("the same, with the cache", &|frame| drop(cache.render(shot, &keyed.palette, WorkingSpace::Srgb, frame, &view)));
}
