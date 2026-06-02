import { escapeHtml } from '../../text';

type HighlightLang = 'rust' | 'ts' | 'c' | 'lua' | null;

const RUST_KEYWORDS = new Set([
  'fn', 'struct', 'enum', 'trait', 'impl', 'let', 'mut', 'const', 'static',
  'pub', 'use', 'mod', 'return', 'if', 'else', 'for', 'while', 'loop', 'match',
  'where', 'async', 'await', 'move', 'self', 'Self', 'true', 'false', 'as',
  'in', 'ref', 'unsafe', 'extern', 'type', 'dyn', 'crate', 'super', 'break',
  'continue', 'box', 'macro_rules',
]);

const TS_KEYWORDS = new Set([
  'function', 'class', 'interface', 'type', 'enum', 'const', 'let', 'var',
  'if', 'else', 'for', 'while', 'do', 'switch', 'case', 'default', 'break',
  'continue', 'return', 'throw', 'try', 'catch', 'finally', 'new', 'this',
  'super', 'extends', 'implements', 'export', 'import', 'from', 'as', 'in',
  'of', 'async', 'await', 'yield', 'true', 'false', 'null', 'undefined',
  'void', 'never', 'any', 'unknown', 'string', 'number', 'boolean', 'object',
  'symbol', 'bigint', 'public', 'private', 'protected', 'readonly', 'static',
  'abstract', 'override', 'declare',
]);

const C_KEYWORDS = new Set([
  'auto', 'break', 'case', 'char', 'const', 'continue', 'default', 'do',
  'double', 'else', 'enum', 'extern', 'float', 'for', 'goto', 'if', 'inline',
  'int', 'long', 'register', 'restrict', 'return', 'short', 'signed', 'sizeof',
  'static', 'struct', 'switch', 'typedef', 'union', 'unsigned', 'void',
  'volatile', 'while', '_Bool', '_Atomic', '_Static_assert',
  // stdbool / stdint / common spellings so they read as keyword-ish
  'bool', 'true', 'false', 'NULL', 'size_t', 'ssize_t', 'ptrdiff_t',
  'uint8_t', 'uint16_t', 'uint32_t', 'uint64_t',
  'int8_t', 'int16_t', 'int32_t', 'int64_t',
]);

const LUA_KEYWORDS = new Set([
  'and', 'break', 'do', 'else', 'elseif', 'end', 'false', 'for', 'function',
  'goto', 'if', 'in', 'local', 'nil', 'not', 'or', 'repeat', 'return', 'then',
  'true', 'until', 'while', 'self',
]);

export function detectLang(file: string): HighlightLang {
  const f = file.toLowerCase();
  if (f.endsWith('.rs')) return 'rust';
  if (f.endsWith('.lua')) return 'lua';
  if (f.endsWith('.c') || f.endsWith('.h')) return 'c';
  if (f.endsWith('.ts') || f.endsWith('.tsx') || f.endsWith('.js') || f.endsWith('.jsx') || f.endsWith('.mts') || f.endsWith('.cts')) return 'ts';
  return null;
}

function keywordsFor(lang: Exclude<HighlightLang, null>): Set<string> {
  switch (lang) {
    case 'rust': return RUST_KEYWORDS;
    case 'ts': return TS_KEYWORDS;
    case 'c': return C_KEYWORDS;
    case 'lua': return LUA_KEYWORDS;
  }
}

/**
 * Single-pass syntax highlighter producing an HTML string with `<span>`
 * wrappers around tokens. Comments / strings / numbers / keywords / types
 * each get a CSS variable hook from the existing kind palette so the
 * highlighting blends with the rest of the shell rather than introducing
 * a new colour scheme.
 *
 * Trade-offs (V0):
 *   - Regex tokeniser, not a real lexer — fine for read-only previews,
 *     would mis-tokenise pathological cases (nested template literals,
 *     escaped quotes spanning lines) but those rarely appear in symbol
 *     bodies.
 *   - Two languages only: Rust + TS family. Unknown extensions render
 *     as plain escaped text.
 */
export function highlightSource(code: string, file: string): string {
  const lang = detectLang(file);
  if (lang === null) return escapeHtml(code);
  const keywords = keywordsFor(lang);
  // Order matters in the alternation: comments + strings must win
  // over keywords/identifiers since e.g. `// fn foo` should stay all-
  // comment, not partly-keyword. Lua diverges: `--` / `--[[ ]]`
  // comments and `[[ ]]` long strings instead of C-style `//` `/* */`.
  const re = lang === 'lua'
    ? /(--\[\[[\s\S]*?\]\]|--[^\n]*|\[\[[\s\S]*?\]\]|"(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|\b\d+(?:\.\d+)?(?:[eE][-+]?\d+)?\b|\b[A-Z][a-zA-Z0-9_]*\b|\b[a-zA-Z_][a-zA-Z0-9_]*\b)/g
    : /(\/\/[^\n]*|\/\*[\s\S]*?\*\/|"(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|`(?:[^`\\]|\\.)*`|\b\d+(?:\.\d+)?(?:[eE][-+]?\d+)?\b|\b[A-Z][a-zA-Z0-9_]*\b|\b[a-zA-Z_][a-zA-Z0-9_]*\b)/g;
  let out = '';
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(code)) !== null) {
    const tok = m[0];
    const start = m.index;
    if (start > last) out += escapeHtml(code.slice(last, start));
    const cls = classifyToken(tok, keywords);
    if (cls === null) out += escapeHtml(tok);
    else out += `<span style="color: var(${cls})">${escapeHtml(tok)}</span>`;
    last = start + tok.length;
  }
  if (last < code.length) out += escapeHtml(code.slice(last));
  return out;
}

function classifyToken(tok: string, keywords: Set<string>): string | null {
  if (tok.startsWith('//') || tok.startsWith('/*') || tok.startsWith('--')) return '--sd-fg-muted';
  if (tok.startsWith('"') || tok.startsWith("'") || tok.startsWith('`') || tok.startsWith('[[')) return '--sd-status-ok';
  if (/^\d/.test(tok)) return '--sd-kind-value';
  if (keywords.has(tok)) return '--sd-kind-callable';
  if (/^[A-Z]/.test(tok)) return '--sd-kind-type';
  return null;
}
