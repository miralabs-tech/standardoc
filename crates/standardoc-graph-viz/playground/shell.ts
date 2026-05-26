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
import '@standarx/standardoc-viz/components/compare-panel';
import '@standarx/standardoc-viz/components/panel-host';
import '@standarx/standardoc-viz/components/legend';

import { focusStore } from '@standarx/standardoc-viz/focus-store';
import { panelManager } from '@standarx/standardoc-viz/panel-manager';
import { viewPrefsStore } from '@standarx/standardoc-viz/view-prefs-store';
import { McpBrowse } from '@standarx/standardoc-viz/mcp-client';
import type {
  BrowseSymbol,
  GetContextResponse,
  RawSymbol,
} from '@standarx/standardoc-viz/mcp-client';
import type {
  ComparePanelElement,
  CompareRefreshRequestDetail,
  ExplorerElement,
  ExplorerEntryPoint,
  ExplorerNodeKind,
  ExplorerSelectDetail,
  ExplorerTreeNode,
  ExplorerTreeView,
  ExplorerViewChangeDetail,
  EntryPointKind,
  FocusGraphElement,
  FocusGraphErrorDetail,
  FocusGraphHopChangeDetail,
  FocusGraphNodeClickDetail,
  OverviewClusterClickDetail,
  OverviewElement,
  OverviewErrorDetail,
  PanelHostElement,
  PanelLayoutElement,
  PanelSlot,
  SearchElement,
  SymbolDetail,
  SymbolDetailsActionDetail,
  SymbolDetailsElement,
  SymbolDetailsTabChangeDetail,
  SymbolRelationBucket,
  SymbolRelationKind,
  SymbolSearchResult,
  SymbolSubItem,
} from '@standarx/standardoc-viz';

const explorerEl = document.getElementById('explorer') as ExplorerElement;
const detailsEl = document.getElementById('details') as SymbolDetailsElement;
const searchEl = document.getElementById('search') as SearchElement;
const overviewEl = document.getElementById('overview') as OverviewElement;
const focusEl = document.getElementById('focus') as FocusGraphElement;
const panelsEl = document.getElementById('panels') as PanelHostElement;
const statusEl = document.getElementById('status') as HTMLSpanElement;
const shellEl = document.getElementById('shell') as PanelLayoutElement;

// Wire toolbar panel toggle buttons → `panelLayout.togglePanel(slot)`.
// The element handles the actual show/hide + persistence; the host
// only needs to keep the button `aria-pressed` state in sync with the
// real state (covers both user-driven toggles and persisted state
// restored from a previous session).
const toggleBtns = document.querySelectorAll<HTMLButtonElement>('button[data-toggle-panel]');
const syncToggleBtns = (): void => {
  const vis = shellEl.state.visibility;
  toggleBtns.forEach(btn => {
    const slot = btn.dataset['togglePanel'] as PanelSlot | undefined;
    if (slot === undefined) return;
    btn.setAttribute('aria-pressed', vis[slot] ? 'true' : 'false');
  });
};
toggleBtns.forEach(btn => {
  btn.addEventListener('click', () => {
    const slot = btn.dataset['togglePanel'] as PanelSlot | undefined;
    if (slot === undefined) return;
    shellEl.togglePanel(slot);
  });
});
shellEl.addEventListener('sd-panel-layout-change', () => { syncToggleBtns(); });
// Initial sync — the element may have restored a persisted layout
// on `connectedCallback` before we wired the listener.
syncToggleBtns();

