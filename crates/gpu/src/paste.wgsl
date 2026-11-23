// What went through an effect node, put into the picture it belongs to: colour times alpha,
// which the blend state puts over what is there. The picture of the effect was made with room
// round it, which is left out here, or is a part of the picture away from its corner.

struct Paste {
    // The pixel of what is pasted that the picture's first pixel is.
    offset: vec2<f32>,
    // 0: `made` as it is. 1: a glow, `made` over a light as wide as `spread` is solid.
    // 2: `made` faded, `strength` of it left. 3: a style, `strength` of the way from `made`
    // to `spread`, the picture in the style, and where `light.x` says a matte is wired by how
    // solid `matte` is. 4: a cutter's, `made` cut by how solid `spread`, its matte, is: kept
    // where the matte is with `strength` 1, inverted, and where it is not with 0. 5: a Colour
    // Card's, `light` all over. 6: a Tone's or a Highlight's, `light` laid over `made` where
    // it is, as much as `spread` is solid times `strength`.
    kind: f32,
    strength: f32,
    // The glow's colour times how solid it is, in the working space.
    light: vec4<f32>,
};

@group(0) @binding(0) var<uniform> told: Paste;
@group(0) @binding(1) var made: texture_2d<f32>;
@group(0) @binding(2) var spread: texture_2d<f32>;
@group(0) @binding(3) var matte: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(corners[index], 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    if (told.kind > 4.5 && told.kind < 5.5) {
        return told.light;
    }
    let at = vec2<i32>(position.xy) + vec2<i32>(told.offset);
    // Outside what was made there is nothing to put: what is there stays.
    let size = vec2<i32>(textureDimensions(made));
    if (any(at < vec2<i32>(0)) || any(at >= size)) {
        return vec4<f32>(0.0);
    }
    let pixel = textureLoad(made, at, 0);
    if (told.kind < 0.5) {
        return pixel;
    }
    if (told.kind > 5.5) {
        let part = clamp(textureLoad(spread, at, 0).a, 0.0, 1.0) * told.strength * told.light.a;
        var colour = vec3<f32>(0.0);
        if (told.light.a > 0.0) {
            colour = told.light.rgb / told.light.a;
        }
        return vec4<f32>(pixel.rgb * (1.0 - part) + colour * pixel.a * part, pixel.a);
    }
    if (told.kind > 3.5) {
        let solid = clamp(textureLoad(spread, at, 0).a, 0.0, 1.0);
        return pixel * select(1.0 - solid, solid, told.strength > 0.5);
    }
    if (told.kind > 2.5) {
        var share = told.strength;
        if (told.light.x > 0.5) {
            share = share * clamp(textureLoad(matte, at, 0).a, 0.0, 1.0);
        }
        return mix(pixel, textureLoad(spread, at, 0), share);
    }
    if (told.kind > 1.5) {
        // Colour is kept times alpha, so all four fade together.
        return pixel * told.strength;
    }
    let by = min(clamp(textureLoad(spread, at, 0).a, 0.0, 1.0) * told.strength, 1.0);
    return pixel + told.light * by * (1.0 - pixel.a);
}
