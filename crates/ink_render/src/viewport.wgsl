// Viewport shading: image based PBR or toon from the texture set channels, channel
// and material ID views, an inverted-hull ink outline and the environment background.

struct Uniforms {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,  // xyz: eye
    env: vec4<f32>,         // x: rotation (radians), y: exposure factor, z: background opacity, w: background blur 0..1
    env_info: vec4<f32>,    // x: highest specular mip level
    toon_light: vec4<f32>,  // xyz: direction towards the light (world), w: shade bands
    ink: vec4<f32>,         // rgb: outline color (linear), w: outline width in pixels
    viewport: vec4<f32>,    // xy: size in pixels, z: shading (0 pbr, 1 toon), w: display mode
    highlight: vec4<f32>,   // rgb: highlight color (linear), w: strength
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    sh: array<vec4<f32>, 9>, // irradiance, see Environment::irradiance_sh
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var env_panorama: texture_2d<f32>;
@group(0) @binding(2) var env_specular: texture_cube<f32>;
@group(0) @binding(3) var brdf_lut: texture_2d<f32>;
@group(0) @binding(4) var panorama_sampler: sampler;
@group(0) @binding(5) var clamp_sampler: sampler;

struct TextureSetUniforms {
    id_color: vec4<f32>,    // rgb: material ID color (linear), w: alpha mode (0 opaque, 1 test, 2 blend)
}

@group(1) @binding(0) var<uniform> texture_set: TextureSetUniforms;
@group(1) @binding(1) var base_color_map: texture_2d<f32>; // rgb: base color, a: opacity
@group(1) @binding(2) var normal_map: texture_2d<f32>;     // tangent space, OpenGL convention
@group(1) @binding(3) var surface_map: texture_2d<f32>;    // r: roughness, g: metallic, b: ambient occlusion
@group(1) @binding(4) var emissive_map: texture_2d<f32>;
@group(1) @binding(5) var material_sampler: sampler;

const PI: f32 = 3.14159265;

// Display modes, must match DisplayMode::code.
const DISPLAY_MATERIAL: u32 = 0u;
const DISPLAY_MATERIAL_ID: u32 = 1u;
const DISPLAY_BASE_COLOR: u32 = 2u;
const DISPLAY_ROUGHNESS: u32 = 3u;
const DISPLAY_METALLIC: u32 = 4u;
const DISPLAY_NORMAL: u32 = 5u;
const DISPLAY_EMISSIVE: u32 = 6u;
const DISPLAY_OPACITY: u32 = 7u;
const DISPLAY_AO: u32 = 8u;
const DISPLAY_HEIGHT: u32 = 9u;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    // Draws of highlighted parts start at instance 1.
    @builtin(instance_index) highlight: u32,
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) @interpolate(flat) highlight: u32,
}

@vertex
fn vs_mesh(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(v.position, 1.0);
    out.world = v.position;
    out.normal = v.normal;
    out.uv = v.uv;
    out.tangent = v.tangent;
    out.highlight = v.highlight;
    return out;
}

// ---- Environment lookups ----

fn rotate_y(d: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3<f32>(c * d.x + s * d.z, d.y, -s * d.x + c * d.z);
}

// World direction to environment space.
fn env_dir(d: vec3<f32>) -> vec3<f32> {
    return rotate_y(d, -u.env.x);
}

// Must match equirect_uv in ibl_common.wgsl.
fn equirect_uv(d: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(atan2(d.x, -d.z) / (2.0 * PI) + 0.5, acos(clamp(d.y, -1.0, 1.0)) / PI);
}

fn irradiance(n: vec3<f32>) -> vec3<f32> {
    let x = n.x;
    let y = n.y;
    let z = n.z;
    let e = u.sh[0].rgb * 0.282095
        + u.sh[1].rgb * (0.488603 * y)
        + u.sh[2].rgb * (0.488603 * z)
        + u.sh[3].rgb * (0.488603 * x)
        + u.sh[4].rgb * (1.092548 * x * y)
        + u.sh[5].rgb * (1.092548 * y * z)
        + u.sh[6].rgb * (0.315392 * (3.0 * z * z - 1.0))
        + u.sh[7].rgb * (1.092548 * x * z)
        + u.sh[8].rgb * (0.546274 * (x * x - y * y));
    return max(e, vec3<f32>(0.0));
}

