# FAQ

[English](../en/FAQ.md) · 📖 Français

[Démarrage rapide](QUICKSTART.md) · [Philosophie](storytelling/philosophy.md) · [Vision court terme](storytelling/vision-court-terme.md) · [Comparaison](COMPARISON.md) · [Support](SUPPORT.md)

---

## C'est un outil de documentation ?

Pas encore, et pas au sens TypeDoc / JSDoc. Aujourd'hui Standardoc est
un **indexeur sémantique** — un graphe de code vivant exposé via
MCP / LSP. La doc *rendue* (sites statiques, composants
`<Doc id>`) est une couche de consommation prévue en **beta.3** :
`@standardoc/core` (query API framework-agnostic) + `@standardoc/react`
(premier renderer, adapters Next / Nextra / Astro / Docusaurus). Le
graphe est déjà prêt à la servir ; le rendu n'est juste pas encore
écrit. Voir [vision moyen terme](storytelling/vision-moyen-terme.md).

## Ça remplace mon LSP ?

Non, ça le **complète**. Standardoc *expose* LSP comme une de ses
surfaces de consommation, mais sous le capot c'est un graphe global
cross-langage, pas un serveur per-langage. `rust-analyzer` / `tsserver`
gardent la résolution per-langage profonde (inférence de type complexe,
expansion macro, completion contextuelle) ; Standardoc apporte le
graphe transverse + la surface MCP. Utilise les deux.
Post-1.0, un bridge optionnel vers `rust-analyzer` / `tsserver` est même
envisagé pour fusionner les deux vues via une seule interface MCP — cf.
[vision long terme](storytelling/vision-long-terme.md).

## En quoi c'est différent de Sourcegraph ?

Sourcegraph est un moteur de recherche full-text + symbol partagé en
équipe, hébergé (cloud SaaS), avec un focus produit sur la
collaboration et la code review. Standardoc est une **infrastructure
d'indexation sémantique locale**, multi-surface, focus agent IA +
multi-frontend. Pas de cloud, pas d'auth, pas de facturation par
utilisateur — l'index vit dans `.standardoc/` sur ta machine. Les deux
peuvent coexister sur le même monorepo : ils n'adressent pas le même
problème. Grille détaillée dans [COMPARISON.md](COMPARISON.md).

## Pourquoi pas juste Tree-sitter ou ripgrep directement ?

Sur une petite SPA, `ripgrep` & ton LSP IDE suffisent largement — on
l'assume ([philosophie](storytelling/philosophy.md)). Tree-sitter en
standalone donne un AST **de surface** (fonctions, classes, calls —
proche du regex). Standardoc utilise des parsers AST **profonds** :
`syn` (Rust), `swc` (TS / JS / JSX / TSX), `full_moon` (Lua), parsers
SFC custom (Vue, Svelte) — signatures complètes, types, génériques,
traits, modifiers, edges typés. C'est le différentiel central face aux
approches textuelles.

Nuance : tree-sitter **reviendra** post-1.0, mais comme parser
universel *sous* le plug-in layer UST + Lua (parsing délégué à
tree-sitter, sémantique en Lua, validation IR en Rust) — pas comme
moteur d'indexation de surface. Voir
[vision long terme](storytelling/vision-long-terme.md).

## Quelle différence vs `Read` / `Grep` / `Glob` de mon agent ?

Les outils natifs d'un agent répondent aux questions **niveau texte**.
Standardoc répond aux questions **niveau graphe** — callers, callees,
imports, relations de types, arêtes cross-langage — sans que l'agent
ait à reconstituer ces faits depuis des scans texte. Ordre de grandeur
observé en dogfood : **~100 tokens par question au lieu de ~30k**. Le
skill agent auto-généré au workspace init instruit l'agent d'utiliser
Standardoc en priorité, et de ne fallback sur `Read` / `Grep` / `Glob`
que quand le graphe ne peut pas répondre (vrais string-literals :
commentaires, configs hors-code, build files).

## Quels langages sont supportés ?

Trois providers de langage natifs : **Rust** (`syn`),
**TypeScript / JavaScript** incluant **JSX & TSX — React** (`swc`),
**Lua** (`full_moon`). **Vue** et **Svelte** sont supportés en plus via
parsing SFC : le `<script>` d'un composant est extrait et confié au
provider TS, et les `<template>` passent par des parsers SFC custom. Ce
sont les langages qui ont porté Standardoc en dogfood jusqu'à 1.0 — le
critère n'est pas le nombre de langages, c'est la profondeur AST. Deux
langages bien supportés valent mieux que dix à moitié.

