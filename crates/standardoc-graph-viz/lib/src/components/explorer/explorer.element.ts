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
	legend: s.explorer__legend ?? '',
	legendRow: s['explorer__legend-row'] ?? '',
	legendSwatch: s['explorer__legend-swatch'] ?? '',
	legendEdge: s['explorer__legend-edge'] ?? '',
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

export class ExplorerElement extends HTMLElement {
	#mounted = false;
	#tree: ReadonlyArray<ExplorerTreeNode> = [];
	#entryPoints: ReadonlyArray<ExplorerEntryPoint> = [];
	#expanded = new Set<string>();
	#searchDebounceHandle: number | null = null;
	#unsubscribeFocus: (() => void) | null = null;
	#focus: FocusState = focusStore.get();

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

	connectedCallback(): void {
		if (this.#mounted) return;
		this.#mounted = true;
		this.#render();
		this.#unsubscribeFocus = focusStore.subscribe(state => {
			this.#focus = state;
			this.#renderRecents();
			this.#renderTree();
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
				<section class="${C.section}">
					<div class="${C.sectionTitle}">Legend</div>
					<div class="${C.legend}" data-role="legend-mount"></div>
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
		this.#renderTree();
		this.#renderEntryPoints();
		this.#renderRecents();
		this.#renderLegend(root.querySelector<HTMLElement>('[data-role="legend-mount"]')!);
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
		for (const node of this.#tree) ul.appendChild(this.#renderNode(node, 0));
		n.treeMount.replaceChildren(ul);
	}

	#renderNode(node: ExplorerTreeNode, depth: number): HTMLLIElement {
		const li = document.createElement('li');
		li.className = C.node;
		const hasChildren = node.children !== undefined && node.children.length > 0;
		const isExpanded = this.#expanded.has(node.id);
		const isSelected = node.fqdn !== null && node.fqdn !== undefined && node.fqdn === this.#focus.current;

		const row = document.createElement('div');
		row.className = classigo(C.nodeRow, isSelected && C.nodeRowSelected);
		row.style.paddingLeft = `${12 + depth * 12}px`;

		const twisty = document.createElement('span');
		twisty.className = C.nodeTwisty;
		twisty.textContent = hasChildren ? (isExpanded ? '▾' : '▸') : '';

		const icon = document.createElement('span');
		icon.className = classigo(C.nodeIcon, this.#iconClassFor(node.kind));

		const label = document.createElement('span');
		label.className = C.nodeLabel;
		label.textContent = node.label;

		row.appendChild(twisty);
		row.appendChild(icon);
		row.appendChild(label);
		row.addEventListener('click', () => {
			if (hasChildren) {
				if (isExpanded) this.#expanded.delete(node.id);
				else this.#expanded.add(node.id);
				this.#renderTree();
			}
			if (node.fqdn) this.#select(node.fqdn, 'tree');
		});

		li.appendChild(row);

		if (hasChildren && isExpanded) {
			const childUl = document.createElement('ul');
			childUl.className = C.nodeChildren;
			for (const child of node.children!) childUl.appendChild(this.#renderNode(child, depth + 1));
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
			row.addEventListener('click', () => this.#select(ep.fqdn, 'entry-points'));
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
		const frag = document.createDocumentFragment();
		for (const fqdn of recent) {
			const row = document.createElement('div');
			row.className = classigo(C.recent, fqdn === this.#focus.current && C.recentCurrent);
			row.textContent = shortFqdn(fqdn);
			row.title = fqdn;
			row.addEventListener('click', () => this.#select(fqdn, 'recents'));
			frag.appendChild(row);
		}
		n.recentsMount.replaceChildren(frag);
	}

	#renderLegend(mount: HTMLElement): void {
		const rows: Array<{ kind: 'swatch' | 'edge'; cls?: string; label: string }> = [
			{ kind: 'swatch', cls: C.kindModule, label: 'Module / Project' },
			{ kind: 'swatch', cls: C.kindType, label: 'Struct / Enum / Trait' },
			{ kind: 'swatch', cls: C.kindCallable, label: 'Function / Method' },
			{ kind: 'swatch', cls: C.kindValue, label: 'Const / Value' },
			{ kind: 'swatch', cls: C.kindMacro, label: 'Macro' },
			{ kind: 'edge', label: 'Edge (calls, uses, imports, tests)' },
		];
		const frag = document.createDocumentFragment();
		for (const r of rows) {
			const row = document.createElement('div');
			row.className = C.legendRow;
			const mark = document.createElement('span');
			if (r.kind === 'swatch') mark.className = classigo(C.legendSwatch, r.cls ?? '');
			else mark.className = C.legendEdge;
			const label = document.createElement('span');
			label.textContent = r.label;
			row.appendChild(mark);
			row.appendChild(label);
			frag.appendChild(row);
		}
		mount.replaceChildren(frag);
	}

	#select(fqdn: string, source: ExplorerSelectDetail['source']): void {
		focusStore.setFocus(fqdn);
		const detail: ExplorerSelectDetail = { fqdn, source };
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
