// Geometry in texture space, for projections that need to know where a texel lies on the
// model (tri-planar). First the triangle of every covered texel is drawn into a map; per
// chunk, the position, normal and tangent at every texel's seed are then interpolated from
// its triangle's vertices.

const NONE: u32 = 0xffffffffu;
// Floats per vertex in the vertex buffer: position, normal, uv, tangent (see Vertex in
// viewport.rs).
const STRIDE: u32 = 12u;

// Bindings 5 and 6: the geometry pass below uses 0 to 4 of the same group.
@group(0) @binding(5) var<storage, read> tri_vertices: array<f32>;
@group(0) @binding(6) var<storage, read> tri_indices: array<u32>;

struct TriangleOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) triangle: u32,
}

@vertex
fn vs_triangles(@builtin(vertex_index) i: u32) -> TriangleOut {
    let v = tri_indices[i];
    let uv = vec2<f32>(tri_vertices[v * STRIDE + 6u], tri_vertices[v * STRIDE + 7u]);
    var out: TriangleOut;
    out.clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.triangle = i / 3u;
    return out;
}

@fragment
fn fs_triangles(in: TriangleOut) -> @location(0) u32 {
    return in.triangle;
}

// Must match GeometryUniforms in compositor.rs.
struct Geometry {
    area: vec4<i32>,   // xy: texture set texel at chunk texel (0, 0), z: resolution
    bounds: vec4<f32>, // xyz: center of the model's bounds, w: 1 / their largest side
}

@group(0) @binding(0) var<uniform> geometry: Geometry;
@group(0) @binding(1) var seeds: texture_2d<u32>;
@group(0) @binding(2) var triangles: texture_2d<u32>;
@group(0) @binding(3) var<storage, read> vertices: array<f32>;
@group(0) @binding(4) var<storage, read> indices: array<u32>;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(xy, 0.0, 1.0);
}

struct GeometryOut {
    // Position in the model's bounds: -0.5 to 0.5 along the largest side.
    @location(0) position: vec4<f32>,
    // World normal; w: size of a texel in the same units as the position.
    @location(1) normal: vec4<f32>,
    // World tangent; w: bitangent sign.
    @location(2) tangent: vec4<f32>,
}

fn vec3_at(v: u32, offset: u32) -> vec3<f32> {
    let i = v * STRIDE + offset;
    return vec3<f32>(vertices[i], vertices[i + 1u], vertices[i + 2u]);
}

fn uv_at(v: u32) -> vec2<f32> {
    return vec2<f32>(vertices[v * STRIDE + 6u], vertices[v * STRIDE + 7u]);
}

@fragment
fn fs_geometry(@builtin(position) frag: vec4<f32>) -> GeometryOut {
    let size = geometry.area.z;
    let t = clamp(vec2<i32>(frag.xy) + geometry.area.xy, vec2<i32>(0), vec2<i32>(size - 1));
    var s = t;
    let seed = textureLoad(seeds, t, 0).r;
    if seed != NONE {
        s = vec2<i32>(i32(seed & 0xffffu), i32(seed >> 16u));
    }
    var out: GeometryOut;
    out.position = vec4<f32>(0.0);
    out.normal = vec4<f32>(0.0, 0.0, 1.0, 0.0);
    out.tangent = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    let tri = textureLoad(triangles, s, 0).r;
    if tri == NONE {
        return out;
    }
    let v = vec3<u32>(indices[tri * 3u], indices[tri * 3u + 1u], indices[tri * 3u + 2u]);
    let uv0 = uv_at(v.x);
    let uv1 = uv_at(v.y);
    let uv2 = uv_at(v.z);
    let p0 = vec3_at(v.x, 0u);
    let p1 = vec3_at(v.y, 0u);
    let p2 = vec3_at(v.z, 0u);

    // Barycentric coordinates of the texel center in the triangle's UVs.
    let uv = (vec2<f32>(s) + 0.5) / f32(size);
    let e1 = uv1 - uv0;
    let e2 = uv2 - uv0;
    let d = e1.x * e2.y - e2.x * e1.y;
    var b = vec3<f32>(1.0 / 3.0);
    if abs(d) > 1e-12 {
        let q = uv - uv0;
        let b1 = (q.x * e2.y - e2.x * q.y) / d;
        let b2 = (e1.x * q.y - q.x * e1.y) / d;
        b = max(vec3<f32>(1.0 - b1 - b2, b1, b2), vec3<f32>(0.0));
        b /= max(b.x + b.y + b.z, 1e-6);
    }
    let position = p0 * b.x + p1 * b.y + p2 * b.z;
    let normal = vec3_at(v.x, 3u) * b.x + vec3_at(v.y, 3u) * b.y + vec3_at(v.z, 3u) * b.z;
    let t0 = vec4<f32>(vec3_at(v.x, 8u), vertices[v.x * STRIDE + 11u]);
    let tangent = t0.xyz * b.x + vec3_at(v.y, 8u) * b.y + vec3_at(v.z, 8u) * b.z;

    // World size of one texel: how fast the position changes along u and v.
    var texel = 0.0;
    if abs(d) > 1e-12 {
        let f1 = p1 - p0;
        let f2 = p2 - p0;
        let dp_du = (f1 * e2.y - f2 * e1.y) / d;
        let dp_dv = (f2 * e1.x - f1 * e2.x) / d;
        texel = max(length(dp_du), length(dp_dv)) / f32(size);
    }
    let scale = geometry.bounds.w;
    out.position = vec4<f32>((position - geometry.bounds.xyz) * scale, 1.0);
    out.normal = vec4<f32>(normalize(normal + vec3<f32>(0.0, 0.0, 1e-9)), texel * scale);
    out.tangent = vec4<f32>(tangent, select(-1.0, 1.0, t0.w >= 0.0));
    return out;
}
