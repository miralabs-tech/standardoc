/**
 * `mountShell` — the multi-panel shell as a reusable lib entry. Builds
 * the toolbar + panel-layout + four panels + panel-host into a host
 * container, registers every component, and runs the full wiring
 * (Overview / Explorer / Focus / Symbol Details / Search / Compare,
 * the hide-tests toggle, focus history) against an injected MCP
 * transport and WASM canvas factory.
 *
 * The host owns two things the lib can't assume:
 *   - the MCP `Transport` (playground → StreamableHTTP to its dev-server
 *     proxy; VSCode webview → postMessage relay through the ext host);
 *   - the WASM module (init fn + the two canvas ctors) and the URL its
 *     `.wasm` binary is fetched from.
 *
 * Everything else — DOM, component registration, payload building,
 * event wiring — lives here so the playground and the webview render
 * a byte-identical shell from a single source.
 */

import type { Transport } from '@modelcontextprotocol/sdk/shared/transport.js';

import './shell.scss';

import '../components/panel-layout';
import '../components/explorer';
import '../components/symbol-details';
import '../components/search';
import '../components/overview';
import '../components/focus-graph';
import '../components/compare-panel';
import '../components/panel-host';
import '../components/legend';

import { focusStore } from '../focus-store';
import { panelManager } from '../panel-manager';
import { viewPrefsStore } from '../view-prefs-store';
import { McpBrowse } from '../mcp-client';
import type {
  BrowseSymbol,
  McpClientInfo,
  RawSymbol,
} from '../mcp-client';
import type {
  ComparePanelElement,
  CompareRefreshRequestDetail,
  ExplorerElement,
  ExplorerSelectDetail,
  ExplorerTreeNode,
  ExplorerTreeView,
  ExplorerViewChangeDetail,
  FocusGraphCanvasFacade,
  FocusGraphElement,
  FocusGraphErrorDetail,
  FocusGraphHopChangeDetail,
  FocusGraphNodeClickDetail,
  OverviewCanvasFacade,
  OverviewClusterClickDetail,
  OverviewElement,
  OverviewErrorDetail,
  PanelHostElement,
  PanelLayoutElement,
  PanelSlot,
  SearchElement,
  SymbolDetailsActionDetail,
  SymbolDetailsElement,
  SymbolDetailsTabChangeDetail,
  SymbolSearchResult,
} from '../index';

import type { ClusterTarget, OverviewScope, TreeOut } from './types';
import {
  collectWorkspaceSymbols,
  looksLikeTest,
  rawToBrowseSymbol,
  shortFqdn,
  toSymbolSearchResult,
} from './symbols';
import {
  buildModulesTree,
  buildProjectsTreeFlat,
  buildWorkspaceTree,
} from './explorer-tree';
import {
  buildOverviewPayloadForScope,
  scopeBreadcrumbLabel,
} from './overview-payload';
import { buildEmptyFocusPayload, buildFocusPayload } from './focus-payload';
import {
  buildFileSyntheticDetail,
  buildSymbolDetail,
  fetchSubItems,
} from './symbol-detail';

/**
 * The slice of the wasm-bindgen module the shell drives. The host
 * imports the generated bindings and passes them in; the lib stays
 * decoupled from a specific `pkg/` build. The canvas ctors must
 * produce objects satisfying the component facade interfaces.
 */
export interface ShellWasm {
  init: (options: { module_or_path: string | URL }) => Promise<unknown>;
  OverviewCanvas: new (
    canvas: HTMLCanvasElement,
    width: number,
    height: number,
    dpr: number,
  ) => OverviewCanvasFacade;
  FocusGraphCanvas: new (
    canvas: HTMLCanvasElement,
    width: number,
    height: number,
    dpr: number,
  ) => FocusGraphCanvasFacade;
}

export interface MountShellOptions {
  /** MCP transport — host-built (HTTP for the playground, postMessage for the webview). */
  readonly transport: Transport;
  /** wasm-bindgen module: init fn + the two canvas ctors. */
  readonly wasm: ShellWasm;
  /** URL the `.wasm` binary is fetched from (asWebviewUri in the webview, `/pkg/...` in the playground). */
  readonly wasmUrl: string | URL;
  /** Optional MCP client identity reported in the connect handshake. */
  readonly clientInfo?: McpClientInfo;
}

const SHELL_MARKUP = `
<standardoc-panel-layout data-shell-root>
  <div data-slot="toolbar" class="sd-shell-toolbar">
    <span class="sd-shell-toolbar__brand">standardoc</span>
    <standardoc-search data-shell-search global-shortcut placeholder="Search symbols, files, types…"></standardoc-search>
    <span class="sd-shell-toolbar__spacer"></span>
    <div class="sd-shell-toggles" role="toolbar" aria-label="Panel visibility">
      <button type="button" class="sd-shell-toggle" data-toggle-panel="explorer" aria-pressed="true" title="Toggle Explorer">Explorer</button>
      <button type="button" class="sd-shell-toggle" data-toggle-panel="overview" aria-pressed="true" title="Toggle Overview">Overview</button>
      <button type="button" class="sd-shell-toggle" data-toggle-panel="focus" aria-pressed="true" title="Toggle Focus">Focus</button>
      <button type="button" class="sd-shell-toggle" data-toggle-panel="details" aria-pressed="true" title="Toggle Details">Details</button>
    </div>
    <button type="button" data-shell-hide-tests class="sd-shell-toggle" aria-pressed="false" title="Hide test-shaped symbols across all panels">hide tests</button>
    <span data-shell-status class="sd-shell-status">booting…</span>
  </div>
  <standardoc-explorer data-shell-explorer data-slot="explorer"></standardoc-explorer>
  <standardoc-overview data-shell-overview data-slot="overview"></standardoc-overview>
  <standardoc-focus-graph data-shell-focus data-slot="focus"></standardoc-focus-graph>
  <standardoc-symbol-details data-shell-details data-slot="details"></standardoc-symbol-details>
</standardoc-panel-layout>
<standardoc-panel-host data-shell-panels></standardoc-panel-host>
`;

