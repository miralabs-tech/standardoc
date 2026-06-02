/**
 * `<standardoc-focus-graph>` — wraps the wasm-bindgen
 * `FocusGraphCanvas` for the symbol-local view. Composes a hop selector
 * (1 / 2 / 3 / All) above the canvas and an absolute DOM overlay for
 * edge labels (CALLS / IMPORTS / USES_TYPE / etc.). The overlay is
 * refreshed from `canvas.label_layout()` on each tick: the wasm side
 * emits bucket headers + per-kind edge chips, and an empty payload
 * just clears the overlay.
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
 *   - `sd-focus-bucket-expand`     detail: { bucket, hiddenCount, newCap }
 *   - `sd-focus-graph-error`       detail: { source, message }
 */

import classigo from 'classigo';

import type {
  FocusBucketExpandDetail,
  FocusGraphBackDetail,
  FocusGraphCanvasFacade,
  FocusGraphCanvasFactory,
  FocusGraphErrorDetail,
  FocusGraphHopChangeDetail,
  FocusGraphNodeClickDetail,
  FocusGraphNodeHoverDetail,
  FocusGraphReadyDetail,
} from './focus-graph.type';
import s from './focus-graph.module.scss';
import { escapeHtml } from '../../text';

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
  legend: s.focus__legend ?? '',
  legendEmpty: s['focus__legend--empty'] ?? '',
  legendItem: s['focus__legend-item'] ?? '',
  legendSwatch: s['focus__legend-swatch'] ?? '',
  legendSwatchDashed: s['focus__legend-swatch--dashed'] ?? '',
  breadcrumb: s.focus__breadcrumb ?? '',
  breadcrumbEmpty: s['focus__breadcrumb--empty'] ?? '',
  breadcrumbBack: s['focus__breadcrumb-back'] ?? '',
  breadcrumbSeg: s['focus__breadcrumb-seg'] ?? '',
  breadcrumbSegCurrent: s['focus__breadcrumb-seg--current'] ?? '',
  breadcrumbSep: s['focus__breadcrumb-sep'] ?? '',
} as const;

interface BucketLabel {
  readonly text: string;
  readonly count: number;
  readonly x: number;
  readonly y: number;
}

interface EdgeKindChip {
  readonly text: string;
  readonly color: string;
  /// `true` when the kind renders as a dashed curve on the canvas
  /// (IMPLEMENTS / EXTENDS). The legend swatch mirrors this so the
  /// chip and the actual arrow share the same visual language.
  readonly dashed: boolean;
}

