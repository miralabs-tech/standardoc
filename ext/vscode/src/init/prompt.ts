import * as vscode from 'vscode';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { resolveBinary } from '../daemon/binary';
import {
  buildStandardocEntry,
  mergeMcpConfig,
  parseMcpConfig,
  serializeMcpConfig,
} from './mcp-config';
import {
  decidePromptOnActivate,
  type GlobalInitState,
  type WorkspaceInitState,
} from './prompt-state';
import {
  SKILL_RELATIVE_DIR,
  SKILL_RELATIVE_PATH,
  buildSkillContent,
  skillContentMatches,
} from './skill-template';

const WORKSPACE_STATE_KEY = 'standardoc.initState';
const GLOBAL_STATE_KEY = 'standardoc.initStateGlobal';
const CODE_MARKERS = ['Cargo.toml', 'package.json', 'pyproject.toml'];
const STANDARDOC_DIR = '.standardoc';
const MCP_CONFIG_FILE = '.mcp.json';

export interface InitDeps {
  readonly context: vscode.ExtensionContext;
  readonly workspaceRoot: string;
  readonly output: vscode.OutputChannel;
  readonly onOptedIn: () => void;
}

export function getWorkspaceInitState(ctx: vscode.ExtensionContext): WorkspaceInitState {
  return ctx.workspaceState.get<WorkspaceInitState>(WORKSPACE_STATE_KEY);
}

export function getGlobalInitState(ctx: vscode.ExtensionContext): GlobalInitState {
  return ctx.globalState.get<GlobalInitState>(GLOBAL_STATE_KEY);
}

export async function clearGlobalInitState(ctx: vscode.ExtensionContext): Promise<void> {
  await ctx.globalState.update(GLOBAL_STATE_KEY, undefined);
}

function workspaceHasStandardocDir(workspaceRoot: string): boolean {
  return fs.existsSync(path.join(workspaceRoot, STANDARDOC_DIR));
}

function workspaceHasCodeMarker(workspaceRoot: string): boolean {
  return CODE_MARKERS.some(marker => fs.existsSync(path.join(workspaceRoot, marker)));
}

export async function maybePromptForInit(deps: InitDeps): Promise<void> {
  const decision = decidePromptOnActivate({
    hasStandardocDir: workspaceHasStandardocDir(deps.workspaceRoot),
    hasCodeMarker: workspaceHasCodeMarker(deps.workspaceRoot),
    workspaceState: getWorkspaceInitState(deps.context),
    globalState: getGlobalInitState(deps.context),
  });

  if (decision.kind === 'spawn-immediately') {
    deps.onOptedIn();
    return;
  }
  if (decision.kind === 'do-nothing') return;

  await showInitNotification(deps);
}

async function showInitNotification(deps: InitDeps): Promise<void> {
  const choice = await vscode.window.showInformationMessage(
    'Standardoc: Initialize this workspace? (DB index + register MCP for Claude Code CLI)',
    'Initialize',
    'Skip',
    'Never for this workspace',
    'Never (any workspace)',
  );

  switch (choice) {
    case 'Initialize':
      await initializeWorkspace(deps);
      break;
    case 'Never for this workspace':
      await deps.context.workspaceState.update(WORKSPACE_STATE_KEY, 'opted-out');
      deps.output.appendLine('[init] opted out for this workspace');
      break;
    case 'Never (any workspace)':
      await deps.context.globalState.update(GLOBAL_STATE_KEY, 'never');
      deps.output.appendLine('[init] opted out globally (any workspace)');
      break;
    default:
      break;
  }
}

export async function initializeWorkspace(deps: InitDeps): Promise<void> {
  await deps.context.workspaceState.update(WORKSPACE_STATE_KEY, 'opted-in');
  await writeMcpConfig(deps);
  await writeSkillFile(deps, { mode: 'init' });
  deps.onOptedIn();
}

