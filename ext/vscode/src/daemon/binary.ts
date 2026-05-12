import * as vscode from 'vscode';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { resolveBinaryWith, STANDARDOC_EXE, type ResolvedBinary } from './binary-resolver';

export { BinaryNotFoundError, STANDARDOC_EXE, type ResolvedBinary } from './binary-resolver';

export async function resolveBinary(context: vscode.ExtensionContext): Promise<ResolvedBinary> {
  const config = vscode.workspace.getConfiguration('standardoc');
  return resolveBinaryWith({
    settingsPath: config.get<string>('binaryPath'),
    bundledPath: path.join(context.extensionPath, 'dist', 'bin', STANDARDOC_EXE),
    pathEnv: process.env.PATH,
    pathSeparator: path.delimiter,
    exeName: STANDARDOC_EXE,
    existsSync: fs.existsSync,
  });
}
