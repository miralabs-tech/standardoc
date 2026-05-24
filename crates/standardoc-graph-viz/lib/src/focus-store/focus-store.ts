// `FocusStore` — minimal observable holding the shell's "currently
// inspected symbol" + a deduplicated recents list. Every panel of the
// multi-panel shell (Symbol Details, Focus Graph, Source View, Field
// Details, Callers Graph) is a lens on this single piece of state, so
// the store is intentionally trivial: one FQDN current value, one
// capped MRU list, one subscriber set.
//
// Persistence: snapshot is written to localStorage on every mutation
// under a configurable key (default `standardoc:focus-state`) so the
// shell remembers the last focus across reloads. Wrapped in try/catch
// because localStorage throws in restricted contexts (Safari private
// mode, sandboxed iframes) and the store must still work without it.
//
// Event bridge: each mutation also fires a `sd-focus-change` CustomEvent
// on `document` carrying the new state, so legacy Web Components that
// haven't been ported to the store-subscribe pattern can still react
// declaratively. New components should subscribe directly.

export interface FocusState {
	readonly current: string | null;
	readonly recent: ReadonlyArray<string>;
}

export interface FocusStoreOptions {
	readonly storageKey?: string;
	readonly maxRecent?: number;
	readonly storage?: Storage | null;
	readonly eventTarget?: EventTarget | null;
}

export type FocusChangeEvent = CustomEvent<FocusState>;

const DEFAULT_STORAGE_KEY = 'standardoc:focus-state';
const DEFAULT_MAX_RECENT = 20;
const FOCUS_CHANGE_EVENT = 'sd-focus-change';

export class FocusStore {
	private state: FocusState = { current: null, recent: [] };
	private readonly subscribers = new Set<(s: FocusState) => void>();
	private readonly storageKey: string;
	private readonly maxRecent: number;
	private readonly storage: Storage | null;
	private readonly eventTarget: EventTarget | null;

	constructor(options: FocusStoreOptions = {}) {
		this.storageKey = options.storageKey ?? DEFAULT_STORAGE_KEY;
		this.maxRecent = Math.max(1, options.maxRecent ?? DEFAULT_MAX_RECENT);
		this.storage = options.storage === undefined ? safeLocalStorage() : options.storage;
		this.eventTarget = options.eventTarget === undefined ? safeDocument() : options.eventTarget;
		this.state = this.load();
	}

	get(): FocusState {
		return this.state;
	}

	setFocus(fqdn: string | null): void {
		const trimmed = typeof fqdn === 'string' ? fqdn.trim() : null;
		const next = trimmed && trimmed.length > 0 ? trimmed : null;
		if (next === this.state.current) return;
		const recent = next === null
			? this.state.recent
			: [next, ...this.state.recent.filter(f => f !== next)].slice(0, this.maxRecent);
		this.state = { current: next, recent };
		this.persist();
		this.emit();
	}

	clearRecent(): void {
		if (this.state.recent.length === 0) return;
		this.state = { current: this.state.current, recent: [] };
		this.persist();
		this.emit();
	}

	subscribe(cb: (s: FocusState) => void): () => void {
		this.subscribers.add(cb);
		return () => { this.subscribers.delete(cb); };
	}

	private emit(): void {
		for (const cb of this.subscribers) cb(this.state);
		if (this.eventTarget) {
			const ev: FocusChangeEvent = new CustomEvent(FOCUS_CHANGE_EVENT, { detail: this.state });
			this.eventTarget.dispatchEvent(ev);
		}
	}

	private persist(): void {
		if (!this.storage) return;
		try {
			this.storage.setItem(this.storageKey, JSON.stringify(this.state));
		} catch {
			// Storage quota exceeded or sandboxed context — drop silently.
			// Subscribers still get the in-memory update.
		}
	}

	private load(): FocusState {
		if (!this.storage) return { current: null, recent: [] };
		try {
			const raw = this.storage.getItem(this.storageKey);
			if (raw === null) return { current: null, recent: [] };
			const parsed = JSON.parse(raw) as Partial<FocusState>;
			const current = typeof parsed.current === 'string' ? parsed.current : null;
			const recent = Array.isArray(parsed.recent)
				? parsed.recent.filter((f): f is string => typeof f === 'string').slice(0, this.maxRecent)
				: [];
			return { current, recent };
		} catch {
			return { current: null, recent: [] };
		}
	}
}

function safeLocalStorage(): Storage | null {
	try {
		return typeof window !== 'undefined' && window.localStorage ? window.localStorage : null;
	} catch {
		return null;
	}
}

function safeDocument(): EventTarget | null {
	return typeof document !== 'undefined' ? document : null;
}

/**
 * Shared singleton — the conventional store for the app shell. Tests
 * and embedded scenarios that need isolation can instantiate their own
 * `FocusStore` directly.
 */
export const focusStore = new FocusStore();

export { FOCUS_CHANGE_EVENT };
