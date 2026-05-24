// This crate is wasm-only by design: the renderer drives a browser
// canvas (Canvas2D + WebGPU) and the public API is wired through
// `wasm-bindgen`. Gating the whole crate behind `wasm32` keeps
// `cargo check --workspace` (which defaults to the host target) green
// — without this guard, native compilation trips on `wgpu`'s
// `SurfaceTarget::Canvas` variant (`#[cfg(web)]` upstream), which has
// no native counterpart.
#![cfg(target_arch = "wasm32")]

//! WASM graph viewer for the standardoc semantic IR.
//!
//! This crate exposes a single `GraphEngine` object via `wasm-bindgen`.
//! Every interaction with JavaScript is statically typed — no
//! `js_sys::Reflect`, no dynamic property access — so the JS side gets
//! TypeScript declarations and the Rust side gets compile-time
//! verification of every cross-boundary call.
//!
//! Rendering uses `CanvasRenderingContext2d` (zero ThreeJS dependency).
//! The cluster-pack layout is implemented in [`layout`], the
//! pan/zoom math in [`viewport`], hit-testing and pointer state in
//! [`interaction`]. Data shapes the JS host POSTs at us live in
//! [`payload`], and theme colors are isolated in [`palette`] so the
//! renderer reads from a single struct rather than scattered constants.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_lossless)]

mod camera;
mod force3d;
mod gpu;
mod interaction;
mod kind;
mod layout;
mod palette;
mod payload;
mod render;
mod scene;
mod tree;
mod viewport;

/// One breadcrumb crumb — a tree node's display label and its
/// `DrillTree.nodes` index. Serialised into the `focus_path` JSON
/// the host renders as a clickable breadcrumb; `id` round-trips
/// back through `fit_to_frame` to refocus that node.
#[derive(serde::Serialize)]
struct FocusCrumb {
    label: String,
    id: u32,
}

/// One projected node label for the WebGPU view's DOM overlay. The
/// host pins a text element at `(x, y)` (CSS pixels) when `on` is
/// true; `on` is false when the node sits behind the camera.
#[derive(serde::Serialize)]
struct LabelPos {
    text: String,
    x: f32,
    y: f32,
    on: bool,
}

use std::collections::HashMap;

use glam::Vec3;
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, Performance, window};

use crate::camera::Camera3D;
use crate::force3d::Force3D;
use crate::gpu::{LevelNode, WebGpuBackend};
use crate::interaction::InteractionState;
use crate::palette::Palette;
use crate::payload::{EdgesPayload, GraphPayload};
use crate::scene::Scene;
use crate::tree::DrillTree;
use crate::viewport::Viewport;

/// Multiplier on the primary force-cloud radius for the ghost ring
/// in the 3D view. Picked empirically so ghosts sit clearly outside
/// the cloud even with sparse levels.
const GHOST_RING_RADIUS_FACTOR: f32 = 1.8;
/// Additive margin (world units) on the ghost ring, on top of the
/// scaled primary radius — keeps small clouds from making ghosts
/// land on top of primaries.
const GHOST_RING_MARGIN: f32 = 120.0;
/// Minimum subtree size for a container's label to be painted
/// permanently (without hover) in the 3D view. Below this, the
/// label only surfaces on hover — keeps the canvas from becoming a
/// wall of text while still pinning the high-signal anchors
/// (projects + heavy modules) so the user can orient without
/// scrubbing every node.
const LABEL_VISIBLE_THRESHOLD: u32 = 15;

/// Phase 3 (Flow) 3.4 — maximum number of entry-point satellites
/// rendered around a single project cube at the workspace overview
/// level. Surplus collapses into a single `+N` overflow badge so a
/// large binary with many `main` / `luaopen_*` exports doesn't drown
/// its own cube. Picked empirically: 5 is the sweet spot for an even
/// hexagonal ring (5 satellites + 1 badge = 6 slots).
const SATELLITE_CAP: usize = 5;

/// One Phase 3 (Flow) 3.4 satellite — an entry-point sphere orbiting
/// a project cube at the workspace overview level. `tree_idx ==
/// u32::MAX` flags an overflow badge (`overflow_count` tells the
/// label layer how many entry-points were elided). All other
/// satellites map to a real entry-point symbol whose fqdn drives the
/// click drill.
#[derive(Debug, Clone)]
struct SatelliteSpec {
    /// Index into `current_level()` of the project this satellite
    /// orbits. The satellite's world position is `parent_center +
    /// ring_offset`, so it follows the parent as the force layout
    /// settles.
    parent_primary: u32,
    /// `DrillTree.nodes` index of the entry-point symbol, or
    /// `u32::MAX` for an overflow badge. Real EP tree_idx ⇒ click
    /// fires `focus_to(fqdn)`. Sentinel ⇒ click is a no-op.
    tree_idx: u32,
    /// Orbit angle (radians) around the parent cube. Combined with
    /// `parent_primary`'s current center each frame so the ring
    /// tracks the parent through force settling.
    angle: f32,
    /// `0` for a real entry-point satellite, `> 0` for the overflow
    /// badge — number of entry-points hidden behind the `+N` glyph.
    overflow_count: u32,
}

/// Active render backend. Mirrored to JS as `"canvas2d"` / `"webgpu"`
/// via [`GraphEngine::mode`] so the host UI can reflect which path
/// the engine is currently driving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderMode {
    Blueprint2D,
    Force3D,
}

/// Called automatically the first time `init()` runs on the JS side.
/// Installs a panic hook that pipes Rust panics into the browser
/// console with full backtraces — without it, panics show up as
/// `unreachable executed` which is useless to debug.
#[wasm_bindgen(start)]
pub fn _wasm_start() {
    console_error_panic_hook::set_once();
}

