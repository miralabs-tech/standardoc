/**
 * `<standardoc-symbol-details>` — right-rail panel showing the rich
 * profile of the currently focused symbol. Layout (top-to-bottom):
 *
 *   ┌──────────────────────────────────────────────┐
 *   │ name                            ★  ⋮         │  head (sticky-ish)
 *   │ struct  public                               │
 *   │ file/path.rs:42                    ⧉  ⤴      │
 *   ├──────────────────────────────────────────────┤
 *   │ Overview │ Fields (10) │ Methods (2) │ Source│  tabs
 *   ├──────────────────────────────────────────────┤
 *   │ Documentation                                │  body (scrolls)
 *   │   ...                                        │
 *   │ Relations                                    │
 *   │   Used by (3)                          See > │
 *   │   • reader::read_chunk            fn         │
 *   │   ...                                        │
 *   │ Entry Point Kind: public_api                 │
 *   ├──────────────────────────────────────────────┤
 *   │ Open in Editor       Show Callers Graph      │  action footer
 *   │ Add to Compare       Show Callees Graph      │
 *   └──────────────────────────────────────────────┘
 *
 * Data is injected via the `symbol` property setter — host calls
 * `element.symbol = await mcp.getContext(fqdn)`-derived `SymbolDetail`.
 * The element subscribes to `focusStore` so it can show the focus FQDN
 * in an empty-state header while the host's fetch is in flight; once
 * `symbol` arrives the full profile renders.
 *
 * Tabs other than Overview (Fields / Methods / Source) are stubbed —
 * the bodies render a placeholder pointing to the relevant follow-up
 * phase. The tab headers respect counts injected via `SymbolDetail`
 * (e.g. "Fields (10)") so the chrome already feels alive.
 *
 * Events emitted (all bubble + composed):
 *   - `sd-symbol-action`         detail: SymbolDetailsActionDetail
 *   - `sd-symbol-tab-change`     detail: SymbolDetailsTabChangeDetail
 *   - `sd-symbol-relation-click` detail: SymbolDetailsRelationClickDetail
 *
 * Relation-click also calls `focusStore.setFocus(target)` so clicking
 * a "Used by" entry shifts the global focus, which the upstream
 * Explorer / Focus Graph panels then react to on their own.
 */

import classigo from 'classigo';

import { focusStore, type FocusState } from '../../focus-store';
import type { EntryPointKind } from '../explorer/explorer.type';
import type {
	SymbolDetail,
	SymbolDetailsAction,
	SymbolDetailsActionDetail,
	SymbolDetailsRelationClickDetail,
	SymbolDetailsTab,
	SymbolDetailsTabChangeDetail,
	SymbolRelationBucket,
	SymbolRelationKind,
} from './symbol-details.type';
import s from './symbol-details.module.scss';

export const STANDARDOC_SYMBOL_DETAILS_TAG = 'standardoc-symbol-details';

const C = {
	details: s.details ?? '',
	headerBar: s['details__header-bar'] ?? '',
	head: s.details__head ?? '',
	nameRow: s['details__name-row'] ?? '',
	name: s.details__name ?? '',
	iconBtn: s['details__icon-btn'] ?? '',
	tags: s.details__tags ?? '',
	tag: s.details__tag ?? '',
	location: s.details__location ?? '',
	locationPath: s['details__location-path'] ?? '',
	tabs: s.details__tabs ?? '',
	tab: s.details__tab ?? '',
	tabActive: s['details__tab--active'] ?? '',
	body: s.details__body ?? '',
	empty: s.details__empty ?? '',
	section: s.details__section ?? '',
	sectionTitle: s['details__section-title'] ?? '',
	doc: s.details__doc ?? '',
	docToggle: s['details__doc-toggle'] ?? '',
	relation: s.details__relation ?? '',
	relationHeader: s['details__relation-header'] ?? '',
	relationTitle: s['details__relation-title'] ?? '',
	relationCount: s['details__relation-count'] ?? '',
	relationSpacer: s['details__relation-spacer'] ?? '',
	seeAll: s['details__see-all'] ?? '',
	relationList: s['details__relation-list'] ?? '',
	relationItem: s['details__relation-item'] ?? '',
	relationItemLabel: s['details__relation-item-label'] ?? '',
	relationItemKind: s['details__relation-item-kind'] ?? '',
	entryPoint: s['details__entry-point'] ?? '',
	entryPointBadge: s['details__entry-point-badge'] ?? '',
	entryPointBadgeBinMain: s['details__entry-point-badge--binary-main'] ?? '',
	entryPointBadgePublicApi: s['details__entry-point-badge--public-api'] ?? '',
	entryPointBadgeFfiExport: s['details__entry-point-badge--ffi-export'] ?? '',
	actions: s.details__actions ?? '',
	action: s.details__action ?? '',
} as const;

