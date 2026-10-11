// Split-sum BRDF lookup table: x = n·v, y = roughness, output rg = scale and bias
// applied to F0. Prepended with ibl_common.wgsl.

@group(0) @binding(0) var lut: texture_storage_2d<rgba16float, write>;

fn geometry_smith_ibl(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    let k = roughness * roughness * 0.5;
    let gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return gv * gl;
}

@compute @workgroup_size(8, 8, 1)
fn brdf_lut(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(lut);
    if id.x >= dims.x || id.y >= dims.y {
        return;
    }
    let n_dot_v = max((f32(id.x) + 0.5) / f32(dims.x), 1e-3);
    let roughness = (f32(id.y) + 0.5) / f32(dims.y);
    let a = roughness * roughness;
    let v = vec3<f32>(sqrt(1.0 - n_dot_v * n_dot_v), 0.0, n_dot_v);
    let n = vec3<f32>(0.0, 0.0, 1.0);

    const SAMPLES: u32 = 256u;
    var scale = 0.0;
    var bias = 0.0;
    for (var i = 0u; i < SAMPLES; i++) {
        let h = importance_sample_ggx(hammersley(i, SAMPLES), n, a);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let n_dot_l = max(l.z, 0.0);
        let n_dot_h = max(h.z, 0.0);
        let v_dot_h = max(dot(v, h), 0.0);
        if n_dot_l > 0.0 {
            let g = geometry_smith_ibl(n_dot_v, n_dot_l, roughness);
            let g_vis = g * v_dot_h / (n_dot_h * n_dot_v + 1e-6);
            let fc = pow(1.0 - v_dot_h, 5.0);
            scale += (1.0 - fc) * g_vis;
            bias += fc * g_vis;
        }
    }
    textureStore(lut, id.xy, vec4<f32>(scale, bias, 0.0, 1.0) / vec4<f32>(f32(SAMPLES), f32(SAMPLES), 1.0, 1.0));
}
