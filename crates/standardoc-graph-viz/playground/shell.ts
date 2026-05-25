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

	// Click on an overview cluster → drill into focus on a representative
	// symbol (Phase 3c lands the cluster→symbol lookup; for now we don't
	// drill until that resolution is wired).
	overviewEl.addEventListener('sd-overview-cluster-click', e => {
		const _detail = (e as CustomEvent<OverviewClusterClickDetail>).detail;
		// Phase 3c: focusStore.setFocus(representativeFqdnFor(_detail.clusterId));
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

	// Entry points — list_symbols doesn't expose an entry_point filter
	// yet, so we walk every page via the cursor and filter client-side.
	// Bounded above by PAGE_SIZE * MAX_PAGES so a runaway daemon can't
	// hang the boot path; the limit is generous enough for realistic
	// workspaces.
	setStatus('entry points…');
	const entryPoints = await collectEntryPoints(mcp, status => setStatus(status));
	explorerEl.entryPoints = entryPoints;

	// Load the full workspace graph into the overview canvas. We reuse
	// the same symbol set to build the Explorer file tree below — one
	// fetch feeds both the canvas and the navigation panel.
	setStatus('fetch graph…');
	const graph = await mcp.fetchGraph(false).catch(() => null);
	const symbolByFqdn = new Map<string, BrowseSymbol>();
	for (const s of graph?.symbols ?? []) symbolByFqdn.set(s.fqdn, s);
	if (graph !== null) {
		overview.set_payload(buildOverviewPayload(projects, graph.symbols, graph.edges, symbolByFqdn));
		overview.fit();
	}

	// IDE-style workspace tree built synchronously from the already-
	// fetched symbol set (no extra round-trip). Top level is the
	// workspace root; below it, projects are grouped under their first
	// path segment (crates/, ext/, …) so the structure mirrors a real
	// file explorer; each project then expands into its own src/tests/
	// folder hierarchy + per-file symbol list. The fileById index lets
	// the shell react to file clicks by spawning a synthetic SymbolDetail
	// profile listing the symbols defined in that file.
	setStatus('build tree…');
	const fileById = new Map<string, FileEntry>();
	const workspaceRoot = buildWorkspaceTree('Workspace', projects, graph?.symbols ?? [], fileById);
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

	// Focus → Symbol Details. Concurrent fetch token guards against a
	// stale response landing after the user has moved on to another FQDN.
	let focusToken = 0;
	focusStore.subscribe(async state => {
		const fqdn = state.current;
		if (fqdn === null) {
			detailsEl.symbol = null;
			return;
		}
		const myToken = ++focusToken;
		detailsEl.symbol = null;
		setStatus(`fetch ${shortFqdn(fqdn)}…`);
		const [ctx, neighborhood] = await Promise.all([
			mcp.getContext(fqdn).catch(() => null),
			mcp.fetchNeighborhood(fqdn, false).catch(() => null),
		]);
		if (myToken !== focusToken) return; // a newer focus arrived
		// Push the focal payload into the FocusGraph canvas alongside the
		// SymbolDetails update — both panels react to the same focus shift
		// off the same fetched neighborhood snapshot.
		focusCanvas.set_payload(buildFocusPayload(fqdn, ctx, neighborhood?.edges ?? [], neighborhood?.symbols ?? []));
		focusCanvas.fit();
		if (ctx === null) {
			setStatus(`get_context failed for ${shortFqdn(fqdn)}`);
			return;
		}
		const sym = buildSymbolDetail(ctx, neighborhood?.edges ?? [], fqdn);
		detailsEl.symbol = sym;
		setStatus(`ready (${entryPoints.length} entry points)`);
	});

	setStatus(`ready (${entryPoints.length} entry points)`);
}

const EP_PAGE_SIZE = 500;
const EP_MAX_PAGES = 50; // 25k symbols ceiling, generous for any realistic workspace

async function collectEntryPoints(
	mcp: McpBrowse,
	report: (status: string) => void,
): Promise<ExplorerEntryPoint[]> {
	const found: ExplorerEntryPoint[] = [];
	let cursor: string | undefined;
	let page = 0;
	while (page < EP_MAX_PAGES) {
		page++;
		const res = await mcp.listSymbols({ limit: EP_PAGE_SIZE, cursor }).catch(() => null);
		if (res === null) break;
		for (const s of res.items) {
			if (typeof s.entry_point === 'string' && s.entry_point.length > 0) {
				found.push({
					fqdn: s.fqdn,
					label: shortFqdn(s.fqdn),
					kind: s.entry_point as EntryPointKind,
				});
			}
		}
		report(`entry points… (page ${page}, ${found.length} found)`);
		if (res.next_cursor === undefined || res.next_cursor === null || res.next_cursor.length === 0) break;
		cursor = res.next_cursor;
	}
	return found;
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
	const neighbors = neighborhoodSymbols
		.filter(s => s.fqdn !== fqdn)
		.map(s => ({
			fqdn: s.fqdn,
			name: s.name,
			kind: s.language_kind ?? s.kind,
			depth: 1,
		}));
	const focalEdges = neighborhoodEdges.map(e => ({
		from: e.from,
		to: e.to,
		kind: e.kind,
		depth: 1,
	}));
	return JSON.stringify({ center, neighbors, edges: focalEdges });
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
