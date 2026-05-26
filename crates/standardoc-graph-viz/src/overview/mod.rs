//! Phase 3 (Shell) — **Overview canvas**. Workspace-level view: each
//! project rendered as a luminous cluster (nebula-ish), inter-project
//! dependencies as glow strands whose intensity tracks edge weight.
//!
//! Phase 3c upgrade: projects live in 3D world space (deterministic
//! sunflower in the XZ plane, biggest near origin, others spiralling
//! out via the Fibonacci angle). A `Camera3D` orbits the workspace
//! centroid — drag rotates yaw + pitch, wheel dollies in/out, presets
//! re-aim the camera (orbit / top / front / side). Each frame projects
//! cluster centres through view × proj into screen space; clusters
//! depth-sort back-to-front so closer nebulae paint over farther ones.
//!
//! Hit-test re-projects the cluster positions every move so the click
//! target tracks the current camera. Inter-project edges draw as 2D
//! lines between the projected endpoints with weight-driven width.
//!
//! Phase 3d will slim the legacy GraphEngine path — this module stays.

use glam::{Vec3, Vec4};
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::camera::Camera3D;

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

/// World-space cluster placement. `pos` is the cluster centre in world
/// coords; `world_radius` is the cluster's outer sphere radius; `hue`
/// is the deterministic HSL hue used to colour the cluster. The per-
/// symbol "sub_points constellation" of the V0 was removed per the Dark
/// Semantic Observatory manifesto — the Overview shows crates / hubs,
/// not individual symbols, so satellite dots were decorative noise.
#[derive(Debug, Clone, Copy)]
struct LaidCluster {
	pos: Vec3,
	world_radius: f32,
	hue: f32,
}

/// Screen-space projection of a cluster for the current frame, cached
/// per-tick so hit_test + draw read the same numbers.
#[allow(dead_code)] // id retained for debug-print symmetry with positions
#[derive(Debug, Clone, Copy)]
struct ProjectedCluster {
	id: u32,
	screen_x: f64,
	screen_y: f64,
	screen_radius: f64,
	/// Distance from the camera eye in world units — used for depth
	/// sort + alpha-fade on far clusters.
	depth: f64,
	/// True when the cluster is in front of the near plane (visible).
	visible: bool,
}

const MIN_CLUSTER_RADIUS_WORLD: f32 = 70.0;
const MAX_CLUSTER_RADIUS_WORLD: f32 = 220.0;
const CLUSTER_GAP_WORLD: f32 = 380.0;
const SUNFLOWER_SCALE: f32 = 440.0;
/// Vertical world-space range across which clusters spread by
/// popularity. The most-imported cluster sits at `+POPULARITY_Y_RANGE`
/// world units above the XZ plane; the least-imported sits roughly at
/// zero. Gives the topology a third axis — hubs literally float above
/// their consumers — without overloading the sunflower spacing.
const POPULARITY_Y_RANGE: f32 = 320.0;
const ZOOM_STEP: f64 = 0.0012;
const CLICK_DRAG_THRESHOLD: f64 = 4.0;
const LABEL_OFFSET: f64 = 14.0;
const HIT_PAD: f64 = 6.0;
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653_3;
// Sub-points constellation removed (manifesto anti-pattern: "galaxie
// décorative"). If we ever want per-cluster texture, regenerate from
// the IR signal (hotspot indicators, complexity score, etc.) — not
// abstract dot scatter.

