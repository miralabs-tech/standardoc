import { describe, expect, test } from 'bun:test';
import {
  describeFatalConfig,
  findFatalMarker,
  parseFatalMarker,
} from '../src/daemon/fatal-marker';

describe('parseFatalMarker', () => {
  test('returns null on lines without prefix', () => {
    expect(parseFatalMarker('error: schema_too_new db=2 supported=1')).toBeNull();
    expect(parseFatalMarker('')).toBeNull();
    expect(parseFatalMarker('arbitrary log line')).toBeNull();
  });

  test('parses schema_too_new with two integer fields', () => {
    const m = parseFatalMarker('STDOC_FATAL: schema_too_new db=2 supported=1');
    expect(m).toEqual({ kind: 'schema_too_new', db: 2, supported: 1 });
  });

  test('tolerates leading and trailing whitespace', () => {
    const m = parseFatalMarker('  STDOC_FATAL: schema_too_new db=2 supported=1  \n');
    expect(m).toEqual({ kind: 'schema_too_new', db: 2, supported: 1 });
  });

  test('field order is irrelevant', () => {
    const m = parseFatalMarker('STDOC_FATAL: schema_too_new supported=1 db=42');
    expect(m).toEqual({ kind: 'schema_too_new', db: 42, supported: 1 });
  });

  test('falls back to unknown when code is unrecognised', () => {
    const m = parseFatalMarker('STDOC_FATAL: lock_held path=/tmp/db.lock');
    expect(m).toEqual({
      kind: 'unknown',
      code: 'lock_held',
      raw: 'STDOC_FATAL: lock_held path=/tmp/db.lock',
    });
  });

  test('schema_too_new with non-numeric values falls back to unknown', () => {
    const m = parseFatalMarker('STDOC_FATAL: schema_too_new db=abc supported=1');
    expect(m).toEqual({
      kind: 'unknown',
      code: 'schema_too_new',
      raw: 'STDOC_FATAL: schema_too_new db=abc supported=1',
    });
  });
});

describe('findFatalMarker', () => {
  test('locates a marker buried inside a multi-line stderr blob', () => {
    const chunk = [
      '[stdoc] starting',
      '[stdoc] opening db at /tmp/.standardoc/index.db',
      'STDOC_FATAL: schema_too_new db=2 supported=1',
      'error: database schema version v2 is newer than supported v1',
    ].join('\n');
    expect(findFatalMarker(chunk)).toEqual({
      kind: 'schema_too_new',
      db: 2,
      supported: 1,
    });
  });

  test('returns null when the chunk holds no marker', () => {
    expect(findFatalMarker('just a regular log\nwith two lines')).toBeNull();
  });

  test('handles CRLF line endings', () => {
    const chunk = 'noise\r\nSTDOC_FATAL: schema_too_new db=3 supported=2\r\nmore noise';
    expect(findFatalMarker(chunk)).toEqual({
      kind: 'schema_too_new',
      db: 3,
      supported: 2,
    });
  });

  test('returns the first marker when multiple are present', () => {
    const chunk =
      'STDOC_FATAL: schema_too_new db=2 supported=1\nSTDOC_FATAL: schema_too_new db=9 supported=1';
    const m = findFatalMarker(chunk);
    expect(m).toEqual({ kind: 'schema_too_new', db: 2, supported: 1 });
  });
});

describe('describeFatalConfig', () => {
  test('schema_too_new reads as actionable text', () => {
    const text = describeFatalConfig({
      kind: 'schema_too_new',
      db: 2,
      supported: 1,
    });
    expect(text).toContain('v2');
    expect(text).toContain('v1');
    expect(text.toLowerCase()).toContain('rebuild');
  });

  test('unknown variant falls back to raw line for diagnostics', () => {
    const text = describeFatalConfig({
      kind: 'unknown',
      code: 'lock_held',
      raw: 'STDOC_FATAL: lock_held path=/tmp/db.lock',
    });
    expect(text).toContain('lock_held');
  });
});
