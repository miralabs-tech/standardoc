//! Phase 4 — **Focus Graph canvas (bucket layout)**. The focal symbol
//! sits centre-stage as a rounded card; its immediate neighbours are
//! grouped into typed buckets (Used by, Calls, Uses types, Imports,
//! Imported by, Tested by, Implements/Extends) and laid out spatially
//! around the centre instead of on BFS-depth concentric rings. Each
//! bucket renders as a vertical column of small cards under a DOM-
//! overlay header supplied by `bucket_layout()`. Edges are straight
//! lines coloured by edge kind (palette unchanged) and labels live on
//! the bucket headers, not on the edges themselves.
//!
//! Per-bucket categorisation is driven by the edge connecting each
//! neighbour to the centre (kind + direction). Neighbours that don't
//! touch the centre directly — depth-2+ in multi-hop modes — are
//! dropped from the layout entirely (they have no canonical bucket).
//! Each bucket caps at BUCKET_TOP_N items and surfaces the truncation
//! as a "+N more" badge under the column.
//!
//! Camera (pan/zoom + drag) is unchanged from the prior multi-ring
//! revision; hover illumination + hit-test were ported to the new
//! rectangular geometry.

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::kind::Kind;
use crate::palette::kind_color_hex;

#[allow(dead_code)] // depth retained for hop filtering
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FocusNode {
	pub fqdn: String,
	pub name: String,
	#[serde(default)]
	pub kind: Option<String>,
	/// BFS depth from the centre. `0` for the focal symbol itself.
	pub depth: u32,
	/// Optional sub-symbol counts displayed in the centre card footer
	/// ("N fields · N methods"). Only the centre uses these; other
	/// nodes leave them None.
	#[serde(default)]
	pub field_count: Option<u32>,
	#[serde(default)]
	pub method_count: Option<u32>,
	/// Source location metadata surfaced as the neighbour card's 3rd
	/// line ("file.ts:42") so each card reads as a mini semantic record
	/// per the manifesto (typography-first, semantic density).
	#[serde(default)]
	pub file: Option<String>,
	#[serde(default)]
	pub start_line: Option<u32>,
}

#[allow(dead_code)]
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

/// Computed canvas-space rectangle for a single node card.
#[derive(Debug, Clone, Copy)]
struct LaidNode {
	x: f64,
	y: f64,
	w: f64,
	h: f64,
	is_center: bool,
	/// Bucket this card belongs to (None for the centre). Drives the
	/// card border colour so neighbours read as part of their bucket
	/// at a glance, not just from their angular position.
	bucket: Option<Bucket>,
}

/// Spatial bucket a neighbour gets placed into based on the edge type
/// + direction connecting it to the centre. The mapping is canonical:
/// inbound CALLS → UsedBy, outbound CALLS → Calls, etc. Neighbours
/// reachable only via depth-2+ paths are skipped entirely (no bucket).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Bucket {
	UsedBy,
	UsesTypes,
	Calls,
	Imports,
	ImportedBy,
	TestedBy,
	ImplementsExtends,
	/// Neighbours reachable via depth-2+ paths (no direct edge to the
	/// centre). Placed far south below TestedBy so the multi-hop modes
	/// surface them without crowding the immediate-neighbour buckets.
	Indirect,
}

#[derive(Serialize)]
struct BucketLabelOut<'a> {
	text: &'a str,
	count: usize,
	x: f64,
	y: f64,
}

#[derive(Serialize)]
struct EdgeLabelOut<'a> {
	text: &'a str,
	color: &'a str,
	x: f64,
	y: f64,
}

#[derive(Serialize)]
struct OverlayPayload<'a> {
	buckets: Vec<BucketLabelOut<'a>>,
	edges: Vec<EdgeLabelOut<'a>>,
}

const CENTER_W: f64 = 220.0;
const CENTER_H: f64 = 78.0;
const ITEM_W: f64 = 178.0;
const ITEM_H: f64 = 48.0;
const ITEM_GAP: f64 = 6.0;
const ITEM_GAP_H: f64 = 10.0; // horizontal gap when bucket lays items in a row
const BUCKET_HEADER_GAP: f64 = 22.0;
const MORE_BADGE_H: f64 = 18.0;
const CARD_RADIUS: f64 = 10.0;
const EDGE_LINE_WIDTH: f64 = 2.4;
const ARROW_LEN: f64 = 11.0;
const ARROW_HALF_WIDTH: f64 = 5.5;
/// Quadratic Bézier control-point offset perpendicular to the
/// centre-to-centre direction. Drives how much the edge bows away from
/// a straight line — at twice the EDGE_LABEL_OFFSET so the curve apex
/// (t=0.5) coincides with the label's perpendicular lift.
const EDGE_BEZIER_BOW: f64 = 24.0;
/// Perpendicular lift applied to edge labels so they sit beside the
/// line rather than straddling it (the pill background would otherwise
/// occlude the stroke). World-space pixels, scaled by the camera zoom
/// at projection time.
const EDGE_LABEL_OFFSET: f64 = 12.0;

