import { describe, expect, test } from 'bun:test';
import {
  formatNeighborGroup,
  formatSymbolContext,
  formatSymbolHeader,
  parseToolResult,
  pickTopFqdn,
  targetLabel,
} from '../src/commands-render';
import type {
  NeighborSymbolJson,
  RawSymbolJson,
  SymbolContextWithNeighborsJson,
} from '../src/mcp/types';

const sampleSymbol: RawSymbolJson = {
  name: 'parse_workspace',
  fqdn: 'standardoc_core::pipeline::parse_workspace',
  kind: 'function',
  language_kind: 'rust',
  module: 'standardoc_core::pipeline',
  visibility: 'public',
  location: {
    file: 'crates/standardoc-core/src/pipeline/mod.rs',
    start_line: 42,
    end_line: 87,
    start_col: 0,
    end_col: 1,
  },
  body_hash: 'abcdef0123',
};

describe('parseToolResult', () => {
  test('detects indexing-in-progress prefix', () => {
    const raw = 'Workspace indexing in progress (10/100 files). Please retry.';
    const r = parseToolResult<unknown>(raw);
    expect(r.kind).toBe('indexing');
    if (r.kind === 'indexing') expect(r.message).toBe(raw);
  });

  test('parses JSON when not indexing', () => {
    const r = parseToolResult<{ a: number }>('{"a":1}');
    expect(r.kind).toBe('ok');
    if (r.kind === 'ok') expect(r.value).toEqual({ a: 1 });
  });

  test('throws on invalid JSON when not indexing', () => {
    expect(() => parseToolResult<unknown>('{not json')).toThrow();
  });
});

describe('pickTopFqdn', () => {
  test('returns null on empty array', () => {
    expect(pickTopFqdn([])).toBeNull();
  });
  test('returns first fqdn when populated', () => {
    expect(pickTopFqdn([sampleSymbol])).toBe(sampleSymbol.fqdn);
  });
});

describe('targetLabel', () => {
  test('Resolved → fqdn', () => {
    expect(targetLabel({ Resolved: { fqdn: 'a::b' } })).toBe('a::b');
  });
  test('Unresolved → bracketed name', () => {
    expect(targetLabel({ Unresolved: { name: 'foo' } })).toBe('<unresolved: foo>');
  });
  test('UnresolvedBridge → bracketed bridge+name', () => {
    expect(targetLabel({ UnresolvedBridge: { bridge: 'tauri', name: 'cmd' } })).toBe(
      '<bridge tauri: cmd>',
    );
  });
});

describe('formatSymbolHeader', () => {
  test('contains fqdn, kind, visibility, location', () => {
    const s = formatSymbolHeader(sampleSymbol);
    expect(s).toContain(sampleSymbol.fqdn);
    expect(s).toContain('function');
    expect(s).toContain('public');
    expect(s).toContain('crates/standardoc-core/src/pipeline/mod.rs:42');
  });
});

describe('formatNeighborGroup', () => {
  test('empty group renders (none)', () => {
    expect(formatNeighborGroup('callers', [])).toBe('callers (0):\n  (none)');
  });

  test('non-empty group lists each neighbor with edge_kind', () => {
    const ns: NeighborSymbolJson[] = [
      { edge_kind: 'CALLS', target: { Resolved: { fqdn: 'a::b' } }, resolved_symbol: null },
      { edge_kind: 'CALLS', target: { Unresolved: { name: 'unk' } }, resolved_symbol: null },
    ];
    const s = formatNeighborGroup('callees', ns);
    expect(s).toContain('callees (2):');
    expect(s).toContain('- a::b [CALLS]');
    expect(s).toContain('- <unresolved: unk> [CALLS]');
  });
});

describe('formatSymbolContext', () => {
  const ctx: SymbolContextWithNeighborsJson = {
    context: {
      symbol: sampleSymbol,
      enrichment_description: 'Custom enrichment.',
      document_description: 'Documents the parse pipeline.',
    },
    callers: [
      { edge_kind: 'CALLS', target: { Resolved: { fqdn: 'cli::main' } }, resolved_symbol: null },
    ],
    callees: [],
    imports: [
      { edge_kind: 'IMPORTS', target: { Resolved: { fqdn: 'std::sync::Arc' } }, resolved_symbol: null },
    ],
    imported_by: [],
  };

  test('emits header + descriptions + 4 groups', () => {
    const s = formatSymbolContext(ctx);
    expect(s).toContain(sampleSymbol.fqdn);
    expect(s).toContain('Documents the parse pipeline.');
    expect(s).toContain('Custom enrichment.');
    expect(s).toContain('callers (1):');
    expect(s).toContain('callees (0):');
    expect(s).toContain('imports (1):');
    expect(s).toContain('imported_by (0):');
  });

  test('omits doc/enrichment lines when null', () => {
    const ctxBare: SymbolContextWithNeighborsJson = {
      ...ctx,
      context: { ...ctx.context, enrichment_description: null, document_description: null },
    };
    const s = formatSymbolContext(ctxBare);
    expect(s).not.toContain('doc:');
    expect(s).not.toContain('enrichment:');
  });
});
