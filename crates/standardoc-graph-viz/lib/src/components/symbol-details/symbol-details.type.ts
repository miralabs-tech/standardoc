// UI-shaped data consumed by `<standardoc-symbol-details>`. The host
// derives this from the MCP `get_context` response (plus optional
// `get_body` for the Source tab) and hands it to the component via the
// `symbol` property setter. Decoupling the UI shape from the wire
// shape lets the host coalesce / enrich (e.g. resolve entry-point kind
// from `entry_point`, format kindLabel) without polluting the
// component with MCP knowledge.

import type { EntryPointKind } from '../explorer/explorer.type';

export type SymbolKindLabel = string; // free-form for now; lower-cased decl_kind

export interface SymbolRelationItem {
  readonly fqdn: string;
  readonly label: string;
  readonly kindLabel: SymbolKindLabel;
}

export type SymbolRelationKind =
  | 'usedBy'
  | 'usesTypes'
  | 'calls'
  | 'imports'
  | 'importedBy'
  | 'testedBy'
  | 'implements'
  | 'extends'
  | 'definedHere';

export interface SymbolRelationBucket {
  readonly kind: SymbolRelationKind;
  readonly items: ReadonlyArray<SymbolRelationItem>;
  /** Total count including items beyond what's surfaced in `items`. */
  readonly total: number;
}

export interface SymbolSubItem {
  readonly fqdn: string;
  readonly name: string;
  readonly kindLabel: SymbolKindLabel;
  readonly file: string | null;
  readonly startLine: number | null;
  /**
   * Pre-formatted anatomy hint shown next to the name in Fields/Methods
   * tabs. Field-style `": T"` when params are absent, function-style
   * `"(p: T, q: U) → R"` otherwise. `null` for entries whose extractor
   * did not capture a signature (re-exports, builtins, externals).
   */
  readonly signature: string | null;
  /** `public` / `private` / `crate` / `protected`. `null` when the
   *  extractor didn't classify (re-exports, builtins). */
  readonly visibility: string | null;
  /** True when the symbol's `flags` carries `"async"` (TS Promise
   *  return / `async` keyword, Rust `Future` return). Surfaced as a
   *  chip in the Fields/Methods rows so the call shape reads at a
   *  glance without opening Source. */
  readonly isAsync: boolean;
  /** Daemon-computed test-symbol verdict, carried from the list_symbols
   *  projection that sources the sub-items so the "Hide tests" toggle
   *  reads it instead of re-deriving `looksLikeTest`. */
  readonly is_test: boolean;
  /** Standalone type display for FIELD-shaped sub-items (no params,
   *  return = the field's type). Methods leave this `null` — their
   *  return type is already visible in `signature` and a separate
   *  chip would be redundant. Source: `sig.returns.display` filtered
   *  by `params.length === 0`. */
  readonly type: string | null;
}

export interface SymbolDetail {
  readonly fqdn: string;
  readonly name: string;
  readonly kindLabel: SymbolKindLabel;
  readonly visibility: string | null;
  readonly file: string;
  readonly startLine: number;
  readonly documentation: string | null;
  readonly entryPointKind: EntryPointKind | null;
  readonly fields: ReadonlyArray<SymbolSubItem>;
  readonly methods: ReadonlyArray<SymbolSubItem>;
  readonly relations: ReadonlyArray<SymbolRelationBucket>;
}

export type SymbolDetailsTab = 'overview' | 'fields' | 'methods' | 'source';

export type SymbolDetailsAction =
  | 'open-in-editor'
  | 'copy-fqdn'
  | 'add-to-compare';

export interface SymbolDetailsActionDetail {
  readonly action: SymbolDetailsAction;
  readonly fqdn: string;
}

export interface SymbolDetailsRelationClickDetail {
  readonly fqdn: string;
  readonly relationKind: SymbolRelationKind;
}

export interface SymbolDetailsTabChangeDetail {
  readonly tab: SymbolDetailsTab;
}
