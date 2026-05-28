/**
 * `<standardoc-symbol-details>` — right-rail panel showing the rich
 * profile of the currently focused symbol. Layout (top-to-bottom):
 *
 *   ┌──────────────────────────────────────────────┐
 *   │ name                            ★  ⋮         │  head (sticky-ish)
 *   │ struct  public                               │
 *   │ file/path.rs:42                    ⧉  ⤴     │
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
import { viewPrefsStore } from '../../view-prefs-store';
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
import {
  C,
  DOC_COLLAPSED_CHARS,
  RELATION_KIND_CLASS,
  RELATION_TITLE,
  STANDARDOC_SYMBOL_DETAILS_TAG,
  TOP_PER_RELATION,
  entryPointBadgeClass,
} from './symbol-details.constants';
import {
  escapeHtml,
  kindFamilyTagClass,
  looksLikeTest,
  shortFqdn,
  visibilityTagClass,
} from './symbol-details.utils';
import { renderMarkdown } from './symbol-details.markdown';
import { highlightSource } from './symbol-details.highlight';

export { STANDARDOC_SYMBOL_DETAILS_TAG };

export class SymbolDetailsElement extends HTMLElement {
  #mounted = false;
  #symbol: SymbolDetail | null = null;
  #sourceBody: string | null = null;
  #sourceLoading = false;
  #tab: SymbolDetailsTab = 'overview';
  #docExpanded = false;
  #excludeTests = viewPrefsStore.get().excludeTests;
  #unsubscribeViewPrefs: (() => void) | null = null;
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
    this.#unsubscribeViewPrefs?.();
    this.#unsubscribeViewPrefs = null;
  }

  #render(): void {
    const root = document.createElement('div');
    root.className = C.details;
    root.innerHTML = `
			<div class="${C.headerBar}">
				<span>Symbol Details</span>
				<button type="button" class="${C.headerToggle}" data-role="hide-tests" title="Hide test-shaped symbols + relations">
					hide tests
				</button>
			</div>
			<div class="${C.head}" data-role="head"></div>
			<div class="${C.tabs}" data-role="tabs"></div>
			<div class="${C.body}" data-role="body"></div>
			<div class="${C.actions}" data-role="actions"></div>
		`;
    this.replaceChildren(root);

    const hideTestsBtn = root.querySelector<HTMLButtonElement>('[data-role="hide-tests"]')!;
    this.#syncHideTestsBtn(hideTestsBtn);
    hideTestsBtn.addEventListener('click', () => {
      // Toggle via the shared store so sibling panels (Focus Graph,
      // future Explorer / Overview wiring) stay in sync. The
      // subscribe() below picks up the change for THIS panel — no
      // need to mutate `#excludeTests` directly here.
      viewPrefsStore.setPrefs({ excludeTests: !viewPrefsStore.get().excludeTests });
    });
    this.#unsubscribeViewPrefs = viewPrefsStore.subscribe(state => {
      if (state.excludeTests === this.#excludeTests) return;
      this.#excludeTests = state.excludeTests;
      this.#syncHideTestsBtn(hideTestsBtn);
      this.#renderBody();
    });

    this.#nodes = {
      root,
      head: root.querySelector<HTMLElement>('[data-role="head"]')!,
      tabs: root.querySelector<HTMLElement>('[data-role="tabs"]')!,
      body: root.querySelector<HTMLElement>('[data-role="body"]')!,
      actions: root.querySelector<HTMLElement>('[data-role="actions"]')!,
    };

    this.#refresh();
  }

  #syncHideTestsBtn(btn: HTMLButtonElement): void {
    btn.className = this.#excludeTests
      ? classigo(C.headerToggle, C.headerToggleActive)
      : C.headerToggle;
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
    // Apply the panel-level "hide tests" toggle to the row set. The
    // fqdn-only check covers `::tests::` modules; the file fallback
    // catches `*_test.rs` / `*.test.ts` / `__tests__/`. Empty-state
    // message stays the same — the user can flip the toggle off if
    // the absence is suspicious.
    const visibleItems = this.#excludeTests
      ? items.filter(it => !looksLikeTest(it.fqdn, it.file))
      : items;
    if (visibleItems.length === 0) {
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
    for (const it of visibleItems) {
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
      li.appendChild(label);
      // Type chip — only on the Fields tab. Surfaces the field's
      // type as a standalone column so `pool: Arc<DbPool>` reads as
      // `pool  Arc<DbPool>  pub  field` instead of the ambiguous
      // `pool: Arc<DbPool>` cramped into the label. Methods skip
      // this — the return type is already visible in `signature`.
      if (kind === 'fields' && it.type) {
        const typeChip = document.createElement('span');
        typeChip.className = classigo(C.subItemChip, C.subItemChipType);
        typeChip.textContent = it.type;
        typeChip.title = it.type;
        li.appendChild(typeChip);
      }
      // Visibility + async chips sit between the label and the
      // kind so they read as "what is this symbol" attributes
      // rather than getting lost at the row edge. Visibility
      // only renders when present (re-exports / builtins skip).
      if (it.visibility) {
        const vis = document.createElement('span');
        vis.className = classigo(C.subItemChip, visibilityTagClass(it.visibility));
        vis.textContent = it.visibility;
        li.appendChild(vis);
      }
      if (it.isAsync) {
        const async = document.createElement('span');
        async.className = classigo(C.subItemChip, C.subItemChipAsync);
        async.textContent = 'async';
        li.appendChild(async);
      }
      const k = document.createElement('span');
      k.className = C.relationItemKind;
      k.textContent = it.kindLabel;
      li.appendChild(k);
      // Hover popup — structured DOM with one row per fact so
      // the FULL signature, kind, and modifier chips don't
      // concatenate into one unreadable line. Uses
      // `position: fixed` so the popup escapes the body's
      // scroll container that clips absolute-positioned children
      // behind the tabs bar.
      const popup = document.createElement('div');
      popup.className = C.itemPopup;
      const popupName = document.createElement('div');
      popupName.className = C.itemPopupName;
      popupName.textContent = it.name;
      popup.appendChild(popupName);
      if (it.signature) {
        const popupSig = document.createElement('div');
        popupSig.className = C.itemPopupSignature;
        popupSig.textContent = it.signature;
        popup.appendChild(popupSig);
      }
      const popupTags = document.createElement('div');
      popupTags.className = C.itemPopupTags;
      const kindTag = document.createElement('span');
      kindTag.className = C.subItemChip;
      kindTag.textContent = it.kindLabel;
      popupTags.appendChild(kindTag);
      if (it.visibility) {
        const visTag = document.createElement('span');
        visTag.className = classigo(C.subItemChip, visibilityTagClass(it.visibility));
        visTag.textContent = it.visibility;
        popupTags.appendChild(visTag);
      }
      if (it.isAsync) {
        const asyncTag = document.createElement('span');
        asyncTag.className = classigo(C.subItemChip, C.subItemChipAsync);
        asyncTag.textContent = 'async';
        popupTags.appendChild(asyncTag);
      }
      popup.appendChild(popupTags);
      li.appendChild(popup);
      li.addEventListener('mouseenter', () => {
        const rect = li.getBoundingClientRect();
        popup.style.left = `${rect.left + rect.width / 2}px`;
        popup.style.top = `${rect.top}px`;
      });
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
      doc.className = classigo(C.doc, C.markdown);
      const truncated = sym.documentation.length > DOC_COLLAPSED_CHARS;
      const raw = !truncated || this.#docExpanded
        ? sym.documentation
        : `${sym.documentation.slice(0, DOC_COLLAPSED_CHARS)}…`;
      doc.innerHTML = renderMarkdown(raw);
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
      // Apply "hide tests" toggle to each bucket's items. Buckets that
      // become empty drop entirely so the user doesn't see ghosts.
      const buckets = this.#excludeTests
        ? sym.relations
            .map(b => ({
              ...b,
              items: b.items.filter(it => !looksLikeTest(it.fqdn)),
            }))
            .filter(b => b.items.length > 0)
        : sym.relations;
      if (buckets.length > 0) {
        const relSection = document.createElement('section');
        relSection.className = C.section;
        const relTitle = document.createElement('div');
        relTitle.className = classigo(C.sectionTitle, C.sectionTitleRelations);
        relTitle.textContent = 'Relations';
        relSection.appendChild(relTitle);
        for (const bucket of buckets) {
          relSection.appendChild(this.#renderRelation(bucket, sym.fqdn));
        }
        mount.appendChild(relSection);
      }
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
    wrap.className = classigo(C.relation, RELATION_KIND_CLASS[bucket.kind]);
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

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_SYMBOL_DETAILS_TAG)) {
  customElements.define(STANDARDOC_SYMBOL_DETAILS_TAG, SymbolDetailsElement);
}

declare global {
  interface HTMLElementTagNameMap {
    [STANDARDOC_SYMBOL_DETAILS_TAG]: SymbolDetailsElement;
  }
}
