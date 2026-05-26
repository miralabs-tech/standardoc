/**
 * `<standardoc-panel-layout>` — CSS-grid shell hosting the four main
 * panels (Explorer, Overview, Focus Graph, Symbol Details) plus a top
 * toolbar row. Children are placed by their `data-slot` attribute:
 *
 *   <standardoc-panel-layout>
 *     <standardoc-toolbar data-slot="toolbar"></standardoc-toolbar>
 *     <standardoc-explorer data-slot="explorer"></standardoc-explorer>
 *     <standardoc-overview data-slot="overview"></standardoc-overview>
 *     <standardoc-focus-graph data-slot="focus"></standardoc-focus-graph>
 *     <standardoc-symbol-details data-slot="details"></standardoc-symbol-details>
 *   </standardoc-panel-layout>
 *
 * Owns:
 *   - 4 booleans for per-panel visibility (toggle to hide/show a panel
 *     and reclaim its space for the neighbours)
 *   - 3 resizer divs (col-left, col-right, row) injected as children
 *     on mount with pointer drag wiring
 *   - Optional `localStorage` persistence keyed by `persistKey`
 *
 * Events:
 *   - `sd-panel-layout-change` detail: { state } — on any state mutation
 *     (toggle, resize, reset). Hosts can listen for analytics or to
 *     refresh dependent layouts.
 */

import s from './panel-layout.module.scss';

export const STANDARDOC_PANEL_LAYOUT_TAG = 'standardoc-panel-layout';

export type PanelSlot = 'explorer' | 'overview' | 'focus' | 'details';

export interface PanelLayoutState {
  readonly visibility: Readonly<Record<PanelSlot, boolean>>;
  /// Width of the explorer column in CSS px when visible.
  readonly leftColPx: number;
  /// Width of the details column in CSS px when visible.
  readonly rightColPx: number;
  /// Ratio of the overview row to the total middle column height —
  /// `0.5` means overview / focus share equally; `0.7` makes overview
  /// 70% of the middle column.
  readonly middleSplit: number;
}

export interface PanelLayoutChangeDetail {
  readonly state: PanelLayoutState;
}

const DEFAULT_STATE: PanelLayoutState = {
  visibility: { explorer: true, overview: true, focus: true, details: true },
  leftColPx: 260,
  rightColPx: 360,
  middleSplit: 0.5,
};

const STORAGE_KEY_DEFAULT = 'sd-panel-layout';
const MIN_COL_PX = 140;
const MAX_COL_PX = 600;
const MIN_SPLIT = 0.15;
const MAX_SPLIT = 0.85;

const C = {
  layout: s.layout ?? '',
  explorerHidden: s['layout--explorer-hidden'] ?? '',
  overviewHidden: s['layout--overview-hidden'] ?? '',
  focusHidden: s['layout--focus-hidden'] ?? '',
  detailsHidden: s['layout--details-hidden'] ?? '',
} as const;

export class PanelLayoutElement extends HTMLElement {
  #mounted = false;
  #state: PanelLayoutState = DEFAULT_STATE;
  #persistKey: string | null = STORAGE_KEY_DEFAULT;

