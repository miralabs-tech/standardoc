/**
 * Shell bootstrap — wires the multi-panel layout against the standardoc
 * daemon. Co-exists with the legacy `main.ts` entry; both routes target
 * the same daemon and use the same components. Once the shell is the
 * accepted production target the legacy entry can be retired.
 *
 *   /        → legacy single-canvas playground (index.html + main.ts)
 *   /shell.html → multi-panel shell (shell.html + this file)
 */

import init, { FocusGraphCanvas, OverviewCanvas } from '../pkg/standardoc_graph_viz.js';

import '@standarx/standardoc-viz/components/panel-layout';
import '@standarx/standardoc-viz/components/explorer';
import '@standarx/standardoc-viz/components/symbol-details';
import '@standarx/standardoc-viz/components/search';
import '@standarx/standardoc-viz/components/overview';
import '@standarx/standardoc-viz/components/focus-graph';

import { focusStore } from '@standarx/standardoc-viz/focus-store';
import { McpBrowse } from '@standarx/standardoc-viz/mcp-client';
import type {
  BrowseSymbol,
  GetContextResponse,
  RawSymbol,
} from '@standarx/standardoc-viz/mcp-client';
import type {
  ExplorerElement,
  ExplorerEntryPoint,
  ExplorerNodeKind,
  ExplorerSelectDetail,
  ExplorerTreeNode,
  EntryPointKind,
  FocusGraphElement,
  FocusGraphErrorDetail,
  FocusGraphHopChangeDetail,
  FocusGraphNodeClickDetail,
  OverviewClusterClickDetail,
  OverviewElement,
  OverviewErrorDetail,
  SearchElement,
  SymbolDetail,
  SymbolDetailsElement,
  SymbolDetailsTabChangeDetail,
  SymbolRelationBucket,
  SymbolRelationKind,
  SymbolSearchResult,
} from '@standarx/standardoc-viz';

const explorerEl = document.getElementById('explorer') as ExplorerElement;
const detailsEl = document.getElementById('details') as SymbolDetailsElement;
const searchEl = document.getElementById('search') as SearchElement;
const overviewEl = document.getElementById('overview') as OverviewElement;
const focusEl = document.getElementById('focus') as FocusGraphElement;
const statusEl = document.getElementById('status') as HTMLSpanElement;

function setStatus(text: string): void {
  if (statusEl) statusEl.textContent = text;
}

