import * as vscode from 'vscode';
import { matcher } from 'matchigo';
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
  .with({ kind: 'ready' }, ({ pid }) => ({
    text: '$(check) Standardoc',
    tooltip: `Standardoc daemon ready (pid ${pid})`,
  }))
  .with({ kind: 'restarting' }, ({ attempt }) => ({
    text: '$(sync~spin) Standardoc',
    tooltip: `Standardoc daemon restarting (attempt ${attempt})…`,
  }))
  .with({ kind: 'failed' }, ({ reason }) => ({
    text: '$(error) Standardoc',
    tooltip: `Standardoc daemon failed: ${reason}`,
  }))
  .exhaustive();

export class StatusBarController implements vscode.Disposable {
  private readonly item: vscode.StatusBarItem;

  constructor() {
    this.item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
    this.item.command = 'Standardoc.daemon.restart';
    this.item.show();
  }

  update(state: DaemonState): void {
    const r = renderStatus(state);
    this.item.text = r.text;
    this.item.tooltip = r.tooltip;
  }

  dispose(): void {
    this.item.dispose();
  }
}
