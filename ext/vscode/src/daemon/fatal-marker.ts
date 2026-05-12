/**
 * Parses the structured fatal-error markers emitted on stderr by the
 * `standardoc` binary. The Rust-side contract lives in
 * `crates/standardoc-cli/src/main.rs::fatal_marker_for` — keep both
 * sides in sync when extending the protocol.
 *
 * Marker shape on the wire:
 *
 * ```text
 * STDOC_FATAL: <code> <key>=<value> ...
 * ```
 *
 * Currently emitted codes:
 *
 * - `schema_too_new db=<n> supported=<n>` — the on-disk index DB schema
 *   is newer than the running binary supports. The binary needs an
 *   upgrade and retries are pointless until the user does so.
 */

export type FatalConfig =
  | { kind: 'schema_too_new'; db: number; supported: number }
  | { kind: 'unknown'; code: string; raw: string };

const PREFIX = 'STDOC_FATAL: ';

/**
 * Returns a parsed `FatalConfig` when `line` carries a structured
 * fatal-error marker, `null` otherwise. Whitespace at line edges is
 * tolerated; key/value pairs are split on `=` and order-independent.
 */
export function parseFatalMarker(line: string): FatalConfig | null {
  const trimmed = line.trim();
  if (!trimmed.startsWith(PREFIX)) return null;

  const rest = trimmed.slice(PREFIX.length).trim();
  const tokens = rest.split(/\s+/);
  const code = tokens[0];
  if (!code) return null;
  const fields = parseFields(tokens.slice(1));

  if (code === 'schema_too_new') {
    const db = parseIntStrict(fields.get('db'));
    const supported = parseIntStrict(fields.get('supported'));
    if (db !== null && supported !== null) {
      return { kind: 'schema_too_new', db, supported };
    }
  }
  return { kind: 'unknown', code, raw: trimmed };
}

/**
 * Scans a chunk of stderr text for the FIRST fatal marker. Useful when
 * the daemon emits the marker as part of a larger startup-error blob.
 * Returns `null` when no line in `chunk` matches.
 */
export function findFatalMarker(chunk: string): FatalConfig | null {
  for (const line of chunk.split(/\r?\n/)) {
    const parsed = parseFatalMarker(line);
    if (parsed !== null) return parsed;
  }
  return null;
}

/** Human-readable rendering for status bar / toast text. */
export function describeFatalConfig(f: FatalConfig): string {
  switch (f.kind) {
    case 'schema_too_new':
      return `Index schema v${f.db} is newer than this standardoc binary supports (v${f.supported}). Rebuild and re-install the binary.`;
    case 'unknown':
      return `Daemon reported a fatal config error: ${f.code} (${f.raw}).`;
  }
}

function parseFields(tokens: ReadonlyArray<string>): Map<string, string> {
  const out = new Map<string, string>();
  for (const t of tokens) {
    const eq = t.indexOf('=');
    if (eq <= 0) continue;
    out.set(t.slice(0, eq), t.slice(eq + 1));
  }
  return out;
}

function parseIntStrict(raw: string | undefined): number | null {
  if (raw === undefined) return null;
  if (!/^\d+$/.test(raw)) return null;
  const n = Number.parseInt(raw, 10);
  return Number.isFinite(n) ? n : null;
}
