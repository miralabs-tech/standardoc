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
 * The element itself is logic-free — pure styling host wrapped as a
 * custom element so the lib's barrel + tag-map typings stay uniform.
 * Spawnable popup panels (Field Details / Callers Graph / Source View)
 * live outside this grid and are layered absolutely; that surface is
 * Phase 4.
 *
 * No attributes, no events. Override `--sd-shell-grid-cols` /
 * `--sd-shell-grid-rows` on the host to retheme the layout proportions.
 */

import s from './panel-layout.module.scss';

export const STANDARDOC_PANEL_LAYOUT_TAG = 'standardoc-panel-layout';

const C = {
	layout: s.layout ?? '',
} as const;

export class PanelLayoutElement extends HTMLElement {
	#mounted = false;

	connectedCallback(): void {
		if (this.#mounted) return;
		this.#mounted = true;
		this.classList.add(C.layout);
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
