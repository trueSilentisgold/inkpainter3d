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
// For every texel, the nearest texel inside a UV island (paint_seeds.wgsl).
@group(0) @binding(2) var seeds: texture_2d<u32>;

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

// A texel's channels with the defaults filled in, as the viewport and the export see them.
struct Texel {
    base: vec4<f32>,     // linear base color and opacity
    normal: vec3<f32>,   // unit tangent-space normal, OpenGL style, with the height's detail
    height: f32,
    surface: vec2<f32>,  // roughness, metallic
    emissive: vec3<f32>, // linear
}

fn resolved(local: vec2<i32>) -> Texel {
    var t: Texel;
    let base = channel(local, BASE_COLOR);
    let opacity = scalar(local, OPACITY, resolve.surface.z);
    t.base = vec4<f32>(
        clamp(base.rgb + resolve.base_color.rgb * (1.0 - base.w), vec3<f32>(0.0), vec3<f32>(1.0)),
        clamp(opacity, 0.0, 1.0),
    );
    t.surface = vec2<f32>(
        clamp(scalar(local, ROUGHNESS, resolve.surface.x), 0.0, 1.0),
        clamp(scalar(local, METALLIC, resolve.surface.y), 0.0, 1.0),
    );
    t.emissive = clamp(channel(local, EMISSIVE).rgb, vec3<f32>(0.0), vec3<f32>(1.0));

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
    t.normal = n;
    t.height = height(local);
    return t;
}

@fragment
fn fs_resolve(@builtin(position) frag: vec4<f32>) -> Outputs {
    let t = resolved(vec2<i32>(frag.xy) - resolve.area.xy);
    var out: Outputs;
    out.base = t.base;
    out.surface = vec4<f32>(t.surface, 1.0, 1.0);
    out.emissive = vec4<f32>(t.emissive, 1.0);
    out.normal = vec4<f32>(t.normal * 0.5 + 0.5, clamp(t.height * 0.5 + 0.5, 0.0, 1.0));
    return out;
}

// The export reads these back; must match `export_texel` in compositor.rs.
struct ExportOutputs {
    @location(0) base: vec4<f32>,     // linear base color, opacity
    @location(1) normal: vec4<f32>,   // unit normal, height
    @location(2) surface: vec4<f32>,  // roughness, metallic, distance to the nearest island texel
    @location(3) emissive: vec4<f32>,
}

// The compositor's APRON: the chunk's texel (0, 0) in the channels.
const APRON: i32 = 1;
const NO_SEED: u32 = 0xffffffffu;
// Largest 16-bit float.
const FAR: f32 = 65504.0;

// Renders a chunk at the export targets' origin.
@fragment
fn fs_export(@builtin(position) frag: vec4<f32>) -> ExportOutputs {
    let local = vec2<i32>(frag.xy) + vec2<i32>(APRON);
    let t = resolved(local);
    let texel = local + resolve.area.xy;
    let seed = textureLoad(seeds, texel, 0).r;
    var distance = FAR;
    if seed != NO_SEED {
        let nearest = vec2<i32>(i32(seed & 0xffffu), i32(seed >> 16u));
        distance = min(length(vec2<f32>(nearest - texel)), FAR);
    }
    var out: ExportOutputs;
    out.base = t.base;
    out.normal = vec4<f32>(t.normal, t.height);
    out.surface = vec4<f32>(t.surface, distance, 0.0);
    out.emissive = vec4<f32>(t.emissive, 0.0);
    return out;
}
