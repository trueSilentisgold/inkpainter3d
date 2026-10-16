// The layer stack compositor. A texture set is composited in chunks: for one chunk at a
// time, one channel at a time, each layer is blended onto an accumulator texture the size
// of the chunk (ping-pong between two of them). Masks are evaluated into their own
// textures first. Every texel is evaluated at its seed, the nearest texel covered by the
// texture set's triangles (see paint_seeds.wgsl), which pads the result past the UV
// islands.
//
// Accumulators of colors and scalars are premultiplied: rgb (or x) is value * alpha, w the
// alpha, which is 0 where no layer put anything yet. Normals are stored straight: rgb is
// the encoded normal, w its alpha.

const NONE: u32 = 0xffffffffu;
const EMPTY: u32 = 0xffffffffu;
const TILE: i32 = 128;
const PAGE_TILES: u32 = 16u;

// Channel kinds, must match ChannelKind in compositor.rs.
const KIND_COLOR: u32 = 0u;
const KIND_SCALAR: u32 = 1u;
const KIND_NORMAL: u32 = 2u;

// Blend modes, must match BlendMode::code in ink_core.
const BLEND_NORMAL: u32 = 0u;
const BLEND_PASSTHROUGH: u32 = 1u;
const BLEND_DISABLE: u32 = 2u;
const BLEND_REPLACE: u32 = 3u;
const BLEND_MULTIPLY: u32 = 4u;
const BLEND_DIVIDE: u32 = 5u;
const BLEND_INVERSE_DIVIDE: u32 = 6u;
const BLEND_DARKEN: u32 = 7u;
const BLEND_LIGHTEN: u32 = 8u;
const BLEND_LINEAR_DODGE: u32 = 9u;
const BLEND_SUBTRACT: u32 = 10u;
const BLEND_INVERSE_SUBTRACT: u32 = 11u;
const BLEND_DIFFERENCE: u32 = 12u;
const BLEND_EXCLUSION: u32 = 13u;
const BLEND_SIGNED_ADDITION: u32 = 14u;
const BLEND_OVERLAY: u32 = 15u;
const BLEND_SCREEN: u32 = 16u;
const BLEND_LINEAR_BURN: u32 = 17u;
const BLEND_COLOR_BURN: u32 = 18u;
const BLEND_COLOR_DODGE: u32 = 19u;
const BLEND_SOFT_LIGHT: u32 = 20u;
const BLEND_HARD_LIGHT: u32 = 21u;
const BLEND_VIVID_LIGHT: u32 = 22u;
const BLEND_LINEAR_LIGHT: u32 = 23u;
const BLEND_PIN_LIGHT: u32 = 24u;
const BLEND_TINT: u32 = 25u;
const BLEND_SATURATION: u32 = 26u;
const BLEND_COLOR: u32 = 27u;
const BLEND_VALUE: u32 = 28u;
const BLEND_NORMAL_MAP_COMBINE: u32 = 29u;
const BLEND_NORMAL_MAP_DETAIL: u32 = 30u;
const BLEND_NORMAL_MAP_INVERSE_DETAIL: u32 = 31u;

// Pass flags.
const FLAG_MASK: u32 = 1u;
const FLAG_IMAGE: u32 = 2u;
const FLAG_CLAMP: u32 = 4u;
const FLAG_TRIPLANAR: u32 = 8u;
const FLAG_HEIGHT_IMAGE: u32 = 16u;
const FLAG_NORMAL_IMAGE: u32 = 32u;
const FLAG_PAINT: u32 = 64u;
const FLAG_DIRECTX: u32 = 128u;

// Must match PassUniforms in compositor.rs.
struct Pass {
    info: vec4<u32>,      // x: blend mode, y: channel kind, z: flags, w: tiles per side of the paint
    area: vec4<i32>,      // xy: texture set texel at chunk texel (0, 0), z: resolution, w: first slot of the channel
    value: vec4<f32>,     // fill value: linear color, encoded normal or scalar in x
    params: vec4<f32>,    // x: opacity, y: image size in texels (blur: radius in texels), z: tri-planar hardness
    transform: vec4<f32>, // projection: xy scale, z: rotation in radians
    offset: vec4<f32>,    // projection: xy offset (tri-planar: xyz)
    levels: vec4<f32>,    // levels: input low, input high, gamma, output low
    levels2: vec4<f32>,   // x: levels output high
}

