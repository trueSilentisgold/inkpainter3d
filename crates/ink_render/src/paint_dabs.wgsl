// Rasterizes brush dabs into a stroke's coverage texture. Surface strokes draw the texture
// set's triangles in UV space so every texel knows its world position; UV strokes (2D view)
// cover the texture directly. Coverage accumulates per dab like paint: c = 1 - prod(1 - d).

struct StrokeParams {
    shape: vec4<f32>,      // x: hardness, y: tip (0 round, 1 square, 2 image), z: backface cosine (-1: off), w: dab count
    sizes: vec4<f32>,      // x: texture size in texels, y: alpha image size in pixels, z: 1 to test occlusion
    view_proj: mat4x4<f32>, // the stroke's camera
    eye: vec4<f32>,        // xyz: camera position, w: view height (see StrokeCamera::height)
    forward: vec4<f32>,    // xyz: view direction, w: 1 for orthographic cameras
}

// Must match GpuDab in paint.rs.
struct Dab {
    position: vec4<f32>, // xyz: world position or (u, v, 0); w: radius
    normal: vec4<f32>,   // xyz: plane of the alpha; w: flow
    up: vec4<f32>,       // xyz: top of the alpha
    view: vec4<f32>,     // xyz: from the camera towards the dab
}

@group(0) @binding(0) var<uniform> stroke: StrokeParams;
@group(0) @binding(1) var<storage, read> dabs: array<Dab>;
@group(0) @binding(2) var alpha_map: texture_2d<f32>;
@group(0) @binding(3) var alpha_sampler: sampler;
// Distance along the view direction to the nearest surface (paint_depth.wgsl).
@group(0) @binding(4) var depth_map: texture_2d<f32>;

const TIP_ROUND: u32 = 0u;
const TIP_SQUARE: u32 = 1u;

// Brush tip at `local` ([-1, 1] across the dab, y up). `aa` is one texel in the same units.
fn tip(local: vec2<f32>, aa: f32) -> f32 {
    let kind = u32(stroke.shape.y + 0.5);
    if kind == TIP_ROUND || kind == TIP_SQUARE {
        var d = length(local);
        if kind == TIP_SQUARE {
            d = max(abs(local.x), abs(local.y));
        }
        let edge = max(aa, 1e-4);
        let inner = min(clamp(stroke.shape.x, 0.0, 1.0), 1.0 - edge);
        return 1.0 - smoothstep(inner, 1.0, d);
    }
    if abs(local.x) > 1.0 || abs(local.y) > 1.0 {
        return 0.0;
    }
    // Pick the mip whose pixels are about one texel of the painted texture.
    let lod = max(log2(max(stroke.sizes.y * aa * 0.5, 1e-6)), 0.0);
    let uv = vec2<f32>(local.x, -local.y) * 0.5 + 0.5;
    return textureSampleLevel(alpha_map, alpha_sampler, uv, lod).r;
}

// Whether another surface hides `world` from the stroke's camera. Outside the view
// nothing is known, so nothing is hidden there.
fn occluded(world: vec3<f32>, n: vec3<f32>, texel: f32) -> bool {
    let clip = stroke.view_proj * vec4<f32>(world, 1.0);
    if clip.w <= 0.0 {
        return true;
    }
    let ndc = clip.xy / clip.w;
    let size = vec2<f32>(textureDimensions(depth_map));
    let pixel = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * size;
    if any(pixel < vec2<f32>(0.0)) || any(pixel >= size) {
        return false;
    }
    // The farthest of the four nearest depth pixels, so silhouettes don't eat into the
    // surface behind them.
    let corner = vec2<i32>(floor(pixel - 0.5));
    let last = vec2<i32>(size) - 1;
    var stored = 0.0;
    for (var i = 0; i < 4; i++) {
        let q = clamp(corner + vec2<i32>(i & 1, i >> 1u), vec2<i32>(0), last);
        stored = max(stored, textureLoad(depth_map, q, 0).r);
    }
    let forward = stroke.forward.xyz;
    let d = dot(world - stroke.eye.xyz, forward);
    // World size of one depth pixel here; steep surfaces change depth faster across it.
    var pixel_world = stroke.eye.w * d / size.y;
    if stroke.forward.w > 0.5 {
        pixel_world = stroke.eye.w / size.y;
    }
    let c = abs(dot(n, forward));
    let slope = min(sqrt(max(1.0 - c * c, 0.0)) / max(c, 0.05), 20.0);
    let bias = 2.0 * texel + pixel_world * (1.5 + slope);
    return d > stored + bias;
}

