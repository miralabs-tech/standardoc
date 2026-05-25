/**
 * `<standardoc-compare-panel>` — V0 spawnable panel that places two
 * `SymbolDetail` lenses side by side. The host pushes pre-resolved
 * details for each side via `setSide(side, detail)` / `setLoading`,
 * matching the dumb-component pattern used elsewhere in the lib.
 *
 * Render is intentionally lighter than `<standardoc-symbol-details>`:
 * the goal here isn't to inspect one symbol in depth but to spot
 * differences between two at a glance — name / kind / visibility /
 * file location / doc / relation counts. Click on a relation count
 * sets the global focus to nothing (yet) — V0 surfaces totals only,
 * not the individual edges.
 */

import classigo from 'classigo';

import type {
  SymbolDetail,
  SymbolRelationBucket,
  SymbolRelationKind,
} from '../symbol-details/symbol-details.type';
import type {
  ComparePanelData,
  CompareRefreshRequestDetail,
  CompareSide,
} from './compare-panel.type';
import s from './compare-panel.module.scss';

export const STANDARDOC_COMPARE_PANEL_TAG = 'standardoc-compare-panel';

const C = {
  root: s.compare ?? '',
  header: s.compare__header ?? '',
  title: s.compare__title ?? '',
  refresh: s.compare__refresh ?? '',
  grid: s.compare__grid ?? '',
  side: s.compare__side ?? '',
  sideEmpty: s['compare__side-empty'] ?? '',
  name: s.compare__name ?? '',
  fqdn: s.compare__fqdn ?? '',
  tags: s.compare__tags ?? '',
  tag: s.compare__tag ?? '',
  location: s.compare__location ?? '',
  doc: s.compare__doc ?? '',
  relations: s.compare__relations ?? '',
  relRow: s['compare__rel-row'] ?? '',
} as const;

