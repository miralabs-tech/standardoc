# FAQ

📖 English · [Français](../fr/FAQ.md)

[Quickstart](QUICKSTART.md) · [Philosophy](storytelling/philosophy.md) · [Short-term vision](storytelling/vision-short-term.md) · [Comparison](COMPARISON.md) · [Support](SUPPORT.md)

---

## Is it a documentation tool?

Not yet, and not in the TypeDoc / JSDoc sense. Today Standardoc is a
**semantic indexer** — a living code graph exposed via MCP / LSP.
The *rendered* docs (static sites, `<Doc id>` components) is a
consumption layer planned for **beta.3**: `@standardoc/core`
(framework-agnostic query API) + `@standardoc/react` (the first
renderer, Next / Nextra / Astro / Docusaurus adapters). The graph is
already ready to serve it; the rendering just isn't written yet. See
[mid-term vision](storytelling/vision-mid-term.md).

## Does it replace my LSP?

No, it **complements** it. Standardoc *exposes* LSP as one of its
consumption surfaces, but under the hood it's a global cross-language
graph, not a per-language server. `rust-analyzer` / `tsserver` keep the
deep per-language resolution (complex type inference, macro expansion,
contextual completion); Standardoc brings the cross-cutting graph + the
MCP surface. Use both. Post-1.0, an optional bridge to
`rust-analyzer` / `tsserver` is even on the table to merge the two views
through a single MCP interface — cf. [long-term
vision](storytelling/vision-long-term.md).

## How is it different from Sourcegraph?

Sourcegraph is a shared full-text + symbol search engine for teams,
hosted (cloud SaaS), with a product focus on collaboration and code
review. Standardoc is a **local semantic indexing infrastructure**,
multi-surface, focused on AI agents + multi-frontend. No cloud, no
auth, no per-user billing — the index lives in `.standardoc/` on your
machine. The two can coexist on the same monorepo: they don't address
the same problem. Detailed grid in [COMPARISON.md](COMPARISON.md).

## Why not just Tree-sitter or ripgrep directly?

On a small SPA, `ripgrep` & your IDE LSP are largely enough — we own
that ([philosophy](storytelling/philosophy.md)). Tree-sitter as
standalone gives a **surface** AST (functions, classes, calls — close
to regex). Standardoc uses **deep** AST parsers: `syn` (Rust), `swc`
(TS / JS / JSX / TSX), `full_moon` (Lua), custom SFC parsers (Vue,
Svelte) — full signatures, types, generics, traits, modifiers, typed
edges. It's the central differential against the textual approaches.

Nuance: tree-sitter **will come back** post-1.0, but as a universal
parser *under* the UST + Lua plug-in layer (parsing delegated to
tree-sitter, semantics in Lua, IR validation in Rust) — not as a
surface-indexing engine. See [long-term
vision](storytelling/vision-long-term.md).

## What's the difference vs `Read` / `Grep` / `Glob` from my agent?

The native tools of an agent answer **text-level** questions. Standardoc
answers **graph-level** questions — callers, callees, imports, type
relations, cross-language edges — without the agent having to
reconstitute these facts from text scans. Order of magnitude observed in
dogfood: **~100 tokens per question instead of ~30k**. The
auto-generated agent skill at workspace init instructs the agent to use
Standardoc first, and to fall back on `Read` / `Grep` / `Glob` only when
the graph can't answer (true string-literals: comments, out-of-code
configs, build files).

## Which languages are supported?

Three native language providers: **Rust** (`syn`), **TypeScript /
JavaScript** including **JSX & TSX — React** (`swc`), **Lua**
(`full_moon`). **Vue** and **Svelte** are also supported via SFC
parsing: a component's `<script>` is extracted and handed to the TS
provider, and the `<template>` goes through custom SFC parsers. These
are the languages that carried Standardoc through dogfood up to 1.0 —
the criterion isn't the number of languages, it's AST depth. Two
languages well supported are worth more than ten half-done.

## When will language X (Python / Go / Java / C…) be supported?

Not before 1.0 in the core. The strategy is **not** to pile up built-in
Rust providers indefinitely — every provider is a significant PR on the
core, with long-term maintenance on me. Post-1.0, adding a language
goes through the **UST + Lua plug-in layer**: tree-sitter parses, a
sandboxed Lua plug-in defines symbols / edges / attributes, the Rust
core validates conformance to the IR schema. Adding Go / Java / Swift /
Python / C / C++ becomes a `.lua` file dropped in the workspace, not a
core PR. The core keeps its native providers (Rust / TS-JS / Lua, + the
SFC support for Vue / Svelte); everything else goes through plug-in.
See [long-term vision](storytelling/vision-long-term.md).

## Does it work with an agent other than Claude?

Yes — Standardoc is a standard MCP server, consumable by any MCP-aware
client (Cursor, Continue, Copilot Chat, Aider, Goose, Cody, Claude
Desktop, Claude Code…). **But** the reference calibration is done on
**Claude Code in Opus mode**, 1M-token window. The other agents work,
with gaps: some shortcut to grep the moment the task gets complicated,
or ignore the corrective `routing_hint`.
The calibration is **tripartite** — infrastructure + agent + operator.
The MCP-first hooks (on the Claude Code side) enforce the discipline;
for another client you can wire equivalent hooks (`standardoc claude
pre-tool-hook`). Details in [test
feedback](storytelling/test-feedback.md).

## How do I install?

