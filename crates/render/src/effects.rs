//! What each kind of effect node does to the picture that goes through it (plan §8): the
//! reference, on the processor. The kinds themselves, with their ports and parameters, are
//! described in the model; here is only their picture.
//!
//! The blurs are `effectcraft-raster`'s, taken as they are. The effects of EffectCraft
//! themselves are bound to its project, so what a kind is and has is said here, in the
//! model, and the picture maths it shares with them is called.

use effectcraft_raster::{Image, Px, directional_blur, gaussian_blur, radial_blur};
use iskra_drawing::{Affine, Point};
use iskra_model::{Effect, MATTE, WorkingSpace};

/// The name a Style node is given what goes through it under, drawn in its style, beside its
/// mattes.
pub(crate) const STYLED: &str = "styled";

/// What an effect is given beside the picture that goes through it.
pub(crate) struct Given<'a> {
    /// The size of the picture, and of every matte.
    pub width: u32,
    pub height: u32,
    /// How many pixels of the picture a pixel of the frame is across: sizes are said in the
    /// frame's pixels, so that an effect looks the same at whatever size it is made.
    pub scale: f64,
    /// From the frame's units to the picture's pixels.
    pub from_frame: &'a Affine,
    /// What is wired into the other ports, by their names, each as a picture of that size.
    pub mattes: &'a [(&'a str, Vec<Px>)],
    /// The space the picture's colours are in.
    pub space: WorkingSpace,
    /// The shot's frame, in its own pixels.
    pub frame: [f64; 2],
    /// What a Particles node draws its particles as, where something is wired into its
    /// sprite port.
    pub sprites: &'a [crate::particles::Sprite],
    /// What a Particles node draws its rain's splashes as, where something is wired into its
    /// splash's sprite port.
    pub splash_sprites: &'a [crate::particles::Sprite],
    /// How a splash plays its sprites, frame by frame (`splash_play`): empty where it throws
    /// droplets instead.
    pub splash_play: &'a [Option<usize>],
    /// How the shot's camera sees the scene at a depth on a frame, from the scene to the
    /// frame's units: what rain, snow and wind are seen through.
    pub camera: &'a dyn Fn(f64, u32) -> Option<Affine>,
    /// Of a Particles node's rain or snow in sheets, which of how many is made.
    pub plane: Option<(u32, u32)>,
    /// A Particles node's burst stepped to the frame, where it bounces off a collider.
    pub stepped: Option<&'a [crate::simulation::Live]>,
}

impl Given<'_> {
    fn image(&self, data: Vec<Px>) -> Image {
        Image { width: self.width, height: self.height, data }
    }

    /// The matte that is wired in, as how solid it is at each pixel.
    fn matte(&self) -> Option<Vec<f32>> {
        self.mattes.iter().find(|(port, _)| *port == MATTE).map(|(_, matte)| solid(matte))
    }

    /// A plane of values, blurred by `radius` of the frame's pixels.
    fn softened(&self, plane: Vec<f32>, radius: f64) -> Vec<f32> {
        let sigma = radius * self.scale / 2.0;
        if sigma <= 0.05 {
            return plane;
        }
        let soft = gaussian_blur(&self.image(plane.iter().map(|value| [*value; 4]).collect()), sigma, sigma, false);
        soft.data.iter().map(|pixel| pixel[3].clamp(0.0, 1.0)).collect()
    }

    /// A plane of values, each the greatest within `by` of the frame's pixels of it, or below
    /// 0 the least: a shape made wider all round, or narrower.
    fn grown(&self, plane: Vec<f32>, by: f64) -> Vec<f32> {
        let pixels = by * self.scale;
        if pixels.abs() < 0.5 {
            return plane;
        }
        widened(&plane, self.width as usize, self.height as usize, pixels.abs().round() as usize, pixels > 0.0)
    }

    fn paint(&self, colour: [f64; 4]) -> Px {
        paint(colour, self.space)
    }
}

/// A colour of a parameter, sRGB with how solid, as a pixel: its colour times how solid, in
/// `space`.
pub fn paint(colour: [f64; 4], space: WorkingSpace) -> Px {
    let light = |value: f64| match space {
        WorkingSpace::Srgb => value as f32,
        WorkingSpace::Linear => super::to_linear(value as f32),
    };
    let solid = colour[3] as f32;
    [light(colour[0]) * solid, light(colour[1]) * solid, light(colour[2]) * solid, solid]
}

