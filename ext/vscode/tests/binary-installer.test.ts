import { describe, expect, test } from 'bun:test';
import {
  currentPlatformTarget,
  InstallError,
  parseManifest,
  pickPlatformAsset,
  sha256Hex,
  verifySha256,
  type VersionManifest,
} from '../src/daemon/binary-installer';

const manifestFixture: VersionManifest = {
  core_version: '1.0.0-beta.2',
  ext_version: '1.0.0',
  protocol_version: 1,
  min_compat: { core: '1.0.0-beta.2', ext: '1.0.0' },
  released_at: '2026-05-15',
  binaries: {
    'x86_64-unknown-linux-gnu':
      'https://github.com/miralabs-tech/standardoc/releases/download/v1.0.0-beta.2/standardoc-v1.0.0-beta.2-x86_64-unknown-linux-gnu.tar.gz',
    'x86_64-pc-windows-msvc':
      'https://github.com/miralabs-tech/standardoc/releases/download/v1.0.0-beta.2/standardoc-v1.0.0-beta.2-x86_64-pc-windows-msvc.zip',
  },
  checksums_sha256: {
    'x86_64-unknown-linux-gnu': 'a'.repeat(64),
    'x86_64-pc-windows-msvc': 'B'.repeat(64),
  },
};

describe('currentPlatformTarget', () => {
  test('maps linux x64 to gnu triple', () => {
    expect(currentPlatformTarget('linux', 'x64')).toEqual({
      triple: 'x86_64-unknown-linux-gnu',
      archive: 'tar.gz',
      exe: 'standardoc',
    });
  });

  test('maps linux arm64', () => {
    expect(currentPlatformTarget('linux', 'arm64')?.triple).toBe('aarch64-unknown-linux-gnu');
  });

  test('maps darwin x64 and arm64', () => {
    expect(currentPlatformTarget('darwin', 'x64')?.triple).toBe('x86_64-apple-darwin');
    expect(currentPlatformTarget('darwin', 'arm64')?.triple).toBe('aarch64-apple-darwin');
  });

  test('maps win32 x64 to msvc + zip + .exe', () => {
    expect(currentPlatformTarget('win32', 'x64')).toEqual({
      triple: 'x86_64-pc-windows-msvc',
      archive: 'zip',
      exe: 'standardoc.exe',
    });
  });

  test('returns null for unsupported combos', () => {
    expect(currentPlatformTarget('win32', 'arm64')).toBeNull();
    expect(currentPlatformTarget('freebsd' as NodeJS.Platform, 'x64')).toBeNull();
  });
});

describe('parseManifest', () => {
  test('round-trips a valid manifest', () => {
    const text = JSON.stringify(manifestFixture);
    const parsed = parseManifest(text);
    expect(parsed.core_version).toBe('1.0.0-beta.2');
    expect(parsed.protocol_version).toBe(1);
    expect(Object.keys(parsed.binaries)).toContain('x86_64-unknown-linux-gnu');
  });

  test('throws on non-JSON', () => {
    expect(() => parseManifest('not json')).toThrow(InstallError);
  });

  test('throws on JSON array', () => {
    expect(() => parseManifest('[]')).toThrow(InstallError);
  });

  test('throws when a required field is missing', () => {
    const { protocol_version: _drop, ...rest } = manifestFixture;
    expect(() => parseManifest(JSON.stringify(rest))).toThrow(/protocol_version/);
  });

  test('throws when protocol_version is not a number', () => {
    expect(() =>
      parseManifest(JSON.stringify({ ...manifestFixture, protocol_version: 'one' })),
    ).toThrow(/protocol_version must be a number/);
  });

  test('throws when binaries values are not strings', () => {
    expect(() =>
      parseManifest(
        JSON.stringify({
          ...manifestFixture,
          binaries: { 'x86_64-unknown-linux-gnu': 42 },
        }),
      ),
    ).toThrow(/binaries must be string map/);
  });
});

describe('pickPlatformAsset', () => {
  test('returns url + lowercased sha for a matching target', () => {
    const got = pickPlatformAsset(manifestFixture, {
      triple: 'x86_64-pc-windows-msvc',
      archive: 'zip',
      exe: 'standardoc.exe',
    });
    expect(got.url).toBe(manifestFixture.binaries['x86_64-pc-windows-msvc']!);
    expect(got.sha256).toBe('b'.repeat(64));
  });

  test('throws when the target has no entry', () => {
    expect(() =>
      pickPlatformAsset(manifestFixture, {
        triple: 'aarch64-apple-darwin',
        archive: 'tar.gz',
        exe: 'standardoc',
      }),
    ).toThrow(/no entry for target aarch64-apple-darwin/);
  });
});

describe('sha256Hex / verifySha256', () => {
  test('sha256Hex produces a 64-char lowercase hex string', () => {
    const hex = sha256Hex(Buffer.from('hello world'));
    expect(hex).toMatch(/^[0-9a-f]{64}$/);
    expect(hex).toBe('b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9');
  });

  test('verifySha256 accepts a matching digest (case-insensitive)', () => {
    const buf = Buffer.from('hello world');
    const hex = sha256Hex(buf);
    expect(() => verifySha256(buf, hex)).not.toThrow();
    expect(() => verifySha256(buf, hex.toUpperCase())).not.toThrow();
  });

  test('verifySha256 throws InstallError on mismatch', () => {
    expect(() => verifySha256(Buffer.from('hello world'), '0'.repeat(64))).toThrow(InstallError);
  });
});
