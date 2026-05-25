// Thin wrapper over the MCP SDK Client surfacing the subset of tools
// the shell panels need (`fetch_graph`, `current_revision`). Designed
// so the host owns transport construction — the playground passes a
// `StreamableHTTPClientTransport` pointed at its dev-server proxy, a
// future VSCode webview can pass a `postMessage`-backed transport that
// relays through the extension host. The class itself is transport-
// agnostic past `connect()`.
//
// Tool-call result shape: `Client.callTool` returns
// `{ content: [{ type: 'text', text: '<json>' }, ...] }` — we read the
// first text block and `JSON.parse` it. Tools that return non-text
// (resources, images) are out of scope here.

import { Client } from '@modelcontextprotocol/sdk/client';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import type { Transport } from '@modelcontextprotocol/sdk/shared/transport.js';

import type {
	CurrentRevision,
	FetchGraphResponse,
	GetBodyResponse,
	GetContextResponse,
	ListProjectsResponse,
	ListSymbolsOptions,
	ListSymbolsResponse,
	RawSymbol,
} from './types';

export interface McpClientInfo {
	readonly name: string;
	readonly version: string;
}

const DEFAULT_CLIENT_INFO: McpClientInfo = {
	name: 'standardoc-graph-viz',
	version: '0.0.0',
};

export class McpBrowse {
	private constructor(private readonly client: Client) {}

	/**
	 * Connect with a caller-provided transport. Use this when embedding
	 * the lib in a host that doesn't speak HTTP (e.g. a VSCode webview
	 * tunnelling MCP through `postMessage`).
	 */
	static async connect(transport: Transport, info: McpClientInfo = DEFAULT_CLIENT_INFO): Promise<McpBrowse> {
		const client = new Client({ name: info.name, version: info.version }, { capabilities: {} });
		await client.connect(transport);
		return new McpBrowse(client);
	}

	/**
	 * Convenience for the streaming-HTTP case (playground dev server
	 * proxies `/mcp` to the daemon endpoint). Accepts a string or URL.
	 */
	static async connectHttp(endpoint: URL | string, info?: McpClientInfo): Promise<McpBrowse> {
		const url = typeof endpoint === 'string' ? new URL(endpoint) : endpoint;
		const transport = new StreamableHTTPClientTransport(url);
		return McpBrowse.connect(transport, info);
	}

	async fetchGraph(includeExternal: boolean): Promise<FetchGraphResponse> {
		// Single bounded snapshot — `fetch_graph` already does the JOIN
		// with files/projects server-side and returns the flat wire shape
		// the WASM engine consumes directly.
		const raw = await this.callTool('fetch_graph', {
			include_external: includeExternal,
			max_nodes: 5000,
		});
		return JSON.parse(raw) as FetchGraphResponse;
	}

	/**
	 * BFS expansion around `fqdn` up to the given `depth` (default 1).
	 * Unlike `get_context` (which only surfaces callers/callees/imports
	 * /imported_by — i.e. CALLS + IMPORTS), `fetch_graph` focal mode
	 * carries every edge kind: EXTENDS / IMPLEMENTS / USES_TYPE /
	 * REFERENCES too. Larger depths fan out quickly — the daemon
	 * caps the total node count so depth=4+ on a hub symbol may not
	 * round-trip the full reachable set.
	 */
	async fetchNeighborhood(fqdn: string, includeExternal: boolean, depth = 1): Promise<FetchGraphResponse> {
		const raw = await this.callTool('fetch_graph', {
			focal: fqdn,
			depth,
			include_external: includeExternal,
		});
		return JSON.parse(raw) as FetchGraphResponse;
	}

	/**
	 * Rich per-symbol context — symbol metadata + documentation
	 * + callers + callees + imports + imported_by. Used by the Symbol
	 * Details panel for the Overview tab.
	 *
	 * Note: this surfaces CALLS / IMPORTS edges only. For the full
	 * relation breakdown including USES_TYPE / TESTS / IMPLEMENTS /
	 * EXTENDS / REFERENCES, pair this with `fetchNeighborhood(fqdn, ...)`.
	 */
	async getContext(fqdn: string): Promise<GetContextResponse> {
		const raw = await this.callTool('get_context', { fqdn });
		return JSON.parse(raw) as GetContextResponse;
	}

	/**
	 * Source body for a symbol. The daemon strips any common leading
	 * indentation (returned via `dedented_prefix_len` for callers that
	 * want to reconstruct the original) and may truncate very long
	 * bodies — check `truncated` on the response.
	 */
	async getBody(fqdn: string): Promise<GetBodyResponse> {
		const raw = await this.callTool('get_body', { fqdn });
		return JSON.parse(raw) as GetBodyResponse;
	}

	/** Workspace project listing — feeds the Explorer tree top-level. */
	async listProjects(): Promise<ListProjectsResponse> {
		const raw = await this.callTool('list_projects', {});
		return JSON.parse(raw) as ListProjectsResponse;
	}

	/**
	 * Paginated structured listing. Use the `kind` / `module` /
	 * `visibility` / `externals` filters to narrow; loop on `next_cursor`
	 * for large modules. Default daemon limit applies when `limit` is
	 * omitted.
	 */
	async listSymbols(options: ListSymbolsOptions = {}): Promise<ListSymbolsResponse> {
		// Daemon-side argument names: ext / vis / kind / module / limit /
		// cursor. The 'externals' / 'visibility' aliases here are just
		// nicer-looking client-side spellings; they get translated on
		// the way out. Sending the long names directly was silently
		// dropped by the daemon (no error, just unfiltered results) —
		// which is how the playground walk used to burn its 25k cap on
		// '<builtin>::*' before reaching any workspace project.
		const args: Record<string, unknown> = {};
		if (options.kind !== undefined) args.kind = options.kind;
		if (options.module !== undefined) args.module = options.module;
		if (options.visibility !== undefined) args.vis = options.visibility;
		if (options.externals !== undefined) args.ext = options.externals;
		if (options.limit !== undefined) args.limit = options.limit;
		if (options.cursor !== undefined) args.cursor = options.cursor;
		const raw = await this.callTool('list_symbols', args);
		return JSON.parse(raw) as ListSymbolsResponse;
	}

	/**
	 * Fuzzy / glob pattern match. Feeds the global search autocomplete.
	 * Daemon caps the result count; `limit` lets the caller request fewer.
	 */
	async findSymbolsByPattern(pattern: string, limit?: number): Promise<ReadonlyArray<RawSymbol>> {
		const args: Record<string, unknown> = { pattern };
		if (limit !== undefined) args.limit = limit;
		const raw = await this.callTool('find_symbols_by_pattern', args);
		return JSON.parse(raw) as ReadonlyArray<RawSymbol>;
	}

	/**
	 * Lightweight liveness probe used by the revision watcher (client
	 * polling in lieu of SSE notifications). Returns the current daemon
	 * revision plus whether indexing has finished its cold-start sweep.
	 */
	async currentRevision(): Promise<CurrentRevision> {
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
