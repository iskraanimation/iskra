// A Colour Correction as the reference makes it (`iskra_render`'s `corrected`): each pixel
// changed alone, by its colour without its alpha, in sRGB whatever the working space. Levels
// and the middle, the curves, a turn of the hue, the saturation, the lightness and the tint,
// in that order.

struct Correction {
    // Levels: the black, from black to white, and one over the middle.
    black: f32,
    span: f32,
    middle: f32,
    // The saturation, 1 for as it is.
    strong: f32,
    // The lightness, -1 to 1; how much of the tint; 1 in linear light; 1 where the curves are
    // not all straight.
    light: f32,
    glass: f32,
    linear: f32,
    curved: f32,
    // The turn of the hue: how much of a colour stays itself, and of the next and the last.
    same: f32,
    next: f32,
    last: f32,
    unused: f32,
    // The tint's colour.
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> told: Correction;
@group(0) @binding(1) var picture: texture_2d<f32>;
// The curves as tables along a row: the master's in red, and each colour's own in green, blue
// and alpha.
@group(0) @binding(2) var curves: texture_2d<f32>;

const LUMA = vec3<f32>(0.2126, 0.7152, 0.0722);

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle that covers the whole picture.
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(corners[index], 0.0, 1.0);
}

fn to_srgb(value: f32) -> f32 {
    if (value <= 0.0031308) {
        return value * 12.92;
    }
    return 1.055 * pow(value, 1.0 / 2.4) - 0.055;
}

fn to_linear(value: f32) -> f32 {
    if (value <= 0.04045) {
        return value / 12.92;
    }
    return pow((value + 0.055) / 1.055, 2.4);
}

// A tone through the curve in channel `at` of the tables: between its steps straight, and
// past white on as it was.
fn through(at: i32, tone: f32) -> f32 {
    let steps = i32(textureDimensions(curves).x);
    let place = clamp(tone, 0.0, 1.0) * f32(steps - 1);
    let low = min(i32(place), steps - 2);
    let part = place - f32(low);
    let a = textureLoad(curves, vec2<i32>(low, 0), 0)[at];
    let b = textureLoad(curves, vec2<i32>(low + 1, 0), 0)[at];
    return a + (b - a) * part + max(tone - 1.0, 0.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = textureLoad(picture, vec2<i32>(position.xy), 0);
    let solid = pixel.a;
    if (solid <= 0.0) {
        return pixel;
    }
    var colour = pixel.rgb / solid;
    if (told.linear > 0.5) {
        colour = vec3<f32>(to_srgb(max(colour.r, 0.0)), to_srgb(max(colour.g, 0.0)), to_srgb(max(colour.b, 0.0)));
    }
    let levelled = max((colour - vec3<f32>(told.black)) / told.span, vec3<f32>(0.0));
    // Nothing to the middle's power is nothing.
    colour = select(pow(levelled, vec3<f32>(told.middle)), vec3<f32>(0.0), levelled <= vec3<f32>(0.0));
    if (told.curved > 0.5) {
        colour = vec3<f32>(through(1, through(0, colour.r)), through(2, through(0, colour.g)), through(3, through(0, colour.b)));
    }
    colour = vec3<f32>(
        told.same * colour.r + told.last * colour.g + told.next * colour.b,
        told.next * colour.r + told.same * colour.g + told.last * colour.b,
        told.last * colour.r + told.next * colour.g + told.same * colour.b,
    );
    let luma = dot(colour, LUMA);
    colour = vec3<f32>(luma) + (colour - vec3<f32>(luma)) * told.strong;
    if (told.light >= 0.0) {
        colour = colour + max(vec3<f32>(1.0) - colour, vec3<f32>(0.0)) * told.light;
    } else {
        colour = colour * (1.0 + told.light);
    }
    colour = colour * (vec3<f32>(1.0) + (told.tint.rgb - vec3<f32>(1.0)) * told.glass);
    colour = max(colour, vec3<f32>(0.0));
    if (told.linear > 0.5) {
        colour = vec3<f32>(to_linear(colour.r), to_linear(colour.g), to_linear(colour.b));
    }
    return vec4<f32>(colour * solid, solid);
}
