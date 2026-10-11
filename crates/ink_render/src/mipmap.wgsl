// Box-filters one mip level into the next: the bilinear sample at the corner shared by
// four source texels is their average.

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(xy, 0.0, 1.0);
}

@fragment
fn fs_downsample(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let target_size = max(vec2<f32>(textureDimensions(source)) * 0.5, vec2<f32>(1.0));
    return textureSampleLevel(source, linear_sampler, p.xy / target_size, 0.0);
}
