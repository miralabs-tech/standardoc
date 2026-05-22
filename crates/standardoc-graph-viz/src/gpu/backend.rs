//! WebGPU backend internals.
//!
//! All scene geometry — clusters AND chips — is rendered through a
//! single indexed-instanced draw call. The instance buffer carries
//! per-rectangle data (offset, size, fill, stroke, corner radius,
//! stroke width); the fragment shader resolves each fragment via a
//! signed-distance field so rounded corners and stroke bands stay
//! crisp at any zoom level without per-frame path construction.

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::HtmlCanvasElement;
use wgpu::util::DeviceExt;

use crate::palette::Palette;
use crate::scene::Scene;
use crate::viewport::Viewport;

const INITIAL_INSTANCE_CAPACITY: u64 = 4096;
const CLUSTER_RADIUS: f32 = 6.0;
const CHIP_RADIUS: f32 = 4.0;
const CLUSTER_STROKE_WIDTH: f32 = 1.0;
const CHIP_STROKE_WIDTH: f32 = 1.0;

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
struct ChipInstance {
    offset: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
    stroke: [f32; 4],
    /// `[corner_radius, stroke_width, reserved, reserved]`.
    /// Reserved slots are picked up by upcoming hover highlight + LOD
    /// fade work without forcing a layout change.
    params: [f32; 4],
    /// Language accent bar color (RGBA, 0..1). `a == 0` disables the
    /// bar — cluster frames use that to opt out.
    accent: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct ViewUniform {
    view_proj: [[f32; 4]; 4],
}

pub(crate) struct WebGpuBackend {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    instance_count: u32,
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

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chip-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("chip.wgsl").into()),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view-uniform"),
            size: size_of::<ViewUniform>() as u64,
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
                        array_stride: size_of::<ChipInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            1 => Float32x2,
                            2 => Float32x2,
                            3 => Float32x4,
                            4 => Float32x4,
                            5 => Float32x4,
                            6 => Float32x4,
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
            depth_stencil: None,
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
            size: INITIAL_INSTANCE_CAPACITY * size_of::<ChipInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            pipeline,
            bind_group,
            vertex_buffer,
            index_buffer,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            instance_count: 0,
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
    }

    /// Walk the scene and pack every container frame + chip into the
    /// instance buffer. Containers (both leaves and intermediates)
    /// land first so chips paint on top of them in the single
    /// back-to-front draw pass — no depth buffer needed at this
    /// scale, and back-to-front order is stable across runs.
    ///
    /// The Canvas2D path renders intermediates as stroke-only frames
    /// for a clearer nesting affordance. The WebGPU path uses a
    /// single fill style for both, since adding a second instance
    /// kind for stroke-only is not worth the shader complexity for
    /// the current scope. Visually the GPU mode loses the
    /// outline/fill distinction; switch to Canvas2D for the nicer
    /// look while a richer GPU pipeline lands.
    pub(crate) fn upload_scene(&mut self, scene: &Scene, palette: &Palette) {
        let mut instances: Vec<ChipInstance> =
            Vec::with_capacity(scene.hierarchy.nodes.len() + scene.nodes.len());

        let cluster_fill = parse_hex(&palette.widget_background);
        let cluster_stroke = parse_hex(&palette.panel_border);
        // Cluster frames opt out of the accent bar via alpha 0.
        let no_accent = [0.0_f32; 4];
        for c in &scene.hierarchy.nodes {
            instances.push(ChipInstance {
                offset: [c.x, c.y],
                size: [c.w, c.h],
                color: cluster_fill,
                stroke: cluster_stroke,
                params: [CLUSTER_RADIUS, CLUSTER_STROKE_WIDTH, 0.0, 0.0],
                accent: no_accent,
            });
        }

        // Project-frame header bands — one extra instance per project
        // node, kind-coloured, drawn over the cluster fills but under
        // the chips (chips sit below the header, so no overlap).
        let band_h = crate::layout::CONTAINER_HEADER_H;
        for c in &scene.hierarchy.nodes {
            let Some(kind) = c.project_kind.as_deref() else {
                continue;
            };
            instances.push(ChipInstance {
                offset: [c.x, c.y],
                size: [c.w, band_h.min(c.h)],
                color: parse_hex(palette.project_color(kind)),
                stroke: [0.0; 4],
                params: [CLUSTER_RADIUS, 0.0, 0.0, 0.0],
                accent: no_accent,
            });
        }

        let chip_fill = parse_hex(&palette.background);
        let chip_stroke = parse_hex(&palette.panel_border);
        for n in &scene.nodes {
            instances.push(ChipInstance {
                offset: [n.x as f32, n.y as f32],
                size: [n.w as f32, n.h as f32],
                color: chip_fill,
                stroke: chip_stroke,
                params: [CHIP_RADIUS, CHIP_STROKE_WIDTH, 0.0, 0.0],
                accent: parse_hex(palette.language_color(&n.language)),
            });
        }

        self.clear_color = wgpu_color(&palette.background);
        self.ensure_instance_capacity(instances.len() as u64);
        self.queue
            .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));
        self.instance_count = instances.len() as u32;
    }

    /// Re-upload only the view-projection matrix. Cheap: a single
    /// 64-byte mat4 — no instance buffer touch. Call this whenever
    /// the viewport (pan/zoom) or canvas size changes.
    pub(crate) fn upload_view(&mut self, viewport: &Viewport, width: f64, height: f64) {
        let proj = Mat4::orthographic_rh(
            0.0_f32,
            width as f32,
            // Swap top/bottom so Y grows downward, matching the
            // Canvas2D convention the rest of the codebase uses.
            height as f32,
            0.0_f32,
            -1.0,
            1.0,
        );
        let view = Mat4::from_translation(glam::vec3(
            viewport.offset_x as f32,
            viewport.offset_y as f32,
            0.0,
        )) * Mat4::from_scale(glam::vec3(
            viewport.scale as f32,
            viewport.scale as f32,
            1.0,
        ));
        let mat = proj * view;
        let uniform = ViewUniform {
            view_proj: mat.to_cols_array_2d(),
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
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

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
            size: new_capacity * size_of::<ChipInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = new_capacity;
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
