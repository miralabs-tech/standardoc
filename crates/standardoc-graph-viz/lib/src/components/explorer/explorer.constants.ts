import type { EntryPointKind, ExplorerNodeKind } from './explorer.type';
import s from './explorer.module.scss';

export const STANDARDOC_EXPLORER_TAG = 'standardoc-explorer';

export const C = {
  explorer: s.explorer ?? '',
  header: s.explorer__header ?? '',
  body: s.explorer__body ?? '',
  section: s.explorer__section ?? '',
  sectionTitle: s['explorer__section-title'] ?? '',
  empty: s.explorer__empty ?? '',
  treeHeader: s['explorer__tree-header'] ?? '',
  treeViewToggle: s['explorer__tree-view-toggle'] ?? '',
  treeViewBtn: s['explorer__tree-view-btn'] ?? '',
  treeViewBtnActive: s['explorer__tree-view-btn--active'] ?? '',
  tree: s.explorer__tree ?? '',
  node: s.explorer__node ?? '',
  nodeRow: s['explorer__node-row'] ?? '',
  nodeRowSelected: s['explorer__node-row--selected'] ?? '',
  nodeTwisty: s['explorer__node-twisty'] ?? '',
  nodeIcon: s['explorer__node-icon'] ?? '',
  nodeLabel: s['explorer__node-label'] ?? '',
  nodeChildren: s['explorer__node-children'] ?? '',
  entry: s.explorer__entry ?? '',
  entryText: s['explorer__entry-text'] ?? '',
  entryLabel: s['explorer__entry-label'] ?? '',
  entryScope: s['explorer__entry-scope'] ?? '',
  entryBadge: s['explorer__entry-badge'] ?? '',
  entryBadgeBinMain: s['explorer__entry-badge--binary-main'] ?? '',
  entryBadgePublicApi: s['explorer__entry-badge--public-api'] ?? '',
  entryBadgeFfiExport: s['explorer__entry-badge--ffi-export'] ?? '',
  recent: s.explorer__recent ?? '',
  recentCurrent: s['explorer__recent--current'] ?? '',
  // Kind swatch palette — drives both the tree icons and the inline
  // filter chips. The dedicated legend section was retired; the chips
  // act as the live legend.
  kindModule: s['kind-module'] ?? '',
  kindType: s['kind-type'] ?? '',
  kindCallable: s['kind-callable'] ?? '',
  kindValue: s['kind-value'] ?? '',
  kindMacro: s['kind-macro'] ?? '',
  kindUnknown: s['kind-unknown'] ?? '',
} as const;

export const kindIconClass: Record<ExplorerNodeKind, string> = {
  workspace: C.kindModule,
  project: C.kindModule,
  folder: C.kindUnknown,
  file: C.kindUnknown,
  module: C.kindModule,
  struct: C.kindType,
  enum: C.kindType,
  function: C.kindCallable,
  trait: C.kindType,
  macro: C.kindMacro,
  value: C.kindValue,
  unknown: C.kindUnknown,
};

export const entryBadgeClass: Record<EntryPointKind, string> = {
  binary_main: C.entryBadgeBinMain,
  public_api: C.entryBadgePublicApi,
  ffi_export: C.entryBadgeFfiExport,
};

export const entryBadgeLabel: Record<EntryPointKind, string> = {
  binary_main: 'binary_main',
  public_api: 'public_api',
  ffi_export: 'ffi_export',
};
