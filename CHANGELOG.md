# Changelog

Per-release notes live alongside the published release tags — see the release
page for any tagged version to find what shipped in that version. This file
is intentionally minimal and only buffers in-flight work between releases.

## [Unreleased]

## [1.0.0-beta.3]

The planned axes were doc rendering + visual navigation + CLI autonomy. In
practice, dogfood pulled the release toward **multi-workspace graphs,
interactive visualization, a native C provider, and a deep edge-resolution
overhaul** — while the RAG layer and the session DB were cut entirely. (Doc
rendering slips forward.)

- **Cross-workspace / multi-root graph**: symbols tagged by `workspace_id`
  (UNIQUE relaxed to `(workspace_id, fqdn)`); link / unlink / refresh peer
  workspaces with live add/remove + scoped watcher dispatch; AOT
  `ModuleLookup` persistence + cross-workspace import resolver; `projects`
  and `workspace_catalog` tables with cold-start project discovery and
  manifest-driven re-discovery. New MCP tools: `link_workspace`,
  `unlink_workspace`, `resolve_cross_workspace`, `list_linked_workspaces`,
  `set_link_direction`, `refresh_peer`, `module_lookup`, `list_projects`,
  `project_for_file`.
- **`standardoc.sxd` workspace config** (v0.1): explicit `project` / `group`
  / `ignore` / `mcp` / `viz` blocks **replace** mechanical detection and
  absorb `.stdignore`; loader with `ScanFilters` back-compat. LSP-side
  schema-aware live diagnostics + hovers; VSCode TextMate grammar +
  language-configuration; `list_groups` MCP endpoint; `sxd-preview` CLI.
- **Interactive graph visualization** (`standardoc-graph-viz`): WASM crate +
  web-component shell (overview / focus-graph / explorer / symbol-details /
  search panels); real 3D overview (orbit camera, system topology, project
  clusters); bucketed focus layout; hide-tests / kind / visibility filters.
  Hosted in a VSCode webview ("Open Graph Viz") and a standalone playground,
  driven entirely through MCP.
- **Native C provider + FFI**: `.c` / `.h` indexing with cross-file `.h`↔`.c`
  join, struct/union fields, CALLS edges; `extern "C"` / `bun:ffi` /
  `Deno.dlopen` / NAPI FFI taggers; CMake layout detection; Lua C-API export
  tagging.
- **Multi-workspace proxy**: `standardoc-mcp-proxy` consolidated into
  `standardoc proxy` — singleton with deterministic `/ws/<id>/mcp` routing,
  runtime register / list / unregister admin endpoints, supervisor
  auto-spawn, and a long-lived forwarder that survives daemon restarts.
- **Edge-resolution overhaul**: AOT `ModuleLookup` pre-pass + `BuiltinRegistry`
  (Drop / Attribute / Edge tiers) across Rust / TS / C / Lua; trait dispatch
  (`#[derive]` IMPLEMENTS + Into / Iterator / ToString builtin fallback);
  global return-type registry; `receiver_type` on CALLS edges; local type
  env, struct-field tables, closure-arg + parametric inference, pattern-
  binding resolution; References via `Expr::Path`. `RawCallSite` persisted to
  a `call_sites` table + `find_call_sites` MCP tool.
- **IR evolution**: `Kind::Function` → **`Kind::Callable`** (rename),
  `DeclKind` + `decl_kind`, `implements_trait` / `receiver_type` /
  `flags: Vec<String>` on `RawSymbol`, `EntryPointKind` classification;
  `BridgeKind` 1.0 vocabulary locked with validate-on-insert; `Substrate`
  widened to 6 variants; dead `Defines` / `ExposesApi` edge variants removed.
  New `standardoc-sourcemap` crate carrying the v1 preproc↔extractor protocol.
- **MCP surface**: `get_code`, `get_context_summary`, `find_symbol_fqdns`,
  `list_symbol_fqdns`, `fetch_graph`; cursor pagination on `list_symbols`;
  `workspace_id` param + `relative_to` projection; noise reduction (silent
  default depth, blank-line collapse, `exclude_tests`); `language` surfaced in
  `SymbolContext`. HTTP transport hardened (stateless + `json_response`,
  persisted session store for transparent reconnect, port reuse across
  restarts).
- **CLI autonomy**: `standardoc init` (agent skill + MCP-first hooks +
  `AGENTS.md` + `.mcp.json`), `standardoc mcp --connect` (stdio↔http bridge),
  `standardoc self-update`.
- **Removed**: the RAG layer (`standardoc-rag`), the session DB, and the
  usage-stats / token-savings surface — all shipped in beta.2, all cut here.
  Standardoc bets on resolved structure over vector similarity, and stays a
  code graph rather than an agent-memory store.
- **Security / deps**: `rmcp` 0.16 → 1.7 (fixes GHSA-89vp-x53w-74fx — DNS
  rebinding in the streamable-HTTP server transport); `bincode` 2.0,
  `swc_core` 68, `reqwest` 0.13, `r2d2_sqlite` 0.34, `standarbuild-detect` 0.3.
- **Docs**: `.important/{en,fr}` corpus rethought — value-first README,
  trimmed comparison / quickstart, storytelling de-narrated; roadmap
  reconciled to the beta.3 surface.
- **Internals / CI**: ~17k LOC of inline tests extracted to sibling files;
  clippy 85 → 0 workspace warnings; `handler.rs` split (−41%); toolchain
  pinned to 1.95; multiple SQLite schema migrations + a v0 baseline reboot.

