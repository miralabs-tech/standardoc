/**
 * `<standardoc-search>` — global symbol-search box with an autocomplete
 * dropdown. Standalone so the host can drop it anywhere (a shell top
 * bar, an Explorer header, a command-palette overlay).
 *
 * Wire contract: the element emits `sd-search-query` (debounced) as
 * the user types and waits for the host to set `results` via the
 * property setter. The host owns the MCP fetch — typically
 * `find_symbols_by_pattern(query)` — so this element is transport-
 * agnostic. Set `loading` true while a fetch is in flight to surface
 * the loading state in the dropdown without flickering between two
 * "empty" renders.
 *
 * Keyboard:
 *   - Cmd/Ctrl+K from anywhere → focus the input (if `globalShortcut`
 *     attribute is present)
 *   - Esc → clear + close dropdown
 *   - ArrowUp / ArrowDown → move active item
 *   - Enter → select active item
 *
 * Click on a result calls `focusStore.setFocus(fqdn)` AND emits
 * `sd-search-select` for hosts that need to react beyond the focus
 * change (e.g. closing a command-palette overlay).
 */

import classigo from 'classigo';

import { focusStore } from '../../focus-store';
import type {
  SearchQueryDetail,
  SearchSelectDetail,
  SymbolSearchResult,
  SymbolSearchSuggestion,
} from './search.type';
import s from './search.module.scss';

export const STANDARDOC_SEARCH_TAG = 'standardoc-search';

const C = {
  search: s.search ?? '',
  input: s.search__input ?? '',
  shortcut: s.search__shortcut ?? '',
  dropdown: s.search__dropdown ?? '',
  dropdownOpen: s['search__dropdown--open'] ?? '',
  status: s.search__status ?? '',
  list: s.search__list ?? '',
  item: s.search__item ?? '',
  itemActive: s['search__item--active'] ?? '',
  itemName: s['search__item-name'] ?? '',
  itemKind: s['search__item-kind'] ?? '',
  itemFqdn: s['search__item-fqdn'] ?? '',
  section: s.search__section ?? '',
  sectionTitle: s['search__section-title'] ?? '',
  tip: s.search__tip ?? '',
} as const;

const QUERY_DEBOUNCE_MS = 150;
const SHORTCUT_LABEL = isMacLike() ? '⌘K' : 'Ctrl+K';

function isMacLike(): boolean {
  if (typeof navigator === 'undefined') return false;
  const p = navigator.platform ?? '';
  const ua = navigator.userAgent ?? '';
  return /Mac|iPhone|iPad/.test(p) || /Mac OS X/.test(ua);
}

export class SearchElement extends HTMLElement {
  static readonly observedAttributes = ['placeholder', 'global-shortcut'] as const;

  #mounted = false;
  #results: ReadonlyArray<SymbolSearchResult> = [];
  #suggestions: ReadonlyArray<SymbolSearchSuggestion> = [];
  #recents: ReadonlyArray<SymbolSearchResult> = [];
  #entryPoints: ReadonlyArray<SymbolSearchResult> = [];
  #loading = false;
  #query = '';
  #open = false;
  #activeIdx = 0;
  #debounceHandle: number | null = null;
  #shortcutHandler: ((e: KeyboardEvent) => void) | null = null;

