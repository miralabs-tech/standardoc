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
const ZOOM_MIN: f64 = 0.1;
const ZOOM_MAX: f64 = 8.0;
const ZOOM_STEP: f64 = 0.0012;
const CLICK_DRAG_THRESHOLD: f64 = 4.0;

#[derive(Debug, Clone, Copy)]
struct DragState {
	start_pointer_x: f64,
	start_pointer_y: f64,
	start_offset_x: f64,
	start_offset_y: f64,
	moved: bool,
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
	/// FQDN → laid-out position, recomputed on `set_payload` /
	/// `resize`. Centre lives under the focal FQDN; neighbours each
	/// have their own entry.
	positions: std::collections::HashMap<String, LaidNode>,
	hovered: Option<String>,
	drag: Option<DragState>,
	/// Camera translation in screen pixels — pan offset applied via
	/// `ctx.translate` before nodes/edges render. Zero on fit().
	cam_offset_x: f64,
	cam_offset_y: f64,
	/// Camera zoom multiplier applied via `ctx.scale`. 1.0 on fit().
	cam_zoom: f64,
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
			drag: None,
			cam_offset_x: 0.0,
			cam_offset_y: 0.0,
			cam_zoom: 1.0,
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
		// If a drag is active, the pointer move pans the camera. The
		// world point under the cursor stays put because we shift the
		// camera offset by the screen-space delta.
		if let Some(drag) = &mut self.drag {
			let dx = x - drag.start_pointer_x;
			let dy = y - drag.start_pointer_y;
			if dx.hypot(dy) >= CLICK_DRAG_THRESHOLD {
				drag.moved = true;
			}
			self.cam_offset_x = drag.start_offset_x + dx;
			self.cam_offset_y = drag.start_offset_y + dy;
			self.needs_redraw = true;
			return;
		}
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
		self.drag = Some(DragState {
			start_pointer_x: x,
			start_pointer_y: y,
			start_offset_x: self.cam_offset_x,
			start_offset_y: self.cam_offset_y,
			moved: false,
		});
	}

	pub fn on_pointer_up(&mut self, x: f64, y: f64, _button: i16) {
		let was_click = self.drag.as_ref().is_some_and(|d| !d.moved);
		self.drag = None;
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
		self.drag = None;
		if self.hovered.is_none() {
			return;
		}
		self.hovered = None;
		self.needs_redraw = true;
		if let Some(cb) = &self.on_node_hover {
			let _ = cb.call1(&JsValue::NULL, &JsValue::NULL);
		}
	}

	pub fn on_wheel(&mut self, x: f64, y: f64, delta_y: f64) {
		// Exponential zoom keeps the per-notch sensation consistent
		// whether the camera is at 0.5× or 4×. Anchored at the pointer
		// so zooming feels like a magnifier on the spot the user is
		// looking at, not a re-centre on the canvas origin.
		let factor = (-delta_y * ZOOM_STEP).exp();
		let next_zoom = (self.cam_zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
		if (next_zoom - self.cam_zoom).abs() < f64::EPSILON {
			return;
		}
		let ratio = next_zoom / self.cam_zoom;
		self.cam_offset_x = x - (x - self.cam_offset_x) * ratio;
		self.cam_offset_y = y - (y - self.cam_offset_y) * ratio;
		self.cam_zoom = next_zoom;
		self.needs_redraw = true;
	}

	pub fn fit(&mut self) {
		// Reset camera + re-layout in case dimensions changed since the
		// last paint. After fit the focal node lands at canvas centre at
		// 1× zoom.
		self.cam_offset_x = 0.0;
		self.cam_offset_y = 0.0;
		self.cam_zoom = 1.0;
		self.layout();
		self.needs_redraw = true;
	}

	/// JSON for the DOM overlay layer — one entry per edge label
	/// (CALLS / IMPORTS / etc.) anchored at the canvas-space midpoint
	/// of the edge. Coordinates are in CSS pixels with the camera
	/// transform applied so labels track pan + zoom alongside the
	/// canvas content.
	///
	/// Only first-ring edges get labelled: at BFS-2/3 the edge count
	/// explodes and a full label set drowns the canvas in text. The
	/// focal-to-immediate-neighbour kind is the bit a reader needs to
	/// orient; deeper edges still draw but read by line colour alone.
	pub fn label_layout(&self) -> String {
		let mut out = Vec::with_capacity(self.edges.len());
		for e in &self.edges {
			if e.depth > 1 {
				continue;
			}
			let Some(from) = self.positions.get(&e.from) else { continue };
			let Some(to) = self.positions.get(&e.to) else { continue };
			let wx = (from.x + to.x) * 0.5;
			let wy = (from.y + to.y) * 0.5;
			out.push(EdgeLabelOut {
				text: &e.kind,
				x: wx * self.cam_zoom + self.cam_offset_x,
				y: wy * self.cam_zoom + self.cam_offset_y,
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

		// Group neighbours by BFS depth so each ring carries only its
		// own depth. Single-ring layouts at BFS-2/3 turned into spaghetti
		// the moment the focal had >20 second-degree neighbours; one
		// concentric ring per depth restores readability.
		let mut by_depth: std::collections::BTreeMap<u32, Vec<&FocusNode>> = std::collections::BTreeMap::new();
		for n in &self.neighbors {
			by_depth.entry(n.depth.max(1)).or_default().push(n);
		}
		let max_depth = by_depth.keys().copied().max().unwrap_or(1);

		// Ring radii: inner ring at 28% of the smaller dimension, deeper
		// rings step outward so the outermost lands inside the canvas
		// with room for labels. Step shrinks as max_depth grows so a
		// BFS-5 layout still fits.
		let inner = f64::min(self.width, self.height) * 0.26;
		let outer = f64::min(self.width, self.height) * 0.46;
		let step = if max_depth > 1 {
			(outer - inner) / (max_depth as f64 - 1.0)
		} else {
			0.0
		};

		for (depth, nodes) in &by_depth {
			let ring = inner + (*depth as f64 - 1.0) * step;
			let count = nodes.len() as f64;
			// Stagger the starting angle per ring so radial spokes don't
			// align across depths (which used to make the eye read the
			// outer ring as one continuous mass).
			let offset = (*depth as f64 - 1.0) * std::f64::consts::FRAC_PI_6;
			let start = -std::f64::consts::FRAC_PI_2 + offset;
			let node_radius = if *depth == 1 {
				NEIGHBOR_RADIUS
			} else {
				// Deeper rings get smaller discs so the eye reads the
				// hierarchy at a glance.
				(NEIGHBOR_RADIUS / (*depth as f64).sqrt()).max(6.0)
			};
			for (i, neighbor) in nodes.iter().enumerate() {
				let angle = start + (i as f64) * std::f64::consts::TAU / count.max(1.0);
				let nx = cx + ring * angle.cos();
				let ny = cy + ring * angle.sin();
				self.positions.insert(neighbor.fqdn.clone(), LaidNode { x: nx, y: ny, r: node_radius });
			}
		}
	}

	fn draw(&self) {
		self.ctx.set_fill_style_str("#161616");
		self.ctx.fill_rect(0.0, 0.0, self.width, self.height);

		if self.center.is_none() {
			self.draw_empty_state();
			return;
		}

		// Apply camera transform — every draw call below operates in
		// world space (the coords `layout()` stored in `positions`).
		// Restoring at the end keeps the empty-state path + the next
		// frame's fill_rect in canvas-pixel space.
		self.ctx.save();
		self.ctx.translate(self.cam_offset_x, self.cam_offset_y).ok();
		self.ctx.scale(self.cam_zoom, self.cam_zoom).ok();

		// Edges first so nodes paint on top. Alpha decays with depth so
		// the outer rings don't visually dominate the focal connections.
		// When a node is hovered, anything not connected to it dims
		// further so the user's eye lands on the relationship subgraph.
		let connected: Option<std::collections::HashSet<&str>> = self.hovered.as_deref().map(|h| {
			let mut set: std::collections::HashSet<&str> = std::collections::HashSet::new();
			set.insert(h);
			for e in &self.edges {
				if e.from == h {
					set.insert(e.to.as_str());
				} else if e.to == h {
					set.insert(e.from.as_str());
				}
			}
			set
		});
		self.ctx.set_line_width(EDGE_LINE_WIDTH);
		for e in &self.edges {
			let (Some(from), Some(to)) = (self.positions.get(&e.from), self.positions.get(&e.to))
			else { continue };
			let base_alpha: f64 = match e.depth {
				0 | 1 => 0.95,
				2 => 0.55,
				_ => 0.3,
			};
			let touches_hover = connected
				.as_ref()
				.is_some_and(|set| set.contains(e.from.as_str()) && set.contains(e.to.as_str()));
			let alpha = match (&connected, touches_hover) {
				(Some(_), true) => 1.0,
				(Some(_), false) => base_alpha * 0.15,
				(None, _) => base_alpha,
			};
			let line_w = if touches_hover { EDGE_LINE_WIDTH * 1.8 } else { EDGE_LINE_WIDTH };
			self.ctx.set_global_alpha(alpha);
			self.ctx.set_line_width(line_w);
			self.ctx.set_stroke_style_str(edge_color_for_kind(&e.kind));
			self.ctx.begin_path();
			self.ctx.move_to(from.x, from.y);
			self.ctx.line_to(to.x, to.y);
			self.ctx.stroke();
		}
		self.ctx.set_global_alpha(1.0);
		self.ctx.set_line_width(EDGE_LINE_WIDTH);

		// Nodes (centre + neighbours). When a hover is active, dim
		// nodes that aren't in the hovered node's connected set so the
		// eye lands on the subgraph rather than the noise around it.
		let node_alpha = |fqdn: &str| -> f64 {
			match &connected {
				Some(set) if set.contains(fqdn) => 1.0,
				Some(_) => 0.2,
				None => 1.0,
			}
		};
		if let Some(centre) = &self.center {
			if let Some(pos) = self.positions.get(&centre.fqdn) {
				let highlighted = self.hovered.as_deref() == Some(centre.fqdn.as_str());
				self.ctx.set_global_alpha(node_alpha(&centre.fqdn));
				self.draw_node(pos, centre, true, highlighted);
			}
		}
		for n in &self.neighbors {
			if let Some(pos) = self.positions.get(&n.fqdn) {
				let highlighted = self.hovered.as_deref() == Some(n.fqdn.as_str());
				self.ctx.set_global_alpha(node_alpha(&n.fqdn));
				self.draw_node(pos, n, false, highlighted);
			}
		}
		self.ctx.set_global_alpha(1.0);

		self.ctx.restore();
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

	fn hit_test(&self, screen_x: f64, screen_y: f64) -> Option<String> {
		// Pointer events arrive in screen (CSS pixel) space; the layout
		// positions are world-space. Convert by inverting the camera
		// transform so the hit-test threshold compares like-with-like.
		let world_x = (screen_x - self.cam_offset_x) / self.cam_zoom;
		let world_y = (screen_y - self.cam_offset_y) / self.cam_zoom;
		// The HIT_PAD slop is also in screen space, so divide it by
		// zoom so the click target stays the same screen size as the
		// user zooms in.
		let pad = HIT_PAD / self.cam_zoom;
		let mut best: Option<(f64, String)> = None;
		for (fqdn, p) in &self.positions {
			let d = (world_x - p.x).hypot(world_y - p.y);
			if d <= p.r + pad {
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
