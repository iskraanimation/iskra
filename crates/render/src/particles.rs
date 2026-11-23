//! The Particles node's picture (plan §8.4, *Particles*): rain, snow and speed lines, made
//! from the frame alone. Each particle's place comes straight from its number, the seed and the frame, with
//! nothing carried from one frame to the next, so any frame is the same however it is
//! reached: scrubbed to, rendered alone in the queue, or found in the cache.
//!
//! The particles lie over the frame and a margin round it, and what falls out of one side
//! comes in at the other. Speed lines are drawn anew on every frame that is not held, from
//! the frame's edge in, so that they boil as drawn ones do. Sizes and speeds are in the frame's pixels, and speeds are a frame's
//! worth of movement, so a shot looks the same at whatever size it is made.

use effectcraft_raster::Px;
use iskra_drawing::{Affine, Point};
use iskra_model::Effect;

use crate::Flat;
use crate::effects::paint;

/// A picture a particle can be drawn as: a drawing wired into the node's sprite port, with
/// how many of its pixels a pixel of the frame is at its drawn size.
pub(crate) struct Sprite {
    pub(crate) flat: Flat,
    pub(crate) per_unit: f64,
}

/// A particle as it is drawn: a mark, or a sprite placed by a transform from its pixels to
/// the picture's, as solid as `alpha`.
enum Drawn {
    Mark(Mark),
    Sprite {
        index: usize,
        to_picture: Affine,
        alpha: f32,
    },
    /// One of the splash's own sprites, as a droplet.
    Splash {
        index: usize,
        to_picture: Affine,
        alpha: f32,
    },
}

/// How far a drop falls in a frame at a speed of 100%, as a part of the frame's height: it
/// crosses the frame in some 18 frames.
const RAIN_FALL: f64 = 1.0 / 18.0;
/// The same for a flake, which takes some 6 seconds at 24 frames a second.
const SNOW_FALL: f64 = 1.0 / 150.0;

/// A number from 0 to 1 that particle `i` has for its `what`, the same for every frame.
pub(crate) fn hashed(seed: u32, i: u32, what: u32) -> f64 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ i.wrapping_mul(0x85EB_CA6B) ^ what.wrapping_mul(0xC2B2_AE35);
    // A few rounds of mixing, as integer hashes have them.
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    f64::from(x) / (f64::from(u32::MAX) + 1.0)
}

/// The same, from -1 to 1.
pub(crate) fn spread(seed: u32, i: u32, what: u32) -> f64 {
    hashed(seed, i, what) * 2.0 - 1.0
}

/// `value` brought into `[low, low + span)`, as round a cylinder.
fn wrapped(value: f64, low: f64, span: f64) -> f64 {
    if span <= 0.0 { value } else { low + (value - low).rem_euclid(span) }
}

/// One particle as it is drawn, in the picture's pixels: a line from `from` to `to`, as wide
/// as `radius` each side at `from` and `taper` times that at `to`, soft at its edge by `soft`,
/// of `colour` as much as `alpha`.
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    pub from: Point,
    pub to: Point,
    pub radius: f64,
    pub taper: f64,
    pub soft: f64,
    pub alpha: f32,
}

impl Mark {
    /// How far from its line it covers anything: as far as its widest end reaches.
    pub fn reach(&self) -> f64 {
        self.radius.max(0.5) + self.soft.max(1e-6) / 2.0
    }
}

/// A picture a particle is drawn as, colour times alpha in rows from the top, and where its
/// pixels lie in its own: from `bounds[0]`, `bounds[1]` to before `bounds[2]`, `bounds[3]`.
pub struct Picture {
    pub bounds: [i32; 4],
    pub pixels: Vec<Px>,
}

/// A particle as it is laid: a mark, or one of the pictures placed by a transform from its
/// pixels to the picture's, as solid as `alpha`.
pub enum Particle {
    Mark(Mark),
    Sprite { picture: usize, to_picture: Affine, alpha: f32 },
}

/// A Particles node's particles on a frame, worked out and not yet laid onto a picture, in
/// the order they are laid: what the GPU compositor draws on the card (plan §8.4,
/// *Particles*, step 6). The marks are of `colour`; a sprite takes as much of `tint`'s colour
/// as its second number says, as solid as it is.
pub struct Particles {
    pub colour: Px,
    pub tint: (Px, f32),
    pub pictures: Vec<Picture>,
    pub drawn: Vec<Particle>,
}

/// Where a Particles node's picture is drawn: the shot's frame, in its own pixels; how many of
/// the picture's pixels a pixel of the frame is; what takes the frame's units, its middle at
/// the origin, to the picture's; and how the camera sees the scene at a depth on a frame,
/// from the scene to the frame's units, `None` where it can't.
pub(crate) struct Place<'a> {
    pub(crate) size: [f64; 2],
    pub(crate) scale: f64,
    pub(crate) from_frame: &'a Affine,
    pub(crate) camera: &'a dyn Fn(f64, u32) -> Option<Affine>,
    /// Of rain or snow in sheets, which of how many is drawn: its own particles alone, all at
    /// its depth. `None` for all of them, at their own depths.
    pub(crate) plane: Option<(u32, u32)>,
    /// A burst's particles where they are stepped to the frame, bouncing off a collider, in
    /// place of where they are thrown to.
    pub(crate) stepped: Option<&'a [crate::simulation::Live]>,
}

/// What falls or blows: each moves through the scene, seen through the camera.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fall {
    Rain,
    Snow,
    Wind,
}

/// How far a streak of wind goes in a frame at a speed of 100%, as a part of the frame's
/// width, and about how many frames one lasts.
const WIND_BLOW: f64 = 1.0 / 12.0;
const WIND_LIFE: f64 = 24.0;

/// How many frames a splash lasts, and how many droplets a drop throws up as it lands.
const SPLASH_LIFE: f64 = 8.0;
const SPLASH_DROPS: u32 = 4;
/// How much faster a splash's droplets fall each frame, in pixels a frame, with no pull.
const SPLASH_GRAVITY: f64 = 0.5;

/// How fast a burst throws its particles at a speed of 100%, as a part of the frame's height
/// a frame.
pub(crate) const BURST_THROW: f64 = 1.0 / 30.0;

/// How many throws of a burst that repeats are about at once, at most.
const MOST_THROWS: usize = 64;

/// How many times at most a particle's tile is repeated each way into what the camera sees,
/// where the camera looks at far more than its first frame did.
const COPIES: i64 = 16;

/// The frame `effect`'s particles are as on `frame`: held on twos or threes, the particles
/// move, or the lines are drawn anew, only every second or third frame.
pub(crate) fn held(effect: &Effect, frame: u32) -> u32 {
    let hold = match effect.choice("hold", frame).as_str() {
        "On Twos" => 2,
        "On Threes" => 3,
        _ => 1,
    };
    frame - frame % hold
}

/// The particles of `effect` on `frame`, drawn onto a picture `width` by `height` pixels at
/// `place`.
pub(crate) fn draw(
    effect: &Effect,
    frame: u32,
    (width, height): (u32, u32),
    place: &Place,
    space: iskra_model::WorkingSpace,
    sprites: &[Sprite],
    (splash_sprites, splash_play): (&[Sprite], &[Option<usize>]),
) -> Vec<Px> {
    let (colour, tint, drawn) = scatter(effect, frame, place, space, sprites, (splash_sprites, splash_play));
    let mut picture = vec![[0.0f32; 4]; width as usize * height as usize];
    for one in &drawn {
        match one {
            Drawn::Mark(mark) => lay(&mut picture, (width, height), mark, colour),
            Drawn::Sprite { index, to_picture, alpha } => stamp(&mut picture, (width, height), &sprites[*index], to_picture, *alpha, tint),
            Drawn::Splash { index, to_picture, alpha } => stamp(&mut picture, (width, height), &splash_sprites[*index], to_picture, *alpha, tint),
        }
    }
    picture
}

/// The particles of `effect` on `frame` at `place`, worked out as [`draw`] lays them, for the
/// card to lay. The sprites are the pictures, then the splash's.
pub(crate) fn particles(
    effect: &Effect,
    frame: u32,
    place: &Place,
    space: iskra_model::WorkingSpace,
    sprites: Vec<Sprite>,
    (splash_sprites, splash_play): (Vec<Sprite>, &[Option<usize>]),
) -> Particles {
    let (colour, tint, drawn) = scatter(effect, frame, place, space, &sprites, (&splash_sprites, splash_play));
    let first = sprites.len();
    let drawn = drawn
        .into_iter()
        .map(|one| match one {
            Drawn::Mark(mark) => Particle::Mark(mark),
            Drawn::Sprite { index, to_picture, alpha } => Particle::Sprite { picture: index, to_picture, alpha },
            Drawn::Splash { index, to_picture, alpha } => Particle::Sprite { picture: first + index, to_picture, alpha },
        })
        .collect();
    let pictures = sprites
        .into_iter()
        .chain(splash_sprites)
        .map(|sprite| {
            let b = sprite.flat.bounds;
            Picture { bounds: [b.x0, b.y0, b.x1, b.y1], pixels: sprite.flat.pixels }
        })
        .collect();
    Particles { colour, tint, pictures, drawn }
}

