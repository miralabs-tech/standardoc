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

interface BrowseSymbol {
	readonly fqdn: string;
	readonly name: string;
	readonly kind: string;
	readonly visibility: string;
	readonly module: string | null;
	readonly language_kind: string;
	readonly language: string;
	readonly is_external: boolean;
	readonly file: string;
	readonly start_line: number;
	readonly project_id?: number | null;
}

interface BrowseEdge {
	readonly from: string;
	readonly to: string;
	readonly kind: string;
	readonly outbound: boolean;
}

interface BrowseProject {
	readonly project_id: number;
	readonly label: string;
	readonly kind: string;
	readonly rel_path: string;
}

interface FetchGraphResponse {
	readonly symbols: ReadonlyArray<BrowseSymbol>;
	readonly edges: ReadonlyArray<BrowseEdge>;
	readonly projects?: ReadonlyArray<BrowseProject>;
	readonly focal?: string | null;
}

const toolbarEl = document.getElementById('toolbar') as HTMLElement;
const hudEl = document.getElementById('hud') as HudElement;
const graphEl = document.getElementById('graph') as GraphElement;
const detailEl = document.getElementById('detail') as HTMLElement;
const breadcrumbEl = document.getElementById('breadcrumb') as HTMLElement;

interface FocusCrumb {
	readonly label: string;
	readonly id: number;
}

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

	async fetchGraph(includeExternal: boolean): Promise<FetchGraphResponse> {
		// Single bounded snapshot — `fetch_graph` already does the JOIN
		// with files/projects server-side and returns the flat wire shape
		// the WASM engine consumes directly. Replaces the previous
		// `list_symbols(kind=module)` + per-module `list_symbols(module=)`
		// N+1 walk and the `rawToBrowse` reshape it required.
		const raw = await this.callTool('fetch_graph', {
			include_external: includeExternal,
			max_nodes: 5000,
		});
		return JSON.parse(raw) as FetchGraphResponse;
	}

	/**
	 * Depth-1 BFS expansion around `fqdn`. Unlike `get_context` (which
	 * only surfaces callers/callees/imports/imported_by — i.e. CALLS +
	 * IMPORTS), `fetch_graph` focal mode carries every edge kind:
	 * EXTENDS / IMPLEMENTS / USES_TYPE / REFERENCES / DEFINES /
	 * EXPOSES_API too. The hover panel needs all of them.
	 */
	async fetchNeighborhood(fqdn: string, includeExternal: boolean): Promise<FetchGraphResponse> {
		const raw = await this.callTool('fetch_graph', {
			focal: fqdn,
			depth: 1,
			include_external: includeExternal,
		});
		return JSON.parse(raw) as FetchGraphResponse;
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

	private async callTool(name: string, args: Record<string, unknown>): Promise<string> {
		const result = await this.client.callTool({ name, arguments: args });
		const content = (result as { content?: ReadonlyArray<{ type?: string; text?: string }> }).content;
		if (!content || content.length === 0) return '';
		const first = content[0];
		if (!first || typeof first.text !== 'string') return '';
		return first.text;
	}
}

