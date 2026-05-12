export interface McpServerEntry {
  type?: 'http' | 'stdio';
  /** Set for `type=http` (or omitted-type-defaults-to-http) entries. */
  url?: string;
  /** Set for legacy `type=stdio` entries (kept for back-compat readers). */
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  [key: string]: unknown;
}

export interface McpConfigShape {
  mcpServers?: Record<string, McpServerEntry>;
  [key: string]: unknown;
}

export interface BuildEntryArgs {
  readonly endpointUrl: string;
}

export const STANDARDOC_SERVER_KEY = 'standardoc';

/**
 * Builds the canonical `.mcp.json` entry for Standardoc. Modern MCP
 * clients (Claude Code 4.6+, Claude Desktop, Cursor, Copilot Chat in
 * VSCode 1.105+) connect over HTTP/SSE to the daemon supervised by the
 * VSCode extension. Per-client stdio children are no longer needed.
 */
export function buildStandardocEntry(args: BuildEntryArgs): McpServerEntry {
  return {
    type: 'http',
    url: args.endpointUrl,
  };
}

export type ParseResult =
  | { kind: 'absent' }
  | { kind: 'invalid'; error: string }
  | { kind: 'parsed'; value: McpConfigShape };

export function parseMcpConfig(raw: string | null): ParseResult {
  if (raw === null) return { kind: 'absent' };
  if (raw.trim() === '') return { kind: 'parsed', value: {} };
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (e) {
    return { kind: 'invalid', error: e instanceof Error ? e.message : String(e) };
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return { kind: 'invalid', error: 'Root of .mcp.json must be a JSON object' };
  }
  return { kind: 'parsed', value: parsed as McpConfigShape };
}

export type MergeAction =
  | { kind: 'no-op' }
  | { kind: 'create'; result: McpConfigShape }
  | { kind: 'add-first'; result: McpConfigShape }
  | { kind: 'overwrite-stale'; result: McpConfigShape }
  | { kind: 'invalid'; error: string };

export function mergeMcpConfig(parsed: ParseResult, expected: McpServerEntry): MergeAction {
  if (parsed.kind === 'invalid') return { kind: 'invalid', error: parsed.error };

  const existing: McpConfigShape = parsed.kind === 'absent' ? {} : parsed.value;
  const servers = existing.mcpServers ?? {};
  const current = servers[STANDARDOC_SERVER_KEY];

  if (current && entryMatches(current, expected)) {
    return { kind: 'no-op' };
  }

  if (current) {
    // Preserve user-customised siblings (e.g. `env`) while pinning the
    // canonical fields (`type`, `url`, dropping any legacy stdio
    // `command`/`args`).
    const merged: McpServerEntry = {
      ...current,
      type: expected.type,
      url: expected.url,
    };
    delete merged.command;
    delete merged.args;
    return {
      kind: 'overwrite-stale',
      result: { ...existing, mcpServers: { ...servers, [STANDARDOC_SERVER_KEY]: merged } },
    };
  }

  const newServers: Record<string, McpServerEntry> = {
    [STANDARDOC_SERVER_KEY]: expected,
    ...servers,
  };
  const result: McpConfigShape = { ...existing, mcpServers: newServers };

  if (parsed.kind === 'absent' || Object.keys(servers).length === 0) {
    return { kind: 'create', result };
  }
  return { kind: 'add-first', result };
}

function entryMatches(actual: McpServerEntry, expected: McpServerEntry): boolean {
  if ((actual.type ?? 'stdio') !== (expected.type ?? 'stdio')) return false;
  if (expected.url !== undefined && actual.url !== expected.url) return false;
  if (expected.command !== undefined && actual.command !== expected.command) return false;
  if (expected.args !== undefined && !arrayEqualsString(actual.args, expected.args)) return false;
  return true;
}

function arrayEqualsString(a: unknown, b: string[]): boolean {
  if (!Array.isArray(a)) return false;
  if (a.length !== b.length) return false;
  for (let i = 0; i < b.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

export function serializeMcpConfig(config: McpConfigShape): string {
  return JSON.stringify(config, null, 2) + '\n';
}
