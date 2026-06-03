import * as fs from 'node:fs';
import * as path from 'node:path';

const SXD_CONFIG_FILENAME = 'standardoc.sxd';

/**
 * Best-effort, regex-based extraction of `<kind> { port N }` from the
 * workspace's `standardoc.sxd`. Returns `null` when the file is
 * absent, unreadable, or the block / field aren't declared — the
 * caller falls through to its own default in those cases.
 *
 * Intentionally lightweight: we don't want to ship the full
 * standarx-dsl parser into the VSCode extension just to read a single
 * integer field. If users push the schema's expressivity (computed
 * ports, interpolation), we'll graduate this helper to shelling out
 * to a `standardoc resolve-config` CLI subcommand that does proper
 * parsing.
 */
export function readSxdPort(workspaceRoot: string, kind: 'mcp' | 'viz' | 'proxy'): number | null {
  const sxdPath = path.join(workspaceRoot, SXD_CONFIG_FILENAME);
  let source: string;
  try {
    source = fs.readFileSync(sxdPath, 'utf-8');
  } catch {
    return null;
  }
  return extractPort(source, kind);
}

/**
 * Pure helper exposed for testing. Returns null when no `<kind>` block
 * exists OR when the block exists but doesn't declare `port`.
 * Tolerates extra whitespace, multi-line blocks, and other fields
 * (e.g. `proxy { bind "..." port N }`).
 */
export function extractPort(source: string, kind: 'mcp' | 'viz' | 'proxy'): number | null {
  // [\s\S] instead of . to cross newlines without dotall flag; non-greedy
  // closing brace match so nested-ish content (we don't really support
  // nested blocks here, but be defensive) doesn't bleed into the next
  // top-level block.
  const blockRe = new RegExp(`(?:^|\\n)\\s*${kind}\\s*\\{([\\s\\S]*?)\\}`, 'm');
  const m = blockRe.exec(source);
  if (m === null) return null;
  const body = m[1] ?? '';
  const portRe = /\bport\s+(\d+)\b/;
  const pm = portRe.exec(body);
  if (pm === null) return null;
  const value = pm[1];
  if (value === undefined) return null;
  const n = Number.parseInt(value, 10);
  if (!Number.isFinite(n) || n < 1 || n > 65535) return null;
  return n;
}