interface OverlayPayload {
  readonly buckets: ReadonlyArray<BucketLabel>;
  readonly edges: ReadonlyArray<EdgeKindChip>;
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
    legend: HTMLElement;
    breadcrumb: HTMLElement;
  } | null = null;
  #hops = 1;
  /// Last FQDN reflected in the breadcrumb — diffed against
  /// `canvas.focus_fqdn` each rAF tick so we only re-render the
  /// breadcrumb when the focal symbol actually changes.
  #breadcrumbFqdn = '';

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
				<div class="${C.breadcrumb} ${C.breadcrumbEmpty}" data-role="breadcrumb"></div>
				<div class="${C.legend} ${C.legendEmpty}" data-role="legend" aria-label="Edge kinds"></div>
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
      legend: root.querySelector<HTMLElement>('[data-role="legend"]')!,
      breadcrumb: root.querySelector<HTMLElement>('[data-role="breadcrumb"]')!,
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
        // `queueMicrotask` defers the DOM dispatch until after the
        // current wasm call returns, dropping the wasm-bindgen mutable
        // borrow before any host listener can re-enter the canvas
        // (e.g. via `set_payload` on a node-click drill). Without this
        // wasm-bindgen panics with "recursive use of an object
        // detected" the moment a click handler calls back into wasm.
        canvas.set_on_node_hover((fqdn: string | null) => {
          queueMicrotask(() => {
            this.dispatchEvent(new CustomEvent<FocusGraphNodeHoverDetail>('sd-focus-graph-node-hover', {
              detail: { fqdn }, bubbles: true, composed: true,
            }));
          });
        });
        canvas.set_on_node_click((fqdn: string) => {
          queueMicrotask(() => {
            this.dispatchEvent(new CustomEvent<FocusGraphNodeClickDetail>('sd-focus-graph-node-click', {
              detail: { fqdn }, bubbles: true, composed: true,
            }));
          });
        });
        canvas.set_on_overflow_click((bucket: string, hiddenCount: number, newCap: number) => {
          queueMicrotask(() => {
            this.dispatchEvent(new CustomEvent<FocusBucketExpandDetail>('sd-focus-bucket-expand', {
              detail: { bucket, hiddenCount, newCap }, bubbles: true, composed: true,
            }));
          });
        });
        // Observe the STAGE wrapper, not the canvas itself. The
        // canvas carries an inline `width: ${px}` set by
        // `apply_canvas_size` (Rust pin to dodge the DPR-bitmap →
        // intrinsic-size resize loop), so it does NOT auto-track its
        // parent. The stage is `flex: 1 1 auto` inside the focus
        // panel, so observing it catches every layout reflow.
        this.#observer = new ResizeObserver(() => this.#syncSize());
        this.#observer.observe(n.stage);
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
    // Use the stage's rect, not the canvas's: the canvas inline size
    // is pinned by `apply_canvas_size` and lags the parent's reflow
    // until we resize it ourselves.
    const rect = n.stage.getBoundingClientRect();
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
    this.#syncBreadcrumb();
    this.#rafHandle = requestAnimationFrame(() => this.#loop());
  }

  #syncBreadcrumb(): void {
    const n = this.#nodes;
    if (n === null || this.#canvas === null) return;
    const fqdn = this.#canvas.focus_fqdn;
    if (fqdn === this.#breadcrumbFqdn) return;
    this.#breadcrumbFqdn = fqdn;
    if (fqdn === '') {
      n.breadcrumb.replaceChildren();
      n.breadcrumb.classList.add(C.breadcrumbEmpty);
      return;
    }
    // FQDN segments — Rust / TS / Lua all use `::` as a path
    // separator in our IR. Last segment is the symbol name itself
    // (highlighted current), earlier segments are the module / file
    // path leading to it.
    const segments = fqdn.split('::').filter(s => s.length > 0);
    if (segments.length === 0) {
      n.breadcrumb.replaceChildren();
      n.breadcrumb.classList.add(C.breadcrumbEmpty);
      return;
    }
    const frag = document.createDocumentFragment();
    // Back button — mirrors the overview's `← Workspace` affordance.
    // Emits `sd-focus-graph-back` so the host can decide what "back"
    // means (pop history, navigate to parent module, switch to the
    // workspace view, …).
    const back = document.createElement('button');
    back.type = 'button';
    back.className = C.breadcrumbBack;
    back.textContent = '← Back';
    back.title = 'Back';
    back.addEventListener('click', () => {
      this.dispatchEvent(new CustomEvent<FocusGraphBackDetail>('sd-focus-graph-back', {
        detail: { from: this.#breadcrumbFqdn },
        bubbles: true,
        composed: true,
      }));
    });
    frag.appendChild(back);
    segments.forEach((seg, i) => {
      const sep = document.createElement('span');
      sep.className = C.breadcrumbSep;
      sep.textContent = '›';
      frag.appendChild(sep);
      const segEl = document.createElement('span');
      const isLast = i === segments.length - 1;
      segEl.className = isLast
        ? classigo(C.breadcrumbSeg, C.breadcrumbSegCurrent)
        : C.breadcrumbSeg;
      segEl.textContent = seg;
      frag.appendChild(segEl);
    });
    n.breadcrumb.replaceChildren(frag);
    n.breadcrumb.classList.remove(C.breadcrumbEmpty);
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
    // Bucket headers — DOM overlay positioned in camera space.
    if (parsed.buckets.length === 0) {
      if (n.overlay.childNodes.length > 0) n.overlay.replaceChildren();
    } else {
      const frag = document.createDocumentFragment();
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
    // Mini-legend — pinned top-left, one chip per unique edge kind
    // present in the current focus view. The chip itself is coloured
    // via `color: <kind-color>`; both the swatch (currentColor) and
    // the border inherit, so a single style assignment paints both.
    if (parsed.edges.length === 0) {
      if (n.legend.childNodes.length > 0) n.legend.replaceChildren();
      n.legend.classList.add(C.legendEmpty);
    } else {
      const frag = document.createDocumentFragment();
      for (const chip of parsed.edges) {
        const el = document.createElement('span');
        el.className = C.legendItem;
        el.style.color = chip.color;
        const swatch = document.createElement('span');
        swatch.className = chip.dashed
          ? classigo(C.legendSwatch, C.legendSwatchDashed)
          : C.legendSwatch;
        const label = document.createElement('span');
        label.textContent = chip.text;
        label.style.color = 'var(--sd-fg, #cccccc)';
        el.append(swatch, label);
        frag.appendChild(el);
      }
      n.legend.replaceChildren(frag);
      n.legend.classList.remove(C.legendEmpty);
    }
  }
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
  FocusBucketExpandDetail,
  FocusGraphBackDetail,
  FocusGraphNodeClickDetail,
  FocusGraphNodeHoverDetail,
};
