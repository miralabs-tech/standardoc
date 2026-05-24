//! Phase 3 (Shell) — **Overview canvas**. Workspace-level view: each
//! project rendered as a luminous cluster (nebula), inter-project
//! dependencies as glow strands whose intensity tracks edge weight.
//!
//! Phase 3a (this commit) is the additive skeleton: the wasm-bindgen
//! struct compiles, accepts a payload, exposes telemetry, and renders
//! a Canvas2D placeholder so the new TS component
//! (`<standardoc-overview>`) can mount end-to-end against a real engine
//! instance. The real cluster layout (force3d retuned for project-
//! granularity) + glow rendering lands in Phase 3b. The wasm-bindgen
//! surface defined here is the contract Phase 3b will fill in.
//!
//! The legacy [`crate::GraphEngine`] stays alongside this module
//! through Phase 3c — slim-down + delete of tree/scene/render/layout/
//! viewport is Phase 3d.

use serde::Deserialize;
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

/// One project cluster — a nebula in the workspace overview. Phase 3a
/// only uses `id` / `label` / `symbol_count` for the placeholder
/// status text; Phase 3b will consume `kind` (project type → tint) +
/// `symbol_count` (cluster radius) + the `(x, y, z)` slot a force
/// layout will compute.
#[allow(dead_code)] // fields read in Phase 3b
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OverviewCluster {
	pub id: u32,
	pub label: String,
	pub kind: Option<String>,
	pub symbol_count: u32,
}

/// One inter-project edge. `weight` is the count of cross-project
/// symbol-level edges aggregated into this lane (e.g. CALLS + IMPORTS
/// + USES_TYPE from project A symbols into project B symbols).
#[allow(dead_code)] // fields read in Phase 3b
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OverviewEdge {
	pub from: u32,
	pub to: u32,
	pub weight: u32,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct OverviewPayload {
	#[serde(default)]
	clusters: Vec<OverviewCluster>,
	#[serde(default)]
	edges: Vec<OverviewEdge>,
}

#[wasm_bindgen]
pub struct OverviewCanvas {
	canvas: HtmlCanvasElement,
	ctx: CanvasRenderingContext2d,
	width: f64,
	height: f64,
	device_pixel_ratio: f64,
	clusters: Vec<OverviewCluster>,
	edges: Vec<OverviewEdge>,
	on_cluster_click: Option<js_sys::Function>,
	on_cluster_hover: Option<js_sys::Function>,
	needs_redraw: bool,
}

#[wasm_bindgen]
impl OverviewCanvas {
	#[wasm_bindgen(constructor)]
	pub fn new(canvas: HtmlCanvasElement, width: u32, height: u32, dpr: f64) -> Result<OverviewCanvas, JsValue> {
		let ctx = canvas
			.get_context("2d")?
			.ok_or_else(|| JsValue::from_str("OverviewCanvas: 2d context unavailable"))?
			.dyn_into::<CanvasRenderingContext2d>()?;
		crate::apply_canvas_size(&canvas, width, height, dpr);
		Ok(OverviewCanvas {
			canvas,
			ctx,
			width: f64::from(width),
			height: f64::from(height),
			device_pixel_ratio: dpr,
			clusters: Vec::new(),
			edges: Vec::new(),
			on_cluster_click: None,
			on_cluster_hover: None,
			needs_redraw: true,
		})
	}

	pub fn set_payload(&mut self, json: &str) -> Result<(), JsValue> {
		let parsed: OverviewPayload = serde_json::from_str(json)
			.map_err(|e| JsValue::from_str(&format!("OverviewCanvas: payload parse error: {e}")))?;
		self.clusters = parsed.clusters;
		self.edges = parsed.edges;
		self.needs_redraw = true;
		Ok(())
	}

	pub fn tick(&mut self) {
		if !self.needs_redraw {
			return;
		}
		self.draw_placeholder();
		self.needs_redraw = false;
	}

	pub fn invalidate(&mut self) {
		self.needs_redraw = true;
	}

	pub fn resize(&mut self, width: u32, height: u32) {
		self.width = f64::from(width);
		self.height = f64::from(height);
		crate::apply_canvas_size(&self.canvas, width, height, self.device_pixel_ratio);
		self.needs_redraw = true;
	}

	pub fn set_device_pixel_ratio(&mut self, dpr: f64) {
		if (self.device_pixel_ratio - dpr).abs() < f64::EPSILON {
			return;
		}
		self.device_pixel_ratio = dpr;
		crate::apply_canvas_size(&self.canvas, self.width as u32, self.height as u32, dpr);
		self.needs_redraw = true;
	}

	pub fn set_on_cluster_click(&mut self, cb: js_sys::Function) {
		self.on_cluster_click = Some(cb);
	}

	pub fn set_on_cluster_hover(&mut self, cb: js_sys::Function) {
		self.on_cluster_hover = Some(cb);
	}

	pub fn on_pointer_move(&mut self, _x: f64, _y: f64) {
		// Phase 3b: hit-test clusters, fire on_cluster_hover with the
		// resolved cluster id (or null on exit). Phase 3a is a no-op.
	}

	pub fn on_pointer_down(&mut self, _x: f64, _y: f64, _button: i16) {
		// Phase 3b: track drag start for orbit camera (3D view).
	}

	pub fn on_pointer_up(&mut self, _x: f64, _y: f64, _button: i16) {
		// Phase 3b: on quick-click without drag, fire on_cluster_click
		// with the hit cluster id.
	}

	pub fn on_pointer_leave(&mut self) {
		// Phase 3b: clear hover state, fire on_cluster_hover(null).
	}

	pub fn on_wheel(&mut self, _x: f64, _y: f64, _delta_y: f64) {
		// Phase 3b: zoom (dolly the orbit camera).
	}

	pub fn fit(&mut self) {
		// Phase 3b: reset camera to a framing that holds every cluster
		// in view.
		self.needs_redraw = true;
	}

	pub fn set_camera_preset(&mut self, _preset: &str) {
		// Phase 3b: orbit/top/front/side presets.
		self.needs_redraw = true;
	}

	#[wasm_bindgen(getter)]
	pub fn cluster_count(&self) -> usize {
		self.clusters.len()
	}

	#[wasm_bindgen(getter)]
	pub fn edge_count(&self) -> usize {
		self.edges.len()
	}

	fn draw_placeholder(&self) {
		self.ctx.set_fill_style_str("#161616");
		self.ctx.fill_rect(0.0, 0.0, self.width, self.height);

		self.ctx.set_fill_style_str("#9d9d9d");
		self.ctx.set_font("13px ui-monospace, SFMono-Regular, monospace");
		self.ctx.set_text_align("center");
		self.ctx.set_text_baseline("middle");

		let title = "OverviewCanvas — Phase 3a skeleton";
		let _ = self.ctx.fill_text(title, self.width * 0.5, self.height * 0.5 - 12.0);

		let counts = format!(
			"{} clusters · {} inter-project edges (Phase 3b will render the nebula layout)",
			self.clusters.len(),
			self.edges.len(),
		);
		self.ctx.set_fill_style_str("#666666");
		let _ = self.ctx.fill_text(&counts, self.width * 0.5, self.height * 0.5 + 12.0);
	}
}
