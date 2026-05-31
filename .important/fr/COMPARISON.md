# Comparaison

[English](../en/COMPARISON.md) · 📖 Français

[Démarrage rapide](QUICKSTART.md) · [Philosophie](storytelling/philosophy.md) · [FAQ](FAQ.md) · [Support](SUPPORT.md)

Comment Standardoc se positionne face aux outils adjacents — ceux que
tu utilises peut-être déjà, et ceux qu'on te citera en réponse à
« c'est pas déjà fait, ça ? ».

> Cette page essaie d'être **honnête, pas flatteuse**. Plusieurs des
> outils ci-dessous sont excellents dans leur métier. La question
> n'est pas « lequel est le meilleur » — c'est « lequel résout *quel*
> problème ». Voir aussi [philosophie](storytelling/philosophy.md)
> pour le cadrage de fond.

---

## L'axe Standardoc

Avant la grille, le repère. Standardoc est une **infrastructure
d'indexation sémantique** avec cinq propriétés tenues comme des
invariants :

- **Local** — l'index vit dans `.standardoc/`, pas dans un cloud
- **AST profond** — `syn` / `swc` / `full_moon` / parsers SFC, pas du
  tree-sitter de surface ni du regex
- **Multi-surface** — un seul graphe consommé par LSP, MCP, et
  bientôt la doc rendue (beta.3)
- **Agent-first** — la surface MCP et la discipline encodée (hooks
  MCP-first) sont conçues pour un agent IA
- **OSS irréversible** — FSL-1.1-MIT avec conversion automatique en MIT
  pur 2 ans par release

La plupart des outils comparés ici tiennent une ou deux de ces
propriétés. Aucun ne tient les cinq — c'est précisément le créneau.

---

## Grille comparative

| Outil | Hébergement | Licence | Parsing | Graphe cross-langage | Surface agent | Prix |
| --- | --- | --- | --- | :---: | --- | --- |
| **Standardoc** | Local | FSL → MIT | AST profond | ✅ IR canonique | MCP natif | Gratuit |
| **code-review-graph** | Local | MIT | AST surface (tree-sitter) | ⚠️ multi-langage | MCP natif | Gratuit |
| **Sourcegraph** | Cloud SaaS | Propriétaire | SCIP / compiler-accurate | ✅ | via Cody | ~$49–59/user/mois |
| **Serena** | Local | MIT | LSP per-langage | ❌ vue per-langage | MCP natif | Gratuit |
| **Aider (repo map)** | Local | Apache 2.0 | AST surface (tree-sitter) | ⚠️ graphe de fichiers | intégré (pas MCP) | Gratuit |
| **Continue** | Local | Apache 2.0 | RAG vectoriel | ❌ pas de graphe | framework MCP | Gratuit |
| **SCIP / Glean / Kythe** | Self-host | OSS (Apache) | Compiler-accurate | ✅ | ❌ format / infra | Gratuit |
| **LSP** (rust-analyzer, tsserver) | Local | OSS | AST profond per-langage | ❌ per-langage | ❌ protocole IDE | Gratuit |
| **TypeDoc / JSDoc / Sphinx** | Local | OSS | AST + annotations requises | ❌ | ❌ | Gratuit |
| **GitBook** | Cloud SaaS | Propriétaire | — (prose manuelle) | ❌ | ❌ | Freemium payant |

Légende : ✅ first-class · ⚠️ partiel / ça dépend · ❌ absent.

La grille n'est qu'un résumé — les paragraphes suivants expliquent ce
qu'aucune colonne ne capture.

---

## Code intelligence SaaS — Sourcegraph

Sourcegraph est l'**anti-Standardoc**. Jadis open-source (Apache 2.0),
il est passé propriétaire en 2023, a fermé son code en 2024, et a pivoté
autour de **Cody** ; l'offre est désormais enterprise-only
(~$49–59/user/mois), hébergée, centrée sur la collaboration et la review
en équipe.

Un produit cohérent — mais il contredit chacun des invariants de
Standardoc : cloud pas local, propriétaire pas OSS irréversible, par
siège pas gratuit, un graphe que tu *loues* pas que tu possèdes. Si ton
besoin est comprendre ton propre code, en local, sans dépendre d'un
fournisseur, Sourcegraph 2026 n'est plus la réponse qu'il était en 2020.

