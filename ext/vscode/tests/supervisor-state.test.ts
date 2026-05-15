import { describe, expect, test } from 'bun:test';
import { describeState } from '../src/daemon/supervisor-state';

describe('describeState', () => {
  test('stopped', () => {
    expect(describeState({ kind: 'stopped' })).toBe('Stopped');
  });

  test('starting', () => {
    expect(describeState({ kind: 'starting' })).toBe('Starting');
  });

  test('ready is bare (no pid placeholder leak)', () => {
    // 'ready' aggregates two children (LSP + MCP) — there is no single
    // PID to surface. Earlier versions leaked `pid 0` here.
    expect(describeState({ kind: 'ready' })).toBe('Ready');
  });

  test('restarting includes attempt', () => {
    expect(describeState({ kind: 'restarting', attempt: 2 })).toBe('Restarting (attempt 2)');
  });

  test('failed includes reason', () => {
    expect(describeState({ kind: 'failed', reason: 'oops' })).toBe('Failed: oops');
  });

  test('fatal_config schema_too_new is actionable', () => {
    const text = describeState({
      kind: 'fatal_config',
      config: { kind: 'schema_too_new', db: 2, supported: 1 },
    });
    expect(text).toContain('Fatal config:');
    expect(text).toContain('v2');
    expect(text).toContain('v1');
  });

  test('fatal_config unknown falls back to raw line', () => {
    const text = describeState({
      kind: 'fatal_config',
      config: {
        kind: 'unknown',
        code: 'lock_held',
        raw: 'STDOC_FATAL: lock_held path=/tmp/db.lock',
      },
    });
    expect(text).toContain('Fatal config:');
    expect(text).toContain('lock_held');
  });
});