const RELATION_TITLE: Record<SymbolRelationKind, string> = {
	usedBy: 'Used by',
	usesTypes: 'Uses types',
	calls: 'Calls',
	importedBy: 'Imported by',
	testedBy: 'Tested by',
	implements: 'Implements',
	extends: 'Extends',
};

const TOP_PER_RELATION = 5;
const DOC_COLLAPSED_CHARS = 180;

const entryPointBadgeClass: Record<EntryPointKind, string> = {
	binary_main: C.entryPointBadgeBinMain,
	public_api: C.entryPointBadgePublicApi,
	ffi_export: C.entryPointBadgeFfiExport,
};

function shortFqdn(fqdn: string): string {
	const idx = fqdn.lastIndexOf('::');
	return idx >= 0 ? fqdn.slice(idx + 2) : fqdn;
}

export class SymbolDetailsElement extends HTMLElement {
	#mounted = false;
	#symbol: SymbolDetail | null = null;
	#sourceBody: string | null = null;
	#sourceLoading = false;
	#tab: SymbolDetailsTab = 'overview';
	#docExpanded = false;
	#unsubscribeFocus: (() => void) | null = null;
	#focus: FocusState = focusStore.get();

	#nodes: {
		root: HTMLElement;
		head: HTMLElement;
		tabs: HTMLElement;
		body: HTMLElement;
		actions: HTMLElement;
	} | null = null;

	set symbol(next: SymbolDetail | null) {
		this.#symbol = next;
		if (next !== null) this.#docExpanded = false;
		// Symbol change invalidates the Source-tab cache so the host
		// doesn't surface a stale body while a fresh fetch is in flight.
		this.#sourceBody = null;
		this.#sourceLoading = false;
		this.#refresh();
	}

	get symbol(): SymbolDetail | null {
		return this.#symbol;
	}

	set sourceBody(next: string | null) {
		this.#sourceBody = next;
		this.#sourceLoading = false;
		if (this.#tab === 'source') this.#renderBody();
	}

	set sourceLoading(next: boolean) {
		if (next === this.#sourceLoading) return;
		this.#sourceLoading = next;
		if (this.#tab === 'source') this.#renderBody();
	}

	set tab(next: SymbolDetailsTab) {
		if (next === this.#tab) return;
		this.#tab = next;
		this.#refresh();
	}

	connectedCallback(): void {
		if (this.#mounted) return;
		this.#mounted = true;
		this.#render();
		this.#unsubscribeFocus = focusStore.subscribe(state => {
			this.#focus = state;
			// Only re-render the head; the symbol body keeps whatever the
			// host injected. Clearing it would race the host's fetch.
			this.#renderHead();
		});
	}

	disconnectedCallback(): void {
		this.#unsubscribeFocus?.();
		this.#unsubscribeFocus = null;
	}

	#render(): void {
		const root = document.createElement('div');
		root.className = C.details;
		root.innerHTML = `
			<div class="${C.headerBar}">Symbol Details</div>
			<div class="${C.head}" data-role="head"></div>
			<div class="${C.tabs}" data-role="tabs"></div>
			<div class="${C.body}" data-role="body"></div>
			<div class="${C.actions}" data-role="actions"></div>
		`;
		this.replaceChildren(root);

		this.#nodes = {
			root,
			head: root.querySelector<HTMLElement>('[data-role="head"]')!,
			tabs: root.querySelector<HTMLElement>('[data-role="tabs"]')!,
			body: root.querySelector<HTMLElement>('[data-role="body"]')!,
			actions: root.querySelector<HTMLElement>('[data-role="actions"]')!,
		};

		this.#refresh();
	}

	#refresh(): void {
		this.#renderHead();
		this.#renderTabs();
		this.#renderBody();
		this.#renderActions();
	}

	#renderHead(): void {
		const n = this.#nodes;
		if (n === null) return;
		const sym = this.#symbol;

		if (sym === null) {
			const fallbackFqdn = this.#focus.current;
			n.head.innerHTML = fallbackFqdn
				? `<div class="${C.nameRow}"><span class="${C.name}">${escapeHtml(shortFqdn(fallbackFqdn))}</span></div>
				   <div class="${C.location}"><span class="${C.locationPath}">${escapeHtml(fallbackFqdn)}</span></div>`
				: `<div class="${C.empty}">Click a symbol to inspect.</div>`;
			return;
		}

		const tags: string[] = [];
		if (sym.kindLabel) tags.push(sym.kindLabel);
		if (sym.visibility) tags.push(sym.visibility);
		const tagsHtml = tags.length === 0
			? ''
			: `<div class="${C.tags}">${tags.map(t => `<span class="${C.tag}">${escapeHtml(t)}</span>`).join('')}</div>`;

		n.head.innerHTML = `
			<div class="${C.nameRow}">
				<span class="${C.name}" title="${escapeHtml(sym.fqdn)}">${escapeHtml(sym.name)}</span>
				<button type="button" class="${C.iconBtn}" data-role="copy-fqdn" title="Copy FQDN">⧉</button>
				<button type="button" class="${C.iconBtn}" data-role="open-editor" title="Open in editor">⤴</button>
			</div>
			${tagsHtml}
			<div class="${C.location}">
				<span class="${C.locationPath}" title="${escapeHtml(sym.file)}:${sym.startLine}">${escapeHtml(sym.file)}:${sym.startLine}</span>
			</div>
		`;
		n.head.querySelector<HTMLButtonElement>('[data-role="copy-fqdn"]')?.addEventListener('click', () => {
			this.#emitAction('copy-fqdn', sym.fqdn);
		});
		n.head.querySelector<HTMLButtonElement>('[data-role="open-editor"]')?.addEventListener('click', () => {
			this.#emitAction('open-in-editor', sym.fqdn);
		});
	}

	#renderTabs(): void {
		const n = this.#nodes;
		if (n === null) return;
		const sym = this.#symbol;
		const fieldCount = sym?.fieldCount ?? 0;
		const methodCount = sym?.methodCount ?? 0;
		const disabled = sym === null;

		const tabs: Array<{ id: SymbolDetailsTab; label: string; disabled: boolean }> = [
			{ id: 'overview', label: 'Overview', disabled },
			{ id: 'fields', label: `Fields${fieldCount > 0 ? ` (${fieldCount})` : ''}`, disabled },
			{ id: 'methods', label: `Methods${methodCount > 0 ? ` (${methodCount})` : ''}`, disabled },
			{ id: 'source', label: 'Source', disabled },
		];

		n.tabs.innerHTML = tabs.map(t => {
			const cls = classigo(C.tab, t.id === this.#tab && C.tabActive);
			const disAttr = t.disabled ? 'disabled' : '';
			return `<button type="button" class="${cls}" data-tab="${t.id}" ${disAttr}>${escapeHtml(t.label)}</button>`;
		}).join('');

		n.tabs.querySelectorAll<HTMLButtonElement>('[data-tab]').forEach(btn => {
			btn.addEventListener('click', () => {
				const next = btn.dataset.tab as SymbolDetailsTab | undefined;
				if (!next) return;
				this.tab = next;
				this.dispatchEvent(new CustomEvent<SymbolDetailsTabChangeDetail>('sd-symbol-tab-change', {
					detail: { tab: next }, bubbles: true, composed: true,
				}));
			});
		});
	}

	#renderBody(): void {
		const n = this.#nodes;
		if (n === null) return;
		const sym = this.#symbol;

		if (sym === null) {
			n.body.innerHTML = `<div class="${C.empty}">No symbol selected.</div>`;
			return;
		}

		switch (this.#tab) {
			case 'overview':
				this.#renderOverview(n.body, sym);
				break;
			case 'fields':
				n.body.innerHTML = `<div class="${C.empty}">Fields tab — coming in Phase 4 (Field Details panel).</div>`;
				break;
			case 'methods':
				n.body.innerHTML = `<div class="${C.empty}">Methods tab — coming in Phase 4.</div>`;
				break;
			case 'source':
				this.#renderSource(n.body, sym);
				break;
		}
	}

	#renderSource(mount: HTMLElement, sym: SymbolDetail): void {
		mount.innerHTML = '';
		const header = document.createElement('div');
		header.className = C.section;
		const title = document.createElement('div');
		title.className = C.sectionTitle;
		title.textContent = `${sym.file}:${sym.startLine}`;
		header.appendChild(title);
		mount.appendChild(header);

		if (this.#sourceLoading) {
			const loading = document.createElement('div');
			loading.className = C.empty;
			loading.textContent = 'Loading source…';
			mount.appendChild(loading);
			return;
		}
		if (this.#sourceBody === null) {
			const empty = document.createElement('div');
			empty.className = C.empty;
			empty.textContent = 'No source loaded — host should fetch via getBody.';
			mount.appendChild(empty);
			return;
		}
		const pre = document.createElement('pre');
		pre.style.margin = '0';
		pre.style.padding = 'var(--sd-space-3, 12px)';
		pre.style.background = 'var(--sd-bg-elevated, #252526)';
		pre.style.borderRadius = 'var(--sd-radius-md, 6px)';
		pre.style.fontFamily = 'var(--sd-font-mono, ui-monospace, monospace)';
		pre.style.fontSize = 'var(--sd-text-sm, 11px)';
		pre.style.color = 'var(--sd-fg, #cccccc)';
		pre.style.overflow = 'auto';
		pre.style.whiteSpace = 'pre';
		pre.textContent = this.#sourceBody;
		mount.appendChild(pre);
	}

	#renderOverview(mount: HTMLElement, sym: SymbolDetail): void {
		mount.innerHTML = '';

		// Documentation section
		if (sym.documentation && sym.documentation.length > 0) {
			const docSection = document.createElement('section');
			docSection.className = C.section;
			const title = document.createElement('div');
			title.className = C.sectionTitle;
			title.textContent = 'Documentation';
			const doc = document.createElement('div');
			doc.className = C.doc;
			const truncated = sym.documentation.length > DOC_COLLAPSED_CHARS;
			doc.textContent = !truncated || this.#docExpanded
				? sym.documentation
				: `${sym.documentation.slice(0, DOC_COLLAPSED_CHARS)}…`;
			docSection.appendChild(title);
			docSection.appendChild(doc);
			if (truncated) {
				const toggle = document.createElement('button');
				toggle.type = 'button';
				toggle.className = C.docToggle;
				toggle.textContent = this.#docExpanded ? 'Show less' : 'Show more';
				toggle.addEventListener('click', () => {
					this.#docExpanded = !this.#docExpanded;
					this.#renderBody();
				});
				docSection.appendChild(toggle);
			}
			mount.appendChild(docSection);
		}

		// Relations section
		if (sym.relations.length > 0) {
			const relSection = document.createElement('section');
			relSection.className = C.section;
			const relTitle = document.createElement('div');
			relTitle.className = C.sectionTitle;
			relTitle.textContent = 'Relations';
			relSection.appendChild(relTitle);
			for (const bucket of sym.relations) {
				relSection.appendChild(this.#renderRelation(bucket, sym.fqdn));
			}
			mount.appendChild(relSection);
		}

		// Entry Point Kind
		if (sym.entryPointKind !== null) {
			const epSection = document.createElement('section');
			epSection.className = C.section;
			const epTitle = document.createElement('div');
			epTitle.className = C.sectionTitle;
			epTitle.textContent = 'Entry point';
			const row = document.createElement('div');
			row.className = C.entryPoint;
			const label = document.createElement('span');
			label.textContent = 'Kind:';
			const badge = document.createElement('span');
			badge.className = classigo(C.entryPointBadge, entryPointBadgeClass[sym.entryPointKind] ?? '');
			badge.textContent = sym.entryPointKind;
			row.appendChild(label);
			row.appendChild(badge);
			epSection.appendChild(epTitle);
			epSection.appendChild(row);
			mount.appendChild(epSection);
		}
	}

	#renderRelation(bucket: SymbolRelationBucket, ownFqdn: string): HTMLElement {
		const wrap = document.createElement('div');
		wrap.className = C.relation;

		const header = document.createElement('div');
		header.className = C.relationHeader;
		const title = document.createElement('span');
		title.className = C.relationTitle;
		title.textContent = RELATION_TITLE[bucket.kind] ?? bucket.kind;
		const count = document.createElement('span');
		count.className = C.relationCount;
		count.textContent = `${bucket.total}`;
		const spacer = document.createElement('span');
		spacer.className = C.relationSpacer;
		header.appendChild(title);
		header.appendChild(count);
		header.appendChild(spacer);
		if (bucket.total > TOP_PER_RELATION) {
			const seeAll = document.createElement('button');
			seeAll.type = 'button';
			seeAll.className = C.seeAll;
			seeAll.textContent = 'See all ›';
			seeAll.addEventListener('click', () => {
				this.dispatchEvent(new CustomEvent<SymbolDetailsActionDetail>('sd-symbol-action', {
					detail: { action: 'see-all', fqdn: ownFqdn, relationKind: bucket.kind },
					bubbles: true, composed: true,
				}));
			});
			header.appendChild(seeAll);
		}
		wrap.appendChild(header);

		const ul = document.createElement('ul');
		ul.className = C.relationList;
		for (const item of bucket.items.slice(0, TOP_PER_RELATION)) {
			const li = document.createElement('li');
			li.className = C.relationItem;
			li.title = item.fqdn;
			const labelEl = document.createElement('span');
			labelEl.className = C.relationItemLabel;
			labelEl.textContent = item.label;
			const kindEl = document.createElement('span');
			kindEl.className = C.relationItemKind;
			kindEl.textContent = item.kindLabel;
			li.appendChild(labelEl);
			li.appendChild(kindEl);
			li.addEventListener('click', () => {
				focusStore.setFocus(item.fqdn);
				this.dispatchEvent(new CustomEvent<SymbolDetailsRelationClickDetail>('sd-symbol-relation-click', {
					detail: { fqdn: item.fqdn, relationKind: bucket.kind },
					bubbles: true, composed: true,
				}));
			});
			ul.appendChild(li);
		}
		wrap.appendChild(ul);
		return wrap;
	}

	#renderActions(): void {
		const n = this.#nodes;
		if (n === null) return;
		const sym = this.#symbol;
		if (sym === null) {
			n.actions.innerHTML = '';
			return;
		}
		const acts: Array<{ id: SymbolDetailsAction; label: string }> = [
			{ id: 'open-in-editor', label: 'Open in editor' },
			{ id: 'show-callers', label: 'Show callers graph' },
			{ id: 'add-to-compare', label: 'Add to compare' },
			{ id: 'show-callees', label: 'Show callees graph' },
		];
		n.actions.innerHTML = acts.map(a =>
			`<button type="button" class="${C.action}" data-action="${a.id}">${escapeHtml(a.label)}</button>`,
		).join('');
		n.actions.querySelectorAll<HTMLButtonElement>('[data-action]').forEach(btn => {
			const action = btn.dataset.action as SymbolDetailsAction | undefined;
			if (!action) return;
			btn.addEventListener('click', () => this.#emitAction(action, sym.fqdn));
		});
	}

	#emitAction(action: SymbolDetailsAction, fqdn: string): void {
		this.dispatchEvent(new CustomEvent<SymbolDetailsActionDetail>('sd-symbol-action', {
			detail: { action, fqdn },
			bubbles: true, composed: true,
		}));
	}
}

function escapeHtml(text: string): string {
	return text
		.replace(/&/g, '&amp;')
		.replace(/</g, '&lt;')
		.replace(/>/g, '&gt;')
		.replace(/"/g, '&quot;');
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_SYMBOL_DETAILS_TAG)) {
	customElements.define(STANDARDOC_SYMBOL_DETAILS_TAG, SymbolDetailsElement);
}

declare global {
	interface HTMLElementTagNameMap {
		[STANDARDOC_SYMBOL_DETAILS_TAG]: SymbolDetailsElement;
	}
}