@group(0) @binding(0) var seeds: texture_2d<u32>;
@group(0) @binding(1) var image_sampler: sampler;
// Chunk geometry for tri-planar projections, evaluated at the seeds: world position (xyz),
// world normal (xyz, w: world size of a texel), world tangent (xyz, w: bitangent sign).
@group(0) @binding(2) var geometry_position: texture_2d<f32>;
@group(0) @binding(3) var geometry_normal: texture_2d<f32>;
@group(0) @binding(4) var geometry_tangent: texture_2d<f32>;

@group(1) @binding(0) var<uniform> pass_: Pass;
// The accumulator or mask so far.
@group(1) @binding(1) var below: texture_2d<f32>;
// A folder's children, or the accumulator saved before a passthrough folder.
@group(1) @binding(2) var other: texture_2d<f32>;
@group(1) @binding(3) var mask: texture_2d<f32>;
@group(1) @binding(4) var tiles: texture_2d_array<f32>;
@group(1) @binding(5) var<storage, read> slots: array<u32>;
@group(1) @binding(6) var image: texture_2d<f32>;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(xy, 0.0, 1.0);
}

// ---- Where a chunk texel samples the texture set ----

fn chunk_texel(frag: vec4<f32>) -> vec2<i32> {
    return vec2<i32>(frag.xy);
}

// The texture set texel a chunk texel stands for, and the texel to evaluate there: its
// seed.
fn seed_of(local: vec2<i32>) -> vec2<i32> {
    let size = pass_.area.z;
    let t = clamp(local + pass_.area.xy, vec2<i32>(0), vec2<i32>(size - 1));
    let s = textureLoad(seeds, t, 0).r;
    if s == NONE {
        return t;
    }
    return vec2<i32>(i32(s & 0xffffu), i32(s >> 16u));
}

// Paint of the pass's paint layer at a texel, premultiplied: rgb (or x) and alpha in w.
fn paint_at(t: vec2<i32>) -> vec4<f32> {
    let tile = t / TILE;
    let side = i32(pass_.info.w);
    let slot = slots[u32(pass_.area.w + tile.y * side + tile.x)];
    if slot == EMPTY {
        return vec4<f32>(0.0);
    }
    let page = slot / (PAGE_TILES * PAGE_TILES);
    let i = slot % (PAGE_TILES * PAGE_TILES);
    let origin = vec2<i32>(i32(i % PAGE_TILES), i32(i / PAGE_TILES)) * TILE;
    let v = textureLoad(tiles, origin + t % TILE, page, 0);
    if pass_.info.y == KIND_SCALAR {
        return vec4<f32>(v.r, 0.0, 0.0, v.g);
    }
    return v;
}

fn mask_at(local: vec2<i32>) -> f32 {
    if (pass_.info.z & FLAG_MASK) == 0u {
        return 1.0;
    }
    return textureLoad(mask, local, 0).r;
}

// ---- Fill sources ----

fn rotate(v: vec2<f32>, angle: f32) -> vec2<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

// UV projection: scaled around the UV origin, rotated around the middle of the first tile,
// then offset in tiles.
fn uv_projection(uv: vec2<f32>) -> vec2<f32> {
    let scaled = uv * pass_.transform.xy;
    // Turns the image counterclockwise as seen in the 2D view, like tri-planar projections.
    return rotate(scaled - 0.5, pass_.transform.z) + 0.5 + pass_.offset.xy;
}

fn sample_image(uv: vec2<f32>, lod: f32) -> vec4<f32> {
    return textureSampleLevel(image, image_sampler, uv, lod);
}

// Mip level for one texel of the texture set covering `texels` texels of the image.
fn lod_for(texels: f32) -> f32 {
    return max(log2(max(texels, 1e-6)), 0.0);
}

fn decode_normal(c: vec3<f32>) -> vec3<f32> {
    return normalize(c * 2.0 - 1.0);
}

fn encode_normal(n: vec3<f32>) -> vec3<f32> {
    return normalize(n) * 0.5 + 0.5;
}

