# Long-term vision — beyond 1.0

📖 English · [Français](../../fr/storytelling/vision-long-terme.md)

[← Mid-term vision](vision-mid-term.md) · [← Philosophy](philosophy.md) · [Notes](notes.md) · [Test feedback](test-feedback.md)

> This document describes the inflection Standardoc takes **after** 1.0.
> The list of ideas (with no calendar commitment) lives in
> [`TODO-LIST.md`](../TODO-LIST.md) — the *Post-1.0 ideas* section.

---

## The post-1.0 inversion: the core doesn't grow, the ecosystem does

At 1.0, the API is frozen. **All feature-addition pressure stops pushing
on the core** — it pushes on the ecosystem that wraps around it.

It's a strong philosophical choice. Until 1.0, the core grew naturally:
RAG, sessions DB, MCP toolkit, language providers, MCP-first hooks, the
doc rendering layer in progress. Each cycle added a piece of
infrastructure. **That's healthy during the construction phase.**
Maintained post-1.0, it produces a god-binary — a core that ends up
doing everything badly instead of doing its base job well.

The base job stays: **a living semantic graph + stable consumption
surfaces**. Everything that can be delegated must be delegated. The core
becomes minimal and stable; extensibility goes through a system of
external plug-ins.

It's not a retreat. It's the architecture that makes Standardoc durable
at 5 years and beyond.

---

## The UST + Lua plug-in layer — the central piece

**Honest framing: not a single brick of this plug-in layer exists in
code today.** mlua isn't integrated into the project; neither is
tree-sitter; no UST spec is written; no plug-in discovery. It's pure
roadmap. But the architecture is clear and the use cases already
identified in dogfood.

### The target architecture

```
source code
  → tree-sitter (universal parser, 100+ community grammars)
  → UST (Universal Symbol Tree — minimal common schema)
  → Lua plugin (sandboxed mlua) — defines symbols, edges, attributes
  → Rust core validates conformance to the IR schema + stores
```

Parsing is delegated to tree-sitter, the semantic transformation to
Lua, and the Rust core only validates conformance to the IR schema.
**Adding a language or a detector stops being a PR on the core**; it's a
`.standardoc/plugins/<lang>.lua` file dropped in the workspace or
shipped through a community channel.

### Why Lua rather than WASM

Three reasons:

1. **Lower barrier.** Lua reads in 30 minutes. Rust + WASM bindgen takes
   days. For a Prisma detector written by a community front-end dev to
   have a chance of existing, the cost of writing it has to be
   absorbable in one evening.
2. **mlua is proven in the StandarX ecosystem.** Other internal projects
   already use it as a sandboxed hooks engine. The capability-based
   sandboxing (no filesystem / network / process access by default) is
   solid.
3. **WASM stays the option for high-performance native plug-ins.** When
   a Lua parser becomes too slow (extreme case on very large
   monorepos), a compiled WASM binding can take over. For 95% of cases
   (declarative detectors, AST transformations), Lua is largely enough.

### Canonical use case 1: cross-substrate detectors

The whole substrate × language × ORM combinatorics that 1.0 refused to
freeze into the core (cf. [mid-term vision](vision-mid-term.md) —
*Cross-substrate bridges*). Tauri commands, WASM bindings, FFI
declarations, Prisma queries, Drizzle, SQLAlchemy, Mongoose, inline SQL
schemas, GraphQL resolvers, REST endpoints → DB tables.

Each detector = a Lua plug-in. The contributor writes their Prisma
plug-in once, shares it, **all projects using Prisma + Standardoc
benefit from it without touching the Rust core**.

### Canonical use case 2: safe-edit comment import/export

Reintroduction of the `materialize` command (punted in beta.1, cf.
[test feedback](test-feedback.md) — *What we dropped*) in a rigorous
version. The plug-in writes structured comments — doc-comments, `@doc`
blocks, JSDoc, rustdoc — **into the source**, **anchored on FQDN**
rather than on line ranges (more stable across refactors).

The end goal: maintain a **clean codebase** — bare code, just signature
+ body — with the docs living in the graph and the ability to
**re-inject locally on demand**, with no risk of desync between the docs
and the source.

---

## The mechanical expansion of languages

A direct consequence of the plug-in layer. Today Standardoc has 3
built-in Rust language providers (Rust via `syn`, TS / JS / JSX / TSX —
React included — via `swc`, Lua via `full_moon`), plus support for Vue
and Svelte via SFC parsing (the extracted `<script>` → TS provider, the
`<template>` via custom SFC parsers). Adding a new language today =
writing a Rust provider = a significant PR on the core, with long-term
maintenance on the maintainers' shoulders.

