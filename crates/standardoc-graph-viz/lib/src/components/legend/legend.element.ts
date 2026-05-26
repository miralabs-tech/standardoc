/**
 * `<standardoc-legend>` — floating bottom-right legend showing the
 * three colour axes used across the shell:
 *   • Kinds — fill colour of the Focus card border, Explorer icons,
 *     and Symbol Details kind chip.
 *   • Edges — stroke colour of the Focus graph connectors. Double
 *     stroke surfaces IMPLEMENTS / EXTENDS.
 *   • Languages — Explorer language indicator + future per-card
 *     language chip (deferred backend bind).
 *
 * Phase A is purely visual: clicking an entry emits
 * `sd-legend-filter` with `{ category, value }`. No host currently
 * subscribes; Phase B will wire the event to Explorer / Focus /
 * Overview filter state.
 *
 * Collapse state is local; `visible` and `collapsed` are both host-
 * controllable properties so a global shortcut (e.g. "L") can drive
 * the panel without the legend reaching back into the host.
 */

import classigo from 'classigo';

import type { LegendCategory, LegendFilterDetail } from './legend.type';
import s from './legend.module.scss';

export const STANDARDOC_LEGEND_TAG = 'standardoc-legend';

const C = {
	legend: s.legend ?? '',
	collapsed: s['legend--collapsed'] ?? '',
	header: s.legend__header ?? '',
	title: s.legend__title ?? '',
	toggle: s.legend__toggle ?? '',
	body: s.legend__body ?? '',
	section: s.legend__section ?? '',
	sectionTitle: s['legend__section-title'] ?? '',
	entry: s.legend__entry ?? '',
	swatchKind: s['legend__swatch-kind'] ?? '',
	swatchEdge: s['legend__swatch-edge'] ?? '',
	swatchEdgeDashed: s['legend__swatch-edge--dashed'] ?? '',
	swatchLang: s['legend__swatch-lang'] ?? '',
	entryLabel: s['legend__entry-label'] ?? '',
} as const;

interface SwatchEntry {
	readonly value: string;
	readonly label: string;
	readonly color: string;
	/// Renders the edge swatch with a dashed pattern instead of a
	/// solid stroke. Mirrors the canvas-side rendering of
	/// IMPLEMENTS / EXTENDS edges in the focus graph.
	readonly dashed?: boolean;
	readonly short?: string;
}

const KIND_ENTRIES: ReadonlyArray<SwatchEntry> = [
	{ value: 'module', label: 'Module', color: '#b180d7' },
	{ value: 'type', label: 'Type', color: '#cca700' },
	{ value: 'callable', label: 'Callable', color: '#3794ff' },
	{ value: 'value', label: 'Value', color: '#89d185' },
	{ value: 'macro', label: 'Macro', color: '#f48771' },
];

// EXTENDS (~0.03%) and TESTS (0 emitted as of rev 836 — bucket TestedBy
// uses an FQDN heuristic, not extracted edges) are intentionally omitted.
// Re-add when the daemon emits them with material counts.
const EDGE_ENTRIES: ReadonlyArray<SwatchEntry> = [
	{ value: 'CALLS', label: 'Calls', color: '#3794ff' },
	{ value: 'IMPORTS', label: 'Imports', color: '#b180d7' },
	{ value: 'USES_TYPE', label: 'Uses type', color: '#cca700' },
	{ value: 'IMPLEMENTS', label: 'Implements', color: '#f48771', dashed: true },
	{ value: 'REFERENCES', label: 'References', color: '#9d9d9d' },
];

const LANG_ENTRIES: ReadonlyArray<SwatchEntry> = [
	{ value: 'rust', label: 'Rust', color: '#f48771', short: 'rs' },
	{ value: 'typescript', label: 'TypeScript', color: '#3794ff', short: 'ts' },
	{ value: 'javascript', label: 'JavaScript', color: '#cca700', short: 'js' },
	{ value: 'lua', label: 'Lua', color: '#5aa9ff', short: 'lua' },
	{ value: 'c', label: 'C', color: '#9d9d9d', short: 'c' },
	{ value: 'vue', label: 'Vue', color: '#89d185', short: 'vue' },
	{ value: 'svelte', label: 'Svelte', color: '#ff8a3a', short: 'svl' },
];

