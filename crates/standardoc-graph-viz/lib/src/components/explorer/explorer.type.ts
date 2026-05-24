// Public data shapes consumed by `<standardoc-explorer>`. The host is
// expected to derive these from MCP responses (list_projects + a
// module-scoped list_symbols walk for the tree; list_symbols filtered
// on `entry_point != null` for the entry-points section) and hand them
// to the element via the `tree` / `entryPoints` property setters.
// Keeping the shapes UI-shaped (not wire-shaped) lets the host coalesce
// across multiple MCP calls without leaking the source structure into
// the component.

export type ExplorerNodeKind =
	| 'workspace'
	| 'project'
	| 'folder'
	| 'module'
	| 'struct'
	| 'enum'
	| 'function'
	| 'trait'
	| 'macro'
	| 'value'
	| 'unknown';

export interface ExplorerTreeNode {
	readonly id: string;
	readonly label: string;
	readonly kind: ExplorerNodeKind;
	/** Resolved FQDN if this node maps to a symbol. Folders/projects leave this null. */
	readonly fqdn?: string | null;
	readonly children?: ReadonlyArray<ExplorerTreeNode>;
	/**
	 * `true` when this node should render as expandable even though
	 * `children` is undefined — the host will lazily populate them in
	 * response to `sd-explorer-expand`.
	 */
	readonly expandable?: boolean;
	/** Optional loading marker — renders as a `…` child placeholder. */
	readonly loading?: boolean;
}

export interface ExplorerExpandDetail {
	readonly id: string;
	readonly fqdn?: string | null;
}

export type EntryPointKind = 'binary_main' | 'public_api' | 'ffi_export';

export interface ExplorerEntryPoint {
	readonly fqdn: string;
	readonly label: string;
	readonly kind: EntryPointKind;
}

export interface ExplorerSelectDetail {
	readonly fqdn: string;
	readonly source: 'tree' | 'entry-points' | 'recents';
}

export interface ExplorerSearchDetail {
	readonly query: string;
}
