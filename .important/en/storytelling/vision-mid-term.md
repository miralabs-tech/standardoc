# Mid-term vision — beta.3 and 1.0

📖 English · [Français](../../fr/storytelling/vision-moyen-terme.md)

[← Short-term vision](vision-short-term.md) · [← Philosophy](philosophy.md) · [Long-term vision →](vision-long-term.md)

> This document is the **narrative** behind beta.3 and the 1.0
> stabilization. The exhaustive feature list per milestone lives in
> [`TODO-LIST.md`](../TODO-LIST.md) — that's the one that moves, not
> this doc.

---

## Where this fits

beta.2 laid down the maturity: MCP surface as a toolkit, multi-frontend
architecture validated, discipline encoded into the system. What follows
isn't an explosion of new features.

**beta.3 has a theme: pluralizing the uses of the graph.** Until now,
the graph mostly served a single-session AI agent in an editor. beta.3
opens three new consumption surfaces:

1. **Rendered docs** for external visitors (consumers of the project's
   API)
2. **Visual navigation** for internal maintainers (humans auditing their
   own code in the IDE)
3. **The autonomous CLI** for non-VSCode uses (ops, CI, non-Microsoft-IDE
   devs)

(A fourth idea, persisted cross-session project understanding, was
extracted to a sibling session-store tool — the core stays a code graph.)

**1.0**, for its part, seals the contracts: conventions frozen,
benchmarks published, the right to a unilateral breaking change
extinguished.

**But 1.0 isn't the next step after beta.3.** Between the two, several
additional `beta.X` will probably emerge — each time a need reveals
itself in dogfood without us having planned it, as was the case for
beta.2. **1.0 isn't a calendar, it's a maturity criterion**: when the
API is judged worthy of being frozen, and not before.

---

## beta.3 — why a rendering layer now

The v0 templating DSL (`{{ @doc.X }}`) was killed in beta.1. The graph
is complete, the canonical IR holds up, the MCP/LSP surfaces are
mature — but between the graph and a static doc site, there's still
nothing.

**We deliberately waited.** Writing a renderer on top of an API that's
still moving is wasteful: every breaking change on the graph side breaks
the rendering. beta.2 stabilized the surface; beta.3 can now lay down
the rendering without risking a full rewrite in 3 months.

### The rendering architecture

The SQLite graph stays **the** source of truth. Not MDX, not Markdown,
not a second source to maintain in a mirror. The renderers are
**consumers** of the graph, not sources.

- **`@standardoc/core`** — a framework-agnostic query API in plain
  JS/TS. `queryDocs("api.*")` queries the graph through a client layer,
  without imposing React / Vue / other. A project that just wants to
  output Markdown from the graph can consume `@standardoc/core` with
  nothing else.
- **`@standardoc/react`** — the first renderer. `<Doc id>`, `<Params
  id>`, `<Examples id>`, `<Signature id>` components. Drop-in adapters
  for Next.js, Nextra, Astro, Docusaurus.
- **Vue / Svelte renderers** — same graph, separate packages,
  post-beta.3. The framework-agnostic nature of `@standardoc/core` makes
  these additions mechanical, not architectural.

### The narrative annotations

The rendering needs more than the signature — it needs the prose that
describes *what a function is for*, what its parameters mean, usage
examples. That's the role of the `@doc`, `@param`, `@returns`,
`@example` annotations.

**This mechanism already exists in the code, on Lua.** The emmylua
provider (`enrich_signature` in
`standardoc-lang-provider::lua::emmylua`) parses the `---@param` /
`---@return` / `---@field` comments and enriches the signature with the
extracted metadata. beta.3 generalizes this pattern to **JSDoc** (TS /
JS / Vue / Svelte) and **rustdoc** (Rust) via language-provider hooks.
No custom DSL — we capitalize on each language's already-universal
conventions.

---

## beta.3 — visual navigation for maintainers

