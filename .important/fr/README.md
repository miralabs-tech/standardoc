# Standardoc

<p align="center">
  <a href="https://github.com/miralabs-tech/standardoc/releases"><img src="https://img.shields.io/badge/status-archived-lightgrey?style=flat-square" alt="Status: archived"></a>
  <a href="https://github.com/miralabs-tech/standardoc/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/miralabs-tech/standardoc/ci.yml?branch=main&label=ci&style=flat-square" alt="CI"></a>
  <a href="QUICKSTART.md"><img src="https://img.shields.io/badge/surfaces-LSP%20·%20MCP-blue?style=flat-square" alt="Surfaces: LSP · MCP"></a>
  <a href="../../LICENSE"><img src="https://img.shields.io/badge/license-FSL--1.1--MIT%20→%20MIT%202028-green?style=flat-square" alt="License: FSL-1.1-MIT → MIT 2028"></a>
  <a href="https://github.com/miralabs-tech/standardoc/stargazers"><img src="https://img.shields.io/github/stars/miralabs-tech/standardoc?label=stars&style=flat-square" alt="Stars"></a>
  <a href="https://marketplace.visualstudio.com/items?itemName=miralabs-tech.standardoc-vscode"><img src="https://img.shields.io/visual-studio-marketplace/i/miralabs-tech.standardoc-vscode?label=vscode%20installs&style=flat-square" alt="VSCode installs"></a>
  <a href="https://open-vsx.org/extension/miralabs-tech/standardoc"><img src="https://img.shields.io/open-vsx/dt/miralabs-tech/standardoc?label=ovsx%20downloads&style=flat-square" alt="OpenVSX downloads"></a>
</p>

> ## ⚠️ Archivé sur v1.0.0-beta.2 — 2026-09-17
>
> Standardoc est archivé. **[v1.0.0-beta.2](https://github.com/miralabs-tech/standardoc/releases/tag/v1.0.0-beta.2)**
> est la dernière release compilée : elle s'installe et tourne encore, mais
> rien ne sera corrigé, mis à jour ni répondu. L'extension VSCode sur le
> Marketplace / Open VSX épingle ce binaire et ne bougera plus.
>
> Le source beta.3 sur `main` (graphes multi-workspace, graph viz, provider
> C, `standardoc init`, refonte de la résolution d'arêtes) ne sera **pas**
> tagué ni publié. Tu le veux quand même ? Compile-le toi-même, à tes
> risques :
> `cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli`
>
> **Pourquoi.** Mesuré les 16-17/09/2026 — A/B en vrais tokens (même modèle,
> mêmes questions, sur ce repo) plus comptages `sqlite3` sur
> `.standardoc/index.db` :
>
> - **Aucun gain mesurable face à grep** sur un repo de taille courante. Le
>   chemin MCP économise des tours quand il faut énumérer des appelants et
>   coûte plus de tokens partout ailleurs. Toute tâche qui finit par une
>   édition exige de toute façon le `Read` du fichier : la requête graphe
>   est purement additive.
> - **Des trous silencieux.** Moins de 40 % des arêtes `CALLS` résolues ; le
>   code généré par macro est un angle mort ; les appelants non résolus
>   reviennent comme une liste vide, pas comme « inconnu ». Un agent qui
>   fait confiance conclut « code mort ».
> - **Les harnais ont rattrapé.** Claude Code et Codex embarquent maintenant
>   des plugins LSP officiels ; le cas cross-langage (Tauri) est couvert par
>   specta / bindgen.
>
> Le slogan « ~100 tokens au lieu de 30k » des versions précédentes de ce
> README n'a jamais été mesuré et est retiré.

> **Ton agent IA re-lit toute ta codebase à chaque tâche.** Standardoc
> l'indexait une fois en une carte vivante de ton code — l'agent n'avait
> qu'à *demander*. Local, open-source.

[English](../../README.md) · 📖 Français &nbsp;|&nbsp; [Démarrage rapide](QUICKSTART.md) · [Roadmap](TODO-LIST.md) · [Changelog](../../CHANGELOG.md)

---

## Le problème

À chaque tâche, ton agent repart de zéro : il grep, il lit des fichiers, il
reconstruit un contexte qu'il avait déjà la session d'avant. Plus la
codebase grossit, pire c'est — plus de tokens, plus de dérive, et du code
qui *ressemble* au tien mais casse tes invariants en silence. C'était le
pari. Mesuré, le pari n'a pas payé sur des repos de taille courante (voir
le bandeau ci-dessus).

## Ce que fait Standardoc

Il lit ton code directement depuis l'arbre syntaxique et en garde un
**graphe vivant** — chaque symbole, et les liens typés entre eux : qui
appelle qui, qui importe quoi, qui implémente quoi. Un watcher le garde à
jour pendant que tu tapes.

Tes outils requêtent ce graphe unique au lieu de re-parser ton code chacun
de leur côté :

- **Les agents** demandent via MCP (`find_symbol`, `get_context`,
  `find_call_sites`, …). Testé avec Claude Code uniquement ; les autres
  clients MCP n'ont jamais été exercés.
- **Les éditeurs** se connectent via LSP — l'extension VSCode est la seule
  intégration construite et testée.

Rust, TypeScript / JavaScript (React, JSX, TSX), Vue, Svelte, Lua, et C
aujourd'hui.

## Installer

**VSCode** — cherche *Standardoc* dans le Marketplace ou Open VSX. Tu
obtiens l'extension archivée, épinglée sur le binaire beta.2.

**Depuis le source** (beta.3, non publiée — `standardoc init` n'existe
qu'ici) :

```sh
cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli
standardoc init   # câble la skill agent, AGENTS.md, .mcp.json
```

→ [Démarrage rapide](QUICKSTART.md) (décrit le source `main`)

## Où ça a servi

Uniquement sur ce dépôt et une poignée de projets perso de l'auteur. Ça n'a
jamais tourné sur un compilateur, un moteur de jeu ou un gros monorepo, et
aucun benchmark de scale n'a jamais été construit. Sur un projet de
week-end, `ripgrep` + ton IDE suffisent — et, mesuré, ils suffisaient ici
aussi.

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