#[derive(Debug, Clone, Copy)]
enum BucketDirection {
	/// Items stacked top-to-bottom (left/right buckets default).
	Vertical,
	/// Items laid out left-to-right (bottom bucket like TestedBy in the
	/// mockup so the row width takes horizontal canvas space instead of
	/// stacking tall under the centre).
	Horizontal,
}
const HIT_PAD: f64 = 4.0;
const ZOOM_MIN: f64 = 0.1;
const ZOOM_MAX: f64 = 8.0;
const ZOOM_STEP: f64 = 0.0012;
const CLICK_DRAG_THRESHOLD: f64 = 4.0;
/// Cap the number of items rendered per bucket. Matches the mockup
/// (each bucket shows 2-5 items, the rest collapsed into a "+N more"
/// badge under the column). Real symbols can have dozens of inbound
/// CALLS — rendering them all turned the V0 into a giga-list wall.
const BUCKET_TOP_N: usize = 5;

/// Ordered bucket placements at 6 distinct compass positions around
/// the centre — NW / NE / W / E / SW / SE. Eight placements were
/// tested and collided (intermediate angles cluttered the bottom
/// half). The chosen six are airy enough for crowded symbols and map
/// directly to the mockup spatial intuition.
const BUCKET_PLACEMENTS: &[(Bucket, &str, f64, f64, BucketDirection)] = &[
	// (bucket, header label, angle_rad, radial_offset_pixels, direction)
	// Angles use canvas convention (0 right, 0.5π down, π left, 1.5π up).
	// SEMANTIC CONVENTION (per the Dark Semantic Observatory manifesto):
	//   • W (left)  = callers / used by
	//   • E (right) = dependencies / uses types
	//   • N (up)    = imports / API
	//   • S (down)  = tests / implementations
	// Each cardinal axis gets ±0.10π offset to split inbound vs outbound
	// without overlapping columns. Reading the graph by direction tells
	// the user the relationship immediately — no hover needed.
	(Bucket::UsedBy,            "Used by",     std::f64::consts::PI * 1.00, 380.0, BucketDirection::Vertical),   // W   — callers
	(Bucket::Imports,           "Imports",     std::f64::consts::PI * 1.40, 380.0, BucketDirection::Vertical),   // N-L — outbound API namespace
	(Bucket::ImportedBy,        "Imported by", std::f64::consts::PI * 1.60, 380.0, BucketDirection::Vertical),   // N-R — inbound API namespace
	(Bucket::UsesTypes,         "Uses types",  std::f64::consts::PI * 1.90, 380.0, BucketDirection::Vertical),   // E-↑ — outbound type deps
	(Bucket::Calls,             "Calls",       std::f64::consts::PI * 0.10, 380.0, BucketDirection::Vertical),   // E-↓ — outbound behaviour deps
	(Bucket::ImplementsExtends, "Impl/Extend", std::f64::consts::PI * 0.40, 380.0, BucketDirection::Vertical),   // S-R — implementations
	(Bucket::TestedBy,          "Tested by",   std::f64::consts::PI * 0.60, 380.0, BucketDirection::Horizontal), // S-L — tests
	(Bucket::Indirect,          "Indirect",    std::f64::consts::PI * 0.50, 580.0, BucketDirection::Horizontal), // S-far — depth-2+
];

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
	/// Per-bucket "+N more" badge anchor (x, y, hidden_count). Populated
	/// during layout() for buckets whose item count exceeded BUCKET_TOP_N.
	/// Rendered as a non-interactive badge under the column.
	overflow_badges: Vec<(Bucket, f64, f64, usize)>,
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
			overflow_badges: Vec::new(),
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
		// Auto-fit on payload load so the user always sees the full
		// bucket layout instead of a clipped slice when switching to a
		// symbol whose bbox doesn't naturally fit the drawer.
		self.fit_to_content();
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
		// Re-layout + camera-frame to the full bounding box so the whole
		// bucket-layout fits regardless of the drawer dimensions.
		self.layout();
		self.fit_to_content();
		self.needs_redraw = true;
	}

	/// Compute the bounding box of every laid-out card and adjust
	/// cam_zoom + cam_offset so the bbox lands centred inside the
	/// canvas with a uniform padding. Called on every `set_payload` so
	/// the user never has to manually fit after switching symbols.
	fn fit_to_content(&mut self) {
		if self.positions.is_empty() {
			self.cam_zoom = 1.0;
			self.cam_offset_x = 0.0;
			self.cam_offset_y = 0.0;
			return;
		}
		let mut min_x = f64::INFINITY;
		let mut min_y = f64::INFINITY;
		let mut max_x = f64::NEG_INFINITY;
		let mut max_y = f64::NEG_INFINITY;
		for p in self.positions.values() {
			if p.x < min_x { min_x = p.x; }
			if p.y < min_y { min_y = p.y; }
			if p.x + p.w > max_x { max_x = p.x + p.w; }
			if p.y + p.h > max_y { max_y = p.y + p.h; }
		}
		// Pad-out the bbox so bucket header pills (above the column top)
		// and the "+N more" badge (below the column bottom) don't get
		// clipped at the canvas edges.
		let header_pad = 32.0;
		let badge_pad = MORE_BADGE_H + 12.0;
		min_y -= header_pad;
		max_y += badge_pad;
		let bbox_w = (max_x - min_x).max(1.0);
		let bbox_h = (max_y - min_y).max(1.0);
		let pad = 24.0;
		let zoom_x = (self.width - pad * 2.0) / bbox_w;
		let zoom_y = (self.height - pad * 2.0) / bbox_h;
		let zoom = zoom_x.min(zoom_y).clamp(ZOOM_MIN, ZOOM_MAX);
		let bbox_cx = (min_x + max_x) * 0.5;
		let bbox_cy = (min_y + max_y) * 0.5;
		self.cam_zoom = zoom;
		self.cam_offset_x = self.width * 0.5 - bbox_cx * zoom;
		self.cam_offset_y = self.height * 0.5 - bbox_cy * zoom;
	}

	/// JSON for the DOM overlay layer — one entry per non-empty bucket
	/// header ("Used by (3)", "Calls (2)", …) anchored at the bucket's
	/// column top. Coordinates are in CSS pixels with the camera
	/// transform applied so headers track pan + zoom.
	///
	/// Replaces the prior `label_layout()` (per-edge labels at edge
	/// midpoints). The new payload reads more like a structured
	/// relation card than a connection diagram, matching the Phase 4
	/// mockup.
	pub fn label_layout(&self) -> String {
		let centre_fqdn = match &self.center {
			Some(c) => c.fqdn.clone(),
			None => return r#"{"buckets":[],"edges":[]}"#.to_string(),
		};
		// Bucket header labels — one pill per non-empty bucket, anchored
		// above the bucket's first item card.
		let mut counts: std::collections::HashMap<Bucket, usize> = std::collections::HashMap::new();
		let mut top_y: std::collections::HashMap<Bucket, f64> = std::collections::HashMap::new();
		let mut bucket_x: std::collections::HashMap<Bucket, f64> = std::collections::HashMap::new();
		for n in &self.neighbors {
			let Some(bucket) = self.bucket_for_neighbor(n, &centre_fqdn) else { continue };
			*counts.entry(bucket).or_insert(0) += 1;
			if let Some(pos) = self.positions.get(&n.fqdn) {
				let top = top_y.entry(bucket).or_insert(f64::INFINITY);
				if pos.y < *top {
					*top = pos.y;
				}
				let cx = pos.x + pos.w * 0.5;
				bucket_x.insert(bucket, cx);
			}
		}
		let mut buckets = Vec::with_capacity(counts.len());
		for (bucket, label, _angle, _radial, _direction) in BUCKET_PLACEMENTS {
			let count = match counts.get(bucket) {
				Some(c) if *c > 0 => *c,
				_ => continue,
			};
			let (wx, wy) = match (bucket_x.get(bucket), top_y.get(bucket)) {
				(Some(x), Some(y)) => (*x, *y - BUCKET_HEADER_GAP),
				_ => continue,
			};
			buckets.push(BucketLabelOut {
				text: label,
				count,
				x: wx * self.cam_zoom + self.cam_offset_x,
				y: wy * self.cam_zoom + self.cam_offset_y,
			});
		}

		// Per-edge labels — depth-1 only (depth-2+ floods the canvas),
		// positioned at the midpoint of each centre↔neighbour edge.
		// One label per bucket suffices to read the relation type at a
		// glance, but the mockup labels each edge so we render them all
		// up to a reasonable cap (FIRST_BUCKET_ITEM only when more than
		// 3 items share the same kind, otherwise per-edge).
		let mut edge_labels = Vec::new();
		let mut per_bucket_labelled: std::collections::HashMap<Bucket, usize> = std::collections::HashMap::new();
		for e in &self.edges {
			if e.depth > 1 { continue; }
			let other_fqdn = if e.from == centre_fqdn {
				&e.to
			} else if e.to == centre_fqdn {
				&e.from
			} else {
				continue;
			};
			let Some(other_pos) = self.positions.get(other_fqdn) else { continue };
			// Find the bucket for this neighbour so we can rate-limit
			// labels per bucket (max 2 labels per bucket — beyond that
			// the bucket header already conveys the kind).
			let bucket_opt = self.neighbors.iter()
				.find(|n| n.fqdn == *other_fqdn)
				.and_then(|n| self.bucket_for_neighbor(n, &centre_fqdn));
			if let Some(b) = bucket_opt {
				let n = per_bucket_labelled.entry(b).or_insert(0);
				if *n >= 2 { continue; }
				*n += 1;
			}
			let Some(centre_pos) = self.positions.get(&centre_fqdn) else { continue };
			let (cx, cy) = (centre_pos.x + centre_pos.w * 0.5, centre_pos.y + centre_pos.h * 0.5);
			let (ox, oy) = (other_pos.x + other_pos.w * 0.5, other_pos.y + other_pos.h * 0.5);
			// Position labels at the midpoint of the CLIPPED edge segment
			// (between the two card borders) so they never land on either
			// endpoint card. A small perpendicular lift keeps the pill
			// from straddling the stroke.
			let dx = ox - cx;
			let dy = oy - cy;
			let len = dx.hypot(dy).max(1.0);
			let ux = dx / len;
			let uy = dy / len;
			let centre_t = rect_edge_t(centre_pos.w * 0.5, centre_pos.h * 0.5, ux, uy);
			let other_t = rect_edge_t(other_pos.w * 0.5, other_pos.h * 0.5, ux, uy);
			let cx_c = cx + ux * centre_t;
			let cy_c = cy + uy * centre_t;
			let ox_c = ox - ux * other_t;
			let oy_c = oy - uy * other_t;
			let mid_x = (cx_c + ox_c) * 0.5;
			let mid_y = (cy_c + oy_c) * 0.5;
			let wx = mid_x + -uy * EDGE_LABEL_OFFSET;
			let wy = mid_y + ux * EDGE_LABEL_OFFSET;
			edge_labels.push(EdgeLabelOut {
				text: edge_phrase(&e.kind),
				color: edge_color_for_kind(&e.kind),
				x: wx * self.cam_zoom + self.cam_offset_x,
				y: wy * self.cam_zoom + self.cam_offset_y,
			});
		}

		let payload = OverlayPayload { buckets, edges: edge_labels };
		serde_json::to_string(&payload).unwrap_or_else(|_| r#"{"buckets":[],"edges":[]}"#.to_string())
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
		self.overflow_badges.clear();
		let cx = self.width * 0.5;
		let cy = self.height * 0.5;
		let Some(centre) = &self.center else { return };
		// Centre card anchored on the canvas centre.
		self.positions.insert(centre.fqdn.clone(), LaidNode {
			x: cx - CENTER_W * 0.5,
			y: cy - CENTER_H * 0.5,
			w: CENTER_W,
			h: CENTER_H,
			is_center: true,
			bucket: None,
		});
		if self.neighbors.is_empty() {
			return;
		}

		// Categorise neighbours into typed buckets based on the edge
		// connecting them to the centre. Multi-edge neighbours collapse
		// to the first edge's bucket. Neighbours with no direct edge to
		// the centre (depth-2+) are skipped entirely in V0 — they have
		// no canonical bucket and would clutter the layout.
		let centre_fqdn = centre.fqdn.clone();
		let mut buckets: std::collections::HashMap<Bucket, Vec<&FocusNode>> =
			std::collections::HashMap::new();
		for n in &self.neighbors {
			let Some(bucket) = self.bucket_for_neighbor(n, &centre_fqdn) else { continue };
			buckets.entry(bucket).or_default().push(n);
		}

		// Lay out each bucket centred on its (angle, radial) placement.
		// Vertical buckets stack items top-to-bottom; horizontal buckets
		// (TestedBy at the bottom) lay them side-by-side. Cap each bucket
		// at BUCKET_TOP_N — the rest collapse into a "+N more" badge.
		// fit_to_content() on set_payload zooms the camera so the full
		// bbox fits the drawer, so the radials can stay generous.
		for (bucket, _label, angle, radial, direction) in BUCKET_PLACEMENTS {
			let items = match buckets.get(bucket) {
				Some(v) if !v.is_empty() => v,
				_ => continue,
			};
			let visible = items.len().min(BUCKET_TOP_N);
			let hidden = items.len().saturating_sub(visible);
			let bucket_cx = cx + radial * angle.cos();
			let bucket_cy = cy + radial * angle.sin();

			match direction {
				BucketDirection::Vertical => {
					let total_h = (visible as f64) * (ITEM_H + ITEM_GAP) - ITEM_GAP;
					let start_y = bucket_cy - total_h * 0.5;
					for (i, n) in items.iter().take(BUCKET_TOP_N).enumerate() {
						let item_y = start_y + (i as f64) * (ITEM_H + ITEM_GAP);
						self.positions.insert(n.fqdn.clone(), LaidNode {
							x: bucket_cx - ITEM_W * 0.5,
							y: item_y,
							w: ITEM_W,
							h: ITEM_H,
							is_center: false,
							bucket: Some(*bucket),
						});
					}
					if hidden > 0 {
						let badge_y = start_y + total_h + ITEM_GAP;
						self.overflow_badges.push((*bucket, bucket_cx, badge_y, hidden));
					}
				}
				BucketDirection::Horizontal => {
					let total_w = (visible as f64) * (ITEM_W + ITEM_GAP_H) - ITEM_GAP_H;
					let start_x = bucket_cx - total_w * 0.5;
					for (i, n) in items.iter().take(BUCKET_TOP_N).enumerate() {
						let item_x = start_x + (i as f64) * (ITEM_W + ITEM_GAP_H);
						self.positions.insert(n.fqdn.clone(), LaidNode {
							x: item_x,
							y: bucket_cy - ITEM_H * 0.5,
							w: ITEM_W,
							h: ITEM_H,
							is_center: false,
							bucket: Some(*bucket),
						});
					}
					if hidden > 0 {
						// Badge to the right of the last item rather than
						// below (the row already takes horizontal space).
						let badge_x = start_x + total_w + ITEM_GAP_H + 48.0;
						self.overflow_badges.push((*bucket, badge_x, bucket_cy - MORE_BADGE_H * 0.5, hidden));
					}
				}
			}
		}
	}

	/// Classify a neighbour into its display bucket from the edge that
	/// connects it to the centre. Direction matters: an inbound CALLS
	/// edge (other → centre) means the centre is *used by* the other;
	/// an outbound CALLS (centre → other) means the centre *calls*
	/// other. Neighbours with no direct edge to the centre (depth-2+
	/// only) land in `Indirect` so 2/3-hop views surface them rather
	/// than dropping them invisibly.
	fn bucket_for_neighbor(&self, n: &FocusNode, centre_fqdn: &str) -> Option<Bucket> {
		for e in &self.edges {
			if e.from == n.fqdn && e.to == centre_fqdn {
				// Inbound: other → centre.
				return Some(match e.kind.as_str() {
					"CALLS" => Bucket::UsedBy,
					"REFERENCES" | "USES_TYPE" => Bucket::UsedBy,
					"IMPORTS" => Bucket::ImportedBy,
					"TESTS" => Bucket::TestedBy,
					"IMPLEMENTS" | "EXTENDS" => Bucket::ImplementsExtends,
					_ => Bucket::UsedBy,
				});
			}
			if e.from == centre_fqdn && e.to == n.fqdn {
				// Outbound: centre → other.
				return Some(match e.kind.as_str() {
					"CALLS" => Bucket::Calls,
					"USES_TYPE" | "REFERENCES" => Bucket::UsesTypes,
					"IMPORTS" => Bucket::Imports,
					"TESTS" => Bucket::TestedBy,
					"IMPLEMENTS" | "EXTENDS" => Bucket::ImplementsExtends,
					_ => Bucket::Calls,
				});
			}
		}
		// No direct edge to the centre — the neighbour is only reachable
		// via depth-2+ paths, surface it in the Indirect bucket so multi-
		// hop views are not silently empty.
		Some(Bucket::Indirect)
	}

	fn draw(&self) {
		// Canvas clear — aligned with the modern shell palette
		// (--sd-bg-deep ≈ #0b0e14) so the focus canvas reads as part
		// of the shell instead of the legacy gray.
		self.ctx.set_fill_style_str("#0b0e14");
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
				0 | 1 => 1.0,
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
			// Clip edge endpoints to card borders, then render as a
			// quadratic Bézier bowing perpendicular by EDGE_BEZIER_BOW.
			// Curves separate parallel edges visually and bend away
			// from intermediate cards on the straight path.
			let (fx, fy) = (from.x + from.w * 0.5, from.y + from.h * 0.5);
			let (tx, ty) = (to.x + to.w * 0.5, to.y + to.h * 0.5);
			let dx = tx - fx;
			let dy = ty - fy;
			let len = dx.hypot(dy).max(1.0);
			let ux = dx / len;
			let uy = dy / len;
			let from_t = rect_edge_t(from.w * 0.5, from.h * 0.5, ux, uy);
			let to_t = rect_edge_t(to.w * 0.5, to.h * 0.5, ux, uy);
			let fx_c = fx + ux * from_t;
			let fy_c = fy + uy * from_t;
			let tx_c = tx - ux * to_t;
			let ty_c = ty - uy * to_t;
			let mid_x = (fx_c + tx_c) * 0.5;
			let mid_y = (fy_c + ty_c) * 0.5;
			let cp_x = mid_x + -uy * EDGE_BEZIER_BOW;
			let cp_y = mid_y + ux * EDGE_BEZIER_BOW;
			// IMPLEMENTS / EXTENDS render as a double-line (two parallel
			// strokes offset perpendicular to the line direction) so they
			// read as "inheritance/contract" visually — a single line
			// would look like any other call/uses edge.
			let is_double = e.kind == "IMPLEMENTS" || e.kind == "EXTENDS";
			if is_double {
				let px = -uy * 2.0;
				let py = ux * 2.0;
				self.ctx.begin_path();
				self.ctx.move_to(fx_c + px, fy_c + py);
				self.ctx.quadratic_curve_to(cp_x + px, cp_y + py, tx_c + px, ty_c + py);
				self.ctx.stroke();
				self.ctx.begin_path();
				self.ctx.move_to(fx_c - px, fy_c - py);
				self.ctx.quadratic_curve_to(cp_x - px, cp_y - py, tx_c - px, ty_c - py);
				self.ctx.stroke();
			} else {
				self.ctx.begin_path();
				self.ctx.move_to(fx_c, fy_c);
				self.ctx.quadratic_curve_to(cp_x, cp_y, tx_c, ty_c);
				self.ctx.stroke();
			}
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

		// Arrowheads — drawn AFTER the cards so they land on top of the
		// destination card border rather than disappearing under the
		// next paint pass. One arrow per edge at the `to` endpoint,
		// coloured to match the edge kind.
		for e in &self.edges {
			let (Some(from), Some(to)) = (self.positions.get(&e.from), self.positions.get(&e.to))
			else { continue };
			let touches_hover = connected
				.as_ref()
				.is_some_and(|set| set.contains(e.from.as_str()) && set.contains(e.to.as_str()));
			let base_alpha: f64 = match e.depth {
				0 | 1 => 0.95,
				2 => 0.5,
				_ => 0.3,
			};
			let alpha = match (&connected, touches_hover) {
				(Some(_), true) => 1.0,
				(Some(_), false) => base_alpha * 0.2,
				(None, _) => base_alpha,
			};
			self.ctx.set_global_alpha(alpha);
			self.ctx.set_fill_style_str(edge_color_for_kind(&e.kind));
			let (fx, fy) = (from.x + from.w * 0.5, from.y + from.h * 0.5);
			let (tx, ty) = (to.x + to.w * 0.5, to.y + to.h * 0.5);
			let dx = tx - fx;
			let dy = ty - fy;
			let len = dx.hypot(dy).max(1.0);
			let ux = dx / len;
			let uy = dy / len;
			// Recompute clipped endpoints + control point so the arrow
			// tangent matches the Bézier curve drawn above. Tangent at
			// t=1 of a quadratic Bézier is (P2 - P1).normalize().
			let from_t = rect_edge_t(from.w * 0.5, from.h * 0.5, ux, uy);
			let to_t = rect_edge_t(to.w * 0.5, to.h * 0.5, ux, uy);
			let fx_c = fx + ux * from_t;
			let fy_c = fy + uy * from_t;
			let tx_c = tx - ux * to_t;
			let ty_c = ty - uy * to_t;
			let mid_x = (fx_c + tx_c) * 0.5;
			let mid_y = (fy_c + ty_c) * 0.5;
			let cp_x = mid_x + -uy * EDGE_BEZIER_BOW;
			let cp_y = mid_y + ux * EDGE_BEZIER_BOW;
			let tan_dx = tx_c - cp_x;
			let tan_dy = ty_c - cp_y;
			let tan_len = tan_dx.hypot(tan_dy).max(1.0);
			let tan_ux = tan_dx / tan_len;
			let tan_uy = tan_dy / tan_len;
			let tip_x = tx_c + tan_ux * 2.0;
			let tip_y = ty_c + tan_uy * 2.0;
			let base_x = tip_x - tan_ux * ARROW_LEN;
			let base_y = tip_y - tan_uy * ARROW_LEN;
			let perp_x = -tan_uy * ARROW_HALF_WIDTH;
			let perp_y = tan_ux * ARROW_HALF_WIDTH;
			self.ctx.begin_path();
			self.ctx.move_to(tip_x, tip_y);
			self.ctx.line_to(base_x + perp_x, base_y + perp_y);
			self.ctx.line_to(base_x - perp_x, base_y - perp_y);
			self.ctx.close_path();
			self.ctx.fill();
		}
		self.ctx.set_global_alpha(1.0);

		// Overflow badges sit under each truncated bucket column. They
		// surface that the bucket has more items than the cap shows
		// without forcing the layout to render them inline.
		for (_bucket, x, y, count) in &self.overflow_badges {
			let badge_w = 96.0;
			let badge_x = x - badge_w * 0.5;
			self.ctx.set_fill_style_str("#252526");
			rounded_rect_path(&self.ctx, badge_x, *y, badge_w, MORE_BADGE_H, MORE_BADGE_H * 0.5);
			self.ctx.fill();
			self.ctx.set_stroke_style_str("#3d3d3d");
			self.ctx.set_line_width(1.0);
			rounded_rect_path(&self.ctx, badge_x, *y, badge_w, MORE_BADGE_H, MORE_BADGE_H * 0.5);
			self.ctx.stroke();
			self.ctx.set_fill_style_str("#9d9d9d");
			self.ctx.set_font("600 10px ui-monospace, SFMono-Regular, monospace");
			self.ctx.set_text_align("center");
			self.ctx.set_text_baseline("middle");
			let _ = self.ctx.fill_text(&format!("+{count} more"), *x, *y + MORE_BADGE_H * 0.5);
		}

		self.ctx.restore();
	}

	fn draw_node(&self, pos: &LaidNode, node: &FocusNode, is_center: bool, highlighted: bool) {
		let kind = parse_kind(node.kind.as_deref());
		let kind_color = kind_color_hex(kind);
		// Kind-driven border colour: the perimeter doubles as a kind
		// badge. Bucket categorical info is conveyed by position
		// (manifesto W=callers / E=deps / N=imports / S=tests).
		let border_color = kind_color;
		// Drop shadow under every card — gives the layout real depth
		// vs the flat VSCode-panel feel the V0 had. Larger soft shadow
		// for the centre to anchor it as the focal point.
		let shadow_off = if is_center { 6.0 } else { 4.0 };
		let shadow_alpha = if is_center { 0.6 } else { 0.45 };
		self.ctx.set_fill_style_str(&format!("rgba(0, 0, 0, {shadow_alpha:.3})"));
		rounded_rect_path(&self.ctx, pos.x + shadow_off * 0.4, pos.y + shadow_off, pos.w, pos.h, CARD_RADIUS);
		self.ctx.fill();
		// Soft halo behind the centre / hovered card for visual anchor.
		if is_center || highlighted {
			let halo_color = if is_center { border_color } else { "rgba(74, 158, 255, 1.0)" };
			self.ctx.set_fill_style_str(&format!("{halo_color}33")); // ~20% alpha via hex appended
			rounded_rect_path(&self.ctx, pos.x - 8.0, pos.y - 8.0, pos.w + 16.0, pos.h + 16.0, CARD_RADIUS + 6.0);
			self.ctx.fill();
		}
		// Card body — gradient on centre, flat dark on neighbours.
		// Tinted to the modern shell palette (cool blue-noir vs the
		// legacy neutral grey) so cards harmonise with --sd-panel-bg.
		if is_center {
			let g = self.ctx.create_linear_gradient(pos.x, pos.y, pos.x, pos.y + pos.h);
			let _ = g.add_color_stop(0.0, "#1d2433");
			let _ = g.add_color_stop(1.0, "#13182a");
			self.ctx.set_fill_style_canvas_gradient(&g);
		} else {
			self.ctx.set_fill_style_str("#161b25");
		}
		rounded_rect_path(&self.ctx, pos.x, pos.y, pos.w, pos.h, CARD_RADIUS);
		self.ctx.fill();
		self.ctx.set_stroke_style_str(border_color);
		self.ctx.set_line_width(if is_center { 2.5 } else { 1.8 });
		rounded_rect_path(&self.ctx, pos.x, pos.y, pos.w, pos.h, CARD_RADIUS);
		self.ctx.stroke();

		// Text content. Centre gets 3 lines (name / kind / footer);
		// neighbour cards get 2 (name / kindLabel).
		self.ctx.set_text_align("left");
		self.ctx.set_text_baseline("top");
		let pad_x = pos.x + 10.0;
		if is_center {
			self.ctx.set_fill_style_str("#ffffff");
			self.ctx.set_font("600 14px ui-monospace, SFMono-Regular, monospace");
			let _ = self.ctx.fill_text(&node.name, pad_x, pos.y + 10.0);
			self.ctx.set_fill_style_str("#9d9d9d");
			self.ctx.set_font("11px ui-monospace, SFMono-Regular, monospace");
			let kind_label = node.kind.clone().unwrap_or_else(|| "symbol".to_string());
			let _ = self.ctx.fill_text(&kind_label, pad_x, pos.y + 30.0);
			let footer = format_center_footer(node.field_count, node.method_count);
			if !footer.is_empty() {
				self.ctx.set_fill_style_str("#cccccc");
				self.ctx.set_font("11px ui-monospace, SFMono-Regular, monospace");
				let _ = self.ctx.fill_text(&footer, pad_x, pos.y + 50.0);
			}
		} else {
			// Three-line neighbour card: name (bold), kind (muted), file:line
			// (small muted) — matches the manifesto's "mini semantic record"
			// per node so the graph reads without hover.
			self.ctx.set_fill_style_str("#e3e3e3");
			self.ctx.set_font("12px ui-monospace, SFMono-Regular, monospace");
			let _ = self.ctx.fill_text(&node.name, pad_x, pos.y + 4.0);
			self.ctx.set_fill_style_str("#8a8d96");
			self.ctx.set_font("10px ui-monospace, SFMono-Regular, monospace");
			let kind_label = node.kind.clone().unwrap_or_default();
			if !kind_label.is_empty() {
				let _ = self.ctx.fill_text(&kind_label, pad_x, pos.y + 17.0);
			}
			// 3rd line: file basename + line number, only when both are
			// present and the card has room. Truncate the file basename
			// so it fits — readers care about the relative location, not
			// the full path which is already in the inspector panel.
			if let (Some(f), Some(line)) = (&node.file, node.start_line) {
				let basename = f.rsplit(|c: char| c == '/' || c == '\\').next().unwrap_or(f);
				let max_chars = 22;
				let trimmed = if basename.len() > max_chars {
					format!("…{}", &basename[basename.len() - (max_chars - 1)..])
				} else {
					basename.to_string()
				};
				self.ctx.set_fill_style_str("#6e7079");
				self.ctx.set_font("9px ui-monospace, SFMono-Regular, monospace");
				let _ = self.ctx.fill_text(&format!("{trimmed}:{line}"), pad_x, pos.y + 28.0);
			}
		}
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
		let pad = HIT_PAD / self.cam_zoom;
		for (fqdn, p) in &self.positions {
			let inside = world_x >= p.x - pad
				&& world_x <= p.x + p.w + pad
				&& world_y >= p.y - pad
				&& world_y <= p.y + p.h + pad;
			if inside {
				return Some(fqdn.clone());
			}
		}
		None
	}
}

