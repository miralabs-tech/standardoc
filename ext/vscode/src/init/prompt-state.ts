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
  // Presence of `.standardoc/` is the single source of truth for
  // "this workspace is already initialised". A prior `workspaceState =
  // 'opted-in'` flag does NOT bypass the prompt — deleting `.standardoc/`
  // is a deliberate reset signal from the user, and we must re-ask
  // instead of silently re-spawning the daemon (which would recreate
  // `.standardoc/` and `.stdignore` cold-start).
  if (input.hasStandardocDir) return { kind: 'spawn-immediately' };

  if (!input.hasCodeMarker) return { kind: 'do-nothing' };
  if (input.workspaceState === 'opted-out') return { kind: 'do-nothing' };
  if (input.globalState === 'never') return { kind: 'do-nothing' };

  return { kind: 'show-prompt' };
}
