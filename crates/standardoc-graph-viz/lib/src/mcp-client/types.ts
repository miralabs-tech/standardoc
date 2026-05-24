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