> **SCIP / Glean / Kythe** — dans la même famille « infrastructure
> d'indexing », mais côté formats et back-ends : SCIP est le protocole
> d'indexation de Sourcegraph (successeur de LSIF), Glean celui de
> Meta, Kythe celui de Google. Ils sont compiler-accurate et
> cross-langage, mais **batch-oriented**, lourds à opérer, et pensés
> pour l'indexation interne d'une grande boîte — pas pour un index
> live sur la machine d'un dev solo, pas agent-first, pas de surface
> MCP. Standardoc joue dans la même cour conceptuelle, à l'échelle
> opposée : léger, local, vivant, consommable par un agent en un tool
> call.

---

## Le voisin le plus proche — code-review-graph

[`code-review-graph`](https://github.com/tirth8205/code-review-graph)
(tirth8205) est, de loin, **le voisin idéologique le plus proche** de
Standardoc. Il mérite une comparaison franche et détaillée — d'autant
qu'il est sérieux, bien construit, et qu'il a vu **le même problème de
fond**.

### Ce qu'on partage

Sur le diagnostic, on est d'accord :

- Les agents IA brûlent des tokens à re-scanner la codebase à chaque
  tâche
- La réponse est un **graphe de code local persistant** consommé via
  une surface stable
- Préprocessing structurel **avant** inférence LLM, pas l'inverse

Sur la posture aussi :

- Local SQLite, pas de cloud, pas de télémétrie
- MCP-native, incrémental, watcher hash-based
- Licences permissives (MIT côté eux, FSL → MIT côté nous)
- Multi-langues comme prérequis

C'est exactement la même famille d'idées. Ce n'est pas un adversaire
idéologique comme Sourcegraph — c'est un projet qui a vu le même mal
et qui tente d'y répondre.

### Épistémologie : structure résolue vs supposition scorée

Là où ça diverge nettement, c'est sur **comment on construit le
graphe**.

**code-review-graph** = AST tree-sitter (vrai CST) **+ couche
analytique scorée par-dessus** :

- Edge confidence 3-tier (`EXTRACTED` / `INFERRED` / `AMBIGUOUS`) avec
  scores float
- Leiden communities pour *deviner* les bounded contexts implicites
- Betweenness centrality pour *deviner* les hubs et les bridges
  architecturaux
- Surprise scoring pour *flagger* du couplage inattendu
  (cross-community, cross-language, peripheral-to-hub)
- Knowledge gap analysis, refactoring suggestions générées

C'est de l'épistémologie **probabiliste sur graphe syntaxique** : "cet
edge a confidence X", "ce nœud est probablement un chokepoint avec
score Y". Architecture qualifiable de **structural mining** — fouille
analytique sur ce que tree-sitter a pu extraire.

**Standardoc** = parsers compiler-grade qui résolvent par
construction :

- `syn` lit le système de types Rust (génériques, traits, lifetimes,
  modifiers, signatures full)
- `swc` lit la sémantique TypeScript / JavaScript / JSX / TSX complète
- `full_moon` lit Lua avec extraction emmylua
- Parsers SFC custom pour Vue / Svelte (le `<script>` extrait →
  provider TS)

Ce qui entre dans l'IR canonique n'est **pas inféré avec un score** —
c'est ce que le langage **dit**, par construction. Un `EdgeKind::Calls`
est posé parce que le compiler-grade parser a vu un appel ; un
`EdgeKind::UsesType` parce que le système de types l'a résolu. Quand
on ne sait pas, on marque `Unresolved { name }` plutôt que de
deviner — l'agent en aval peut décider quoi en faire.

Deux épistémologies différentes — **probabiliste vs résolue**. Ni
meilleure ni pire dans l'absolu : ils donnent breadth (24 langues en
surface) ; on donne depth (3 langues + Vue/Svelte SFC en profondeur).
Le tradeoff est explicite.

### Pari produit et horizon de temps

