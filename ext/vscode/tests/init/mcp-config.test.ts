import { describe, expect, test } from 'bun:test';
import {
  buildStandardocEntry,
  mergeMcpConfig,
  parseMcpConfig,
  serializeMcpConfig,
  type McpConfigShape,
  type McpServerEntry,
} from '../../src/init/mcp-config';

const EXPECTED = buildStandardocEntry({
  endpointUrl: 'http://127.0.0.1:7700/mcp',
});

describe('parseMcpConfig', () => {
  test('returns absent when raw is null', () => {
    expect(parseMcpConfig(null)).toEqual({ kind: 'absent' });
  });

  test('returns parsed empty for whitespace-only string', () => {
    expect(parseMcpConfig('   \n')).toEqual({ kind: 'parsed', value: {} });
  });

  test('returns invalid for malformed JSON', () => {
    const r = parseMcpConfig('{');
    expect(r.kind).toBe('invalid');
  });

  test('returns invalid when root is JSON array', () => {
    const r = parseMcpConfig('[1, 2, 3]');
    expect(r.kind).toBe('invalid');
  });

  test('returns invalid when root is JSON null', () => {
    const r = parseMcpConfig('null');
    expect(r.kind).toBe('invalid');
  });

  test('returns parsed value for object root', () => {
    const r = parseMcpConfig('{"mcpServers": {}}');
    expect(r).toEqual({ kind: 'parsed', value: { mcpServers: {} } });
  });
});

describe('mergeMcpConfig', () => {
  test('propagates invalid', () => {
    const r = mergeMcpConfig({ kind: 'invalid', error: 'boom' }, EXPECTED);
    expect(r).toEqual({ kind: 'invalid', error: 'boom' });
  });

  test('creates fresh when absent', () => {
    const r = mergeMcpConfig({ kind: 'absent' }, EXPECTED);
    expect(r.kind).toBe('create');
    if (r.kind !== 'create') return;
    expect(r.result.mcpServers).toEqual({ standardoc: EXPECTED });
  });

  test('creates when parsed but mcpServers missing', () => {
    const r = mergeMcpConfig({ kind: 'parsed', value: {} }, EXPECTED);
    expect(r.kind).toBe('create');
  });

  test('add-first when other servers exist', () => {
    const existing: McpConfigShape = {
      mcpServers: {
        other: { type: 'stdio', command: '/x/other', args: ['run'] },
      },
    };
    const r = mergeMcpConfig({ kind: 'parsed', value: existing }, EXPECTED);
    expect(r.kind).toBe('add-first');
    if (r.kind !== 'add-first') return;
    const keys = Object.keys(r.result.mcpServers ?? {});
    expect(keys[0]).toBe('standardoc');
    expect(keys).toContain('other');
  });

  test('no-op when standardoc entry already correctly configured', () => {
    const existing: McpConfigShape = {
      mcpServers: { standardoc: EXPECTED },
    };
    const r = mergeMcpConfig({ kind: 'parsed', value: existing }, EXPECTED);
    expect(r.kind).toBe('no-op');
  });

  test('overwrite-stale when url differs', () => {
    const stale: McpServerEntry = {
      type: 'http',
      url: 'http://127.0.0.1:9999/mcp',
    };
    const existing: McpConfigShape = { mcpServers: { standardoc: stale } };
    const r = mergeMcpConfig({ kind: 'parsed', value: existing }, EXPECTED);
    expect(r.kind).toBe('overwrite-stale');
    if (r.kind !== 'overwrite-stale') return;
    expect(r.result.mcpServers?.standardoc?.url).toBe('http://127.0.0.1:7700/mcp');
  });

  test('overwrite-stale migrates legacy stdio entry to http (drops command/args)', () => {
    const stale: McpServerEntry = {
      type: 'stdio',
      command: '/old/path/standardoc.exe',
      args: ['mcp', '/abs/workspace', '--readonly'],
    };
    const existing: McpConfigShape = { mcpServers: { standardoc: stale } };
    const r = mergeMcpConfig({ kind: 'parsed', value: existing }, EXPECTED);
    expect(r.kind).toBe('overwrite-stale');
    if (r.kind !== 'overwrite-stale') return;
    expect(r.result.mcpServers?.standardoc?.type).toBe('http');
    expect(r.result.mcpServers?.standardoc?.url).toBe('http://127.0.0.1:7700/mcp');
    expect(r.result.mcpServers?.standardoc?.command).toBeUndefined();
    expect(r.result.mcpServers?.standardoc?.args).toBeUndefined();
  });

  test('overwrite-stale preserves user-customised env field', () => {
    const stale: McpServerEntry = {
      type: 'http',
      url: 'http://127.0.0.1:9999/mcp',
      env: { RUST_LOG: 'debug' },
    };
    const existing: McpConfigShape = { mcpServers: { standardoc: stale } };
    const r = mergeMcpConfig({ kind: 'parsed', value: existing }, EXPECTED);
    expect(r.kind).toBe('overwrite-stale');
    if (r.kind !== 'overwrite-stale') return;
    expect(r.result.mcpServers?.standardoc?.env).toEqual({ RUST_LOG: 'debug' });
    expect(r.result.mcpServers?.standardoc?.url).toBe('http://127.0.0.1:7700/mcp');
  });

  test('preserves sibling top-level keys outside mcpServers', () => {
    const existing: McpConfigShape = {
      mcpServers: {},
      customMetadata: { lastEditedBy: 'user' } as never,
    };
    const r = mergeMcpConfig({ kind: 'parsed', value: existing }, EXPECTED);
    expect(r.kind).toBe('create');
    if (r.kind !== 'create') return;
    expect((r.result as Record<string, unknown>).customMetadata).toEqual({
      lastEditedBy: 'user',
    });
  });
});

describe('serializeMcpConfig', () => {
  test('produces 2-space indented JSON with trailing newline', () => {
    const out = serializeMcpConfig({ mcpServers: { standardoc: EXPECTED } });
    expect(out.endsWith('\n')).toBe(true);
    expect(out).toContain('  "mcpServers"');
    expect(out).toContain('    "standardoc"');
  });

  test('round-trips through parseMcpConfig as parsed', () => {
    const out = serializeMcpConfig({ mcpServers: { standardoc: EXPECTED } });
    const back = parseMcpConfig(out);
    expect(back.kind).toBe('parsed');
    if (back.kind !== 'parsed') return;
    expect(back.value.mcpServers?.standardoc).toEqual(EXPECTED);
  });
});

describe('buildStandardocEntry', () => {
  test('generates type=http with url pointing at the shared daemon', () => {
    const e = buildStandardocEntry({ endpointUrl: 'http://127.0.0.1:7700/mcp' });
    expect(e.type).toBe('http');
    expect(e.url).toBe('http://127.0.0.1:7700/mcp');
    expect(e.command).toBeUndefined();
    expect(e.args).toBeUndefined();
  });
});