/// The colour of the marks, the tint of the sprites, and every particle in the order it is
/// laid.
fn scatter(
    effect: &Effect,
    frame: u32,
    place: &Place,
    space: iskra_model::WorkingSpace,
    sprites: &[Sprite],
    (splash_sprites, splash_play): (&[Sprite], &[Option<usize>]),
) -> (Px, (Px, f32), Vec<Drawn>) {
    let time = held(effect, frame);
    let colour = paint(effect.colour("colour", frame), space);
    // How much a sprite takes of the node's colour, as solid as the sprite is.
    let [red, green, blue, _] = effect.colour("colour", frame);
    let tint = (paint([red, green, blue, 1.0], space), (effect.number("tint", frame) / 100.0).clamp(0.0, 1.0) as f32);
    let fall = |fall: Fall| falling(effect, frame, fall, f64::from(time), place, sprites, (splash_sprites, splash_play));
    let drawn = match effect.choice("kind", frame).as_str() {
        "Speed Lines" => speed_lines(effect, frame, time, place, sprites),
        "Snow" => fall(Fall::Snow),
        "Wind" => fall(Fall::Wind),
        "Burst" => burst(effect, frame, f64::from(time), place, sprites),
        _ => fall(Fall::Rain),
    };
    (colour, tint, drawn)
}

