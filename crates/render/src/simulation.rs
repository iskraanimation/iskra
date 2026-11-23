//! A burst that bounces off a shape (plan §8.4, *Particles*, the stepped simulation): where
//! something is wired into a Particles node's collider port, its burst is no longer worked out
//! from the frame alone but stepped, frame by frame from its start, its particles bouncing off
//! wherever the collider is painted as the camera sees it on each frame.
//!
//! Any frame still comes out the same however it is reached. A run of the simulation is kept,
//! with its particles every few frames and on the last frame it reached, and with what each
//! frame of it was made of: the node as set on it, what the collider is made of there and how
//! the camera sees the node. A frame is stepped to from the nearest frame kept before it that
//! is still made of the same; a change to the collider on a frame lets go of the run from that
//! frame on, and no sooner.
//!
//! Between collisions a particle goes as [`crate::particles`]'s burst throws it, by the same
//! exact formula over each step, so a collider that is never met changes nothing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use iskra_drawing::{Affine, Point};
use iskra_eval::Card;
use iskra_model::{COLLIDER, Camera, Effect, EffectId, Layer, Palette, Peg, Shot, ShotId, Value, WorkingSpace};

use crate::cache::Part;
use crate::particles::{BURST_THROW, hashed, spread, thrown};
use crate::{View, draw};

/// A particle of a stepped burst on a frame: of which throw, which of it, where it is in the
/// scene at its depth, and how fast it goes there, the wind left out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Live {
    pub(crate) round: u32,
    pub(crate) i: u32,
    pub(crate) at: [f64; 2],
    pub(crate) going: [f64; 2],
}

/// How many steps a frame is cut into, so that a fast particle does not pass through a thin
/// shape.
const STEPS: u32 = 4;
/// Every how many frames a run keeps its particles, beside the last frame it reached.
const KEPT_EVERY: i64 = 8;
/// How many runs are kept at once, the ones used longest ago let go first.
const RUNS: usize = 16;
/// How large the collider is drawn to be bounced off: the frame, at most this tall.
const MASK_HEIGHT: f64 = 540.0;
/// How far, in the collider's pixels, a particle the shape has come over is pushed out at most.
const PUSH: u32 = 48;

/// What a frame of a run was made of.
#[derive(PartialEq)]
struct Made {
    values: Vec<Option<Value>>,
    collider: Vec<Part>,
    camera: Vec<Option<Affine>>,
}

/// What of the shot a run was last found to be right for, held so that the addresses are not
/// used again: while all of it is the very same, so is every frame of the run.
struct Held {
    layers: Vec<Arc<Layer>>,
    pegs: Vec<Arc<Peg>>,
    composites: Vec<Arc<iskra_model::Composite>>,
    deformers: Vec<Arc<iskra_model::Deformer>>,
    groups: Vec<Arc<iskra_model::Group>>,
    effects: Vec<Arc<Effect>>,
    camera: Camera,
    display: Option<iskra_model::Source>,
    size: [f64; 2],
}

impl Held {
    fn of(shot: &Shot, size: [f64; 2]) -> Held {
        Held {
            layers: shot.layers.clone(),
            pegs: shot.pegs.clone(),
            composites: shot.composites.clone(),
            deformers: shot.deformers.clone(),
            groups: shot.groups.clone(),
            effects: shot.effects.clone(),
            camera: shot.camera.clone(),
            display: shot.display,
            size,
        }
    }

    fn holds(&self, shot: &Shot, size: [f64; 2]) -> bool {
        fn same<T>(a: &[Arc<T>], b: &[Arc<T>]) -> bool {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| Arc::ptr_eq(a, b))
        }
        same(&self.layers, &shot.layers)
            && same(&self.pegs, &shot.pegs)
            && same(&self.composites, &shot.composites)
            && same(&self.deformers, &shot.deformers)
            && same(&self.groups, &shot.groups)
            && same(&self.effects, &shot.effects)
            && self.camera == shot.camera
            && self.display == shot.display
            && self.size == size
    }
}

/// A run of one node's burst: from the frame before its first, `first - 1`, where nothing is
/// yet; what each frame since was made of; and its particles on some of them.
struct Run {
    first: i64,
    made: Vec<Made>,
    kept: HashMap<i64, Arc<Vec<Live>>>,
    held: Option<Held>,
    used: u64,
}

#[derive(Default)]
struct Runs {
    runs: HashMap<(ShotId, EffectId), Arc<Mutex<Run>>>,
    clock: u64,
}