/// The blur of the kinds here, said so that another compositor can make the same: a Gaussian
/// of deviation `sigma` pixels is three passes each way of a mean over 2r + 1 pixels, with
/// nothing past the picture's edges, and these are the three r (Kovesi's boxes, as
/// `effectcraft-raster` reckons them). Below a deviation of 0.05 there is no blur.
pub fn boxes(sigma: f64) -> [usize; 3] {
    if sigma <= 0.05 {
        return [0; 3];
    }
    const PASSES: f64 = 3.0;
    let ideal = (12.0 * sigma * sigma / PASSES + 1.0).sqrt();
    let mut lower = ideal.floor() as i64;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let upper = lower + 2;
    let lower_f = lower as f64;
    let split = ((12.0 * sigma * sigma - PASSES * lower_f * lower_f - 4.0 * PASSES * lower_f - 3.0 * PASSES) / (-4.0 * lower_f - 4.0)).round() as i64;
    [0, 1, 2].map(|pass| (((if pass < split { lower } else { upper }) - 1) / 2).max(0) as usize)
}

/// How solid a picture is at each pixel.
fn solid(picture: &[Px]) -> Vec<f32> {
    picture.iter().map(|pixel| pixel[3].clamp(0.0, 1.0)).collect()
}

/// One pixel over another, both colour times alpha.
fn over(top: Px, under: Px) -> Px {
    std::array::from_fn(|c| top[c] + under[c] * (1.0 - top[3]))
}

/// `top` laid onto `under` by a Blend's `mode` (plan §8.4), both colour times alpha. These are
/// what a graphics card's blending does, so that every compositor lays a picture on alike:
/// the alpha is always as one over the other. *Add* adds the colours; *Screen* adds each
/// colour less what the two have in common, never past white; *Multiply* darkens what is
/// under it by the top's colour, and over nothing leaves nothing; *Subtract* takes the top's
/// colours away from what is under it. Any other mode is *Normal*: one over the other.
pub fn laid(mode: &str, top: Px, under: Px) -> Px {
    let alpha = top[3] + under[3] * (1.0 - top[3]);
    let colour = |c: usize| match mode {
        "Add" => top[c] + under[c],
        "Screen" => top[c] + under[c] * (1.0 - top[c]),
        "Multiply" => top[c] * under[c] + under[c] * (1.0 - top[3]),
        "Subtract" => under[c] - top[c],
        _ => top[c] + under[c] * (1.0 - top[3]),
    };
    [colour(0), colour(1), colour(2), alpha]
}

/// How far a Gaussian blur of `radius` pixels spreads a picture.
fn spread(radius: f64) -> f64 {
    radius * 1.5 + 1.0
}

/// Whether an effect changes only what goes through it, by what lies no farther round it than
/// it reaches ([`reach`]): then it can be worked out round what is painted alone, and leaves
/// the rest as it is. A blur, a glow or a shadow reaches out past what is painted, a radial
/// blur from the whole picture, and one that is not known here is not said to.
pub(crate) fn confined(effect: &Effect) -> bool {
    #[cfg(test)]
    if WHOLE.get() {
        return false;
    }
    matches!(effect.kind.as_str(), "transparency" | "cutter" | "tone" | "highlight" | iskra_model::BLEND | iskra_model::STYLE)
}

