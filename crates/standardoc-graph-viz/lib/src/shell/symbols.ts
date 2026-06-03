import type { McpBrowse } from '../mcp-client';
import type {
  BrowseSymbol,
  RawSymbol,
} from '../mcp-client';
import type {
  EntryPointKind,
  ExplorerEntryPoint,
  ExplorerNodeKind,
  SymbolSearchResult,
} from '../index';

import { shortFqdn } from '../text';

// Daemon caps `limit` at u8 (255) — we used to send 500 and the
// request died silently inside the McpBrowse catch, returning ZERO
// symbols and leaving the tree empty + 'ready (0 entry points)'.
const EP_PAGE_SIZE = 200;
const EP_MAX_PAGES = 2500; // 500k symbols ceiling — safety net only, ext:false should cap us far below this

/**
 * Single paginated walk of the workspace symbol index. Returns both
 * the full RawSymbol set (drives the IDE-style file tree) and the
 * filtered entry-points subset (drives the Explorer's Entry Points
 * section). Merging the two consumers into one pass avoids paying
 * the list_symbols round-trip cost twice.
 */
export async function collectWorkspaceSymbols(
  mcp: McpBrowse,
  report: (status: string) => void,
): Promise<{ all: RawSymbol[]; entryPoints: ExplorerEntryPoint[] }> {
  const all: RawSymbol[] = [];
  const entryPoints: ExplorerEntryPoint[] = [];
  let cursor: string | undefined;
  let page = 0;
  while (page < EP_MAX_PAGES) {
    page++;
    // externals: false drops dependency crate symbols server-side.
    // Builtins ('<builtin>::*') aren't covered by that flag so we
    // also filter them client-side below — they otherwise drown the
    // workspace symbols under hundreds of pages on cold start.
    const res = await mcp.listSymbols({ limit: EP_PAGE_SIZE, externals: false, cursor }).catch(e => {
      // Log instead of silently breaking so a daemon-side regression
      // (param shape changed, limit cap tightened, etc.) surfaces in
      // the console rather than showing up as a magically empty tree.
      // eslint-disable-next-line no-console
      console.warn('[shell] listSymbols failed:', e);
      return null;
    });
    if (res === null) break;
    for (const s of res.items) {
      if (s.language_kind === 'builtin' || s.location.file.startsWith('<builtin>')) {
        continue;
      }
      all.push(s);
      if (typeof s.entry_point === 'string' && s.entry_point.length > 0) {
        entryPoints.push({
          fqdn: s.fqdn,
          label: shortFqdn(s.fqdn),
          kind: s.entry_point as EntryPointKind,
        });
      }
    }
    report(`workspace symbols… (page ${page}, ${all.length} kept, ${entryPoints.length} entry points)`);
    if (res.next_cursor === undefined || res.next_cursor === null || res.next_cursor.length === 0) break;
    cursor = res.next_cursor;
  }
  return { all, entryPoints };
}

/**
 * Convert a list_symbols RawSymbol into the flatter BrowseSymbol shape
 * the tree builder consumes. `project_id` and `is_test` ride straight
 * off the daemon's list_symbols projection — the shell no longer infers
 * the owning project from a path-prefix match (that duplicated the
 * daemon's `reconcile_files_project_id` JOIN) nor re-derives the test
 * verdict.
 */
export function rawToBrowseSymbol(s: RawSymbol): BrowseSymbol {
  return {
    fqdn: s.fqdn,
    name: s.name,
    kind: s.kind,
    visibility: s.visibility,
    module: s.module,
    language_kind: s.language_kind,
    is_external: false,
    is_test: s.is_test ?? false,
    file: s.location.file,
    start_line: s.location.start_line,
    project_id: s.project_id ?? null,
    entry_point: s.entry_point ?? null,
  };
}

export function stripProjectPrefix(filePath: string, projectRelPath: string): string | null {
  const norm = (p: string) => p.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  const file = norm(filePath);
  const prefix = norm(projectRelPath);
  if (prefix.length === 0) return file;
  if (file === prefix) return '';
  if (file.startsWith(`${prefix}/`)) return file.slice(prefix.length + 1);
  return null;
}

export function mapBrowseSymbolKind(s: BrowseSymbol): ExplorerNodeKind {
  const lk = s.language_kind;
  if (lk === 'struct') return 'struct';
  if (lk === 'enum') return 'enum';
  if (lk === 'fn' || lk === 'function' || lk === 'method') return 'function';
  if (lk === 'trait' || lk === 'interface') return 'trait';
  if (lk === 'const' || lk === 'static') return 'value';
  if (lk === 'macro' || lk === 'macro_rules') return 'macro';
  switch (s.kind) {
    case 'type': return 'struct';
    case 'callable': return 'function';
    case 'value': return 'value';
    case 'macro': return 'macro';
    default: return 'unknown';
  }
}

export function formatSignature(sig: RawSymbol['signature']): string | null {
  if (!sig) return null;
  const params = sig.params ?? [];
  const ret = sig.returns?.display ?? null;
  if (params.length === 0) {
    return ret ? `: ${ret}` : null;
  }
  const paramStr = params
    .map((p) => (p.ty?.display ? `${p.name}: ${p.ty.display}` : p.name))
    .join(', ');
  return ret ? `(${paramStr}) → ${ret}` : `(${paramStr})`;
}

/**
 * Display-kind ladder shared by every shell mapper: prefer the
 * extractor's `decl_kind`, fall back to `language_kind`, then the
 * coarse `kind`. One place so the panels don't drift apart.
 */
export function displayKindLabel(
  s: { decl_kind?: string | null; language_kind?: string | null; kind: string },
): string {
  return s.decl_kind ?? s.language_kind ?? s.kind;
}

export function toSymbolSearchResult(s: RawSymbol): SymbolSearchResult {
  return {
    fqdn: s.fqdn,
    name: s.name,
    kindLabel: displayKindLabel(s),
    kind: s.kind,
    file: s.location.file,
    startLine: s.location.start_line,
  };
}
