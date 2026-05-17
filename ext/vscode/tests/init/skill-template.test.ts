import { describe, expect, test } from 'bun:test';
import {
  SKILL_RELATIVE_DIR,
  SKILL_RELATIVE_PATH,
  buildSkillContent,
  skillContentMatches,
} from '../../src/init/skill-template';

describe('SKILL paths', () => {
  test('relative dir is .claude/skills/standardoc', () => {
    expect(SKILL_RELATIVE_DIR).toBe('.claude/skills/standardoc');
  });

  test('relative path is .claude/skills/standardoc/SKILL.md', () => {
    expect(SKILL_RELATIVE_PATH).toBe('.claude/skills/standardoc/SKILL.md');
  });
});

describe('buildSkillContent', () => {
  test('returns identical content on every call (deterministic)', () => {
    expect(buildSkillContent()).toBe(buildSkillContent());
  });

  test('starts with YAML frontmatter delimiter', () => {
    expect(buildSkillContent().startsWith('---\n')).toBe(true);
  });

  test('declares name: standardoc in frontmatter', () => {
    expect(buildSkillContent()).toContain('name: standardoc');
  });

  test('declares description matching MCP-first reflex framing', () => {
    const c = buildSkillContent();
    expect(c).toContain('description:');
    expect(c).toContain('FIRST reflex');
    expect(c).toContain('source of truth for code structure');
  });

  test('declares when_to_use with fallback hierarchy', () => {
    const c = buildSkillContent();
    expect(c).toContain('when_to_use:');
    expect(c).toContain('ALWAYS use Standardoc FIRST');
    expect(c).toContain('BEFORE Read/Grep/Glob');
  });

  test('declares allowed-tools pre-approving every shipped MCP tool', () => {
    const c = buildSkillContent();
    expect(c).toContain('allowed-tools:');
    for (const tool of [
      // Discovery
      'mcp__standardoc__find_symbol',
      'mcp__standardoc__find_symbols_by_pattern',
      'mcp__standardoc__find_similar_symbols',
      'mcp__standardoc__list_symbols',
      'mcp__standardoc__find_call_sites',
      // Reasoning
      'mcp__standardoc__get_context',
      'mcp__standardoc__get_body',
      'mcp__standardoc__fetch_chunks',
      // External + cross-workspace
      'mcp__standardoc__resolve_external',
      'mcp__standardoc__module_lookup',
      'mcp__standardoc__resolve_cross_workspace',
      'mcp__standardoc__link_workspace',
      'mcp__standardoc__unlink_workspace',
      'mcp__standardoc__list_linked_workspaces',
      'mcp__standardoc__set_link_direction',
      'mcp__standardoc__refresh_peer',
      // Projects
      'mcp__standardoc__list_projects',
      'mcp__standardoc__project_for_file',
      // Boot-time + telemetry
      'mcp__standardoc__current_revision',
      'mcp__standardoc__check_stale',
      'mcp__standardoc__usage_stats',
      // Sessions
      'mcp__standardoc__session_save',
      'mcp__standardoc__session_list',
      'mcp__standardoc__session_get',
      'mcp__standardoc__session_sync_in',
      'mcp__standardoc__session_sync_out',
    ]) {
      expect(c).toContain(tool);
    }
  });

  test('documents every shipped tool with a section header', () => {
    const c = buildSkillContent();
    // Discovery
    expect(c).toContain('### find_symbol');
    expect(c).toContain('### find_symbols_by_pattern');
    expect(c).toContain('### find_similar_symbols');
    expect(c).toContain('### list_symbols');
    expect(c).toContain('### find_call_sites');
    // Reasoning
    expect(c).toContain('### get_context');
    expect(c).toContain('### get_body');
    expect(c).toContain('### fetch_chunks');
    // External + cross-workspace
    expect(c).toContain('### resolve_external');
    expect(c).toContain('### module_lookup');
    expect(c).toContain('### resolve_cross_workspace');
    expect(c).toContain('### link_workspace');
    expect(c).toContain('### unlink_workspace');
    expect(c).toContain('### list_linked_workspaces');
    expect(c).toContain('### set_link_direction');
    expect(c).toContain('### refresh_peer');
    // Projects
    expect(c).toContain('### list_projects');
    expect(c).toContain('### project_for_file');
    // Boot-time + telemetry
    expect(c).toContain('### current_revision');
    expect(c).toContain('### check_stale');
    expect(c).toContain('### usage_stats');
    // Sessions
    expect(c).toContain('### session_save');
    expect(c).toContain('### session_list');
    expect(c).toContain('### session_get');
    expect(c).toContain('### session_sync_in');
    expect(c).toContain('### session_sync_out');
  });

  test('documents workspace_id default-primary scope on the 3 discovery tools (L3e)', () => {
    const c = buildSkillContent();
    expect(c).toContain('Workspace scoping convention');
    expect(c).toContain('defaults to the **primary** workspace');
    // The 3 tools whose signatures expose the param.
    expect(c).toContain(
      'find_symbol({ query, limit?, kind?, visibility?, module?, include_external?, workspace_id? })'
    );
    expect(c).toContain(
      'find_symbols_by_pattern({ pattern, kind?, visibility?, module?, limit?, include_external?, workspace_id? })'
    );
    expect(c).toContain(
      'list_symbols({ kind?, visibility?, module?, limit?, include_external?, cursor?, workspace_id? })'
    );
  });

  test('documents indexing_mode? on link_workspace (Stage 3b-7-b L3c)', () => {
    const c = buildSkillContent();
    expect(c).toContain('link_workspace({ path, direction, indexing_mode? })');
    expect(c).toContain('`blob_import`');
    expect(c).toContain('`extract`');
  });

  test('documents set_link_direction with Out<->{In,Bidirectional} watcher transition (post-3b-7-b)', () => {
    const c = buildSkillContent();
    expect(c).toContain('set_link_direction({ workspace_id, direction })');
    expect(c).toContain('previous_direction');
    expect(c).toContain('new_direction');
    expect(c).toContain('`Out → {in, bidirectional}`');
    expect(c).toContain('`{in, bidirectional} → out`');
  });

  test('documents refresh_peer Q4 staleness escape hatch (L3-bis)', () => {
    const c = buildSkillContent();
    expect(c).toContain('refresh_peer({ workspace_id })');
    expect(c).toContain('files_extracted');
    expect(c).toContain('files_skipped_unchanged');
    expect(c).toContain('files_parse_errors');
    expect(c).toContain('`skipped_inactive`');
    expect(c).toContain('`skipped_missing`');
  });

  test('explains depth=1 cheap vs depth=2 rich semantics', () => {
    const c = buildSkillContent();
    expect(c).toContain('cheap, exploration');
    expect(c).toContain('rich, reasoning');
  });

  test('documents similarity tool default threshold and hybrid scoring', () => {
    const c = buildSkillContent();
    expect(c).toContain('threshold');
    expect(c).toContain('0.8');
    expect(c).toContain('Jaro-Winkler');
    expect(c).toContain('Jaccard');
  });

  test('contrasts pattern (deterministic glob) vs similarity (scored anchor)', () => {
    const c = buildSkillContent();
    expect(c).toContain('SQLite');
    expect(c).toContain('GLOB');
    expect(c).toContain('anchor');
    expect(c).toContain('score');
  });

  test('lists the 3-tier tool fallback hierarchy in body', () => {
    const c = buildSkillContent();
    expect(c).toContain('## Tool fallback hierarchy');
    expect(c).toContain('Standardoc MCP');
    expect(c).toContain('LSP / IDE Go-to-Definition');
    expect(c).toContain('Raw Read / Grep / Glob');
  });

  test('lists every recommended workflow including peer-related ones', () => {
    const c = buildSkillContent();
    expect(c).toContain('What does X do / where is X used');
    expect(c).toContain('I need to modify behavior Y');
    expect(c).toContain('Is symbol X used anywhere');
    expect(c).toContain("I'm starting a feature involving area Z");
    expect(c).toContain('Detect templated/duplicate names across modules');
    expect(c).toContain('A neighbor is Unresolved and looks like an external dependency');
    expect(c).toContain('A neighbor looks like a symbol from a linked peer workspace');
    expect(c).toContain('User edited a linked peer outside the daemon');
    expect(c).toContain("User wants to change a peer's direction");
    expect(c).toContain('Detect what changed since last fetch');
    expect(c).toContain('Resume / save a session handoff');
  });

  test('documents resolve_external envelope statuses', () => {
    const c = buildSkillContent();
    expect(c).toContain('`resolved`');
    expect(c).toContain('`not_found`');
    expect(c).toContain('`missing_binary`');
    expect(c).toContain('`lockfile_not_found`');
  });

  test('documents check_stale stateless contract + status set', () => {
    const c = buildSkillContent();
    expect(c).toContain('Stateless server-side');
    expect(c).toContain('`stale`');
    expect(c).toContain('`fresh`');
    expect(c).toContain('`missing`');
  });

  test('documents session_save UPSERT semantic + supersedes chain', () => {
    const c = buildSkillContent();
    expect(c).toContain('UPSERT by `slug`');
    expect(c).toContain('supersedes');
  });

  test('documents key concepts (FQDN, edge kinds, Resolved/Unresolved, workspace scope)', () => {
    const c = buildSkillContent();
    expect(c).toContain('## Key concepts');
    expect(c).toContain('FQDN');
    expect(c).toContain('Edge kinds');
    expect(c).toContain('Resolved vs Unresolved');
    expect(c).toContain('UnresolvedBridge');
    expect(c).toContain('Workspace scope');
  });

  test('mentions cold start indexing-in-progress fallback message', () => {
    const c = buildSkillContent();
    expect(c).toContain('Workspace indexing in progress');
  });

  test('ends with regenerate footer pointing at the cmd palette command', () => {
    const c = buildSkillContent();
    expect(c).toContain('Regenerate AI agent');
    expect(c).toContain('skill` command from the VSCode command palette');
  });

  test('stays under the soft 1000-line cap', () => {
    // Bumped from the original 500-line guideline: the cross-workspace
    // surface (link/unlink/list/set_link_direction/refresh_peer +
    // workspace_id scoping + find_call_sites + projects + session
    // sync_in/out) outgrew the original budget. 1000 keeps a real ceiling
    // while letting the comprehensive docs land.
    const lines = buildSkillContent().split('\n').length;
    expect(lines).toBeLessThan(1000);
  });
});

describe('skillContentMatches', () => {
  test('matches identical strings', () => {
    const c = buildSkillContent();
    expect(skillContentMatches(c, c)).toBe(true);
  });

  test('matches across CRLF / LF line ending differences', () => {
    const lf = 'a\nb\nc';
    const crlf = 'a\r\nb\r\nc';
    expect(skillContentMatches(lf, crlf)).toBe(true);
  });

  test('matches when actual has trailing whitespace differences', () => {
    const a = 'hello\nworld\n\n\n';
    const b = 'hello\nworld';
    expect(skillContentMatches(a, b)).toBe(true);
  });

  test('does not match when content differs', () => {
    expect(skillContentMatches('foo', 'bar')).toBe(false);
  });

  test('does not match when expected has been edited mid-body', () => {
    const expected = buildSkillContent();
    const tampered = expected.replace('FIRST reflex', 'sometimes maybe');
    expect(skillContentMatches(tampered, expected)).toBe(false);
  });
});
