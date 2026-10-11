// Puts a stroke's coverage into paint tiles. Each tile is a quad in a page of a tile pool;
// its fragments map back to the texels of the texture set they stand for. Texels are
// premultiplied: rgb (or r) is value * coverage, alpha (or g) the coverage.

const TILE: f32 = 128.0;
const PAGE: f32 = 2048.0;

struct ApplyParams {
    value: vec4<f32>, // xyz: linear value; scalar channels and masks use x
    info: vec4<f32>,  // x: stroke opacity, y: 1 erases, z: 4 for rgba tiles, 2 for rg tiles
}

// Must match TileDraw in paint.rs.
struct TileDraw {
    dest: vec4<u32>,   // xy: origin in the page drawn into, zw: origin in the texture set
    before: vec4<u32>, // x: 1 if the tile had paint before, y: its snapshot page, zw: origin there
}

@group(0) @binding(0) var<uniform> params: ApplyParams;
@group(0) @binding(1) var<storage, read> draws: array<TileDraw>;
@group(0) @binding(2) var coverage: texture_2d<f32>;
@group(0) @binding(3) var snapshots: texture_2d_array<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) draw: u32,
}

@vertex
fn vs_tile(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let p = (vec2<f32>(draws[i].dest.xy) + corners[v] * TILE) / PAGE;
    var out: VsOut;
    out.clip = vec4<f32>(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    out.draw = i;
    return out;
}

// The stroke's paint at this fragment, premultiplied, with the coverage also in w.
fn paint(clip: vec4<f32>, d: TileDraw) -> vec4<f32> {
    let local = vec2<u32>(clip.xy) - d.dest.xy;
    let texel = d.dest.zw + local;
    let a = clamp(textureLoad(coverage, texel, 0).r * params.info.x, 0.0, 1.0);
    if params.info.z > 3.0 {
        return vec4<f32>(params.value.xyz * a, a);
    }
    return vec4<f32>(params.value.x * a, a, 0.0, a);
}

fn outside(clip: vec4<f32>, d: TileDraw) -> bool {
    let texel = d.dest.zw + vec2<u32>(clip.xy) - d.dest.xy;
    return any(texel >= textureDimensions(coverage));
}

// Writes the whole tile: what it held before the stroke, with the stroke on top.
@fragment
fn fs_replace(in: VsOut) -> @location(0) vec4<f32> {
    let d = draws[in.draw];
    if outside(in.clip, d) {
        discard;
    }
    var before = vec4<f32>(0.0);
    if d.before.x == 1u {
        let local = vec2<u32>(in.clip.xy) - d.dest.xy;
        before = textureLoad(snapshots, d.before.zw + local, d.before.y, 0);
    }
    let p = paint(in.clip, d);
    if params.info.y > 0.5 {
        return before * (1.0 - p.w);
    }
    var paint = p;
    if params.info.z <= 3.0 {
        paint = vec4<f32>(p.xy, 0.0, 0.0);
    }
    return paint + before * (1.0 - p.w);
}

// The stroke alone; the blend state puts it over the tile or erases with it.
@fragment
fn fs_blend(in: VsOut) -> @location(0) vec4<f32> {
    let d = draws[in.draw];
    if outside(in.clip, d) {
        discard;
    }
    return paint(in.clip, d);
}
