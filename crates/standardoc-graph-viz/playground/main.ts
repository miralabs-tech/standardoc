/**
 * Playground bootstrap — thin integrator.
 *
 *   1. Initialise the wasm module + hand a `GraphEngine` factory to
 *      `<standardoc-graph>`. The component owns canvases, pointer
 *      events, ResizeObserver, mode switching (with engineBusy gate),
 *      lazy WebGPU init, and localStorage mode persistence.
 *   2. Connect to the standardoc daemon over MCP via the dev server's
 *      `/mcp` proxy and orchestrate a workspace-wide browse.
 *   3. Wire the lib components (toolbar / hud / graph / profiler) to
 *      each other. The host doesn't touch the engine internals beyond
 *      what's needed to push scene data + read stats.
 */

import init, { GraphEngine } from '../pkg/standardoc_graph_viz.js';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';

import { matcher } from 'matchigo';

import '@standarx/standardoc-viz/components/toolbar';
import '@standarx/standardoc-viz/components/hud';
import '@standarx/standardoc-viz/components/graph';
import { Profiler } from '@standarx/standardoc-viz/profiler';
import type {
	GraphClickDetail,
	GraphElement,
	GraphErrorDetail,
	GraphHoverDetail,
	GraphModeChangeDetail,
	HudCopyDetail,
	HudElement,
	HudRecordStartDetail,
	RecordingResult,
	RenderMode,
	StatusKind,
	ToolbarFlagChangeDetail,
	ToolbarModeRequestDetail,
} from '@standarx/standardoc-viz';

interface RawSymbol {
	readonly fqdn: string;
	readonly name: string;
	readonly kind: string;
	readonly visibility: string;
	readonly module: string | null;
	readonly language_kind: string;
	readonly is_external?: boolean;
	readonly location: { file: string; start_line: number };
}

interface ListSymbolsPage {
	readonly items: ReadonlyArray<RawSymbol>;
	readonly next_cursor: string | null;
}

interface BrowseSymbol {
	readonly fqdn: string;
	readonly name: string;
	readonly kind: string;
	readonly visibility: string;
	readonly module: string | null;
	readonly language_kind: string;
	readonly is_external: boolean;
	readonly file: string;
	readonly start_line: number;
}

interface BrowseEdge {
	readonly from: string;
	readonly to: string;
	readonly kind: string;
	readonly outbound: boolean;
}

interface ResolvedTarget {
	readonly kind: 'resolved';
	readonly fqdn: string;
}

interface UnresolvedTarget {
	readonly kind: 'unresolved';
	readonly name: string;
}

interface UnresolvedBridgeTarget {
	readonly kind: 'unresolved_bridge';
	readonly bridge: string;
	readonly name: string;
}

type NeighborTarget = ResolvedTarget | UnresolvedTarget | UnresolvedBridgeTarget;

interface NeighborSymbol {
	readonly edge_kind: string;
	readonly target: NeighborTarget;
}

// Hoisted module-scope matcher per the matchigo AI usage contract:
// lazy-compiled once, then O(1) literal dispatch on `kind`.
// Compile-time exhaustive — if the Rust-side `NeighborTarget` union
// grows a new variant, this fails the typecheck instead of silently
// dropping the edge.
const resolveNeighborFqdn = matcher<NeighborTarget, string | null>()
	.with({ kind: 'resolved' }, t => t.fqdn)
	.with({ kind: 'unresolved' }, () => null)
	.with({ kind: 'unresolved_bridge' }, () => null)
	.exhaustive();

interface SymbolContextWithNeighbors {
	readonly callers: ReadonlyArray<NeighborSymbol>;
	readonly callees: ReadonlyArray<NeighborSymbol>;
	readonly imports: ReadonlyArray<NeighborSymbol>;
	readonly imported_by: ReadonlyArray<NeighborSymbol>;
}

const toolbarEl = document.getElementById('toolbar') as HTMLElement;
const hudEl = document.getElementById('hud') as HudElement;
const graphEl = document.getElementById('graph') as GraphElement;
const detailEl = document.getElementById('detail') as HTMLElement;

