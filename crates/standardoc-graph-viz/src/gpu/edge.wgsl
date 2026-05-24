// 3D edge rendering — one line-list segment per graph edge. Both
// endpoints are force-layout node centres; the colour comes from the
// edge kind (resolved host-side via `Palette::edge_color`). Shares the
// node pipeline's camera uniform at @group(0) @binding(0).

struct CameraUniform {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

struct VertexInput {
    @location(0) pos:   vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(v: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = camera.proj * (camera.view * vec4<f32>(v.pos, 1.0));
    out.color = v.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
