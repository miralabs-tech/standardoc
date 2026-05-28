import type { ExplorerTreeNode } from './explorer.type';

export function shortFqdn(fqdn: string): string {
  const idx = fqdn.lastIndexOf('::');
  return idx >= 0 ? fqdn.slice(idx + 2) : fqdn;
}

export function entryPointScope(fqdn: string): string | null {
  const colonIdx = fqdn.indexOf('::');
  if (colonIdx >= 0) return fqdn.slice(0, colonIdx);
  const dotIdx = fqdn.indexOf('.');
  if (dotIdx >= 0) return fqdn.slice(0, dotIdx);
  return null;
}

/**
 * Depth-first walk returning every id on the path from the top-level
 * roots down to the first node whose `fqdn` matches `target`. Null when
 * the tree doesn't contain the target — the host stays a no-op in
 * that case (search hits outside the indexed tree shouldn't force the
 * Explorer into a half-expanded mess).
 */
export function findAncestorIds(
  tree: ReadonlyArray<ExplorerTreeNode>,
  target: string,
  path: ReadonlyArray<string> = [],
): string[] | null {
  for (const node of tree) {
    const mine = [...path, node.id];
    if (node.fqdn === target) return mine;
    if (node.children !== undefined && node.children.length > 0) {
      const child = findAncestorIds(node.children, target, mine);
      if (child !== null) return child;
    }
  }
  return null;
}
