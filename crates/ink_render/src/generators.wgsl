// Generators: procedural masks computed from the mesh maps and noise. Appended to
// composite.wgsl, whose passes and helpers it uses. The kinds and the order of their
// parameters are described in ink_core::generator.

// The mesh maps of the texture set being composited (a 1x1 dummy where there is none; the
// bits of `present` say which exist). Bound with the group of the seeds.
@group(0) @binding(5) var map_ao: texture_2d<f32>;
@group(0) @binding(6) var map_curvature: texture_2d<f32>;
@group(0) @binding(7) var map_normal: texture_2d<f32>;
@group(0) @binding(8) var map_position: texture_2d<f32>;
@group(0) @binding(9) var map_thickness: texture_2d<f32>;
@group(0) @binding(10) var map_id: texture_2d<f32>;

const HAS_AO: u32 = 1u;
const HAS_CURVATURE: u32 = 2u;
const HAS_NORMAL: u32 = 4u;
const HAS_POSITION: u32 = 8u;
const HAS_THICKNESS: u32 = 16u;
const HAS_ID: u32 = 32u;

const GEN_NOISE: u32 = 0u;
const GEN_MASK_BUILDER: u32 = 1u;
const GEN_EDGE_WEAR: u32 = 2u;
const GEN_DIRT: u32 = 3u;
const GEN_POSITION_GRADIENT: u32 = 4u;
const GEN_LIGHT: u32 = 5u;
const GEN_AO: u32 = 6u;
const GEN_CURVATURE: u32 = 7u;
const GEN_THICKNESS: u32 = 8u;

const PI: f32 = 3.14159265;

// Parameter `i` of the generator, in the order of its description.
fn gp(i: u32) -> f32 {
    let v = select(
        select(
            select(pass_.levels, pass_.offset, i < 12u),
            pass_.transform,
            i < 8u,
        ),
        pass_.value,
        i < 4u,
    );
    return v[i % 4u];
}

fn present(bit: u32) -> bool {
    return ((pass_.info.w >> 8u) & bit) != 0u;
}

// ---- Noise ----

fn hash_u(x: u32) -> u32 {
    var h = x * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

fn hash3(p: vec3<i32>, seed: u32) -> u32 {
    return hash_u(u32(p.x) * 73856093u ^ u32(p.y) * 19349663u ^ u32(p.z) * 83492791u ^ hash_u(seed + 0x9e3779b9u));
}

fn unit_random(h: u32) -> vec3<f32> {
    let a = f32(h & 0xffffu) / 65535.0 * 2.0 * PI;
    let z = f32((h >> 16u) & 0xffffu) / 65535.0 * 2.0 - 1.0;
    let r = sqrt(max(1.0 - z * z, 0.0));
    return vec3<f32>(r * cos(a), r * sin(a), z);
}

// Gradient noise in about -1 to 1.
fn perlin(p: vec3<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let c = vec3<i32>(i);
    var corner: array<f32, 8>;
    for (var k = 0; k < 8; k++) {
        let o = vec3<i32>(k & 1, (k >> 1) & 1, (k >> 2) & 1);
        corner[k] = dot(unit_random(hash3(c + o, seed)), f - vec3<f32>(o));
    }
    let x0 = mix(corner[0], corner[1], u.x);
    let x1 = mix(corner[2], corner[3], u.x);
    let x2 = mix(corner[4], corner[5], u.x);
    let x3 = mix(corner[6], corner[7], u.x);
    return mix(mix(x0, x1, u.y), mix(x2, x3, u.y), u.z) * 1.6;
}

// Layers of Perlin noise, 0 to 1.
fn fractal(p: vec3<f32>, octaves: i32, gain: f32, seed: u32) -> f32 {
    var sum = 0.0;
    var amplitude = 1.0;
    var total = 0.0;
    var q = p;
    for (var o = 0; o < octaves; o++) {
        sum += perlin(q, seed + u32(o) * 101u) * amplitude;
        total += amplitude;
        amplitude *= gain;
        q = q * 2.0;
    }
    return clamp(0.5 + 0.5 * sum / max(total, 1e-6), 0.0, 1.0);
}

// Distances to the nearest (x) and second nearest (y) of a randomly placed point per cell.
fn cell_distances(p: vec3<f32>, seed: u32) -> vec2<f32> {
    let i = vec3<i32>(floor(p));
    let f = p - floor(p);
    var best = 8.0;
    var second = 8.0;
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let o = vec3<i32>(x, y, z);
                let h = hash3(i + o, seed);
                let jitter = vec3<f32>(f32(h & 0x3ffu), f32((h >> 10u) & 0x3ffu), f32((h >> 20u) & 0x3ffu)) / 1023.0;
                let d = distance(f, vec3<f32>(o) + jitter);
                second = min(second, max(best, d));
                best = min(best, d);
            }
        }
    }
    return vec2<f32>(best, second);
}

