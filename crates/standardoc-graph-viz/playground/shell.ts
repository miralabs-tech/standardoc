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
	GetContextResponse,
	RawSymbol,
} from '@standarx/standardoc-viz/mcp-client';
import type {
	ExplorerElement,
	ExplorerEntryPoint,
	ExplorerExpandDetail,
	ExplorerNodeKind,
	ExplorerTreeNode,
	EntryPointKind,
	GraphClickDetail,
	GraphElement,
	GraphErrorDetail,
	SearchElement,
	SymbolDetail,
	SymbolDetailsElement,
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

	// Mount projects into the Explorer tree. Projects are marked
	// `expandable` so the first click triggers a lazy fetch of their
	// root-level symbols via `list_symbols(module=project.label)`.
	setStatus('list projects…');
	const projectsRes = await mcp.listProjects().catch(() => null);
	const projectByNodeId = new Map<string, { label: string }>();
	let tree: ExplorerTreeNode[] = projectsRes
		? projectsRes.projects.map(p => {
			const id = `project:${p.project_id}`;
			projectByNodeId.set(id, { label: p.label });
			return {
				id,
				label: p.label,
				kind: 'project' as ExplorerNodeKind,
				expandable: true,
			};
		})
		: [];
	explorerEl.tree = tree;

	// Lazy tree expansion. When the user expands a project node we
	// fetch list_symbols(module=label) — exact-match returns root-level
	// items of the project (re-exports, top-level fns, root structs).
	// Deeper module navigation needs a different IR query and is left
	// for Phase 3 alongside the Overview canvas.
	explorerEl.addEventListener('sd-explorer-expand', async ev => {
		const detail = (ev as CustomEvent<ExplorerExpandDetail>).detail;
		const project = projectByNodeId.get(detail.id);
		if (project === undefined) return;
		// Optimistically render a loading placeholder.
		tree = mutateNode(tree, detail.id, n => ({ ...n, loading: true }));
		explorerEl.tree = tree;
		try {
			const res = await mcp.listSymbols({ module: project.label, limit: 200 });
			const children: ExplorerTreeNode[] = res.items.map(s => ({
				id: `sym:${s.fqdn}`,
				label: s.name,
				kind: mapRawKind(s),
				fqdn: s.fqdn,
			}));
			tree = mutateNode(tree, detail.id, n => ({ ...n, children, loading: false }));
			explorerEl.tree = tree;
		} catch {
			tree = mutateNode(tree, detail.id, n => ({ ...n, loading: false }));
			explorerEl.tree = tree;
		}
	});

	// Entry points — list_symbols doesn't expose an entry_point filter
	// yet, so we walk every page via the cursor and filter client-side.
	// Bounded above by PAGE_SIZE * MAX_PAGES so a runaway daemon can't
	// hang the boot path; the limit is generous enough for realistic
	// workspaces.
	setStatus('entry points…');
	const entryPoints = await collectEntryPoints(mcp, status => setStatus(status));
	explorerEl.entryPoints = entryPoints;

	// Load the full workspace graph into the overview canvas.
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

function mapRawKind(s: RawSymbol): ExplorerNodeKind {
	const decl = s.decl_kind ?? '';
	if (decl === 'struct') return 'struct';
	if (decl === 'enum') return 'enum';
	if (decl === 'function' || decl === 'method') return 'function';
	if (decl === 'trait' || decl === 'interface') return 'trait';
	if (decl === 'const' || decl === 'static') return 'value';
	if (decl === 'macro') return 'macro';
	switch (s.kind) {
		case 'type': return 'struct';
		case 'callable': return 'function';
		case 'value': return 'value';
		case 'macro': return 'macro';
		default: return 'unknown';
	}
}

function mutateNode(
	tree: ReadonlyArray<ExplorerTreeNode>,
	id: string,
	patch: (n: ExplorerTreeNode) => ExplorerTreeNode,
): ExplorerTreeNode[] {
	return tree.map(n => {
		if (n.id === id) return patch(n);
		if (n.children !== undefined && n.children.length > 0) {
			return { ...n, children: mutateNode(n.children, id, patch) };
		}
		return n;
	});
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