/// Public engine handle. One instance per `<canvas>` element.
///
/// The boundary with JS is intentionally narrow: every method is a
/// strict `extern "C"`-equivalent through `wasm-bindgen`, taking
/// primitives or `JsValue` and returning the same. The JS host owns
/// the `requestAnimationFrame` loop and calls [`tick`](Self::tick) on
/// every frame; the engine internally short-circuits when nothing has
/// changed since the previous draw.
#[wasm_bindgen]
pub struct GraphEngine {
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    width: f64,
    height: f64,
    device_pixel_ratio: f64,
    scene: Scene,
    viewport: Viewport,
    interaction: InteractionState,
    palette: Palette,
    on_node_click: Option<js_sys::Function>,
    on_node_hover: Option<js_sys::Function>,
    needs_redraw: bool,
    /// Wall-clock duration (microseconds) of the last actual draw
    /// inside `tick()`. Ticks that short-circuited (nothing changed)
    /// don't update this — so the JS profiler sees the cost of *real*
    /// frames, not the trivial pass-through case.
    last_tick_us: u32,
    perf: Option<Performance>,
    /// Active render path. Defaults to Blueprint2D; flips to Force3D
    /// after a successful `enable_webgpu(canvas)` round-trip.
    mode: RenderMode,
    /// Lazily-initialised WebGPU backend. `None` until the host calls
    /// `enable_webgpu` and the async init completes. Holding both
    /// backends warm lets the host toggle between them with a single
    /// `set_mode` call.
    gpu: Option<WebGpuBackend>,
    /// Orbit camera driving the WebGPU 3D path. Independent of the 2D
    /// `viewport`; `frame`d to the scene on `load_graph` and steered by
    /// the pointer handlers while `mode` is `WebGpu`.
    camera: Camera3D,
    /// Per-level force-directed layout for the WebGPU path. Rebuilt
    /// every drill (`load_graph` / descend / ascend); stepped once per
    /// `tick` until it settles.
    force3d: Force3D,
    /// Drill-down hierarchy backing the WebGPU view — built from the
    /// payload, navigated by clicks.
    tree: DrillTree,
    /// Aggregated edges of the currently-focused drill level, cached
    /// so `tick` need not recompute them every settling frame. Each
    /// triple is `(from_level_idx, to_level_idx, weight)` — weight is
    /// the count of underlying symbol→symbol cross-links collapsed
    /// into this aggregate edge (always `>= 1`). Primary-to-primary
    /// only — cross-level edges (to ghost nodes) are computed lazily
    /// in `build_gpu_edges` so they don't pollute the force-spring set.
    level_edges: Vec<(u32, u32, u32)>,
    /// `DrillTree.nodes` indices of the ghost nodes the 3D view
    /// materialises around the primary force cloud — siblings of
    /// the focused node that have cross-level edges into it. Mirrors
    /// the 2D `Scene` ghost cards. Populated in `rebuild_current_level`,
    /// consumed by `build_level_nodes`, `build_gpu_edges`,
    /// `label_layout`, and `pick`.
    ghost_tree_idxs: Vec<u32>,
    /// Phase 3 (Flow) 3.4 — workspace-overview satellites: small
    /// spheres orbiting each project cube to surface its entry-points
    /// (`main` / `luaopen_*` / …) without forcing the user to drill.
    /// Populated only at the root level (`tree.is_root_level()`),
    /// cleared otherwise. Layered behind ghosts in the combined
    /// `build_level_nodes()` output (primaries → ghosts → satellites),
    /// so picking maps the third zone to satellite specs.
    satellites: Vec<SatelliteSpec>,
    /// Screen position of a pending click in `WebGpu` mode — `Some`
    /// between `pointer_down` and `pointer_up` while no drag has
    /// happened, so `pointer_up` can tell a click from an orbit.
    drill_press: Option<(f32, f32)>,
}

#[wasm_bindgen]
impl GraphEngine {
    /// Bind the engine to an existing `<canvas>` element. The canvas
    /// backing buffer is set to `width * device_pixel_ratio` so the
    /// drawing is sharp on HiDPI displays; the CSS size remains the
    /// logical `width × height` you pass in.
    #[wasm_bindgen(constructor)]
    pub fn new(
        canvas: HtmlCanvasElement,
        width: u32,
        height: u32,
        device_pixel_ratio: f64,
    ) -> Result<GraphEngine, JsValue> {
        let ctx = canvas
            .get_context("2d")?
            .ok_or_else(|| JsValue::from_str("CanvasRenderingContext2d unavailable"))?
            .dyn_into::<CanvasRenderingContext2d>()?;

        let dpr = if device_pixel_ratio.is_finite() && device_pixel_ratio > 0.0 {
            device_pixel_ratio
        } else {
            1.0
        };
        apply_canvas_size(&canvas, width, height, dpr);

        let perf = window().and_then(|w| w.performance());

        Ok(Self {
            canvas,
            ctx,
            width: f64::from(width),
            height: f64::from(height),
            device_pixel_ratio: dpr,
            scene: Scene::default(),
            viewport: Viewport::identity(),
            interaction: InteractionState::default(),
            palette: Palette::default(),
            on_node_click: None,
            on_node_hover: None,
            needs_redraw: true,
            last_tick_us: 0,
            perf,
            mode: RenderMode::Blueprint2D,
            gpu: None,
            camera: Camera3D::identity(),
            force3d: Force3D::empty(),
            tree: DrillTree::empty(),
            level_edges: Vec::new(),
            ghost_tree_idxs: Vec::new(),
            satellites: Vec::new(),
            drill_press: None,
        })
    }

    /// Spin up the WebGPU backend against a separate canvas element.
    /// A `<canvas>` can only host one rendering context at a time —
    /// `getContext("2d")` and `getContext("webgpu")` are mutually
    /// exclusive — so the host owns two stacked canvases and hands
    /// the 3D one to this method. Resolves once the GPU adapter +
    /// device are ready; the current scene is auto-uploaded so a
    /// subsequent `set_mode("webgpu")` paints on the next `tick`.
    pub async fn enable_webgpu(&mut self, canvas: HtmlCanvasElement) -> Result<(), JsValue> {
        let backing_w = (self.width * self.device_pixel_ratio)
            .round()
            .clamp(1.0, f64::from(u32::MAX)) as u32;
        let backing_h = (self.height * self.device_pixel_ratio)
            .round()
            .clamp(1.0, f64::from(u32::MAX)) as u32;
        let mut backend = WebGpuBackend::init(canvas, backing_w, backing_h).await?;
        let level = self.build_level_nodes();
        let edges = self.build_gpu_edges();
        backend.upload_scene(&level, &edges, &self.palette);
        backend.upload_view(&self.camera);
        self.gpu = Some(backend);
        self.needs_redraw = true;
        Ok(())
    }

    /// Switch the active render path. Accepts `"canvas2d"` or
    /// `"webgpu"`. Returns an error if the requested mode isn't
    /// initialised yet (call `enable_webgpu` first for `"webgpu"`).
    /// Cheap — no resources are freed; the inactive backend stays
    /// warm so the toggle is instantaneous.
    pub fn set_mode(&mut self, mode: &str) -> Result<(), JsValue> {
        let next = match mode {
            "canvas2d" => RenderMode::Blueprint2D,
            "webgpu" => {
                if self.gpu.is_none() {
                    return Err(JsValue::from_str(
                        "WebGPU backend not initialised — call enable_webgpu(canvas) first.",
                    ));
                }
                RenderMode::Force3D
            }
            other => {
                return Err(JsValue::from_str(&format!(
                    "unknown mode `{other}` (expected `canvas2d` or `webgpu`)",
                )));
            }
        };
        if next != self.mode {
            self.mode = next;
            self.needs_redraw = true;
        }
        Ok(())
    }

    /// Currently active render path, mirrored as a string for the JS
    /// host's UI (toggle buttons read the canonical state).
    pub fn mode(&self) -> String {
        match self.mode {
            RenderMode::Blueprint2D => "canvas2d".into(),
            RenderMode::Force3D => "webgpu".into(),
        }
    }

