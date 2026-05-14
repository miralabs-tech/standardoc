# À propos de Standardoc

[English](ABOUT.md) · 📖 Français

> **Pitch en une ligne** — un graphe sémantique local de votre code, avec
> l'AST comme source de vérité, exposé via MCP pour que les agents IA
> arrêtent de grepper et commencent à requêter.

---

## 🧠 Le problème

Quand un agent IA répond *« qu'est-ce que la fonction X prend en entrée ? »*,
aujourd'hui il fait `grep -r "fn X" .` puis `cat src/foo.rs` puis devine.
Coût : **30k-100k tokens** par question, sur chaque conversation, sur
chaque projet.

Pour les humains le problème est parallèle. Les codebases modernes sont trop
complexes pour en garder un modèle mental. Les outils existants fragmentent
la réponse :

- **LSP** vous donne une résolution de symboles précise par langage mais pas
  de graphe cross-language ni de surface de requête AI-friendly.
- **Grep / Sourcegraph** vous donne de la navigation au niveau texte mais
  pas de sens sémantique — vous trouvez des occurrences, pas des relations.
- **JSDoc / TypeDoc / Sphinx** vous donne de la prose narrative,
  maintenue à la main, qui dérive perpétuellement du code qu'elle prétend
  décrire.

Standardoc fait le pont : un **index local unique** construit depuis l'AST,
exposé aux humains (LSP) et à l'IA (MCP) via un seul contrat stable.

---

## 💡 La thèse

### Le code est la source de vérité

L'AST est **structurellement exact par définition**. Standardoc parse le Rust
avec `syn` et le TypeScript avec `swc`, normalise les deux dans une
représentation intermédiaire canonique (symboles à clé FQDN, arêtes typées),
et persiste le graphe en SQLite avec FTS5 pour le fuzzy search.

Aucune annotation requise. L'AST dit la vérité — Standardoc se contente
d'écouter.

### La structure est dérivée

`<package>::<module>::<name>` est un identifiant stable à travers Rust et
TypeScript. Les arêtes sont typées : `CALLS`, `IMPORTS`, `EXTENDS`,
`IMPLEMENTS`, `REFERENCES`, `DEFINES`, `USES_TYPE`, `EXPOSES_API`. Les sauts
cross-language (Rust ↔ TS via Tauri commands, TS ↔ Rust via WASM bindings) se
matérialisent en arêtes `UnresolvedBridge` que les futurs plug-ins de bridge
résoudront.

### La compréhension est un système

Un graphe que vous pouvez requêter est plus utile que dix pages de doc qu'il
faut lire. Le serveur MCP expose deux outils :

- `find_symbol(query, limit?)` — recherche fuzzy FTS5 → liste de symboles.
- `get_context(fqdn, depth: 1|2)` — symbole + voisins (callers, callees,
  imports, imported_by) en tranche de graphe.

Deux outils. Contrat honnête. Les agents IA les apprennent une fois et les
réutilisent sur chaque projet.

---

## 🤖 Pourquoi MCP-first

Les serveurs MCP existants tombent dans deux camps :

- **Spécifiques à un produit** (Stripe MCP, Linear MCP, GitHub MCP) — utiles
  pour ce produit, inutiles pour *votre code*.
- **Spécifiques à une lib** (un MCP par framework) — fragmentation, charge de
  maintenance, ne couvre jamais ce que vous utilisez réellement.

Standardoc est le premier serveur MCP qui :

- Indexe **n'importe quelle codebase** (Rust + TS day-1, plus de langages
  post-beta.1).
- Marche **sans aucune annotation** — drop in, lance le cold start, requête.
- Expose **un contrat stable** que les agents apprennent une fois et
  réutilisent sur chaque projet.

Pour un agent IA, `get_context("myapp::server::handle_request", 1)` résolu
via MCP coûte ~100 tokens vs 10k-100k tokens de `grep + read` sur le repo.
**Gain de 100x à 1000x sur les tokens**, systématiquement.

