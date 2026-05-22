//! Arena-allocated module hierarchy.
//!
//! Built by splitting each `SymbolEntry.module` on `::` and merging
//! shared prefixes — e.g. `std::io::BufReader` and `std::io::Cursor`
//! collapse into a single `std → io → {…}` chain. Symbols attach to
//! the **deepest** node of their path; an intermediate node carries
//! its own symbols only if some symbol's module ended at that exact
//! depth.
//!
//! ## Scale design notes
//!
//! Storage is a flat `Vec<HierarchyNode>` indexed by `u32`. Children
//! are `Vec<u32>`, parent is `Option<u32>`. Layout coords are `f32`.
//! This keeps the per-node footprint compact: at 500 k symbols /
//! 50 k modules the arena fits in ~30 MB instead of the 300 MB a
//! naive nested `Vec<HierarchyNode>` would cost.
//!
//! Hit-testing and the per-frame draw loop remain linear in workspace
//! size — that becomes a quadtree / virtualisation problem in a later
//! round if 100 k+ workspaces become a real target. The arena layout
//! is the **foundation** for that future work, not its blocker.

// Phase A.2 builds the arena and the layout consumes leaves. The
// `parent` / `depth` fields and the `is_empty` / `full_path`
// methods land for Phase B (collapse state keys) and Phase C
// (intermediate cluster framing).
#![allow(dead_code)]

use std::collections::HashMap;

use crate::kind::Kind;

