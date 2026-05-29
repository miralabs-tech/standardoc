import * as vscode from 'vscode';
import { spawn } from 'node:child_process';
import { resolveBinary } from '../daemon/binary';
import {
  renderPreviewMarkdown,
  type PatternPreview,
} from '../stdignore/hover-render';
import { detectHoverTarget } from './hover-detect';

const LANGUAGE_ID = 'standardoc-sxd';
const PREVIEW_LIMIT = 20;
const PREVIEW_TIMEOUT_MS = 5_000;
const HOVER_DEBOUNCE_MS = 250;

export function registerSxdHover(
  context: vscode.ExtensionContext,
  workspaceRoot: string,
  output: vscode.OutputChannel,
): void {
  const provider: vscode.HoverProvider = {
    async provideHover(document, position, token) {
      if (document.languageId !== LANGUAGE_ID) return null;
      const target = detectHoverTarget(document, position);
      if (!target) return null;
      try {
        const preview = await runPreview(context, workspaceRoot, target.value, output, token);
        if (token.isCancellationRequested) return null;
        const label = target.kind === 'pattern' ? 'Pattern' : 'Path';
        const md = new vscode.MarkdownString(
          renderPreviewMarkdown(preview, label),
          false,
        );
        md.isTrusted = false;
        return new vscode.Hover(md);
      } catch (e) {
        output.appendLine(
          `[sxd-hover] preview failed: ${e instanceof Error ? e.message : String(e)}`,
        );
        return null;
      }
    },
  };
  context.subscriptions.push(vscode.languages.registerHoverProvider(LANGUAGE_ID, provider));
}

async function runPreview(
  context: vscode.ExtensionContext,
  workspaceRoot: string,
  pattern: string,
  output: vscode.OutputChannel,
  token: vscode.CancellationToken,
): Promise<PatternPreview> {
  const binary = await resolveBinary(context);
  const args = [
    'sxd-preview',
    workspaceRoot,
    '--pattern',
    pattern,
    '--limit',
    String(PREVIEW_LIMIT),
  ];
  return await new Promise<PatternPreview>((resolve, reject) => {
    const child = spawn(binary.path, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`sxd-preview timed out after ${PREVIEW_TIMEOUT_MS}ms`));
    }, PREVIEW_TIMEOUT_MS);
    const cancelSub = token.onCancellationRequested(() => {
      child.kill();
      reject(new Error('cancelled'));
    });
    child.stdout?.on('data', chunk => (stdout += chunk.toString()));
    child.stderr?.on('data', chunk => (stderr += chunk.toString()));
    child.on('error', err => {
      clearTimeout(timer);
      cancelSub.dispose();
      reject(err);
    });
    child.on('close', code => {
      clearTimeout(timer);
      cancelSub.dispose();
      if (code !== 0) {
        output.appendLine(`[sxd-hover] child exit ${code}, stderr: ${stderr.trim()}`);
        reject(new Error(`sxd-preview exited ${code}`));
        return;
      }
      try {
        const json = JSON.parse(stdout) as PatternPreview;
        resolve(json);
      } catch (e) {
        reject(new Error(`parse JSON failed: ${e instanceof Error ? e.message : String(e)}`));
      }
    });
  });
}

export const __test_internals = {
  HOVER_DEBOUNCE_MS,
  PREVIEW_LIMIT,
  PREVIEW_TIMEOUT_MS,
  LANGUAGE_ID,
};
