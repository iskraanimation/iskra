// A frame made ready to be shown: from colour times alpha in the working space, 16-bit
// floats, to what a window draws: sRGB colour times alpha, 8 bits a channel.

struct Shown {
    // 1 where the frame is in linear light.
    linear: f32,
};

@group(0) @binding(0) var<uniform> shown: Shown;
@group(0) @binding(1) var frame: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle that covers the whole picture.
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(corners[index], 0.0, 1.0);
}

fn to_srgb(value: vec3<f32>) -> vec3<f32> {
    let low = value * 12.92;
    let high = 1.055 * pow(value, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(high, low, value <= vec3<f32>(0.0031308));
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let made = textureLoad(frame, vec2<i32>(position.xy), 0);
    if (made.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    let alpha = clamp(made.a, 0.0, 1.0);
    var color = clamp(made.rgb / made.a, vec3<f32>(0.0), vec3<f32>(1.0));
    if (shown.linear > 0.5) {
        color = to_srgb(color);
    }
    return vec4<f32>(color * alpha, alpha);
}
