//! Phase 3 (Shell) — **Focus Graph canvas**. Symbol-local view: the
//! focused symbol at the centre, depth-N BFS neighbourhood expanded
//! around it, edges labelled inline (CALLS / IMPORTS / USES_TYPE /
//! IMPLEMENTS / EXTENDS / TESTS) via a DOM overlay supplied by
//! `label_layout()` — the host pins text elements over the canvas
//! coordinates we compute.
//!
//! Phase 3b (this revision) implements the real radial layout: focal
//! at centre, neighbours evenly distributed on a ring around. Each
//! node is a filled circle whose colour echoes its kind (via the
//! shared `palette::kind_color_hex`); edges are straight lines whose
//! hue echoes their edge kind, with the edge's `kind` rendered as a
//! DOM-overlay label at the midpoint. Hit-test fires `on_node_click`
//! for any neighbour quick-clicked.
//!
//! Pan/zoom + drag camera and depth-2+ multi-ring layouts land in
//! Phase 3c. Slim-down of the legacy GraphEngine path is Phase 3d.

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::kind::Kind;
use crate::palette::kind_color_hex;

#[allow(dead_code)] // depth read in Phase 3c for multi-ring layouts
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FocusNode {
	pub fqdn: String,
	pub name: String,
	#[serde(default)]
	pub kind: Option<String>,
	/// BFS depth from the centre. `0` for the focal symbol itself.
	pub depth: u32,
}

#[allow(dead_code)] // depth read in Phase 3c
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

/// Computed canvas-space position for a single node. Centre is at
/// `radius == 0.0`; neighbours sit on a ring whose radius is derived
/// from canvas size in `layout()`.
#[derive(Debug, Clone, Copy)]
struct LaidNode {
	x: f64,
	y: f64,
	r: f64,
}

#[derive(Serialize)]
struct EdgeLabelOut<'a> {
	text: &'a str,
	x: f64,
	y: f64,
}

