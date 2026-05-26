/**
 * `<standardoc-toolbar>` — top bar with brand, status badge, render-mode
 * toggle (2D/3D), externals checkbox, and fit/reset/refetch buttons.
 *
 * The element is intentionally declarative: state lives on attributes
 * (`mode`, `status-kind`, `status-text`, `externals`, `webgpu-available`)
 * and intent flows out as `CustomEvent`s. The host wires the events
 * back to the engine and updates the attributes once the engine
 * confirms the state change.
 *
 * Events emitted:
 *   - `sd-mode-request`   detail: { mode: RenderMode }
 *   - `sd-fit`            no detail
 *   - `sd-reset-zoom`     no detail
 *   - `sd-refetch`        no detail
 *   - `sd-externals-change` detail: { value: boolean }
 */

import classigo from 'classigo';
import { matcher } from 'matchigo';

import type { RenderMode, StatusKind } from '../../types';
import type { ToolbarFlagChangeDetail, ToolbarModeRequestDetail } from './toolbar.type';
import s from './toolbar.module.scss';

export const STANDARDOC_TOOLBAR_TAG = 'standardoc-toolbar';

// Module-scope class lookup. Each SCSS-modules import returns a
// `Record<string, string | undefined>`; resolving everything once here
// keeps the render path allocation-free and makes the missing-class
// fallback (`?? ''`) live in a single place.
const C = {
  toolbar: s.toolbar ?? '',
  brand: s.toolbar__brand ?? '',
  status: s.toolbar__status ?? '',
  statusOk: s['toolbar__status--ok'] ?? '',
  statusLoading: s['toolbar__status--loading'] ?? '',
  statusErr: s['toolbar__status--err'] ?? '',
  spacer: s.toolbar__spacer ?? '',
  group: s.toolbar__group ?? '',
  seg: s.toolbar__seg ?? '',
  segActive: s['toolbar__seg--active'] ?? '',
  btn: s.toolbar__btn ?? '',
  toggle: s.toolbar__toggle ?? '',
  hint: s.toolbar__hint ?? '',
} as const;

// Status modifier dispatch — hoisted at module scope, lazy-compiled on
// first call, then O(1) literal Map dispatch per the matchigo docs.
// Compile-time exhaustiveness over `StatusKind` so a future variant
// addition fails the typecheck instead of silently falling through.
const statusModifier = matcher<StatusKind, string>()
  .with('booting', () => '')
  .with('ready', () => C.statusOk)
  .with('loading', () => C.statusLoading)
  .with('error', () => C.statusErr)
  .exhaustive();

// Fallback status label, also hoisted. The previous version rebuilt the
// matcher on every call — the explicit anti-pattern from the matchigo
// readme that defeats the compile cache and triggers a dev-time warn.
const statusFallbackText = matcher<StatusKind, string>()
  .with('booting', () => 'booting…')
  .with('ready', () => 'ready')
  .with('loading', () => 'loading…')
  .with('error', () => 'error')
  .exhaustive();

// External-source attribute parsers — narrow `string | null` into the
// strict enums via matchigo's literal dispatch with `.otherwise()`
// fallback. Both hoisted, both compile-time exhaustive on the success
// branches, both O(1) literal Map. Replaces the previous hand-rolled
// if/else helpers that the matchigo AI usage contract flags as
// "simulating pattern matching manually".
const parseModeAttr = matcher<string | null, RenderMode>()
  .with('webgpu', () => 'webgpu' as const)
  .with('canvas2d', () => 'canvas2d' as const)
  .otherwise('canvas2d');

const parseStatusKind = matcher<string | null, StatusKind>()
  .with('ready', () => 'ready' as const)
  .with('loading', () => 'loading' as const)
  .with('error', () => 'error' as const)
  .with('booting', () => 'booting' as const)
  .otherwise('booting');


export class ToolbarElement extends HTMLElement {
  static readonly observedAttributes = [
    'mode',
    'status-kind',
    'status-text',
    'externals',
    'webgpu-available',
  ] as const;

  #mounted = false;

