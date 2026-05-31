/**
 * Pure shape + merge helpers for the `.claude/settings.json` hooks that
 * enforce Standardoc's MCP-first discipline.
 *
 * Four hooks are managed here (all idempotent — each is identified by a
 * unique marker substring embedded in its `command`):
 * - `UserPromptSubmit`: advisory nudge surfacing the MCP tool surface.
 * - `PreToolUse` (mcp__standardoc__.*): marks the sentinel that proves the
 *   agent has paid the MCP-first toll for this session.
 * - `PreToolUse` (Bash|Read|Grep|Glob): denies the call when the sentinel
 *   is absent — this is the actual MCP-first guardrail.
 * - `SessionStart`: resets the sentinel so each new chat starts strict.
 *
 * `mergeClaudeHook` greps for each marker in the existing settings so
 * re-running `init` after an upgrade only appends the missing entries —
 * pre-existing user-authored hooks are never reordered or removed.
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
    PreToolUse?: ClaudeHookMatcherGroup[];
    PostToolUse?: ClaudeHookMatcherGroup[];
    SessionStart?: ClaudeHookMatcherGroup[];
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

/**
 * Grep-stable signatures for the three MCP-first PreToolUse/SessionStart
 * hooks. Each `--mode <X>` argument is unique to the standardoc binary,
 * so the substring doubles as the marker — `mergeClaudeHook` greps for
 * it to keep re-runs of `init` idempotent.
 */
export const STANDARDOC_MCP_FIRST_MARK_MARKER = 'pre-tool-hook --mode mark';
export const STANDARDOC_MCP_FIRST_CHECK_MARKER = 'pre-tool-hook --mode check';
export const STANDARDOC_MCP_FIRST_RESET_MARKER = 'pre-tool-hook --mode reset';

/**
 * Exact shell commands. The Rust binary is resolved via `PATH` (the same
 * `standardoc.exe` the daemon supervisor launches), so a single string
 * works cross-OS — no per-platform `powershell` / `bash` adaptation
 * needed at this layer.
 */
export const STANDARDOC_MCP_FIRST_MARK_COMMAND = 'standardoc claude pre-tool-hook --mode mark';
export const STANDARDOC_MCP_FIRST_CHECK_COMMAND = 'standardoc claude pre-tool-hook --mode check';
export const STANDARDOC_MCP_FIRST_RESET_COMMAND = 'standardoc claude pre-tool-hook --mode reset';

export function buildStandardocHookGroup(): ClaudeHookMatcherGroup {
  return {
    matcher: '',
    hooks: [{ type: 'command', command: STANDARDOC_HOOK_COMMAND }],
  };
}

/**
 * PreToolUse hook group fired on every standardoc MCP tool call. Touches
 * the sentinel `<cwd>/.standardoc/mcp_called_this_session` so the check
 * hook lets subsequent Bash/Read/Grep/Glob through for the rest of the
 * chat. The matcher relies on Claude Code's tool-name convention:
 * `mcp__<server>__<tool>` — only standardoc's own tools toll the marker.
 */
export function buildStandardocMcpFirstMarkHookGroup(): ClaudeHookMatcherGroup {
  return {
    matcher: 'mcp__standardoc__.*',
    hooks: [{ type: 'command', command: STANDARDOC_MCP_FIRST_MARK_COMMAND }],
  };
}

/**
 * PreToolUse hook group fired on raw code-exploration tools. When the
 * sentinel is absent for this chat, the binary emits a JSON
 * `permissionDecision: "deny"` on stdout — Claude Code blocks the call
 * and surfaces the reason to the agent, who is expected to switch to
 * MCP. When the sentinel exists (i.e. the agent already used MCP this
 * chat), the binary emits `{}` and the call proceeds.
 */
export function buildStandardocMcpFirstCheckHookGroup(): ClaudeHookMatcherGroup {
  return {
    matcher: 'Bash|Read|Grep|Glob',
    hooks: [{ type: 'command', command: STANDARDOC_MCP_FIRST_CHECK_COMMAND }],
  };
}

