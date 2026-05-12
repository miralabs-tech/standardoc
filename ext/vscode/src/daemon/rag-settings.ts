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

/**
 * Subscribes to configuration changes that affect the RAG flags. Returns
 * a disposable. `callback` is invoked synchronously on every change
 * that touches either `ragEnabled` or `ragEmbedder` — the caller
 * decides whether to debounce or trigger a restart.
 */
export function watchRagSettings(callback: (next: RagSettings) => void): vscode.Disposable {
  return vscode.workspace.onDidChangeConfiguration(e => {
    if (e.affectsConfiguration(SETTING_ENABLED) || e.affectsConfiguration(SETTING_EMBEDDER)) {
      callback(readRagSettings());
    }
  });
}
