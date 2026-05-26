/**
 * `<standardoc-explorer>` — left-rail panel of the shell. Stacked
 * inside the panel body, with inline filter chips above the tree:
 *
 *   1. FILTER CHIPS — kind / visibility / entry-point rows, AND across
 *      rows, OR within a row. Doubles as the live legend.
 *   2. TREE — workspace → projects → modules → leaf symbols
 *   3. ENTRY POINTS — flat list of binary_main / public_api / ffi_export
 *   4. RECENTLY VIEWED — driven by the shared `focusStore`
 *
 * Global symbol search lives in `<standardoc-search>` (header) — the
 * Explorer no longer carries a redundant input.
 *
 * Data is injected via property setters (`tree`, `entryPoints`) — the
 * element does not call MCP itself. Recents subscribe directly to the
 * `focusStore` singleton so the host doesn't have to plumb that update.
 * Clicks on tree items, entry points, and recents all call
 * `focusStore.setFocus(fqdn)` AND emit `sd-explorer-select` for any
 * host that wants to react beyond the focus change (telemetry, route
 * sync, multi-panel state).
 *
 * Events emitted:
 *   - `sd-explorer-select` detail: ExplorerSelectDetail
 */

import classigo from 'classigo';

import { focusStore, type FocusState } from '../../focus-store';
import '../legend/legend.element';
import type {
  ExplorerEntryPoint,
  ExplorerExpandDetail,
  ExplorerNodeKind,
  ExplorerSelectDetail,
  ExplorerTreeNode,
  ExplorerTreeView,
  ExplorerViewChangeDetail,
  EntryPointKind,
} from './explorer.type';
import s from './explorer.module.scss';

export const STANDARDOC_EXPLORER_TAG = 'standardoc-explorer';

