# FAQ

📖 English · [Français](FAQ.fr.md)

---

## Is this a documentation tool?

Not in the TypeDoc / JSDoc sense. Standardoc is a **semantic indexer** —
narrative documentation will return post-beta.1 as a derived output of the
index. Day-1 the deliverable is the live graph + MCP / LSP surface.

## Does it replace LSP?

No, it **complements** LSP. The Standardoc LSP daemon serves the
Standardoc-managed graph; rust-analyzer / tsserver still serve their own
language-specific surface. Use both.

## Why only Rust + TypeScript at beta.1?

Two languages well-supported beats ten languages half-supported. We're
perfecting the foundation (FQDN unification, cross-language bridges, edge
resolution, perf on 200k LOC monorepos) before expanding.

## When will language X be supported?

Post-beta.1, in this rough order: Python, Go, Java, C#, Swift. Either via
native parsers or tree-sitter. Adding a language = implementing the
`LanguageProvider` trait — see [`crates/standardoc-lang-provider/`](crates/standardoc-lang-provider/).

## How do I install?

```sh
cargo install standardoc-cli
```

That gets you the `standardoc` binary. For the integrated VSCode flow, install the
Standardoc extension on top. Full walkthrough: [QUICKSTART.md](QUICKSTART.md).

## Is the VSCode extension required?

No. The CLI works standalone — you can run `standardoc lsp <ws>` + `standardoc mcp <ws>
--readonly` in two terminals and use Standardoc from Claude Desktop, Cursor,
the Claude Code CLI, or any MCP-aware client. The extension just makes the
flow seamless inside VSCode.

## What's the difference vs Claude Code's built-in `Read` / `Grep` / `Glob`?

Claude Code's built-ins answer **text-level** questions. Standardoc answers
**graph-level** questions — callers, callees, imports, type relationships,
cross-language edges — without the agent having to assemble those facts from
text scans. ~100 tokens vs ~30k tokens per question.

The generated AI agent skill instructs Claude Code to use Standardoc **first**
on any code task, falling back to `Read` / `Grep` / `Glob` only when
Standardoc can't answer.

## Is my code sent anywhere?

No. **Standardoc is local-only.** The index lives in `.standardoc/index.db`
on your disk. The MCP daemon serves data over `stdio` on your machine. No
network calls, no telemetry, no SaaS.

## How does it perform on large workspaces?

Cold start on the Standardoc repo itself (~600 files, mixed Rust + TS) takes
under a second. Watcher overhead during edits is negligible. SQLite + FTS5
scales well to 200k LOC monorepos. Perf on 1M+ LOC is on the post-beta.1
benchmark agenda.

## Why open-core?

I'm the sole maintainer working on Standardoc on top of a day job. Open-core
posture lets the dev tooling stay free (max ecosystem reach) while keeping
the door open for an optional paid tooling tier later if the project takes
off. Whatever happens: no SaaS, no subscription, no telemetry. As long as
Standardoc has no cloud/server component that needs recurring infrastructure,
there's no reason to charge a subscription — and none is planned. Anything
paid would be local-only (runs on your machine, no hosting) and **one-time
lifetime license** — and only if there's clear demand for it.

The Core itself is locked to convert from FSL-1.1-MIT to **plain MIT on
April 26, 2028** regardless.

## Why FSL-1.1-MIT and not MIT outright?

[FSL-1.1-MIT](LICENSE) is permissive for any **non-competing use**. It
prevents direct competing offerings (the "open-and-pillage" pattern) without
locking down the core for honest end-users. Adopted by Sentry, CodeCrafters,
Keygen. Two years after each release, that release converts to plain MIT.

## Can I use Standardoc commercially?

Yes, freely, as long as you're not building a product that **substitutes for
Standardoc itself**. Internal tooling, customer-facing apps, SaaS products —
all fine. Reselling Standardoc as your own SaaS — not fine. Read the
[license](LICENSE) for details.

## Does it lose precision compared to LSP?

Yes, intentionally — for cross-language and AI-friendly query reasons. LSP
gives you per-language perfect resolution; Standardoc gives you cross-language
graph at the cost of some per-language depth. Use both: LSP for editor
precision, Standardoc for cross-cutting graph + AI queries.

## What about documentation rendering / the v0 DSL?

The v0 templating DSL (`{{ @doc.X }}` expressions inside markdown) has been
**dropped**. It made markdown unreadable for human authors and hard to
maintain without a dedicated UI.

The replacement, targeting **beta.2**, is an npm package exposing
React/MDX components fed by the doc graph:

```mdx
<Doc id="user.create" />
<Params id="user.create" />
<Examples id="user.create" />

{queryDocs("api.*").map(d => <Doc key={d.id} id={d.id} />)}
```

Drop-in for Next / Nextra / Astro / Docusaurus / any framework that consumes
MDX. The pipeline becomes:

```
source code
  ↓
annotation parser (@doc)
  ↓
doc graph (SQLite)
  ↓
MDX / React rendering layer (npm package)
  ↓
your framework (Next / Nextra / Astro / Docusaurus / …)
```

Until beta.2, the Standardoc index is consumed exclusively via MCP / LSP.

## How do I report a bug or request a feature?

[GitHub Issues](https://github.com/miralabs-tech/standardoc/issues). For
security issues, email the maintainer (see `Cargo.toml` `authors`) — do not
post publicly until a `SECURITY.md` policy ships.
