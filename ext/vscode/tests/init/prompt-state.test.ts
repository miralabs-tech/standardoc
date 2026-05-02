import { describe, expect, test } from 'bun:test';
import {
  decidePromptOnActivate,
  type PromptDecisionInput,
} from '../../src/init/prompt-state';

const baseline = (overrides: Partial<PromptDecisionInput> = {}): PromptDecisionInput => ({
  hasStandardocDir: false,
  hasCodeMarker: false,
  workspaceState: undefined,
  globalState: undefined,
  ...overrides,
});

describe('decidePromptOnActivate', () => {
  test('spawn-immediately when .standardoc/ already exists (overrides everything)', () => {
    const r = decidePromptOnActivate(
      baseline({
        hasStandardocDir: true,
        workspaceState: 'opted-out',
        globalState: 'never',
      }),
    );
    expect(r).toEqual({ kind: 'spawn-immediately' });
  });

  test('spawn-immediately when opted-in even without .standardoc/ yet', () => {
    const r = decidePromptOnActivate(baseline({ workspaceState: 'opted-in' }));
    expect(r).toEqual({ kind: 'spawn-immediately' });
  });

  test('do-nothing when no code marker present', () => {
    const r = decidePromptOnActivate(baseline());
    expect(r).toEqual({ kind: 'do-nothing' });
  });

  test('do-nothing when workspace opted-out', () => {
    const r = decidePromptOnActivate(
      baseline({ hasCodeMarker: true, workspaceState: 'opted-out' }),
    );
    expect(r).toEqual({ kind: 'do-nothing' });
  });

  test('do-nothing when globally never', () => {
    const r = decidePromptOnActivate(
      baseline({ hasCodeMarker: true, globalState: 'never' }),
    );
    expect(r).toEqual({ kind: 'do-nothing' });
  });

  test('show-prompt when code present and nothing decided', () => {
    const r = decidePromptOnActivate(baseline({ hasCodeMarker: true }));
    expect(r).toEqual({ kind: 'show-prompt' });
  });

  test('global never wins over local opted-in only because we already returned spawn earlier', () => {
    // If opted-in AND globalState=never, opted-in wins (user explicitly chose this workspace).
    const r = decidePromptOnActivate(
      baseline({
        hasCodeMarker: true,
        workspaceState: 'opted-in',
        globalState: 'never',
      }),
    );
    expect(r).toEqual({ kind: 'spawn-immediately' });
  });
});
