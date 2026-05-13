import * as vscode from 'vscode';
import {
  DEFAULT_RAG_SETTINGS,
  coerceEmbedder,
  type RagEmbedder,
  type RagSettings,
} from './rag-flags';

export {
  DEFAULT_RAG_SETTINGS,
  ragSpawnFlags,
  type RagEmbedder,
  type RagSettings,
} from './rag-flags';

const SETTING_ENABLED = 'standardoc.ragEnabled';
const SETTING_EMBEDDER = 'standardoc.ragEmbedder';

/** Reads the current RAG settings snapshot from VSCode configuration. */
export function readRagSettings(): RagSettings {
  const cfg = vscode.workspace.getConfiguration();
  const enabled = cfg.get<boolean>(SETTING_ENABLED, DEFAULT_RAG_SETTINGS.enabled);
  const raw = cfg.get<string>(SETTING_EMBEDDER, DEFAULT_RAG_SETTINGS.embedder);
  return { enabled, embedder: coerceEmbedder(raw) };
}

/** Persists the new `enabled` value at the workspace level (resource scope). */
export async function writeRagEnabled(enabled: boolean): Promise<void> {
  await vscode.workspace
    .getConfiguration()
    .update(SETTING_ENABLED, enabled, vscode.ConfigurationTarget.Workspace);
}

export async function writeRagEmbedder(embedder: RagEmbedder): Promise<void> {
  await vscode.workspace
    .getConfiguration()
    .update(SETTING_EMBEDDER, embedder, vscode.ConfigurationTarget.Workspace);
}

/** Debounce window for RAG settings changes. Coalesces rapid double
 * fires (settings.json batched save, QuickPick double-click, …) into a
 * single callback so the supervisor restarts only once per intent. */
const WATCH_DEBOUNCE_MS = 200;

/**
 * Subscribes to configuration changes that affect the RAG flags. The
 * callback fires `WATCH_DEBOUNCE_MS` after the LAST observed change
 * to either `ragEnabled` or `ragEmbedder` — rapid double-fires
 * collapse into a single callback. Returns a disposable that cancels
 * the pending fire on dispose.
 */
export function watchRagSettings(callback: (next: RagSettings) => void): vscode.Disposable {
  let pending: ReturnType<typeof setTimeout> | null = null;
  const subscription = vscode.workspace.onDidChangeConfiguration(e => {
    if (!(e.affectsConfiguration(SETTING_ENABLED) || e.affectsConfiguration(SETTING_EMBEDDER))) {
      return;
    }
    if (pending !== null) clearTimeout(pending);
    pending = setTimeout(() => {
      pending = null;
      callback(readRagSettings());
    }, WATCH_DEBOUNCE_MS);
  });
  return {
    dispose: () => {
      if (pending !== null) {
        clearTimeout(pending);
        pending = null;
      }
      subscription.dispose();
    },
  };
}