**code-review-graph parie sur la breadth.** 24 langues via tree-sitter,
12 plateformes IA auto-détectées à l'install, et une surface produit
large (visualisation D3.js, multi-repo daemon, semantic search
multi-provider, exports GraphML / Neo4j / Obsidian / SVG, MCP prompt
templates) — préprocessing structurel scoré sur le maximum de stack
possible.

**Standardoc parie sur la depth et le contrat.** 3 providers de langage
en profondeur + Vue/Svelte SFC (le reste passe post-1.0 par le plug-in
layer UST + Lua) ; un IR canonique versionné, figé comme contrat public
à 1.0 ; une licence FSL → MIT irréversible. Un substrat sémantique
stable que humains, CI, agents et futurs renderers consomment sur des
années — pas un cache de tokens par tâche.

### Ce qu'on n'a pas

Honnêteté nécessaire — où ils sont objectivement en avance :

- **Visualisation graphe interactive** — D3.js force-directed. Notre
  webview de navigation visuelle est candidate beta.3, pas shippée.
- **Multi-repo daemon natif** — un seul process supervise plusieurs
  workspaces ; chez nous chaque workspace a son propre couple daemon.
- **Recherche sémantique / vectorielle** — ils offrent des embeddings
  multi-provider. On parie sur la résolution structurelle plutôt que la
  similarité, donc pas de recherche vectorielle chez nous — un choix de
  cohérence, pas un accident.
- **Exports hors-IDE** — GraphML, Neo4j Cypher, Obsidian, SVG. Notre
  graphe se consulte via MCP / LSP / CLI, pas exporté vers des formats
  tiers.
- **Analytics architecturales scorées** — Leiden communities,
  betweenness centrality, surprise scoring. Absentes chez nous **par
  cohérence avec le pari** (résolution structurelle exacte, pas fouille
  scorée).

### Cible et angle

**code-review-graph** est l'outil le plus complet aujourd'hui pour le
token-efficient AI coding sur **un maximum de langues et de
plateformes**, avec des analytics architecturales scorées en bonus.

**Standardoc** est purpose-built pour le **co-work AI-dev cohérent dans
le temps long** sur **monorepos lourds et complexes** — un substrat
sémantique résolu, contractualisé à 1.0, dont le graphe est un **asset
partagé** (humains, CI, agents, futurs renderers) plutôt qu'un cache de
tokens.

Les deux prouvent la même chose : les agents IA qui re-scannent à chaque
tâche, c'est un vrai problème, et la réponse est un graphe local
partagé. **On le résout sous deux paris différents.**

---

## Agents avec contexte intégré — Serena, Aider, Continue

Trois outils qui donnent du contexte de code à un agent, par trois
mécanismes différents — et aucun n'est un index sémantique partagé
multi-surface.

- **Serena** — MCP OSS qui **wrappe les language servers** (LSP) pour
  offrir une navigation et une édition au niveau symbole, sur 30+
  langages. Solide et token-efficient. Mais la vue reste
  **per-langage** (celle du LSP sous-jacent), il n'y a pas de graphe
  cross-langage propre, pas d'IR canonique, pas d'index persistant
  réutilisable hors agent. Serena est un
  *adaptateur LSP pour agents* ; Standardoc est une *infrastructure
  d'indexation* dont le LSP n'est qu'une surface parmi d'autres.

- **Aider (repo map)** — extrait les symboles via tree-sitter et range
  les fichiers par importance (algorithme de ranking type PageRank sur
  le graphe de dépendances de fichiers), dans un budget de tokens
  réglable. Efficace — mais **éphémère par chat**, intégré à Aider, pas
  un index persistant, pas une surface MCP réutilisable par d'autres
  outils.

- **Continue** — **RAG vectoriel** : la codebase est découpée en
  chunks, embeddée, et les chunks les plus *similaires sémantiquement*
  à la tâche sont remontés. C'est une approche par **similarité**, pas
  par **structure** : pas de graphe, pas d'arêtes typées, pas de
  résolution FQDN. Standardoc parie l'inverse — structure résolue, pas
  similarité scorée.

Ces trois-là peuvent même cohabiter avec Standardoc : ce sont des
consommateurs de contexte, Standardoc est le producteur de contexte
structuré qu'ils pourraient consommer.

---

## LSP — complémentaire, pas concurrent

