// Wire shapes returned by the standardoc daemon's MCP `fetch_graph`
// endpoint. Kept identical to what the daemon emits so consumers can
// feed the payload straight into the wasm engine or DOM panels with
// zero reshape. Adding fields here is safe; removing or renaming any
// of them requires a matching update on the Rust serializer side.

export interface BrowseSymbol {
	readonly fqdn: string;
	readonly name: string;
	readonly kind: string;
	readonly visibility: string;
	readonly module: string | null;
	readonly language_kind: string;
	readonly language: string;
	readonly is_external: boolean;
	readonly file: string;
	readonly start_line: number;
	readonly project_id?: number | null;
}

export interface BrowseEdge {
	readonly from: string;
	readonly to: string;
	readonly kind: string;
	readonly outbound: boolean;
}

export interface BrowseProject {
	readonly project_id: number;
	readonly label: string;
	readonly kind: string;
	readonly rel_path: string;
}

export interface FetchGraphResponse {
	readonly symbols: ReadonlyArray<BrowseSymbol>;
	readonly edges: ReadonlyArray<BrowseEdge>;
	readonly projects?: ReadonlyArray<BrowseProject>;
	readonly focal?: string | null;
}

export interface CurrentRevision {
	readonly revision: number;
	readonly indexingReady: boolean;
}

// --- list_symbols / find_symbols_by_pattern / get_context wire ---

export interface SymbolLocation {
	readonly file: string;
	readonly start_line: number;
	readonly end_line: number;
	readonly start_col: number;
	readonly end_col: number;
}

export interface SymbolSignatureParam {
	readonly name: string;
	readonly ty?: { display: string };
}

export interface SymbolSignature {
	readonly params?: ReadonlyArray<SymbolSignatureParam>;
	readonly returns?: { display: string };
}

/**
 * Structured per-symbol record returned by `list_symbols`,
 * `find_symbols_by_pattern`, `find_symbol`. Fields beyond the core
 * five are optional because builtins / re-exports / externals
 * legitimately lack them.
 */
export interface RawSymbol {
	readonly fqdn: string;
	readonly name: string;
	readonly kind: string;
	readonly module: string | null;
	readonly visibility: string;
	readonly language_kind: string;
	readonly location: SymbolLocation;
	readonly decl_kind?: string;
	readonly body_hash?: string;
	readonly signature?: SymbolSignature;
	readonly entry_point?: string | null;
	readonly receiver_type?: string | null;
	readonly implements_trait?: string | null;
}

export interface ListSymbolsOptions {
	readonly kind?: string;
	readonly module?: string;
	readonly visibility?: string;
	readonly externals?: boolean;
	readonly limit?: number;
	readonly cursor?: string;
}

export interface ListSymbolsResponse {
	readonly items: ReadonlyArray<RawSymbol>;
	readonly next_cursor?: string | null;
}

export interface ProjectKind {
	readonly kind: string;
}

export interface RawProject {
	readonly project_id: number;
	readonly label: string;
	readonly kind: ProjectKind;
	readonly rel_path: string;
	readonly root_path: string;
}

export interface ListProjectsResponse {
	readonly projects: ReadonlyArray<RawProject>;
}

export type ContextEdgeKind =
	| 'CALLS'
	| 'IMPORTS'
	| 'USES_TYPE'
	| 'IMPLEMENTS'
	| 'EXTENDS'
	| 'REFERENCES'
	| 'TESTS';

export interface ContextEdgeTarget {
	readonly kind: 'resolved' | 'unresolved' | 'external';
	readonly fqdn?: string;
	readonly raw_path?: string;
}

export interface ContextEdge {
	readonly edge_kind: ContextEdgeKind;
	readonly target: ContextEdgeTarget;
	readonly resolved_symbol?: RawSymbol | null;
}

export interface GetContextResponse {
	readonly context: {
		readonly symbol: RawSymbol;
		readonly enrichment_description: string | null;
		readonly document_description: string | null;
	};
	readonly callers: ReadonlyArray<ContextEdge>;
	readonly callees: ReadonlyArray<ContextEdge>;
	readonly imports: ReadonlyArray<ContextEdge>;
	readonly imported_by: ReadonlyArray<ContextEdge>;
}