/// One sub-section within an owner node's chip region. Chips of the
/// same `Kind` (Function / Type / Value / Module / Macro / Unknown)
/// are grouped under a collapsible header. Sections are computed at
/// layout time and consumed by the renderer + hit-tester; they're
/// not part of the persistent payload.
#[derive(Debug, Clone)]
pub(crate) struct SectionLayout {
    pub kind: Kind,
    pub chip_count: u32,
    pub expanded: bool,
    /// Y offset of this section, relative to the owner's chip-region
    /// origin (i.e. `owner.y + CONTAINER_HEADER_H`). Section headers
    /// are flush-left at this offset; chips paint below the header
    /// when `expanded`.
    pub y_offset: f32,
    /// Height of the section in its current state (header + chip grid
    /// if expanded, header alone if collapsed). Used by the layout to
    /// stack sections and by hit-test to compute section bounds.
    pub total_h: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct HierarchyNode {
    /// Last path segment ("io" in "std::io"). Empty for the synthetic
    /// `(root)` bucket — by convention we emit the literal string
    /// `"(root)"` so the renderer can identify it.
    pub segment: String,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    /// Indices into `Scene.nodes` of symbols whose module path ends
    /// exactly at this node. Set by `layout::pack` after laying chips
    /// out; empty until then.
    pub symbol_indices: Vec<u32>,
    pub depth: u16,
    /// `symbol_indices.len()` plus the recursive count of every
    /// descendant. Drives the treemap weight in Phase C and the
    /// aggregate header count in Phase D.
    pub recursive_symbol_count: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Pre-truncated header label, computed once by
    /// `Scene::prepare_labels`. Empty until that pass runs.
    pub display_title: String,
    /// Pre-truncated full module path (`a::b::c`), shown as a
    /// description line under `display_title`. Empty when the path
    /// equals the segment (i.e. for a root node) — the renderer
    /// skips the description line in that case.
    pub display_subtitle: String,
    /// Per-Kind sub-sections of this owner's own chips. Empty when
    /// the node has no own chips. Order matches `SECTIONS_ORDER`
    /// (Module, Type, Function, Value, Macro, Unknown) — skipping
    /// kinds with zero chips.
    pub sections: Vec<SectionLayout>,
    /// `Some(kind)` marks a **project-tier** frame (the level above
    /// the module tree) — `kind` is the ecosystem tag (`rust` /
    /// `bun` / `node` / …) the renderer colours the frame by. `None`
    /// is an ordinary module node.
    pub project_kind: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Hierarchy {
    pub nodes: Vec<HierarchyNode>,
    pub roots: Vec<u32>,
}

impl Hierarchy {
    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Append a fresh node, wiring it under `parent` (or registering
    /// it as a root when `parent` is `None`) and returning its arena
    /// index. The single construction site for `HierarchyNode` — both
    /// the project tier and the module tree route through here.
    pub(crate) fn push(
        &mut self,
        segment: String,
        parent: Option<u32>,
        depth: u16,
        project_kind: Option<String>,
    ) -> u32 {
        let idx = self.nodes.len() as u32;
        self.nodes.push(HierarchyNode {
            segment,
            parent,
            children: Vec::new(),
            symbol_indices: Vec::new(),
            depth,
            recursive_symbol_count: 0,
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
            display_title: String::new(),
            display_subtitle: String::new(),
            sections: Vec::new(),
            project_kind,
        });
        match parent {
            Some(p) => self.nodes[p as usize].children.push(idx),
            None => self.roots.push(idx),
        }
        idx
    }

    /// Iterate leaves (nodes with no children) in arena order. The
    /// renderer and layout both walk this — it's the set of "terminal
    /// modules" that own actual chips.
    pub(crate) fn leaves(&self) -> impl Iterator<Item = (u32, &HierarchyNode)> {
        self.nodes.iter().enumerate().filter_map(|(i, n)| {
            if n.children.is_empty() {
                Some((i as u32, n))
            } else {
                None
            }
        })
    }

    /// Reconstruct the full `a::b::c` path for a node. O(depth) walk
    /// up the parent chain — out of the render hot path. Used by
    /// Phase B's localStorage collapse-state key and by debug logs.
    pub(crate) fn full_path(&self, index: u32) -> String {
        let mut segments: Vec<&str> = Vec::new();
        let mut cur = Some(index);
        while let Some(i) = cur {
            let n = &self.nodes[i as usize];
            segments.push(n.segment.as_str());
            cur = n.parent;
        }
        segments.reverse();
        segments.join("::")
    }
}

/// Insert one `::`-delimited module path **under an existing parent
/// node** (a project frame, or a deeper module). Creates any missing
/// intermediate module nodes and returns the index of the terminal
/// node. Idempotent within one `path_to_idx` map — paths that share a
/// prefix reuse the existing nodes.
///
/// `path_to_idx` is expected to be scoped to a single project so two
/// projects with a same-named module (`foo::util`) never collide.
/// An empty `path` resolves to `parent` itself — used when a symbol's
/// module was nothing but the stripped project segment.
pub(crate) fn ensure_path_under(
    h: &mut Hierarchy,
    path_to_idx: &mut HashMap<String, u32>,
    parent: u32,
    path: &str,
) -> u32 {
    if path.is_empty() {
        return parent;
    }
    if let Some(&idx) = path_to_idx.get(path) {
        return idx;
    }

    let base_depth = h.nodes[parent as usize].depth;
    let mut cur = parent;
    let mut acc = String::new();
    for (offset, seg) in path.split("::").enumerate() {
        if !acc.is_empty() {
            acc.push_str("::");
        }
        acc.push_str(seg);
        cur = if let Some(&i) = path_to_idx.get(&acc) {
            i
        } else {
            let depth = base_depth + 1 + offset as u16;
            let i = h.push(seg.to_string(), Some(cur), depth, None);
            path_to_idx.insert(acc.clone(), i);
            i
        };
    }
    cur
}

/// Post-order DFS that fills `recursive_symbol_count` for every
/// node. Call once after `symbol_indices` has been populated. The
/// recursion depth equals the deepest namespace path — for a Rust
/// crate that's ~5–6, for Unreal-Engine-class C++ ~10. Stack-safe.
pub(crate) fn fill_recursive_counts(h: &mut Hierarchy) {
    fn rec(h: &mut Hierarchy, idx: u32) -> u32 {
        // Cloning the children indices is cheap (Vec<u32>) and
        // sidesteps the &mut borrow that would otherwise alias with
        // the recursive call writing to `h.nodes[idx].recursive_…`.
        let children = h.nodes[idx as usize].children.clone();
        let mut total = h.nodes[idx as usize].symbol_indices.len() as u32;
        for c in children {
            total += rec(h, c);
        }
        h.nodes[idx as usize].recursive_symbol_count = total;
        total
    }
    let roots = h.roots.clone();
    for r in roots {
        rec(h, r);
    }
}
