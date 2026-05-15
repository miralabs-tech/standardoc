# Vision moyen terme — beta.3 et 1.0

[English](../../en/storytelling/vision-mid-term.md) · 📖 Français

[← Vision court terme](vision-court-terme.md) · [← Philosophie](philosophy.md) · [Vision long terme →](vision-long-terme.md)

> Ce document est le **narratif** derrière beta.3 et la stabilisation 1.0.
> La liste exhaustive des features par milestone vit dans
> [`TODO-LIST.md`](../TODO-LIST.md) — c'est elle qui bouge, pas ce doc.

---

## Où on s'inscrit

beta.2 a posé la maturité : surface MCP en toolkit, archi
multi-frontend validée, discipline encodée dans le système. Ce qui
suit n'est pas une explosion de nouvelles features.

**beta.3 a un thème : pluraliser les usages du graphe.** Jusqu'ici,
le graphe servait essentiellement à un agent IA en session unique
dans un éditeur. beta.3 ouvre quatre nouvelles surfaces de
consommation :

1. La **doc rendue** pour les visiteurs externes (consommateurs de
   l'API du projet)
2. La **navigation visuelle** pour les mainteneurs internes (humains
   qui auditent leur propre code dans l'IDE)
3. Le **CLI autonome** pour les usages hors-VSCode (ops, CI, devs
   non-IDE Microsoft)
4. La **compréhension projet persistée** pour la continuité
   cross-session (l'agent qui reprend son travail sans re-découvrir
   le contexte narratif à chaque fois)

**1.0**, lui, scelle les contrats : conventions figées, benchmarks
publiés, droit de breaking change unilatéral éteint.

**Mais 1.0 n'est pas la prochaine étape après beta.3.** Entre les
deux, plusieurs `beta.X` supplémentaires émergeront probablement —
chaque fois qu'un besoin se révèle en dogfood sans qu'on l'ait
planifié, comme ce fut le cas pour beta.2. **La 1.0 n'est pas un
calendrier, c'est un critère de maturité** : quand l'API est jugée
digne d'être figée, et pas avant.

---

## beta.3 — pourquoi une couche de rendu maintenant

Le DSL templating v0 (`{{ @doc.X }}`) a été tué en beta.1. Le graphe est
complet, l'IR canonique tient la route, les surfaces MCP/LSP/RAG sont
mûres — mais entre le graphe et un site de doc statique, il n'y a
encore rien.

**On a délibérément attendu.** Écrire un renderer au-dessus d'une API
qui bouge encore est un gaspillage : chaque breaking change côté graphe
casse le rendu. beta.2 a stabilisé la surface ; beta.3 peut maintenant
poser le rendu sans risquer de tout réécrire dans 3 mois.

### L'architecture du rendu

Le graphe SQLite reste **la** source de vérité. Pas MDX, pas Markdown,
pas une seconde source à maintenir en miroir. Les renderers sont des
**consommateurs** du graphe, pas des sources.

- **`@standardoc/core`** — query API framework-agnostic en plain JS/TS.
  `queryDocs("api.*")` interroge le graphe via une couche client, sans
  imposer React / Vue / autre. Un projet qui veut juste sortir du
  Markdown depuis le graphe peut consommer `@standardoc/core` sans rien
  d'autre.
- **`@standardoc/react`** — premier renderer. Composants `<Doc id>`,
  `<Params id>`, `<Examples id>`, `<Signature id>`. Adapters drop-in
  pour Next.js, Nextra, Astro, Docusaurus.
- **Renderers Vue / Svelte** — même graphe, packages séparés,
  post-beta.3. Le framework-agnostic de `@standardoc/core` rend ces
  ajouts mécaniques, pas architecturaux.

### Les annotations narratives

Le rendu a besoin de plus que la signature — il a besoin de la prose
qui décrit *à quoi sert* une fonction, ce que ses paramètres
signifient, des exemples d'usage. C'est le rôle des annotations
`@doc`, `@param`, `@returns`, `@example`.

**Cette mécanique existe déjà dans le code, sur Lua.** Le provider
emmylua (`enrich_signature` dans
`standardoc-lang-provider::lua::emmylua`) parse les commentaires
`---@param` / `---@return` / `---@field` et enrichit la signature avec
les métadonnées extraites. beta.3 généralise ce pattern à **JSDoc**
(TS / JS / Vue / Svelte) et **rustdoc** (Rust) via des hooks
language-provider. Pas de DSL custom — on capitalise sur les
conventions déjà universelles de chaque langue.

---

## beta.3 — la navigation visuelle pour les mainteneurs

Le graphe ne sert pas qu'aux agents IA. **L'autre consommateur, souvent
oublié dans le marketing AI-first, c'est l'humain qui maintient le
code.** Le dev qui revient sur son projet après 6 mois. Le mainteneur
qui audite une codebase qu'il avait quittée il y a 2 ans. Le reviewer
qui découvre un module qu'il n'a jamais touché.

**L'humain comprend son projet avant que l'agent en ait besoin.** Si
le dev lui-même n'arrive plus à se réorienter dans sa codebase, aucune
IA ne suffira à compenser. Le contrôle humain long-terme passe par une
surface qui rend le graphe **directement lisible** — pas seulement
interrogeable.

Pour ces cas-là, une signature renvoyée par le LSP ne suffit pas. Un
résultat MCP donné à un agent non plus. **Il faut une surface visuelle
interactive** dans l'IDE :

- Affichage du graphe local autour d'un symbole (callers / callees /
  imports / imported_by typés)
- Navigation par clic — drill dans un voisin, retour en arrière,
  marquage de symboles d'intérêt
- Vue compacte des enrichissements (descriptions, exemples, params
  annotés) sans ouvrir chaque fichier
- Filtrage par `kind` / `visibility` / langue pour cadrer un audit

C'est techniquement une **webview Preact** embarquée dans l'extension
VSCode. **Calendrier non garanti** — elle est candidate pour beta.3
parce que la valeur dogfood est haute, mais peut glisser vers une beta
ultérieure selon ce qui émerge en parallèle.

### Pourquoi ce besoin remonte si haut dans la roadmap

Le cas d'usage observé en dogfood : **LurLang** — le langage maison
cité comme cible dogfood en [vision court terme](vision-court-terme.md),
2-3 ans d'inactivité — a pu être repris sans phase de réarchéologie.
La raison n'était pas l'IA. C'était le fait que le graphe rendait la
ré-immersion **faisable en une session**, là où autrement c'était des
semaines de re-lecture. Ce qui faisait défaut, ce n'était pas le
temps ou l'effort — c'était l'**élan psychologique** qu'on perd à
regarder 100k lignes écrites par le soi-d'il-y-a-2-ans sans repère
structurel.

Ce cas est exactement le 5e principe system-thinking de
[`philosophy.md`](philosophy.md) ("qu'est-ce qui devient
incompréhensible dans 6 mois ?"), appliqué cette fois au **mainteneur
humain solo** plutôt qu'à l'agent IA. **Standardoc facilite la review
et l'audit sur le très long terme** — c'est un angle de valeur qui n'a
rien à voir avec les agents, et qui justifie de remonter la webview
plus haut que "post-1.0".

---

## beta.3 — standardoc autonome hors-VSCode

Tous les utilisateurs ne sont pas en VSCode. Pour celui qui consomme
`standardoc` via `cargo install`, `curl | sh`, ou directement le
binaire depuis une release GitHub, il faut que le CLI soit
**autosuffisant** — pas dépendant de l'extension VSCode pour se mettre
à jour ou s'installer correctement.

- **`standardoc self-update`** — lit `version.json` depuis la dernière
  release, détecte la plateforme, télécharge l'artefact correspondant,
  SHA256-vérifie, remplace le binaire en place (avec gestion Windows
  rename-on-replace).
- **Injection PATH** à l'installation initiale — `~/.stdoc/bin/` (Unix)
  ou `%USERPROFILE%\.stdoc\bin\` (Windows), avec ajout au shell profile
  (bash / zsh / PowerShell) et à `HKCU\Environment\Path` côté Windows.
- **One-liner bootstrap** — `curl -sSf https://… | sh` (Unix) et
  `irm https://… | iex` (PowerShell).

L'enjeu n'est pas l'ergonomie CLI. **C'est que le binaire doit savoir
vivre hors-VSCode** — pour les serveurs CI, pour les ops, pour les
devs qui ne veulent pas d'IDE Microsoft, pour les pipelines
automatisés. Cette plumbing réutilise par ailleurs la mécanique
`version.json` posée en beta.2 pour le binary-decoupling de
l'extension — ce n'est pas une nouvelle invention, c'est une
généralisation d'un mécanisme déjà éprouvé.

---

## beta.3 — la compréhension projet persistée cross-session

Le graphe sait suivre le code. Les sessions DB savent persister les
décisions d'agent. Mais entre une session d'aujourd'hui et une
session de la semaine prochaine, **la compréhension synthétique du
projet** — ses objectifs court/moyen/long terme, sa posture, ses
décisions structurantes lockées, son intention narrative — se
reconstruit à chaque fois par fetches dispersés (RAG + memos +
relecture des docs).

Pour du code, ce n'est pas grave : le graphe est suffisant. **Pour
de la rédaction narrative ou des décisions de scope, c'est cher.**
L'agent doit re-découvrir le ton, les principes, les choix figés
avant de pouvoir contribuer dans la ligne du projet.

beta.3 candidate : étendre `sessions.db` pour persister une
**compréhension globale projet cross-session** — synthèse vivante
des objectifs, posture, décisions, ton. Pas un dump bullets : une
**structure exploitable** que l'agent consulte en un tool call.

**Garde-fou non négociable** : la vérité reste le code source.
Toute synthèse persistée doit être **ré-validable par re-check du
graphe** à la session suivante. Si elle contredit la réalité du
code, c'est elle qui se corrige, pas le code. La synthèse est une
projection dérivée, invalidable à tout instant — jamais une source
de vérité indépendante.

Ce chantier vient d'une observation dogfood récente
(cf. [retours-tests](retours-tests.md)) : générer les `.md` de
présentation projet a consommé plus de tokens que toute la phase
shipping beta.1 → beta.2. **Calendrier non garanti** — candidate
pour beta.3 si on n'observe pas de trou plus prioritaire pendant
les 2 semaines de tests, sinon glisse vers beta.4.

---

## 1.0 — l'API freeze, en profondeur cette fois

Le principe a été posé en [vision court terme](vision-court-terme.md) :
à 1.0, on contractualise. Ce que ça veut dire concrètement, item par
item.

### Virtual annotations enrichments

**Cadrage honnête : la couche storage existe déjà.** Le module
`standardoc-core::storage::enrichments` ship la table SQLite, l'API
`upsert_enrichment` / `get_enrichment`, les types `EnrichmentInput` et
`ConfidenceLevel` (`Low` / `Medium` / `High`), la cascade FK sur
suppression, et les tests round-trip. Le premier consumer (emmylua sur
Lua) tourne déjà en production.

Ce que 1.0 fige, ce n'est pas la primitive — c'est les **conventions
qui la remplissent** :

- *Verb-prefix conventions* — comment un nom de fonction (`get_*`,
  `find_*`, `parse_*`, …) génère une description par défaut quand
  aucune doc-comment n'est trouvée
- *Type-signature narratives* — comment une signature compose une
  description en langage naturel (params + returns + modifiers)
- *Trait impl templates* — comment décrire l'instanciation d'un trait
  pour un type donné

Et l'**extension du premier consumer aux deux frontends manquants** :
rustdoc parser côté Rust, JSDoc parser côté TS. À 1.0, ces trois
sources d'enrichissement (rustdoc / JSDoc / emmylua) sont disponibles
et leur sémantique est figée.

### Bridges cross-substrat (Tauri / WASM / FFI / DB / ORM / …)

**Cadrage honnête : la primitive IR existe déjà.**
`standardoc-ir::bridge_kind::BridgeKind` est un tag opaque
(`pub struct BridgeKind(pub String)`) attaché aux edges et aux
signatures depuis beta.2. Cette primitive sert de point de
rendez-vous pour décrire des arêtes qui **traversent un substrat
hétérogène** :

- **Cross-language au sens classique** — code Rust ↔ JS via Tauri,
  WASM bindings, FFI déclarations C/C++
- **Code ↔ schéma de données** — code applicatif ↔ table / modèle
  DB via ORM (Prisma, Drizzle, Diesel, SeaORM, Mongoose,
  SQLAlchemy, …) ou requêtes SQL inline
- **Autres ponts** — code ↔ IPC, code ↔ système externe, à
  cartographier au fil du dogfood

Ce que 1.0 fige, c'est **le vocabulaire des kinds** — `"tauri"`,
`"wasm"`, `"ffi"`, `"sql"`, `"orm"`, `"db-table"`, `"db-model"`, et
ce qu'on aura besoin de définir d'ici là. À partir de 1.0, l'ajout
d'un nouveau kind devient un changement de protocole, pas un choix
interne.

**Calendrier de l'extension du vocabulaire** : sur une des betas
entre maintenant et 1.0, sans engagement sur laquelle. C'est
dogfood-driven — chaque nouveau substrat rencontré en pratique
fait remonter une demande, et le vocab grossit en conséquence.
**Doit shipper avant le freeze 1.0**, sinon impossible d'ajouter
des kinds sans breaking change.

**Les détecteurs frontends, eux, arrivent post-1.0 via le plug-in
layer** (cf. [vision long terme](vision-long-terme.md) — UST +
Lua). Le contributeur écrit son détecteur Prisma / Tauri /
SQLAlchemy / … en Lua, sans toucher au Rust core. **Aucun
détecteur built-in n'est promis à 1.0** : la combinatoire
substrat × langage × ORM est trop large pour absorber dans le
core, et le plug-in layer est précisément conçu pour distribuer ce
travail à l'écosystème.

L'effet net visé (une fois les détecteurs livrés post-1.0) :
tracer un click handler React jusqu'à la fonction Rust qu'il
invoque via Tauri ; tracer un endpoint REST jusqu'à la table SQL
qu'il update ; tracer une mutation GraphQL jusqu'à son model
Prisma. **Une seule arête typée du graphe**, pas du grep.

*Note honnête : quand on aura tout ça, Standardoc sera
profondément post-1.0. Le mot juste pour ce moment : **ENFIN**.*

### Perf benchmarks publiés

Cold start, watcher delta, MCP query latency p99 — mesurés sur des
monorepos 1M+ LOC, et **publiés**. Pas "ça scale, faites-nous
confiance". Les chiffres tournent en CI, sont attachés aux releases,
et régressent visiblement quand on les casse.

### API freeze contractuel

Tools MCP, méthodes LSP custom, types IR exportés par `standardoc-ir`,
schema SQLite — l'ensemble devient un contrat public. Tout changement
breaking ultérieur passe par un bump `protocol_version` et une période
de coexistence côté daemon. Plus de droits unilatéraux de changer la
sémantique d'un tool ou d'un edge sans accord explicite.

---

## La logique d'ensemble : primitives d'abord, conventions ensuite

Un pattern récurrent émerge en regardant ce qui est shippé vs ce qui
reste à faire : **on a posé les primitives stables avant d'en remplir
tous les consumers**. La table enrichments existait avant que le
premier parser de doc-comments ne l'utilise. Le tag `BridgeKind`
existait dans l'IR avant qu'un détecteur Tauri ne le produise. Les
edges typés (`CALLS`, `IMPORTS`, `EXTENDS`, …) étaient là day-1, on
enrichit progressivement leurs attributs.

C'est l'inverse du marketing software classique ("on annonce, on
développe"). Ici : **on construit l'invariant d'abord, on remplit les
conventions ensuite, on fige le contrat au moment où des tiers peuvent
effectivement en dépendre.**

C'est cohérent avec les 5 principes system-thinking de
[`philosophy.md`](philosophy.md) — particulièrement le 1er
("qu'est-ce qui reste stable malgré les changements ?") et le 2e
("quels choix deviennent irréversibles ?"). Une primitive bien posée
encaisse 10 itérations de conventions sans casser. Une convention
figée trop tôt calcifie l'outil.

---

## L'arc beta.1 → 1.0 vu en entier

- **beta.1** = la grammaire (IR + 2 surfaces day-1)
- **beta.2** = la maturité (toolkit MCP, archi multi-frontend, discipline
  encodée, primitives storage + IR posées sans bruit)
- **beta.3** = pluralisation des usages du graphe (doc rendue +
  navigation visuelle + CLI autonome + compréhension cross-session)
- **beta.4 / 5 / …** = ce qui émergera en dogfood entre beta.3 et
  1.0 (impossible à lister à l'avance — beta.2 elle-même n'avait
  pas été planifiée dans sa forme actuelle)
- **1.0** = le contrat (conventions remplies, sémantique figée,
  benchmarks publiés, droit de breaking change unilatéral éteint)

À 1.0, Standardoc cesse d'être *un outil qu'on raffine en interne*
pour devenir *une infrastructure dont des tiers peuvent dépendre en
confiance*. C'est le pivot où on perd certains droits (changement
sémantique unilatéral) en échange d'autres (que des projets reposent
dessus en sachant qu'on tiendra parole).

---

## Ce qu'on ne fait PAS dans cette phase

Cadrage négatif important :

- **Pas de SaaS, pas de cloud sync.** Le core open-source reste
  local-first sans condition. Le pivot SaaS reste hors-table.
- **Pas de plug-and-play multi-langues via Lua/UST.** Cette vision
  appartient au post-1.0 (cf. [vision long terme](vision-long-terme.md)).
  On stabilise d'abord les langues actuelles avant d'ouvrir aux
  plugins.
- **Pas de renderers Vue / Svelte avant beta.3 fini.** Le
  framework-agnostic `@standardoc/core` rendra leur ajout mécanique
  post-beta.3, mais on ne disperse pas l'effort avant que React soit
  solide.
- **Pas d'extension de la surface MCP sans nécessité dogfood claire.**
  beta.2 a posé 16 tools sur des besoins observés. 1.0 fige cette
  surface ; on n'en ajoute pas sans un trou dogfood manifeste.

---

## Les invariants protégés à 1.0

Les 4 invariants posés en [vision court terme](vision-court-terme.md)
restent intacts :

- **L'IR reste stable.** Pas de retrait, pas de changement sémantique
  sans bump `protocol_version`.
- **Le graphe reste local.** Pas de cloud sync, pas d'auth, pas de
  telemetry phone-home.
- **Le license timer reste armé.** FSL-1.1-MIT → MIT pur 2 ans après
  chaque release. Première release : 26 avril 2028.
- **Le format SQLite reste ouvert.** Schema versionné, lisible avec
  un sqlite3 standard.

À 1.0, un cinquième invariant apparaît :

- **L'API publique est gelée.** MCP tool signatures, LSP custom
  methods, types IR exportés, schema SQLite. Bump `protocol_version`
  + coexistence obligatoire pour tout breaking change.

---

## Pour aller plus loin

- **[← Vision court terme](vision-court-terme.md)** — beta.2 et la
  phase de stabilisation
- **[Vision long terme →](vision-long-terme.md)** — UST + Lua plugin
  layer post-1.0, écosystème, plateforme
- **[Remarques](remarques.md)** — observations dogfood, décisions
  lockées, apprentissages
- **[Retours de tests](retours-tests.md)** — ce qu'on a testé, ce
  qu'on a abandonné, les mesures
- **[TODO-LIST](../TODO-LIST.md)** — checkboxes exhaustives par
  milestone
