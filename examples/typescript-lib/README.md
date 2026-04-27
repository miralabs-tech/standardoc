# typescript-lib example

📖 English · [Français](README.fr.md)

```sh
# From the repo root:
cargo run -p standardoc -- scan      examples/typescript-lib/src
cargo run -p standardoc -- transform examples/typescript-lib examples/typescript-lib/docs-src/api.md
```

Files:
- `src/users.ts` — interface + functions + type alias, all annotated with JSDoc `@doc` tags.
- `docs-src/api.md` — template that renders each function's signature, params and returns.
