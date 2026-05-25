// `PanelManager` — observable singleton driving the spawnable-panel
// surface (bottom drawer). State is intentionally minimal: an ordered
// list of `PanelInstance` + the active panel id. The matching custom
// element (`<standardoc-panel-host>`) subscribes here and renders the
// drawer; the shell calls `open()` from symbol-details actions and
// pushes per-panel data via the element returned by the host.
//
// Lifecycle is ephemeral by design — panels are NOT persisted to
// localStorage (unlike `FocusStore`). A page reload starts with an
// empty manager; this matches the "spawnable" mental model and avoids
// resurrecting stale panels on top of a different focus.
//
// Event bridge: every mutation fires `sd-panel-change` on the optional
// `eventTarget` (default `document`) so listeners that haven't been
// ported to the subscribe pattern can still react.

import type {
	PanelChangeDetail,
	PanelInstance,
	PanelKind,
	PanelManagerState,
	PanelPropsMap,
} from './types';

const PANEL_CHANGE_EVENT = 'sd-panel-change';

export interface PanelManagerOptions {
	readonly eventTarget?: EventTarget | null;
}

let idCounter = 1;

export class PanelManager {
	private state: PanelManagerState = { panels: [], activeId: null };
	private readonly subscribers = new Set<(s: PanelManagerState) => void>();
	private readonly eventTarget: EventTarget | null;

	constructor(options: PanelManagerOptions = {}) {
		this.eventTarget = options.eventTarget === undefined ? safeDocument() : options.eventTarget;
	}

	get(): PanelManagerState {
		return this.state;
	}

	open<K extends PanelKind>(kind: K, props: PanelPropsMap[K], title?: string): string {
		const id = `panel-${idCounter++}`;
		const instance = { id, kind, props, title: title ?? defaultTitle(kind, props) } as PanelInstance<K>;
		this.state = {
			panels: [...this.state.panels, instance],
			activeId: id,
		};
		this.emit(id);
		return id;
	}

	close(id: string): void {
		const idx = this.state.panels.findIndex(p => p.id === id);
		if (idx < 0) return;
		const panels = this.state.panels.filter(p => p.id !== id);
		let activeId = this.state.activeId;
		if (activeId === id) {
			// Prefer the panel that took this one's slot (next neighbour),
			// otherwise the previous one — matches the dismissal feel of
			// IDE tabs (you usually want to land on the adjacent context,
			// not have the surface go blank when there are still tabs).
			activeId = panels[idx]?.id ?? panels[idx - 1]?.id ?? null;
		}
		this.state = { panels, activeId };
		this.emit(id);
	}

	focus(id: string): void {
		if (this.state.activeId === id) return;
		if (!this.state.panels.some(p => p.id === id)) return;
		this.state = { panels: this.state.panels, activeId: id };
		this.emit(id);
	}

	closeAll(): void {
		if (this.state.panels.length === 0) return;
		this.state = { panels: [], activeId: null };
		this.emit(null);
	}

	subscribe(cb: (s: PanelManagerState) => void): () => void {
		this.subscribers.add(cb);
		return () => { this.subscribers.delete(cb); };
	}

	private emit(changedId: string | null): void {
		for (const cb of this.subscribers) cb(this.state);
		if (this.eventTarget) {
			const detail: PanelChangeDetail = { ...this.state, changedId };
			this.eventTarget.dispatchEvent(new CustomEvent(PANEL_CHANGE_EVENT, { detail }));
		}
	}
}

function defaultTitle<K extends PanelKind>(kind: K, props: PanelPropsMap[K]): string {
	if (kind === 'compare') {
		const p = props as PanelPropsMap['compare'];
		return `Compare · ${shortFqdn(p.leftFqdn)} ↔ ${shortFqdn(p.rightFqdn)}`;
	}
	return kind;
}

function shortFqdn(fqdn: string): string {
	const idx = fqdn.lastIndexOf('::');
	return idx >= 0 ? fqdn.slice(idx + 2) : fqdn;
}

function safeDocument(): EventTarget | null {
	return typeof document !== 'undefined' ? document : null;
}

export const panelManager = new PanelManager();
export { PANEL_CHANGE_EVENT };
