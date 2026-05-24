//! Renderable snapshot of the **current drill level**.
//!
//! A `Scene` holds the children of the focused tree node laid out as
//! cards, plus the cross-link edges aggregated to this altitude. It is
//! rebuilt from scratch every time the drill focus moves (descend /
//! ascend / focus_to) — there is no workspace-wide layout cache. The
//! single source of truth for hierarchy is [`crate::tree::DrillTree`];
//! `Scene` is a *dumb projection* the 2D renderer consumes.

#![allow(dead_code)]

use std::collections::HashMap;

use web_sys::CanvasRenderingContext2d;

use crate::kind::Kind;
use crate::layout;
use crate::payload::EdgeEntry;
use crate::render::truncate_to_width;
use crate::tree::DrillTree;

#[derive(Debug, Default)]
pub(crate) struct Scene {
    pub cards: Vec<Card>,
    pub edges: Vec<Edge>,
    /// `tree.nodes` index → index in `cards`. Built once per level
    /// rebuild; used by hit-testing and by `replace_edges` to map
    /// hover-fetched edges back to visible cards.
    pub card_by_tree_idx: HashMap<u32, usize>,
    /// fqdn → index in `cards`. Only populated for leaf cards (with
    /// a non-empty `fqdn`). Used by `replace_edges` to resolve a
    /// hover-fetched edge set against the current level.
    pub card_by_fqdn: HashMap<String, usize>,
    /// Cached AABB over all cards — used by `Viewport::fit_to`.
    pub bounds: Bounds,
}

