// Draws one decoded video frame (NV12 or I420) into the rectangle iced gives
// the widget. The frame arrives as separate planes: Y in `plane_y`, then
// either the interleaved UV plane in `plane_a` (NV12, `planar` = 0) or U in
// `plane_a` and V in `plane_b` (I420, `planar` = 1). The unused `plane_b` of
// an NV12 frame is a 1 x 1 texture.
//
// A full-screen triangle is scaled down by `fit` so that the picture keeps its
// shape inside the widget (the rest of the widget shows what is behind it),
// and `mirror` flips it left to right for our own camera.

struct Params {
    // Fit scale in clip space: (1, 1) fills the widget.
    fit: vec2<f32>,
    // 1.0 mirrors the picture left to right.
    mirror: f32,
    // 1.0: U and V are separate planes (I420); 0.0: one interleaved plane.
    planar: f32,
    // Y: (y - y_offset) * y_scale; U and V: (c - 0.5) * c_scale.
    y_offset: f32,
    y_scale: f32,
    c_scale: f32,
    // 1.0 when the target is an sRGB surface: the picture is already
    // gamma-encoded, so it is turned back to linear for the hardware to
    // encode it again.
    linearise: f32,
    // R = y + rv * cr; G = y - gu * cb - gv * cr; B = y + bu * cb.
    rv: f32,
    gu: f32,
    gv: f32,
    bu: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var plane_y: texture_2d<f32>;
@group(0) @binding(2) var plane_a: texture_2d<f32>;
@group(0) @binding(3) var plane_b: texture_2d<f32>;
@group(0) @binding(4) var smp: sampler;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Vertex {
    // One triangle that covers the whole clip space.
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: Vertex;
    out.position = vec4<f32>((corner * 2.0 - 1.0) * params.fit * vec2<f32>(1.0, -1.0), 0.0, 1.0);
    var u = corner.x;
    if params.mirror > 0.5 {
        u = 1.0 - u;
    }
    out.uv = vec2<f32>(u, corner.y);
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: Vertex) -> @location(0) vec4<f32> {
    let y = (textureSample(plane_y, smp, in.uv).r - params.y_offset) * params.y_scale;
    var cb: f32;
    var cr: f32;
    if params.planar > 0.5 {
        cb = textureSample(plane_a, smp, in.uv).r;
        cr = textureSample(plane_b, smp, in.uv).r;
    } else {
        let uv = textureSample(plane_a, smp, in.uv).rg;
        cb = uv.r;
        cr = uv.g;
    }
    cb = (cb - 0.5) * params.c_scale;
    cr = (cr - 0.5) * params.c_scale;
    var rgb = vec3<f32>(
        y + params.rv * cr,
        y - params.gu * cb - params.gv * cr,
        y + params.bu * cb,
    );
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if params.linearise > 0.5 {
        rgb = srgb_to_linear(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
