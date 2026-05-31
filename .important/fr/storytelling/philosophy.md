# Philosophie

[English](../../en/storytelling/philosophy.md) · 📖 Français

[← Retour au README](../README.md) · [Vision court terme](vision-court-terme.md) · [Vision moyen terme](vision-moyen-terme.md) · [Vision long terme](vision-long-terme.md) · [Remarques](remarques.md) · [Retours de tests](retours-tests.md)

---

## Le problème qu'on essaie de résoudre

**La compréhension de code est un système.** Pas une suite de recherches.
Pas un agent IA isolé qui re-grep ta codebase à chaque tâche. Pas un LSP
qui sait répondre à `goto-definition` mais qui oublie ton intention
deux secondes après. Pas un Sourcegraph qui indexe ton repo dans un
cloud distant pour te le renvoyer en API payante par utilisateur.

C'est un **système** — c'est-à-dire un ensemble de composants qui se
parlent, qui partagent une représentation commune, qui évoluent ensemble,
et qui restent cohérents dans le temps. Quand un système de compréhension
de code est correctement bâti, **n'importe quel outil de l'écosystème**
(IDE, agent IA, générateur de doc, dashboard de navigation, plugin de
revue) peut consommer la même source de vérité sans la re-parser de son
côté, sans drift, sans dette cognitive parallèle.

Aujourd'hui, ce système n'existe pas. Chaque outil construit le sien :

- Ton LSP parse ton code → graphe per-IDE, mort entre deux ouvertures
- Ton agent IA grep ton code → archéologie par tâche, 30k tokens, zéro
  capitalisation entre les sessions
- Ton générateur de doc parse ton code → encore un autre AST, encore
  une autre invalidation, encore une fois rotté dès la prochaine MAJ
- Ton outil de revue parse ton code → ad nauseam

**N fois le même travail, N fois les mêmes bugs, N fois la même
désynchronisation entre la vérité (le code source) et les représentations
dérivées (les indexes éparpillés).**

Standardoc est la tentative de collapser tout ça en **une seule
indexation partagée**, avec un IR canonique au centre, et autant de
surfaces de consommation que ton workflow en demande.

---

## Le diagnostic — pourquoi les approches actuelles ne tiennent pas

### Les LSP sont per-IDE et per-langage

Tower-LSP, vscode-languageclient, tower-lsp-server, etc. — chaque IDE a
sa propre intégration LSP, et chaque langage a son propre serveur LSP
(`rust-analyzer`, `tsserver`, `vue-language-server`). C'est bien — pour
un IDE. Pour un agent IA qui veut comprendre **une codebase
multi-langues** (TS qui consomme une lib Rust via WASM, Vue qui appelle
des composants TS, Lua qui require un module Rust), les LSP ne te
donnent qu'une vue partielle, fragmentée, et avec un API conçu pour un
IDE humain qui clique, pas pour un agent qui requête.

### Les outils regex / textuels rotent

`grep`, `ripgrep`, ctags, tree-sitter en mode standalone — c'est rapide,
c'est portable, mais c'est **fragile dès que le code mute**. Un rename de
variable, un refactor de namespace, une migration de framework, et toutes
les heuristiques regex et conventions de nommage volent en éclats.
Maintenir un tooling regex sur une grosse codebase, c'est faire de la
maintenance permanente d'un cache qui se désynchronise tout le temps.

### Les agents IA en mode `grep + read` ne capitalisent pas