// The fill's value at seed texel `s` (chunk texel `local` for the geometry).
fn fill_value(s: vec2<i32>, local: vec2<i32>) -> vec4<f32> {
    let flags = pass_.info.z;
    if (flags & FLAG_IMAGE) == 0u {
        return pass_.value;
    }
    let size = f32(pass_.area.z);
    var c: vec4<f32>;
    if (flags & FLAG_TRIPLANAR) == 0u {
        let uv = (vec2<f32>(s) + 0.5) / size;
        let scale = max(abs(pass_.transform.x), abs(pass_.transform.y));
        c = sample_image(uv_projection(uv), lod_for(pass_.params.y * scale / size));
        if (flags & FLAG_DIRECTX) != 0u {
            c.y = 1.0 - c.y;
        }
        if (flags & FLAG_NORMAL_IMAGE) != 0u {
            // The relief turned and mirrored with the image, so its normals turn too (in the
            // tangent frame, y up).
            let n = decode_normal(c.xyz);
            let mirror = vec2<f32>(sign_or_one(pass_.transform.x), sign_or_one(pass_.transform.y));
            let xy = rotate(n.xy, pass_.transform.z) * mirror;
            c = vec4<f32>(encode_normal(vec3<f32>(xy, n.z)), c.w);
        }
    } else {
        c = triplanar(local);
    }
    if (flags & FLAG_HEIGHT_IMAGE) != 0u {
        // Images in height map 0..1 to -1..1: mid grey adds nothing.
        return vec4<f32>(c.r * 2.0 - 1.0, 0.0, 0.0, 1.0);
    }
    return c;
}

// Weights of the three planes of a tri-planar projection; hardness 1 cuts between them.
fn plane_weights(n: vec3<f32>) -> vec3<f32> {
    let hardness = clamp(pass_.params.z, 0.0, 1.0);
    let a = abs(n);
    if hardness >= 0.999 {
        if a.x >= a.y && a.x >= a.z {
            return vec3<f32>(1.0, 0.0, 0.0);
        }
        if a.y >= a.z {
            return vec3<f32>(0.0, 1.0, 0.0);
        }
        return vec3<f32>(0.0, 0.0, 1.0);
    }
    let w = pow(a, vec3<f32>(exp2(hardness * 6.0)));
    return w / max(w.x + w.y + w.z, 1e-6);
}

// Tri-planar projection: the image projected along the three axes of the model's bounds,
// blended by the surface normal. Normal map images are turned into the surface's tangent
// space.
fn triplanar(local: vec2<i32>) -> vec4<f32> {
    let p = textureLoad(geometry_position, local, 0).xyz;
    let n_texel = textureLoad(geometry_normal, local, 0);
    let n = normalize(n_texel.xyz);
    let w = plane_weights(n);
    let scale = pass_.transform.xy;
    let angle = -pass_.transform.z;
    let lod = lod_for(pass_.params.y * max(abs(scale.x), abs(scale.y)) * n_texel.w);
    let sx = sign_or_one(n.x);
    let sy = sign_or_one(n.y);
    let sz = sign_or_one(n.z);
    // Image axes on each plane, chosen so the image is upright and not mirrored when seen
    // from outside: (right, up) in world space.
    // The image's center sits at the center of the model's bounds.
    let flip = vec2<f32>(1.0, -1.0);
    let uv_x = rotate(vec2<f32>(-p.z * sx, p.y) * scale, angle) * flip + 0.5;
    let uv_y = rotate(vec2<f32>(p.x, -p.z * sy) * scale, angle) * flip + 0.5;
    let uv_z = rotate(vec2<f32>(p.x * sz, p.y) * scale, angle) * flip + 0.5;
    let cx = sample_image(uv_x, lod);
    let cy = sample_image(uv_y, lod);
    let cz = sample_image(uv_z, lod);
    if (pass_.info.z & FLAG_NORMAL_IMAGE) == 0u {
        return cx * w.x + cy * w.y + cz * w.z;
    }
    // Normal maps: each plane's tangent-space normal into world space along that plane's
    // image axes, blended, then into the surface's tangent space.
    let green = select(1.0, -1.0, (pass_.info.z & FLAG_DIRECTX) != 0u);
    let tx = decode_normal(cx.xyz) * vec3<f32>(1.0, green, 1.0);
    let ty = decode_normal(cy.xyz) * vec3<f32>(1.0, green, 1.0);
    let tz = decode_normal(cz.xyz) * vec3<f32>(1.0, green, 1.0);
    let world_x = vec3<f32>(tx.z * sx, 0.0, 0.0) + rotate_axes(tx.xy, angle, vec3<f32>(0.0, 0.0, -sx), vec3<f32>(0.0, 1.0, 0.0));
    let world_y = vec3<f32>(0.0, ty.z * sy, 0.0) + rotate_axes(ty.xy, angle, vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, -sy));
    let world_z = vec3<f32>(0.0, 0.0, tz.z * sz) + rotate_axes(tz.xy, angle, vec3<f32>(sz, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0));
    let world = normalize(world_x * w.x + world_y * w.y + world_z * w.z + n * 1e-4);
    let t_texel = textureLoad(geometry_tangent, local, 0);
    var t = t_texel.xyz - n * dot(t_texel.xyz, n);
    if dot(t, t) < 1e-12 {
        return vec4<f32>(0.5, 0.5, 1.0, 1.0);
    }
    t = normalize(t);
    let b = cross(n, t) * t_texel.w;
    let ts = vec3<f32>(dot(world, t), dot(world, b), dot(world, n));
    return vec4<f32>(encode_normal(ts), 1.0);
}