function setStatus(text: string, kind: StatusKind): void {
	toolbarEl.setAttribute('status-text', text);
	toolbarEl.setAttribute('status-kind', kind);
}

function paletteFromCss(): string {
	const styles = getComputedStyle(document.documentElement);
	const v = (name: string, fallback: string) => styles.getPropertyValue(name).trim() || fallback;
	return JSON.stringify({
		background: v('--bg', '#1e1e1e'),
		foreground: v('--fg', '#cccccc'),
		description: v('--fg-muted', '#9d9d9d'),
		panel_border: v('--border', '#454545'),
		focus_border: v('--accent', '#3794ff'),
		widget_background: v('--bg-elevated', '#252526'),
		list_hover: v('--hover', '#2a2d2e'),
		text_link: v('--accent', '#3794ff'),
	});
}

class McpBrowse {
	private constructor(private readonly client: Client) {}

	static async connect(): Promise<McpBrowse> {
		const transport = new StreamableHTTPClientTransport(new URL('/mcp', window.location.origin));
		const client = new Client({ name: 'standardoc-graph-viz-playground', version: '0.0.1' }, { capabilities: {} });
		await client.connect(transport);
		return new McpBrowse(client);
	}

	async listModules(includeExternal: boolean): Promise<ReadonlyArray<RawSymbol>> {
		return this.listAllSymbols({ kind: 'module', include_external: includeExternal });
	}

	async listInModule(module: string, includeExternal: boolean): Promise<ReadonlyArray<RawSymbol>> {
		return this.listAllSymbols({ module, include_external: includeExternal });
	}

	async getContext(fqdn: string): Promise<SymbolContextWithNeighbors> {
		const raw = await this.callTool('get_context', { fqdn, depth: 1 });
		return JSON.parse(raw) as SymbolContextWithNeighbors;
	}

	/**
	 * Lightweight liveness probe used by the revision watcher
	 * (option (a) — client polling in lieu of SSE notifications).
	 * Returns the current daemon revision plus whether indexing has
	 * finished its cold-start sweep. Cheap on the server side: a
	 * single in-memory atomic read + a capability snapshot.
	 */
	async currentRevision(): Promise<{ revision: number; indexingReady: boolean }> {
		const raw = await this.callTool('current_revision', {});
		const parsed = JSON.parse(raw) as {
			revision: number;
			indexing?: { ready?: boolean };
		};
		return {
			revision: parsed.revision,
			indexingReady: parsed.indexing?.ready === true,
		};
	}

	/**
	 * Walks `list_symbols` page by page using cursor pagination. The
	 * daemon returns at most 100 items per call AND a `next_cursor`;
	 * we re-call with that cursor until it goes `null`. Defensive cap
	 * at 1 000 pages (100 k symbols) so a misbehaving daemon can't
	 * spin the loop forever.
	 */
	private async listAllSymbols(filters: Record<string, unknown>): Promise<ReadonlyArray<RawSymbol>> {
		const out: RawSymbol[] = [];
		let cursor: string | undefined;
		const MAX_PAGES = 1000;
		for (let page = 0; page < MAX_PAGES; page++) {
			const args: Record<string, unknown> = { ...filters, limit: 100 };
			if (cursor !== undefined) args.cursor = cursor;
			const raw = await this.callTool('list_symbols', args);
			const parsed = JSON.parse(raw) as ListSymbolsPage;
			if (!Array.isArray(parsed.items)) break;
			out.push(...parsed.items);
			if (parsed.next_cursor === null || parsed.next_cursor === undefined) break;
			cursor = parsed.next_cursor;
		}
		return out;
	}

	private async callTool(name: string, args: Record<string, unknown>): Promise<string> {
		const result = await this.client.callTool({ name, arguments: args });
		const content = (result as { content?: ReadonlyArray<{ type?: string; text?: string }> }).content;
		if (!content || content.length === 0) return '';
		const first = content[0];
		if (!first || typeof first.text !== 'string') return '';
		return first.text;
	}
}

function rawToBrowse(s: RawSymbol): BrowseSymbol {
	return {
		fqdn: s.fqdn,
		name: s.name,
		kind: s.kind,
		visibility: s.visibility,
		module: s.module,
		language_kind: s.language_kind,
		is_external: s.is_external === true,
		file: s.location.file,
		start_line: s.location.start_line,
	};
}

