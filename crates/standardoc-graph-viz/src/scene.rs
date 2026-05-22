//! Laid-out scene: every node has a world-space rectangle, edges
//! reference nodes by index. Built from a [`GraphPayload`] in one pass
//! by the [`crate::layout`] module.
//!
//! `Node::visibility` / `language_kind` / `owner_index` are stored
//! today even though the renderer ignores them — the upcoming filter
//! chips (kind / visibility / language) read them, as does the focal
//! mode for the within-cluster anchoring.

#![allow(dead_code)]

use std::collections::HashMap;

use web_sys::CanvasRenderingContext2d;

use crate::hierarchy::Hierarchy;
use crate::kind::Kind;
use crate::layout;
use crate::payload::{EdgeEntry, GraphPayload};
use crate::render::truncate_to_width;

#[derive(Debug, Default)]
pub(crate) struct Scene {
    pub hierarchy: Hierarchy,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// fqdn → index in `nodes`. Built once, used both by hit-testing
    /// and by edge resolution. Hot path on every pointer-move so we
    /// keep it as a `HashMap` rather than a linear scan.
    pub node_by_fqdn: HashMap<String, usize>,
    /// Cached AABB over all nodes — used by the viewport's `fit_to`.
    pub bounds: Bounds,
    /// `(foundation, dependant)` pairs of `hierarchy.nodes` indices —
    /// the aggregated frame-tier dependencies the renderer draws as
    /// persistent wires.
    pub frame_edges: Vec<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub fqdn: String,
    pub name: String,
    pub kind: Kind,
    pub visibility: String,
    pub language_kind: String,
    /// Broad source language (`rust` / `typescript` / `c` / `lua` /
    /// …) — the serde-lowercased IR `Language`. Drives the chip's
    /// left accent bar via `Palette::language_color`.
    pub language: String,
    pub is_external: bool,
    /// Index into `Scene.hierarchy.nodes` of the container node that
    /// owns this chip. Usually a leaf, but can be an intermediate
    /// node when a module path is both a terminal (own items) and a
    /// parent (sub-modules) — e.g. `std::io` carries `Read` AND
    /// parents `std::io::BufReader`. `u32` keeps the per-node
    /// back-pointer compact at scale (vs. `String` full-paths which
    /// blow up to ~100 MB on 500 k-symbol workspaces).
    pub owner_index: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Pre-truncated chip label, computed once after layout via
    /// `Scene::prepare_labels`. Empty until that pass runs; the
    /// renderer falls back to `name` when empty so a forgotten
    /// `prepare_labels` call still draws (just less compact).
    pub display_name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Edge {
    pub from_node: usize,
    pub to_node: usize,
    pub kind: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Bounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Bounds {
    pub(crate) const EMPTY: Self = Self {
        min_x: f64::INFINITY,
        min_y: f64::INFINITY,
        max_x: f64::NEG_INFINITY,
        max_y: f64::NEG_INFINITY,
    };

    pub(crate) fn width(self) -> f64 {
        self.max_x - self.min_x
    }

    pub(crate) fn height(self) -> f64 {
        self.max_y - self.min_y
    }

    pub(crate) fn is_valid(self) -> bool {
        self.max_x.is_finite()
            && self.min_x.is_finite()
            && self.max_y.is_finite()
            && self.min_y.is_finite()
            && self.max_x >= self.min_x
            && self.max_y >= self.min_y
    }

    pub(crate) fn extend_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if x < self.min_x {
            self.min_x = x;
        }
        if y < self.min_y {
            self.min_y = y;
        }
        if x + w > self.max_x {
            self.max_x = x + w;
        }
        if y + h > self.max_y {
            self.max_y = y + h;
        }
    }
}

impl Default for Bounds {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Scene {
    pub(crate) fn from_payload(payload: GraphPayload) -> Self {
        let (hierarchy, nodes, frame_edges) =
            layout::pack(payload.symbols, payload.projects, &payload.edges);

        let mut node_by_fqdn = HashMap::with_capacity(nodes.len());
        let mut bounds = Bounds::EMPTY;
        for (idx, n) in nodes.iter().enumerate() {
            node_by_fqdn.insert(n.fqdn.clone(), idx);
        }
        // Bounds is the union of all roots. Each root container
        // transitively encloses every chip beneath it, so the union
        // of roots is the world AABB.
        for &r in &hierarchy.roots {
            let n = &hierarchy.nodes[r as usize];
            bounds.extend_rect(n.x as f64, n.y as f64, n.w as f64, n.h as f64);
        }

        let edges = resolve_edges(payload.edges, &node_by_fqdn);

        Self {
            hierarchy,
            nodes,
            edges,
            node_by_fqdn,
            bounds,
            frame_edges,
        }
    }

    /// Replace the edge set without touching nodes, hierarchy, or the
    /// `node_by_fqdn` index. Called from `GraphEngine::set_edges` so
    /// the caller can stream in lazily-fetched edges (e.g. via hover)
    /// without resetting the viewport.
    pub(crate) fn replace_edges(&mut self, raw: Vec<EdgeEntry>) {
        self.edges = resolve_edges(raw, &self.node_by_fqdn);
    }

