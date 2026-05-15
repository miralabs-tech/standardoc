# Comparison

📖 English · [Français](../fr/COMPARISON.md)

[Quickstart](QUICKSTART.md) · [Philosophy](storytelling/philosophy.md) · [FAQ](FAQ.md) · [Support](SUPPORT.md)

How Standardoc positions against adjacent tools — those you might
already use, and those you'll be quoted in response to "isn't this
already done?".

> This page tries to be **honest, not flattering**. Several of the
> tools below are excellent at what they do. The question isn't "which
> is best" — it's "which solves *which* problem". See also
> [philosophy](storytelling/philosophy.md) for the underlying framing.

---

## The Standardoc axis

Before the grid, the marker. Standardoc is a **semantic indexing
infrastructure** with five properties held as invariants:

- **Local** — the index lives in `.standardoc/`, not in a cloud
- **Deep AST** — `syn` / `swc` / `full_moon` / SFC parsers, not
  surface-level tree-sitter or regex
- **Multi-surface** — one graph consumed by LSP, MCP, RAG, and soon
  rendered docs (beta.3)
- **Agent-first** — the MCP surface and the encoded discipline
  (MCP-first hooks, sessions DB, `routing_hint`) are designed for an
  AI agent
- **Irreversible OSS** — FSL-1.1-MIT with automatic conversion to
  plain MIT 2 years per release

Most of the tools compared here hold one or two of these properties.
None hold all five — that's precisely the niche.

---

## Comparison grid

| Tool | Hosting | License | Parsing | Cross-language graph | Agent surface | Price |
| --- | --- | --- | --- | :---: | --- | --- |
| **Standardoc** | Local | FSL → MIT | Deep AST | ✅ canonical IR | Native MCP | Free |
| **code-review-graph** | Local | MIT | Surface AST (tree-sitter) | ⚠️ multi-language | Native MCP | Free |
| **Sourcegraph** | Cloud SaaS | Proprietary | SCIP / compiler-accurate | ✅ | via Cody | ~$49–59/user/mo |
| **Serena** | Local | MIT | Per-language LSP | ❌ per-language view | Native MCP | Free |
| **Aider (repo map)** | Local | Apache 2.0 | Surface AST (tree-sitter) | ⚠️ file graph | Built-in (no MCP) | Free |
| **Continue** | Local | Apache 2.0 | Vector RAG | ❌ no graph | MCP framework | Free |
| **SCIP / Glean / Kythe** | Self-host | OSS (Apache) | Compiler-accurate | ✅ | ❌ format / infra | Free |
| **LSP** (rust-analyzer, tsserver) | Local | OSS | Per-language deep AST | ❌ per-language | ❌ IDE protocol | Free |
| **TypeDoc / JSDoc / Sphinx** | Local | OSS | AST + required annotations | ❌ | ❌ | Free |
| **GitBook** | Cloud SaaS | Proprietary | — (manual prose) | ❌ | ❌ | Freemium paid |

Legend: ✅ first-class · ⚠️ partial / it depends · ❌ absent.

The grid is only a summary — the paragraphs that follow explain what no
column captures.

---

## Code intelligence SaaS — Sourcegraph

Sourcegraph is the **anti-Standardoc**, and its trajectory is worth as
a counter-example.

Originally open-source (Apache 2.0), it was a shared-team code search
engine, excellent at its job. Then: relicensed in 2023 to a proprietary
"Sourcegraph Enterprise" license, code moved to a private repo in 2024
(no longer even source-available), full pivot around **Cody** (AI
assistant), investment stopped on the code search product itself. The
Free and Pro tiers of Cody were closed in 2025 — the offering is now
**enterprise-only, ~$49–59 per user per month**, hosted, with a product
focus on collaboration and code review at large-organization scale.

It's a coherent product for what it aims at. But it contradicts
**every** Standardoc invariant: cloud instead of local, proprietary
instead of irreversible OSS, per-seat billing instead of free, and a
graph you *rent* instead of own. If the company pivots, changes
pricing, or shuts down, you lose the asset.

The two can coexist on the same monorepo — they don't address the same
problem. But if your need is "understand my own code, locally, without
depending on a vendor", Sourcegraph 2026 is no longer the answer it was
in 2020.

> **SCIP / Glean / Kythe** — in the same "indexing infrastructure"
> family, but on the format and back-end side: SCIP is Sourcegraph's
> indexing protocol (the LSIF successor), Glean is Meta's, Kythe is
> Google's. They are compiler-accurate and cross-language, but
> **batch-oriented**, heavy to operate, and designed for large-company
> internal indexing — not for a live index on a solo dev's machine, not
> agent-first, no MCP surface. Standardoc plays in the same conceptual
> court, at the opposite scale: light, local, alive, consumable by an
> agent in a single tool call.

