/**
 * `<standardoc-hud>` — overlay panel showing live engine telemetry.
 *
 * Pure render-only component: the host (playground / VSCode webview)
 * owns the rAF loop, samples the engine, and feeds the result via the
 * `stats` property setter. The HUD never reaches back into the engine.
 * Visibility is controlled by the `visible` property (host-owned
 * keybinding, attribute, whatever the host wants).
 *
 * Sections are progressive: the GPU row only paints when `stats.gpu`
 * is non-null, so a Canvas2D session shows a tight 6-row panel and a
 * WebGPU session grows to 7 with the instance / capacity readout.
 */

import classigo from 'classigo';

import { DEFAULT_PERF_THRESHOLDS } from '../../profiler/profiler';
import type {
	HudCopyDetail,
	HudHeap,
	HudRange,
	HudRecordStartDetail,
	HudRecordingMode,
	HudStats,
} from './hud.type';
import s from './hud.module.scss';

export const STANDARDOC_HUD_TAG = 'standardoc-hud';

const C = {
	hud: s.hud ?? '',
	hudHidden: s['hud--hidden'] ?? '',
	header: s.hud__header ?? '',
	title: s.hud__title ?? '',
	hint: s.hud__hint ?? '',
	section: s.hud__section ?? '',
	term: s.hud__term ?? '',
	value: s.hud__value ?? '',
	muted: s.hud__muted ?? '',
	chip: s.hud__chip ?? '',
	chipGpu: s['hud__chip--gpu'] ?? '',
	range: s.hud__range ?? '',
	rangeSpike: s['hud__range--spike'] ?? '',
	pct: s.hud__pct ?? '',
	pctWarn: s['hud__pct--warn'] ?? '',
	emptyHint: s.hud__empty_hint ?? '',
	actions: s.hud__actions ?? '',
	action: s.hud__action ?? '',
	actionRecording: s['hud__action--recording'] ?? '',
	actionFlash: s['hud__action--flash'] ?? '',
	mode: s.hud__mode ?? '',
	recInfo: s.hud__rec_info ?? '',
	valueWarn: s['hud__value--warn'] ?? '',
} as const;

const GPU_WARN_PCT = 85;

function fmtMs(ms: number): string {
	if (!Number.isFinite(ms)) return '—';
	if (ms < 1) return ms.toFixed(2);
	if (ms < 10) return ms.toFixed(1);
	return ms.toFixed(0);
}

function fmtCount(n: number): string {
	if (!Number.isFinite(n)) return '—';
	return n.toLocaleString('en-US');
}

/// Smart byte formatter: picks MB or GB based on magnitude, strips the
/// trailing `.0` so a flat 4 GB reads `4 GB` instead of `4.0 GB`.
/// Threshold at 1024 MB (1 GiB) so the limit-of-the-Chromium-heap
/// constant `jsHeapSizeLimit ≈ 4 GiB` always lands in GB.
function fmtBytes(bytes: number): string {
	const mb = bytes / (1024 * 1024);
	if (mb >= 1024) {
		const gb = mb / 1024;
		return `${gb.toFixed(1).replace(/\.0$/, '')} GB`;
	}
	return `${mb.toFixed(mb < 10 ? 1 : 0).replace(/\.0$/, '')} MB`;
}

function fmtHeap(heap: HudHeap | null): string {
	if (heap === null || heap.limitBytes === 0) return 'n/a';
	return `${fmtBytes(heap.usedBytes)} / ${fmtBytes(heap.limitBytes)}`;
}

function fmtFps(fps: number): string {
	if (!Number.isFinite(fps) || fps <= 0) return '—';
	return Math.min(999, fps).toFixed(0);
}

