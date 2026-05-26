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
	SymbolSubItem,
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
	tagKindCallable: s['details__tag--kind-callable'] ?? '',
	tagKindType: s['details__tag--kind-type'] ?? '',
	tagKindValue: s['details__tag--kind-value'] ?? '',
	tagKindModule: s['details__tag--kind-module'] ?? '',
	tagKindMacro: s['details__tag--kind-macro'] ?? '',
	tagVisPublic: s['details__tag--visibility-public'] ?? '',
	tagVisPrivate: s['details__tag--visibility-private'] ?? '',
	tagVisCrate: s['details__tag--visibility-crate'] ?? '',
	tagVisProtected: s['details__tag--visibility-protected'] ?? '',
	location: s.details__location ?? '',
	locationPath: s['details__location-path'] ?? '',
	tabs: s.details__tabs ?? '',
	tab: s.details__tab ?? '',
	tabActive: s['details__tab--active'] ?? '',
	body: s.details__body ?? '',
	empty: s.details__empty ?? '',
	section: s.details__section ?? '',
	sectionTitle: s['details__section-title'] ?? '',
	sectionTitleDoc: s['details__section-title--doc'] ?? '',
	sectionTitleRelations: s['details__section-title--relations'] ?? '',
	sectionTitleEntry: s['details__section-title--entry'] ?? '',
	sectionTitleFields: s['details__section-title--fields'] ?? '',
	sectionTitleMethods: s['details__section-title--methods'] ?? '',
	sectionTitleSource: s['details__section-title--source'] ?? '',
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
	relationItemSignature: s['details__relation-item-signature'] ?? '',
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
	imports: 'Imports',
	importedBy: 'Imported by',
	testedBy: 'Tested by',
	implements: 'Implements',
	extends: 'Extends',
	definedHere: 'Defined here',
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

const KIND_CALLABLE = new Set(['function', 'fn', 'method', 'impl_fn', 'trait_fn', 'interface_method', 'getter', 'setter', 'constructor']);
const KIND_TYPE = new Set(['struct', 'enum', 'class', 'interface', 'trait', 'type_alias', 'union']);
const KIND_VALUE = new Set(['const', 'static', 'let', 'var', 'field', 'enum_variant', 'property', 'interface_property']);
const KIND_MODULE = new Set(['module', 'namespace', 'package', 'crate']);
const KIND_MACRO = new Set(['macro', 'macro_rules', 'proc_macro', 'decorator', 'declarativemacro', 'procmacro']);

function kindFamilyTagClass(kindLabel: string): string {
	const k = kindLabel.toLowerCase();
	if (KIND_CALLABLE.has(k)) return C.tagKindCallable;
	if (KIND_TYPE.has(k)) return C.tagKindType;
	if (KIND_VALUE.has(k)) return C.tagKindValue;
	if (KIND_MODULE.has(k)) return C.tagKindModule;
	if (KIND_MACRO.has(k)) return C.tagKindMacro;
	return '';
}

function visibilityTagClass(visibility: string): string {
	switch (visibility.toLowerCase()) {
		case 'public': return C.tagVisPublic;
		case 'private': return C.tagVisPrivate;
		case 'crate': return C.tagVisCrate;
		case 'protected': return C.tagVisProtected;
		default: return '';
	}
}

