//! Hierarchical shelf-pack layout (Round 4 / Phase C+B).
//!
//! Each module path is split on `::` and walked into the [`Hierarchy`]
//! arena. Within every owner node (a node carrying its own symbols),
//! chips are partitioned by [`Kind`] into sub-sections — see
//! `SECTIONS_ORDER`. Sections collapse by default when their count
//! exceeds [`SECTION_COLLAPSE_THRESHOLD`] so dense workspaces don't
//! drown the viewport at load time.
//!
//! Bottom-up we compute every node's intrinsic size: chip region
//! (sections stacked vertically), plus children containers shelf-
//! packed below within a fixed target width. Top-down we then
//! position each node relative to its parent's inner origin.
//!
//! The shelf-pack (CSS flex-wrap-like) was chosen over a pure
//! squarified treemap because our chips are **fixed-size** for label
//! readability and a treemap that varies item area cannot preserve
//! that constraint. Shelf-pack exploits the horizontal axis well
//! enough for the "Figma canvas / UE Blueprint" metaphor while
//! keeping the algorithm prévisible and easy to audit.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::hierarchy::{self, Hierarchy, SectionLayout};
use crate::kind::{Kind, SECTIONS_ORDER};
use crate::payload::SymbolEntry;
use crate::scene::Node;

// Chip / container geometry.
const CHIP_W: f32 = 200.0;
const CHIP_H: f32 = 28.0;
const CHIP_HSPACING: f32 = 8.0;
const CHIP_VSPACING: f32 = 6.0;
pub(crate) const CONTAINER_PADDING: f32 = 16.0;
// Two-line header: top line = `segment` (bold), bottom line = full
// module path (description style). The extra ~14 px buys readability
// when the user is zoomed in on a container and wants the full
// `a::b::c` without walking up the parent chain visually.
pub(crate) const CONTAINER_HEADER_H: f32 = 42.0;
const CONTAINER_GUTTER: f32 = 28.0;

// Section geometry: header strip + spacing between successive
// sections inside the same owner.
pub(crate) const SECTION_HEADER_H: f32 = 22.0;
const SECTION_GUTTER: f32 = 6.0;

// World canvas width target before wrapping siblings to the next row.
// 3200 px ≈ 16 chips wide at CHIP_W=200 — gives a horizontal canvas
// the user pans across, instead of stacking everything vertically.
// Same target is reused at every nesting level so layout stays
// prévisible regardless of depth.
const TARGET_WIDTH: f32 = 3200.0;

// Cap on cols inside a single owner's chip grid. Without a cap, an
// owner with 200 chips would stretch ~3000 px wide on one row and
// dominate the layout. With cap=8, that same owner becomes ~1600 px
// wide × 25 rows — still readable and shelf-packable next to peers.
const MAX_LEAF_COLS: usize = 8;

// Auto-collapse threshold: any section whose chip count exceeds this
// loads collapsed by default. The user can toggle interactively
// (Phase B.2 — toggle wiring lands in a follow-up).
const SECTION_COLLAPSE_THRESHOLD: u32 = 20;

