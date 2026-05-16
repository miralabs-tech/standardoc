/**
 * `<standardoc-graph>` — the canvas-and-engine shell.
 *
 * Owns:
 *   - two stacked canvases (Canvas2D + WebGPU)
 *   - pointer/wheel/resize events with CSS-pixel coord math
 *   - mode switching (`canvas2d` / `webgpu`) with the engineBusy gate
 *     that protects the wasm-bindgen `&mut self` borrow during the
 *     async `enable_webgpu` round-trip
 *   - lazy WebGPU init (no GPU resources allocated until the host
 *     requests `setMode('webgpu')`)
 *   - localStorage mode persistence so the user picks up where they
 *     left off across reloads
 *
 * Does NOT own:
 *   - the WASM module init (host provides a factory)
 *   - graph data (host calls `engine.load_graph(json)` directly via
 *     the `.engine` getter)
 *   - MCP / network / detail-panel rendering
 *
 * Events emitted:
 *   - `sd-graph-ready`        detail: { engine }     — engine wired
 *   - `sd-graph-hover`        detail: { fqdn | null } — node hover change
 *   - `sd-graph-click`        detail: { fqdn }       — node click
 *   - `sd-graph-mode-change`  detail: { mode }       — mode flipped
 *   - `sd-graph-error`        detail: { source, message } — recoverable failure
 */

import classigo from 'classigo';
import { matcher } from 'matchigo';

import type { RenderMode } from '../../types';
import type {
	GraphEngineFacade,
	GraphEngineFactory,
	GraphErrorSource,
	GraphErrorDetail,
	GraphHoverDetail,
	GraphClickDetail,
	GraphModeChangeDetail,
	GraphReadyDetail,
} from './graph.type';
import s from './graph.module.scss';

export const STANDARDOC_GRAPH_TAG = 'standardoc-graph';

const MODE_STORAGE_KEY = 'standardoc-graph-viz:mode';

const C = {
	graph: s.graph ?? '',
	stack: s.graph__canvas_stack ?? '',
	stackGrabbing: s['graph__canvas_stack--grabbing'] ?? '',
	canvas: s.graph__canvas ?? '',
	canvas3d: s['graph__canvas--3d'] ?? '',
} as const;

// `string | null` → typed RenderMode | null, exhaustive on the
// localStorage payload string. Hoisted per the matchigo AI contract.
const parsePersistedMode = matcher<string | null, RenderMode | null>()
	.with('canvas2d', () => 'canvas2d' as const)
	.with('webgpu', () => 'webgpu' as const)
	.otherwise(null);

function readPersistedMode(): RenderMode | null {
	try {
		return parsePersistedMode(localStorage.getItem(MODE_STORAGE_KEY));
	} catch {
		return null;
	}
}

function persistMode(mode: RenderMode): void {
	try {
		localStorage.setItem(MODE_STORAGE_KEY, mode);
	} catch {
		// Storage may be disabled (private mode, sandboxed iframe).
		// The mode toggle still works for the session.
	}
}

function clearPersistedMode(): void {
	try {
		localStorage.removeItem(MODE_STORAGE_KEY);
	} catch {
		// Same rationale as `persistMode`.
	}
}

