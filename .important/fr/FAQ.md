# FAQ

[English](../en/FAQ.md) · 📖 Français &nbsp;|&nbsp; ← [README](README.md) · [Démarrage rapide](QUICKSTART.md) · [Roadmap](TODO-LIST.md)

---

## Ça remplace mon LSP ?

Non — ça le complète. Standardoc *expose* LSP comme une surface, mais sous le
capot c'est un graphe cross-langage global, pas un serveur per-langage.
`rust-analyzer` / `tsserver` gardent la résolution per-langage profonde
(inférence de types, expansion macro) ; Standardoc apporte le graphe
transverse + la surface MCP. Utilise les deux.

## En quoi c'est différent de Sourcegraph ?

Sourcegraph est un moteur de recherche d'équipe hébergé (cloud), centré sur
la collaboration et la review. Standardoc est de l'indexation sémantique
**locale** pour agents IA et outils — pas de cloud, pas d'auth, pas de
facturation par siège ; l'index vit dans `.standardoc/` sur ta machine. Les
deux peuvent coexister.

## Pourquoi pas tree-sitter ou ripgrep directement ?

Sur un petit projet, `ripgrep` + ton IDE suffisent. Tree-sitter standalone
donne un AST *de surface* (fonctions / classes / appels). Standardoc utilise
des parsers *profonds* — `syn`, `swc`, `full_moon`, SFC custom — avec
signatures complètes, types, génériques, traits, et arêtes typées.
(Tree-sitter revient post-1.0, mais *sous* le plug-in layer UST + Lua, pas
comme indexeur de surface.)

## Quels langages ?

Natifs : **Rust** (`syn`), **TypeScript / JavaScript** dont JSX / TSX / React
(`swc`), **Lua** (`full_moon`), et **C** (avec join cross-fichier `.h` ↔
`.c`). Plus **Vue** et **Svelte** via parsing SFC. Le critère n'est pas le
nombre de langages — c'est la profondeur d'AST.

## Python / Go / Java / … c'est pour quand ?

Pas comme providers core built-in. Post-1.0 ils passent par le **plug-in
layer UST + Lua** : tree-sitter parse, un plug-in Lua sandboxé mappe symboles
/ arêtes, le core Rust valide contre l'IR — un fichier `.lua` posé dans le
workspace, pas une PR sur le core. Voir la [roadmap](TODO-LIST.md).

## Ça marche avec un agent autre que Claude ?

Oui — c'est un serveur MCP standard (Cursor, Continue, Copilot, Aider, Goose,
Cody, Claude Desktop / Code, …). La calibration est réglée sur Claude Code
(Opus) ; les autres agents marchent mais varient — certains shortcut vers
grep quand la tâche se corse. Les hooks MCP-first imposent la discipline côté
Claude Code ; câble l'équivalent ailleurs via `standardoc claude
pre-tool-hook`.

## Ça génère de la doc (façon TypeDoc) ?

Pas encore. Standardoc est un indexeur sémantique aujourd'hui. Une couche de
rendu (`@standardoc/core` + `@standardoc/react`, nourrie directement par le
graphe) est prévue mais a **glissé après beta.3** — voir la
[roadmap](TODO-LIST.md).

## Mon code part quelque part ?

Non. **Local-only, sans condition.** L'index vit dans `.standardoc/` sur ton
disque ; aucun appel réseau pour indexer, pas de télémétrie, pas de
phone-home — jamais, même opt-in. Si Standardoc disparaissait demain, ton
index continue de marcher.

## Ça tient sur les gros workspaces ?

AST natif + SQLite + FTS5 + watcher incrémental — cold start en secondes sur
un repo moyen (Standardoc s'indexe lui-même en quelques secondes). Les
benchmarks de scale publiés (1M+ LOC ; cold start / delta watcher / latence
query p99) arrivent à 1.0, tournent en CI — pas de « ça scale, faites-nous
confiance ».

## C'est payant ? Un SaaS ?

Le core est et reste **gratuit, open-source, local**. Pas de SaaS, pas
d'abonnement, pas de cloud. Si un tier payant apparaît un jour (ex. une UI
doc locale), il serait local-only, à vie en achat unique, et seulement sur
demande réelle. Le core reste FSL → MIT.

## Pourquoi FSL-1.1-MIT et pas MIT pur ?

[FSL-1.1-MIT](../../LICENSE) est permissive pour tout usage non-concurrent et
bloque les concurrents « fork-and-close ». MIT pur ne donne aucune protection
court-terme ; AGPL ne couvre pas les concurrents non-SaaS. FSL combine la
protection maintenant avec une ouverture irréversible : **chaque release se
convertit automatiquement en MIT pur deux ans plus tard** (la première : 26
avril 2028). L'usage commercial est OK — tu ne peux juste pas revendre
Standardoc lui-même comme ton propre produit d'indexation.

## Je peux contribuer ?

Avant le freeze 1.0 : **pas de PR tierces** (l'API doit d'abord se stabiliser
proprement). Mais issues, feedback et idées sont très bienvenus via GitHub.
Post-1.0 s'ouvre — le plug-in layer UST + Lua est fait pour absorber les
langages / détecteurs communautaires sans toucher au core figé.

## Bug ou problème de sécurité ?

Bugs / features : [GitHub Issues](https://github.com/miralabs-tech/standardoc/issues).
Sécurité : ne le poste pas publiquement — suis [SECURITY.md](SECURITY.md).

---

← [README](README.md) · [Démarrage rapide](QUICKSTART.md) · [Roadmap](TODO-LIST.md)
