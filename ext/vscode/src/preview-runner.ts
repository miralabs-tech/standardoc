import * as vscode from 'vscode';
import { spawn } from 'node:child_process';
import { resolveBinary } from './daemon/binary';
import type { PatternPreview } from './stdignore/hover-render';

// Shared subprocess runner behind the `.stdignore` and `.sxd` hover
// previews. Both shell out to a `standardoc <kind>-preview` subcommand
// that prints a `PatternPreview` JSON; only the subcommand and the log
// prefix differ, so the spawn / timeout / cancellation / parse plumbing
// lives here once.

export interface PreviewRequest {
  readonly subcommand: 'stdignore-preview' | 'sxd-preview';
  readonly workspaceRoot: string;
  readonly pattern: string;
  readonly limit: number;
  readonly timeoutMs: number;
  /** Output-channel prefix, e.g. `[stdignore-hover]`. */
  readonly logPrefix: string;
}

export async function runPreviewSubprocess(
  context: vscode.ExtensionContext,
  output: vscode.OutputChannel,
  token: vscode.CancellationToken,
  req: PreviewRequest,
): Promise<PatternPreview> {
  const binary = await resolveBinary(context);
  const args = [
    req.subcommand,
    req.workspaceRoot,
    '--pattern',
    req.pattern,
    '--limit',
    String(req.limit),
  ];
  return await new Promise<PatternPreview>((resolve, reject) => {
    const child = spawn(binary.path, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`${req.subcommand} timed out after ${req.timeoutMs}ms`));
    }, req.timeoutMs);
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
        output.appendLine(`${req.logPrefix} child exit ${code}, stderr: ${stderr.trim()}`);
        reject(new Error(`${req.subcommand} exited ${code}`));
        return;
      }
      try {
        resolve(JSON.parse(stdout) as PatternPreview);
      } catch (e) {
        reject(new Error(`parse JSON failed: ${e instanceof Error ? e.message : String(e)}`));
      }
    });
  });
}