/// Rain, snow or wind: each particle where it has gone by `time`, in the scene at its depth,
/// seen through the camera. It wraps round a tile of the scene as large as what the camera
/// saw at that depth on the first frame, and is drawn wherever that tile repeats into what the
/// camera sees now, so that the camera can move over it. With `sprites`, each particle is one
/// of them, picked at random, in place of a streak or a flake.
fn falling(
    effect: &Effect,
    frame: u32,
    fall: Fall,
    time: f64,
    place: &Place,
    sprites: &[Sprite],
    (splash_sprites, splash_play): (&[Sprite], &[Option<usize>]),
) -> Vec<Drawn> {
    let number = |id: &str| effect.number(id, frame);
    let seed = number("seed").max(0.0) as u32;
    let count = number("count").max(0.0) as u32;
    let [frame_width, frame_height] = place.size;
    let pace = number("speed") / 100.0
        * match fall {
            Fall::Rain => frame_height * RAIN_FALL,
            Fall::Snow => frame_height * SNOW_FALL,
            Fall::Wind => frame_width * WIND_BLOW,
        };
    let (variation, size_variation) = (number("speedVariation") / 100.0, number("sizeVariation") / 100.0);
    let (wind, sway, length, size, far) = (number("wind"), number("sway"), number("length"), number("size"), number("far") / 100.0);
    let (depth, range, angle) = (number(iskra_model::DEPTH), number("depthRange").max(0.0), number("angle").to_radians());
    let (sprite_scale, turn, spin) = (number("spriteScale") / 100.0, number("turn") / 100.0, number("spin"));
    // Rain and snow fall faster by their pull, at a steady speed as real rain does; a splash's
    // droplets fall back by as much more than they did without.
    let pull = if fall == Fall::Wind { 0.0 } else { number("pull") };
    let (splash, ground, gravity) = (fall == Fall::Rain && effect.switch("splash", frame), number("ground"), SPLASH_GRAVITY + pull.max(0.0) / 10.0);
    let splash_scale = number("splashScale") / 100.0;
    // Out of an emitter rather than over the whole frame: from its Start Frame, each particle
    // is born at a place of its own in it, goes until it is past the frame, and is born again.
    let emitter = effect.choice("emitter", frame);
    let emitting = emitter != "Whole Frame";
    let (emit_width, emit_height) = match emitter.as_str() {
        "Area" => (number("emitterWidth"), number("emitterHeight")),
        "Line" => (number("emitterWidth"), 0.0),
        _ => (0.0, 0.0),
    };
    let (start, preroll) = (number("start"), number("preroll").max(0.0));
    // Where it is in the scene, at the node's depth as the camera saw it on the first frame,
    // and how many of the frame's pixels a unit of the scene is there.
    let at_node = (place.camera)(depth, 0);
    let node_scale = at_node.map_or(1.0, |seen| seen.determinant().abs().sqrt().max(1e-9));
    let Some(emit_middle) = at_node.and_then(|seen| seen.inverse()).map(|home| home.apply(Point::new(number("emitterX"), number("emitterY")))) else {
        return Vec::new();
    };
    // A splash that plays its drawings lasts as long as they play.
    let splash_life = if splash_play.is_empty() { SPLASH_LIFE } else { splash_play.len() as f64 };
    // How far the largest sprite reaches from its origin, in the scene's units at its drawn
    // size, so that it wraps round wholly out of sight.
    let sprite_reach = sprites
        .iter()
        .map(|sprite| {
            let b = sprite.flat.bounds;
            [b.x0, b.x1].into_iter().chain([b.y0, b.y1]).map(|side| f64::from(side.abs())).fold(0.0, f64::max) * std::f64::consts::SQRT_2 / sprite.per_unit
        })
        .fold(0.0, f64::max);
    let mut drawn = Vec::new();
    for i in 0..count {
        let mut z = hashed(seed, i, 1);
        // In sheets, each particle is in one of them, at its depth, and drawn with it alone.
        if let (Some((plane, planes)), Fall::Rain | Fall::Snow) = (place.plane, fall) {
            let sheet = ((hashed(seed, i, 15) * f64::from(planes)) as u32).min(planes - 1);
            if sheet != plane {
                continue;
            }
            z = f64::from(sheet) / f64::from(planes.max(2) - 1);
        }
        // Spread in depth and seen through the camera; or all at the node's depth, the far
        // ones only made smaller, slower and fainter.
        let (at_depth, near, faint) = if range > 0.0 { (depth - range * z, 1.0, 1.0 - 0.6 * z) } else { (depth, 1.0 - far * z, 1.0 - far * z) };
        let (Some(now), Some(first)) = ((place.camera)(at_depth, frame), (place.camera)(at_depth, 0)) else { continue };
        // How many of the frame's pixels a unit of the scene is there, on the first frame and now.
        let (first_scale, now_scale) = (first.determinant().abs().sqrt(), now.determinant().abs().sqrt());
        let (Some(home), Some(back)) = (first.inverse(), now.inverse()) else { continue };
        let going = pace * near * (1.0 + variation * spread(seed, i, 2));
        let (vx, vy) = match fall {
            Fall::Wind => (angle.cos() * going + wind * near, angle.sin() * going),
            _ => (wind * near, going + pull * near),
        };
        let grown = near * (1.0 + size_variation * spread(seed, i, 3));
        let size = (size * grown).max(0.1);
        // What a drop or a gust leaves behind it: a streak as long as its way in `length`
        // frames, or ten times that for wind. Snow and sprites have none.
        let (back_x, back_y) = match fall {
            Fall::Rain if sprites.is_empty() => (vx * length, vy * length),
            Fall::Wind if sprites.is_empty() => (vx * length * 10.0, vy * length * 10.0),
            _ => (0.0, 0.0),
        };
        let margin = back_x.abs().max(back_y.abs()) + size.max(sprite_reach * sprite_scale * grown) + sway + 2.0;
        let middle = home.apply(Point::new(0.0, 0.0));
        let (span_x, span_y) = (frame_width / first_scale + 2.0 * margin, frame_height / first_scale + 2.0 * margin);
        let (left, top) = (middle.x - span_x / 2.0, middle.y - span_y / 2.0);
        // Where it is on the held frame. A streak of wind is blown anew, somewhere else, every
        // time its life is over, fading in and out over it.
        let (x, y, fade) = match fall {
            Fall::Wind => {
                let life = WIND_LIFE * (1.0 + 0.5 * spread(seed, i, 5));
                let lived = time + hashed(seed, i, 6) * life;
                let round = (lived / life).floor();
                let age = lived - round * life;
                let anew = seed ^ (round as i64 as u32).wrapping_mul(0x2545_F491);
                (left + hashed(anew, i, 3) * span_x + vx * age, top + hashed(anew, i, 4) * span_y + vy * age, (std::f64::consts::PI * age / life).sin())
            }
            _ => (left + hashed(seed, i, 3) * span_x + vx * time, top + hashed(seed, i, 4) * span_y + vy * time, 1.0),
        };
        let (mut x, mut y) = (wrapped(x, left, span_x), wrapped(y, top, span_y));
        let mut fade = fade;
        // Out of an emitter: born at its own moment after the start, and again each time it
        // has gone as far as takes it past the frame, at another place of the emitter.
        let mut born: Option<(f64, f64)> = None;
        if emitting {
            let speed = vx.hypot(vy);
            if speed < 1e-6 {
                continue;
            }
            let way = (frame_width.hypot(frame_height) / first_scale + 2.0 * margin) + (emit_width.hypot(emit_height) / node_scale);
            let life = (way / speed).min(100_000.0);
            // Coming for as many frames before the start as the pre-roll says, and seen from it.
            let lived = time - (start - preroll + hashed(seed, i, 6) * life);
            if lived < 0.0 || time < start {
                continue;
            }
            let round = (lived / life).floor();
            let age = lived - round * life;
            let anew = seed ^ (round as i64 as u32).wrapping_mul(0x2545_F491);
            let from =
                (emit_middle.x + (hashed(anew, i, 3) - 0.5) * emit_width / node_scale, emit_middle.y + (hashed(anew, i, 4) - 0.5) * emit_height / node_scale);
            (x, y) = (from.0 + vx * age, from.1 + vy * age);
            fade = if fall == Fall::Wind { (std::f64::consts::PI * age / life).sin() } else { 1.0 };
            born = Some((from.1, age));
        }
        // A flake sways from side to side as it falls, each at its own pace.
        if fall == Fall::Snow {
            let pace = 0.03 * (1.0 + 0.5 * spread(seed, i, 5));
            x += sway * near * (std::f64::consts::TAU * (pace * time + hashed(seed, i, 6))).sin();
        }
        let alpha = ((0.4 + 0.6 * faint) * fade * if fall == Fall::Wind { 0.6 } else { 1.0 }) as f32;
        // On a ground, a drop ends where it meets it, and throws up a few droplets that fall
        // back: where it is between the ground and the foot of its tile, it has met it as far
        // back as it has come since, and before that, a whole tile's height before.
        let mut landed = false;
        let mut droplets: Vec<(f64, f64, f32)> = Vec::new();
        // Or the splash's drawing on this frame of it, with where it landed.
        let mut played: Option<(f64, usize)> = None;
        // Out of an emitter, it meets the ground as far after its birth as its fall from there
        // takes, if it was born above it.
        let since = match born {
            Some((from, age)) if splash && vy > 0.0 && from < ground => {
                let met = (ground - from) / vy;
                (age >= met).then_some((age - met) * vy)
            }
            Some(_) => None,
            None if splash && vy > 0.0 && ground > top && ground < top + span_y => {
                Some((top + hashed(seed, i, 4) * span_y + vy * time - ground).rem_euclid(span_y))
            }
            None => None,
        };
        if let Some(since) = since {
            landed = y > ground;
            let age = since / vy;
            if age < splash_life {
                let hit = x - vx * age;
                if !splash_play.is_empty() {
                    played = splash_play[(age as usize).min(splash_play.len() - 1)].map(|index| (hit, index));
                }
                for j in (0..SPLASH_DROPS).filter(|_| splash_play.is_empty()) {
                    let which = 100 + j;
                    let turn = (-90.0 + 60.0 * spread(seed, i, which)).to_radians();
                    let thrown = size * splash_scale * 1.2 * (0.5 + hashed(seed, i, which + 10));
                    let (dx, dy) = (turn.cos() * thrown * age, turn.sin() * thrown * age + 0.5 * gravity * age * age);
                    if dy <= 0.0 {
                        droplets.push((hit + dx, ground + dy, alpha * (1.0 - (age / SPLASH_LIFE) as f32)));
                    }
                }
            }
        }
        // What the camera sees now, and the margin round it, in the scene at its depth.
        let (out_x, out_y) = (frame_width / 2.0 + margin * now_scale, frame_height / 2.0 + margin * now_scale);
        let seen = [(-out_x, -out_y), (out_x, -out_y), (out_x, out_y), (-out_x, out_y)].map(|(cx, cy)| back.apply(Point::new(cx, cy)));
        let (low_x, high_x) = seen.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), at| (low.min(at.x), high.max(at.x)));
        let (low_y, high_y) = seen.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), at| (low.min(at.y), high.max(at.y)));
        // Out of an emitter there is the one, where it has gone.
        let copies = |at: f64, low: f64, high: f64, span: f64| {
            if emitting {
                return 0..=0;
            }
            let first = ((low - at) / span).ceil() as i64;
            first..=(((high - at) / span).floor() as i64).min(first + COPIES - 1)
        };
        let to_frame = place.from_frame.mul(&now);
        for across in copies(x, low_x, high_x, span_x) {
            for down in copies(y, low_y, high_y, span_y) {
                let (off_x, off_y) = (across as f64 * span_x, down as f64 * span_y);
                let (cx, cy) = (x + off_x, y + off_y);
                let seen_size = size * now_scale * place.scale;
                for (j, (dx, dy, faded)) in droplets.iter().enumerate() {
                    // A droplet is one of the splash's own sprites, as drawn; or else, where the
                    // drops are sprites, a small one of theirs.
                    let pick = |all: &[Sprite]| ((hashed(seed, i, 120 + j as u32) * all.len() as f64) as usize).min(all.len().saturating_sub(1));
                    let at = Affine::translate(dx + off_x, dy + off_y);
                    if !splash_sprites.is_empty() {
                        let index = pick(splash_sprites);
                        let to_picture = to_frame.mul(&at).mul(&Affine::scale(splash_scale * grown / splash_sprites[index].per_unit));
                        drawn.push(Drawn::Splash { index, to_picture, alpha: *faded });
                        continue;
                    }
                    if !sprites.is_empty() {
                        let index = pick(sprites);
                        let to_picture = to_frame.mul(&at).mul(&Affine::scale(sprite_scale * splash_scale * grown * 0.3 / sprites[index].per_unit));
                        drawn.push(Drawn::Sprite { index, to_picture, alpha: *faded });
                        continue;
                    }
                    let at = to_frame.apply(Point::new(dx + off_x, dy + off_y));
                    let radius = (seen_size * splash_scale / 4.0).max(0.4);
                    drawn.push(Drawn::Mark(Mark { from: at, to: at, radius, taper: 1.0, soft: radius, alpha: *faded }));
                }
                // A splash that plays: its drawing for the frame, its origin where the drop met
                // the ground.
                if let Some((hit, index)) = played {
                    let at = Affine::translate(hit + off_x, ground + off_y);
                    let to_picture = to_frame.mul(&at).mul(&Affine::scale(splash_scale * grown / splash_sprites[index].per_unit));
                    drawn.push(Drawn::Splash { index, to_picture, alpha });
                }
                if landed {
                    continue;
                }
                // A sprite, turned as far as its own share of the turn and spinning its own way.
                if !sprites.is_empty() {
                    let index = ((hashed(seed, i, 7) * sprites.len() as f64) as usize).min(sprites.len() - 1);
                    let way = if hashed(seed, i, 9) < 0.5 { -1.0 } else { 1.0 };
                    // A drop drawn upright leans along its fall, and a gust drawn from left to right
                    // lies along its way, as the wind slants them.
                    let along = match fall {
                        Fall::Rain => vy.atan2(vx) - std::f64::consts::FRAC_PI_2,
                        Fall::Wind => vy.atan2(vx),
                        Fall::Snow => 0.0,
                    };
                    let angle =
                        along + turn * hashed(seed, i, 8) * std::f64::consts::TAU + (spin * way * (1.0 + 0.3 * spread(seed, i, 10)) * time).to_radians();
                    let to_picture = to_frame
                        .mul(&Affine::translate(cx, cy))
                        .mul(&Affine::rotate(angle))
                        .mul(&Affine::scale(sprite_scale * grown / sprites[index].per_unit));
                    drawn.push(Drawn::Sprite { index, to_picture, alpha });
                    continue;
                }
                let (to, from) = (to_frame.apply(Point::new(cx, cy)), to_frame.apply(Point::new(cx - back_x, cy - back_y)));
                drawn.push(Drawn::Mark(match fall {
                    // A soft round flake as wide as its size.
                    Fall::Snow => Mark { from, to, radius: seen_size / 2.0, taper: 1.0, soft: seen_size / 2.0, alpha },
                    // A thin streak, a third of its size wide, with a soft edge of a pixel.
                    _ => Mark { from, to, radius: (seen_size / 6.0).max(0.25), taper: 1.0, soft: 1.0, alpha },
                }));
            }
        }
    }
    drawn
}

