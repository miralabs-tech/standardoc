// Edge-kind palette lives in the shared leaf `src/edge-kinds.ts` so the
// Overview chips and the standalone legend can't drift apart. Re-exported
// here for the Overview element's existing import path.
export { CROSS_EDGE_KINDS, type CrossEdgeKindSpec } from '../../edge-kinds';

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
