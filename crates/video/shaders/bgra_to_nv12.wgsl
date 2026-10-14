// BGRA (or RGBA) to NV12 at another size: each invocation writes one
// chroma sample and the 2x2 luma samples under it. Scaling is a box
// filter over the source pixels each output pixel covers (at most 8 taps
// per axis), so text stays readable when a 4K screen goes to 540p.

struct Params {
    src_size: vec2<u32>,
    dst_size: vec2<u32>,
    // Y' = kr R' + (1 - kr - kb) G' + kb B'
    kr: f32,
    kb: f32,
    full_range: u32,
    pad: u32,
}

var<push_constant> p: Params;

// Read through a B8G8R8A8 or R8G8B8A8 view, so `.rgb` is in order.
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(2) var dst_luma: texture_storage_2d<r8unorm, write>;
@group(0) @binding(3) var dst_chroma: texture_storage_2d<rg8unorm, write>;

// The source pixels [lo, hi) that output pixel `d` covers on one axis.
fn footprint(d: u32, dst: u32, src_len: u32) -> vec2<u32> {
    let lo = min(d * src_len / dst, src_len - 1u);
    let hi = max(lo + 1u, min(src_len, ((d + 1u) * src_len + dst - 1u) / dst));
    return vec2<u32>(lo, hi);
}

fn average(px: vec2<u32>) -> vec3<f32> {
    let fx = footprint(px.x, p.dst_size.x, p.src_size.x);
    let fy = footprint(px.y, p.dst_size.y, p.src_size.y);
    let nx = min(fx.y - fx.x, 8u);
    let ny = min(fy.y - fy.x, 8u);
    var sum = vec3<f32>(0.0);
    for (var j = 0u; j < ny; j++) {
        let y = fy.x + j * (fy.y - fy.x) / ny;
        for (var i = 0u; i < nx; i++) {
            let x = fx.x + i * (fx.y - fx.x) / nx;
            sum += textureLoad(src, vec2<u32>(x, y), 0).rgb;
        }
    }
    return sum / f32(nx * ny);
}

fn to_luma(y: f32) -> f32 {
    if (p.full_range != 0u) {
        return y;
    }
    return (16.0 + 219.0 * y) / 255.0;
}

fn to_chroma(c: f32) -> f32 {
    if (p.full_range != 0u) {
        return clamp((128.0 + 255.0 * c) / 255.0, 0.0, 1.0);
    }
    return (128.0 + 224.0 * c) / 255.0;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = id.xy;
    let chroma_size = p.dst_size / 2u;
    if (c.x >= chroma_size.x || c.y >= chroma_size.y) {
        return;
    }
    let kg = 1.0 - p.kr - p.kb;
    let k = vec3<f32>(p.kr, kg, p.kb);
    var block = vec3<f32>(0.0);
    for (var dy = 0u; dy < 2u; dy++) {
        for (var dx = 0u; dx < 2u; dx++) {
            let l = c * 2u + vec2<u32>(dx, dy);
            let rgb = average(l);
            block += rgb;
            textureStore(dst_luma, l, vec4<f32>(to_luma(dot(k, rgb)), 0.0, 0.0, 1.0));
        }
    }
    let rgb = block * 0.25;
    let y = dot(k, rgb);
    let cb = (rgb.b - y) / (2.0 * (1.0 - p.kb));
    let cr = (rgb.r - y) / (2.0 * (1.0 - p.kr));
    textureStore(dst_chroma, c, vec4<f32>(to_chroma(cb), to_chroma(cr), 0.0, 1.0));
}
