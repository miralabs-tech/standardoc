# Remarques

[English](../../en/storytelling/notes.md) · 📖 Français

[← Philosophie](philosophy.md) · [Vision court terme](vision-court-terme.md) · [Vision moyen terme](vision-moyen-terme.md) · [Vision long terme](vision-long-terme.md) · [Retours de tests](retours-tests.md)

> Ce document rassemble les **synthèses transversales** des
> sessions de développement : décisions structurantes qui sont
> revenues plusieurs fois, apprentissages dogfood, patterns
> d'usage agent, anti-patterns évités, et la posture qui découle
> de la réalité matérielle du projet.
>
> Ce n'est pas un inventaire chronologique des memos — c'est ce
> qui en émerge en relief.

---

## Décisions structurantes

Choix qui sont revenus dans plusieurs memos et qui structurent le
projet de bout en bout :

- **IR canonique + AST direct comme moat.** Le projet rejette
  explicitement le LSP-per-IDE et le tree-sitter de surface
  utilisés ailleurs : « AST profond `syn` / `swc` (signatures,
  types, génériques, traits) vs tree-sitter surface (regex-like,
  fonctions + classes + calls seulement) ». C'est le différentiel
  central face aux outils existants — pas une décision d'opinion,
  une décision d'invariant.

- **License-as-moat (FSL-1.1-MIT → MIT 2028).** Verrou central
  relocké à plusieurs reprises. MIT seul a été écarté (pas de
  protection court-terme), AGPL aussi (insuffisant contre des
  concurrents closed-source non-SaaS qui forkeraient sans
  publier). FSL avec **conversion automatique en MIT 2 ans plus
  tard, par release**, est le seul mécanisme qui combine
  protection initiale et engagement irréversible d'ouverture.

- **Sessions DB orthogonale au graphe, RAG linkée par FQDN.**
  Précision architecturale qu'on a dû relocker plusieurs fois :
  « ne pas dire 'tous lisent le même graphe' (faux) ». Le LSP
  et le MCP partagent `.standardoc/index.db` (graphe) et
  `.standardoc/rag.db` (chunks prose linkés par FQDN). La
  sessions DB (`.standardoc-sessions/sessions.db`) est
  **séparée** — c'est de la mémoire d'agent, pas du graphe
  dérivé du code.

- **`.md` = transport canonique, `.db` = cache local
  reproductible.** Pattern récurrent : les memos sessions ont
  une forme persistante en SQLite (consultable rapidement,
  indexée), mais leur format canonique d'échange et de
  versioning reste Markdown avec frontmatter (`status`,
  `supersedes`, `created_at`). On peut perdre la DB sans perdre
  les memos — juste re-builder.

- **Primitives d'abord, conventions ensuite.** Pattern dogfood
  observé sur plusieurs cycles : on pose la primitive stable
  (table `enrichments`, tag `BridgeKind`, edges typés) **avant**
  de remplir tous ses consumers. Rationale : une primitive bien
  posée encaisse 10 itérations de conventions sans casser ; une
  convention figée trop tôt calcifie l'outil (cf.
  [vision moyen terme](vision-moyen-terme.md)).

---

## Apprentissages dogfood

Ce qui a marché de manière inattendue, ce qui a échoué, les
pivots de scope.

- **Pivot beta.2 → beta.3 sur le doc rendering.** Plan original :
  beta.2 = doc rendering layer + CLI self-managed. Réalité : le
  dogfood a fait remonter des besoins plus urgents (surface MCP
  trop pauvre, sessions qui s'évaporent entre chats, daemon
  fragile sous orchestration). Le doc rendering a glissé en
  beta.3 — **pas par procrastination, par priorité différente
  révélée par l'usage réel**.

- **Surprise infra : le transport HTTP/SSE.** En beta.1, le MCP
  stdio créait un child standardoc par fenêtre de chat : « 5
  chats = 5 children, RAM hog ». Le passage à HTTP/SSE en
  beta.2 a collapsé ça : « 2 processes par window VSCode,
  indépendant du nombre de chats ». Effet secondaire qui
  n'avait pas été anticipé : le parent-death-watch via stdin
  pipe a éliminé tous les orphelins (kill VSCode, BSOD, OOM —
  le child se kill tout seul).