// A tangent-space xy on a projection plane whose image axes are `right` and `up`, turned
// by the projection's rotation, in world space.
fn rotate_axes(xy: vec2<f32>, angle: f32, right: vec3<f32>, up: vec3<f32>) -> vec3<f32> {
    // The image was rotated by `angle` in (right, up); its axes turn back the other way.
    let v = rotate(xy, -angle);
    return right * v.x + up * v.y;
}

fn sign_or_one(v: f32) -> f32 {
    return select(-1.0, 1.0, v >= 0.0);
}

// ---- Blend modes ----

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let v = max(c.r, max(c.g, c.b));
    let m = min(c.r, min(c.g, c.b));
    let d = v - m;
    var h = 0.0;
    if d > 1e-6 {
        if v == c.r {
            h = (c.g - c.b) / d;
        } else if v == c.g {
            h = 2.0 + (c.b - c.r) / d;
        } else {
            h = 4.0 + (c.r - c.g) / d;
        }
        h = fract(h / 6.0 + 1.0);
    }
    var s = 0.0;
    if v > 1e-6 {
        s = d / v;
    }
    return vec3<f32>(h, s, v);
}

fn hsv_to_rgb(c: vec3<f32>) -> vec3<f32> {
    let k = vec3<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0);
    let p = abs(fract(c.xxx + k) * 6.0 - 3.0);
    return c.z * mix(vec3<f32>(1.0), clamp(p - 1.0, vec3<f32>(0.0), vec3<f32>(1.0)), c.y);
}

fn burn(b: f32, t: f32) -> f32 {
    if b >= 1.0 {
        return 1.0;
    }
    if t <= 0.0 {
        return 0.0;
    }
    return clamp(1.0 - (1.0 - b) / t, 0.0, 1.0);
}

fn dodge(b: f32, t: f32) -> f32 {
    if b <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    return clamp(b / (1.0 - t), 0.0, 1.0);
}

fn soft_light(b: f32, t: f32) -> f32 {
    if t <= 0.5 {
        return b - (1.0 - 2.0 * t) * b * (1.0 - b);
    }
    var d = sqrt(b);
    if b <= 0.25 {
        d = ((16.0 * b - 12.0) * b + 4.0) * b;
    }
    return b + (2.0 * t - 1.0) * (d - b);
}

fn hard_light(b: f32, t: f32) -> f32 {
    if t < 0.5 {
        return 2.0 * b * t;
    }
    return 1.0 - 2.0 * (1.0 - b) * (1.0 - t);
}

