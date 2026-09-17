# Standardoc

<p align="center">
  <a href="https://github.com/miralabs-tech/standardoc/releases"><img src="https://img.shields.io/badge/status-archived-lightgrey?style=flat-square" alt="Status: archived"></a>
  <a href="https://github.com/miralabs-tech/standardoc/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/miralabs-tech/standardoc/ci.yml?branch=main&label=ci&style=flat-square" alt="CI"></a>
  <a href=".important/en/QUICKSTART.md"><img src="https://img.shields.io/badge/surfaces-LSP%20·%20MCP-blue?style=flat-square" alt="Surfaces: LSP · MCP"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-FSL--1.1--MIT%20→%20MIT%202028-green?style=flat-square" alt="License: FSL-1.1-MIT → MIT 2028"></a>
  <a href="https://github.com/miralabs-tech/standardoc/stargazers"><img src="https://img.shields.io/github/stars/miralabs-tech/standardoc?label=stars&style=flat-square" alt="Stars"></a>
  <a href="https://marketplace.visualstudio.com/items?itemName=miralabs-tech.standardoc-vscode"><img src="https://img.shields.io/visual-studio-marketplace/i/miralabs-tech.standardoc-vscode?label=vscode%20installs&style=flat-square" alt="VSCode installs"></a>
  <a href="https://open-vsx.org/extension/miralabs-tech/standardoc"><img src="https://img.shields.io/open-vsx/dt/miralabs-tech/standardoc?label=ovsx%20downloads&style=flat-square" alt="OpenVSX downloads"></a>
</p>

> ## ⚠️ Archived on v1.0.0-beta.2 — 2026-09-17
>
> Standardoc is archived. **[v1.0.0-beta.2](https://github.com/miralabs-tech/standardoc/releases/tag/v1.0.0-beta.2)**
> is the last compiled release: it still installs and runs, but nothing will
> be fixed, updated or answered. The VSCode extension on the Marketplace /
> Open VSX pins that binary and will not move.
>
> The beta.3 source on `main` (multi-workspace graphs, graph viz, C provider,
> `standardoc init`, edge-resolution rework) will **not** be tagged or
> published. Want it anyway? Build it yourself, at your own risk:
> `cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli`
>
> **Why.** Measured on 2026-09-16/17 — A/B runs in real tokens (same model,
> same questions, on this repo) plus `sqlite3` counts on `.standardoc/index.db`:
>
> - **No measurable gain over grep** on a repository of ordinary size. The MCP
>   path saved turns when enumerating callers and cost more tokens everywhere
>   else. Any task that ends in an edit still needs the file `Read`, so the
>   graph query is purely additive.
> - **Silent misses.** Under 40 % of `CALLS` edges resolved; macro-generated
>   code is a blind spot; unresolved callers come back as an empty list, not
>   as "unknown". An agent that trusts the answer concludes "dead code".
> - **The harnesses caught up.** Claude Code and Codex now ship official LSP
>   plugins; the cross-language case (Tauri) is covered by specta / bindgen.
>
> The "~100 tokens instead of 30k" headline of earlier versions of this README
> was never measured and is withdrawn.

> **Your AI agent re-reads your whole codebase on every task.** Standardoc
> indexed it once into a living map of your code — so the agent could just
> *ask*. Local, open-source.

📖 English · [Français](.important/fr/README.md) &nbsp;|&nbsp; [Quickstart](.important/en/QUICKSTART.md) · [Roadmap](.important/en/TODO-LIST.md) · [Changelog](CHANGELOG.md)

---

## The problem

Every task, your agent starts from zero: it greps, it reads files, it
rebuilds context it already had last session. The bigger the codebase, the
worse it gets — more tokens, more drift, more code that *looks* like yours
but quietly breaks your invariants. That was the bet. Measured, the bet did
not pay off on repositories of ordinary size (see the banner above).

## What Standardoc does

It reads your code straight from the syntax tree and keeps a **living graph**
of it — every symbol, and the typed links between them: who calls who, what
imports what, what implements what. A file watcher keeps it current as you
type.

Your tools query that one graph instead of each re-parsing your code:

- **Agents** ask over MCP (`find_symbol`, `get_context`, `find_call_sites`, …).
  Tested with Claude Code only; other MCP clients were never exercised.
- **Editors** connect over LSP — the VSCode extension is the only integration
  that was built and tested.

Rust, TypeScript / JavaScript (React, JSX, TSX), Vue, Svelte, Lua, and C today.

## Install

**VSCode** — search *Standardoc* in the Marketplace or Open VSX. You get the
archived extension, pinned to the beta.2 binary.

**From source** (beta.3, unreleased — `standardoc init` exists only here):

```sh
cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli
standardoc init   # wires the agent skill, AGENTS.md, .mcp.json
```

→ [Quickstart](.important/en/QUICKSTART.md) (describes the `main` source)

## Where it was used

Only on this repository and a handful of the author's own projects. It was
never run on a compiler, a game engine or a large monorepo, and no scale
benchmark was ever built. On a weekend project `ripgrep` + your IDE are
plenty — and, measured, they were plenty here too.

## Why it's built this way

- **One graph, not N.** Every tool re-parsing your code is one more thing that
  drifts out of sync.
- **Real AST, never regex.** Heuristics rot the moment the code moves.
- **Yours, for good.** Local, [FSL-1.1-MIT](LICENSE) auto-converting to plain
  MIT (first release: April 26, 2028). No cloud, no lock-in, no rented graph.

---

Built by [**StandarX**](https://opencollective.com/standarx) &nbsp;·&nbsp; [Star the repo](https://github.com/miralabs-tech/standardoc) · [Sponsor](https://opencollective.com/standarx) · [Security policy](SECURITY.md)
