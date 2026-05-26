/// Categories the legend surfaces. Kept open as a string union so a host
/// can wire Phase B filter dispatch without leaking enum imports across
/// the lib / playground / webview boundary.
export type LegendCategory = 'kind' | 'edge' | 'language';

/// Event detail emitted on legend entry click. Phase A leaves this
/// undefined-dispatched (the click handler logs but doesn't fire); Phase
/// B will wire it through to Explorer / Focus / Overview filter state.
export interface LegendFilterDetail {
	readonly category: LegendCategory;
	readonly value: string;
}
