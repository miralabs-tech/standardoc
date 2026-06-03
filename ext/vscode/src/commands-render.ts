import type {
  DeclKindJson,
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
  const kindLabel = s.decl_kind ? `${s.kind} (${formatDeclKind(s.decl_kind)})` : s.kind;
  return `=== ${s.fqdn} ===\nkind: ${kindLabel} | visibility: ${s.visibility} | ${loc}`;
}

export function formatDeclKind(d: DeclKindJson): string {
  if (typeof d === 'string') return d;
  return `custom:${d.custom.lang}:${d.custom.tag}`;
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
  switch (t.kind) {
    case 'resolved':
      return t.fqdn;
    case 'unresolved':
      return `<unresolved: ${t.name}>`;
    case 'unresolved_bridge':
      return `<bridge ${t.bridge}: ${t.name}>`;
  }
}

function indent(text: string): string {
  return text
    .split('\n')
    .map(l => `  ${l}`)
    .join('\n');
}
