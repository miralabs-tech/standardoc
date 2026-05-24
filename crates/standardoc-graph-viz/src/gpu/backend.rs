//! WebGPU backend internals for the 3D graph view.
//!
//! Two pipelines share one camera uniform: instanced billboard quads
//! for the nodes (a signed-distance field gives crisp rounded corners
//! and stroke bands) and a line list for the edges. Node billboards
//! are expanded in the vertex stage so each card faces the camera;
//! both pipelines depth-test against a shared `Depth32Float` buffer.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::HtmlCanvasElement;
use wgpu::util::DeviceExt;

use crate::camera::Camera3D;
use crate::kind::Kind;
use crate::palette::{Palette, entry_point_halo_color, kind_color_hex};

const INITIAL_INSTANCE_CAPACITY: u64 = 4096;
/// Initial edge vertex buffer capacity — two vertices per edge.
const INITIAL_EDGE_CAPACITY: u64 = 8192;
const CHIP_RADIUS: f32 = 4.0;
const CHIP_STROKE_WIDTH: f32 = 1.0;
/// Depth attachment format for the 3D pass.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

#[rustfmt::skip]
const QUAD_VERTICES: &[QuadVertex] = &[
    QuadVertex { corner: [0.0, 0.0] },
    QuadVertex { corner: [1.0, 0.0] },
    QuadVertex { corner: [0.0, 1.0] },
    QuadVertex { corner: [1.0, 1.0] },
];

const QUAD_INDICES: &[u16] = &[0, 1, 2, 1, 3, 2];

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct NodeInstance {
    /// World-space node centre. The vertex shader billboards the unit
    /// quad around this point so the card always faces the camera.
    world_pos: [f32; 3],
    size: [f32; 2],
    color: [f32; 4],
    stroke: [f32; 4],
    /// `[corner_radius, stroke_width, is_cube_flag, reserved]`.
    /// `is_cube_flag` ⇒ 1.0 = container (hex cube), 0.0 = leaf (sphere).
    /// Last slot is picked up by upcoming hover highlight / LOD fade
    /// without forcing a layout change.
    params: [f32; 4],
    /// Language accent bar color (RGBA, 0..1).
    accent: [f32; 4],
    /// Phase 3 (Flow) entry-point halo color (RGBA, 0..1). `a == 0`
    /// ⇒ no halo: the impostor fills the full quad as before. `a > 0`
    /// ⇒ the vertex shader inflates the billboard quad so the shape
    /// keeps its full apparent size, and the fragment shader paints a
    /// faded aura in the inflated margin.
    halo: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct CameraUniform {
    /// View and projection kept separate — the billboard vertex stage
    /// places the node centre in view space, then expands the quad in
    /// the view plane before projecting.
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
}

/// One endpoint of a graph edge — a line-list vertex carrying its own
/// colour so the edge pipeline needs no per-edge bindings.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct EdgeVertex {
    pos: [f32; 3],
    color: [f32; 4],
}

/// One node of the currently-focused drill level — built host-side by
/// `GraphEngine` from the drill tree and the per-level force layout.
/// `center` is the live force-layout position (for primaries) or a
/// fixed ring position (for ghosts); `size` scales with the subtree
/// weight; `kind` + `is_project` + `language` together pick the body
/// colour the same way the 2D card view does; `is_container` flips
/// the fragment shader between a sphere impostor (leaves) and a
/// stylised cube (containers); `is_ghost` dims the body alpha so
/// sibling-of-focus context reads as "outside the current level".
pub(crate) struct LevelNode {
    pub center: Vec3,
    pub size: [f32; 2],
    pub language: String,
    pub kind: Kind,
    pub is_project: bool,
    pub is_container: bool,
    pub is_ghost: bool,
    /// Phase 3 (Flow) entry-point tag, mirrored from `TreeNode`. When
    /// `Some(_)` the GPU receives a non-zero `halo` colour on the
    /// instance and the fragment shader renders a coloured aura
    /// around the impostor shape. `None` for synthetic projects and
    /// for internal symbols.
    pub entry_point: Option<String>,
}

