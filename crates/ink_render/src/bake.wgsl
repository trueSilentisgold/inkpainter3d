// Mesh map baking: one thread per output texel casts rays against the model's own
// triangles and against high-poly triangles, both in a bounding volume hierarchy, and writes
// the map's value (rgb) and how much of the texel the surface covers (a).
//
// The trace functions are written once between the TEMPLATE markers and instantiated for the
// low-poly ("low") and the high-poly ("high") data by the Rust side (see bake.rs).

const NONE: u32 = 0xffffffffu;
// Floats per vertex of the vertex buffer: position, normal, uv, tangent (see Vertex in
// viewport.rs).
const STRIDE: u32 = 12u;
const PI: f32 = 3.14159265;

// The map codes are the indices of MeshMap::ALL.
const MAP_AO: u32 = 0u;
const MAP_CURVATURE: u32 = 1u;
const MAP_WORLD_NORMAL: u32 = 2u;
const MAP_POSITION: u32 = 3u;
const MAP_THICKNESS: u32 = 4u;
const MAP_NORMAL: u32 = 5u;
const MAP_HEIGHT: u32 = 6u;
const MAP_ID: u32 = 7u;

// Must match Params in bake.rs.
struct Params {
    tile: vec4<i32>,       // x, y: first output texel of the tile; z, w: its size in texels
    config: vec4<u32>,     // x: texture resolution, y: subsampling, z: map code, w: ray count
    distances: vec4<f32>,  // x: front, y: rear, z: ao / thickness distance, w: curvature radius
    bounds_min: vec4<f32>, // xyz: minimum of the model's bounds
    bounds_size: vec4<f32>, // xyz: size of the model's bounds, w: how far rays start off a surface
    flags: vec4<u32>,      // x: smooth normals, y: ignore backfaces, z: occluders (0 low, 1 high), w: id source
    matching: vec4<u32>,   // x: 1 when high-poly meshes are matched by name, y: low triangle count
    low_offsets: vec4<u32>,  // x: first triangle (in vec4s), y: first slot info
    high_offsets: vec4<u32>, // x: first triangle, y: first attribute
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> low_data: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> high_data: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> vertices: array<f32>;
// Triangle corners, then one group id per triangle (after 3 * low triangle count entries).
@group(0) @binding(4) var<storage, read> indices: array<u32>;
@group(0) @binding(5) var tri_map: texture_2d<u32>;
@group(0) @binding(6) var out_map: texture_storage_2d<rgba16float, write>;

struct Hit {
    t: f32,
    // The slot of the triangle in the hierarchy's order; NONE for a miss.
    slot: u32,
    u: f32,
    v: f32,
    front: bool,
}

// ---- TEMPLATE BEGIN ----
// Casts a ray against the $B-poly triangles. `any_hit` stops at the first hit; `group` and
// `check_group` limit the triangles by mesh name (high-poly only); `ignore_back` skips
// triangles seen from behind.
fn trace_$B(origin: vec3<f32>, dir: vec3<f32>, tmax: f32, any_hit: bool, group: u32, check_group: bool, ignore_back: bool) -> Hit {
    var best: Hit;
    best.t = tmax;
    best.slot = NONE;
    best.front = true;
    let safe = select(dir, vec3<f32>(1e-20), abs(dir) < vec3<f32>(1e-20));
    let inv = 1.0 / safe;
    var stack: array<u32, 48>;
    var sp = 1u;
    stack[0] = 0u;
    loop {
        if sp == 0u {
            break;
        }
        sp -= 1u;
        let node = stack[sp];
        let a = $B_data[node * 2u];
        let b = $B_data[node * 2u + 1u];
        let t0 = (a.xyz - origin) * inv;
        let t1 = (b.xyz - origin) * inv;
        let near = max(max(min(t0.x, t1.x), min(t0.y, t1.y)), max(min(t0.z, t1.z), 0.0));
        let far = min(min(max(t0.x, t1.x), max(t0.y, t1.y)), min(max(t0.z, t1.z), best.t));
        if near > far {
            continue;
        }
        let first = bitcast<u32>(a.w);
        let count = bitcast<u32>(b.w);
        if count == 0u {
            stack[sp] = first;
            stack[sp + 1u] = first + 1u;
            sp += 2u;
            continue;
        }
        for (var i = 0u; i < count; i++) {
            let slot = first + i;
            let base = $TRI + slot * 3u;
            let v0 = $B_data[base].xyz;
            let e1 = $B_data[base + 1u].xyz - v0;
            let e2 = $B_data[base + 2u].xyz - v0;
            let p = cross(safe, e2);
            let det = dot(e1, p);
            if abs(det) < 1e-14 {
                continue;
            }
            let front = det > 0.0;
            if ignore_back && !front {
                continue;
            }
            let idet = 1.0 / det;
            let tv = origin - v0;
            let u = dot(tv, p) * idet;
            if u < 0.0 || u > 1.0 {
                continue;
            }
            let q = cross(tv, e1);
            let v = dot(safe, q) * idet;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let t = dot(e2, q) * idet;
            if t <= 0.0 || t >= best.t {
                continue;
            }
            $FILTER
            best.t = t;
            best.slot = slot;
            best.u = u;
            best.v = v;
            best.front = front;
            if any_hit {
                return best;
            }
        }
    }
    return best;
}
// ---- TEMPLATE END ----
$INSTANCES

// ---- Surface at a texel ----

struct Surface {
    position: vec3<f32>,
    normal: vec3<f32>,
    // The normal rays follow: the smooth one or the face's.
    ray_normal: vec3<f32>,
    tangent: vec3<f32>,
    sign: f32,
    triangle: u32,
}

fn vertex_vec3(v: u32, offset: u32) -> vec3<f32> {
    let i = v * STRIDE + offset;
    return vec3<f32>(vertices[i], vertices[i + 1u], vertices[i + 2u]);
}

fn vertex_uv(v: u32) -> vec2<f32> {
    return vec2<f32>(vertices[v * STRIDE + 6u], vertices[v * STRIDE + 7u]);
}

fn surface_at(triangle: u32, uv: vec2<f32>) -> Surface {
    let v0 = indices[triangle * 3u];
    let v1 = indices[triangle * 3u + 1u];
    let v2 = indices[triangle * 3u + 2u];
    let a = vertex_uv(v0);
    let b = vertex_uv(v1);
    let c = vertex_uv(v2);
    let d = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
    var w1 = 0.0;
    var w2 = 0.0;
    if abs(d) > 1e-20 {
        w1 = ((uv.x - a.x) * (c.y - a.y) - (c.x - a.x) * (uv.y - a.y)) / d;
        w2 = ((b.x - a.x) * (uv.y - a.y) - (uv.x - a.x) * (b.y - a.y)) / d;
    }
    // Texel centers just outside the triangle (rasterization is conservative to a texel)
    // are clamped onto it.
    w1 = clamp(w1, 0.0, 1.0);
    w2 = clamp(w2, 0.0, 1.0 - w1);
    let w0 = 1.0 - w1 - w2;
    var s: Surface;
    s.triangle = triangle;
    let p0 = vertex_vec3(v0, 0u);
    let p1 = vertex_vec3(v1, 0u);
    let p2 = vertex_vec3(v2, 0u);
    s.position = p0 * w0 + p1 * w1 + p2 * w2;
    s.normal = normalize(vertex_vec3(v0, 3u) * w0 + vertex_vec3(v1, 3u) * w1 + vertex_vec3(v2, 3u) * w2);
    let face = normalize(cross(p1 - p0, p2 - p0));
    s.ray_normal = s.normal;
    if params.flags.x == 0u {
        // Faces facing the same way as the vertex normals.
        s.ray_normal = select(-face, face, dot(face, s.normal) >= 0.0);
    }
    let t0 = vec4<f32>(vertices[v0 * STRIDE + 8u], vertices[v0 * STRIDE + 9u], vertices[v0 * STRIDE + 10u], vertices[v0 * STRIDE + 11u]);
    let t1 = vec3<f32>(vertices[v1 * STRIDE + 8u], vertices[v1 * STRIDE + 9u], vertices[v1 * STRIDE + 10u]);
    let t2 = vec3<f32>(vertices[v2 * STRIDE + 8u], vertices[v2 * STRIDE + 9u], vertices[v2 * STRIDE + 10u]);
    s.tangent = normalize(t0.xyz * w0 + t1 * w1 + t2 * w2);
    s.sign = select(-1.0, 1.0, t0.w >= 0.0);
    return s;
}

// ---- Sampling ----

fn hash(x: u32) -> u32 {
    var h = x * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

fn random(seed: u32) -> f32 {
    return f32(hash(seed) & 0xffffffu) / 16777216.0;
}

fn radical_inverse(i: u32) -> f32 {
    var bits = i;
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return f32(bits) * 2.3283064365386963e-10;
}

fn basis(n: vec3<f32>) -> mat3x3<f32> {
    let helper = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.9);
    let t = normalize(cross(helper, n));
    let b = cross(n, t);
    return mat3x3<f32>(t, b, n);
}

// The i-th of `count` cosine-weighted directions around +z, turned by `turn` (0 to 1).
fn cosine_direction(i: u32, count: u32, turn: f32) -> vec3<f32> {
    let u1 = (f32(i) + 0.5) / f32(count);
    let phi = 2.0 * PI * fract(radical_inverse(i) + turn);
    let r = sqrt(u1);
    return vec3<f32>(r * cos(phi), r * sin(phi), sqrt(max(1.0 - u1, 0.0)));
}

// ---- Maps ----

fn high_group_of(slot: u32) -> u32 {
    return bitcast<u32>(high_data[params.high_offsets.y + slot * 4u + 3u].z);
}

fn trace_high_matched(origin: vec3<f32>, dir: vec3<f32>, tmax: f32, low_triangle: u32) -> Hit {
    let low_group = indices[params.matching.y * 3u + low_triangle];
    let check = params.matching.x == 1u && low_group != NONE;
    return trace_high(origin, dir, tmax, false, low_group, check, params.flags.y == 1u);
}

fn occluded(origin: vec3<f32>, dir: vec3<f32>, tmax: f32) -> bool {
    if params.flags.z == 1u {
        return trace_high(origin, dir, tmax, true, NONE, false, false).slot != NONE;
    }
    return trace_low(origin, dir, tmax, true, NONE, false, false).slot != NONE;
}

fn closest_any(origin: vec3<f32>, dir: vec3<f32>, tmax: f32) -> Hit {
    if params.flags.z == 1u {
        return trace_high(origin, dir, tmax, false, NONE, false, false);
    }
    return trace_low(origin, dir, tmax, false, NONE, false, false);
}

fn id_color(id: u32) -> vec3<f32> {
    let hue = fract(f32(id) * 0.618034);
    let s = 0.65;
    let v = 0.95;
    let h = hue * 6.0;
    var out: vec3<f32>;
    for (var c = 0; c < 3; c++) {
        let n = array<f32, 3>(5.0, 3.0, 1.0)[c];
        let k = (n + h) % 6.0;
        out[c] = v - v * s * clamp(min(k, 4.0 - k), 0.0, 1.0);
    }
    return out;
}

fn bake_value(s: Surface, seed: u32) -> vec3<f32> {
    let map = params.config.z;
    let count = params.config.w;
    let eps = params.bounds_size.w;
    if map == MAP_POSITION {
        return clamp((s.position - params.bounds_min.xyz) / max(params.bounds_size.xyz, vec3<f32>(1e-9)), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if map == MAP_WORLD_NORMAL {
        return s.normal * 0.5 + 0.5;
    }
    let n = s.ray_normal;
    if map == MAP_AO {
        let frame = basis(n);
        let turn = random(seed);
        var open = 0.0;
        for (var i = 0u; i < count; i++) {
            let d = frame * cosine_direction(i, count, turn);
            if !occluded(s.position + n * eps, d, params.distances.z) {
                open += 1.0;
            }
        }
        return vec3<f32>(open / f32(count));
    }
    if map == MAP_THICKNESS {
        let frame = basis(-n);
        let turn = random(seed);
        var sum = 0.0;
        for (var i = 0u; i < count; i++) {
            let d = frame * cosine_direction(i, count, turn);
            let hit = closest_any(s.position - n * eps, d, params.distances.z);
            sum += select(params.distances.z, hit.t, hit.slot != NONE);
        }
        return vec3<f32>(sum / f32(count) / params.distances.z);
    }
    if map == MAP_CURVATURE {
        // How far the surface lies above (concave) or below (convex) the tangent plane on a
        // ring around the point.
        let radius = params.distances.w;
        let frame = basis(n);
        let turn = random(seed) * 2.0 * PI;
        var sum = 0.0;
        for (var i = 0u; i < count; i++) {
            let angle = turn + 2.0 * PI * f32(i) / f32(count);
            let around = frame[0] * cos(angle) + frame[1] * sin(angle);
            // A wall rising out of the surface shows up as a hit along the tangent plane.
            let side = closest_any(s.position + n * eps * 2.0, around, radius);
            var height = 0.0;
            if side.slot != NONE {
                height = 2.0 * radius * (1.0 - side.t / radius);
            } else {
                // Otherwise look at how far the surface lies above or below the plane.
                let origin = s.position + around * radius + n * radius * 2.0;
                let hit = closest_any(origin, -n, radius * 5.0);
                height = -2.0 * radius;
                if hit.slot != NONE {
                    height = 2.0 * radius - hit.t;
                }
            }
            sum += clamp(height, -2.0 * radius, 2.0 * radius);
        }
        let mean = sum / f32(count) / radius;
        return vec3<f32>(clamp(0.5 - mean * 0.8, 0.0, 1.0));
    }
    // The maps that need the high-poly.
    let front = params.distances.x;
    let rear = params.distances.y;
    let hit = trace_high_matched(s.position + n * front, -n, front + rear, s.triangle);
    if map == MAP_NORMAL {
        if hit.slot == NONE {
            return vec3<f32>(0.5, 0.5, 1.0);
        }
        let base = params.high_offsets.y + hit.slot * 4u;
        let w = 1.0 - hit.u - hit.v;
        var hn = normalize(high_data[base].xyz * w + high_data[base + 1u].xyz * hit.u + high_data[base + 2u].xyz * hit.v);
        if !hit.front {
            hn = -hn;
        }
        let bitangent = s.sign * cross(s.normal, s.tangent);
        let ts = vec3<f32>(dot(hn, s.tangent), dot(hn, bitangent), dot(hn, s.normal));
        return normalize(ts) * 0.5 + 0.5;
    }
    if map == MAP_HEIGHT {
        if hit.slot == NONE {
            return vec3<f32>(rear / (front + rear));
        }
        let above = front - hit.t;
        return vec3<f32>(clamp((above + rear) / (front + rear), 0.0, 1.0));
    }
    // MAP_ID
    if hit.slot == NONE {
        return vec3<f32>(0.0);
    }
    let info = high_data[params.high_offsets.y + hit.slot * 4u + 3u];
    let id = select(bitcast<u32>(info.y), bitcast<u32>(info.x), params.flags.w == 1u);
    return id_color(id);
}

@compute @workgroup_size(8, 8, 1)
fn bake_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if i32(id.x) >= params.tile.z || i32(id.y) >= params.tile.w {
        return;
    }
    let res = params.config.x;
    let sub = params.config.y;
    let texel = vec2<i32>(id.xy) + params.tile.xy;
    var sum = vec3<f32>(0.0);
    var covered = 0.0;
    for (var sy = 0u; sy < sub; sy++) {
        for (var sx = 0u; sx < sub; sx++) {
            let m = vec2<i32>(i32(id.x * sub + sx), i32(id.y * sub + sy));
            let triangle = textureLoad(tri_map, m, 0).r;
            if triangle == NONE {
                continue;
            }
            let p = (vec2<f32>(texel) * f32(sub) + vec2<f32>(f32(sx), f32(sy)) + 0.5) / f32(res * sub);
            let s = surface_at(triangle, p);
            let seed = (u32(texel.y) * res + u32(texel.x)) * 16u + sy * sub + sx;
            sum += bake_value(s, seed);
            covered += 1.0;
        }
    }
    let total = f32(sub * sub);
    var value = vec3<f32>(0.0);
    if covered > 0.0 {
        value = sum / covered;
    }
    textureStore(out_map, texel, vec4<f32>(value, covered / total));
}
