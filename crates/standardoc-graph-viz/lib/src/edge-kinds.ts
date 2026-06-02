/**
 * Canonical cross-edge kind palette — the single TS source of truth for
 * edge-kind colour / label / dash, shared by the Overview mini-legend
 * chips and the standalone `<standardoc-legend>`. Must stay in sync with
 * `cross_edge_style` in `src/overview/mod.rs` (the wasm renderer that
 * actually strokes the edges). `?? FALLBACK` at call sites maps unknown
 * kinds to a neutral gray.
 */
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
