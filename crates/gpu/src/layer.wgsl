// One piece of a drawing, drawn into the picture as the reference compositor samples it
// (`iskra-render`): each pixel of the picture is the mean of `points` × `points` places in
// it, each read between the four texels around it. Colours come out times alpha, in the
// working space, and the blend state puts them over what is there.

struct Piece {
    // From the picture back to the drawing: the three rows of a matrix applied to (x, y, 1)
    // of a point of the picture. For a drawing that faces the camera squarely the last row is
    // (0, 0, 1); for a tilted one, the first two give the drawing's point times the third,
    // which is one over how far in front of the camera the point is.
    back_x: vec4<f32>,
    back_y: vec4<f32>,
    back_s: vec4<f32>,
    // For a flat drawing, from its pixels to the picture, as (m0, m1, m2, m3) and in
    // `moves.zw` (m4, m5): x' = m0·x + m2·y + m4.
    to_picture: vec4<f32>,
    moves: vec4<f32>,
    // What is drawn: x0, y0, x1, y1. Of a flat drawing, in its pixels; of a tilted one, the
    // pixels of the picture that it can reach.
    part: vec4<f32>,
    // The drawing pixel that the texture's first texel is, and the picture's size.
    origin: vec2<f32>,
    picture: vec2<f32>,
    // Points a side, and 1 where layers are blended in linear light.
    points: f32,
    linear: f32,
    // 1 for a drawing seen in perspective.
    tilted: f32,
    // How near the camera a point may be and still be seen.
    nearest: f32,
    // How the window looks at it where that is not as the shot shows it. `tint.rgb` is a
    // colour in the working space that the drawing is drawn in alone, wherever anything is
    // painted, if `tint.a` is 1: an onion skin's ghost. `fade.x` is how much of it is there.
    tint: vec4<f32>,
    fade: vec4<f32>,
    // Of a tilted drawing in pieces, the part of the drawing whose pixels this piece puts in,
    // so that where the pieces overlap each pixel is put in once: x0, y0, x1, y1.
    own: vec4<f32>,
};

@group(0) @binding(0) var<uniform> piece: Piece;

// <held>
// What the piece is read from: here a texture of its colours, flattened and sent as they
// are. `sheet.wgsl` takes this part's place for a drawing that is kept as it was painted.
@group(0) @binding(1) var pixels: texture_2d<f32>;

// How many texels are held, each way.
fn held() -> vec2<i32> {
    return vec2<i32>(textureDimensions(pixels));
}

// The straight colour of a texel that is held, each channel from 0 to 1.
fn straight_at(inside: vec2<i32>) -> vec4<f32> {
    return textureLoad(pixels, inside, 0);
}
// </held>

// Whether the piece is an onion skin's ghost: drawn in one colour wherever it covers.
fn ghosted() -> bool {
    return piece.tint.a > 0.5;
}

fn apply(m: vec4<f32>, t: vec2<f32>, p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(m.x * p.x + m.z * p.y + t.x, m.y * p.x + m.w * p.y + t.y);
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 6>(
        piece.part.xy, piece.part.zy, piece.part.zw,
        piece.part.xy, piece.part.zw, piece.part.xw,
    );
    var at = corners[index];
    if (piece.tilted < 0.5) {
        at = apply(piece.to_picture, piece.moves.zw, corners[index]);
    }
    return vec4<f32>(at.x / piece.picture.x * 2.0 - 1.0, 1.0 - at.y / piece.picture.y * 2.0, 0.0, 1.0);
}

fn to_linear(value: vec3<f32>) -> vec3<f32> {
    let low = value / 12.92;
    let high = pow((value + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, value <= vec3<f32>(0.04045));
}

// The drawing's pixel at (x, y): colour times alpha, in the working space. Clear outside
// what the texture holds.
fn texel(at: vec2<i32>) -> vec4<f32> {
    let size = held();
    let inside = at - vec2<i32>(piece.origin);
    if (inside.x < 0 || inside.y < 0 || inside.x >= size.x || inside.y >= size.y) {
        return vec4<f32>(0.0);
    }
    let straight = straight_at(inside);
    var color = straight.rgb;
    if (piece.linear > 0.5) {
        color = to_linear(color);
    }
    return vec4<f32>(color * straight.a, straight.a);
}

// The colour at a point of the drawing, between the four pixels around it.
fn between(point: vec2<f32>) -> vec4<f32> {
    let start = point - vec2<f32>(0.5);
    let corner = floor(start);
    let part = start - corner;
    let at = vec2<i32>(corner);
    let top = mix(texel(at), texel(at + vec2<i32>(1, 0)), part.x);
    let bottom = mix(texel(at + vec2<i32>(0, 1)), texel(at + vec2<i32>(1, 1)), part.x);
    return mix(top, bottom, part.y);
}

// The point of the drawing seen at a point of the picture, and whether it is seen there.
fn back_of(place: vec2<f32>) -> vec3<f32> {
    let here = vec3<f32>(place, 1.0);
    return vec3<f32>(dot(piece.back_x.xyz, here), dot(piece.back_y.xyz, here), dot(piece.back_s.xyz, here));
}

fn owned(point: vec2<f32>) -> bool {
    return all(point >= piece.own.xy) && all(point < piece.own.zw);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = floor(position.xy);
    let points = u32(piece.points);
    // A tilted drawing in pieces: a pixel is put in by the piece that has the point seen at
    // its middle, all of it, as a flat one is by the piece it is in. Two pieces' shares of one
    // pixel, each put over the other, would not be the pixel.
    var whole = false;
    if (piece.tilted > 0.5) {
        let middle = back_of(pixel + vec2<f32>(0.5));
        if (middle.z > 0.0 && middle.z * piece.nearest < 1.0) {
            if (!owned(middle.xy / middle.z)) {
                return vec4<f32>(0.0);
            }
            whole = true;
        }
    }
    var sum = vec4<f32>(0.0);
    for (var i = 0u; i < points; i++) {
        for (var j = 0u; j < points; j++) {
            let place = pixel + (vec2<f32>(f32(i), f32(j)) + vec2<f32>(0.5)) / piece.points;
            let back = back_of(place);
            if (piece.tilted < 0.5) {
                sum += between(back.xy);
            } else if (back.z > 0.0 && back.z * piece.nearest < 1.0) {
                // A point at which the card is behind the camera adds nothing. Where the
                // middle of the pixel is behind it, each piece puts in its own points.
                let point = back.xy / back.z;
                if (whole || owned(point)) {
                    sum += between(point);
                }
            }
        }
    }
    var seen = sum / (piece.points * piece.points);
    if (piece.tint.a > 0.5) {
        seen = vec4<f32>(piece.tint.rgb * seen.a, seen.a);
    }
    return seen * piece.fade.x;
}
