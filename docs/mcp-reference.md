# MCP reference

📖 English · [Français](mcp-reference.fr.md)

Standardoc speaks the [Model Context Protocol](https://modelcontextprotocol.io/) over stdio. This document lists every tool and resource the `standardoc-server --mcp` daemon exposes, derived live from the source annotations — so it never drifts away from what the binary actually serves.

**Setup** — drop a `.mcp.json` at your workspace root :

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "stdio",
      "command": "/absolute/path/to/standardoc-server",
      "args": ["--mcp", "--workspace", "/absolute/path/to/your/project"]
    }
  }
}
```

Then launch any MCP-aware client (Claude Code, Cursor, Zed, Continue, …). The daemon scans once at boot, watches the workspace for changes, and pushes notifications to your client when the index changes.

---

## Read & navigation

Tools that read the index, search by query, evaluate DSL, and extract raw comments.


### ``evaluate_dsl``

Evaluate a single DSL expression (e.g. '@doc.foo:label') against the index. Returns the rendered string.


**Input schema** :

```dsl
{"type":"object","properties":{"expression":{"type":"string"}},"required":["expression"]}
```


### ``get_comments``

Extract all comment nodes from a single source file using tree-sitter. Returns [{line, text}] for every comment in the file (single-line, multi-line, and doc comments). Works for any language with a registered tree-sitter grammar (e.g. Lua, Teal, and any language defined in .standardoc/languages/). Does not require the file to be indexed or annotated — useful for auditing, translation, or exploring undocumented code.


**Input schema** :

```dsl
{"type":"object","properties":{"file":{"type":"string","description":"Path to the source file. Absolute or relative to the workspace root."}},"required":["file"]}
```


### ``get_doc``

Fetch a single DocBlock by its key.


**Input schema** :

```dsl
{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}
```


### ``list_docs``

List all documentable blocks in the workspace. Supports substring filtering on key/label and pagination.


**Input schema** :

```dsl
{"type":"object","properties":{"filter":{"type":"string","description":"Substring matched against key or label."},"limit":{"type":"integer","minimum":1},"offset":{"type":"integer","minimum":0}}}
```


### ``render_markdown``

Render a markdown template containing DSL expressions against the current index.


**Input schema** :

```dsl
{"type":"object","properties":{"content":{"type":"string"}},"required":["content"]}
```


### ``search_docs``

Search the doc index by query. First tries exact substring matching across key, label, and description. If that returns nothing, automatically falls back to token-based fuzzy search across all fields (signature, params, return type, all tags). The response includes `"mode": "exact" | "fuzzy"` so callers know which path was taken.


**Input schema** :

```dsl
{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1}},"required":["query"]}
```


---

## Cross-reference

Type-graph navigation — find usages, implementations, and parameter/return-type matches by short name.


### ``find_implementations``

List every type that implements the given trait/interface (Rust `impl Trait for X`, TS `class X implements I`). Returns the implementor types deduplicated, with the keys of their associated trait methods.


**Input schema** :

```dsl
{"type":"object","properties":{"trait_name":{"type":"string"}},"required":["trait_name"]}
```


### ``find_usages``

List every block that references the given short name through its outgoing references — param types, return types, field types, trait implementations. Match is on **short name** (last FQN segment), so an ambiguous name returns all candidates; use the disambiguation filters below to scope the results.

- `kind` restricts the relation ('param-type', 'return-type', 'field-type', 'implements', 'extends', 'generic-arg', 'call', 'other').
- `from_path_prefix` restricts to the subtree of the referrer file (e.g. 'crates/standardoc-core/' to see only usages in that crate).
- `from_key_prefix` restricts to the subtree of the referrer's FQN module (e.g. 'matchigo.parser' to see only usages in that module). More precise than `from_path_prefix` when file paths don't track the FQN hierarchy.


**Input schema** :

```dsl
{"type":"object","properties":{"name":{"type":"string","description":"Short symbol name to look up (e.g. 'ParseError')."},"kind":{"type":"string","description":"Optional ref kind to filter on."},"from_path_prefix":{"type":"string","description":"Optional path prefix on the referrer file (relative to workspace root)."},"from_key_prefix":{"type":"string","description":"Optional FQN prefix on the referrer key — matches the key itself or a strict descendant ('foo.bar' matches 'foo.bar' and 'foo.bar.baz' but not 'foo.barber')."}},"required":["name"]}
```


### ``get_type_hierarchy``

Walk the inheritance graph of a type. Returns the ancestors (transitively `extends`-ed types) and descendants (types that `extends` the given type), plus the interfaces it implements (`implements_outgoing`) and the types that implement it (`implementors_incoming`). Match is on **short name**.


**Input schema** :

```dsl
{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}
```


### ``search_by_param_type``

List functions/methods whose parameter list contains the given short name. Use this to answer 'who accepts a Foo?'. Match is on **short name** (last FQN segment). Optional `from_path_prefix` / `from_key_prefix` scope the results — same semantics as `find_usages`.


**Input schema** :

```dsl
{"type":"object","properties":{"name":{"type":"string"},"from_path_prefix":{"type":"string"},"from_key_prefix":{"type":"string"}},"required":["name"]}
```


### ``search_by_return_type``

List functions/methods whose return type contains the given short name. Use this to answer 'where does X come from?' or 'what produces a Foo?'. Match is on **short name** (last FQN segment). Optional `from_path_prefix` / `from_key_prefix` scope the results — same semantics as `find_usages`.


**Input schema** :

```dsl
{"type":"object","properties":{"name":{"type":"string"},"from_path_prefix":{"type":"string"},"from_key_prefix":{"type":"string"}},"required":["name"]}
```


---

## LSP semantic exposure

These mirror behaviors the LSP gives editors, exposed as MCP tools so agents (which don't speak LSP) can use them.


### ``find_references``

Scan narrative `.md` pages for occurrences of `@doc.KEY`. Distinct from `find_usages` (which follows type-graph edges in source): this finds where the key is *mentioned* in documentation pages. Returns [{page, line, context}].


**Input schema** :

```dsl
{"type":"object","properties":{"key":{"type":"string","description":"Canonical doc key to search for (also matches short-name occurrences)."}},"required":["key"]}
```


### ``get_definition``

Return the source file location (path + line) for a doc block key. Lighter than `get_doc` when you only need to navigate to the definition, not read the full block.


**Input schema** :

```dsl
{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}
```


### ``get_hover``

Return a formatted Markdown summary for a doc block (label, signature, description, source location) — the same text the LSP shows on hover. Use this for a quick human-readable snapshot of a symbol without parsing the full `get_doc` JSON.


**Input schema** :

```dsl
{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}
```


### ``resolve_symbol``

Resolve a short symbol name (label or FQN suffix) to one or more doc blocks. Prefers exact label matches, then FQN-suffix matches. Use this before `get_doc` when you only know the short name (e.g. 'LanguageProvider') and need the canonical key. Optional `path_prefix` scopes results to a subtree.


**Input schema** :

```dsl
{"type":"object","properties":{"name":{"type":"string","description":"Short symbol name or label to resolve."},"path_prefix":{"type":"string","description":"Optional path prefix to restrict results (relative to workspace root)."}},"required":["name"]}
```


---

## Quality & validation

Validator output, coverage stats, and tools to surface or audit documentation gaps.


### ``coverage_report``

Summarize documentation coverage: total blocks, how many are inferred/annotated/hybrid, distribution by kind.


### ``find_undocumented``

List blocks that were auto-inferred from AST but have no `@doc` annotation — candidates for documentation.


### ``list_collisions``

List key collisions detected during the last scan. Each entry shows the key, the winning location, and every location that was silently dropped. Surfaces what would otherwise be invisible data loss.


### ``list_diagnostics``

Run all validator rules against the current index and return diagnostics. Codes shipped: STD001 (dup key), STD002 (malformed @tag — missing key/name/type), STD003 (param missing description), STD004 (DSL ref to unknown DocKey in narrative pages), STD005 (no block description), STD006 (public symbol without @doc), STD007 (DSL syntax error in narrative pages), STD008 (param name not in signature), STD012 (param type mismatch). Filter by `severity` ('error'/'warning'/'info'/'hint') or `code` ('STD001', etc.).


**Input schema** :

```dsl
{"type":"object","properties":{"severity":{"type":"string","description":"Optional severity filter."},"code":{"type":"string","description":"Optional diagnostic code filter (e.g. 'STD006')."}}}
```


### ``validate_doc_syntax``

Parse a string as a `@doc` annotation body and report whether it is syntactically well-formed. Used by agents before they write annotations into source.


**Input schema** :

```dsl
{"type":"object","properties":{"content":{"type":"string"}},"required":["content"]}
```


---

## Agent docs emission

Generate well-known agent-oriented documentation formats (`llms.txt`, `skill.md`, OpenAPI, …) from your live workspace scan.


### ``emit_llms_full``

Generate the `llms-full.txt` file content — same coverage as `llms.txt` but inlined: signatures + descriptions + params + returns in one file. For agents that ingest in bulk rather than follow links.


**Input schema** :

```dsl
{"type":"object","properties":{"name":{"type":"string"},"tagline":{"type":"string"}}}
```


### ``emit_llms_txt``

Generate the `llms.txt` file content for the current index — Jeremy Howard's link-based Markdown index for LLMs. Optional `name` (project), `tagline`, and `link_base` (URL prefix for hyperlinks).


**Input schema** :

```dsl
{"type":"object","properties":{"name":{"type":"string"},"tagline":{"type":"string"},"link_base":{"type":"string"}}}
```


### ``emit_openapi``

Generate an OpenAPI 3.0 spec from blocks tagged with `@route METHOD PATH`. Recognized companion tags: `@param NAME TYPE description` (path param if `{NAME}` is in the route, else query), `@response CODE description`, `@request_body TYPE description`. Blocks without `@route` are ignored — empty `paths` is the expected output for projects that don't expose an HTTP API.


**Input schema** :

```dsl
{"type":"object","properties":{"title":{"type":"string","description":"API title for `info.title`. Default 'API'."},"version":{"type":"string","description":"API version for `info.version`. Default '0.1.0'."},"description":{"type":"string","description":"Optional `info.description`."},"servers":{"type":"array","items":{"type":"string"},"description":"Optional list of server URLs (added as `servers[]`)."}}}
```


### ``emit_skill_md``

Generate a Claude Code-style `skill.md` with YAML front-matter — describes the project's key types, traits (with implementors), and public functions as an acquireable skill for agents.


**Input schema** :

```dsl
{"type":"object","properties":{"name":{"type":"string"},"tagline":{"type":"string"}}}
```


---

## Lifecycle & runtime

Force rescans, fetch the DSL reference, control the watcher.


### ``get_dsl_reference``

Return the Standardoc DSL syntax reference (accessors, block directives, functions). Use this to produce valid templates without guessing.


### ``get_watch_status``

Report the current watcher state: whether a watcher is active, whether it's paused, and the current index revision. Call this after set_watch_paused or when you want to check freshness.


### ``rescan``

Re-scan the workspace from disk. Bumps the index revision.


### ``set_watch_paused``

Pause or resume the filesystem watcher. While paused, FS changes are drained without triggering re-scans — useful during heavy refactors, initial scaffolding, or when you want a frozen snapshot of the index. Call again with false to resume. The auto-pause heuristic also flips this flag when it detects repeated parse errors.


**Input schema** :

```dsl
{"type":"object","properties":{"paused":{"type":"boolean"}},"required":["paused"]}
```


---

## Resources

MCP resources are read-only data exposed under URIs.
### ``standardoc://config``

Resolved `.standardoc.json` (or defaults) currently in use.

**MIME type** : ``application/json``
### ``standardoc://index``

Every DocBlock discovered in the workspace, as a JSON array.

**MIME type** : ``application/json``
### ``standardoc://schema/dsl``

Standardoc DSL grammar reference — feed this to an agent so it writes valid templates.

**MIME type** : ``text/markdown``
### ``standardoc://schema/tags``

Built-in plus user tag schemas (name → `[fields]`, `[required fields]`).

**MIME type** : ``application/json``

Subscribe to a resource (per MCP spec) to receive updates when the underlying data changes.

---

## Notifications

The daemon pushes JSON-RPC notifications when state changes. Hosts that subscribe see :

- `notifications/standardoc/index_changed` — `{ revision, added, removed }` (DocKey lists, no full blocks — refetch via `get_doc` if needed) after every successful rescan
- `notifications/standardoc/diagnostics` — `{ path, diagnostics: [...] }` when validator output changes for a file
- `notifications/standardoc/config_reloaded` — `{ config }` after `.standardoc.json` is edited

These let agents react to file changes without polling — same delivery mechanism the LSP uses for `publishDiagnostics`.