pub(crate) fn pack(symbols: Vec<SymbolEntry>) -> (Hierarchy, Vec<Node>) {
    if symbols.is_empty() {
        return (Hierarchy::default(), Vec::new());
    }

    // Group symbols by their module path (alphabetical via BTreeMap
    // for deterministic creation order — that order also seeds the
    // hierarchy arena, which carries through to sibling ordering).
    let mut groups: BTreeMap<String, Vec<SymbolEntry>> = BTreeMap::new();
    for s in symbols {
        let key = s.module.clone().unwrap_or_else(|| "(root)".to_string());
        groups.entry(key).or_default().push(s);
    }

    let paths: Vec<String> = groups.keys().cloned().collect();
    let (mut hierarchy, path_to_idx) = hierarchy::build(paths.iter().map(String::as_str));

    // Attach each group to the terminal node of its path. Note: that
    // terminal can be an **intermediate** node (e.g. `std::io` is a
    // terminal for the `Read` trait AND a parent of `std::io::BufReader`).
    let mut node_symbols: HashMap<u32, Vec<SymbolEntry>> = HashMap::new();
    for (path, group) in groups {
        let idx = *path_to_idx
            .get(&path)
            .expect("hierarchy::build inserted path");
        node_symbols.insert(idx, group);
    }

    // Drop module-kind chips that duplicate a sub-container. Without
    // this, an owner like `std` carries a `module io` chip in its
    // "Modules" section AND has a sub-container `io` drawn below it
    // — the chip is visual noise because the container already
    // surfaces the same module name (with richer affordances).
    for (&owner_idx, group) in node_symbols.iter_mut() {
        let child_names: HashSet<&str> = hierarchy.nodes[owner_idx as usize]
            .children
            .iter()
            .map(|&ci| hierarchy.nodes[ci as usize].segment.as_str())
            .collect();
        if child_names.is_empty() {
            continue;
        }
        // Need owned Strings for the comparison since `retain` borrows
        // the SymbolEntry — collect names into a HashSet<String> to
        // sidestep the lifetime overlap.
        let owned: HashSet<String> = child_names.iter().map(|s| (*s).to_string()).collect();
        group.retain(|s| !(s.kind == Kind::Module && owned.contains(&s.name)));
    }

    // Build per-owner sections AND sort each kind's chips by name so
    // the in-section order is stable. The cols heuristic is computed
    // per owner from its total chip count and reused for every
    // section in that owner — vertical alignment across sections
    // is what keeps the inner area visually tidy.
    let mut owner_cols: HashMap<u32, usize> = HashMap::new();
    for (&owner_idx, group) in node_symbols.iter_mut() {
        group.sort_by(|a, b| {
            // Primary: SECTIONS_ORDER position so the iteration order
            // matches the visual section order (Modules → Type → …).
            // Secondary: alpha by name within a kind.
            a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name))
        });
        let cols = leaf_cols(group.len());
        owner_cols.insert(owner_idx, cols);
        hierarchy.nodes[owner_idx as usize].sections = build_sections(group, cols);
    }
    // Re-order each group so chips are emitted in the iteration order
    // of `SECTIONS_ORDER` (Modules first, etc.), then alpha within a
    // kind. The plain `Kind::cmp` derives PascalCase order which is
    // NOT the visual order — re-sort explicitly.
    for (_, group) in node_symbols.iter_mut() {
        group.sort_by(|a, b| {
            section_order_index(a.kind)
                .cmp(&section_order_index(b.kind))
                .then_with(|| a.name.cmp(&b.name))
        });
    }

    compute_intrinsic_sizes(&mut hierarchy);
    position_layout(&mut hierarchy);

    // Emit Vec<Node> from each owner's final position + per-section
    // chip placement. Chips of collapsed sections are skipped — they
    // don't exist as `Node` until the section is expanded.
    let total: usize = node_symbols.values().map(Vec::len).sum();
    let mut nodes_out: Vec<Node> = Vec::with_capacity(total);
    for idx in 0..hierarchy.nodes.len() {
        let owner_idx = idx as u32;
        let Some(group) = node_symbols.remove(&owner_idx) else {
            continue;
        };
        if group.is_empty() {
            continue;
        }
        let cols = *owner_cols.get(&owner_idx).expect("owner_cols set above");
        let self_x = hierarchy.nodes[idx].x as f64;
        let self_y = hierarchy.nodes[idx].y as f64;
        let chips_origin_x = self_x + CONTAINER_PADDING as f64;
        let chips_origin_y = self_y + CONTAINER_HEADER_H as f64;

        // Walk the group in section order (already sorted above).
        // For each section, place its chips into the grid IF expanded.
        let mut group_iter = group.into_iter().peekable();
        let sections = hierarchy.nodes[idx].sections.clone();
        for section in &sections {
            let mut chip_in_section: usize = 0;
            while let Some(sym) = group_iter.peek() {
                if sym.kind != section.kind {
                    break;
                }
                let sym = group_iter.next().unwrap();
                if !section.expanded {
                    // Section collapsed — skip emitting this chip
                    // as a Node. It re-emerges if the user toggles
                    // the section open and the engine re-packs.
                    continue;
                }
                let col = chip_in_section % cols;
                let row = chip_in_section / cols;
                let chip_x = chips_origin_x + col as f64 * (CHIP_W as f64 + CHIP_HSPACING as f64);
                let chip_y = chips_origin_y
                    + section.y_offset as f64
                    + SECTION_HEADER_H as f64
                    + row as f64 * (CHIP_H as f64 + CHIP_VSPACING as f64);
                let node_idx = nodes_out.len() as u32;
                hierarchy.nodes[idx].symbol_indices.push(node_idx);
                nodes_out.push(Node {
                    fqdn: sym.fqdn,
                    name: sym.name,
                    kind: sym.kind,
                    visibility: sym.visibility,
                    language_kind: sym.language_kind,
                    is_external: sym.is_external,
                    owner_index: owner_idx,
                    x: chip_x,
                    y: chip_y,
                    w: CHIP_W as f64,
                    h: CHIP_H as f64,
                    display_name: String::new(),
                });
                chip_in_section += 1;
            }
        }
    }

    hierarchy::fill_recursive_counts(&mut hierarchy);

    (hierarchy, nodes_out)
}

