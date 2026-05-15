# Standardoc

![status](https://img.shields.io/badge/status-beta-yellow)
![ci](https://img.shields.io/github/actions/workflow/status/miralabs-tech/standardoc/ci.yml?branch=main&label=ci)
![core](https://img.shields.io/badge/core-canonical%20IR%20%2B%20live%20graph-blueviolet)
![surfaces](https://img.shields.io/badge/surfaces-LSP%20·%20MCP%20·%20RAG-blue)
![license](https://img.shields.io/badge/license-FSL--1.1--MIT%20→%20MIT%202028-green)
![stars](https://img.shields.io/github/stars/miralabs-tech/standardoc?label=stars&color=informational)
![release downloads](https://img.shields.io/github/downloads/miralabs-tech/standardoc/total?label=release%20downloads)
![vscode installs](https://img.shields.io/visual-studio-marketplace/i/miralabs-tech.standardoc?label=vscode%20installs)
![ovsx downloads](https://img.shields.io/open-vsx/dt/miralabs-tech/standardoc?label=ovsx%20downloads)

> Une infrastructure d'intelligence de code, bâtie sur un IR canonique
> multi-langues et un graphe sémantique vivant. Un graphe, plusieurs
> daemons (LSP, MCP stdio + HTTP/SSE), tous tes outils branchés dessus.
> ~100 tokens par requête d'agent au lieu de 30k de grep + read.
> Local, dérivé du code source, open-source.
>
> Pensé pour ce qui casse à l'échelle et devient ingérable dans 6 mois,
> pas pour la démo de 2 minutes. La compréhension de code est un
> système, pas une suite de greps.

[English](../../README.md) · 📖 Français

[À propos](storytelling/philosophy.md) · [Quickstart](QUICKSTART.md) · [Roadmap](TODO-LIST.md) · [Comparaison](COMPARISON.md) · [FAQ](FAQ.md) · [Support](SUPPORT.md) · [Changelog](../../CHANGELOG.md)

---

## C'est quoi ?

Standardoc indexe ton code en un **graphe sémantique vivant** :

- AST direct, multi-langues (Rust, TypeScript & JavaScript avec React/JSX/TSX, Vue, Svelte, Lua aujourd'hui)
- IR canonique unifié — types de nœuds + edges typés partagés cross-langue
  (`CALLS`, `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`, `DEFINES`,
  `USES_TYPE`, `EXPOSES_API`), avec attributs structurés sur certains edges
- SQLite + FTS5, watcher filesystem, invalidation BLAKE3, schéma versionné
- Dérivé du code (pas une source à porter en plus), reproductible sur
  n'importe quelle machine en quelques secondes

**Plusieurs surfaces consomment cet état** :

- **LSP daemon** (`standardoc lsp`, stdio, primary writer du graphe) —
  l'extension VSCode officielle l'embarque ; tout client LSP peut s'y
  connecter (IntelliJ, Neovim, Helix, Emacs eglot, …) en pointant le
  binaire
- **MCP daemon** (`standardoc mcp`, stdio ou HTTP/SSE multi-client,
  readonly) — 16 tools pour Claude Code, Cursor, Continue, Cody, Aider,
  Goose, et tout client MCP
- **Layer RAG** (`.standardoc/rag.db`, linkée au graphe par FQDN) —
  chunks prose (`README.md`, `docs/`, `notes/`, ABOUT, etc.) ré-rangés
  via embedder Candle/BGE-small, accessibles depuis les deux daemons
  (via `fetch_chunks` MCP ou `chunk_refs` de `get_context`)
- **Sessions DB** (`.standardoc-sessions/sessions.db`, orthogonale au
  graphe) — memos d'agent persistants entre chats, accédés via les
  `session_*` tools MCP. Contenu humain, pas dérivé du code
- *À venir* — doc statique générée depuis le graphe (`@standardoc/react`
  + adapters Nextra/Docusaurus/Astro), navigation visuelle, plugins de
  langues via UST + Lua

**Le résultat** : tes outils arrêtent de re-parser ton code chacun de leur
côté. Le graphe est l'asset partagé. ~100 tokens par requête d'agent au
lieu de 30k de grep + read.

---

## Posture

Standardoc optimise pour les questions qu'on se pose **après 6 mois** sur
un monorepo, pas pour la démo de 2 minutes :

- *Qu'est-ce qui reste stable malgré les changements ?* → **IR canonique**
  (les langages mutent, l'IR pas)
- *Quels choix deviennent irréversibles ?* → **open-source FSL-1.1-MIT**
  qui devient MIT au 26 avril 2028 (pas de lock-in SaaS, pas de changement
  de termes rétroactif possible)
- *Qu'est-ce qui crée de la dette cognitive ?* → **un graphe partagé** (N
  outils qui re-parsent ton code = N points de désynchro)
- *Qu'est-ce qui casse à l'échelle ?* → **AST direct** (pas de regex ni
  d'heuristiques qui rot fast)
- *Qu'est-ce qui devient incompréhensible dans 6 mois ?* → **MCP-first
  guardrail** (un agent qui grep 30k tokens à chaque tâche n'est ni
  compréhensible ni débuggable)

La compréhension de code est un système, pas une suite de greps.
Standardoc est l'infrastructure de ce système.

→ Détails dans [`storytelling/`](storytelling/) : philosophie, vision
court/moyen/long terme, observations dogfood, retours de tests.

---

## Installer

**Extension VSCode** (recommandée — embarque le daemon, génère la skill IA,
écrit `.mcp.json`) :

Cherche "Standardoc" dans le panneau Extensions, ou récupère le `.vsix` sur
les [releases](https://github.com/miralabs-tech/standardoc/releases).

**CLI standalone** (sans VSCode) :

```sh
cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli
standardoc --version
```

→ [Walkthrough complet en 5 minutes (QUICKSTART)](QUICKSTART.md)

---

## Pour qui ?

Standardoc est conçu pour les **codebases grosses et complexes** — pensé
en dogfood sur Standardoc lui-même et calibré pour des projets du même
calibre : compilateurs, langages de programmation, moteurs (jeu / runtime
/ db), monorepos applicatifs lourds, infra multi-équipes. Pas pour la
petite app JS de week-end — ça marchera quand même, mais ce n'est pas
là que la valeur est la plus forte.

Le problème central qu'il résout :
**maintenir un co-work stable, contrôlé, non-déviant avec un agent IA
sur un codebase qui évolue**. C'est le problème que personne d'autre
n'aborde frontalement aujourd'hui — la plupart des outils s'arrêtent
à "lui donner le contexte d'une session", pas "tenir la cohérence
sur 6 mois".

Les agents IA dérivent : ils oublient le contexte d'une session à
l'autre, re-greppent ce qu'ils auraient pu interroger, inventent du
code qui ressemble au tien sans respecter tes invariants, ne se
souviennent pas de la décision lockée la semaine dernière. À chaque
tâche, l'archéologie recommence — et plus le projet grossit, plus
l'archéologie coûte cher (tokens, patience, bugs subtils, dette
cognitive humaine).

Standardoc adresse ça par 3 angles complémentaires :

- **Le graphe** — l'agent interroge la vraie structure (FQDN, edges,
  body, prose RAG), il n'invente pas
- **La discipline** — `MCP-first guardrail` empêche l'agent de
  shortcut vers `grep + read` ; le hook PreToolUse le force à passer
  par le graphe avant tout
- **La mémoire** — les décisions lockées d'une session survivent
  dans la DB sessions (`session_save` / `session_get`) ; l'agent
  retrouve le contexte à la session suivante au lieu de tout
  re-découvrir

Standardoc est un outil de co-work AI-dev, **pas un substitut au
dev**. Un agent qui interroge un graphe sémantique stable est
puissant ; un dev qui ne comprend pas son code en sortira toujours
frustré quelle que soit l'IA derrière.

---

## Support

Standardoc est un projet de [**StandarX**](https://opencollective.com/standarx)
— une organisation qui développe des outils open-source orientés
intelligence de code, infrastructure et agents IA.

Si Standardoc te fait gagner du temps :

[Star le repo](https://github.com/miralabs-tech/standardoc) · [Soutenir StandarX sur OpenCollective](https://opencollective.com/standarx) · [Autres moyens](SUPPORT.md)

---

## Licence

[**FSL-1.1-MIT**](../../LICENSE) — Functional Source License v1.1 avec
conversion automatique en MIT pur. Chaque release devient MIT 2 ans
après sa date de sortie ; la première release convertit le
**26 avril 2028**. Gratuit pour tout usage non-concurrent aujourd'hui,
entièrement MIT à partir de ces dates.