/// Trace a rounded-rect path on the canvas context. Used by all card
/// surfaces (centre, neighbours, halos, accent stripes). Independent
/// of the browser's CanvasPath2D `roundRect` so we don't depend on the
/// 2023+ Web API surface.
fn rounded_rect_path(ctx: &CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64, r: f64) {
	let r = r.min(w * 0.5).min(h * 0.5);
	ctx.begin_path();
	ctx.move_to(x + r, y);
	ctx.line_to(x + w - r, y);
	let _ = ctx.arc_to(x + w, y, x + w, y + r, r);
	ctx.line_to(x + w, y + h - r);
	let _ = ctx.arc_to(x + w, y + h, x + w - r, y + h, r);
	ctx.line_to(x + r, y + h);
	let _ = ctx.arc_to(x, y + h, x, y + h - r, r);
	ctx.line_to(x, y + r);
	let _ = ctx.arc_to(x, y, x + r, y, r);
	ctx.close_path();
}

/// Parametric distance from the centre of a rectangle (half-extents
/// `hw`/`hh`) to its border along the unit direction `(ux, uy)`. The
/// border point is `(cx + ux*t, cy + uy*t)` where `t` is the returned
/// value. Used to clip edge endpoints to card borders so lines don't
/// cross transparent / dimmed cards.
fn rect_edge_t(hw: f64, hh: f64, ux: f64, uy: f64) -> f64 {
	let t_x = hw / ux.abs().max(0.001);
	let t_y = hh / uy.abs().max(0.001);
	t_x.min(t_y)
}

fn format_center_footer(field_count: Option<u32>, method_count: Option<u32>) -> String {
	let f = field_count.unwrap_or(0);
	let m = method_count.unwrap_or(0);
	match (f, m) {
		(0, 0) => String::new(),
		(_, 0) => format!("{f} field{}", if f == 1 { "" } else { "s" }),
		(0, _) => format!("{m} method{}", if m == 1 { "" } else { "s" }),
		(_, _) => format!(
			"{f} field{} · {m} method{}",
			if f == 1 { "" } else { "s" },
			if m == 1 { "" } else { "s" },
		),
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

/// Human-readable edge label, lower-case so the graph reads as a
/// sentence ("reader::read_chunk uses type Const") instead of shouted
/// constants ("USES_TYPE"). Mirrors the manifesto's "lue comme une
/// phrase" principle.
fn edge_phrase(kind: &str) -> &'static str {
	match kind {
		"CALLS" => "calls",
		"IMPORTS" => "imports",
		"USES_TYPE" => "uses type",
		"IMPLEMENTS" => "implements",
		"EXTENDS" => "extends",
		"TESTS" => "tests",
		"REFERENCES" => "references",
		_ => "links",
	}
}

