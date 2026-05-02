import { matcher } from 'matchigo';

export type DaemonState =
  | { kind: 'stopped' }
  | { kind: 'starting' }
  | { kind: 'ready'; pid: number }
  | { kind: 'restarting'; attempt: number }
  | { kind: 'failed'; reason: string };

export const describeState = matcher<DaemonState, string>()
  .with({ kind: 'stopped' }, () => 'Stopped')
  .with({ kind: 'starting' }, () => 'Starting')
  .with({ kind: 'ready' }, ({ pid }) => `Ready (pid ${pid})`)
  .with({ kind: 'restarting' }, ({ attempt }) => `Restarting (attempt ${attempt})`)
  .with({ kind: 'failed' }, ({ reason }) => `Failed: ${reason}`)
  .exhaustive();

export const BACKOFF_MS: ReadonlyArray<number> = [0, 2000, 8000];
export const STABLE_UPTIME_MS = 5 * 60_000;
export const CRASH_WINDOW_MS = 60_000;
