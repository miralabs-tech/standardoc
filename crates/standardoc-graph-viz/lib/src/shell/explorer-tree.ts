import type { BrowseSymbol } from '../mcp-client';
import type {
  EntryPointKind,
  ExplorerTreeNode,
} from '../index';

import type {
  DirNode,
  ModuleTrieNode,
  PathTrieNode,
  ProjectLike,
  TreeOut,
} from './types';
import { mapBrowseSymbolKind, stripProjectPrefix } from './symbols';

function emptyDir(): DirNode {
  return { children: new Map(), files: new Map() };
}

function emptyTrie(): PathTrieNode {
  return { children: new Map() };
}

/**
 * Disambiguate display labels when several projects share the same
 * daemon-provided `label` (e.g. `Lurlang` root + `Lurlang` runtime,
 * `Standardoc` crates + `Standardoc` ext/vscode). Collision-only:
 * unique labels pass through untouched. Suffix uses the rel_path's
 * last path segment, which is concise and matches what users would
 * naturally type in a file picker (`runtime`, `crates`, `vscode`).
 * The full rel_path remains on the node's `description` for hover.
 */
function disambiguateProjectLabels(
  projects: ReadonlyArray<ProjectLike>,
): Map<number, string> {
  const byLabel = new Map<string, ProjectLike[]>();
  for (const p of projects) {
    const bucket = byLabel.get(p.label);
    if (bucket === undefined) byLabel.set(p.label, [p]);
    else bucket.push(p);
  }
  const out = new Map<number, string>();
  for (const [label, group] of byLabel) {
    if (group.length === 1) {
      out.set(group[0]!.project_id, label);
      continue;
    }
    for (const p of group) {
      const segs = p.rel_path.replace(/\\/g, '/').split('/').filter(Boolean);
      const tail = segs[segs.length - 1] ?? p.rel_path;
      out.set(p.project_id, `${label} (${tail})`);
    }
  }
  return out;
}

/**
 * IDE-style workspace tree. We project every project's rel_path onto
 * a path trie so siblings under shared directories nest properly:
 * `crates/standardoc-graph-viz/{lib,pkg,playground}` end up as
 * children of `standardoc-graph-viz` rather than four flat entries
 * under `crates`. Labels are taken from the path segment (matching
 * what you'd see in any file explorer); the daemon-provided project
 * label sits in `title` so hover surfaces the canonical name without
 * polluting the visible label with crate-system suffixes.
 *
 * If a project's directory is ALSO an ancestor of other projects, it
 * renders as both project + folder: its own file tree merges with the
 * sub-projects' nodes under one combined entry.
 */
export function buildWorkspaceTree(
  workspaceLabel: string,
  projects: ReadonlyArray<ProjectLike>,
  allSymbols: ReadonlyArray<BrowseSymbol>,
  out: TreeOut,
): ExplorerTreeNode {
  const trie = emptyTrie();
  for (const p of projects) {
    const segs = p.rel_path.replace(/\\/g, '/').split('/').filter(Boolean);
    let cur = trie;
    for (const seg of segs) {
      let next = cur.children.get(seg);
      if (next === undefined) {
        next = emptyTrie();
        cur.children.set(seg, next);
      }
      cur = next;
    }
    cur.project = p;
  }

  return {
    id: 'workspace',
    label: workspaceLabel,
    kind: 'workspace',
    children: trieToExplorerNodes(trie, 'ws', allSymbols, out),
  };
}

/**
 * Flat-projects view — workspace → projects → modules. Drops FS layout
 * but keeps project membership visible; each project expands to a flat
 * alphabetical list of its modules so the user can drill without
 * traversing folder hierarchy. Differs from `buildModulesTree` in that
 * modules are NOT nested by `::` segments — flat.
 */
export function buildProjectsTreeFlat(
  workspaceLabel: string,
  projects: ReadonlyArray<ProjectLike>,
  allSymbols: ReadonlyArray<BrowseSymbol>,
  out: TreeOut,
): ExplorerTreeNode {
  const modulesByProject = collectModulesByProject(allSymbols);
  const displayLabels = disambiguateProjectLabels(projects);
  const children: ExplorerTreeNode[] = [];
  const sorted = [...projects].sort((a, b) =>
    (displayLabels.get(a.project_id) ?? a.label)
      .localeCompare(displayLabels.get(b.project_id) ?? b.label),
  );
  for (const p of sorted) {
    const id = `proj/${p.project_id}`;
    out.projectByExplorerId.set(id, p);
    const modules = [...(modulesByProject.get(p.project_id) ?? new Set<string>())]
      .sort((a, b) => a.localeCompare(b));
    const moduleNodes: ExplorerTreeNode[] = modules.map(mod => ({
      id: `${id}::${mod}`,
      label: mod,
      kind: 'module',
      children: undefined,
      fqdn: mod,
    }));
    children.push({
      id,
      label: displayLabels.get(p.project_id) ?? p.label,
      kind: 'project',
      children: moduleNodes.length > 0 ? moduleNodes : undefined,
      fqdn: null,
      description: p.rel_path,
    });
  }
  return {
    id: 'workspace',
    label: workspaceLabel,
    kind: 'workspace',
    children,
  };
}

