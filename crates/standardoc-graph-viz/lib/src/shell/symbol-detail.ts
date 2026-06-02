import type { McpBrowse } from '../mcp-client';
import type {
  GetContextResponse,
  RawSymbol,
} from '../mcp-client';
import type {
  EntryPointKind,
  SymbolDetail,
  SymbolRelationBucket,
  SymbolRelationKind,
  SymbolSubItem,
} from '../index';

import type { FileEntry, GraphEdge } from './types';
import { displayKindLabel, formatSignature, shortFqdn } from './symbols';

export function buildSymbolDetail(
  ctx: GetContextResponse,
  neighborhoodEdges: ReadonlyArray<GraphEdge>,
  subItems: ReadonlyArray<RawSymbol>,
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

  // Prefer the real kind off the edge's resolved_symbol (get_context
  // ships it at depth >= 2, the default) over a hardcoded guess so a
  // method / closure caller isn't mislabelled as a plain "fn".
  const edgeKindLabel = (sym: RawSymbol | null | undefined, fallback: string): string =>
    sym ? (sym.decl_kind ?? sym.language_kind ?? sym.kind) : fallback;

  for (const e of ctx.callers) {
    if (e.target.fqdn) pushBucket('usedBy', e.target.fqdn, edgeKindLabel(e.resolved_symbol, 'fn'));
  }
  for (const e of ctx.callees) {
    if (e.target.fqdn) pushBucket('calls', e.target.fqdn, edgeKindLabel(e.resolved_symbol, 'fn'));
  }
  // `ctx.imports` = OUTBOUND imports from this symbol (what it pulls in).
  // `ctx.imported_by` = INBOUND imports (who imports this symbol).
  // Used to be collapsed into the same `importedBy` bucket which mis-
  // labelled outbound imports as "Imported by" in the panel.
  for (const e of ctx.imports) {
    if (e.target.fqdn) pushBucket('imports', e.target.fqdn, edgeKindLabel(e.resolved_symbol, 'mod'));
  }
  for (const e of ctx.imported_by) {
    if (e.target.fqdn) pushBucket('importedBy', e.target.fqdn, edgeKindLabel(e.resolved_symbol, 'mod'));
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
        // Split on direction like CALLS — an outbound import is "imports",
        // not "importedBy" (mirror of the ctx-path fix above; collapsing
        // both into importedBy re-introduced the double-listing bug).
        if (e.outbound) pushBucket('imports', other, 'mod');
        else pushBucket('importedBy', other, 'mod');
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
    'usedBy', 'usesTypes', 'calls', 'imports', 'importedBy', 'testedBy', 'implements', 'extends',
  ];
  const relations: SymbolRelationBucket[] = [];
  for (const k of orderedKinds) {
    const m = buckets.get(k);
    if (m === undefined || m.size === 0) continue;
    const items = [...m.values()];
    relations.push({ kind: k, items, total: items.length });
  }

  const fields: SymbolSubItem[] = [];
  const methods: SymbolSubItem[] = [];
  for (const s of subItems) {
    const cls = classifySubItem(s);
    if (cls === null) continue;
    // Field-shaped items (no params, return = the field's type)
    // get a standalone `type` so the Fields tab can render the
    // type as its own chip. Methods leave it null — their return
    // type is already inside `signature`.
    const sig = s.signature;
    const fieldType = sig && (!sig.params || sig.params.length === 0)
      ? (sig.returns?.display ?? null)
      : null;
    const item: SymbolSubItem = {
      fqdn: s.fqdn,
      name: s.name,
      kindLabel: displayKindLabel(s),
      file: s.location.file,
      startLine: s.location.start_line,
      signature: formatSignature(s.signature),
      visibility: s.visibility ?? null,
      isAsync: Array.isArray(s.flags) && s.flags.includes('async'),
      type: fieldType,
    };
    if (cls === 'field') fields.push(item);
    else methods.push(item);
  }
  fields.sort((a, b) => (a.startLine ?? 0) - (b.startLine ?? 0));
  methods.sort((a, b) => (a.startLine ?? 0) - (b.startLine ?? 0));

  return {
    fqdn: sym.fqdn,
    name: sym.name,
    kindLabel: displayKindLabel(sym),
    visibility: sym.visibility,
    file: sym.location.file,
    startLine: sym.location.start_line,
    documentation: doc,
    entryPointKind: epKind,
    fields,
    methods,
    relations,
  };
}

/**
 * Best-effort sub-symbols fetch for a parent FQDN. `list_symbols`
 * scoped by `module = parentFqdn` returns the direct children that the
 * extractors registered as nested symbols (Rust struct fields / enum
 * variants / impl methods, TS interface properties / class methods).
 * Bounded by SUB_PAGE_SIZE — structs with > 200 members are vanishingly
 * rare and we don't paginate here; the daemon caps `limit` at u8 (255)
 * so SUB_PAGE_SIZE stays safely below.
 */
const SUB_PAGE_SIZE = 200;
export async function fetchSubItems(mcp: McpBrowse, fqdn: string): Promise<ReadonlyArray<RawSymbol>> {
  try {
    const res = await mcp.listSymbols({ module: fqdn, limit: SUB_PAGE_SIZE });
    return res.items;
  } catch {
    return [];
  }
}

/**
 * Classify a sub-symbol returned by `list_symbols({ module: parentFqdn })`
 * into the Fields or Methods tab bucket. Falls back through decl_kind →
 * language_kind so the heuristic catches both Rust (`field` / `method`)
 * and TS (`interface_property` / `class_method`) shapes. Unrecognised
 * sub-symbols (associated consts, nested types, etc.) are dropped from
 * V0 — they need their own tab to render cleanly.
 */
function classifySubItem(s: RawSymbol): 'field' | 'method' | null {
  const dk = s.decl_kind;
  const lk = s.language_kind;
  if (dk === 'field' || dk === 'variant') return 'field';
  if (lk === 'field' || lk === 'interface_property' || lk === 'enum_variant' || lk === 'class_property' || lk === 'struct_field') return 'field';
  if (dk === 'method' || dk === 'function') return 'method';
  if (lk === 'method' || lk === 'class_method' || lk === 'function') return 'method';
  return null;
}

export function buildFileSyntheticDetail(file: FileEntry): SymbolDetail {
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
    fields: [],
    methods: [],
    relations: [{
      kind: 'definedHere',
      items: file.symbols.map(s => ({
        fqdn: s.fqdn,
        label: s.name,
        kindLabel: displayKindLabel(s),
      })),
      total: file.symbols.length,
    }],
  };
}
