/**
 * `<standardoc-focus-graph>` — wraps the wasm-bindgen
 * `FocusGraphCanvas` for the symbol-local view. Composes a hop selector
 * (1 / 2 / 3 / All) above the canvas and an absolute DOM overlay for
 * edge labels (CALLS / IMPORTS / USES_TYPE / etc.). The overlay is
 * refreshed from `canvas.label_layout()` on each tick — Phase 3a
 * returns `[]` so the overlay stays empty until Phase 3b populates it.
 *
 * Owns:
 *   - canvas + overlay div
 *   - pointer/wheel/resize events
 *   - rAF loop
 *   - hop-selector chip state
 *
 * Does NOT own:
 *   - WASM init (host provides `canvasFactory`)
 *   - payload (host calls `el.canvas.set_payload(json)` via the
 *     `.canvas` getter once `sd-focus-graph-ready` fires)
 *
 * Events emitted:
 *   - `sd-focus-graph-ready`       detail: { canvas }
 *   - `sd-focus-graph-node-hover`  detail: { fqdn | null }
 *   - `sd-focus-graph-node-click`  detail: { fqdn }
 *   - `sd-focus-graph-hop-change`  detail: { hops }
 *   - `sd-focus-graph-error`       detail: { source, message }
 */

import classigo from 'classigo';

import type {
  FocusGraphCanvasFacade,
  FocusGraphCanvasFactory,
  FocusGraphErrorDetail,
  FocusGraphHopChangeDetail,
  FocusGraphNodeClickDetail,
  FocusGraphNodeHoverDetail,
  FocusGraphReadyDetail,
} from './focus-graph.type';
import s from './focus-graph.module.scss';

export const STANDARDOC_FOCUS_GRAPH_TAG = 'standardoc-focus-graph';

const C = {
  focus: s.focus ?? '',
  toolbar: s.focus__toolbar ?? '',
  title: s.focus__title ?? '',
  hops: s.focus__hops ?? '',
  hop: s.focus__hop ?? '',
  hopActive: s['focus__hop--active'] ?? '',
  stage: s.focus__stage ?? '',
  stageGrabbing: s['focus__stage--grabbing'] ?? '',
  canvas: s.focus__canvas ?? '',
  overlay: s.focus__overlay ?? '',
  label: s.focus__label ?? '',
  edgeLabel: s['focus__edge-label'] ?? '',
} as const;

interface BucketLabel {
  readonly text: string;
  readonly count: number;
  readonly x: number;
  readonly y: number;
}

interface EdgeLabel {
  readonly text: string;
  readonly color: string;
  readonly x: number;
  readonly y: number;
}

interface OverlayPayload {
  readonly buckets: ReadonlyArray<BucketLabel>;
  readonly edges: ReadonlyArray<EdgeLabel>;
}

const HOPS: ReadonlyArray<{ hops: number; label: string }> = [
  { hops: 1, label: '1 hop' },
  { hops: 2, label: '2 hops' },
  { hops: 3, label: '3 hops' },
  { hops: 0, label: 'All' },
];

