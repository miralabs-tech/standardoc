// The skill body is maintained as a single shared asset
// (crates/standardoc-cli/assets/skill.md) so the Rust `standardoc init`
// command and this extension emit byte-identical SKILL.md content.
import skillMd from '../../../../crates/standardoc-cli/assets/skill.md' with { type: 'text' };

export const SKILL_RELATIVE_DIR = '.claude/skills/standardoc';
export const SKILL_RELATIVE_PATH = '.claude/skills/standardoc/SKILL.md';

const SKILL_CONTENT = skillMd;

export function buildSkillContent(): string {
  return SKILL_CONTENT;
}

export function skillContentMatches(actual: string, expected: string): boolean {
  return normalize(actual) === normalize(expected);
}

function normalize(s: string): string {
  return s.replace(/\r\n/g, '\n').trimEnd();
}
