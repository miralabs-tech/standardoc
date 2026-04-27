# Exemple mixed

[English](README.md) · 📖 Français

Un seul scan à travers un fichier Rust et un fichier TypeScript — **même
pipeline, même DSL**, doc cross-language sur une seule page.

```sh
# Depuis la racine du repo :
cargo run -p standardoc -- scan      examples/mixed/src
cargo run -p standardoc -- transform examples/mixed examples/mixed/docs-src/greet.md
```

Fichiers :
- `src/server.rs` — un handler Rust avec une annotation standardoc.
- `src/client.ts` — un wrapper TypeScript avec une annotation standardoc en JSDoc.
- `docs-src/greet.md` — template qui pioche dans les deux.