  #nodes: {
    root: HTMLElement;
    input: HTMLInputElement;
    dropdown: HTMLElement;
  } | null = null;

  set results(next: ReadonlyArray<SymbolSearchResult>) {
    this.#results = next;
    this.#activeIdx = 0;
    this.#renderDropdown();
  }

  /**
   * "Did you mean…" fallback list pushed by the host when a query
   * returns zero direct matches but the daemon surfaced strsim-near
   * suggestions. Rendered as a secondary section in the dropdown.
   */
  set suggestions(next: ReadonlyArray<SymbolSearchSuggestion>) {
    this.#suggestions = next;
    this.#renderDropdown();
  }

  /**
   * Recently focused symbols (host-pushed from focusStore). Surfaces
   * in the empty-state dropdown so users can jump back without
   * retyping.
   */
  set recents(next: ReadonlyArray<SymbolSearchResult>) {
    this.#recents = next;
    if (this.#query.length === 0) this.#renderDropdown();
  }

  /**
   * Workspace entry points (host-pushed). Shown in the empty-state
   * dropdown so the API surface is one click away from focusing the
   * search field.
   */
  set entryPoints(next: ReadonlyArray<SymbolSearchResult>) {
    this.#entryPoints = next;
    if (this.#query.length === 0) this.#renderDropdown();
  }

  set loading(next: boolean) {
    if (next === this.#loading) return;
    this.#loading = next;
    this.#renderDropdown();
  }

  connectedCallback(): void {
    if (this.#mounted) return;
    this.#mounted = true;
    this.#render();
    this.#syncShortcut();
  }

  disconnectedCallback(): void {
    this.#unbindShortcut();
    if (this.#debounceHandle !== null) {
      window.clearTimeout(this.#debounceHandle);
      this.#debounceHandle = null;
    }
  }

  attributeChangedCallback(name: string): void {
    if (!this.#mounted) return;
    if (name === 'global-shortcut') this.#syncShortcut();
    if (name === 'placeholder' && this.#nodes !== null) {
      this.#nodes.input.placeholder = this.getAttribute('placeholder') ?? 'Search symbols…';
    }
  }

  override focus(): void {
    this.#nodes?.input.focus();
  }

  #render(): void {
    const root = document.createElement('div');
    root.className = C.search;
    const placeholder = this.getAttribute('placeholder') ?? 'Search symbols, files, types…';
    root.innerHTML = `
			<input
				type="search"
				class="${C.input}"
				placeholder="${escapeAttr(placeholder)}"
				data-role="input"
				autocomplete="off"
				spellcheck="false"
				aria-label="Search symbols"
			/>
			<span class="${C.shortcut}" data-role="shortcut">${escapeAttr(SHORTCUT_LABEL)}</span>
			<div class="${C.dropdown}" data-role="dropdown" role="listbox"></div>
		`;
    this.replaceChildren(root);
    this.#nodes = {
      root,
      input: root.querySelector<HTMLInputElement>('[data-role="input"]')!,
      dropdown: root.querySelector<HTMLElement>('[data-role="dropdown"]')!,
    };
    this.#wireInput();
    this.#renderDropdown();
  }

  #wireInput(): void {
    const n = this.#nodes;
    if (n === null) return;

    n.input.addEventListener('input', () => {
      const q = n.input.value;
      this.#query = q;
      if (this.#debounceHandle !== null) window.clearTimeout(this.#debounceHandle);
      this.#debounceHandle = window.setTimeout(() => {
        this.#debounceHandle = null;
        this.dispatchEvent(new CustomEvent<SearchQueryDetail>('sd-search-query', {
          detail: { query: q }, bubbles: true, composed: true,
        }));
      }, QUERY_DEBOUNCE_MS);
      // Always open while focused — empty query now surfaces the
      // recents + entry-point preview sections so the user has a
      // starting point even before typing.
      this.#setOpen(true);
    });

    n.input.addEventListener('focus', () => {
      this.#setOpen(true);
    });

    n.input.addEventListener('blur', () => {
      // Delay so a click on a result lands first.
      window.setTimeout(() => this.#setOpen(false), 120);
    });

    n.input.addEventListener('keydown', e => {
      if (e.key === 'Escape') {
        e.preventDefault();
        n.input.value = '';
        this.#query = '';
        this.#setOpen(false);
        this.dispatchEvent(new CustomEvent<SearchQueryDetail>('sd-search-query', {
          detail: { query: '' }, bubbles: true, composed: true,
        }));
        return;
      }
      const items = this.#navigableItems();
      if (items.length === 0) return;
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        this.#activeIdx = (this.#activeIdx + 1) % items.length;
        this.#renderDropdown();
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        this.#activeIdx = (this.#activeIdx - 1 + items.length) % items.length;
        this.#renderDropdown();
      } else if (e.key === 'Enter') {
        e.preventDefault();
        const pick = items[this.#activeIdx];
        if (pick) this.#select(pick.fqdn);
      }
    });
  }

  /**
   * Flat list of currently focusable entries — drives keyboard
   * navigation across whichever sections are visible (results,
   * suggestions, recents, entry points).
   */
  #navigableItems(): ReadonlyArray<{ fqdn: string }> {
    if (this.#query.length === 0) {
      return [...this.#recents, ...this.#entryPoints];
    }
    if (this.#results.length > 0) return this.#results;
    return this.#suggestions;
  }

  #renderDropdown(): void {
    const n = this.#nodes;
    if (n === null) return;
    n.dropdown.className = classigo(C.dropdown, this.#open && C.dropdownOpen);
    n.dropdown.replaceChildren();

    if (this.#loading) {
      n.dropdown.innerHTML = `<div class="${C.status}">Searching…</div>`;
      return;
    }

    // Empty state — surface recents + entry points + a one-liner
    // hint so the dropdown is informative even before the user types.
    if (this.#query.length === 0) {
      this.#renderEmptyState(n.dropdown);
      return;
    }

    if (this.#results.length === 0) {
      const status = document.createElement('div');
      status.className = C.status;
      status.textContent = 'No results.';
      n.dropdown.appendChild(status);
      if (this.#suggestions.length > 0) {
        this.#appendSectionTitle(n.dropdown, 'Did you mean…');
        this.#appendItemList(
          n.dropdown,
          this.#suggestions.map(s => ({ fqdn: s.fqdn, name: s.name, kindLabel: s.kindLabel, kind: s.kind })),
          0,
        );
      }
      return;
    }

    this.#appendItemList(n.dropdown, this.#results, 0);
  }

  #renderEmptyState(mount: HTMLElement): void {
    const tip = document.createElement('div');
    tip.className = C.tip;
    tip.innerHTML = 'Type a name fragment. <code>⌘K</code> focuses this field from anywhere. <kbd>Esc</kbd> clears.';
    mount.appendChild(tip);

    let offset = 0;
    if (this.#recents.length > 0) {
      this.#appendSectionTitle(mount, 'Recently viewed');
      this.#appendItemList(mount, this.#recents, offset);
      offset += this.#recents.length;
    }
    if (this.#entryPoints.length > 0) {
      this.#appendSectionTitle(mount, 'Entry points');
      this.#appendItemList(mount, this.#entryPoints, offset);
    }
    if (this.#recents.length === 0 && this.#entryPoints.length === 0) {
      const status = document.createElement('div');
      status.className = C.status;
      status.textContent = 'No history yet — start typing to search.';
      mount.appendChild(status);
    }
  }

  #appendSectionTitle(mount: HTMLElement, label: string): void {
    const title = document.createElement('div');
    title.className = C.sectionTitle;
    title.textContent = label;
    mount.appendChild(title);
  }

  #appendItemList(
    mount: HTMLElement,
    items: ReadonlyArray<SymbolSearchResult>,
    indexOffset: number,
  ): void {
    const ul = document.createElement('ul');
    ul.className = classigo(C.list, C.section);
    items.forEach((r, idx) => {
      const flatIdx = indexOffset + idx;
      const li = document.createElement('li');
      li.className = classigo(C.item, flatIdx === this.#activeIdx && C.itemActive);
      li.title = r.fqdn;
      li.setAttribute('role', 'option');
      li.dataset['kind'] = bucketKind(r.kind);
      li.innerHTML = `
				<span class="${C.itemKind}">${escapeHtml(r.kindLabel)}</span>
				<span class="${C.itemName}">${escapeHtml(r.name)}</span>
				<span class="${C.itemFqdn}">${escapeHtml(r.fqdn)}</span>
			`;
      // mousedown (not click) so it fires before the input's blur-close.
      li.addEventListener('mousedown', e => {
        e.preventDefault();
        this.#select(r.fqdn);
      });
      li.addEventListener('mouseenter', () => {
        if (this.#activeIdx !== flatIdx) {
          this.#activeIdx = flatIdx;
          this.#renderDropdown();
        }
      });
      ul.appendChild(li);
    });
    mount.appendChild(ul);
  }

  #setOpen(open: boolean): void {
    if (this.#open === open) return;
    this.#open = open;
    this.#renderDropdown();
  }

  #select(fqdn: string): void {
    focusStore.setFocus(fqdn);
    this.dispatchEvent(new CustomEvent<SearchSelectDetail>('sd-search-select', {
      detail: { fqdn }, bubbles: true, composed: true,
    }));
    this.#setOpen(false);
    if (this.#nodes !== null) this.#nodes.input.blur();
  }

  #syncShortcut(): void {
    const enabled = this.getAttribute('global-shortcut') !== null
      && this.getAttribute('global-shortcut') !== 'false';
    if (enabled && this.#shortcutHandler === null) {
      this.#shortcutHandler = e => {
        if ((e.ctrlKey || e.metaKey) && (e.key === 'k' || e.key === 'K')) {
          e.preventDefault();
          this.focus();
        }
      };
      window.addEventListener('keydown', this.#shortcutHandler);
    } else if (!enabled) {
      this.#unbindShortcut();
    }
  }

  #unbindShortcut(): void {
    if (this.#shortcutHandler !== null) {
      window.removeEventListener('keydown', this.#shortcutHandler);
      this.#shortcutHandler = null;
    }
  }
}

/**
 * Narrow the daemon's free-form kind string into one of the 5 IR
 * buckets the SCSS variants colour. Anything unrecognised falls
 * back to `unknown` so the chip still renders without a kind tint.
 */
function bucketKind(kind: string | undefined): string {
  if (kind === undefined) return 'unknown';
  switch (kind.toLowerCase()) {
    case 'callable':
    case 'function':
    case 'method':
      return 'callable';
    case 'type':
    case 'struct':
    case 'class':
    case 'interface':
    case 'enum':
    case 'trait':
    case 'union':
    case 'type_alias':
    case 'typedef':
      return 'type';
    case 'value':
    case 'field':
    case 'property':
    case 'variable':
    case 'constant':
    case 'enum_variant':
    case 'interface_property':
      return 'value';
    case 'module':
    case 'namespace':
    case 'package':
      return 'module';
    case 'macro':
      return 'macro';
    default:
      return 'unknown';
  }
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function escapeAttr(text: string): string {
  return escapeHtml(text);
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_SEARCH_TAG)) {
  customElements.define(STANDARDOC_SEARCH_TAG, SearchElement);
}

declare global {
  interface HTMLElementTagNameMap {
    [STANDARDOC_SEARCH_TAG]: SearchElement;
  }
}
