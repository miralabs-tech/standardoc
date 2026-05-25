/**
 * `<standardoc-explorer>` — left-rail panel of the shell. Four sections
 * stacked top-to-bottom inside the panel body, plus a sticky search
 * field at the top:
 *
 *   1. SEARCH — debounced input emitting `sd-explorer-search`
 *   2. TREE — workspace → projects → modules → leaf symbols
 *   3. ENTRY POINTS — flat list of binary_main / public_api / ffi_export
 *   4. RECENTLY VIEWED — driven by the shared `focusStore`
 *   5. LEGEND — kind + edge palette, always visible at the bottom
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
 *   - `sd-explorer-search` detail: ExplorerSearchDetail
 */

import classigo from 'classigo';

import { focusStore, type FocusState } from '../../focus-store';
import type {
	ExplorerEntryPoint,
	ExplorerExpandDetail,
	ExplorerNodeKind,
	ExplorerSearchDetail,
	ExplorerSelectDetail,
	ExplorerTreeNode,
	EntryPointKind,
} from './explorer.type';
import s from './explorer.module.scss';

export const STANDARDOC_EXPLORER_TAG = 'standardoc-explorer';

const C = {
	explorer: s.explorer ?? '',
	header: s.explorer__header ?? '',
	search: s.explorer__search ?? '',
	searchInput: s['explorer__search-input'] ?? '',
	body: s.explorer__body ?? '',
	section: s.explorer__section ?? '',
	sectionTitle: s['explorer__section-title'] ?? '',
	empty: s.explorer__empty ?? '',
	tree: s.explorer__tree ?? '',
	node: s.explorer__node ?? '',
	nodeRow: s['explorer__node-row'] ?? '',
	nodeRowSelected: s['explorer__node-row--selected'] ?? '',
	nodeTwisty: s['explorer__node-twisty'] ?? '',
	nodeIcon: s['explorer__node-icon'] ?? '',
	nodeLabel: s['explorer__node-label'] ?? '',
	nodeChildren: s['explorer__node-children'] ?? '',
	entry: s.explorer__entry ?? '',
	entryLabel: s['explorer__entry-label'] ?? '',
	entryBadge: s['explorer__entry-badge'] ?? '',
	entryBadgeBinMain: s['explorer__entry-badge--binary-main'] ?? '',
	entryBadgePublicApi: s['explorer__entry-badge--public-api'] ?? '',
	entryBadgeFfiExport: s['explorer__entry-badge--ffi-export'] ?? '',
	recent: s.explorer__recent ?? '',
	recentCurrent: s['explorer__recent--current'] ?? '',
	// Legend SCSS classes retained in the stylesheet but unused — removed
	// the legend section in favour of inline-coloured filter chips.
	kindModule: s['kind-module'] ?? '',
	kindType: s['kind-type'] ?? '',
	kindCallable: s['kind-callable'] ?? '',
	kindValue: s['kind-value'] ?? '',
	kindMacro: s['kind-macro'] ?? '',
	kindUnknown: s['kind-unknown'] ?? '',
} as const;

