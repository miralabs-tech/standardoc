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
  | 'file'
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
  /**
   * Symbol visibility — `public`, `private`, `crate`, `protected` (free
   * string to allow extractor-specific extensions). Drives the
   * visibility chip filter. Only meaningful on symbol-leaf nodes.
   */
  readonly visibility?: string;
  /**
   * Entry-point kind for symbols promoted as program / API surface.
   * Drives the entry-point chip filter. `null` (the default) means the
   * symbol is not an entry point.
   */
  readonly entryPointKind?: EntryPointKind | null;
  readonly children?: ReadonlyArray<ExplorerTreeNode>;
  /**
   * `true` when this node should render as expandable even though
   * `children` is undefined — the host will lazily populate them in
   * response to `sd-explorer-expand`.
   */
  readonly expandable?: boolean;
  /** Optional loading marker — renders as a `…` child placeholder. */
  readonly loading?: boolean;
  /**
   * Optional tooltip / secondary text. Shown via the row's `title`
   * attribute on hover. Use to surface the canonical project label
   * when the visible `label` is a path segment, or the file's full
   * path when only the basename is shown, etc.
   */
  readonly description?: string;
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
  readonly id: string;
  readonly kind: ExplorerNodeKind;
  readonly label: string;
  /** Present when the node maps to a real symbol. Folders / files / projects leave this null. */
  readonly fqdn: string | null;
  readonly source: 'tree' | 'entry-points' | 'recents';
}

/**
 * Navigation modes for the Explorer tree:
 *   - `files`    — filesystem layout (folders/sub-folders/files); the
 *                  default and what most IDE-style explorers show.
 *   - `modules`  — IR-aligned: only modules + their leaf symbols,
 *                  ignoring intermediate FS layout. Reads like the
 *                  daemon's module hierarchy.
 *   - `projects` — flat: just projects (crates / packages), no nesting.
 *                  Useful for big workspaces to see the top-level shape
 *                  at a glance.
 */
export type ExplorerTreeView = 'files' | 'modules' | 'projects';

export interface ExplorerViewChangeDetail {
  readonly view: ExplorerTreeView;
}

