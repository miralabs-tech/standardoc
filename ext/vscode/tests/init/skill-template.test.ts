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
      'mcp__standardoc__find_symbol',
      'mcp__standardoc__get_context',
      'mcp__standardoc__list_symbols',
      'mcp__standardoc__find_symbols_by_pattern',
      'mcp__standardoc__find_similar_symbols',
      'mcp__standardoc__get_body',
      'mcp__standardoc__resolve_external',
      'mcp__standardoc__current_revision',
      'mcp__standardoc__check_stale',
      'mcp__standardoc__usage_stats',
      'mcp__standardoc__session_save',
      'mcp__standardoc__session_list',
      'mcp__standardoc__session_get',
    ]) {
      expect(c).toContain(tool);
    }
  });

  test('documents every shipped tool with a section header', () => {
    const c = buildSkillContent();
    expect(c).toContain('### find_symbol(query, limit?)');
    expect(c).toContain('### get_context(fqdn, depth: 1|2)');
    expect(c).toContain('### list_symbols(');
    expect(c).toContain('### find_symbols_by_pattern(');
    expect(c).toContain('### find_similar_symbols(');
    expect(c).toContain('### get_body(');
    expect(c).toContain('### resolve_external(');
    expect(c).toContain('### current_revision()');
    expect(c).toContain('### check_stale(');
    expect(c).toContain('### usage_stats(');
    expect(c).toContain('### session_save(');
    expect(c).toContain('### session_list(');
    expect(c).toContain('### session_get(');
  });

  test('explains depth=1 cheap vs depth=2 rich semantics', () => {
    const c = buildSkillContent();
    expect(c).toContain('depth=1 (cheap)');
    expect(c).toContain('depth=2 (rich)');
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
    // Pattern tool prose mentions glob shape.
    expect(c).toContain('SQLite');
    expect(c).toContain('GLOB');
    // Similarity tool prose mentions anchor + score.
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

  test('lists every recommended workflow including duplicate-detection', () => {
    const c = buildSkillContent();
    expect(c).toContain('What does X do / where is X used');
    expect(c).toContain('I need to modify behavior Y');
    expect(c).toContain('Is symbol X used anywhere');
    expect(c).toContain("I'm starting a feature involving area Z");
    expect(c).toContain('Detect templated/duplicate names across modules');
    expect(c).toContain('A neighbor is Unresolved and looks like an external dependency');
    expect(c).toContain('Detect what changed since last fetch');
    expect(c).toContain('Resume / save a session handoff');
  });

  test('documents resolve_external envelope statuses', () => {
    const c = buildSkillContent();
    expect(c).toContain('"resolved"');
    expect(c).toContain('"not_found"');
    expect(c).toContain('"missing_binary"');
    expect(c).toContain('"lockfile_not_found"');
  });

  test('documents check_stale stateless contract + status set', () => {
    const c = buildSkillContent();
    expect(c).toContain('Stateless server-side');
    expect(c).toContain('"stale"');
    expect(c).toContain('"fresh"');
    expect(c).toContain('"missing"');
  });

  test('documents session_save UPSERT semantic + supersedes chain', () => {
    const c = buildSkillContent();
    expect(c).toContain('UPSERT by `slug`');
    expect(c).toContain('supersedes');
  });

  test('documents key concepts (FQDN, edge kinds, Resolved/Unresolved)', () => {
    const c = buildSkillContent();
    expect(c).toContain('## Key concepts');
    expect(c).toContain('FQDN');
    expect(c).toContain('Edge kinds');
    expect(c).toContain('Resolved vs Unresolved');
    expect(c).toContain('UnresolvedBridge');
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

  test('stays well under the 500-line skill recommendation', () => {
    const lines = buildSkillContent().split('\n').length;
    expect(lines).toBeLessThan(500);
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
