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

  test('re-prompt when opted-in but .standardoc/ has been deleted (user reset signal)', () => {
    const r = decidePromptOnActivate(
      baseline({ workspaceState: 'opted-in', hasCodeMarker: true }),
    );
    expect(r).toEqual({ kind: 'show-prompt' });
  });

  test('do-nothing when opted-in but no code marker (avoid prompting on empty dirs)', () => {
    const r = decidePromptOnActivate(baseline({ workspaceState: 'opted-in' }));
    expect(r).toEqual({ kind: 'do-nothing' });
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

  test('opt-outs win over stale opted-in flag when .standardoc/ is absent', () => {
    // Without `.standardoc/` on disk, the `opted-in` flag no longer
    // bypasses anything — the global `never` (or workspace `opted-out`)
    // takes effect and we stay silent.
    const r = decidePromptOnActivate(
      baseline({
        hasCodeMarker: true,
        workspaceState: 'opted-in',
        globalState: 'never',
      }),
    );
    expect(r).toEqual({ kind: 'do-nothing' });
  });
});