static RUNS_KEPT: Mutex<Option<Runs>> = Mutex::new(None);

/// The collider as the camera sees it on a frame: how solid it is, in its own pixels, the
/// frame `scale` times as large, its middle at the picture's.
struct Mask {
    width: u32,
    height: u32,
    scale: f64,
    solid: Vec<u8>,
}

impl Mask {
    fn view(size: [f64; 2]) -> View {
        let scale = (MASK_HEIGHT / size[1].max(1.0)).min(1.0);
        let (width, height) = ((size[0] * scale).round().max(1.0), (size[1] * scale).round().max(1.0));
        View { width: width as u32, height: height as u32, from_frame: Affine::translate(width / 2.0, height / 2.0).mul(&Affine::scale(scale)), frame: size }
    }

    /// How solid it is at a pixel of its own: nothing off it.
    fn at(&self, x: i64, y: i64) -> f64 {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return 0.0;
        }
        f64::from(self.solid[(y * i64::from(self.width) + x) as usize]) / 255.0
    }

    /// Its pixel at a point of the frame.
    fn pixel(&self, at: Point) -> (i64, i64) {
        let (x, y) = ((at.x * self.scale + f64::from(self.width) / 2.0).floor(), (at.y * self.scale + f64::from(self.height) / 2.0).floor());
        (x as i64, y as i64)
    }

    fn hits(&self, at: Point) -> bool {
        let (x, y) = self.pixel(at);
        self.at(x, y) > 0.5
    }

    /// Which way is out of it at a point of the frame, in the frame's units: down the slope of
    /// how solid it is round there. `None` where it is as solid all round.
    fn out(&self, at: Point) -> Option<[f64; 2]> {
        let (x, y) = self.pixel(at);
        let across = self.at(x - 2, y) - self.at(x + 2, y);
        let down = self.at(x, y - 2) - self.at(x, y + 2);
        let long = across.hypot(down);
        (long > 1e-6).then(|| [across / long, down / long])
    }
}

/// The card `effect` makes, wherever it is among `cards`.
fn find(cards: &[Card], effect: EffectId) -> Option<&Card> {
    cards.iter().find_map(|card| match card {
        Card::Through { effect: id, .. } if *id == effect => Some(card),
        Card::Through { inside, mattes, .. } => find(inside, effect).or_else(|| mattes.iter().find_map(|(_, cards)| find(cards, effect))),
        Card::Flat { inside, .. } => find(inside, effect),
        Card::Layer(_) => None,
    })
}