/**
 * SessionStart hook group: removes the sentinel so every new chat starts
 * MCP-first-strict, regardless of the previous chat's history.
 */
export function buildStandardocMcpFirstResetHookGroup(): ClaudeHookMatcherGroup {
  return {
    matcher: '',
    hooks: [{ type: 'command', command: STANDARDOC_MCP_FIRST_RESET_COMMAND }],
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
 * Merge the four Standardoc hook groups into a parsed `.claude/settings.json`:
 * - `UserPromptSubmit` MCP nudge (advisory).
 * - `PreToolUse` (mcp__standardoc__.*) MCP-first mark.
 * - `PreToolUse` (Bash|Read|Grep|Glob) MCP-first check (the actual deny).
 * - `SessionStart` MCP-first reset.
 *
 * Behaviour:
 * - `absent` (file missing) → `create` with all four hooks installed.
 * - Existing settings missing some → `append` only the missing ones; never
 *   reorder or remove pre-existing user-authored entries.
 * - All four already present → `no-op`. Idempotent across re-runs.
 *
 * Detection uses each hook's grep-stable marker substring inside the
 * command string, so any matcher value still counts.
 */
export function mergeClaudeHook(parsed: ParseResult): MergeAction {
  if (parsed.kind === 'invalid') return { kind: 'invalid', error: parsed.error };

  const existing: ClaudeSettingsShape = parsed.kind === 'absent' ? {} : parsed.value;
  const existingUserPrompt = existing.hooks?.UserPromptSubmit ?? [];
  const existingPreTool = existing.hooks?.PreToolUse ?? [];
  const existingSessionStart = existing.hooks?.SessionStart ?? [];

  const hasMcpNudge = containsMarker(existingUserPrompt, STANDARDOC_HOOK_MARKER);
  const hasMcpFirstMark = containsMarker(existingPreTool, STANDARDOC_MCP_FIRST_MARK_MARKER);
  const hasMcpFirstCheck = containsMarker(existingPreTool, STANDARDOC_MCP_FIRST_CHECK_MARKER);
  const hasMcpFirstReset = containsMarker(existingSessionStart, STANDARDOC_MCP_FIRST_RESET_MARKER);

  if (hasMcpNudge && hasMcpFirstMark && hasMcpFirstCheck && hasMcpFirstReset) {
    return { kind: 'no-op' };
  }

  const nextUserPrompt = hasMcpNudge
    ? existingUserPrompt
    : [...existingUserPrompt, buildStandardocHookGroup()];

  const nextPreTool = [...existingPreTool];
  if (!hasMcpFirstMark) nextPreTool.push(buildStandardocMcpFirstMarkHookGroup());
  if (!hasMcpFirstCheck) nextPreTool.push(buildStandardocMcpFirstCheckHookGroup());

  const nextSessionStart = hasMcpFirstReset
    ? existingSessionStart
    : [...existingSessionStart, buildStandardocMcpFirstResetHookGroup()];

  const nextHooks = {
    ...(existing.hooks ?? {}),
    UserPromptSubmit: nextUserPrompt,
    PreToolUse: nextPreTool,
    SessionStart: nextSessionStart,
  };
  const result: ClaudeSettingsShape = { ...existing, hooks: nextHooks };

  if (parsed.kind === 'absent') {
    return { kind: 'create', result };
  }
  return { kind: 'append', result };
}

function containsMarker(
  groups: ReadonlyArray<ClaudeHookMatcherGroup>,
  marker: string,
): boolean {
  for (const g of groups) {
    for (const h of g.hooks ?? []) {
      if (h.type === 'command' && typeof h.command === 'string'
        && h.command.includes(marker)) {
        return true;
      }
    }
  }
  return false;
}

export function serializeClaudeSettings(settings: ClaudeSettingsShape): string {
  return JSON.stringify(settings, null, 2) + '\n';
}