/// Where a thing thrown at `velocity` has gone `age` frames later, and how fast it goes
/// then, falling by `gravity` a frame each frame and keeping `kept` of its speed through
/// the air each frame: worked out at once, not frame by frame, so any frame is found alone.
pub(crate) fn thrown(velocity: [f64; 2], gravity: f64, kept: f64, age: f64) -> ([f64; 2], [f64; 2]) {
    let slowing = -kept.max(1e-9).ln();
    if slowing < 1e-9 {
        return ([velocity[0] * age, velocity[1] * age + 0.5 * gravity * age * age], [velocity[0], velocity[1] + gravity * age]);
    }
    // Slowed in proportion to its speed, it falls towards a speed it can't pass.
    let (left, most) = ((-slowing * age).exp(), gravity / slowing);
    let gone = (1.0 - left) / slowing;
    ([velocity[0] * gone, most * age + (velocity[1] - most) * gone], [velocity[0] * left, velocity[1] * left + most * (1.0 - left)])
}

/// A burst: from the centre, in the scene at the node's depth, every particle is thrown at its
/// own moment between the start frame and as many frames after it as it is let out over, in
/// its own direction within the spread round the angle. Each falls, slows and fades out over
/// its life, and is then gone. Repeated, it is thrown again every so many frames, each throw
/// with particles of its own.
fn burst(effect: &Effect, frame: u32, time: f64, place: &Place, sprites: &[Sprite]) -> Vec<Drawn> {
    let number = |id: &str| effect.number(id, frame);
    let seed = number("seed").max(0.0) as u32;
    let count = number("count").max(0.0) as u32;
    let pace = number("speed") / 100.0 * place.size[1] * BURST_THROW;
    let (variation, size_variation) = (number("speedVariation") / 100.0, number("sizeVariation") / 100.0);
    let (wind, length, size, far) = (number("wind"), number("length"), number("size"), number("far") / 100.0);
    let (depth, range, angle, spread_by) = (number(iskra_model::DEPTH), number("depthRange").max(0.0), number("angle"), number("spread"));
    let (first_start, over, life, gravity, kept) =
        (number("start"), number("over").max(1.0), number("life").max(1.0), number("gravity"), 1.0 - number("drag") / 100.0);
    // Thrown again every so many frames, each time anew: the throws whose particles may still
    // be about, as a particle lives at most its life and a third, after it is let out.
    let repeat = number("repeat");
    let throws: Vec<(f64, u32)> = match repeat >= 1.0 {
        true => {
            let last = ((time - first_start) / repeat).floor();
            let first = (last - ((over + life * 1.3) / repeat).ceil()).max(0.0);
            let rounds = (first as i64..=last as i64).rev().take(MOST_THROWS).collect::<Vec<_>>();
            rounds.into_iter().rev().map(|round| (first_start + round as f64 * repeat, seed ^ (round as u32).wrapping_mul(0x2545_F491))).collect()
        }
        false => vec![(first_start, seed)],
    };
    let (sprite_scale, turn, spin) = (number("spriteScale") / 100.0, number("turn") / 100.0, number("spin"));
    // The centre, in the scene: where the camera saw it at the node's depth on the first frame.
    let Some(centre) = (place.camera)(depth, 0).and_then(|first| first.inverse()).map(|home| home.apply(Point::new(number("centreX"), number("centreY"))))
    else {
        return Vec::new();
    };
    let mut drawn = Vec::new();
    // Stepped, each particle that is about is where the simulation has it; else where it is
    // thrown to.
    let about: Vec<(u32, u32, Option<([f64; 2], [f64; 2])>)> = match place.stepped {
        Some(live) => live.iter().map(|one| (one.round, one.i, Some((one.at, one.going)))).collect(),
        None => (0..throws.len() as u32).flat_map(|round| (0..count).map(move |i| (round, i, None))).collect(),
    };
    // A stepped burst says its throws by their rounds from the first.
    let throw = |round: u32| match (place.stepped.is_some(), repeat >= 1.0) {
        (true, true) => (first_start + f64::from(round) * repeat, seed ^ round.wrapping_mul(0x2545_F491)),
        (true, false) => (first_start, seed),
        (false, _) => throws[round as usize],
    };
    for (round, i, stepped) in about {
        let (start, seed) = throw(round);
        let z = hashed(seed, i, 1);
        let (at_depth, near, faint) = if range > 0.0 { (depth - range * z, 1.0, 1.0 - 0.6 * z) } else { (depth, 1.0 - far * z, 1.0 - far * z) };
        let lives = life * (1.0 + 0.3 * spread(seed, i, 12));
        let age = time - (start + (over - 1.0) * hashed(seed, i, 11));
        if !(0.0..lives).contains(&age) {
            continue;
        }
        let Some(now) = (place.camera)(at_depth, frame) else { continue };
        let now_scale = now.determinant().abs().sqrt();
        let (x, y, vx, vy) = match stepped {
            Some((at, going)) => (at[0], at[1], going[0] + wind * near, going[1]),
            None => {
                let way = (angle + spread_by * (hashed(seed, i, 13) - 0.5)).to_radians();
                let speed = pace * near * (1.0 + variation * spread(seed, i, 2));
                let (gone, going) = thrown([way.cos() * speed, way.sin() * speed], gravity * near, kept, age);
                (centre.x + gone[0] + wind * near * age, centre.y + gone[1], going[0] + wind * near, going[1])
            }
        };
        let grown = near * (1.0 + size_variation * spread(seed, i, 3));
        let fading = 1.0 - age / lives;
        let alpha = ((0.4 + 0.6 * faint) * fading) as f32;
        let to_frame = place.from_frame.mul(&now);
        if !sprites.is_empty() {
            let index = ((hashed(seed, i, 7) * sprites.len() as f64) as usize).min(sprites.len() - 1);
            let way = if hashed(seed, i, 9) < 0.5 { -1.0 } else { 1.0 };
            let angle = turn * hashed(seed, i, 8) * std::f64::consts::TAU + (spin * way * (1.0 + 0.3 * spread(seed, i, 10)) * age).to_radians();
            let to_picture =
                to_frame.mul(&Affine::translate(x, y)).mul(&Affine::rotate(angle)).mul(&Affine::scale(sprite_scale * grown / sprites[index].per_unit));
            drawn.push(Drawn::Sprite { index, to_picture, alpha });
            continue;
        }
        // A spark: a streak as long as its way in `length` frames, as wide as half its size.
        let (to, from) = (to_frame.apply(Point::new(x, y)), to_frame.apply(Point::new(x - vx * length, y - vy * length)));
        let radius = ((size * grown).max(0.1) * now_scale * place.scale / 4.0).max(0.25);
        drawn.push(Drawn::Mark(Mark { from, to, radius, taper: 1.0, soft: 1.0, alpha }));
    }
    drawn
}

