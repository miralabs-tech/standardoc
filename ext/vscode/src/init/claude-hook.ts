/**
 * Pure shape + merge helpers for the `.claude/settings.json` UserPromptSubmit
 * hook that nudges Claude Code agents to use Standardoc's MCP tools before
 * falling back to raw Read/Grep/Glob.
 *
 * The hook is identified by a unique marker string embedded in its command;
 * `mergeClaudeHook` is idempotent against that marker so re-running the
 * workspace init never duplicates the entry.
 */

export interface ClaudeHookEntry {
  readonly type: 'command';
  readonly command: string;
}

export interface ClaudeHookMatcherGroup {
  readonly matcher?: string;
  readonly hooks: ClaudeHookEntry[];
}

export interface ClaudeSettingsShape {
  hooks?: {
    UserPromptSubmit?: ClaudeHookMatcherGroup[];
    [key: string]: ClaudeHookMatcherGroup[] | undefined;
  };
  [key: string]: unknown;
}

/** Embedded in the hook command so the merger can detect prior installs. */
export const STANDARDOC_HOOK_MARKER = 'STANDARDOC_MCP_NUDGE';

/** Single-line reminder injected ahead of every user prompt. */
export const STANDARDOC_HOOK_MESSAGE =
  'Standardoc live AST index is available via MCP tools (find_symbol, get_context, list_symbols, find_symbols_by_pattern, find_similar_symbols, current_revision, check_stale). Use them BEFORE Read/Grep/Glob for any code task.';

/** The exact shell command shipped with the hook; the marker is grep-stable. */
export const STANDARDOC_HOOK_COMMAND = `echo "${STANDARDOC_HOOK_MARKER}: ${STANDARDOC_HOOK_MESSAGE}"`;

export function buildStandardocHookGroup(): ClaudeHookMatcherGroup {
  return {
    matcher: '',
    hooks: [{ type: 'command', command: STANDARDOC_HOOK_COMMAND }],
  };
}

export type ParseResult =
  | { kind: 'absent' }
  | { kind: 'invalid'; error: string }
  | { kind: 'parsed'; value: ClaudeSettingsShape };

export function parseClaudeSettings(raw: string | null): ParseResult {
  if (raw === null) return { kind: 'absent' };
  if (raw.trim() === '') return { kind: 'parsed', value: {} };
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (e) {
    return { kind: 'invalid', error: e instanceof Error ? e.message : String(e) };
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return { kind: 'invalid', error: 'Root of .claude/settings.json must be a JSON object' };
  }
  return { kind: 'parsed', value: parsed as ClaudeSettingsShape };
}

export type MergeAction =
  | { kind: 'no-op' }
  | { kind: 'create'; result: ClaudeSettingsShape }
  | { kind: 'append'; result: ClaudeSettingsShape }
  | { kind: 'invalid'; error: string };

/**
 * Merge our Standardoc hook group into a parsed `.claude/settings.json`.
 *
 * - `absent` (file missing) → emit `create` with a freshly-minted object.
 * - Existing settings without our marker → `append` ours to `UserPromptSubmit`.
 * - Existing settings whose `UserPromptSubmit` already contains a group with
 *   our marker (any matcher) → `no-op`. We never duplicate, never overwrite
 *   user-side groups, and never re-order other groups.
 */
export function mergeClaudeHook(parsed: ParseResult): MergeAction {
  if (parsed.kind === 'invalid') return { kind: 'invalid', error: parsed.error };

  const existing: ClaudeSettingsShape = parsed.kind === 'absent' ? {} : parsed.value;
  const existingGroups = existing.hooks?.UserPromptSubmit ?? [];

  if (containsStandardocHook(existingGroups)) {
    return { kind: 'no-op' };
  }

  const ourGroup = buildStandardocHookGroup();
  const nextGroups: ClaudeHookMatcherGroup[] = [...existingGroups, ourGroup];
  const nextHooks = { ...(existing.hooks ?? {}), UserPromptSubmit: nextGroups };
  const result: ClaudeSettingsShape = { ...existing, hooks: nextHooks };

  if (parsed.kind === 'absent') {
    return { kind: 'create', result };
  }
  return { kind: 'append', result };
}

function containsStandardocHook(groups: ReadonlyArray<ClaudeHookMatcherGroup>): boolean {
  for (const g of groups) {
    for (const h of g.hooks ?? []) {
      if (h.type === 'command' && typeof h.command === 'string'
        && h.command.includes(STANDARDOC_HOOK_MARKER)) {
        return true;
      }
    }
  }
  return false;
}

export function serializeClaudeSettings(settings: ClaudeSettingsShape): string {
  return JSON.stringify(settings, null, 2) + '\n';
}