Two paths. **Pre-built binaries** (recommended channel) — download the
archive matching your platform from
[releases/latest](https://github.com/miralabs-tech/standardoc/releases/latest),
the `version.json` manifest lists the SHA256s for verification. **OR
`cargo install --git`** for a source build. For the integrated VSCode
flow, also install the Standardoc extension. Full walkthrough:
[QUICKSTART.md](QUICKSTART.md).

> `cargo install standardoc-cli` from crates.io is **not** the primary
> channel — too slow for CI, requires a Rust toolchain.

## Is the VSCode extension mandatory?

No. The CLI works standalone: `standardoc lsp <ws>` (primary writer) +
`standardoc mcp <ws> --readonly` (stdio transport, or `--http 0` for
multi-client HTTP/SSE), and you connect Claude Desktop / Cursor / the
MCP client of your choice. The extension just makes the flow seamless
in VSCode — daemon supervision, opt-in init flow, skill generation,
`.mcp.json` merge. The CLI will become even more self-sufficient in
beta.3 (`self-update`, PATH injection, one-liner bootstrap).

## Is my code sent anywhere?

No. **Standardoc is local-only**, unconditionally. The index lives in
`.standardoc/` on your disk (gitignored, reproducible). No network call
to index, no telemetry, no phone-home — **ever, even opt-in**: it's a
non-negotiable
cultural invariant. If Standardoc disappears tomorrow, your index keeps
working.

## How does it perform on large workspaces?

Native AST + SQLite + FTS5 + incremental watcher: cold start counts in
seconds on a medium-sized repo (Standardoc indexes itself in a few
seconds), the watcher overhead during edits is negligible. The
**published scale benchmarks** — cold start, watcher delta, MCP query
latency p99 on 1M+ LOC monorepos — arrive at **1.0**: run in CI,
attached to releases, regressing visibly when we break them. No "it
scales, trust us": the numbers will be there before we freeze the
contract.

## Is Standardoc paid? Will there be a SaaS or a subscription?

The core is and stays **free and open-source**. **No SaaS, no
subscription, no cloud, no telemetry** — as long as Standardoc has no
server component that demands recurring infrastructure, there is no
reason to charge a subscription, and none is planned. If a paid tier
ever emerges (for example a post-1.0 local doc UI), it would be
**local-only** (runs on your machine, no hosting) and on a **lifetime
license, one-time purchase** — and only if there's real demand. The
core, for its part, stays FSL-1.1-MIT → MIT unchanged. The internal
discussions about future funding stay out of public discourse until
1.0 — see [SUPPORT.md](SUPPORT.md) for the current model
(OpenCollective).

## Why FSL-1.1-MIT and not plain MIT?

[FSL-1.1-MIT](../../LICENSE) is permissive for any **non-competing
use** and stops the "open-and-pillage" pattern (a closed-source
competitor that forks without publishing anything). Plain MIT was ruled
out — no short-term protection; AGPL too — insufficient against
non-SaaS competitors. FSL is the only mechanism that combines initial
protection **and** an irreversible commitment to openness: **two years
after each release, that release converts automatically to plain
MIT**. The first one (`v1.0.0-beta.1`) converts on **April 26, 2028**.
From then on the core is legally MIT forever — whatever happens to the
company, the maintainer, the market. Adopted by Sentry, CodeCrafters,
Keygen.

## Can I use it commercially?

Yes, freely — internal tooling, customer-facing apps, SaaS products
that *use* Standardoc. The only limit: you don't build a product that
**substitutes for Standardoc itself** (resell it as your own indexing
SaaS). See the [license](../../LICENSE) for the details.

## What about the v0 DSL / the doc rendering?

The v0 templating DSL (`{{ @doc.X }}` expressions in Markdown) was
**dropped** in beta.1 — it was becoming a second source to maintain,
unreadable for human authors. The replacement, targeting **beta.3**,
doesn't reinvent a DSL: `@standardoc/core` (query API) +
`@standardoc/react` (`<Doc id>`, `<Params id>`, `<Examples id>`
components) consume the **graph directly**, which stays the only source
of truth. The narrative annotations rely on the already-universal
conventions — JSDoc, rustdoc, emmylua (`---@param`) — not on a custom
format. See [mid-term vision](storytelling/vision-mid-term.md).

## Can I contribute?

Before the 1.0 freeze: **no third-party PRs**. The API surface has to
freeze cleanly, and accepting external PRs now would introduce noise on
choices I have to keep controlled to carry the IR contract. **On the
other hand, issues / feedback / technical or global ideas are very
welcome** via GitHub Issues / Discussions — that's where the holes
surface fastest. Post-1.0, the model opens up: the UST + Lua plug-in
layer is precisely designed to absorb community contributions
(languages, cross-substrate detectors) without touching the frozen
core.

## How do I report a bug or a security issue?

Bugs and feature requests: [GitHub
Issues](https://github.com/miralabs-tech/standardoc/issues). **Security
issues**: don't post them publicly — follow the responsible disclosure
procedure described in [SECURITY.md](../../SECURITY.md).

---

## Going further

- **[Quickstart](QUICKSTART.md)** — from zero to an indexed workspace
- **[Philosophy](storytelling/philosophy.md)** — the 5 system-thinking
  principles and the construction ethics
- **[Comparison](COMPARISON.md)** — vs LSP / Sourcegraph / Tree-sitter
  / others
- **[Support](SUPPORT.md)** — how to support the project
- **[TODO-LIST](TODO-LIST.md)** — exhaustive checkboxes per milestone
