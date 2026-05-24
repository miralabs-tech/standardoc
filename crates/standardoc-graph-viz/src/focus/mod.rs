//! Phase 3 (Shell) — **Focus Graph canvas**. Symbol-local view: the
//! focused symbol at the centre, depth-N BFS neighbourhood expanded
//! around it, edges labelled inline (CALLS / IMPORTS / USES_TYPE /
//! IMPLEMENTS / EXTENDS / TESTS) via a DOM overlay supplied by
//! `label_layout()` so the host pins text elements over the canvas
//! coordinates we compute.
//!
//! Phase 3a (this commit) is the additive skeleton matching the
//! `<standardoc-focus-graph>` TS wrapper. The wasm-bindgen surface is
//! the locked contract Phase 3b will fill with the real layout +
//! rendering. Hop selector wiring + click drill lands in Phase 3c.
//!
//! The legacy [`crate::GraphEngine`] stays alongside this module
//! through Phase 3c — slim-down + delete of tree/scene/render/layout/
//! viewport is Phase 3d.

use serde::Deserialize;
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

#[allow(dead_code)] // fqdn / kind / depth read in Phase 3b
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FocusNode {
	pub fqdn: String,
	pub name: String,
	pub kind: Option<String>,
	/// BFS depth from the centre. `0` for the focal symbol itself.
	pub depth: u32,
}

#[allow(dead_code)] // fields read in Phase 3b
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FocusEdge {
	pub from: String,
	pub to: String,
	pub kind: String,
	pub depth: u32,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FocusPayload {
	center: Option<FocusNode>,
	#[serde(default)]
	neighbors: Vec<FocusNode>,
	#[serde(default)]
	edges: Vec<FocusEdge>,
}

#[wasm_bindgen]
pub struct FocusGraphCanvas {
	canvas: HtmlCanvasElement,
	ctx: CanvasRenderingContext2d,
	width: f64,
	height: f64,
	device_pixel_ratio: f64,
	center: Option<FocusNode>,
	neighbors: Vec<FocusNode>,
	edges: Vec<FocusEdge>,
	/// `0` means "All" (no hop cap); `1` / `2` / `3` cap BFS depth.
	hop_count: u32,
	on_node_click: Option<js_sys::Function>,
	on_node_hover: Option<js_sys::Function>,
	needs_redraw: bool,
}

#[wasm_bindgen]
impl FocusGraphCanvas {
	#[wasm_bindgen(constructor)]
	pub fn new(canvas: HtmlCanvasElement, width: u32, height: u32, dpr: f64) -> Result<FocusGraphCanvas, JsValue> {
		let ctx = canvas
			.get_context("2d")?
			.ok_or_else(|| JsValue::from_str("FocusGraphCanvas: 2d context unavailable"))?
			.dyn_into::<CanvasRenderingContext2d>()?;
		crate::apply_canvas_size(&canvas, width, height, dpr);
		Ok(FocusGraphCanvas {
			canvas,
			ctx,
			width: f64::from(width),
			height: f64::from(height),
			device_pixel_ratio: dpr,
			center: None,
			neighbors: Vec::new(),
			edges: Vec::new(),
			hop_count: 1,
			on_node_click: None,
			on_node_hover: None,
			needs_redraw: true,
		})
	}

	pub fn set_payload(&mut self, json: &str) -> Result<(), JsValue> {
		let parsed: FocusPayload = serde_json::from_str(json)
			.map_err(|e| JsValue::from_str(&format!("FocusGraphCanvas: payload parse error: {e}")))?;
		self.center = parsed.center;
		self.neighbors = parsed.neighbors;
		self.edges = parsed.edges;
		self.needs_redraw = true;
		Ok(())
	}

	pub fn set_hop_count(&mut self, hops: u32) {
		if self.hop_count == hops {
			return;
		}
		self.hop_count = hops;
		self.needs_redraw = true;
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

	pub fn set_on_node_click(&mut self, cb: js_sys::Function) {
		self.on_node_click = Some(cb);
	}

	pub fn set_on_node_hover(&mut self, cb: js_sys::Function) {
		self.on_node_hover = Some(cb);
	}

	pub fn on_pointer_move(&mut self, _x: f64, _y: f64) {
		// Phase 3b: hit-test nodes, fire hover callback.
	}

	pub fn on_pointer_down(&mut self, _x: f64, _y: f64, _button: i16) {
		// Phase 3b: track drag for pan.
	}

	pub fn on_pointer_up(&mut self, _x: f64, _y: f64, _button: i16) {
		// Phase 3b: on quick-click without drag, fire node click with
		// the hit fqdn.
	}

	pub fn on_pointer_leave(&mut self) {
		// Phase 3b: clear hover, fire hover(null).
	}

	pub fn on_wheel(&mut self, _x: f64, _y: f64, _delta_y: f64) {
		// Phase 3b: zoom (scale the focal-radial layout).
	}

	pub fn fit(&mut self) {
		// Phase 3b: frame all currently-visible nodes (after the hop
		// cap is applied).
		self.needs_redraw = true;
	}

	/// JSON for the DOM overlay layer — one entry per edge label
	/// (CALLS / IMPORTS / etc.) anchored at the canvas-space midpoint
	/// of the edge. Phase 3a returns `[]`; Phase 3b will fill this in
	/// once the layout is real.
	pub fn label_layout(&self) -> String {
		"[]".to_string()
	}

	#[wasm_bindgen(getter)]
	pub fn node_count(&self) -> usize {
		self.neighbors.len() + usize::from(self.center.is_some())
	}

	#[wasm_bindgen(getter)]
	pub fn edge_count(&self) -> usize {
		self.edges.len()
	}

	#[wasm_bindgen(getter)]
	pub fn current_hop_count(&self) -> u32 {
		self.hop_count
	}

	fn draw_placeholder(&self) {
		self.ctx.set_fill_style_str("#161616");
		self.ctx.fill_rect(0.0, 0.0, self.width, self.height);

		self.ctx.set_fill_style_str("#9d9d9d");
		self.ctx.set_font("13px ui-monospace, SFMono-Regular, monospace");
		self.ctx.set_text_align("center");
		self.ctx.set_text_baseline("middle");

		let title = "FocusGraphCanvas — Phase 3a skeleton";
		let _ = self.ctx.fill_text(title, self.width * 0.5, self.height * 0.5 - 24.0);

		let centre = self.center.as_ref().map_or("<no focus>", |c| c.name.as_str());
		let focus_line = format!("focal: {centre}");
		self.ctx.set_fill_style_str("#cccccc");
		let _ = self.ctx.fill_text(&focus_line, self.width * 0.5, self.height * 0.5);

		let hop_label = if self.hop_count == 0 { "All".to_string() } else { self.hop_count.to_string() };
		let counts = format!(
			"{} neighbours · {} edges · hops: {} (Phase 3b will render the focal layout)",
			self.neighbors.len(),
			self.edges.len(),
			hop_label,
		);
		self.ctx.set_fill_style_str("#666666");
		let _ = self.ctx.fill_text(&counts, self.width * 0.5, self.height * 0.5 + 24.0);
	}
}
