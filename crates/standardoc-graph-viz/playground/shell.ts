/**
 * Shell bootstrap — wires the multi-panel layout against the standardoc
 * daemon. Co-exists with the legacy `main.ts` entry; both routes target
 * the same daemon and use the same components. Once the shell is the
 * accepted production target the legacy entry can be retired.
 *
 *   /        → legacy single-canvas playground (index.html + main.ts)
 *   /shell.html → multi-panel shell (shell.html + this file)
 */

import init, { GraphEngine } from '../pkg/standardoc_graph_viz.js';

import '@standarx/standardoc-viz/components/panel-layout';
import '@standarx/standardoc-viz/components/explorer';
import '@standarx/standardoc-viz/components/symbol-details';
import '@standarx/standardoc-viz/components/search';
import '@standarx/standardoc-viz/components/graph';

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
	ExplorerTreeNode,
	EntryPointKind,
	GraphClickDetail,
	GraphElement,
	GraphErrorDetail,
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
const overviewEl = document.getElementById('overview') as GraphElement;
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

	// Engine factory + ready handshake. The component owns canvases +
	// pointer events; it calls back into us to instantiate the engine
	// once they're in the DOM.
	const ready = new Promise<void>((resolve, reject) => {
		overviewEl.addEventListener('sd-graph-ready', () => resolve(), { once: true });
		overviewEl.addEventListener('sd-graph-error', e => {
			const { source, message } = (e as CustomEvent<GraphErrorDetail>).detail;
			if (source === 'engine-init') reject(new Error(message));
		}, { once: true });
	});
	overviewEl.engineFactory = (canvas, w, h, dpr) => new GraphEngine(canvas, w, h, dpr);
	await ready;
	const engine = overviewEl.engine!;

	// Click on a graph node → shift global focus.
	overviewEl.addEventListener('sd-graph-click', e => {
		const { fqdn } = (e as CustomEvent<GraphClickDetail>).detail;
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
	if (graph !== null) {
		engine.load_graph(JSON.stringify({
			symbols: graph.symbols,
			projects: graph.projects ?? [],
			edges: graph.edges,
		}));
		engine.fit();
	}

	// IDE-style file tree per project, built synchronously from the
	// already-fetched symbol set (no extra round-trip). Each project
	// expands into its folder/file hierarchy; each file expands into the
	// symbols defined inside it (sorted by start_line for source order).
	setStatus('build tree…');
	const tree: ExplorerTreeNode[] = projects.map(p =>
		buildProjectNode(p, graph?.symbols ?? []),
	);
	explorerEl.tree = tree;

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

function buildProjectNode(
	project: { project_id: number; label: string; rel_path: string },
	allSymbols: ReadonlyArray<BrowseSymbol>,
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
	const children = touchedFiles > 0 ? dirToNodes(root, id) : undefined;
	return {
		id,
		label: project.label,
		kind: 'project',
		children,
	};
}

function dirToNodes(dir: DirNode, idPrefix: string): ExplorerTreeNode[] {
	const out: ExplorerTreeNode[] = [];
	for (const name of [...dir.children.keys()].sort()) {
		const child = dir.children.get(name);
		if (child === undefined) continue;
		const id = `${idPrefix}/${name}`;
		out.push({
			id,
			label: name,
			kind: 'folder',
			children: dirToNodes(child, id),
		});
	}
	for (const name of [...dir.files.keys()].sort()) {
		const symbols = (dir.files.get(name) ?? []).slice().sort((a, b) => a.start_line - b.start_line);
		const id = `${idPrefix}/${name}`;
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
