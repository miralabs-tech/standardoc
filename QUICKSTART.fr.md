# Démarrage rapide — 5 minutes pour passer de zéro à un projet documenté

[English](QUICKSTART.md) · 📖 Français

Ce guide te fait passer d'un workspace Rust vierge à un daemon standardoc qui
tourne et qu'un agent IA (Claude Code, Cursor, …) peut interroger, avec des
diagnostics live dans ton éditeur.

> Si tu as déjà cloné standardoc et veux juste l'utiliser : saute à
> **l'étape 2** avec le binaire à `target/release/standardoc-server`.

## Étape 0 — Build des binaires

```sh
git clone https://github.com/miralabs-tech/standardoc
cd standardoc
cargo build --release -p standardoc-server -p standardoc
```

Après le build, les binaires que tu vas utiliser :
- `target/release/standardoc-server` — le daemon (LSP + MCP)
- `target/release/standardoc` — le CLI (one-shot scan / transform / validate)

## Étape 1 — Annoter ton code

Dans n'importe quel fichier Rust / TypeScript / Python / Lua, ajoute des
commentaires `@doc` au-dessus des symboles publics :

```rust
/// Additionne deux entiers.
/// @doc math.add add
/// @param a i32 premier opérande
/// @param b i32 deuxième opérande
/// @returns i32 la somme
pub fn add(a: i32, b: i32) -> i32 { a + b }
```

Le minimum est `@doc <key>`. Tout le reste est optionnel mais débloque plus
de features DSL (`@param`, `@returns`, `@example`, `@see`, custom tags).

## Étape 2 — Scan & validate

```sh
# Voir ce qui a été découvert
standardoc scan /chemin/vers/ton/projet

# Lancer le validator (affiche les éventuels STD001-STD012)
standardoc validate /chemin/vers/ton/projet
```

Sortie typique au premier run : quelques hints `STD006` ("symbole public sans
@doc") — choisis ceux qui méritent d'être documentés et ignore le reste.
Ajoute `"STD006": "off"` au `.standardoc.json` pour les faire taire en
permanence.

## Étape 3 — Écrire des pages narratives

Crée `.standardoc/pages/01-getting-started.md` :

```markdown
---
title: Démarrage
---

# Bienvenue

On expose une seule fonction `add` :

`{{ @doc.math.add:symbol.signature }}`

{{ @doc.math.add:description }}

**Paramètres**

{{ each p in @doc.math.add:param }}
- `{{ p.name }}` (`{{ p.type }}`) : {{ p.description }}
{{ /each }}
```

Les expressions `{{ @doc.math.add:… }}` se résolvent au render time contre
l'index live — change le commentaire source et la doc se met à jour au
prochain rescan.

## Étape 4 — Lancer le daemon (LSP + MCP)

```sh
target/release/standardoc-server --mcp --workspace /chemin/vers/ton/projet
```

Ça démarre les deux protocoles sur stdio simultanément :
- Le LSP push les diagnostics, complétion sur `@doc.…`, hover, goto-def,
  references, rename et highlighting semantic-token DSL vers ton éditeur
- Le MCP expose 28 tools aux agents IA

Pour Claude Code / Cursor, dépose un `.mcp.json` à la racine de ton workspace :

```json
{
  "mcpServers": {
    "myproj": {
      "type": "stdio",
      "command": "/chemin/abs/vers/standardoc-server",
      "args": ["--mcp", "--workspace", "/chemin/abs/vers/ton/projet"]
    }
  }
}
```

Pour VSCode + l'extension LSP standardoc (à venir), l'éditeur spawn le daemon
automatiquement.

## Étape 5 — Ajouter un langage custom (sans recompiler)

Tu as un langage absent des built-ins ? Dépose un JSON dans
`.standardoc/languages/` :

**Regex pure** (n'importe quel langage, pas besoin d'AST) :

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

Redémarre le daemon pour qu'il pick up les nouveaux fichiers
`.standardoc/languages/*.json`.

Pour les **forks tree-sitter** qui étendent une grammaire existante
(uniquement `lua` aujourd'hui) avec des patterns de capture supplémentaires,
voir [`examples/dynamic-langs/`](examples/dynamic-langs/) — ce README documente
aussi les limites (un fork **ne peut pas** changer la syntaxe, ajouter des
opérateurs ou introduire de nouveaux tokens ; il ajoute seulement des
captures par-dessus une grammaire existante).

## Étape 6 — Itérer

Pendant que tu édites le source ou du markdown, le watcher rescan au save :
- Les nouvelles annotations `@doc` apparaissent immédiatement dans les tools MCP
- Les références `@doc.X` cassées déclenchent des warnings STD004 dans ton éditeur
- Les erreurs de syntaxe DSL déclenchent STD007

Rebuild & redéploiement du daemon après un upgrade standardoc :

```sh
./scripts/build.sh    # ou ./scripts/build.ps1 sur Windows
# Choisir [2] prod — kill les serveurs en cours, rebuild dans target/release/.
# Ensuite : ouvrir une nouvelle conversation Claude Code. Pas besoin de redémarrer VSCode.
```

## Où aller ensuite

- [`README.fr.md`](README.fr.md) — toute la surface fonctionnelle
- [`examples/`](examples/) — démos runnables pour Rust, TypeScript, multi-langages et providers dynamiques
- Tool MCP `get_dsl_reference` — référence DSL exhaustive (`each`, `if`, appels de fonction, itération de blocs, …)
- Resources MCP `standardoc://*` — chaque tool et resource MCP, découvrables directement depuis ton IDE
