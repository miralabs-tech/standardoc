import { describe, expect, test } from 'bun:test';
import {
  STANDARDOC_HOOK_COMMAND,
  STANDARDOC_HOOK_MARKER,
  STANDARDOC_MCP_FIRST_CHECK_COMMAND,
  STANDARDOC_MCP_FIRST_CHECK_MARKER,
  STANDARDOC_MCP_FIRST_MARK_COMMAND,
  STANDARDOC_MCP_FIRST_MARK_MARKER,
  STANDARDOC_MCP_FIRST_RESET_COMMAND,
  STANDARDOC_MCP_FIRST_RESET_MARKER,
  STANDARDOC_SESSION_SYNC_COMMAND,
  STANDARDOC_SESSION_SYNC_MARKER,
  buildStandardocHookGroup,
  buildStandardocMcpFirstCheckHookGroup,
  buildStandardocMcpFirstMarkHookGroup,
  buildStandardocMcpFirstResetHookGroup,
  buildStandardocSessionSyncHookGroup,
  mergeClaudeHook,
  parseClaudeSettings,
  serializeClaudeSettings,
} from '../../src/init/claude-hook';

describe('parseClaudeSettings', () => {
  test('returns absent for null', () => {
    expect(parseClaudeSettings(null)).toEqual({ kind: 'absent' });
  });

  test('parses an empty document as parsed/empty', () => {
    expect(parseClaudeSettings('')).toEqual({ kind: 'parsed', value: {} });
  });

  test('returns invalid for malformed JSON', () => {
    const result = parseClaudeSettings('{ not json');
    expect(result.kind).toBe('invalid');
  });

  test('returns invalid when the root is not an object', () => {
    expect(parseClaudeSettings('[]').kind).toBe('invalid');
    expect(parseClaudeSettings('"foo"').kind).toBe('invalid');
  });

  test('parses a valid object', () => {
    const result = parseClaudeSettings(JSON.stringify({ hooks: {} }));
    expect(result.kind).toBe('parsed');
  });
});