function collectModulesByProject(allSymbols: ReadonlyArray<BrowseSymbol>): Map<number, Set<string>> {
  const byProject = new Map<number, Set<string>>();
  for (const s of allSymbols) {
    const pid = s.project_id;
    if (typeof pid !== 'number') continue;
    const m = s.module;
    if (typeof m !== 'string' || m.length === 0) continue;
    let set = byProject.get(pid);
    if (set === undefined) {
      set = new Set<string>();
      byProject.set(pid, set);
    }
    set.add(m);
  }
  return byProject;
}

/**
 * IR-aligned view — projects → modules nested by `::` segments. Strips
 * incidental FS layout entirely; each project's module hierarchy is
 * reconstructed from the daemon's `module` strings via a segment trie.
 * Modules that exist only as ancestors get virtual nodes so leaves
 * still nest properly (e.g. `foo::bar::baz` produces foo → bar → baz
 * even if `foo::bar` itself never holds a symbol).
 */
export function buildModulesTree(
  workspaceLabel: string,
  projects: ReadonlyArray<ProjectLike>,
  allSymbols: ReadonlyArray<BrowseSymbol>,
  out: TreeOut,
): ExplorerTreeNode {
  const byProject = collectModulesByProject(allSymbols);
  const displayLabels = disambiguateProjectLabels(projects);
  const sortedProjects = [...projects].sort((a, b) =>
    (displayLabels.get(a.project_id) ?? a.label)
      .localeCompare(displayLabels.get(b.project_id) ?? b.label),
  );
  const children: ExplorerTreeNode[] = [];
  for (const p of sortedProjects) {
    const projId = `proj/${p.project_id}`;
    out.projectByExplorerId.set(projId, p);
    const modules = [...(byProject.get(p.project_id) ?? new Set<string>())].sort((a, b) => a.localeCompare(b));
    const moduleNodes = modulesToTreeNodes(modules, projId);
    children.push({
      id: projId,
      label: displayLabels.get(p.project_id) ?? p.label,
      kind: 'project',
      children: moduleNodes.length > 0 ? moduleNodes : undefined,
      fqdn: null,
      description: p.rel_path,
    });
  }
  return {
    id: 'workspace',
    label: workspaceLabel,
    kind: 'workspace',
    children,
  };
}

function modulesToTreeNodes(modules: ReadonlyArray<string>, idPrefix: string): ExplorerTreeNode[] {
  const root: ModuleTrieNode = { children: new Map() };
  for (const m of modules) {
    const segs = m.split('::').filter(s => s.length > 0);
    if (segs.length === 0) continue;
    let cur = root;
    for (const seg of segs) {
      let next = cur.children.get(seg);
      if (next === undefined) {
        next = { children: new Map() };
        cur.children.set(seg, next);
      }
      cur = next;
    }
    cur.fullFqdn = m;
  }
  return moduleTrieToNodes(root, idPrefix, '');
}

function moduleTrieToNodes(node: ModuleTrieNode, idPrefix: string, fqdnPrefix: string): ExplorerTreeNode[] {
  const nodes: ExplorerTreeNode[] = [];
  for (const seg of [...node.children.keys()].sort((a, b) => a.localeCompare(b))) {
    const child = node.children.get(seg);
    if (child === undefined) continue;
    const fqdn = fqdnPrefix.length > 0 ? `${fqdnPrefix}::${seg}` : seg;
    const childId = `${idPrefix}::${seg}`;
    const grandChildren = moduleTrieToNodes(child, childId, fqdn);
    nodes.push({
      id: childId,
      label: seg,
      kind: 'module',
      children: grandChildren.length > 0 ? grandChildren : undefined,
      fqdn: child.fullFqdn ?? fqdn,
    });
  }
  return nodes;
}