## [1.0.0-beta.2]

Second beta on top of the beta.1 foundation. Focus: prose/symbol retrieval,
agent-facing ergonomics, daemon resilience, and decoupling the native binary
from the VSIX.

- **RAG layer** (`standardoc-rag` crate): chunker, candle BGE-small embedder
  (~130 MB, local-first), SQLite store, prose↔symbol linker, hybrid scoring.
  Exposed via MCP `fetch_chunks` and `chunk_refs` in `get_context` responses;
  CLI `--rag --embedder mock|candle` flags; chunks re-linked on graph-symbol
  changes so anchors stay stable across revisions.
- **Binary ↔ extension decoupling**: VSIX no longer bundles `standardoc[.exe]`.
  Ext transitions into `awaiting_binary` on first activation, fetches the
  pinned `version.json` manifest, SHA256-verifies and installs the archive
  under `<globalStorageUri>/bin/<rust-target-triple>/`. `binary-resolver.ts`
  ordering is `settings → globalStorage → PATH`. `standardoc.binaryPath`
  setting kept as override for dev / pre-release pinning.
- **Session sync pipeline**: `session_save` / `session_list` / `session_get`
  MCP tools + `session_sync_in` / `session_sync_out` for filesystem
  round-trips with frontmatter fidelity. New `SessionKind::Lock` variant
  (ADR-equivalent) alongside `Session`, `Feedback`, `Profile`.
- **Usage stats**: SQLite schema v7 `usage_stats` table; `usage_stats` MCP
  tool; `Standardoc.showTokenSavings` / `Standardoc.resetTokenSavings`
  commands and status-bar menu entries. Baseline-reset tooling for clean
  measurement windows.
- **MCP-first guardrail**: PreToolUse hook denies `Bash|Read|Grep|Glob`
  until a standardoc MCP tool has been called in the session;
  SessionStart hook wipes the sentinel each chat.
- **HTTP/SSE MCP transport**: streamable-http multi-client transport;
  `standardoc mcp --http <port>` (`--http 0` = ephemeral); endpoint URL
  written to `.standardoc/mcp.endpoint`; silent port fallback on
  `EADDRINUSE`; parent death-watch via stdin EOF.
- **`.stdignore` language contribution**: VSCode syntax + hover preview.
- **MCP boundary polish**: `get_context` `routing_hint` nudging the 3-phase
  protocol; `get_body` `strip_attrs` / `signature_only` knobs;
  `find_symbol` FTS5 query sanitization + `did_you_mean` enrichment;
  OOP-style FQDN separator normalization; `current_revision` exposes
  daemon capabilities (`rag.enabled`, `rag.embedder`, `watcher.active`,
  `indexing.ready`).
- **Lua native provider** (`full_moon` with luau/luajit/cfxlua + emmylua
  documentation extraction).
- **Vue + Svelte SFCs**: TS-side single-file component support.
- **Daemon resilience**: serialised restart chain + debounced RAG settings
  watcher; readonly daemon no longer races LSP on RAG writes; structured
  `STDOC_FATAL` markers; FatalConfig state distinct from Failed; SQLite
  busy retry on `IndexHandle::open` and `SessionsHandle::open`; r2d2 pool
  lazy-init + retry on transient timeouts; Windows EBUSY/EPERM retry on
  `rag.db` unlink; pid placeholder leak removed from `ready` state.
- **VSCode ext UX**: re-prompt init when `.standardoc/` deleted after
  opt-in; `.mcp.json` rewritten to daemon's actual URL on `ready` (port
  fallback case); RAG commands palette + settings + status-bar entries
  + endpoint race fix; SKILL.md template refreshed for post-beta.1
  surface; cross-OS via binary in PATH (no shell-script adaptation).
- **IR / lang-provider polish**: compact display rendering for type and
  attribute strings; common-prefix dedent + tab indent in `get_body`;
  rust `pub use` phantoms; `impl` skip for non-nominal targets;
  module_path crate-relative; TS visit + SFC support.
- **Repo reorg**: long-form docs moved into `.important/{en,fr}/`
  (mirrored bilingual hub; storytelling/ sub-dir for philosophy / visions
  / retours / notes); root README V4; root SECURITY switcher.
- **CI / chore**: workflow permissions hardened (5 code-scanning autofix
  MRs); macOS cache fix (`cache-bin: false` + `v1-rust` prefix-key);
  toolchain action swap (`dtolnay/rust-toolchain`); fmt/clippy/doc
  workspace gates green on rustfmt 1.8.0 / toolchain 1.91;
  `bundle-binary.ts` script removed; `dist/bin/**` blacklisted from
  VSIX.

## [1.0.0-beta.1]

First beta, major rewrite from the v0 prototype:

- AST-direct Rust + TS providers (`syn`, `swc`)
- SQLite + FTS5 graph storage (zero-duplication external content)
- 2 MCP tools day-1 (`find_symbol`, `get_context`)
- Single binary `standardoc` with sub-commands (LSP, MCP, index, rescan, watch, query, purge-excluded)
- LSP daemon = primary writer, MCP daemons = read-only
- VSCode extension with daemon supervisor + init opt-in flow
- AI agent skill generation for Claude Code
- MDX/React rendering layer (npm package, replaces the v0 DSL templating) — held back post-beta.2
- Virtual annotations enrichments, cross-language bridge plug-ins — held back post-beta.1