/// The particles of `effect`'s burst on the held frame `time`, stepped from its start, where
/// a collider is wired into it; `None` where none is, or it is no burst. `size` is the shot's
/// frame, in its own pixels.
pub(crate) fn stepped(shot: &Shot, palette: &Palette, space: WorkingSpace, effect: &Effect, time: u32, size: [f64; 2]) -> Option<Arc<Vec<Live>>> {
    if effect.kind != iskra_model::PARTICLES || effect.choice("kind", time) != "Burst" || !effect.inputs.contains_key(COLLIDER) {
        return None;
    }
    let first = effect.number("start", time).max(0.0).floor() as i64;
    let time = i64::from(time);
    if time < first {
        return Some(Arc::new(Vec::new()));
    }
    let run = {
        let mut kept = RUNS_KEPT.lock().unwrap_or_else(PoisonError::into_inner);
        let runs = kept.get_or_insert_with(Runs::default);
        runs.clock += 1;
        let now = runs.clock;
        let run = runs
            .runs
            .entry((shot.id, effect.id))
            .or_insert_with(|| Arc::new(Mutex::new(Run { first, made: Vec::new(), kept: HashMap::new(), held: None, used: now })))
            .clone();
        run.lock().unwrap_or_else(PoisonError::into_inner).used = now;
        if runs.runs.len() > RUNS {
            let oldest = runs.runs.iter().min_by_key(|(_, run)| run.lock().unwrap_or_else(PoisonError::into_inner).used).map(|(at, _)| *at);
            runs.runs.retain(|at, _| Some(*at) != oldest || *at == (shot.id, effect.id));
        }
        run
    };
    let mut locked = run.lock().unwrap_or_else(PoisonError::into_inner);
    let run = &mut *locked;
    let mask_view = Mask::view(size);
    // What a frame is made of, and the collider's cards on it.
    let made = |frame: i64| -> (Made, Vec<Card>) {
        let at = frame as u32;
        let evaluated = iskra_eval::evaluate(shot, at);
        let wired = find(&evaluated.cards, effect.id).and_then(|card| match card {
            Card::Through { mattes, .. } => mattes.iter().find(|(port, _)| port == COLLIDER).map(|(_, cards)| cards.clone()),
            _ => None,
        });
        let cards = wired.unwrap_or_default();
        let near = effect.number(iskra_model::DEPTH, at);
        let far = near - effect.number("depthRange", at).max(0.0) - 1.0;
        let values = effect.described().map(|kind| kind.params.iter().map(|param| effect.value(param.id, at)).collect()).unwrap_or_default();
        let collider = crate::cache::parts(0, palette, space, shot, at, &mask_view, &cards);
        let camera = [(near, at), (far, at), (near, 0)].map(|(depth, at)| shot.seen(depth, at)).to_vec();
        (Made { values, collider, camera }, cards)
    };
    // Where it begins again, or what of it no longer holds.
    if run.first != first {
        *run = Run { first, made: Vec::new(), kept: HashMap::new(), held: None, used: run.used };
    }
    // Where the shot is not the very same, each frame is looked at up to the one wanted, and
    // the run let go from the first that is made of something else; past the one wanted it may
    // hold or not, and is let go too. What is left holds for the shot as it is.
    if !run.held.as_ref().is_some_and(|held| held.holds(shot, size)) {
        let reached = (run.first + run.made.len() as i64).min(time + 1);
        let mut good = run.first;
        while good < reached && made(good).0 == run.made[(good - run.first) as usize] {
            good += 1;
        }
        run.made.truncate((good - run.first) as usize);
        run.kept.retain(|frame, _| *frame < good);
        run.held = Some(Held::of(shot, size));
    }
    if let Some(found) = run.kept.get(&time) {
        return Some(found.clone());
    }
    // From the nearest frame kept before it, or from nothing.
    let from = run.kept.keys().copied().filter(|frame| *frame < time && *frame < run.first + run.made.len() as i64).max();
    let (mut frame, mut live) = match from {
        Some(frame) => (frame, run.kept[&frame].as_ref().clone()),
        None => (run.first - 1, Vec::new()),
    };
    while frame < time {
        frame += 1;
        let (what, cards) = made(frame);
        let index = (frame - run.first) as usize;
        if index < run.made.len() {
            run.made[index] = what;
        } else {
            run.made.push(what);
        }
        let mut mask = vec![[0.0f32; 4]; mask_view.width as usize * mask_view.height as usize];
        draw(shot, palette, space, frame as u32, &mask_view, &cards, &mut mask, None);
        let mask = Mask {
            width: mask_view.width,
            height: mask_view.height,
            scale: mask_view.from_frame.m[0],
            solid: mask.iter().map(|pixel| (pixel[3].clamp(0.0, 1.0) * 255.0).round() as u8).collect(),
        };
        live = step(shot, effect, frame as u32, size, &mask, live);
        if (frame - run.first) % KEPT_EVERY == 0 || frame == time {
            run.kept.insert(frame, Arc::new(live.clone()));
        }
    }
    // Only the frames every few, and the last one reached.
    let last = run.first + run.made.len() as i64 - 1;
    let first = run.first;
    run.kept.retain(|kept, _| (kept - first) % KEPT_EVERY == 0 || *kept == time || *kept == last);
    run.kept.get(&time).cloned()
}

