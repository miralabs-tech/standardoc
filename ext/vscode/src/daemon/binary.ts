import * as vscode from 'vscode';

export interface ResolvedBinary {
  readonly path: string;
  readonly source: 'settings' | 'bundled' | 'path';
}

export class BinaryNotFoundError extends Error {
  constructor() {
    super(
      'Standardoc binary not found. Set `standardoc.binaryPath`, bundle a binary into `dist/bin/`, or place `standardoc` on PATH.',
    );
    this.name = 'BinaryNotFoundError';
  }
}

export const STANDARDOC_EXE = process.platform === 'win32' ? 'standardoc.exe' : 'standardoc';

export async function resolveBinary(_context: vscode.ExtensionContext): Promise<ResolvedBinary> {
  throw new Error(
    'TODO: Phase B — resolve standardoc binary via settings.binaryPath > bundled dist/bin/ > PATH lookup',
  );
}
