//! Phase 3 (Shell) — **Overview canvas**. Workspace-level view: each
//! project rendered as a luminous cluster (nebula-ish), inter-project
//! dependencies as glow strands whose intensity tracks edge weight.
//!
//! Phase 3b (this revision) implements a deterministic sunflower
//! packing — biggest project at centre, others spiralling out via the
//! Fibonacci angle so the layout reads organically without running a
//! force simulation each frame. Each cluster paints as a radial-
//! gradient disc whose radius derives from `sqrt(symbol_count)`, with
//! the label below. Inter-project edges are straight lines whose
//! stroke width is weight-driven (logarithmic so a single dependency
//! still draws and a 200-edge bundle isn't a black slab).
//!
//! Hit-test fires `on_cluster_hover` / `on_cluster_click` for the
//! disc under the pointer. Pan / zoom + a real 3D force layout +
//! glow particle layer land in Phase 3c. Slim-down of the legacy
//! GraphEngine path is Phase 3d.

use serde::Deserialize;
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

/// One project cluster — a nebula in the workspace overview. Phase 3b
/// uses `id` / `label` / `symbol_count` for layout + rendering. The
/// `kind` field is parked for Phase 3c where it'll tint the radial
/// gradient (rust → violet, bun → orange, node → green, etc.).
#[allow(dead_code)] // kind consumed in Phase 3c
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

/// Canvas-space position + radius for a single cluster, computed once
/// in `layout()` and consumed by `draw()` / `hit_test()`.
#[derive(Debug, Clone, Copy)]
struct LaidCluster {
	x: f64,
	y: f64,
	r: f64,
}

