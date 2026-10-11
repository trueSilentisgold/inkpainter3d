// Padding for UV seams: every texel learns the nearest texel covered by a triangle of the
// texture set (jump flooding), and the composite takes its paint from there. Colors thus
// bleed past the UV islands, so filtering and mip maps show no seams.

const NONE: u32 = 0xffffffffu;

fn pack(t: vec2<u32>) -> u32 {
    return t.x | (t.y << 16u);
}

fn unpack(s: u32) -> vec2<i32> {
    return vec2<i32>(i32(s & 0xffffu), i32(s >> 16u));
}

// Covered texels point to themselves; the target is cleared to NONE before.
@vertex
fn vs_uv(@location(2) uv: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_init(@builtin(position) p: vec4<f32>) -> @location(0) u32 {
    return pack(vec2<u32>(p.xy));
}

@group(0) @binding(0) var seeds: texture_2d<u32>;

struct JfaOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) step: u32,
}

// The jump distance rides in the instance index.
@vertex
fn vs_jfa(@builtin(vertex_index) i: u32, @builtin(instance_index) step: u32) -> JfaOut {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var out: JfaOut;
    out.clip = vec4<f32>(xy, 0.0, 1.0);
    out.step = step;
    return out;
}

@fragment
fn fs_jfa(in: JfaOut) -> @location(0) u32 {
    let size = vec2<i32>(textureDimensions(seeds));
    let here = vec2<i32>(in.clip.xy);
    let step = i32(in.step);
    var best = NONE;
    var best_distance = 0x7fffffff;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = here + vec2<i32>(x, y) * step;
            if any(q < vec2<i32>(0)) || any(q >= size) {
                continue;
            }
            let s = textureLoad(seeds, q, 0).r;
            if s == NONE {
                continue;
            }
            let d = unpack(s) - here;
            let distance = d.x * d.x + d.y * d.y;
            if distance < best_distance {
                best = s;
                best_distance = distance;
            }
        }
    }
    return best;
}
