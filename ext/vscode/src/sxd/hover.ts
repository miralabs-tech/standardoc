import * as vscode from 'vscode';
import { spawn } from 'node:child_process';
import { resolveBinary } from '../daemon/binary';
import { renderHoverMarkdown, type PatternPreview } from '../stdignore/hover-render';

const LANGUAGE_ID = 'standardoc-sxd';
const PREVIEW_LIMIT = 20;
const PREVIEW_TIMEOUT_MS = 5_000;
const HOVER_DEBOUNCE_MS = 250;

const PATTERNS_OPEN_RE = /^\s*patterns\s+```/;
const PATTERNS_CLOSE_RE = /^\s*```/;

export function registerSxdHover(
  context: vscode.ExtensionContext,
  workspaceRoot: string,
  output: vscode.OutputChannel,
): void {
  const provider: vscode.HoverProvider = {
    async provideHover(document, position, token) {
      if (document.languageId !== LANGUAGE_ID) return null;
      if (!isInsideIgnorePatternsBlock(document, position.line)) return null;
      const lineText = document.lineAt(position.line).text;
      const pattern = lineText.trim();
      if (pattern.length === 0 || pattern.startsWith('#') || PATTERNS_CLOSE_RE.test(lineText)) {
        return null;
      }
      try {
        const preview = await runPreview(context, workspaceRoot, pattern, output, token);
        if (token.isCancellationRequested) return null;
        const md = new vscode.MarkdownString(renderHoverMarkdown(preview), false);
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

// Heuristic: walk back from `lineNumber` looking for `patterns ```` ; bail
// if we hit a `}` (block close) or another `patterns ```` first. Symmetric
// forward scan for the closing fence. The hover only fires inside that
// range — anything outside (version, project blocks, ignore { ... } braces,
// blank lines, comments above the block) is suppressed.
function isInsideIgnorePatternsBlock(
  document: vscode.TextDocument,
  lineNumber: number,
): boolean {
  let opened = false;
  for (let i = lineNumber - 1; i >= 0; i--) {
    const text = document.lineAt(i).text;
    if (PATTERNS_OPEN_RE.test(text)) {
      opened = true;
      break;
    }
    if (/^\s*\}/.test(text) || /^\s*[a-z]+\s*\{/.test(text)) {
      return false;
    }
  }
  if (!opened) return false;
  for (let i = lineNumber + 1; i < document.lineCount; i++) {
    const text = document.lineAt(i).text;
    if (PATTERNS_CLOSE_RE.test(text)) return true;
    if (PATTERNS_OPEN_RE.test(text)) return false;
  }
  return false;
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
  isInsideIgnorePatternsBlock,
};