/// Position of `kind` within `SECTIONS_ORDER`, used as the primary
/// sort key for chips within an owner.
fn section_order_index(kind: Kind) -> usize {
    SECTIONS_ORDER
        .iter()
        .position(|&k| k == kind)
        .unwrap_or(SECTIONS_ORDER.len())
}

/// Build per-Kind sections for one owner. Sections are ordered by
/// `SECTIONS_ORDER`; kinds with zero chips are skipped entirely.
/// Sections whose count exceeds `SECTION_COLLAPSE_THRESHOLD` start
/// collapsed (header only, no chip area).
fn build_sections(group: &[SymbolEntry], cols: usize) -> Vec<SectionLayout> {
    let mut by_kind: HashMap<Kind, u32> = HashMap::new();
    for s in group {
        *by_kind.entry(s.kind).or_insert(0) += 1;
    }

    let mut sections: Vec<SectionLayout> = Vec::new();
    let mut cursor_y: f32 = 0.0;
    let mut first = true;
    for &k in &SECTIONS_ORDER {
        let count = *by_kind.get(&k).unwrap_or(&0);
        if count == 0 {
            continue;
        }
        let expanded = count <= SECTION_COLLAPSE_THRESHOLD;
        let chips_h = if expanded {
            section_chips_height(count as usize, cols)
        } else {
            0.0
        };
        let total_h = SECTION_HEADER_H + chips_h;
        if !first {
            cursor_y += SECTION_GUTTER;
        }
        sections.push(SectionLayout {
            kind: k,
            chip_count: count,
            expanded,
            y_offset: cursor_y,
            total_h,
        });
        cursor_y += total_h;
        first = false;
    }
    sections
}

/// Height of the chip grid for one section, given the owner's
/// `cols` (kept constant across sections in an owner so the grid
/// aligns vertically).
fn section_chips_height(chip_count: usize, cols: usize) -> f32 {
    if chip_count == 0 || cols == 0 {
        return 0.0;
    }
    let rows = (chip_count + cols - 1) / cols;
    rows as f32 * CHIP_H + (rows.saturating_sub(1) as f32) * CHIP_VSPACING
}

/// Number of cols inside an owner's chip grid. Aimed at a near-square
/// aspect ratio, capped by `MAX_LEAF_COLS` so mega-owners (200+
/// chips) stay reasonably wide.
fn leaf_cols(chip_count: usize) -> usize {
    if chip_count == 0 {
        return 1;
    }
    let approx = (chip_count as f64).sqrt().ceil() as usize;
    approx.min(MAX_LEAF_COLS).max(1)
}

/// Total height of the chip region (all sections stacked) for an
/// owner. Reads pre-computed section sizes — no recomputation.
fn chips_region_height(sections: &[SectionLayout]) -> f32 {
    if sections.is_empty() {
        return 0.0;
    }
    // Last section's bottom edge IS the region bottom; sections were
    // laid out by `build_sections` with their own gutters.
    let last = &sections[sections.len() - 1];
    last.y_offset + last.total_h
}

/// Width of the chip region: `cols` chips wide. Returns 0 when the
/// owner has no sections — that lets `compute_intrinsic_sizes` fold
/// the chip-region term out without a special case.
fn chips_region_width(sections: &[SectionLayout], cols: usize) -> f32 {
    if sections.is_empty() {
        return 0.0;
    }
    cols as f32 * CHIP_W + (cols.saturating_sub(1) as f32) * CHIP_HSPACING
}

/// Post-order DFS: every child's intrinsic size is known before its
/// parent is processed. Arena order guarantees parent-before-child
/// (paths are walked depth-first when building), so reverse-iterating
/// the arena is a valid post-order traversal — no explicit stack.
fn compute_intrinsic_sizes(h: &mut Hierarchy) {
    let inner_target = TARGET_WIDTH - CONTAINER_PADDING * 2.0;
    for idx in (0..h.nodes.len()).rev() {
        let sections = h.nodes[idx].sections.clone();
        // Compute chip-region dimensions from the section list.
        // `cols` is derived from the total chip count across sections
        // (= the same heuristic used when sections were built).
        let total_chips: u32 = sections.iter().map(|s| s.chip_count).sum();
        let cols = leaf_cols(total_chips as usize);
        let chips_w = chips_region_width(&sections, cols);
        let chips_h = chips_region_height(&sections);

        let children = h.nodes[idx].children.clone();
        let (kids_w, kids_h) = if children.is_empty() {
            (0.0, 0.0)
        } else {
            shelf_pack_sizes(
                children
                    .iter()
                    .map(|&c| (h.nodes[c as usize].w, h.nodes[c as usize].h)),
                inner_target,
            )
        };

        let inner_w = chips_w.max(kids_w);
        let inner_h = if chips_h > 0.0 && !children.is_empty() {
            chips_h + CONTAINER_GUTTER + kids_h
        } else {
            chips_h + kids_h
        };

        h.nodes[idx].w = inner_w + CONTAINER_PADDING * 2.0;
        h.nodes[idx].h = CONTAINER_HEADER_H + inner_h + CONTAINER_PADDING;
    }
}

