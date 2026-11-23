// A plane of values made wider or narrower as the reference makes it (`iskra_render`'s
// `widened`), one way: each value the greatest, or the least, within `radius` pixels of it
// along a row or a column. Past the plane's edges there is nothing, which is 0. The values
// are how solid the plane is, its alpha. In a rim's pass, what is left is the original less
// that: the band inside its edge.

struct Widen {
    // The step from one pixel to the next: (1, 0) along a row, (0, 1) along a column.
    step: vec2<f32>,
    radius: f32,
    // 1 for the greatest, 0 for the least.
    grow: f32,
    // 1 for the original less what this pass makes.
    rim: f32,
    unused: f32,
    unused2: f32,
    unused3: f32,
};

@group(0) @binding(0) var<uniform> told: Widen;
@group(0) @binding(1) var plane: texture_2d<f32>;
@group(0) @binding(2) var original: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle that covers the whole picture.
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(corners[index], 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(position.xy);
    let size = vec2<i32>(textureDimensions(plane));
    let step = vec2<i32>(told.step);
    let radius = i32(told.radius);
    let grow = told.grow > 0.5;
    var best = select(1.0e30, 0.0, grow);
    for (var by = -radius; by <= radius; by++) {
        let read = at + step * by;
        var value = 0.0;
        if (all(read >= vec2<i32>(0)) && all(read < size)) {
            value = clamp(textureLoad(plane, read, 0).a, 0.0, 1.0);
        }
        best = select(min(best, value), max(best, value), grow);
    }
    if (told.rim > 0.5) {
        best = max(clamp(textureLoad(original, at, 0).a, 0.0, 1.0) - best, 0.0);
    }
    return vec4<f32>(best);
}
