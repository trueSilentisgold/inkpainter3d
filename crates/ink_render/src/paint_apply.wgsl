// Puts a stroke's coverage into one paint layer channel. Layer texels are premultiplied:
// rgb (or r) is value * coverage, alpha (or g) the coverage. The blend state does the rest:
// paint is "over", erasing scales the existing paint down.

struct ApplyParams {
    value: vec4<f32>, // xyz: linear value; scalar channels use (v, 1, 0) so g gets the coverage
    info: vec4<f32>,  // x: stroke opacity
}

@group(0) @binding(0) var<uniform> params: ApplyParams;
@group(0) @binding(1) var coverage: texture_2d<f32>;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(xy, 0.0, 1.0);
}

@fragment
fn fs_apply(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let a = clamp(textureLoad(coverage, vec2<i32>(p.xy), 0).r * params.info.x, 0.0, 1.0);
    return vec4<f32>(params.value.xyz * a, a);
}
