# Standardoc

<p align="center">
  <a href="https://github.com/miralabs-tech/standardoc/releases"><img src="https://img.shields.io/badge/status-beta-yellow?style=flat-square" alt="Status: beta"></a>
  <a href="https://github.com/miralabs-tech/standardoc/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/miralabs-tech/standardoc/ci.yml?branch=main&label=ci&style=flat-square" alt="CI"></a>
  <a href="QUICKSTART.md"><img src="https://img.shields.io/badge/surfaces-LSP%20·%20MCP-blue?style=flat-square" alt="Surfaces: LSP · MCP"></a>
  <a href="../../LICENSE"><img src="https://img.shields.io/badge/license-FSL--1.1--MIT%20→%20MIT%202028-green?style=flat-square" alt="License: FSL-1.1-MIT → MIT 2028"></a>
  <a href="https://github.com/miralabs-tech/standardoc/stargazers"><img src="https://img.shields.io/github/stars/miralabs-tech/standardoc?label=stars&style=flat-square" alt="Stars"></a>
  <a href="https://marketplace.visualstudio.com/items?itemName=miralabs-tech.standardoc-vscode"><img src="https://img.shields.io/visual-studio-marketplace/i/miralabs-tech.standardoc-vscode?label=vscode%20installs&style=flat-square" alt="VSCode installs"></a>
  <a href="https://open-vsx.org/extension/miralabs-tech/standardoc"><img src="https://img.shields.io/open-vsx/dt/miralabs-tech/standardoc?label=ovsx%20downloads&style=flat-square" alt="OpenVSX downloads"></a>
</p>

> **Ton agent IA re-lit toute ta codebase à chaque tâche.** Standardoc
> l'indexe une fois en une carte vivante de ton code — l'agent se contente
> de *demander*. **~100 tokens par question au lieu de 30k.** Local,
> open-source.

[English](../../README.md) · 📖 Français &nbsp;|&nbsp; [Démarrage rapide](QUICKSTART.md) · [Roadmap](TODO-LIST.md) · [Changelog](../../CHANGELOG.md)

---

## Le problème

À chaque tâche, ton agent repart de zéro : il grep, il lit des fichiers, il
crame 30k tokens à reconstruire un contexte qu'il avait déjà la session
d'avant. Plus la codebase grossit, pire c'est — plus de tokens, plus de
dérive, et du code qui *ressemble* au tien mais casse tes invariants en
silence.

## Ce que fait Standardoc

Il lit ton code directement depuis l'arbre syntaxique et en garde un
**graphe vivant** — chaque symbole, et les liens typés entre eux : qui
appelle qui, qui importe quoi, qui implémente quoi. Un watcher le garde à
jour pendant que tu tapes.

Tes outils requêtent ce graphe unique au lieu de re-parser ton code chacun
de leur côté :

- **Les agents** demandent via MCP (`find_symbol`, `get_context`,
  `find_call_sites`, …) — **~100 tokens** là où grep + read en coûtent 30k.
  Claude Code, Cursor, Continue, Copilot, n'importe quel client MCP.
- **Les éditeurs** se connectent via LSP — l'extension VSCode l'embarque ;
  Neovim, Helix, JetBrains pointent le même binaire.

Rust, TypeScript / JavaScript (React, JSX, TSX), Vue, Svelte, Lua, et C
aujourd'hui.

## Installer

**VSCode** — cherche *Standardoc* dans le Marketplace ou Open VSX.

**CLI** (n'importe quel agent, sans VSCode) :

```sh
cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli
standardoc init   # câble la skill agent, les hooks MCP-first, AGENTS.md, .mcp.json
```

→ [Démarrage rapide en 5 minutes](QUICKSTART.md)

## Pour qui

Les codebases grosses et complexes — compilateurs, moteurs, monorepos
lourds. Sur un projet de week-end, `ripgrep` + ton IDE suffisent ;
Standardoc gagne son pain quand l'archéologie commence à te coûter du temps.

## Pourquoi c'est construit comme ça

- **Un seul graphe, pas N.** Chaque outil qui re-parse ton code, c'est une
  chose de plus qui dérive.
- **Du vrai AST, jamais de regex.** Les heuristiques pourrissent dès que le
  code bouge.
- **À toi, pour de bon.** Local, [FSL-1.1-MIT](../../LICENSE) qui se
  convertit en MIT pur (première release : 26 avril 2028). Pas de cloud, pas
  de lock-in, pas de graphe loué.

---

Porté par [**StandarX**](https://opencollective.com/standarx) &nbsp;·&nbsp; [Star le repo](https://github.com/miralabs-tech/standardoc) · [Sponsor](https://opencollective.com/standarx) · [Sécurité](SECURITY.md)
