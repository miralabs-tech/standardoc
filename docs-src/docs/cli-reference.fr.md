# Référence CLI

[English](cli-reference.md) · 📖 Français

Standardoc embarque deux binaires :

- **[`standardoc`](#standardoc-cli)** — CLI one-shot pour scan / transform / emit / validate / materialize
- **[`standardoc-server`](#standardoc-server-daemon)** — daemon long-running avec quatre transports (LSP / MCP / Web / export statique)

---

## `standardoc` — CLI

```
standardoc <commande> [args...]
```

Comportements globaux :
- Toutes les commandes prennent une racine de workspace en premier argument positionnel.
- Le workspace est scanné avec les quatre providers built-in (Rust / TS / Python / Lua tree-sitter) plus tout provider dynamique déclaré dans `.standardoc/languages/*.json`.
- La configuration est lue depuis `.standardoc.json` à la racine du workspace si présent, sinon les défauts s'appliquent.
- La sortie va sur **stdout**. Les statuts, erreurs et compteurs vont sur **stderr** — tu peux piper stdout proprement.

### `scan`

```
standardoc scan <path>
```

Parcourt `<path>` et émet les [`DocBlock`](../crates/standardoc-core/src/model/) canoniques en JSON, un bloc par enregistrement.

Utile pour : piper dans `jq`, construire du tooling externe, debug de discovery, diffs snapshot en CI.

**Codes de sortie** :
- `0` — succès
- `1` — erreur pipeline (path illisible, échec parse)
- `2` — argument requis manquant

**Exemple** :

```sh
standardoc scan ./mon-projet | jq '.[] | {key, kind: .symbol.kind}'
```

### `transform`

```
standardoc transform <path> <template.md>
```

Scan `<path>`, puis rend `<template.md>` contre l'index résultant. Le template utilise le DSL standardoc (`{{ @doc.KEY:tag }}`, `{{ each x in @docs.module(...) }}`, `{{ if ... }}`, …). Résultat imprimé sur stdout.

**Codes de sortie** :
- `0` — render OK
- `1` — erreur pipeline ou render
- `2` — argument manquant

**Exemple** :

```sh
standardoc transform ./mon-projet ./docs-src/api.md > ./public/api.md
```

### `emit`

```
standardoc emit <format> <path> [--name <projet>] [--tagline <ligne>] [--link-base <url>]
```

Génère un des trois standards de doc orientés agents depuis un scan de workspace.

**Formats** :
- `llms` (alias `llms.txt`) — index sommaire [`llms.txt`](https://llmstxt.org/) de Jeremy Howard
- `llms-full` (alias `llms-full.txt`) — variante long-form `llms-full.txt`
- `skill` (alias `skill.md`) — format [`SKILL.md`](https://docs.anthropic.com/en/docs/claude-code/skills) de Claude Code

**Options** :
- `--name <projet>` — surcharge le nom de projet auto-détecté (défaut : nom du dossier racine du workspace)
- `--tagline <ligne>` — courte description embarquée dans le header
- `--link-base <url>` — préfixe URL pour les liens source (ex : `https://github.com/owner/repo/blob/main`)

Sortie sur stdout. Redirige avec `>` pour écrire un fichier.

**Exemple** :

```sh
standardoc emit llms ./mon-projet \
  --name "Mon Projet" \
  --tagline "API REST pour X" \
  --link-base "https://github.com/owner/repo/blob/main" \
  > llms.txt
```

### `validate`

```
standardoc validate <path>
```

Lance la suite validator complète sur un workspace, imprime un diagnostic par ligne au format `<sévérité> [STD###] <path>:<ligne>: <message>`. Un résumé de comptes est imprimé sur stderr.

**Sévérités** : `error`, `warning`, `info`, `hint` — voir la [table des règles du validator dans README.fr.md](../README.fr.md#validator) pour la liste complète.

**Codes de sortie** :
- `0` — aucun diagnostic de sévérité error trouvé (les warnings/info/hints ne font pas échouer)
- `1` — au moins un diagnostic `error`
- `2` — argument manquant

**Exemple** :

```sh
standardoc validate ./mon-projet
# error [STD001] src/lib.rs:42: duplicate DocKey "foo.bar"
# warning [STD006] src/lib.rs:10: public symbol with no @doc annotation
# 1 error(s), 1 warning(s), 0 info, 0 hint(s)
```

Intégration CI : lance `standardoc validate .` comme étape ; un exit non-zéro bloque le merge.

### `materialize`

```
standardoc materialize <path> [--apply] [--confidence low|medium|high]
```

Promeut les annotations virtuelles (synthétisées par le pass virtual-annotation sur les blocs `Inferred`) en vrais doc-comments `///` au niveau du source. Par défaut, fait un dry-run qui affiche exactement ce qui serait inséré, fichier par fichier ; passer `--apply` pour vraiment éditer le source.

**Options** :
- `--apply` — applique les éditions. Sans ce flag, seul un rapport dry-run est imprimé.
- `--confidence <tier>` — confidence minimale requise pour qu'une annotation virtuelle soit éligible. `low` (tout), `medium` (défaut), `high` (seulement les templates les plus sûrs : constructeurs, trait impls, prédicats, etc.).

La sortie respecte la syntaxe doc-comment préférée du langage (`///` pour Rust, `---` pour Lua, `/** … */` pour TS/JS) et préserve l'indentation du symbole qu'elle documente. Python est volontairement non supporté dans ce MVP — les docstrings vivent à l'intérieur du body de la fonction, ce qui demande une logique de placement différente.

**Codes de sortie** :
- `0` — dry-run imprimé, ou `--apply` réussi
- `1` — erreur pipeline ou échec d'écriture
- `2` — argument invalide

**Exemple** :

```sh
# Preview de ce qui serait ajouté sur l'API publique
standardoc materialize ./mon-projet --confidence high

# Vraiment écrire
standardoc materialize ./mon-projet --confidence high --apply
```

### `--help`, `-h`

```
standardoc --help
standardoc -h
```

Imprime la liste des commandes avec usage bref. Sort toujours `0`.

---

## `standardoc-server` — daemon

```
standardoc-server <transport> --workspace <path> [args spécifiques au transport]
```

Un seul binaire, quatre transports mutuellement exclusifs — choisis-en un.

### `--mcp`

```
standardoc-server --mcp --workspace <path>
```

Parle le [Model Context Protocol](https://modelcontextprotocol.io/) sur **stdio** (JSON-RPC 2.0). Utilise-le depuis `.mcp.json` pour exposer le workspace aux agents IA (Claude Code, Cursor, Zed, Continue, …). Voir la [référence MCP](mcp-reference.fr.md) pour la liste complète des tools disponibles.

Le daemon scan une fois au boot, watch le workspace pour les changements, et push des notifications quand l'index change. L'état reste vivant pendant toute la durée du processus host.

### `--lsp`

```
standardoc-server --lsp --workspace <path>
```

Parle [LSP](https://microsoft.github.io/language-server-protocol/) sur **stdio** pour les éditeurs (VSCode, Helix, Neovim, Zed, …). Capabilities :

- Complétion sur les triggers `@`, `{`, `.`, `:`
- Hover, goto-definition (DSL → source), references (source → `.md`)
- Document / workspace symbols, code actions
- **Rename** qui propage les changements de `DocKey` dans tous les `.md` consommateurs
- Formatting, push diagnostics à chaque rescan
- 10 codes de diagnostic (STD001-STD008 + STD012-STD013 ; STD009-STD011 réservés)

### `--web --port <N>`

```
standardoc-server --web --port 4173 --workspace <path>
```

Sert une API HTTP REST + SSE sur le port donné. Endpoints :

- `GET /api/health` — `{ "ok": true, "revision": N }`
- `GET /api/index` — snapshot complet de l'index
- `GET /api/doc/{key}` — détail d'un bloc
- `GET /api/search?q=...` — recherche substring + fallback fuzzy
- `GET /api/dsl-reference` — référence DSL en markdown (même contenu que le tool MCP `get_dsl_reference`)
- `GET /api/config` — config résolue
- `GET /api/pages` — liste des pages narratives
- `GET /api/page/{*slug}` — contenu complet d'une page (aussi `PUT`, `PATCH`, `DELETE`)
- `GET /api/events` — flux Server-Sent Events (`index_changed`, `diagnostics`, …)
- `GET /api/syntax.css` — CSS généré par syntect pour le highlighting de code
- Fallback `/*` — SPA embarquée (seulement quand le binaire est build avec `--features standardoc-web/embedded-frontend`, càd Standardoc Pro), sinon un placeholder

**CORS** est wide-open par défaut (`allow_origin: any`) pour le dev local et les SPA self-hosted. Resserre via reverse-proxy si tu exposes au-delà de `localhost`.

### `--export --out <dir>`

```
standardoc-server --export --workspace <path> --out <dir>
```

Export statique one-shot. Écrit `static-data.json` (snapshot complet de l'index, tous les blocs, pages pré-rendues, config source-link résolue) dans `<dir>`. Si le binaire a été build avec `embedded-frontend`, écrit aussi la SPA bundled comme un site déployable sur CDN ; sinon c'est data-only et consommable par n'importe quel SSG externe (Astro, Vitepress, Hugo, custom).

### `--workspace <path>` *(requis)*

Chemin absolu ou relatif vers la racine du workspace. Standardoc traite ça comme le scope d'indexation et regarde aussi ici pour `.standardoc.json` et `.standardoc/languages/*.json`.

### Codes de sortie

- `0` — shutdown propre
- `1` — erreur runtime (bind failure, erreur scan, etc.)
- `2` — erreur d'argument

---

## Variables d'environnement

Standardoc lui-même ne lit qu'une seule variable d'environnement directement :

- `RUST_LOG` — contrôle le niveau de log pour `tracing` (ex : `RUST_LOG=standardoc=debug`)

Les scripts d'install (`scripts/install.{sh,ps1}`) honorent :

- `STANDARDOC_VERSION` — pin une release spécifique (ex : `v0.1.0`)
- `STANDARDOC_HOME` — racine d'installation (défaut `$HOME/.standardoc` ou `$env:USERPROFILE\.standardoc`)
- `STANDARDOC_NO_PATH` — skip le message de suggestion PATH

## Fichier de configuration

`.standardoc.json` à la racine du workspace — entièrement optionnel. Voir la
[section Configuration dans README.fr.md](../README.fr.md#configuration-standardocjson)
pour le schéma complet.