/// One renderable card in the current drill level. Each card stands
/// for one child of the focused tree node. Container cards (with
/// children of their own) descend on click; leaf cards (no children)
/// fire the node-click callback with their `fqdn`.
#[derive(Debug, Clone)]
pub(crate) struct Card {
    /// Back-pointer into `DrillTree.nodes`. Stable across renders for
    /// the same workspace — the renderer round-trips this through
    /// hover / focus callbacks rather than relying on positional
    /// indices that shift with every layout.
    pub tree_idx: u32,
    /// Display name (project label or symbol name).
    pub label: String,
    /// Broad language tag (`rust` / `typescript` / `bun` / …) used by
    /// the palette to colour the card header.
    pub language: String,
    /// Source-symbol FQDN — empty for synthetic project cards. The
    /// node-click callback fires with this value for leaf cards.
    pub fqdn: String,
    /// Symbol kind — drives the per-card header colour and the
    /// per-Kind grouping the level layout applies (Modules first,
    /// then Types / Functions / Values / Macros / Unknown).
    /// `Unknown` for project cards.
    pub kind: Kind,
    pub is_container: bool,
    /// Number of nodes in this subtree (excluding self). Drives the
    /// card's intrinsic size — projects with more symbols read larger.
    pub descendant_count: u32,
    /// `true` for ghost cards materialised from `tree.cross_edges()`
    /// — siblings of the focused node that the focused subtree
    /// couples to. Drawn dashed + semi-transparent so the eye reads
    /// them as "context, not current level". Clicking one refocuses.
    pub is_ghost: bool,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Pre-truncated label, computed once by `prepare_labels`. Empty
    /// until that pass runs; the renderer falls back to `label` so a
    /// forgotten call still draws.
    pub display_label: String,
    /// Phase 3 (Flow) entry-point tag, mirrored from `TreeNode`. The
    /// 2D renderer paints a coloured halo behind cards where this is
    /// `Some(_)` so program roots / public-API surfaces / FFI exports
    /// pop visually. `None` for synthetic project / virtual-module
    /// cards and for any internal symbol.
    pub entry_point: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Edge {
    pub from_card: usize,
    pub to_card: usize,
    /// Aggregated edges (from `tree.level_edges()`) carry an empty
    /// kind — they sum every cross-link kind between two subtrees.
    /// Hover-specific edges (from `replace_edges`) keep the original
    /// kind (`CALLS` / `IMPORTS` / …) so the renderer can colour them.
    pub kind: String,
    /// Phase 3 (Flow) 3.4 — count of distinct underlying symbol→symbol
    /// cross-links collapsed into this edge. `1` for hover-specific or
    /// ghost cross-edges (those are already per-link); `≥1` for
    /// `level_edges()` aggregates. 2D renderer maps to `set_line_width`,
    /// 3D maps to alpha intensity (wgpu line topology can't vary width
    /// per segment).
    pub weight: u32,
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
    /// Build a fresh scene for the tree's current drill focus. Lays
    /// the focused node's children out as cards (grid sized by
    /// subtree weight) and aggregates the cross-link edges to this
    /// altitude via [`DrillTree::level_edges`]. On top of that,
    /// materialises **ghost cards** for every sibling of the focused
    /// node that the focused subtree couples to (via
    /// [`DrillTree::cross_edges`]); ghosts are positioned in a ring
    /// around the primary grid so the user sees the cross-project /
    /// cross-module relations the previous version dropped.
    pub(crate) fn from_level(tree: &DrillTree, ctx: &CanvasRenderingContext2d) -> Self {
        let (mut cards, mut bounds) = layout::layout_level(tree);
        let primary_count = cards.len();

        // Build the primary `tree_idx → card_index` lookup before any
        // ghost cards land — `tree.level_edges` references primaries
        // only, and ghost cross-edges resolve their inside endpoint
        // through this same map.
        let mut card_by_tree_idx: HashMap<u32, usize> = HashMap::with_capacity(primary_count);
        for (i, c) in cards.iter().enumerate() {
            card_by_tree_idx.insert(c.tree_idx, i);
        }

        let mut edges: Vec<Edge> = tree
            .level_edges()
            .into_iter()
            .map(|(a, b, w)| Edge {
                from_card: a as usize,
                to_card: b as usize,
                kind: String::new(),
                weight: w,
            })
            .collect();

        // Ghost cards — one per distinct sibling of the focused node
        // that the focused subtree couples to. Stays empty at the
        // root level (root has no siblings) and on leaf focus.
        for (inside_tree_idx, sibling_tree_idx) in tree.cross_edges() {
            let Some(&inside_card) = card_by_tree_idx.get(&inside_tree_idx) else {
                continue;
            };
            let ghost_card = match card_by_tree_idx.get(&sibling_tree_idx).copied() {
                Some(idx) => idx,
                None => {
                    let node = tree.node(sibling_tree_idx);
                    let idx = cards.len();
                    cards.push(Card {
                        tree_idx: sibling_tree_idx,
                        label: node.label.clone(),
                        language: node.language.clone(),
                        fqdn: node.fqdn.clone(),
                        kind: node.kind,
                        is_container: tree.is_container(sibling_tree_idx),
                        descendant_count: node.descendant_count,
                        is_ghost: true,
                        x: 0.0,
                        y: 0.0,
                        w: 0.0,
                        h: 0.0,
                        display_label: String::new(),
                        entry_point: node.entry_point.clone(),
                    });
                    card_by_tree_idx.insert(sibling_tree_idx, idx);
                    idx
                }
            };
            edges.push(Edge {
                from_card: inside_card,
                to_card: ghost_card,
                kind: String::new(),
                weight: 1,
            });
        }

        // Place ghosts in a ring around the primary grid, expanding
        // `bounds` to cover them so `Viewport::fit_to` frames the
        // full context (primaries + ghosts) at once.
        layout::place_ghosts(&mut cards, primary_count, &mut bounds);

        // fqdn lookup (for `replace_edges` hover-edge resolution) —
        // populated after ghosts so a sibling project's fqdn (empty
        // for projects, so this row is a no-op) doesn't accidentally
        // hijack the primary lookup.
        let mut card_by_fqdn: HashMap<String, usize> = HashMap::with_capacity(cards.len());
        for (i, c) in cards.iter().enumerate() {
            if !c.fqdn.is_empty() && !c.is_ghost {
                card_by_fqdn.insert(c.fqdn.clone(), i);
            }
        }

        let mut scene = Self {
            cards,
            edges,
            card_by_tree_idx,
            card_by_fqdn,
            bounds,
        };
        scene.prepare_labels(ctx);
        scene
    }

    /// Replace the drawn edge set with a hover-fetched neighborhood.
    /// Endpoints not currently visible as leaf cards (no matching
    /// `fqdn` at the current level) are dropped silently — a future
    /// follow-up could surface them as off-card ghost markers.
    pub(crate) fn replace_edges(&mut self, raw: Vec<EdgeEntry>) {
        self.edges = raw
            .into_iter()
            .filter_map(|e| {
                let from = *self.card_by_fqdn.get(e.from.as_str())?;
                let to = *self.card_by_fqdn.get(e.to.as_str())?;
                if from == to {
                    return None;
                }
                Some(Edge {
                    from_card: from,
                    to_card: to,
                    kind: e.kind,
                    weight: 1,
                })
            })
            .collect();
    }

    /// One-shot truncation pass — pre-computes `display_label` for
    /// every card so the render hot path never calls `measure_text`.
    /// Called from `from_level` after layout completes; safe to
    /// re-run after a palette/theme change (idempotent for the same
    /// ctx font).
    pub(crate) fn prepare_labels(&mut self, ctx: &CanvasRenderingContext2d) {
        ctx.set_font("600 14px system-ui, sans-serif");
        for c in &mut self.cards {
            let max_w = (c.w - 24.0).max(20.0);
            c.display_label = truncate_to_width(ctx, &c.label, max_w);
        }
    }

    /// Pick the card under a world-space coordinate, or `None` for a
    /// hit on the void. Linear scan — at the current-level only, so
    /// card counts stay bounded (~k entries, not workspace-wide).
    pub(crate) fn hit_test(&self, world_x: f64, world_y: f64) -> Option<usize> {
        for (i, c) in self.cards.iter().enumerate() {
            if world_x >= c.x
                && world_x <= c.x + c.w
                && world_y >= c.y
                && world_y <= c.y + c.h
            {
                return Some(i);
            }
        }
        None
    }

    pub(crate) fn bounds(&self) -> Bounds {
        self.bounds
    }

    pub(crate) fn symbol_count(&self) -> usize {
        self.cards.len()
    }

    pub(crate) fn edge_count(&self) -> usize {
        self.edges.len()
    }
}
