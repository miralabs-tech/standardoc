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
      this.#setOpen(q.length > 0);
    });

    n.input.addEventListener('focus', () => {
      if (this.#query.length > 0) this.#setOpen(true);
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
      if (this.#results.length === 0) return;
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        this.#activeIdx = (this.#activeIdx + 1) % this.#results.length;
        this.#renderDropdown();
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        this.#activeIdx = (this.#activeIdx - 1 + this.#results.length) % this.#results.length;
        this.#renderDropdown();
      } else if (e.key === 'Enter') {
        e.preventDefault();
        const pick = this.#results[this.#activeIdx];
        if (pick) this.#select(pick.fqdn);
      }
    });
  }

  #renderDropdown(): void {
    const n = this.#nodes;
    if (n === null) return;
    n.dropdown.className = classigo(C.dropdown, this.#open && C.dropdownOpen);

    if (this.#loading) {
      n.dropdown.innerHTML = `<div class="${C.status}">Searching…</div>`;
      return;
    }
    if (this.#query.length === 0) {
      n.dropdown.innerHTML = '';
      return;
    }
    if (this.#results.length === 0) {
      n.dropdown.innerHTML = `<div class="${C.status}">No results.</div>`;
      return;
    }

    const ul = document.createElement('ul');
    ul.className = C.list;
    this.#results.forEach((r, idx) => {
      const li = document.createElement('li');
      li.className = classigo(C.item, idx === this.#activeIdx && C.itemActive);
      li.title = r.fqdn;
      li.setAttribute('role', 'option');
      li.innerHTML = `
				<span class="${C.itemName}">${escapeHtml(r.name)}</span>
				<span class="${C.itemKind}">${escapeHtml(r.kindLabel)}</span>
				<span class="${C.itemFqdn}">${escapeHtml(r.fqdn)}</span>
			`;
      // mousedown (not click) so it fires before the input's blur-close.
      li.addEventListener('mousedown', e => {
        e.preventDefault();
        this.#select(r.fqdn);
      });
      li.addEventListener('mouseenter', () => {
        if (this.#activeIdx !== idx) {
          this.#activeIdx = idx;
          this.#renderDropdown();
        }
      });
      ul.appendChild(li);
    });
    n.dropdown.replaceChildren(ul);
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
