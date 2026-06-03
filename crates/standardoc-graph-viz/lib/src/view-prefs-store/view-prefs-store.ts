// `ViewPrefsStore` — shell-wide preferences observed by every panel.
// Mirrors `FocusStore`'s shape (subscribe / get / persist / emit
// CustomEvent on document) so the wiring pattern is identical and
// new prefs land additively. Today carries a single boolean
// (`excludeTests`); the contract scales by extending `ViewPrefsState`
// and `setPrefs(partial)` without breaking subscribers.

export interface ViewPrefsState {
  /** When true, panels filter out test-shaped symbols + neighbors
   *  (Rust `::tests::` modules, `*_test.rs`, TS `*.test.ts`,
   *  `__tests__/` dirs). Sourced from the same heuristic the Rust
   *  MCP `exclude_tests` flag uses server-side. */
  readonly excludeTests: boolean;
}

export interface ViewPrefsStoreOptions {
  readonly storageKey?: string;
  readonly storage?: Storage | null;
  readonly eventTarget?: EventTarget | null;
}

export type ViewPrefsChangeEvent = CustomEvent<ViewPrefsState>;

const DEFAULT_STORAGE_KEY = 'standardoc:view-prefs';
const VIEW_PREFS_CHANGE_EVENT = 'sd-view-prefs-change';

const DEFAULT_STATE: ViewPrefsState = { excludeTests: false };

export class ViewPrefsStore {
  private state: ViewPrefsState = DEFAULT_STATE;
  private readonly subscribers = new Set<(s: ViewPrefsState) => void>();
  private readonly storageKey: string;
  private readonly storage: Storage | null;
  private readonly eventTarget: EventTarget | null;

  constructor(options: ViewPrefsStoreOptions = {}) {
    this.storageKey = options.storageKey ?? DEFAULT_STORAGE_KEY;
    this.storage = options.storage === undefined ? safeLocalStorage() : options.storage;
    this.eventTarget = options.eventTarget === undefined ? safeDocument() : options.eventTarget;
    this.state = this.load();
  }

  get(): ViewPrefsState {
    return this.state;
  }

  /** Merge-update: pass only the fields you want to flip. Subscribers
   *  fire iff the merged state actually differs from the previous. */
  setPrefs(partial: Partial<ViewPrefsState>): void {
    const next: ViewPrefsState = { ...this.state, ...partial };
    // Structural diff over every key so the store scales additively —
    // a new pref field is picked up here without editing this guard
    // (a single-field `===` check would silently no-op the new field).
    const changed = (Object.keys(next) as (keyof ViewPrefsState)[])
      .some(k => next[k] !== this.state[k]);
    if (!changed) return;
    this.state = next;
    this.persist();
    this.emit();
  }

  subscribe(cb: (s: ViewPrefsState) => void): () => void {
    this.subscribers.add(cb);
    return () => { this.subscribers.delete(cb); };
  }

  private emit(): void {
    for (const cb of this.subscribers) cb(this.state);
    if (this.eventTarget) {
      const ev: ViewPrefsChangeEvent = new CustomEvent(VIEW_PREFS_CHANGE_EVENT, { detail: this.state });
      this.eventTarget.dispatchEvent(ev);
    }
  }

  private persist(): void {
    if (!this.storage) return;
    try {
      this.storage.setItem(this.storageKey, JSON.stringify(this.state));
    } catch {
      // Quota exceeded / sandboxed context — drop silently.
    }
  }

  private load(): ViewPrefsState {
    if (!this.storage) return DEFAULT_STATE;
    try {
      const raw = this.storage.getItem(this.storageKey);
      if (raw === null) return DEFAULT_STATE;
      const parsed = JSON.parse(raw) as Partial<ViewPrefsState>;
      return {
        excludeTests: typeof parsed.excludeTests === 'boolean' ? parsed.excludeTests : false,
      };
    } catch {
      return DEFAULT_STATE;
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

/** Shared singleton — the conventional store for the app shell. */
export const viewPrefsStore = new ViewPrefsStore();

export { VIEW_PREFS_CHANGE_EVENT };