async function boot(): Promise<void> {
  setStatus('init wasm…');
  await init({ module_or_path: '/pkg/standardoc_graph_viz_bg.wasm' });

  setStatus('connect MCP…');
  const mcp = await McpBrowse.connectHttp(new URL('/mcp', window.location.origin), {
    name: 'standardoc-graph-viz-shell',
    version: '0.0.1',
  });

  // Canvas factories + ready handshakes for the two split canvases.
  // Overview owns the workspace nebula; FocusGraph owns the symbol-
  // local neighbourhood. Both components own pointer + rAF; we just
  // hand them a factory and wait for `*-ready`.
  const overviewReady = new Promise<void>((resolve, reject) => {
    overviewEl.addEventListener('sd-overview-ready', () => resolve(), { once: true });
    overviewEl.addEventListener('sd-overview-error', e => {
      const { source, message } = (e as CustomEvent<OverviewErrorDetail>).detail;
      if (source === 'canvas-init') reject(new Error(message));
    }, { once: true });
  });
  const focusReady = new Promise<void>((resolve, reject) => {
    focusEl.addEventListener('sd-focus-graph-ready', () => resolve(), { once: true });
    focusEl.addEventListener('sd-focus-graph-error', e => {
      const { source, message } = (e as CustomEvent<FocusGraphErrorDetail>).detail;
      if (source === 'canvas-init') reject(new Error(message));
    }, { once: true });
  });
  overviewEl.canvasFactory = (canvas, w, h, dpr) => new OverviewCanvas(canvas, w, h, dpr);
  focusEl.canvasFactory = (canvas, w, h, dpr) => new FocusGraphCanvas(canvas, w, h, dpr);
  await Promise.all([overviewReady, focusReady]);
  const overview = overviewEl.canvas!;
  const focusCanvas = focusEl.canvas!;

  // Cluster click drill — resolved against a representative-symbol
  // map built right after the workspace walk (we have the full symbol
  // set there, this is just a one-pass index).
  const representativeByProjectId = new Map<number, string>();
  // (filled below, after treeSymbols is built)

  overviewEl.addEventListener('sd-overview-cluster-click', e => {
    const { clusterId } = (e as CustomEvent<OverviewClusterClickDetail>).detail;
    const fqdn = representativeByProjectId.get(clusterId);
    if (fqdn !== undefined) focusStore.setFocus(fqdn);
  });

  // Click on a focus-graph node → shift global focus.
  focusEl.addEventListener('sd-focus-graph-node-click', e => {
    const { fqdn } = (e as CustomEvent<FocusGraphNodeClickDetail>).detail;
    focusStore.setFocus(fqdn);
  });

  // Project list — we keep the full project records around because
  // building the IDE-style file tree needs each project's rel_path to
  // strip the workspace prefix off symbol file paths.
  setStatus('list projects…');
  const projectsRes = await mcp.listProjects().catch(() => null);
  const projects = projectsRes?.projects ?? [];

  // Walk the full workspace symbol index in one paginated pass that
  // feeds BOTH the Entry Points section and the IDE-style tree below.
  // fetch_graph's 5k node cap is fine for the visual overview canvas
  // but it truncates the tree on real workspaces — list_symbols' cursor
  // pagination gives complete coverage at the cost of N round-trips
  // (bounded above by EP_MAX_PAGES so a runaway daemon can't hang boot).
  setStatus('workspace symbols…');
  const { all: rawSymbols, entryPoints } = await collectWorkspaceSymbols(mcp, status => setStatus(status));
  explorerEl.entryPoints = entryPoints;

  // Resolve every symbol to its owning project via longest-prefix
  // match on the file path. list_symbols doesn't surface project_id
  // directly, but rel_path on each project gives us the same answer
  // — and longest-prefix correctly nests sub-projects (lib/pkg/
  // playground all under standardoc-graph-viz) instead of pulling
  // their files up to the parent.
  const treeSymbols: BrowseSymbol[] = rawSymbols.map(s => rawToBrowseSymbol(s, projects));

  // One-pass index: pick a representative symbol per project so a
  // cluster click in the overview has a focal target to drill into.
  // Preference rules: symbol whose module exactly equals the project
  // label (the canonical root) → then shortest FQDN → then alphabetical.
  for (const p of projects) {
    let best: { fqdn: string; rank: [number, number, string] } | null = null;
    for (const s of treeSymbols) {
      if (s.project_id !== p.project_id) continue;
      const moduleMatch = s.module === p.label ? 0 : 1;
      const segCount = s.fqdn.split('::').length;
      const rank: [number, number, string] = [moduleMatch, segCount, s.fqdn];
      if (best === null
        || rank[0] < best.rank[0]
        || (rank[0] === best.rank[0] && rank[1] < best.rank[1])
        || (rank[0] === best.rank[0] && rank[1] === best.rank[1] && rank[2] < best.rank[2])
      ) {
        best = { fqdn: s.fqdn, rank };
      }
    }
    if (best !== null) representativeByProjectId.set(p.project_id, best.fqdn);
  }

  // Load the workspace graph into the overview canvas. Edges still
  // come from fetchGraph (bounded ~5k — adequate for cross-project
  // aggregation), but the per-cluster symbol_count is sourced from
  // the full paginated treeSymbols set so clusters past the fetchGraph
  // cap don't render as '0 symbols'.
  setStatus('fetch graph…');
  const graph = await mcp.fetchGraph(false).catch(() => null);
  const symbolByFqdn = new Map<string, BrowseSymbol>();
  for (const s of treeSymbols) symbolByFqdn.set(s.fqdn, s);
  // Edges from fetch_graph reference fqdns that may not be in
  // treeSymbols (externals). Index those into the same map so the
  // edge endpoint→project lookup resolves cross-boundary edges too.
  for (const s of graph?.symbols ?? []) {
    if (!symbolByFqdn.has(s.fqdn)) symbolByFqdn.set(s.fqdn, s);
  }
  overview.set_payload(buildOverviewPayload(projects, treeSymbols, graph?.edges ?? [], symbolByFqdn));
  overview.fit();

  // IDE-style workspace tree built from the full paginated symbol
  // set so every indexed file shows even when the workspace has more
  // than 5000 symbols (the fetch_graph cap). The fileById index lets
  // the shell react to file clicks by spawning a synthetic SymbolDetail
  // profile listing the symbols defined in that file.
  setStatus('build tree…');
  const fileById = new Map<string, FileEntry>();
  const workspaceRoot = buildWorkspaceTree('Workspace', projects, treeSymbols, fileById);
  explorerEl.tree = [workspaceRoot];

  // File click → synthetic SymbolDetail listing the file's symbols.
  // Folder / workspace / project clicks just toggle expand + update
  // the Explorer's own selection highlight; no panel cascade.
  explorerEl.addEventListener('sd-explorer-select', ev => {
    const detail = (ev as CustomEvent<ExplorerSelectDetail>).detail;
    if (detail.fqdn !== null) return; // symbol click — handled by focus subscription
    if (detail.kind === 'file') {
      const entry = fileById.get(detail.id);
      if (entry === undefined) return;
      detailsEl.symbol = buildFileSyntheticDetail(entry);
      focusCanvas.set_payload(buildEmptyFocusPayload());
    }
  });

  // Wire search.
  searchEl.addEventListener('sd-search-query', async (ev: Event) => {
    const detail = (ev as CustomEvent<{ query: string }>).detail;
    const q = detail.query.trim();
    if (q.length < 2) {
      searchEl.results = [];
      return;
    }
    searchEl.loading = true;
    try {
      const results = await mcp.findSymbolsByPattern(q, 20);
      searchEl.results = results.map(toSymbolSearchResult);
    } catch {
      searchEl.results = [];
    } finally {
      searchEl.loading = false;
    }
  });

  // Source tab → lazy fetch get_body. Cached against the symbol FQDN
  // so re-selecting the same tab on the same symbol skips the round-
  // trip; symbol changes invalidate the cache via the panel's own
  // `symbol` setter (which clears its sourceBody).
  const sourceCache = new Map<string, string>();
  detailsEl.addEventListener('sd-symbol-tab-change', async ev => {
    const detail = (ev as CustomEvent<SymbolDetailsTabChangeDetail>).detail;
    if (detail.tab !== 'source') return;
    const sym = detailsEl.symbol;
    if (sym === null) return;
    const cached = sourceCache.get(sym.fqdn);
    if (cached !== undefined) {
      detailsEl.sourceBody = cached;
      return;
    }
    detailsEl.sourceLoading = true;
    try {
      const res = await mcp.getBody(sym.fqdn);
      sourceCache.set(sym.fqdn, res.body);
      if (detailsEl.symbol?.fqdn === sym.fqdn) detailsEl.sourceBody = res.body;
    } catch {
      if (detailsEl.symbol?.fqdn === sym.fqdn) detailsEl.sourceBody = null;
    } finally {
      if (detailsEl.symbol?.fqdn === sym.fqdn) detailsEl.sourceLoading = false;
    }
  });

  // Hop selector drives the BFS depth on every focus fetch. Phase 3a
  // hardcoded depth=1 on the wire; now the user's choice in the focus
  // panel chips picks the real depth. 'All' (hops=0) caps at 5 — the
  // daemon's fetch_graph node-count limit fires somewhere around
  // depth=4 on hub symbols anyway, so 5 is a safe ceiling.
  let currentHops = focusEl.hops;
  focusEl.addEventListener('sd-focus-graph-hop-change', ev => {
    currentHops = (ev as CustomEvent<FocusGraphHopChangeDetail>).detail.hops;
    const fqdn = focusStore.get().current;
    if (fqdn !== null) void refreshFocus(fqdn);
  });

  // Focus → Symbol Details. Concurrent fetch token guards against a
  // stale response landing after the user has moved on to another FQDN.
  let focusToken = 0;
  async function refreshFocus(fqdn: string): Promise<void> {
    const myToken = ++focusToken;
    const depth = currentHops === 0 ? 5 : currentHops;
    detailsEl.symbol = null;
    setStatus(`fetch ${shortFqdn(fqdn)} (depth ${depth})…`);
    const [ctx, neighborhood] = await Promise.all([
      mcp.getContext(fqdn).catch(() => null),
      mcp.fetchNeighborhood(fqdn, false, depth).catch(() => null),
    ]);
    if (myToken !== focusToken) return; // a newer focus arrived
    focusCanvas.set_payload(buildFocusPayload(fqdn, ctx, neighborhood?.edges ?? [], neighborhood?.symbols ?? []));
    focusCanvas.fit();
    if (ctx === null) {
      setStatus(`get_context failed for ${shortFqdn(fqdn)}`);
      return;
    }
    const sym = buildSymbolDetail(ctx, neighborhood?.edges ?? [], fqdn);
    detailsEl.symbol = sym;
    setStatus(`ready (${entryPoints.length} entry points)`);
  }
  focusStore.subscribe(async state => {
    const fqdn = state.current;
    if (fqdn === null) {
      detailsEl.symbol = null;
      return;
    }
    await refreshFocus(fqdn);
  });

  setStatus(`ready (${entryPoints.length} entry points)`);
}