---

## 🎯 Pourquoi Rust + TypeScript en premier

Deux langages. Tous deux populaires. Tous deux ont des parsers natifs de
qualité (`syn`, `swc`). Tous deux courants dans les stacks modernes (backend
Rust + frontend TS, apps desktop Tauri, extensions web).

Réduire le scope à deux langages permet de **perfectionner la base** avant
d'élargir. Une fois Rust et TS solides comme le roc (unification FQDN,
bridges cross-language, résolution d'arêtes, perf sur des monorepos de 200k
LOC), ajouter Python / Go / Java / Swift via tree-sitter ou des parsers
natifs devient un copy-paste du trait language provider.

L'alternative — ship 10 langages avec une profondeur médiocre — c'est ce que
font les outils existants, et c'est pourquoi aucun ne donne aux agents IA une
surface sémantique utile.

---

## 🔓 Posture open-core

**Standardoc Core** — CLI, LSP, MCP, tous les language providers, extension
VSCode. Source sous [FSL-1.1-MIT](../LICENSE) — convertit en **MIT pur le
26 avril 2028**. Gratuit pour tout usage non-concurrent aujourd'hui,
entièrement MIT à partir de cette date.

Le focus reste le Core open-source. Tant que Standardoc n'a pas de
composant cloud/serveur qui justifierait une infrastructure récurrente,
il n'y a **aucune raison de proposer un abonnement** — et aucun n'est
prévu. Si un outil compagnon voit le jour (genre UI façon GitBook locale
qui tourne sur votre machine, sans hébergement), ce sera sous **licence
à vie achat unique**. Dans tous les cas, le Core reste OSS et la date de
conversion MIT ci-dessus est verrouillée.

Pas de SaaS. Pas d'abonnement par siège. Pas de télémétrie. Pas de modal
upsell dans votre IDE.

---

## 🚀 Direction long-terme

À venir :

- **Plus de language providers** — Python / Go / Java / C# / Swift / Zig via
  parsers natifs ou tree-sitter.
- **Plug-ins de bridges cross-language** (WASM) — résolution Tauri commands,
  WASM bindings, déclarations FFI résolues à travers le graphe.
- **Virtual annotations** — descriptions de doc synthétisées pour les
  symboles publics non-documentés (conventions verb-prefix, narratives
  type-signature, templates trait impl).
- **Couche de rendering MDX/React** — package npm exposant
  `<Doc id="user.create" />`, `<Params id="user.create" />`,
  `<Examples id="user.create" />`, `queryDocs("api.*")`. Drop-in pour
  Next/Nextra/Astro/Docusaurus/… Le doc graph (SQLite) alimente la couche
  de rendering ; pas de moteur de template, pas de DSL custom — juste du
  MDX avec des queries structurées. Cible beta.2.
- **Outil compagnon UI façon GitBook (optionnel)** — si l'idée se
  concrétise, local-only (tourne sur votre machine, pas d'hébergement),
  licence à vie achat unique. Décidé selon l'adoption.

Le backlog complet et le découpage des milestones vivent dans [TODO-LIST.md](TODO-LIST.md).

---

## 🙋 Pourquoi ce projet existe

Je suis le seul mainteneur, je travaille sur Standardoc en plus d'un job à
plein temps. Des années à voir des agents IA brûler des milliers de tokens
pour comprendre ce que fait une fonction de 5 lignes, et à voir la
documentation dériver loin du code qu'elle prétend décrire. Chaque outil de
doc essayé résolvait une partie du problème et repoussait le reste sur le
travail manuel.

Il fallait une source de vérité unique que les humains puissent lire et que
l'IA puisse requêter — sans que je doive la réécrire pour chaque
consommateur. C'est Standardoc.

Si ça vous fait gagner du temps, [soutenez le projet](SUPPORT.fr.md).
Si vous trouvez un bug, [ouvrez une issue](https://github.com/miralabs-tech/standardoc/issues).