// Narkowicz ACES fit.
fn tonemap(x: vec3<f32>) -> vec3<f32> {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

// Shows a data value (roughness, normal, ...) unchanged on the sRGB screen.
fn show_data(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(srgb_to_linear(c.x), srgb_to_linear(c.y), srgb_to_linear(c.z));
}

// ---- Surface ----

struct Surface {
    albedo: vec3<f32>,
    opacity: f32,
    roughness: f32,
    metallic: f32,
    ao: f32,
    emissive: vec3<f32>,
    normal: vec3<f32>,
    // Raw normal map texel for the normal channel view.
    normal_texel: vec3<f32>,
    // Height from the normal map's alpha, -1 to 1.
    height: f32,
}

fn surface(in: VsOut, front: bool) -> Surface {
    let base = textureSample(base_color_map, material_sampler, in.uv);
    let surf = textureSample(surface_map, material_sampler, in.uv);
    let emissive = textureSample(emissive_map, material_sampler, in.uv).rgb;
    let normal_sample = textureSample(normal_map, material_sampler, in.uv);
    let normal_texel = normal_sample.rgb;

    let n = normalize(in.normal);
    var t = in.tangent.xyz - n * dot(in.tangent.xyz, n);
    var mapped = n;
    if dot(t, t) > 1e-12 {
        t = normalize(t);
        let b = cross(n, t) * in.tangent.w;
        let tn = normal_texel * 2.0 - 1.0;
        mapped = normalize(t * tn.x + b * tn.y + n * tn.z);
    }

    var s: Surface;
    s.albedo = base.rgb;
    s.opacity = base.a;
    s.roughness = clamp(surf.r, 0.03, 1.0);
    s.metallic = surf.g;
    s.ao = surf.b;
    s.emissive = emissive;
    s.normal = select(-mapped, mapped, front);
    s.normal_texel = normal_texel;
    s.height = normal_sample.a * 2.0 - 1.0;
    return s;
}

fn fresnel_roughness(n_dot_v: f32, f0: vec3<f32>, roughness: f32) -> vec3<f32> {
    return f0 + (max(vec3<f32>(1.0 - roughness), f0) - f0) * pow(1.0 - n_dot_v, 5.0);
}

fn shade_pbr(s: Surface, v: vec3<f32>) -> vec3<f32> {
    let n = s.normal;
    let n_dot_v = max(dot(n, v), 1e-4);
    let f0 = mix(vec3<f32>(0.04), s.albedo, s.metallic);

    let r = reflect(-v, n);
    let prefiltered = textureSampleLevel(env_specular, clamp_sampler, env_dir(r), s.roughness * u.env_info.x).rgb;
    let brdf = textureSampleLevel(brdf_lut, clamp_sampler, vec2<f32>(n_dot_v, s.roughness), 0.0).rg;
    let specular = prefiltered * (f0 * brdf.x + brdf.y);

    let kd = (1.0 - fresnel_roughness(n_dot_v, f0, s.roughness)) * (1.0 - s.metallic);
    let diffuse = kd * s.albedo * irradiance(env_dir(n)) / PI;

    return ((diffuse + specular) * s.ao + s.emissive) * u.env.y;
}

fn shade_toon(s: Surface, v: vec3<f32>) -> vec3<f32> {
    let n = s.normal;
    let l = normalize(u.toon_light.xyz);
    let bands = max(u.toon_light.w, 2.0);
    let lambert = dot(n, l) * 0.5 + 0.5;
    let band = clamp(floor(lambert * bands) / (bands - 1.0), 0.0, 1.0);
    let shadow_tint = vec3<f32>(0.45, 0.42, 0.6);
    var color = s.albedo * mix(shadow_tint, vec3<f32>(1.0), band) * mix(1.0, s.ao, 0.5);

    let h = normalize(l + v);
    let shininess = mix(256.0, 8.0, s.roughness);
    let spec = step(0.5, pow(max(dot(n, h), 0.0), shininess));
    color += vec3<f32>(spec * (1.0 - s.roughness) * 0.6);

    let rim = smoothstep(0.72, 0.78, 1.0 - max(dot(n, v), 0.0)) * smoothstep(0.0, 0.3, dot(n, l));
    color += rim * 0.25;
    return color + s.emissive;
}

@fragment
fn fs_mesh(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let s = surface(in, front);
    let alpha_mode = u32(texture_set.id_color.w + 0.5);
    if alpha_mode == 1u && s.opacity < 0.5 {
        discard;
    }
    let v = normalize(u.camera_pos.xyz - in.world);

    var color: vec3<f32>;
    switch u32(u.viewport.w + 0.5) {
        case DISPLAY_MATERIAL_ID: {
            color = texture_set.id_color.rgb * (0.25 + 0.75 * max(dot(s.normal, v), 0.0));
        }
        case DISPLAY_BASE_COLOR: { color = s.albedo; }
        case DISPLAY_ROUGHNESS: { color = show_data(vec3<f32>(s.roughness)); }
        case DISPLAY_METALLIC: { color = show_data(vec3<f32>(s.metallic)); }
        case DISPLAY_NORMAL: { color = show_data(s.normal_texel); }
        case DISPLAY_EMISSIVE: { color = s.emissive; }
        case DISPLAY_OPACITY: { color = show_data(vec3<f32>(s.opacity)); }
        case DISPLAY_AO: { color = show_data(vec3<f32>(s.ao)); }
        // Positive heights, as Painter shows the height channel.
        case DISPLAY_HEIGHT: { color = show_data(vec3<f32>(clamp(s.height, 0.0, 1.0))); }
        default: {
            if u.viewport.z > 0.5 {
                color = shade_toon(s, v) * u.env.y;
            } else {
                color = tonemap(shade_pbr(s, v));
            }
        }
    }

    if in.highlight != 0u {
        let rim = pow(1.0 - max(dot(s.normal, v), 0.0), 2.0);
        color = mix(color, u.highlight.rgb, u.highlight.w * (0.5 + 0.5 * rim));
    }
    var alpha = 1.0;
    if alpha_mode == 2u {
        alpha = s.opacity;
    }
    return vec4<f32>(color, alpha);
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

// ---- Wireframe: the mesh edges, nudged towards the camera so they sit on the surface ----

@vertex
fn vs_wire(v: VsIn) -> @builtin(position) vec4<f32> {
    var clip = u.view_proj * vec4<f32>(v.position, 1.0);
    clip.z = clip.z - 2e-4 * clip.w;
    return clip;
}

@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    return vec4<f32>(0.62, 0.9, 0.25, 0.75);
}

// ---- Background: environment over a gradient (fullscreen triangle) ----

struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_background(@builtin(vertex_index) i: u32) -> BgOut {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var out: BgOut;
    out.clip = vec4<f32>(xy, 1.0, 1.0);
    out.ndc = xy;
    return out;
}

@fragment
fn fs_background(in: BgOut) -> @location(0) vec4<f32> {
    let gradient = mix(u.bg_bottom.rgb, u.bg_top.rgb, in.ndc.y * 0.5 + 0.5);
    if u.env.z <= 0.0 {
        return vec4<f32>(gradient, 1.0);
    }
    let near = u.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let far = u.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let d = env_dir(normalize(far.xyz / far.w - near.xyz / near.w));
    var env: vec3<f32>;
    if u.env.w <= 0.001 {
        env = textureSampleLevel(env_panorama, panorama_sampler, equirect_uv(d), 0.0).rgb;
    } else {
        env = textureSampleLevel(env_specular, clamp_sampler, d, u.env.w * u.env_info.x).rgb;
    }
    return vec4<f32>(mix(gradient, tonemap(env * u.env.y), u.env.z), 1.0);
}
