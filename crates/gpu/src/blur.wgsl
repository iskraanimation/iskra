// A blur as the reference compositor makes it (`iskra_render::boxes`), one way: along a row
// or along a column. The reference's three passes of a mean are folded into the one kernel
// they come to, and that is read two pixels at a time: between two pixels the card gives
// their mix, so each pair is one reading, placed where the mix is in the pair's own
// proportion.

struct Pass {
    // The step from one pixel to the next: (1, 0) along a row, (0, 1) along a column.
    step: vec2<f32>,
    // How much of the pixel itself, and how many readings there are to each side of it.
    middle: f32,
    count: f32,
    // For each reading: how far from the pixel it is, in pixels, and how much it counts.
    taps: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var<uniform> told: Pass;
@group(0) @binding(1) var picture: texture_2d<f32>;
@group(0) @binding(2) var between: sampler;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle that covers the whole picture.
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(corners[index], 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(picture));
    let at = position.xy / size;
    let along = told.step / size;
    var sum = textureSampleLevel(picture, between, at, 0.0) * told.middle;
    let count = u32(told.count);
    for (var i = 0u; i < count; i++) {
        let tap = told.taps[i];
        sum += (textureSampleLevel(picture, between, at + along * tap.x, 0.0) + textureSampleLevel(picture, between, at - along * tap.x, 0.0)) * tap.y;
    }
    return sum;
}
