// Combines the fill values and the paint layer into the channel textures the viewport
// samples. Each texel reads the paint of its nearest covered texel (see paint_seeds.wgsl).

const NONE: u32 = 0xffffffffu;

// Linear values. Must match FillUniforms in paint.rs.
struct Fill {
    base: vec4<f32>,     // rgb: base color, a: opacity
    surface: vec4<f32>,  // x: roughness, y: metallic
    emissive: vec4<f32>,
}

@group(0) @binding(0) var<uniform> fill: Fill;
@group(0) @binding(1) var seeds: texture_2d<u32>;
// Premultiplied paint: rgb or r is value * coverage, a or g the coverage.
@group(0) @binding(2) var paint_base_color: texture_2d<f32>;
@group(0) @binding(3) var paint_roughness: texture_2d<f32>;
@group(0) @binding(4) var paint_metallic: texture_2d<f32>;
@group(0) @binding(5) var paint_emissive: texture_2d<f32>;
@group(0) @binding(6) var paint_opacity: texture_2d<f32>;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(xy, 0.0, 1.0);
}

struct Channels {
    @location(0) base: vec4<f32>,     // base color and opacity
    @location(1) surface: vec4<f32>,  // roughness, metallic, ambient occlusion
    @location(2) emissive: vec4<f32>,
}

fn over(below: f32, paint: vec2<f32>) -> f32 {
    return clamp(below * (1.0 - paint.y) + paint.x, 0.0, 1.0);
}

@fragment
fn fs_composite(@builtin(position) p: vec4<f32>) -> Channels {
    var t = vec2<i32>(p.xy);
    let seed = textureLoad(seeds, t, 0).r;
    if seed != NONE {
        t = vec2<i32>(i32(seed & 0xffffu), i32(seed >> 16u));
    }
    let base = textureLoad(paint_base_color, t, 0);
    let emissive = textureLoad(paint_emissive, t, 0);
    let roughness = textureLoad(paint_roughness, t, 0).rg;
    let metallic = textureLoad(paint_metallic, t, 0).rg;
    let opacity = textureLoad(paint_opacity, t, 0).rg;

    var out: Channels;
    out.base = vec4<f32>(
        clamp(fill.base.rgb * (1.0 - base.a) + base.rgb, vec3<f32>(0.0), vec3<f32>(1.0)),
        over(fill.base.a, opacity),
    );
    out.surface = vec4<f32>(over(fill.surface.x, roughness), over(fill.surface.y, metallic), 1.0, 1.0);
    out.emissive = vec4<f32>(
        clamp(fill.emissive.rgb * (1.0 - emissive.a) + emissive.rgb, vec3<f32>(0.0), vec3<f32>(1.0)),
        1.0,
    );
    return out;
}
