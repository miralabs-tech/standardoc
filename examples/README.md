# Examples

📖 English · [Français](README.fr.md)

End-to-end demos of the full standardoc pipeline: **scan** → **extract** → **transform**.

Each example is a minimal, self-contained workspace. From the repo root:

```sh
# Scan a folder and print canonical DocBlocks as JSON
cargo run -p standardoc -- scan examples/rust-lib/src

# Render a markdown template against a scan
cargo run -p standardoc -- transform examples/rust-lib examples/rust-lib/docs-src/api.md
```

## Layout

| Folder | What it shows |
|---|---|
| [`rust-lib/`](rust-lib/) | A tiny Rust library documented with `@doc`/`@param`/`@returns` plus implicit-description prose |
| [`typescript-lib/`](typescript-lib/) | TypeScript with JSDoc `@doc` annotations + `each`/`if` DSL in the template |
| [`mixed/`](mixed/) | Rust **and** TypeScript files documented side-by-side; one template pulls from both |
| [`users-api/`](users-api/) | A small REST-style API showing `@route`/`@param`/`@response` tags consumable by the `emit_openapi` MCP tool |
| [`dynamic-langs/`](dynamic-langs/) | A `.standardoc/languages/*.json` config — pure-regex provider for an exotic language (no AST grammar). Tree-sitter forks are documented there too, with their limits. |

## Template cheat sheet

```markdown
{{ @doc.KEY:label }}                    -- block label
{{ @doc.KEY:description }}              -- tag shortcut (first occurrence)
{{ @doc.KEY:symbol.signature }}         -- AST symbol sub-path
{{ @doc.KEY:meta.path }}                -- metadata sub-path

{{ each p in @doc.KEY:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

{{ if @doc.KEY:has(example) }}
Example: `{{ @doc.KEY:example }}`
{{ else }}
*No example.*
{{ /if }}
```

Block directives (`each`, `if`, `else`, `/each`, `/if`) alone on a line consume that line —
rendered output is clean markdown, no phantom blank lines.
