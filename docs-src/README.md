<!--
  AUTO-GENERATED FILE — DO NOT EDIT.
  Source: docs-src/README.md
  Re-render via: ./scripts/render-docs.sh
  CI gate: .github/workflows/docs-render.yml
-->

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="branding/lockup-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="branding/lockup-light.svg">
    <img alt="standardoc" src="branding/lockup-light.svg" width="520">
  </picture>
</p>

<p align="center"><strong>Source-of-truth documentation for code that AI agents can actually consume.</strong></p>

<p align="center">📖 English · <a href="README.fr.md">Français</a></p>

<p align="center">
  <a href="ABOUT.md">About</a> ·
  <a href="QUICKSTART.md">Quickstart</a> ·
  <a href="docs/cli-reference.md">CLI reference</a> ·
  <a href="docs/mcp-reference.md">MCP reference</a> ·
  <a href="docs/ai-integration.md">AI integration</a> ·
  <a href="CHANGELOG.md">Changelog</a>
</p>

---

> [!WARNING]
> **Alpha/beta — `v0.x.x`.** I'm shipping fast on this rewrite. Expect
> frequent releases, occasional breaking changes between minor versions,
> and rapid iteration. The API surface freezes only at `v1.0.0` — until
> then, the entire toolchain stays fully OSS under FSL-1.1-MIT and the
> closed-source [Standardoc Pro](/) tier won't ship.
>
> This Rust rewrite supersedes my earlier TypeScript prototype
> [`SUP2Ak/standardoc-cli`](https://github.com/SUP2Ak/standardoc-cli),
> which I iterated on personally for years. What you see here is the
> rebuilt-from-scratch version with proper AST parsers, an MCP server,
> an LSP, virtual annotations, and a much wider ambition.

Standardoc decouples *structured data* (annotations in your source) from *narrative
prose* (markdown you write). It scans any codebase, builds an index of every
documentable symbol, and exposes it through:

- A **DSL** to inject up-to-date code fragments into hand-written `.md`
- An **LSP server** for completions, navigation, diagnostics, and rename in your editor
- An **MCP server** so agents (Claude Code, Cursor, Zed, Continue, …) query the index in ~100 tokens instead of grep+read'ing 30k–100k tokens

The core value proposition: **zero drift** between what's in the code and what
appears in the docs. Annotations live next to their symbol; prose lives in
markdown; the DSL stitches the two.

## Install

**Linux / macOS** :

```sh
curl -fsSL https://raw.githubusercontent.com/miralabs-tech/standardoc/main/scripts/install.sh | sh
```

**Windows (PowerShell)** :

```powershell
irm https://raw.githubusercontent.com/miralabs-tech/standardoc/main/scripts/install.ps1 | iex
```

Both scripts pull the latest release from
[GitHub Releases](https://github.com/miralabs-tech/standardoc/releases),
verify the SHA256 checksum, and install the `standardoc` + `standardoc-server`
binaries into `~/.standardoc/bin/`. Pin a specific version with
`STANDARDOC_VERSION=v0.1.0` (override env var works on both platforms).

**Build from source** :

```sh
git clone https://github.com/miralabs-tech/standardoc
cd standardoc
cargo build --release -p standardoc -p standardoc-server
```

The release binaries land in `target/release/`. Add this directory to your
`PATH` or copy `standardoc` and `standardoc-server` somewhere already on it.

**Pre-built binaries** : every tagged release ships archives for
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`,
`aarch64-apple-darwin`, and `x86_64-pc-windows-msvc` — download manually from
the [latest release](https://github.com/miralabs-tech/standardoc/releases/latest)
if the install scripts don't fit your setup.

## Quick start

See [`QUICKSTART.md`](QUICKSTART.md) for a 5-minute walkthrough. Briefly:

```sh
# 1. Scan a workspace, print canonical DocBlocks as JSON
cargo run -p standardoc -- scan examples/rust-lib/src

# 2. Render a markdown template against the scan
cargo run -p standardoc -- transform examples/rust-lib examples/rust-lib/docs-src/api.md

# 3. Validate annotations + DSL (STD001 dup keys, STD004 broken refs, …)
cargo run -p standardoc -- validate examples/rust-lib

# 4. Run the daemon (LSP + MCP + watcher) for live editor / agent integration
cargo run -p standardoc-server --release -- --mcp --workspace .
```

## Annotating source

```rust
/// Adds two integers together.
/// @doc calculator.add add
/// @param a i32 first operand
/// @param b i32 second operand
/// @returns i32 the sum
pub fn add(a: i32, b: i32) -> i32 { a + b }
```

The `@doc` line declares the canonical key and an optional label. `@param` and
`@returns` use a positional convention `<name> <type> <description>`. Prose
above the first `@tag` becomes the implicit description.

Languages supported out of the box:

| Language | Provider crate | Backend |
|---|---|---|
{{ each p in @docs.module(lang.providers) }}
| {{ p.label }} | ``{{ p.crate }}`` | ``{{ p.backend }}`` |
{{ /each }}

**Add any other language without recompiling**: drop a JSON config into
`.standardoc/languages/`. See [`examples/dynamic-langs/`](examples/dynamic-langs/)
for tree-sitter forks (CfxLua, MoonScript, …) and pure-regex fallbacks.

## Writing markdown that pulls from the index

````markdown
# `{{ @doc.calculator.add:label }}`

{{ @doc.calculator.add:description }}

```rust
{{ @doc.calculator.add:symbol.signature }}
```

## Parameters

{{ each p in @doc.calculator.add:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.calculator.add:returns.type }}`): {{ @doc.calculator.add:returns.description }}
````

DSL key rules :
- `.` navigates inside the block key (FQN can contain dots: `api.users.create`)
- `:` switches from the key to an accessor — block field (`label`, `meta.path`,
  `symbol.signature`) or a tag (`description`, `param[0].name`)
- `{{ each X in @doc.KEY:tag }} … {{ /each }}` iterates
- `{{ each block in @docs.module(prefix) }} … {{ /each }}` iterates blocks of
  a sub-module (or `@docs.all` for everything)
- `{{ if CONDITION }} … {{ else if CONDITION }} … {{ else }} … {{ /if }}`
- Block directives alone on a line consume that line — no phantom blanks

Full DSL reference is exposed via the MCP tool `get_dsl_reference` — the
same content is served to agents and IDE consumers directly.

## What's in the box

### CLI (`standardoc`)
`scan`, `transform`, `emit`, `validate`, `materialize`. Single-shot
operations on a workspace.

### Daemon (`standardoc-server`)
Long-running process exposing two protocols simultaneously:
- **LSP** (stdio) — completions on `@doc.…`, hover, goto-definition,
  references, workspace symbols, document outline, semantic tokens for the
  DSL, code actions (insert `@doc` skeleton, quick fixes), rename across
  `.md` and source `@doc` tags, push diagnostics on every rescan
- **MCP** (stdio, `--mcp` flag) — tools for agents:
{{ each t in @docs.module(mcp.tools) }}{{ if t.label != "args" }}
  - ``{{ t.label }}``{{ /if }}{{ /each }}

A built-in **watcher** (debounced + auto-pause on parse storms) keeps the
index in sync with disk; revision bumps push fresh diagnostics to LSP
clients without polling.

### Validator
10 lint rules out of the box, each overridable via `.standardoc.json`'s `rules`:

| Code | Severity | Description |
|------|----------|-------------|
{{ each rule in @docs.module(validator.rules) }}
| {{ rule.code }} | {{ rule.severity }} | {{ rule.description }} |
{{ /each }}

(Codes STD009–STD011 reserved.)

### Emit formats
`llms.txt` / `llms-full.txt` (Jeremy Howard's standard), `skill.md` (Claude
Code skills), and `OpenAPI 3.0` (from `@route`/`@param`/`@response` tags).

## MCP setup

Drop a `.mcp.json` at your workspace root :

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

After rebuilding the binary (`./scripts/build.sh` or `./scripts/build.ps1`,
option `[2] prod`), start a new Claude Code conversation — the MCP picks up
the new binary without a full VSCode restart.

## Workspace layout

```
crates/
├── standardoc-core             # data model, DSL, index, watcher, validator, scanner
├── standardoc-lang-rust        # syn-based provider
├── standardoc-lang-ts          # swc-based provider
├── standardoc-lang-python      # Python AST provider
├── standardoc-lang-tree-sitter # generic tree-sitter provider (Lua + dynamic forks)
├── standardoc-cli              # one-shot CLI
├── standardoc-server           # LSP + MCP + Web (HTTP/SSE) daemon
├── standardoc-web              # HTTP/SSE backend (REST API for any frontend)
├── standardoc-wasm             # browser bindings
└── standardoc-test-utils       # internal test helpers
examples/                       # runnable end-to-end demos
scripts/                        # build helpers (interactive menu: dev / prod / inspect)
```

## Configuration (`.standardoc.json`)

Optional. Drop at the workspace root to customize:

```json
{
  "version": 2,
  "docTag": "doc",
  "hideTag": "hide",
  "discovery": {
    "exclude": ["myproject.bench.*", "myproject.dev.__*"],
    "exclude_files": ["**/*.generated.ts"],
    "virtual_annotations": "medium"
  },
  "rules": {
    "STD006": "off"
  },
  "watch": {
    "enabled": true,
    "debounceMs": 100
  }
}
```

`@hide` (or whatever `hideTag` is set to) on a doc-comment excludes the block
from the index, source-side. `discovery.exclude` does the same via `DocKey`
patterns, config-side.

**File-level filtering** (applied *before* the scanner opens any file):

- The scanner already respects your repo's `.gitignore` (so `node_modules/`,
  `target/`, `dist/`, … are skipped without configuration).
