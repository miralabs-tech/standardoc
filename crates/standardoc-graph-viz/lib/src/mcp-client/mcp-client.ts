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

import type { CurrentRevision, FetchGraphResponse } from './types';

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
	 * Depth-1 BFS expansion around `fqdn`. Unlike `get_context` (which
	 * only surfaces callers/callees/imports/imported_by — i.e. CALLS +
	 * IMPORTS), `fetch_graph` focal mode carries every edge kind:
	 * EXTENDS / IMPLEMENTS / USES_TYPE / REFERENCES too.
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
