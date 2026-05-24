//! Grid layout for the current drill level.
//!
//! The 2D Blueprint view shows **only the focused node's direct
//! children** as cards — never the workspace-wide tree. Pick a single
//! card size per level (driven by the biggest subtree's weight, so a
//! crate with 1k symbols reads larger than a singleton) and pack the
//! children into a near-square grid. Edges and interactions are
//! handled elsewhere — this module sizes and positions, nothing more.
//!
//! The previous workspace-wide cluster-pack (Phase A.2 nested
//! frames + sections + chip grids) is gone: deconstruction is now
//! progressive via drill navigation, not visual via section
//! expansion. The whole pipeline (`pack` → `compute_intrinsic_sizes`
//! → `layered_arrange` → `position_layout`) was retired with the
//! cards-only refactor.

use crate::kind::Kind;
use crate::scene::{Bounds, Card};
use crate::tree::DrillTree;

/// Minimum card width — keeps a sparse level (1–2 cards) from
/// degenerating to a sliver.
pub(crate) const CARD_MIN_W: f64 = 180.0;
/// Minimum card height. Cards default to a ~3:2 landscape ratio so
/// the label / count badge has horizontal room.
pub(crate) const CARD_MIN_H: f64 = 120.0;
/// Horizontal space between cards in the grid.
const CARD_GUTTER_X: f64 = 36.0;
/// Vertical space between rows in the grid.
const CARD_GUTTER_Y: f64 = 36.0;
/// Base card width for an empty (leaf) subtree.
const CARD_BASE_W: f64 = 200.0;
/// Logarithmic growth factor on `descendant_count` — keeps very
/// large subtrees readable without blowing the layout out.
const CARD_GROWTH: f64 = 28.0;

/// Lay the focused level's children out as a grid of cards, grouped
/// by `Kind` (Modules → Types → Functions → Values → Macros →
/// Unknown). Each kind starts on a fresh row so the visual rhythm
/// signals the section boundary; within a kind, cards are sorted
/// alphabetically by label for stability. Returns the cards + the
/// AABB enclosing them. Empty when the focused node has no children.
pub(crate) fn layout_level(tree: &DrillTree) -> (Vec<Card>, Bounds) {
    let level = tree.current_level();
    let n = level.len();
    if n == 0 {
        return (Vec::new(), Bounds::EMPTY);
    }

    // Near-square grid; biased to widen rather than tall when n is
    // not a perfect square, since canvases are typically landscape.
    let cols = (n as f64).sqrt().ceil() as usize;

    // Uniform card size for the whole level — picked from the
    // biggest subtree's weight so the visual emphasis still tracks
    // subtree size, but the grid stays clean.
    let max_descendants = level
        .iter()
        .map(|&i| tree.node(i).descendant_count)
        .max()
        .unwrap_or(0);
    let (card_w, card_h) = card_size_for_weight(max_descendants);

    // Sort indices by (kind section order, label alpha). The level
    // slice itself is borrowed from the tree and must not be mutated;
    // we permute through an index vector instead.
    let mut order: Vec<u32> = level.to_vec();
    order.sort_by(|&a, &b| {
        let na = tree.node(a);
        let nb = tree.node(b);
        kind_order(na.kind)
            .cmp(&kind_order(nb.kind))
            .then_with(|| na.label.cmp(&nb.label))
    });

    let mut cards: Vec<Card> = Vec::with_capacity(n);
    let mut bounds = Bounds::EMPTY;
    let mut col = 0_usize;
    let mut row = 0_usize;
    let mut prev_kind: Option<Kind> = None;
    for tree_idx in order {
        let node = tree.node(tree_idx);
        // New kind section ⇒ wrap to the next row (unless we're
        // already at column 0). Skips the wrap for the very first
        // card (prev_kind is None).
        if let Some(pk) = prev_kind {
            if pk != node.kind && col != 0 {
                row += 1;
                col = 0;
            }
        }
        let x = col as f64 * (card_w + CARD_GUTTER_X);
        let y = row as f64 * (card_h + CARD_GUTTER_Y);
        bounds.extend_rect(x, y, card_w, card_h);
        cards.push(Card {
            tree_idx,
            label: node.label.clone(),
            language: node.language.clone(),
            fqdn: node.fqdn.clone(),
            kind: node.kind,
            is_container: tree.is_container(tree_idx),
            descendant_count: node.descendant_count,
            is_ghost: false,
            x,
            y,
            w: card_w,
            h: card_h,
            display_label: String::new(),
            entry_point: node.entry_point.clone(),
        });
        prev_kind = Some(node.kind);
        col += 1;
        if col >= cols {
            col = 0;
            row += 1;
        }
    }
    (cards, bounds)
}