- Drop a `.stdocignore` next to `.gitignore` for additional gitignore-style
  rules dedicated to documentation indexing — useful when you want different
  policies for git tracking vs doc indexing.
- On top of those, `discovery.exclude_files` adds Standardoc-specific
  gitignore-style patterns straight from `.standardoc.json`. Use
  `!pattern` to re-include a file excluded by a parent pattern.

The distinction matters: `discovery.exclude` filters by `DocKey` *after* the
scan (post-extraction, useful to hide modules whose code you can't or won't
modify); `exclude_files` and `.stdocignore` skip files *before* parsing
(faster, more aggressive).

## Virtual annotations — day-1 utility on any fork

The most common pain point with documentation tooling: clone a project, the
agent has *nothing* to chew on, every question turns back into `grep + cat`.
Standardoc's virtual-annotation pass closes that gap automatically.

After AST extraction, every undocumented public symbol gets virtual
`@doc`/`@param`/`@returns` content synthesized from naming conventions,
type signatures, and module structure. The synthesized content lives in
`DocBlock.virtualTags` (separate from real `tags`) and `get_doc` returns
both — agents see useful descriptions without anyone writing them.

A few examples (`level: medium`, the default):
- `fn new(...) -> Self` → "Creates a new `{ParentType}`."
- `is_active(&self) -> bool` → "Returns `true` if active."
- `get_user(id: u64) -> Option<User>` → "Returns the user." + virtual
  `@param id` and `@returns Option<User>`
