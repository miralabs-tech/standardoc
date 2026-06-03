import type { BrowseSymbol } from '../mcp-client';

import type {
  BuiltOverviewPayload,
  ClusterTarget,
  CollapseResult,
  CollapsedProject,
  GraphEdge,
  OverviewNodeBuilder,
  OverviewScope,
  WorkspaceProject,
} from './types';

/**
 * Workspace Overview = flat root-package view. One node per project,
 * placed at depth 0 in the depth-stacked layout. No sub-modules, no
 * public symbols — the focus-graph + drill-down are the right tools
 * for that. Keeps the workspace canvas to ~12 spheres on standardoc
 * scale, which is the only count that stays readable + 60fps on a
 * dense workspace.
 */
export function buildOverviewPayloadForScope(
  scope: OverviewScope,
  projects: ReadonlyArray<WorkspaceProject>,
  symbols: ReadonlyArray<BrowseSymbol>,
  edges: ReadonlyArray<GraphEdge>,
  symbolByFqdn: Map<string, BrowseSymbol>,
): BuiltOverviewPayload {
  if (scope.kind === 'workspace') {
    return buildWorkspacePackagesPayload(projects, symbols, edges, symbolByFqdn);
  }
  return buildScopedModulesPayload(scope, projects, symbols, edges, symbolByFqdn);
}

/**
 * Hierarchical Overview within a drilled scope. Modules only — public
 * symbols are intentionally excluded so the depth-stacked cone stays
 * readable even on a heavy crate like standardoc-ir. Recenters the
 * shallowest in-scope module to depth 0 so the drill root anchors
 * the bottom of the visible cone.
 */