pub(crate) struct WebGpuBackend {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    edge_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    instance_count: u32,
    edge_vertex_buffer: wgpu::Buffer,
    edge_capacity: u64,
    edge_vertex_count: u32,
    uniform_buffer: wgpu::Buffer,
    clear_color: wgpu::Color,
}

impl WebGpuBackend {
    /// Spin up an adapter + device against the given canvas. Async
    /// because both `request_adapter` and `request_device` return
    /// futures backed by JS promises on the web target.
    pub(crate) async fn init(
        canvas: HtmlCanvasElement,
        width: u32,
        height: u32,
    ) -> Result<Self, JsValue> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            flags: wgpu::InstanceFlags::default(),
            backend_options: wgpu::BackendOptions::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        });

        let surface_target = wgpu::SurfaceTarget::Canvas(canvas);
        let surface = instance
            .create_surface(surface_target)
            .map_err(|e| JsValue::from_str(&format!("create_surface: {e}")))?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("request_adapter: {e}")))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("standardoc-graph-viz"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("request_device: {e}")))?;

        let capabilities = surface.get_capabilities(&adapter);
        // Prefer an SRGB format so colours land on screen exactly as
        // hex-decoded; fall back to whatever the surface offers when
        // none is available (some Chromium builds expose only
        // `Rgba8Unorm`).
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(capabilities.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: capabilities.present_modes[0],
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        let depth_view = create_depth_view(&device, surface_config.width, surface_config.height);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chip-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("chip.wgsl").into()),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view-uniform"),
            size: size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view-bind-group-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view-bind-group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chip-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("chip-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: size_of::<QuadVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: size_of::<NodeInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            1 => Float32x3,
                            2 => Float32x2,
                            3 => Float32x4,
                            4 => Float32x4,
                            5 => Float32x4,
                            6 => Float32x4,
                            7 => Float32x4,
                        ],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            // `LessEqual` keeps coplanar instances (every chip sits on
            // the Z=0 plane until the force layout lands) painting in
            // submission order, while still occluding geometry pushed
            // to a different depth in later stages.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let edge_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("edge-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("edge.wgsl").into()),
        });

        let edge_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("edge-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &edge_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: size_of::<EdgeVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &edge_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad-vertices"),
            contents: bytemuck::cast_slice(QUAD_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad-indices"),
            contents: bytemuck::cast_slice(QUAD_INDICES),
            usage: wgpu::BufferUsages::INDEX,
        });

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chip-instances"),
            size: INITIAL_INSTANCE_CAPACITY * size_of::<NodeInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let edge_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("edge-vertices"),
            size: INITIAL_EDGE_CAPACITY * size_of::<EdgeVertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            depth_view,
            pipeline,
            edge_pipeline,
            bind_group,
            vertex_buffer,
            index_buffer,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            instance_count: 0,
            edge_vertex_buffer,
            edge_capacity: INITIAL_EDGE_CAPACITY,
            edge_vertex_count: 0,
            uniform_buffer,
            clear_color: wgpu::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        })
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.surface_config.width = width.max(1);
        self.surface_config.height = height.max(1);
        self.surface.configure(&self.device, &self.surface_config);
        self.depth_view = create_depth_view(
            &self.device,
            self.surface_config.width,
            self.surface_config.height,
        );
    }

    /// Upload one drill level: a billboard per `nodes` entry, plus a
    /// line segment per aggregated `edges` pair (index pairs into
    /// `nodes`). `nodes` is small by construction — only the focused
    /// node's direct children — so this re-runs cheaply each frame
    /// while the level's force layout is still settling.
    pub(crate) fn upload_scene(
        &mut self,
        nodes: &[LevelNode],
        edges: &[(u32, u32)],
        palette: &Palette,
    ) {
        let stroke = parse_hex(&palette.panel_border);
        let instances: Vec<NodeInstance> = nodes
            .iter()
            .map(|n| {
                // `params.z` encodes the impostor shape — 1.0 ⇒ cube
                // (container: project / module / struct …),
                // 0.0 ⇒ sphere (leaf: function / value / …). Read by
                // the fragment shader to branch the SDF.
                let shape_flag = if n.is_container { 1.0 } else { 0.0 };
                // Ghosts (sibling-of-focus context) dim their body
                // alpha so they read as "outside the current level"
                // without changing the WGSL — the fragment outputs
                // `in.fill.a * mask`, so reducing the instance color
                // alpha here propagates through both impostor shapes.
                let mut body = parse_hex(node_body_hex(n, palette));
                if n.is_ghost {
                    body[3] *= 0.45;
                }
                // Phase 3 (Flow) halo — non-zero alpha tells the
                // shader to inflate the quad and paint an aura
                // around the impostor. Entry-points on ghost
                // siblings keep their identity (a `main` is a
                // `main` even when shown as context) but with a
                // matching alpha cut so the halo doesn't overpower
                // the dimmed body.
                let halo = n
                    .entry_point
                    .as_deref()
                    .and_then(entry_point_halo_color)
                    .map(|hex| {
                        let mut rgba = parse_hex(hex);
                        rgba[3] = if n.is_ghost { 0.27 } else { 0.6 };
                        rgba
                    })
                    .unwrap_or([0.0, 0.0, 0.0, 0.0]);
                NodeInstance {
                    world_pos: [n.center.x, n.center.y, n.center.z],
                    size: n.size,
                    color: body,
                    stroke,
                    params: [CHIP_RADIUS, CHIP_STROKE_WIDTH, shape_flag, 0.0],
                    accent: parse_hex(palette.language_color(&n.language)),
                    halo,
                }
            })
            .collect();

        // Dependency wires — each endpoint coloured with its own
        // node's body hex so the line draws a gradient between the
        // two kinds / projects. Replaces the previous monochromatic
        // `text_link` blue with something that *says something*
        // visually: module→function reads purple→blue, project→
        // project ecosystem-coloured, etc.
        let mut edge_verts: Vec<EdgeVertex> = Vec::with_capacity(edges.len() * 2);
        for &(a, b) in edges {
            let (Some(na), Some(nb)) = (nodes.get(a as usize), nodes.get(b as usize)) else {
                continue;
            };
            let mut color_a = parse_hex(node_body_hex(na, palette));
            let mut color_b = parse_hex(node_body_hex(nb, palette));
            // Ghost-touching edges fade so they read as background
            // context, matching the 2D dashed treatment.
            if na.is_ghost || nb.is_ghost {
                color_a[3] *= 0.55;
                color_b[3] *= 0.55;
            }
            edge_verts.push(EdgeVertex {
                pos: [na.center.x, na.center.y, na.center.z],
                color: color_a,
            });
            edge_verts.push(EdgeVertex {
                pos: [nb.center.x, nb.center.y, nb.center.z],
                color: color_b,
            });
        }

        self.clear_color = wgpu_color(&palette.background);
        self.ensure_instance_capacity(instances.len() as u64);
        self.queue
            .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));
        self.instance_count = instances.len() as u32;

        self.ensure_edge_capacity(edge_verts.len() as u64);
        self.queue
            .write_buffer(&self.edge_vertex_buffer, 0, bytemuck::cast_slice(&edge_verts));
        self.edge_vertex_count = edge_verts.len() as u32;
    }

    /// Re-upload only the view-projection matrix. Cheap: a single
    /// 64-byte mat4 — no instance buffer touch. Call this whenever the
    /// orbit camera changes (orbit/dolly/pan) or the canvas resizes.
    pub(crate) fn upload_view(&mut self, camera: &Camera3D) {
        let aspect =
            self.surface_config.width as f32 / self.surface_config.height.max(1) as f32;
        let uniform = CameraUniform {
            view: camera.view().to_cols_array_2d(),
            proj: camera.proj(aspect).to_cols_array_2d(),
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    pub(crate) fn render(&mut self) -> Result<(), JsValue> {
        // wgpu 29: `get_current_texture` returns the `CurrentSurfaceTexture`
        // enum — Success / Suboptimal yield a `SurfaceTexture`; every other
        // variant means we should skip the frame and either reconfigure
        // (Outdated) or just bail (Lost / Timeout / Occluded / Validation).
        let frame: wgpu::SurfaceTexture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(JsValue::from_str("surface lost — recreate the backend"));
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(JsValue::from_str("get_current_texture: validation error"));
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame-encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // Edges first so the node billboards paint over the line
            // stubs entering each node centre.
            if self.edge_vertex_count > 0 {
                pass.set_pipeline(&self.edge_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.edge_vertex_buffer.slice(..));
                pass.draw(0..self.edge_vertex_count, 0..1);
            }

            if self.instance_count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
                pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..QUAD_INDICES.len() as u32, 0, 0..self.instance_count);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }

    /// Number of instances rendered in the most recent
    /// `upload_scene` pass. Surfaced to the JS profiler via
    /// `GraphEngine::gpu_instance_count` so the HUD can show how
    /// much geometry the WebGPU path is actually touching.
    pub(crate) fn instance_count(&self) -> u32 {
        self.instance_count
    }

    /// Capacity (in instances) of the current `instance_buffer`. Grows
    /// monotonically by powers of two via `ensure_instance_capacity`;
    /// the gap between `instance_count` and `instance_capacity`
    /// indicates head-room before the next reallocation. Clamped to
    /// `u32::MAX` for the JS bridge — the underlying field is `u64`
    /// but no realistic scene approaches that range.
    pub(crate) fn instance_capacity(&self) -> u32 {
        self.instance_capacity.min(u64::from(u32::MAX)) as u32
    }

    fn ensure_instance_capacity(&mut self, needed: u64) {
        if needed <= self.instance_capacity {
            return;
        }
        let new_capacity = needed.next_power_of_two().max(INITIAL_INSTANCE_CAPACITY);
        self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chip-instances"),
            size: new_capacity * size_of::<NodeInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = new_capacity;
    }

    fn ensure_edge_capacity(&mut self, needed: u64) {
        if needed <= self.edge_capacity {
            return;
        }
        let new_capacity = needed.next_power_of_two().max(INITIAL_EDGE_CAPACITY);
        self.edge_vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("edge-vertices"),
            size: new_capacity * size_of::<EdgeVertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.edge_capacity = new_capacity;
    }
}

