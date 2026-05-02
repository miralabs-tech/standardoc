export type WorkspaceInitState = 'opted-in' | 'opted-out' | undefined;
export type GlobalInitState = 'never' | undefined;

export interface PromptDecisionInput {
  readonly hasStandardocDir: boolean;
  readonly hasCodeMarker: boolean;
  readonly workspaceState: WorkspaceInitState;
  readonly globalState: GlobalInitState;
}

export type PromptDecision =
  | { kind: 'spawn-immediately' }
  | { kind: 'show-prompt' }
  | { kind: 'do-nothing' };

export function decidePromptOnActivate(input: PromptDecisionInput): PromptDecision {
  if (input.hasStandardocDir) return { kind: 'spawn-immediately' };
  if (input.workspaceState === 'opted-in') return { kind: 'spawn-immediately' };

  if (!input.hasCodeMarker) return { kind: 'do-nothing' };
  if (input.workspaceState === 'opted-out') return { kind: 'do-nothing' };
  if (input.globalState === 'never') return { kind: 'do-nothing' };

  return { kind: 'show-prompt' };
}