// Daemon caps `limit` at u8 (255) — we used to send 500 and the
// request died silently inside the McpBrowse catch, returning ZERO
// symbols and leaving the tree empty + 'ready (0 entry points)'.
const EP_PAGE_SIZE = 200;
const EP_MAX_PAGES = 2500; // 500k symbols ceiling — safety net only, ext:false should cap us far below this

/**
 * Single paginated walk of the workspace symbol index. Returns both
 * the full RawSymbol set (drives the IDE-style file tree) and the
 * filtered entry-points subset (drives the Explorer's Entry Points
 * section). Merging the two consumers into one pass avoids paying
 * the list_symbols round-trip cost twice.
 */
async function collectWorkspaceSymbols(
  mcp: McpBrowse,
  report: (status: string) => void,
): Promise<{ all: RawSymbol[]; entryPoints: ExplorerEntryPoint[] }> {
  const all: RawSymbol[] = [];
  const entryPoints: ExplorerEntryPoint[] = [];
  let cursor: string | undefined;
  let page = 0;
  while (page < EP_MAX_PAGES) {
    page++;
    // externals: false drops dependency crate symbols server-side.
    // Builtins ('<builtin>::*') aren't covered by that flag so we
    // also filter them client-side below — they otherwise drown the
    // workspace symbols under hundreds of pages on cold start.
    const res = await mcp.listSymbols({ limit: EP_PAGE_SIZE, externals: false, cursor }).catch(e => {
      // Log instead of silently breaking so a daemon-side regression
      // (param shape changed, limit cap tightened, etc.) surfaces in
      // the console rather than showing up as a magically empty tree.
      // eslint-disable-next-line no-console
      console.warn('[shell] listSymbols failed:', e);
      return null;
    });
    if (res === null) break;
    for (const s of res.items) {
      if (s.language_kind === 'builtin' || s.location.file.startsWith('<builtin>')) {
        continue;
      }
      all.push(s);
      if (typeof s.entry_point === 'string' && s.entry_point.length > 0) {
        entryPoints.push({
          fqdn: s.fqdn,
          label: shortFqdn(s.fqdn),
          kind: s.entry_point as EntryPointKind,
        });
      }
    }
    report(`workspace symbols… (page ${page}, ${all.length} kept, ${entryPoints.length} entry points)`);
    if (res.next_cursor === undefined || res.next_cursor === null || res.next_cursor.length === 0) break;
    cursor = res.next_cursor;
  }
  return { all, entryPoints };
}

