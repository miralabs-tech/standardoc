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

- **Le graphe n'est pas la mémoire d'agent.** Le graphe de code
  (`.standardoc/`) a toujours été tenu séparé de la mémoire de session
  de l'agent ; en beta.3 cette mémoire — et la couche RAG de prose qui
  vivait à côté — est sortie du core entièrement. Le core dérive du
  code, rien d'autre.

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

---

## Patterns d'usage agent au-delà de la calibration

Compléments à [retours de tests](retours-tests.md) — patterns
spécifiquement observés sur la consommation agent du graphe :

- **MCP-first strict avec protocole 3-phase** (`find` →
  `context` → `body`). AST graph navigation **avant** Grep,
  toujours. Grep réservé aux « true string-literal targets with
  no graph anchor » (commentaires, configs hors-code, build
  files).

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

- **Reposer une question déjà tranchée.** Une décision lockée ne se
  rediscute pas à chaque nouvelle session — sauf si un fait nouveau
  invalide sa base. Elle reste explicitement superseded, pas remplacée
  en silence.

---

## Contexte

Standardoc est open-source, local, sans tracking utilisateur, maintenu
solo sous [miralabs.tech](https://miralabs.tech) (l'entité derrière l'org
`miralabs-tech`). Il fait partie de **StandarX**, un ensemble d'outils OSS
qui standardisent des problèmes structurels du dev ecosystem ; Standardoc
est le premier à sortir publiquement.

Il sort maintenant parce que deux choses se sont alignées : les agents IA
sont devenus assez fiables pour amplifier un mainteneur solo (MCP, grandes
fenêtres de contexte, hooks), et le problème de fond — chaque outil qui
re-parse le code, le drift entre indexes, la dette cognitive cross-langage
— est devenu assez pénible pour mériter d'être résolu proprement.

### Contribution & soutien (pre-1.0)

- **Pas de PR tiers avant le freeze 1.0** — la surface API doit d'abord se
  stabiliser proprement. **Issues, feedback et idées sont bienvenus** via
  GitHub Issues / Discussions. Post-1.0 s'ouvre (le plug-in layer UST + Lua
  est fait exactement pour ça — des providers communautaires sans toucher
  au core figé).
- Le soutien via [StandarX sur OpenCollective](https://opencollective.com/standarx)
  va directement dans la vitesse de shipping.

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