    /// Animate the orbit camera to a named preset angle for the WebGPU
    /// 3D view — `"orbit"`, `"top"`, `"front"` or `"side"`. Unknown
    /// names are ignored; the transition eases in over the next ~30
    /// `tick`s.
    pub fn set_camera_preset(&mut self, preset: &str) {
        self.camera.apply_preset(preset);
        self.needs_redraw = true;
    }

    /// Ascend one level in the WebGPU drill view — back toward the
    /// projects. No-op at the root.
    pub fn drill_up(&mut self) {
        if self.tree.ascend() {
            self.rebuild_current_level();
            self.needs_redraw = true;
        }
    }

    /// Screen positions of the current drill level's node labels, as
    /// JSON `[{text, x, y, on}]` — `(x, y)` in CSS pixels, `on` false
    /// when the node is behind the camera. The host pins one DOM text
    /// element per entry over the WebGPU canvas; call it every frame
    /// so labels track orbit / dolly / layout settling.
    pub fn label_layout(&self) -> String {
        if !matches!(self.mode, RenderMode::Force3D) {
            return "[]".to_string();
        }
        let w = self.width as f32;
        let h = self.height.max(1.0) as f32;
        let view_proj = self.camera.proj(w / h) * self.camera.view();
        // Only the hovered node's label is emitted with `on = true`.
        // The host pool keeps every DOM element but hides those whose
        // `on` is false, so cluttering the 3D scene with permanent
        // labels (the previous behaviour) is gone.
        let hovered = self.interaction.hovered_tree_idx();
        let project_label_pos = |pos: Vec3, idx: u32| -> LabelPos {
            let clip = view_proj * pos.extend(1.0);
            let visible = clip.w > 0.001;
            let node = self.tree.node(idx);
            // Pin a permanent label on heavy containers so the user
            // can orient without scrubbing every node. Hover always
            // wins regardless of weight.
            let is_anchor = self.tree.is_container(idx)
                && node.descendant_count >= LABEL_VISIBLE_THRESHOLD;
            let on = visible && (Some(idx) == hovered || is_anchor);
            let (x, y) = if on {
                (
                    (clip.x / clip.w * 0.5 + 0.5) * w,
                    (1.0 - (clip.y / clip.w * 0.5 + 0.5)) * h,
                )
            } else {
                (0.0, 0.0)
            };
            LabelPos {
                text: node.label.clone(),
                x,
                y,
                on,
            }
        };
        let mut labels: Vec<LabelPos> = self
            .tree
            .current_level()
            .iter()
            .zip(self.force3d.positions())
            .map(|(&idx, pos)| project_label_pos(*pos, idx))
            .collect();
        // Ghost labels — same hover gating as primaries, so a
        // sibling-of-focus card only surfaces its name when the user
        // points at it.
        for (i, &tree_idx) in self.ghost_tree_idxs.iter().enumerate() {
            labels.push(project_label_pos(self.ghost_position_3d(i), tree_idx));
        }
        // Phase 3 (Flow) 3.4 satellite labels. Real entry-point
        // satellites surface their symbol name on hover only — keeps
        // the workspace overview from drowning in floating text when
        // a big workspace has dozens of `main` / `luaopen_*`. Overflow
        // badges (`+N`) are always-on: there's at most one per cube
        // and the count IS the whole point of the badge.
        for spec in &self.satellites {
            let Some(pos) = self.satellite_position(spec) else {
                continue;
            };
            let clip = view_proj * pos.extend(1.0);
            let visible = clip.w > 0.001;
            let (text, always_on) = if spec.overflow_count > 0 {
                (format!("+{}", spec.overflow_count), true)
            } else {
                (self.tree.node(spec.tree_idx).label.clone(), false)
            };
            let is_hovered = spec.overflow_count == 0 && Some(spec.tree_idx) == hovered;
            let on = visible && (always_on || is_hovered);
            let (x, y) = if on {
                (
                    (clip.x / clip.w * 0.5 + 0.5) * w,
                    (1.0 - (clip.y / clip.w * 0.5 + 0.5)) * h,
                )
            } else {
                (0.0, 0.0)
            };
            labels.push(LabelPos { text, x, y, on });
        }
        serde_json::to_string(&labels).unwrap_or_else(|_| "[]".to_string())
    }