/**
 * Convert a list_symbols RawSymbol into the flatter BrowseSymbol shape
 * the tree builder consumes. list_symbols doesn't carry project_id, so
 * we resolve it via longest-prefix match on the file path — sub-project
 * paths (lib/pkg/playground inside standardoc-graph-viz) bind to the
 * deepest matching project rather than the parent crate.
 */
function rawToBrowseSymbol(s: RawSymbol, projects: ReadonlyArray<ProjectLike>): BrowseSymbol {
  const file = s.location.file;
  return {
    fqdn: s.fqdn,
    name: s.name,
    kind: s.kind,
    visibility: s.visibility,
    module: s.module,
    language_kind: s.language_kind,
    language: '',
    is_external: false,
    file,
    start_line: s.location.start_line,
    project_id: inferProjectId(file, projects),
  };
}

function inferProjectId(filePath: string, projects: ReadonlyArray<ProjectLike>): number | null {
  if (!filePath) return null;
  const norm = filePath.replace(/\\/g, '/');
  let best: { id: number; len: number } | null = null;
  for (const p of projects) {
    const prefix = p.rel_path.replace(/\\/g, '/');
    if (prefix.length === 0) continue;
    if (norm === prefix || norm.startsWith(`${prefix}/`)) {
      if (best === null || prefix.length > best.len) {
        best = { id: p.project_id, len: prefix.length };
      }
    }
  }
  return best?.id ?? null;
}

