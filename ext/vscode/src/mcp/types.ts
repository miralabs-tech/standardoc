export type SymbolKindJson = 'callable' | 'type' | 'value' | 'module' | 'macro';

export type EdgeKindJson =
  | 'CALLS'
  | 'IMPORTS'
  | 'EXTENDS'
  | 'IMPLEMENTS'
  | 'REFERENCES'
  | 'USES_TYPE';

export type VisibilityJson = 'public' | 'private' | 'crate' | 'protected';

export type LanguageJson =
  | 'rust'
  | 'typescript'
  | 'javascript'
  | 'lua'
  | 'vue'
  | 'svelte'
  | 'c';

export type EntryPointKindJson = 'binary_main' | 'public_api' | 'ffi_export';

export type DeclKindJson =
  | 'module'
  | 'namespace'
  | 'crate'
  | 'struct'
  | 'enum'
  | 'union'
  | 'class'
  | 'interface'
  | 'type_alias'
  | 'function'
  | 'method'
  | 'constructor'
  | 'getter'
  | 'setter'
  | 'const'
  | 'static'
  | 'var'
  | 'field'
  | 'enum_variant'
  | 'declarative_macro'
  | 'proc_macro'
  | 'decorator'
  | { custom: { lang: LanguageJson; tag: string } };

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
  is_external?: boolean;
  decl_kind?: DeclKindJson;
  implements_trait?: string;
  receiver_type?: string;
  entry_point?: EntryPointKindJson;
}

export type ResolvedOrUnresolvedJson =
  | { kind: 'resolved'; fqdn: string }
  | { kind: 'unresolved'; name: string }
  | { kind: 'unresolved_bridge'; bridge: string; name: string };

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

export interface ListSymbolsPageJson {
  items: RawSymbolJson[];
  next_cursor: string | null;
}

export interface CurrentRevisionJson {
  revision: number;
  watcher: { active: boolean };
  indexing: { ready: boolean };
}

export type CheckStaleEntryJson =
  | { fqdn: string; status: 'fresh' }
  | { fqdn: string; status: 'stale'; last_modified_revision: number }
  | { fqdn: string; status: 'missing' };

export interface CheckStaleJson {
  results: CheckStaleEntryJson[];
}