describe('mergeClaudeHook', () => {
  test('absent file → create with all five hooks', () => {
    const action = mergeClaudeHook({ kind: 'absent' });
    expect(action.kind).toBe('create');
    if (action.kind === 'create') {
      const userPrompt = action.result.hooks?.UserPromptSubmit ?? [];
      expect(userPrompt.length).toBe(1);
      expect(userPrompt[0]?.hooks[0]?.command).toContain(STANDARDOC_HOOK_MARKER);

      const preTool = action.result.hooks?.PreToolUse ?? [];
      expect(preTool.length).toBe(2);
      expect(preTool[0]?.matcher).toBe('mcp__standardoc__.*');
      expect(preTool[0]?.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_MARK_COMMAND);
      expect(preTool[1]?.matcher).toBe('Bash|Read|Grep|Glob');
      expect(preTool[1]?.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_CHECK_COMMAND);

      const postTool = action.result.hooks?.PostToolUse ?? [];
      expect(postTool.length).toBe(1);
      expect(postTool[0]?.hooks[0]?.command).toContain(STANDARDOC_SESSION_SYNC_MARKER);

      const sessionStart = action.result.hooks?.SessionStart ?? [];
      expect(sessionStart.length).toBe(1);
      expect(sessionStart[0]?.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_RESET_COMMAND);
    }
  });

  test('existing settings without our hooks → append all five', () => {
    const result = mergeClaudeHook({ kind: 'parsed', value: { hooks: {} } });
    expect(result.kind).toBe('append');
    if (result.kind === 'append') {
      expect((result.result.hooks?.UserPromptSubmit ?? []).length).toBe(1);
      expect((result.result.hooks?.PreToolUse ?? []).length).toBe(2);
      expect((result.result.hooks?.PostToolUse ?? []).length).toBe(1);
      expect((result.result.hooks?.SessionStart ?? []).length).toBe(1);
    }
  });

  test('existing UserPromptSubmit groups → preserve and append ours last', () => {
    const userGroup = {
      matcher: 'foo',
      hooks: [{ type: 'command' as const, command: 'echo "user hook"' }],
    };
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: { hooks: { UserPromptSubmit: [userGroup] } },
    });
    expect(action.kind).toBe('append');
    if (action.kind === 'append') {
      const groups = action.result.hooks?.UserPromptSubmit ?? [];
      expect(groups.length).toBe(2);
      expect(groups[0]).toEqual(userGroup);
      expect(groups[1]?.hooks[0]?.command).toContain(STANDARDOC_HOOK_MARKER);
    }
  });

  test('existing PreToolUse user groups → preserve and append the two MCP-first groups last', () => {
    const userGroup = {
      matcher: 'WebFetch',
      hooks: [{ type: 'command' as const, command: 'echo "user pre-tool"' }],
    };
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: { hooks: { PreToolUse: [userGroup] } },
    });
    expect(action.kind).toBe('append');
    if (action.kind === 'append') {
      const groups = action.result.hooks?.PreToolUse ?? [];
      expect(groups.length).toBe(3);
      expect(groups[0]).toEqual(userGroup);
      expect(groups[1]?.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_MARK_COMMAND);
      expect(groups[2]?.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_CHECK_COMMAND);
    }
  });

  test('existing PostToolUse groups → preserve and append ours last', () => {
    const userGroup = {
      matcher: 'Bash',
      hooks: [{ type: 'command' as const, command: 'echo "user post-tool hook"' }],
    };
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: { hooks: { PostToolUse: [userGroup] } },
    });
    expect(action.kind).toBe('append');
    if (action.kind === 'append') {
      const groups = action.result.hooks?.PostToolUse ?? [];
      expect(groups.length).toBe(2);
      expect(groups[0]).toEqual(userGroup);
      expect(groups[1]?.hooks[0]?.command).toContain(STANDARDOC_SESSION_SYNC_MARKER);
    }
  });

  test('existing SessionStart user groups → preserve and append the reset group last', () => {
    const userGroup = {
      matcher: '',
      hooks: [{ type: 'command' as const, command: 'echo "user session-start"' }],
    };
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: { hooks: { SessionStart: [userGroup] } },
    });
    expect(action.kind).toBe('append');
    if (action.kind === 'append') {
      const groups = action.result.hooks?.SessionStart ?? [];
      expect(groups.length).toBe(2);
      expect(groups[0]).toEqual(userGroup);
      expect(groups[1]?.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_RESET_COMMAND);
    }
  });

  test('idempotent when ALL five markers already exist', () => {
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: {
        hooks: {
          UserPromptSubmit: [buildStandardocHookGroup()],
          PreToolUse: [
            buildStandardocMcpFirstMarkHookGroup(),
            buildStandardocMcpFirstCheckHookGroup(),
          ],
          PostToolUse: [buildStandardocSessionSyncHookGroup()],
          SessionStart: [buildStandardocMcpFirstResetHookGroup()],
        },
      },
    });
    expect(action).toEqual({ kind: 'no-op' });
  });

  test('idempotent even when our hooks live under different matchers', () => {
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: {
        hooks: {
          UserPromptSubmit: [
            {
              matcher: 'special-pattern',
              hooks: [{ type: 'command' as const, command: STANDARDOC_HOOK_COMMAND }],
            },
          ],
          PreToolUse: [
            {
              matcher: 'custom-matcher-1',
              hooks: [{ type: 'command' as const, command: STANDARDOC_MCP_FIRST_MARK_COMMAND }],
            },
            {
              matcher: 'custom-matcher-2',
              hooks: [{ type: 'command' as const, command: STANDARDOC_MCP_FIRST_CHECK_COMMAND }],
            },
          ],
          PostToolUse: [
            {
              matcher: 'Write',
              hooks: [{ type: 'command' as const, command: STANDARDOC_SESSION_SYNC_COMMAND }],
            },
          ],
          SessionStart: [
            {
              matcher: 'whatever',
              hooks: [{ type: 'command' as const, command: STANDARDOC_MCP_FIRST_RESET_COMMAND }],
            },
          ],
        },
      },
    });
    expect(action).toEqual({ kind: 'no-op' });
  });

  test('partial install: only nudge present → adds the four missing hooks', () => {
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: { hooks: { UserPromptSubmit: [buildStandardocHookGroup()] } },
    });
    expect(action.kind).toBe('append');
    if (action.kind === 'append') {
      expect((action.result.hooks?.UserPromptSubmit ?? []).length).toBe(1);
      expect((action.result.hooks?.PreToolUse ?? []).length).toBe(2);
      expect((action.result.hooks?.PostToolUse ?? []).length).toBe(1);
      expect((action.result.hooks?.SessionStart ?? []).length).toBe(1);
    }
  });

  test('partial install: only the mark hook present → adds the missing check + reset + nudge + sync', () => {
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: {
        hooks: { PreToolUse: [buildStandardocMcpFirstMarkHookGroup()] },
      },
    });
    expect(action.kind).toBe('append');
    if (action.kind === 'append') {
      const pre = action.result.hooks?.PreToolUse ?? [];
      expect(pre.length).toBe(2);
      // Existing mark is preserved first, then the missing check is appended.
      expect(pre[0]?.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_MARK_COMMAND);
      expect(pre[1]?.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_CHECK_COMMAND);
      expect((action.result.hooks?.SessionStart ?? []).length).toBe(1);
    }
  });

  test('partial install: only the check hook present → adds the missing mark second', () => {
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: {
        hooks: { PreToolUse: [buildStandardocMcpFirstCheckHookGroup()] },
      },
    });
    expect(action.kind).toBe('append');
    if (action.kind === 'append') {
      const pre = action.result.hooks?.PreToolUse ?? [];
      expect(pre.length).toBe(2);
      expect(pre[0]?.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_CHECK_COMMAND);
      expect(pre[1]?.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_MARK_COMMAND);
    }
  });

  test('propagates invalid from parse', () => {
    const action = mergeClaudeHook({ kind: 'invalid', error: 'bad JSON' });
    expect(action.kind).toBe('invalid');
    if (action.kind === 'invalid') {
      expect(action.error).toBe('bad JSON');
    }
  });

  test('does not mutate the input object', () => {
    const before = {
      hooks: {
        UserPromptSubmit: [] as never[],
        PreToolUse: [] as never[],
        PostToolUse: [] as never[],
        SessionStart: [] as never[],
      },
    };
    const frozen = Object.freeze(before);
    expect(() => mergeClaudeHook({ kind: 'parsed', value: frozen })).not.toThrow();
  });

  test('preserves unrelated top-level keys', () => {
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: { theme: 'dark', model: 'claude-opus-4', hooks: {} } as never,
    });
    expect(action.kind).toBe('append');
    if (action.kind === 'append') {
      expect((action.result as Record<string, unknown>).theme).toBe('dark');
      expect((action.result as Record<string, unknown>).model).toBe('claude-opus-4');
    }
  });
});