const MIN_CLUSTER_RADIUS: f64 = 18.0;
const MAX_CLUSTER_RADIUS: f64 = 72.0;
const CLUSTER_GAP: f64 = 72.0;
const HIT_PAD: f64 = 6.0;
const LABEL_OFFSET: f64 = 8.0;
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
/// Golden angle in radians — 137.5° = `π * (3 - sqrt(5))`. Distributes
/// points evenly around the origin when paired with `sqrt(i)` radial
/// growth.
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653_3;

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
	/// Cluster id → laid-out position. Re-computed on `set_payload` /
	/// `resize`. Hover + click hit-test scan this map.
	positions: std::collections::HashMap<u32, LaidCluster>,
	/// Pre-sorted (by symbol_count desc) cluster index → cluster id so
	/// the centre is always the heaviest project.
	render_order: Vec<u32>,
	hovered: Option<u32>,
	drag: Option<DragState>,
	cam_offset_x: f64,
	cam_offset_y: f64,
	cam_zoom: f64,
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
			positions: std::collections::HashMap::new(),
			render_order: Vec::new(),
			hovered: None,
			drag: None,
			cam_offset_x: 0.0,
			cam_offset_y: 0.0,
			cam_zoom: 1.0,
		})
	}

	pub fn set_payload(&mut self, json: &str) -> Result<(), JsValue> {
		let parsed: OverviewPayload = serde_json::from_str(json)
			.map_err(|e| JsValue::from_str(&format!("OverviewCanvas: payload parse error: {e}")))?;
		self.clusters = parsed.clusters;
		self.edges = parsed.edges;
		self.layout();
		self.needs_redraw = true;
		Ok(())
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

	pub fn set_on_cluster_click(&mut self, cb: js_sys::Function) {
		self.on_cluster_click = Some(cb);
	}

	pub fn set_on_cluster_hover(&mut self, cb: js_sys::Function) {
		self.on_cluster_hover = Some(cb);
	}

	pub fn on_pointer_move(&mut self, x: f64, y: f64) {
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
		self.hovered = hit;
		self.needs_redraw = true;
		if let Some(cb) = &self.on_cluster_hover {
			let arg = hit.map_or(JsValue::NULL, |id| JsValue::from_f64(f64::from(id)));
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
		if let Some(id) = self.hit_test(x, y) {
			if let Some(cb) = &self.on_cluster_click {
				let _ = cb.call1(&JsValue::NULL, &JsValue::from_f64(f64::from(id)));
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
		if let Some(cb) = &self.on_cluster_hover {
			let _ = cb.call1(&JsValue::NULL, &JsValue::NULL);
		}
	}

	pub fn on_wheel(&mut self, x: f64, y: f64, delta_y: f64) {
		// Exponential zoom anchored at the pointer — same semantics as
		// FocusGraphCanvas so muscle-memory transfers between panels.
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
		// Reset camera + re-lay in case dimensions shifted since the
		// last paint. After fit the sunflower lands centred at 1× zoom.
		self.cam_offset_x = 0.0;
		self.cam_offset_y = 0.0;
		self.cam_zoom = 1.0;
		self.layout();
		self.needs_redraw = true;
	}

	pub fn set_camera_preset(&mut self, _preset: &str) {
		// Phase 3c (3D camera).
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

	fn layout(&mut self) {
		self.positions.clear();
		self.render_order.clear();
		if self.clusters.is_empty() {
			return;
		}
		// Sort by symbol_count desc (with stable id tiebreaker) so the
		// heaviest project anchors the centre. Render the ring outward
		// from there.
		let mut order: Vec<usize> = (0..self.clusters.len()).collect();
		order.sort_by(|&a, &b| {
			let ca = &self.clusters[a];
			let cb = &self.clusters[b];
			cb.symbol_count.cmp(&ca.symbol_count).then(ca.id.cmp(&cb.id))
		});

		let cx = self.width * 0.5;
		let cy = self.height * 0.5;
		// Spiral scale tuned so a workspace of ~10–20 clusters lands
		// inside a typical panel without overflowing. Larger workspaces
		// degrade gracefully (just spiral wider). The 0.14 multiplier +
		// doubled CLUSTER_GAP push siblings far enough apart that their
		// halos no longer touch — the nebula reads as discrete clusters
		// instead of a single blob.
		let scale = f64::min(self.width, self.height) * 0.14;

		// Cluster radius from sqrt(symbol_count). Floor + ceiling keep
		// the visual hierarchy readable when counts span 3+ orders of
		// magnitude (a 10-symbol crate next to a 2000-symbol crate).
		let max_count = self
			.clusters
			.iter()
			.map(|c| c.symbol_count.max(1))
			.max()
			.unwrap_or(1) as f64;

		for (i, &cluster_idx) in order.iter().enumerate() {
			let c = &self.clusters[cluster_idx];
			let raw = (c.symbol_count.max(1) as f64).sqrt();
			let normalised = raw / max_count.sqrt();
			let r = MIN_CLUSTER_RADIUS + normalised * (MAX_CLUSTER_RADIUS - MIN_CLUSTER_RADIUS);

			let (x, y) = if i == 0 {
				(cx, cy)
			} else {
				let angle = (i as f64) * GOLDEN_ANGLE;
				let radial = scale * (i as f64).sqrt() + CLUSTER_GAP;
				(cx + radial * angle.cos(), cy + radial * angle.sin())
			};

			self.positions.insert(c.id, LaidCluster { x, y, r });
			self.render_order.push(c.id);
		}
	}

	fn draw(&self) {
		self.ctx.set_fill_style_str("#161616");
		self.ctx.fill_rect(0.0, 0.0, self.width, self.height);

		if self.clusters.is_empty() {
			self.draw_empty_state();
			return;
		}

		self.ctx.save();
		self.ctx.translate(self.cam_offset_x, self.cam_offset_y).ok();
		self.ctx.scale(self.cam_zoom, self.cam_zoom).ok();

		// Edges first so cluster discs paint on top.
		let max_weight = self
			.edges
			.iter()
			.map(|e| e.weight.max(1))
			.max()
			.unwrap_or(1) as f64;
		for e in &self.edges {
			let (Some(from), Some(to)) = (self.positions.get(&e.from), self.positions.get(&e.to))
			else { continue };
			// Logarithmic so a single edge still draws and a heavy
			// bundle doesn't dominate. Floor on alpha + width bumped
			// so even one-off cross-project edges read against the
			// dark background — used to read as invisible hairlines.
			let w_norm = (f64::from(e.weight.max(1))).ln() / (max_weight.ln().max(1.0));
			let line_w = 1.4 + w_norm * 3.2;
			let alpha = 0.55 + w_norm * 0.4;
			self.ctx.set_stroke_style_str(&format!("rgba(120, 180, 255, {alpha:.3})"));
			self.ctx.set_line_width(line_w);
			self.ctx.begin_path();
			self.ctx.move_to(from.x, from.y);
			self.ctx.line_to(to.x, to.y);
			self.ctx.stroke();
		}

		// Cluster discs — biggest first so smaller ones layer in front
		// when the spiral edges overlap.
		for id in &self.render_order {
			let Some(pos) = self.positions.get(id) else { continue };
			let Some(c) = self.clusters.iter().find(|c| c.id == *id) else { continue };
			let highlighted = self.hovered == Some(*id);
			self.draw_cluster(pos, c, highlighted);
		}

		self.ctx.restore();
	}

	fn draw_cluster(&self, pos: &LaidCluster, cluster: &OverviewCluster, highlighted: bool) {
		// Soft outer glow — bigger than the disc, low alpha. Conveys
		// the nebula feel without paying the cost of a real particle
		// system. The hovered cluster gets a brighter halo.
		let halo_alpha = if highlighted { 0.45 } else { 0.18 };
		self.ctx.set_fill_style_str(&format!("rgba(177, 128, 215, {halo_alpha:.3})"));
		self.ctx.begin_path();
		let _ = self.ctx.arc(pos.x, pos.y, pos.r + 16.0, 0.0, std::f64::consts::TAU);
		self.ctx.fill();

		// Radial gradient — bright violet centre fading to dark. The
		// browser handles the math, no shader required.
		let gradient = match self.ctx.create_radial_gradient(pos.x, pos.y, 0.0, pos.x, pos.y, pos.r) {
			Ok(g) => g,
			Err(_) => return,
		};
		let _ = gradient.add_color_stop(0.0, "rgba(220, 200, 255, 0.95)");
		let _ = gradient.add_color_stop(0.55, "rgba(170, 130, 215, 0.75)");
		let _ = gradient.add_color_stop(1.0, "rgba(60, 40, 95, 0.4)");
		self.ctx.set_fill_style_canvas_gradient(&gradient);
		self.ctx.begin_path();
		let _ = self.ctx.arc(pos.x, pos.y, pos.r, 0.0, std::f64::consts::TAU);
		self.ctx.fill();

		// Label below the cluster. Counts paint smaller + dimmer.
		self.ctx.set_fill_style_str(if highlighted { "#ffffff" } else { "#cccccc" });
		self.ctx.set_font("600 13px ui-monospace, SFMono-Regular, monospace");
		self.ctx.set_text_align("center");
		self.ctx.set_text_baseline("top");
		let _ = self.ctx.fill_text(&cluster.label, pos.x, pos.y + pos.r + LABEL_OFFSET);

		self.ctx.set_fill_style_str("#9d9d9d");
		self.ctx.set_font("10px ui-monospace, SFMono-Regular, monospace");
		let _ = self.ctx.fill_text(
			&format!("{} symbols", cluster.symbol_count),
			pos.x,
			pos.y + pos.r + LABEL_OFFSET + 16.0,
		);
	}

	fn draw_empty_state(&self) {
		self.ctx.set_fill_style_str("#666666");
		self.ctx.set_font("13px ui-monospace, SFMono-Regular, monospace");
		self.ctx.set_text_align("center");
		self.ctx.set_text_baseline("middle");
		let _ = self.ctx.fill_text(
			"No projects loaded yet.",
			self.width * 0.5,
			self.height * 0.5,
		);
	}

	fn hit_test(&self, screen_x: f64, screen_y: f64) -> Option<u32> {
		// Invert the camera so the threshold compares like-with-like
		// against the world-space positions stored in `positions`.
		let world_x = (screen_x - self.cam_offset_x) / self.cam_zoom;
		let world_y = (screen_y - self.cam_offset_y) / self.cam_zoom;
		let pad = HIT_PAD / self.cam_zoom;
		let mut best: Option<(f64, u32)> = None;
		for (id, p) in &self.positions {
			let d = (world_x - p.x).hypot(world_y - p.y);
			if d <= p.r + pad {
				if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
					best = Some((d, *id));
				}
			}
		}
		best.map(|(_, id)| id)
	}
}
