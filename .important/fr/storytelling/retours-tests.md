# Retours de tests

[English](../../en/storytelling/test-feedback.md) · 📖 Français

[← Philosophie](philosophy.md) · [Vision court terme](vision-court-terme.md) · [Vision moyen terme](vision-moyen-terme.md) · [Vision long terme](vision-long-terme.md) · [Remarques](remarques.md)

> Ce document rassemble les **observations honnêtes** issues du dogfood :
> ce qu'on a testé, ce qu'on a abandonné, et les surprises qu'on a vues
> en utilisant Standardoc sur des cas réels.

---

## Calibration et matching d'agent

Pendant la phase **v0.1.0-rc** (prototype) puis sur tout le cycle
beta.1 → beta.2, on a testé Standardoc avec plusieurs agents IA
disponibles à l'époque. **Les comportements observés sont très
inégaux selon l'agent.**

### Les agents qui ne matchent pas

Trois patterns récurrents observés sur des agents qui ne s'alignent
pas naturellement avec l'architecture de Standardoc :

- **Shortcut vers `grep + read`** — l'agent connaît MCP, mais dès que
  la tâche se complique, il bascule vers `Bash` / `Grep` / `Read` par
  réflexe. Sans hook PreToolUse pour le bloquer, le protocole
  MCP-first reste une bonne intention non-observable.
- **Ignore le `routing_hint`** — quand un agent appelle
  `get_context(fqdn, depth=2)` sans avoir fait `depth=1` récemment
  (dans les 5 dernières minutes) sur le même FQDN, la réponse
  contient un `routing_hint` correctif qui rappelle de mapper
  d'abord en `depth=1` avant de driller en `depth=2`. C'est le
  seul cas où le hint apparaît — c'est un signal de correction
  ponctuel, pas un système de guidage général. Certains agents
  l'ignorent et continuent en `depth=2` d'office, résultat :
  drill complet là où un map peu coûteux aurait suffi à éliminer
  80% des voisins.
- **Exploration non-structurée** — l'agent consomme 100% du budget
  en recherches itératives sans capitaliser. Pas de réutilisation
  des symboles déjà visités, pas de `check_stale` pour réutiliser
  un cache, pas de `session_save` pour persister les décisions
  lockées.

### L'agent qui matche le mieux