// ---- Surface strokes ----

struct SurfaceOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
}

@vertex
fn vs_surface(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> SurfaceOut {
    var out: SurfaceOut;
    out.clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.world = position;
    out.normal = normal;
    return out;
}

@fragment
fn fs_surface(in: SurfaceOut) -> @location(0) vec4<f32> {
    // World size of one texel here, for anti-aliasing the tip's edge.
    let texel = max(max(length(dpdx(in.world)), length(dpdy(in.world))), 1e-8);
    let n = normalize(in.normal);
    if stroke.sizes.z > 0.5 && occluded(in.world, n, texel) {
        return vec4<f32>(0.0);
    }
    let count = u32(stroke.shape.w);
    let cull = stroke.shape.z > -0.999;
    var keep = 1.0;
    for (var i = 0u; i < count; i++) {
        let dab = dabs[i];
        // Dabs smaller than a texel would fall between texel centers: paint them one texel
        // wide with the same amount of paint.
        let radius = max(dab.position.w, 0.7 * texel);
        let flow = dab.normal.w * min(dab.position.w * dab.position.w / (radius * radius), 1.0);
        let offset = in.world - dab.position.xyz;
        let dist = length(offset);
        if dist > radius + texel {
            continue;
        }
        // Backface culling against the stamp, like Painter, with a soft edge.
        let axis = dab.normal.xyz;
        var facing = 1.0;
        if cull {
            facing = smoothstep(stroke.shape.z - 0.05, stroke.shape.z + 0.05, dot(n, axis));
            if facing <= 0.0 {
                continue;
            }
        }
        // Tangent wrap: the direction comes from the alpha's plane, the distance from 3D,
        // so the tip wraps around curved surfaces and edges instead of stretching.
        let up = dab.up.xyz;
        let right = cross(up, axis);
        let planar = offset - axis * dot(offset, axis);
        let planar_length = length(planar);
        var local = vec2<f32>(0.0);
        if planar_length > radius * 1e-6 {
            local = vec2<f32>(dot(planar, right), dot(planar, up)) * (dist / (planar_length * radius));
        }
        keep *= 1.0 - clamp(tip(local, texel / radius) * flow * facing, 0.0, 1.0);
    }
    return vec4<f32>(1.0 - keep, 0.0, 0.0, 1.0);
}

// ---- UV strokes (2D view) ----

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(xy, 0.0, 1.0);
}

@fragment
fn fs_uv(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let size = stroke.sizes.x;
    let uv = p.xy / size;
    let texel = 1.0 / size;
    let count = u32(stroke.shape.w);
    var keep = 1.0;
    for (var i = 0u; i < count; i++) {
        let dab = dabs[i];
        let radius = max(dab.position.w, 0.7 * texel);
        let flow = dab.normal.w * min(dab.position.w * dab.position.w / (radius * radius), 1.0);
        let offset = uv - dab.position.xy;
        if length(offset) > radius + texel {
            continue;
        }
        // UV y points down the image; `up` points to the top of the alpha.
        let up = dab.up.xy;
        let right = vec2<f32>(-up.y, up.x);
        let local = vec2<f32>(dot(offset, right), dot(offset, up)) / radius;
        keep *= 1.0 - clamp(tip(local, texel / radius) * flow, 0.0, 1.0);
    }
    return vec4<f32>(1.0 - keep, 0.0, 0.0, 1.0);
}
