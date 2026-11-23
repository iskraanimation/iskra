// A Particles node's particles, laid onto a picture of their own as the reference lays them
// (`iskra-render`'s `particles.rs`, `lay` and `stamp`): each a quad round it, in the order
// they are laid, colour times alpha, which the blend state puts over what is there.

struct Told {
    // The size of the picture, in pixels.
    size: vec4<f32>,
    // The marks' colour, times alpha, in the working space.
    colour: vec4<f32>,
    // The tint's colour, and how much of it a sprite takes.
    tint: vec4<f32>,
};

struct Item {
    // The four corners of the quad round it, in the picture's pixels.
    corners: array<vec4<f32>, 2>,
    // The pixels it may touch: from x, y to before z, w.
    clip: vec4<f32>,
    // 0 for a mark, 1 for a sprite; how solid it is.
    kind: vec4<f32>,
    // A mark: from, to; its radius, taper and softness.
    line: vec4<f32>,
    shape: vec4<f32>,
    // A sprite: from the picture's pixels to the sprite's (a, b, c, d; e, f), the sprite's
    // first pixel in its own; and where it lies in the atlas, and its size.
    to_sprite: vec4<f32>,
    moved: vec4<f32>,
    atlas: vec4<f32>,
};

@group(0) @binding(0) var<uniform> told: Told;
@group(0) @binding(1) var<storage, read> items: array<Item>;
@group(0) @binding(2) var atlas: texture_2d<f32>;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) item: u32,
};

@vertex
fn vs(@builtin(vertex_index) index: u32, @builtin(instance_index) item: u32) -> Out {
    // Two triangles: 0, 1, 2 and 0, 2, 3.
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let corner = order[index];
    let pair = items[item].corners[corner / 2u];
    let at = select(pair.xy, pair.zw, corner % 2u == 1u);
    var out: Out;
    out.position = vec4<f32>(at.x / told.size.x * 2.0 - 1.0, 1.0 - at.y / told.size.y * 2.0, 0.0, 1.0);
    out.item = item;
    return out;
}

// A pixel of the sprite, by its place in it: nothing outside it.
fn pixel(item: Item, x: i32, y: i32) -> vec4<f32> {
    let size = vec2<i32>(item.atlas.zw);
    if (x < 0 || y < 0 || x >= size.x || y >= size.y) {
        return vec4<f32>(0.0);
    }
    return textureLoad(atlas, vec2<i32>(item.atlas.xy) + vec2<i32>(x, y), 0);
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let item = items[in.item];
    let p = in.position.xy;
    let whole = floor(p);
    if (whole.x < item.clip.x || whole.y < item.clip.y || whole.x >= item.clip.z || whole.y >= item.clip.w) {
        discard;
    }
    if (item.kind.x < 0.5) {
        // How much of the pixel the mark covers: how far its middle is from the line, fading
        // over the softness across its edge, half inside and half out.
        let start = item.line.xy;
        let along = item.line.zw - start;
        let soft = max(item.shape.z, 1e-6);
        let off = p - start;
        let long = dot(along, along);
        var part = 0.0;
        if (long > 0.0) {
            part = clamp(dot(off, along) / long, 0.0, 1.0);
        }
        let distance = length(off - along * part);
        let here = item.shape.x * (1.0 - part * (1.0 - item.shape.y));
        let radius = max(here, 0.5);
        let covered = clamp((radius + soft / 2.0 - distance) / soft, 0.0, 1.0) * (here / radius) * item.kind.y;
        if (covered <= 0.0) {
            discard;
        }
        return told.colour * covered;
    }
    // The sprite's colour at the point of it here, between the four pixels round it.
    let m = item.to_sprite;
    let at = vec2<f32>(m.x * p.x + m.z * p.y + item.moved.x, m.y * p.x + m.w * p.y + item.moved.y);
    let u = at - vec2<f32>(0.5) - item.moved.zw;
    let low = floor(u);
    let f = u - low;
    let x = i32(low.x);
    let y = i32(low.y);
    let top = mix(pixel(item, x, y), pixel(item, x + 1, y), f.x);
    let bottom = mix(pixel(item, x, y + 1), pixel(item, x + 1, y + 1), f.x);
    let colour = top * (1.0 - f.y) + bottom * f.y;
    if (colour.a <= 0.0) {
        discard;
    }
    // Its own colour, taking as much of the tint's as told, as solid as it is.
    let tinted = told.tint.w;
    let rgb = colour.rgb * (1.0 - tinted) + told.tint.rgb * colour.a * tinted;
    return vec4<f32>(rgb, colour.a) * item.kind.y;
}
