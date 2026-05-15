import { describe, expect, test } from 'bun:test';
import {
  DEFAULT_RAG_SETTINGS,
  coerceEmbedder,
  ragSpawnFlags,
} from '../src/daemon/rag-flags';

describe('DEFAULT_RAG_SETTINGS', () => {
  test('starts disabled with mock embedder (opt-in policy)', () => {
    expect(DEFAULT_RAG_SETTINGS).toEqual({ enabled: false, embedder: 'mock' });
  });
});

describe('coerceEmbedder', () => {
  test('accepts mock', () => {
    expect(coerceEmbedder('mock')).toBe('mock');
  });

  test('accepts candle', () => {
    expect(coerceEmbedder('candle')).toBe('candle');
  });

  test('falls back to mock on unknown value', () => {
    expect(coerceEmbedder('onnx')).toBe('mock');
    expect(coerceEmbedder('')).toBe('mock');
    expect(coerceEmbedder(undefined)).toBe('mock');
    expect(coerceEmbedder(null)).toBe('mock');
    expect(coerceEmbedder(42)).toBe('mock');
  });
});

describe('ragSpawnFlags', () => {
  test('returns no flags when RAG is disabled', () => {
    expect(ragSpawnFlags({ enabled: false, embedder: 'mock' })).toEqual([]);
    expect(ragSpawnFlags({ enabled: false, embedder: 'candle' })).toEqual([]);
  });

  test('returns --rag --embedder mock when enabled with mock', () => {
    expect(ragSpawnFlags({ enabled: true, embedder: 'mock' })).toEqual([
      '--rag',
      '--embedder',
      'mock',
    ]);
  });

  test('returns --rag --embedder candle when enabled with candle', () => {
    expect(ragSpawnFlags({ enabled: true, embedder: 'candle' })).toEqual([
      '--rag',
      '--embedder',
      'candle',
    ]);
  });

  test('return value is splat-safe (always an array)', () => {
    const args = ['mcp', '/ws', '--readonly', '--http', '0', ...ragSpawnFlags(DEFAULT_RAG_SETTINGS)];
    expect(args).toEqual(['mcp', '/ws', '--readonly', '--http', '0']);
  });

  test('splatting enabled settings yields the full arg vector', () => {
    const args = [
      'mcp',
      '/ws',
      '--readonly',
      '--http',
      '0',
      ...ragSpawnFlags({ enabled: true, embedder: 'candle' }),
    ];
    expect(args).toEqual([
      'mcp',
      '/ws',
      '--readonly',
      '--http',
      '0',
      '--rag',
      '--embedder',
      'candle',
    ]);
  });
});
