import type {
  NeighborSymbolJson,
  RawSymbolJson,
  ResolvedOrUnresolvedJson,
  SymbolContextWithNeighborsJson,
} from './mcp/types';

const INDEXING_PREFIX = 'Workspace indexing in progress';

export type ToolResult<T> = { kind: 'ok'; value: T } | { kind: 'indexing'; message: string };

export function parseToolResult<T>(raw: string): ToolResult<T> {
  if (raw.startsWith(INDEXING_PREFIX)) {
    return { kind: 'indexing', message: raw };
  }
  return { kind: 'ok', value: JSON.parse(raw) as T };
}

export function pickTopFqdn(symbols: ReadonlyArray<RawSymbolJson>): string | null {
  if (symbols.length === 0) return null;
  const first = symbols[0];
  return first ? first.fqdn : null;
}

export function formatSymbolHeader(s: RawSymbolJson): string {
  const loc = `${s.location.file}:${s.location.start_line}`;
  return `=== ${s.fqdn} ===\nkind: ${s.kind} | visibility: ${s.visibility} | ${loc}`;
}

export function formatSymbolContext(ctx: SymbolContextWithNeighborsJson): string {
  const lines: string[] = [];
  lines.push(formatSymbolHeader(ctx.context.symbol));
  lines.push('');

  if (ctx.context.document_description) {
    lines.push('doc:');
    lines.push(indent(ctx.context.document_description));
    lines.push('');
  }
  if (ctx.context.enrichment_description) {
    lines.push('enrichment:');
    lines.push(indent(ctx.context.enrichment_description));
    lines.push('');
  }

  lines.push(formatNeighborGroup('callers', ctx.callers));
  lines.push(formatNeighborGroup('callees', ctx.callees));
  lines.push(formatNeighborGroup('imports', ctx.imports));
  lines.push(formatNeighborGroup('imported_by', ctx.imported_by));

  return lines.join('\n');
}

export function formatNeighborGroup(
  label: string,
  neighbors: ReadonlyArray<NeighborSymbolJson>,
): string {
  const header = `${label} (${neighbors.length}):`;
  if (neighbors.length === 0) return `${header}\n  (none)`;
  const body = neighbors.map(n => `  - ${targetLabel(n.target)} [${n.edge_kind}]`).join('\n');
  return `${header}\n${body}`;
}

export function targetLabel(t: ResolvedOrUnresolvedJson): string {
  if ('Resolved' in t) return t.Resolved.fqdn;
  if ('Unresolved' in t) return `<unresolved: ${t.Unresolved.name}>`;
  return `<bridge ${t.UnresolvedBridge.bridge}: ${t.UnresolvedBridge.name}>`;
}

function indent(text: string): string {
  return text
    .split('\n')
    .map(l => `  ${l}`)
    .join('\n');
}

export interface UsageStatsJson {
  readonly period: string;
  readonly calls: number;
  readonly bytes_out_total: number;
  readonly baseline_bytes_total: number;
  readonly bytes_saved: number;
  readonly ratio: number;
}

/**
 * Renders the aggregated usage_stats response as a one-line summary for a
 * VSCode information notification. Honest framing: baseline = sum of file
 * sizes of distinct source files referenced by responses (graph-grounded,
 * no estimation multiplier).
 */
export function formatUsageStats(stats: UsageStatsJson): string {
  if (stats.calls === 0) {
    return `Standardoc — no tool calls logged yet (${stats.period}).`;
  }
  const savedKb = (stats.bytes_saved / 1024).toFixed(1);
  const outKb = (stats.bytes_out_total / 1024).toFixed(1);
  const baselineKb = (stats.baseline_bytes_total / 1024).toFixed(1);
  const pct = (stats.ratio * 100).toFixed(1);
  return (
    `Standardoc — ${stats.calls} call(s) over ${stats.period}: ` +
    `returned ${outKb} KB vs ${baselineKb} KB raw (${pct}%) → saved ${savedKb} KB of AI context.`
  );
}