The graph doesn't only serve AI agents. **The other consumer, often
forgotten in AI-first marketing, is the human who maintains the code.**
The dev who comes back to their project after 6 months. The maintainer
auditing a codebase they left 2 years ago. The reviewer discovering a
module they've never touched.

**The human understands their project before the agent needs to.** If
the dev themselves can no longer reorient in their codebase, no AI will
be enough to compensate. Long-term human control goes through a surface
that makes the graph **directly readable** — not just queryable.

For those cases, a signature returned by the LSP isn't enough. An MCP
result handed to an agent isn't either. **You need an interactive visual
surface** in the IDE:

- Display of the local graph around a symbol (typed callers / callees /
  imports / imported_by)
- Click navigation — drill into a neighbor, go back, mark symbols of
  interest
- Compact view of the enrichments (descriptions, examples, annotated
  params) without opening every file
- Filtering by `kind` / `visibility` / language to scope an audit

Technically it's a **Preact webview** embedded in the VSCode extension.
**Calendar not guaranteed** — it's a candidate for beta.3 because the
dogfood value is high, but it may slip to a later beta depending on what
emerges in parallel.

### Why this need climbs so high in the roadmap

The use case observed in dogfood: **LurLang** — the homegrown language
cited as a dogfood target in [short-term vision](vision-short-term.md),
2-3 years of inactivity — could be picked back up without a
re-archaeology phase. The reason wasn't the AI. It was the fact that the
graph made the re-immersion **doable in one session**, where otherwise
it was weeks of re-reading. What was missing wasn't time or effort — it
was the **psychological momentum** you lose staring at 100k lines
written by the self-of-2-years-ago without a structural landmark.