    /// Load (or replace) the graph data the engine renders. The
    /// payload is JSON with the shape produced by the VSCode webview's
    /// `runBrowse` orchestration — modules + symbols + optional edges.
    /// Calling this re-runs the cluster pack layout AND auto-fits the
    /// viewport (user pan/zoom is lost). For edge-only refreshes —
    /// e.g. a lazy hover fetch — call `set_edges` instead.
    pub fn load_graph(&mut self, json: &str) -> Result<(), JsValue> {
        let payload: GraphPayload = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("load_graph parse: {e}")))?;
        // Single source of truth for hierarchy + focus. The 2D scene
        // and the 3D layout are both projections of this tree —
        // `rebuild_current_level` builds them.
        self.tree = DrillTree::build(&payload);
        self.tree.reset_focus();
        self.rebuild_current_level();
        self.fit();
        self.needs_redraw = true;
        Ok(())
    }

    /// Replace just the edge set. Preserves the node layout and the
    /// viewport (current pan/zoom is kept), so this is safe to call
    /// from a hover handler. Edges referencing fqdns not present in
    /// the current node set are dropped silently — same policy as
    /// `load_graph`'s edge resolver.
    pub fn set_edges(&mut self, json: &str) -> Result<(), JsValue> {
        let payload: EdgesPayload = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("set_edges parse: {e}")))?;
        self.scene.replace_edges(payload.edges);
        self.needs_redraw = true;
        Ok(())
    }

    /// Replace the theme palette in one call. JSON shape:
    /// `{ "background": "#1e1e1e", "foreground": "#cccccc", ... }`.
    /// Mismatched keys are ignored; missing keys keep their defaults.
    pub fn set_palette(&mut self, json: &str) -> Result<(), JsValue> {
        let next: Palette = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("set_palette parse: {e}")))?;
        self.palette = next;
        let level = self.build_level_nodes();
        let edges = self.build_gpu_edges();
        if let Some(gpu) = &mut self.gpu {
            gpu.upload_scene(&level, &edges, &self.palette);
        }
        self.needs_redraw = true;
        Ok(())
    }

    /// The fully-resolved theme palette as JSON — every default filled
    /// in. The host reads it to build the colour legend without
    /// duplicating the language / project-kind / edge-kind palettes.
    pub fn palette_json(&self) -> String {
        serde_json::to_string(&self.palette).unwrap_or_else(|_| "{}".to_string())
    }

    /// Render one frame. Caller wires this into
    /// `requestAnimationFrame`; the engine returns early when nothing
    /// has changed since the last paint, so a quiescent loop costs
    /// essentially nothing.
    pub fn tick(&mut self) {
        // While the 3D layout is still settling, advance it once per
        // frame and re-upload — keeps the WebGPU view animating even
        // with no pointer input.
        if matches!(self.mode, RenderMode::Force3D) && !self.force3d.settled() {
            self.force3d.step();
            let level = self.build_level_nodes();
            let edges = self.build_gpu_edges();
            if let Some(gpu) = &mut self.gpu {
                gpu.upload_scene(&level, &edges, &self.palette);
            }
            self.needs_redraw = true;
        }
        // Advance an in-flight camera preset transition.
        if self.camera.animating() {
            self.camera.step_animation();
            self.needs_redraw = true;
        }
        if !self.needs_redraw {
            return;
        }
        let start_ms = self.perf.as_ref().map(|p| p.now()).unwrap_or(0.0);
        self.dispatch_draw();
        if let Some(perf) = &self.perf {
            // `performance.now()` is sub-ms on modern browsers; we
            // store the result in integer microseconds to keep the
            // wasm-bindgen boundary one cheap u32 instead of a f64.
            let elapsed_us = (perf.now() - start_ms) * 1000.0;
            self.last_tick_us = elapsed_us.clamp(0.0, f64::from(u32::MAX)) as u32;
        }
        self.needs_redraw = false;
    }

    fn dispatch_draw(&mut self) {
        match self.mode {
            RenderMode::Blueprint2D => render::draw(
                &self.ctx,
                self.width,
                self.height,
                self.device_pixel_ratio,
                &self.scene,
                &self.viewport,
                &self.interaction,
                &self.palette,
            ),
            RenderMode::Force3D => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_view(&self.camera);
                    if let Err(e) = gpu.render() {
                        web_sys::console::error_1(&e);
                    }
                }
            }
        }
    }

    /// Microseconds spent inside the most recent non-trivial `tick()`.
    /// Zero before the first frame has rendered or when no
    /// `performance.now()` is available. The JS profiler polls this
    /// every ~250 ms to surface the in-engine draw cost separately
    /// from JS-side rAF overhead.
    pub fn last_tick_us(&self) -> u32 {
        self.last_tick_us
    }

    /// Number of resolved edges currently held by the scene. Used by
    /// the profiler overlay (and any future inspector) to display the
    /// graph weight without forcing the JS host to maintain its own
    /// counter.
    pub fn edge_count(&self) -> usize {
        self.scene.edge_count()
    }

    /// Logical (CSS) size change. The internal backing buffer is
    /// reallocated to `width * dpr × height * dpr`.
    pub fn resize(&mut self, width: u32, height: u32) {
        apply_canvas_size(&self.canvas, width, height, self.device_pixel_ratio);
        self.width = f64::from(width);
        self.height = f64::from(height);
        if let Some(gpu) = &mut self.gpu {
            let backing_w = (f64::from(width) * self.device_pixel_ratio)
                .round()
                .clamp(1.0, f64::from(u32::MAX)) as u32;
            let backing_h = (f64::from(height) * self.device_pixel_ratio)
                .round()
                .clamp(1.0, f64::from(u32::MAX)) as u32;
            gpu.resize(backing_w, backing_h);
        }
        self.fit_keep_zoom();
        self.needs_redraw = true;
    }

    /// Update the device pixel ratio (called on the rare `devicePixelRatio` change event).
    pub fn set_device_pixel_ratio(&mut self, dpr: f64) {
        let safe = if dpr.is_finite() && dpr > 0.0 {
            dpr
        } else {
            1.0
        };
        self.device_pixel_ratio = safe;
        apply_canvas_size(&self.canvas, self.width as u32, self.height as u32, safe);
        self.needs_redraw = true;
    }

    /// Recenter and scale so the entire scene fits in the viewport.
    pub fn fit(&mut self) {
        self.viewport
            .fit_to(self.scene.bounds(), self.width, self.height);
        self.needs_redraw = true;
    }

    /// Reset the zoom back to 1× without changing the pan offset.
    pub fn reset_zoom(&mut self) {
        self.viewport.set_scale(1.0);
        self.needs_redraw = true;
    }

    /// JS hands us pointer events. Coordinates are in CSS pixels
    /// relative to the canvas's bounding rect (NOT clientX/Y).
    pub fn on_pointer_move(&mut self, x: f32, y: f32) {
        let sx = f64::from(x);
        let sy = f64::from(y);
        if let Some((origin_x, origin_y)) = self.interaction.pan_origin() {
            let dx = sx - origin_x;
            let dy = sy - origin_y;
            match self.mode {
                RenderMode::Force3D => {
                    self.camera.orbit(dx, dy);
                    // A drag is an orbit, not a click — drop the pick.
                    self.drill_press = None;
                }
                RenderMode::Blueprint2D => self.viewport.pan(dx, dy),
            }
            self.interaction.set_pan_origin(Some((sx, sy)));
            self.needs_redraw = true;
            return;
        }
        // 3D path: pick a level node in screen space, then set both
        // the leaf-fqdn hover (drives the JS callback + detail panel)
        // AND the universal tree-idx hover (drives the label layer's
        // hover-only filter, which covers containers AND ghosts).
        if matches!(self.mode, RenderMode::Force3D) {
            let primary_count = self.tree.current_level().len() as u32;
            let (next_tree_idx, next_fqdn) = match self.pick(x, y) {
                Some(combined_idx) => {
                    // `pick` returns a combined index — primaries
                    // first then ghosts. Resolve to the underlying
                    // `DrillTree.nodes` index without panicking on
                    // ghost hits (the old `current_level()[idx]`
                    // would have OOB'd and stuck the wasm borrow).
                    let tree_idx = if combined_idx < primary_count {
                        self.tree.current_level()[combined_idx as usize]
                    } else {
                        let ghost_i = (combined_idx - primary_count) as usize;
                        match self.ghost_tree_idxs.get(ghost_i).copied() {
                            Some(t) => t,
                            None => return,
                        }
                    };
                    let fqdn = self.tree.node(tree_idx).fqdn.clone();
                    let fqdn_opt = if fqdn.is_empty() { None } else { Some(fqdn) };
                    (Some(tree_idx), fqdn_opt)
                }
                None => (None, None),
            };
            let changed = next_tree_idx != self.interaction.hovered_tree_idx()
                || next_fqdn != self.interaction.hovered();
            if changed {
                self.interaction.set_hovered_tree_idx(next_tree_idx);
                self.interaction.set_hovered(next_fqdn.clone());
                self.needs_redraw = true;
                if let (Some(cb), Some(fqdn)) = (&self.on_node_hover, next_fqdn) {
                    defer_callback(cb.clone(), Some(fqdn));
                }
            }
            return;
        }
        // A pointer over the minimap panel must not hover-select the
        // chips painted behind it.
        let over_minimap = render::minimap_world_target(
            self.width,
            self.height,
            self.scene.bounds(),
            sx,
            sy,
        )
        .is_some();
        let (wx, wy) = self.viewport.screen_to_world(sx, sy);
        // hit_test now returns a card index into the current-level
        // `scene.cards`. Convert to an fqdn for the hover state, but
        // only for leaf cards — container cards carry an empty fqdn
        // and don't fire the hover callback.
        let hit_idx = if over_minimap {
            None
        } else {
            self.scene.hit_test(wx, wy)
        };
        let hit_fqdn = hit_idx.and_then(|i| {
            let c = &self.scene.cards[i];
            if c.fqdn.is_empty() {
                None
            } else {
                Some(c.fqdn.clone())
            }
        });
        if hit_fqdn != self.interaction.hovered() {
            self.interaction.set_hovered(hit_fqdn.clone());
            self.needs_redraw = true;
            if let (Some(cb), Some(fqdn)) = (&self.on_node_hover, hit_fqdn) {
                defer_callback(cb.clone(), Some(fqdn));
            }
        }
    }

    pub fn on_pointer_down(&mut self, x: f32, y: f32, button: u8) {
        if button != 0 {
            return;
        }
        let sx = f64::from(x);
        let sy = f64::from(y);
        // WebGPU path: a still press is a drill click, a press+drag is
        // a camera orbit — `pointer_up` decides which from `drill_press`.
        if matches!(self.mode, RenderMode::Force3D) {
            self.interaction.set_pan_origin(Some((sx, sy)));
            self.drill_press = Some((x, y));
            return;
        }
        // A click inside the minimap teleports the viewport (recenter,
        // keep zoom) instead of starting a pan or selecting a chip.
        if let Some((wx, wy)) = render::minimap_world_target(
            self.width,
            self.height,
            self.scene.bounds(),
            sx,
            sy,
        ) {
            self.viewport
                .center_world(wx, wy, self.width, self.height);
            self.needs_redraw = true;
            return;
        }
        let (wx, wy) = self.viewport.screen_to_world(sx, sy);
        if let Some(i) = self.scene.hit_test(wx, wy) {
            // Stash the stable tree-node index; `on_pointer_up`
            // resolves it back to a card to decide drill vs callback.
            let tree_idx = self.scene.cards[i].tree_idx;
            self.interaction
                .set_click_candidate(Some((tree_idx, sx, sy)));
        } else {
            self.interaction.set_pan_origin(Some((sx, sy)));
        }
    }

    pub fn on_pointer_up(&mut self, x: f32, y: f32, _button: u8) {
        if matches!(self.mode, RenderMode::Force3D) {
            // `drill_press` survives only when no orbit drag happened
            // between down and up — i.e. this was a click.
            if self.drill_press.take().is_some() {
                self.drill_pick(x, y);
            }
            self.interaction.set_pan_origin(None);
            return;
        }
        if let Some((tree_idx, dx, dy)) = self.interaction.take_click_candidate() {
            let moved = (f64::from(x) - dx).hypot(f64::from(y) - dy);
            if moved < 5.0 {
                // Ghost card ⇒ refocus on the sibling it represents
                // (a one-shot drill-out-and-into-sibling). Regular
                // container ⇒ drill descend (parity with the 3D
                // click-to-drill path). Leaf ⇒ fire the click
                // callback with the card's fqdn.
                let is_ghost = self
                    .scene
                    .card_by_tree_idx
                    .get(&tree_idx)
                    .map(|&i| self.scene.cards[i].is_ghost)
                    .unwrap_or(false);
                if is_ghost {
                    if self.tree.focus_to(tree_idx) {
                        self.rebuild_current_level();
                        self.needs_redraw = true;
                    }
                } else if self.tree.is_container(tree_idx) {
                    if self.tree.descend(tree_idx) {
                        self.rebuild_current_level();
                        self.needs_redraw = true;
                    }
                } else {
                    let fqdn = self.tree.node(tree_idx).fqdn.clone();
                    if !fqdn.is_empty() {
                        if let Some(cb) = &self.on_node_click {
                            defer_callback(cb.clone(), Some(fqdn));
                        }
                    }
                }
            }
        }
        self.interaction.set_pan_origin(None);
    }

    /// JS double-click → drill UP gesture. In 2D Blueprint mode a
    /// double-click on the empty space between cards ascends one
    /// level (descend is already handled by single-click on a
    /// container, so dblclick on a card would otherwise produce a
    /// double-descend after the layout rebuilds). The 3D path is
    /// inert here — it has its own up affordance.
    pub fn on_double_click(&mut self, x: f32, y: f32) {
        if matches!(self.mode, RenderMode::Force3D) {
            return;
        }
        let (wx, wy) = self
            .viewport
            .screen_to_world(f64::from(x), f64::from(y));
        if self.scene.hit_test(wx, wy).is_some() {
            return;
        }
        if self.tree.ascend() {
            self.rebuild_current_level();
            self.needs_redraw = true;
        }
    }

    /// Breadcrumb trail for the current drill focus as a JSON array
    /// `[{label, id}]`, root → focus. The host renders it as a
    /// clickable breadcrumb; each `id` is a `DrillTree.nodes` index
    /// that round-trips through `fit_to_frame` to refocus that node.
    /// Empty array at the root level (no project descended into).
    pub fn focus_path(&self) -> String {
        let crumbs: Vec<FocusCrumb> = self
            .tree
            .breadcrumb()
            .into_iter()
            .map(|(label, id)| FocusCrumb { label, id })
            .collect();
        serde_json::to_string(&crumbs).unwrap_or_else(|_| "[]".to_string())
    }

    /// Refocus the drill view on the tree node `id` (a breadcrumb
    /// crumb's id). The 2D scene + 3D layout rebuild around it. The
    /// method name is kept for backwards compatibility with the JS
    /// host; semantically it drives drill navigation now, not a 2D
    /// viewport fit.
    pub fn fit_to_frame(&mut self, id: u32) {
        if self.tree.focus_to(id) {
            self.rebuild_current_level();
            self.needs_redraw = true;
        }
    }

    /// Reset the drill focus to the root level (projects). The host's
    /// "workspace" breadcrumb crumb feeds this. No-op when the focus
    /// already sits at the root.
    pub fn reset_focus(&mut self) {
        if self.tree.reset_focus() {
            self.rebuild_current_level();
            self.needs_redraw = true;
        }
    }

    pub fn on_pointer_leave(&mut self) {
        let had_hover = self.interaction.hovered().is_some()
            || self.interaction.hovered_tree_idx().is_some();
        if had_hover {
            self.interaction.set_hovered(None);
            self.interaction.set_hovered_tree_idx(None);
            self.needs_redraw = true;
        }
        self.interaction.set_pan_origin(None);
    }

    pub fn on_wheel(&mut self, x: f32, y: f32, delta_y: f32) {
        let factor = if delta_y < 0.0 { 1.15 } else { 1.0 / 1.15 };
        match self.mode {
            RenderMode::Force3D => self.camera.dolly(factor as f32),
            RenderMode::Blueprint2D => {
                self.viewport
                    .zoom_around(f64::from(x), f64::from(y), factor);
            }
        }
        self.needs_redraw = true;
    }

    /// Register a callback that receives the FQDN string when the
    /// user clicks (not drags) a node. Pass `null` from JS to clear.
    pub fn set_on_node_click(&mut self, cb: js_sys::Function) {
        self.on_node_click = Some(cb);
    }

    pub fn set_on_node_hover(&mut self, cb: js_sys::Function) {
        self.on_node_hover = Some(cb);
    }

    /// Number of symbols currently loaded (cheap, used by the UI for
    /// the "N sym" readout in the toolbar).
    pub fn symbol_count(&self) -> usize {
        self.scene.symbol_count()
    }

    /// `true` when an initialised WebGPU backend is attached AND the
    /// engine is currently dispatching to it. The HUD uses this to
    /// decide whether to surface the GPU stat row at all.
    pub fn gpu_active(&self) -> bool {
        matches!(self.mode, RenderMode::Force3D) && self.gpu.is_some()
    }

    /// Number of instances rendered in the last WebGPU pass. Zero when
    /// the GPU backend isn't initialised or when the engine is on the
    /// Canvas2D path. Cheap — returns a cached counter, no traversal.
    pub fn gpu_instance_count(&self) -> u32 {
        self.gpu.as_ref().map_or(0, |g| g.instance_count())
    }

    /// Capacity (in instances) of the GPU instance buffer. The buffer
    /// grows monotonically by powers of two; the gap with
    /// `gpu_instance_count` is the head-room before the next
    /// reallocation. Zero when the GPU backend isn't initialised.
    pub fn gpu_instance_capacity(&self) -> u32 {
        self.gpu.as_ref().map_or(0, |g| g.instance_capacity())
    }

    /// Force a redraw on the next `tick()`. Useful when the host
    /// changes external state (palette via setter already triggers
    /// this, but e.g. a parent CSS theme toggle does not).
    pub fn invalidate(&mut self) {
        self.needs_redraw = true;
    }

    fn fit_keep_zoom(&mut self) {
        self.viewport
            .center_on(self.scene.bounds(), self.width, self.height);
    }

    /// Rebuild every per-level cache: the 2D card scene, the cached
    /// aggregated edges, the 3D force layout + ghost ring, and the
    /// orbit camera framing. Called whenever the drill focus moves
    /// (`load_graph`, `descend`, `ascend`, `focus_to`) AND after a
    /// `set_mode` flip — both backends are kept warm so toggling
    /// between them is instantaneous. The GPU backend is pushed too
    /// when present.
    fn rebuild_current_level(&mut self) {
        // Primary-to-primary level edges — consumed by the 3D force
        // springs AND by the 2D scene. Cross-level (primary→ghost)
        // edges are computed lazily in `build_gpu_edges` so they
        // don't pollute the spring set.
        self.level_edges = self.tree.level_edges();
        // 3D ghost set — sibling-of-focus tree indices the focused
        // subtree couples to. Deduplicated, order = first-seen.
        let cross = self.tree.cross_edges();
        let mut ghosts: Vec<u32> = Vec::new();
        let mut ghost_seen: HashMap<u32, ()> = HashMap::new();
        for &(_, sibling) in &cross {
            if ghost_seen.insert(sibling, ()).is_none() {
                ghosts.push(sibling);
            }
        }
        self.ghost_tree_idxs = ghosts;
        // Phase 3 (Flow) 3.4 — workspace-overview satellites. Only at
        // root level; deeper levels keep their primary+ghost layout
        // intact. Each project gets up to `SATELLITE_CAP` entry-point
        // satellites placed evenly around it; the cap+1 slot is the
        // overflow badge when there are more entry-points than the
        // cap allows. Positions are recomputed each frame from the
        // parent's settling center, so the ring tracks the layout.
        self.satellites = Vec::new();
        if self.tree.is_root_level() {
            for (primary_idx, &tree_idx) in self.tree.current_level().iter().enumerate() {
                let node = self.tree.node(tree_idx);
                // Only projects orbit satellites — orphan symbols at
                // root (no `project_id` resolved) have no children
                // to surface as entry-points.
                if !node.fqdn.is_empty() {
                    continue;
                }
                let eps = self.tree.entry_points_for_project(tree_idx);
                if eps.is_empty() {
                    continue;
                }
                let shown = eps.len().min(SATELLITE_CAP);
                let overflow = eps.len().saturating_sub(shown);
                let total_slots = shown + usize::from(overflow > 0);
                for (k, &ep_idx) in eps.iter().take(shown).enumerate() {
                    self.satellites.push(SatelliteSpec {
                        parent_primary: primary_idx as u32,
                        tree_idx: ep_idx,
                        angle: std::f32::consts::TAU * (k as f32) / total_slots as f32,
                        overflow_count: 0,
                    });
                }
                if overflow > 0 {
                    self.satellites.push(SatelliteSpec {
                        parent_primary: primary_idx as u32,
                        tree_idx: u32::MAX,
                        angle: std::f32::consts::TAU * (shown as f32) / total_slots as f32,
                        overflow_count: overflow as u32,
                    });
                }
            }
        }
        // 2D scene — cards + level edges + bounds + label truncation.
        // Scene independently materialises its own 2D ghost cards
        // via `tree.cross_edges()`; the 3D ghost list above is for
        // the 3D ring placement only.
        self.scene = Scene::from_level(&self.tree, &self.ctx);
        // 3D primary layout — seeded on a sphere, settles over the
        // next ticks. Weight is dropped here — force springs treat
        // every edge equally; the weight only modulates the renderer's
        // alpha so the visual hierarchy doesn't drag the layout.
        let n = self.tree.current_level().len();
        let spring_edges: Vec<(u32, u32)> =
            self.level_edges.iter().map(|&(a, b, _)| (a, b)).collect();
        self.force3d = Force3D::for_level(n, spring_edges);
        // Camera frames the union of primary cloud + ghost ring so
        // the user sees the full context (including off-focus
        // dependencies) at once.
        let (center, radius) = self.force3d.bounding_sphere();
        let frame_radius = if self.ghost_tree_idxs.is_empty() {
            radius
        } else {
            ghost_ring_radius(radius) + radius * 0.3
        };
        self.camera.frame(center, frame_radius);
        // Push to GPU — primaries + ghosts + combined edges.
        if self.gpu.is_some() {
            let nodes = self.build_level_nodes();
            let edges = self.build_gpu_edges();
            if let Some(gpu) = &mut self.gpu {
                gpu.upload_scene(&nodes, &edges, &self.palette);
            }
        }
    }

    /// Snapshot the focused level's children + the ghost ring as
    /// renderable nodes. Primaries come first (indices 0..n_primary,
    /// matching `tree.current_level()`), ghosts next (indices
    /// `n_primary..` matching `self.ghost_tree_idxs`). The ghost
    /// positions are deterministic — a planar ring around the
    /// primary cloud's bounding sphere.
    fn build_level_nodes(&self) -> Vec<LevelNode> {
        let mut nodes: Vec<LevelNode> = self
            .tree
            .current_level()
            .iter()
            .zip(self.force3d.positions())
            .map(|(&idx, pos)| {
                let node = self.tree.node(idx);
                LevelNode {
                    center: *pos,
                    size: level_node_size(node.descendant_count),
                    language: node.language.clone(),
                    kind: node.kind,
                    is_project: node.fqdn.is_empty(),
                    is_container: self.tree.is_container(idx),
                    is_ghost: false,
                    entry_point: node.entry_point.clone(),
                }
            })
            .collect();
        for (i, &tree_idx) in self.ghost_tree_idxs.iter().enumerate() {
            let node = self.tree.node(tree_idx);
            nodes.push(LevelNode {
                center: self.ghost_position_3d(i),
                size: level_node_size(node.descendant_count),
                language: node.language.clone(),
                kind: node.kind,
                is_project: node.fqdn.is_empty(),
                is_container: self.tree.is_container(tree_idx),
                is_ghost: true,
                entry_point: node.entry_point.clone(),
            });
        }
        // Phase 3 (Flow) 3.4 satellite layer — entry-point spheres
        // orbiting each project cube. Read parent positions from the
        // already-built primary slice so the ring tracks the parent
        // through force-layout settling.
        for spec in &self.satellites {
            let parent = match nodes.get(spec.parent_primary as usize) {
                Some(p) => p.clone(),
                None => continue,
            };
            let pos = satellite_position_3d(parent.center, parent.size, spec.angle);
            let size = satellite_size_3d(parent.size);
            if spec.overflow_count > 0 {
                // Overflow badge — grey leaf, no halo. The `+N` text
                // is surfaced via `label_layout`.
                nodes.push(LevelNode {
                    center: pos,
                    size,
                    language: parent.language.clone(),
                    kind: kind::Kind::Unknown,
                    is_project: false,
                    is_container: false,
                    is_ghost: false,
                    entry_point: None,
                });
            } else {
                let ep = self.tree.node(spec.tree_idx);
                nodes.push(LevelNode {
                    center: pos,
                    size,
                    language: ep.language.clone(),
                    kind: ep.kind,
                    is_project: false,
                    is_container: false,
                    is_ghost: false,
                    entry_point: ep.entry_point.clone(),
                });
            }
        }
        nodes
    }

    /// Deterministic 3D position for a ghost node — a planar ring at
    /// `y = center.y` orbiting the primary cloud's bounding sphere.
    /// Same formula consumed by `build_level_nodes`, `label_layout`,
    /// and `pick` so the three views stay in lockstep without
    /// caching positions in `self`.
    fn ghost_position_3d(&self, ghost_index: usize) -> Vec3 {
        let count = self.ghost_tree_idxs.len().max(1) as f32;
        let theta = std::f32::consts::TAU * (ghost_index as f32) / count;
        let (center, radius) = self.force3d.bounding_sphere();
        let ring = ghost_ring_radius(radius);
        Vec3::new(
            center.x + ring * theta.cos(),
            center.y,
            center.z + ring * theta.sin(),
        )
    }

    /// Resolve a satellite's parent position the way the renderer
    /// sees it — read the primary slice from `build_level_nodes`
    /// inline rather than re-running `force3d.positions()`, so a
    /// satellite shifted mid-frame by force settling tracks its
    /// parent. Used by `pick` and `label_layout` to compute satellite
    /// screen positions without duplicating the ring-offset math.
    fn satellite_position(&self, spec: &SatelliteSpec) -> Option<Vec3> {
        let parent_pos = self.force3d.positions().get(spec.parent_primary as usize).copied()?;
        let parent_node = self
            .tree
            .current_level()
            .get(spec.parent_primary as usize)
            .map(|&t| self.tree.node(t))?;
        let parent_size = level_node_size(parent_node.descendant_count);
        Some(satellite_position_3d(parent_pos, parent_size, spec.angle))
    }

    /// Combined edges for GPU upload — primary-to-primary
    /// (`self.level_edges`, carrying their aggregated weight) plus
    /// primary-to-ghost (remapped from `tree.cross_edges()` to
    /// indices in `build_level_nodes`'s output: ghosts occupy
    /// `n_primary..`). Ghost edges carry weight 1 — they are
    /// per-link by construction, not aggregated.
    fn build_gpu_edges(&self) -> Vec<(u32, u32, u32)> {
        let primary_count = self.tree.current_level().len() as u32;
        if self.ghost_tree_idxs.is_empty() {
            return self.level_edges.clone();
        }
        let primary_lookup: HashMap<u32, u32> = self
            .tree
            .current_level()
            .iter()
            .enumerate()
            .map(|(i, &t)| (t, i as u32))
            .collect();
        let ghost_lookup: HashMap<u32, u32> = self
            .ghost_tree_idxs
            .iter()
            .enumerate()
            .map(|(i, &t)| (t, primary_count + i as u32))
            .collect();
        let mut out = self.level_edges.clone();
        for (inside, sibling) in self.tree.cross_edges() {
            if let (Some(&i), Some(&g)) =
                (primary_lookup.get(&inside), ghost_lookup.get(&sibling))
            {
                out.push((i, g, 1));
            }
        }
        out
    }

    /// Screen-space pick over primary nodes, the ghost ring AND the
    /// Phase 3.4 satellite layer. The returned index is into the
    /// combined `build_level_nodes()` order:
    /// `primaries (0..primary_count)`, then `ghosts
    /// (primary_count..primary_count+ghost_count)`, then `satellites
    /// (primary_count+ghost_count..)`. `None` for a click on the void.
    fn pick(&self, sx: f32, sy: f32) -> Option<u32> {
        let primaries = self.force3d.positions();
        let primary_count = primaries.len();
        let ghost_count = self.ghost_tree_idxs.len();
        let w = self.width as f32;
        let h = self.height.max(1.0) as f32;
        let view_proj = self.camera.proj(w / h) * self.camera.view();
        let mut best_idx: Option<u32> = None;
        let mut best_d = 90.0_f32;
        let consider = |idx: u32, pos: Vec3, best_idx: &mut Option<u32>, best_d: &mut f32| {
            let clip = view_proj * pos.extend(1.0);
            if clip.w <= 0.0 {
                return;
            }
            let px = (clip.x / clip.w * 0.5 + 0.5) * w;
            let py = (1.0 - (clip.y / clip.w * 0.5 + 0.5)) * h;
            let d = (px - sx).hypot(py - sy);
            if d < *best_d {
                *best_d = d;
                *best_idx = Some(idx);
            }
        };
        for (i, pos) in primaries.iter().enumerate() {
            consider(i as u32, *pos, &mut best_idx, &mut best_d);
        }
        for (i, _) in self.ghost_tree_idxs.iter().enumerate() {
            let combined = (primary_count + i) as u32;
            consider(combined, self.ghost_position_3d(i), &mut best_idx, &mut best_d);
        }
        for (i, spec) in self.satellites.iter().enumerate() {
            let Some(pos) = self.satellite_position(spec) else {
                continue;
            };
            let combined = (primary_count + ghost_count + i) as u32;
            consider(combined, pos, &mut best_idx, &mut best_d);
        }
        best_idx
    }

    /// Resolve a click in `Force3D` mode. Primary container ⇒ drill
    /// descend, primary leaf ⇒ fire the node-click callback. Ghost
    /// (sibling-of-focus context node, drawn in the ring around the
    /// primary cloud) ⇒ refocus the drill on that sibling. Satellite
    /// (Phase 3.4 entry-point sphere orbiting a project cube) ⇒
    /// focus_to the entry-point symbol so the user lands inside the
    /// program right at the natural starting point. Overflow badge
    /// (`tree_idx == u32::MAX`) ⇒ no-op (it's a count glyph, not a
    /// link).
    fn drill_pick(&mut self, sx: f32, sy: f32) {
        let Some(combined_idx) = self.pick(sx, sy) else {
            return;
        };
        let primary_count = self.tree.current_level().len() as u32;
        let ghost_count = self.ghost_tree_idxs.len() as u32;
        if combined_idx >= primary_count + ghost_count {
            let sat_idx = (combined_idx - primary_count - ghost_count) as usize;
            let Some(spec) = self.satellites.get(sat_idx).cloned() else {
                return;
            };
            if spec.overflow_count > 0 {
                return; // overflow badge — not clickable
            }
            if self.tree.focus_to(spec.tree_idx) {
                self.rebuild_current_level();
                self.needs_redraw = true;
            }
            return;
        }
        if combined_idx >= primary_count {
            let ghost_idx = (combined_idx - primary_count) as usize;
            let Some(&tree_idx) = self.ghost_tree_idxs.get(ghost_idx) else {
                return;
            };
            if self.tree.focus_to(tree_idx) {
                self.rebuild_current_level();
                self.needs_redraw = true;
            }
            return;
        }
        let tree_idx = self.tree.current_level()[combined_idx as usize];
        if self.tree.is_container(tree_idx) {
            if self.tree.descend(tree_idx) {
                self.rebuild_current_level();
                self.needs_redraw = true;
            }
        } else if let Some(cb) = &self.on_node_click {
            let fqdn = self.tree.node(tree_idx).fqdn.clone();
            if !fqdn.is_empty() {
                defer_callback(cb.clone(), Some(fqdn));
            }
        }
    }
}