async function fetchEdgesFor(
	mcp: McpBrowse,
	fqdn: string,
	includeExternal: boolean,
): Promise<ReadonlyArray<BrowseEdge>> {
	const graph = await mcp.fetchNeighborhood(fqdn, includeExternal).catch(() => null);
	// `from`/`to` are already canonical (source → target) and the
	// payload carries every edge kind — no per-bucket reshape needed.
	return graph?.edges ?? [];
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
			<dt>lang</dt><dd>${escapeHtml(symbol.language || '(unknown)')}</dd>
			<dt>vis</dt><dd>${escapeHtml(symbol.visibility)}</dd>
			<dt>module</dt><dd><code>${escapeHtml(symbol.module ?? '(root)')}</code></dd>
			<dt>project</dt><dd>${symbol.project_id ?? '(orphan)'}</dd>
			<dt>loc</dt><dd><code>${escapeHtml(symbol.file)}:${symbol.start_line}</code></dd>
			${symbol.is_external ? '<dt>ext</dt><dd>yes</dd>' : ''}
		</dl>
	`;
}

const legendEl = document.getElementById('legend') as HTMLElement;
const legendBodyEl = document.getElementById('legend-body') as HTMLElement;
const legendToggleEl = document.getElementById('legend-toggle') as HTMLElement;

type PaletteMap = Record<string, string>;

// Legend sections — `[palette key, display label]`. Keys match the
// fields the engine serialises from its `Palette` struct.
const LEGEND_SECTIONS: ReadonlyArray<{
	readonly title: string;
	readonly rows: ReadonlyArray<readonly [string, string]>;
}> = [
	{
		title: 'Languages',
		rows: [
			['lang_rust', 'Rust'],
			['lang_typescript', 'TypeScript'],
			['lang_javascript', 'JavaScript'],
			['lang_c', 'C'],
			['lang_lua', 'Lua'],
			['lang_vue', 'Vue'],
			['lang_svelte', 'Svelte'],
		],
	},
	{
		title: 'Project kinds',
		rows: [
			['proj_rust', 'Cargo'],
			['proj_node', 'Node'],
			['proj_bun', 'Bun'],
			['proj_deno', 'Deno'],
			['proj_python', 'Python'],
			['proj_lua', 'Lua'],
			['proj_c', 'C'],
			['proj_cpp', 'C++'],
		],
	},
	{
		title: 'Edge kinds',
		rows: [
			['edge_calls', 'Calls'],
			['edge_imports', 'Imports'],
			['edge_extends', 'Extends'],
			['edge_implements', 'Implements'],
			['edge_references', 'References'],
			['edge_defines', 'Defines'],
			['edge_uses_type', 'Uses type'],
			['edge_exposes_api', 'Exposes API'],
		],
	},
];

function buildLegend(palette: PaletteMap): void {
	legendBodyEl.replaceChildren();
	for (const section of LEGEND_SECTIONS) {
		const wrap = document.createElement('div');
		const title = document.createElement('div');
		title.className = 'legend-section-title';
		title.textContent = section.title;
		wrap.appendChild(title);
		for (const [key, label] of section.rows) {
			const color = palette[key];
			if (color === undefined) continue;
			const row = document.createElement('div');
			row.className = 'legend-row';
			const swatch = document.createElement('span');
			swatch.className = 'legend-swatch';
			swatch.style.background = color;
			row.appendChild(swatch);
			const text = document.createElement('span');
			text.textContent = label;
			row.appendChild(text);
			wrap.appendChild(row);
		}
		legendBodyEl.appendChild(wrap);
	}
}

legendToggleEl.addEventListener('click', () => legendEl.classList.toggle('collapsed'));

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
	buildLegend(JSON.parse(engine.palette_json()) as PaletteMap);

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
		setStatus('fetching graph…', 'loading');
		const { symbols, projects, edges } = await mcp.fetchGraph(includeExternal);
		symbolByFqdn = new Map(symbols.map(s => [s.fqdn, s]));
		// Push symbols + projects + edges. Edges are needed at load
		// time now: the layout lays root projects out in dependency
		// columns, so `pack` must see the edge set. The hover handler
		// still narrows the *drawn* edges via `set_edges`.
		engine.load_graph(JSON.stringify({ symbols, projects: projects ?? [], edges }));
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
			edges = await fetchEdgesFor(mcp, fqdn, includeExternal);
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

	// Double-click a frame → zoom-to-fit it. `dblclick` is a composed
	// DOM event so it bubbles out of the graph component's shadow root;
	// we map client coords to the element's local box, same convention
	// the engine's pointer handlers expect.
	graphEl.addEventListener('dblclick', e => {
		const rect = graphEl.getBoundingClientRect();
		engine.on_double_click(e.clientX - rect.left, e.clientY - rect.top);
	});
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

	// Breadcrumb — the deepest frame path containing the viewport,
	// derived live from the engine. The JSON string is diffed so the
	// DOM is rebuilt only when the focus actually changes, not 60×/s.
	let lastBreadcrumb = '';
	const syncBreadcrumb = (): void => {
		const json = engine.focus_path();
		if (json === lastBreadcrumb) return;
		lastBreadcrumb = json;
		const crumbs = JSON.parse(json) as ReadonlyArray<FocusCrumb>;
		breadcrumbEl.replaceChildren();
		const addCrumb = (label: string, onClick: () => void): void => {
			const b = document.createElement('button');
			b.className = 'crumb';
			b.textContent = label;
			b.addEventListener('click', onClick);
			breadcrumbEl.appendChild(b);
		};
		// Static root crumb — clicking it fits the whole workspace.
		addCrumb('workspace', () => engine.fit());
		for (const c of crumbs) {
			const sep = document.createElement('span');
			sep.className = 'crumb-sep';
			sep.textContent = '›';
			breadcrumbEl.appendChild(sep);
			addCrumb(c.label, () => engine.fit_to_frame(c.id));
		}
	};

	const loop = (): void => {
		const now = performance.now();
		// `tick()` is the only `&mut self` call we still drive from the
		// host; everything else (pointer, resize, mode switch) is owned
		// by the graph component. Match the same engineBusy gate so we
		// don't race the async webgpu init.
		if (!graphEl.engineBusy) engine.tick();
		syncBreadcrumb();
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