    /// One-shot pass that runs `measure_text`-driven truncation for
    /// every node and leaf title. Without this, the renderer would
    /// call `ctx.measure_text` thousands of times PER FRAME — the
    /// dominant cost for medium-sized graphs (>1 k symbols). Called
    /// from `GraphEngine::load_graph` after `Scene::from_payload`.
    /// Idempotent: re-running it with the same font + ctx is a no-op
    /// in terms of output, just recomputes the same strings.
    pub(crate) fn prepare_labels(&mut self, ctx: &CanvasRenderingContext2d) {
        // Chip name truncation. CHIP_W and chip padding from
        // `layout.rs` are duplicated as constants here to keep `scene`
        // free of a circular dependency on the layout module's
        // internals; if those change, bump these in lock-step (small
        // surface, two consts).
        const CHIP_TEXT_MAX_WIDTH: f64 = 200.0 - 50.0;
        ctx.set_font("12px system-ui, sans-serif");
        for n in &mut self.nodes {
            n.display_name = truncate_to_width(ctx, &n.name, CHIP_TEXT_MAX_WIDTH);
        }

        // Header title (segment) and subtitle (full path), truncated
        // for every node — leaves AND intermediates. The subtitle is
        // skipped at draw time when it equals the title (root nodes
        // whose only segment IS the full path).
        ctx.set_font("600 12px system-ui, sans-serif");
        for idx in 0..self.hierarchy.nodes.len() {
            let title = {
                let n = &self.hierarchy.nodes[idx];
                truncate_to_width(ctx, &n.segment, n.w as f64 - 60.0)
            };
            self.hierarchy.nodes[idx].display_title = title;
        }
        ctx.set_font("10px system-ui, sans-serif");
        for idx in 0..self.hierarchy.nodes.len() {
            let i = idx as u32;
            let full = self.hierarchy.full_path(i);
            let n = &self.hierarchy.nodes[idx];
            let subtitle = if full == n.segment {
                String::new()
            } else {
                truncate_to_width(ctx, &full, n.w as f64 - 24.0)
            };
            self.hierarchy.nodes[idx].display_subtitle = subtitle;
        }
    }

    pub(crate) fn hit_test(&self, world_x: f64, world_y: f64) -> Option<String> {
        // Linear scan over nodes. Sufficient for the prototype scale
        // (~10k nodes worst case). Spatial index (quadtree) lands
        // with Round 6 when 100k+ workspaces become a real target.
        for n in &self.nodes {
            if world_x >= n.x && world_x <= n.x + n.w && world_y >= n.y && world_y <= n.y + n.h {
                return Some(n.fqdn.clone());
            }
        }
        None
    }

    pub(crate) fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// Bounds of the deepest hierarchy frame whose rectangle contains
    /// the world point, or `None` when the point hits no frame. Drives
    /// double-click zoom-to-fit navigation.
    pub(crate) fn frame_bounds_at(&self, world_x: f64, world_y: f64) -> Option<Bounds> {
        self.hierarchy
            .nodes
            .iter()
            .filter(|n| {
                let (x, y) = (n.x as f64, n.y as f64);
                world_x >= x
                    && world_x <= x + n.w as f64
                    && world_y >= y
                    && world_y <= y + n.h as f64
            })
            .max_by_key(|n| n.depth)
            .map(|n| Bounds {
                min_x: n.x as f64,
                min_y: n.y as f64,
                max_x: (n.x + n.w) as f64,
                max_y: (n.y + n.h) as f64,
            })
    }

    /// Bounds of the hierarchy frame at arena index `id`, or `None`
    /// when `id` is out of range. Drives breadcrumb `fit_to_frame`.
    pub(crate) fn frame_bounds(&self, id: u32) -> Option<Bounds> {
        self.hierarchy.nodes.get(id as usize).map(|n| Bounds {
            min_x: n.x as f64,
            min_y: n.y as f64,
            max_x: (n.x + n.w) as f64,
            max_y: (n.y + n.h) as f64,
        })
    }

    /// Breadcrumb trail (root → deepest) of frames whose rectangle
    /// fully contains the world-space viewbox. Empty when the viewbox
    /// is larger than every frame (full overview). Each entry is the
    /// frame's `(segment, arena_index)`.
    pub(crate) fn focus_path(&self, vx0: f64, vy0: f64, vx1: f64, vy1: f64) -> Vec<(String, u32)> {
        let deepest = self
            .hierarchy
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                (n.x as f64) <= vx0
                    && (n.y as f64) <= vy0
                    && (n.x + n.w) as f64 >= vx1
                    && (n.y + n.h) as f64 >= vy1
            })
            .max_by_key(|(_, n)| n.depth)
            .map(|(i, _)| i as u32);
        let Some(mut cur) = deepest else {
            return Vec::new();
        };
        let mut path: Vec<(String, u32)> = Vec::new();
        loop {
            let n = &self.hierarchy.nodes[cur as usize];
            path.push((n.segment.clone(), cur));
            match n.parent {
                Some(p) => cur = p,
                None => break,
            }
        }
        path.reverse();
        path
    }

    pub(crate) fn symbol_count(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

fn resolve_edges(raw: Vec<EdgeEntry>, by_fqdn: &HashMap<String, usize>) -> Vec<Edge> {
    raw.into_iter()
        .filter_map(|e| {
            let from = *by_fqdn.get(&e.from)?;
            let to = *by_fqdn.get(&e.to)?;
            Some(Edge {
                from_node: from,
                to_node: to,
                kind: e.kind,
            })
        })
        .collect()
}