// Shell-wide "hide tests" toggle — writes through the shared
// `viewPrefsStore` so every panel that subscribes (Symbol Details
// today, focus/explorer/overview as they get wired) sees the same
// state without per-panel local copies drifting apart.
const hideTestsBtn = document.getElementById('hide-tests-toggle') as HTMLButtonElement | null;
if (hideTestsBtn !== null) {
  const syncHideTestsBtn = (excludeTests: boolean): void => {
    hideTestsBtn.setAttribute('aria-pressed', excludeTests ? 'true' : 'false');
  };
  syncHideTestsBtn(viewPrefsStore.get().excludeTests);
  hideTestsBtn.addEventListener('click', () => {
    viewPrefsStore.setPrefs({ excludeTests: !viewPrefsStore.get().excludeTests });
  });
  viewPrefsStore.subscribe(state => syncHideTestsBtn(state.excludeTests));
}

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

  // Overview scope is driven from Explorer clicks (folder / project)
  // and from cluster clicks in the canvas itself (workspace mode
  // drills into a project, project/folder mode focuses a module
  // representative). `clusterTargets` is rebuilt on every scope
  // change; the cluster click handler defined below consults it to
  // dispatch. `applyOverviewScope` is bound to a real implementation
  // once the workspace data is fetched (it captures `projects`,
  // `treeSymbols`, `symbolByFqdn`, `graphEdges`); the listeners below
  // reference it through this closure box.
  let clusterTargets = new Map<number, ClusterTarget>();
  let currentOverviewScope: OverviewScope = { kind: 'workspace' };
  let applyOverviewScope: (next: OverviewScope) => void = () => {};

  overviewEl.addEventListener('sd-overview-cluster-click', e => {
    const { clusterId } = (e as CustomEvent<OverviewClusterClickDetail>).detail;
    const target = clusterTargets.get(clusterId);
    if (target === undefined) return;
    switch (target.kind) {
      case 'drill-project':
        // Workspace → project: enter the project's module hierarchy.
        // We use the `module` scope (FQDN prefix) so subsequent drills
        // share the same "one segment deeper at a time" semantics.
        applyOverviewScope({ kind: 'module', prefix: target.label, label: target.label });
        break;
      case 'drill-folder':
        applyOverviewScope({
          kind: 'folder',
          projectId: target.projectId,
          label: target.label,
          relPath: target.relPath,
        });
        break;
      case 'drill-module':
        applyOverviewScope({ kind: 'module', prefix: target.prefix, label: target.label });
        break;
      case 'focus-symbol':
        focusStore.setFocus(target.fqdn);
        break;
    }
  });

  overviewEl.addEventListener('sd-overview-back', () => {
    // Pop one FQDN segment when in module scope; only `workspace`
    // when the popped prefix would be empty. Other scopes (project /
    // folder, coming from Explorer) jump straight back to workspace
    // since they're not segment-stacked.
    const current = currentOverviewScope;
    if (current.kind === 'module') {
      const segs = current.prefix.split('::');
      segs.pop();
      if (segs.length === 0) {
        applyOverviewScope({ kind: 'workspace' });
      } else {
        const prefix = segs.join('::');
        applyOverviewScope({ kind: 'module', prefix, label: segs[segs.length - 1]! });
      }
      return;
    }
    applyOverviewScope({ kind: 'workspace' });
  });

  // Click on a focus-graph node → shift global focus.
  focusEl.addEventListener('sd-focus-graph-node-click', e => {
    const { fqdn } = (e as CustomEvent<FocusGraphNodeClickDetail>).detail;
    focusStore.setFocus(fqdn);
  });

  // Focus breadcrumb back button → real history navigation. We can't
  // use `focusStore.recent` for this because that ring is sorted MRU
  // and re-promotes whatever we navigate to; clicking back twice with
  // it just toggles between the last two focals. Instead we maintain
  // a dedicated linear back-stack synced via `focusStore.subscribe`:
  // every forward focus change pushes the OUTGOING focal onto the
  // stack; every back action pops it. An `isBackNav` flag keeps the
  // subscribe handler from double-counting our own pop.
  const focusBackStack: string[] = [];
  let isBackNav = false;
  let lastFocus: string | null = focusStore.get().current;
  focusStore.subscribe(state => {
    if (isBackNav) {
      isBackNav = false;
    } else if (lastFocus !== null && state.current !== lastFocus) {
      focusBackStack.push(lastFocus);
    }
    lastFocus = state.current;
  });
  focusEl.addEventListener('sd-focus-graph-back', () => {
    if (focusBackStack.length > 0) {
      const prev = focusBackStack.pop()!;
      isBackNav = true;
      focusStore.setFocus(prev);
      return;
    }
    // No deeper history — un-focus so the user lands on the empty
    // state instead of a dead button.
    if (focusStore.get().current !== null) {
      isBackNav = true;
      focusStore.setFocus(null);
    }
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

  // Load the workspace graph for inter-cluster edge aggregation. Edges
  // come from fetchGraph (bounded ~5k) but the per-cluster symbol_count
  // is sourced from the full paginated treeSymbols set so clusters past
  // the fetchGraph cap don't render as '0 symbols'.
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
  const graphEdges = graph?.edges ?? [];

  // Bind the scope handler now that the workspace data is in scope.
  // Initial render = workspace mode (paints projects + project edges).
  // Applies the shell-wide `excludeTests` toggle: when on, both the
  // symbol set and the edge endpoints get filtered to drop test-
  // shaped nodes before the payload is built.
  applyOverviewScope = (next: OverviewScope) => {
    currentOverviewScope = next;
    const excludeTests = viewPrefsStore.get().excludeTests;
    const scopedSymbols = excludeTests
      ? treeSymbols.filter(s => !looksLikeTest(s.fqdn, s.file))
      : treeSymbols;
    const allowedFqdns = excludeTests
      ? new Set(scopedSymbols.map(s => s.fqdn))
      : null;
    const scopedEdges = excludeTests && allowedFqdns !== null
      ? graphEdges.filter(e => allowedFqdns.has(e.from) && allowedFqdns.has(e.to))
      : graphEdges;
    const built = buildOverviewPayloadForScope(next, projects, scopedSymbols, scopedEdges, symbolByFqdn);
    clusterTargets = built.targets;
    overview.set_payload(built.json);
    // Workspace = label only depth-0 root packages so the high-level
    // view stays readable on workspaces with hundreds of modules. Any
    // drilled scope is small enough to label every node.
    overview.set_label_depth_cap(next.kind === 'workspace' ? 0 : 0xffff_ffff);
    overview.fit();
    overviewEl.crossEdgeKinds = built.crossKinds;
    overviewEl.scopeLabel = scopeBreadcrumbLabel(next);
  };
  applyOverviewScope({ kind: 'workspace' });

  // Re-trigger Overview + Focus + Explorer tree on hide-tests toggle
  // so all three views pick up the new filter without the user
  // manually re-navigating. Symbol Details panel subscribes on its
  // own. The Explorer tree rebuild happens later in this boot()
  // (after `rebuildTree` is defined); we attach the same subscribe
  // there so the closure has access to it.
  viewPrefsStore.subscribe(() => {
    applyOverviewScope(currentOverviewScope);
    const fqdn = focusStore.get().current;
    if (fqdn !== null) void refreshFocus(fqdn);
  });

  // Seed the header search empty state. Entry points (top 5,
  // binary_main + public_api priority) surface the workspace API
  // surface one click away; recents track focusStore so the user can
  // jump back without retyping. Both reuse the BrowseSymbol index so
  // file:line is resolved when known (drops gracefully to undefined
  // for builtins / re-exports without a recorded location).
  const browseToSearchResult = (s: BrowseSymbol): SymbolSearchResult => ({
    fqdn: s.fqdn,
    name: s.name,
    kindLabel: s.kind,
    file: s.file,
    startLine: s.start_line,
  });
  const fqdnToSearchResult = (fqdn: string): SymbolSearchResult => {
    const sym = symbolByFqdn.get(fqdn);
    if (sym !== undefined) return browseToSearchResult(sym);
    return { fqdn, name: shortFqdn(fqdn), kindLabel: 'symbol' };
  };
  // Stable entry-point ranking : binary_main → public_api → ffi_export → alpha.
  const epRank = (k: string): number => k === 'binary_main' ? 0 : k === 'public_api' ? 1 : k === 'ffi_export' ? 2 : 3;
  const sortedEntryPoints = [...entryPoints].sort((a, b) => {
    const dr = epRank(a.kind) - epRank(b.kind);
    return dr !== 0 ? dr : a.label.localeCompare(b.label);
  });
  searchEl.entryPoints = sortedEntryPoints.slice(0, 5).map(ep => {
    const sym = symbolByFqdn.get(ep.fqdn);
    return {
      fqdn: ep.fqdn,
      name: sym?.name ?? ep.label,
      kindLabel: ep.kind,
      file: sym?.file,
      startLine: sym?.start_line,
    };
  });
  const pushRecentsToSearch = () => {
    searchEl.recents = focusStore.get().recent.slice(0, 3).map(fqdnToSearchResult);
  };
  pushRecentsToSearch();
  focusStore.subscribe(() => pushRecentsToSearch());

  // Auto-scope the Overview to the module containing the focused
  // symbol. Without this, navigating via Explorer / Focus Graph /
  // Search left the 3D view stuck on the previous scope — the user
  // had to drill manually to see where the new focal sits. Skips when
  // the focal is already inside the current scope (drill stays put),
  // and ignores transitions where we already drilled to that exact
  // module (no-op re-render).
  focusStore.subscribe(state => {
    const fqdn = state.current;
    if (fqdn === null) return;
    const sym = symbolByFqdn.get(fqdn);
    if (sym === undefined || sym.module === null || sym.module.length === 0) return;
    const targetModule = sym.module;
    const current = currentOverviewScope;
    if (current.kind === 'module' && current.prefix === targetModule) return;
    // If the focused symbol's module is already under the active drill
    // scope (workspace, project, folder, or a shallower module), don't
    // re-scope — the user can drill deeper themselves.
    if (current.kind === 'module' && targetModule.startsWith(`${current.prefix}::`)) return;
    const label = targetModule.split('::').pop() ?? targetModule;
    applyOverviewScope({ kind: 'module', prefix: targetModule, label });
  });

  // IDE-style workspace tree built from the full paginated symbol
  // set so every indexed file shows even when the workspace has more
  // than 5000 symbols (the fetch_graph cap). The fileById index lets
  // the shell react to file clicks by spawning a synthetic SymbolDetail
  // profile listing the symbols defined in that file.
  setStatus('build tree…');
  const treeOut: TreeOut = {
    fileById: new Map(),
    folderById: new Map(),
    projectByExplorerId: new Map(),
  };
  const rebuildTree = (view: ExplorerTreeView): void => {
    treeOut.fileById.clear();
    treeOut.folderById.clear();
    treeOut.projectByExplorerId.clear();
    // Same `excludeTests` filter the Focus + Overview wiring applies
    // so the three views agree on what counts as "production". The
    // tree builders walk symbols' file paths; passing them a pre-
    // filtered set drops the test files entirely (their parent
    // folders / projects collapse if they end up empty).
    const symbols = viewPrefsStore.get().excludeTests
      ? treeSymbols.filter(s => !looksLikeTest(s.fqdn, s.file))
      : treeSymbols;
    let root: ExplorerTreeNode;
    if (view === 'projects') {
      root = buildProjectsTreeFlat('Workspace', projects, symbols, treeOut);
    } else if (view === 'modules') {
      root = buildModulesTree('Workspace', projects, symbols, treeOut);
    } else {
      root = buildWorkspaceTree('Workspace', projects, symbols, treeOut);
    }
    explorerEl.tree = [root];
  };
  rebuildTree(explorerEl.treeView);
  explorerEl.addEventListener('sd-explorer-view-change', ev => {
    const detail = (ev as CustomEvent<ExplorerViewChangeDetail>).detail;
    rebuildTree(detail.view);
  });
  // Pick up shell-wide hide-tests toggles for the tree too. The
  // canvas-backed views have their own subscribe above; this one
  // handles the DOM tree which can't share that closure (the tree
  // builder lives later in boot()).
  viewPrefsStore.subscribe(() => rebuildTree(explorerEl.treeView));
  const { fileById, folderById, projectByExplorerId } = treeOut;

  // File click → synthetic SymbolDetail listing the file's symbols.
  // Project / folder click → switch the Overview scope so the canvas
  // paints modules inside that scope rather than the workspace
  // projects. Workspace-level folders (no project bound) are still
  // navigational scaffolding — no scope cascade for them.
  explorerEl.addEventListener('sd-explorer-select', ev => {
    const detail = (ev as CustomEvent<ExplorerSelectDetail>).detail;
    if (detail.fqdn !== null) return; // symbol click — handled by focus subscription
    if (detail.kind === 'file') {
      const entry = fileById.get(detail.id);
      if (entry === undefined) return;
      detailsEl.symbol = buildFileSyntheticDetail(entry);
      focusCanvas.set_payload(buildEmptyFocusPayload());
      return;
    }
    if (detail.kind === 'project') {
      const proj = projectByExplorerId.get(detail.id);
      if (proj === undefined) return;
      applyOverviewScope({ kind: 'project', projectId: proj.project_id, label: proj.label });
      return;
    }
    if (detail.kind === 'folder') {
      const folder = folderById.get(detail.id);
      if (folder === undefined) return;
      applyOverviewScope({
        kind: 'folder',
        projectId: folder.projectId,
        label: folder.projectLabel,
        relPath: folder.relPath,
      });
    }
  });

  // Wire search. Fan out to both `find_symbol` (FTS5 ranked by name +
  // fqdn tokens) and `find_symbols_by_pattern` (glob substring match)
  // in parallel — merging the two catches a broader scoring surface
  // than either alone (FTS misses middle-substring hits, pattern
  // misses fuzzy / tokenized matches). FTS results lead; pattern hits
  // fill in by fqdn dedup. When FTS returns 0, its strsim
  // `did_you_mean` suggestions surface as a "Did you mean…" section.
  searchEl.addEventListener('sd-search-query', async (ev: Event) => {
    const detail = (ev as CustomEvent<{ query: string }>).detail;
    const q = detail.query.trim();
    if (q.length === 0) {
      searchEl.results = [];
      searchEl.suggestions = [];
      return;
    }
    searchEl.loading = true;
    try {
      const [fts, pattern] = await Promise.all([
        mcp.findSymbol(q, 20).catch(() => ({ results: [] as ReadonlyArray<RawSymbol>, suggestions: [] })),
        mcp.findSymbolsByPattern(`*${q}*`, 20).catch(() => [] as ReadonlyArray<RawSymbol>),
      ]);
      const seen = new Set<string>();
      const merged: RawSymbol[] = [];
      for (const r of fts.results) {
        if (seen.has(r.fqdn)) continue;
        seen.add(r.fqdn);
        merged.push(r);
      }
      for (const r of pattern) {
        if (seen.has(r.fqdn)) continue;
        seen.add(r.fqdn);
        merged.push(r);
      }
      searchEl.results = merged.slice(0, 20).map(toSymbolSearchResult);
      searchEl.suggestions = fts.suggestions.map(s => ({
        fqdn: s.fqdn,
        name: s.name,
        kindLabel: s.kind,
      }));
    } catch {
      searchEl.results = [];
      searchEl.suggestions = [];
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

  // Spawnable Compare panel — Phase 4. Pin-pattern UX: first click on
  // "Add to compare" pins the current symbol; second click on a DIFFERENT
  // symbol opens the panel with both. Clicking the same pinned symbol
  // again toggles the pin off without spawning. The status bar surfaces
  // the pending pin so the user knows the next click will spawn.
  let comparePinned: string | null = null;
  let compareToken = 0;

  detailsEl.addEventListener('sd-symbol-action', ev => {
    const detail = (ev as CustomEvent<SymbolDetailsActionDetail>).detail;
    switch (detail.action) {
      case 'add-to-compare':
        void handleAddToCompare(detail.fqdn);
        break;
      case 'copy-fqdn':
        void handleCopyFqdn(detail.fqdn);
        break;
      case 'open-in-editor':
        // Playground has no editor host to hand the symbol off to —
        // surface the intent so the user sees the click did register,
        // and let the future VSCode webview branch take over here.
        setStatus(`open in editor: ${shortFqdn(detail.fqdn)} — not available in playground`);
        break;
    }
  });

  panelsEl.addEventListener('sd-compare-refresh-request', ev => {
    const detail = (ev as CustomEvent<CompareRefreshRequestDetail>).detail;
    const target = panelsEl.activePanelElement as ComparePanelElement | null;
    if (target === null) return;
    const myToken = ++compareToken;
    void loadComparePanel(target, detail.leftFqdn, detail.rightFqdn, myToken);
  });

  async function handleCopyFqdn(fqdn: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(fqdn);
      setStatus(`copied: ${shortFqdn(fqdn)}`);
    } catch (e) {
      setStatus(`copy failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async function handleAddToCompare(fqdn: string): Promise<void> {
    if (comparePinned === fqdn) {
      comparePinned = null;
      setStatus(`compare: unpinned ${shortFqdn(fqdn)}`);
      return;
    }
    if (comparePinned === null) {
      comparePinned = fqdn;
      setStatus(`compare: pinned ${shortFqdn(fqdn)} — Add to compare another symbol to open`);
      return;
    }
    const left = comparePinned;
    const right = fqdn;
    comparePinned = null;
    await spawnComparePanel(left, right);
  }

  async function spawnComparePanel(leftFqdn: string, rightFqdn: string): Promise<void> {
    const myToken = ++compareToken;
    panelManager.open('compare', { leftFqdn, rightFqdn });
    // Sync render: panel-host subscribers fire synchronously, so the
    // compare-panel element is mounted by the time `open()` returns.
    const target = panelsEl.activePanelElement as ComparePanelElement | null;
    if (target === null) return;
    await loadComparePanel(target, leftFqdn, rightFqdn, myToken);
  }

  async function loadComparePanel(
    target: ComparePanelElement,
    leftFqdn: string,
    rightFqdn: string,
    myToken: number,
  ): Promise<void> {
    target.data = {
      left: { fqdn: leftFqdn, detail: null, loading: true },
      right: { fqdn: rightFqdn, detail: null, loading: true },
    };
    setStatus(`compare: ${shortFqdn(leftFqdn)} ↔ ${shortFqdn(rightFqdn)}…`);
    const [leftCtx, rightCtx, leftSubs, rightSubs] = await Promise.all([
      mcp.getContext(leftFqdn).catch(() => null),
      mcp.getContext(rightFqdn).catch(() => null),
      fetchSubItems(mcp, leftFqdn),
      fetchSubItems(mcp, rightFqdn),
    ]);
    if (myToken !== compareToken) return;
    target.data = {
      left: {
        fqdn: leftFqdn,
        detail: leftCtx !== null ? buildSymbolDetail(leftCtx, [], leftSubs, leftFqdn) : null,
        loading: false,
      },
      right: {
        fqdn: rightFqdn,
        detail: rightCtx !== null ? buildSymbolDetail(rightCtx, [], rightSubs, rightFqdn) : null,
        loading: false,
      },
    };
    setStatus(`compare ready (${shortFqdn(leftFqdn)} ↔ ${shortFqdn(rightFqdn)})`);
  }

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
    const [ctx, neighborhood, subItems] = await Promise.all([
      mcp.getContext(fqdn).catch(() => null),
      mcp.fetchNeighborhood(fqdn, false, depth).catch(() => null),
      fetchSubItems(mcp, fqdn),
    ]);
    if (myToken !== focusToken) return; // a newer focus arrived
    // Apply the shell-wide `excludeTests` toggle to the neighborhood
    // BEFORE building either the SymbolDetail or the focus payload.
    // Filters both endpoints (skip an edge if either endpoint is a
    // test) so the wasm canvas matches the panel's "hide tests" view.
    const excludeTests = viewPrefsStore.get().excludeTests;
    const neighborhoodSymbols = excludeTests
      ? (neighborhood?.symbols ?? []).filter(s => !looksLikeTest(s.fqdn, s.file))
      : (neighborhood?.symbols ?? []);
    const allowedFqdns = excludeTests
      ? new Set(neighborhoodSymbols.map(s => s.fqdn).concat([fqdn]))
      : null;
    const neighborhoodEdges = excludeTests && allowedFqdns !== null
      ? (neighborhood?.edges ?? []).filter(e => allowedFqdns.has(e.from) && allowedFqdns.has(e.to))
      : (neighborhood?.edges ?? []);
    // Build SymbolDetail first so its fields/methods arrays feed the
    // focus payload's centre-card footer ("N fields · N methods").
    // The build is purely TS-side, no extra MCP round-trip.
    let fieldCount = 0;
    let methodCount = 0;
    if (ctx !== null) {
      const sym = buildSymbolDetail(ctx, neighborhoodEdges, subItems, fqdn);
      fieldCount = sym.fields.length;
      methodCount = sym.methods.length;
      detailsEl.symbol = sym;
    }
    focusCanvas.set_payload(buildFocusPayload(
      fqdn,
      ctx,
      neighborhoodEdges,
      neighborhoodSymbols,
      fieldCount,
      methodCount,
    ));
    focusCanvas.fit();
    if (ctx === null) {
      setStatus(`get_context failed for ${shortFqdn(fqdn)}`);
      return;
    }
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
    entry_point: s.entry_point ?? null,
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

/**
 * Overview navigation state. The default `workspace` mode paints
 * every project as a cluster (project_id = cluster_id). Clicking a
 * project cluster — or a folder/project in the Explorer — switches
 * the scope to `project` or `folder`; the Overview then paints the
 * modules inside that scope and their inter-module edges. Going back
 * is one click on the breadcrumb pill the OverviewElement renders.
 */
type OverviewScope =
  | { kind: 'workspace' }
  | { kind: 'project'; projectId: number; label: string }
  | { kind: 'folder'; projectId: number; label: string; relPath: string }
  | { kind: 'module'; prefix: string; label: string };

/**
 * What a cluster click should dispatch to. The Overview canvas only
 * knows opaque u32 ids; the shell owns the resolution table so each
 * scope can rewrite click semantics independently — workspace mode
 * drills into projects, scoped modes drill one FQDN segment deeper
 * via `drill-module`.
 */
type ClusterTarget =
  | { kind: 'drill-project'; projectId: number; label: string }
  | { kind: 'drill-folder'; projectId: number; label: string; relPath: string }
  | { kind: 'drill-module'; prefix: string; label: string }
  | { kind: 'focus-symbol'; fqdn: string };

interface FileEntry {
  readonly id: string;
  readonly path: string;
  readonly projectLabel: string;
  readonly symbols: ReadonlyArray<BrowseSymbol>;
}

interface FolderEntry {
  readonly projectId: number;
  readonly projectLabel: string;
  /**
   * Project-relative folder path (no leading separator). Empty string
   * means the project root itself (only used when the tree-builder
   * exposes it as a folder, normally the project node is preferred).
   */
  readonly relPath: string;
}

interface ProjectLike {
  readonly project_id: number;
  readonly label: string;
  readonly rel_path: string;
}

/**
 * Side-channel maps collected during the workspace tree walk. The
 * Explorer dispatches clicks via opaque node ids; these maps let the
 * host resolve a click back to the rich metadata (file entry,
 * folder coords inside a project, project) needed to drive the
 * Inspector and the Overview scope.
 */
interface TreeOut {
  readonly fileById: Map<string, FileEntry>;
  readonly folderById: Map<string, FolderEntry>;
  readonly projectByExplorerId: Map<string, ProjectLike>;
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
  out: TreeOut,
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
    children: trieToExplorerNodes(trie, 'ws', allSymbols, out),
  };
}

/**
 * Flat-projects view — workspace → projects → modules. Drops FS layout
 * but keeps project membership visible; each project expands to a flat
 * alphabetical list of its modules so the user can drill without
 * traversing folder hierarchy. Differs from `buildModulesTree` in that
 * modules are NOT nested by `::` segments — flat.
 */
function buildProjectsTreeFlat(
  workspaceLabel: string,
  projects: ReadonlyArray<ProjectLike>,
  allSymbols: ReadonlyArray<BrowseSymbol>,
  out: TreeOut,
): ExplorerTreeNode {
  const modulesByProject = collectModulesByProject(allSymbols);
  const children: ExplorerTreeNode[] = [];
  const sorted = [...projects].sort((a, b) => a.label.localeCompare(b.label));
  for (const p of sorted) {
    const id = `proj/${p.project_id}`;
    out.projectByExplorerId.set(id, p);
    const modules = [...(modulesByProject.get(p.project_id) ?? new Set<string>())]
      .sort((a, b) => a.localeCompare(b));
    const moduleNodes: ExplorerTreeNode[] = modules.map(mod => ({
      id: `${id}::${mod}`,
      label: mod,
      kind: 'module',
      children: undefined,
      fqdn: mod,
    }));
    children.push({
      id,
      label: p.label,
      kind: 'project',
      children: moduleNodes.length > 0 ? moduleNodes : undefined,
      fqdn: null,
      description: p.rel_path,
    });
  }
  return {
    id: 'workspace',
    label: workspaceLabel,
    kind: 'workspace',
    children,
  };
}

function collectModulesByProject(allSymbols: ReadonlyArray<BrowseSymbol>): Map<number, Set<string>> {
  const byProject = new Map<number, Set<string>>();
  for (const s of allSymbols) {
    const pid = s.project_id;
    if (typeof pid !== 'number') continue;
    const m = s.module;
    if (typeof m !== 'string' || m.length === 0) continue;
    let set = byProject.get(pid);
    if (set === undefined) {
      set = new Set<string>();
      byProject.set(pid, set);
    }
    set.add(m);
  }
  return byProject;
}

/**
 * IR-aligned view — projects → modules nested by `::` segments. Strips
 * incidental FS layout entirely; each project's module hierarchy is
 * reconstructed from the daemon's `module` strings via a segment trie.
 * Modules that exist only as ancestors get virtual nodes so leaves
 * still nest properly (e.g. `foo::bar::baz` produces foo → bar → baz
 * even if `foo::bar` itself never holds a symbol).
 */
function buildModulesTree(
  workspaceLabel: string,
  projects: ReadonlyArray<ProjectLike>,
  allSymbols: ReadonlyArray<BrowseSymbol>,
  out: TreeOut,
): ExplorerTreeNode {
  const byProject = collectModulesByProject(allSymbols);
  const sortedProjects = [...projects].sort((a, b) => a.label.localeCompare(b.label));
  const children: ExplorerTreeNode[] = [];
  for (const p of sortedProjects) {
    const projId = `proj/${p.project_id}`;
    out.projectByExplorerId.set(projId, p);
    const modules = [...(byProject.get(p.project_id) ?? new Set<string>())].sort((a, b) => a.localeCompare(b));
    const moduleNodes = modulesToTreeNodes(modules, projId);
    children.push({
      id: projId,
      label: p.label,
      kind: 'project',
      children: moduleNodes.length > 0 ? moduleNodes : undefined,
      fqdn: null,
      description: p.rel_path,
    });
  }
  return {
    id: 'workspace',
    label: workspaceLabel,
    kind: 'workspace',
    children,
  };
}

interface ModuleTrieNode {
  fullFqdn?: string;
  children: Map<string, ModuleTrieNode>;
}

function modulesToTreeNodes(modules: ReadonlyArray<string>, idPrefix: string): ExplorerTreeNode[] {
  const root: ModuleTrieNode = { children: new Map() };
  for (const m of modules) {
    const segs = m.split('::').filter(s => s.length > 0);
    if (segs.length === 0) continue;
    let cur = root;
    for (const seg of segs) {
      let next = cur.children.get(seg);
      if (next === undefined) {
        next = { children: new Map() };
        cur.children.set(seg, next);
      }
      cur = next;
    }
    cur.fullFqdn = m;
  }
  return moduleTrieToNodes(root, idPrefix, '');
}

function moduleTrieToNodes(node: ModuleTrieNode, idPrefix: string, fqdnPrefix: string): ExplorerTreeNode[] {
  const nodes: ExplorerTreeNode[] = [];
  for (const seg of [...node.children.keys()].sort((a, b) => a.localeCompare(b))) {
    const child = node.children.get(seg);
    if (child === undefined) continue;
    const fqdn = fqdnPrefix.length > 0 ? `${fqdnPrefix}::${seg}` : seg;
    const childId = `${idPrefix}::${seg}`;
    const grandChildren = moduleTrieToNodes(child, childId, fqdn);
    nodes.push({
      id: childId,
      label: seg,
      kind: 'module',
      children: grandChildren.length > 0 ? grandChildren : undefined,
      fqdn: child.fullFqdn ?? fqdn,
    });
  }
  return nodes;
}

function trieToExplorerNodes(
  trie: PathTrieNode,
  idPrefix: string,
  allSymbols: ReadonlyArray<BrowseSymbol>,
  out: TreeOut,
): ExplorerTreeNode[] {
  const nodes: ExplorerTreeNode[] = [];
  for (const name of [...trie.children.keys()].sort((a, b) => a.localeCompare(b))) {
    const child = trie.children.get(name);
    if (child === undefined) continue;
    const childId = `${idPrefix}/${name}`;
    const subProjectNodes = trieToExplorerNodes(child, childId, allSymbols, out);
    if (child.project !== undefined) {
      // This trie level is a real project. Render with project kind,
      // path-segment as the visible label, daemon label as tooltip-
      // shaped metadata. Merge sub-project entries with the project's
      // own file tree under one combined children array.
      const project = child.project;
      out.projectByExplorerId.set(childId, project);
      const projectNode = buildProjectNode(project, allSymbols, out);
      const merged: ExplorerTreeNode[] = [
        ...subProjectNodes,
        ...(projectNode.children ?? []),
      ];
      nodes.push({
        id: childId,
        label: name,
        kind: 'project',
        children: merged.length > 0 ? merged : undefined,
        fqdn: null,
        description: `${project.label} (${project.rel_path})`,
      });
    } else {
      // Pure folder — only purpose is to nest sub-projects.
      nodes.push({
        id: childId,
        label: name,
        kind: 'folder',
        children: subProjectNodes,
      });
    }
  }
  return nodes;
}

function buildProjectNode(
  project: { project_id: number; label: string; rel_path: string },
  allSymbols: ReadonlyArray<BrowseSymbol>,
  out: TreeOut,
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
  out.projectByExplorerId.set(id, project);
  const children = touchedFiles > 0
    ? dirToNodes(root, id, project, '', out)
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
  project: ProjectLike,
  currentPath: string,
  out: TreeOut,
): ExplorerTreeNode[] {
  const nodes: ExplorerTreeNode[] = [];
  for (const name of [...dir.children.keys()].sort()) {
    const child = dir.children.get(name);
    if (child === undefined) continue;
    const id = `${idPrefix}/${name}`;
    const subPath = currentPath.length > 0 ? `${currentPath}/${name}` : name;
    out.folderById.set(id, {
      projectId: project.project_id,
      projectLabel: project.label,
      relPath: subPath,
    });
    nodes.push({
      id,
      label: name,
      kind: 'folder',
      children: dirToNodes(child, id, project, subPath, out),
    });
  }
  for (const name of [...dir.files.keys()].sort()) {
    const symbols = (dir.files.get(name) ?? []).slice().sort((a, b) => a.start_line - b.start_line);
    const id = `${idPrefix}/${name}`;
    const filePath = currentPath.length > 0 ? `${currentPath}/${name}` : name;
    out.fileById.set(id, { id, path: filePath, projectLabel: project.label, symbols });
    nodes.push({
      id,
      label: name,
      kind: 'file',
      children: symbols.map(s => ({
        id: `sym:${s.fqdn}`,
        label: s.name,
        kind: mapBrowseSymbolKind(s),
        fqdn: s.fqdn,
        visibility: s.visibility,
        entryPointKind: (s.entry_point ?? null) as EntryPointKind | null,
      })),
    });
  }
  return nodes;
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
    fields: [],
    methods: [],
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

interface BuiltOverviewPayload {
  readonly json: string;
  readonly targets: Map<number, ClusterTarget>;
  /** Distinct IR edge kinds present in the cross-edge set; consumed
   *  by the OverviewElement legend so chips reflect what's actually
   *  in the current scope (no dead chips for kinds with 0 edges). */
  readonly crossKinds: ReadonlyArray<string>;
}

type GraphEdge = { from: string; to: string; kind: string; outbound: boolean };

interface OverviewNodeBuilder {
  fqdn: string;
  label: string;
  depth: number;
  parent_fqdn: string | null;
  node_kind: 'module' | 'public_symbol';
  symbol_count: number;
  project_kind?: string;
}

/**
 * Workspace Overview = flat root-package view. One node per project,
 * placed at depth 0 in the depth-stacked layout. No sub-modules, no
 * public symbols — the focus-graph + drill-down are the right tools
 * for that. Keeps the workspace canvas to ~12 spheres on standardoc
 * scale, which is the only count that stays readable + 60fps on a
 * dense workspace.
 */
function buildOverviewPayloadForScope(
  scope: OverviewScope,
  projects: ReadonlyArray<{ project_id: number; label: string; rel_path: string; kind: { kind: string } }>,
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
  projects: ReadonlyArray<{ project_id: number; label: string; rel_path: string; kind: { kind: string } }>,
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
function scopeBreadcrumbLabel(scope: OverviewScope): string | null {
  switch (scope.kind) {
    case 'workspace': return null;
    case 'project': return scope.label;
    case 'folder': return `${scope.label}/${scope.relPath}`;
    case 'module': return scope.prefix;
  }
}

/**
 * Workspace flat root view — one sphere per project at depth 0. Edges
 * aggregated by `(from_project, to_project, kind)` so each IR kind
 * keeps its swatch in the legend. Click target = drill-project so
 * the user lands on the scoped hierarchical view of that project.
 */
function buildWorkspacePackagesPayload(
  projects: ReadonlyArray<{ project_id: number; label: string; rel_path: string; kind: { kind: string } }>,
  symbols: ReadonlyArray<BrowseSymbol>,
  edges: ReadonlyArray<GraphEdge>,
  symbolByFqdn: Map<string, BrowseSymbol>,
): BuiltOverviewPayload {
  const counts = new Map<number, number>();
  for (const s of symbols) {
    if (s.project_id === undefined || s.project_id === null) continue;
    counts.set(s.project_id, (counts.get(s.project_id) ?? 0) + 1);
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
  for (const p of projects) {
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
    const fromProj = symbolByFqdn.get(e.from)?.project_id;
    const toProj = symbolByFqdn.get(e.to)?.project_id;
    if (fromProj === undefined || fromProj === null) continue;
    if (toProj === undefined || toProj === null) continue;
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

function buildFocusPayload(
  fqdn: string,
  ctx: GetContextResponse | null,
  neighborhoodEdges: ReadonlyArray<{ from: string; to: string; kind: string; outbound: boolean }>,
  neighborhoodSymbols: ReadonlyArray<BrowseSymbol>,
  centerFieldCount: number,
  centerMethodCount: number,
): string {
  const centerSym = ctx?.context.symbol;
  const center = centerSym !== undefined ? {
    fqdn: centerSym.fqdn,
    name: centerSym.name,
    kind: centerSym.decl_kind ?? centerSym.language_kind ?? centerSym.kind,
    depth: 0,
    field_count: centerFieldCount,
    method_count: centerMethodCount,
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
      file: s.file && s.file.length > 0 ? s.file : null,
      start_line: typeof s.start_line === 'number' ? s.start_line : null,
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


/**
 * Coarse test-symbol heuristic, mirror of the Rust
 * `query::symbol_looks_like_test` + the Symbol Details panel's
 * local `looksLikeTest`. Used by the shell to filter focus / overview
 * payloads when the global `viewPrefsStore.excludeTests` is on so
 * the wasm canvases match the panel's "hide tests" state without
 * a round-trip.
 */
function looksLikeTest(fqdn: string, file?: string | null): boolean {
  if (fqdn.includes('::tests::') || fqdn.includes('::test::')) return true;
  if (fqdn.endsWith('::tests') || fqdn.endsWith('::test')) return true;
  if (fqdn.endsWith('_test') || fqdn.endsWith('_tests')) return true;
  if (fqdn.endsWith('.test') || fqdn.endsWith('.spec')) return true;
  if (!file) return false;
  const norm = file.replace(/\\/g, '/');
  if (norm.includes('/tests/') || norm.includes('/test/') || norm.includes('/__tests__/')) return true;
  if (/(?:_tests?\.rs|\.(?:test|spec)\.(?:tsx?|jsx?))$/.test(norm)) return true;
  return false;
}

function formatSignature(sig: RawSymbol['signature']): string | null {
  if (!sig) return null;
  const params = sig.params ?? [];
  const ret = sig.returns?.display ?? null;
  if (params.length === 0) {
    return ret ? `: ${ret}` : null;
  }
  const paramStr = params
    .map((p) => (p.ty?.display ? `${p.name}: ${p.ty.display}` : p.name))
    .join(', ');
  return ret ? `(${paramStr}) → ${ret}` : `(${paramStr})`;
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
  subItems: ReadonlyArray<RawSymbol>,
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
  // `ctx.imports` = OUTBOUND imports from this symbol (what it pulls in).
  // `ctx.imported_by` = INBOUND imports (who imports this symbol).
  // Used to be collapsed into the same `importedBy` bucket which mis-
  // labelled outbound imports as "Imported by" in the panel.
  for (const e of ctx.imports) {
    if (e.target.fqdn) pushBucket('imports', e.target.fqdn, 'mod');
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
    'usedBy', 'usesTypes', 'calls', 'imports', 'importedBy', 'testedBy', 'implements', 'extends',
  ];
  const relations: SymbolRelationBucket[] = [];
  for (const k of orderedKinds) {
    const m = buckets.get(k);
    if (m === undefined || m.size === 0) continue;
    const items = [...m.values()];
    relations.push({ kind: k, items, total: items.length });
  }

  const fields: SymbolSubItem[] = [];
  const methods: SymbolSubItem[] = [];
  for (const s of subItems) {
    const cls = classifySubItem(s);
    if (cls === null) continue;
    // Field-shaped items (no params, return = the field's type)
    // get a standalone `type` so the Fields tab can render the
    // type as its own chip. Methods leave it null — their return
    // type is already inside `signature`.
    const sig = s.signature;
    const fieldType = sig && (!sig.params || sig.params.length === 0)
      ? (sig.returns?.display ?? null)
      : null;
    const item: SymbolSubItem = {
      fqdn: s.fqdn,
      name: s.name,
      kindLabel: s.decl_kind ?? s.language_kind ?? s.kind,
      file: s.location.file,
      startLine: s.location.start_line,
      signature: formatSignature(s.signature),
      visibility: s.visibility ?? null,
      isAsync: Array.isArray(s.flags) && s.flags.includes('async'),
      type: fieldType,
    };
    if (cls === 'field') fields.push(item);
    else methods.push(item);
  }
  fields.sort((a, b) => (a.startLine ?? 0) - (b.startLine ?? 0));
  methods.sort((a, b) => (a.startLine ?? 0) - (b.startLine ?? 0));

  return {
    fqdn: sym.fqdn,
    name: sym.name,
    kindLabel: sym.decl_kind ?? sym.language_kind ?? sym.kind,
    visibility: sym.visibility,
    file: sym.location.file,
    startLine: sym.location.start_line,
    documentation: doc,
    entryPointKind: epKind,
    fields,
    methods,
    relations,
  };
}

/**
 * Best-effort sub-symbols fetch for a parent FQDN. `list_symbols`
 * scoped by `module = parentFqdn` returns the direct children that the
 * extractors registered as nested symbols (Rust struct fields / enum
 * variants / impl methods, TS interface properties / class methods).
 * Bounded by SUB_PAGE_SIZE — structs with > 200 members are vanishingly
 * rare and we don't paginate here; the daemon caps `limit` at u8 (255)
 * so SUB_PAGE_SIZE stays safely below.
 */
const SUB_PAGE_SIZE = 200;
async function fetchSubItems(mcp: McpBrowse, fqdn: string): Promise<ReadonlyArray<RawSymbol>> {
  try {
    const res = await mcp.listSymbols({ module: fqdn, limit: SUB_PAGE_SIZE });
    return res.items;
  } catch {
    return [];
  }
}

/**
 * Classify a sub-symbol returned by `list_symbols({ module: parentFqdn })`
 * into the Fields or Methods tab bucket. Falls back through decl_kind →
 * language_kind so the heuristic catches both Rust (`field` / `method`)
 * and TS (`interface_property` / `class_method`) shapes. Unrecognised
 * sub-symbols (associated consts, nested types, etc.) are dropped from
 * V0 — they need their own tab to render cleanly.
 */
function classifySubItem(s: RawSymbol): 'field' | 'method' | null {
  const dk = s.decl_kind;
  const lk = s.language_kind;
  if (dk === 'field' || dk === 'variant') return 'field';
  if (lk === 'field' || lk === 'interface_property' || lk === 'enum_variant' || lk === 'class_property' || lk === 'struct_field') return 'field';
  if (dk === 'method' || dk === 'function') return 'method';
  if (lk === 'method' || lk === 'class_method' || lk === 'function') return 'method';
  return null;
}

boot().catch((e: unknown) => {
  const msg = e instanceof Error ? e.message : String(e);
  setStatus(`fatal: ${msg}`);
  console.error('[shell] boot failed', e);
});
