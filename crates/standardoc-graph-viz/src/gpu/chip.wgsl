// Instanced chip rendering. One static unit-quad in the vertex buffer,
// one row per chip in the instance buffer. The whole scene draws in a
// single indexed-instanced draw call — that's the pattern that scales
// to 10⁵+ symbols without a per-object overhead trail.

struct ViewUniform {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> view: ViewUniform;

struct VertexInput {
    // Unit quad corner in [0,1]² — multiplied by `size` and offset by
    // `offset` to land in world space.
    @location(0) corner: vec2<f32>,
};

struct InstanceInput {
    @location(1) offset:  vec2<f32>,
    @location(2) size:    vec2<f32>,
    @location(3) color:   vec4<f32>,
    @location(4) stroke:  vec4<f32>,
    // x: corner radius (world units). y: stroke width (world units).
    // z, w: reserved (highlight flag / LOD bias) for upcoming iterations.
    @location(5) params:  vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) fill:   vec4<f32>,
    @location(1) stroke: vec4<f32>,
    // Position in the chip's local space `[0, size]` — the fragment
    // stage uses it to compute the SDF for rounded corners + stroke.
    @location(2) local:  vec2<f32>,
    @location(3) size:   vec2<f32>,
    @location(4) params: vec4<f32>,
};

@vertex
fn vs_main(v: VertexInput, i: InstanceInput) -> VertexOutput {
    let world_pos = v.corner * i.size + i.offset;
    var out: VertexOutput;
    out.clip_pos = view.view_proj * vec4<f32>(world_pos, 0.0, 1.0);
    out.fill   = i.color;
    out.stroke = i.stroke;
    out.local  = v.corner * i.size;
    out.size   = i.size;
    out.params = i.params;
    return out;
}

// Signed-distance to a rounded rectangle centred at `half`, half-size
// `half`, corner radius `r`. Negative inside, zero on the edge,
// positive outside. Classic IQ formulation.
fn sd_round_rect(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p - half) - half + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let half = in.size * 0.5;
    let r = in.params.x;
    let stroke_w = max(in.params.y, 0.0);
    let d = sd_round_rect(in.local, half, r);

    // 1-pixel anti-aliased edge band, computed via fwidth so it stays
    // crisp under any view scale.
    let aa = fwidth(d);
    let fill_a = 1.0 - smoothstep(-aa, 0.0, d);
    if (fill_a <= 0.0) {
        discard;
    }

    var color = in.fill;
    if (stroke_w > 0.0) {
        // Blend stroke over fill near the edge. Stroke band is
        // `[-stroke_w, 0]` from the SDF; outside we discard, inside
        // the band we lerp from fill to stroke.
        let stroke_t = 1.0 - smoothstep(-stroke_w - aa, -stroke_w + aa, d);
        color = mix(in.stroke, in.fill, stroke_t);
    }
    color.a *= fill_a;
    return color;
}
