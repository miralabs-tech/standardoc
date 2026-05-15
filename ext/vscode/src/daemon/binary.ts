import * as vscode from 'vscode';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { resolveBinaryWith, STANDARDOC_EXE, type ResolvedBinary } from './binary-resolver';
import { currentPlatformTarget } from './binary-installer';

export { BinaryNotFoundError, STANDARDOC_EXE, type ResolvedBinary } from './binary-resolver';

export async function resolveBinary(context: vscode.ExtensionContext): Promise<ResolvedBinary> {
  const config = vscode.workspace.getConfiguration('standardoc');
  return resolveBinaryWith({
    settingsPath: config.get<string>('binaryPath'),
    globalStoragePath: globalStorageBinaryPath(context),
    pathEnv: process.env.PATH,
    pathSeparator: path.delimiter,
    exeName: STANDARDOC_EXE,
    existsSync: fs.existsSync,
  });
}

export function globalStorageBinaryPath(context: vscode.ExtensionContext): string {
  const target = currentPlatformTarget();
  // Unsupported platforms still get a synthetic path so the resolver's
  // existsSync probe returns false cleanly; the installer is the one
  // that surfaces UnsupportedPlatformError when actually invoked.
  const triple = target?.triple ?? 'unsupported';
  const exe = target?.exe ?? STANDARDOC_EXE;
  return path.join(context.globalStorageUri.fsPath, 'bin', triple, exe);
}
