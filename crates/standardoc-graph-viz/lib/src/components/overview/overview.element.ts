/**
 * `<standardoc-overview>` — wraps the wasm-bindgen `OverviewCanvas`
 * for the workspace-level nebula view. Mirrors the pattern of
 * `<standardoc-graph>` but driven by the new Phase 3 cluster/edge
 * payload shape rather than the legacy GraphEngine API.
 *
 * Owns:
 *   - the canvas element
 *   - pointer/wheel/resize events with CSS-pixel coords
 *   - rAF loop while the engine is mounted
 *
 * Does NOT own:
 *   - WASM init (host provides `canvasFactory`)
 *   - cluster/edge data (host calls `el.canvas.set_payload(json)`
 *     via the `.canvas` getter once `sd-overview-ready` fires)
 *
 * Events emitted:
 *   - `sd-overview-ready`         detail: { canvas }
 *   - `sd-overview-cluster-hover` detail: { clusterId | null }
 *   - `sd-overview-cluster-click` detail: { clusterId }
 *   - `sd-overview-error`         detail: { source, message }
 */

import classigo from 'classigo';

import type {
	OverviewCanvasFacade,
	OverviewCanvasFactory,
	OverviewClusterClickDetail,
	OverviewClusterHoverDetail,
	OverviewErrorDetail,
	OverviewReadyDetail,
} from './overview.type';
import s from './overview.module.scss';

export const STANDARDOC_OVERVIEW_TAG = 'standardoc-overview';

const C = {
	overview: s.overview ?? '',
	grabbing: s['overview--grabbing'] ?? '',
	canvas: s.overview__canvas ?? '',
} as const;

export class OverviewElement extends HTMLElement {
	#mounted = false;
	#initStarted = false;
	#canvas: OverviewCanvasFacade | null = null;
	#factory: OverviewCanvasFactory | null = null;
	#observer: ResizeObserver | null = null;
	#rafHandle: number | null = null;
	#nodes: {
		root: HTMLElement;
		canvas: HTMLCanvasElement;
	} | null = null;
	#dragging = false;

	set canvasFactory(factory: OverviewCanvasFactory) {
		this.#factory = factory;
		this.#tryInit();
	}

	get canvas(): OverviewCanvasFacade | null {
		return this.#canvas;
	}

	connectedCallback(): void {
		if (this.#mounted) return;
		this.#mounted = true;
		this.#render();
		this.#tryInit();
	}

	disconnectedCallback(): void {
		if (this.#rafHandle !== null) {
			cancelAnimationFrame(this.#rafHandle);
			this.#rafHandle = null;
		}
		this.#observer?.disconnect();
		this.#observer = null;
	}

	#render(): void {
		const root = document.createElement('div');
		root.className = C.overview;
		const canvas = document.createElement('canvas');
		canvas.className = C.canvas;
		root.appendChild(canvas);
		this.replaceChildren(root);
		this.#nodes = { root, canvas };
		this.#wirePointer();
	}

	#wirePointer(): void {
		const n = this.#nodes;
		if (n === null) return;
		n.canvas.addEventListener('pointermove', e => {
			if (this.#canvas === null) return;
			const { x, y } = this.#cssCoords(e);
			this.#canvas.on_pointer_move(x, y);
		});
		n.canvas.addEventListener('pointerdown', e => {
			if (this.#canvas === null) return;
			const { x, y } = this.#cssCoords(e);
			this.#dragging = true;
			n.canvas.setPointerCapture(e.pointerId);
			n.root.className = classigo(C.overview, C.grabbing);
			this.#canvas.on_pointer_down(x, y, e.button);
		});
		n.canvas.addEventListener('pointerup', e => {
			if (this.#canvas === null) return;
			const { x, y } = this.#cssCoords(e);
			this.#dragging = false;
			try { n.canvas.releasePointerCapture(e.pointerId); } catch { /* noop */ }
			n.root.className = C.overview;
			this.#canvas.on_pointer_up(x, y, e.button);
		});
		n.canvas.addEventListener('pointerleave', () => {
			if (this.#canvas === null) return;
			this.#canvas.on_pointer_leave();
		});
		n.canvas.addEventListener('wheel', e => {
			if (this.#canvas === null) return;
			e.preventDefault();
			const { x, y } = this.#cssCoords(e);
			this.#canvas.on_wheel(x, y, e.deltaY);
		}, { passive: false });
	}

	#cssCoords(e: PointerEvent | WheelEvent): { x: number; y: number } {
		const n = this.#nodes;
		if (n === null) return { x: 0, y: 0 };
		const r = n.canvas.getBoundingClientRect();
		return { x: e.clientX - r.left, y: e.clientY - r.top };
	}

	#tryInit(): void {
		if (this.#initStarted) return;
		if (this.#nodes === null || this.#factory === null) return;
		this.#initStarted = true;
		const n = this.#nodes;
		const rect = n.canvas.getBoundingClientRect();
		const w = Math.max(1, Math.round(rect.width || 320));
		const h = Math.max(1, Math.round(rect.height || 240));
		const dpr = Math.max(1, Math.round(window.devicePixelRatio || 1));
		void Promise.resolve(this.#factory(n.canvas, w, h, dpr))
			.then(canvas => {
				this.#canvas = canvas;
				this.#observer = new ResizeObserver(() => this.#syncSize());
				this.#observer.observe(n.canvas);
				this.dispatchEvent(new CustomEvent<OverviewReadyDetail>('sd-overview-ready', {
					detail: { canvas }, bubbles: true, composed: true,
				}));
				this.#loop();
			})
			.catch((e: unknown) => {
				const message = e instanceof Error ? e.message : String(e);
				this.dispatchEvent(new CustomEvent<OverviewErrorDetail>('sd-overview-error', {
					detail: { source: 'canvas-init', message }, bubbles: true, composed: true,
				}));
			});
	}

	#syncSize(): void {
		const n = this.#nodes;
		if (n === null || this.#canvas === null) return;
		const rect = n.canvas.getBoundingClientRect();
		const w = Math.max(1, Math.round(rect.width));
		const h = Math.max(1, Math.round(rect.height));
		this.#canvas.resize(w, h);
		const dpr = Math.max(1, Math.round(window.devicePixelRatio || 1));
		this.#canvas.set_device_pixel_ratio(dpr);
	}

	#loop(): void {
		if (this.#canvas === null) return;
		this.#canvas.tick();
		this.#rafHandle = requestAnimationFrame(() => this.#loop());
	}
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_OVERVIEW_TAG)) {
	customElements.define(STANDARDOC_OVERVIEW_TAG, OverviewElement);
}

declare global {
	interface HTMLElementTagNameMap {
		[STANDARDOC_OVERVIEW_TAG]: OverviewElement;
	}
}

// Re-export click/hover detail types for host convenience.
export type {
	OverviewClusterClickDetail,
	OverviewClusterHoverDetail,
};