export class FocusGraphElement extends HTMLElement {
  #mounted = false;
  #initStarted = false;
  #canvas: FocusGraphCanvasFacade | null = null;
  #factory: FocusGraphCanvasFactory | null = null;
  #observer: ResizeObserver | null = null;
  #rafHandle: number | null = null;
  #nodes: {
    root: HTMLElement;
    toolbar: HTMLElement;
    stage: HTMLElement;
    canvas: HTMLCanvasElement;
    overlay: HTMLElement;
  } | null = null;
  #hops = 1;

  set canvasFactory(factory: FocusGraphCanvasFactory) {
    this.#factory = factory;
    this.#tryInit();
  }

  get canvas(): FocusGraphCanvasFacade | null {
    return this.#canvas;
  }

  get hops(): number {
    return this.#hops;
  }

  set hops(next: number) {
    if (next === this.#hops) return;
    this.#hops = next;
    this.#syncHopChips();
    this.#canvas?.set_hop_count(next);
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
    root.className = C.focus;
    root.innerHTML = `
			<div class="${C.toolbar}">
				<span class="${C.title}">Focus graph</span>
				<div class="${C.hops}" role="group" aria-label="Hop count">
					${HOPS.map(h => `<button type="button" class="${C.hop}" data-hops="${h.hops}">${escapeHtml(h.label)}</button>`).join('')}
				</div>
			</div>
			<div class="${C.stage}" data-role="stage">
				<canvas class="${C.canvas}" data-role="canvas"></canvas>
				<div class="${C.overlay}" data-role="overlay" aria-hidden="true"></div>
			</div>
		`;
    this.replaceChildren(root);
    this.#nodes = {
      root,
      toolbar: root.querySelector<HTMLElement>(`.${C.toolbar}`)!,
      stage: root.querySelector<HTMLElement>('[data-role="stage"]')!,
      canvas: root.querySelector<HTMLCanvasElement>('[data-role="canvas"]')!,
      overlay: root.querySelector<HTMLElement>('[data-role="overlay"]')!,
    };
    this.#syncHopChips();
    this.#wireHopChips();
    this.#wirePointer();
  }

  #wireHopChips(): void {
    const n = this.#nodes;
    if (n === null) return;
    n.toolbar.querySelectorAll<HTMLButtonElement>('[data-hops]').forEach(btn => {
      btn.addEventListener('click', () => {
        const v = Number(btn.dataset.hops);
        if (Number.isNaN(v)) return;
        this.hops = v;
        this.dispatchEvent(new CustomEvent<FocusGraphHopChangeDetail>('sd-focus-graph-hop-change', {
          detail: { hops: v }, bubbles: true, composed: true,
        }));
      });
    });
  }

  #syncHopChips(): void {
    const n = this.#nodes;
    if (n === null) return;
    n.toolbar.querySelectorAll<HTMLButtonElement>('[data-hops]').forEach(btn => {
      const v = Number(btn.dataset.hops);
      btn.className = classigo(C.hop, v === this.#hops && C.hopActive);
    });
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
      n.canvas.setPointerCapture(e.pointerId);
      n.stage.className = classigo(C.stage, C.stageGrabbing);
      this.#canvas.on_pointer_down(x, y, e.button);
    });
    n.canvas.addEventListener('pointerup', e => {
      if (this.#canvas === null) return;
      const { x, y } = this.#cssCoords(e);
      try { n.canvas.releasePointerCapture(e.pointerId); } catch { /* noop */ }
      n.stage.className = C.stage;
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
        canvas.set_hop_count(this.#hops);
        canvas.set_on_node_hover((fqdn: string | null) => {
          this.dispatchEvent(new CustomEvent<FocusGraphNodeHoverDetail>('sd-focus-graph-node-hover', {
            detail: { fqdn }, bubbles: true, composed: true,
          }));
        });
        canvas.set_on_node_click((fqdn: string) => {
          this.dispatchEvent(new CustomEvent<FocusGraphNodeClickDetail>('sd-focus-graph-node-click', {
            detail: { fqdn }, bubbles: true, composed: true,
          }));
        });
        this.#observer = new ResizeObserver(() => this.#syncSize());
        this.#observer.observe(n.canvas);
        this.dispatchEvent(new CustomEvent<FocusGraphReadyDetail>('sd-focus-graph-ready', {
          detail: { canvas }, bubbles: true, composed: true,
        }));
        this.#loop();
      })
      .catch((e: unknown) => {
        const message = e instanceof Error ? e.message : String(e);
        this.dispatchEvent(new CustomEvent<FocusGraphErrorDetail>('sd-focus-graph-error', {
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
    this.#syncOverlay();
    this.#rafHandle = requestAnimationFrame(() => this.#loop());
  }

  #syncOverlay(): void {
    const n = this.#nodes;
    if (n === null || this.#canvas === null) return;
    const raw = this.#canvas.label_layout();
    let parsed: OverlayPayload = { buckets: [], edges: [] };
    try {
      const obj = JSON.parse(raw) as OverlayPayload;
      if (obj && Array.isArray(obj.buckets) && Array.isArray(obj.edges)) parsed = obj;
    } catch {
      return;
    }
    if (parsed.buckets.length === 0 && parsed.edges.length === 0) {
      if (n.overlay.childNodes.length > 0) n.overlay.replaceChildren();
      return;
    }
    const frag = document.createDocumentFragment();
    // Edge labels first (lower z visually) so bucket pills land on
    // top if they ever overlap.
    for (const l of parsed.edges) {
      const el = document.createElement('span');
      el.className = C.edgeLabel;
      el.textContent = l.text;
      el.style.left = `${l.x}px`;
      el.style.top = `${l.y}px`;
      el.style.color = l.color;
      el.style.borderColor = l.color;
      frag.appendChild(el);
    }
    for (const l of parsed.buckets) {
      const el = document.createElement('span');
      el.className = C.label;
      el.textContent = `${l.text} (${l.count})`;
      el.style.left = `${l.x}px`;
      el.style.top = `${l.y}px`;
      frag.appendChild(el);
    }
    n.overlay.replaceChildren(frag);
  }
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_FOCUS_GRAPH_TAG)) {
  customElements.define(STANDARDOC_FOCUS_GRAPH_TAG, FocusGraphElement);
}

declare global {
  interface HTMLElementTagNameMap {
    [STANDARDOC_FOCUS_GRAPH_TAG]: FocusGraphElement;
  }
}

export type {
  FocusGraphNodeClickDetail,
  FocusGraphNodeHoverDetail,
};
