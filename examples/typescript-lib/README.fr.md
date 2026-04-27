# Exemple typescript-lib

[English](README.md) · 📖 Français

```sh
# Depuis la racine du repo :
cargo run -p standardoc -- scan      examples/typescript-lib/src
cargo run -p standardoc -- transform examples/typescript-lib examples/typescript-lib/docs-src/api.md
```

Fichiers :
- `src/users.ts` — interface + fonctions + alias de type, tous annotés avec des tags JSDoc `@doc`.
- `docs-src/api.md` — template qui rend la signature, les params et les returns de chaque fonction.