/// Renders an inline `(min–max)` only when the range is meaningful:
/// the band must be wider than a couple of low-bit jitter ticks AND
/// must differ from the average enough to be worth surfacing.
function rangeText(range: HudRange | undefined, avg: number): string {
	if (range === undefined) return '';
	if (range.max <= range.min) return '';
	// Drop trivially-narrow ranges to keep the readout calm on a
	// steady-state frame loop (e.g. min 6.85 / max 6.95 / avg 6.9
	// adds nothing).
	if (range.max - range.min < Math.max(0.5, avg * 0.1)) return '';
	return `${fmtMs(range.min)}–${fmtMs(range.max)}`;
}

export class HudElement extends HTMLElement {
	#mounted = false;
	#visible = true;
	#stats: HudStats | null = null;
	#nodes: {
		root: HTMLElement;
		fps: HTMLElement;
		frame: HTMLElement;
		frameRange: HTMLElement;
		tick: HTMLElement;
		tickRange: HTMLElement;
		nodesEl: HTMLElement;
		edges: HTMLElement;
		edgesHint: HTMLElement;
		heap: HTMLElement;
		gpuRow: HTMLElement;
		gpuInstances: HTMLElement;
		gpuCapacity: HTMLElement;
		gpuPct: HTMLElement;
		modeChip: HTMLElement;
		modeSelect: HTMLSelectElement;
		recBtn: HTMLButtonElement;
		copyBtn: HTMLButtonElement;
		recInfo: HTMLElement;
	} | null = null;

	#recording: { startedMs: number } | null = null;

	connectedCallback(): void {
		if (this.#mounted) return;
		this.#mounted = true;
		this.#render();
		this.#sync();
	}

	set visible(value: boolean) {
		if (this.#visible === value) return;
		this.#visible = value;
		this.#sync();
	}

	get visible(): boolean {
		return this.#visible;
	}

	set stats(value: HudStats) {
		this.#stats = value;
		this.#sync();
	}

	get stats(): HudStats | null {
		return this.#stats;
	}

	#render(): void {
		const root = document.createElement('div');
		root.className = C.hud;

		root.innerHTML = `
			<header class="${C.header}">
				<span class="${C.title}">profiler <span class="${C.chip}" data-role="mode">2D</span></span>
				<span class="${C.hint}">press P to toggle</span>
			</header>
			<dl class="${C.section}">
				<dt class="${C.term}">fps</dt><dd class="${C.value}" data-role="fps">—</dd>
				<dt class="${C.term}">frame</dt><dd class="${C.value}"><span data-role="frame">—</span> ms<span class="${C.range}" data-role="frame-range"></span></dd>
				<dt class="${C.term}">tick</dt><dd class="${C.value}"><span data-role="tick">—</span> ms <span class="${C.muted}">(Rust)</span><span class="${C.range}" data-role="tick-range"></span></dd>
			</dl>
			<dl class="${C.section}">
				<dt class="${C.term}">nodes</dt><dd class="${C.value}" data-role="nodes">—</dd>
				<dt class="${C.term}">edges</dt><dd class="${C.value}"><span data-role="edges">—</span><span class="${C.emptyHint}" data-role="edges-hint" hidden> hover a chip</span></dd>
				<dt class="${C.term}">heap</dt><dd class="${C.value}" data-role="heap">—</dd>
			</dl>
			<dl class="${C.section}" data-role="gpu-row" hidden>
				<dt class="${C.term}">gpu</dt><dd class="${C.value}"><span data-role="gpu-instances">—</span> <span class="${C.muted}">/ <span data-role="gpu-capacity">—</span></span> <span class="${C.pct}" data-role="gpu-pct"></span></dd>
			</dl>
			<div class="${C.actions}">
				<select class="${C.mode}" data-role="rec-mode" title="Recording capture mode">
					<option value="every">every</option>
					<option value="warn">warn</option>
					<option value="danger">danger</option>
				</select>
				<button type="button" class="${C.action}" data-role="rec-toggle" title="Start recording (writes JSON on stop)">⏺</button>
				<button type="button" class="${C.action}" data-role="copy" title="Copy live snapshot as JSON">📋</button>
				<span class="${C.recInfo}" data-role="rec-info" hidden>—</span>
			</div>
		`;

		this.replaceChildren(root);

		this.#nodes = {
			root,
			fps: root.querySelector<HTMLElement>('[data-role="fps"]')!,
			frame: root.querySelector<HTMLElement>('[data-role="frame"]')!,
			frameRange: root.querySelector<HTMLElement>('[data-role="frame-range"]')!,
			tick: root.querySelector<HTMLElement>('[data-role="tick"]')!,
			tickRange: root.querySelector<HTMLElement>('[data-role="tick-range"]')!,
			nodesEl: root.querySelector<HTMLElement>('[data-role="nodes"]')!,
			edges: root.querySelector<HTMLElement>('[data-role="edges"]')!,
			edgesHint: root.querySelector<HTMLElement>('[data-role="edges-hint"]')!,
			heap: root.querySelector<HTMLElement>('[data-role="heap"]')!,
			gpuRow: root.querySelector<HTMLElement>('[data-role="gpu-row"]')!,
			gpuInstances: root.querySelector<HTMLElement>('[data-role="gpu-instances"]')!,
			gpuCapacity: root.querySelector<HTMLElement>('[data-role="gpu-capacity"]')!,
			gpuPct: root.querySelector<HTMLElement>('[data-role="gpu-pct"]')!,
			modeChip: root.querySelector<HTMLElement>('[data-role="mode"]')!,
			modeSelect: root.querySelector<HTMLSelectElement>('[data-role="rec-mode"]')!,
			recBtn: root.querySelector<HTMLButtonElement>('[data-role="rec-toggle"]')!,
			copyBtn: root.querySelector<HTMLButtonElement>('[data-role="copy"]')!,
			recInfo: root.querySelector<HTMLElement>('[data-role="rec-info"]')!,
		};

		this.#wireActions();
	}

	#wireActions(): void {
		const n = this.#nodes;
		if (n === null) return;

		n.recBtn.addEventListener('click', () => {
			if (this.#recording === null) {
				const mode = readRecMode(n.modeSelect.value);
				this.#recording = { startedMs: performance.now() };
				n.recBtn.textContent = '⏹';
				n.recBtn.title = 'Stop recording';
				n.recBtn.className = classigo(C.action, C.actionRecording);
				n.modeSelect.disabled = true;
				n.recInfo.hidden = false;
				n.recInfo.textContent = '0.0s';
				this.#emit<HudRecordStartDetail>('sd-hud-rec-start', { mode });
			} else {
				this.#recording = null;
				n.recBtn.textContent = '⏺';
				n.recBtn.title = 'Start recording (writes JSON on stop)';
				n.recBtn.className = C.action;
				n.modeSelect.disabled = false;
				n.recInfo.hidden = true;
				this.#emit('sd-hud-rec-stop');
			}
		});

		n.copyBtn.addEventListener('click', () => {
			void this.#copyLive();
		});
	}

	async #copyLive(): Promise<void> {
		const n = this.#nodes;
		if (n === null) return;
		const payload = this.#stats === null ? null : JSON.stringify(this.#stats, null, 2);
		if (payload === null) {
			this.#flashCopy(n.copyBtn, '∅', false);
			this.#emit<HudCopyDetail>('sd-hud-copy', { success: false, bytes: 0 });
			return;
		}
		let success = false;
		try {
			await navigator.clipboard.writeText(payload);
			success = true;
		} catch {
			success = false;
		}
		this.#flashCopy(n.copyBtn, success ? '✓' : '✗', success);
		this.#emit<HudCopyDetail>('sd-hud-copy', { success, bytes: payload.length });
	}

	#flashCopy(btn: HTMLButtonElement, glyph: string, success: boolean): void {
		const prev = btn.textContent ?? '📋';
		btn.textContent = glyph;
		btn.className = classigo(C.action, C.actionFlash);
		setTimeout(() => {
			btn.textContent = prev;
			btn.className = C.action;
		}, success ? 700 : 1200);
	}

	#emit<T = undefined>(name: string, detail?: T): void {
		this.dispatchEvent(new CustomEvent(name, { detail, bubbles: true, composed: true }));
	}

	#sync(): void {
		const n = this.#nodes;
		if (n === null) return;

		// classigo canonical falsy-value pattern for the hidden modifier.
		n.root.className = classigo(C.hud, !this.#visible && C.hudHidden);

		const stats = this.#stats;
		if (stats === null) return;

		n.fps.textContent = fmtFps(stats.fps);
		n.frame.textContent = fmtMs(stats.frameMs);
		n.tick.textContent = fmtMs(stats.tickMs);
		n.nodesEl.textContent = fmtCount(stats.nodes);
		n.heap.textContent = fmtHeap(stats.heap);

		// Warn coloring is now anchored to the rolling-avg breaching the
		// same threshold the Profiler uses for `warn`-severity recording.
		// Single-frame jitter (max > 2× avg but avg still healthy) no
		// longer fires red — that was visual noise that didn't match the
		// recording trigger. The min-max range still renders as a muted
		// readout so the variance stays visible without alarming.
		const frameWarn = stats.frameMs >= DEFAULT_PERF_THRESHOLDS.frameWarnMs;
		const tickWarn = stats.tickMs >= DEFAULT_PERF_THRESHOLDS.tickWarnMs;
		n.frame.className = classigo(frameWarn && C.valueWarn);
		n.tick.className = classigo(tickWarn && C.valueWarn);

		const frameRangeText = rangeText(stats.frameMsRange, stats.frameMs);
		const tickRangeText = rangeText(stats.tickMsRange, stats.tickMs);
		n.frameRange.textContent = frameRangeText.length > 0 ? ` (${frameRangeText})` : '';
		n.tickRange.textContent = tickRangeText.length > 0 ? ` (${tickRangeText})` : '';
		n.frameRange.className = classigo(C.range, frameWarn && C.rangeSpike);
		n.tickRange.className = classigo(C.range, tickWarn && C.rangeSpike);

		// Edges: a steady "0" is uninformative on its own — the engine
		// only renders edges for the currently-hovered chip, so we surface
		// the hint inline when the row would otherwise read empty.
		n.edges.textContent = fmtCount(stats.edges);
		n.edgesHint.hidden = stats.edges !== 0;

		const isGpu = stats.mode === 'webgpu';
		n.modeChip.textContent = isGpu ? '3D' : '2D';
		n.modeChip.className = classigo(C.chip, isGpu && C.chipGpu);

		if (stats.gpu !== null) {
			n.gpuRow.hidden = false;
			n.gpuInstances.textContent = fmtCount(stats.gpu.instanceCount);
			n.gpuCapacity.textContent = fmtCount(stats.gpu.instanceCapacity);
			const pct = stats.gpu.instanceCapacity > 0
				? Math.round((100 * stats.gpu.instanceCount) / stats.gpu.instanceCapacity)
				: 0;
			n.gpuPct.textContent = `(${pct}%)`;
			n.gpuPct.className = classigo(C.pct, pct >= GPU_WARN_PCT && C.pctWarn);
		} else {
			n.gpuRow.hidden = true;
		}

		if (this.#recording !== null) {
			const elapsed = (performance.now() - this.#recording.startedMs) / 1000;
			n.recInfo.textContent = `${elapsed.toFixed(1)}s`;
		}
	}
}

function readRecMode(value: string): HudRecordingMode {
	return value === 'warn' || value === 'danger' ? value : 'every';
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_HUD_TAG)) {
	customElements.define(STANDARDOC_HUD_TAG, HudElement);
}

declare global {
	interface HTMLElementTagNameMap {
		[STANDARDOC_HUD_TAG]: HudElement;
	}
}
