import type { HudElement } from '../components/hud/hud.element';
import type { HudGpu, HudHeap, HudStats } from '../components/hud/hud.type';
import type { RenderMode } from '../types';

/// Recording write modes.
///   - `every`   keep every snapshot but throttled to `everyIntervalMs`.
///               warn/danger events bypass the throttle so spikes are
///               never lost.
///   - `warn`    keep snapshots that breach the warn threshold or worse.
///   - `danger`  keep snapshots that breach the danger threshold only.
export type RecordingMode = 'every' | 'warn' | 'danger';

/// Severity attached to a recorded snapshot. `every` means the
/// snapshot was captured by the periodic write, not by a threshold
/// breach.
export type RecordedSeverity = 'every' | 'warn' | 'danger';

export interface PerfThresholds {
	readonly frameWarnMs?: number;
	readonly frameDangerMs?: number;
	readonly tickWarnMs?: number;
	readonly tickDangerMs?: number;
	readonly fpsWarnUnder?: number;
	readonly fpsDangerUnder?: number;
	readonly gpuWarnPct?: number;
	readonly gpuDangerPct?: number;
}

export interface RecordingOptions {
	readonly mode: RecordingMode;
	/// For `every` mode: minimum spacing between periodic writes.
	/// Default 1000 ms. Warn/danger writes are NOT subject to this.
	readonly everyIntervalMs?: number;
	/// Threshold overrides. Any field left undefined falls back to
	/// `DEFAULT_PERF_THRESHOLDS`.
	readonly thresholds?: PerfThresholds;
	/// Per-reason rate limit on warn/danger writes. A subsequent event
	/// with the same reason set within this window doesn't append a
	/// new entry; it increments the `count` on the previous one and
	/// bumps the `coalescedByDedup` counter. Default 500 ms — strictly
	/// larger than the paint cadence (250 ms) so two consecutive
	/// paints in the same burst CAN coalesce.
	readonly perReasonRateLimitMs?: number;
}

export interface RecordedSnapshot {
	/// Milliseconds since `startRecording()` was called.
	readonly tMs: number;
	readonly severity: RecordedSeverity;
	/// Compact human-readable reason keys (`frame≥16.7ms`, `gpu≥85%`).
	/// Empty for a plain `every`-severity snapshot.
	readonly reasons: ReadonlyArray<string>;
	/// 1 on the first observation; >1 when subsequent same-reason
	/// events were coalesced into this row under the rate limit.
	readonly count: number;
	readonly stats: HudStats;
}

export interface RecordingResult {
	readonly mode: RecordingMode;
	readonly thresholds: Required<PerfThresholds>;
	readonly startedAtIso: string;
	readonly durationMs: number;
	readonly totalEvents: number;
	/// How many `every`-mode periodic writes were swallowed by the
	/// throttle. Surfacing this prevents misreading the snapshot
	/// density as a timeline.
	readonly skippedByThrottle: number;
	/// How many warn/danger writes were coalesced into a previous
	/// entry's `count` instead of producing a new row.
	readonly coalescedByDedup: number;
	readonly snapshots: ReadonlyArray<RecordedSnapshot>;
}

export interface ProfilerEngineSnapshot {
	readonly symbolCount: number;
	readonly edgeCount: number;
	/// Microseconds spent in the last engine tick. The HUD divides by
	/// 1000 to display milliseconds. Zero before the first non-trivial
	/// frame.
	readonly lastTickUs: number;
	readonly mode: RenderMode;
	readonly gpu: HudGpu | null;
}

export type ProfilerSampleFn = () => ProfilerEngineSnapshot;
export type ProfilerHeapFn = () => HudHeap | null;
export type ProfilerPausedFn = () => boolean;

export interface ProfilerOptions {
	readonly hud: HudElement;
	readonly sample: ProfilerSampleFn;
	/// Optional heap-stat reader. Defaults to the Chromium-only
	/// `performance.memory` API; returns `null` on browsers that
	/// don't expose it, which the HUD renders as "n/a".
	readonly readHeap?: ProfilerHeapFn;
	/// Optional pause predicate. While it returns `true`, `tick()` is
	/// a no-op — no sample, no paint, no ring update. Use it to gate
	/// the profiler from re-entering the engine during async `&mut`
	/// borrows held by methods like `enable_webgpu`.
	readonly isPaused?: ProfilerPausedFn;
	/// Ring buffer length for fps / frame-ms / tick-ms averaging.
	/// Default 60 samples ≈ 1 s at 60 fps.
	readonly ringSize?: number;
	/// Minimum interval between HUD repaints. The profiler still
	/// samples on every `tick()` to keep the rolling averages tight,
	/// but it only pushes a new snapshot to the HUD every
	/// `paintIntervalMs`. Default 250 ms.
	readonly paintIntervalMs?: number;
}