## Quand le langage X (Python / Go / Java / C…) sera-t-il supporté ?

Pas avant 1.0 dans le core. La stratégie n'est **pas** d'empiler des
providers Rust built-in indéfiniment — chaque provider est un PR
significatif sur le core, avec une maintenance long-terme à ma
charge. Post-1.0, l'ajout d'une langue passe par le **plug-in
layer UST + Lua** : tree-sitter parse, un plug-in Lua sandboxé définit
symboles / edges / attributs, le core Rust valide la conformité au
schéma IR. Ajouter Go / Java / Swift / Python / C / C++ devient un
fichier `.lua` posé dans le workspace, pas un PR core. Le core garde
ses providers natifs (Rust / TS-JS / Lua, + le support SFC
Vue/Svelte) ; tout le reste passe par plug-in. Voir
[vision long terme](storytelling/vision-long-terme.md).

## Ça marche avec un autre agent que Claude ?

Oui — Standardoc est un serveur MCP standard, consommable par
n'importe quel client MCP-aware (Cursor, Continue, Copilot Chat,
Aider, Goose, Cody, Claude Desktop, Claude Code…). **Mais** la
calibration de référence est faite sur **Claude Code en mode Opus**,
fenêtre 1M tokens. Les autres agents fonctionnent, avec des écarts :
certains shortcut vers grep dès que la tâche se complique, ou ignorent
le `routing_hint` correctif. La calibration est **tripartite** —
infrastructure + agent + opérateur.
Les hooks MCP-first (côté Claude Code) forcent la discipline ; pour un
autre client tu peux wirer des hooks équivalents (`standardoc claude
pre-tool-hook`). Détails dans
[retours de tests](storytelling/retours-tests.md).

## Comment installer ?

