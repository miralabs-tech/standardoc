# Roadmap

📖 English · [Français](../fr/TODO-LIST.md)

Source of truth for what's planned, what's shipping, and what's deliberately
deferred. [`CHANGELOG.md`](../../CHANGELOG.md) tracks what actually shipped per
release; this file tracks intent.

Box convention: `[x]` shipped · `[ ]` planned · `~~struck~~` killed or
deferred to a later milestone.

---

## Versioning policy

Standardoc has two release deliverables that evolve at different cadences:

- **Core** — distributed as pre-built binaries (single binary `standardoc`).
  Tag-driven: pushing `vX.Y.Z` triggers `release.yml` (cross-platform pre-built
  binaries + `version.json` manifest + GitHub Release). Source builds via
  `cargo install --git https://github.com/miralabs-tech/standardoc`.
- **Extension** — published to the VSCode Marketplace + Open VSX. Manual
  trigger via `release-ext.yml` (workflow_dispatch with `version` +
  `pre_release` inputs). Decoupled from tag push.

### Cadence

| Phase                    | Policy                                                                                          |
| ------------------------ | ----------------------------------------------------------------------------------------------- |
| `1.0.0-betaN` / `-rcN`   | **Lockstep**. One tag drives both at the same version. Core moves fast, ext is tightly coupled. |
| `1.0.0` and beyond       | **Independence by default.** Each component bumps on its own pace.                              |

### Independence rules (post-`1.0.0`)

- **MAJOR bump** on core OR ext → mandatory resync. Both bump together.
  Reflects a breaking change in the IPC protocol or a contract change that
  invalidates older counterparts.
- **MINOR bump** on either → independent. New tools or new capabilities on
  the core can land without forcing the ext to update; ext can iterate UI/UX
  without bumping the core.
- **PATCH bump** on either → fully independent.

### `version.json` manifest

Each core release attaches a `version.json` to the GitHub Release artifacts:

```json
{
  "core_version": "1.0.0-beta.1",
  "ext_version": "1.0.0-beta.1",
  "protocol_version": 1,
  "min_compat": { "core": "1.0.0-beta.1", "ext": "1.0.0-beta.1" },
  "released_at": "2026-05-XX",
  "binaries": { "x86_64-unknown-linux-gnu": "https://.../standardoc-...tar.gz", ... },
  "checksums_sha256": { "x86_64-unknown-linux-gnu": "abc123...", ... }
}
```

Stable URL: `https://github.com/miralabs-tech/standardoc/releases/latest/download/version.json`.

The future ext version selector consumes this file to populate the
`standardoc.coreVersion` setting and download matching `standardoc` binaries
on demand.

### Protocol version

`protocol_version` (currently `1`) is decoupled from semver. It tracks the
IPC contract (MCP tool signatures, LSP custom methods) and bumps only on
real wire-format breaks. Ext checks the `protocol_version` field in the
`version.json` manifest at boot; mismatch triggers a warning toast.

---

## v1.0.0-beta.1 — Foundation

**Theme**: AST-direct semantic graph + MCP/LSP surface + VSCode extension.
Rust + TypeScript only. Two MCP tools. Local-only.

### Shipped

#### Core data layer
- [x] AST-direct providers (Rust via `syn`, TypeScript via `swc`)
- [x] Canonical IR (FQDN-keyed symbols, typed edges, `ResolvedOrUnresolved` variant)
- [x] SQLite + FTS5 graph storage (zero-duplication external content)
- [x] BLAKE3 double-level invalidation
- [x] FQDN unification across Rust + TS (`<package>::<module>::<name>`)
- [x] Typed edges day-1: `CALLS`, `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`, `USES_TYPE`

#### Pipeline
- [x] Cold-start eager full-workspace index
- [x] Live file watcher with debounced auto-rescan (`notify` + `notify-debouncer-full`)
- [x] `.stdignore` (gitignore syntax) + auto-seed at workspace root
- [x] Hot-reload of `.stdignore` (diff/swap/warn/reindex)
- [x] `purge-excluded` sub-command for post-`.stdignore`-edit cleanup
- [x] Pause / resume index handle

