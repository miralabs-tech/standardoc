/**
 * Pure helpers for translating RAG settings into CLI spawn flags. Lives
 * in its own module (no `vscode` import) so bun unit tests can exercise
 * it without a VSCode runtime mock. The VSCode-coupled side
 * (read/write/watch) lives in `rag-settings.ts`.
 */

export type RagEmbedder = 'mock' | 'candle';

export interface RagSettings {
  readonly enabled: boolean;
  readonly embedder: RagEmbedder;
}

export const DEFAULT_RAG_SETTINGS: RagSettings = {
  enabled: false,
  embedder: 'mock',
};

/** Normalises an unknown embedder string into the closed enum. */
export function coerceEmbedder(raw: unknown): RagEmbedder {
  return raw === 'candle' ? 'candle' : 'mock';
}

/**
 * Translates a `RagSettings` snapshot into the CLI flags appended to
 * the `standardoc mcp` spawn args. Returns the empty list when RAG is
 * disabled — callers should NOT special-case `enabled=false`, they can
 * always splat the return value into the args.
 */
export function ragSpawnFlags(settings: RagSettings): readonly string[] {
  if (!settings.enabled) return [];
  return ['--rag', '--embedder', settings.embedder];
}

