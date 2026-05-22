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

mod gpu;
mod hierarchy;
mod interaction;
mod kind;
mod layout;
mod palette;
mod payload;
mod render;
mod scene;
mod viewport;

/// One breadcrumb crumb — a frame's display label and its hierarchy
/// arena index. Serialised into the `focus_path` JSON the host renders
/// as a clickable breadcrumb; `id` round-trips back through
/// `fit_to_frame`.
#[derive(serde::Serialize)]
struct FocusCrumb {
    label: String,
    id: u32,
}

use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, Performance, window};

use crate::gpu::WebGpuBackend;
use crate::interaction::InteractionState;
use crate::palette::Palette;
use crate::payload::{EdgesPayload, GraphPayload};
use crate::scene::Scene;
use crate::viewport::Viewport;

/// Active render backend. Mirrored to JS as `"canvas2d"` / `"webgpu"`
/// via [`GraphEngine::mode`] so the host UI can reflect which path
/// the engine is currently driving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendMode {
    Canvas2D,
    WebGpu,
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
    /// Active render path. Defaults to Canvas2D; flips to WebGpu
    /// after a successful `enable_webgpu(canvas)` round-trip.
    mode: BackendMode,
    /// Lazily-initialised WebGPU backend. `None` until the host calls
    /// `enable_webgpu` and the async init completes. Holding both
    /// backends warm lets the host toggle between them with a single
    /// `set_mode` call.
    gpu: Option<WebGpuBackend>,
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
            mode: BackendMode::Canvas2D,
            gpu: None,
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
        backend.upload_scene(&self.scene, &self.palette);
        backend.upload_view(
            &self.viewport,
            self.width * self.device_pixel_ratio,
            self.height * self.device_pixel_ratio,
        );
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
            "canvas2d" => BackendMode::Canvas2D,
            "webgpu" => {
                if self.gpu.is_none() {
                    return Err(JsValue::from_str(
                        "WebGPU backend not initialised — call enable_webgpu(canvas) first.",
                    ));
                }
                BackendMode::WebGpu
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
            BackendMode::Canvas2D => "canvas2d".into(),
            BackendMode::WebGpu => "webgpu".into(),
        }
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
        let mut scene = Scene::from_payload(payload);
        // One-shot truncation pass. Without this the render loop would
        // call `measure_text` once per chip, per frame — the dominant
        // cost we measured at ~1 k symbols.
        scene.prepare_labels(&self.ctx);
        self.scene = scene;
        if let Some(gpu) = &mut self.gpu {
            gpu.upload_scene(&self.scene, &self.palette);
        }
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
        if let Some(gpu) = &mut self.gpu {
            gpu.upload_scene(&self.scene, &self.palette);
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
            BackendMode::Canvas2D => render::draw(
                &self.ctx,
                self.width,
                self.height,
                self.device_pixel_ratio,
                &self.scene,
                &self.viewport,
                &self.interaction,
                &self.palette,
            ),
            BackendMode::WebGpu => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_view(
                        &self.viewport,
                        self.width * self.device_pixel_ratio,
                        self.height * self.device_pixel_ratio,
                    );
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
            self.viewport.pan(dx, dy);
            self.interaction.set_pan_origin(Some((sx, sy)));
            self.needs_redraw = true;
            return;
        }
        // A pointer over the minimap panel must not hover-select the
        // chips painted behind it.
        let over_minimap = crate::render::minimap_world_target(
            self.width,
            self.height,
            self.scene.bounds(),
            sx,
            sy,
        )
        .is_some();
        let (wx, wy) = self.viewport.screen_to_world(sx, sy);
        let hit = if over_minimap {
            None
        } else {
            self.scene.hit_test(wx, wy)
        };
        if hit != self.interaction.hovered() {
            self.interaction.set_hovered(hit.clone());
            self.needs_redraw = true;
            if let (Some(cb), Some(fqdn)) = (&self.on_node_hover, hit) {
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
        // A click inside the minimap teleports the viewport (recenter,
        // keep zoom) instead of starting a pan or selecting a chip.
        if let Some((wx, wy)) = crate::render::minimap_world_target(
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
        if let Some(fqdn) = self.scene.hit_test(wx, wy) {
            self.interaction.set_click_candidate(Some((fqdn, sx, sy)));
        } else {
            self.interaction.set_pan_origin(Some((sx, sy)));
        }
    }

    pub fn on_pointer_up(&mut self, x: f32, y: f32, _button: u8) {
        if let Some((fqdn, dx, dy)) = self.interaction.take_click_candidate() {
            let moved = (f64::from(x) - dx).hypot(f64::from(y) - dy);
            if moved < 5.0 {
                if let Some(cb) = &self.on_node_click {
                    defer_callback(cb.clone(), Some(fqdn));
                }
            }
        }
        self.interaction.set_pan_origin(None);
    }

    /// JS double-click → zoom-to-fit the deepest frame under the
    /// cursor. Coordinates are CSS pixels relative to the canvas's
    /// bounding rect, same convention as `on_pointer_*`.
    pub fn on_double_click(&mut self, x: f32, y: f32) {
        let (wx, wy) = self
            .viewport
            .screen_to_world(f64::from(x), f64::from(y));
        if let Some(bounds) = self.scene.frame_bounds_at(wx, wy) {
            self.viewport.fit_to(bounds, self.width, self.height);
            self.needs_redraw = true;
        }
    }

    /// Breadcrumb trail for the current viewport as a JSON array
    /// `[{label, id}]`, root → deepest. The host renders it as a
    /// clickable breadcrumb; each `id` feeds `fit_to_frame`. Empty
    /// array at full overview (no frame contains the viewport).
    pub fn focus_path(&self) -> String {
        let (vx0, vy0) = self.viewport.screen_to_world(0.0, 0.0);
        let (vx1, vy1) = self
            .viewport
            .screen_to_world(self.width, self.height);
        let crumbs: Vec<FocusCrumb> = self
            .scene
            .focus_path(vx0, vy0, vx1, vy1)
            .into_iter()
            .map(|(label, id)| FocusCrumb { label, id })
            .collect();
        serde_json::to_string(&crumbs).unwrap_or_else(|_| "[]".to_string())
    }

    /// Zoom-to-fit a frame by its hierarchy arena index (a breadcrumb
    /// crumb `id`). No-op for an out-of-range id.
    pub fn fit_to_frame(&mut self, id: u32) {
        if let Some(bounds) = self.scene.frame_bounds(id) {
            self.viewport.fit_to(bounds, self.width, self.height);
            self.needs_redraw = true;
        }
    }

    pub fn on_pointer_leave(&mut self) {
        if self.interaction.hovered().is_some() {
            self.interaction.set_hovered(None);
            self.needs_redraw = true;
        }
        self.interaction.set_pan_origin(None);
    }

    pub fn on_wheel(&mut self, x: f32, y: f32, delta_y: f32) {
        let factor = if delta_y < 0.0 { 1.15 } else { 1.0 / 1.15 };
        self.viewport
            .zoom_around(f64::from(x), f64::from(y), factor);
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
        matches!(self.mode, BackendMode::WebGpu) && self.gpu.is_some()
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