fn blend_component(mode: u32, b: f32, t: f32) -> f32 {
    switch mode {
        case BLEND_MULTIPLY: { return b * t; }
        case BLEND_DIVIDE: { return b / max(t, 1e-4); }
        case BLEND_INVERSE_DIVIDE: { return t / max(b, 1e-4); }
        case BLEND_DARKEN: { return min(b, t); }
        case BLEND_LIGHTEN: { return max(b, t); }
        case BLEND_LINEAR_DODGE: { return b + t; }
        case BLEND_SUBTRACT: { return b - t; }
        case BLEND_INVERSE_SUBTRACT: { return t - b; }
        case BLEND_DIFFERENCE: { return abs(b - t); }
        case BLEND_EXCLUSION: { return b + t - 2.0 * b * t; }
        case BLEND_SIGNED_ADDITION: { return b + 2.0 * t - 1.0; }
        case BLEND_OVERLAY: { return hard_light(t, b); }
        case BLEND_SCREEN: { return 1.0 - (1.0 - b) * (1.0 - t); }
        case BLEND_LINEAR_BURN: { return b + t - 1.0; }
        case BLEND_COLOR_BURN: { return burn(b, t); }
        case BLEND_COLOR_DODGE: { return dodge(b, t); }
        case BLEND_SOFT_LIGHT: { return soft_light(b, t); }
        case BLEND_HARD_LIGHT: { return hard_light(b, t); }
        case BLEND_VIVID_LIGHT: {
            if t < 0.5 {
                return burn(b, 2.0 * t);
            }
            return dodge(b, 2.0 * t - 1.0);
        }
        case BLEND_LINEAR_LIGHT: { return b + 2.0 * t - 1.0; }
        case BLEND_PIN_LIGHT: {
            if t < 0.5 {
                return min(b, 2.0 * t);
            }
            return max(b, 2.0 * t - 1.0);
        }
        default: { return t; }
    }
}

// The blend mode's result for straight values below (b) and on top (t).
fn blend_values(mode: u32, kind: u32, b: vec3<f32>, t: vec3<f32>) -> vec3<f32> {
    if kind == KIND_COLOR && mode >= BLEND_TINT && mode <= BLEND_VALUE {
        let hb = rgb_to_hsv(b);
        let ht = rgb_to_hsv(t);
        switch mode {
            case BLEND_TINT: {
                if ht.y < 1e-4 || ht.z < 1e-4 {
                    return b;
                }
                return hsv_to_rgb(vec3<f32>(ht.x, hb.y, hb.z));
            }
            case BLEND_SATURATION: { return hsv_to_rgb(vec3<f32>(hb.x, ht.y, hb.z)); }
            case BLEND_COLOR: { return hsv_to_rgb(vec3<f32>(ht.x, ht.y, hb.z)); }
            default: { return hsv_to_rgb(vec3<f32>(hb.x, hb.y, ht.z)); }
        }
    }
    return vec3<f32>(
        blend_component(mode, b.x, t.x),
        blend_component(mode, b.y, t.y),
        blend_component(mode, b.z, t.z),
    );
}

// Blends premultiplied `top` onto premultiplied `below` with weight `k` (opacity times
// mask), following the W3C compositing model: the blend mode decides the color where both
// are present, the layer's alpha how much of it shows.
fn blend_premultiplied(below: vec4<f32>, top: vec4<f32>, k: f32, mode: u32, kind: u32, clamp_result: bool) -> vec4<f32> {
    var out: vec4<f32>;
    if mode == BLEND_REPLACE {
        out = mix(below, top, k);
    } else {
        let a = top.w * k;
        var color = top.xyz;
        if mode != BLEND_NORMAL && mode != BLEND_PASSTHROUGH && top.w > 0.0 {
            let t = top.xyz / top.w;
            var b = vec3<f32>(0.0);
            if below.w > 0.0 {
                b = below.xyz / below.w;
            }
            color = ((1.0 - below.w) * t + below.w * blend_values(mode, kind, b, t)) * top.w;
        }
        out = vec4<f32>(color * k + below.xyz * (1.0 - a), a + below.w * (1.0 - a));
    }
    if clamp_result {
        out = vec4<f32>(clamp(out.xyz, vec3<f32>(0.0), vec3<f32>(out.w)), out.w);
    }
    return out;
}

// Fades a normal towards flat, keeping its direction (Painter's normalFade).
// Scales the normal's tilt away from flat by `amount`, staying short of 90°.
fn tilt_normal(n: vec3<f32>, amount: f32) -> vec3<f32> {
    if amount == 1.0 || n.z >= 1.0 {
        return n;
    }
    let phi = min(amount * acos(clamp(n.z, -1.0, 1.0)), 1.55);
    let xy = n.xy / max(sqrt(max(1.0 - n.z * n.z, 0.0)), 1e-6) * sin(phi);
    return vec3<f32>(xy, cos(phi));
}

