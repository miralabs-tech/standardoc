# Standardoc

<p align="center">
  <a href="https://github.com/miralabs-tech/standardoc/releases"><img src="https://img.shields.io/badge/status-beta-yellow?style=flat-square" alt="Status: beta"></a>
  <a href="https://github.com/miralabs-tech/standardoc/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/miralabs-tech/standardoc/ci.yml?branch=main&label=ci&style=flat-square" alt="CI"></a>
  <a href="ABOUT.md"><img src="https://img.shields.io/badge/core-canonical%20IR%20%2B%20live%20graph-blueviolet?style=flat-square" alt="Core: canonical IR + live graph"></a>
  <a href="QUICKSTART.md"><img src="https://img.shields.io/badge/surfaces-LSP%20·%20MCP-blue?style=flat-square" alt="Surfaces: LSP · MCP"></a>
  <a href="../../LICENSE"><img src="https://img.shields.io/badge/license-FSL--1.1--MIT%20→%20MIT%202028-green?style=flat-square" alt="License: FSL-1.1-MIT → MIT 2028"></a>
  <a href="https://github.com/miralabs-tech/standardoc/stargazers"><img src="https://img.shields.io/github/stars/miralabs-tech/standardoc?label=stars&style=flat-square" alt="Stars"></a>
  <a href="https://github.com/miralabs-tech/standardoc/releases"><img src="https://img.shields.io/github/downloads/miralabs-tech/standardoc/total?label=release%20downloads&style=flat-square" alt="Release downloads"></a>
  <a href="https://marketplace.visualstudio.com/items?itemName=miralabs-tech.standardoc-vscode"><img src="https://img.shields.io/visual-studio-marketplace/i/miralabs-tech.standardoc-vscode?label=vscode%20installs&style=flat-square" alt="VSCode installs"></a>
  <a href="https://open-vsx.org/extension/miralabs-tech/standardoc"><img src="https://img.shields.io/open-vsx/dt/miralabs-tech/standardoc?label=ovsx%20downloads&style=flat-square" alt="OpenVSX downloads"></a>
</p>

> **Ton agent IA re-lit toute ta codebase à chaque tâche.** Standardoc
> l'indexe une fois en une carte partagée et toujours à jour de ton code —
> pour que l'agent (et tes autres outils) se contentent de *demander* au
> lieu de re-grepper. ~100 tokens par question au lieu de 30k. Local,
> open-source, dérivé directement de ta source.

[English](../../README.md) · 📖 Français

[À propos](storytelling/philosophy.md) · [Quickstart](QUICKSTART.md) · [Roadmap](TODO-LIST.md) · [Comparaison](COMPARISON.md) · [FAQ](FAQ.md) · [Support](SUPPORT.md) · [Changelog](../../CHANGELOG.md)

---

## C'est quoi ?

Standardoc lit ton code directement depuis son arbre syntaxique (Rust,
TypeScript & JavaScript avec React/JSX/TSX, Vue, Svelte, Lua) et en garde
un **graphe sémantique vivant** — chaque symbole, et les liens typés entre
eux : qui appelle qui, qui importe quoi, qui implémente quoi. Un watcher le
garde à jour à mesure que tu édites.

Tes outils se branchent sur ce graphe unique au lieu de re-parser ton code
chacun de leur côté :

- **Pour les agents IA** — un serveur MCP avec des requêtes graphe ciblées
  et read-only (Claude Code, Cursor, Continue, Cody, Aider, Goose, tout
  client MCP). ~100 tokens par question au lieu de 30k de grep + read.
- **Pour les éditeurs** — un daemon LSP auquel tout client se connecte
  (l'extension VSCode officielle l'embarque ; IntelliJ, Neovim, Helix,
  Emacs eglot aussi).
- **À venir** — doc et navigation visuelle générées depuis le même graphe.

Le graphe est l'asset partagé. Personne ne re-parse ton code à côté.

---

## Pourquoi c'est construit comme ça

Ça optimise pour les questions qu'on se pose **après 6 mois** sur une
grosse codebase, pas pour la démo de 2 minutes :

- **Un seul graphe, pas N.** Chaque outil qui re-parse ton code, c'est un
  point de désynchro de plus.
- **AST direct, pas de regex.** Les heuristiques pourrissent dès que le
  code bouge.
- **Local & open-source.** FSL-1.1-MIT, conversion auto en MIT pur
  (première release : 26 avril 2028). Pas de cloud, pas de lock-in, pas de
  graphe loué.

→ L'histoire complète : [`storytelling/`](storytelling/).

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

Puis, depuis un workspace, branche-le sur ton agent en une commande :

```sh
standardoc init        # écrit la skill IA, les hooks MCP-first, AGENTS.md et .mcp.json
```

`init` écrit la skill IA, les hooks MCP-first, une section `AGENTS.md` et
un `.mcp.json` qui lance `standardoc mcp --connect` — un pont léger qui
garde un daemon vivant et watcher-backed pour le workspace. Relancer `init`
est sûr (chaque merge préserve ton contenu). `.mcp.json` porte des chemins
machine-spécifiques — ajoute-le à `.gitignore` si tu collabores.

→ [Walkthrough complet en 5 minutes (QUICKSTART)](QUICKSTART.md)

---

## Pour qui ?

Les codebases grosses et complexes — compilateurs, langages, moteurs,
monorepos lourds, infra multi-équipes. Ça marche aussi sur un petit projet,
mais ce n'est pas là qu'est la valeur ; là, `ripgrep` + ton IDE suffisent.

Le problème qu'il résout : **maintenir un co-work stable et non-déviant
avec un agent IA sur une codebase qui évolue.** Les agents oublient le
contexte d'une session à l'autre, re-greppent ce qu'ils pourraient
interroger, et inventent du code qui ressemble au tien mais casse tes
invariants. Standardoc répond par deux angles — le **graphe** (l'agent
interroge la vraie structure au lieu de deviner) et la **discipline** (un
hook l'empêche de shortcut vers grep avant d'avoir consulté le graphe).

C'est un outil pour le dev, pas un remplaçant : un graphe stable rend un
bon dev plus rapide ; il ne sauvera pas une codebase que personne ne
comprend.

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