/// Speed lines on the held frame `time`: thick where they come in from past the frame's edge,
/// thinning to a point. *Radial*, towards the centre, stopping short of the clear middle;
/// *Parallel*, along the angle, across the frame. Each held frame has lines of its own.
fn speed_lines(effect: &Effect, frame: u32, time: u32, place: &Place, sprites: &[Sprite]) -> Vec<Drawn> {
    let ([frame_width, frame_height], scale, from_frame) = (place.size, place.scale, place.from_frame);
    let number = |id: &str| effect.number(id, frame);
    let sprite_scale = number("spriteScale") / 100.0;
    // Streaming, each line goes on being itself, moving; boiling, the seed and the held frame
    // together give other lines on every frame that is drawn.
    let stream = effect.choice("flow", frame) == "Stream";
    let seed = number("seed").max(0.0) as u32;
    let seed = if stream { seed } else { seed.wrapping_add(time.wrapping_mul(0x632B_E5AB)) };
    let count = number("count").max(0.0) as u32;
    let (size, size_variation) = (number("size"), number("sizeVariation") / 100.0);
    let length_variation = number("lengthVariation") / 100.0;
    // From the middle of the frame to its corner, and past it by the widest a line is.
    let corner = frame_width.hypot(frame_height) / 2.0 + size * 2.0;
    let radial = effect.choice("lines", frame) != "Parallel";
    let (centre_x, centre_y) = (number("centreX"), number("centreY"));
    let clear = number("clear") / 100.0;
    let angle = number("angle").to_radians();
    // How far a streaming line has gone, each at its own pace: at 100% it crosses from the
    // middle to the corner in some 12 frames.
    let gone = |i: u32| if stream { number("speed") / 100.0 * corner / 12.0 * (1.0 + 0.3 * spread(seed, i, 4)) * f64::from(time) } else { 0.0 };
    (0..count)
        .map(|i| {
            let width = (size * (1.0 + size_variation * spread(seed, i, 3))).max(0.1);
            let (outer, inner) = if radial {
                // Round the centre at an angle of its own, from past the frame's corner to as
                // near the centre as the clear middle and its own variation let it.
                let turn = hashed(seed, i, 1) * std::f64::consts::TAU;
                let (dx, dy) = (turn.cos(), turn.sin());
                let far = corner + centre_x.abs().max(centre_y.abs());
                let near = (corner * clear * (1.0 + length_variation * spread(seed, i, 2))).max(0.0);
                let (far, near) = match stream {
                    // Streaming out along its ray, as long as it is, from the clear middle to past
                    // the corner, and in again at the middle.
                    true => {
                        let long = (far - near) * 0.5 * (1.0 + length_variation * spread(seed, i, 5)).max(0.1);
                        let way = far - near + long;
                        let at = near + wrapped(hashed(seed, i, 6) * way + gone(i), 0.0, way);
                        (at, (at - long).max(near))
                    }
                    false => (far, near),
                };
                ((centre_x + dx * far, centre_y + dy * far), (centre_x + dx * near, centre_y + dy * near))
            } else {
                // Along the angle, from past one side of the frame, at a place of its own across
                // it, as far in as the clear part and its own variation let it.
                let (dx, dy) = (angle.cos(), angle.sin());
                let across = spread(seed, i, 1) * corner;
                let reach = 2.0 * corner * (1.0 - clear) * (1.0 + length_variation * spread(seed, i, 2)).max(0.05);
                // Streaming along the angle, from past one side to past the other, and round.
                let along = match stream {
                    true => wrapped(hashed(seed, i, 6) * (2.0 * corner + reach) + gone(i), 0.0, 2.0 * corner + reach) - reach,
                    false => 0.0,
                };
                let start = (-dx * corner - dy * across + dx * along, -dy * corner + dx * across + dy * along);
                // The thick end leads, as a line streaming by is drawn.
                match stream {
                    true => ((start.0 + dx * reach, start.1 + dy * reach), start),
                    false => (start, (start.0 + dx * reach, start.1 + dy * reach)),
                }
            };
            // A sprite is stretched along the line: drawn from left to right, its left end where
            // the line comes in, and as high as drawn, times the sprite scale.
            if !sprites.is_empty() {
                let index = ((hashed(seed, i, 7) * sprites.len() as f64) as usize).min(sprites.len() - 1);
                let (sprite, (along_x, along_y)) = (&sprites[index], (inner.0 - outer.0, inner.1 - outer.1));
                let b = sprite.flat.bounds;
                let long = along_x.hypot(along_y) / f64::from(b.width().max(1));
                let stretch = Affine { m: [long, 0.0, 0.0, sprite_scale / sprite.per_unit, 0.0, 0.0] };
                let to_picture = from_frame
                    .mul(&Affine::translate(outer.0, outer.1))
                    .mul(&Affine::rotate(along_y.atan2(along_x)))
                    .mul(&stretch)
                    .mul(&Affine::translate(-f64::from(b.x0), -f64::from(b.y0 + b.y1) / 2.0));
                return Drawn::Sprite { index, to_picture, alpha: 1.0 };
            }
            let from = from_frame.apply(Point::new(outer.0, outer.1));
            let to = from_frame.apply(Point::new(inner.0, inner.1));
            Drawn::Mark(Mark { from, to, radius: width * scale / 2.0, taper: 0.0, soft: 1.0, alpha: 1.0 })
        })
        .collect()
}

/// Lay `sprite` over `picture` where `to_picture` puts it, as solid as `alpha`: each pixel
/// it covers takes the sprite's colour at the point of it there.
fn stamp(picture: &mut [Px], (width, height): (u32, u32), sprite: &Sprite, to_picture: &Affine, alpha: f32, (tint, tinted): (Px, f32)) {
    let Some(to_sprite) = to_picture.inverse() else { return };
    let b = sprite.flat.bounds;
    let corners = [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)].map(|(x, y)| to_picture.apply(Point::new(f64::from(x), f64::from(y))));
    let (x0, x1) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), at| (low.min(at.x), high.max(at.x)));
    let (y0, y1) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), at| (low.min(at.y), high.max(at.y)));
    if x1 < 0.0 || y1 < 0.0 || x0 >= f64::from(width) || y0 >= f64::from(height) {
        return;
    }
    for y in (y0.floor().max(0.0) as u32)..(y1.ceil().min(f64::from(height)) as u32) {
        for x in (x0.floor().max(0.0) as u32)..(x1.ceil().min(f64::from(width)) as u32) {
            let colour = sprite.flat.sample(to_sprite.apply(Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5)));
            if colour[3] <= 0.0 {
                continue;
            }
            // Its own colour, taking as much of the tint's as `tinted` says, as solid as it is.
            let colour: Px = std::array::from_fn(|c| if c == 3 { colour[3] } else { colour[c] * (1.0 - tinted) + tint[c] * colour[3] * tinted });
            let under = &mut picture[(y * width + x) as usize];
            *under = std::array::from_fn(|c| colour[c] * alpha + under[c] * (1.0 - colour[3] * alpha));
        }
    }
}

/// Lay `mark` over `picture`, of `colour`: how much of each pixel it covers is how far the
/// pixel's middle is from its line, fading over `soft` across its edge, half inside and half
/// out, so that it covers as much as its width says at whatever size it is drawn.
fn lay(picture: &mut [Px], (width, height): (u32, u32), mark: &Mark, colour: Px) {
    let (soft, reach) = (mark.soft.max(1e-6), mark.reach());
    let (x0, x1) = (mark.from.x.min(mark.to.x) - reach, mark.from.x.max(mark.to.x) + reach);
    let (y0, y1) = (mark.from.y.min(mark.to.y) - reach, mark.from.y.max(mark.to.y) + reach);
    if x1 < 0.0 || y1 < 0.0 || x0 >= f64::from(width) || y0 >= f64::from(height) {
        return;
    }
    let (along_x, along_y) = (mark.to.x - mark.from.x, mark.to.y - mark.from.y);
    let long = along_x * along_x + along_y * along_y;
    // Across the line, one way: a pixel's middle farther than `reach` from the line through
    // the mark is not covered, so each row is looked at only where the line crosses it. A long
    // slanting line covers little of the box round it.
    let across = if long > 0.0 { [-along_y / long.sqrt(), along_x / long.sqrt()] } else { [0.0, 0.0] };
    let (left, right) = (x0.floor().max(0.0) as u32, x1.ceil().min(f64::from(width)) as u32);
    for y in (y0.floor().max(0.0) as u32)..(y1.ceil().min(f64::from(height)) as u32) {
        let (from_x, to_x) = match across[0].abs() > 1e-3 {
            true => {
                let off = across[1] * (f64::from(y) + 0.5 - mark.from.y);
                let ends = [(-reach - off) / across[0], (reach - off) / across[0]].map(|at| mark.from.x + at - 0.5);
                let (low, high) = (ends[0].min(ends[1]), ends[0].max(ends[1]));
                ((low.floor() - 1.0).max(f64::from(left)) as u32, ((high.ceil() + 2.0).max(0.0) as u32).min(right))
            }
            false => (left, right),
        };
        for x in from_x..to_x {
            let (px, py) = (f64::from(x) + 0.5 - mark.from.x, f64::from(y) + 0.5 - mark.from.y);
            let part = if long > 0.0 { ((px * along_x + py * along_y) / long).clamp(0.0, 1.0) } else { 0.0 };
            let distance = (px - along_x * part).hypot(py - along_y * part);
            // As wide as it is there; thinner than a pixel, a pixel wide and fainter by as much.
            let here = mark.radius * (1.0 - part * (1.0 - mark.taper));
            let radius = here.max(0.5);
            let thin = (here / radius) as f32;
            let covered = ((radius + soft / 2.0 - distance) / soft).clamp(0.0, 1.0) as f32 * thin * mark.alpha;
            if covered <= 0.0 {
                continue;
            }
            let under = &mut picture[(y * width + x) as usize];
            *under = std::array::from_fn(|c| colour[c] * covered + under[c] * (1.0 - colour[3] * covered));
        }
    }
}