`rust-analyzer`, `tsserver`, `vue-language-server` donnent une
résolution per-langage **profonde** : hover, go-to-definition,
find-references, rename, inférence de type, expansion macro. Standardoc
ne remplace pas ça — il *expose* d'ailleurs LSP comme une de ses
surfaces.

La différence est structurelle : un LSP est **per-IDE et per-langage**,
son graphe est reconstruit à chaque ouverture et meurt entre deux
sessions, et son API est conçue pour un humain qui clique dans un
éditeur — pas pour un agent qui requête une codebase **multi-langues**.
Standardoc unifie Rust + TS + JS + Vue + Svelte + Lua dans un graphe
cross-langage unique, persistant, requêtable via MCP.

**Utilise les deux.** LSP pour la précision éditeur ; Standardoc pour
le graphe transverse et les requêtes d'agent. Post-1.0, un bridge
optionnel vers `rust-analyzer` / `tsserver` est même envisagé pour
fusionner les deux vues (cf.
[vision long terme](storytelling/vision-long-terme.md)).

---

## Générateurs et plateformes de doc — TypeDoc, GitBook & co

`TypeDoc` / `JSDoc` / `Sphinx` répondent à « comment générer un site de
doc narrative depuis mon code ». Ils exigent des **annotations
partout**, produisent un **rendu statique** qui dérive dès le prochain
commit, et ciblent des lecteurs humains sur un site web. Standardoc
indexe **n'importe quelle codebase sans annotation** (l'AST suffit) et
garde l'index **live**.

`GitBook` va plus loin dans le SaaS : plateforme hébergée, prose
manuelle, **aucun lien avec un graphe de code**. C'est un éditeur de
documentation, pas un outil de compréhension de code.

Ce n'est pas vraiment une concurrence — c'est un *futur recouvrement
partiel* : la couche de doc rendue de Standardoc (`@standardoc/core` +
`@standardoc/react`, beta.3) consommera **directement le graphe** comme
source de vérité, avec des adapters Next / Nextra / Astro / Docusaurus.
Le but n'est pas de battre TypeDoc sur son terrain — c'est d'avoir une
doc qui *ne peut pas* dériver, parce qu'elle est dérivée du graphe et
non maintenue à la main. Voir
[vision moyen terme](storytelling/vision-moyen-terme.md).

---

## Quand *ne pas* choisir Standardoc

Réponse honnête, cohérente avec
[la philosophie](storytelling/philosophy.md) :

- **Petite SPA ou projet de quelques milliers de lignes** → `ripgrep` &
  ton LSP IDE suffisent largement. Standardoc serait overkill.
- **Recherche de texte pure** (chaînes littérales, commentaires,
  configs hors-code) → c'est le métier de `grep` / `ripgrep`, et le
  skill agent dit explicitement de fallback dessus dans ces cas.
- **Lire un fichier connu à un chemin connu** → ouvre-le, tout
  simplement.
- **Langage hors du périmètre natif** (Rust / TS-JS / Lua, + Vue /
  Svelte en SFC) → attends le plug-in layer UST + Lua post-1.0, ou
  indexe la partie supportée et documente le reste.
- **Tu veux un assistant IA clé en main** → Standardoc n'est *pas* un
  agent. Il fournit le substrat ; l'agent reste Claude / Cursor /
  Continue / etc. (cf. [FAQ](FAQ.md)).
- **Tu veux une recherche de code partagée en équipe avec UI de
  collaboration** → c'est le métier de Sourcegraph, assume le coût et
  le cloud.

Standardoc est purpose-built pour la **compréhension sémantique
profonde de la structure du code, en local, par un agent IA discipliné**.
Forte là où elle est forte, **overkill ailleurs**, et on l'admet.

---

## Pour aller plus loin

- **[Démarrage rapide](QUICKSTART.md)** — de zéro à un workspace indexé
- **[Philosophie](storytelling/philosophy.md)** — les 5 principes
  system-thinking et l'éthique de construction
- **[FAQ](FAQ.md)** — questions courantes
- **[Support](SUPPORT.md)** — comment soutenir le projet
- **[TODO-LIST](TODO-LIST.md)** — checkboxes exhaustives par milestone