export class GraphElement extends HTMLElement {
	#mounted = false;
	#initStarted = false;
	#engine: GraphEngineFacade | null = null;
	#factory: GraphEngineFactory | null = null;
	#engineBusy = false;
	#currentMode: RenderMode = 'canvas2d';
	#webgpuAvailable = false;
	#webgpuInitialised = false;
	#nodes: {
		root: HTMLElement;
		stack: HTMLElement;
		canvas2d: HTMLCanvasElement;
		canvas3d: HTMLCanvasElement;
	} | null = null;
	#ro: ResizeObserver | null = null;

	connectedCallback(): void {
		if (this.#mounted) return;
		this.#mounted = true;
		this.#render();
		if (this.#factory !== null) void this.#init();
	}

	disconnectedCallback(): void {
		this.#ro?.disconnect();
		this.#ro = null;
	}

	set engineFactory(factory: GraphEngineFactory) {
		this.#factory = factory;
		if (this.#mounted && !this.#initStarted) void this.#init();
	}

	get engine(): GraphEngineFacade | null {
		return this.#engine;
	}

	get engineBusy(): boolean {
		return this.#engineBusy;
	}

	get currentMode(): RenderMode {
		return this.#currentMode;
	}

	get webgpuAvailable(): boolean {
		return this.#webgpuAvailable;
	}

	async setMode(mode: RenderMode): Promise<void> {
		const engine = this.#engine;
		const n = this.#nodes;
		if (engine === null || n === null) return;
		if (mode === this.#currentMode) return;

		if (mode === 'webgpu') {
			if (!this.#webgpuAvailable) return;
			if (!this.#webgpuInitialised) {
				this.#engineBusy = true;
				try {
					await engine.enable_webgpu(n.canvas3d);
					this.#webgpuInitialised = true;
				} catch (e) {
					this.#emitError('webgpu-init', e);
					this.#webgpuAvailable = false;
					// Drop the persisted webgpu mode so the next reload
					// doesn't immediately re-fire the failing init.
					clearPersistedMode();
					return;
				} finally {
					this.#engineBusy = false;
				}
			}
		}

		try {
			engine.set_mode(mode);
		} catch (e) {
			this.#emitError('set-mode', e);
			return;
		}

		this.#currentMode = mode;
		this.#applyCanvasVisibility(mode);
		persistMode(mode);
		this.dispatchEvent(new CustomEvent<GraphModeChangeDetail>('sd-graph-mode-change', {
			detail: { mode },
			bubbles: true,
			composed: true,
		}));
	}

	#render(): void {
		const root = document.createElement('div');
		root.className = C.graph;

		root.innerHTML = `
			<div class="${C.stack}" data-role="stack">
				<canvas class="${C.canvas}" data-layer="2d" data-role="canvas-2d"></canvas>
				<canvas class="${classigo(C.canvas, C.canvas3d)}" data-layer="3d" data-role="canvas-3d" hidden></canvas>
			</div>
		`;

		this.replaceChildren(root);

		this.#nodes = {
			root,
			stack: root.querySelector<HTMLElement>('[data-role="stack"]')!,
			canvas2d: root.querySelector<HTMLCanvasElement>('[data-role="canvas-2d"]')!,
			canvas3d: root.querySelector<HTMLCanvasElement>('[data-role="canvas-3d"]')!,
		};
	}

	async #init(): Promise<void> {
		if (this.#initStarted) return;
		if (this.#factory === null || this.#nodes === null) return;
		this.#initStarted = true;

		// Probe the WebGPU adapter at boot — cheap, no device creation
		// yet. A null adapter (browser exposes the API without GPU
		// support) lets the host disable the 3D toggle before the user
		// ever clicks it.
		if (typeof navigator !== 'undefined' && 'gpu' in navigator) {
			try {
				const adapter = await navigator.gpu.requestAdapter();
				this.#webgpuAvailable = adapter !== null;
			} catch {
				this.#webgpuAvailable = false;
			}
		}

		const n = this.#nodes;
		const dpr = window.devicePixelRatio || 1;
		try {
			this.#engine = await this.#factory(n.canvas2d, n.canvas2d.clientWidth, n.canvas2d.clientHeight, dpr);
		} catch (e) {
			this.#initStarted = false;
			this.#emitError('engine-init', e);
			return;
		}

		// Re-emit engine hover/click callbacks as DOM events so the host
		// can listen without coupling to the wasm-bindgen callback API.
		this.#engine.set_on_node_hover(fqdn => {
			this.dispatchEvent(new CustomEvent<GraphHoverDetail>('sd-graph-hover', {
				detail: { fqdn },
				bubbles: true,
				composed: true,
			}));
		});
		this.#engine.set_on_node_click(fqdn => {
			this.dispatchEvent(new CustomEvent<GraphClickDetail>('sd-graph-click', {
				detail: { fqdn },
				bubbles: true,
				composed: true,
			}));
		});

		this.#wirePointerEvents();
		this.#wireResizeObserver();

		this.dispatchEvent(new CustomEvent<GraphReadyDetail>('sd-graph-ready', {
			detail: { engine: this.#engine },
			bubbles: true,
			composed: true,
		}));

		// Apply persisted mode AFTER the ready event so the host has
		// already wired the data layer (palette + symbols load).
		// `setMode` honours engineBusy, so the boot-time WebGPU init
		// is gated even before the host calls anything.
		const persisted = readPersistedMode();
		if (persisted === 'webgpu' && this.#webgpuAvailable) {
			void this.setMode('webgpu');
		}
	}

	#applyCanvasVisibility(mode: RenderMode): void {
		const n = this.#nodes;
		if (n === null) return;
		const is3d = mode === 'webgpu';
		n.canvas2d.hidden = is3d;
		n.canvas3d.hidden = !is3d;
	}

	#wirePointerEvents(): void {
		const n = this.#nodes;
		if (n === null) return;
		const stack = n.stack;

		const rectAt = (e: MouseEvent): { x: number; y: number } => {
			const r = stack.getBoundingClientRect();
			return { x: e.clientX - r.left, y: e.clientY - r.top };
		};

		stack.addEventListener('pointermove', e => {
			if (this.#engineBusy || this.#engine === null) return;
			const { x, y } = rectAt(e);
			this.#engine.on_pointer_move(x, y);
		});
		stack.addEventListener('pointerdown', e => {
			if (this.#engineBusy || this.#engine === null) return;
			if (e.button === 0) stack.classList.add(C.stackGrabbing);
			const { x, y } = rectAt(e);
			this.#engine.on_pointer_down(x, y, e.button);
		});
		stack.addEventListener('pointerup', e => {
			stack.classList.remove(C.stackGrabbing);
			if (this.#engineBusy || this.#engine === null) return;
			const { x, y } = rectAt(e);
			this.#engine.on_pointer_up(x, y, e.button);
		});
		stack.addEventListener('pointerleave', () => {
			stack.classList.remove(C.stackGrabbing);
			if (this.#engineBusy || this.#engine === null) return;
			this.#engine.on_pointer_leave();
		});
		stack.addEventListener(
			'wheel',
			e => {
				e.preventDefault();
				if (this.#engineBusy || this.#engine === null) return;
				const { x, y } = rectAt(e);
				this.#engine.on_wheel(x, y, e.deltaY);
			},
			{ passive: false },
		);
	}

	#wireResizeObserver(): void {
		const n = this.#nodes;
		if (n === null) return;
		this.#ro = new ResizeObserver(entries => {
			if (this.#engineBusy || this.#engine === null) return;
			for (const entry of entries) {
				const w = Math.max(1, Math.floor(entry.contentRect.width));
				const h = Math.max(1, Math.floor(entry.contentRect.height));
				this.#engine.resize(w, h);
			}
		});
		this.#ro.observe(n.stack);
	}

	#emitError(source: GraphErrorSource, e: unknown): void {
		const message = e instanceof Error ? e.message : String(e);
		this.dispatchEvent(new CustomEvent<GraphErrorDetail>('sd-graph-error', {
			detail: { source, message },
			bubbles: true,
			composed: true,
		}));
	}
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_GRAPH_TAG)) {
	customElements.define(STANDARDOC_GRAPH_TAG, GraphElement);
}

declare global {
	interface HTMLElementTagNameMap {
		[STANDARDOC_GRAPH_TAG]: GraphElement;
	}
}