- `impl Display for Foo` → "Formats `Foo` for human-readable display."
- `impl From<&str> for Url` → "Converts a `&str` into a `Url`."

Tier control via `discovery.virtual_annotations` in `.standardoc.json`:

| Level | What it covers |
|---|---|
| `off` | Pass disabled. MCP returns AST signatures only. |
| `low` | Public symbols, highest-confidence templates only (`new`, `is_*`, `len`, trait impls). |
| `medium` (default) | `low` + verb-prefix conventions + param-name hints + return-type narrative. |
| `high` | `medium` + crate-private symbols + module-path categorization. |

Once a virtual annotation is good enough, promote it to a real `///`
comment in source via:

```sh
standardoc materialize ./my-project
# Dry-run by default. Add --apply to actually edit files.
# --confidence low|medium|high to filter (default: medium).
```

The `materialize` command formats virtual content as language-appropriate
doc-comments (`///` for Rust, `---` for Lua, `/** … */` for TS/JS) and inserts them
above the symbol declaration, preserving indentation. Once written, the
real annotation wins — virtual content disappears for that block on the
next scan.

## Status (2026-04)

**v0.1.0 release-ready** for the core pipeline + LSP + MCP. CLI, daemon,
full MCP tool surface, LSP with rename propagation, Rust/TS/Python/Lua tree-sitter language providers all live.

## Open-source vs Standardoc Pro

Standardoc is **open-core**. Two distinct deliverables :

- **Standardoc Core** *(this repo)* — CLI, LSP, MCP server, all language
  providers, DSL, validator, plugins API, HTTP/SSE backend. Source under
  **FSL-1.1-MIT**. Free for any non-competing use; converts to plain MIT
  on the second anniversary of each release.
- **Standardoc Pro** *(separate, post `v1.0.0`)* — the polished web UI
  (GitBook-like navigation, MDX live components, live editing, search,
  polish). Closed-source, one-time **lifetime** purchase, no subscription.
  Distributed as a binary bundling the official frontend. Ships separately
  to keep this repo fully OSS. **Held back during the `v0.x.x` cycle** so
  the API surface stabilizes first ; everything you can install today is
  OSS under FSL-1.1-MIT.

Without Pro, the open-source `standardoc-server` binary still exposes
everything programmatically — you can build your own frontend against the
documented `/api/*` endpoints, or use any external static site generator
(Astro, Vitepress, Hugo, …) on top of `standardoc emit web --out` (data-only
mode).

## Versioning & releases

