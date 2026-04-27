# rust-lib example

📖 English · [Français](README.fr.md)

```sh
# From the repo root:
cargo run -p standardoc -- scan      examples/rust-lib/src
cargo run -p standardoc -- transform examples/rust-lib examples/rust-lib/docs-src/api.md
```

Files:
- `src/lib.rs` — the source with `@doc`/`@param`/`@returns`/`@example` annotations (plus one symbol auto-inferred with implicit description from its prose).
- `docs-src/api.md` — template that pulls from the discovered blocks.
