//! Drill-down hierarchy for the WebGPU graph view.
//!
//! `fetch_graph` returns a flat symbol list, but every symbol's
//! `module` field names its parent FQDN — so the workspace is really
//! a tree: `root → projects → modules → … → types → members`. This
//! module rebuilds that tree and tracks a `focus` pointer. The 3D
//! view only ever renders the focused node's direct children, so the
//! screen stays bounded no matter how large the workspace is.

use std::collections::{HashMap, HashSet};

use crate::kind::Kind;
use crate::payload::GraphPayload;

/// One node of the drill tree — a project or a symbol.
pub(crate) struct TreeNode {
    /// Display name — symbol name or project label. Rendered by the
    /// host's DOM label layer over the WebGPU canvas.
    pub label: String,
    /// Broad language for colouring (`rust` / `typescript` / …), or
    /// the project kind for project nodes.
    pub language: String,
    /// Source-symbol FQDN — handed back to the host on a leaf click.
    /// Empty for synthetic project nodes.
    pub fqdn: String,
    /// Symbol kind (`Function` / `Type` / `Value` / `Module` /
    /// `Macro`) — `Unknown` for synthetic project nodes. Drives the
    /// per-Kind grouping + header colouring in the 2D card view.
    pub kind: Kind,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    /// Nodes in this subtree excluding self — drives node sizing.
    pub descendant_count: u32,
    /// Phase 3 (Flow) entry-point tag — `binary_main` / `public_api`
    /// / `ffi_export`. `None` for internal symbols and for synthetic
    /// project / virtual-module nodes. Threaded through `Card` and
    /// `LevelNode` so both 2D and 3D renderers can paint a halo.
    pub entry_point: Option<String>,
}

pub(crate) struct DrillTree {
    nodes: Vec<TreeNode>,
    /// Project nodes — the level shown when `focus` is `None`.
    root_children: Vec<u32>,
    /// Cross-link edges as tree-node-index pairs. Aggregated to the
    /// current drill level by `level_edges`.
    edges: Vec<(u32, u32)>,
    /// `None` = root level (projects). `Some(i)` = node `i`'s children.
    focus: Option<u32>,
    /// Project tree index → sorted list of entry-point symbol tree
    /// indices anywhere in that project's subtree. Empty entry / absent
    /// key both mean "no entry-points known". Sort key is fqdn so the
    /// satellite ring is stable across rebuilds.
    entry_points_by_project: HashMap<u32, Vec<u32>>,
}

