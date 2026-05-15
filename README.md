# Standardoc

<p align="center">
  <a href="https://github.com/miralabs-tech/standardoc/releases"><img src="https://img.shields.io/badge/status-beta-yellow?style=flat-square" alt="Status: beta"></a>
  <a href="https://github.com/miralabs-tech/standardoc/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/miralabs-tech/standardoc/ci.yml?branch=main&label=ci&style=flat-square" alt="CI"></a>
  <a href=".important/en/ABOUT.md"><img src="https://img.shields.io/badge/core-canonical%20IR%20%2B%20live%20graph-blueviolet?style=flat-square" alt="Core: canonical IR + live graph"></a>
  <a href=".important/en/QUICKSTART.md"><img src="https://img.shields.io/badge/surfaces-LSP%20·%20MCP%20·%20RAG-blue?style=flat-square" alt="Surfaces: LSP · MCP · RAG"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-FSL--1.1--MIT%20→%20MIT%202028-green?style=flat-square" alt="License: FSL-1.1-MIT → MIT 2028"></a>
  <a href="https://github.com/miralabs-tech/standardoc/stargazers"><img src="https://img.shields.io/github/stars/miralabs-tech/standardoc?label=stars&style=flat-square" alt="Stars"></a>
  <a href="https://github.com/miralabs-tech/standardoc/releases"><img src="https://img.shields.io/github/downloads/miralabs-tech/standardoc/total?label=release%20downloads&style=flat-square" alt="Release downloads"></a>
  <a href="https://marketplace.visualstudio.com/items?itemName=miralabs-tech.standardoc-vscode"><img src="https://img.shields.io/visual-studio-marketplace/i/miralabs-tech.standardoc?label=vscode%20installs&style=flat-square" alt="VSCode installs"></a>
  <a href="https://open-vsx.org/extension/miralabs-tech/standardoc"><img src="https://img.shields.io/open-vsx/dt/miralabs-tech/standardoc?label=ovsx%20downloads&style=flat-square" alt="OpenVSX downloads"></a>
</p>

> A code intelligence infrastructure, built on a canonical multi-language
> IR and a living semantic graph. One graph, several daemons (LSP, MCP
> stdio + HTTP/SSE), all your tools plugged into it. ~100 tokens per
> agent query instead of 30k of grep + read. Local, derived from source
> code, open-source.
>
> Built for what breaks at scale and turns unmanageable in 6 months, not
> for the 2-minute demo. Code understanding is a system, not a string of
> greps.

📖 English · [Français](.important/fr/README.md)

[About](.important/en/ABOUT.md) · [Quickstart](.important/en/QUICKSTART.md) · [Roadmap](.important/en/TODO-LIST.md) · [Comparison](.important/en/COMPARISON.md) · [FAQ](.important/en/FAQ.md) · [Support](.important/en/SUPPORT.md) · [Changelog](CHANGELOG.md)

---

## What is it?

Standardoc indexes your code into a **living semantic graph**:

- Direct AST, multi-language (Rust, TypeScript & JavaScript with React/JSX/TSX, Vue, Svelte, Lua today)
- Unified canonical IR — node types + typed edges shared cross-language
  (`CALLS`, `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`, `DEFINES`,
  `USES_TYPE`, `EXPOSES_API`), with structured attributes on some edges
- SQLite + FTS5, filesystem watcher, BLAKE3 invalidation, versioned schema
- Derived from the code (not another source to maintain on the side),
  reproducible on any machine in seconds

**Several surfaces consume this state**:

- **LSP daemon** (`standardoc lsp`, stdio, primary writer of the graph) —
  the official VSCode extension embeds it; any LSP client can connect
  (IntelliJ, Neovim, Helix, Emacs eglot, …) by pointing at the binary
- **MCP daemon** (`standardoc mcp`, stdio or HTTP/SSE multi-client,
  readonly) — 16 tools for Claude Code, Cursor, Continue, Cody, Aider,
  Goose, and any MCP client
- **RAG layer** (`.standardoc/rag.db`, linked to the graph by FQDN) —
  prose chunks (`README.md`, `docs/`, `notes/`, ABOUT, etc.) re-ranked
  through a Candle/BGE-small embedder, reachable from both daemons (via
  the `fetch_chunks` MCP tool or the `chunk_refs` of `get_context`)
- **Sessions DB** (`.standardoc-sessions/sessions.db`, orthogonal to the
  graph) — persistent agent memos across chats, accessed through the
  `session_*` MCP tools. Human content, not derived from code
- *Coming* — static docs generated from the graph (`@standardoc/react`
  + Nextra/Docusaurus/Astro adapters), visual navigation, language
  plugins via UST + Lua

**The result**: your tools stop re-parsing your code each on their own
side. The graph is the shared asset. ~100 tokens per agent query instead
of 30k of grep + read.

---

## Posture

Standardoc optimizes for the questions you ask **after 6 months** on a
monorepo, not for the 2-minute demo:

- *What stays stable despite the changes?* → **canonical IR** (languages
  mutate, the IR doesn't)
- *Which choices become irreversible?* → **open-source FSL-1.1-MIT** that
  becomes plain MIT on April 26, 2028 (no SaaS lock-in, no retroactive
  change of terms possible)
- *What creates cognitive debt?* → **a shared graph** (N tools re-parsing
  your code = N points of desync)
- *What breaks at scale?* → **direct AST** (no regex or heuristics that
  rot fast)
- *What becomes incomprehensible in 6 months?* → **MCP-first guardrail**
  (an agent that greps 30k tokens on every task is neither comprehensible
  nor debuggable)

Code understanding is a system, not a string of greps. Standardoc is the
infrastructure for that system.

→ Details in [`.important/en/storytelling/`](.important/en/storytelling/):
philosophy, short/mid/long-term vision, dogfood observations, test
feedback.

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

→ [Full 5-minute walkthrough (QUICKSTART)](.important/en/QUICKSTART.md)

---

## Who is it for?

Standardoc is built for **large, complex codebases** — designed by
dogfooding on Standardoc itself and calibrated for projects of the same
caliber: compilers, programming languages, engines (game / runtime / db),
heavy application monorepos, multi-team infra. Not for the weekend JS app
— it'll still work, but that's not where the value is strongest.

The core problem it solves:
**keeping a stable, controlled, non-drifting co-work with an AI agent on
a codebase that evolves**. It's the problem nobody else tackles head-on
today — most tools stop at "give it the context of one session", not
"hold coherence over 6 months".

AI agents drift: they forget context from one session to the next,
re-grep what they could have queried, invent code that looks like yours
without respecting your invariants, don't remember the decision locked
last week. Every task, the archaeology starts over — and the bigger the
project gets, the more the archaeology costs (tokens, patience, subtle
bugs, human cognitive debt).

Standardoc addresses this from 3 complementary angles:

- **The graph** — the agent queries the real structure (FQDN, edges,
  body, RAG prose), it doesn't invent
- **The discipline** — the `MCP-first guardrail` stops the agent from
  shortcutting to `grep + read`; the PreToolUse hook forces it through
  the graph before anything else
- **The memory** — locked decisions from one session survive in the
  sessions DB (`session_save` / `session_get`); the agent recovers the
  context at the next session instead of rediscovering everything

Standardoc is an AI-dev co-work tool, **not a substitute for the dev**.
An agent querying a stable semantic graph is powerful; a dev who doesn't
understand their code will stay frustrated whatever the AI behind it.

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
