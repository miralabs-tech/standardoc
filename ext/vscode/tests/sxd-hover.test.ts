import { describe, expect, test } from 'bun:test';
import { renderPreviewMarkdown } from '../src/stdignore/hover-render';
import {
  detectHoverTarget,
  pathValueUnderCursor,
  stringContainingColumn,
  type LineReader,
} from '../src/sxd/hover-detect';

function docOf(...lines: string[]): LineReader {
  return {
    lineCount: lines.length,
    lineAt(n: number) {
      return { text: lines[n] ?? '' };
    },
  };
}

describe('renderPreviewMarkdown — Path label', () => {
  test('uses the provided label in the empty-state message', () => {
    const text = renderPreviewMarkdown(
      {
        pattern: 'crates',
        matches: [],
        total_count: 0,
        truncated: false,
        walk_truncated: false,
      },
      'Path',
    );
    expect(text).toContain('**Path');
    expect(text).toContain('no matches in workspace');
  });

  test('uses the provided label in the multi-match header', () => {
    const text = renderPreviewMarkdown(
      {
        pattern: 'crates',
        matches: ['crates/a/Cargo.toml', 'crates/b/Cargo.toml'],
        total_count: 2,
        truncated: false,
        walk_truncated: false,
      },
      'Path',
    );
    expect(text).toContain('**Path');
    expect(text).toContain('2 matches');
  });
});

describe('stringContainingColumn', () => {
  test('returns the inner contents when cursor sits inside the literal', () => {
    const got = stringContainingColumn('  path "crates"', 9); // inside "crates"
    expect(got).not.toBeNull();
    expect(got?.value).toBe('crates');
  });

  test('returns null outside any string', () => {
    expect(stringContainingColumn('  path "crates"', 2)).toBeNull();
  });

  test('picks the right one with multiple strings on the line', () => {
    const got = stringContainingColumn('paths ["a" "b"]', 12); // inside "b"
    expect(got?.value).toBe('b');
  });

  test('quote boundaries are NOT inside the string (no spurious hover on the quote char)', () => {
    const line = 'path "x"';
    const startQuote = line.indexOf('"');
    expect(stringContainingColumn(line, startQuote)).toBeNull();
  });
});

describe('pathValueUnderCursor', () => {
  test('detects single-line `path "..."`', () => {
    const doc = docOf('project "x" {', '  path "crates"', '}');
    const col = doc.lineAt(1).text.indexOf('crates') + 2;
    expect(pathValueUnderCursor(doc, { line: 1, character: col })).toBe('crates');
  });

  test('detects same-line `paths ["a" "b"]` item', () => {
    const doc = docOf('project "x" {', '  paths ["crates" "ext/vscode"]', '}');
    const col = doc.lineAt(1).text.indexOf('ext/vscode') + 3;
    expect(pathValueUnderCursor(doc, { line: 1, character: col })).toBe('ext/vscode');
  });

  test('detects multi-line `paths [` block item', () => {
    const doc = docOf(
      'project "x" {',
      '  paths [',
      '    "crates"',
      '    "ext/vscode"',
      '  ]',
      '}',
    );
    const lineText = doc.lineAt(3).text;
    const col = lineText.indexOf('ext/vscode') + 2;
    expect(pathValueUnderCursor(doc, { line: 3, character: col })).toBe('ext/vscode');
  });

  test('does NOT fire on a `label "..."` value', () => {
    const doc = docOf('project "x" {', '  label "X"', '  path "foo"', '}');
    const col = doc.lineAt(1).text.indexOf('X') + 1;
    expect(pathValueUnderCursor(doc, { line: 1, character: col })).toBeNull();
  });

  test('does NOT fire on the project slug', () => {
    const doc = docOf('project "x" {', '}');
    const col = doc.lineAt(0).text.indexOf('"x"') + 1;
    expect(pathValueUnderCursor(doc, { line: 0, character: col })).toBeNull();
  });

  test('bails out of a `paths [` block when a `]` is seen earlier', () => {
    // closing bracket on a previous line — cursor is no longer inside the array
    const doc = docOf(
      'project "a" { paths ["x"] }',
      'project "b" {',
      '  label "B"',
      '  path "y"',
      '}',
    );
    // Hover on "B" — should NOT think it's inside paths.
    const col = doc.lineAt(2).text.indexOf('B') + 1;
    expect(pathValueUnderCursor(doc, { line: 2, character: col })).toBeNull();
  });
});

describe('detectHoverTarget', () => {
  test('returns "pattern" inside an `ignore { patterns ```...``` }` block', () => {
    const doc = docOf(
      'ignore {',
      '  patterns ```',
      'target/',
      'node_modules/',
      '```',
      '}',
    );
    const got = detectHoverTarget(doc, { line: 3, character: 4 });
    expect(got).toEqual({ kind: 'pattern', value: 'node_modules/' });
  });

  test('returns "path" on a `path "..."` value', () => {
    const doc = docOf('project "x" {', '  path "crates"', '}');
    const col = doc.lineAt(1).text.indexOf('crates') + 2;
    const got = detectHoverTarget(doc, { line: 1, character: col });
    expect(got).toEqual({ kind: 'path', value: 'crates' });
  });

  test('returns null on the `path` field key itself (LSP territory)', () => {
    const doc = docOf('project "x" {', '  path "crates"', '}');
    // Cursor on the `path` ident, not inside the string literal.
    expect(detectHoverTarget(doc, { line: 1, character: 3 })).toBeNull();
  });

  test('returns null on a blank line inside `patterns` (no pattern to preview)', () => {
    const doc = docOf('ignore {', '  patterns ```', '', '```', '}');
    expect(detectHoverTarget(doc, { line: 2, character: 0 })).toBeNull();
  });

  test('returns null on a `#` comment line inside `patterns`', () => {
    const doc = docOf('ignore {', '  patterns ```', '# comment', '```', '}');
    expect(detectHoverTarget(doc, { line: 2, character: 3 })).toBeNull();
  });
});
