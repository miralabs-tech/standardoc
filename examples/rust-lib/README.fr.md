# Exemple rust-lib

[English](README.md) · 📖 Français

```sh
# Depuis la racine du repo :
cargo run -p standardoc -- scan      examples/rust-lib/src
cargo run -p standardoc -- transform examples/rust-lib examples/rust-lib/docs-src/api.md
```

Fichiers :
- `src/lib.rs` — le source avec annotations `@doc`/`@param`/`@returns`/`@example` (plus un symbole auto-inféré avec description implicite depuis sa prose).
- `docs-src/api.md` — template qui pioche dans les blocs découverts.
