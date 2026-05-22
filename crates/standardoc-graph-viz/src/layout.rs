//! Layered dependency layout (Sugiyama-style).
//!
//! Each module path is split on `::` and walked into the [`Hierarchy`]
//! arena. Within every owner node (a node carrying its own symbols),
//! chips are partitioned by [`Kind`] into sub-sections — see
//! `SECTIONS_ORDER`. Sections collapse by default when their count
//! exceeds [`SECTION_COLLAPSE_THRESHOLD`] so dense workspaces don't
//! drown the viewport at load time.
//!
//! Sibling frames — root projects, and the module subtree inside each
//! one — are arranged in dependency COLUMNS: a frame sits one column
//! right of every sibling it depends on, so foundational code lands
//! left and the flow reads left→right (the UE-Blueprint metaphor).
//! Symbol edges are aggregated to sibling-frame dependencies via the
//! lowest-common-ancestor of each edge's endpoints. Within a column
//! frames flow top-down, wrapping to a parallel sub-column past
//! `TARGET_HEIGHT` so a dependency-free layer never becomes one
//! endless vertical strip.
//!
//! Bottom-up we compute every node's intrinsic size (chip region plus
//! the layered children envelope); top-down we then position each
//! node relative to its parent's inner origin. Chips inside a leaf
//! keep their fixed-size kind-section grid.

use std::collections::{HashMap, HashSet};

use crate::hierarchy::{self, Hierarchy, SectionLayout};
use crate::kind::{Kind, SECTIONS_ORDER};
use crate::payload::{EdgeEntry, ProjectEntry, SymbolEntry};
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

// Horizontal gap between dependency columns. Wider than
// `CONTAINER_GUTTER` so the column flow reads clearly and leaves room
// for the dependency wires (layout Stage 3).
const COLUMN_GUTTER: f32 = 160.0;

// Soft height ceiling for one dependency column. A column past this
// wraps its frames into a parallel sub-column, so a layer with no
// internal dependencies never becomes one endless vertical strip.
const TARGET_HEIGHT: f32 = 2600.0;

// Cap on cols inside a single owner's chip grid. Without a cap, an
// owner with 200 chips would stretch ~3000 px wide on one row and
// dominate the layout. With cap=8, that same owner becomes ~1600 px
// wide × 25 rows — still readable and shelf-packable next to peers.
const MAX_LEAF_COLS: usize = 8;

// Auto-collapse threshold: any section whose chip count exceeds this
// loads collapsed by default. The user can toggle interactively
// (Phase B.2 — toggle wiring lands in a follow-up).
const SECTION_COLLAPSE_THRESHOLD: u32 = 20;