function buildScopedModulesPayload(
  scope: Exclude<OverviewScope, { kind: 'workspace' }>,
  projects: ReadonlyArray<WorkspaceProject>,
  symbols: ReadonlyArray<BrowseSymbol>,
  edges: ReadonlyArray<GraphEdge>,
  symbolByFqdn: Map<string, BrowseSymbol>,
): BuiltOverviewPayload {
  const project = scope.kind === 'project' || scope.kind === 'folder'
    ? projects.find(p => p.project_id === scope.projectId) ?? null
    : null;
  const projRelPath = project?.rel_path.replace(/\\/g, '/') ?? '';
  const pathPrefix = scope.kind === 'folder' && scope.relPath.length > 0
    ? `${projRelPath}/${scope.relPath}`
    : projRelPath;

  const inScope = (s: BrowseSymbol): boolean => {
    if (scope.kind === 'module') {
      // FQDN-prefix drill — accept symbols whose module fqdn is
      // exactly the prefix or sits under it (`prefix::…`). This is
      // the "drill one segment deeper" semantics.
      const mod = typeof s.module === 'string' ? s.module : '';
      return mod === scope.prefix || mod.startsWith(`${scope.prefix}::`);
    }
    if (s.project_id !== scope.projectId) return false;
    if (scope.kind === 'project') return true;
    if (pathPrefix.length === 0) return true;
    const norm = (s.file ?? '').replace(/\\/g, '/');
    return norm === pathPrefix || norm.startsWith(`${pathPrefix}/`);
  };

  const inScopeSymbols = symbols.filter(inScope);
  if (inScopeSymbols.length === 0) {
    return { json: JSON.stringify({ nodes: [], edges: [] }), targets: new Map(), crossKinds: [] };
  }

  // Walk an `s.module` fqdn up to the nearest actual module symbol.
  // The IR sets a struct/enum's fields' `module` to the parent type
  // fqdn (`WriterContext::pool` → module = `WriterContext`), but the
  // Overview only shows `kind === 'module'` symbols as spheres —
  // types are explored via the focus-graph, not by drilling here.
  const resolveContainingModule = (fqdn: string): string | null => {
    let cursor: string | null = fqdn;
    while (cursor !== null) {
      const sym = symbolByFqdn.get(cursor);
      if (sym === undefined || sym.kind === 'module') return cursor;
      const segs: string[] = cursor.split('::');
      cursor = segs.length > 1 ? segs.slice(0, -1).join('::') : null;
    }
    return null;
  };

  const moduleFqdns = new Set<string>();
  for (const s of inScopeSymbols) {
    if (typeof s.module !== 'string' || s.module.length === 0) continue;
    const resolved = resolveContainingModule(s.module);
    if (resolved !== null) moduleFqdns.add(resolved);
  }

  const projectByLabel = new Map<string, { project_id: number; label: string; kind: { kind: string } }>(
    projects.map(p => [p.label, p]),
  );

  // Scope root FQDN — the drilled prefix anchors the layout at
  // depth 0 after recentering, so we MUST stop the parent-chain
  // synthesis at it (otherwise `ensure` walks up to the package
  // root, dragging extra strata above the anchor and pushing the
  // drilled prefix to depth 1, which breaks the "drill +1 seg"
  // semantics on subsequent clicks). For project scope the root is
  // the project label; for folder scope there's no clean single
  // root, so we let synthesis run naturally and recenter by min.
  const scopeRoot: string | null =
    scope.kind === 'module' ? scope.prefix :
    scope.kind === 'project' ? scope.label :
    null;
  const isWithinScope = (fqdn: string): boolean => {
    if (scopeRoot === null) return true;
    return fqdn === scopeRoot || fqdn.startsWith(`${scopeRoot}::`);
  };

  const builders = new Map<string, OverviewNodeBuilder>();
  const ensure = (fqdn: string): OverviewNodeBuilder => {
    const existing = builders.get(fqdn);
    if (existing !== undefined) return existing;
    const segments = fqdn.split('::');
    const depth = segments.length - 1;
    const rawParent = depth > 0 ? segments.slice(0, -1).join('::') : null;
    // Cut the parent chain at the scope boundary so the drilled
    // prefix sits at the shallowest depth and recenters to 0.
    const parent_fqdn = (rawParent !== null && isWithinScope(rawParent)) ? rawParent : null;
    const label = segments[segments.length - 1] ?? fqdn;
    const proj = projectByLabel.get(fqdn);
    const builder: OverviewNodeBuilder = {
      fqdn,
      label,
      depth,
      parent_fqdn,
      node_kind: 'module',
      symbol_count: 0,
      project_kind: proj?.kind.kind,
    };
    builders.set(fqdn, builder);
    if (parent_fqdn !== null) ensure(parent_fqdn);
    return builder;
  };

  // Make sure the scope root itself is in the builders even if no
  // in-scope symbol declares it as its `module` (common when the
  // drilled prefix is a pure container module with no direct
  // symbols of its own).
  if (scopeRoot !== null) ensure(scopeRoot);
  for (const fqdn of moduleFqdns) {
    ensure(fqdn);
  }
  // 3. Symbol counts per module — total symbols whose `module` matches.
  for (const s of inScopeSymbols) {
    if (typeof s.module !== 'string' || s.module.length === 0) continue;
    const m = builders.get(s.module);
    if (m !== undefined) m.symbol_count += 1;
  }

  // Recenter depths so the shallowest in-scope node sits at depth 0.
  const minDepth = [...builders.values()].reduce((m, b) => Math.min(m, b.depth), Number.MAX_SAFE_INTEGER);
  if (minDepth > 0) {
    for (const b of builders.values()) b.depth -= minDepth;
  }

  // Drill one segment deeper at a time: render only the anchor
  // (depth 0) + its DIRECT children (depth 1). Grand-children stay
  // off-screen until the user drills into one of the direct children,
  // which becomes the new anchor in the next scope.
  for (const fqdn of [...builders.keys()]) {
    if (builders.get(fqdn)!.depth > 1) builders.delete(fqdn);
  }

  // Assign stable u32 ids — sort by (depth asc, fqdn asc) so the id
  // ordering visually matches the rendered hierarchy.
  const orderedBuilders = [...builders.values()].sort((a, b) => {
    if (a.depth !== b.depth) return a.depth - b.depth;
    return a.fqdn.localeCompare(b.fqdn);
  });
  const idByFqdn = new Map<string, number>();
  let nextId = 0;
  for (const b of orderedBuilders) {
    idByFqdn.set(b.fqdn, nextId++);
  }

  const nodes = orderedBuilders.map(b => ({
    id: idByFqdn.get(b.fqdn)!,
    label: b.label,
    kind: b.project_kind ?? null,
    symbol_count: b.symbol_count,
    depth: b.depth,
    parent_id: b.parent_fqdn !== null ? idByFqdn.get(b.parent_fqdn) ?? null : null,
    node_kind: b.node_kind,
  }));

  // Parent_child edges — synthesized from the FQDN tree.
  const treeEdges = orderedBuilders
    .filter(b => b.parent_fqdn !== null && idByFqdn.has(b.parent_fqdn))
    .map(b => ({
      from: idByFqdn.get(b.parent_fqdn!)!,
      to: idByFqdn.get(b.fqdn)!,
      weight: 1,
      edge_kind: 'parent_child' as const,
    }));

  // Cross edges — IR edges between in-scope nodes, aggregated by
  // (from, to, kind) so each kind keeps its own count and color in
  // the legend. Map each endpoint to its node (the fqdn itself if
  // it's a node, else its containing module). Drop edges that already
  // exist as the structural parent_child to avoid double-counting.
  const resolveNodeFqdn = (fqdn: string): string | null => {
    if (builders.has(fqdn)) return fqdn;
    const sym = symbolByFqdn.get(fqdn);
    if (typeof sym?.module === 'string' && builders.has(sym.module)) {
      return sym.module;
    }
    return null;
  };
  const aggregated = new Map<string, { from: number; to: number; weight: number; kind: string }>();
  for (const e of edges) {
    const fromFqdn = resolveNodeFqdn(e.from);
    const toFqdn = resolveNodeFqdn(e.to);
    if (fromFqdn === null || toFqdn === null) continue;
    if (fromFqdn === toFqdn) continue;
    const fromBuilder = builders.get(fromFqdn)!;
    const toBuilder = builders.get(toFqdn)!;
    if (fromBuilder.parent_fqdn === toFqdn || toBuilder.parent_fqdn === fromFqdn) continue;
    const fromId = idByFqdn.get(fromFqdn)!;
    const toId = idByFqdn.get(toFqdn)!;
    const key = `${fromId}->${toId}#${e.kind}`;
    const bucket = aggregated.get(key);
    if (bucket === undefined) aggregated.set(key, { from: fromId, to: toId, weight: 1, kind: e.kind });
    else bucket.weight += 1;
  }
  const crossEdges = [...aggregated.values()].map(e => ({ ...e, edge_kind: 'cross' as const }));

  // Click targets — bucket symbols by RESOLVED containing module so
  // a struct's fields/methods (whose raw `module` points to the
  // struct, not the file) count against the file module they
  // physically live in. Otherwise leaf modules with no top-level
  // symbols would end up with an empty `own` bucket and the click
  // would do nothing.
  const symbolsByPrefix = new Map<string, BrowseSymbol[]>();
  for (const s of inScopeSymbols) {
    if (typeof s.module !== 'string' || s.module.length === 0) continue;
    const resolved = resolveContainingModule(s.module);
    if (resolved === null) continue;
    const bucket = symbolsByPrefix.get(resolved);
    if (bucket === undefined) symbolsByPrefix.set(resolved, [s]);
    else bucket.push(s);
  }
  // Targets:
  //   * depth-0 anchor → focus-symbol on its representative (clicking
  //     "you are here" surfaces details about the current module
  //     instead of being a no-op).
  //   * depth-1 with sub-modules → drill-module (one segment deeper).
  //   * depth-1 leaf module → focus-symbol on representative so the
  //     focus-graph / details panels react and the user sees the
  //     module's content even when there's nothing to drill into.
  const hasDescendants = (fqdn: string): boolean => {
    const prefix = `${fqdn}::`;
    for (const candidate of moduleFqdns) {
      if (candidate !== fqdn && candidate.startsWith(prefix)) return true;
    }
    return false;
  };
  const targets = new Map<number, ClusterTarget>();
  for (const b of orderedBuilders) {
    const id = idByFqdn.get(b.fqdn)!;
    if (b.depth > 0 && hasDescendants(b.fqdn)) {
      targets.set(id, { kind: 'drill-module', prefix: b.fqdn, label: b.label });
      continue;
    }
    // Focus the MODULE symbol itself when it exists in the index —
    // the user clicked "shell", they get details about "shell", not
    // some arbitrary `BuiltOverviewPayload` interface that happens to
    // win the alphabetical tiebreaker inside the file. Falls back to
    // a top-symbol representative when the module fqdn is a pure
    // synthesized container with no indexed symbol of its own.
    if (symbolByFqdn.has(b.fqdn)) {
      targets.set(id, { kind: 'focus-symbol', fqdn: b.fqdn });
      continue;
    }
    const own = symbolsByPrefix.get(b.fqdn) ?? [];
    if (own.length > 0) {
      targets.set(id, { kind: 'focus-symbol', fqdn: pickModuleRepresentative(own) });
    }
  }

  const crossKinds = [...new Set(crossEdges.map(e => e.kind))].sort();
  return {
    json: JSON.stringify({ nodes, edges: [...treeEdges, ...crossEdges] }),
    targets,
    crossKinds,
  };
}

