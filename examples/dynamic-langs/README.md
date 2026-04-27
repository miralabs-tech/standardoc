# Dynamic language providers

📖 English · [Français](README.fr.md)

Two ways to add a language to standardoc **without recompiling** the binary :

## 1. Tree-sitter fork (post-MVP — see limits below)

Reuse a built-in tree-sitter grammar (currently `lua`) with extra query
patterns to capture additional symbol shapes. Fork = **add patterns**, not
**change grammar**.

### What it can do
- Add new captures over the existing grammar (e.g. capture
  `bind("name", function() end)` in addition to top-level fns)
- Override the comment styles (`---` vs `--`)
- Map to a new id / extension

### What it CANNOT do
- Add new operators (`+=`, `-=`, `??=`, …) — the underlying tree-sitter
  grammar will fail to parse them
- Change reserved keywords or token rules
- Add syntactic constructs (backtick hash strings like CfxLua's `joaat`,
  decorators, …)

For real language dialects with grammar changes (CfxLua, MoonScript, Teal,
Fennel, …), the proper fix is **a dedicated tree-sitter grammar compiled
into standardoc**, not a JSON config. Runtime grammar loading (WASM) is on
the roadmap.

## 2. Regex provider (this directory: `exotic.json`)

Pure regex scan — no AST, no grammar dependency. Works on **any** text
format. Less precise (a `function` keyword in a string literal will be
picked up too) but covers languages without a tree-sitter grammar.

### When to use
- Niche / proprietary languages with no public tree-sitter grammar
- Plain text formats that have a function-like structure (config DSLs,
  schema files, …)
- Quick prototyping while waiting for a proper grammar

### Schema reference

```json
{
  "id": "myx",
  "extensions": [".myx"],
  "commentStyles": {
    "single": ["#"],
    "docSingle": ["##"],
    "multi": { "start": "/*", "end": "*/" }
  },
  "backend": {
    "kind": "regex",
    "patterns": [
      { "kind": "function", "regex": "^\\s*fn\\s+(?P<name>\\w+)\\((?P<params>[^)]*)\\)" }
    ]
  }
}
```

Pattern requirements :
- `name` capture is **mandatory**
- `params` capture optional, comma-split into `ParamInfo`
- `signature` capture optional, used as the displayed signature override
- `kind` field maps to `SymbolKind` (`function`, `method`, `class`,
  `struct`, `enum`, `trait`, `module`, `field`, `variant`, `const`, …)

## Loading

Drop your JSON files into `.standardoc/languages/` at the workspace root.
Standardoc loads them at boot. Restart the daemon (run `./scripts/build.sh`
or `./scripts/build.ps1`, pick `[2] prod`, then start a new Claude Code
conversation) to pick up changes.

Invalid configs are logged to stderr and skipped — they don't block other
providers.

## Conflict resolution

If a dynamic provider declares an extension also handled by a built-in
provider (e.g. `.lua`), the **built-in wins** (registered first). Full
provider replacement is on the roadmap.