#[cfg(test)]
thread_local! {
    /// For the tests: every effect worked out over the whole picture, as before it was
    /// confined, to hold the two against each other.
    pub(crate) static WHOLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How many pixels of room the effect needs round the picture it is given on `frame`, at
/// `scale`, to be right up to the picture's edges; `side` is the picture's longer side.
pub fn reach(effect: &Effect, frame: u32, scale: f64, side: usize) -> usize {
    let number = |id: &str| effect.number(id, frame);
    let room = match effect.kind.as_str() {
        "blur" => spread(number("radius") * scale),
        "directionalBlur" => number("radius") * scale + 1.0,
        // What is spun or zoomed comes from as far as the amount turns or pulls it, which
        // grows with the distance from the centre: reckoned for the picture's own width.
        "radialBlur" => {
            let part = if effect.choice("kind", frame) == "Zoom" { number("amount") / 200.0 } else { number("amount").to_radians() / 2.0 };
            side as f64 * part.min(0.5)
        }
        // The matte is what is blurred and grown, and it too is given with this room.
        "cutter" => spread(number("blur") * scale) + number("grow").abs() * scale,
        "glow" => spread(number("radius") * scale),
        "dropShadow" => spread(number("softness") * scale) + number("offsetX").abs().max(number("offsetY").abs()) * scale,
        "tone" | "highlight" => spread(number("softness") * scale) + number("width") * scale,
        _ => 0.0,
    };
    // Never so much that a picture could not be held.
    (room.ceil().max(0.0) as usize).min(4096)
}

/// The picture as `effect` changes it on `frame`: for each pixel, in rows from the top, its
/// colour times its alpha, and its alpha, in the working space. A kind that is not known here
/// leaves the picture as it is.
pub(crate) fn apply(effect: &Effect, frame: u32, given: &Given, picture: Vec<Px>) -> Vec<Px> {
    let number = |id: &str| effect.number(id, frame);
    let image = |data: Vec<Px>| given.image(data);
    match effect.kind.as_str() {
        // The whole picture, of the one colour: a card the size of the frame.
        iskra_model::COLOUR_CARD => vec![given.paint(effect.colour("colour", frame)); picture.len()],
        iskra_model::PARTICLES => {
            let place = crate::particles::Place {
                size: given.frame,
                scale: given.scale,
                from_frame: given.from_frame,
                camera: given.camera,
                plane: given.plane,
                stepped: given.stepped,
            };
            crate::particles::draw(effect, frame, (given.width, given.height), &place, given.space, given.sprites, (given.splash_sprites, given.splash_play))
        }
        // What a Blend lays on is the picture, as much of it as the amount says: how it is laid
        // on is its mode's ([`laid`]).
        iskra_model::BLEND => {
            let kept = (number("amount") / 100.0).clamp(0.0, 1.0) as f32;
            picture.into_iter().map(|pixel| pixel.map(|value| value * kept)).collect()
        }
        "transparency" => {
            let kept = (1.0 - number("amount") / 100.0).clamp(0.0, 1.0) as f32;
            // Colour is kept times alpha, so all four fade together.
            picture.into_iter().map(|pixel| pixel.map(|value| value * kept)).collect()
        }
        // Between the picture as the shot shows it and as the style does, by the amount, and
        // where a matte is wired by how solid it is.
        iskra_model::STYLE => {
            let Some((_, styled)) = given.mattes.iter().find(|(port, _)| *port == STYLED) else { return picture };
            let (amount, matte) = ((number("amount") / 100.0).clamp(0.0, 1.0) as f32, given.matte());
            let share = |at: usize| amount * matte.as_ref().map_or(1.0, |matte| matte[at]);
            let mixed = picture.into_iter().zip(styled).enumerate().map(|(at, (shown, styled))| {
                let share = share(at);
                std::array::from_fn(|c| shown[c] + (styled[c] - shown[c]) * share)
            });
            mixed.collect()
        }
        "blur" => {
            let sigma = number("radius") * given.scale / 2.0;
            gaussian_blur(&image(picture), sigma, sigma, false).data
        }
        "directionalBlur" => {
            // The blur's own angle is from straight up, clockwise.
            directional_blur(&image(picture), 90.0 - number("angle"), number("radius") * given.scale * 2.0).data
        }
        "radialBlur" => {
            let centre = given.from_frame.apply(Point::new(number("centreX"), number("centreY")));
            let zoom = effect.choice("kind", frame) == "Zoom";
            let amount = if zoom { number("amount") / 100.0 } else { number("amount") };
            radial_blur(&image(picture), (centre.x, centre.y), amount, zoom).data
        }
        "cutter" => {
            // With no matte wired there is nothing to cut by.
            let Some(matte) = given.matte() else { return picture };
            let cut = given.softened(given.grown(matte, number("grow")), number("blur"));
            let inverted = effect.switch("inverted", frame);
            let kept = |solid: f32| if inverted { solid } else { 1.0 - solid };
            picture.into_iter().zip(cut).map(|(pixel, solid)| pixel.map(|value| value * kept(solid))).collect()
        }
        "glow" => {
            // The picture's shape, spread and coloured, under the picture.
            let (light, strength) = (given.paint(effect.colour("colour", frame)), (number("strength") / 100.0) as f32);
            let spread = given.softened(solid(&picture), number("radius"));
            picture.into_iter().zip(spread).map(|(pixel, by)| over(pixel, light.map(|value| value * (by * strength).min(1.0)))).collect()
        }
        "dropShadow" => {
            let (dark, opacity) = (given.paint(effect.colour("colour", frame)), (number("opacity") / 100.0) as f32);
            let (width, height) = (given.width as i64, given.height as i64);
            let (across, down) = ((number("offsetX") * given.scale).round() as i64, (number("offsetY") * given.scale).round() as i64);
            // The shape as it is that far up and to the left of each pixel: moved aside.
            let shape = solid(&picture);
            let from = |x: i64, y: i64| if x < 0 || y < 0 || x >= width || y >= height { 0.0 } else { shape[(y * width + x) as usize] };
            let moved: Vec<f32> = (0..height).flat_map(|y| (0..width).map(move |x| (x, y))).map(|(x, y)| from(x - across, y - down)).collect();
            let shadow = given.softened(moved, number("softness"));
            picture.into_iter().zip(shadow).map(|(pixel, by)| over(pixel, dark.map(|value| value * by * opacity))).collect()
        }
        "tone" | "highlight" => {
            let (shade, amount) = (given.paint(effect.colour("colour", frame)), (number("amount") / 100.0) as f32);
            let shape = solid(&picture);
            // Where the matte is, a little wider; or with none, a rim inside the picture's
            // edge: the shape less the shape made narrower.
            let region = match given.matte() {
                Some(matte) => given.grown(matte, number("width")),
                None => {
                    let inner = given.grown(shape.clone(), -number("width"));
                    shape.iter().zip(inner).map(|(whole, inner)| (whole - inner).max(0.0)).collect()
                }
            };
            let region = given.softened(region, number("softness"));
            // Laid over the picture only where the picture is: its shape stays as it is.
            let laid = |pixel: Px, by: f32| {
                let part = by * amount * shade[3];
                let colour = |c: usize| if shade[3] > 0.0 { shade[c] / shade[3] } else { 0.0 };
                [0, 1, 2, 3].map(|c| if c == 3 { pixel[3] } else { pixel[c] * (1.0 - part) + colour(c) * pixel[3] * part })
            };
            picture.into_iter().zip(region).map(|(pixel, by)| laid(pixel, by)).collect()
        }
        "colourCorrection" => corrected(effect, frame, given, picture),
        _ => picture,
    }
}

/// How light a colour is to the eye, from its red, green and blue.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// How many tones from black to white a curve is reckoned at; between them it is straight.
pub const CURVE_STEPS: usize = 1024;

/// The picture with its colours corrected: the levels first, then the curves, the hue and how strong the
/// colours are, then how light, then the tint. It is reckoned on the colours as sRGB, whatever
/// the working space, so that a number does the same in either; what is lighter than white
/// stays so, and is not cut off.
fn corrected(effect: &Effect, frame: u32, given: &Given, picture: Vec<Px>) -> Vec<Px> {
    let number = |id: &str| effect.number(id, frame) as f32;
    let black = number("black") / 100.0;
    let span = (number("white") / 100.0 - black).max(0.001);
    let middle = 1.0 / number("middle").max(0.01);
    // A turn round the grey axis, which takes red to green at a third of the way round.
    let (sin, cos) = number("hue").to_radians().sin_cos();
    let (same, part, side) = (cos + (1.0 - cos) / 3.0, (1.0 - cos) / 3.0, sin / 3.0f32.sqrt());
    let (next, last) = (part + side, part - side);
    let strong = 1.0 + number("saturation") / 100.0;
    let light = number("lightness") / 100.0;
    let tint = effect.colour("tint", frame).map(|value| value as f32);
    let glass = tint[3] * number("tintAmount") / 100.0;
    let linear = matches!(given.space, WorkingSpace::Linear);
    // The curves as tables, the master's and each colour's own after it; none where all four
    // are straight. A tone outside black to white goes on from the table's end as it was.
    let curves = effect.curves("curves");
    let tables: Option<[Vec<f32>; 4]> = (!curves.is_straight()).then(|| std::array::from_fn(|at| curves.table(at, CURVE_STEPS)));
    let through = |table: &[f32], tone: f32| {
        let at = tone.clamp(0.0, 1.0) * (CURVE_STEPS - 1) as f32;
        let (low, part) = ((at as usize).min(CURVE_STEPS - 2), at - (at as usize).min(CURVE_STEPS - 2) as f32);
        table[low] + (table[low + 1] - table[low]) * part + (tone - 1.0).max(0.0)
    };
    let one = |pixel: Px| {
        let solid = pixel[3];
        if solid <= 0.0 {
            return pixel;
        }
        let seen = |value: f32| if linear { super::to_srgb(value.max(0.0)) } else { value };
        let mut colour = [seen(pixel[0] / solid), seen(pixel[1] / solid), seen(pixel[2] / solid)];
        colour = colour.map(|value| ((value - black) / span).max(0.0).powf(middle));
        if let Some([master, red, green, blue]) = &tables {
            colour = [through(red, through(master, colour[0])), through(green, through(master, colour[1])), through(blue, through(master, colour[2]))];
        }
        let [red, green, blue] = colour;
        colour = [same * red + last * green + next * blue, next * red + same * green + last * blue, last * red + next * green + same * blue];
        let luma: f32 = colour.iter().zip(LUMA).map(|(value, weight)| value * weight).sum();
        colour = colour.map(|value| luma + (value - luma) * strong);
        colour = colour.map(|value| if light >= 0.0 { value + (1.0 - value).max(0.0) * light } else { value * (1.0 + light) });
        colour = std::array::from_fn(|c| colour[c] * (1.0 + (tint[c] - 1.0) * glass));
        let kept = |value: f32| if linear { super::to_linear(value.max(0.0)) } else { value.max(0.0) };
        [kept(colour[0]) * solid, kept(colour[1]) * solid, kept(colour[2]) * solid, solid]
    };
    picture.into_iter().map(one).collect()
}

/// A plane of values with each the greatest within `radius` of it along its row and then its
/// column, or with `grow` false the least: a shape made wider all round, or narrower. Past
/// the plane's edges there is nothing, which is 0.
fn widened(plane: &[f32], width: usize, height: usize, radius: usize, grow: bool) -> Vec<f32> {
    // One line: for each place the best of the window round it, by a queue of the places
    // that can still be the best, the best first.
    let along = |line: &[f32], out: &mut [f32]| {
        let better = |a: f32, b: f32| if grow { a >= b } else { a <= b };
        let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        let count = line.len();
        for at in 0..count + radius {
            if at < count {
                while queue.back().is_some_and(|last| better(line[at], line[*last])) {
                    queue.pop_back();
                }
                queue.push_back(at);
            }
            let Some(centre) = at.checked_sub(radius) else { continue };
            while queue.front().is_some_and(|first| *first + radius < centre) {
                queue.pop_front();
            }
            let best = queue.front().map_or(0.0, |first| line[*first]);
            // A window that hangs over the edge has nothing there: no shape narrowed past it.
            let over = centre < radius || centre + radius >= count;
            out[centre] = if over && !grow { 0.0f32.min(best) } else { best };
        }
    };
    let mut rows = vec![0.0f32; plane.len()];
    for (line, out) in plane.chunks(width).zip(rows.chunks_mut(width)) {
        along(line, out);
    }
    let mut both = vec![0.0f32; plane.len()];
    let (mut line, mut out) = (vec![0.0f32; height], vec![0.0f32; height]);
    for x in 0..width {
        for y in 0..height {
            line[y] = rows[y * width + x];
        }
        along(&line, &mut out);
        for y in 0..height {
            both[y * width + x] = out[y];
        }
    }
    both
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boxes_that_are_said_are_the_blur_that_is_made() {
        use effectcraft_raster::box_blur;
        // A picture of noise that is the same every time.
        let (width, height) = (40u32, 30u32);
        let mut seed = 7u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        let image = Image { width, height, data: (0..width * height).map(|_| [noise(), noise(), noise(), noise()]).collect() };
        for sigma in [0.3, 1.0, 2.5, 6.0, 11.0] {
            let said = boxes(sigma);
            let by_boxes = said.iter().fold(image.clone(), |made, r| box_blur(&made, *r, *r, 1, false));
            let whole = gaussian_blur(&image, sigma, sigma, false);
            let apart = by_boxes.data.iter().zip(&whole.data).flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (a - b).abs())).fold(0.0, f32::max);
            assert!(apart < 1e-4, "at {sigma}: {said:?}, {apart} apart");
        }
        assert_eq!((boxes(0.0), boxes(0.05)), ([0; 3], [0; 3]));
    }

    #[test]
    fn a_shape_is_made_wider_all_round_and_narrower() {
        // A square of four by four in the middle of twelve by twelve.
        let (side, at) = (12usize, |x: usize, y: usize| y * 12 + x);
        let mut plane = vec![0.0f32; side * side];
        for (x, y) in (4..8).flat_map(|x| (4..8).map(move |y| (x, y))) {
            plane[at(x, y)] = 1.0;
        }
        let count = |plane: &[f32]| plane.iter().filter(|value| **value > 0.5).count();
        let wider = widened(&plane, side, side, 2, true);
        assert_eq!((count(&wider), wider[at(2, 2)], wider[at(1, 5)], wider[at(9, 9)]), (64, 1.0, 0.0, 1.0));
        let narrower = widened(&plane, side, side, 1, false);
        assert_eq!((count(&narrower), narrower[at(5, 5)], narrower[at(4, 5)]), (4, 1.0, 0.0));
        assert_eq!(count(&widened(&plane, side, side, 2, false)), 0, "narrowed by half its width, it is gone");
        // A shape that fills the plane is narrowed from the plane's edges too.
        let full = vec![1.0f32; side * side];
        assert_eq!(count(&widened(&full, side, side, 2, false)), 64);
    }
}
