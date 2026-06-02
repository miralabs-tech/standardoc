import type {
  BrowseSymbol,
  GetContextResponse,
} from '../mcp-client';

import type { GraphEdge } from './types';
import { displayKindLabel } from './symbols';

export function buildFocusPayload(
  fqdn: string,
  ctx: GetContextResponse | null,
  neighborhoodEdges: ReadonlyArray<GraphEdge>,
  neighborhoodSymbols: ReadonlyArray<BrowseSymbol>,
  centerFieldCount: number,
  centerMethodCount: number,
): string {
  const centerSym = ctx?.context.symbol;
  const center = centerSym !== undefined ? {
    fqdn: centerSym.fqdn,
    name: centerSym.name,
    kind: displayKindLabel(centerSym),
    depth: 0,
    field_count: centerFieldCount,
    method_count: centerMethodCount,
  } : null;
  // BFS depth per neighbour: shortest hop count from the focal symbol.
  // fetch_graph doesn't surface per-node depth in the wire payload,
  // so we reconstruct it client-side by walking the edges. Phase 3c
  // canvas only renders the data we send, no further filtering.
  const depthByFqdn = computeDepthFromFocal(fqdn, neighborhoodEdges);
  const neighbors = neighborhoodSymbols
    .filter(s => s.fqdn !== fqdn)
    .map(s => ({
      fqdn: s.fqdn,
      name: s.name,
      kind: displayKindLabel(s),
      depth: depthByFqdn.get(s.fqdn) ?? 1,
      file: s.file && s.file.length > 0 ? s.file : null,
      start_line: typeof s.start_line === 'number' ? s.start_line : null,
    }));
  const focalEdges = neighborhoodEdges.map(e => ({
    from: e.from,
    to: e.to,
    kind: e.kind,
    depth: Math.max(depthByFqdn.get(e.from) ?? 0, depthByFqdn.get(e.to) ?? 0),
  }));
  return JSON.stringify({ center, neighbors, edges: focalEdges });
}

function computeDepthFromFocal(
  focal: string,
  edges: ReadonlyArray<{ from: string; to: string }>,
): Map<string, number> {
  const adj = new Map<string, Set<string>>();
  const pushAdj = (a: string, b: string): void => {
    let set = adj.get(a);
    if (set === undefined) { set = new Set(); adj.set(a, set); }
    set.add(b);
  };
  for (const e of edges) {
    pushAdj(e.from, e.to);
    pushAdj(e.to, e.from);
  }
  const depths = new Map<string, number>();
  depths.set(focal, 0);
  const queue: string[] = [focal];
  while (queue.length > 0) {
    const cur = queue.shift()!;
    const here = depths.get(cur) ?? 0;
    for (const next of adj.get(cur) ?? []) {
      if (depths.has(next)) continue;
      depths.set(next, here + 1);
      queue.push(next);
    }
  }
  return depths;
}

export function buildEmptyFocusPayload(): string {
  return JSON.stringify({ center: null, neighbors: [], edges: [] });
}