#### CLI / Daemons
- [x] Single binary `standardoc` with sub-commands (`lsp`, `mcp`, `index`, `rescan`, `watch`, `query`, `purge-excluded`)
- [x] LSP daemon = primary writer (acquires fs lock via `fs4`)
- [x] MCP daemon `--readonly` (`SQLITE_OPEN_READ_ONLY`, skip fs lock)
- [x] Multiple `--readonly` MCP clients can attach concurrently
- [x] LSP: hover, document/workspace symbols, navigation, $/progress on cold start
- [x] 2 MCP tools: `find_symbol(query, limit?)`, `get_context(fqdn, depth=1|2)`

#### VSCode extension
- [x] Daemon supervisor (LSP + MCP, parallel spawn, `Promise.allSettled` rollback)
- [x] Backoff state machine for daemon restarts
- [x] Status bar item
- [x] Init opt-in flow (4-button notification, per-workspace + global memento)
- [x] `.mcp.json` cross-client merge (5 actions discriminated, preserves user fields)
- [x] AI agent skill generation (`.claude/skills/standardoc/SKILL.md`)
- [x] MCP server provider for Copilot Chat / Claude Code in VSCode
- [x] Command palette: Find symbol, Get context, Daemon Stop/Start/Restart, Init, Refresh `.mcp.json`, Regenerate skill, Reset global init prompt, Purge excluded

#### Distribution & infra
- [x] Renovate config (conservative, weekly, group minor/patch, automerge GHA only)
- [x] CI: fmt + clippy + test cross-OS + docs + ext (bun test/tsc/build)
- [x] Labels sync workflow

### Released ✓

- [x] `Cargo.toml` workspace `publish` flags audit
- [x] Path-only deps verified to have `version` field (aligned to `1.0.0-beta.1`)
- [x] `bridge-sdk` aligned to `version.workspace = true`
- [x] `cargo publish --dry-run` validated on leaf crates
- [x] ~~First publish chain to crates.io~~ — dropped; distribution = GitHub Release pre-built binaries + `cargo install --git` (crates.io publish deferred, no firm commitment)
- [x] Push `v1.0.0-beta.1` tag → `release.yml` (cross-platform binaries + `version.json` + GitHub Release)
- [x] Trigger `release-ext.yml` workflow_dispatch (`version=1.0.0`, `pre_release=true`)
- [ ] Smoke F5 full E2E re-test post init opt-in flow + skill gen
- [ ] Public roadmap announcement (link this file from README + GitHub Discussions)

---

## v0.x.x cycle — between beta.1 and v1.0.0

**Theme**: dogfood, fix, harden. Open to first additional language providers
once user feedback validates the foundation.

- [ ] First post-Rust+TS language provider (Python via `rustpython-parser` or tree-sitter)
- [ ] Cross-folder fqdn collision schema fix (`module_path` UNIQUE composite)
- [ ] mtime-based incremental skip optimization
- [ ] Public roadmap on GitHub Discussions / project board
- [ ] `SECURITY.md` policy
- [ ] Code of Conduct (Contributor Covenant) formalization

---

## v1.0.0-beta.2 — Hardening + MCP surface refinement

**Theme**: prove the foundation under real agent workloads. The 2-tool day-1
MCP surface of beta.1 grows into a broader agent toolkit; HTTP/SSE transport
lands; lang coverage triples (Lua, Vue, Svelte added; Rust + TS hardened);
daemon resilience handles real-world process orchestration. No new public
crates or npm packages — those land in beta.3.

