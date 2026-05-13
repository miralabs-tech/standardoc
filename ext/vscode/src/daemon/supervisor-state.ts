import { matcher } from 'matchigo';
import { describeFatalConfig, type FatalConfig } from './fatal-marker';

export type DaemonState =
  | { kind: 'stopped' }
  | { kind: 'starting' }
  | { kind: 'ready' }
  | { kind: 'restarting'; attempt: number }
  | { kind: 'failed'; reason: string }
  /**
   * The daemon's stderr emitted a structured `STDOC_FATAL` marker that
   * tells the supervisor that retrying is pointless until the user fixes
   * the host-side configuration (typically: rebuild + re-install the
   * binary after a schema bump). Distinct from `failed` so the UI can
   * surface an actionable hint and the backoff machinery stays put.
   */
  | { kind: 'fatal_config'; config: FatalConfig };

export const describeState = matcher<DaemonState, string>()
  .with({ kind: 'stopped' }, () => 'Stopped')
  .with({ kind: 'starting' }, () => 'Starting')
  // 'ready' carries no PID — the supervisor aggregates two separate
  // children (LSP + MCP). Displaying `pid 0` was a placeholder leak.
  .with({ kind: 'ready' }, () => 'Ready')
  .with({ kind: 'restarting' }, ({ attempt }) => `Restarting (attempt ${attempt})`)
  .with({ kind: 'failed' }, ({ reason }) => `Failed: ${reason}`)
  .with({ kind: 'fatal_config' }, ({ config }) => `Fatal config: ${describeFatalConfig(config)}`)
  .exhaustive();

export const BACKOFF_MS: ReadonlyArray<number> = [0, 2000, 8000];
export const STABLE_UPTIME_MS = 5 * 60_000;
export const CRASH_WINDOW_MS = 60_000;
