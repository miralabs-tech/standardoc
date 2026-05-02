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

  test('declares allowed-tools pre-approving the two MCP tools', () => {
    const c = buildSkillContent();
    expect(c).toContain('allowed-tools: mcp__standardoc__find_symbol mcp__standardoc__get_context');
  });

  test('documents both tools (find_symbol and get_context)', () => {
    const c = buildSkillContent();
    expect(c).toContain('### find_symbol(query, limit?)');
    expect(c).toContain('### get_context(fqdn, depth: 1|2)');
  });

  test('explains depth=1 cheap vs depth=2 rich semantics', () => {
    const c = buildSkillContent();
    expect(c).toContain('depth=1 (cheap)');
    expect(c).toContain('depth=2 (rich)');
  });

  test('lists the 3-tier tool fallback hierarchy in body', () => {
    const c = buildSkillContent();
    expect(c).toContain('## Tool fallback hierarchy');
    expect(c).toContain('Standardoc MCP');
    expect(c).toContain('LSP / IDE Go-to-Definition');
    expect(c).toContain('Raw Read / Grep / Glob');
  });

  test('lists the four recommended workflows', () => {
    const c = buildSkillContent();
    expect(c).toContain('What does X do / where is X used');
    expect(c).toContain('I need to modify behavior Y');
    expect(c).toContain('Is symbol X used anywhere');
    expect(c).toContain("I'm starting a feature involving area Z");
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