> **Re-scoped in beta.3.** beta.2 also shipped a RAG prose-retrieval layer,
> a session-handoff DB, and a usage-stats / token-savings surface. beta.3
> removed or extracted all three — RAG (structural resolution beats vector
> similarity), sessions (moved to a sibling session-store tool), usage-stats
> (dropped). They're struck below and recorded under
> [Deferred / killed](#deferred--killed).

### Shipped

#### Core data layer
- [x] Schema v6: persisted workspace revision, secondary R/W handle, edge confidence column
- [x] ~~`usage_stats` table (schema v2) + `log_usage` query API~~ — removed in beta.3
- [x] Compact display rendering for type / attribute strings (Rust `to_token_stream`-derived plus generic neutralizer)

#### MCP tool surface — expansion from the 2 day-1 tools
- [x] **Symbol discovery** — `find_symbol` (FTS5 + did_you_mean fallback), `find_symbols_by_pattern` (GLOB), `find_similar_symbols` (strsim), `list_symbols` (filter-only)
- [x] **Context** — `get_context` with `depth=1|2` semantics; `routing_hint` nudges the **depth=1 → depth=2 pacing** specifically (fires when depth=2 is called on a fqdn without a recent depth=1 within 5 min, silent otherwise)
- [x] **Body** — `get_body` with `max_lines`, `strip_attrs`, `signature_only` knobs and compact common-prefix-dedent + tab-indent output
- [x] **Externals** — `resolve_external` for cross-language external lookups
- [x] **Capabilities & freshness** — `current_revision` exposes `{watcher.active, indexing.ready}`; `check_stale` for cached-fqdn invalidation
- [x] ~~**RAG** — `fetch_chunks`~~ · ~~**Telemetry** — `usage_stats` tool~~ · ~~**Sessions** — `session_save` / `session_list` / `session_get` / `session_sync_in` / `session_sync_out`~~ — removed or extracted in beta.3
- [x] FTS5 query sanitization (handles snake_case, camelCase, partial tokens, did_you_mean strsim fallback at threshold 0.6)
- [x] OOP-style FQDN normalisation at MCP boundary (`Class.method` → `Class::method`)

#### HTTP/SSE MCP transport
- [x] Streamable-http transport (multi-client, decoupled from stdio child-spawn)
- [x] `standardoc mcp --http <port>` CLI flag; `--http 0` lets kernel pick ephemeral port
- [x] Endpoint URL written to `.standardoc/mcp.endpoint` for client discovery
- [x] Port auto-fallback on `EADDRINUSE` (no fatal marker, warning log only)
- [x] Parent death-watch via stdin EOF (TTY-guarded) — eliminates orphan workspace locks on supervisor crash
- [x] Boot binary sweep (detects orphan `standardoc.exe` processes from prior runs)
- [x] Boot lockfile invalidation sweep (recovers from stale fs4 locks)
- [x] `STDOC_FATAL: <code> <key>=<value>` marker protocol for supervisor-side fatal-config recognition

#### RAG (prose retrieval) layer — ~~removed in beta.3~~
- [x] ~~`standardoc-rag` crate (chunker, embedder, store, linker, score); convention-prose discovery; BGE-small-en-v1.5 via Candle; BLAKE3 chunk invalidation; `relink_watcher` re-anchoring on graph changes; LSP-driven cold-start~~ — shipped in beta.2, **removed in beta.3**: Standardoc bets on structural resolution, not vector similarity.

#### Session handoff DB — ~~extracted in beta.3~~
- [x] ~~`.standardoc-sessions/sessions.db`; `SessionKind` discriminator (`session` / `feedback` / `profile` / `lock`); bidirectional `.md` ↔ DB sync; `standardoc session {sync-in,sync-out,hook}` CLI~~ — shipped in beta.2, **extracted in beta.3** to a sibling session-store tool; the core no longer carries agent memory.

#### MCP-first guardrail
- [x] `standardoc claude pre-tool-hook --mode {mark,check,reset}` CLI driver
- [x] PreToolUse hook denies `Bash|Read|Grep|Glob` until a standardoc MCP tool has been called
- [x] SessionStart hook wipes the sentinel so each new chat starts strict
- [x] Cross-OS via binary in PATH (no shell-script adaptation, no OS detection needed in TS layer)

#### Externals (cargo / npm / luarocks)
- [x] Lazy on-demand external resolvers — no pre-walk of vendored deps at index time
- [x] Walk-down manifest discovery (`Cargo.toml`, `package.json`, `*.rockspec`)
- [x] `resolve_external` MCP tool surfaces resolved metadata to agents
- [x] E2E integration test surface

#### Usage stats / token savings — ~~removed in beta.3~~
- [x] ~~per-tool read-handler logging into `usage_stats`; `usage_stats` MCP query tool; `standardoc reset-usage` CLI; VSCode token-savings command + status bar; skill-template savings angle~~ — shipped in beta.2, **removed in beta.3**.

#### Language providers
- [x] **Lua native provider** (`full_moon`): functions, locals, module tables (`M = {}`), `require` imports, call edges, emmylua annotation extraction
- [x] **Vue SFC** (`.vue`): extract `<script>` / `<script setup lang="ts">` → `TsProvider`; `<template>` ref edges with attributes (component name, prop bindings, slot kind)
- [x] **Svelte components** (`.svelte`): script-extract pipeline, template ref attributes
- [x] **Rust hardening**: `pub use` phantoms (re-export visibility chain), impl skip on non-nominal types, `module_path` made crate-relative
- [x] **TS visit + SFC unification**: consistent FQDN scheme across `.ts` / `.tsx` / `.vue` / `.svelte`
- [x] Shared `utils` module across providers (FQDN helpers, common walk primitives)
- [x] Edge `attributes` field — structured metadata for template refs

#### Pipeline & storage hardening
- [x] `IndexHandle::open` retry with exponential backoff on transient lock errors (`SQLITE_PROTOCOL`, `database is locked`, `database is busy`, bare r2d2 timeout)
- [x] r2d2 connection pool: lazy init (`min_idle = 0`), 10 s timeout, retry helper cycles fast
- [x] Cleanup pass for unseen files (maintains XOR CHECK constraint after `.stdignore` edits)

#### VSCode extension
- [x] Supervised LSP + MCP HTTP daemons (parallel spawn, `Promise.allSettled` rollback, backoff state machine)
- [x] Init opt-in flow (4-button notification, per-workspace + global memento, re-prompts on `.standardoc/` deletion)
- [x] AI agent skill generation (`.claude/skills/standardoc/SKILL.md`) with language coverage table and edge-attributes documentation
- [x] MCP server provider for Copilot Chat / Claude Code; `.mcp.json` cross-client merge (5 actions discriminated, preserves user fields)
- [x] `.mcp.json` rewritten to the daemon's actual URL on every `ready` transition (covers ephemeral port fallback)
- [x] `.stdignore` language contribution + gitignore-style hover preview
- [x] ~~RAG commands palette + settings + status bar + endpoint race fix + DL progress markers~~ — removed in beta.3
- [x] Daemon restart serialised
- [x] Fatal error handling parses `STDOC_FATAL` markers (no regex on prose error messages)
- [x] ~~Token savings command + status bar item~~ — removed in beta.3

#### Infra
- [x] CI hardening: `cargo fmt --all` workspace cleanup, broken intra-doc-link fixes, `clippy::format_push_string` / `match_same_arms` fixes
- [x] CI switched from `actions-rust-lang/setup-rust-toolchain` to `dtolnay/rust-toolchain` (macos-latest reliability)
- [x] Code-scanning workflow permission tightening (5 auto-fix PRs merged)
- [x] `release.yml` simplified (crates.io publish steps removed)
- [x] Cargo `package.publisher` field fix
- [x] `.gitignore` covers `.standardoc-sessions/`, `sessions-export/`, `.claude/`, `.mcp.json`, `ext/vscode/.standardoc/`
- [x] README + SECURITY.md + SUPPORT.md refreshed (AST + install details, supported versions, links audit)

### Remaining work

- [x] **Decouple `standardoc` binary from the VSCode extension VSIX**
  - VSIX ships without `standardoc[.exe]`. On first activation with no
    binary resolvable, the supervisor transitions into a new
    `awaiting_binary` state (distinct from `failed` so retry/backoff
    stays put).
  - Toast surfaces `Standardoc needs to download the native binary for
    this platform.` with `Download` / `Later` / `Show logs`. The status
    bar shows a `$(cloud-download) Standardoc` affordance that bypasses
    the regular menu and routes one click straight to the download
    command.
  - On `Download`: the installer fetches
    `releases/download/v<BINARY_VERSION>/version.json` (pinned via the
    ext's `binary-version.ts`, not `latest`), picks the asset for the
    current `process.platform + process.arch` mapped to the Rust
    target triple, downloads the archive, verifies SHA256, extracts
    via the system `tar`, and writes the binary under
    `<globalStorageUri>/bin/<rust-target-triple>/standardoc[.exe]`.
  - `binary-resolver.ts` ordering is `settings → globalStorage → PATH
    → throw`. `standardoc.binaryPath` remains the documented escape
    hatch for local dev (`target/debug/standardoc`) and pre-release
    pinning.
  - The `bundle-binary.ts` script and the `dev:bundle` / `package`
    chain that copied `target/release/standardoc[.exe]` into the VSIX
    are removed.
  - `protocol_version` compat verification rides on the SHA256: a
    matching SHA proves the binary published under the pinned tag,
    which proves the protocol contract. Surfacing
    `protocol_version` via `standardoc --version --json` as a runtime
    cross-check stays a post-1.0 item.
  - Net effect: VSIX size drops by tens of MB; binary updates ride
    independently of the ext release cadence; aligns with the beta.3
    `self-update` plumbing (same `version.json` consumption path).

- [ ] **Repo root audit + reorg into `.important/`**
  - Move long-form docs into a top-level `.important/` directory
    (intentionally eye-catching in the GitHub file listing so
    newcomers notice it from the README hub).
  - Files moving: `ABOUT(.fr).md`, `QUICKSTART(.fr).md`, `FAQ(.fr).md`,
    `COMPARISON(.fr).md`, `SUPPORT(.fr).md`, `TODO-LIST.md`.
  - Keep at root only what GitHub surfaces by convention:
    `README.md`, `LICENSE`, `SECURITY.md`, `CHANGELOG.md`
    (+ `CONTRIBUTING.md` if/when added).
  - `README.md` gets a "Navigate" section linking to each moved doc
    (en + fr pair per line), so the click-through path is one hop.
  - Update inbound link references everywhere: README cross-refs,
    `SUPPORT.md` links, ext `package.json` documentation URL,
    release-note links, in-repo file mentions.
  - Refresh content while moving — anything stale or pre-beta.1
    framing gets a pass.

- [ ] **Fix `renovate.json`** — currently non-functional, diagnose
  from scratch
  - Confirm the Renovate GitHub App is installed on
    `miralabs-tech/standardoc` (Settings → Apps).
  - Validate the existing `renovate.json` config via
    `npx --package renovate -c renovate-config-validator`.
  - Reconfigure to target `dev` (not `main` — `main` is
    branch-protected and Renovate PRs against it would be rejected):
    set `"baseBranches": ["dev"]`.
  - Trigger a hosted-app dry-run via the dependency-dashboard issue;
    inspect log for the actual reason past runs produced nothing.
  - Verify by waiting for the next scheduled run to produce a PR
    against `dev`.

### Release ops (pending)

- [ ] `CHANGELOG.md` entry for v1.0.0-beta.2 summarising the above
- [ ] Bump `Cargo.toml` workspace `version` → `1.0.0-beta.2`; sync member version refs in `[workspace.dependencies]`
- [ ] Bump `ext/vscode/package.json` `version` per cadence policy
- [ ] Tag push `v1.0.0-beta.2` → `release.yml` triggers (cross-platform binaries + `version.json` + GitHub Release)
- [ ] Workflow_dispatch `release-ext.yml` (`version=<ext-version>`, `pre_release=true`)
- [ ] Smoke F5 full E2E re-test post init opt-in flow + MCP-first hooks + skill gen
- [ ] Public roadmap announcement (link this file from README + GitHub Discussions)

---

## v1.0.0-beta.3 — Multi-workspace graphs · visual navigation · C provider

**Theme**: pluralize graph consumers. The *plan* was doc rendering + visual navigation + CLI autonomy; in practice dogfood pulled beta.3 toward **multi-workspace / multi-root graphs, interactive visualization, a native C provider, a multi-workspace proxy, and a deep edge-resolution overhaul** — while RAG and the session DB were cut. **Visual navigation and CLI autonomy shipped; doc rendering slipped** to a later beta. The full shipped surface lives in [`CHANGELOG.md`](../../CHANGELOG.md); the planned axes below are annotated with what landed.

### Documentation rendering layer — ~~slipped past beta.3~~

The doc graph (SQLite) is the universal source of truth. Renderers are consumers — MDX is one option, not the base. The same graph must be consumable by any framework without a MDX dependency.

Pipeline target:

```
source code → @doc parser → doc graph (SQLite) → framework-agnostic query API → renderer
```

**Doc graph & query layer** (framework-agnostic, ships as `@standardoc/core`):
- [ ] Doc-graph schema additions (`description`, `examples`, `tags`) without re-introducing a custom DSL
- [ ] Annotation parser (`@doc`, `@param`, `@returns`, `@example`) with language-provider hooks
- [ ] `queryDocs("api.*")` — glob query helper exposed as a plain JS/TS API (no framework required)

**React renderer** (ships as `@standardoc/react`, first renderer):
- [ ] `<Doc id="…" />` — single doc block render
- [ ] `<Params id="…" />` — parameter table
- [ ] `<Examples id="…" />` — example snippets
- [ ] `<Signature id="…" />` — code-fence signature
- [ ] Drop-in adapters for Next.js, Nextra, Astro, Docusaurus

**Future renderers** (post-beta.3, same graph, different packages):
- [ ] `@standardoc/vue` — same components for Vue / VitePress / Nuxt
- [ ] `@standardoc/svelte` — for SvelteKit, plain Svelte

### Visual navigation — ✅ shipped (`standardoc-graph-viz`)

Surface the graph as an interactive visual artifact for the maintainer who reviews/audits their own code. Shipped as the `standardoc-graph-viz` WASM crate + web-component shell, hosted in a VSCode webview and a standalone playground, driven through MCP.

- [x] Graph shell: overview (3D topology + project clusters) / focus-graph / explorer / symbol-details / search panels
- [x] Click-to-navigate (drill into neighbors, breadcrumb back)
- [x] Compact enrichment view (signatures / fields / relations without opening files)
- [x] Filter chips for `kind` / `visibility` / language + hide-tests

### CLI self-management (`standardoc` without VSCode) — ✅ partly shipped

- [x] `standardoc init` (agent skill + MCP-first hooks + `AGENTS.md` + `.mcp.json`) + `standardoc mcp --connect` stdio↔http bridge — first-class non-VSCode agent wiring
- [x] `standardoc self-update` sub-command: reads `version.json` from GitHub Releases, detects platform, downloads + SHA256-verifies the matching binary, replaces the current executable (Windows-aware rename-on-replace)
- [ ] Initial install PATH injection: places binary under `~/.stdoc/bin/` (Unix) or `%USERPROFILE%\.stdoc\bin\` (Windows) and registers the path in:
  - bash/zsh: appends `export PATH="$HOME/.stdoc/bin:$PATH"` to `.bashrc` / `.zshrc`
  - PowerShell: appends to `$PROFILE`
  - CMD / Windows permanent: writes to `HKCU\Environment\Path` via `winreg` crate
- [ ] One-liner bootstrap scripts: `curl -sSf https://… | sh` (Unix) + `irm https://… | iex` (PowerShell)

### Cross-session project understanding — ~~extracted to a sibling tool~~

The synthesized project understanding an agent reloads each session (goals,
posture, locked decisions, narrative intent) no longer lives in Standardoc
core. It moved to a **sibling session-store tool**, alongside the beta.3
extraction of the session-handoff DB — the core stays a code graph, not an
agent-memory store. The guardrail is unchanged: any such synthesis is a
derived projection re-validated against the graph, never an independent
source of truth.

---

## v1.0.0 — Stabilization

**Theme**: API freeze. Performance and operational maturity before locking
the surface.

- [ ] Virtual annotations enrichments (verb-prefix conventions, type-signature narratives, trait impl templates)
- [ ] **Cross-substrate bridge kinds** — close the `BridgeKind` vocabulary (`"tauri"`, `"wasm"`, `"ffi"`, `"sql"`, `"orm"`, `"db-table"`, `"db-model"`, and whatever else dogfood surfaces) before the 1.0 freeze. The actual vocab extension can ship on any of the `beta.X` between now and 1.0 (dogfood-driven, not calendar-driven). **Frontend detectors are NOT shipped at 1.0** — they land post-1.0 via the UST + Lua plug-in layer (the combinatorics of substrate × language × ORM is too wide for the core)
- [ ] HTTP/SSE MCP transport for multi-machine shared daemon
- [ ] Performance benchmarks on 1M+ LOC monorepos
- [ ] API surface freeze documented + first stable contract

---

## Post-1.0 ideas (no commitment)

- [ ] Additional language providers (Go, Java, Swift, C#, Kotlin, Zig) — Lua, Vue, Svelte shipped in beta.2
- [ ] Custom LSP methods for Standardoc-specific queries
- [ ] Optional GitBook-style local doc UI (if demand emerges; lifetime license)
- [ ] LSP bridge to rust-analyzer / tsserver for richer per-language depth
- [ ] **Code commentary import/export via FQDN-anchored safe-edit pointers** — replace the killed `materialize` command with a more rigorous primitive: write doc-comments / annotations / `@doc` blocks back into source code, anchored on FQDN locations (more stable than raw line ranges across refactors). Goal: maintain clean codebases (signatures + body, no comment walls) while keeping the prose in the graph, with safe re-injection on demand and no risk of desync between graph and source.

### Universal language provider: UST + Lua scripting layer

**Vision**: Standardoc stops understanding languages directly — it understands a universal normalized representation (UST), and Lua defines how each language maps into it. Adding a new language becomes writing a Lua plugin, not a Rust backend.

```
source code
  → parser (tree-sitter / any tool) → UST (language-agnostic normalized AST)
  → Lua plugin (defines symbols, relations, edges)
  → Rust validates + stores into the IR graph
```

- [ ] **UST spec**: define a minimal language-agnostic node schema (kind, name, span, children, attributes) that all parsers output
- [ ] **Lua runtime** (embedded `mlua`): sandbox that receives a UST tree and returns `Vec<IrSymbol>` + `Vec<IrEdge>`
- [ ] **tree-sitter integration**: universal parser front-end; community grammars cover 100+ languages without new Rust deps
- [ ] **Plugin discovery**: `.standardoc/plugins/<lang>.lua` auto-loaded per workspace
- [ ] Replaces / complements the WASM bridge approach for community language providers (Lua = lower barrier than WASM; WASM kept for full native performance plugins)

### VSCode extension version selector (consumes `version.json`)

- [ ] `standardoc.coreVersion` setting: `"bundled" | "latest" | "<semver>"`
- [ ] Binary downloader: fetch `version.json`, GET tarball matching platform, verify SHA256, cache in `globalStorageUri`
- [ ] Switch `binary-resolver.ts` target to downloaded binary when setting != `"bundled"`
- [ ] Compat check at boot: warn toast if core/ext outside `min_compat` window
- [ ] Surface `protocol_version` in `standardoc --version` output, parse + check it ext-side
- [ ] Settings UI affordance to refresh / re-download / clear cache

---

## Deferred / killed

- [x] ~~v0 DSL templating (`{{ @doc.X }}` markdown expressions)~~ — killed in favour of MDX/React layer (see beta.3)
- [x] ~~`materialize` command~~ (write virtual annotations back to source) — punted; may return as opt-in once virtual annotations land
- [x] ~~`standardoc-server` separate binary~~ — consolidated into `standardoc` sub-commands
- [x] ~~Lua / Python / tree-sitter providers in beta.1~~ — Lua native provider shipped in beta.2 (`full_moon`); Python + tree-sitter deferred post-1.0
- [x] ~~`.standardoc.json` config file~~ — replaced by `.stdignore` + `schema_meta` SQLite table
- [x] ~~`.stdocignore`~~ — renamed to `.stdignore`
- [x] ~~`cargo install standardoc-cli` as sole distribution channel~~ — beta.1 ships pre-built cross-platform binaries via GitHub Releases (`release.yml`); `cargo install --git` available for source builds
- [x] ~~RAG prose-retrieval layer (beta.2)~~ — removed in beta.3; Standardoc bets on structural resolution over vector similarity
- [x] ~~Session-handoff DB + cross-session project understanding (beta.2)~~ — extracted to a sibling session-store tool; the core stays a code graph, not an agent-memory store
- [x] ~~`usage_stats` / token-savings surface (beta.2)~~ — removed in beta.3