interface DirNode {
  readonly children: Map<string, DirNode>;
  readonly files: Map<string, BrowseSymbol[]>;
}

function emptyDir(): DirNode {
  return { children: new Map(), files: new Map() };
}

interface FileEntry {
  readonly id: string;
  readonly path: string;
  readonly projectLabel: string;
  readonly symbols: ReadonlyArray<BrowseSymbol>;
}

interface ProjectLike {
  readonly project_id: number;
  readonly label: string;
  readonly rel_path: string;
}

interface PathTrieNode {
  /** Project bound at this exact path (rel_path === idPath), if any. */
  project?: ProjectLike;
  /** Sub-segments under this node. */
  children: Map<string, PathTrieNode>;
}

function emptyTrie(): PathTrieNode {
  return { children: new Map() };
}

/**
 * IDE-style workspace tree. We project every project's rel_path onto
 * a path trie so siblings under shared directories nest properly:
 * `crates/standardoc-graph-viz/{lib,pkg,playground}` end up as
 * children of `standardoc-graph-viz` rather than four flat entries
 * under `crates`. Labels are taken from the path segment (matching
 * what you'd see in any file explorer); the daemon-provided project
 * label sits in `title` so hover surfaces the canonical name without
 * polluting the visible label with crate-system suffixes.
 *
 * If a project's directory is ALSO an ancestor of other projects, it
 * renders as both project + folder: its own file tree merges with the
 * sub-projects' nodes under one combined entry.
 */
function buildWorkspaceTree(
  workspaceLabel: string,
  projects: ReadonlyArray<ProjectLike>,
  allSymbols: ReadonlyArray<BrowseSymbol>,
  fileById: Map<string, FileEntry>,
): ExplorerTreeNode {
  const trie = emptyTrie();
  for (const p of projects) {
    const segs = p.rel_path.replace(/\\/g, '/').split('/').filter(Boolean);
    let cur = trie;
    for (const seg of segs) {
      let next = cur.children.get(seg);
      if (next === undefined) {
        next = emptyTrie();
        cur.children.set(seg, next);
      }
      cur = next;
    }
    cur.project = p;
  }

  return {
    id: 'workspace',
    label: workspaceLabel,
    kind: 'workspace',
    children: trieToExplorerNodes(trie, 'ws', allSymbols, fileById),
  };
}

function trieToExplorerNodes(
  trie: PathTrieNode,
  idPrefix: string,
  allSymbols: ReadonlyArray<BrowseSymbol>,
  fileById: Map<string, FileEntry>,
): ExplorerTreeNode[] {
  const out: ExplorerTreeNode[] = [];
  for (const name of [...trie.children.keys()].sort((a, b) => a.localeCompare(b))) {
    const child = trie.children.get(name);
    if (child === undefined) continue;
    const childId = `${idPrefix}/${name}`;
    const subProjectNodes = trieToExplorerNodes(child, childId, allSymbols, fileById);
    if (child.project !== undefined) {
      // This trie level is a real project. Render with project kind,
      // path-segment as the visible label, daemon label as tooltip-
      // shaped metadata. Merge sub-project entries with the project's
      // own file tree under one combined children array.
      const project = child.project;
      const projectNode = buildProjectNode(project, allSymbols, fileById);
      const merged: ExplorerTreeNode[] = [
        ...subProjectNodes,
        ...(projectNode.children ?? []),
      ];
      out.push({
        id: childId,
        label: name,
        kind: 'project',
        children: merged.length > 0 ? merged : undefined,
        fqdn: null,
        description: `${project.label} (${project.rel_path})`,
      });
    } else {
      // Pure folder — only purpose is to nest sub-projects.
      out.push({
        id: childId,
        label: name,
        kind: 'folder',
        children: subProjectNodes,
      });
    }
  }
  return out;
}