/**
 * Breadcrumb label shown in the Overview top-left pill. `null` hides
 * the breadcrumb entirely (workspace mode).
 */
export function scopeBreadcrumbLabel(scope: OverviewScope): string | null {
  switch (scope.kind) {
    case 'workspace': return null;
    case 'project': return scope.label;
    case 'folder': return `${scope.label}/${scope.relPath}`;
    case 'module': return scope.prefix;
  }
}

function longestCommonPathPrefix(paths: ReadonlyArray<string>): string {
  if (paths.length === 0) return '';
  const first = paths[0]!;
  if (paths.length === 1) return first;
  const segmented = paths.map(p => p.split('/').filter(s => s.length > 0));
  const minLen = Math.min(...segmented.map(s => s.length));
  const common: string[] = [];
  for (let i = 0; i < minLen; i++) {
    const seg = segmented[0]![i]!;
    if (segmented.every(s => s[i] === seg)) common.push(seg);
    else break;
  }
  return common.join('/');
}

function collapseProjectsByLabel(projects: ReadonlyArray<WorkspaceProject>): CollapseResult {
  const groups = new Map<string, WorkspaceProject[]>();
  for (const p of projects) {
    const bucket = groups.get(p.label);
    if (bucket === undefined) groups.set(p.label, [p]);
    else bucket.push(p);
  }
  const canonicalProjectId = new Map<number, number>();
  const collapsed: CollapsedProject[] = [];
  for (const members of groups.values()) {
    const sorted = members.slice().sort((a, b) => a.project_id - b.project_id);
    const canonical = sorted[0]!;
    for (const m of sorted) canonicalProjectId.set(m.project_id, canonical.project_id);
    const relPaths = sorted.map(m => m.rel_path);
    const first = relPaths[0]!;
    let rel_path: string;
    if (relPaths.length === 1) {
      rel_path = first;
    } else {
      const lcp = longestCommonPathPrefix(relPaths);
      const head = lcp.length > 0 ? lcp : first;
      rel_path = `${head} (+${relPaths.length - 1} more)`;
    }
    collapsed.push({
      project_id: canonical.project_id,
      label: canonical.label,
      rel_path,
      kind: canonical.kind,
    });
  }
  return { collapsed, canonicalProjectId };
}

