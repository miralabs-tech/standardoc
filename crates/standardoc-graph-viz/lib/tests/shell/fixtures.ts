// Deterministic MCP tool-response fixtures for the shell harness.
// Shapes mirror exactly what the standardoc daemon returns — the
// McpBrowse wrapper JSON.parses the first text block of each tool
// result, so these must match the wire types in
// `src/mcp-client/types.ts`:
//   - list_symbols  → { items: RawSymbol[], next_cursor } where each row
//                       is also stamped with project_id + is_test (the
//                       daemon's list_symbols projection)
//   - RawSymbol     → { fqdn, name, kind, module, visibility,
//                       language_kind, location: {...}, entry_point? }
//   - get_context   → { context: { symbol, enrichment_description,
//                       document_description }, callers, callees,
//                       imports, imported_by }
//
// Small but structurally complete: enough projects + symbols + edges +
// one rich get_context so mountShell builds the full tree + entry
// points + an Overview payload without a live daemon.

export interface ToolResult {
  readonly content: ReadonlyArray<{ type: 'text'; text: string }>;
}

function text(value: unknown): ToolResult {
  return { content: [{ type: 'text', text: JSON.stringify(value) }] };
}

interface FixtureSymbol {
  fqdn: string;
  name: string;
  kind: string;
  module: string;
  visibility: string;
  language_kind: string;
  location: {
    file: string;
    start_line: number;
    end_line: number;
    start_col: number;
    end_col: number;
  };
  entry_point?: string;
}

function loc(file: string, line: number): FixtureSymbol['location'] {
  return { file, start_line: line, end_line: line + 4, start_col: 0, end_col: 0 };
}

// --- RawSymbol rows (list_symbols / find_* wire shape) ---
const SYMBOLS: FixtureSymbol[] = [
  {
    fqdn: 'demo::main',
    name: 'main',
    kind: 'function',
    module: 'demo',
    visibility: 'public',
    language_kind: 'function',
    location: loc('src/main.rs', 1),
    entry_point: 'binary_main',
  },
  {
    fqdn: 'demo::engine::Engine',
    name: 'Engine',
    kind: 'struct',
    module: 'demo::engine',
    visibility: 'public',
    language_kind: 'struct',
    location: loc('src/engine.rs', 10),
    entry_point: 'public_api',
  },
  {
    fqdn: 'demo::engine::Engine::run',
    name: 'run',
    kind: 'method',
    module: 'demo::engine',
    visibility: 'public',
    language_kind: 'method',
    location: loc('src/engine.rs', 22),
  },
  {
    fqdn: 'demo::util::helper',
    name: 'helper',
    kind: 'function',
    module: 'demo::util',
    visibility: 'crate',
    language_kind: 'function',
    location: loc('src/util.rs', 3),
  },
  {
    fqdn: 'demo::tests::it_works',
    name: 'it_works',
    kind: 'function',
    module: 'demo::tests',
    visibility: 'private',
    language_kind: 'function',
    location: loc('src/tests.rs', 5),
  },
];

const PROJECTS = {
  projects: [
    { project_id: 1, label: 'Demo', kind: { kind: 'binary' }, rel_path: '', root_path: '/demo' },
  ],
};

// fetch_graph wire shape: BrowseSymbol (flat file/start_line) + edges.
const GRAPH = {
  symbols: SYMBOLS.map(s => ({
    fqdn: s.fqdn,
    name: s.name,
    kind: s.kind,
    visibility: s.visibility,
    module: s.module,
    language_kind: s.language_kind,
    is_external: false,
    is_test: s.fqdn.includes('::tests::'),
    file: s.location.file,
    start_line: s.location.start_line,
    project_id: 1,
    entry_point: s.entry_point ?? null,
  })),
  edges: [
    { from: 'demo::main', to: 'demo::engine::Engine::run', kind: 'CALLS', outbound: true },
    { from: 'demo::engine::Engine::run', to: 'demo::util::helper', kind: 'CALLS', outbound: true },
  ],
};

function contextFor(fqdn: string): unknown {
  const sym = SYMBOLS.find(s => s.fqdn === fqdn) ?? SYMBOLS[0]!;
  return {
    context: {
      symbol: sym,
      enrichment_description: null,
      document_description: `Fixture doc for ${sym.name}.`,
    },
    callers: [],
    callees: [],
    imports: [],
    imported_by: [],
  };
}

/**
 * Resolve a tool call to its fixture result. `list_symbols` returns the
 * whole set in one page (no cursor) — enough for the harness; the real
 * cursor pagination is exercised against the daemon.
 */
export function resolveTool(name: string, args: Record<string, unknown>): ToolResult {
  switch (name) {
    case 'list_projects':
      return text(PROJECTS);
    case 'list_symbols':
      // Mirror the daemon's list_symbols projection: each RawSymbol row
      // carries the JOIN-resolved project_id + the is_test verdict.
      return text({
        items: SYMBOLS.map(s => ({
          ...s,
          project_id: 1,
          is_test: s.fqdn.includes('::tests::'),
        })),
        next_cursor: null,
      });
    case 'fetch_graph':
      return text(GRAPH);
    case 'get_context':
      return text(contextFor(String(args.fqdn ?? '')));
    case 'get_body':
      return text({
        fqdn: String(args.fqdn ?? ''),
        file: 'src/main.rs',
        start_line: 1,
        end_line: 3,
        body: '// fixture body',
        truncated: false,
        dedented_prefix_len: 0,
      });
    case 'find_symbol': {
      const q = String(args.query ?? '').toLowerCase();
      const results = SYMBOLS.filter(
        s => s.name.toLowerCase().includes(q) || s.fqdn.toLowerCase().includes(q),
      );
      return text({ results, did_you_mean: [] });
    }
    case 'find_symbols_by_pattern': {
      const raw = String(args.pattern ?? '').replace(/\*/g, '').toLowerCase();
      const results = SYMBOLS.filter(s => s.fqdn.toLowerCase().includes(raw));
      return text(results);
    }
    case 'current_revision':
      return text({ revision: 1, indexing: { ready: true } });
    default:
      return text({});
  }
}

export const FIXTURE_ENTRY_POINT_COUNT = SYMBOLS.filter(
  s => typeof s.entry_point === 'string',
).length;