/**
 * Build and wire the shell into `container`. Resolves once the initial
 * workspace data has been fetched and rendered (parity with the
 * previous `boot()` — the returned promise rejects on a fatal init
 * error so hosts can surface it).
 */
export async function mountShell(
  container: HTMLElement,
  opts: MountShellOptions,
): Promise<void> {
  // The panel-layout `:host` is `height: 100%`, so it needs a parent
  // with a definite height. The host hands us an arbitrary container
  // (e.g. the playground's `#app`, the webview's `#app`) that has no
  // intrinsic height — without this the grid collapses to 0, the canvas
  // panels init at ~0×0 and the Overview camera flies off / Focus
  // vanishes. Own the container: fill it (its parent — `<body>` in both
  // hosts — is sized) and lay the panel-layout + drawer out as a column.
  container.style.width = '100%';
  container.style.height = '100%';
  container.style.display = 'flex';
  container.style.flexDirection = 'column';
  container.style.minHeight = '0';

  container.innerHTML = SHELL_MARKUP;

  const q = <T extends Element>(sel: string): T => {
    const el = container.querySelector<T>(sel);
    if (el === null) throw new Error(`mountShell: missing element ${sel}`);
    return el;
  };

  const explorerEl = q<ExplorerElement>('[data-shell-explorer]');
  const detailsEl = q<SymbolDetailsElement>('[data-shell-details]');
  const searchEl = q<SearchElement>('[data-shell-search]');
  const overviewEl = q<OverviewElement>('[data-shell-overview]');
  const focusEl = q<FocusGraphElement>('[data-shell-focus]');
  const panelsEl = q<PanelHostElement>('[data-shell-panels]');
  const statusEl = q<HTMLSpanElement>('[data-shell-status]');
  const shellEl = q<PanelLayoutElement>('[data-shell-root]');

  // Wire toolbar panel toggle buttons → `panelLayout.togglePanel(slot)`.
  // The element handles the actual show/hide + persistence; the host
  // only needs to keep the button `aria-pressed` state in sync with the
  // real state (covers both user-driven toggles and persisted state
  // restored from a previous session).
  const toggleBtns = container.querySelectorAll<HTMLButtonElement>('button[data-toggle-panel]');
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
  const hideTestsBtn = container.querySelector<HTMLButtonElement>('[data-shell-hide-tests]');
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

  const setStatus = (text: string): void => {
    statusEl.textContent = text;
  };

  setStatus('init wasm…');
  await opts.wasm.init({ module_or_path: opts.wasmUrl });

  setStatus('connect MCP…');
  const mcp = await McpBrowse.connect(
    opts.transport,
    opts.clientInfo ?? { name: 'standardoc-graph-viz-shell', version: '0.0.1' },
  );

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
  overviewEl.canvasFactory = (canvas, w, h, dpr) => new opts.wasm.OverviewCanvas(canvas, w, h, dpr);
  focusEl.canvasFactory = (canvas, w, h, dpr) => new opts.wasm.FocusGraphCanvas(canvas, w, h, dpr);
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
        // Workspace → project: scope the Overview to the project's
        // modules — the same `project` scope the Explorer project click
        // dispatches. NOT a `module` scope keyed on the label: the
        // daemon project label ("Standardoc", "Lurlang", …) is not an
        // FQDN root, so a label-prefix module scope matches nothing and
        // the canvas comes back empty.
        applyOverviewScope({ kind: 'project', projectId: target.projectId, label: target.label });
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
    kind: s.kind,
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
      kind: sym?.kind,
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
    if (sym === undefined) return;
    // `sym.module` can be null OR undefined (BrowseSymbol fields
    // come from the daemon which omits falsy strings); use a typeof
    // guard to cover both safely. Same bug bit the Overview payload
    // builder a few commits back.
    if (typeof sym.module !== 'string' || sym.module.length === 0) return;
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
    const query = detail.query.trim();
    if (query.length === 0) {
      searchEl.results = [];
      searchEl.suggestions = [];
      return;
    }
    searchEl.loading = true;
    try {
      const [fts, pattern] = await Promise.all([
        mcp.findSymbol(query, 20).catch(() => ({ results: [] as ReadonlyArray<RawSymbol>, suggestions: [] })),
        mcp.findSymbolsByPattern(`*${query}*`, 20).catch(() => [] as ReadonlyArray<RawSymbol>),
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
        kind: s.kind,
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
        // The lib shell has no editor host to hand the symbol off to —
        // surface the intent so the user sees the click did register.
        // A host that CAN reveal (the VSCode webview) overrides this by
        // listening for `sd-symbol-action` on its own container before
        // mountShell runs, or via a future host hook.
        setStatus(`open in editor: ${shortFqdn(detail.fqdn)} — not available here`);
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
