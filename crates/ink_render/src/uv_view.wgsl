// The 2D view: a texture set's channel textures laid out in UV space, dimmed outside the
// UV islands, with the UV wireframe on top.

struct UvUniforms {
    transform: vec4<f32>, // xy: UV at the view center, zw: scale from UV to NDC
    rotation: vec4<f32>,  // xy: cosine and sine of the view rotation (clockwise on screen)
    info: vec4<f32>,      // x: display mode, y: wireframe opacity
    wire: vec4<f32>,      // rgb: wireframe color (linear)
}

@group(0) @binding(0) var<uniform> u: UvUniforms;

struct TextureSetUniforms {
    id_color: vec4<f32>,
}

// Same layout as @group(1) in viewport.wgsl.
@group(1) @binding(0) var<uniform> texture_set: TextureSetUniforms;
@group(1) @binding(1) var base_color_map: texture_2d<f32>;
@group(1) @binding(2) var normal_map: texture_2d<f32>;
@group(1) @binding(3) var surface_map: texture_2d<f32>;
@group(1) @binding(4) var emissive_map: texture_2d<f32>;
@group(1) @binding(5) var material_sampler: sampler;

// Display modes, must match DisplayMode::code.
const DISPLAY_MATERIAL_ID: u32 = 1u;
const DISPLAY_ROUGHNESS: u32 = 3u;
const DISPLAY_METALLIC: u32 = 4u;
const DISPLAY_NORMAL: u32 = 5u;
const DISPLAY_EMISSIVE: u32 = 6u;
const DISPLAY_OPACITY: u32 = 7u;
const DISPLAY_AO: u32 = 8u;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Must match UvCamera::to_pixel: rotate in UV space (y down), then scale to NDC (y up).
fn to_clip(uv: vec2<f32>) -> vec4<f32> {
    let d = uv - u.transform.xy;
    let c = u.rotation.x;
    let s = u.rotation.y;
    let p = vec2<f32>(c * d.x - s * d.y, s * d.x + c * d.y) * u.transform.zw;
    return vec4<f32>(p.x, -p.y, 0.5, 1.0);
}

// The unit UV square as two triangles.
@vertex
fn vs_square(@builtin(vertex_index) i: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    var out: VsOut;
    out.uv = corners[i];
    out.clip = to_clip(out.uv);
    return out;
}

@vertex
fn vs_mesh(@location(2) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.uv = uv;
    out.clip = to_clip(uv);
    return out;
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

// Shows a data value unchanged on the sRGB screen.
fn show_data(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(srgb_to_linear(c.x), srgb_to_linear(c.y), srgb_to_linear(c.z));
}

fn channel(uv: vec2<f32>) -> vec3<f32> {
    let surface = textureSample(surface_map, material_sampler, uv);
    switch u32(u.info.x + 0.5) {
        case DISPLAY_MATERIAL_ID: { return texture_set.id_color.rgb; }
        case DISPLAY_ROUGHNESS: { return show_data(vec3<f32>(surface.r)); }
        case DISPLAY_METALLIC: { return show_data(vec3<f32>(surface.g)); }
        case DISPLAY_NORMAL: { return show_data(textureSample(normal_map, material_sampler, uv).rgb); }
        case DISPLAY_EMISSIVE: { return textureSample(emissive_map, material_sampler, uv).rgb; }
        case DISPLAY_OPACITY: { return show_data(vec3<f32>(textureSample(base_color_map, material_sampler, uv).a)); }
        case DISPLAY_AO: { return show_data(vec3<f32>(surface.b)); }
        default: { return textureSample(base_color_map, material_sampler, uv).rgb; }
    }
}

// The whole texture, dimmed: what lies outside the UV islands is padding.
@fragment
fn fs_square(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(channel(in.uv) * 0.3, 1.0);
}

@fragment
fn fs_islands(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(channel(in.uv), 1.0);
}

@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    return vec4<f32>(u.wire.rgb, u.info.y);
}
