/**
 * vscode-free hover target detection for `.sxd` documents. Lives in
 * its own module so unit tests can exercise it without pulling in
 * the `vscode` runtime — the wrapper in `hover.ts` only adapts
 * vscode types to these structural ones.
 *
 * The detector classifies what the cursor sits on into one of two
 * preview-driving buckets ; everything else is LSP territory and
 * returns `null` here.
 */

export type HoverTarget =
  | { readonly kind: 'pattern'; readonly value: string }
  | { readonly kind: 'path'; readonly value: string };

/** Structural subset of `vscode.TextDocument`. */
export interface LineReader {
  readonly lineCount: number;
  lineAt(line: number): { readonly text: string };
}

/** Structural subset of `vscode.Position`. */
export interface CursorPos {
  readonly line: number;
  readonly character: number;
}

const PATTERNS_OPEN_RE = /^\s*patterns\s+```/;
const PATTERNS_CLOSE_RE = /^\s*```/;

/**
 * Classify what the cursor is hovering. Two cases drive a filesystem
 * preview (LSP handles static doc separately):
 *
 *   - inside a `patterns ```...``` ` block → 'pattern' (line trimmed)
 *   - on a string-literal value of `path "..."` or `paths [ "..." ... ]`
 *     → 'path' (string contents)
 *
 * Returns `null` for everything else — block kinds, field keys,
 * `version`, comments — those are LSP-side.
 */
export function detectHoverTarget(document: LineReader, position: CursorPos): HoverTarget | null {
  if (isInsideIgnorePatternsBlock(document, position.line)) {
    const lineText = document.lineAt(position.line).text;
    const pattern = lineText.trim();
    if (pattern.length === 0 || pattern.startsWith('#') || PATTERNS_CLOSE_RE.test(lineText)) {
      return null;
    }
    return { kind: 'pattern', value: pattern };
  }
  const pathValue = pathValueUnderCursor(document, position);
  if (pathValue !== null) {
    return { kind: 'path', value: pathValue };
  }
  return null;
}

// Heuristic: walk back from `lineNumber` looking for `patterns ```` ; bail
// if we hit a `}` (block close) or another `patterns ```` first. Symmetric
// forward scan for the closing fence. The hover only fires inside that
// range — anything outside (version, project blocks, ignore { ... } braces,
// blank lines, comments above the block) is suppressed.
export function isInsideIgnorePatternsBlock(document: LineReader, lineNumber: number): boolean {
  let opened = false;
  for (let i = lineNumber - 1; i >= 0; i--) {
    const text = document.lineAt(i).text;
    if (PATTERNS_OPEN_RE.test(text)) {
      opened = true;
      break;
    }
    if (/^\s*\}/.test(text) || /^\s*[a-z]+\s*\{/.test(text)) {
      return false;
    }
  }
  if (!opened) return false;
  for (let i = lineNumber + 1; i < document.lineCount; i++) {
    const text = document.lineAt(i).text;
    if (PATTERNS_CLOSE_RE.test(text)) return true;
    if (PATTERNS_OPEN_RE.test(text)) return false;
  }
  return false;
}

/**
 * Return the literal contents of the string under the cursor IF that
 * string is the value of a `path "..."` assignment or an item in a
 * `paths [ ... ]` array. Returns `null` otherwise.
 *
 * Regex-based — robust enough for V1 single-line and multi-line shapes.
 * The full AST is the LSP server's job ; here we only need to dispatch
 * the sxd-preview subprocess call.
 */
export function pathValueUnderCursor(document: LineReader, position: CursorPos): string | null {
  const line = document.lineAt(position.line).text;
  const str = stringContainingColumn(line, position.character);
  if (!str) return null;
  const before = line.slice(0, str.start);

  // Same-line `path "..."` assignment.
  if (/\bpath\s+$/.test(before)) {
    return str.value;
  }
  // Same-line `paths [ "..." ... ]` — bracket already opened on this line.
  if (/\bpaths\s*\[[^\]]*$/.test(before)) {
    return str.value;
  }
  // Multi-line `paths [\n  "..."\n  "..."\n]` — walk back until we find
  // `paths [` (still inside the array). Bail on `]`, `{`, or any line
  // that is not whitespace / comment / bare string.
  for (let i = position.line - 1; i >= 0; i--) {
    const t = document.lineAt(i).text;
    if (/\bpaths\s*\[/.test(t)) {
      return str.value;
    }
    if (/[\]{}]/.test(t)) return null;
    const stripped = t.trim();
    if (stripped.length === 0) continue;
    if (stripped.startsWith('#')) continue;
    // A pure string literal on its own line is OK — that's another array item.
    if (/^"[^"]*"$/.test(stripped)) continue;
    return null;
  }
  return null;
}

/**
 * If `column` falls inside a `"..."` double-quoted string on `line`,
 * return its byte-start and unquoted contents ; otherwise `null`.
 * Only handles plain strings (no escapes, no interpolation) — matches
 * the schema's `version`/`path`/`label`/`bind` value shape.
 */
export function stringContainingColumn(
  line: string,
  column: number,
): { readonly start: number; readonly value: string } | null {
  const re = /"([^"\\]*)"/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(line)) !== null) {
    const start = match.index;
    const end = match.index + match[0].length;
    if (column > start && column < end) {
      return { start, value: match[1] ?? '' };
    }
  }
  return null;
}
