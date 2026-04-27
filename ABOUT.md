# About Standardoc

📖 English · [Français](ABOUT.fr.md)

> **One-line pitch** : a documentation tool that solves a single problem — **your docs never drift from your code**, no matter what you refactor. And along the way, it exposes your codebase to any AI agent via MCP, regardless of language, even if the project was never initialized with standardoc.

---

## The problem

Documentation drifts. Always.

Every doc tool today asks you to write the same information twice :
- once in the code (signatures, types, constraints, semantics)
- once in markdown / JSDoc / a `.rst` file / a wiki / Notion / Confluence

The instant the code changes, the doc is wrong. You can rename a parameter in code in one second. Updating the 47 markdown files that mention it takes hours — and nobody does it consistently. Six months later, half the doc is lying to your users.

JSDoc, TypeDoc, Sphinx, Docusaurus in manual mode, Nextra, GitBook — they all share the same flaw : the **link from code to prose is human-maintained**, and humans break that link every time they ship a feature under deadline.

For AI agents, the problem multiplies. To answer "what does function X take as input?", an agent today does `grep -r "fn X" .` then `cat src/foo.rs` then guesses. Cost : 30k–100k tokens per question. With a fresh, structured doc index, the same answer costs ~100 tokens. **A 100x to 1000x reduction**, systematically.

## The thesis

Decouple **structured data** (annotations next to symbols, machine-readable) from **narrative prose** (markdown, human-written), and link the two with a small DSL the IDE understands.

```rust
/// Adds two integers.
/// @doc math.add add
/// @param a i32 first operand
/// @param b i32 second operand
/// @returns i32 the sum
pub fn add(a: i32, b: i32) -> i32 { a + b }
```

```markdown
## `{{ @doc.math.add:label }}`

{{ @doc.math.add:description }}

`{{ @doc.math.add:symbol.signature }}`

{{ each p in @doc.math.add:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}
```

Rename `add` to `sum` in the source ? The doc follows automatically. Change a parameter type ? The doc follows. Delete a parameter ? The LSP screams red in your editor with diagnostic STD008.

The annotation lives **next to** its symbol. The prose lives in markdown **separately**. The DSL stitches them together at render time. Move either, the link survives. Drift becomes architecturally impossible.

## The double moat

### 1. Zero-drift between code and prose

This is the *functional* moat. TypeDoc / Nextra / Docusaurus in manual usage all push the dev to re-edit the prose every time the code shifts. Standardoc eliminates that step. Once you've used a tool that guarantees the doc matches the code, going back feels reckless.

### 2. Universal, language-agnostic MCP server

This is the *strategic* moat. Existing MCP servers are nearly all :
- **product-specific** (Stripe MCP, Linear MCP, GitHub MCP, …) — useful for that one product, useless for your code
- **library-specific** (one MCP per framework) — fragmentation, maintenance burden

Standardoc is the first MCP server that :

- indexes **any codebase** (Rust, TypeScript, Python natively + tree-sitter for Lua / and dynamically-loaded providers)
- works **even on a project never initialized with standardoc** : the AST auto-discovers all exports, `@doc` annotations are just enrichment
- exposes one stable protocol agents learn once and reuse everywhere — same set of tools whether you're indexing a 200-line script or a 200k-LOC monorepo

For an AI agent, `{{ @doc.foo:description }}` resolved via MCP costs ~100 tokens vs 10k–100k tokens of `grep + read` over the repo. **100x to 1000x token savings**, on every question, every conversation, every project.

## Who it's for

Three personas, three modes of usage :

### The developer (writes annotations)

You annotate functions with `@doc key`, optionally `@param name type description`, `@returns type description`, etc. Your IDE (via the LSP) gives completion, hover, goto-definition for every reference, and rename refactoring that propagates `DocKey` changes into all `.md` files automatically.

You spend 30 seconds per function annotating, save weeks of doc maintenance.

### The doc reader (consumes the rendered output)

A user opens your published doc site and reads exactly what was in the source at the moment of build. The signatures are accurate. The parameter types match. The links work. Examples are real, executable code, not fictionalized snippets. Trust restored.

### The AI agent (queries the index)

An agent (Claude Code, Cursor, Zed, Continue, …) connects via MCP, gets 28 tools to query the index : list docs, search by type, find usages, validate doc syntax, generate llms.txt / skill.md / OpenAPI exports. The agent never has to grep. The agent never hallucinates a signature. The agent answers questions in 100 tokens, not 100k.

## Open-core

Standardoc is **open-core, GitLab-style**.

- **Standardoc Core** — CLI, LSP, MCP, all language providers, DSL, validator, plugins API, HTTP/SSE backend. Source under [FSL-1.1-MIT](LICENSE) (converts to plain MIT after 2 years). Free for any non-competing use.
- **Standardoc Pro** — the polished web UI (GitBook-like navigation, MDX live components, search, polish). Closed-source, one-time **lifetime** purchase, no subscription. Distributed as a binary.

The bet : the dev tooling stays free to maximize ecosystem reach (the moat is adoption). The polished UI for non-dev doc creation gets monetized as a one-time purchase to fund my work on the Core (because no infrastructure cost can be sustained without revenue).

**Pro doesn't ship until `v1.0.0`.** While Standardoc is in `v0.x.x`, everything I publish is OSS — the Pro tier is held back so the API surface stabilizes first and so paying users get something that won't break under their feet next week.

No SaaS, no per-seat subscription, no telemetry, no upsell modal in your IDE. Pay once for Pro if you want the UI, otherwise use the Core forever for free.

## Long-term direction

What's coming :

- **VSCode extension** — thin wrapper that auto-spawns the daemon, surfaces status, ships independent of `standardoc-server` itself
- **Runtime WASM grammar loading** — drop a `tree-sitter-X.wasm` to add support for any language with a public grammar
- **More language providers** — Java / Kotlin / Go / C# / Swift / Zig / etc. via tree-sitter once WASM loading lands
- **Cross-ref FQN resolution** — proper `use` / `import` resolution per provider, ends the short-name ambiguity for large workspaces
- **Pro web UI features** — version snapshots, doc usage analytics, AI-assisted annotation generation, team review queues, cross-repo references

The full backlog and milestone breakdown live in the project's internal notes — the public roadmap is published at v0.1.1.

## Why this project exists

I've spent years writing the same markdown files three times — once for the team, once for the website, once for the AI tools that didn't read the website. Each rewrite wrong by the time it's done. Each AI agent burning thousands of tokens to figure out what a 5-line function does.

Before LLMs, doc was 100% manual. When you're the only one maintaining an open-source project on top of a day job, writing **and keeping current** the docs becomes the bottleneck that makes you give up. I've got a stack of side projects I either abandoned or kept private because of this — not because I dislike doc, but because keeping it in sync with a moving codebase took longer than writing the code itself. Standardoc is exactly the tooling I wish I'd had for those projects.

There had to be a single source of truth that humans could read narratively and agents could query structurally — without me rewriting it for each consumer. That's standardoc.

If it saves you time, [support the project](README.md#support-the-project).

If you find a bug or want a feature, [open an issue](https://github.com/miralabs-tech/standardoc/issues).

Once `v1.0.0` lands and [Standardoc Pro](/) ships, you'll be able to grab a one-time lifetime license for the polished web UI. Until then, everything is OSS and free.
