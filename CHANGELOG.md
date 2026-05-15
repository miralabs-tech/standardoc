# Changelog

Per-release notes live alongside the published release tags — see the release
page for any tagged version to find what shipped in that version. This file
is intentionally minimal and only buffers in-flight work between releases.

## [Unreleased]

Targeting `v1.0.0-beta.3` — see [.important/en/TODO-LIST.md](.important/en/TODO-LIST.md) for the roadmap.

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