L'agent ouvre une session. Il grep, il read, il accumule 30k tokens de
contexte. Il fait sa tâche. Il ferme la session. **Tout le contexte est
perdu.** À la session suivante, il recommence à zéro — re-grep, re-read,
re-30k tokens. Si la décision de la session précédente avait des
implications subtiles sur l'architecture, l'agent ne les retient pas. Si
tu avais lock une convention ("on ne tape jamais directement la DB
depuis le handler, on passe par le repository pattern"), l'agent doit
re-découvrir cette règle à chaque tâche.

C'est **non-scalable cognitivement** : plus la codebase grossit, plus
chaque tâche coûte de tokens, et plus le risque de dérive (l'agent qui
prend un raccourci, qui invente un appel qui n'existe pas, qui ignore une
convention) augmente. On a vu des projets où l'agent finit par "ne plus
être utilisable" — non pas parce que l'IA est mauvaise, mais parce que
le système de compréhension autour de l'IA est inexistant.

### Les SaaS de code intelligence créent du lock-in

Sourcegraph (cloud), Codesee, GitGuardian, etc. — ces services indexent
ton code dans LEUR infrastructure, te le renvoient via LEUR API, et te
facturent à l'utilisateur. Si tu arrêtes de payer, tu perds l'index.
Si leur entreprise pivote, change de pricing, ferme, est rachetée — tu
perds l'asset. **Le graphe n'est pas le tien.** C'est un service que tu
loues. Pour les boîtes ça peut être OK. Pour les projets open-source,
les langages indépendants, les indé qui veulent porter leur tooling sur
plusieurs machines sans demander la permission à un fournisseur — c'est
une régression.

---

## Les principes system-thinking appliqués

Cinq questions guident chaque décision de conception dans Standardoc.
Elles viennent d'une lecture system-thinking du problème, pas d'une
checklist marketing. Elles se résument à : **qu'est-ce qui survit à 6
mois ?** Si la réponse à cette question est "rien", alors on ne devrait
pas le construire.

### 1. Qu'est-ce qui reste stable malgré les changements ?

Les langages mutent. TypeScript ajoute des features chaque release.
Rust évolue son `async`. Vue passe de 2 à 3 puis ajoute le composition
API. Lua a déjà 5 dialectes (PUC, Luau, LuaJIT, …).

**Ce qui doit rester stable, c'est l'IR.** Un symbole, c'est un nom
(FQDN), une localisation (file + span), une signature, des modifiers,
des relations sortantes (edges). Cette abstraction est cross-langage
et cross-décennie. Quand on ajoute Go dans 6 mois, on n'invente pas un
nouveau format — on étend les frontends pour produire des `RawSymbol`
et `RawEdge` qui existent déjà.

→ **Décision** : un crate dédié `standardoc-ir` qui définit la grammaire
stable. Tout le reste est consumer.

### 2. Quels choix deviennent irréversibles ?

Les pivots de licence. Les modèles SaaS-locked. Les formats de DB
propriétaires. Les dépendances sur des services cloud d'un fournisseur.
Une fois engagé, on ne revient pas en arrière sans casser ses
utilisateurs.

**Standardoc minimise les pièges irréversibles** :

- License FSL-1.1-MIT avec **conversion automatique en MIT pur** au bout
  de 2 ans par release. La première convertit le 26 avril 2028. À
  partir de là, le core est légalement MIT pour toujours — peu importe
  ce qui se passe avec l'entreprise, le mainteneur, le marché.
- SQLite + FTS5 — format binaire OUVERT, lisible avec n'importe quel
  outil sqlite3 standard, dumpable, migrable, versionable.
- Pas de cloud, pas d'auth, pas de telemetry phone-home. Si on disparaît
  demain, ton index continue de fonctionner.

→ **Décision** : open-source avec un moat temporel garanti par licence,
pas par contrat ou bonne foi.

### 3. Qu'est-ce qui crée de la dette cognitive ?

**Tout ce qui existe en N exemplaires qui doivent être maintenus en
phase.** N parsers de ton code dans N outils différents. N versions
quasi-identiques de ton schéma de doc. N façons de représenter la même
relation entre deux symboles. Plus N est grand, plus la probabilité de
drift entre les exemplaires tend vers 1 quand le code mute.

**Standardoc collapse N → 1 sur l'indexation.** Le graphe est calculé
une fois (par revision du code), stocké une fois, consommé par autant
de surfaces que ton workflow le demande. Quand le code mute, le watcher
re-indexe le delta, et toutes les surfaces voient simultanément le
nouvel état.

→ **Décision** : un graphe partagé exposé par plusieurs daemons, pas
plusieurs graphes synchronisés à la main.

### 4. Qu'est-ce qui casse à l'échelle ?

Les heuristiques. Les regex. Les conventions de nommage qu'on suppose
sans pouvoir les vérifier. Tout ce qui fonctionne sur 100 fichiers et
plante sur 10000.

**Standardoc utilise des parsers AST natifs** : `syn` pour Rust, `swc`
pour TS / JS / JSX / TSX (React inclus), `full_moon` pour Lua, des
parsers SFC custom pour Vue et Svelte. Pas de regex pour extraire les FQDNs. Pas d'heuristique pour
deviner si un identifier est une fonction ou un type. Si l'AST le dit,
on a la vérité ; sinon, on ne le sait pas et on l'admet (`Unresolved
{ name }`).

→ **Décision** : AST direct, jamais regex. Quand un symbole ne peut pas
être résolu, on marque `Unresolved` plutôt que de deviner — l'agent en
aval peut décider quoi en faire.

### 5. Qu'est-ce qui devient incompréhensible dans 6 mois ?

Les agents IA qui shortcut leur protocole. Les sessions de chat qui
n'ont pas de trace persistante des décisions. Les caches qui ne
s'invalident pas correctement quand le code mute.

**Standardoc impose une discipline observable** :

- **MCP-first guardrail** : avant qu'un agent puisse `Bash` / `Read` /
  `Grep` / `Glob` sur ta codebase, il DOIT avoir appelé un tool MCP
  Standardoc dans la session courante. Le hook PreToolUse bloque, le
  hook SessionStart reset le sentinel à chaque nouveau chat. Résultat :
  l'agent ne peut pas dégénérer en grep-loop par paresse.
- **`current_revision()` + `check_stale()`** : l'agent peut vérifier si
  sa connaissance d'un symbole est encore fraîche, ou si le watcher a
  re-indexé quelque chose depuis. Plus de "j'avais lu cette fonction il
  y a 3 minutes mais elle a changé entre-temps".

→ **Décision** : la discipline est encodée dans le système, pas dans
les bons sentiments de l'utilisateur.

---

## Ce que Standardoc n'est PAS

Pour éviter les comparaisons paresseuses :

- **Ce n'est pas un Sourcegraph local.** Sourcegraph est un moteur de
  recherche full-text + symbol partagé en équipe avec un focus produit
  sur la collaboration code review. Standardoc est une infrastructure
  d'indexation sémantique multi-surface, focus AI agent + multi-frontend.
  Les deux peuvent coexister sur le même monorepo et n'adressent pas le
  même problème.

- **Ce n'est pas un LSP de plus.** Standardoc EXPOSE LSP comme une de
  ses surfaces de consommation, mais sous le capot c'est un graphe
  global, pas un serveur per-langage. L'extension VSCode wrap le LSP
  daemon, les clients LSP standard peuvent s'y connecter — mais la
  valeur réelle est dans le graphe + MCP, pas dans le LSP isolé.

- **Ce n'est pas un agent IA.** Standardoc fournit l'infrastructure
  qu'un agent IA consomme. L'agent reste Claude / Cursor / Continue /
  Cody / ce-que-tu-veux. Standardoc ne décide pas à la place de l'agent,
  il lui donne juste un substrat structuré pour ne pas avoir à grep.

- **Ce n'est pas un générateur de doc (encore).** Le doc rendering layer
  (`@standardoc/react`, adapters Nextra/Docusaurus) arrive en beta.3. Le
  graphe est prêt à le servir aujourd'hui ; le rendu n'est juste pas
  encore écrit.

- **Ce n'est pas un service hosted.** Pas de cloud, pas de SaaS, pas de
  telemetry. Tout vit dans `.standardoc/` sur ta machine, gitignored,
  reproductible. Si un jour un service hosted complémentaire émerge
  (doc UI, dashboard de navigation), il sera optionnel et le core
  restera permanent open-source.

- **Ce n'est pas un substitut au dev.** Un outil de co-work est puissant
  pour un dev qui comprend déjà son système. Pour un dev qui ne maîtrise
  pas sa codebase, aucune IA et aucun graphe ne suffira à compenser.
  Standardoc est un amplificateur, pas un remplaçant.

- **Ce n'est pas un système auto-magique.** Standardoc encode ce qui
  peut l'être — un skill template auto-généré qui enseigne à l'agent la
  logique d'usage, des hooks MCP-first. Mais **coupler un agent à ces
  conventions, architecturer son projet avec un minimum de cohérence, et
  savoir indiquer à l'agent quand utiliser quel outil** reste à la charge
  de l'opérateur. Tous les agents ne consomment pas le protocole de la
  même façon : la calibration est tripartite (infra + agent + opérateur).
  Voir [retours de tests](retours-tests.md).

---

## L'éthique de construction

**Craft before promises.** Rien ne ship avant que ça marche localement,
ait des tests, et s'intègre proprement. On n'annonce pas une feature pour
le buzz avant qu'elle prenne forme. Et on dogfood — Standardoc utilise
Standardoc pour s'auto-comprendre ; si l'outil ne nous est pas utile, il
n'est utile à personne.

Ce n'est pas pour tout le monde, et c'est OK : sur une SPA de 5000 lignes,
`ripgrep` + ton IDE suffisent. La valeur de Standardoc est forte sur les
codebases grosses et complexes, **overkill ailleurs**, et on l'admet.

---

## Pour aller plus loin

- **[Vision court terme →](vision-court-terme.md)** — ce qui ship en
  beta.2 et 1.0
- **[Vision moyen terme →](vision-moyen-terme.md)** — beta.3 (doc
  rendering, CLI self-managed) et 1.x
- **[Vision long terme →](vision-long-terme.md)** — UST + Lua plugin
  layer, plateforme post-1.0
- **[Remarques →](remarques.md)** — observations, décisions lockées,
  apprentissages dogfood
- **[Retours de tests →](retours-tests.md)** — ce qu'on a testé, ce qui
  a marché, ce qu'on a abandonné
