import * as vscode from 'vscode';
import { matcher } from 'matchigo';
import { describeFatalConfig } from './daemon/fatal-marker';
import type { RagSettings } from './daemon/rag-flags';
import type { DaemonState } from './daemon/supervisor';

interface StatusRender {
  readonly text: string;
  readonly tooltip: string;
}

const renderStatus = matcher<DaemonState, StatusRender>()
  .with({ kind: 'stopped' }, () => ({
    text: '$(circle-slash) Standardoc',
    tooltip: 'Standardoc daemon stopped',
  }))
  .with({ kind: 'starting' }, () => ({
    text: '$(sync~spin) Standardoc',
    tooltip: 'Standardoc daemon starting…',
  }))
  .with({ kind: 'ready' }, () => ({
    text: '$(check) Standardoc',
    tooltip: 'Standardoc daemon ready',
  }))
  .with({ kind: 'restarting' }, ({ attempt }) => ({
    text: '$(sync~spin) Standardoc',
    tooltip: `Standardoc daemon restarting (attempt ${attempt})…`,
  }))
  .with({ kind: 'failed' }, ({ reason }) => ({
    text: '$(error) Standardoc',
    tooltip: `Standardoc daemon failed: ${reason}`,
  }))
  .with({ kind: 'fatal_config' }, ({ config }) => ({
    text: '$(warning) Standardoc',
    tooltip: `Standardoc daemon halted — ${describeFatalConfig(config)}`,
  }))
  .exhaustive();

export class StatusBarController implements vscode.Disposable {
  private readonly item: vscode.StatusBarItem;

  constructor() {
    this.item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
    this.item.command = 'Standardoc.statusBarMenu';
    this.item.show();
  }

  update(state: DaemonState, rag?: RagSettings): void {
    const r = renderStatus(state);
    const ragSuffix = rag?.enabled ? ` · RAG (${rag.embedder})` : '';
    this.item.text = `${r.text}${ragSuffix}`;
    this.item.tooltip = rag?.enabled
      ? `${r.tooltip}\nRAG enabled (embedder: ${rag.embedder})`
      : r.tooltip;
  }

  dispose(): void {
    this.item.dispose();
  }
}
