//! Overview canvas — FQDN-hierarchical 3D rendering.
//!
//! Each node is positioned on a Y plane corresponding to its FQDN
//! depth (Y = -depth * DEPTH_Y_STRIDE). Within a plane, children
//! orbit their parent's XZ center via a polar layout. Roots are
//! laid in a sunflower in the XZ plane (Y = 0).
//!
//! Edges come in two kinds:
//!   * `parent_child` — structural FQDN parent → child, always
//!     rendered as thin vertical-ish strands (the spine of the tree)
//!   * `cross` — inter-module imports/calls/uses, rendered as curves
//!     between projected points; hidden by default, toggle via
//!     `set_show_cross_edges`
//!
//! Camera, hit-test, projection: unchanged from the workspace nebula
//! era. Layout + render-edges replaced; the rest is reused.

use glam::{Vec3, Vec4};
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::camera::Camera3D;

/// One node in the FQDN-hierarchical overview. `depth` is the
/// number of `::` segments minus one (`standardoc-core` = 0,
/// `standardoc-core::ir` = 1, …). `parent_id` points to the
/// FQDN-prefix parent in the same payload; `None` only on roots.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OverviewNode {
    pub id: u32,
    pub label: String,
    /// Reserved wire field — `overview-payload.ts` sends the project
    /// ecosystem tag (`rust` / `node` / …) here, but the overview
    /// currently tints nodes by a hash of `label`, not by this. Kept so
    /// the payload contract stays stable until per-kind tinting lands.
    #[serde(default)]
    #[allow(dead_code)]
    pub kind: Option<String>,
    pub symbol_count: u32,
    pub depth: u32,
    #[serde(default)]
    pub parent_id: Option<u32>,
    /// `"module"` (default) or `"public_symbol"`. Public symbols
    /// render slightly smaller so the eye reads "leaf" without
    /// having to look at the label.
    #[serde(default)]
    pub node_kind: Option<String>,
}

/// One edge in the overview. `edge_kind` discriminates structural
/// parent→child relations (always rendered) from cross-module
/// relations (rendered only when `show_cross_edges` is true). `kind`
/// is the IR edge kind (CALLS, IMPORTS, USES_TYPE, IMPLEMENTS, …)
/// for cross edges, used to pick the per-kind color; absent for
/// parent_child structural edges.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OverviewEdge {
    pub from: u32,
    pub to: u32,
    pub weight: u32,
    #[serde(default = "default_edge_kind")]
    pub edge_kind: String,
    #[serde(default)]
    pub kind: Option<String>,
}

fn default_edge_kind() -> String {
    "cross".to_string()
}

