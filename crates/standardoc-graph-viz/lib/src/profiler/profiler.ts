/**
 * `Profiler` — frame-rate / engine-stat sampler that drives a
 * `<standardoc-hud>` element. The lib doesn't know about wasm-bindgen
 * directly: the host hands a `sample` callback returning a
 * `ProfilerEngineSnapshot`, the profiler does the ring-buffer
 * averaging, JS-heap reading, paint throttling, and pushes the
 * resulting `HudStats` to the HUD on its own schedule.
 *
 * Decoupled by design so the same controller can drive the playground,
 * the VSCode webview, or a future Storybook fixture without dragging
 * the engine type into the lib.
 *
 * Lifecycle:
 *   1. `new Profiler({ hud, sample, isPaused })` — cheap, no rAF
 *      hijacking. The host owns the loop.
 *   2. `profiler.tick(now)` — call from `requestAnimationFrame`. Skips
 *      sampling when `isPaused()` returns true (e.g., while an async
 *      `&mut self` is held on the engine). Throttles HUD repaint to
 *      `paintIntervalMs` (default 250 ms) so the readout stays stable.
 *   3. `profiler.reset()` — clear rings + paint timestamps. Use this
 *      after a long pause so the rolling averages don't carry stale
 *      data into the next active window.
 */

import type { HudElement } from '../components/hud/hud.element';
import type { HudHeap, HudStats } from '../components/hud/hud.type';
import type {
	PerfThresholds,
	ProfilerHeapFn,
	ProfilerOptions,
	ProfilerPausedFn,
	ProfilerSampleFn,
	RecordedSeverity,
	RecordedSnapshot,
	RecordingMode,
	RecordingOptions,
	RecordingResult,
} from './profiler.type';

/// Canonical perf thresholds shared between the Profiler's recording
/// classifier AND the HUD's live-warn indicator. Exported so the HUD
/// renders red exactly when the recording would emit a `warn`/`danger`
/// entry — no more sub-second visual noise on natural frame jitter.
export const DEFAULT_PERF_THRESHOLDS: Required<PerfThresholds> = {
	// 60fps refresh budget = 16.7 ms ; 30fps floor = 33.3 ms.
	frameWarnMs: 16.7,
	frameDangerMs: 33.3,
	// Engine tick alone consuming >2 ms on a 2 k node scene is a
	// regression worth flagging; >5 ms is a code red.
	tickWarnMs: 2,
	tickDangerMs: 5,
	// Sustained drop below 60Hz feels janky; <30Hz is broken UX.
	fpsWarnUnder: 50,
	fpsDangerUnder: 30,
	// Instance buffer realloc happens at the next power-of-two; >85%
	// utilisation means a realloc is about to fire mid-scene.
	gpuWarnPct: 85,
	gpuDangerPct: 95,
};

interface RecordingState {
	mode: RecordingMode;
	thresholds: Required<PerfThresholds>;
	everyIntervalMs: number;
	perReasonRateLimitMs: number;
	startedAt: Date;
	startedMs: number;
	lastEveryWriteMs: number;
	lastWriteByReason: Map<string, { atMs: number; index: number }>;
	snapshots: RecordedSnapshot[];
	skippedByThrottle: number;
	coalescedByDedup: number;
}

const DEFAULT_RING_SIZE = 60;
const DEFAULT_PAINT_INTERVAL_MS = 250;

function defaultReadHeap(): HudHeap | null {
	interface HeapMemoryInfo {
		readonly usedJSHeapSize: number;
		readonly totalJSHeapSize: number;
		readonly jsHeapSizeLimit: number;
	}
	type PerfWithMemory = Performance & { memory?: HeapMemoryInfo };
	const mem = (performance as PerfWithMemory).memory;
	if (mem === undefined || mem.jsHeapSizeLimit === 0) return null;
	return { usedBytes: mem.usedJSHeapSize, limitBytes: mem.jsHeapSizeLimit };
}

export class Profiler {
	readonly #hud: HudElement;
	readonly #sample: ProfilerSampleFn;
	readonly #readHeap: ProfilerHeapFn;
	readonly #isPaused: ProfilerPausedFn;
	readonly #ringSize: number;
	readonly #paintIntervalMs: number;
	readonly #frameRing: Float32Array;
	readonly #tickRing: Float32Array;
	#ringIndex = 0;
	#ringFilled = 0;
	// -1 sentinel = "no previous sample yet"; avoids logging a huge
	// frame-delta the first time `tick()` runs after construction or
	// after `reset()`.
	#lastSampleMs = -1;
	#lastPaintMs = 0;
	// Most recently built `HudStats` — cached so `snapshot()` can hand
	// it back outside the paint cycle (used by the HUD copy button).
	#lastStats: HudStats | null = null;
	#recording: RecordingState | null = null;

