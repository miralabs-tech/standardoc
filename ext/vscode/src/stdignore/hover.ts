import * as vscode from 'vscode';
import { renderHoverMarkdown } from './hover-render';
import { runPreviewSubprocess } from '../preview-runner';

const LANGUAGE_ID = 'stdignore';
const PREVIEW_LIMIT = 20;
const PREVIEW_TIMEOUT_MS = 5_000;

export function registerStdignoreHover(
  context: vscode.ExtensionContext,
  workspaceRoot: string,
  output: vscode.OutputChannel,
): void {
  const provider: vscode.HoverProvider = {
    async provideHover(document, position, token) {
      if (document.languageId !== LANGUAGE_ID) return null;
      const lineText = document.lineAt(position.line).text;
      const pattern = lineText.trim();
      if (pattern.length === 0 || pattern.startsWith('#')) return null;
      try {
        const preview = await runPreviewSubprocess(context, output, token, {
          subcommand: 'stdignore-preview',
          workspaceRoot,
          pattern,
          limit: PREVIEW_LIMIT,
          timeoutMs: PREVIEW_TIMEOUT_MS,
          logPrefix: '[stdignore-hover]',
        });
        if (token.isCancellationRequested) return null;
        const md = new vscode.MarkdownString(renderHoverMarkdown(preview), false);
        md.isTrusted = false;
        return new vscode.Hover(md);
      } catch (e) {
        output.appendLine(
          `[stdignore-hover] preview failed: ${e instanceof Error ? e.message : String(e)}`,
        );
        return null;
      }
    },
  };
  context.subscriptions.push(vscode.languages.registerHoverProvider(LANGUAGE_ID, provider));
}