/// Visual ordering of the per-kind sections. Mirrors what most IDEs
/// surface by default — declarations first (modules + types), then
/// behaviour (functions + values + macros), catch-all last.
fn kind_order(kind: Kind) -> u8 {
    match kind {
        Kind::Module => 0,
        Kind::Type => 1,
        Kind::Callable => 2,
        Kind::Value => 3,
        Kind::Macro => 4,
        Kind::Unknown => 5,
    }
}

fn card_size_for_weight(descendant_count: u32) -> (f64, f64) {
    let s = CARD_BASE_W + ((1.0 + descendant_count as f64).ln() * CARD_GROWTH);
    let w = s.max(CARD_MIN_W);
    let h = (s * 0.6).max(CARD_MIN_H);
    (w, h)
}

/// Position ghost cards in a ring around the primary grid's bounding
/// box. The slice `cards[primary_count..]` is assumed to hold the
/// freshly-pushed ghosts (with zero-valued positions) — this fn sizes
/// them, places them around the bbox, and grows `bounds` to cover
/// them so the viewport fits the full context (primaries + ghosts) at
/// once. No-op when there are no ghosts or when the primary bbox is
/// degenerate.
///
/// Ghosts get a uniform size driven by the heaviest ghost's subtree
/// weight (same heuristic as `layout_level`) — keeps the ring tidy
/// even when the focused subtree couples to a mix of large and small
/// crates.
pub(crate) fn place_ghosts(cards: &mut [Card], primary_count: usize, bounds: &mut Bounds) {
    if primary_count >= cards.len() || !bounds.is_valid() {
        return;
    }
    let cx = (bounds.min_x + bounds.max_x) * 0.5;
    let cy = (bounds.min_y + bounds.max_y) * 0.5;
    let half_w = (bounds.max_x - bounds.min_x) * 0.5;
    let half_h = (bounds.max_y - bounds.min_y) * 0.5;
    let base = half_w.max(half_h);

    let max_desc: u32 = cards[primary_count..]
        .iter()
        .map(|c| c.descendant_count)
        .max()
        .unwrap_or(0);
    let (gw, gh) = card_size_for_weight(max_desc);

    // Clearance between the primary bbox and the closest ghost edge.
    let margin = gw.max(gh) * 0.7 + 60.0;
    let radius = base + margin;

    let n = (cards.len() - primary_count) as f64;
    // Start the ring at "north" (-π/2 in screen coordinates so y
    // grows down) so the first ghost lands at the top — feels less
    // arbitrary than a sweep that begins from due-east.
    let start = -std::f64::consts::FRAC_PI_2;
    for (i, ghost) in cards[primary_count..].iter_mut().enumerate() {
        let theta = start + std::f64::consts::TAU * (i as f64) / n;
        let gx = cx + radius * theta.cos() - gw * 0.5;
        let gy = cy + radius * theta.sin() - gh * 0.5;
        ghost.x = gx;
        ghost.y = gy;
        ghost.w = gw;
        ghost.h = gh;
        bounds.extend_rect(gx, gy, gw, gh);
    }
}
