# CLI reference

📖 English · [Français](cli-reference.fr.md)

Standardoc ships two binaries :

- **[`standardoc`](#standardoc-cli)** — one-shot CLI for scan / transform / emit / validate / materialize
- **[`standardoc-server`](#standardoc-server)** — long-running daemon with four transports (LSP / MCP / Web / static export)

---

## `standardoc` — CLI

```
standardoc <command> [args...]
```

Global behaviors :
- All commands take a workspace root as their first positional argument.
- The workspace is scanned with the four built-in providers (Rust / TS / Python / Lua tree-sitter) plus any dynamic provider declared in `.standardoc/languages/*.json`.
- Configuration is read from `.standardoc.json` at the workspace root if present; otherwise defaults apply.
- Output goes to **stdout**. Status, errors and counters go to **stderr** so you can pipe stdout cleanly.

### `scan`

`standardoc scan <path>`

Walk `<path>` and emit canonical [`DocBlock`](../crates/standardoc-core/src/model/)
entries as JSON, one block per record.

Useful for : piping into `jq`, building external tooling, debugging discovery,
snapshot diffs in CI.

**Exit codes** :
- `0` — success
- `1` — pipeline error (unreadable path, parse failure)
- `2` — missing required argument

**Example** :
```sh
standardoc scan ./my-project | jq '.[] | {key, kind: .symbol.kind}'
```

### `transform`

`standardoc transform <path> <template.md>`

Scan `<path>`, then render `<template.md>` against the resulting index. The
template uses the standardoc DSL (`{{ @doc.KEY:tag }}`,
`{{ each x in @docs.module(...) }}`, `{{ if ... }}`, …). Result printed to stdout.

**Exit codes** :
- `0` — render OK
- `1` — pipeline or render error
- `2` — missing argument

**Example** :
```sh
standardoc transform ./my-project ./docs-src/api.md > ./public/api.md
```

### `emit`

`standardoc emit <format> <path> [--name <project>] [--tagline <line>] [--link-base <url>]`

Generate one of three agent-oriented documentation standards from a workspace scan.

**Formats** :
- `llms` (alias `llms.txt`) — [Jeremy Howard's `llms.txt`](https://llmstxt.org/) summary index
- `llms-full` (alias `llms-full.txt`) — `llms-full.txt` long-form variant
- `skill` (alias `skill.md`) — Claude Code [`SKILL.md`](https://docs.anthropic.com/en/docs/claude-code/skills) format

**Options** :
- `--name <project>` — overrides the auto-detected project name (default : the workspace root directory name)
- `--tagline <line>` — short description embedded in the output header
- `--link-base <url>` — base URL prefix for source links (e.g. `https://github.com/owner/repo/blob/main`)

Output goes to stdout. Redirect with `>` to write a file.

**Example** :
```sh
standardoc emit llms ./my-project \
  --name "My Project" \
  --tagline "REST API for X" \
  --link-base "https://github.com/owner/repo/blob/main" \
  > llms.txt
```

### `validate`

`standardoc validate <path>`

Run the full validator suite over a workspace, print one diagnostic per line in the
format `<severity> [STD###] <path>:<line>: <message>`. A summary count is printed
to stderr.

**Severities** : `error`, `warning`, `info`, `hint` — see the
[validator rules table in README.md](../README.md#validator) for the full list.

**Exit codes** :
- `0` — no error-severity diagnostic found (warnings/info/hints don't fail)
- `1` — at least one `error` diagnostic
- `2` — missing argument

**Example** :
```sh
standardoc validate ./my-project
# error [STD001] src/lib.rs:42: duplicate DocKey "foo.bar"
# warning [STD006] src/lib.rs:10: public symbol with no @doc annotation
# 1 error(s), 1 warning(s), 0 info, 0 hint(s)
```

CI integration : run `standardoc validate .` as a step; non-zero exit blocks the merge.

### `materialize`

`standardoc materialize <path> [--apply] [--confidence low|medium|high]`

Promote virtual annotations (synthesized by the virtual-annotation pass on
`Inferred` blocks) into real source-level `///` doc-comments. Defaults to a dry-run
that prints exactly what would be inserted, file-by-file ; pass `--apply` to actually
edit the source.

**Options** :
- `--apply` — perform the edits. Without this flag, only a dry-run report is printed.
- `--confidence <tier>` — minimum confidence required for a virtual annotation to be
  eligible. `low` (everything), `medium` (default), `high` (only the most confident
  templates : constructors, trait impls, predicates, etc.).

The output respects the language's preferred doc-comment syntax (`///` for Rust,
`---` for Lua, `/** … */` for TS/JS) and preserves the indentation of the symbol it
documents. Python is intentionally unsupported in this MVP — docstrings live inside
the function body, which needs different placement logic.

**Exit codes** :
- `0` — dry-run printed, or `--apply` succeeded
- `1` — pipeline error or write failure
- `2` — bad argument

**Example** :
```sh
# Preview what would be added on the public API
standardoc materialize ./my-project --confidence high

# Actually write
standardoc materialize ./my-project --confidence high --apply
```

### `--help`, `-h`

standardoc --help, `standardoc -h`

Print the command list with brief usage. Always exits `0`.

---

## `standardoc-server` — daemon

```
standardoc-server <transport> --workspace <path> [transport-specific args]
```

A single binary, four mutually exclusive transports — pick exactly one.

### `--mcp`

`standardoc-server --mcp --workspace <path>`

Speak the [Model Context Protocol](https://modelcontextprotocol.io/) over
**stdio** (JSON-RPC 2.0). Use this from `.mcp.json` to expose the workspace
to AI agents (Claude Code, Cursor, Zed, Continue, …). See the
[MCP reference](mcp-reference.md) for the full list of tools available.

The daemon scans once at boot, watches the workspace for changes, and pushes
notifications when the index changes. State stays alive for the lifetime of
the host process.

### `--lsp`

`standardoc-server --lsp --workspace <path>`

Speak [LSP](https://microsoft.github.io/language-server-protocol/) over **stdio**
for editors (VSCode, Helix, Neovim, Zed, …). Capabilities :

- Completion on `@`, `{`, `.`, `:` triggers
- Hover, goto-definition (DSL → source), references (source → `.md`)
- Document / workspace symbols, code actions
- **Rename** that propagates `DocKey` changes into all `.md` consumers
- Formatting, push diagnostics on every rescan
- 10 diagnostic codes (STD001-STD008 + STD012-STD013; STD009-STD011 reserved)

### `--web --port <N>`

`standardoc-server --web --port <N> --workspace <path>`

Serve a REST + SSE HTTP API on the given port. Endpoints:

- `GET /api/health` — `{ "ok": true, "revision": N }`
- `GET /api/index` — full index snapshot
- `GET /api/doc/{key}` — single block detail
- `GET /api/search?q=...` — substring + fuzzy fallback search
- `GET /api/dsl-reference` — markdown DSL reference (same content as MCP `get_dsl_reference`)
- `GET /api/config` — resolved configuration
- `GET /api/pages` — list narrative pages
- `GET /api/page/{*slug}` — full content of one page (also `PUT`, `PATCH`, `DELETE`)
- `GET /api/events` — Server-Sent Events stream (`index_changed`, `diagnostics`, …)
- `GET /api/syntax.css` — syntect-generated CSS for code highlighting
- Fallback `/*` — embedded SPA (only when binary is built with
  `--features standardoc-web/embedded-frontend`, i.e. Standardoc Pro),
  otherwise a placeholder

**CORS** is wide-open by default (`allow_origin: any`) for local dev and
self-hosted SPAs. Tighten in a reverse-proxy if you expose this beyond
`localhost`.

### `--export --out <dir>`

`standardoc-server --export --workspace <path> --out <dir>`

One-shot static export. Writes `static-data.json` (full index snapshot, all
blocks, pre-rendered pages, resolved source-link config) to `<dir>`. If the
binary was built with `embedded-frontend`, also writes the bundled SPA as a
CDN-deployable site; otherwise it's data-only and consumable by any external
SSG (Astro, Vitepress, Hugo, custom).

### `--workspace <path>` *(required)*

Absolute or relative path to the workspace root. Standardoc treats this as the indexing scope and also looks here for `.standardoc.json` and `.standardoc/languages/*.json`.

### Exit codes

- `0` — clean shutdown
- `1` — runtime error (bind failure, scan error, etc.)
- `2` — argument error

---

## Environment variables

Standardoc itself reads only one environment variable directly :

- `RUST_LOG` — controls log level for `tracing` (e.g. `RUST_LOG=standardoc=debug`)

Install scripts (`scripts/install.{sh,ps1}`) honor :

- `STANDARDOC_VERSION` — pin to a specific release (e.g. `v0.1.0`)
- `STANDARDOC_HOME` — install root (default `$HOME/.standardoc` or `$env:USERPROFILE\.standardoc`)
- `STANDARDOC_NO_PATH` — skip the PATH suggestion message

## Configuration file

`.standardoc.json` at the workspace root — fully optional. See the
[Configuration section in README.md](../README.md#configuration-standardocjson)
for the full schema.
