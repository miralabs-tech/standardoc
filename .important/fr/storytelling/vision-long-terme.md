# Vision long terme — au-delà de la 1.0

[English](../../en/storytelling/vision-long-term.md) · 📖 Français

[← Vision moyen terme](vision-moyen-terme.md) · [← Philosophie](philosophy.md) · [Remarques](remarques.md) · [Retours de tests](retours-tests.md)

> Ce document décrit l'inflexion que prend Standardoc **après** la
> 1.0. La liste des idées (sans engagement de calendrier) vit dans
> [`TODO-LIST.md`](../TODO-LIST.md) — section *Post-1.0 ideas*.

---

## L'inversion post-1.0 : le core ne grossit pas, l'écosystème, oui

À 1.0, l'API est figée. **Toute pression d'ajout de features cesse
de pousser sur le core** — elle pousse sur l'écosystème qui s'enroule
autour.

C'est un choix philosophique fort. Jusqu'à 1.0, le core a grossi
naturellement : RAG, sessions DB, MCP toolkit, 5 language providers,
hooks MCP-first, doc rendering layer en cours. Chaque cycle a ajouté
une pièce d'infrastructure. **C'est sain pendant la phase de
construction.** Maintenu post-1.0, ça produit un god-binary — un
core qui finit par tout faire mal au lieu de bien faire son métier
de base.

Le métier de base reste : **graphe sémantique vivant + surfaces de
consommation stables**. Tout ce qui peut être délégué doit être
délégué. Le core devient minimal et stable ; l'extensibilité passe
par un système de plug-ins externes.

Ce n'est pas une retraite. C'est l'architecture qui rend Standardoc
durable à 5 ans et plus.

---

## Le plug-in layer UST + Lua — la pièce centrale

**Cadrage honnête : aucune brique de ce plug-in layer n'existe en
code à ce jour.** mlua n'est pas intégré dans le projet ;
tree-sitter non plus ; aucune spec UST écrite ; pas de plug-in
discovery. C'est pure roadmap. Mais l'architecture est claire et
les use cases déjà identifiés en dogfood.

### L'architecture cible

```
source code
  → tree-sitter (parser universel, 100+ grammars community)
  → UST (Universal Symbol Tree — schéma minimal commun)
  → Lua plugin (mlua sandboxé) — définit symboles, edges, attributs
  → core Rust valide la conformité au schéma IR + stocke
```

Le parsing est délégué à tree-sitter, la transformation sémantique
à Lua, et le core Rust ne valide que la conformité au schéma IR.
**Ajouter une langue ou un détecteur cesse d'être un PR sur le
core** ; c'est un fichier `.standardoc/plugins/<lang>.lua` posé
dans le workspace ou shippé via un canal communautaire.

### Pourquoi Lua plutôt que WASM

Trois raisons :

1. **Lower barrier.** Lua se lit en 30 minutes. Rust + WASM bindgen
   prennent des jours. Pour qu'un détecteur Prisma écrit par un dev
   front-end communautaire ait une chance d'exister, il faut que le
   coût d'écriture soit absorbable en une soirée.
2. **mlua est éprouvé dans l'écosystème StandarX.** D'autres projets
   internes l'utilisent déjà comme moteur de hooks sandboxés. La
   capability-based sandboxing (pas d'accès filesystem / réseau /
   process par défaut) est solide.
3. **WASM reste l'option pour les plug-ins natifs haute performance.**
   Quand un parser Lua devient trop lent (cas extrême sur très gros
   monorepos), un binding WASM compilé peut prendre le relais. Pour
   95% des cas (détecteurs déclaratifs, transformations AST), Lua
   suffit largement.

### Use case canonique 1 : détecteurs cross-substrat

Toute la combinatoire substrat × langage × ORM que la 1.0 a refusé
de figer dans le core (cf. [vision moyen terme](vision-moyen-terme.md) —
*Bridges cross-substrat*). Tauri commands, WASM bindings, FFI
declarations, Prisma queries, Drizzle, SQLAlchemy, Mongoose, schemas
SQL inline, GraphQL resolvers, REST endpoints → DB tables.

Chaque détecteur = un plug-in Lua. Le contributeur écrit son
plug-in Prisma une fois, le partage, **tous les projets qui
utilisent Prisma + Standardoc en bénéficient sans toucher au
Rust core**.

### Use case canonique 2 : import/export commentaires safe-edit

