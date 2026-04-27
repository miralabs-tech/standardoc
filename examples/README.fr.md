# Exemples

[English](README.md) · 📖 Français

Démos end-to-end du pipeline standardoc complet : **scan** → **extract** → **transform**.

Chaque exemple est un workspace minimal et autonome. Depuis la racine du repo :

```sh
# Scan un dossier et imprime les DocBlocks canoniques en JSON
cargo run -p standardoc -- scan examples/rust-lib/src

# Rend un template markdown contre un scan
cargo run -p standardoc -- transform examples/rust-lib examples/rust-lib/docs-src/api.md
```

## Layout

| Dossier | Ce qu'il montre |
|---|---|
| [`rust-lib/`](rust-lib/) | Une mini lib Rust documentée avec `@doc`/`@param`/`@returns` plus de la prose en description implicite |
| [`typescript-lib/`](typescript-lib/) | TypeScript avec annotations JSDoc `@doc` + DSL `each`/`if` dans le template |
| [`mixed/`](mixed/) | Fichiers Rust **et** TypeScript documentés côte à côte ; un seul template pioche dans les deux |
| [`users-api/`](users-api/) | Une mini API REST montrant les tags `@route`/`@param`/`@response` consommables par le tool MCP `emit_openapi` |
| [`dynamic-langs/`](dynamic-langs/) | Une config `.standardoc/languages/*.json` — provider regex pure pour un langage exotique (pas de grammaire AST). Les forks tree-sitter y sont aussi documentés, avec leurs limites. |

## Cheat sheet template

```markdown
{{ @doc.KEY:label }}                    -- label du bloc
{{ @doc.KEY:description }}              -- shortcut tag (première occurrence)
{{ @doc.KEY:symbol.signature }}         -- sub-path symbole AST
{{ @doc.KEY:meta.path }}                -- sub-path metadata

{{ each p in @doc.KEY:param }}
- **{{ p.name }}** (`{{ p.type }}`) : {{ p.description }}
{{ /each }}

{{ if @doc.KEY:has(example) }}
Exemple : `{{ @doc.KEY:example }}`
{{ else }}
*Pas d'exemple.*
{{ /if }}
```

Les directives de bloc (`each`, `if`, `else`, `/each`, `/if`) seules sur une ligne consomment cette ligne — la sortie rendue est du markdown propre, pas de blanc fantôme.