  connectedCallback(): void {
    if (this.#mounted) return;
    this.#mounted = true;
    this.classList.add(C.layout);
    this.#loadPersistedState();
    this.#createResizers();
    this.#applyState();
  }

  get state(): PanelLayoutState {
    return this.#state;
  }

  /// Replace the full state. Use `togglePanel` / individual setters for
  /// finer-grained mutations; this setter is the bulk-update path
  /// (e.g. host restoring a saved layout from somewhere other than the
  /// element's own `localStorage`).
  set state(next: PanelLayoutState) {
    this.#state = next;
    this.#applyState();
    this.#persist();
    this.#emit();
  }

  /// `localStorage` key used to persist + restore state. Set to `null`
  /// to disable persistence entirely. Defaults to `'sd-panel-layout'`.
  set persistKey(key: string | null) {
    this.#persistKey = key;
    if (key !== null) this.#loadPersistedState();
    this.#applyState();
  }

  get persistKey(): string | null {
    return this.#persistKey;
  }

  /// Flip the visibility of a single panel. Adjacent resizer lane
  /// collapses with it.
  togglePanel(slot: PanelSlot): void {
    const next = !this.#state.visibility[slot];
    this.#state = {
      ...this.#state,
      visibility: { ...this.#state.visibility, [slot]: next },
    };
    this.#applyState();
    this.#persist();
    this.#emit();
  }

  /// Restore the default layout (everything visible, default col / row
  /// sizes). Useful for a "Reset layout" toolbar action.
  resetLayout(): void {
    this.#state = DEFAULT_STATE;
    this.#applyState();
    this.#persist();
    this.#emit();
  }

  #createResizers(): void {
    const left = document.createElement('div');
    left.dataset['role'] = 'resizer-left';
    left.setAttribute('role', 'separator');
    left.setAttribute('aria-orientation', 'vertical');
    left.setAttribute('aria-label', 'Resize explorer panel');
    const right = document.createElement('div');
    right.dataset['role'] = 'resizer-right';
    right.setAttribute('role', 'separator');
    right.setAttribute('aria-orientation', 'vertical');
    right.setAttribute('aria-label', 'Resize details panel');
    const row = document.createElement('div');
    row.dataset['role'] = 'resizer-row';
    row.setAttribute('role', 'separator');
    row.setAttribute('aria-orientation', 'horizontal');
    row.setAttribute('aria-label', 'Resize overview vs focus split');
    this.appendChild(left);
    this.appendChild(right);
    this.appendChild(row);
    this.#wireResizer(left, 'col-left');
    this.#wireResizer(right, 'col-right');
    this.#wireResizer(row, 'row');
  }

