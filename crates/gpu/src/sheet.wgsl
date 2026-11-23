// What a piece is read from, for a drawing kept on the card as it was painted (plan §4.8):
// its art layers, the bottom first, each a texture as large as the others. An indexed layer
// holds for each pixel a swatch's id and how much of the pixel it covers, and the swatch's
// colour is looked up here, in the palette; a layer of free colour holds its colours. So
// another palette, or a swatch that changes, is another small texture and no drawing is sent
// again. This takes the place of the part of `layer.wgsl` between its `held` marks.
//
// The layers are put over each other as `Drawing::to_rgba8` puts them, at 8 bits a channel
// with the same rounding, so that the picture is the reference's.

struct Sheet {
    // What each of the four places holds: 0 nothing, 1 an indexed layer, 2 free colour.
    kinds: vec4<u32>,
};

@group(0) @binding(1) var art0: texture_2d<u32>;
@group(0) @binding(2) var art1: texture_2d<u32>;
@group(0) @binding(3) var art2: texture_2d<u32>;
@group(0) @binding(4) var art3: texture_2d<u32>;
// The palette: the colour of swatch `id` is at (id mod 256, id / 256), straight, 0 to 255.
@group(0) @binding(5) var palette: texture_2d<u32>;
@group(0) @binding(6) var<uniform> sheet: Sheet;

fn held() -> vec2<i32> {
    return vec2<i32>(textureDimensions(art0));
}

fn art(place: u32, inside: vec2<i32>) -> vec4<u32> {
    switch place {
        case 0u { return textureLoad(art0, inside, 0); }
        case 1u { return textureLoad(art1, inside, 0); }
        case 2u { return textureLoad(art2, inside, 0); }
        default { return textureLoad(art3, inside, 0); }
    }
}

// `top` over `under`, both straight colours of 0 to 255 a channel, rounded to whole numbers.
fn over(top: vec4<f32>, under: vec4<f32>) -> vec4<f32> {
    let a = top.a / 255.0;
    let b = under.a / 255.0;
    let alpha = a + b * (1.0 - a);
    if (alpha <= 0.0) {
        return vec4<f32>(0.0);
    }
    let colour = floor((top.rgb * a + under.rgb * b * (1.0 - a)) / alpha + vec3<f32>(0.5));
    return vec4<f32>(colour, floor(alpha * 255.0 + 0.5));
}

fn straight_at(inside: vec2<i32>) -> vec4<f32> {
    var all = vec4<f32>(0.0);
    for (var place = 0u; place < 4u; place++) {
        let kind = sheet.kinds[place];
        if (kind == 0u) {
            continue;
        }
        let kept = art(place, inside);
        var top = vec4<f32>(0.0);
        if (kind == 2u) {
            top = vec4<f32>(kept);
        } else if (kept.g > 0u) {
            // The swatch's colour, as much of it as the pixel is covered. An onion skin's ghost
            // is what is covered, whatever the swatch's alpha, as the window's tiles have it.
            let colour = vec4<f32>(textureLoad(palette, vec2<i32>(i32(kept.r & 255u), i32(kept.r >> 8u)), 0));
            let alpha = select(colour.a, 255.0, ghosted());
            top = vec4<f32>(colour.rgb, floor(alpha * (f32(kept.g) / 65535.0) + 0.5));
        }
        all = over(top, all);
    }
    return all / 255.0;
}
