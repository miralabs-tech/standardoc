/**
 * Strip JSONC — `//` line comments, block comments, and trailing commas —
 * down to plain JSON, preserving string literals verbatim. Claude Code
 * tolerates JSONC in `.claude/settings.json`, so `init` strips it before
 * parsing instead of bailing to a manual-merge prompt. Two string-aware
 * passes guarantee a `//` / `,]` sequence inside a string value is never
 * touched.
 */
export function stripJsonc(raw: string): string {
  return stripTrailingCommas(stripComments(raw));
}

function stripComments(raw: string): string {
  let out = '';
  let inString = false;
  let escaped = false;
  for (let i = 0; i < raw.length; i++) {
    const ch = raw[i];
    if (inString) {
      out += ch;
      if (escaped) escaped = false;
      else if (ch === '\\') escaped = true;
      else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"') {
      inString = true;
      out += ch;
      continue;
    }
    if (ch === '/' && raw[i + 1] === '/') {
      i += 2;
      while (i < raw.length && raw[i] !== '\n') i++;
      if (i < raw.length) out += '\n'; // keep line count stable for error spans
      continue;
    }
    if (ch === '/' && raw[i + 1] === '*') {
      i += 2;
      while (i < raw.length && !(raw[i] === '*' && raw[i + 1] === '/')) i++;
      i += 1; // skip the closing '/'
      continue;
    }
    out += ch;
  }
  return out;
}

function stripTrailingCommas(raw: string): string {
  let out = '';
  let inString = false;
  let escaped = false;
  for (let i = 0; i < raw.length; i++) {
    const ch = raw[i];
    if (inString) {
      out += ch;
      if (escaped) escaped = false;
      else if (ch === '\\') escaped = true;
      else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"') {
      inString = true;
      out += ch;
      continue;
    }
    if (ch === ',') {
      let j = i + 1;
      while (j < raw.length && /\s/.test(raw[j] ?? '')) j++;
      if (raw[j] === '}' || raw[j] === ']') continue; // drop trailing comma
    }
    out += ch;
  }
  return out;
}
