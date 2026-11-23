// A drawing kept as it was painted, bent by a deformer (plan §4.5) on the card, as
// `iskra_render::bend` bends it on the processor: each triangle of the mesh is drawn where the
// deformer puts it, showing the part of the drawing it has at rest, read between its pixels
// as colour times alpha. What comes out is straight colour in sRGB, rounded to 8 bits as the
// reference rounds it, and is drawn from there on as any flattened drawing is. A point that
// two triangles cover shows the first: each is nearer than the ones after it.
//
// It follows `sheet.wgsl`, which reads the art layers and the palette.

struct Bend {
    // The bent picture's first pixel and its size, in drawing pixels.
    bent: vec4<f32>,
    // The drawing pixel that the art layers' first texel is; and in `z`, 1 for an onion
    // skin's ghost.
    origin: vec4<f32>,
};

@group(0) @binding(0) var<uniform> bend: Bend;

struct Corner {
    @builtin(position) position: vec4<f32>,
    // Where the point is in the drawing at rest.
    @location(0) rest: vec2<f32>,
};

@vertex
fn vs(@location(0) posed: vec2<f32>, @location(1) rest: vec2<f32>, @location(2) order: f32) -> Corner {
    let at = posed - bend.bent.xy;
    return Corner(vec4<f32>(at.x / bend.bent.z * 2.0 - 1.0, 1.0 - at.y / bend.bent.w * 2.0, order, 1.0), rest);
}

// The drawing's pixel at (x, y), colour times alpha in sRGB; clear outside what is held.
// A ghost is bent as what it covers: what draws it later tints it.
fn ghosted() -> bool {
    return bend.origin.z > 0.5;
}

fn texel(at: vec2<i32>) -> vec4<f32> {
    let size = held();
    let inside = at - vec2<i32>(bend.origin.xy);
    if (inside.x < 0 || inside.y < 0 || inside.x >= size.x || inside.y >= size.y) {
        return vec4<f32>(0.0);
    }
    let straight = straight_at(inside);
    return vec4<f32>(straight.rgb * straight.a, straight.a);
}

@fragment
fn fs(corner: Corner) -> @location(0) vec4<f32> {
    let start = corner.rest - vec2<f32>(0.5);
    let low = floor(start);
    let part = start - low;
    let at = vec2<i32>(low);
    let top = mix(texel(at), texel(at + vec2<i32>(1, 0)), part.x);
    let bottom = mix(texel(at + vec2<i32>(0, 1)), texel(at + vec2<i32>(1, 1)), part.x);
    let seen = mix(top, bottom, part.y);
    if (seen.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    let colour = floor(clamp(seen.rgb / seen.a, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0 + vec3<f32>(0.5));
    return vec4<f32>(colour, floor(clamp(seen.a, 0.0, 1.0) * 255.0 + 0.5)) / 255.0;
}
