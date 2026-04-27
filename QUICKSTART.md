# Quickstart — 5 minutes from zero to a documented project

📖 English · [Français](QUICKSTART.fr.md)

This walks you from a fresh Rust workspace to a running standardoc daemon
that an AI agent (Claude Code, Cursor, …) can query, with live diagnostics
in your editor.

> If you already cloned standardoc and just want to use it: skip to
> **Step 2** with the binary at `target/release/standardoc-server`.

## Step 0 — Build the binaries

```sh
git clone https://github.com/miralabs-tech/standardoc
cd standardoc
cargo build --release -p standardoc-server -p standardoc
```

After build, the binaries you'll use are:
- `target/release/standardoc-server` — the daemon (LSP + MCP)
- `target/release/standardoc` — the CLI (one-shot scan / transform / validate)

## Step 1 — Annotate your code

In any Rust / TypeScript / Python / Lua file, add `@doc` comments above
public symbols:

```rust
/// Adds two integers.
/// @doc math.add add
/// @param a i32 first operand
/// @param b i32 second operand
/// @returns i32 the sum
pub fn add(a: i32, b: i32) -> i32 { a + b }
```

The minimum is `@doc <key>`. Everything else is optional but unlocks more
DSL features (`@param`, `@returns`, `@example`, `@see`, custom tags).

## Step 2 — Scan and validate

```sh
# See what was discovered
standardoc scan /path/to/your/project

# Run the validator (prints any STD001-STD012 issues)
standardoc validate /path/to/your/project
```

Common first-time output: a few `STD006` hints ("public symbol with no
@doc") — pick the ones worth documenting and ignore the rest. Add
`"STD006": "off"` to `.standardoc.json` to silence permanently.

## Step 3 — Write narrative pages

Create `.standardoc/pages/01-getting-started.md`:

```markdown
---
title: Getting Started
---

# Welcome

We expose a single `add` function:

`{{ @doc.math.add:symbol.signature }}`

{{ @doc.math.add:description }}

**Parameters**

{{ each p in @doc.math.add:param }}
- `{{ p.name }}` (`{{ p.type }}`): {{ p.description }}
{{ /each }}
```

The `{{ @doc.math.add:… }}` expressions resolve at render time against the
live index — change the source comment and the doc updates next rescan.

## Step 4 — Run the daemon (LSP + MCP)

```sh
target/release/standardoc-server --mcp --workspace /path/to/your/project
```

This starts both protocols on stdio simultaneously :
- The LSP pushes diagnostics, completions on `@doc.…`, hover, goto-def,
  references, rename, and DSL semantic-token highlighting to your editor
- The MCP exposes 28 tools to AI agents

For Claude Code / Cursor, drop a `.mcp.json` at your workspace root :

```json
{
  "mcpServers": {
    "myproj": {
      "type": "stdio",
      "command": "/abs/path/to/standardoc-server",
      "args": ["--mcp", "--workspace", "/abs/path/to/your/project"]
    }
  }
}
```

For VSCode + the standardoc LSP extension (TBD), the editor auto-spawns the
daemon.

## Step 5 — Add a custom language (no recompile)

Got a language not in the built-ins? Drop a JSON in
`.standardoc/languages/`:

**Pure regex** (any language, no AST needed):

```json
{
  "id": "myx",
  "extensions": [".myx"],
  "commentStyles": { "single": ["#"], "docSingle": ["##"] },
  "backend": {
    "kind": "regex",
    "patterns": [
      { "kind": "function", "regex": "^\\s*fn\\s+(?P<name>\\w+)\\((?P<params>[^)]*)\\)" }
    ]
  }
}
```

Restart the daemon to pick up new `.standardoc/languages/*.json` files.

For **tree-sitter forks** that extend an existing grammar (currently only
`lua`) with extra capture patterns, see
[`examples/dynamic-langs/`](examples/dynamic-langs/) — that README also
documents the limits (a fork **cannot** change syntax, add operators or
introduce new tokens; it only adds captures over an existing grammar).

## Step 6 — Iterate

While editing source or markdown, the watcher rescans on save:
- New `@doc` annotations show up in MCP tools immediately
- Broken `@doc.X` references trigger STD004 warnings in your editor
- DSL syntax errors trigger STD007

Rebuild & redeploy the daemon after a standardoc upgrade :

```sh
./scripts/build.sh    # or ./scripts/build.ps1 on Windows
# Pick [2] prod — kills running servers, rebuilds into target/release/.
# Then: open a new Claude Code conversation. No VSCode restart needed.
```

## Where to next

- [`README.md`](README.md) — the full feature surface
- [`examples/`](examples/) — runnable demos for Rust, TypeScript, mixed-language, and dynamic providers
- MCP tool `get_dsl_reference` — exhaustive DSL reference (`each`, `if`, function calls, block iteration, …)
- MCP resources `standardoc://*` — every MCP tool and resource discoverable directly from your IDE