Post-1.0, **adding a language = writing a Lua plug-in on that
language's tree-sitter grammar**. The cost drops by an order of
magnitude. Consequences:

- **Go, Java, Swift, C#, Kotlin, Zig, Python** — all the languages
  where tree-sitter has a mature grammar become indexable without
  modifying the core
- **C and C++** — the LurLang dogfood case (a personal Rust + C
  language, the C not indexed at 1.0) finally gets an answer via a
  plug-in
- **Declarative DB schemas** — `schema.prisma`, `models.py`, SQL
  migrations become **frontends in their own right** parsed by Lua
  plug-ins. Their content (Table, Column, Model) enters the graph on the
  same footing as a code symbol, linkable by the ORM detectors
  (canonical use case 1)
- **Structured configs** — `.gql`, `.proto`, `openapi.yaml` can be
  indexed via plug-in if demand emerges

The core keeps its native providers (Rust / TS / Lua, + the SFC support
for Vue / Svelte) — the languages that carried Standardoc through
dogfood up to 1.0. **Everything else goes through plug-in.**

---

## The optional enriched surfaces

Standardoc at 1.0 exposes the graph via LSP, MCP, RAG, doc rendering
(beta.3), visual navigation (beta.3). Post-1.0, optional surfaces can
emerge depending on dogfood demand — always optional, never imposed.

### Custom LSP methods

Non-standardized LSP methods specific to Standardoc — for example
`standardoc/findCallers(fqdn)`, `standardoc/showEdges(fqdn)`,
`standardoc/checkStale(fqdns)`. The LSP clients that implement them gain
in richness. The LSP standard stays supported in parallel for universal
compat.

### LSP bridge to rust-analyzer / tsserver

For questions where per-language depth matters (complex Rust type
inference, contextual TS completion, macro expansion), Standardoc can
**bridge** to rust-analyzer or tsserver and merge their answer with its
own graph view. The agent gets the best of both worlds through a single
MCP interface — a cross-language semantic graph **plus** the per-language
depth of the official LSPs.

### Optional local doc UI

If demand emerges — not by default — a **GitBook-style local doc UI**
that consumes the graph and the beta.3 rendering. Served over local
HTTP, gitignored, **never hosted by StandarX**. The idea: visual
navigation richer than the VSCode webview for projects that want to
publish their docs around the Standardoc graph.

Probably under a **distinct lifetime license** from the open-source core
(cf. [SUPPORT.md](../SUPPORT.md)) if it becomes a StandarX funding
asset. **The core, for its part, stays FSL → MIT unchanged.**

---

## The invariants protected over the very long term

The 5 invariants laid down at 1.0 stay intact:

- **The IR stays stable.** A `protocol_version` bump + mandatory
  coexistence for any breaking change.
- **The graph stays local.** No cloud sync, no auth, no phone-home
  telemetry.
- **The license timer stays armed.** FSL → MIT 2 years per release.
  First release: April 26, 2028.
- **The SQLite format stays open.** Versioned schema, readable with a
  standard sqlite3.
- **The public API is frozen.** MCP tools, custom LSP methods, exported
  IR types, SQLite schema.

And **a 6th invariant appears post-1.0**:

- **The core doesn't grow.** All extension pressure goes through the
  plug-in layer, not through an addition to the Rust core. This
  constraint preserves the simplicity of the system and the readability
  of the IR contract. The plug-in sandboxing (mlua + capability-based)
  is itself frozen too.

---

## What we do NOT do post-1.0

Essential negative framing so we don't drift:

- **No mandatory hosted service.** If a hosted doc UI ever emerges, it
  stays optional, and the core works entirely without it.
- **No centralized monetized plug-in registry.** Plug-ins distribute via
  GitHub, local files, community channels — like dotfiles. No
  proprietary marketplace that captures the ecosystem or exfiltrates
  usage data.
- **No phone-home telemetry.** Ever. Even opt-in. It's a non-negotiable
  cultural invariant.
- **No unilateral breaking change to the IR.** A `protocol_version` bump
  + mandatory coexistence for any evolution.
- **No scope creep on the core side.** If a feature can live in a
  plug-in, it lives in a plug-in. If it can't, we question *why* first
  before widening the core.

---

## Going further

- **[← Mid-term vision](vision-mid-term.md)** — beta.3 and 1.0
- **[← Philosophy](philosophy.md)** — the 5 system-thinking principles
  and the construction ethics
- **[Notes](notes.md)** — dogfood observations, locked decisions
- **[Test feedback](test-feedback.md)** — what we tested, what we
  dropped, the estimates
- **[TODO-LIST → Post-1.0 ideas](../TODO-LIST.md)** — exhaustive
  checkboxes per milestone (*Post-1.0 ideas, no commitment* section)
