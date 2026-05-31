# Standardoc

<p align="center">
  <a href="https://github.com/miralabs-tech/standardoc/releases"><img src="https://img.shields.io/badge/status-beta-yellow?style=flat-square" alt="Status: beta"></a>
  <a href="https://github.com/miralabs-tech/standardoc/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/miralabs-tech/standardoc/ci.yml?branch=main&label=ci&style=flat-square" alt="CI"></a>
  <a href=".important/en/ABOUT.md"><img src="https://img.shields.io/badge/core-canonical%20IR%20%2B%20live%20graph-blueviolet?style=flat-square" alt="Core: canonical IR + live graph"></a>
  <a href=".important/en/QUICKSTART.md"><img src="https://img.shields.io/badge/surfaces-LSP%20·%20MCP-blue?style=flat-square" alt="Surfaces: LSP · MCP"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-FSL--1.1--MIT%20→%20MIT%202028-green?style=flat-square" alt="License: FSL-1.1-MIT → MIT 2028"></a>
  <a href="https://github.com/miralabs-tech/standardoc/stargazers"><img src="https://img.shields.io/github/stars/miralabs-tech/standardoc?label=stars&style=flat-square" alt="Stars"></a>
  <a href="https://github.com/miralabs-tech/standardoc/releases"><img src="https://img.shields.io/github/downloads/miralabs-tech/standardoc/total?label=release%20downloads&style=flat-square" alt="Release downloads"></a>
  <a href="https://marketplace.visualstudio.com/items?itemName=miralabs-tech.standardoc-vscode"><img src="https://img.shields.io/visual-studio-marketplace/i/miralabs-tech.standardoc-vscode?label=vscode%20installs&style=flat-square" alt="VSCode installs"></a>
  <a href="https://open-vsx.org/extension/miralabs-tech/standardoc"><img src="https://img.shields.io/open-vsx/dt/miralabs-tech/standardoc?label=ovsx%20downloads&style=flat-square" alt="OpenVSX downloads"></a>
</p>

> **Your AI agent re-reads your whole codebase on every task.** Standardoc
> indexes it once into a shared, always-live map of your code — so the
> agent (and your other tools) just *ask* instead of re-grepping. ~100
> tokens per question instead of 30k. Local, open-source, derived straight
> from your source.

📖 English · [Français](.important/fr/README.md)

[About](.important/en/ABOUT.md) · [Quickstart](.important/en/QUICKSTART.md) · [Roadmap](.important/en/TODO-LIST.md) · [Comparison](.important/en/COMPARISON.md) · [FAQ](.important/en/FAQ.md) · [Support](.important/en/SUPPORT.md) · [Changelog](CHANGELOG.md)

---

## What is it?

Standardoc reads your code straight from its syntax tree (Rust,
TypeScript & JavaScript with React/JSX/TSX, Vue, Svelte, Lua) and keeps a
**living semantic graph** of it — every symbol, and the typed links between
them: who calls who, what imports what, what implements what. A file watcher
keeps it current as you edit.

Your tools plug into that one graph instead of each re-parsing your code:

- **For AI agents** — an MCP server with focused, read-only graph queries
  (Claude Code, Cursor, Continue, Cody, Aider, Goose, any MCP client).
  ~100 tokens per question instead of 30k of grep + read.
- **For editors** — an LSP daemon any client can connect to (the official
  VSCode extension embeds it; IntelliJ, Neovim, Helix, Emacs eglot too).
- **Coming** — docs and visual navigation generated from the same graph.

The graph is the shared asset. Nobody re-parses your code on the side.

---

## Why it's built this way

It optimizes for the questions you ask **after 6 months** on a big
codebase, not the 2-minute demo:

- **One graph, not N.** Every tool re-parsing your code is one more point
  of drift.
- **Direct AST, no regex.** Heuristics rot the moment the code moves.
- **Local & open-source.** FSL-1.1-MIT, auto-converting to plain MIT (first
  release: April 26, 2028). No cloud, no lock-in, no rented graph.

→ The longer story: [`storytelling/`](.important/en/storytelling/).

---

## Install

**VSCode extension** (recommended — embeds the daemon, generates the AI
skill, writes `.mcp.json`):

Search for "Standardoc" in the Extensions panel, or grab the `.vsix` from
the [releases](https://github.com/miralabs-tech/standardoc/releases).

**Standalone CLI** (without VSCode):

```sh
cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli
standardoc --version
```

Then, from a workspace, wire it into your agent in one command:

```sh
standardoc init        # writes the AI skill, MCP-first hooks, AGENTS.md, and .mcp.json
```

This makes a bare **Claude Code CLI** user first-class: `init` writes the
AI skill, the MCP-first hooks, an `AGENTS.md` section, and a `.mcp.json`
that runs `standardoc mcp --connect` — a thin bridge keeping one live,
watcher-backed daemon for the workspace. Re-running `init` is safe (every
merge preserves your own content). `.mcp.json` holds machine-specific
paths — add it to `.gitignore` if collaborating.

→ [Full 5-minute walkthrough (QUICKSTART)](.important/en/QUICKSTART.md)

---

## Who is it for?

Big, complex codebases — compilers, languages, engines, heavy monorepos,
multi-team infra. It works on a small project too, but that's not where the
value is; there, `ripgrep` + your IDE are enough.

The problem it solves: **keeping a stable, non-drifting co-work with an AI
agent on a codebase that keeps changing.** Agents forget context between
sessions, re-grep what they could query, and invent code that looks like
yours but breaks your invariants. Standardoc answers that two ways — the
**graph** (the agent queries real structure instead of guessing) and the
**discipline** (a hook stops it from shortcutting to grep before it has
checked the graph).

It's a tool for the dev, not a replacement: a stable graph makes a good dev
faster; it won't rescue a codebase nobody understands.

---

## Support

Standardoc is a [**StandarX**](https://opencollective.com/standarx)
project — an organization building open-source tools focused on code
intelligence, infrastructure, and AI agents.

If Standardoc saves you time:

[Star the repo](https://github.com/miralabs-tech/standardoc) · [Support StandarX on OpenCollective](https://opencollective.com/standarx) · [Other ways](.important/en/SUPPORT.md)

---

## License

[**FSL-1.1-MIT**](LICENSE) — Functional Source License v1.1 with
automatic conversion to plain MIT. Each release becomes MIT 2 years after
its release date; the first release converts on **April 26, 2028**. Free
for any non-competing use today, fully MIT from those dates on.
