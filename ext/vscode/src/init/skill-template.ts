export const SKILL_RELATIVE_DIR = '.claude/skills/standardoc';
export const SKILL_RELATIVE_PATH = '.claude/skills/standardoc/SKILL.md';

const SKILL_CONTENT = `---
name: standardoc
description: Standardoc is the primary tool for understanding the semantic
  structure of this codebase. Use it as your FIRST reflex on any task that
  touches code — adding a feature, fixing a bug, refactoring, code review,
  answering "where is X defined", "who calls Y", "what does Z depend on".
  The index is built from the AST and kept live by a file watcher; it is
  the source of truth for code structure on this workspace.
when_to_use: ALWAYS use Standardoc FIRST when exploring or modifying code
  on this workspace, BEFORE Read/Grep/Glob. Tool fallback hierarchy is
  (1) Standardoc MCP → (2) LSP / IDE Go-to-Definition (where available)
  → (3) raw file Read/Grep/Glob. Skip Standardoc only for pure text
  matching (strings, comments, config files unrelated to code symbols)
  or for files at a known path that you just need to read verbatim.
allowed-tools: mcp__standardoc__find_symbol mcp__standardoc__get_context mcp__standardoc__list_symbols mcp__standardoc__find_symbols_by_pattern mcp__standardoc__find_similar_symbols
---

# Standardoc — Primary Code Navigation

**The presence of this skill means this workspace has a live Standardoc
semantic index.** Use it FIRST for any code task. The index is derived
from the AST and kept in sync by a file watcher — it is the source of
truth for code structure here.

## Tool fallback hierarchy

For any task that involves understanding or modifying code on this
workspace:

1. **Standardoc MCP** (this skill) — semantic graph, FQDN-keyed,
   AST-derived. Try first.
2. **LSP / IDE Go-to-Definition** — fallback when Standardoc returns
   no result or you need editor-level navigation.
3. **Raw Read / Grep / Glob** — last resort, or for plain-text needs
   (comments, strings, build files, markdown).

## Tools available

### find_symbol(query, limit?)

Fuzzy FTS5 search over symbol names → array of \`RawSymbol\` with FQDN,
kind, file:line. Optional filters: \`kind\`, \`visibility\`, \`module\`.

\`\`\`
find_symbol("createUser", 5)
\`\`\`

### get_context(fqdn, depth: 1|2)

Returns the symbol + its neighbors (callers, callees, imports,
imported_by) as a graph slice.

- **depth=1 (cheap)** — neighbors as FQDN + edge_kind only. For
  exploration, building a mental map.
- **depth=2 (rich)** — same + full \`resolved_symbol\` payload for
  Resolved targets. For reasoning about actual code.

### list_symbols(kind?, visibility?, module?, limit?)

Filter-only listing — no query string, no glob pattern. Returns every
symbol matching the provided filters, ordered by FQDN. Use for audits
and inventories like "every private function" or "every type in module
X". Pass at least one filter to keep the result set bounded.

\`\`\`
list_symbols({ kind: "type", module: "myapp::domain" })
\`\`\`

### find_symbols_by_pattern(pattern, kind?, visibility?, module?, limit?)

Glob-pattern search over symbol \`name\` and \`fqdn\` (SQLite \`GLOB\`:
\`*\`, \`?\`, \`[abc]\`, **case-sensitive**). Use when you already know the
shape and want a deterministic match — e.g. \`strip_*_extension\` to
catch every \`strip_<lang>_extension\` helper, or
\`myapp::utils::*\` to enumerate a module.

\`\`\`
find_symbols_by_pattern("strip_*_extension")
\`\`\`

### find_similar_symbols(reference, threshold?, limit?, kind?, visibility?, module?)

Similarity-scored search around an anchor. Returns
\`[{score, symbol}]\` ranked by score descending. Use when you have ONE
anchor and want to discover related names without guessing a glob —
the score combines Jaro-Winkler (typo / prefix-similar) and Jaccard
over snake/camel-case tokens (templated families). The anchor itself
is self-skipped by case-insensitive name.

- \`threshold\` defaults to \`0.8\` in \`[0.0, 1.0]\` — lower for broader
  fuzzy matching, higher for tighter clusters.
- \`reference\` is raw text: a known name, an FQDN's tail, or a
  hypothetical name all work.

\`\`\`
find_similar_symbols("strip_rs_extension", 0.7)
// → strip_ts_extension, strip_lua_extension, strip_extension, ...
\`\`\`

## Recommended workflows

**"What does X do / where is X used"**

1. \`find_symbol("X")\` → pick the right FQDN
2. \`get_context(fqdn, 1)\` → cheap neighborhood
3. \`get_context(fqdn, 2)\` → rich payloads if reasoning needed

**"I need to modify behavior Y"**

1. \`find_symbol("Y")\` → entry points
2. \`get_context(fqdn, 2)\` on each candidate → understand the call chain
3. Now you know what to read/edit

**"Is symbol X used anywhere"**

1. \`find_symbol("X")\` → get FQDN
2. \`get_context(fqdn, 1)\` → check \`callers\` (CALLS) and \`imported_by\`
   (IMPORTS)

**"I'm starting a feature involving area Z"**

1. \`find_symbol("Z")\` → discover related symbols
2. For each, \`get_context(fqdn, 1)\` → map the surrounding graph
3. Read source files only after you have located the right entry points

**"Detect templated/duplicate names across modules"**

1. \`find_similar_symbols(anchor)\` with one known sibling — the score
   surfaces the cluster (\`strip_rs_extension\` reveals the
   \`strip_<lang>_extension\` family without guessing the glob).
2. If you already know the pattern shape, prefer
   \`find_symbols_by_pattern\` for a deterministic glob match.

## Key concepts

- **FQDN** — \`<package>::<module>::<name>\` (Rust + TS unified). Stable
  identifier across the workspace.
- **Edge kinds** — CALLS, IMPORTS, EXTENDS, IMPLEMENTS, REFERENCES,
  DEFINES, USES_TYPE, EXPOSES_API.
- **Resolved vs Unresolved targets** — an edge target may be:
  - \`Resolved { fqdn }\` — known, points to an indexed symbol.
  - \`Unresolved { name }\` — name only, external or unindexed.
  - \`UnresolvedBridge { bridge, name }\` — cross-language jump (e.g.
    Rust ↔ TS via Tauri command).

  Don't blindly follow Unresolved targets — they leave the indexed
  graph.

## Indexing state

If a tool returns \`"Workspace indexing in progress..."\`, cold start is
running (typically 5-15s on first activation). Wait briefly and retry.
After cold start, the watcher keeps the index live.

---

> Generated by Standardoc. Re-run the \`Standardoc: Regenerate AI agent
> skill\` command from the VSCode command palette to refresh after
> upgrades. Manual edits will be overwritten on regenerate.
`;

export function buildSkillContent(): string {
  return SKILL_CONTENT;
}

export function skillContentMatches(actual: string, expected: string): boolean {
  return normalize(actual) === normalize(expected);
}

function normalize(s: string): string {
  return s.replace(/\r\n/g, '\n').trimEnd();
}