async function fetchAllSymbols(mcp: McpBrowse, includeExternal: boolean): Promise<{ symbols: BrowseSymbol[] }> {
	setStatus('listing modules…', 'loading');
	const modules = await mcp.listModules(includeExternal);

	const symbols: BrowseSymbol[] = [];
	const seenFqdn = new Set<string>();
	for (const m of modules) {
		if (!seenFqdn.has(m.fqdn)) {
			seenFqdn.add(m.fqdn);
			symbols.push(rawToBrowse(m));
		}
	}

	let processed = 0;
	for (const m of modules) {
		setStatus(`fetching module ${++processed}/${modules.length} (${symbols.length} symbols)`, 'loading');
		const children = await mcp.listInModule(m.fqdn, includeExternal).catch(() => [] as ReadonlyArray<RawSymbol>);
		for (const c of children) {
			if (seenFqdn.has(c.fqdn)) continue;
			seenFqdn.add(c.fqdn);
			symbols.push(rawToBrowse(c));
		}
	}
	return { symbols };
}

async function fetchEdgesFor(mcp: McpBrowse, fqdn: string): Promise<ReadonlyArray<BrowseEdge>> {
	const ctx = await mcp.getContext(fqdn).catch(() => null);
	if (ctx === null) return [];
	const edges: BrowseEdge[] = [];
	const collect = (list: ReadonlyArray<NeighborSymbol>, outbound: boolean): void => {
		for (const n of list) {
			const targetFqdn = resolveNeighborFqdn(n.target);
			if (targetFqdn === null) continue;
			edges.push({
				from: outbound ? fqdn : targetFqdn,
				to: outbound ? targetFqdn : fqdn,
				kind: n.edge_kind,
				outbound,
			});
		}
	};
	collect(ctx.callees, true);
	collect(ctx.imports, true);
	collect(ctx.callers, false);
	collect(ctx.imported_by, false);
	return edges;
}

function renderDetail(symbol: BrowseSymbol | null): void {
	if (symbol === null) {
		detailEl.innerHTML = '<h3>Pick a symbol</h3><p class="empty">Hover a chip in the canvas to inspect its edges; click to log its FQDN.</p>';
		return;
	}
	detailEl.innerHTML = `
		<h3>${escapeHtml(symbol.name)}</h3>
		<dl>
			<dt>fqdn</dt><dd><code>${escapeHtml(symbol.fqdn)}</code></dd>
			<dt>kind</dt><dd>${escapeHtml(symbol.kind)} (${escapeHtml(symbol.language_kind)})</dd>
			<dt>vis</dt><dd>${escapeHtml(symbol.visibility)}</dd>
			<dt>module</dt><dd><code>${escapeHtml(symbol.module ?? '(root)')}</code></dd>
			<dt>loc</dt><dd><code>${escapeHtml(symbol.file)}:${symbol.start_line}</code></dd>
			${symbol.is_external ? '<dt>ext</dt><dd>yes</dd>' : ''}
		</dl>
	`;
}

function escapeHtml(s: string): string {
	return s
		.replaceAll('&', '&amp;')
		.replaceAll('<', '&lt;')
		.replaceAll('>', '&gt;')
		.replaceAll('"', '&quot;')
		.replaceAll("'", '&#039;');
}