#[cfg(test)]
mod tests {
    use iskra_model::{EffectId, PARTICLES, Value, WorkingSpace, effect_kind};

    use super::*;

    /// A Particles node with its parameters set as `set` says.
    fn node(set: &[(&str, Value)]) -> Effect {
        let mut node = Effect::new(EffectId(1), "Rain", effect_kind(PARTICLES).unwrap());
        for (param, value) in set {
            node.param_mut(param).expect("the kind has it").value = value.clone();
        }
        node
    }

    /// The node's picture on `frame`, of a frame 320 by 180 made at `scale`, the camera at home.
    fn picture(node: &Effect, frame: u32, scale: f64) -> (Vec<Px>, u32, u32) {
        filmed(node, frame, scale, &|_, _| Some(Affine::IDENTITY))
    }

    /// The same, through `camera`.
    fn filmed(node: &Effect, frame: u32, scale: f64, camera: &dyn Fn(f64, u32) -> Option<Affine>) -> (Vec<Px>, u32, u32) {
        let (width, height) = ((320.0 * scale) as u32, (180.0 * scale) as u32);
        let from_frame = Affine::translate(f64::from(width) / 2.0, f64::from(height) / 2.0).mul(&Affine::scale(scale));
        let place = Place { size: [320.0, 180.0], scale, from_frame: &from_frame, camera, plane: None, stepped: None };
        (draw(node, frame, (width, height), &place, WorkingSpace::Srgb, &[], (&[], &[])), width, height)
    }

    /// Where a picture is painted, on average, by how much, from its top left. `None` with
    /// nothing painted.
    fn middle_of((pixels, width, _): &(Vec<Px>, u32, u32)) -> Option<[f64; 2]> {
        let (mut sum, mut at) = (0.0, [0.0, 0.0]);
        for (i, pixel) in pixels.iter().enumerate() {
            let alpha = f64::from(pixel[3]);
            sum += alpha;
            at[0] += alpha * (i as u32 % width) as f64;
            at[1] += alpha * (i as u32 / width) as f64;
        }
        (sum > 0.0).then(|| [at[0] / sum, at[1] / sum])
    }

    /// Where the picture is painted, on average, by how much: in the frame's pixels from its
    /// top left. `None` with nothing painted.
    fn middle(node: &Effect, frame: u32) -> Option<[f64; 2]> {
        let (pixels, width, _) = picture(node, frame, 1.0);
        let (mut sum, mut at) = (0.0, [0.0, 0.0]);
        for (i, pixel) in pixels.iter().enumerate() {
            let alpha = f64::from(pixel[3]);
            sum += alpha;
            at[0] += alpha * (i as u32 % width) as f64;
            at[1] += alpha * (i as u32 / width) as f64;
        }
        (sum > 0.0).then(|| [at[0] / sum, at[1] / sum])
    }

    #[test]
    fn a_drop_falls_as_fast_as_its_speed_says_and_the_wind_takes_it_aside() {
        let one = |seed: f64, wind: f64| {
            node(&[
                ("count", Value::Number(1.0)),
                ("far", Value::Number(0.0)),
                ("speedVariation", Value::Number(0.0)),
                ("seed", Value::Number(seed)),
                ("wind", Value::Number(wind)),
            ])
        };
        // A seed whose drop is well inside the frame on both frames, away from where it wraps.
        let inside = |at: Option<[f64; 2]>| at.is_some_and(|[x, y]| (40.0..280.0).contains(&x) && (30.0..150.0).contains(&y));
        let seed = (0..200).map(f64::from).find(|seed| inside(middle(&one(*seed, 4.0), 0)) && inside(middle(&one(*seed, 4.0), 1))).expect("a seed");
        let (before, after) = (middle(&one(seed, 4.0), 0).unwrap(), middle(&one(seed, 4.0), 1).unwrap());
        assert!((after[1] - before[1] - 180.0 * RAIN_FALL).abs() < 0.5, "{before:?} to {after:?}");
        assert!((after[0] - before[0] - 4.0).abs() < 0.5, "{before:?} to {after:?}");
    }

    #[test]
    fn any_frame_is_the_same_however_it_is_reached_and_held_on_twos_it_moves_every_second() {
        let rain = node(&[]);
        let (first, ..) = picture(&rain, 10, 1.0);
        assert!(first.iter().any(|pixel| pixel[3] > 0.0), "something is drawn");
        assert_eq!(picture(&rain, 10, 1.0).0, first, "the same frame again, after no other");
        assert_ne!(picture(&rain, 11, 1.0).0, first, "it falls");
        assert_ne!(picture(&node(&[("seed", Value::Number(2.0))]), 10, 1.0).0, first, "another seed, other drops");
        let twos = node(&[("hold", Value::Choice("On Twos".to_string()))]);
        assert_eq!(picture(&twos, 11, 1.0).0, picture(&twos, 10, 1.0).0);
        assert_ne!(picture(&twos, 12, 1.0).0, picture(&twos, 10, 1.0).0);
        // Snow is drawn too, and none at all is nothing.
        assert!(picture(&node(&[("kind", Value::Choice("Snow".to_string()))]), 10, 1.0).0.iter().any(|pixel| pixel[3] > 0.0));
        assert!(picture(&node(&[("count", Value::Number(0.0))]), 10, 1.0).0.iter().all(|pixel| pixel[3] == 0.0));
    }

    /// How solid the picture of `node` on `frame` is at a point of the frame, from its middle.
    fn solid_at(node: &Effect, frame: u32, [x, y]: [f64; 2]) -> f32 {
        let (pixels, width, _) = picture(node, frame, 1.0);
        pixels[((90.0 + y) as u32 * width + (160.0 + x) as u32) as usize][3]
    }

    #[test]
    fn radial_speed_lines_leave_the_middle_clear_and_boil_unless_held() {
        let lines = |set: &[(&str, Value)]| {
            let mut all = vec![("kind", Value::Choice("Speed Lines".to_string())), ("count", Value::Number(300.0)), ("clear", Value::Number(50.0))];
            all.extend(set.iter().cloned());
            node(&all)
        };
        let radial = lines(&[]);
        // The clear middle, half way to the corner less its variation, has nothing; the edge does.
        let (pixels, width, height) = picture(&radial, 4, 1.0);
        let corner = 320f64.hypot(180.0) / 2.0;
        let mut inner = 0.0;
        let mut rim = 0.0;
        for (i, pixel) in pixels.iter().enumerate() {
            let (x, y) = ((i as u32 % width) as f64 + 0.5 - 160.0, (i as u32 / width) as f64 + 0.5 - 90.0);
            let from_middle = x.hypot(y);
            if from_middle < corner * 0.5 * 0.65 {
                inner += pixel[3];
            } else if from_middle > corner * 0.9 {
                rim += pixel[3];
            }
        }
        assert!(inner == 0.0 && rim > 100.0, "{inner} in the middle, {rim} at the rim of {width} by {height}");
        // Its centre moved, the clear middle goes with it.
        let aside = lines(&[("centreX", Value::Number(80.0))]);
        assert_eq!(solid_at(&aside, 4, [80.0, 0.0]), 0.0);
        // Drawn anew on each frame, unless held.
        assert_ne!(picture(&radial, 5, 1.0).0, pixels);
        let twos = lines(&[("hold", Value::Choice("On Twos".to_string()))]);
        assert_eq!(picture(&twos, 5, 1.0).0, picture(&twos, 4, 1.0).0);
        assert_eq!(picture(&radial, 4, 1.0).0, pixels, "the same frame again is the same");
    }

    #[test]
    fn parallel_speed_lines_run_along_their_angle() {
        let extent = |angle: f64| {
            let one = node(&[
                ("kind", Value::Choice("Speed Lines".to_string())),
                ("lines", Value::Choice("Parallel".to_string())),
                ("count", Value::Number(1.0)),
                ("clear", Value::Number(0.0)),
                ("lengthVariation", Value::Number(0.0)),
                ("angle", Value::Number(angle)),
            ]);
            let (pixels, width, _) = picture(&one, 0, 1.0);
            let painted: Vec<(u32, u32)> =
                pixels.iter().enumerate().filter(|(_, pixel)| pixel[3] > 0.0).map(|(i, _)| (i as u32 % width, i as u32 / width)).collect();
            let span = |of: fn(&(u32, u32)) -> u32| painted.iter().map(of).max().unwrap_or(0) - painted.iter().map(of).min().unwrap_or(0);
            (span(|at| at.0), span(|at| at.1))
        };
        let (wide, high) = extent(0.0);
        assert!(wide > 300 && high < 8, "across: {wide} by {high}");
        let (wide, high) = extent(90.0);
        assert!(high > 170 && wide < 8, "down: {wide} by {high}");
    }