function trieToExplorerNodes(
  trie: PathTrieNode,
  idPrefix: string,
  allSymbols: ReadonlyArray<BrowseSymbol>,
  out: TreeOut,
): ExplorerTreeNode[] {
  const nodes: ExplorerTreeNode[] = [];
  for (const name of [...trie.children.keys()].sort((a, b) => a.localeCompare(b))) {
    const child = trie.children.get(name);
    if (child === undefined) continue;
    const childId = `${idPrefix}/${name}`;
    const subProjectNodes = trieToExplorerNodes(child, childId, allSymbols, out);
    if (child.project !== undefined) {
      // This trie level is a real project. Render with project kind,
      // path-segment as the visible label, daemon label as tooltip-
      // shaped metadata. Merge sub-project entries with the project's
      // own file tree under one combined children array.
      const project = child.project;
      out.projectByExplorerId.set(childId, project);
      const projectNode = buildProjectNode(project, allSymbols, out);
      const merged: ExplorerTreeNode[] = [
        ...subProjectNodes,
        ...(projectNode.children ?? []),
      ];
      nodes.push({
        id: childId,
        label: name,
        kind: 'project',
        children: merged.length > 0 ? merged : undefined,
        fqdn: null,
        description: `${project.label} (${project.rel_path})`,
      });
    } else {
      // Pure folder — only purpose is to nest sub-projects.
      nodes.push({
        id: childId,
        label: name,
        kind: 'folder',
        children: subProjectNodes,
      });
    }
  }
  return nodes;
}

function buildProjectNode(
  project: { project_id: number; label: string; rel_path: string },
  allSymbols: ReadonlyArray<BrowseSymbol>,
  out: TreeOut,
): ExplorerTreeNode {
  const root = emptyDir();
  let touchedFiles = 0;
  for (const s of allSymbols) {
    if (s.project_id !== project.project_id) continue;
    if (!s.file || s.file.length === 0) continue;
    const rel = stripProjectPrefix(s.file, project.rel_path);
    if (rel === null || rel.length === 0) continue;
    const parts = rel.split(/[/\\]/).filter(p => p.length > 0);
    if (parts.length === 0) continue;
    const fileName = parts[parts.length - 1];
    if (fileName === undefined) continue;
    const dirs = parts.slice(0, -1);
    let cur = root;
    for (const d of dirs) {
      let next = cur.children.get(d);
      if (next === undefined) {
        next = emptyDir();
        cur.children.set(d, next);
      }
      cur = next;
    }
    const bucket = cur.files.get(fileName);
    if (bucket === undefined) {
      cur.files.set(fileName, [s]);
      touchedFiles++;
    } else {
      bucket.push(s);
    }
  }
  const id = `project:${project.project_id}`;
  out.projectByExplorerId.set(id, project);
  const children = touchedFiles > 0
    ? dirToNodes(root, id, project, '', out)
    : undefined;
  return {
    id,
    label: project.label,
    kind: 'project',
    children,
  };
}

function dirToNodes(
  dir: DirNode,
  idPrefix: string,
  project: ProjectLike,
  currentPath: string,
  out: TreeOut,
): ExplorerTreeNode[] {
  const nodes: ExplorerTreeNode[] = [];
  for (const name of [...dir.children.keys()].sort()) {
    const child = dir.children.get(name);
    if (child === undefined) continue;
    const id = `${idPrefix}/${name}`;
    const subPath = currentPath.length > 0 ? `${currentPath}/${name}` : name;
    out.folderById.set(id, {
      projectId: project.project_id,
      projectLabel: project.label,
      relPath: subPath,
    });
    nodes.push({
      id,
      label: name,
      kind: 'folder',
      children: dirToNodes(child, id, project, subPath, out),
    });
  }
  for (const name of [...dir.files.keys()].sort()) {
    const symbols = (dir.files.get(name) ?? []).slice().sort((a, b) => a.start_line - b.start_line);
    const id = `${idPrefix}/${name}`;
    const filePath = currentPath.length > 0 ? `${currentPath}/${name}` : name;
    out.fileById.set(id, { id, path: filePath, projectLabel: project.label, symbols });
    nodes.push({
      id,
      label: name,
      kind: 'file',
      children: symbols.map(s => ({
        id: `sym:${s.fqdn}`,
        label: s.name,
        kind: mapBrowseSymbolKind(s),
        fqdn: s.fqdn,
        visibility: s.visibility,
        entryPointKind: (s.entry_point ?? null) as EntryPointKind | null,
      })),
    });
  }
  return nodes;
}