/**
 * Workspace flat root view — one sphere per LABEL group at depth 0
 * (projects sharing a `.sxd` label collapse into a single visual node;
 * mechanical detection projects each have a unique label and stay
 * 1-to-1). Edges aggregated by `(from_label, to_label, kind)` so each
 * IR kind keeps its swatch in the legend. Click target = drill-project
 * on the canonical (min) project_id of the group.
 */
function buildWorkspacePackagesPayload(
  projects: ReadonlyArray<WorkspaceProject>,
  symbols: ReadonlyArray<BrowseSymbol>,
  edges: ReadonlyArray<GraphEdge>,
  symbolByFqdn: Map<string, BrowseSymbol>,
): BuiltOverviewPayload {
  const { collapsed, canonicalProjectId } = collapseProjectsByLabel(projects);
  const canonical = (pid: number | null | undefined): number | undefined => {
    if (pid === undefined || pid === null) return undefined;
    return canonicalProjectId.get(pid) ?? pid;
  };

  const counts = new Map<number, number>();
  for (const s of symbols) {
    const pid = canonical(s.project_id);
    if (pid === undefined) continue;
    counts.set(pid, (counts.get(pid) ?? 0) + 1);
  }
  const idByProject = new Map<number, number>();
  const nodes: Array<{
    id: number;
    label: string;
    kind: string | null;
    symbol_count: number;
    depth: number;
    parent_id: number | null;
    node_kind: 'module';
  }> = [];
  const targets = new Map<number, ClusterTarget>();
  let nextId = 0;
  for (const p of collapsed) {
    const id = nextId++;
    idByProject.set(p.project_id, id);
    nodes.push({
      id,
      label: p.label,
      kind: p.kind.kind,
      symbol_count: counts.get(p.project_id) ?? 0,
      depth: 0,
      parent_id: null,
      node_kind: 'module',
    });
    targets.set(id, { kind: 'drill-project', projectId: p.project_id, label: p.label });
  }

  const aggregated = new Map<string, { from: number; to: number; weight: number; kind: string }>();
  for (const e of edges) {
    const fromProj = canonical(symbolByFqdn.get(e.from)?.project_id);
    const toProj = canonical(symbolByFqdn.get(e.to)?.project_id);
    if (fromProj === undefined || toProj === undefined) continue;
    if (fromProj === toProj) continue;
    const fromId = idByProject.get(fromProj);
    const toId = idByProject.get(toProj);
    if (fromId === undefined || toId === undefined) continue;
    const key = `${fromId}->${toId}#${e.kind}`;
    const bucket = aggregated.get(key);
    if (bucket === undefined) aggregated.set(key, { from: fromId, to: toId, weight: 1, kind: e.kind });
    else bucket.weight += 1;
  }
  const crossEdges = [...aggregated.values()].map(e => ({ ...e, edge_kind: 'cross' as const }));
  const crossKinds = [...new Set(crossEdges.map(e => e.kind))].sort();

  return {
    json: JSON.stringify({ nodes, edges: crossEdges }),
    targets,
    crossKinds,
  };
}

/**
 * Pick a single symbol to focus when a module cluster is clicked.
 * Prefer entry points → public symbols → shortest fqdn → alphabetical
 * so the click lands on a meaningful surface rather than a random
 * private helper buried in the middle of the file.
 */
function pickModuleRepresentative(symbols: ReadonlyArray<BrowseSymbol>): string {
  let best: { fqdn: string; rank: [number, number, number, string] } | null = null;
  for (const s of symbols) {
    const ep = s.entry_point ? 0 : 1;
    const vis = s.visibility === 'public' ? 0 : 1;
    const segCount = s.fqdn.split('::').length;
    const rank: [number, number, number, string] = [ep, vis, segCount, s.fqdn];
    if (best === null
      || rank[0] < best.rank[0]
      || (rank[0] === best.rank[0] && rank[1] < best.rank[1])
      || (rank[0] === best.rank[0] && rank[1] === best.rank[1] && rank[2] < best.rank[2])
      || (rank[0] === best.rank[0] && rank[1] === best.rank[1] && rank[2] === best.rank[2] && rank[3] < best.rank[3])
    ) {
      best = { fqdn: s.fqdn, rank };
    }
  }
  return best?.fqdn ?? symbols[0]?.fqdn ?? '';
}