export class LegendElement extends HTMLElement {
	#mounted = false;
	#visible = true;
	#collapsed = false;
	#nodes: { root: HTMLElement; body: HTMLElement; toggle: HTMLElement } | null = null;

	connectedCallback(): void {
		if (this.#mounted) return;
		this.#mounted = true;
		// Honour HTML attributes for initial state so embedders can
		// `<standardoc-legend collapsed>` without reaching for JS.
		if (this.hasAttribute('collapsed')) this.#collapsed = true;
		if (this.hasAttribute('hidden')) this.#visible = false;
		this.#render();
		this.#sync();
	}

	set visible(value: boolean) {
		if (this.#visible === value) return;
		this.#visible = value;
		this.#sync();
	}

	get visible(): boolean { return this.#visible; }

	set collapsed(value: boolean) {
		if (this.#collapsed === value) return;
		this.#collapsed = value;
		this.#sync();
	}

	get collapsed(): boolean { return this.#collapsed; }

	#render(): void {
		const root = document.createElement('div');
		root.className = C.legend;
		root.innerHTML = `
			<header class="${C.header}" data-role="header">
				<span class="${C.title}">Legend</span>
				<span class="${C.toggle}" data-role="toggle">▾</span>
			</header>
			<div class="${C.body}" data-role="body"></div>
		`;
		this.replaceChildren(root);
		const body = root.querySelector<HTMLElement>('[data-role="body"]')!;
		const header = root.querySelector<HTMLElement>('[data-role="header"]')!;
		const toggle = root.querySelector<HTMLElement>('[data-role="toggle"]')!;
		header.addEventListener('click', () => {
			this.#collapsed = !this.#collapsed;
			this.#sync();
		});
		this.#nodes = { root, body, toggle };
		this.#renderBody();
	}

	#renderBody(): void {
		const n = this.#nodes;
		if (n === null) return;
		const frag = document.createDocumentFragment();
		frag.appendChild(this.#renderSection('Kinds', 'kind', KIND_ENTRIES));
		frag.appendChild(this.#renderSection('Edges', 'edge', EDGE_ENTRIES));
		frag.appendChild(this.#renderSection('Languages', 'language', LANG_ENTRIES));
		n.body.replaceChildren(frag);
	}

	#renderSection(label: string, category: LegendCategory, entries: ReadonlyArray<SwatchEntry>): HTMLElement {
		const section = document.createElement('section');
		section.className = C.section;
		const title = document.createElement('div');
		title.className = C.sectionTitle;
		title.textContent = label;
		section.appendChild(title);
		for (const entry of entries) {
			const row = document.createElement('div');
			row.className = C.entry;
			row.appendChild(this.#renderSwatch(category, entry));
			const lbl = document.createElement('span');
			lbl.className = C.entryLabel;
			lbl.textContent = entry.label;
			row.appendChild(lbl);
			row.addEventListener('click', () => {
				this.dispatchEvent(new CustomEvent<LegendFilterDetail>('sd-legend-filter', {
					detail: { category, value: entry.value },
					bubbles: true,
					composed: true,
				}));
			});
			section.appendChild(row);
		}
		return section;
	}

	#renderSwatch(category: LegendCategory, entry: SwatchEntry): HTMLElement {
		const swatch = document.createElement('span');
		if (category === 'kind') {
			swatch.className = C.swatchKind;
			swatch.style.setProperty('--swatch-color', entry.color);
		} else if (category === 'edge') {
			swatch.className = classigo(C.swatchEdge, entry.dashed === true && C.swatchEdgeDashed);
			swatch.style.setProperty('--swatch-color', entry.color);
		} else {
			swatch.className = C.swatchLang;
			swatch.style.setProperty('--swatch-color', entry.color);
			swatch.textContent = entry.short ?? entry.value;
		}
		return swatch;
	}

	#sync(): void {
		const n = this.#nodes;
		if (n === null) return;
		n.root.className = classigo(C.legend, this.#collapsed && C.collapsed);
		n.root.style.display = this.#visible ? '' : 'none';
		n.toggle.textContent = this.#collapsed ? '▸' : '▾';
	}
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_LEGEND_TAG)) {
	customElements.define(STANDARDOC_LEGEND_TAG, LegendElement);
}

declare global {
	interface HTMLElementTagNameMap {
		[STANDARDOC_LEGEND_TAG]: LegendElement;
	}
}