pub(crate) fn pack(
    symbols: Vec<SymbolEntry>,
    projects: Vec<ProjectEntry>,
    edges: &[EdgeEntry],
) -> (Hierarchy, Vec<Node>, Vec<(u32, u32)>) {
    if symbols.is_empty() {
        return (Hierarchy::default(), Vec::new(), Vec::new());
    }

    let mut hierarchy = Hierarchy::default();

    // --- Project tier ---------------------------------------------------
    // Frame only the projects that actually own a symbol here. Sorted
    // by `rel_path` so a parent project is always created before any
    // project nested inside it (shorter prefix sorts first).
    let referenced: HashSet<u32> = symbols.iter().filter_map(|s| s.project_id).collect();
    let mut refs: Vec<&ProjectEntry> = projects
        .iter()
        .filter(|p| referenced.contains(&p.project_id))
        .collect();
    refs.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    let mut project_node: HashMap<u32, u32> = HashMap::new();
    for p in &refs {
        let parent = project_parent(&refs, &project_node, p);
        let depth = parent.map_or(0, |pi| hierarchy.nodes[pi as usize].depth + 1);
        let idx = hierarchy.push(p.label.clone(), parent, depth, Some(p.kind.clone()));
        project_node.insert(p.project_id, idx);
    }

    // A symbol is "unscoped" when it has no `project_id` OR names a
    // project absent from the payload's `projects` table (stale data).
    // Such symbols share one synthetic catch-all frame — never the old
    // global `(root)` bucket.
    let needs_unscoped = symbols.iter().any(|s| {
        s.project_id
            .is_none_or(|pid| !project_node.contains_key(&pid))
    });
    let unscoped_node: Option<u32> = needs_unscoped
        .then(|| hierarchy.push("(unscoped)".to_string(), None, 0, None));

    // --- Route every symbol to its owning hierarchy node ----------------
    // Bucket by project frame first so each project's shared module
    // prefix can be computed before its module subtree is built.
    let mut by_frame: HashMap<u32, Vec<SymbolEntry>> = HashMap::new();
    for s in symbols {
        let frame = s
            .project_id
            .and_then(|pid| project_node.get(&pid).copied())
            .or(unscoped_node)
            .expect("needs_unscoped covers every unresolved symbol");
        by_frame.entry(frame).or_default().push(s);
    }

    // Build each project's module subtree under its frame. Per-frame
    // `path_to_idx` maps keep same-named modules in different projects
    // from colliding. Frames processed in arena order for determinism.
    let mut node_symbols: HashMap<u32, Vec<SymbolEntry>> = HashMap::new();
    let mut frames: Vec<u32> = by_frame.keys().copied().collect();
    frames.sort_unstable();
    for frame in frames {
        let group = by_frame.remove(&frame).expect("frame key from by_frame");
        // When every symbol of a project sits under one root module
        // segment, that segment just echoes the project label — strip
        // it so the frame contains `query::graph`, not a redundant
        // `standardoc-core` node wrapping it.
        let shared = shared_module_prefix(&group);
        let mut map: HashMap<String, u32> = HashMap::new();
        for s in group {
            let sub = module_subpath(s.module.as_deref(), shared.as_deref());
            let owner = hierarchy::ensure_path_under(&mut hierarchy, &mut map, frame, &sub);
            node_symbols.entry(owner).or_default().push(s);
        }
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

    // Aggregate symbol edges into frame-tier dependencies. Each edge
    // `u → v` maps to the pair of sibling frames just below the lowest
    // common ancestor of `u`'s and `v`'s owner frames; the `None` key
    // holds the root-tier (cross-project) dependencies.
    let mut sym_frame: HashMap<&str, u32> = HashMap::new();
    for (&frame, group) in &node_symbols {
        for s in group {
            sym_frame.insert(s.fqdn.as_str(), frame);
        }
    }
    // Each dependency pair is `(foundation, dependant)` — the edge
    // target (`e.to`, what the code relies on) is passed to
    // `frame_dep` first so the layered pass places the dependant one
    // column to the RIGHT. Foundations end up left, the flow reads →.
    let mut child_deps: HashMap<Option<u32>, HashSet<(u32, u32)>> = HashMap::new();
    for e in edges {
        let (Some(&from_frame), Some(&to_frame)) =
            (sym_frame.get(e.from.as_str()), sym_frame.get(e.to.as_str()))
        else {
            continue;
        };
        if let Some((parent, foundation, dependant)) =
            frame_dep(&hierarchy, to_frame, from_frame)
        {
            child_deps
                .entry(parent)
                .or_default()
                .insert((foundation, dependant));
        }
    }
    drop(sym_frame);

    compute_intrinsic_sizes(&mut hierarchy, &child_deps);
    position_layout(&mut hierarchy, &child_deps);

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
                    language: sym.language,
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

    // Flatten the per-tier dependency pairs into a single list of
    // `(foundation, dependant)` frame edges — the renderer draws these
    // as persistent dependency wires. Sorted for deterministic draw
    // order.
    let mut frame_edges: Vec<(u32, u32)> =
        child_deps.values().flatten().copied().collect();
    frame_edges.sort_unstable();

    (hierarchy, nodes_out, frame_edges)
}

/// Frame a nested project hangs under: the referenced project with
/// the longest `rel_path` that is a proper ancestor directory of
/// `child.rel_path`. `refs` is sorted by `rel_path`, so every
/// candidate ancestor is already present in `project_node`.
fn project_parent(
    refs: &[&ProjectEntry],
    project_node: &HashMap<u32, u32>,
    child: &ProjectEntry,
) -> Option<u32> {
    let mut best: Option<(usize, u32)> = None;
    for cand in refs {
        let Some(&node) = project_node.get(&cand.project_id) else {
            continue; // not inserted yet — cannot be an ancestor
        };
        if is_ancestor_path(&cand.rel_path, &child.rel_path)
            && best.is_none_or(|(len, _)| cand.rel_path.len() > len)
        {
            best = Some((cand.rel_path.len(), node));
        }
    }
    best.map(|(_, node)| node)
}

/// True when `a` is a proper ancestor directory of `b` (POSIX-style
/// rel paths). `.` is the workspace root — ancestor of everything.
/// The byte after the `a` prefix must be `/` so `crates/foo` is not
/// treated as an ancestor of `crates/foobar`.
fn is_ancestor_path(a: &str, b: &str) -> bool {
    if a == b {
        return false;
    }
    if a == "." {
        return true;
    }
    b.starts_with(a) && b.as_bytes().get(a.len()) == Some(&b'/')
}

/// First `::` segment shared by every module path in `group`, or
/// `None` when the modules disagree (or none carries a module).
fn shared_module_prefix(group: &[SymbolEntry]) -> Option<String> {
    let mut shared: Option<&str> = None;
    for s in group {
        let Some(module) = s.module.as_deref() else {
            continue;
        };
        let first = module.split("::").next().unwrap_or(module);
        match shared {
            None => shared = Some(first),
            Some(prev) if prev != first => return None,
            Some(_) => {}
        }
    }
    shared.map(str::to_string)
}

/// A symbol's module path **relative to its project frame**: the raw
/// `module` with the shared project-root segment stripped. A `None`
/// module — or one equal to the stripped segment — yields `""`, which
/// `ensure_path_under` resolves to the frame node itself.
fn module_subpath(module: Option<&str>, shared: Option<&str>) -> String {
    let Some(module) = module else {
        return String::new();
    };
    match shared {
        Some(sh) if module == sh => String::new(),
        Some(sh)
            if module.len() > sh.len() + 2
                && module.starts_with(sh)
                && module.as_bytes().get(sh.len()) == Some(&b':') =>
        {
            module[sh.len() + 2..].to_string()
        }
        _ => module.to_string(),
    }
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
/// A container's children envelope is the layered-column arrangement
/// (`layered_arrange`), keyed on the parent's dependency set.
fn compute_intrinsic_sizes(
    h: &mut Hierarchy,
    child_deps: &HashMap<Option<u32>, HashSet<(u32, u32)>>,
) {
    let empty: HashSet<(u32, u32)> = HashSet::new();
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
            let items: Vec<(u32, f32, f32)> = children
                .iter()
                .map(|&c| (c, h.nodes[c as usize].w, h.nodes[c as usize].h))
                .collect();
            let deps = child_deps.get(&Some(idx as u32)).unwrap_or(&empty);
            let (_, w, hh) = layered_arrange(&items, deps);
            (w, hh)
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

/// Ancestor chain of `n`, from `n` itself up to its root (inclusive).
fn ancestors(h: &Hierarchy, mut n: u32) -> Vec<u32> {
    let mut chain = vec![n];
    while let Some(p) = h.nodes[n as usize].parent {
        chain.push(p);
        n = p;
    }
    chain
}

/// Map a symbol edge between owner frames `fa` and `fb` to a sibling
/// dependency: the two frames just below their lowest common
/// ancestor. Returns `(lca, child_a, child_b)` — `lca` is `None` when
/// the frames sit in different root trees (a cross-project, root-tier
/// dependency). Returns `None` overall when one frame is an ancestor
/// of the other (no sibling pair) or they are the same frame.
fn frame_dep(h: &Hierarchy, fa: u32, fb: u32) -> Option<(Option<u32>, u32, u32)> {
    if fa == fb {
        return None;
    }
    let mut ra = ancestors(h, fa);
    let mut rb = ancestors(h, fb);
    ra.reverse(); // root → fa
    rb.reverse(); // root → fb
    let mut i = 0;
    while i < ra.len() && i < rb.len() && ra[i] == rb[i] {
        i += 1;
    }
    // A full-prefix match means one frame is an ancestor of the other.
    if i >= ra.len() || i >= rb.len() {
        return None;
    }
    let lca = if i == 0 { None } else { Some(ra[i - 1]) };
    Some((lca, ra[i], rb[i]))
}

/// Arrange a set of sibling frames in dependency COLUMNS. Returns the
/// per-item relative `(idx, x, y)` and the envelope `(w, h)`. A frame
/// sits one column right of every sibling it depends on; within a
/// column frames flow top-down and wrap to a parallel sub-column past
/// `TARGET_HEIGHT`, so a dependency-free layer never becomes one
/// endless vertical strip. Used at every tier — root projects and the
/// module subtree inside each container.
fn layered_arrange(
    items: &[(u32, f32, f32)],
    deps: &HashSet<(u32, u32)>,
) -> (Vec<(u32, f32, f32)>, f32, f32) {
    if items.is_empty() {
        return (Vec::new(), 0.0, 0.0);
    }
    let in_set: HashSet<u32> = items.iter().map(|&(i, _, _)| i).collect();
    let size_of: HashMap<u32, (f32, f32)> =
        items.iter().map(|&(i, w, h)| (i, (w, h))).collect();
    let deps: Vec<(u32, u32)> = deps
        .iter()
        .copied()
        .filter(|(u, v)| u != v && in_set.contains(u) && in_set.contains(v))
        .collect();

    // Layer assignment — iterative longest-path relaxation. Cycle-safe:
    // a dependency cycle just saturates after `items.len()` rounds
    // instead of looping forever.
    let mut layer: HashMap<u32, u32> = items.iter().map(|&(i, _, _)| (i, 0u32)).collect();
    for _ in 0..items.len() {
        let mut changed = false;
        for &(u, v) in &deps {
            let cand = layer[&u] + 1;
            if cand > layer[&v] {
                layer.insert(v, cand);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let max_layer = layer.values().copied().max().unwrap_or(0);
    let mut columns: Vec<Vec<u32>> = vec![Vec::new(); max_layer as usize + 1];
    for &(i, _, _) in items {
        columns[layer[&i] as usize].push(i);
    }

    // Within-column ordering — barycentre of predecessors. Column 0
    // keeps `items` order (stable); each later column sorts by the
    // mean row of its dependency sources in the column to its left.
    let mut row_of: HashMap<u32, f32> = HashMap::new();
    for (col_idx, col) in columns.iter_mut().enumerate() {
        if col_idx > 0 {
            col.sort_by(|&a, &b| {
                bary(a, &deps, &row_of)
                    .partial_cmp(&bary(b, &deps, &row_of))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        for (row, &n) in col.iter().enumerate() {
            row_of.insert(n, row as f32);
        }
    }

    // Position — columns left→right; each column vertical-shelf-packs
    // its frames within `TARGET_HEIGHT`, wrapping to a sub-column.
    let mut out: Vec<(u32, f32, f32)> = Vec::with_capacity(items.len());
    let mut col_x = 0.0_f32;
    let mut env_h = 0.0_f32;
    for col in &columns {
        let mut sub_x = 0.0_f32;
        let mut sub_y = 0.0_f32;
        let mut sub_w = 0.0_f32;
        let mut col_w = 0.0_f32;
        let mut first = true;
        for &n in col {
            let (w, hh) = size_of[&n];
            if !first && sub_y + CONTAINER_GUTTER + hh > TARGET_HEIGHT {
                sub_x += sub_w + CONTAINER_GUTTER;
                sub_y = 0.0;
                sub_w = 0.0;
                first = true;
            }
            if !first {
                sub_y += CONTAINER_GUTTER;
            }
            out.push((n, col_x + sub_x, sub_y));
            sub_y += hh;
            sub_w = sub_w.max(w);
            col_w = col_w.max(sub_x + sub_w);
            env_h = env_h.max(sub_y);
            first = false;
        }
        col_x += col_w + COLUMN_GUTTER;
    }
    let env_w = (col_x - COLUMN_GUTTER).max(0.0);
    (out, env_w, env_h)
}

/// Mean row of a node's dependency predecessors — the barycentre key
/// for within-column ordering. A node with no placed predecessor
/// sorts to the top (`0.0`).
fn bary(node: u32, deps: &[(u32, u32)], row_of: &HashMap<u32, f32>) -> f32 {
    let (mut sum, mut n) = (0.0_f32, 0.0_f32);
    for &(u, v) in deps {
        if v == node {
            if let Some(&r) = row_of.get(&u) {
                sum += r;
                n += 1.0;
            }
        }
    }
    if n > 0.0 { sum / n } else { 0.0 }
}

/// Top-down positioning. The root projects are arranged at the world
/// origin in dependency columns; then every container's children are
/// arranged relative to it the same way. Arena order
/// (parent-before-child) lets us iterate forward so a container's
/// `(x, y)` is already final by the time we reach its children.
fn position_layout(h: &mut Hierarchy, child_deps: &HashMap<Option<u32>, HashSet<(u32, u32)>>) {
    let empty: HashSet<(u32, u32)> = HashSet::new();

    // Root tier — laid out at the world origin.
    let roots: Vec<(u32, f32, f32)> = h
        .roots
        .iter()
        .map(|&r| (r, h.nodes[r as usize].w, h.nodes[r as usize].h))
        .collect();
    let (root_pos, _, _) = layered_arrange(&roots, child_deps.get(&None).unwrap_or(&empty));
    for (n, x, y) in root_pos {
        h.nodes[n as usize].x = x;
        h.nodes[n as usize].y = y;
    }

    // Every container's children, relative to the container's inner
    // origin (below the header, below its own chip region).
    for idx in 0..h.nodes.len() {
        let children = h.nodes[idx].children.clone();
        if children.is_empty() {
            continue;
        }
        let chips_h = chips_region_height(&h.nodes[idx].sections);
        let origin_x = h.nodes[idx].x + CONTAINER_PADDING;
        let chips_origin_y = h.nodes[idx].y + CONTAINER_HEADER_H;
        let origin_y = if chips_h > 0.0 {
            chips_origin_y + chips_h + CONTAINER_GUTTER
        } else {
            chips_origin_y
        };

        let items: Vec<(u32, f32, f32)> = children
            .iter()
            .map(|&c| (c, h.nodes[c as usize].w, h.nodes[c as usize].h))
            .collect();
        let deps = child_deps.get(&Some(idx as u32)).unwrap_or(&empty);
        let (pos, _, _) = layered_arrange(&items, deps);
        for (n, rx, ry) in pos {
            h.nodes[n as usize].x = origin_x + rx;
            h.nodes[n as usize].y = origin_y + ry;
        }
    }
}