impl DrillTree {
    /// An empty tree — the engine's state before a graph is loaded.
    pub(crate) fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            root_children: Vec::new(),
            edges: Vec::new(),
            focus: None,
            entry_points_by_project: HashMap::new(),
        }
    }

    /// Rebuild the tree from a freshly-parsed payload.
    pub(crate) fn build(payload: &GraphPayload) -> Self {
        let mut nodes: Vec<TreeNode> =
            Vec::with_capacity(payload.symbols.len() + payload.projects.len());
        // Owned-key map so we can insert paths for synthetic
        // intermediate modules (see the virtual-module pass below) —
        // those paths don't exist in `payload.symbols` and so cannot
        // borrow from it.
        let mut by_fqdn: HashMap<String, u32> = HashMap::with_capacity(payload.symbols.len());
        let mut project_node: HashMap<u32, u32> = HashMap::new();
        let mut project_label_to_idx: HashMap<String, u32> = HashMap::new();

        // Project nodes first — they anchor every top-level symbol and
        // occupy tree indices `0..projects.len()`.
        for p in &payload.projects {
            let idx = nodes.len() as u32;
            project_node.insert(p.project_id, idx);
            project_label_to_idx.insert(p.label.clone(), idx);
            nodes.push(TreeNode {
                label: p.label.clone(),
                language: p.kind.clone(),
                fqdn: String::new(),
                kind: Kind::Unknown,
                parent: None,
                children: Vec::new(),
                descendant_count: 0,
                entry_point: None,
            });
        }

        // Symbol nodes — all created before parents resolve, so a
        // parent appearing later in the list still links.
        let symbol_base = nodes.len() as u32;
        for s in &payload.symbols {
            by_fqdn.insert(s.fqdn.clone(), nodes.len() as u32);
            nodes.push(TreeNode {
                label: s.name.clone(),
                language: s.language.clone(),
                fqdn: s.fqdn.clone(),
                kind: s.kind,
                parent: None,
                children: Vec::new(),
                descendant_count: 0,
                entry_point: s.entry_point.clone(),
            });
        }

        // Synthetic module pass — when a symbol's `module` field
        // references an fqdn that isn't itself a symbol in the
        // payload (the daemon sampled it out, or it never had a
        // declarable form e.g. Rust `#[cfg(test)] mod tests`), we
        // need to materialise an intermediate `Kind::Module` node so
        // the focused-level view shows containers instead of
        // hundreds of leaves flattened against the project root.
        let mut needed: HashMap<String, String> = HashMap::new();
        for s in &payload.symbols {
            let Some(m) = s.module.as_deref() else {
                continue;
            };
            if by_fqdn.contains_key(m) {
                continue;
            }
            needed.entry(m.to_string()).or_insert_with(|| s.language.clone());
        }
        // Walk paths shortest-first so a parent virtual module is
        // always created before any of its descendants.
        let mut sorted: Vec<(String, String)> = needed.into_iter().collect();
        sorted.sort_by_key(|(p, _)| p.len());
        for (path, language) in sorted {
            ensure_virtual_path(
                &mut nodes,
                &mut by_fqdn,
                &project_label_to_idx,
                &path,
                &language,
            );
        }

        // Resolve each symbol's parent: the symbol named by `module`
        // when it is in the payload, else the project, else the root.
        let mut root_children: Vec<u32> = (0..payload.projects.len() as u32).collect();
        for (i, s) in payload.symbols.iter().enumerate() {
            let idx = symbol_base + i as u32;
            let parent = s
                .module
                .as_deref()
                .and_then(|m| by_fqdn.get(m).copied())
                .or_else(|| s.project_id.and_then(|pid| project_node.get(&pid).copied()));
            match parent {
                Some(p) => {
                    nodes[p as usize].children.push(idx);
                    nodes[idx as usize].parent = Some(p);
                }
                None => root_children.push(idx),
            }
        }

        // Cross-link edges → tree-node-index pairs; unresolved
        // endpoints and self-loops dropped.
        let mut edges = Vec::with_capacity(payload.edges.len());
        for e in &payload.edges {
            if let (Some(&from), Some(&to)) =
                (by_fqdn.get(e.from.as_str()), by_fqdn.get(e.to.as_str()))
            {
                if from != to {
                    edges.push((from, to));
                }
            }
        }

        // Local project_of[] — for each tree node index, the index of
        // the project ancestor (or u32::MAX for root-orphan symbols
        // whose `project_id` couldn't be resolved). Walks each node's
        // parent chain until landing on a project node (`idx <
        // n_projects`) or the root. Done after all parents are
        // resolved (including symbols pointing into virtual modules
        // created higher in the vec). Consumed below to bucket
        // entry-points by project, then discarded — the bucket map is
        // the only durable state.
        let n_projects = payload.projects.len();
        let mut project_of: Vec<u32> = vec![u32::MAX; nodes.len()];
        for (i, p) in project_of.iter_mut().enumerate().take(n_projects) {
            *p = i as u32;
        }
        for i in n_projects..nodes.len() {
            let mut cur = i as u32;
            loop {
                match nodes[cur as usize].parent {
                    None => break, // root-orphan: project_of[i] stays MAX
                    Some(p) => {
                        if (p as usize) < n_projects {
                            project_of[i] = p;
                            break;
                        }
                        cur = p;
                    }
                }
            }
        }

        // Group entry-points by their owning project. Symbols with no
        // resolvable project are dropped — they would have no cube to
        // satellite from. Sort each project's list by fqdn so the ring
        // placement is stable rebuild-to-rebuild.
        let mut entry_points_by_project: HashMap<u32, Vec<u32>> = HashMap::new();
        for (i, node) in nodes.iter().enumerate() {
            if node.entry_point.is_none() {
                continue;
            }
            let pi = project_of[i];
            if pi == u32::MAX {
                continue;
            }
            entry_points_by_project
                .entry(pi)
                .or_default()
                .push(i as u32);
        }
        for v in entry_points_by_project.values_mut() {
            v.sort_by(|&a, &b| nodes[a as usize].fqdn.cmp(&nodes[b as usize].fqdn));
        }

        let mut tree = Self {
            nodes,
            root_children,
            edges,
            focus: None,
            entry_points_by_project,
        };
        for r in tree.root_children.clone() {
            tree.fill_counts(r);
        }
        tree
    }

    /// Post-order fill of every subtree's `descendant_count`.
    fn fill_counts(&mut self, idx: u32) -> u32 {
        let children = self.nodes[idx as usize].children.clone();
        let mut total = 0;
        for c in children {
            total += 1 + self.fill_counts(c);
        }
        self.nodes[idx as usize].descendant_count = total;
        total
    }

    pub(crate) fn node(&self, idx: u32) -> &TreeNode {
        &self.nodes[idx as usize]
    }

    /// Node indices shown at the current focus — the focused node's
    /// children, or the projects at the root.
    pub(crate) fn current_level(&self) -> &[u32] {
        match self.focus {
            None => &self.root_children,
            Some(i) => &self.nodes[i as usize].children,
        }
    }

    pub(crate) fn is_container(&self, idx: u32) -> bool {
        !self.nodes[idx as usize].children.is_empty()
    }

    /// Descend into `idx` when it has children. Returns whether the
    /// focus actually moved.
    pub(crate) fn descend(&mut self, idx: u32) -> bool {
        if self.is_container(idx) {
            self.focus = Some(idx);
            true
        } else {
            false
        }
    }

    /// Ascend to the parent level. Returns whether the focus moved.
    pub(crate) fn ascend(&mut self) -> bool {
        match self.focus {
            None => false,
            Some(i) => {
                self.focus = self.nodes[i as usize].parent;
                true
            }
        }
    }

    /// Jump the focus directly to `idx`. The focused node becomes the
    /// new container whose children are rendered. Out-of-range indices
    /// are ignored; the focus is unchanged in that case.
    pub(crate) fn focus_to(&mut self, idx: u32) -> bool {
        if (idx as usize) >= self.nodes.len() {
            return false;
        }
        if self.focus == Some(idx) {
            return false;
        }
        self.focus = Some(idx);
        true
    }

    /// Reset the focus to the root level (the projects). Returns
    /// whether the focus actually moved.
    pub(crate) fn reset_focus(&mut self) -> bool {
        if self.focus.is_none() {
            return false;
        }
        self.focus = None;
        true
    }

    /// Breadcrumb trail from root to the current focus (inclusive),
    /// each entry `(label, tree_idx)`. Empty when the focus sits at
    /// the root (no project descended into yet). The JS host renders
    /// this as a clickable breadcrumb; clicking a crumb feeds its
    /// `tree_idx` back through `focus_to`.
    pub(crate) fn breadcrumb(&self) -> Vec<(String, u32)> {
        let mut path: Vec<(String, u32)> = Vec::new();
        let mut cur = self.focus;
        while let Some(idx) = cur {
            let n = &self.nodes[idx as usize];
            path.push((n.label.clone(), idx));
            cur = n.parent;
        }
        path.reverse();
        path
    }

    /// Walk up the parent chain from `node` until the encountered
    /// node's parent is exactly `level_parent`. Returns that node's
    /// `DrillTree.nodes` index, or `None` when the walk hits the root
    /// without ever matching. Used by `cross_edges` to map an
    /// out-of-focus endpoint to its sibling-of-focus ancestor.
    fn ancestor_with_parent(&self, mut node: u32, level_parent: Option<u32>) -> Option<u32> {
        loop {
            let n = &self.nodes[node as usize];
            if n.parent == level_parent {
                return Some(node);
            }
            match n.parent {
                Some(p) => node = p,
                None => return None,
            }
        }
    }

    /// Edges that **leave** the focused subtree — for every cross-
    /// boundary edge, returns `(inside_tree_idx, sibling_tree_idx)`
    /// where:
    /// - `inside_tree_idx`: the level-child of the focused node whose
    ///   subtree contains the inside endpoint
    /// - `sibling_tree_idx`: the sibling of the focused node whose
    ///   subtree contains the outside endpoint
    ///
    /// Empty at the root level (the root has no siblings) and when
    /// the focus is set on a leaf. The scene materialises one
    /// **ghost card** per distinct sibling so the user sees *which*
    /// crates/modules the focused subtree couples to, even though
    /// those crates aren't part of the focused level.
    pub(crate) fn cross_edges(&self) -> Vec<(u32, u32)> {
        let Some(focus) = self.focus else {
            return Vec::new();
        };
        let focus_parent = self.nodes[focus as usize].parent;
        let level = self.current_level();
        if level.is_empty() {
            return Vec::new();
        }
        // Map every descendant of the focus subtree to the level
        // child that owns it. Inside endpoints land in this map.
        let mut inside_owner: HashMap<u32, u32> = HashMap::new();
        for &root in level.iter() {
            let mut stack = vec![root];
            while let Some(n) = stack.pop() {
                inside_owner.insert(n, root);
                stack.extend_from_slice(&self.nodes[n as usize].children);
            }
        }
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        let mut out = Vec::new();
        for &(from, to) in &self.edges {
            let from_in = inside_owner.get(&from).copied();
            let to_in = inside_owner.get(&to).copied();
            let (inside, outside_node) = match (from_in, to_in) {
                (Some(inside), None) => (inside, to),
                (None, Some(inside)) => (inside, from),
                _ => continue, // internal-only or unrelated
            };
            let Some(sibling) = self.ancestor_with_parent(outside_node, focus_parent) else {
                continue;
            };
            if sibling == focus {
                continue;
            }
            let key = (inside, sibling);
            if seen.insert(key) {
                out.push(key);
            }
        }
        out
    }

    /// Edges between the current level's siblings, as `(from_level_idx,
    /// to_level_idx, weight)` triples directed along their original
    /// cross-link direction. `weight` is the count of distinct
    /// underlying symbol→symbol cross-links collapsed to this altitude
    /// — at the workspace root, that's how many times any symbol in
    /// project A links to any symbol in project B, separately counted
    /// from B→A. Renderers map weight to stroke thickness (2D) or
    /// alpha intensity (3D, where wgpu line topology can't vary width
    /// per segment).
    pub(crate) fn level_edges(&self) -> Vec<(u32, u32, u32)> {
        let level = self.current_level();
        // tree-node index → the sibling (level index) owning it.
        let mut owner: HashMap<u32, u32> = HashMap::new();
        for (li, &root) in level.iter().enumerate() {
            let mut stack = vec![root];
            while let Some(n) = stack.pop() {
                owner.insert(n, li as u32);
                stack.extend_from_slice(&self.nodes[n as usize].children);
            }
        }
        let mut weights: HashMap<(u32, u32), u32> = HashMap::new();
        for &(from, to) in &self.edges {
            if let (Some(&a), Some(&b)) = (owner.get(&from), owner.get(&to)) {
                if a != b {
                    *weights.entry((a, b)).or_insert(0) += 1;
                }
            }
        }
        weights.into_iter().map(|((a, b), w)| (a, b, w)).collect()
    }

    /// Entry-point symbol tree indices anywhere inside `project_idx`'s
    /// subtree, sorted by fqdn. Empty when the project has no
    /// entry-points or when `project_idx` is not a project node. Used
    /// by the workspace-overview renderer to materialise satellite
    /// nodes around each project cube.
    pub(crate) fn entry_points_for_project(&self, project_idx: u32) -> &[u32] {
        self.entry_points_by_project
            .get(&project_idx)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// `true` when no node is currently focused — the rendered level
    /// is the project list. Workspace-overview-only enrichments
    /// (project→project edge aggregation, entry-point satellites,
    /// kind-mix glyphs…) gate on this.
    pub(crate) fn is_root_level(&self) -> bool {
        self.focus.is_none()
    }
}

