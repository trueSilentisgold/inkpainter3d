// Viewport shading: physically based (metallic/roughness) or toon, plus an
// inverted-hull ink outline and a gradient background.

struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    light_dir: vec4<f32>,   // xyz: direction towards the light, w: intensity
    base_color: vec4<f32>,  // linear rgb
    material: vec4<f32>,    // x: roughness, y: metallic, z: exposure, w: toon bands
    ink: vec4<f32>,         // rgb: outline color (linear), w: outline width in pixels
    viewport: vec4<f32>,    // xy: size in pixels, z: shading mode (0 pbr, 1 toon)
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

const PI: f32 = 3.14159265;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

@vertex
fn vs_mesh(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(v.position, 1.0);
    out.world = v.position;
    out.normal = v.normal;
    out.uv = v.uv;
    return out;
}

fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return gv * gl;
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (1.0 - f0) * pow(1.0 - cos_theta, 5.0);
}

fn direct_light(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, radiance: vec3<f32>,
                albedo: vec3<f32>, f0: vec3<f32>, roughness: f32, metallic: f32) -> vec3<f32> {
    let h = normalize(v + l);
    let n_dot_l = max(dot(n, l), 0.0);
    let n_dot_v = max(dot(n, v), 1e-4);
    let n_dot_h = max(dot(n, h), 0.0);
    let f = fresnel_schlick(max(dot(h, v), 0.0), f0);
    let specular = distribution_ggx(n_dot_h, roughness) * geometry_smith(n_dot_v, n_dot_l, roughness) * f
        / (4.0 * n_dot_v * n_dot_l + 1e-4);
    let kd = (1.0 - f) * (1.0 - metallic);
    return (kd * albedo / PI + specular) * radiance * n_dot_l;
}

// Narkowicz ACES fit.
fn tonemap(x: vec3<f32>) -> vec3<f32> {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn shade_pbr(n: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let albedo = u.base_color.rgb;
    let roughness = clamp(u.material.x, 0.04, 1.0);
    let metallic = u.material.y;
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let key = normalize(u.light_dir.xyz);
    let fill = normalize(vec3<f32>(-key.x, 0.3, -key.z));

    var color = direct_light(n, v, key, vec3<f32>(1.0, 0.97, 0.92) * u.light_dir.w, albedo, f0, roughness, metallic);
    color += direct_light(n, v, fill, vec3<f32>(0.55, 0.62, 0.75) * u.light_dir.w * 0.35, albedo, f0, roughness, metallic);

    // Hemisphere ambient as a cheap stand-in for image based lighting.
    let sky = mix(vec3<f32>(0.18, 0.17, 0.16), vec3<f32>(0.45, 0.5, 0.58), n.y * 0.5 + 0.5);
    let n_dot_v = max(dot(n, v), 0.0);
    let f_amb = f0 + (max(vec3<f32>(1.0 - roughness), f0) - f0) * pow(1.0 - n_dot_v, 5.0);
    color += sky * ((1.0 - f_amb) * (1.0 - metallic) * albedo + f_amb * (1.0 - roughness * 0.7));

    return tonemap(color * u.material.z);
}

fn shade_toon(n: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let albedo = u.base_color.rgb;
    let l = normalize(u.light_dir.xyz);
    let bands = max(u.material.w, 2.0);
    let lambert = dot(n, l) * 0.5 + 0.5;
    let band = clamp(floor(lambert * bands) / (bands - 1.0), 0.0, 1.0);
    let shadow_tint = vec3<f32>(0.45, 0.42, 0.6);
    var color = albedo * mix(shadow_tint, vec3<f32>(1.0), band);

    let h = normalize(l + v);
    let shininess = mix(256.0, 8.0, u.material.x);
    let spec = step(0.5, pow(max(dot(n, h), 0.0), shininess));
    color += vec3<f32>(spec * (1.0 - u.material.x) * 0.6);

    let rim = smoothstep(0.72, 0.78, 1.0 - max(dot(n, v), 0.0)) * smoothstep(0.0, 0.3, dot(n, l));
    color += rim * 0.25;
    return color * u.material.z;
}

@fragment
fn fs_mesh(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(in.normal);
    if !front {
        n = -n;
    }
    let v = normalize(u.camera_pos.xyz - in.world);
    if u.viewport.z > 0.5 {
        return vec4<f32>(shade_toon(n, v), 1.0);
    }
    return vec4<f32>(shade_pbr(n, v), 1.0);
}

// ---- Ink outline (inverted hull, constant width in screen space) ----

@vertex
fn vs_outline(v: VsIn) -> @builtin(position) vec4<f32> {
    var clip = u.view_proj * vec4<f32>(v.position, 1.0);
    let clip_normal = (u.view_proj * vec4<f32>(v.normal, 0.0)).xy;
    let len = length(clip_normal);
    if len > 1e-6 {
        let offset = clip_normal / len * u.ink.w * 2.0 / u.viewport.xy;
        clip = vec4<f32>(clip.xy + offset * clip.w, clip.zw);
    }
    return clip;
}

@fragment
fn fs_outline() -> @location(0) vec4<f32> {
    return vec4<f32>(u.ink.rgb, 1.0);
}

// ---- Background gradient (fullscreen triangle) ----

struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) t: f32,
}

@vertex
fn vs_background(@builtin(vertex_index) i: u32) -> BgOut {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var out: BgOut;
    out.clip = vec4<f32>(xy, 1.0, 1.0);
    out.t = xy.y * 0.5 + 0.5;
    return out;
}

@fragment
fn fs_background(in: BgOut) -> @location(0) vec4<f32> {
    return vec4<f32>(mix(u.bg_bottom.rgb, u.bg_top.rgb, in.t), 1.0);
}