fn fade_normal(n: vec3<f32>, amount: f32) -> vec3<f32> {
    return tilt_normal(n, min(amount, 1.0));
}

// Whiteout blending (Normal Map Combine).
fn whiteout(base: vec3<f32>, over: vec3<f32>) -> vec3<f32> {
    return normalize(vec3<f32>(base.xy + over.xy, base.z * over.z));
}

// Reoriented normal mapping (Normal Map Detail): `over` follows the surface of `base`.
fn reoriented(base: vec3<f32>, over: vec3<f32>) -> vec3<f32> {
    let b = base + vec3<f32>(0.0, 0.0, 1.0);
    let o = vec3<f32>(-over.xy, over.z);
    return normalize(b * dot(b, o) - o * b.z);
}

// Normals are straight, not premultiplied: rgb the encoded normal, w its alpha.
fn blend_normal(below: vec4<f32>, top: vec4<f32>, k: f32, mode: u32) -> vec4<f32> {
    let flat = vec3<f32>(0.0, 0.0, 1.0);
    var b = flat;
    if below.w > 0.0 {
        b = decode_normal(below.xyz);
    }
    var t = flat;
    if top.w > 0.0 {
        t = decode_normal(top.xyz);
    }
    if mode == BLEND_REPLACE {
        let w = mix(below.w, top.w, k);
        let n = b * below.w * (1.0 - k) + t * top.w * k;
        if w <= 0.0 {
            return vec4<f32>(0.5, 0.5, 1.0, 0.0);
        }
        return vec4<f32>(encode_normal(n + flat * 1e-6), w);
    }
    let a = top.w * k;
    let alpha = a + below.w * (1.0 - a);
    if alpha <= 0.0 {
        return vec4<f32>(0.5, 0.5, 1.0, 0.0);
    }
    var n: vec3<f32>;
    switch mode {
        case BLEND_NORMAL_MAP_COMBINE, BLEND_NORMAL_MAP_DETAIL, BLEND_NORMAL_MAP_INVERSE_DETAIL: {
            // Both normals at the strength their coverage gives them...
            let fb = fade_normal(b, below.w);
            let ft = fade_normal(t, a);
            switch mode {
                case BLEND_NORMAL_MAP_COMBINE: { n = whiteout(fb, ft); }
                case BLEND_NORMAL_MAP_DETAIL: { n = reoriented(fb, ft); }
                default: { n = reoriented(ft, fb); }
            }
            // ...and stored straight, as the resolve fades the result by its coverage.
            n = tilt_normal(normalize(n), 1.0 / alpha);
        }
        default: {
            // Normal and the color modes: weighted like premultiplied "over".
            n = t * a + b * below.w * (1.0 - a);
        }
    }
    return vec4<f32>(encode_normal(n + flat * 1e-6), alpha);
}

fn blend(below: vec4<f32>, top: vec4<f32>, k: f32) -> vec4<f32> {
    let mode = pass_.info.x;
    let kind = pass_.info.y;
    if kind == KIND_NORMAL {
        return blend_normal(below, top, k, mode);
    }
    return blend_premultiplied(below, top, k, mode, kind, (pass_.info.z & FLAG_CLAMP) != 0u);
}

// ---- Layer passes: one channel of one layer onto the accumulator ----

@fragment
fn fs_layer_paint(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let local = chunk_texel(frag);
    let s = seed_of(local);
    var top = paint_at(s);
    if pass_.info.y == KIND_NORMAL && top.w > 0.0 {
        // Tiles are premultiplied; normals blend straight.
        top = vec4<f32>(top.xyz / top.w, top.w);
    }
    let k = pass_.params.x * mask_at(local);
    return blend(textureLoad(below, local, 0), top, k);
}

@fragment
fn fs_layer_fill(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let local = chunk_texel(frag);
    let s = seed_of(local);
    var v = fill_value(s, local);
    var top: vec4<f32>;
    switch pass_.info.y {
        case KIND_COLOR: { top = vec4<f32>(v.xyz, 1.0); }
        case KIND_NORMAL: { top = vec4<f32>(v.xyz, 1.0); }
        default: { top = vec4<f32>(v.x, 0.0, 0.0, 1.0); }
    }
    let k = pass_.params.x * mask_at(local);
    return blend(textureLoad(below, local, 0), top, k);
}