// 0 Perlin, 1 fractal, 2 cells, 3 clouds, 4 cell edges.
fn noise_field(kind: u32, p: vec3<f32>, octaves: i32, gain: f32, seed: u32) -> f32 {
    if kind == 0u {
        return clamp(0.5 + 0.5 * perlin(p, seed), 0.0, 1.0);
    }
    if kind == 2u {
        // Distance to the nearest point, 0 to 1.
        return clamp(cell_distances(p, seed).x, 0.0, 1.0);
    }
    if kind == 4u {
        // How far from the border between two cells: 0 on it, for cracks and creases.
        let d = cell_distances(p, seed);
        return clamp(d.y - d.x, 0.0, 1.0);
    }
    let f = fractal(p, octaves, gain, seed);
    if kind == 3u {
        return smoothstep(0.25, 0.8, f);
    }
    return f;
}

// ---- Where the texel lies ----

struct Texel {
    uv: vec2<f32>,
    // Position in the model's bounds, 0 to 1 per axis.
    position: vec3<f32>,
    world_normal: vec3<f32>,
}

fn map_texel(s: vec2<i32>) -> Texel {
    var t: Texel;
    t.uv = (vec2<f32>(s) + 0.5) / f32(pass_.area.z);
    t.position = vec3<f32>(t.uv, 0.0);
    if present(HAS_POSITION) {
        t.position = textureSampleLevel(map_position, image_sampler, t.uv, 0.0).xyz;
    }
    t.world_normal = vec3<f32>(0.0, 1.0, 0.0);
    if present(HAS_NORMAL) {
        t.world_normal = normalize(textureSampleLevel(map_normal, image_sampler, t.uv, 0.0).xyz * 2.0 - 1.0);
    }
    return t;
}

fn ao_at(t: Texel) -> f32 {
    if present(HAS_AO) {
        return textureSampleLevel(map_ao, image_sampler, t.uv, 0.0).r;
    }
    return 1.0;
}

fn curvature_at(t: Texel) -> f32 {
    if present(HAS_CURVATURE) {
        return textureSampleLevel(map_curvature, image_sampler, t.uv, 0.0).r;
    }
    return 0.5;
}

fn thickness_at(t: Texel) -> f32 {
    if present(HAS_THICKNESS) {
        return textureSampleLevel(map_thickness, image_sampler, t.uv, 0.0).r;
    }
    return 1.0;
}

// Noise at the texel: in object space with a Position map (one unit of scale is the
// model's largest side; params.yzw holds the sizes of the bounds relative to it), in UV
// space without.
fn noise_at(t: Texel, kind: u32, scale: f32, octaves: i32, gain: f32, seed: u32) -> f32 {
    var p = vec3<f32>(t.uv * scale, 0.0);
    if present(HAS_POSITION) {
        p = t.position * pass_.params.yzw * scale;
    }
    return noise_field(kind, p, octaves, gain, seed);
}

fn adjust(v: f32, contrast: f32, balance: f32, invert: f32) -> f32 {
    var x = clamp((v - 0.5) * contrast + 0.5 + balance, 0.0, 1.0);
    if invert > 0.5 {
        x = 1.0 - x;
    }
    return x;
}