- **Échecs et coupes assumées.** Plusieurs pistes ont été tuées
  sans regret au fil du projet — v0 DSL templating
  `{{ @doc.X }}`, commande `materialize`, binaire séparé
  `standardoc-server`, fichier de config `.standardoc.json`,
  publish initial sur crates.io (détails dans
  [retours de tests → ce qu'on a abandonné](retours-tests.md)).
  Chaque coupe a libéré du scope pour ce qui est devenu beta.2.

- **Cap RAG corpus, pas filtre.** Track dogfood : on a tuned les
  filtres RAG (threshold 0.55, stop-list 23 mots, confidence
  floor) et constaté que « le signal légitime est limité par
  le corpus » — pas un problème de filtre, un problème de masse
  prose dans un projet encore en construction. À mesure que la
  doc grossit, le RAG devient plus utile.

---

## Patterns d'usage agent au-delà de la calibration

Compléments à [retours de tests](retours-tests.md) — patterns
spécifiquement observés sur la consommation agent du graphe :

- **MCP-first strict avec protocole 3-phase** (`find` →
  `context` → `body`). AST graph navigation **avant** Grep,
  toujours. Grep réservé aux « true string-literal targets with
  no graph anchor » (commentaires, configs hors-code, build
  files).

- **`session_save` over copy-paste handoff.** Pivot observé en
  session interne : le bootstrap d'une nouvelle session par
  copie-collage du contexte précédent finit par être supplanté
  par `session_save(slug, body_md)` + `session_get()` au début
  du chat suivant. Le tool MCP **supersede** la convention
  globale de "passer le contexte" — c'est l'infrastructure qui
  porte la mémoire, pas l'opérateur.

- **RAG cross-session via FQDN.** Les `chunk_refs` injectés dans
  `get_context` font qu'une session retrouve la prose pertinente
  d'une session précédente — par exemple une section de
  documentation ancrée sur un FQDN précis. Confidence threshold
  privé de `0.55`, stop-list de 23 mots (`data`, `default`,
  `done`, `file`, `find`, …) pour éviter les ancres triviales.

- **Probe discipline.** « Une seule probe ciblée, accepter `[]`
  comme signal "n'existe pas" ». Anti-pattern explicitement
  refusé : 4 variantes `*foo*` / `*bar_foo*` séquentielles
  quand la première a déjà retourné un `did_you_mean` suggérant
  la bonne piste.

- **POUR / CONTRE / VOTE sur tout design avec plus d'une option
  viable.** Quand l'opérateur peut trancher, il tranche ; quand
  seul l'AI voit la nuance technique, l'AI tranche et documente.
  Cette convention évite les bikesheds sur des choix sans
  incidence réelle.

---

## Anti-patterns évités côté travail

Différent des *"ce qu'on ne fait PAS"* côté produit (déjà dans
les visions) — ici, les **anti-patterns de travail quotidien**
qu'on refuse :

- **Dummy items pour héberger des annotations.** Quand un langage
  manque d'une primitive (par exemple Lua sans système de types
  natif), la tentation est d'introduire des stub fonctions /
  valeurs juste pour leur attacher des annotations. Refus net :
  « polluer la source avec du code mort qui n'a aucun sens ».
  Si l'annotation ne peut pas être attachée à un symbole vivant,
  elle vit ailleurs (enrichments table, sidecar).

- **Time estimates en jours / semaines.** Constat assumé : « mes
  estimations sont systématiquement surévaluées d'un ordre de
  magnitude ». Conséquence : on ne donne pas de calendrier ferme
  sur les chantiers ; les labels sont structurels (beta.2 /
  beta.3 / 1.0) ou conditionnels (« dogfood-driven, peut
  glisser »), jamais datés à la semaine.

- **BUSINESS-MODEL out of marketing before 1.0.** Le différenciant
  public de Standardoc reste DX / perf / AST direct. Les
  discussions internes sur le pricing futur, le sponsoring, le
  SaaS pivot hypothétique post-1.0 — tout ça reste **hors du
  discours public** jusqu'à 1.0. Pas de teasing, pas
  d'ambiguïté.

- **Reposer une question déjà tranchée dans une spec locked.**
  Quand une décision a été lockée (`SessionKind::Lock`), elle ne
  se rediscute pas à chaque nouvelle session — sauf si un fait
  nouveau émerge qui invalide la base de la décision. Le memo
  reste superseded explicitement (via le champ `supersedes`),
  pas remplacé en silence.

---

## La réalité matérielle du projet et la posture qui en découle

### Standardoc n'est pas un projet de la semaine dernière

Le repo `miralabs-tech/standardoc` est récent (post-refonte
communication, post-rebranding), mais **j'y pense depuis
plusieurs années** et **je l'ai prototypé il y a ~6 mois** sur
un compte personnel :
[SUP2Ak/standardoc-cli](https://github.com/SUP2Ak/standardoc-cli).
La version actuelle a beaucoup évolué depuis ce prototype (en
vrai, rien à voir techniquement), mais la trajectoire de pensée
est bien antérieure au repo officiel.

Ce que Standardoc représente est issu de **plus d'une décennie
de pratique dev** où j'ai vu, au fil des projets, certains
problèmes structurels du *dev ecosystem* devenir visibles —
chaque outil qui re-parse le code, drift entre les indexes
parallèles, dette cognitive cross-langage, dépendance aux SaaS
pour comprendre son propre code. Standardoc est ma tentative de
résolution de ces problèmes mûris dans la tête sur 10-15 ans.

Ce qui rend Standardoc **shippable maintenant** plutôt qu'il y
a 2 ans, c'est la conjonction de deux choses :

1. **Les technos AI sont passées d'expérimental à utilisable.**
   Les outils MCP, les fenêtres 1M tokens, les hooks
   `PreToolUse` / `SessionStart`, l'écosystème agent Claude
   Code — tout ça est devenu fiable courant 2026, après une
   phase mi-2025 prometteuse mais brouillonne.
2. **L'archi solo amplifiée** (cf. section suivante) me permet
   d'avancer au rythme nécessaire pour stabiliser une API
   publique, livrer une surface MCP complète, et figer un IR
   canonique en quelques mois — pas en plusieurs années.

### Standardoc dans la suite StandarX

**StandarX**, c'est l'ensemble des projets perso que je porte en
OSS, et qui visent tous à **standardiser des problèmes
structurels du dev ecosystem**. Standardoc est le premier à
sortir publiquement — d'autres mijotent dans le tiroir privé
depuis des années.

**L'entité légale derrière**, c'est mon auto-entreprise
**[miralabs.tech](https://miralabs.tech)** — c'est elle qui possède
l'organisation GitHub `miralabs-tech` et qui héberge Standardoc.
La raison du statut juridique est banale : en droit français,
pour travailler indépendamment (sponsoring, contrats freelance,
collaborations), il faut être *quelque chose* aux yeux de la
loi, pas *quelqu'un*. **Je suis ouvert à des opportunités indé
via miralabs.tech — ou à un recrutement classique tout court**,
en parallèle du reste.

Standardoc est **la pièce la plus importante de StandarX pour
moi** parce que c'est celle qui peut **faire concrétiser des
idées qui mijotent depuis des années** sur des problèmes
structurels du dev ecosystem (compréhension de code comme
système, pas comme suite de greps).

J'ai d'autres projets perso qui traînent en privé depuis
plusieurs années — jamais sortis parce que jamais prêts pour la
prod, et surtout parce qu'ils ne rapportent rien et n'ont pas le
même alignement avec un besoin écosystème large. Standardoc
passe en premier parce qu'il a la chance de matcher un moment où
le problème est devenu visible **et** où les outils pour le
résoudre existent.

Et tout cela **OSS, local, sans tracking utilisateur** — parce
que le problème est réel pour les dev indé, ceux qui bossent
sur des projets OSS, ou les devs solo sur grosse codebase :
**écrire du code en plus de gérer l'archi, le design, la stack,
la dette long-terme, la doc, le CI, le packaging** est un
gouffre mental qui s'aggrave avec le temps. **Il n'y a pas que
la RAM qui peut overflow ; le cerveau aussi**, surtout en solo
sur des grosses codebases.

### Le rôle de l'AI : archi solo amplifiée, pas "vibe coding"

L'air des AI tools depuis mi-2025 (mature courant 2026) a
changé mon mode de travail solo. **Ce n'est pas du "vibe
coding"** — je ne lance pas un prompt vague à un agent et je
regarde ce qui sort. C'est de l'**architecture solo amplifiée** :

- **Je fixe le design**, les **snippets canoniques**, les
  **techniques d'approche au sens algorithmique** — pas vague
  (« tu fais une API REST avec des endpoints »), précis (« tu
  fais une FTS5 query avec tokenization snake / camel et
  fallback strsim seuil 0.6 »).
- L'agent **exécute sous discipline** — MCP-first guardrail,
  3-phase `find → context → body`, `session_save` / `get` (cf.
  [retours de tests](retours-tests.md)).
- **Je reste responsable** de la **review**, de
  l'**architecture d'ensemble**, du **contrat IR**. L'agent
  amplifie ; il ne décide pas.

Sans cette amplification, **je ne pourrais pas avancer à ce
rythme tout seul** (~90 commits sur le cycle beta.1 → beta.2,
douzaine de chantiers techniques majeurs). Avec elle, des
idées qui mijotaient en privé deviennent shippables. Ce n'est
pas magique — c'est de l'archi consciente couplée à une
exécution disciplinée.

### Règles de contribution pre-1.0

- **Pas de PR tiers avant le freeze 1.0.** La surface API doit
  se figer proprement, et accepter des PRs externes avant la
  stabilisation introduirait du noise sur des choix que je
  dois garder contrôlés pour porter le contrat.
- **Issues, feedback, idées techniques ou globales : tout est
  bienvenu**, via GitHub Issues / Discussions. C'est même là
  où j'apprends le plus rapidement ce qui manque.
- **Post-1.0, le modèle s'ouvre** — c'est en partie pour ça
  que le plug-in layer UST + Lua est central dans
  [vision long terme](vision-long-terme.md) : il permet
  d'absorber des contributions communautaires sans toucher au
  core figé.

### OpenCollective n'est pas décoratif

[StandarX sur OpenCollective](https://opencollective.com/standarx)
n'est pas un bouton placé pour faire joli sur le README.

**Je porte Standardoc seul, en cumulant deux emplois à côté.**
Le projet n'est pas une source de revenu — je le maintiens
parce qu'il est utile, pas parce qu'il finance. Si tu veux
**voir 1.0 arriver vite**, le soutien compte concrètement : il
me permet de réduire le temps consacré aux jobs de subsistance
et d'avancer plus vite sur Standardoc (et sur les autres
projets StandarX qui suivent).

Ce n'est pas du chantage. **C'est un fait matériel** : sans
soutien, le rythme de shipping dépend de mon temps libre
résiduel, et j'ai déjà mes semaines occupées ailleurs. Avec
soutien, le rythme s'aligne sur la priorité que la communauté
accorde au projet.

**Note honnête : je ne fais pas de promotion en général.**
Pour moi le code parle plus que le marketing, et c'est la
première fois que je fais un vrai effort de communication
autour d'un de mes projets — précisément parce que Standardoc
est celui qui peut faire concrétiser ce qui mijotait depuis
longtemps. Si l'OpenCollective ne décolle pas, je continuerai
à shipper sur mon temps résiduel ; ce sera juste plus lent.

→ Détails dans [SUPPORT.md](../SUPPORT.md).

### Posture communication

Trois principes calibrés au fil du dogfood :

- **Ne pas être cheerleader, ne pas être doomer.** Le projet
  n'a pas besoin de hype, ni de catastrophisme. La trajectoire
  est posée, la qualité du code parle, le narratif suit le
  réel.
- **Stats GitHub asymétriques (clones >> visits) ≠ signal.**
  Le ratio se lit comme **bruit pré-beta.2 + pré-refonte com**,
  pas comme métrique d'échec. La cible Standardoc (devs qui
  maîtrisent leur tooling) clone direct via `gh repo clone`
  sans visiter la page GitHub — la vraie audience ne laisse
  pas de visit trace.
- **Honnêteté du positionnement maintenue.** La ligne « ripgrep
  & LSP suffisent là » pour les petites SPA est gardée
  volontairement dans le README. Refus de prétendre
  l'universalité — Standardoc est fort sur les codebases
  grosses et complexes (compilateurs, langages, moteurs,
  monorepos lourds), **overkill ailleurs**, et c'est OK.

---

## Pour aller plus loin

- **[← Philosophie](philosophy.md)** — les 5 principes
  system-thinking et l'éthique de construction
- **[Vision court terme](vision-court-terme.md)** — beta.2 et
  la phase de stabilisation
- **[Vision moyen terme](vision-moyen-terme.md)** — beta.3 et
  1.0
- **[Vision long terme](vision-long-terme.md)** — UST + Lua
  plug-in layer post-1.0
- **[Retours de tests](retours-tests.md)** — calibration agent,
  ce qu'on a abandonné, observations dogfood
- **[TODO-LIST](../TODO-LIST.md)** — checkboxes exhaustives par
  milestone