describe('serializeClaudeSettings', () => {
  test('produces pretty JSON terminated with a newline', () => {
    const out = serializeClaudeSettings({ hooks: { UserPromptSubmit: [] } });
    expect(out.endsWith('\n')).toBe(true);
    expect(out).toContain('  "hooks"');
  });
});

describe('session-sync hook contract', () => {
  test('marker is grep-stable inside the command', () => {
    expect(STANDARDOC_SESSION_SYNC_COMMAND).toContain(STANDARDOC_SESSION_SYNC_MARKER);
  });

  test('PostToolUse group matches Write/Edit/MultiEdit', () => {
    const group = buildStandardocSessionSyncHookGroup();
    expect(group.matcher).toBe('Write|Edit|MultiEdit');
  });
});

describe('mcp-first hook contracts', () => {
  test('each marker is grep-stable inside its command', () => {
    expect(STANDARDOC_MCP_FIRST_MARK_COMMAND).toContain(STANDARDOC_MCP_FIRST_MARK_MARKER);
    expect(STANDARDOC_MCP_FIRST_CHECK_COMMAND).toContain(STANDARDOC_MCP_FIRST_CHECK_MARKER);
    expect(STANDARDOC_MCP_FIRST_RESET_COMMAND).toContain(STANDARDOC_MCP_FIRST_RESET_MARKER);
  });

  test('markers do not collide across modes', () => {
    expect(STANDARDOC_MCP_FIRST_MARK_COMMAND).not.toContain(STANDARDOC_MCP_FIRST_CHECK_MARKER);
    expect(STANDARDOC_MCP_FIRST_MARK_COMMAND).not.toContain(STANDARDOC_MCP_FIRST_RESET_MARKER);
    expect(STANDARDOC_MCP_FIRST_CHECK_COMMAND).not.toContain(STANDARDOC_MCP_FIRST_MARK_MARKER);
    expect(STANDARDOC_MCP_FIRST_CHECK_COMMAND).not.toContain(STANDARDOC_MCP_FIRST_RESET_MARKER);
    expect(STANDARDOC_MCP_FIRST_RESET_COMMAND).not.toContain(STANDARDOC_MCP_FIRST_MARK_MARKER);
    expect(STANDARDOC_MCP_FIRST_RESET_COMMAND).not.toContain(STANDARDOC_MCP_FIRST_CHECK_MARKER);
  });

  test('mark group matches every standardoc MCP tool', () => {
    const group = buildStandardocMcpFirstMarkHookGroup();
    expect(group.matcher).toBe('mcp__standardoc__.*');
    expect(group.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_MARK_COMMAND);
  });

  test('check group matches code-exploration tools', () => {
    const group = buildStandardocMcpFirstCheckHookGroup();
    expect(group.matcher).toBe('Bash|Read|Grep|Glob');
    expect(group.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_CHECK_COMMAND);
  });

  test('reset group fires on every SessionStart (empty matcher)', () => {
    const group = buildStandardocMcpFirstResetHookGroup();
    expect(group.matcher).toBe('');
    expect(group.hooks[0]?.command).toBe(STANDARDOC_MCP_FIRST_RESET_COMMAND);
  });
});