  #wireResizer(el: HTMLElement, kind: 'col-left' | 'col-right' | 'row'): void {
    // `pointercapture` on the resizer host plus window-level move /
    // up listeners — same drag pattern as the overview orbit ball.
    // SVG-vs-HTML capture differences don't apply here (the resizer
    // is a `<div>`), but window-level listeners are still the most
    // robust path for "follow the cursor anywhere on the screen".
    el.addEventListener('pointerdown', e => {
      e.preventDefault();
      let last = { x: e.clientX, y: e.clientY };
      const onMove = (mv: PointerEvent) => {
        const dx = mv.clientX - last.x;
        const dy = mv.clientY - last.y;
        last = { x: mv.clientX, y: mv.clientY };
        const cur = this.#state;
        if (kind === 'col-left') {
          const next = Math.max(MIN_COL_PX, Math.min(MAX_COL_PX, cur.leftColPx + dx));
          this.#state = { ...cur, leftColPx: next };
        } else if (kind === 'col-right') {
          // Right resizer: drag right = grow middle = shrink right
          const next = Math.max(MIN_COL_PX, Math.min(MAX_COL_PX, cur.rightColPx - dx));
          this.#state = { ...cur, rightColPx: next };
        } else {
          // Row resizer: convert pixel drag to ratio of available
          // middle column height (excludes toolbar + row resizer gap).
          const r = this.getBoundingClientRect();
          const padding = 16; // ~ outer padding contributions
          const middleH = Math.max(1, r.height - 48 - 4 - padding);
          const newTopPx = cur.middleSplit * middleH + dy;
          const ratio = Math.max(MIN_SPLIT, Math.min(MAX_SPLIT, newTopPx / middleH));
          this.#state = { ...cur, middleSplit: ratio };
        }
        this.#applyState();
      };
      const onUp = () => {
        window.removeEventListener('pointermove', onMove);
        window.removeEventListener('pointerup', onUp);
        window.removeEventListener('pointercancel', onUp);
        // Persist + emit once at the END of the drag rather than per
        // frame — writes to localStorage are cheap but firing the
        // change event every frame would spam any listener.
        this.#persist();
        this.#emit();
      };
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', onUp);
      window.addEventListener('pointercancel', onUp);
    });
  }

  #applyState(): void {
    const st = this.#state;
    this.classList.toggle(C.explorerHidden, !st.visibility.explorer);
    this.classList.toggle(C.overviewHidden, !st.visibility.overview);
    this.classList.toggle(C.focusHidden, !st.visibility.focus);
    this.classList.toggle(C.detailsHidden, !st.visibility.details);
    // Inline CSS vars feed the grid template defined in SCSS. They
    // also carry the VISIBILITY state — a hidden panel's lane plus
    // its adjacent gap collapse to 0 so the neighbour reclaims the
    // pixels (otherwise the inline `${px}` overrides the modifier
    // class' `0` and the column stays wide-but-empty).
    const leftCol = st.visibility.explorer ? `${st.leftColPx}px` : '0';
    const rightCol = st.visibility.details ? `${st.rightColPx}px` : '0';
    const gapLeft = st.visibility.explorer ? '8px' : '0';
    const gapRight = st.visibility.details ? '8px' : '0';
    const gapRow = (st.visibility.overview && st.visibility.focus) ? '8px' : '0';
    // Both row tracks live in `fr` space so the row split adapts
    // smoothly to container height. 100× scale keeps the fr math
    // stable for tight splits (0.15 → 15fr vs 85fr reads cleanly).
    const rowOverview = st.visibility.overview ? `${st.middleSplit * 100}fr` : '0';
    const rowFocus = st.visibility.focus ? `${(1 - st.middleSplit) * 100}fr` : '0';
    this.style.setProperty('--sd-shell-left-col', leftCol);
    this.style.setProperty('--sd-shell-right-col', rightCol);
    this.style.setProperty('--sd-shell-gap-left', gapLeft);
    this.style.setProperty('--sd-shell-gap-right', gapRight);
    this.style.setProperty('--sd-shell-gap-row', gapRow);
    this.style.setProperty('--sd-shell-row-overview', rowOverview);
    this.style.setProperty('--sd-shell-row-focus', rowFocus);
  }

  #persist(): void {
    if (this.#persistKey === null) return;
    try {
      localStorage.setItem(this.#persistKey, JSON.stringify(this.#state));
    } catch {
      // SecurityError / QuotaExceeded — silently ignore so a private-
      // browsing context doesn't break the layout entirely.
    }
  }

  #loadPersistedState(): void {
    if (this.#persistKey === null) return;
    try {
      const raw = localStorage.getItem(this.#persistKey);
      if (raw === null) return;
      const parsed = JSON.parse(raw) as Partial<PanelLayoutState>;
      if (typeof parsed !== 'object' || parsed === null) return;
      // Merge with defaults so an older schema or malformed entry
      // can't leave the layout in an inconsistent shape.
      const visibility = {
        ...DEFAULT_STATE.visibility,
        ...(parsed.visibility ?? {}),
      };
      this.#state = {
        visibility,
        leftColPx: typeof parsed.leftColPx === 'number' ? parsed.leftColPx : DEFAULT_STATE.leftColPx,
        rightColPx: typeof parsed.rightColPx === 'number' ? parsed.rightColPx : DEFAULT_STATE.rightColPx,
        middleSplit: typeof parsed.middleSplit === 'number' ? parsed.middleSplit : DEFAULT_STATE.middleSplit,
      };
    } catch {
      // Malformed JSON — fall back to defaults silently.
    }
  }

  #emit(): void {
    this.dispatchEvent(new CustomEvent<PanelLayoutChangeDetail>('sd-panel-layout-change', {
      detail: { state: this.#state },
      bubbles: true,
      composed: true,
    }));
  }
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_PANEL_LAYOUT_TAG)) {
  customElements.define(STANDARDOC_PANEL_LAYOUT_TAG, PanelLayoutElement);
}

declare global {
  interface HTMLElementTagNameMap {
    [STANDARDOC_PANEL_LAYOUT_TAG]: PanelLayoutElement;
  }
}