/// Billboard size for a drill node — gently scaled by subtree weight
/// so a project full of symbols reads larger than a lone leaf. The
/// quad is square so the impostor shapes (sphere / cube SDFs in
/// `chip.wgsl` operate on a uniform `[-1, 1]²` parameter space)
/// render as round circles / square boxes rather than the previous
/// flattened ovals.
fn level_node_size(descendant_count: u32) -> [f32; 2] {
    let s = 60.0 + (1.0 + descendant_count as f32).ln() * 26.0;
    [s, s]
}

/// World-space radius of the ghost ring around a primary cloud of
/// `primary_radius`. Same factor + margin combination used by
/// `build_level_nodes`, `label_layout`, and `pick` so the three
/// views agree on where ghosts sit.
fn ghost_ring_radius(primary_radius: f32) -> f32 {
    primary_radius * GHOST_RING_RADIUS_FACTOR + GHOST_RING_MARGIN
}

/// Phase 3 (Flow) 3.4 — world-space size of a satellite sphere.
/// A fraction of the parent cube's apparent size so the satellite
/// reads as "an exit point of this thing" rather than a peer.
fn satellite_size_3d(parent_size: [f32; 2]) -> [f32; 2] {
    let s = parent_size[0].max(parent_size[1]) * 0.28;
    [s, s]
}

