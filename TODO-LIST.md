# Roadmap

Source of truth for what's planned, what's shipping, and what's deliberately
deferred. [`CHANGELOG.md`](CHANGELOG.md) tracks what actually shipped per
release; this file tracks intent.

Box convention: `[x]` shipped · `[ ]` planned · `~~struck~~` killed or
deferred to a later milestone.

---

## Versioning policy

Standardoc has two release deliverables that evolve at different cadences:

- **Core** — published to crates.io as `standardoc-cli` (single binary `stdoc`).
  Tag-driven: pushing `vX.Y.Z` triggers `release.yml` (cross-platform pre-built
  binaries + `version.json` manifest + crates.io publish + GitHub Release).
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
  "binaries": { "x86_64-unknown-linux-gnu": "https://.../stdoc-...tar.gz", ... },
  "checksums_sha256": { "x86_64-unknown-linux-gnu": "abc123...", ... }
}
```

Stable URL: `https://github.com/miralabs-tech/standardoc/releases/latest/download/version.json`.

The future ext version selector consumes this file to populate the
`standardoc.coreVersion` setting and download matching `stdoc` binaries
on demand.

### Protocol version

`protocol_version` (currently `1`) is decoupled from semver. It tracks the
IPC contract (MCP tool signatures, LSP custom methods) and bumps only on
real wire-format breaks. Ext checks `stdoc --version` (which exposes the
protocol version) at boot; mismatch triggers a warning toast.

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
- [x] Typed edges day-1: `CALLS`, `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`, `DEFINES`, `USES_TYPE`, `EXPOSES_API`

#### Pipeline
- [x] Cold-start eager full-workspace index
- [x] Live file watcher with debounced auto-rescan (`notify` + `notify-debouncer-full`)
- [x] `.stdignore` (gitignore syntax) + auto-seed at workspace root
- [x] Hot-reload of `.stdignore` (diff/swap/warn/reindex)
- [x] `purge-excluded` sub-command for post-`.stdignore`-edit cleanup
- [x] Pause / resume index handle

#### CLI / Daemons
- [x] Single binary `stdoc` with sub-commands (`lsp`, `mcp`, `index`, `rescan`, `watch`, `query`, `purge-excluded`)
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

### Remaining before tag

- [x] `Cargo.toml` workspace `publish` flags audit (all crates default to `publish = true`, no opt-out needed)
- [x] Path-only deps verified to have `version` field (5 internal deps in `[workspace.dependencies]` aligned to `1.0.0-beta.1`)
- [x] `bridge-sdk` aligned to `version.workspace = true` (was hardcoded `0.0.1`)
- [x] `cargo publish --dry-run -p standardoc-ir` (leaf, validated)
- [x] `cargo publish --dry-run -p standardoc-bridge-sdk` (leaf, validated)
- [x] `cargo package --list` for downstream crates (packaging shape OK for `core`, `lang-provider`, `server`, `cli`)
- [ ] **First publish chain (manual, in dependency order)**:
  ```sh
  cargo publish -p standardoc-ir            && sleep 30
  cargo publish -p standardoc-bridge-sdk    && sleep 30
  cargo publish -p standardoc-core          && sleep 30
  cargo publish -p standardoc-lang-provider && sleep 30
  cargo publish -p standardoc-server        && sleep 30
  cargo publish -p standardoc-cli
  ```
  Each `sleep 30` lets crates.io index propagate before the next dependent crate validates against it. After the first chain is on crates.io, push the `v1.0.0-beta.1` tag → `release.yml` handles subsequent releases via `cargo publish -p standardoc-cli` (leaf only).
- [ ] Push `v1.0.0-beta.1` tag (drives `release.yml`: cross-platform binaries + version.json + crates.io publish + GitHub Release)
- [ ] Trigger `release-ext.yml` workflow_dispatch with `version=1.0.0-beta.1` + `pre_release=true`
- [ ] Smoke F5 full E2E re-test post init opt-in flow + skill gen
- [ ] Public roadmap announcement (link this file from README + GitHub Discussions)