function buildProjectNode(
  project: { project_id: number; label: string; rel_path: string },
  allSymbols: ReadonlyArray<BrowseSymbol>,
  fileById: Map<string, FileEntry>,
): ExplorerTreeNode {
  const root = emptyDir();
  let touchedFiles = 0;
  for (const s of allSymbols) {
    if (s.project_id !== project.project_id) continue;
    if (!s.file || s.file.length === 0) continue;
    const rel = stripProjectPrefix(s.file, project.rel_path);
    if (rel === null || rel.length === 0) continue;
    const parts = rel.split(/[/\\]/).filter(p => p.length > 0);
    if (parts.length === 0) continue;
    const fileName = parts[parts.length - 1];
    if (fileName === undefined) continue;
    const dirs = parts.slice(0, -1);
    let cur = root;
    for (const d of dirs) {
      let next = cur.children.get(d);
      if (next === undefined) {
        next = emptyDir();
        cur.children.set(d, next);
      }
      cur = next;
    }
    const bucket = cur.files.get(fileName);
    if (bucket === undefined) {
      cur.files.set(fileName, [s]);
      touchedFiles++;
    } else {
      bucket.push(s);
    }
  }
  const id = `project:${project.project_id}`;
  const children = touchedFiles > 0
    ? dirToNodes(root, id, project.label, project.rel_path, fileById)
    : undefined;
  return {
    id,
    label: project.label,
    kind: 'project',
    children,
  };
}

function dirToNodes(
  dir: DirNode,
  idPrefix: string,
  projectLabel: string,
  currentPath: string,
  fileById: Map<string, FileEntry>,
): ExplorerTreeNode[] {
  const out: ExplorerTreeNode[] = [];
  for (const name of [...dir.children.keys()].sort()) {
    const child = dir.children.get(name);
    if (child === undefined) continue;
    const id = `${idPrefix}/${name}`;
    const subPath = currentPath.length > 0 ? `${currentPath}/${name}` : name;
    out.push({
      id,
      label: name,
      kind: 'folder',
      children: dirToNodes(child, id, projectLabel, subPath, fileById),
    });
  }
  for (const name of [...dir.files.keys()].sort()) {
    const symbols = (dir.files.get(name) ?? []).slice().sort((a, b) => a.start_line - b.start_line);
    const id = `${idPrefix}/${name}`;
    const filePath = currentPath.length > 0 ? `${currentPath}/${name}` : name;
    fileById.set(id, { id, path: filePath, projectLabel, symbols });
    out.push({
      id,
      label: name,
      kind: 'file',
      children: symbols.map(s => ({
        id: `sym:${s.fqdn}`,
        label: s.name,
        kind: mapBrowseSymbolKind(s),
        fqdn: s.fqdn,
      })),
    });
  }
  return out;
}

function buildFileSyntheticDetail(file: FileEntry): SymbolDetail {
  const name = file.path.split('/').pop() ?? file.path;
  return {
    fqdn: `file:${file.path}`,
    name,
    kindLabel: 'file',
    visibility: null,
    file: file.path,
    startLine: 1,
    documentation: `${file.symbols.length} symbol${file.symbols.length === 1 ? '' : 's'} defined in this file · project: ${file.projectLabel}`,
    entryPointKind: null,
    fieldCount: 0,
    methodCount: 0,
    relations: [{
      kind: 'definedHere',
      items: file.symbols.map(s => ({
        fqdn: s.fqdn,
        label: s.name,
        kindLabel: s.language_kind ?? s.kind,
      })),
      total: file.symbols.length,
    }],
  };
}

function buildEmptyFocusPayload(): string {
  return JSON.stringify({ center: null, neighbors: [], edges: [] });
}

function stripProjectPrefix(filePath: string, projectRelPath: string): string | null {
  const norm = (p: string) => p.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  const file = norm(filePath);
  const prefix = norm(projectRelPath);
  if (prefix.length === 0) return file;
  if (file === prefix) return '';
  if (file.startsWith(`${prefix}/`)) return file.slice(prefix.length + 1);
  return null;
}

function mapBrowseSymbolKind(s: BrowseSymbol): ExplorerNodeKind {
  const lk = s.language_kind;
  if (lk === 'struct') return 'struct';
  if (lk === 'enum') return 'enum';
  if (lk === 'fn' || lk === 'function' || lk === 'method') return 'function';
  if (lk === 'trait' || lk === 'interface') return 'trait';
  if (lk === 'const' || lk === 'static') return 'value';
  if (lk === 'macro' || lk === 'macro_rules') return 'macro';
  switch (s.kind) {
    case 'type': return 'struct';
    case 'callable': return 'function';
    case 'value': return 'value';
    case 'macro': return 'macro';
    default: return 'unknown';
  }
}