/// Allocate the depth attachment for the 3D pass. Recreated on every
/// resize since the depth texture must match the surface dimensions.
fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth-texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Body hex colour for a level node — synthetic project nodes take
/// their ecosystem colour, symbol nodes take their per-`Kind` colour.
/// Same mapping as the 2D card header (`render::card_header_color`)
/// so a node's identity reads the same across both backends.
fn node_body_hex<'a>(n: &'a LevelNode, palette: &'a Palette) -> &'a str {
    if n.is_project {
        palette.project_color(&n.language)
    } else {
        kind_color_hex(n.kind)
    }
}

fn parse_hex(s: &str) -> [f32; 4] {
    let trimmed = s.trim_start_matches('#');
    let bytes = trimmed.as_bytes();
    if bytes.len() < 6 {
        // Fallback magenta so a malformed palette is immediately
        // visible in the rendered scene rather than silently white.
        return [1.0, 0.0, 1.0, 1.0];
    }
    let parse = |a: u8, b: u8| -> f32 {
        let s = [a as char, b as char];
        let s: String = s.iter().collect();
        u8::from_str_radix(&s, 16).unwrap_or(0) as f32 / 255.0
    };
    let r = parse(bytes[0], bytes[1]);
    let g = parse(bytes[2], bytes[3]);
    let b = parse(bytes[4], bytes[5]);
    let a = if bytes.len() >= 8 {
        parse(bytes[6], bytes[7])
    } else {
        1.0
    };
    [r, g, b, a]
}

fn wgpu_color(hex: &str) -> wgpu::Color {
    let [r, g, b, a] = parse_hex(hex);
    wgpu::Color {
        r: r as f64,
        g: g as f64,
        b: b as f64,
        a: a as f64,
    }
}

/// Silence the unused import lint when the Canvas2D backend is the
/// only one wired in by a downstream consumer; once GraphEngine
/// dispatches to this backend, the cast is exercised by the
/// `wgpu::SurfaceTarget::Canvas` path.
#[allow(dead_code)]
fn _force_jscast(canvas: HtmlCanvasElement) -> Option<HtmlCanvasElement> {
    canvas.dyn_into().ok()
}