    #[test]
    fn a_streak_of_wind_blows_along_its_angle_and_fades_in_and_out() {
        let gust = |seed: f64| {
            node(&[
                ("kind", Value::Choice("Wind".to_string())),
                ("count", Value::Number(1.0)),
                // Short, so that all of it is in the frame.
                ("length", Value::Number(0.05)),
                ("far", Value::Number(0.0)),
                ("speedVariation", Value::Number(0.0)),
                ("seed", Value::Number(seed)),
            ])
        };
        // A seed whose streak is half way through its life, and well inside the frame on two
        // frames, then on a third a whole life later.
        let inside = |at: Option<[f64; 2]>| at.is_some_and(|[x, y]| (60.0..260.0).contains(&x) && (30.0..150.0).contains(&y));
        let seed = (0..400)
            .map(f64::from)
            .find(|seed| (0.3..0.6).contains(&hashed(*seed as u32, 0, 6)) && inside(middle(&gust(*seed), 0)) && inside(middle(&gust(*seed), 1)))
            .expect("a seed");
        let (before, after) = (middle(&gust(seed), 0).unwrap(), middle(&gust(seed), 1).unwrap());
        assert!((after[0] - before[0] - 320.0 * WIND_BLOW).abs() < 0.5 && (after[1] - before[1]).abs() < 0.01, "{before:?} to {after:?}");
        // Fainter near the end of its life than in the middle of it.
        let solid = |frame: u32| picture(&gust(seed), frame, 1.0).0.iter().map(|pixel| f64::from(pixel[3])).sum::<f64>();
        let life = WIND_LIFE * (1.0 + 0.5 * spread(seed as u32, 0, 5));
        let to_end = (life * (1.0 - hashed(seed as u32, 0, 6)) - 1.0).floor() as u32;
        assert!(solid(to_end) < solid(0) * 0.5, "{} at its end, {} half way", solid(to_end), solid(0));
    }

    /// A camera as far from the scene as 100, which pans right by 10 of the frame's pixels a
    /// frame: what is at depth 0 moves left by 10 a frame, and what is farther less.
    fn panning(depth: f64, frame: u32) -> Option<Affine> {
        let larger = 100.0 / (100.0 - depth);
        Some(Affine::scale(larger).mul(&Affine::translate(-10.0 * f64::from(frame), 0.0)))
    }

    #[test]
    fn a_particle_is_seen_through_the_camera_and_the_far_ones_move_less() {
        let still = |depth: f64| {
            node(&[
                ("count", Value::Number(1.0)),
                ("speed", Value::Number(0.0)),
                ("far", Value::Number(0.0)),
                ("seed", Value::Number(3.0)),
                (iskra_model::DEPTH, Value::Number(depth)),
            ])
        };
        for (depth, moved) in [(0.0, 10.0), (-100.0, 5.0)] {
            let at = |frame: u32| middle_of(&filmed(&still(depth), frame, 1.0, &panning)).expect("it is seen");
            let (first, second) = (at(0), at(1));
            assert!((first[0] - second[0] - moved).abs() < 0.2, "at depth {depth}: {first:?} to {second:?}");
        }
        // Spread in depth, the near ones go by faster than the far: the picture changes as the
        // camera pans, with nothing falling.
        let deep = node(&[("speed", Value::Number(0.0)), ("depthRange", Value::Number(200.0))]);
        assert_ne!(filmed(&deep, 1, 1.0, &panning).0, filmed(&deep, 0, 1.0, &panning).0);
    }

    #[test]
    fn the_camera_panned_far_away_still_sees_as_many() {
        let rain = node(&[("speed", Value::Number(0.0)), ("depthRange", Value::Number(50.0))]);
        let solid = |frame: u32| filmed(&rain, frame, 1.0, &panning).0.iter().map(|pixel| f64::from(pixel[3])).sum::<f64>();
        let (home, away) = (solid(0), solid(300));
        assert!((away / home - 1.0).abs() < 0.2, "{home} at home, {away} 3000 to the right");
    }

    #[test]
    fn a_throw_is_where_going_frame_by_frame_finely_would_take_it() {
        for (velocity, gravity, kept) in [([3.0, -8.0], 0.5, 1.0), ([3.0, -8.0], 0.5, 0.9), ([-2.0, 4.0], -0.3, 0.5)] {
            // A thousand steps a frame, slowed and pulled in proportion to the step.
            let (mut at, mut going, steps) = ([0.0f64, 0.0], velocity, 1000);
            let slowing = -f64::ln(kept);
            for _ in 0..(20 * steps) {
                let dt = 1.0 / f64::from(steps);
                going = [going[0] - slowing * going[0] * dt, going[1] + (gravity - slowing * going[1]) * dt];
                at = [at[0] + going[0] * dt, at[1] + going[1] * dt];
            }
            let (gone, now) = thrown(velocity, gravity, kept, 20.0);
            assert!((gone[0] - at[0]).abs() < 0.05 && (gone[1] - at[1]).abs() < 0.05, "{gone:?} against {at:?}");
            assert!((now[0] - going[0]).abs() < 0.01 && (now[1] - going[1]).abs() < 0.01, "{now:?} against {going:?}");
        }
    }

    #[test]
    fn a_burst_is_thrown_out_on_its_frame_and_is_gone_after_its_life() {
        let up = |set: &[(&str, Value)]| {
            let mut all = vec![
                ("kind", Value::Choice("Burst".to_string())),
                ("count", Value::Number(1.0)),
                ("far", Value::Number(0.0)),
                ("speedVariation", Value::Number(0.0)),
                ("length", Value::Number(0.0)),
                ("spread", Value::Number(0.0)),
                ("angle", Value::Number(-90.0)),
                ("gravity", Value::Number(0.0)),
                ("drag", Value::Number(0.0)),
                ("start", Value::Number(10.0)),
                ("life", Value::Number(20.0)),
                ("centreX", Value::Number(40.0)),
                ("centreY", Value::Number(30.0)),
            ];
            all.extend(set.iter().cloned());
            node(&all)
        };
        let one = up(&[]);
        assert_eq!(middle(&one, 9), None, "not yet thrown");
        let at = |frame: u32| middle(&one, frame).expect("it is there");
        let thrown_at = at(10);
        assert!((thrown_at[0] - 200.0).abs() < 0.6 && (thrown_at[1] - 120.0).abs() < 0.6, "from the centre: {thrown_at:?}");
        let later = at(12);
        assert!((thrown_at[1] - later[1] - 2.0 * 180.0 * BURST_THROW).abs() < 0.3, "up at its speed: {thrown_at:?} to {later:?}");
        assert_eq!(middle(&one, 40), None, "gone after its life");
        // With gravity it slows as it rises, and comes down.
        let falls = up(&[("gravity", Value::Number(1.0))]);
        let heights: Vec<f64> = [10, 14, 18, 22].iter().map(|frame| middle(&falls, *frame).unwrap()[1]).collect();
        // Up is less: it rises less in the second four frames than in the first, then falls.
        assert!(heights[0] - heights[1] > heights[1] - heights[2] && heights[3] > heights[2], "{heights:?}");
    }

    #[test]
    fn a_burst_that_repeats_is_thrown_again_each_time_anew() {
        let burst = |repeat: f64, life: f64| {
            node(&[
                ("kind", Value::Choice("Burst".to_string())),
                ("count", Value::Number(40.0)),
                ("life", Value::Number(life)),
                ("repeat", Value::Number(repeat)),
            ])
        };
        let solid = |node: &Effect, frame: u32| picture(node, frame, 1.0).0.iter().map(|pixel| f64::from(pixel[3])).sum::<f64>();
        // Once, it is gone after its life; repeated every 20 frames, it is back, with
        // particles of its own.
        assert_eq!(solid(&burst(0.0, 10.0), 25), 0.0);
        let again = burst(20.0, 10.0);
        assert!(solid(&again, 25) > 0.0);
        assert_ne!(picture(&again, 25, 1.0).0, picture(&again, 5, 1.0).0);
        // Thrown every 5 frames and living 30, several throws are about at once.
        let fountain = burst(5.0, 30.0);
        assert!(solid(&fountain, 60) > 3.0 * solid(&burst(0.0, 30.0), 5), "{} against one throw's {}", solid(&fountain, 60), solid(&burst(0.0, 30.0), 5));
    }

