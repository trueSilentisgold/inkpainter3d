// Prefilters one mip level of the specular cube map from the equirectangular
// environment: GGX importance sampling with filtered (mip based) lookups.
// Prepended with ibl_common.wgsl.

struct Params {
    roughness: f32,
    // Face size of the output mip level in pixels.
    size: f32,
    sample_count: u32,
    _pad: u32,
    // Size of the source panorama, level 0.
    source_size: vec2<f32>,
    _pad2: vec2<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;
@group(0) @binding(3) var output: texture_storage_2d_array<rgba16float, write>;

// Direction through a cube face texel; `uv` in [-1, 1] with y pointing down.
fn cube_dir(face: u32, uv: vec2<f32>) -> vec3<f32> {
    let s = uv.x;
    let t = uv.y;
    switch face {
        case 0u: { return normalize(vec3<f32>(1.0, -t, -s)); }
        case 1u: { return normalize(vec3<f32>(-1.0, -t, s)); }
        case 2u: { return normalize(vec3<f32>(s, 1.0, t)); }
        case 3u: { return normalize(vec3<f32>(s, -1.0, -t)); }
        case 4u: { return normalize(vec3<f32>(s, -t, 1.0)); }
        default: { return normalize(vec3<f32>(-s, -t, -1.0)); }
    }
}

@compute @workgroup_size(8, 8, 1)
fn prefilter(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = u32(params.size);
    if id.x >= size || id.y >= size {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / params.size * 2.0 - 1.0;
    let n = cube_dir(id.z, uv);

    var color = vec3<f32>(0.0);
    if params.roughness <= 0.0 {
        color = textureSampleLevel(source, source_sampler, equirect_uv(n), 0.0).rgb;
    } else {
        let a = params.roughness * params.roughness;
        // Solid angle of a panorama texel at the equator.
        let texel = (2.0 * PI / params.source_size.x) * (PI / params.source_size.y);
        var weight = 0.0;
        for (var i = 0u; i < params.sample_count; i++) {
            let h = importance_sample_ggx(hammersley(i, params.sample_count), n, a);
            // View direction equals the normal, as usual for prefiltered maps.
            let l = normalize(2.0 * dot(n, h) * h - n);
            let n_dot_l = dot(n, l);
            if n_dot_l > 0.0 {
                let pdf = distribution_ggx(max(dot(n, h), 0.0), a) * 0.25 + 1e-4;
                let sample_angle = 1.0 / (f32(params.sample_count) * pdf);
                let texel_angle = texel * max(sqrt(1.0 - l.y * l.y), 1e-3);
                let lod = max(0.5 * log2(sample_angle / texel_angle) + 1.0, 0.0);
                color += textureSampleLevel(source, source_sampler, equirect_uv(l), lod).rgb * n_dot_l;
                weight += n_dot_l;
            }
        }
        color /= max(weight, 1e-4);
    }
    textureStore(output, id.xy, id.z, vec4<f32>(color, 1.0));
}