const CENTER_RADIUS: f64 = 22.0;
const NEIGHBOR_RADIUS: f64 = 14.0;
const NODE_LABEL_OFFSET: f64 = 18.0;
const EDGE_LINE_WIDTH: f64 = 1.5;
const HIT_PAD: f64 = 4.0;

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
	/// FQDN → laid-out position, recomputed on `set_payload` /
	/// `resize`. Centre lives under the focal FQDN; neighbours each
	/// have their own entry.
	positions: std::collections::HashMap<String, LaidNode>,
	hovered: Option<String>,
	drag_origin: Option<(f64, f64)>,
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
			positions: std::collections::HashMap::new(),
			hovered: None,
			drag_origin: None,
		})
	}

	pub fn set_payload(&mut self, json: &str) -> Result<(), JsValue> {
		let parsed: FocusPayload = serde_json::from_str(json)
			.map_err(|e| JsValue::from_str(&format!("FocusGraphCanvas: payload parse error: {e}")))?;
		self.center = parsed.center;
		self.neighbors = parsed.neighbors;
		// Stable visual order — alphabetical by name so the same payload
		// always reads the same way across reloads / cache hits.
		self.neighbors.sort_by(|a, b| a.name.cmp(&b.name));
		self.edges = parsed.edges;
		self.layout();
		self.needs_redraw = true;
		Ok(())
	}

	pub fn set_hop_count(&mut self, hops: u32) {
		if self.hop_count == hops {
			return;
		}
		self.hop_count = hops;
		// Layout is depth-1 only in Phase 3b; depth filtering lands in
		// Phase 3c when multi-ring layouts arrive.
		self.layout();
		self.needs_redraw = true;
	}

	pub fn tick(&mut self) {
		if !self.needs_redraw {
			return;
		}
		self.draw();
		self.needs_redraw = false;
	}

	pub fn invalidate(&mut self) {
		self.needs_redraw = true;
	}

	pub fn resize(&mut self, width: u32, height: u32) {
		self.width = f64::from(width);
		self.height = f64::from(height);
		crate::apply_canvas_size(&self.canvas, width, height, self.device_pixel_ratio);
		self.layout();
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

	pub fn on_pointer_move(&mut self, x: f64, y: f64) {
		let hit = self.hit_test(x, y);
		if hit == self.hovered {
			return;
		}
		self.hovered = hit.clone();
		self.needs_redraw = true;
		if let Some(cb) = &self.on_node_hover {
			let arg = hit.map_or(JsValue::NULL, JsValue::from);
			let _ = cb.call1(&JsValue::NULL, &arg);
		}
	}

	pub fn on_pointer_down(&mut self, x: f64, y: f64, _button: i16) {
		self.drag_origin = Some((x, y));
	}

	pub fn on_pointer_up(&mut self, x: f64, y: f64, _button: i16) {
		// Quick-click discriminator: if the pointer barely moved between
		// down and up, treat it as a click; otherwise it was a drag (no-op
		// in Phase 3b, pan camera in 3c).
		let was_click = match self.drag_origin {
			Some((sx, sy)) => (x - sx).hypot(y - sy) < 4.0,
			None => true,
		};
		self.drag_origin = None;
		if !was_click {
			return;
		}
		if let Some(fqdn) = self.hit_test(x, y) {
			if let Some(cb) = &self.on_node_click {
				let _ = cb.call1(&JsValue::NULL, &JsValue::from(fqdn));
			}
		}
	}

	pub fn on_pointer_leave(&mut self) {
		self.drag_origin = None;
		if self.hovered.is_none() {
			return;
		}
		self.hovered = None;
		self.needs_redraw = true;
		if let Some(cb) = &self.on_node_hover {
			let _ = cb.call1(&JsValue::NULL, &JsValue::NULL);
		}
	}

	pub fn on_wheel(&mut self, _x: f64, _y: f64, _delta_y: f64) {
		// Phase 3c: zoom around pointer.
	}

	pub fn fit(&mut self) {
		// Radial layout is already canvas-fitted; just re-layout in case
		// dimensions changed since the last paint.
		self.layout();
		self.needs_redraw = true;
	}

	/// JSON for the DOM overlay layer — one entry per edge label
	/// (CALLS / IMPORTS / etc.) anchored at the canvas-space midpoint
	/// of the edge. Coordinates are in CSS pixels, ready to drop into
	/// the wrapper component's absolute overlay.
	pub fn label_layout(&self) -> String {
		let mut out = Vec::with_capacity(self.edges.len());
		for e in &self.edges {
			let Some(from) = self.positions.get(&e.from) else { continue };
			let Some(to) = self.positions.get(&e.to) else { continue };
			out.push(EdgeLabelOut {
				text: &e.kind,
				x: (from.x + to.x) * 0.5,
				y: (from.y + to.y) * 0.5,
			});
		}
		serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string())
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

	fn layout(&mut self) {
		self.positions.clear();
		let cx = self.width * 0.5;
		let cy = self.height * 0.5;
		let Some(centre) = &self.center else { return };
		self.positions.insert(centre.fqdn.clone(), LaidNode { x: cx, y: cy, r: CENTER_RADIUS });
		if self.neighbors.is_empty() {
			return;
		}
		// Ring radius: 38% of the smaller dimension so even small
		// canvases keep the ring inside the viewport with room for
		// labels. Phase 3c will adapt this for multi-ring layouts.
		let ring = f64::min(self.width, self.height) * 0.38;
		let n = self.neighbors.len() as f64;
		// Start at -π/2 (top) so the first neighbour reads at 12 o'clock
		// instead of the conventional 3 o'clock — feels more natural for
		// a focal-symbol view.
		let start = -std::f64::consts::FRAC_PI_2;
		for (i, neighbor) in self.neighbors.iter().enumerate() {
			let angle = start + (i as f64) * std::f64::consts::TAU / n;
			let nx = cx + ring * angle.cos();
			let ny = cy + ring * angle.sin();
			self.positions.insert(neighbor.fqdn.clone(), LaidNode { x: nx, y: ny, r: NEIGHBOR_RADIUS });
		}
	}

	fn draw(&self) {
		self.ctx.set_fill_style_str("#161616");
		self.ctx.fill_rect(0.0, 0.0, self.width, self.height);

		if self.center.is_none() {
			self.draw_empty_state();
			return;
		}

		// Edges first so nodes paint on top.
		self.ctx.set_line_width(EDGE_LINE_WIDTH);
		for e in &self.edges {
			let (Some(from), Some(to)) = (self.positions.get(&e.from), self.positions.get(&e.to))
			else { continue };
			self.ctx.set_stroke_style_str(edge_color_for_kind(&e.kind));
			self.ctx.begin_path();
			self.ctx.move_to(from.x, from.y);
			self.ctx.line_to(to.x, to.y);
			self.ctx.stroke();
		}

		// Nodes (centre + neighbours).
		if let Some(centre) = &self.center {
			if let Some(pos) = self.positions.get(&centre.fqdn) {
				let highlighted = self.hovered.as_deref() == Some(centre.fqdn.as_str());
				self.draw_node(pos, centre, true, highlighted);
			}
		}
		for n in &self.neighbors {
			if let Some(pos) = self.positions.get(&n.fqdn) {
				let highlighted = self.hovered.as_deref() == Some(n.fqdn.as_str());
				self.draw_node(pos, n, false, highlighted);
			}
		}
	}

	fn draw_node(&self, pos: &LaidNode, node: &FocusNode, is_center: bool, highlighted: bool) {
		let kind = parse_kind(node.kind.as_deref());
		let fill = kind_color_hex(kind);
		// Halo behind the centre and any hovered node — soft cue that
		// reads on both light and dark surrounding lines.
		if is_center || highlighted {
			self.ctx.set_fill_style_str("rgba(74, 158, 255, 0.18)");
			self.ctx.begin_path();
			let _ = self.ctx.arc(pos.x, pos.y, pos.r + 6.0, 0.0, std::f64::consts::TAU);
			self.ctx.fill();
		}
		self.ctx.set_fill_style_str(fill);
		self.ctx.begin_path();
		let _ = self.ctx.arc(pos.x, pos.y, pos.r, 0.0, std::f64::consts::TAU);
		self.ctx.fill();
		// Outline for the centre to anchor the eye.
		if is_center {
			self.ctx.set_stroke_style_str("#ffffff");
			self.ctx.set_line_width(2.0);
			self.ctx.begin_path();
			let _ = self.ctx.arc(pos.x, pos.y, pos.r, 0.0, std::f64::consts::TAU);
			self.ctx.stroke();
		}
		// Label below the node. Centre's label is bolder + larger.
		self.ctx.set_fill_style_str(if is_center { "#ffffff" } else { "#cccccc" });
		self.ctx.set_font(if is_center {
			"600 13px ui-monospace, SFMono-Regular, monospace"
		} else {
			"11px ui-monospace, SFMono-Regular, monospace"
		});
		self.ctx.set_text_align("center");
		self.ctx.set_text_baseline("top");
		let _ = self.ctx.fill_text(&node.name, pos.x, pos.y + pos.r + NODE_LABEL_OFFSET - 14.0);
	}

	fn draw_empty_state(&self) {
		self.ctx.set_fill_style_str("#666666");
		self.ctx.set_font("13px ui-monospace, SFMono-Regular, monospace");
		self.ctx.set_text_align("center");
		self.ctx.set_text_baseline("middle");
		let _ = self.ctx.fill_text(
			"Click a symbol to inspect its neighbourhood.",
			self.width * 0.5,
			self.height * 0.5,
		);
	}

	fn hit_test(&self, x: f64, y: f64) -> Option<String> {
		let mut best: Option<(f64, String)> = None;
		for (fqdn, p) in &self.positions {
			let d = (x - p.x).hypot(y - p.y);
			if d <= p.r + HIT_PAD {
				if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
					best = Some((d, fqdn.clone()));
				}
			}
		}
		best.map(|(_, f)| f)
	}
}

fn parse_kind(kind: Option<&str>) -> Kind {
	match kind.unwrap_or("") {
		"type" | "struct" | "enum" | "trait" | "interface" => Kind::Type,
		"callable" | "function" | "method" | "fn" => Kind::Callable,
		"value" | "const" | "static" => Kind::Value,
		"module" | "mod" => Kind::Module,
		"macro" | "macro_rules" => Kind::Macro,
		_ => Kind::Unknown,
	}
}

fn edge_color_for_kind(kind: &str) -> &'static str {
	match kind {
		"CALLS" => "#3794ff",      // blue — behaviour link
		"IMPORTS" => "#b180d7",    // purple — namespace link
		"USES_TYPE" => "#cca700",  // yellow — type reference
		"IMPLEMENTS" => "#f48771", // orange — contract fulfilment
		"EXTENDS" => "#5aa9ff",    // bright blue — inheritance
		"TESTS" => "#89d185",      // green — verification
		"REFERENCES" => "#9d9d9d", // grey — generic reference
		_ => "#666666",
	}
}
