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
allowed-tools: mcp__standardoc__find_symbol mcp__standardoc__get_context mcp__standardoc__list_symbols mcp__standardoc__find_symbols_by_pattern mcp__standardoc__find_similar_symbols mcp__standardoc__get_body mcp__standardoc__fetch_chunks mcp__standardoc__resolve_external mcp__standardoc__current_revision mcp__standardoc__check_stale mcp__standardoc__usage_stats mcp__standardoc__session_save mcp__standardoc__session_list mcp__standardoc__session_get
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

## 3-phase MCP-first protocol (mandatory pacing)

To keep responses tight and avoid blowing the context window, every
investigation should walk these three phases in order. Skipping
straight to Phase 3 on cold context is the most expensive mistake.

| Phase           | Tools                                                                                  | Cost / call     | Goal                                                |
| --------------- | -------------------------------------------------------------------------------------- | --------------- | --------------------------------------------------- |
| 1. **Explore**  | \`find_symbol\`, \`list_symbols\`, \`find_symbols_by_pattern\` — **always with filters** | ~0.5–2 KB       | Cartographier les FQDNs candidats                   |
| 2. **Cibler**   | \`get_context(fqdn, depth=1)\`                                                         | ~1–3 KB         | Voir les voisins en FQDN-only, repérer les 1-3 qui comptent |
| 3. **Drill**    | \`get_context(fqdn, depth=2)\` ou \`get_body(fqdn, …, strip_attrs=true, signature_only=true)\` | ~5–15 KB        | Lecture détaillée, **uniquement** sur les voisins validés en Phase 2 |

**Daemon-side enforcement:** when \`get_context(depth=2)\` is called on
a FQDN that has not had a \`depth=1\` call in the last 5 minutes, the
response includes a \`routing_hint\` explaining the protocol. Treat
that hint as a correction signal — back off, run depth=1 first, then
return to depth=2 only on the specific neighbor you actually need.

Rule of thumb: you should be able to name the exact symbol and the
specific reason before reaching for depth=2 or unbounded \`get_body\`.
If you can't, you're still in Phase 1 or Phase 2.

## FQDN input convention

Every exact-match endpoint (\`get_body\`, \`get_context\`,
\`resolve_external\`, \`check_stale\`, and the \`module\` filter on every
search tool) normalises \`.\` to \`::\` at the MCP boundary. You can
type the FQDN in OOP style (\`Type.method\`) or canonical style
(\`Type::method\`) — both resolve to the same stored row. The
canonical form on disk stays \`::\`, and that's what the server
returns in every \`fqdn\` field. Don't try alternative separators
(\`/\`, \`->\`, \`-\`); only \`.\` is normalised.

## Discovery tools

### find_symbol(query, limit?)

Fuzzy FTS5 search over symbol names → array of \`RawSymbol\` with FQDN,
kind, file:line. Optional filters: \`kind\`, \`visibility\`, \`module\`,
\`include_external\` (default true).

Hyphens, dots, and \`::\` in the query are split into AND-tokens
server-side — \`find_symbol("standardoc-cli")\` matches the same
symbols as \`find_symbol("standardoc cli")\`. No need to escape.

**Empty result enrichment** : when the query produces zero matches,
the response switches from a bare array to
\`{results: [], did_you_mean: [{fqdn, name, kind, score}, …]}\` with
up to 5 strsim-scored suggestions (threshold 0.6). Accept the
\`did_you_mean\` list as the answer instead of spinning variant
queries. If the suggestions don't fit, the symbol genuinely doesn't
exist.

\`\`\`
find_symbol("createUser", 5)
\`\`\`

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

Same \`did_you_mean\` enrichment as \`find_symbol\` on empty result :
\`{results: [], did_you_mean: [...]}\` with strsim run on the
pattern's core (wildcards stripped). \`*to_token_string*\` surfaces
\`to_token_stream\` in one call — accept it rather than guessing
spellings.

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

## Reasoning tools

### get_context(fqdn, depth: 1|2)

Returns the symbol + its neighbors (callers, callees, imports,
imported_by, dependents, tests) as a graph slice.

- **depth=1 (cheap)** — neighbors as FQDN + edge_kind only. For
  exploration, building a mental map.
- **depth=2 (rich)** — same + full \`resolved_symbol\` payload for
  Resolved targets. For reasoning about actual code.

### get_body(fqdn, max_lines?, strip_attrs?, signature_only?)

Returns the raw source text of a symbol identified by FQDN, sliced
from the file at its declared \`start_line..end_line\`. Pair with
\`get_context\` (graph relations) when you need to actually read the
function body — the graph tells you WHERE, this tells you WHAT.

Three orthogonal knobs to keep the response tight:

- \`max_lines\` clamps total returned lines. Response sets
  \`truncated: true\` and \`total_body_lines\` so you can re-fetch
  without the cap when needed.
- \`strip_attrs: true\` drops leading doc comments (\`///\`, \`//\`,
  \`/* … */\`) AND attribute blocks (\`#[…]\`, \`#![…]\` — multi-line
  attrs are detected via paren depth). Response sets
  \`stripped_lines: N\`. Massive shrink on handlers buried under
  verbose \`#[tool(description = "…")]\` blocks.
- \`signature_only: true\` truncates after the first line containing
  \`{\` — returns the multi-line signature without the
  implementation. Response sets \`signature_only: true\`. Combine
  with \`strip_attrs\` for the cleanest signature view.

Returns \`null\` when no symbol matches the FQDN — call
\`find_symbol\` first if you only have a name fragment.

**Indentation is compacted** before the body reaches you : the
longest leading-whitespace prefix shared by every non-blank line is
stripped (\`dedented_prefix_len\` reports how many bytes were shaved),
and remaining uniform 4-space (or 2-space) runs are converted to
\`\\t\` (\`indent_unit\` is \`"\\t"\` for tab-indented output, \`""\` when
the residual indent was mixed and left verbatim). Column positions
inside the returned body are NOT 1:1 with the source file — refetch
verbatim by reading the file directly at \`start_line\` if you need
column-exact positions.

\`\`\`
get_body("crate::module::function_name", null, true, true)
// → multi-line signature only, no docs/attrs noise.
\`\`\`

### fetch_chunks(uris)

Resolves a list of \`rag://<id>\` URIs (the references surfaced in
\`chunk_refs\` on \`get_context\`) to full \`Chunk\` rows
\`{id, source_path, chunk_idx, text, text_hash, section_header,
byte_start, byte_end, created_at}\`. Unknown / non-existent ids are
silently skipped — diff inputs vs outputs to detect drops. Returns
chunks ordered by id ASC.

The chunk-aware reasoning loop :

\`\`\`
get_context(fqdn, depth=1, query?)
  → response.chunk_refs = [{uri, confidence, source_path, section_header}, ...]
fetch_chunks([uri1, uri2, ...])
  → [{text, ...}, ...]   // the actual prose to consider alongside the graph
\`\`\`

\`get_context\` alone gives you envelopes ; \`fetch_chunks\` retrieves
the actual text. Don't fetch every chunk — pick the 1-3 with the
highest \`confidence\` (or the ones whose \`section_header\` matches
your task) and fetch only those.

\`\`\`
fetch_chunks(["rag://42", "rag://17"])
\`\`\`

### resolve_external(fqdn)

Lazy on-demand resolution of an external FQDN — a symbol that lives
outside the workspace (Cargo crate, npm package, luarocks rock).
Routes the FQDN through registered resolvers and submits the produced
source to the index with \`is_external = 1\`. Use when
\`get_context(fqdn)\` returned a neighbor as
\`Unresolved { name }\` whose name looks like a known dependency.

Returns \`{status, fqdn, source_origin?, symbol?, missing_binary?, detail?}\`:

- \`status = "resolved"\` — \`symbol\` is the newly-indexed \`RawSymbol\`.
- \`status = "not_found"\` — no resolver claimed this FQDN (likely
  not a workspace dependency).
- \`status = "missing_binary"\` — the matching resolver is gated behind
  a CLI that is not installed; \`missing_binary\` names which one,
  \`detail\` names the env var to override the lookup.
- \`status = "lockfile_not_found"\` — workspace lacks the lockfile
  needed (\`Cargo.lock\` / \`package-lock.json\` / …).
- \`status = "error"\` — resolver-level failure; \`detail\` carries the
  message.

\`\`\`
resolve_external("serde::Deserialize")
\`\`\`

## Boot-time capability check

### current_revision()

Returns the current workspace revision AND the daemon's wired
capabilities. Call this ONCE at session start to learn what's
available, then route your tool flow accordingly.

\`\`\`json
{
  "revision": 354,
  "rag": {
    "enabled": true,
    "embedder": { "id": "bge-small-en-v1.5", "dim": 384 }
  },
  "watcher": { "active": true },
  "indexing": { "ready": true }
}
\`\`\`

**Decision matrix:**

- \`rag.enabled = false\` → never call \`fetch_chunks\`; do not pass
  \`query\` to \`get_context\` (will be ignored).
- \`rag.enabled = true\` AND \`rag.embedder = null\` → \`chunk_refs\` are
  populated but link-confidence ordered only; passing \`query\` to
  \`get_context\` is a no-op (silent).
- \`rag.embedder.id\` known → semantic re-rank works; pass natural-
  language \`query\` to \`get_context\` for relevant prose.
- \`indexing.ready = false\` → cold start in progress; read tools
  return a friendly "indexing in progress" text. Wait or back off.
- \`watcher.active = false\` after \`indexing.ready = true\` → the daemon
  was booted in \`--readonly\` mode. The index will not refresh on file
  edits; rely on \`check_stale\` against the revision you observed.

The \`revision\` is a monotonic counter that bumps on every successful
index write (cold-start ingest, watcher upsert, external resolution).
Pair it with \`check_stale\` to detect when symbols you previously
cited have been modified since your last fetch.

### check_stale(fetched: [{fqdn, fetched_at_revision}, ...])

Compares a set of \`(fqdn, fetched_at_revision)\` pairs against the
current \`last_modified_revision\` of each row. Returns
\`[{fqdn, fetched_at_revision, last_modified_revision, status}]\` where
\`status\` is:

- \`"stale"\` — the symbol was modified since you fetched it; re-query.
- \`"fresh"\` — no change since fetch.
- \`"missing"\` — the FQDN is no longer indexed (renamed / removed).

Stateless server-side — track the \`(fqdn → revision)\` map yourself
across turns. Call BEFORE re-reasoning on cached context.

## Telemetry

### usage_stats(period?)

Returns the running tally of bytes the standardoc tools have returned
vs. the raw file bytes those responses pointed at. \`period\` accepts
\`day\`, \`week\`, \`all\` (default). Baseline is \`sum(file_sizes)\` of
distinct source files referenced by each response — the honest "what
an AI would have consumed reading the relevant sources raw" floor (no
estimation multiplier). Response shape:

\`\`\`
{
  period: "all",
  calls: <int>,
  bytes_out_total: <int>,       // what tools returned to the AI
  baseline_bytes_total: <int>,  // raw file bytes that would have been read
  bytes_saved: <int>,           // baseline - out (can be negative)
  ratio: <float>                // bytes_out / baseline
}
\`\`\`

A ratio of 0.14 means standardoc surfaced 14% of the raw bytes for
the relevant source files — the rest is context the AI did not pay
for. Only successful read-path tool calls are logged.

\`reset_usage_stats\` exists as a separate \`stdoc reset-usage\` CLI
sub-command for baselining a measurement run — invoke it from the
shell when you want a clean zero before counting bytes saved.

## Session handoffs

A separate SQLite DB at \`.standardoc-sessions/sessions.db\` (path
independent of \`.standardoc/\` so a workspace reset doesn't wipe your
handoff memos).

### session_save(slug, body_md, supersedes?)

UPSERT by \`slug\`. Use AT END of any session that locks decisions or
ships meaningful work so the next chat can pick up via \`session_get\`.
Optional \`supersedes\` marks a prior slug as \`superseded\` (chain
semantics — useful when a refactor invalidates an older lock).

### session_list(active_only?)

List session memos newest-first. \`active_only\` defaults to true and
filters out superseded entries. Returns the full \`body_md\` per row.

### session_get(slug?)

Fetch one memo. Pass \`slug\` to target a specific entry; omit it to
get the most recent active session — the natural reentry point for a
new chat. Returns \`null\` when nothing matches.

## Recommended workflows

**"What does X do / where is X used"**

1. \`find_symbol("X")\` → pick the right FQDN
2. \`get_context(fqdn, 1)\` → cheap neighborhood
3. \`get_context(fqdn, 2)\` → rich payloads if reasoning needed

**"I need to modify behavior Y"**

1. \`find_symbol("Y")\` → entry points
2. \`get_context(fqdn, 2)\` on each candidate → understand the call chain
3. \`get_body(fqdn)\` on the symbol you intend to edit → read the actual code
4. Now you know what to read/edit

**"Is symbol X used anywhere"**

1. \`find_symbol("X")\` → get FQDN
2. \`get_context(fqdn, 1)\` → check \`callers\` (CALLS), \`imported_by\`
   (IMPORTS), and \`dependents\` (everything else that breaks if X
   changes shape)

**"I'm starting a feature involving area Z"**

1. \`find_symbol("Z")\` → discover related symbols
2. For each, \`get_context(fqdn, 1)\` → map the surrounding graph
3. Read source files only after you have located the right entry points

**"Pull in prose / docs alongside the code graph"**

1. \`current_revision()\` → confirm \`rag.enabled = true\`.
2. \`get_context(fqdn, 1, query?)\` → response carries
   \`chunk_refs: [{uri, confidence, source_path, section_header}]\`.
3. \`fetch_chunks([top_uri])\` → actual prose text. Pair the prose with
   the graph slice from step 2 for full picture (graph says WHERE
   and WHO ; prose says WHY).

**"Detect templated/duplicate names across modules"**

1. \`find_similar_symbols(anchor)\` with one known sibling — the score
   surfaces the cluster (\`strip_rs_extension\` reveals the
   \`strip_<lang>_extension\` family without guessing the glob).
2. If you already know the pattern shape, prefer
   \`find_symbols_by_pattern\` for a deterministic glob match.

**"A neighbor is Unresolved and looks like an external dependency"**

1. \`get_context(fqdn, 1)\` reveals \`Unresolved { name }\` whose name
   matches a Cargo crate / npm package / luarocks rock you depend on.
2. \`resolve_external(name)\` indexes it on demand (one-shot;
   subsequent \`get_context\`/\`find_symbol\` calls on the same crate
   reuse the cache).
3. If \`status = "missing_binary"\` or \`"lockfile_not_found"\`,
   surface the diagnostic to the user — they need to install the CLI
   or commit a lockfile.

**"Detect what changed since last fetch"**

1. When you fetch context, record \`(fqdn, current_revision())\`.
2. Across turns, call
   \`check_stale({fetched: [{fqdn, fetched_at_revision}, ...]})\` to
   diff against \`last_modified_revision\`.
3. Re-query the \`"stale"\` fqdns; drop the \`"missing"\` ones.

**"Resume / save a session handoff"**

1. \`session_get()\` (no slug) → latest active handoff, the natural
   entry point for a new chat.
2. Or \`session_list({active_only: true})\` to scan recent memos.
3. At end of a session that locks decisions or ships work,
   \`session_save(slug, body_md)\` so the next chat can pick up.

## Key concepts

- **FQDN** — \`<package>::<module>::<name>\` (Rust + TS unified). Stable
  identifier across the workspace.
- **Edge kinds** — CALLS, IMPORTS, EXTENDS, IMPLEMENTS, REFERENCES,
  DEFINES, USES_TYPE, EXPOSES_API.
- **Resolved vs Unresolved targets** — an edge target may be:
  - \`Resolved { fqdn }\` — known, points to an indexed symbol.
  - \`Unresolved { name }\` — name only, external or unindexed.
    Candidate for \`resolve_external\` if it looks like a dependency.
  - \`UnresolvedBridge { bridge, name }\` — cross-language jump (e.g.
    Rust ↔ TS via Tauri command).

  Don't blindly follow Unresolved targets — they leave the indexed
  graph unless you \`resolve_external\` them.

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
