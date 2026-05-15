import { describe, expect, test } from 'bun:test';
import * as path from 'node:path';
import {
  BinaryNotFoundError,
  resolveBinaryWith,
  type ResolveDeps,
} from '../src/daemon/binary-resolver';

const baseDeps = (overrides: Partial<ResolveDeps> = {}): ResolveDeps => ({
  settingsPath: undefined,
  globalStoragePath: '/storage/bin/x86_64-unknown-linux-gnu/standardoc',
  pathEnv: undefined,
  pathSeparator: ':',
  exeName: 'standardoc',
  existsSync: () => false,
  ...overrides,
});

describe('resolveBinaryWith', () => {
  test('returns settings path when set and existing', () => {
    const r = resolveBinaryWith(
      baseDeps({
        settingsPath: '/custom/standardoc',
        existsSync: p => p === '/custom/standardoc',
      }),
    );
    expect(r).toEqual({ path: '/custom/standardoc', source: 'settings' });
  });

  test('throws when settings path is set but does not exist', () => {
    expect(() =>
      resolveBinaryWith(
        baseDeps({
          settingsPath: '/missing/standardoc',
        }),
      ),
    ).toThrow(BinaryNotFoundError);
  });

  test('falls back to globalStorage path when settings empty and storage binary exists', () => {
    const storagePath = '/storage/bin/x86_64-unknown-linux-gnu/standardoc';
    const r = resolveBinaryWith(
      baseDeps({
        settingsPath: '',
        globalStoragePath: storagePath,
        existsSync: p => p === storagePath,
      }),
    );
    expect(r).toEqual({ path: storagePath, source: 'globalStorage' });
  });

  test('falls back to PATH lookup when settings empty and globalStorage missing', () => {
    const expected = path.join('/usr/bin', 'standardoc');
    const r = resolveBinaryWith(
      baseDeps({
        pathEnv: ['/usr/local/bin', '/usr/bin'].join(':'),
        pathSeparator: ':',
        exeName: 'standardoc',
        existsSync: p => p === expected,
      }),
    );
    expect(r).toEqual({ path: expected, source: 'path' });
  });

  test('throws BinaryNotFoundError when nothing matches', () => {
    expect(() =>
      resolveBinaryWith(
        baseDeps({
          pathEnv: '/usr/bin',
        }),
      ),
    ).toThrow(BinaryNotFoundError);
  });

  test('settings takes priority over globalStorage even when both exist', () => {
    const r = resolveBinaryWith(
      baseDeps({
        settingsPath: '/custom/standardoc',
        globalStoragePath: '/storage/bin/x86_64-unknown-linux-gnu/standardoc',
        existsSync: () => true,
      }),
    );
    expect(r.source).toBe('settings');
    expect(r.path).toBe('/custom/standardoc');
  });

  test('globalStorage takes priority over PATH when both exist', () => {
    const storagePath = '/storage/bin/x86_64-unknown-linux-gnu/standardoc';
    const r = resolveBinaryWith(
      baseDeps({
        globalStoragePath: storagePath,
        pathEnv: '/usr/bin',
        existsSync: () => true,
      }),
    );
    expect(r.source).toBe('globalStorage');
    expect(r.path).toBe(storagePath);
  });
});
