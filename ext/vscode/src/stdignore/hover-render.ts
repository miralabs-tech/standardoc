/**
 * vscode-free renderer for the `.stdignore` hover preview. Lives in
 * its own file so unit tests can import it without pulling in the
 * `vscode` runtime — the wrapper in `hover.ts` adapts the output to
 * `vscode.MarkdownString`.
 */

export interface PatternPreview {
  readonly pattern: string;
  readonly matches: ReadonlyArray<string>;
  readonly total_count: number;
  readonly truncated: boolean;
  readonly walk_truncated: boolean;
}

export function renderHoverMarkdown(preview: PatternPreview): string {
  if (preview.total_count === 0) {
    return `**Pattern \`${escapeMd(preview.pattern)}\`** — no matches in workspace.`;
  }
  const shownLabel = preview.truncated
    ? `showing ${preview.matches.length} of ${preview.total_count}`
    : `${preview.total_count} match${preview.total_count === 1 ? '' : 'es'}`;
  let out = `**Pattern \`${escapeMd(preview.pattern)}\`** — ${shownLabel}\n\n`;
  for (const m of preview.matches) {
    out += `- \`${escapeMd(m)}\`\n`;
  }
  if (preview.walk_truncated) {
    out += '\n_Walk capped — actual matches may be higher._';
  }
  return out;
}

function escapeMd(s: string): string {
  return s.replace(/[`\\*_]/g, ch => `\\${ch}`);
}