  #nodes: {
    root: HTMLElement;
    status: HTMLElement;
    segCanvas: HTMLButtonElement;
    segGpu: HTMLButtonElement;
    externals: HTMLInputElement;
    fit: HTMLButtonElement;
    reset: HTMLButtonElement;
    refetch: HTMLButtonElement;
  } | null = null;

  connectedCallback(): void {
    if (this.#mounted) return;
    this.#mounted = true;
    this.#render();
    this.#syncFromAttributes();
  }

  attributeChangedCallback(): void {
    if (!this.#mounted) return;
    this.#syncFromAttributes();
  }

  #render(): void {
    const root = document.createElement('div');
    root.className = C.toolbar;

    root.innerHTML = `
			<strong class="${C.brand}">standardoc-graph-viz</strong>
			<span class="${C.status}" data-role="status">booting…</span>
			<span class="${C.spacer}"></span>
			<span class="${C.group}" role="group" aria-label="Render mode">
				<button type="button" class="${C.seg}" data-role="seg-canvas" data-mode="canvas2d" aria-pressed="true">2D</button>
				<button type="button" class="${C.seg}" data-role="seg-gpu" data-mode="webgpu" aria-pressed="false">3D</button>
			</span>
			<label class="${C.toggle}">
				<input type="checkbox" data-role="externals" />
				externals
			</label>
			<button type="button" class="${C.btn}" data-role="refetch">Refetch graph</button>
			<button type="button" class="${C.btn}" data-role="fit">⤢ Fit</button>
			<button type="button" class="${C.btn}" data-role="reset">1×</button>
			<span class="${C.hint}">drag to pan · wheel to zoom · hover a chip for edges</span>
		`;

    this.replaceChildren(root);

    this.#nodes = {
      root,
      status: root.querySelector<HTMLElement>('[data-role="status"]')!,
      segCanvas: root.querySelector<HTMLButtonElement>('[data-role="seg-canvas"]')!,
      segGpu: root.querySelector<HTMLButtonElement>('[data-role="seg-gpu"]')!,
      externals: root.querySelector<HTMLInputElement>('[data-role="externals"]')!,
      fit: root.querySelector<HTMLButtonElement>('[data-role="fit"]')!,
      reset: root.querySelector<HTMLButtonElement>('[data-role="reset"]')!,
      refetch: root.querySelector<HTMLButtonElement>('[data-role="refetch"]')!,
    };

    this.#wireEvents();
  }

  #wireEvents(): void {
    const n = this.#nodes;
    if (n === null) return;

    const emit = <T>(name: string, detail?: T): void => {
      this.dispatchEvent(new CustomEvent(name, { detail, bubbles: true, composed: true }));
    };

    n.segCanvas.addEventListener('click', () => {
      emit<ToolbarModeRequestDetail>('sd-mode-request', { mode: 'canvas2d' });
    });
    n.segGpu.addEventListener('click', () => {
      if (n.segGpu.getAttribute('aria-disabled') === 'true') return;
      emit<ToolbarModeRequestDetail>('sd-mode-request', { mode: 'webgpu' });
    });
    n.fit.addEventListener('click', () => emit('sd-fit'));
    n.reset.addEventListener('click', () => emit('sd-reset-zoom'));
    n.refetch.addEventListener('click', () => emit('sd-refetch'));
    n.externals.addEventListener('change', () => {
      emit<ToolbarFlagChangeDetail>('sd-externals-change', { value: n.externals.checked });
    });
  }

  #syncFromAttributes(): void {
    const n = this.#nodes;
    if (n === null) return;

    const mode = parseModeAttr(this.getAttribute('mode'));
    const isCanvas = mode === 'canvas2d';
    n.segCanvas.setAttribute('aria-pressed', String(isCanvas));
    n.segGpu.setAttribute('aria-pressed', String(!isCanvas));
    // classigo canonical falsy-value pattern: a `false && className`
    // short-circuit lands `false` in the args, which classigo skips.
    // No empty-string keys, no object dance.
    n.segCanvas.className = classigo(C.seg, isCanvas && C.segActive);
    n.segGpu.className = classigo(C.seg, !isCanvas && C.segActive);

    const webgpuAvailable = this.getAttribute('webgpu-available') !== 'false';
    if (!webgpuAvailable) {
      n.segGpu.setAttribute('aria-disabled', 'true');
      n.segGpu.title = 'WebGPU not available in this browser';
    } else {
      n.segGpu.removeAttribute('aria-disabled');
      n.segGpu.removeAttribute('title');
    }

    const statusKind = parseStatusKind(this.getAttribute('status-kind'));
    const statusText = this.getAttribute('status-text') ?? '';
    n.status.textContent = statusText.length > 0 ? statusText : statusFallbackText(statusKind);
    n.status.className = classigo(C.status, statusModifier(statusKind));

    const externalsOn = this.getAttribute('externals') === 'true';
    if (n.externals.checked !== externalsOn) n.externals.checked = externalsOn;
  }
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_TOOLBAR_TAG)) {
  customElements.define(STANDARDOC_TOOLBAR_TAG, ToolbarElement);
}

declare global {
  interface HTMLElementTagNameMap {
    [STANDARDOC_TOOLBAR_TAG]: ToolbarElement;
  }
}