Deux voies. **Pre-built binaries** (canal recommandé) — télécharge
l'archive matching ta plateforme depuis
[releases/latest](https://github.com/miralabs-tech/standardoc/releases/latest),
le manifest `version.json` liste les SHA256 pour vérification. **OU
`cargo install --git`** pour un build source. Pour le flow VSCode
intégré, installe en plus l'extension Standardoc. Walkthrough complet :
[QUICKSTART.md](QUICKSTART.md).

> `cargo install standardoc-cli` depuis crates.io **n'est pas** le
> canal principal — trop lent pour les CI, exige une toolchain Rust.

## L'extension VSCode est-elle obligatoire ?

Non. Le CLI marche standalone : `standardoc lsp <ws>` (writer
principal) + `standardoc mcp <ws> --readonly` (transport stdio, ou
`--http 0` pour du multi-client HTTP/SSE), et tu connectes Claude
Desktop / Cursor / le client MCP de ton choix. L'extension rend juste
le flow seamless dans VSCode — supervision daemon, init opt-in flow,
génération du skill, merge `.mcp.json`. Le CLI deviendra encore plus
autosuffisant en beta.3 (`self-update`, injection PATH, one-liner
bootstrap).

## Mon code est-il envoyé quelque part ?

Non. **Standardoc est local-only**, sans condition. L'index vit dans
`.standardoc/` sur ton disque (gitignored, reproductible). Pas d'appel
réseau pour indexer, pas de télémétrie, pas de phone-home — **jamais, même
opt-in** : c'est un invariant culturel non négociable. Si Standardoc
disparaît demain, ton index continue de marcher.

## Comment ça performe sur de gros workspaces ?

AST natif + SQLite + FTS5 + watcher incrémental : le cold start se
compte en secondes sur un repo de taille moyenne (Standardoc s'indexe
lui-même en quelques secondes), l'overhead du watcher pendant les édits
est négligeable. Les **benchmarks de scale publiés** — cold start,
watcher delta, MCP query latency p99 sur monorepos 1M+ LOC — arrivent
à **1.0** : tournés en CI, attachés aux releases, régressent
visiblement quand on les casse. Pas de « ça scale, faites-nous
confiance » : les chiffres seront là avant qu'on fige le contrat.

## Standardoc est-il payant ? Y aura-t-il un SaaS ou un abonnement ?

Le core est et reste **gratuit et open-source**. **Pas de SaaS, pas
d'abonnement, pas de cloud, pas de télémétrie** — tant que Standardoc
n'a pas de composant serveur qui demande une infrastructure
récurrente, il n'y a aucune raison de facturer un abonnement, et aucun
n'est prévu. Si une tier payante émerge un jour (par exemple une UI
doc locale post-1.0), ce serait **local-only** (tourne sur ta machine,
pas d'hébergement) et en **licence à vie, achat unique** — et
seulement s'il y a une vraie demande. Le core, lui, reste FSL-1.1-MIT
→ MIT inchangé. Les discussions internes sur le financement futur
restent hors du discours public jusqu'à 1.0 — voir [SUPPORT.md](SUPPORT.md)
pour le modèle actuel (OpenCollective).

## Pourquoi FSL-1.1-MIT et pas MIT pur ?

[FSL-1.1-MIT](../../LICENSE) est permissive pour tout **usage
non-concurrent** et empêche le pattern « open-and-pillage » (un
concurrent closed-source qui forke sans rien publier). MIT pur a été
écarté — aucune protection court-terme ; AGPL aussi — insuffisant
contre des concurrents non-SaaS. FSL est le seul mécanisme qui combine
protection initiale **et** engagement irréversible d'ouverture : **deux
ans après chaque release, cette release convertit automatiquement en
MIT pur**. La première (`v1.0.0-beta.1`) convertit le **26 avril
2028**. À partir de là le core est légalement MIT pour toujours — peu
importe ce qui arrive à l'entreprise, au mainteneur, au marché.
Adoptée par Sentry, CodeCrafters, Keygen.

## Je peux l'utiliser commercialement ?

Oui, librement — tooling interne, apps customer-facing, produits SaaS
qui *utilisent* Standardoc. La seule limite : tu ne construis pas un
produit qui **se substitue à Standardoc lui-même** (le revendre comme
ton propre SaaS d'indexation). Voir la [licence](../../LICENSE) pour
les détails.

## Et le DSL v0 / le rendu de doc ?

Le DSL de templating v0 (expressions `{{ @doc.X }}` dans le Markdown) a
été **abandonné** en beta.1 — il devenait une seconde source à
maintenir, illisible pour les auteurs humains. Le remplacement, cible
**beta.3**, ne réinvente pas de DSL : `@standardoc/core` (query API) +
`@standardoc/react` (composants `<Doc id>`, `<Params id>`,
`<Examples id>`) consomment **directement le graphe**, qui reste la
seule source de vérité. Les annotations narratives s'appuient sur les
conventions déjà universelles — JSDoc, rustdoc, emmylua (`---@param`) —
pas sur un format custom. Voir
[vision moyen terme](storytelling/vision-moyen-terme.md).

## Puis-je contribuer ?

Avant le freeze 1.0 : **pas de PR tiers**. La surface API doit se
figer proprement, et accepter des PRs externes maintenant
introduirait du bruit sur des choix que je dois garder contrôlés
pour porter le contrat IR. **En revanche, issues / feedback /
idées techniques ou globales sont très bienvenus** via GitHub Issues /
Discussions — c'est là que remontent le plus vite les trous. Post-1.0,
le modèle s'ouvre : le plug-in layer UST + Lua est précisément conçu
pour absorber les contributions communautaires (langues, détecteurs
cross-substrat) sans toucher au core figé.

## Comment reporter un bug ou une faille de sécurité ?

Bugs et demandes de feature : [GitHub Issues](https://github.com/miralabs-tech/standardoc/issues).
**Failles de sécurité** : ne les poste pas publiquement — suis la
procédure de divulgation responsable décrite dans
[SECURITY.md](SECURITY.md).

---

## Pour aller plus loin

- **[Démarrage rapide](QUICKSTART.md)** — de zéro à un workspace indexé
- **[Philosophie](storytelling/philosophy.md)** — les 5 principes
  system-thinking et l'éthique de construction
- **[Comparaison](COMPARISON.md)** — vs LSP / Sourcegraph / Tree-sitter
  / autres
- **[Support](SUPPORT.md)** — comment soutenir le projet
- **[TODO-LIST](TODO-LIST.md)** — checkboxes exhaustives par milestone
