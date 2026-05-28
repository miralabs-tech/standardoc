/// Cross-edge kind palette — must stay in sync with `cross_edge_style`
/// in `src/overview/mod.rs` and the global legend in `lib/components/legend`.
/// Used to render the mini-legend chips and to map unknown kinds to a
/// neutral gray (the `?? FALLBACK` at call sites).
export interface CrossEdgeKindSpec {
  readonly kind: string;
  readonly label: string;
  readonly color: string;
  readonly dashed: boolean;
}

export const CROSS_EDGE_KINDS: ReadonlyArray<CrossEdgeKindSpec> = [
  { kind: 'CALLS', label: 'Calls', color: '#3794ff', dashed: false },
  { kind: 'IMPORTS', label: 'Imports', color: '#b180d7', dashed: false },
  { kind: 'USES_TYPE', label: 'Uses type', color: '#cca700', dashed: false },
  { kind: 'IMPLEMENTS', label: 'Implements', color: '#f48771', dashed: true },
  { kind: 'EXTENDS', label: 'Extends', color: '#f48771', dashed: true },
  { kind: 'REFERENCES', label: 'References', color: '#9d9d9d', dashed: false },
];

export const DEFAULT_SHOW_CROSS_EDGES = false;
const CROSS_EDGES_STORAGE_KEY = 'sd-overview-cross-edges';

export function readPersistedCrossEdges(): boolean {
  try {
    const raw = globalThis.localStorage?.getItem(CROSS_EDGES_STORAGE_KEY);
    if (raw === null || raw === undefined) return DEFAULT_SHOW_CROSS_EDGES;
    return raw === '1';
  } catch {
    return DEFAULT_SHOW_CROSS_EDGES;
  }
}

export function writePersistedCrossEdges(value: boolean): void {
  try {
    globalThis.localStorage?.setItem(CROSS_EDGES_STORAGE_KEY, value ? '1' : '0');
  } catch {
    /* localStorage unavailable (private mode, sandbox) — silently skip. */
  }
}