	constructor(opts: ProfilerOptions) {
		this.#hud = opts.hud;
		this.#sample = opts.sample;
		this.#readHeap = opts.readHeap ?? defaultReadHeap;
		this.#isPaused = opts.isPaused ?? alwaysFalse;
		this.#ringSize = opts.ringSize ?? DEFAULT_RING_SIZE;
		this.#paintIntervalMs = opts.paintIntervalMs ?? DEFAULT_PAINT_INTERVAL_MS;
		this.#frameRing = new Float32Array(this.#ringSize);
		this.#tickRing = new Float32Array(this.#ringSize);
	}

	tick(now: number): void {
		if (this.#isPaused()) return;

		const frameMs = this.#lastSampleMs < 0 ? 0 : now - this.#lastSampleMs;
		this.#lastSampleMs = now;

		const snap = this.#sample();
		const tickMs = snap.lastTickUs / 1000;

		if (frameMs > 0) {
			this.#frameRing[this.#ringIndex] = frameMs;
			this.#tickRing[this.#ringIndex] = tickMs;
			this.#ringIndex = (this.#ringIndex + 1) % this.#ringSize;
			if (this.#ringFilled < this.#ringSize) this.#ringFilled++;
		}

		if (now - this.#lastPaintMs < this.#paintIntervalMs) return;
		this.#lastPaintMs = now;

		const frameAvg = avg(this.#frameRing, this.#ringFilled);
		const tickAvg = avg(this.#tickRing, this.#ringFilled);
		const fps = frameAvg > 0 ? Math.min(999, 1000 / frameAvg) : 0;
		// Min / max within the same window as the average so the HUD can
		// surface spikes the moving mean smooths out. Both are clamped
		// against the filled-count to avoid leaking the zero-initialised
		// tail of an unfilled ring into the readout.
		const frameRange = rangeOf(this.#frameRing, this.#ringFilled);
		const tickRange = rangeOf(this.#tickRing, this.#ringFilled);

		const stats: HudStats = {
			fps,
			frameMs: frameAvg,
			tickMs: tickAvg,
			frameMsRange: frameRange,
			tickMsRange: tickRange,
			ringFilled: this.#ringFilled,
			nodes: snap.symbolCount,
			edges: snap.edgeCount,
			heap: this.#readHeap(),
			mode: snap.mode,
			gpu: snap.gpu,
		};
		this.#lastStats = stats;
		this.#hud.stats = stats;
		if (this.#recording !== null) this.#recordIfNeeded(now, stats);
	}

	/// Most recent stats snapshot, or `null` if no paint has happened yet.
	/// Useful for one-shot exports outside the paint loop (e.g. the HUD
	/// "copy live JSON" button).
	snapshot(): HudStats | null {
		return this.#lastStats;
	}

	isRecording(): boolean {
		return this.#recording !== null;
	}

	startRecording(opts: RecordingOptions): void {
		// Defensive: silently restart if already recording. The host's
		// UI is supposed to gate this, but a re-entrant call shouldn't
		// silently double-write into the same buffer.
		if (this.#recording !== null) this.stopRecording();
		// Wipe the rolling rings so the first recorded snapshots aren't
		// polluted by boot-time transient frames (the very first rAF
		// delta is often sub-millisecond AND a subsequent JIT warm-up
		// frame routinely lands at 15-20 ms; both poison the early
		// `frameMsRange` until the ring rolls over).
		this.reset();
		const now = performance.now();
		const thresholds: Required<PerfThresholds> = {
			...DEFAULT_PERF_THRESHOLDS,
			...(opts.thresholds ?? {}),
		};
		this.#recording = {
			mode: opts.mode,
			thresholds,
			everyIntervalMs: opts.everyIntervalMs ?? 1000,
			// Default 500 ms — strictly larger than the paint interval
			// (250 ms) so two consecutive paints in the same burst CAN
			// coalesce. The previous 100 ms default was smaller than the
			// paint cadence, making dedup dead code.
			perReasonRateLimitMs: opts.perReasonRateLimitMs ?? 500,
			startedAt: new Date(),
			startedMs: now,
			lastEveryWriteMs: Number.NEGATIVE_INFINITY,
			lastWriteByReason: new Map(),
			snapshots: [],
			skippedByThrottle: 0,
			coalescedByDedup: 0,
		};
	}

	stopRecording(): RecordingResult {
		const rec = this.#recording;
		if (rec === null) {
			throw new Error('Profiler.stopRecording() called while not recording');
		}
		this.#recording = null;
		return {
			mode: rec.mode,
			thresholds: rec.thresholds,
			startedAtIso: rec.startedAt.toISOString(),
			durationMs: performance.now() - rec.startedMs,
			totalEvents: rec.snapshots.length,
			skippedByThrottle: rec.skippedByThrottle,
			coalescedByDedup: rec.coalescedByDedup,
			snapshots: rec.snapshots,
		};
	}

	#recordIfNeeded(now: number, stats: HudStats): void {
		const rec = this.#recording!;
		const { severity, reasons } = classifySeverity(stats, rec.thresholds);

		// Filter by mode. `every` keeps all; `warn` drops bare `every`s;
		// `danger` only keeps `danger`s.
		if (rec.mode === 'warn' && severity === 'every') return;
		if (rec.mode === 'danger' && severity !== 'danger') return;

		// For periodic `every`-severity writes, apply the interval
		// throttle. Warn/danger always bypass — losing a spike to a
		// throttle defeats the point of the recording.
		if (severity === 'every') {
			if (now - rec.lastEveryWriteMs < rec.everyIntervalMs) {
				rec.skippedByThrottle++;
				return;
			}
			rec.lastEveryWriteMs = now;
		} else {
			// Warn/danger anti-flood: same reason set within the rate
			// window coalesces into the prior entry's `count`. The
			// window slides — every coalesce updates `atMs` so a
			// sustained burst stays attached to the SAME head entry
			// instead of cutting a new head every `perReasonRateLimitMs`.
			const key = [...reasons].sort().join('|');
			const prev = rec.lastWriteByReason.get(key);
			if (prev !== undefined && now - prev.atMs < rec.perReasonRateLimitMs) {
				const old = rec.snapshots[prev.index];
				if (old !== undefined) {
					rec.snapshots[prev.index] = { ...old, count: old.count + 1 };
				}
				prev.atMs = now;
				rec.coalescedByDedup++;
				return;
			}
			rec.lastWriteByReason.set(key, { atMs: now, index: rec.snapshots.length });
		}

		rec.snapshots.push({
			tMs: now - rec.startedMs,
			severity,
			reasons,
			count: 1,
			stats,
		});
	}

	reset(): void {
		this.#frameRing.fill(0);
		this.#tickRing.fill(0);
		this.#ringIndex = 0;
		this.#ringFilled = 0;
		this.#lastSampleMs = -1;
		this.#lastPaintMs = 0;
	}
}

function alwaysFalse(): boolean {
	return false;
}

function classifySeverity(
	stats: HudStats,
	t: Required<PerfThresholds>,
): { severity: RecordedSeverity; reasons: string[] } {
	const reasons: string[] = [];
	let sev: RecordedSeverity = 'every';
	const bump = (next: RecordedSeverity): void => {
		if (next === 'danger') sev = 'danger';
		else if (next === 'warn' && sev !== 'danger') sev = 'warn';
	};

	if (stats.frameMs >= t.frameDangerMs) {
		reasons.push(`frame≥${t.frameDangerMs}ms`);
		bump('danger');
	} else if (stats.frameMs >= t.frameWarnMs) {
		reasons.push(`frame≥${t.frameWarnMs}ms`);
		bump('warn');
	}

	if (stats.tickMs >= t.tickDangerMs) {
		reasons.push(`tick≥${t.tickDangerMs}ms`);
		bump('danger');
	} else if (stats.tickMs >= t.tickWarnMs) {
		reasons.push(`tick≥${t.tickWarnMs}ms`);
		bump('warn');
	}

	if (stats.fps > 0 && stats.fps <= t.fpsDangerUnder) {
		reasons.push(`fps≤${t.fpsDangerUnder}`);
		bump('danger');
	} else if (stats.fps > 0 && stats.fps <= t.fpsWarnUnder) {
		reasons.push(`fps≤${t.fpsWarnUnder}`);
		bump('warn');
	}

	if (stats.gpu !== null && stats.gpu.instanceCapacity > 0) {
		const pct = (100 * stats.gpu.instanceCount) / stats.gpu.instanceCapacity;
		if (pct >= t.gpuDangerPct) {
			reasons.push(`gpu≥${t.gpuDangerPct}%`);
			bump('danger');
		} else if (pct >= t.gpuWarnPct) {
			reasons.push(`gpu≥${t.gpuWarnPct}%`);
			bump('warn');
		}
	}

	return { severity: sev, reasons };
}

function avg(buf: Float32Array, count: number): number {
	if (count === 0) return 0;
	let sum = 0;
	for (let i = 0; i < count; i++) sum += buf[i] ?? 0;
	return sum / count;
}

function rangeOf(buf: Float32Array, count: number): { min: number; max: number } {
	if (count === 0) return { min: 0, max: 0 };
	let mn = Number.POSITIVE_INFINITY;
	let mx = Number.NEGATIVE_INFINITY;
	for (let i = 0; i < count; i++) {
		const v = buf[i] ?? 0;
		if (v < mn) mn = v;
		if (v > mx) mx = v;
	}
	return {
		min: Number.isFinite(mn) ? mn : 0,
		max: Number.isFinite(mx) ? mx : 0,
	};
}
