export type SymbolKindJson = 'function' | 'type' | 'value' | 'module' | 'macro';

export type EdgeKindJson =
  | 'CALLS'
  | 'IMPORTS'
  | 'EXTENDS'
  | 'IMPLEMENTS'
  | 'REFERENCES'
  | 'DEFINES'
  | 'USES_TYPE'
  | 'EXPOSES_API';

export type VisibilityJson = 'public' | 'private' | 'crate' | 'protected';

export interface SymbolLocationJson {
  file: string;
  start_line: number;
  end_line: number;
  start_col: number;
  end_col: number;
}

export interface RawSymbolJson {
  name: string;
  fqdn: string;
  kind: SymbolKindJson;
  language_kind: string;
  module: string | null;
  visibility: VisibilityJson;
  location: SymbolLocationJson;
  signature?: unknown;
  body_hash: string | null;
  attributes?: unknown[];
}

export type ResolvedOrUnresolvedJson =
  | { Resolved: { fqdn: string } }
  | { Unresolved: { name: string } }
  | { UnresolvedBridge: { bridge: string; name: string } };

export interface NeighborSymbolJson {
  edge_kind: EdgeKindJson;
  target: ResolvedOrUnresolvedJson;
  resolved_symbol: RawSymbolJson | null;
}

export interface SymbolContextJson {
  symbol: RawSymbolJson;
  enrichment_description: string | null;
  document_description: string | null;
}

export interface SymbolContextWithNeighborsJson {
  context: SymbolContextJson;
  callers: NeighborSymbolJson[];
  callees: NeighborSymbolJson[];
  imports: NeighborSymbolJson[];
  imported_by: NeighborSymbolJson[];
}
