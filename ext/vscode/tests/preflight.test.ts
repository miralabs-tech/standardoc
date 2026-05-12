import { describe, expect, test } from 'bun:test';
import {
  preflightSchemaVersion,
  type ExecFn,
} from '../src/daemon/preflight';

function execReturning(stdout: string): ExecFn {
  return async () => ({ stdout, stderr: '' });
}

function execRejects(reason: string): ExecFn {
  return async () => {
    throw new Error(reason);
  };
}

describe('preflightSchemaVersion', () => {
  test('returns ok when the binary reports compatible', async () => {
    const exec = execReturning(JSON.stringify({ db: 3, supported: 4, compatible: true }));
    const result = await preflightSchemaVersion('/bin/standardoc', '/workspace', exec);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.db).toBe(3);
      expect(result.supported).toBe(4);
    }
  });

  test('treats db=null as compatible (fresh workspace, no index yet)', async () => {
    const exec = execReturning(JSON.stringify({ db: null, supported: 4, compatible: true }));
    const result = await preflightSchemaVersion('/bin/standardoc', '/workspace', exec);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.db).toBeNull();
    }
  });

  test('fails when binary reports incompatible (db > supported)', async () => {
    const exec = execReturning(JSON.stringify({ db: 7, supported: 4, compatible: false }));
    const result = await preflightSchemaVersion('/bin/standardoc', '/workspace', exec);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.db).toBe(7);
      expect(result.supported).toBe(4);
      expect(result.reason).toContain('v7');
      expect(result.reason).toContain('v4');
    }
  });

  test('fails when exec rejects (binary not found, IO error, ...)', async () => {
    const exec = execRejects('ENOENT: no such file or directory');
    const result = await preflightSchemaVersion('/bin/standardoc', '/workspace', exec);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain('ENOENT');
      expect(result.supported).toBeNull();
    }
  });

  test('fails when stdout is not valid JSON', async () => {
    const exec = execReturning('this is not json');
    const result = await preflightSchemaVersion('/bin/standardoc', '/workspace', exec);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain('not valid JSON');
    }
  });

  test('fails when JSON has wrong shape (missing required fields)', async () => {
    const exec = execReturning(JSON.stringify({ db: 3 }));
    const result = await preflightSchemaVersion('/bin/standardoc', '/workspace', exec);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain('unexpected shape');
    }
  });

  test('invokes exec with schema-version subcommand and workspace path', async () => {
    let capturedBinary = '';
    let capturedArgs: string[] = [];
    const exec: ExecFn = async (bin, args) => {
      capturedBinary = bin;
      capturedArgs = args;
      return { stdout: JSON.stringify({ db: null, supported: 4, compatible: true }), stderr: '' };
    };
    await preflightSchemaVersion('/bin/standardoc', '/some/workspace', exec);
    expect(capturedBinary).toBe('/bin/standardoc');
    expect(capturedArgs).toEqual(['schema-version', '/some/workspace']);
  });

  test('tolerates surrounding whitespace in stdout', async () => {
    const exec = execReturning(`  ${JSON.stringify({ db: 1, supported: 4, compatible: true })}\n`);
    const result = await preflightSchemaVersion('/bin/standardoc', '/workspace', exec);
    expect(result.ok).toBe(true);
  });
});