/// Color + dashed style per IR edge kind. Mirrors the global legend
/// palette in `lib/components/legend` so the Overview reads in the
/// same visual language as the Focus graph and Explorer chips.
fn cross_edge_style(kind: Option<&str>) -> (&'static str, bool) {
    match kind {
        Some("CALLS") => ("#3794ff", false),
        Some("IMPORTS") => ("#b180d7", false),
        Some("USES_TYPE") => ("#cca700", false),
        Some("IMPLEMENTS") | Some("EXTENDS") => ("#f48771", true),
        Some("REFERENCES") => ("#9d9d9d", false),
        _ => ("#88b4d8", false),
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct OverviewPayload {
    #[serde(default)]
    nodes: Vec<OverviewNode>,
    #[serde(default)]
    edges: Vec<OverviewEdge>,
}

/// World-space node placement. Hue is deterministic per label so
/// reloads keep colors stable.
#[derive(Debug, Clone, Copy)]
struct LaidNode {
    pos: Vec3,
    world_radius: f32,
    hue: f32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
struct ProjectedNode {
    id: u32,
    screen_x: f64,
    screen_y: f64,
    screen_radius: f64,
    depth: f64,
    visible: bool,
}

const MIN_NODE_RADIUS_WORLD: f32 = 36.0;
const MAX_NODE_RADIUS_WORLD: f32 = 180.0;
const PUBLIC_SYMBOL_RADIUS_WORLD: f32 = 22.0;

/// Y distance between two adjacent depth planes. Roots sit at Y=0,
/// depth=1 at Y=-DEPTH_Y_STRIDE, etc.
const DEPTH_Y_STRIDE: f32 = 220.0;

/// Root sunflower scale in the XZ plane. Bigger value = more spread
/// between root packages.
const ROOT_SUNFLOWER_SCALE: f32 = 520.0;
const ROOT_GAP_WORLD: f32 = 420.0;

/// Base radius of the orbit ring around a parent at depth 1. Each
/// subsequent depth shrinks this by `CHILD_RADIUS_DECAY` so deeply
/// nested subtrees cluster tightly around their root.
const CHILD_RADIUS_BASE: f32 = 280.0;
const CHILD_RADIUS_DECAY: f32 = 0.72;

const ZOOM_STEP: f64 = 0.0012;
const CLICK_DRAG_THRESHOLD: f64 = 4.0;
/// Gap in screen pixels between the sphere outer edge and the label
/// pill — small so the label reads as "belonging" to the sphere.
const LABEL_OFFSET: f64 = 5.0;
const HIT_PAD: f64 = 6.0;
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653_3;

#[derive(Debug, Clone, Copy)]
struct DragState {
    last_x: f64,
    last_y: f64,
    moved: bool,
    pan_mode: bool,
}

#[wasm_bindgen]
pub struct OverviewCanvas {
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    width: f64,
    height: f64,
    device_pixel_ratio: f64,
    nodes: Vec<OverviewNode>,
    edges: Vec<OverviewEdge>,
    on_cluster_click: Option<js_sys::Function>,
    on_cluster_hover: Option<js_sys::Function>,
    needs_redraw: bool,
    positions: std::collections::HashMap<u32, LaidNode>,
    render_order: Vec<u32>,
    hovered: Option<u32>,
    drag: Option<DragState>,
    cam: Camera3D,
    max_visible_labels: u32,
    /// Max FQDN depth at which labels are rendered. `u32::MAX` = no
    /// cap (every visible node may be labelled, subject to
    /// `max_visible_labels`). At workspace scope the host clamps
    /// this to `0` so only root-package labels paint — sub-modules
    /// stay as halos until the user drills in and the spatial
    /// crowding drops.
    label_depth_cap: u32,
    /// When false, only `parent_child` edges render. Cross edges
    /// (imports/calls/uses-type between non-parent siblings) are
    /// hidden by default so the spine of the FQDN tree reads
    /// without overlay clutter. Host toggles via the gizmo.
    show_cross_edges: bool,
}

#[wasm_bindgen]
impl OverviewCanvas {
    #[wasm_bindgen(constructor)]
    pub fn new(
        canvas: HtmlCanvasElement,
        width: u32,
        height: u32,
        dpr: f64,
    ) -> Result<OverviewCanvas, JsValue> {
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
            nodes: Vec::new(),
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
            label_depth_cap: u32::MAX,
            show_cross_edges: false,
        })
    }

    pub fn set_max_visible_labels(&mut self, n: u32) {
        if self.max_visible_labels == n {
            return;
        }
        self.max_visible_labels = n;
        self.needs_redraw = true;
    }

    /// Cap label rendering to nodes whose FQDN depth is `<= cap`.
    /// Pass `u32::MAX` (or just a very large value) to disable the
    /// cap. `0` = root-package labels only.
    pub fn set_label_depth_cap(&mut self, cap: u32) {
        if self.label_depth_cap == cap {
            return;
        }
        self.label_depth_cap = cap;
        self.needs_redraw = true;
    }

    /// Toggle cross-edge rendering. Parent-child edges always render.
    pub fn set_show_cross_edges(&mut self, show: bool) {
        if self.show_cross_edges == show {
            return;
        }
        self.show_cross_edges = show;
        self.needs_redraw = true;
    }

    pub fn set_payload(&mut self, json: &str) -> Result<(), JsValue> {
        let parsed: OverviewPayload = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("OverviewCanvas: payload parse error: {e}")))?;
        self.nodes = parsed.nodes;
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
            if drag.pan_mode {
                self.cam.pan(dx, dy, self.height);
            } else {
                self.cam.orbit(dx, dy);
            }
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

    pub fn on_pointer_down(&mut self, x: f64, y: f64, _button: i16, pan_mode: bool) {
        self.drag = Some(DragState {
            last_x: x,
            last_y: y,
            moved: false,
            pan_mode,
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

    pub fn on_wheel(&mut self, _x: f64, _y: f64, delta_y: f64) {
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
    pub fn camera_yaw(&self) -> f32 {
        self.cam.yaw
    }

    #[wasm_bindgen(getter)]
    pub fn camera_pitch(&self) -> f32 {
        self.cam.pitch
    }

    pub fn orbit_camera(&mut self, dx: f64, dy: f64) {
        self.cam.orbit(dx, dy);
        self.needs_redraw = true;
    }

    pub fn pan_camera(&mut self, dx: f64, dy: f64) {
        self.cam.pan(dx, dy, self.height);
        self.needs_redraw = true;
    }

    pub fn dolly_camera(&mut self, factor: f64) {
        self.cam.dolly(factor as f32);
        self.needs_redraw = true;
    }

    #[wasm_bindgen(getter)]
    pub fn cluster_count(&self) -> usize {
        self.nodes.len()
    }

    #[wasm_bindgen(getter)]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    fn layout(&mut self) {
        self.positions.clear();
        self.render_order.clear();
        if self.nodes.is_empty() {
            return;
        }

        // Bucket nodes by parent_id; parent_id == None marks roots.
        let mut children_of: std::collections::HashMap<Option<u32>, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            children_of.entry(n.parent_id).or_default().push(i);
        }

        // Sort roots by symbol_count desc, id tiebreaker — heaviest
        // crate anchors the sunflower center.
        if let Some(roots) = children_of.get_mut(&None) {
            roots.sort_by(|&a, &b| {
                let na = &self.nodes[a];
                let nb = &self.nodes[b];
                nb.symbol_count
                    .cmp(&na.symbol_count)
                    .then(na.id.cmp(&nb.id))
            });
        }

        let max_count = self
            .nodes
            .iter()
            .map(|n| n.symbol_count.max(1))
            .max()
            .unwrap_or(1) as f32;

        // Place roots in a sunflower in XZ plane (Y = 0).
        let root_indices = children_of.get(&None).cloned().unwrap_or_default();
        for (i, &root_idx) in root_indices.iter().enumerate() {
            let pos = if i == 0 {
                Vec3::ZERO
            } else {
                let angle = (i as f64) * GOLDEN_ANGLE;
                let radial = ROOT_SUNFLOWER_SCALE * (i as f32).sqrt() + ROOT_GAP_WORLD;
                Vec3::new(
                    radial * angle.cos() as f32,
                    0.0,
                    radial * angle.sin() as f32,
                )
            };
            self.place_node(root_idx, pos, max_count, &children_of);
            // Recurse children with parent pos as anchor.
            self.place_subtree(root_idx, pos, max_count, &children_of);
        }

        // Render order seed — `draw()` re-sorts this by projected depth
        // every frame, so the set just needs to carry every node id; an
        // up-front depth sort here would only be overwritten.
        self.render_order = self.nodes.iter().map(|n| n.id).collect();
    }

    fn place_node(
        &mut self,
        idx: usize,
        pos: Vec3,
        max_count: f32,
        _children_of: &std::collections::HashMap<Option<u32>, Vec<usize>>,
    ) {
        let n = &self.nodes[idx];
        let is_leaf_symbol = n.node_kind.as_deref() == Some("public_symbol");
        let world_r = if is_leaf_symbol {
            PUBLIC_SYMBOL_RADIUS_WORLD
        } else {
            let raw = (n.symbol_count.max(1) as f32).sqrt();
            let normalised = raw / max_count.sqrt().max(1.0);
            MIN_NODE_RADIUS_WORLD + normalised * (MAX_NODE_RADIUS_WORLD - MIN_NODE_RADIUS_WORLD)
        };
        let hue = label_hue(&n.label);
        self.positions.insert(
            n.id,
            LaidNode {
                pos,
                world_radius: world_r,
                hue,
            },
        );
    }

    fn place_subtree(
        &mut self,
        parent_idx: usize,
        parent_pos: Vec3,
        max_count: f32,
        children_of: &std::collections::HashMap<Option<u32>, Vec<usize>>,
    ) {
        let parent_id = self.nodes[parent_idx].id;
        let Some(child_indices) = children_of.get(&Some(parent_id)) else {
            return;
        };
        if child_indices.is_empty() {
            return;
        }
        // Stable child order: by symbol_count desc, id tiebreaker.
        let mut ordered = child_indices.clone();
        ordered.sort_by(|&a, &b| {
            let na = &self.nodes[a];
            let nb = &self.nodes[b];
            nb.symbol_count
                .cmp(&na.symbol_count)
                .then(na.id.cmp(&nb.id))
        });
        let n = ordered.len() as f32;
        // Polar around parent. Depth-dependent radius so deep
        // subtrees stay close to their root.
        let depth_at_children = self.nodes[ordered[0]].depth.max(1);
        let radius = CHILD_RADIUS_BASE * CHILD_RADIUS_DECAY.powi((depth_at_children - 1) as i32);
        // Phase shift per parent so siblings of different subtrees
        // don't perfectly align (visual breathing room).
        let phase = ((parent_id as f64) * GOLDEN_ANGLE) % std::f64::consts::TAU;
        for (i, &child_idx) in ordered.iter().enumerate() {
            let angle = phase + (i as f64) * std::f64::consts::TAU / (n as f64);
            let child_depth = self.nodes[child_idx].depth as f32;
            let pos = Vec3::new(
                parent_pos.x + radius * angle.cos() as f32,
                -child_depth * DEPTH_Y_STRIDE,
                parent_pos.z + radius * angle.sin() as f32,
            );
            self.place_node(child_idx, pos, max_count, children_of);
            self.place_subtree(child_idx, pos, max_count, children_of);
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
        // Background vignette + starfield, unchanged from V1.
        if let Ok(bg) = self.ctx.create_radial_gradient(
            self.width * 0.5,
            self.height * 0.5,
            0.0,
            self.width * 0.5,
            self.height * 0.5,
            (self.width.max(self.height)) * 0.8,
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
        self.draw_starfield();

        if self.nodes.is_empty() {
            self.draw_empty_state();
            return;
        }

        let projected = self.project_nodes();

        // Pass 1: parent_child spine — thin dim verticals, drawn
        // BEFORE node halos so they ground each subtree.
        for e in &self.edges {
            if e.edge_kind != "parent_child" {
                continue;
            }
            let (Some(from), Some(to)) = (projected.get(&e.from), projected.get(&e.to)) else {
                continue;
            };
            if !from.visible || !to.visible {
                continue;
            }
            self.ctx.set_stroke_style_str("rgba(120, 140, 180, 0.32)");
            self.ctx.set_line_width(1.0);
            self.ctx.begin_path();
            self.ctx.move_to(from.screen_x, from.screen_y);
            self.ctx.line_to(to.screen_x, to.screen_y);
            self.ctx.stroke();
        }

        // Pass 2: cross-edges underlay (wide soft blur), only when
        // the host has toggled them on. Color tracks IR edge kind so
        // CALLS / IMPORTS / USES_TYPE / IMPLEMENTS each read distinct;
        // IMPLEMENTS / EXTENDS render dashed to mirror the focus graph.
        // Hovered-node edges glow brighter, unrelated edges fade so
        // the user can trace the hovered node's relations at a glance.
        if self.show_cross_edges {
            let max_weight = self
                .edges
                .iter()
                .filter(|e| e.edge_kind == "cross")
                .map(|e| e.weight.max(1))
                .max()
                .unwrap_or(1) as f64;
            for e in &self.edges {
                if e.edge_kind != "cross" {
                    continue;
                }
                let (Some(from), Some(to)) = (projected.get(&e.from), projected.get(&e.to)) else {
                    continue;
                };
                if !from.visible || !to.visible {
                    continue;
                }
                let w_norm = (f64::from(e.weight.max(1))).ln() / (max_weight.ln().max(1.0));
                let connected = self.hovered.is_some_and(|h| h == e.from || h == e.to);
                let dimmed = self.hovered.is_some() && !connected;
                let line_w = 2.0 + w_norm * 3.0;
                let base_alpha = 0.10 + w_norm * 0.08;
                let alpha = if connected {
                    base_alpha * 1.8
                } else if dimmed {
                    base_alpha * 0.25
                } else {
                    base_alpha
                };
                let (color, dashed) = cross_edge_style(e.kind.as_deref());
                let rgba = hex_to_rgba(color, alpha);
                self.ctx.set_stroke_style_str(&rgba);
                self.ctx.set_line_width(line_w);
                if dashed {
                    let arr = js_sys::Array::of2(&8.0.into(), &6.0.into());
                    let _ = self.ctx.set_line_dash(&arr);
                } else {
                    let arr = js_sys::Array::new();
                    let _ = self.ctx.set_line_dash(&arr);
                }
                self.ctx.begin_path();
                self.ctx.move_to(from.screen_x, from.screen_y);
                self.ctx.line_to(to.screen_x, to.screen_y);
                self.ctx.stroke();
            }
            let arr = js_sys::Array::new();
            let _ = self.ctx.set_line_dash(&arr);
        }

        // Depth-sort nodes back-to-front (no z-buffer in Canvas2D).
        let mut order: Vec<u32> = self.render_order.clone();
        order.sort_by(|a, b| {
            let da = projected.get(a).map_or(f64::INFINITY, |p| -p.depth);
            let db = projected.get(b).map_or(f64::INFINITY, |p| -p.depth);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Pick which nodes render their text label.
        let label_visible_ids: std::collections::HashSet<u32> = if self.max_visible_labels > 0 {
            let mut by_depth: Vec<(u32, f64)> = projected
                .iter()
                .filter_map(|(id, p)| {
                    if p.visible {
                        Some((*id, p.depth))
                    } else {
                        None
                    }
                })
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

        // O(1) id → node lookup so the per-node loop stays linear; a
        // `self.nodes.iter().find()` here is O(n) per node → O(n²) over
        // the full render, quadratic at workspace scale.
        let node_by_id: std::collections::HashMap<u32, &OverviewNode> =
            self.nodes.iter().map(|n| (n.id, n)).collect();
        for id in &order {
            let Some(proj) = projected.get(id) else {
                continue;
            };
            if !proj.visible {
                continue;
            }
            let Some(n) = node_by_id.get(id).copied() else {
                continue;
            };
            let highlighted = self.hovered == Some(*id);
            let depth_ok = n.depth <= self.label_depth_cap;
            let show_label = depth_ok && (label_visible_ids.contains(id) || highlighted);
            self.draw_node(proj, n, highlighted, show_label);
        }

        // Pass 3: cross-edges overlay (bright narrow strand) — ON
        // TOP of nodes so they remain visible across halos.
        if self.show_cross_edges {
            let max_weight = self
                .edges
                .iter()
                .filter(|e| e.edge_kind == "cross")
                .map(|e| e.weight.max(1))
                .max()
                .unwrap_or(1) as f64;
            for e in &self.edges {
                if e.edge_kind != "cross" {
                    continue;
                }
                let (Some(from), Some(to)) = (projected.get(&e.from), projected.get(&e.to)) else {
                    continue;
                };
                if !from.visible || !to.visible {
                    continue;
                }
                let w_norm = (f64::from(e.weight.max(1))).ln() / (max_weight.ln().max(1.0));
                let connected = self.hovered.is_some_and(|h| h == e.from || h == e.to);
                let dimmed = self.hovered.is_some() && !connected;
                let line_w = if connected {
                    (0.9 + w_norm * 1.0) * 1.6
                } else {
                    0.9 + w_norm * 1.0
                };
                let base_alpha = 0.55 + w_norm * 0.25;
                let alpha = if connected {
                    (base_alpha * 1.3).min(0.95)
                } else if dimmed {
                    base_alpha * 0.25
                } else {
                    base_alpha
                };
                let (color, dashed) = cross_edge_style(e.kind.as_deref());
                let rgba = hex_to_rgba(color, alpha);
                self.ctx.set_stroke_style_str(&rgba);
                self.ctx.set_line_width(line_w);
                if dashed {
                    let arr = js_sys::Array::of2(&8.0.into(), &6.0.into());
                    let _ = self.ctx.set_line_dash(&arr);
                } else {
                    let arr = js_sys::Array::new();
                    let _ = self.ctx.set_line_dash(&arr);
                }
                self.ctx.begin_path();
                self.ctx.move_to(from.screen_x, from.screen_y);
                self.ctx.line_to(to.screen_x, to.screen_y);
                self.ctx.stroke();
            }
            let arr = js_sys::Array::new();
            let _ = self.ctx.set_line_dash(&arr);
        }
    }

    fn draw_starfield(&self) {
        let mut s: u32 = 0x9e37_79b9;
        for _ in 0..120 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let rx = (s >> 8) as f64 / (u32::MAX >> 8) as f64;
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let ry = (s >> 8) as f64 / (u32::MAX >> 8) as f64;
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let bright_pick = (s % 10) as f64 / 10.0;
            let alpha = if bright_pick > 0.85 {
                0.55
            } else {
                0.18 + bright_pick * 0.12
            };
            let size = if bright_pick > 0.92 { 1.6 } else { 0.7 };
            self.ctx
                .set_fill_style_str(&format!("rgba(220, 230, 255, {alpha:.3})"));
            self.ctx.begin_path();
            let _ = self.ctx.arc(
                rx * self.width,
                ry * self.height,
                size,
                0.0,
                std::f64::consts::TAU,
            );
            self.ctx.fill();
        }
    }

    fn project_nodes(&self) -> std::collections::HashMap<u32, ProjectedNode> {
        let mut out: std::collections::HashMap<u32, ProjectedNode> =
            std::collections::HashMap::new();
        let view = self.cam.view();
        let aspect = (self.width / self.height.max(1.0)) as f32;
        let proj = self.cam.proj(aspect);
        let vp = proj * view;
        let eye = self.cam.eye();
        let focal_pixels = (self.height * 0.5) / (self.cam.fov_y as f64 * 0.5).tan();
        for (id, n) in &self.positions {
            let clip = vp * Vec4::new(n.pos.x, n.pos.y, n.pos.z, 1.0);
            if clip.w <= 0.0 {
                out.insert(
                    *id,
                    ProjectedNode {
                        id: *id,
                        screen_x: 0.0,
                        screen_y: 0.0,
                        screen_radius: 0.0,
                        depth: f64::MAX,
                        visible: false,
                    },
                );
                continue;
            }
            let ndc_x = clip.x / clip.w;
            let ndc_y = clip.y / clip.w;
            let sx = (ndc_x as f64 * 0.5 + 0.5) * self.width;
            let sy = (ndc_y as f64 * 0.5 + 0.5) * self.height;
            let depth = (n.pos - eye).length() as f64;
            let screen_radius = (n.world_radius as f64) * focal_pixels / depth.max(1.0);
            out.insert(
                *id,
                ProjectedNode {
                    id: *id,
                    screen_x: sx,
                    screen_y: sy,
                    screen_radius,
                    depth,
                    visible: true,
                },
            );
        }
        out
    }

    fn draw_node(
        &self,
        proj: &ProjectedNode,
        node: &OverviewNode,
        highlighted: bool,
        show_label: bool,
    ) {
        let r = proj.screen_radius.max(2.0);
        let Some(laid) = self.positions.get(&node.id) else {
            return;
        };
        let hue = laid.hue;
        let is_leaf_symbol = node.node_kind.as_deref() == Some("public_symbol");

        // Halo — barely-there atmospheric tint at rest, only swells
        // on hover. The dot + core do the heavy lifting for the eye;
        // halos exist to tag the territory, not to glow.
        let halo_outer = if is_leaf_symbol { r + 14.0 } else { r + 28.0 };
        if let Ok(halo) = self.ctx.create_radial_gradient(
            proj.screen_x,
            proj.screen_y,
            r * 0.10,
            proj.screen_x,
            proj.screen_y,
            halo_outer,
        ) {
            let inner_alpha = if highlighted {
                0.18
            } else if is_leaf_symbol {
                0.02
            } else {
                0.03
            };
            let mid_alpha = if highlighted {
                0.07
            } else if is_leaf_symbol {
                0.01
            } else {
                0.015
            };
            let _ =
                halo.add_color_stop(0.0, &format!("hsla({hue:.0}, 42%, 52%, {inner_alpha:.3})"));
            let _ = halo.add_color_stop(0.45, &format!("hsla({hue:.0}, 35%, 42%, {mid_alpha:.3})"));
            let _ = halo.add_color_stop(1.0, &format!("hsla({hue:.0}, 30%, 28%, 0.0)"));
            self.ctx.set_fill_style_canvas_gradient(&halo);
            self.ctx.begin_path();
            let _ = self.ctx.arc(
                proj.screen_x,
                proj.screen_y,
                halo_outer,
                0.0,
                std::f64::consts::TAU,
            );
            self.ctx.fill();
        }

        // Core glow — small bloom around the dot, calm at rest.
        let core_r = (r * 0.40).max(8.0);
        if let Ok(core_glow) = self.ctx.create_radial_gradient(
            proj.screen_x,
            proj.screen_y,
            0.0,
            proj.screen_x,
            proj.screen_y,
            core_r,
        ) {
            let core_inner = if highlighted { 0.45 } else { 0.22 };
            let core_mid = if highlighted { 0.18 } else { 0.08 };
            let _ = core_glow
                .add_color_stop(0.0, &format!("hsla({hue:.0}, 60%, 68%, {core_inner:.3})"));
            let _ =
                core_glow.add_color_stop(0.5, &format!("hsla({hue:.0}, 50%, 55%, {core_mid:.3})"));
            let _ = core_glow.add_color_stop(1.0, &format!("hsla({hue:.0}, 45%, 48%, 0.0)"));
            self.ctx.set_fill_style_canvas_gradient(&core_glow);
            self.ctx.begin_path();
            let _ = self.ctx.arc(
                proj.screen_x,
                proj.screen_y,
                core_r,
                0.0,
                std::f64::consts::TAU,
            );
            self.ctx.fill();
        }

        // Hard centre point.
        let dot_r = if is_leaf_symbol { 2.4 } else { 3.5 };
        self.ctx
            .set_fill_style_str(&format!("hsla({hue:.0}, 70%, 75%, 0.80)"));
        self.ctx.begin_path();
        let _ = self.ctx.arc(
            proj.screen_x,
            proj.screen_y,
            dot_r,
            0.0,
            std::f64::consts::TAU,
        );
        self.ctx.fill();

        if show_label {
            let label_size = if is_leaf_symbol { 11 } else { 14 };
            self.ctx.set_font(&format!(
                "600 {label_size}px ui-monospace, SFMono-Regular, monospace"
            ));
            self.ctx.set_text_align("center");
            self.ctx.set_text_baseline("top");

            // Position the label just under the VISIBLE bright zone
            // (dot + core glow), not under the faint outer halo. The
            // halo extends way beyond core_r but is mostly invisible,
            // so anchoring on `r` makes the label feel detached.
            let label_y = proj.screen_y + core_r + LABEL_OFFSET;
            self.draw_label_pill(&node.label, proj.screen_x, label_y, f64::from(label_size));
            self.ctx
                .set_fill_style_str(if highlighted { "#ffffff" } else { "#e3e3e3" });
            let _ = self.ctx.fill_text(&node.label, proj.screen_x, label_y);

            if !is_leaf_symbol && node.symbol_count > 0 {
                self.ctx
                    .set_font("11px ui-monospace, SFMono-Regular, monospace");
                let count_text = format!("{} symbols", format_count(node.symbol_count));
                let count_y = label_y + f64::from(label_size as u32) + 2.0;
                self.draw_label_pill(&count_text, proj.screen_x, count_y, 11.0);
                self.ctx
                    .set_fill_style_str(&format!("hsla({hue:.0}, 50%, 70%, 0.9)"));
                let _ = self.ctx.fill_text(&count_text, proj.screen_x, count_y);
            }
        }
    }

    /// Dark semi-transparent pill behind a label so the text stays
    /// legible across the parent_child spine + cross-edge strands.
    /// Without this, labels disappear into edge lines at certain
    /// camera angles. Assumes the current `set_font` / `set_text_align`
    /// is what will be used for the upcoming `fill_text` call.
    fn draw_label_pill(&self, text: &str, cx: f64, top_y: f64, font_px: f64) {
        let metrics = match self.ctx.measure_text(text) {
            Ok(m) => m,
            Err(_) => return,
        };
        let w = metrics.width().max(8.0);
        let pad_x = 5.0;
        let pad_y = 2.0;
        let pill_w = w + pad_x * 2.0;
        let pill_h = font_px + pad_y * 2.0;
        let pill_x = cx - pill_w * 0.5;
        let pill_y = top_y - pad_y;
        self.ctx.set_fill_style_str("rgba(8, 9, 14, 0.78)");
        self.ctx.begin_path();
        let _ = self
            .ctx
            .round_rect_with_f64(pill_x, pill_y, pill_w, pill_h, 4.0);
        self.ctx.fill();
    }

    fn draw_empty_state(&self) {
        self.ctx.set_fill_style_str("#666666");
        self.ctx
            .set_font("13px ui-monospace, SFMono-Regular, monospace");
        self.ctx.set_text_align("center");
        self.ctx.set_text_baseline("middle");
        let _ = self.ctx.fill_text(
            "No projects loaded yet.",
            self.width * 0.5,
            self.height * 0.5,
        );
    }

    fn hit_test(&self, screen_x: f64, screen_y: f64) -> Option<u32> {
        let projected = self.project_nodes();
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

/// Convert a `#rrggbb` color literal + alpha [0..1] into an
/// `rgba(R, G, B, A)` CSS string usable by Canvas2D stroke/fill.
fn hex_to_rgba(hex: &str, alpha: f64) -> String {
    let h = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(0);
    format!("rgba({r}, {g}, {b}, {alpha:.3})")
}

fn label_hue(label: &str) -> f32 {
    let mut h: u32 = 5381;
    for b in label.bytes() {
        h = h.wrapping_mul(33).wrapping_add(u32::from(b));
    }
    (h % 360) as f32
}

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
