// Instanced sphere-impostor rendering. One static unit-quad in the
// vertex buffer, one row per node in the instance buffer. The whole
// scene draws in a single indexed-instanced draw call — that's the
// pattern that scales to 10⁵+ symbols without a per-object overhead
// trail. The fragment stage paints each quad as a shaded sphere, not
// a chip — so what reads as 3D balls is still a single draw call.

struct CameraUniform {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

struct VertexInput {
    // Unit quad corner in [0,1]² — billboarded around the instance's
    // world-space centre in the vertex stage.
    @location(0) corner: vec2<f32>,
};

struct InstanceInput {
    // World-space node centre.
    @location(1) world_pos: vec3<f32>,
    @location(2) size:      vec2<f32>,
    @location(3) color:     vec4<f32>,
    @location(4) stroke:    vec4<f32>,
    // x: corner radius (world units). y: stroke width (world units).
    // z: 1.0 = cube impostor (containers), 0.0 = sphere (leaves).
    // w: reserved (LOD bias).
    @location(5) params:    vec4<f32>,
    // Language accent bar color (RGBA, 0..1).
    @location(6) accent:    vec4<f32>,
    // Phase 3 (Flow) entry-point halo color (RGBA, 0..1). a > 0 ⇒
    // the vertex stage inflates the quad and the fragment stage
    // paints a faded aura in the margin around the impostor shape.
    @location(7) halo:      vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) fill:   vec4<f32>,
    @location(1) stroke: vec4<f32>,
    // Position in the chip's local space `[0, expanded_size]` — the
    // fragment stage divides by `size` to recover `uv ∈ [-1, 1]` over
    // the inflated quad, then compares against `shape_extent` to know
    // whether the pixel sits in the impostor or the halo ring.
    @location(2) local:  vec2<f32>,
    @location(3) size:   vec2<f32>,
    @location(4) params: vec4<f32>,
    @location(5) accent: vec4<f32>,
    @location(6) halo:   vec4<f32>,
    // Fraction of `uv` radius (sphere) / hex mask (cube) occupied by
    // the impostor shape itself. 1.0 when no halo (shape fills the
    // quad). < 1.0 when halo present (the remainder of the quad is
    // halo ring). Computed once in the vertex stage so all 4 quad
    // vertices interpolate the same constant.
    @location(7) shape_extent: f32,
};

// How much to inflate the billboard quad when an entry-point halo is
// requested. 1.0 = no inflation; 1.35 ⇒ the halo ring takes ~26% of
// the screen-space radius and the shape keeps ~74%. Chosen so the
// halo reads as a clear aura without overpowering the impostor.
const HALO_INFLATION: f32 = 1.35;

@vertex
fn vs_main(v: VertexInput, i: InstanceInput) -> VertexOutput {
    // Inflate the quad when a halo is requested so the aura has room
    // to render outside the impostor silhouette. `shape_extent` then
    // tells the fragment shader the radius (in normalised uv) at
    // which the impostor ends and the halo ring begins.
    let inflate = select(1.0, HALO_INFLATION, i.halo.a > 0.01);
    let expanded_size = i.size * inflate;
    // Billboard: place the node centre in view space, then expand the
    // unit quad in the view plane so the card always faces the camera.
    let center_view = camera.view * vec4<f32>(i.world_pos, 1.0);
    let local = v.corner * expanded_size;
    let centred = local - expanded_size * 0.5;
    let pos_view = center_view + vec4<f32>(centred, 0.0, 0.0);
    var out: VertexOutput;
    out.clip_pos = camera.proj * pos_view;
    out.fill   = i.color;
    out.stroke = i.stroke;
    out.local  = local;
    out.size   = expanded_size;
    out.params = i.params;
    out.accent = i.accent;
    out.halo   = i.halo;
    out.shape_extent = 1.0 / inflate;
    return out;
}

// Dual-shape impostor. The billboard quad is treated as the screen-
// space projection of either a sphere (leaves: functions, values, …)
// or a stylised cube (containers: projects, modules, structs). The
// instance buffer's `params.z` flips between the two — 0.0 ⇒ sphere,
// 1.0 ⇒ cube. The shape signal lets the user tell hierarchy levels
// apart at a glance, complementing the per-`Kind` body colour.
//
// When `halo.a > 0` the quad was inflated in the vertex stage; the
// shape now sits in the inner disk `metric < shape_extent` and the
// halo aura paints the ring `shape_extent ≤ metric < 1`. `metric` is
// `length(uv)` for the sphere (Euclidean radius) and the hex SDF
// `|y| + |x|/√3` for the cube — so the halo silhouette follows the
// impostor shape rather than always being a circle.
//
// No `frag_depth` output for either branch — overlapping nodes z-test
// against the quad-centre depth. Acceptable visually (true per-pixel
// depth would need passing the view-space centre + world radius
// through to the fragment).
const HEX_INV_ROOT3: f32 = 0.57735027;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = (in.local / in.size) * 2.0 - vec2<f32>(1.0, 1.0);
    let is_cube = in.params.z > 0.5;
    let has_halo = in.halo.a > 0.01;
    let extent = in.shape_extent;
    let metric = select(length(uv), abs(uv.y) + abs(uv.x) * HEX_INV_ROOT3, is_cube);

    if (!has_halo) {
        if (is_cube) { return shade_cube(in, uv); }
        return shade_sphere(in, uv);
    }

    // Halo present: the inflated quad means `metric ∈ [0, 1]` covers
    // the whole renderable area; the impostor lives in `[0, extent]`
    // and the halo ring in `[extent, 1]`. Re-scale uv into the
    // shape's own `[-1, 1]` frame before delegating to the SDFs so
    // they keep their existing silhouette logic intact.
    if (metric < extent) {
        let shape_uv = uv / extent;
        if (is_cube) { return shade_cube(in, shape_uv); }
        return shade_sphere(in, shape_uv);
    }
    if (metric > 1.0) {
        discard;
    }
    let aa = max(fwidth(metric), 0.0001);
    let outer_alpha = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, metric);
    let falloff = 1.0 - smoothstep(extent, 1.0, metric);
    let halo_alpha = in.halo.a * falloff * outer_alpha;
    return vec4<f32>(in.halo.rgb, halo_alpha);
}