const SEARCH_DEBOUNCE_MS = 150;

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
	#searchDebounceHandle: number | null = null;
	#unsubscribeFocus: (() => void) | null = null;
	#focus: FocusState = focusStore.get();
	#kindFilter = new Set<ExplorerNodeKind>();

	#nodes: {
		root: HTMLElement;
		searchInput: HTMLInputElement;
		treeMount: HTMLElement;
		entryPointsMount: HTMLElement;
		recentsMount: HTMLElement;
	} | null = null;

	set tree(next: ReadonlyArray<ExplorerTreeNode>) {
		this.#tree = next;
		this.#renderTree();
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
		if (this.#searchDebounceHandle !== null) {
			window.clearTimeout(this.#searchDebounceHandle);
			this.#searchDebounceHandle = null;
		}
	}

	#render(): void {
		const root = document.createElement('div');
		root.className = C.explorer;
		root.innerHTML = `
			<div class="${C.header}">Explorer</div>
			<div class="${C.search}">
				<input
					type="search"
					class="${C.searchInput}"
					placeholder="Search in workspace…"
					data-role="search-input"
					aria-label="Search symbols in workspace"
				/>
			</div>
			<div data-role="filter-mount" style="display: flex; flex-wrap: wrap; gap: 4px; padding: 4px 8px; border-bottom: 1px solid var(--sd-border-subtle, #2d2d2d);"></div>
			<div class="${C.body}">
				<section class="${C.section}">
					<div class="${C.sectionTitle}">Tree</div>
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
			</div>
		`;
		this.replaceChildren(root);

		this.#nodes = {
			root,
			searchInput: root.querySelector<HTMLInputElement>('[data-role="search-input"]')!,
			treeMount: root.querySelector<HTMLElement>('[data-role="tree-mount"]')!,
			entryPointsMount: root.querySelector<HTMLElement>('[data-role="entry-points-mount"]')!,
			recentsMount: root.querySelector<HTMLElement>('[data-role="recents-mount"]')!,
		};

		this.#wireSearch();
		this.#renderFilterChips(root.querySelector<HTMLElement>('[data-role="filter-mount"]')!);
		this.#renderTree();
		this.#renderEntryPoints();
		this.#renderRecents();
	}

	/// Render a chip row above the tree letting the user toggle which
	/// symbol kinds appear in the tree. Containers (workspace / project /
	/// folder / file / module) are NOT filterable — they're navigational
	/// scaffolding. Filtering replaces the legend section which was an
	/// after-thought; the chips' colour-coded swatches double as legend.
	#renderFilterChips(mount: HTMLElement): void {
		const filterableKinds: Array<{ kind: ExplorerNodeKind; label: string; cls: string }> = [
			{ kind: 'struct',   label: 'struct',   cls: C.kindType },
			{ kind: 'enum',     label: 'enum',     cls: C.kindType },
			{ kind: 'trait',    label: 'trait',    cls: C.kindType },
			{ kind: 'function', label: 'fn',       cls: C.kindCallable },
			{ kind: 'value',    label: 'const',    cls: C.kindValue },
			{ kind: 'macro',    label: 'macro',    cls: C.kindMacro },
		];
		mount.replaceChildren();
		for (const { kind, label, cls } of filterableKinds) {
			const chip = document.createElement('button');
			chip.type = 'button';
			const active = this.#kindFilter.has(kind);
			chip.textContent = label;
			chip.style.cssText = [
				`background: ${active ? 'var(--sd-selection, #094771)' : 'transparent'}`,
				`border: 1px solid var(--sd-border-subtle, #2d2d2d)`,
				`color: var(--sd-fg, #cccccc)`,
				`font-size: 10px`,
				`padding: 2px 8px`,
				`border-radius: 999px`,
				`cursor: pointer`,
				`display: inline-flex`,
				`align-items: center`,
				`gap: 4px`,
				`font-family: var(--sd-font-mono, ui-monospace, monospace)`,
				active ? 'opacity: 1' : 'opacity: 0.7',
			].join(';');
			// Swatch inline so the chip doubles as the kind legend.
			const swatch = document.createElement('span');
			swatch.className = cls;
			swatch.style.cssText = 'width: 8px; height: 8px; border-radius: 50%; display: inline-block; background: currentColor;';
			chip.prepend(swatch);
			chip.addEventListener('click', () => {
				if (this.#kindFilter.has(kind)) this.#kindFilter.delete(kind);
				else this.#kindFilter.add(kind);
				this.#renderFilterChips(mount);
				this.#renderTree();
			});
			mount.appendChild(chip);
		}
		if (this.#kindFilter.size > 0) {
			const clear = document.createElement('button');
			clear.type = 'button';
			clear.textContent = 'clear';
			clear.style.cssText = [
				`background: transparent`,
				`border: 1px dashed var(--sd-border, #454545)`,
				`color: var(--sd-fg-muted, #9d9d9d)`,
				`font-size: 10px`,
				`padding: 2px 8px`,
				`border-radius: 999px`,
				`cursor: pointer`,
				`font-family: var(--sd-font-mono, ui-monospace, monospace)`,
			].join(';');
			clear.addEventListener('click', () => {
				this.#kindFilter.clear();
				this.#renderFilterChips(mount);
				this.#renderTree();
			});
			mount.appendChild(clear);
		}
	}

	/// Apply the active kind filter to the tree. Symbol leaves with a
	/// filterable kind are dropped when their kind isn't selected;
	/// containers with no surviving descendants disappear so the tree
	/// doesn't expand into empty folders. Returns null when the whole
	/// node + subtree is filtered out.
	#filterTreeNode(node: ExplorerTreeNode): ExplorerTreeNode | null {
		if (this.#kindFilter.size === 0) return node;
		const filterableSymbolKinds: ExplorerNodeKind[] = ['struct', 'enum', 'function', 'trait', 'value', 'macro'];
		const isFilterable = filterableSymbolKinds.includes(node.kind);
		if (isFilterable) {
			return this.#kindFilter.has(node.kind) ? node : null;
		}
		// Container: filter children, drop if empty.
		if (node.children === undefined || node.children.length === 0) {
			// 'unknown' leaves stay (we don't filter on those), but
			// unknown containers without children just pass through.
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

	#wireSearch(): void {
		const n = this.#nodes;
		if (n === null) return;
		n.searchInput.addEventListener('input', () => {
			if (this.#searchDebounceHandle !== null) window.clearTimeout(this.#searchDebounceHandle);
			this.#searchDebounceHandle = window.setTimeout(() => {
				this.#searchDebounceHandle = null;
				const detail: ExplorerSearchDetail = { query: n.searchInput.value };
				this.dispatchEvent(new CustomEvent('sd-explorer-search', {
					detail, bubbles: true, composed: true,
				}));
			}, SEARCH_DEBOUNCE_MS);
		});
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
			const label = document.createElement('span');
			label.className = C.entryLabel;
			label.textContent = ep.label;
			const badge = document.createElement('span');
			badge.className = classigo(C.entryBadge, entryBadgeClass[ep.kind] ?? '');
			badge.textContent = entryBadgeLabel[ep.kind] ?? ep.kind;
			row.appendChild(label);
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
