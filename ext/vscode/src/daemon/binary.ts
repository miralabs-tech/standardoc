import * as vscode from 'vscode';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { resolveBinaryWith, STDOC_EXE, type ResolvedBinary } from './binary-resolver';

export { BinaryNotFoundError, STDOC_EXE, type ResolvedBinary } from './binary-resolver';

export async function resolveBinary(context: vscode.ExtensionContext): Promise<ResolvedBinary> {
  const config = vscode.workspace.getConfiguration('standardoc');
  return resolveBinaryWith({
    settingsPath: config.get<string>('binaryPath'),
    bundledPath: path.join(context.extensionPath, 'dist', 'bin', STDOC_EXE),
    pathEnv: process.env.PATH,
    pathSeparator: path.delimiter,
    exeName: STDOC_EXE,
    existsSync: fs.existsSync,
  });
}