export async function writeMcpConfig(deps: InitDeps): Promise<void> {
  let binaryPath: string;
  try {
    const resolved = await resolveBinary(deps.context);
    binaryPath = resolved.path;
  } catch (e) {
    deps.output.appendLine(
      `[init] cannot resolve binary for .mcp.json: ${describeError(e)}`,
    );
    return;
  }

  const expected = buildStandardocEntry({
    binaryPath,
    workspaceRoot: deps.workspaceRoot,
  });
  const target = path.join(deps.workspaceRoot, MCP_CONFIG_FILE);
  const raw = readFileOrNull(target);
  const parsed = parseMcpConfig(raw);
  const action = mergeMcpConfig(parsed, expected);

  switch (action.kind) {
    case 'no-op':
      deps.output.appendLine('[init] .mcp.json already configured for Standardoc');
      return;
    case 'invalid': {
      const snippet = serializeMcpConfig({ mcpServers: { standardoc: expected } });
      void vscode.window.showWarningMessage(
        `Standardoc: .mcp.json could not be parsed (${action.error}). ` +
          `Please add this entry manually:\n\n${snippet}`,
        { modal: true },
      );
      deps.output.appendLine(`[init] .mcp.json invalid: ${action.error}`);
      return;
    }
    case 'create':
    case 'add-first':
    case 'overwrite-stale': {
      try {
        fs.writeFileSync(target, serializeMcpConfig(action.result), 'utf8');
      } catch (e) {
        deps.output.appendLine(`[init] failed to write .mcp.json: ${describeError(e)}`);
        void vscode.window.showErrorMessage(
          `Standardoc: could not write .mcp.json (${describeError(e)})`,
        );
        return;
      }
      const verb =
        action.kind === 'create'
          ? 'wrote'
          : action.kind === 'add-first'
            ? 'added Standardoc to'
            : 'updated Standardoc entry in';
      deps.output.appendLine(`[init] ${verb} .mcp.json`);
      void vscode.window.showInformationMessage(
        `Standardoc: ${verb} .mcp.json with absolute paths. ` +
          `Consider adding it to .gitignore if collaborating.`,
      );
      return;
    }
  }
}

export interface WriteSkillOptions {
  readonly mode: 'init' | 'force';
}

export async function writeSkillFile(
  deps: InitDeps,
  options: WriteSkillOptions,
): Promise<void> {
  const dir = path.join(deps.workspaceRoot, SKILL_RELATIVE_DIR);
  const target = path.join(deps.workspaceRoot, SKILL_RELATIVE_PATH);
  const expected = buildSkillContent();
  const existing = readFileOrNull(target);

  if (existing !== null && skillContentMatches(existing, expected)) {
    deps.output.appendLine('[init] AI agent skill already up to date');
    return;
  }

  if (existing !== null && options.mode === 'init') {
    deps.output.appendLine(
      `[init] AI agent skill already present at ${SKILL_RELATIVE_PATH} — leaving untouched ` +
        `(run "Standardoc: Regenerate AI agent skill" to refresh)`,
    );
    return;
  }

  try {
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(target, expected, 'utf8');
  } catch (e) {
    deps.output.appendLine(`[init] failed to write AI agent skill: ${describeError(e)}`);
    void vscode.window.showErrorMessage(
      `Standardoc: could not write ${SKILL_RELATIVE_PATH} (${describeError(e)})`,
    );
    return;
  }

  const verb = existing === null ? 'wrote' : 'regenerated';
  deps.output.appendLine(`[init] ${verb} AI agent skill at ${SKILL_RELATIVE_PATH}`);
  void vscode.window.showInformationMessage(
    `Standardoc: ${verb} AI agent skill at ${SKILL_RELATIVE_PATH}.`,
  );
}

export async function regenerateSkill(deps: InitDeps): Promise<void> {
  await writeSkillFile(deps, { mode: 'force' });
}

function readFileOrNull(p: string): string | null {
  try {
    return fs.readFileSync(p, 'utf8');
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code === 'ENOENT') return null;
    throw e;
  }
}

function describeError(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
