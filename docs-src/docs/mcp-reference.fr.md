# Référence MCP

[English](mcp-reference.md) · 📖 Français

Standardoc parle le [Model Context Protocol](https://modelcontextprotocol.io/) sur stdio. Ce document liste chaque tool et resource exposés par le daemon `standardoc-server --mcp`, avec ce que chacun fait concrètement et quand l'utiliser.

**Setup** — dépose un `.mcp.json` à la racine de ton workspace :

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "stdio",
      "command": "/chemin/absolu/vers/standardoc-server",
      "args": ["--mcp", "--workspace", "/chemin/absolu/vers/ton/projet"]
    }
  }
}
```

Lance ensuite n'importe quel client MCP (Claude Code, Cursor, Zed, Continue, …). Le daemon scan une fois au boot, watch le workspace pour les changements et push des notifications à ton client quand l'index change.

---

## Index des tools

| Catégorie | Tools |
|---|---|
| [Lecture & navigation](#lecture--navigation) | `list_docs`, `get_doc`, `search_docs`, `evaluate_dsl`, `render_markdown`, `get_comments` |
| [Cross-référence](#cross-référence) | `find_usages`, `find_implementations`, `get_type_hierarchy`, `search_by_param_type`, `search_by_return_type` |
| [Exposition sémantique LSP](#exposition-sémantique-lsp) | `resolve_symbol`, `get_definition`, `get_hover`, `find_references` |
| [Qualité & validation](#qualité--validation) | `validate_doc_syntax`, `coverage_report`, `find_undocumented`, `list_collisions`, `list_diagnostics` |
| [Émission de docs agents](#émission-de-docs-agents) | `emit_llms_txt`, `emit_llms_full`, `emit_skill_md`, `emit_openapi` |
| [Lifecycle & runtime](#lifecycle--runtime) | `rescan`, `get_dsl_reference`, `set_watch_paused`, `get_watch_status` |

---

## Lecture & navigation

### `list_docs`

**Quoi** : liste chaque `DocBlock` de l'index, optionnellement filtré par tag, prefix, ou path.
**Quand** : l'agent a besoin d'une vue d'ensemble ("qu'est-ce qui est documenté dans ce projet ?") ou d'énumérer des items matchant un pattern avant de creuser.
**Args** : `{ prefix?: string, tag?: string, path_prefix?: string, limit?: number }`
**Retourne** : `{ count: N, entries: [{ key, label, kind, path, line }] }`

### `get_doc`

**Quoi** : payload complet pour un bloc — label, description, tous les tags, infos AST symbole (signature, kind, params), path source, range de lignes.
**Quand** : après que `list_docs` / `search_docs` ait retourné une clé, ou n'importe quand tu as une `DocKey` et veux tout ce qu'on sait dessus.
**Args** : `{ key: string }`
**Retourne** : `DocResponse` avec `{ key, label, description, tags, symbol, meta, ... }`

### `search_docs`

**Quoi** : recherche substring sur `key`, `label`, signature, params, returns, generics, tous les tags. Fallback automatique en scoring fuzzy token-based si la substring retourne 0 hit.
**Quand** : l'agent a un concept vague ("y a-t-il quelque chose sur l'auth ?") plutôt qu'une clé précise.
**Args** : `{ query: string, limit?: number }`
**Retourne** : `{ mode: "exact" | "fuzzy", count: N, entries: [...] }` — le champ `mode` te dit si c'est un hit substring exact ou un fallback fuzzy.

### `evaluate_dsl`

**Quoi** : évalue une expression DSL unique (`@doc.KEY:tag`, `@docs.module(prefix)`, blocs `each`, `if/else`, …) contre l'index live, retourne la/les valeur(s) résolues.
**Quand** : l'agent veut tester une expression DSL avant de la committer dans un template `.md`.
**Args** : `{ expression: string }`
**Retourne** : `{ value: string | array | object, errors: [...] }`

### `render_markdown`

**Quoi** : rend un template markdown complet (avec DSL embarqué) et retourne la string markdown résultante.
**Quand** : prévisualiser le rendu complet d'un template sans écrire de fichier. Équivalent de `standardoc transform <path> <template>` mais en mémoire.
**Args** : `{ template: string }` (contenu du template, pas un path)
**Retourne** : `{ rendered: string, errors: [...] }`

### `get_comments`

**Quoi** : extrait tous les commentaires (réguliers, doc, blocs) d'un fichier source via tree-sitter, avec numéros de ligne 1-based.
**Quand** : l'agent a besoin de lire chaque commentaire d'un fichier sans le parser à la main — utile pour des scripts de migration, audits de doc, ou tooling d'annotation.
**Args** : `{ file: string }` (path relatif ou absolu)
**Retourne** : `{ count: N, comments: [{ line, text }] }`. Marche seulement pour les langages avec une grammaire tree-sitter enregistrée (Lua + providers dynamiques aujourd'hui ; les providers natifs Rust/TS/Python ne sont pas encore couverts).

---

## Cross-référence

### `find_usages`

**Quoi** : trouve chaque endroit du codebase qui référence un symbole donné par nom (ex : `ParseError`).
**Quand** : analyse d'impact avant un rename, audit des consommateurs d'API publique, localisation du graphe de dépendances inverse.
**Args** : `{ name: string, from_path_prefix?: string, from_key_prefix?: string, limit?: number }`
**Retourne** : `{ count: N, entries: [{ from_key, from_path, from_line }] }` — les filtres `from_*` désambiguïsent quand plusieurs symboles partagent le même nom court.

### `find_implementations`

**Quoi** : liste les types qui implémentent un trait / une interface donnés.
**Quand** : "montre-moi tout ce qui satisfait le trait `Display`", "quelles classes étendent `BaseHandler` ?".
**Args** : `{ trait_name: string, limit?: number }`
**Retourne** : `{ count: N, entries: [{ key, label, path, line }] }`

### `get_type_hierarchy`

**Quoi** : arbre complet des sous-types pour un type donné — implémentations d'un trait et sous-types structurels.
**Quand** : visualiser une chaîne d'héritage, générer la doc d'une API polymorphe.
**Args** : `{ name: string }`
**Retourne** : arbre imbriqué `{ root, children: [...] }`

### `search_by_param_type`

**Quoi** : trouve chaque fonction/méthode dont les paramètres incluent un type donné.
**Quand** : "quelles fonctions prennent un `User` ?" — utile pour planifier un refactor ou la découvrabilité de l'API.
**Args** : `{ name: string, from_path_prefix?: string, from_key_prefix?: string, limit?: number }`
**Retourne** : `{ count: N, entries: [...] }`

### `search_by_return_type`

**Quoi** : trouve chaque fonction/méthode qui retourne un type donné.
**Quand** : "quelles fonctions peuvent produire un `Result<User, Error>` ?" — symétrique à `search_by_param_type`.
**Args** : `{ name: string, from_path_prefix?: string, from_key_prefix?: string, limit?: number }`
**Retourne** : `{ count: N, entries: [...] }`

---

## Exposition sémantique LSP

Ces tools mirrorent des comportements que le LSP donne aux éditeurs, mais exposés en tools MCP pour que les agents (qui ne parlent pas LSP) puissent les utiliser.

### `resolve_symbol`

**Quoi** : nom de symbole court → liste de candidats `DocKey` fully-qualified, classés par précision (match exact label > suffix FQN > FQN contient).
**Quand** : l'agent a une référence casuelle comme `"create"` et a besoin de désambiguïser vers la vraie clé (`api.users.create`) avant d'appeler d'autres tools.
**Args** : `{ name: string, path_prefix?: string }`
**Retourne** : `{ count: N, candidates: [{ key, label, kind, score }] }`

### `get_definition`

**Quoi** : payload léger de navigation pour une clé — `{ path, line, abs_path }`. Designed pour l'UX "go to source".
**Quand** : l'agent produit une référence cliquable ou veut lire le source brut après avoir localisé un symbole.
**Args** : `{ key: string }`
**Retourne** : `{ path: string, line: number, abs_path: string }`

### `get_hover`

**Quoi** : contenu hover formaté en markdown, identique à ce que le LSP montre au hover dans ton éditeur.
**Quand** : l'agent veut une "preview card" pour un symbole — signature + description + tags formatés pour lecture humaine.
**Args** : `{ key: string }`
**Retourne** : `{ markdown: string }`

### `find_references`

**Quoi** : scan toutes les pages `.md` pour les occurrences `@doc.KEY` qui référencent ce bloc.
**Quand** : "avant de supprimer cette `DocKey`, quelles pages narratives la mentionnent ?". Distinct de `find_usages` qui parcourt le type-graph dans le code source.
**Args** : `{ key: string }`
**Retourne** : `{ count: N, entries: [{ path, line }] }`

---

## Qualité & validation

### `validate_doc_syntax`

**Quoi** : valide une string d'annotation `@doc` (sans la committer dans le source) — vérifie syntaxe des tags, format de clé, plausibilité de type, etc.
**Quand** : l'agent est sur le point d'écrire une nouvelle annotation et veut confirmer qu'elle parse proprement avant.
**Args** : `{ source: string, language?: "rust" | "ts" | "python" | "lua" }`
**Retourne** : `{ valid: boolean, diagnostics: [{ severity, code, message }] }`

### `coverage_report`

**Quoi** : stats de couverture doc workspace-wide — total de symboles documentables, combien sont documentés, breakdown par kind / par fichier.
**Quand** : "à quel point ce projet est documenté ?" — utile pour fixer des CI gates ou triager les zones les moins couvertes.
**Args** : `{}` (aucun argument)
**Retourne** : `{ total, documented, percentage, by_kind: {...}, by_file: [...] }`

### `find_undocumented`

**Quoi** : liste chaque symbole public sans annotation `@doc`.
**Quand** : queue de travail pour l'agent — "pour chaque fonction non documentée, propose une annotation". À combiner avec `validate_doc_syntax` et des éditions de source.
**Args** : `{ path_prefix?: string, kind?: string, limit?: number }`
**Retourne** : `{ count: N, entries: [{ name, kind, path, line }] }`

### `list_collisions`

**Quoi** : chaque collision de clé dans l'index (plusieurs blocs revendiquent la même `DocKey`).
**Quand** : investigation de root cause quand une référence `@doc.X` se résout vers le mauvais bloc, ou après un rename qui a introduit un doublon.
**Args** : `{}` (aucun argument)
**Retourne** : `{ count: N, collisions: [{ key, kept: {...}, dropped: [...] }] }`

### `list_diagnostics`

**Quoi** : diagnostics actuels du validator, même set que le LSP push aux éditeurs. Codes STD001-STD012.
**Quand** : l'agent a besoin de l'état live errors/warnings du workspace avant de suggérer des fixes.
**Args** : `{ severity?: "error" | "warning" | "info" | "hint", code?: string, path_prefix?: string, limit?: number }`
**Retourne** : `{ count: N, diagnostics: [{ code, severity, path, line, message }] }`

---

## Émission de docs agents

Ces tools génèrent des formats de doc orientés agents bien connus depuis ton scan de workspace live.

### `emit_llms_txt`

**Quoi** : format index sommaire `llms.txt` de Jeremy Howard.
**Args** : `{ name?: string, tagline?: string, link_base?: string }`
**Retourne** : `{ output: string }` — contenu complet du fichier en string unique.

### `emit_llms_full`

**Quoi** : variante long-form `llms-full.txt`, inclut les descriptions de blocs complètes.
**Args** : pareil que `emit_llms_txt`
**Retourne** : `{ output: string }`

### `emit_skill_md`

**Quoi** : format Claude Code [`SKILL.md`](https://docs.anthropic.com/en/docs/claude-code/skills) — packagise ton codebase comme un skill réutilisable qu'un agent peut load.
**Args** : pareil que `emit_llms_txt`
**Retourne** : `{ output: string }`

### `emit_openapi`

**Quoi** : spec OpenAPI 3.0 générée depuis les tags `@route` / `@param` / `@response`.
**Quand** : ton codebase a des endpoints REST annotés avec metadata route et tu veux une spec consommable par Swagger UI, Postman, des générateurs de code.
**Args** : `{ title?: string, version?: string, server_url?: string }`
**Retourne** : `{ openapi: string }` (YAML, par convention de la spec).

---

## Lifecycle & runtime

### `rescan`

**Quoi** : force un re-scan complet du workspace, en bypass du watcher.
**Quand** : l'agent suspecte que l'index est stale (rare — le watcher est debouncé, auto-pause sur les parse storms, et émet des notifications de change) ou après une opération bulk de fichiers en dehors du périmètre du workspace.
**Args** : `{}`
**Retourne** : `{ ok: true, revision: N, scanned_files: K }`

### `get_dsl_reference`

**Quoi** : retourne le markdown de référence DSL canonique — même contenu que l'endpoint `--web` `/api/dsl-reference` et le fichier livré dans le binaire.
**Quand** : l'agent ou l'utilisateur a besoin de consulter la syntaxe DSL complète (accesseurs, `each`, `if/else`, appels de fonction, itération de blocs) sans quitter le chat / l'IDE.
**Args** : `{}`
**Retourne** : `{ markdown: string }`

### `set_watch_paused`

**Quoi** : pause ou reprend le watcher filesystem. Pendant la pause, les changements de fichiers ne déclenchent pas de rescans.
**Quand** : l'agent est sur le point de faire un bulk d'éditions de source et veut supprimer le firehose de diagnostics intermédiaires ; reprend quand fini.
**Args** : `{ paused: boolean }`
**Retourne** : `{ paused: boolean, revision: N }`

### `get_watch_status`

**Quoi** : état actuel du watcher — paused?, timestamp du dernier rescan, dernier numéro de revision, nombre de changes en queue.
**Quand** : sanity check avant de se reposer sur un état fresh de diagnostics.
**Args** : `{}`
**Retourne** : `{ paused: boolean, last_rescan_ms_ago: N, revision: N, queued: K }`

---

## Resources

Les resources MCP sont des données read-only exposées sous des URIs. Standardoc en expose quatre :

| URI | Quoi |
|---|---|
| `standardoc://index` | Snapshot complet de l'index (tous les blocs, toutes les métadonnées). Lourd — les agents préfèrent typiquement `list_docs` + `get_doc` à la place. |
| `standardoc://config` | `.standardoc.json` résolu pour le workspace courant. |
| `standardoc://schema/dsl` | Référence de grammaire DSL (forme machine-readable de `get_dsl_reference`). |
| `standardoc://schema/tags` | Catalogue des noms de `@tag` reconnus avec leur cardinalité et leurs champs acceptés. |

S'abonner à une resource (par spec MCP) pour recevoir des mises à jour quand la donnée sous-jacente change.

---

## Notifications

Le daemon push des notifications JSON-RPC quand l'état change. Les hosts qui s'abonnent voient :

- `notifications/standardoc/index_changed` — `{ revision, added, removed }` (listes de `DocKey`, pas les blocs entiers — refetch via `get_doc` si besoin) après chaque rescan réussi
- `notifications/standardoc/diagnostics` — `{ path, diagnostics: [...] }` quand la sortie validator change pour un fichier
- `notifications/standardoc/config_reloaded` — `{ config }` après que `.standardoc.json` est édité

Ça permet aux agents de réagir aux changements de fichiers sans polling — même mécanisme de delivery que le LSP utilise pour `publishDiagnostics`.
