// The triangle of every texel of a tile, drawn in UV space; bake.wgsl reads it.

const STRIDE: u32 = 12u;

// Must match RasterParams in bake.rs.
struct RasterParams {
    // xy: first sub-texel of the tile, zw: its size in sub-texels.
    area: vec4<f32>,
    // x: size of the whole map in sub-texels.
    total: vec4<f32>,
}

@group(0) @binding(0) var<uniform> raster: RasterParams;
@group(0) @binding(1) var<storage, read> raster_vertices: array<f32>;
@group(0) @binding(2) var<storage, read> raster_indices: array<u32>;
// The mesh's own index of each triangle drawn.
@group(0) @binding(3) var<storage, read> raster_triangles: array<u32>;

struct TriangleOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) triangle: u32,
}

@vertex
fn vs_triangles(@builtin(vertex_index) i: u32) -> TriangleOut {
    let v = raster_indices[i];
    let uv = vec2<f32>(raster_vertices[v * STRIDE + 6u], raster_vertices[v * STRIDE + 7u]);
    let pixel = uv * raster.total.x;
    let local = (pixel - raster.area.xy) / raster.area.zw;
    var out: TriangleOut;
    out.clip = vec4<f32>(local.x * 2.0 - 1.0, 1.0 - local.y * 2.0, 0.0, 1.0);
    out.triangle = raster_triangles[i / 3u];
    return out;
}

@fragment
fn fs_triangles(in: TriangleOut) -> @location(0) u32 {
    return in.triangle;
}