/// Phase 3 (Flow) 3.4 — world-space position of a satellite at
/// `angle` radians around `parent_center`. Orbits in the XZ plane
/// (Y kept at the parent's altitude) so the satellite ring stays
/// horizontal relative to the camera's up-vector, matching the
/// ghost-ring orientation. Radius = the parent cube's apparent
/// half-extent + a small clearance margin so the satellite sits
/// outside the cube silhouette without being thrown into the void.
fn satellite_position_3d(parent_center: Vec3, parent_size: [f32; 2], angle: f32) -> Vec3 {
    let half = parent_size[0].max(parent_size[1]) * 0.5;
    let radius = half + 18.0;
    Vec3::new(
        parent_center.x + radius * angle.cos(),
        parent_center.y,
        parent_center.z + radius * angle.sin(),
    )
}

/// Resize the canvas. The bitmap (backing store) is set to
/// `width * dpr × height * dpr` for sharp HiDPI rendering; the CSS
/// (intrinsic) size is pinned to the logical `width × height` so the
/// canvas does NOT advertise its DPR-scaled backing size as its
/// layout size. Without that pin, Chrome treats the bitmap size as
/// the intrinsic size and grows the parent flex container, which the
/// `ResizeObserver` immediately observes — yielding a runaway loop
/// where every resize tick makes the canvas (and its scroll overflow)
/// a few pixels bigger. Firefox is more lenient and didn't surface
/// the bug, which is how it stayed hidden until now.
fn apply_canvas_size(canvas: &HtmlCanvasElement, width: u32, height: u32, dpr: f64) {
    canvas.set_width((f64::from(width) * dpr) as u32);
    canvas.set_height((f64::from(height) * dpr) as u32);
    let style = canvas.style();
    let _ = style.set_property("width", &format!("{width}px"));
    let _ = style.set_property("height", &format!("{height}px"));
}

/// Fire a JS callback `cb(fqdn_or_null)` on the next microtask, never
/// inline. wasm-bindgen tracks `&mut self` borrows across the JS
/// boundary; if the callback re-enters the engine while the current
/// method still holds the mutable borrow (e.g. a hover fires while
/// we are inside `on_pointer_move`, and the JS handler calls
/// `set_edges`) wasm-bindgen detects the recursive borrow and panics
/// with "recursive use of an object". Deferring via `spawn_local`
/// lets the current method finish (releasing the borrow) before the
/// callback runs, so re-entrant `&mut self` calls are safe by
/// construction.
fn defer_callback(cb: js_sys::Function, fqdn: Option<String>) {
    wasm_bindgen_futures::spawn_local(async move {
        let arg = match fqdn {
            Some(s) => JsValue::from_str(&s),
            None => JsValue::NULL,
        };
        let _ = cb.call1(&JsValue::NULL, &arg);
    });
}
