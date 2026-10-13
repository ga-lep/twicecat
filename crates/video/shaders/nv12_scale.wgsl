// NV12 to NV12 at another size (colours untouched): each invocation writes
// one chroma sample and the 2x2 luma samples under it, each a box filter
// over the source samples it covers (at most 8 taps per axis).

struct Params {
    src_size: vec2<u32>,
    dst_size: vec2<u32>,
    kr: f32,
    kb: f32,
    full_range: u32,
    pad: u32,
}

var<push_constant> p: Params;

@group(0) @binding(0) var src_luma: texture_2d<f32>;
@group(0) @binding(1) var src_chroma: texture_2d<f32>;
@group(0) @binding(2) var dst_luma: texture_storage_2d<r8unorm, write>;
@group(0) @binding(3) var dst_chroma: texture_storage_2d<rg8unorm, write>;

fn footprint(d: u32, dst: u32, src_len: u32) -> vec2<u32> {
    let lo = min(d * src_len / dst, src_len - 1u);
    let hi = max(lo + 1u, min(src_len, ((d + 1u) * src_len + dst - 1u) / dst));
    return vec2<u32>(lo, hi);
}

fn luma(px: vec2<u32>) -> f32 {
    let fx = footprint(px.x, p.dst_size.x, p.src_size.x);
    let fy = footprint(px.y, p.dst_size.y, p.src_size.y);
    let nx = min(fx.y - fx.x, 8u);
    let ny = min(fy.y - fy.x, 8u);
    var sum = 0.0;
    for (var j = 0u; j < ny; j++) {
        let y = fy.x + j * (fy.y - fy.x) / ny;
        for (var i = 0u; i < nx; i++) {
            let x = fx.x + i * (fx.y - fx.x) / nx;
            sum += textureLoad(src_luma, vec2<u32>(x, y), 0).r;
        }
    }
    return sum / f32(nx * ny);
}

fn chroma(px: vec2<u32>) -> vec2<f32> {
    let src = p.src_size / 2u;
    let dst = p.dst_size / 2u;
    let fx = footprint(px.x, dst.x, src.x);
    let fy = footprint(px.y, dst.y, src.y);
    let nx = min(fx.y - fx.x, 8u);
    let ny = min(fy.y - fy.x, 8u);
    var sum = vec2<f32>(0.0);
    for (var j = 0u; j < ny; j++) {
        let y = fy.x + j * (fy.y - fy.x) / ny;
        for (var i = 0u; i < nx; i++) {
            let x = fx.x + i * (fx.y - fx.x) / nx;
            sum += textureLoad(src_chroma, vec2<u32>(x, y), 0).rg;
        }
    }
    return sum / f32(nx * ny);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = id.xy;
    let chroma_size = p.dst_size / 2u;
    if (c.x >= chroma_size.x || c.y >= chroma_size.y) {
        return;
    }
    for (var dy = 0u; dy < 2u; dy++) {
        for (var dx = 0u; dx < 2u; dx++) {
            let l = c * 2u + vec2<u32>(dx, dy);
            textureStore(dst_luma, l, vec4<f32>(luma(l), 0.0, 0.0, 1.0));
        }
    }
    textureStore(dst_chroma, c, vec4<f32>(chroma(c), 0.0, 1.0));
}