fn generate(t: Texel) -> f32 {
    let kind = pass_.info.w & 0xffu;
    if kind == GEN_NOISE {
        let n = noise_at(t, u32(gp(0u) + 0.5), gp(1u), i32(gp(2u) + 0.5), mix(0.35, 0.8, gp(3u)), u32(gp(4u) + 0.5));
        return adjust(n, gp(5u), gp(6u), gp(7u));
    }
    if kind == GEN_MASK_BUILDER {
        let curvature = (curvature_at(t) - 0.5) * 2.0;
        let occlusion = (ao_at(t) - 0.5) * 2.0;
        let thickness = (thickness_at(t) - 0.5) * 2.0;
        let height = (t.position.y - 0.5) * 2.0;
        var v = 0.5 + 0.5 * (gp(0u) * curvature + gp(1u) * occlusion + gp(2u) * thickness + gp(3u) * height);
        let n = noise_at(t, 1u, gp(5u), i32(gp(6u) + 0.5), 0.5, u32(gp(7u) + 0.5));
        v += (n - 0.5) * gp(4u);
        return adjust(v, gp(8u), gp(9u), gp(10u));
    }
    if kind == GEN_EDGE_WEAR {
        let edge = clamp((curvature_at(t) - 0.5) * 2.0, 0.0, 1.0);
        let n = noise_at(t, 1u, gp(3u), i32(gp(4u) + 0.5), 0.55, u32(gp(5u) + 0.5));
        let m = edge * (0.5 + gp(0u) * 3.0) + (n - 0.5) * gp(2u) * 1.5;
        let soft = max(gp(1u) * 0.5, 0.01);
        var v = clamp((m - 0.5) / soft * 0.5 + 0.5, 0.0, 1.0);
        if gp(6u) > 0.5 {
            v = 1.0 - v;
        }
        return v;
    }
    if kind == GEN_DIRT {
        let hollow = (1.0 - ao_at(t)) * gp(1u) * 2.0;
        let concave = clamp((0.5 - curvature_at(t)) * 2.0, 0.0, 1.0) * gp(2u) * 2.0;
        let up = pow(max(t.world_normal.y, 0.0), 2.0) * gp(3u);
        var d = hollow + concave + up;
        let n = noise_at(t, 3u, gp(5u), i32(gp(6u) + 0.5), 0.5, u32(gp(7u) + 0.5));
        d = d * mix(1.0, n * 1.7, gp(4u));
        var v = clamp((d - (0.5 - gp(0u) * 0.5)) * gp(8u) + 0.0, 0.0, 1.0);
        if gp(9u) > 0.5 {
            v = 1.0 - v;
        }
        return v;
    }
    if kind == GEN_POSITION_GRADIENT {
        var axis = t.position.y;
        let which = u32(gp(0u) + 0.5);
        if which == 0u {
            axis = t.position.x;
        } else if which == 2u {
            axis = t.position.z;
        }
        let n = noise_at(t, 1u, gp(4u), 4, 0.5, u32(gp(5u) + 0.5));
        axis += (n - 0.5) * gp(3u) * 0.5;
        let w = max(gp(2u) * 0.5, 0.002);
        var v = smoothstep(gp(1u) - w, gp(1u) + w, axis);
        if gp(6u) > 0.5 {
            v = 1.0 - v;
        }
        return v;
    }
    if kind == GEN_LIGHT {
        let az = radians(gp(0u));
        let el = radians(gp(1u));
        let l = vec3<f32>(cos(el) * sin(az), sin(el), cos(el) * cos(az));
        let lit = clamp(dot(t.world_normal, l), 0.0, 1.0);
        return adjust(lit, gp(2u), gp(3u), gp(4u));
    }
    if kind == GEN_AO {
        return adjust(ao_at(t), gp(0u), gp(1u), gp(2u));
    }
    if kind == GEN_THICKNESS {
        return adjust(thickness_at(t), gp(0u), gp(1u), gp(2u));
    }
    // GEN_CURVATURE: 0 convex, 1 concave, 2 both.
    let c = curvature_at(t);
    var v = c;
    let part = u32(gp(3u) + 0.5);
    if part == 0u {
        v = clamp((c - 0.5) * 2.0, 0.0, 1.0);
    } else if part == 1u {
        v = clamp((0.5 - c) * 2.0, 0.0, 1.0);
    }
    return adjust(v, gp(0u), gp(1u), gp(2u));
}

// A generator's mask, blended onto what lies below with the effect's mode and opacity.
// ---- Color selection: parts of the ID map by color ----
//
// The picked colors ride in the generator parameter slots, one per float as the integer
// r·65536 + g·256 + b (8 bits each); levels2 holds tolerance, hardness, invert and the
// number of colors.

fn picked_color(i: u32) -> vec3<f32> {
    let packed = u32(gp(i) + 0.5);
    return vec3<f32>(f32(packed >> 16u), f32((packed >> 8u) & 255u), f32(packed & 255u)) / 255.0;
}

fn color_selection(t: Texel) -> f32 {
    if !present(HAS_ID) {
        return 0.0;
    }
    // The nearest texel: filtering would blend the colors of neighbouring parts.
    let size = vec2<i32>(textureDimensions(map_id));
    let p = clamp(vec2<i32>(t.uv * vec2<f32>(size)), vec2<i32>(0), size - 1);
    let id = textureLoad(map_id, p, 0).rgb;
    var nearest = 2.0;
    let count = u32(pass_.levels2.w + 0.5);
    for (var i = 0u; i < count; i++) {
        // 1 between black and white.
        nearest = min(nearest, distance(id, picked_color(i)) / sqrt(3.0));
    }
    let tolerance = max(pass_.levels2.x, 1e-4);
    let start = tolerance * clamp(pass_.levels2.y, 0.0, 1.0);
    var selected = 1.0 - smoothstep(start, tolerance + 1e-4, nearest);
    if pass_.levels2.z > 0.5 {
        selected = 1.0 - selected;
    }
    return selected;
}

@fragment
fn fs_mask_color_selection(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let local = chunk_texel(frag);
    let t = map_texel(seed_of(local));
    let top = vec4<f32>(color_selection(t), 0.0, 0.0, 1.0);
    let m = textureLoad(below, local, 0).r;
    let out = blend_premultiplied(vec4<f32>(m, 0.0, 0.0, 1.0), top, pass_.params.x, pass_.info.x, KIND_SCALAR, true);
    return vec4<f32>(out.x, 0.0, 0.0, 1.0);
}

@fragment
fn fs_mask_generator(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let local = chunk_texel(frag);
    let t = map_texel(seed_of(local));
    let top = vec4<f32>(generate(t), 0.0, 0.0, 1.0);
    let m = textureLoad(below, local, 0).r;
    let out = blend_premultiplied(vec4<f32>(m, 0.0, 0.0, 1.0), top, pass_.params.x, pass_.info.x, KIND_SCALAR, true);
    return vec4<f32>(out.x, 0.0, 0.0, 1.0);
}
