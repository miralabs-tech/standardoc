import { describe, expect, test } from 'bun:test';
import * as path from 'node:path';
import {
  BinaryNotFoundError,
  resolveBinaryWith,
  type ResolveDeps,
} from '../src/daemon/binary-resolver';

const baseDeps = (overrides: Partial<ResolveDeps> = {}): ResolveDeps => ({
  settingsPath: undefined,
  bundledPath: '/ext/dist/bin/stdoc',
  pathEnv: undefined,
  pathSeparator: ':',
  exeName: 'stdoc',
  existsSync: () => false,
  ...overrides,
});

describe('resolveBinaryWith', () => {
  test('returns settings path when set and existing', () => {
    const r = resolveBinaryWith(
      baseDeps({
        settingsPath: '/custom/stdoc',
        existsSync: p => p === '/custom/stdoc',
      }),
    );
    expect(r).toEqual({ path: '/custom/stdoc', source: 'settings' });
  });

  test('throws when settings path is set but does not exist', () => {
    expect(() =>
      resolveBinaryWith(
        baseDeps({
          settingsPath: '/missing/stdoc',
        }),
      ),
    ).toThrow(BinaryNotFoundError);
  });

  test('falls back to bundled path when settings empty and bundled exists', () => {
    const r = resolveBinaryWith(
      baseDeps({
        settingsPath: '',
        bundledPath: '/ext/dist/bin/stdoc',
        existsSync: p => p === '/ext/dist/bin/stdoc',
      }),
    );
    expect(r).toEqual({ path: '/ext/dist/bin/stdoc', source: 'bundled' });
  });

  test('falls back to PATH lookup when settings empty and bundled missing', () => {
    const expected = path.join('/usr/bin', 'stdoc');
    const r = resolveBinaryWith(
      baseDeps({
        pathEnv: ['/usr/local/bin', '/usr/bin'].join(':'),
        pathSeparator: ':',
        exeName: 'stdoc',
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
});