**[Claude Code](https://www.anthropic.com/claude-code) en mode Opus**,
fenêtre 1M tokens, avec un tier d'effort qui varie selon la tâche
(medium / high / extra-high — jamais figé). C'est sur cet agent que
la calibration de Standardoc est faite :

- Hooks PreToolUse / SessionStart qui forcent le MCP-first guardrail
- `routing_hint` du `get_context` respecté en pratique (Opus mappe
  en `depth=1` avant de driller en `depth=2` au lieu de subir le
  correctif)
- Knobs `signature_only` / `strip_attrs` de `get_body` utilisés
  spontanément quand le budget de contexte se tend
- Convention `session_save` / `session_get` adoptée naturellement
  entre les chats — les décisions lockées d'une session survivent

Les autres agents fonctionnent, mais sans la même fluidité, et avec
des écarts de consommation qu'on n'observe pas avec Opus.

### Quelques estimations dogfood

Standardoc indexant Standardoc lui-même, sessions Claude Code Opus
(alternance medium / high / extra-high selon la nature de la tâche,
fenêtre 1M tokens) :

- **Au début du projet**, on avait du mal à tenir les **5h de
  quota** Claude Code avant le throttle. La majorité du budget
  partait en exploration redondante (grep, re-lecture de fichiers,
  re-découverte de structures déjà visitées la session précédente).
- **Aujourd'hui**, on tient en moyenne **2 sessions** sur cette
  fenêtre 5h. Ça varie beaucoup avec la nature de la tâche
  (génération de code dense, rédaction narrative, debugging,
  refactor) et avec le tier d'effort choisi sur la session.
- **Sur les sessions 1M tokens**, on reboot une nouvelle session
  **entre ~200k et ~600k tokens utilisés dans le contexte** —
  selon la nature de la tâche. Pas parce qu'on a atteint la
  limite, mais parce qu'au-delà la qualité de raisonnement de
  l'agent se dégrade.

Ces chiffres sont des **estimations dogfood**, pas des benchmarks
contrôlés. Ils reflètent Standardoc + Claude Code Opus + un dev qui
maîtrise son code et applique la discipline 3-phase.

### Ce qui a vraiment impressionné en dogfood

Au-delà des chiffres, l'observation qualitative la plus marquante :
**le code généré par l'agent break rarement, et respecte
spontanément l'architecture et le style du projet**. Les conventions
tacites — nommage, granularité des modules, style d'erreur,
séparation des responsabilités IR / storage / query / providers —
sont reprises par l'agent sans qu'on ait à les lui ré-expliquer à
chaque tâche.

C'est exactement ce que doit produire un graphe partagé couplé à
une discipline encodée : **un agent qui se conforme au projet, pas
qui l'écrase.**

**Mais ça ne dispense pas de la review.** Un projet maintenable
exige que le dev **connaisse son code et le comprenne** — pas
seulement qu'un agent puisse en produire qui passe les tests. La
review reste obligatoire à chaque PR, à chaque chantier. Standardoc
rend la review *plus rapide et plus fiable* (l'agent suit les
conventions, donc moins de surprises côté reviewer) ; il ne la
*remplace pas*.

### Disclaimer : calibration tripartite

**On ne garantit pas que ton agent va matcher de la même façon.**
Standardoc fournit l'infrastructure (graphe + MCP + discipline) ;
si ton agent ne respecte pas le protocole MCP-first, ignore les
correctifs du `routing_hint` quand il viole le pacing `depth=1 →
depth=2` de `get_context`, ou n'utilise pas les sessions DB, tu
n'auras qu'une partie des gains. **La calibration est tripartite** :
infrastructure + agent + opérateur.

#### Ce que l'infrastructure fournit déjà

- **Sessions DB avec 4 kinds discriminés** (`SessionKind` enum) —
  `Session` (handoffs entre chats, par défaut), `Feedback` (règles
  comportementales que l'agent doit suivre), `Profile` (facts
  utilisateur stables), `Lock` (décisions lockées — l'**équivalent
  ADR**, Architecture Decision Record, au format memo Standardoc).
  Le schema SQL distingue ces catégories ; les MCP tools
  (`session_save`, `session_get`, `session_list`,
  `session_sync_in`, `session_sync_out`) les exploitent. Le
  `sync_in/out` permet le bridge avec un dossier `.md` externe
  pour cross-pollination avec la memory native d'autres agents.
- **Skill template auto-généré** (`SKILL.md` de ~480 lignes écrit
  dans `.claude/skills/standardoc/`) — enseigne à l'agent : tool
  fallback hierarchy (Standardoc → LSP → Read / Grep), 3-phase
  protocol (Explore → Cibler → Drill), 9 workflows recommandés
  dont *"Resume / save a session handoff"* et *"Pull in prose
  alongside the code graph"*, edge kinds, language coverage.
  L'agent qui lit ce skill au boot n'a pas à deviner la mécanique.
- **RAG sur la prose adjacente** — `docs/`, `notes/`, `*.md` au
  root sont chunkés et accessibles via `fetch_chunks(uri)` ou via
  les `chunk_refs` de `get_context(fqdn, depth, query?)`. La prose
  narrative vit dans le même daemon que le graphe et est linkée
  par FQDN.
- **Hooks MCP-first** (côté Claude Code) — PreToolUse bloque
  `Bash` / `Read` / `Grep` / `Glob` tant qu'aucun tool Standardoc
  n'a été appelé dans la session ; SessionStart wipe le sentinel
  à chaque nouveau chat. Discipline observable, pas bonne
  intention.

#### Ce qui reste à charge de l'opérateur

Trois responsabilités humaines non-déléguables :

1. **Coupler son agent à Standardoc.** Installer l'extension
   VSCode (qui génère le skill et écrit `.mcp.json` automatiquement
   via l'init opt-in flow) ou wirer `.mcp.json` à la main pour un
   autre client MCP. Si l'agent ne voit pas Standardoc, il ne
   l'utilise pas.
2. **Architecturer son projet avec un minimum de cohérence.**
   Standardoc indexe ce qui existe — si la codebase est un
   spaghetti sans modules clairs, le graphe reflète le spaghetti.
   L'outil amplifie ce que l'opérateur cultive ; il ne le remplace
   pas.
3. **Indiquer à l'agent quand et comment utiliser les outils.**
   Le skill template enseigne la mécanique générale ; mais pour
   une tâche donnée, c'est à l'opérateur de dire *"commence par
   `find_symbol`"*, *"vérifie d'abord qui appelle X"*, *"ne grep
   pas ici"* — surtout en début de calibration, avant que l'agent
   ait intégré les habitudes de pacing par lui-même.

Si ton agent par défaut est mauvais sur ce protocole, l'option qui
marche en pratique est : (a) lui forcer le MCP-first via les hooks
Claude Code (guardrail), (b) lui donner les sessions persistantes,
(c) le laisser absorber sur 2-3 tâches les correctifs `routing_hint`
quand il saute le pacing `depth=1 → depth=2` de `get_context`.

---

## Ce qu'on a abandonné

Au fil des cycles, certaines pistes initiales ont été tuées ou
repoussées. Chacune était cohérente sur le papier ; chacune s'est
révélée moins prioritaire que ce qu'on a fait à la place.

- **DSL templating `{{ @doc.X }}` (v0)** — concept initial pour
  injecter de la doc générée dans des fichiers Markdown via des
  expressions custom. Tué en beta.1. Le DSL devenait une seconde
  source à maintenir, mal outillée. Remplacé par la layer
  `@standardoc/core` + `@standardoc/react` (beta.3) qui consomme
  directement le graphe.
- **Commande `materialize`** — devait écrire les enrichissements
  virtuels dans la source. Puntée. **Candidate post-1.0** dans le
  cadre plus large de l'**import/export des commentaires via
  pointeurs FQDN-ancrés safe-edit** — l'objectif à terme étant de
  pouvoir maintenir une codebase épurée (sans pavés de commentaires)
  tout en gardant la doc dans le graphe, avec la capacité de
  réinjecter localement sans risque de désynchro entre la doc et
  la source. On préfère ne pas muter le code source tant que la
  sémantique des enrichissements n'est pas figée à 1.0.
- **Binaire séparé `standardoc-server`** — devait isoler le daemon
  des sub-commands CLI. Consolidé dans un seul `standardoc` avec
  sub-commands (`lsp`, `mcp`, `index`, `query`, …). Moins de
  binaires à distribuer, moins de combinatoire de versions, moins
  de surface d'erreur de déploiement.
- **Fichier de config `.standardoc.json`** — proposait une config
  centralisée projet. Remplacé par `.stdignore` (gitignore-syntax,
  contribution langage VSCode) pour l'exclusion + table SQLite
  `schema_meta` pour l'état runtime. Plus simple, plus dérivé,
  moins d'API stable à maintenir.
- **Renommage `.stdocignore` → `.stdignore`** — choix purement
  cosmétique mais figé tôt pour éviter le rename downstream.
- **`cargo install standardoc-cli` comme unique canal** — abandonné
  comme distribution principale. Trop lent pour les CI, exige une
  toolchain Rust. Remplacé par **pre-built cross-platform binaries
  via GitHub Releases** (avec `version.json` manifest pour les
  agents programmatiques). `cargo install --git` reste disponible
  pour les builds source, mais n'est plus la voie recommandée.
- **Providers Lua / Python / tree-sitter en beta.1** — scope
  original trop large. Lua a été shippé en beta.2 (provider natif
  via `full_moon`). Python + tree-sitter sont repoussés post-1.0
  (cf. [vision long terme](vision-long-terme.md) pour la stratégie
  UST + Lua plugin layer qui rend ces ajouts community-driven).
- **Publish initial sur crates.io** — droppé pour beta.1. Trop tôt
  pour figer les noms de crates publics ; pas de demande tierce
  identifiée ; les pre-built binaires GitHub Releases couvrent 95%
  des cas d'usage. Pas d'engagement ferme sur quand (ou si) on
  repassera sur cette voie.

---

## Observation dogfood : la doc coûte plus cher que le code

Sur les 2–3 derniers jours, **générer les `.md` de `.important/`**
(les docs storytelling, README, FAQ, comparaisons, et compagnie) a
consommé **plus de tokens Claude Code Opus que toute la phase
shipping beta.1 → beta.2** — **~90 commits, une douzaine de
chantiers techniques majeurs** : hardening daemon, expansion MCP
de 2 à 16 tools, RAG layer, sessions DB, MCP-first guardrail,
providers Lua/Vue/Svelte, archi HTTP/SSE, externals resolvers,
usage stats, init opt-in flow ext VSCode, CI hardening, etc.
C'est un paradoxe qui mérite d'être regardé en face : **rédiger
l'explication d'un projet coûte plus cher que l'avancer.**

Note pratique côté tier d'effort : on a fait cette phase avec
Opus pour la tester, mais en vrai **Sonnet est mieux placé pour
ce genre de rédaction narrative** — moins cher, suffisamment
précis pour ces tâches, et l'écart de coût se voit immédiatement
sur le quota. Opus reste indiqué pour le code dense, le debugging
avec contexte fort, le refactor cross-modules ; pas pour écrire
des docs.

### Diagnostic

La prose RAG est **sous-exploitée cross-session**. Le graphe est
partagé (chaque nouvelle session voit le même état). Le RAG retrieve
fonctionne intra-session (les chunks pertinents sont remontés via
`fetch_chunks`). Mais la **synthèse de compréhension projet** — les
objectifs court/moyen/long terme, la posture system-thinking, les
décisions structurantes lockées, l'intention narrative — n'est pas
capitalisée d'une session à l'autre.

Chaque nouvelle session re-découvre cette synthèse par fetches
dispersés (RAG + memos + relecture des docs) au lieu de récupérer
une compréhension consolidée. Pour du code, ce n'est pas grave : le
graphe est suffisant. **Pour de la rédaction narrative, c'est cher.**
L'agent doit reconstruire le contexte narratif à chaque session
avant de pouvoir écrire dans le ton du projet.

### Piste solution

Étendre `sessions.db` au-delà des memos d'agent (déjà supportés via
`session_save` / `session_get`) pour persister une **compréhension
globale projet cross-session** — synthèse vivante des objectifs, de
la posture, des décisions lockées, du ton narratif. Pas un dump
bullets : une **structure exploitable** que l'agent peut consulter
en un tool call au lieu de dix fetches dispersés.

### Roadmap candidate

Candidate pour **beta.3** (en plus du rendering layer et de la
navigation visuelle), ou pour **beta.4** selon ce qui émerge en
dogfood pendant les 2 semaines de tests sur d'autres projets. Si
d'autres trous se révèlent plus prioritaires, le rendering peut
glisser en beta.4 et cette persistance cross-session devient l'axe
central de beta.3.

### Garde-fou non négociable

**La vérité reste le code source.** Toute synthèse persistée doit
être **ré-validable par re-check du graphe** à la session suivante.
Si la synthèse cross-session contredit la réalité du code, c'est la
synthèse qui se corrige, pas l'inverse. Aucune compréhension
consolidée ne devient une source de vérité indépendante — elle
reste une projection dérivée, invalidable à tout instant.

Observation récente — les 2 semaines de tests sur d'autres projets
affineront le diagnostic et la priorisation.

---

## Pour aller plus loin

- **[← Philosophie](philosophy.md)** — les 5 principes
  system-thinking et l'éthique de construction
- **[Vision court terme](vision-court-terme.md)** — beta.2 et la
  phase de stabilisation
- **[Vision moyen terme](vision-moyen-terme.md)** — beta.3 et 1.0
- **[Vision long terme](vision-long-terme.md)** — UST + Lua plugin
  layer post-1.0
- **[Remarques](remarques.md)** — observations dogfood, décisions
  lockées, apprentissages
- **[TODO-LIST](../TODO-LIST.md)** — checkboxes exhaustives par
  milestone
