import { describe, expect, test } from 'bun:test';
import {
  STANDARDOC_HOOK_COMMAND,
  STANDARDOC_HOOK_MARKER,
  buildStandardocHookGroup,
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
  test('absent file → create with our hook', () => {
    const action = mergeClaudeHook({ kind: 'absent' });
    expect(action.kind).toBe('create');
    if (action.kind === 'create') {
      const group = action.result.hooks?.UserPromptSubmit?.[0];
      expect(group?.hooks[0]?.command).toContain(STANDARDOC_HOOK_MARKER);
    }
  });

  test('existing settings without UserPromptSubmit → append our group', () => {
    const result = mergeClaudeHook({ kind: 'parsed', value: { hooks: {} } });
    expect(result.kind).toBe('append');
    if (result.kind === 'append') {
      const groups = result.result.hooks?.UserPromptSubmit ?? [];
      expect(groups.length).toBe(1);
      expect(groups[0]?.hooks[0]?.command).toBe(STANDARDOC_HOOK_COMMAND);
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
      // User group preserved first.
      expect(groups[0]).toEqual(userGroup);
      // Our group last.
      expect(groups[1]?.hooks[0]?.command).toContain(STANDARDOC_HOOK_MARKER);
    }
  });

  test('idempotent when our marker already exists', () => {
    const ourGroup = buildStandardocHookGroup();
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: { hooks: { UserPromptSubmit: [ourGroup] } },
    });
    expect(action).toEqual({ kind: 'no-op' });
  });

  test('idempotent even when our hook lives under a different matcher', () => {
    const customMatcherGroup = {
      matcher: 'special-pattern',
      hooks: [{ type: 'command' as const, command: STANDARDOC_HOOK_COMMAND }],
    };
    const action = mergeClaudeHook({
      kind: 'parsed',
      value: { hooks: { UserPromptSubmit: [customMatcherGroup] } },
    });
    expect(action).toEqual({ kind: 'no-op' });
  });

  test('propagates invalid from parse', () => {
    const action = mergeClaudeHook({ kind: 'invalid', error: 'bad JSON' });
    expect(action.kind).toBe('invalid');
    if (action.kind === 'invalid') {
      expect(action.error).toBe('bad JSON');
    }
  });

  test('does not mutate the input object', () => {
    const before = { hooks: { UserPromptSubmit: [] as never[] } };
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
