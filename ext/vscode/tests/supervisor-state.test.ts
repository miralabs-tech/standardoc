import { describe, expect, test } from 'bun:test';
import { describeState } from '../src/daemon/supervisor-state';

describe('describeState', () => {
  test('stopped', () => {
    expect(describeState({ kind: 'stopped' })).toBe('Stopped');
  });

  test('starting', () => {
    expect(describeState({ kind: 'starting' })).toBe('Starting');
  });

  test('ready includes pid', () => {
    expect(describeState({ kind: 'ready', pid: 1234 })).toBe('Ready (pid 1234)');
  });

  test('restarting includes attempt', () => {
    expect(describeState({ kind: 'restarting', attempt: 2 })).toBe('Restarting (attempt 2)');
  });

  test('failed includes reason', () => {
    expect(describeState({ kind: 'failed', reason: 'oops' })).toBe('Failed: oops');
  });
});