const C = {
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

const kindIconClass: Record<ExplorerNodeKind, string> = {
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

const entryBadgeClass: Record<EntryPointKind, string> = {
  binary_main: C.entryBadgeBinMain,
  public_api: C.entryBadgePublicApi,
  ffi_export: C.entryBadgeFfiExport,
};

const entryBadgeLabel: Record<EntryPointKind, string> = {
  binary_main: 'binary_main',
  public_api: 'public_api',
  ffi_export: 'ffi_export',
};

function shortFqdn(fqdn: string): string {
  const idx = fqdn.lastIndexOf('::');
  return idx >= 0 ? fqdn.slice(idx + 2) : fqdn;
}

function entryPointScope(fqdn: string): string | null {
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
function findAncestorIds(
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

export class ExplorerElement extends HTMLElement {
  #mounted = false;
  #tree: ReadonlyArray<ExplorerTreeNode> = [];
  #entryPoints: ReadonlyArray<ExplorerEntryPoint> = [];
  #expanded = new Set<string>();
  #userExpanded = new Set<string>();
  #autoExpanded = new Set<string>();
  #selectedId: string | null = null;
  #unsubscribeFocus: (() => void) | null = null;
  #focus: FocusState = focusStore.get();
  #kindFilter = new Set<ExplorerNodeKind>();
  #visibilityFilter = new Set<string>();
  #entryPointFilter = new Set<EntryPointKind>();
  #treeView: ExplorerTreeView = 'files';

  #nodes: {
    root: HTMLElement;
    treeMount: HTMLElement;
    entryPointsMount: HTMLElement;
    recentsMount: HTMLElement;
    treeViewBtns: ReadonlyArray<HTMLButtonElement>;
  } | null = null;

  set tree(next: ReadonlyArray<ExplorerTreeNode>) {
    this.#tree = next;
    this.#renderTree();
  }

  set treeView(next: ExplorerTreeView) {
    if (this.#treeView === next) return;
    this.#treeView = next;
    this.#syncTreeViewButtons();
  }

  get treeView(): ExplorerTreeView {
    return this.#treeView;
  }

  #syncTreeViewButtons(): void {
    const n = this.#nodes;
    if (n === null) return;
    for (const btn of n.treeViewBtns) {
      const v = btn.dataset['view'];
      btn.className = classigo(C.treeViewBtn, v === this.#treeView && C.treeViewBtnActive);
    }
  }

  set entryPoints(next: ReadonlyArray<ExplorerEntryPoint>) {
    this.#entryPoints = next;
    this.#renderEntryPoints();
  }

  /**
   * Host-set selection id — useful when the host wants the Explorer
   * to highlight a node that wasn't picked via a click in here
   * (e.g. a deep link, a synthetic file selection from another panel).
   */
  set selectedId(id: string | null) {
    if (id === this.#selectedId) return;
    this.#selectedId = id;
    this.#renderTree();
  }

  get selectedId(): string | null {
    return this.#selectedId;
  }

  connectedCallback(): void {
    if (this.#mounted) return;
    this.#mounted = true;
    this.#render();
    this.#unsubscribeFocus = focusStore.subscribe(state => {
      const prevFqdn = this.#focus.current;
      this.#focus = state;
      // Auto-expand the path leading to the focused symbol so its
      // row is actually visible — useful when the focus shift
      // originates outside the Explorer (search, focus graph click,
      // cluster drill, recents click on a hidden node). Track which
      // ids we auto-expanded so the next focus shift can close any
      // ancestor the user didn't manually open — keeps the tree
      // from sprawling open as navigation accumulates.
      if (state.current !== null && state.current !== prevFqdn) {
        const path = findAncestorIds(this.#tree, state.current);
        const newAuto = new Set<string>();
        if (path !== null) {
          for (const id of path.slice(0, -1)) {
            newAuto.add(id);
            this.#expanded.add(id);
          }
        }
        // Collapse anything we auto-opened last time that's no
        // longer on the new path AND wasn't user-toggled in
        // between.
        for (const stale of this.#autoExpanded) {
          if (!newAuto.has(stale) && !this.#userExpanded.has(stale)) {
            this.#expanded.delete(stale);
          }
        }
        this.#autoExpanded = newAuto;
      }
      this.#renderRecents();
      this.#renderTree();
      if (state.current !== null && state.current !== prevFqdn) {
        this.#scrollSelectedIntoView();
      }
    });
  }

  disconnectedCallback(): void {
    this.#unsubscribeFocus?.();
    this.#unsubscribeFocus = null;
  }

  #render(): void {
    const root = document.createElement('div');
    root.className = C.explorer;
    root.innerHTML = `
			<div class="${C.header}">Explorer</div>
			<div data-role="filter-mount"></div>
			<div class="${C.body}">
				<section class="${C.section}">
					<div class="${C.treeHeader}">
						<span class="${C.sectionTitle}">Tree</span>
						<span class="${C.treeViewToggle}">
							<button type="button" class="${C.treeViewBtn}" data-view="files">files</button>
							<button type="button" class="${C.treeViewBtn}" data-view="modules">modules</button>
							<button type="button" class="${C.treeViewBtn}" data-view="projects">projects</button>
						</span>
					</div>
					<div data-role="tree-mount"></div>
				</section>
				<section class="${C.section}">
					<div class="${C.sectionTitle}">Entry points</div>
					<div data-role="entry-points-mount"></div>
				</section>
				<section class="${C.section}">
					<div class="${C.sectionTitle}">Recently viewed</div>
					<div data-role="recents-mount"></div>
				</section>
				<section class="${C.section}">
					<standardoc-legend collapsed></standardoc-legend>
				</section>
			</div>
		`;
    this.replaceChildren(root);

    const treeViewBtns = Array.from(root.querySelectorAll<HTMLButtonElement>(`.${C.treeViewBtn}`));
    for (const btn of treeViewBtns) {
      btn.addEventListener('click', () => {
        const next = btn.dataset['view'] as ExplorerTreeView | undefined;
        if (next === undefined || next === this.#treeView) return;
        this.#treeView = next;
        this.#syncTreeViewButtons();
        this.dispatchEvent(new CustomEvent<ExplorerViewChangeDetail>('sd-explorer-view-change', {
          detail: { view: next },
          bubbles: true,
          composed: true,
        }));
      });
    }

    this.#nodes = {
      root,
      treeMount: root.querySelector<HTMLElement>('[data-role="tree-mount"]')!,
      entryPointsMount: root.querySelector<HTMLElement>('[data-role="entry-points-mount"]')!,
      recentsMount: root.querySelector<HTMLElement>('[data-role="recents-mount"]')!,
      treeViewBtns,
    };
    this.#syncTreeViewButtons();

    this.#renderFilterChips(root.querySelector<HTMLElement>('[data-role="filter-mount"]')!);
    this.#renderTree();
    this.#renderEntryPoints();
    this.#renderRecents();
  }

  /// Render the three filter chip rows above the tree (kind / vis /
  /// entry). Containers (workspace / project / folder / file / module)
  /// are NOT filterable — they're navigational scaffolding. The chips'
  /// colour-coded swatches (kind row) double as the kind legend, so the
  /// dedicated legend section was retired.
  #renderFilterChips(mount: HTMLElement): void {
    mount.replaceChildren();
    mount.style.cssText = 'display:flex;flex-direction:column;gap:2px;padding:4px 8px;border-bottom:1px solid var(--sd-border-subtle,#2d2d2d);';

    const kindChips: Array<{ value: ExplorerNodeKind; label: string; swatchCls: string }> = [
      { value: 'struct', label: 'struct', swatchCls: C.kindType },
      { value: 'enum', label: 'enum', swatchCls: C.kindType },
      { value: 'trait', label: 'trait', swatchCls: C.kindType },
      { value: 'function', label: 'fn', swatchCls: C.kindCallable },
      { value: 'value', label: 'const', swatchCls: C.kindValue },
      { value: 'macro', label: 'macro', swatchCls: C.kindMacro },
    ];
    this.#renderChipRow<ExplorerNodeKind>(mount, 'kind', kindChips, this.#kindFilter);

    const visChips = (['public', 'crate', 'private', 'protected'] as const).map((v) => ({
      value: v,
      label: v,
      swatchCls: '',
    }));
    this.#renderChipRow<string>(mount, 'vis', visChips, this.#visibilityFilter);

    const entryChips: Array<{ value: EntryPointKind; label: string; swatchCls: string }> = [
      { value: 'binary_main', label: entryBadgeLabel.binary_main, swatchCls: '' },
      { value: 'public_api', label: entryBadgeLabel.public_api, swatchCls: '' },
      { value: 'ffi_export', label: entryBadgeLabel.ffi_export, swatchCls: '' },
    ];
    this.#renderChipRow<EntryPointKind>(mount, 'entry', entryChips, this.#entryPointFilter);
  }

  #renderChipRow<T extends string>(
    parent: HTMLElement,
    label: string,
    chips: ReadonlyArray<{ value: T; label: string; swatchCls: string }>,
    activeSet: Set<T>,
  ): void {
    const row = document.createElement('div');
    row.style.cssText = 'display:flex;flex-wrap:wrap;gap:4px;align-items:center;';
    const tag = document.createElement('span');
    tag.textContent = `${label}:`;
    tag.style.cssText = 'font-size:9px;color:var(--sd-fg-dim,#6e7681);text-transform:uppercase;letter-spacing:0.5px;font-family:var(--sd-font-mono,ui-monospace,monospace);margin-right:2px;';
    row.appendChild(tag);
    for (const { value, label: chipLabel, swatchCls } of chips) {
      const active = activeSet.has(value);
      const chip = document.createElement('button');
      chip.type = 'button';
      chip.textContent = chipLabel;
      chip.style.cssText = [
        `background: ${active ? 'var(--sd-selection, #094771)' : 'transparent'}`,
        'border: 1px solid var(--sd-border-subtle, #2d2d2d)',
        'color: var(--sd-fg, #cccccc)',
        'font-size: 10px',
        'padding: 2px 8px',
        'border-radius: 999px',
        'cursor: pointer',
        'display: inline-flex',
        'align-items: center',
        'gap: 4px',
        'font-family: var(--sd-font-mono, ui-monospace, monospace)',
        active ? 'opacity: 1' : 'opacity: 0.7',
      ].join(';');
      if (swatchCls.length > 0) {
        const swatch = document.createElement('span');
        swatch.className = swatchCls;
        swatch.style.cssText = 'width:8px;height:8px;border-radius:50%;display:inline-block;background:currentColor;';
        chip.prepend(swatch);
      }
      chip.addEventListener('click', () => {
        if (activeSet.has(value)) activeSet.delete(value);
        else activeSet.add(value);
        this.#renderFilterChips(parent);
        this.#renderTree();
      });
      row.appendChild(chip);
    }
    if (activeSet.size > 0) {
      const clear = document.createElement('button');
      clear.type = 'button';
      clear.textContent = 'clear';
      clear.style.cssText = [
        'background: transparent',
        'border: 1px dashed var(--sd-border, #454545)',
        'color: var(--sd-fg-muted, #9d9d9d)',
        'font-size: 10px',
        'padding: 2px 8px',
        'border-radius: 999px',
        'cursor: pointer',
        'font-family: var(--sd-font-mono, ui-monospace, monospace)',
      ].join(';');
      clear.addEventListener('click', () => {
        activeSet.clear();
        this.#renderFilterChips(parent);
        this.#renderTree();
      });
      row.appendChild(clear);
    }
    parent.appendChild(row);
  }

  /// Apply the active filters (kind / visibility / entry-point) to the
  /// tree. Symbol leaves are dropped when they fail ANY active filter
  /// (cross-row AND, within-row OR); containers with no surviving
  /// descendants disappear so the tree doesn't expand into empty
  /// folders. Returns null when the whole node + subtree is filtered
  /// out.
  #filterTreeNode(node: ExplorerTreeNode): ExplorerTreeNode | null {
    const noFilters =
      this.#kindFilter.size === 0 &&
      this.#visibilityFilter.size === 0 &&
      this.#entryPointFilter.size === 0;
    if (noFilters) return node;
    const filterableSymbolKinds: ExplorerNodeKind[] = ['struct', 'enum', 'function', 'trait', 'value', 'macro'];
    const isFilterable = filterableSymbolKinds.includes(node.kind);
    if (isFilterable) {
      if (this.#kindFilter.size > 0 && !this.#kindFilter.has(node.kind)) return null;
      if (this.#visibilityFilter.size > 0) {
        const vis = node.visibility ?? '';
        if (!this.#visibilityFilter.has(vis)) return null;
      }
      if (this.#entryPointFilter.size > 0) {
        const ep = node.entryPointKind ?? null;
        if (ep === null || !this.#entryPointFilter.has(ep)) return null;
      }
      return node;
    }
    if (node.children === undefined || node.children.length === 0) {
      return node;
    }
    const filteredChildren: ExplorerTreeNode[] = [];
    for (const c of node.children) {
      const kept = this.#filterTreeNode(c);
      if (kept !== null) filteredChildren.push(kept);
    }
    if (filteredChildren.length === 0) return null;
    return { ...node, children: filteredChildren };
  }

  #renderTree(): void {
    const n = this.#nodes;
    if (n === null) return;
    if (this.#tree.length === 0) {
      n.treeMount.innerHTML = `<div class="${C.empty}">No projects yet — load a workspace.</div>`;
      return;
    }
    const ul = document.createElement('ul');
    ul.className = C.tree;
    const filtered: ExplorerTreeNode[] = [];
    for (const root of this.#tree) {
      const kept = this.#filterTreeNode(root);
      if (kept !== null) filtered.push(kept);
    }
    if (filtered.length === 0) {
      n.treeMount.innerHTML = `<div class="${C.empty}">No symbols match the active filter.</div>`;
      return;
    }
    for (const node of filtered) ul.appendChild(this.#renderNode(node, 0));
    n.treeMount.replaceChildren(ul);
  }

  #renderNode(node: ExplorerTreeNode, depth: number): HTMLLIElement {
    const li = document.createElement('li');
    li.className = C.node;
    const hasChildren = node.children !== undefined && node.children.length > 0;
    const canExpand = hasChildren || node.expandable === true;
    const isExpanded = this.#expanded.has(node.id);
    const isFocused = node.fqdn !== null && node.fqdn !== undefined && node.fqdn === this.#focus.current;
    const isSelected = node.id === this.#selectedId || isFocused;

    const row = document.createElement('div');
    row.className = classigo(C.nodeRow, isSelected && C.nodeRowSelected);
    row.style.paddingLeft = `${6 + depth * 8}px`;

    const twisty = document.createElement('span');
    twisty.className = C.nodeTwisty;
    twisty.textContent = canExpand ? (isExpanded ? '▾' : '▸') : '';

    const icon = document.createElement('span');
    icon.className = classigo(C.nodeIcon, this.#iconClassFor(node.kind));

    const label = document.createElement('span');
    label.className = C.nodeLabel;
    label.textContent = node.label;

    if (node.description !== undefined && node.description.length > 0) {
      row.title = node.description;
    } else if (node.fqdn !== null && node.fqdn !== undefined) {
      row.title = node.fqdn;
    }

    row.appendChild(twisty);
    row.appendChild(icon);
    row.appendChild(label);
    row.addEventListener('click', () => {
      if (canExpand) {
        if (isExpanded) {
          this.#expanded.delete(node.id);
          this.#userExpanded.delete(node.id);
        } else {
          this.#expanded.add(node.id);
          // User-driven expansion → record so the auto-close
          // sweep on the next focus shift leaves it alone.
          this.#userExpanded.add(node.id);
          // Lazy-load: first expansion of a node declared expandable
          // but with no children yet → ask the host to populate.
          if (!hasChildren && node.expandable === true) {
            const detail: ExplorerExpandDetail = { id: node.id, fqdn: node.fqdn ?? null };
            this.dispatchEvent(new CustomEvent('sd-explorer-expand', {
              detail, bubbles: true, composed: true,
            }));
          }
        }
        this.#renderTree();
      }
      // Always emit a select event so the host can react to non-symbol
      // clicks (file profile, folder breadcrumb) — even when there's
      // no FQDN to push into the focus store.
      this.#selectedId = node.id;
      this.#selectNode(node, 'tree');
    });

    li.appendChild(row);

    if (canExpand && isExpanded) {
      const childUl = document.createElement('ul');
      childUl.className = C.nodeChildren;
      if (hasChildren) {
        for (const child of node.children!) childUl.appendChild(this.#renderNode(child, depth + 1));
      } else if (node.loading === true) {
        const loadingLi = document.createElement('li');
        loadingLi.className = classigo(C.node, C.empty);
        loadingLi.style.paddingLeft = `${12 + (depth + 1) * 12}px`;
        loadingLi.textContent = 'Loading…';
        childUl.appendChild(loadingLi);
      }
      li.appendChild(childUl);
    }
    return li;
  }

  #iconClassFor(kind: ExplorerNodeKind): string {
    return kindIconClass[kind] ?? C.kindUnknown;
  }

  #renderEntryPoints(): void {
    const n = this.#nodes;
    if (n === null) return;
    if (this.#entryPoints.length === 0) {
      n.entryPointsMount.innerHTML = `<div class="${C.empty}">None detected.</div>`;
      return;
    }
    const frag = document.createDocumentFragment();
    for (const ep of this.#entryPoints) {
      const row = document.createElement('div');
      row.className = C.entry;
      const text = document.createElement('div');
      text.className = C.entryText;
      const label = document.createElement('span');
      label.className = C.entryLabel;
      label.textContent = ep.label;
      text.appendChild(label);
      const scope = entryPointScope(ep.fqdn);
      if (scope !== null && scope !== ep.label) {
        const scopeEl = document.createElement('span');
        scopeEl.className = C.entryScope;
        scopeEl.textContent = scope;
        text.appendChild(scopeEl);
      }
      const badge = document.createElement('span');
      badge.className = classigo(C.entryBadge, entryBadgeClass[ep.kind] ?? '');
      badge.textContent = entryBadgeLabel[ep.kind] ?? ep.kind;
      row.appendChild(text);
      row.appendChild(badge);
      row.addEventListener('click', () => {
        this.#selectedId = `entry:${ep.fqdn}`;
        this.#emitSelect({
          id: `entry:${ep.fqdn}`,
          kind: 'function',
          label: ep.label,
          fqdn: ep.fqdn,
          source: 'entry-points',
        });
        focusStore.setFocus(ep.fqdn);
      });
      frag.appendChild(row);
    }
    n.entryPointsMount.replaceChildren(frag);
  }

  #renderRecents(): void {
    const n = this.#nodes;
    if (n === null) return;
    const recent = this.#focus.recent;
    if (recent.length === 0) {
      n.recentsMount.innerHTML = `<div class="${C.empty}">Click a symbol to remember it.</div>`;
      return;
    }
    // Cap visible recents at 5 items + make the list scrollable past
    // that — long histories used to push the rest of the explorer
    // off-screen. The full focusStore.recent set still drives the
    // state; we just window the render.
    n.recentsMount.style.maxHeight = `${5 * 22 + 4}px`;
    n.recentsMount.style.overflowY = 'auto';
    const frag = document.createDocumentFragment();
    for (const fqdn of recent) {
      const row = document.createElement('div');
      row.className = classigo(C.recent, fqdn === this.#focus.current && C.recentCurrent);
      row.textContent = shortFqdn(fqdn);
      row.title = fqdn;
      row.addEventListener('click', () => {
        this.#selectedId = `recent:${fqdn}`;
        this.#emitSelect({
          id: `recent:${fqdn}`,
          kind: 'unknown',
          label: shortFqdn(fqdn),
          fqdn,
          source: 'recents',
        });
        focusStore.setFocus(fqdn);
      });
      frag.appendChild(row);
    }
    n.recentsMount.replaceChildren(frag);
  }

  // Legend removed — the kind filter chips at the top of the Explorer
  // now double as a live legend (each chip carries a coloured swatch
  // of the same hue used by the tree icons). The standalone Legend
  // section was muted at the bottom and never read in practice.

  #scrollSelectedIntoView(): void {
    const n = this.#nodes;
    if (n === null) return;
    const sel = n.treeMount.querySelector<HTMLElement>(`.${C.nodeRowSelected}`);
    if (sel !== null) {
      sel.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }
  }

  #selectNode(node: ExplorerTreeNode, source: ExplorerSelectDetail['source']): void {
    if (node.fqdn !== null && node.fqdn !== undefined) {
      focusStore.setFocus(node.fqdn);
    }
    this.#emitSelect({
      id: node.id,
      kind: node.kind,
      label: node.label,
      fqdn: node.fqdn ?? null,
      source,
    });
  }

  #emitSelect(detail: ExplorerSelectDetail): void {
    this.dispatchEvent(new CustomEvent('sd-explorer-select', {
      detail, bubbles: true, composed: true,
    }));
  }
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_EXPLORER_TAG)) {
  customElements.define(STANDARDOC_EXPLORER_TAG, ExplorerElement);
}

declare global {
  interface HTMLElementTagNameMap {
    [STANDARDOC_EXPLORER_TAG]: ExplorerElement;
  }
}
