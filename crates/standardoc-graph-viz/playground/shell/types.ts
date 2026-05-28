import type { BrowseSymbol } from '@standarx/standardoc-viz/mcp-client';

/**
 * Overview navigation state. The default `workspace` mode paints
 * every project as a cluster (project_id = cluster_id). Clicking a
 * project cluster — or a folder/project in the Explorer — switches
 * the scope to `project` or `folder`; the Overview then paints the
 * modules inside that scope and their inter-module edges. Going back
 * is one click on the breadcrumb pill the OverviewElement renders.
 */
export type OverviewScope =
  | { kind: 'workspace' }
  | { kind: 'project'; projectId: number; label: string }
  | { kind: 'folder'; projectId: number; label: string; relPath: string }
  | { kind: 'module'; prefix: string; label: string };

/**
 * What a cluster click should dispatch to. The Overview canvas only
 * knows opaque u32 ids; the shell owns the resolution table so each
 * scope can rewrite click semantics independently — workspace mode
 * drills into projects, scoped modes drill one FQDN segment deeper
 * via `drill-module`.
 */
export type ClusterTarget =
  | { kind: 'drill-project'; projectId: number; label: string }
  | { kind: 'drill-folder'; projectId: number; label: string; relPath: string }
  | { kind: 'drill-module'; prefix: string; label: string }
  | { kind: 'focus-symbol'; fqdn: string };

export interface DirNode {
  readonly children: Map<string, DirNode>;
  readonly files: Map<string, BrowseSymbol[]>;
}

export interface FileEntry {
  readonly id: string;
  readonly path: string;
  readonly projectLabel: string;
  readonly symbols: ReadonlyArray<BrowseSymbol>;
}

export interface FolderEntry {
  readonly projectId: number;
  readonly projectLabel: string;
  /**
   * Project-relative folder path (no leading separator). Empty string
   * means the project root itself (only used when the tree-builder
   * exposes it as a folder, normally the project node is preferred).
   */
  readonly relPath: string;
}

export interface ProjectLike {
  readonly project_id: number;
  readonly label: string;
  readonly rel_path: string;
}

/**
 * Side-channel maps collected during the workspace tree walk. The
 * Explorer dispatches clicks via opaque node ids; these maps let the
 * host resolve a click back to the rich metadata (file entry,
 * folder coords inside a project, project) needed to drive the
 * Inspector and the Overview scope.
 */
export interface TreeOut {
  readonly fileById: Map<string, FileEntry>;
  readonly folderById: Map<string, FolderEntry>;
  readonly projectByExplorerId: Map<string, ProjectLike>;
}

export interface PathTrieNode {
  /** Project bound at this exact path (rel_path === idPath), if any. */
  project?: ProjectLike;
  /** Sub-segments under this node. */
  children: Map<string, PathTrieNode>;
}

export interface ModuleTrieNode {
  fullFqdn?: string;
  children: Map<string, ModuleTrieNode>;
}

export interface BuiltOverviewPayload {
  readonly json: string;
  readonly targets: Map<number, ClusterTarget>;
  /** Distinct IR edge kinds present in the cross-edge set; consumed
   *  by the OverviewElement legend so chips reflect what's actually
   *  in the current scope (no dead chips for kinds with 0 edges). */
  readonly crossKinds: ReadonlyArray<string>;
}

export type GraphEdge = { from: string; to: string; kind: string; outbound: boolean };

export interface OverviewNodeBuilder {
  fqdn: string;
  label: string;
  depth: number;
  parent_fqdn: string | null;
  node_kind: 'module' | 'public_symbol';
  symbol_count: number;
  project_kind?: string;
}

export type WorkspaceProject = { project_id: number; label: string; rel_path: string; kind: { kind: string } };

export interface CollapsedProject {
  project_id: number;
  label: string;
  rel_path: string;
  kind: { kind: string };
}

export interface CollapseResult {
  collapsed: ReadonlyArray<CollapsedProject>;
  canonicalProjectId: Map<number, number>;
}
