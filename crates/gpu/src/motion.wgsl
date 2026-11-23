// A Directional and a Radial Blur as the reference makes them (`effectcraft-raster`'s
// `directional_blur` and `radial_blur`): each pixel the mean of `count` readings between the
// pixels, along a line through it, or round the centre, or towards it and away. Past the
// picture's edges there is nothing.

struct Motion {
    // 0 along a line, 1 spun round the centre, 2 zoomed from it.
    kind: f32,
    count: f32,
    // Along a line: the step's direction, and how long the line is, in pixels.
    along: vec2<f32>,
    length: f32,
    // Spun or zoomed: by how much (degrees spun, or the part of the distance zoomed), round
    // the centre, in pixels.
    amount: f32,
    centre: vec2<f32>,
};

@group(0) @binding(0) var<uniform> told: Motion;
@group(0) @binding(1) var picture: texture_2d<f32>;
@group(0) @binding(2) var unused: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle that covers the whole picture.
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(corners[index], 0.0, 1.0);
}

// A pixel, nothing past the edges.
fn pixel(at: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(picture));
    if (any(at < vec2<i32>(0)) || any(at >= size)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(picture, at, 0);
}

// The picture between its pixels at `at`, where a pixel's centre is at its corner plus a half.
fn between(at: vec2<f32>) -> vec4<f32> {
    let place = at - vec2<f32>(0.5);
    let corner = floor(place);
    let part = place - corner;
    let first = vec2<i32>(corner);
    let top = mix(pixel(first), pixel(first + vec2<i32>(1, 0)), part.x);
    let bottom = mix(pixel(first + vec2<i32>(0, 1)), pixel(first + vec2<i32>(1, 1)), part.x);
    return mix(top, bottom, part.y);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // The pixel's centre, as `position` is.
    let here = position.xy;
    let count = i32(told.count);
    let from_centre = here - told.centre;
    var sum = vec4<f32>(0.0);
    for (var at = 0; at < count; at++) {
        let t = f32(at) / f32(count - 1) - 0.5;
        var read = here + told.along * t * told.length;
        if (told.kind > 1.5) {
            read = told.centre + from_centre * (1.0 + t * told.amount);
        } else if (told.kind > 0.5) {
            let turn = radians(t * told.amount);
            let s = sin(turn);
            let c = cos(turn);
            read = told.centre + vec2<f32>(from_centre.x * c - from_centre.y * s, from_centre.x * s + from_centre.y * c);
        }
        sum += between(read);
    }
    return sum / f32(count);
}
