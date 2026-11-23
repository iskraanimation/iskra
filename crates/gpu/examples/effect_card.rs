//! Times a blur and a glow over the whole of a picture of 1920 by 1080, made by the reference
//! and made on the card, at several widths (plan §8.5 step 9):
//!
//! ```text
//! cargo run --release -p iskra-gpu --example effect_card
//! ```

use std::sync::Arc;
use std::time::Instant;

use iskra_gpu::Compositor;
use iskra_model::{Effect, EffectId, PICTURE, Source, Value, WorkingSpace, effect_kind};
use iskra_render::View;

#[path = "../../render/tests/common/mod.rs"]
#[allow(dead_code)]
mod common;

const FRAMES: u32 = 12;
/// The scene is 320 by 180: six times that is 1920 by 1080.
const SCALE: f64 = 6.0;

fn main() {
    let Some(mut compositor) = Compositor::headless() else { return println!("no graphics card here") };
    // With no kind, the picture as it is: what the rest of a frame takes.
    let cases = [("", 0.0), ("blur", 10.0), ("blur", 30.0), ("blur", 60.0), ("blur", 120.0), ("blur", 300.0), ("blur", 500.0), ("glow", 30.0)];
    for (kind, radius) in cases {
        let (mut project, ..) = common::scene();
        let id = EffectId(project.new_id());
        let shot = Arc::make_mut(&mut project.shots[0]);
        let Some(shown) = shot.display else { return };
        if let Some(described) = effect_kind(kind) {
            let mut node = Effect::new(id, kind, described);
            node.inputs.insert(PICTURE.to_string(), shown);
            // Said in the frame's pixels, of which the picture has six to each.
            node.param_mut("radius").expect("it has a radius").value = Value::Number(radius / SCALE);
            shot.effects.push(Arc::new(node));
            shot.display = Some(Source::Effect(id));
        }
        let view = View::frame(&project.settings, SCALE);
        let (shot, palette) = (&project.shots[0], &project.palette);
        let start = Instant::now();
        for frame in 0..FRAMES {
            drop(iskra_render::render(shot, palette, WorkingSpace::Srgb, frame, &view));
        }
        let reference = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
        // Once before the clock starts: the drawings go to the card.
        let first = compositor.render(shot, palette, WorkingSpace::Srgb, 0, &view);
        drop(compositor.read_shown(&compositor.shown(&first)));
        let before = compositor.effects;
        let start = Instant::now();
        for frame in 0..FRAMES {
            // Read back, so that the card has made the frame when the clock is read.
            let made = compositor.render(shot, palette, WorkingSpace::Srgb, frame, &view);
            drop(compositor.read_shown(&compositor.shown(&made)));
        }
        let card = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
        let what = if kind.is_empty() { "no effect".to_string() } else { format!("a {kind} of {radius} pixels") };
        let by = if compositor.effects.1 > before.1 { "by the card, the effect by the reference" } else { "on the card" };
        println!("{what} over 1920 by 1080: {reference:.0} ms a frame by the reference, {card:.0} ms {by}");
    }
}