#[derive(Debug, Clone, Copy)]
struct DragState {
	last_x: f64,
	last_y: f64,
	moved: bool,
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
	positions: std::collections::HashMap<u32, LaidCluster>,
	render_order: Vec<u32>,
	hovered: Option<u32>,
	drag: Option<DragState>,
	cam: Camera3D,
	/// Cap on the number of cluster text labels rendered each frame.
	/// `0` means "no cap, render all visible". Anything else picks the
	/// N closest clusters to the camera — far-field clusters render
	/// their halo + dot but skip the text so the canvas stays
	/// readable at workspace scale.
	max_visible_labels: u32,
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
			cam: Camera3D::identity(),
			max_visible_labels: 0,
		})
	}

	/// Host-driven label cap. `0` renders every visible cluster's
	/// label; any other value renders text for the N clusters
	/// closest to the camera and skips the rest. Halos + dots stay
	/// visible so the topology still reads; only the text drops.
	pub fn set_max_visible_labels(&mut self, n: u32) {
		if self.max_visible_labels == n {
			return;
		}
		self.max_visible_labels = n;
		self.needs_redraw = true;
	}

	pub fn set_payload(&mut self, json: &str) -> Result<(), JsValue> {
		let parsed: OverviewPayload = serde_json::from_str(json)
			.map_err(|e| JsValue::from_str(&format!("OverviewCanvas: payload parse error: {e}")))?;
		self.clusters = parsed.clusters;
		self.edges = parsed.edges;
		self.layout();
		self.fit_camera();
		self.needs_redraw = true;
		Ok(())
	}

	pub fn tick(&mut self) {
		if self.cam.animating() {
			self.cam.step_animation();
			self.needs_redraw = true;
		}
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
			let dx = x - drag.last_x;
			let dy = y - drag.last_y;
			drag.last_x = x;
			drag.last_y = y;
			if dx.hypot(dy) >= CLICK_DRAG_THRESHOLD {
				drag.moved = true;
			}
			self.cam.orbit(dx, dy);
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
		self.drag = Some(DragState { last_x: x, last_y: y, moved: false });
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

	pub fn on_wheel(&mut self, _x: f64, _y: f64, delta_y: f64) {
		// Exponential dolly. Negative delta_y (scroll up) pulls the eye
		// closer; positive (scroll down) pushes it back.
		let factor = (-delta_y * ZOOM_STEP).exp() as f32;
		self.cam.dolly(factor);
		self.needs_redraw = true;
	}

	pub fn fit(&mut self) {
		self.fit_camera();
		self.needs_redraw = true;
	}

	pub fn set_camera_preset(&mut self, preset: &str) {
		self.cam.apply_preset(preset);
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
		// Sort by symbol_count desc (id tiebreaker) so the heaviest
		// project anchors the centre. Sunflower lays the rest outward
		// in the XZ plane; Y is driven separately by inbound-edge
		// popularity so the topology gets a third axis.
		let mut order: Vec<usize> = (0..self.clusters.len()).collect();
		order.sort_by(|&a, &b| {
			let ca = &self.clusters[a];
			let cb = &self.clusters[b];
			cb.symbol_count.cmp(&ca.symbol_count).then(ca.id.cmp(&cb.id))
		});

		let max_count = self
			.clusters
			.iter()
			.map(|c| c.symbol_count.max(1))
			.max()
			.unwrap_or(1) as f32;

		// Popularity = sum of inbound edge weights. Used for the Y
		// axis: hubs others depend on float above their consumers,
		// leaves sit at the XZ baseline. ln-compressed because the
		// inbound distribution is heavy-tailed (one root crate gets
		// orders of magnitude more inbound than leaves).
		let mut inbound: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
		for e in &self.edges {
			*inbound.entry(e.to).or_insert(0) += e.weight;
		}
		let max_inbound = inbound.values().copied().max().unwrap_or(0).max(1) as f32;
		let max_inbound_ln = (max_inbound + 1.0).ln().max(1.0);

		for (i, &cluster_idx) in order.iter().enumerate() {
			let c = &self.clusters[cluster_idx];
			let raw = (c.symbol_count.max(1) as f32).sqrt();
			let normalised = raw / max_count.sqrt();
			let world_r = MIN_CLUSTER_RADIUS_WORLD
				+ normalised * (MAX_CLUSTER_RADIUS_WORLD - MIN_CLUSTER_RADIUS_WORLD);

			let inbound_w = inbound.get(&c.id).copied().unwrap_or(0) as f32;
			let popularity = (inbound_w + 1.0).ln() / max_inbound_ln;
			// Camera up is -Y, so subtract to float popular clusters
			// upward on screen.
			let y = -popularity * POPULARITY_Y_RANGE;

			let pos = if i == 0 {
				Vec3::new(0.0, y, 0.0)
			} else {
				let angle = (i as f64) * GOLDEN_ANGLE;
				let radial = SUNFLOWER_SCALE * (i as f32).sqrt() + CLUSTER_GAP_WORLD;
				Vec3::new(
					radial * angle.cos() as f32,
					y,
					radial * angle.sin() as f32,
				)
			};

			let hue = label_hue(&c.label);

			self.positions.insert(c.id, LaidCluster {
				pos,
				world_radius: world_r,
				hue,
			});
			self.render_order.push(c.id);
		}
	}

	fn fit_camera(&mut self) {
		if self.positions.is_empty() {
			return;
		}
		let mut centroid = Vec3::ZERO;
		let mut count = 0.0_f32;
		for p in self.positions.values() {
			centroid += p.pos;
			count += 1.0;
		}
		centroid /= count.max(1.0);
		let mut max_dist = 0.0_f32;
		for p in self.positions.values() {
			let d = (p.pos - centroid).length() + p.world_radius;
			if d > max_dist {
				max_dist = d;
			}
		}
		self.cam.frame(centroid, max_dist);
	}

	fn draw(&self) {
		// Spatial vignette: radial gradient from a deep blue centre out
		// to near-black at the corners — gives the canvas a sense of
		// 3D space depth instead of the flat #161616 the V0 used.
		if let Ok(bg) = self.ctx.create_radial_gradient(
			self.width * 0.5, self.height * 0.5, 0.0,
			self.width * 0.5, self.height * 0.5, (self.width.max(self.height)) * 0.8,
		) {
			let _ = bg.add_color_stop(0.0, "#1a1e2a");
			let _ = bg.add_color_stop(0.6, "#10121a");
			let _ = bg.add_color_stop(1.0, "#08090e");
			self.ctx.set_fill_style_canvas_gradient(&bg);
			self.ctx.fill_rect(0.0, 0.0, self.width, self.height);
		} else {
			self.ctx.set_fill_style_str("#10121a");
			self.ctx.fill_rect(0.0, 0.0, self.width, self.height);
		}
		// Subtle starfield — 120 tiny dots at deterministic positions
		// scaled to canvas. The dim alpha keeps them background-only.
		self.draw_starfield();

		if self.clusters.is_empty() {
			self.draw_empty_state();
			return;
		}

		// Project every cluster once for this frame. Hit-test in the
		// next on_pointer_move will re-project — cheap enough at <100
		// clusters, simpler than a stale screen-space cache.
		let projected = self.project_clusters();

		// Inter-cluster edges rendered in TWO PASSES around the cluster
		// stack: a wide blurred backdrop layer first, then narrow bright
		// strands ON TOP of the clusters so edges remain visible across
		// the cluster halos instead of disappearing under them like in
		// the V0. This is the "glow strand" effect from the mockup.
		let max_weight = self
			.edges
			.iter()
			.map(|e| e.weight.max(1))
			.max()
			.unwrap_or(1) as f64;

		// Pass 1: wide soft blur behind clusters. Toned down vs V1 —
		// thinner widths and lower alpha so the "strand" reads
		// without screaming for attention.
		for e in &self.edges {
			let (Some(from), Some(to)) = (projected.get(&e.from), projected.get(&e.to)) else { continue };
			if !from.visible || !to.visible { continue; }
			let w_norm = (f64::from(e.weight.max(1))).ln() / (max_weight.ln().max(1.0));
			let line_w = 2.0 + w_norm * 3.0;
			let alpha = 0.10 + w_norm * 0.08;
			self.ctx.set_stroke_style_str(&format!("rgba(80, 150, 210, {alpha:.3})"));
			self.ctx.set_line_width(line_w);
			self.ctx.begin_path();
			self.ctx.move_to(from.screen_x, from.screen_y);
			self.ctx.line_to(to.screen_x, to.screen_y);
			self.ctx.stroke();
		}

		// Depth-sort clusters back-to-front so closer nebulae layer over
		// farther ones (no z-buffer in Canvas2D).
		let mut order: Vec<u32> = self.render_order.clone();
		order.sort_by(|a, b| {
			let da = projected.get(a).map_or(f64::INFINITY, |p| -p.depth);
			let db = projected.get(b).map_or(f64::INFINITY, |p| -p.depth);
			da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
		});

		// Pick which clusters render their text label. When the host
		// caps it (max_visible_labels > 0) we take the N closest to
		// the camera; the hovered cluster always shows its label even
		// when it would otherwise be culled, since it's the user's
		// current point of attention.
		let label_visible_ids: std::collections::HashSet<u32> = if self.max_visible_labels > 0 {
			let mut by_depth: Vec<(u32, f64)> = projected
				.iter()
				.filter_map(|(id, p)| if p.visible { Some((*id, p.depth)) } else { None })
				.collect();
			by_depth.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
			let mut keep: std::collections::HashSet<u32> = by_depth
				.iter()
				.take(self.max_visible_labels as usize)
				.map(|(id, _)| *id)
				.collect();
			if let Some(h) = self.hovered {
				keep.insert(h);
			}
			keep
		} else {
			projected.keys().copied().collect()
		};

		for id in &order {
			let Some(proj) = projected.get(id) else { continue };
			if !proj.visible {
				continue;
			}
			let Some(c) = self.clusters.iter().find(|c| c.id == *id) else { continue };
			let highlighted = self.hovered == Some(*id);
			let show_label = label_visible_ids.contains(id);
			self.draw_cluster(proj, c, highlighted, show_label);
		}

		// Pass 2: narrower strand on top of clusters. Alpha cut so
		// the bright pass adds shape without flashing — pairs with
		// the toned pass 1 above for a calm beam, not a laser.
		for e in &self.edges {
			let (Some(from), Some(to)) = (projected.get(&e.from), projected.get(&e.to)) else { continue };
			if !from.visible || !to.visible { continue; }
			let w_norm = (f64::from(e.weight.max(1))).ln() / (max_weight.ln().max(1.0));
			let line_w = 0.9 + w_norm * 1.0;
			let alpha = 0.45 + w_norm * 0.20;
			self.ctx.set_stroke_style_str(&format!("rgba(150, 195, 230, {alpha:.3})"));
			self.ctx.set_line_width(line_w);
			self.ctx.begin_path();
			self.ctx.move_to(from.screen_x, from.screen_y);
			self.ctx.line_to(to.screen_x, to.screen_y);
			self.ctx.stroke();
		}
	}

	/// Deterministic starfield backdrop — 120 tiny dim dots distributed
	/// across the canvas via a simple LCG seeded on a fixed constant so
	/// the pattern is stable across frames + reloads. Gives the empty
	/// space between clusters texture instead of plain black.
	fn draw_starfield(&self) {
		let mut s: u32 = 0x9e37_79b9;
		for _ in 0..120 {
			s = s.wrapping_mul(1664525).wrapping_add(1013904223);
			let rx = (s >> 8) as f64 / (u32::MAX >> 8) as f64;
			s = s.wrapping_mul(1664525).wrapping_add(1013904223);
			let ry = (s >> 8) as f64 / (u32::MAX >> 8) as f64;
			s = s.wrapping_mul(1664525).wrapping_add(1013904223);
			let bright_pick = (s % 10) as f64 / 10.0;
			let alpha = if bright_pick > 0.85 { 0.55 } else { 0.18 + bright_pick * 0.12 };
			let size = if bright_pick > 0.92 { 1.6 } else { 0.7 };
			self.ctx.set_fill_style_str(&format!("rgba(220, 230, 255, {alpha:.3})"));
			self.ctx.begin_path();
			let _ = self.ctx.arc(rx * self.width, ry * self.height, size, 0.0, std::f64::consts::TAU);
			self.ctx.fill();
		}
	}

	fn project_clusters(&self) -> std::collections::HashMap<u32, ProjectedCluster> {
		let mut out: std::collections::HashMap<u32, ProjectedCluster> = std::collections::HashMap::new();
		let view = self.cam.view();
		let aspect = (self.width / self.height.max(1.0)) as f32;
		let proj = self.cam.proj(aspect);
		let vp = proj * view;
		let eye = self.cam.eye();
		// focal_pixels = height/2 / tan(fov_y/2) — converts a world
		// radius at a given camera-space depth into a screen-space disc
		// radius. cf. pinhole perspective.
		let focal_pixels = (self.height * 0.5) / (self.cam.fov_y as f64 * 0.5).tan();
		for (id, c) in &self.positions {
			let clip = vp * Vec4::new(c.pos.x, c.pos.y, c.pos.z, 1.0);
			if clip.w <= 0.0 {
				out.insert(*id, ProjectedCluster {
					id: *id,
					screen_x: 0.0,
					screen_y: 0.0,
					screen_radius: 0.0,
					depth: f64::MAX,
					visible: false,
				});
				continue;
			}
			let ndc_x = clip.x / clip.w;
			let ndc_y = clip.y / clip.w;
			// Camera::view() uses up=NEG_Y → ndc Y is already screen-down,
			// no need to flip.
			let sx = (ndc_x as f64 * 0.5 + 0.5) * self.width;
			let sy = (ndc_y as f64 * 0.5 + 0.5) * self.height;
			let depth = (c.pos - eye).length() as f64;
			let screen_radius = (c.world_radius as f64) * focal_pixels / depth.max(1.0);
			out.insert(*id, ProjectedCluster {
				id: *id,
				screen_x: sx,
				screen_y: sy,
				screen_radius,
				depth,
				visible: true,
			});
		}
		out
	}

	fn draw_cluster(&self, proj: &ProjectedCluster, cluster: &OverviewCluster, highlighted: bool, show_label: bool) {
		let r = proj.screen_radius.max(2.0);
		let Some(laid) = self.positions.get(&cluster.id) else { return };
		let hue = laid.hue;

		// Cluster halo — soft territory marker. Saturations + alphas
		// dialed down vs the V1 "fluorescent rainbow blob" the user
		// flagged; the goal is a calm system-topology accent, not a
		// neon glow stick.
		if let Ok(halo) = self.ctx.create_radial_gradient(
			proj.screen_x, proj.screen_y, r * 0.10,
			proj.screen_x, proj.screen_y, r + 60.0,
		) {
			let inner_alpha = if highlighted { 0.42 } else { 0.26 };
			let mid_alpha = if highlighted { 0.22 } else { 0.13 };
			let _ = halo.add_color_stop(0.0, &format!("hsla({hue:.0}, 55%, 62%, {inner_alpha:.3})"));
			let _ = halo.add_color_stop(0.45, &format!("hsla({hue:.0}, 45%, 48%, {mid_alpha:.3})"));
			let _ = halo.add_color_stop(1.0, &format!("hsla({hue:.0}, 40%, 36%, 0.0)"));
			self.ctx.set_fill_style_canvas_gradient(&halo);
			self.ctx.begin_path();
			let _ = self.ctx.arc(proj.screen_x, proj.screen_y, r + 60.0, 0.0, std::f64::consts::TAU);
			self.ctx.fill();
		}

		// Core glow — the cluster's identity dot's halo. Lower
		// saturation/lightness so it reads as a warm node light
		// instead of a screaming highlight.
		if let Ok(core_glow) = self.ctx.create_radial_gradient(
			proj.screen_x, proj.screen_y, 0.0,
			proj.screen_x, proj.screen_y, (r * 0.40).max(12.0),
		) {
			let _ = core_glow.add_color_stop(0.0, &format!("hsla({hue:.0}, 70%, 78%, 0.80)"));
			let _ = core_glow.add_color_stop(0.5, &format!("hsla({hue:.0}, 60%, 62%, 0.40)"));
			let _ = core_glow.add_color_stop(1.0, &format!("hsla({hue:.0}, 55%, 52%, 0.0)"));
			self.ctx.set_fill_style_canvas_gradient(&core_glow);
			self.ctx.begin_path();
			let _ = self.ctx.arc(proj.screen_x, proj.screen_y, (r * 0.40).max(12.0), 0.0, std::f64::consts::TAU);
			self.ctx.fill();
		}

		// Hard centre point — anchor dot stays punchier than the
		// halos so the cluster's exact position never drowns in
		// gradient blur, but capped at 86% lightness vs full white.
		self.ctx.set_fill_style_str(&format!("hsla({hue:.0}, 85%, 86%, 0.95)"));
		self.ctx.begin_path();
		let _ = self.ctx.arc(proj.screen_x, proj.screen_y, 3.5, 0.0, std::f64::consts::TAU);
		self.ctx.fill();

		// Label + count below the constellation. Skipped when the host
		// has capped visible labels and this cluster fell outside the
		// N-closest set — halo+dot still anchor the cluster's
		// position so the topology remains readable.
		if show_label {
			self.ctx.set_fill_style_str(if highlighted { "#ffffff" } else { "#e3e3e3" });
			self.ctx.set_font("600 14px ui-monospace, SFMono-Regular, monospace");
			self.ctx.set_text_align("center");
			self.ctx.set_text_baseline("top");
			let _ = self.ctx.fill_text(&cluster.label, proj.screen_x, proj.screen_y + r + LABEL_OFFSET);

			self.ctx.set_fill_style_str(&format!("hsla({hue:.0}, 50%, 70%, 0.9)"));
			self.ctx.set_font("11px ui-monospace, SFMono-Regular, monospace");
			let _ = self.ctx.fill_text(
				&format!("{} symbols", format_count(cluster.symbol_count)),
				proj.screen_x,
				proj.screen_y + r + LABEL_OFFSET + 18.0,
			);
		}
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
		let projected = self.project_clusters();
		let mut best: Option<(f64, u32)> = None;
		for (id, p) in &projected {
			if !p.visible {
				continue;
			}
			let d = (screen_x - p.screen_x).hypot(screen_y - p.screen_y);
			if d <= p.screen_radius + HIT_PAD {
				if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
					best = Some((d, *id));
				}
			}
		}
		best.map(|(_, id)| id)
	}
}

/// Deterministic HSL hue (0-360°) from a cluster label. DJB-2 hash
/// modulo 360 — cheap, stable across reloads, distributes adjacent
/// crate names to visually distinct hues.
fn label_hue(label: &str) -> f32 {
	let mut h: u32 = 5381;
	for b in label.bytes() {
		h = h.wrapping_mul(33).wrapping_add(u32::from(b));
	}
	(h % 360) as f32
}

/// Compact symbol-count formatter for cluster labels. `2400` → `2.4k`,
/// `1_200_000` → `1.2M`, anything below 1000 prints as-is. Matches the
/// mockup's `compiler  2.4k symbols` reading.
fn format_count(n: u32) -> String {
	if n < 1000 {
		return n.to_string();
	}
	if n < 1_000_000 {
		let v = f64::from(n) / 1000.0;
		if v < 10.0 {
			return format!("{v:.1}k");
		}
		return format!("{v:.0}k");
	}
	let v = f64::from(n) / 1_000_000.0;
	if v < 10.0 {
		format!("{v:.1}M")
	} else {
		format!("{v:.0}M")
	}
}
