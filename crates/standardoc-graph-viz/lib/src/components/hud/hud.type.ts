import type { RenderMode } from '../../types';

export interface HudHeap {
	readonly usedBytes: number;
	readonly limitBytes: number;
}

export interface HudGpu {
	readonly instanceCount: number;
	readonly instanceCapacity: number;
}

export interface HudRange {
	readonly min: number;
	readonly max: number;
}

export interface HudStats {
	readonly fps: number;
	readonly frameMs: number;
	readonly tickMs: number;
	/// Min/max observed in the same averaging window as `frameMs`.
	/// Optional so a host that doesn't track variance can omit it.
	readonly frameMsRange?: HudRange;
	/// Min/max observed in the same averaging window as `tickMs`.
	readonly tickMsRange?: HudRange;
	/// How many samples back the rolling averages were computed over.
	/// Useful in analyser tools to filter snapshots where the ring
	/// wasn't fully warmed up yet (e.g. drop entries with
	/// `ringFilled < ringSize / 2`).
	readonly ringFilled?: number;
	readonly nodes: number;
	readonly edges: number;
	readonly heap: HudHeap | null;
	readonly mode: RenderMode;
	readonly gpu: HudGpu | null;
}

/// Subset of `RecordingMode` re-exported here so consumers of the HUD
/// don't have to import from the profiler package just to type the
/// event detail. Kept structurally compatible.
export type HudRecordingMode = 'every' | 'warn' | 'danger';

export interface HudRecordStartDetail {
	readonly mode: HudRecordingMode;
}

export interface HudCopyDetail {
	readonly success: boolean;
	readonly bytes: number;
}