fn shade_sphere(in: VertexOutput, uv: vec2<f32>) -> vec4<f32> {
    let r2 = dot(uv, uv);
    if (r2 > 1.0) {
        discard;
    }
    let aa = max(fwidth(r2), 0.0001);
    let alpha = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, r2);
    // For a unit sphere facing the camera, `(uv.x, uv.y, z)` IS the
    // normal (already length 1, since uv² + z² = 1).
    let z = sqrt(max(1.0 - r2, 0.0));
    let normal = vec3<f32>(uv.x, uv.y, z);
    let light_dir = normalize(vec3<f32>(0.4, -0.5, 0.8));
    let diffuse = max(dot(normal, light_dir), 0.0);
    let ambient = 0.30;
    let lit = ambient + (1.0 - ambient) * diffuse;
    let half_dir = normalize(light_dir + vec3<f32>(0.0, 0.0, 1.0));
    let spec = pow(max(dot(normal, half_dir), 0.0), 24.0) * 0.35;
    let body = in.fill.rgb * lit + vec3<f32>(spec, spec, spec);
    return vec4<f32>(body, in.fill.a * alpha);
}

fn shade_cube(in: VertexOutput, uv: vec2<f32>) -> vec4<f32> {
    // Isometric cube — the silhouette is the 2D projection of a
    // cube viewed from the canonical 30°/45° isometric angle (a
    // regular hexagon). The interior splits into three rhombi (top,
    // front-right, front-left), each lit to a different intensity so
    // the eye reads the shape as a 3D box, not a flat polygon. Two
    // dark inner lines along the rhombus boundaries sell the
    // wireframe edges of the cube without extra geometry.
    let inv_root3 = 0.57735027; // 1/sqrt(3)

    // Hex silhouette : |y| + |x|/√3 ≤ 1, with anti-aliased edge.
    let mask = abs(uv.y) + abs(uv.x) * inv_root3;
    if (mask > 1.0) {
        discard;
    }
    let aa = max(fwidth(mask), 0.0001);
    let alpha = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, mask);

    // Three rhombi meeting at the centre. The top face is bounded
    // below by the two lines y = ±x/√3 going through the origin;
    // the left/right split is the y-axis below that boundary.
    let top_boundary = -abs(uv.x) * inv_root3;
    let on_top = uv.y < top_boundary;
    var face_lit: f32;
    if (on_top) {
        face_lit = 1.25; // sun catches the upper face
    } else if (uv.x > 0.0) {
        face_lit = 0.85; // front-right (slight shade)
    } else {
        face_lit = 0.55; // front-left (in shadow)
    }

    // Wireframe — thin dark band along the 3 internal rhombus
    // borders. Distance computed as the screen-space perpendicular
    // distance to each meeting line (centre→top, centre→bottom-right,
    // centre→bottom-left). Only the line inside the current face
    // contributes, so the wireframe traces the cube's silhouette
    // edges and inner crease cleanly.
    let dist_vertical = abs(uv.x);                          // centre→top vertex (x = 0, y in [-1, 0])
    let dist_diag_right = abs(uv.y - uv.x * inv_root3) * 0.866; // centre→bottom-right
    let dist_diag_left = abs(uv.y + uv.x * inv_root3) * 0.866;  // centre→bottom-left
    var inner_d: f32 = 100.0;
    if (on_top) {
        // Top face boundaries are the two upper diagonals.
        inner_d = min(dist_diag_right, dist_diag_left);
    } else if (uv.x > 0.0) {
        // Front-right meets the vertical (centre→top) on its left
        // and the lower-right diagonal on its top.
        inner_d = min(dist_vertical, dist_diag_right);
    } else {
        inner_d = min(dist_vertical, dist_diag_left);
    }
    let wire_w = 0.025;
    let wire_aa = max(fwidth(inner_d), 0.0001);
    let wire_intensity = 1.0 - smoothstep(wire_w - wire_aa, wire_w + wire_aa, inner_d);

    let body = in.fill.rgb * face_lit * (1.0 - wire_intensity * 0.45);
    return vec4<f32>(body, in.fill.a * alpha);
}
