# FAQ

[English](../en/FAQ.md) · 📖 Français &nbsp;|&nbsp; ← [README](README.md) · [Démarrage rapide](QUICKSTART.md) · [Roadmap](TODO-LIST.md)

> **⚠️ Archivé (2026-09-17).** Standardoc est archivé sur **v1.0.0-beta.2** ;
> le [README](README.md) explique pourquoi. Les réponses ci-dessous ont été
> réécrites en conséquence : rien de « prévu », « post-1.0 » ou « à la 1.0 »
> n'arrivera.

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

Jamais. Le projet est archivé. Le **plug-in layer UST + Lua** prévu n'a
jamais été commencé ; les six providers listés ci-dessus sont le jeu final.

## Ça marche avec un agent autre que Claude ?

Côté protocole c'est un serveur MCP standard, donc n'importe quel client
peut se connecter. Seul Claude Code a été testé — aucune fixture, aucun run
CI pour Cursor, Continue, Copilot ou les autres. Les hooks MCP-first qu'il
installait pour Claude Code ont bloqué plus de travail qu'ils n'en ont aidé
et ne sont pas recommandés.

## Ça génère de la doc (façon TypeDoc) ?

Non, et ça ne le fera jamais. `@standardoc/core` / `@standardoc/react`
n'ont jamais été commencés ; Standardoc est resté un indexeur sémantique.

## Mon code part quelque part ?

Non. **Local-only, sans condition.** L'index vit dans `.standardoc/` sur ton
disque ; aucun appel réseau pour indexer, pas de télémétrie, pas de
phone-home — jamais, même opt-in. Si Standardoc disparaissait demain, ton
index continue de marcher.

## Ça tient sur les gros workspaces ?

Inconnu. Ça n'a tourné que sur ce dépôt et quelques petits projets de
l'auteur. Les benchmarks 1M+ LOC n'ont jamais été construits, donc aucun
claim de scale à faire.

## C'est payant ? Un SaaS ?

Non. Le core est **gratuit, open-source, local**, et aucun tier payant n'a
jamais existé. La licence reste FSL → MIT.

## Pourquoi FSL-1.1-MIT et pas MIT pur ?

[FSL-1.1-MIT](../../LICENSE) est permissive pour tout usage non-concurrent et
bloque les concurrents « fork-and-close ». MIT pur ne donne aucune protection
court-terme ; AGPL ne couvre pas les concurrents non-SaaS. FSL combine la
protection maintenant avec une ouverture irréversible : **chaque release se
convertit automatiquement en MIT pur deux ans plus tard** (la première : 26
avril 2028). L'usage commercial est OK — tu ne peux juste pas revendre
Standardoc lui-même comme ton propre produit d'indexation.

## Je peux contribuer ?

Le dépôt est archivé (lecture seule). Forke-le sous les termes de la
[FSL-1.1-MIT](../../LICENSE) si tu veux le reprendre.

## Bug ou problème de sécurité ?

Rien ne sera corrigé. Les signalements de sécurité sont encore lus — voir
[SECURITY.md](SECURITY.md) — mais aucun correctif ne sortira.

---

← [README](README.md) · [Démarrage rapide](QUICKSTART.md) · [Roadmap](TODO-LIST.md)