Réintroduction de la commande `materialize` (puntée en beta.1, cf.
[retours de tests](retours-tests.md) — *Ce qu'on a abandonné*) en
version rigoureuse. Le plug-in écrit des commentaires structurés —
doc-comments, blocs `@doc`, JSDoc, rustdoc — **dans la source**,
**ancrés sur FQDN** plutôt que sur des line ranges (plus stables
aux refactors).

Le but final : maintenir une **codebase épurée** — code nu, juste
signature + body — avec la doc qui vit dans le graphe et la
capacité de **réinjecter localement à la demande**, sans risque de
désynchro entre la doc et la source.

---

## L'expansion mécanique des langues

Conséquence directe du plug-in layer. Aujourd'hui Standardoc a
3 providers de langage Rust built-in (Rust via `syn`,
TS / JS / JSX / TSX — React inclus — via `swc`, Lua via
`full_moon`), plus le support de Vue et Svelte via parsing SFC
(le `<script>` extrait → provider TS, les `<template>` via des
parsers SFC custom). Ajouter une nouvelle langue aujourd'hui =
écrire un provider Rust = un PR significatif sur le core, avec
maintenance long-terme à charge des mainteneurs.

Post-1.0, **ajouter une langue = écrire un plug-in Lua sur la
grammar tree-sitter de cette langue**. Le coût chute d'un ordre
de grandeur. Conséquences :

- **Go, Java, Swift, C#, Kotlin, Zig, Python** — toutes les
  langues où tree-sitter a une grammar mature deviennent
  indexables sans modification du core
- **C et C++** — le cas dogfood LurLang (langage perso Rust + C,
  le C n'est pas indexé en 1.0) trouve enfin une réponse via
  plug-in
- **Schemas DB déclaratifs** — `schema.prisma`, `models.py`,
  migrations SQL deviennent des **frontends à part entière**
  parsés par des plug-ins Lua. Leur contenu (Table, Column, Model)
  entre dans le graphe au même titre qu'un symbole de code,
  linkable par les détecteurs ORM (use case canonique 1)
- **Configs structurées** — `.gql`, `.proto`, `openapi.yaml`
  peuvent être indexés via plug-in si la demande émerge

Le core garde ses providers natifs (Rust / TS / Lua, + le
support SFC Vue / Svelte) — les langages qui ont porté
Standardoc en dogfood jusqu'à 1.0. **Tout le reste passe par
plug-in.**

---

## Les surfaces enrichies optionnelles

Standardoc à 1.0 expose le graphe via LSP, MCP, RAG, doc rendering
(beta.3), navigation visuelle (beta.3). Post-1.0, des surfaces
optionnelles peuvent émerger selon la demande dogfood — toujours
optionnelles, jamais imposées.

### Custom LSP methods

Des méthodes LSP non-standardisées spécifiques à Standardoc — par
exemple `standardoc/findCallers(fqdn)`, `standardoc/showEdges(fqdn)`,
`standardoc/checkStale(fqdns)`. Les clients LSP qui les
implémentent gagnent en richesse. Le standard LSP reste supporté
en parallèle pour la compat universelle.

### LSP bridge to rust-analyzer / tsserver

Pour les questions où la profondeur per-langage compte (inférence
de type complexe Rust, completion TS contextuelle, expansion
macro), Standardoc peut **ponter** vers rust-analyzer ou tsserver
et fusionner leur réponse avec sa propre vue graphe. L'agent
obtient le meilleur des deux mondes via une seule interface MCP —
graphe sémantique cross-langage **plus** profondeur per-langage
des LSP officiels.

### Doc UI local optionnel

Si la demande émerge — pas par défaut — une **UI doc locale style
GitBook** qui consomme le graphe et le rendu beta.3. Servie en
HTTP local, gitignored, **jamais hostée par StandarX**. L'idée :
navigation visuelle plus riche que la webview VSCode pour les
projets qui veulent publier leur doc autour du graphe Standardoc.

Probablement sous **licence lifetime distincte** du core
open-source (cf. [SUPPORT.md](../SUPPORT.md)) si elle devient un
asset de financement de StandarX. **Le core, lui, reste FSL → MIT
inchangé.**

---

## Les invariants protégés à très long terme

Les 5 invariants posés à 1.0 restent intacts :

- **L'IR reste stable.** Bump `protocol_version` + coexistence
  obligatoire pour tout breaking change.
- **Le graphe reste local.** Pas de cloud sync, pas d'auth, pas
  de telemetry phone-home.
- **Le license timer reste armé.** FSL → MIT 2 ans par release.
  Première release : 26 avril 2028.
- **Le format SQLite reste ouvert.** Schema versionné, lisible
  avec un sqlite3 standard.
- **L'API publique est gelée.** MCP tools, méthodes LSP custom,
  types IR exportés, schema SQLite.

Et **un 6e invariant apparaît post-1.0** :

- **Le core ne grossit pas.** Toute pression d'extension passe
  par le plug-in layer, pas par un ajout dans le core Rust. Cette
  contrainte préserve la simplicité du système et la lisibilité
  du contrat IR. Le sandboxing du plug-in (mlua + capability-based)
  est lui aussi figé.

---

## Ce qu'on ne fait PAS post-1.0

Cadrage négatif essentiel pour ne pas dériver :

- **Pas de service hosted obligatoire.** Si une UI doc hostée
  émerge un jour, elle reste optionnelle, et le core fonctionne
  intégralement sans elle.
- **Pas de plug-in registry centralisé monétisé.** Les plug-ins
  se distribuent via GitHub, fichiers locaux, canaux
  communautaires — comme les dotfiles. Pas de marketplace
  propriétaire qui captive l'écosystème ni qui exfiltre des
  données d'usage.
- **Pas de telemetry phone-home.** Jamais. Même opt-in. C'est un
  invariant culturel non-négociable.
- **Pas de breaking change unilatéral de l'IR.** Bump
  `protocol_version` + coexistence obligatoire pour toute
  évolution.
- **Pas de scope creep côté core.** Si une feature peut vivre en
  plug-in, elle vit en plug-in. Si elle ne peut pas, on
  questionne d'abord *pourquoi* avant d'élargir le core.

---

## Pour aller plus loin

- **[← Vision moyen terme](vision-moyen-terme.md)** — beta.3 et 1.0
- **[← Philosophie](philosophy.md)** — les 5 principes
  system-thinking et l'éthique de construction
- **[Remarques](remarques.md)** — observations dogfood, décisions
  lockées
- **[Retours de tests](retours-tests.md)** — ce qu'on a testé, ce
  qu'on a abandonné, les estimations
- **[TODO-LIST → Post-1.0 ideas](../TODO-LIST.md)** — checkboxes
  exhaustives par milestone (section *Post-1.0 ideas, no
  commitment*)
