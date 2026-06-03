import * as vscode from 'vscode';
import { renderPreviewMarkdown } from '../stdignore/hover-render';
import { detectHoverTarget } from './hover-detect';
import { runPreviewSubprocess } from '../preview-runner';

const LANGUAGE_ID = 'standardoc-sxd';
const PREVIEW_LIMIT = 20;
const PREVIEW_TIMEOUT_MS = 5_000;

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
        const preview = await runPreviewSubprocess(context, output, token, {
          subcommand: 'sxd-preview',
          workspaceRoot,
          pattern: target.value,
          limit: PREVIEW_LIMIT,
          timeoutMs: PREVIEW_TIMEOUT_MS,
          logPrefix: '[sxd-hover]',
        });
        if (token.isCancellationRequested) return null;
        const label = target.kind === 'pattern' ? 'Pattern' : 'Path';
        const md = new vscode.MarkdownString(renderPreviewMarkdown(preview, label), false);
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