const RELATION_LABEL: Record<SymbolRelationKind, string> = {
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

const RELATION_ORDER: ReadonlyArray<SymbolRelationKind> = [
  'usedBy', 'usesTypes', 'calls', 'imports', 'importedBy', 'testedBy', 'implements', 'extends',
];

const DOC_COLLAPSED_CHARS = 240;

export class ComparePanelElement extends HTMLElement {
  #mounted = false;
  #data: ComparePanelData | null = null;
  #nodes: { root: HTMLElement; grid: HTMLElement } | null = null;

  set data(next: ComparePanelData | null) {
    this.#data = next;
    this.#refresh();
  }

  setSide(side: 'left' | 'right', value: CompareSide): void {
    if (this.#data === null) {
      this.#data = {
        left: side === 'left' ? value : { fqdn: '', detail: null, loading: false },
        right: side === 'right' ? value : { fqdn: '', detail: null, loading: false },
      };
    } else {
      this.#data = side === 'left'
        ? { left: value, right: this.#data.right }
        : { left: this.#data.left, right: value };
    }
    this.#refresh();
  }

  connectedCallback(): void {
    if (this.#mounted) return;
    this.#mounted = true;
    const root = document.createElement('div');
    root.className = C.root;
    root.innerHTML = `
			<div class="${C.header}">
				<span class="${C.title}">Compare</span>
				<span data-role="subtitle"></span>
				<button type="button" class="${C.refresh}" data-role="refresh" title="Re-fetch both symbols">↻ refresh</button>
			</div>
			<div class="${C.grid}" data-role="grid"></div>
		`;
    this.replaceChildren(root);
    const grid = root.querySelector<HTMLElement>('[data-role="grid"]');
    if (grid === null) return;
    this.#nodes = { root, grid };
    const refreshBtn = root.querySelector<HTMLButtonElement>('[data-role="refresh"]');
    if (refreshBtn !== null) {
      refreshBtn.addEventListener('click', () => {
        if (this.#data === null) return;
        const detail: CompareRefreshRequestDetail = {
          leftFqdn: this.#data.left.fqdn,
          rightFqdn: this.#data.right.fqdn,
        };
        if (detail.leftFqdn.length === 0 && detail.rightFqdn.length === 0) return;
        this.dispatchEvent(new CustomEvent<CompareRefreshRequestDetail>('sd-compare-refresh-request', {
          detail, bubbles: true, composed: true,
        }));
      });
    }
    this.#refresh();
  }

  #refresh(): void {
    const n = this.#nodes;
    if (n === null) return;

    const subtitle = n.root.querySelector<HTMLElement>('[data-role="subtitle"]');
    const refreshBtn = n.root.querySelector<HTMLButtonElement>('[data-role="refresh"]');
    if (this.#data === null) {
      if (subtitle) subtitle.textContent = '';
      if (refreshBtn) refreshBtn.disabled = true;
      n.grid.innerHTML = '';
      n.grid.appendChild(this.#renderEmpty('Pin two symbols via Add to compare to populate.'));
      return;
    }
    if (subtitle) {
      subtitle.textContent = this.#data.left.detail !== null && this.#data.right.detail !== null
        ? `${this.#data.left.detail.name} ↔ ${this.#data.right.detail.name}`
        : '';
    }
    if (refreshBtn) {
      const busy = this.#data.left.loading || this.#data.right.loading;
      const hasPair = this.#data.left.fqdn.length > 0 && this.#data.right.fqdn.length > 0;
      refreshBtn.disabled = busy || !hasPair;
    }

    n.grid.replaceChildren(
      this.#renderSide(this.#data.left, this.#data.right.detail),
      this.#renderSide(this.#data.right, this.#data.left.detail),
    );
  }

  #renderEmpty(text: string): HTMLElement {
    const el = document.createElement('div');
    el.className = classigo(C.side, C.sideEmpty);
    el.textContent = text;
    return el;
  }

  #renderSide(side: CompareSide, other: SymbolDetail | null): HTMLElement {
    if (side.loading) return this.#renderEmpty('Loading…');
    if (side.detail === null) {
      return this.#renderEmpty(side.fqdn.length > 0 ? `Loading ${side.fqdn}…` : 'Empty slot');
    }
    const detail = side.detail;
    const wrap = document.createElement('div');
    wrap.className = C.side;

    const name = document.createElement('div');
    name.className = C.name;
    name.textContent = detail.name;
    wrap.appendChild(name);

    const fqdn = document.createElement('div');
    fqdn.className = C.fqdn;
    fqdn.textContent = detail.fqdn;
    wrap.appendChild(fqdn);

    const tags = document.createElement('div');
    tags.className = C.tags;
    const tagValues: string[] = [];
    if (detail.kindLabel) tagValues.push(detail.kindLabel);
    if (detail.visibility) tagValues.push(detail.visibility);
    if (detail.entryPointKind !== null) tagValues.push(`ep:${detail.entryPointKind}`);
    for (const t of tagValues) {
      const span = document.createElement('span');
      span.className = C.tag;
      span.textContent = t;
      tags.appendChild(span);
    }
    wrap.appendChild(tags);

    const loc = document.createElement('div');
    loc.className = C.location;
    loc.textContent = `${detail.file}:${detail.startLine}`;
    wrap.appendChild(loc);

    if (detail.documentation !== null && detail.documentation.length > 0) {
      const doc = document.createElement('div');
      doc.className = C.doc;
      doc.textContent = detail.documentation.length > DOC_COLLAPSED_CHARS
        ? `${detail.documentation.slice(0, DOC_COLLAPSED_CHARS)}…`
        : detail.documentation;
      wrap.appendChild(doc);
    }

    wrap.appendChild(this.#renderRelations(detail, other));
    return wrap;
  }

  #renderRelations(detail: SymbolDetail, other: SymbolDetail | null): HTMLElement {
    const wrap = document.createElement('div');
    wrap.className = C.relations;
    const byKind = new Map<SymbolRelationKind, SymbolRelationBucket>();
    for (const b of detail.relations) byKind.set(b.kind, b);
    const otherByKind = new Map<SymbolRelationKind, SymbolRelationBucket>();
    if (other !== null) for (const b of other.relations) otherByKind.set(b.kind, b);

    for (const kind of RELATION_ORDER) {
      const mine = byKind.get(kind)?.total ?? 0;
      const yours = otherByKind.get(kind)?.total ?? 0;
      if (mine === 0 && yours === 0) continue;
      const row = document.createElement('div');
      row.className = C.relRow;
      const label = document.createElement('span');
      label.textContent = RELATION_LABEL[kind];
      const value = document.createElement('span');
      value.className = 'diff';
      value.textContent = other === null ? `${mine}` : `${mine} (Δ ${signedDelta(mine, yours)})`;
      row.appendChild(label);
      row.appendChild(value);
      wrap.appendChild(row);
    }
    return wrap;
  }
}

function signedDelta(a: number, b: number): string {
  const d = a - b;
  if (d === 0) return '=';
  return d > 0 ? `+${d}` : `${d}`;
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_COMPARE_PANEL_TAG)) {
  customElements.define(STANDARDOC_COMPARE_PANEL_TAG, ComparePanelElement);
}

declare global {
  interface HTMLElementTagNameMap {
    [STANDARDOC_COMPARE_PANEL_TAG]: ComparePanelElement;
  }
}