// A folder's children, composited apart, onto what lies below the folder.
@fragment
fn fs_folder(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let local = chunk_texel(frag);
    let k = pass_.params.x * mask_at(local);
    return blend(textureLoad(below, local, 0), textureLoad(other, local, 0), k);
}

// A passthrough folder's mask and opacity: between what lay below it (saved in `other`)
// and the result with its children.
@fragment
fn fs_passthrough(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let local = chunk_texel(frag);
    let k = pass_.params.x * mask_at(local);
    let before = textureLoad(other, local, 0);
    let after = textureLoad(below, local, 0);
    if pass_.info.y == KIND_NORMAL {
        return blend_normal(before, after, k, BLEND_REPLACE);
    }
    return mix(before, after, k);
}

// ---- Mask passes: grayscale, one value per texel ----

// Paint directly in the mask, over what lies below.
@fragment
fn fs_mask_paint(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let local = chunk_texel(frag);
    let p = paint_at(seed_of(local));
    let m = textureLoad(below, local, 0).r;
    return vec4<f32>(p.x + m * (1.0 - p.w), 0.0, 0.0, 1.0);
}

// A paint or fill effect, blended with its mode and opacity.
@fragment
fn fs_mask_effect(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let local = chunk_texel(frag);
    let s = seed_of(local);
    var top: vec4<f32>;
    if (pass_.info.z & FLAG_PAINT) != 0u {
        top = paint_at(s);
    } else {
        top = vec4<f32>(fill_value(s, local).x, 0.0, 0.0, 1.0);
    }
    let m = textureLoad(below, local, 0).r;
    let out = blend_premultiplied(vec4<f32>(m, 0.0, 0.0, 1.0), top, pass_.params.x, pass_.info.x, KIND_SCALAR, true);
    return vec4<f32>(out.x, 0.0, 0.0, 1.0);
}

// ---- Blur: a separable Gaussian over the mask below ----
//
// The radius (params.y, in texels) spans three standard deviations. The compositor widens
// the chunk's apron by the radius, so every texel read here was evaluated; reads past the
// temp texture are clamped to its edge, which only touches texels outside the chunk.

fn blur_line(source: texture_2d<f32>, local: vec2<i32>, step: vec2<i32>) -> f32 {
    let radius = pass_.params.y;
    let taps = i32(ceil(radius));
    let last = vec2<i32>(textureDimensions(source)) - 1;
    let sigma = max(radius / 3.0, 1e-3);
    var sum = 0.0;
    var weights = 0.0;
    for (var i = -taps; i <= taps; i++) {
        let w = exp(-f32(i * i) / (2.0 * sigma * sigma));
        sum += w * textureLoad(source, clamp(local + step * i, vec2<i32>(0), last), 0).r;
        weights += w;
    }
    return sum / weights;
}

// Along x, into a temp texture of its own.
@fragment
fn fs_mask_blur_x(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(blur_line(below, chunk_texel(frag), vec2<i32>(1, 0)), 0.0, 0.0, 1.0);
}

// Along y over the first pass (`other`), blended onto the mask below with the effect's
// mode and opacity.
@fragment
fn fs_mask_blur_y(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let local = chunk_texel(frag);
    let blurred = blur_line(other, local, vec2<i32>(0, 1));
    let m = textureLoad(below, local, 0).r;
    let out = blend_premultiplied(vec4<f32>(m, 0.0, 0.0, 1.0), vec4<f32>(blurred, 0.0, 0.0, 1.0), pass_.params.x, pass_.info.x, KIND_SCALAR, true);
    return vec4<f32>(out.x, 0.0, 0.0, 1.0);
}

@fragment
fn fs_mask_levels(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let local = chunk_texel(frag);
    let m = textureLoad(below, local, 0).r;
    let l = pass_.levels;
    let span = l.y - l.x;
    var t = select(step(l.x, m), clamp((m - l.x) / span, 0.0, 1.0), abs(span) > 1e-6);
    t = pow(t, 1.0 / max(l.z, 1e-3));
    return vec4<f32>(mix(l.w, pass_.levels2.x, t), 0.0, 0.0, 1.0);
}
