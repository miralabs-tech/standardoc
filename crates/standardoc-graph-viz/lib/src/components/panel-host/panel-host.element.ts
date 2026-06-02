/**
 * `<standardoc-panel-host>` — bottom drawer hosting the spawnable
 * Phase-4 panels (Compare today; Callers Graph / Field Details /
 * Source View later). The element subscribes to the global `panelManager`
 * singleton: every mutation re-renders the tab strip + the active
 * panel's body. Lives as a sibling of `<standardoc-panel-layout>`
 * (not inside its grid) so the existing layout isn't disturbed —
 * the drawer overlays the bottom of the viewport when one or more
 * panels are open and is fully hidden otherwise.
 *
 * Per-kind component mounting: each `PanelKind` maps to a custom
 * element here. Switching tabs swaps the body's child element; the
 * host exposes `activePanelElement` so the shell can fetch data and
 * push it through the right element's setter (callers-graph etc.)
 * without having to know the host's internal DOM structure.
 */

import classigo from 'classigo';

import {
  STANDARDOC_COMPARE_PANEL_TAG,
} from '../compare-panel';
import {
  panelManager,
  type PanelInstance,
  type PanelKind,
  type PanelManagerState,
} from '../../panel-manager';
import s from './panel-host.module.scss';

export const STANDARDOC_PANEL_HOST_TAG = 'standardoc-panel-host';

const C = {
  host: s.host ?? '',
  hidden: s['host--hidden'] ?? '',
  tabs: s.host__tabs ?? '',
  tab: s.host__tab ?? '',
  tabActive: s['host__tab--active'] ?? '',
  tabLabel: s['host__tab-label'] ?? '',
  close: s.host__close ?? '',
  body: s.host__body ?? '',
} as const;

const CHILD_TAG_BY_KIND: Record<PanelKind, string> = {
  compare: STANDARDOC_COMPARE_PANEL_TAG,
};

export class PanelHostElement extends HTMLElement {
  #mounted = false;
  #unsubscribe: (() => void) | null = null;
  #tabsEl: HTMLElement | null = null;
  #bodyEl: HTMLElement | null = null;
  #renderedActiveId: string | null = null;

  connectedCallback(): void {
    if (this.#mounted) return;
    this.#mounted = true;
    this.classList.add(C.host);
    this.innerHTML = `
			<div class="${C.tabs}" data-role="tabs"></div>
			<div class="${C.body}" data-role="body"></div>
		`;
    this.#tabsEl = this.querySelector<HTMLElement>('[data-role="tabs"]');
    this.#bodyEl = this.querySelector<HTMLElement>('[data-role="body"]');
    this.#apply(panelManager.get());
    this.#unsubscribe = panelManager.subscribe(state => this.#apply(state));
  }

  disconnectedCallback(): void {
    this.#unsubscribe?.();
    this.#unsubscribe = null;
  }

  /**
   * The currently mounted child element (e.g. `<standardoc-compare-panel>`).
   * Hosts use this to push panel-specific data after calling
   * `panelManager.open(...)`.
   */
  get activePanelElement(): HTMLElement | null {
    return this.#bodyEl?.firstElementChild as HTMLElement | null;
  }

  /**
   * Lookup the element rendering a specific panel id. Returns null
   * when the id isn't the active panel (only the active body is
   * mounted — others are torn down on tab switch).
   */
  getPanelElement(id: string): HTMLElement | null {
    if (id !== this.#renderedActiveId) return null;
    return this.activePanelElement;
  }

  #apply(state: PanelManagerState): void {
    if (state.panels.length === 0) {
      this.classList.add(C.hidden);
      this.#renderedActiveId = null;
      if (this.#tabsEl) this.#tabsEl.replaceChildren();
      if (this.#bodyEl) this.#bodyEl.replaceChildren();
      return;
    }
    this.classList.remove(C.hidden);
    this.#renderTabs(state);
    this.#renderBody(state);
  }

  #renderTabs(state: PanelManagerState): void {
    const el = this.#tabsEl;
    if (el === null) return;
    el.replaceChildren();
    for (const p of state.panels) {
      el.appendChild(this.#renderTab(p, p.id === state.activeId));
    }
  }

  #renderTab(p: PanelInstance, active: boolean): HTMLElement {
    const tab = document.createElement('div');
    tab.className = classigo(C.tab, active && C.tabActive);
    tab.dataset.id = p.id;

    const label = document.createElement('button');
    label.type = 'button';
    label.className = C.tabLabel;
    label.textContent = p.title;
    label.title = p.title;
    label.addEventListener('click', () => panelManager.focus(p.id));
    tab.appendChild(label);

    const close = document.createElement('button');
    close.type = 'button';
    close.className = C.close;
    close.textContent = '×';
    close.title = 'Close panel';
    close.addEventListener('click', e => {
      e.stopPropagation();
      panelManager.close(p.id);
    });
    tab.appendChild(close);

    return tab;
  }

  #renderBody(state: PanelManagerState): void {
    const el = this.#bodyEl;
    if (el === null) return;
    const active = state.panels.find(p => p.id === state.activeId);
    if (active === undefined) {
      el.replaceChildren();
      this.#renderedActiveId = null;
      return;
    }
    if (this.#renderedActiveId === active.id) {
      // Same panel still active — props may have been updated but
      // the host owns data injection through setters, so leave the
      // child element alone. No-op.
      return;
    }
    const tag = CHILD_TAG_BY_KIND[active.kind];
    const child = document.createElement(tag);
    el.replaceChildren(child);
    this.#renderedActiveId = active.id;
  }
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_PANEL_HOST_TAG)) {
  customElements.define(STANDARDOC_PANEL_HOST_TAG, PanelHostElement);
}

declare global {
  interface HTMLElementTagNameMap {
    [STANDARDOC_PANEL_HOST_TAG]: PanelHostElement;
  }
}
