# Comparison

📖 English · [Français](COMPARISON.fr.md)

How Standardoc relates to adjacent tools you might already use.

---

## At a glance

| Capability                       | Standardoc | LSP (per-lang) | Grep / Sourcegraph | TypeDoc / JSDoc / Sphinx |
| -------------------------------- | :--------: | :------------: | :----------------: | :----------------------: |
| Cross-language graph             |     ✅     |       ❌       |         ⚠️         |            ❌            |
| Semantic graph (typed edges)     |     ✅     |       ⚠️       |         ❌         |            ❌            |
| AI-first MCP surface             |     ✅     |       ❌       |         ⚠️         |            ❌            |
| Live index (file watcher)        |     ✅     |       ✅       |         ❌         |            ❌            |
| Source of truth = AST            |     ✅     |       ✅       |         ❌         |       ⚠️ (manual)        |
| No annotation required           |     ✅     |       ✅       |         ✅         |            ❌            |
| Local-first, no SaaS             |     ✅     |       ✅       |       ⚠️ (B)       |            ✅            |
| Setup overhead                   |    Low     |     Builtin    |     Low (grep)     |        Medium-high       |

Legend: ✅ first-class · ⚠️ partial / depends · ❌ absent

---

## vs LSP (rust-analyzer, tsserver, …)

LSP is **complementary**, not competing.

- LSP gives precise per-language symbol resolution, hover, go-to-definition,
  find-references, rename. Standardoc's LSP daemon does the same for the
  Standardoc-managed surface.
- LSP is **per-language and per-editor**. Standardoc unifies Rust + TS into
  one cross-language graph, queryable over MCP from any AI client.
- Standardoc's MCP exposes the same graph LSP serves to your editor — same
  source of truth, two protocols.

**Use both.** LSP for editor navigation, Standardoc for cross-language graph
+ AI agent queries.

---

## vs Grep / Sourcegraph

Grep finds **text**. Standardoc finds **meaning**.

| You ask                                     | Grep                            | Standardoc                              |
| ------------------------------------------- | ------------------------------- | --------------------------------------- |
| "Find all `parse_workspace` calls"          | All occurrences in any context  | Just the actual `CALLS` edges           |
| "What does `createUser` depend on?"         | Manual file walk                | `get_context(fqdn, depth=1)` → callees  |
| "Who imports the `Auth` module?"            | `grep -r "from .*Auth"`         | `imported_by` edge list, FQDN-resolved  |
| "What's the signature of this function?"    | Open file, scroll               | `RawSymbol.signature` from `find_symbol` |

Sourcegraph adds web-scale search and some semantic features but stays
**text-centric** and **server-hosted**. Standardoc is **graph-centric** and
**local-only**.

---

## vs Documentation tools (TypeDoc, JSDoc, Sphinx, Docusaurus)

These tools answer **"how do I write narrative prose for my code"**.
Standardoc answers **"how do I expose my code's structure to AI agents and
tooling"**.

- TypeDoc / JSDoc / Sphinx require **annotations everywhere**. Standardoc
  works on **any codebase, no annotations required** — the AST is enough.
- Doc tools produce **static rendered output** that drifts the moment code
  changes. Standardoc keeps the index **live** via a file watcher.
- Doc tools target **human readers** of websites. Standardoc targets **AI
  agents, IDE tooling, and humans** through one stable contract.

A documentation rendering layer is on the post-beta.2 roadmap as an npm
package shipping React/MDX components (`<Doc id="…" />`, `<Params id="…" />`,
`queryDocs("api.*")`) consumable from Next/Nextra/Astro/Docusaurus/…
The doc graph (SQLite) feeds the rendering layer; no template engine,
no custom DSL — just MDX with structured queries. Once it ships, you'll
get both: live structure and narrative prose that can never drift.

---

## vs MCP-per-product servers (Stripe MCP, GitHub MCP, …)

Those servers expose **one product** to AI agents. Standardoc exposes
**your codebase** to AI agents — agnostic of product, framework, or
deployment target.

You'd use a Stripe MCP to manage your Stripe account from an agent. You'd use
Standardoc MCP to let the agent understand the code you wrote that *uses*
Stripe.

Complementary. Compose them.

---

## When *not* to use Standardoc

Honest answer:

- **Pure text search across files** → use Grep.
- **File path / glob patterns** → use Glob.
- **Reading a known file at a known path** → use `cat` / your editor's open.
- **Markdown / config files unrelated to code symbols** → not in scope.
- **Languages other than Rust / TypeScript / JavaScript** → wait for
  post-beta.1, or contribute a `LanguageProvider` (see [`crates/standardoc-lang-provider/`](crates/standardoc-lang-provider/)).

Standardoc is purpose-built for **semantic understanding of code structure**.
Outside that surface, dedicated tools win.