/// Shelf-pack a stream of `(w, h)` rectangles within `target_w` and
/// return the resulting bounding box. Each item starts a new row when
/// adding it to the current row would exceed `target_w`. Mirrors what
/// `shelf_pack_positions` does for the position pass — but we don't
/// need the per-item positions during size computation, only the
/// envelope.
fn shelf_pack_sizes<I: Iterator<Item = (f32, f32)>>(items: I, target_w: f32) -> (f32, f32) {
    let mut cursor_x: f32 = 0.0;
    let mut cursor_y: f32 = 0.0;
    let mut row_max_h: f32 = 0.0;
    let mut max_used_w: f32 = 0.0;
    let mut first_in_row = true;
    for (w, h) in items {
        let needed = if first_in_row {
            w
        } else {
            cursor_x + CONTAINER_GUTTER + w
        };
        if !first_in_row && needed > target_w {
            cursor_y += row_max_h + CONTAINER_GUTTER;
            cursor_x = 0.0;
            row_max_h = 0.0;
            first_in_row = true;
        }
        if !first_in_row {
            cursor_x += CONTAINER_GUTTER;
        }
        cursor_x += w;
        if cursor_x > max_used_w {
            max_used_w = cursor_x;
        }
        if h > row_max_h {
            row_max_h = h;
        }
        first_in_row = false;
    }
    (max_used_w, cursor_y + row_max_h)
}

/// Top-down positioning. Roots are shelf-packed at the world origin,
/// then every container's children are positioned relative to it.
/// Arena order (parent-before-child) lets us iterate forward so each
/// container's `(x, y)` is already set by the time we touch its
/// children.
fn position_layout(h: &mut Hierarchy) {
    let roots = h.roots.clone();
    shelf_pack_positions(h, &roots, 0.0, 0.0, TARGET_WIDTH);

    let inner_target = TARGET_WIDTH - CONTAINER_PADDING * 2.0;
    for idx in 0..h.nodes.len() {
        let children = h.nodes[idx].children.clone();
        if children.is_empty() {
            continue;
        }
        let chips_h = chips_region_height(&h.nodes[idx].sections);

        let self_x = h.nodes[idx].x;
        let self_y = h.nodes[idx].y;
        let children_origin_x = self_x + CONTAINER_PADDING;
        let chips_origin_y = self_y + CONTAINER_HEADER_H;
        let children_origin_y = if chips_h > 0.0 {
            chips_origin_y + chips_h + CONTAINER_GUTTER
        } else {
            chips_origin_y
        };

        shelf_pack_positions(
            h,
            &children,
            children_origin_x,
            children_origin_y,
            inner_target,
        );
    }
}

/// Position-writing companion of `shelf_pack_sizes`. Writes `(x, y)`
/// onto each item in `items` (an arena-index slice) as it walks the
/// shelf-pack. Origin is the inner-content corner of the parent.
fn shelf_pack_positions(
    h: &mut Hierarchy,
    items: &[u32],
    origin_x: f32,
    origin_y: f32,
    target_w: f32,
) {
    let mut cursor_x: f32 = 0.0;
    let mut cursor_y: f32 = 0.0;
    let mut row_max_h: f32 = 0.0;
    let mut first_in_row = true;
    for &i in items {
        let (w, hh) = (h.nodes[i as usize].w, h.nodes[i as usize].h);
        let needed = if first_in_row {
            w
        } else {
            cursor_x + CONTAINER_GUTTER + w
        };
        if !first_in_row && needed > target_w {
            cursor_y += row_max_h + CONTAINER_GUTTER;
            cursor_x = 0.0;
            row_max_h = 0.0;
            first_in_row = true;
        }
        if !first_in_row {
            cursor_x += CONTAINER_GUTTER;
        }
        h.nodes[i as usize].x = origin_x + cursor_x;
        h.nodes[i as usize].y = origin_y + cursor_y;
        cursor_x += w;
        if hh > row_max_h {
            row_max_h = hh;
        }
        first_in_row = false;
    }
}