export class SymbolDetailsElement extends HTMLElement {
	#mounted = false;
	#symbol: SymbolDetail | null = null;
	#sourceBody: string | null = null;
	#sourceLoading = false;
	#tab: SymbolDetailsTab = 'overview';
	#docExpanded = false;
	#expandedBuckets = new Set<SymbolRelationKind>();
	#bucketSortBy = new Map<SymbolRelationKind, 'default' | 'name' | 'kind'>();
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
		// Reset per-bucket expand state — keeping it would cross-pollute
		// between symbols (e.g. user expands Used by on A, switches to
		// B which also has Used by → B opens expanded by surprise).
		this.#expandedBuckets.clear();
		this.#bucketSortBy.clear();
		this.#refresh();
		// If the user was already on the Source tab when the symbol
		// changed, the cache is now null and the body shows the empty
		// placeholder. Re-emit the tab-change so the host's existing
		// source-fetch listener fires for the new symbol without the
		// user having to manually re-click the tab.
		if (this.#tab === 'source' && next !== null) {
			this.dispatchEvent(new CustomEvent<SymbolDetailsTabChangeDetail>('sd-symbol-tab-change', {
				detail: { tab: 'source' }, bubbles: true, composed: true,
			}));
		}
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

		const tagDescs: Array<{ text: string; extraClass: string }> = [];
		if (sym.kindLabel) tagDescs.push({ text: sym.kindLabel, extraClass: kindFamilyTagClass(sym.kindLabel) });
		if (sym.visibility) tagDescs.push({ text: sym.visibility, extraClass: visibilityTagClass(sym.visibility) });
		const tagsHtml = tagDescs.length === 0
			? ''
			: `<div class="${C.tags}">${tagDescs.map(d => `<span class="${classigo(C.tag, d.extraClass)}">${escapeHtml(d.text)}</span>`).join('')}</div>`;

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
		const fieldCount = sym?.fields.length ?? 0;
		const methodCount = sym?.methods.length ?? 0;
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
				this.#renderSubItems(n.body, sym.fields, 'fields');
				break;
			case 'methods':
				this.#renderSubItems(n.body, sym.methods, 'methods');
				break;
			case 'source':
				this.#renderSource(n.body, sym);
				break;
		}
	}

	#renderSubItems(
		mount: HTMLElement,
		items: ReadonlyArray<SymbolSubItem>,
		kind: 'fields' | 'methods',
	): void {
		mount.innerHTML = '';
		if (items.length === 0) {
			const empty = document.createElement('div');
			empty.className = C.empty;
			empty.textContent = kind === 'fields'
				? 'No fields or variants on this symbol.'
				: 'No methods on this symbol.';
			mount.appendChild(empty);
			return;
		}
		const section = document.createElement('section');
		section.className = C.section;
		const ul = document.createElement('ul');
		ul.className = C.relationList;
		for (const it of items) {
			const li = document.createElement('li');
			li.className = C.relationItem;
			li.title = it.fqdn;
			const label = document.createElement('span');
			label.className = C.relationItemLabel;
			const nameSpan = document.createElement('span');
			nameSpan.textContent = it.name;
			label.appendChild(nameSpan);
			if (it.signature) {
				const sig = document.createElement('span');
				sig.className = C.relationItemSignature;
				sig.textContent = it.signature;
				label.appendChild(sig);
			}
			const k = document.createElement('span');
			k.className = C.relationItemKind;
			k.textContent = it.kindLabel;
			li.appendChild(label);
			li.appendChild(k);
			li.addEventListener('click', () => {
				focusStore.setFocus(it.fqdn);
				this.dispatchEvent(new CustomEvent<SymbolDetailsRelationClickDetail>('sd-symbol-relation-click', {
					detail: { fqdn: it.fqdn, relationKind: 'definedHere' },
					bubbles: true, composed: true,
				}));
			});
			ul.appendChild(li);
		}
		section.appendChild(ul);
		mount.appendChild(section);
	}

	#renderSource(mount: HTMLElement, sym: SymbolDetail): void {
		mount.innerHTML = '';
		const header = document.createElement('div');
		header.className = C.section;
		const title = document.createElement('div');
		title.className = classigo(C.sectionTitle, C.sectionTitleSource);
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
		// Light-weight syntax-highlighter — single-pass regex tokenizer
		// keyed on the file extension. Not a full lexer, just enough to
		// give the Source preview the typography-first feel the manifesto
		// asks for. Falls back to plain text when the language is
		// unknown.
		pre.innerHTML = highlightSource(this.#sourceBody, sym.file);
		mount.appendChild(pre);
	}

	#renderOverview(mount: HTMLElement, sym: SymbolDetail): void {
		mount.innerHTML = '';

		// Documentation section
		if (sym.documentation && sym.documentation.length > 0) {
			const docSection = document.createElement('section');
			docSection.className = C.section;
			const title = document.createElement('div');
			title.className = classigo(C.sectionTitle, C.sectionTitleDoc);
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
			relTitle.className = classigo(C.sectionTitle, C.sectionTitleRelations);
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
			epTitle.className = classigo(C.sectionTitle, C.sectionTitleEntry);
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

	#renderRelation(bucket: SymbolRelationBucket, _ownFqdn: string): HTMLElement {
		const wrap = document.createElement('div');
		wrap.className = C.relation;
		const expanded = this.#expandedBuckets.has(bucket.kind);
		const sortBy = this.#bucketSortBy.get(bucket.kind) ?? 'default';

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
		// Sort toggle — cycles default → name → kind → default. Compact
		// "↕ <mode>" button so the inspector stays "IDE-like" instead of
		// a static dump.
		const sortBtn = document.createElement('button');
		sortBtn.type = 'button';
		sortBtn.className = C.seeAll;
		const sortLabel = sortBy === 'default' ? '↕ order' : sortBy === 'name' ? '↕ name' : '↕ kind';
		sortBtn.textContent = sortLabel;
		sortBtn.title = 'Cycle sort: default → name → kind';
		sortBtn.addEventListener('click', () => {
			const next: 'default' | 'name' | 'kind' = sortBy === 'default' ? 'name' : sortBy === 'name' ? 'kind' : 'default';
			if (next === 'default') this.#bucketSortBy.delete(bucket.kind);
			else this.#bucketSortBy.set(bucket.kind, next);
			this.#renderBody();
		});
		header.appendChild(sortBtn);
		if (bucket.total > TOP_PER_RELATION) {
			// Inline expand — no spawnable drawer, just toggle which slice
			// of the bucket we render. Resets on symbol swap to avoid
			// cross-pollution.
			const toggle = document.createElement('button');
			toggle.type = 'button';
			toggle.className = C.seeAll;
			toggle.textContent = expanded ? 'Show less' : `See all (${bucket.total}) ›`;
			toggle.addEventListener('click', () => {
				if (this.#expandedBuckets.has(bucket.kind)) this.#expandedBuckets.delete(bucket.kind);
				else this.#expandedBuckets.add(bucket.kind);
				this.#renderBody();
			});
			header.appendChild(toggle);
		}
		wrap.appendChild(header);

		const ul = document.createElement('ul');
		ul.className = C.relationList;
		// Apply sort BEFORE the visible-slice cap so the slice picks up
		// the right top-5 after sort. Default keeps server order.
		const sortedItems = sortBy === 'default'
			? bucket.items
			: [...bucket.items].sort((a, b) => sortBy === 'name'
				? a.label.localeCompare(b.label)
				: a.kindLabel.localeCompare(b.kindLabel) || a.label.localeCompare(b.label));
		const visible = expanded ? sortedItems : sortedItems.slice(0, TOP_PER_RELATION);
		for (const item of visible) {
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
		// Two wired actions only. The four manifesto extras (show callers /
		// deps graphs, isolate subgraph, expand neighborhood) were stubs
		// with no concrete behavior — removed pending real implementation.
		const acts: Array<{ id: SymbolDetailsAction; label: string }> = [
			{ id: 'open-in-editor', label: 'Open in editor' },
			{ id: 'add-to-compare', label: 'Add to compare' },
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

type HighlightLang = 'rust' | 'ts' | null;

const RUST_KEYWORDS = new Set([
	'fn', 'struct', 'enum', 'trait', 'impl', 'let', 'mut', 'const', 'static',
	'pub', 'use', 'mod', 'return', 'if', 'else', 'for', 'while', 'loop', 'match',
	'where', 'async', 'await', 'move', 'self', 'Self', 'true', 'false', 'as',
	'in', 'ref', 'unsafe', 'extern', 'type', 'dyn', 'crate', 'super', 'break',
	'continue', 'box', 'macro_rules',
]);

const TS_KEYWORDS = new Set([
	'function', 'class', 'interface', 'type', 'enum', 'const', 'let', 'var',
	'if', 'else', 'for', 'while', 'do', 'switch', 'case', 'default', 'break',
	'continue', 'return', 'throw', 'try', 'catch', 'finally', 'new', 'this',
	'super', 'extends', 'implements', 'export', 'import', 'from', 'as', 'in',
	'of', 'async', 'await', 'yield', 'true', 'false', 'null', 'undefined',
	'void', 'never', 'any', 'unknown', 'string', 'number', 'boolean', 'object',
	'symbol', 'bigint', 'public', 'private', 'protected', 'readonly', 'static',
	'abstract', 'override', 'declare',
]);

function detectLang(file: string): HighlightLang {
	const f = file.toLowerCase();
	if (f.endsWith('.rs')) return 'rust';
	if (f.endsWith('.ts') || f.endsWith('.tsx') || f.endsWith('.js') || f.endsWith('.jsx') || f.endsWith('.mts') || f.endsWith('.cts')) return 'ts';
	return null;
}

/**
 * Single-pass syntax highlighter producing an HTML string with `<span>`
 * wrappers around tokens. Comments / strings / numbers / keywords / types
 * each get a CSS variable hook from the existing kind palette so the
 * highlighting blends with the rest of the shell rather than introducing
 * a new colour scheme.
 *
 * Trade-offs (V0):
 *   - Regex tokeniser, not a real lexer — fine for read-only previews,
 *     would mis-tokenise pathological cases (nested template literals,
 *     escaped quotes spanning lines) but those rarely appear in symbol
 *     bodies.
 *   - Two languages only: Rust + TS family. Unknown extensions render
 *     as plain escaped text.
 */
function highlightSource(code: string, file: string): string {
	const lang = detectLang(file);
	if (lang === null) return escapeHtml(code);
	const keywords = lang === 'rust' ? RUST_KEYWORDS : TS_KEYWORDS;
	// Order matters in the alternation: comments + strings must win
	// over keywords/identifiers since e.g. `// fn foo` should stay all-
	// comment, not partly-keyword.
	const re = /(\/\/[^\n]*|\/\*[\s\S]*?\*\/|"(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|`(?:[^`\\]|\\.)*`|\b\d+(?:\.\d+)?(?:[eE][-+]?\d+)?\b|\b[A-Z][a-zA-Z0-9_]*\b|\b[a-zA-Z_][a-zA-Z0-9_]*\b)/g;
	let out = '';
	let last = 0;
	let m: RegExpExecArray | null;
	while ((m = re.exec(code)) !== null) {
		const tok = m[0];
		const start = m.index;
		if (start > last) out += escapeHtml(code.slice(last, start));
		const cls = classifyToken(tok, keywords);
		if (cls === null) out += escapeHtml(tok);
		else out += `<span style="color: var(${cls})">${escapeHtml(tok)}</span>`;
		last = start + tok.length;
	}
	if (last < code.length) out += escapeHtml(code.slice(last));
	return out;
}

function classifyToken(tok: string, keywords: Set<string>): string | null {
	if (tok.startsWith('//') || tok.startsWith('/*')) return '--sd-fg-muted';
	if (tok.startsWith('"') || tok.startsWith("'") || tok.startsWith('`')) return '--sd-status-ok';
	if (/^\d/.test(tok)) return '--sd-kind-value';
	if (keywords.has(tok)) return '--sd-kind-callable';
	if (/^[A-Z]/.test(tok)) return '--sd-kind-type';
	return null;
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
