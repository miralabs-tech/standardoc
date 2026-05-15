import { execFile } from 'node:child_process';

export interface PreflightOk {
  readonly ok: true;
  readonly db: number | null;
  readonly supported: number;
}

export interface PreflightFail {
  readonly ok: false;
  readonly reason: string;
  readonly db: number | null;
  readonly supported: number | null;
}

export type PreflightResult = PreflightOk | PreflightFail;

export type ExecFn = (
  binary: string,
  args: string[],
) => Promise<{ stdout: string; stderr: string }>;

interface RawPayload {
  readonly db: number | null;
  readonly supported: number;
  readonly compatible: boolean;
}

function isRawPayload(value: unknown): value is RawPayload {
  if (typeof value !== 'object' || value === null) return false;
  const o = value as Record<string, unknown>;
  const dbOk = o.db === null || typeof o.db === 'number';
  const suppOk = typeof o.supported === 'number';
  const compatOk = typeof o.compatible === 'boolean';
  return dbOk && suppOk && compatOk;
}

/**
 * Spawn `<binary> schema-version <workspaceRoot>` and parse the JSON it
 * prints on stdout. Returns an actionable PreflightResult — never throws on
 * an expected failure path; only the exec layer can reject.
 *
 * The binary itself decides what is compatible: it knows both its compiled-in
 * `SUPPORTED_SCHEMA_VERSION` and the on-disk `schema_meta.schema_version`.
 * We surface its verdict here and let the supervisor decide what to do
 * (typically block the daemon spawn + show a toast).
 */
export async function preflightSchemaVersion(
  binary: string,
  workspaceRoot: string,
  exec: ExecFn,
): Promise<PreflightResult> {
  let result: { stdout: string; stderr: string };
  try {
    result = await exec(binary, ['schema-version', workspaceRoot]);
  } catch (e) {
    return {
      ok: false,
      reason: `schema-version invocation failed: ${e instanceof Error ? e.message : String(e)}`,
      db: null,
      supported: null,
    };
  }

  const trimmed = result.stdout.trim();
  let raw: unknown;
  try {
    raw = JSON.parse(trimmed);
  } catch (e) {
    return {
      ok: false,
      reason: `schema-version output is not valid JSON: ${trimmed}`,
      db: null,
      supported: null,
    };
  }

  if (!isRawPayload(raw)) {
    return {
      ok: false,
      reason: `schema-version output has unexpected shape: ${trimmed}`,
      db: null,
      supported: null,
    };
  }

  if (!raw.compatible) {
    const dbStr = raw.db === null ? 'unknown' : `v${raw.db}`;
    return {
      ok: false,
      reason: `DB schema ${dbStr} > binary supports v${raw.supported}. Update the Standardoc extension or remove .standardoc/.`,
      db: raw.db,
      supported: raw.supported,
    };
  }

  return { ok: true, db: raw.db, supported: raw.supported };
}

/**
 * Default exec implementation backed by `child_process.execFile`. Pure
 * modules (tests) can pass their own `ExecFn` to bypass the network of
 * Node side effects.
 */
export const defaultExec: ExecFn = (binary, args) =>
  new Promise((resolve, reject) => {
    execFile(binary, args, (err, stdout, stderr) => {
      if (err) reject(err);
      else resolve({ stdout, stderr });
    });
  });