---

## The closest neighbor — code-review-graph

[`code-review-graph`](https://github.com/tirth8205/code-review-graph)
(tirth8205) is, by far, **the closest ideological neighbor** to
Standardoc. It deserves a frank and detailed comparison — all the more
so because it's serious, well-built, and saw **the same underlying
problem**.

### What we share

On the diagnosis, we agree:

- AI agents burn tokens re-scanning the codebase on every task
- The answer is a **local persistent code graph** consumed through a
  stable surface
- Structural preprocessing **before** LLM inference, not the other way
  around

On the posture too:

- Local SQLite, no cloud, no telemetry
- MCP-native, incremental, hash-based watcher
- Permissive licenses (MIT on their side, FSL → MIT on ours)
- Multi-language as a baseline

It's exactly the same family of ideas. It's not an ideological
adversary like Sourcegraph — it's a project that saw the same illness
and tries to answer it.

### Epistemology: resolved structure vs scored supposition

Where things clearly diverge is on **how we build the graph**.

**code-review-graph** = tree-sitter AST (real CST) **+ a scored
analytic layer on top**:

- 3-tier edge confidence (`EXTRACTED` / `INFERRED` / `AMBIGUOUS`) with
  float scores
- Leiden communities to *guess* implicit bounded contexts
- Betweenness centrality to *guess* architectural hubs and bridges
- Surprise scoring to *flag* unexpected coupling (cross-community,
  cross-language, peripheral-to-hub)
- Knowledge gap analysis, generated refactoring suggestions

It's a **probabilistic epistemology over a syntactic graph**: "this
edge has confidence X", "this node is probably a chokepoint with score
Y". An architecture qualifiable as **structural mining** — analytic
fouille over what tree-sitter could extract.

**Standardoc** = compiler-grade parsers that resolve by construction:

- `syn` reads the Rust type system (generics, traits, lifetimes,
  modifiers, full signatures)
- `swc` reads the full TypeScript / JavaScript / JSX / TSX semantics
- `full_moon` reads Lua with emmylua extraction
- Custom SFC parsers for Vue / Svelte (the extracted `<script>` → TS
  provider)

What enters the canonical IR is **not inferred with a score** — it's
what the language **says**, by construction. An `EdgeKind::Calls` is
laid down because the compiler-grade parser saw a call; an
`EdgeKind::UsesType` because the type system resolved it. When we
don't know, we mark `Unresolved { name }` rather than guessing — the
downstream agent can decide what to do with it.

Two different epistemologies — **probabilistic vs resolved**. Neither
better nor worse in the absolute: they give breadth (24 languages at
the surface); we give depth (3 languages + Vue/Svelte SFC in depth).
The tradeoff is explicit.

### Product bet and time horizon

**code-review-graph** bets on **breadth + structural mining + platform
reach**:

- **24 languages** via tree-sitter (TS, JS, Python, Rust, Go, Java,
  Scala, C#, Ruby, Kotlin, Swift, PHP, Solidity, C/C++, Dart, R, Perl,
  Lua, Zig, PowerShell, Julia, Nix, Vue, Svelte, Jupyter / Databricks
  `.ipynb`)
- **12 AI platforms auto-detected** at `install` (Codex, Claude Code,
  Cursor, Windsurf, Zed, Continue, OpenCode, Antigravity, Qwen, Qoder,
  Kiro, GitHub Copilot + CLI)
- **Wide product surface**: interactive D3.js visualization, multi-repo
  daemon (`crg-daemon`), semantic search via multi-provider embedders
  (sentence-transformers, Gemini, MiniMax, OpenAI-compatible), exports
  to GraphML / Neo4j Cypher / Obsidian / SVG, wiki generation, 5 MCP
  prompt templates (review, architecture, debug, onboard, pre-merge)
- **Python 3.10+ stack**, permissive MIT license, community-flavored
  ecosystem (Discord, opinionated multi-platform install)

Bet: *scored structural preprocessing → LLM reasoning downstream, on
the maximum stack and platforms possible*.

**Standardoc** bets on **depth + contract + repository cognition
primitives**:

- **3 deep language providers** + Vue/Svelte SFC; the rest goes
  post-1.0 through the **UST + Lua** plug-in layer (community-driven,
  not core)
- **Versioned canonical IR** (`standardoc-ir`) with **contractual API
  freeze at 1.0**: MCP tool signatures, LSP custom methods, IR types,
  SQLite schema. `protocol_version` bump + coexistence for any later
  breaking change
- **`BridgeKind`** = opaque cross-substrate primitive attached to
  edges and signatures (Tauri / WASM / FFI / SQL / ORM / DB-table /
  DB-model); vocab to freeze at 1.0, frontend detectors post-1.0 via
  the plug-in layer
- **Orthogonal sessions DB with 4 kinds** (`Session` / `Feedback` /
  `Profile` / **`Lock`**) with bidirectional `.md` ↔ DB sync; the
  `Lock` is the **ADR equivalent** persisted (Architecture Decision
  Record in memo format)
- **RAG layer** linked to the graph by FQDN with `relink_watcher`
  re-anchoring prose chunks at every graph revision — the prose ↔
  structure link evolves with the code
- **Encoded and tested discipline**: `compute_routing_hint` with 4
  dedicated tests (silent depth=1, fires naked depth=2, silent after
  recent depth=1, fires again after window expires); PreToolUse hook
  **blocking** (deny) rather than purely advisory; SessionStart wipe
  to start strict
- **Irreversible license-as-moat**: FSL-1.1-MIT with automatic
  conversion to plain MIT 2 years per release; first conversion **April
  26, 2028**, non-negotiable temporal lock
- **Rust stack**, solo-maintainer pre-1.0 contributor lockdown (issues
  / feedback OK, third-party PRs refused until the freeze)

Bet: *a stable, contractual semantic substrate that humans + CI +
agents + future renderers / plug-ins consume over 5+ years, with
primitives already laid toward what others call "repository cognition"
without a marketing claim.*

### What we don't have

Necessary honesty — features where they're objectively ahead:

- **Interactive graph visualization** — D3.js force-directed with
  search, community legend, degree-scaled nodes. Our visual navigation
  webview is a beta.3 candidate, not shipped.
- **Native multi-repo daemon** — `crg-daemon` supervises several
  workspaces from a single process; on our side each workspace has its
  own LSP / MCP daemon pair.
- **Multi-provider semantic search** — they support
  sentence-transformers, Gemini, MiniMax, OpenAI-compatible. Our RAG
  runs with a single local embedder (Candle/BGE-small) — a deliberate
  choice to stay local-only with no API key or network call, but flatly
  it's fewer options.
- **Out-of-IDE exports** — GraphML (Gephi/yEd), Neo4j Cypher, Obsidian
  vault with wikilinks, static SVG. On our side the graph is queried
  via MCP / LSP / CLI but doesn't export to third-party formats.
- **Scored architectural analytics** — Leiden communities, betweenness
  centrality (hubs/bridges), surprise scoring, generated refactoring
  suggestions. These metrics don't exist on our side, **by consistency
  with the epistemological bet** (exact structural resolution, not
  scored analytic mining) — not an accidental gap.

### Target and angle

**code-review-graph** is what we have most complete today for:
token-efficient AI coding on **a maximum of languages and platforms**,
with a scored graph that reduces `what to read`, and architectural
analytics (communities, hubs, bridges) as a bonus. Review stays its
most visible use case (the name, the `review-delta` / `review-pr`
slash commands, benchmarks on commits) but the tool now goes beyond
pure review (architecture / debug / onboard / pre-merge are
first-class MCP prompt templates).

**Standardoc** is purpose-built for: **AI-dev co-work coherent over
the long run** on **heavy and complex monorepos**, with a resolved
semantic substrate, contractualized at 1.0, whose graph is a **shared
asset** (humans, CI, agents, future renderers, future plug-ins) rather
than a token-optimization cache. The repository cognition primitives
(`BridgeKind`, `SessionKind::Lock`, FQDN-linked RAG, enrichments with
`ConfidenceLevel`) are laid down **before** the conventions that fill
them are frozen.

The two projects mostly prove the same thing: the problem — AI agents
re-scanning on every task — is real, and the answer is a shared local
graph. **We solve it under two different bets on the future of the AI
↔ codebase problem.**

---

## Agents with built-in context — Serena, Aider, Continue

Three tools that give an agent code context, through three different
mechanisms — and none is a multi-surface shared semantic index.

- **Serena** — an OSS MCP that **wraps language servers** (LSP) to
  offer symbol-level navigation and editing across 30+ languages.
  Solid and token-efficient. But the view stays **per-language** (the
  underlying LSP's), there's no clean cross-language graph, no
  canonical IR, no persistent index reusable outside the agent, no
  RAG over prose. Serena is an *LSP adapter for agents*; Standardoc is
  an *indexing infrastructure* of which LSP is just one surface among
  others.

- **Aider (repo map)** — extracts symbols via tree-sitter and ranks
  files by importance (a PageRank-like algorithm on the file
  dependency graph), within a tunable token budget. Effective — but
  **per-chat ephemeral**, integrated into Aider, not a persistent
  index, not an MCP surface reusable by other tools.

- **Continue** — **vector RAG**: the codebase is split into chunks,
  embedded, stored in a vector DB, and the chunks most *semantically
  similar* to the task are surfaced. It's a **similarity** approach,
  not a **structure** one: no graph, no typed edges, no FQDN
  resolution. (Standardoc also uses RAG — but *as a complement* to the
  graph, on adjacent prose linked by FQDN, never as a substitute for
  structure.)

These three can even cohabit with Standardoc: they're context
consumers, Standardoc is the producer of structured context they could
consume.

---

## LSP — complementary, not a competitor

`rust-analyzer`, `tsserver`, `vue-language-server` give a **deep**
per-language resolution: hover, go-to-definition, find-references,
rename, type inference, macro expansion. Standardoc doesn't replace
that — it actually *exposes* LSP as one of its surfaces.

The difference is structural: an LSP is **per-IDE and per-language**,
its graph is rebuilt on every opening and dies between sessions, and
its API is designed for a human clicking in an editor — not for an
agent querying a **multi-language** codebase. Standardoc unifies Rust +
TS + JS + Vue + Svelte + Lua into a single, persistent cross-language
graph queryable via MCP.

**Use both.** LSP for editor precision; Standardoc for the
cross-cutting graph and agent queries. Post-1.0, an optional bridge to
`rust-analyzer` / `tsserver` is even on the table to merge the two
views (cf. [long-term vision](storytelling/vision-long-term.md)).

---

## Doc generators and platforms — TypeDoc, GitBook & co

`TypeDoc` / `JSDoc` / `Sphinx` answer "how do I generate a narrative
doc site from my code". They require **annotations everywhere**,
produce a **static rendering** that drifts from the next commit on, and
target human readers on a website. Standardoc indexes **any codebase
without annotation** (the AST is enough) and keeps the index **live**.

`GitBook` goes further on the SaaS side: a hosted platform, manual
prose, **no link to a code graph**. It's a documentation editor, not a
code understanding tool.

It's not really competition — it's a *future partial overlap*:
Standardoc's rendering layer (`@standardoc/core` + `@standardoc/react`,
beta.3) will consume **the graph directly** as the source of truth,
with adapters for Next / Nextra / Astro / Docusaurus. The goal isn't to
beat TypeDoc on its turf — it's to have docs that *can't* drift,
because they're derived from the graph and not maintained by hand. See
[mid-term vision](storytelling/vision-mid-term.md).

---

## When *not* to choose Standardoc

Honest answer, consistent with [the
philosophy](storytelling/philosophy.md):

- **Small SPA or a few-thousand-line project** → `ripgrep` & your IDE
  LSP are largely enough. Standardoc would be overkill.
- **Pure text search** (literal strings, comments, out-of-code configs)
  → that's the job of `grep` / `ripgrep`, and the agent skill
  explicitly says to fall back on it in those cases.
- **Read a known file at a known path** → just open it.
- **A language outside the native scope** (Rust / TS-JS / Lua, + Vue /
  Svelte in SFC) → wait for the post-1.0 UST + Lua plug-in layer, or
  index the supported part and document the rest.
- **You want a turnkey AI assistant** → Standardoc is *not* an agent.
  It provides the substrate; the agent stays Claude / Cursor /
  Continue / etc. (cf. [FAQ](FAQ.md)).
- **You want a team-shared code search with a collaboration UI** →
  that's Sourcegraph's job, accept the cost and the cloud.

Standardoc is purpose-built for the **deep semantic understanding of
code structure, locally, by a disciplined AI agent**. Strong where it's
strong, **overkill elsewhere**, and we own it.

---

## Going further

- **[Quickstart](QUICKSTART.md)** — from zero to an indexed workspace
- **[Philosophy](storytelling/philosophy.md)** — the 5 system-thinking
  principles and the construction ethics
- **[FAQ](FAQ.md)** — common questions
- **[Support](SUPPORT.md)** — how to support the project
- **[TODO-LIST](TODO-LIST.md)** — exhaustive checkboxes per milestone
