/**
 * Minimal markdown → HTML renderer for doc comments. Covers the
 * subset that shows up in real Rust/TS doc strings:
 *   - `# / ## / ###` headings
 *   - **bold**, *italic*, `inline code`
 *   - ```fenced code blocks```
 *   - bullet + numbered lists
 *   - [text](url) links (opens in new tab)
 *   - paragraphs separated by blank lines, single newlines as <br>
 *
 * All non-code text is HTML-escaped before substitution so a stray
 * `<script>` in a doc comment can't inject. Code blocks / inline
 * code escape their bodies too. The output is meant to be assigned
 * via `innerHTML` — the styling lives in `.details__markdown` SCSS.
 */
export function renderMarkdown(md: string): string {
  const escape = (s: string): string =>
    s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  const codeBlocks: string[] = [];
  const inlineCodes: string[] = [];
  let text = md.replace(/```([\w-]*)\n?([\s\S]*?)```/g, (_, lang: string, body: string) => {
    const cls = lang ? ` class="lang-${escape(lang)}"` : '';
    codeBlocks.push(`<pre><code${cls}>${escape(body.replace(/\n$/, ''))}</code></pre>`);
    return `CB${codeBlocks.length - 1}`;
  });
  text = text.replace(/`([^`\n]+)`/g, (_, body: string) => {
    inlineCodes.push(`<code>${escape(body)}</code>`);
    return `IC${inlineCodes.length - 1}`;
  });
  text = escape(text);
  text = text.replace(/^### (.+)$/gm, '<h3>$1</h3>');
  text = text.replace(/^## (.+)$/gm, '<h2>$1</h2>');
  text = text.replace(/^# (.+)$/gm, '<h1>$1</h1>');
  text = text.replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>');
  text = text.replace(/(^|[^\w*])\*([^*\n]+)\*(?!\w)/g, '$1<em>$2</em>');
  text = text.replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, (_, t: string, u: string) =>
    `<a href="${u}" target="_blank" rel="noopener noreferrer">${t}</a>`);
  text = text.replace(/^(\s*)[-*] (.+)$/gm, '$1<li>$2</li>');
  text = text.replace(/^(\s*)\d+\. (.+)$/gm, '$1<li data-ord="1">$2</li>');
  text = text.replace(/((?:^<li[^>]*>.*<\/li>\n?)+)/gm, m =>
    m.includes('data-ord') ? `<ol>${m}</ol>` : `<ul>${m}</ul>`);
  text = text.split(/\n{2,}/).map(p => {
    const trimmed = p.trim();
    if (!trimmed) return '';
    if (/^<(h[1-6]|ul|ol|pre|CB)/.test(trimmed)) return trimmed;
    return `<p>${trimmed.replace(/\n/g, '<br/>')}</p>`;
  }).join('\n');
  text = text.replace(/CB(\d+)/g, (_, i: string) => codeBlocks[Number(i)] ?? '');
  text = text.replace(/IC(\d+)/g, (_, i: string) => inlineCodes[Number(i)] ?? '');
  return text;
}