/// Walk a `::`-delimited module path and ensure every prefix exists
/// as a tree node — creating virtual `Kind::Module` nodes for the
/// segments that aren't already present in `by_fqdn`. The walk
/// anchors on the longest existing ancestor (an actual symbol or a
/// previously-created virtual node); if none matches, the topmost
/// segment is looked up against `project_label_to_idx` so paths like
/// `standardoc-core::query::similarity::tests` re-attach to the
/// `standardoc-core` project node when none of the intermediate
/// modules made it into the payload.
fn ensure_virtual_path(
    nodes: &mut Vec<TreeNode>,
    by_fqdn: &mut HashMap<String, u32>,
    project_label_to_idx: &HashMap<String, u32>,
    path: &str,
    language: &str,
) {
    if by_fqdn.contains_key(path) {
        return;
    }
    let segments: Vec<&str> = path.split("::").collect();
    if segments.is_empty() {
        return;
    }
    // Find the longest existing ancestor by scanning prefixes from
    // longest to shortest. Bounded by depth (typically < 10).
    let mut ancestor: Option<u32> = None;
    let mut ancestor_depth: usize = 0;
    for i in (1..segments.len()).rev() {
        let prefix = segments[..i].join("::");
        if let Some(&idx) = by_fqdn.get(&prefix) {
            ancestor = Some(idx);
            ancestor_depth = i;
            break;
        }
    }
    // Fall back to project label match when no ancestor symbol/
    // virtual exists — the topmost segment IS the project label.
    if ancestor.is_none() {
        if let Some(&idx) = project_label_to_idx.get(segments[0]) {
            ancestor = Some(idx);
            ancestor_depth = 1;
        }
    }
    let Some(mut parent_idx) = ancestor else {
        // Unable to anchor — caller's symbol will fall back to its
        // project node via `project_id` at parent resolution time.
        return;
    };
    // Materialise the missing tail. `acc_path` accumulates the full
    // fqdn for each new node so cross-link edge resolution can find
    // them later (edges aren't routed at virtual nodes today, but
    // `cross_edges` walks the tree by index so the membership map
    // it builds still includes virtual descendants for free).
    let mut acc_path = segments[..ancestor_depth].join("::");
    for &seg in &segments[ancestor_depth..] {
        if !acc_path.is_empty() {
            acc_path.push_str("::");
        }
        acc_path.push_str(seg);
        let new_idx = nodes.len() as u32;
        nodes.push(TreeNode {
            label: seg.to_string(),
            language: language.to_string(),
            fqdn: acc_path.clone(),
            kind: Kind::Module,
            parent: Some(parent_idx),
            children: Vec::new(),
            descendant_count: 0,
            entry_point: None,
        });
        nodes[parent_idx as usize].children.push(new_idx);
        by_fqdn.insert(acc_path.clone(), new_idx);
        parent_idx = new_idx;
    }
}