/// The burst's particles on `frame`, from what they were on the frame before: each that is
/// still about goes on for a frame, and each thrown since is thrown, bouncing off `mask`.
fn step(shot: &Shot, effect: &Effect, frame: u32, size: [f64; 2], mask: &Mask, before: Vec<Live>) -> Vec<Live> {
    let number = |id: &str| effect.number(id, frame);
    let seed = number("seed").max(0.0) as u32;
    let count = number("count").max(0.0) as u32;
    let pace = number("speed") / 100.0 * size[1] * BURST_THROW;
    let variation = number("speedVariation") / 100.0;
    let (wind, far, depth, range) = (number("wind"), number("far") / 100.0, number(iskra_model::DEPTH), number("depthRange").max(0.0));
    let (angle, spread_by, first_start, over, life) = (number("angle"), number("spread"), number("start"), number("over").max(1.0), number("life").max(1.0));
    let (gravity, kept, repeat) = (number("gravity"), 1.0 - number("drag") / 100.0, number("repeat"));
    let (bounce, friction) = ((number("bounce") / 100.0).clamp(0.0, 1.0), (number("friction") / 100.0).clamp(0.0, 1.0));
    let Some(centre) = shot.seen(depth, 0).and_then(|first| first.inverse()).map(|home| home.apply(Point::new(number("centreX"), number("centreY")))) else {
        return Vec::new();
    };
    let now = f64::from(frame);
    // The throws, and when and how each of their particles is thrown.
    let throw = |round: u32| match repeat >= 1.0 {
        true => (first_start + f64::from(round) * repeat, seed ^ round.wrapping_mul(0x2545_F491)),
        false => (first_start, seed),
    };
    let about = |round: u32, i: u32| {
        let (start, seed) = throw(round);
        let z = hashed(seed, i, 1);
        let (at_depth, near) = if range > 0.0 { (depth - range * z, 1.0) } else { (depth, 1.0 - far * z) };
        let lives = life * (1.0 + 0.3 * spread(seed, i, 12));
        (start + (over - 1.0) * hashed(seed, i, 11), at_depth, near, lives, seed)
    };
    let mut after = Vec::with_capacity(before.len());
    let mut go = |mut one: Live, from: f64| {
        let (born, at_depth, near, lives, _) = about(one.round, one.i);
        if now - born >= lives {
            return;
        }
        let Some(seen) = shot.seen(at_depth, frame) else { return };
        // A step of the collider's pixel, in the scene there.
        let pixel = 1.0 / (mask.scale * seen.determinant().abs().sqrt()).max(1e-9);
        let steps = ((now - from) * f64::from(STEPS)).ceil().max(1.0);
        let short = (now - from) / steps;
        for _ in 0..steps as u32 {
            let was = one.at;
            let (gone, going) = thrown(one.going, gravity * near, kept, short);
            one.at = [one.at[0] + gone[0] + wind * near * short, one.at[1] + gone[1]];
            one.going = going;
            let here = seen.apply(Point::new(one.at[0], one.at[1]));
            if !mask.hits(here) {
                continue;
            }
            // Out of the shape, in the scene: the way out on the frame, brought back through
            // the camera; or straight back the way it came where the shape is solid all round.
            let out = mask.out(here).map(|[x, y]| {
                let [a, b, c, d, ..] = seen.m;
                // A direction on the frame, as one across the scene: through the transpose.
                let (sx, sy) = (a * x + b * y, c * x + d * y);
                let long = sx.hypot(sy).max(1e-12);
                [sx / long, sy / long]
            });
            let out = out.unwrap_or_else(|| {
                let long = one.going[0].hypot(one.going[1]).max(1e-12);
                [-one.going[0] / long, -one.going[1] / long]
            });
            let into = one.going[0] * out[0] + one.going[1] * out[1];
            if into < 0.0 {
                // Bounced back by as much as it keeps, and slowed along the shape.
                let along = [one.going[0] - into * out[0], one.going[1] - into * out[1]];
                let kept_along = 1.0 - friction;
                one.going = [along[0] * kept_along - bounce * into * out[0], along[1] * kept_along - bounce * into * out[1]];
            }
            // Back where it was; and where the shape has come over that too, pushed out of it.
            one.at = was;
            let mut pushed = 0;
            while pushed < PUSH && mask.hits(seen.apply(Point::new(one.at[0], one.at[1]))) {
                one.at = [one.at[0] + out[0] * pixel, one.at[1] + out[1] * pixel];
                pushed += 1;
            }
        }
        after.push(one);
    };
    for one in before {
        go(one, now - 1.0);
    }
    // Those thrown since the frame before.
    let rounds = match repeat >= 1.0 {
        true => {
            let low = ((now - 1.0 - (over - 1.0) - first_start) / repeat).floor().max(0.0) as u32;
            let high = ((now - first_start) / repeat).floor();
            if high < 0.0 { 0..0 } else { low..high as u32 + 1 }
        }
        false => 0..1,
    };
    for round in rounds {
        for i in 0..count {
            let (born, _, near, _, seed) = about(round, i);
            if !(born > now - 1.0 && born <= now) {
                continue;
            }
            let way = (angle + spread_by * (hashed(seed, i, 13) - 0.5)).to_radians();
            let speed = pace * near * (1.0 + variation * spread(seed, i, 2));
            go(Live { round, i, at: [centre.x, centre.y], going: [way.cos() * speed, way.sin() * speed] }, born);
        }
    }
    after
}
