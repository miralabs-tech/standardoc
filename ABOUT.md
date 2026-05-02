# About Standardoc

📖 English · [Français](ABOUT.fr.md)

> **One-line pitch** — a local semantic graph of your code, with the AST as
> source of truth, surfaced over MCP so AI agents stop grepping and start
> querying.

---

## 🧠 The problem

When an AI agent answers *"what does function X take as input?"*, today it does
`grep -r "fn X" .` then `cat src/foo.rs` then guesses. Cost: **30k-100k tokens**
per question, on every conversation, on every project.

For humans the problem is parallel. Modern codebases are too complex to keep a
mental model of. Existing tooling fragments the answer:

- **LSP** gives you precise per-language symbol resolution but no cross-language
  graph and no AI-friendly query surface.
- **Grep / Sourcegraph** gives you text-level navigation but no semantic
  meaning — you find occurrences, not relationships.
- **JSDoc / TypeDoc / Sphinx** gives you narrative prose, hand-maintained,
  perpetually drifting from the code it claims to describe.

Standardoc bridges this: a **single local index** built from the AST, exposed
to humans (LSP) and AI (MCP) through one stable contract.

---

## 💡 The thesis

### Code is the source of truth

The AST is **structurally accurate by definition**. Standardoc parses Rust
with `syn` and TypeScript with `swc`, normalizes both into a canonical
intermediate representation (FQDN-keyed symbols, typed edges), and persists
the graph in SQLite with FTS5 for fuzzy search.

No annotations required. The AST tells the truth — Standardoc just listens.

### Structure is derived

`<package>::<module>::<name>` is a stable identifier across Rust and TypeScript.
Edges are typed: `CALLS`, `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`,
`DEFINES`, `USES_TYPE`, `EXPOSES_API`. Cross-language jumps (Rust ↔ TS via
Tauri commands, TS ↔ Rust via WASM bindings) materialize as
`UnresolvedBridge` edges that future bridge plug-ins will resolve.

### Understanding is a system

A graph you can query is more useful than ten doc pages you have to read.
The MCP server exposes two tools:

- `find_symbol(query, limit?)` — fuzzy FTS5 search → list of symbols.
- `get_context(fqdn, depth: 1|2)` — symbol + neighbors (callers, callees,
  imports, imported_by) as a graph slice.

Two tools. Honest contract. AI agents learn them once and reuse them on every
project.

---

## 🤖 Why MCP-first

Existing MCP servers fall into two camps:

- **Product-specific** (Stripe MCP, Linear MCP, GitHub MCP) — useful for that
  one product, useless for *your code*.
- **Library-specific** (one MCP per framework) — fragmentation, maintenance
  burden, never covers what you actually use.

Standardoc is the first MCP server that:

- Indexes **any codebase** (Rust + TS day-1, more languages coming
  post-beta.1).
- Works **without any annotation** — drop in, run cold start, query.
- Exposes **one stable contract** that agents learn once and reuse on every
  project.

For an AI agent, `get_context("myapp::server::handle_request", 1)` resolved via
MCP costs ~100 tokens vs 10k-100k tokens of `grep + read` over the repo.
**100x to 1000x token savings**, systematically.

---

## 🎯 Why Rust + TypeScript first

Two languages. Both popular. Both have first-class native parsers (`syn`,
`swc`). Both common in modern stacks (Rust backend + TS frontend, Tauri
desktop apps, web extensions).

Reducing scope to two languages lets us **perfect the foundation** before
expanding. Once Rust and TS are rock-solid (FQDN unification, cross-language
bridges, edge resolution, performance on 200k LOC monorepos), adding Python /
Go / Java / Swift via tree-sitter or native parsers becomes a copy-paste of
the language provider trait.

The alternative — shipping 10 languages with mediocre depth — is what existing
tools do, and it's why none of them give AI agents a useful semantic surface.

---

## 🔓 Open-core posture

**Standardoc Core** — CLI, LSP, MCP, all language providers, VSCode
extension. Source under [FSL-1.1-MIT](LICENSE) — converts to **plain MIT on
April 26, 2028**. Free for any non-competing use today, fully MIT after that.

The focus stays on the open-source Core. As long as Standardoc has no
cloud/server component that would justify recurring infrastructure, there
is **no reason to offer a subscription** — and none is planned. If a
companion tool ever ships (e.g. a GitBook-style local doc UI that runs
on your machine, no hosting), it would ship under a **one-time lifetime
license**. Either way, the Core stays OSS and the MIT conversion date
above is locked.

No SaaS. No per-seat subscription. No telemetry. No upsell modal in your IDE.

---

## 🚀 Long-term direction

What's coming:

- **More language providers** — Python / Go / Java / C# / Swift / Zig via
  native parsers or tree-sitter.
- **Cross-language bridge plug-ins** (WASM) — Tauri command resolution, WASM
  bindings, FFI declarations resolved across the graph.
- **Virtual annotations** — synthesized doc descriptions for undocumented
  public symbols (verb-prefix conventions, type-signature narratives, trait
  impl templates).
- **MDX/React rendering layer** — npm package shipping
  `<Doc id="user.create" />`, `<Params id="user.create" />`,
  `<Examples id="user.create" />`, `queryDocs("api.*")`. Drop-in for
  Next/Nextra/Astro/Docusaurus/… The doc graph (SQLite) feeds the rendering
  layer; no template engine, no custom DSL — just MDX with structured
  queries. Targeting beta.2.
- **Optional GitBook-style companion UI** — if the idea materializes,
  local-only (runs on your machine, no hosting), one-time lifetime
  license. Decided based on adoption.

The full backlog and milestone breakdown live in [TODO-LIST.md](TODO-LIST.md).

---

## 🙋 Why this project exists

I'm the sole maintainer, working on Standardoc on top of a day job. Years of
watching AI agents burn thousands of tokens to figure out what a 5-line
function does, and watching documentation drift away from the code it claims
to describe. Every doc tool I tried solved part of the problem and pushed the
rest back onto manual work.

There had to be a single source of truth that humans could read and AI could
query — without me rewriting it for each consumer. That's Standardoc.

If it saves you time, [support the project](SUPPORT.md).
If you find a bug, [open an issue](https://github.com/miralabs-tech/standardoc/issues).