function buildOverviewPayload(
  projects: ReadonlyArray<{ project_id: number; label: string; kind: { kind: string } }>,
  symbols: ReadonlyArray<BrowseSymbol>,
  edges: ReadonlyArray<{ from: string; to: string; kind: string; outbound: boolean }>,
  symbolByFqdn: Map<string, BrowseSymbol>,
): string {
  const counts = new Map<number, number>();
  for (const s of symbols) {
    if (s.project_id === undefined || s.project_id === null) continue;
    counts.set(s.project_id, (counts.get(s.project_id) ?? 0) + 1);
  }
  const clusters = projects.map(p => ({
    id: p.project_id,
    label: p.label,
    kind: p.kind.kind,
    symbol_count: counts.get(p.project_id) ?? 0,
  }));
  const aggregated = new Map<string, { from: number; to: number; weight: number }>();
  for (const e of edges) {
    const from = symbolByFqdn.get(e.from)?.project_id;
    const to = symbolByFqdn.get(e.to)?.project_id;
    if (from === undefined || from === null) continue;
    if (to === undefined || to === null) continue;
    if (from === to) continue;
    const key = `${from}->${to}`;
    const bucket = aggregated.get(key);
    if (bucket === undefined) aggregated.set(key, { from, to, weight: 1 });
    else bucket.weight += 1;
  }
  return JSON.stringify({
    clusters,
    edges: [...aggregated.values()],
  });
}

function buildFocusPayload(
  fqdn: string,
  ctx: GetContextResponse | null,
  neighborhoodEdges: ReadonlyArray<{ from: string; to: string; kind: string; outbound: boolean }>,
  neighborhoodSymbols: ReadonlyArray<BrowseSymbol>,
): string {
  const centerSym = ctx?.context.symbol;
  const center = centerSym !== undefined ? {
    fqdn: centerSym.fqdn,
    name: centerSym.name,
    kind: centerSym.decl_kind ?? centerSym.language_kind ?? centerSym.kind,
    depth: 0,
  } : null;
  // BFS depth per neighbour: shortest hop count from the focal symbol.
  // fetch_graph doesn't surface per-node depth in the wire payload,
  // so we reconstruct it client-side by walking the edges. Phase 3c
  // canvas only renders the data we send, no further filtering.
  const depthByFqdn = computeDepthFromFocal(fqdn, neighborhoodEdges);
  const neighbors = neighborhoodSymbols
    .filter(s => s.fqdn !== fqdn)
    .map(s => ({
      fqdn: s.fqdn,
      name: s.name,
      kind: s.language_kind ?? s.kind,
      depth: depthByFqdn.get(s.fqdn) ?? 1,
    }));
  const focalEdges = neighborhoodEdges.map(e => ({
    from: e.from,
    to: e.to,
    kind: e.kind,
    depth: Math.max(depthByFqdn.get(e.from) ?? 0, depthByFqdn.get(e.to) ?? 0),
  }));
  return JSON.stringify({ center, neighbors, edges: focalEdges });
}

function computeDepthFromFocal(
  focal: string,
  edges: ReadonlyArray<{ from: string; to: string; outbound: boolean }>,
): Map<string, number> {
  const adj = new Map<string, Set<string>>();
  const pushAdj = (a: string, b: string): void => {
    let set = adj.get(a);
    if (set === undefined) { set = new Set(); adj.set(a, set); }
    set.add(b);
  };
  for (const e of edges) {
    pushAdj(e.from, e.to);
    pushAdj(e.to, e.from);
  }
  const depths = new Map<string, number>();
  depths.set(focal, 0);
  const queue: string[] = [focal];
  while (queue.length > 0) {
    const cur = queue.shift()!;
    const here = depths.get(cur) ?? 0;
    for (const next of adj.get(cur) ?? []) {
      if (depths.has(next)) continue;
      depths.set(next, here + 1);
      queue.push(next);
    }
  }
  return depths;
}

