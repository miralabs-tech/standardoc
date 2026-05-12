import * as path from 'node:path';

export interface ResolvedBinary {
  readonly path: string;
  readonly source: 'settings' | 'bundled' | 'path';
}

const DEFAULT_NOT_FOUND_MESSAGE =
  'Standardoc binary not found. Set `standardoc.binaryPath`, bundle a binary into `dist/bin/`, or place `standardoc` on PATH.';

export class BinaryNotFoundError extends Error {
  constructor(message?: string) {
    super(message ?? DEFAULT_NOT_FOUND_MESSAGE);
    this.name = 'BinaryNotFoundError';
  }
}

export const STANDARDOC_EXE = process.platform === 'win32' ? 'standardoc.exe' : 'standardoc';

export interface ResolveDeps {
  readonly settingsPath: string | undefined;
  readonly bundledPath: string;
  readonly pathEnv: string | undefined;
  readonly pathSeparator: string;
  readonly exeName: string;
  readonly existsSync: (p: string) => boolean;
}

export function resolveBinaryWith(deps: ResolveDeps): ResolvedBinary {
  const settings = deps.settingsPath?.trim();
  if (settings) {
    if (deps.existsSync(settings)) {
      return { path: settings, source: 'settings' };
    }
    throw new BinaryNotFoundError(
      `Standardoc binary path \`${settings}\` (from \`standardoc.binaryPath\` setting) does not exist.`,
    );
  }
  if (deps.existsSync(deps.bundledPath)) {
    return { path: deps.bundledPath, source: 'bundled' };
  }
  const fromPath = lookupOnPath(deps);
  if (fromPath) {
    return { path: fromPath, source: 'path' };
  }
  throw new BinaryNotFoundError();
}

function lookupOnPath(deps: ResolveDeps): string | null {
  if (!deps.pathEnv) return null;
  for (const dir of deps.pathEnv.split(deps.pathSeparator)) {
    if (!dir) continue;
    const candidate = path.join(dir, deps.exeName);
    if (deps.existsSync(candidate)) return candidate;
  }
  return null;
}
