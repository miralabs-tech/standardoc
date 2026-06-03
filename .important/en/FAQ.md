# FAQ

📖 English · [Français](../fr/FAQ.md) &nbsp;|&nbsp; ← [README](../../README.md) · [Quickstart](QUICKSTART.md) · [Roadmap](TODO-LIST.md)

---

## Does it replace my LSP?

No — it complements it. Standardoc *exposes* LSP as one surface, but under
the hood it's a global cross-language graph, not a per-language server.
`rust-analyzer` / `tsserver` keep the deep per-language resolution (type
inference, macro expansion); Standardoc brings the cross-cutting graph + the
MCP surface. Use both.

## How is it different from Sourcegraph?

Sourcegraph is a hosted (cloud) team search engine focused on collaboration
and review. Standardoc is **local** semantic indexing for AI agents and
tools — no cloud, no auth, no per-seat billing; the index lives in
`.standardoc/` on your machine. They can coexist on the same repo.

## Why not tree-sitter or ripgrep directly?

On a small project, `ripgrep` + your IDE are plenty. Tree-sitter standalone
gives a *surface* AST (functions / classes / calls). Standardoc uses *deep*
parsers — `syn`, `swc`, `full_moon`, custom SFC — with full signatures,
types, generics, traits, and typed edges. (Tree-sitter returns post-1.0, but
*under* the UST + Lua plug-in layer, not as a surface indexer.)

## Which languages?

Native: **Rust** (`syn`), **TypeScript / JavaScript** incl. JSX / TSX / React
(`swc`), **Lua** (`full_moon`), and **C** (with cross-file `.h` ↔ `.c` join).
Plus **Vue** and **Svelte** via SFC parsing. The bar isn't language count —
it's AST depth.

## When will Python / Go / Java / … land?

Not as built-in core providers. Post-1.0 they come through the **UST + Lua
plug-in layer**: tree-sitter parses, a sandboxed Lua plug-in maps symbols /
edges, the Rust core validates against the IR — a `.lua` file dropped in the
workspace, not a core PR. See the [roadmap](TODO-LIST.md).

## Does it work with agents other than Claude?

Yes — it's a standard MCP server (Cursor, Continue, Copilot, Aider, Goose,
Cody, Claude Desktop / Code, …). Calibration is tuned on Claude Code (Opus);
other agents work but vary — some shortcut to grep when a task gets hard. The
MCP-first hooks enforce the discipline on Claude Code; wire the equivalent
elsewhere via `standardoc claude pre-tool-hook`.

## Does it render docs (TypeDoc-style)?

Not yet. Standardoc is a semantic indexer today. A rendering layer
(`@standardoc/core` + `@standardoc/react`, fed straight from the graph) is
planned but **slipped past beta.3** — see the [roadmap](TODO-LIST.md).

## Is my code sent anywhere?

No. **Local-only, unconditionally.** The index lives in `.standardoc/` on
your disk; no network call to index, no telemetry, no phone-home — ever, even
opt-in. If Standardoc vanished tomorrow, your index keeps working.

## How does it scale?

Native AST + SQLite + FTS5 + incremental watcher — cold start in seconds on a
medium repo (Standardoc indexes itself in a few). Published scale benchmarks
(1M+ LOC; cold start / watcher delta / query p99) land at 1.0, run in CI — no
"it scales, trust us".

## Is it paid? A SaaS?

The core is and stays **free, open-source, local**. No SaaS, no subscription,
no cloud. If a paid tier ever appears (e.g. a local doc UI), it'd be
local-only, lifetime one-time, and only on real demand. The core stays
FSL → MIT.

## Why FSL-1.1-MIT, not plain MIT?

[FSL-1.1-MIT](../../LICENSE) is permissive for any non-competing use and
blocks "fork-and-close" competitors. Plain MIT gives no short-term
protection; AGPL doesn't cover non-SaaS competitors. FSL combines protection
now with an irreversible opening: **each release auto-converts to plain MIT
two years later** (first: April 26, 2028). Commercial use is fine — you just
can't resell Standardoc itself as your own indexing product.

## Can I contribute?

Before the 1.0 freeze: **no third-party PRs** (the API has to stabilize
cleanly first). But issues, feedback, and ideas are very welcome via GitHub.
Post-1.0 opens up — the UST + Lua plug-in layer is built to absorb community
languages / detectors without touching the frozen core.

## Bug or security issue?

Bugs / features: [GitHub Issues](https://github.com/miralabs-tech/standardoc/issues).
Security: don't post it publicly — follow [SECURITY.md](../../SECURITY.md).

---

← [README](../../README.md) · [Quickstart](QUICKSTART.md) · [Roadmap](TODO-LIST.md)