function shortFqdn(fqdn: string): string {
  const idx = fqdn.lastIndexOf('::');
  return idx >= 0 ? fqdn.slice(idx + 2) : fqdn;
}

function toSymbolSearchResult(s: RawSymbol): SymbolSearchResult {
  return {
    fqdn: s.fqdn,
    name: s.name,
    kindLabel: s.decl_kind ?? s.language_kind ?? s.kind,
    file: s.location.file,
    startLine: s.location.start_line,
  };
}

function buildSymbolDetail(
  ctx: GetContextResponse,
  neighborhoodEdges: ReadonlyArray<{ from: string; to: string; kind: string; outbound: boolean }>,
  fqdn: string,
): SymbolDetail {
  const sym = ctx.context.symbol;
  const doc = ctx.context.document_description ?? ctx.context.enrichment_description;
  const epKind = (typeof sym.entry_point === 'string' ? sym.entry_point : null) as EntryPointKind | null;

  // Build relation buckets from a combination of get_context (callers /
  // callees / imports / imported_by — CALLS + IMPORTS edges only) and
  // the focal neighborhood (every edge kind). Bucket by UI relation
  // kind so the panel reads "Used by (n)" / "Uses types (n)" etc.
  const buckets = new Map<SymbolRelationKind, Map<string, { fqdn: string; label: string; kindLabel: string }>>();
  const pushBucket = (kind: SymbolRelationKind, fq: string, kindLabel: string): void => {
    if (fq === fqdn) return;
    let m = buckets.get(kind);
    if (m === undefined) { m = new Map(); buckets.set(kind, m); }
    if (!m.has(fq)) m.set(fq, { fqdn: fq, label: shortFqdn(fq), kindLabel });
  };

  for (const e of ctx.callers) {
    if (e.target.fqdn) pushBucket('usedBy', e.target.fqdn, 'fn');
  }
  for (const e of ctx.callees) {
    if (e.target.fqdn) pushBucket('calls', e.target.fqdn, 'fn');
  }
  for (const e of ctx.imports) {
    if (e.target.fqdn) pushBucket('importedBy', e.target.fqdn, 'mod');
  }
  for (const e of ctx.imported_by) {
    if (e.target.fqdn) pushBucket('importedBy', e.target.fqdn, 'mod');
  }

  // Walk the focal neighborhood — every edge kind, both directions.
  for (const e of neighborhoodEdges) {
    const other = e.outbound ? e.to : e.from;
    const kindLabel = '';
    switch (e.kind) {
      case 'CALLS':
        if (e.outbound) pushBucket('calls', other, kindLabel);
        else pushBucket('usedBy', other, kindLabel);
        break;
      case 'IMPORTS':
        pushBucket('importedBy', other, 'mod');
        break;
      case 'USES_TYPE':
      case 'REFERENCES':
        if (e.outbound) pushBucket('usesTypes', other, kindLabel);
        else pushBucket('usedBy', other, kindLabel);
        break;
      case 'TESTS':
        if (e.outbound) pushBucket('calls', other, kindLabel);
        else pushBucket('testedBy', other, 'test');
        break;
      case 'IMPLEMENTS':
        if (e.outbound) pushBucket('implements', other, kindLabel);
        break;
      case 'EXTENDS':
        if (e.outbound) pushBucket('extends', other, kindLabel);
        break;
    }
  }

  const orderedKinds: SymbolRelationKind[] = [
    'usedBy', 'usesTypes', 'calls', 'importedBy', 'testedBy', 'implements', 'extends',
  ];
  const relations: SymbolRelationBucket[] = [];
  for (const k of orderedKinds) {
    const m = buckets.get(k);
    if (m === undefined || m.size === 0) continue;
    const items = [...m.values()];
    relations.push({ kind: k, items, total: items.length });
  }

  return {
    fqdn: sym.fqdn,
    name: sym.name,
    kindLabel: sym.decl_kind ?? sym.language_kind ?? sym.kind,
    visibility: sym.visibility,
    file: sym.location.file,
    startLine: sym.location.start_line,
    documentation: doc,
    entryPointKind: epKind,
    fieldCount: 0,
    methodCount: 0,
    relations,
  };
}

boot().catch((e: unknown) => {
  const msg = e instanceof Error ? e.message : String(e);
  setStatus(`fatal: ${msg}`);
  console.error('[shell] boot failed', e);
});