Standardoc follows [SemVer 2.0](https://semver.org/spec/v2.0.0.html).

- **Stable releases** are tagged `vX.Y.Z` (e.g. `v0.1.0`).
- **Pre-releases** are tagged `vX.Y.Z-rc.N` and are flagged as pre-release on GitHub.
- **Pre-1.0 caveat**: while we're below `v1.0.0`, MINOR bumps may include
  breaking changes (per SemVer pre-1.0 convention). PATCH bumps stay
  backwards-compatible. From `v1.0.0` onwards, breaking changes require MAJOR.

Every release ships:
- Pre-built binaries for Linux x64/arm64, macOS x64/arm64, Windows x64
- SHA256 checksums for each archive
- Release notes (attached to the tag) describing every user-visible change
  since the previous release

The release pipeline is fully automated: pushing a `vX.Y.Z` tag triggers
[`.github/workflows/release.yml`](.github/workflows/release.yml), which builds,
packages, verifies, and publishes the GitHub Release.

**Bumping**:

1. Update `version` in the workspace `Cargo.toml` (root).
2. Commit with `chore: release vX.Y.Z`.
3. Tag : `git tag vX.Y.Z && git push origin main vX.Y.Z`.
4. Write the release notes directly on the tag page once the build pipeline
   has produced the archives.

## Contributing

Contributions are welcome — bug reports, fixes, language providers, validator
rules, doc improvements, all of it.

**Setup**:

```sh
git clone https://github.com/miralabs-tech/standardoc
cd standardoc
# Install Rust toolchain (1.89+) — see rust-toolchain.toml
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

The interactive build helpers in `scripts/build.{sh,ps1}` cover the common
local-dev cycle (`[1] dev` builds to `target-dev/` for parallel-safe iteration,
`[2] prod` kills running servers and rebuilds in `target/`).

**Workflow**:

1. Open an issue first for non-trivial changes — saves you from writing code
   that conflicts with in-flight work or a roadmap decision.
2. Fork → branch → PR against `main`. Keep PRs focused on one concern.
3. Reference the issue (`Fixes #123`) and describe the change in the body.
4. CI must be green: `fmt`, `clippy --all-targets -D warnings`, `test`, `docs`.
5. A maintainer reviews and merges (squash strategy, so commit count inside
   the PR doesn't matter — the message at merge does).

**Conventions**:

- **Commits**: Conventional Commits format (`feat:`, `fix:`, `chore:`,
  `docs:`, `refactor:`, `test:`, `perf:`, etc.). Used to generate changelog
  entries and enforce release semantics.
- **Branch names**: `feat/short-description`, `fix/issue-123`, etc.
- **No comments by default**: code should be self-documenting via clear
  naming. Add comments only when the *why* is non-obvious — and write them
  in **English only**.
- **No new dependencies without discussion**: open an issue first.
- **Multi-language docs**: the main user-facing `.md` files have a `.fr.md`
  variant. If you modify `README.md`, update `README.fr.md` too (or open an
  issue to flag the drift if you don't speak French). EN is the source of
  truth ; FR follows.

**Adding a language provider**:

Implement the `LanguageProvider` trait from `standardoc-core`. Existing
providers (`standardoc-lang-rust`, `standardoc-lang-ts`,
`standardoc-lang-python`, `standardoc-lang-tree-sitter`) are good references.
For exotic languages without an existing native parser, a tree-sitter grammar
is usually the path of least resistance — see
[`examples/dynamic-langs/`](examples/dynamic-langs/) for the runtime-loaded
JSON config approach.

**Code of Conduct**: be respectful, assume good faith, no harassment. Until a
formal CoC is published, the
[Contributor Covenant](https://www.contributor-covenant.org/) applies by
default — flag issues to the maintainers via the email in `Cargo.toml`.

**Security**: do not open public issues for security vulnerabilities. Email
the maintainer directly (see `Cargo.toml` `authors`) until a `SECURITY.md`
policy is published.

## Support the project

I'm the sole maintainer of Standardoc Core, and I work on this on top of a
day job. If it saves you time, two ways to give back :

- **OpenCollective** — recurring or one-time donations support core
  development, language providers, and validator rules.
  *(Profile setup in progress — link will be added at v0.1.1.)*
- **[Standardoc Pro](/)** — buy a lifetime license for the polished web UI
  *(available post `v1.0.0` — held back during the `v0.x.x` cycle so the
  API stabilizes first)*. Direct revenue funds the entire ecosystem,
  including the OSS Core you're using right now.

For commercial sponsorship, custom language providers, or paid support
contracts : contact via the email in `Cargo.toml` `authors`.

## License

[**FSL-1.1-MIT**](LICENSE) — Functional Source License v1.1 with MIT future
license. You may use, modify and redistribute Standardoc Core for any
purpose **except** offering a competing product or service that substitutes
for Standardoc itself. Two years after each release, that release converts
automatically to plain MIT.

Why FSL : protects against direct competing offerings (the "open-and-pillage"
pattern) without locking down the core for honest end-users. Adopted by
Sentry, CodeCrafters, Keygen.