    #[test]
    fn streaming_speed_lines_go_on_moving_out_or_along() {
        // Radial lines start at the clear middle: a small one, for them to be seen whole.
        let one = |lines: &str, seed: f64| {
            node(&[
                ("kind", Value::Choice("Speed Lines".to_string())),
                ("flow", Value::Choice("Stream".to_string())),
                ("lines", Value::Choice(lines.to_string())),
                ("count", Value::Number(1.0)),
                ("clear", Value::Number(if lines == "Radial" { 20.0 } else { 80.0 })),
                ("lengthVariation", Value::Number(0.0)),
                ("seed", Value::Number(seed)),
            ])
        };
        let corner = 320f64.hypot(180.0) / 2.0 + 8.0;
        let pace = |seed: f64| corner / 12.0 * (1.0 + 0.3 * spread(seed as u32, 0, 4));
        // Parallel, along the angle: a seed whose line is well inside on two frames.
        let inside = |at: Option<[f64; 2]>| at.is_some_and(|[x, y]| (80.0..240.0).contains(&x) && (20.0..160.0).contains(&y));
        let seed =
            (0..400).map(f64::from).find(|seed| inside(middle(&one("Parallel", *seed), 0)) && inside(middle(&one("Parallel", *seed), 1))).expect("a seed");
        let (before, after) = (middle(&one("Parallel", seed), 0).unwrap(), middle(&one("Parallel", seed), 1).unwrap());
        assert!((after[0] - before[0] - pace(seed)).abs() < 0.5 && (after[1] - before[1]).abs() < 0.01, "{before:?} to {after:?}");
        // Radial, out from the centre: its thick outer end farther from it a frame later, by
        // its pace. Its inner end waits at the clear middle until the line is all out.
        let outer = |seed: f64, frame: u32| {
            let (pixels, width, _) = picture(&one("Radial", seed), frame, 1.0);
            let far = pixels.iter().enumerate().filter(|(_, pixel)| pixel[3] > 0.05);
            far.map(|(i, _)| ((i as u32 % width) as f64 + 0.5 - 160.0).hypot((i as u32 / width) as f64 + 0.5 - 90.0)).fold(0.0, f64::max)
        };
        let seed = (0..400).map(f64::from).find(|seed| outer(*seed, 0) > 50.0 && outer(*seed, 1) < 85.0 && outer(*seed, 1) > outer(*seed, 0)).expect("a seed");
        let (before, after) = (outer(seed, 0), outer(seed, 1));
        assert!((after - before - pace(seed)).abs() < 1.5, "{before} to {after}, at {}", pace(seed));
    }

    #[test]
    fn rain_that_splashes_ends_on_the_ground() {
        let rain = |splash: bool| node(&[("splash", Value::Switch(splash)), ("ground", Value::Number(40.0)), ("count", Value::Number(800.0))]);
        let below = |splash: bool| {
            let (pixels, width, _) = picture(&rain(splash), 7, 1.0);
            pixels.chunks(width as usize).skip(90 + 42).flatten().map(|pixel| f64::from(pixel[3])).sum::<f64>()
        };
        assert!(below(false) > 10.0 && below(true) == 0.0, "{} below the ground without, {} with", below(false), below(true));
    }

    #[test]
    fn made_twice_as_large_it_covers_as_much_of_the_frame() {
        for kind in ["Rain", "Snow", "Wind", "Burst", "Speed Lines"] {
            let falling = node(&[("kind", Value::Choice(kind.to_string()))]);
            let covered = |scale: f64| {
                let (pixels, ..) = picture(&falling, 5, scale);
                pixels.iter().map(|pixel| f64::from(pixel[3])).sum::<f64>() / (scale * scale)
            };
            let (small, large) = (covered(1.0), covered(2.0));
            assert!((large / small - 1.0).abs() < 0.15, "{kind}: {small} at 1, {large} at 2");
        }
    }

    #[test]
    fn gravity_makes_rain_and_snow_fall_faster_by_as_many_pixels_a_frame() {
        let one = |kind: &str, pull: f64, seed: f64| {
            node(&[
                ("kind", Value::Choice(kind.to_string())),
                ("count", Value::Number(1.0)),
                ("far", Value::Number(0.0)),
                ("speedVariation", Value::Number(0.0)),
                ("sway", Value::Number(0.0)),
                ("length", Value::Number(0.2)),
                ("seed", Value::Number(seed)),
                ("pull", Value::Number(pull)),
            ])
        };
        let inside = |at: Option<[f64; 2]>| at.is_some_and(|[x, y]| (20.0..300.0).contains(&x) && (30.0..150.0).contains(&y));
        for (kind, pull) in [("Snow", 3.0), ("Rain", 3.0), ("Snow", -3.0)] {
            // How far it goes down in a frame, without and with the pull, where it is inside the
            // frame on both frames either way.
            let down = |pull: f64, seed: f64| Some(middle(&one(kind, pull, seed), 1)?[1] - middle(&one(kind, pull, seed), 0)?[1]);
            let seed = (0..400)
                .map(f64::from)
                .find(|seed| [0.0, pull].iter().all(|pull| inside(middle(&one(kind, *pull, *seed), 0)) && inside(middle(&one(kind, *pull, *seed), 1))))
                .expect("a seed");
            let (without, with) = (down(0.0, seed).unwrap(), down(pull, seed).unwrap());
            assert!((with - without - pull).abs() < 0.2, "{kind}: {without} a frame, {with} with a pull of {pull}");
        }
    }

    #[test]
    fn out_of_an_emitter_rain_starts_on_its_frame_and_falls_into_the_frame() {
        let rain = |set: &[(&str, Value)]| {
            let mut all = vec![
                ("emitter", Value::Choice("Line".to_string())),
                ("emitterY", Value::Number(-110.0)),
                ("emitterWidth", Value::Number(400.0)),
                ("start", Value::Number(10.0)),
                ("count", Value::Number(300.0)),
            ];
            all.extend(set.iter().cloned());
            node(&all)
        };
        // The rows painted, from the top of the frame.
        let rows = |node: &Effect, frame: u32| {
            let (pixels, width, _) = picture(node, frame, 1.0);
            pixels.chunks(width as usize).enumerate().filter(|(_, row)| row.iter().any(|pixel| pixel[3] > 0.0)).map(|(y, _)| y).collect::<Vec<_>>()
        };
        let line = rain(&[]);
        assert!(rows(&line, 9).is_empty(), "none before the start");
        // Three frames in, a drop has fallen 30 at most from 20 above the frame: only the top.
        let early = rows(&line, 13);
        assert!(!early.is_empty() && early.iter().all(|y| *y < 20), "{early:?}");
        // Later it rains all down the frame.
        assert!(rows(&line, 60).iter().any(|y| *y > 150));
        // With a pre-roll, still none before the start, but on it it rains all down the frame.
        let rolled = rain(&[("preroll", Value::Number(60.0))]);
        assert!(rows(&rolled, 9).is_empty());
        assert!(rows(&rolled, 10).iter().any(|y| *y > 150), "{:?}", rows(&rolled, 10).last());
        // Out of a point, in a narrow fall below it.
        let point = rain(&[("emitter", Value::Choice("Point".to_string())), ("emitterX", Value::Number(40.0)), ("far", Value::Number(0.0))]);
        let (pixels, width, _) = picture(&point, 60, 1.0);
        let across: Vec<u32> = pixels.iter().enumerate().filter(|(_, pixel)| pixel[3] > 0.05).map(|(i, _)| i as u32 % width).collect();
        assert!(!across.is_empty() && across.iter().all(|x| (195..=205).contains(x)), "{:?}", across.iter().min().zip(across.iter().max()));
        // Splashing, it ends on the ground as before.
        let splashing = rain(&[("splash", Value::Switch(true)), ("ground", Value::Number(40.0))]);
        assert!(rows(&splashing, 60).iter().all(|y| *y <= 131), "{:?}", rows(&splashing, 60).last());
    }

    #[test]
    fn a_particles_numbers_are_its_own_and_the_same_every_time() {
        assert_eq!(hashed(1, 7, 3), hashed(1, 7, 3));
        assert_ne!(hashed(1, 7, 3), hashed(2, 7, 3));
        assert_ne!(hashed(1, 7, 3), hashed(1, 8, 3));
        let all: Vec<f64> = (0..1000).map(|i| hashed(4, i, 1)).collect();
        assert!(all.iter().all(|value| (0.0..1.0).contains(value)));
        let mean = all.iter().sum::<f64>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05, "spread evenly: {mean}");
        assert_eq!(wrapped(12.0, 0.0, 10.0), 2.0);
        assert_eq!(wrapped(-3.0, 0.0, 10.0), 7.0);
    }
}
