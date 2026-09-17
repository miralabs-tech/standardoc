# FAQ

📖 English · [Français](../fr/FAQ.md) &nbsp;|&nbsp; ← [README](../../README.md) · [Quickstart](QUICKSTART.md) · [Roadmap](TODO-LIST.md)

> **⚠️ Archived (2026-09-17).** Standardoc is archived on **v1.0.0-beta.2**;
> the [README](../../README.md) says why. Answers below were rewritten to
> match that: nothing "planned", "post-1.0" or "at 1.0" will happen.

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

Never. The project is archived. The planned **UST + Lua plug-in layer** was
never started; the six providers listed above are the final set.

## Does it work with agents other than Claude?

Protocol-wise it's a standard MCP server, so any client can connect. Only
Claude Code was ever tested — no fixture, no CI run for Cursor, Continue,
Copilot or the others. The MCP-first hooks it installed for Claude Code
blocked more work than they helped and are not recommended.

## Does it render docs (TypeDoc-style)?

No, and it never will. `@standardoc/core` / `@standardoc/react` were never
started; Standardoc stayed a semantic indexer.

## Is my code sent anywhere?

No. **Local-only, unconditionally.** The index lives in `.standardoc/` on
your disk; no network call to index, no telemetry, no phone-home — ever, even
opt-in. If Standardoc vanished tomorrow, your index keeps working.

## How does it scale?

Unknown. It was only ever run on this repository and a few small projects of
the author. The 1M+ LOC benchmarks were never built, so there is no scale
claim to make.

## Is it paid? A SaaS?

No. The core is **free, open-source, local**, and no paid tier ever existed.
The license stays FSL → MIT.

## Why FSL-1.1-MIT, not plain MIT?

[FSL-1.1-MIT](../../LICENSE) is permissive for any non-competing use and
blocks "fork-and-close" competitors. Plain MIT gives no short-term
protection; AGPL doesn't cover non-SaaS competitors. FSL combines protection
now with an irreversible opening: **each release auto-converts to plain MIT
two years later** (first: April 26, 2028). Commercial use is fine — you just
can't resell Standardoc itself as your own indexing product.

## Can I contribute?

The repository is archived (read-only). Fork it under the
[FSL-1.1-MIT](../../LICENSE) terms if you want to carry it on.

## Bug or security issue?

Nothing will be fixed. Security reports are still read — see
[SECURITY.md](../../SECURITY.md) — but no patch will ship.

---

← [README](../../README.md) · [Quickstart](QUICKSTART.md) · [Roadmap](TODO-LIST.md)