This case is exactly the 5th system-thinking principle of
[`philosophy.md`](philosophy.md) ("what becomes incomprehensible in 6
months?"), applied this time to the **solo human maintainer** rather
than the AI agent. **Standardoc makes review and audit easier over the
very long term** — it's an angle of value that has nothing to do with
agents, and that justifies bumping the webview higher than "post-1.0".

---

## beta.3 — standardoc autonomous outside VSCode

Not all users are in VSCode. For the one who consumes `standardoc` via
`cargo install`, `curl | sh`, or directly the binary from a GitHub
release, the CLI needs to be **self-sufficient** — not dependent on the
VSCode extension to update or install itself correctly.

- **`standardoc self-update`** — reads `version.json` from the latest
  release, detects the platform, downloads the matching artifact,
  SHA256-verifies, replaces the binary in place (with Windows
  rename-on-replace handling).
- **PATH injection** at initial install — `~/.stdoc/bin/` (Unix) or
  `%USERPROFILE%\.stdoc\bin\` (Windows), with addition to the shell
  profile (bash / zsh / PowerShell) and to `HKCU\Environment\Path` on
  Windows.
- **One-liner bootstrap** — `curl -sSf https://… | sh` (Unix) and `irm
  https://… | iex` (PowerShell).

The challenge isn't CLI ergonomics. **It's that the binary must know how
to live outside VSCode** — for CI servers, for ops, for devs who don't
want a Microsoft IDE, for automated pipelines. This plumbing also reuses
the `version.json` mechanism laid down in beta.2 for the extension's
binary decoupling — it's not a new invention, it's a generalization of
an already-proven mechanism.

---

## Cross-session project understanding — extracted out of core

The synthesized understanding an agent reloads each session (goals,
posture, locked decisions, narrative intent) no longer lives in
Standardoc core — it moved to a **sibling session-store tool**, alongside
the beta.3 extraction of the session-handoff DB. The core stays a code
graph, not an agent-memory store. The guardrail it carried is unchanged:
any such synthesis is a derived projection re-validated against the graph,
never an independent source of truth.

---

## 1.0 — the API freeze, in depth this time

The principle was laid down in [short-term
vision](vision-short-term.md): at 1.0, we contractualize. What that
means concretely, item by item.

### Virtual annotations enrichments

**Honest framing: the storage layer already exists.** The
`standardoc-core::storage::enrichments` module ships the SQLite table,
the `upsert_enrichment` / `get_enrichment` API, the `EnrichmentInput`
and `ConfidenceLevel` types (`Low` / `Medium` / `High`), the FK cascade
on deletion, and the round-trip tests. The first consumer (emmylua on
Lua) is already running in production.

What 1.0 freezes isn't the primitive — it's the **conventions that fill
it**:

- *Verb-prefix conventions* — how a function name (`get_*`, `find_*`,
  `parse_*`, …) generates a default description when no doc-comment is
  found
- *Type-signature narratives* — how a signature composes a
  natural-language description (params + returns + modifiers)
- *Trait impl templates* — how to describe the instantiation of a trait
  for a given type

And the **extension of the first consumer to the two missing
frontends**: a rustdoc parser on the Rust side, a JSDoc parser on the TS
side. At 1.0, these three enrichment sources (rustdoc / JSDoc / emmylua)
are available and their semantics are frozen.

### Cross-substrate bridges (Tauri / WASM / FFI / DB / ORM / …)

**Honest framing: the IR primitive already exists.**
`standardoc-ir::bridge_kind::BridgeKind` is an opaque tag (`pub struct
BridgeKind(pub String)`) attached to edges and signatures since beta.2.
This primitive serves as a rendezvous point to describe edges that
**cross a heterogeneous substrate**:

- **Cross-language in the classic sense** — Rust ↔ JS code via Tauri,
  WASM bindings, C/C++ FFI declarations
- **Code ↔ data schema** — application code ↔ DB table / model via an
  ORM (Prisma, Drizzle, Diesel, SeaORM, Mongoose, SQLAlchemy, …) or
  inline SQL queries
- **Other bridges** — code ↔ IPC, code ↔ external system, to be mapped
  as dogfood goes

What 1.0 freezes is **the vocabulary of the kinds** — `"tauri"`,
`"wasm"`, `"ffi"`, `"sql"`, `"orm"`, `"db-table"`, `"db-model"`, and
whatever we'll need to define by then. From 1.0 on, adding a new kind
becomes a protocol change, not an internal choice.

**Calendar for the vocabulary extension**: on one of the betas between
now and 1.0, with no commitment on which. It's dogfood-driven — each new
substrate met in practice raises a request, and the vocab grows
accordingly. **Must ship before the 1.0 freeze**, otherwise it's
impossible to add kinds without a breaking change.

**The frontend detectors, themselves, arrive post-1.0 via the plug-in
layer** (cf. [long-term vision](vision-long-term.md) — UST + Lua). The
contributor writes their Prisma / Tauri / SQLAlchemy / … detector in
Lua, without touching the Rust core. **No built-in detector is promised
at 1.0**: the substrate × language × ORM combinatorics is too wide to
absorb into the core, and the plug-in layer is precisely designed to
distribute that work to the ecosystem.

The net effect aimed for (once the detectors are delivered post-1.0):
trace a React click handler down to the Rust function it invokes via
Tauri; trace a REST endpoint down to the SQL table it updates; trace a
GraphQL mutation down to its Prisma model. **A single typed graph edge**,
not grep.

*Honest note: when we have all that, Standardoc will be deeply post-1.0.
The right word for that moment: **FINALLY**.*

### Published perf benchmarks

Cold start, watcher delta, MCP query latency p99 — measured on 1M+ LOC
monorepos, and **published**. Not "it scales, trust us". The numbers run
in CI, are attached to releases, and regress visibly when we break them.

### Contractual API freeze

MCP tools, custom LSP methods, IR types exported by `standardoc-ir`,
SQLite schema — the whole becomes a public contract. Every later
breaking change goes through a `protocol_version` bump and a daemon-side
coexistence period. No more unilateral right to change the semantics of
a tool or an edge without explicit agreement.

---

## The overall logic: primitives first, conventions after

A recurring pattern emerges from looking at what's shipped vs. what's
left to do: **we laid down the stable primitives before filling all
their consumers**. The enrichments table existed before the first
doc-comment parser used it. The `BridgeKind` tag existed in the IR
before a Tauri detector produced it. The typed edges (`CALLS`,
`IMPORTS`, `EXTENDS`, …) were there day-1, we progressively enrich their
attributes.

It's the opposite of classic software marketing ("announce, then
build"). Here: **we build the invariant first, fill the conventions
after, freeze the contract at the moment third parties can actually
depend on it.**

It's consistent with the 5 system-thinking principles of
[`philosophy.md`](philosophy.md) — particularly the 1st ("what stays
stable despite the changes?") and the 2nd ("which choices become
irreversible?"). A well-laid primitive absorbs 10 iterations of
conventions without breaking. A convention frozen too early calcifies
the tool.

---

## The beta.1 → 1.0 arc, seen whole

- **beta.1** = the grammar (IR + 2 day-1 surfaces)
- **beta.2** = the maturity (MCP toolkit, multi-frontend architecture,
  encoded discipline, storage + IR primitives laid down without noise)
- **beta.3** = pluralization of the graph's uses (rendered docs + visual
  navigation + autonomous CLI)
- **beta.4 / 5 / …** = whatever emerges in dogfood between beta.3 and
  1.0 (impossible to list in advance — beta.2 itself wasn't planned in
  its current form)
- **1.0** = the contract (conventions filled, semantics frozen,
  benchmarks published, unilateral breaking-change right extinguished)

At 1.0, Standardoc stops being *a tool we refine internally* and becomes
*an infrastructure third parties can depend on with confidence*. It's
the pivot where we lose some rights (unilateral semantic change) in
exchange for others (projects resting on it knowing we'll keep our
word).

---

## What we do NOT do in this phase

Important negative framing:

- **No SaaS, no cloud sync.** The open-source core stays local-first,
  unconditionally. The SaaS pivot stays off the table.
- **No plug-and-play multi-language via Lua/UST.** That vision belongs
  to post-1.0 (cf. [long-term vision](vision-long-term.md)). We
  stabilize the current languages first before opening to plugins.
- **No Vue / Svelte renderers before beta.3 is done.** The
  framework-agnostic `@standardoc/core` will make their addition
  mechanical post-beta.3, but we don't scatter the effort before React
  is solid.
- **No extension of the MCP surface without a clear dogfood need.**
  beta.2 grew the MCP surface on observed needs. 1.0 freezes it; we don't
  add to it without a manifest dogfood hole.

---

## The invariants protected at 1.0

The 4 invariants laid down in [short-term
vision](vision-short-term.md) stay intact:

- **The IR stays stable.** No removal, no semantic change without a
  `protocol_version` bump.
- **The graph stays local.** No cloud sync, no auth, no phone-home
  telemetry.
- **The license timer stays armed.** FSL-1.1-MIT → plain MIT 2 years
  after each release. First release: April 26, 2028.
- **The SQLite format stays open.** Versioned schema, readable with a
  standard sqlite3.

At 1.0, a fifth invariant appears:

- **The public API is frozen.** MCP tool signatures, LSP custom
  methods, exported IR types, SQLite schema. A `protocol_version` bump
  + mandatory coexistence for any breaking change.

---

## Going further

- **[← Short-term vision](vision-short-term.md)** — beta.2 and the
  stabilization phase
- **[Long-term vision →](vision-long-term.md)** — UST + Lua plugin layer
  post-1.0, ecosystem, platform
- **[Notes](notes.md)** — dogfood observations, locked decisions,
  learnings
- **[Test feedback](test-feedback.md)** — what we tested, what we
  dropped, the measurements
- **[TODO-LIST](../TODO-LIST.md)** — exhaustive checkboxes per milestone