### Future workflow enhancement (post-beta.1)

- [ ] Enhance `release.yml` to publish the full chain in dependency order on tag push (currently only publishes the `standardoc-cli` leaf, assumes upstream chain already on crates.io). Either iterate the 6 crates with sleeps, or use `cargo workspaces publish` / `release-plz`.

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

## v1.0.0-beta.2 — Documentation rendering layer

**Theme**: ship the doc-rendering replacement for the killed v0 DSL.

- [ ] npm package shipping React/MDX components fed by the doc graph
  - [ ] `<Doc id="…" />` — single doc block render
  - [ ] `<Params id="…" />` — parameter table
  - [ ] `<Examples id="…" />` — example snippets
  - [ ] `<Signature id="…" />` — code-fence signature
  - [ ] `queryDocs("api.*")` — glob query helper
- [ ] Drop-in adapters for Next, Nextra, Astro, Docusaurus
- [ ] MDX live-resolution against the SQLite doc graph
- [ ] Doc-graph schema additions (description, examples, tags) without re-introducing a custom DSL
- [ ] Annotation parser (`@doc`, `@param`, `@returns`, `@example`) with language-provider hooks

Pipeline target:

```
source code → @doc parser → doc graph (SQLite) → MDX/React layer → framework
```

---

## v1.0.0 — Stabilization

**Theme**: API freeze. Performance and operational maturity before locking
the surface.

- [ ] Virtual annotations enrichments (verb-prefix conventions, type-signature narratives, trait impl templates)
- [ ] Cross-language bridge plug-ins (WASM): Tauri commands, WASM bindings, FFI declarations
- [ ] HTTP/SSE MCP transport for multi-machine shared daemon
- [ ] Performance benchmarks on 1M+ LOC monorepos
- [ ] API surface freeze documented + first stable contract

---

## Post-1.0 ideas (no commitment)

- [ ] Additional language providers (Go, Java, Swift, C#, Kotlin, Zig)
- [ ] Custom LSP methods for Standardoc-specific queries
- [ ] Webview Preact rendering for richer in-VSCode display
- [ ] Optional GitBook-style local doc UI (if demand emerges; lifetime license, see [SUPPORT.md](SUPPORT.md))
- [ ] LSP bridge to rust-analyzer / tsserver for richer per-language depth

### VSCode extension version selector (consumes `version.json`)

- [ ] `standardoc.coreVersion` setting: `"bundled" | "latest" | "<semver>"`
- [ ] Binary downloader: fetch `version.json`, GET tarball matching platform, verify SHA256, cache in `globalStorageUri`
- [ ] Switch `binary-resolver.ts` target to downloaded binary when setting != `"bundled"`
- [ ] Compat check at boot: warn toast if core/ext outside `min_compat` window
- [ ] Surface `protocol_version` in `stdoc --version` output, parse + check it ext-side
- [ ] Settings UI affordance to refresh / re-download / clear cache

---

## Deferred / killed

- [x] ~~v0 DSL templating (`{{ @doc.X }}` markdown expressions)~~ — killed in favour of MDX/React layer (see beta.2)
- [x] ~~`materialize` command~~ (write virtual annotations back to source) — punted; may return as opt-in once virtual annotations land
- [x] ~~`standardoc-server` separate binary~~ — consolidated into `stdoc` sub-commands
- [x] ~~Lua / Python / tree-sitter providers in core~~ — Lua/Python out of beta.1 scope; tree-sitter providers via plug-in architecture only
- [x] ~~`.standardoc.json` config file~~ — replaced by `.stdignore` + `schema_meta` SQLite table
- [x] ~~`.stdocignore`~~ — renamed to `.stdignore`
- [x] ~~Pre-built cross-platform binaries on GitHub Releases~~ — beta.1 ships `cargo install standardoc-cli` only; pre-builts may return post-beta.1 if demand emerges
