# Standardoc

<p align="center">
  <a href="https://github.com/miralabs-tech/standardoc/releases"><img src="https://img.shields.io/badge/status-beta-yellow?style=flat-square" alt="Status: beta"></a>
  <a href="https://github.com/miralabs-tech/standardoc/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/miralabs-tech/standardoc/ci.yml?branch=main&label=ci&style=flat-square" alt="CI"></a>
  <a href=".important/en/QUICKSTART.md"><img src="https://img.shields.io/badge/surfaces-LSP%20·%20MCP-blue?style=flat-square" alt="Surfaces: LSP · MCP"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-FSL--1.1--MIT%20→%20MIT%202028-green?style=flat-square" alt="License: FSL-1.1-MIT → MIT 2028"></a>
  <a href="https://github.com/miralabs-tech/standardoc/stargazers"><img src="https://img.shields.io/github/stars/miralabs-tech/standardoc?label=stars&style=flat-square" alt="Stars"></a>
  <a href="https://marketplace.visualstudio.com/items?itemName=miralabs-tech.standardoc-vscode"><img src="https://img.shields.io/visual-studio-marketplace/i/miralabs-tech.standardoc-vscode?label=vscode%20installs&style=flat-square" alt="VSCode installs"></a>
  <a href="https://open-vsx.org/extension/miralabs-tech/standardoc"><img src="https://img.shields.io/open-vsx/dt/miralabs-tech/standardoc?label=ovsx%20downloads&style=flat-square" alt="OpenVSX downloads"></a>
</p>

> **Your AI agent re-reads your whole codebase on every task.** Standardoc
> indexes it once into a living map of your code — so the agent just *asks*.
> **~100 tokens per question instead of 30k.** Local, open-source.

📖 English · [Français](.important/fr/README.md) &nbsp;|&nbsp; [Quickstart](.important/en/QUICKSTART.md) · [Roadmap](.important/en/TODO-LIST.md) · [Changelog](CHANGELOG.md)

---

## The problem

Every task, your agent starts from zero: it greps, it reads files, it burns
30k tokens rebuilding context it already had last session. The bigger the
codebase, the worse it gets — more tokens, more drift, more code that *looks*
like yours but quietly breaks your invariants.

## What Standardoc does

It reads your code straight from the syntax tree and keeps a **living graph**
of it — every symbol, and the typed links between them: who calls who, what
imports what, what implements what. A file watcher keeps it current as you
type.

Your tools query that one graph instead of each re-parsing your code:

- **Agents** ask over MCP (`find_symbol`, `get_context`, `find_call_sites`, …)
  — **~100 tokens** where grep + read cost 30k. Claude Code, Cursor, Continue,
  Copilot, any MCP client.
- **Editors** connect over LSP — the VSCode extension is built in; Neovim,
  Helix, JetBrains point at the same binary.

Rust, TypeScript / JavaScript (React, JSX, TSX), Vue, Svelte, Lua, and C today.

## Install

**VSCode** — search *Standardoc* in the Marketplace or Open VSX.

**CLI** (any agent, no VSCode):

```sh
cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli
standardoc init   # wires the agent skill, MCP-first hooks, AGENTS.md, .mcp.json
```

→ [5-minute quickstart](.important/en/QUICKSTART.md)

## Who it's for

Big, complex codebases — compilers, engines, heavy monorepos. On a weekend
project `ripgrep` + your IDE are plenty; Standardoc earns its keep once the
archaeology starts costing you real time.

## Why it's built this way

- **One graph, not N.** Every tool re-parsing your code is one more thing that
  drifts out of sync.
- **Real AST, never regex.** Heuristics rot the moment the code moves.
- **Yours, for good.** Local, [FSL-1.1-MIT](LICENSE) auto-converting to plain
  MIT (first release: April 26, 2028). No cloud, no lock-in, no rented graph.

---

Built by [**StandarX**](https://opencollective.com/standarx) &nbsp;·&nbsp; [Star the repo](https://github.com/miralabs-tech/standardoc) · [Sponsor](https://opencollective.com/standarx) · [Security policy](SECURITY.md)