async function main(): Promise<void> {
	setStatus('initialising wasm…', 'loading');
	await init({ module_or_path: '/pkg/standardoc_graph_viz_bg.wasm' });

	// Hand the engine constructor to the graph component as a factory.
	// The component creates canvases on its own, then calls back into
	// us to instantiate the engine against the live 2D canvas.
	const ready = new Promise<void>((resolve, reject) => {
		graphEl.addEventListener('sd-graph-ready', () => resolve(), { once: true });
		graphEl.addEventListener(
			'sd-graph-error',
			e => {
				const { source, message } = (e as CustomEvent<GraphErrorDetail>).detail;
				if (source === 'engine-init') reject(new Error(message));
			},
			{ once: true },
		);
	});

	graphEl.engineFactory = (canvas, width, height, dpr) => new GraphEngine(canvas, width, height, dpr);

	try {
		await ready;
	} catch (e) {
		const reason = e instanceof Error ? e.message : String(e);
		setStatus(`engine init failed: ${reason}`, 'error');
		return;
	}

	const engine = graphEl.engine!;
	engine.set_palette(paletteFromCss());

	toolbarEl.setAttribute('webgpu-available', String(graphEl.webgpuAvailable));
	toolbarEl.setAttribute('mode', graphEl.currentMode);

	setStatus('connecting MCP…', 'loading');
	let mcp: McpBrowse;
	try {
		mcp = await McpBrowse.connect();
	} catch (e) {
		const reason = e instanceof Error ? e.message : String(e);
		setStatus(`MCP connect failed: ${reason}`, 'error');
		return;
	}

	let symbolByFqdn = new Map<string, BrowseSymbol>();
	let includeExternal = false;

	const loadGraph = async (): Promise<void> => {
		const { symbols } = await fetchAllSymbols(mcp, includeExternal);
		symbolByFqdn = new Map(symbols.map(s => [s.fqdn, s]));
		engine.load_graph(JSON.stringify({ symbols, edges: [] }));
		engine.fit();
		setStatus(`${symbols.length} symbols loaded`, 'ready');
	};

	await loadGraph();

	const edgesByFqdn = new Map<string, ReadonlyArray<BrowseEdge>>();
	let lastHovered: string | null = null;

	graphEl.addEventListener('sd-graph-hover', async e => {
		const { fqdn } = (e as CustomEvent<GraphHoverDetail>).detail;
		if (fqdn === null) return;
		lastHovered = fqdn;
		const symbol = symbolByFqdn.get(fqdn);
		renderDetail(symbol ?? null);

		let edges = edgesByFqdn.get(fqdn);
		if (edges === undefined) {
			edges = await fetchEdgesFor(mcp, fqdn);
			edgesByFqdn.set(fqdn, edges);
		}
		// If the user moved on to another chip while the fetch was
		// in-flight, skip the push — the newer hover already swapped
		// the scene to its own edge set.
		if (lastHovered !== fqdn) return;
		engine.set_edges(JSON.stringify({ edges }));
	});

	graphEl.addEventListener('sd-graph-click', e => {
		const { fqdn } = (e as CustomEvent<GraphClickDetail>).detail;
		console.log('[playground] click', fqdn);
		const symbol = symbolByFqdn.get(fqdn);
		renderDetail(symbol ?? null);
	});

	graphEl.addEventListener('sd-graph-mode-change', e => {
		const { mode } = (e as CustomEvent<GraphModeChangeDetail>).detail;
		toolbarEl.setAttribute('mode', mode);
		setStatus(`${symbolByFqdn.size} symbols loaded`, 'ready');
	});

	graphEl.addEventListener('sd-graph-error', e => {
		const { source, message } = (e as CustomEvent<GraphErrorDetail>).detail;
		setStatus(`${source}: ${message}`, 'error');
		if (source === 'webgpu-init') {
			toolbarEl.setAttribute('webgpu-available', 'false');
		}
	});

	// Toolbar intents → graph methods / data refresh.
	toolbarEl.addEventListener('sd-mode-request', e => {
		const { mode } = (e as CustomEvent<ToolbarModeRequestDetail>).detail;
		void graphEl.setMode(mode);
	});
	toolbarEl.addEventListener('sd-fit', () => engine.fit());
	toolbarEl.addEventListener('sd-reset-zoom', () => engine.reset_zoom());
	toolbarEl.addEventListener('sd-refetch', () => {
		edgesByFqdn.clear();
		void loadGraph();
	});
	toolbarEl.addEventListener('sd-externals-change', e => {
		const { value } = (e as CustomEvent<ToolbarFlagChangeDetail>).detail;
		includeExternal = value;
		toolbarEl.setAttribute('externals', String(includeExternal));
		edgesByFqdn.clear();
		void loadGraph();
	});

	// (a) Daemon revision watcher — polls `current_revision`
	// periodically and refetches the graph when the index revision
	// bumps (file watcher tick, manual re-index, etc.). 7 s cadence
	// is well below human-noticeable latency for "code changed →
	// graph updates" while keeping the request rate trivial.
	//
	// We picked polling over server-pushed SSE notifications because
	// the MCP daemon now runs in stateless + json_response mode (the
	// fix that resolved the "Session not found" cascade observed
	// when `bun run dev` and Claude Code shared the same daemon).
	// All notifications are coarse-grained ("revision bumped") — no
	// need for instant push. Multi-client collab is out of scope for
	// v1 (separate binary planned), so we don't need session
	// coordination either.
	const REVISION_POLL_MS = 7000;
	let lastRevision: number | null = null;
	let revisionPollBusy = false;
	const pollRevision = async (): Promise<void> => {
		if (revisionPollBusy) return;
		revisionPollBusy = true;
		try {
			const { revision, indexingReady } = await mcp.currentRevision();
			if (lastRevision === null) {
				lastRevision = revision;
				return;
			}
			if (revision !== lastRevision && indexingReady) {
				lastRevision = revision;
				edgesByFqdn.clear();
				await loadGraph();
			}
		} catch (e) {
			console.warn('[playground] revision poll failed:', e);
		} finally {
			revisionPollBusy = false;
		}
	};
	window.setInterval(() => {
		void pollRevision();
	}, REVISION_POLL_MS);

	// HUD wiring delegated to the lib `Profiler`.
	window.addEventListener('keydown', e => {
		if (e.code !== 'KeyP') return;
		const tag = (e.target as Element | null)?.tagName ?? '';
		if (tag === 'INPUT' || tag === 'TEXTAREA') return;
		hudEl.visible = !hudEl.visible;
	});

	const currentMode = (): RenderMode => graphEl.currentMode;
	const profiler = new Profiler({
		hud: hudEl,
		isPaused: () => graphEl.engineBusy,
		sample: () => ({
			symbolCount: engine.symbol_count(),
			edgeCount: engine.edge_count(),
			lastTickUs: engine.last_tick_us(),
			mode: currentMode(),
			gpu: engine.gpu_active()
				? {
						instanceCount: engine.gpu_instance_count(),
						instanceCapacity: engine.gpu_instance_capacity(),
					}
				: null,
		}),
	});

	hudEl.addEventListener('sd-hud-rec-start', e => {
		const detail = (e as CustomEvent<HudRecordStartDetail>).detail;
		profiler.startRecording({ mode: detail.mode });
	});
	hudEl.addEventListener('sd-hud-rec-stop', () => {
		if (!profiler.isRecording()) return;
		const result: RecordingResult = profiler.stopRecording();
		const blob = new Blob([JSON.stringify(result, null, 2)], { type: 'application/json' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = `standardoc-perf-${new Date().toISOString().replace(/[:.]/g, '-')}.json`;
		document.body.appendChild(a);
		a.click();
		document.body.removeChild(a);
		URL.revokeObjectURL(url);
		console.info(
			`[playground] recording: ${result.totalEvents} events ` +
				`over ${(result.durationMs / 1000).toFixed(1)}s ` +
				`(throttled=${result.skippedByThrottle}, coalesced=${result.coalescedByDedup})`,
		);
	});
	hudEl.addEventListener('sd-hud-copy', e => {
		const { success, bytes } = (e as CustomEvent<HudCopyDetail>).detail;
		if (success) console.info(`[playground] live snapshot copied (${bytes} bytes)`);
		else console.warn('[playground] live snapshot copy failed (clipboard denied?)');
	});

	const loop = (): void => {
		const now = performance.now();
		// `tick()` is the only `&mut self` call we still drive from the
		// host; everything else (pointer, resize, mode switch) is owned
		// by the graph component. Match the same engineBusy gate so we
		// don't race the async webgpu init.
		if (!graphEl.engineBusy) engine.tick();
		profiler.tick(now);
		requestAnimationFrame(loop);
	};
	requestAnimationFrame(loop);
}

void main().catch(e => {
	const reason = e instanceof Error ? e.message : String(e);
	setStatus(`boot failed: ${reason}`, 'error');
	console.error('[playground] boot failed', e);
});
