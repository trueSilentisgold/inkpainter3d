// Turns the composited channels of a chunk into the texture set's channel textures: adds
// Painter's defaults where the stack left nothing, and derives normals from the height
// with Painter's Sobel filter, combined with the normal channel.

// Must match ResolveUniforms in compositor.rs.
struct Resolve {
    area: vec4<i32>,       // xy: texture set texel at chunk texel (0, 0), z: resolution
    params: vec4<f32>,     // x: height factor (Painter's HEIGHT_FACTOR)
    base_color: vec4<f32>, // default base color, linear
    surface: vec4<f32>,    // defaults: roughness, metallic, opacity, height
}

@group(0) @binding(0) var<uniform> resolve: Resolve;
// The finished channels in Channel::ALL order: premultiplied, normals straight.
@group(0) @binding(1) var channels: texture_2d_array<f32>;

const BASE_COLOR: i32 = 0;
const HEIGHT: i32 = 1;
const ROUGHNESS: i32 = 2;
const METALLIC: i32 = 3;
const NORMAL: i32 = 4;
const EMISSIVE: i32 = 5;
const OPACITY: i32 = 6;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(xy, 0.0, 1.0);
}

struct Outputs {
    @location(0) base: vec4<f32>,     // base color and opacity
    @location(1) normal: vec4<f32>,   // tangent-space normal, OpenGL style; a: height * 0.5 + 0.5
    @location(2) surface: vec4<f32>,  // roughness, metallic, ambient occlusion
    @location(3) emissive: vec4<f32>,
}

fn channel(local: vec2<i32>, index: i32) -> vec4<f32> {
    return textureLoad(channels, local, index, 0);
}

// A premultiplied scalar with the default where it is empty.
fn scalar(local: vec2<i32>, index: i32, default_value: f32) -> f32 {
    let v = channel(local, index);
    return v.x + default_value * (1.0 - v.w);
}

fn height(local: vec2<i32>) -> f32 {
    return scalar(local, HEIGHT, resolve.surface.w);
}

fn fade_normal(n: vec3<f32>, amount: f32) -> vec3<f32> {
    if amount >= 1.0 || n.z >= 1.0 {
        return n;
    }
    let phi = amount * acos(clamp(n.z, -1.0, 1.0));
    let xy = n.xy / max(sqrt(max(1.0 - n.z * n.z, 0.0)), 1e-6) * sin(phi);
    return vec3<f32>(xy, cos(phi));
}

fn reoriented(base: vec3<f32>, over: vec3<f32>) -> vec3<f32> {
    let b = base + vec3<f32>(0.0, 0.0, 1.0);
    let o = vec3<f32>(-over.xy, over.z);
    return normalize(b * dot(b, o) - o * b.z);
}

@fragment
fn fs_resolve(@builtin(position) frag: vec4<f32>) -> Outputs {
    let local = vec2<i32>(frag.xy) - resolve.area.xy;
    var out: Outputs;

    let base = channel(local, BASE_COLOR);
    let opacity = scalar(local, OPACITY, resolve.surface.z);
    out.base = vec4<f32>(
        clamp(base.rgb + resolve.base_color.rgb * (1.0 - base.w), vec3<f32>(0.0), vec3<f32>(1.0)),
        clamp(opacity, 0.0, 1.0),
    );
    out.surface = vec4<f32>(
        clamp(scalar(local, ROUGHNESS, resolve.surface.x), 0.0, 1.0),
        clamp(scalar(local, METALLIC, resolve.surface.y), 0.0, 1.0),
        1.0,
        1.0,
    );
    out.emissive = vec4<f32>(clamp(channel(local, EMISSIVE).rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);

    // Painter's normalFromHeight, Smooth (Sobel), one texel apart. Texel rows go down the
    // image while the tangent space's y goes up: "top" is the row above.
    let h_l = height(local + vec2<i32>(-1, 0));
    let h_r = height(local + vec2<i32>(1, 0));
    let h_t = height(local + vec2<i32>(0, -1));
    let h_b = height(local + vec2<i32>(0, 1));
    let h_lt = height(local + vec2<i32>(-1, -1));
    let h_rt = height(local + vec2<i32>(1, -1));
    let h_lb = height(local + vec2<i32>(-1, 1));
    let h_rb = height(local + vec2<i32>(1, 1));
    let size = f32(resolve.area.z);
    let dh = 0.5 * size * vec2<f32>(
        2.0 * (h_l - h_r) + h_lt - h_rt + h_lb - h_rb,
        2.0 * (h_b - h_t) + h_rb - h_rt + h_lb - h_lt,
    );
    let from_height = normalize(vec3<f32>(dh, resolve.params.x));

    // The normal channel: straight, faded towards flat by its coverage.
    let painted = channel(local, NORMAL);
    var n = from_height;
    if painted.w > 0.0 {
        var c = normalize(painted.xyz * 2.0 - 1.0);
        c.z = max(c.z, 1e-3);
        n = reoriented(from_height, fade_normal(normalize(c), painted.w));
    }
    out.normal = vec4<f32>(n * 0.5 + 0.5, clamp(height(local) * 0.5 + 0.5, 0.0, 1.0));
    return out;
}
