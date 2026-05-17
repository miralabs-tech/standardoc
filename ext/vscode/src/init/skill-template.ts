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
allowed-tools: mcp__standardoc__find_symbol mcp__standardoc__find_symbols_by_pattern mcp__standardoc__find_similar_symbols mcp__standardoc__list_symbols mcp__standardoc__find_call_sites mcp__standardoc__get_context mcp__standardoc__get_body mcp__standardoc__fetch_chunks mcp__standardoc__resolve_external mcp__standardoc__module_lookup mcp__standardoc__resolve_cross_workspace mcp__standardoc__current_revision mcp__standardoc__check_stale mcp__standardoc__usage_stats mcp__standardoc__list_projects mcp__standardoc__project_for_file mcp__standardoc__link_workspace mcp__standardoc__unlink_workspace mcp__standardoc__list_linked_workspaces mcp__standardoc__set_link_direction mcp__standardoc__refresh_peer mcp__standardoc__session_save mcp__standardoc__session_list mcp__standardoc__session_get mcp__standardoc__session_sync_in mcp__standardoc__session_sync_out
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

| Phase | Tools | Cost / call | Goal |
|---|---|---|---|
| 1. **Explore** | \`find_symbol\`, \`list_symbols\`, \`find_symbols_by_pattern\`, \`find_call_sites\` — **always with filters** | ~0.5–2 KB | Map candidate FQDNs / call patterns |
| 2. **Target** | \`get_context(fqdn, depth=1)\` | ~1–3 KB | See neighbors as FQDNs only, pick the 1–3 that matter |
| 3. **Drill** | \`get_context(fqdn, depth=2)\` or \`get_body(fqdn, …, strip_attrs=true, signature_only=true)\` | ~5–15 KB | Detailed read — **only** on neighbors validated in Phase 2 |

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
\`resolve_external\`, \`check_stale\`, \`find_call_sites.from_fqdn\`,
\`module_lookup\`, and the \`module\` filter on every search tool)
normalises \`.\` to \`::\` at the MCP boundary. You can type the FQDN in
OOP style (\`Type.method\`) or canonical style (\`Type::method\`) — both
resolve to the same stored row. The canonical form on disk stays
\`::\`, and that's what the server returns in every \`fqdn\` field.
Don't try alternative separators (\`/\`, \`->\`, \`-\`); only \`.\` is
normalised.

## Workspace scoping convention

Every discovery tool (\`find_symbol\`, \`find_symbols_by_pattern\`,
\`list_symbols\`) accepts an optional \`workspace_id\` parameter and
defaults to the **primary** workspace when omitted — matches the
"give me MY symbols" mental model. Pass a peer's \`workspace_id\` (as
returned by \`link_workspace\` or \`list_linked_workspaces\`) to query a
specific linked peer's source. There is **no "all workspaces" mode**
— call the tool once per workspace_id if you need cross-peer
visibility.

---

## Discovery tools

### find_symbol

\`find_symbol({ query, limit?, kind?, visibility?, module?, include_external?, workspace_id? })\`

Fuzzy FTS5 search over symbol \`name\` and \`fqdn\` columns → array of
\`RawSymbol\` with FQDN, kind, file:line.

| Param | Type | Default | Notes |
|---|---|---|---|
| \`query\` | string | required | Hyphens, dots, and \`::\` are split into AND-tokens server-side — \`find_symbol("standardoc-cli")\` matches the same as \`find_symbol("standardoc cli")\`. |
| \`limit\` | u8 | 20 | Capped at 100 server-side. |
| \`kind\` | string | — | One of \`function\`, \`type\`, \`value\`, \`module\`, \`macro\`. |
| \`visibility\` | string | — | One of \`public\`, \`private\`, \`crate\`, \`protected\`. |
| \`module\` | string | — | Exact match on the symbol's \`module\` fqdn. |
| \`include_external\` | bool | true | Set \`false\` to scope to workspace-only symbols. |
| \`workspace_id\` | string | primary | UUID of a linked peer; omit for primary scope. |

**Empty result enrichment** — when the query produces zero matches,
the response switches from a bare array to
\`{results: [], did_you_mean: [{fqdn, name, kind, score}, …]}\` with
up to 5 strsim-scored suggestions (threshold 0.6). Accept the
\`did_you_mean\` list as the answer instead of spinning variant
queries.

\`\`\`
find_symbol({ query: "createUser", limit: 5 })
\`\`\`

### find_symbols_by_pattern

\`find_symbols_by_pattern({ pattern, kind?, visibility?, module?, limit?, include_external?, workspace_id? })\`

Glob-pattern search over symbol \`name\` and \`fqdn\` (SQLite \`GLOB\`:
\`*\`, \`?\`, \`[abc]\`, **case-sensitive**). Use when you already know the
shape and want a deterministic match — e.g. \`strip_*_extension\` to
catch every \`strip_<lang>_extension\` helper, or \`myapp::utils::*\` to
enumerate a module.

Same \`workspace_id\` semantics as \`find_symbol\` (defaults to primary).
Same \`did_you_mean\` enrichment on empty result:
\`{results: [], did_you_mean: [...]}\` with strsim run on the
pattern's core (wildcards stripped).

\`\`\`
find_symbols_by_pattern({ pattern: "strip_*_extension" })
\`\`\`

### find_similar_symbols

\`find_similar_symbols({ reference, threshold?, limit?, kind?, visibility?, module?, include_external? })\`

Similarity-scored search around an anchor. Returns
\`[{score, symbol}]\` ranked by score descending. The score combines
Jaro-Winkler (typo / prefix-similar) and Jaccard over snake/camel-case
tokens (templated families). The anchor itself is self-skipped by
case-insensitive name.

| Param | Type | Default | Notes |
|---|---|---|---|
| \`reference\` | string | required | Raw text — known name, FQDN tail, or hypothetical name. |
| \`threshold\` | f32 | 0.8 | Score cutoff in \`[0.0, 1.0]\`. Lower for broader matching. |
| \`limit\` | u8 | 20 | Capped at 100. |

Note: \`find_similar_symbols\` does **not** expose a \`workspace_id\`
param — it implicitly scopes to primary at the SQL layer. Use the
other three discovery tools if you need explicit peer scoping.

\`\`\`
find_similar_symbols({ reference: "strip_rs_extension", threshold: 0.7 })
// → strip_ts_extension, strip_lua_extension, strip_extension, ...
\`\`\`

### list_symbols

\`list_symbols({ kind?, visibility?, module?, limit?, include_external?, cursor?, workspace_id? })\`

Filter-only listing — no query string, no glob pattern. Returns every
symbol matching the filters, ordered by FQDN. Use for audits like
"every private function" or "every type in module X". Pass at least
one filter to keep the result set bounded.

| Param | Type | Default | Notes |
|---|---|---|---|
| \`cursor\` | string | — | Pagination anchor — pass the previous response's \`next_cursor\`. Strict \`fqdn > cursor\` filter; ordering is always by \`fqdn\`. |
| \`limit\` | u8 | 20 | Per page; capped at 100. |
| \`workspace_id\` | string | primary | Same semantics as the other discovery tools. |

Response shape: \`{items: [RawSymbol, …], next_cursor: string | null}\`.
Walk the full set by re-calling with \`cursor = next_cursor\` until it
returns \`null\`.

\`\`\`
list_symbols({ kind: "type", module: "myapp::domain" })
\`\`\`

### find_call_sites *(IR-4-f, schema v10+)*

\`find_call_sites({ from_fqdn?, callee_text?, callee_pattern?, limit? })\`

Read-only query over the \`call_sites\` table — every call expression
emitted by the extractors (CallExpr in Rust/TS/Lua, NewExpr,
OptChain call, method call, macro invocation). Surfaces textual call
patterns the symbol graph alone can't expose: bridge invocations,
method-chain shapes, literal-string arg payloads.

Three filters AND-compose:

| Param | Type | Semantics |
|---|---|---|
| \`from_fqdn\` | string | Exact match on the enclosing fn/method FQDN — answers \`what does X call?\`. |
| \`callee_text\` | string | Exact match on the called expression text (\`tauri::invoke\`, \`obj.api.create\`, \`print\`) — answers \`who calls Y?\`. |
| \`callee_pattern\` | string | SQLite GLOB on the callee text. \`*\`, \`?\`, \`[abc]\`, case-sensitive. e.g. \`*tauri.invoke*\` for every Tauri invocation, \`M.api.*\` for every call into the \`M.api\` namespace. |
| \`limit\` | u8 | Defaults to 50, capped at 200 server-side. |

Empty/whitespace-only filter strings normalise to "no filter"
server-side. Calling with no filters returns the most recent N
call_sites for ops-style scanning.

Response shape: JSON array of
\`{from_fqdn, callee_text, args: [{value, is_string_literal}], receiver_chain: [..], site: {file, line, col}}\`.

\`\`\`
find_call_sites({ callee_pattern: "*tauri.invoke*" })
// → every Tauri command invocation workspace-wide, with arg payloads
find_call_sites({ from_fqdn: "myapp::frontend::login::handler" })
// → every call site inside that handler, including method chains
\`\`\`

---

## Reasoning tools

### get_context

\`get_context({ fqdn, depth?, query? })\`

Returns the symbol + its neighbors (callers, callees, imports,
imported_by, dependents, tests) as a graph slice.

| Param | Type | Default | Notes |
|---|---|---|---|
| \`fqdn\` | string | required | Canonical (\`::\`) or OOP (\`.\`) form both accepted. |
| \`depth\` | u8 | 1 | \`1\` = neighbor FQDNs + edge_kind only (cheap, exploration). \`2\` = same + full \`resolved_symbol\` payload per Resolved target (rich, reasoning). Hard-clamped to \`1..=2\`. |
| \`query\` | string | — | Natural-language query used to re-rank \`chunk_refs\` by cosine similarity. Silently ignored when the daemon has no RAG store or no embedder. |

The response also carries \`chunk_refs\` (envelopes pointing at related
prose chunks — markdown docs, ADRs, design notes); use
\`fetch_chunks\` to retrieve the actual text.

### get_body

\`get_body({ fqdn, max_lines?, strip_attrs?, signature_only? })\`

Returns the raw source text of a symbol identified by FQDN, sliced
from the file at its declared \`start_line..end_line\`. Pair with
\`get_context\` (graph relations) when you need to actually read the
function body — the graph tells you WHERE, this tells you WHAT.

Three orthogonal knobs to keep the response tight:

| Param | Type | Notes |
|---|---|---|
| \`max_lines\` | u32 | Cap total returned lines. Response sets \`truncated: true\` and \`total_body_lines\` so you can re-fetch without the cap when needed. |
| \`strip_attrs\` | bool | Drop leading doc comments (\`///\`, \`//!\`, \`//\`, \`/* … */\`) AND attribute lines (\`#[…]\`, \`#![…]\`, including multi-line continuations) AND blank lines between them. Response sets \`stripped_lines: N\`. Massive shrink on handlers buried under verbose \`#[tool(description = "…")]\` blocks. |
| \`signature_only\` | bool | Truncate after the first line containing \`{\` — returns the multi-line signature without the implementation. Response sets \`signature_only: true\`. No-op when no \`{\` is present. Combine with \`strip_attrs\` for the cleanest signature view. |

Returns \`null\` when no symbol matches the FQDN — call \`find_symbol\`
first if you only have a name fragment.

**Indentation is compacted** before the body reaches you: the
longest leading-whitespace prefix shared by every non-blank line is
stripped (\`dedented_prefix_len\` reports how many bytes were shaved),
and remaining uniform 4-space (or 2-space) runs are converted to
\`\\t\` (\`indent_unit\` is \`"\\t"\` for tab-indented output, \`""\` when
the residual indent was mixed and left verbatim). Column positions
inside the returned body are NOT 1:1 with the source file — refetch
verbatim by reading the file directly at \`start_line\` if you need
column-exact positions.

\`\`\`
get_body({ fqdn: "myapp::auth::verify_token", strip_attrs: true, signature_only: true })
// → multi-line signature only, no docs/attrs noise.
\`\`\`

### fetch_chunks

\`fetch_chunks({ uris })\`

Resolves a list of \`rag://<id>\` URIs (the references surfaced in
\`chunk_refs\` on \`get_context\`) to full \`Chunk\` rows
\`{id, source_path, chunk_idx, text, text_hash, section_header,
byte_start, byte_end, created_at}\`. Unknown / non-existent ids are
silently skipped — diff inputs vs outputs to detect drops. Returns
chunks ordered by id ASC. The \`uris\` list is hard-capped at 64.

The chunk-aware reasoning loop:

\`\`\`
get_context({ fqdn, depth: 1, query? })
  → response.chunk_refs = [{uri, confidence, source_path, section_header}, ...]
fetch_chunks({ uris: [uri1, uri2, ...] })
  → [{text, ...}, ...]   // the actual prose to consider alongside the graph
\`\`\`

\`get_context\` alone gives you envelopes; \`fetch_chunks\` retrieves
the actual text. Don't fetch every chunk — pick the 1–3 with the
highest \`confidence\` (or the ones whose \`section_header\` matches
your task) and fetch only those.

\`\`\`
fetch_chunks({ uris: ["rag://42", "rag://17"] })
\`\`\`

---

## External resolution

### resolve_external

\`resolve_external({ fqdn })\`

Lazy on-demand resolution of an external FQDN — a symbol that lives
outside the workspace (Cargo crate, npm package, luarocks rock).
Routes the FQDN through registered resolvers and submits the
produced source to the index with \`is_external = 1\`. Use when
\`get_context(fqdn)\` returned a neighbor as \`Unresolved { name }\`
whose name looks like a known dependency.

Returns \`{status, fqdn, source_origin?, symbol?, missing_binary?, detail?}\`:

| \`status\` | Meaning |
|---|---|
| \`resolved\` | \`symbol\` carries the newly-indexed \`RawSymbol\`, \`source_origin\` names the origin (\`cargo_registry\` / \`node_modules_dts\` / …). |
| \`not_found\` | No resolver claimed this FQDN (likely not a workspace dependency). |
| \`missing_binary\` | The matching resolver is gated behind a CLI that is not installed; \`missing_binary\` names it, \`detail\` names the env var override. |
| \`lockfile_not_found\` | Workspace lacks the lockfile needed (\`Cargo.lock\` / \`package-lock.json\` / …); \`detail\` carries the absent name. |
| \`error\` | Resolver-level failure; \`detail\` carries the message. |

\`\`\`
resolve_external({ fqdn: "serde::Deserialize" })
\`\`\`

---

## Cross-workspace catalog

Linked peer workspaces extend symbol resolution beyond this
workspace's tree without copying source. Their \`ModuleLookup\` blobs
and (with \`indexing_mode=extract\`) their source symbols are imported
at cold start and reachable through the cross-workspace resolver.

### link_workspace

\`link_workspace({ path, direction, indexing_mode? })\`

Register a linked peer workspace.

| Param | Type | Notes |
|---|---|---|
| \`path\` | string | Canonicalised server-side. Missing path returns \`invalid_params\` with a \`did_you_mean\` list built from sibling directories of the closest existing ancestor. |
| \`direction\` | string | One of \`in\` (peer feeds us), \`out\` (we feed peer), \`bidirectional\`. |
| \`indexing_mode\` | string | Optional; defaults to \`blob_import\` (Stage 3b-7-a — cheap copy of peer's pre-built DB). Pass \`extract\` to opt this peer into the Stage 3b-7-b autonomous source-walk pipeline (primary walks the peer's source files into its own index, tagged with the peer's workspace_id). |

Returns \`{workspace_id, root_path, direction}\` — \`workspace_id\` is a
freshly-minted UUID. Side effect: when \`direction\` is \`in\` or
\`bidirectional\`, the peer root is also registered on the live
watcher so subsequent file changes in that peer flow into the index
without waiting for the next cold_start.

### unlink_workspace

\`unlink_workspace({ workspace_id })\`

Unregister a linked workspace, dropping its \`module_lookups\` and
\`workspace_imports\` rows transactionally. Idempotent — unregistering
an unknown id is a no-op. Returns \`{ok: true}\`. Side effect: the
peer is also removed from the live watcher registry (idempotent).

### list_linked_workspaces

\`list_linked_workspaces()\`

Enumerate every linked workspace registered in the catalog. Each
entry surfaces \`workspace_id\`, canonical \`root_path\`, \`direction\`,
\`status\`, \`indexing_mode\`, and the epoch ms of registration and
last index run. Returns \`{workspaces: [...]}\`.

### set_link_direction

\`set_link_direction({ workspace_id, direction })\`

Flip the persisted link direction of a registered peer AND propagate
the change to the live watcher. Replaces the prior workaround of
\`unlink_workspace\` + \`link_workspace\` (which lost the
\`workspace_id\` and forced the caller to re-discover it).

| Param | Type | Notes |
|---|---|---|
| \`workspace_id\` | string | UUID of the peer to update. |
| \`direction\` | string | New direction: \`in\`, \`out\`, or \`bidirectional\`. |

Returns
\`{workspace_id, root_path, previous_direction, new_direction}\` so
callers can confirm the transition. Watcher side effects:

- \`Out → {in, bidirectional}\` → registers the peer on the live
  watcher (\`add_peer\`).
- \`{in, bidirectional} → out\` → unregisters the peer
  (\`remove_peer\`).
- Same-side transitions (e.g. \`in → bidirectional\`) are watcher
  no-ops — direction is metadata once the peer is being observed.

Unknown \`workspace_id\` returns \`invalid_params\` with the offending
id in the data envelope.

\`\`\`
set_link_direction({ workspace_id: "<uuid>", direction: "bidirectional" })
\`\`\`

### refresh_peer

\`refresh_peer({ workspace_id })\`

Explicit re-extraction of a single linked peer's source files into
the primary index. Use after editing the peer's source between
\`cold_start\`s — the live watcher only catches edits that happen
while the daemon is currently up, so peer source can drift across
sessions. This tool is the user-facing escape hatch: "I edited the
peer, re-index it now".

Same code path as \`cold_start\` uses internally, scoped to one peer.

Returns
\`{workspace_id, root_path, status, files_extracted, files_skipped_unchanged, files_parse_errors}\`
where \`status\` is one of:

| \`status.kind\` | Meaning |
|---|---|
| \`ok\` | Extraction completed; counters reflect what landed. |
| \`skipped_inactive\` | Peer's \`status\` in the catalog is not \`Active\`. |
| \`skipped_missing\` | Peer's root path no longer exists on disk. |
| \`failed\` | Extraction failed mid-run; \`detail\` carries the message. |

Unknown \`workspace_id\` returns \`invalid_params\` with the offending
id in the data envelope.

\`\`\`
refresh_peer({ workspace_id: "<uuid>" })
\`\`\`

### module_lookup

\`module_lookup({ module_fqdn, workspace_id? })\`

Fetch the persisted \`ModuleLookup\` blob (full structured
bindings/scopes/imports as JSON) for \`(workspace_id, module_fqdn)\`.
\`workspace_id\` defaults to \`"primary"\` — pass a linked workspace
UUID to inspect a peer's lookup state.

Returns the \`ModuleLookup\` object, or \`null\` when no row matches.
Useful to debug the Stage 3a AOT pre-pass output without re-running
the indexer.

### resolve_cross_workspace

\`resolve_cross_workspace({ origin_module, origin_symbol })\`

Enumerate every linked workspace that re-exports / declares the
requested symbol at module-root scope. Returns
\`{providers: [{workspace_id, root_path, …}, ...]}\` listing each peer
that exposes the symbol. Empty list = no peer matches.

---

## Projects

Every cold start runs project detection (Stage 3d) against the
workspace root via \`standarbuild-detect\`. The projects table is
re-synced when a manifest file changes (Stage 3d-5 watcher).

### list_projects

\`list_projects()\`

Enumerate every project detected at cold-start. Returns
\`{projects: [...]}\` with one row per project, each carrying
\`project_id\`, \`label\`, \`kind\` (\`rust\` / \`node\` / \`bun\` / \`deno\` /
\`python\` / \`lua\` / \`c\` / \`cpp\` / \`custom:<tag>\`), absolute
\`root_path\`, and POSIX-style \`rel_path\` to the workspace root.
Empty list = no manifests found (workspace treated as a single
anonymous project).

### project_for_file

\`project_for_file({ path })\`

Resolve the project owning a file by absolute path. Walks up the
registered project tree for the deepest ancestor match. Returns the
matched project's metadata, or \`null\` when the path is outside
every registered project root.

---

## Boot-time capability check

### current_revision

\`current_revision()\`

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
  "indexing": { "ready": true },
  "workspace": { "kind": "cargo" }
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
- \`workspace.kind\` → detected workspace organizer (\`cargo\`, \`npm\`,
  \`pnpm\`, \`yarn\`, \`bun\`, \`deno\`, \`go\`, \`lerna\`, \`nx\`, \`turborepo\`,
  \`mira\`, \`custom:<tag>\`). \`null\` when no workspace manifest is
  present at the root (loose project tree).

The \`revision\` is a monotonic counter that bumps on every successful
index write (cold-start ingest, watcher upsert, external resolution).
Pair it with \`check_stale\` to detect when symbols you previously
cited have been modified since your last fetch.

### check_stale

\`check_stale({ fetched: [{fqdn, fetched_at_revision}, ...] })\`

Compares a set of \`(fqdn, fetched_at_revision)\` pairs against the
current \`last_modified_revision\` of each row. Returns
\`[{fqdn, fetched_at_revision, last_modified_revision, status}]\`
where \`status\` is:

- \`stale\` — the symbol was modified since you fetched it; re-query.
- \`fresh\` — no change since fetch.
- \`missing\` — the FQDN is no longer indexed (renamed / removed).

Stateless server-side — track the \`(fqdn → revision)\` map yourself
across turns. Call BEFORE re-reasoning on cached context.

---

## Telemetry

### usage_stats

\`usage_stats({ period? })\`

Returns the running tally of bytes the standardoc tools have returned
vs. the raw file bytes those responses pointed at.

| Param | Accepted values | Default |
|---|---|---|
| \`period\` | \`day\` / \`d\` / \`today\`, \`week\` / \`w\` / \`7d\`, \`all\` | \`all\` |

Anything else returns \`invalid_params\` rather than silently coercing.

Baseline is \`sum(file_sizes)\` of distinct source files referenced by
each response — the honest "what an AI would have consumed reading
the relevant sources raw" floor (no estimation multiplier).
Response shape:

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

The CLI sub-command \`stdoc reset-usage\` zeroes the counters from the
shell when you want a clean baseline for a measurement run.

---

## Session handoffs

A separate SQLite DB at \`.standardoc-sessions/sessions.db\` (path
independent of \`.standardoc/\` so a workspace reset doesn't wipe your
handoff memos).

### session_save

\`session_save({ slug, body_md, supersedes? })\`

UPSERT by \`slug\`. Use AT END of any session that locks decisions or
ships meaningful work so the next chat can pick up via \`session_get\`.

| Param | Type | Notes |
|---|---|---|
| \`slug\` | string | Unique identifier; UPSERT semantics. |
| \`body_md\` | string | Markdown payload — the memo content. |
| \`supersedes\` | string | Optional prior slug to mark as \`superseded\` (chain semantics — useful when a refactor invalidates an older lock). |

### session_list

\`session_list({ active_only? })\`

List session memos newest-first.

| Param | Type | Default | Notes |
|---|---|---|---|
| \`active_only\` | bool | true | Filter out superseded entries. |

Returns the full \`body_md\` per row.

### session_get

\`session_get({ slug? })\`

Fetch one memo. Pass \`slug\` to target a specific entry; omit it to
get the most recent active session — the natural reentry point for a
new chat. Returns \`null\` when nothing matches.

### session_sync_in

\`session_sync_in({ dir })\`

Walk a directory of session-memo \`.md\` files, classify each by its
frontmatter \`type:\`, and UPSERT the full row state (status,
supersedes, created_at) into the sessions table. Used when migrating
memos from another machine.

### session_sync_out

\`session_sync_out({ dir })\`

Inverse of \`session_sync_in\`. Dump every session row to \`<slug>.md\`
under \`dir\` with extended frontmatter (status, supersedes,
created_at) and regenerate \`MEMORY.md\` as an index. Used to migrate
sessions across machines or back up to a git repo.

---

## Recommended workflows

**"What does X do / where is X used"**

1. \`find_symbol({ query: "X" })\` → pick the right FQDN
2. \`get_context({ fqdn, depth: 1 })\` → cheap neighborhood
3. \`get_context({ fqdn, depth: 2 })\` → rich payloads if reasoning needed

**"I need to modify behavior Y"**

1. \`find_symbol({ query: "Y" })\` → entry points
2. \`get_context({ fqdn, depth: 2 })\` on each candidate → understand the call chain
3. \`get_body({ fqdn })\` on the symbol you intend to edit → read the actual code

**"Is symbol X used anywhere"**

1. \`find_symbol({ query: "X" })\` → get FQDN
2. \`get_context({ fqdn, depth: 1 })\` → check \`callers\` (CALLS),
   \`imported_by\` (IMPORTS), and \`dependents\` (everything else that
   breaks if X changes shape)

**"I'm starting a feature involving area Z"**

1. \`find_symbol({ query: "Z" })\` → discover related symbols
2. For each, \`get_context({ fqdn, depth: 1 })\` → map the surrounding graph
3. Read source files only after you have located the right entry points

**"Find every textual call pattern of shape Z"** *(IR-4-f)*

1. \`find_call_sites({ callee_pattern: "*Z*" })\` → every call whose
   callee text matches. Returns args (literal strings classified
   separately) and receiver_chain so you can see the full shape
   without re-parsing source.
2. For a specific caller: \`find_call_sites({ from_fqdn: "X" })\`.
3. Use this for bridge-invocation audits, method-chain shape
   queries, literal-arg payload mining.

**"Detect templated/duplicate names across modules"**

1. \`find_similar_symbols({ reference: anchor })\` with one known
   sibling — the score surfaces the cluster (\`strip_rs_extension\`
   reveals the \`strip_<lang>_extension\` family without guessing the
   glob).
2. If you already know the pattern shape, prefer
   \`find_symbols_by_pattern\` for a deterministic glob match.

**"Pull in prose / docs alongside the code graph"**

1. \`current_revision()\` → confirm \`rag.enabled = true\`.
2. \`get_context({ fqdn, depth: 1, query: "..." })\` → response
   carries \`chunk_refs: [{uri, confidence, source_path, section_header}]\`.
3. \`fetch_chunks({ uris: [top_uri] })\` → actual prose text. Pair the
   prose with the graph slice from step 2 for the full picture
   (graph says WHERE and WHO; prose says WHY).

**"A neighbor is Unresolved and looks like an external dependency"**

1. \`get_context({ fqdn, depth: 1 })\` reveals \`Unresolved { name }\`
   whose name matches a Cargo crate / npm package / luarocks rock
   you depend on.
2. \`resolve_external({ fqdn: name })\` indexes it on demand (one-shot;
   subsequent calls on the same crate reuse the cache).
3. If \`status = "missing_binary"\` or \`"lockfile_not_found"\`, surface
   the diagnostic to the user — they need to install the CLI or
   commit a lockfile.

**"A neighbor looks like a symbol from a linked peer workspace"**

1. \`list_linked_workspaces()\` → confirm a peer is registered.
2. \`resolve_cross_workspace({ origin_module, origin_symbol })\` → see
   which peers expose it.
3. \`module_lookup({ module_fqdn, workspace_id: <peer-uuid> })\` → drill
   into the peer's persisted \`ModuleLookup\` for the full binding
   shape if you need to debug resolution.
4. \`find_symbol({ query: name, workspace_id: <peer-uuid> })\` → search
   that peer's symbols directly (defaults to primary otherwise).

**"User edited a linked peer outside the daemon's lifetime"**

1. \`list_linked_workspaces()\` → identify the peer's \`workspace_id\`.
2. \`refresh_peer({ workspace_id })\` → re-extract that peer's source.
3. Response carries \`files_extracted\` / \`files_skipped_unchanged\` /
   \`files_parse_errors\` so you can confirm what landed.

**"User wants to change a peer's direction without losing its workspace_id"**

1. \`set_link_direction({ workspace_id, direction: "in" | "out" | "bidirectional" })\`
2. Response surfaces \`previous_direction\` + \`new_direction\` — confirm
   the transition. The watcher is updated transparently (peer is
   added or removed from the live watch on Out ↔ {in, bidirectional}
   transitions).

**"Detect what changed since last fetch"**

1. When you fetch context, record \`(fqdn, current_revision())\`.
2. Across turns, call
   \`check_stale({ fetched: [{fqdn, fetched_at_revision}, ...] })\` to
   diff against \`last_modified_revision\`.
3. Re-query the \`stale\` fqdns; drop the \`missing\` ones.

**"Map the project layout of an unfamiliar workspace"**

1. \`current_revision()\` → check \`workspace.kind\` to learn the
   monorepo organizer (or \`null\` for a loose project tree).
2. \`list_projects()\` → enumerate every detected sub-project with
   its kind and root path.
3. For a specific file, \`project_for_file({ path })\` to learn which
   sub-project owns it.

**"Resume / save a session handoff"**

1. \`session_get()\` (no slug) → latest active handoff, the natural
   entry point for a new chat.
2. Or \`session_list({ active_only: true })\` to scan recent memos.
3. At end of a session that locks decisions or ships work,
   \`session_save({ slug, body_md })\` so the next chat can pick up.

---

## Language coverage

The Standardoc parser layer wires one provider per language. Coverage
status as of this workspace:

| Language | Symbols | Calls / Imports | Doc extraction | Template refs | Call sites *(IR-4-b/c/d)* | Notes |
|---|:---:|:---:|:---:|:---:|:---:|---|
| **Rust** | ✓ | ✓ | ✓ | — | ✓ | \`syn\` AST, full visit, attribute-aware |
| **TS / JS** | ✓ | ✓ | ✓ | ✓ | ✓ | \`swc\` AST, JSX refs in CallVisitor. Top-level \`Stmt::Expr\` and constructor bodies pending. |
| **Vue** | ✓ | ✓ | — | ✓ | ✓ | SFC custom parser; \`<template>\` refs. Inherits TS extractor's call_sites. |
| **Svelte** | ✓ | ✓ | — | ✓ | ✓ | SFC custom parser; template ref attributes. Inherits TS. |
| **Lua** | partial | partial | — | — | ✓ | Annotation-driven; gaps known. \`require()\` routes to IMPORTS edge, not call_site. |

Files in languages outside this table appear in the file graph
(paths, hashes) but contribute no semantic symbols. Use
\`list_symbols({ module: "..." })\` against a known file path to
verify what the indexer captured for any given file.

---

## Key concepts

- **FQDN** — \`<package>::<module>::<name>\` (Rust + TS unified).
  Stable identifier across the workspace.
- **Edge kinds** — \`CALLS\`, \`IMPORTS\`, \`EXTENDS\`, \`IMPLEMENTS\`,
  \`REFERENCES\`, \`DEFINES\`, \`USES_TYPE\`, \`EXPOSES_API\`.
- **Edge attributes** — every edge carries an optional JSON
  \`attributes\` field with structured metadata. Today it's populated
  for template refs in Vue / Svelte / JSX (component name, prop
  bindings, slot kind) and for tier markers like \`via-builtin\`,
  \`via-alias[-mutable]\`, \`value-read\`, \`macro\`. Consult the edge
  payload when an \`Unresolved\` bare name feels incomplete — the
  attributes often disambiguate which component is meant.
- **Resolved vs Unresolved targets** — an edge target may be:
  - \`Resolved { fqdn }\` — known, points to an indexed symbol.
  - \`Unresolved { name }\` — name only, external or unindexed.
    Candidate for \`resolve_external\` if it looks like a dependency.
  - \`UnresolvedBridge { bridge, name }\` — cross-language jump (e.g.
    Rust ↔ TS via a Tauri command). The \`bridge\` slug is locked to
    the IR-1 1.0 vocabulary (\`tauri\`, \`wasm-bindgen\`, \`napi\`, \`prisma\`,
    \`kafka\`, etc.) or \`custom:<tag>\`.

  Don't blindly follow Unresolved targets — they leave the indexed
  graph unless you \`resolve_external\` them.
- **Call sites** *(IR-4-b/c/d/f)* — every call expression the
  extractor sees is recorded in the \`call_sites\` table alongside its
  \`Calls\` edge. The two layers are orthogonal: the symbol graph
  drops Drop-tier builtins (\`Vec::new()\`, etc.) but call_sites are
  observational and capture them regardless. Query via
  \`find_call_sites\`. Each record carries \`from_fqdn\`, \`callee_text\`,
  \`args[]\` (with \`is_string_literal\` classification), \`receiver_chain[]\`
  (member-access prefix segments), and the source site.
- **Workspace scope** — every symbol row carries a \`workspace_id\`
  tagging it as primary or as belonging to a linked peer (\`Stage
  3b-7-b\`). Discovery tools (\`find_symbol\`,
  \`find_symbols_by_pattern\`, \`list_symbols\`) default-narrow to
  primary; pass \`workspace_id: <uuid>\` to query a peer explicitly.

## Indexing state

If a tool returns \`"Workspace indexing in progress..."\`, cold start is
running (typically 5-15s on first activation). Wait briefly and retry.
After cold start, the watcher keeps the index live — including
manifest changes (Stage 3d-5) which re-run project discovery
automatically when a \`Cargo.toml\`, \`package.json\`,
\`pnpm-workspace.yaml\`, \`*.sxb\`, etc. is edited.

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
