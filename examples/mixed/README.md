# mixed example

📖 English · [Français](README.fr.md)

A single scan across a Rust file and a TypeScript file — **same pipeline,
same DSL**, cross-language documentation on one page.

```sh
# From the repo root:
cargo run -p standardoc -- scan      examples/mixed/src
cargo run -p standardoc -- transform examples/mixed examples/mixed/docs-src/greet.md
```

Files:
- `src/server.rs` — a Rust handler with a standardoc annotation.
- `src/client.ts` — a TypeScript wrapper with a JSDoc standardoc annotation.
- `docs-src/greet.md` — template that pulls from both.
